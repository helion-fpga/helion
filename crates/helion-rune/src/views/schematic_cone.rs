//! Schematic connectivity tools: fan-in / fan-out cones, selection history,
//! a real Fit, sheet find, and Mark Debug / Add Probe on the selected net.
//!
//! Ports the old Schematic toolbar (`helion-ide.rs:8690-8990`) and its model
//! verbs (`ide.rs` `expand_fanin_cone_opts` / `expand_fanout_cone_opts`,
//! `expand_cone`, `collapse_cone`, `expand_inside`, `collapse_inside`,
//! `schematic_previous_view`, `schematic_next_view`) onto the live engine
//! design ([`crate::live::live_design`]). A cone is a breadth-first walk over
//! the real `helion_ir` nets, using each primitive's output pins to decide
//! the direction; nothing is cached across design changes except the last
//! trace result, which the user re-runs or collapses.
//!
//! Every toolbar control runs the same Tcl verb the console accepts
//! ([`tcl_verb`]), so the console transcript records what happened.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el/tx/scroll.rs`: `Tx::scroll_into_view`,
//!   `Tx::scroll_to_instant` (find reveal, Fit).
//! - `crates/rune/src/el/layout_watch.rs`: `El::on_layout` (canvas viewport).
//! - `crates/rune/src/el/text.rs`: `text_input(..).on_submit`.
//! - `crates/rune-kit/src/checkbox.rs`: `Checkbox` (Stop at FF).

use std::collections::{HashMap, HashSet, VecDeque};

use rune::{El, Tx, col, el, row, text, text_input};
use rune_kit::Checkbox;

use crate::live::{LiveKind, LiveNetlist, live_design, live_netlist};
use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::schematic::{SchemSel, SchematicState, WORLD_W};

/// Hop-count limits for the cone stepper.
pub const HOPS_MIN: usize = 1;
pub const HOPS_MAX: usize = 16;
/// Selection history cap.
const HISTORY_CAP: usize = 64;
/// Results strip: rows shown before it scrolls.
const RESULTS_ROWS: usize = 6;

/// Cone traversal direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConeDir {
    FanIn,
    FanOut,
    /// Fan-in plus fan-out (`expand_cone`, Vivado Expand Cone).
    Both,
}

impl ConeDir {
    pub fn label(self) -> &'static str {
        match self {
            ConeDir::FanIn => "Fan-in",
            ConeDir::FanOut => "Fan-out",
            ConeDir::Both => "Fan-in + fan-out",
        }
    }

    /// Old `ConeTraceState::summary_text` direction token.
    pub fn token(self) -> &'static str {
        match self {
            ConeDir::FanIn => "fanin",
            ConeDir::FanOut => "fanout",
            ConeDir::Both => "bi",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            ConeDir::FanIn => "expand_fanin_cone",
            ConeDir::FanOut => "expand_fanout_cone",
            ConeDir::Both => "expand_cone",
        }
    }
}

/// Cone row object class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConeObj {
    Cell,
    Net,
}

/// One results-strip row: engine name, primitive/net type, hop depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConeRow {
    pub name: String,
    pub obj: ConeObj,
    pub kind: String,
    pub hops: usize,
}

impl ConeRow {
    /// Cross-probe target for this row.
    pub fn sel(&self, live: &LiveNetlist) -> SchemSel {
        match self.obj {
            ConeObj::Cell => SchemSel::Cell(self.name.clone()),
            ConeObj::Net
                if live.ports_in.contains(&self.name) || live.ports_out.contains(&self.name) =>
            {
                SchemSel::Port(self.name.clone())
            }
            ConeObj::Net => SchemSel::Net(self.name.clone()),
        }
    }
}

/// A traced cone over the live design (old `ConeTraceState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConeTrace {
    pub root: String,
    pub dir: ConeDir,
    pub hops: usize,
    pub stop_at_ff: bool,
    /// Rows sorted by hop depth, cells before nets, then name.
    pub rows: Vec<ConeRow>,
    pub cells: HashSet<String>,
    pub nets: HashSet<String>,
}

impl ConeTrace {
    pub fn has_cell(&self, name: &str) -> bool {
        self.cells.contains(name)
    }

    pub fn has_net(&self, name: &str) -> bool {
        self.nets.contains(name)
    }

    /// Old `ConeTraceState::summary_text` dump.
    pub fn summary_text(&self) -> String {
        let list = |obj: ConeObj| {
            let v: Vec<&str> = self
                .rows
                .iter()
                .filter(|r| r.obj == obj)
                .map(|r| r.name.as_str())
                .collect();
            if v.is_empty() {
                "-".to_string()
            } else {
                v.join(",")
            }
        };
        format!(
            "cone_trace DIR={} ROOT={} TARGET=- MAX_HOPS={} STOP_AT_FF={} CELLS={} NETS={}",
            self.dir.token(),
            self.root,
            self.hops,
            u8::from(self.stop_at_ff),
            list(ConeObj::Cell),
            list(ConeObj::Net)
        )
    }
}

/// Output pins per primitive (same table the HNF netlist interchange uses).
fn pin_is_output(kind: &helion_ir::CellKind, pin: &str) -> bool {
    use helion_ir::CellKind as K;
    match kind {
        K::Lut6 { .. } | K::IobIn | K::ClockBuf => pin == "O",
        K::Hff => pin == "Q",
        K::IobOut => pin == "PAD" || pin == "O",
        K::Mac27 => pin == "P" || pin == "O",
        K::Dsp48e1 { .. } => pin == "P" || pin.starts_with("P["),
        K::Bram18 => pin == "O" || pin.starts_with("DO"),
        K::Bram18e1 { .. } => pin.starts_with("DOA") || pin.starts_with("DOB"),
        K::Carry4 => pin.starts_with('O') || pin.starts_with("CO"),
        K::Ila { .. } | K::BlackBox { .. } => false,
    }
}

