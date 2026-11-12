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
                LiveCell {
                    fanout: touch.get(&name).copied().unwrap_or(0),
                    name,
                    kind: LiveKind::from_cell(&c.kind),
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
        let inst_names: Vec<String> = d
            .instances
            .iter()
            .map(|i| helion_ir::resolve(i.name).to_string())
            .collect();
        let hier: Vec<LiveHier> = d
            .instances
            .iter()
            .map(|i| {
                let name = helion_ir::resolve(i.name).to_string();
                let owned = cells
                    .iter()
                    .filter(|c| owner_of(&inst_names, &c.name).as_deref() == Some(&name))
                    .count();
                LiveHier {
                    name,
                    module: helion_ir::resolve(i.module).to_string(),
                    conns: i.conns.len(),
                    cells: owned,
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

    pub fn hier_of(&self, name: &str) -> Option<&LiveHier> {
        self.hier
            .binary_search_by(|h| h.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.hier[i])
    }

    /// Owning real instance of a cell/net name, if any.
    pub fn owner_of(&self, name: &str) -> Option<&str> {
        let names: Vec<String> = self.hier.iter().map(|h| h.name.clone()).collect();
        owner_of(&names, name).and_then(|o| {
            self.hier
                .iter()
                .find(|h| h.name == o)
                .map(|h| h.name.as_str())
        })
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && self.nets.is_empty()
    }
}

/// Longest-match owner by the synth `{inst}_` prefix rule (`/` and `.`
/// separators accepted for other front ends).
fn owner_of(instances: &[String], name: &str) -> Option<String> {
    let mut best: Option<&str> = None;
    for inst in instances {
        let owned = name == inst
            || name.starts_with(&format!("{inst}_"))
            || name.starts_with(&format!("{inst}/"))
            || name.starts_with(&format!("{inst}."));
        if owned && best.is_none_or(|b: &str| inst.len() > b.len()) {
            best = Some(inst);
        }
    }
    best.map(str::to_string)
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
        let mut cells: Vec<LivePlacedCell> = design
            .cells
            .iter()
            .filter_map(|c| {
                let name = helion_ir::resolve(c.name).to_string();
                let bel = p.cell_bel(&name)?;
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
        LivePlaced {
            cells,
            part: geom.part.clone(),
            die_cols: geom.cols,
            die_rows: geom.rows,
        }
    }

    pub fn site_of(&self, cell: &str) -> Option<&LivePlacedCell> {
        self.cells.iter().find(|c| c.cell == cell)
    }
}

/// The session's live engine design: the last run result wins, else the Tcl
/// `read_sv`/`synth_design` elaboration. `None` until either exists.
pub fn live_design(s: &AppState) -> Option<&helion_ir::Design> {
    s.runs.live_design.as_ref().or(s.tcl.design.as_ref())
}

/// Identity of the design (and placement) a cached view model was built
/// from. Cheap to compute; the per-frame reset in [`begin_frame`] covers
/// in-place edits that keep the same allocation and sizes.
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

thread_local! {
    static NETLIST: std::cell::RefCell<Option<(BuiltFrom, std::rc::Rc<LiveNetlist>)>> =
        const { std::cell::RefCell::new(None) };
    static PLACED: std::cell::RefCell<Option<(BuiltFrom, std::rc::Rc<LivePlaced>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Drop the cached view models. `view()` calls this once per frame, so every
/// pane and table row in a frame shares one build (a 19k-cell design used to
/// be rebuilt once per I/O port row, every frame).
pub fn begin_frame() {
    NETLIST.with(|c| c.borrow_mut().take());
    PLACED.with(|c| c.borrow_mut().take());
}

/// Sorted live netlist of the session design, if any (shared within a frame).
pub fn live_netlist(s: &AppState) -> Option<std::rc::Rc<LiveNetlist>> {
    let d = live_design(s)?;
    let key = built_from(d, None);
    NETLIST.with(|c| {
        if let Some((k, l)) = c.borrow().as_ref()
            && *k == key
        {
            return Some(l.clone());
        }
        let l = std::rc::Rc::new(LiveNetlist::from_design(d));
        *c.borrow_mut() = Some((key, l.clone()));
        Some(l)
    })
}

/// Sorted live placement of the last implementation run, if any (shared
/// within a frame).
pub fn live_placed(s: &AppState) -> Option<std::rc::Rc<LivePlaced>> {
    let placed = s.runs.live_placed.as_ref()?;
    let design = live_design(s)?;
    let key = built_from(design, Some(placed));
    PLACED.with(|c| {
        if let Some((k, l)) = c.borrow().as_ref()
            && *k == key
        {
            return Some(l.clone());
        }
        let geom = live_geom(s);
        let l = std::rc::Rc::new(LivePlaced::from_placed(placed, design, &geom));
        *c.borrow_mut() = Some((key, l.clone()));
        Some(l)
    })
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
