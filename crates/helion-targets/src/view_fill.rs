//! Pieces the family fillers share: the schematic stages from a Helion IR
//! netlist, setup timing over a placed LUT/flop graph, congestion levels
//! over the usage grid, control sets and high fanout nets.
//!
//! No em dashes anywhere.

use std::collections::{BTreeMap, HashMap, HashSet};

use helion_backend::TimingModel;
use helion_ir::Design;

use crate::view::{
    CellPlacement, CongestionWindow, ControlSet, EndpointSlack, HighFanoutNet, ImplView, NetlistCell, NetlistPin, NetlistStage, Provenance, SchematicNetlist,
    TimingPath, TimingSummary,
};

/// The three schematic netlists of a run: elaborated (when the front end
/// kept one, else `rtl_missing` says why not), synthesized, and synthesized
/// with the placement's site and BEL on each cell. `engines` names the
/// front end and the placer for the provenance.
pub fn fill_schematic(
    view: &mut ImplView,
    rtl: Result<&Design, &str>,
    design: &Design,
    kind: impl Fn(&helion_ir::Cell, &[NetlistPin]) -> String + Copy,
    is_output: impl Fn(&helion_ir::Cell, &str) -> bool + Copy,
    engines: (&str, &str),
    scope: &str,
) {
    match rtl {
        Ok(r) => {
            view.schematic.push(SchematicNetlist::from_ir(r, NetlistStage::Rtl, kind, is_output));
            view.set_provenance(
                NetlistStage::Rtl.section(),
                Provenance::new(engines.0, "elaborated word-level netlist before bit blasting: one cell per operator, register and constant", scope),
            );
        }
        Err(why) => view.mark_missing(NetlistStage::Rtl.section(), why),
    }
    let syn = SchematicNetlist::from_ir(design, NetlistStage::Synthesized, kind, is_output);
    view.set_provenance(
        NetlistStage::Synthesized.section(),
        Provenance::new(engines.0, "the mapped netlist the packer reads (LUTs, flops, carry chains, IO and clock buffers)", scope),
    );
    let mut imp = syn.clone();
    imp.stage = NetlistStage::Implemented;
    let at: HashMap<&str, &CellPlacement> = view.cells.iter().map(|c| (c.cell.as_str(), c)).collect();
    let mut placed = 0usize;
    for c in imp.cells.iter_mut() {
        if let Some(p) = at.get(c.name.as_str()) {
            c.site = Some(p.site.clone());
            c.bel = p.bel.clone();
            placed += 1;
        }
    }
    let names: HashSet<&str> = syn.cells.iter().map(|c| c.name.as_str()).collect();
    let mut added = 0usize;
    for p in view.cells.iter().filter(|p| !names.contains(p.cell.as_str())) {
        imp.cells.push(NetlistCell { name: p.cell.clone(), kind: p.kind.clone(), pins: Vec::new(), site: Some(p.site.clone()), bel: p.bel.clone() });
        added += 1;
    }
    view.set_provenance(
        NetlistStage::Implemented.section(),
        Provenance::new(
            engines.1,
            format!(
                "synthesized netlist with the placement's site and BEL: {placed} of {} cells placed under their own name; {added} cells the packer made (buffers, split LUTs) carry a site but no pins",
                syn.cells.len()
            ),
            scope,
        ),
    );
    view.schematic.push(syn);
    view.schematic.push(imp);
}

/// Whether a pin of a Helion IR cell drives its net: the cell's `OUT_PINS`
/// list when it has one, else the usual output pin names.
// shortcut: name-based for primitives without OUT_PINS; a new primitive with other output names reads as all inputs until it is listed here.
pub fn is_output_pin(c: &helion_ir::Cell, pin: &str) -> bool {
    if let Some(outs) = c.attrs.get("OUT_PINS") {
        return outs.split(',').any(|o| o == pin);
    }
    if matches!(c.kind, helion_ir::CellKind::IobOut) {
        return pin == "PAD";
    }
    let base = pin.split('[').next().unwrap_or(pin).trim_end_matches(|ch: char| ch.is_ascii_digit());
    matches!(base, "O" | "Q" | "CO" | "P" | "PCOUT" | "ACOUT" | "BCOUT" | "CARRYOUT" | "DOADO" | "DOBDO" | "DOPADOP" | "DOPBDOP" | "LOCKED" | "CLKOUT" | "CLKFBOUT")
}

