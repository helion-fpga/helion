//! UltraScale+ `helion impl` flow (xcku3p / xcvu3p).
//!
//! Synth (helion-sv) -> pack -> place -> legality -> preliminary STA ->
//! route -> bitgen. Every stage that is not implemented for UltraScale+
//! fails loud with the stage name; there is no silent fallback to another
//! part and no empty bitstream file.
//!
//! Current honest state (see COORD.md gate):
//! - SYNTH, PACK, PLACE, LEGALITY, STA (preliminary) run.
//! - ROUTE is blocked: the fabric WireDb is empty (no routing graph yet).
//! - BITGEN is not implemented (bitgen-core owns emit_bitstream).
//! - INT_PIP emission is blocked on the WireDb naming seam (proposal in
//!   COORD.md, needs CTO ruling).
//! - Clock nets are phase 2 for the router (it rejects them loudly) and
//!   IOBs have no placed site in this flow yet; both are recorded in the
//!   gate, not hidden.

use helion_ir::{intern, resolve, CellKind, Design};
use helion_ultrascale::{
    check_legality, pack_ultra, place_ultra, preliminary_xcvu3p, route_ultra, sta_routed,
    ControlSetLimits, FabricWithPlaceholders, RoutableNet, StaNetlist, TimingReport,
    UltraPlaceOpts, UltraRouteOpts, UltraScaleDevice, UltraScaleFabric,
};
use std::collections::{HashMap, HashSet};

/// True for the UltraScale+ parts this flow owns.
pub fn is_ultra_part(part: &str) -> bool {
    matches!(part, "xcku3p" | "xcvu3p")
}

/// Packer inputs extracted from a synthesized [`Design`].
///
/// Same extraction logic as the pack-place acceptance test
/// (`crates/helion-ultrascale/tests/corpus_legality.rs`); the two copies
/// are kept in sync by that test. The CLI copy lives here because
/// helion-ultrascale does not depend on helion-ir.
pub struct ExtractedCells {
    pub luts: Vec<(String, u64, bool)>,
    pub ffs: Vec<(String, String, Option<String>, Option<String>)>,
    pub carry_chains: Vec<Vec<String>>,
}

pub fn extract_cells(design: &Design) -> Result<ExtractedCells, String> {
    let mut luts = Vec::new();
    let mut ffs = Vec::new();
    let mut carry_cells: Vec<String> = Vec::new();
    // Carry4 cell -> CI net name, if any.
    let mut carry_ci: HashMap<String, Option<String>> = HashMap::new();
    // Net name -> Carry4 cell driving it through CO3/CO[3].
    let mut co3_driver: HashMap<String, String> = HashMap::new();

    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    let net_of = |cell: helion_ir::Spur, pin: &str| -> Option<String> {
        pin_to_net
            .get(&(cell, intern(pin)))
            .map(|s| resolve(*s).to_string())
    };

    for cell in &design.cells {
        let name = resolve(cell.name).to_string();
        match cell.kind {
            CellKind::Lut6 { init } => {
                let is_mem = cell.attrs.get("MEMORY").map(|s| s == "1").unwrap_or(false);
                luts.push((name, init, is_mem));
            }
            CellKind::Hff => {
                let clk = net_of(cell.name, "CLK").ok_or_else(|| {
                    format!("ULTRA PACK: FF {name} has no CLK pin net")
                })?;
                let ce = net_of(cell.name, "CE");
                let sr = net_of(cell.name, "SR");
                ffs.push((name, clk, ce, sr));
            }
            CellKind::Carry4 => {
                carry_cells.push(name.clone());
                carry_ci.insert(name.clone(), net_of(cell.name, "CI"));
                for pin in ["CO3", "CO[3]"] {
                    if let Some(net) = net_of(cell.name, pin) {
                        co3_driver.insert(net, name.clone());
                    }
                }
            }
            CellKind::IobIn | CellKind::IobOut | CellKind::ClockBuf => {}
            CellKind::BlackBox { module } => {
                return Err(format!(
                    "ULTRA PACK: black-box instance {} ({}) is not supported for UltraScale+",
                    name,
                    resolve(module)
                ));
            }
            _ => {
                return Err(format!(
                    "ULTRA PACK: cell {name} of kind {:?} is not supported for UltraScale+",
                    cell.kind
                ));
            }
        }
    }

    // Build CARRY8 chains from Carry4 CI/CO3 links. Chain heads are cells
    // whose CI net is not driven by another Carry4's CO3 (CYINIT or a
    // constant). One Carry4 occupies one CARRY8 slice in this flow (the
    // upper 4 bits unused); the pack-place worker owns the 2:1 mapping.
    let mut carry_chains: Vec<Vec<String>> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    for cell in &carry_cells {
        let is_head = match carry_ci.get(cell) {
            Some(Some(ci_net)) => !co3_driver.contains_key(ci_net),
            _ => true,
        };
        if !is_head || visited.contains(cell) {
            continue;
        }
        let mut chain = vec![cell.clone()];
        visited.insert(cell.clone());
        let mut cur = cell.clone();
        loop {
            let mut next = None;
            for cand in &carry_cells {
                if visited.contains(cand) {
                    continue;
                }
                if let Some(Some(ci_net)) = carry_ci.get(cand) {
                    if co3_driver.get(ci_net) == Some(&cur) {
                        next = Some(cand.clone());
                        break;
                    }
                }
            }
            match next {
                Some(n) => {
                    chain.push(n.clone());
                    visited.insert(n.clone());
                    cur = n;
                }
                None => break,
            }
        }
        carry_chains.push(chain);
    }
    // Any Carry4 not visited is part of a cycle or a broken link.
    for cell in &carry_cells {
        if !visited.contains(cell) {
            return Err(format!(
                "ULTRA PACK: Carry4 {cell} is not reachable from a chain head (cycle or broken CI link)"
            ));
        }
    }

    Ok(ExtractedCells {
        luts,
        ffs,
        carry_chains,
    })
}

