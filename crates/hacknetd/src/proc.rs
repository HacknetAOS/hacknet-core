//! /proc parsing: memory, process tree, CPU and responsive-state heuristics.

use hacknet_proto::window::{Memory, WinState};
use std::collections::HashMap;

const PAGE_KB: u64 = 4;

pub fn memory() -> Memory {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let get = |key: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let total = get("MemTotal:");
    let avail = get("MemAvailable:");
    let used = total.saturating_sub(avail);
    let swap_total = get("SwapTotal:");
    let swap_free = get("SwapFree:");
    Memory {
        total_kb: total,
        used_kb: used,
        pct: if total > 0 {
            (used as f32 / total as f32) * 100.0
        } else {
            0.0
        },
        swap_total_kb: swap_total,
        swap_used_kb: swap_total.saturating_sub(swap_free),
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProcStat {
    pub ppid: i32,
    pub rss_kb: u64,
    pub state: char,
    pub cpu_pct: f32,
}

/// Parses `/proc/<pid>/stat`. The comm field may contain spaces and parens, so
/// split on the *last* ')'.
pub fn read_stat(pid: i32) -> Option<ProcStat> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = text.rfind(')')?;
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // rest[0] = state (field 3), rest[1] = ppid (4), rest[11] = utime (14),
    // rest[12] = stime (15), rest[21] = rss pages (24)
    let state = rest.first()?.chars().next()?;
    let ppid = rest.get(1)?.parse().ok()?;
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    let rss: u64 = rest.get(21)?.parse().ok()?;
    Some(ProcStat {
        ppid,
        rss_kb: rss * PAGE_KB,
        state,
        cpu_pct: (utime + stime) as f32,
    })
}

pub fn ancestors(pid: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut cur = pid;
    for _ in 0..32 {
        let Some(st) = read_stat(cur) else { break };
        if st.ppid <= 1 || st.ppid == cur {
            break;
        }
        out.push(st.ppid);
        cur = st.ppid;
    }
    out
}

#[allow(dead_code)]
pub fn alive(pid: i32) -> bool {
    pid > 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[derive(Default)]
pub struct Sampler {
    prev: HashMap<i32, (f32, std::time::Instant)>,
    hz: f32,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            prev: HashMap::new(),
            hz: 100.0,
        }
    }

    /// Sample pids. CPU is expressed as percent of one core, measured between
    /// consecutive samples.
    pub fn sample(&mut self, pids: &[i32]) -> HashMap<i32, ProcStat> {
        let now = std::time::Instant::now();
        let mut out = HashMap::new();
        for &pid in pids {
            let Some(mut st) = read_stat(pid) else {
                continue;
            };
            if let Some((prev_ticks, prev_time)) = self.prev.get(&pid) {
                let dt = now.duration_since(*prev_time).as_secs_f32();
                if dt > 0.05 {
                    let ticks = (st.cpu_pct - prev_ticks).max(0.0);
                    st.cpu_pct = ticks / self.hz / dt * 100.0;
                } else {
                    st.cpu_pct = 0.0;
                }
            } else {
                st.cpu_pct = 0.0;
            }
            self.prev.insert(pid, (st.cpu_pct, now));
            out.insert(pid, st);
        }
        out
    }

    pub fn state_of(st: Option<&ProcStat>) -> WinState {
        match st {
            None => WinState::Unresponsive,
            // 'T' = stopped: the window is still mapped but the app cannot answer.
            Some(s) if s.state == 'D' || s.state == 'Z' || s.state == 'T' => WinState::Unresponsive,
            Some(s) if s.cpu_pct > 95.0 => WinState::Busy,
            Some(_) => WinState::Normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_is_sane() {
        let m = memory();
        assert!(m.total_kb > 0);
        assert!(m.used_kb <= m.total_kb);
        assert!((0.0..=100.0).contains(&m.pct));
    }

    #[test]
    fn reads_own_process() {
        let pid = std::process::id() as i32;
        let st = read_stat(pid).expect("own stat");
        assert!(st.ppid > 0);
        assert!(st.rss_kb > 0);
        assert!(st.state.is_ascii_alphabetic());
    }

    #[test]
    fn ancestors_terminate() {
        let a = ancestors(std::process::id() as i32);
        assert!(a.len() < 32);
    }

    #[test]
    fn absent_pid_is_unresponsive() {
        assert_eq!(Sampler::state_of(None), WinState::Unresponsive);
        let zombie = ProcStat {
            state: 'Z',
            ..Default::default()
        };
        assert_eq!(Sampler::state_of(Some(&zombie)), WinState::Unresponsive);
        let busy = ProcStat {
            state: 'R',
            cpu_pct: 99.0,
            ..Default::default()
        };
        assert_eq!(Sampler::state_of(Some(&busy)), WinState::Busy);
    }
}
