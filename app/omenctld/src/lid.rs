//! Manage the logind lid-switch inhibitor child.

use omenctl_core::config::LidAction;
use omenctl_core::controller::lid_should_ignore;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const RETRY_S: f64 = 30.0;
/// systemd-inhibit exits at once if logind refuses the lock; only a child that is
/// still running after this long is reported as holding it.
const CONFIRM: Duration = Duration::from_secs(2);

pub struct Lid {
    enabled: bool,
    child: Option<(Child, Instant)>,
    failure: Option<(f64, String)>,
}

impl Lid {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, child: None, failure: None }
    }

    pub fn inhibited(&self) -> bool {
        self.child.as_ref().is_some_and(|(_, since)| since.elapsed() >= CONFIRM)
    }

    pub fn alert(&self) -> Option<&str> {
        self.failure.as_ref().map(|(_, message)| message.as_str())
    }

    pub fn update(&mut self, now: f64, close: LidAction, ac: Option<bool>) {
        if !self.enabled {
            return;
        }
        if !lid_should_ignore(close, ac) {
            self.release();
            self.failure = None;
            return;
        }

        if let Some((child, _)) = &mut self.child {
            let message = match child.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) => format!("inhibitor exited unexpectedly ({status})"),
                Err(e) => format!("checking inhibitor: {e}"),
            };
            self.release();
            self.failed(now, message);
        }
        if self.failure.as_ref().is_some_and(|(t, _)| now - t < RETRY_S) {
            return;
        }

        match spawn() {
            Ok(child) => {
                self.child = Some((child, Instant::now()));
                self.failure = None;
                let qualifier = if close == LidAction::IgnoreOnAc { " (on AC)" } else { "" };
                eprintln!("lid close: ignored{qualifier}");
            }
            Err(e) => self.failed(now, format!("starting inhibitor: {e}")),
        }
    }

    fn failed(&mut self, now: f64, message: String) {
        let message = format!("lid close: {message}");
        eprintln!("{message}");
        self.failure = Some((now, message));
    }

    pub fn release(&mut self) {
        if let Some((mut child, _)) = self.child.take() {
            let _ = child.kill();
            if let Err(e) = child.wait() {
                eprintln!("lid close: waiting for inhibitor: {e}");
            }
            eprintln!("lid close: normal");
        }
    }
}

fn spawn() -> io::Result<Child> {
    let mut cmd = Command::new("systemd-inhibit");
    cmd.args([
        "--what=handle-lid-switch",
        "--who=omenctl",
        "--why=Lid close ignored (omenctl)",
        "--mode=block",
        "sleep",
        "infinity",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::inherit());
    let parent = unsafe { libc::getpid() };
    // Only async-signal-safe operations run between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::signal(libc::SIGTERM, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(io::Error::last_os_error());
            }
            let zero: libc::c_ulong = 0;
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, zero, zero, zero) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    cmd.spawn()
}
