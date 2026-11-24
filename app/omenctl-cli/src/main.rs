//! omenctl: command-line client for omenctld.

use omenctl_core::config::{BatteryConfig, LidAction, ModeRule, ModesConfig};
use omenctl_core::curve::{LEVEL_MAX, LEVEL_MIN};
use omenctl_core::proto::{ChargeState, Client, Request, Response, Status, SOCKET_PATH};
use omenctl_core::{FanMode, Mode};

const USAGE: &str = "usage:
  omenctl status [--json]                  show the current state
  omenctl mode saver|balanced|unleashed    switch performance mode
  omenctl fan auto                         fans controlled by the BIOS (not allowed in Unleashed)
  omenctl fan curve                        fans follow the fan curve (default)
  omenctl fan max                          fans at the top level (5200 rpm)
  omenctl fan <18-52>                      fixed speed in units of 100 rpm (30 = 3000 rpm)
  omenctl defaults                         show the startup / power-source mode rules
  omenctl defaults startup|ac|battery|low <keep|saver|balanced|unleashed>
                                           set one rule (startup keep = last used mode,
                                           ac/battery/low keep = do not switch)
  omenctl defaults low-at <5-50>           low battery threshold in percent
  omenctl charge                           show the charge limit state
  omenctl charge <limit> [resume]          set percentages (resume defaults to limit minus 5)
  omenctl charge off                       turn off the charge limit
  omenctl charge full [off]                start or cancel a one-time full charge
  omenctl lid                              show the lid close setting
  omenctl lid suspend|ignore-on-ac|ignore  set what closing the lid does

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
    if let Some(w) = s.gpu_priority_w {
        println!("GPU priority      on: CPU sustained power limited to {w} W while the GPU is busy");
    }
    println!("fan speed         CPU {} / GPU {}", opt(s.fan_rpm[0], " rpm"), opt(s.fan_rpm[1], " rpm"));
    println!("fan target        CPU {} / GPU {}", rpm(s.fan_target[0]), rpm(s.fan_target[1]));
    println!("fan recommended   CPU {} / GPU {}", rpm(s.fan_recommended[0]), rpm(s.fan_recommended[1]));
    let p = &s.power;
    let src = match (s.ac_online, p.adapter_w) {
        (Some(true), Some(w)) => format!("AC adapter ({w} W)"),
        (Some(true), None) => "AC adapter".into(),
        (Some(false), _) => "battery".into(),
        (None, _) => "-".into(),
    };
    println!("power source      {src}");
    if let Some(b) = &p.battery {
        let mut line = format!("{}% {}", opt(b.percent, ""), b.status.to_lowercase());
        if let Some(w) = b.power_w.filter(|w| w.abs() >= 0.1) {
            line += &format!(", {:+.1} W", w);
        }
        if let Some(h) = b.hours_left() {
            line += &format!(", {}h{:02} {}", h as u32, ((h.fract()) * 60.0) as u32, if b.power_w.unwrap_or(0.0) < 0.0 { "left" } else { "to full" });
        }
        println!("battery           {line}");
        if let (Some(f), Some(d), Some(h)) = (b.full_wh, b.design_wh, b.health()) {
            println!("battery health    {:.0}% ({f:.1} of {d:.1} Wh){}", h * 100.0, b.cycles.map_or(String::new(), |c| format!(", {c} cycles")));
        }
    }
    for u in p.usb_c.iter().filter(|u| u.online) {
        let mut line = "power input".to_string();
        if let Some(k) = &u.kind {
            line += &format!(", USB {}", k.replace('_', " "));
        }
        if let Some(ma) = u.current_ma {
            line += &format!(", {:.2} A", ma as f32 / 1000.0);
        }
        if let Some(mv) = u.voltage_mv {
            line += &format!(", {:.1} V", mv as f32 / 1000.0);
        }
        println!("USB-C             {line}");
    }
    print_charge(s);
    print_lid(s);
    if s.alerts.is_empty() {
        println!("alerts            none");
    } else {
        for a in &s.alerts {
            println!("alert             {a}");
        }
    }
}

