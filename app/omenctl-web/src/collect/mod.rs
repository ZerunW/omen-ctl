//! Collects system snapshots from procfs, sysfs and omenctld.

mod cpu;
mod gpu;
mod process;
mod vpn;

#[cfg(test)]
mod tests;

use crate::snapshot::{Memory, NetIf, Snapshot};
use omenctl_core::proto::{Client, Request, Response, Status};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct Collector {
    root: PathBuf,
    socket: Option<PathBuf>,
    previous_time: Option<Instant>,
    cpu: cpu::Counters,
    net: HashMap<String, (u64, u64)>,
    processes: process::Counters,
    ticks_per_second: f64,
    igpu: Option<(PathBuf, u64)>,
    nvidia_name: Option<String>,
    vpn_since: HashMap<String, u64>,
}

impl Collector {
    /// Reads system files under root and optionally queries the daemon socket.
    pub fn new(root: PathBuf, socket: Option<PathBuf>) -> Collector {
        Collector {
            root,
            socket,
            previous_time: None,
            cpu: cpu::Counters::default(),
            net: HashMap::new(),
            processes: HashMap::new(),
            ticks_per_second: process::clock_ticks_per_second(),
            igpu: None,
            nvidia_name: None,
            vpn_since: HashMap::new(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        self.sample_at(Instant::now())
    }

    /// Uses now to calculate rates relative to the previous sample.
    pub fn sample_at(&mut self, now: Instant) -> Snapshot {
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        let elapsed = self
            .previous_time
            .replace(now)
            .and_then(|previous| now.checked_duration_since(previous))
            .map_or(0.0, |duration| duration.as_secs_f64());

        let root = &self.root;
        let hostname = fs::read_to_string(root.join("proc/sys/kernel/hostname"))
            .map(|text| text.trim().to_owned())
            .unwrap_or_default();
        let uptime_s = fs::read_to_string(root.join("proc/uptime"))
            .ok()
            .and_then(|text| parse_nonnegative::<f64>(text.split_whitespace().next()?))
            .map_or(0, |seconds| seconds as u64);
        let mut load = [0.0; 3];
        if let Ok(text) = fs::read_to_string(root.join("proc/loadavg")) {
            for (value, field) in load.iter_mut().zip(text.split_whitespace()) {
                *value = parse_nonnegative::<f32>(field).unwrap_or_default();
            }
        }

        let cpu = cpu::sample(root, &mut self.cpu);
        let memory = memory(root);
        let net = network(root, &mut self.net, elapsed);
        let processes =
            process::sample(root, &mut self.processes, elapsed, self.ticks_per_second);
        let vpn = vpn::sample(root, &mut self.vpn_since, time_ms, &processes, self.ticks_per_second);
        let igpu = gpu::intel(root, &mut self.igpu, elapsed);
        let nvidia = gpu::nvidia(root, &mut self.nvidia_name);
        let (omen, omen_error) = match self.socket.as_deref().map(daemon_status) {
            Some(Ok(status)) => (Some(status), None),
            Some(Err(message)) => (None, Some(message)),
            None => (None, None),
        };

        Snapshot {
            time_ms,
            hostname,
            uptime_s,
            load,
            cpu,
            memory,
            igpu,
            nvidia,
            net,
            vpn,
            omen,
            omen_error,
            processes,
        }
    }
}

fn read_value<T: FromStr>(path: &Path) -> Option<T> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn parse_nonnegative<T: FromStr + Copy + Into<f64>>(field: &str) -> Option<T> {
    let value: T = field.parse().ok()?;
    let number = value.into();
    (number.is_finite() && number >= 0.0).then_some(value)
}

fn kib_bytes(field: &str) -> u64 {
    field
        .split_whitespace()
        .next()
        .and_then(|number| number.parse::<u64>().ok())
        .unwrap_or_default()
        .saturating_mul(1024)
}

fn memory(root: &Path) -> Memory {
    let mut memory = Memory::default();
    let mut cached = 0;
    let mut buffers = 0;
    let mut swap_free = 0;
    if let Ok(text) = fs::read_to_string(root.join("proc/meminfo")) {
        for (key, field) in text.lines().filter_map(|line| line.split_once(':')) {
            let value = match key {
                "MemTotal" => &mut memory.total_bytes,
                "MemAvailable" => &mut memory.available_bytes,
                "Cached" => &mut cached,
                "Buffers" => &mut buffers,
                "SwapTotal" => &mut memory.swap_total_bytes,
                "SwapFree" => &mut swap_free,
                _ => continue,
            };
            *value = kib_bytes(field);
        }
    }
    memory.used_bytes = memory.total_bytes.saturating_sub(memory.available_bytes);
    memory.cache_bytes = cached.saturating_add(buffers);
    memory.swap_used_bytes = memory.swap_total_bytes.saturating_sub(swap_free);
    memory
}

fn network(root: &Path, previous: &mut HashMap<String, (u64, u64)>, elapsed: f64) -> Vec<NetIf> {
    let mut interfaces = Vec::new();
    let mut current = HashMap::new();
    if let Ok(text) = fs::read_to_string(root.join("proc/net/dev")) {
        for (name, counters) in text.lines().filter_map(|line| line.rsplit_once(':')) {
            let name = name.trim();
            if matches!(name, "" | "lo" | "." | "..") || name.contains('/') {
                continue;
            }
            let mut fields = counters.split_whitespace();
            let rx = fields.next().and_then(|field| field.parse::<u64>().ok());
            let tx = fields.nth(7).and_then(|field| field.parse::<u64>().ok());
            let (Some(rx), Some(tx)) = (rx, tx) else {
                continue;
            };
            let (rx_bps, tx_bps) = match previous.get(name) {
                Some(&(old_rx, old_tx)) if elapsed > 0.0 => (
                    rx.saturating_sub(old_rx) as f64 / elapsed,
                    tx.saturating_sub(old_tx) as f64 / elapsed,
                ),
                _ => (0.0, 0.0),
            };
            let up = fs::read_to_string(root.join("sys/class/net").join(name).join("operstate"))
                // Tunnels (VPN, WireGuard) report "unknown" while they carry traffic.
                .is_ok_and(|value| matches!(value.trim(), "up" | "unknown"));
            current.insert(name.to_owned(), (rx, tx));
            interfaces.push(NetIf {
                name: name.to_owned(),
                up,
                rx_bps,
                tx_bps,
                rx_total_bytes: rx,
                tx_total_bytes: tx,
            });
        }
    }
    *previous = current;
    interfaces.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

fn daemon_status(socket: &Path) -> Result<Status, String> {
    let path = socket
        .to_str()
        .ok_or_else(|| format!("socket path is not UTF-8: {}", socket.display()))?;
    match Client::connect(path)?.call(&Request::Status)? {
        Response::Ok { status } => Ok(status),
        Response::Error { message } => Err(message),
        _ => Err("unexpected response to omenctld status request".into()),
    }
}
