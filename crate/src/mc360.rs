//! "MC360": the MC360 dashboard skin's blades around the app's own pages.
//! One blade per menu section, each with a menu of that section's pages; the
//! open page sits on the blade in a dark see-through pane, drawn by the app
//! exactly as in its other looks. Three skins (Default, Glass, Carbon), the
//! skin's sounds, a guide (G) with look, skin, sounds and updates.
//! Shared by Lifestream, Turks and Jenova; the same layout as the web pages.

#![allow(clippy::too_many_arguments, dead_code)]

use crate::mc360_data::{ATLAS_SIZE, GRADS, SPRITES};
use eframe::egui;
use egui::{pos2, vec2, Align2, Color32, FontFamily, FontId, Id, Key, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureId, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

const SLIDE: f32 = 0.7;
const SKINS: [&str; 3] = ["default", "glass", "carbon"];
const SKIN_LABELS: [&str; 3] = ["Default", "Glass", "Carbon"];
const VOLS: [f32; 5] = [0.2, 0.4, 0.6, 0.8, 1.0];
const SWIRL_FRAMES: usize = 16;
const SWIRL_SECS: f32 = 4.3;
// Left edge of the open blade's strip and of a right-hand blade's (from the skin art, 128x1024 drawn 80x576).
const EDGE: [(f32, f32); 17] = [(0., 8.), (64., 16.), (128., 23.), (192., 29.), (256., 34.), (320., 38.), (384., 40.), (448., 42.), (512., 43.), (576., 43.), (640., 42.), (704., 39.), (768., 36.), (832., 31.), (896., 25.), (960., 18.), (1024., 11.)];
const EDGE_R: [(f32, f32); 17] = [(0., 54.), (64., 45.), (128., 38.), (192., 32.), (256., 28.), (320., 24.), (384., 21.), (448., 19.), (512., 19.), (576., 19.), (640., 20.), (704., 22.), (768., 25.), (832., 30.), (896., 35.), (960., 42.), (1024., 51.)];

pub const FONT: &str = "mc360";

/// Adds the skin's font as the "mc360" family. Call from the app's font setup.
pub fn add_font(fonts: &mut egui::FontDefinitions) {
    fonts.font_data.insert(FONT.to_owned(), Arc::new(egui::FontData::from_static(include_bytes!("mc360/mc360.ttf"))));
    let mut fam = vec![FONT.to_owned()];
    if let Some(p) = fonts.families.get(&FontFamily::Proportional) {
        fam.extend(p.iter().cloned());
    }
    fonts.families.insert(FontFamily::Name(FONT.into()), fam);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tint {
    Green,
    Orange,
    Blue,
    Purple,
    Grey,
    Red,
}

impl Tint {
    fn key(self) -> &'static str {
        match self {
            Tint::Green => "green",
            Tint::Orange => "orange",
            Tint::Blue => "blue",
            Tint::Purple => "purple",
            Tint::Grey => "grey",
            Tint::Red => "red",
        }
    }
    fn haze(self) -> Option<&'static str> {
        match self {
            Tint::Green | Tint::Orange | Tint::Blue | Tint::Purple => Some(self.key()),
            _ => None,
        }
    }
}

/// One menu entry: `id` is the app's own number for it.
#[derive(Clone)]
pub struct Item {
    pub id: u32,
    pub label: String,
    pub icon: &'static str,
    pub hint: String,
}

impl Item {
    pub fn new(id: u32, label: &str, icon: &'static str, hint: &str) -> Self {
        Item { id, label: label.into(), icon, hint: hint.into() }
    }
}

#[derive(Clone)]
pub struct Blade {
    pub key: String,
    pub tint: Tint,
    pub items: Vec<Item>,
}

/// What the app tells the look each frame.
pub struct Info<'a> {
    pub app: &'a str,
    pub ver: &'a str,
    pub plat: &'a str,
    pub icon: &'static str,
    pub card: Vec<String>,
    /// Look names for the guide; `look` is the current one (MC360).
    pub looks: &'a [&'a str],
    pub look: usize,
    /// A dialog of the app's is open: keys and clicks belong to it.
    pub popup_open: bool,
    /// The app's own Reduce motion setting.
    pub app_calm: bool,
    /// Shown in the guide's update row while a check runs.
    pub checking: bool,
}

/// What happened, for the app to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Open this blade's entry (the app shows that page).
    Open(usize, u32),
    /// The menu of this blade is showing (no page).
    Home(usize),
    /// Esc or Back on a page: the app may step back inside the page first;
    /// return true from `before_back` for that.
    SetLook(usize),
    CheckUpdates,
}

#[derive(Clone, Copy)]
struct Prefs {
    skin: usize,
    sound: bool,
    vol: f32,
    calm: bool,
}

struct Slide {
    from_b: usize,
    from_sel: usize,
    dir: i32,
    d: f32,
    t0: Instant,
}

/// A value that glides to new targets like a CSS transition.
#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    t0: Option<Instant>,
    secs: f32,
}

impl Glide {
    fn new(v: f32, secs: f32) -> Self {
        Glide { from: v, to: v, t0: None, secs }
    }
    fn get(&self) -> f32 {
        match self.t0 {
            Some(t0) => {
                let k = t0.elapsed().as_secs_f32() / self.secs;
                if k >= 1.0 { self.to } else { self.from + (self.to - self.from) * ease(k) }
            }
            None => self.to,
        }
    }
    fn set(&mut self, v: f32, animate: bool) {
        if (v - self.to).abs() < 0.01 {
            return;
        }
        self.from = if animate { self.get() } else { v };
        self.to = v;
        self.t0 = if animate { Some(Instant::now()) } else { None };
    }
    fn moving(&self) -> bool {
        self.t0.is_some_and(|t| t.elapsed().as_secs_f32() < self.secs)
    }
}

/// CSS cubic-bezier(.22,.9,.25,1), the Blades look's slide.
fn ease(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    let (x1, y1, x2, y2) = (0.22_f32, 0.9_f32, 0.25_f32, 1.0_f32);
    let bez = |t: f32, a: f32, b: f32| 3.0 * (1.0 - t) * (1.0 - t) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t;
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(mid, x1, x2) < k { lo = mid } else { hi = mid }
    }
    bez((lo + hi) / 2.0, y1, y2)
}

