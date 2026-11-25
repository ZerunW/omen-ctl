//! JSON served at `GET /api/snapshot`. This is the contract between the
//! collectors (`collect`) and the dashboard (`assets/index.html`).
//!
//! Units: percentages are 0..=100, sizes are bytes, rates are bytes per
//! second, temperatures are degrees Celsius, power is watts. `None` (JSON
//! `null`) means "not available right now", never zero.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unix time of the sample, milliseconds.
    pub time_ms: u64,
    pub hostname: String,
    pub uptime_s: u64,
    /// 1, 5 and 15 minute load averages.
    pub load: [f32; 3],
    pub cpu: Cpu,
    pub memory: Memory,
    /// Intel integrated GPU (i915), if present.
    pub igpu: Option<Igpu>,
    /// NVIDIA GPU, if the driver is present.
    pub nvidia: Option<Nvidia>,
    /// Network interfaces except `lo`, sorted by name.
    pub net: Vec<NetIf>,
    /// VPN clients that are installed or have an interface (GlobalProtect, Tailscale).
    #[serde(default)]
    pub vpn: Vec<Vpn>,
    /// omenctld status (mode, fans, temperatures, power, battery). `None` when
    /// the service cannot be reached; the reason is in `omen_error`.
    pub omen: Option<omenctl_core::proto::Status>,
    pub omen_error: Option<String>,
    /// Every process, sorted by `cpu` descending, then `mem_bytes` descending.
    pub processes: Vec<Process>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    /// "model name" from /proc/cpuinfo.
    pub model: String,
    /// Busy share of all logical CPUs since the previous sample.
    pub usage: f32,
    /// One entry per logical CPU, ordered by `id`.
    pub cores: Vec<Core>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Core {
    pub id: u32,
    /// Busy share since the previous sample (busy = total - idle - iowait).
    pub usage: f32,
    /// Current clock from cpufreq scaling_cur_freq.
    pub mhz: Option<u32>,
    /// "P" (performance) or "E" (efficiency) on hybrid CPUs, from
    /// /sys/devices/cpu_core/cpus and /sys/devices/cpu_atom/cpus.
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub total_bytes: u64,
    /// total - available.
    pub used_bytes: u64,
    pub available_bytes: u64,
    /// Cached + Buffers.
    pub cache_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Igpu {
    pub name: String,
    /// 100 - share of time in RC6 (idle) since the previous sample.
    pub usage: Option<f32>,
    pub mhz: Option<u32>,
    pub max_mhz: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Nvidia {
    pub name: String,
    /// false while the GPU is in runtime suspend (D3). Nothing else is read
    /// then, so all values below are `None` and `processes` is empty.
    pub awake: bool,
    pub usage: Option<f32>,
    /// Memory controller busy share.
    pub mem_usage: Option<f32>,
    pub mem_used_bytes: Option<u64>,
    pub mem_total_bytes: Option<u64>,
    pub temp: Option<f32>,
    pub power_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    pub mhz: Option<u32>,
    /// What `nvidia-smi` lists under "Processes".
    pub processes: Vec<GpuProcess>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuProcess {
    pub pid: u32,
    /// /proc/<pid>/comm, or "?" if the process is gone or hidden.
    pub name: String,
    /// "compute" or "graphics".
    pub kind: String,
    pub mem_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetIf {
    pub name: String,
    /// operstate is "up", or "unknown" (tunnels such as VPNs report that).
    pub up: bool,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_total_bytes: u64,
    pub tx_total_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Vpn {
    /// e.g. "OpenConnect (GlobalProtect)", "GlobalProtect", "Tailscale".
    pub name: String,
    /// Tunnel interface, if it exists (e.g. "tun0", "gpd0").
    pub interface: Option<String>,
    /// The tunnel interface exists and is not down.
    pub connected: bool,
    /// "connected", "connecting" (OpenConnect running but no tunnel yet: logging in,
    /// e.g. waiting for a Duo push to be approved) or "disconnected".
    #[serde(default)]
    pub state: String,
    /// While connecting: when the client process started (Unix ms).
    #[serde(default)]
    pub connecting_since_ms: Option<u64>,
    /// Server the client connects to (OpenConnect's host argument), if known.
    #[serde(default)]
    pub server: Option<String>,
    /// IPv4 and global IPv6 addresses of the tunnel while connected.
    pub addresses: Vec<String>,
    /// When this service first saw it connected (Unix ms); None while disconnected.
    pub since_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    /// /proc/<pid>/comm.
    pub name: String,
    /// Command line, arguments joined by spaces, at most 300 characters;
    /// `[name]` for kernel threads.
    pub cmd: String,
    /// CPU time since the previous sample as a share of ONE logical CPU
    /// (can exceed 100 for multi-threaded processes, like top).
    pub cpu: f32,
    /// Resident set size.
    pub mem_bytes: u64,
    /// One-letter state from /proc/<pid>/stat (R, S, D, Z, ...).
    pub state: String,
    pub threads: u32,
    pub nice: i32,
}
