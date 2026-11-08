//! Telemetry helpers that do not drive any control decision except sleep detection:
//! CPU package power from RAPL energy counters, and resume-from-sleep detection.

use std::path::{Path, PathBuf};

/// CPU package power from the difference of two `energy_uj` readings.
pub struct RaplPower {
    dir: PathBuf,
    prev: Option<(u64, f64)>,
}

impl RaplPower {
    pub fn new(root: &Path) -> RaplPower {
        RaplPower { dir: root.join("sys/class/powercap/intel-rapl:0"), prev: None }
    }

    fn read(&self, name: &str) -> Option<u64> {
        std::fs::read_to_string(self.dir.join(name)).ok()?.trim().parse().ok()
    }

    /// Call once per tick with monotonic seconds. Returns watts (None on the first call
    /// or if the counter is unreadable, e.g. energy_uj is root-only and we are not root).
    pub fn sample(&mut self, now: f64) -> Option<f32> {
        let e = self.read("energy_uj");
        let Some(e) = e else {
            self.prev = None;
            return None;
        };
        let out = self.prev.and_then(|(pe, pt)| {
            let dt = now - pt;
            if dt <= 0.05 {
                return None;
            }
            let de = if e >= pe {
                e - pe
            } else {
                // counter wrapped at max_energy_range_uj
                self.read("max_energy_range_uj")?.checked_sub(pe)? + e
            };
            Some((de as f64 / 1e6 / dt) as f32)
        });
        self.prev = Some((e, now));
        out
    }
}

/// Seconds of CLOCK_BOOTTIME minus CLOCK_MONOTONIC: grows only while suspended.
pub fn suspended_seconds() -> f64 {
    fn get(clock: libc::clockid_t) -> f64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: valid pointer to a timespec; clock ids are constants.
        unsafe { libc::clock_gettime(clock, &mut ts) };
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
    }
    get(libc::CLOCK_BOOTTIME) - get(libc::CLOCK_MONOTONIC)
}

/// Reports true once after each suspend/resume (suspended time jumped by >= 2 s).
pub struct ResumeDetector {
    last: f64,
}

impl ResumeDetector {
    pub fn new(suspended: f64) -> Self {
        ResumeDetector { last: suspended }
    }

    pub fn check(&mut self, suspended: f64) -> bool {
        let jump = suspended - self.last;
        self.last = suspended;
        jump >= 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapl_power_and_wrap() {
        let root = std::env::temp_dir().join(format!("omenctld-rapl-{}", std::process::id()));
        let d = root.join("sys/class/powercap/intel-rapl:0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("max_energy_range_uj"), "1000000000\n").unwrap();
        std::fs::write(d.join("energy_uj"), "999000000\n").unwrap();
        let mut r = RaplPower::new(&root);
        assert_eq!(r.sample(0.0), None);
        std::fs::write(d.join("energy_uj"), "999045000\n").unwrap();
        assert_eq!(r.sample(1.0), Some(0.045));
        // 90 J over 1 s across the wrap: 955000 uJ to the top, then 89045000
        std::fs::write(d.join("energy_uj"), "89045000\n").unwrap();
        let w = r.sample(2.0).unwrap();
        assert!((w - 90.0).abs() < 0.01, "{w}");
        std::fs::remove_file(d.join("energy_uj")).unwrap();
        assert_eq!(r.sample(3.0), None);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn resume_detection() {
        let mut r = ResumeDetector::new(10.0);
        assert!(!r.check(10.001));
        assert!(!r.check(10.5));
        assert!(r.check(400.0));
        assert!(!r.check(400.0));
        // the real clocks are readable and non-negative
        assert!(suspended_seconds() >= -0.01);
    }
}
