//! Fan curve: piecewise-linear temperature -> fan level (units of 100 rpm),
//! rounded up and clamped to the BIOS fan table range, plus asymmetric smoothing.

use serde::{Deserialize, Serialize};

/// Lowest and highest level in the 8D3F BIOS fan table (0x2F), in 100 rpm.
pub const LEVEL_MIN: u8 = 18;
pub const LEVEL_MAX: u8 = 52;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temp: f32,
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanCurve {
    pub points: Vec<CurvePoint>,
}

impl FanCurve {
    /// Validate and normalise: sort by temperature, clamp levels, require >= 2 points
    /// and non-decreasing levels (a hotter point may never ask for less cooling).
    pub fn validated(mut self) -> Result<Self, String> {
        if self.points.len() < 2 {
            return Err("curve needs at least 2 points".into());
        }
        for p in &self.points {
            if !p.temp.is_finite() || p.temp < 0.0 || p.temp > 110.0 {
                return Err(format!("bad temperature {}", p.temp));
            }
        }
        self.points.sort_by(|a, b| a.temp.partial_cmp(&b.temp).unwrap());
        for p in &mut self.points {
            p.level = p.level.clamp(LEVEL_MIN, LEVEL_MAX);
        }
        for w in self.points.windows(2) {
            if w[1].level < w[0].level {
                return Err("curve levels must not decrease as temperature rises".into());
            }
            if w[1].temp == w[0].temp {
                return Err("duplicate temperature in curve".into());
            }
        }
        Ok(self)
    }

    /// Piecewise-linear interpolation, rounded up, clamped to [LEVEL_MIN, LEVEL_MAX].
    pub fn eval(&self, temp: f32) -> u8 {
        let pts = &self.points;
        let raw = if temp <= pts[0].temp {
            pts[0].level as f32
        } else if temp >= pts[pts.len() - 1].temp {
            pts[pts.len() - 1].level as f32
        } else {
            let mut v = pts[pts.len() - 1].level as f32;
            for w in pts.windows(2) {
                if temp >= w[0].temp && temp <= w[1].temp {
                    let t = (temp - w[0].temp) / (w[1].temp - w[0].temp);
                    v = w[0].level as f32 + t * (w[1].level as f32 - w[0].level as f32);
                    break;
                }
            }
            v
        };
        (raw.ceil() as i32).clamp(LEVEL_MIN as i32, LEVEL_MAX as i32) as u8
    }

    /// BIOS fan table rows (0x2F on BIOS F.14), CPU fan column. The table has no
    /// temperatures; these breakpoints are provisional until calibrated against
    /// BIOS-auto behaviour, with >= 85 C mapped to the top row.
    pub fn bios_default_cpu() -> Self {
        Self::from_pairs(&[
            (40.0, 18), (45.0, 21), (50.0, 24), (55.0, 26), (60.0, 28), (64.0, 29),
            (68.0, 33), (72.0, 36), (76.0, 42), (79.0, 45), (82.0, 48), (85.0, 52),
        ])
    }

    /// GPU fan column of the same table (levels below 18 raised to 18 by the driver).
    pub fn bios_default_gpu() -> Self {
        Self::from_pairs(&[
            (40.0, 18), (45.0, 19), (50.0, 21), (55.0, 24), (60.0, 26), (64.0, 32),
            (68.0, 35), (72.0, 39), (76.0, 44), (79.0, 47), (82.0, 51), (85.0, 52),
        ])
    }

    fn from_pairs(p: &[(f32, u8)]) -> Self {
        FanCurve {
            points: p
                .iter()
                .map(|&(temp, level)| CurvePoint { temp, level: level.max(LEVEL_MIN) })
                .collect(),
        }
    }
}

/// Fan level smoothing: follow increases immediately; after the last increase
/// hold the level for `hold_s` seconds, then come down by at most `fall_per_s`
/// levels per second (1 level = 100 rpm). Output rounded up.
#[derive(Debug, Clone)]
pub struct Smoother {
    pub fall_per_s: f32,
    pub hold_s: f64,
    value: Option<f32>,
    last_rise: f64,
    last_step: f64,
}

impl Smoother {
    pub fn new(fall_per_s: f32, hold_s: f64) -> Self {
        Smoother { fall_per_s: fall_per_s.max(0.01), hold_s: hold_s.max(0.0), value: None, last_rise: 0.0, last_step: 0.0 }
    }

    pub fn reset(&mut self) {
        self.value = None;
    }

    /// `now` is monotonic seconds.
    pub fn step(&mut self, target: u8, now: f64) -> u8 {
        let t = target as f32;
        let v = match self.value {
            None => {
                self.last_rise = now;
                t
            }
            Some(prev) if t >= prev => {
                self.last_rise = now;
                t
            }
            Some(prev) if now - self.last_rise < self.hold_s => prev,
            Some(prev) => {
                let dt = (now - self.last_step).clamp(0.0, 5.0) as f32;
                (prev - self.fall_per_s * dt).max(t)
            }
        };
        self.value = Some(v);
        self.last_step = now;
        (v.ceil() as i32).clamp(LEVEL_MIN as i32, LEVEL_MAX as i32) as u8
    }
}

