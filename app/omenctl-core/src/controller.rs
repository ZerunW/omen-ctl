//! Control loop. Call `tick(now)` once per second with a monotonic time in seconds.
//!
//! Safety rules encoded here (see PLAN.md):
//! - Fans are put in manual mode BEFORE switching platform profile, so the EC's
//!   120 s software window never runs with stale/zero fan targets.
//! - BIOS-auto fans are not allowed in Unleashed (the EC would drop the 90/130 W state).
//! - Any doubt (missing CPU temperature, fan write error, rpm mismatch, overheat)
//!   moves towards more cooling and less power. If fan writes fail, the kernel
//!   watchdog is deliberately NOT kicked so the driver forces max fans itself.

use crate::config::Config;
use crate::curve::{level_to_pwm, FanCurve, Smoother, LEVEL_MAX, LEVEL_MIN};
use crate::hw::{Hw, Sensors, PWM_AUTO, PWM_MANUAL};
use crate::proto::Status;
use crate::{FanMode, Mode};

/// Grace period after a fan target change before rpm is checked (fans take ~10 s to ramp).
const RPM_GRACE_S: f64 = 12.0;

pub struct Controller<H: Hw> {
    pub hw: H,
    cfg: Config,
    user_mode: Mode,
    effective_mode: Mode,
    fan_mode: FanMode,
    applied_profile: Option<String>,
    smoothers: [Smoother; 2],
    written: Option<[u8; 2]>,
    target: [u8; 2],
    last_target_change: f64,
    hot_since: Option<f64>,
    cool_since: Option<f64>,
    protect_drop: bool,
    rpm_bad_since: Option<f64>,
    fault: Option<String>,
    last_ac: Option<bool>,
    last_sensors: Sensors,
    errors: Vec<String>,
}

impl<H: Hw> Controller<H> {
    pub fn new(hw: H, cfg: Config, mode: Mode, fan: FanMode) -> Self {
        let fall = cfg.fan.fall;
        let fan = if mode == Mode::Unleashed && fan == FanMode::Auto { FanMode::Curve } else { fan };
        Controller {
            hw,
            cfg,
            user_mode: mode,
            effective_mode: mode,
            fan_mode: fan,
            applied_profile: None,
            smoothers: [Smoother::new(fall), Smoother::new(fall)],
            written: None,
            target: [LEVEL_MIN, LEVEL_MIN],
            last_target_change: f64::NEG_INFINITY,
            hot_since: None,
            cool_since: None,
            protect_drop: false,
            rpm_bad_since: None,
            fault: None,
            last_ac: None,
            last_sensors: Sensors::default(),
            errors: Vec::new(),
        }
    }

    pub fn user_mode(&self) -> Mode {
        self.user_mode
    }
    pub fn fan_mode(&self) -> FanMode {
        self.fan_mode
    }

    /// Hottest known temperature; None if the CPU sensor is missing (treated as unsafe).
    fn temp(s: &Sensors) -> Option<f32> {
        let cpu = s.cpu_temp?;
        Some(match s.gpu_temp {
            Some(g) => cpu.max(g),
            None => cpu,
        })
    }

    fn note<T>(&mut self, what: &str, r: std::io::Result<T>) -> bool {
        match r {
            Ok(_) => true,
            Err(e) => {
                self.errors.push(format!("{what}: {e}"));
                false
            }
        }
    }

    /// Compute the fan levels we want right now (before writing).
    fn desired_levels(&mut self, s: &Sensors) -> [u8; 2] {
        let t = Self::temp(s);
        let p = &self.cfg.protect;
        let force_max = self.fault.is_some() || t.map_or(true, |t| t >= p.fans_max_at);
        if force_max {
            // keep the smoothers in sync so we decay gracefully afterwards
            self.smoothers[0].step(LEVEL_MAX);
            self.smoothers[1].step(LEVEL_MAX);
            return [LEVEL_MAX, LEVEL_MAX];
        }
        let t = t.unwrap();
        match self.fan_mode {
            FanMode::Curve | FanMode::Auto => {
                let c = self.smoothers[0].step(self.cfg.fan.cpu_curve.eval(t));
                let g = self.smoothers[1].step(self.cfg.fan.gpu_curve.eval(t));
                [c, g]
            }
            FanMode::Fixed(l) => [l.clamp(LEVEL_MIN, LEVEL_MAX); 2],
            FanMode::Max => [LEVEL_MAX, LEVEL_MAX],
        }
    }

