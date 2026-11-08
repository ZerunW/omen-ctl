//! omenctl-gui: desktop front end for omenctld (runs as a normal user in the
//! omenctl group). All changes go through the service; nothing here touches sysfs.

mod edit;
mod theme;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use omenctl_core::curve::{FanCurve, LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theme::{bold, font, ButtonKind, Palette};

// ---------------------------------------------------------------- worker thread

#[derive(Default)]
struct Shared {
    status: Option<Status>,
    /// Set while the service cannot be reached.
    conn_err: Option<String>,
    /// Last refused request (shown for a few seconds).
    last_err: Option<(String, Instant)>,
    /// Curves from the service (Some after a GetCurves answer).
    curves: Option<(FanCurve, FanCurve)>,
    /// Bumped on every curves answer so the editor knows to reload.
    curves_gen: u64,
}

/// Talks to the service: polls status every second and runs queued requests.
fn worker(socket: String, rx: Receiver<Request>, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let mut client: Option<Client> = None;
    let mut pending: Option<Request> = None;
    loop {
        let req = match pending.take() {
            Some(r) => r,
            None => match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => Request::Status,
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        if client.is_none() {
            match Client::connect(&socket) {
                Ok(c) => {
                    client = Some(c);
                    // fetch the curves first after every (re)connect
                    if !matches!(req, Request::GetCurves) {
                        pending = Some(req);
                        continue_with(&mut client, Request::GetCurves, &shared, &ctx, &mut pending);
                        continue;
                    }
                }
                Err(e) => {
                    {
                        let mut s = shared.lock().unwrap();
                        s.conn_err = Some(e);
                        s.status = None;
                    }
                    ctx.request_repaint();
                    if !matches!(req, Request::Status) {
                        pending = Some(req); // retry user requests after reconnecting
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        continue_with(&mut client, req, &shared, &ctx, &mut pending);
    }
}

fn continue_with(
    client: &mut Option<Client>,
    req: Request,
    shared: &Arc<Mutex<Shared>>,
    ctx: &egui::Context,
    pending: &mut Option<Request>,
) {
    let Some(c) = client.as_mut() else { return };
    let is_set_curves = matches!(req, Request::SetCurves { .. });
    let r = c.call(&req);
    {
        let mut s = shared.lock().unwrap();
        match r {
            Ok(Response::Ok { status }) => {
                s.status = Some(status);
                s.conn_err = None;
                if is_set_curves && pending.is_none() {
                    *pending = Some(Request::GetCurves);
                }
            }
            Ok(Response::Curves { cpu, gpu }) => {
                s.curves = Some((cpu, gpu));
                s.curves_gen += 1;
                s.conn_err = None;
            }
            Ok(Response::Error { message }) => s.last_err = Some((message, Instant::now())),
            Err(e) => {
                s.conn_err = Some(e);
                s.status = None;
                *client = None;
            }
        }
    }
    ctx.request_repaint();
}

// ---------------------------------------------------------------- app

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Cpu,
    Gpu,
}

struct App {
    tx: Sender<Request>,
    shared: Arc<Mutex<Shared>>,
    /// Curves being edited (None until the service answered).
    edit: Option<(FanCurve, FanCurve)>,
    edit_gen: u64,
    dirty: bool,
    which: Which,
    dragging: Option<usize>,
    /// Manual speed slider value (fan level, 100 rpm).
    manual_level: u8,
    /// True while the user is moving the slider (don't overwrite it from status).
    manual_touched: bool,
    curve_open: bool,
}

const MODES: [Mode; 3] = [Mode::PowerSaver, Mode::Balanced, Mode::Unleashed];

fn rpm(level: u8) -> String {
    format!("{} rpm", level as u32 * 100)
}

fn mode_caption(m: Mode) -> &'static str {
    match m {
        Mode::PowerSaver => "Lowest power and heat. Good for battery and light work.",
        Mode::Balanced => "The system's standard power limits.",
        Mode::Unleashed => "CPU 90 W sustained, 130 W boost. GPU up to 115 W. Fans are always software controlled.",
    }
}

/// Alerts that mean something is wrong (red) rather than a caution (orange).
fn is_severe(a: &str) -> bool {
    a.contains("fault") || a.contains("overheat") || a.contains("unavailable")
}

impl App {
    fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    fn header(&self, ui: &mut egui::Ui, p: &Palette, connected: bool) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("OMEN Control").font(bold(26.0)).color(p.label));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (txt, col) = if connected { ("Connected", p.green) } else { ("Offline", p.red) };
                ui.label(egui::RichText::new(txt).font(font(12.0)).color(p.secondary));
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, col);
            });
        });
        ui.label(egui::RichText::new("HP OMEN 16  ·  board 8D3F").font(font(12.0)).color(p.secondary));
        ui.add_space(6.0);
    }

    fn performance(&self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        theme::section_title(ui, p, "Performance");
        theme::card(ui, p, |ui| {
            let sel = MODES.iter().position(|m| *m == st.mode);
            let labels: Vec<&str> = MODES.iter().map(|m| m.label()).collect();
            let w = ui.available_width();
            if let Some(i) = theme::segmented(ui, p, "mode", &labels, sel, &[true; 3], w) {
                self.send(Request::SetMode { mode: MODES[i] });
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(mode_caption(st.mode)).font(font(12.0)).color(p.secondary));
            if st.mode != st.effective_mode {
                ui.label(
                    egui::RichText::new(format!("Temporarily running as {} while protection is active.", st.effective_mode.label()))
                        .font(font(12.0))
                        .color(p.orange),
                );
            }
        });
    }

    fn live(&self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        theme::section_title(ui, p, "Live");
        let temp = |t: Option<f32>| t.map_or("–".into(), |t| format!("{t:.0}°"));
        let watts = |w: Option<f32>| w.map_or("–".into(), |w| format!("{w:.1} W"));
        let tiles: [(&str, String, String); 4] = [
            ("CPU", temp(st.cpu_temp), watts(st.cpu_power_w)),
            (
                "GPU",
                st.gpu_temp.map_or("Asleep".into(), |t| format!("{t:.0}°")),
                if st.gpu_temp.is_some() { watts(st.gpu_power_w) } else { "Not in use".into() },
            ),
            ("CPU Fan", st.fan_rpm[0].map_or("–".into(), |r| r.to_string()), format!("rpm  ·  target {}", st.fan_target[0] as u32 * 100)),
            ("GPU Fan", st.fan_rpm[1].map_or("–".into(), |r| r.to_string()), format!("rpm  ·  target {}", st.fan_target[1] as u32 * 100)),
        ];
        ui.columns(4, |cols| {
            for (ui, (title, value, sub)) in cols.iter_mut().zip(tiles) {
                theme::card(ui, p, |ui| {
                    ui.label(egui::RichText::new(title).font(font(12.0)).color(p.secondary));
                    ui.label(egui::RichText::new(value).font(bold(26.0)).color(p.label));
                    ui.label(egui::RichText::new(sub).font(font(11.0)).color(p.secondary));
                });
            }
        });
        let power = st.ac_online.map_or("", |a| if a { "On AC power" } else { "On battery" });
        if !power.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(egui::RichText::new(power).font(font(11.0)).color(p.secondary));
            });
        }
    }

    fn fans(&mut self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        theme::section_title(ui, p, "Fans");
        let unleashed = st.effective_mode == Mode::Unleashed;
        if let FanMode::Fixed(l) = st.fan {
            if !self.manual_touched {
                self.manual_level = l;
            }
        }
        theme::card(ui, p, |ui| {
            // row 1: fan control mode
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Fan Control").font(font(13.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let sel = match st.fan {
                        FanMode::Auto => 0,
                        FanMode::Curve => 1,
                        FanMode::Fixed(_) => 2,
                        FanMode::Max => 3,
                    };
                    let w = ui.available_width().min(340.0);
                    let choice = theme::segmented(ui, p, "fan", &["Auto", "Curve", "Manual", "Max"], Some(sel), &[!unleashed, true, true, true], w);
                    match choice {
                        Some(0) => self.send(Request::SetFan { fan: FanMode::Auto }),
                        Some(1) => self.send(Request::SetFan { fan: FanMode::Curve }),
                        Some(2) => self.send(Request::SetFan { fan: FanMode::Fixed(self.manual_level) }),
                        Some(3) => self.send(Request::SetFan { fan: FanMode::Max }),
                        _ => {}
                    }
                });
            });
            let caption = match st.fan {
                FanMode::Auto => "The BIOS controls the fans.",
                FanMode::Curve => "Fans follow the fan curve for the hotter of CPU and GPU.",
                FanMode::Fixed(_) => "Both fans run at the manual speed below.",
                FanMode::Max => "Both fans at full speed.",
            };
            ui.label(egui::RichText::new(caption).font(font(12.0)).color(p.secondary));
            if unleashed {
                ui.label(egui::RichText::new("Auto is not available in Unleashed mode.").font(font(12.0)).color(p.secondary));
            }

            theme::separator(ui, p);

            // row 2: manual speed
            let rec = st.fan_recommended;
            let rec_max = rec[0].max(rec[1]).max(LEVEL_MIN);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Manual Speed").font(font(13.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_sized(
                        Vec2::new(80.0, 20.0),
                        egui::Label::new(egui::RichText::new(rpm(self.manual_level)).font(bold(13.0))),
                    );
                    let w = ui.available_width().min(340.0);
                    let (holding, commit) =
                        theme::level_slider(ui, p, &mut self.manual_level, (LEVEL_MIN, LEVEL_MAX), Some(rec_max), w);
                    self.manual_touched = holding;
                    if commit {
                        self.manual_touched = false;
                        self.send(Request::SetFan { fan: FanMode::Fixed(self.manual_level) });
                    }
                });
            });
            let t = st.cpu_temp.map(|c| st.gpu_temp.map_or(c, |g| c.max(g)));
            let at = t.map_or(String::new(), |t| format!(" at {t:.0} °C"));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 14.0), Sense::hover());
                ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(2.0, p.orange));
                ui.label(
                    egui::RichText::new(format!("Recommended{at}: CPU fan {}, GPU fan {}", rpm(rec[0]), rpm(rec[1])))
                        .font(font(12.0))
                        .color(p.secondary),
                );
            });
            let showing_manual = matches!(st.fan, FanMode::Fixed(_)) || self.manual_touched;
            if showing_manual && self.manual_level < rec_max {
                ui.add_space(4.0);
                theme::banner(
                    ui,
                    p,
                    p.orange,
                    "Below recommended.",
                    &format!(
                        "{} is lower than the fan curve suggests for the current temperature ({}). The system may run hotter. Protection still takes over at high temperatures.",
                        rpm(self.manual_level),
                        rpm(rec_max)
                    ),
                );
            }
        });
    }

    fn curve(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>) {
        ui.add_space(14.0);
        theme::card(ui, p, |ui| {
            // disclosure row
            let (row, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click());
            if resp.clicked() {
                self.curve_open = !self.curve_open;
            }
            let open = ui.ctx().animate_bool_with_time(egui::Id::unique("curve_open"), self.curve_open, 0.18);
            let painter = ui.painter();
            theme::chevron(painter, Pos2::new(row.left() + 6.0, row.center().y), open, p.secondary);
            painter.text(Pos2::new(row.left() + 20.0, row.center().y), egui::Align2::LEFT_CENTER, "Fan Curve", bold(13.0), p.label);
            let right = if self.dirty { "Unsaved changes" } else { "Used in Curve mode  ·  default: BIOS table" };
            painter.text(
                Pos2::new(row.right(), row.center().y),
                egui::Align2::RIGHT_CENTER,
                right,
                font(12.0),
                if self.dirty { p.orange } else { p.secondary },
            );
            if open <= 0.0 {
                return;
            }
            ui.add_space(8.0);
            self.curve_body(ui, p, st);
        });
    }

    fn curve_body(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>) {
        let Some((cpu, gpu)) = self.edit.as_mut() else {
            ui.label(egui::RichText::new("Loading…").color(p.secondary));
            return;
        };
        let sel = if self.which == Which::Cpu { 0 } else { 1 };
        ui.horizontal(|ui| {
            if let Some(i) = theme::segmented(ui, p, "which", &["CPU Fan", "GPU Fan"], Some(sel), &[true, true], 200.0) {
                self.which = if i == 0 { Which::Cpu } else { Which::Gpu };
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Drag the points. X: hotter of CPU and GPU. Y: fan speed.").font(font(12.0)).color(p.secondary));
        });
        ui.add_space(6.0);
        let curve = if self.which == Which::Cpu { cpu } else { gpu };

        let size = Vec2::new(ui.available_width(), 260.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let plot = Rect::from_min_max(rect.min + Vec2::new(46.0, 10.0), rect.max - Vec2::new(10.0, 24.0));
        let to_screen = |t: f32, l: f32| {
            Pos2::new(
                plot.left() + (t - edit::T_MIN) / (edit::T_MAX - edit::T_MIN) * plot.width(),
                plot.bottom() - (l - LEVEL_MIN as f32) / (LEVEL_MAX - LEVEL_MIN) as f32 * plot.height(),
            )
        };
        let from_screen = |q: Pos2| {
            (
                edit::T_MIN + (q.x - plot.left()) / plot.width() * (edit::T_MAX - edit::T_MIN),
                LEVEL_MIN as f32 + (plot.bottom() - q.y) / plot.height() * (LEVEL_MAX - LEVEL_MIN) as f32,
            )
        };
        let painter = ui.painter_at(rect);
        let grid = Stroke::new(1.0, p.separator);
        for t in (20..=100).step_by(10) {
            let x = to_screen(t as f32, 0.0).x;
            painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], grid);
            painter.text(Pos2::new(x, plot.bottom() + 6.0), egui::Align2::CENTER_TOP, format!("{t}°"), font(11.0), p.secondary);
        }
        for l in [18u8, 25, 30, 35, 40, 45, 52] {
            let y = to_screen(0.0, l as f32).y;
            painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], grid);
            painter.text(Pos2::new(plot.left() - 8.0, y), egui::Align2::RIGHT_CENTER, format!("{}", l as u32 * 100), font(11.0), p.secondary);
        }

        // area under the curve (convex quads) and the line
        let mut pts = vec![(edit::T_MIN, curve.points[0].level as f32)];
        pts.extend(curve.points.iter().map(|q| (q.temp, q.level as f32)));
        pts.push((edit::T_MAX, curve.points.last().unwrap().level as f32));
        let fill = p.blue.gamma_multiply(if p.dark { 0.22 } else { 0.12 });
        for w in pts.windows(2) {
            let a = to_screen(w[0].0, w[0].1);
            let b = to_screen(w[1].0, w[1].1);
            painter.add(egui::Shape::convex_polygon(
                vec![a, b, Pos2::new(b.x, plot.bottom()), Pos2::new(a.x, plot.bottom())],
                fill,
                Stroke::NONE,
            ));
        }
        let line: Vec<Pos2> = pts.iter().map(|&(t, l)| to_screen(t, l)).collect();
        painter.add(egui::Shape::line(line, Stroke::new(2.5, p.blue)));

        // current temperature
        if let Some(st) = st {
            if let Some(t) = st.cpu_temp.map(|c| st.gpu_temp.map_or(c, |g| c.max(g))) {
                let x = to_screen(t.clamp(edit::T_MIN, edit::T_MAX), 0.0).x;
                painter.extend(egui::Shape::dashed_line(&[Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.5, p.orange), 4.0, 3.0));
                painter.text(Pos2::new(x + 5.0, plot.top() + 2.0), egui::Align2::LEFT_TOP, format!("Now {t:.0}°"), font(11.0), p.orange);
            }
        }

        // dragging
        if resp.drag_started() {
            // where the button went down, not where the drag threshold was crossed
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            if let Some(q) = origin {
                self.dragging = curve
                    .points
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i, to_screen(c.temp, c.level as f32).distance(q)))
                    .filter(|(_, d)| *d < 14.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i);
            }
        }
        if let (Some(i), Some(q)) = (self.dragging, resp.interact_pointer_pos()) {
            if resp.dragged() {
                let (t, l) = from_screen(q);
                edit::drag_point(&mut curve.points, i, t, l);
                self.dirty = true;
            }
        }
        if resp.drag_stopped() {
            self.dragging = None;
        }
        let hover = resp.hover_pos();
        let mut tip = None;
        for (i, c) in curve.points.iter().enumerate() {
            let at = to_screen(c.temp, c.level as f32);
            let hot = self.dragging == Some(i) || (self.dragging.is_none() && hover.is_some_and(|h| h.distance(at) < 14.0));
            let r = if hot { 7.0 } else { 5.5 };
            if !p.dark {
                painter.circle_filled(at + Vec2::new(0.0, 1.0), r + 0.5, p.shadow);
            }
            painter.circle_filled(at, r, Color32::WHITE);
            painter.circle_stroke(at, r, Stroke::new(2.0, p.blue));
            if hot {
                tip = Some((at, format!("{:.1}°  →  {}", c.temp, rpm(c.level))));
            }
        }
        if let Some((at, text)) = tip {
            let g = painter.layout_no_wrap(text, font(12.0), Color32::WHITE);
            let size = g.size() + Vec2::new(16.0, 8.0);
            let mut r = Rect::from_min_size(Pos2::new(at.x - size.x / 2.0, at.y - size.y - 12.0), size);
            r = r.translate(Vec2::new((plot.left() - r.left()).max(0.0) + (plot.right() - r.right()).min(0.0), (rect.top() - r.top()).max(0.0)));
            painter.rect_filled(r, 6.0, Color32::from_black_alpha(200));
            painter.galley(r.min + Vec2::new(8.0, 4.0), g, Color32::WHITE);
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if theme::button(ui, p, "Restore Defaults", ButtonKind::Secondary, true) {
                self.edit = Some((FanCurve::bios_default_cpu(), FanCurve::bios_default_gpu()));
                self.dirty = true;
            }
            if theme::button(ui, p, "Revert", ButtonKind::Secondary, self.dirty) {
                self.edit_gen = 0; // reload the service's copy on the next frame
                self.dirty = false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::button(ui, p, "Apply", ButtonKind::Primary, self.dirty) {
                    if let Some((c, g)) = &self.edit {
                        let _ = self.tx.send(Request::SetCurves { cpu: c.clone(), gpu: g.clone() });
                        self.dirty = false;
                    }
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = theme::palette(ui.ctx());
        let (status, conn_err, last_err, curves, gen) = {
            let mut s = self.shared.lock().unwrap();
            if s.last_err.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(8)) {
                s.last_err = None;
            }
            (s.status.clone(), s.conn_err.clone(), s.last_err.as_ref().map(|e| e.0.clone()), s.curves.clone(), s.curves_gen)
        };
        // take the service's curves unless the user is editing
        if gen != self.edit_gen && !self.dirty && self.dragging.is_none() {
            if let Some(c) = curves {
                self.edit = Some(c);
                self.edit_gen = gen;
            }
        }
        if last_err.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin::symmetric(24, 20)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let w = ui.available_width().min(780.0);
                    let pad = (ui.available_width() - w) / 2.0;
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(w);
                            self.header(ui, &p, status.is_some());
                            if let Some(e) = &conn_err {
                                theme::banner(ui, &p, p.red, "Service unavailable.", e);
                            }
                            if let Some(e) = &last_err {
                                theme::banner(ui, &p, p.red, "Request refused.", e);
                            }
                            if let Some(st) = &status {
                                // the manual-speed warning is shown inline next to the slider
                                for a in st.alerts.iter().filter(|a| !a.contains("below the recommended")) {
                                    let (col, title) = if is_severe(a) { (p.red, "Warning.") } else { (p.orange, "Note.") };
                                    theme::banner(ui, &p, col, title, a);
                                }
                                self.performance(ui, &p, st);
                                self.live(ui, &p, st);
                                self.fans(ui, &p, st);
                            } else if conn_err.is_none() {
                                ui.label(egui::RichText::new("Connecting to the omenctl service…").color(p.secondary));
                            }
                            self.curve(ui, &p, status.as_ref());
                            ui.add_space(12.0);
                        });
                    });
                });
            });
    }
}

