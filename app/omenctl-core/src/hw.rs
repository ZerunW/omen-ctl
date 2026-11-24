//! Hardware access through standard sysfs files only.
//! `Hw` is the interface the controller uses; `SysfsHw` is the real implementation
//! (with a configurable root so tests can point it at a fake tree).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const PWM_MANUAL: u8 = 1;
pub const PWM_AUTO: u8 = 2;

const CHARGE_BEHAVIOUR: &str = "sys/class/power_supply/BAT0/charge_behaviour";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChargeBehaviour {
    #[default]
    Auto,
    Inhibit,
}

impl ChargeBehaviour {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Inhibit => "inhibit-charge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Full,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sensors {
    /// CPU package temperature (C).
    pub cpu_temp: Option<f32>,
    /// GPU temperature (C); None while the dGPU is runtime-suspended.
    pub gpu_temp: Option<f32>,
    pub fan_rpm: [Option<u32>; 2],
    pub ac_online: Option<bool>,
    /// Laptop battery charge (%).
    pub battery_percent: Option<u8>,
    pub battery_status: Option<BatteryStatus>,
    /// Busy share (0..1) of the busiest logical CPU since the previous reading.
    pub cpu_busy_max: Option<f32>,
}

pub trait Hw {
    fn sensors(&mut self) -> Sensors;
    fn platform_profile(&self) -> io::Result<String>;
    fn set_platform_profile(&mut self, p: &str) -> io::Result<()>;
    fn pwm_enable(&self) -> io::Result<u8>;
    fn set_pwm_enable(&mut self, v: u8) -> io::Result<()>;
    /// Set both fan pwm values (driver order: fan1 = CPU, fan2 = GPU).
    fn set_pwm(&mut self, cpu: u8, gpu: u8) -> io::Result<()>;
    /// Tell the kernel fan watchdog that userspace is alive.
    fn kick(&mut self) -> io::Result<()>;
    fn set_concurrent_75w(&mut self) -> io::Result<()>;
    fn set_rapl(&mut self, pl1_w: u32, pl2_w: u32) -> io::Result<()>;
    /// Current intel_rapl package PL1/PL2 (W).
    fn rapl(&self) -> io::Result<(u32, u32)>;
    /// CPU energy/performance preference (intel_pstate EPP) of cpu0.
    fn cpu_epp(&self) -> io::Result<String>;
    /// Set the EPP on every CPU.
    fn set_cpu_epp(&mut self, epp: &str) -> io::Result<()>;
    /// Current kernel value, or None if charge inhibition is unavailable.
    fn charge_behaviour(&self) -> Option<String>;
    fn set_charge_behaviour(&mut self, behaviour: ChargeBehaviour) -> io::Result<()>;
}

/// Paths relative to a root (normally "/").
pub struct SysfsHw {
    root: PathBuf,
    gpu: Option<Box<dyn FnMut() -> Option<f32> + Send>>,
    /// Per-CPU (busy, total) jiffies from the previous /proc/stat reading.
    prev_stat: Vec<(u64, u64)>,
    /// charge_behaviour write still running (started at, value). The firmware method
    /// behind it waits in a loop for the EC to acknowledge, so a write that never
    /// returns must not hold up the control loop.
    charge_pending: Arc<Mutex<Option<(Instant, ChargeBehaviour)>>>,
}

/// How long a charge_behaviour write may block the caller before it is left to
/// finish in the background.
const CHARGE_WRITE_WAIT: Duration = Duration::from_secs(2);

impl SysfsHw {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        SysfsHw { root: root.into(), gpu: None, prev_stat: Vec::new(), charge_pending: Arc::new(Mutex::new(None)) }
    }

    /// Install a GPU temperature reader (e.g. NVML). It is only called while the
    /// dGPU is runtime-active, so polling never wakes a sleeping GPU.
    pub fn with_gpu_reader(mut self, f: impl FnMut() -> Option<f32> + Send + 'static) -> Self {
        self.gpu = Some(Box::new(f));
        self
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn hpwmi(&self) -> PathBuf {
        self.p("sys/devices/platform/hp-wmi")
    }

    fn hp_hwmon(&self) -> io::Result<PathBuf> {
        first_dir(&self.hpwmi().join("hwmon"))
    }

    fn coretemp(&self) -> Option<PathBuf> {
        let dir = self.p("sys/class/hwmon");
        let mut v: Vec<_> = fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).collect();
        v.sort();
        v.into_iter().find(|h| read_trim(&h.join("name")).ok().as_deref() == Some("coretemp"))
    }

    /// Busy share of the busiest CPU since the last call, from /proc/stat.
    fn busiest_cpu(&mut self) -> Option<f32> {
        let text = fs::read_to_string(self.p("proc/stat")).ok()?;
        let now = parse_proc_stat(&text);
        let r = (self.prev_stat.len() == now.len())
            .then(|| {
                now.iter()
                    .zip(&self.prev_stat)
                    .filter_map(|(&(b, t), &(pb, pt))| (t > pt).then(|| b.saturating_sub(pb) as f32 / (t - pt) as f32))
                    .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
            })
            .flatten();
        self.prev_stat = now;
        r
    }

    fn gpu_active(&self) -> bool {
        read_trim(&self.p("sys/bus/pci/devices/0000:01:00.0/power/runtime_status"))
            .map(|s| s == "active")
            .unwrap_or(false)
    }
}

