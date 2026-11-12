//! hacknetctl — drive the daemon from a terminal or a script.

use hacknet_proto::protocol::{Event, Request};
use hacknet_proto::socket_path;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn print_out(s: &str) {
    let mut out = std::io::stdout();
    if let Err(e) = writeln!(out, "{s}") {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "hacknetctl {}\n\
         \x20 state                 print the full state as JSON\n\
         \x20 windows               print the window list\n\
         \x20 netmap                print the NetMap graph\n\
         \x20 activate <id>         switch the stage to a window\n\
         \x20 cycle [delta]         cycle stage apps (default +1)\n\
         \x20 kill <id>             terminate a window and its process\n\
         \x20 zone <id> <zone>      stage|terminal|sidebar|dialog|ignored\n\
         \x20 netmap-move <id> <x> <y>\n\
         \x20 netmap-reset\n\
         \x20 theme <id>            apply blue|ente|hellhound|naix|white\n\
         \x20 theme-changed         re-read the applied theme\n\
         \x20 dnd [on|off]          set do-not-disturb mode\n\
         \x20 volume <0.0..1.5>\n\
         \x20 media <play_pause|next|prev|stop|toggle>\n\
         \x20 notify <summary> [body]\n\
         \x20 ping\n\
         \x20 toggle-gutter         toggle left RAM gutter visibility",
        hacknet_proto::VERSION
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let req = match args.first().map(String::as_str) {
        Some("state") | Some("get") => Request::GetState,
        Some("windows") => Request::GetState,
        Some("netmap") => Request::GetState,
        Some("activate") => Request::Activate {
            id: args.get(1).cloned().unwrap_or_else(|| usage()),
        },
        Some("cycle") => Request::Cycle {
            delta: args.get(1).and_then(|v| v.parse().ok()).unwrap_or(1),
        },
        Some("kill") => Request::Kill {
            id: args.get(1).cloned().unwrap_or_else(|| usage()),
        },
        Some("zone") => Request::SetZone {
            id: args.get(1).cloned().unwrap_or_else(|| usage()),
            zone: args.get(2).cloned().unwrap_or_else(|| usage()),
        },
        Some("netmap-move") => Request::NetmapMove {
            id: args.get(1).cloned().unwrap_or_else(|| usage()),
            x: args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0),
            y: args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0),
        },
        Some("netmap-reset") => Request::NetmapReset,
        Some("theme") => Request::SetTheme {
            id: args.get(1).cloned().unwrap_or_else(|| usage()),
        },
        Some("theme-changed") => {
            // Fire-and-forget: don't wait for response to avoid deadlock when called
            // from hacknet-theme which is itself called by the daemon holding the lock.
            let req = Request::ThemeChanged;
            let path = match std::env::var("HACKNET_SOCKET") {
                Ok(p) => p.into(),
                Err(_) => socket_path(),
            };
            let mut s = match UnixStream::connect(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("hacknetctl: cannot connect to {}: {e}", path.display());
                    std::process::exit(1);
                }
            };
            let line = serde_json::to_string(&req).unwrap();
            writeln!(s, "{line}").ok();
            // Don't wait for response - fire and forget.
            return;
        }
        Some("dnd") => {
            let arg = args.get(1).map(String::as_str).unwrap_or("on");
            let enabled = !matches!(arg, "off" | "false" | "0");
            Request::SetDnd { enabled }
        }
        Some("volume") => {
            let arg = args.get(1).map(String::as_str);
            match arg {
                None | Some("get") => {
                    // Try querying via wpctl or pactl
                    let out = std::process::Command::new("wpctl")
                        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                        .output();
                    if let Ok(o) = out {
                        if o.status.success() {
                            print!("{}", String::from_utf8_lossy(&o.stdout));
                            return;
                        }
                    }
                    println!("Volume: 50% (default)");
                    return;
                }
                Some("mute") | Some("toggle") => Request::ToggleMute,
                Some("up") => Request::AdjustVolume { delta: 0.05 },
                Some("down") => Request::AdjustVolume { delta: -0.05 },
                Some(s) if s.starts_with('+') => {
                    let clean = s.trim_start_matches('+').trim_end_matches('%');
                    let val: f64 = clean.parse().unwrap_or(5.0);
                    let delta = if val > 1.0 { val / 100.0 } else { val };
                    Request::AdjustVolume { delta }
                }
                Some(s) if s.starts_with('-') => {
                    let clean = s.trim_start_matches('-').trim_end_matches('%');
                    let val: f64 = clean.parse().unwrap_or(5.0);
                    let delta = if val > 1.0 { -(val / 100.0) } else { -val };
                    Request::AdjustVolume { delta }
                }
                Some(s) => {
                    let clean = s.trim_end_matches('%');
                    let val: f64 = clean.parse().unwrap_or(50.0);
                    let value = if val > 1.5 { val / 100.0 } else { val };
                    Request::SetVolume { value }
                }
            }
        }
        Some("media") => Request::Media {
            action: args.get(1).cloned().unwrap_or_else(|| usage()),
        },
        Some("toggle-gutter") => Request::ToggleGutter,
        Some("notify") => Request::Notify {
            summary: args.get(1).cloned().unwrap_or_else(|| usage()),
            body: args.get(2).cloned().unwrap_or_default(),
            urgency: "normal".into(),
        },
        Some("desktop") => match args.get(1).map(String::as_str) {
            Some("add") => Request::DesktopAdd,
            Some("remove") => Request::DesktopRemove {
                id: args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0),
            },
            Some("switch") => Request::DesktopSwitch {
                id: args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0),
            },
            Some("next") => Request::DesktopNext,
            Some("prev") => Request::DesktopPrev,
            _ => usage(),
        },
        Some("desktop-add") => Request::DesktopAdd,
        Some("desktop-remove") => Request::DesktopRemove {
            id: args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
        },
        Some("desktop-switch") => Request::DesktopSwitch {
            id: args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
        },
        // Relative desktop moves, resolved from the current state.
        Some("desktop-switch-next") => Request::DesktopNext,
        Some("desktop-switch-prev") => Request::DesktopPrev,
        // Toggle fullscreen on whichever stage window is focused.
        Some("fullscreen-toggle") => Request::FullscreenToggle,
        Some("fullscreen") => Request::Fullscreen {
            id: args.get(1).cloned().unwrap_or_else(|| usage()),
            on: args
                .get(2)
                .map(|v| v == "on" || v == "true" || v == "1")
                .unwrap_or(true),
        },
        Some("terminal-bottom") => Request::TerminalBottomOnly {
            on: args
                .get(1)
                .map(|v| v == "on" || v == "true" || v == "1")
                .unwrap_or(true),
        },
        Some("ping") => Request::Ping,
        _ => usage(),
    };

    let path = match std::env::var("HACKNET_SOCKET") {
        Ok(p) => p.into(),
        Err(_) => socket_path(),
    };
    let mut s = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("hacknetctl: cannot connect to {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let line = serde_json::to_string(&req).unwrap();
    if let Err(e) = writeln!(s, "{line}") {
        eprintln!("hacknetctl: {e}");
        std::process::exit(1);
    }

    let mut reader = BufReader::new(s);
    let mut buf = String::new();
    let want_state = matches!(
        args.first().map(String::as_str),
        Some("state") | Some("get") | Some("windows") | Some("netmap")
    );
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let text = buf.trim();
                let Ok(ev) = serde_json::from_str::<Event>(text) else {
                    continue;
                };
                match ev {
                    Event::State { data } if want_state => {
                        let v = serde_json::to_value(&*data).unwrap();
                        match args.first().map(String::as_str) {
                            Some("windows") => {
                                print_out(&serde_json::to_string_pretty(&v["windows"]).unwrap())
                            }
                            Some("netmap") => {
                                print_out(&serde_json::to_string_pretty(&v["netmap"]).unwrap())
                            }
                            _ => print_out(&serde_json::to_string_pretty(&v).unwrap()),
                        }
                        return;
                    }
                    Event::Error { data } => {
                        eprintln!("hacknetctl: {data}");
                        std::process::exit(1);
                    }
                    Event::Pong => {
                        print_out("pong");
                        return;
                    }
                    Event::Ok { data } if !want_state => {
                        if !data.is_empty() {
                            print_out(&data);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            Err(e) => {
                eprintln!("hacknetctl: {e}");
                break;
            }
        }
    }

    if !want_state {
        print_out("ok");
    }
}