fn edge_at(edge: &[(f32, f32); 17], y: f32) -> f32 {
    let ty = (y / 0.5625).clamp(0.0, 1024.0);
    for w in edge.windows(2) {
        if ty <= w[1].0 {
            let k = (ty - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * k;
        }
    }
    edge[16].1
}

fn grad(name: &str) -> Option<(bool, &'static [(f32, u8, u8, u8, u8)])> {
    GRADS.iter().find(|g| g.0 == name).map(|g| (g.1, g.2))
}

fn grad_at(stops: &[(f32, u8, u8, u8, u8)], u: f32) -> Color32 {
    let u = u.clamp(0.0, 1.0);
    let c = |s: &(f32, u8, u8, u8, u8)| [s.1 as f32, s.2 as f32, s.3 as f32, s.4 as f32];
    let mut a = c(&stops[0]);
    let mut out = a;
    for w in stops.windows(2) {
        if u <= w[1].0 {
            let k = ((u - w[0].0) / (w[1].0 - w[0].0).max(1e-6)).clamp(0.0, 1.0);
            let (p, q) = (c(&w[0]), c(&w[1]));
            out = [0, 1, 2, 3].map(|i| p[i] + (q[i] - p[i]) * k);
            return Color32::from_rgba_unmultiplied(out[0] as u8, out[1] as u8, out[2] as u8, out[3] as u8);
        }
        a = c(&w[1]);
        out = a;
    }
    Color32::from_rgba_unmultiplied(out[0] as u8, out[1] as u8, out[2] as u8, out[3] as u8)
}

fn sprite(name: &str) -> Option<(u8, f32, f32, f32, f32)> {
    SPRITES.iter().find(|s| s.0 == name).map(|s| (s.1, s.2 as f32, s.3 as f32, s.4 as f32, s.5 as f32))
}

/// Icon names from the web engine, mapped to the skin's icon pictures.
fn icon_sprite(name: &str) -> &str {
    match name {
        "list" | "folder" => "defaultfolderbig",
        "down" => "icon-sortby",
        "up" => "defaultfolderbackbig",
        "plus" => "icon-custom",
        "trash" | "block" => "icon-clear",
        "rss" | "net" => "icon-network",
        "rule" | "gear" => "icon-scripts",
        "shield" => "defaultlockedbig",
        "gauge" => "icon-computer",
        "palette" => "icon-appearance",
        "refresh" => "icon-update",
        "info" => "icon-info",
        "doc" => "defaultfilebig",
        "alert" => "icon-question",
        "copy" => "icon-stack",
        "gem" | "star" => "icon-trophy",
        "search" => "icon-search",
        "clock" => "icon-repeat",
        "apps" => "icon-switchview",
        "plug" => "defaultnetworkbig",
        "bell" => "icon-messages",
        "tag" => "icon-profile",
        "tv" => "defaultvideobig",
        "film" => "icon-trailer",
        "cal" => "icon-viewas",
        "queue" => "defaultplaylistbig",
        "disk" => "defaultharddiskbig",
        "task" => "icon-scan",
        "cc" => "icon-slideshow",
        "eye" => "icon-pictures",
        "bolt" => "icon-power",
        "swirl" => "icon-randomize",
        "user" => "icon-friends",
        "music" => "icon-music",
        other => other,
    }
}

// ---------- sound ----------

const SOUNDS: [(&str, &[u8]); 15] = [
    ("back", include_bytes!("mc360/snd/back.wav")),
    ("bladeslide_1", include_bytes!("mc360/snd/bladeslide_1.wav")),
    ("bladeslide_2", include_bytes!("mc360/snd/bladeslide_2.wav")),
    ("bladeslide_3", include_bytes!("mc360/snd/bladeslide_3.wav")),
    ("bladeslide_4", include_bytes!("mc360/snd/bladeslide_4.wav")),
    ("cursor", include_bytes!("mc360/snd/cursor.wav")),
    ("guide_close", include_bytes!("mc360/snd/guide_close.wav")),
    ("guide_open", include_bytes!("mc360/snd/guide_open.wav")),
    ("startup", include_bytes!("mc360/snd/startup.wav")),
    ("notification", include_bytes!("mc360/snd/notification.wav")),
    ("select", include_bytes!("mc360/snd/select.wav")),
    ("tier2_enter", include_bytes!("mc360/snd/tier2_enter.wav")),
    ("tier2_exit", include_bytes!("mc360/snd/tier2_exit.wav")),
    ("tier3_enter", include_bytes!("mc360/snd/tier3_enter.wav")),
    ("tier3_exit", include_bytes!("mc360/snd/tier3_exit.wav")),
];

/// The WAV with its 16-bit samples scaled to `vol`.
fn scaled(wav: &[u8], vol: f32) -> Vec<u8> {
    let mut out = wav.to_vec();
    // Find the "data" chunk.
    let mut i = 12;
    while i + 8 <= out.len() {
        let len = u32::from_le_bytes([out[i + 4], out[i + 5], out[i + 6], out[i + 7]]) as usize;
        if &out[i..i + 4] == b"data" {
            let end = (i + 8 + len).min(out.len());
            let mut j = i + 8;
            while j + 1 < end {
                let s = i16::from_le_bytes([out[j], out[j + 1]]) as f32 * vol;
                let b = (s.clamp(-32768.0, 32767.0) as i16).to_le_bytes();
                out[j] = b[0];
                out[j + 1] = b[1];
                j += 2;
            }
            break;
        }
        i += 8 + len + (len & 1);
    }
    out
}

#[derive(Default)]
struct Sound {
    cache: HashMap<(&'static str, u8), Arc<Vec<u8>>>,
    // Windows plays straight from this memory, so the last two stay alive.
    playing: Vec<Arc<Vec<u8>>>,
}

impl Sound {
    fn play(&mut self, name: &'static str, vol: f32) {
        let Some((_, wav)) = SOUNDS.iter().find(|s| s.0 == name) else { return };
        let key = (name, (vol * 100.0) as u8);
        let buf = self.cache.entry(key).or_insert_with(|| Arc::new(scaled(wav, vol))).clone();
        play_wav(&buf);
        self.playing.push(buf);
        if self.playing.len() > 2 {
            self.playing.remove(0);
        }
    }
}

#[cfg(windows)]
fn play_wav(buf: &[u8]) {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    unsafe {
        let _ = PlaySoundW(PCWSTR(buf.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(not(windows))]
fn play_wav(_buf: &[u8]) {}

// ---------- textures ----------

const ATLASES: [&[u8]; 6] = [
    include_bytes!("mc360/blades-default.png"),
    include_bytes!("mc360/blades-glass.png"),
    include_bytes!("mc360/blades-carbon.png"),
    include_bytes!("mc360/highlights.png"),
    include_bytes!("mc360/icons.png"),
    include_bytes!("mc360/ui.png"),
];

macro_rules! swirl {
    ($s:literal) => {
        [
            include_bytes!(concat!("mc360/swirl/", $s, "-00.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-01.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-02.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-03.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-04.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-05.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-06.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-07.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-08.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-09.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-10.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-11.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-12.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-13.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-14.jpg")).as_slice(),
            include_bytes!(concat!("mc360/swirl/", $s, "-15.jpg")).as_slice(),
        ]
    };
}
const SWIRL: [[&[u8]; SWIRL_FRAMES]; 3] = [swirl!("default"), swirl!("glass"), swirl!("carbon")];
const RING: &[u8] = include_bytes!("mc360/ring.png");

fn load(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(name, ci, egui::TextureOptions::LINEAR))
}

#[derive(Default)]
struct Tex {
    atlas: Vec<Option<TextureHandle>>,
    swirl: Vec<Option<TextureHandle>>,
    swirl_skin: usize,
    ring: Option<TextureHandle>,
}

impl Tex {
    fn atlas(&mut self, ctx: &egui::Context, i: u8) -> Option<TextureId> {
        if self.atlas.is_empty() {
            self.atlas = vec![None; ATLASES.len()];
        }
        let i = i as usize;
        if self.atlas[i].is_none() {
            self.atlas[i] = load(ctx, &format!("mc360-atlas-{i}"), ATLASES[i]);
        }
        self.atlas[i].as_ref().map(|t| t.id())
    }
    fn swirl(&mut self, ctx: &egui::Context, skin: usize, f: usize) -> Option<TextureId> {
        if self.swirl.is_empty() || self.swirl_skin != skin {
            self.swirl = vec![None; SWIRL_FRAMES];
            self.swirl_skin = skin;
        }
        if self.swirl[f].is_none() {
            self.swirl[f] = load(ctx, &format!("mc360-swirl-{skin}-{f}"), SWIRL[skin][f]);
        }
        self.swirl[f].as_ref().map(|t| t.id())
    }
    fn ring(&mut self, ctx: &egui::Context) -> Option<TextureId> {
        if self.ring.is_none() {
            self.ring = load(ctx, "mc360-ring", RING);
        }
        self.ring.as_ref().map(|t| t.id())
    }
}

// ---------- the look ----------

struct GuideRow {
    label: String,
    value: Option<String>,
    check: Option<bool>,
    kind: GuideKind,
}

#[derive(Clone, Copy, PartialEq)]
enum GuideKind {
    Look,
    Skin,
    Sounds,
    Volume,
    Calm,
    Updates,
}

pub struct Mc360 {
    prefs: Prefs,
    prefs_path: Option<PathBuf>,
    b: usize,
    sel: usize,
    last_sel: usize,
    tier: u8,
    item: Option<u32>,
    guide: bool,
    guide_t0: Instant,
    gsel: usize,
    look_pick: Option<usize>,
    slide: Option<Slide>,
    blade_x: Vec<Glide>,
    frame_x0: Glide,
    frame_end: Glide,
    haze: Glide,
    content_t0: Option<(Instant, f32)>,
    page_t0: Option<Instant>,
    toast: Option<(String, String, Instant)>,
    tex: Tex,
    sound: Sound,
    started: bool,
    n_last: usize,
    w_last: f32,
}

impl Mc360 {
    /// `prefs_path`: a small file for the skin, sound and motion choices.
    pub fn new(prefs_path: Option<PathBuf>) -> Self {
        let mut prefs = Prefs { skin: 0, sound: true, vol: 0.6, calm: false };
        if let Some(text) = prefs_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                let (k, v) = line.split_once('=').unwrap_or(("", ""));
                match k.trim() {
                    "skin" => prefs.skin = SKINS.iter().position(|s| *s == v.trim()).unwrap_or(0),
                    "sound" => prefs.sound = v.trim() != "off",
                    "volume" => prefs.vol = v.trim().parse::<f32>().ok().filter(|x| VOLS.contains(x)).unwrap_or(0.6),
                    "calm" => prefs.calm = v.trim() == "on",
                    _ => {}
                }
            }
        }
        Mc360 {
            prefs,
            prefs_path,
            b: 0,
            sel: 0,
            last_sel: 0,
            tier: 1,
            item: None,
            guide: false,
            guide_t0: Instant::now(),
            gsel: 0,
            look_pick: None,
            slide: None,
            blade_x: Vec::new(),
            frame_x0: Glide::new(18.0, SLIDE),
            frame_end: Glide::new(1024.0, SLIDE),
            haze: Glide::new(1.0, 0.5),
            content_t0: None,
            page_t0: None,
            toast: None,
            tex: Tex::default(),
            sound: Sound::default(),
            started: false,
            n_last: 0,
            w_last: 0.0,
        }
    }

    fn save(&self) {
        if let Some(p) = &self.prefs_path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let text = format!(
                "skin={}\nsound={}\nvolume={}\ncalm={}\n",
                SKINS[self.prefs.skin],
                if self.prefs.sound { "on" } else { "off" },
                self.prefs.vol,
                if self.prefs.calm { "on" } else { "off" }
            );
            let _ = std::fs::write(p, text);
        }
    }

    fn play(&mut self, name: &'static str) {
        if self.prefs.sound {
            self.sound.play(name, self.prefs.vol);
        }
    }

    pub fn tier(&self) -> u8 {
        self.tier
    }
    pub fn blade(&self) -> usize {
        self.b
    }
    pub fn item(&self) -> Option<u32> {
        self.item
    }
    pub fn guide_open(&self) -> bool {
        self.guide
    }

    /// The app opened a page itself (a link, a button): show it on its blade.
    pub fn show_page(&mut self, blade: usize, item: u32, items_of_blade: &[Item]) {
        if self.tier == 2 && self.b == blade && self.item == Some(item) {
            return;
        }
        let was_home = self.tier == 1;
        self.slide = None;
        self.b = blade;
        self.item = Some(item);
        self.tier = 2;
        if let Some(i) = items_of_blade.iter().position(|x| x.id == item) {
            self.sel = i;
            self.last_sel = i;
        }
        if was_home {
            self.page_t0 = Some(Instant::now());
        }
    }

    /// Show a blade's menu.
    pub fn home(&mut self, blade: usize) {
        self.tier = 1;
        if blade != self.b {
            self.b = blade;
            self.sel = 0;
        }
    }

    pub fn toast(&mut self, title: &str, text: &str) {
        self.toast = Some((title.into(), text.into(), Instant::now()));
        self.play("notification");
    }

    fn calm(&self, info: &Info) -> bool {
        self.prefs.calm || info.app_calm
    }

    fn go(&mut self, i: usize, n: usize, w: f32, calm: bool, events: &mut Vec<Event>) {
        if i >= n || i == self.b || self.tier != 1 {
            return;
        }
        let old = self.b;
        let dir = if i > old { 1 } else { -1 };
        let xr = |j: usize| w - 87.0 - 35.0 * (n - 1 - j) as f32;
        let d = if dir > 0 { xr(i) - (18.0 + 35.0 * i as f32) } else { xr(old) - (18.0 + 35.0 * old as f32) };
        self.slide = if calm { None } else { Some(Slide { from_b: old, from_sel: self.sel, dir, d, t0: Instant::now() }) };
        self.b = i;
        self.sel = 0;
        self.last_sel = 0;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) as usize;
        self.play(["bladeslide_1", "bladeslide_2", "bladeslide_3", "bladeslide_4"][(nanos / 1000 + i) % 4]);
        events.push(Event::Home(i));
    }

    fn open(&mut self, blades: &[Blade], i: usize, events: &mut Vec<Event>) {
        let Some(it) = blades.get(self.b).and_then(|b| b.items.get(i)) else { return };
        self.sel = i;
        self.last_sel = i;
        events.push(Event::Open(self.b, it.id));
        let id = it.id;
        let items = blades[self.b].items.clone();
        if self.tier == 1 {
            self.play("tier2_enter");
        }
        self.show_page(self.b, id, &items);
    }

    fn back(&mut self, before_back: &mut dyn FnMut() -> bool, n: usize, w: f32, calm: bool, events: &mut Vec<Event>) {
        if self.tier == 2 {
            if before_back() {
                self.play("back");
                return;
            }
            self.tier = 1;
            self.play("tier2_exit");
            self.content_t0 = if calm { None } else { Some((Instant::now(), -60.0)) };
            events.push(Event::Home(self.b));
        } else if self.b > 0 {
            self.play("back");
            self.go(0, n, w, calm, events);
        }
    }

    fn toggle_guide(&mut self) {
        self.guide = !self.guide;
        self.guide_t0 = Instant::now();
        self.gsel = 0;
        self.look_pick = None;
        self.play(if self.guide { "guide_open" } else { "guide_close" });
    }

    fn toggle_sound(&mut self) {
        self.prefs.sound = !self.prefs.sound;
        self.save();
        if self.prefs.sound {
            self.play("select");
        }
    }

    fn guide_rows(&self, info: &Info) -> Vec<GuideRow> {
        let mut rows = Vec::new();
        if !info.looks.is_empty() {
            let cur = self.look_pick.unwrap_or(info.look).min(info.looks.len() - 1);
            rows.push(GuideRow { label: "Look".into(), value: Some(info.looks[cur].into()), check: None, kind: GuideKind::Look });
        }
        rows.push(GuideRow { label: "Skin".into(), value: Some(SKIN_LABELS[self.prefs.skin].into()), check: None, kind: GuideKind::Skin });
        rows.push(GuideRow { label: "Sounds".into(), value: None, check: Some(self.prefs.sound), kind: GuideKind::Sounds });
        rows.push(GuideRow { label: "Volume".into(), value: Some(format!("{}%", (self.prefs.vol * 100.0).round())), check: None, kind: GuideKind::Volume });
        rows.push(GuideRow { label: "Calmer motion".into(), value: None, check: Some(self.prefs.calm || info.app_calm), kind: GuideKind::Calm });
        rows.push(GuideRow {
            label: if info.checking { "Checking for updates…".into() } else { "Check for updates".into() },
            value: None,
            check: None,
            kind: GuideKind::Updates,
        });
        rows
    }

    fn guide_cycle(&mut self, info: &Info, kind: GuideKind, d: i32) {
        match kind {
            GuideKind::Look => {
                let n = info.looks.len() as i32;
                if n > 0 {
                    let cur = self.look_pick.unwrap_or(info.look) as i32;
                    self.look_pick = Some(((cur + d + n) % n) as usize);
                    self.play("cursor");
                }
            }
            GuideKind::Skin => {
                self.prefs.skin = ((self.prefs.skin as i32 + d + 3) % 3) as usize;
                self.save();
                self.play("cursor");
            }
            GuideKind::Volume => {
                let i = VOLS.iter().position(|v| (*v - self.prefs.vol).abs() < 0.01).unwrap_or(2) as i32;
                self.prefs.vol = VOLS[(i + d).clamp(0, 4) as usize];
                self.save();
                self.play("cursor");
            }
            _ => {}
        }
    }

    fn guide_run(&mut self, info: &Info, kind: GuideKind, events: &mut Vec<Event>) {
        match kind {
            GuideKind::Look => match self.look_pick {
                Some(p) if p != info.look => {
                    self.play("select");
                    events.push(Event::SetLook(p));
                }
                _ => self.play("back"),
            },
            GuideKind::Skin | GuideKind::Volume => self.guide_cycle(info, kind, 1),
            GuideKind::Sounds => self.toggle_sound(),
            GuideKind::Calm => {
                self.prefs.calm = !self.prefs.calm;
                self.save();
                self.play("select");
            }
            GuideKind::Updates => {
                if !info.checking {
                    self.play("select");
                    events.push(Event::CheckUpdates);
                }
            }
        }
    }

    fn blade_pos(&self, n: usize, w: f32) -> Vec<(f32, &'static str, char)> {
        let xr = |j: usize| w - 87.0 - 35.0 * (n - 1 - j) as f32;
        let depth = |d: usize, side: &str| -> &'static str {
            match (d, side) {
                (1, "left") => "blades-size3-left-nf",
                (2, "left") => "blades-size2-left-nf",
                (_, "left") => "blades-size1-left-nf",
                (1, _) => "blades-size3-right-nf",
                (2, _) => "blades-size2-right-nf",
                _ => "blades-size1-right-nf",
            }
        };
        (0..n)
            .map(|i| {
                if i == self.b {
                    (if self.tier == 1 { 18.0 + 35.0 * i as f32 } else { -40.0 }, "blades-size4-header", 'h')
                } else if i < self.b {
                    (if self.tier == 1 { 18.0 + 35.0 * i as f32 } else { -120.0 - 35.0 * (self.b - i) as f32 }, depth(self.b - i, "left"), 'l')
                } else {
                    (if self.tier == 1 { xr(i) } else { w + 40.0 + 35.0 * (i - self.b) as f32 }, depth(i - self.b, "right"), 'r')
                }
            })
            .collect()
    }

    /// Draws the look and the open page. `page` draws the app's page into the
    /// pane; `before_back` lets the app step back inside a page first.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        blades: &[Blade],
        info: &Info,
        before_back: &mut dyn FnMut() -> bool,
        page: &mut dyn FnMut(&mut egui::Ui),
    ) -> Vec<Event> {
        let mut events = Vec::new();
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let n = blades.len().max(1);
        // Scale to the window's height, but never so far that the narrowest
        // layout (560 wide) runs off the side; leftover height is split above and below.
        let s = (full.height() / 576.0).min(full.width() / 560.0).clamp(0.3, 3.0);
        let w = (full.width() / s).max(560.0);
        let oy = ((full.height() - 576.0 * s) / 2.0).max(0.0);
        let calm = self.calm(info);
        if self.b >= blades.len() {
            self.b = 0;
        }
        if !self.started {
            self.started = true;
            self.play("startup");
        }
        let n_items = blades.get(self.b).map(|b| b.items.len()).unwrap_or(0);
        self.sel = self.sel.min(n_items.saturating_sub(1));

        // Glides: the blades and the panel follow their targets.
        let resized = (w - self.w_last).abs() > 0.5 || n != self.n_last;
        self.w_last = w;
        self.n_last = n;
        if self.blade_x.len() != n {
            self.blade_x = vec![Glide::new(0.0, SLIDE); n];
        }
        let pos = self.blade_pos(n, w);
        let animate = !calm && !resized;
        for (i, p) in pos.iter().enumerate() {
            let placed = self.blade_x[i].to != 0.0;
            self.blade_x[i].set(p.0, animate && placed);
        }
        let (fx0, fend) = if self.tier == 1 {
            let end = if self.b + 1 < n { w - 87.0 - 35.0 * (n - 1 - (self.b + 1)) as f32 + 65.0 } else { w };
            (18.0 + 35.0 * self.b as f32, end)
        } else {
            (-40.0, w + 200.0)
        };
        // During a slide the panel jumps to its new place and slides as a whole.
        let frame_anim = animate && self.slide.is_none();
        self.frame_x0.set(fx0, frame_anim);
        self.frame_end.set(fend, frame_anim);
        let show_haze = self.tier == 1 && blades.get(self.b).and_then(|b| b.tint.haze()).is_some();
        self.haze.set(if show_haze { 1.0 } else { 0.0 }, animate);
        if let Some(sl) = &self.slide {
            if sl.t0.elapsed().as_secs_f32() > SLIDE + 0.02 {
                self.slide = None;
            }
        }

        let to_screen = |x: f32, y: f32| full.min + vec2(x * s, oy + y * s);
        let painter = ui.painter().with_clip_rect(full);
        painter.rect_filled(full, 0.0, Color32::BLACK);
        let t = ctx.input(|i| i.time) as f32;

        // ---- panels: the one being covered or carried off, then the open one.
        let slide_k = self.slide.as_ref().map(|sl| ease(sl.t0.elapsed().as_secs_f32() / SLIDE));
        let blade_tint = |b: usize| blades.get(b).map(|x| x.tint).unwrap_or(Tint::Grey);
        let mut ghost_shift = 0.0;
        let mut main_shift = 0.0;
        if let (Some(sl), Some(k)) = (&self.slide, slide_k) {
            if sl.dir > 0 {
                main_shift = sl.d * (1.0 - k);
            } else {
                ghost_shift = sl.d * k;
            }
        }
        let skin = self.prefs.skin;
        let swirl_f = if calm { 0.0 } else { (t / SWIRL_SECS).fract() * SWIRL_FRAMES as f32 };
        let ghost = self.slide.as_ref().map(|sl| (sl.from_b, sl.from_sel, sl.dir));
        // Forward: the old panel stays underneath; backward: it slides off on top.
        if let Some((gb, _, dir)) = ghost {
            if dir > 0 {
                let gx0 = 18.0 + 35.0 * gb as f32;
                self.panel(&ctx, &painter, &to_screen, s, gx0, w + 900.0, w + 900.0, blade_tint(gb), skin, swirl_f, ghost_shift, &EDGE);
                self.ghost_menu(&ctx, &painter, &to_screen, s, blades, gb, ghost.unwrap().1, gx0, ghost_shift, w);
            }
        }
        let x0 = self.frame_x0.get();
        let end = self.frame_end.get();
        let wash_end = if self.tier == 1 { end } else { w + 90.0 };
        self.panel(&ctx, &painter, &to_screen, s, x0, end, wash_end, blade_tint(self.b), skin, swirl_f, main_shift, &EDGE);

        // ---- content (menu or page title) with its slide-in
        let mut c_shift = 0.0;
        let mut c_alpha = 1.0;
        if let (Some(sl), Some(k)) = (&self.slide, slide_k) {
            c_shift = if sl.dir > 0 { (sl.d + 60.0) * (1.0 - k) } else { -60.0 * (1.0 - k) };
            c_alpha = k;
        } else if let Some((t0, from)) = self.content_t0 {
            let k = ease(t0.elapsed().as_secs_f32() / SLIDE);
            if k >= 1.0 {
                self.content_t0 = None;
            } else {
                c_shift = from * (1.0 - k);
                c_alpha = k;
            }
        }
        let clock = chrono::Local::now().format("%-I:%M %p").to_string();
        if self.tier == 1 {
            self.home_content(&ctx, ui, &painter, &to_screen, s, blades, info, x0, if self.b + 1 < n { w - 87.0 - 35.0 * (n - 1 - (self.b + 1)) as f32 + 65.0 } else { w }, c_shift, c_alpha, &clock, &mut events, n, w, calm);
        } else {
            let bl = &blades[self.b.min(blades.len() - 1)];
            let it = bl.items.iter().find(|x| Some(x.id) == self.item);
            let title = format!("{}{}", title_case(&bl.key), it.map(|x| format!(" - {}", x.label)).unwrap_or_default());
            text(&painter, &to_screen, s, &title, 102.0, 38.0, 23.0, Color32::WHITE, Shadow::Black, Align2::LEFT_TOP, 1.0);
            text(&painter, &to_screen, s, &clock, w - 68.0, 44.0, 17.0, Color32::WHITE, Shadow::Black, Align2::RIGHT_TOP, 1.0);
        }

        // ---- the backward ghost slides off over the new panel
        if let Some((gb, gsel, dir)) = ghost {
            if dir < 0 {
                let gx0 = 18.0 + 35.0 * gb as f32;
                self.panel(&ctx, &painter, &to_screen, s, gx0, w + 900.0, w + 900.0, blade_tint(gb), skin, swirl_f, ghost_shift, &EDGE_R);
                self.ghost_menu(&ctx, &painter, &to_screen, s, blades, gb, gsel, gx0, ghost_shift, w);
            }
        }

        // ---- runners and blades
        let skin_name = SKINS[skin];
        self.spr(&ctx, &painter, &to_screen, s, &format!("{skin_name}:blades-runner-left"), if self.tier == 1 { -61.0 } else { -119.0 }, 0.0, 128.0, 576.0, 1.0);
        self.spr(&ctx, &painter, &to_screen, s, &format!("{skin_name}:blades-runner-right"), if self.tier == 1 { w - 55.0 } else { w + 40.0 }, 0.0, 128.0, 576.0, 1.0);
        // Left blades first (lowest first), then the right ones from the far end, then the open one.
        let mut order: Vec<usize> = (0..n).filter(|i| pos[*i].2 == 'l').collect();
        order.extend((0..n).filter(|i| pos[*i].2 == 'r').rev());
        order.extend((0..n).filter(|i| pos[*i].2 == 'h'));
        for i in order {
            let (_, tex, side) = pos[i];
            let x = self.blade_x[i].get();
            self.spr(&ctx, &painter, &to_screen, s, &format!("{skin_name}:{tex}"), x, 0.0, 80.0, 576.0, 1.0);
            let ly = match tex {
                "blades-size4-header" => 155.0,
                "blades-size3-left-nf" | "blades-size3-right-nf" => 163.0,
                "blades-size2-left-nf" | "blades-size2-right-nf" => 170.0,
                _ => 177.0,
            };
            let lx = if side == 'r' { x + 39.5 } else { x + 54.5 };
            let col = if side == 'h' { Color32::BLACK } else { Color32::from_rgb(0x35, 0x35, 0x35) };
            let label = blades.get(i).map(|b| b.key.as_str()).unwrap_or("");
            let galley = painter.layout_no_wrap(label.into(), FontId::new(17.0 * s, FontFamily::Name(FONT.into())), col);
            painter.add(Shape::Text(egui::epaint::TextShape::new(to_screen(lx, ly), galley, col).with_angle(std::f32::consts::FRAC_PI_2)));
            if self.tier == 1 && side != 'h' && !self.guide {
                let hw = if side == 'l' { 35.0 } else { 40.0 };
                let r = Rect::from_min_max(to_screen(x + 20.0, 0.0), to_screen(x + 20.0 + hw, 576.0));
                let resp = ui.interact(r, Id::new(("mc360-blade", i)), Sense::click()).on_hover_text(label);
                if resp.clicked() {
                    self.go(i, n, w, calm, &mut events);
                }
            }
        }
        if self.tier == 2 && !self.guide {
            let r = Rect::from_min_max(to_screen(0.0, 0.0), to_screen(30.0, 576.0));
            if ui.interact(r, Id::new("mc360-back-strip"), Sense::click()).clicked() {
                self.back(before_back, n, w, calm, &mut events);
            }
        }
        // Hazes under the blades' feet.
        let hk = self.haze.get();
        if hk > 0.01 {
            let hz = blades.get(self.b).and_then(|b| b.tint.haze()).unwrap_or("blue");
            let end_h = if self.b + 1 < n { w - 87.0 - 35.0 * (n - 1 - (self.b + 1)) as f32 + 65.0 } else { w };
            self.spr(&ctx, &painter, &to_screen, s, &format!("haze-{hz}-left"), x0 - 88.0, 350.0, 150.0, 228.0, hk);
            self.spr(&ctx, &painter, &to_screen, s, &format!("haze-{hz}-right"), end_h - 41.0, 350.0, 150.0, 228.0, hk);
        }

        // ---- buttons along the bottom
        let (lx, bend) = if self.tier == 1 { (18.0 + 35.0 * self.b as f32 + 107.0, if self.b + 1 < n { w - 87.0 - 35.0 * (n - 1 - (self.b + 1)) as f32 + 65.0 } else { w }) } else { (125.0, w) };
        if !self.guide {
            self.buttons(&ctx, ui, &painter, &to_screen, s, lx, bend, blades, before_back, &mut events, n, w, calm);
        }

        // ---- the page in its pane
        if self.tier == 2 {
            let pane = Rect::from_min_max(to_screen(70.0, 70.0), to_screen(w - 40.0, 505.0));
            let (mut shift, mut alpha) = (0.0, 1.0);
            if let Some(t0) = self.page_t0 {
                let k = ease(t0.elapsed().as_secs_f32() / SLIDE);
                if k >= 1.0 || calm {
                    self.page_t0 = None;
                } else {
                    shift = 60.0 * (1.0 - k) * s;
                    alpha = k;
                }
            }
            let pane = pane.translate(vec2(shift, 0.0));
            painter.rect(pane, 10.0 * s, Color32::from_rgba_unmultiplied(8, 12, 22, (128.0 * alpha) as u8), Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, (77.0 * alpha) as u8)), egui::StrokeKind::Inside);
            let inner = pane.shrink2(vec2(14.0, 10.0));
            let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("mc360-page", self.b, self.item)).max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(pane.shrink(2.0).intersect(full));
            child.multiply_opacity(alpha);
            page(&mut child);
        }

        // ---- guide
        if self.guide {
            self.guide_ui(&ctx, ui, &to_screen, s, w, full, info, &mut events);
        }
        // ---- toast
        if let Some((title, msg, t0)) = &self.toast {
            let el = t0.elapsed().as_secs_f32();
            if el > 3.6 {
                self.toast = None;
            } else {
                let (title, msg) = (title.clone(), msg.clone());
                let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("mc360-toast")));
                let k = if calm { 1.0 } else { (el / 0.2).min(1.0) };
                let tx = w / 2.0 - 107.0;
                self.spr_on(&ctx, &p, &to_screen, s, "notify-panel", tx, 450.0, 215.0 * k, 64.0, 1.0);
                if k >= 1.0 {
                    if let Some(tex) = self.tex.ring(&ctx) {
                        p.image(tex, Rect::from_min_size(to_screen(tx + 8.0, 456.0), vec2(50.0 * s, 50.0 * s)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    text(&p, &to_screen, s, &title, tx + 65.0, 461.0, 15.0, Color32::WHITE, Shadow::Hard, Align2::LEFT_TOP, 1.0);
                    let msg = ellipsize(&p, &msg, 13.0 * s, 145.0 * s);
                    text(&p, &to_screen, s, &msg, tx + 65.0, 484.0, 13.0, Color32::WHITE, Shadow::Hard, Align2::LEFT_TOP, 0.9);
                }
            }
        }

        // ---- keys
        self.keys(&ctx, ui, blades, info, before_back, &mut events, n, w, calm);

        let moving = self.slide.is_some()
            || self.blade_x.iter().any(|g| g.moving())
            || self.frame_x0.moving()
            || self.frame_end.moving()
            || self.haze.moving()
            || self.content_t0.is_some()
            || self.page_t0.is_some()
            || self.toast.is_some()
            || (self.guide && self.guide_t0.elapsed().as_secs_f32() < 0.3);
        if moving {
            ctx.request_repaint();
        } else if !calm {
            // The swirl keeps turning gently.
            ctx.request_repaint_after(std::time::Duration::from_millis(66));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(15));
        }
        events
    }

    fn keys(&mut self, ctx: &egui::Context, ui: &egui::Ui, blades: &[Blade], info: &Info, before_back: &mut dyn FnMut() -> bool, events: &mut Vec<Event>, n: usize, w: f32, calm: bool) {
        let typing = ctx.egui_wants_keyboard_input();
        let (nl, nr, nu, nd, enter, esc, g, bs, mback) = ui.input(|i| {
            (
                i.num_presses(Key::ArrowLeft),
                i.num_presses(Key::ArrowRight),
                i.num_presses(Key::ArrowUp),
                i.num_presses(Key::ArrowDown),
                i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) && !i.modifiers.any(),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::G) && !i.modifiers.any(),
                i.key_pressed(Key::Backspace),
                i.pointer.button_pressed(egui::PointerButton::Extra1),
            )
        });
        let (l, r, u, d) = (nl > 0, nr > 0, nu > 0, nd > 0);
        if self.guide {
            let rows = self.guide_rows(info);
            if esc || (bs && !typing) || (g && !typing) || mback {
                self.toggle_guide();
            } else if u || d {
                let j = if u { self.gsel.saturating_sub(nu) } else { (self.gsel + nd).min(rows.len() - 1) };
                if j != self.gsel {
                    self.gsel = j;
                    self.play("cursor");
                }
            } else if l || r {
                if let Some(row) = rows.get(self.gsel) {
                    for _ in 0..nl.max(nr) {
                        self.guide_cycle(info, row.kind, if l { -1 } else { 1 });
                    }
                }
            } else if enter {
                if let Some(row) = rows.get(self.gsel) {
                    self.guide_run(info, row.kind, events);
                }
            }
            return;
        }
        if mback && !info.popup_open {
            if self.tier == 2 {
                self.back(before_back, n, w, calm, events);
            } else if self.b > 0 {
                self.go(self.b - 1, n, w, calm, events);
            }
            return;
        }
        if info.popup_open {
            return;
        }
        // Esc in a text box on a page leaves the box first; the next Esc goes back.
        if typing {
            if esc && self.tier == 2 {
                ctx.memory_mut(|m| m.stop_text_input());
            }
            return;
        }
        if g {
            self.toggle_guide();
            return;
        }
        if self.tier == 1 {
            if l && self.b > 0 {
                self.go(self.b.saturating_sub(nl), n, w, calm, events);
            } else if r {
                self.go((self.b + nr).min(n - 1), n, w, calm, events);
            } else if u || d {
                let m = blades.get(self.b).map(|b| b.items.len()).unwrap_or(0);
                let j = if u { self.sel.saturating_sub(nu) } else { (self.sel + nd).min(m.saturating_sub(1)) };
                if j != self.sel {
                    self.sel = j;
                    self.last_sel = j;
                    self.play("cursor");
                }
            } else if enter {
                self.open(blades, self.sel, events);
            } else if esc || bs {
                self.back(before_back, n, w, calm, events);
            }
        } else if esc {
            self.back(before_back, n, w, calm, events);
        }
    }

    // ---- drawing helpers

    fn spr(&mut self, ctx: &egui::Context, p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, s: f32, name: &str, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        self.spr_on(ctx, p, to_screen, s, name, x, y, w, h, alpha);
    }

    fn spr_on(&mut self, ctx: &egui::Context, p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, _s: f32, name: &str, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        let Some((a, sx, sy, sw, sh)) = sprite(name) else { return };
        let Some(tex) = self.tex.atlas(ctx, a) else { return };
        let (aw, ah) = ATLAS_SIZE[a as usize];
        let (aw, ah) = (aw as f32, ah as f32);
        let uv = Rect::from_min_max(pos2(sx / aw, sy / ah), pos2((sx + sw) / aw, (sy + sh) / ah));
        let rect = Rect::from_min_max(to_screen(x, y), to_screen(x + w, y + h));
        p.image(tex, rect, uv, Color32::WHITE.gamma_multiply(alpha.clamp(0.0, 1.0)));
    }

    /// The panel of a blade: swirl, tint and the white washes, cut along the
    /// open blade's curve on the left.
    fn panel(&mut self, ctx: &egui::Context, p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, _s: f32, x0: f32, end: f32, wash_end: f32, tint: Tint, skin: usize, swirl_f: f32, shift: f32, edge: &[(f32, f32); 17]) {
        let left = |y: f32| x0 + edge_at(edge, y) * 0.625 + 2.0 + shift;
        let right = end + shift;
        if right <= left(288.0) {
            return;
        }
        // Swirl frames, crossfaded (cover-fit into the panel, like the web page's video).
        let pw = end - x0;
        let size = pw.max(576.0);
        let (ux0, uy0) = (x0 + shift + (pw - size) / 2.0, (576.0 - size) / 2.0);
        let f0 = swirl_f.floor() as usize % SWIRL_FRAMES;
        let f1 = (f0 + 1) % SWIRL_FRAMES;
        let mix = swirl_f.fract();
        for (f, a) in [(f0, 1.0), (f1, mix)] {
            if a < 0.01 {
                continue;
            }
            let Some(tex) = self.tex.swirl(ctx, skin, f) else { continue };
            let mut m = Mesh::with_texture(tex);
            let col = Color32::WHITE.gamma_multiply(a);
            rows(&mut m, &left, &|_| right, 0.0, 576.0, 8.0, to_screen, &|x, y| (pos2((x - ux0) / size, (y - uy0) / size), col));
            p.add(m);
        }
        let skin_name = SKINS[skin];
        if let Some((_, stops)) = grad(&format!("{skin_name}:tint-{}", tint.key())) {
            let mut m = Mesh::default();
            rows(&mut m, &left, &|_| right, 0.0, 576.0, 8.0, to_screen, &|_, y| (Pos2::ZERO, grad_at(stops, y / 576.0)));
            p.add(m);
        }
        // The white washes fade in over 46 px at both ends.
        let we = wash_end + shift;
        let fade = |x: f32, a: f32, b: f32| ((x - a) / 46.0).clamp(0.0, 1.0).min(((b - x) / 46.0).clamp(0.0, 1.0));
        let wash = |m: &mut Mesh, name: &str, ya: f32, yb: f32, xa: f32, xb: f32| {
            if let Some((_, stops)) = grad(name) {
                let (xa, xb) = (xa.max(left(ya.max(1.0))), xb);
                if xb <= xa {
                    return;
                }
                let xs = [xa, (xa + 46.0).min(xb), (xb - 46.0).max(xa), xb];
                let mut y = ya;
                while y < yb {
                    let y2 = (y + 8.0).min(yb);
                    let base = m.vertices.len() as u32;
                    for yy in [y, y2] {
                        let c = grad_at(stops, (yy - ya) / (yb - ya));
                        for x in xs {
                            m.colored_vertex(to_screen(x.max(left(yy)), yy), c.gamma_multiply(fade(x, xa, xb)));
                        }
                    }
                    for k in 0..3 {
                        m.add_triangle(base + k, base + k + 1, base + 4 + k);
                        m.add_triangle(base + k + 1, base + 5 + k, base + 4 + k);
                    }
                    y = y2;
                }
            }
        };
        let mut m = Mesh::default();
        wash(&mut m, &format!("{skin_name}:wash-mid"), 0.0, 576.0, x0 + 30.0 + shift, we - 16.0);
        wash(&mut m, "bkgd-whitewash-glass-top-middle", 0.0, 64.0, x0 + 40.0 + shift, we - 20.0);
        wash(&mut m, "bkgd-whitewash-glass-bottom-middle", 512.0, 576.0, x0 + 40.0 + shift, we - 20.0);
        p.add(m);
    }

    /// The menu the outgoing panel carries while it slides.
    fn ghost_menu(&mut self, ctx: &egui::Context, p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, s: f32, blades: &[Blade], gb: usize, gsel: usize, xh: f32, shift: f32, w: f32) {
        let Some(bl) = blades.get(gb) else { return };
        let x = xh + shift;
        text(p, to_screen, s, &title_case(&bl.key), x + 84.0, 38.0, 23.0, Color32::WHITE, Shadow::Black, Align2::LEFT_TOP, 1.0);
        let bx = x + 92.0;
        let start = gsel.saturating_sub(9).min(bl.items.len().saturating_sub(10));
        let shown: Vec<_> = bl.items.iter().skip(start).take(10).collect();
        if !shown.is_empty() {
            let fr = gsel - start;
            self.spr(ctx, p, to_screen, s, &format!("hl{}", fr + 1), bx - 35.0, 92.0, 255.0, 367.0, 1.0);
        }
        for (k, it) in shown.iter().enumerate() {
            let y = 96.0 + 35.0 * k as f32;
            self.spr(ctx, p, to_screen, s, icon_sprite(it.icon), bx, y, 30.0, 30.0, 1.0);
            text(p, to_screen, s, &it.label, bx + 45.0, y + 7.0, 17.0, Color32::BLACK, Shadow::Grey, Align2::LEFT_TOP, 1.0);
        }
        let _ = w;
    }

    fn home_content(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, s: f32, blades: &[Blade], info: &Info, xh: f32, end: f32, shift: f32, alpha: f32, clock: &str, events: &mut Vec<Event>, n: usize, w: f32, calm: bool) {
        let Some(bl) = blades.get(self.b) else { return };
        let xh = 18.0 + 35.0 * self.b as f32 + (xh - (18.0 + 35.0 * self.b as f32));
        let x = xh + shift;
        text(p, to_screen, s, &title_case(&bl.key), x + 84.0, 38.0, 23.0, Color32::WHITE, Shadow::Black, Align2::LEFT_TOP, alpha);
        text(p, to_screen, s, clock, end - 68.0 + shift, 44.0, 17.0, Color32::WHITE, Shadow::Black, Align2::RIGHT_TOP, alpha);
        let bx = x + 92.0;
        let start = self.sel.saturating_sub(9).min(bl.items.len().saturating_sub(10));
        let shown: Vec<(usize, Item)> = bl.items.iter().cloned().enumerate().skip(start).take(10).collect();
        if !shown.is_empty() {
            let fr = self.sel - start;
            self.spr(ctx, p, to_screen, s, &format!("hl{}", fr + 1), bx - 35.0, 92.0, 255.0, 367.0, alpha);
            // A soft shine sweeps over the highlighted entry now and then.
            if !calm && !self.guide && shift == 0.0 {
                let t = (ctx.input(|i| i.time) as f32 % 3.2) / 3.2;
                if t < 0.5 {
                    let k = t / 0.5;
                    let y = 96.0 + 35.0 * fr as f32 - 5.0;
                    let cx = bx + 5.0 + 220.0 * k;
                    let mut m = Mesh::default();
                    let a = (1.0 - (2.0 * k - 1.0).abs()) * 0.35;
                    for (xx, al) in [(cx - 40.0, 0.0), (cx, a), (cx + 40.0, 0.0)] {
                        let c = Color32::WHITE.gamma_multiply(al);
                        m.colored_vertex(to_screen(xx.clamp(bx + 5.0, bx + 225.0), y), c);
                        m.colored_vertex(to_screen(xx.clamp(bx + 5.0, bx + 225.0), y + 38.0), c);
                    }
                    for k in 0..2u32 {
                        m.add_triangle(2 * k, 2 * k + 1, 2 * k + 2);
                        m.add_triangle(2 * k + 1, 2 * k + 3, 2 * k + 2);
                    }
                    p.add(m);
                }
            }
        }
        if let Some((_, stops)) = grad("divider") {
            for (k, _) in shown.iter().enumerate() {
                let y = 96.0 + 35.0 * k as f32 - 4.0;
                p.rect_filled(Rect::from_min_max(to_screen(bx - 10.0, y), to_screen(bx + 210.0, y + 1.0)), 0.0, grad_at(stops, 0.0).gamma_multiply(alpha));
            }
        }
        for (k, (idx, it)) in shown.iter().enumerate() {
            let y = 96.0 + 35.0 * k as f32;
            let on = *idx == self.sel;
            let dx = if on { -3.0 } else { 0.0 };
            self.spr(ctx, p, to_screen, s, icon_sprite(it.icon), bx + dx, y, 30.0, 30.0, alpha);
            let label = ellipsize(p, &it.label, 17.0 * s, 170.0 * s);
            text(p, to_screen, s, &label, bx + 45.0 + dx, y + 7.0, 17.0, Color32::BLACK, Shadow::Grey, Align2::LEFT_TOP, alpha);
            if !self.guide && self.slide.is_none() {
                let r = Rect::from_min_max(to_screen(bx - 20.0, y - 3.0), to_screen(bx + 215.0, y + 30.0));
                let resp = ui.interact(r, Id::new(("mc360-item", self.b, *idx)), Sense::click());
                if resp.hovered() && self.sel != *idx && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                    self.sel = *idx;
                    self.last_sel = *idx;
                    self.play("cursor");
                }
                if resp.clicked() {
                    self.open(blades, *idx, events);
                }
            }
        }
        if bl.items.len() > 10 {
            text(p, to_screen, s, &format!("{}–{} of {}", start + 1, start + shown.len(), bl.items.len()), bx + 120.0, 448.0, 13.0, Color32::BLACK, Shadow::Grey, Align2::LEFT_TOP, alpha);
        }
        // Status card and the entry's description, when there's room.
        let cx = end - 300.0 + shift;
        if cx > bx + 230.0 {
            let cy = 74.0;
            self.spr(ctx, p, to_screen, s, "gamecard_home", cx, cy, 256.0, 128.0, alpha);
            self.spr(ctx, p, to_screen, s, "home_logoshadow", cx + 12.0, cy + 124.0, 232.0, 16.0, alpha);
            text(p, to_screen, s, info.app, cx + 14.0, cy + 15.0, 15.0, Color32::from_rgb(17, 17, 17), Shadow::None, Align2::LEFT_TOP, alpha);
            self.spr(ctx, p, to_screen, s, icon_sprite(info.icon), cx + 14.0, cy + 48.0, 52.0, 52.0, alpha);
            for (i, l) in info.card.iter().take(3).enumerate() {
                let l = ellipsize(p, l, 12.0 * s, 160.0 * s);
                text(p, to_screen, s, &l, cx + 76.0, cy + 48.0 + 19.0 * i as f32, 12.0, Color32::WHITE, Shadow::Black, Align2::LEFT_TOP, alpha);
            }
            if let Some(it) = bl.items.get(self.sel) {
                if !it.hint.is_empty() {
                    let wrap = (cx + 240.0 - (bx + 245.0)).min(220.0) * s;
                    let galley = p.layout(it.hint.clone(), FontId::new(17.0 * s, FontFamily::Name(FONT.into())), Color32::BLACK.gamma_multiply(alpha), wrap);
                    let pos = to_screen(bx + 245.0, 222.0);
                    p.galley(pos + vec2(1.0, 1.0), galley.clone(), Color32::from_rgba_unmultiplied(218, 218, 218, (160.0 * alpha) as u8));
                    p.galley(pos, galley, Color32::BLACK);
                }
            }
        }
        let _ = (n, w, calm);
    }

    fn buttons(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, s: f32, lx: f32, end: f32, blades: &[Blade], before_back: &mut dyn FnMut() -> bool, events: &mut Vec<Event>, n: usize, w: f32, calm: bool) {
        self.spr(ctx, p, to_screen, s, "button-y", lx, 505.0, 21.0, 21.0, 1.0);
        self.spr(ctx, p, to_screen, s, "button-x", lx - 13.0, 525.0, 21.0, 21.0, 1.0);
        text(p, to_screen, s, "guide", lx + 26.0, 508.0, 15.0, Color32::WHITE, Shadow::Black, Align2::LEFT_TOP, 1.0);
        let snd = if self.prefs.sound { "sound on" } else { "sound off" };
        text(p, to_screen, s, snd, lx + 13.0, 528.0, 15.0, Color32::WHITE, Shadow::Black, Align2::LEFT_TOP, 1.0);
        self.spr(ctx, p, to_screen, s, "button-b", end - 113.0, 505.0, 21.0, 21.0, 1.0);
        text(p, to_screen, s, "back", end - 118.0, 508.0, 15.0, Color32::WHITE, Shadow::Black, Align2::RIGHT_TOP, 1.0);
        if self.tier == 1 {
            self.spr(ctx, p, to_screen, s, "button-a", end - 100.0, 525.0, 21.0, 21.0, 1.0);
            text(p, to_screen, s, "select", end - 110.0, 528.0, 15.0, Color32::WHITE, Shadow::Black, Align2::RIGHT_TOP, 1.0);
        }
        let hit = |ui: &mut egui::Ui, id: &str, x0: f32, y0: f32, x1: f32, y1: f32| ui.interact(Rect::from_min_max(to_screen(x0, y0), to_screen(x1, y1)), Id::new(("mc360-btn", id.to_string())), Sense::click()).clicked();
        if hit(ui, "guide", lx - 4.0, 501.0, lx + 106.0, 525.0) {
            self.toggle_guide();
        }
        if hit(ui, "sound", lx - 17.0, 523.0, lx + 103.0, 547.0) {
            self.toggle_sound();
        }
        if hit(ui, "back", end - 210.0, 501.0, end - 90.0, 525.0) {
            self.back(before_back, n, w, calm, events);
        }
        if self.tier == 1 && hit(ui, "select", end - 200.0, 523.0, end - 75.0, 547.0) {
            self.open(blades, self.sel, events);
        }
    }

    fn guide_ui(&mut self, ctx: &egui::Context, _ui: &mut egui::Ui, to_screen: &dyn Fn(f32, f32) -> Pos2, s: f32, w: f32, full: Rect, info: &Info, events: &mut Vec<Event>) {
        let calm = self.calm(info);
        let el = self.guide_t0.elapsed().as_secs_f32();
        let k = if calm { 1.0 } else { ease(el / 0.25) };
        let slide = -390.0 * (1.0 - k);
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("mc360-guide")));
        let p = p.with_clip_rect(full);
        let scrim_a = if calm { 1.0 } else { (el / 0.3).min(1.0) };
        p.rect_filled(full, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, (204.0 * scrim_a) as u8));
        // Clicks outside the guide close it; clicks inside belong to it.
        let area = egui::Area::new(Id::new("mc360-guide-area")).order(egui::Order::Foreground).fixed_pos(full.min).interactable(true);
        let mut run: Option<(usize, i32)> = None;
        let mut close = false;
        let grows = self.guide_rows(info);
        area.show(ctx, |gui| {
            let bg = gui.interact(full, Id::new("mc360-guide-scrim"), Sense::click());
            if bg.clicked() {
                let pos = bg.interact_pointer_pos().unwrap_or(Pos2::ZERO);
                if pos.x > to_screen(390.0 + slide, 0.0).x {
                    close = true;
                }
            }
            let ts = |x: f32, y: f32| to_screen(x + slide, y);
            let bx = 296.0;
            // Grey pane, tucked behind the blade's curve.
            if let Some((a, sx, sy, sw, sh)) = sprite("background-grey") {
                if let Some(tex) = self.tex.atlas(ctx, a) {
                    let (aw, ah) = ATLAS_SIZE[a as usize];
                    let mut m = Mesh::with_texture(tex);
                    let right = |y: f32| bx + edge_at(&EDGE, y) * 0.625 + 22.0;
                    rows(&mut m, &|_| -10.0, &right, 0.0, 576.0, 8.0, &ts, &|x, y| {
                        (pos2((sx + sw * ((x + 10.0_f32) / (bx + 90.0)).clamp(0.0, 1.0)) / aw as f32, (sy + sh * (y / 576.0)) / ah as f32), Color32::WHITE)
                    });
                    p.add(m);
                    // Soft shading at the top and bottom, and toward the blade.
                    let mut sh_m = Mesh::default();
                    let stops: [(f32, f32); 9] = [(0.0, -0.30), (0.05, -0.16), (0.11, -0.05), (0.17, 0.10), (0.5, 0.05), (0.8, 0.08), (0.86, -0.05), (0.94, -0.16), (1.0, -0.30)];
                    rows(&mut sh_m, &|_| -10.0, &right, 0.0, 576.0, 6.0, &ts, &|_, y| {
                        let u = y / 576.0;
                        let mut v = 0.0;
                        for wnd in stops.windows(2) {
                            if u <= wnd[1].0 {
                                v = wnd[0].1 + (wnd[1].1 - wnd[0].1) * ((u - wnd[0].0) / (wnd[1].0 - wnd[0].0));
                                break;
                            }
                        }
                        let c = if v < 0.0 { Color32::from_rgba_unmultiplied(0, 0, 0, (-v * 255.0) as u8) } else { Color32::from_rgba_unmultiplied(255, 255, 255, (v * 255.0) as u8) };
                        (Pos2::ZERO, c)
                    });
                    p.add(sh_m);
                }
            }
            let skin_name = SKINS[self.prefs.skin];
            self.spr_on(ctx, &p, &ts, s, &format!("{skin_name}:blades-size4-header"), bx, 0.0, 80.0, 576.0, 1.0);
            self.spr_on(ctx, &p, &ts, s, icon_sprite(info.icon), 40.0, 26.0, 44.0, 44.0, 1.0);
            text(&p, &ts, s, info.app, 94.0, 30.0, 23.0, Color32::BLACK, Shadow::Grey, Align2::LEFT_TOP, 1.0);
            text(&p, &ts, s, &format!("version {} · {}", info.ver, info.plat), 95.0, 57.0, 13.0, Color32::BLACK, Shadow::None, Align2::LEFT_TOP, 0.75);
            for (kk, r) in grows.iter().enumerate() {
                let y = 88.0 + 36.0 * kk as f32;
                if let Some((_, stops)) = grad("divider") {
                    p.rect_filled(Rect::from_min_max(ts(40.0, y + 10.0), ts(270.0, y + 11.0)), 0.0, grad_at(stops, 0.0));
                }
                if self.gsel == kk {
                    self.spr_on(ctx, &p, &ts, s, "guide-button-focus", 18.0, y, 280.0, 59.0, 1.0);
                }
                text(&p, &ts, s, &r.label, 46.0, y + 21.0, 17.0, Color32::BLACK, Shadow::Grey, Align2::LEFT_TOP, 1.0);
                let row_rect = Rect::from_min_max(ts(26.0, y + 12.0), ts(288.0, y + 46.0));
                let resp = gui.interact(row_rect, Id::new(("mc360-guide-row", kk)), Sense::click());
                if resp.hovered() && self.gsel != kk && gui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                    self.gsel = kk;
                    self.play("cursor");
                }
                if let Some(v) = &r.value {
                    self.spr_on(ctx, &p, &ts, s, "selectarrow-left", 168.0, y + 20.0, 9.0, 18.0, 1.0);
                    let v = ellipsize(&p, v, 15.0 * s, 68.0 * s);
                    text(&p, &ts, s, &v, 215.0, y + 22.0, 15.0, Color32::BLACK, Shadow::Grey, Align2::CENTER_TOP, 1.0);
                    self.spr_on(ctx, &p, &ts, s, "selectarrow-right", 254.0, y + 20.0, 9.0, 18.0, 1.0);
                    if gui.interact(Rect::from_min_max(ts(160.0, y + 12.0), ts(184.0, y + 46.0)), Id::new(("mc360-guide-prev", kk)), Sense::click()).clicked() {
                        run = Some((kk, -1));
                    } else if gui.interact(Rect::from_min_max(ts(248.0, y + 12.0), ts(272.0, y + 46.0)), Id::new(("mc360-guide-next", kk)), Sense::click()).clicked() {
                        run = Some((kk, 1));
                    } else if resp.clicked() {
                        run = Some((kk, 0));
                    }
                } else if resp.clicked() {
                    run = Some((kk, 0));
                }
                if let Some(c) = r.check {
                    self.spr_on(ctx, &p, &ts, s, if c { "check-box" } else { "check-boxnf" }, 244.0, y + 18.0, 22.0, 22.0, 1.0);
                }
            }
            self.spr_on(ctx, &p, &ts, s, "button-y", 40.0, 524.0, 21.0, 21.0, 1.0);
            text(&p, &ts, s, "close", 66.0, 528.0, 15.0, Color32::BLACK, Shadow::Grey, Align2::LEFT_TOP, 1.0);
            self.spr_on(ctx, &p, &ts, s, "button-back", 252.0, 500.0, 21.0, 21.0, 1.0);
            text(&p, &ts, s, "back", 246.0, 504.0, 15.0, Color32::BLACK, Shadow::Grey, Align2::RIGHT_TOP, 1.0);
            self.spr_on(ctx, &p, &ts, s, "button-a", 252.0, 524.0, 21.0, 21.0, 1.0);
            text(&p, &ts, s, "select", 246.0, 528.0, 15.0, Color32::BLACK, Shadow::Grey, Align2::RIGHT_TOP, 1.0);
            if gui.interact(Rect::from_min_max(ts(34.0, 519.0), ts(124.0, 547.0)), Id::new("mc360-guide-close"), Sense::click()).clicked()
                || gui.interact(Rect::from_min_max(ts(170.0, 496.0), ts(280.0, 520.0)), Id::new("mc360-guide-back"), Sense::click()).clicked()
            {
                close = true;
            }
            if gui.interact(Rect::from_min_max(ts(170.0, 520.0), ts(280.0, 548.0)), Id::new("mc360-guide-select"), Sense::click()).clicked() {
                run = Some((self.gsel, 0));
            }
        });
        if close {
            self.toggle_guide();
        } else if let Some((kk, d)) = run {
            self.gsel = kk;
            if let Some(r) = grows.get(kk) {
                if d != 0 {
                    self.guide_cycle(info, r.kind, d);
                } else {
                    self.guide_run(info, r.kind, events);
                }
            }
        }
        let _ = w;
    }
}