/// Placed/routed netlist view for preliminary STA.
struct UltraStaNetlist<'a> {
    design: &'a Design,
    /// cell name -> (x, y) for LUTs and FFs.
    sites: HashMap<String, (i32, i32)>,
    /// net name -> (driver cell, x, y). Driver cell is "ff:<name>" for FF Q.
    drivers: HashMap<String, (String, i32, i32)>,
    /// cell name -> input net names.
    fanin: HashMap<String, Vec<String>>,
    ff_q: HashMap<String, String>,
    ff_d: HashMap<String, String>,
}

impl<'a> StaNetlist for UltraStaNetlist<'a> {
    fn flip_flops(&self) -> Vec<(String, i32, i32)> {
        self.design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .filter_map(|c| {
                let name = resolve(c.name).to_string();
                self.sites.get(&name).map(|(x, y)| (name, *x, *y))
            })
            .collect()
    }

    fn luts(&self) -> Vec<(String, i32, i32)> {
        self.design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .filter_map(|c| {
                let name = resolve(c.name).to_string();
                self.sites.get(&name).map(|(x, y)| (name, *x, *y))
            })
            .collect()
    }

    fn net_drivers(&self) -> HashMap<String, (String, i32, i32)> {
        self.drivers.clone()
    }

    fn cell_fanin(&self) -> HashMap<String, Vec<String>> {
        self.fanin.clone()
    }

    fn ff_q_net(&self, ff: &str) -> Option<String> {
        self.ff_q.get(ff).cloned()
    }

    fn ff_d_net(&self, ff: &str) -> Option<String> {
        self.ff_d.get(ff).cloned()
    }
}

