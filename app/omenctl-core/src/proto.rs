//! Line-delimited JSON protocol between the CLI/GUI and the daemon over
//! /run/omenctl.sock. One request per line, one response per line.

use crate::{FanMode, Mode};
use serde::{Deserialize, Serialize};

pub const SOCKET_PATH: &str = "/run/omenctl.sock";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    SetMode { mode: Mode },
    SetFan { fan: FanMode },
}

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
    pub cpu_power_w: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub ac_online: Option<bool>,
    /// Active protection / fault messages (empty when all is well).
    pub alerts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Ok { status: Status },
    Error { message: String },
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
    }
}