/// Exact emulation of the hp-wmi driver path for a pwm write (all integer floor):
/// rpm = pwm*max/255, clamped to [min,max]; stored = rpm*255/max; written = stored*max/255.
/// The double conversion usually loses one level (e.g. pwm 221 -> 44, not 45).
pub fn driver_written_level(pwm: u8, min_level: u8, max_level: u8) -> u8 {
    let max = max_level as u32;
    let rpm = (pwm as u32 * max / 255).clamp(min_level as u32, max);
    let stored = rpm * 255 / max;
    (stored * max / 255) as u8
}

/// Smallest pwm value for which the driver actually writes at least `level`
/// (clamped to [LEVEL_MIN, max_level]).
pub fn level_to_pwm(level: u8, max_level: u8) -> u8 {
    let level = level.clamp(LEVEL_MIN, max_level);
    (0..=255u8)
        .find(|&p| driver_written_level(p, LEVEL_MIN, max_level) >= level)
        .unwrap_or(255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(p: &[(f32, u8)]) -> FanCurve {
        FanCurve { points: p.iter().map(|&(t, l)| CurvePoint { temp: t, level: l }).collect() }
            .validated()
            .unwrap()
    }

    #[test]
    fn interpolates_and_rounds_up() {
        let k = c(&[(50.0, 20), (60.0, 30)]);
        assert_eq!(k.eval(55.0), 25);
        assert_eq!(k.eval(55.1), 26); // 25.1 -> ceil 26
        assert_eq!(k.eval(10.0), 20);
        assert_eq!(k.eval(99.0), 30);
    }

    #[test]
    fn clamps_to_table_range() {
        let k = c(&[(30.0, 5), (90.0, 80)]);
        assert_eq!(k.eval(0.0), LEVEL_MIN);
        assert_eq!(k.eval(100.0), LEVEL_MAX);
    }

    #[test]
    fn rejects_decreasing_or_tiny_curves() {
        let bad = FanCurve { points: vec![CurvePoint { temp: 50.0, level: 30 }, CurvePoint { temp: 60.0, level: 20 }] };
        assert!(bad.validated().is_err());
        let one = FanCurve { points: vec![CurvePoint { temp: 50.0, level: 30 }] };
        assert!(one.validated().is_err());
    }

    #[test]
    fn bios_defaults_are_valid_and_hit_max_at_85() {
        let cpu = FanCurve::bios_default_cpu().validated().unwrap();
        let gpu = FanCurve::bios_default_gpu().validated().unwrap();
        assert_eq!(cpu.eval(85.0), 52);
        assert_eq!(gpu.eval(90.0), 52);
        assert_eq!(cpu.eval(30.0), 18);
    }

    #[test]
    fn smoother_rises_fast_holds_then_falls_slowly() {
        let mut s = Smoother::new(0.5, 15.0);
        assert_eq!(s.step(30, 0.0), 30);
        assert_eq!(s.step(50, 1.0), 50); // immediate rise
        assert_eq!(s.step(20, 2.0), 50); // held
        assert_eq!(s.step(20, 15.9), 50); // still held
        assert_eq!(s.step(20, 16.0), 50); // hold over, falls from here
        assert_eq!(s.step(20, 18.0), 49); // 50 - 0.5*2
        assert_eq!(s.step(20, 20.0), 48);
        assert_eq!(s.step(45, 21.0), 48, "below current: keeps falling slowly");
        assert_eq!(s.step(49, 22.0), 49, "rise is immediate and restarts the hold");
        assert_eq!(s.step(20, 30.0), 49);
        // never below the target
        let mut s = Smoother::new(5.0, 0.0);
        s.step(40, 0.0);
        assert_eq!(s.step(38, 10.0), 38);
    }

    #[test]
    fn driver_emulation_matches_observed() {
        // 2026-11-08: pwm 221 measured 4400 rpm, pwm 128 measured 2500 rpm, pwm 255 -> 5200
        assert_eq!(driver_written_level(221, 18, 52), 44);
        assert_eq!(driver_written_level(128, 18, 52), 25);
        assert_eq!(driver_written_level(255, 18, 52), 52);
    }

    #[test]
    fn pwm_gives_level_or_one_above() {
        // The driver's double floor conversion makes some levels unreachable exactly
        // (e.g. 51 -> 52); we always land on the level or one step more cooling.
        for level in LEVEL_MIN..=LEVEL_MAX {
            let pwm = level_to_pwm(level, 52);
            let got = driver_written_level(pwm, 18, 52);
            assert!(got == level || got == level + 1, "level {level} pwm {pwm} got {got}");
        }
    }
}