    /// True if the daemon (not BIOS) should be driving the fans this tick.
    fn manual_needed(&self, s: &Sensors) -> bool {
        if self.fan_mode != FanMode::Auto {
            return true;
        }
        // In BIOS-auto, still take over when hot, faulty or blind.
        self.fault.is_some() || Self::temp(s).map_or(true, |t| t >= self.cfg.protect.fans_max_at)
    }

    /// Write fan levels, switching the driver to manual first if needed.
    /// Returns false if any fan write failed.
    fn drive_fans(&mut self, levels: [u8; 2], now: f64) -> bool {
        let mut ok = true;
        if self.hw.pwm_enable().ok() != Some(PWM_MANUAL) {
            let r = self.hw.kick(); // arm the kernel watchdog before taking manual control
            self.note("kick", r);
            let r = self.hw.set_pwm_enable(PWM_MANUAL);
            ok &= self.note("pwm_enable=1", r);
            self.written = None;
        }
        if self.written != Some(levels) {
            let max = self.cfg.fan.table_max;
            let r = self.hw.set_pwm(level_to_pwm(levels[0], max), level_to_pwm(levels[1], max));
            ok &= self.note("pwm", r);
            if ok {
                if self.written.map_or(true, |w| levels[0] > w[0] || levels[1] > w[1]) {
                    self.last_target_change = now;
                }
                self.written = Some(levels);
            }
        }
        self.target = levels;
        ok
    }

    fn release_fans_to_bios(&mut self) {
        if self.hw.pwm_enable().ok() != Some(PWM_AUTO) {
            let r = self.hw.set_pwm_enable(PWM_AUTO); // driver holds current speed, then BIOS
            self.note("pwm_enable=2", r);
        }
        self.written = None;
        self.smoothers[0].reset();
        self.smoothers[1].reset();
        self.rpm_bad_since = None;
    }

    /// Apply a mode in the safe order: fans manual (if needed) -> profile -> extras.
    fn apply_mode(&mut self, mode: Mode, now: f64) {
        let s = self.last_sensors;
        if mode == Mode::Unleashed || self.manual_needed(&s) {
            let lv = self.desired_levels(&s);
            self.drive_fans(lv, now);
        }
        let prof = mode.platform_profile();
        let r = self.hw.set_platform_profile(prof);
        if self.note("platform_profile", r) {
            self.applied_profile = Some(prof.to_string());
        }
        if mode == Mode::Unleashed && self.cfg.unleashed_concurrent_75w {
            let r = self.hw.set_concurrent_75w();
            self.note("concurrent 75 W", r);
        }
        let rapl = match mode {
            Mode::PowerSaver => self.cfg.rapl.power_saver,
            Mode::Balanced => self.cfg.rapl.balanced,
            Mode::Unleashed => self.cfg.rapl.unleashed,
        };
        if let Some((pl1, pl2)) = rapl {
            let r = self.hw.set_rapl(pl1, pl2);
            self.note("rapl", r);
        }
        self.effective_mode = mode;
    }

    /// First call after start: read sensors and apply the starting mode.
    pub fn start(&mut self, now: f64) {
        self.last_sensors = self.hw.sensors();
        self.last_ac = self.last_sensors.ac_online;
        let m = self.user_mode;
        self.apply_mode(m, now);
    }

    pub fn set_mode(&mut self, mode: Mode, now: f64) {
        self.fault = None;
        self.protect_drop = false;
        self.hot_since = None;
        self.cool_since = None;
        self.user_mode = mode;
        if mode == Mode::Unleashed && self.fan_mode == FanMode::Auto {
            self.fan_mode = FanMode::Curve;
        }
        self.apply_mode(mode, now);
    }

