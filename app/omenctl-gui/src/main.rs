//! omenctl-gui: desktop front end for omenctld (runs as a normal user in the
//! omenctl group). All changes go through the service; nothing here touches sysfs.

mod edit;
mod theme;

use eframe::egui::{self, Pos2, Rect, Sense, Stroke, Vec2};
use omenctl_core::config::{ModeRule, ModesConfig};
use omenctl_core::curve::{FanCurve, LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use theme::{font, medium, ButtonKind, Palette, CONTROL_H};

/// Seconds of history shown in the charts (one sample per status poll).
const HISTORY: usize = 90;
const GAP: f32 = 12.0;

// ---------------------------------------------------------------- worker thread

#[derive(Clone, Copy, Default)]
struct Sample {
    cpu_t: Option<f32>,
    gpu_t: Option<f32>,
    rpm: [Option<f32>; 2],
    cpu_w: Option<f32>,
    gpu_w: Option<f32>,
    /// Whole-machine draw, known only while running on battery.
    system_w: Option<f32>,
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
    modes: Option<ModesConfig>,
    /// Shown briefly after the rules were saved.
    modes_saved: Option<Instant>,
}

fn sample(st: &Status) -> Sample {
    let system_w = st.power.battery.as_ref().and_then(|b| b.power_w).filter(|w| *w < -0.1).map(|w| -w);
    Sample {
        cpu_t: st.cpu_temp,
        gpu_t: st.gpu_temp,
        rpm: st.fan_rpm.map(|r| r.map(|v| v as f32)),
        cpu_w: st.cpu_power_w,
        // a sleeping GPU draws next to nothing; plot it as 0 instead of a gap
        gpu_w: st.gpu_power_w.or(st.gpu_temp.is_none().then_some(0.0)),
        system_w,
    }
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
                    // after every (re)connect: fetch curves and rules before anything else
                    if !matches!(req, Request::GetCurves) {
                        queue.push_front(req);
                        queue.push_front(Request::GetSettings);
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
        let set_modes = matches!(req, Request::SetSettings { .. });
        let r = client.as_mut().unwrap().call(&req);
        {
            let mut s = shared.lock().unwrap();
            match r {
                Ok(Response::Ok { status }) => {
                    if poll {
                        s.history.push_back(sample(&status));
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
                Ok(Response::Settings { modes }) => {
                    s.modes = Some(modes);
                    if set_modes {
                        s.modes_saved = Some(Instant::now());
                    }
                    s.conn_err = None;
                }
                Ok(Response::Error { message }) => {
                    s.last_err = Some((message, Instant::now()));
                    if set_modes {
                        queue.push_back(Request::GetSettings); // show what is really saved
                    }
                }
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
    /// Light / dark / follow the desktop; saved in ~/.config/omenctl/gui.toml.
    theme: egui::ThemePreference,
}

const THEMES: [egui::ThemePreference; 3] =
    [egui::ThemePreference::System, egui::ThemePreference::Light, egui::ThemePreference::Dark];

fn theme_name(t: egui::ThemePreference) -> &'static str {
    match t {
        egui::ThemePreference::System => "system",
        egui::ThemePreference::Light => "light",
        egui::ThemePreference::Dark => "dark",
    }
}

fn theme_parse(s: &str) -> Option<egui::ThemePreference> {
    THEMES.into_iter().find(|t| theme_name(*t) == s.trim())
}

/// Per-user GUI settings file (not the service config).
fn gui_config_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("omenctl").join("gui.toml"))
}

fn load_theme() -> Option<egui::ThemePreference> {
    let text = std::fs::read_to_string(gui_config_path()?).ok()?;
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "theme").then(|| theme_parse(v.trim().trim_matches('"')))?
    })
}

fn save_theme(t: egui::ThemePreference) {
    let Some(p) = gui_config_path() else { return };
    let r = p.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| {
        std::fs::write(&p, format!("# omenctl-gui settings\ntheme = \"{}\"\n", theme_name(t)))
    });
    if let Err(e) = r {
        eprintln!("omenctl-gui: cannot save {}: {e}", p.display());
    }
}

const MODES: [Mode; 3] = [Mode::PowerSaver, Mode::Balanced, Mode::Unleashed];

