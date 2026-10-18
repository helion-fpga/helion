//! Pack logical cells into Helion site primitives (LUTFF + IOB + MAC27 + BRAM18 + ILA).

use helion_device::{Device, Site};
use helion_ir::{intern, resolve, CellKind, Design, Spur};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Packed {
    pub lutffs: Vec<PackedLutFf>,
    pub iobs: Vec<PackedIob>,
    pub macs: Vec<PackedMac>,
    pub brams: Vec<PackedBram>,
}

#[derive(Clone, Debug)]
pub struct PackedBram {
    pub cell: String,
    /// INIT words from IR attr `INIT` (comma-separated hex).
    pub init: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct PackedLutFf {
    pub lut_cell: String,
    pub ff_cell: String,
    pub init: u64,
    /// LUT pin → FF cell whose Q drives it (local cluster).
    pub lut_pins: Vec<(u8, String)>,
    /// Q net of the packed FF (IOB matching).
    pub q_net: String,
}

#[derive(Clone, Debug)]
pub struct PackedIob {
    pub cell: String,
    pub from_net: String,
    /// Optional `IOB_XxYy` from IR port LOC.
    pub loc: Option<String>,
    /// UG893 I/O Ports `DRIVE` (mA), copied from the PAD port.
    pub drive: Option<String>,
    /// UG893 I/O Ports `SLEW`.
    pub slew: Option<String>,
    /// UG893 I/O Ports `PULLTYPE`.
    pub pulltype: Option<String>,
    /// UG893 I/O Ports `DIFF_TERM`.
    pub diff_term: Option<String>,
    /// UG893 I/O Ports `IN_TERM`.
    pub in_term: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PackedMac {
    pub cell: String,
}

/// LUTFF occupancy per HAD CLB site ID (`CLB_X2Y1` → N). Derived from
/// placement sites, not a decorative pie.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackingSummary {
    pub sites: BTreeMap<String, usize>,
}

impl PackingSummary {
    /// Group placed LUTFF clusters by [`Site::id`].
    pub fn from_lutff_sites(sites: &[(Site, u8)]) -> Self {
        let mut m = BTreeMap::new();
        for (site, _) in sites {
            *m.entry(site.id()).or_insert(0) += 1;
        }
        Self { sites: m }
    }

    pub fn lutff_in(&self, site_id: &str) -> usize {
        self.sites.get(site_id).copied().unwrap_or(0)
    }
}

pub fn pack(design: &Design, _dev: &Device) -> Result<Packed, String> {
    // Multi-backend cell kinds never reach the HL10T packer: reject loudly
    // instead of silently dropping them (which would miscompile).
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobIn) {
            return Err(format!(
                "cell {} is IobIn (input IOB): unsupported for HL10T pack (multi-backend flow only)",
                resolve(c.name)
            ));
        }
        if matches!(c.kind, CellKind::ClockBuf) {
            return Err(format!(
                "cell {} is ClockBuf (global clock buffer): unsupported for HL10T pack (multi-backend flow only)",
                resolve(c.name)
            ));
        }
    }
    // Ibex-scale: linear scans via Design::net_on are O(n^3). Index once.
    let pins = design.pin_index();
    let mut d_driver: std::collections::HashMap<Spur, Spur> = std::collections::HashMap::new();
    let mut q_driver: std::collections::HashMap<Spur, Spur> = std::collections::HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) {
            if let Some(d) = pins.net_on(resolve(c.name), "D") {
                d_driver.entry(d).or_insert(c.name);
            }
            if let Some(q) = pins.net_on(resolve(c.name), "Q") {
                q_driver.insert(q, c.name);
            }
        }
    }
    let mut lutffs = Vec::new();
    let mut used_ff = std::collections::HashSet::<Spur>::new();
    for c in &design.cells {
        let CellKind::Lut6 { init } = c.kind else {
            continue;
        };
        let Some(o_net) = pins.net_on(resolve(c.name), "O") else {
            continue;
        };
        // Comb LUTs (no FF on O) pack as LUT-only: empty ff_cell, q_net = O so
        // IOB/route match the LUT output. Registered LUTs keep the FF cluster.
        let ff_name = d_driver
            .get(&o_net)
            .copied()
            .filter(|n| !used_ff.contains(n));
        if let Some(n) = ff_name {
            used_ff.insert(n);
        }
        let mut lut_pins = Vec::new();
        const IPINS: [&str; 6] = ["I0", "I1", "I2", "I3", "I4", "I5"];
        for (pin, key) in IPINS.iter().enumerate() {
            if let Some(net) = pins.net_on(resolve(c.name), key) {
                if let Some(src) = q_driver.get(&net) {
                    lut_pins.push((pin as u8, resolve(*src).to_string()));
                }
            }
        }
        let (ff_cell, q_net) = if let Some(n) = ff_name {
            (
                resolve(n).to_string(),
                pins.net_on(resolve(n), "Q").map(resolve).unwrap_or("").to_string(),
            )
        } else {
            (String::new(), resolve(o_net).to_string())
        };
        lutffs.push(PackedLutFf {
            lut_cell: resolve(c.name).to_string(),
            ff_cell,
            init,
            lut_pins,
            q_net,
        });
    }
    let mut iobs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            let net = pins
                .net_on(resolve(c.name), "I")
                .ok_or_else(|| format!("IOB {} has no I net", resolve(c.name)))?;
            let pad = pins.net_on(resolve(c.name), "PAD").unwrap_or(intern(""));
            let port = design.ports.iter().find(|p| p.name == pad);
            let loc = port.and_then(|p| p.attrs.get("LOC").map(|s| s.to_string()));
            iobs.push(PackedIob {
                cell: resolve(c.name).to_string(),
                from_net: resolve(net).to_string(),
                loc,
                drive: port.and_then(|p| p.attrs.get("DRIVE").map(|s| s.to_string())),
                slew: port.and_then(|p| p.attrs.get("SLEW").map(|s| s.to_string())),
                pulltype: port.and_then(|p| p.attrs.get("PULLTYPE").map(|s| s.to_string())),
                diff_term: port.and_then(|p| p.attrs.get("DIFF_TERM").map(|s| s.to_string())),
                in_term: port.and_then(|p| p.attrs.get("IN_TERM").map(|s| s.to_string())),
            });
        }
    }
    let mut macs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }) {
            macs.push(PackedMac {
                cell: resolve(c.name).to_string(),
            });
        }
    }
    let mut brams = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. }) {
            let init = c
                .attrs
                .get("INIT")
                .unwrap_or("")
                .split(',')
                .filter(|t| !t.is_empty())
                .map(|t| u64::from_str_radix(t.trim(), 16).unwrap_or(0))
                .collect();
            brams.push(PackedBram {
                cell: resolve(c.name).to_string(),
                init,
            });
        }
    }
    Ok(Packed {
        lutffs,
        iobs,
        macs,
        brams,
    })
}