fn print_charge(s: &Status) {
    let line = s.charge.map_or_else(
        || "not supported".into(),
        |c| {
            let state = match c.state {
                ChargeState::Holding => "holding",
                ChargeState::Charging => "charging",
                ChargeState::Discharging => "discharging",
                ChargeState::Off => return "off".into(),
                ChargeState::Unavailable => return "not supported".into(),
            };
            let once = if c.full_once { " (full charge once)" } else { "" };
            format!("{}% (charge again at {}%): {state}{once}", c.limit, c.resume)
        },
    );
    println!("charge limit      {line}");
}

fn print_lid(s: &Status) {
    let (line, ignored) = match s.lid {
        None => ("not supported", false),
        Some(lid) => {
            let setting = match lid.close {
                LidAction::Suspend => "suspend (desktop default)",
                LidAction::IgnoreOnAc => "keep running on AC",
                LidAction::Ignore => "keep running",
            };
            (setting, lid.inhibited)
        }
    };
    let now = if ignored { " (lid switch ignored now)" } else { "" };
    println!("lid close         {line}{now}");
}

fn rule_label(r: ModeRule, startup: bool) -> &'static str {
    match r {
        ModeRule::Keep if startup => "last used (or the power-source rule)",
        ModeRule::Keep => "no change",
        ModeRule::PowerSaver => "Power Saver",
        ModeRule::Balanced => "Balanced",
        ModeRule::Unleashed => "Unleashed",
    }
}

fn print_modes(m: &ModesConfig) {
    println!("at startup        {}", rule_label(m.startup, true));
    println!("plugged in        {}", rule_label(m.on_ac, false));
    println!("on battery        {}", rule_label(m.on_battery, false));
    println!("low battery       {} (at {}% or less, on battery)", rule_label(m.on_low_battery, false), m.low_battery_percent);
}

enum Cmd {
    Call(Request, bool),
    Charge(Request),
    Lid(Request),
    /// Change one mode rule: read the current rules, then write them back.
    SetRule(String, ModeRule),
    /// Change the low battery threshold.
    SetLowAt(u8),
}

/// Parse command words into a request (test helper; `parse_cmd` is the real parser).
#[cfg(test)]
fn parse(words: &[String]) -> Result<(Request, bool), String> {
    match parse_cmd(words)? {
        Cmd::Call(r, j) => Ok((r, j)),
        Cmd::Charge(r) | Cmd::Lid(r) => Ok((r, false)),
        Cmd::SetRule(..) | Cmd::SetLowAt(_) => Ok((Request::GetSettings, false)),
    }
}

fn parse_cmd(words: &[String]) -> Result<Cmd, String> {
    let w: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
    if let ["defaults", "low-at", v] = w.as_slice() {
        let n: u8 = v.parse().map_err(|_| format!("not a number: {v}"))?;
        if !(5..=50).contains(&n) {
            return Err("low battery threshold must be 5-50 (%)".into());
        }
        return Ok(Cmd::SetLowAt(n));
    }
    if let ["defaults", which, v] = w.as_slice() {
        let which = match *which {
            "startup" | "boot" => "startup",
            "ac" | "plugged" => "ac",
            "battery" | "bat" => "battery",
            "low" | "low-battery" => "low",
            o => return Err(format!("unknown rule {o} (use startup, ac, battery, low or low-at)")),
        };
        let r = ModeRule::parse(v).ok_or_else(|| format!("unknown mode {v} (use keep, saver, balanced or unleashed)"))?;
        return Ok(Cmd::SetRule(which.into(), r));
    }
    if let ["defaults"] = w.as_slice() {
        return Ok(Cmd::Call(Request::GetSettings, false));
    }
    if let ["charge", args @ ..] = w.as_slice() {
        return Ok(Cmd::Charge(parse_charge(args)?));
    }
    if let ["lid"] = w.as_slice() {
        return Ok(Cmd::Lid(Request::Status));
    }
    if let ["lid", action] = w.as_slice() {
        let close = LidAction::parse(&action.replace('-', "_"))
            .ok_or_else(|| format!("unknown lid action {action} (use suspend, ignore-on-ac or ignore)"))?;
        return Ok(Cmd::Lid(Request::SetLid { close }));
    }
    let (r, j) = parse_basic(&w)?;
    Ok(Cmd::Call(r, j))
}

