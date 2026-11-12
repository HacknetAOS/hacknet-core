//! Unix-socket server. One JSON object per line, broadcast to every client.

use hacknet_proto::protocol::{Event, Request};
use hacknet_proto::socket_path;
use hacknet_proto::window::Zone;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use crate::state::Engine;

pub struct Hub {
    clients: Mutex<Vec<UnixStream>>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(Vec::new()),
        }
    }

    pub fn add(&self, s: UnixStream) {
        self.clients.lock().unwrap().push(s);
    }

    pub fn broadcast(&self, ev: &Event) {
        let line = format!("{}\n", ev.to_line());
        let mut clients = self.clients.lock().unwrap();
        clients.retain_mut(|c| c.write_all(line.as_bytes()).and_then(|_| c.flush()).is_ok());
    }

    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.clients.lock().unwrap().len()
    }
}

pub fn broadcast_all(engine: &Engine, hub: &Hub) {
    hub.broadcast(&Event::Windows {
        data: engine.windows.clone(),
    });
    hub.broadcast(&Event::Netmap {
        data: engine.netmap.clone(),
    });
    hub.broadcast(&Event::Memory {
        data: engine.memory.clone(),
    });
    hub.broadcast(&Event::State {
        data: Box::new(engine.snapshot()),
    });
}

pub fn serve(engine: Arc<Mutex<Engine>>, hub: Arc<Hub>) -> std::io::Result<()> {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let engine = engine.clone();
            let hub = hub.clone();
            std::thread::spawn(move || handle(stream, engine, hub));
        }
    });
    Ok(())
}

fn handle(stream: UnixStream, engine: Arc<Mutex<Engine>>, hub: Arc<Hub>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    hub.add(write_half);
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let quit = matches!(req, Request::Quit);
                let mut e = engine.lock().unwrap();
                let reply = apply(&mut e, req);
                match reply {
                    Ok(msg) => {
                        hub.broadcast(&Event::Ok { data: msg });
                        broadcast_all(&e, &hub);
                    }
                    Err(err) => hub.broadcast(&Event::Error { data: err }),
                }
                drop(e);
                if quit {
                    break;
                }
            }
            Err(err) => hub.broadcast(&Event::Error {
                data: format!("bad request: {err}"),
            }),
        }
    }
}

fn apply(e: &mut Engine, req: Request) -> Result<String, String> {
    match req {
        Request::Hello { client } => Ok(format!("hello {client}")),
        Request::GetState => {
            // The state event is broadcast by the caller.
            Ok(String::new())
        }
        Request::Activate { id } => e.activate(&id).map(|_| String::new()),
        Request::Cycle { delta } => e.cycle(delta).map(|_| String::new()),
        Request::Kill { id } => e.kill(&id).map(|_| String::new()),
        Request::SetZone { id, zone } => {
            let z = Zone::parse(&zone).ok_or_else(|| format!("bad zone {zone}"))?;
            e.set_zone(&id, z).map(|_| String::new())
        }
        Request::NetmapMove { id, x, y } => {
            e.netmap_move(&id, x, y);
            Ok(String::new())
        }
        Request::NetmapReset => {
            e.netmap_reset();
            Ok(String::new())
        }
        Request::SetTheme { id } => e.set_theme(&id).map(|_| format!("theme {id}")),
        Request::ThemeChanged => {
            if let Ok(s) = std::fs::read_to_string(hacknet_proto::theme_dir().join("current")) {
                e.theme_id = s.trim().to_string();
            }
            e.revision += 1;
            Ok(format!("theme {}", e.theme_id))
        }
        Request::SetDnd { enabled } => {
            e.dnd = enabled;
            e.revision += 1;
            Ok(String::new())
        }
        Request::Media { action } => e.media(&action).map(|_| String::new()),
        Request::SetVolume { value } => e.set_volume(value).map(|_| String::new()),
        Request::AdjustVolume { delta } => e.adjust_volume(delta).map(|_| String::new()),
        Request::ToggleMute => e.toggle_mute().map(|_| String::new()),
        Request::ToggleGutter => {
            e.toggle_gutter();
            e.reconcile()?;
            Ok("gutter toggled".into())
        }
        Request::Notify {
            summary,
            body,
            urgency,
        } => {
            let mut cmd = std::process::Command::new("notify-send");
            cmd.arg("-u").arg(&urgency).arg(summary);
            if !body.is_empty() {
                cmd.arg(body);
            }
            cmd.output()
                .map_err(|e| format!("notify-send: {e}"))
                .map(|_| String::new())
        }
        Request::Ping => Ok("pong".into()),
        Request::Quit => Ok("bye".into()),
        // Virtual desktop management
        Request::DesktopAdd => e.add_desktop().map(|_| String::new()),
        Request::DesktopRemove { id } => e.remove_desktop(id).map(|_| String::new()),
        Request::DesktopSwitch { id } => e.switch_desktop(id).map(|_| String::new()),
        // Fullscreen
        Request::Fullscreen { id, on } => e.set_fullscreen(&id, on).map(|_| String::new()),
        Request::DesktopNext => e.step_desktop(1).map(|_| String::new()),
        Request::DesktopPrev => e.step_desktop(-1).map(|_| String::new()),
        Request::FullscreenToggle => e.toggle_fullscreen_active().map(|_| String::new()),
        // Terminal bottom-only
        Request::TerminalBottomOnly { .. } => {
            e.toggle_terminal_bottom_only()?;
            e.reconcile()?;
            Ok("terminal bottom toggled".into())
        }
    }
}
