//! Daemon configuration (/etc/omenctl/config.toml). Every field has a safe default,
//! so a missing or partial file still yields a complete config.

use crate::curve::{FanCurve, LEVEL_MAX};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Mode applied at daemon start if no saved state exists and no rule in
    /// `[modes]` picks one.
    pub default_mode: crate::Mode,
    /// Startup and power-source mode rules (editable from the app).
    pub modes: ModesConfig,
    /// Write the 75 W CPU+GPU concurrent target in Unleashed mode.
    pub unleashed_concurrent_75w: bool,
    /// Optional intel_rapl PL1/PL2 (W) per mode; None = leave untouched.
    pub rapl: RaplConfig,
    pub fan: FanConfig,
    pub protect: ProtectConfig,
    pub cpu: CpuConfig,
}

/// EPP values accepted by intel_pstate, plus "keep" (don't touch) and "auto"
/// (performance while a CPU is busy, balance_performance when idle).
pub const EPP_VALUES: [&str; 7] = ["auto", "keep", "default", "performance", "balance_performance", "balance_power", "power"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuConfig {
    /// CPU energy/performance preference while in Unleashed. "auto" = performance
    /// while any CPU is busy (full boost clocks), balance_performance when idle
    /// (balance_performance alone holds the cores near 3.8 GHz on 8D3F).
    pub unleashed_epp: String,
    /// auto: a CPU counts as busy at or above this share of one second (10..=100 %).
    pub auto_busy_percent: u8,
    /// auto: return to balance_performance after this many idle seconds (1..=120).
    pub auto_idle_s: u32,
}

impl Default for CpuConfig {
    fn default() -> Self {
        CpuConfig { unleashed_epp: "auto".into(), auto_busy_percent: 50, auto_idle_s: 10 }
    }
}

/// A mode, or "keep" (startup: the last used mode; power source: no change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeRule {
    Keep,
    PowerSaver,
    Balanced,
    Unleashed,
}

impl ModeRule {
    pub const ALL: [ModeRule; 4] = [ModeRule::Keep, ModeRule::PowerSaver, ModeRule::Balanced, ModeRule::Unleashed];

    pub fn mode(self) -> Option<crate::Mode> {
        match self {
            ModeRule::Keep => None,
            ModeRule::PowerSaver => Some(crate::Mode::PowerSaver),
            ModeRule::Balanced => Some(crate::Mode::Balanced),
            ModeRule::Unleashed => Some(crate::Mode::Unleashed),
        }
    }

