//! VPN state: OpenConnect (any protocol, e.g. GlobalProtect), the official
//! GlobalProtect client (`gpd*`) and Tailscale (`tailscale0`).
//!
//! A tunnel interface that exists and is not down counts as connected. A VPN is
//! listed while its client process runs or its interface exists (Tailscale: also
//! while it is installed, since tailscaled normally runs all the time).

use crate::snapshot::{Process, Vpn};
use std::{collections::HashMap, fs, path::Path};

fn interfaces(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root.join("sys/class/net"))
        .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn is_up(root: &Path, iface: &str) -> bool {
    fs::read_to_string(root.join("sys/class/net").join(iface).join("operstate")).is_ok_and(|s| s.trim() != "down")
}

/// Value of `--name=value`, `--name value` or `-s value` in a space-joined command line.
fn arg<'a>(cmd: &'a str, long: &str, short: Option<&str>) -> Option<&'a str> {
    let mut words = cmd.split_whitespace();
    while let Some(w) = words.next() {
        if let Some(v) = w.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
            return Some(v);
        }
        if w == long || short == Some(w) {
            return words.next();
        }
    }
    None
}

fn protocol_name(p: &str) -> String {
    match p {
        "gp" => "GlobalProtect".into(),
        "anyconnect" => "AnyConnect".into(),
        "nc" | "pulse" => "Pulse".into(),
        "f5" => "F5".into(),
        "fortinet" => "Fortinet".into(),
        "array" => "Array".into(),
        other => other.into(),
    }
}

/// `since`: when each VPN was first seen connected (Unix ms), kept between samples.
pub(super) fn sample(root: &Path, since: &mut HashMap<String, u64>, now_ms: u64, processes: &[Process]) -> Vec<Vpn> {
    let ifaces = interfaces(root);
    // (name, interface, listed)
    let mut found: Vec<(String, Option<String>)> = Vec::new();

    // OpenConnect: one entry per running openconnect process.
    let mut taken: Vec<String> = Vec::new();
    for p in processes.iter().filter(|p| p.name == "openconnect") {
        let proto = arg(&p.cmd, "--protocol", None).map_or_else(|| "AnyConnect".to_string(), protocol_name);
        let wanted = arg(&p.cmd, "--interface", Some("-i")).map(str::to_owned);
        let iface = match wanted {
            Some(i) => ifaces.contains(&i).then_some(i),
            None => ifaces.iter().find(|i| (i.starts_with("tun") || i.starts_with("vpn")) && !taken.contains(i)).cloned(),
        };
        if let Some(i) = &iface {
            taken.push(i.clone());
        }
        found.push((format!("OpenConnect ({proto})"), iface));
    }

    // Official GlobalProtect client.
    let gpd = ifaces.iter().find(|i| i.starts_with("gpd")).cloned();
    if gpd.is_some() || processes.iter().any(|p| p.name == "PanGPS") {
        found.push(("GlobalProtect".into(), gpd));
    }

    // Tailscale.
    let ts = ifaces.iter().find(|i| *i == "tailscale0").cloned();
    if ts.is_some() || root.join("usr/sbin/tailscaled").exists() {
        found.push(("Tailscale".into(), ts));
    }

    let mut out = Vec::new();
    let mut seen = Vec::new();
    for (name, iface) in found {
        let connected = iface.as_deref().is_some_and(|i| is_up(root, i));
        let since_ms = if connected { Some(*since.entry(name.clone()).or_insert(now_ms)) } else { None };
        if !connected {
            since.remove(&name);
        }
        let addresses = match &iface {
            Some(i) if connected && root == Path::new("/") => addresses(i),
            _ => Vec::new(),
        };
        seen.push(name.clone());
        out.push(Vpn { name, interface: iface, connected, addresses, since_ms });
    }
    since.retain(|k, _| seen.contains(k));
    out
}

/// IPv4 and global IPv6 addresses of one interface (getifaddrs).
fn addresses(interface: &str) -> Vec<String> {
    let mut list = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we only read and then free once.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return list;
    }
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list returned above.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated string owned by the list.
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) };
        if name.to_bytes() != interface.as_bytes() {
            continue;
        }
        // SAFETY: the family field tells which sockaddr type ifa_addr points to.
        match i32::from(unsafe { (*ifa.ifa_addr).sa_family }) {
            libc::AF_INET => {
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                list.push(std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr)).to_string());
            }
            libc::AF_INET6 => {
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                let ip = std::net::Ipv6Addr::from(a.sin6_addr.s6_addr);
                // Skip link-local (fe80::/10).
                if ip.segments()[0] & 0xffc0 != 0xfe80 {
                    list.push(ip.to_string());
                }
            }
            _ => {}
        }
    }
    // SAFETY: `head` came from getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    list
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn openconnect_arguments() {
        let cmd = "openconnect --protocol=gp --user=someone --authgroup DC Gateway -i vpn7 vpn.example.edu";
        assert_eq!(arg(cmd, "--protocol", None), Some("gp"));
        assert_eq!(arg(cmd, "--interface", Some("-i")), Some("vpn7"));
        assert_eq!(arg("openconnect --interface tun3 x", "--interface", Some("-i")), Some("tun3"));
        assert_eq!(arg("openconnect x", "--protocol", None), None);
        assert_eq!(protocol_name("gp"), "GlobalProtect");
    }
}
