//! omenctl-web: read-only system monitor over HTTP.
//!
//! - `GET /` serves the dashboard (assets/index.html, embedded);
//! - `GET /api/snapshot` serves the latest `Snapshot` as JSON;
//! - `GET /healthz` answers "ok".
//!
//! Nothing can be changed through it: it reads omenctld's status through the
//! read-only status socket. Sampling runs on its own thread and only while someone
//! is watching (a request in the last `IDLE_AFTER`), so an idle service costs
//! nothing and never wakes the NVIDIA GPU.
//!
//! Default listen address is 127.0.0.1:8787, for a reverse proxy or tunnel on the
//! same machine. With `--token-file`, every request must carry the token
//! (`Authorization: Bearer <token>`, or the cookie set by opening `/?token=<token>`).
//!
//! HTTP handling is deliberately minimal: GET/HEAD only, headers limited to
//! `MAX_HEAD` bytes and `REQUEST_DEADLINE`, request bodies never read, one
//! request per connection.

use omenctl_web::collect::Collector;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const INDEX: &str = include_str!("../assets/index.html");
const PERIOD: Duration = Duration::from_secs(2);
const IDLE_AFTER: Duration = Duration::from_secs(15);
/// A snapshot older than this is not served without waiting for a new one first.
const FRESH: Duration = Duration::from_secs(4);
/// Older than this, the data is reported as unavailable (503) instead of live.
const STALE: Duration = Duration::from_secs(10);
const COOKIE: &str = "omenctl_token";
const WORKERS: usize = 8;
const MAX_HEAD: usize = 16 * 1024;
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

const USAGE: &str = "usage: omenctl-web [--listen ADDR:PORT] [--socket PATH] [--token-file FILE] [--root DIR]

  --listen ADDR:PORT  address to serve on (default 127.0.0.1:8787; env OMENCTL_WEB_LISTEN)
  --socket PATH       omenctld read-only status socket (default /run/omenctl-status.sock)
  --token-file FILE   require the token in FILE on every request (env OMENCTL_WEB_TOKEN_FILE);
                      16 or more characters from A-Z a-z 0-9 - _
  --root DIR          prefix for /proc and /sys (testing)";

struct Args {
    listen: String,
    socket: PathBuf,
    token_file: Option<PathBuf>,
    root: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mut a = Args {
        listen: env("OMENCTL_WEB_LISTEN").unwrap_or_else(|| "127.0.0.1:8787".into()),
        socket: PathBuf::from(omenctl_core::proto::STATUS_SOCKET_PATH),
        token_file: env("OMENCTL_WEB_TOKEN_FILE").map(PathBuf::from),
        root: PathBuf::from("/"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--listen" => a.listen = val()?,
            "--socket" => a.socket = val()?.into(),
            "--token-file" => a.token_file = Some(val()?.into()),
            "--root" => a.root = val()?.into(),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {arg}\n{USAGE}")),
        }
    }
    Ok(a)
}

