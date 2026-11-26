//! Live engine netlist + placement behind the schematic/device views.
//!
//! New module (deep-04-device-schem-live). Both canvases used to draw a
//! static demo netlist (`project1_demo` cells/nets/sites); they now read the
//! real engine design:
//! - [`LiveNetlist::from_design`] flattens a [`helion_ir::Design`] (the
//!   `tcl.design` elaborated by `read_sv`/`synth_design`, or the design a
//!   `launch_runs` worker synthesized) into sorted cells/nets/ports plus the
//!   real instance tree (`Design.instances`, recorded by
//!   `helion-sv/src/lib.rs:record_instances`).
//! - [`LivePlaced::from_placed`] maps a [`helion_place::Placed`] (the
//!   `impl_1` worker result) to per-cell real sites/BELs via
//!   [`helion_place::Placed::cell_bel`], with die geometry from
//!   [`helion_device::Device`] (`interior_cols`/`interior_rows`/columns).
//! - [`seed_live_fixture`] elaborates + packs + places the inline
//!   [`FIXTURE_SRC`] counter through the real engine for scenes and tests
//!   (same pattern as the `wave-live` scene's real `run_engine_vcd` import:
//!   engine output, never canned coordinates).
//!
//! Hierarchy ownership follows the synth prefix rule: instance `u_div` owns
//! cells/nets named `u_div` or starting with `u_div_` (nested instances
//! accumulate the prefix, so the longest match wins).

use std::collections::HashMap;

use crate::state::AppState;

/// Fixture RTL for scenes/tests: two submodules so the real instance tree is
/// non-trivial (`u_div`, `u_led`). Elaborated by `helion_sv::synth_sv`.
pub const FIXTURE_SRC: &str = "module divclk_gen (input logic clk, input logic rst, output logic divclk);\n\
    logic [3:0] cnt;\n\
    always_ff @(posedge clk) begin\n\
        if (rst) cnt <= 4'd0;\n\
        else cnt <= cnt + 1;\n\
    end\n\
    assign divclk = cnt[3];\n\
endmodule\n\
\n\
module led_decode (input logic [3:0] cnt, output logic [7:0] seg);\n\
    assign seg = {4'b0, cnt};\n\
endmodule\n\
\n\
module counter_top (input logic clk, input logic rst, output logic [7:0] seg, output logic divclk);\n\
    logic [3:0] cnt;\n\
    divclk_gen u_div (.clk(clk), .rst(rst), .divclk(divclk));\n\
    always_ff @(posedge divclk) begin\n\
        if (rst) cnt <= 4'd0;\n\
        else cnt <= cnt + 1;\n\
    end\n\
    led_decode u_led (.cnt(cnt), .seg(seg));\n\
endmodule\n";

/// Default part for the fixture die (same default `GpuiShell::default` ships).
pub const FIXTURE_PART: &str = "HL10T-C32-1";

/// Primitive class of a live cell or port. Typed, never a bare string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveKind {
    Lut,
    Ff,
    IobIn,
    IobOut,
    ClkBuf,
    Dsp,
    Bram,
    Carry,
    Ila,
    BlackBox,
    PortIn,
    PortOut,
}

