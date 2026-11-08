//! Daemon configuration (/etc/omenctl/config.toml). Every field has a safe default,
//! so a missing or partial file still yields a complete config.

use crate::curve::{FanCurve, LEVEL_MAX};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Mode applied at daemon start if no saved state exists.
    pub default_mode: crate::Mode,
    /// Write the 75 W CPU+GPU concurrent target in Unleashed mode.
    pub unleashed_concurrent_75w: bool,
    /// Optional intel_rapl PL1/PL2 (W) per mode; None = leave untouched.
    pub rapl: RaplConfig,
    pub fan: FanConfig,
    pub protect: ProtectConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaplConfig {
    pub power_saver: Option<(u32, u32)>,
    pub balanced: Option<(u32, u32)>,
    pub unleashed: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FanConfig {
    pub cpu_curve: FanCurve,
    pub gpu_curve: FanCurve,
    /// Down-smoothing factor (0..1]; rises are followed immediately.
    pub fall: f32,
    /// Highest level in the BIOS fan table (driver maps pwm 255 to this).
    pub table_max: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtectConfig {
    /// Force fans to max at or above this temperature (C).
    pub fans_max_at: f32,
    /// Leave Unleashed when at or above this temperature for `drop_after_s`.
    pub drop_at: f32,
    pub drop_after_s: u64,
    /// Return to the user's mode once below this temperature for `restore_after_s`.
    pub restore_below: f32,
    pub restore_after_s: u64,
    /// Fan check: actual rpm below `rpm_ratio` x target for `rpm_fail_after_s` = fault.
    pub rpm_ratio: f32,
    pub rpm_fail_after_s: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_mode: crate::Mode::Balanced,
            unleashed_concurrent_75w: true,
            rapl: RaplConfig::default(),
            fan: FanConfig::default(),
            protect: ProtectConfig::default(),
        }
    }
}

impl Default for RaplConfig {
    fn default() -> Self {
        RaplConfig { power_saver: None, balanced: None, unleashed: None }
    }
}

impl Default for FanConfig {
    fn default() -> Self {
        FanConfig {
            cpu_curve: FanCurve::bios_default_cpu(),
            gpu_curve: FanCurve::bios_default_gpu(),
            fall: 0.4,
            table_max: LEVEL_MAX,
        }
    }
}

