//! [`EquivModel`]: the combinational cone model both sides of an
//! equivalence check are built from.
//!
//! Two constructors:
//! - [`EquivModel::from_merged_cones`] merges caller-supplied per-output cone
//!   AIGs with the tested [`merge_cones`][crate::aig_opt::merge_cones].
//! - [`EquivModel::from_design`] elaborates a [`Design`]: sequential
//!   elements are cut (`Hff` Q becomes pseudo-PI `ff.<cell>.Q`, D becomes
//!   pseudo-output `ff.<cell>.D`), `Lut6` cells go through the LUT bridge,
//!   top ports become PIs/outputs. Anything else (`Bram18`, `Mac27`,
//!   `BlackBox`, `Ila`, undriven nets, combinational loops) fails loud with
//!   [`EquivError::UnsupportedCell`], never a silent pass.
//!
//! Cone construction is iterative (explicit work stack): netlist LUT chains
//! can be thousands deep, and a recursive walk would risk the same
//! debug-stack overflow fixed elsewhere in this crate.

use crate::aig_opt::merge_cones;
use crate::equiv::lut_bridge::{instantiate, LutCache};
use crate::equiv::EquivError;
use crate::{Aig, Lit};
use helion_ir::{CellKind, Design, PortDir};
use std::collections::{HashMap, HashSet};

/// Combinational model of one side of an equivalence check.
///
/// `inputs` are PI names in deterministic order (top-level input ports in
/// declared order, then `ff.<cell>.Q` pseudo-PIs in cell order). `outputs`
/// pairs each observable output name with its AIG literal, in deterministic
/// order (`ff.<cell>.D` pseudo-outputs in cell order, then top ports in
/// declared order).
#[derive(Clone, Debug)]
pub struct EquivModel {
    pub inputs: Vec<String>,
    pub outputs: Vec<(String, Lit)>,
    pub aig: Aig,
}

impl EquivModel {
    /// Merge caller-supplied per-output cone AIGs (name, cone) into one
    /// model. PIs union by name via [`merge_cones`][crate::aig_opt::merge_cones].
    pub fn from_merged_cones(named_cones: &[(String, Aig)]) -> EquivModel {
        let cones: Vec<Aig> = named_cones.iter().map(|(_, c)| c.clone()).collect();
        let (aig, lits) = merge_cones(&cones);
        let outputs: Vec<(String, Lit)> = named_cones
            .iter()
            .zip(lits)
            .map(|((name, _), lit)| (name.clone(), lit))
            .collect();
        EquivModel {
            inputs: aig.pis.clone(),
            outputs,
            aig,
        }
    }

