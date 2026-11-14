//! Blades layout for Jenova: one blade per menu page (Watch, Activity,
//! Settings). The log opens inside the Settings blade. Drawing lives in
//! `blades` (shared with Lifestream and Turks).

use eframe::egui::{self, Align, Color32, Layout, UiBuilder};

use crate::blades::{self, Blade, Status};
use crate::data_structures::{MenuPage, SubtitleDownloader};
use crate::ff_layout::{format_count, MENU};
use crate::ff_theme as ff;

fn colors(page: MenuPage) -> [Color32; 4] {
    match page {
        MenuPage::Watch => blades::GREEN,
        MenuPage::Activity => blades::ORANGE,
        MenuPage::Config | MenuPage::Log => blades::PURPLE,
    }
}

impl SubtitleDownloader {
    pub fn render_blades_layout(&mut self, ui: &mut egui::Ui) {
        let deps_ready = self.dependencies_ready();
        if !deps_ready {
            self.menu_page = MenuPage::Config;
        }
        let shown = if self.menu_page == MenuPage::Log { MenuPage::Config } else { self.menu_page };
        let cur = MENU.iter().position(|m| m.0 == shown).unwrap_or(0);
        let list: Vec<Blade> = MENU.iter().map(|(p, l)| Blade { label: l, colors: colors(*p) }).collect();

        let shared = &self.watch.shared;
        let queued = shared.queued.load(std::sync::atomic::Ordering::SeqCst) + shared.running.load(std::sync::atomic::Ordering::SeqCst);
        let line = format!("{} queued · {} found today", format_count(queued), format_count(shared.found_today()));
        let (chip, dot) = if !deps_ready {
            ("Setup needed", blades::DOT_BAD)
        } else if self.watch_enabled {
            ("Watching", blades::DOT_GOOD)
        } else {
            ("Paused", blades::DOT_WARN)
        };
        let f = blades::show(ui, &mut self.blades, &list, cur, false, &Status { chip, dot, line: &line }, false);

        if deps_ready {
            if let Some(i) = f.go {
                self.menu_page = MENU[i].0;
                self.menu_touched = true;
            }
            if f.chip_clicked {
                self.menu_page = MenuPage::Watch;
                self.menu_touched = true;
            }
            // Esc or the mouse's back button: out of the log first, then back a blade.
            if f.esc || f.back {
                if self.menu_page == MenuPage::Log {
                    self.menu_page = MenuPage::Config;
                } else if f.back && cur > 0 {
                    self.menu_page = MENU[cur - 1].0;
                } else if f.esc && cur > 0 {
                    self.menu_page = MENU[0].0;
                }
            }
        }

        let mut child = ui.new_child(UiBuilder::new().id_salt(("blade_body", shown as u8)).max_rect(f.body).layout(Layout::top_down(Align::Min)));
        child.set_clip_rect(f.body.expand(4.0).intersect(f.visible));
        child.multiply_opacity(f.fade);
        ff::set_hide_headings(true);
        self.render_page_body(&mut child);
        ff::set_hide_headings(false);
    }
}
