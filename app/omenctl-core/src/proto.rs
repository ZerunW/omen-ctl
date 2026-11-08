//! Line-delimited JSON protocol between the CLI/GUI and the daemon over
//! /run/omenctl.sock. One request per line, one response per line.

use crate::curve::FanCurve;
use crate::{FanMode, Mode};
use serde::{Deserialize, Serialize};

pub const SOCKET_PATH: &str = "/run/omenctl.sock";

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
    Curves { cpu: FanCurve, gpu: FanCurve },
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
                format!("连不上后台服务（{path}）：{e}。服务是否在运行？ systemctl status omenctld")
            }
            std::io::ErrorKind::PermissionDenied => {
                format!("没有权限访问 {path}：{e}。需要把当前用户加入 omenctl 组并重新登录")
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
        self.reader.get_mut().write_all(line.as_bytes()).map_err(|e| format!("发送失败：{e}"))?;
        let mut resp = String::new();
        let n = self.reader.read_line(&mut resp).map_err(|e| format!("接收失败：{e}"))?;
        if n == 0 {
            return Err("后台服务关闭了连接".into());
        }
        serde_json::from_str(&resp).map_err(|e| format!("无法解析服务端回复：{e}"))
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
    }
}
