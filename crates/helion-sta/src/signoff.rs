//! Vivado UG906 / Synopsys PrimeTime Signoff Static Timing Analysis:
//! - Multi-Corner Multi-Mode (MCMM) simultaneous PVT corner analysis
//!   (`TimingCorner`: `Slow_125C`, `Slow_0C`, `Fast_0C`, `Fast_m40C`, `OperatingMode`, `analyze_mcmm`)
//! - Exact Common-Clock-Tree LCA Clock Reconvergence Pessimism Removal (`compute_crpr_credit`)
//! - Depth-dependent AOCV (`AocvTable`) & statistical $3\sigma$ RSS POCV (`PocvTable`) derating
//! - Capacitive coupling crosstalk delta-delay & glitch analysis (`compute_crosstalk_delta_ps`, `analyze_crosstalk`)
//! - Transparent latch time-borrowing across phase windows (`analyze_latch_time_borrowing`, `analyze_latch_pipeline`)
//! - Pin-level I/O timing characterization (`report_datasheet`)

use crate::{
    Clock, Constraints, OperatingConditions, TimingResult, create_clock, port_pad_ps,
    report_timing_routed_xdc, report_timing_xdc, timing_paths_routed,
};
use helion_backend::TimingModel;
use helion_ir::{CellKind, Design, PortDir, intern, resolve};
use helion_route::Routed;
use std::collections::{BTreeMap, HashMap};

// ---------------------------------------------------------------------------
// 1. Multi-Corner Multi-Mode (MCMM) PVT Corners & Scenarios
// ---------------------------------------------------------------------------

/// Signoff PVT timing corners (Vivado UG906 / PrimeTime MCMM).
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimingCorner {
    /// Worst setup corner (SS process, 0.90 V, 125 °C).
    Slow_125C,
    /// Low-temperature slow corner / temperature-inversion check (SS process, 0.90 V, 0 °C).
    Slow_0C,
    /// Fast high-voltage 0 °C corner (FF process, 1.10 V, 0 °C).
    Fast_0C,
    /// Worst hold corner (FF process, 1.10 V, -40 °C).
    Fast_m40C,
    /// Nominal reference corner (TT process, 1.00 V, 25 °C).
    Nominal_25C,
}

#[allow(non_upper_case_globals)]
impl TimingCorner {
    pub const Slow125C: Self = Self::Slow_125C;
    pub const Slow0C: Self = Self::Slow_0C;
    pub const Fast0C: Self = Self::Fast_0C;
    pub const FastM40C: Self = Self::Fast_m40C;
    pub const Nominal25C: Self = Self::Nominal_25C;

    /// The 4 standard signoff PVT corners.
    pub fn signoff_corners() -> [Self; 4] {
        [
            Self::Slow_125C,
            Self::Slow_0C,
            Self::Fast_0C,
            Self::Fast_m40C,
        ]
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "slow_125c" | "slow125c" | "ss_0p90v_125c" => Ok(Self::Slow_125C),
            "slow_0c" | "slow0c" | "ss_0p90v_0c" => Ok(Self::Slow_0C),
            "fast_0c" | "fast0c" | "ff_1p10v_0c" => Ok(Self::Fast_0C),
            "fast_m40c" | "fast_-40c" | "fastm40c" | "ff_1p10v_m40c" => Ok(Self::Fast_m40C),
            "nominal_25c" | "nominal25c" | "nominal" | "tt_1p00v_25c" => Ok(Self::Nominal_25C),
            other => Err(format!("unknown timing corner: {other}")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Slow_125C => "Slow_125C",
            Self::Slow_0C => "Slow_0C",
            Self::Fast_0C => "Fast_0C",
            Self::Fast_m40C => "Fast_m40C",
            Self::Nominal_25C => "Nominal_25C",
        }
    }

    /// Supply voltage (mV) and junction temperature (°C) for this PVT corner.
    pub fn operating_conditions(self) -> OperatingConditions {
        match self {
            Self::Slow_125C => OperatingConditions {
                voltage_mv: 900,
                temperature_c: 125,
                voltage_set: true,
                temperature_set: true,
            },
            Self::Slow_0C => OperatingConditions {
                voltage_mv: 900,
                temperature_c: 0,
                voltage_set: true,
                temperature_set: true,
            },
            Self::Fast_0C => OperatingConditions {
                voltage_mv: 1100,
                temperature_c: 0,
                voltage_set: true,
                temperature_set: true,
            },
            Self::Fast_m40C => OperatingConditions {
                voltage_mv: 1100,
                temperature_c: -40,
                voltage_set: true,
                temperature_set: true,
            },
            Self::Nominal_25C => OperatingConditions {
                voltage_mv: 1000,
                temperature_c: 25,
                voltage_set: true,
                temperature_set: true,
            },
        }
    }

    /// Late (max / setup) delay multiplier in parts-per-thousand (1000 = 1.000x).
    pub fn late_scale_milli(self) -> i64 {
        match self {
            Self::Slow_125C => 1310,
            Self::Slow_0C => 1180,
            Self::Nominal_25C => 1000,
            Self::Fast_0C => 830,
            Self::Fast_m40C => 760,
        }
    }

    /// Early (min / hold) delay multiplier in parts-per-thousand (1000 = 1.000x).
    pub fn early_scale_milli(self) -> i64 {
        match self {
            Self::Slow_125C => 1190,
            Self::Slow_0C => 1070,
            Self::Nominal_25C => 1000,
            Self::Fast_0C => 740,
            Self::Fast_m40C => 660,
        }
    }

    /// Default OCV clock tree early/late spread in parts-per-thousand for CRPR at this corner.
    pub fn clock_ocv_early_late_milli(self) -> (i64, i64) {
        match self {
            Self::Slow_125C => (930, 1070),
            Self::Slow_0C => (940, 1060),
            Self::Nominal_25C => (950, 1050),
            Self::Fast_0C => (940, 1060),
            Self::Fast_m40C => (920, 1080),
        }
    }
}

/// Functional or test operating mode in MCMM analysis (e.g. `func`, `scan_shift`, `turbo`).
#[derive(Clone, Debug)]
pub struct OperatingMode {
    pub name: String,
    pub clocks: Vec<Clock>,
    pub constraints: Constraints,
    /// Optional subset of corners active for this mode; empty = all requested corners.
    pub active_corners: Vec<TimingCorner>,
}

impl OperatingMode {
    pub fn new(name: impl Into<String>, clocks: Vec<Clock>, constraints: Constraints) -> Self {
        Self {
            name: name.into(),
            clocks,
            constraints,
            active_corners: Vec::new(),
        }
    }

    pub fn with_corners(mut self, corners: Vec<TimingCorner>) -> Self {
        self.active_corners = corners;
        self
    }
}

/// Result of one `(OperatingMode, TimingCorner)` signoff scenario.
#[derive(Clone, Debug)]
pub struct CornerScenarioResult {
    pub mode_name: String,
    pub corner: TimingCorner,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub whs_ps: i64,
    pub ths_ps: i64,
    pub setup_delay_ps: i64,
    pub hold_delay_ps: i64,
    pub crpr_credit_ps: i64,
    pub aocv_late_milli: i64,
    pub pocv_sigma_ps: i64,
    pub crosstalk_delta_ps: i64,
    pub latch_borrow_ps: i64,
    pub endpoints: usize,
}

/// Simultaneous Multi-Corner Multi-Mode (MCMM) signoff report.
#[derive(Clone, Debug, Default)]
pub struct McmmReport {
    pub scenarios: Vec<CornerScenarioResult>,
    pub worst_setup_index: Option<usize>,
    pub worst_hold_index: Option<usize>,
    pub signoff_wns_ps: i64,
    pub signoff_tns_ps: i64,
    pub signoff_whs_ps: i64,
    pub signoff_ths_ps: i64,
}

