//! Hyprland backend — the production compositor.

use super::{cmd, Backend, RawWindow};
use hacknet_proto::layout::Rect;
use serde_json::Value;

pub struct Hyprland;

impl Hyprland {
    pub fn new() -> Self {
        Self
    }

    fn dispatch(&self, args: &[&str]) -> Result<String, String> {
        let mut owned: Vec<String> = vec!["dispatch".into()];
        owned.extend(args.iter().map(|s| s.to_string()));
        cmd("hyprctl", &owned)
    }

    fn dispatch_window(&self, cmd: &str, id: &str) -> Result<(), String> {
        let addr = if id.starts_with("0x") {
            id.to_string()
        } else {
            format!("0x{id}")
        };
        // Hyprland's argument parser is whitespace sensitive: no spaces.
        self.dispatch(&[cmd, &format!("address:{addr}")])
            .map(|_| ())
    }
}

impl Default for Hyprland {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Hyprland {
    fn name(&self) -> &'static str {
        "hyprland"
    }

    fn list(&self) -> Result<Vec<RawWindow>, String> {
        let text = cmd("hyprctl", &["clients".into(), "-j".into()])?;
        let arr: Vec<Value> =
            serde_json::from_str(&text).map_err(|e| format!("hyprctl clients: {e}"))?;
        let mut out = Vec::new();
        for c in arr {
            let g = |k: &str| c.get(k).and_then(Value::as_i64).unwrap_or(0) as i32;
            let at = c.get("at").and_then(Value::as_array);
            let size = c.get("size").and_then(Value::as_array);
            let xy = |a: Option<&Vec<Value>>, i: usize| {
                a.and_then(|v| v.get(i))
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32
            };
            out.push(RawWindow {
                id: c
                    .get("address")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                title: c
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                class: c
                    .get("class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                pid: g("pid"),
                workspace: c
                    .pointer("/workspace/id")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32,
                active: c
                    .get("focusHistoryID")
                    .and_then(Value::as_i64)
                    .map(|v| v == 0)
                    .unwrap_or(false),
                floating: c.get("floating").and_then(Value::as_bool).unwrap_or(false),
                sticky: c.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                rect: Rect::new(xy(at, 0), xy(at, 1), xy(size, 0), xy(size, 1)),
            });
        }
        Ok(out)
    }

    fn place(&self, w: &RawWindow, rect: Rect, sticky: bool) -> Result<(), String> {
        if !w.floating {
            self.dispatch_window("setfloating", &w.id)?;
        }
        if sticky != w.sticky {
            self.dispatch_window("pin", &w.id)?;
        }
        self.dispatch(&[
            "resizewindowpixel",
            &format!("exact {},address:{}", rect_size(rect), w.id),
        ])?;
        self.dispatch(&[
            "movewindowpixel",
            &format!("exact {},address:{}", rect_pos(rect), w.id),
        ])?;
        Ok(())
    }

    fn tile(&self, id: &str) -> Result<(), String> {
        self.dispatch_window("settiled", id)
    }

    fn focus(&self, id: &str) -> Result<(), String> {
        self.dispatch_window("focuswindow", id)?;
        let addr = if id.starts_with("0x") {
            id.to_string()
        } else {
            format!("0x{id}")
        };
        let _ = self.dispatch(&["alterzorder", &format!("top,address:{addr}")]);
        let _ = self.dispatch(&["bringactivetotop", ""]);
        Ok(())
    }

    fn kill(&self, id: &str, pid: i32) -> Result<(), String> {
        let _ = self.dispatch_window("closewindow", id);
        std::thread::sleep(std::time::Duration::from_millis(350));
        if pid > 0 {
            let wins = self.list().unwrap_or_default();
            let still_open = wins.iter().any(|w| w.id == id);
            let has_sibling = wins.iter().any(|w| w.id != id && w.pid == pid);
            if still_open && !has_sibling && std::path::Path::new(&format!("/proc/{pid}")).exists()
            {
                let _ = cmd("kill", &["-9".into(), pid.to_string()]);
            }
        }
        Ok(())
    }

    fn to_workspace(&self, id: &str, ws: i32) -> Result<(), String> {
        self.dispatch(&["movetoworkspacesilent", &format!("{ws},address:{id}")])
            .map(|_| ())
    }

    fn switch_workspace(&self, ws: i32) -> Result<(), String> {
        self.dispatch(&["workspace", &ws.to_string()]).map(|_| ())
    }

    fn reload(&self) -> Result<(), String> {
        cmd("hyprctl", &["reload".into()]).map(|_| ())
    }

    fn native_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.dispatch(&[
            "fullscreen",
            if on { "1" } else { "0" },
            &format!("address:0x{}", id.trim_start_matches("0x")),
        ])
        .map(|_| ())
    }

    fn output_size(&self) -> Option<(i32, i32)> {
        self.monitors()
    }

    fn add_workspace(&self, id: i32) -> Result<(), String> {
        self.dispatch(&["workspace", &id.to_string()]).map(|_| ())
    }

    fn remove_workspace(&self, id: i32) -> Result<(), String> {
        self.dispatch(&["workspace", &id.to_string()]).map(|_| ())
    }

    fn set_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.native_fullscreen(id, on)
    }
}

impl Hyprland {
    fn monitors(&self) -> Option<(i32, i32)> {
        let text = cmd("hyprctl", &["monitors".into(), "-j".into()]).ok()?;
        let arr: Vec<Value> = serde_json::from_str(&text).ok()?;
        let m = arr
            .iter()
            .find(|m| m.get("focused").and_then(Value::as_bool).unwrap_or(false))
            .or_else(|| arr.first())?;
        Some((
            m.get("width")?.as_i64()? as i32,
            m.get("height")?.as_i64()? as i32,
        ))
    }
}

fn rect_size(r: Rect) -> String {
    format!("{} {}", r.w, r.h)
}
fn rect_pos(r: Rect) -> String {
    format!("{} {}", r.x, r.y)
}
