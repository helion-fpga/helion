//! Schematic canvas: live engine netlist gates/wires, hierarchy, LOD, cross-probe.
//!
//! New Rune `El` code written from scratch for the Vivado brief: the
//! Elaborated/RTL Schematic viewer — a white canvas with a dotted grid, pale
//! yellow primitive cells (LUT/FDRE), pale blue hierarchy blocks, green net
//! wires, and red selection. Gates, wires, ports, and hierarchy blocks all
//! come from the live engine design ([`crate::live::LiveNetlist`]): the
//! `tcl.design` elaborated by `read_sv`/`synth_design`, or the design a
//! `launch_runs` worker synthesized. Nothing here is a static demo netlist:
//! with no elaborated design the sheet stays visibly empty with a reason.
//!
//! Behavior: hierarchy expand/collapse over the real instance tree (canvas
//! block and tree row stay in sync through
//! [`SchematicState::collapsed`]; collapsing a block re-anchors its wires to
//! the block outline), three zoom-derived LOD levels ([`Lod`]) over the real
//! hierarchy, zoom in/out/fit toolbar, pan via the scroll container, and
//! cross-probe selection shared between canvas cells, hierarchy rows, the
//! cell-properties strip, and the Sources tree (`Locate in Sources` selects
//! the file and echoes the Tcl verb, like the timing delay links).
//!
//! All colors come from [`VivadoTheme`]; no manual geometry beyond the
//! deterministic column layout ([`SchemLayout`]) scaled by the zoom factor.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()/canvas()`.
//! - `crates/rune/src/shape.rs`: `Shape::new/stroke/line/fill`, `LineCap`.
//! - `crates/rune/src/el/layout.rs`: `.absolute()` overlay nodes, `.pin()`.
//! - `crates/rune/src/el/scroll.rs`: `.scroll_xy()` pan container.
//! - `crates/rune/src/el/handlers.rs`: `.on_press()` selection dispatch.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_RIGHT/DOWN` twisties.

use std::collections::{HashMap, HashSet};

use rune::tok::TextStyle;
use rune::{El, Shape, canvas, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::live::{LiveNetlist, live_netlist, slug};
use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::schematic_cone::{self as cone, ConeTrace, SheetFind};
use crate::views::sources::pane_header;

/// Sheet width in points at 100% (columns are fixed; height grows with the
/// live netlist).
pub const WORLD_W: f32 = 660.0;
/// Minimum sheet height; the layout extends it for large netlists.
pub const WORLD_H_MIN: f32 = 300.0;
/// Hierarchy tree width.
const HIER_W: f32 = 188.0;
/// Tree row height (compact CAD density).
const ROW_H: f32 = 20.0;
/// Tree indent per depth level.
const INDENT: f32 = 14.0;
/// Toolbar height.
const TOOL_H: f32 = 26.0;
/// Grid dot pitch in world points.
const GRID: f32 = 20.0;
/// Zoom step per in/out press.
const ZOOM_STEP: f32 = 1.25;
/// Zoom limits.
pub const ZOOM_MIN: f32 = 0.25;
/// Zoom limits.
pub const ZOOM_MAX: f32 = 4.0;
/// Input-port column.
const PORT_IN_X: f32 = 8.0;
/// Port node width.
const PORT_W: f32 = 84.0;
/// Port node height.
const PORT_H: f32 = 24.0;
/// Port row pitch.
const PORT_PITCH: f32 = 34.0;
/// Hierarchy-block column.
const BLOCK_X: f32 = 110.0;
/// Hierarchy-block width.
const BLOCK_W: f32 = 190.0;
/// Block header height (name strip above the child grid).
const BLOCK_HEAD: f32 = 26.0;
/// Child mini-cell size inside an expanded block.
const CHILD_W: f32 = 88.0;
const CHILD_H: f32 = 24.0;
/// Top-level kind columns (LUT / FF / other).
const KIND_X: [f32; 3] = [110.0, 225.0, 340.0];
const KIND_W: f32 = 105.0;
const CELL_H: f32 = 28.0;
const CELL_PITCH: f32 = 34.0;

/// Pin/net label type: smaller than CAD body text.
const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

/// Cross-probed schematic selection: a real cell, port, or hierarchy block.
/// Names are engine names, never display strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemSel {
    Cell(String),
    Port(String),
    Hier(String),
    /// A logical net (find, cone rows); nets have no canvas block.
    Net(String),
}

impl SchemSel {
    pub fn name(&self) -> &str {
        match self {
            SchemSel::Cell(n) | SchemSel::Port(n) | SchemSel::Hier(n) | SchemSel::Net(n) => n,
        }
    }

    pub fn key(&self) -> String {
        match self {
            SchemSel::Net(n) => format!("schematic-net-{}", slug(n)),
            _ => format!("schematic-cell-{}", slug(self.name())),
        }
    }

    pub fn hier_key(&self) -> String {
        format!("schematic-hier-{}", slug(self.name()))
    }

    /// Tcl verb echo for the console, mirroring the timing links.
    pub fn tcl(&self) -> String {
        match self {
            SchemSel::Cell(n) => format!("select_objects [get_cells {n}]"),
            SchemSel::Port(n) => format!("select_objects [get_ports {n}]"),
            SchemSel::Hier(n) => format!("select_objects [get_cells {n}]"),
            SchemSel::Net(n) => format!("select_objects [get_nets {n}]"),
        }
    }
}

/// Net visibility class: backbones survive the coarsest LOD, detail nets drop
/// out, internal nets show only while their block is expanded at Full LOD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetLevel {
    Backbone,
    Detail,
    Internal(String),
}

