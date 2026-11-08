//! Look and feel modelled on macOS System Settings: grouped background, rounded
//! cards without borders, segmented controls with a sliding thumb, system colours.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};

pub const SEMIBOLD: &str = "semibold";

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub card: Color32,
    pub label: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub separator: Color32,
    pub track: Color32,
    pub thumb: Color32,
    pub blue: Color32,
    pub orange: Color32,
    pub red: Color32,
    pub green: Color32,
    pub shadow: Color32,
    pub dark: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub const LIGHT: Palette = Palette {
    bg: rgb(242, 242, 247),
    card: rgb(255, 255, 255),
    label: rgb(0, 0, 0),
    secondary: rgb(138, 138, 142),
    tertiary: rgb(196, 196, 199),
    separator: rgb(229, 229, 234),
    track: rgb(227, 227, 232),
    thumb: rgb(255, 255, 255),
    blue: rgb(0, 122, 255),
    orange: rgb(255, 149, 0),
    red: rgb(255, 59, 48),
    green: rgb(52, 199, 89),
    shadow: Color32::from_black_alpha(28),
    dark: false,
};

pub const DARK: Palette = Palette {
    bg: rgb(28, 28, 30),
    card: rgb(44, 44, 46),
    label: rgb(255, 255, 255),
    secondary: rgb(152, 152, 159),
    tertiary: rgb(84, 84, 88),
    separator: rgb(58, 58, 60),
    track: rgb(58, 58, 60),
    thumb: rgb(99, 99, 102),
    blue: rgb(10, 132, 255),
    orange: rgb(255, 159, 10),
    red: rgb(255, 69, 58),
    green: rgb(48, 209, 88),
    shadow: Color32::from_black_alpha(90),
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

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Inter (bundled, SIL OFL) as the UI font; egui's own fonts stay as fallback.
pub fn install(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert("inter".into(), egui::FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf")).into());
    f.font_data.insert("inter-sb".into(), egui::FontData::from_static(include_bytes!("../assets/Inter-SemiBold.ttf")).into());
    let fallback = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let prop = f.families.entry(FontFamily::Proportional).or_default();
    prop.insert(0, "inter".into());
    let mut sb = vec!["inter-sb".to_string()];
    sb.extend(fallback);
    f.families.insert(FontFamily::Name(SEMIBOLD.into()), sb);
    ctx.set_fonts(f);

    for (theme, p) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
        ctx.style_mut_of(theme, |s| {
            let v = &mut s.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.card;
            v.extreme_bg_color = p.card;
            v.override_text_color = Some(p.label);
            v.selection.bg_fill = p.blue;
            v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
            v.hyperlink_color = p.blue;
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.noninteractive] {
                w.corner_radius = CornerRadius::same(7);
                w.bg_stroke = Stroke::NONE;
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
            v.widgets.inactive.weak_bg_fill = p.track;
            v.widgets.inactive.bg_fill = p.track;
            v.widgets.hovered.weak_bg_fill = p.track.gamma_multiply(0.85);
            v.widgets.active.weak_bg_fill = p.track.gamma_multiply(0.7);
            s.spacing.item_spacing = Vec2::new(8.0, 8.0);
            s.spacing.button_padding = Vec2::new(14.0, 6.0);
            s.interaction.selectable_labels = false;
            s.text_styles.insert(egui::TextStyle::Body, font(13.0));
            s.text_styles.insert(egui::TextStyle::Button, font(13.0));
            s.text_styles.insert(egui::TextStyle::Small, font(11.0));
        });
    }
}

/// Small grey upper-case title above a card.
pub fn section_title(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(text.to_uppercase()).font(font(11.0)).color(p.secondary));
    });
    ui.add_space(2.0);
}

pub fn card<R>(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(p.card)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(16, 14))
        .shadow(egui::Shadow { offset: [0, 1], blur: 3, spread: 0, color: if p.dark { Color32::TRANSPARENT } else { p.shadow } })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Hairline between rows of a grouped card.
pub fn separator(ui: &mut egui::Ui, p: &Palette) {
    ui.add_space(6.0);
    let r = ui.available_rect_before_wrap();
    let y = r.top();
    ui.painter().line_segment([Pos2::new(r.left(), y), Pos2::new(r.right(), y)], Stroke::new(1.0, p.separator));
    ui.add_space(7.0);
}

fn soft_shadow(painter: &egui::Painter, rect: Rect, radius: f32, p: &Palette) {
    if !p.dark {
        painter.rect_filled(rect.translate(Vec2::new(0.0, 1.0)).expand(0.5), radius + 0.5, p.shadow);
    }
}