fn first_dir(dir: &Path) -> io::Result<PathBuf> {
    let mut v: Vec<_> = fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
    v.sort();
    v.into_iter().next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("nothing in {}", dir.display())))
}

/// (busy, total) jiffies of each "cpuN" line; idle and iowait count as not busy.
pub fn parse_proc_stat(text: &str) -> Vec<(u64, u64)> {
    text.lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .map(|l| {
            let v: Vec<u64> = l.split_whitespace().skip(1).take(8).filter_map(|x| x.parse().ok()).collect();
            let total: u64 = v.iter().sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            (total - idle, total)
        })
        .collect()
}

pub fn read_trim(p: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(p)?.trim().to_string())
}

fn write(p: &Path, v: &str) -> io::Result<()> {
    fs::write(p, v).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", p.display())))
}

impl Hw for SysfsHw {
    fn sensors(&mut self) -> Sensors {
        let cpu_temp = self
            .coretemp()
            .and_then(|h| read_trim(&h.join("temp1_input")).ok())
            .and_then(|s| s.parse::<f32>().ok())
            .map(|m| m / 1000.0);
        let gpu_temp = if self.gpu_active() { self.gpu.as_mut().and_then(|f| f()) } else { None };
        let mut fan_rpm = [None, None];
        if let Ok(h) = self.hp_hwmon() {
            for (i, slot) in fan_rpm.iter_mut().enumerate() {
                *slot = read_trim(&h.join(format!("fan{}_input", i + 1))).ok().and_then(|s| s.parse().ok());
            }
        }
        let ac_online = read_trim(&self.p("sys/class/power_supply/ADP1/online")).ok().map(|s| s == "1");
        let battery = crate::power::read(&self.root).battery;
        let battery_percent = battery.as_ref().and_then(|b| b.percent);
        let battery_status = match battery.as_ref().map(|b| b.status.as_str()) {
            Some("Charging") => Some(BatteryStatus::Charging),
            Some("Full") => Some(BatteryStatus::Full),
            _ => None,
        };
        let cpu_busy_max = self.busiest_cpu();
        Sensors { cpu_temp, gpu_temp, fan_rpm, ac_online, battery_percent, battery_status, cpu_busy_max }
    }

    fn charge_behaviour(&self) -> Option<String> {
        let text = read_trim(&self.p(CHARGE_BEHAVIOUR)).ok()?;
        let mut values = text.split_whitespace();
        if !values.clone().any(|v| matches!(v, "inhibit-charge" | "[inhibit-charge]")) {
            return None;
        }
        values.find_map(|v| v.strip_prefix('[')?.strip_suffix(']')).map(String::from)
    }

