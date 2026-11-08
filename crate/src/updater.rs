//! Self-updater backed by GitHub Releases.
//!
//! On launch (and on demand) the latest release is fetched. If it's newer
//! than this build the UI asks the user. On "Yes" the release's `jenova.exe`
//! (bare, or inside a .zip) is downloaded, checked against GitHub's SHA-256
//! digest, swapped in for the running exe, and the app restarts.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::config::APP_VERSION;
use crate::{info, warn};

pub const RELEASES_API: &str = "https://api.github.com/repos/AnimaTheAeon/Jenova/releases/latest";
const USER_AGENT: &str = "jenova-updater";
/// Suffix for the replaced exe, deleted on the next launch.
const OLD_SUFFIX: &str = "old";

/// A release asset that can be installed.
#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex SHA-256, when GitHub provides it.
    pub sha256: Option<String>,
}

/// A newer release.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub title: String,
    pub notes: String,
    pub page_url: String,
    pub asset: Option<Asset>,
}

/// Where the updater is, for the UI.
#[derive(Clone, Debug, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    /// Newer release found; waiting for Yes/No.
    Available(Release),
    UpToDate,
    Downloading {
        release: Release,
    },
    /// Downloaded and swapped in; restart pending.
    Installed,
    Failed(String),
}

/// Shared between the UI and the update threads.
#[derive(Default)]
pub struct Updater {
    pub state: Mutex<UpdateState>,
    pub downloaded: AtomicU64,
    pub total: AtomicU64,
    /// User pressed "No" for this session.
    pub dismissed: std::sync::atomic::AtomicBool,
    /// The check was started by the user (show "up to date" / errors).
    pub manual: std::sync::atomic::AtomicBool,
}

impl Updater {
    pub fn state(&self) -> UpdateState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, state: UpdateState) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = state;
    }

    pub fn reset(&self) {
        self.set(UpdateState::Idle);
    }
}

/// Start a release check in the background.
pub fn check_in_background(
    updater: Arc<Updater>,
    manual: bool,
    ctx: Option<eframe::egui::Context>,
) {
    if matches!(
        updater.state(),
        UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installed
    ) {
        return;
    }
    updater.manual.store(manual, Ordering::SeqCst);
    if manual {
        updater.dismissed.store(false, Ordering::SeqCst);
    }
    updater.set(UpdateState::Checking);
    std::thread::spawn(move || {
        let state = match fetch_latest() {
            Ok(Some(release)) => {
                info!("Update available: v{}", release.version);
                UpdateState::Available(release)
            }
            Ok(None) => UpdateState::UpToDate,
            Err(error) => {
                warn!("Update check failed: {}", error);
                UpdateState::Failed(error)
            }
        };
        updater.set(state);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
}

/// Download and install `release`, then restart into the new version.
pub fn install_in_background(
    updater: Arc<Updater>,
    release: Release,
    ctx: Option<eframe::egui::Context>,
) {
    updater.set(UpdateState::Downloading {
        release: release.clone(),
    });
    updater.downloaded.store(0, Ordering::SeqCst);
    updater.total.store(0, Ordering::SeqCst);
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let asset = release
                .asset
                .clone()
                .ok_or("This release has no Windows download (jenova.exe or a .zip).")?;
            let bytes = download(&asset, &updater, ctx.as_ref())?;
            let exe_bytes = extract_exe(&asset.name, bytes)?;
            // The background watcher runs the same exe; stop it first.
            crate::background::stop_all_background();
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            replace_exe(&exe, &exe_bytes)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                info!("Updated to v{}; restarting", release.version);
                updater.set(UpdateState::Installed);
                if let Some(ctx) = ctx.as_ref() {
                    ctx.request_repaint();
                }
                std::thread::sleep(Duration::from_millis(800));
                restart(ctx.as_ref());
            }
            Err(error) => {
                warn!("Update failed: {}", error);
                updater.set(UpdateState::Failed(error));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            }
        }
    });
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// Latest release if it's newer than this build.
pub fn fetch_latest() -> Result<Option<Release>, String> {
    // JENOVA_UPDATE_URL lets tests point at a local server.
    let url = std::env::var("JENOVA_UPDATE_URL").unwrap_or_else(|_| RELEASES_API.to_string());
    let response = client()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Err("No releases found (is the Jenova repo public?)".into()),
        403 | 429 => return Err("GitHub rate limit reached; try again later".into()),
        code => return Err(format!("GitHub returned HTTP {code}")),
    }
    let json: serde_json::Value = response.json().map_err(|e| e.to_string())?;
    Ok(parse_release(&json).filter(|release| is_newer(&release.version, APP_VERSION)))
}