/// One laid-out wire: orthogonal world-space points plus the endpoint names
/// (used for selection highlight and endpoint-visibility filtering).
#[derive(Clone, Debug)]
pub struct SchemWire {
    pub name: String,
    pub pts: Vec<[f32; 2]>,
    pub level: NetLevel,
    pub ends: (String, String),
}

/// Deterministic column layout of a live netlist: rects for every port,
/// hierarchy block, and cell, plus one wire star per net. Rebuilt per frame
/// from the session design (cheap); never stored.
#[derive(Clone, Debug, Default)]
pub struct SchemLayout {
    /// World height for this netlist (width is [`WORLD_W`]).
    pub world_h: f32,
    /// Cell name → world rect.
    pub cells: HashMap<String, [f32; 4]>,
    /// Port name → world rect.
    pub ports: HashMap<String, [f32; 4]>,
    /// Instance name → world rect.
    pub blocks: HashMap<String, [f32; 4]>,
    /// One star leg per net sink.
    pub wires: Vec<SchemWire>,
}

impl SchemLayout {
    /// Column layout of a live netlist. Collapsed blocks shrink to a header
    /// row (their children unmount, so they get no rects); everything below
    /// reflows, so collapse never leaves a void.
    pub fn of(live: &LiveNetlist, st: &SchematicState) -> Self {
        let mut out = SchemLayout {
            world_h: WORLD_H_MIN,
            ..SchemLayout::default()
        };
        // Input ports, left column.
        for (i, p) in live.ports_in.iter().enumerate() {
            out.ports.insert(
                p.clone(),
                [PORT_IN_X, 40.0 + i as f32 * PORT_PITCH, PORT_W, PORT_H],
            );
        }
        // Hierarchy blocks, stacked; expanded children in a 2-wide mini-grid.
        let mut y = 24.0;
        for h in &live.hier {
            if !st.is_expanded(&h.name) {
                out.blocks
                    .insert(h.name.clone(), [BLOCK_X, y, BLOCK_W, BLOCK_HEAD]);
                y += BLOCK_HEAD + 12.0;
                continue;
            }
            let kids: Vec<&str> = live
                .cells
                .iter()
                .map(|c| c.name.as_str())
                .filter(|c| live.owner_of(c) == Some(h.name.as_str()))
                .collect();
            let rows = kids.len().div_ceil(2).max(1) as f32;
            let bh = BLOCK_HEAD + rows * (CHILD_H + 6.0) + 4.0;
            out.blocks.insert(h.name.clone(), [BLOCK_X, y, BLOCK_W, bh]);
            for (i, kid) in kids.iter().enumerate() {
                let cx = BLOCK_X + 6.0 + (i % 2) as f32 * (CHILD_W + 6.0);
                let cy = y + BLOCK_HEAD + (i / 2) as f32 * (CHILD_H + 6.0);
                out.cells
                    .insert((*kid).to_string(), [cx, cy, CHILD_W, CHILD_H]);
            }
            y += bh + 12.0;
        }
        // Top-level cells in kind columns below the blocks.
        let top_y = if live.hier.is_empty() { 24.0 } else { y };
        let mut col_y = [top_y; 3];
        for cell in &live.cells {
            if live.owner_of(&cell.name).is_some() {
                continue;
            }
            let col = match cell.kind {
                crate::live::LiveKind::Lut => 0,
                crate::live::LiveKind::Ff => 1,
                _ => 2,
            };
            out.cells
                .insert(cell.name.clone(), [KIND_X[col], col_y[col], KIND_W, CELL_H]);
            col_y[col] += CELL_PITCH;
            y = y.max(col_y[col] + 12.0);
        }
        // Output ports, right column.
        for (i, p) in live.ports_out.iter().enumerate() {
            out.ports.insert(
                p.clone(),
                [
                    WORLD_W - PORT_W - 10.0,
                    40.0 + i as f32 * PORT_PITCH,
                    PORT_W,
                    PORT_H,
                ],
            );
        }
        // One star leg per sink: the first sorted endpoint drives (ports
        // drive when they are inputs, sink when they are outputs).
        for net in &live.nets {
            let in_port = live.ports_in.iter().find(|p| *p == &net.name).cloned();
            let out_port = live.ports_out.iter().find(|p| *p == &net.name).cloned();
            let mut ends: Vec<String> = net.cells.clone();
            if in_port.is_some() {
                ends.insert(0, net.name.clone());
            }
            if out_port.is_some() {
                ends.push(net.name.clone());
            }
            if ends.len() < 2 {
                continue;
            }
            let level = if in_port.is_some() || out_port.is_some() || net.loads >= 8 {
                NetLevel::Backbone
            } else {
                let owners: HashSet<&str> = ends.iter().filter_map(|e| live.owner_of(e)).collect();
                if owners.len() == 1
                    && !ends
                        .iter()
                        .any(|e| live.ports_in.contains(e) || live.ports_out.contains(e))
                {
                    NetLevel::Internal(owners.into_iter().next().unwrap().to_string())
                } else {
                    NetLevel::Detail
                }
            };
            let (driver, sinks) = (&ends[0], &ends[1..]);
            for sink in sinks {
                let (Some(a), Some(b)) = (Self::anchor(&out, driver), Self::anchor(&out, sink))
                else {
                    continue;
                };
                let x0 = a[0] + a[2];
                let y0 = a[1] + a[3] / 2.0;
                let x1 = b[0];
                let y1 = b[1] + b[3] / 2.0;
                let mx = (x0 + x1) / 2.0;
                out.wires.push(SchemWire {
                    name: net.name.clone(),
                    pts: vec![[x0, y0], [mx, y0], [mx, y1], [x1, y1]],
                    level: level.clone(),
                    ends: (driver.clone(), sink.clone()),
                });
            }
        }
        // Sheet height covers the lowest rect.
        let mut bottom = WORLD_H_MIN - 24.0;
        for r in out
            .cells
            .values()
            .chain(out.ports.values())
            .chain(out.blocks.values())
        {
            bottom = bottom.max(r[1] + r[3]);
        }
        out.world_h = bottom + 24.0;
        out
    }

