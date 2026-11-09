//! Party-menu style layout: a main window, a command menu, a time/count box
//! and a status bar.

use std::sync::atomic::Ordering;

use eframe::egui::{self, pos2, Align, Layout, Rect, RichText, Vec2};
use rfd::FileDialog;

use crate::data_structures::{DownloadJob, MenuPage, SubtitleDownloader};
use crate::ff_theme::{self as ff, LABEL, TEXT, TEXT_DIM};
use crate::gui::render_jobs_table;
use crate::helper_functions::{Utils, Validation};
use crate::info;
use crate::subtitle_utils::SubtitleUtils;

const MENU: &[(MenuPage, &str)] = &[
    (MenuPage::Watch, "Watch"),
    (MenuPage::Scan, "Scan"),
    (MenuPage::Downloads, "Downloads"),
    (MenuPage::Language, "Language"),
    (MenuPage::Matching, "Matching"),
    (MenuPage::Config, "Config"),
    (MenuPage::Log, "Log"),
];

impl SubtitleDownloader {
    /// Lay out every window.
    pub fn render_ff_layout(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        let gap = 12.0;
        let side_w = 210.0_f32.min(full.width() * 0.3);
        let bar_h = 48.0;
        let deps_ready = self.dependencies_ready();
        if !deps_ready {
            self.menu_page = MenuPage::Config;
        }

        let main = Rect::from_min_max(
            full.min,
            pos2(full.right() - side_w - gap, full.bottom() - bar_h - gap),
        );
        let menu_h =
            MENU.len() as f32 * (ff::MENU_ITEM_HEIGHT + 6.0) + ff::WINDOW_PADDING as f32 * 2.0;
        let menu = Rect::from_min_size(
            pos2(main.right() + gap, full.top()),
            Vec2::new(side_w, menu_h),
        );
        let info = Rect::from_min_max(
            pos2(menu.left(), main.bottom() - 92.0),
            pos2(full.right(), main.bottom()),
        );
        let bar = Rect::from_min_max(pos2(full.left(), full.bottom() - bar_h), full.max);

        ff::window_at(ui, menu, "ff_menu", |ui| {
            for (page, label) in MENU {
                let enabled = deps_ready || *page == MenuPage::Config;
                // No cursor on the selected item until the menu is used.
                let response = ff::menu_item(
                    ui,
                    label,
                    self.menu_touched && self.menu_page == *page,
                    enabled,
                );
                if enabled && response.clicked() {
                    self.menu_touched = true;
                    self.menu_page = *page;
                    self.set_keep_dropdown_open(*page == MenuPage::Language);
                }
            }
        });

        ff::window_at(ui, info, "ff_info", |ui| {
            let secs = self.app_started.elapsed().as_secs();
            let time = format!(
                "{:02}:{:02}:{:02}",
                secs / 3600,
                (secs / 60) % 60,
                secs % 60
            );
            let subs = self
                .scan_history
                .lock()
                .map(|history| history.records.len())
                .unwrap_or(0);
            info_row(ui, "Time", &time);
            ui.add_space(4.0);
            info_row(ui, "Subs", &format_count(subs));
        });

        ff::window_at(ui, bar, "ff_status", |ui| {
            ui.add_space(-4.0);
            ui.horizontal_centered(|ui| {
                ff::title(ui, &self.status_line(), 16.0, TEXT);
            });
        });

        self.render_in_app_toasts(
            &ui.ctx().clone(),
            pos2(main.right() - 14.0, main.top() + 14.0),
        );

        self.render_update_prompt(ui.ctx());

        ff::window_at(ui, main, "ff_main", |ui| {
            if !deps_ready {
                self.render_config_page(ui);
                return;
            }
            match self.menu_page {
                MenuPage::Watch => self.render_watch_page(ui),
                MenuPage::Scan => self.render_scan_page(ui),
                MenuPage::Downloads => self.render_downloads_page(ui),
                MenuPage::Language => {
                    page_title(ui, "Language", "Subtitle languages and what to download");
                    egui::ScrollArea::vertical()
                        .id_salt("lang_scroll")
                        .show(ui, |ui| {
                            self.set_keep_dropdown_open(true);
                            self.render_language_selection(ui);
                        });
                }
                MenuPage::Matching => {
                    page_title(ui, "Matching", "Providers, refiners and match quality");
                    egui::ScrollArea::vertical()
                        .id_salt("match_scroll")
                        .show(ui, |ui| {
                            self.render_matching_options(ui);
                        });
                }
                MenuPage::Config => self.render_config_page(ui),
                MenuPage::Log => self.render_log_page(ui),
            }
        });
    }

