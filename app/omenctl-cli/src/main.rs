//! omenctl: command-line client for omenctld.

use omenctl_core::curve::{LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};

const USAGE: &str = "usage:
  omenctl status [--json]                  show the current state
  omenctl mode saver|balanced|unleashed    switch performance mode
  omenctl fan auto                         fans controlled by the BIOS (not allowed in Unleashed)
  omenctl fan curve                        fans follow the fan curve (default)
  omenctl fan max                          fans at the top level (5200 rpm)
  omenctl fan <18-52>                      fixed speed in units of 100 rpm (30 = 3000 rpm)

options:
  --socket PATH                            service socket (default /run/omenctl.sock)";

fn opt<T: std::fmt::Display>(v: Option<T>, unit: &str) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{v}{unit}"))
}

fn fan_label(f: FanMode) -> String {
    match f {
        FanMode::Auto => "BIOS automatic".into(),
        FanMode::Curve => "curve".into(),
        FanMode::Fixed(l) => format!("fixed {} rpm", l as u32 * 100),
        FanMode::Max => "maximum".into(),
    }
}

fn print_status(s: &Status) {
    let mode = if s.mode == s.effective_mode {
        s.mode.label().to_string()
    } else {
        format!("{} (running as {})", s.mode.label(), s.effective_mode.label())
    };
    let rpm = |l: u8| format!("{} rpm", l as u32 * 100);
    println!("mode              {mode}");
    println!("fan mode          {}", fan_label(s.fan));
    println!("CPU temperature   {}", opt(s.cpu_temp.map(|t| format!("{t:.0}")), " °C"));
    println!("GPU temperature   {}", s.gpu_temp.map_or("- (GPU asleep or unavailable)".into(), |t| format!("{t:.0} °C")));
    println!("CPU power         {}", opt(s.cpu_power_w.map(|w| format!("{w:.1}")), " W"));
    println!("GPU power         {}", opt(s.gpu_power_w.map(|w| format!("{w:.1}")), " W"));
    println!("fan speed         CPU {} / GPU {}", opt(s.fan_rpm[0], " rpm"), opt(s.fan_rpm[1], " rpm"));
    println!("fan target        CPU {} / GPU {}", rpm(s.fan_target[0]), rpm(s.fan_target[1]));
    println!("fan recommended   CPU {} / GPU {}", rpm(s.fan_recommended[0]), rpm(s.fan_recommended[1]));
    println!("power source      {}", s.ac_online.map_or("-", |a| if a { "AC" } else { "battery" }));
    if s.alerts.is_empty() {
        println!("alerts            none");
    } else {
        for a in &s.alerts {
            println!("alert             {a}");
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
            let mode = Mode::parse(m).ok_or_else(|| format!("unknown mode {m} (use saver, balanced or unleashed)"))?;
            Ok((Request::SetMode { mode }, false))
        }
        ["fan", f] => {
            let fan = match *f {
                "auto" => FanMode::Auto,
                "curve" => FanMode::Curve,
                "max" => FanMode::Max,
                n => {
                    let l: u8 = n.parse().map_err(|_| format!("unknown fan setting {n}"))?;
                    if !(LEVEL_MIN..=LEVEL_MAX).contains(&l) {
                        return Err(format!("fan level must be {LEVEL_MIN}-{LEVEL_MAX} (units of 100 rpm)"));
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
            "--socket" => socket = it.next().ok_or("--socket needs a path")?,
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
        Response::Curves { .. } => Err("unexpected reply from the service".into()),
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