impl LiveKind {
    pub fn label(self) -> &'static str {
        match self {
            LiveKind::Lut => "LUT",
            LiveKind::Ff => "FDRE",
            LiveKind::IobIn => "IBUF",
            LiveKind::IobOut => "OBUF",
            LiveKind::ClkBuf => "BUFG",
            LiveKind::Dsp => "DSP",
            LiveKind::Bram => "BRAM",
            LiveKind::Carry => "CARRY",
            LiveKind::Ila => "ILA",
            LiveKind::BlackBox => "Black box",
            LiveKind::PortIn => "Input port",
            LiveKind::PortOut => "Output port",
        }
    }

    /// Class of a type name from the view contract's netlist (Xilinx
    /// primitive or RTL operator); operators draw in the logic column.
    pub fn from_prim(k: &str) -> Self {
        match k {
            _ if k.starts_with("LUT") || (k.starts_with("RTL_") && !matches!(k, "RTL_REG" | "RTL_LATCH" | "RTL_CONST")) => LiveKind::Lut,
            _ if k.starts_with("FD") || k == "RTL_REG" || k == "RTL_LATCH" || k == "FF" => LiveKind::Ff,
            "IBUF" => LiveKind::IobIn,
            "OBUF" => LiveKind::IobOut,
            _ if k.starts_with("BUFG") => LiveKind::ClkBuf,
            _ if k.starts_with("DSP") => LiveKind::Dsp,
            _ if k.starts_with("RAMB") => LiveKind::Bram,
            _ if k.starts_with("CARRY") => LiveKind::Carry,
            _ => LiveKind::BlackBox,
        }
    }

    pub fn from_cell(kind: &helion_ir::CellKind) -> Self {
        match kind {
            helion_ir::CellKind::Lut6 { .. } => LiveKind::Lut,
            helion_ir::CellKind::Hff => LiveKind::Ff,
            helion_ir::CellKind::IobIn => LiveKind::IobIn,
            helion_ir::CellKind::IobOut => LiveKind::IobOut,
            helion_ir::CellKind::ClockBuf => LiveKind::ClkBuf,
            helion_ir::CellKind::Mac27 | helion_ir::CellKind::Dsp48e1 { .. } => LiveKind::Dsp,
            helion_ir::CellKind::Bram18 | helion_ir::CellKind::Bram18e1 { .. } => LiveKind::Bram,
            helion_ir::CellKind::Carry4 => LiveKind::Carry,
            helion_ir::CellKind::Ila { .. } => LiveKind::Ila,
            helion_ir::CellKind::BlackBox { .. } => LiveKind::BlackBox,
        }
    }
}

/// One real cell: engine name, primitive class, and static fanout (the number
/// of *other* endpoints sharing its nets).
#[derive(Clone, Debug)]
pub struct LiveCell {
    pub name: String,
    pub kind: LiveKind,
    pub fanout: usize,
    /// The cell's own type name (`FDRE`, `LUT4`, `RTL_ADD`).
    pub prim: String,
    /// Site and BEL, in an implemented netlist.
    pub loc: Option<String>,
}

/// One real net: engine name, endpoint count, and the sorted cell names on it.
#[derive(Clone, Debug)]
pub struct LiveNet {
    pub name: String,
    pub loads: usize,
    pub cells: Vec<String>,
}

/// One real hierarchy instance: engine name, module, connection count, and
/// the number of cells it owns by the synth prefix rule.
#[derive(Clone, Debug)]
pub struct LiveHier {
    pub name: String,
    pub module: String,
    pub conns: usize,
    pub cells: usize,
}

/// Sorted live view of an engine design. Built fresh per frame from the
/// session design (cheap: O(cells + nets)); never stored.
#[derive(Clone, Debug, Default)]
pub struct LiveNetlist {
    pub design: String,
    pub cells: Vec<LiveCell>,
    pub nets: Vec<LiveNet>,
    pub ports_in: Vec<String>,
    pub ports_out: Vec<String>,
    pub hier: Vec<LiveHier>,
}

