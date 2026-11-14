//! Classic JRPG party-menu look: blue gradient windows with beveled silver
//! borders, drop-shadowed white text, a pointing-hand menu cursor and
//! gradient stat bars.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};

use std::sync::atomic::{AtomicU8, Ordering};

/// Which look the app is drawn in. Saved in settings as "ff" or "green".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Classic JRPG party menu (blue windows, pointing hand).
    FfMenu,
    /// Green console dashboard (glowing grid, jelly orb, slanted tabs).
    GreenDash,
    /// Glossy silver blades; the open one wears its page's color.
    Blades,
}

impl Theme {
    pub fn from_key(key: &str) -> Self {
        match key {
            "green" => Theme::GreenDash,
            "blades" => Theme::Blades,
            _ => Theme::FfMenu,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Theme::FfMenu => "ff",
            Theme::GreenDash => "green",
            Theme::Blades => "blades",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Theme::FfMenu => "FFVII Menu",
            Theme::GreenDash => "Xbox",
            Theme::Blades => "Blades",
        }
    }
}

static THEME: AtomicU8 = AtomicU8::new(0);
static HIDE_HEADINGS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// While set, page headings (titles 24px and up) are skipped: the green
/// layout already shows the page name in its title bar.
pub fn set_hide_headings(hide: bool) {
    HIDE_HEADINGS.store(hide, Ordering::Relaxed);
}

pub fn set_theme(theme: Theme) {
    THEME.store(theme as u8, Ordering::Relaxed);
}

pub fn theme() -> Theme {
    match THEME.load(Ordering::Relaxed) {
        1 => Theme::GreenDash,
        2 => Theme::Blades,
        _ => Theme::FfMenu,
    }
}

pub fn is_blades() -> bool {
    theme() == Theme::Blades
}

/// Inner margin for text boxes. Blades rounds them like pills, so the text
/// needs room to clear the curved ends (egui's default is 4 x 2).
pub fn input_margin() -> egui::Margin {
    if is_blades() { egui::Margin::symmetric(12, 4) } else { egui::Margin::symmetric(4, 2) }
}

pub fn is_green() -> bool {
    theme() == Theme::GreenDash
}

/// Every color the widgets use, per theme.
pub struct Palette {
    pub win_tl: Color32,
    pub win_tr: Color32,
    pub win_bl: Color32,
    pub win_br: Color32,
    pub border_light: Color32,
    pub border_mid: Color32,
    pub border_dark: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub shadow: Color32,
    pub label: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub backdrop: Color32,
    /// Stat bar gradient (left to right) used by the green theme.
    pub bar_from: Color32,
    pub bar_to: Color32,
    /// Highlight fill (selected tab / row) and the text drawn on it.
    pub accent: Color32,
    pub on_accent: Color32,
}

pub const FF_PALETTE: Palette = Palette {
    win_tl: Color32::from_rgb(46, 86, 190),
    win_tr: Color32::from_rgb(18, 30, 120),
    win_bl: Color32::from_rgb(24, 40, 146),
    win_br: Color32::from_rgb(6, 8, 58),
    border_light: Color32::from_rgb(214, 216, 228),
    border_mid: Color32::from_rgb(140, 144, 168),
    border_dark: Color32::from_rgb(44, 46, 70),
    text: Color32::from_rgb(244, 244, 248),
    text_dim: Color32::from_rgb(170, 178, 214),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 200),
    label: Color32::from_rgb(112, 204, 255),
    good: Color32::from_rgb(120, 230, 140),
    warn: Color32::from_rgb(255, 226, 96),
    bad: Color32::from_rgb(255, 110, 110),
    backdrop: Color32::from_rgb(4, 4, 12),
    bar_from: Color32::from_rgb(30, 110, 230),
    bar_to: Color32::from_rgb(140, 230, 255),
    accent: Color32::from_rgb(64, 112, 210),
    on_accent: Color32::WHITE,
};