const INFO_MODE: &str = "Power Saver: lowest power and heat.\n\
Balanced: the system's standard power limits.\n\
Unleashed: CPU 90 W sustained and 130 W boost, GPU up to 115 W. Fans are always software controlled in this mode.\n\n\
If the CPU or GPU stays at 95 °C, Unleashed pauses until it cools below 80 °C.";
const INFO_POWER: &str = "CPU: package power from the processor's energy counter.\n\
GPU: board power reported by the NVIDIA driver (0 while the GPU sleeps; it is never woken up to be measured).\n\
System: whole-machine draw from the battery. Linux can only measure this while running on battery.";
const INFO_FANS: &str = "Auto: the BIOS controls the fans (not available in Unleashed).\n\
Curve: fans follow the fan curve for the hotter of CPU and GPU.\n\
Manual: both fans at a fixed speed.\n\
Max: both fans at full speed.\n\n\
At 92 °C both fans always go to full speed.";
const INFO_MANUAL: &str = "Fixed speed for both fans. Moving the slider switches to Manual.\n\
The tick marks what the fan curve recommends at the current temperature.";
const INFO_CURVE: &str = "Drag a point to change it. Horizontal: the hotter of CPU and GPU. Vertical: fan speed.\n\
Changes take effect after Apply. Reset restores the BIOS fan table.";
const INFO_SUPPLY: &str = "Read from the kernel's power supply interface (/sys/class/power_supply).\n\
Battery health is the current full capacity compared with the design capacity.";
const INFO_ADAPTER: &str = "Rated power of the connected adapter as reported by the BIOS. \
It needs a driver version that exposes this value; until then it shows a dash.";
const INFO_DEFAULTS: &str = "At startup: the mode used when the computer starts. \"Last used\" keeps whatever was selected before; \
if a plugged in / on battery rule matches the current power source, that rule is used instead.\n\n\
Plugged in / On battery: switch to this mode when the adapter is connected or removed. \
You can still change the mode by hand at any time; the rule acts again on the next plug or unplug.\n\n\
Low battery: switch to this mode once when the battery falls to the threshold while unplugged. \
It acts again after plugging in or after the charge rose 5% above the threshold.";

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

fn hours(h: f32) -> String {
    let m = (h * 60.0).round() as u32;
    format!("{}h {:02}m", m / 60, m % 60)
}

fn rule_label(r: ModeRule, startup: bool) -> &'static str {
    match r {
        ModeRule::Keep if startup => "Last used",
        ModeRule::Keep => "No change",
        ModeRule::PowerSaver => "Power Saver",
        ModeRule::Balanced => "Balanced",
        ModeRule::Unleashed => "Unleashed",
    }
}

impl App {
    fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    // ------------------------------------------------------------ header

