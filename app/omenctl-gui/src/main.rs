//! omenctl-gui: desktop front end for omenctld (runs as a normal user in the
//! omenctl group). All changes go through the daemon; nothing here touches sysfs.

mod edit;

use eframe::egui;
use omenctl_core::curve::{FanCurve, LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- worker thread

#[derive(Default)]
struct Shared {
    status: Option<Status>,
    /// Set while the daemon cannot be reached.
    conn_err: Option<String>,
    /// Last refused request (shown for a few seconds).
    last_err: Option<(String, Instant)>,
    /// Curves from the daemon (Some after a GetCurves/SetCurves answer).
    curves: Option<(FanCurve, FanCurve)>,
    /// Bumped on every curves answer so the editor knows to reload.
    curves_gen: u64,
}

/// Talks to the daemon: polls status every second and runs queued requests.
fn worker(socket: String, rx: Receiver<Request>, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let mut client: Option<Client> = None;
    let mut pending: Option<Request> = Some(Request::GetCurves);
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
                Ok(c) => client = Some(c),
                Err(e) => {
                    let mut s = shared.lock().unwrap();
                    s.conn_err = Some(e);
                    s.status = None;
                    drop(s);
                    ctx.request_repaint();
                    // keep the request (except plain polls) for the next attempt
                    if !matches!(req, Request::Status) {
                        pending = Some(req);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
            pending = Some(Request::GetCurves); // reload curves after (re)connect
        }
        let is_set_curves = matches!(req, Request::SetCurves { .. });
        let r = client.as_mut().unwrap().call(&req);
        let mut s = shared.lock().unwrap();
        match r {
            Ok(Response::Ok { status }) => {
                s.status = Some(status);
                s.conn_err = None;
                if is_set_curves {
                    pending = Some(Request::GetCurves);
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
        drop(s);
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------- fonts

/// egui's built-in fonts have no Chinese glyphs: load a system CJK font.
fn install_cjk_font(ctx: &egui::Context) -> bool {
    // (path, face index). NotoSansCJK .ttc face 2 is Simplified Chinese.
    const CANDIDATES: &[(&str, u32)] = &[
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc", 2),
        ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
        ("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc", 0),
        ("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf", 0),
        ("/usr/share/fonts/truetype/arphic/uming.ttc", 0),
    ];
    for &(path, index) in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            let fam = |family| egui::epaint::text::InsertFontFamily {
                family,
                priority: egui::epaint::text::FontPriority::Lowest,
            };
            ctx.add_font(egui::epaint::text::FontInsert::new(
                "cjk",
                data,
                vec![fam(egui::FontFamily::Proportional), fam(egui::FontFamily::Monospace)],
            ));
            return true;
        }
    }
    eprintln!("omenctl-gui: no CJK font found; install fonts-noto-cjk");
    false
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
    /// Curves being edited (None until the daemon answered).
    edit: Option<(FanCurve, FanCurve)>,
    edit_gen: u64,
    dirty: bool,
    which: Which,
    dragging: Option<usize>,
    fixed_level: u8,
}

impl App {
    fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    fn mode_section(&self, ui: &mut egui::Ui, st: &Status) {
        ui.heading("性能模式");
        ui.horizontal(|ui| {
            for (m, tip) in [
                (Mode::PowerSaver, "低功耗，风扇默认按曲线"),
                (Mode::Balanced, "系统默认档位"),
                (Mode::Unleashed, "CPU 90/130 W，显卡上限 115 W；风扇必须由软件控制"),
            ] {
                let b = ui.add(egui::Button::selectable(st.mode == m, m.label_zh()).min_size(egui::vec2(90.0, 32.0)));
                if b.on_hover_text(tip).clicked() && st.mode != m {
                    self.send(Request::SetMode { mode: m });
                }
            }
        });
        if st.mode != st.effective_mode {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("当前实际运行：{}（保护中）", st.effective_mode.label_zh()),
            );
        }
    }

    fn fan_section(&mut self, ui: &mut egui::Ui, st: &Status) {
        ui.heading("风扇");
        let unleashed = st.effective_mode == Mode::Unleashed;
        ui.horizontal(|ui| {
            let auto = ui.add_enabled(!unleashed, egui::Button::selectable(st.fan == FanMode::Auto, "BIOS 自动"));
            let auto = auto.on_disabled_hover_text("满血模式下不能用 BIOS 自动风扇（EC 会退出满血状态）");
            if auto.clicked() {
                self.send(Request::SetFan { fan: FanMode::Auto });
            }
            if ui.add(egui::Button::selectable(st.fan == FanMode::Curve, "曲线")).clicked() {
                self.send(Request::SetFan { fan: FanMode::Curve });
            }
            let is_fixed = matches!(st.fan, FanMode::Fixed(_));
            if ui.add(egui::Button::selectable(is_fixed, "固定转速")).clicked() {
                self.send(Request::SetFan { fan: FanMode::Fixed(self.fixed_level) });
            }
            if ui.add(egui::Button::selectable(st.fan == FanMode::Max, "最高档")).clicked() {
                self.send(Request::SetFan { fan: FanMode::Max });
            }
        });
        if let FanMode::Fixed(l) = st.fan {
            if self.dragging.is_none() && !ui.ctx().egui_is_using_pointer() {
                self.fixed_level = l;
            }
        }
        ui.horizontal(|ui| {
            ui.label("固定转速：");
            let r = ui.add(
                egui::Slider::new(&mut self.fixed_level, LEVEL_MIN..=LEVEL_MAX)
                    .custom_formatter(|v, _| format!("{} rpm", v as u32 * 100)),
            );
            // apply when the user lets go (or after keyboard change), not on every pixel
            if (r.drag_stopped() || (r.changed() && !r.dragged())) && matches!(st.fan, FanMode::Fixed(_)) {
                self.send(Request::SetFan { fan: FanMode::Fixed(self.fixed_level) });
            }
        });
    }

    fn live_section(&self, ui: &mut egui::Ui, st: &Status) {
        ui.heading("实时数据");
        let f = |v: Option<f32>, unit: &str, dec: usize| v.map_or("—".to_string(), |v| format!("{v:.dec$} {unit}"));
        egui::Grid::new("live").num_columns(3).spacing([24.0, 6.0]).striped(true).show(ui, |ui| {
            ui.label("");
            ui.strong("CPU");
            ui.strong("显卡");
            ui.end_row();
            ui.label("温度");
            ui.label(f(st.cpu_temp, "°C", 0));
            ui.label(st.gpu_temp.map_or("休眠 / 无数据".into(), |t| format!("{t:.0} °C")));
            ui.end_row();
            ui.label("功率");
            ui.label(f(st.cpu_power_w, "W", 1));
            ui.label(f(st.gpu_power_w, "W", 1));
            ui.end_row();
            ui.label("风扇转速");
            ui.label(st.fan_rpm[0].map_or("—".into(), |r| format!("{r} rpm")));
            ui.label(st.fan_rpm[1].map_or("—".into(), |r| format!("{r} rpm")));
            ui.end_row();
            ui.label("风扇目标");
            ui.label(format!("{} rpm", st.fan_target[0] as u32 * 100));
            ui.label(format!("{} rpm", st.fan_target[1] as u32 * 100));
            ui.end_row();
            ui.label("电源");
            ui.label(st.ac_online.map_or("—", |a| if a { "已接通" } else { "电池" }));
            ui.end_row();
        });
        if !st.alerts.is_empty() {
            ui.add_space(6.0);
            for a in &st.alerts {
                ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {a}"));
            }
        }
    }

    fn curve_section(&mut self, ui: &mut egui::Ui, st: Option<&Status>) {
        ui.heading("风扇曲线");
        let Some((cpu, gpu)) = self.edit.as_mut() else {
            ui.label("正在读取曲线…");
            return;
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.which, Which::Cpu, "CPU 风扇");
            ui.selectable_value(&mut self.which, Which::Gpu, "显卡风扇");
            ui.label("（横轴：CPU 和显卡中较高的温度；纵轴：转速）");
        });
        let curve = if self.which == Which::Cpu { cpu } else { gpu };

        let size = egui::vec2(ui.available_width().min(720.0), 300.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let plot = rect.shrink2(egui::vec2(44.0, 22.0));
        let to_screen = |t: f32, l: f32| {
            egui::pos2(
                plot.left() + (t - edit::T_MIN) / (edit::T_MAX - edit::T_MIN) * plot.width(),
                plot.bottom() - (l - LEVEL_MIN as f32) / (LEVEL_MAX - LEVEL_MIN) as f32 * plot.height(),
            )
        };
        let from_screen = |p: egui::Pos2| {
            (
                edit::T_MIN + (p.x - plot.left()) / plot.width() * (edit::T_MAX - edit::T_MIN),
                LEVEL_MIN as f32 + (plot.bottom() - p.y) / plot.height() * (LEVEL_MAX - LEVEL_MIN) as f32,
            )
        };
        let v = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        painter.rect_filled(plot, 2.0, v.extreme_bg_color);
        let grid = egui::Stroke::new(1.0, v.weak_text_color().gamma_multiply(0.3));
        let font = egui::FontId::proportional(11.0);
        for t in (20..=100).step_by(10) {
            let x = to_screen(t as f32, 0.0).x;
            painter.line_segment([egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())], grid);
            painter.text(egui::pos2(x, plot.bottom() + 4.0), egui::Align2::CENTER_TOP, format!("{t}°C"), font.clone(), v.weak_text_color());
        }
        for l in [18u8, 25, 30, 35, 40, 45, 52] {
            let y = to_screen(0.0, l as f32).y;
            painter.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], grid);
            painter.text(egui::pos2(plot.left() - 4.0, y), egui::Align2::RIGHT_CENTER, format!("{}", l as u32 * 100), font.clone(), v.weak_text_color());
        }
        // protection threshold
        if let Some(st) = st {
            let hot = st.cpu_temp.map(|c| st.gpu_temp.map_or(c, |g| c.max(g)));
            if let Some(t) = hot {
                let x = to_screen(t.clamp(edit::T_MIN, edit::T_MAX), 0.0).x;
                painter.line_segment(
                    [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                    egui::Stroke::new(1.5, v.warn_fg_color),
                );
                painter.text(egui::pos2(x + 3.0, plot.top() + 2.0), egui::Align2::LEFT_TOP, format!("当前 {t:.0}°C"), font.clone(), v.warn_fg_color);
            }
        }
        // curve (flat before the first and after the last point)
        let mut line = vec![to_screen(edit::T_MIN, curve.points[0].level as f32)];
        line.extend(curve.points.iter().map(|p| to_screen(p.temp, p.level as f32)));
        line.push(to_screen(edit::T_MAX, curve.points.last().unwrap().level as f32));
        let accent = v.selection.bg_fill;
        painter.add(egui::Shape::line(line, egui::Stroke::new(2.0, accent)));

        // dragging
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.dragging = curve
                    .points
                    .iter()
                    .enumerate()
                    .map(|(i, q)| (i, to_screen(q.temp, q.level as f32).distance(p)))
                    .filter(|(_, d)| *d < 12.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i);
            }
        }
        if let (Some(i), Some(p)) = (self.dragging, resp.interact_pointer_pos()) {
            if resp.dragged() {
                let (t, l) = from_screen(p);
                edit::drag_point(&mut curve.points, i, t, l);
                self.dirty = true;
            }
        }
        if resp.drag_stopped() {
            self.dragging = None;
        }
        let hover = resp.hover_pos();
        for (i, q) in curve.points.iter().enumerate() {
            let c = to_screen(q.temp, q.level as f32);
            let hot = self.dragging == Some(i) || hover.is_some_and(|h| h.distance(c) < 12.0);
            painter.circle_filled(c, if hot { 7.0 } else { 5.0 }, accent);
            if hot {
                painter.text(c + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, format!("{:.1}°C → {} rpm", q.temp, q.level as u32 * 100), font.clone(), v.strong_text_color());
            }
        }

        ui.horizontal(|ui| {
            if ui.add_enabled(self.dirty, egui::Button::new("保存并应用")).clicked() {
                if let Some((c, g)) = &self.edit {
                    let _ = self.tx.send(Request::SetCurves { cpu: c.clone(), gpu: g.clone() });
                    self.dirty = false;
                }
            }
            if ui.add_enabled(self.dirty, egui::Button::new("放弃修改")).clicked() {
                self.edit_gen = 0; // reload from the daemon's copy on the next frame
                self.dirty = false;
            }
            if ui.button("恢复 BIOS 默认曲线").on_hover_text("只改编辑器里的曲线，点“保存并应用”后才生效").clicked() {
                self.edit = Some((FanCurve::bios_default_cpu(), FanCurve::bios_default_gpu()));
                self.dirty = true;
            }
            if self.dirty {
                ui.colored_label(ui.visuals().warn_fg_color, "有未保存的修改");
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (status, conn_err, last_err, curves, gen) = {
            let mut s = self.shared.lock().unwrap();
            if s.last_err.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(8)) {
                s.last_err = None;
            }
            (s.status.clone(), s.conn_err.clone(), s.last_err.as_ref().map(|e| e.0.clone()), s.curves.clone(), s.curves_gen)
        };
        // take the daemon's curves unless the user is in the middle of editing
        if gen != self.edit_gen && !self.dirty && self.dragging.is_none() {
            if let Some(c) = curves {
                self.edit = Some(c);
                self.edit_gen = gen;
            }
        }
        if last_err.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(e) = &conn_err {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                    ui.separator();
                }
                if let Some(e) = &last_err {
                    ui.colored_label(ui.visuals().error_fg_color, format!("操作被拒绝：{e}"));
                    ui.separator();
                }
                match &status {
                    Some(st) => {
                        ui.columns(2, |cols| {
                            self.mode_section(&mut cols[0], st);
                            cols[0].add_space(10.0);
                            self.fan_section(&mut cols[0], st);
                            self.live_section(&mut cols[1], st);
                        });
                    }
                    None => {
                        ui.label("正在连接后台服务…");
                    }
                }
                ui.separator();
                self.curve_section(ui, status.as_ref());
            });
        });
    }
}

fn main() -> eframe::Result {
    let mut socket = SOCKET_PATH.to_string();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = it.next().unwrap_or(socket),
            _ => {
                eprintln!("usage: omenctl-gui [--socket PATH]");
                std::process::exit(2);
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OMEN 控制")
            .with_app_id("omenctl-gui")
            .with_inner_size([820.0, 720.0])
            .with_min_inner_size([600.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "omenctl-gui",
        options,
        Box::new(move |cc| {
            install_cjk_font(&cc.egui_ctx);
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
                fixed_level: 30,
            }))
        }),
    )
}
