//! sway / i3 backend — used for headless integration tests and as a fallback
//! session. Everything goes through `swaymsg`, which is stable and scriptable.

use super::{cmd, Backend, RawWindow};
use hacknet_proto::layout::Rect;
use serde_json::Value;

pub struct Sway;

impl Sway {
    pub fn new() -> Self {
        Self
    }
    fn msg(&self, args: &[&str]) -> Result<String, String> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        cmd("swaymsg", &owned)
    }
    fn select(&self, id: &str, command: &str) -> Result<(), String> {
        self.msg(&[&format!("[con_id={id}]"), command]).map(|_| ())
    }
}

impl Default for Sway {
    fn default() -> Self {
        Self::new()
    }
}

fn walk(node: &Value, workspace: i32, floating: bool, out: &mut Vec<RawWindow>) {
    let ntype = node.get("type").and_then(Value::as_str).unwrap_or("");
    let ws = node
        .get("num")
        .and_then(Value::as_i64)
        .filter(|_| ntype == "workspace")
        .map(|n| n as i32)
        .unwrap_or(workspace);
    let floating = floating || ntype == "floating_con";

    if let Some(pid) = node.get("pid").and_then(Value::as_i64) {
        if pid > 0 {
            let app_id = node
                .get("app_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    node.pointer("/window_properties/class")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            let rect = node.get("rect");
            let g = |k: &str| {
                rect.and_then(|r| r.get(k))
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i32
            };
            out.push(RawWindow {
                id: node
                    .get("id")
                    .and_then(Value::as_i64)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                title: node
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                class: app_id,
                pid: pid as i32,
                workspace: ws,
                active: node
                    .get("focused")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                floating,
                sticky: node.get("sticky").and_then(Value::as_bool).unwrap_or(false),
                rect: Rect::new(g("x"), g("y"), g("width"), g("height")),
            });
        }
    }

    if let Some(kids) = node.get("nodes").and_then(Value::as_array) {
        for k in kids {
            walk(k, ws, floating, out);
        }
    }
    if let Some(kids) = node.get("floating_nodes").and_then(Value::as_array) {
        for k in kids {
            walk(k, ws, true, out);
        }
    }
}

impl Backend for Sway {
    fn name(&self) -> &'static str {
        "sway"
    }

    fn list(&self) -> Result<Vec<RawWindow>, String> {
        let text = self.msg(&["-t", "get_tree", "-r"])?;
        let tree: Value = serde_json::from_str(&text).map_err(|e| format!("sway tree: {e}"))?;
        let mut out = Vec::new();
        walk(&tree, 1, false, &mut out);
        Ok(out)
    }

    fn place(&self, w: &RawWindow, rect: Rect, sticky: bool) -> Result<(), String> {
        let id = &w.id;
        // Each step is independent: a tiled container rejects `resize set` with
        // "Cannot resize tiled container", and aborting there used to skip the
        // move as well — which left GTK apps (Thunar, dialogs) stuck at 0,0 on
        // top of the shell chrome.
        if !w.floating {
            let _ = self.select(id, "floating enable");
        }
        if sticky != w.sticky {
            let _ = self.select(
                id,
                if sticky {
                    "sticky enable"
                } else {
                    "sticky disable"
                },
            );
        }
        let _ = self.select(id, &format!("resize set {} {}", rect.w, rect.h));
        // Always move: this is what actually keeps a window inside its region.
        self.select(id, &format!("move absolute position {} {}", rect.x, rect.y))
    }

    fn tile(&self, id: &str) -> Result<(), String> {
        self.select(id, "floating disable")
    }

    fn focus(&self, id: &str) -> Result<(), String> {
        self.select(id, "focus")
    }

    fn kill(&self, id: &str, pid: i32) -> Result<(), String> {
        self.select(id, "kill")?;
        // Escalate only if THIS window is still open and no sibling window shares the PID.
        std::thread::sleep(std::time::Duration::from_millis(350));
        if pid > 0 {
            let wins = self.list().unwrap_or_default();
            let still_open = wins.iter().any(|w| w.id == id);
            let has_sibling = wins.iter().any(|w| w.id != id && w.pid == pid);
            if still_open && !has_sibling && std::path::Path::new(&format!("/proc/{pid}")).exists()
            {
                let _ = super::cmd("kill", &["-9".into(), pid.to_string()]);
            }
        }
        Ok(())
    }

    fn to_workspace(&self, id: &str, ws: i32) -> Result<(), String> {
        self.select(id, &format!("move container to workspace number {ws}"))
    }

    fn switch_workspace(&self, ws: i32) -> Result<(), String> {
        self.msg(&["workspace", "number", &ws.to_string()])
            .map(|_| ())
    }

    fn reload(&self) -> Result<(), String> {
        self.msg(&["reload"]).map(|_| ())
    }

    fn native_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.select(
            id,
            if on {
                "fullscreen enable"
            } else {
                "fullscreen disable"
            },
        )
    }

    fn output_size(&self) -> Option<(i32, i32)> {
        let text = self.msg(&["-t", "get_outputs", "-r"]).ok()?;
        let arr: Value = serde_json::from_str(&text).ok()?;
        let out = arr
            .as_array()?
            .iter()
            .find(|o| o.get("focused").and_then(Value::as_bool).unwrap_or(false))
            .or_else(|| arr.as_array()?.first())?;
        let r = out.get("rect")?;
        Some((
            r.get("width")?.as_i64()? as i32,
            r.get("height")?.as_i64()? as i32,
        ))
    }

    fn add_workspace(&self, id: i32) -> Result<(), String> {
        self.msg(&["workspace", "number", &id.to_string()])
            .map(|_| ())
    }

    fn remove_workspace(&self, id: i32) -> Result<(), String> {
        self.msg(&["workspace", "number", &id.to_string()])
            .map(|_| ())
    }

    fn set_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.native_fullscreen(id, on)
    }
}