/// Driver/sink adjacency of a design, by engine name.
struct Graph<'a> {
    kinds: HashMap<String, &'a helion_ir::CellKind>,
    drivers: HashMap<String, Vec<String>>,
    sinks: HashMap<String, Vec<String>>,
    outs: HashMap<String, Vec<String>>,
    ins: HashMap<String, Vec<String>>,
    ports: HashSet<String>,
}

impl<'a> Graph<'a> {
    fn of(d: &'a helion_ir::Design) -> Self {
        let kinds: HashMap<String, &helion_ir::CellKind> = d
            .cells
            .iter()
            .map(|c| (helion_ir::resolve(c.name).to_string(), &c.kind))
            .collect();
        let mut g = Graph {
            kinds,
            drivers: HashMap::new(),
            sinks: HashMap::new(),
            outs: HashMap::new(),
            ins: HashMap::new(),
            ports: d
                .ports
                .iter()
                .map(|p| helion_ir::resolve(p.name).to_string())
                .collect(),
        };
        for net in &d.nets {
            let n = helion_ir::resolve(net.name).to_string();
            for ep in &net.endpoints {
                let cell = helion_ir::resolve(ep.cell).to_string();
                let Some(kind) = g.kinds.get(&cell) else {
                    continue;
                };
                if pin_is_output(kind, helion_ir::resolve(ep.pin)) {
                    g.drivers.entry(n.clone()).or_default().push(cell.clone());
                    g.outs.entry(cell).or_default().push(n.clone());
                } else {
                    g.sinks.entry(n.clone()).or_default().push(cell.clone());
                    g.ins.entry(cell).or_default().push(n.clone());
                }
            }
        }
        g
    }

    fn is_ff(&self, cell: &str) -> bool {
        matches!(self.kinds.get(cell), Some(helion_ir::CellKind::Hff))
    }

    /// One-direction breadth-first walk: (cell depths, net depths).
    fn walk(
        &self,
        root: &str,
        root_is_cell: bool,
        forward: bool,
        hops: usize,
        stop_at_ff: bool,
    ) -> (HashMap<String, usize>, HashMap<String, usize>) {
        let empty: Vec<String> = Vec::new();
        let net_cells = |n: &str| -> &Vec<String> {
            if forward {
                self.sinks.get(n).unwrap_or(&empty)
            } else {
                self.drivers.get(n).unwrap_or(&empty)
            }
        };
        let cell_nets = |c: &str| -> &Vec<String> {
            if forward {
                self.outs.get(c).unwrap_or(&empty)
            } else {
                self.ins.get(c).unwrap_or(&empty)
            }
        };
        let mut cells: HashMap<String, usize> = HashMap::new();
        let mut nets: HashMap<String, usize> = HashMap::new();
        let mut q: VecDeque<(String, usize)> = VecDeque::new();
        if root_is_cell {
            cells.insert(root.to_string(), 0);
            q.push_back((root.to_string(), 0));
        } else {
            nets.insert(root.to_string(), 0);
            if hops > 0 {
                for c in net_cells(root) {
                    if !cells.contains_key(c) {
                        cells.insert(c.clone(), 1);
                        q.push_back((c.clone(), 1));
                    }
                }
            }
        }
        while let Some((cell, depth)) = q.pop_front() {
            if depth >= hops || (depth > 0 && stop_at_ff && self.is_ff(&cell)) {
                continue;
            }
            for n in cell_nets(&cell) {
                nets.entry(n.clone()).or_insert(depth + 1);
                for c in net_cells(n) {
                    if !cells.contains_key(c) {
                        cells.insert(c.clone(), depth + 1);
                        q.push_back((c.clone(), depth + 1));
                    }
                }
            }
        }
        (cells, nets)
    }
}

/// Trace a cone from a cell or net (`cell/pin` accepted, like the old
/// `normalize_cone_endpoint`) up to `hops` hops.
pub fn trace(
    d: &helion_ir::Design,
    root: &str,
    dir: ConeDir,
    hops: usize,
    stop_at_ff: bool,
) -> Result<ConeTrace, String> {
    let root = root.trim();
    let root = root.split_once('/').map(|(c, _)| c).unwrap_or(root);
    let g = Graph::of(d);
    let is_cell = g.kinds.contains_key(root);
    let is_net = d.nets.iter().any(|n| helion_ir::resolve(n.name) == root);
    if !is_cell && !is_net {
        return Err(format!("unknown root {root}"));
    }
    let mut cells: HashMap<String, usize> = HashMap::new();
    let mut nets: HashMap<String, usize> = HashMap::new();
    let mut merge = |(c, n): (HashMap<String, usize>, HashMap<String, usize>)| {
        for (k, v) in c {
            let e = cells.entry(k).or_insert(v);
            *e = (*e).min(v);
        }
        for (k, v) in n {
            let e = nets.entry(k).or_insert(v);
            *e = (*e).min(v);
        }
    };
    if matches!(dir, ConeDir::FanIn | ConeDir::Both) {
        merge(g.walk(root, is_cell, false, hops, stop_at_ff));
    }
    if matches!(dir, ConeDir::FanOut | ConeDir::Both) {
        merge(g.walk(root, is_cell, true, hops, stop_at_ff));
    }
    let mut rows: Vec<ConeRow> = cells
        .iter()
        .map(|(name, hops)| ConeRow {
            name: name.clone(),
            obj: ConeObj::Cell,
            kind: g
                .kinds
                .get(name)
                .map(|k| LiveKind::from_cell(k).label().to_string())
                .unwrap_or_else(|| "Cell".into()),
            hops: *hops,
        })
        .chain(nets.iter().map(|(name, hops)| ConeRow {
            name: name.clone(),
            obj: ConeObj::Net,
            kind: if g.ports.contains(name) {
                "Port net".into()
            } else {
                "Net".into()
            },
            hops: *hops,
        }))
        .collect();
    rows.sort_by(|a, b| (a.hops, a.obj, &a.name).cmp(&(b.hops, b.obj, &b.name)));
    Ok(ConeTrace {
        root: root.to_string(),
        dir,
        hops,
        stop_at_ff,
        rows,
        cells: cells.into_keys().collect(),
        nets: nets.into_keys().collect(),
    })
}