/// A family's name for a synthesized cell: `names` gives the flop, input
/// pad, output pad, clock buffer, block RAM, multiplier and carry
/// primitives; a LUT is `LUT<inputs>`.
pub fn primitive_name(c: &helion_ir::Cell, pins: &[NetlistPin], names: [&str; 7]) -> String {
    use helion_ir::CellKind as K;
    match &c.kind {
        K::Lut6 { .. } => format!("LUT{}", pins.iter().filter(|p| !p.output && p.name.starts_with('I')).count().max(1)),
        K::Hff => names[0].into(),
        K::IobIn => names[1].into(),
        K::IobOut => names[2].into(),
        K::ClockBuf => names[3].into(),
        K::Bram18 | K::Bram18e1 { .. } => names[4].into(),
        K::Mac27 | K::Dsp48e1 { .. } => names[5].into(),
        K::Carry4 => names[6].into(),
        K::Ila { .. } => "ILA".into(),
        K::BlackBox { module } => helion_ir::resolve(*module).to_string(),
    }
}

/// One placed LUT of the timing graph.
#[derive(Clone, Debug)]
pub struct GraphLut {
    pub name: String,
    /// Routed input nets (a dedicated carry input is `carry_prev`, not here).
    pub inputs: Vec<String>,
    pub output: String,
    /// Index of the LUT whose carry out feeds this one's carry in.
    pub carry_prev: Option<usize>,
    pub xy: (u32, u32),
}

/// One placed flop of the timing graph.
#[derive(Clone, Debug)]
pub struct GraphFf {
    pub name: String,
    pub d: String,
    pub q: String,
    pub clk: String,
    pub xy: (u32, u32),
    pub site: String,
}

