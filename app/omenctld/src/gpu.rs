//! NVIDIA GPU temperature and power through NVML.
//!
//! Any process that keeps /dev/nvidia* open stops the dGPU from entering runtime D3,
//! and polling tools such as nvidia-smi in a loop keep it awake. So:
//! - `SysfsHw` only calls us while power/runtime_status is "active" (the GPU is
//!   already awake because something else uses it);
//! - NVML is loaded, queried and shut down again on every read (nothing stays open);
//! - reads are rate limited to one every `GPU_POLL_S` seconds; between reads the
//!   cached value is returned.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const GPU_POLL_S: u64 = 5;
const PCI_BUS_ID: &str = "0000:01:00.0";

#[derive(Default)]
struct Inner {
    last_read: Option<Instant>,
    /// Last time `SysfsHw` asked (i.e. the GPU was active).
    last_asked: Option<Instant>,
    temp: Option<f32>,
    power_w: Option<f32>,
    last_err: Option<String>,
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
    /// Temperature reader for `SysfsHw::with_gpu_reader`.
    pub fn read_temp(&self) -> Option<f32> {
        let mut g = self.0.lock().unwrap();
        let now = Instant::now();
        g.last_asked = Some(now);
        if g.last_read.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(GPU_POLL_S)) {
            g.last_read = Some(now);
            match query() {
                Ok((t, p)) => {
                    g.temp = Some(t);
                    g.power_w = p;
                    g.last_err = None;
                }
                Err(e) => {
                    if g.last_err.as_deref() != Some(&e) {
                        eprintln!("GPU: {e}");
                    }
                    g.last_err = Some(e);
                    g.temp = None;
                    g.power_w = None;
                }
            }
        }
        g.temp
    }

    /// GPU power, only if the GPU was active during the last tick.
    pub fn power_w(&self) -> Option<f32> {
        let mut g = self.0.lock().unwrap();
        let fresh = g.last_asked.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        if !fresh {
            // GPU went to sleep: forget values so the next wake-up re-reads at once
            g.temp = None;
            g.power_w = None;
            g.last_read = None;
            return None;
        }
        g.power_w
    }
}
