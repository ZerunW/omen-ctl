//! VPN state: GlobalProtect (`gpd*`) and Tailscale (`tailscale0`).
//!
//! GlobalProtect creates its tunnel interface only while connected, so "the
//! interface exists and is not down" means connected. A VPN is listed when its
//! client is installed or its interface exists.

use crate::snapshot::Vpn;
use std::{collections::HashMap, fs, path::Path};

struct Kind {
    name: &'static str,
    /// Interface name, or prefix when it ends with '*'.
    interface: &'static str,
    /// Path (relative to root) whose existence means the client is installed.
    client: &'static str,
}

const KINDS: [Kind; 2] = [
    Kind { name: "GlobalProtect", interface: "gpd*", client: "opt/paloaltonetworks/globalprotect/PanGPS" },
    Kind { name: "Tailscale", interface: "tailscale0", client: "usr/sbin/tailscaled" },
];

fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// `since`: when each VPN was first seen connected (Unix ms), kept between samples.
pub(super) fn sample(root: &Path, since: &mut HashMap<&'static str, u64>, now_ms: u64) -> Vec<Vpn> {
    let interfaces: Vec<String> = fs::read_dir(root.join("sys/class/net"))
        .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for k in &KINDS {
        let iface = interfaces.iter().filter(|n| matches(k.interface, n)).min().cloned();
        let installed = root.join(k.client).exists();
        if iface.is_none() && !installed {
            since.remove(k.name);
            continue;
        }
        let connected = iface.as_ref().is_some_and(|i| {
            fs::read_to_string(root.join("sys/class/net").join(i).join("operstate")).is_ok_and(|s| s.trim() != "down")
        });
        let since_ms = if connected {
            Some(*since.entry(k.name).or_insert(now_ms))
        } else {
            since.remove(k.name);
            None
        };
        let addresses = match &iface {
            Some(i) if connected && root == Path::new("/") => addresses(i),
            _ => Vec::new(),
        };
        out.push(Vpn { name: k.name.into(), interface: iface, connected, addresses, since_ms });
    }
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
