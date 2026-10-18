//! Classification of raw crossings into the 11 `CdcClass` violations.
//!
//! Priority order (design doc Part C section 4): safe-chain -> skip (after
//! the handshake / FIFO / reconvergence review checks); single-FF ->
//! `SINGLE_FF_SYNC`; tapped -> `TAPPED_SYNC`; mux-select -> `MUXED_CROSSING`;
//! bus aggregate -> `MULTI_BIT_UNSYNC`; else `UNSYNC_CROSSING`. The info
//! classes `INPUT_CROSSING` / `BLACKBOX_BOUNDARY` and the warning
//! `UNCONSTRAINED_CLOCK_NET` are emitted alongside.

use std::collections::{HashMap, HashSet};

use helion_ir::{resolve, CellKind, Design, Spur};

use super::crossings::{bus_stem, driver_infos, CdcClass, CdcCrossing, CrossingSource, RawCrossing};
use super::domains::{DomainKind, DomainMap};
use super::synchronizers::{
    d_fanin_cells, mux_select_hazard, q_cone_hits_bram, q_fwd_cone, sync_chain, SyncChain,
};

#[allow(clippy::too_many_arguments)]
fn mk(
    net: &str,
    bus: Option<String>,
    from_domain: &str,
    to_domain: &str,
    class: CdcClass,
    driver_cells: Vec<String>,
    load_cells: Vec<String>,
    sync_depth: u8,
    detail: &str,
) -> CdcCrossing {
    CdcCrossing {
        net: net.to_string(),
        bus,
        from_domain: from_domain.to_string(),
        to_domain: to_domain.to_string(),
        class,
        severity: class.severity(),
        check: class.check_id().to_string(),
        driver_cells,
        load_cells,
        sync_depth,
        detail: detail.to_string(),
    }
}

/// A raw crossing whose chain is clean (depth >= 2, untapped), pending the
/// handshake / FIFO / reconvergence review checks.
struct SafeWork {
    raw: RawCrossing,
    from: usize,
    chain: SyncChain,
}

fn domain_name(domains: &DomainMap, idx: usize) -> &str {
    &domains.domains[idx].name
}

