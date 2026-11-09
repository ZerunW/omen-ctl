//! NVIDIA GPU temperature and power through NVML.
//!
//! Any process that keeps /dev/nvidia* open stops the dGPU from entering runtime D3,
//! and polling tools such as nvidia-smi in a loop keep it awake. So:
//! - `SysfsHw` only calls us while power/runtime_status is "active" (the GPU is
//!   already awake because something else uses it);
//! - NVML is loaded, queried and shut down again on every read (nothing stays open);
//! - reads are rate limited to one every `GPU_POLL_S` seconds.
//!
//! NVML calls run on their own thread and never on the control loop: while the
//! system prepares for sleep, nvidia-suspend.service suspends the GPU and a new
//! NVML session blocks until resume. On the control loop that stalled the fan
//! control and tripped the service watchdog (2026-11-08). Now the loop only reads
//! cached values; a stuck query just makes the values go stale (treated as unknown).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const GPU_POLL_S: u64 = 5;
/// Values older than this are not used (query stuck or GPU asleep).
const STALE_AFTER: Duration = Duration::from_secs(15);
const PCI_BUS_ID: &str = "0000:01:00.0";

#[derive(Default)]
struct Inner {
    /// Last time `SysfsHw` asked (i.e. the GPU was active).
    last_asked: Option<Instant>,
    /// When the cached values were measured.
    measured: Option<Instant>,
    temp: Option<f32>,
    power_w: Option<f32>,
    last_err: Option<String>,
    worker: bool,
}

#[derive(Clone, Default)]
pub struct GpuMonitor(Arc<Mutex<Inner>>);

fn query() -> Result<(f32, Option<f32>), String> {
    use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
    let nvml = nvml_wrapper::Nvml::init().map_err(|e| format!("NVML init: {e}"))?;
    let dev = nvml.device_by_pci_bus_id(PCI_BUS_ID).map_err(|e| format!("NVML device: {e}"))?;
    let t = dev.temperature(TemperatureSensor::Gpu).map_err(|e| format!("NVML temperature: {e}"))?;
    let p = dev.power_usage().ok().map(|mw| mw as f32 / 1000.0);
    drop(dev);
    drop(nvml); // nvmlShutdown + unload: nothing stays open
    Ok((t as f32, p))
}

impl GpuMonitor {
    fn fresh(g: &Inner) -> bool {
        g.measured.is_some_and(|t| t.elapsed() < STALE_AFTER)
    }

    /// Background reader: queries only while the GPU was reported active in the
    /// last 1.5 s, at most every `GPU_POLL_S` seconds.
    fn worker(self) {
        let mut last_query: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let wanted = {
                let g = self.0.lock().unwrap();
                g.last_asked.is_some_and(|t| t.elapsed() < Duration::from_millis(1500))
            };
            if !wanted {
                last_query = None; // re-read at once when the GPU wakes up
                continue;
            }
            if last_query.is_some_and(|t| t.elapsed() < Duration::from_secs(GPU_POLL_S)) {
                continue;
            }
            last_query = Some(Instant::now());
            let r = query(); // may block (GPU suspending); the control loop doesn't wait
            let mut g = self.0.lock().unwrap();
            match r {
                Ok((t, p)) => {
                    g.temp = Some(t);
                    g.power_w = p;
                    g.measured = Some(Instant::now());
                    g.last_err = None;
                }
                Err(e) => {
                    if g.last_err.as_deref() != Some(&e) {
                        eprintln!("GPU: {e}");
                    }
                    g.last_err = Some(e);
                    g.temp = None;
                    g.power_w = None;
                    g.measured = None;
                }
            }
        }
    }

    /// Temperature reader for `SysfsHw::with_gpu_reader`. Never blocks on NVML.
    pub fn read_temp(&self) -> Option<f32> {
        let mut g = self.0.lock().unwrap();
        g.last_asked = Some(Instant::now());
        if !g.worker {
            g.worker = true;
            let me = self.clone();
            if let Err(e) = std::thread::Builder::new().name("gpu-nvml".into()).spawn(move || me.worker()) {
                eprintln!("GPU: cannot start reader thread: {e}");
                g.worker = false;
            }
        }
        if Self::fresh(&g) {
            g.temp
        } else {
            None
        }
    }

    /// GPU power, only if the GPU was active during the last tick and the value is fresh.
    pub fn power_w(&self) -> Option<f32> {
        let mut g = self.0.lock().unwrap();
        let active = g.last_asked.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        if !active {
            // GPU went to sleep: forget values so they are not shown as current
            g.temp = None;
            g.power_w = None;
            g.measured = None;
            return None;
        }
        if Self::fresh(&g) {
            g.power_w
        } else {
            None
        }
    }
}
