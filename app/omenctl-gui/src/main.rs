//! omenctl-gui: desktop front end for omenctld (runs as a normal user in the
//! omenctl group). All changes go through the service; nothing here touches sysfs.

mod edit;
mod theme;

use eframe::egui::{self, Pos2, Rect, Sense, Stroke, Vec2};
use omenctl_core::curve::{FanCurve, LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theme::{font, medium, ButtonKind, Palette, CONTROL_H};

/// Seconds of history shown in the sparklines (one sample per status poll).
const HISTORY: usize = 60;
const LABEL_W: f32 = 116.0;

// ---------------------------------------------------------------- worker thread

#[derive(Clone, Copy, Default)]
struct Sample {
    cpu_t: Option<f32>,
    gpu_t: Option<f32>,
    rpm: [Option<f32>; 2],
}

#[derive(Default)]
struct Shared {
    status: Option<Status>,
    history: VecDeque<Sample>,
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
    let mut queue: VecDeque<Request> = VecDeque::new();
    loop {
        let req = match queue.pop_front() {
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
                    // after every (re)connect: fetch the curves before anything else
                    if !matches!(req, Request::GetCurves) {
                        queue.push_front(req);
                        queue.push_front(Request::GetCurves);
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
                        queue.push_front(req); // retry user requests after reconnecting
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let poll = matches!(req, Request::Status);
        let set_curves = matches!(req, Request::SetCurves { .. });
        let r = client.as_mut().unwrap().call(&req);
        {
            let mut s = shared.lock().unwrap();
            match r {
                Ok(Response::Ok { status }) => {
                    if poll {
                        s.history.push_back(Sample {
                            cpu_t: status.cpu_temp,
                            gpu_t: status.gpu_temp,
                            rpm: status.fan_rpm.map(|r| r.map(|v| v as f32)),
                        });
                        while s.history.len() > HISTORY {
                            s.history.pop_front();
                        }
                    }
                    s.status = Some(status);
                    s.conn_err = None;
                    if set_curves {
                        queue.push_back(Request::GetCurves);
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
                    client = None;
                }
            }
        }
        ctx.request_repaint();
    }
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
    /// True while the user holds the slider (don't overwrite it from status).
    manual_touched: bool,
    curve_open: bool,
}

const MODES: [Mode; 3] = [Mode::PowerSaver, Mode::Balanced, Mode::Unleashed];

const INFO_MODE: &str = "Power Saver: lowest power and heat.\n\
Balanced: the system's standard power limits.\n\
Unleashed: CPU 90 W sustained and 130 W boost, GPU up to 115 W. Fans are always software controlled in this mode.\n\n\
If the CPU or GPU stays at 95 °C, Unleashed pauses until it cools below 80 °C.";
const INFO_LIVE: &str = "Last 60 seconds. GPU readings appear only while the GPU is awake, so monitoring never wakes it.";
const INFO_FANS: &str = "Auto: the BIOS controls the fans (not available in Unleashed).\n\
Curve: fans follow the fan curve for the hotter of CPU and GPU.\n\
Manual: both fans at a fixed speed.\n\
Max: both fans at full speed.\n\n\
At 92 °C both fans always go to full speed.";
const INFO_MANUAL: &str = "Fixed speed for both fans. Moving the slider switches to Manual.\n\
The tick marks what the fan curve recommends at the current temperature.";
const INFO_CURVE: &str = "Drag a point to change it. Horizontal: the hotter of CPU and GPU. Vertical: fan speed.\n\
Changes take effect after Apply. Reset restores the BIOS fan table.";

fn rpm(level: u8) -> String {
    format!("{} rpm", level as u32 * 100)
}

/// Alerts that mean something is wrong (red) rather than a caution (amber).
fn is_severe(a: &str) -> bool {
    a.contains("fault") || a.contains("overheat") || a.contains("unavailable")
}

fn hottest(st: &Status) -> Option<f32> {
    st.cpu_temp.map(|c| st.gpu_temp.map_or(c, |g| c.max(g)))
}

impl App {
    fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    fn row<R>(ui: &mut egui::Ui, p: &Palette, label: &str, info: &str, add: impl FnOnce(&mut egui::Ui, f32) -> R) -> R {
        ui.horizontal(|ui| {
            theme::row_label(ui, p, label, LABEL_W);
            let w = ui.available_width() - 16.0 - ui.spacing().item_spacing.x;
            let r = add(ui, w);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::info(ui, p, info));
            r
        })
        .inner
    }

    fn mode_row(&self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        let clicked = Self::row(ui, p, "Mode", INFO_MODE, |ui, w| {
            let labels: Vec<&str> = MODES.iter().map(|m| m.label()).collect();
            let sel = MODES.iter().position(|m| *m == st.mode);
            theme::segmented(ui, p, "mode", &labels, sel, &[true; 3], w.min(420.0))
        });
        if let Some(i) = clicked {
            self.send(Request::SetMode { mode: MODES[i] });
        }
    }

    fn live(&self, ui: &mut egui::Ui, p: &Palette, st: &Status, hist: &[Sample]) {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 92.0), Sense::hover());
        let col_w = (w - 24.0) / 3.0;
        let painter = ui.painter_at(rect);
        let watts = |v: Option<f32>| v.map_or(String::new(), |v| format!("{v:.1} W"));
        let temp = |v: Option<f32>| v.map_or("–".to_string(), |v| format!("{v:.0}°"));
        let fan = |v: Option<u32>| v.map_or("–".to_string(), |v| v.to_string());
        let cols: [(&str, String, String); 3] = [
            ("CPU", temp(st.cpu_temp), watts(st.cpu_power_w)),
            (
                "GPU",
                st.gpu_temp.map_or("Idle".into(), |t| format!("{t:.0}°")),
                if st.gpu_temp.is_some() { watts(st.gpu_power_w) } else { String::new() },
            ),
            ("Fans", format!("{} · {}", fan(st.fan_rpm[0]), fan(st.fan_rpm[1])), "rpm".into()),
        ];
        for (i, (title, value, unit)) in cols.iter().enumerate() {
            let x = rect.left() + i as f32 * (col_w + 12.0);
            painter.text(Pos2::new(x, rect.top() + 10.0), egui::Align2::LEFT_TOP, *title, font(11.5), p.secondary);
            // value and unit on one baseline
            let mut job = egui::text::LayoutJob::default();
            job.append(value, 0.0, egui::TextFormat { font_id: medium(22.0), color: p.label, valign: egui::Align::BOTTOM, ..Default::default() });
            if !unit.is_empty() {
                job.append(unit, 6.0, egui::TextFormat { font_id: font(12.0), color: p.secondary, valign: egui::Align::BOTTOM, ..Default::default() });
            }
            let g = painter.layout_job(job);
            painter.galley(Pos2::new(x, rect.top() + 26.0), g, p.label);
            let spark = Rect::from_min_size(Pos2::new(x, rect.top() + 60.0), Vec2::new(col_w, 24.0));
            let s = Stroke::new(1.25, p.secondary);
            match i {
                0 => theme::sparkline(&painter, spark, &hist.iter().map(|h| h.cpu_t).collect::<Vec<_>>(), (30.0, 100.0), HISTORY, s),
                1 => theme::sparkline(&painter, spark, &hist.iter().map(|h| h.gpu_t).collect::<Vec<_>>(), (30.0, 100.0), HISTORY, s),
                _ => {
                    let faint = Stroke::new(1.25, p.tertiary);
                    theme::sparkline(&painter, spark, &hist.iter().map(|h| h.rpm[1]).collect::<Vec<_>>(), (0.0, 5400.0), HISTORY, faint);
                    theme::sparkline(&painter, spark, &hist.iter().map(|h| h.rpm[0]).collect::<Vec<_>>(), (0.0, 5400.0), HISTORY, s);
                }
            }
        }
        // (i) for the strip, top right
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(rect.right() - 16.0, rect.top() + 2.0), Vec2::new(16.0, 20.0))));
        theme::info(&mut child, p, INFO_LIVE);
    }

    fn fan_rows(&mut self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        let unleashed = st.effective_mode == Mode::Unleashed;
        if let FanMode::Fixed(l) = st.fan {
            if !self.manual_touched {
                self.manual_level = l;
            }
        }
        let sel = match st.fan {
            FanMode::Auto => 0,
            FanMode::Curve => 1,
            FanMode::Fixed(_) => 2,
            FanMode::Max => 3,
        };
        let choice = Self::row(ui, p, "Fan control", INFO_FANS, |ui, w| {
            theme::segmented(ui, p, "fan", &["Auto", "Curve", "Manual", "Max"], Some(sel), &[!unleashed, true, true, true], w.min(420.0))
        });
        match choice {
            Some(0) => self.send(Request::SetFan { fan: FanMode::Auto }),
            Some(1) => self.send(Request::SetFan { fan: FanMode::Curve }),
            Some(2) => self.send(Request::SetFan { fan: FanMode::Fixed(self.manual_level) }),
            Some(3) => self.send(Request::SetFan { fan: FanMode::Max }),
            _ => {}
        }

        ui.add_space(4.0);
        let rec = st.fan_recommended[0].max(st.fan_recommended[1]).max(LEVEL_MIN);
        let mut level = self.manual_level;
        let (holding, commit) = Self::row(ui, p, "Manual speed", INFO_MANUAL, |ui, w| {
            let sw = (w - 84.0).min(420.0 - 84.0);
            let r = theme::level_slider(ui, p, &mut level, (LEVEL_MIN, LEVEL_MAX), rec, sw);
            let (vr, _) = ui.allocate_exact_size(Vec2::new(76.0, CONTROL_H), Sense::hover());
            ui.painter().text(vr.right_center(), egui::Align2::RIGHT_CENTER, rpm(level), font(13.0), p.label);
            r
        });
        self.manual_level = level;
        self.manual_touched = holding;
        if commit {
            self.send(Request::SetFan { fan: FanMode::Fixed(self.manual_level) });
        }
        let manual = matches!(st.fan, FanMode::Fixed(_)) || holding;
        if manual && self.manual_level < rec {
            ui.horizontal(|ui| {
                ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
                let at = hottest(st).map_or(String::new(), |t| format!(" at {t:.0}°"));
                theme::notice(ui, p.amber, &format!("Below recommended: {}{at}", rpm(rec)));
            });
        }
    }

    fn curve(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>) {
        let (row, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
        if resp.clicked() {
            self.curve_open = !self.curve_open;
        }
        let open = ui.ctx().animate_bool_with_time(egui::Id::unique("curve_open"), self.curve_open, 0.16);
        let painter = ui.painter();
        let c = if resp.hovered() { p.label } else { p.secondary };
        theme::chevron(painter, Pos2::new(row.left() + 4.0, row.center().y), open, c);
        painter.text(Pos2::new(row.left() + 16.0, row.center().y), egui::Align2::LEFT_CENTER, "Fan curve", font(13.0), c);
        if self.dirty {
            painter.text(row.right_center(), egui::Align2::RIGHT_CENTER, "Edited", font(12.0), p.amber);
        }
        if open <= 0.0 {
            return;
        }
        ui.add_space(4.0);
        self.curve_body(ui, p, st);
    }

    fn curve_body(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>) {
        let Some((cpu, gpu)) = self.edit.as_mut() else {
            ui.label(egui::RichText::new("Loading…").color(p.secondary));
            return;
        };
        let sel = if self.which == Which::Cpu { 0 } else { 1 };
        let mut pick = None;
        ui.horizontal(|ui| {
            pick = theme::segmented(ui, p, "which", &["CPU fan", "GPU fan"], Some(sel), &[true, true], 180.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::info(ui, p, INFO_CURVE));
        });
        if let Some(i) = pick {
            self.which = if i == 0 { Which::Cpu } else { Which::Gpu };
        }
        ui.add_space(6.0);
        let curve = if self.which == Which::Cpu { cpu } else { gpu };

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 230.0), Sense::click_and_drag());
        let plot = Rect::from_min_max(rect.min + Vec2::new(40.0, 8.0), rect.max - Vec2::new(16.0, 22.0));
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
        for l in [18u8, 30, 40, 52] {
            let y = to_screen(0.0, l as f32).y;
            painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0, p.hairline));
            painter.text(Pos2::new(plot.left() - 8.0, y), egui::Align2::RIGHT_CENTER, format!("{}", l as u32 * 100), font(10.5), p.tertiary);
        }
        for t in (20..=100).step_by(20) {
            let x = to_screen(t as f32, 0.0).x;
            painter.text(Pos2::new(x, plot.bottom() + 6.0), egui::Align2::CENTER_TOP, format!("{t}°"), font(10.5), p.tertiary);
        }
        if let Some(t) = st.and_then(hottest) {
            let x = to_screen(t.clamp(edit::T_MIN, edit::T_MAX), 0.0).x;
            painter.extend(egui::Shape::dashed_line(&[Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, p.tertiary), 3.0, 3.0));
            painter.text(Pos2::new(x + 4.0, plot.top()), egui::Align2::LEFT_TOP, format!("{t:.0}°"), font(10.5), p.secondary);
        }
        let mut line = vec![to_screen(edit::T_MIN, curve.points[0].level as f32)];
        line.extend(curve.points.iter().map(|q| to_screen(q.temp, q.level as f32)));
        line.push(to_screen(edit::T_MAX, curve.points.last().unwrap().level as f32));
        painter.add(egui::Shape::line(line, Stroke::new(1.5, p.label)));

        if resp.drag_started() {
            // where the button went down, not where the drag threshold was crossed
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            if let Some(q) = origin {
                self.dragging = curve
                    .points
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i, to_screen(c.temp, c.level as f32).distance(q)))
                    .filter(|(_, d)| *d < 12.0)
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
            let hot = self.dragging == Some(i) || (self.dragging.is_none() && hover.is_some_and(|h| h.distance(at) < 12.0));
            let r = if hot { 4.5 } else { 3.5 };
            painter.circle_filled(at, r, p.bg);
            painter.circle_stroke(at, r, Stroke::new(1.25, p.label));
            if hot {
                tip = Some((at, format!("{:.1}°  {}", c.temp, rpm(c.level))));
            }
        }
        if let Some((at, text)) = tip {
            let g = painter.layout_no_wrap(text, font(11.5), p.bg);
            let size = g.size() + Vec2::new(12.0, 6.0);
            let mut r = Rect::from_min_size(Pos2::new(at.x - size.x / 2.0, at.y - size.y - 10.0), size);
            r = r.translate(Vec2::new((plot.left() - r.left()).max(0.0) + (plot.right() - r.right()).min(0.0), (rect.top() - r.top()).max(0.0)));
            painter.rect_filled(r, 4.0, p.label);
            painter.galley(r.min + Vec2::new(6.0, 3.0), g, p.bg);
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::button(ui, p, "Apply", ButtonKind::Primary, self.dirty) {
                    if let Some((c, g)) = &self.edit {
                        let _ = self.tx.send(Request::SetCurves { cpu: c.clone(), gpu: g.clone() });
                        self.dirty = false;
                    }
                }
                if theme::button(ui, p, "Revert", ButtonKind::Plain, self.dirty) {
                    self.edit_gen = 0; // reload the service's copy on the next frame
                    self.dirty = false;
                }
                if theme::button(ui, p, "Reset", ButtonKind::Plain, true) {
                    self.edit = Some((FanCurve::bios_default_cpu(), FanCurve::bios_default_gpu()));
                    self.dirty = true;
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = theme::palette(ui.ctx());
        let (status, hist, conn_err, last_err, curves, gen) = {
            let mut s = self.shared.lock().unwrap();
            if s.last_err.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(8)) {
                s.last_err = None;
            }
            let hist: Vec<Sample> = s.history.iter().copied().collect();
            (s.status.clone(), hist, s.conn_err.clone(), s.last_err.as_ref().map(|e| e.0.clone()), s.curves.clone(), s.curves_gen)
        };
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
            .frame(egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let w = ui.available_width().min(720.0);
                    let pad = (ui.available_width() - w) / 2.0;
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(w);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("OMEN Control").font(medium(19.0)).color(p.label));
                                if conn_err.is_some() {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new("Offline").font(font(12.0)).color(p.red));
                                    });
                                }
                            });
                            ui.add_space(6.0);
                            if let Some(e) = &conn_err {
                                theme::notice(ui, p.red, e);
                            }
                            if let Some(e) = &last_err {
                                theme::notice(ui, p.red, &format!("Not applied: {e}"));
                            }
                            if let Some(st) = &status {
                                if st.mode != st.effective_mode {
                                    theme::notice(ui, p.amber, &format!("Running {} until temperatures recover", st.effective_mode.label()));
                                }
                                for a in st.alerts.iter().filter(|a| !a.contains("below the recommended") && !a.starts_with("overheat protection")) {
                                    theme::notice(ui, if is_severe(a) { p.red } else { p.amber }, a);
                                }
                            }
                            ui.add_space(12.0);
                            match &status {
                                Some(st) => {
                                    self.mode_row(ui, &p, st);
                                    ui.add_space(14.0);
                                    theme::hairline(ui, &p);
                                    self.live(ui, &p, st, &hist);
                                    ui.add_space(6.0);
                                    theme::hairline(ui, &p);
                                    ui.add_space(14.0);
                                    self.fan_rows(ui, &p, st);
                                    ui.add_space(14.0);
                                    theme::hairline(ui, &p);
                                }
                                None if conn_err.is_none() => {
                                    ui.label(egui::RichText::new("Connecting…").color(p.secondary));
                                }
                                None => {}
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
            .with_inner_size([780.0, 560.0])
            .with_min_inner_size([640.0, 420.0]),
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