pub const GREEN_PALETTE: Palette = Palette {
    win_tl: Color32::from_rgba_premultiplied(26, 86, 12, 225),
    win_tr: Color32::from_rgba_premultiplied(14, 60, 8, 225),
    win_bl: Color32::from_rgba_premultiplied(10, 48, 6, 230),
    win_br: Color32::from_rgba_premultiplied(3, 22, 3, 235),
    border_light: Color32::from_rgb(160, 255, 80),
    border_mid: Color32::from_rgb(80, 160, 40),
    border_dark: Color32::from_rgb(8, 34, 6),
    text: Color32::from_rgb(228, 255, 196),
    text_dim: Color32::from_rgb(166, 220, 128),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 200),
    label: Color32::from_rgb(200, 255, 74),
    good: Color32::from_rgb(140, 255, 110),
    warn: Color32::from_rgb(255, 225, 74),
    bad: Color32::from_rgb(255, 122, 90),
    backdrop: Color32::from_rgb(2, 12, 2),
    bar_from: Color32::from_rgb(79, 181, 26),
    bar_to: Color32::from_rgb(226, 255, 106),
    accent: Color32::from_rgb(220, 255, 100),
    on_accent: Color32::from_rgb(28, 53, 4),
};

/// Blades: white text and see-through glass on the open blade's color.
pub const BLADES_PALETTE: Palette = Palette {
    win_tl: Color32::from_rgba_premultiplied(30, 30, 30, 30),
    win_tr: Color32::from_rgba_premultiplied(20, 20, 20, 20),
    win_bl: Color32::from_rgba_premultiplied(18, 18, 18, 18),
    win_br: Color32::from_rgba_premultiplied(10, 10, 10, 10),
    border_light: Color32::from_rgb(255, 255, 255),
    border_mid: Color32::from_rgb(222, 226, 236),
    border_dark: Color32::from_rgb(40, 40, 60),
    text: Color32::from_rgb(255, 255, 255),
    text_dim: Color32::from_rgb(228, 232, 242),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 70),
    label: Color32::from_rgb(238, 242, 255),
    good: Color32::from_rgb(214, 255, 196),
    warn: Color32::from_rgb(255, 238, 160),
    bad: Color32::from_rgb(255, 204, 194),
    backdrop: Color32::from_rgb(207, 207, 207),
    bar_from: Color32::from_rgb(255, 255, 255),
    bar_to: Color32::from_rgb(226, 255, 206),
    accent: Color32::from_rgba_premultiplied(96, 96, 96, 96),
    on_accent: Color32::WHITE,
};

pub fn pal() -> &'static Palette {
    match theme() {
        Theme::GreenDash => &GREEN_PALETTE,
        Theme::Blades => &BLADES_PALETTE,
        Theme::FfMenu => &FF_PALETTE,
    }
}

macro_rules! palette_fns {
    ($($name:ident => $field:ident),* $(,)?) => {
        $(
            #[allow(non_snake_case)]
            #[inline]
            pub fn $name() -> Color32 { pal().$field }
        )*
    };
}
palette_fns! {
    WIN_TL => win_tl, WIN_TR => win_tr, WIN_BL => win_bl, WIN_BR => win_br,
    BORDER_LIGHT => border_light, BORDER_MID => border_mid, BORDER_DARK => border_dark,
    TEXT => text, TEXT_DIM => text_dim, SHADOW => shadow, LABEL => label,
    GOOD => good, WARN => warn, BAD => bad, BACKDROP => backdrop,
}

/// The blocky display face used by the green theme for headings.
pub fn display_font(size: f32) -> FontId {
    if is_green() {
        FontId::new(size, egui::FontFamily::Name("display".into()))
    } else {
        FontId::proportional(size)
    }
}

/// Window gradient corners: top-left, top-right, bottom-left, bottom-right.


/// Stat labels ("LV", "HP", "Time").

pub const WINDOW_RADIUS: u8 = 7;
pub const WINDOW_PADDING: i8 = 16;
pub const MENU_ITEM_HEIGHT: f32 = 32.0;

