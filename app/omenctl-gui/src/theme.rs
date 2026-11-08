//! "Precision instrument" look: one flat warm-grey surface, structure from spacing
//! and low-contrast hairlines (no cards, no shadows), monochrome controls whose
//! selected state is inverted, amber only for cautions and red only for faults.
//! Explanations live behind small (i) icons instead of on screen.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};

pub const MEDIUM: &str = "medium";
pub const RADIUS: u8 = 6;
pub const CONTROL_H: f32 = 26.0;

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub label: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub hairline: Color32,
    pub track: Color32,
    pub border: Color32,
    pub amber: Color32,
    pub red: Color32,
    pub dark: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LIGHT: Palette = Palette {
    bg: rgb(250, 250, 249),
    label: rgb(28, 28, 26),
    secondary: rgb(111, 110, 105),
    tertiary: rgb(168, 167, 162),
    hairline: rgb(232, 231, 227),
    track: rgb(240, 239, 236),
    border: rgb(222, 221, 217),
    amber: rgb(180, 83, 9),
    red: rgb(198, 53, 43),
    dark: false,
};

pub const DARK: Palette = Palette {
    bg: rgb(22, 22, 21),
    label: rgb(236, 236, 234),
    secondary: rgb(147, 146, 141),
    tertiary: rgb(94, 93, 89),
    hairline: rgb(42, 42, 40),
    track: rgb(33, 33, 31),
    border: rgb(50, 50, 47),
    amber: rgb(245, 158, 11),
    red: rgb(255, 107, 94),
    dark: true,
};

pub fn palette(ctx: &egui::Context) -> Palette {
    match ctx.theme() {
        egui::Theme::Dark => DARK,
        egui::Theme::Light => LIGHT,
    }
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

/// IBM Plex Sans (bundled, SIL OFL; tabular figures by default) as the UI font.
pub fn install(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert("plex".into(), egui::FontData::from_static(include_bytes!("../assets/IBMPlexSans-Regular.ttf")).into());
    f.font_data.insert("plex-medium".into(), egui::FontData::from_static(include_bytes!("../assets/IBMPlexSans-Medium.ttf")).into());
    let fallback = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    f.families.entry(FontFamily::Proportional).or_default().insert(0, "plex".into());
    let mut m = vec!["plex-medium".to_string()];
    m.extend(fallback);
    f.families.insert(FontFamily::Name(MEDIUM.into()), m);
    ctx.set_fonts(f);

    for (theme, p) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
        ctx.style_mut_of(theme, |s| {
            let v = &mut s.visuals;
            v.panel_fill = p.bg;
            v.window_fill = if p.dark { rgb(34, 34, 32) } else { rgb(255, 255, 255) };
            v.window_stroke = Stroke::new(1.0, p.border);
            v.window_corner_radius = CornerRadius::same(RADIUS);
            v.window_shadow = egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if p.dark { 80 } else { 18 }) };
            v.popup_shadow = v.window_shadow;
            v.override_text_color = Some(p.label);
            v.selection.bg_fill = p.label.gamma_multiply(0.25);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
            s.spacing.tooltip_width = 300.0;
            s.interaction.selectable_labels = false;
            s.interaction.tooltip_delay = 0.15;
            s.text_styles.insert(egui::TextStyle::Body, font(13.0));
            s.text_styles.insert(egui::TextStyle::Button, font(13.0));
            s.text_styles.insert(egui::TextStyle::Small, font(11.0));
        });
    }
}

/// Full-width low-contrast divider.
pub fn hairline(ui: &mut egui::Ui, p: &Palette) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0, p.hairline));
}

/// Small outlined (i); the explanation appears on hover.
pub fn info(ui: &mut egui::Ui, p: &Palette, text: &str) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, CONTROL_H), Sense::hover());
    let c = if resp.hovered() { p.secondary } else { p.tertiary };
    let o = rect.center();
    let painter = ui.painter();
    painter.circle_stroke(o, 6.5, Stroke::new(1.0, c));
    painter.circle_filled(o + Vec2::new(0.0, -2.7), 0.9, c);
    painter.line_segment([o + Vec2::new(0.0, -0.9), o + Vec2::new(0.0, 3.2)], Stroke::new(1.2, c));
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(text).font(font(12.0)).color(p.label));
    });
}

/// Label column of a settings row.
pub fn row_label(ui: &mut egui::Ui, p: &Palette, text: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, CONTROL_H), Sense::hover());
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, text, font(13.0), p.secondary);
}

/// Segmented control. The selected segment is inverted (label-coloured fill).
pub fn segmented(
    ui: &mut egui::Ui,
    p: &Palette,
    id: &str,
    labels: &[&str],
    selected: Option<usize>,
    enabled: &[bool],
    width: f32,
) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, CONTROL_H), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, RADIUS, p.track);
    painter.rect_stroke(rect, RADIUS, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let n = labels.len() as f32;
    let seg_w = (rect.width() - 4.0) / n;
    if let Some(sel) = selected {
        let x = ui.ctx().animate_value_with_time(egui::Id::unique(id).with("thumb"), sel as f32, 0.16);
        let thumb = Rect::from_min_size(Pos2::new(rect.left() + 2.0 + x * seg_w, rect.top() + 2.0), Vec2::new(seg_w, CONTROL_H - 4.0));
        painter.rect_filled(thumb, RADIUS - 2, p.label);
    }
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + 2.0 + i as f32 * seg_w, rect.top()), Vec2::new(seg_w, CONTROL_H));
        let on = enabled.get(i).copied().unwrap_or(true);
        let resp = ui.interact(r, egui::Id::unique(id).with(i), if on { Sense::click() } else { Sense::hover() });
        let is_sel = selected == Some(i);
        let color = if is_sel {
            p.bg
        } else if !on {
            p.tertiary
        } else if resp.hovered() {
            p.label
        } else {
            p.secondary
        };
        painter.text(r.center(), egui::Align2::CENTER_CENTER, *label, if is_sel { medium(12.5) } else { font(12.5) }, color);
        if on && resp.clicked() && !is_sel {
            clicked = Some(i);
        }
    }
    clicked
}