impl LiveNetlist {
    pub fn from_design(d: &helion_ir::Design) -> Self {
        let mut touch: HashMap<String, usize> = HashMap::new();
        let mut nets: Vec<LiveNet> = d
            .nets
            .iter()
            .map(|n| {
                let name = helion_ir::resolve(n.name).to_string();
                let mut cells: Vec<String> = n
                    .endpoints
                    .iter()
                    .map(|e| helion_ir::resolve(e.cell).to_string())
                    .collect();
                cells.sort();
                cells.dedup();
                for c in &cells {
                    *touch.entry(c.clone()).or_insert(0) += n.endpoints.len().saturating_sub(1);
                }
                LiveNet {
                    name,
                    loads: n.endpoints.len(),
                    cells,
                }
            })
            .collect();
        nets.sort_by(|a, b| a.name.cmp(&b.name));
        let mut cells: Vec<LiveCell> = d
            .cells
            .iter()
            .map(|c| {
                let name = helion_ir::resolve(c.name).to_string();
                let kind = LiveKind::from_cell(&c.kind);
                LiveCell {
                    fanout: touch.get(&name).copied().unwrap_or(0),
                    name,
                    kind,
                    prim: kind.label().to_string(),
                    loc: None,
                }
            })
            .collect();
        cells.sort_by(|a, b| a.name.cmp(&b.name));
        let mut ports_in = Vec::new();
        let mut ports_out = Vec::new();
        for p in &d.ports {
            let name = helion_ir::resolve(p.name).to_string();
            match p.dir {
                helion_ir::PortDir::In => ports_in.push(name),
                helion_ir::PortDir::Out | helion_ir::PortDir::Inout => ports_out.push(name),
            }
        }
        ports_in.sort();
        ports_out.sort();
        let mut inst_names: Vec<String> = d
            .instances
            .iter()
            .map(|i| helion_ir::resolve(i.name).to_string())
            .collect();
        inst_names.sort();
        let mut owned: HashMap<&str, usize> = HashMap::new();
        for c in &cells {
            if let Some(o) = owner_prefix(&c.name, |p| inst_names.binary_search_by(|n| n.as_str().cmp(p)).is_ok()) {
                *owned.entry(o).or_insert(0) += 1;
            }
        }
        let hier: Vec<LiveHier> = d
            .instances
            .iter()
            .map(|i| {
                let name = helion_ir::resolve(i.name).to_string();
                LiveHier {
                    cells: owned.get(name.as_str()).copied().unwrap_or(0),
                    name,
                    module: helion_ir::resolve(i.module).to_string(),
                    conns: i.conns.len(),
                }
            })
            .collect();
        let mut hier = hier;
        hier.sort_by(|a, b| a.name.cmp(&b.name));
        LiveNetlist {
            design: helion_ir::resolve(d.name).to_string(),
            cells,
            nets,
            ports_in,
            ports_out,
            hier,
        }
    }

    /// Live view of one stage of the view contract's schematic netlist. An
    /// output port fed by a net of another name gets a net of its own name
    /// carrying that net's drivers, so the port draws its wire.
    pub fn from_netlist(n: &helion_targets::view::SchematicNetlist) -> Self {
        let cell_of = |e: &str| e.rsplit_once('/').map(|(c, _)| c.to_string());
        let mut touch: HashMap<String, usize> = HashMap::new();
        let mut nets: Vec<LiveNet> = n
            .nets
            .iter()
            .map(|net| {
                let ends = net.drivers.len() + net.loads.len();
                // Driver cells first: the sheet layout draws from the first cell.
                let mut cells: Vec<String> = net.drivers.iter().filter_map(|e| cell_of(e)).collect();
                let mut loads: Vec<String> = net.loads.iter().filter_map(|e| cell_of(e)).filter(|c| !cells.contains(c)).collect();
                loads.sort();
                loads.dedup();
                cells.dedup();
                cells.extend(loads);
                for c in &cells {
                    *touch.entry(c.clone()).or_insert(0) += ends.saturating_sub(1);
                }
                LiveNet { name: net.name.clone(), loads: ends, cells }
            })
            .collect();
        let by_name: HashMap<&str, &helion_targets::view::NetlistNet> = n.nets.iter().map(|x| (x.name.as_str(), x)).collect();
        for p in n.ports.iter().filter(|p| p.direction != "input") {
            if by_name.contains_key(p.name.as_str()) {
                continue;
            }
            let mut cells: Vec<String> = p.nets.iter().filter_map(|x| by_name.get(x.as_str())).flat_map(|x| x.drivers.iter().filter_map(|e| cell_of(e))).collect();
            cells.sort();
            cells.dedup();
            nets.push(LiveNet { name: p.name.clone(), loads: cells.len() + 1, cells });
        }
        nets.sort_by(|a, b| a.name.cmp(&b.name));
        let mut cells: Vec<LiveCell> = n
            .cells
            .iter()
            .map(|c| LiveCell {
                fanout: touch.get(&c.name).copied().unwrap_or(0),
                name: c.name.clone(),
                kind: LiveKind::from_prim(&c.kind),
                prim: c.kind.clone(),
                loc: c.site.as_ref().map(|s| match &c.bel {
                    Some(b) => format!("{s}/{b}"),
                    None => s.clone(),
                }),
            })
            .collect();
        cells.sort_by(|a, b| a.name.cmp(&b.name));
        let mut ports_in: Vec<String> = n.ports.iter().filter(|p| p.direction == "input").map(|p| p.name.clone()).collect();
        let mut ports_out: Vec<String> = n.ports.iter().filter(|p| p.direction != "input").map(|p| p.name.clone()).collect();
        ports_in.sort();
        ports_out.sort();
        let mut inst_names: Vec<&str> = n.instances.iter().map(|(i, _)| i.as_str()).collect();
        inst_names.sort_unstable();
        let mut owned: HashMap<&str, usize> = HashMap::new();
        for c in &cells {
            if let Some(o) = owner_prefix(&c.name, |p| inst_names.binary_search(&p).is_ok()) {
                *owned.entry(o).or_insert(0) += 1;
            }
        }
        let mut hier: Vec<LiveHier> = n
            .instances
            .iter()
            .map(|(name, module)| LiveHier { cells: owned.get(name.as_str()).copied().unwrap_or(0), name: name.clone(), module: module.clone(), conns: 0 })
            .collect();
        hier.sort_by(|a, b| a.name.cmp(&b.name));
        hier.dedup_by(|a, b| a.name == b.name);
        LiveNetlist { design: n.top.clone(), cells, nets, ports_in, ports_out, hier }
    }

