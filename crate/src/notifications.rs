//! "Subtitles found" notifications: a pop-up inside Jenova while its window
//! is showing, a Windows notification while it's hidden in the tray.
//!
//! Download workers call [`subtitle_found`]. A background thread batches
//! results so a big scan produces a few summary toasts rather than hundreds.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;

use crate::subtitle_utils::SubtitleUtils;
#[allow(unused_imports)]
use crate::{debug, warn};

/// Windows AppUserModelID the toasts are sent under.
#[cfg(windows)]
const APP_ID: &str = "AnimaTheAeon.Jenova";
/// Wait after the first result so near-simultaneous ones are grouped.
const GATHER: Duration = Duration::from_millis(1500);
/// Minimum gap between desktop toasts.
const MIN_GAP: Duration = Duration::from_secs(8);
/// Results listed individually before switching to a summary.
const MAX_INDIVIDUAL: usize = 3;
/// How long an in-app pop-up stays up.
pub const IN_APP_LIFETIME: Duration = Duration::from_secs(7);
const MAX_IN_APP: usize = 3;

/// The "Notify me when subtitles are found" setting.
pub static ENABLED: AtomicBool = AtomicBool::new(true);

struct Found {
    video: PathBuf,
    langs: Vec<String>,
}

/// One message ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub title: String,
    pub body: String,
}

/// An in-app pop-up and when it appeared.
#[derive(Clone)]
pub struct InAppToast {
    pub toast: Toast,
    pub shown_at: Instant,
}

static QUEUE: Lazy<Mutex<Vec<Found>>> = Lazy::new(|| Mutex::new(Vec::new()));
static IN_APP: Lazy<Mutex<VecDeque<InAppToast>>> = Lazy::new(|| Mutex::new(VecDeque::new()));
static CTX: Lazy<Mutex<Option<eframe::egui::Context>>> = Lazy::new(|| Mutex::new(None));
static WORKER: Lazy<Mutex<Option<std::sync::mpsc::Sender<()>>>> = Lazy::new(|| Mutex::new(None));

/// Let the notifier wake the UI.
pub fn set_context(ctx: &eframe::egui::Context) {
    let mut slot = CTX.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(ctx.clone());
    }
}

/// Report that subtitles were downloaded for `video` in `langs`.
pub fn subtitle_found(video: &Path, langs: &[String]) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(Found {
        video: video.to_path_buf(),
        langs: langs.to_vec(),
    });
    let mut worker = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    if worker
        .as_ref()
        .map(|tx| tx.send(()).is_err())
        .unwrap_or(true)
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(());
        *worker = Some(tx);
        std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || run_worker(rx))
            .ok();
    }
}

/// Current in-app pop-ups, oldest first; expired ones are dropped.
pub fn in_app_toasts() -> Vec<InAppToast> {
    let mut list = IN_APP.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|t| t.shown_at.elapsed() < IN_APP_LIFETIME);
    list.iter().cloned().collect()
}

pub fn dismiss_in_app(index: usize) {
    let mut list = IN_APP.lock().unwrap_or_else(|e| e.into_inner());
    if index < list.len() {
        list.remove(index);
    }
}

fn run_worker(rx: std::sync::mpsc::Receiver<()>) {
    let mut last_desktop: Option<Instant> = None;
    while rx.recv().is_ok() {
        std::thread::sleep(GATHER);
        if let Some(last) = last_desktop {
            let since = last.elapsed();
            if since < MIN_GAP {
                std::thread::sleep(MIN_GAP - since);
            }
        }
        // Absorb wake-ups that arrived while waiting.
        while rx.try_recv().is_ok() {}
        let found: Vec<Found> =
            std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()));
        if found.is_empty() {
            continue;
        }
        let toasts = build_toasts(&found);
        if crate::tray::window_visible() {
            let mut list = IN_APP.lock().unwrap_or_else(|e| e.into_inner());
            for toast in &toasts {
                list.push_back(InAppToast {
                    toast: toast.clone(),
                    shown_at: Instant::now(),
                });
            }
            while list.len() > MAX_IN_APP {
                list.pop_front();
            }
            drop(list);
            if let Some(ctx) = CTX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                ctx.request_repaint();
            }
        } else {
            for toast in &toasts {
                show_desktop(toast);
            }
            last_desktop = Some(Instant::now());
        }
    }
}

/// Turn a batch of results into one toast each, or a single summary.
fn build_toasts(found: &[Found]) -> Vec<Toast> {
    if found.len() <= MAX_INDIVIDUAL {
        return found
            .iter()
            .map(|f| Toast {
                title: format!("Subtitles found: {}", language_list(&f.langs)),
                body: pretty_title(&f.video),
            })
            .collect();
    }
    let mut langs: Vec<String> = Vec::new();
    for f in found {
        for lang in &f.langs {
            if !langs.contains(lang) {
                langs.push(lang.clone());
            }
        }
    }
    let names: Vec<String> = found
        .iter()
        .take(3)
        .map(|f| pretty_title(&f.video))
        .collect();
    let mut body = names.join("\n");
    if found.len() > 3 {
        body.push_str(&format!("\n…and {} more", found.len() - 3));
    }
    vec![Toast {
        title: format!(
            "Subtitles found for {} videos ({})",
            found.len(),
            language_list(&langs)
        ),
        body,
    }]
}

