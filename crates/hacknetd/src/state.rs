//! The placement + NetMap engine. This is the heart of the "single-app stage".

use crate::backend::{Backend, RawWindow};
use crate::proc::{self, Sampler};
use hacknet_proto::layout::{Compiled, Layout, Rect};
#[allow(unused_imports)]
use hacknet_proto::window::{Edge, Memory, Netmap, Node, StateSnapshot, WinState, Window, Zone};
use hacknet_proto::{state_dir, theme_dir};

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Config {
    /// app_id / class values that belong in the docked terminal column.
    pub terminal: Vec<String>,
    /// app_id prefix for sidebar slots (e.g. `hacknet-dock-1`).
    pub dock_prefix: String,
    /// never displayed or tracked.
    pub ignored: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            terminal: vec!["hacknet-term".into(), "hacknet-terminal".into()],
            dock_prefix: "hacknet-dock".into(),
            ignored: vec!["hacknet-osd".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Persist {
    positions: HashMap<String, (i32, i32)>,
    decks: HashMap<String, i32>,
}

#[allow(dead_code)]
enum Action {
    Place {
        w: RawWindow,
        rect: Rect,
        sticky: bool,
    },
    Tile {
        id: String,
    },
    ToWorkspace {
        id: String,
        ws: i32,
    },
    Switch {
        ws: i32,
    },
    Focus {
        id: String,
    },
}

pub struct Engine {
    pub backend: Box<dyn Backend>,
    pub layout: Layout,
    pub compiled: Compiled,
    pub cfg: Config,
    pub windows: Vec<Window>,
    pub netmap: Netmap,
    pub deck_active: i32,
    // Virtual desktops
    pub desktop_count: i32,
    pub current_desktop: i32,
    // Fullscreen state
    pub fullscreen_window: Option<String>,
    // Docked console occupies only the bottom-right quadrant, so DISPLAY gets
    // the full width above it.
    pub terminal_bottom_only: bool,
    pub positions: HashMap<String, (i32, i32)>,
    pub decks: HashMap<String, i32>,
    pub placed: HashMap<String, Rect>,
    pub sampler: Sampler,
    pub memory: Memory,
    pub dnd: bool,
    pub gutter_hidden: bool,
    pub theme_id: String,
    pub revision: u64,
    pub state_dir: std::path::PathBuf,
    signature: String,
    next_deck: i32,
    overrides: HashMap<String, Zone>,
}

impl Engine {
    pub fn new(
        layout: Layout,
        backend: Box<dyn Backend>,
        width: i32,
        height: i32,
    ) -> Result<Self, String> {
        Self::new_in(layout, backend, width, height, state_dir())
    }

    pub fn new_in(
        layout: Layout,
        backend: Box<dyn Backend>,
        width: i32,
        height: i32,
        state_dir: std::path::PathBuf,
    ) -> Result<Self, String> {
        let compiled = layout.compile(width, height)?;
        let mut e = Self {
            backend,
            layout,
            compiled,
            cfg: Config::default(),
            windows: Vec::new(),
            netmap: Netmap::default(),
            deck_active: 1,
            desktop_count: 1,
            current_desktop: 1,
            fullscreen_window: None,
            terminal_bottom_only: false,
            positions: HashMap::new(),
            decks: HashMap::new(),
            placed: HashMap::new(),
            sampler: Sampler::new(),
            memory: proc::memory(),
            dnd: false,
            gutter_hidden: false,
            theme_id: std::fs::read_to_string(theme_dir().join("current"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "blue".into()),
            revision: 0,
            state_dir,
            signature: String::new(),
            next_deck: 2,
            overrides: HashMap::new(),
        };
        e.load();
        Ok(e)
    }

    fn persist_path(&self) -> std::path::PathBuf {
        self.state_dir.join("netmap.json")
    }

    fn load(&mut self) {
        if let Ok(text) = std::fs::read_to_string(self.persist_path()) {
            if let Ok(p) = serde_json::from_str::<Persist>(&text) {
                self.positions = p.positions;
                self.decks = p.decks;
                // Workspace 1 is strictly reserved for the Home / Connected screen.
                // Any persisted deck < 2 must be remapped to at least 2.
                for d in self.decks.values_mut() {
                    if *d < 2 {
                        *d = 2;
                    }
                }
            }
        }
        self.next_deck = self.decks.values().copied().max().unwrap_or(1).max(1) + 1;
        self.deck_active = 1;
    }

    fn save(&self) {
        let p = Persist {
            positions: self.positions.clone(),
            decks: self.decks.clone(),
        };
        if let Some(parent) = self.persist_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&p) {
            let _ = std::fs::write(self.persist_path(), text);
        }
    }

    /// Rect for dock slot `index` of `count` windows in the left sidebar
    /// column: below the RAM block, above the visualiser. Every dock terminal
    /// gets its own slot, so opening a new one never stacks it invisibly
    /// behind the previous.
    pub fn sidebar_slot_rect(&self, index: usize, count: usize) -> Rect {
        let s = self.compiled.rects.sidebar;
        let v = self.compiled.rects.visualizer;
        let t = &self.compiled.tokens;
        let top = s.y + t.ram + t.splitter;
        let bottom = (v.y - t.visualizer_padding).max(top + 24);
        let available = (bottom - top).max(24);
        let n = count.max(1) as i32;
        // Never squeeze a dock terminal below a usable height; the launcher
        // caps how many dock slots exist so this cannot overflow the column.
        let slot_h = (available / n).max(120).min(available);
        let y = (top + (index as i32) * slot_h).min(bottom - slot_h.min(available));
        Rect::new(s.x + t.gap, y, (s.w - 2 * t.gap).max(1), slot_h)
    }

    fn dialog_rect(&self, w: &RawWindow) -> Rect {
        let st = self.compiled.rects.stage_app;
        let max_w = (st.w - 16).max(320);
        let max_h = (st.h - 16).max(200);
        let width = if w.rect.w > 0 {
            w.rect.w.clamp(320, max_w)
        } else {
            max_w
        };
        let height = if w.rect.h > 0 {
            w.rect.h.clamp(200, max_h)
        } else {
            max_h
        };
        Rect::new(
            st.x + (st.w - width) / 2,
            st.y + (st.h - height) / 2,
            width,
            height,
        )
    }

    fn classify(&self, w: &RawWindow, known: &[Window]) -> Zone {
        let class = w.class.to_lowercase();
        let title = w.title.to_lowercase();
        if self
            .cfg
            .ignored
            .iter()
            .any(|i| !i.is_empty() && class.contains(&i.to_lowercase()))
        {
            return Zone::Ignored;
        }
        if self
            .cfg
            .terminal
            .iter()
            .any(|t| class.eq_ignore_ascii_case(t))
        {
            return Zone::Terminal;
        }
        if class.starts_with(&self.cfg.dock_prefix) {
            return Zone::Sidebar;
        }
        if let Some(z) = self.overrides.get(&w.id) {
            return *z;
        }

        // Dialogs, portal file choosers, authentication prompters, and floating helper popups
        // belong in Zone::Dialog so they are placed and constrained inside the DISPLAY section.
        let is_dialog_class = class.contains("portal")
            || class.contains("prompter")
            || class.contains("polkit")
            || class.contains("pinentry");

        let is_dialog_title = title.starts_with("open ")
            || title.starts_with("save ")
            || title.starts_with("select ")
            || title.starts_with("choose ")
            || title.contains("file chooser")
            || title.contains("file upload")
            || title == "open"
            || title == "save"
            || title.contains("preferences")
            || title.contains("properties")
            || title.contains("settings")
            || title.starts_with("confirm");

        if is_dialog_class || is_dialog_title {
            return Zone::Dialog;
        }

        let is_main_stage_app = class == "thunar"
            || class.contains("code")
            || class.contains("browser")
            || class.contains("firefox")
            || class.contains("chromium");

        // A second window from an app that already owns the stage, or a smaller toplevel,
        // is a dialog only if it is genuinely a small auxiliary window and not a main stage app.
        let st = self.compiled.rects.stage_app;
        if !is_main_stage_app {
            if let Some(_parent) = known.iter().find(|x| {
                (x.pid == w.pid || (!x.class.is_empty() && x.class == w.class))
                    && x.zone == Zone::Stage
            }) {
                let small = (w.rect.w > 0 && w.rect.w < st.w * 3 / 4)
                    || (w.rect.h > 0 && w.rect.h < st.h * 3 / 4);
                if small {
                    return Zone::Dialog;
                }
            }
        }
        Zone::Stage
    }

    pub fn desktop_of(&self, deck: i32) -> i32 {
        if deck < 100 {
            1
        } else {
            ((deck - 1) / 100) + 1
        }
    }

    pub fn home_deck_for(&self, desktop: i32) -> i32 {
        if desktop <= 1 {
            1
        } else {
            (desktop - 1) * 100 + 1
        }
    }

    fn deck_for(&mut self, id: &str, class: &str, used_decks: &[i32]) -> i32 {
        let desk = self.current_desktop.max(1);
        let base = if desk == 1 { 2 } else { (desk - 1) * 100 + 2 };
        let max_deck = base + 90;

        let id_key = if desk == 1 {
            format!("id:{id}")
        } else {
            format!("{desk}:id:{id}")
        };
        if let Some(d) = self.decks.get(&id_key).copied() {
            if d >= base && d < max_deck && !used_decks.contains(&d) {
                return d;
            }
        }

        let class_key = if desk == 1 {
            class.to_string()
        } else {
            format!("{desk}:{class}")
        };
        if let Some(d) = self.decks.get(&class_key).copied() {
            if d >= base && d < max_deck && !used_decks.contains(&d) {
                self.decks.insert(id_key, d);
                return d;
            }
        }

        let mut d = base;
        while d < max_deck {
            if !used_decks.contains(&d) {
                break;
            }
            d += 1;
        }
        self.decks.insert(class_key, d);
        self.decks.insert(id_key, d);
        d
    }

    /// Poll the compositor, update the model and enforce the grid.
    pub fn reconcile(&mut self) -> Result<bool, String> {
        // The output can change at runtime (resolution switch, monitor hotplug).
        // Recompile the grid before placing anything so the shell (which reads
        // these rects) and the windows always agree.
        if let Some((w, h)) = self.backend.output_size() {
            let scr = &self.compiled.screen;
            if w > 0 && h > 0 && (w != scr.w || h != scr.h) {
                if let Ok(c) = self.layout.compile(w, h) {
                    self.compiled = c;
                    self.apply_gutter();
                    self.placed.clear();
                }
            }
        }

        let raws = self.backend.list()?;
        let old: HashMap<String, Window> =
            self.windows.drain(..).map(|w| (w.id.clone(), w)).collect();
        let existing: HashSet<String> = old.keys().cloned().collect();
        let pids: Vec<i32> = raws.iter().map(|r| r.pid).collect();
        let stats = self.sampler.sample(&pids);

        let mut fresh_zones: HashMap<String, Zone> = HashMap::new();
        for r in &raws {
            if existing.contains(&r.id) {
                continue;
            }
            let known: Vec<Window> = old.values().cloned().collect();
            fresh_zones.insert(r.id.clone(), self.classify(r, &known));
        }

        let mut actions: Vec<Action> = Vec::new();
        let mut windows: Vec<Window> = Vec::new();
        let mut new_stage: Option<(String, i32)> = None;

        // Stable slot order for sidebar (dock) windows: sorted by id so slots
        // don't reshuffle every reconcile.
        let mut sidebar_order: Vec<String> = Vec::new();
        for r in &raws {
            if r.class.is_empty() && r.title.is_empty() {
                continue;
            }
            let zone = old
                .get(&r.id)
                .map(|p| p.zone)
                .or_else(|| fresh_zones.get(&r.id).copied())
                .unwrap_or(Zone::Stage);
            if zone == Zone::Sidebar {
                sidebar_order.push(r.id.clone());
            }
        }
        sidebar_order.sort();
        let sidebar_count = sidebar_order.len();

        for r in raws {
            if r.class.is_empty() && r.title.is_empty() {
                continue;
            }
            let prev = old.get(&r.id);
            let zone = prev
                .map(|p| p.zone)
                .or_else(|| fresh_zones.get(&r.id).copied())
                .unwrap_or(Zone::Stage);
            if zone == Zone::Ignored {
                continue;
            }

            let deck = match zone {
                Zone::Stage => prev.map(|p| p.deck).filter(|d| *d > 0).unwrap_or_else(|| {
                    let used: Vec<i32> = old
                        .values()
                        .chain(windows.iter())
                        .filter(|w| {
                            w.id != r.id && (w.zone == Zone::Stage || w.zone == Zone::Dialog)
                        })
                        .map(|w| w.deck)
                        .collect();
                    self.deck_for(&r.id, &r.class, &used)
                }),
                Zone::Dialog => prev
                    .map(|p| p.deck)
                    .filter(|d| *d > 0)
                    .or_else(|| {
                        old.values()
                            .find(|p| {
                                (p.pid == r.pid || (!p.class.is_empty() && p.class == r.class))
                                    && p.zone == Zone::Stage
                            })
                            .map(|p| p.deck)
                    })
                    .unwrap_or(self.deck_active),
                _ => 0,
            };

            if zone == Zone::Stage && prev.is_none() {
                new_stage = Some((r.id.clone(), deck));
            }

            let stat = stats.get(&r.pid);
            let state = Sampler::state_of(stat);

            // Enforce placement.
            let mut eff_rect = r.rect;
            match zone {
                Zone::Terminal => {
                    eff_rect = self.compiled.rects.terminal_app;
                    actions.push(Action::Place {
                        w: r.clone(),
                        rect: eff_rect,
                        sticky: true,
                    });
                }
                Zone::Sidebar => {
                    let idx = sidebar_order.iter().position(|x| x == &r.id).unwrap_or(0);
                    eff_rect = self.sidebar_slot_rect(idx, sidebar_count);
                    if self.gutter_hidden {
                        actions.push(Action::Place {
                            w: r.clone(),
                            rect: eff_rect,
                            sticky: false,
                        });
                        if r.workspace != 99 {
                            actions.push(Action::ToWorkspace {
                                id: r.id.clone(),
                                ws: 99,
                            });
                        }
                    } else {
                        if r.workspace == 99 {
                            actions.push(Action::ToWorkspace {
                                id: r.id.clone(),
                                ws: self.deck_active.max(1),
                            });
                        }
                        actions.push(Action::Place {
                            w: r.clone(),
                            rect: eff_rect,
                            sticky: true,
                        });
                    }
                }
                Zone::Dialog => {
                    let rect = self.dialog_rect(&r);
                    eff_rect = rect;
                    actions.push(Action::Place {
                        w: r.clone(),
                        rect,
                        sticky: false,
                    });
                    if r.workspace != deck {
                        actions.push(Action::ToWorkspace {
                            id: r.id.clone(),
                            ws: deck,
                        });
                    }
                }
                Zone::Stage => {
                    // Place inside the viewport below the caption and window
                    // title strip, never over the shell chrome.
                    eff_rect = self.compiled.rects.stage_app;
                    actions.push(Action::Place {
                        w: r.clone(),
                        rect: eff_rect,
                        sticky: false,
                    });
                    if r.workspace != deck {
                        actions.push(Action::ToWorkspace {
                            id: r.id.clone(),
                            ws: deck,
                        });
                    }
                }
                _ => {}
            }

            windows.push(Window {
                id: r.id.clone(),
                title: r.title.clone(),
                class: r.class.clone(),
                pid: r.pid,
                zone,
                workspace: if deck > 0 { deck } else { r.workspace },
                active: r.active,
                floating: r.floating,
                rect: eff_rect,
                rss_kb: stat.map(|s| s.rss_kb).unwrap_or(0),
                cpu_pct: stat.map(|s| s.cpu_pct).unwrap_or(0.0),
                state,
                deck,
                parent_pid: stat.map(|s| s.ppid).unwrap_or(0),
            });
        }

        // A brand new stage app takes over the deck (deck swapping elsewhere is
        // explicit: NetMap click or keybind).
        let had_stage_on_active = old.values().any(|w| {
            (w.zone == Zone::Stage || w.zone == Zone::Dialog) && w.deck == self.deck_active
        });
        let has_stage_on_active = windows.iter().any(|w| {
            (w.zone == Zone::Stage || w.zone == Zone::Dialog) && w.deck == self.deck_active
        });
        if let Some((id, deck)) = new_stage {
            actions.push(Action::Switch { ws: deck });
            actions.push(Action::Focus { id: id.clone() });
            self.deck_active = deck;
        } else if self.deck_active > 1 && had_stage_on_active && !has_stage_on_active {
            let cur_desk = self.desktop_of(self.deck_active);
            let home_deck = self.home_deck_for(cur_desk);
            if let Some(next) = windows
                .iter()
                .find(|w| w.zone == Zone::Stage && self.desktop_of(w.deck) == cur_desk && w.active)
                .or_else(|| {
                    windows
                        .iter()
                        .rev()
                        .find(|w| w.zone == Zone::Stage && self.desktop_of(w.deck) == cur_desk)
                })
            {
                self.deck_active = next.deck;
                actions.push(Action::Switch { ws: next.deck });
                actions.push(Action::Focus {
                    id: next.id.clone(),
                });
            } else {
                self.deck_active = home_deck;
                actions.push(Action::Switch { ws: home_deck });
            }
        }

        self.windows = windows;
        self.memory = proc::memory();

        for a in actions {
            let _ = match a {
                Action::Place { w, rect, sticky } => {
                    let prev = self.placed.get(&w.id).copied();
                    // Re-assert when the client (or compositor decoration) has
                    // drifted from the grid; one-shot placement is not enough
                    // because clients can request a size after mapping.
                    let drift = (w.rect.x - rect.x).abs() > 2
                        || (w.rect.y - rect.y).abs() > 2
                        || (w.rect.w - rect.w).abs() > 2
                        || (w.rect.h - rect.h).abs() > 2;
                    if prev != Some(rect) || !w.floating || w.sticky != sticky || drift {
                        self.placed.insert(w.id.clone(), rect);
                        self.backend.place(&w, rect, sticky)
                    } else {
                        Ok(())
                    }
                }
                Action::Tile { id } => {
                    self.placed.remove(&id);
                    self.backend.tile(&id)
                }
                Action::ToWorkspace { id, ws } => self.backend.to_workspace(&id, ws),
                Action::Switch { ws } => self.backend.switch_workspace(ws),
                Action::Focus { id } => self.backend.focus(&id),
            };
        }

        self.rebuild_netmap();
        let sig = serde_json::to_string(&(&self.windows, &self.netmap)).unwrap_or_default();
        let changed = sig != self.signature;
        if changed {
            self.signature = sig;
            self.revision += 1;
        }
        self.save();
        Ok(changed)
    }

    fn rebuild_netmap(&mut self) {
        let rect = self.compiled.rects.netmap;
        // Filter by current desktop: sticky windows (Terminal, Sidebar) or windows on current desktop
        let mut nodes_src: Vec<&Window> = self
            .windows
            .iter()
            .filter(|w| {
                w.zone.on_netmap()
                    && (w.zone == Zone::Terminal
                        || w.zone == Zone::Sidebar
                        || self.desktop_of(w.deck) == self.current_desktop)
            })
            .collect();
        // Stable order: sort by numeric window id. The window list itself is
        // focus-ordered (the active window moves within it), and an unstable
        // order made netmap nodes reshuffle on every focus change.
        nodes_src.sort_by_key(|w| {
            if let Some(hex) = w.id.strip_prefix("0x") {
                i64::from_str_radix(hex, 16).unwrap_or(i64::MAX)
            } else {
                w.id.parse::<i64>().unwrap_or(i64::MAX)
            }
        });

        let count = nodes_src.len().max(1);
        let rows = if rect.h >= 100 { 2 } else { 1 };
        let cols = ((count as f32) / (rows as f32)).ceil().max(3.0) as i32;
        let cell_w = (rect.w - 80).max(120) / cols.max(1);
        let cell_h = (rect.h - 40).max(40) / rows;

        let mut nodes = Vec::new();
        let mut term_idx = 1;
        for (i, w) in nodes_src.iter().enumerate() {
            // Position cache keyed by WINDOW ID, not class: windows sharing a
            // class (two stage terminals, two browsers) must each keep their
            // own cell — a class-keyed cache made them share one and overlap.
            let key = w.id.clone();
            let col = i as i32 % cols;
            let row = (i as i32 / cols) % rows;
            // Deterministic hash of w.id for organic scatter
            let mut h: u32 = 0x811c9dc5;
            for b in w.id.bytes() {
                h ^= b as u32;
                h = h.wrapping_mul(0x01000193);
            }
            let jitter_x = ((h % 100) as i32 - 50) * cell_w / 300;
            let jitter_y = (((h / 100) % 100) as i32 - 50) * cell_h / 300;
            let def_x = 40 + col * cell_w + cell_w / 2 + jitter_x;
            let def_y = 24 + row * cell_h + cell_h / 2 + jitter_y;
            let (x, y) = *self.positions.get(&key).unwrap_or(&(def_x, def_y));
            let label = if w.zone == Zone::Terminal {
                let l = format!("terminal {term_idx}");
                term_idx += 1;
                l
            } else {
                w.label()
            };
            nodes.push(Node {
                id: w.id.clone(),
                label,
                class: w.class.clone(),
                x: x.clamp(24, rect.w - 24),
                y: y.clamp(24, rect.h - 24),
                zone: w.zone,
                active: w.active,
                state: w.state,
                deck: w.deck,
            });
        }

        let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

        // Parent/child links: prefer the process tree, fall back to "same pid".
        let mut edges = Vec::new();
        for w in &self.windows {
            if !w.zone.on_netmap()
                || (w.zone != Zone::Terminal
                    && w.zone != Zone::Sidebar
                    && self.desktop_of(w.deck) != self.current_desktop)
            {
                continue;
            }
            let mut linked = false;
            for anc in proc::ancestors(w.pid) {
                if let Some(parent) = self.windows.iter().find(|p| {
                    p.pid == anc
                        && p.zone.on_netmap()
                        && p.id != w.id
                        && node_ids.contains(p.id.as_str())
                }) {
                    edges.push(Edge {
                        from: parent.id.clone(),
                        to: w.id.clone(),
                    });
                    linked = true;
                    break;
                }
            }
            if !linked && w.zone == Zone::Dialog {
                if let Some(parent) = self.windows.iter().find(|p| {
                    p.pid == w.pid && p.zone == Zone::Stage && node_ids.contains(p.id.as_str())
                }) {
                    edges.push(Edge {
                        from: parent.id.clone(),
                        to: w.id.clone(),
                    });
                    linked = true;
                }
            }
            if !linked {
                // Keybind-spawned terminals (dock slots, the docked console) have
                // no mapped ancestor — link them to the operator's own machine,
                // which is how the game draws locally opened terminals.
                edges.push(Edge {
                    from: "home".into(),
                    to: w.id.clone(),
                });
            }
        }
        self.netmap = Netmap { nodes, edges };

        // Drop cached positions of windows that no longer exist, so a new
        // window reusing a slot class starts from the default grid.
        let live: std::collections::HashSet<&str> =
            self.windows.iter().map(|w| w.id.as_str()).collect();
        self.positions.retain(|k, _| live.contains(k.as_str()));
    }

    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            backend: self.backend.name().into(),
            version: hacknet_proto::VERSION.into(),
            theme_id: self.theme_id.clone(),
            screen: self.compiled.screen.clone(),
            rects: self.compiled.rects.clone(),
            tokens: Some(self.compiled.tokens.clone()),
            windows: self.windows.clone(),
            netmap: self.netmap.clone(),
            memory: self.memory.clone(),
            dnd: self.dnd,
            deck_active: self.deck_active,
            gutter_hidden: self.gutter_hidden,
            terminal_bottom_only: self.terminal_bottom_only,
            desktop_count: self.desktop_count,
            current_desktop: self.current_desktop,
            audio: std::fs::read_to_string(self.state_dir.join("audio").join("latest"))
                .ok()
                .map(|s| {
                    s.split(',')
                        .filter_map(|v| v.trim().parse::<u8>().ok())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Recompute the grid for the current display options: the RAM gutter may be
    /// hidden (DISPLAY extends left) and the docked console may be limited to the
    /// bottom-right quadrant (DISPLAY then takes the full width above it).
    fn apply_gutter(&mut self) {
        let (sw, sh) = (self.compiled.screen.w, self.compiled.screen.h);
        let Ok(base) = self.layout.compile(sw, sh) else {
            return;
        };
        let mut r = base.rects;

        if self.gutter_hidden {
            let gw = r.sidebar.w;
            r.sidebar = Rect::new(0, r.sidebar.y, 0, r.sidebar.h);
            r.stage = Rect::new(0, r.stage.y, r.stage.w + gw, r.stage.h);
            r.stage_app = Rect::new(0, r.stage_app.y, r.stage_app.w + gw, r.stage_app.h);
            // Netmap already starts at x = 0 and ends at sw - terminal.w, so its rect does not change.
        }

        if self.terminal_bottom_only {
            // Terminal shrinks to the bottom-right quadrant; NETMAP sits to its
            // left in the bottom row, and DISPLAY grows to the full width above
            // both so no two panels ever overlap.
            let t = r.terminal;
            let bottom_h = (t.h / 3).max(r.netmap.h);
            r.terminal = Rect::new(t.x, sh - bottom_h, t.w, bottom_h);
            r.terminal_app = Rect::new(
                r.terminal.x,
                r.terminal.y + self.compiled.tokens.caption,
                r.terminal.w,
                (r.terminal.h - self.compiled.tokens.caption).max(40),
            );

            let left = if self.gutter_hidden { 0 } else { r.sidebar.w };
            let full_w = sw - left;
            let stage_h = (sh - r.stage.y - bottom_h).max(160);
            r.stage = Rect::new(left, r.stage.y, full_w, stage_h);
            r.stage_app = Rect::new(
                left,
                r.stage_app.y,
                full_w,
                (stage_h - self.compiled.tokens.caption - self.compiled.tokens.stage_title).max(40),
            );
            if !self.gutter_hidden {
                r.sidebar = Rect::new(0, r.sidebar.y, r.sidebar.w, stage_h);
            }
            let pad = self.compiled.tokens.visualizer_padding;
            let vis = self.compiled.tokens.visualizer;
            r.visualizer = Rect::new(pad, sh - bottom_h - vis - pad, r.visualizer.w, vis);
            // Netmap sits on the bottom-left beside the bottom-right terminal.
            r.netmap = Rect::new(0, sh - bottom_h, sw - t.w, bottom_h);
        }

        self.compiled.rects = r;
    }

    pub fn toggle_gutter(&mut self) {
        self.gutter_hidden = !self.gutter_hidden;
        self.apply_gutter();
        // Windows must be re-placed into the new grid, otherwise GTK apps keep
        // their old geometry and end up drawn over the shell chrome.
        self.placed.clear();
        self.revision += 1;
    }

    /// Add a new virtual desktop
    pub fn add_desktop(&mut self) -> Result<(), String> {
        self.desktop_count += 1;
        self.switch_desktop(self.desktop_count)
    }

    /// Remove a virtual desktop (cannot remove if only one exists)
    pub fn remove_desktop(&mut self, id: i32) -> Result<(), String> {
        if self.desktop_count <= 1 || id < 1 || id > self.desktop_count {
            return Ok(());
        }
        // Close stage windows on this desktop
        let wins_to_close: Vec<String> = self
            .windows
            .iter()
            .filter(|w| self.desktop_of(w.deck) == id)
            .map(|w| w.id.clone())
            .collect();
        for wid in wins_to_close {
            let _ = self.kill(&wid);
        }
        self.desktop_count -= 1;
        if self.current_desktop >= id {
            let target = self.current_desktop.min(self.desktop_count).max(1);
            self.switch_desktop(target)?;
        }
        self.rebuild_netmap();
        self.revision += 1;
        Ok(())
    }

    /// Switch to a specific virtual desktop
    pub fn switch_desktop(&mut self, id: i32) -> Result<(), String> {
        if id < 1 {
            return Ok(());
        }
        if id > self.desktop_count {
            self.desktop_count = id;
        }
        self.current_desktop = id;
        let home = self.home_deck_for(id);
        let target_deck = self
            .windows
            .iter()
            .find(|w| {
                (w.zone == Zone::Stage || w.zone == Zone::Dialog)
                    && self.desktop_of(w.deck) == id
                    && w.active
            })
            .or_else(|| {
                self.windows.iter().rev().find(|w| {
                    (w.zone == Zone::Stage || w.zone == Zone::Dialog)
                        && self.desktop_of(w.deck) == id
                })
            })
            .map(|w| w.deck)
            .unwrap_or(home);

        self.deck_active = target_deck;
        self.backend.switch_workspace(target_deck)?;
        if let Some(target_win) = self.windows.iter().find(|w| w.deck == target_deck) {
            let _ = self.backend.focus(&target_win.id);
        }
        self.rebuild_netmap();
        self.revision += 1;
        Ok(())
    }

    /// Move to the next/previous virtual desktop, wrapping around.
    pub fn step_desktop(&mut self, delta: i32) -> Result<(), String> {
        let n = self.desktop_count.max(1);
        let next = ((self.current_desktop - 1 + delta) % n + n) % n + 1;
        self.switch_desktop(next)
    }

    /// Toggle fullscreen on the focused stage window (or the first one).
    pub fn toggle_fullscreen_active(&mut self) -> Result<(), String> {
        let current = self.fullscreen_window.clone();
        if let Some(id) = current {
            return self.set_fullscreen(&id, false);
        }
        let target = self
            .windows
            .iter()
            .find(|w| w.zone == Zone::Stage && w.active)
            .or_else(|| self.windows.iter().find(|w| w.zone == Zone::Stage))
            .map(|w| w.id.clone());
        match target {
            Some(id) => self.set_fullscreen(&id, true),
            None => Ok(()),
        }
    }

    /// Toggle fullscreen for a window
    pub fn set_fullscreen(&mut self, id: &str, enable: bool) -> Result<(), String> {
        if enable {
            self.fullscreen_window = Some(id.to_string());
            self.backend.set_fullscreen(id, true)?;
        } else {
            if self.fullscreen_window.as_deref() == Some(id) {
                self.fullscreen_window = None;
            }
            self.backend.set_fullscreen(id, false)?;
        }
        self.revision += 1;
        Ok(())
    }

    pub fn toggle_terminal_bottom_only(&mut self) -> Result<(), String> {
        self.terminal_bottom_only = !self.terminal_bottom_only;
        self.apply_gutter();
        self.placed.clear(); // force the docked console to move to its new rect
        self.revision += 1;
        Ok(())
    }

    pub fn activate(&mut self, id: &str) -> Result<(), String> {
        let w = self
            .windows
            .iter()
            .find(|w| w.id == id)
            .cloned()
            .ok_or_else(|| format!("no window {id}"))?;
        let desk = self.desktop_of(w.deck);
        if desk != self.current_desktop {
            self.current_desktop = desk;
        }
        match w.zone {
            Zone::Stage => {
                self.deck_active = w.deck;
                self.backend.switch_workspace(w.deck)?;
                self.backend.focus(id)?;
            }
            Zone::Dialog => {
                if w.deck > 0 {
                    self.deck_active = w.deck;
                    self.backend.switch_workspace(w.deck)?;
                }
                self.backend.focus(id)?;
            }
            _ => self.backend.focus(id)?,
        }
        // Track focus so cycle() and the netmap highlight follow it.
        for x in self.windows.iter_mut() {
            x.active = x.id == id;
        }
        self.rebuild_netmap();
        self.revision += 1;
        Ok(())
    }

    /// Cycle the stage: +1 = next window, -1 = previous, 0 = go home (close all
    /// stage windows so the Connected screen shows again).
    pub fn cycle(&mut self, delta: i32) -> Result<(), String> {
        // delta == 0 means "go home": switch to home deck of active desktop.
        // Always switch to home deck so StageHome (Connected screen) shows
        // without killing open stage windows.
        if delta == 0 {
            let deck = self.home_deck_for(self.current_desktop);
            self.deck_active = deck;
            self.backend.switch_workspace(deck)?;
            for x in self.windows.iter_mut() {
                if x.zone == Zone::Stage {
                    x.active = false;
                }
            }
            self.rebuild_netmap();
            self.revision += 1;
            return Ok(());
        }

        // If a terminal is currently active, cycle between open terminal windows
        let is_terminal_active = self
            .windows
            .iter()
            .any(|w| w.active && w.zone == Zone::Terminal);
        if is_terminal_active {
            let mut term_windows: Vec<Window> = self
                .windows
                .iter()
                .filter(|w| w.zone == Zone::Terminal)
                .cloned()
                .collect();
            if term_windows.len() > 1 {
                term_windows.sort_by(|a, b| a.id.cmp(&b.id));
                let cur = term_windows.iter().position(|w| w.active).unwrap_or(0);
                let n = term_windows.len() as i32;
                let next = ((cur as i32 + delta) % n + n) % n;
                let target = term_windows[next as usize].id.clone();
                return self.activate(&target);
            }
        }

        // Cycle over stage WINDOWS on the current desktop (sorted by deck, then id):
        // several windows of the same class share one deck, and cycling must
        // still bring each of them up in turn. activate() switches to the
        // target's deck and focuses it, so the deck change is visible.
        let cur_desk = self.current_desktop;
        let mut stage_windows: Vec<Window> = self
            .windows
            .iter()
            .filter(|w| w.zone == Zone::Stage && self.desktop_of(w.deck) == cur_desk)
            .cloned()
            .collect();
        stage_windows.sort_by_key(|a| (a.deck, a.id.clone()));
        if stage_windows.is_empty() {
            return Ok(());
        }
        let n = stage_windows.len() as i32;
        // Start from the focused window; if none reports active (e.g. focus was
        // set outside a reconcile), fall back to the deck we're on.
        let cur = stage_windows.iter().position(|w| w.active).or_else(|| {
            stage_windows
                .iter()
                .position(|w| w.deck == self.deck_active)
        });
        let next = match cur {
            Some(p) => ((p as i32 + delta) % n + n) % n,
            None => 0,
        };
        let target = stage_windows[next as usize].id.clone();
        self.activate(&target)
    }

    pub fn kill(&mut self, id: &str) -> Result<(), String> {
        let w = self
            .windows
            .iter()
            .find(|w| w.id == id)
            .cloned()
            .ok_or_else(|| format!("no window {id}"))?;
        self.backend.kill(&w.id, w.pid)?;
        self.windows.retain(|x| x.id != id);
        self.placed.remove(id);
        let cur_desk = self.desktop_of(self.deck_active);
        let home_deck = self.home_deck_for(cur_desk);
        if self.deck_active != home_deck
            && !self.windows.iter().any(|w| {
                (w.zone == Zone::Stage || w.zone == Zone::Dialog) && w.deck == self.deck_active
            })
        {
            if let Some(next) = self
                .windows
                .iter()
                .rev()
                .find(|w| w.zone == Zone::Stage && self.desktop_of(w.deck) == cur_desk)
                .cloned()
            {
                self.deck_active = next.deck;
                let _ = self.backend.switch_workspace(next.deck);
                let _ = self.backend.focus(&next.id);
            } else {
                self.deck_active = home_deck;
                let _ = self.backend.switch_workspace(home_deck);
            }
        }
        self.rebuild_netmap();
        self.revision += 1;
        Ok(())
    }

    pub fn set_zone(&mut self, id: &str, zone: Zone) -> Result<(), String> {
        if !self.windows.iter().any(|w| w.id == id) {
            return Err(format!("no window {id}"));
        }
        self.overrides.insert(id.to_string(), zone);
        let class = self
            .windows
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.class.clone())
            .unwrap_or_default();
        let used: Vec<i32> = self
            .windows
            .iter()
            .filter(|w| w.id != id && (w.zone == Zone::Stage || w.zone == Zone::Dialog))
            .map(|w| w.deck)
            .collect();
        let deck = if zone == Zone::Stage {
            Some(self.deck_for(id, &class, &used))
        } else {
            None
        };
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.zone = zone;
            if let Some(d) = deck {
                w.deck = d;
            }
        }
        self.placed.remove(id);
        self.rebuild_netmap();
        self.revision += 1;
        Ok(())
    }

    pub fn netmap_move(&mut self, id: &str, x: i32, y: i32) {
        if let Some(node) = self.netmap.nodes.iter_mut().find(|n| n.id == id) {
            let rect = self.compiled.rects.netmap;
            node.x = x.clamp(24, rect.w - 24);
            node.y = y.clamp(24, rect.h - 24);
            // Cache by window id — same-class windows each need their own slot.
            self.positions.insert(node.id.clone(), (node.x, node.y));
            self.revision += 1;
            self.save();
        }
    }

    pub fn netmap_reset(&mut self) {
        self.positions.clear();
        self.rebuild_netmap();
        self.revision += 1;
        self.save();
    }

    pub fn set_theme(&mut self, id: &str) -> Result<(), String> {
        let status = std::process::Command::new("hacknet-theme")
            .arg("apply")
            .arg(id)
            .output()
            .map_err(|e| format!("hacknet-theme: {e}"))?;
        if !status.status.success() {
            return Err(String::from_utf8_lossy(&status.stderr).trim().to_string());
        }
        self.theme_id = id.to_string();
        self.revision += 1;
        // Signal foot terminals to reload their config (foot supports SIGUSR1).
        let _ = std::process::Command::new("pkill")
            .args(["-USR1", "-x", "foot"])
            .status();
        Ok(())
    }

    pub fn media(&self, action: &str) -> Result<(), String> {
        let arg = match action {
            "play_pause" | "toggle" => "play-pause",
            "next" => "next",
            "previous" | "prev" => "previous",
            "stop" => "stop",
            other => return Err(format!("unknown media action {other}")),
        };
        std::process::Command::new("playerctl")
            .arg(arg)
            .output()
            .map_err(|e| format!("playerctl: {e}"))?;
        Ok(())
    }

    pub fn set_volume(&self, value: f64) -> Result<(), String> {
        let v = (value.clamp(0.0, 1.5) * 100.0).round() as i64;
        let _ = std::process::Command::new("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{v}%")])
            .output();
        Ok(())
    }

    pub fn adjust_volume(&self, delta: f64) -> Result<(), String> {
        let pct = (delta.abs() * 100.0).round() as i64;
        let arg = if delta < 0.0 {
            format!("{pct}%-")
        } else {
            format!("{pct}%+")
        };
        let _ = std::process::Command::new("wpctl")
            .args(["set-volume", "-l", "1.5", "@DEFAULT_AUDIO_SINK@", &arg])
            .output();
        Ok(())
    }

    pub fn toggle_mute(&self) -> Result<(), String> {
        let _ = std::process::Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
            .output();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::Mock;
    use hacknet_proto::layout::Rects;

    /// The geometry the current design/layout.toml compiles to at 1920x1080.
    /// Tests assert against this instead of hard-coded pixels so a token change
    /// cannot silently leave them asserting an obsolete layout.
    fn expected() -> Rects {
        Layout::parse(include_str!("../../../design/layout.toml"))
            .unwrap()
            .compile(1920, 1080)
            .unwrap()
            .rects
    }

    fn engine() -> (Engine, std::sync::Arc<Mock>) {
        let layout = Layout::parse(include_str!("../../../design/layout.toml")).unwrap();
        let mock = std::sync::Arc::new(Mock::default());
        struct Shared(std::sync::Arc<Mock>);
        impl Backend for Shared {
            fn name(&self) -> &'static str {
                "mock"
            }
            fn list(&self) -> Result<Vec<RawWindow>, String> {
                self.0.list()
            }
            fn place(&self, w: &RawWindow, r: Rect, s: bool) -> Result<(), String> {
                self.0.place(w, r, s)
            }
            fn tile(&self, id: &str) -> Result<(), String> {
                self.0.tile(id)
            }
            fn focus(&self, id: &str) -> Result<(), String> {
                self.0.focus(id)
            }
            fn kill(&self, id: &str, p: i32) -> Result<(), String> {
                self.0.kill(id, p)
            }
            fn to_workspace(&self, id: &str, w: i32) -> Result<(), String> {
                self.0.to_workspace(id, w)
            }
            fn switch_workspace(&self, w: i32) -> Result<(), String> {
                self.0.switch_workspace(w)
            }
            fn reload(&self) -> Result<(), String> {
                self.0.reload()
            }
            fn native_fullscreen(&self, id: &str, on: bool) -> Result<(), String> {
                self.0.native_fullscreen(id, on)
            }
        }
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hacknet-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let e = Engine::new_in(layout, Box::new(Shared(mock.clone())), 1920, 1080, dir).unwrap();
        (e, mock)
    }

    fn raw(id: &str, class: &str, pid: i32, w: i32, h: i32) -> RawWindow {
        RawWindow {
            id: id.into(),
            title: class.into(),
            class: class.into(),
            pid,
            rect: Rect::new(0, 0, w, h),
            ..Default::default()
        }
    }

    #[test]
    fn stage_windows_are_placed_inside_the_viewport() {
        let (mut e, m) = engine();
        m.insert(raw("s1", "editor", 7000, 400, 400));
        e.reconcile().unwrap();
        let w = e.windows.iter().find(|w| w.id == "s1").unwrap();
        let r = expected().stage_app;
        assert_eq!(w.zone, Zone::Stage);
        assert_eq!(w.rect, r);
        assert!(m
            .calls()
            .iter()
            .any(|c| c == &format!("place s1 {} {} {} {} sticky=false", r.x, r.y, r.w, r.h)));
    }

    #[test]
    fn terminal_is_pinned_to_the_terminal_rect() {
        let (mut e, m) = engine();
        m.insert(raw("t1", "hacknet-term", 1000, 400, 900));
        e.reconcile().unwrap();
        let w = &e.windows[0];
        let r = expected().terminal_app;
        assert_eq!(w.zone, Zone::Terminal);
        assert_eq!(w.rect, r);
        // Terminals appear in the netmap (each one is a clickable node) and
        // link to the operator's home machine when they have no mapped parent.
        assert!(e.netmap.nodes.iter().any(|n| n.id == "t1"));
        assert!(e
            .netmap
            .edges
            .iter()
            .any(|ed| ed.from == "home" && ed.to == "t1"));
        assert!(m
            .calls()
            .iter()
            .any(|c| c == &format!("place t1 {} {} {} {} sticky=true", r.x, r.y, r.w, r.h)));
    }

    #[test]
    fn small_second_window_is_a_dialog_and_centred() {
        let (mut e, m) = engine();
        m.insert(raw("s1", "editor", 2000, 1224, 884));
        e.reconcile().unwrap();
        m.insert(raw("d1", "editor", 2000, 500, 300));
        e.reconcile().unwrap();
        let d = e.windows.iter().find(|w| w.id == "d1").unwrap();
        assert_eq!(d.zone, Zone::Dialog);
        // centred inside the stage_app region
        let s = expected().stage_app;
        assert!((d.rect.x - (s.x + (s.w - 500) / 2)).abs() <= 2);
        assert!((d.rect.y - (s.y + (s.h - 300) / 2)).abs() <= 2);
    }

    #[test]
    fn portal_file_chooser_is_dialog_and_constrained_to_stage_app() {
        let (mut e, m) = engine();
        m.insert(raw("s1", "code-oss", 2000, 1224, 884));
        e.reconcile().unwrap();
        // Massive file chooser from portal
        m.insert(raw("p1", "xdg-desktop-portal-gtk", 2001, 1203, 800));
        e.reconcile().unwrap();
        let p = e.windows.iter().find(|w| w.id == "p1").unwrap();
        assert_eq!(p.zone, Zone::Dialog);
        let s = expected().stage_app;
        // Dialog must be strictly contained within stage_app
        assert!(p.rect.x >= s.x);
        assert!(p.rect.y >= s.y);
        assert!(p.rect.x + p.rect.w <= s.x + s.w);
        assert!(p.rect.y + p.rect.h <= s.y + s.h);
    }

    #[test]
    fn sidebar_apps_get_dedicated_slots_and_netmap_nodes() {
        let (mut e, m) = engine();
        m.insert(raw("k1", "hacknet-dock-1", 3000, 200, 200));
        m.insert(raw("k2", "hacknet-dock-2", 3001, 200, 200));
        e.reconcile().unwrap();
        assert_eq!(e.windows[0].zone, Zone::Sidebar);
        // Two dock terminals: two netmap nodes, two distinct slot rects.
        assert!(e.netmap.nodes.iter().any(|n| n.id == "k1"));
        assert!(e.netmap.nodes.iter().any(|n| n.id == "k2"));
        let r1 = e.windows.iter().find(|w| w.id == "k1").unwrap().rect;
        let r2 = e.windows.iter().find(|w| w.id == "k2").unwrap().rect;
        assert_ne!((r1.x, r1.y), (r2.x, r2.y));
    }

    #[test]
    fn netmap_positions_persist() {
        let (mut e, m) = engine();
        m.insert(raw("a1", "browser", 4000, 1224, 884));
        m.insert(raw("b1", "chat", 4001, 1224, 884));
        e.reconcile().unwrap();
        assert_eq!(e.netmap.nodes.len(), 2);
        let id = e.netmap.nodes[0].id.clone();
        e.netmap_move(&id, 111, 44);
        // Positions are cached per WINDOW ID (same-class windows each need
        // their own slot), and the moved node keeps its custom position.
        assert_eq!(e.positions.get(&id), Some(&(111, 44)));
        let node = e.netmap.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!((node.x, node.y), (111, 44));
    }

    #[test]
    fn switching_deck_targets_the_right_workspace() {
        let (mut e, m) = engine();
        m.insert(raw("a1", "browser", 5000, 1224, 884));
        e.reconcile().unwrap();
        m.insert(raw("b1", "chat", 5001, 1224, 884));
        e.reconcile().unwrap();
        let a = e.windows.iter().find(|w| w.id == "a1").unwrap().clone();
        let b = e.windows.iter().find(|w| w.id == "b1").unwrap().clone();
        e.activate("a1").unwrap();
        assert_eq!(m.workspace.lock().unwrap().clone(), a.deck);
        e.activate("b1").unwrap();
        assert_eq!(m.workspace.lock().unwrap().clone(), b.deck);
    }

    #[test]
    fn unresponsive_when_pid_vanishes() {
        let (mut e, m) = engine();
        m.insert(raw("a1", "browser", 999_999, 1224, 884));
        e.reconcile().unwrap();
        assert_eq!(e.windows[0].state, WinState::Unresponsive);
    }

    #[test]
    fn cycle_walks_stage_decks_and_wraps() {
        let (mut e, m) = engine();
        m.insert(raw("a1", "browser", 6000, 1224, 884));
        e.reconcile().unwrap();
        m.insert(raw("b1", "chat", 6001, 1224, 884));
        e.reconcile().unwrap();
        let a = e.windows.iter().find(|w| w.id == "a1").unwrap().deck;
        let b = e.windows.iter().find(|w| w.id == "b1").unwrap().deck;
        assert_ne!(a, b);
        e.activate("a1").unwrap();
        e.cycle(1).unwrap();
        assert_eq!(e.deck_active, b);
        e.cycle(1).unwrap();
        assert_eq!(e.deck_active, a);
        e.cycle(-1).unwrap();
        assert_eq!(e.deck_active, b);
    }

    #[test]
    fn gutter_toggle_maintains_netmap_and_terminal_boundary() {
        let (mut e, _m) = engine();
        let base_netmap = e.compiled.rects.netmap;
        let base_terminal = e.compiled.rects.terminal;
        assert_eq!(base_netmap.x + base_netmap.w, base_terminal.x);

        // Toggle gutter hidden
        e.toggle_gutter();
        let hidden_netmap = e.compiled.rects.netmap;
        let hidden_terminal = e.compiled.rects.terminal;
        assert_eq!(hidden_netmap.x + hidden_netmap.w, hidden_terminal.x);
        assert_eq!(hidden_netmap.w, base_netmap.w);
        assert_eq!(e.compiled.rects.sidebar.w, 0);

        // Toggle gutter back on
        e.toggle_gutter();
        let restored_netmap = e.compiled.rects.netmap;
        let restored_terminal = e.compiled.rects.terminal;
        assert_eq!(restored_netmap.x + restored_netmap.w, restored_terminal.x);
        assert_eq!(restored_netmap.w, base_netmap.w);
    }

    #[test]
    fn terminal_bottom_only_and_gutter_combinations_never_overlap() {
        let (mut e, _m) = engine();
        let overlaps = |a: Rect, b: Rect| -> bool {
            a.w > 0
                && a.h > 0
                && b.w > 0
                && b.h > 0
                && a.x < b.x + b.w
                && a.x + a.w > b.x
                && a.y < b.y + b.h
                && a.y + a.h > b.y
        };
        for gutter in [false, true] {
            for bottom in [false, true] {
                if e.gutter_hidden != gutter {
                    e.toggle_gutter();
                }
                if e.terminal_bottom_only != bottom {
                    e.toggle_terminal_bottom_only().unwrap();
                }
                let r = &e.compiled.rects;
                let regions = [
                    ("topbar", r.topbar),
                    ("sidebar", r.sidebar),
                    ("stage", r.stage),
                    ("netmap", r.netmap),
                    ("terminal", r.terminal),
                ];
                for i in 0..regions.len() {
                    for j in (i + 1)..regions.len() {
                        assert!(
                            !overlaps(regions[i].1, regions[j].1),
                            "overlap between {} ({:?}) and {} ({:?}) when gutter_hidden={} terminal_bottom_only={}",
                            regions[i].0,
                            regions[i].1,
                            regions[j].0,
                            regions[j].1,
                            gutter,
                            bottom
                        );
                    }
                }
                assert_eq!(r.netmap.x + r.netmap.w, r.terminal.x);
            }
        }
    }

    #[test]
    fn closing_active_stage_deck_switches_to_remaining_stage_or_home() {
        let (mut e, m) = engine();
        m.insert(raw("a1", "browser", 6000, 1224, 884));
        e.reconcile().unwrap();
        let a_deck = e.deck_active;
        assert!(a_deck > 1);

        m.insert(raw("b1", "chat", 6001, 1224, 884));
        e.reconcile().unwrap();
        let b_deck = e.deck_active;
        assert_ne!(a_deck, b_deck);

        // b1 closes naturally (removed from backend window list): reconcile
        // should switch back to a1's deck instead of stranding on empty b_deck.
        m.windows.lock().unwrap().retain(|w| w.id != "b1");
        e.reconcile().unwrap();
        assert_eq!(e.deck_active, a_deck);
        assert_eq!(*m.workspace.lock().unwrap(), a_deck);

        // a1 closes via kill(): should return to deck 1 (Home).
        e.kill("a1").unwrap();
        assert_eq!(e.deck_active, 1);
        assert_eq!(*m.workspace.lock().unwrap(), 1);
    }

    #[test]
    fn multiple_desktops_isolate_windows_and_switch_cleanly() {
        let (mut e, m) = engine();
        assert_eq!(e.desktop_count, 1);
        assert_eq!(e.current_desktop, 1);

        // Add Desktop 2
        e.add_desktop().unwrap();
        assert_eq!(e.desktop_count, 2);
        assert_eq!(e.current_desktop, 2);
        assert_eq!(e.deck_active, 101); // Desktop 2 Home
        assert_eq!(*m.workspace.lock().unwrap(), 101);

        // Open an app while on Desktop 2
        m.insert(raw("d2_app", "thunar", 8000, 1224, 884));
        e.reconcile().unwrap();
        assert_eq!(e.deck_active, 102);
        assert_eq!(*m.workspace.lock().unwrap(), 102);

        // Switch to Desktop 1 and open an app there
        e.switch_desktop(1).unwrap();
        assert_eq!(e.current_desktop, 1);
        assert_eq!(e.deck_active, 1);
        assert_eq!(*m.workspace.lock().unwrap(), 1);

        m.insert(raw("d1_app", "browser", 8001, 1224, 884));
        e.reconcile().unwrap();
        assert_eq!(e.deck_active, 2);
        assert_eq!(*m.workspace.lock().unwrap(), 2);

        // Switch back to Desktop 2: restores d2_app
        e.switch_desktop(2).unwrap();
        assert_eq!(e.current_desktop, 2);
        assert_eq!(e.deck_active, 102);
        assert_eq!(*m.workspace.lock().unwrap(), 102);

        // Switch back to Desktop 1: restores d1_app
        e.switch_desktop(1).unwrap();
        assert_eq!(e.current_desktop, 1);
        assert_eq!(e.deck_active, 2);
        assert_eq!(*m.workspace.lock().unwrap(), 2);

        // Remove Desktop 2: cleans up d2_app, stays on Desktop 1
        e.remove_desktop(2).unwrap();
        assert_eq!(e.desktop_count, 1);
        assert_eq!(e.current_desktop, 1);
        assert!(!e.windows.iter().any(|w| w.id == "d2_app"));
        assert!(e.windows.iter().any(|w| w.id == "d1_app"));
    }

    #[test]
    fn multiple_stage_apps_get_distinct_decks_and_no_dialog_border() {
        let (mut e, m) = engine();
        // VS Code opens
        m.insert(raw("code1", "code-oss", 5001, 1224, 884));
        e.reconcile().unwrap();
        let code_deck = e.windows.iter().find(|w| w.id == "code1").unwrap().deck;
        let code_zone = e.windows.iter().find(|w| w.id == "code1").unwrap().zone;
        assert_eq!(code_zone, Zone::Stage);
        assert_eq!(code_deck, 2);

        // Thunar opens while VS Code is open
        m.insert(raw("thunar1", "thunar", 5002, 1224, 884));
        e.reconcile().unwrap();
        let thunar_deck = e.windows.iter().find(|w| w.id == "thunar1").unwrap().deck;
        let thunar_zone = e.windows.iter().find(|w| w.id == "thunar1").unwrap().zone;
        // Thunar must be a Stage app with its own deck, NOT a Dialog stacking on VS Code
        assert_eq!(thunar_zone, Zone::Stage);
        assert_ne!(thunar_deck, code_deck);
        assert_eq!(thunar_deck, 3);
        assert_eq!(e.deck_active, 3);

        // A second Thunar window opens
        m.insert(raw("thunar2", "thunar", 5002, 1224, 884));
        e.reconcile().unwrap();
        let thunar2_deck = e.windows.iter().find(|w| w.id == "thunar2").unwrap().deck;
        let thunar2_zone = e.windows.iter().find(|w| w.id == "thunar2").unwrap().zone;
        assert_eq!(thunar2_zone, Zone::Stage);
        assert_ne!(thunar2_deck, thunar_deck);
        assert_ne!(thunar2_deck, code_deck);
        assert_eq!(thunar2_deck, 4);

        // Preferences dialog for Thunar opens
        let mut pref = raw("pref1", "thunar", 5002, 596, 594);
        pref.title = "Thunar Preferences".to_string();
        m.insert(pref);
        e.reconcile().unwrap();
        let pref_deck = e.windows.iter().find(|w| w.id == "pref1").unwrap().deck;
        let pref_zone = e.windows.iter().find(|w| w.id == "pref1").unwrap().zone;
        assert_eq!(pref_zone, Zone::Dialog);
        // Belongs on Thunar's deck, not VS Code
        assert!(pref_deck == 3 || pref_deck == 4);
    }

    #[test]
    fn netmap_isolates_desktops() {
        let (mut e, m) = engine();
        // Desktop 1 app
        m.insert(raw("app_d1", "code-oss", 6001, 1224, 884));
        e.reconcile().unwrap();
        assert!(e.netmap.nodes.iter().any(|n| n.id == "app_d1"));

        // Add Desktop 2 and open app there
        e.add_desktop().unwrap();
        m.insert(raw("app_d2", "thunar", 6002, 1224, 884));
        e.reconcile().unwrap();

        // While on Desktop 2: app_d2 is in netmap, app_d1 is NOT
        assert_eq!(e.current_desktop, 2);
        assert!(e.netmap.nodes.iter().any(|n| n.id == "app_d2"));
        assert!(!e.netmap.nodes.iter().any(|n| n.id == "app_d1"));

        // Switch back to Desktop 1: app_d1 is in netmap, app_d2 is NOT
        e.switch_desktop(1).unwrap();
        assert_eq!(e.current_desktop, 1);
        assert!(e.netmap.nodes.iter().any(|n| n.id == "app_d1"));
        assert!(!e.netmap.nodes.iter().any(|n| n.id == "app_d2"));
    }

    #[test]
    fn netmap_scales_grid_for_many_apps() {
        let (mut e, m) = engine();
        // Spawn 8 apps (> 6 apps)
        for i in 1..=8 {
            m.insert(raw(&format!("app_{i}"), "browser", 7000 + i, 1224, 884));
        }
        e.reconcile().unwrap();

        // NetMap must contain all 8 nodes
        assert_eq!(e.netmap.nodes.len(), 8);
        for i in 1..=8 {
            assert!(e.netmap.nodes.iter().any(|n| n.id == format!("app_{i}")));
        }

        // Nodes must span 2D space (not all on the same Y line)
        let y_coords: std::collections::HashSet<i32> = e.netmap.nodes.iter().map(|n| n.y).collect();
        assert!(
            y_coords.len() > 1,
            "NetMap nodes must be distributed across multiple Y positions"
        );
    }
}