/// Setup timing over a placed LUT/flop graph with a family delay model.
///
/// A path starts at a flop's Q (clock to Q) or at any other net (a pad, a
/// RAM or DSP output; arrival 0, as IO paths are unconstrained) and ends at
/// a flop's D. A LUT costs its arc plus its per-input loading; a connection
/// costs one routing hop per tile of Manhattan distance between the placed
/// driver and load; a carry step costs the carry arc. `period_ps` gives the
/// period of a flop's clock net, `None` for an unconstrained clock (its
/// flops are not endpoints). Returns `None` when no flop is constrained.
pub fn graph_timing(luts: &[GraphLut], ffs: &[GraphFf], tm: &TimingModel, period_ps: &dyn Fn(&str) -> Option<u32>) -> Option<TimingSummary> {
    #[derive(Clone, Copy)]
    enum Src {
        Lut(usize),
        Ff(usize),
    }
    let mut driver: HashMap<&str, Src> = HashMap::new();
    for (i, l) in luts.iter().enumerate() {
        driver.insert(l.output.as_str(), Src::Lut(i));
    }
    // A flop's Q wins over the LUT packed with it: the net leaves from the flop.
    for (i, f) in ffs.iter().enumerate() {
        driver.insert(f.q.as_str(), Src::Ff(i));
    }
    let dist = |a: (u32, u32), b: (u32, u32)| (a.0.abs_diff(b.0) + a.1.abs_diff(b.1)) as i64;
    // Per LUT: (output arrival, carry-out arrival, start point of the worst path).
    let mut arr: Vec<Option<(i64, i64, Option<usize>)>> = vec![None; luts.len()];
    let mut busy = vec![false; luts.len()];
    // Arrival of `net` at a load placed at `at`, and the flop it started from.
    let at_load = |net: &str, at: (u32, u32), arr: &[Option<(i64, i64, Option<usize>)>]| -> (i64, Option<usize>) {
        match driver.get(net) {
            Some(Src::Ff(j)) => (tm.ff_ckq_ps + dist(ffs[*j].xy, at) * tm.hop_ps, Some(*j)),
            Some(Src::Lut(j)) => arr[*j].map(|a| (a.0 + dist(luts[*j].xy, at) * tm.hop_ps, a.2)).unwrap_or((0, None)),
            None => (0, None),
        }
    };
    for root in 0..luts.len() {
        let mut stack = vec![root];
        while let Some(&i) = stack.last() {
            if arr[i].is_some() {
                stack.pop();
                continue;
            }
            busy[i] = true;
            let l = &luts[i];
            // A LUT still being computed below is a combinational loop: it reads as settled at 0.
            let pending: Vec<usize> = l
                .inputs
                .iter()
                .filter_map(|n| match driver.get(n.as_str()) {
                    Some(Src::Lut(j)) => Some(*j),
                    _ => None,
                })
                .chain(l.carry_prev)
                .filter(|&j| arr[j].is_none() && !busy[j])
                .collect();
            if !pending.is_empty() {
                stack.extend(pending);
                continue;
            }
            let mut worst: (i64, Option<usize>) = (0, None);
            for n in &l.inputs {
                let a = at_load(n, l.xy, &arr);
                if a.0 > worst.0 || (worst.1.is_none() && a.0 == worst.0) {
                    worst = a;
                }
            }
            let cin = l.carry_prev.and_then(|p| arr[p]).map(|a| (a.1, a.2));
            let through = match cin {
                Some(c) if c.0 > worst.0 => c,
                _ => worst,
            };
            let out = through.0 + tm.lut_ps + l.inputs.len() as i64 * tm.pin_ps;
            arr[i] = Some((out, through.0 + tm.carry_ps, through.1));
            stack.pop();
        }
    }
    let mut endpoints = Vec::new();
    let mut worst: Option<(i64, usize, Option<usize>, u32)> = None;
    for (i, f) in ffs.iter().enumerate() {
        let Some(period) = period_ps(&f.clk) else { continue };
        let (a, start) = at_load(&f.d, f.xy, &arr);
        let slack = period as i64 - a - tm.setup_ps;
        endpoints.push(EndpointSlack { endpoint: format!("{}.D", f.name), slack_ps: slack, clock: f.clk.clone(), site: Some(f.site.clone()), x: Some(f.xy.0), y: Some(f.xy.1) });
        if worst.is_none_or(|w| slack < w.0) {
            worst = Some((slack, i, start, period));
        }
    }
    let (wns, end, start, period) = worst?;
    Some(TimingSummary {
        wns_ps: wns,
        tns_ps: endpoints.iter().map(|e| e.slack_ps.min(0)).sum(),
        whs_ps: None,
        ths_ps: None,
        failing_endpoints: endpoints.iter().filter(|e| e.slack_ps < 0).count(),
        total_endpoints: endpoints.len(),
        worst_path: Some(TimingPath {
            from: start.map(|j| format!("{}.Q", ffs[j].name)).unwrap_or_else(|| "an unclocked source (pad, RAM or constant)".into()),
            to: format!("{}.D", ffs[end].name),
            clock: ffs[end].clk.clone(),
            slack_ps: wns,
            requirement_ps: period as i64,
            kind: "setup",
        }),
        worst_hold_path: None,
        endpoints,
        fmax_mhz: (period as i64 - wns > 0).then(|| 1.0e6 / (period as i64 - wns) as f64),
    })
}

/// The method note for [`graph_timing`] on a family model.
pub fn graph_timing_method(model: &str) -> String {
    format!(
        "setup over the placed LUT and flop graph with the {model}: clock to Q, LUT arc plus per-input loading, carry arc, and one routing hop per tile of Manhattan distance between placed driver and load (the router's own wire delays are not exported); paths from pads, RAMs and DSPs start at 0; ideal clock"
    )
}