    pub fn parse(s: &str) -> Option<ModeRule> {
        match s.trim().to_ascii_lowercase().as_str() {
            "keep" | "last" | "none" | "off" => Some(ModeRule::Keep),
            m => crate::Mode::parse(m).map(|m| match m {
                crate::Mode::PowerSaver => ModeRule::PowerSaver,
                crate::Mode::Balanced => ModeRule::Balanced,
                crate::Mode::Unleashed => ModeRule::Unleashed,
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ModeRule::Keep => "keep",
            ModeRule::PowerSaver => "power_saver",
            ModeRule::Balanced => "balanced",
            ModeRule::Unleashed => "unleashed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModesConfig {
    /// Mode at service start (boot). Keep = the mode used last time.
    pub startup: ModeRule,
    /// Switch to this mode when the adapter is plugged in. Keep = no change.
    pub on_ac: ModeRule,
    /// Switch to this mode when running on battery. Keep = no change.
    pub on_battery: ModeRule,
    /// Switch to this mode when the battery falls to `low_battery_percent`
    /// while unplugged. Keep = no change.
    pub on_low_battery: ModeRule,
    /// Low battery threshold (%), 5..=50.
    pub low_battery_percent: u8,
}

/// The low battery rule acts again only after the charge rose this far above
/// the threshold (or the adapter was plugged in).
pub const LOW_BATTERY_HYSTERESIS: u8 = 5;

impl Default for ModesConfig {
    fn default() -> Self {
        ModesConfig {
            startup: ModeRule::Keep,
            on_ac: ModeRule::Keep,
            on_battery: ModeRule::Keep,
            on_low_battery: ModeRule::Keep,
            low_battery_percent: 20,
        }
    }
}

impl ModesConfig {
    /// True when unplugged and the battery is at or below the threshold.
    pub fn is_low(&self, ac: Option<bool>, battery: Option<u8>) -> bool {
        ac == Some(false) && battery.is_some_and(|b| b <= self.low_battery_percent)
    }

    /// Rule for a power source (None = unknown source, no rule). On battery, a
    /// set low battery rule wins while the charge is at or below the threshold.
    pub fn for_source(&self, ac: Option<bool>, battery: Option<u8>) -> Option<crate::Mode> {
        match ac? {
            true => self.on_ac.mode(),
            false => {
                let low = if self.is_low(ac, battery) { self.on_low_battery.mode() } else { None };
                low.or(self.on_battery.mode())
            }
        }
    }

    /// Mode to start in: explicit startup rule, else the rule for the current
    /// power source, else the saved mode, else the configured default.
    /// The low battery rule applies on top of any startup choice.
    pub fn startup_mode(&self, ac: Option<bool>, battery: Option<u8>, saved: Option<crate::Mode>, default: crate::Mode) -> crate::Mode {
        if self.is_low(ac, battery) {
            if let Some(m) = self.on_low_battery.mode() {
                return m;
            }
        }
        self.startup.mode().or_else(|| self.for_source(ac, battery)).or(saved).unwrap_or(default)
    }
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
    /// The curves use the average of the hotter of CPU and GPU over this many
    /// seconds, so one-second load spikes don't move the fans (1..=30).
    pub average_s: u32,
    /// After the last increase, keep the fan speed this many seconds (0..=120).
    pub hold_s: u32,
    /// Then lower it by at most this many levels (100 rpm) per second (0.1..=5).
    pub fall_per_s: f32,
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
            modes: ModesConfig::default(),
            unleashed_concurrent_75w: true,
            rapl: RaplConfig::default(),
            fan: FanConfig::default(),
            protect: ProtectConfig::default(),
            cpu: CpuConfig::default(),
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
            average_s: 5,
            hold_s: 15,
            fall_per_s: 0.5,
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
        Self::from_toml_with_warnings(s).map(|(c, _)| c)
    }

    /// Parse and validate, also returning warnings about keys the service does
    /// not know (typos, or a key under the wrong `[table]`): those are ignored,
    /// so the setting silently keeps its default.
    pub fn from_toml_with_warnings(s: &str) -> Result<(Self, Vec<String>), String> {
        let mut t: toml::Table = s.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut warnings = Vec::new();
        // 0.2.0-0.2.4 shipped this key after `[modes]`, where TOML files it as
        // modes.unleashed_concurrent_75w. Honour it there unless it is also set
        // at the top.
        let legacy = t.get_mut("modes").and_then(|m| m.as_table_mut()).and_then(|m| m.remove("unleashed_concurrent_75w"));
        if let Some(v) = legacy {
            warnings.push("unleashed_concurrent_75w belongs above [modes]; using the value found under [modes]".into());
            t.entry("unleashed_concurrent_75w").or_insert(v);
        }
        unknown_keys(&t, &reference_table(), "", &mut warnings);
        let c: Config = toml::Value::Table(t).try_into().map_err(|e: toml::de::Error| e.to_string())?;
        Ok((c.validated()?, warnings))
    }

    pub fn validated(mut self) -> Result<Self, String> {
        self.fan.cpu_curve = self.fan.cpu_curve.validated().map_err(|e| format!("cpu_curve: {e}"))?;
        self.fan.gpu_curve = self.fan.gpu_curve.validated().map_err(|e| format!("gpu_curve: {e}"))?;
        if !(1..=30).contains(&self.fan.average_s) {
            return Err("fan.average_s must be 1-30".into());
        }
        if self.fan.hold_s > 120 {
            return Err("fan.hold_s must be 0-120".into());
        }
        if !(0.1..=5.0).contains(&self.fan.fall_per_s) {
            return Err("fan.fall_per_s must be 0.1-5".into());
        }
        if self.fan.table_max != LEVEL_MAX {
            return Err(format!("fan.table_max must be {LEVEL_MAX} on 8D3F"));
        }
        let p = &self.protect;
        if !(p.fans_max_at <= p.drop_at && p.restore_below < p.fans_max_at && p.drop_at <= 100.0) {
            return Err("protect: need restore_below < fans_max_at <= drop_at <= 100".into());
        }
        if !EPP_VALUES.contains(&self.cpu.unleashed_epp.as_str()) {
            return Err(format!("cpu.unleashed_epp must be one of {}", EPP_VALUES.join(", ")));
        }
        if !(10..=100).contains(&self.cpu.auto_busy_percent) {
            return Err("cpu.auto_busy_percent must be 10-100".into());
        }
        if !(1..=120).contains(&self.cpu.auto_idle_s) {
            return Err("cpu.auto_idle_s must be 1-120".into());
        }
        if !(5..=50).contains(&self.modes.low_battery_percent) {
            return Err("modes.low_battery_percent must be 5-50".into());
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

/// Every key the config knows, as a TOML table (optional values filled in so
/// they serialize).
fn reference_table() -> toml::Table {
    let mut c = Config::default();
    c.rapl = RaplConfig { power_saver: Some((1, 1)), balanced: Some((1, 1)), unleashed: Some((1, 1)) };
    toml::Table::try_from(c).unwrap_or_default()
}

fn unknown_keys(t: &toml::Table, known: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    for (k, v) in t {
        let path = format!("{prefix}{k}");
        match known.get(k) {
            None => out.push(format!("unknown setting {path} is ignored")),
            Some(toml::Value::Table(kt)) => {
                if let toml::Value::Table(vt) = v {
                    unknown_keys(vt, kt, &format!("{path}."), out);
                }
            }
            Some(_) => {}
        }
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

/// Return `existing` (config.toml text, may be empty) with the `[modes]` table
/// replaced, keeping every other line and comment. Validated before returning.
pub fn with_modes(existing: &str, m: &ModesConfig) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|e| format!("config.toml: {e}"))?;
    let t = doc.entry("modes").or_insert(toml_edit::table());
    let t = t.as_table_mut().ok_or("config.toml: [modes] is not a table")?;
    let values: [(&str, toml_edit::Value); 5] = [
        ("startup", m.startup.as_str().into()),
        ("on_ac", m.on_ac.as_str().into()),
        ("on_battery", m.on_battery.as_str().into()),
        ("on_low_battery", m.on_low_battery.as_str().into()),
        ("low_battery_percent", (m.low_battery_percent as i64).into()),
    ];
    for (k, v) in values {
        match t.get_mut(k).and_then(|i| i.as_value_mut()) {
            // keep any trailing comment on the line
            Some(old) => {
                let decor = old.decor().clone();
                *old = v;
                *old.decor_mut() = decor;
            }
            None => {
                t.insert(k, toml_edit::value(v));
            }
        }
    }
    let out = doc.to_string();
    let c = Config::from_toml(&out)?;
    if &c.modes != m {
        return Err("internal error: modes did not round-trip".into());
    }
    Ok(out)
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
    fn modes_rules_and_round_trip() {
        use crate::Mode::*;
        let m = ModesConfig {
            on_ac: ModeRule::Unleashed,
            on_battery: ModeRule::Balanced,
            on_low_battery: ModeRule::PowerSaver,
            ..Default::default()
        };
        assert_eq!(m.startup_mode(Some(true), Some(10), Some(PowerSaver), Balanced), Unleashed);
        assert_eq!(m.startup_mode(Some(false), Some(80), Some(Unleashed), Balanced), Balanced);
        assert_eq!(m.startup_mode(Some(false), Some(20), Some(Unleashed), Balanced), PowerSaver);
        assert_eq!(m.startup_mode(None, None, Some(PowerSaver), Balanced), PowerSaver);
        assert_eq!(ModesConfig::default().startup_mode(Some(true), None, None, Balanced), Balanced);
        let fixed = ModesConfig { startup: ModeRule::Unleashed, ..m };
        assert_eq!(fixed.startup_mode(Some(true), Some(90), Some(Balanced), Balanced), Unleashed);
        assert_eq!(fixed.startup_mode(Some(false), Some(12), Some(Balanced), Balanced), PowerSaver, "low battery wins");
        assert_eq!(m.for_source(Some(false), Some(21)), Some(Balanced));
        assert_eq!(m.for_source(Some(false), Some(19)), Some(PowerSaver));
        assert!(Config::from_toml("[modes]\nlow_battery_percent = 70\n").is_err());

        let orig = "# top\ndefault_mode = \"balanced\"\n\n[modes]\nstartup = \"keep\" # boot\n\n[protect]\nfans_max_at = 90.0\n";
        let out = with_modes(orig, &m).unwrap();
        assert!(out.contains("# top") && out.contains("# boot"), "{out}");
        let c = Config::from_toml(&out).unwrap();
        assert_eq!(c.modes, m);
        assert_eq!(c.protect.fans_max_at, 90.0);
        assert_eq!(Config::from_toml(&with_modes("", &m).unwrap()).unwrap().modes, m);
        assert!(Config::from_toml("[modes]\non_ac = \"turbo\"\n").is_err());
        assert_eq!(ModeRule::parse("last"), Some(ModeRule::Keep));
        assert_eq!(ModeRule::parse("max"), Some(ModeRule::Unleashed));
    }

    #[test]
    fn packaged_example_config_equals_defaults() {
        let c = Config::from_toml(include_str!("../../../packaging/config.toml")).unwrap();
        assert_eq!(c, Config::default().validated().unwrap());
    }

    #[test]
    fn with_curves_rejects_bad_curve() {
        let bad = FanCurve { points: vec![crate::curve::CurvePoint { temp: 50.0, level: 30 }] };
        let ok = FanCurve::bios_default_cpu();
        assert!(with_curves("", &bad, &ok).is_err());
    }

    #[test]
    fn example_config_has_no_unknown_keys() {
        let (_, w) = Config::from_toml_with_warnings(include_str!("../../../packaging/config.toml")).unwrap();
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn concurrent_75w_can_be_turned_off() {
        // the packaged example with the switch flipped must really read false
        let example = include_str!("../../../packaging/config.toml");
        assert!(example.contains("unleashed_concurrent_75w = true"));
        let off = example.replace("unleashed_concurrent_75w = true", "unleashed_concurrent_75w = false");
        let (c, w) = Config::from_toml_with_warnings(&off).unwrap();
        assert!(!c.unleashed_concurrent_75w);
        assert!(w.is_empty(), "{w:?}");
        // the key must sit above [modes] in the example
        let at = example.find("unleashed_concurrent_75w").unwrap();
        assert!(at < example.find("\n[").unwrap(), "unleashed_concurrent_75w must come before the first [table]");
        // configs written by 0.2.0-0.2.4 have it under [modes]: still honoured
        let (c, w) = Config::from_toml_with_warnings("[modes]\nunleashed_concurrent_75w = false\n").unwrap();
        assert!(!c.unleashed_concurrent_75w);
        assert_eq!(w.len(), 1, "{w:?}");
        // the top-level value wins over a stray one
        let c = Config::from_toml("unleashed_concurrent_75w = true\n[modes]\nunleashed_concurrent_75w = false\n").unwrap();
        assert!(c.unleashed_concurrent_75w);
    }

    #[test]
    fn unknown_keys_are_reported() {
        let (_, w) = Config::from_toml_with_warnings("defualt_mode = \"x\"\n[fan]\nhold = 3\n[rapl]\nbalanced = [35, 45]\n").unwrap();
        assert_eq!(w, vec!["unknown setting defualt_mode is ignored", "unknown setting fan.hold is ignored"]);
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
