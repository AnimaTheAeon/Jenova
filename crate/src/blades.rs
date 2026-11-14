//! "Blades": glossy silver blades stacked left and right; the open one wears its
//! page's color. Passed blades stack on the left, coming ones on the right.
//! Switching slides the blades (0.7 s, gentle); calmer motion turns that off.
//! Shared drawing code, the same in every app that has the theme (first built
//! in Lifestream). Project doc: claude/themes/blades.md.

#![allow(dead_code)]

use eframe::egui;
use egui::{pos2, Color32, FontId, Key, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use std::time::Instant;

const SLIDE_SECS: f32 = 0.7;
const BAND_H: f32 = 78.0;
const FOOT_H: f32 = 64.0;
const ORB: f32 = 470.0;

fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    rgba(m(0), m(1), m(2), m(3))
}

/// Color at `u` (0..1) along gradient stops.
fn stops_at(stops: &[(f32, Color32)], u: f32) -> Color32 {
    let u = u.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        if u <= w[1].0 {
            return lerp(w[0].1, w[1].1, (u - w[0].0) / (w[1].0 - w[0].0).max(1e-6));
        }
    }
    stops.last().map(|s| s.1).unwrap_or(Color32::TRANSPARENT)
}

/// Mixes `over` (unmultiplied, with alpha) onto an opaque color.
fn over(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let t = top.to_srgba_unmultiplied();
    let b = base.to_srgba_unmultiplied();
    let m = |i: usize| (b[i] as f32 * (1.0 - a) + t[i] as f32 * a).round() as u8;
    rgba(m(0), m(1), m(2), b[3])
}

fn ease_out(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    1.0 - (1.0 - k).powi(3)
}

/// Blade colors: panel top-left, middle, bottom-right, and the darker bands.
pub const GREEN: [Color32; 4] = [Color32::from_rgb(74, 154, 50), Color32::from_rgb(53, 122, 34), Color32::from_rgb(42, 102, 25), Color32::from_rgb(31, 76, 18)];
pub const ORANGE: [Color32; 4] = [Color32::from_rgb(194, 116, 22), Color32::from_rgb(157, 86, 11), Color32::from_rgb(134, 72, 7), Color32::from_rgb(94, 50, 4)];
pub const BLUE: [Color32; 4] = [Color32::from_rgb(52, 121, 192), Color32::from_rgb(36, 95, 156), Color32::from_rgb(28, 79, 133), Color32::from_rgb(19, 58, 99)];
pub const PURPLE: [Color32; 4] = [Color32::from_rgb(128, 100, 180), Color32::from_rgb(103, 76, 156), Color32::from_rgb(88, 63, 138), Color32::from_rgb(61, 42, 99)];
pub const TEAL: [Color32; 4] = [Color32::from_rgb(46, 140, 140), Color32::from_rgb(32, 112, 114), Color32::from_rgb(24, 92, 96), Color32::from_rgb(14, 62, 66)];
pub const RED: [Color32; 4] = [Color32::from_rgb(196, 64, 58), Color32::from_rgb(160, 44, 40), Color32::from_rgb(136, 32, 30), Color32::from_rgb(92, 20, 18)];

/// How far a blade's curved leading edge sits right of its left line at height
/// `y`: nothing in the middle, `r` at the very top and bottom (a half ellipse).
fn arc(y: f32, top: f32, h: f32, r: f32) -> f32 {
    let d = ((y - (top + h / 2.0)) / (h / 2.0)).clamp(-1.0, 1.0);
    r * (1.0 - (1.0 - d * d).max(0.0).sqrt())
}