    /// Re-run the full mode switch for the current state, e.g. after resume from
    /// sleep (the EC may have lost the 0x31 state, fan targets and the 75 W value).
    /// Fan values are rewritten even if they look unchanged.
    pub fn reapply(&mut self, now: f64) {
        self.last_sensors = self.hw.sensors();
        self.last_ac = self.last_sensors.ac_online;
        self.written = None;
        self.smoothers[0].reset();
        self.smoothers[1].reset();
        self.last_target_change = now; // give the fans time to ramp before the rpm check
        self.rpm_bad_since = None;
        let m = self.effective_mode;
        self.apply_mode(m, now);
    }

    pub fn curves(&self) -> (&FanCurve, &FanCurve) {
        (&self.cfg.fan.cpu_curve, &self.cfg.fan.gpu_curve)
    }

    /// Validate and use new fan curves (returns the normalised curves).
    pub fn set_curves(&mut self, cpu: FanCurve, gpu: FanCurve) -> Result<(FanCurve, FanCurve), String> {
        let mut c = self.cfg.clone();
        c.fan.cpu_curve = cpu;
        c.fan.gpu_curve = gpu;
        let c = c.validated()?;
        let out = (c.fan.cpu_curve.clone(), c.fan.gpu_curve.clone());
        self.cfg = c;
        self.smoothers[0].reset();
        self.smoothers[1].reset();
        Ok(out)
    }

    pub fn set_fan(&mut self, fan: FanMode, now: f64) -> Result<(), String> {
        let fan = match fan {
            FanMode::Fixed(l) if !(LEVEL_MIN..=LEVEL_MAX).contains(&l) => {
                return Err(format!("fixed fan level must be {LEVEL_MIN}-{LEVEL_MAX} (units of 100 rpm)"))
            }
            FanMode::Auto if self.effective_mode == Mode::Unleashed => {
                return Err("BIOS automatic fans are not allowed in Unleashed mode (the EC would leave the Unleashed state)".into())
            }
            f => f,
        };
        self.fan_mode = fan;
        self.smoothers[0].reset();
        self.smoothers[1].reset();
        let s = self.last_sensors;
        if self.manual_needed(&s) {
            let lv = self.desired_levels(&s);
            self.drive_fans(lv, now);
        } else {
            self.release_fans_to_bios();
        }
        Ok(())
    }