/// Build a [`Release`] from the GitHub API JSON.
pub fn parse_release(json: &serde_json::Value) -> Option<Release> {
    let text = |key: &str| {
        json.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let tag = text("tag_name");
    let name = text("name");
    let version = extract_version(&tag).or_else(|| extract_version(&name))?;
    let assets: Vec<Asset> = json
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a.get("name")?.as_str()?.to_string(),
                        url: a.get("browser_download_url")?.as_str()?.to_string(),
                        size: a.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                        sha256: a
                            .get("digest")
                            .and_then(|d| d.as_str())
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .map(|d| d.to_ascii_lowercase()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release {
        version,
        title: if name.is_empty() { tag } else { name },
        notes: text("body"),
        page_url: text("html_url"),
        asset: pick_asset(&assets),
    })
}

/// Prefer jenova.exe, then any .exe, then a .zip.
fn pick_asset(assets: &[Asset]) -> Option<Asset> {
    let lower = |a: &Asset| a.name.to_ascii_lowercase();
    assets
        .iter()
        .find(|a| lower(a) == "jenova.exe")
        .or_else(|| assets.iter().find(|a| lower(a).ends_with(".exe")))
        .or_else(|| assets.iter().find(|a| lower(a).ends_with(".zip")))
        .cloned()
}

/// First version-looking token: "v1.2.0" -> "1.2.0", "Jenova  v1" -> "1".
pub fn extract_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() {
            continue;
        }
        // Must start a token or follow a 'v'.
        if start > 0 {
            let prev = bytes[start - 1];
            if prev.is_ascii_alphanumeric() && prev != b'v' && prev != b'V' {
                continue;
            }
        }
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
            end += 1;
        }
        let version = text[start..end].trim_end_matches('.');
        if !version.is_empty() {
            return Some(version.to_string());
        }
    }
    None
}

/// Numeric dotted compare; missing parts count as 0.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches(['v', 'V'])
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

fn download(
    asset: &Asset,
    updater: &Updater,
    ctx: Option<&eframe::egui::Context>,
) -> Result<Vec<u8>, String> {
    let mut response = client()?
        .get(&asset.url)
        .send()
        .map_err(|e| format!("Download failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed: HTTP {}", response.status()));
    }
    let total = response.content_length().unwrap_or(asset.size);
    updater.total.store(total, Ordering::SeqCst);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        updater
            .downloaded
            .store(bytes.len() as u64, Ordering::SeqCst);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }
    if let Some(expected) = &asset.sha256 {
        let actual = hex(&Sha256::digest(&bytes));
        if &actual != expected {
            return Err("Download was corrupted (checksum mismatch). Nothing was changed.".into());
        }
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Get the exe bytes from a bare exe or a zip containing one.
pub fn extract_exe(name: &str, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if !name.to_ascii_lowercase().ends_with(".zip") {
        return check_exe(bytes);
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Couldn't open the update zip: {e}"))?;
    let mut fallback = None;
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| e.to_string())?;
        let entry = file.name().map_err(|e| e.to_string())?.to_ascii_lowercase();
        let base = entry.rsplit('/').next().unwrap_or(&entry).to_string();
        if base == "jenova.exe" {
            fallback = Some(i);
            break;
        }
        if base.ends_with(".exe") && fallback.is_none() {
            fallback = Some(i);
        }
    }
    let index = fallback.ok_or("The update zip doesn't contain jenova.exe")?;
    let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    file.read_to_end(&mut out).map_err(|e| e.to_string())?;
    check_exe(out)
}

/// Basic sanity check that this is a Windows executable.
fn check_exe(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if cfg!(windows) && !bytes.starts_with(b"MZ") {
        return Err("The downloaded file isn't a Windows program. Nothing was changed.".into());
    }
    if bytes.len() < 1024 {
        return Err("The downloaded file is too small. Nothing was changed.".into());
    }
    Ok(bytes)
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{OLD_SUFFIX}"));
    exe.with_file_name(name)
}