/// Copy HNF port DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM onto packed IOBs
/// (post-pack set_property).
pub fn apply_iob_electrical(design: &Design, iobs: &mut [PackedIob]) {
    let pins = design.pin_index();
    let ports: std::collections::HashMap<&str, &helion_ir::Port> =
        design.ports.iter().map(|p| (resolve(p.name), p)).collect();
    for iob in iobs.iter_mut() {
        let pad = pins.net_on(&iob.cell, "PAD").unwrap_or(intern(""));
        if let Some(p) = ports.get(resolve(pad)) {
            iob.drive = p.attrs.get("DRIVE").map(|s| s.to_string());
            iob.slew = p.attrs.get("SLEW").map(|s| s.to_string());
            iob.pulltype = p.attrs.get("PULLTYPE").map(|s| s.to_string());
            iob.diff_term = p.attrs.get("DIFF_TERM").map(|s| s.to_string());
            iob.in_term = p.attrs.get("IN_TERM").map(|s| s.to_string());
        }
    }
}

// ============================================================================
// Vivado UG904 Post-Pack Physical Optimizations:
// 1. High-Fanout Flip-Flop Replication (`replicate_high_fanout_ffs`)
// 2. `SRL16E` -> `SRLC32E` Shift-Register Cascade Merging (`merge_srl16_to_srlc32`)
// 3. `LUT6` Critical-Arrival Input Pin Permutation (`balance_lut_pin_delays`)
// ============================================================================

/// Default 7-series `LUT6` input pin propagation delays in picoseconds (`I0..I5`).
/// Higher-index pins are physically closer to the output mux tree:
/// `d(I5) < d(I4) < d(I3) < d(I2) < d(I1) < d(I0)`.
pub const DEFAULT_LUT6_PIN_DELAYS_PS: [u32; 6] = [125, 110, 95, 80, 62, 45];

/// Replication details for one high-fanout source flip-flop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicatedFfGroup {
    pub original_ff: String,
    pub replica_ffs: Vec<String>,
    pub original_fanout: usize,
    pub max_cluster_fanout: usize,
}

/// Summary report from [`replicate_high_fanout_ffs`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FfReplicationReport {
    pub groups: Vec<ReplicatedFfGroup>,
    pub total_replicas_added: usize,
}

/// UG904 High-Fanout Flip-Flop Replication (`phys_opt_design -fanout_opt`):
/// When a source `CellKind::Hff` drives `> max_fanout` loads on its `Q` net,
/// partition its sink endpoints into balanced clusters of size `<= max_fanout`,
/// clone the `Hff` (preserving `D`, `CLK`, `CE`, `SR`/`RST`, `INIT`, and cell attrs),
/// and rewire each sink cluster to its dedicated replica.
pub fn replicate_high_fanout_ffs(design: &mut Design, max_fanout: usize) -> FfReplicationReport {
    let k_max = max_fanout.max(1);
    let pins = design.pin_index();
    let q_pin = intern("Q");

    let ila_nets: std::collections::HashSet<Spur> = design
        .cells
        .iter()
        .filter_map(|c| match c.kind {
            CellKind::Ila { net } => Some(net),
            _ => None,
        })
        .collect();
    let out_port_nets: std::collections::HashSet<Spur> = design
        .ports
        .iter()
        .filter(|p| matches!(p.dir, helion_ir::PortDir::Out | helion_ir::PortDir::Inout))
        .map(|p| p.name)
        .collect();

    struct RepPlan {
        ff_name: String,
        ff_spur: Spur,
        ff_attrs: helion_ir::Attrs,
        input_conns: Vec<(String, String)>,
        q_net_spur: Spur,
        q_net_name: String,
        sinks: Vec<helion_ir::Endpoint>,
    }

    let mut plans: Vec<RepPlan> = Vec::new();
    for cell in &design.cells {
        if !matches!(cell.kind, CellKind::Hff) || cell.attrs.flag("DONT_TOUCH") {
            continue;
        }
        let ff_name = resolve(cell.name).to_string();
        let Some(q_net_spur) = pins.net_on(&ff_name, "Q") else {
            continue;
        };
        if ila_nets.contains(&q_net_spur) || out_port_nets.contains(&q_net_spur) {
            continue;
        }
        let Some(net_obj) = design.nets.iter().find(|n| n.name == q_net_spur) else {
            continue;
        };
        if net_obj.attrs.flag("mark_debug") {
            continue;
        }
        let sinks: Vec<helion_ir::Endpoint> = net_obj
            .endpoints
            .iter()
            .filter(|e| !(e.cell == cell.name && e.pin == q_pin))
            .cloned()
            .collect();
        if sinks.len() <= k_max {
            continue;
        }

        let mut input_conns = Vec::new();
        for pin_name in ["D", "CLK", "C", "CE", "SR", "RST", "R", "S"] {
            if let Some(in_net) = pins.net_on(&ff_name, pin_name) {
                input_conns.push((pin_name.to_string(), resolve(in_net).to_string()));
            }
        }
        plans.push(RepPlan {
            ff_name,
            ff_spur: cell.name,
            ff_attrs: cell.attrs.clone(),
            input_conns,
            q_net_spur,
            q_net_name: resolve(q_net_spur).to_string(),
            sinks,
        });
    }

    let mut report = FfReplicationReport::default();
    if plans.is_empty() {
        return report;
    }

    for plan in plans {
        let total_sinks = plan.sinks.len();
        let n_clusters = (total_sinks + k_max - 1) / k_max;
        // Balanced cluster sizes: each cluster gets `total_sinks / n_clusters` or `+ 1` (all <= k_max).
        let base_sz = total_sinks / n_clusters;
        let rem = total_sinks % n_clusters;

        let mut clusters: Vec<Vec<helion_ir::Endpoint>> = Vec::with_capacity(n_clusters);
        let mut offset = 0usize;
        let mut max_cluster_fanout = 0usize;
        for c_idx in 0..n_clusters {
            let sz = base_sz + usize::from(c_idx < rem);
            max_cluster_fanout = max_cluster_fanout.max(sz);
            clusters.push(plan.sinks[offset..offset + sz].to_vec());
            offset += sz;
        }

        // Keep only the driver + cluster 0 on the original Q net.
        if let Some(orig_net) = design.nets.iter_mut().find(|n| n.name == plan.q_net_spur) {
            let mut new_eps = vec![helion_ir::Endpoint {
                cell: plan.ff_spur,
                pin: q_pin,
            }];
            new_eps.extend(clusters[0].iter().cloned());
            orig_net.endpoints = new_eps;
        }

        let mut replica_ffs = Vec::with_capacity(n_clusters - 1);
        for (r_idx, sink_cluster) in clusters.into_iter().enumerate().skip(1) {
            let rep_cell_name = format!("{}_rep{r_idx}", plan.ff_name);
            let rep_q_name = format!("{}_rep{r_idx}", plan.q_net_name);
            let mut rep_attrs = plan.ff_attrs.clone();
            rep_attrs.set("REPLICA_OF", plan.ff_name.clone());

            design.push_cell(helion_ir::Cell {
                name: intern(&rep_cell_name),
                kind: CellKind::Hff,
                attrs: rep_attrs,
            });
            for (pin_name, net_name) in &plan.input_conns {
                design.connect(net_name, &rep_cell_name, pin_name);
            }

            let rep_cell_spur = intern(&rep_cell_name);
            let rep_q_spur = intern(&rep_q_name);
            let mut rep_eps = Vec::with_capacity(1 + sink_cluster.len());
            rep_eps.push(helion_ir::Endpoint {
                cell: rep_cell_spur,
                pin: q_pin,
            });
            rep_eps.extend(sink_cluster);
            design.nets.push(helion_ir::Net {
                name: rep_q_spur,
                endpoints: rep_eps,
                attrs: helion_ir::Attrs::default(),
            });
            replica_ffs.push(rep_cell_name);
        }

        report.total_replicas_added += replica_ffs.len();
        report.groups.push(ReplicatedFfGroup {
            original_ff: plan.ff_name,
            replica_ffs,
            original_fanout: total_sinks,
            max_cluster_fanout,
        });
    }

    design.rebuild_indexes();
    report
}