const USAGE: &str = "usage: omenctl-gui [--socket PATH] [--theme system|light|dark]";

fn main() -> eframe::Result {
    let mut socket = SOCKET_PATH.to_string();
    let mut theme_pref = egui::ThemePreference::System;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--socket", Some(s)) => socket = s,
            ("--theme", Some(t)) if t == "light" => theme_pref = egui::ThemePreference::Light,
            ("--theme", Some(t)) if t == "dark" => theme_pref = egui::ThemePreference::Dark,
            ("--theme", Some(t)) if t == "system" => {}
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OMEN Control")
            .with_app_id("omenctl-gui")
            .with_inner_size([860.0, 760.0])
            .with_min_inner_size([680.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "omenctl-gui",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            cc.egui_ctx.set_theme(theme_pref);
            let shared = Arc::new(Mutex::new(Shared::default()));
            let (tx, rx) = channel();
            let ctx = cc.egui_ctx.clone();
            let sh = shared.clone();
            std::thread::spawn(move || worker(socket, rx, sh, ctx));
            Ok(Box::new(App {
                tx,
                shared,
                edit: None,
                edit_gen: 0,
                dirty: false,
                which: Which::Cpu,
                dragging: None,
                manual_level: 30,
                manual_touched: false,
                curve_open: false,
            }))
        }),
    )
}
