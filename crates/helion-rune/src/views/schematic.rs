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

use crate::live::{LiveNetlist, schematic_netlist, slug};
use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::schematic_cone::{self as cone, ConeTrace, SheetFind};
use crate::views::schematic_flow as flow;
use crate::views::sources::pane_header;

/// Minimum sheet width in points at 100%; the layout extends it with the
/// logic depth.
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
/// Hierarchy-block width when collapsed (an expanded block grows to fit).
const BLOCK_W: f32 = 190.0;
/// Block header height (name strip above the block's own cells).
const BLOCK_HEAD: f32 = 26.0;
/// Space between a block's outline and its own sheet.
const BLOCK_PAD: f32 = 6.0;
/// Child mini-cell size inside an expanded block.
const CHILD_W: f32 = 88.0;
const CHILD_H: f32 = 24.0;
/// Top-level cell size.
const KIND_W: f32 = 105.0;
const CELL_H: f32 = 28.0;
/// Space above the first row of the sheet.
const SHEET_TOP: f32 = 16.0;

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
    /// Leading points that run from a cell inside an expanded block to the
    /// block's edge. They are dropped while that cell does not draw (the
    /// Blocks LOD), so the wire starts at the block outline.
    pub head: usize,
    /// Trailing points from a block's edge to a cell inside it.
    pub tail: usize,
}

