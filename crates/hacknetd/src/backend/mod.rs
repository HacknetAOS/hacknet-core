//! Compositor abstraction. Hyprland is the production target; sway is the
//! headless CI/dev backend; mock is used by unit tests.

use hacknet_proto::layout::Rect;

#[derive(Debug, Clone, Default)]
pub struct RawWindow {
    pub id: String,
    pub title: String,
    pub class: String,
    pub pid: i32,
    pub workspace: i32,
    pub active: bool,
    pub floating: bool,
    pub sticky: bool,
    pub rect: Rect,
}

#[allow(dead_code)]
pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn list(&self) -> Result<Vec<RawWindow>, String>;
    /// Float + pin + resize + move a window.
    fn place(&self, w: &RawWindow, rect: Rect, sticky: bool) -> Result<(), String>;
    fn tile(&self, id: &str) -> Result<(), String>;
    fn focus(&self, id: &str) -> Result<(), String>;
    fn kill(&self, id: &str, pid: i32) -> Result<(), String>;
    fn to_workspace(&self, id: &str, ws: i32) -> Result<(), String>;
    fn switch_workspace(&self, ws: i32) -> Result<(), String>;
    fn reload(&self) -> Result<(), String>;
    fn native_fullscreen(&self, id: &str, on: bool) -> Result<(), String>;
    fn output_size(&self) -> Option<(i32, i32)> {
        None
    }

    // Virtual desktop management
    fn add_workspace(&self, _id: i32) -> Result<(), String> {
        Ok(())
    }
    fn remove_workspace(&self, _id: i32) -> Result<(), String> {
        Ok(())
    }

    // Fullscreen
    fn set_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.native_fullscreen(id, on)
    }
}

pub mod hypr;
pub mod mock;
pub mod sway;

/// Pick a backend from the environment, preferring the production target.
pub fn detect() -> Box<dyn Backend> {
    match std::env::var("HACKNET_BACKEND").as_deref() {
        Ok("mock") => return Box::new(mock::Mock::default()),
        Ok("sway") => return Box::new(sway::Sway::new()),
        Ok("hyprland") => return Box::new(hypr::Hyprland::new()),
        _ => {}
    }
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        Box::new(hypr::Hyprland::new())
    } else if std::env::var("SWAYSOCK").is_ok() {
        Box::new(sway::Sway::new())
    } else {
        // Daemon still runs (theme/netmap/audio) without a compositor.
        Box::new(mock::Mock::default())
    }
}

pub(crate) fn cmd(program: &str, args: &[String]) -> Result<String, String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} exited {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