    /// One control step. Returns the current status.
    pub fn tick(&mut self, now: f64) -> Status {
        self.errors.clear();
        let s = self.hw.sensors();
        self.last_sensors = s;
        let t = Self::temp(&s);
        let p = self.cfg.protect.clone();

        // 1. Someone else (e.g. GNOME power menu via power-profiles-daemon) changed the profile.
        if let Ok(cur) = self.hw.platform_profile() {
            if self.applied_profile.as_deref().is_some_and(|a| a != cur) {
                match Mode::from_platform_profile(&cur) {
                    Some(m) if !self.protect_drop => {
                        self.errors.push(format!("platform profile was changed externally to {cur}"));
                        self.set_mode(m, now);
                    }
                    _ => {
                        let m = self.effective_mode;
                        self.apply_mode(m, now);
                    }
                }
            }
        }

        // 2. AC plugged/unplugged: the driver may have reset power limits; re-assert extras.
        if s.ac_online != self.last_ac {
            self.last_ac = s.ac_online;
            if self.effective_mode == Mode::Unleashed && self.cfg.unleashed_concurrent_75w {
                let r = self.hw.set_concurrent_75w();
                self.note("concurrent 75 W (AC change)", r);
            }
        }

        // 3. Thermal protection: drop out of Unleashed when it stays too hot, restore when cool.
        match t {
            Some(t) if t >= p.drop_at => {
                let since = *self.hot_since.get_or_insert(now);
                if now - since >= p.drop_after_s as f64 && self.effective_mode == Mode::Unleashed {
                    self.protect_drop = true;
                    self.apply_mode(Mode::Balanced, now);
                }
            }
            _ => self.hot_since = None,
        }
        if self.protect_drop {
            match t {
                Some(t) if t < p.restore_below => {
                    let since = *self.cool_since.get_or_insert(now);
                    if now - since >= p.restore_after_s as f64 {
                        self.protect_drop = false;
                        self.cool_since = None;
                        let m = self.user_mode;
                        self.apply_mode(m, now);
                    }
                }
                _ => self.cool_since = None,
            }
        }

        // 4. Fans.
        let mut fans_ok = true;
        if self.manual_needed(&s) {
            let lv = self.desired_levels(&s);
            fans_ok = self.drive_fans(lv, now);
        } else if self.written.is_some() || self.hw.pwm_enable().ok() == Some(PWM_MANUAL) {
            // back from a temporary takeover in BIOS-auto mode
            self.release_fans_to_bios();
        }

        // 5. Fan sanity: rpm must follow the requested level once the ramp time has passed.
        if let (Some(w), true) = (self.written, now - self.last_target_change >= RPM_GRACE_S) {
            let bad = (0..2).any(|i| match s.fan_rpm[i] {
                Some(rpm) => (rpm as f32) < p.rpm_ratio * w[i] as f32 * 100.0,
                None => true,
            });
            if bad {
                let since = *self.rpm_bad_since.get_or_insert(now);
                if now - since >= p.rpm_fail_after_s as f64 && self.fault.is_none() {
                    self.fault = Some(format!(
                        "fan speed fault: target {}/{} rpm, measured {}/{} rpm; fans forced to maximum",
                        w[0] as u32 * 100,
                        w[1] as u32 * 100,
                        s.fan_rpm[0].map_or("?".into(), |r| r.to_string()),
                        s.fan_rpm[1].map_or("?".into(), |r| r.to_string())
                    ));
                    if self.effective_mode == Mode::Unleashed {
                        self.apply_mode(Mode::Balanced, now);
                    }
                    let lv = self.desired_levels(&s);
                    fans_ok &= self.drive_fans(lv, now);
                }
            } else {
                self.rpm_bad_since = None;
            }
        }

        // 6. Kernel watchdog: only kick if we are really in control of the fans.
        if fans_ok {
            let r = self.hw.kick();
            self.note("kick", r);
        }

        self.status()
    }

    /// Levels the curves give for the current temperature, without smoothing
    /// (both fans at maximum while the CPU temperature is unknown).
    pub fn recommended(&self) -> [u8; 2] {
        match Self::temp(&self.last_sensors) {
            Some(t) => [self.cfg.fan.cpu_curve.eval(t), self.cfg.fan.gpu_curve.eval(t)],
            None => [LEVEL_MAX, LEVEL_MAX],
        }
    }

    pub fn status(&self) -> Status {
        let s = &self.last_sensors;
        let mut alerts = self.errors.clone();
        let rec = self.recommended();
        if let FanMode::Fixed(l) = self.fan_mode {
            if l < rec[0] || l < rec[1] {
                alerts.push(format!(
                    "fixed fan speed {} rpm is below the recommended {} rpm (CPU fan) / {} rpm (GPU fan) at {}",
                    l as u32 * 100,
                    rec[0] as u32 * 100,
                    rec[1] as u32 * 100,
                    Self::temp(s).map_or("unknown temperature".into(), |t| format!("{t:.0} °C"))
                ));
            }
        }
        if let Some(f) = &self.fault {
            alerts.push(f.clone());
        }
        if self.protect_drop {
            alerts.push(format!(
                "overheat protection: {} paused, resumes below {} °C",
                self.user_mode.label(),
                self.cfg.protect.restore_below
            ));
        }
        if s.cpu_temp.is_none() {
            alerts.push("CPU temperature unavailable: fans at maximum".into());
        }
        Status {
            mode: self.user_mode,
            effective_mode: self.effective_mode,
            fan: self.fan_mode,
            cpu_temp: s.cpu_temp,
            gpu_temp: s.gpu_temp,
            fan_rpm: s.fan_rpm,
            fan_target: self.target,
            fan_recommended: rec,
            cpu_power_w: None,
            gpu_power_w: None,
            ac_online: s.ac_online,
            alerts,
        }
    }