    fn anchor(layout: &SchemLayout, name: &str) -> Option<[f32; 4]> {
        layout
            .cells
            .get(name)
            .or_else(|| layout.ports.get(name))
            .copied()
    }

    /// Mounted rect for a wire endpoint: the cell/port rect when it mounts,
    /// else the owner block outline (collapse re-anchors wires to the block).
    pub fn end_rect(
        &self,
        st: &SchematicState,
        live: &LiveNetlist,
        name: &str,
    ) -> Option<[f32; 4]> {
        if let Some(r) = self.ports.get(name) {
            return Some(*r);
        }
        if st.cell_visible(live, name) {
            if let Some(r) = self.cells.get(name) {
                return Some(*r);
            }
        }
        let owner = live.owner_of(name)?;
        self.blocks.get(owner).copied()
    }
}

/// Level of detail, derived from the zoom factor (never stored separately).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lod {
    /// Zoomed out: hierarchy blocks and ports only, backbone nets.
    Blocks,
    /// Mid zoom: every cell with names, all external nets.
    #[default]
    Symbols,
    /// Close zoom: names, kinds, pin dots.
    Full,
}

impl Lod {
    pub fn label(self) -> &'static str {
        match self {
            Lod::Blocks => "Blocks",
            Lod::Symbols => "Symbols",
            Lod::Full => "Full",
        }
    }

    pub fn of(zoom: f32) -> Self {
        if zoom < 0.45 {
            Lod::Blocks
        } else if zoom < 0.9 {
            Lod::Symbols
        } else {
            Lod::Full
        }
    }
}

/// Schematic-owned UI state: zoom, shared cross-probe selection, the
/// collapsed real instance names (empty = all expanded), the active cone,
/// selection history, the measured canvas viewport, and sheet find.
#[derive(Clone, Debug)]
pub struct SchematicState {
    pub zoom: f32,
    pub selected: Option<SchemSel>,
    pub collapsed: HashSet<String>,
    /// Last traced fan-in/fan-out cone (`None` = no cone highlight).
    pub cone: Option<ConeTrace>,
    /// Hop count the next cone traces with.
    pub cone_hops: usize,
    /// Stop cone traversal at FF boundaries.
    pub stop_at_ff: bool,
    /// Selection history (Previous/Next) and the current entry.
    pub history: Vec<SchemSel>,
    pub history_ix: usize,
    /// Canvas viewport size in points (from `on_layout`; Fit reads it).
    pub viewport: [f32; 2],
    /// Sheet find query, scope, and the next hit index.
    pub find: rune::TextEdit,
    pub find_kind: SheetFind,
    pub find_ix: usize,
    /// Query `find_ix` belongs to (a new query restarts at the first hit).
    pub find_last: String,
}

impl Default for SchematicState {
    fn default() -> Self {
        Self::new()
    }
}

impl SchematicState {
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            selected: None,
            collapsed: HashSet::new(),
            cone: None,
            cone_hops: 2,
            stop_at_ff: false,
            history: Vec::new(),
            history_ix: 0,
            viewport: [0.0, 0.0],
            find: rune::TextEdit::new(""),
            find_kind: SheetFind::Cells,
            find_ix: 0,
            find_last: String::new(),
        }
    }

    pub fn lod(&self) -> Lod {
        Lod::of(self.zoom)
    }

    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * ZOOM_STEP).min(ZOOM_MAX);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / ZOOM_STEP).max(ZOOM_MIN);
    }

    pub fn zoom_label(&self) -> String {
        format!("{}%", (self.zoom * 100.0).round())
    }

    pub fn toggle(&mut self, hier: &str) {
        if !self.collapsed.remove(hier) {
            self.collapsed.insert(hier.to_string());
        }
    }

    pub fn set_all(&mut self, live: &LiveNetlist, expanded: bool) {
        self.collapsed.clear();
        if !expanded {
            self.collapsed
                .extend(live.hier.iter().map(|h| h.name.clone()));
        }
    }

    pub fn is_expanded(&self, hier: &str) -> bool {
        !self.collapsed.contains(hier)
    }

    /// Whether a live cell's canvas node mounts at the current LOD/expand
    /// state. Blocks hides owned cells (their block stands in); top-level
    /// cells are the top of the hierarchy and stay at every LOD.
    pub fn cell_visible(&self, live: &LiveNetlist, name: &str) -> bool {
        if let Some(owner) = live.owner_of(name) {
            if !self.is_expanded(owner) {
                return false;
            }
            if self.lod() == Lod::Blocks {
                return false;
            }
            return true;
        }
        let _ = live;
        true
    }

    /// Whether a wire leg draws: its level must pass the LOD, its internal
    /// block (if any) must be expanded at Full, and both endpoints must have
    /// a mounted rect (cells re-anchor to their block when collapsed).
    pub fn wire_visible(&self, live: &LiveNetlist, layout: &SchemLayout, wire: &SchemWire) -> bool {
        let level_ok = match &wire.level {
            NetLevel::Backbone => true,
            NetLevel::Detail => self.lod() != Lod::Blocks,
            NetLevel::Internal(h) => self.is_expanded(h) && self.lod() == Lod::Full,
        };
        level_ok
            && layout.end_rect(self, live, &wire.ends.0).is_some()
            && layout.end_rect(self, live, &wire.ends.1).is_some()
    }
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

/// Select a live object, record it in the selection history, and
/// cross-probe it to the console like the timing links do.
pub(crate) fn select(s: &mut AppState, sel: SchemSel) {
    let verb = sel.tcl();
    s.schematic.push_history(&sel);
    s.schematic.selected = Some(sel);
    echo(s, verb);
}

