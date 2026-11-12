//! hacknet-theme — compile `Theme.sys` presets into every surface of the OS.
//!
//! One trigger recolours the shell (via theme.json), the terminal (foot.ini),
//! GTK and Qt apps, the compositor and the prompt. Consumers either watch the
//! generated files or are signalled to reload.

use hacknet_proto::theme::{self, Theme};
use hacknet_proto::{home, state_dir, theme_dir};
use std::path::{Path, PathBuf};
use std::process::Command;

fn usage() {
    eprintln!(
        "hacknet-theme {}\n\
         usage:\n\
         \x20 hacknet-theme list\n\
         \x20 hacknet-theme apply <id>\n\
         \x20 hacknet-theme current\n\
         \x20 hacknet-theme validate <file.toml>\n\
         \x20 hacknet-theme presets-dir",
        hacknet_proto::VERSION
    );
}

fn presets_dir() -> PathBuf {
    if let Ok(p) = std::env::var("HACKNET_THEME_DIR") {
        return p.into();
    }
    // dev checkout: <repo>/theme/presets next to the crate
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../theme/presets");
    if dev.is_dir() {
        return dev.canonicalize().unwrap_or(dev);
    }
    PathBuf::from("/usr/share/hacknet/themes")
}

fn load(id: &str) -> Result<(Theme, PathBuf), String> {
    let dir = presets_dir();
    let path = dir.join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("unknown theme {id:?} ({}): {e}", path.display()))?;
    Ok((theme::parse(&text)?, path))
}

/// Write via a temp file + rename so a failure can never leave a half-written
/// (0-byte) theme file behind, which would silently blank the desktop.
fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("out")
    ));
    std::fs::write(&tmp, contents).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

fn which(cmd: &str) -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':').any(|p| Path::new(p).join(cmd).is_file())
}

fn signal(name: &str) {
    let _ = Command::new("pkill")
        .args(["-USR1", "-x", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn apply(id: &str) -> Result<(), String> {
    let (t, src) = load(id)?;
    let out = theme_dir();
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;

    // Refuse early when the target is not writable (e.g. a system-wide state
    // directory owned by root) instead of writing some files and failing later.
    let probe = out.join(".write-probe");
    std::fs::write(&probe, b"").map_err(|e| format!("{} is not writable: {e}", out.display()))?;
    let _ = std::fs::remove_file(&probe);

    write(&out.join("theme.json"), &t.to_json())?;
    write(&out.join("foot.ini"), &t.foot_ini())?;
    write(&out.join("gtk.css"), &t.gtk_css())?;
    write(&out.join("qt.qss"), &t.qt_qss())?;
    write(&out.join("hypr-colors.lua"), &t.hypr_lua())?;
    write(&out.join("starship.toml"), &t.starship_toml())?;
    write(&out.join("env.sh"), &t.shell_env())?;
    write(&out.join("current"), &format!("{id}\n"))?;
    write(&state_dir().join("theme-applied"), &format!("{id}\n"))?;

    // GTK reads user CSS from these paths; no theme engine required.
    let cfg = home().join(".config");
    write(&cfg.join("gtk-3.0/gtk.css"), &t.gtk_css())?;
    write(&cfg.join("gtk-4.0/gtk.css"), &t.gtk_css())?;

    let gtk_settings =
        "[Settings]\ngtk-theme-name = Adwaita-dark\ngtk-application-prefer-dark-theme = 1\n";
    let _ = write(&cfg.join("gtk-3.0/settings.ini"), gtk_settings);
    let _ = write(&cfg.join("gtk-4.0/settings.ini"), gtk_settings);

    if which("gsettings") {
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.interface",
                "gtk-theme",
                "Adwaita-dark",
            ])
            .output();
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.interface",
                "color-scheme",
                "prefer-dark",
            ])
            .output();
    }

    // foot reloads its config on SIGUSR1; Hyprland on `hyprctl reload`.
    signal("foot");
    if which("hyprctl") && std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        let _ = Command::new("hyprctl").arg("reload").output();
    }
    if which("swaymsg") && std::env::var("SWAYSOCK").is_ok() {
        let _ = Command::new("swaymsg").arg("reload").output();
    }

    // Notify the daemon so connected shells pick the change up immediately.
    let _ = Command::new("hacknetctl").arg("theme-changed").output();

    println!(
        "theme {id} applied from {}\n  written to {}",
        src.display(),
        out.display()
    );
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("list") => {
            let mut ids: Vec<String> = std::fs::read_dir(presets_dir())
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let p = e.path();
                            (p.extension()? == "toml")
                                .then(|| p.file_stem()?.to_str().map(str::to_owned))?
                        })
                        .collect()
                })
                .unwrap_or_default();
            ids.sort();
            for id in ids {
                if let Ok((t, _)) = load(&id) {
                    println!("{:<12} {} — {}", t.meta.id, t.meta.name, t.meta.description);
                }
            }
            Ok(())
        }
        Some("apply") => match args.get(1) {
            Some(id) => apply(id),
            None => {
                usage();
                std::process::exit(2);
            }
        },
        Some("current") => {
            let cur = std::fs::read_to_string(theme_dir().join("current"))
                .map_err(|_| "no theme applied yet".to_string())?;
            print!("{cur}");
            Ok(())
        }
        Some("validate") => match args.get(1) {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                let t = theme::parse(&text)?;
                println!("ok: {} ({})", t.meta.id, t.meta.name);
                Ok(())
            }
            None => {
                usage();
                std::process::exit(2);
            }
        },
        Some("presets-dir") => {
            println!("{}", presets_dir().display());
            Ok(())
        }
        _ => {
            usage();
            std::process::exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("hacknet-theme: {e}");
        std::process::exit(1);
    }
}