    fn header(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>, offline: bool) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("OMEN Control").font(medium(20.0)).color(p.label));
                let (dot, text) = if offline {
                    (p.red, "Service offline".to_string())
                } else if let Some(st) = st {
                    let m = if st.mode == st.effective_mode {
                        st.mode.label().to_string()
                    } else {
                        format!("{} (paused, running {})", st.mode.label(), st.effective_mode.label())
                    };
                    (p.green, format!("Connected · {m}"))
                } else {
                    (p.tertiary, "Connecting…".to_string())
                };
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(8.0, 14.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, dot);
                    ui.label(egui::RichText::new(text).font(font(12.0)).color(p.secondary));
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // appearance: follow the desktop, light or dark
                let sel = THEMES.iter().position(|t| *t == self.theme);
                if let Some(i) = theme::segmented(ui, p, "theme", &["Auto", "Light", "Dark"], sel, &[true; 3], 168.0) {
                    self.theme = THEMES[i];
                    ui.ctx().set_theme(self.theme);
                    save_theme(self.theme);
                }
                ui.add_space(12.0);
                if let Some(st) = st {
                    theme::info(ui, p, INFO_MODE);
                    let w = ui.available_width().min(360.0);
                    let labels: Vec<&str> = MODES.iter().map(|m| m.label()).collect();
                    let sel = MODES.iter().position(|m| *m == st.mode);
                    if let Some(i) = theme::segmented(ui, p, "mode", &labels, sel, &[true; 3], w) {
                        self.send(Request::SetMode { mode: MODES[i] });
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------ stat tiles

    fn tiles(&self, ui: &mut egui::Ui, p: &Palette, st: &Status, hist: &[Sample]) {
        let w = ui.available_width();
        let cols = if w >= 640.0 { 4 } else { 2 };
        let tile_w = (w - GAP * (cols as f32 - 1.0)) / cols as f32;
        let h = 112.0;
        let rows = 4 / cols;
        let (area, _) = ui.allocate_exact_size(Vec2::new(w, rows as f32 * h + (rows as f32 - 1.0) * GAP), Sense::hover());
        let painter = ui.painter_at(area.expand(2.0));
        let watts = |v: Option<f32>| v.map_or("–".to_string(), |v| format!("{v:.0} W"));

        for i in 0..4 {
            let (r, c) = (i / cols, i % cols);
            let rect = Rect::from_min_size(
                Pos2::new(area.left() + c as f32 * (tile_w + GAP), area.top() + r as f32 * (h + GAP)),
                Vec2::new(tile_w, h),
            );
            painter.rect_filled(rect, theme::CARD_RADIUS, p.card);
            painter.rect_stroke(rect, theme::CARD_RADIUS, Stroke::new(1.0, p.card_border), egui::StrokeKind::Inside);
            let inner = rect.shrink2(Vec2::new(14.0, 12.0));
            let (title, accent, value, sub): (&str, egui::Color32, String, String) = match i {
                0 => ("CPU", p.cpu, st.cpu_temp.map_or("–".into(), |t| format!("{t:.0}°C")), watts(st.cpu_power_w)),
                1 => match st.gpu_temp {
                    Some(t) => ("GPU", p.gpu, format!("{t:.0}°C"), watts(st.gpu_power_w)),
                    None => ("GPU", p.gpu, "Idle".into(), "asleep, not polled".into()),
                },
                2 => {
                    let f = |v: Option<u32>| v.map_or("–".to_string(), |v| v.to_string());
                    let mode = match st.fan {
                        FanMode::Auto => "BIOS auto".to_string(),
                        FanMode::Curve => "curve".into(),
                        FanMode::Fixed(l) => format!("fixed {}", l as u32 * 100),
                        FanMode::Max => "maximum".into(),
                    };
                    ("Fans", p.secondary, format!("{} · {}", f(st.fan_rpm[0]), f(st.fan_rpm[1])), format!("rpm · {mode}"))
                }
                _ => {
                    let pw = &st.power;
                    let b = pw.battery.as_ref();
                    match st.ac_online {
                        Some(false) => {
                            let v = b.and_then(|b| b.percent).map_or("Battery".into(), |x| format!("{x}%"));
                            let draw = b.and_then(|b| b.power_w).filter(|w| *w < -0.1).map(|w| format!("{:.0} W draw", -w));
                            let left = b.and_then(|b| b.hours_left()).map(|h| format!("{} left", hours(h)));
                            let sub = [draw, left].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                            ("Battery", p.system, v, if sub.is_empty() { "on battery".into() } else { sub })
                        }
                        _ => {
                            let sub = match pw.adapter_w {
                                Some(w) => format!("{w} W adapter"),
                                None => "adapter connected".into(),
                            };
                            ("Power", p.system, "AC".into(), sub)
                        }
                    }
                }
            };
            painter.circle_filled(Pos2::new(inner.left() + 3.5, inner.top() + 8.0), 3.5, accent);
            painter.text(Pos2::new(inner.left() + 12.0, inner.top() + 8.0), egui::Align2::LEFT_CENTER, title, font(12.0), p.secondary);
            painter.text(Pos2::new(inner.left(), inner.top() + 20.0), egui::Align2::LEFT_TOP, &value, medium(26.0), p.label);
            painter.text(Pos2::new(inner.left(), inner.top() + 54.0), egui::Align2::LEFT_TOP, &sub, font(12.0), p.secondary);
            let spark = Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - 18.0), inner.right_bottom());
            let s = Stroke::new(1.5, accent);
            match i {
                0 => theme::sparkline(&painter, spark, &hist.iter().map(|h| h.cpu_t).collect::<Vec<_>>(), (30.0, 100.0), HISTORY, s),
                1 => theme::sparkline(&painter, spark, &hist.iter().map(|h| h.gpu_t).collect::<Vec<_>>(), (30.0, 100.0), HISTORY, s),
                2 => {
                    theme::sparkline(&painter, spark, &hist.iter().map(|h| h.rpm[1]).collect::<Vec<_>>(), (0.0, 5400.0), HISTORY, Stroke::new(1.5, p.tertiary));
                    theme::sparkline(&painter, spark, &hist.iter().map(|h| h.rpm[0]).collect::<Vec<_>>(), (0.0, 5400.0), HISTORY, s);
                }
                _ => {
                    // battery level bar
                    if let Some(pc) = st.power.battery.as_ref().and_then(|b| b.percent) {
                        let bar = Rect::from_min_size(Pos2::new(spark.left(), spark.center().y - 3.0), Vec2::new(spark.width(), 6.0));
                        painter.rect_filled(bar, 3.0, p.track);
                        let fill = Rect::from_min_size(bar.min, Vec2::new(bar.width() * pc as f32 / 100.0, bar.height()));
                        let c = if pc <= 15 { p.red } else if st.ac_online == Some(true) { p.green } else { p.system };
                        painter.rect_filled(fill, 3.0, c);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ power chart

    fn power_chart(&self, ui: &mut egui::Ui, p: &Palette, st: &Status, hist: &[Sample]) {
        let total = st.cpu_power_w.unwrap_or(0.0) + st.gpu_power_w.unwrap_or(0.0);
        let has_system = hist.iter().any(|h| h.system_w.is_some());
        theme::card(
            ui,
            p,
            Some("Power"),
            |ui| {
                theme::info(ui, p, INFO_POWER);
                ui.label(egui::RichText::new(format!("CPU + GPU {total:.0} W")).font(medium(13.0)).color(p.label));
                ui.add_space(8.0);
                if has_system {
                    theme::legend(ui, p, p.system, "System");
                }
                theme::legend(ui, p, p.gpu, "GPU");
                theme::legend(ui, p, p.cpu, "CPU");
            },
            |ui| {
                let w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 150.0), Sense::hover());
                let plot = Rect::from_min_max(rect.min + Vec2::new(40.0, 6.0), rect.max - Vec2::new(0.0, 18.0));
                let painter = ui.painter_at(rect);
                let peak = hist
                    .iter()
                    .flat_map(|h| [h.cpu_w, h.gpu_w, h.system_w])
                    .flatten()
                    .fold(0.0f32, f32::max);
                let (step, top) = theme::nice_scale(peak.max(40.0));
                let n = (top / step).round() as usize;
                for k in 0..=n {
                    let v = step * k as f32;
                    let y = plot.bottom() - plot.height() * v / top;
                    painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0, p.hairline));
                    painter.text(Pos2::new(plot.left() - 8.0, y), egui::Align2::RIGHT_CENTER, format!("{v:.0} W"), font(10.5), p.tertiary);
                }
                for (secs, label) in [(HISTORY - 1, format!("{}s", HISTORY - 1)), (HISTORY / 2, format!("{}s", HISTORY / 2)), (0, "now".to_string())] {
                    let x = plot.right() - secs as f32 / (HISTORY - 1) as f32 * plot.width();
                    let align = if secs == 0 { egui::Align2::RIGHT_TOP } else if secs == HISTORY - 1 { egui::Align2::LEFT_TOP } else { egui::Align2::CENTER_TOP };
                    painter.text(Pos2::new(x, plot.bottom() + 4.0), align, label, font(10.5), p.tertiary);
                }
                let line = |vals: Vec<Option<f32>>, c| theme::sparkline(&painter, plot, &vals, (0.0, top), HISTORY, Stroke::new(1.8, c));
                if has_system {
                    line(hist.iter().map(|h| h.system_w).collect(), p.system);
                }
                line(hist.iter().map(|h| h.gpu_w).collect(), p.gpu);
                line(hist.iter().map(|h| h.cpu_w).collect(), p.cpu);
            },
        );
    }

    // ------------------------------------------------------------ fans

    fn fans_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        let unleashed = st.effective_mode == Mode::Unleashed;
        if let FanMode::Fixed(l) = st.fan {
            if !self.manual_touched {
                self.manual_level = l;
            }
        }
        let dirty = self.dirty;
        let open = self.curve_open;
        let mut toggle_curve = false;
        theme::card(
            ui,
            p,
            Some("Fans"),
            |ui| theme::info(ui, p, INFO_FANS),
            |ui| {
                let sel = match st.fan {
                    FanMode::Auto => 0,
                    FanMode::Curve => 1,
                    FanMode::Fixed(_) => 2,
                    FanMode::Max => 3,
                };
                let w = ui.available_width();
                match theme::segmented(ui, p, "fan", &["Auto", "Curve", "Manual", "Max"], Some(sel), &[!unleashed, true, true, true], w) {
                    Some(0) => self.send(Request::SetFan { fan: FanMode::Auto }),
                    Some(1) => self.send(Request::SetFan { fan: FanMode::Curve }),
                    Some(2) => {
                        // start from what the fans are doing now, so nothing jumps
                        self.manual_level = st.fan_target[0].max(st.fan_target[1]).clamp(LEVEL_MIN, LEVEL_MAX);
                        self.send(Request::SetFan { fan: FanMode::Fixed(self.manual_level) });
                    }
                    Some(3) => self.send(Request::SetFan { fan: FanMode::Max }),
                    _ => {}
                }
                let rec = st.fan_recommended[0].max(st.fan_recommended[1]).max(LEVEL_MIN);
                // the speed slider belongs to Manual only
                if matches!(st.fan, FanMode::Fixed(_)) || self.manual_touched {
                ui.add_space(10.0);
                let mut level = self.manual_level;
                let (holding, commit) = ui
                    .horizontal(|ui| {
                        ui.label(egui::RichText::new("Manual").font(font(13.0)).color(p.secondary));
                        let sw = ui.available_width() - 100.0;
                        let r = theme::level_slider(ui, p, &mut level, (LEVEL_MIN, LEVEL_MAX), rec, sw);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            theme::info(ui, p, INFO_MANUAL);
                            ui.label(egui::RichText::new(rpm(level)).font(font(13.0)).color(p.label));
                        });
                        r
                    })
                    .inner;
                self.manual_level = level;
                self.manual_touched = holding;
                if commit {
                    self.send(Request::SetFan { fan: FanMode::Fixed(self.manual_level) });
                }
                let manual = matches!(st.fan, FanMode::Fixed(_)) || holding;
                if manual && self.manual_level < rec {
                    ui.add_space(4.0);
                    let at = hottest(st).map_or(String::new(), |t| format!(" at {t:.0} °C"));
                    theme::notice(ui, p, p.amber, &format!("Below the recommended {}{at}", rpm(rec)));
                }
                }
                ui.add_space(8.0);
                theme::hairline(ui, p);
                ui.add_space(4.0);
                let target = format!("target {} / {}", st.fan_target[0] as u32 * 100, st.fan_target[1] as u32 * 100);
                ui.horizontal(|ui| {
                    let (row, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
                    let c = if resp.hovered() { p.label } else { p.secondary };
                    let o = ui.ctx().animate_bool_with_time(egui::Id::unique("curve_open"), open, 0.16);
                    let painter = ui.painter();
                    theme::chevron(painter, Pos2::new(row.left() + 4.0, row.center().y), o, c);
                    painter.text(Pos2::new(row.left() + 16.0, row.center().y), egui::Align2::LEFT_CENTER, "Fan curve", font(13.0), c);
                    let right = if dirty { ("Edited, not applied".to_string(), p.amber) } else { (format!("{target} rpm"), p.tertiary) };
                    painter.text(row.right_center(), egui::Align2::RIGHT_CENTER, right.0, font(12.0), right.1);
                    if resp.clicked() {
                        toggle_curve = true;
                    }
                });
            },
        );
        if toggle_curve {
            self.curve_open = !self.curve_open;
        }
    }

    // ------------------------------------------------------------ power supply

    fn supply_card(&self, ui: &mut egui::Ui, p: &Palette, st: &Status) {
        let pw = &st.power;
        theme::card(
            ui,
            p,
            Some("Power supply"),
            |ui| theme::info(ui, p, INFO_SUPPLY),
            |ui| {
                let src = match st.ac_online {
                    Some(true) => "AC adapter",
                    Some(false) => "Battery",
                    None => "Unknown",
                };
                theme::kv(ui, p, "Source", src, p.label);
                ui.horizontal(|ui| {
                    ui.set_min_height(24.0);
                    ui.label(egui::RichText::new("Adapter rating").font(font(13.0)).color(p.secondary));
                    theme::info(ui, p, INFO_ADAPTER);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match pw.adapter_w {
                        Some(w) => ui.label(egui::RichText::new(format!("{w} W")).font(font(13.0)).color(p.label)),
                        None => ui.label(egui::RichText::new("–").font(font(13.0)).color(p.tertiary)),
                    });
                });
                let usb = pw.usb_c.iter().find(|u| u.online);
                let usb_text = match usb {
                    Some(u) => {
                        let mut s = "Power input".to_string();
                        if let Some(k) = &u.kind {
                            s += &format!(" · USB {}", k.replace('_', " "));
                        }
                        if let Some(ma) = u.current_ma {
                            s += &format!(" · {:.2} A", ma as f32 / 1000.0);
                        }
                        if let Some(mv) = u.voltage_mv {
                            s += &format!(" · {:.0} V", mv as f32 / 1000.0);
                        }
                        s
                    }
                    None if pw.usb_c.is_empty() => "Not reported".into(),
                    None => "No power input".into(),
                };
                theme::kv(ui, p, "USB-C", &usb_text, if usb.is_some() { p.label } else { p.tertiary });
                ui.add_space(4.0);
                theme::hairline(ui, p);
                ui.add_space(4.0);
                match &pw.battery {
                    Some(b) => {
                        let mut s = format!("{} · {}", b.percent.map_or("–".into(), |x| format!("{x}%")), b.status);
                        if let Some(w) = b.power_w.filter(|w| w.abs() >= 0.1) {
                            s += &format!(" · {:.1} W", w.abs());
                        }
                        let color = match b.status.as_str() {
                            "Charging" => p.green,
                            "Discharging" if b.percent.is_some_and(|x| x <= 15) => p.red,
                            _ => p.label,
                        };
                        theme::kv(ui, p, "Battery", &s, color);
                        if let Some(h) = b.hours_left() {
                            let what = if b.power_w.unwrap_or(0.0) < 0.0 { "Time left" } else { "Time to full" };
                            theme::kv(ui, p, what, &hours(h), p.label);
                        }
                        if let (Some(h), Some(f), Some(d)) = (b.health(), b.full_wh, b.design_wh) {
                            theme::kv(ui, p, "Health", &format!("{:.0}% · {f:.1} of {d:.1} Wh", h * 100.0), p.label);
                        }
                        if let Some(c) = b.cycles {
                            theme::kv(ui, p, "Charge cycles", &c.to_string(), p.label);
                        }
                    }
                    None => theme::kv(ui, p, "Battery", "Not found", p.tertiary),
                }
            },
        );
    }

    // ------------------------------------------------------------ default modes

    fn defaults_card(&self, ui: &mut egui::Ui, p: &Palette, modes: Option<ModesConfig>, saved: bool) {
        theme::card(
            ui,
            p,
            Some("Default modes"),
            |ui| {
                theme::info(ui, p, INFO_DEFAULTS);
                if saved {
                    ui.label(egui::RichText::new("Saved").font(font(12.0)).color(p.green));
                }
            },
            |ui| {
                let Some(m) = modes else {
                    ui.label(egui::RichText::new("Loading…").color(p.secondary));
                    return;
                };
                let mut next = m;
                let rows: [(&str, &str, &mut ModeRule, bool); 4] = [
                    ("At startup", "startup", &mut next.startup, true),
                    ("Plugged in", "on_ac", &mut next.on_ac, false),
                    ("On battery", "on_battery", &mut next.on_battery, false),
                    ("Low battery", "on_low_battery", &mut next.on_low_battery, false),
                ];
                for (label, id, value, startup) in rows {
                    ui.horizontal(|ui| {
                        ui.set_min_height(CONTROL_H + 4.0);
                        ui.label(egui::RichText::new(label).font(font(13.0)).color(p.secondary));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::ComboBox::from_id_salt(id)
                                .width(150.0)
                                .selected_text(rule_label(*value, startup))
                                .show_ui(ui, |ui| {
                                    for r in ModeRule::ALL {
                                        ui.selectable_value(value, r, rule_label(r, startup));
                                    }
                                });
                        });
                    });
                }
                ui.horizontal(|ui| {
                    ui.set_min_height(CONTROL_H + 4.0);
                    ui.label(egui::RichText::new("Low battery at").font(font(13.0)).color(p.secondary));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("low_battery_percent")
                            .width(150.0)
                            .selected_text(format!("{}% or less", next.low_battery_percent))
                            .show_ui(ui, |ui| {
                                let mut opts = vec![10u8, 15, 20, 25, 30, 40];
                                if !opts.contains(&next.low_battery_percent) {
                                    opts.push(next.low_battery_percent);
                                    opts.sort();
                                }
                                for v in opts {
                                    ui.selectable_value(&mut next.low_battery_percent, v, format!("{v}% or less"));
                                }
                            });
                    });
                });
                if next != m {
                    self.send(Request::SetSettings { modes: next });
                }
            },
        );
    }

    // ------------------------------------------------------------ fan curve editor

    fn curve_card(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>) {
        let sel = if self.which == Which::Cpu { 0 } else { 1 };
        let mut pick = None;
        theme::card(
            ui,
            p,
            Some("Fan curve"),
            |ui| {
                theme::info(ui, p, INFO_CURVE);
                pick = theme::segmented(ui, p, "which", &["CPU fan", "GPU fan"], Some(sel), &[true, true], 180.0);
            },
            |ui| self.curve_body(ui, p, st),
        );
        if let Some(i) = pick {
            self.which = if i == 0 { Which::Cpu } else { Which::Gpu };
        }
    }

    fn curve_body(&mut self, ui: &mut egui::Ui, p: &Palette, st: Option<&Status>) {
        let Some((cpu, gpu)) = self.edit.as_mut() else {
            ui.label(egui::RichText::new("Loading…").color(p.secondary));
            return;
        };
        let color = if self.which == Which::Cpu { p.cpu } else { p.gpu };
        let curve = if self.which == Which::Cpu { cpu } else { gpu };

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 240.0), Sense::click_and_drag());
        let plot = Rect::from_min_max(rect.min + Vec2::new(48.0, 8.0), rect.max - Vec2::new(12.0, 22.0));
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
        for t in (20..=100).step_by(10) {
            let x = to_screen(t as f32, 0.0).x;
            painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, p.hairline));
            painter.text(Pos2::new(x, plot.bottom() + 6.0), egui::Align2::CENTER_TOP, format!("{t}°"), font(10.5), p.tertiary);
        }
        if let Some(t) = st.and_then(hottest) {
            let x = to_screen(t.clamp(edit::T_MIN, edit::T_MAX), 0.0).x;
            painter.extend(egui::Shape::dashed_line(&[Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0, p.secondary), 3.0, 3.0));
            painter.text(Pos2::new(x + 4.0, plot.top()), egui::Align2::LEFT_TOP, format!("now {t:.0}°"), font(10.5), p.secondary);
        }
        let mut line = vec![to_screen(edit::T_MIN, curve.points[0].level as f32)];
        line.extend(curve.points.iter().map(|q| to_screen(q.temp, q.level as f32)));
        line.push(to_screen(edit::T_MAX, curve.points.last().unwrap().level as f32));
        // soft fill under the curve
        let mut area = line.clone();
        area.push(Pos2::new(plot.right(), plot.bottom()));
        area.push(Pos2::new(plot.left(), plot.bottom()));
        let fill = color.gamma_multiply(if p.dark { 0.14 } else { 0.08 });
        for w in line.windows(2) {
            painter.add(egui::Shape::convex_polygon(
                vec![w[0], w[1], Pos2::new(w[1].x, plot.bottom()), Pos2::new(w[0].x, plot.bottom())],
                fill,
                Stroke::NONE,
            ));
        }
        painter.add(egui::Shape::line(line, Stroke::new(2.0, color)));

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
            let r = if hot { 6.0 } else { 4.5 };
            painter.circle_filled(at, r, p.card);
            painter.circle_stroke(at, r, Stroke::new(2.0, color));
            if hot {
                tip = Some((at, format!("{:.1} °C  ·  {}", c.temp, rpm(c.level))));
            }
        }
        if let Some((at, text)) = tip {
            let g = painter.layout_no_wrap(text, font(12.0), p.card);
            let size = g.size() + Vec2::new(14.0, 8.0);
            let mut r = Rect::from_min_size(Pos2::new(at.x - size.x / 2.0, at.y - size.y - 12.0), size);
            r = r.translate(Vec2::new((plot.left() - r.left()).max(0.0) + (plot.right() - r.right()).min(0.0), (rect.top() - r.top()).max(0.0)));
            painter.rect_filled(r, 5.0, p.label);
            painter.galley(r.min + Vec2::new(7.0, 4.0), g, p.card);
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if self.dirty {
                ui.label(egui::RichText::new("Not applied yet").font(font(12.0)).color(p.amber));
            }
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
                if theme::button(ui, p, "Reset to BIOS table", ButtonKind::Plain, true) {
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
        let (status, hist, conn_err, last_err, curves, gen, modes, modes_saved) = {
            let mut s = self.shared.lock().unwrap();
            if s.last_err.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(8)) {
                s.last_err = None;
            }
            if s.modes_saved.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
                s.modes_saved = None;
            }
            let hist: Vec<Sample> = s.history.iter().copied().collect();
            (
                s.status.clone(),
                hist,
                s.conn_err.clone(),
                s.last_err.as_ref().map(|e| e.0.clone()),
                s.curves.clone(),
                s.curves_gen,
                s.modes,
                s.modes_saved.is_some(),
            )
        };
        if gen != self.edit_gen && !self.dirty && self.dragging.is_none() {
            if let Some(c) = curves {
                self.edit = Some(c);
                self.edit_gen = gen;
            }
        }
        if last_err.is_some() || modes_saved {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin::symmetric(22, 18)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let w = ui.available_width().min(1040.0);
                    let pad = (ui.available_width() - w) / 2.0;
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(w);
                            ui.spacing_mut().item_spacing.y = GAP;
                            self.header(ui, &p, status.as_ref(), conn_err.is_some());
                            if let Some(e) = &conn_err {
                                theme::notice(ui, &p, p.red, e);
                            }
                            if let Some(e) = &last_err {
                                theme::notice(ui, &p, p.red, &format!("Not applied: {e}"));
                            }
                            if let Some(st) = &status {
                                for a in st.alerts.iter().filter(|a| !a.contains("below the recommended")) {
                                    theme::notice(ui, &p, if is_severe(a) { p.red } else { p.amber }, a);
                                }
                            }
                            let Some(st) = &status else { return };
                            self.tiles(ui, &p, st, &hist);
                            self.power_chart(ui, &p, st, &hist);
                            let two = w >= 760.0;
                            if two {
                                let col = (w - GAP) / 2.0;
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = GAP;
                                    ui.vertical(|ui| {
                                        ui.set_width(col);
                                        ui.spacing_mut().item_spacing.y = GAP;
                                        self.fans_card(ui, &p, st);
                                        self.defaults_card(ui, &p, modes, modes_saved);
                                    });
                                    ui.vertical(|ui| {
                                        ui.set_width(col);
                                        self.supply_card(ui, &p, st);
                                    });
                                });
                            } else {
                                self.fans_card(ui, &p, st);
                                self.supply_card(ui, &p, st);
                                self.defaults_card(ui, &p, modes, modes_saved);
                            }
                            if self.curve_open {
                                self.curve_card(ui, &p, Some(st));
                            }
                            ui.add_space(4.0);
                        });
                    });
                });
            });
    }
}

const USAGE: &str = "usage: omenctl-gui [--socket PATH] [--theme system|light|dark]";

fn main() -> eframe::Result {
    let mut socket = SOCKET_PATH.to_string();
    // --theme wins for this run; otherwise the choice saved from the app
    let mut theme_arg = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--socket", Some(s)) => socket = s,
            ("--theme", Some(t)) if theme_parse(&t).is_some() => theme_arg = theme_parse(&t),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let theme_pref = theme_arg.or_else(load_theme).unwrap_or(egui::ThemePreference::System);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OMEN Control")
            .with_app_id("omenctl-gui")
            .with_inner_size([1000.0, 800.0])
            .with_min_inner_size([560.0, 480.0])
            .with_icon(egui::IconData {
                rgba: include_bytes!("../../../packaging/icon/omenctl-gui-256.rgba").to_vec(),
                width: 256,
                height: 256,
            }),
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
                theme: theme_pref,
            }))
        }),
    )
}
