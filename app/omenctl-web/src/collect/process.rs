use super::kib_bytes;
use crate::snapshot::Process;
use std::{collections::HashMap, fs, io::Read, path::Path};

const CMD_CHARS: usize = 300;

pub(super) type Counters = HashMap<u32, ProcessTicks>;

pub(super) struct ProcessTicks {
    total: u64,
    start_time: Option<u64>,
}

pub(super) fn clock_ticks_per_second() -> f64 {
    // SAFETY: sysconf accepts _SC_CLK_TCK and takes no pointers.
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as f64 }
}

pub(super) fn sample(
    root: &Path,
    previous: &mut Counters,
    elapsed: f64,
    ticks_per_second: f64,
) -> Vec<Process> {
    let mut current = Counters::new();
    let mut processes = Vec::new();
    let can_compute_cpu = elapsed > 0.0 && ticks_per_second > 0.0;
    if let Ok(entries) = fs::read_dir(root.join("proc")) {
        let passwd = fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
        let users: HashMap<u32, &str> = passwd
            .lines()
            .filter_map(|line| {
                let mut fields = line.split(':');
                let name = fields.next()?;
                let uid = fields.nth(1)?.parse().ok()?;
                Some((uid, name))
            })
            .collect();

        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.as_encoded_bytes().iter().all(u8::is_ascii_digit) {
                continue;
            }
            let Some(pid) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Some((mut process, ticks)) = read_process(&entry.path(), pid, &users) else {
                continue;
            };
            if let Some(old) = previous
                .get(&pid)
                .filter(|old| can_compute_cpu && old.start_time == ticks.start_time)
            {
                let delta = ticks.total.saturating_sub(old.total);
                process.cpu = (100.0 * delta as f64 / ticks_per_second / elapsed) as f32;
            }
            current.insert(pid, ticks);
            processes.push(process);
        }
    }
    *previous = current;
    processes.sort_unstable_by(|a, b| {
        b.cpu
            .total_cmp(&a.cpu)
            .then_with(|| b.mem_bytes.cmp(&a.mem_bytes))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    processes
}

fn read_process(
    dir: &Path,
    pid: u32,
    users: &HashMap<u32, &str>,
) -> Option<(Process, ProcessTicks)> {
    let stat = fs::read(dir.join("stat")).ok()?;
    let stat = String::from_utf8_lossy(&stat);
    let (_, stat) = stat.split_once('(')?;
    let (name, fields) = stat.rsplit_once(')')?;
    let mut fields = fields.split_whitespace();
    let state = fields.next()?;
    if state.len() != 1 || !state.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    let ppid = fields.next()?.parse().ok()?;
    let utime: u64 = fields.nth(9)?.parse().ok()?;
    let stime = fields.next()?.parse().ok()?;
    let nice = fields.nth(3)?.parse().ok()?;
    let threads = fields.next()?.parse().ok()?;
    let ticks = ProcessTicks {
        total: utime.checked_add(stime)?,
        start_time: fields.nth(1).and_then(|field| field.parse().ok()),
    };

    let status = fs::read(dir.join("status")).ok()?;
    let status = String::from_utf8_lossy(&status);
    let mut mem_bytes = 0;
    let mut uid = None;
    for (key, field) in status.lines().filter_map(|line| line.split_once(':')) {
        match key {
            "VmRSS" => mem_bytes = kib_bytes(field),
            "Uid" => {
                uid = field.split_whitespace().next().and_then(|value| value.parse().ok());
            }
            _ => {}
        }
    }
    let user = uid.map_or_else(String::new, |uid| {
        users
            .get(&uid)
            .map_or_else(|| uid.to_string(), |name| (*name).to_owned())
    });

    // Four bytes per UTF-8 character, plus one byte to see a trailing NUL.
    let mut cmdline = Vec::new();
    fs::File::open(dir.join("cmdline"))
        .ok()?
        .take((CMD_CHARS * 4 + 1) as u64)
        .read_to_end(&mut cmdline)
        .ok()?;
    let cmd = if cmdline.is_empty() {
        std::iter::once('[')
            .chain(name.chars())
            .chain(std::iter::once(']'))
            .take(CMD_CHARS)
            .collect()
    } else {
        let args = cmdline.strip_suffix(b"\0").unwrap_or(&cmdline);
        String::from_utf8_lossy(args)
            .chars()
            .map(|c| if c == '\0' { ' ' } else { c })
            .take(CMD_CHARS)
            .collect()
    };

    Some((
        Process {
            pid,
            ppid,
            user,
            // stat exposes the same task name as /proc/<pid>/comm.
            name: name.to_owned(),
            cmd,
            cpu: 0.0,
            mem_bytes,
            state: state.to_owned(),
            threads,
            nice,
        },
        ticks,
    ))
}