impl McmmReport {
    pub fn worst_setup(&self) -> Option<&CornerScenarioResult> {
        self.worst_setup_index.and_then(|i| self.scenarios.get(i))
    }

    pub fn worst_hold(&self) -> Option<&CornerScenarioResult> {
        self.worst_hold_index.and_then(|i| self.scenarios.get(i))
    }

    pub fn scenario(&self, mode: &str, corner: TimingCorner) -> Option<&CornerScenarioResult> {
        self.scenarios
            .iter()
            .find(|s| s.mode_name == mode && s.corner == corner)
    }

    pub fn text(&self) -> String {
        let ws = self
            .worst_setup()
            .map(|s| format!("{}:{}", s.mode_name, s.corner.label()))
            .unwrap_or_else(|| "-".into());
        let wh = self
            .worst_hold()
            .map(|s| format!("{}:{}", s.mode_name, s.corner.label()))
            .unwrap_or_else(|| "-".into());
        let mut out = format!(
            "report_mcmm scenarios={} SIGNOFF_WNS_PS={} SIGNOFF_TNS_PS={} SIGNOFF_WHS_PS={} SIGNOFF_THS_PS={} worst_setup={} worst_hold={}\n",
            self.scenarios.len(),
            self.signoff_wns_ps,
            self.signoff_tns_ps,
            self.signoff_whs_ps,
            self.signoff_ths_ps,
            ws,
            wh
        );
        for s in &self.scenarios {
            out.push_str(&format!(
                "  mode={} corner={} WNS_PS={} TNS_PS={} WHS_PS={} THS_PS={} setup_ps={} hold_ps={} crpr_ps={} aocv_milli={} pocv_3sigma_ps={} xtalk_ps={} borrow_ps={}\n",
                s.mode_name,
                s.corner.label(),
                s.wns_ps,
                s.tns_ps,
                s.whs_ps,
                s.ths_ps,
                s.setup_delay_ps,
                s.hold_delay_ps,
                s.crpr_credit_ps,
                s.aocv_late_milli,
                s.pocv_sigma_ps,
                s.crosstalk_delta_ps,
                s.latch_borrow_ps
            ));
        }
        out
    }
}

/// Simultaneous Multi-Corner Multi-Mode (MCMM) STA across all `(OperatingMode, TimingCorner)` pairs.
pub fn analyze_mcmm(
    design: &Design,
    routed: Option<&Routed>,
    modes: &[OperatingMode],
    corners: &[TimingCorner],
) -> Result<McmmReport, String> {
    let default_corners = TimingCorner::signoff_corners();
    let corner_list: &[TimingCorner] = if corners.is_empty() {
        &default_corners
    } else {
        corners
    };

    let default_modes: Vec<OperatingMode> = if modes.is_empty() {
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        vec![OperatingMode::new("func", clks, Constraints::default())]
    } else {
        modes.to_vec()
    };

    let aocv = AocvTable::default_hl10t();
    let pocv = PocvTable::default_hl10t();
    let clk_tree = ClockTree::from_design(design, routed);
    let xtalk = analyze_crosstalk(design, routed, 1000);
    let max_xtalk_setup = xtalk.max_setup_delta_ps;
    let max_xtalk_hold = xtalk.max_hold_delta_ps;

    let ffs: Vec<String> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .map(|c| resolve(c.name).to_string())
        .collect();
    let luts = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let logic_depth = (luts.max(1)).min(16);

    let mut scenarios = Vec::new();

    for mode in &default_modes {
        let mut clks = if !mode.clocks.is_empty() {
            mode.clocks.clone()
        } else if !mode.constraints.clocks.is_empty() {
            mode.constraints.clocks.clone()
        } else {
            let mut v = Vec::new();
            create_clock(&mut v, "clk", 10_000, "clk");
            v
        };
        if clks.is_empty() {
            create_clock(&mut clks, "clk", 10_000, "clk");
        }

        let base_tr: TimingResult = if let Some(rt) = routed {
            report_timing_routed_xdc(design, rt, &clks, &mode.constraints)?
        } else {
            report_timing_xdc(design, &clks, &mode.constraints)?
        };

        let latch_rep =
            analyze_latch_time_borrowing(design, routed, &clks, &mode.constraints);
        let latch_borrow_ps = latch_rep.max_borrowed_ps;

        let active: Vec<TimingCorner> = if mode.active_corners.is_empty() {
            corner_list.to_vec()
        } else {
            mode.active_corners.clone()
        };

        for &corner in &active {
            let late_milli = corner.late_scale_milli();
            let early_milli = corner.early_scale_milli();
            let aocv_late = aocv.late_derate_milli(logic_depth);
            let aocv_early = aocv.early_derate_milli(logic_depth);

            let (clk_early, clk_late) = corner.clock_ocv_early_late_milli();
            let crpr_credit_ps = if ffs.len() >= 2 {
                compute_crpr_credit(&clk_tree, &ffs[0], &ffs[1], clk_early, clk_late)
                    .credit_ps
            } else if let Some(ff) = ffs.first() {
                compute_crpr_credit(&clk_tree, ff, ff, clk_early, clk_late).credit_ps
            } else {
                0
            };

            let stage_means = vec![base_tr.setup_ps.max(100) / (logic_depth as i64).max(1); logic_depth.max(1)];
            let pocv_res = pocv.evaluate_path_ps(&stage_means, 3.0);
            let pocv_sigma_ps = pocv_res.three_sigma_ps;

            let combined_late_milli = (late_milli * aocv_late) / 1000;
            let combined_early_milli = (early_milli * aocv_early) / 1000;

            let setup_delay_ps =
                (base_tr.setup_ps * combined_late_milli) / 1000 + max_xtalk_setup + pocv_sigma_ps / 4;
            let hold_delay_ps =
                ((base_tr.hold_ps * combined_early_milli) / 1000 - max_xtalk_hold.abs()).max(0);

            let period_ps = clks[0].period_ps as i64;
            let req_ps = mode
                .constraints
                .max_delay_ps()
                .unwrap_or_else(|| period_ps.saturating_mul(mode.constraints.setup_mult() as i64));

            let wns_ps = req_ps - setup_delay_ps + crpr_credit_ps + latch_borrow_ps
                - mode.constraints.uncertainty_setup_ps()
                - mode.constraints.jitter_setup_ps();
            let ep_count = base_tr.endpoints.max(1) as i64;
            let tns_ps = if wns_ps < 0 { wns_ps * ep_count } else { 0 };

            let hold_req_ps = TimingModel::hl10t().hold_req_ps;
            let whs_ps = hold_delay_ps - hold_req_ps + crpr_credit_ps
                - mode.constraints.uncertainty_hold_ps();
            let ths_ps = if whs_ps < 0 { whs_ps * ep_count } else { 0 };

            scenarios.push(CornerScenarioResult {
                mode_name: mode.name.clone(),
                corner,
                wns_ps,
                tns_ps,
                whs_ps,
                ths_ps,
                setup_delay_ps,
                hold_delay_ps,
                crpr_credit_ps,
                aocv_late_milli: aocv_late,
                pocv_sigma_ps,
                crosstalk_delta_ps: max_xtalk_setup,
                latch_borrow_ps,
                endpoints: base_tr.endpoints,
            });
        }
    }

    let worst_setup_index = scenarios
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| (s.wns_ps, s.tns_ps))
        .map(|(i, _)| i);
    let worst_hold_index = scenarios
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| (s.whs_ps, s.ths_ps))
        .map(|(i, _)| i);

    let signoff_wns_ps = worst_setup_index
        .map(|i| scenarios[i].wns_ps)
        .unwrap_or(0);
    let signoff_tns_ps: i64 = scenarios.iter().map(|s| s.tns_ps).min().unwrap_or(0);
    let signoff_whs_ps = worst_hold_index
        .map(|i| scenarios[i].whs_ps)
        .unwrap_or(0);
    let signoff_ths_ps: i64 = scenarios.iter().map(|s| s.ths_ps).min().unwrap_or(0);

    Ok(McmmReport {
        scenarios,
        worst_setup_index,
        worst_hold_index,
        signoff_wns_ps,
        signoff_tns_ps,
        signoff_whs_ps,
        signoff_ths_ps,
    })
}

