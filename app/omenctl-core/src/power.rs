//! Power supply telemetry from /sys/class/power_supply (read only, display only).
//! Nothing here drives a control decision except `ac_online`, which the
//! controller already reads through `Hw::sensors`.

use crate::hw::read_trim;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PowerInfo {
    /// Barrel or USB-C adapter present (ADP1/online).
    pub ac_online: Option<bool>,
    /// Rated adapter power reported by the BIOS (W), if the driver exposes it.
    #[serde(default)]
    pub adapter_w: Option<u32>,
    pub battery: Option<Battery>,
    /// USB-C power inputs (UCSI), one entry per port with a partner.
    #[serde(default)]
    pub usb_c: Vec<UsbC>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    pub percent: Option<u8>,
    /// "Charging", "Discharging", "Full", "Not charging" (kernel wording).
    pub status: String,
    /// Battery power in watts: positive while charging, negative while discharging.
    pub power_w: Option<f32>,
    pub energy_wh: Option<f32>,
    pub full_wh: Option<f32>,
    pub design_wh: Option<f32>,
    pub cycles: Option<u32>,
}

impl Battery {
    /// Current full capacity as a share of the design capacity (0..=1).
    pub fn health(&self) -> Option<f32> {
        match (self.full_wh, self.design_wh) {
            (Some(f), Some(d)) if d > 0.0 => Some(f / d),
            _ => None,
        }
    }