/// Apply the global style.
pub fn apply_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    let g = is_green();
    // (FF value, green value)
    let pick = |ff: Color32, green: Color32| if g { green } else { ff };
    visuals.override_text_color = Some(TEXT());
    visuals.panel_fill = BACKDROP();
    visuals.window_fill = pick(Color32::from_rgb(22, 34, 120), Color32::from_rgb(10, 40, 6));
    visuals.window_stroke = Stroke::new(2.0, BORDER_LIGHT());
    visuals.window_corner_radius = CornerRadius::same(WINDOW_RADIUS);
    visuals.extreme_bg_color = pick(Color32::from_rgb(8, 12, 52), Color32::from_rgb(2, 16, 2));
    visuals.faint_bg_color = pick(Color32::from_rgba_premultiplied(18, 22, 40, 40), Color32::from_rgba_premultiplied(10, 30, 6, 40));
    visuals.code_bg_color = pick(Color32::from_rgb(8, 12, 52), Color32::from_rgb(2, 16, 2));
    visuals.hyperlink_color = LABEL();
    visuals.warn_fg_color = WARN();
    visuals.error_fg_color = BAD();
    visuals.selection.bg_fill = pick(Color32::from_rgb(64, 112, 210), Color32::from_rgb(70, 140, 24));
    visuals.selection.stroke = Stroke::new(1.0, TEXT());

    let widget = |bg: Color32, stroke: Color32| egui::style::WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(4),
        fg_stroke: Stroke::new(1.5, TEXT()),
        expansion: 0.0,
    };
    visuals.widgets.noninteractive = widget(pick(Color32::from_rgb(14, 20, 70), Color32::from_rgb(6, 26, 4)), BORDER_MID());
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pick(Color32::from_rgb(90, 100, 160), Color32::from_rgb(60, 120, 30)));
    visuals.widgets.inactive = widget(pick(Color32::from_rgb(16, 26, 92), Color32::from_rgb(16, 60, 8)), BORDER_MID());
    visuals.widgets.hovered = widget(pick(Color32::from_rgb(44, 72, 170), Color32::from_rgb(40, 110, 16)), BORDER_LIGHT());
    visuals.widgets.active = widget(pick(Color32::from_rgb(70, 108, 214), Color32::from_rgb(150, 220, 40)), Color32::WHITE);
    visuals.widgets.open = widget(pick(Color32::from_rgb(36, 58, 150), Color32::from_rgb(30, 90, 12)), BORDER_LIGHT());
    if g {
        // Slanted look: square corners on buttons and boxes.
        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            w.corner_radius = CornerRadius::same(1);
        }
    }

    if is_blades() {
        // Glass: see-through white on the blade's color, rounded like pills.
        let glass = |a: u8| Color32::from_rgba_unmultiplied(255, 255, 255, a);
        visuals.window_fill = Color32::from_rgb(58, 52, 78);
        visuals.window_stroke = Stroke::new(1.5, glass(160));
        visuals.window_corner_radius = CornerRadius::same(18);
        visuals.extreme_bg_color = Color32::from_rgba_unmultiplied(0, 0, 0, 60);
        visuals.faint_bg_color = glass(12);
        visuals.code_bg_color = Color32::from_rgba_unmultiplied(0, 0, 0, 60);
        visuals.selection.bg_fill = glass(80);
        visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        let w = |bg: u8, stroke: u8| egui::style::WidgetVisuals {
            bg_fill: glass(bg),
            weak_bg_fill: glass(bg),
            bg_stroke: Stroke::new(1.0, glass(stroke)),
            corner_radius: CornerRadius::same(12),
            fg_stroke: Stroke::new(1.5, Color32::WHITE),
            expansion: 0.0,
        };
        visuals.widgets.noninteractive = w(18, 70);
        visuals.widgets.inactive = w(40, 130);
        visuals.widgets.hovered = w(72, 210);
        visuals.widgets.active = w(110, 255);
        visuals.widgets.open = w(60, 200);
    }
    let blades_pad = is_blades();

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        use egui::{FontFamily::*, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, Proportional)),
            (TextStyle::Body, FontId::new(15.0, Proportional)),
            (TextStyle::Button, FontId::new(15.0, Proportional)),
            (TextStyle::Heading, FontId::new(24.0, Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, Monospace)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        // Blades: pill buttons need more room at the rounded ends.
        style.spacing.button_padding = if blades_pad { Vec2::new(14.0, 5.0) } else { Vec2::new(10.0, 4.0) };
        style.spacing.interact_size.y = 24.0;
        style.spacing.scroll.floating = false;
        style.spacing.scroll.bar_width = 8.0;
    });
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// Bilinear blend of the four window corner colors.
fn window_color(rect: Rect, p: Pos2) -> Color32 {
    let u = ((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let v = ((p.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
    let top = lerp_color(WIN_TL(), WIN_TR(), u);
    let bottom = lerp_color(WIN_BL(), WIN_BR(), u);
    lerp_color(top, bottom, v)
}

/// Outline points of a rounded rectangle, clockwise from the top-left arc.
fn rounded_points(rect: Rect, r: f32) -> Vec<Pos2> {
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let segs = 6;
    let corners = [
        (Pos2::new(rect.left() + r, rect.top() + r), 180.0_f32),
        (Pos2::new(rect.right() - r, rect.top() + r), 270.0),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 90.0),
    ];
    let mut pts = Vec::with_capacity(4 * (segs + 1));
    for (center, start) in corners {
        for i in 0..=segs {
            let a = (start + 90.0 * i as f32 / segs as f32).to_radians();
            pts.push(center + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    pts
}

/// Corner-cut outline: top-left and bottom-right corners clipped at `cut`.
pub fn chamfer_points(rect: Rect, cut: f32) -> Vec<Pos2> {
    let c = cut.min(rect.width() / 3.0).min(rect.height() / 3.0);
    vec![
        Pos2::new(rect.left() + c, rect.top()),
        Pos2::new(rect.right(), rect.top()),
        Pos2::new(rect.right(), rect.bottom() - c),
        Pos2::new(rect.right() - c, rect.bottom()),
        Pos2::new(rect.left(), rect.bottom()),
        Pos2::new(rect.left(), rect.top() + c),
    ]
}

/// Fills a convex outline with the window gradient.
fn gradient_fill(rect: Rect, pts: &[Pos2]) -> Shape {
    let mut mesh = egui::Mesh::default();
    let center = rect.center();
    mesh.colored_vertex(center, window_color(rect, center));
    for p in pts {
        mesh.colored_vertex(*p, window_color(rect, *p));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    Shape::mesh(mesh)
}

/// Green theme window: translucent green glass with clipped corners.
fn green_window_shapes(rect: Rect) -> Vec<Shape> {
    let cut = 22.0;
    let pts = chamfer_points(rect, cut);
    let inner = chamfer_points(rect.shrink(5.0), cut - 3.0);
    let glow = |w: f32, a: u8| {
        Shape::closed_line(pts.clone(), Stroke::new(w, Color32::from_rgba_unmultiplied(150, 255, 60, a)))
    };
    vec![
        glow(9.0, 18),
        glow(5.0, 36),
        gradient_fill(rect, &pts),
        Shape::closed_line(pts.clone(), Stroke::new(2.0, BORDER_LIGHT().gamma_multiply(0.75))),
        Shape::closed_line(inner, Stroke::new(1.0, Color32::from_rgba_unmultiplied(160, 255, 80, 40))),
    ]
}

/// Shapes for a gradient window with a beveled border.
pub fn window_shapes(rect: Rect) -> Vec<Shape> {
    if is_green() {
        return green_window_shapes(rect);
    }
    if is_blades() {
        // A pane of glass: faint white, a thin light rim and a bright top edge.
        let radius = CornerRadius::same(18);
        return vec![
            Shape::rect_filled(rect, radius, Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
            Shape::rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 90)), StrokeKind::Inside),
            Shape::line_segment(
                [Pos2::new(rect.left() + 18.0, rect.top() + 1.0), Pos2::new(rect.right() - 18.0, rect.top() + 1.0)],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 150)),
            ),
        ];
    }
    let r = WINDOW_RADIUS as f32;
    let mut mesh = egui::Mesh::default();
    let center = rect.center();
    mesh.colored_vertex(center, window_color(rect, center));
    let pts = rounded_points(rect.shrink(1.0), r);
    for p in &pts {
        mesh.colored_vertex(*p, window_color(rect, *p));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    let radius = CornerRadius::same(WINDOW_RADIUS);
    vec![
        // Soft drop shadow.
        Shape::rect_filled(
            rect.translate(Vec2::new(3.0, 4.0)),
            radius,
            Color32::from_black_alpha(140),
        ),
        Shape::mesh(mesh),
        Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, BORDER_DARK()),
            StrokeKind::Outside,
        ),
        Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(3.0, BORDER_LIGHT()),
            StrokeKind::Inside,
        ),
        Shape::rect_stroke(
            rect.shrink(3.0),
            CornerRadius::same(WINDOW_RADIUS.saturating_sub(3)),
            Stroke::new(1.0, BORDER_MID()),
            StrokeKind::Inside,
        ),
    ]
}

/// Paint a window into `rect` and lay out contents inside it.
pub fn window_at<R>(
    ui: &mut Ui,
    rect: Rect,
    id: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.painter().extend(window_shapes(rect));
    let inner = rect.shrink(WINDOW_PADDING as f32);
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(id)
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(rect.shrink(4.0));
    add_contents(&mut child)
}

/// A window that sizes itself to its contents.
pub fn window<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let bg = ui.painter().add(Shape::Noop);
    let response = egui::Frame::new()
        .inner_margin(Margin::same(WINDOW_PADDING))
        .show(ui, add_contents);
    let shapes = window_shapes(response.response.rect);
    ui.painter().set(bg, Shape::Vec(shapes));
    response.inner
}

