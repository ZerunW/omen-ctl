//! End-to-end test without hardware: run the real omenctld binary against a fake
//! sysfs tree, drive it through the socket, check sd_notify and the safe exit.

use omenctl_core::config::LidAction;
use omenctl_core::curve::FanCurve;
use omenctl_core::proto::{ChargeState, Client, LidStatus, Request, Response};
use omenctl_core::{FanMode, Mode};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CHARGE_BEHAVIOUR: &str = "sys/class/power_supply/BAT0/charge_behaviour";

fn mk(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, v).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap().trim().to_string()
}

fn fake_tree(root: &Path) {
    mk(root, "sys/firmware/acpi/platform_profile", "balanced\n");
    let h = "sys/devices/platform/hp-wmi/hwmon/hwmon7";
    mk(root, &format!("{h}/pwm1_enable"), "2\n");
    mk(root, &format!("{h}/pwm1"), "0\n");
    mk(root, &format!("{h}/pwm2"), "0\n");
    mk(root, &format!("{h}/fan1_input"), "2600\n");
    mk(root, &format!("{h}/fan2_input"), "2400\n");
    mk(root, "sys/devices/platform/hp-wmi/omenctl_kick", "");
    mk(root, "sys/devices/platform/hp-wmi/omenctl_concurrent_tdp", "");
    mk(root, "sys/class/hwmon/hwmon9/name", "coretemp\n");
    mk(root, "sys/class/hwmon/hwmon9/temp1_input", "50000\n");
    mk(root, "sys/class/power_supply/ADP1/online", "1\n");
    mk(root, "sys/class/power_supply/BAT0/type", "Battery\n");
    mk(root, "sys/class/power_supply/BAT0/capacity", "60\n");
    mk(root, CHARGE_BEHAVIOUR, "[auto] inhibit-charge force-discharge\n");
    mk(root, "sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "suspended\n");
    mk(root, "sys/class/powercap/intel-rapl:0/energy_uj", "1000\n");
    mk(root, "sys/class/powercap/intel-rapl:0/max_energy_range_uj", "262143328850\n");
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timeout waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn daemon_end_to_end_on_fake_sysfs() {
    let dir: PathBuf = std::env::temp_dir().join(format!("omenctld-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let root = dir.join("root");
    fake_tree(&root);
    let sock = dir.join("omenctl.sock");
    let state = dir.join("state.json");
    let config = dir.join("config.toml");
    std::fs::write(&config, "# test config\ndefault_mode = \"balanced\"\n").unwrap();
    let notify = dir.join("notify");
    let nrx = UnixDatagram::bind(&notify).unwrap();
    nrx.set_read_timeout(Some(Duration::from_secs(10))).unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_omenctld"))
        .args(["--root", root.to_str().unwrap()])
        .args(["--socket", sock.to_str().unwrap()])
        .args(["--state", state.to_str().unwrap()])
        .args(["--config", config.to_str().unwrap()])
        .args(["--group", "no-such-group-omenctl-test", "--no-gpu"])
        .env("NOTIFY_SOCKET", &notify)
        .env("WATCHDOG_USEC", "3000000")
        .env_remove("WATCHDOG_PID")
        .spawn()
        .unwrap();

    // READY=1, then WATCHDOG=1 (every WATCHDOG_USEC/3 = 1 s)
    let mut buf = [0u8; 128];
    let n = nrx.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"READY=1");
    let n = nrx.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"WATCHDOG=1");

    // socket is root-only when the group is missing
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600);

    let mut c = Client::connect(sock.to_str().unwrap()).unwrap();
    let Response::Ok { status } = c.call(&Request::Status).unwrap() else { panic!() };
    assert_eq!(status.mode, Mode::Balanced);
    assert_eq!(status.fan, FanMode::Curve);
    assert_eq!(status.cpu_temp, Some(50.0));

    // read-only status socket next to it: open to everyone, Status only
    let status_sock = dir.join(omenctl_core::proto::STATUS_SOCKET_NAME);
    assert_eq!(std::fs::metadata(&status_sock).unwrap().permissions().mode() & 0o777, 0o666);
    let mut ro = Client::connect(status_sock.to_str().unwrap()).unwrap();
    assert!(matches!(ro.call(&Request::Status).unwrap(), Response::Ok { .. }));
    let r = ro.call(&Request::SetMode { mode: Mode::PowerSaver }).unwrap();
    assert!(matches!(r, Response::Error { ref message } if message.contains("read-only")), "{r:?}");
    drop(ro);

    // Unleashed: fans manual, profile performance, 75 W written, kernel watchdog kicked
    let r = c.call(&Request::SetMode { mode: Mode::Unleashed }).unwrap();
    assert!(matches!(r, Response::Ok { .. }), "{r:?}");
    let hp = "sys/devices/platform/hp-wmi";
    assert_eq!(read(&root, "sys/firmware/acpi/platform_profile"), "performance");
    assert_eq!(read(&root, &format!("{hp}/hwmon/hwmon7/pwm1_enable")), "1");
    assert_eq!(read(&root, &format!("{hp}/omenctl_concurrent_tdp")), "75");
    std::fs::write(root.join(format!("{hp}/omenctl_kick")), "").unwrap();
    wait_for("kick", || read(&root, &format!("{hp}/omenctl_kick")) == "1");

    // BIOS-auto refused in Unleashed, fixed level accepted, out of range refused
    let r = c.call(&Request::SetFan { fan: FanMode::Auto }).unwrap();
    assert!(matches!(r, Response::Error { .. }), "{r:?}");
    let r = c.call(&Request::SetFan { fan: FanMode::Fixed(60) }).unwrap();
    assert!(matches!(r, Response::Error { .. }), "{r:?}");
    let r = c.call(&Request::SetFan { fan: FanMode::Fixed(30) }).unwrap();
    let Response::Ok { status } = r else { panic!("{r:?}") };
    assert_eq!(status.fan_target, [30, 30]);

    // state saved
    let saved = std::fs::read_to_string(&state).unwrap();
    assert!(saved.contains("unleashed") && saved.contains("fixed"), "{saved}");

    // curves: bad one refused (file unchanged), good one written with comment kept
    let before = std::fs::read_to_string(&config).unwrap();
    let bad = FanCurve { points: vec![] };
    let r = c.call(&Request::SetCurves { cpu: bad.clone(), gpu: bad }).unwrap();
    assert!(matches!(r, Response::Error { .. }), "{r:?}");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    let mut cpu = FanCurve::bios_default_cpu();
    cpu.points[0].level = 20;
    let r = c.call(&Request::SetCurves { cpu: cpu.clone(), gpu: FanCurve::bios_default_gpu() }).unwrap();
    assert!(matches!(r, Response::Ok { .. }), "{r:?}");
    let Response::Curves { cpu: got, .. } = c.call(&Request::GetCurves).unwrap() else { panic!() };
    assert_eq!(got.points[0].level, 20);
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("# test config") && text.contains("[fan.cpu_curve]"), "{text}");

    let r = c.call(&Request::SetCharge { limit: 60, resume: 60 }).unwrap();
    assert!(matches!(r, Response::Error { .. }), "{r:?}");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
    assert_eq!(read(&root, CHARGE_BEHAVIOUR), "[auto] inhibit-charge force-discharge");
    let Response::Ok { status } = c.call(&Request::SetCharge { limit: 60, resume: 55 }).unwrap() else { panic!() };
    assert_eq!(status.charge.unwrap().state, ChargeState::Holding);
    assert_eq!(read(&root, CHARGE_BEHAVIOUR), "inhibit-charge");
    let Response::Ok { status } = c.call(&Request::SetLid { close: LidAction::IgnoreOnAc }).unwrap() else { panic!() };
    assert_eq!(status.lid, Some(LidStatus { close: LidAction::IgnoreOnAc, inhibited: false }));
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("# test config") && text.contains("[battery]") && text.contains("[lid]"), "{text}");
    let Response::Ok { status } = c.call(&Request::ChargeFullOnce { on: true }).unwrap() else { panic!() };
    assert!(status.charge.unwrap().full_once);
    assert_eq!(read(&root, CHARGE_BEHAVIOUR), "auto");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), text);
    let Response::Ok { status } = c.call(&Request::ChargeFullOnce { on: false }).unwrap() else { panic!() };
    assert!(!status.charge.unwrap().full_once);
    assert_eq!(read(&root, CHARGE_BEHAVIOUR), "inhibit-charge");
    // Emulate the kernel's bracketed readback after the write.
    mk(&root, CHARGE_BEHAVIOUR, "auto [inhibit-charge] force-discharge\n");

    // garbage line gets an error reply, connection keeps working
    {
        use std::io::{BufRead, BufReader, Write};
        let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        s.write_all(b"not json\n{\"cmd\":\"status\"}\n").unwrap();
        let mut r = BufReader::new(s);
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        assert!(l.contains("\"error\""), "{l}");
        l.clear();
        r.read_line(&mut l).unwrap();
        assert!(l.contains("\"ok\""), "{l}");
    }

    // external profile change (GNOME power menu) is adopted
    std::fs::write(root.join("sys/firmware/acpi/platform_profile"), "balanced").unwrap();
    wait_for("adopt balanced", || {
        let Response::Ok { status } = c.call(&Request::Status).unwrap() else { return false };
        status.mode == Mode::Balanced
    });

    // SIGTERM: balanced profile, fans back to BIOS, socket removed, exit 0
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let st = child.wait().unwrap();
    assert!(st.success(), "{st:?}");
    assert_eq!(read(&root, "sys/firmware/acpi/platform_profile"), "balanced");
    assert_eq!(read(&root, &format!("{hp}/hwmon/hwmon7/pwm1_enable")), "2");
    assert_eq!(read(&root, CHARGE_BEHAVIOUR), "auto");
    assert!(!sock.exists());
    assert!(!status_sock.exists());

    // restart: saved state is restored
    mk(&root, CHARGE_BEHAVIOUR, "[auto] inhibit-charge force-discharge\n");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_omenctld"))
        .args(["--root", root.to_str().unwrap(), "--socket", sock.to_str().unwrap()])
        .args(["--state", state.to_str().unwrap(), "--config", config.to_str().unwrap()])
        .args(["--group", "no-such-group-omenctl-test", "--no-gpu"])
        .env_remove("NOTIFY_SOCKET")
        .spawn()
        .unwrap();
    wait_for("socket", || sock.exists());
    let mut c = Client::connect(sock.to_str().unwrap()).unwrap();
    let Response::Ok { status } = c.call(&Request::Status).unwrap() else { panic!() };
    assert_eq!(status.mode, Mode::Balanced);
    assert_eq!(status.fan, FanMode::Fixed(30));
    assert_eq!(status.charge.unwrap().state, ChargeState::Holding);
    assert_eq!(status.lid, Some(LidStatus { close: LidAction::IgnoreOnAc, inhibited: false }));
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert!(child.wait().unwrap().success());

    // invalid config: refuses to start (exit code 1)
    std::fs::write(&config, "[protect]\ndrop_at = 150.0\n").unwrap();
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_omenctld"))
        .args(["--root", root.to_str().unwrap(), "--socket", sock.to_str().unwrap()])
        .args(["--state", state.to_str().unwrap(), "--config", config.to_str().unwrap(), "--no-gpu"])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(1));

    std::fs::remove_dir_all(&dir).ok();
}