    /// Hours left: until empty while discharging, until full while charging.
    pub fn hours_left(&self) -> Option<f32> {
        let p = self.power_w?;
        let (e, f) = (self.energy_wh?, self.full_wh?);
        if p < -0.5 {
            Some(e / -p)
        } else if p > 0.5 && f > e {
            Some((f - e) / p)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsbC {
    pub name: String,
    pub online: bool,
    /// Negotiated mode, e.g. "PD" or "PD_PPS" (the bracketed entry of usb_type).
    pub kind: Option<String>,
    pub current_ma: Option<u32>,
    pub voltage_mv: Option<u32>,
}

fn num(dir: &Path, name: &str) -> Option<u64> {
    read_trim(&dir.join(name)).ok()?.parse().ok()
}

fn bracketed(s: &str) -> Option<String> {
    let a = s.find('[')?;
    let b = s[a..].find(']')? + a;
    Some(s[a + 1..b].to_string())
}

/// Read everything under `<root>/sys/class/power_supply`. Peripheral batteries
/// (mouse, keyboard: scope = Device) are skipped.
pub fn read(root: &Path) -> PowerInfo {
    let base = root.join("sys/class/power_supply");
    let mut out = PowerInfo::default();
    let Ok(rd) = std::fs::read_dir(&base) else { return out };
    let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let typ = read_trim(&d.join("type")).unwrap_or_default();
        let scope = read_trim(&d.join("scope")).unwrap_or_default();
        let online = num(&d, "online").map(|v| v == 1);
        match typ.as_str() {
            "Mains" => out.ac_online = online.or(out.ac_online),
            "Battery" if scope != "Device" && out.battery.is_none() && !name.starts_with("hid") => {
                let uw = |f: &str| num(&d, f).map(|v| v as f32 / 1e6);
                let status = read_trim(&d.join("status")).unwrap_or_default();
                // power_now (uW) or current_now (uA) x voltage_now (uV)
                let watts = uw("power_now").or_else(|| Some(uw("current_now")? * uw("voltage_now")?));
                let power_w = watts.map(|w| match status.as_str() {
                    "Discharging" => -w,
                    "Charging" => w,
                    _ => 0.0,
                });
                // energy_* (uWh) or charge_* (uAh) x voltage_min_design
                let wh = |e: &str, c: &str| {
                    uw(e).or_else(|| Some(uw(c)? * uw("voltage_min_design").or_else(|| uw("voltage_now"))?))
                };
                out.battery = Some(Battery {
                    percent: num(&d, "capacity").map(|v| v.min(100) as u8),
                    status,
                    power_w,
                    energy_wh: wh("energy_now", "charge_now"),
                    full_wh: wh("energy_full", "charge_full"),
                    design_wh: wh("energy_full_design", "charge_full_design"),
                    cycles: num(&d, "cycle_count").map(|v| v as u32).filter(|v| *v > 0),
                });
            }
            "USB" if name.starts_with("ucsi-") => {
                let kind = read_trim(&d.join("usb_type")).ok().and_then(|s| bracketed(&s));
                out.usb_c.push(UsbC {
                    name,
                    online: online.unwrap_or(false),
                    kind,
                    current_ma: num(&d, "current_now").map(|v| (v / 1000) as u32).filter(|v| *v > 0),
                    voltage_mv: num(&d, "voltage_now").map(|v| (v / 1000) as u32).filter(|v| *v > 0),
                });
            }
            _ => {}
        }
    }
    out
}

/// Charge (%) of the laptop battery, without the rest of `read`.
pub fn battery_percent(root: &Path) -> Option<u8> {
    read(root).battery.and_then(|b| b.percent)
}

/// Rated adapter power from the patched hp-wmi driver (a BIOS query, so call it
/// rarely: at start, on plug/unplug and once a minute). None without adapter or
/// with an older driver.
pub fn read_adapter_w(root: &Path) -> Option<u32> {
    num(&root.join("sys/devices/platform/hp-wmi"), "omenctl_adapter_w").map(|v| v as u32).filter(|v| *v > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_fake_tree() {
        let d = std::env::temp_dir().join(format!("omenctl-power-{}", std::process::id()));
        let mk = |rel: &str, v: &str| {
            let p = d.join("sys/class/power_supply").join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, v).unwrap();
        };
        mk("ADP1/type", "Mains\n");
        mk("ADP1/online", "0\n");
        mk("BAT0/type", "Battery\n");
        mk("BAT0/status", "Discharging\n");
        mk("BAT0/capacity", "54\n");
        mk("BAT0/power_now", "32500000\n");
        mk("BAT0/energy_now", "37800000\n");
        mk("BAT0/energy_full", "70082000\n");
        mk("BAT0/energy_full_design", "83028000\n");
        mk("BAT0/cycle_count", "44\n");
        mk("hidpp_battery_0/type", "Battery\n");
        mk("hidpp_battery_0/scope", "Device\n");
        mk("hidpp_battery_0/capacity", "70\n");
        mk("ucsi-source-psy-USBC000:001/type", "USB\n");
        mk("ucsi-source-psy-USBC000:001/online", "1\n");
        mk("ucsi-source-psy-USBC000:001/usb_type", "C [PD] PD_PPS\n");
        mk("ucsi-source-psy-USBC000:001/current_now", "4250000\n");
        mk("ucsi-source-psy-USBC000:001/voltage_now", "0\n");
        let p = read(&d);
        assert_eq!(p.ac_online, Some(false));
        let b = p.battery.clone().unwrap();
        assert_eq!(b.percent, Some(54));
        assert_eq!(b.power_w, Some(-32.5));
        assert_eq!(b.cycles, Some(44));
        assert!((b.health().unwrap() - 0.844).abs() < 0.001);
        assert!((b.hours_left().unwrap() - 1.163).abs() < 0.01);
        assert_eq!(p.usb_c.len(), 1);
        assert_eq!(p.usb_c[0].kind.as_deref(), Some("PD"));
        assert_eq!(p.usb_c[0].current_ma, Some(4250));
        assert_eq!(p.usb_c[0].voltage_mv, None);
        assert_eq!(read_adapter_w(&d), None);
        fs::create_dir_all(d.join("sys/devices/platform/hp-wmi")).unwrap();
        fs::write(d.join("sys/devices/platform/hp-wmi/omenctl_adapter_w"), "230\n").unwrap();
        assert_eq!(read_adapter_w(&d), Some(230));
        fs::remove_dir_all(d).ok();
    }
}
