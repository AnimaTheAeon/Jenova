//! Party-menu style layout: a main window, a command menu, a time/count box
//! and a status bar.

use std::sync::atomic::Ordering;

use eframe::egui::{self, pos2, Align, Layout, Rect, RichText, Vec2};
use rfd::FileDialog;

use crate::data_structures::{DownloadJob, MenuPage, SubtitleDownloader};
use crate::ff_theme::{self as ff, LABEL, TEXT, TEXT_DIM};
use crate::gui::render_activity_table;
use crate::helper_functions::{Utils, Validation};
use crate::info;
use crate::subtitle_utils::SubtitleUtils;

pub(crate) const MENU: &[(MenuPage, &str)] = &[
    (MenuPage::Watch, "Watch"),
    (MenuPage::Activity, "Activity"),
    (MenuPage::Config, "Settings"),
];

/// Menu label for a page. The log isn't in the menu; it opens from Settings
/// and counts as part of it.
pub(crate) fn page_label(page: MenuPage) -> &'static str {
    match page {
        MenuPage::Log => "Log",
        _ => MENU.iter().find(|(p, _)| *p == page).map(|(_, l)| *l).unwrap_or("Settings"),
    }
}

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
                    self.menu_touched
                        && (self.menu_page == *page
                            || (*page == MenuPage::Config && self.menu_page == MenuPage::Log)),
                    enabled,
                );
                if enabled && response.clicked() {
                    self.menu_touched = true;
                    self.menu_page = *page;
                }
            }
        });

        ff::window_at(ui, info, "ff_info", |ui| {
            let shared = &self.watch.shared;
            let queued = shared.queued.load(Ordering::SeqCst) + shared.running.load(Ordering::SeqCst);
            info_row(ui, "Queued", &format_count(queued));
            ui.add_space(4.0);
            info_row(ui, "Found today", &format_count(shared.found_today()));
        });

        ff::window_at(ui, bar, "ff_status", |ui| {
            ui.add_space(-4.0);
            ui.horizontal_centered(|ui| {
                ff::title(ui, &self.status_line(), 16.0, TEXT());
            });
        });

        self.render_in_app_toasts(
            &ui.ctx().clone(),
            pos2(main.right() - 14.0, main.top() + 14.0),
        );

        self.render_update_prompt(ui.ctx());

        ff::window_at(ui, main, "ff_main", |ui| self.render_page_body(ui));
    }

    /// The current page's contents; shared by both themes' layouts.
    pub(crate) fn render_page_body(&mut self, ui: &mut egui::Ui) {
        if !self.dependencies_ready() {
            self.render_config_page(ui);
            return;
        }
        match self.menu_page {
            MenuPage::Watch => self.render_watch_page(ui),
            MenuPage::Activity => self.render_activity_page(ui),
            MenuPage::Config => self.render_config_page(ui),
            MenuPage::Log => self.render_log_page(ui),
        }
    }

    /// Menu-style "Subtitles found" pop-ups in the top-right of the main window.
    pub fn render_in_app_toasts(&self, ctx: &egui::Context, anchor: egui::Pos2) {
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
                        ff::title(ui, &item.toast.title, 15.0, LABEL());
                        ui.add(
                            egui::Label::new(RichText::new(&item.toast.body).color(TEXT()).strong())
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

    pub(crate) fn status_line(&self) -> String {
        let shared = &self.watch.shared;
        if !self.dependencies_ready() {
            return "Install the missing dependencies to begin.".into();
        }
        let running = shared.running.load(Ordering::SeqCst);
        let queued = shared.queued.load(Ordering::SeqCst);
        if running + queued > 0 {
            return if queued > 0 {
                format!("Getting subtitles: {running} running, {queued} waiting")
            } else {
                format!("Getting subtitles for {running} video(s)")
            };
        }
        if shared.catching_up.load(Ordering::SeqCst) {
            return "Checking existing files…".into();
        }
        if self.watch_enabled && !self.watch_folders.is_empty() {
            let n = shared.watched_folders.load(Ordering::SeqCst);
            let settling = shared.settling.load(Ordering::SeqCst);
            if settling > 0 {
                return format!("Watching {n} folder(s). {settling} new file(s) settling…");
            }
            let waiting = shared.retry_waiting.load(Ordering::SeqCst);
            if waiting > 0 {
                return format!("Watching {n} folder(s). {waiting} video(s) waiting for subtitles to come out.");
            }
            return format!("Watching {n} folder(s) for new videos.");
        }
        if self.watch_folders.is_empty() {
            return "Add a folder on the Watch page to get started.".into();
        }
        "Watching is paused.".into()
    }

    fn render_watch_page(&mut self, ui: &mut egui::Ui) {
        let shared = std::sync::Arc::clone(&self.watch.shared);
        let mut changed = false;

        ui.horizontal(|ui| {
            ff::title(ui, "Watch", 26.0, TEXT());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, color) = if self.watch_enabled {
                    ("WATCHING: ON", ff::GOOD())
                } else {
                    ("WATCHING: OFF", ff::BAD())
                };
                if ui
                    .add(egui::Button::new(RichText::new(label).color(color).strong()))
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
                .color(TEXT_DIM()),
        );
        ff::divider(ui);

        let catching_up = shared.catching_up.load(Ordering::SeqCst);
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
                        ff::title(ui, "No folders yet", 20.0, TEXT_DIM());
                        ui.label(
                            RichText::new("Add your Movies or TV folder to start watching.")
                                .color(TEXT_DIM()),
                        );
                    });
                }
                for (index, folder) in self.watch_folders.iter().enumerate() {
                    let path = std::path::Path::new(folder);
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| folder.clone());
                    let online = shared.folder_online(path);
                    let (status, status_color) = match (self.watch_enabled, online) {
                        (_, Some(false)) => ("OFFLINE", ff::BAD()),
                        (false, _) => ("PAUSED", ff::WARN()),
                        (true, Some(true)) => ("WATCHING", ff::GOOD()),
                        (true, None) => ("STARTING", TEXT_DIM()),
                    };
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ff::title(ui, &name, 20.0, TEXT());
                        ui.add_space(12.0);
                        ff::title(ui, status, 14.0, status_color)
                            .on_hover_text(match status {
                                "OFFLINE" => "Can't reach this folder right now (drive disconnected?). Jenova picks it up again when it's back.",
                                "PAUSED" => "Watching is off",
                                _ => "",
                            });
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
                    ui.label(RichText::new(folder).color(TEXT_DIM()).size(12.0));
                    ui.horizontal(|ui| {
                        match shared.stats_for(path) {
                            Some(stats) if stats.videos > 0 => {
                                let fraction = stats.with_subs as f32 / stats.videos as f32;
                                ff::stat_bar(
                                    ui,
                                    fraction,
                                    120.0,
                                    egui::Color32::from_rgb(30, 110, 230),
                                    egui::Color32::from_rgb(140, 230, 255),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} of {} videos have subtitles",
                                        format_count(stats.with_subs),
                                        format_count(stats.videos)
                                    ))
                                    .color(TEXT_DIM()),
                                );
                            }
                            Some(_) => {
                                ui.label(RichText::new("No videos yet").color(TEXT_DIM()));
                            }
                            None if self.watch_enabled && online != Some(false) => {
                                ui.label(RichText::new("Counting videos…").color(TEXT_DIM()));
                            }
                            None => {}
                        }
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
                        if Validation::is_valid_folder(&folder) && !self.watch_folders.contains(&folder) {
                            info!("Added watch folder: {}", folder);
                            self.watch_folders.push(folder);
                            if self.watch_folders.len() == 1 {
                                self.watch_enabled = true;
                            }
                            changed = true;
                        }
                    }
                }
                let can_check = self.watch_enabled && !self.watch_folders.is_empty() && !catching_up;
                if ui
                    .add_enabled(can_check, egui::Button::new("Check existing files"))
                    .on_hover_text(
                        "Look through the watched folders now for videos missing subtitles. Jenova also does this once a day by itself.",
                    )
                    .clicked()
                {
                    self.watch.check_existing_now();
                }
                if ui
                    .add_enabled(!catching_up, egui::Button::new("Scan a folder once…"))
                    .on_hover_text("Get subtitles for a folder you don't want to watch, one time")
                    .clicked()
                {
                    if let Some(folder) = FileDialog::new().pick_folder() {
                        info!("Scanning once: {}", folder.display());
                        self.watch.scan_folder(folder);
                        self.menu_page = MenuPage::Activity;
                    }
                }
                if catching_up {
                    ui.label(RichText::new("Checking…").color(ff::WARN()));
                }
            });
        });

        if changed {
            self.save_current_settings();
        }
    }

    fn render_activity_page(&mut self, ui: &mut egui::Ui) {
        let busy = self.watch.shared.busy();
        ui.horizontal(|ui| {
            ff::title(ui, "Activity", 26.0, TEXT());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Clear finished").clicked() {
                    self.watch.clear_finished();
                }
                if busy
                    && ui
                        .button("Cancel downloads")
                        .on_hover_text("Stop what's running and empty the queue")
                        .clicked()
                {
                    self.watch.cancel_all();
                }
            });
        });
        ui.label(
            RichText::new(
                "Videos Jenova looked at, newest first. When nothing's online yet, it tries again later by itself.",
            )
            .color(TEXT_DIM()),
        );
        ff::divider(ui);
        let jobs: Vec<DownloadJob> = self.watch.shared.jobs_snapshot().into_iter().rev().collect();
        if jobs.is_empty() {
            ui.label(RichText::new("Nothing yet. New videos will show up here.").color(TEXT_DIM()));
        } else {
            let roots: Vec<std::path::PathBuf> =
                self.watch_folders.iter().map(std::path::PathBuf::from).collect();
            render_activity_table(ui, &jobs, &roots, "activity_jobs");
        }
    }

    fn render_config_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ff::title(ui, "Settings", 26.0, TEXT());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.dependencies_ready()
                    && ui
                        .button(RichText::new("View Log").size(16.0))
                        .on_hover_text("Folder watch activity")
                        .clicked()
                {
                    self.menu_page = MenuPage::Log;
                }
            });
        });
        ui.label(RichText::new("Changes take effect right away.").color(TEXT_DIM()));
        ff::divider(ui);
        egui::ScrollArea::vertical().id_salt("config_scroll").show(ui, |ui| {
            if !self.dependencies_ready() {
                ff::stat_label(ui, "DEPENDENCIES");
                self.render_dependencies(ui);
                return;
            }
            let mut changed = false;

            ff::stat_label(ui, "LANGUAGES");
            self.render_languages(ui);
            changed |= ui
                .checkbox(&mut self.exclude_hearing_impaired, "Skip hearing-impaired (SDH) subtitles")
                .changed();
            ff::divider(ui);

            ff::stat_label(ui, "WATCHING");
            ui.horizontal(|ui| {
                ui.label("Wait until a new file stops changing for");
                changed |= ui
                    .add(egui::DragValue::new(&mut self.watch_settle_seconds).range(5..=3600).suffix(" s"))
                    .on_hover_text("Gives downloads and copies time to finish before Subliminal reads the file")
                    .changed();
            });
            changed |= ui
                .checkbox(&mut self.watch_catch_up, "Check existing files once a day")
                .on_hover_text("Catches videos added while Jenova wasn't running")
                .changed();
            changed |= ui
                .checkbox(&mut self.ignore_local_extras, "Skip Plex extras folders")
                .on_hover_text("Behind The Scenes, Deleted Scenes, Featurettes, Interviews, Scenes, Shorts, Trailers and Other")
                .changed();
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut self.watch_polling, "Polling mode (NAS / network drives)")
                    .on_hover_text("Use this if new files on a mapped drive or network share aren't noticed")
                    .changed();
                if self.watch_polling {
                    ui.label("every");
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.watch_poll_interval_seconds).range(5..=3600).suffix(" s"))
                        .changed();
                }
            });
            ff::divider(ui);

            ff::stat_label(ui, "DOWNLOADS");
            self.render_concurrent_downloads(ui);
            changed |= ui
                .checkbox(&mut self.force_download, "Download even if the video has embedded subtitles")
                .changed();
            ff::divider(ui);

            ff::stat_label(ui, "SUBTITLE SITES");
            self.render_providers(ui);
            ff::divider(ui);

            self.render_startup_section(ui);
            changed |= ui
                .checkbox(&mut self.notify_desktop, "Notify me when subtitles are found")
                .on_hover_text("A pop-up inside Jenova while it's open, a Windows notification while it's in the tray")
                .changed();
            ff::divider(ui);

            ff::stat_label(ui, "LOOK");
            ui.horizontal(|ui| {
                for theme in [ff::Theme::FfMenu, ff::Theme::GreenDash, ff::Theme::Blades] {
                    let selected = self.theme == theme;
                    let text = if selected {
                        RichText::new(theme.label()).strong().color(ff::pal().on_accent)
                    } else {
                        RichText::new(theme.label())
                    };
                    let mut button = egui::Button::new(text);
                    if selected {
                        button = button.fill(ff::pal().accent);
                    }
                    if ui.add(button).clicked() && !selected {
                        self.theme = theme;
                        ff::set_theme(theme);
                        ff::apply_style(ui.ctx());
                        // Land on this page in the new layout too.
                        self.green_in_page = true;
                        self.menu_touched = true;
                        self.save_current_settings();
                    }
                }
            });
            ff::divider(ui);

            if changed {
                self.save_current_settings();
            }

            self.render_updates_section(ui);
            ff::divider(ui);

            let advanced = egui::CollapsingHeader::new(RichText::new("Advanced").color(LABEL()).strong())
                .open(Some(self.advanced_open))
                .show(ui, |ui| {
                    let mut changed = ui
                        .checkbox(&mut self.skip_scanned_media, "Skip videos Jenova already got subtitles for")
                        .on_hover_text("Unless the video file changed or you added a language")
                        .changed();
                    ui.add_space(4.0);
                    self.render_matching_advanced(ui);
                    ui.add_space(4.0);
                    ff::stat_label(ui, "DEPENDENCIES");
                    self.render_dependencies(ui);
                    ui.add_space(4.0);
                    self.render_bak_cleanup(ui);
                    if changed {
                        self.save_current_settings();
                        changed = false;
                    }
                    let _ = changed;
                });
            if advanced.header_response.clicked() {
                self.advanced_open = !self.advanced_open;
            }
            ff::divider(ui);
            ui.label(RichText::new(format!("Jenova v{}", crate::config::APP_VERSION)).color(TEXT_DIM()));
        });
    }

    fn render_dependencies(&mut self, ui: &mut egui::Ui) {
        self.render_python_status(ui);
        self.render_pipx_status(ui);
        self.render_subliminal_status(ui);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        self.render_ffmpeg_status(ui);
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
                ui.label(RichText::new("Older versions saved copies like Movie.en.srt.bak next to your subtitles.").color(TEXT_DIM()));
                if ui
                    .add_enabled(!roots.is_empty(), egui::Button::new("Find .bak files"))
                    .on_hover_text("Searches your watched folders")
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
                ui.label(RichText::new("Working…").color(ff::WARN()));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            BakCleanup::Found(files) if files.is_empty() => {
                ui.label(RichText::new("No subtitle backups found.").color(ff::GOOD()));
                if ui.small_button("OK").clicked() {
                    set(BakCleanup::Idle);
                }
            }
            BakCleanup::Found(files) => {
                ui.label(RichText::new(format!("Found {} subtitle backup file(s).", files.len())).color(ff::WARN()));
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
                ui.label(RichText::new(text).color(ff::GOOD()));
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
                    TEXT_DIM(),
                ),
                UpdateState::Checking => ("Checking GitHub…".to_string(), ff::WARN()),
                UpdateState::UpToDate => (
                    format!("You're up to date (v{})", crate::config::APP_VERSION),
                    ff::GOOD(),
                ),
                UpdateState::Available(release) => {
                    (format!("v{} is available", release.version), ff::WARN())
                }
                UpdateState::Downloading { .. } => ("Downloading update…".to_string(), ff::WARN()),
                UpdateState::Installed => ("Restarting…".to_string(), ff::GOOD()),
                UpdateState::Failed(error) => (error.clone(), ff::BAD()),
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
    pub(crate) fn render_update_prompt(&mut self, ctx: &egui::Context) {
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
                            ff::title(ui, "Update available", 24.0, TEXT());
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
                                        .color(ff::GOOD()),
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
                                                .color(TEXT_DIM())
                                                .size(13.0),
                                        );
                                    });
                            }
                            ui.add_space(6.0);
                            if release.asset.is_none() {
                                ui.label(
                                    RichText::new("This release has no Windows download attached.")
                                        .color(ff::WARN()),
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
                            });
                        }
                        UpdateState::Downloading { release } => {
                            ff::title(ui, &format!("Updating to v{}", release.version), 22.0, TEXT());
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
                            ff::title(ui, "Update installed", 22.0, TEXT());
                            ui.label("Restarting Jenova…");
                        }
                        UpdateState::Failed(error) => {
                            ff::title(ui, "Update failed", 22.0, ff::BAD());
                            ff::divider(ui);
                            ui.label(RichText::new(error).color(TEXT()));
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

    fn render_startup_section(&mut self, ui: &mut egui::Ui) {
        ff::stat_label(ui, "STARTUP & NOTIFICATIONS");
        if crate::background::startup::supported() {
            let label = if cfg!(windows) {
                "Start with Windows (hidden in the tray)"
            } else {
                "Start when you log in"
            };
            let mut enabled = self.background_service;
            if ui
                .checkbox(&mut enabled, label)
                .on_hover_text("Turning it off removes the startup entry completely")
                .changed()
            {
                self.set_start_with_windows(enabled);
            }
        }
        if crate::tray::available() {
            ui.label(
                RichText::new("Closing the window keeps Jenova watching from the tray. Right-click its tray icon to quit.")
                    .color(TEXT_DIM())
                    .size(12.0),
            );
        }
    }

    fn render_log_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ff::title(ui, "Log", 26.0, TEXT());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("Back to Settings").size(16.0)).clicked() {
                    self.menu_page = MenuPage::Config;
                }
            });
        });
        ui.label(RichText::new("Folder watch activity").color(TEXT_DIM()));
        ff::divider(ui);
        let lines = self.watch.shared.log_lines();
        egui::ScrollArea::vertical()
            .id_salt("ff_log")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if lines.is_empty() {
                    ui.label(RichText::new("No activity yet.").color(TEXT_DIM()));
                }
                for line in lines {
                    let (time, rest) = line.split_once("  ").unwrap_or(("", &line));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(time).color(LABEL()).monospace());
                        ui.label(RichText::new(rest).monospace());
                    });
                }
            });
    }
}

fn info_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ff::title(ui, label, 18.0, LABEL());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ff::title(ui, value, 18.0, TEXT());
        });
    });
}

pub(crate) fn format_count(n: usize) -> String {
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
