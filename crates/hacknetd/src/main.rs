//! hacknetd — places windows in the HacknetOS grid and keeps the shell in sync.

mod audio;
mod backend;
mod proc;
mod server;
mod state;

use hacknet_proto::layout::Layout;
use state::Engine;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn layout_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HACKNET_LAYOUT") {
        return p.into();
    }
    let installed = std::path::PathBuf::from("/usr/share/hacknet/layout.toml");
    if installed.is_file() {
        return installed;
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../design/layout.toml");
    dev.canonicalize().unwrap_or(dev)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut once = false;
    let mut width = std::env::var("HACKNET_WIDTH")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut height = std::env::var("HACKNET_HEIGHT")
        .ok()
        .and_then(|v| v.parse().ok());
    while let Some(a) = args.next() {
        match a.as_str() {
            "--once" => once = true,
            "--width" => width = args.next().and_then(|v| v.parse().ok()),
            "--height" => height = args.next().and_then(|v| v.parse().ok()),
            "--help" | "-h" => {
                println!(
                    "hacknetd {} — backend: auto|sway|hyprland|mock\n\
                     \x20 --once            reconcile once, print state, exit\n\
                     \x20 --width/--height  override the output size\n\
                     \x20 --version         print version",
                    hacknet_proto::VERSION
                );
                return;
            }
            "--version" => {
                println!("{}", hacknet_proto::VERSION);
                return;
            }
            other => eprintln!("hacknetd: ignoring unknown argument {other:?}"),
        }
    }

    let layout = match Layout::from_path(&layout_path()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hacknetd: {e}");
            std::process::exit(1);
        }
    };

    let backend = backend::detect();
    let (w, h) = match (width, height) {
        (Some(w), Some(h)) => (w, h),
        _ => backend.output_size().unwrap_or_else(|| layout.baseline()),
    };

    let mut engine = match Engine::new(layout, backend, w, h) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("hacknetd: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = engine.reconcile() {
        eprintln!("hacknetd: initial reconcile: {e}");
    }

    if std::fs::read_to_string(hacknet_proto::theme_dir().join("current")).is_err() {
        let _ = std::process::Command::new("hacknet-theme")
            .args(["apply", "blue"])
            .output();
        if let Ok(s) = std::fs::read_to_string(hacknet_proto::theme_dir().join("current")) {
            engine.theme_id = s.trim().to_string();
        }
    }

    if once {
        println!(
            "{}",
            serde_json::to_string_pretty(&engine.snapshot()).unwrap_or_default()
        );
        return;
    }

    eprintln!(
        "hacknetd {} backend={} output={}x{} stage={}x{}",
        hacknet_proto::VERSION,
        engine.backend.name(),
        w,
        h,
        engine.compiled.rects.stage.w,
        engine.compiled.rects.stage.h
    );

    let audio_dir = engine.state_dir.join("audio");
    let engine = Arc::new(Mutex::new(engine));
    let hub = Arc::new(server::Hub::new());

    if let Err(e) = server::serve(engine.clone(), hub.clone()) {
        eprintln!("hacknetd: cannot listen: {e}");
        std::process::exit(1);
    }
    audio::spawn(hub.clone(), audio_dir);

    loop {
        std::thread::sleep(Duration::from_millis(500));
        let mut e = engine.lock().unwrap();
        match e.reconcile() {
            Ok(true) => {
                server::Hub::broadcast(
                    &hub,
                    &hacknet_proto::protocol::Event::Windows {
                        data: e.windows.clone(),
                    },
                );
                server::Hub::broadcast(
                    &hub,
                    &hacknet_proto::protocol::Event::Netmap {
                        data: e.netmap.clone(),
                    },
                );
                server::Hub::broadcast(
                    &hub,
                    &hacknet_proto::protocol::Event::Memory {
                        data: e.memory.clone(),
                    },
                );
            }
            Ok(false) => {
                // RAM meter needs a steady 1 Hz-ish heartbeat.
                server::Hub::broadcast(
                    &hub,
                    &hacknet_proto::protocol::Event::Memory {
                        data: e.memory.clone(),
                    },
                );
            }
            Err(err) => {
                server::Hub::broadcast(
                    &hub,
                    &hacknet_proto::protocol::Event::Log {
                        data: format!("reconcile: {err}"),
                    },
                );
            }
        }
    }
}
