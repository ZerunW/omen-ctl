use super::Collector;
use crate::snapshot::Nvidia;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct Tree(PathBuf);

impl Tree {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("omenctl-web-test-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("cannot create {}: {err}", path.display()),
            }
        }
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn collector(&self) -> Collector {
        Collector::new(self.0.clone(), None)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cpu_usage_and_core_kinds() {
    let tree = Tree::new();
    tree.write(
        "proc/cpuinfo",
        "processor: 0\nmodel name : First CPU\nmodel name: Second CPU\n",
    );
    tree.write("sys/devices/cpu_core/cpus", "0,2,4-7\n");
    tree.write("sys/devices/cpu_atom/cpus", "8-15\n");
    tree.write("sys/devices/system/cpu/cpu8/cpufreq/scaling_cur_freq", "2599500\n");
    tree.write(
        "proc/stat",
        "cpu 100 0 0 95 5 0 0 0 9 6\ncpu8 30 0 0 65 5\ncpu31 0 0 0 10 0\ncpu4 70 0 0 20 0\n",
    );

    let mut collector = tree.collector();
    let now = Instant::now();
    let first = collector.sample_at(now).cpu;
    assert_eq!(first.model, "First CPU");
    assert_eq!(first.usage, 0.0);
    assert!(first.cores.iter().all(|core| core.usage == 0.0));
    assert_eq!(
        first.cores.iter().map(|core| (core.id, core.kind.as_deref(), core.mhz)).collect::<Vec<_>>(),
        [(4, Some("P"), None), (8, Some("E"), Some(2599)), (31, None, None)]
    );

    tree.write(
        "proc/stat",
        "cpu 180 0 0 195 25 0 0 0 39 12\ncpu8 50 0 0 125 25\ncpu4 130 0 0 60 0\ncpu31 0 0 0 10 0\n",
    );
    let second = collector.sample_at(now + Duration::from_secs(2)).cpu;
    assert_eq!(second.usage, 40.0);
    assert_eq!(second.cores.iter().map(|core| core.usage).collect::<Vec<_>>(), [60.0, 20.0, 0.0]);
}

#[test]
fn system_metadata_and_memory() {
    let tree = Tree::new();
    tree.write("proc/sys/kernel/hostname", "test-host\n");
    tree.write("proc/uptime", "123.45 678.90\n");
    tree.write("proc/loadavg", "0.50 1.25 2.75 1/10 42\n");
    tree.write(
        "proc/meminfo",
        "MemTotal: 16384 kB\nMemAvailable: 4096 kB\nCached: 900 kB\nBuffers: 100 kB\nSwapTotal: 2048 kB\nSwapFree: 1024 kB\n",
    );
    let snapshot = tree.collector().sample();
    assert!(snapshot.time_ms > 0);
    assert_eq!(snapshot.hostname, "test-host");
    assert_eq!(snapshot.uptime_s, 123);
    assert_eq!(snapshot.load, [0.5, 1.25, 2.75]);
    assert_eq!(snapshot.memory.total_bytes, 16384 * 1024);
    assert_eq!(snapshot.memory.used_bytes, 12288 * 1024);
    assert_eq!(snapshot.memory.available_bytes, 4096 * 1024);
    assert_eq!(snapshot.memory.cache_bytes, 1000 * 1024);
    assert_eq!(snapshot.memory.swap_total_bytes, 2048 * 1024);
    assert_eq!(snapshot.memory.swap_used_bytes, 1024 * 1024);
}

#[test]
fn network_rates_and_counter_resets() {
    let tree = Tree::new();
    tree.write("sys/class/net/eth0/operstate", "up\n");
    tree.write("sys/class/net/wlan0/operstate", "down\n");
    let write_counters = |rx, tx| {
        tree.write(
            "proc/net/dev",
            format!(
                " wlan0: 9 0 0 0 0 0 0 0 11 0 0 0 0 0 0 0\n lo: 5 0 0 0 0 0 0 0 5 0 0 0 0 0 0 0\n eth0: {rx} 0 0 0 0 0 0 0 {tx} 0 0 0 0 0 0 0\n"
            ),
        );
    };
    write_counters(100, 150);
    let mut collector = tree.collector();
    let now = Instant::now();
    let first = collector.sample_at(now).net;
    assert_eq!(first.iter().map(|iface| iface.name.as_str()).collect::<Vec<_>>(), ["eth0", "wlan0"]);
    assert!(first[0].up);
    assert!(!first[1].up);
    assert_eq!((first[0].rx_bps, first[0].tx_bps), (0.0, 0.0));

    write_counters(300, 250);
    let second = collector.sample_at(now + Duration::from_secs(2)).net;
    assert_eq!((second[0].rx_bps, second[0].tx_bps), (100.0, 50.0));
    assert_eq!((second[0].rx_total_bytes, second[0].tx_total_bytes), (300, 250));

    write_counters(10, 350);
    let third = collector.sample_at(now + Duration::from_secs(4)).net;
    assert_eq!((third[0].rx_bps, third[0].tx_bps), (0.0, 50.0));
    assert_eq!(third[0].rx_total_bytes, 10);
}

fn write_stat(tree: &Tree, pid: u32, name: &str, utime: u64, stime: u64, start_time: u64) {
    tree.write(
        &format!("proc/{pid}/stat"),
        format!("{pid} ({name}) S 3 0 0 0 0 0 0 0 0 0 {utime} {stime} 0 0 0 -5 4 0 {start_time}\n"),
    );
}

#[test]
fn processes_parse_and_compute_cpu_deltas() {
    let tree = Tree::new();
    tree.write("etc/passwd", "monitor:x:1000:1000::/home/monitor:/bin/sh\n");
    write_stat(&tree, 12, "strange) (name", 1000, 2000, 10);
    tree.write("proc/12/status", "Uid:\t1000\t9999\t9999\t9999\nVmRSS: 14 kB\n");
    tree.write(
        "proc/12/cmdline",
        format!("tool\0--arg\0{}\0", "🦀".repeat(310)),
    );
    write_stat(&tree, 9, "kworker", 5, 7, 20);
    tree.write("proc/9/status", "Uid: 0 0 0 0\n");
    tree.write("proc/9/cmdline", "");
    write_stat(&tree, 24, "daemon", 15, 18, 30);
    tree.write("proc/24/status", "Uid: 999 999 999 999\nVmRSS: 50 kB\n");
    tree.write("proc/24/cmdline", b"daemon\0--debug\0");
    write_stat(&tree, 99, "vanishing", 1, 2, 40);
    tree.write("proc/99/status", "Uid: 0\n");

    let mut collector = tree.collector();
    let now = Instant::now();
    let first = collector.sample_at(now).processes;
    assert_eq!(first.iter().map(|process| process.pid).collect::<Vec<_>>(), [24, 12, 9]);
    assert!(first.iter().all(|process| process.cpu == 0.0));
    assert_eq!(first[0].cmd, "daemon --debug");
    assert_eq!(first[0].user, "999");
    let process = &first[1];
    assert_eq!(process.name, "strange) (name");
    assert_eq!(process.user, "monitor");
    assert_eq!(process.ppid, 3);
    assert_eq!(process.state, "S");
    assert_eq!(process.threads, 4);
    assert_eq!(process.nice, -5);
    assert_eq!(process.mem_bytes, 14 * 1024);
    assert_eq!(process.cmd, format!("tool --arg {}", "🦀".repeat(289)));
    assert_eq!(first[2].cmd, "[kworker]");
    assert_eq!(first[2].mem_bytes, 0);

    // SAFETY: sysconf accepts _SC_CLK_TCK and takes no pointers.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    assert!(ticks > 0);
    let user_delta = 4 * ticks;
    let system_delta = ticks / 2;
    write_stat(&tree, 12, "strange) (name", 1000 + user_delta, 2000 + system_delta, 10);
    fs::remove_dir_all(tree.0.join("proc/9")).unwrap();
    let second = collector.sample_at(now + Duration::from_secs(2)).processes;
    assert_eq!(second.iter().map(|process| process.pid).collect::<Vec<_>>(), [12, 24]);
    assert_eq!(second[0].cpu, (100.0 * (user_delta + system_delta) as f64 / ticks as f64 / 2.0) as f32);

    write_stat(&tree, 12, "new process", 10000, 20000, 100);
    write_stat(&tree, 9, "kworker", 100, 200, 20);
    tree.write("proc/9/status", "Uid: 0 0 0 0\n");
    tree.write("proc/9/cmdline", "");
    let third = collector.sample_at(now + Duration::from_secs(4)).processes;
    assert_eq!(third.len(), 3);
    assert!(third.iter().all(|process| process.cpu == 0.0));
}

#[test]
fn igpu_usage_and_clocks() {
    let tree = Tree::new();
    let residency = "sys/class/drm/card2/gt/gt0/rc6_residency_ms";
    tree.write(residency, "750\n");
    tree.write("sys/class/drm/card2/gt_act_freq_mhz", "300\n");
    tree.write("sys/class/drm/card2/gt_max_freq_mhz", "1450\n");
    tree.write("sys/class/drm/card10/gt/gt0/rc6_residency_ms", "200\n");
    tree.write("sys/class/drm/card0-DP-1/gt/gt0/rc6_residency_ms", "0\n");

    let mut collector = tree.collector();
    let now = Instant::now();
    let first = collector.sample_at(now).igpu.unwrap();
    assert_eq!(first.name, "Intel UHD Graphics");
    assert_eq!(first.mhz, Some(300));
    assert_eq!(first.max_mhz, Some(1450));
    assert_eq!(first.usage, None);

    tree.write(residency, "2250\n");
    let second = collector.sample_at(now + Duration::from_secs(2)).igpu.unwrap();
    assert_eq!(second.usage, Some(25.0));

    tree.write(residency, "5500\n");
    let third = collector.sample_at(now + Duration::from_secs(4)).igpu.unwrap();
    assert_eq!(third.usage, Some(0.0));
}

#[test]
fn nvidia_missing_and_suspended() {
    let tree = Tree::new();
    let mut collector = tree.collector();
    let now = Instant::now();
    assert_eq!(collector.sample_at(now).nvidia, None);

    tree.write("sys/bus/pci/devices/0000:01:00.0/power/runtime_status", "suspended\n");
    assert_eq!(
        collector.sample_at(now + Duration::from_secs(2)).nvidia,
        Some(Nvidia { name: "NVIDIA GPU".into(), ..Nvidia::default() })
    );
}

#[test]
fn missing_daemon_socket_reports_error() {
    let tree = Tree::new();
    let skipped = tree.collector().sample();
    assert!(skipped.omen.is_none());
    assert!(skipped.omen_error.is_none());

    let snapshot = Collector::new(tree.0.clone(), Some(tree.0.join("missing.sock"))).sample();
    assert!(snapshot.omen.is_none());
    assert!(snapshot.omen_error.is_some());
}

#[test]
fn malformed_inputs_leave_other_fields_available() {
    let tree = Tree::new();
    tree.write("proc/stat", "cpu 18446744073709551615 1 0 0 0\ncpu0 0 0 0 broken 0\n");
    tree.write("sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq", "1800000\n");
    tree.write("proc/loadavg", "NaN 0.75 -1.0\n");
    tree.write("proc/uptime", "NaN 0.0\n");
    tree.write("proc/meminfo", "MemTotal: bad kB\nMemAvailable: 10 kB\nCached: 25 kB\n");
    tree.write("sys/class/drm/card0/gt/gt0/rc6_residency_ms", "broken\n");
    tree.write("sys/class/drm/card0/gt_max_freq_mhz", "1000\n");

    let snapshot = tree.collector().sample();
    assert_eq!(snapshot.cpu.usage, 0.0);
    assert_eq!(snapshot.cpu.cores[0].usage, 0.0);
    assert_eq!(snapshot.cpu.cores[0].mhz, Some(1800));
    assert_eq!(snapshot.load, [0.0, 0.75, 0.0]);
    assert_eq!(snapshot.uptime_s, 0);
    assert_eq!(snapshot.memory.total_bytes, 0);
    assert_eq!(snapshot.memory.used_bytes, 0);
    assert_eq!(snapshot.memory.available_bytes, 10 * 1024);
    assert_eq!(snapshot.memory.cache_bytes, 25 * 1024);
    let gpu = snapshot.igpu.unwrap();
    assert_eq!(gpu.usage, None);
    assert_eq!(gpu.max_mhz, Some(1000));
}

#[test]
fn vpn_openconnect_connected_and_disconnected() {
    let tree = Tree::new();
    write_stat(&tree, 77, "openconnect", 1, 1, 10);
    tree.write("proc/77/status", "Uid:\t0\t0\t0\t0\nVmRSS: 9000 kB\n");
    tree.write("proc/77/cmdline", b"openconnect\0--protocol=gp\0--user=someone\0vpn.example.edu\0");
    let mut collector = tree.collector();

    // process running, tunnel not up yet
    let vpn = collector.sample_at(Instant::now()).vpn;
    assert_eq!(vpn.len(), 1);
    assert_eq!(vpn[0].name, "OpenConnect (GlobalProtect)");
    assert!(!vpn[0].connected);
    assert_eq!(vpn[0].interface, None);

    tree.write("sys/class/net/tun0/operstate", "unknown\n");
    let first = collector.sample_at(Instant::now()).vpn;
    assert!(first[0].connected);
    assert_eq!(first[0].interface.as_deref(), Some("tun0"));
    let since = first[0].since_ms;
    assert!(since.is_some());
    assert!(first[0].addresses.is_empty(), "addresses are only read on the real root");
    assert_eq!(collector.sample_at(Instant::now()).vpn[0].since_ms, since, "connected time is kept");

    tree.write("sys/class/net/tun0/operstate", "down\n");
    let down = collector.sample_at(Instant::now()).vpn;
    assert!(!down[0].connected);
    assert_eq!(down[0].since_ms, None);
}

#[test]
fn vpn_official_globalprotect_only_when_running_or_connected() {
    let tree = Tree::new();
    // installed but not running: not listed
    tree.write("opt/paloaltonetworks/globalprotect/PanGPS", "");
    tree.write("sys/class/net/wlo1/operstate", "up\n");
    assert!(tree.collector().sample_at(Instant::now()).vpn.is_empty());
    tree.write("sys/class/net/gpd0/operstate", "unknown\n");
    let vpn = tree.collector().sample_at(Instant::now()).vpn;
    assert_eq!((vpn[0].name.as_str(), vpn[0].connected), ("GlobalProtect", true));
}