fn language_list(codes: &[String]) -> String {
    if codes.is_empty() {
        return "subtitles".into();
    }
    codes
        .iter()
        .map(|code| SubtitleUtils::language_code_to_name(code).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// "Some.Show.S01E02.1080p.WEB" -> "Some Show S01E02"; "Arrival.2016.1080p" -> "Arrival (2016)".
pub fn pretty_title(video: &Path) -> String {
    let stem = video
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned = stem.replace(['.', '_'], " ");
    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    for token in &tokens {
        let bare = token.trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']');
        if is_episode(bare) {
            out.push(bare.to_uppercase());
            break;
        }
        if is_year(bare) && !out.is_empty() {
            out.push(format!("({bare})"));
            break;
        }
        out.push(token.to_string());
    }
    let title = out.join(" ");
    if title.is_empty() {
        stem
    } else {
        title
    }
}

fn is_year(s: &str) -> bool {
    s.len() == 4
        && s.chars().all(|c| c.is_ascii_digit())
        && matches!(s.parse::<u32>(), Ok(1900..=2099))
}

fn is_episode(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let b = lower.as_bytes();
    if b.len() < 6 || b[0] != b's' {
        return false;
    }
    let Some(e) = lower.find('e') else {
        return false;
    };
    e > 1
        && lower[1..e].chars().all(|c| c.is_ascii_digit())
        && lower[e + 1..].len() >= 2
        && lower[e + 1..e + 3].chars().all(|c| c.is_ascii_digit())
}

#[cfg(windows)]
fn show_desktop(toast: &Toast) {
    use std::sync::Once;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(register_app_id);
    let result = notify_rust::Notification::new()
        .app_id(APP_ID)
        .summary(&toast.title)
        .body(&toast.body)
        .show();
    if let Err(error) = result {
        warn!("Desktop notification failed: {error}");
    }
}

#[cfg(not(windows))]
fn show_desktop(toast: &Toast) {
    let result = notify_rust::Notification::new()
        .appname("Jenova")
        .summary(&toast.title)
        .body(&toast.body)
        .show();
    if let Err(error) = result {
        debug!("Desktop notification failed: {error}");
    }
}

/// Register the app's name and icon so Windows shows toasts as "Jenova"
/// (unpackaged apps need this, or toasts are silently dropped).
#[cfg(windows)]
fn register_app_id() {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let icon_path = crate::helper_functions::Utils::app_data_dir()
        .ok()
        .map(|dir| dir.join("jenova.png"));
    if let Some(path) = icon_path.as_ref() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, include_bytes!("../resources/jenova_icon_256.png"));
    }
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}")) {
        Ok((key, _)) => {
            let _ = key.set_value("DisplayName", &"Jenova");
            if let Some(path) = icon_path {
                let _ = key.set_value("IconUri", &path.to_string_lossy().into_owned());
            }
            let _ = key.set_value("IconBackgroundColor", &"FF12207A");
        }
        Err(error) => {
            warn!("Could not register notification app id: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cleaned_up() {
        assert_eq!(
            pretty_title(Path::new("Arrival.2016.1080p.BluRay.mkv")),
            "Arrival (2016)"
        );
        assert_eq!(
            pretty_title(Path::new("Some.Show.s01e02.1080p.WEB.mkv")),
            "Some Show S01E02"
        );
        assert_eq!(pretty_title(Path::new("Home Movie.mp4")), "Home Movie");
        assert_eq!(pretty_title(Path::new("2012.2009.mkv")), "2012 (2009)");
    }

    #[test]
    fn small_batches_are_individual_and_big_ones_summarised() {
        let found = |name: &str, lang: &str| Found {
            video: PathBuf::from(name),
            langs: vec![lang.to_string()],
        };
        let one = build_toasts(&[found("Heat.1995.mkv", "en")]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].title, "Subtitles found: English");
        assert_eq!(one[0].body, "Heat (1995)");

        let many: Vec<Found> = (1..=5)
            .map(|i| {
                found(
                    &format!("Show.S01E0{i}.mkv"),
                    if i == 5 { "es" } else { "en" },
                )
            })
            .collect();
        let summary = build_toasts(&many);
        assert_eq!(summary.len(), 1);
        assert_eq!(
            summary[0].title,
            "Subtitles found for 5 videos (English, Spanish)"
        );
        assert!(summary[0].body.ends_with("…and 2 more"));
    }
}