/// The token goes into a cookie and a URL, so only URL- and cookie-safe characters.
fn valid_token(t: &str) -> bool {
    t.len() >= 16 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn read_token(path: &PathBuf) -> Result<String, String> {
    let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t = t.trim().to_string();
    if !valid_token(&t) {
        return Err(format!("{}: token must be 16 or more characters from A-Z a-z 0-9 - _", path.display()));
    }
    Ok(t)
}

#[derive(Default)]
struct Shared {
    json: Option<(Instant, Arc<String>)>,
    last_request: Option<Instant>,
}

type State = Arc<(Mutex<Shared>, Condvar)>;

fn sampler(state: State, mut c: Collector) {
    let mut last_sample: Option<Instant> = None;
    loop {
        let watching = {
            let g = state.0.lock().unwrap_or_else(|e| e.into_inner());
            g.last_request.is_some_and(|t| t.elapsed() < IDLE_AFTER)
        };
        if !watching {
            last_sample = None;
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        if last_sample.is_some_and(|t| t.elapsed() < PERIOD) {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        // After a pause, take a quick first sample so rates have a baseline.
        if last_sample.is_none() {
            c.sample();
            std::thread::sleep(Duration::from_millis(500));
        }
        let snap = c.sample();
        last_sample = Some(Instant::now());
        match serde_json::to_string(&snap) {
            Ok(s) => {
                let mut g = state.0.lock().unwrap_or_else(|e| e.into_inner());
                g.json = Some((Instant::now(), Arc::new(s)));
                state.1.notify_all();
            }
            Err(e) => eprintln!("omenctl-web: cannot encode snapshot: {e}"),
        }
    }
}

/// Latest snapshot; waits up to 3 s for a fresh one after an idle period. None when
/// there is no data younger than `STALE`.
fn snapshot_json(state: &State) -> Option<Arc<String>> {
    let (lock, cv) = &**state;
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    g.last_request = Some(Instant::now());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !g.json.as_ref().is_some_and(|(t, _)| t.elapsed() < FRESH) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        g = cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    g.json.as_ref().filter(|(t, _)| t.elapsed() < STALE).map(|(_, s)| s.clone())
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Read the request line and headers (never the body) within the size and time limits.
fn read_request(s: &mut TcpStream) -> Result<Request, u16> {
    let start = Instant::now();
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() >= MAX_HEAD {
            return Err(431);
        }
        let left = REQUEST_DEADLINE.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Err(408);
        }
        s.set_read_timeout(Some(left)).map_err(|_| 500u16)?;
        match s.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return Err(408),
        }
    };
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().ok_or(400u16)?.split(' ');
    let (method, target) = (first.next().ok_or(400u16)?, first.next().ok_or(400u16)?);
    if !first.next().is_some_and(|v| v.starts_with("HTTP/1.")) {
        return Err(400);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Ok(Request { method: method.into(), path: path.into(), query: query.into(), headers })
}

const SECURITY_HEADERS: &str = "X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n\
Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'\r\n";

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &[u8], extra: &str, head_only: bool) {
    let reason = match code {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n{SECURITY_HEADERS}{extra}\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    if !head_only {
        let _ = s.write_all(body);
    }
    let _ = s.flush();
    let _ = s.shutdown(Shutdown::Write);
}

const TEXT: &str = "text/plain; charset=utf-8";

/// Constant-time comparison, so the token can't be guessed byte by byte from timing.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn authorized(req: &Request, token: &str) -> bool {
    if let Some(v) = req.header("Authorization").and_then(|v| v.strip_prefix("Bearer ")) {
        if same(v.trim(), token) {
            return true;
        }
    }
    req.header("Cookie").is_some_and(|c| {
        c.split(';').filter_map(|kv| kv.trim().split_once('=')).any(|(k, v)| k == COOKIE && same(v, token))
    })
}

fn query_token(query: &str) -> Option<&str> {
    query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == "token").map(|(_, v)| v)
}

fn serve(mut s: TcpStream, state: &State, token: Option<&str>) {
    s.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let req = match read_request(&mut s) {
        Ok(r) => r,
        Err(code) => return respond(&mut s, code, TEXT, b"bad request\n", "", false),
    };
    let head_only = req.method == "HEAD";
    if req.method != "GET" && !head_only {
        return respond(&mut s, 405, TEXT, b"method not allowed\n", "Allow: GET, HEAD\r\n", false);
    }
    if req.path == "/healthz" {
        return respond(&mut s, 200, TEXT, b"ok\n", "", head_only);
    }
    if let Some(token) = token {
        if !authorized(&req, token) {
            // Opening /?token=... once stores the token in a cookie and drops it from the URL.
            if req.path == "/" {
                if let Some(t) = query_token(&req.query).filter(|t| same(t, token)) {
                    let secure = req.header("X-Forwarded-Proto").is_some_and(|p| p.eq_ignore_ascii_case("https"));
                    let extra = format!(
                        "Set-Cookie: {COOKIE}={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{}\r\nLocation: ./\r\n",
                        if secure { "; Secure" } else { "" }
                    );
                    return respond(&mut s, 303, TEXT, b"", &extra, head_only);
                }
            }
            return respond(&mut s, 401, TEXT, b"unauthorized: open this page as /?token=<token>\n", "", head_only);
        }
    }
    match req.path.as_str() {
        "/" | "/index.html" => respond(&mut s, 200, "text/html; charset=utf-8", INDEX.as_bytes(), "Cache-Control: no-cache\r\n", head_only),
        "/api/snapshot" => match snapshot_json(state) {
            Some(json) => respond(&mut s, 200, "application/json", json.as_bytes(), "Cache-Control: no-store\r\n", head_only),
            None => respond(&mut s, 503, TEXT, b"no current data\n", "Retry-After: 2\r\nCache-Control: no-store\r\n", head_only),
        },
        _ => respond(&mut s, 404, TEXT, b"not found\n", "", head_only),
    }
}