/// Classify `raw` crossings into `CdcCrossing` violation rows, honoring the
/// priority order from the design doc. Clean (>= 2-deep, untapped) chains
/// are skipped unless they match the handshake shape, the async-FIFO
/// heuristic, or combinational reconvergence — all flagged for review, never
/// declared safe.
pub fn classify_crossings(
    design: &Design,
    domains: &DomainMap,
    raw: Vec<RawCrossing>,
) -> Vec<CdcCrossing> {
    let mut out: Vec<CdcCrossing> = Vec::new();
    let mut safe: Vec<SafeWork> = Vec::new();
    let mut unsync_bits: Vec<CdcCrossing> = Vec::new();

    // Per-crossing classification.
    for r in raw {
        let to_name = domain_name(domains, r.to).to_string();
        match r.from {
            CrossingSource::PortInput => {
                out.push(mk(
                    &r.net,
                    bus_stem(&r.net),
                    "PortInput",
                    &to_name,
                    CdcClass::InputCrossing,
                    r.driver_cells.clone(),
                    vec![r.load_ff.clone()],
                    0,
                    &format!(
                        "top-level input port drives {}.D (net {}); the designer's set_input_delay contract applies",
                        r.load_ff, r.net
                    ),
                ));
            }
            CrossingSource::Domain(from) => {
                let from_name = domain_name(domains, from).to_string();
                let chain = sync_chain(design, &r.load_ff, r.to, domains);
                let depth = chain.depth;
                let launch_txt = chain.launch.clone().unwrap_or_else(|| "-".into());
                if depth >= 2 && !chain.tapped {
                    safe.push(SafeWork { raw: r, from, chain });
                    continue;
                }
                let (class, detail) = if depth == 1 {
                    (
                        CdcClass::SingleFfSync,
                        format!(
                            "single FF {} samples {} ({} -> {}); one flop is not a synchronizer",
                            r.load_ff, launch_txt, from_name, to_name
                        ),
                    )
                } else if chain.tapped {
                    (
                        CdcClass::TappedSync,
                        format!(
                            "synchronizer chain at {} ({} -> {}) is tapped: an intermediate stage Q fans out beyond the next stage's D-cone",
                            r.load_ff, from_name, to_name
                        ),
                    )
                } else if let Some((lut, pin)) =
                    mux_select_hazard(design, domains, &r.load_ff, from)
                {
                    (
                        CdcClass::MuxedCrossing,
                        format!(
                            "unsynchronized net {} drives {}/I{} as a 2:1 mux select ({} -> {}); a metastable select glitches the mux output and no downstream sync can clean it",
                            r.net, lut, pin, from_name, to_name
                        ),
                    )
                } else {
                    (
                        CdcClass::UnsyncCrossing,
                        format!(
                            "{} ({} -> {}) reaches {}.D with no synchronizer",
                            r.driver_cells.join(","),
                            from_name,
                            to_name,
                            r.load_ff
                        ),
                    )
                };
                let c = mk(
                    &r.net,
                    bus_stem(&r.net),
                    &from_name,
                    &to_name,
                    class,
                    r.driver_cells.clone(),
                    vec![r.load_ff.clone()],
                    depth,
                    &detail,
                );
                if class == CdcClass::UnsyncCrossing {
                    unsync_bits.push(c.clone());
                }
                out.push(c);
            }
        }
    }

    // Bus aggregate: >= 2 unsynchronized bits of one bus stem, same (from, to).
    let mut groups: HashMap<(String, String, String), Vec<&CdcCrossing>> = HashMap::new();
    for c in &unsync_bits {
        if let Some(stem) = c.bus.clone() {
            groups
                .entry((stem, c.from_domain.clone(), c.to_domain.clone()))
                .or_default()
                .push(c);
        }
    }
    for ((stem, from, to), bits) in &groups {
        if bits.len() < 2 {
            continue;
        }
        let mut loads: Vec<String> =
            bits.iter().flat_map(|b| b.load_cells.clone()).collect();
        loads.sort();
        loads.dedup();
        let mut drivers: Vec<String> =
            bits.iter().flat_map(|b| b.driver_cells.clone()).collect();
        drivers.sort();
        drivers.dedup();
        out.push(mk(
            stem,
            Some(stem.clone()),
            from,
            to,
            CdcClass::MultiBitUnsync,
            drivers,
            loads,
            0,
            &format!(
                "{}-bit bus {} crosses {} -> {} with no per-bit synchronizer; word skew and coherency loss need gray-coding or a handshake",
                bits.len(),
                stem,
                from,
                to
            ),
        ));
    }

    // Handshake shape: safe A->B chain x plus safe B->A chain y, where y's
    // launch FF has x's final stage in its D fanin (ack derived from the
    // synchronized req). The protocol is NOT proven: flagged for review.
    let mut launch_fanin: HashMap<String, HashSet<String>> = HashMap::new();
    for w in &safe {
        if let Some(launch) = w.chain.launch.as_deref() {
            launch_fanin
                .entry(launch.to_string())
                .or_insert_with(|| d_fanin_cells(design, launch));
        }
    }
    let mut handshake_idx: HashSet<usize> = HashSet::new();
    for (i, x) in safe.iter().enumerate() {
        let Some(final_x) = x.chain.final_stage() else {
            continue;
        };
        for (j, y) in safe.iter().enumerate() {
            if i == j || x.raw.to != y.from || y.raw.to != x.from {
                continue;
            }
            if let Some(launch_y) = y.chain.launch.as_deref() {
                if launch_fanin
                    .get(launch_y)
                    .is_some_and(|fanin| fanin.contains(final_x))
                {
                    handshake_idx.insert(i);
                    handshake_idx.insert(j);
                }
            }
        }
    }
    let mut handshakes: Vec<CdcCrossing> = Vec::new();
    for &i in &handshake_idx {
        let w = &safe[i];
        let from_name = domain_name(domains, w.from).to_string();
        let to_name = domain_name(domains, w.raw.to).to_string();
        handshakes.push(mk(
            &w.raw.net,
            bus_stem(&w.raw.net),
            &from_name,
            &to_name,
            CdcClass::HandshakeNeedsReview,
            w.raw.driver_cells.clone(),
            w.chain.stages.clone(),
            w.chain.depth,
            "req/ack-shaped 2FF pair across the domains; the 4-phase protocol is NOT proven structurally, review required",
        ));
    }
    out.extend(handshakes);
    let mut rest: Vec<SafeWork> = Vec::new();
    for (i, w) in safe.into_iter().enumerate() {
        if !handshake_idx.contains(&i) {
            rest.push(w);
        }
    }
    safe = rest;

    // Async-FIFO heuristic: a bus of >= 2 safe bits (each depth >= 2) whose
    // final-stage cones all reach Bram18 pins. Gray-code safety needs
    // sequential proof (SAT track): flagged Info, never safe.
    let mut fifo_groups: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
    for (i, w) in safe.iter().enumerate() {
        if let Some(stem) = bus_stem(&w.raw.net) {
            let key = (
                stem,
                domain_name(domains, w.from).to_string(),
                domain_name(domains, w.raw.to).to_string(),
            );
            fifo_groups.entry(key).or_default().push(i);
        }
    }
    let mut fifo_idx: HashSet<usize> = HashSet::new();
    for ((stem, from, to), idxs) in &fifo_groups {
        if idxs.len() < 2 {
            continue;
        }
        let all_bram = idxs.iter().all(|&i| {
            safe[i]
                .chain
                .final_stage()
                .is_some_and(|f| q_cone_hits_bram(design, f))
        });
        if !all_bram {
            continue;
        }
        let mut loads: Vec<String> = idxs
            .iter()
            .filter_map(|&i| safe[i].chain.final_stage().map(str::to_string))
            .collect();
        loads.sort();
        let mut drivers: Vec<String> = idxs
            .iter()
            .flat_map(|&i| safe[i].raw.driver_cells.clone())
            .collect();
        drivers.sort();
        drivers.dedup();
        let depth = idxs
            .iter()
            .map(|&i| safe[i].chain.depth)
            .min()
            .unwrap_or(0);
        out.push(mk(
            stem,
            Some(stem.clone()),
            from,
            to,
            CdcClass::AsyncFifoUnverified,
            drivers,
            loads,
            depth,
            "possible async-FIFO pointer bus: synchronized per bit but gray-coding and full/empty safety are NOT proven; needs sequential proof",
        ));
        fifo_idx.extend(idxs.iter().copied());
    }
    let mut rest: Vec<SafeWork> = Vec::new();
    for (i, w) in safe.into_iter().enumerate() {
        if !fifo_idx.contains(&i) {
            rest.push(w);
        }
    }
    safe = rest;

    // Sync reconvergence: >= 2 synchronized crossings, same (from, to),
    // whose final-stage forward cones share a LUT or a load FF. Skew between
    // the chains can glitch the reconvergent logic: designer confirmation.
    let cones: Vec<(HashSet<String>, HashSet<String>)> = safe
        .iter()
        .map(|w| {
            w.chain
                .final_stage()
                .map(|f| q_fwd_cone(design, f))
                .unwrap_or_default()
        })
        .collect();
    let mut reconv: HashMap<usize, Vec<String>> = HashMap::new();
    for i in 0..safe.len() {
        for j in (i + 1)..safe.len() {
            if safe[i].from != safe[j].from || safe[i].raw.to != safe[j].raw.to {
                continue;
            }
            let (li, fi) = &cones[i];
            let (lj, fj) = &cones[j];
            let shared_lut = li.intersection(lj).next().is_some();
            let shared_ff = fi.intersection(fj).next().is_some();
            if shared_lut || shared_ff {
                reconv
                    .entry(i)
                    .or_default()
                    .push(safe[j].raw.net.clone());
                reconv
                    .entry(j)
                    .or_default()
                    .push(safe[i].raw.net.clone());
            }
        }
    }
    for (i, partners) in &reconv {
        let w = &safe[*i];
        let from_name = domain_name(domains, w.from).to_string();
        let to_name = domain_name(domains, w.raw.to).to_string();
        let mut partners = partners.clone();
        partners.sort();
        partners.dedup();
        out.push(mk(
            &w.raw.net,
            bus_stem(&w.raw.net),
            &from_name,
            &to_name,
            CdcClass::SyncReconvergence,
            w.raw.driver_cells.clone(),
            w.chain.stages.clone(),
            w.chain.depth,
            &format!(
                "synchronized crossing reconverges combinationally with {} in {}; inter-chain skew needs designer confirmation",
                partners.join(","),
                to_name
            ),
        ));
    }

    // Unconstrained clock nets: honest-unknown beats guessed-safe.
    for (idx, dom) in domains.domains.iter().enumerate() {
        if dom.kind != DomainKind::Unknown {
            continue;
        }
        let mut loads: Vec<String> = domains
            .ff_domain
            .iter()
            .filter(|&(_, d)| *d == idx)
            .map(|(f, _)| f.clone())
            .collect();
        if loads.is_empty() {
            continue;
        }
        loads.sort();
        out.push(mk(
            &dom.net,
            None,
            "-",
            &dom.name,
            CdcClass::UnconstrainedClockNet,
            Vec::new(),
            loads.clone(),
            0,
            &format!(
                "{} FFs clocked by '{}' with no create_clock; crossings involving this domain cannot be classified",
                loads.len(),
                dom.net
            ),
        ));
    }

    // Black-box boundaries: one Info per cell listing the feeding nets and
    // their domains. Bram18/Mac27/BlackBox expose no clock pins, so Helion
    // cannot classify crossings into them: visible but unclassifiable, an
    // honest limitation. Aggregated per cell to avoid one record per bus bit.
    let mut bb_feeds: HashMap<Spur, (Vec<Spur>, Vec<String>)> = HashMap::new();
    for net in &design.nets {
        let mut bb_cells: Vec<Spur> = Vec::new();
        for e in &net.endpoints {
            if let Some(c) = design.cell(resolve(e.cell)) {
                match &c.kind {
                    CellKind::Bram18 | CellKind::Mac27 | CellKind::BlackBox { .. } => {
                        if !bb_cells.contains(&e.cell) {
                            bb_cells.push(e.cell);
                        }
                    }
                    _ => {}
                }
            }
        }
        if bb_cells.is_empty() {
            continue;
        }
        let mut from_names: Vec<String> = Vec::new();
        for di in driver_infos(design, domains, resolve(net.name)) {
            if let CrossingSource::Domain(d) = di.source {
                if domains.domains[d].kind != DomainKind::Unknown {
                    from_names.push(domain_name(domains, d).to_string());
                }
            }
        }
        from_names.sort();
        from_names.dedup();
        if from_names.is_empty() {
            continue;
        }
        for bb in &bb_cells {
            let entry = bb_feeds.entry(*bb).or_default();
            entry.0.push(net.name);
            entry.1.extend(from_names.clone());
        }
    }
    let mut bb_cells: Vec<Spur> = bb_feeds.keys().cloned().collect();
    bb_cells.sort();
    for bb in &bb_cells {
        let (mut nets, mut doms) = bb_feeds.remove(bb).unwrap_or_default();
        nets.sort();
        nets.dedup();
        doms.sort();
        doms.dedup();
        let nets_s: Vec<&str> = nets.iter().map(|n| resolve(*n)).collect();
        out.push(mk(
            resolve(*bb),
            None,
            &doms.join(","),
            resolve(*bb),
            CdcClass::BlackBoxBoundary,
            Vec::new(),
            vec![resolve(*bb).to_string()],
            0,
            &format!(
                "{} exposes no clock pins; nets {} driven from {} cannot be timing-classified",
                resolve(*bb),
                nets_s.join(","),
                doms.join(",")
            ),
        ));
    }

    out.sort_by(|a, b| {
        (
            &a.net,
            a.class.as_str(),
            &a.from_domain,
            &a.to_domain,
        )
            .cmp(&(&b.net, b.class.as_str(), &b.from_domain, &b.to_domain))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::super::crossings::CdcClass;
    use crate::{create_clock, report_cdc, CdcSeverity, Constraints};
    use helion_ir::{intern, CellKind, Design, PortDir};

    /// `out = I0` buffer LUT (passthrough).
    const BUF_INIT: u64 = 0xAAAAAAAAAAAAAAAA;

    fn two_clocks() -> Vec<crate::Clock> {
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk_a", 10_000, "clk_a");
        create_clock(&mut clks, "clk_b", 8_000, "clk_b");
        clks
    }

    fn ff(d: &mut Design, name: &str, clk: &str, d_net: &str) {
        d.add_cell(name, CellKind::Hff);
        d.connect(clk, name, "CLK");
        d.connect(d_net, name, "D");
        d.connect(&format!("{name}_q"), name, "Q");
    }

    fn buf_lut(d: &mut Design, name: &str, src_q_net: &str, dst_d_net: &str) {
        d.add_cell(name, CellKind::Lut6 { init: BUF_INIT });
        d.connect(src_q_net, name, "I0");
        d.connect(dst_d_net, name, "O");
    }

    fn crossings_of(d: &Design, clks: &[crate::Clock]) -> Vec<super::super::crossings::CdcCrossing> {
        let r = report_cdc(clks, &Constraints::default(), None, Some(d));
        r.crossings
    }

    fn of_class(
        xs: &[super::super::crossings::CdcCrossing],
        class: CdcClass,
    ) -> Vec<&super::super::crossings::CdcCrossing> {
        xs.iter().filter(|c| c.class == class).collect()
    }

    #[test]
    fn unsync_direct_q_depth_zero() {
        // Direct Q->D sample across domains, no LUT: UNSYNC_CROSSING depth 0.
        let mut d = Design::new("t");
        d.add_port("clk_a", PortDir::In);
        d.add_port("clk_b", PortDir::In);
        ff(&mut d, "launch", "clk_a", "launch_d");
        d.add_cell("cap", CellKind::Hff);
        d.connect("clk_b", "cap", "CLK");
        d.connect("launch_q", "cap", "D");
        d.connect("cap_q", "cap", "Q");
        let clks = two_clocks();
        let xs = crossings_of(&d, &clks);
        let unsync = of_class(&xs, CdcClass::UnsyncCrossing);
        assert_eq!(unsync.len(), 1, "{xs:?}");
        assert_eq!(unsync[0].severity, CdcSeverity::Critical);
        assert_eq!(unsync[0].check, "CDC-10");
        assert_eq!(unsync[0].sync_depth, 0);
        assert_eq!(unsync[0].from_domain, "clk_a");
        assert_eq!(unsync[0].to_domain, "clk_b");
    }

    #[test]
    fn single_ff_through_lut_depth_one() {
        // One passthrough LUT between domains: SINGLE_FF_SYNC depth 1.
        let mut d = Design::new("t");
        d.add_port("clk_a", PortDir::In);
        d.add_port("clk_b", PortDir::In);
        ff(&mut d, "launch", "clk_a", "launch_d");
        d.add_cell("cap", CellKind::Hff);
        d.connect("clk_b", "cap", "CLK");
        d.connect("cap_d", "cap", "D");
        d.connect("cap_q", "cap", "Q");
        buf_lut(&mut d, "buf0", "launch_q", "cap_d");
        let clks = two_clocks();
        let xs = crossings_of(&d, &clks);
        let single = of_class(&xs, CdcClass::SingleFfSync);
        assert_eq!(single.len(), 1, "{xs:?}");
        assert_eq!(single[0].severity, CdcSeverity::Critical);
        assert_eq!(single[0].check, "CDC-10");
        assert_eq!(single[0].sync_depth, 1);
        assert!(of_class(&xs, CdcClass::UnsyncCrossing).is_empty());
    }

    #[test]
    fn tapped_sync_warns() {
        // 2-deep chain, but stage-0 Q also drives unrelated clk_b logic.
        let mut d = Design::new("t");
        d.add_port("clk_a", PortDir::In);
        d.add_port("clk_b", PortDir::In);
        ff(&mut d, "launch", "clk_a", "launch_d");
        for (f, dn) in [("s0", "s0_d"), ("s1", "s1_d"), ("tap", "tap_d")] {
            d.add_cell(f, CellKind::Hff);
            d.connect("clk_b", f, "CLK");
            d.connect(dn, f, "D");
            d.connect(&format!("{f}_q"), f, "Q");
        }
        buf_lut(&mut d, "buf0", "launch_q", "s0_d");
        buf_lut(&mut d, "buf1", "s0_q", "s1_d");
        buf_lut(&mut d, "buf2", "s0_q", "tap_d");
        let clks = two_clocks();
        let xs = crossings_of(&d, &clks);
        let tapped = of_class(&xs, CdcClass::TappedSync);
        assert_eq!(tapped.len(), 1, "{xs:?}");
        assert_eq!(tapped[0].severity, CdcSeverity::Warning);
        assert_eq!(tapped[0].check, "HELION-CDC-1");
        assert_eq!(tapped[0].sync_depth, 2);
    }

    #[test]
    fn multibit_bus_aggregate() {
        // 4-bit bus, each bit directly sampled: 4 UNSYNC + 1 aggregate.
        let mut d = Design::new("t");
        d.add_port("clk_a", PortDir::In);
        d.add_port("clk_b", PortDir::In);
        for i in 0..4 {
            let src = format!("cnt{i}");
            d.add_cell(&src, CellKind::Hff);
            d.connect("clk_a", &src, "CLK");
            d.connect(&format!("cnt{i}_d"), &src, "D");
            d.connect(&format!("cnt[{i}]"), &src, "Q");
            let cap = format!("cap{i}");
            d.add_cell(&cap, CellKind::Hff);
            d.connect("clk_b", &cap, "CLK");
            d.connect(&format!("cnt[{i}]"), &cap, "D");
            d.connect(&format!("{cap}_q"), &cap, "Q");
        }
        let clks = two_clocks();
        let xs = crossings_of(&d, &clks);
        let unsync = of_class(&xs, CdcClass::UnsyncCrossing);
        assert_eq!(unsync.len(), 4, "{xs:?}");
        let agg = of_class(&xs, CdcClass::MultiBitUnsync);
        assert_eq!(agg.len(), 1, "{xs:?}");
        assert_eq!(agg[0].bus.as_deref(), Some("cnt"));
        assert_eq!(agg[0].severity, CdcSeverity::Warning);
        assert_eq!(agg[0].check, "HELION-CDC-3");
        assert_eq!(agg[0].load_cells.len(), 4);
        assert_eq!(agg[0].from_domain, "clk_a");
        assert_eq!(agg[0].to_domain, "clk_b");
    }

    #[test]
    fn blackbox_boundary_info() {
        let mut d = Design::new("t");
        d.add_port("clk_a", PortDir::In);
        ff(&mut d, "src", "clk_a", "src_d");
        d.add_cell("u_bb", CellKind::BlackBox { module: intern("ip") });
        d.add_cell("u_bb", CellKind::BlackBox { module: helion_ir::intern("ip") });
        d.add_cell("u_bb", CellKind::BlackBox { module: helion_ir::intern("ip") });
        d.add_cell("u_bb", CellKind::BlackBox { module: helion_ir::intern("ip") });
        d.add_cell("u_bb", CellKind::BlackBox { module: helion_ir::intern("ip") });
        d.connect("sig", "src", "Q");
        d.connect("sig", "u_bb", "INP");
        let clks = two_clocks();
        let xs = crossings_of(&d, &clks);
        let bb = of_class(&xs, CdcClass::BlackBoxBoundary);
        assert_eq!(bb.len(), 1, "{xs:?}");
        assert_eq!(bb[0].severity, CdcSeverity::Info);
        assert_eq!(bb[0].check, "HELION-CDC-8");
        assert_eq!(bb[0].from_domain, "clk_a");
    }

    #[test]
    fn async_fifo_heuristic_info() {
        // 2-bit gray pointer, 2FF per bit, synced bits feed Bram18.
        let mut d = Design::new("t");
        d.add_port("clk_a", PortDir::In);
        d.add_port("clk_b", PortDir::In);
        d.add_cell("u_bram", CellKind::Bram18);
        for i in 0..2 {
            ff(&mut d, &format!("wptr{i}"), "clk_a", &format!("wptr{i}_d"));
            for (f, dn) in [(format!("ws{i}"), format!("wptr[{i}]")), (format!("wr{i}"), format!("wr{i}_d"))] {
                d.add_cell(&f, CellKind::Hff);
                d.connect("clk_b", &f, "CLK");
                d.connect(&dn, &f, "D");
                d.connect(&format!("{f}_q"), &f, "Q");
            }
            buf_lut(&mut d, &format!("wb{i}a"), &format!("wptr{i}_q"), &format!("wptr[{i}]"));
            buf_lut(&mut d, &format!("wb{i}b"), &format!("ws{i}_q"), &format!("wr{i}_d"));
            d.connect(&format!("wr{i}_q"), "u_bram", &format!("ADDR{i}"));
        }
        let clks = two_clocks();
        let xs = crossings_of(&d, &clks);
        let fifo = of_class(&xs, CdcClass::AsyncFifoUnverified);
        assert_eq!(fifo.len(), 1, "{xs:?}");
        assert_eq!(fifo[0].severity, CdcSeverity::Info);
        assert_eq!(fifo[0].check, "HELION-CDC-6");
        assert_eq!(fifo[0].bus.as_deref(), Some("wptr"));
        // The per-bit safe chains are consumed by the aggregate, not skipped
        // silently; the Bram18 itself still gets one honest boundary record.
        let bb = of_class(&xs, CdcClass::BlackBoxBoundary);
        assert_eq!(bb.len(), 1, "{xs:?}");
        assert_eq!(xs.len(), 2, "{xs:?}");
    }
}