pub enum ButtonKind {
    Primary,
    Plain,
}

pub fn button(ui: &mut egui::Ui, p: &Palette, text: &str, kind: ButtonKind, enabled: bool) -> bool {
    let g = ui.painter().layout_no_wrap(text.to_string(), font(12.5), p.label);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 24.0, CONTROL_H), if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    match kind {
        ButtonKind::Primary => {
            let fill = if !enabled {
                p.tertiary.gamma_multiply(0.5)
            } else if resp.hovered() {
                p.label.gamma_multiply(0.85)
            } else {
                p.label
            };
            painter.rect_filled(rect, RADIUS, fill);
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, medium(12.5), p.bg);
        }
        ButtonKind::Plain => {
            if enabled && resp.hovered() {
                painter.rect_filled(rect, RADIUS, p.track);
            }
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, font(12.5), if enabled { p.label } else { p.tertiary });
        }
    }
    enabled && resp.clicked()
}

/// Fan level slider with a tick at the recommended level (amber when below it).
/// Returns (user is holding the knob, commit = released or clicked).
pub fn level_slider(ui: &mut egui::Ui, p: &Palette, value: &mut u8, range: (u8, u8), recommended: u8, width: f32) -> (bool, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, CONTROL_H), Sense::click_and_drag());
    let (lo, hi) = (range.0 as f32, range.1 as f32);
    let r = 7.0;
    let (left, right) = (rect.left() + r, rect.right() - r);
    let x_of = |v: f32| left + (v - lo) / (hi - lo) * (right - left);
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() || resp.drag_started() {
            let v = (lo + (pos.x - left) / (right - left) * (hi - lo)).round().clamp(lo, hi) as u8;
            changed = v != *value;
            *value = v;
        }
    }
    let painter = ui.painter_at(rect.expand(2.0));
    let cy = rect.center().y;
    painter.line_segment([Pos2::new(left, cy), Pos2::new(right, cy)], Stroke::new(2.0, p.border));
    let kx = x_of(*value as f32);
    painter.line_segment([Pos2::new(left, cy), Pos2::new(kx, cy)], Stroke::new(2.0, p.label));
    let rx = x_of(recommended as f32);
    let tick = if *value < recommended { p.amber } else { p.tertiary };
    painter.line_segment([Pos2::new(rx, cy - 6.0), Pos2::new(rx, cy + 6.0)], Stroke::new(1.5, tick));
    let knob = Pos2::new(kx, cy);
    painter.circle_filled(knob, r, if p.dark { p.label } else { Color32::WHITE });
    painter.circle_stroke(knob, r, Stroke::new(1.0, if p.dark { p.label } else { p.tertiary }));
    (resp.dragged() || resp.is_pointer_button_down_on(), resp.drag_stopped() || (resp.clicked() && changed))
}

/// One line of state text with a small leading mark (no boxes).
pub fn notice(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal_wrapped(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(8.0, 16.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 2.5, color);
        ui.label(egui::RichText::new(text).font(font(12.0)).color(color));
    });
}

/// Disclosure chevron (right when closed, down when open), animated by `openness`.
pub fn chevron(painter: &egui::Painter, center: Pos2, openness: f32, color: Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| Vec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let pts = [Vec2::new(-2.0, -4.0), Vec2::new(2.0, 0.0), Vec2::new(-2.0, 4.0)].map(|v| center + rot(v));
    painter.line(pts.to_vec(), Stroke::new(1.4, color));
}

/// Sparkline over a fixed value range, newest sample at the right edge.
/// Gaps (None) break the line; the newest point gets a small dot.
pub fn sparkline(painter: &egui::Painter, rect: Rect, values: &[Option<f32>], range: (f32, f32), slots: usize, stroke: Stroke) {
    if slots < 2 {
        return;
    }
    let x_of = |i: usize| rect.left() + i as f32 / (slots - 1) as f32 * rect.width();
    let y_of = |v: f32| rect.bottom() - ((v - range.0) / (range.1 - range.0)).clamp(0.0, 1.0) * rect.height();
    let offset = slots.saturating_sub(values.len());
    let mut run: Vec<Pos2> = Vec::new();
    let mut last = None;
    for (i, v) in values.iter().enumerate().skip(values.len().saturating_sub(slots)) {
        let slot = offset + i - values.len().saturating_sub(slots);
        match v {
            Some(v) => {
                let q = Pos2::new(x_of(slot), y_of(*v));
                run.push(q);
                last = Some(q);
            }
            None => {
                if run.len() > 1 {
                    painter.add(egui::Shape::line(std::mem::take(&mut run), stroke));
                }
                run.clear();
                last = None;
            }
        }
    }
    if run.len() > 1 {
        painter.add(egui::Shape::line(run, stroke));
    }
    if let Some(q) = last {
        painter.circle_filled(q, 2.0, stroke.color);
    }
}