// ---- selection history ----

impl SchematicState {
    /// Record a selection as the newest history entry (dropping any
    /// forward entries, like a browser).
    pub fn push_history(&mut self, sel: &SchemSel) {
        if self.history.get(self.history_ix) == Some(sel) {
            return;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.history_ix + 1);
        }
        self.history.push(sel.clone());
        if self.history.len() > HISTORY_CAP {
            self.history.remove(0);
        }
        self.history_ix = self.history.len() - 1;
    }

    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty() && self.history_ix > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_ix + 1 < self.history.len()
    }

    /// Step back one selection; `None` at the oldest entry.
    pub fn go_back(&mut self) -> Option<SchemSel> {
        if !self.can_go_back() {
            return None;
        }
        self.history_ix -= 1;
        self.selected = Some(self.history[self.history_ix].clone());
        self.selected.clone()
    }

    /// Step forward one selection; `None` at the newest entry.
    pub fn go_forward(&mut self) -> Option<SchemSel> {
        if !self.can_go_forward() {
            return None;
        }
        self.history_ix += 1;
        self.selected = Some(self.history[self.history_ix].clone());
        self.selected.clone()
    }

    /// Zoom that fits the whole sheet into the canvas viewport (`None`
    /// until the canvas has been laid out once).
    pub fn fit_zoom(&self, world_h: f32) -> Option<f32> {
        let [vw, vh] = self.viewport;
        if vw < 8.0 || vh < 8.0 || world_h <= 0.0 {
            return None;
        }
        let z = ((vw - 4.0) / WORLD_W).min((vh - 4.0) / world_h);
        Some(z.clamp(
            crate::views::schematic::ZOOM_MIN,
            crate::views::schematic::ZOOM_MAX,
        ))
    }
}

/// Fit the drawing to the canvas viewport and scroll to its origin.
pub fn fit(s: &mut AppState) -> String {
    if let Some(live) = live_netlist(s) {
        let layout = crate::views::schematic::SchemLayout::of(&live, &s.schematic);
        match s.schematic.fit_zoom(layout.world_h) {
            Some(z) => s.schematic.zoom = z,
            None => s.schematic.zoom = 1.0,
        }
    }
    format!("camera zoom={:.3}", s.schematic.zoom)
}

// ---- reveal / find ----

/// Make a selection's canvas node mount (expand its owner, leave Blocks
/// LOD) and scroll the canvas and the hierarchy tree to it.
pub fn reveal(s: &mut AppState, sel: &SchemSel, tx: &mut Tx) {
    let Some(live) = live_netlist(s) else {
        return;
    };
    let target = match sel {
        SchemSel::Net(n) => match live.net(n).and_then(|w| w.cells.first()) {
            Some(c) => SchemSel::Cell(c.clone()),
            None => return,
        },
        other => other.clone(),
    };
    if let SchemSel::Cell(c) = &target {
        if let Some(owner) = live.owner_of(c) {
            s.schematic.collapsed.remove(owner);
            if s.schematic.lod() == crate::views::schematic::Lod::Blocks {
                s.schematic.zoom = 0.5;
            }
        }
    }
    tx.scroll_into_view(target.key());
    tx.scroll_into_view(target.hier_key());
}

/// Sheet find scope (old `sheet_find cells|ports|nets`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SheetFind {
    #[default]
    Cells,
    Ports,
    Nets,
}

impl SheetFind {
    pub const ALL: [SheetFind; 3] = [SheetFind::Cells, SheetFind::Ports, SheetFind::Nets];

    pub fn label(self) -> &'static str {
        match self {
            SheetFind::Cells => "Cells",
            SheetFind::Ports => "Ports",
            SheetFind::Nets => "Nets",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SheetFind::Cells => "schematic-find-cells",
            SheetFind::Ports => "schematic-find-ports",
            SheetFind::Nets => "schematic-find-nets",
        }
    }
}

/// Live hits for a sheet find, in stable sorted order.
pub fn find_hits(live: &LiveNetlist, query: &str, kind: SheetFind) -> Vec<SchemSel> {
    let q = query.trim().to_lowercase();
    let ok = |n: &str| q.is_empty() || n.to_lowercase().contains(&q);
    match kind {
        SheetFind::Cells => live
            .cells
            .iter()
            .filter(|c| ok(&c.name))
            .map(|c| SchemSel::Cell(c.name.clone()))
            .collect(),
        SheetFind::Ports => live
            .ports_in
            .iter()
            .chain(live.ports_out.iter())
            .filter(|p| ok(p))
            .map(|p| SchemSel::Port(p.clone()))
            .collect(),
        SheetFind::Nets => live
            .nets
            .iter()
            .filter(|n| ok(&n.name))
            .map(|n| SchemSel::Net(n.name.clone()))
            .collect(),
    }
}

/// Select and reveal the next find hit (Return cycles through them).
pub fn find_next(s: &mut AppState, tx: &mut Tx) {
    let Some(live) = live_netlist(s) else {
        return;
    };
    let hits = find_hits(&live, &s.schematic.find.text, s.schematic.find_kind);
    if s.schematic.find_last != s.schematic.find.text {
        s.schematic.find_last = s.schematic.find.text.clone();
        s.schematic.find_ix = 0;
    }
    if hits.is_empty() {
        return;
    }
    let i = s.schematic.find_ix % hits.len();
    s.schematic.find_ix = i + 1;
    let sel = hits[i].clone();
    crate::views::schematic::select(s, sel.clone());
    reveal(s, &sel, tx);
}

// ---- debug ----

/// Net Mark Debug / Add Probe act on: a selected net, or a port's net.
pub fn debug_net(s: &AppState, live: &LiveNetlist) -> Option<String> {
    match s.schematic.selected.as_ref()? {
        SchemSel::Net(n) | SchemSel::Port(n) if live.net(n).is_some() => Some(n.clone()),
        _ => None,
    }
}