    pub fn cell(&self, name: &str) -> Option<&LiveCell> {
        self.cells
            .binary_search_by(|c| c.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.cells[i])
    }

    // `nets`, `cells` and `hier` are sorted by name in `from_design`.
    pub fn net(&self, name: &str) -> Option<&LiveNet> {
        self.nets
            .binary_search_by(|n| n.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.nets[i])
    }

    // `ports_in` and `ports_out` are sorted in `from_design`.
    pub fn is_in_port(&self, name: &str) -> bool {
        self.ports_in.binary_search_by(|p| p.as_str().cmp(name)).is_ok()
    }

    pub fn is_out_port(&self, name: &str) -> bool {
        self.ports_out.binary_search_by(|p| p.as_str().cmp(name)).is_ok()
    }

    pub fn hier_of(&self, name: &str) -> Option<&LiveHier> {
        self.hier
            .binary_search_by(|h| h.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.hier[i])
    }

    /// Owning real instance of a cell/net name, if any.
    pub fn owner_of(&self, name: &str) -> Option<&str> {
        owner_prefix(name, |p| self.hier_of(p).is_some())
            .and_then(|o| self.hier_of(o))
            .map(|h| h.name.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.nets.is_empty()
    }
}

/// Longest-match owner by the synth `{inst}_` prefix rule (`/` and `.`
/// separators accepted for other front ends): the name itself, then each
/// prefix that ends before a separator, longest first.
fn owner_prefix(name: &str, is_inst: impl Fn(&str) -> bool) -> Option<&str> {
    if is_inst(name) {
        return Some(name);
    }
    name.char_indices()
        .rev()
        .filter(|&(_, ch)| matches!(ch, '_' | '/' | '.'))
        .map(|(i, _)| &name[..i])
        .find(|p| is_inst(p))
}

/// One placed cell: engine name plus its real site/BEL from the placer.
#[derive(Clone, Debug)]
pub struct LivePlacedCell {
    pub cell: String,
    pub site: String,
    pub site_x: u32,
    pub site_y: u32,
    pub site_kind: String,
    pub bel: String,
}

/// Sorted live view of a placement, with real die geometry.
#[derive(Clone, Debug, Default)]
pub struct LivePlaced {
    pub cells: Vec<LivePlacedCell>,
    /// Site (x, y) -> indices into `cells` placed there, in name order.
    pub site_mates: HashMap<(u32, u32), Vec<usize>>,
    pub part: String,
    pub die_cols: u32,
    pub die_rows: u32,
}

/// Real die geometry, cached on the run slots so views never reload the
/// HAD model per frame. Bounds come from enumerating [`helion_device::Device::all_sites`].
#[derive(Clone, Debug, Default)]
pub struct LiveDieGeom {
    pub part: String,
    pub cols: u32,
    pub rows: u32,
    pub x0: u32,
    pub y0: u32,
    pub clb_x0: u32,
    pub clb_y0: u32,
    pub dsp_x: u32,
    pub bram_x: u32,
    pub clk_spine: u32,
    pub interior_cols: u32,
    pub interior_rows: u32,
    pub n_dsp: u32,
    pub dsp_y0: u32,
    pub n_bram: u32,
    pub bram_y0: u32,
}

impl LiveDieGeom {
    pub fn from_device(dev: &helion_device::Device) -> Self {
        let (mut min_x, mut min_y) = (u32::MAX, u32::MAX);
        let (mut max_x, mut max_y) = (0u32, 0u32);
        for s in dev.all_sites() {
            min_x = min_x.min(s.x);
            min_y = min_y.min(s.y);
            max_x = max_x.max(s.x);
            max_y = max_y.max(s.y);
        }
        if min_x == u32::MAX {
            min_x = 0;
            min_y = 0;
        }
        LiveDieGeom {
            part: dev.part.clone(),
            cols: max_x.saturating_sub(min_x) + 1,
            rows: max_y.saturating_sub(min_y) + 1,
            x0: min_x,
            y0: min_y,
            clb_x0: dev.clb_x0,
            clb_y0: dev.clb_y0,
            dsp_x: dev.dsp_x,
            bram_x: dev.bram_x,
            clk_spine: dev.clk_spine,
            interior_cols: dev.interior_cols,
            interior_rows: dev.interior_rows,
            n_dsp: dev.n_dsp,
            dsp_y0: dev.dsp_y0,
            n_bram: dev.n_bram,
            bram_y0: dev.bram_y0,
        }
    }
}

impl LivePlaced {
    pub fn from_placed(
        p: &helion_place::Placed,
        design: &helion_ir::Design,
        geom: &LiveDieGeom,
    ) -> Self {
        let bels = p.cell_bels();
        let mut cells: Vec<LivePlacedCell> = design
            .cells
            .iter()
            .filter_map(|c| {
                let name = helion_ir::resolve(c.name).to_string();
                let bel = bels.get(name.as_str()).cloned().flatten()?;
                let site = bel.site;
                Some(LivePlacedCell {
                    cell: name,
                    site: site.id(),
                    site_x: site.x,
                    site_y: site.y,
                    site_kind: format!("{:?}", site.kind).to_uppercase(),
                    bel: bel.bel.clone(),
                })
            })
            .collect();
        cells.sort_by(|a, b| a.cell.cmp(&b.cell));
        let mut site_mates: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, c) in cells.iter().enumerate() {
            site_mates.entry((c.site_x, c.site_y)).or_default().push(i);
        }
        LivePlaced {
            cells,
            site_mates,
            part: geom.part.clone(),
            die_cols: geom.cols,
            die_rows: geom.rows,
        }
    }

