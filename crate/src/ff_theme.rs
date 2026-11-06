//! Classic JRPG party-menu look: blue gradient windows with beveled silver
//! borders, drop-shadowed white text, a pointing-hand menu cursor and
//! gradient stat bars.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};

/// Window gradient corners: top-left, top-right, bottom-left, bottom-right.
pub const WIN_TL: Color32 = Color32::from_rgb(46, 86, 190);
pub const WIN_TR: Color32 = Color32::from_rgb(18, 30, 120);
pub const WIN_BL: Color32 = Color32::from_rgb(24, 40, 146);
pub const WIN_BR: Color32 = Color32::from_rgb(6, 8, 58);

pub const BORDER_LIGHT: Color32 = Color32::from_rgb(214, 216, 228);
pub const BORDER_MID: Color32 = Color32::from_rgb(140, 144, 168);
pub const BORDER_DARK: Color32 = Color32::from_rgb(44, 46, 70);

pub const TEXT: Color32 = Color32::from_rgb(244, 244, 248);
pub const TEXT_DIM: Color32 = Color32::from_rgb(170, 178, 214);
pub const SHADOW: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 200);
/// Stat labels ("LV", "HP", "Time").
pub const LABEL: Color32 = Color32::from_rgb(112, 204, 255);
pub const GOOD: Color32 = Color32::from_rgb(120, 230, 140);
pub const WARN: Color32 = Color32::from_rgb(255, 226, 96);
pub const BAD: Color32 = Color32::from_rgb(255, 110, 110);
pub const BACKDROP: Color32 = Color32::from_rgb(4, 4, 12);

pub const WINDOW_RADIUS: u8 = 7;
pub const WINDOW_PADDING: i8 = 16;
pub const MENU_ITEM_HEIGHT: f32 = 32.0;

/// Apply the global style.
pub fn apply_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BACKDROP;
    visuals.window_fill = Color32::from_rgb(22, 34, 120);
    visuals.window_stroke = Stroke::new(2.0, BORDER_LIGHT);
    visuals.window_corner_radius = CornerRadius::same(WINDOW_RADIUS);
    visuals.extreme_bg_color = Color32::from_rgb(8, 12, 52);
    visuals.faint_bg_color = Color32::from_rgba_premultiplied(18, 22, 40, 40);
    visuals.code_bg_color = Color32::from_rgb(8, 12, 52);
    visuals.hyperlink_color = LABEL;
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = BAD;
    visuals.selection.bg_fill = Color32::from_rgb(64, 112, 210);
    visuals.selection.stroke = Stroke::new(1.0, TEXT);

    let widget = |bg: Color32, stroke: Color32| egui::style::WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(4),
        fg_stroke: Stroke::new(1.5, TEXT),
        expansion: 0.0,
    };
    visuals.widgets.noninteractive = widget(Color32::from_rgb(14, 20, 70), BORDER_MID);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(90, 100, 160));
    visuals.widgets.inactive = widget(Color32::from_rgb(16, 26, 92), BORDER_MID);
    visuals.widgets.hovered = widget(Color32::from_rgb(44, 72, 170), BORDER_LIGHT);
    visuals.widgets.active = widget(Color32::from_rgb(70, 108, 214), Color32::WHITE);
    visuals.widgets.open = widget(Color32::from_rgb(36, 58, 150), BORDER_LIGHT);

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
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
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
    let top = lerp_color(WIN_TL, WIN_TR, u);
    let bottom = lerp_color(WIN_BL, WIN_BR, u);
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

/// Shapes for a gradient window with a beveled border.
pub fn window_shapes(rect: Rect) -> Vec<Shape> {
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
            Stroke::new(1.0, BORDER_DARK),
            StrokeKind::Outside,
        ),
        Shape::rect_stroke(
            rect,
            radius,
            Stroke::new(3.0, BORDER_LIGHT),
            StrokeKind::Inside,
        ),
        Shape::rect_stroke(
            rect.shrink(3.0),
            CornerRadius::same(WINDOW_RADIUS.saturating_sub(3)),
            Stroke::new(1.0, BORDER_MID),
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
    painter.text(pos + Vec2::new(1.5, 1.5), align, text, font.clone(), SHADOW);
    painter.text(pos, align, text, font, color)
}

/// A shadowed text label that takes up layout space.
pub fn title(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Response {
    let font = FontId::proportional(size);
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
    title(ui, text, 14.0, LABEL)
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
        TEXT
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
        Color32::from_rgb(10, 10, 30),
    );
    let fraction = fraction.clamp(0.0, 1.0);
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
        Stroke::new(1.0, BORDER_MID),
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
        Stroke::new(1.0, Color32::from_rgba_premultiplied(150, 160, 220, 90)),
    );
}

/// Square emblem with a letter, used as a folder "portrait".
pub fn emblem(ui: &mut Ui, letter: &str, size: f32, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let (a, b) = if active {
        (
            Color32::from_rgb(70, 130, 230),
            Color32::from_rgb(14, 30, 110),
        )
    } else {
        (
            Color32::from_rgb(90, 96, 120),
            Color32::from_rgb(30, 32, 50),
        )
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), a);
    mesh.colored_vertex(rect.right_top(), lerp_color(a, b, 0.5));
    mesh.colored_vertex(rect.right_bottom(), b);
    mesh.colored_vertex(rect.left_bottom(), lerp_color(a, b, 0.5));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(2.0, BORDER_LIGHT),
        StrokeKind::Inside,
    );
    shadow_text(
        ui,
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        FontId::proportional(size * 0.55),
        TEXT,
    );
    response
}