/// Whether a net already carries `mark_debug` in the console or run design.
pub fn net_marked(s: &AppState, net: &str) -> bool {
    [s.tcl.design.as_ref(), live_design(s)]
        .into_iter()
        .flatten()
        .any(|d| {
            d.nets.iter().any(|n| {
                helion_ir::resolve(n.name) == net && n.attrs.get("mark_debug") == Some("true")
            })
        })
}

// ---- Tcl verbs ----

/// Verbs this module owns (registered in `tcl::COMMANDS`).
pub const VERBS: &[&str] = &[
    "expand_cone",
    "expand_fanin_cone",
    "expand_fanout_cone",
    "collapse_cone",
    "expand_inside",
    "collapse_inside",
    "schematic_previous",
    "previous_view",
    "schematic_next",
    "next_view",
    "schematic_zoom_fit",
];

/// Root from the current selection (cells, ports and nets; never blocks).
fn selection_root(s: &AppState) -> Result<String, String> {
    match &s.schematic.selected {
        Some(SchemSel::Cell(n)) | Some(SchemSel::Port(n)) | Some(SchemSel::Net(n)) => Ok(n.clone()),
        Some(SchemSel::Hier(n)) => Err(format!(
            "{n} is a hierarchy block; select a cell, port, or net"
        )),
        None => Err("select a cell, port, or net first".into()),
    }
}

/// Parse `?root? ?hops? ?-stop_at_ff?`.
fn cone_args(args: &[String]) -> (Option<String>, Option<usize>, bool) {
    let mut stop = false;
    let mut pos = Vec::new();
    for a in args {
        if a == "-stop_at_ff" || a == "--stop-at-ff" {
            stop = true;
        } else {
            pos.push(a.as_str());
        }
    }
    let root = pos.first().map(|r| r.to_string());
    let hops = pos.get(1).and_then(|h| h.parse().ok());
    (root, hops, stop)
}

fn run_cone(
    s: &mut AppState,
    verb: &str,
    root: String,
    dir: ConeDir,
    hops: usize,
    stop: bool,
) -> Result<String, String> {
    let d = live_design(s).ok_or_else(|| format!("{verb}: no synthesized design"))?;
    let hops = hops.clamp(HOPS_MIN, HOPS_MAX);
    let cone = trace(d, &root, dir, hops, stop).map_err(|e| format!("{verb}: {e}"))?;
    let text = cone.summary_text();
    s.schematic.cone_hops = hops;
    s.schematic.stop_at_ff = stop;
    s.schematic.cone = Some(cone);
    s.navigate(CenterPane::Schematic);
    Ok(text)
}

/// Dispatch one schematic verb. Errors use the old `verb: reason` words.
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "expand_fanin_cone" | "expand_fanout_cone" => {
            let dir = if name == "expand_fanin_cone" {
                ConeDir::FanIn
            } else {
                ConeDir::FanOut
            };
            let (root, hops, stop) = cone_args(args);
            let root = match root {
                Some(r) => r,
                None => selection_root(s).map_err(|e| format!("{name}: {e}"))?,
            };
            run_cone(s, name, root, dir, hops.unwrap_or(1), stop)
        }
        "expand_cone" => {
            let (root, hops, stop) = cone_args(args);
            match (root, &s.schematic.cone) {
                // Bare `expand_cone` grows the active cone by one hop.
                (None, Some(c)) if hops.is_none() => {
                    let c = c.clone();
                    run_cone(s, name, c.root, c.dir, c.hops + 1, c.stop_at_ff || stop)
                }
                (root, _) => {
                    let root = match root {
                        Some(r) => r,
                        None => selection_root(s).map_err(|e| format!("{name}: {e}"))?,
                    };
                    let stop = stop || s.schematic.stop_at_ff;
                    run_cone(s, name, root, ConeDir::Both, hops.unwrap_or(1), stop)
                }
            }
        }
        "collapse_cone" => {
            let had = s.schematic.cone.take();
            s.navigate(CenterPane::Schematic);
            Ok(match had {
                Some(c) => format!("collapse_cone root={}", c.root),
                None => "collapse_cone (no active cone)".into(),
            })
        }
        "expand_inside" | "collapse_inside" => {
            let live = live_netlist(s).ok_or_else(|| format!("{name}: no synthesized design"))?;
            let inst = match args.first() {
                Some(n) => n.clone(),
                None => match &s.schematic.selected {
                    Some(SchemSel::Hier(h)) => h.clone(),
                    Some(other) => match live.owner_of(other.name()) {
                        Some(o) => o.to_string(),
                        None => {
                            return Err(format!(
                                "{name}: {} is not inside a hierarchy block",
                                other.name()
                            ));
                        }
                    },
                    None => return Err(format!("{name}: select an instance first")),
                },
            };
            let Some(h) = live.hier_of(&inst) else {
                return Err(format!("{name}: no hierarchical instance {inst}"));
            };
            if name == "expand_inside" {
                s.schematic.collapsed.remove(&inst);
            } else {
                s.schematic.collapsed.insert(inst.clone());
            }
            crate::views::schematic::select(s, SchemSel::Hier(inst.clone()));
            s.navigate(CenterPane::Schematic);
            Ok(format!("{name} {inst} nested={}", h.cells))
        }
        "schematic_previous" | "previous_view" => {
            let sel = s
                .schematic
                .go_back()
                .ok_or_else(|| "previous_view: no earlier selection".to_string())?;
            s.navigate(CenterPane::Schematic);
            Ok(history_text(s, &sel))
        }
        "schematic_next" | "next_view" => {
            let sel = s
                .schematic
                .go_forward()
                .ok_or_else(|| "next_view: no later selection".to_string())?;
            s.navigate(CenterPane::Schematic);
            Ok(history_text(s, &sel))
        }
        "schematic_zoom_fit" => {
            s.navigate(CenterPane::Schematic);
            Ok(fit(s))
        }
        _ => Err(format!("invalid command name \"{name}\"")),
    }
}