fn parse_charge(args: &[&str]) -> Result<Request, String> {
    let percent = |s: &str| s.parse::<u8>().map_err(|_| format!("invalid charge percentage: {s}"));
    match args {
        [] => Ok(Request::Status),
        ["full"] => Ok(Request::ChargeFullOnce { on: true }),
        ["full", "off"] => Ok(Request::ChargeFullOnce { on: false }),
        ["off"] => Ok(Request::SetCharge { limit: 0, resume: BatteryConfig::default().charge_resume }),
        [limit] => {
            let limit = percent(limit)?;
            let resume = limit.checked_sub(5).ok_or("charge limit too low to calculate resume percentage")?;
            Ok(Request::SetCharge { limit, resume })
        }
        [limit, resume] => Ok(Request::SetCharge { limit: percent(limit)?, resume: percent(resume)? }),
        _ => Err(USAGE.into()),
    }
}

fn parse_basic(w: &[&str]) -> Result<(Request, bool), String> {
    match w {
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
    let cmd = parse_cmd(&words)?;
    let mut c = Client::connect(&socket)?;
    let (req, json, print): (_, _, fn(&Status)) = match cmd {
        Cmd::Call(r, j) => (r, j, print_status),
        Cmd::Charge(r) => (r, false, print_charge),
        Cmd::Lid(r) => (r, false, print_lid),
        Cmd::SetRule(..) | Cmd::SetLowAt(_) => {
            let Response::Settings { mut modes } = c.call(&Request::GetSettings)? else {
                return Err("unexpected reply from the service".into());
            };
            match cmd {
                Cmd::SetRule(which, rule) => match which.as_str() {
                    "startup" => modes.startup = rule,
                    "ac" => modes.on_ac = rule,
                    "low" => modes.on_low_battery = rule,
                    _ => modes.on_battery = rule,
                },
                Cmd::SetLowAt(n) => modes.low_battery_percent = n,
                _ => unreachable!(),
            }
            (Request::SetSettings { modes }, false, print_status)
        }
    };
    match c.call(&req)? {
        Response::Settings { modes } => {
            print_modes(&modes);
            Ok(())
        }
        Response::Ok { status } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?);
            } else {
                print(&status);
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
        assert_eq!(p("defaults").unwrap().0, Request::GetSettings);
        assert!(matches!(parse_cmd(&["defaults".into(), "ac".into(), "unleashed".into()]), Ok(Cmd::SetRule(w, ModeRule::Unleashed)) if w == "ac"));
        assert!(parse_cmd(&["defaults".into(), "ac".into(), "turbo".into()]).is_err());
        assert!(parse_cmd(&["defaults".into(), "lunch".into(), "keep".into()]).is_err());
        assert!(matches!(parse_cmd(&["defaults".into(), "low".into(), "saver".into()]), Ok(Cmd::SetRule(w, ModeRule::PowerSaver)) if w == "low"));
        assert!(matches!(parse_cmd(&["defaults".into(), "low-at".into(), "15".into()]), Ok(Cmd::SetLowAt(15))));
        assert!(parse_cmd(&["defaults".into(), "low-at".into(), "80".into()]).is_err());
    }

    #[test]
    fn parses_charge_and_lid_commands() {
        assert!(matches!(parse_cmd(&["charge".into()]), Ok(Cmd::Charge(Request::Status))));
        assert_eq!(p("charge 60 54").unwrap().0, Request::SetCharge { limit: 60, resume: 54 });
        assert_eq!(p("charge 60").unwrap().0, Request::SetCharge { limit: 60, resume: 55 });
        assert_eq!(p("charge off").unwrap().0, Request::SetCharge { limit: 0, resume: BatteryConfig::default().charge_resume });
        assert_eq!(p("charge full").unwrap().0, Request::ChargeFullOnce { on: true });
        assert_eq!(p("charge full off").unwrap().0, Request::ChargeFullOnce { on: false });
        assert!(p("charge 3").is_err());
        assert!(p("charge 60 x").is_err());
        assert!(p("charge full on").is_err());
        assert!(matches!(parse_cmd(&["lid".into()]), Ok(Cmd::Lid(Request::Status))));
        assert_eq!(p("lid suspend").unwrap().0, Request::SetLid { close: LidAction::Suspend });
        assert_eq!(p("lid ignore-on-ac").unwrap().0, Request::SetLid { close: LidAction::IgnoreOnAc });
        assert_eq!(p("lid ignore").unwrap().0, Request::SetLid { close: LidAction::Ignore });
        assert!(p("lid sleep").is_err());
    }
}