/// Fills the band between `left(y)` and `right(y)` from `top` to `bottom`,
/// Fills the band between `left(y)` and `right(y)` from `top` to `bottom`,
/// coloring each point with `color(x, y)`. Both side edges get a 1 px fade to
/// transparent (raw meshes aren't anti-aliased by egui), and the rows are 2 px
/// apart so curved edges stay smooth.
fn fill_rows(
    p: &Painter,
    top: f32,
    bottom: f32,
    left: impl Fn(f32) -> f32,
    right: impl Fn(f32) -> f32,
    cols: usize,
    color: impl Fn(f32, f32) -> Color32,
) {
    if bottom <= top {
        return;
    }
    const FEATHER: f32 = 1.0;
    let rows = ((bottom - top) / 2.0).ceil().max(1.0) as usize;
    let mut mesh = egui::Mesh::default();
    for r in 0..=rows {
        let y = top + (bottom - top) * r as f32 / rows as f32;
        let (l, rt) = (left(y), right(y).max(left(y)));
        // Outer fringe (transparent), the band itself, outer fringe again.
        mesh.colored_vertex(pos2(l - FEATHER, y), Color32::TRANSPARENT);
        for c in 0..=cols {
            let x = l + (rt - l) * c as f32 / cols as f32;
            mesh.colored_vertex(pos2(x, y), color(x, y));
        }
        mesh.colored_vertex(pos2(rt + FEATHER, y), Color32::TRANSPARENT);
    }
    let w = (cols + 3) as u32;
    for r in 0..rows as u32 {
        for c in 0..(cols + 2) as u32 {
            let a = r * w + c;
            mesh.add_triangle(a, a + 1, a + w + 1);
            mesh.add_triangle(a, a + w + 1, a + w);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A disc colored point by point (clipped to the circle).
fn fill_disc(p: &Painter, c: Pos2, r: f32, color: impl Fn(Pos2) -> Color32) {
    let (rings, segs) = (34u32, 160u32);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, color(c));
    for i in 1..=rings {
        let rr = r * i as f32 / rings as f32;
        for j in 0..segs {
            let a = j as f32 / segs as f32 * std::f32::consts::TAU;
            let q = c + Vec2::new(a.cos(), a.sin()) * rr;
            mesh.colored_vertex(q, color(q));
        }
    }
    // A 1 px fringe fading to transparent smooths the disc's outline.
    for j in 0..segs {
        let a = j as f32 / segs as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(c + Vec2::new(a.cos(), a.sin()) * (r + 1.0), Color32::TRANSPARENT);
    }
    for j in 0..segs {
        mesh.add_triangle(0, 1 + j, 1 + (j + 1) % segs);
    }
    for i in 1..=rings {
        let a0 = 1 + (i - 1) * segs;
        let a1 = 1 + i * segs;
        for j in 0..segs {
            let k = (j + 1) % segs;
            mesh.add_triangle(a0 + j, a0 + k, a1 + k);
            mesh.add_triangle(a0 + j, a1 + k, a1 + j);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// The glass orb in the open blade's corner: still, with one soft glint that
/// sweeps across it every few seconds (`sheen` is 0..1 through the sweep, or
/// none to hold it off to the side).
fn paint_orb(p: &Painter, min: Pos2, sheen: Option<f32>) {
    let s = ORB;
    let c = min + Vec2::splat(s / 2.0);
    let ell = |u: f32, v: f32, cx: f32, cy: f32, rx: f32, ry: f32| (((u - cx) / rx).powi(2) + ((v - cy) / ry).powi(2)).sqrt();
    let ramp = |x: f32, pts: &[(f32, f32)]| {
        let mut prev = pts[0];
        if x <= prev.0 {
            return prev.1;
        }
        for &q in &pts[1..] {
            if x <= q.0 {
                return prev.1 + (q.1 - prev.1) * (x - prev.0) / (q.0 - prev.0);
            }
            prev = q;
        }
        prev.1
    };
    // The sweeping glint: a ring 306 px across its middle, its center moving
    // from up-left of the orb to its middle (where the ring is outside it).
    let glint_c = sheen.map(|k| {
        let e = k * k * (3.0 - 2.0 * k);
        let off = -165.0 + 400.0 * e;
        min + Vec2::splat(off)
    });
    fill_disc(p, c, s / 2.0, |q| {
        let (u, v) = ((q.x - min.x) / s, (q.y - min.y) / s);
        let mut white = 1.0_f32;
        let mut add = |a: f32| white *= 1.0 - a.clamp(0.0, 1.0);
        // Body, brighter toward the upper left.
        add(ramp(ell(u, v, 0.40, 0.36, 1.0, 1.0) / 0.877, &[(0.0, 0.34), (0.3, 0.2), (0.55, 0.09), (0.63, 0.0)]));
        // A faint ring just inside the edge, and the thin rim.
        add(ramp(ell(u, v, 0.5, 0.5, 1.0, 1.0) / 0.707, &[(0.68, 0.0), (0.70, 0.1), (0.72, 0.0)]));
        add(ramp(ell(u, v, 0.5, 0.5, 0.5, 0.5), &[(0.94, 0.0), (0.975, 0.2), (1.0, 0.04)]));
        // Light gathering at the bottom, a reflection across the top.
        add(0.42 * (1.0 - ell(u, v, 0.58, 1.0, 0.56, 0.32) / 0.75).max(0.0));
        add(0.3 * (1.0 - ell(u, v, 0.5, 0.02, 0.58, 0.38) / 0.72).max(0.0));
        // The gloss and the bright sun glint near the upper-left edge.
        add(0.5 * (1.0 - ell(u, v, 0.35, 0.27, 0.2, 0.12)).max(0.0));
        add(0.95 * (1.0 - ell(u, v, 0.26, 0.19, 0.09, 0.055)).max(0.0));
        if let Some(g) = glint_c {
            let d = (q - g).length();
            add(ramp(d, &[(270.0, 0.0), (296.0, 0.12), (306.0, 0.3), (316.0, 0.12), (340.0, 0.0)]));
        }
        let aw = 1.0 - white;
        // A slightly darker band inside the edge gives it depth.
        let ad = ramp(ell(u, v, 0.5, 0.5, 0.5, 0.5), &[(0.6, 0.0), (0.88, 0.18), (1.0, 0.0)]);
        let a = aw + ad - aw * ad;
        let w = (255.0 * aw * (1.0 - ad)).round() as u8;
        Color32::from_rgba_premultiplied(w, w, w, (a * 255.0).round() as u8)
    });
}

/// Where everything goes this frame.
struct Geo {
    top: f32,
    bottom: f32,
    h: f32,
    /// Pitch between stacked tabs and the leading-edge curve.
    p: f32,
    r: f32,
    /// Blade width (it runs on under the next blade).
    bw: f32,
}

impl Geo {
    fn tab_w(&self) -> f32 {
        self.p + 2.0 + self.r
    }
    fn arc(&self, y: f32) -> f32 {
        arc(y, self.top, self.h, self.r)
    }
}

/// One blade: its name and colors.
pub struct Blade<'a> {
    pub label: &'a str,
    pub colors: [Color32; 4],
}

/// Kept between frames by the app.
#[derive(Default)]
pub struct State {
    anim: Option<(usize, Instant)>,
    last: usize,
    drag: f32,
}

/// What the top band shows: a status chip (dot color and text) and a line on its right.
pub struct Status<'a> {
    pub chip: &'a str,
    pub dot: Color32,
    pub line: &'a str,
}

pub const DOT_GOOD: Color32 = Color32::from_rgb(141, 255, 106);
pub const DOT_WARN: Color32 = Color32::from_rgb(255, 225, 74);
pub const DOT_BAD: Color32 = Color32::from_rgb(255, 122, 90);

/// What happened this frame. The app draws its page into `body` (with
/// `ui.multiply_opacity(fade)`), and handles the rest.
pub struct Frame {
    pub body: Rect,
    /// Up to where the open blade can be seen (clip the page to it).
    pub visible: Rect,
    pub fade: f32,
    /// A blade to open (clicked, arrows, drag, Left/Right keys).
    pub go: Option<usize>,
    pub chip_clicked: bool,
    /// Esc (not while typing) or the mouse's back button was pressed: the app
    /// closes what's open first, else goes back (`back` = mouse button).
    pub esc: bool,
    pub back: bool,
}

/// Paints the blades and handles switching. `cur` is the open blade.
/// `dialog` blocks the Left/Right keys (a pop-up is open).
pub fn show(ui: &mut egui::Ui, st: &mut State, blades: &[Blade], cur: usize, calm: bool, status: &Status, dialog: bool) -> Frame {
    let full = ui.max_rect();
    let ctx = ui.ctx().clone();
    let n = blades.len().max(1);
    let cur = cur.min(n - 1);
    // A page change (from anywhere) starts a slide from the blade shown last.
    if cur != st.last {
        st.anim = if calm { None } else { Some((st.last.min(n - 1), Instant::now())) };
        st.last = cur;
    }
    let k = match st.anim {
        Some((_, at)) => {
            let k = at.elapsed().as_secs_f32() / SLIDE_SECS;
            if k >= 1.0 {
                st.anim = None;
                1.0
            } else {
                ctx.request_repaint();
                k
            }
        }
        None => 1.0,
    };
    let from = st.anim.map(|a| a.0).unwrap_or(cur);
    let narrow = full.width() < 700.0;
    let (p, r, edge) = if narrow { (34.0, 22.0, 16.0) } else { (52.0, 44.0, 30.0) };
    let g = Geo { top: full.top(), bottom: full.bottom(), h: full.height(), p, r, bw: full.width() - edge - (n as f32 - 1.0) * p + r };
    let slot = |i: usize, open: usize| if i <= open { i as f32 * p } else { full.width() - edge - (n - i) as f32 * p };
    let e = ease_out(k);
    let xs: Vec<f32> = (0..n).map(|i| full.left() + slot(i, from) + (slot(i, cur) - slot(i, from)) * e).collect();
    let visible_right = |i: usize| if i + 1 < n { xs[i + 1] } else { full.right() - edge };

    let painter = ui.painter().with_clip_rect(full);
    fill_rows(&painter, full.top(), full.bottom(), |_| full.left(), |_| full.right(), 8, |x, _| {
        stops_at(&[(0.0, rgb(207, 207, 207)), (0.5, rgb(242, 242, 242)), (1.0, rgb(200, 200, 200))], (x - full.left()) / full.width())
    });
    let t = ctx.input(|i| i.time) as f32;
    let sheen = if calm || narrow { None } else { Some(((t % 6.0) / 6.0 / 0.55).min(1.0)) };
    for (i, b) in blades.iter().enumerate() {
        paint_blade(&painter, &g, xs[i], b, i == cur, visible_right(i), sheen, status);
    }
    let ex = full.right() - edge;
    fill_rows(&painter, full.top(), full.bottom(), |y| ex + arc(y, full.top(), full.height(), r * 0.7), |_| full.right(), 4, |x, _| {
        stops_at(&[(0.0, rgb(255, 255, 255)), (0.5, rgb(214, 214, 214)), (1.0, rgb(171, 171, 171))], (x - ex) / edge)
    });

    let mut go: Option<usize> = None;
    let step = |d: i32| {
        let j = (cur as i32 + d).clamp(0, n as i32 - 1) as usize;
        (j != cur).then_some(j)
    };
    let typing = ctx.egui_wants_keyboard_input();
    let (left, right, esc, back) = ui.input(|i| {
        (i.key_pressed(Key::ArrowLeft), i.key_pressed(Key::ArrowRight), i.key_pressed(Key::Escape), i.pointer.button_pressed(egui::PointerButton::Extra1))
    });
    if !typing && !dialog {
        if right {
            go = step(1);
        }
        if left {
            go = step(-1);
        }
    }
    let drag_end = |resp: &egui::Response, st: &mut State| -> Option<usize> {
        if resp.drag_started() {
            st.drag = 0.0;
        }
        if resp.dragged() {
            st.drag += resp.drag_delta().x;
        }
        if resp.drag_stopped() {
            let dx = std::mem::take(&mut st.drag);
            if dx.abs() > 70.0 {
                return step(if dx < 0.0 { 1 } else { -1 });
            }
        }
        None
    };
    // Tabs: click a blade's edge to open it.
    for (i, b) in blades.iter().enumerate() {
        if i == cur {
            continue;
        }
        let rt = if i < cur { xs[i + 1] } else { xs[i] + p };
        let rect = Rect::from_min_max(pos2(xs[i], full.top()), pos2(rt.min(full.right()), full.bottom()));
        let resp = ui.interact(rect, ui.id().with(("blade_tab", i)), Sense::click_and_drag()).on_hover_text(b.label);
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            go = Some(i);
        }
        if let Some(j) = drag_end(&resp, st) {
            go = Some(j);
        }
    }
    let x0 = xs[cur];
    let vr = visible_right(cur);
    let chip_rect = chip_rect(&painter, &g, x0, vr, status);
    let chip_resp = ui.interact(chip_rect, ui.id().with("blade_chip"), Sense::click());
    if chip_resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let open_tab = Rect::from_min_max(pos2(x0, full.top()), pos2(x0 + p, full.bottom()));
    let band = Rect::from_min_max(pos2(x0 + p, full.top()), pos2(vr, full.top() + BAND_H));
    for (id, rect) in [("blade_drag_tab", open_tab), ("blade_drag_band", band)] {
        let resp = ui.interact(rect, ui.id().with(id), Sense::drag());
        if let Some(j) = drag_end(&resp, st) {
            go = Some(j);
        }
    }
    // The ← → buttons in the bottom band.
    let fy = full.bottom() - FOOT_H / 2.0;
    for (j, d) in [(0usize, -1i32), (1, 1)] {
        let c = pos2(vr - (r + 24.0).min(54.0) - 22.0 - (1 - j) as f32 * 54.0, fy);
        let rect = Rect::from_center_size(c, Vec2::splat(44.0));
        let enabled = step(d).is_some();
        let resp = ui.interact(rect, ui.id().with(("blade_nav", j)), if enabled { Sense::click() } else { Sense::hover() });
        let fill = if !enabled { rgba(255, 255, 255, 80) } else if resp.hovered() { Color32::WHITE } else { rgba(255, 255, 255, 216) };
        painter.circle_filled(c, 22.0, fill);
        let ink = rgba(38, 32, 58, if enabled { 255 } else { 120 });
        let s = -(d as f32);
        painter.line_segment([c + Vec2::new(2.0 * s, -5.0), c + Vec2::new(-3.0 * s, 0.0)], Stroke::new(2.4, ink));
        painter.line_segment([c + Vec2::new(-3.0 * s, 0.0), c + Vec2::new(2.0 * s, 5.0)], Stroke::new(2.4, ink));
        if enabled && resp.clicked() {
            go = step(d);
        }
    }
    if !calm && !narrow {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    let xp = x0 + p + 2.0;
    let body_top = full.top() + BAND_H + 10.0;
    let fade = if st.anim.is_some() { e } else { 1.0 };
    let body = Rect::from_min_max(pos2(xp + g.arc(body_top) + 14.0, body_top), pos2(vr - (r + 30.0).min(60.0), full.bottom() - FOOT_H - 6.0))
        .translate(Vec2::new(60.0 * (1.0 - fade), 0.0));
    Frame {
        body,
        visible: Rect::from_min_max(full.min, pos2(vr, full.bottom())),
        fade,
        go,
        chip_clicked: chip_resp.clicked(),
        esc: esc && !typing,
        back,
    }
}

/// Where the status chip sits in the open blade's top band.
fn chip_rect(p: &Painter, g: &Geo, x0: f32, vr: f32, status: &Status) -> Rect {
    let sp = p.layout_no_wrap(status.line.to_string(), FontId::proportional(14.0), Color32::WHITE).size().x;
    let cw = p.layout_no_wrap(status.chip.to_string(), FontId::proportional(14.0), Color32::WHITE).size().x + 41.0;
    let right = vr - (g.r + 30.0).min(60.0) - sp - 18.0;
    Rect::from_min_size(pos2(right - cw, g.top + BAND_H / 2.0 - 13.0), Vec2::new(cw, 26.0)).intersect(Rect::from_min_max(pos2(x0, g.top), pos2(vr, g.bottom)))
}

#[allow(clippy::too_many_arguments)]
fn paint_blade(p: &Painter, g: &Geo, x0: f32, blade: &Blade, open: bool, vr: f32, sheen: Option<f32>, status: &Status) {
    let (top, bottom, h) = (g.top, g.bottom, g.h);
    let right = x0 + g.bw;
    let mid = top + h / 2.0;

    // A soft shadow on whatever lies to the left.
    for (off, a) in [(9.0, 14u8), (6.0, 18), (3.0, 24)] {
        fill_rows(p, top, bottom, |y| x0 - off + g.arc(y), |_| x0 + 40.0, 1, |_, _| rgba(0, 0, 0, a));
    }

    // ---- The tab: a rounded silver bar.
    let tw = g.tab_w();
    let tab_stops = [
        (0.0, rgb(156, 156, 156)),
        (0.14, rgb(241, 241, 241)),
        (0.26, rgb(255, 255, 255)),
        (0.48, rgb(227, 227, 227)),
        (0.78, rgb(196, 196, 196)),
        (1.0, rgb(160, 160, 160)),
    ];
    fill_rows(p, top, bottom, |y| x0 + g.arc(y), |_| x0 + tw, 18, |x, y| {
        let mut c = stops_at(&tab_stops, (x - x0) / tw);
        let dt = y - top;
        if dt < 14.0 {
            c = over(c, rgba(255, 255, 255, (140.0 * (1.0 - dt / 14.0)) as u8));
        }
        let db = bottom - y;
        if db < 18.0 {
            c = over(c, rgba(0, 0, 0, (46.0 * (1.0 - db / 18.0)) as u8));
        }
        c
    });
    // Bright line along the curved edge, and a long curved gleam inside it.
    let steps = 60;
    let edge_pts: Vec<Pos2> = (0..=steps)
        .map(|s| {
            let y = top + h * s as f32 / steps as f32;
            pos2(x0 + g.arc(y) + 2.0, y)
        })
        .collect();
    p.add(Shape::line(edge_pts, Stroke::new(3.0, rgba(255, 255, 255, 200))));
    let (g_top, g_bot) = (top + 34.0, bottom - 34.0);
    let gh = (g_bot - g_top) / 2.0;
    let mut prev: Option<Pos2> = None;
    for s in 0..=steps {
        let y = g_top + (g_bot - g_top) * s as f32 / steps as f32;
        let d = ((y - mid) / gh).clamp(-1.0, 1.0);
        let k = (1.0 - d * d).max(0.0).sqrt();
        let q = pos2(x0 + 8.5 + 15.0 * (1.0 - k), y);
        let f = (s as f32 / steps as f32).min(1.0 - s as f32 / steps as f32) / 0.22;
        if let Some(a) = prev {
            p.line_segment([a, q], Stroke::new(5.0 * k + 0.5, rgba(255, 255, 255, (190.0 * f.min(1.0)) as u8)));
        }
        prev = Some(q);
    }

    // The grip: a smooth sunken capsule in the middle of the tab, with the name inside it.
    let gw: f32 = if g.p < 40.0 { 26.0 } else { 38.0 };
    let gcx = x0 + (g.p + 2.0) / 2.0;
    let cap = Rect::from_center_size(pos2(gcx, mid), Vec2::new(gw, 300.0_f32.min(h * 0.7)));
    let rad: f32 = gw / 2.0;
    let half = |y: f32| {
        let dy = if y < cap.top() + rad { cap.top() + rad - y } else if y > cap.bottom() - rad { y - (cap.bottom() - rad) } else { 0.0 };
        (rad * rad - dy * dy).max(0.0).sqrt()
    };
    p.rect_filled(cap.translate(Vec2::new(2.0, 2.0)), rad, rgba(255, 255, 255, 120));
    p.rect_filled(cap.translate(Vec2::new(-1.0, 0.0)).expand(0.5), rad, rgba(0, 0, 0, 60));
    let grip_stops = [(0.0, rgb(176, 176, 176)), (0.4, rgb(240, 240, 240)), (0.7, rgb(228, 228, 228)), (1.0, rgb(200, 200, 200))];
    fill_rows(p, cap.top(), cap.bottom(), |y| gcx - half(y), |y| gcx + half(y), 8, |x, y| {
        let mut c = stops_at(&grip_stops, (x - cap.left()) / gw);
        let from_left = x - (gcx - half(y));
        if from_left < 7.0 {
            c = over(c, rgba(0, 0, 0, (90.0 * (1.0 - from_left / 7.0)) as u8));
        }
        let from_top = y - cap.top();
        if from_top < 7.0 {
            c = over(c, rgba(0, 0, 0, (40.0 * (1.0 - from_top / 7.0)) as u8));
        }
        let from_right = gcx + half(y) - x;
        if from_right < 4.0 {
            c = over(c, rgba(255, 255, 255, (170.0 * (1.0 - from_right / 4.0)) as u8));
        }
        c
    });
    p.rect_stroke(cap, rad, Stroke::new(1.0, rgba(255, 255, 255, 180)), egui::StrokeKind::Outside);

    // The name, written top to bottom in the grip.
    let font = FontId::proportional(if g.p < 40.0 { 18.0 } else { 22.0 });
    let ink = if open { rgb(29, 29, 29) } else { rgb(106, 106, 106) };
    let galley = p.layout_no_wrap(blade.label.to_lowercase(), font, ink);
    let sz = galley.size();
    p.add(egui::epaint::TextShape::new(pos2(gcx + 1.5 + sz.y / 2.0, mid - sz.x / 2.0), galley, ink).with_angle(std::f32::consts::FRAC_PI_2));

    // ---- The colored panel, laid over the tab like a sheet.
    let [c0, c1, c2, band] = blade.colors;
    let xp = x0 + g.p + 2.0;
    let pw = right - xp;
    let dir = Vec2::new(0.342, 0.94);
    let span = pw * dir.x.abs() + h * dir.y.abs();
    fill_rows(p, top, bottom, |y| xp + g.arc(y), |_| right, 12, |x, y| {
        stops_at(&[(0.0, c0), (0.45, c1), (1.0, c2)], ((x - xp) * dir.x + (y - top) * dir.y) / span)
    });
    // The panel's curved edge casts a soft shadow on the tab, with a lit rim.
    let along = |o: f32| -> Vec<Pos2> {
        (0..=steps)
            .map(|s| {
                let y = top + h * s as f32 / steps as f32;
                pos2(xp + g.arc(y) + o, y)
            })
            .collect()
    };
    for (o, a) in [(1.0, 70u8), (2.5, 46), (4.0, 28), (5.5, 14)] {
        p.add(Shape::line(along(-o), Stroke::new(1.6, rgba(0, 0, 0, a))));
    }
    p.add(Shape::line(along(1.0), Stroke::new(2.0, rgba(255, 255, 255, 115))));

    if open {
        let clip = Rect::from_min_max(pos2(xp, top), pos2(vr.min(right), bottom));
        if clip.width() > 300.0 {
            paint_orb(&p.with_clip_rect(clip.intersect(p.clip_rect())), pos2(right - 14.0 - ORB, top + h * 0.38), sheen);
        }
    }

    // Top and bottom bands.
    let bt = top + BAND_H;
    fill_rows(p, top, bt, |y| xp + g.arc(y), |_| right, 2, |_, y| over(band, rgba(255, 255, 255, (56.0 * (1.0 - (y - top) / (BAND_H * 0.7))).max(0.0) as u8)));
    p.line_segment([pos2(xp + g.arc(bt), bt), pos2(right, bt)], Stroke::new(2.0, rgba(255, 255, 255, 90)));
    let ft = bottom - FOOT_H;
    fill_rows(p, ft, bottom, |y| xp + g.arc(y), |_| right, 2, |_, y| over(band, rgba(0, 0, 0, (13.0 + 51.0 * (y - ft) / FOOT_H) as u8)));
    p.line_segment([pos2(xp + g.arc(ft), ft), pos2(right, ft)], Stroke::new(2.0, rgba(255, 255, 255, 64)));

    if !open {
        return;
    }
    let bold = FontId::proportional;
    p.text(pos2(xp + g.r, top + BAND_H / 2.0), egui::Align2::LEFT_CENTER, blade.label, bold(28.0), Color32::WHITE);
    p.text(pos2(vr - (g.r + 30.0).min(60.0), top + BAND_H / 2.0), egui::Align2::RIGHT_CENTER, status.line, bold(14.0), rgba(255, 255, 255, 235));
    if g.bw > 700.0 && !status.chip.is_empty() {
        let cr = chip_rect(p, g, x0, vr, status);
        p.rect_filled(cr, 13.0, rgba(255, 255, 255, 41));
        let dc = pos2(cr.left() + 16.0, cr.center().y);
        p.circle_filled(dc, 8.0, status.dot.gamma_multiply(0.25));
        p.circle_filled(dc, 4.5, status.dot);
        p.text(pos2(cr.left() + 29.0, cr.center().y), egui::Align2::LEFT_CENTER, status.chip, bold(14.0), Color32::WHITE);
    }
    // Hints along the bottom band.
    let fy = bottom - FOOT_H / 2.0;
    let mut x = xp + g.r;
    if g.bw > 600.0 {
        for (key, what) in [("", "Blades"), ("ENTER", "Select"), ("ESC", "Back")] {
            let kg = p.layout_no_wrap(key.to_string(), bold(12.0), Color32::WHITE);
            let kw = if key.is_empty() { 22.0 } else { kg.size().x };
            let kr = Rect::from_min_size(pos2(x + 8.0, fy - 10.0), Vec2::new(kw + 14.0, 20.0));
            p.rect_stroke(kr, 6.0, Stroke::new(1.5, rgba(255, 255, 255, 204)), egui::StrokeKind::Inside);
            if key.is_empty() {
                // Left and right arrows (drawn: not every font has the glyphs).
                let c = kr.center();
                let ink = Stroke::new(1.5, Color32::WHITE);
                p.line_segment([c + Vec2::new(-10.0, 0.0), c + Vec2::new(10.0, 0.0)], ink);
                for s in [-1.0_f32, 1.0] {
                    p.line_segment([c + Vec2::new(10.0 * s, 0.0), c + Vec2::new(6.5 * s, -3.5)], ink);
                    p.line_segment([c + Vec2::new(10.0 * s, 0.0), c + Vec2::new(6.5 * s, 3.5)], ink);
                }
            } else {
                p.galley(pos2(kr.left() + 7.0, fy - kg.size().y / 2.0), kg, Color32::WHITE);
            }
            let r = p.text(pos2(kr.right() + 10.0, fy), egui::Align2::LEFT_CENTER, what, bold(14.0), Color32::WHITE);
            x = r.right() + 2.0;
        }
    }
}
