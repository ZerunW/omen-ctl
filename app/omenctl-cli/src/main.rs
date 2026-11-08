//! omenctl: command-line client for omenctld.

use omenctl_core::curve::{LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};

const USAGE: &str = "用法：
  omenctl status [--json]                 显示当前状态
  omenctl mode saver|balanced|unleashed   切换模式（省电 / 平衡 / 满血）
  omenctl fan auto                        风扇交给 BIOS（满血模式下不允许）
  omenctl fan curve                       风扇按曲线（默认）
  omenctl fan max                         风扇最高档（5200 rpm）
  omenctl fan <18-52>                     固定档位，单位 100 rpm（例如 30 = 3000 rpm）

选项：
  --socket PATH                           服务端 socket（默认 /run/omenctl.sock）";

fn fmt_opt<T: std::fmt::Display>(v: Option<T>, unit: &str) -> String {
    v.map_or_else(|| "—".to_string(), |v| format!("{v}{unit}"))
}

fn fan_label(f: FanMode) -> String {
    match f {
        FanMode::Auto => "BIOS 自动".into(),
        FanMode::Curve => "曲线".into(),
        FanMode::Fixed(l) => format!("固定 {} rpm", l as u32 * 100),
        FanMode::Max => "最高档".into(),
    }
}

fn print_status(s: &Status) {
    let mode = if s.mode == s.effective_mode {
        s.mode.label_zh().to_string()
    } else {
        format!("{}（当前实际：{}）", s.mode.label_zh(), s.effective_mode.label_zh())
    };
    println!("模式      {mode}");
    println!("风扇模式  {}", fan_label(s.fan));
    println!("CPU 温度  {}", fmt_opt(s.cpu_temp.map(|t| format!("{t:.0}")), " °C"));
    println!("显卡温度  {}", s.gpu_temp.map_or("—（独显休眠或无法读取）".into(), |t| format!("{t:.0} °C")));
    println!("CPU 功率  {}", fmt_opt(s.cpu_power_w.map(|w| format!("{w:.1}")), " W"));
    println!("显卡功率  {}", fmt_opt(s.gpu_power_w.map(|w| format!("{w:.1}")), " W"));
    println!(
        "风扇转速  CPU {} / 显卡 {}",
        fmt_opt(s.fan_rpm[0], " rpm"),
        fmt_opt(s.fan_rpm[1], " rpm")
    );
    println!("风扇目标  CPU {} rpm / 显卡 {} rpm", s.fan_target[0] as u32 * 100, s.fan_target[1] as u32 * 100);
    println!("电源      {}", s.ac_online.map_or("—", |a| if a { "已接通" } else { "电池" }));
    if s.alerts.is_empty() {
        println!("告警      无");
    } else {
        for a in &s.alerts {
            println!("告警      {a}");
        }
    }
}

/// Parse command words (without the program name / options) into a request.
fn parse(words: &[String]) -> Result<(Request, bool), String> {
    let w: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
    match w.as_slice() {
        [] | ["status"] => Ok((Request::Status, false)),
        ["status", "--json"] => Ok((Request::Status, true)),
        ["mode", m] => {
            let mode = Mode::parse(m).ok_or_else(|| format!("未知模式 {m}（可用：saver / balanced / unleashed）"))?;
            Ok((Request::SetMode { mode }, false))
        }
        ["fan", f] => {
            let fan = match *f {
                "auto" => FanMode::Auto,
                "curve" => FanMode::Curve,
                "max" => FanMode::Max,
                n => {
                    let l: u8 = n.parse().map_err(|_| format!("无法识别的风扇参数 {n}"))?;
                    if !(LEVEL_MIN..=LEVEL_MAX).contains(&l) {
                        return Err(format!("档位必须在 {LEVEL_MIN}–{LEVEL_MAX} 之间（单位 100 rpm）"));
                    }
                    FanMode::Fixed(l)
                }
            };
            Ok((Request::SetFan { fan }, false))
        }
        _ => Err(USAGE.into()),
    }
}

fn run() -> Result<(), String> {
    let mut socket = SOCKET_PATH.to_string();
    let mut words = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = it.next().ok_or("--socket 需要一个路径")?,
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => words.push(a),
        }
    }
    let (req, json) = parse(&words)?;
    let mut c = Client::connect(&socket)?;
    match c.call(&req)? {
        Response::Ok { status } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?);
            } else {
                print_status(&status);
            }
            Ok(())
        }
        Response::Error { message } => Err(message),
        Response::Curves { .. } => Err("服务端回复了意外的内容".into()),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("omenctl: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<(Request, bool), String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn parses_commands() {
        assert_eq!(p("").unwrap(), (Request::Status, false));
        assert_eq!(p("status --json").unwrap(), (Request::Status, true));
        assert_eq!(p("mode saver").unwrap().0, Request::SetMode { mode: Mode::PowerSaver });
        assert_eq!(p("mode unleashed").unwrap().0, Request::SetMode { mode: Mode::Unleashed });
        assert_eq!(p("fan auto").unwrap().0, Request::SetFan { fan: FanMode::Auto });
        assert_eq!(p("fan max").unwrap().0, Request::SetFan { fan: FanMode::Max });
        assert_eq!(p("fan 30").unwrap().0, Request::SetFan { fan: FanMode::Fixed(30) });
        assert!(p("fan 17").is_err());
        assert!(p("fan 53").is_err());
        assert!(p("fan x").is_err());
        assert!(p("mode turbo").is_err());
        assert!(p("bogus").is_err());
    }
}