/// Where to move the running exe. Clears a leftover from an earlier update
/// first; if that leftover is still locked (an old Jenova never exited), its
/// process is ended, and failing that a fresh name is used so the update
/// isn't blocked.
fn aside_path(exe: &Path) -> PathBuf {
    let old = old_path(exe);
    if !old.exists() || std::fs::remove_file(&old).is_ok() {
        return old;
    }
    end_processes_running(&old);
    for _ in 0..10 {
        if std::fs::remove_file(&old).is_ok() || !old.exists() {
            return old;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let stamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    let mut name = old.file_name().unwrap_or_default().to_os_string();
    name.push(format!("-{stamp}"));
    old.with_file_name(name)
}

/// Leftover exes from earlier updates next to `exe` ("jenova.exe.old",
/// "jenova.exe.old-20261108...").
fn leftover_exes(exe: &Path) -> Vec<PathBuf> {
    let prefix = old_path(exe)
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .map(|n| n.to_string_lossy().to_ascii_lowercase())
                            .is_some_and(|n| n == prefix || n.starts_with(&format!("{prefix}-")))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Swap the running exe for `new_bytes`. A running exe can't be overwritten
/// on Windows but it can be renamed, so: write new -> rename old aside ->
/// move new into place. Rolls back if the last step fails.
pub fn replace_exe(exe: &Path, new_bytes: &[u8]) -> Result<(), String> {
    let staged = exe.with_extension("new");
    std::fs::write(&staged, new_bytes).map_err(|e| format!("Couldn't save the update: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    let old = aside_path(exe);
    if let Err(error) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "Windows wouldn't let Jenova replace itself: {error}. Close any other Jenova windows and try again, or download the new version from the release page."
        ));
    }
    if let Err(error) = std::fs::rename(&staged, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&staged);
        return Err(format!("Couldn't install the update: {error}"));
    }
    Ok(())
}

/// Argument passed to the new exe after an update.
pub const AFTER_UPDATE_ARG: &str = "--after-update";

/// Delete the exe left over from the last update (call at startup, off the
/// UI thread). An old process that never exited is ended first; if the file
/// still can't be deleted it's moved into %APPDATA%\\Jenova\\old and removed
/// on a later launch.
pub fn cleanup_old_exe() {
    purge_old_versions_folder();
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    for old in leftover_exes(&exe) {
        remove_leftover(&old);
    }
}

fn remove_leftover(old: &Path) {
    let try_delete = |tries: u32| {
        for _ in 0..tries {
            if std::fs::remove_file(old).is_ok() || !old.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        false
    };
    // Give the previous version a moment to finish closing.
    if try_delete(20) {
        info!("Removed leftover {}", old.display());
        return;
    }
    // Still locked: an old Jenova is still running from that file.
    warn!("{} is still in use; ending the old process", old.display());
    end_processes_running(old);
    if try_delete(20) {
        info!("Removed leftover {}", old.display());
        return;
    }
    // Last resort: get it out of the exe's folder.
    if let Some(dir) = old_versions_dir() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let target = dir.join(format!("jenova-{stamp}.exe.old"));
        match std::fs::rename(old, &target) {
            Ok(()) => {
                info!("Moved leftover exe to {}", target.display());
            }
            Err(error) => {
                warn!("Couldn't remove {}: {}", old.display(), error);
            }
        }
    }
}

fn old_versions_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = crate::helper_functions::Utils::app_data_dir().ok();
    #[cfg(not(windows))]
    let base = dirs::cache_dir().map(|d| d.join("jenova"));
    let dir = base?.join("old");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Delete parked old versions that are no longer in use.
fn purge_old_versions_folder() {
    let Some(dir) = old_versions_dir() else {
        return;
    };
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// End any process whose program file is `path`.
fn end_processes_running(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let quoted = path.to_string_lossy().replace('\'', "''");
        let script = format!(
            "Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -eq '{quoted}' -and $_.ProcessId -ne {me} }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
            me = std::process::id()
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// Launch the new exe, then close this window through the normal shutdown
/// path (ending the process from a worker thread can hang it on Windows,
/// keeping the old exe locked).
fn restart(ctx: Option<&eframe::egui::Context>) {
    if let Ok(exe) = std::env::current_exe() {
        // current_exe may now point at the renamed ".old" file on some systems.
        let exe = {
            let s = exe.to_string_lossy();
            match s.strip_suffix(&format!(".{OLD_SUFFIX}")) {
                Some(original) => PathBuf::from(original),
                None => exe.clone(),
            }
        };
        let _ = std::process::Command::new(exe)
            .arg(AFTER_UPDATE_ARG)
            .spawn();
    }
    match ctx {
        Some(ctx) => {
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
            ctx.request_repaint();
            // Safety net if the window doesn't close.
            std::thread::sleep(Duration::from_secs(10));
            std::process::exit(0);
        }
        None => std::process::exit(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_extracted_from_tags_and_titles() {
        assert_eq!(extract_version("v1.2.0").as_deref(), Some("1.2.0"));
        assert_eq!(extract_version("Jenova  v1").as_deref(), Some("1"));
        assert_eq!(extract_version("Release 2.0").as_deref(), Some("2.0"));
        assert_eq!(extract_version("App"), None);
        assert_eq!(extract_version("x64build").as_deref(), None);
    }

    #[test]
    fn version_compare() {
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(is_newer("v2", "1.9.9"));
        assert!(!is_newer("1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.1.0"));
        assert!(is_newer("1.0.10", "1.0.9"));
    }

    #[test]
    fn manual_zip_release_is_understood() {
        let json = serde_json::json!({
            "tag_name": "App", "name": "Jenova  v1", "body": "notes",
            "html_url": "https://github.com/AnimaTheAeon/Jenova/releases/tag/App",
            "assets": [{"name": "jenova-windows-x64.zip", "size": 10,
                        "browser_download_url": "https://example.com/a.zip",
                        "digest": "sha256:ABC"}]
        });
        let release = parse_release(&json).unwrap();
        assert_eq!(release.version, "1");
        let asset = release.asset.unwrap();
        assert_eq!(asset.name, "jenova-windows-x64.zip");
        assert_eq!(asset.sha256.as_deref(), Some("abc"));
    }

    #[test]
    fn exe_asset_preferred_over_zip() {
        let json = serde_json::json!({
            "tag_name": "v1.1.0", "assets": [
                {"name": "source.zip", "browser_download_url": "z"},
                {"name": "jenova.exe", "browser_download_url": "e"}]
        });
        assert_eq!(
            parse_release(&json).unwrap().asset.unwrap().name,
            "jenova.exe"
        );
    }

    #[test]
    fn exe_is_pulled_out_of_a_zip() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("readme.txt", opts).unwrap();
            std::io::Write::write_all(&mut zip, b"hi").unwrap();
            zip.start_file("jenova.exe", opts).unwrap();
            let mut exe = b"MZ".to_vec();
            exe.resize(4096, 7);
            std::io::Write::write_all(&mut zip, &exe).unwrap();
            zip.finish().unwrap();
        }
        let out = extract_exe("jenova-windows-x64.zip", buf.into_inner()).unwrap();
        assert!(out.starts_with(b"MZ"));
        assert_eq!(out.len(), 4096);
    }

    #[test]
    fn stuck_leftover_does_not_block_the_update() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("jenova.exe");
        std::fs::write(&exe, b"current").unwrap();
        // A leftover that can't be deleted (a non-empty folder stands in for
        // a file locked by a hung process).
        let stuck = old_path(&exe);
        std::fs::create_dir(&stuck).unwrap();
        std::fs::write(stuck.join("x"), b"x").unwrap();

        let mut new = b"MZ".to_vec();
        new.resize(2048, 1);
        replace_exe(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), new);
        let aside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("jenova.exe.old-"))
            .collect();
        assert_eq!(aside.len(), 1, "current exe set aside under a new name");
        assert_eq!(
            std::fs::read(dir.path().join(&aside[0])).unwrap(),
            b"current"
        );
        // Cleanup finds both kinds of leftover file.
        assert_eq!(leftover_exes(&exe).len(), 1);
    }

    #[test]
    fn running_exe_is_swapped_and_old_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("jenova.exe");
        std::fs::write(&exe, b"old version").unwrap();
        // Keep a handle open like a running program would.
        let _held = std::fs::File::open(&exe).unwrap();
        let mut new = b"MZ".to_vec();
        new.resize(2048, 1);
        replace_exe(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), new);
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old version");
        assert!(!exe.with_extension("new").exists());
    }
}
