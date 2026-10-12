//! Synchronizer recognition for structural CDC analysis.
//!
//! Generalizes the pair-level 2-FF detect (`cdc_synchronizer`) to a full
//! chain walk (`sync_chain`), plus the mux-select `init`-word predicate,
//! the handshake shape detector, and the async-FIFO heuristic.
//!
//! Depth semantics: `sync_depth` counts passthrough-`Lut6` links in the
//! destination-domain synchronizer chain. On tech-mapped netlists every FF
//! `D` is driven through a LUT, so one stage contributes exactly one link:
//! a lone capture FF has depth 1, a 2-FF synchronizer depth 2. A `D` driven
//! directly by another domain's FF `Q` (no LUT) or by non-passthrough
//! combinational logic has depth 0.

use helion_ir::{CellKind, Design};
use std::collections::{HashMap, HashSet};

use super::domains::DomainMap;

/// How an FF's `D` pin is driven.
#[derive(Clone, Debug)]
enum DSource {
    /// `D` net driven directly by one FF `Q` (no LUT in between).
    DirectQ(String),
    /// `D` net driven by one passthrough LUT whose sole FF-`Q` input is `src`.
    ViaLut { lut: String, src: String },
    /// Anything else: combinational logic, ports, multi-driver nets.
    Other,
}

/// Classify what drives `ff`'s `D` pin. Refines [`ff_d_source_ff`]: the
/// direct-`Q` case and the passthrough-LUT case are distinguished because
/// only the LUT link counts toward `sync_depth`.
fn d_source(design: &Design, ff: &str) -> DSource {
    let Some(d_net) = design.net_on(ff, "D") else {
        return DSource::Other;
    };
    let Some(net) = design.net(d_net) else {
        return DSource::Other;
    };
    let mut q_drivers: Vec<String> = Vec::new();
    let mut lut_drivers: Vec<String> = Vec::new();
    for e in &net.endpoints {
        let Some(cell) = design.cell(&e.cell) else {
            continue;
        };
        match (&cell.kind, e.pin.as_str()) {
            (CellKind::Hff, "Q") => {
                if !q_drivers.contains(&e.cell) {
                    q_drivers.push(e.cell.clone());
                }
            }
            (CellKind::Lut6 { .. }, "O") => {
                if !lut_drivers.contains(&e.cell) {
                    lut_drivers.push(e.cell.clone());
                }
            }
            _ => {}
        }
    }
    if q_drivers.len() == 1 && lut_drivers.is_empty() {
        return DSource::DirectQ(q_drivers.pop().unwrap_or_default());
    }
    if lut_drivers.len() != 1 || !q_drivers.is_empty() {
        return DSource::Other;
    }
    // Exactly one LUT driver: passthrough iff its sole FF-Q input is one FF.
    let lut = lut_drivers.pop().unwrap_or_default();
    let mut q_pis: Vec<String> = Vec::new();
    for pin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
        let Some(pi) = design.net_on(&lut, pin) else {
            continue;
        };
        let Some(pin_net) = design.net(pi) else {
            continue;
        };
        for e in &pin_net.endpoints {
            if e.pin == "Q"
                && design
                    .cell(&e.cell)
                    .is_some_and(|c| matches!(c.kind, CellKind::Hff))
                && !q_pis.contains(&e.cell)
            {
                q_pis.push(e.cell.clone());
            }
        }
    }
    if q_pis.len() == 1 {
        DSource::ViaLut {
            lut,
            src: q_pis.pop().unwrap_or_default(),
        }
    } else {
        DSource::Other
    }
}

