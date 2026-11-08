//! Unix socket server. Accepts connections on a background thread; each connection
//! gets its own thread that reads one JSON request per line and forwards it to the
//! main loop (which owns the controller) over a channel, then writes the reply.

use omenctl_core::proto::{Request, Response, MAX_LINE};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::Duration;

const MAX_CLIENTS: usize = 16;

pub struct Job {
    pub req: Request,
    pub reply: SyncSender<Response>,
}

/// Create the socket: remove a stale one, bind, then chmod 0660 and chgrp `group`
/// (falls back to 0600 root-only if the group does not exist).
pub fn bind(path: &Path, group: &str) -> std::io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let l = UnixListener::bind(path)?;
    match lookup_gid(group) {
        Some(gid) => {
            std::os::unix::fs::chown(path, None, Some(gid))?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
        }
        None => {
            eprintln!("group '{group}' not found: socket is root-only (0600)");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(l)
}

fn lookup_gid(group: &str) -> Option<u32> {
    let c = std::ffi::CString::new(group).ok()?;
    // SAFETY: getgrnam returns NULL or a pointer to static storage; we only read gr_gid
    // right away on this single thread (called once at startup before other threads).
    let g = unsafe { libc::getgrnam(c.as_ptr()) };
    if g.is_null() {
        None
    } else {
        Some(unsafe { (*g).gr_gid })
    }
}

pub fn spawn(listener: UnixListener, jobs: Sender<Job>) {
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            if active.load(Ordering::SeqCst) >= MAX_CLIENTS {
                let _ = reply_line(&stream, &Response::Error { message: "too many clients".into() });
                continue;
            }
            active.fetch_add(1, Ordering::SeqCst);
            let jobs = jobs.clone();
            let active = active.clone();
            std::thread::spawn(move || {
                handle(stream, jobs);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
}

fn reply_line(mut s: &UnixStream, r: &Response) -> std::io::Result<()> {
    let mut line = serde_json::to_string(r).map_err(std::io::Error::other)?;
    line.push('\n');
    s.write_all(line.as_bytes())
}

fn handle(stream: UnixStream, jobs: Sender<Job>) {
    // idle clients are dropped after 10 min; the GUI polls every second
    stream.set_read_timeout(Some(Duration::from_secs(600))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let Ok(wr) = stream.try_clone() else { return };
    let mut rd = BufReader::new(stream);
    loop {
        let mut line = String::new();
        // read at most MAX_LINE + 1 bytes so a huge line can't eat memory
        match (&mut rd).take(MAX_LINE as u64 + 1).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.len() > MAX_LINE {
            let _ = reply_line(&wr, &Response::Error { message: "request too long".into() });
            return;
        }
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Err(e) => Response::Error { message: format!("bad request: {e}") },
            Ok(req) => {
                let (tx, rx): (SyncSender<Response>, Receiver<Response>) = sync_channel(1);
                if jobs.send(Job { req, reply: tx }).is_err() {
                    return; // daemon shutting down
                }
                match rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(r) => r,
                    Err(_) => Response::Error { message: "daemon did not answer".into() },
                }
            }
        };
        if reply_line(&wr, &resp).is_err() {
            return;
        }
    }
}
