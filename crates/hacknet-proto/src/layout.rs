//! The fixed HacknetOS grid. Mirrors `tools/hacknet-layout.py` exactly.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rects {
    pub topbar: Rect,
    pub sidebar: Rect,
    pub visualizer: Rect,
    pub stage: Rect,
    /// Stage viewport below the caption + window title strip.
    pub stage_app: Rect,
    pub terminal: Rect,
    /// Terminal viewport below its caption.
    pub terminal_app: Rect,
    pub netmap: Rect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub gap: i32,
    pub topbar: i32,
    pub netmap: i32,
    pub sidebar: i32,
    pub terminal: i32,
    pub visualizer: i32,
    pub visualizer_padding: i32,
    pub ram: i32,
    pub min_slot: i32,
    pub splitter: i32,
    pub stage_border: i32,
    pub stage_bracket: i32,
    /// Caption strip above each region ("DISPLAY", "TERMINAL").
    pub caption: i32,
    /// Extra title strip inside DISPLAY that names the active window.
    pub stage_title: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Screen {
    pub w: i32,
    pub h: i32,
}

/// Result of compiling the layout for one output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compiled {
    pub version: i64,
    pub screen: Screen,
    pub compact: bool,
    pub tokens: Tokens,
    pub rects: Rects,
}

#[derive(Debug, Clone, Deserialize)]
struct Raw {
    #[serde(default = "one")]
    version: i64,
    display: RawDisplay,
    grid: RawGrid,
    dock: RawDock,
    visualizer: RawVis,
    stage: RawStage,
    #[serde(default)]
    breakpoints: RawBreakpoints,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Clone, Deserialize)]