/// Run `helion impl` for an UltraScale+ part. Prints hang_diag lines and
/// returns Err naming the first blocking stage.
pub fn run_impl(path: &str, part: &str, seed: u64) -> Result<(), String> {
    let design = super::synth_any(path)?;
    let name = resolve(design.name).to_string();
    let device = match part {
        "xcku3p" => UltraScaleDevice::xcku3p(),
        "xcvu3p" => UltraScaleDevice::xcvu3p(),
        _ => return Err(format!("ULTRA: unsupported part {part}")),
    };
    let fabric = match part {
        "xcku3p" => FabricWithPlaceholders::xcku3p(),
        _ => FabricWithPlaceholders::xcvu3p(),
    };

    // SYNTH is done (synth_any above); extract packer inputs.
    let cells = extract_cells(&design)?;
    println!(
        "hang_diag ultra synth {} part={} luts={} ffs={} carry_chains={}",
        name,
        part,
        cells.luts.len(),
        cells.ffs.len(),
        cells.carry_chains.len()
    );

    // PACK.
    let limits = ControlSetLimits::default();
    let packed = pack_ultra(&cells.luts, &cells.ffs, &cells.carry_chains, &limits)
        .map_err(|e| format!("ULTRA PACK: {e}"))?;
    println!(
        "hang_diag ultra pack slices={} slicel={} slicem={} luts={} ffs={} carry8={}",
        packed.slices.len(),
        packed.stats.slicel_count,
        packed.stats.slicem_count,
        packed.stats.lut_count,
        packed.stats.ff_count,
        packed.stats.carry8_count
    );

    // PLACE.
    let placed = place_ultra(
        &packed.slices,
        &device,
        &UltraPlaceOpts {
            seed,
            timing_weight: 0.0,
        },
    )
    .map_err(|e| format!("ULTRA PLACE: {e}"))?;
    println!(
        "hang_diag ultra place slices={} seed={}",
        placed.slices.len(),
        placed.seed
    );

    // LEGALITY.
    let violations = check_legality(&packed.slices, &placed.slices, &device, &limits);
    if !violations.is_empty() {
        let mut msg = String::from("ULTRA LEGALITY: placement illegal:\n");
        for v in &violations {
            msg.push_str(&format!("  {v}\n"));
        }
        return Err(msg);
    }
    println!("hang_diag ultra legality ok violations=0");

    // Preliminary STA on the placed netlist (Manhattan estimates, ballpark
    // delays; NOT signoff timing).
    let timing = sta_on_placed(&design, &packed, &placed)?;
    println!("{}", timing.hang_diag_line(0));

    // ROUTE: blocked until a real WireDb exists. Fail loud with the stage
    // name; do not pretend routing happened.
    if fabric.wires().wire_count() == 0 {
        return Err(
            "ULTRA ROUTE: WireDb is empty (no routing graph; routing-graph worker owns it). \
             Clock-net routing (phase 2) and IOB placement are also pending; see COORD.md gate."
                .to_string(),
        );
    }
    let nets = build_routable_nets(&design, &packed, &placed)?;
    let routed = route_ultra(
        &nets,
        fabric.wires(),
        UltraRouteOpts {
            seed,
            max_iters: 50,
        },
    )
    .map_err(|e| format!("ULTRA ROUTE: {e}"))?;
    println!(
        "hang_diag ultra route nets={} iters={} overused={}",
        routed.routes.len(),
        routed.iters_used,
        routed.overused_nodes
    );

    // BITGEN: not implemented. bitgen-core owns emit_bitstream; the flow
    // stops here rather than writing an empty or placeholder bitstream.
    Err("ULTRA BITGEN: UltraScale+ bitgen not implemented (bitgen-core owns emit_bitstream; INT_PIP emission also awaits the WireDb naming seam, see COORD.md)".to_string())
}

/// Preliminary STA over the placed design.
fn sta_on_placed(
    design: &Design,
    packed: &helion_ultrascale::UltraPacked,
    placed: &helion_ultrascale::UltraPlaced,
) -> Result<TimingReport, String> {
    // Map cell name -> placed (x, y) via its packed slice.
    let mut sites: HashMap<String, (i32, i32)> = HashMap::new();
    for p in &placed.slices {
        let Some(slice) = packed.slices.get(p.packed_idx) else {
            continue;
        };
        for (_, cell) in slice.luts.iter().chain(slice.ffs.iter()) {
            sites.insert(cell.clone(), (p.x, p.y));
        }
    }
    // Pin -> net map.
    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    let net_of = |cell: helion_ir::Spur, pin: &str| -> Option<String> {
        pin_to_net
            .get(&(cell, intern(pin)))
            .map(|s| resolve(*s).to_string())
    };

    let mut drivers: HashMap<String, (String, i32, i32)> = HashMap::new();
    let mut fanin: HashMap<String, Vec<String>> = HashMap::new();
    let mut ff_q: HashMap<String, String> = HashMap::new();
    let mut ff_d: HashMap<String, String> = HashMap::new();
    for cell in &design.cells {
        let cname = resolve(cell.name).to_string();
        let Some(&(x, y)) = sites.get(&cname) else {
            continue;
        };
        match cell.kind {
            CellKind::Lut6 { .. } => {
                if let Some(o) = net_of(cell.name, "O") {
                    drivers.insert(o, (cname.clone(), x, y));
                }
                let mut ins = Vec::new();
                for pin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                    if let Some(n) = net_of(cell.name, pin) {
                        ins.push(n);
                    }
                }
                fanin.insert(cname, ins);
            }
            CellKind::Hff => {
                if let Some(q) = net_of(cell.name, "Q") {
                    drivers.insert(q.clone(), (format!("ff:{cname}"), x, y));
                    ff_q.insert(cname.clone(), q);
                }
                if let Some(d) = net_of(cell.name, "D") {
                    ff_d.insert(cname.clone(), d);
                }
            }
            _ => {}
        }
    }
    // Nets driven by top-level input ports (e.g. clk) have no cell driver;
    // STA only traverses LUT/FF fanin cones, so they are not needed here.
    let nl = UltraStaNetlist {
        design,
        sites,
        drivers,
        fanin,
        ff_q,
        ff_d,
    };
    let db = preliminary_xcvu3p();
    sta_routed(&nl, &db, 10_000).map_err(|e| format!("ULTRA STA: {e}"))
}