    /// Menu-style "Subtitles found" pop-ups in the top-right of the main window.
    pub fn render_in_app_toasts(&self, ctx: &egui::Context, anchor: egui::Pos2) {
        if !self.notify_in_app {
            return;
        }
        let toasts = crate::notifications::in_app_toasts();
        if toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let mut y = anchor.y;
        for (index, item) in toasts.iter().enumerate().rev() {
            let age = item.shown_at.elapsed().as_secs_f32();
            let life = crate::notifications::IN_APP_LIFETIME.as_secs_f32();
            let slide = ((age / 0.25).min(1.0) - 1.0) * -30.0;
            let fade = ((life - age) / 0.6).clamp(0.0, 1.0);
            let area = egui::Area::new(egui::Id::new(("ff_toast", index)))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(egui::pos2(anchor.x + slide, y))
                .interactable(true)
                .show(ctx, |ui| {
                    ui.set_opacity(fade);
                    ui.set_max_width(340.0);
                    ff::window(ui, |ui| {
                        ui.set_width(300.0);
                        ff::title(ui, &item.toast.title, 15.0, LABEL);
                        ui.add(
                            egui::Label::new(RichText::new(&item.toast.body).color(TEXT).strong())
                                .wrap(),
                        );
                    })
                });
            if area.response.interact(egui::Sense::click()).clicked() {
                crate::notifications::dismiss_in_app(index);
            }
            y += area.response.rect.height() + 10.0;
        }
    }

    fn status_line(&self) -> String {
        let shared = &self.watch.shared;
        if self.downloading || self.scanning {
            return self.status.clone();
        }
        if shared.batch_running.load(Ordering::SeqCst) {
            let jobs = shared.jobs_snapshot();
            let running = jobs
                .iter()
                .filter(|j| j.status == crate::JobStatus::Running)
                .count();
            let pending = jobs
                .iter()
                .filter(|j| j.status == crate::JobStatus::Pending)
                .count();
            return format!(
                "Fetching subtitles for new arrivals: {running} running, {pending} waiting"
            );
        }
        if !self.dependencies_ready() {
            return "Install the missing dependencies to begin.".into();
        }
        if self.watch_enabled && !self.watch_folders.is_empty() {
            let settling = shared.settling.load(Ordering::SeqCst);
            let n = shared.watched_folders.load(Ordering::SeqCst);
            if settling > 0 {
                return format!("Watching {n} folder(s). {settling} new file(s) settling…");
            }
            return format!("Watching {n} folder(s) for new videos.");
        }
        self.status.clone()
    }

