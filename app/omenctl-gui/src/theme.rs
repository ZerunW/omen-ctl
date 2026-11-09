//! "Data panel" look: a light grey (or near-black) page with flat cards, big
//! tabular numbers, thin live charts. One accent colour marks the selected
//! control; CPU and GPU each have a fixed series colour; amber is for cautions
//! and red for faults. Light and dark follow the desktop theme.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};

pub const MEDIUM: &str = "medium";
pub const RADIUS: u8 = 7;
pub const CARD_RADIUS: u8 = 12;
pub const CONTROL_H: f32 = 28.0;

#[derive(Clone, Copy)]
pub struct Palette {
    /// Page background.
    pub bg: Color32,
    /// Card surface.
    pub card: Color32,
    pub card_border: Color32,
    pub label: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub hairline: Color32,
    /// Inset track behind segmented controls and sliders.
    pub track: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub cpu: Color32,
    pub gpu: Color32,
    pub system: Color32,
    pub green: Color32,
    pub amber: Color32,
    pub red: Color32,
    pub dark: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LIGHT: Palette = Palette {
    bg: rgb(243, 243, 241),
    card: rgb(255, 255, 255),
    card_border: rgb(228, 227, 223),
    label: rgb(26, 26, 25),
    secondary: rgb(104, 103, 99),
    tertiary: rgb(160, 159, 154),
    hairline: rgb(236, 235, 232),
    track: rgb(240, 240, 237),
    border: rgb(220, 219, 215),
    accent: rgb(37, 99, 235),
    on_accent: rgb(255, 255, 255),
    cpu: rgb(216, 90, 48),
    gpu: rgb(15, 140, 104),
    system: rgb(124, 101, 214),
    green: rgb(22, 128, 61),
    amber: rgb(180, 83, 9),
    red: rgb(198, 40, 40),
    dark: false,
};

pub const DARK: Palette = Palette {
    bg: rgb(18, 18, 20),
    card: rgb(28, 28, 31),
    card_border: rgb(42, 42, 46),
    label: rgb(237, 237, 239),
    secondary: rgb(150, 150, 156),
    tertiary: rgb(98, 98, 104),
    hairline: rgb(40, 40, 44),
    track: rgb(37, 37, 41),
    border: rgb(52, 52, 57),
    accent: rgb(77, 139, 245),
    on_accent: rgb(255, 255, 255),
    cpu: rgb(240, 128, 88),
    gpu: rgb(52, 196, 150),
    system: rgb(160, 140, 240),
    green: rgb(74, 210, 120),
    amber: rgb(245, 166, 35),
    red: rgb(255, 100, 90),
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
            v.window_fill = p.card;
            v.window_stroke = Stroke::new(1.0, p.card_border);
            v.window_corner_radius = CornerRadius::same(RADIUS);
            v.window_shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 28 }) };
            v.popup_shadow = v.window_shadow;
            v.menu_corner_radius = CornerRadius::same(RADIUS);
            v.override_text_color = Some(p.label);
            v.selection.bg_fill = p.accent.gamma_multiply(0.85);
            v.selection.stroke = Stroke::new(1.0, p.on_accent);
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.corner_radius = CornerRadius::same(RADIUS);
                w.fg_stroke = Stroke::new(1.0, p.label);
            }
            v.widgets.inactive.weak_bg_fill = p.track;
            v.widgets.inactive.bg_fill = p.track;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
            v.widgets.hovered.weak_bg_fill = p.hairline;
            v.widgets.hovered.bg_fill = p.hairline;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.tertiary);
            v.widgets.active.weak_bg_fill = p.border;
            v.widgets.active.bg_stroke = Stroke::new(1.0, p.tertiary);
            v.widgets.open.weak_bg_fill = p.track;
            v.widgets.open.bg_stroke = Stroke::new(1.0, p.accent);
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
            s.spacing.button_padding = Vec2::new(10.0, 5.0);
            s.spacing.interact_size.y = CONTROL_H;
            s.spacing.tooltip_width = 320.0;
            s.interaction.selectable_labels = false;
            s.interaction.tooltip_delay = 0.15;
            s.text_styles.insert(egui::TextStyle::Body, font(13.0));
            s.text_styles.insert(egui::TextStyle::Button, font(13.0));
            s.text_styles.insert(egui::TextStyle::Small, font(11.0));
        });
    }
}

/// Flat card with an optional title row. `right` draws on the title row's right side.
pub fn card<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    title: Option<&str>,
    right: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
            if let Some(t) = title {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(t).font(medium(14.0)).color(p.label));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
                });
                ui.add_space(6.0);
            }
            body(ui)
        })
        .inner
}

/// Full-width low-contrast divider.
pub fn hairline(ui: &mut egui::Ui, p: &Palette) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0, p.hairline));
}

/// Small outlined (i); the explanation appears on hover.
pub fn info(ui: &mut egui::Ui, p: &Palette, text: &str) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, 18.0), Sense::hover());
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

/// Small coloured dot followed by a label (chart legends). Drawn as one
/// item so the dot stays on the left in right-to-left layouts too.
pub fn legend(ui: &mut egui::Ui, p: &Palette, color: Color32, text: &str) {
    let g = ui.painter().layout_no_wrap(text.to_string(), font(12.0), p.secondary);
    let (r, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 14.0, 18.0), Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(Pos2::new(r.left() + 4.0, r.center().y), 3.5, color);
    painter.galley(Pos2::new(r.left() + 14.0, r.center().y - g.size().y / 2.0), g, p.secondary);
}

