//! omenctld: root daemon for HP OMEN 16 (board 8D3F) performance and fan control.
//!
//! - ticks the controller once per second (fan curve, protection, kernel watchdog kick);
//! - serves CLI/GUI requests on a Unix socket (line-delimited JSON, see proto.rs);
//! - systemd Type=notify: READY=1 after the first apply, WATCHDOG=1 from the main loop;
//! - re-applies the current mode after resume from sleep;
//! - on SIGTERM/SIGINT: controller.shutdown() (balanced profile, fans back to BIOS).

mod files;
mod gpu;
mod sdnotify;
mod server;
mod telemetry;

use files::State;
use omenctl_core::controller::Controller;
use omenctl_core::hw::{Hw, SysfsHw};
use omenctl_core::proto::{Request, Response, Status, SOCKET_PATH};
use omenctl_core::FanMode;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(1);

struct Args {
    root: PathBuf,
    config: PathBuf,
    state: PathBuf,
    socket: PathBuf,
    group: String,
    gpu: bool,
}

const USAGE: &str = "usage: omenctld [--config FILE] [--state FILE] [--socket PATH] [--group NAME] [--root DIR] [--no-gpu]

  --config FILE   configuration (default /etc/omenctl/config.toml)
  --state  FILE   saved mode / fan mode (default /var/lib/omenctl/state.json)
  --socket PATH   control socket (default /run/omenctl.sock)
  --group  NAME   group allowed to use the socket (default omenctl)
  --root   DIR    prefix for every /sys path (testing with a fake sysfs tree)
  --no-gpu        never load NVML";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        root: "/".into(),
        config: "/etc/omenctl/config.toml".into(),
        state: "/var/lib/omenctl/state.json".into(),
        socket: SOCKET_PATH.into(),
        group: "omenctl".into(),
        gpu: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--root" => a.root = val()?.into(),
            "--config" => a.config = val()?.into(),
            "--state" => a.state = val()?.into(),
            "--socket" => a.socket = val()?.into(),
            "--group" => a.group = val()?,
            "--no-gpu" => a.gpu = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {k}\n{USAGE}")),
        }
    }
    Ok(a)
}

/// Daemon state around the controller.
struct Daemon<H: Hw> {
    ctl: Controller<H>,
    state_path: PathBuf,
    config_path: PathBuf,
    saved: Option<State>,
    rapl: telemetry::RaplPower,
    gpu: Option<gpu::GpuMonitor>,
    cpu_w: Option<f32>,
    last_alerts: Vec<String>,
    start: Instant,
}

impl<H: Hw> Daemon<H> {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn persist(&mut self) {
        let st = State { mode: self.ctl.user_mode(), fan: self.ctl.fan_mode() };
        if self.saved != Some(st) {
            match files::save_state(&self.state_path, &st) {
                Ok(()) => self.saved = Some(st),
                Err(e) => eprintln!("saving {}: {e}", self.state_path.display()),
            }
        }
    }

    fn fill(&self, mut s: Status) -> Status {
        s.cpu_power_w = self.cpu_w;
        s.gpu_power_w = self.gpu.as_ref().and_then(|g| g.power_w());
        s
    }

    fn tick(&mut self) -> Status {
        let now = self.now();
        let st = self.ctl.tick(now);
        self.cpu_w = self.rapl.sample(now);
        if st.alerts != self.last_alerts {
            for a in st.alerts.iter().filter(|a| !self.last_alerts.contains(a)) {
                eprintln!("ALERT: {a}");
            }
            if st.alerts.is_empty() {
                eprintln!("all alerts cleared");
            }
            self.last_alerts = st.alerts.clone();
        }
        self.persist(); // the controller may adopt an external profile change
        self.fill(st)
    }

