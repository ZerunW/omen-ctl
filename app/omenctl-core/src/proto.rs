//! Line-delimited JSON protocol between the CLI/GUI and the daemon over
//! /run/omenctl.sock. One request per line, one response per line.

use crate::config::{LidAction, ModesConfig};
use crate::curve::FanCurve;
use crate::power::PowerInfo;
use crate::{FanMode, Mode};
use serde::{Deserialize, Serialize};

pub const SOCKET_PATH: &str = "/run/omenctl.sock";
/// Read-only socket next to the control socket: accepts only `Status`, open to every
/// local user (mode 0666). Used by the web monitor, which must not be able to change
/// anything.
pub const STATUS_SOCKET_NAME: &str = "omenctl-status.sock";
pub const STATUS_SOCKET_PATH: &str = "/run/omenctl-status.sock";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    SetMode { mode: Mode },
    SetFan { fan: FanMode },
    /// Current CPU and GPU fan curves.
    GetCurves,
    /// Replace both fan curves. The daemon validates them, writes them to
    /// /etc/omenctl/config.toml and applies them immediately.
    SetCurves { cpu: FanCurve, gpu: FanCurve },
    /// Startup and power-source mode rules.
    GetSettings,
    /// Replace the mode rules; written to /etc/omenctl/config.toml.
    SetSettings { modes: ModesConfig },
    /// Charge limit: stop at `limit` %, charge again at `resume` %. `limit` 0 = off.
    /// Validated like `[battery]` and written to /etc/omenctl/config.toml.
    SetCharge { limit: u8, resume: u8 },
    /// Charge to 100 % once, ignoring the limit, until the adapter is unplugged
    /// or the battery is full. Not saved. `on: false` cancels it.
    ChargeFullOnce { on: bool },
    /// What closing the lid does; written to /etc/omenctl/config.toml.
    SetLid { close: LidAction },
}

/// Longest request line the daemon accepts (bytes).
pub const MAX_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// Mode the user asked for.
    pub mode: Mode,
    /// Mode actually applied (differs while thermal protection is active).
    pub effective_mode: Mode,
    pub fan: FanMode,
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub fan_rpm: [Option<u32>; 2],
    /// Fan levels (100 rpm) the daemon is currently requesting.
    pub fan_target: [u8; 2],
    /// Levels the fan curves recommend for the current temperature (100 rpm).
    /// A fixed speed below this is allowed but reported in `alerts`.
    #[serde(default)]
    pub fan_recommended: [u8; 2],
    pub cpu_power_w: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub ac_online: Option<bool>,
    /// Adapter, battery and USB-C power details (display only).
    #[serde(default)]
    pub power: PowerInfo,
    /// Active protection / fault messages (empty when all is well).
    pub alerts: Vec<String>,
    /// GPU priority active: CPU sustained power (W) lowered while the GPU is busy.
    #[serde(default)]
    pub gpu_priority_w: Option<u32>,
    /// Charge limit state. `None` from services older than 0.3.0.
    #[serde(default)]
    pub charge: Option<ChargeStatus>,
    /// Lid close setting and whether the lid switch is being ignored right now.
    #[serde(default)]
    pub lid: Option<LidStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeState {
    /// No limit set: the battery charges to full as usual.
    Off,
    /// On AC at or above the limit: charging is inhibited, the laptop runs from the adapter.
    Holding,
    /// On AC below the limit (or full-once): charging normally.
    Charging,
    /// On battery.
    Discharging,
    /// The kernel offers no charge control (driver without support, or no battery).
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChargeStatus {
    /// 0 = off.
    pub limit: u8,
    pub resume: u8,
    pub state: ChargeState,
    /// A one-time full charge is in progress.
    pub full_once: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LidStatus {
    pub close: LidAction,
    /// The service holds the logind lid-switch inhibitor (closing the lid does nothing).
    pub inhibited: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Ok { status: Status },
    Curves { cpu: FanCurve, gpu: FanCurve },
    Settings { modes: ModesConfig },
    Error { message: String },
}

/// Blocking client used by the CLI and the GUI. Keeps one connection open.
pub struct Client {
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
}

impl Client {
    pub fn connect(path: &str) -> Result<Client, String> {
        let s = std::os::unix::net::UnixStream::connect(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                format!("cannot reach the omenctl service at {path}: {e}. Is it running? (systemctl status omenctld)")
            }
            std::io::ErrorKind::PermissionDenied => {
                format!("no permission to use {path}: {e}. Add your user to the omenctl group and log in again.")
            }
            _ => format!("{path}: {e}"),
        })?;
        let t = Some(std::time::Duration::from_secs(10));
        s.set_read_timeout(t).ok();
        s.set_write_timeout(t).ok();
        Ok(Client { reader: std::io::BufReader::new(s) })
    }

    pub fn call(&mut self, req: &Request) -> Result<Response, String> {
        use std::io::{BufRead, Write};
        let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        line.push('\n');
        self.reader.get_mut().write_all(line.as_bytes()).map_err(|e| format!("send failed: {e}"))?;
        let mut resp = String::new();
        let n = self.reader.read_line(&mut resp).map_err(|e| format!("receive failed: {e}"))?;
        if n == 0 {
            return Err("the service closed the connection".into());
        }
        serde_json::from_str(&resp).map_err(|e| format!("cannot parse the service reply: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_mode","mode":"unleashed"}"#).unwrap();
        assert_eq!(r, Request::SetMode { mode: Mode::Unleashed });
        let f: Request = serde_json::from_str(r#"{"cmd":"set_fan","fan":{"kind":"fixed","level":30}}"#).unwrap();
        assert_eq!(f, Request::SetFan { fan: FanMode::Fixed(30) });
        let a: Request = serde_json::from_str(r#"{"cmd":"set_fan","fan":{"kind":"auto"}}"#).unwrap();
        assert_eq!(a, Request::SetFan { fan: FanMode::Auto });
        let c: Request = serde_json::from_str(
            r#"{"cmd":"set_curves","cpu":{"points":[{"temp":40.0,"level":18},{"temp":85.0,"level":52}]},"gpu":{"points":[{"temp":40,"level":18},{"temp":85,"level":52}]}}"#,
        )
        .unwrap();
        assert!(matches!(c, Request::SetCurves { .. }));
        assert_eq!(serde_json::to_string(&Request::GetCurves).unwrap(), r#"{"cmd":"get_curves"}"#);
        let m: Request = serde_json::from_str(r#"{"cmd":"set_settings","modes":{"startup":"keep","on_ac":"unleashed","on_battery":"balanced"}}"#).unwrap();
        assert!(matches!(m, Request::SetSettings { modes } if modes.on_ac == crate::config::ModeRule::Unleashed));
    }
}