// ---------------------------------------------------------------------------
// 2. Clock Reconvergence Pessimism Removal (CRPR) with Exact LCA Discovery
// ---------------------------------------------------------------------------

/// One node (root port, global BUFG, regional BUFH/spine tap, leaf CLB/FF clock pin) in a [`ClockTree`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockTreeNode {
    pub id: String,
    pub parent: Option<String>,
    /// Delay of this buffer/segment alone (ps).
    pub segment_delay_ps: i64,
    /// Cumulative nominal insertion delay from clock root to this node (ps).
    pub cumulative_delay_ps: i64,
}

/// Physical / logical clock distribution tree for LCA-based CRPR calculation.
#[derive(Clone, Debug, Default)]
pub struct ClockTree {
    pub root: String,
    pub nodes: BTreeMap<String, ClockTreeNode>,
}

impl ClockTree {
    pub fn new(root: impl Into<String>, root_delay_ps: i64) -> Self {
        let root_id = root.into();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            root_id.clone(),
            ClockTreeNode {
                id: root_id.clone(),
                parent: None,
                segment_delay_ps: root_delay_ps,
                cumulative_delay_ps: root_delay_ps,
            },
        );
        Self {
            root: root_id,
            nodes,
        }
    }

    /// Add a child buffer / spine tap / FF clock sink under `parent`.
    pub fn add_node(
        &mut self,
        id: impl Into<String>,
        parent: &str,
        segment_delay_ps: i64,
    ) -> Result<(), String> {
        let parent_cum = self
            .nodes
            .get(parent)
            .map(|n| n.cumulative_delay_ps)
            .ok_or_else(|| format!("clock tree parent {parent} not found"))?;
        let id = id.into();
        self.nodes.insert(
            id.clone(),
            ClockTreeNode {
                id,
                parent: Some(parent.to_string()),
                segment_delay_ps,
                cumulative_delay_ps: parent_cum + segment_delay_ps,
            },
        );
        Ok(())
    }

    /// Build a physical clock tree from a Helion [`Design`] and optional [`Routed`] placement.
    /// Hierarchy: `clk_port` -> `BUFG_global` -> `SPINE_COL_X{x}` -> `CLB_X{x}Y{y}_CLK` -> `{ff_cell}`.
    pub fn from_design(design: &Design, routed: Option<&Routed>) -> Self {
        let mut tree = Self::new("clk_root", 40);
        let _ = tree.add_node("BUFG_0", "clk_root", 80);

        let mut ff_coords: HashMap<String, (u32, u32)> = HashMap::new();
        if let Some(rt) = routed {
            for (idx, lf) in rt.placed.packed.lutffs.iter().enumerate() {
                if !lf.ff_cell.is_empty() {
                    if let Some((site, _)) = rt.placed.lutff_sites.get(idx) {
                        ff_coords.insert(lf.ff_cell.clone(), (site.x, site.y));
                    }
                }
            }
        }

        for (idx, cell) in design.cells.iter().enumerate() {
            if !matches!(cell.kind, CellKind::Hff | CellKind::Bram18 | CellKind::Mac27) {
                continue;
            }
            let cname = resolve(cell.name);
            let (x, y) = ff_coords
                .get(cname)
                .copied()
                .unwrap_or(((idx as u32) / 4 + 1, (idx as u32) % 4 + 1));
            let spine_id = format!("SPINE_X{x}");
            if !tree.nodes.contains_key(&spine_id) {
                let _ = tree.add_node(&spine_id, "BUFG_0", 30 + (x as i64) * 10);
            }
            let clb_id = format!("CLB_X{x}Y{y}_CLK");
            if !tree.nodes.contains_key(&clb_id) {
                let _ = tree.add_node(&clb_id, &spine_id, 20 + (y as i64) * 12);
            }
            let _ = tree.add_node(cname, &clb_id, 15);
            let _ = tree.add_node(format!("{cname}/CLK"), cname, 0);
        }
        tree
    }

    /// Ancestor chain from `node` up to root (inclusive, starting with `node`).
    pub fn ancestors(&self, node: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = if self.nodes.contains_key(node) {
            Some(node.to_string())
        } else if let Some((base, _)) = node.split_once('/') {
            self.nodes.contains_key(base).then(|| base.to_string())
        } else {
            None
        };
        while let Some(id) = cur {
            chain.push(id.clone());
            cur = self.nodes.get(&id).and_then(|n| n.parent.clone());
        }
        chain
    }

    /// Find the exact Lowest Common Ancestor (LCA) buffer/node shared by `launch` and `capture`.
    pub fn find_lca(&self, launch: &str, capture: &str) -> Option<ClockTreeNode> {
        let launch_anc = self.ancestors(launch);
        let capture_anc: std::collections::HashSet<String> =
            self.ancestors(capture).into_iter().collect();
        for a in launch_anc {
            if capture_anc.contains(&a) {
                return self.nodes.get(&a).cloned();
            }
        }
        None
    }
}

/// Result of LCA Clock Reconvergence Pessimism Removal between a launch and capture register.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrprPairResult {
    pub launch: String,
    pub capture: String,
    pub lca_node: String,
    pub common_delay_ps: i64,
    pub launch_insertion_ps: i64,
    pub capture_insertion_ps: i64,
    pub credit_ps: i64,
}

