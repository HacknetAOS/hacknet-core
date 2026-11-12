//! Line-delimited JSON protocol on `$XDG_RUNTIME_DIR/hacknet/hacknetd.sock`.
//!
//! The QML shell cannot speak D-Bus without a helper, so the daemon's primary
//! transport is a plain Unix socket. Every line is one JSON object.

use crate::window::{Memory, Netmap, StateSnapshot, Window};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Hello {
        client: String,
    },
    GetState,
    Activate {
        id: String,
    },
    /// Cycle the visible stage app forward (+1) or backward (-1).
    Cycle {
        delta: i32,
    },
    Kill {
        id: String,
    },
    SetZone {
        id: String,
        zone: String,
    },
    NetmapMove {
        id: String,
        x: i32,
        y: i32,
    },
    NetmapReset,
    SetTheme {
        id: String,
    },
    ThemeChanged,
    SetDnd {
        enabled: bool,
    },
    Media {
        action: String,
    },
    SetVolume {
        value: f64,
    },
    AdjustVolume {
        delta: f64,
    },
    ToggleMute,
    ToggleGutter,
    Notify {
        summary: String,
        #[serde(default)]
        body: String,
        #[serde(default = "normal")]
        urgency: String,
    },
    Ping,
    Quit,
    // Virtual desktop management
    DesktopAdd,
    DesktopRemove {
        id: i32,
    },
    DesktopSwitch {
        id: i32,
    },
    DesktopNext,
    DesktopPrev,
    // Fullscreen
    Fullscreen {
        id: String,
        on: bool,
    },
    FullscreenToggle,
    // Terminal bottom-only
    TerminalBottomOnly {
        on: bool,
    },
}

fn normal() -> String {
    "normal".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    State { data: Box<StateSnapshot> },
    Windows { data: Vec<Window> },
    Netmap { data: Netmap },
    Theme { data: serde_json::Value },
    Audio { data: Vec<u8> },
    Memory { data: Memory },
    Log { data: String },
    Ok { data: String },
    Error { data: String },
    Pong,
}

impl Event {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| "{\"type\":\"error\",\"data\":\"encode\"}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_roundtrip() {
        for req in [
            Request::GetState,
            Request::Activate { id: "0x1".into() },
            Request::Cycle { delta: 1 },
            Request::SetTheme { id: "naix".into() },
            Request::SetVolume { value: 0.5 },
            Request::AdjustVolume { delta: 0.05 },
            Request::ToggleMute,
            Request::Notify {
                summary: "hi".into(),
                body: String::new(),
                urgency: "normal".into(),
            },
        ] {
            let s = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&s).unwrap();
            assert_eq!(std::mem::discriminant(&req), std::mem::discriminant(&back));
        }
    }

    #[test]
    fn toggle_gutter_roundtrip() {
        let req = Request::ToggleGutter;
        let s = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(std::mem::discriminant(&req), std::mem::discriminant(&back));
    }

    #[test]
    fn wire_shape_is_stable() {
        let line = serde_json::to_string(&Request::SetZone {
            id: "w1".into(),
            zone: "stage".into(),
        })
        .unwrap();
        assert_eq!(line, r#"{"cmd":"set_zone","id":"w1","zone":"stage"}"#);
        let ev = Event::Log { data: "x".into() }.to_line();
        assert_eq!(ev, r#"{"type":"log","data":"x"}"#);
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"rm_rf"}"#).is_err());
    }
}
