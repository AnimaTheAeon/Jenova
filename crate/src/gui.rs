//! GUI rendering components.

#[cfg(target_os = "linux")]
use crate::python_manager::PythonManager;
use crate::{
    config::{APP_VERSION, MAX_CONCURRENT_DOWNLOADS},
    data_structures::{DownloadJob, JobStatus, SubtitleDownloader},
    ff_theme::{self as ff, pal, TEXT_DIM},
    helper_functions::Utils,
    info,
    settings::Settings,
    warn,
};
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

/// Donation and dependency text color.
const DEPS_YELLOW: egui::Color32 = egui::Color32::from_rgb(247, 233, 181);

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn render_dependency_command(ui: &mut egui::Ui, label: &str, command: &str) -> bool {
    let mut copied_now = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        let mut command_edit = command.to_string();
        ui.add(
            egui::TextEdit::singleline(&mut command_edit).margin(crate::ff_theme::input_margin())
                .desired_width(350.0)
                .interactive(false)
                .font(egui::TextStyle::Monospace)
                .horizontal_align(egui::Align::Center),
        );
        if ui.button("Copy").clicked() {
            ui.output_mut(|output| {
                output
                    .commands
                    .push(egui::OutputCommand::CopyText(command.to_string()));
            });
            copied_now = true;
        }
        if copied_now {
            ui.label(egui::RichText::new("Copied!").color(egui::Color32::from_rgb(76, 175, 80)));
        }
    });
    copied_now
}

#[cfg(target_os = "linux")]
fn linux_install_command(apt: &str, dnf: &str, pacman: &str) -> String {
    match PythonManager::linux_package_manager() {
        "dnf" => dnf.to_string(),
        "pacman" => pacman.to_string(),
        _ => apt.to_string(),
    }
}

#[cfg(target_os = "macos")]
fn macos_install_command(package: &str, homebrew_installed: bool) -> String {
    let shellenv = "eval \"$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv 2>/dev/null)\"";
    if homebrew_installed {
        return format!("{shellenv} && brew install {package}");
    }

    format!(
        "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\" && {shellenv} && brew install {package}"
    )
}

impl SubtitleDownloader {
    /// Render the startup / installation title screen.
    pub fn render_installation_wait(&self, ui: &mut egui::Ui, fade_out: bool) {
        use crate::ff_theme as ff;
        let time = self.splash_started.elapsed().as_secs_f32();
        let fade_in = (time / 0.9).clamp(0.0, 1.0);
        let sub_in = ((time - 0.7) / 0.8).clamp(0.0, 1.0);
        let out_alpha = if fade_out {
            self.splash_out_alpha()
        } else {
            1.0
        };
        if fade_in < 1.0 || sub_in < 1.0 || (fade_out && out_alpha > 0.001) || !fade_out {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }

        let center = ui.max_rect().center();
        let alpha = |a: f32| (a * out_alpha * 255.0) as u8;
        let with_alpha = |c: egui::Color32, a: f32| {
            egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha(a))
        };
        let painter = ui.painter();

        // Title, letter-spaced, sliding up as it fades in.
        let title = "J E N O V A";
        let rise = (1.0 - fade_in) * 18.0;
        let title_pos = egui::pos2(center.x, center.y - 30.0 + rise);
        let font = egui::FontId::proportional(72.0);
        painter.text(
            title_pos + egui::vec2(3.0, 4.0),
            egui::Align2::CENTER_CENTER,
            title,
            font.clone(),
            with_alpha(egui::Color32::BLACK, fade_in * 0.8),
        );
        painter.text(
            title_pos,
            egui::Align2::CENTER_CENTER,
            title,
            font,
            with_alpha(ff::TEXT(), fade_in),
        );

        // Underline bar.
        let bar_w = 360.0 * fade_in;
        let bar = egui::Rect::from_center_size(
            egui::pos2(center.x, center.y + 22.0),
            egui::vec2(bar_w, 3.0),
        );
        painter.rect_filled(
            bar,
            egui::CornerRadius::same(1),
            with_alpha(ff::LABEL(), fade_in),
        );