    /// Safe exit: max fans if hot, balanced profile, then hand fans back to BIOS
    /// (the patched driver holds the current speed until the EC takes over).
    pub fn shutdown(&mut self) {
        let s = self.hw.sensors();
        if Self::temp(&s).map_or(true, |t| t >= self.cfg.protect.restore_below) {
            let _ = self.hw.set_pwm_enable(PWM_MANUAL);
            let m = self.cfg.fan.table_max;
            let _ = self.hw.set_pwm(level_to_pwm(LEVEL_MAX, m), level_to_pwm(LEVEL_MAX, m));
        }
        let _ = self.hw.set_platform_profile(Mode::Balanced.platform_profile());
        let _ = self.hw.set_pwm_enable(PWM_AUTO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    /// Simulated machine: records every write in order.
    #[derive(Default)]
    struct Mock {
        s: Sensors,
        profile: String,
        pwm_enable: u8,
        pwm: [u8; 2],
        log: Vec<String>,
        kicks: u32,
        c75: u32,
        fail_pwm: bool,
    }

    impl Mock {
        fn new(cpu: f32) -> Self {
            Mock {
                s: Sensors { cpu_temp: Some(cpu), gpu_temp: None, fan_rpm: [Some(2600), Some(2400)], ac_online: Some(true) },
                profile: "balanced".into(),
                pwm_enable: 2,
                ..Default::default()
            }
        }
        /// Fans instantly follow the written pwm (driver emulation).
        fn follow(&mut self) {
            if self.pwm_enable == 1 {
                for i in 0..2 {
                    self.s.fan_rpm[i] = Some(crate::curve::driver_written_level(self.pwm[i], 18, 52) as u32 * 100);
                }
            }
        }
    }

    impl Hw for Mock {
        fn sensors(&mut self) -> Sensors {
            self.s
        }
        fn platform_profile(&self) -> io::Result<String> {
            Ok(self.profile.clone())
        }
        fn set_platform_profile(&mut self, p: &str) -> io::Result<()> {
            self.log.push(format!("profile={p}"));
            self.profile = p.into();
            Ok(())
        }
        fn pwm_enable(&self) -> io::Result<u8> {
            Ok(self.pwm_enable)
        }
        fn set_pwm_enable(&mut self, v: u8) -> io::Result<()> {
            self.log.push(format!("pwm_enable={v}"));
            self.pwm_enable = v;
            Ok(())
        }
        fn set_pwm(&mut self, c: u8, g: u8) -> io::Result<()> {
            if self.fail_pwm {
                return Err(io::Error::other("pwm write failed"));
            }
            self.log.push(format!("pwm={c}/{g}"));
            self.pwm = [c, g];
            Ok(())
        }
        fn kick(&mut self) -> io::Result<()> {
            self.kicks += 1;
            Ok(())
        }
        fn set_concurrent_75w(&mut self) -> io::Result<()> {
            self.log.push("c75".into());
            self.c75 += 1;
            Ok(())
        }
        fn set_rapl(&mut self, a: u32, b: u32) -> io::Result<()> {
            self.log.push(format!("rapl={a}/{b}"));
            Ok(())
        }
    }

    fn ctl(cpu: f32, mode: Mode, fan: FanMode) -> Controller<Mock> {
        let mut c = Controller::new(Mock::new(cpu), Config::default().validated().unwrap(), mode, fan);
        c.start(0.0);
        c
    }

    fn pos(log: &[String], what: &str) -> usize {
        log.iter().position(|l| l.starts_with(what)).unwrap_or_else(|| panic!("{what} not in {log:?}"))
    }

    #[test]
    fn unleashed_puts_fans_manual_before_profile_then_75w() {
        let c = ctl(55.0, Mode::Balanced, FanMode::Auto);
        let mut c = c;
        c.hw.log.clear();
        c.set_mode(Mode::Unleashed, 1.0);
        let l = &c.hw.log;
        assert!(pos(l, "pwm_enable=1") < pos(l, "profile=performance"), "{l:?}");
        assert!(pos(l, "pwm=") < pos(l, "profile=performance"), "{l:?}");
        assert!(pos(l, "profile=performance") < pos(l, "c75"), "{l:?}");
        assert_eq!(c.fan_mode(), FanMode::Curve, "auto must become curve in unleashed");
    }

    #[test]
    fn auto_fans_rejected_in_unleashed() {
        let mut c = ctl(55.0, Mode::Unleashed, FanMode::Curve);
        assert!(c.set_fan(FanMode::Auto, 1.0).is_err());
        assert!(c.set_fan(FanMode::Fixed(10), 1.0).is_err());
        assert!(c.set_fan(FanMode::Fixed(30), 1.0).is_ok());
    }

    #[test]
    fn curve_follows_temperature_and_kicks() {
        let mut c = ctl(50.0, Mode::Unleashed, FanMode::Curve);
        let st = c.tick(1.0);
        assert_eq!(st.fan_target, [24, 21]); // BIOS-table defaults at 50 C
        c.hw.s.cpu_temp = Some(85.0);
        let st = c.tick(2.0);
        assert_eq!(st.fan_target, [52, 52]);
        assert!(c.hw.kicks >= 2);
    }

    #[test]
    fn overheat_forces_max_even_in_bios_auto_then_releases() {
        let mut c = ctl(50.0, Mode::Balanced, FanMode::Auto);
        assert_eq!(c.hw.pwm_enable, 2);
        c.hw.s.cpu_temp = Some(93.0);
        let st = c.tick(1.0);
        assert_eq!(c.hw.pwm_enable, 1);
        assert_eq!(st.fan_target, [52, 52]);
        c.hw.s.cpu_temp = Some(60.0);
        c.tick(2.0);
        assert_eq!(c.hw.pwm_enable, 2, "returns fans to BIOS when cool");
    }

    #[test]
    fn sustained_95c_drops_unleashed_then_restores() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Curve);
        c.hw.s.cpu_temp = Some(96.0);
        for t in 1..=12 {
            c.hw.follow();
            c.tick(t as f64);
        }
        assert_eq!(c.hw.profile, "balanced");
        let st = c.status();
        assert_eq!(st.effective_mode, Mode::Balanced);
        assert_eq!(st.mode, Mode::Unleashed);
        assert!(!st.alerts.is_empty());
        c.hw.s.cpu_temp = Some(70.0);
        for t in 13..=45 {
            c.hw.follow();
            c.tick(t as f64);
        }
        assert_eq!(c.hw.profile, "performance");
        assert!(c.hw.c75 >= 2, "75 W re-applied after restore");
    }