/// Summary report from [`merge_srl16_to_srlc32`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SrlMergeReport {
    pub merged_srlc32_cells: Vec<String>,
    pub srl16_pairs_merged: usize,
    pub ff_chains_merged: usize,
    pub stages_collapsed: usize,
}

fn is_srl16_cell(cell: &helion_ir::Cell) -> bool {
    cell.attrs.get("SRL_TYPE") == Some("SRL16E") || cell.attrs.get("BEL_TYPE") == Some("SRL16E")
}

fn srl_cell_depth(cell: &helion_ir::Cell) -> usize {
    cell.attrs
        .get("DEPTH")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(16)
}

/// UG904 `SRL16E` -> `SRLC32E` Shift-Register Cascade Merger:
/// 1. Detects cascaded pairs of `SRL16E` cells (`srl0.Q/O -> srl1.D/I0`) sharing `CLK` and `CE`
///    with no intermediate fanout taps and merges them into a single `SRLC32E` primitive with `Q31` cascade output.
/// 2. Detects linear `Hff` shift-register chains (up to 32 stages per `SRLC32E`) without intermediate
///    fanout taps or resets and collapses them into a single `SRLC32E` primitive with `Q31` output.
pub fn merge_srl16_to_srlc32(design: &mut Design) -> SrlMergeReport {
    let mut report = SrlMergeReport::default();
    let pins = design.pin_index();

    let out_port_nets: std::collections::HashSet<Spur> = design
        .ports
        .iter()
        .filter(|p| matches!(p.dir, helion_ir::PortDir::Out | helion_ir::PortDir::Inout))
        .map(|p| p.name)
        .collect();

    // Pass 1: Merge chained `SRL16E` pairs (`srl0` -> `srl1`).
    let srl16_names: Vec<String> = design
        .cells
        .iter()
        .filter(|c| is_srl16_cell(c))
        .map(|c| resolve(c.name).to_string())
        .collect();

    let mut srl_by_d_net: std::collections::HashMap<Spur, String> =
        std::collections::HashMap::new();
    for sname in &srl16_names {
        if let Some(dnet) = pins.net_on(sname, "D").or_else(|| pins.net_on(sname, "I0")) {
            srl_by_d_net.insert(dnet, sname.clone());
        }
    }

    let mut consumed_srl: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cells_to_remove: std::collections::HashSet<Spur> = std::collections::HashSet::new();
    let mut nets_to_remove: std::collections::HashSet<Spur> = std::collections::HashSet::new();

    for srl0 in &srl16_names {
        if consumed_srl.contains(srl0) {
            continue;
        }
        let Some(mid_net) = pins.net_on(srl0, "Q").or_else(|| pins.net_on(srl0, "O")) else {
            continue;
        };
        if out_port_nets.contains(&mid_net) {
            continue;
        }
        let Some(srl1) = srl_by_d_net.get(&mid_net).cloned() else {
            continue;
        };
        if &srl1 == srl0 || consumed_srl.contains(&srl1) {
            continue;
        }
        // Check that `mid_net` only connects `srl0` and `srl1`.
        let srl0_spur = intern(srl0);
        let srl1_spur = intern(&srl1);
        let mid_only_two = design
            .nets
            .iter()
            .find(|n| n.name == mid_net)
            .is_some_and(|n| {
                !n.attrs.flag("mark_debug")
                    && n.endpoints
                        .iter()
                        .all(|e| e.cell == srl0_spur || e.cell == srl1_spur)
            });
        if !mid_only_two {
            continue;
        }
        // Verify matching CLK and CE.
        let clk0 = pins.net_on(srl0, "CLK");
        let clk1 = pins.net_on(&srl1, "CLK");
        let ce0 = pins.net_on(srl0, "CE");
        let ce1 = pins.net_on(&srl1, "CE");
        if clk0 != clk1 || ce0 != ce1 {
            continue;
        }
        let d0 = design.cell(srl0).map(srl_cell_depth).unwrap_or(16);
        let d1 = design.cell(&srl1).map(srl_cell_depth).unwrap_or(16);
        if d0 + d1 > 32 {
            continue;
        }
        let Some(out_net) = pins.net_on(&srl1, "Q").or_else(|| pins.net_on(&srl1, "O")) else {
            continue;
        };

        consumed_srl.insert(srl0.clone());
        consumed_srl.insert(srl1.clone());
        cells_to_remove.insert(srl1_spur);
        nets_to_remove.insert(mid_net);

        // Rewire `out_net` endpoints from `srl1` onto `srl0` (`O`, `Q`, and `Q31`).
        if let Some(onet_obj) = design.nets.iter_mut().find(|n| n.name == out_net) {
            onet_obj.endpoints.retain(|e| e.cell != srl1_spur);
            for p in ["O", "Q", "Q31"] {
                onet_obj.endpoints.push(helion_ir::Endpoint {
                    cell: srl0_spur,
                    pin: intern(p),
                });
            }
        }
        if let Some(c0_mut) = design.cells.iter_mut().find(|c| c.name == srl0_spur) {
            c0_mut.attrs.set("SRL_TYPE", "SRLC32E");
            c0_mut.attrs.set("BEL_TYPE", "SRLC32E");
            c0_mut.attrs.set("DEPTH", (d0 + d1).to_string());
            c0_mut.attrs.set("MERGED_SRL16E_PAIR", format!("{srl0},{srl1}"));
        }
        report.srl16_pairs_merged += 1;
        report.stages_collapsed += d0 + d1;
        report.merged_srlc32_cells.push(srl0.clone());
    }

    if !cells_to_remove.is_empty() || !nets_to_remove.is_empty() {
        design.cells.retain(|c| !cells_to_remove.contains(&c.name));
        design.nets.retain(|n| !nets_to_remove.contains(&n.name));
        for n in &mut design.nets {
            n.endpoints.retain(|e| !cells_to_remove.contains(&e.cell));
        }
        design.rebuild_indexes();
    }

    // Pass 2: Collapse resetless `Hff` shift chains without intermediate taps into `SRLC32E`.
    let pins = design.pin_index();
    let mut ff_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ff_by_q: std::collections::HashMap<Spur, String> = std::collections::HashMap::new();
    let mut ff_by_d: std::collections::HashMap<Spur, String> = std::collections::HashMap::new();

    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) && !c.attrs.flag("DONT_TOUCH") {
            let cname = resolve(c.name).to_string();
            // Only resetless FFs can merge into an SRLC32E.
            if pins.net_on(&cname, "SR").is_some()
                || pins.net_on(&cname, "RST").is_some()
                || pins.net_on(&cname, "R").is_some()
                || pins.net_on(&cname, "S").is_some()
            {
                continue;
            }
            if let Some(q) = pins.net_on(&cname, "Q") {
                ff_by_q.insert(q, cname.clone());
            }
            if let Some(d) = pins.net_on(&cname, "D") {
                ff_by_d.insert(d, cname.clone());
            }
            ff_set.insert(cname);
        }
    }

    // `next_ff[u] = v` iff `u.Q` drives ONLY `v.D` (1 driver, 1 load, not an output port)
    // and `u` and `v` share identical `(CLK, CE)`.
    let mut next_ff: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut has_prev_ff: std::collections::HashSet<String> = std::collections::HashSet::new();
    for u in &ff_set {
        let Some(q_net) = pins.net_on(u, "Q") else {
            continue;
        };
        if out_port_nets.contains(&q_net) {
            continue;
        }
        let Some(v) = ff_by_d.get(&q_net) else {
            continue;
        };
        if v == u {
            continue;
        }
        let u_spur = intern(u);
        let v_spur = intern(v);
        let single_fanout = design
            .nets
            .iter()
            .find(|n| n.name == q_net)
            .is_some_and(|n| {
                !n.attrs.flag("mark_debug")
                    && n.endpoints.len() == 2
                    && n.endpoints
                        .iter()
                        .all(|e| e.cell == u_spur || (e.cell == v_spur && e.pin == intern("D")))
            });
        if !single_fanout {
            continue;
        }
        if pins.net_on(u, "CLK") == pins.net_on(v, "CLK")
            && pins.net_on(u, "CE") == pins.net_on(v, "CE")
        {
            next_ff.insert(u.clone(), v.clone());
            has_prev_ff.insert(v.clone());
        }
    }

    let mut ff_cells_to_remove: std::collections::HashSet<Spur> = std::collections::HashSet::new();
    let mut ff_nets_to_remove: std::collections::HashSet<Spur> = std::collections::HashSet::new();
    let mut visited_ff: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut roots: Vec<String> = ff_set
        .iter()
        .filter(|f| !has_prev_ff.contains(*f) && next_ff.contains_key(*f))
        .cloned()
        .collect();
    roots.sort();

    for root in roots {
        let mut chain = vec![root.clone()];
        visited_ff.insert(root.clone());
        let mut cur = root;
        while let Some(nxt) = next_ff.get(&cur) {
            if !visited_ff.insert(nxt.clone()) {
                break;
            }
            chain.push(nxt.clone());
            cur = nxt.clone();
        }
        if chain.len() < 2 {
            continue;
        }

        for (chunk_idx, chunk) in chain.chunks(32).enumerate() {
            if chunk.len() < 2 {
                continue;
            }
            let first_ff = &chunk[0];
            let last_ff = &chunk[chunk.len() - 1];
            let Some(din_net) = pins.net_on(first_ff, "D") else {
                continue;
            };
            let Some(qout_net) = pins.net_on(last_ff, "Q") else {
                continue;
            };
            let Some(clk_net) = pins.net_on(first_ff, "CLK") else {
                continue;
            };
            let ce_net = pins.net_on(first_ff, "CE");

            let srl_name = format!("u_srlc32e_{first_ff}_c{chunk_idx}");
            let srl_spur = intern(&srl_name);
            let mut attrs = helion_ir::Attrs::default();
            attrs.set("SRL_TYPE", "SRLC32E");
            attrs.set("BEL_TYPE", "SRLC32E");
            attrs.set("SHREG_EXTRACT", "YES");
            attrs.set("DEPTH", chunk.len().to_string());
            attrs.set("TAP_INDEX", (chunk.len() - 1).to_string());
            attrs.set("DONT_TOUCH", "true");
            design.push_cell(helion_ir::Cell {
                name: srl_spur,
                kind: CellKind::Lut6 {
                    init: 0xAAAA_AAAA_AAAA_AAAA,
                },
                attrs,
            });

            design.connect(resolve(clk_net), &srl_name, "CLK");
            if let Some(ce) = ce_net {
                design.connect(resolve(ce), &srl_name, "CE");
            }
            design.connect(resolve(din_net), &srl_name, "D");
            design.connect(resolve(din_net), &srl_name, "I0");
            design.connect(resolve(qout_net), &srl_name, "Q");
            design.connect(resolve(qout_net), &srl_name, "O");
            design.connect(resolve(qout_net), &srl_name, "Q31");

            for (idx, ff_name) in chunk.iter().enumerate() {
                ff_cells_to_remove.insert(intern(ff_name));
                if idx + 1 < chunk.len() {
                    if let Some(mid_q) = pins.net_on(ff_name, "Q") {
                        ff_nets_to_remove.insert(mid_q);
                    }
                }
            }

            report.ff_chains_merged += 1;
            report.stages_collapsed += chunk.len();
            report.merged_srlc32_cells.push(srl_name);
        }
    }

    if !ff_cells_to_remove.is_empty() || !ff_nets_to_remove.is_empty() {
        design.cells.retain(|c| !ff_cells_to_remove.contains(&c.name));
        design.nets.retain(|n| !ff_nets_to_remove.contains(&n.name));
        for n in &mut design.nets {
            n.endpoints
                .retain(|e| !ff_cells_to_remove.contains(&e.cell));
        }
        design.rebuild_indexes();
    }

    report
}