/// Label/value line inside a card.
pub fn kv(ui: &mut egui::Ui, p: &Palette, key: &str, value: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.label(egui::RichText::new(key).font(font(13.0)).color(p.secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).font(font(13.0)).color(color));
        });
    });
}

/// Segmented control. The selected segment is filled with the accent colour.
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
        painter.rect_filled(thumb, RADIUS - 2, p.accent);
    }
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + 2.0 + i as f32 * seg_w, rect.top()), Vec2::new(seg_w, CONTROL_H));
        let on = enabled.get(i).copied().unwrap_or(true);
        let resp = ui.interact(r, egui::Id::unique(id).with(i), if on { Sense::click() } else { Sense::hover() });
        let is_sel = selected == Some(i);
        let color = if is_sel {
            p.on_accent
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
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 26.0, CONTROL_H), if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    match kind {
        ButtonKind::Primary => {
            let fill = if !enabled {
                p.border
            } else if resp.hovered() {
                p.accent.gamma_multiply(0.88)
            } else {
                p.accent
            };
            painter.rect_filled(rect, RADIUS, fill);
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, medium(12.5), if enabled { p.on_accent } else { p.tertiary });
        }
        ButtonKind::Plain => {
            let fill = if enabled && resp.hovered() { p.hairline } else { p.track };
            painter.rect_filled(rect, RADIUS, fill);
            painter.rect_stroke(rect, RADIUS, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
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
    let r = 8.0;
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
    painter.line_segment([Pos2::new(left, cy), Pos2::new(right, cy)], Stroke::new(4.0, p.track));
    let kx = x_of(*value as f32);
    painter.line_segment([Pos2::new(left, cy), Pos2::new(kx, cy)], Stroke::new(4.0, p.accent));
    let rx = x_of(recommended as f32);
    let tick = if *value < recommended { p.amber } else { p.tertiary };
    painter.line_segment([Pos2::new(rx, cy - 8.0), Pos2::new(rx, cy + 8.0)], Stroke::new(2.0, tick));
    let knob = Pos2::new(kx, cy);
    painter.circle_filled(knob, r, if p.dark { p.label } else { Color32::WHITE });
    painter.circle_stroke(knob, r, Stroke::new(1.5, p.accent));
    (resp.dragged() || resp.is_pointer_button_down_on(), resp.drag_stopped() || (resp.clicked() && changed))
}

/// One line of state text in a tinted strip.
pub fn notice(ui: &mut egui::Ui, p: &Palette, color: Color32, text: &str) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(if p.dark { 0.16 } else { 0.09 }))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(8.0, 16.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.0, color);
                ui.label(egui::RichText::new(text).font(font(12.5)).color(color));
            });
        });
}

/// Disclosure chevron (right when closed, down when open), animated by `openness`.
pub fn chevron(painter: &egui::Painter, center: Pos2, openness: f32, color: Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| Vec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let pts = [Vec2::new(-2.0, -4.0), Vec2::new(2.0, 0.0), Vec2::new(-2.0, 4.0)].map(|v| center + rot(v));
    painter.line(pts.to_vec(), Stroke::new(1.4, color));
}

/// Line over a fixed value range, newest sample at the right edge.
/// Gaps (None) break the line; the newest point gets a small dot.
pub fn sparkline(painter: &egui::Painter, rect: Rect, values: &[Option<f32>], range: (f32, f32), slots: usize, stroke: Stroke) {
    if slots < 2 {
        return;
    }
    let x_of = |i: usize| rect.left() + i as f32 / (slots - 1) as f32 * rect.width();
    let y_of = |v: f32| rect.bottom() - ((v - range.0) / (range.1 - range.0)).clamp(0.0, 1.0) * rect.height();
    let skip = values.len().saturating_sub(slots);
    let offset = slots.saturating_sub(values.len());
    let mut run: Vec<Pos2> = Vec::new();
    let mut last = None;
    for (i, v) in values.iter().enumerate().skip(skip) {
        let slot = offset + i - skip;
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
        painter.circle_filled(q, 2.5, stroke.color);
    }
}

/// Chart scale for values up to `v`: (step, top) with a round step and at
/// most five steps, e.g. 120 -> (25, 125), 183 -> (50, 200).
pub fn nice_scale(v: f32) -> (f32, f32) {
    for step in [10.0, 20.0, 25.0, 50.0, 100.0] {
        let n = (v / step).ceil().max(1.0);
        if n <= 5.0 {
            return (step, n * step);
        }
    }
    let n = (v / 100.0).ceil().max(1.0);
    (100.0 * (n / 5.0).ceil(), 100.0 * n)
}

#[cfg(test)]
mod tests {
    #[test]
    fn nice_scale_steps() {
        assert_eq!(super::nice_scale(0.0), (10.0, 10.0));
        assert_eq!(super::nice_scale(37.0), (10.0, 40.0));
        assert_eq!(super::nice_scale(61.0), (20.0, 80.0));
        assert_eq!(super::nice_scale(120.0), (25.0, 125.0));
        assert_eq!(super::nice_scale(183.0), (50.0, 200.0));
        assert_eq!(super::nice_scale(260.0), (100.0, 300.0));
    }
}