fn history_text(s: &AppState, sel: &SchemSel) -> String {
    format!(
        "{} hist={}/{}",
        sel.tcl(),
        s.schematic.history_ix + 1,
        s.schematic.history.len()
    )
}

// ---- views ----

/// Disabled-aware glyph button for the main toolbar (Previous/Next).
fn glyph_btn(
    key: &'static str,
    label: &'static str,
    glyph: &'static str,
    reason: Option<&'static str>,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    let mut b = el()
        .key(key)
        .role("button")
        .label(label)
        .size(22.0, 22.0)
        .center()
        .child(
            text(glyph)
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text_dim)
                .a11y_hidden(),
        );
    match reason {
        None => b = b.focusable(true).hover(t.selection).on_press_tx(on),
        Some(why) => {
            b = b
                .disabled(true)
                .description(why)
                .help(why)
                .layer_opacity(0.4)
        }
    }
    b
}

/// Previous / Next selection buttons for the main toolbar.
pub fn history_buttons(s: &AppState, t: &VivadoTheme) -> El {
    row()
        .gap(2.0)
        .items_center()
        .child(glyph_btn(
            "schematic-prev",
            "Previous selection",
            "←",
            (!s.schematic.can_go_back()).then_some("No earlier selection"),
            t,
            |s: &mut AppState, tx: &mut Tx| {
                crate::tcl::eval_command(s, "schematic_previous");
                if let Some(sel) = s.schematic.selected.clone() {
                    reveal(s, &sel, tx);
                }
            },
        ))
        .child(glyph_btn(
            "schematic-next",
            "Next selection",
            "→",
            (!s.schematic.can_go_forward()).then_some("No later selection"),
            t,
            |s: &mut AppState, tx: &mut Tx| {
                crate::tcl::eval_command(s, "schematic_next");
                if let Some(sel) = s.schematic.selected.clone() {
                    reveal(s, &sel, tx);
                }
            },
        ))
}