    fn set_charge_behaviour(&mut self, behaviour: ChargeBehaviour) -> io::Result<()> {
        let path = self.p(CHARGE_BEHAVIOUR);
        {
            let mut pending = self.charge_pending.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((since, b)) = *pending {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("writing {} has not returned for {} s (firmware not answering)", b.as_str(), since.elapsed().as_secs()),
                ));
            }
            *pending = Some((Instant::now(), behaviour));
        }
        let (tx, rx) = mpsc::channel();
        let pending = self.charge_pending.clone();
        let spawned = std::thread::Builder::new().name("charge-write".into()).spawn(move || {
            let r = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, behaviour.as_str().as_bytes()))
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())));
            *pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = tx.send(r);
        });
        if let Err(e) = spawned {
            *self.charge_pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(e);
        }
        match rx.recv_timeout(CHARGE_WRITE_WAIT) {
            Ok(r) => r,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("writing {} did not return within {} s (firmware not answering)", behaviour.as_str(), CHARGE_WRITE_WAIT.as_secs()),
            )),
        }
    }

    fn platform_profile(&self) -> io::Result<String> {
        read_trim(&self.p("sys/firmware/acpi/platform_profile"))
    }

    fn set_platform_profile(&mut self, p: &str) -> io::Result<()> {
        write(&self.p("sys/firmware/acpi/platform_profile"), p)
    }

    fn pwm_enable(&self) -> io::Result<u8> {
        read_trim(&self.hp_hwmon()?.join("pwm1_enable"))?
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("pwm1_enable: {e}")))
    }

    fn set_pwm_enable(&mut self, v: u8) -> io::Result<()> {
        write(&self.hp_hwmon()?.join("pwm1_enable"), &v.to_string())
    }

    fn set_pwm(&mut self, cpu: u8, gpu: u8) -> io::Result<()> {
        let h = self.hp_hwmon()?;
        write(&h.join("pwm1"), &cpu.to_string())?;
        write(&h.join("pwm2"), &gpu.to_string())
    }

    fn kick(&mut self) -> io::Result<()> {
        write(&self.hpwmi().join("omenctl_kick"), "1")
    }

    fn set_concurrent_75w(&mut self) -> io::Result<()> {
        write(&self.hpwmi().join("omenctl_concurrent_tdp"), "75")
    }

    fn set_rapl(&mut self, pl1_w: u32, pl2_w: u32) -> io::Result<()> {
        let z = self.p("sys/class/powercap/intel-rapl:0");
        write(&z.join("constraint_0_power_limit_uw"), &(pl1_w as u64 * 1_000_000).to_string())?;
        write(&z.join("constraint_1_power_limit_uw"), &(pl2_w as u64 * 1_000_000).to_string())
    }

    fn rapl(&self) -> io::Result<(u32, u32)> {
        let z = self.p("sys/class/powercap/intel-rapl:0");
        let w = |f: &str| -> io::Result<u32> {
            let uw: u64 = read_trim(&z.join(f))?.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{f}: {e}")))?;
            Ok((uw / 1_000_000) as u32)
        };
        Ok((w("constraint_0_power_limit_uw")?, w("constraint_1_power_limit_uw")?))
    }

    fn cpu_epp(&self) -> io::Result<String> {
        read_trim(&self.p("sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"))
    }

    fn set_cpu_epp(&mut self, epp: &str) -> io::Result<()> {
        let dir = self.p("sys/devices/system/cpu");
        let mut n = 0;
        for e in fs::read_dir(&dir)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("cpu") && name[3..].chars().all(|c| c.is_ascii_digit()) && name.len() > 3) {
                continue;
            }
            let f = e.path().join("cpufreq/energy_performance_preference");
            if f.exists() {
                write(&f, epp)?;
                n += 1;
            }
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no energy_performance_preference files"));
        }
        Ok(())
    }
}

