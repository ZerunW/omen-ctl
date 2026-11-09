//! omenctl core: fan curve, configuration, hardware access (sysfs only) and the
//! control loop for HP OMEN 16-am0xxx (board 8D3F). No direct WMI/EC access here:
//! every firmware command is issued by the patched hp-wmi kernel driver.

pub mod config;
pub mod controller;
pub mod curve;
pub mod hw;
pub mod power;
pub mod proto;

use serde::{Deserialize, Serialize};

/// User-facing performance modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    PowerSaver,
    Balanced,
    Unleashed,
}

impl Mode {
    /// Value written to /sys/firmware/acpi/platform_profile.
    pub fn platform_profile(self) -> &'static str {
        match self {
            Mode::PowerSaver => "low-power",
            Mode::Balanced => "balanced",
            Mode::Unleashed => "performance",
        }
    }

    pub fn from_platform_profile(s: &str) -> Option<Mode> {
        match s.trim() {
            "low-power" | "quiet" | "cool" => Some(Mode::PowerSaver),
            "balanced" => Some(Mode::Balanced),
            "performance" | "balanced-performance" => Some(Mode::Unleashed),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "saver" | "power-saver" | "power_saver" | "eco" => Some(Mode::PowerSaver),
            "balanced" => Some(Mode::Balanced),
            "unleashed" | "performance" | "max" => Some(Mode::Unleashed),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::PowerSaver => "Power Saver",
            Mode::Balanced => "Balanced",
            Mode::Unleashed => "Unleashed",
        }
    }
}

/// Fan control modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "level")]
pub enum FanMode {
    /// BIOS automatic curve. Not allowed in Unleashed (EC would drop the boost).
    Auto,
    /// Software curve from the config (default in every mode).
    Curve,
    /// Fixed level in 100 rpm units (18..=52).
    Fixed(u8),
    /// Top table level (52), never the 0x27 max-fan switch.
    Max,
}