    pub fn site_of(&self, cell: &str) -> Option<&LivePlacedCell> {
        self.cells
            .binary_search_by(|c| c.cell.as_str().cmp(cell))
            .ok()
            .map(|i| &self.cells[i])
    }
}

/// The session's live engine design: the last run result wins, else the Tcl
/// `read_sv`/`synth_design` elaboration. `None` until either exists.
pub fn live_design(s: &AppState) -> Option<&helion_ir::Design> {
    s.runs.live_design.as_ref().or(s.tcl.design.as_ref())
}

/// Identity of the design (and placement) a cached view model was built
/// from: where it lives and its sizes, checked on every call.
#[derive(Clone, Copy, PartialEq, Eq)]
struct BuiltFrom {
    design: usize,
    cells: usize,
    nets: usize,
    placed: usize,
}

fn built_from(d: &helion_ir::Design, placed: Option<&helion_place::Placed>) -> BuiltFrom {
    BuiltFrom {
        design: std::ptr::from_ref(d) as usize,
        cells: d.cells.len(),
        nets: d.nets.len(),
        placed: placed.map_or(0, |p| std::ptr::from_ref(p) as usize),
    }
}

/// Everything `from_design` / `from_placed` read, hashed: an in-place edit
/// (a Tcl rename, an ECO, a new run in the same slot) changes it.
fn fingerprint(d: &helion_ir::Design, placed: Option<(&helion_place::Placed, &LiveDieGeom)>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    d.name.hash(&mut h);
    for p in &d.ports {
        (p.name, p.dir as u8).hash(&mut h);
    }
    for c in &d.cells {
        c.name.hash(&mut h);
        std::mem::discriminant(&c.kind).hash(&mut h);
    }
    for n in &d.nets {
        n.name.hash(&mut h);
        for e in &n.endpoints {
            (e.cell, e.pin).hash(&mut h);
        }
    }
    for i in &d.instances {
        (i.name, i.module, i.conns.len()).hash(&mut h);
    }
    if let Some((p, g)) = placed {
        (&g.part, g.cols, g.rows).hash(&mut h);
        (&p.lutff_sites, &p.iob_sites, &p.mac_sites, &p.bram_sites).hash(&mut h);
        for l in &p.packed.lutffs {
            (&l.lut_cell, &l.ff_cell).hash(&mut h);
        }
        for c in &p.packed.iobs {
            c.cell.hash(&mut h);
        }
        for c in &p.packed.macs {
            c.cell.hash(&mut h);
        }
        for c in &p.packed.brams {
            c.cell.hash(&mut h);
        }
    }
    h.finish()
}