/// Extra telemetry for status display (not used for control decisions).
pub fn read_cpu_energy_uj(root: &Path) -> Option<u64> {
    read_trim(&root.join("sys/class/powercap/intel-rapl:0/energy_uj")).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_busy() {
        let a = "cpu  10 0 10 80 0 0 0 0 0 0\ncpu0 5 0 5 40 0 0 0 0 0 0\ncpu1 5 0 5 40 0 0 0 0 0 0\nintr 1\n";
        assert_eq!(parse_proc_stat(a), vec![(10, 50), (10, 50)]);
        let d = std::env::temp_dir().join(format!("omenctl-stat-{}-{}", std::process::id(), rand_suffix()));
        fs::create_dir_all(d.join("proc")).unwrap();
        fs::write(d.join("proc/stat"), a).unwrap();
        let mut hw = SysfsHw::new(&d);
        assert_eq!(hw.busiest_cpu(), None, "first reading has nothing to compare with");
        // cpu0: 90 of 100 new jiffies busy, cpu1: 10 of 100
        fs::write(d.join("proc/stat"), "cpu0 50 0 50 50 0 0 0 0\ncpu1 10 0 5 130 0 0 0 0\n").unwrap();
        assert!((hw.busiest_cpu().unwrap() - 0.9).abs() < 1e-6);
        fs::remove_dir_all(d).ok();
    }

    fn tree() -> PathBuf {
        let d = std::env::temp_dir().join(format!("omenctl-hw-{}-{}", std::process::id(), rand_suffix()));
        let mk = |rel: &str, v: &str| {
            let p = d.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, v).unwrap();
        };
        mk("sys/firmware/acpi/platform_profile", "balanced\n");
        mk("sys/devices/platform/hp-wmi/hwmon/hwmon7/pwm1_enable", "2\n");
        mk("sys/devices/platform/hp-wmi/hwmon/hwmon7/pwm1", "0\n");
        mk("sys/devices/platform/hp-wmi/hwmon/hwmon7/pwm2", "0\n");
        mk("sys/devices/platform/hp-wmi/hwmon/hwmon7/fan1_input", "2600\n");
        mk("sys/devices/platform/hp-wmi/hwmon/hwmon7/fan2_input", "2400\n");
        mk("sys/devices/platform/hp-wmi/omenctl_kick", "");
        mk("sys/devices/platform/hp-wmi/omenctl_concurrent_tdp", "");
        mk("sys/class/hwmon/hwmon1/name", "acpi_fan\n");
        mk("sys/class/hwmon/hwmon9/name", "coretemp\n");
        mk("sys/class/hwmon/hwmon9/temp1_input", "56000\n");
        mk("sys/class/power_supply/ADP1/online", "1\n");
        mk("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "suspended\n");
        mk("sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference", "performance\n");
        mk("sys/devices/system/cpu/cpu1/cpufreq/energy_performance_preference", "performance\n");
        mk("sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference", "performance\n");
        d
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn reads_and_writes_fake_tree() {
        let root = tree();
        let mut hw = SysfsHw::new(&root).with_gpu_reader(|| Some(70.0));
        let s = hw.sensors();
        assert_eq!(s.cpu_temp, Some(56.0));
        assert_eq!(s.gpu_temp, None, "suspended GPU must not be queried");
        assert_eq!(s.fan_rpm, [Some(2600), Some(2400)]);
        assert_eq!(s.ac_online, Some(true));
        assert_eq!(hw.platform_profile().unwrap(), "balanced");
        hw.set_pwm_enable(PWM_MANUAL).unwrap();
        hw.set_pwm(221, 200).unwrap();
        hw.kick().unwrap();
        hw.set_concurrent_75w().unwrap();
        let h = root.join("sys/devices/platform/hp-wmi");
        assert_eq!(read_trim(&h.join("hwmon/hwmon7/pwm1_enable")).unwrap(), "1");
        assert_eq!(read_trim(&h.join("hwmon/hwmon7/pwm2")).unwrap(), "200");
        assert_eq!(read_trim(&h.join("omenctl_concurrent_tdp")).unwrap(), "75");
        hw.set_cpu_epp("balance_performance").unwrap();
        assert_eq!(hw.cpu_epp().unwrap(), "balance_performance");
        assert_eq!(read_trim(&root.join("sys/devices/system/cpu/cpu1/cpufreq/energy_performance_preference")).unwrap(), "balance_performance");
        fs::write(root.join("sys/bus/pci/devices/0000:01:00.0/power/runtime_status"), "active").unwrap();
        assert_eq!(hw.sensors().gpu_temp, Some(70.0));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reads_and_writes_charge_behaviour() {
        let root = tree();
        let mut hw = SysfsHw::new(&root);
        let behaviour = root.join(CHARGE_BEHAVIOUR);
        assert_eq!(hw.charge_behaviour(), None);
        assert!(hw.set_charge_behaviour(ChargeBehaviour::Auto).is_err());
        fs::create_dir_all(behaviour.parent().unwrap()).unwrap();
        fs::write(&behaviour, "[auto] force-discharge\n").unwrap();
        assert_eq!(hw.charge_behaviour(), None);
        for (options, current) in [
            ("[auto] inhibit-charge force-discharge\n", "auto"),
            ("auto [inhibit-charge] force-discharge\n", "inhibit-charge"),
            ("auto inhibit-charge [force-discharge]\n", "force-discharge"),
        ] {
            fs::write(&behaviour, options).unwrap();
            assert_eq!(hw.charge_behaviour().as_deref(), Some(current));
        }
        hw.set_charge_behaviour(ChargeBehaviour::Inhibit).unwrap();
        assert_eq!(read_trim(&behaviour).unwrap(), "inhibit-charge");
        hw.set_charge_behaviour(ChargeBehaviour::Auto).unwrap();
        assert_eq!(read_trim(&behaviour).unwrap(), "auto");
        fs::remove_dir_all(root).ok();
    }
}