/// Signal-flow layout of a live netlist ([`flow`]): rects for every port,
/// hierarchy block and cell, plus one routed wire per net sink. Derived
/// from the session design; [`SchemLayout::of`] keeps the last one.
#[derive(Clone, Debug, Default)]
pub struct SchemLayout {
    /// World width for this netlist: it grows with the logic depth.
    pub world_w: f32,
    /// World height for this netlist.
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
    /// Signal-flow layout of a live netlist. Input ports sit on the left,
    /// output ports on the right, and blocks and top-level cells in columns
    /// by logic level between them. An expanded block lays its own cells out
    /// the same way inside its outline. A collapsed block shrinks to a header
    /// row (its children unmount, so they get no rects) and the wires of its
    /// cells end on its outline; the sheet reflows, so collapse never leaves
    /// a void.
    pub fn of(live: &LiveNetlist, st: &SchematicState) -> std::rc::Rc<Self> {
        use std::hash::{Hash, Hasher};
        thread_local! {
            static LAST: std::cell::RefCell<Option<(u64, std::rc::Rc<SchemLayout>)>> = const { std::cell::RefCell::new(None) };
        }
        // The layout depends on the netlist and on which blocks are open:
        // hash both (cheap next to the layout) and keep the last result, so
        // scrolling and selecting do not lay the sheet out again each frame.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        live.design.hash(&mut h);
        for c in &live.cells {
            (c.name.as_str(), matches!(c.kind, crate::live::LiveKind::Ff)).hash(&mut h);
        }
        for n in &live.nets {
            (n.name.as_str(), n.loads, &n.cells).hash(&mut h);
        }
        (&live.ports_in, &live.ports_out).hash(&mut h);
        for b in &live.hier {
            (b.name.as_str(), st.is_expanded(&b.name)).hash(&mut h);
        }
        let key = h.finish();
        if let Some(hit) = LAST.with(|c| c.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, l)| l.clone())) {
            return hit;
        }
        let out = std::rc::Rc::new(Self::build(live, st));
        LAST.with(|c| *c.borrow_mut() = Some((key, out.clone())));
        out
    }

    fn build(live: &LiveNetlist, st: &SchematicState) -> Self {
        use flow::{Edge, End};
        /// Where a net endpoint draws: a port, a top-level node, or cell
        /// `.1` of block `.0`.
        #[derive(Clone, Copy)]
        enum At {
            In(usize),
            Out(usize),
            Top(usize),
            Kid(usize, usize),
        }
        /// Which sheet routed a wire, and the edge's index there.
        enum Route {
            Inner(usize, usize),
            Top(usize),
        }

        // Top-level nodes are the blocks, then the cells outside every block.
        let nb = live.hier.len();
        let block_of: HashMap<&str, usize> = live.hier.iter().enumerate().map(|(i, h)| (h.name.as_str(), i)).collect();
        let open: Vec<bool> = live.hier.iter().map(|h| st.is_expanded(&h.name)).collect();
        let mut kids: Vec<Vec<&str>> = vec![Vec::new(); nb];
        let mut tops: Vec<&str> = Vec::new();
        let mut at: HashMap<&str, At> = HashMap::new();
        for c in &live.cells {
            let name = c.name.as_str();
            match live.owner_of(name).and_then(|o| block_of.get(o)) {
                Some(&b) => {
                    at.insert(name, At::Kid(b, kids[b].len()));
                    kids[b].push(name);
                }
                None => {
                    at.insert(name, At::Top(nb + tops.len()));
                    tops.push(name);
                }
            }
        }

        // One leg per sink: an input port drives its net, else the net's
        // first cell does; an output port is one more sink.
        let mut inner_edges: Vec<Vec<Edge>> = vec![Vec::new(); nb];
        let mut top_edges: Vec<Edge> = Vec::new();
        let mut legs: Vec<(usize, NetLevel, &str, At, &str, At, Route)> = Vec::new();
        for (ni, net) in live.nets.iter().enumerate() {
            let name = net.name.as_str();
            let in_port = live.ports_in.binary_search(&net.name).ok();
            let out_port = live.ports_out.binary_search(&net.name).ok();
            let mut ends: Vec<(&str, At)> = Vec::new();
            if let Some(i) = in_port {
                ends.push((name, At::In(i)));
            }
            ends.extend(net.cells.iter().filter_map(|c| at.get(c.as_str()).map(|a| (c.as_str(), *a))));
            if let Some(j) = out_port {
                ends.push((name, At::Out(j)));
            }
            if ends.len() < 2 {
                continue;
            }
            let level = if in_port.is_some() || out_port.is_some() || net.loads >= 8 {
                NetLevel::Backbone
            } else {
                let owners: HashSet<usize> = ends.iter().filter_map(|e| if let At::Kid(b, _) = e.1 { Some(b) } else { None }).collect();
                match owners.iter().next() {
                    Some(&b) if owners.len() == 1 => NetLevel::Internal(live.hier[b].name.clone()),
                    _ => NetLevel::Detail,
                }
            };
            let (driver, da) = ends[0];
            for &(sink, sa) in &ends[1..] {
                let route = match (da, sa) {
                    (At::Kid(b, k0), At::Kid(b1, k1)) if b == b1 => {
                        if !open[b] {
                            // Both ends are inside one collapsed block.
                            continue;
                        }
                        inner_edges[b].push(Edge { net: ni, from: End::Node(k0), to: End::Node(k1) });
                        Route::Inner(b, inner_edges[b].len() - 1)
                    }
                    _ => {
                        let from = match da {
                            At::In(i) => End::Left(i),
                            At::Top(n) | At::Kid(n, _) => End::Node(n),
                            // An output port never drives: it is always last.
                            At::Out(_) => continue,
                        };
                        let to = match sa {
                            At::Out(j) => End::Right(j),
                            At::Top(n) | At::Kid(n, _) => End::Node(n),
                            At::In(_) => continue,
                        };
                        top_edges.push(Edge { net: ni, from, to });
                        Route::Top(top_edges.len() - 1)
                    }
                };
                legs.push((ni, level.clone(), driver, da, sink, sa, route));
            }
        }

        // Registers and latches end a loop: the flow layout cuts it at their output.
        let is_seq = |name: &str| live.cell(name).is_some_and(|c| matches!(c.kind, crate::live::LiveKind::Ff));
        // Expanded blocks first: their size is what the top sheet places.
        let inner: Vec<Option<flow::Sheet>> = (0..nb)
            .map(|b| {
                let seq: Vec<bool> = kids[b].iter().map(|k| is_seq(k)).collect();
                open[b].then(|| flow::layout(&vec![[CHILD_W, CHILD_H]; kids[b].len()], &seq, 0, 0, [0.0, 0.0], &inner_edges[b], [0.0, 0.0]))
            })
            .collect();
        let mut sizes: Vec<[f32; 2]> = inner
            .iter()
            .map(|s| match s {
                Some(s) => [BLOCK_W.max(s.size[0] + 2.0 * BLOCK_PAD), BLOCK_HEAD + s.size[1].max(CHILD_H) + BLOCK_PAD],
                None => [BLOCK_W, BLOCK_HEAD],
            })
            .collect();
        sizes.extend(tops.iter().map(|_| [KIND_W, CELL_H]));
        let seq: Vec<bool> = (0..nb).map(|_| false).chain(tops.iter().map(|t| is_seq(t))).collect();
        let sheet = flow::layout(&sizes, &seq, live.ports_in.len(), live.ports_out.len(), [PORT_W, PORT_H], &top_edges, [PORT_IN_X, SHEET_TOP]);

        let mut out = SchemLayout {
            world_w: (PORT_IN_X + sheet.size[0] + 10.0).max(WORLD_W),
            world_h: (SHEET_TOP + sheet.size[1] + 24.0).max(WORLD_H_MIN),
            ..SchemLayout::default()
        };
        for (i, p) in live.ports_in.iter().enumerate() {
            out.ports.insert(p.clone(), sheet.left[i]);
        }
        for (j, p) in live.ports_out.iter().enumerate() {
            out.ports.insert(p.clone(), sheet.right[j]);
        }
        for (i, name) in tops.iter().enumerate() {
            out.cells.insert((*name).to_string(), sheet.nodes[nb + i]);
        }
        // Where each expanded block's own sheet starts.
        let inner_at = |b: usize| [sheet.nodes[b][0] + BLOCK_PAD, sheet.nodes[b][1] + BLOCK_HEAD];
        for (b, h) in live.hier.iter().enumerate() {
            out.blocks.insert(h.name.clone(), sheet.nodes[b]);
            if let Some(s) = &inner[b] {
                let [ox, oy] = inner_at(b);
                for (k, kid) in kids[b].iter().enumerate() {
                    let r = s.nodes[k];
                    out.cells.insert((*kid).to_string(), [r[0] + ox, r[1] + oy, r[2], r[3]]);
                }
            }
        }
        for (ni, level, driver, da, sink, sa, route) in legs {
            let (mut pts, mut head, mut tail) = (Vec::new(), 0, 0);
            match route {
                Route::Inner(b, e) => {
                    let [ox, oy] = inner_at(b);
                    if let Some(s) = &inner[b] {
                        pts.extend(s.paths[e].iter().map(|p| [p[0] + ox, p[1] + oy]));
                    }
                }
                Route::Top(e) => {
                    // shortcut: inside an expanded block the leg from its outline to the cell takes the short way and can pass under the block's other cells; give blocks their own terminals when that reads badly.
                    if let (At::Kid(b, _), Some(c)) = (da, out.cells.get(driver)) {
                        if open[b] {
                            let (cy, jx, edge) = (c[1] + c[3] / 2.0, c[0] + c[2] + 4.0, sheet.paths[e][0]);
                            pts.extend([[c[0] + c[2], cy], [jx, cy], [jx, edge[1]]]);
                            head = 3;
                        }
                    }
                    pts.extend(sheet.paths[e].iter().copied());
                    if let (At::Kid(b, _), Some(c)) = (sa, out.cells.get(sink)) {
                        if open[b] {
                            let (cy, jx, edge) = (c[1] + c[3] / 2.0, c[0] - 4.0, pts[pts.len() - 1]);
                            pts.extend([[jx, edge[1]], [jx, cy], [c[0], cy]]);
                            tail = 3;
                        }
                    }
                }
            }
            out.wires.push(SchemWire {
                name: live.nets[ni].name.clone(),
                pts,
                level,
                ends: (driver.to_string(), sink.to_string()),
                head,
                tail,
            });
        }
        out
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
    /// Netlist stage from the last implementation run's view contract
    /// (`None`: the session design).
    pub stage: Option<helion_targets::view::NetlistStage>,
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
    /// Canvas top-left in window points, and the sheet point (world units)
    /// at it when the cell nodes were last mounted around the view.
    pub canvas_at: [f32; 2],
    pub view_at: [f32; 2],
    /// Sheet find query, scope, and the next hit index.
    pub find: rune::TextEdit,
    pub find_kind: SheetFind,
    pub find_ix: usize,
    /// Query `find_ix` belongs to (a new query restarts at the first hit).
    pub find_last: String,
    /// The sheet has not been fitted to its canvas yet: the first layout
    /// (and a stage switch or a new run) zooms out to show all of it, as
    /// Vivado opens a schematic.
    pub fit_pending: bool,
}

