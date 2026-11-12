//! In-memory backend used by unit tests and by `hacknetd` when no compositor is
//! available (theme/audio/netmap still work).

use super::{Backend, RawWindow};
use hacknet_proto::layout::Rect;
use std::sync::Mutex;

#[derive(Default)]
pub struct Mock {
    pub windows: Mutex<Vec<RawWindow>>,
    pub calls: Mutex<Vec<String>>,
    pub workspace: Mutex<i32>,
}

#[allow(dead_code)]
impl Mock {
    pub fn insert(&self, w: RawWindow) {
        self.windows.lock().unwrap().push(w);
    }
    fn record(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Backend for Mock {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn list(&self) -> Result<Vec<RawWindow>, String> {
        Ok(self.windows.lock().unwrap().clone())
    }

    fn place(&self, w: &RawWindow, rect: Rect, sticky: bool) -> Result<(), String> {
        self.record(format!(
            "place {} {} {} {} {} sticky={sticky}",
            w.id, rect.x, rect.y, rect.w, rect.h
        ));
        if let Some(win) = self
            .windows
            .lock()
            .unwrap()
            .iter_mut()
            .find(|x| x.id == w.id)
        {
            win.rect = rect;
            win.floating = true;
            win.sticky = sticky;
        }
        Ok(())
    }

    fn tile(&self, id: &str) -> Result<(), String> {
        self.record(format!("tile {id}"));
        if let Some(w) = self.windows.lock().unwrap().iter_mut().find(|x| x.id == id) {
            w.floating = false;
        }
        Ok(())
    }

    fn focus(&self, id: &str) -> Result<(), String> {
        self.record(format!("focus {id}"));
        Ok(())
    }

    fn kill(&self, id: &str, _pid: i32) -> Result<(), String> {
        self.record(format!("kill {id}"));
        self.windows.lock().unwrap().retain(|w| w.id != id);
        Ok(())
    }

    fn to_workspace(&self, id: &str, ws: i32) -> Result<(), String> {
        self.record(format!("to_workspace {id} {ws}"));
        if let Some(w) = self.windows.lock().unwrap().iter_mut().find(|x| x.id == id) {
            w.workspace = ws;
        }
        Ok(())
    }

    fn switch_workspace(&self, ws: i32) -> Result<(), String> {
        self.record(format!("switch_workspace {ws}"));
        *self.workspace.lock().unwrap() = ws;
        Ok(())
    }

    fn reload(&self) -> Result<(), String> {
        self.record("reload".into());
        Ok(())
    }

    fn native_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.record(format!("fullscreen {id} {on}"));
        Ok(())
    }

    fn output_size(&self) -> Option<(i32, i32)> {
        Some((1920, 1080))
    }

    fn add_workspace(&self, id: i32) -> Result<(), String> {
        self.record(format!("add_workspace {id}"));
        Ok(())
    }

    fn remove_workspace(&self, id: i32) -> Result<(), String> {
        self.record(format!("remove_workspace {id}"));
        Ok(())
    }

    fn set_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
        self.native_fullscreen(id, on)
    }
}