    /// Elaborate a mapped [`Design`] into a combinational model.
    ///
    /// Supported cells: `Lut6` (via the LUT bridge), `Hff` (cut), `IobOut`
    /// (output extraction). Unconnected LUT input pins read 0, matching the
    /// simulator's convention.
    pub fn from_design(d: &Design) -> Result<EquivModel, EquivError> {
        // 1. Fail loud on anything that is not plain combinational logic.
        for c in &d.cells {
            match &c.kind {
                CellKind::Lut6 { .. } | CellKind::Hff | CellKind::IobOut => {}
                other => {
                    return Err(EquivError::UnsupportedCell(format!(
                        "cell '{}' has unsupported kind {other:?}",
                        c.name
                    )));
                }
            }
        }

        // 2. Inputs: top-level input ports in declared order (Inout counts as
        //    input, matching synth), then FF Q pseudo-PIs in cell order.
        let mut inputs: Vec<String> = d
            .ports
            .iter()
            .filter(|p| matches!(p.dir, PortDir::In | PortDir::Inout))
            .map(|p| p.name.clone())
            .collect();

        // 3. FF cut: Q net -> pseudo-PI, D net -> pseudo-output.
        let mut ff_q_net: HashMap<String, String> = HashMap::new();
        let mut ff_d_outputs: Vec<(String, String)> = Vec::new();
        for c in &d.cells {
            if !matches!(c.kind, CellKind::Hff) {
                continue;
            }
            let q = d.net_on(&c.name, "Q").ok_or_else(|| {
                EquivError::UnsupportedCell(format!("Hff cell '{}' has no net on Q", c.name))
            })?;
            let dn = d.net_on(&c.name, "D").ok_or_else(|| {
                EquivError::UnsupportedCell(format!("Hff cell '{}' has no net on D", c.name))
            })?;
            ff_q_net.insert(q.to_string(), c.name.clone());
            inputs.push(format!("ff.{}.Q", c.name));
            ff_d_outputs.push((format!("ff.{}.D", c.name), dn.to_string()));
        }

        // 4. AIG with every PI interned up front (keeps the PIs-first node-id
        //    invariant that merge_cones/from_expr maintain).
        let mut aig = Aig::with_pis(Vec::new());
        for name in &inputs {
            aig.pi_lit(name);
        }
        let pi_set: HashSet<String> = inputs.iter().cloned().collect();

        // 5. Driver index: net -> (driving Lut6 cell, INIT).
        let mut driver: HashMap<String, (String, u64)> = HashMap::new();
        for c in &d.cells {
            if let CellKind::Lut6 { init } = c.kind {
                if let Some(net) = d.net_on(&c.name, "O") {
                    driver.entry(net.to_string()).or_insert((c.name.clone(), init));
                }
            }
        }

        // 6. Outputs: FF D pseudo-outputs first, then top-level output ports
        //    via their IobOut cells (PAD net = port name).
        let mut out_nets: Vec<(String, String)> = ff_d_outputs;
        out_nets.extend(top_output_nets(d)?);

        // 7. Build every cone into the shared AIG (memoized, iterative).
        let mut ctx = ConeCtx {
            d,
            aig,
            cache: LutCache::new(),
            memo: HashMap::new(),
            in_progress: HashSet::new(),
            pi_set,
            ff_q_net,
            driver,
        };
        let mut outputs = Vec::with_capacity(out_nets.len());
        for (name, net) in &out_nets {
            let lit = cone_lit(&mut ctx, net)?;
            outputs.push((name.clone(), lit));
        }
        Ok(EquivModel {
            inputs,
            outputs,
            aig: ctx.aig,
        })
    }
}

/// (output name, driving net) for every top-level output port, in port
/// declared order. A port driven through one IobOut keeps its port name; a
/// port fanned out through N IobOuts (multi-bit buses share the PAD net
/// across bits) gets per-bit names `<port>_<i>`, ordered by the IOB bit
/// index encoded in the cell name (`u_iob` = bit 0, `u_iobN` = bit N).
fn top_output_nets(d: &Design) -> Result<Vec<(String, String)>, EquivError> {
    let mut out = Vec::new();
    for p in &d.ports {
        if !matches!(p.dir, PortDir::Out) {
            continue;
        }
        let mut iobs: Vec<(&str, &str)> = Vec::new(); // (cell, I net)
        for c in &d.cells {
            if !matches!(c.kind, CellKind::IobOut) {
                continue;
            }
            let pad = d.net_on(&c.name, "PAD");
            if pad == Some(p.name.as_str()) {
                let i = d.net_on(&c.name, "I").ok_or_else(|| {
                    EquivError::UnsupportedCell(format!(
                        "IobOut cell '{}' has no net on I",
                        c.name
                    ))
                })?;
                iobs.push((c.name.as_str(), i));
            }
        }
        if iobs.is_empty() {
            return Err(EquivError::UnsupportedCell(format!(
                "output port '{}' has no IobOut driver",
                p.name
            )));
        }
        iobs.sort_by_key(|(cell, _)| iob_bit_index(cell));
        if iobs.len() == 1 {
            out.push((p.name.clone(), iobs[0].1.to_string()));
        } else {
            for (i, (_, net)) in iobs.iter().enumerate() {
                out.push((format!("{}_{}", p.name, i), net.to_string()));
            }
        }
    }
    Ok(out)
}