/// A cached view model: its key, content fingerprint, and whether the
/// fingerprint was checked since the last [`begin_frame`].
struct Cached<T> {
    key: BuiltFrom,
    fp: u64,
    checked: bool,
    val: std::rc::Rc<T>,
}

thread_local! {
    static NETLIST: std::cell::RefCell<Option<Cached<LiveNetlist>>> =
        const { std::cell::RefCell::new(None) };
    static PLACED: std::cell::RefCell<Option<Cached<LivePlaced>>> =
        const { std::cell::RefCell::new(None) };
    static STAGE_NETLIST: std::cell::RefCell<Option<Cached<LiveNetlist>>> =
        const { std::cell::RefCell::new(None) };
}

/// Re-check the cached view models' fingerprints on their next use. `view()`
/// calls this once per frame: every pane and table row in a frame shares one
/// check, and an unchanged design is not rebuilt across frames (Ibex took
/// 0.1-0.4 s per view rebuild).
pub fn begin_frame() {
    NETLIST.with(|c| c.borrow_mut().as_mut().map(|e| e.checked = false));
    PLACED.with(|c| c.borrow_mut().as_mut().map(|e| e.checked = false));
}

fn cached<T>(
    cell: &'static std::thread::LocalKey<std::cell::RefCell<Option<Cached<T>>>>,
    key: BuiltFrom,
    fp: impl Fn() -> u64,
    build: impl FnOnce() -> T,
) -> std::rc::Rc<T> {
    cell.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(e) = c.as_mut()
            && e.key == key
            && (e.checked || e.fp == fp())
        {
            e.checked = true;
            return e.val.clone();
        }
        let val = std::rc::Rc::new(build());
        *c = Some(Cached { key, fp: fp(), checked: true, val: val.clone() });
        val
    })
}

/// Sorted live netlist of the session design, if any.
pub fn live_netlist(s: &AppState) -> Option<std::rc::Rc<LiveNetlist>> {
    let d = live_design(s)?;
    Some(cached(&NETLIST, built_from(d, None), || fingerprint(d, None), || LiveNetlist::from_design(d)))
}

