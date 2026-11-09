//! Window frame drawn by the app itself (the window has no system decorations):
//! rounded corners, a flat header bar you can drag, GNOME/libadwaita style
//! window buttons in the order of the desktop's `button-layout`, resizing from
//! the edges, and a zoom that follows the desktop text scaling and grows with
//! the window.

use crate::theme::Palette;
use eframe::egui::{self, Color32, CornerRadius, CursorIcon, Pos2, Rect, ResizeDirection, Sense, Stroke, Vec2, ViewportCommand};

pub const WINDOW_RADIUS: u8 = 12;
pub const BAR_H: f32 = 46.0;
const BTN: f32 = 24.0;
const BTN_GAP: f32 = 12.0;
const BTN_MARGIN: f32 = 11.0;
const EDGE: f32 = 6.0;
/// Content size (in points at the desktop text scale) the layout is designed for;
/// a bigger window enlarges everything up to MAX_GROW.
const DESIGN: Vec2 = Vec2::new(1000.0, 800.0);
const MAX_GROW: f32 = 1.4;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Btn {
    Minimize,
    Maximize,
    Close,
}

/// Desktop settings read once at start.
pub struct Desktop {
    pub left: Vec<Btn>,
    pub right: Vec<Btn>,
    /// GNOME "Large text" / text-scaling-factor.
    pub text_scale: f32,
}

fn gsettings(schema: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings").args(["get", schema, key]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string())
}

/// "appmenu:minimize,maximize,close" -> buttons left and right of the title.
pub fn parse_layout(s: &str) -> (Vec<Btn>, Vec<Btn>) {
    let side = |part: &str| {
        part.split(',')
            .filter_map(|b| match b.trim() {
                "minimize" => Some(Btn::Minimize),
                "maximize" => Some(Btn::Maximize),
                "close" => Some(Btn::Close),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    match s.split_once(':') {
        Some((l, r)) => (side(l), side(r)),
        None => (vec![], side(s)),
    }
}

impl Desktop {
    pub fn read() -> Self {
        let (left, right) = gsettings("org.gnome.desktop.wm.preferences", "button-layout")
            .map(|s| parse_layout(&s))
            .filter(|(l, r)| !l.is_empty() || !r.is_empty())
            .unwrap_or_else(|| (vec![], vec![Btn::Minimize, Btn::Maximize, Btn::Close]));
        let text_scale = gsettings("org.gnome.desktop.interface", "text-scaling-factor")
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| (0.5..=3.0).contains(v))
            .unwrap_or(1.0);
        Desktop { left, right, text_scale }
    }
}

/// Zoom = desktop text scale, times up to MAX_GROW when the window is larger
/// than the design size. Computed from the window size in desktop pixels, so
/// changing the zoom does not feed back into it.
pub fn update_zoom(ctx: &egui::Context, text_scale: f32) {
    let Some(native) = ctx.native_pixels_per_point() else { return };
    let Some(inner) = ctx.input(|i| i.viewport().inner_rect) else { return };
    let desktop_px = inner.size() * ctx.pixels_per_point() / native;
    let at_scale = desktop_px / text_scale;
    let grow = (at_scale.x / DESIGN.x).min(at_scale.y / DESIGN.y).clamp(1.0, MAX_GROW);
    let z = text_scale * grow;
    if (z - ctx.zoom_factor()).abs() > 0.01 {
        ctx.set_zoom_factor(z);
    }
}

pub fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

pub fn window_radius(ctx: &egui::Context) -> CornerRadius {
    CornerRadius::same(if maximized(ctx) { 0 } else { WINDOW_RADIUS })
}

/// Outline of the window: a hairline so the rounded edge reads on any wallpaper.
pub fn outline(p: &Palette) -> Stroke {
    Stroke::new(1.0, if p.dark { Color32::from_white_alpha(22) } else { Color32::from_black_alpha(38) })
}

/// Resize from the window edges (the window has no system frame to grab).
/// Call before drawing anything else so a press on the edge starts a resize.
/// Returns true while the pointer is on an edge.
pub fn edge_resize(ctx: &egui::Context) -> bool {
    if maximized(ctx) {
        return false;
    }
    let rect = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return false };
    let (l, r) = (pos.x - rect.left() < EDGE, rect.right() - pos.x < EDGE);
    let (t, b) = (pos.y - rect.top() < EDGE, rect.bottom() - pos.y < EDGE);
    let corner = EDGE * 2.5;
    let (lc, rc) = (pos.x - rect.left() < corner, rect.right() - pos.x < corner);
    let (tc, bc) = (pos.y - rect.top() < corner, rect.bottom() - pos.y < corner);
    let dir = match () {
        _ if (t && lc) || (l && tc) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)),
        _ if (t && rc) || (r && tc) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)),
        _ if (b && lc) || (l && bc) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)),
        _ if (b && rc) || (r && bc) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)),
        _ if t => Some((ResizeDirection::North, CursorIcon::ResizeNorth)),
        _ if b => Some((ResizeDirection::South, CursorIcon::ResizeSouth)),
        _ if l => Some((ResizeDirection::West, CursorIcon::ResizeWest)),
        _ if r => Some((ResizeDirection::East, CursorIcon::ResizeEast)),
        _ => None,
    };
    let Some((d, cursor)) = dir else { return false };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
    }
    true
}