impl Default for ProtectConfig {
    fn default() -> Self {
        ProtectConfig {
            fans_max_at: 92.0,
            drop_at: 95.0,
            drop_after_s: 10,
            restore_below: 80.0,
            restore_after_s: 30,
            rpm_ratio: 0.7,
            rpm_fail_after_s: 10,
        }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Self, String> {
        let c: Config = toml::from_str(s).map_err(|e| e.to_string())?;
        c.validated()
    }

    pub fn validated(mut self) -> Result<Self, String> {
        self.fan.cpu_curve = self.fan.cpu_curve.validated().map_err(|e| format!("cpu_curve: {e}"))?;
        self.fan.gpu_curve = self.fan.gpu_curve.validated().map_err(|e| format!("gpu_curve: {e}"))?;
        if !(0.01..=1.0).contains(&self.fan.fall) {
            return Err("fan.fall must be in 0.01..=1.0".into());
        }
        if self.fan.table_max != LEVEL_MAX {
            return Err(format!("fan.table_max must be {LEVEL_MAX} on 8D3F"));
        }
        let p = &self.protect;
        if !(p.fans_max_at <= p.drop_at && p.restore_below < p.fans_max_at && p.drop_at <= 100.0) {
            return Err("protect: need restore_below < fans_max_at <= drop_at <= 100".into());
        }
        if !(0.1..=1.0).contains(&p.rpm_ratio) {
            return Err("protect.rpm_ratio must be in 0.1..=1.0".into());
        }
        for (name, v) in [("power_saver", self.rapl.power_saver), ("balanced", self.rapl.balanced), ("unleashed", self.rapl.unleashed)] {
            if let Some((pl1, pl2)) = v {
                if pl1 == 0 || pl2 < pl1 || pl2 > 130 {
                    return Err(format!("rapl.{name}: need 0 < PL1 <= PL2 <= 130"));
                }
            }
        }
        Ok(self)
    }
}

/// Shortest decimal form of an f32, as f64 (40.5f32 -> 40.5, not 40.500000xxx).
fn clean_f64(v: f32) -> f64 {
    format!("{v}").parse().unwrap_or(v as f64)
}

fn curve_item(c: &FanCurve) -> toml_edit::Item {
    let mut arr = toml_edit::Array::new();
    for p in &c.points {
        let mut t = toml_edit::InlineTable::new();
        t.insert("temp", clean_f64(p.temp).into());
        t.insert("level", (p.level as i64).into());
        arr.push(t);
        arr.iter_mut().last().unwrap().decor_mut().set_prefix("\n  ");
    }
    arr.set_trailing("\n");
    arr.set_trailing_comma(true);
    let mut tbl = toml_edit::Table::new();
    tbl.insert("points", toml_edit::value(arr));
    toml_edit::Item::Table(tbl)
}

/// Return `existing` (the text of config.toml, may be empty) with `[fan.cpu_curve]`
/// and `[fan.gpu_curve]` replaced, keeping every other line and comment.
/// The result is validated as a whole before it is returned.
pub fn with_curves(existing: &str, cpu: &FanCurve, gpu: &FanCurve) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|e| format!("config.toml: {e}"))?;
    let fan = doc.entry("fan").or_insert(toml_edit::table());
    let fan = fan.as_table_mut().ok_or("config.toml: [fan] is not a table")?;
    fan.set_implicit(true);
    fan.insert("cpu_curve", curve_item(cpu));
    fan.insert("gpu_curve", curve_item(gpu));
    let out = doc.to_string();
    let c = Config::from_toml(&out)?;
    if &c.fan.cpu_curve != cpu || &c.fan.gpu_curve != gpu {
        return Err("internal error: curves did not round-trip".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_curves_keeps_comments_and_round_trips() {
        let orig = "# my comment\ndefault_mode = \"unleashed\"\n\n[protect]\nfans_max_at = 90.0 # keep\n";
        let cpu = FanCurve::bios_default_cpu().validated().unwrap();
        let mut gpu = FanCurve::bios_default_gpu().validated().unwrap();
        gpu.points[0].temp = 40.5;
        let out = with_curves(orig, &cpu, &gpu).unwrap();
        assert!(out.contains("# my comment") && out.contains("# keep"), "{out}");
        assert!(out.contains("temp = 40.5,"), "{out}");
        let c = Config::from_toml(&out).unwrap();
        assert_eq!(c.default_mode, crate::Mode::Unleashed);
        assert_eq!(c.protect.fans_max_at, 90.0);
        assert_eq!(c.fan.gpu_curve, gpu);
        // replacing again does not duplicate tables
        let out2 = with_curves(&out, &cpu, &cpu).unwrap();
        assert_eq!(out2.matches("[fan.cpu_curve]").count(), 1, "{out2}");
        assert_eq!(Config::from_toml(&out2).unwrap().fan.gpu_curve, cpu);
        // empty file works too
        assert!(with_curves("", &cpu, &gpu).is_ok());
    }

    #[test]
    fn with_curves_rejects_bad_curve() {
        let bad = FanCurve { points: vec![crate::curve::CurvePoint { temp: 50.0, level: 30 }] };
        let ok = FanCurve::bios_default_cpu();
        assert!(with_curves("", &bad, &ok).is_err());
    }

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c, Config::default().validated().unwrap());
        assert!(c.unleashed_concurrent_75w);
    }

    #[test]
    fn partial_override() {
        let c = Config::from_toml("[protect]\nfans_max_at = 90.0\n").unwrap();
        assert_eq!(c.protect.fans_max_at, 90.0);
        assert_eq!(c.protect.drop_at, 95.0);
    }

    #[test]
    fn rejects_unsafe_values() {
        assert!(Config::from_toml("[protect]\ndrop_at = 105.0\n").is_err());
        assert!(Config::from_toml("[rapl]\nunleashed = [150, 200]\n").is_err());
        assert!(Config::from_toml("[fan]\ntable_max = 60\n").is_err());
    }
}