// ---------- shared drawing ----------

#[derive(Clone, Copy)]
enum Shadow {
    None,
    Black,
    Grey,
    Hard,
}

fn text(p: &Painter, to_screen: &dyn Fn(f32, f32) -> Pos2, s: f32, t: &str, x: f32, y: f32, size: f32, color: Color32, shadow: Shadow, align: Align2, alpha: f32) {
    if alpha <= 0.01 || t.is_empty() {
        return;
    }
    let font = FontId::new(size * s, FontFamily::Name(FONT.into()));
    let pos = to_screen(x, y);
    match shadow {
        Shadow::Black => {
            p.text(pos + vec2(0.0, 1.0), align, t, font.clone(), Color32::BLACK.gamma_multiply(0.3 * alpha));
            p.text(pos + vec2(1.0, 0.0), align, t, font.clone(), Color32::BLACK.gamma_multiply(0.3 * alpha));
            p.text(pos + vec2(1.0, 1.0), align, t, font.clone(), Color32::BLACK.gamma_multiply(alpha));
        }
        Shadow::Hard => {
            p.text(pos + vec2(1.0, 1.0), align, t, font.clone(), Color32::BLACK.gamma_multiply(alpha));
        }
        Shadow::Grey => {
            p.text(pos + vec2(1.0, 1.0), align, t, font.clone(), Color32::from_rgba_unmultiplied(218, 218, 218, (160.0 * alpha) as u8));
        }
        Shadow::None => {}
    }
    p.text(pos, align, t, font, color.gamma_multiply(alpha));
}