/// Driver FF of `ff`'s D pin: direct Q, or one tech-mapped passthrough LUT
/// whose sole data PI is an FF Q. Multi-input LUTs are not passthrough.
/// Moved verbatim from `helion-sta/src/lib.rs` (was `ff_d_source_ff`).
pub(crate) fn ff_d_source_ff(design: &Design, ff: &str) -> Option<String> {
    let d_net = design.net_on(ff, "D")?;
    let net = design.net(d_net)?;
    let q_drivers: Vec<String> = net
        .endpoints
        .iter()
        .filter(|e| e.pin == "Q")
        .filter(|e| {
            design
                .cell(&e.cell)
                .is_some_and(|c| matches!(c.kind, CellKind::Hff))
        })
        .map(|e| e.cell.clone())
        .collect();
    if q_drivers.len() == 1 {
        return Some(q_drivers[0].clone());
    }
    let lut_drivers: Vec<String> = net
        .endpoints
        .iter()
        .filter(|e| e.pin == "O")
        .filter(|e| {
            design
                .cell(&e.cell)
                .is_some_and(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        })
        .map(|e| e.cell.clone())
        .collect();
    if lut_drivers.len() != 1 {
        return None;
    }
    let lut = &lut_drivers[0];
    let mut q_pis = Vec::new();
    for pin in ["I0", "I1", "I2", "I3", "I4", "I5"] {
        let Some(pi) = design.net_on(lut, pin) else {
            continue;
        };
        let Some(pin_net) = design.net(pi) else {
            continue;
        };
        for e in &pin_net.endpoints {
            if e.pin == "Q"
                && design
                    .cell(&e.cell)
                    .is_some_and(|c| matches!(c.kind, CellKind::Hff))
                && !q_pis.iter().any(|x| x == &e.cell)
            {
                q_pis.push(e.cell.clone());
            }
        }
    }
    if q_pis.len() == 1 {
        Some(q_pis[0].clone())
    } else {
        None
    }
}

/// UG906-style sync detect: a 2FF chain on `to_net` whose first stage samples
/// a launch FF on `from_net`. Tech-map inserts a LUT on every D; walk one
/// passthrough LUT. A lone capture FF (cdc_cross) stays unsynchronized.
/// Moved verbatim from `helion-sta/src/lib.rs` (was `cdc_synchronizer`).
pub(crate) fn cdc_synchronizer(design: &Design, from_net: &str, to_net: &str) -> (usize, bool) {
    let mut endpoints = 0usize;
    let mut sync = false;
    let dest_ffs: Vec<String> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .filter(|c| design.net_on(&c.name, "CLK") == Some(to_net))
        .map(|c| c.name.clone())
        .collect();
    for ff in &dest_ffs {
        let Some(src) = ff_d_source_ff(design, ff) else {
            continue;
        };
        let src_clk = design.net_on(&src, "CLK");
        if src_clk == Some(from_net) {
            endpoints += 1;
            continue;
        }
        if src_clk != Some(to_net) {
            continue;
        }
        // Second stage of a 2FF sync: dest FF <- dest FF <- launch FF.
        if let Some(launch) = ff_d_source_ff(design, &src) {
            if design.net_on(&launch, "CLK") == Some(from_net) {
                endpoints += 1;
                sync = true;
            }
        }
    }
    (endpoints, sync)
}

/// Result of [`sync_chain`]: the destination-domain synchronizer chain
/// through the crossing load FF.
#[derive(Clone, Debug, Default)]
pub struct SyncChain {
    /// Destination-domain FFs on the chain, load FF first, then downstream.
    pub stages: Vec<String>,
    /// Reported `sync_depth`: the number of synchronizer stages. A lone FF
    /// whose `D` is driven directly by a foreign `Q` (no LUT) or by
    /// non-passthrough combinational logic is depth 0 (`UNSYNC_CROSSING`);
    /// one stage through a passthrough LUT is depth 1 (`SINGLE_FF_SYNC`).
    pub depth: u8,
    /// The cross-domain launch FF, when the chain terminates at one.
    pub launch: Option<String>,
    /// True when an intermediate stage `Q` fans out beyond the next stage's
    /// `D`-cone (tapped = unsafe, `TAPPED_SYNC`).
    pub tapped: bool,
}

impl SyncChain {
    /// Final (most downstream) stage of the chain.
    pub fn final_stage(&self) -> Option<&str> {
        self.stages.last().map(String::as_str)
    }
}

/// FFs in `domain` whose `D` is driven by `src`'s `Q` (directly or through
/// one passthrough LUT). Sorted by name for determinism.
fn q_load_ffs(design: &Design, domains: &DomainMap, src: &str, domain: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cell in &design.cells {
        if !matches!(cell.kind, CellKind::Hff) || cell.name == src {
            continue;
        }
        if domains.domain_of_ff(&cell.name) != Some(domain) {
            continue;
        }
        match d_source(design, &cell.name) {
            DSource::DirectQ(q) if q == src => out.push(cell.name.clone()),
            DSource::ViaLut { src: q, .. } if q == src => out.push(cell.name.clone()),
            _ => {}
        }
    }
    out.sort();
    out
}

/// `true` when every endpoint of `stage`'s `Q` net lies in `next`'s `D`-cone:
/// either `next`'s `D` pin directly, or an input pin of the passthrough LUT
/// feeding `next`'s `D`.
fn q_confined_to_next_d_cone(
    design: &Design,
    stage: &str,
    next: &str,
    via_lut: Option<&str>,
) -> bool {
    let Some(q_net) = design.net_on(stage, "Q") else {
        return true;
    };
    let Some(d_net) = design.net_on(next, "D") else {
        return false;
    };
    let Some(net) = design.net(q_net) else {
        return true;
    };
    net.endpoints.iter().all(|e| {
        if e.cell == next && e.pin == "D" {
            return true;
        }
        if let Some(lut) = via_lut {
            if e.cell == lut
                && e.pin.starts_with('I')
                && design.net_on(lut, "O") == Some(d_net)
            {
                return true;
            }
        }
        // The stage's own Q pin is on this net; that is not a load.
        e.cell == stage && e.pin == "Q"
    })
}

/// Walk the destination-domain synchronizer chain through `load_ff`.
///
/// Backward: through passthrough LUTs / FF `Q`s to the launch FF (or to
/// combinational logic / a port, in which case `launch` is `None`).
/// Forward: through `Q` into same-domain FFs. `depth` counts the
/// destination-domain stages (a lone FF driven directly by a foreign `Q`
/// or by combo logic is depth 0); `tapped` flags intermediate `Q` fanout
/// beyond the next stage's `D`-cone.
pub fn sync_chain(
    design: &Design,
    load_ff: &str,
    load_domain: usize,
    domains: &DomainMap,
) -> SyncChain {
    let mut chain = SyncChain::default();
    chain.stages.push(load_ff.to_string());
    let mut visited: HashSet<String> = HashSet::from([load_ff.to_string()]);
    // Backward walk to the launch FF. Remember the shape of the first link:
    // a lone FF driven directly by a foreign Q (or by combo logic) is depth 0.
    let mut first_link_direct_cross = false;
    let mut first_link_other = false;
    let mut cur = load_ff.to_string();
    for i in 0..64 {
        match d_source(design, &cur) {
            DSource::ViaLut { src, .. } => {
                if domains.domain_of_ff(&src) != Some(load_domain) {
                    chain.launch = Some(src);
                    break;
                }
                if !visited.insert(src.clone()) {
                    break;
                }
                cur = src;
            }
            DSource::DirectQ(src) => {
                if i == 0 && domains.domain_of_ff(&src) != Some(load_domain) {
                    first_link_direct_cross = true;
                }
                if domains.domain_of_ff(&src) != Some(load_domain) {
                    chain.launch = Some(src);
                    break;
                }
                if !visited.insert(src.clone()) {
                    break;
                }
                cur = src;
            }
            DSource::Other => {
                if i == 0 {
                    first_link_other = true;
                }
                break;
            }
        }
    }
    // Forward walk through same-domain stages.
    let mut cur = load_ff.to_string();
    for _ in 0..64 {
        let nexts = q_load_ffs(design, domains, &cur, load_domain);
        if nexts.is_empty() {
            break;
        }
        let next = &nexts[0];
        let via_lut: Option<String> = match d_source(design, next) {
            DSource::ViaLut { lut, .. } => Some(lut),
            _ => None,
        };
        // A fork, or any Q endpoint outside the next stage's D-cone, is a tap.
        if nexts.len() > 1
            || !q_confined_to_next_d_cone(design, &cur, next, via_lut.as_deref())
        {
            chain.tapped = true;
        }
        if !visited.insert(next.clone()) {
            break;
        }
        chain.stages.push(next.clone());
        cur = next.clone();
    }
    // Depth: every destination-domain FF on the chain is a stage, except a
    // lone FF driven directly by a foreign Q or by combo logic (depth 0).
    let n = chain.stages.len();
    chain.depth = if n >= 2 {
        n.min(u8::MAX as usize) as u8
    } else if first_link_direct_cross || first_link_other {
        0
    } else {
        1
    };
    chain
}

/// `true` when the `Lut6` `init` word implements a 2:1 mux with input `pin`
/// (0-5 for `I0`-`I5`) as the select: `out = (s & a) | (~s & b)` for some
/// ordered pair of distinct data pins `(a, b)`.
///
/// A metastable select fans the metastability out to the mux output no
/// downstream synchronizer can clean, hence `MUXED_CROSSING`.
pub fn is_mux_select(init: u64, pin: u32) -> bool {
    if pin >= 6 {
        return false;
    }
    let others: Vec<u32> = (0..6).filter(|&q| q != pin).collect();
    for &pa in &others {
        for &pb in &others {
            if pa == pb {
                continue;
            }
            let mut ok = true;
            for addr in 0..64u64 {
                let s = (addr >> pin) & 1;
                let da = (addr >> pa) & 1;
                let db = (addr >> pb) & 1;
                let expected = if s == 1 { da } else { db };
                if ((init >> addr) & 1) != expected {
                    ok = false;
                    break;
                }
            }
            if ok {
                return true;
            }
        }
    }
    false
}

/// Transitive fanin of `ff`'s `D` pin: backward through `Lut6`s and `Hff`
/// `Q`s (cycle-safe, bounded). Used by the handshake shape detector.
pub(crate) fn d_fanin_cells(design: &Design, ff: &str) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    if let Some(d) = design.net_on(ff, "D") {
        stack.push(d.to_string());
    }
    let mut nets_done: HashSet<String> = HashSet::new();
    while let Some(net_name) = stack.pop() {
        if !nets_done.insert(net_name.clone()) {
            continue;
        }
        if seen.len() > 4096 {
            break;
        }
        let Some(net) = design.net(&net_name) else {
            continue;
        };
        for e in &net.endpoints {
            let Some(cell) = design.cell(&e.cell) else {
                continue;
            };
            match (&cell.kind, e.pin.as_str()) {
                (CellKind::Hff, "Q") => {
                    if seen.insert(e.cell.clone()) {
                        if let Some(d) = design.net_on(&e.cell, "D") {
                            stack.push(d.to_string());
                        }
                    }
                }
                (CellKind::Lut6 { .. }, "O") => {
                    for p in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                        if let Some(pn) = design.net_on(&e.cell, p) {
                            stack.push(pn.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    seen
}

/// Forward combinational cone of `ff`'s `Q`: through `Lut6`s only (never
/// through FFs). Returns `(luts, ff_loads)` where `ff_loads` are the FFs
/// whose `D` the cone reaches. Used for reconvergence detection.
pub(crate) fn q_fwd_cone(design: &Design, ff: &str) -> (HashSet<String>, HashSet<String>) {
    let mut luts: HashSet<String> = HashSet::new();
    let mut ff_loads: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    if let Some(q) = design.net_on(ff, "Q") {
        stack.push(q.to_string());
    }
    let mut nets_done: HashSet<String> = HashSet::new();
    while let Some(net_name) = stack.pop() {
        if !nets_done.insert(net_name.clone()) {
            continue;
        }
        if luts.len() + ff_loads.len() > 4096 {
            break;
        }
        let Some(net) = design.net(&net_name) else {
            continue;
        };
        for e in &net.endpoints {
            let Some(cell) = design.cell(&e.cell) else {
                continue;
            };
            match (&cell.kind, e.pin.as_str()) {
                (CellKind::Lut6 { .. }, _) if e.pin.starts_with('I') => {
                    if luts.insert(e.cell.clone()) {
                        if let Some(o) = design.net_on(&e.cell, "O") {
                            stack.push(o.to_string());
                        }
                    }
                }
                (CellKind::Hff, "D") => {
                    ff_loads.insert(e.cell.clone());
                }
                _ => {}
            }
        }
    }
    (luts, ff_loads)
}

/// Forward cone of `ff`'s `Q` through `Lut6`s, reporting whether it reaches
/// any `Bram18` pin. Used by the async-FIFO heuristic.
pub(crate) fn q_cone_hits_bram(design: &Design, ff: &str) -> bool {
    let mut stack: Vec<String> = Vec::new();
    if let Some(q) = design.net_on(ff, "Q") {
        stack.push(q.to_string());
    }
    let mut nets_done: HashSet<String> = HashSet::new();
    let mut visited = 0usize;
    while let Some(net_name) = stack.pop() {
        if !nets_done.insert(net_name.clone()) {
            continue;
        }
        visited += 1;
        if visited > 4096 {
            break;
        }
        let Some(net) = design.net(&net_name) else {
            continue;
        };
        for e in &net.endpoints {
            let Some(cell) = design.cell(&e.cell) else {
                continue;
            };
            match &cell.kind {
                CellKind::Bram18 => return true,
                CellKind::Lut6 { .. } if e.pin.starts_with('I') => {
                    if let Some(o) = design.net_on(&e.cell, "O") {
                        stack.push(o.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Per-bit driver-domain lookup shared by the mux-select check.
pub(crate) fn net_driver_domains(
    design: &Design,
    domains: &DomainMap,
    net: &str,
) -> HashSet<usize> {
    let mut out: HashSet<usize> = HashSet::new();
    let mut stack = vec![net.to_string()];
    let mut done: HashSet<String> = HashSet::new();
    while let Some(n) = stack.pop() {
        if !done.insert(n.clone()) {
            continue;
        }
        if done.len() > 4096 {
            break;
        }
        let Some(netobj) = design.net(&n) else {
            continue;
        };
        for e in &netobj.endpoints {
            let Some(cell) = design.cell(&e.cell) else {
                continue;
            };
            match (&cell.kind, e.pin.as_str()) {
                (CellKind::Hff, "Q") => {
                    if let Some(d) = domains.domain_of_ff(&e.cell) {
                        out.insert(d);
                    }
                }
                (CellKind::Lut6 { .. }, "O") => {
                    for p in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                        if let Some(pn) = design.net_on(&e.cell, p) {
                            stack.push(pn.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Mux-select hazard: does the `D`-cone of `load_ff` contain a `Lut6` whose
/// `init` is a 2:1 mux with a `from_domain`-driven net on the select pin?
pub(crate) fn mux_select_hazard(
    design: &Design,
    domains: &DomainMap,
    load_ff: &str,
    from_domain: usize,
) -> Option<(String, u32)> {
    let mut stack: Vec<String> = Vec::new();
    if let Some(d) = design.net_on(load_ff, "D") {
        stack.push(d.to_string());
    }
    let mut done: HashSet<String> = HashSet::new();
    // Cache driver-domain sets per net; the cone walk revisits shared nets.
    let mut dom_cache: HashMap<String, HashSet<usize>> = HashMap::new();
    while let Some(n) = stack.pop() {
        if !done.insert(n.clone()) {
            continue;
        }
        if done.len() > 4096 {
            break;
        }
        let Some(netobj) = design.net(&n) else {
            continue;
        };
        for e in &netobj.endpoints {
            let Some(cell) = design.cell(&e.cell) else {
                continue;
            };
            let CellKind::Lut6 { init } = &cell.kind else {
                continue;
            };
            if e.pin != "O" {
                continue;
            }
            for pin in 0..6u32 {
                if !is_mux_select(*init, pin) {
                    continue;
                }
                let pin_name = format!("I{pin}");
                let Some(pnet) = design.net_on(&e.cell, &pin_name) else {
                    continue;
                };
                let doms = dom_cache
                    .entry(pnet.to_string())
                    .or_insert_with(|| net_driver_domains(design, domains, pnet));
                if doms.contains(&from_domain) {
                    return Some((e.cell.clone(), pin));
                }
            }
            for p in ["I0", "I1", "I2", "I3", "I4", "I5"] {
                if let Some(pn) = design.net_on(&e.cell, p) {
                    stack.push(pn.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::is_mux_select;

    /// Build the init word for `out = (I{sel} & I{pa}) | (~I{sel} & I{pb})`.
    fn mux_init(sel: u32, pa: u32, pb: u32) -> u64 {
        let mut init = 0u64;
        for addr in 0..64u64 {
            let s = (addr >> sel) & 1;
            let a = (addr >> pa) & 1;
            let b = (addr >> pb) & 1;
            if (s == 1 && a == 1) || (s == 0 && b == 1) {
                init |= 1 << addr;
            }
        }
        init
    }

    #[test]
    fn mux_select_detected() {
        let init = mux_init(2, 0, 5);
        assert!(is_mux_select(init, 2));
        // Data pins are not selects of a single-pin-data mux.
        assert!(!is_mux_select(init, 0));
        assert!(!is_mux_select(init, 5));
        assert!(!is_mux_select(init, 1));
    }

    #[test]
    fn non_mux_rejected() {
        // 3-input AND on I0..I2.
        let mut and3 = 0u64;
        for addr in 0..64u64 {
            if addr & 0b111 == 0b111 {
                and3 |= 1 << addr;
            }
        }
        for pin in 0..6u32 {
            assert!(!is_mux_select(and3, pin), "pin {pin}");
        }
        // Constant and buffer are not muxes.
        for pin in 0..6u32 {
            assert!(!is_mux_select(0, pin));
            assert!(!is_mux_select(0xAAAAAAAAAAAAAAAA, pin), "pin {pin}");
        }
    }

    #[test]
    fn mux_select_pin_out_of_range() {
        assert!(!is_mux_select(u64::MAX, 6));
    }
}
