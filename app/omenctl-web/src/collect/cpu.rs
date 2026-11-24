use super::read_value;
use crate::snapshot::{Core, Cpu};
use std::{collections::BTreeMap, fs, ops::RangeInclusive, path::Path};

#[derive(Default)]
pub(super) struct Counters {
    total: Option<Ticks>,
    cores: BTreeMap<u32, Option<Ticks>>,
}

#[derive(Clone, Copy)]
struct Ticks {
    busy: u64,
    total: u64,
}

impl Ticks {
    fn parse<'a>(fields: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut total = 0_u64;
        let mut idle = 0;
        let mut count = 0;
        // Guest time is already included in user and nice time.
        for (index, field) in fields.take(8).enumerate() {
            let ticks = field.parse::<u64>().ok()?;
            total = total.checked_add(ticks)?;
            if matches!(index, 3 | 4) {
                idle += ticks;
            }
            count += 1;
        }
        (count >= 4).then_some(Self {
            busy: total - idle,
            total,
        })
    }

    fn usage(self, previous: Option<Self>) -> f32 {
        let Some(previous) = previous else {
            return 0.0;
        };
        let total = self.total.saturating_sub(previous.total);
        let busy = self.busy.saturating_sub(previous.busy);
        if total == 0 {
            return 0.0;
        }
        (100.0 * busy.min(total) as f64 / total as f64) as f32
    }
}

pub(super) fn sample(root: &Path, previous: &mut Counters) -> Cpu {
    let model = fs::read_to_string(root.join("proc/cpuinfo"))
        .ok()
        .and_then(|text| {
            text.lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.trim() == "model name")
                .map(|(_, model)| model.trim().to_owned())
        })
        .unwrap_or_default();

    let mut current = Counters::default();
    if let Ok(text) = fs::read_to_string(root.join("proc/stat")) {
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            match fields.next() {
                Some("cpu") => current.total = Ticks::parse(fields),
                Some(label) => {
                    if let Some(id) = label.strip_prefix("cpu").and_then(cpu_id) {
                        // Keep the core and its metadata even if its counters are malformed.
                        current.cores.insert(id, Ticks::parse(fields));
                    }
                }
                None => {}
            }
        }
    }

    let performance = cpu_list(&root.join("sys/devices/cpu_core/cpus"));
    let efficiency = cpu_list(&root.join("sys/devices/cpu_atom/cpus"));
    let cpu = Cpu {
        model,
        usage: current
            .total
            .map_or(0.0, |ticks| ticks.usage(previous.total)),
        cores: current
            .cores
            .iter()
            .map(|(&id, &ticks)| {
                let path = root.join(format!(
                    "sys/devices/system/cpu/cpu{id}/cpufreq/scaling_cur_freq"
                ));
                let kind = [(&performance, "P"), (&efficiency, "E")]
                    .into_iter()
                    .find(|(list, _)| list.iter().any(|range| range.contains(&id)))
                    .map(|(_, kind)| kind.to_owned());
                Core {
                    id,
                    usage: ticks.map_or(0.0, |ticks| {
                        ticks.usage(previous.cores.get(&id).copied().flatten())
                    }),
                    mhz: read_value::<u32>(&path).map(|khz| khz / 1000),
                    kind,
                }
            })
            .collect(),
    };
    *previous = current;
    cpu
}

fn cpu_id(text: &str) -> Option<u32> {
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

fn cpu_list(path: &Path) -> Vec<RangeInclusive<u32>> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.trim()
        .split(',')
        .filter_map(|field| {
            let (first, last) = field.split_once('-').unwrap_or((field, field));
            let first = cpu_id(first.trim())?;
            let last = cpu_id(last.trim())?;
            (first <= last).then_some(first..=last)
        })
        .collect()
}
