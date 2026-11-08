//! Minimal sd_notify(3): send "READY=1", "WATCHDOG=1", ... to $NOTIFY_SOCKET.
//! Supports filesystem paths and abstract sockets ("@name"). Does nothing when the
//! daemon was not started by systemd (no NOTIFY_SOCKET).

use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::Duration;

pub struct Notifier {
    sock: Option<(UnixDatagram, SocketAddr)>,
    /// Interval systemd expects WATCHDOG=1 at (WATCHDOG_USEC), if enabled for us.
    pub watchdog: Option<Duration>,
}

fn parse_addr(path: &str) -> Option<SocketAddr> {
    if let Some(name) = path.strip_prefix('@') {
        use std::os::linux::net::SocketAddrExt;
        SocketAddr::from_abstract_name(name.as_bytes()).ok()
    } else if path.starts_with('/') {
        SocketAddr::from_pathname(path).ok()
    } else {
        None
    }
}

/// WATCHDOG_USEC applies only if WATCHDOG_PID is unset or equals our pid.
fn watchdog_from_env(usec: Option<String>, pid: Option<String>, me: u32) -> Option<Duration> {
    if let Some(p) = pid {
        if p.trim().parse::<u32>().ok() != Some(me) {
            return None;
        }
    }
    let us: u64 = usec?.trim().parse().ok()?;
    (us > 0).then(|| Duration::from_micros(us))
}

impl Notifier {
    pub fn from_env() -> Notifier {
        let sock = std::env::var("NOTIFY_SOCKET").ok().and_then(|p| {
            let addr = parse_addr(&p)?;
            let s = UnixDatagram::unbound().ok()?;
            Some((s, addr))
        });
        let watchdog = watchdog_from_env(
            std::env::var("WATCHDOG_USEC").ok(),
            std::env::var("WATCHDOG_PID").ok(),
            std::process::id(),
        );
        Notifier { sock, watchdog }
    }

    pub fn notify(&self, msg: &str) {
        if let Some((s, addr)) = &self.sock {
            if let Err(e) = s.send_to_addr(msg.as_bytes(), addr) {
                eprintln!("sd_notify({msg}): {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_to_path_socket() {
        let dir = std::env::temp_dir().join(format!("omenctld-sdn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("notify");
        let _ = std::fs::remove_file(&p);
        let rx = UnixDatagram::bind(&p).unwrap();
        let n = Notifier {
            sock: Some((UnixDatagram::unbound().unwrap(), parse_addr(p.to_str().unwrap()).unwrap())),
            watchdog: None,
        };
        n.notify("READY=1");
        let mut buf = [0u8; 64];
        let k = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..k], b"READY=1");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sends_to_abstract_socket() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("omenctld-test-{}", std::process::id());
        let rx = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
        let n = Notifier {
            sock: Some((UnixDatagram::unbound().unwrap(), parse_addr(&format!("@{name}")).unwrap())),
            watchdog: None,
        };
        n.notify("WATCHDOG=1");
        let mut buf = [0u8; 64];
        let k = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..k], b"WATCHDOG=1");
    }

    #[test]
    fn watchdog_env_rules() {
        assert_eq!(watchdog_from_env(Some("15000000".into()), None, 7), Some(Duration::from_secs(15)));
        assert_eq!(watchdog_from_env(Some("15000000".into()), Some("7".into()), 7), Some(Duration::from_secs(15)));
        assert_eq!(watchdog_from_env(Some("15000000".into()), Some("8".into()), 7), None);
        assert_eq!(watchdog_from_env(None, None, 7), None);
        assert_eq!(watchdog_from_env(Some("0".into()), None, 7), None);
        assert!(parse_addr("relative").is_none());
    }
}
