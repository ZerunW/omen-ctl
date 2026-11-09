//! Persistent files: saved state (/var/lib/omenctl/state.json) and the config
//! (/etc/omenctl/config.toml). All writes are atomic (temp file + fsync + rename).

use omenctl_core::config::Config;
use omenctl_core::{FanMode, Mode};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub mode: Mode,
    pub fan: FanMode,
}

pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Missing file = None. Unreadable/corrupt file = None with a log line (we never
/// refuse to start because of it; the safe default mode is used instead).
pub fn load_state(path: &Path) -> Option<State> {
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("{}: {e}; using defaults", path.display());
            return None;
        }
    };
    match serde_json::from_str(&s) {
        Ok(st) => Some(st),
        Err(e) => {
            eprintln!("{}: {e}; using defaults", path.display());
            None
        }
    }
}

pub fn save_state(path: &Path, st: &State) -> std::io::Result<()> {
    let mut s = serde_json::to_string_pretty(st).map_err(std::io::Error::other)?;
    s.push('\n');
    write_atomic(path, s.as_bytes(), 0o644)
}

/// Missing config = defaults. An invalid config is an error: the daemon exits and
/// systemd's ExecStopPost safety script leaves the machine in a safe state.
pub fn load_config(path: &Path) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Config::from_toml(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default().validated()?),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write new (already validated) curves into the config file, keeping comments.
pub fn save_curves(
    path: &Path,
    cpu: &omenctl_core::curve::FanCurve,
    gpu: &omenctl_core::curve::FanCurve,
) -> Result<(), String> {
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let out = omenctl_core::config::with_curves(&existing, cpu, gpu)?;
    write_atomic(path, out.as_bytes(), 0o644).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write new (already validated) mode rules into the config file, keeping comments.
pub fn save_modes(path: &Path, m: &omenctl_core::config::ModesConfig) -> Result<(), String> {
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let out = omenctl_core::config::with_modes(&existing, m)?;
    write_atomic(path, out.as_bytes(), 0o644).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip_and_corrupt_file() {
        let d = std::env::temp_dir().join(format!("omenctld-files-{}", std::process::id()));
        let p = d.join("sub/state.json");
        assert_eq!(load_state(&p), None);
        let st = State { mode: Mode::Unleashed, fan: FanMode::Fixed(33) };
        save_state(&p, &st).unwrap();
        assert_eq!(load_state(&p), Some(st));
        std::fs::write(&p, "{garbage").unwrap();
        assert_eq!(load_state(&p), None);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn config_missing_invalid_and_curve_save() {
        let d = std::env::temp_dir().join(format!("omenctld-cfg-{}", std::process::id()));
        let p = d.join("config.toml");
        assert!(load_config(&p).is_ok());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(&p, "[protect]\ndrop_at = 120.0\n").unwrap();
        assert!(load_config(&p).is_err());
        std::fs::write(&p, "# hello\n").unwrap();
        let cpu = omenctl_core::curve::FanCurve::bios_default_cpu().validated().unwrap();
        save_curves(&p, &cpu, &cpu).unwrap();
        let c = load_config(&p).unwrap();
        assert_eq!(c.fan.gpu_curve, cpu);
        assert!(std::fs::read_to_string(&p).unwrap().contains("# hello"));
        std::fs::remove_dir_all(d).ok();
    }
}