        let status = if self.installing_python {
            "Installing Python…"
        } else if self.installing_subliminal {
            "Installing Subliminal…"
        } else {
            "Subtitle watcher"
        };
        painter.text(
            egui::pos2(center.x, center.y + 50.0),
            egui::Align2::CENTER_CENTER,
            status,
            egui::FontId::proportional(20.0),
            with_alpha(ff::TEXT_DIM(), sub_in),
        );
    }

    /// Render Python installation status.
    pub fn render_python_status(&mut self, ui: &mut egui::Ui) {
        if self.is_python_installed() {
            let raw = self.get_python_version().cloned().unwrap_or_default();
            let ver = raw.strip_prefix("Python ").unwrap_or(&raw);
            ui.label(
                egui::RichText::new(format!("Python v{}", ver))
                    .color(DEPS_YELLOW)
                    .size(12.0)
                    .monospace(),
            );
        } else {
            ui.label(
                egui::RichText::new("Python not found")
                    .color(DEPS_YELLOW)
                    .size(12.0)
                    .monospace(),
            );
            #[cfg(windows)]
            if ui.button("Install Python").clicked() {
                info!("User initiated Python installation");
                self.start_python_install();
            }
            #[cfg(target_os = "linux")]
            {
                let command = linux_install_command(
                    "sudo apt install python3 python3-pip",
                    "sudo dnf install python3 python3-pip",
                    "sudo pacman -S python python-pip",
                );
                render_dependency_command(ui, "Install Python 3:", &command);
            }
            #[cfg(target_os = "macos")]
            {
                let command = macos_install_command("python", self.homebrew_installed);
                let label = if self.homebrew_installed {
                    "Install Python 3:"
                } else {
                    "Install Homebrew and Python 3:"
                };
                render_dependency_command(ui, label, &command);
            }
        }
    }

    /// Render pipx installation status.
    pub fn render_pipx_status(&mut self, _ui: &mut egui::Ui) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            if self.is_python_installed() {
                if self.is_pipx_installed() {
                    let text = self
                        .get_pipx_version()
                        .map(|v| format!("pipx v{}", v))
                        .unwrap_or_else(|| "pipx is installed".to_string());
                    _ui.label(
                        egui::RichText::new(text)
                            .color(DEPS_YELLOW)
                            .size(12.0)
                            .monospace(),
                    );
                } else {
                    _ui.label(
                        egui::RichText::new("pipx not found")
                            .color(DEPS_YELLOW)
                            .size(12.0)
                            .monospace(),
                    );
                }
            }
        }
    }

    /// Render Subliminal installation status.
    pub fn render_subliminal_status(&mut self, ui: &mut egui::Ui) {
        if !self.subliminal_installed && self.subliminal_version.is_some() {
            ui.label(
                "Subliminal 2.4.0 or newer is required. Upgrade your Subliminal installation.",
            );
        }
        if self.is_python_installed() {
            #[cfg(target_os = "linux")]
            {
                if !self.is_pipx_installed() {
                    ui.label(
                        egui::RichText::new("Subliminal not found")
                            .color(DEPS_YELLOW)
                            .size(12.0)
                            .monospace(),
                    );
                    let command = linux_install_command(
                        "sudo apt install pipx && pipx install subliminal",
                        "sudo dnf install python3-pipx && pipx install subliminal",
                        "sudo pacman -S python-pipx && pipx install subliminal",
                    );
                    render_dependency_command(ui, "Install missing dependencies:", &command);
                    return;
                }
            }
            #[cfg(target_os = "macos")]
            {
                if !self.is_pipx_installed() && !self.is_subliminal_installed() {
                    ui.label(
                        egui::RichText::new("Subliminal not found")
                            .color(DEPS_YELLOW)
                            .size(12.0)
                            .monospace(),
                    );
                    let command = macos_install_command(
                        "pipx && pipx ensurepath && pipx install subliminal",
                        self.homebrew_installed,
                    );
                    let label = if self.homebrew_installed {
                        "Install pipx and Subliminal:"
                    } else {
                        "Install Homebrew, pipx, and Subliminal:"
                    };
                    render_dependency_command(ui, label, &command);
                    return;
                }
            }
            if self.is_subliminal_installed() && !self.installing_subliminal {
                let raw = self.get_subliminal_version().cloned().unwrap_or_default();
                let ver = raw
                    .strip_prefix("subliminal, version ")
                    .or_else(|| raw.strip_prefix("subliminal "))
                    .unwrap_or(&raw);
                ui.label(
                    egui::RichText::new(format!("Subliminal v{}", ver))
                        .color(DEPS_YELLOW)
                        .size(12.0)
                        .monospace(),
                );
            } else if !self.is_subliminal_installed() {
                ui.label(
                    egui::RichText::new("Subliminal not found")
                        .color(DEPS_YELLOW)
                        .size(12.0)
                        .monospace(),
                );
                #[cfg(windows)]
                if ui.button("Install Subliminal").clicked() {
                    info!("User initiated Subliminal installation");
                    self.start_subliminal_install();
                }
                #[cfg(target_os = "linux")]
                render_dependency_command(ui, "Install Subliminal:", "pipx install subliminal");
                #[cfg(target_os = "macos")]
                render_dependency_command(ui, "Install Subliminal:", "pipx install subliminal");
            }
        }
        if self.is_version_checked() {
            if let Some(latest) = self.get_latest_version() {
                if Self::is_outdated(APP_VERSION, latest) {
                    ui.label(
                        egui::RichText::new(format!(
                            "Jenova {} is available. Update from the Updates section below.",
                            latest
                        ))
                        .color(egui::Color32::from_rgb(244, 67, 54)),
                    );
                }
            } else if let Some(err) = self.get_version_check_error() {
                ui.label(
                    egui::RichText::new(format!("Version check failed: {}", err))
                        .color(egui::Color32::from_rgb(230, 194, 0)),
                );
            }
        }
    }

    /// Render FFmpeg installation status.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn render_ffmpeg_status(&mut self, ui: &mut egui::Ui) {
        #[cfg(target_os = "linux")]
        if !self.is_ffmpeg_installed() {
            ui.label(
                egui::RichText::new("FFmpeg not found")
                    .color(DEPS_YELLOW)
                    .size(12.0)
                    .monospace(),
            );
            let command = linux_install_command(
                "sudo apt install ffmpeg",
                "sudo dnf install ffmpeg",
                "sudo pacman -S ffmpeg",
            );
            render_dependency_command(ui, "Install FFmpeg:", &command);
        }
        #[cfg(target_os = "macos")]
        if !self.is_ffmpeg_installed() {
            ui.label(
                egui::RichText::new("FFmpeg not found")
                    .color(DEPS_YELLOW)
                    .size(12.0)
                    .monospace(),
            );
            let command = macos_install_command("ffmpeg", self.homebrew_installed);
            let label = if self.homebrew_installed {
                "Install FFmpeg:"
            } else {
                "Install Homebrew and FFmpeg:"
            };
            render_dependency_command(ui, label, &command);
        }
    }

    /// Your languages as removable tags, and a box to add more.
    pub fn render_languages(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (index, code) in self.selected_languages.iter().enumerate() {
                let text = RichText::new(format!("{}  ×", language_name(code)))
                    .strong()
                    .color(pal().on_accent);
                if ui
                    .add(egui::Button::new(text).fill(pal().accent))
                    .on_hover_text("Remove")
                    .clicked()
                {
                    remove = Some(index);
                }
            }
            if self.selected_languages.is_empty() {
                ui.label(
                    RichText::new("No languages yet, so nothing will be downloaded.")
                        .color(ff::WARN()),
                );
            }
        });
        if let Some(index) = remove {
            self.selected_languages.remove(index);
            changed = true;
        }
        let query = self.language_search.trim().to_lowercase();
        let matches: Vec<(&str, &str)> = if query.is_empty() {
            Vec::new()
        } else {
            LANGUAGES
                .iter()
                .filter(|(code, _)| !self.selected_languages.iter().any(|c| c == code))
                .filter(|(code, name)| name.to_lowercase().contains(&query) || *code == query)
                .take(8)
                .copied()
                .collect()
        };
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.language_search).margin(crate::ff_theme::input_margin())
                    .hint_text("Add a language…")
                    .desired_width(220.0),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some((code, _)) = matches.first() {
                    self.selected_languages.push((*code).to_string());
                    self.language_search.clear();
                    changed = true;
                }
            }
        });
        if !query.is_empty() {
            ui.horizontal_wrapped(|ui| {
                if matches.is_empty() {
                    ui.label(RichText::new("No match").color(TEXT_DIM()));
                }
                for (code, name) in &matches {
                    if ui.button(format!("+ {name}")).clicked() {
                        self.selected_languages.push((*code).to_string());
                        self.language_search.clear();
                        changed = true;
                    }
                }
            });
        }
        if changed {
            self.save_current_settings();
        }
    }

    /// Which subtitle sites to search, and the OpenSubtitles.com login.
    pub fn render_providers(&mut self, ui: &mut egui::Ui) {
        const PROVIDERS: &[(&str, &str)] = &[
            ("addic7ed", "Addic7ed"),
            ("gestdown", "Gestdown"),
            ("napiprojekt", "NapiProjekt"),
            ("opensubtitles", "OpenSubtitles"),
            ("opensubtitlescom", "OpenSubtitles.com"),
            ("podnapisi", "Podnapisi"),
            ("tvsubtitles", "TVSubtitles"),
        ];
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            for (code, name) in PROVIDERS {
                let mut selected = self.providers.iter().any(|value| value == code);
                if ui.checkbox(&mut selected, *name).changed() {
                    changed = true;
                    if selected {
                        self.providers.push((*code).to_string());
                    } else {
                        self.providers.retain(|value| value != code);
                    }
                }
            }
        });
        if self.providers.is_empty() {
            ui.label(RichText::new("None ticked: Subliminal uses all of its sites.").color(TEXT_DIM()).size(12.0));
        }
        ui.add_space(4.0);
        ui.label(RichText::new("OpenSubtitles.com login (optional, for higher download limits)").color(TEXT_DIM()));
        ui.horizontal_wrapped(|ui| {
            ui.label("Username");
            if ui
                .add(egui::TextEdit::singleline(&mut self.opensubtitlescom_username).margin(crate::ff_theme::input_margin()).desired_width(140.0))
                .changed()
            {
                changed = true;
            }
            ui.label("Password");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.opensubtitlescom_password).margin(crate::ff_theme::input_margin())
                    .password(true)
                    .desired_width(140.0),
            );
            if response.lost_focus() {
                self.save_password();
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("API key");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.opensubtitlescom_apikey).margin(crate::ff_theme::input_margin())
                        .password(!self.show_opensubtitles_apikey)
                        .desired_width(260.0)
                        .hint_text("Paste from opensubtitles.com/api"),
                )
                .changed()
            {
                changed = true;
            }
            if ui.small_button(if self.show_opensubtitles_apikey { "Hide" } else { "Show" }).clicked() {
                self.show_opensubtitles_apikey = !self.show_opensubtitles_apikey;
            }
            let link = ui.hyperlink_to("Get API key", "https://www.opensubtitles.com/api");
            if link.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        });
        ui.label(RichText::new(Settings::credentials_storage_blurb()).color(TEXT_DIM()).size(11.0));
        if changed {
            self.save_current_settings();
        }
    }

    /// Refiners and match-quality knobs (under Advanced).
    pub fn render_matching_advanced(&mut self, ui: &mut egui::Ui) {
        const REFINERS: &[(&str, &str, &str)] = &[
            ("hash", "File hash", "Match by the video file's fingerprint"),
            ("metadata", "Video metadata", "Read resolution and codecs from the file (skipped on network shares)"),
            ("tmdb", "TMDB", "Look up movie details on The Movie Database"),
            ("tvdb", "TVDB", "Look up episode details on TheTVDB"),
        ];
        let mut changed = false;
        ui.label("Ways to identify a video");
        ui.horizontal_wrapped(|ui| {
            for (code, name, hint) in REFINERS {
                let mut selected = self.refiners.iter().any(|value| value == code);
                if ui.checkbox(&mut selected, *name).on_hover_text(*hint).changed() {
                    changed = true;
                    if selected {
                        self.refiners.push((*code).to_string());
                    } else {
                        self.refiners.retain(|value| value != code);
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Minimum match score");
            changed |= ui
                .add(egui::DragValue::new(&mut self.minimum_score).range(0..=100))
                .on_hover_text("0 accepts the best match Subliminal finds. Higher is stricter but finds less.")
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("OpenSubtitles.com result pages");
            let mut pages = self.opensubtitles_max_pages.unwrap_or(3);
            if ui
                .add(egui::DragValue::new(&mut pages).range(1..=20))
                .on_hover_text("How many pages of results to look through. More is slower.")
                .changed()
            {
                self.opensubtitles_max_pages = Some(pages);
                changed = true;
            }
        });
        if changed {
            self.save_current_settings();
        }
    }

    /// "Downloads at once" (1 to 5).
    pub fn render_concurrent_downloads(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Downloads at once");
            if ui
                .add(egui::DragValue::new(&mut self.concurrent_downloads).range(1..=MAX_CONCURRENT_DOWNLOADS))
                .on_hover_text("Subtitle sites limit (and can block) apps that ask for too much at once")
                .changed()
            {
                self.save_current_settings();
            }
        });
    }
}