/// Sheet find: query, Cells/Ports/Nets scope, and the hit counter. Return
/// selects and reveals the next hit.
pub fn find_box(s: &AppState, t: &VivadoTheme, live: Option<&LiveNetlist>) -> El {
    let hits = live
        .map(|l| find_hits(l, &s.schematic.find.text, s.schematic.find_kind).len())
        .unwrap_or(0);
    let mut seg = row()
        .key("schematic-find-scope")
        .role("group")
        .label("Find scope")
        .gap(0.0);
    for kind in SheetFind::ALL {
        let on = s.schematic.find_kind == kind;
        seg = seg.child(
            el().key(kind.key())
                .role("button")
                .label(format!("Find {}", kind.label()))
                .focus_selected(on)
                .focusable(true)
                .h(22.0)
                .px(6.0)
                .center()
                .fill(if on { t.selection } else { t.panel_bg })
                .border(1.0, t.border_light)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    s.schematic.find_kind = kind;
                    s.schematic.find_ix = 0;
                })
                .child(cad::ui(kind.label(), t)),
        );
    }
    let fresh = s.schematic.find_last != s.schematic.find.text;
    let counter = if fresh || s.schematic.find_ix == 0 {
        format!("{hits}")
    } else if hits == 0 {
        "0".to_string()
    } else {
        format!("{}/{hits}", ((s.schematic.find_ix + hits - 1) % hits) + 1)
    };
    row()
        .key("schematic-find")
        .items_center()
        .gap(4.0)
        .grow(1.0)
        .min_w(0.0)
        .max_w(360.0)
        .child(
            row()
                .role("search")
                .h(22.0)
                .grow(1.0)
                .min_w(80.0)
                .items_center()
                .gap(4.0)
                .px(6.0)
                .radius(4.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .child(
                    rune::icon(&rune_kit::glyphs::SEARCH, 11.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                )
                .child(
                    text_input(&s.schematic.find, VivadoTheme::FONT_UI)
                        .single_line()
                        .key("schematic-find-input")
                        .label("Find in schematic")
                        .grow(1.0)
                        .min_w(0.0)
                        .color(t.text)
                        .placeholder("Find…", t.text_faint)
                        .caret(t.text, t.selection)
                        .on_edit(|s: &mut AppState| &mut s.schematic.find)
                        .on_submit(|s: &mut AppState, tx: &mut Tx| find_next(s, tx)),
                ),
        )
        .child(seg)
        .child(
            el().key("schematic-find-count")
                .role("status")
                .label(format!("{hits} matches"))
                .min_w(28.0)
                .child(text(counter).font(cad::mono_style()).color(t.text_dim)),
        )
}

/// Cone + hierarchy-inside toolbar under the zoom toolbar.
pub fn cone_toolbar(s: &AppState, t: &VivadoTheme, live: Option<&LiveNetlist>) -> El {
    let kit = t.kit();
    let root_reason: Option<&'static str> = match (&s.schematic.selected, live) {
        (_, None) => Some("No elaborated design"),
        (None, _) => Some("Select a cell, port, or net"),
        (Some(SchemSel::Hier(_)), _) => Some("Cones start at a cell, port, or net, not a block"),
        _ => None,
    };
    let cone_on = s.schematic.cone.is_some();
    let hops = s.schematic.cone_hops;
    let fan = move |dir: ConeDir| {
        move |s: &mut AppState| {
            if let Ok(root) = selection_root(s) {
                let flag = if s.schematic.stop_at_ff {
                    " -stop_at_ff"
                } else {
                    ""
                };
                crate::tcl::eval_command(
                    s,
                    &format!("{} {root} {}{flag}", dir.verb(), s.schematic.cone_hops),
                );
            }
        }
    };
    // Hierarchy block the Inside buttons act on.
    let inside: Option<String> = match (&s.schematic.selected, live) {
        (Some(SchemSel::Hier(h)), _) => Some(h.clone()),
        (Some(sel), Some(l)) => l.owner_of(sel.name()).map(str::to_string),
        _ => None,
    };
    let inside_expanded = inside.as_deref().map(|h| s.schematic.is_expanded(h));
    let stepper =
        |key: &'static str, label: &'static str, glyph: &'static str, next: usize, ok: bool| {
            let mut b = el()
                .key(key)
                .role("button")
                .label(label)
                .size(20.0, 22.0)
                .center()
                .child(cad::ui(glyph, t));
            if ok {
                b = b
                    .focusable(true)
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| {
                        s.schematic.cone_hops = next;
                        if let Some(c) = s.schematic.cone.clone() {
                            let flag = if c.stop_at_ff { " -stop_at_ff" } else { "" };
                            crate::tcl::eval_command(
                                s,
                                &format!("{} {} {next}{flag}", c.dir.verb(), c.root),
                            );
                        }
                    });
            } else {
                b = b.disabled(true).layer_opacity(0.4);
            }
            b
        };
    let stop = Checkbox::new(s.schematic.stop_at_ff)
        .label("Stop at FF")
        .on_toggle::<AppState>(|s: &mut AppState| {
            s.schematic.stop_at_ff = !s.schematic.stop_at_ff;
            if let Some(c) = s.schematic.cone.clone() {
                let flag = if s.schematic.stop_at_ff {
                    " -stop_at_ff"
                } else {
                    ""
                };
                crate::tcl::eval_command(
                    s,
                    &format!("{} {} {}{flag}", c.dir.verb(), c.root, c.hops),
                );
            }
        });
    let inside_reason = |want_expanded: bool| -> Option<&'static str> {
        match inside_expanded {
            None => Some("Select a hierarchy block or a cell inside one"),
            Some(e) if e == want_expanded => Some(if want_expanded {
                "Block is already expanded"
            } else {
                "Block is already collapsed"
            }),
            _ => None,
        }
    };
    let ins_a = inside.clone();
    let ins_b = inside.clone();
    cad::toolbar("schematic-cone-bar", t)
        .gap(4.0)
        .child(cad::caps("Cone", t))
        .child(cad::btn(
            "schematic-fanin",
            "Fan-In",
            t,
            root_reason,
            fan(ConeDir::FanIn),
        ))
        .child(cad::btn(
            "schematic-fanout",
            "Fan-Out",
            t,
            root_reason,
            fan(ConeDir::FanOut),
        ))
        .child(
            row()
                .key("schematic-hops")
                .role("group")
                .label(format!("Cone hops {hops}"))
                .items_center()
                .gap(0.0)
                .child(cad::dim("Hops", t).pr(2.0))
                .child(stepper(
                    "schematic-hops-dec",
                    "Fewer hops",
                    "−",
                    hops.saturating_sub(1).max(HOPS_MIN),
                    hops > HOPS_MIN,
                ))
                .child(
                    el().key("schematic-hops-value")
                        .w(18.0)
                        .center()
                        .child(text(hops.to_string()).font(cad::mono_style()).color(t.text)),
                )
                .child(stepper(
                    "schematic-hops-inc",
                    "More hops",
                    "+",
                    (hops + 1).min(HOPS_MAX),
                    hops < HOPS_MAX,
                )),
        )
        .child(
            stop.build(&kit)
                .key("schematic-stop-ff")
                .help("Stop cone traversal at register (FF) boundaries"),
        )
        .child(cad::btn(
            "schematic-expand-cone",
            "Expand",
            t,
            if cone_on { None } else { root_reason },
            |s: &mut AppState| crate::tcl::eval_command(s, "expand_cone"),
        ))
        .child(cad::btn(
            "schematic-collapse-cone",
            "Collapse",
            t,
            (!cone_on).then_some("No active cone"),
            |s: &mut AppState| crate::tcl::eval_command(s, "collapse_cone"),
        ))
        .child(cad::tool_sep(t))
        .child(cad::caps("Inside", t))
        .child(cad::btn(
            "schematic-expand-inside",
            "Expand",
            t,
            inside_reason(true),
            move |s: &mut AppState| {
                if let Some(h) = &ins_a {
                    crate::tcl::eval_command(s, &format!("expand_inside {h}"));
                }
            },
        ))
        .child(cad::btn(
            "schematic-collapse-inside",
            "Collapse",
            t,
            inside_reason(false),
            move |s: &mut AppState| {
                if let Some(h) = &ins_b {
                    crate::tcl::eval_command(s, &format!("collapse_inside {h}"));
                }
            },
        ))
}

const CONE_COLS: [ColSpec; 3] = [
    ColSpec::grow("Name", 1.0),
    ColSpec::fixed("Type", 84.0),
    ColSpec::num("Hops", 40.0),
];