    fn handle(&mut self, req: Request) -> Response {
        let now = self.now();
        let resp = match req {
            Request::Status => Ok(()),
            Request::SetMode { mode } => {
                eprintln!("request: mode {mode:?}");
                self.ctl.set_mode(mode, now);
                Ok(())
            }
            Request::SetFan { fan } => {
                eprintln!("request: fan {fan:?}");
                self.ctl.set_fan(fan, now)
            }
            Request::GetCurves => {
                let (c, g) = self.ctl.curves();
                return Response::Curves { cpu: c.clone(), gpu: g.clone() };
            }
            Request::SetCurves { cpu, gpu } => {
                eprintln!("request: set curves");
                self.set_curves(cpu, gpu)
            }
        };
        self.persist();
        match resp {
            Ok(()) => Response::Ok { status: self.fill(self.ctl.status()) },
            Err(message) => Response::Error { message },
        }
    }

    /// Validate, write the config file, then apply. Nothing changes if any step fails.
    fn set_curves(&mut self, cpu: omenctl_core::curve::FanCurve, gpu: omenctl_core::curve::FanCurve) -> Result<(), String> {
        let cpu = cpu.validated().map_err(|e| format!("CPU 曲线：{e}"))?;
        let gpu = gpu.validated().map_err(|e| format!("显卡曲线：{e}"))?;
        files::save_curves(&self.config_path, &cpu, &gpu).map_err(|e| format!("写配置失败：{e}"))?;
        self.ctl.set_curves(cpu, gpu)?;
        Ok(())
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let cfg = files::load_config(&args.config)?;
    let saved = files::load_state(&args.state);
    let (mode, fan) = match saved {
        Some(s) => (s.mode, s.fan),
        None => (cfg.default_mode, FanMode::Curve),
    };
    eprintln!("omenctld {} starting: mode {mode:?}, fan {fan:?}", env!("CARGO_PKG_VERSION"));

    let term = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, term.clone()).map_err(|e| format!("signal: {e}"))?;
    }

    let gpu = args.gpu.then(gpu::GpuMonitor::default);
    let mut hw = SysfsHw::new(&args.root);
    if let Some(g) = gpu.clone() {
        hw = hw.with_gpu_reader(move || g.read_temp());
    }
    let notifier = sdnotify::Notifier::from_env();

    let mut d = Daemon {
        ctl: Controller::new(hw, cfg, mode, fan),
        state_path: args.state.clone(),
        config_path: args.config.clone(),
        saved,
        rapl: telemetry::RaplPower::new(&args.root),
        gpu,
        cpu_w: None,
        last_alerts: Vec::new(),
        start: Instant::now(),
    };
    let now = d.now();
    d.ctl.start(now);
    d.persist();

    let listener = server::bind(&args.socket, &args.group).map_err(|e| format!("{}: {e}", args.socket.display()))?;
    let (tx, rx) = channel::<server::Job>();
    server::spawn(listener, tx);

    d.tick();
    notifier.notify("READY=1");
    eprintln!("ready; socket {}", args.socket.display());

    let wd_every = notifier.watchdog.map(|w| w / 3);
    let mut last_wd = Instant::now();
    let mut resume = telemetry::ResumeDetector::new(telemetry::suspended_seconds());
    let mut next_tick = Instant::now() + TICK;

    while !term.load(Ordering::Relaxed) {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait.min(Duration::from_millis(250))) {
            Ok(job) => {
                let r = d.handle(job.req);
                let _ = job.reply.send(r);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("socket thread died".into()),
        }
        if Instant::now() < next_tick {
            continue;
        }
        next_tick = Instant::now() + TICK;
        if resume.check(telemetry::suspended_seconds()) {
            eprintln!("resume from sleep detected: re-applying mode");
            let now = d.now();
            d.ctl.reapply(now);
        }
        d.tick();
        if let Some(every) = wd_every {
            if last_wd.elapsed() >= every {
                notifier.notify("WATCHDOG=1");
                last_wd = Instant::now();
            }
        }
    }

    eprintln!("stopping: balanced profile, fans back to BIOS");
    notifier.notify("STOPPING=1");
    d.ctl.shutdown();
    let _ = std::fs::remove_file(&args.socket);
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("omenctld: {e}");
        std::process::exit(1);
    }
}