impl Default for SchematicState {
    fn default() -> Self {
        Self::new()
    }
}

impl SchematicState {
    pub fn new() -> Self {
        Self {
            stage: None,
            zoom: 1.0,
            selected: None,
            collapsed: HashSet::new(),
            cone: None,
            cone_hops: 2,
            stop_at_ff: false,
            history: Vec::new(),
            history_ix: 0,
            viewport: [0.0, 0.0],
            canvas_at: [0.0, 0.0],
            view_at: [0.0, 0.0],
            find: rune::TextEdit::new(""),
            find_kind: SheetFind::Cells,
            find_ix: 0,
            find_last: String::new(),
            fit_pending: true,
        }
    }

    /// Sheet window (world units) that mounts and paints: the view and one
    /// view around it (Ibex has 16k cells on a sheet 375k units tall). The
    /// whole sheet before the first layout.
    pub fn near_window(&self) -> Option<[f32; 4]> {
        let [vw, vh] = [self.viewport[0] / self.zoom, self.viewport[1] / self.zoom];
        let [vx, vy] = self.view_at;
        (vw > 0.0).then(|| [vx - vw, vy - vh, vx + 2.0 * vw, vy + 2.0 * vh])
    }

    pub fn near_view(&self, r: &[f32; 4]) -> bool {
        self.near_window()
            .is_none_or(|[x0, y0, x1, y1]| r[0] + r[2] >= x0 && r[0] <= x1 && r[1] + r[3] >= y0 && r[1] <= y1)
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
/// Show one netlist stage, Vivado style: the top level opens with its
/// hierarchy collapsed, and selections from another stage are dropped.
pub fn set_stage(s: &mut AppState, stage: helion_targets::view::NetlistStage) {
    s.schematic.stage = Some(stage);
    s.schematic.selected = None;
    s.schematic.cone = None;
    if let Some(live) = schematic_netlist(s) {
        s.schematic.set_all(&live, false);
    }
    refit(s);
}

/// A different sheet is on show (another stage, a new run): go back to
/// 100% and fit it again.
pub(crate) fn refit(s: &mut AppState) {
    s.schematic.zoom = 1.0;
    s.schematic.fit_pending = true;
    fit_on_open(s);
}

/// Zoom out to show the whole sheet once its canvas has a size, as Vivado
/// opens a schematic. It keeps a zoom the user or a scene already chose,
/// and never zooms in past 100%.
fn fit_on_open(s: &mut AppState) {
    if !s.schematic.fit_pending || s.schematic.viewport[0] < 8.0 || s.schematic.viewport[1] < 8.0 {
        return;
    }
    let Some(live) = schematic_netlist(s) else {
        return;
    };
    s.schematic.fit_pending = false;
    if (s.schematic.zoom - 1.0).abs() > f32::EPSILON {
        return;
    }
    let layout = SchemLayout::of(&live, &s.schematic);
    if let Some(z) = s.schematic.fit_zoom(layout.world_w, layout.world_h) {
        s.schematic.zoom = z.min(1.0);
    }
}

/// RTL / Synthesized / Implemented tabs for the stages the last run kept.
fn stage_tabs(s: &AppState, t: &VivadoTheme) -> El {
    let mut r = row().key("schematic-stages").role("tablist").gap(2.0).items_center();
    let Some(v) = s.runs.impl_view.as_ref() else {
        return r;
    };
    for n in &v.schematic {
        let st = n.stage;
        r = r.child(
            crate::views::cad::tab(format!("schematic-stage-{}", st.label().to_ascii_lowercase()), st.label(), s.schematic.stage == Some(st), t, move |s: &mut AppState| set_stage(s, st))
                .help(format!("{} netlist: {} cells, {} nets", st.label(), n.cells.len(), n.nets.len())),
        );
    }
    for st in helion_targets::view::NetlistStage::ALL.into_iter().filter(|st| v.netlist(*st).is_none()) {
        if let Some(why) = v.missing.get(st.section()) {
            r = r.child(dim_label(&format!("{}: {why}", st.label()), t).key(format!("schematic-stage-missing-{}", st.label().to_ascii_lowercase())).max_w(260.0).truncate().help(why.clone()));
        }
    }
    r
}

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
        .child(stage_tabs(s, t))
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
                    if let Some(live) = schematic_netlist(s) {
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
                    if let Some(live) = schematic_netlist(s) {
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
    // (selection, detail, depth) per row; rows mount virtually (Ibex has 16k cells).
    let mut rows: Vec<(SchemSel, String, u32)> = Vec::new();
    let mut kids_of: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    let mut top: Vec<&str> = Vec::new();
    for c in &live.cells {
        match live.owner_of(&c.name) {
            Some(o) => kids_of.entry(o).or_default().push(c.name.as_str()),
            None => top.push(c.name.as_str()),
        }
    }
    for h in &live.hier {
        rows.push((SchemSel::Hier(h.name.clone()), format!("{} · {}", h.module, h.cells), 0));
        if s.schematic.is_expanded(&h.name) {
            for c in kids_of.get(h.name.as_str()).into_iter().flatten() {
                rows.push((SchemSel::Cell((*c).to_string()), String::new(), 1));
            }
        }
    }
    rows.extend(top.into_iter().map(|c| (SchemSel::Cell(c.to_string()), String::new(), 0)));
    for p in live.ports_in.iter().chain(live.ports_out.iter()) {
        rows.push((SchemSel::Port(p.clone()), String::new(), 0));
    }
    let (t, rows) = (*t, std::rc::Rc::new(rows));
    let keys = rows.clone();
    col()
        .key("schematic-hier")
        .role("tree")
        .w(HIER_W)
        .h_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .scroll_xy()
        .child(
            col()
                .w_full()
                .growth(rune::GrowthSpec::default())
                .virtual_rows(
                    rows.len(),
                    ROW_H,
                    move |_: &AppState, i| std::borrow::Cow::Owned(keys[i].0.hier_key()),
                    move |s: &AppState, i| {
                        let (sel, detail, depth) = &rows[i];
                        hier_row(s, &t, sel.clone(), detail, *depth)
                    },
                ),
        )
}

/// The sheet under everything: dotted grid and sheet frame.
fn grid_shape(s: &AppState, t: &VivadoTheme, layout: &SchemLayout) -> Shape {
    let z = s.schematic.zoom;
    let px = |x: f32, y: f32| [x * z, y * z];
    let mut shape = Shape::new();
    // Dotted grid, in the near window only.
    let (world_w, world_h) = (layout.world_w, layout.world_h);
    let [x0, y0, x1, y1] = s.schematic.near_window().unwrap_or([0.0, 0.0, world_w, world_h]);
    let first = |lo: f32| ((lo / GRID).floor() * GRID).max(GRID);
    let mut gx = first(x0);
    while gx < world_w.min(x1) {
        let mut gy = first(y0);
        while gy < world_h.min(y1) {
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
    let [fw, fh] = px(world_w, world_h);
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
    shape
}

/// Net wires (red when touching the selection) and pin dots at Full LOD.
/// They paint over the hierarchy blocks, so the nets inside an expanded
/// block show, and under the cells and ports.
fn wires_shape(s: &AppState, t: &VivadoTheme, live: &LiveNetlist, layout: &SchemLayout) -> Shape {
    let z = s.schematic.zoom;
    let px = |x: f32, y: f32| [x * z, y * z];
    let mut shape = Shape::new();
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
        // A wire into an expanded block ends on the block outline while the
        // cell inside does not draw.
        let drawn = |name: &str| layout.ports.contains_key(name) || s.schematic.cell_visible(live, name);
        let from = if drawn(&wire.ends.0) { 0 } else { wire.head };
        let to = wire.pts.len() - if drawn(&wire.ends.1) { 0 } else { wire.tail };
        let Some(world) = wire.pts.get(from..to).filter(|p| p.len() >= 2) else {
            continue;
        };
        let (mut lx, mut ly, mut hx, mut hy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in world {
            (lx, ly, hx, hy) = (lx.min(p[0]), ly.min(p[1]), hx.max(p[0]), hy.max(p[1]));
        }
        if !s.schematic.near_view(&[lx, ly, hx - lx, hy - ly]) {
            continue;
        }
        let pts: Vec<[f32; 2]> = world.iter().map(|p| px(p[0], p[1])).collect();
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
        SchemSel::Cell(n) => live.cell(n).map(|c| c.prim.as_str()).unwrap_or("Cell"),
        SchemSel::Port(n) => {
            if live.is_in_port(n) {
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
        .w(layout.world_w * z)
        .h(layout.world_h * z)
        .fill(t.panel_bg)
        .on_press(|s: &mut AppState| s.schematic.selected = None)
        .on_layout_tx(|s: &mut AppState, r: [f32; 4], tx: &mut rune::Tx| {
            // Remount the cells only after scrolling half a view away.
            let st = &mut s.schematic;
            let at = [(st.canvas_at[0] - r[0]) / st.zoom, (st.canvas_at[1] - r[1]) / st.zoom];
            let half = [st.viewport[0] / st.zoom / 2.0, st.viewport[1] / st.zoom / 2.0];
            if (at[0] - st.view_at[0]).abs() > half[0] || (at[1] - st.view_at[1]).abs() > half[1] {
                st.view_at = at;
            } else {
                tx.live_only();
            }
        })
        .child(canvas(grid_shape(s, t, layout)).w_full().h_full());
    let near = |r: &[f32; 4]| s.schematic.near_view(r);
    // Blocks first (under the wires and their children), then the wires,
    // then ports, then mounted cells.
    for h in &live.hier {
        if let Some(rect) = layout.blocks.get(&h.name) {
            sheet = sheet.child(cell_node(s, t, live, SchemSel::Hier(h.name.clone()), *rect));
        }
    }
    sheet = sheet.child(canvas(wires_shape(s, t, live, layout)).absolute(0.0, 0.0).w_full().h_full().inert());
    for (name, rect) in layout.ports.iter().filter(|(_, r)| near(r)) {
        sheet = sheet.child(cell_node(s, t, live, SchemSel::Port(name.clone()), *rect));
    }
    let mut names: Vec<&String> = layout.cells.keys().collect();
    names.sort();
    for name in names {
        let selected = matches!(&s.schematic.selected, Some(SchemSel::Cell(c)) if c == name);
        if s.schematic.cell_visible(live, name) && (selected || near(&layout.cells[name])) {
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
            if (r[0] - s.schematic.canvas_at[0]).abs() > 0.5 || (r[1] - s.schematic.canvas_at[1]).abs() > 0.5 {
                s.schematic.canvas_at = [r[0], r[1]];
            }
            fit_on_open(s);
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
                        .map(|c| match &c.loc {
                            Some(l) => format!("{} @ {l}", c.prim),
                            None => c.prim.clone(),
                        })
                        .unwrap_or_else(|| "Cell".into()),
                    live.cell(n)
                        .map(|c| format!("fanout {}", c.fanout))
                        .unwrap_or_default(),
                    live.design.clone(),
                ),
                SchemSel::Port(n) => (
                    if live.is_in_port(n) {
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
                    "{}{} · {} cells · {} nets — click a cell or hierarchy row",
                    s.schematic.stage.map(|st| format!("{} netlist · ", st.label())).unwrap_or_default(),
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
    let live = schematic_netlist(s);
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

    #[test]
    fn near_window_spans_a_view_around_the_visible_part() {
        let mut st = SchematicState::new();
        assert!(st.near_view(&[1e6, 1e6, 1.0, 1.0]), "everything before the first layout");
        st.viewport = [400.0, 300.0];
        st.zoom = 2.0;
        st.view_at = [1000.0, 5000.0];
        assert_eq!(st.near_window(), Some([800.0, 4850.0, 1400.0, 5300.0]));
        assert!(st.near_view(&[1100.0, 5100.0, 10.0, 10.0]));
        assert!(st.near_view(&[790.0, 4840.0, 20.0, 20.0]), "overlapping the edge");
        assert!(!st.near_view(&[1000.0, 9000.0, 10.0, 10.0]));
        assert!(!st.near_view(&[1500.0, 5000.0, 10.0, 10.0]));
    }
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
        let live = schematic_netlist(&s).unwrap();
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
        let live = schematic_netlist(&s).unwrap();
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
        let live = schematic_netlist(&s).unwrap();
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
    fn a_blocks_wires_end_on_its_outline_when_collapsed_and_reach_its_cell_when_expanded() {
        let mut s = init();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let live = schematic_netlist(&s).unwrap();
        let mut st = SchematicState::new();
        // A wire with exactly one end inside u_led crosses the block's outline.
        let crossing = |w: &&SchemWire| (live.owner_of(&w.ends.0) == Some("u_led")) != (live.owner_of(&w.ends.1) == Some("u_led"));
        let on_edge = |p: [f32; 2], r: [f32; 4]| ((p[0] - r[0]).abs() < 0.01 || (p[0] - r[0] - r[2]).abs() < 0.01) && p[1] > r[1] && p[1] < r[1] + r[3];
        let open = SchemLayout::of(&live, &st);
        let w = open.wires.iter().find(crossing).expect("a net crosses u_led");
        let into = live.owner_of(&w.ends.1) == Some("u_led");
        let (inside, tip, stub) = if into { (&w.ends.1, w.pts[w.pts.len() - 1], w.tail) } else { (&w.ends.0, w.pts[0], w.head) };
        assert!(stub > 0, "an expanded block's wire has a leg inside it");
        assert!(on_edge(tip, open.cells[inside]), "the wire reaches the cell {inside}: {tip:?}");
        let at_outline = if into { w.pts[w.pts.len() - 1 - stub] } else { w.pts[stub] };
        assert!(on_edge(at_outline, open.blocks["u_led"]), "the leg starts on the block outline");
        st.toggle("u_led");
        let shut = SchemLayout::of(&live, &st);
        let w = shut.wires.iter().find(crossing).expect("a collapsed block keeps its outside nets");
        assert_eq!((w.head, w.tail), (0, 0));
        let tip = if into { w.pts[w.pts.len() - 1] } else { w.pts[0] };
        assert!(on_edge(tip, shut.blocks["u_led"]), "the wire ends on the collapsed block: {tip:?}");
        assert!(st.wire_visible(&live, &shut, w) || !matches!(w.level, NetLevel::Backbone), "a backbone net into a collapsed block draws");
    }

    #[test]
    fn the_sheet_widens_with_the_logic_depth_and_holds_every_rect() {
        let mut s = init();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let live = schematic_netlist(&s).unwrap();
        let layout = SchemLayout::of(&live, &SchematicState::new());
        assert!(layout.world_w >= WORLD_W);
        for r in layout.cells.values().chain(layout.ports.values()).chain(layout.blocks.values()) {
            assert!(r[0] >= 0.0 && r[0] + r[2] <= layout.world_w && r[1] >= 0.0 && r[1] + r[3] <= layout.world_h, "rect on the sheet: {r:?}");
        }
        for w in &layout.wires {
            for p in &w.pts {
                assert!(p[0] >= 0.0 && p[0] <= layout.world_w && p[1] >= 0.0 && p[1] <= layout.world_h, "wire {} on the sheet: {p:?}", w.name);
            }
        }
    }

    #[test]
    fn a_sheet_wider_than_its_canvas_opens_fitted_and_a_chosen_zoom_stays() {
        let mut rt = harness_with(schem_state());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let st = &rt.state.schematic;
        let live = schematic_netlist(&rt.state).unwrap();
        let sheet = SchemLayout::of(&live, st);
        assert!(!st.fit_pending, "the first layout fits the sheet");
        assert!(st.zoom <= 1.0, "opening never zooms in");
        assert!(sheet.world_w * st.zoom <= st.viewport[0] + 0.5 && sheet.world_h * st.zoom <= st.viewport[1] + 0.5 || st.zoom <= ZOOM_MIN, "the whole sheet shows: zoom {}", st.zoom);
        // A zoom chosen before the first layout (a scene, a restored session) is kept.
        let mut chosen = schem_state();
        chosen.schematic.zoom = 1.5;
        let mut rt = harness_with(chosen);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.schematic.zoom, 1.5);
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
        // The sheet fits itself on its first layout; step from there.
        rt.frame(1.0 / 60.0);
        let opened = rt.state.schematic.zoom;
        tap(&mut rt, "schematic-zoom-in");
        assert!(rt.state.schematic.zoom > opened);
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
