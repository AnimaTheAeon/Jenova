//! The MC360 look for Jenova: one skin blade per page (Watch, Activity,
//! Settings, Log), each with a menu of that page's actions. Picking an entry
//! opens the page on the blade, drawn exactly as in the other looks.

use eframe::egui;

use crate::data_structures::{MenuPage, SubtitleDownloader};
use crate::ff_layout::format_count;
use crate::ff_theme as ff;
use crate::mc360::{Blade, Event, Info, Item, Mc360, Tint};

const LOOKS: [ff::Theme; 5] = [ff::Theme::FfMenu, ff::Theme::GreenDash, ff::Theme::Blades, ff::Theme::Glass, ff::Theme::Mc360];
const PAGES: [MenuPage; 4] = [MenuPage::Watch, MenuPage::Activity, MenuPage::Config, MenuPage::Log];

// Entry numbers: blade * 100 + what to do there.
const ADD_FOLDER: u32 = 1;
const CHECK_NOW: u32 = 2;
const CHECK_UPDATES: u32 = 1;

fn blades() -> Vec<Blade> {
    vec![
        Blade {
            key: "watch".into(),
            tint: Tint::Green,
            items: vec![
                Item::new(0, "Watched folders", "folder", "Folders Jenova keeps an eye on."),
                Item::new(ADD_FOLDER, "Add folder…", "plus", "Watch another folder."),
                Item::new(CHECK_NOW, "Check existing files", "refresh", "Look through the watched folders now."),
            ],
        },
        Blade { key: "activity".into(), tint: Tint::Orange, items: vec![Item::new(100, "Recent", "clock", "What was downloaded, and what is still waiting.")] },
        Blade {
            key: "settings".into(),
            tint: Tint::Purple,
            items: vec![
                Item::new(200, "Settings", "cc", "Languages, providers, look and more."),
                Item::new(200 + CHECK_UPDATES, "Check for updates", "refresh", "Look for a newer Jenova now."),
            ],
        },
        Blade { key: "log".into(), tint: Tint::Grey, items: vec![Item::new(300, "Log", "doc", "Detailed log.")] },
    ]
}

impl SubtitleDownloader {
    fn mc360_check_updates(&mut self, ctx: &egui::Context) {
        crate::updater::check_in_background(std::sync::Arc::clone(&self.updater), true, Some(ctx.clone()));
    }

    pub fn render_mc360_layout(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let deps_ready = self.dependencies_ready();
        if !deps_ready {
            self.menu_page = MenuPage::Config;
        }
        let blades = blades();
        let cur_b = PAGES.iter().position(|p| *p == self.menu_page).unwrap_or(0);
        let mut mc = self.mc.take().unwrap_or_else(|| {
            let mut m = Mc360::new(dirs::config_dir().map(|d| d.join("Jenova").join("mc360.txt")));
            m.home(cur_b);
            m
        });
        // The page changed somewhere else (Settings > View Log, a setup step): follow it.
        if (mc.tier() == 2 || !deps_ready) && mc.blade() != cur_b {
            let id = blades[cur_b].items[0].id;
            mc.show_page(cur_b, id, &blades[cur_b].items);
        }
        let shared = &self.watch.shared;
        let queued = shared.queued.load(std::sync::atomic::Ordering::SeqCst) + shared.running.load(std::sync::atomic::Ordering::SeqCst);
        let card = vec![
            if !deps_ready { "Setup needed".into() } else if self.watch_enabled { format!("Watching {} folders", self.watch_folders.len()) } else { "Watching is paused".into() },
            format!("{} queued", format_count(queued)),
            format!("{} found today", format_count(shared.found_today())),
        ];
        let look_names: Vec<&str> = LOOKS.iter().map(|t| t.label()).collect();
        let checking = matches!(self.updater.state(), crate::updater::UpdateState::Checking);
        let info = Info {
            app: "Jenova",
            ver: crate::config::APP_VERSION,
            plat: "Windows",
            icon: "cc",
            card,
            looks: &look_names,
            look: 4,
            popup_open: false,
            app_calm: ff::calm(),
            checking,
        };
        let mut before_back = || false;
        let events = mc.ui(ui, &blades, &info, &mut before_back, &mut |child| {
            ff::set_hide_headings(true);
            self.render_page_body(child);
            ff::set_hide_headings(false);
        });
        for ev in events {
            match ev {
                Event::Open(b, item) => {
                    if !deps_ready {
                        continue;
                    }
                    self.menu_page = PAGES[b];
                    self.menu_touched = true;
                    match (b, item % 100) {
                        (0, ADD_FOLDER) => {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                let folder = folder.display().to_string();
                                if crate::helper_functions::Validation::is_valid_folder(&folder) && !self.watch_folders.contains(&folder) {
                                    self.watch_folders.push(folder);
                                    if self.watch_folders.len() == 1 {
                                        self.watch_enabled = true;
                                    }
                                    self.save_current_settings();
                                }
                            }
                        }
                        (0, CHECK_NOW) => {
                            if self.watch_enabled && !self.watch_folders.is_empty() {
                                self.watch.check_existing_now();
                            }
                        }
                        (2, CHECK_UPDATES) => self.mc360_check_updates(&ctx),
                        _ => {}
                    }
                }
                Event::Home(_) => {}
                Event::SetLook(i) => {
                    let theme = LOOKS[i.min(LOOKS.len() - 1)];
                    self.theme = theme;
                    ff::set_theme(theme);
                    ff::apply_style(&ctx);
                    self.green_in_page = true;
                    self.menu_touched = true;
                    self.save_current_settings();
                }
                Event::CheckUpdates => self.mc360_check_updates(&ctx),
            }
        }
        self.mc = Some(mc);
        let full = ui.max_rect();
        self.render_in_app_toasts(&ctx, egui::pos2(full.right() - 40.0, full.top() + 90.0));
    }
}