/// Select a hierarchy block and flip its expand state (tree and canvas share
/// this so the two never disagree).
fn toggle_hier(s: &mut AppState, hier: String) {
    s.schematic.toggle(&hier);
    select(s, SchemSel::Hier(hier));
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Small toolbar button with a text glyph (matches the Sources toolbar style).
fn tool_btn(
    key: &'static str,
    label: &'static str,
    glyph_text: &'static str,
    t: &VivadoTheme,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .size(22.0, 22.0)
        .center()
        .hover(t.selection)
        .child(
            text(glyph_text)
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text_dim)
                .a11y_hidden(),
        )
}

/// Zoom/LOD/expand toolbar over the canvas.
fn schematic_toolbar(s: &AppState, t: &VivadoTheme, live: Option<&LiveNetlist>) -> El {
    row()
        .key("schematic-toolbar")
        .role("toolbar")
        .w_full()
        .h(TOOL_H)
        .items_center()
        .gap(2.0)
        .px(6.0)
        .fill(t.panel_bg)
        .child(
            tool_btn("schematic-zoom-out", "Zoom out", "−", t)
                .on_press(|s: &mut AppState| s.schematic.zoom_out()),
        )
        .child(
            el().key("schematic-zoom-label")
                .role("status")
                .label(format!("Zoom {}", s.schematic.zoom_label()))
                .w(44.0)
                .center()
                .child(ui_label(&s.schematic.zoom_label(), t)),
        )
        .child(
            tool_btn("schematic-zoom-in", "Zoom in", "+", t)
                .on_press(|s: &mut AppState| s.schematic.zoom_in()),
        )
        .child(
            el().key("schematic-zoom-fit")
                .role("button")
                .label("Zoom to fit")
                .focusable(true)
                .h(22.0)
                .px(6.0)
                .center()
                .hover(t.selection)
                .on_press_tx(|s: &mut AppState, tx: &mut rune::Tx| {
                    crate::tcl::eval_command(s, "schematic_zoom_fit");
                    tx.scroll_to_instant("schematic-canvas", 0.0, 0.0);
                })
                .child(ui_label("Fit", t)),
        )
        .child(
            el().key("schematic-lod")
                .role("status")
                .label(format!("Detail level {}", s.schematic.lod().label()))
                .h(22.0)
                .px(6.0)
                .center()
                .child(dim_label(&format!("LOD: {}", s.schematic.lod().label()), t)),
        )
        .child(
            el().key("schematic-expand-all")
                .role("button")
                .label("Expand all hierarchy")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| {
                    if let Some(live) = live_netlist(s) {
                        s.schematic.set_all(&live, true);
                    }
                })
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 13.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
        .child(
            el().key("schematic-collapse-all")
                .role("button")
                .label("Collapse all hierarchy")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| {
                    if let Some(live) = live_netlist(s) {
                        s.schematic.set_all(&live, false);
                    }
                })
                .child(
                    icon(&glyphs::CHEVRON_UP, 13.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
        .child(cone::history_buttons(s, t))
        .child(el().w(6.0))
        .child(cone::find_box(s, t, live))
        .child(el().grow(1.0))
        .child(
            el().key("schematic-locate")
                .role("button")
                .label("Locate selection in Sources")
                .focusable(true)
                .h(22.0)
                .px(6.0)
                .center_y()
                .hover(t.selection)
                .on_press(|s: &mut AppState| {
                    crate::views::sources::locate_design_top(s);
                })
                .child(
                    text("Locate in Sources")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.link),
                ),
        )
}

/// One hierarchy tree row: twisty for blocks, selection wash, tap to
/// cross-probe (blocks also expand/collapse).
fn hier_row(s: &AppState, t: &VivadoTheme, sel: SchemSel, detail: &str, depth: u32) -> El {
    let selected = s.schematic.selected.as_ref() == Some(&sel);
    let is_hier = matches!(sel, SchemSel::Hier(_));
    let collapsed = match &sel {
        SchemSel::Hier(h) => !s.schematic.is_expanded(h),
        _ => true,
    };
    let label = if detail.is_empty() {
        sel.name().to_string()
    } else {
        format!("{} ({detail})", sel.name())
    };
    let mut r = row()
        .key(sel.hier_key())
        .role("treeitem")
        .label(label.clone())
        .level(depth + 1)
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(4.0)
        .pl(6.0 + depth as f32 * INDENT)
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .child(
            icon(
                if is_hier && collapsed {
                    &glyphs::TRIANGLE_RIGHT
                } else {
                    &glyphs::TRIANGLE_DOWN
                },
                9.0,
            )
            .color(if is_hier {
                t.text_dim
            } else if selected {
                t.selection
            } else {
                t.panel_bg
            })
            .a11y_hidden(),
        );
    if is_hier {
        r = r
            .expanded(!collapsed)
            .on_press(move |s: &mut AppState| toggle_hier(s, sel.name().to_string()))
            .child(ui_label(&label, t));
    } else {
        r = r
            .on_press(move |s: &mut AppState| select(s, sel.clone()))
            .child(ui_label(&label, t));
    }
    r
}

/// Hierarchy navigator over the real instance tree: blocks with children,
/// top-level cells, then ports.
fn hierarchy_tree(s: &AppState, t: &VivadoTheme, live: &LiveNetlist) -> El {
    let mut body = col()
        .key("schematic-hier")
        .role("tree")
        .w(HIER_W)
        .h_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll_xy();
    for h in &live.hier {
        body = body.child(hier_row(
            s,
            t,
            SchemSel::Hier(h.name.clone()),
            &format!("{} · {}", h.module, h.cells),
            0,
        ));
        if s.schematic.is_expanded(&h.name) {
            for cell in live
                .cells
                .iter()
                .filter(|c| live.owner_of(&c.name).as_deref() == Some(h.name.as_str()))
            {
                body = body.child(hier_row(s, t, SchemSel::Cell(cell.name.clone()), "", 1));
            }
        }
    }
    for cell in live
        .cells
        .iter()
        .filter(|c| live.owner_of(&c.name).is_none())
    {
        body = body.child(hier_row(s, t, SchemSel::Cell(cell.name.clone()), "", 0));
    }
    for p in live.ports_in.iter().chain(live.ports_out.iter()) {
        body = body.child(hier_row(s, t, SchemSel::Port(p.clone()), "", 0));
    }
    body
}

/// Wire/highlight shape for the canvas: dotted grid, sheet frame, net wires
/// (red when touching the selection), and pin dots at Full LOD.
fn wires_shape(s: &AppState, t: &VivadoTheme, live: &LiveNetlist, layout: &SchemLayout) -> Shape {
    let z = s.schematic.zoom;
    let world_h = layout.world_h;
    let px = |x: f32, y: f32| [x * z, y * z];
    let mut shape = Shape::new();
    // Dotted grid.
    let mut gx = GRID;
    while gx < WORLD_W {
        let mut gy = GRID;
        while gy < world_h {
            let [dx, dy] = px(gx, gy);
            shape = shape.fill(
                vec![
                    [dx, dy],
                    [dx + 1.5, dy],
                    [dx + 1.5, dy + 1.5],
                    [dx, dy + 1.5],
                ],
                t.border_light,
            );
            gy += GRID;
        }
        gx += GRID;
    }
    // Sheet frame.
    let [fw, fh] = px(WORLD_W, world_h);
    shape = shape.stroke_with(
        vec![
            [0.5, 0.5],
            [fw - 0.5, 0.5],
            [fw - 0.5, fh - 0.5],
            [0.5, fh - 0.5],
        ],
        true,
        1.0,
        rune::LineCap::Butt,
        t.border,
    );
    // Wires, re-anchored through the live visibility rules.
    for wire in &layout.wires {
        if !s.schematic.wire_visible(live, layout, wire) {
            continue;
        }
        let in_cone = s.schematic.cone.as_ref().map(|c| c.has_net(&wire.name));
        let hot = s.schematic.selected.as_ref().is_some_and(|sel| {
            matches!(sel, SchemSel::Net(n) if *n == wire.name)
                || (!matches!(sel, SchemSel::Net(_))
                    && (wire.ends.0 == *sel.name() || wire.ends.1 == *sel.name()))
                || matches!(sel, SchemSel::Hier(h) if live.owner_of(&wire.ends.0).as_deref() == Some(h.as_str()) || live.owner_of(&wire.ends.1).as_deref() == Some(h.as_str()))
        });
        let (Some(a), Some(b)) = (
            layout.end_rect(&s.schematic, live, &wire.ends.0),
            layout.end_rect(&s.schematic, live, &wire.ends.1),
        ) else {
            continue;
        };
        let x0 = a[0] + a[2];
        let y0 = a[1] + a[3] / 2.0;
        let x1 = b[0];
        let y1 = b[1] + b[3] / 2.0;
        let mut mx = (x0 + x1) / 2.0;
        // Keep the vertical drop out of block interiors (wires paint under
        // the nodes, so a drop inside a foreign block reads as a stub).
        // Legs internal to a block keep their midpoint.
        for r in layout.blocks.values() {
            let inside = |p: f32| p > r[0] + 2.0 && p < r[0] + r[2] - 2.0;
            let a_in = inside(a[0] + a[2] / 2.0);
            let b_in = inside(b[0] + b[2] / 2.0);
            if inside(mx) && !(a_in && b_in) {
                mx = r[0] + r[2] + 8.0;
            }
        }
        let pts = vec![px(x0, y0), px(mx, y0), px(mx, y1), px(x1, y1)];
        let (width, ink) = match (hot, in_cone) {
            (true, _) => (2.0, t.fail),
            (false, Some(true)) => (2.5, t.accent),
            (false, Some(false)) => (1.0, t.pass.alpha(0.3)),
            (false, None) => (1.5, t.pass),
        };
        shape = shape.stroke(pts.clone(), width, ink);
        if s.schematic.lod() == Lod::Full {
            for [ex, ey] in [pts[0], pts[pts.len() - 1]] {
                shape = shape.fill(
                    vec![
                        [ex - 2.0, ey - 2.0],
                        [ex + 2.0, ey - 2.0],
                        [ex + 2.0, ey + 2.0],
                        [ex - 2.0, ey + 2.0],
                    ],
                    t.text,
                );
            }
        }
    }
    shape
}

/// One canvas node (cell, port, or hierarchy block): an absolutely positioned
/// keyed block whose label density follows the LOD.
fn cell_node(
    s: &AppState,
    t: &VivadoTheme,
    live: &LiveNetlist,
    sel: SchemSel,
    rect: [f32; 4],
) -> El {
    let z = s.schematic.zoom;
    let [x, y, w, h] = rect;
    let selected = s.schematic.selected.as_ref() == Some(&sel);
    let lod = s.schematic.lod();
    let is_hier = matches!(sel, SchemSel::Hier(_));
    let is_port = matches!(sel, SchemSel::Port(_));
    let expanded_block = match &sel {
        SchemSel::Hier(h) => s.schematic.is_expanded(h),
        _ => false,
    };
    let kind_label = match &sel {
        SchemSel::Cell(n) => live.cell(n).map(|c| c.kind.label()).unwrap_or("Cell"),
        SchemSel::Port(n) => {
            if live.ports_in.contains(n) {
                "Input port"
            } else {
                "Output port"
            }
        }
        SchemSel::Hier(_) => "Hierarchy",
        SchemSel::Net(_) => "Net",
    };
    // Cone membership: members get the accent outline, the rest recede.
    let cone_state = s.schematic.cone.as_ref().and_then(|c| match &sel {
        SchemSel::Cell(n) => Some((c.has_cell(n), c.root == *n)),
        SchemSel::Port(n) => Some((c.has_net(n), c.root == *n)),
        _ => None,
    });
    let fill = if expanded_block || is_port {
        t.panel_bg
    } else if is_hier {
        t.selection
    } else {
        t.current_line
    };
    let mut node = el()
        .key(sel.key())
        .role("button")
        .label(format!("{} ({kind_label})", sel.name()))
        .focus_selected(selected)
        .focusable(true)
        .absolute(x * z, y * z)
        .size(w * z, h * z)
        .fill(fill)
        .border(
            match (selected, cone_state) {
                (true, _) => 2.0,
                (false, Some((true, true))) => 2.5,
                (false, Some((true, false))) => 2.0,
                _ => 1.0,
            },
            match (selected, cone_state) {
                (true, _) => t.fail,
                (false, Some((true, _))) => t.accent,
                _ => t.text_dim,
            },
        );
    if matches!(cone_state, Some((false, _))) && !selected {
        node = node.layer_opacity(0.45);
    }
    if expanded_block {
        node = node.border_dash(4.0, 3.0);
    }
    let tap = sel.clone();
    let name = sel.name().to_string();
    if is_hier {
        let hier = name.clone();
        node = node.on_press(move |s: &mut AppState| toggle_hier(s, hier.clone()));
    } else {
        node = node.on_press(move |s: &mut AppState| select(s, tap.clone()));
    }
    // Label density follows the LOD; small child cells only ever show a name.
    // Labels stay inside the box (long hierarchical names truncate in the
    // middle, full name on hover).
    let small = w < 100.0;
    let label_w = (w * z - 8.0).max(0.0);
    if expanded_block {
        node = node.child(
            text(name.as_str())
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .absolute(5.0 * z, 2.0 * z)
                .max_w(label_w)
                .truncate_middle()
                .help_when_truncated(name.clone()),
        );
    } else if small || lod != Lod::Full {
        node = node.child(
            col().w_full().h_full().center().child(
                text(name.as_str())
                    .font(if small {
                        FONT_PIN
                    } else {
                        VivadoTheme::FONT_UI
                    })
                    .color(t.text)
                    .max_w(label_w)
                    .truncate_middle()
                    .help_when_truncated(name.clone()),
            ),
        );
    } else {
        node = node.child(
            col()
                .w_full()
                .h_full()
                .center()
                .child(
                    text(name.as_str())
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .max_w(label_w)
                        .truncate_middle()
                        .help_when_truncated(name.clone()),
                )
                .child(text(kind_label).font(FONT_PIN).color(t.text_dim)),
        );
    }
    node
}

/// The pan/zoom canvas: a scroll container over the zoomed sheet with the
/// wire shape painted under the live nodes. Pressing empty sheet space
/// clears the cross-probe selection.
fn schematic_canvas(s: &AppState, t: &VivadoTheme, live: &LiveNetlist, layout: &SchemLayout) -> El {
    let z = s.schematic.zoom;
    let mut sheet = el()
        .w(WORLD_W * z)
        .h(layout.world_h * z)
        .fill(t.panel_bg)
        .on_press(|s: &mut AppState| s.schematic.selected = None)
        .child(canvas(wires_shape(s, t, live, layout)).w_full().h_full());
    // Blocks first (under their children), then ports, then mounted cells.
    for h in &live.hier {
        if let Some(rect) = layout.blocks.get(&h.name) {
            sheet = sheet.child(cell_node(s, t, live, SchemSel::Hier(h.name.clone()), *rect));
        }
    }
    for (name, rect) in &layout.ports {
        sheet = sheet.child(cell_node(s, t, live, SchemSel::Port(name.clone()), *rect));
    }
    let mut names: Vec<&String> = layout.cells.keys().collect();
    names.sort();
    for name in names {
        if s.schematic.cell_visible(live, name) {
            sheet = sheet.child(cell_node(
                s,
                t,
                live,
                SchemSel::Cell(name.clone()),
                layout.cells[name],
            ));
        }
    }
    col()
        .key("schematic-canvas")
        .role("application")
        .label("RTL schematic canvas")
        .items_start()
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll_xy()
        .on_layout(|s: &mut AppState, r: [f32; 4]| {
            let v = [r[2], r[3]];
            let old = s.schematic.viewport;
            if (v[0] - old[0]).abs() > 0.5 || (v[1] - old[1]).abs() > 0.5 {
                s.schematic.viewport = v;
            }
        })
        .child(sheet)
}

/// Visibly-disabled sheet: shown instead of tree+canvas until a real design
/// exists, with the reason and the verbs that produce one.
fn schematic_empty(t: &VivadoTheme) -> El {
    col()
        .key("schematic-empty")
        .role("status")
        .grow(1.0)
        .h_full()
        .center()
        .gap(6.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            text("No elaborated design")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(dim_label(
            "Run launch_runs synth_1, synth_design, or read_sv to elaborate RTL.",
            t,
        ))
}

/// Cell-properties strip: the cross-probed selection readout over live data.
fn schematic_props(s: &AppState, t: &VivadoTheme, live: Option<&LiveNetlist>) -> El {
    let mut strip = row()
        .key("schematic-props")
        .role("status")
        .w_full()
        .h(22.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.header_bg);
    let Some(live) = live else {
        return strip.child(dim_label("No elaborated design — schematic is empty", t));
    };
    match &s.schematic.selected {
        Some(sel) => {
            let (kind, fanout, source) = match sel {
                SchemSel::Cell(n) => (
                    live.cell(n)
                        .map(|c| c.kind.label().to_string())
                        .unwrap_or_else(|| "Cell".into()),
                    live.cell(n)
                        .map(|c| format!("fanout {}", c.fanout))
                        .unwrap_or_default(),
                    live.design.clone(),
                ),
                SchemSel::Port(n) => (
                    if live.ports_in.contains(n) {
                        "Input port".into()
                    } else {
                        "Output port".into()
                    },
                    live.net(n)
                        .map(|w| format!("{} endpoints", w.loads))
                        .unwrap_or_default(),
                    live.design.clone(),
                ),
                SchemSel::Hier(n) => (
                    "Hierarchy".to_string(),
                    live.hier_of(n)
                        .map(|h| format!("{} cells · {} conns", h.cells, h.conns))
                        .unwrap_or_default(),
                    live.hier_of(n)
                        .map(|h| h.module.clone())
                        .unwrap_or_else(|| live.design.clone()),
                ),
                SchemSel::Net(n) => (
                    "Net".to_string(),
                    live.net(n)
                        .map(|w| format!("{} endpoints", w.loads))
                        .unwrap_or_else(|| "stale selection (design changed)".into()),
                    live.owner_of(n)
                        .map(str::to_string)
                        .unwrap_or_else(|| live.design.clone()),
                ),
            };
            strip = strip
                .child(
                    text(sel.name())
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .key("schematic-props-name"),
                )
                .child(dim_label(&kind, t).key("schematic-props-kind"))
                .child(dim_label(&fanout, t).key("schematic-props-fanout"))
                .child(dim_label(&source, t).key("schematic-props-source"));
        }
        None => {
            strip = strip.child(dim_label(
                &format!(
                    "{} · {} cells · {} nets — click a cell or hierarchy row",
                    live.design,
                    live.cells.len(),
                    live.nets.len()
                ),
                t,
            ));
        }
    }
    strip.child(el().grow(1.0)).child(cone::debug_actions(s, t, live))
}

/// The center Schematic pane wired into the shell.
pub fn schematic_pane(s: &AppState, t: &VivadoTheme) -> El {
    let live = live_netlist(s);
    let mut body = row().w_full().grow(1.0).min_h(0.0).gap(6.0).padding(6.0);
    let mut results = None;
    if let Some(live) = &live {
        let layout = SchemLayout::of(live, &s.schematic);
        body = body
            .child(hierarchy_tree(s, t, live))
            .child(schematic_canvas(s, t, live, &layout));
        results = cone::cone_results(s, t, live);
    } else {
        body = body.child(schematic_empty(t));
    }
    let mut pane = col()
        .key("schematic")
        .grow(1.0)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Schematic", "schematic-pane", t))
        .child(schematic_toolbar(s, t, live.as_deref()))
        .child(cone::cone_toolbar(s, t, live.as_deref()))
        .child(body);
    if let Some(r) = results {
        pane = pane.child(el().w_full().px(6.0).pb(6.0).child(r));
    }
    pane.child(schematic_props(s, t, live.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn schem_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Schematic;
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        s
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn lod_thresholds_follow_zoom() {
        assert_eq!(Lod::of(0.25), Lod::Blocks);
        assert_eq!(Lod::of(0.44), Lod::Blocks);
        assert_eq!(Lod::of(0.45), Lod::Symbols);
        assert_eq!(Lod::of(0.89), Lod::Symbols);
        assert_eq!(Lod::of(0.9), Lod::Full);
        assert_eq!(Lod::of(4.0), Lod::Full);
    }

    #[test]
    fn collapsed_blocks_hide_children_and_their_nets() {
        let mut s = init();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let live = live_netlist(&s).unwrap();
        let mut st = SchematicState::new();
        assert!(st.cell_visible(&live, "u_div_u_ff0"));
        st.toggle("u_div");
        assert!(!st.cell_visible(&live, "u_div_u_ff0"));
        assert!(st.cell_visible(&live, "u_ff0"), "top-level cells stay");
        st.set_all(&live, true);
        assert!(st.cell_visible(&live, "u_div_u_ff0"));
        st.zoom = 0.25;
        assert!(
            !st.cell_visible(&live, "u_div_u_ff0"),
            "Blocks hides owned cells"
        );
        assert!(
            st.cell_visible(&live, "u_ff0"),
            "Blocks keeps top-level cells"
        );
        let layout = SchemLayout::of(&live, &st);
        assert!(
            layout
                .wires
                .iter()
                .any(|w| matches!(w.level, NetLevel::Backbone))
        );
        assert!(
            layout
                .wires
                .iter()
                .any(|w| matches!(w.level, NetLevel::Detail))
        );
        assert!(
            layout
                .wires
                .iter()
                .any(|w| matches!(w.level, NetLevel::Internal(_)))
        );
    }

    #[test]
    fn layout_covers_every_live_object_deterministically() {
        let mut s = init();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let live = live_netlist(&s).unwrap();
        let st = SchematicState::new();
        let a = SchemLayout::of(&live, &st);
        let b = SchemLayout::of(&live, &st);
        assert_eq!(
            a.cells.len(),
            live.cells.len(),
            "every live cell gets a rect"
        );
        assert_eq!(
            a.ports.len(),
            live.ports_in.len() + live.ports_out.len(),
            "every live port gets a rect"
        );
        assert_eq!(
            a.blocks.len(),
            live.hier.len(),
            "every live instance gets a block"
        );
        assert!(!a.wires.is_empty(), "live nets produce wires");
        assert_eq!(a.cells, b.cells, "layout must be deterministic");
    }

    #[test]
    fn collapsed_blocks_shrink_and_reflow() {
        let mut s = init();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let live = live_netlist(&s).unwrap();
        let mut st = SchematicState::new();
        let open = SchemLayout::of(&live, &st);
        st.toggle("u_led");
        let shut = SchemLayout::of(&live, &st);
        assert!(
            shut.world_h < open.world_h,
            "collapse must reclaim sheet height"
        );
        assert!(
            !shut.cells.contains_key("u_led_u_iob"),
            "collapsed children get no rects"
        );
        assert_eq!(
            shut.blocks["u_led"][3], BLOCK_HEAD,
            "collapsed block is a header row"
        );
        assert!(
            shut.cells.contains_key("u_div_u_ff0"),
            "other blocks keep their rects"
        );
    }

    #[test]
    fn empty_session_shows_the_reason_not_demo_cells() {
        let mut s = init();
        s.center = crate::state::CenterPane::Schematic;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("schematic-empty").is_some(),
            "empty reason must mount"
        );
        assert!(
            rt.find("schematic-hier").is_none(),
            "no tree without a design"
        );
        assert!(
            rt.find("schematic-canvas").is_none(),
            "no canvas without a design"
        );
    }

    #[test]
    fn schematic_tree_exposes_toolbar_hierarchy_canvas_and_props() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        for key in [
            "schematic",
            "schematic-toolbar",
            "schematic-zoom-out",
            "schematic-zoom-label",
            "schematic-zoom-in",
            "schematic-zoom-fit",
            "schematic-lod",
            "schematic-expand-all",
            "schematic-collapse-all",
            "schematic-locate",
            "schematic-hier",
            "schematic-hier-u-div",
            "schematic-hier-u-led",
            "schematic-hier-u-ff0",
            "schematic-hier-clk",
            "schematic-canvas",
            "schematic-cell-clk",
            "schematic-cell-u-div",
            "schematic-cell-u-div-u-ff0",
            "schematic-cell-u-ff0",
            "schematic-cell-u-led",
            "schematic-props",
        ] {
            assert!(rt.find(key).is_some(), "missing schematic key: {key}");
        }
        assert!(
            rt.find("schematic-empty").is_none(),
            "live design must clear the empty state"
        );
        assert!(
            rt.find("schematic-props-name").is_none(),
            "no selection yet"
        );
    }

    #[test]
    fn cell_tap_cross_probes_props_and_console() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        // A first-block child near the sheet origin (visible without scrolling).
        tap(&mut rt, "schematic-cell-u-div-u-ff0");
        assert_eq!(
            rt.state.schematic.selected,
            Some(SchemSel::Cell("u_div_u_ff0".into()))
        );
        assert!(rt.find("schematic-props-name").is_some());
        assert!(rt.find("schematic-props-kind").is_some());
        assert!(rt.find("schematic-props-fanout").is_some());
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "select_objects [get_cells u_div_u_ff0]"),
            "select echoes its Tcl verb"
        );
    }

    #[test]
    fn hierarchy_toggle_expands_and_collapses_canvas_children() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        assert!(
            rt.find("schematic-cell-u-led-u-iob").is_some(),
            "blocks start expanded"
        );
        tap(&mut rt, "schematic-hier-u-led");
        assert_eq!(
            rt.state.schematic.selected,
            Some(SchemSel::Hier("u_led".into()))
        );
        assert!(
            rt.find("schematic-cell-u-led-u-iob").is_none(),
            "collapse unmounts children"
        );
        assert!(
            rt.find("schematic-cell-u-led").is_some(),
            "collapse keeps the block"
        );
        tap(&mut rt, "schematic-hier-u-led");
        assert!(
            rt.find("schematic-cell-u-led-u-iob").is_some(),
            "expand remounts children"
        );
    }

    #[test]
    fn zoom_steps_change_lod() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        tap(&mut rt, "schematic-zoom-in");
        assert!(rt.state.schematic.zoom > 1.0);
        for _ in 0..6 {
            tap(&mut rt, "schematic-zoom-out");
        }
        assert_eq!(rt.state.schematic.lod(), Lod::Blocks);
        assert!(
            rt.find("schematic-cell-u-div-u-ff0").is_none(),
            "Blocks hides owned cells"
        );
        assert!(
            rt.find("schematic-cell-u-div").is_some(),
            "Blocks keeps hierarchy"
        );
        assert!(
            rt.find("schematic-cell-clk").is_some(),
            "Blocks keeps ports"
        );
        for _ in 0..6 {
            tap(&mut rt, "schematic-zoom-in");
        }
        assert_eq!(rt.state.schematic.lod(), Lod::Full);
        assert!(rt.find("schematic-cell-u-div-u-ff0").is_some());
    }

    #[test]
    fn locate_selects_the_file_declaring_the_design_top() {
        use crate::project::{FileSet, ProjectFile};
        let mut s = schem_state();
        s.project_files.add(ProjectFile::in_memory(
            "divclk_gen.sv",
            FileSet::Design,
            crate::live::FIXTURE_SRC,
        ));
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "schematic-cell-u-div-u-ff0");
        tap(&mut rt, "schematic-locate");
        let want = rt.state.project_files.defining("counter_top");
        assert!(want.is_some());
        assert_eq!(rt.state.sources.selected.resolve(&rt.state), want);
        assert!(rt.state.layout.show_sources, "locate reveals Sources");
    }

    #[test]
    fn locate_says_so_when_no_project_file_declares_the_top() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        tap(&mut rt, "schematic-locate");
        assert!(
            rt.state
                .toasts
                .items
                .iter()
                .any(|t| t.title == "Not in project sources"),
            "never pretend a different file is the source"
        );
    }

    #[test]
    fn harness_smoke() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        assert!(rt.find("shell").is_some());
    }
}