/// Why a family filled by [`graph_timing`] has no hold numbers.
pub const NO_HOLD: &str = "the family delay model has one corner (no fast-corner delays), so no hold check is run";

/// Congestion level per direction over every tile of the usage grid.
pub fn fill_congestion(view: &mut ImplView, engine: &str, threshold_pct: f32) {
    let name_at: HashMap<(u32, u32), &str> = view.tiles.iter().map(|t| ((t.x, t.y), t.name.as_str())).collect();
    let mut h: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    let mut v: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for u in &view.usage {
        if let Some(o) = u.overflow {
            h.insert((u.x, u.y), (o.h_used, o.h_capacity));
            v.insert((u.x, u.y), (o.v_used, o.v_capacity));
        }
    }
    let corner = |x: u32, y: u32| name_at.get(&(x, y)).map(|s| s.to_string()).unwrap_or_else(|| format!("X{x}Y{y}"));
    view.congestion = [("Horizontal", &h), ("Vertical", &v)]
        .into_iter()
        .filter_map(|(dir, g)| {
            let (level, percent, (x0, y0, x1, y1)) = crate::view::congestion_level(g, threshold_pct, 7)?;
            Some(CongestionWindow { direction: dir, level, percent, from_tile: corner(x0, y0), to_tile: corner(x1, y1), overlap: percent > 100.0 })
        })
        .collect();
    view.set_provenance(
        "congestion",
        Provenance::new(
            engine,
            format!("routed wires per tile by direction against the tile's wires of that direction; level = largest 2^l x 2^l tile window (l <= 7) whose summed use over summed capacity reaches {threshold_pct}% (the 7-series threshold, applied to this family unchanged), level 0 reports the hottest single tile"),
            "routed design of this run",
        ),
    );
}

/// Control sets from `(clock, enable, set/reset, set/reset kind, flop)`.
pub fn fill_control_sets(view: &mut ImplView, flops: impl Iterator<Item = (String, String, String, String, String)>, engine: &str, method: &str) {
    let mut m: BTreeMap<(String, String, String, String), Vec<String>> = BTreeMap::new();
    for (c, e, s, k, f) in flops {
        m.entry((c, e, s, k)).or_default().push(f);
    }
    let mut sets: Vec<ControlSet> = m.into_iter().map(|(key, flops)| ControlSet { key, flops }).collect();
    sets.sort_by(|a, b| b.flops.len().cmp(&a.flops.len()).then(a.key.cmp(&b.key)));
    view.control_sets = sets;
    view.set_provenance("control_sets", Provenance::new(engine, method, "routed design of this run"));
}