/// Permute a 64-bit `Lut6` truth table `old_init` when old input pin `old_pin` (`0..6`)
/// is moved to new input pin `old_to_new_pin[old_pin]` (`0..6`).
pub fn permute_lut6_init(old_init: u64, old_to_new_pin: &[usize; 6]) -> u64 {
    let mut new_init = 0u64;
    for new_addr in 0..64usize {
        let mut old_addr = 0usize;
        for (old_pin, &new_pin) in old_to_new_pin.iter().enumerate() {
            if ((new_addr >> new_pin) & 1) == 1 {
                old_addr |= 1usize << old_pin;
            }
        }
        if ((old_init >> old_addr) & 1) == 1 {
            new_init |= 1u64 << new_addr;
        }
    }
    new_init
}

/// Per-LUT delay balancing record from [`balance_lut_pin_delays`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalancedLutRecord {
    pub lut_cell: String,
    pub old_init: u64,
    pub new_init: u64,
    pub old_to_new_pin: [usize; 6],
    pub arrival_before_ps: u32,
    pub arrival_after_ps: u32,
}

/// Summary report from [`balance_lut_pin_delays`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LutPinBalanceReport {
    pub balanced_luts: Vec<BalancedLutRecord>,
    pub total_ps_saved: u64,
}

/// UG904 `LUT6` Critical-Arrival Input Pin Delay Balancer:
/// Given per-net arrival times `net_arrivals_ps` and physical LUT pin delays `pin_delays_ps`
/// (`d(I5) < d(I4) < ... < d(I0)`), permutes each `Lut6`'s connected inputs so the
/// latest-arriving nets occupy the fastest (lowest-delay, highest-index) pins and
/// permutes the 64-bit `INIT` truth table to preserve exact Boolean functionality.
pub fn balance_lut_pin_delays(
    design: &mut Design,
    net_arrivals_ps: &std::collections::HashMap<String, u32>,
    pin_delays_ps: &[u32; 6],
) -> LutPinBalanceReport {
    let pins = design.pin_index();
    const IPINS: [&str; 6] = ["I0", "I1", "I2", "I3", "I4", "I5"];

    struct LutUpdate {
        cell_spur: Spur,
        cell_name: String,
        old_init: u64,
        new_init: u64,
        old_to_new_pin: [usize; 6],
        net_pin_moves: Vec<(Spur, Spur, Spur)>, // (net_spur, old_pin_spur, new_pin_spur)
        arrival_before_ps: u32,
        arrival_after_ps: u32,
    }

    let mut updates: Vec<LutUpdate> = Vec::new();

    for cell in &design.cells {
        let CellKind::Lut6 { init: old_init } = cell.kind else {
            continue;
        };
        if cell.attrs.get("SRL_TYPE").is_some() || cell.attrs.get("BEL_TYPE").is_some() {
            continue;
        }
        let cname = resolve(cell.name).to_string();
        let mut conn: Vec<(usize, Spur, u32)> = Vec::new();
        for (p_idx, p_name) in IPINS.iter().enumerate() {
            if let Some(net_spur) = pins.net_on(&cname, p_name) {
                let arr = net_arrivals_ps
                    .get(resolve(net_spur))
                    .copied()
                    .unwrap_or(0);
                conn.push((p_idx, net_spur, arr));
            }
        }
        if conn.len() <= 1 {
            continue;
        }

        let arrival_before_ps = conn
            .iter()
            .map(|&(p_idx, _, arr)| arr.saturating_add(pin_delays_ps[p_idx]))
            .max()
            .unwrap_or(0);

        // Target pin slots are the occupied pins sorted from slowest (largest delay)
        // to fastest (smallest delay; tie-break by higher pin index).
        let mut target_slots: Vec<usize> = conn.iter().map(|&(p_idx, _, _)| p_idx).collect();
        target_slots.sort_by(|&a, &b| pin_delays_ps[b].cmp(&pin_delays_ps[a]).then_with(|| a.cmp(&b)));

        // Sort connected inputs from earliest arrival to latest arrival.
        let mut sorted_inputs = conn.clone();
        sorted_inputs.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

        let mut old_to_new_pin: [usize; 6] = [0, 1, 2, 3, 4, 5];
        let mut net_pin_moves = Vec::new();
        let mut any_changed = false;

        for ((old_pin, net_spur, _), &new_pin) in sorted_inputs.iter().zip(target_slots.iter()) {
            old_to_new_pin[*old_pin] = new_pin;
            if *old_pin != new_pin {
                any_changed = true;
            }
            net_pin_moves.push((
                *net_spur,
                intern(IPINS[*old_pin]),
                intern(IPINS[new_pin]),
            ));
        }
        if !any_changed {
            continue;
        }

        let arrival_after_ps = sorted_inputs
            .iter()
            .zip(target_slots.iter())
            .map(|(&(_, _, arr), &new_pin)| arr.saturating_add(pin_delays_ps[new_pin]))
            .max()
            .unwrap_or(0);

        let new_init = permute_lut6_init(old_init, &old_to_new_pin);
        updates.push(LutUpdate {
            cell_spur: cell.name,
            cell_name: cname,
            old_init,
            new_init,
            old_to_new_pin,
            net_pin_moves,
            arrival_before_ps,
            arrival_after_ps,
        });
    }

    let mut report = LutPinBalanceReport::default();
    if updates.is_empty() {
        return report;
    }

    for upd in updates {
        if let Some(cell_mut) = design.cells.iter_mut().find(|c| c.name == upd.cell_spur) {
            cell_mut.kind = CellKind::Lut6 { init: upd.new_init };
        }
        for (net_spur, old_pin_spur, new_pin_spur) in upd.net_pin_moves {
            if old_pin_spur == new_pin_spur {
                continue;
            }
            if let Some(net_mut) = design.nets.iter_mut().find(|n| n.name == net_spur) {
                if let Some(ep) = net_mut
                    .endpoints
                    .iter_mut()
                    .find(|e| e.cell == upd.cell_spur && e.pin == old_pin_spur)
                {
                    ep.pin = new_pin_spur;
                }
            }
        }
        let saved = u64::from(upd.arrival_before_ps.saturating_sub(upd.arrival_after_ps));
        report.total_ps_saved += saved;
        report.balanced_luts.push(BalancedLutRecord {
            lut_cell: upd.cell_name,
            old_init: upd.old_init,
            new_init: upd.new_init,
            old_to_new_pin: upd.old_to_new_pin,
            arrival_before_ps: upd.arrival_before_ps,
            arrival_after_ps: upd.arrival_after_ps,
        });
    }

    design.rebuild_indexes();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design};

    #[test]
    fn packs_blinky_lutff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert_eq!(p.iobs.len(), 1);
        assert_eq!(p.lutffs[0].init, 0x5555_5555_5555_5555);
        assert_eq!(p.lutffs[0].lut_pins, vec![(0, "u_ff".into())]);
        assert_eq!(p.lutffs[0].q_net, "q");
    }

    #[test]
    fn packs_counter_four_lutffs_with_pins() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 4);
        assert_eq!(p.lutffs[3].lut_pins.len(), 4);
        assert_eq!(p.iobs[0].from_net, "q3");
        assert_eq!(p.lutffs[3].q_net, "q3");
        assert!(p.iobs[0].drive.is_none());
        assert!(p.iobs[0].slew.is_none());
        assert!(p.iobs[0].pulltype.is_none());
        assert!(p.iobs[0].diff_term.is_none());
        assert!(p.iobs[0].in_term.is_none());
        let mut d = Design::structural_counter();
        d.set_drive("led", "4").unwrap();
        d.set_slew("led", "FAST").unwrap();
        d.set_pulltype("led", "PULLUP").unwrap();
        d.set_diff_term("led", "TRUE").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.iobs[0].drive.as_deref(), Some("4"));
        assert_eq!(p.iobs[0].slew.as_deref(), Some("FAST"));
        assert_eq!(p.iobs[0].pulltype.as_deref(), Some("PULLUP"));
        assert_eq!(p.iobs[0].diff_term.as_deref(), Some("TRUE"));
        assert_eq!(p.iobs[0].in_term.as_deref(), Some("UNTUNED_SPLIT_50"));
    }

    #[test]
    fn packs_comb_lut_without_ff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("comb");
        d.add_port("a", helion_ir::PortDir::In);
        d.add_port("y", helion_ir::PortDir::Out);
        d.add_cell(
            "u_lut",
            CellKind::Lut6 {
                init: 0x5555_5555_5555_5555,
            },
        );
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("a", "u_lut", "I0");
        d.connect("n", "u_lut", "O");
        d.connect("n", "u_iob", "I");
        d.connect("y", "u_iob", "PAD");
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert!(
            p.lutffs[0].ff_cell.is_empty(),
            "comb LUT must not invent an FF"
        );
        assert_eq!(p.lutffs[0].q_net, "n");
        assert_eq!(p.iobs[0].from_net, "n");
    }

    #[test]
    fn packs_mac27() {
        let dev = Device::load_part("HL10T-DSP1").unwrap();
        let mut d = Design::new("mac");
        d.add_cell("u_mac", CellKind::Mac27);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.macs.len(), 1);
        assert!(p.lutffs.is_empty());
    }

    #[test]
    fn pack_thousands_of_lutff_pairs_is_subquadratic() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("wide");
        d.add_port("clk", helion_ir::PortDir::In);
        const N: u32 = 3_000;
        for i in 0..N {
            let lut = format!("lut{i}");
            let ff = format!("ff{i}");
            d.add_cell(
                &lut,
                CellKind::Lut6 {
                    init: 0x5555_5555_5555_5555,
                },
            );
            d.add_cell(&ff, CellKind::Hff);
            d.connect("clk", &ff, "CLK");
            d.connect(&format!("d{i}"), &lut, "O");
            d.connect(&format!("d{i}"), &ff, "D");
            d.connect(&format!("q{i}"), &ff, "Q");
            d.connect(&format!("q{i}"), &lut, "I0");
        }
        let t0 = std::time::Instant::now();
        let p = pack(&d, &dev).unwrap();
        let ms = t0.elapsed().as_millis();
        assert_eq!(p.lutffs.len(), N as usize, "every LUT/FF pair packs");
        assert!(
            p.lutffs
                .iter()
                .all(|l| !l.ff_cell.is_empty() && !l.q_net.is_empty()),
            "clusters keep FF and Q net"
        );
        assert!(
            ms < 2000,
            "pack of {N} LUT/FF pairs must stay subquadratic, took {ms}ms"
        );
    }

    #[test]
    fn packs_bram18() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("ram");
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.brams.len(), 1);
    }

    #[test]
    fn packing_summary_groups_lutff_sites_by_had_id() {
        use helion_device::SiteKind;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let s0 = Site {
            x: dev.clb_x0,
            y: dev.clb_y0,
            kind: SiteKind::Clb,
        };
        let s1 = Site {
            x: dev.clb_x0,
            y: dev.clb_y0 + 1,
            kind: SiteKind::Clb,
        };
        assert!(dev.contains_site(s0) && dev.contains_site(s1));
        let sites = vec![(s0, 0), (s0, 1), (s0, 2), (s1, 0)];
        let sum = PackingSummary::from_lutff_sites(&sites);
        assert_eq!(sum.lutff_in(&s0.id()), 3);
        assert_eq!(sum.lutff_in(&s1.id()), 1);
        assert_eq!(dev.site_by_id(&s0.id()), Some(s0));
        assert_eq!(dev.site_by_id(&s1.id()), Some(s1));
        assert_eq!(sum.sites.len(), 2);
    }

    #[test]
    fn pack_rejects_multibackend_cells_for_hl10t() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        for kind in [CellKind::IobIn, CellKind::ClockBuf] {
            let mut d = Design::new("mb");
            d.add_cell("u_mb", kind);
            let err = pack(&d, &dev).unwrap_err();
            assert!(
                err.contains("unsupported for HL10T"),
                "expected HL10T rejection, got: {err}"
            );
        }
        // The gold counter (no multi-backend cells) still packs fine.
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 4);
    }

    #[test]
    fn high_fanout_ff_replication_partitions_loads_into_balanced_clusters() {
        let mut d = Design::new("hf_rep");
        d.add_port("clk", helion_ir::PortDir::In);
        d.add_port("ce", helion_ir::PortDir::In);
        d.add_port("rst", helion_ir::PortDir::In);
        d.add_port("din", helion_ir::PortDir::In);

        d.add_cell("src_ff", CellKind::Hff);
        let _ = d.set_cell_attr("src_ff", "INIT", "1");
        d.connect("clk", "src_ff", "CLK");
        d.connect("ce", "src_ff", "CE");
        d.connect("rst", "src_ff", "SR");
        d.connect("din", "src_ff", "D");
        d.connect("hot_q", "src_ff", "Q");

        for i in 0..10 {
            let sink = format!("sink_lut_{i}");
            d.add_cell(&sink, CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
            d.connect("hot_q", &sink, "I0");
            d.connect(&format!("out_{i}"), &sink, "O");
        }

        let rep = replicate_high_fanout_ffs(&mut d, 4);
        assert_eq!(rep.groups.len(), 1);
        assert_eq!(rep.groups[0].original_ff, "src_ff");
        assert_eq!(rep.groups[0].original_fanout, 10);
        assert_eq!(rep.groups[0].replica_ffs.len(), 2, "10 loads / max 4 -> 3 total FFs (1 orig + 2 replicas)");
        assert!(rep.groups[0].max_cluster_fanout <= 4);
        assert_eq!(rep.total_replicas_added, 2);

        let pins = d.pin_index();
        for rname in &rep.groups[0].replica_ffs {
            let rcell = d.cell(rname).expect("replica FF cell exists");
            assert_eq!(rcell.attrs.get("INIT"), Some("1"), "replica preserves INIT");
            assert_eq!(rcell.attrs.get("REPLICA_OF"), Some("src_ff"));
            assert_eq!(pins.net_on(rname, "CLK"), Some(intern("clk")));
            assert_eq!(pins.net_on(rname, "CE"), Some(intern("ce")));
            assert_eq!(pins.net_on(rname, "SR"), Some(intern("rst")));
            assert_eq!(pins.net_on(rname, "D"), Some(intern("din")));
        }

        // Verify every sink LUT still receives one of `{hot_q, hot_q_rep1, hot_q_rep2}`
        // and no Q net has more than 4 loads.
        for q_name in ["hot_q", "hot_q_rep1", "hot_q_rep2"] {
            let net = d.net(q_name).expect("Q net exists");
            // 1 driver + <=4 sinks -> <=5 endpoints
            assert!(
                net.endpoints.len() <= 5,
                "net {q_name} has {} endpoints (expected <=5)",
                net.endpoints.len()
            );
        }
    }

    #[test]
    fn srl16_to_srlc32_merges_cascaded_srl16_pairs_and_ff_shift_chains() {
        // Part A: Merge a pair of cascaded SRL16E cells into one SRLC32E.
        let mut d1 = Design::new("srl_pair");
        d1.add_cell("srl_lo", CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
        let _ = d1.set_cell_attr("srl_lo", "SRL_TYPE", "SRL16E");
        let _ = d1.set_cell_attr("srl_lo", "DEPTH", "16");
        d1.connect("clk", "srl_lo", "CLK");
        d1.connect("ce", "srl_lo", "CE");
        d1.connect("din", "srl_lo", "D");
        d1.connect("mid_q", "srl_lo", "Q");

        d1.add_cell("srl_hi", CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
        let _ = d1.set_cell_attr("srl_hi", "SRL_TYPE", "SRL16E");
        let _ = d1.set_cell_attr("srl_hi", "DEPTH", "16");
        d1.connect("clk", "srl_hi", "CLK");
        d1.connect("ce", "srl_hi", "CE");
        d1.connect("mid_q", "srl_hi", "D");
        d1.connect("dout", "srl_hi", "Q");

        let rep1 = merge_srl16_to_srlc32(&mut d1);
        assert_eq!(rep1.srl16_pairs_merged, 1);
        assert_eq!(rep1.stages_collapsed, 32);
        assert!(d1.cell("srl_hi").is_none(), "second SRL16E must be merged into first");
        let merged = d1.cell("srl_lo").expect("merged SRLC32E cell");
        assert_eq!(merged.attrs.get("SRL_TYPE"), Some("SRLC32E"));
        assert_eq!(merged.attrs.get("DEPTH"), Some("32"));
        let pins1 = d1.pin_index();
        assert_eq!(pins1.net_on("srl_lo", "D"), Some(intern("din")));
        assert_eq!(pins1.net_on("srl_lo", "Q31"), Some(intern("dout")));

        // Part B: Collapse an 18-stage tapless resetless Hff shift chain into an SRLC32E.
        let mut d2 = Design::new("ff_shift18");
        for i in 0..18 {
            let ff = format!("sr_ff_{i}");
            d2.add_cell(&ff, CellKind::Hff);
            d2.connect("clk", &ff, "CLK");
            d2.connect("ce", &ff, "CE");
            let dnet = if i == 0 {
                "shift_in".to_string()
            } else {
                format!("q_{}", i - 1)
            };
            let qnet = if i == 17 {
                "shift_out".to_string()
            } else {
                format!("q_{i}")
            };
            d2.connect(&dnet, &ff, "D");
            d2.connect(&qnet, &ff, "Q");
        }
        let rep2 = merge_srl16_to_srlc32(&mut d2);
        assert_eq!(rep2.ff_chains_merged, 1);
        assert_eq!(rep2.stages_collapsed, 18);
        assert_eq!(
            d2.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count(),
            0,
            "all 18 tapless shift FFs must collapse into SRLC32E"
        );
        let srlc32 = d2
            .cells
            .iter()
            .find(|c| c.attrs.get("SRL_TYPE") == Some("SRLC32E"))
            .expect("collapsed SRLC32E cell");
        assert_eq!(srlc32.attrs.get("DEPTH"), Some("18"));
        let pins2 = d2.pin_index();
        let sname = resolve(srlc32.name);
        assert_eq!(pins2.net_on(sname, "D"), Some(intern("shift_in")));
        assert_eq!(pins2.net_on(sname, "Q31"), Some(intern("shift_out")));
    }

    #[test]
    fn lut_pin_delay_balancer_moves_critical_arrival_to_fast_pins_and_permutes_init() {
        let mut d = Design::new("lut_balance");
        // 3-input function: `f(I0, I1, I2) = (I0 & ~I1) ^ I2`
        // Originally: I0 = crit_net (500ps), I1 = mid_net (250ps), I2 = early_net (50ps).
        // Since d(I0)=125ps > d(I1)=110ps > d(I2)=95ps, placing crit_net on I2, mid_net on I1,
        // and early_net on I0 reduces max arrival from (500 + 125 = 625ps) to (500 + 95 = 595ps).
        let mut orig_init = 0u64;
        for addr in 0..64usize {
            let i0 = (addr & 1) != 0;
            let i1 = ((addr >> 1) & 1) != 0;
            let i2 = ((addr >> 2) & 1) != 0;
            if (i0 && !i1) ^ i2 {
                orig_init |= 1u64 << addr;
            }
        }
        d.add_cell("u_lut", CellKind::Lut6 { init: orig_init });
        d.connect("crit_net", "u_lut", "I0");
        d.connect("mid_net", "u_lut", "I1");
        d.connect("early_net", "u_lut", "I2");
        d.connect("out_net", "u_lut", "O");

        let mut arrivals = std::collections::HashMap::new();
        arrivals.insert("crit_net".to_string(), 500);
        arrivals.insert("mid_net".to_string(), 250);
        arrivals.insert("early_net".to_string(), 50);

        let rep = balance_lut_pin_delays(&mut d, &arrivals, &DEFAULT_LUT6_PIN_DELAYS_PS);
        assert_eq!(rep.balanced_luts.len(), 1);
        let rec = &rep.balanced_luts[0];
        assert_eq!(rec.arrival_before_ps, 500 + 125);
        assert_eq!(rec.arrival_after_ps, 500 + 95);
        assert_eq!(rep.total_ps_saved, 30);

        let pins = d.pin_index();
        // Earliest net (`early_net`) is now on slowest pin `I0`; latest net (`crit_net`) is on fastest pin `I2`.
        assert_eq!(pins.net_on("u_lut", "I0"), Some(intern("early_net")));
        assert_eq!(pins.net_on("u_lut", "I1"), Some(intern("mid_net")));
        assert_eq!(pins.net_on("u_lut", "I2"), Some(intern("crit_net")));

        // Verify exact Boolean equivalence across all 8 input combinations of (crit, mid, early).
        let new_init = match d.cell("u_lut").unwrap().kind {
            CellKind::Lut6 { init } => init,
            _ => unreachable!(),
        };
        for mask in 0..8usize {
            let crit_v = (mask & 1) != 0;
            let mid_v = ((mask >> 1) & 1) != 0;
            let early_v = ((mask >> 2) & 1) != 0;
            let expected = (crit_v && !mid_v) ^ early_v;
            // In the rewired LUT: I0 = early_v, I1 = mid_v, I2 = crit_v
            let new_addr =
                usize::from(early_v) | (usize::from(mid_v) << 1) | (usize::from(crit_v) << 2);
            let actual = ((new_init >> new_addr) & 1) == 1;
            assert_eq!(
                actual, expected,
                "Boolean mismatch after pin permutation for (crit={crit_v}, mid={mid_v}, early={early_v})"
            );
        }
    }
}
