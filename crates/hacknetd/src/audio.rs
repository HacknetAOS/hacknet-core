//! Audio spectrum via `cava` in raw mode. cava handles PipeWire/Pulse/ALSA
//! backend differences for us and is already a distro dependency.

use hacknet_proto::protocol::Event;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::server::Hub;

pub const BARS: usize = 64;

fn config() -> String {
    format!(
        "[general]\nbars = {BARS}\nframerate = 20\nsleep_timer = 2\n\n\
         [output]\nmethod = raw\nraw_target = /dev/stdout\ndata_format = ascii\n\
         ascii_max_range = 100\nbar_delimiter = 59\n\n\
         [smoothing]\nnoise_reduction = 0.77\nmonstercat = 1\n"
    )
}

fn has_audio_sink() -> bool {
    let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") else {
        return false;
    };
    let trimmed = cards.trim();
    if trimmed.is_empty() || trimmed.contains("no soundcards") {
        return false;
    }
    Command::new("wpctl")
        .args(["inspect", "@DEFAULT_AUDIO_SINK@"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn proc_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rfind(')')?.checked_add(2)?;
    let mut fields = stat.get(after_comm..)?.split_whitespace();
    // After ')': state(0), ppid(1), pgrp(2), session(3), tty_nr(4), tpgid(5),
    // flags(6), minflt(7), cminflt(8), majflt(9), cmajflt(10), utime(11), stime(12)
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

/// Spawn cava and forward one band vector per frame. Fails silently: the shell
/// renders an idle spectrum when no frames arrive.
pub fn spawn(hub: Arc<Hub>, dir: std::path::PathBuf) {
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        let conf = dir.join("cava.conf");
        if std::fs::write(&conf, config()).is_err() {
            return;
        }
        loop {
            if !has_audio_sink() {
                std::thread::sleep(std::time::Duration::from_secs(10));
                continue;
            }
            let child = Command::new("cava")
                .arg("-p")
                .arg(&conf)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = child else {
                // cava missing: nothing to do until it appears.
                std::thread::sleep(std::time::Duration::from_secs(30));
                continue;
            };
            let pid = child.id();
            let watchdog_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let watchdog_flag = watchdog_stop.clone();
            std::thread::spawn(move || {
                let mut prev = proc_ticks(pid).unwrap_or(0);
                while !watchdog_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if watchdog_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let Some(cur) = proc_ticks(pid) else { break };
                    if cur.saturating_sub(prev) > 80 {
                        // cava is spinning >40% CPU (e.g. stalled PipeWire clock); terminate it.
                        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
                        break;
                    }
                    prev = cur;
                }
            });
            if let Some(out) = child.stdout.take() {
                let mut last_data: Vec<u8> = Vec::new();
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    let vals: Vec<u8> = line
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.trim().parse::<u8>().ok())
                        .take(BARS)
                        .collect();
                    if vals.is_empty() {
                        continue;
                    }
                    let mut data = vals;
                    data.resize(BARS, 0);
                    if data != last_data {
                        let csv = data
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(",");
                        let _ = std::fs::write(dir.join("latest"), csv);
                        last_data = data.clone();
                        hub.broadcast(&Event::Audio { data });
                    }
                }
            }
            watchdog_stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = child.wait();
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}