/// The `max_nets` nets with the most loads, from the view's routed nets.
pub fn fill_high_fanout(view: &mut ImplView, clocks: &HashSet<&str>, max_nets: usize, engine: &str) {
    let mut v: Vec<HighFanoutNet> = view.nets.iter().map(|n| HighFanoutNet { net: n.net.clone(), fanout: n.loads.len(), is_clock: clocks.contains(n.net.as_str()) }).collect();
    v.sort_by(|a, b| b.fanout.cmp(&a.fanout).then(a.net.cmp(&b.net)));
    v.truncate(max_nets);
    view.high_fanout = v;
    view.set_provenance("high_fanout", Provenance::new(engine, format!("the {max_nets} nets with the most load pins in the placed netlist"), "routed design of this run"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm() -> TimingModel {
        TimingModel { lut_ps: 100, ff_ckq_ps: 50, setup_ps: 30, pin_ps: 10, iob_ps: 0, hop_ps: 20, hold_req_ps: 0, carry_ps: 5 }
    }

    fn ff(name: &str, d: &str, q: &str, clk: &str, xy: (u32, u32)) -> GraphFf {
        GraphFf { name: name.into(), d: d.into(), q: q.into(), clk: clk.into(), xy, site: format!("S{}_{}", xy.0, xy.1) }
    }

    fn lut(name: &str, inputs: &[&str], output: &str, xy: (u32, u32)) -> GraphLut {
        GraphLut { name: name.into(), inputs: inputs.iter().map(|s| s.to_string()).collect(), output: output.into(), carry_prev: None, xy }
    }

    #[test]
    fn graph_timing_adds_depth_distance_and_carry_and_times_every_flop() {
        // a.Q -> l0 -> l1 -> b.D, one tile apart at each step; c.D straight from a.Q.
        let luts = vec![lut("l0", &["qa", "pad"], "n0", (1, 0)), lut("l1", &["n0"], "n1", (2, 0))];
        let ffs = vec![ff("a", "x", "qa", "clk", (0, 0)), ff("b", "n1", "qb", "clk", (3, 0)), ff("c", "qa", "qc", "clk", (0, 2)), ff("u", "n1", "qu", "other", (9, 9))];
        let period = |c: &str| (c == "clk").then_some(1000u32);
        let t = graph_timing(&luts, &ffs, &tm(), &period).expect("timing");
        // b: 50 ckq + 20 hop + (100 + 2*10) l0 + 20 hop + (100 + 10) l1 + 20 hop + 30 setup = 370.
        let slack = |e: &str| t.endpoints.iter().find(|x| x.endpoint == e).map(|x| x.slack_ps);
        assert_eq!(slack("b.D"), Some(630));
        // c: 50 + 2 hops (40) + 30.
        assert_eq!(slack("c.D"), Some(880));
        // a.D comes from a net nothing in the graph drives: arrival 0.
        assert_eq!(slack("a.D"), Some(970));
        assert_eq!(slack("u.D"), None, "a flop on an unconstrained clock is not an endpoint");
        assert_eq!((t.wns_ps, t.total_endpoints, t.failing_endpoints, t.tns_ps), (630, 3, 0, 0));
        let p = t.worst_path.as_ref().unwrap();
        assert_eq!((p.from.as_str(), p.to.as_str(), p.requirement_ps), ("a.Q", "b.D", 1000));
        assert_eq!(t.endpoints.iter().find(|x| x.endpoint == "b.D").map(|x| (x.x, x.y)), Some((Some(3), Some(0))));
        assert!(t.whs_ps.is_none(), "one corner: no hold");
        // A tighter clock fails the long path only.
        let fast = |c: &str| (c == "clk").then_some(300u32);
        let t = graph_timing(&luts, &ffs, &tm(), &fast).unwrap();
        assert_eq!((t.wns_ps, t.failing_endpoints, t.tns_ps), (-70, 1, -70));
        // A carry step costs the carry arc, not a LUT.
        let mut chain = vec![lut("c0", &["qa"], "s0", (0, 0)), lut("c1", &[], "s1", (0, 0))];
        chain[1].carry_prev = Some(0);
        let ffs = vec![ff("a", "x", "qa", "clk", (0, 0)), ff("b", "s1", "qb", "clk", (0, 0))];
        let t = graph_timing(&chain, &ffs, &tm(), &period).unwrap();
        // 50 ckq, carry out of c0 at 55, c1 output 55 + 100, setup 30.
        assert_eq!(t.wns_ps, 1000 - (50 + 5 + 100 + 30));
        assert!(graph_timing(&luts, &[], &tm(), &period).is_none(), "no flop, no timing");
    }

    #[test]
    fn a_combinational_loop_does_not_hang_the_timing_graph() {
        let luts = vec![lut("l0", &["n1"], "n0", (0, 0)), lut("l1", &["n0"], "n1", (0, 0))];
        let ffs = vec![ff("a", "n1", "qa", "clk", (0, 0))];
        assert!(graph_timing(&luts, &ffs, &tm(), &|_| Some(1000)).is_some());
    }
}
