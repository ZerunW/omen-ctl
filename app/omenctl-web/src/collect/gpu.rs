use super::read_value;
use crate::snapshot::{GpuProcess, Igpu, Nvidia};
use nvml_wrapper::{
    enum_wrappers::device::{Clock, TemperatureSensor},
    enums::device::UsedGpuMemory,
    Nvml,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Once,
};

const PCI_BUS_ID: &str = "0000:01:00.0";

pub(super) fn intel(
    root: &Path,
    previous: &mut Option<(PathBuf, u64)>,
    elapsed: f64,
) -> Option<Igpu> {
    let old = previous.take();
    let (_, card) = fs::read_dir(root.join("sys/class/drm"))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let number = name.to_str()?.strip_prefix("card")?;
            if !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let number = number.parse::<u32>().ok()?;
            let path = entry.path();
            path.join("gt/gt0/rc6_residency_ms")
                .is_file()
                .then_some((number, path))
        })
        .min()?;

    let rc6_ms = read_value::<u64>(&card.join("gt/gt0/rc6_residency_ms"));
    let usage = match (rc6_ms, old) {
        (Some(current), Some((old_card, old_ticks))) if elapsed > 0.0 && old_card == card => {
            current.checked_sub(old_ticks).map(|idle_ms| {
                (100.0 * (1.0 - idle_ms as f64 / (elapsed * 1000.0))).clamp(0.0, 100.0) as f32
            })
        }
        _ => None,
    };
    let gpu = Igpu {
        name: "Intel UHD Graphics".into(),
        usage,
        mhz: read_value(&card.join("gt_act_freq_mhz")),
        max_mhz: read_value(&card.join("gt_max_freq_mhz")),
    };
    *previous = rc6_ms.map(|ms| (card, ms));
    Some(gpu)
}

pub(super) fn nvidia(root: &Path, name: &mut Option<String>) -> Option<Nvidia> {
    let path = root.join("sys/bus/pci/devices").join(PCI_BUS_ID);
    if !path.exists() {
        return None;
    }
    let awake = fs::read_to_string(path.join("power/runtime_status"))
        .is_ok_and(|status| status.trim() == "active");
    let mut gpu = Nvidia {
        awake,
        name: name.as_deref().unwrap_or("NVIDIA GPU").to_owned(),
        ..Nvidia::default()
    };
    if awake && root == Path::new("/") {
        query_nvidia(root, &mut gpu, name);
    }
    Some(gpu)
}

fn query_nvidia(root: &Path, gpu: &mut Nvidia, name: &mut Option<String>) {
    static PIN_LIBRARY: Once = Once::new();
    PIN_LIBRARY.call_once(|| {
        // SAFETY: dlopen takes a static C string. Keep the handle open for process lifetime.
        unsafe {
            libc::dlopen(c"libnvidia-ml.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_NODELETE);
        }
    });

    // Drop the NVML session after each query so the GPU can return to runtime suspend.
    let Ok(nvml) = Nvml::init() else {
        return;
    };
    let Ok(device) = nvml.device_by_pci_bus_id(PCI_BUS_ID) else {
        return;
    };
    if let Ok(current_name) = device.name() {
        gpu.name.clone_from(&current_name);
        *name = Some(current_name);
    }
    if let Ok(utilization) = device.utilization_rates() {
        gpu.usage = Some(utilization.gpu.min(100) as f32);
        gpu.mem_usage = Some(utilization.memory.min(100) as f32);
    }
    if let Ok(memory) = device.memory_info() {
        gpu.mem_used_bytes = Some(memory.used);
        gpu.mem_total_bytes = Some(memory.total);
    }
    gpu.temp = device
        .temperature(TemperatureSensor::Gpu)
        .ok()
        .map(|c| c as f32);
    gpu.power_w = device.power_usage().ok().map(|mw| mw as f32 / 1000.0);
    gpu.power_limit_w = device
        .enforced_power_limit()
        .ok()
        .map(|mw| mw as f32 / 1000.0);
    gpu.mhz = device.clock_info(Clock::Graphics).ok();

    for (kind, processes) in [
        ("compute", device.running_compute_processes()),
        ("graphics", device.running_graphics_processes()),
    ] {
        for process in processes.unwrap_or_default() {
            let pid = process.pid;
            let name = fs::read(root.join(format!("proc/{pid}/comm")))
                .map(|text| String::from_utf8_lossy(&text).trim_end_matches('\n').to_owned())
                .unwrap_or_else(|_| "?".into());
            let mem_bytes = match process.used_gpu_memory {
                UsedGpuMemory::Used(bytes) => Some(bytes),
                UsedGpuMemory::Unavailable => None,
            };
            gpu.processes.push(GpuProcess {
                pid,
                name,
                kind: kind.into(),
                mem_bytes,
            });
        }
    }
}