/// Segmented control (single choice). `enabled[i] == false` greys a segment out.
/// Returns the index the user clicked, if any (only enabled, non-selected segments).
pub fn segmented(
    ui: &mut egui::Ui,
    p: &Palette,
    id: &str,
    labels: &[&str],
    selected: Option<usize>,
    enabled: &[bool],
    width: f32,
) -> Option<usize> {
    let h = 28.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(rect, 8.0, p.track);
    let n = labels.len() as f32;
    let seg_w = (rect.width() - 4.0) / n;
    let mut clicked = None;
    if let Some(sel) = selected {
        let x = ui.ctx().animate_value_with_time(egui::Id::unique(id).with("thumb"), sel as f32, 0.18);
        let thumb = Rect::from_min_size(Pos2::new(rect.left() + 2.0 + x * seg_w, rect.top() + 2.0), Vec2::new(seg_w, h - 4.0));
        soft_shadow(&painter, thumb, 6.5, p);
        painter.rect_filled(thumb, 6.5, p.thumb);
    }
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + 2.0 + i as f32 * seg_w, rect.top()), Vec2::new(seg_w, h));
        let on = enabled.get(i).copied().unwrap_or(true);
        let resp = ui.interact(r, egui::Id::unique(id).with(i), if on { Sense::click() } else { Sense::hover() });
        let is_sel = selected == Some(i);
        // thin divider between two unselected neighbours
        if i > 0 && selected != Some(i) && selected != Some(i - 1) {
            painter.line_segment([Pos2::new(r.left(), r.top() + 7.0), Pos2::new(r.left(), r.bottom() - 7.0)], Stroke::new(1.0, p.tertiary.gamma_multiply(0.6)));
        }
        let color = if !on {
            p.tertiary
        } else if resp.hovered() && !is_sel {
            p.label.gamma_multiply(0.75)
        } else {
            p.label
        };
        let f = if is_sel { bold(13.0) } else { font(13.0) };
        painter.text(r.center(), egui::Align2::CENTER_CENTER, *label, f, color);
        if on && resp.clicked() && !is_sel {
            clicked = Some(i);
        }
    }
    clicked
}

pub enum ButtonKind {
    Primary,
    Secondary,
}

pub fn button(ui: &mut egui::Ui, p: &Palette, text: &str, kind: ButtonKind, enabled: bool) -> bool {
    let galley = ui.painter().layout_no_wrap(text.to_string(), font(13.0), Color32::WHITE);
    let size = Vec2::new(galley.size().x + 28.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let (fill, fg) = match kind {
        ButtonKind::Primary => (p.blue, Color32::WHITE),
        ButtonKind::Secondary => (p.track, p.label),
    };
    let fill = if !enabled {
        fill.gamma_multiply(0.45)
    } else if resp.is_pointer_button_down_on() {
        fill.gamma_multiply(0.8)
    } else if resp.hovered() {
        fill.gamma_multiply(0.92)
    } else {
        fill
    };
    let fg = if enabled { fg } else { fg.gamma_multiply(0.6) };
    ui.painter().rect_filled(rect, 7.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, font(13.0), fg);
    enabled && resp.clicked()
}

/// Slider for fan levels with an optional "recommended" tick.
/// Returns (user is holding the knob, released / clicked = commit).
pub fn level_slider(
    ui: &mut egui::Ui,
    p: &Palette,
    value: &mut u8,
    range: (u8, u8),
    recommended: Option<u8>,
    width: f32,
) -> (bool, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click_and_drag());
    let (lo, hi) = (range.0 as f32, range.1 as f32);
    let knob_r = 10.0;
    let left = rect.left() + knob_r;
    let right = rect.right() - knob_r;
    let x_of = |v: f32| left + (v - lo) / (hi - lo) * (right - left);
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() || resp.drag_started() {
            let v = (lo + (pos.x - left) / (right - left) * (hi - lo)).round().clamp(lo, hi) as u8;
            if v != *value {
                *value = v;
                changed = true;
            }
        }
    }
    let painter = ui.painter_at(rect.expand(4.0));
    let cy = rect.center().y;
    let track = Rect::from_min_max(Pos2::new(left, cy - 2.0), Pos2::new(right, cy + 2.0));
    painter.rect_filled(track, 2.0, p.track);
    let kx = x_of(*value as f32);
    painter.rect_filled(Rect::from_min_max(track.min, Pos2::new(kx, track.max.y)), 2.0, p.blue);
    if let Some(r) = recommended {
        let rx = x_of(r as f32);
        painter.line_segment([Pos2::new(rx, cy - 8.0), Pos2::new(rx, cy + 8.0)], Stroke::new(2.0, p.orange));
    }
    let knob = Pos2::new(kx, cy);
    if !p.dark {
        painter.circle_filled(knob + Vec2::new(0.0, 1.0), knob_r + 0.5, p.shadow);
    }
    painter.circle_filled(knob, knob_r, Color32::WHITE);
    painter.circle_stroke(knob, knob_r, Stroke::new(0.5, p.separator));
    let commit = resp.drag_stopped() || (resp.clicked() && changed);
    (resp.dragged() || resp.is_pointer_button_down_on(), commit)
}

/// Rounded coloured banner for alerts.
pub fn banner(ui: &mut egui::Ui, p: &Palette, color: Color32, title: &str, text: &str) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(if p.dark { 0.22 } else { 0.13 }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, color);
                ui.label(egui::RichText::new(title).font(bold(13.0)).color(p.label));
                ui.label(egui::RichText::new(text).font(font(13.0)).color(p.label));
            });
        });
    ui.add_space(6.0);
}

/// Disclosure chevron (right when closed, down when open), animated by `openness`.
pub fn chevron(painter: &egui::Painter, center: Pos2, openness: f32, color: Color32) {
    let a = openness * std::f32::consts::FRAC_PI_2;
    let rot = |v: Vec2| Vec2::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let pts = [Vec2::new(-2.5, -4.5), Vec2::new(2.5, 0.0), Vec2::new(-2.5, 4.5)].map(|v| center + rot(v));
    painter.line(pts.to_vec(), Stroke::new(1.8, color));
}