/// Flat header bar across the top: drag to move, double-click to maximize,
/// window buttons at the sides.
pub fn header_bar(ui: &mut egui::Ui, p: &Palette, desk: &Desktop, on_edge: bool) {
    let ctx = ui.ctx().clone();
    let (bar, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), BAR_H), Sense::click_and_drag());
    let max = maximized(&ctx);
    if resp.drag_started_by(egui::PointerButton::Primary) && !on_edge {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    let cy = bar.center().y;
    let mut x = bar.right() - BTN_MARGIN - BTN / 2.0;
    for b in desk.right.iter().rev() {
        button(ui, p, *b, Pos2::new(x, cy), max);
        x -= BTN + BTN_GAP;
    }
    let mut x = bar.left() + BTN_MARGIN + BTN / 2.0;
    for b in &desk.left {
        button(ui, p, *b, Pos2::new(x, cy), max);
        x += BTN + BTN_GAP;
    }
}

/// libadwaita window control: a round 24 px button with a 16 px symbolic icon,
/// background 10 % of the text colour, 15 % on hover, 30 % pressed.
fn button(ui: &mut egui::Ui, p: &Palette, b: Btn, center: Pos2, max: bool) {
    let rect = Rect::from_center_size(center, Vec2::splat(BTN));
    let id = egui::Id::unique(("window-button", b as u8));
    let resp = ui.interact(rect, id, Sense::click());
    let alpha = if resp.is_pointer_button_down_on() { 0.30 } else if resp.hovered() { 0.15 } else { 0.10 };
    let painter = ui.painter();
    painter.circle_filled(center, BTN / 2.0, p.label.gamma_multiply(alpha));
    let s = Stroke::new(1.4, p.label);
    let h = 4.0; // half icon size: icons are 8 x 8 px inside the 16 px box
    match b {
        Btn::Minimize => {
            painter.line_segment([Pos2::new(center.x - h, center.y + h * 0.75), Pos2::new(center.x + h, center.y + h * 0.75)], s);
        }
        Btn::Maximize if max => {
            // restore: two overlapping squares
            let back = Rect::from_min_max(Pos2::new(center.x - h + 2.0, center.y - h), Pos2::new(center.x + h, center.y + h - 2.0));
            let front = Rect::from_min_max(Pos2::new(center.x - h, center.y - h + 2.0), Pos2::new(center.x + h - 2.0, center.y + h));
            painter.rect_stroke(back, CornerRadius::same(1), s, egui::StrokeKind::Middle);
            painter.rect_filled(front, CornerRadius::same(1), p.label.gamma_multiply(alpha));
            painter.rect_stroke(front, CornerRadius::same(1), s, egui::StrokeKind::Middle);
        }
        Btn::Maximize => {
            let sq = Rect::from_center_size(center, Vec2::splat(h * 2.0));
            painter.rect_stroke(sq, CornerRadius::same(1), s, egui::StrokeKind::Middle);
        }
        Btn::Close => {
            painter.line_segment([Pos2::new(center.x - h, center.y - h), Pos2::new(center.x + h, center.y + h)], s);
            painter.line_segment([Pos2::new(center.x + h, center.y - h), Pos2::new(center.x - h, center.y + h)], s);
        }
    }
    let label = match b {
        Btn::Minimize => "Minimize",
        Btn::Maximize if max => "Restore",
        Btn::Maximize => "Maximize",
        Btn::Close => "Close",
    };
    if resp.clicked() {
        let ctx = ui.ctx();
        match b {
            Btn::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            Btn::Maximize => ctx.send_viewport_cmd(ViewportCommand::Maximized(!max)),
            Btn::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }
    resp.on_hover_text(label);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(parse_layout(":minimize,maximize,close"), (vec![], vec![Btn::Minimize, Btn::Maximize, Btn::Close]));
        assert_eq!(parse_layout("appmenu:close"), (vec![], vec![Btn::Close]));
        assert_eq!(parse_layout("close,minimize:appmenu"), (vec![Btn::Close, Btn::Minimize], vec![]));
        assert_eq!(parse_layout("close"), (vec![], vec![Btn::Close]));
    }
}