/// Text with a drop shadow.
pub fn shadow_text(
    ui: &Ui,
    pos: Pos2,
    align: egui::Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    let painter = ui.painter();
    painter.text(pos + Vec2::new(1.5, 1.5), align, text, font.clone(), SHADOW());
    painter.text(pos, align, text, font, color)
}

/// A shadowed text label that takes up layout space.
pub fn title(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Response {
    if size >= 24.0 && HIDE_HEADINGS.load(Ordering::Relaxed) {
        return ui.allocate_response(Vec2::ZERO, Sense::hover());
    }
    // Green theme: big headings in the blocky display face, upper case.
    let upper;
    let (text, font) = if is_green() && size >= 19.0 {
        upper = text.to_uppercase();
        (upper.as_str(), display_font(size * 0.86))
    } else {
        (text, FontId::proportional(size))
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), color);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + Vec2::new(2.0, 2.0), Sense::hover());
    shadow_text(ui, rect.min, egui::Align2::LEFT_TOP, text, font, color);
    response
}

/// Small cyan stat label like "LV" or "HP".
pub fn stat_label(ui: &mut Ui, text: &str) -> Response {
    if is_green() {
        let t = text.to_uppercase();
        let font = display_font(11.5);
        let galley = ui.painter().layout_no_wrap(t.clone(), font.clone(), LABEL());
        let (rect, response) = ui.allocate_exact_size(galley.size() + Vec2::new(2.0, 2.0), Sense::hover());
        shadow_text(ui, rect.min, egui::Align2::LEFT_TOP, &t, font, LABEL());
        return response;
    }
    title(ui, text, 14.0, LABEL())
}