/// Languages offered in Settings: (Subliminal code, name).
pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("en-gb", "English (UK)"),
    ("en-us", "English (US)"),
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("az", "Azerbaijani"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("de-at", "German (Austria)"),
    ("de-ch", "German (Switzerland)"),
    ("el", "Greek"),
    ("es", "Spanish"),
    ("es-es", "Spanish (Spain)"),
    ("es-mx", "Spanish (Mexico)"),
    ("et", "Estonian"),
    ("fa", "Persian/Farsi"),
    ("fi", "Finnish"),
    ("fil", "Filipino/Tagalog"),
    ("fr", "French"),
    ("fr-ca", "French (Canada)"),
    ("gu", "Gujarati"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("it-ch", "Italian (Switzerland)"),
    ("ja", "Japanese"),
    ("ka", "Georgian"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("ku", "Kurdish"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("my", "Burmese"),
    ("nl", "Dutch"),
    ("nl-be", "Dutch (Belgium)"),
    ("no", "Norwegian"),
    ("or", "Odia"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("pt-br", "Portuguese (Brazil)"),
    ("pt-pt", "Portuguese (Portugal)"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sr", "Serbian"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("vi", "Vietnamese"),
    ("xh", "Xhosa"),
    ("zh", "Chinese"),
    ("zh-cn", "Chinese (Simplified)"),
    ("zh-tw", "Chinese (Traditional)"),
    ("zu", "Zulu"),
];

/// Display name for a language code.
pub(crate) fn language_name(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| crate::subtitle_utils::SubtitleUtils::language_code_to_name(code).to_string())
}

/// Short status word, its color, and the detail text for a job.
fn job_status_view(job: &DownloadJob) -> (&'static str, egui::Color32, String) {
    let dim = TEXT_DIM();
    match &job.status {
        JobStatus::Pending => ("Queued", ff::WARN(), job.output.clone()),
        JobStatus::Running => ("Searching", ff::LABEL(), job.output.clone()),
        JobStatus::Success => (
            "Found",
            ff::GOOD(),
            job.subtitle_paths
                .first()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        JobStatus::Skipped => ("Had subs", dim, "Already has subtitles in your languages".into()),
        JobStatus::EmbeddedExists(message) => ("Embedded", dim, message.clone()),
        JobStatus::NotFound(message) => ("Not yet", ff::TEXT(), message.clone()),
        JobStatus::Failed(message) => ("Error", ff::BAD(), message.clone()),
        JobStatus::Cancelled => ("Cancelled", dim, String::new()),
    }
}

/// The Activity list: video, status, details, time.
pub(crate) fn render_activity_table(
    ui: &mut egui::Ui,
    jobs: &[DownloadJob],
    roots: &[std::path::PathBuf],
    id_salt: &str,
) {
    let row_height = 24.0;
    let available_height = ui.available_height();
    TableBuilder::new(ui)
        .id_salt(id_salt)
        .max_scroll_height(available_height.max(100.0))
        .min_scrolled_height(100.0)
        .auto_shrink([false, false])
        .striped(true)
        .resizable(true)
        .vscroll(true)
        .column(Column::initial(300.0).at_least(150.0).resizable(true))
        .column(Column::exact(86.0))
        .column(Column::remainder().at_least(150.0))
        .column(Column::exact(64.0))
        .header(row_height, |mut header| {
            for title in ["Video", "Status", "Details", "When"] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|mut body| {
            for job in jobs {
                let (status_text, status_color, detail) = job_status_view(job);
                body.row(row_height, |mut row| {
                    row.col(|ui| {
                        let video_path = &job.video_path;
                        let response = ui
                            .add(
                                egui::Label::new(display_name(video_path, roots))
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text(format!("{}\nClick to open its folder", video_path.display()));
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            if let Err(e) = Utils::open_containing_folder(video_path) {
                                warn!("Failed to open folder for {}: {}", video_path.display(), e);
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(status_text).color(status_color).strong());
                    });
                    row.col(|ui| {
                        let response = ui.add(
                            egui::Label::new(RichText::new(&detail).color(TEXT_DIM()))
                                .truncate()
                                .sense(egui::Sense::click()),
                        );
                        if matches!(job.status, JobStatus::Failed(_)) {
                            if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if response.clicked() {
                                if let Err(e) = Utils::open_log_file() {
                                    warn!("Failed to open log file: {}", e);
                                }
                            }
                            response.on_hover_text(format!("{detail}\nClick to open the log file"));
                        } else if !detail.is_empty() {
                            response.on_hover_text(&detail);
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(when(job.at)).color(TEXT_DIM()).size(12.0));
                    });
                });
            }
        });
}

/// "Show/Season 01/Episode.mkv" relative to the folder it came from.
fn display_name(video_path: &std::path::Path, roots: &[std::path::PathBuf]) -> String {
    let root = roots
        .iter()
        .find(|root| video_path.starts_with(root))
        .map(|root| root.as_path())
        .unwrap_or(std::path::Path::new(""));
    let mut parts = Vec::new();
    if let Some(parent) = video_path.parent().filter(|path| *path != root) {
        if let Some(show) = parent.parent().filter(|path| *path != root).and_then(|p| p.file_name()) {
            parts.push(show.to_string_lossy().into_owned());
        }
        if let Some(season) = parent.file_name() {
            parts.push(season.to_string_lossy().into_owned());
        }
    }
    parts.push(Utils::get_file_name(video_path));
    parts.join(std::path::MAIN_SEPARATOR_STR)
}

/// "14:05" today, "Mon" this week, "Nov 3" before that.
fn when(at: chrono::DateTime<chrono::Local>) -> String {
    let now = chrono::Local::now();
    let age = now.signed_duration_since(at);
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else if age < chrono::Duration::days(6) {
        at.format("%a").to_string()
    } else {
        at.format("%b %-d").to_string()
    }
}

impl eframe::App for SubtitleDownloader {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            if crate::tray::available() && !crate::tray::quitting() {
                // Closing hides Jenova in the tray; it keeps watching.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                crate::tray::hide_window();
                return;
            }
            // Quitting: hide at once, clean up after.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        self.poll_init_check();
        self.handle_installation_states();
        self.sync_with_engine(&ctx);
        if !crate::tray::window_visible() {
            // Hidden in the tray: nothing to draw.
            return;
        }

        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.scroll.floating = false;
            style.spacing.scroll.bar_width = 8.0;
        });
        self.begin_splash_dismiss_if_ready();
        if !self.can_show_main_ui() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.45);
                    self.render_installation_wait(ui, true);
                });
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return;
        }

        self.refresh_installation_status();
        self.poll_version_check();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::ff_theme::BACKDROP())
                    .inner_margin(egui::Margin::same(if self.theme == crate::ff_theme::Theme::Blades { 0 } else { 14 })),
            )
            .show(ui, |ui| {
                if self.installing_python || self.installing_subliminal {
                    crate::ff_theme::window(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.render_installation_wait(ui, false);
                    });
                    return;
                }
                match self.theme {
                    crate::ff_theme::Theme::GreenDash => self.render_green_layout(ui),
                    crate::ff_theme::Theme::Blades => self.render_blades_layout(ui),
                    crate::ff_theme::Theme::FfMenu => self.render_ff_layout(ui),
                }
            });

        if self.installing_python || self.installing_subliminal || self.watch.shared.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        }
    }

    fn on_exit(&mut self) {
        self.shutdown_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        self.background_check_receiver = None;
        self.engine.shutdown();
        info!("Application closed by user");
        info!("");
        info!("---------------------------------------------------------------");
        info!("");
    }
}
