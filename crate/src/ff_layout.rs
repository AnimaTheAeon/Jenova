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
                let response = ff::menu_item(ui, label, self.menu_page == *page, enabled);
                if enabled && response.clicked() {
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
                                ff::stat_label(ui, "LV");
                                let videos = stats
                                    .map(|s| s.videos.to_string())
                                    .unwrap_or_else(|| "--".into());
                                ui.label(RichText::new(videos).strong());
                                ui.add_space(14.0);
                                ff::stat_label(ui, "SUBS");
                                let (fraction, caption) = match stats {
                                    Some(s) if s.videos > 0 => (
                                        s.with_subs as f32 / s.videos as f32,
                                        format!("{}/{}", s.with_subs, s.videos),
                                    ),
                                    Some(_) => (0.0, "0/0".into()),
                                    None => (0.0, "--/--".into()),
                                };
                                ff::stat_bar(
                                    ui,
                                    fraction,
                                    170.0,
                                    egui::Color32::from_rgb(30, 110, 230),
                                    egui::Color32::from_rgb(140, 230, 255),
                                )
                                .on_hover_text(
                                    "Videos that already have subtitles in your languages",
                                );
                                ui.label(RichText::new(caption).strong());
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
        page_title(ui, "Config", "Dependencies, performance and watch timing");
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
            ff::stat_label(ui, "NOTIFICATIONS");
            changed |= ui
                .checkbox(
                    &mut self.notify_desktop,
                    "Desktop notification when subtitles are found",
                )
                .changed();
            changed |= ui
                .checkbox(&mut self.notify_in_app, "Pop-up inside Jenova when subtitles are found")
                .changed();
            if changed {
                self.save_current_settings();
            }
            ff::divider(ui);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Jenova v{} · based on", crate::config::APP_VERSION)).color(TEXT_DIM));
                ui.hyperlink_to("fosterbarnes/rustitles", "https://github.com/fosterbarnes/rustitles");
                ui.label(RichText::new("· support the original:").color(TEXT_DIM));
                ui.hyperlink_to("coff.ee/fosterbarnes", "https://coff.ee/fosterbarnes");
            });
        });
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