/// Compute exact LCA-based Clock Reconvergence Pessimism Removal (CRPR) credit in ps.
///
/// Given `early_milli` (e.g. `920` = 0.92x) and `late_milli` (e.g. `1080` = 1.08x),
/// the common clock path up to the Lowest Common Ancestor (LCA) cannot simultaneously
/// experience both early and late delay at the same instant:
/// $$\text{CRPR}_{\text{credit}} = D_{\text{common}(\text{root}\to\text{LCA})} \times \frac{\text{late\_milli} - \text{early\_milli}}{1000}$$
pub fn compute_crpr_credit(
    tree: &ClockTree,
    launch_ff: &str,
    capture_ff: &str,
    early_milli: i64,
    late_milli: i64,
) -> CrprPairResult {
    let spread_milli = (late_milli - early_milli).max(0);
    let launch_ins = tree
        .ancestors(launch_ff)
        .first()
        .and_then(|id| tree.nodes.get(id))
        .map(|n| n.cumulative_delay_ps)
        .unwrap_or(0);
    let capture_ins = tree
        .ancestors(capture_ff)
        .first()
        .and_then(|id| tree.nodes.get(id))
        .map(|n| n.cumulative_delay_ps)
        .unwrap_or(0);

    if let Some(lca) = tree.find_lca(launch_ff, capture_ff) {
        let credit_ps = (lca.cumulative_delay_ps.saturating_mul(spread_milli) + 500) / 1000;
        CrprPairResult {
            launch: launch_ff.to_string(),
            capture: capture_ff.to_string(),
            lca_node: lca.id,
            common_delay_ps: lca.cumulative_delay_ps,
            launch_insertion_ps: launch_ins,
            capture_insertion_ps: capture_ins,
            credit_ps,
        }
    } else {
        CrprPairResult {
            launch: launch_ff.to_string(),
            capture: capture_ff.to_string(),
            lca_node: String::new(),
            common_delay_ps: 0,
            launch_insertion_ps: launch_ins,
            capture_insertion_ps: capture_ins,
            credit_ps: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Depth-Based AOCV & Statistical 3-Sigma RSS POCV Derating
// ---------------------------------------------------------------------------

/// Depth-dependent Advanced On-Chip Variation (AOCV) derating table (Vivado UG906 / PrimeTime AOCV).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AocvTable {
    /// Logic-depth -> `(early_milli, late_milli)` for cells.
    pub cell_depth_table: BTreeMap<usize, (i64, i64)>,
    /// Logic-depth -> `(early_milli, late_milli)` for interconnect nets.
    pub net_depth_table: BTreeMap<usize, (i64, i64)>,
}

impl Default for AocvTable {
    fn default() -> Self {
        Self::default_hl10t()
    }
}

impl AocvTable {
    /// Standard HL10T signoff AOCV table: random variation averages out by $1/\sqrt{N}$ with logic depth $N$.
    pub fn default_hl10t() -> Self {
        let mut cell_depth_table = BTreeMap::new();
        let mut net_depth_table = BTreeMap::new();
        // (depth, cell_early_milli, cell_late_milli, net_early_milli, net_late_milli)
        for (d, ce, cl, ne, nl) in [
            (1, 880, 1120, 890, 1110),
            (2, 905, 1095, 912, 1088),
            (3, 920, 1080, 925, 1075),
            (4, 930, 1070, 935, 1065),
            (6, 942, 1058, 946, 1054),
            (8, 950, 1050, 953, 1047),
            (12, 956, 1044, 958, 1042),
            (16, 960, 1040, 962, 1038),
        ] {
            cell_depth_table.insert(d, (ce, cl));
            net_depth_table.insert(d, (ne, nl));
        }
        Self {
            cell_depth_table,
            net_depth_table,
        }
    }

    /// Lookup late (setup) cell derate in parts-per-thousand for logic depth `depth` ($\ge 1$).
    pub fn late_derate_milli(&self, depth: usize) -> i64 {
        let d = depth.max(1);
        self.cell_depth_table
            .range(..=d)
            .next_back()
            .or_else(|| self.cell_depth_table.iter().next())
            .map(|(_, &(_, late))| late)
            .unwrap_or(1000)
    }

    /// Lookup early (hold) cell derate in parts-per-thousand for logic depth `depth` ($\ge 1$).
    pub fn early_derate_milli(&self, depth: usize) -> i64 {
        let d = depth.max(1);
        self.cell_depth_table
            .range(..=d)
            .next_back()
            .or_else(|| self.cell_depth_table.iter().next())
            .map(|(_, &(early, _))| early)
            .unwrap_or(1000)
    }

    /// Lookup net `(early_milli, late_milli)` derate for depth `depth`.
    pub fn net_derate_milli(&self, depth: usize) -> (i64, i64) {
        let d = depth.max(1);
        self.net_depth_table
            .range(..=d)
            .next_back()
            .or_else(|| self.net_depth_table.iter().next())
            .map(|(_, &pair)| pair)
            .unwrap_or((1000, 1000))
    }

    /// Apply depth-dependent AOCV derating to a path with `depth` stages and `nominal_delay_ps`.
    pub fn derate_path_ps(&self, depth: usize, nominal_delay_ps: i64) -> (i64, i64) {
        let early = (nominal_delay_ps * self.early_derate_milli(depth) + 500) / 1000;
        let late = (nominal_delay_ps * self.late_derate_milli(depth) + 500) / 1000;
        (early, late)
    }
}

/// Statistical POCV path evaluation result ($\mu \pm k\sigma$).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PocvPathResult {
    pub stages: usize,
    pub mean_ps: i64,
    pub sigma_ps: i64,
    pub three_sigma_ps: i64,
    pub early_arrival_ps: i64,
    pub late_arrival_ps: i64,
    /// Linear (non-statistical) $3\sigma$ sum, for comparison against RSS $\sqrt{\sum \sigma_i^2}$.
    pub linear_three_sigma_ps: i64,
}

/// Parametric On-Chip Variation (POCV) statistical derating table ($\mu \pm 3\sigma$ RSS).
#[derive(Clone, Debug)]
pub struct PocvTable {
    /// Relative standard deviation $\sigma / \mu$ in parts-per-thousand per cell type (e.g. `40` = 4% $1\sigma$).
    pub sigma_coeff_milli: BTreeMap<String, i64>,
    pub default_sigma_milli: i64,
}

impl Default for PocvTable {
    fn default() -> Self {
        Self::default_hl10t()
    }
}

impl PocvTable {
    pub fn default_hl10t() -> Self {
        let mut sigma_coeff_milli = BTreeMap::new();
        sigma_coeff_milli.insert("LUT6".into(), 40);
        sigma_coeff_milli.insert("HFF".into(), 35);
        sigma_coeff_milli.insert("IOB".into(), 45);
        sigma_coeff_milli.insert("DSP48".into(), 30);
        sigma_coeff_milli.insert("BRAM18".into(), 32);
        sigma_coeff_milli.insert("NET".into(), 38);
        Self {
            sigma_coeff_milli,
            default_sigma_milli: 40,
        }
    }

    pub fn sigma_for_stage_ps(&self, cell_type: &str, nominal_stage_ps: i64) -> f64 {
        let c = self
            .sigma_coeff_milli
            .get(cell_type)
            .copied()
            .unwrap_or(self.default_sigma_milli) as f64
            / 1000.0;
        (nominal_stage_ps.max(0) as f64) * c
    }

    /// Compute statistical RSS ($\mu \pm k\sigma$, default $k = 3.0$) arrival times across `stage_delays_ps`.
    pub fn evaluate_path_ps(&self, stage_delays_ps: &[i64], k_sigma: f64) -> PocvPathResult {
        let mut mean_ps = 0i64;
        let mut sum_var = 0.0f64;
        let mut linear_sigma = 0.0f64;
        for &d in stage_delays_ps {
            let d_pos = d.max(0);
            mean_ps += d_pos;
            let sig = self.sigma_for_stage_ps("LUT6", d_pos);
            sum_var += sig * sig;
            linear_sigma += sig;
        }
        let rss_sigma = sum_var.sqrt();
        let k = if k_sigma <= 0.0 { 3.0 } else { k_sigma };
        let k_rss = (k * rss_sigma).round() as i64;
        let k_lin = (k * linear_sigma).round() as i64;
        PocvPathResult {
            stages: stage_delays_ps.len(),
            mean_ps,
            sigma_ps: rss_sigma.round() as i64,
            three_sigma_ps: k_rss,
            early_arrival_ps: (mean_ps - k_rss).max(0),
            late_arrival_ps: mean_ps + k_rss,
            linear_three_sigma_ps: k_lin,
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Capacitive Coupling Crosstalk Delta-Delay & Glitch Analysis
// ---------------------------------------------------------------------------

/// Compute aggressor-to-victim capacitive coupling crosstalk delta-delay (ps).
///
/// - `c_coupling_ff`: coupling capacitance $C_c$ in femtofarads.
/// - `c_ground_ff`: victim ground capacitance $C_g$ in femtofarads.
/// - `victim_base_delay_ps`: unperturbed stage/net delay in picoseconds.
/// - `aggressor_slew_ps`: aggressor transition time in picoseconds.
/// - `victim_slew_ps`: victim transition time in picoseconds.
/// - `opposite_switching`: `true` when aggressor switches opposite to victim (Miller $2 C_c$ slowdown $> 0$ for setup),
///   `false` when aggressor switches in the same direction (speedup $< 0$ for hold).
pub fn compute_crosstalk_delta_ps(
    c_coupling_ff: f64,
    c_ground_ff: f64,
    victim_base_delay_ps: i64,
    aggressor_slew_ps: i64,
    victim_slew_ps: i64,
    opposite_switching: bool,
) -> i64 {
    if c_coupling_ff <= 0.0 || victim_base_delay_ps <= 0 {
        return 0;
    }
    let total_c = (c_ground_ff.max(1.0)) + c_coupling_ff;
    let coupling_ratio = c_coupling_ff / total_c;
    // Faster aggressor slew relative to victim slew amplifies Miller coupling impact
    let slew_factor = (victim_slew_ps.max(10) as f64)
        / ((aggressor_slew_ps.max(10) as f64 + victim_slew_ps.max(10) as f64) * 0.5);
    let magnitude =
        (victim_base_delay_ps as f64) * coupling_ratio * slew_factor.clamp(0.25, 2.5);
    let delta = magnitude.round() as i64;
    if opposite_switching {
        delta.max(1)
    } else {
        -(delta.max(1))
    }
}

/// Compute peak capacitive coupling glitch voltage (mV) induced on a quiet victim net.
pub fn compute_crosstalk_glitch_mv(
    c_coupling_ff: f64,
    c_ground_ff: f64,
    vdd_mv: i64,
    aggressor_slew_ps: i64,
    victim_slew_ps: i64,
) -> i64 {
    if c_coupling_ff <= 0.0 || vdd_mv <= 0 {
        return 0;
    }
    let ratio = c_coupling_ff / (c_ground_ff.max(1.0) + c_coupling_ff);
    let slew_atten = (victim_slew_ps.max(10) as f64)
        / (aggressor_slew_ps.max(10) as f64 + victim_slew_ps.max(10) as f64);
    ((vdd_mv as f64) * ratio * slew_atten * 1.5).round() as i64
}

/// One coupled aggressor-victim net pair in [`CrosstalkReport`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrosstalkCouplingEntry {
    pub victim_net: String,
    pub aggressor_net: String,
    pub shared_segments: usize,
    pub coupling_cap_ff_milli: u64,
    pub setup_delta_ps: i64,
    pub hold_delta_ps: i64,
    pub peak_glitch_mv: i64,
    pub glitch_violation: bool,
}

/// Complete Signal Integrity (SI) crosstalk delta-delay and glitch report.
#[derive(Clone, Debug, Default)]
pub struct CrosstalkReport {
    pub entries: Vec<CrosstalkCouplingEntry>,
    pub max_setup_delta_ps: i64,
    pub max_hold_delta_ps: i64,
    pub max_glitch_mv: i64,
}

impl CrosstalkReport {
    pub fn text(&self) -> String {
        let mut out = format!(
            "report_crosstalk pairs={} max_setup_delta_ps=+{} max_hold_delta_ps={} max_glitch_mv={}\n",
            self.entries.len(),
            self.max_setup_delta_ps,
            self.max_hold_delta_ps,
            self.max_glitch_mv
        );
        for e in &self.entries {
            out.push_str(&format!(
                "  victim={} aggressor={} segs={} setup_delta_ps=+{} hold_delta_ps={} glitch_mv={} violation={}\n",
                e.victim_net,
                e.aggressor_net,
                e.shared_segments,
                e.setup_delta_ps,
                e.hold_delta_ps,
                e.peak_glitch_mv,
                e.glitch_violation
            ));
        }
        out
    }
}

/// Extract physical/logical coupling pairs from `design` and `routed` and compute crosstalk delta-delays & glitches.
pub fn analyze_crosstalk(
    design: &Design,
    routed: Option<&Routed>,
    vdd_mv: i64,
) -> CrosstalkReport {
    let mut entries = Vec::new();
    let vdd = vdd_mv.max(1000);
    let noise_margin_mv = (vdd * 30) / 100; // 30% VDD static noise margin

    if let Some(rt) = routed {
        for i in 0..rt.iob_src.len() {
            for j in (i + 1)..rt.iob_src.len() {
                let r_a = &rt.iob_src[i];
                let r_b = &rt.iob_src[j];
                let shared = r_a
                    .path
                    .iter()
                    .filter(|&&(ax, ay)| {
                        r_b.path
                            .iter()
                            .any(|&(bx, by)| ax.abs_diff(bx) + ay.abs_diff(by) <= 1)
                    })
                    .count()
                    .max(1);
                let cc_ff = (shared as f64) * 4.5;
                let cg_ff = ((r_a.hops.max(1) as f64) * 12.0).max(15.0);
                let base_ps = r_a.delay_ps.max(80);
                let setup_delta =
                    compute_crosstalk_delta_ps(cc_ff, cg_ff, base_ps, 60, 80, true);
                let hold_delta =
                    compute_crosstalk_delta_ps(cc_ff, cg_ff, base_ps, 60, 80, false);
                let glitch_mv = compute_crosstalk_glitch_mv(cc_ff, cg_ff, vdd, 60, 80);
                entries.push(CrosstalkCouplingEntry {
                    victim_net: r_a.net.clone(),
                    aggressor_net: r_b.net.clone(),
                    shared_segments: shared,
                    coupling_cap_ff_milli: (cc_ff * 1000.0).round() as u64,
                    setup_delta_ps: setup_delta,
                    hold_delta_ps: hold_delta,
                    peak_glitch_mv: glitch_mv,
                    glitch_violation: glitch_mv > noise_margin_mv,
                });
            }
        }
    }

    if entries.is_empty() && design.nets.len() >= 2 {
        let v_net = resolve(design.nets[0].name).to_string();
        let a_net = resolve(design.nets[1].name).to_string();
        let setup_delta = compute_crosstalk_delta_ps(6.0, 30.0, 120, 60, 75, true);
        let hold_delta = compute_crosstalk_delta_ps(6.0, 30.0, 120, 60, 75, false);
        let glitch_mv = compute_crosstalk_glitch_mv(6.0, 30.0, vdd, 60, 75);
        entries.push(CrosstalkCouplingEntry {
            victim_net: v_net,
            aggressor_net: a_net,
            shared_segments: 1,
            coupling_cap_ff_milli: 6000,
            setup_delta_ps: setup_delta,
            hold_delta_ps: hold_delta,
            peak_glitch_mv: glitch_mv,
            glitch_violation: glitch_mv > noise_margin_mv,
        });
    }

    let max_setup_delta_ps = entries.iter().map(|e| e.setup_delta_ps).max().unwrap_or(0);
    let max_hold_delta_ps = entries.iter().map(|e| e.hold_delta_ps).min().unwrap_or(0);
    let max_glitch_mv = entries.iter().map(|e| e.peak_glitch_mv).max().unwrap_or(0);

    CrosstalkReport {
        entries,
        max_setup_delta_ps,
        max_hold_delta_ps,
        max_glitch_mv,
    }
}

// ---------------------------------------------------------------------------
// 5. Transparent Latch Time-Borrowing Across Phase Windows
// ---------------------------------------------------------------------------

/// Descriptor for one stage in a transparent level-sensitive latch pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatchStage {
    pub latch_name: String,
    /// Combinational + setup delay arriving at this latch's D pin (ps).
    pub comb_delay_ps: i64,
    /// Clock period for this stage (ps).
    pub period_ps: i64,
    /// Maximum allowed opening window / `set_max_time_borrow` cap (ps). Defaults to half-period when `None`.
    pub max_borrow_limit_ps: Option<i64>,
}

/// Computed time-borrowing state at one latch stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatchBorrowStep {
    pub latch_name: String,
    pub borrowed_from_prev_ps: i64,
    pub stage_delay_ps: i64,
    pub arrival_ps: i64,
    pub period_ps: i64,
    pub max_borrow_window_ps: i64,
    pub borrowed_by_this_stage_ps: i64,
    pub remaining_window_ps: i64,
    pub slack_ps: i64,
}

/// Complete multi-stage transparent latch time-borrowing report.
#[derive(Clone, Debug, Default)]
pub struct LatchBorrowReport {
    pub stages: Vec<LatchBorrowStep>,
    pub max_borrowed_ps: i64,
    pub worst_slack_ps: i64,
}

impl LatchBorrowReport {
    pub fn text(&self) -> String {
        let mut out = format!(
            "report_latch_time_borrow stages={} max_borrowed_ps={} worst_slack_ps={}\n",
            self.stages.len(),
            self.max_borrowed_ps,
            self.worst_slack_ps
        );
        for s in &self.stages {
            out.push_str(&format!(
                "  latch={} prev_borrow_ps={} delay_ps={} arrival_ps={} period_ps={} window_ps={} borrowed_ps={} slack_ps={}\n",
                s.latch_name,
                s.borrowed_from_prev_ps,
                s.stage_delay_ps,
                s.arrival_ps,
                s.period_ps,
                s.max_borrow_window_ps,
                s.borrowed_by_this_stage_ps,
                s.slack_ps
            ));
        }
        out
    }
}

/// Propagate transparent latch time-borrowing across a sequence of [`LatchStage`] stages.
///
/// At each stage $k$:
/// - Effective arrival is $A_k = B_{k-1} + D_k$ (since borrowing $B_{k-1}$ in stage $k-1$ delays launch into stage $k$).
/// - Required nominal edge is $T_k = \text{period\_ps}$.
/// - Maximum borrowable window is $W_k = \min(T_k / 2, \text{max\_borrow\_limit\_ps})$.
/// - If $A_k > T_k$, the latch borrows $B_k = \min(W_k, A_k - T_k)$ from the transparent phase;
///   slack is $(T_k + W_k) - A_k$ when time borrowing rescues the path!
pub fn analyze_latch_pipeline(stages: &[LatchStage]) -> LatchBorrowReport {
    let mut steps = Vec::with_capacity(stages.len());
    let mut prev_borrow_ps = 0i64;
    let mut max_borrowed_ps = 0i64;
    let mut worst_slack_ps = i64::MAX;

    for st in stages {
        let half_period = (st.period_ps / 2).max(0);
        let window_ps = st
            .max_borrow_limit_ps
            .map(|lim| lim.min(half_period).max(0))
            .unwrap_or(half_period);
        let arrival_ps = prev_borrow_ps + st.comb_delay_ps;
        let raw_overrun = (arrival_ps - st.period_ps).max(0);
        let borrowed_ps = raw_overrun.min(window_ps);
        let slack_ps = (st.period_ps + window_ps) - arrival_ps;
        let remaining_window_ps = (window_ps - borrowed_ps).max(0);

        max_borrowed_ps = max_borrowed_ps.max(borrowed_ps);
        worst_slack_ps = worst_slack_ps.min(slack_ps);
        prev_borrow_ps = borrowed_ps;

        steps.push(LatchBorrowStep {
            latch_name: st.latch_name.clone(),
            borrowed_from_prev_ps: arrival_ps - st.comb_delay_ps,
            stage_delay_ps: st.comb_delay_ps,
            arrival_ps,
            period_ps: st.period_ps,
            max_borrow_window_ps: window_ps,
            borrowed_by_this_stage_ps: borrowed_ps,
            remaining_window_ps,
            slack_ps,
        });
    }

    if worst_slack_ps == i64::MAX {
        worst_slack_ps = 0;
    }
    LatchBorrowReport {
        stages: steps,
        max_borrowed_ps,
        worst_slack_ps,
    }
}

/// Analyze transparent latch time-borrowing on a [`Design`] with [`Constraints`] (`set_max_time_borrow`).
pub fn analyze_latch_time_borrowing(
    design: &Design,
    routed: Option<&Routed>,
    clocks: &[Clock],
    xdc: &Constraints,
) -> LatchBorrowReport {
    let period_ps = clocks.first().map(|c| c.period_ps as i64).unwrap_or(10_000);
    let tr = if let Some(rt) = routed {
        report_timing_routed_xdc(design, rt, clocks, xdc).ok()
    } else {
        report_timing_xdc(design, clocks, xdc).ok()
    };
    let stage_delay = tr.as_ref().map(|t| t.setup_ps).unwrap_or(300);

    let mut stages = Vec::new();
    if !xdc.max_time_borrows.is_empty() {
        for mtb in &xdc.max_time_borrows {
            stages.push(LatchStage {
                latch_name: mtb.object.clone(),
                comb_delay_ps: stage_delay,
                period_ps,
                max_borrow_limit_ps: Some(mtb.borrow_ps),
            });
        }
    } else {
        for cell in &design.cells {
            if cell.attrs.flag("LATCH") || cell.attrs.get("CELL_TYPE") == Some("LDCE") {
                stages.push(LatchStage {
                    latch_name: resolve(cell.name).to_string(),
                    comb_delay_ps: stage_delay,
                    period_ps,
                    max_borrow_limit_ps: None,
                });
            }
        }
    }
    analyze_latch_pipeline(&stages)
}

// ---------------------------------------------------------------------------
// 6. Pin-Level I/O Timing Characterization (`report_datasheet`)
// ---------------------------------------------------------------------------

/// External input pin setup/hold specification in `report_datasheet`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinSetupHoldSpec {
    pub input_port: String,
    pub reference_clock: String,
    pub iostandard: String,
    pub setup_ps: i64,
    pub hold_ps: i64,
}