fn run() -> Result<(), String> {
    let a = parse_args()?;
    let token = a.token_file.as_ref().map(read_token).transpose()?;
    let state: State = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
    let collector = Collector::new(a.root.clone(), Some(a.socket.clone()));
    {
        let state = state.clone();
        std::thread::Builder::new()
            .name("sampler".into())
            .spawn(move || sampler(state, collector))
            .map_err(|e| format!("cannot start sampler: {e}"))?;
    }
    let listener = TcpListener::bind(&a.listen).map_err(|e| format!("{}: {e}", a.listen))?;
    eprintln!(
        "omenctl-web {} listening on http://{}{}",
        env!("CARGO_PKG_VERSION"),
        a.listen,
        if token.is_some() { " (token required)" } else { "" }
    );
    // A fixed number of workers, each handling one connection at a time.
    let token: Arc<Option<String>> = Arc::new(token);
    let workers: Vec<_> = (0..WORKERS)
        .map(|i| {
            let listener = listener.try_clone().map_err(|e| format!("listener: {e}"))?;
            let (state, token) = (state.clone(), token.clone());
            std::thread::Builder::new()
                .name(format!("http-{i}"))
                .spawn(move || loop {
                    match listener.accept() {
                        Ok((s, _)) => serve(s, &state, token.as_deref()),
                        Err(e) => {
                            eprintln!("omenctl-web: accept: {e}");
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                })
                .map_err(|e| format!("cannot start worker: {e}"))
        })
        .collect::<Result<_, String>>()?;
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("omenctl-web: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_rules_and_query() {
        assert!(same("abcdefghijklmnop", "abcdefghijklmnop"));
        assert!(!same("abcdefghijklmnop", "abcdefghijklmnoq"));
        assert!(!same("abc", "abcd"));
        assert!(valid_token("Abc-def_0123456789"));
        assert!(!valid_token("short"));
        assert!(!valid_token("abcdefghijklmnop;x"));
        assert_eq!(query_token("a=1&token=xyz"), Some("xyz"));
        assert_eq!(query_token(""), None);
    }

    fn roundtrip(raw: &[u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let raw = raw.to_vec();
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            // The server may answer and close before reading everything (oversized
            // requests), so a reset here is fine: keep whatever reply arrived.
            let _ = c.write_all(&raw);
            let mut out = Vec::new();
            let _ = c.read_to_end(&mut out);
            String::from_utf8_lossy(&out).into_owned()
        });
        let (s, _) = l.accept().unwrap();
        let state: State = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        state.0.lock().unwrap().json = Some((Instant::now(), Arc::new("{}".into())));
        serve(s, &state, Some("abcdefghijklmnop"));
        client.join().unwrap()
    }

    #[test]
    fn http_auth_redirect_and_limits() {
        assert!(roundtrip(b"GET /healthz HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 200"));
        assert!(roundtrip(b"GET / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 401"));
        let r = roundtrip(b"GET /?token=abcdefghijklmnop HTTP/1.1\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 303") && r.contains("Location: ./\r\n") && r.contains("HttpOnly"), "{r}");
        // the token login only works on "/" and never redirects elsewhere
        assert!(roundtrip(b"GET //evil.example/?token=abcdefghijklmnop HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 401"));
        let ok = roundtrip(b"GET /api/snapshot HTTP/1.1\r\nCookie: omenctl_token=abcdefghijklmnop\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("{}"), "{ok}");
        let bearer = roundtrip(b"GET /api/snapshot HTTP/1.1\r\nAuthorization: Bearer abcdefghijklmnop\r\n\r\n");
        assert!(bearer.starts_with("HTTP/1.1 200"));
        // a declared body is never read or allocated
        let post = roundtrip(b"POST / HTTP/1.1\r\nContent-Length: 999999999999\r\n\r\n");
        assert!(post.starts_with("HTTP/1.1 405"));
        let mut huge = b"GET / HTTP/1.1\r\nX: ".to_vec();
        huge.extend(std::iter::repeat_n(b'a', MAX_HEAD + 10));
        huge.extend_from_slice(b"\r\n\r\n");
        assert!(roundtrip(&huge).starts_with("HTTP/1.1 431"));
    }
}