/// Bit index encoded in an IOB cell name: `u_iob` -> 0, `u_iobN` -> N.
/// Non-conforming names sort last, by name (deterministic fallback).
fn iob_bit_index(cell: &str) -> (u64, &str) {
    if let Some(rest) = cell.strip_prefix("u_iob") {
        if rest.is_empty() {
            return (0, cell);
        }
        if let Ok(n) = rest.parse::<u64>() {
            return (n, cell);
        }
    }
    (u64::MAX, cell)
}

struct ConeCtx<'d> {
    d: &'d Design,
    aig: Aig,
    cache: LutCache,
    memo: HashMap<String, Lit>,
    in_progress: HashSet<String>,
    pi_set: HashSet<String>,
    ff_q_net: HashMap<String, String>,
    driver: HashMap<String, (String, u64)>,
}

enum WorkItem {
    Visit(String),
    Build { net: String, in_nets: [Option<String>; 6], init: u64 },
}

/// AIG literal driving `net`, building the transitive LUT cone iteratively.
fn cone_lit(ctx: &mut ConeCtx, net0: &str) -> Result<Lit, EquivError> {
    let mut stack = vec![WorkItem::Visit(net0.to_string())];
    while let Some(item) = stack.pop() {
        match item {
            WorkItem::Visit(net) => {
                if ctx.memo.contains_key(&net) || !ctx.in_progress.insert(net.clone()) {
                    continue; // done, or already scheduled (diamond / loop back-edge)
                }
                if ctx.pi_set.contains(&net) {
                    let l = ctx.aig.pi_lit(&net);
                    ctx.memo.insert(net, l);
                    continue;
                }
                if let Some(cell) = ctx.ff_q_net.get(&net).cloned() {
                    let l = ctx.aig.pi_lit(&format!("ff.{cell}.Q"));
                    ctx.memo.insert(net, l);
                    continue;
                }
                let (cell, init) = ctx.driver.get(&net).cloned().ok_or_else(|| {
                    EquivError::UnsupportedCell(format!(
                        "net '{net}' has no driver (undriven or unsupported source)"
                    ))
                })?;
                let mut in_nets: [Option<String>; 6] = Default::default();
                for (i, slot) in in_nets.iter_mut().enumerate() {
                    *slot = ctx.d.net_on(&cell, &format!("I{i}")).map(|s| s.to_string());
                }
                stack.push(WorkItem::Build {
                    net: net.clone(),
                    in_nets: in_nets.clone(),
                    init,
                });
                // Children pop (and memoize) before their Build. A child
                // already in progress is a combinational loop back-edge: it
                // is skipped here and caught at Build time via the memo miss.
                for child in in_nets.iter().flatten() {
                    if !ctx.memo.contains_key(child) {
                        stack.push(WorkItem::Visit(child.clone()));
                    }
                }
            }
            WorkItem::Build { net, in_nets, init } => {
                let tmpl = ctx.cache.template(init);
                let mut ins = [Lit::c0(); 6];
                for (i, slot) in in_nets.iter().enumerate() {
                    match slot {
                        Some(n) => {
                            let Some(&l) = ctx.memo.get(n) else {
                                return Err(EquivError::UnsupportedCell(format!(
                                    "combinational loop detected at net '{n}'"
                                )));
                            };
                            ins[i] = l;
                        }
                        None => ins[i] = Lit::c0(), // unconnected pin reads 0 (sim convention)
                    }
                }
                let l = instantiate(&mut ctx.aig, &tmpl, ins);
                ctx.in_progress.remove(&net);
                ctx.memo.insert(net, l);
            }
        }
    }
    ctx.memo.get(net0).copied().ok_or_else(|| {
        EquivError::Solver(format!(
            "internal error: cone root '{net0}' was not built"
        ))
    })
}