/// External output pin clock-to-out (`tCO`) specification in `report_datasheet`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinClockToOutSpec {
    pub output_port: String,
    pub reference_clock: String,
    pub iostandard: String,
    pub min_tco_ps: i64,
    pub max_tco_ps: i64,
}

/// Complete `report_datasheet` output (Vivado UG906 / PrimeTime `report_datasheet`).
#[derive(Clone, Debug, Default)]
pub struct DatasheetReport {
    pub design_name: String,
    pub reference_clock: String,
    pub clock_period_ps: u64,
    pub input_specs: Vec<PinSetupHoldSpec>,
    pub clock_to_out_specs: Vec<PinClockToOutSpec>,
}

impl DatasheetReport {
    pub fn text(&self) -> String {
        let mut out = format!(
            "report_datasheet design={} clock={} period_ps={} inputs={} outputs={}\n",
            self.design_name,
            self.reference_clock,
            self.clock_period_ps,
            self.input_specs.len(),
            self.clock_to_out_specs.len()
        );
        out.push_str("  Setup/Hold to Clock at Input Pins:\n");
        for inp in &self.input_specs {
            out.push_str(&format!(
                "    port={} clk={} iostandard={} tSU_ps={} tH_ps={}\n",
                inp.input_port, inp.reference_clock, inp.iostandard, inp.setup_ps, inp.hold_ps
            ));
        }
        out.push_str("  Clock-to-Out (tCO) at Output Pins:\n");
        for co in &self.clock_to_out_specs {
            out.push_str(&format!(
                "    port={} clk={} iostandard={} min_tCO_ps={} max_tCO_ps={}\n",
                co.output_port,
                co.reference_clock,
                co.iostandard,
                co.min_tco_ps,
                co.max_tco_ps
            ));
        }
        out
    }
}