/// Results strip under the canvas: the active cone as Name / Type / Hops;
/// a row tap cross-probes and reveals the object.
pub fn cone_results(s: &AppState, t: &VivadoTheme, live: &LiveNetlist) -> Option<El> {
    let cone = s.schematic.cone.as_ref()?;
    let n_cells = cone.rows.iter().filter(|r| r.obj == ConeObj::Cell).count();
    let n_nets = cone.rows.len() - n_cells;
    let summary = format!(
        "{} cone from {} · {} hop{}{} · {n_cells} cells · {n_nets} nets",
        cone.dir.label(),
        cone.root,
        cone.hops,
        if cone.hops == 1 { "" } else { "s" },
        if cone.stop_at_ff {
            " · stop at FF"
        } else {
            ""
        },
    );
    let mut rows = col().w_full();
    for (i, r) in cone.rows.iter().enumerate() {
        let sel = r.sel(live);
        let selected = s.schematic.selected.as_ref() == Some(&sel);
        let tap = sel.clone();
        rows = rows.child(
            cad::table_row(
                format!("schematic-cone-row-{i}"),
                format!("{} {} hop {}", r.name, r.kind, r.hops),
                selected,
                t,
            )
            .on_press_tx(move |s: &mut AppState, tx: &mut Tx| {
                crate::views::schematic::select(s, tap.clone());
                reveal(s, &tap, tx);
            })
            .child(cad::cell(
                &CONE_COLS[0],
                cad::cut(
                    r.name.clone(),
                    cad::mono_style(),
                    if r.name == cone.root && !selected {
                        t.accent
                    } else {
                        t.text
                    },
                ),
            ))
            .child(cad::cell(
                &CONE_COLS[1],
                cad::cut(r.kind.clone(), VivadoTheme::FONT_UI, t.text_dim),
            ))
            .child(cad::cell(&CONE_COLS[2], cad::mono(r.hops.to_string(), t))),
        );
    }
    Some(
        col()
            .key("schematic-cone")
            .role("region")
            .label("Cone results")
            .w_full()
            .h((cone.rows.len().min(RESULTS_ROWS) as f32) * cad::ROW_H + 2.0 * cad::HEAD_H + 2.0)
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .child(
                row()
                    .w_full()
                    .h(cad::HEAD_H)
                    .items_center()
                    .gap(8.0)
                    .px(8.0)
                    .child(cad::title("Cone", t))
                    .child(
                        el().grow(1.0)
                            .min_w(0.0)
                            .child(cad::cut(summary, VivadoTheme::FONT_UI, t.text_dim))
                            .key("schematic-cone-summary"),
                    ),
            )
            .child(cad::header("schematic-cone-head", &CONE_COLS, t))
            .child(
                col()
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .scroll(rune::ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(rows),
            ),
    )
}

/// Mark Debug / Add Probe actions for the props strip (real `mark_debug` /
/// `add_probe` verbs on the selected net or port net).
pub fn debug_actions(s: &AppState, t: &VivadoTheme, live: &LiveNetlist) -> El {
    let net = debug_net(s, live);
    let reason = net.is_none().then_some("Select a net or port to probe");
    let marked = net.as_deref().is_some_and(|n| net_marked(s, n));
    let a = net.clone();
    let b = net.clone();
    let mut r = row().key("schematic-debug").items_center().gap(6.0);
    if marked {
        r = r.child(cad::badge("DEBUG", t.warn, t).key("schematic-debug-badge"));
    }
    r.child(cad::btn(
        "schematic-mark-debug",
        "Mark Debug",
        t,
        reason,
        move |s: &mut AppState| {
            if let Some(n) = &a {
                crate::tcl::eval_command(s, &format!("mark_debug {n}"));
            }
        },
    ))
    .child(cad::btn(
        "schematic-add-probe",
        "Add Probe",
        t,
        reason,
        move |s: &mut AppState| {
            if let Some(n) = &b {
                crate::tcl::eval_command(s, &format!("add_probe {n}"));
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn fixture() -> AppState {
        let mut s = init();
        s.center = CenterPane::Schematic;
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        s
    }

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn some_ff(s: &AppState) -> String {
        let d = live_design(s).unwrap();
        d.cells
            .iter()
            .find(|c| matches!(c.kind, helion_ir::CellKind::Hff))
            .map(|c| helion_ir::resolve(c.name).to_string())
            .expect("fixture has registers")
    }

    #[test]
    fn fanout_cone_grows_with_hops_and_stop_at_ff_bounds_it() {
        let s = fixture();
        let d = live_design(&s).unwrap();
        let root = some_ff(&s);
        let one = trace(d, &root, ConeDir::FanOut, 1, false).unwrap();
        let three = trace(d, &root, ConeDir::FanOut, 3, false).unwrap();
        assert!(one.has_cell(&root));
        assert_eq!(one.rows[0].hops, 0, "root row comes first");
        assert!(one.rows.iter().all(|r| r.hops <= 1));
        assert!(three.cells.len() >= one.cells.len());
        assert!(three.cells.is_superset(&one.cells));
        let stopped = trace(d, &root, ConeDir::FanOut, 8, true).unwrap();
        let free = trace(d, &root, ConeDir::FanOut, 8, false).unwrap();
        assert!(stopped.cells.len() <= free.cells.len());
        // No row past an FF boundary: every non-root FF is a leaf.
        for r in &stopped.rows {
            assert!(r.hops <= 8);
        }
        assert!(
            one.summary_text()
                .starts_with("cone_trace DIR=fanout ROOT=")
        );
    }

    #[test]
    fn fanin_cone_reaches_drivers_and_rejects_unknown_roots() {
        let s = fixture();
        let d = live_design(&s).unwrap();
        let root = some_ff(&s);
        let cone = trace(d, &root, ConeDir::FanIn, 2, false).unwrap();
        assert!(
            cone.rows
                .iter()
                .any(|r| r.obj == ConeObj::Net && r.hops == 1),
            "{cone:?}"
        );
        assert!(
            cone.rows
                .iter()
                .any(|r| r.obj == ConeObj::Cell && r.hops >= 1),
            "{cone:?}"
        );
        assert!(trace(d, "no_such_thing", ConeDir::FanIn, 2, false).is_err());
        // A net root walks from the net, and `cell/pin` roots normalize.
        let by_pin = trace(d, &format!("{root}/Q"), ConeDir::FanIn, 1, false).unwrap();
        assert_eq!(by_pin.root, root);
    }

    #[test]
    fn cone_verbs_trace_grow_and_collapse_through_the_console() {
        let mut s = fixture();
        let root = some_ff(&s);
        crate::tcl::eval_command(&mut s, &format!("expand_fanout_cone {root} 2 -stop_at_ff"));
        let c = s.schematic.cone.clone().expect("cone traced");
        assert_eq!((c.dir, c.hops, c.stop_at_ff), (ConeDir::FanOut, 2, true));
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.starts_with("cone_trace DIR=fanout"))
        );
        crate::tcl::eval_command(&mut s, "expand_cone");
        assert_eq!(
            s.schematic.cone.as_ref().unwrap().hops,
            3,
            "bare expand_cone adds a hop"
        );
        crate::tcl::eval_command(&mut s, "collapse_cone");
        assert!(s.schematic.cone.is_none());
        crate::tcl::eval_command(&mut s, "expand_fanin_cone");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.contains("select a cell, port, or net first")),
            "no selection is an error, not a guess"
        );
    }

    #[test]
    fn inside_verbs_toggle_blocks_and_refuse_unknown_instances() {
        let mut s = fixture();
        crate::tcl::eval_command(&mut s, "collapse_inside u_led");
        assert!(!s.schematic.is_expanded("u_led"));
        crate::tcl::eval_command(&mut s, "expand_inside u_led");
        assert!(s.schematic.is_expanded("u_led"));
        crate::tcl::eval_command(&mut s, "expand_inside nope");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.contains("no hierarchical instance nope"))
        );
    }

    #[test]
    fn history_steps_back_and_forward_and_truncates() {
        let mut st = SchematicState::new();
        assert!(!st.can_go_back() && !st.can_go_forward());
        for n in ["a", "b", "c"] {
            st.push_history(&SchemSel::Cell(n.into()));
        }
        assert_eq!(st.go_back(), Some(SchemSel::Cell("b".into())));
        assert_eq!(st.go_back(), Some(SchemSel::Cell("a".into())));
        assert_eq!(st.go_back(), None);
        assert_eq!(st.go_forward(), Some(SchemSel::Cell("b".into())));
        st.push_history(&SchemSel::Cell("d".into()));
        assert!(
            !st.can_go_forward(),
            "a new selection drops forward history"
        );
        assert_eq!(st.history.len(), 3);
    }

    #[test]
    fn find_hits_follow_scope_and_query() {
        let s = fixture();
        let live = live_netlist(&s).unwrap();
        assert_eq!(
            find_hits(&live, "", SheetFind::Cells).len(),
            live.cells.len()
        );
        assert_eq!(
            find_hits(&live, "", SheetFind::Ports).len(),
            live.ports_in.len() + live.ports_out.len()
        );
        let hits = find_hits(&live, "U_DIV_U_FF", SheetFind::Cells);
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.name().starts_with("u_div_u_ff")));
        assert!(find_hits(&live, "clk", SheetFind::Nets).contains(&SchemSel::Net("clk".into())));
    }

    #[test]
    fn toolbar_cone_flow_highlights_lists_and_cross_probes() {
        let mut rt = harness_with(fixture());
        rt.frame(0.0);
        assert!(rt.find("schematic-cone").is_none(), "no cone yet");
        tap(&mut rt, "schematic-cell-u-div-u-ff0");
        tap(&mut rt, "schematic-fanout");
        let cone = rt.state.schematic.cone.clone().expect("fan-out traced");
        assert_eq!(cone.root, "u_div_u_ff0");
        assert!(rt.find("schematic-cone").is_some());
        assert!(rt.find("schematic-cone-row-0").is_some());
        tap(&mut rt, "schematic-hops-inc");
        assert_eq!(
            rt.state.schematic.cone.as_ref().unwrap().hops,
            3,
            "stepper re-traces"
        );
        tap(&mut rt, "schematic-cone-row-1");
        let want = rt.state.schematic.cone.as_ref().unwrap().rows[1].clone();
        assert_eq!(
            rt.state
                .schematic
                .selected
                .as_ref()
                .map(|s| s.name().to_string()),
            Some(want.name)
        );
        tap(&mut rt, "schematic-prev");
        assert_eq!(
            rt.state.schematic.selected,
            Some(SchemSel::Cell("u_div_u_ff0".into())),
            "Previous returns to the cone root selection"
        );
        tap(&mut rt, "schematic-collapse-cone");
        assert!(rt.state.schematic.cone.is_none());
        rt.frame(1.0 / 60.0);
        assert!(rt.find("schematic-cone").is_none());
    }

    #[test]
    fn mark_debug_runs_the_real_verb_on_the_selected_port_net() {
        let mut s = fixture();
        s.schematic.selected = Some(SchemSel::Port("divclk".into()));
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "schematic-mark-debug");
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "mark_debug divclk"),
            "{:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
        assert!(net_marked(&rt.state, "divclk"));
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(rt.find("schematic-debug-badge").is_some());
    }

    #[test]
    fn debug_actions_are_disabled_without_a_net() {
        let mut s = fixture();
        s.schematic.selected = Some(SchemSel::Cell("u_div_u_ff0".into()));
        let live = live_netlist(&s).unwrap();
        assert_eq!(debug_net(&s, &live), None);
        s.schematic.selected = Some(SchemSel::Net("clk".into()));
        assert_eq!(debug_net(&s, &live).as_deref(), Some("clk"));
    }

    #[test]
    fn fit_zooms_the_sheet_into_the_measured_viewport() {
        let mut rt = harness_with(fixture());
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        let [vw, vh] = rt.state.schematic.viewport;
        assert!(
            vw > 100.0 && vh > 100.0,
            "canvas reports its viewport: {vw}x{vh}"
        );
        tap(&mut rt, "schematic-zoom-in");
        tap(&mut rt, "schematic-zoom-fit");
        let live = live_netlist(&rt.state).unwrap();
        let h = crate::views::schematic::SchemLayout::of(&live, &rt.state.schematic).world_h;
        let z = rt.state.schematic.zoom;
        assert!(
            WORLD_W * z <= vw + 0.5 && h * z <= vh + 0.5,
            "fit shows the whole sheet"
        );
        assert!(
            WORLD_W * z >= vw - 6.0 || h * z >= vh - 6.0,
            "fit fills one axis of the viewport"
        );
    }

    #[test]
    fn find_submit_selects_and_cycles_hits() {
        let mut s = fixture();
        s.schematic.find = rune::TextEdit::new("u_led_u_iob");
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "schematic-find-input");
        rt.key(rune::KeyEvent {
            key: rune::Key::Enter,
            mods: rune::Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        let first = rt.state.schematic.selected.clone().expect("hit selected");
        assert!(first.name().starts_with("u_led_u_iob"));
        rt.key(rune::KeyEvent {
            key: rune::Key::Enter,
            mods: rune::Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert_ne!(
            rt.state.schematic.selected,
            Some(first),
            "Return cycles to the next hit"
        );
    }
}