/// The netlist the Schematic view draws: the chosen stage of the last
/// implementation run's view contract, else the session design.
pub fn schematic_netlist(s: &AppState) -> Option<std::rc::Rc<LiveNetlist>> {
    let Some(n) = s.schematic.stage.and_then(|st| s.runs.impl_view.as_ref()?.netlist(st)) else {
        return live_netlist(s);
    };
    let key = BuiltFrom { design: std::ptr::from_ref(n) as usize, cells: n.cells.len(), nets: n.nets.len(), placed: n.stage as usize + 1 };
    // The contract is replaced, never edited in place, so its address and sizes identify it.
    Some(cached(&STAGE_NETLIST, key, || 0, || LiveNetlist::from_netlist(n)))
}

/// Sorted live placement of the last implementation run, if any.
pub fn live_placed(s: &AppState) -> Option<std::rc::Rc<LivePlaced>> {
    let placed = s.runs.live_placed.as_ref()?;
    let design = live_design(s)?;
    Some(cached(
        &PLACED,
        built_from(design, Some(placed)),
        || fingerprint(design, Some((placed, &live_geom(s)))),
        || LivePlaced::from_placed(placed, design, &live_geom(s)),
    ))
}

/// Cached run geometry when an impl run finished, else the project part's
/// geometry, else the documented fallback. Views call this per frame; it
/// only loads a HAD model on the no-run-yet path.
pub fn live_geom(s: &AppState) -> LiveDieGeom {
    if let Some(g) = &s.runs.live_geom {
        return g.clone();
    }
    let (dev, _) = live_device(s);
    LiveDieGeom::from_device(&dev)
}

/// Real die geometry for the device canvas: the placed run's part, else the
/// project part, else the documented fallback. The substitution is reported
/// by the caller (device status strip), never silent.
pub fn live_device(s: &AppState) -> (helion_device::Device, String) {
    for part in [&s.runs.live_part, &s.project.part] {
        if !part.is_empty() {
            if let Ok(dev) = helion_device::Device::load_part(part) {
                return (dev, part.clone());
            }
        }
    }
    let dev = helion_device::Device::load_part(FIXTURE_PART)
        .expect("bundled HL10T-C32-1 HAD model must load");
    (dev, FIXTURE_PART.to_string())
}

/// Elaborate + pack + place [`FIXTURE_SRC`] through the real engine into the
/// run slots scenes and tests read. Returns the engine errors instead of
/// faking coordinates when a stage fails.
pub fn seed_live_fixture(s: &mut AppState) -> Result<(), String> {
    let design = helion_sv::synth_sv(FIXTURE_SRC, "fixture.sv")?;
    let dev = helion_device::Device::load_part(FIXTURE_PART)?;
    let packed = helion_pack::pack(&design, &dev)?;
    let placed = helion_place::place(&packed, &dev)?;
    s.runs.live_part = FIXTURE_PART.to_string();
    s.runs.live_geom = Some(LiveDieGeom::from_device(&dev));
    s.runs.live_design = Some(design);
    s.runs.live_placed = Some(placed);
    Ok(())
}

