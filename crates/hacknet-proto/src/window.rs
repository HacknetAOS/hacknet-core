//! Runtime model shared with the QML shell.

use crate::layout::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    #[default]
    Unknown,
    Stage,
    Terminal,
    Sidebar,
    Dialog,
    Ignored,
}

impl Zone {
    pub fn as_str(self) -> &'static str {
        match self {
            Zone::Unknown => "unknown",
            Zone::Stage => "stage",
            Zone::Terminal => "terminal",
            Zone::Sidebar => "sidebar",
            Zone::Dialog => "dialog",
            Zone::Ignored => "ignored",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "stage" => Zone::Stage,
            "terminal" => Zone::Terminal,
            "sidebar" => Zone::Sidebar,
            "dialog" => Zone::Dialog,
            "ignored" => Zone::Ignored,
            "unknown" => Zone::Unknown,
            _ => return None,
        })
    }
    /// Every managed surface appears in the NetMap: stage apps, dialogs, docked
    /// terminals and dock shells each get a node the operator can click.
    pub fn on_netmap(self) -> bool {
        matches!(
            self,
            Zone::Stage | Zone::Dialog | Zone::Terminal | Zone::Sidebar
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WinState {
    #[default]
    Normal,
    Busy,
    Unresponsive,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Window {
    pub id: String,
    pub title: String,
    pub class: String,
    pub pid: i32,
    pub zone: Zone,
    pub workspace: i32,
    pub active: bool,
    pub floating: bool,
    pub rect: Rect,
    pub rss_kb: u64,
    pub cpu_pct: f32,
    pub state: WinState,
    pub deck: i32,
    /// PID of the process that owns this window, when it is a child dialog.
    pub parent_pid: i32,
}

impl Window {
    pub fn label(&self) -> String {
        let t = self.title.trim();
        if t.is_empty() {
            self.class.clone()
        } else {
            t.chars().take(28).collect()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub class: String,
    pub x: i32,
    pub y: i32,
    pub zone: Zone,
    pub active: bool,
    pub state: WinState,
    pub deck: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Netmap {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    pub total_kb: u64,
    pub used_kb: u64,
    pub pct: f32,
    pub swap_total_kb: u64,
    pub swap_used_kb: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub backend: String,
    pub version: String,
    pub theme_id: String,
    pub screen: crate::layout::Screen,
    pub rects: crate::layout::Rects,
    #[serde(default)]
    pub tokens: Option<crate::layout::Tokens>,
    pub windows: Vec<Window>,
    pub netmap: Netmap,
    pub memory: Memory,
    pub dnd: bool,
    pub deck_active: i32,
    #[serde(default)]
    pub gutter_hidden: bool,
    #[serde(default)]
    pub terminal_bottom_only: bool,
    #[serde(default)]
    pub desktop_count: i32,
    #[serde(default)]
    pub current_desktop: i32,
    #[serde(default)]
    pub audio: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_managed_zone_reaches_the_netmap() {
        // Dock terminals and the docked console appear in the NetMap so each
        // one is a clickable node; only Unknown/Ignored are excluded.
        for z in [Zone::Stage, Zone::Dialog, Zone::Terminal, Zone::Sidebar] {
            assert!(z.on_netmap(), "{z:?} must appear in the netmap");
        }
        for z in [Zone::Unknown, Zone::Ignored] {
            assert!(!z.on_netmap(), "{z:?} must be excluded");
        }
    }

    #[test]
    fn zone_strings_roundtrip() {
        for z in [
            Zone::Stage,
            Zone::Terminal,
            Zone::Sidebar,
            Zone::Dialog,
            Zone::Ignored,
            Zone::Unknown,
        ] {
            assert_eq!(Zone::parse(z.as_str()), Some(z));
        }
    }

    #[test]
    fn label_falls_back_to_class() {
        let mut w = Window {
            class: "foot".into(),
            ..Default::default()
        };
        assert_eq!(w.label(), "foot");
        w.title = "user@hacknet: ~".into();
        assert_eq!(w.label(), "user@hacknet: ~");
        w.title = "x".repeat(80);
        assert_eq!(w.label().chars().count(), 28);
    }
}