/// Pointing hand cursor; `tip` is where the index finger points.
pub fn paint_hand(painter: &egui::Painter, tip: Pos2, scale: f32) {
    let s = scale;
    let o = tip - Vec2::new(23.0 * s, 7.0 * s);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(o + Vec2::new(x0, y0) * s, o + Vec2::new(x1, y1) * s)
    };
    let outline = Stroke::new(1.2 * s.max(0.8), Color32::from_rgb(30, 30, 40));
    let white = Color32::from_rgb(250, 250, 252);
    let shade = Color32::from_rgb(196, 200, 214);

    // Drop shadow.
    painter.rect_filled(
        r(1.0, 4.0, 24.0, 17.0),
        CornerRadius::same(4),
        Color32::from_black_alpha(90),
    );
    // Cuff.
    painter.rect(
        r(0.0, 3.0, 6.0, 15.0),
        CornerRadius::same(2),
        shade,
        outline,
        StrokeKind::Middle,
    );
    // Palm.
    painter.rect(
        r(5.0, 2.0, 16.0, 15.0),
        CornerRadius::same(4),
        white,
        outline,
        StrokeKind::Middle,
    );
    // Index finger.
    painter.rect(
        r(12.0, 4.5, 23.0, 9.0),
        CornerRadius::same(3),
        white,
        outline,
        StrokeKind::Middle,
    );
    // Curled fingers.
    for y in [9.0, 12.0] {
        painter.rect(
            r(12.0, y, 17.0, y + 3.0),
            CornerRadius::same(2),
            white,
            outline,
            StrokeKind::Middle,
        );
    }
    // Thumb crease.
    painter.line_segment(
        [o + Vec2::new(7.0, 5.0) * s, o + Vec2::new(12.0, 6.0) * s],
        Stroke::new(1.0, shade),
    );
}