/// URL-safe key slug for a live object name (typed keys, never raw names).
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "n-a".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_design() -> helion_ir::Design {
        helion_sv::synth_sv(FIXTURE_SRC, "fixture.sv").expect("fixture must elaborate")
    }

    #[test]
    fn live_netlist_comes_from_real_synth() {
        let d = fixture_design();
        let live = LiveNetlist::from_design(&d);
        assert_eq!(live.design, "counter_top");
        assert_eq!(
            live.cells.len(),
            d.cells.len(),
            "every engine cell must surface"
        );
        assert_eq!(
            live.nets.len(),
            d.nets.len(),
            "every engine net must surface"
        );
        assert!(
            live.ports_in.iter().any(|p| p == "clk"),
            "ports: {:?}",
            live.ports_in
        );
        assert!(
            live.ports_in.iter().any(|p| p == "rst"),
            "ports: {:?}",
            live.ports_in
        );
        assert!(!live.ports_out.is_empty(), "outputs must surface");
        // Sorted for stable canvas/find order.
        let mut names: Vec<&str> = live.cells.iter().map(|c| c.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "cells must be name-sorted");
        names = live.nets.iter().map(|n| n.name.as_str()).collect();
        sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "nets must be name-sorted");
    }

    #[test]
    fn live_hierarchy_owns_cells_by_synth_prefix() {
        let d = fixture_design();
        let live = LiveNetlist::from_design(&d);
        assert_eq!(
            live.hier.len(),
            d.instances.len(),
            "every engine instance must surface"
        );
        for h in &live.hier {
            assert!(h.cells > 0, "instance {} owns no cells", h.name);
        }
        assert_eq!(live.owner_of("u_div_u_ff0"), Some("u_div"));
        assert_eq!(live.owner_of("u_led_seg_0"), Some("u_led"));
        assert_eq!(
            live.owner_of("u_ff0"),
            None,
            "top-level cells have no owner"
        );
        let insts = ["u_a", "u_a_b", "u.c"];
        let is = |p: &str| insts.contains(&p);
        assert_eq!(owner_prefix("u_a_b_x", is), Some("u_a_b"), "longest instance wins");
        assert_eq!(owner_prefix("u_a_bx", is), Some("u_a"));
        assert_eq!(owner_prefix("u.c/x_y", is), Some("u.c"));
        assert_eq!(owner_prefix("u_a", is), Some("u_a"));
        assert_eq!(owner_prefix("u_ab", is), None);
        let owned: usize = live.hier.iter().map(|h| h.cells).sum();
        assert!(
            owned < live.cells.len(),
            "top level must keep its own cells"
        );
    }

    #[test]
    fn live_fanout_counts_other_endpoints() {
        let d = fixture_design();
        let live = LiveNetlist::from_design(&d);
        for cell in &live.cells {
            let mut expect = 0usize;
            for net in &d.nets {
                let names: Vec<&str> = net
                    .endpoints
                    .iter()
                    .map(|e| helion_ir::resolve(e.cell))
                    .collect();
                if names.contains(&cell.name.as_str()) {
                    expect += net.endpoints.len().saturating_sub(1);
                }
            }
            assert_eq!(cell.fanout, expect, "fanout of {}", cell.name);
        }
        assert!(
            live.cells.iter().any(|c| c.fanout > 0),
            "some cell must drive loads"
        );
    }

    #[test]
    fn live_placed_maps_every_cell_to_a_real_site() {
        let d = fixture_design();
        let dev = helion_device::Device::load_part(FIXTURE_PART).expect("HAD part must load");
        let packed = helion_pack::pack(&d, &dev).expect("fixture must pack");
        let placed = helion_place::place(&packed, &dev).expect("fixture must place");
        let geom = LiveDieGeom::from_device(&dev);
        assert!(geom.cols > 0 && geom.rows > 0, "die bounds must be real");
        let live = LivePlaced::from_placed(&placed, &d, &geom);
        assert_eq!(live.part, FIXTURE_PART);
        assert!(
            live.die_cols > 0 && live.die_rows > 0,
            "die geometry must be real"
        );
        assert!(!live.cells.is_empty(), "placement must surface cells");
        for c in &live.cells {
            assert!(
                !c.site.is_empty() && !c.bel.is_empty(),
                "cell {} lacks site/bel",
                c.cell
            );
            assert!(
                c.site.contains(&format!("X{}Y{}", c.site_x, c.site_y)),
                "site id {} must carry its coordinates",
                c.site
            );
        }
    }

    #[test]
    fn fixture_seed_wires_runs_slots() {
        let mut s = crate::init();
        assert!(
            live_design(&s).is_none(),
            "fresh session has no live design"
        );
        seed_live_fixture(&mut s).expect("fixture seed must run the engine");
        let live = live_netlist(&s).expect("seeded session must surface a netlist");
        assert!(!live.cells.is_empty() && !live.nets.is_empty());
        let placed = live_placed(&s).expect("seeded session must surface a placement");
        assert!(!placed.cells.is_empty());
    }

    #[test]
    fn slug_is_key_safe() {
        assert_eq!(slug("led[15:0]"), "led-15-0");
        assert_eq!(slug("u_div/u_ff0"), "u-div-u-ff0");
        assert_eq!(slug("clk"), "clk");
    }
}