struct RawDisplay {
    baseline_width: i32,
    baseline_height: i32,
    min_width: i32,
    min_height: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct RawGrid {
    gap: i32,
    topbar: i32,
    netmap: i32,
    sidebar: i32,
    terminal: i32,
    #[serde(default)]
    caption: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct RawDock {
    ram: i32,
    min_slot: i32,
    splitter: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct RawVis {
    size: i32,
    padding: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct RawStage {
    border: i32,
    bracket: i32,
    #[serde(default)]
    title: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawBreakpoints {
    #[serde(default)]
    compact: RawCompact,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawCompact {
    #[serde(default)]
    max_width: i32,
    #[serde(default)]
    sidebar: i32,
    #[serde(default)]
    terminal: i32,
    #[serde(default)]
    netmap: i32,
    #[serde(default)]
    visualizer: i32,
}

pub struct Layout {
    raw: Raw,
}

impl Layout {
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| format!("layout.toml: {e}"))?;
        if raw.grid.topbar <= 0 || raw.grid.sidebar <= 0 || raw.grid.terminal <= 0 {
            return Err("layout.toml: grid regions must be positive".into());
        }
        Ok(Self { raw })
    }

    pub fn from_path(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn min_size(&self) -> (i32, i32) {
        (self.raw.display.min_width, self.raw.display.min_height)
    }

    pub fn baseline(&self) -> (i32, i32) {
        (
            self.raw.display.baseline_width,
            self.raw.display.baseline_height,
        )
    }

    /// Compile the grid for a concrete logical output size.
    pub fn compile(&self, w: i32, h: i32) -> Result<Compiled, String> {
        let (min_w, min_h) = self.min_size();
        if w < min_w || h < min_h {
            return Err(format!(
                "unsupported resolution {w}x{h} (minimum {min_w}x{min_h})"
            ));
        }
        let g = &self.raw.grid;
        let c = &self.raw.breakpoints.compact;
        let compact = c.max_width > 0 && w <= c.max_width;
        let pick = |compact_v: i32, normal: i32| {
            if compact && compact_v > 0 {
                compact_v
            } else {
                normal
            }
        };
        // Non-compact layouts scale the baseline tokens with the output so the
        // split stays 13.3% / 48.6% / 38.1% at every resolution (not just 1920).
        let wx = w as f64 / 1920.0;
        let hy = h as f64 / 1080.0;
        let sx = |v: i32| ((v as f64) * wx).round() as i32;
        let sy = |v: i32| ((v as f64) * hy).round() as i32;
        let sidebar = if compact {
            pick(c.sidebar, g.sidebar)
        } else {
            sx(g.sidebar)
        };
        let terminal = if compact {
            pick(c.terminal, g.terminal)
        } else {
            sx(g.terminal)
        };
        let netmap = if compact {
            pick(c.netmap, g.netmap)
        } else {
            sy(g.netmap)
        };
        let vis = pick(c.visualizer, self.raw.visualizer.size);
        let topbar = if compact { g.topbar } else { sy(g.topbar) };
        let pad = self.raw.visualizer.padding;

        let stage = Rect::new(sidebar, topbar, w - sidebar - terminal, h - topbar - netmap);
        if stage.w <= 0 || stage.h <= 0 {
            return Err(format!("layout does not fit {w}x{h}"));
        }
        let terminal_rect = Rect::new(w - terminal, topbar, terminal, h - topbar);
        // Chrome: a caption line above each region, plus a title strip inside
        // DISPLAY naming the active window. Applications live inside the
        // remaining viewport so they never draw over the labels.
        let caption = g.caption.max(0);
        let stage_app = Rect::new(
            stage.x,
            stage.y + caption + self.raw.stage.title.max(0),
            stage.w,
            stage.h - caption - self.raw.stage.title.max(0),
        );
        let terminal_app = Rect::new(
            terminal_rect.x,
            terminal_rect.y + caption,
            terminal_rect.w,
            terminal_rect.h - caption,
        );
        if stage_app.h <= 0 || terminal_app.h <= 0 {
            return Err(format!("chrome does not fit {w}x{h}"));
        }

        Ok(Compiled {
            version: self.raw.version,
            screen: Screen { w, h },
            compact,
            tokens: Tokens {
                gap: g.gap,
                topbar,
                netmap,
                sidebar,
                terminal,
                visualizer: vis,
                visualizer_padding: pad,
                ram: self.raw.dock.ram,
                min_slot: self.raw.dock.min_slot,
                splitter: self.raw.dock.splitter,
                stage_border: self.raw.stage.border,
                stage_bracket: self.raw.stage.bracket,
                caption,
                stage_title: self.raw.stage.title.max(0),
            },
            rects: Rects {
                topbar: Rect::new(0, 0, w, topbar),
                sidebar: Rect::new(0, topbar, sidebar, h - topbar - netmap),
                visualizer: Rect::new(pad, h - netmap - vis - pad, sidebar - 2 * pad, vis),
                stage,
                stage_app,
                terminal: terminal_rect,
                terminal_app,
                netmap: Rect::new(0, h - netmap, w - terminal, netmap),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = include_str!("../../../design/layout.toml");

    #[test]
    fn baseline_matches_generator() {
        let c = Layout::parse(TOML).unwrap().compile(1920, 1080).unwrap();
        assert_eq!(c.rects.topbar, Rect::new(0, 0, 1920, 22));
        assert_eq!(c.rects.sidebar, Rect::new(0, 22, 255, 874));
        assert_eq!(c.rects.stage, Rect::new(255, 22, 933, 874));
        assert_eq!(c.rects.terminal, Rect::new(1188, 22, 732, 1058));
        assert_eq!(c.rects.netmap, Rect::new(0, 896, 1188, 184));
        // App viewports sit below the caption / title chrome (14 + 18 px).
        assert_eq!(c.rects.stage_app.y, 22 + 14 + 18);
        assert_eq!(c.rects.terminal_app.y, 22 + 14);
        assert!(!c.compact);
    }

    #[test]
    fn compact_breakpoint_matches_generator() {
        let c = Layout::parse(TOML).unwrap().compile(1366, 768).unwrap();
        assert!(c.compact);
        assert_eq!(c.rects.sidebar.w, 170);
        assert_eq!(c.rects.terminal.w, 488);
        assert_eq!(c.rects.netmap.h, 136);
        assert_eq!(c.rects.visualizer.w, 146);
    }

    #[test]
    fn rejects_tiny_screens() {
        assert!(Layout::parse(TOML).unwrap().compile(1024, 600).is_err());
    }

    #[test]
    fn regions_tile_without_overlap() {
        let c = Layout::parse(TOML).unwrap().compile(2560, 1440).unwrap();
        let r = &c.rects;
        // stage and terminal never overlap
        assert!(r.stage.right() <= r.terminal.x);
        // stage and netmap never overlap
        assert!(r.stage.bottom() <= r.netmap.y);
        // sidebar and stage share an edge
        assert_eq!(r.sidebar.right(), r.stage.x);
        // visualizer stays inside the sidebar column
        assert!(r.visualizer.right() <= r.sidebar.right());
        assert!(r.visualizer.bottom() <= r.sidebar.bottom());
    }
}