/// A menu command with the hand cursor when selected or hovered.
pub fn menu_item(ui: &mut Ui, label: &str, selected: bool, enabled: bool) -> Response {
    let height = MENU_ITEM_HEIGHT;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let response = if enabled {
        response
    } else {
        response.on_hover_text("Install dependencies first")
    };
    let color = if !enabled {
        Color32::from_rgb(110, 116, 150)
    } else if selected {
        TEXT()
    } else {
        Color32::from_rgb(214, 218, 236)
    };
    let text_pos = Pos2::new(rect.left() + 40.0, rect.center().y);
    shadow_text(
        ui,
        text_pos,
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(20.0),
        color,
    );

    if enabled && (selected || response.hovered()) {
        let t = ui.input(|i| i.time) as f32;
        let bob = if selected {
            (t * 6.0).sin().abs() * 2.5
        } else {
            0.0
        };
        paint_hand(
            ui.painter(),
            Pos2::new(rect.left() + 34.0 - bob, rect.center().y),
            1.35,
        );
        if selected {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Gradient stat bar (HP/MP style) with an optional caption to its right.
pub fn stat_bar(ui: &mut Ui, fraction: f32, width: f32, from: Color32, to: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect.expand(1.0),
        CornerRadius::same(2),
        if is_green() {
            Color32::from_rgb(0, 14, 0)
        } else if is_blades() {
            Color32::from_rgba_unmultiplied(0, 0, 0, 70)
        } else {
            Color32::from_rgb(10, 10, 30)
        },
    );
    let fraction = fraction.clamp(0.0, 1.0);
    // The green theme keeps every bar lime, whatever the caller asked for.
    let (from, to) = if is_green() || (is_blades() && from.b() > from.g()) { (pal().bar_from, pal().bar_to) } else { (from, to) };
    if fraction > 0.0 {
        let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * fraction, rect.height()));
        let mut mesh = egui::Mesh::default();
        let top_to = lerp_color(to, Color32::WHITE, 0.25);
        mesh.colored_vertex(fill.left_top(), lerp_color(from, Color32::WHITE, 0.25));
        mesh.colored_vertex(fill.right_top(), top_to);
        mesh.colored_vertex(fill.right_bottom(), to);
        mesh.colored_vertex(fill.left_bottom(), from);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
    }
    painter.rect_stroke(
        rect.expand(1.0),
        CornerRadius::same(2),
        Stroke::new(1.0, BORDER_MID()),
        StrokeKind::Outside,
    );
    response
}

/// A thin separator that suits the windows.
pub fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.0), Sense::hover());
    let y = rect.center().y;
    ui.painter().line_segment(
        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
        if is_green() {
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(150, 255, 70, 50))
        } else if is_blades() {
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 60))
        } else {
            Stroke::new(1.0, Color32::from_rgba_premultiplied(150, 160, 220, 90))
        },
    );
}

/// Radial gradient disc made of rings, center color to edge color.
pub fn radial_disc(painter: &egui::Painter, center: Pos2, radius: f32, stops: &[(f32, Color32)]) {
    let segs = 48;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, stops[0].1);
    let mut prev_ring: Option<u32> = None;
    for &(t, color) in stops.iter().skip(1) {
        let base = mesh.vertices.len() as u32;
        for i in 0..segs {
            let a = i as f32 / segs as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + Vec2::new(a.cos(), a.sin()) * radius * t, color);
        }
        for i in 0..segs as u32 {
            let j = (i + 1) % segs as u32;
            match prev_ring {
                None => mesh.add_triangle(0, base + i, base + j),
                Some(p) => {
                    mesh.add_triangle(p + i, base + i, base + j);
                    mesh.add_triangle(p + i, base + j, p + j);
                }
            }
        }
        prev_ring = Some(base);
    }
    painter.add(Shape::mesh(mesh));
}

/// A glossy green orb (the green theme's "portrait" and status light).
/// `wobble` (0..1 phase) gently squashes it for the big animated orb.
pub fn paint_orb(painter: &egui::Painter, center: Pos2, r: f32, lit: bool, wobble: f32) {
    let (core, mid, edge, rim) = if lit {
        (
            Color32::from_rgb(244, 255, 156),
            Color32::from_rgb(158, 232, 44),
            Color32::from_rgb(79, 181, 26),
            Color32::from_rgb(28, 90, 10),
        )
    } else {
        (
            Color32::from_rgb(200, 214, 190),
            Color32::from_rgb(110, 128, 98),
            Color32::from_rgb(52, 66, 44),
            Color32::from_rgb(24, 32, 20),
        )
    };
    if lit {
        radial_disc(painter, center, r * 1.5, &[
            (0.0, Color32::from_rgba_unmultiplied(170, 255, 60, 70)),
            (0.66, Color32::from_rgba_unmultiplied(170, 255, 60, 30)),
            (1.0, Color32::TRANSPARENT),
        ]);
    }
    let squash = 1.0 + 0.03 * (wobble * std::f32::consts::TAU).sin();
    radial_disc(painter, center, r * squash, &[(0.0, core), (0.35, mid), (0.72, edge), (1.0, rim)]);
    painter.circle_stroke(center, r, Stroke::new((r * 0.06).max(1.0), Color32::from_rgba_unmultiplied(20, 70, 6, 160)));
    // Gloss highlight.
    let g = center + Vec2::new(-r * 0.32, -r * 0.42);
    radial_disc(painter, g, r * 0.32, &[
        (0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 190)),
        (1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 0)),
    ]);
}