/// Build routable nets from the placed design. Only reached once a real
/// WireDb exists. Nets touching IOBs or clock nets fail loud: IOBs have
/// no placed site in this flow yet, and clock nets are phase 2 for the
/// router.
fn build_routable_nets(
    design: &Design,
    packed: &helion_ultrascale::UltraPacked,
    placed: &helion_ultrascale::UltraPlaced,
) -> Result<Vec<RoutableNet>, String> {
    use helion_ultrascale::UltraSite;
    // Placed site per cell: (x, y) plus BEL name.
    let mut cell_site: HashMap<String, (i32, i32, String)> = HashMap::new();
    for p in &placed.slices {
        let Some(slice) = packed.slices.get(p.packed_idx) else {
            continue;
        };
        for (bel, cell) in &slice.luts {
            let letter = (b'A' + *bel) as char;
            cell_site.insert(cell.clone(), (p.x, p.y, format!("{letter}6LUT")));
        }
        for (bel, cell) in &slice.ffs {
            let letter = (b'A' + (bel / 2)) as char;
            let suffix = if bel % 2 == 0 { "Q" } else { "Q2" };
            cell_site.insert(cell.clone(), (p.x, p.y, format!("{letter}{suffix}")));
        }
        if let Some(cell) = &slice.carry8 {
            cell_site.insert(cell.clone(), (p.x, p.y, "CARRY8".to_string()));
        }
    }
    let site_of = |cell: &str| -> Result<(UltraSite, String), String> {
        cell_site
            .get(cell)
            .map(|(x, y, bel)| (UltraSite { x: *x, y: *y, kind: 0 }, bel.clone()))
            .ok_or_else(|| format!("ULTRA ROUTE: cell {cell} has no placed site"))
    };
    // Pin -> net map.
    let mut pin_to_net: HashMap<(helion_ir::Spur, helion_ir::Spur), helion_ir::Spur> =
        HashMap::new();
    for net in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.cell, ep.pin), net.name);
        }
    }
    // Net name -> driver (cell, pin) and sinks [(cell, pin)].
    let mut nets: Vec<RoutableNet> = Vec::new();
    for net in &design.nets {
        let net_name = resolve(net.name).to_string();
        let mut driver: Option<(String, String)> = None;
        let mut sinks: Vec<(String, String)> = Vec::new();
        for ep in &net.endpoints {
            let cell = resolve(ep.cell).to_string();
            let pin = resolve(ep.pin).to_string();
            let kind = design
                .cells
                .iter()
                .find(|c| c.name == ep.cell)
                .map(|c| &c.kind);
            match kind {
                Some(CellKind::Lut6 { .. }) if pin == "O" => {
                    driver = Some((cell, "O6".to_string()));
                }
                Some(CellKind::Hff) if pin == "Q" => {
                    driver = Some((cell, "Q".to_string()));
                }
                Some(CellKind::Lut6 { .. }) => sinks.push((cell, pin)),
                Some(CellKind::Hff) => sinks.push((cell, pin)),
                Some(CellKind::IobOut) | Some(CellKind::IobIn) => {
                    return Err(format!(
                        "ULTRA ROUTE: net {net_name} touches IOB cell {cell}; IOB placement is not implemented"
                    ));
                }
                Some(CellKind::ClockBuf) => {
                    return Err(format!(
                        "ULTRA ROUTE: net {net_name} is a clock net (driver {cell}); clock routing is phase 2"
                    ));
                }
                _ => {}
            }
        }
        let Some((dcell, dpin)) = driver else {
            continue;
        };
        if sinks.is_empty() {
            continue;
        }
        let (dsite, dbel) = site_of(&dcell)?;
        let mut rsinks = Vec::new();
        for (scell, spin) in &sinks {
            let (ssite, sbel) = site_of(scell)?;
            rsinks.push((ssite, sbel, spin.clone()));
        }
        nets.push(RoutableNet {
            name: net_name,
            driver: (dsite, dbel, dpin),
            sinks: rsinks,
            is_clock: false,
        });
    }
    Ok(nets)
}