    #[test]
    fn external_profile_change_is_adopted_safely() {
        let mut c = ctl(55.0, Mode::Balanced, FanMode::Auto);
        c.hw.log.clear();
        c.hw.profile = "performance".into(); // GNOME menu -> power-profiles-daemon
        c.tick(1.0);
        assert_eq!(c.user_mode(), Mode::Unleashed);
        assert_eq!(c.hw.pwm_enable, 1, "fans taken over");
        assert_eq!(c.hw.c75, 1);
        assert_eq!(c.fan_mode(), FanMode::Curve);
    }

    #[test]
    fn stuck_fan_triggers_fault() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Fixed(40));
        for t in 1..=30 {
            c.hw.s.fan_rpm = [Some(1200), Some(1200)]; // like the OmenMon #137 freeze
            c.tick(t as f64);
        }
        let st = c.status();
        assert!(st.alerts.iter().any(|a| a.contains("fan speed fault")), "{:?}", st.alerts);
        assert_eq!(st.fan_target, [52, 52]);
        assert_eq!(c.hw.profile, "balanced");
        c.set_mode(Mode::Unleashed, 40.0);
        assert!(c.status().alerts.iter().all(|a| !a.contains("fan speed fault")));
    }

    #[test]
    fn missing_cpu_temp_means_max_fans() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Curve);
        c.hw.s.cpu_temp = None;
        let st = c.tick(1.0);
        assert_eq!(st.fan_target, [52, 52]);
    }

    #[test]
    fn pwm_write_failure_stops_kicking() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Curve);
        let k = c.hw.kicks;
        c.hw.fail_pwm = true;
        c.hw.s.cpu_temp = Some(80.0); // forces a new target -> write fails
        c.tick(1.0);
        assert_eq!(c.hw.kicks, k, "no kick when fans can't be driven");
    }

    #[test]
    fn ac_change_reapplies_75w() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Curve);
        let n = c.hw.c75;
        c.hw.s.ac_online = Some(false);
        c.tick(1.0);
        assert_eq!(c.hw.c75, n + 1);
    }

    #[test]
    fn shutdown_returns_to_balanced_and_bios_fans() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Curve);
        c.shutdown();
        assert_eq!(c.hw.profile, "balanced");
        assert_eq!(c.hw.pwm_enable, 2);
    }

    #[test]
    fn reapply_after_resume_rewrites_everything_in_order() {
        let mut c = ctl(60.0, Mode::Unleashed, FanMode::Curve);
        c.tick(1.0);
        // EC lost state during sleep
        c.hw.profile = "balanced".into();
        c.hw.pwm_enable = 2;
        c.hw.log.clear();
        c.reapply(100.0);
        let l = &c.hw.log;
        assert!(pos(l, "pwm_enable=1") < pos(l, "profile=performance"), "{l:?}");
        assert!(pos(l, "pwm=") < pos(l, "profile=performance"), "{l:?}");
        assert!(pos(l, "profile=performance") < pos(l, "c75"), "{l:?}");
        assert_eq!(c.user_mode(), Mode::Unleashed);
        // the tick right after must not see an "external change"
        let st = c.tick(101.0);
        assert!(st.alerts.is_empty(), "{:?}", st.alerts);
    }

    #[test]
    fn reapply_rewrites_same_fan_values() {
        let mut c = ctl(60.0, Mode::Balanced, FanMode::Fixed(30));
        c.tick(1.0);
        c.hw.log.clear();
        c.reapply(50.0);
        assert!(c.hw.log.iter().any(|l| l.starts_with("pwm=")), "{:?}", c.hw.log);
    }

    #[test]
    fn set_curves_validates_and_applies() {
        use crate::curve::CurvePoint;
        let mut c = ctl(50.0, Mode::Unleashed, FanMode::Curve);
        let flat = |l| FanCurve { points: vec![CurvePoint { temp: 85.0, level: 52 }, CurvePoint { temp: 30.0, level: l }] };
        let (cpu, _) = c.set_curves(flat(40), flat(30)).unwrap();
        assert_eq!(cpu.points[0].temp, 30.0, "sorted");
        let st = c.tick(1.0);
        assert!(st.fan_target[0] >= 40, "{:?}", st.fan_target);
        let bad = FanCurve { points: vec![CurvePoint { temp: 50.0, level: 30 }] };
        assert!(c.set_curves(bad, flat(30)).is_err());
        assert_eq!(c.curves().0, &cpu, "failed update leaves old curve");
    }

    #[test]
    fn fixed_speed_below_recommended_warns_but_is_applied() {
        let mut c = ctl(70.0, Mode::Balanced, FanMode::Curve);
        let st = c.tick(1.0);
        let rec = st.fan_recommended;
        assert_eq!(rec, [c.curves().0.eval(70.0), c.curves().1.eval(70.0)]);
        assert!(st.alerts.is_empty(), "{:?}", st.alerts);
        c.set_fan(FanMode::Fixed(20), 2.0).unwrap();
        let st = c.tick(3.0);
        assert_eq!(st.fan_target, [20, 20], "user choice is respected");
        assert!(st.alerts.iter().any(|a| a.contains("below the recommended")), "{:?}", st.alerts);
        c.set_fan(FanMode::Fixed(52), 4.0).unwrap();
        let st = c.tick(5.0);
        assert!(st.alerts.is_empty(), "{:?}", st.alerts);
    }

    #[test]
    fn smoothing_slows_fan_decrease() {
        let mut c = ctl(85.0, Mode::Unleashed, FanMode::Curve);
        c.tick(1.0);
        c.hw.s.cpu_temp = Some(40.0);
        let st = c.tick(2.0);
        assert!(st.fan_target[0] > 30, "should decay, not drop to 18: {:?}", st.fan_target);
    }
}