    fn render_watch_page(&mut self, ui: &mut egui::Ui) {
        let shared = std::sync::Arc::clone(&self.watch.shared);
        let mut changed = false;

        ui.horizontal(|ui| {
            ff::title(ui, "Watch", 26.0, TEXT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, color) = if self.watch_enabled {
                    ("WATCHING: ON", ff::GOOD)
                } else {
                    ("WATCHING: OFF", ff::BAD)
                };
                if ui
                    .add(egui::Button::new(
                        RichText::new(label).color(color).strong(),
                    ))
                    .on_hover_text("Turn folder watching on or off")
                    .clicked()
                {
                    self.watch_enabled = !self.watch_enabled;
                    changed = true;
                }
            });
        });
        ui.label(
            RichText::new("New videos landing in these folders get subtitles automatically.")
                .color(TEXT_DIM),
        );
        ff::divider(ui);

        let watched = shared.watched_folders.load(Ordering::SeqCst);
        let catching_up = shared.catching_up.load(Ordering::SeqCst);
        let errors = shared.errors.lock().map(|e| e.clone()).unwrap_or_default();
        let mut remove = None;

        let footer_h = 40.0;
        egui::ScrollArea::vertical()
            .id_salt("watch_party")
            .max_height((ui.available_height() - footer_h).max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.watch_folders.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ff::title(ui, "No folders yet", 20.0, TEXT_DIM);
                        ui.label(
                            RichText::new("Add your Movies or TV folder to start watching.")
                                .color(TEXT_DIM),
                        );
                    });
                }
                for (index, folder) in self.watch_folders.iter().enumerate() {
                    let path = std::path::Path::new(folder);
                    let exists = path.is_dir();
                    let failed = errors.iter().any(|e| e.contains(folder.as_str()));
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| folder.clone());
                    let letter: String = name
                        .chars()
                        .next()
                        .map(|c| c.to_uppercase().collect())
                        .unwrap_or_default();
                    let (status, status_color) = if !exists || failed {
                        ("OFFLINE", ff::BAD)
                    } else if !self.watch_enabled {
                        ("PAUSED", ff::WARN)
                    } else {
                        ("WATCHING", ff::GOOD)
                    };
                    let stats = shared.stats_for(path);

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ff::emblem(ui, &letter, 62.0, exists && self.watch_enabled);
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ff::title(ui, &name, 21.0, TEXT);
                                ui.add_space(12.0);
                                ff::title(ui, status, 14.0, status_color);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .small_button("Remove")
                                        .on_hover_text("Stop watching this folder")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                    if ui.small_button("Open").clicked() {
                                        let _ = Utils::open_folder(folder);
                                    }
                                });
                            });
                            ui.label(RichText::new(folder).color(TEXT_DIM).size(12.0));
                            ui.horizontal(|ui| {
                                ff::stat_label(ui, "SUBS");
                                let fraction = match stats {
                                    Some(s) if s.videos > 0 => s.with_subs as f32 / s.videos as f32,
                                    _ => 0.0,
                                };
                                ff::stat_bar(
                                    ui,
                                    fraction,
                                    170.0,
                                    egui::Color32::from_rgb(30, 110, 230),
                                    egui::Color32::from_rgb(140, 230, 255),
                                )
                                .on_hover_text(
                                    "How much of this folder already has subtitles in your languages",
                                );
                            });
                        });
                    });
                    ui.add_space(4.0);
                    ff::divider(ui);
                }
            });

        if let Some(index) = remove {
            let removed = self.watch_folders.remove(index);
            info!("Removed watch folder: {}", removed);
            changed = true;
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Add Folder").clicked() {
                    if let Some(folder) = FileDialog::new().pick_folder() {
                        let folder = folder.display().to_string();
                        if Validation::is_valid_folder(&folder)
                            && !self.watch_folders.contains(&folder)
                        {
                            info!("Added watch folder: {}", folder);
                            self.watch_folders.push(folder);
                            if self.watch_folders.len() == 1 {
                                self.watch_enabled = true;
                            }
                            changed = true;
                        }
                    }
                }
                let can_scan = self.watch_enabled && watched > 0 && !catching_up;
                if ui
                    .add_enabled(can_scan, egui::Button::new("Check existing files"))
                    .on_hover_text(
                        "Queue videos in the watched folders that are still missing subtitles",
                    )
                    .clicked()
                {
                    self.watch.catch_up_now();
                }
                if shared.batch_running.load(Ordering::SeqCst)
                    && ui.button("Cancel downloads").clicked()
                {
                    self.watch.cancel_batch();
                }
            });
        });

        if changed {
            self.save_current_settings();
        }
    }

    fn render_scan_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Scan",
            "One-off scan of a whole folder for missing subtitles",
        );
        self.render_folder_selection(ui);
        self.render_scan_results(ui);
        let total = self.get_total_downloads();
        if total > 0 {
            let done = self.get_downloads_completed();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ff::stat_label(ui, "PROGRESS");
                ff::stat_bar(
                    ui,
                    done as f32 / total as f32,
                    (ui.available_width() - 110.0).max(80.0),
                    egui::Color32::from_rgb(200, 150, 20),
                    egui::Color32::from_rgb(255, 236, 140),
                );
                ui.label(RichText::new(format!("{done} / {total}")).strong());
            });
        }
        ff::divider(ui);
        self.update_cached_jobs();
        let jobs = self.get_cached_jobs().clone();
        if jobs.is_empty() {
            ui.label(RichText::new("Pick a folder to scan. Results appear here.").color(TEXT_DIM));
        } else {
            let root = std::path::PathBuf::from(&self.folder_path);
            render_jobs_table(ui, &jobs, &[root], "scan_jobs");
        }
    }

    fn render_downloads_page(&mut self, ui: &mut egui::Ui) {
        let running = self.watch.shared.batch_running.load(Ordering::SeqCst);
        ui.horizontal(|ui| {
            ff::title(ui, "Downloads", 26.0, TEXT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if running {
                    if ui.button("Cancel").clicked() {
                        self.watch.cancel_batch();
                    }
                } else if ui.button("Clear finished").clicked() {
                    self.watch.clear_finished();
                }
            });
        });
        ui.label(
            RichText::new(
                "Subtitles fetched for videos that arrived in watched folders (newest first).",
            )
            .color(TEXT_DIM),
        );
        ff::divider(ui);
        let jobs: Vec<DownloadJob> = self
            .watch
            .shared
            .jobs_snapshot()
            .into_iter()
            .rev()
            .collect();
        if jobs.is_empty() {
            ui.label(RichText::new("Nothing yet. New arrivals will show up here.").color(TEXT_DIM));
        } else {
            let roots: Vec<std::path::PathBuf> = self
                .watch_folders
                .iter()
                .map(std::path::PathBuf::from)
                .collect();
            render_jobs_table(ui, &jobs, &roots, "watch_jobs");
        }
    }

    fn render_config_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ff::title(ui, "Config", 26.0, TEXT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let applying = self.apply_in_progress.load(Ordering::SeqCst);
                if ui
                    .add_enabled(
                        !applying && self.dependencies_ready(),
                        egui::Button::new(RichText::new("Save & Apply").strong().size(16.0)),
                    )
                    .on_hover_text("Save all settings and restart the background watcher so every change takes effect now")
                    .clicked()
                {
                    self.save_and_apply();
                }
                if applying {
                    ui.label(RichText::new("Applying…").color(ff::WARN));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                } else if let Some(at) = self.settings_applied_at {
                    if at.elapsed().as_secs_f32() < 4.0 {
                        ui.label(RichText::new("Saved and applied").color(ff::GOOD));
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                    }
                }
            });
        });
        ui.label(
            RichText::new("Dependencies, performance, background mode and notifications")
                .color(TEXT_DIM),
        );
        ff::divider(ui);
        egui::ScrollArea::vertical().id_salt("config_scroll").show(ui, |ui| {
            ff::stat_label(ui, "DEPENDENCIES");
            self.render_python_status(ui);
            self.render_pipx_status(ui);
            self.render_subliminal_status(ui);
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            self.render_ffmpeg_status(ui);
            if !self.dependencies_ready() {
                return;
            }
            ff::divider(ui);
            ff::stat_label(ui, "DOWNLOADS");
            self.render_concurrent_downloads(ui);
            ff::divider(ui);

            ff::stat_label(ui, "WATCHING");
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Wait until a new file stops changing for");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.watch_settle_seconds).range(5..=3600).suffix(" s"))
                    .on_hover_text("Gives downloads and copies time to finish before Subliminal reads the file")
                    .changed();
            });
            changed |= ui
                .checkbox(&mut self.watch_catch_up, "When watching starts, also check files already in the folders")
                .changed();
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut self.watch_polling, "Polling mode (NAS / network drives)")
                    .on_hover_text("Use this if new files on a mapped drive or network share aren't detected")
                    .changed();
                if self.watch_polling {
                    ui.label("every");
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.watch_poll_interval_seconds).range(5..=3600).suffix(" s"))
                        .changed();
                }
            });
            ff::divider(ui);
            self.render_background_section(ui);
            ff::divider(ui);
            ff::stat_label(ui, "NOTIFICATIONS");
            changed |= ui
                .checkbox(
                    &mut self.notify_desktop,
                    "Windows notifications (toasts) when subtitles are found",
                )
                .changed();
            changed |= ui
                .checkbox(&mut self.notify_in_app, "Pop-up inside Jenova when subtitles are found")
                .changed();
            if changed {
                self.save_current_settings();
            }
            ff::divider(ui);
            self.render_bak_cleanup(ui);
            ff::divider(ui);
            self.render_updates_section(ui);
            ff::divider(ui);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Jenova v{} ·", crate::config::APP_VERSION)).color(TEXT_DIM));
                ui.hyperlink_to("AnimaTheAeon/Jenova", "https://github.com/AnimaTheAeon/Jenova");
            });
        });
    }

    fn render_bak_cleanup(&mut self, ui: &mut egui::Ui) {
        use crate::data_structures::BakCleanup;
        ff::stat_label(ui, "SUBTITLE BACKUPS");
        let state = self
            .bak_cleanup
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let mut roots: Vec<std::path::PathBuf> = self
            .watch_folders
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        if !self.folder_path.is_empty() {
            roots.push(std::path::PathBuf::from(&self.folder_path));
        }
        roots.retain(|r| r.is_dir());
        roots.sort();
        roots.dedup();
        let cleanup = std::sync::Arc::clone(&self.bak_cleanup);
        let ctx = ui.ctx().clone();
        let set = move |s: BakCleanup| {
            if let Ok(mut slot) = cleanup.lock() {
                *slot = s;
            }
            ctx.request_repaint();
        };
        ui.horizontal_wrapped(|ui| match state {
            BakCleanup::Idle => {
                ui.label(RichText::new("Older versions saved copies like Movie.en.srt.bak next to your subtitles.").color(TEXT_DIM));
                if ui
                    .add_enabled(!roots.is_empty(), egui::Button::new("Find .bak files"))
                    .on_hover_text("Searches your watched folders and the last scanned folder")
                    .clicked()
                {
                    set(BakCleanup::Searching);
                    let set = set.clone();
                    std::thread::spawn(move || {
                        let found = SubtitleUtils::find_subtitle_backups(&roots);
                        set(BakCleanup::Found(found));
                    });
                }
            }
            BakCleanup::Searching | BakCleanup::Deleting => {
                ui.label(RichText::new("Working…").color(ff::WARN));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            BakCleanup::Found(files) if files.is_empty() => {
                ui.label(RichText::new("No subtitle backups found.").color(ff::GOOD));
                if ui.small_button("OK").clicked() {
                    set(BakCleanup::Idle);
                }
            }
            BakCleanup::Found(files) => {
                ui.label(RichText::new(format!("Found {} subtitle backup file(s).", files.len())).color(ff::WARN));
                if ui
                    .button(RichText::new(format!("Delete {} files", files.len())).strong())
                    .on_hover_text("Only *.srt/.ass/.ssa/.sub/.vtt .bak copies are deleted; your current subtitles stay")
                    .clicked()
                {
                    set(BakCleanup::Deleting);
                    let set = set.clone();
                    std::thread::spawn(move || {
                        let mut deleted = 0;
                        let mut failed = 0;
                        for file in files {
                            if SubtitleUtils::is_subtitle_backup(&file) && std::fs::remove_file(&file).is_ok() {
                                deleted += 1;
                            } else {
                                failed += 1;
                            }
                        }
                        info!("Deleted {} subtitle backup file(s), {} failed", deleted, failed);
                        set(BakCleanup::Done { deleted, failed });
                    });
                }
                if ui.small_button("Cancel").clicked() {
                    set(BakCleanup::Idle);
                }
            }
            BakCleanup::Done { deleted, failed } => {
                let text = if failed > 0 {
                    format!("Deleted {deleted} file(s); {failed} couldn't be deleted.")
                } else {
                    format!("Deleted {deleted} file(s).")
                };
                ui.label(RichText::new(text).color(ff::GOOD));
                if ui.small_button("OK").clicked() {
                    set(BakCleanup::Idle);
                }
            }
        });
    }

    fn render_updates_section(&mut self, ui: &mut egui::Ui) {
        use crate::updater::UpdateState;
        ff::stat_label(ui, "UPDATES");
        if ui
            .checkbox(
                &mut self.check_updates,
                "Check for updates when Jenova starts",
            )
            .changed()
        {
            self.save_current_settings();
        }
        ui.horizontal(|ui| {
            let state = self.updater.state();
            let busy = matches!(
                state,
                UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installed
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Check now"))
                .clicked()
            {
                crate::updater::check_in_background(
                    std::sync::Arc::clone(&self.updater),
                    true,
                    Some(ui.ctx().clone()),
                );
            }
            let (text, color) = match &state {
                UpdateState::Idle => (
                    format!("You have v{}", crate::config::APP_VERSION),
                    TEXT_DIM,
                ),
                UpdateState::Checking => ("Checking GitHub…".to_string(), ff::WARN),
                UpdateState::UpToDate => (
                    format!("You're up to date (v{})", crate::config::APP_VERSION),
                    ff::GOOD,
                ),
                UpdateState::Available(release) => {
                    (format!("v{} is available", release.version), ff::WARN)
                }
                UpdateState::Downloading { .. } => ("Downloading update…".to_string(), ff::WARN),
                UpdateState::Installed => ("Restarting…".to_string(), ff::GOOD),
                UpdateState::Failed(error) => (error.clone(), ff::BAD),
            };
            ui.label(RichText::new(text).color(color));
            if let UpdateState::Available(_) = state {
                if ui.button("Update…").clicked() {
                    self.updater
                        .dismissed
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
            if busy {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
        });
    }

    /// "Update available" Yes/No prompt and download progress.
    fn render_update_prompt(&mut self, ctx: &egui::Context) {
        use crate::updater::UpdateState;
        use std::sync::atomic::Ordering::SeqCst;
        let state = self.updater.state();
        let show = match &state {
            UpdateState::Available(_) => !self.updater.dismissed.load(SeqCst),
            UpdateState::Downloading { .. } | UpdateState::Installed => true,
            UpdateState::Failed(_) => self.updater.manual.load(SeqCst),
            _ => false,
        };
        if !show {
            return;
        }
        let mut close = false;
        egui::Modal::new(egui::Id::new("ff_update_prompt"))
            .frame(egui::Frame::NONE)
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                ff::window(ui, |ui| {
                    ui.set_width(440.0);
                    match &state {
                        UpdateState::Available(release) => {
                            ff::title(ui, "Update available", 24.0, TEXT);
                            ff::divider(ui);
                            ui.horizontal(|ui| {
                                ff::stat_label(ui, "CURRENT");
                                ui.label(
                                    RichText::new(format!("v{}", crate::config::APP_VERSION))
                                        .strong(),
                                );
                                ui.add_space(16.0);
                                ff::stat_label(ui, "NEW");
                                ui.label(
                                    RichText::new(format!("v{}", release.version))
                                        .strong()
                                        .color(ff::GOOD),
                                );
                            });
                            if !release.notes.trim().is_empty() {
                                ui.add_space(4.0);
                                egui::ScrollArea::vertical()
                                    .id_salt("update_notes")
                                    .max_height(140.0)
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(release.notes.trim())
                                                .color(TEXT_DIM)
                                                .size(13.0),
                                        );
                                    });
                            }
                            ui.add_space(6.0);
                            if release.asset.is_none() {
                                ui.label(
                                    RichText::new("This release has no Windows download attached.")
                                        .color(ff::WARN),
                                );
                            }
                            ui.label("Update now? Jenova will restart.");
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                let can_install = release.asset.is_some();
                                if ui
                                    .add_enabled(
                                        can_install,
                                        egui::Button::new(RichText::new("Yes").strong().size(17.0))
                                            .min_size(egui::vec2(90.0, 30.0)),
                                    )
                                    .clicked()
                                {
                                    self.updater.manual.store(true, SeqCst);
                                    crate::updater::install_in_background(
                                        std::sync::Arc::clone(&self.updater),
                                        release.clone(),
                                        Some(ctx.clone()),
                                    );
                                }
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("No").strong().size(17.0))
                                            .min_size(egui::vec2(90.0, 30.0)),
                                    )
                                    .clicked()
                                {
                                    self.updater.dismissed.store(true, SeqCst);
                                }
                                if !release.page_url.is_empty() {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.hyperlink_to("Release page", &release.page_url);
                                    });
                                }
                            });
                        }
                        UpdateState::Downloading { release } => {
                            ff::title(ui, &format!("Updating to v{}", release.version), 22.0, TEXT);
                            ff::divider(ui);
                            let done = self.updater.downloaded.load(SeqCst) as f32;
                            let total = self.updater.total.load(SeqCst).max(1) as f32;
                            ui.horizontal(|ui| {
                                ff::stat_label(ui, "DOWNLOAD");
                                ff::stat_bar(
                                    ui,
                                    done / total,
                                    260.0,
                                    egui::Color32::from_rgb(200, 150, 20),
                                    egui::Color32::from_rgb(255, 236, 140),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{:.1} / {:.1} MB",
                                        done / 1_048_576.0,
                                        total / 1_048_576.0
                                    ))
                                    .strong(),
                                );
                            });
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        }
                        UpdateState::Installed => {
                            ff::title(ui, "Update installed", 22.0, TEXT);
                            ui.label("Restarting Jenova…");
                        }
                        UpdateState::Failed(error) => {
                            ff::title(ui, "Update failed", 22.0, ff::BAD);
                            ff::divider(ui);
                            ui.label(RichText::new(error).color(TEXT));
                            ui.add_space(6.0);
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        }
                        _ => {}
                    }
                });
            });
        if close {
            self.updater.manual.store(false, SeqCst);
            self.updater.reset();
        }
    }

    fn render_background_section(&mut self, ui: &mut egui::Ui) {
        ff::stat_label(ui, "BACKGROUND");
        if !crate::background::startup::supported() {
            ui.label(
                RichText::new("Running in the background isn't available on this system yet.")
                    .color(TEXT_DIM),
            );
            return;
        }
        let label = if cfg!(windows) {
            "Run in the background when Windows starts"
        } else {
            "Run in the background when you log in"
        };
        let mut enabled = self.background_service;
        if ui
            .checkbox(&mut enabled, label)
            .on_hover_text("Keeps watching your folders even when this window is closed. Turning it off removes the startup entry and stops the background process.")
            .changed()
        {
            if let Err(error) = self.set_background_service(enabled) {
                self.status = format!("Background mode: {error}");
                crate::warn!("Background mode change failed: {}", error);
            }
        }
        let running = crate::background::background_running();
        let registered = crate::background::startup::is_registered();
        let (text, color) = match (self.background_service, running, registered) {
            (true, true, true) => (
                "Background watcher is running. It pauses while this window is open and takes over when you close it.",
                ff::GOOD,
            ),
            (true, false, _) => ("Background watcher is starting…", ff::WARN),
            (true, true, false) => ("Running, but the startup entry is missing. Toggle off and on to fix.", ff::WARN),
            (false, true, _) => ("Stopping background watcher…", ff::WARN),
            (false, false, _) => ("Off. Folders are only watched while this window is open.", TEXT_DIM),
        };
        ui.label(RichText::new(text).color(color).size(13.0));
        if self.background_service {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn render_log_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Log", "Folder watch activity");
        let lines = self.watch.shared.log_lines();
        egui::ScrollArea::vertical()
            .id_salt("ff_log")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if lines.is_empty() {
                    ui.label(RichText::new("No activity yet.").color(TEXT_DIM));
                }
                for line in lines {
                    let (time, rest) = line.split_once("  ").unwrap_or(("", &line));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(time).color(LABEL).monospace());
                        ui.label(RichText::new(rest).monospace());
                    });
                }
            });
    }
}

fn page_title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ff::title(ui, title, 26.0, TEXT);
    ui.label(RichText::new(subtitle).color(TEXT_DIM));
    ff::divider(ui);
}

fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ff::title(ui, label, 18.0, LABEL);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ff::title(ui, value, 18.0, TEXT);
        });
    });
}

fn format_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_get_thousands_separators() {
        assert_eq!(super::format_count(0), "0");
        assert_eq!(super::format_count(1234), "1,234");
        assert_eq!(super::format_count(1234567), "1,234,567");
    }
}