/// Shortens `t` with an ellipsis to fit `max_w` pixels.
fn ellipsize(p: &Painter, t: &str, size: f32, max_w: f32) -> String {
    let font = FontId::new(size, FontFamily::Name(FONT.into()));
    let width = |s: &str| p.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if width(t) <= max_w {
        return t.to_string();
    }
    let chars: Vec<char> = t.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cand: String = chars[..mid].iter().collect::<String>() + "…";
        if width(&cand) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

fn title_case(k: &str) -> String {
    let mut c = k.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Fills between `left(y)` and `right(y)` from `ya` to `yb` in rows `step` tall;
/// `at(x, y)` gives each corner's texture position and color.
fn rows(m: &mut Mesh, left: &dyn Fn(f32) -> f32, right: &dyn Fn(f32) -> f32, ya: f32, yb: f32, step: f32, to_screen: &dyn Fn(f32, f32) -> Pos2, at: &dyn Fn(f32, f32) -> (Pos2, Color32)) {
    let mut y = ya;
    while y < yb {
        let y2 = (y + step).min(yb);
        let (l1, r1, l2, r2) = (left(y), right(y), left(y2), right(y2));
        if r1 > l1 || r2 > l2 {
            let base = m.vertices.len() as u32;
            for (x, yy) in [(l1, y), (r1.max(l1), y), (l2, y2), (r2.max(l2), y2)] {
                let (uv, c) = at(x, yy);
                m.vertices.push(egui::epaint::Vertex { pos: to_screen(x, yy), uv, color: c });
            }
            m.add_triangle(base, base + 1, base + 2);
            m.add_triangle(base + 1, base + 3, base + 2);
        }
        y = y2;
    }
}