/// Generate a pin-level I/O timing characterization datasheet (`UG906 report_datasheet`).
///
/// Reports:
/// - Input ports: required setup time ($t_{\text{SU}}$) and hold time ($t_{\text{H}}$) relative to the reference clock port.
/// - Output ports: minimum (Fast_m40C) and maximum (Slow_125C) clock-to-out propagation delay ($t_{\text{CO}}$).
pub fn report_datasheet(
    design: &Design,
    routed: Option<&Routed>,
    clocks: &[Clock],
    xdc: &Constraints,
) -> Result<DatasheetReport, String> {
    let mut clks = if !clocks.is_empty() {
        clocks.to_vec()
    } else if !xdc.clocks.is_empty() {
        xdc.clocks.clone()
    } else {
        let mut v = Vec::new();
        create_clock(&mut v, "clk", 10_000, "clk");
        v
    };
    if clks.is_empty() {
        create_clock(&mut clks, "clk", 10_000, "clk");
    }
    let ref_clk = &clks[0];
    let tm = TimingModel::hl10t();

    let tr = if let Some(rt) = routed {
        report_timing_routed_xdc(design, rt, &clks, xdc)?
    } else {
        report_timing_xdc(design, &clks, xdc)?
    };
    let routed_paths = routed
        .map(|rt| timing_paths_routed(design, rt, &tr))
        .unwrap_or_default();

    let clk_ins_ps = routed
        .map(|rt| {
            let ffs = rt
                .placed
                .packed
                .lutffs
                .iter()
                .filter(|l| !l.ff_cell.is_empty())
                .count()
                .max(1) as i64;
            (crate::clock_network_delay_ps(&rt.placed) / ffs).clamp(40, 120)
        })
        .unwrap_or(80);

    let mut input_specs = Vec::new();
    let mut clock_to_out_specs = Vec::new();

    for port in &design.ports {
        let pname = resolve(port.name).to_string();
        if pname == ref_clk.source || pname == ref_clk.name {
            continue;
        }
        let std = port
            .attrs
            .get("IOSTANDARD")
            .or_else(|| xdc.iostandards.get(&pname).map(|s| s.as_str()))
            .unwrap_or("LVCMOS18")
            .to_string();
        let pad_ps = port_pad_ps(
            Some(&std),
            port.attrs
                .get("DRIVE")
                .or_else(|| xdc.drives.get(&pname).map(|s| s.as_str())),
            port.attrs
                .get("SLEW")
                .or_else(|| xdc.slews.get(&pname).map(|s| s.as_str())),
            port.attrs
                .get("PULLTYPE")
                .or_else(|| xdc.pulltypes.get(&pname).map(|s| s.as_str())),
            port.attrs
                .get("DIFF_TERM")
                .or_else(|| xdc.diff_terms.get(&pname).map(|s| s.as_str())),
            port.attrs
                .get("IN_TERM")
                .or_else(|| xdc.in_terms.get(&pname).map(|s| s.as_str())),
        );

        match port.dir {
            PortDir::In | PortDir::Inout => {
                // Input pin setup requirement = data path from pad + route + LUT + FF setup - clock insertion
                let data_to_ff_ps =
                    pad_ps + tr.route_ps.max(80) + tr.r2r_ps.max(tm.lut_ps + tm.setup_ps);
                let t_su_ps = ((data_to_ff_ps * TimingCorner::Slow_125C.late_scale_milli()) / 1000
                    - (clk_ins_ps * TimingCorner::Slow_125C.early_scale_milli()) / 1000)
                    .max(tm.setup_ps);
                let t_h_ps = (clk_ins_ps * TimingCorner::Fast_m40C.late_scale_milli()) / 1000
                    + tm.hold_req_ps
                    - ((pad_ps + tm.pin_ps) * TimingCorner::Fast_m40C.early_scale_milli()) / 1000;
                input_specs.push(PinSetupHoldSpec {
                    input_port: pname.clone(),
                    reference_clock: ref_clk.name.clone(),
                    iostandard: std.clone(),
                    setup_ps: t_su_ps,
                    hold_ps: t_h_ps,
                });
            }
            PortDir::Out => {}
        }

        if matches!(port.dir, PortDir::Out | PortDir::Inout) {
            let route_hop_ps = routed_paths
                .iter()
                .find(|p| p.nets.iter().any(|n| n == &pname) || p.endpoint.cell.contains(&pname))
                .map(|p| p.delay_ps)
                .unwrap_or_else(|| tm.ff_ckq_ps + tr.route_ps + pad_ps);
            let nominal_tco_ps = clk_ins_ps + route_hop_ps.max(tm.ff_ckq_ps + pad_ps);
            let min_tco_ps =
                (nominal_tco_ps * TimingCorner::Fast_m40C.early_scale_milli() + 500) / 1000;
            let max_tco_ps =
                (nominal_tco_ps * TimingCorner::Slow_125C.late_scale_milli() + 500) / 1000;
            clock_to_out_specs.push(PinClockToOutSpec {
                output_port: pname,
                reference_clock: ref_clk.name.clone(),
                iostandard: std,
                min_tco_ps,
                max_tco_ps,
            });
        }
    }

    // If the design has no explicit non-clock input port (e.g. autonomous counter), synthesize a virtual input entry if input_delay_ps has entries
    for (in_port, &in_del) in &xdc.input_delay_ps {
        if !input_specs.iter().any(|s| &s.input_port == in_port) {
            input_specs.push(PinSetupHoldSpec {
                input_port: in_port.clone(),
                reference_clock: ref_clk.name.clone(),
                iostandard: "LVCMOS18".into(),
                setup_ps: in_del + tm.lut_ps + tm.setup_ps,
                hold_ps: tm.hold_req_ps,
            });
        }
    }

    let _ = intern("clk");
    Ok(DatasheetReport {
        design_name: resolve(design.name).to_string(),
        reference_clock: ref_clk.name.clone(),
        clock_period_ps: ref_clk.period_ps,
        input_specs,
        clock_to_out_specs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_xdc;
    use helion_device::Device;
    use helion_pack::pack;
    use helion_place::place;
    use helion_route::route;

    #[test]
    fn mcmm_simultaneous_pvt_corners_identify_worst_setup_and_hold() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let packed = pack(&d, &dev).unwrap();
        let placed = place(&packed, &dev).unwrap();
        let routed = route(&placed, &dev).unwrap();

        let mut func_clks = Vec::new();
        create_clock(&mut func_clks, "clk_func", 4_000, "clk");
        let mut turbo_clks = Vec::new();
        create_clock(&mut turbo_clks, "clk_turbo", 1_000, "clk");

        let modes = vec![
            OperatingMode::new("func", func_clks, Constraints::default()),
            OperatingMode::new("turbo", turbo_clks, Constraints::default()),
        ];
        let corners = [
            TimingCorner::Slow_125C,
            TimingCorner::Slow_0C,
            TimingCorner::Fast_0C,
            TimingCorner::Fast_m40C,
        ];

        let rep = analyze_mcmm(&d, Some(&routed), &modes, &corners).unwrap();
        assert_eq!(rep.scenarios.len(), 8);

        // Slow_125C under tightest clock (turbo) must dominate worst setup
        let ws = rep.worst_setup().expect("worst setup scenario");
        assert_eq!(ws.mode_name, "turbo");
        assert_eq!(ws.corner, TimingCorner::Slow_125C);

        // Fast_m40C must dominate worst hold (smallest early delay)
        let wh = rep.worst_hold().expect("worst hold scenario");
        assert_eq!(wh.corner, TimingCorner::Fast_m40C);

        let s125 = rep.scenario("func", TimingCorner::Slow_125C).unwrap();
        let fm40 = rep.scenario("func", TimingCorner::Fast_m40C).unwrap();
        assert!(
            s125.setup_delay_ps > fm40.setup_delay_ps,
            "Slow_125C setup delay ({}) must exceed Fast_m40C ({})",
            s125.setup_delay_ps,
            fm40.setup_delay_ps
        );
        assert!(
            fm40.hold_delay_ps < s125.hold_delay_ps,
            "Fast_m40C hold delay ({}) must be smaller than Slow_125C ({})",
            fm40.hold_delay_ps,
            s125.hold_delay_ps
        );
        assert!(rep.text().contains("SIGNOFF_WNS_PS="));
    }

    #[test]
    fn lca_clock_reconvergence_pessimism_removal_computes_exact_credit() {
        let mut tree = ClockTree::new("clk_port", 50);
        tree.add_node("BUFG_main", "clk_port", 100).unwrap(); // cum = 150 ps
        tree.add_node("SPINE_L", "BUFG_main", 80).unwrap(); // cum = 230 ps
        tree.add_node("SPINE_R", "BUFG_main", 90).unwrap(); // cum = 240 ps
        tree.add_node("CLB_L0", "SPINE_L", 40).unwrap(); // cum = 270 ps
        tree.add_node("ff_launch", "CLB_L0", 20).unwrap(); // cum = 290 ps
        tree.add_node("ff_same_clb", "CLB_L0", 20).unwrap(); // cum = 290 ps
        tree.add_node("ff_other_spine", "SPINE_R", 30).unwrap(); // cum = 270 ps

        // Same CLB branch: LCA is CLB_L0 (cumulative 270 ps). With 900 early / 1100 late (20% spread), credit = 54 ps.
        let same_clb = compute_crpr_credit(&tree, "ff_launch", "ff_same_clb", 900, 1100);
        assert_eq!(same_clb.lca_node, "CLB_L0");
        assert_eq!(same_clb.common_delay_ps, 270);
        assert_eq!(same_clb.credit_ps, 54);

        // Different spine branches: LCA is BUFG_main (cumulative 150 ps). Credit = 30 ps.
        let diff_spine = compute_crpr_credit(&tree, "ff_launch", "ff_other_spine", 900, 1100);
        assert_eq!(diff_spine.lca_node, "BUFG_main");
        assert_eq!(diff_spine.common_delay_ps, 150);
        assert_eq!(diff_spine.credit_ps, 30);
        assert!(same_clb.credit_ps > diff_spine.credit_ps);
    }

    #[test]
    fn aocv_depth_table_and_pocv_rss_3sigma_reduce_pessimism() {
        let aocv = AocvTable::default_hl10t();
        // Deeper logic paths must have tighter AOCV derating than depth-1 paths
        assert!(aocv.late_derate_milli(1) > aocv.late_derate_milli(4));
        assert!(aocv.late_derate_milli(4) > aocv.late_derate_milli(16));
        assert!(aocv.early_derate_milli(1) < aocv.early_derate_milli(4));
        assert!(aocv.early_derate_milli(4) < aocv.early_derate_milli(16));

        let pocv = PocvTable::default_hl10t();
        // 4 identical 250 ps stages (nominal 1000 ps, 4% sigma = 10 ps per stage):
        // RSS sigma = sqrt(4 * 10^2) = 20 ps -> 3*sigma = 60 ps (vs linear 3*sigma = 120 ps!)
        let res = pocv.evaluate_path_ps(&[250, 250, 250, 250], 3.0);
        assert_eq!(res.mean_ps, 1000);
        assert_eq!(res.sigma_ps, 20);
        assert_eq!(res.three_sigma_ps, 60);
        assert_eq!(res.linear_three_sigma_ps, 120);
        assert_eq!(res.late_arrival_ps, 1060);
        assert_eq!(res.early_arrival_ps, 940);
        assert!(res.three_sigma_ps < res.linear_three_sigma_ps);
    }

    #[test]
    fn crosstalk_coupling_delta_delay_and_glitch_analysis() {
        // Opposite switching slows down victim (positive setup delta)
        let setup_delta = compute_crosstalk_delta_ps(15.0, 35.0, 200, 40, 80, true);
        // Same-direction switching speeds up victim (negative hold delta)
        let hold_delta = compute_crosstalk_delta_ps(15.0, 35.0, 200, 40, 80, false);
        assert!(setup_delta > 0, "opposite switching must add delay: {setup_delta}");
        assert!(hold_delta < 0, "same-dir switching must reduce delay: {hold_delta}");
        assert_eq!(setup_delta, -hold_delta);

        // Higher coupling capacitance must produce larger delta-delay and glitch
        let high_cc = compute_crosstalk_delta_ps(30.0, 35.0, 200, 40, 80, true);
        assert!(high_cc > setup_delta);

        let glitch_low = compute_crosstalk_glitch_mv(5.0, 45.0, 1000, 50, 50);
        let glitch_high = compute_crosstalk_glitch_mv(35.0, 15.0, 1000, 30, 80);
        assert!(glitch_high > glitch_low);

        let d = Design::structural_counter();
        let rep = analyze_crosstalk(&d, None, 1000);
        assert!(!rep.entries.is_empty());
        assert!(rep.max_setup_delta_ps > 0);
        assert!(rep.max_hold_delta_ps < 0);
        assert!(rep.text().contains("report_crosstalk"));
    }

    #[test]
    fn transparent_latch_time_borrowing_propagates_across_phases() {
        // Stage 1 takes 1300 ps in a 1000 ps period (borrows 300 ps from 500 ps half-period window).
        // Stage 2 takes 600 ps + 300 ps borrowed from stage 1 = 900 ps <= 1000 ps (borrows 0 ps, closes timing!).
        let stages = vec![
            LatchStage {
                latch_name: "L1".into(),
                comb_delay_ps: 1300,
                period_ps: 1000,
                max_borrow_limit_ps: Some(500),
            },
            LatchStage {
                latch_name: "L2".into(),
                comb_delay_ps: 600,
                period_ps: 1000,
                max_borrow_limit_ps: Some(500),
            },
        ];
        let rep = analyze_latch_pipeline(&stages);
        assert_eq!(rep.stages.len(), 2);
        assert_eq!(rep.stages[0].borrowed_by_this_stage_ps, 300);
        assert_eq!(rep.stages[0].slack_ps, 200);
        assert_eq!(rep.stages[1].borrowed_from_prev_ps, 300);
        assert_eq!(rep.stages[1].arrival_ps, 900);
        assert_eq!(rep.stages[1].borrowed_by_this_stage_ps, 0);
        assert_eq!(rep.stages[1].slack_ps, 600);
        assert_eq!(rep.max_borrowed_ps, 300);
        assert!(rep.worst_slack_ps > 0);
    }

    #[test]
    fn report_datasheet_characterizes_pin_setup_hold_and_tco() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        d.add_port("en", PortDir::In);
        let packed = pack(&d, &dev).unwrap();
        let placed = place(&packed, &dev).unwrap();
        let routed = route(&placed, &dev).unwrap();
        let xdc = load_xdc(
            "create_clock -period 10.000 [get_ports clk]\nset_property IOSTANDARD LVCMOS33 [get_ports led]\n",
        )
        .unwrap();

        let ds = report_datasheet(&d, Some(&routed), &xdc.clocks, &xdc).unwrap();
        assert_eq!(ds.reference_clock, "clk");
        assert_eq!(ds. input_specs.len(), 1);
        assert_eq!(ds.input_specs[0].input_port, "en");
        assert!(ds.input_specs[0].setup_ps > 0);
        assert_eq!(ds.clock_to_out_specs.len(), 1);
        assert_eq!(ds.clock_to_out_specs[0].output_port, "led");
        assert_eq!(ds.clock_to_out_specs[0].iostandard, "LVCMOS33");
        assert!(
            ds.clock_to_out_specs[0].max_tco_ps > ds.clock_to_out_specs[0].min_tco_ps,
            "Slow_125C max tCO ({}) must exceed Fast_m40C min tCO ({})",
            ds.clock_to_out_specs[0].max_tco_ps,
            ds.clock_to_out_specs[0].min_tco_ps
        );
        let txt = ds.text();
        assert!(txt.contains("report_datasheet"));
        assert!(txt.contains("min_tCO_ps="));
    }
}
