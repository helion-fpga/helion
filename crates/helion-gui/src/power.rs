//! VCD-driven on-chip power estimation for the HL10T.
//!
//! ## Model
//!
//! Dynamic power follows the standard CMOS form, summed per resource:
//!
//! ```text
//! P_dynamic = Σ  α · C · V² · f
//! ```
//!
//! * **α** - activity factor, P(0→1) per clock cycle. From VCD: `rising / cycles`.
//!   Vectorless fallback: clocks 1.0, data nets 0.125, IO 0.1, BRAM/DSP 0.2.
//!   The clock tree is scaled by the *detected clock's* α: when the VCD shows
//!   no free-running clock (gated off, or clock not dumped), the tree
//!   contributes nothing - so a zero-activity dump yields ~0 dynamic power.
//!   Vectorless mode keeps the classic always-on clock assumption.
//! * **C** - switched capacitance: cell-internal cap + routing (`base + fanout·k`).
//! * **V** - core voltage from operating conditions (default 1.0 V).
//! * **f** - clock frequency from the fastest STA clock, else the VCD clock.
//!
//! Static power is per-resource leakage scaled by `V · (1 + 0.008·(T−25))`.
//!
//! ## Capacitance table - MODELED, not silicon
//!
//! The HL10T is a fictional 28nm-class FPGA. Every capacitance and leakage
//! number below is **modeled**: scaled from published 7-series XPE proportions
//! and typical 28nm FPGA literature values, rounded to 1–2 significant
//! figures. They are suitable for relative comparison, budgeting, and
//! "what toggles most" analysis - **never for signoff**. If you have measured
//! HL10T data, replace [`CapTable::modeled_hl10t`] and [`LeakTable::modeled_hl10t`].

use crate::vcd::VcdActivity;
use helion_ir::{intern, resolve, CellKind, Design};
use std::collections::{HashMap, HashSet};

/// Switched capacitances in femtofarads. See module docs: modeled, not measured.
#[derive(Clone, Debug)]
pub struct CapTable {
    /// LUT6 internal (output stage + local stub) per output toggle.
    pub lut_internal_ff: f64,
    /// HFF internal (master/slave + clk→Q) per Q toggle.
    pub ff_internal_ff: f64,
    /// Clock-pin load per flip-flop (leaf of the clock tree).
    pub ff_clock_pin_ff: f64,
    /// Global clock spine + branch buffers (whole tree, before leaf loads).
    pub clk_spine_ff: f64,
    /// Routing capacitance base per toggling net.
    pub wire_base_ff: f64,
    /// Routing capacitance per fanout endpoint.
    pub wire_per_fanout_ff: f64,
    /// BRAM18 dynamic per active cycle (array precharge model).
    pub bram_active_ff: f64,
    /// DSP MAC27 dynamic per active cycle.
    pub dsp_active_ff: f64,
    /// Board-level load on a toggling output pin (default 10 pF).
    pub iob_output_load_ff: f64,
    /// IOB internal per toggle.
    pub iob_internal_ff: f64,
}

impl CapTable {
    /// Modeled HL10T capacitances. See module docs - not silicon data.
    pub fn modeled_hl10t() -> Self {
        Self {
            lut_internal_ff: 12.0,
            ff_internal_ff: 8.0,
            ff_clock_pin_ff: 30.0,
            clk_spine_ff: 400_000.0,
            wire_base_ff: 25.0,
            wire_per_fanout_ff: 12.0,
            bram_active_ff: 500_000.0,
            dsp_active_ff: 300_000.0,
            iob_output_load_ff: 10_000_000.0,
            iob_internal_ff: 200.0,
        }
    }
}

/// Leakage in microwatts at 1.0 V / 25 °C. Modeled, not measured.
#[derive(Clone, Debug)]
pub struct LeakTable {
    pub lut_used_uw: f64,
    pub lut_unused_uw: f64,
    pub ff_used_uw: f64,
    pub ff_unused_uw: f64,
    pub bram_used_uw: f64,
    pub bram_unused_uw: f64,
    pub dsp_used_uw: f64,
    pub dsp_unused_uw: f64,
    pub iob_used_uw: f64,
    pub iob_unused_uw: f64,
    /// PLLs, configuration, always-on aux.
    pub device_base_uw: f64,
}

impl LeakTable {
    /// Modeled HL10T leakage. See module docs - not silicon data.
    pub fn modeled_hl10t() -> Self {
        Self {
            lut_used_uw: 3.0,
            lut_unused_uw: 0.8,
            ff_used_uw: 1.5,
            ff_unused_uw: 0.4,
            bram_used_uw: 400.0,
            bram_unused_uw: 150.0,
            dsp_used_uw: 600.0,
            dsp_unused_uw: 250.0,
            iob_used_uw: 25.0,
            iob_unused_uw: 8.0,
            device_base_uw: 4000.0,
        }
    }
}

/// Default toggle rates for the vectorless fallback (clearly labeled estimate).
#[derive(Clone, Debug)]
pub struct VectorlessRates {
    pub clock: f64,
    pub data: f64,
    pub io: f64,
    pub bram_dsp: f64,
}

impl Default for VectorlessRates {
    fn default() -> Self {
        Self {
            clock: 1.0,
            data: 0.125,
            io: 0.1,
            bram_dsp: 0.2,
        }
    }
}

/// Inputs to the power analysis.
#[derive(Clone, Debug)]
pub struct PowerConfig {
    pub part: String,
    pub voltage_mv: u64,
    pub temperature_c: i64,
    /// Fastest clock period in ps (from STA clocks, else VCD clock, else timescale).
    pub clock_period_ps: u64,
    /// Device capacities for unused-tile leakage (0 = unknown, skipped).
    pub lut_cap: u64,
    pub ff_cap: u64,
    pub bram_cap: u64,
    pub dsp_cap: u64,
    pub iob_cap: u64,
    pub caps: CapTable,
    pub leaks: LeakTable,
    pub vectorless_rates: VectorlessRates,
}

impl PowerConfig {
    pub fn for_part(part: &str, voltage_mv: u64, temperature_c: i64, clock_period_ps: u64) -> Self {
        Self {
            part: part.to_string(),
            voltage_mv: if voltage_mv == 0 { 1000 } else { voltage_mv },
            temperature_c,
            clock_period_ps: clock_period_ps.max(1),
            lut_cap: 0,
            ff_cap: 0,
            bram_cap: 0,
            dsp_cap: 0,
            iob_cap: 0,
            caps: CapTable::modeled_hl10t(),
            leaks: LeakTable::modeled_hl10t(),
            vectorless_rates: VectorlessRates::default(),
        }
    }

    fn freq_hz(&self) -> f64 {
        1e12 / self.clock_period_ps as f64
    }

    fn v_dd(&self) -> f64 {
        self.voltage_mv as f64 / 1000.0
    }

    /// Dynamic power scales with V²; leakage with V·(1+0.008·ΔT).
    fn dyn_scale(&self) -> f64 {
        self.v_dd() * self.v_dd()
    }

    fn leak_scale(&self) -> f64 {
        self.v_dd() * (1.0 + 0.008 * (self.temperature_c as f64 - 25.0)).max(0.2)
    }
}

/// One resource rail of the report.
#[derive(Clone, Debug)]
pub struct ResourcePower {
    pub name: String,
    pub mw: f64,
}

/// One hierarchy block's share.
#[derive(Clone, Debug)]
pub struct HierPower {
    pub name: String,
    pub mw: f64,
    pub cells: usize,
}

/// One net's measured contribution.
#[derive(Clone, Debug)]
pub struct NetPower {
    pub name: String,
    pub toggles: u64,
    /// P(0→1) per cycle; 1.0 = every cycle (clock-like).
    pub activity: f64,
    pub mw: f64,
    pub is_clock: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerSeverity {
    Info,
    Warning,
    Critical,
}

/// Actionable, activity-annotated finding.
#[derive(Clone, Debug)]
pub struct PowerWarning {
    pub severity: PowerSeverity,
    pub message: String,
}

/// Full VCD-driven (or vectorless fallback) power report.
#[derive(Clone, Debug)]
pub struct VcdPowerReport {
    /// True when no VCD was loaded and default toggle rates were used.
    pub vectorless: bool,
    pub vcd_path: Option<String>,
    pub part: String,
    pub voltage_mv: u64,
    pub temperature_c: i64,
    pub clock_period_ps: u64,
    pub cycles: u64,
    pub total_mw: f64,
    pub static_mw: f64,
    pub dynamic_mw: f64,
    /// P(0→1) per cycle of the detected clock tree (1.0 = free-running).
    /// VCD-driven: from the detected clock's rising edges; 0.0 when the dump
    /// shows no free-running clock. Vectorless: 1.0.
    pub clock_alpha: f64,
    /// Design nets with at least one VCD signal mapped to them.
    pub nets_matched: usize,
    /// Total design nets (denominator for [`Self::coverage_pct`]).
    pub nets_total: usize,
    /// VCD signal names that matched no design net (first 25; see
    /// [`Self::unmatched_count`] for the full count).
    pub unmatched_signals: Vec<String>,
    /// Full count of unmatched VCD signals.
    pub unmatched_count: usize,
    pub by_resource: Vec<ResourcePower>,
    pub by_hierarchy: Vec<HierPower>,
    pub top_nets: Vec<NetPower>,
    pub warnings: Vec<PowerWarning>,
}

impl VcdPowerReport {
    pub fn resource(&self, name: &str) -> f64 {
        self.by_resource
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.mw)
            .unwrap_or(0.0)
    }

    /// % of design nets with VCD activity data (100 when there are no nets).
    pub fn coverage_pct(&self) -> f64 {
        if self.nets_total == 0 {
            100.0
        } else {
            100.0 * self.nets_matched as f64 / self.nets_total as f64
        }
    }
}

/// Heuristic: is this net name clock-like?
fn is_clock_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "clk"
        || n == "clock"
        || n.starts_with("clk")
        || n.ends_with("/clk")
        || n.ends_with(".clk")
        || n.ends_with("_clk")
        || n.contains("clk_")
}

enum DriverKind {
    Lut,
    Ff,
}

/// Hierarchical block of a cell: first `/`- or `.`-separated segment, else `top`.
fn hier_block(cell: &str) -> &str {
    cell.split(['/', '.'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("top")
}

/// Analyze power for `design`.
///
/// * `activity` - `Some` for VCD-driven analysis, `None` for the vectorless
///   fallback (default toggle rates; report is labeled `vectorless`).
/// * `vcd_path` - shown in the report header only.
pub fn analyze_power(
    design: &Design,
    cfg: &PowerConfig,
    activity: Option<&VcdActivity>,
    vcd_path: Option<String>,
) -> VcdPowerReport {
    let vectorless = activity.is_none();
    let v2 = cfg.dyn_scale();
    let f = cfg.freq_hz();

    // ---- resource counts ----
    let (mut n_lut, mut n_ff, mut n_bram, mut n_dsp, mut n_iob) = (0u64, 0, 0, 0, 0);
    for c in &design.cells {
        match c.kind {
            CellKind::Lut6 { .. } => n_lut += 1,
            CellKind::Hff => n_ff += 1,
            CellKind::Bram18 => n_bram += 1,
            CellKind::Mac27 | CellKind::Dsp48e1 { .. } => n_dsp += 1,
            CellKind::IobOut => n_iob += 1,
            _ => {}
        }
    }

    // ---- net → driver / fanout ----
    struct NetInfo {
        driver: Option<(String, DriverKind)>,
        fanout: usize,
        drives_iob: bool,
    }
    let mut nets: HashMap<&str, NetInfo> = HashMap::new();
    for n in &design.nets {
        let mut info = NetInfo {
            driver: None,
            fanout: 0,
            drives_iob: false,
        };
        for e in &n.endpoints {
            let is_driver = e.pin == intern("Q") || e.pin == intern("O");
            if is_driver && info.driver.is_none() {
                let kind = if e.pin == intern("Q") {
                    DriverKind::Ff
                } else {
                    DriverKind::Lut
                };
                info.driver = Some((resolve(e.cell).to_string(), kind));
            } else {
                info.fanout += 1;
            }
            if e.pin == intern("I") && design.cells.iter().any(|c| {
                c.name == e.cell && matches!(c.kind, CellKind::IobOut)
            }) {
                info.drives_iob = true;
            }
        }
        nets.insert(resolve(n.name), info);
    }

    // ---- activity lookup: VCD signal names may carry a scope prefix ----
    // Build a suffix map: "cnt[0]" matches "top.cnt[0]". Vector references
    // like "top.cnt[3:0]" are additionally indexed under the bare base
    // name ("cnt") so scalar net names still match (exact names win over
    // suffixes; otherwise first-in-file-order wins).
    // `net_signal` maps each design net to the VCD signal name feeding it;
    // VCD signals never used are reported as unmatched (coverage).
    let mut act_by_suffix: HashMap<&str, &str> = HashMap::new(); // key -> full signal name
    let mut sig_by_name: HashMap<&str, (u64, u64)> = HashMap::new(); // full name -> (toggles, rising)
    if let Some(a) = activity {
        for s in &a.signals {
            let full = s.name.as_str();
            sig_by_name.insert(full, (s.toggles, s.rising));
            // Exact names first so a full hierarchical design net name wins.
            act_by_suffix.entry(full).or_insert(full);
        }
        for s in &a.signals {
            let full = s.name.as_str();
            if let Some(short) = full.rsplit('.').next() {
                act_by_suffix.entry(short).or_insert(full);
                if let Some(base) = short.split('[').next() {
                    if base != short {
                        act_by_suffix.entry(base).or_insert(full);
                    }
                }
            }
        }
    }
    let mut net_signal: HashMap<&str, &str> = HashMap::new(); // design net -> VCD signal name
    if !vectorless {
        for net in nets.keys() {
            if let Some(full) = act_by_suffix.get(*net).copied() {
                net_signal.insert(*net, full);
            }
        }
    }
    let nets_matched = net_signal.len();
    let nets_total = nets.len();

    // ---- clock detection ----
    // cycles from VCD duration; vectorless assumes 1000 reference cycles.
    let cycles: u64 = match activity {
        Some(a) if a.duration_ps > 0 => {
            ((a.duration_ps as f64 / cfg.clock_period_ps as f64).round() as u64).max(1)
        }
        _ => 1000,
    };
    // Design-net clocks and VCD-only clocks (e.g. helion-sim's synthetic `clk`).
    let mut clock_nets: Vec<String> = Vec::new();
    let mut vcd_clock_names: Vec<String> = Vec::new();
    let mut clock_rising: Option<u64> = None;
    if !vectorless {
        // A clock toggles ~2× per cycle; require ≥1.8× and prefer clock-like names.
        // (name, toggles, rising, is_design_net)
        let mut cands: Vec<(String, u64, u64, bool)> = Vec::new();
        for (net, full) in &net_signal {
            if let Some((tog, rising)) = sig_by_name.get(*full).copied() {
                if tog as f64 >= 1.8 * cycles as f64 {
                    cands.push(((*net).to_string(), tog, rising, true));
                }
            }
        }
        if let Some(a) = activity {
            for s in &a.signals {
                if s.toggles as f64 >= 1.8 * cycles as f64
                    && !cands.iter().any(|(n, _, _, _)| s.name.ends_with(n.as_str()))
                {
                    cands.push((s.name.clone(), s.toggles, s.rising, false));
                }
            }
        }
        cands.sort_by(|a, b| b.1.cmp(&a.1));
        // Prefer clock-named candidates, else the top toggler.
        let pick = cands
            .iter()
            .find(|(n, _, _, _)| is_clock_name(n))
            .or_else(|| cands.first());
        if let Some((name, _, rising, is_net)) = pick {
            clock_rising = Some(*rising);
            if *is_net {
                clock_nets.push(name.clone());
            } else {
                vcd_clock_names.push(name.clone());
            }
        }
    } else {
        for net in nets.keys() {
            if is_clock_name(net) {
                clock_nets.push((*net).to_string());
            }
        }
    }

    // Clock-tree activity factor: the tree switches with the detected clock.
    // When the dump shows no free-running clock (gated off, or the clock was
    // not dumped), the tree contributes nothing - which is what makes a
    // zero-activity dump yield ~0 dynamic power. Vectorless keeps the classic
    // always-on assumption (α = 1.0).
    let clock_alpha = if vectorless {
        1.0
    } else {
        clock_rising
            .map(|r| (r as f64 / cycles.max(1) as f64).min(1.0))
            .unwrap_or(0.0)
    };

    // VCD signals that matched no design net (the detected VCD-only clock
    // counts as used, not unmatched).
    let mut matched_names: HashSet<&str> = net_signal.values().copied().collect();
    for name in &vcd_clock_names {
        matched_names.insert(name.as_str());
    }
    let (unmatched_signals, unmatched_count) = match activity {
        Some(a) => {
            let mut names: Vec<&str> = a
                .signals
                .iter()
                .map(|s| s.name.as_str())
                .filter(|n| !matched_names.contains(n))
                .collect();
            names.sort_unstable();
            let count = names.len();
            let shown = names
                .into_iter()
                .take(25)
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            (shown, count)
        }
        None => (Vec::new(), 0),
    };

    let rates = &cfg.vectorless_rates;
    let mut warnings: Vec<PowerWarning> = Vec::new();
    let mut top_nets: Vec<NetPower> = Vec::new();
    let mut hier_mw: HashMap<String, (f64, usize)> = HashMap::new();
    let mut dyn_logic = 0.0f64; // LUT+FF internal + routing, watts
    let mut dyn_io = 0.0f64;
    let dyn_clk; // assigned in the clock-tree block below

    // Clock tree first (independent of per-net loop).
    {
        let c_tree_ff = cfg.caps.clk_spine_ff + n_ff as f64 * cfg.caps.ff_clock_pin_ff;
        dyn_clk = clock_alpha * c_tree_ff * 1e-15 * v2 * f;
    }

    // Deterministic net order. `nets` is a HashMap with a per-instance
    // random seed, so iterating it directly reshuffles push order
    // (top_nets, warnings) and perturbs float accumulation order on every
    // call. The power view repaints on mouse movement, so the old code made
    // displayed values jump as the mouse moved. Sorted order makes the
    // report bit-identical across calls for identical inputs.
    let mut net_names: Vec<&str> = nets.keys().copied().collect();
    net_names.sort_unstable();
    for net in net_names {
        let info = &nets[net];
        let is_clock = clock_nets.iter().any(|c| c.as_str() == net);
        let (toggles, rising) = net_signal
            .get(net)
            .and_then(|full| sig_by_name.get(*full))
            .copied()
            .unwrap_or((0, 0));
        // α = P(0→1) per cycle.
        let alpha = if vectorless {
            if is_clock {
                rates.clock
            } else if info.drives_iob {
                rates.io
            } else {
                rates.data
            }
        } else {
            VcdActivity::activity_factor_from(rising, cycles)
        };
        if is_clock {
            // Accounted in the clock tree; still list in top nets.
            if toggles > 0 {
                top_nets.push(NetPower {
                    name: (*net).to_string(),
                    toggles,
                    activity: (toggles as f64 / (2.0 * cycles as f64)).min(1.0),
                    mw: 0.0,
                    is_clock: true,
                });
            }
            continue;
        }
        let c_ff = cfg.caps.wire_base_ff
            + info.fanout as f64 * cfg.caps.wire_per_fanout_ff
            + if info.drives_iob {
                cfg.caps.iob_output_load_ff + cfg.caps.iob_internal_ff
            } else {
                0.0
            };
        let p_net = alpha * c_ff * 1e-15 * v2 * f;
        // Driver cell internal.
        let (p_cell, cell_name) = match &info.driver {
            Some((cell, DriverKind::Lut)) => {
                (alpha * cfg.caps.lut_internal_ff * 1e-15 * v2 * f, Some(cell.clone()))
            }
            Some((cell, DriverKind::Ff)) => {
                (alpha * cfg.caps.ff_internal_ff * 1e-15 * v2 * f, Some(cell.clone()))
            }
            _ => (0.0, None),
        };
        let p = p_net + p_cell;
        if info.drives_iob {
            dyn_io += p;
        } else {
            dyn_logic += p;
        }
        if let Some(cell) = cell_name {
            let e = hier_mw.entry(hier_block(&cell).to_string()).or_insert((0.0, 0));
            e.0 += p;
            e.1 += 1;
        }
        if toggles > 0 {
            top_nets.push(NetPower {
                name: (*net).to_string(),
                toggles,
                activity: alpha.min(1.0),
                mw: p * 1000.0,
                is_clock: false,
            });
        }
        // Warnings: hot non-clock nets.
        if !vectorless && !is_clock && alpha >= 0.4 && cycles >= 10 {
            warnings.push(PowerWarning {
                severity: PowerSeverity::Warning,
                message: format!(
                    "net '{net}' toggles almost every cycle (α={alpha:.2}) - consider clock gating or re-encoding"
                ),
            });
        }
    }

    // BRAM / DSP: no per-pin activity in this kernel - modeled active-cycle term.
    let bram_alpha = if vectorless { rates.bram_dsp } else { 0.2 };
    let dsp_alpha = if vectorless { rates.bram_dsp } else { 0.2 };
    let dyn_bram = n_bram as f64 * bram_alpha * cfg.caps.bram_active_ff * 1e-15 * v2 * f;
    let dyn_dsp = n_dsp as f64 * dsp_alpha * cfg.caps.dsp_active_ff * 1e-15 * v2 * f;
    // Attribute BRAM/DSP dynamic to hierarchy blocks by instance name when possible.
    for c in &design.cells {
        let block = hier_block(resolve(c.name)).to_string();
        match c.kind {
            CellKind::Bram18 => {
                let e = hier_mw.entry(block).or_insert((0.0, 0));
                e.0 += bram_alpha * cfg.caps.bram_active_ff * 1e-15 * v2 * f;
                e.1 += 1;
            }
            CellKind::Mac27 | CellKind::Dsp48e1 { .. } => {
                let e = hier_mw.entry(block).or_insert((0.0, 0));
                e.0 += dsp_alpha * cfg.caps.dsp_active_ff * 1e-15 * v2 * f;
                e.1 += 1;
            }
            _ => {}
        }
    }

    // ---- static ----
    let lk = cfg.leak_scale();
    let l = &cfg.leaks;
    let mut static_w = l.device_base_uw * lk;
    static_w += n_lut as f64 * l.lut_used_uw * lk;
    static_w += n_ff as f64 * l.ff_used_uw * lk;
    static_w += n_bram as f64 * l.bram_used_uw * lk;
    static_w += n_dsp as f64 * l.dsp_used_uw * lk;
    static_w += n_iob as f64 * l.iob_used_uw * lk;
    static_w += cfg.lut_cap.saturating_sub(n_lut) as f64 * l.lut_unused_uw * lk;
    static_w += cfg.ff_cap.saturating_sub(n_ff) as f64 * l.ff_unused_uw * lk;
    static_w += cfg.bram_cap.saturating_sub(n_bram) as f64 * l.bram_unused_uw * lk;
    static_w += cfg.dsp_cap.saturating_sub(n_dsp) as f64 * l.dsp_unused_uw * lk;
    static_w += cfg.iob_cap.saturating_sub(n_iob) as f64 * l.iob_unused_uw * lk;
    static_w /= 1e6; // µW → W

    let dynamic_w = dyn_clk + dyn_logic + dyn_io + dyn_bram + dyn_dsp;
    let total_w = static_w + dynamic_w;
    let w2mw = |w: f64| w * 1000.0;

    let mut by_resource = vec![
        ResourcePower { name: "clocks".into(), mw: w2mw(dyn_clk) },
        ResourcePower { name: "logic".into(), mw: w2mw(dyn_logic) },
        ResourcePower { name: "io".into(), mw: w2mw(dyn_io) },
        ResourcePower { name: "bram".into(), mw: w2mw(dyn_bram) },
        ResourcePower { name: "dsp".into(), mw: w2mw(dyn_dsp) },
        ResourcePower { name: "static".into(), mw: w2mw(static_w) },
    ];
    // Tiebreak by name: equal mW must not reshuffle on repaint.
    by_resource.sort_by(|a, b| {
        b.mw.partial_cmp(&a.mw)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut by_hierarchy: Vec<HierPower> = hier_mw
        .into_iter()
        .map(|(name, (w, cells))| HierPower { name, mw: w2mw(w), cells })
        .collect();
    // Tiebreak by name: equal mW must not reshuffle on repaint.
    by_hierarchy.sort_by(|a, b| {
        b.mw.partial_cmp(&a.mw)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    // Tiebreak by name: equal toggle counts must not reshuffle on repaint.
    top_nets.sort_by(|a, b| b.toggles.cmp(&a.toggles).then_with(|| a.name.cmp(&b.name)));
    // VCD-only clocks (not design nets) still belong in the toggle table.
    if let Some(a) = activity {
        for name in &vcd_clock_names {
            if let Some(s) = a.signals.iter().find(|x| &x.name == name) {
                top_nets.push(NetPower {
                    name: name.clone(),
                    toggles: s.toggles,
                    activity: (s.toggles as f64 / (2.0 * cycles as f64)).min(1.0),
                    mw: 0.0,
                    is_clock: true,
                });
            }
        }
        top_nets.sort_by(|a, b| b.toggles.cmp(&a.toggles).then_with(|| a.name.cmp(&b.name)));
    }
    top_nets.truncate(20);

    // ---- warnings ----
    if vectorless {
        warnings.push(PowerWarning {
            severity: PowerSeverity::Info,
            message: "vectorless estimate: default toggle rates used - load a VCD (read_vcd) for measured switching activity".into(),
        });
    } else if let Some(a) = activity {
        if a.signals.iter().all(|s| s.toggles == 0) {
            warnings.push(PowerWarning {
                severity: PowerSeverity::Warning,
                message: "VCD shows zero toggles - check the dump window covers active simulation time".into(),
            });
        }
        if a.truncated {
            warnings.push(PowerWarning {
                severity: PowerSeverity::Warning,
                message: "VCD signal cap hit - activity is partial".into(),
            });
        }
        let pct = 100.0 * nets_matched as f64 / nets_total.max(1) as f64;
        if nets_total > 0 && pct < 50.0 {
            warnings.push(PowerWarning {
                severity: PowerSeverity::Warning,
                message: format!(
                    "VCD activity covers only {pct:.0}% of nets ({nets_matched}/{nets_total}) - \
                     check signal name mapping (hierarchical scope?)"
                ),
            });
        }
        // Cross-check: the VCD's own measured clock period vs the configured
        // one. A mismatch means `cycles` (and hence every α) is off - usually
        // the operating-conditions clock doesn't match the dump's timescale.
        let vcd_half_period = clock_nets
            .iter()
            .map(|n| n.as_str())
            .filter_map(|n| net_signal.get(n).copied())
            .chain(vcd_clock_names.iter().map(|s| s.as_str()))
            .filter_map(|sig| {
                activity.and_then(|a| {
                    a.signals
                        .iter()
                        .find(|x| x.name.as_str() == sig)
                        .and_then(|x| x.min_period_ps)
                })
            })
            .min();
        if let Some(half) = vcd_half_period {
            let vcd_period = half.saturating_mul(2).max(1);
            let cfg_p = cfg.clock_period_ps.max(1);
            if vcd_period > cfg_p * 2 || vcd_period * 2 < cfg_p {
                warnings.push(PowerWarning {
                    severity: PowerSeverity::Warning,
                    message: format!(
                        "VCD clock period ~{vcd_period} ps differs from configured {cfg_p} ps - \
                         activity factors may be off; check operating conditions"
                    ),
                });
            }
        }
    }
    if dynamic_w > 0.0 && dyn_clk / dynamic_w > 0.4 && n_ff > 0 {
        warnings.push(PowerWarning {
            severity: PowerSeverity::Warning,
            message: format!(
                "clock tree is {:.0}% of dynamic power - consider clock gating enables",
                100.0 * dyn_clk / dynamic_w
            ),
        });
    }
    // Cap warning spam: keep the most actionable.
    warnings.truncate(12);

    VcdPowerReport {
        vectorless,
        vcd_path,
        part: cfg.part.clone(),
        voltage_mv: cfg.voltage_mv,
        temperature_c: cfg.temperature_c,
        clock_period_ps: cfg.clock_period_ps,
        cycles,
        total_mw: w2mw(total_w),
        static_mw: w2mw(static_w),
        dynamic_mw: w2mw(dynamic_w),
        clock_alpha,
        nets_matched,
        nets_total,
        unmatched_signals,
        unmatched_count,
        by_resource,
        by_hierarchy,
        top_nets,
        warnings,
    }
}

impl VcdActivity {
    /// P(0→1) per cycle from a rising-edge count.
    pub fn activity_factor_from(rising: u64, cycles: u64) -> f64 {
        if cycles == 0 {
            0.0
        } else {
            rising as f64 / cycles as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcd::parse_vcd;
    use helion_ir::{CellKind, Design, PortDir};

    fn counter_design() -> Design {
        let src = r#"
module counter (input logic clk, output logic led);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin cnt <= cnt + 1; end
    assign led = cnt[3];
endmodule
"#;
        helion_sv::synth_sv(src, "counter.sv").expect("synth")
    }

    fn cfg_100mhz() -> PowerConfig {
        // 100 MHz, 1.0 V, 25 °C.
        PowerConfig::for_part("HL10T", 1000, 25, 10_000)
    }

    #[test]
    fn synthetic_power_math_is_hand_checkable() {
        // One FF driving one net, one fanout, toggling every other cycle.
        // α = 0.5, C = 25 + 1×12 = 37 fF, V = 1.0, f = 100 MHz:
        //   P_net  = 0.5 × 37e-15 × 1 × 1e8 = 1.85e-6 W = 0.00185 mW
        //   P_cell = 0.5 × 8e-15  × 1 × 1e8 = 0.4e-6 W  = 0.00040 mW
        // Clock `clk` toggles 2×/cycle → clock α = 1.0:
        //   P_clk  = 1.0 × (400000 + 30)e-15 × 1 × 1e8 = 40.003 mW
        let mut d = Design::new("t");
        d.add_port("clk", PortDir::In);
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_lut", "I0");
        // 1ns timescale, 4 cycles of 10ns: clk toggles every 5ns (2×/cycle),
        // q toggles every other cycle.
        let vcd = "$timescale 1ns $end\n$scope module t $end\n$var wire 1 ! q $end\n$var wire 1 \" clk $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n0\"\n$end\n#5\n1\"\n#10\n1!\n0\"\n#15\n1\"\n#20\n0!\n0\"\n#25\n1\"\n#30\n1!\n0\"\n#35\n1\"\n#40\n0!\n0\"\n";
        let a = parse_vcd(vcd).expect("parse");
        let r = analyze_power(&d, &cfg_100mhz(), Some(&a), None);
        assert!(!r.vectorless);
        assert_eq!(r.cycles, 4, "40ns / 10ns");
        assert_eq!(r.clock_alpha, 1.0, "clock rises every cycle");
        let logic = r.resource("logic");
        // 0.00225 mW ± 1%: net 0.00185 + FF internal 0.00040.
        assert!(
            (logic - 0.00225).abs() < 0.00003,
            "hand-computed logic power: {logic}"
        );
        // Clock tree with 1 FF at clock α = 1.0.
        let clocks = r.resource("clocks");
        assert!((clocks - 40.003).abs() < 0.01, "clock tree power: {clocks}");
        assert!(r.total_mw > r.dynamic_mw && r.dynamic_mw > 0.0);
        assert!((r.total_mw - (r.static_mw + r.dynamic_mw)).abs() < 1e-9);
    }

    /// Sanity: a dump where nothing toggles (not even the clock) must yield
    /// ~0 dynamic power, while static (leakage) stays positive.
    #[test]
    fn zero_activity_means_zero_dynamic() {
        let mut d = Design::new("t");
        d.add_port("clk", PortDir::In);
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_lut", "I0");
        let vcd = "$timescale 10ns $end\n$scope module t $end\n$var wire 1 ! q $end\n$var wire 1 \" clk $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n0\"\n$end\n#10\n#20\n";
        let a = parse_vcd(vcd).expect("parse");
        let r = analyze_power(&d, &cfg_100mhz(), Some(&a), None);
        assert!(!r.vectorless);
        assert_eq!(r.clock_alpha, 0.0, "no free-running clock in dump");
        assert_eq!(r.resource("clocks"), 0.0, "clock tree gated off");
        assert_eq!(r.resource("logic"), 0.0, "no toggles → no logic dynamic");
        assert_eq!(r.dynamic_mw, 0.0, "all quiet → ~0 dynamic");
        assert!(r.static_mw > 0.0, "leakage still positive: {}", r.static_mw);
        assert_eq!(r.total_mw, r.static_mw);
        assert!(
            r.warnings.iter().any(|w| w.message.contains("zero toggles")),
            "zero-activity warning"
        );
    }

    /// Sanity: dynamic power of a toggling net scales linearly with its
    /// toggle rate (α = 0.5 vs α = 0.125 → exactly 4× logic power).
    #[test]
    fn power_scales_linearly_with_toggle_rate() {
        let mut d = Design::new("t");
        d.add_port("clk", PortDir::In);
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_lut", "I0");
        let head = "$timescale 10ns $end\n$scope module t $end\n$var wire 1 ! q $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n$end\n";
        // 8 cycles: q toggles every cycle (α = 0.5) vs once (α = 0.125).
        let vcd_fast = format!("{head}#1\n1!\n#2\n0!\n#3\n1!\n#4\n0!\n#5\n1!\n#6\n0!\n#7\n1!\n#8\n0!\n");
        let vcd_slow = format!("{head}#1\n1!\n#8\n0!\n");
        let a_fast = parse_vcd(&vcd_fast).expect("parse fast");
        let a_slow = parse_vcd(&vcd_slow).expect("parse slow");
        let r_fast = analyze_power(&d, &cfg_100mhz(), Some(&a_fast), None);
        let r_slow = analyze_power(&d, &cfg_100mhz(), Some(&a_slow), None);
        assert_eq!(r_fast.cycles, 8);
        assert_eq!(r_slow.cycles, 8);
        let p_fast = r_fast.resource("logic");
        let p_slow = r_slow.resource("logic");
        assert!(p_fast > 0.0 && p_slow > 0.0);
        assert!(
            (p_fast / p_slow - 4.0).abs() < 1e-9,
            "linear in toggle rate: {p_fast} / {p_slow}"
        );
    }

    /// Coverage: VCD covers one of two nets, plus a stray signal that matches
    /// nothing - the report must count coverage and list the stray signal.
    #[test]
    fn coverage_lists_unmatched_vcd_signals() {
        let mut d = Design::new("t");
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_lut", "I0");
        d.connect("r", "u_ff", "D"); // undriven net, no VCD signal
        let vcd = "$timescale 10ns $end\n$scope module t $end\n$var wire 1 ! q $end\n$var wire 1 \" ghost $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n0\"\n$end\n#1\n1!\n#2\n0!\n";
        let a = parse_vcd(vcd).expect("parse");
        let r = analyze_power(&d, &cfg_100mhz(), Some(&a), None);
        assert_eq!(r.nets_total, 2);
        assert_eq!(r.nets_matched, 1, "only q has activity data");
        assert_eq!(r.coverage_pct(), 50.0);
        assert_eq!(r.unmatched_signals, vec!["t.ghost".to_string()]);
        assert_eq!(r.unmatched_count, 1);
    }

    #[test]
    fn vectorless_fallback_is_labeled_and_positive() {
        let d = counter_design();
        let r = analyze_power(&d, &cfg_100mhz(), None, None);
        assert!(r.vectorless, "no VCD → vectorless label");
        assert!(r.total_mw > 0.0);
        assert!(r
            .warnings
            .iter()
            .any(|w| w.message.contains("vectorless estimate")));
        // Sanity: all rails non-negative, totals consistent.
        assert!((r.total_mw - (r.static_mw + r.dynamic_mw)).abs() < 1e-9);
        for res in &r.by_resource {
            assert!(res.mw >= 0.0, "{}", res.name);
        }
    }

    #[test]
    fn vcd_driven_counter_report() {
        let d = counter_design();
        let vcd = helion_sim::vcd::dump_vcd(&d, 64, 10_000);
        let a = parse_vcd(&vcd).expect("parse");
        let r = analyze_power(&d, &cfg_100mhz(), Some(&a), Some("sim.vcd".into()));
        assert!(!r.vectorless);
        assert_eq!(r.vcd_path.as_deref(), Some("sim.vcd"));
        assert!(!r.top_nets.is_empty());
        // cnt[0] net toggles every cycle → hottest non-clock net.
        let hottest_data = r.top_nets.iter().filter(|n| !n.is_clock).max_by_key(|n| n.toggles).expect("data net");
        assert_eq!(hottest_data.toggles, 64, "{hottest_data:?}");
        // Clock detected: one clock-like net present in top nets.
        assert!(r.top_nets.iter().any(|n| n.is_clock), "clock detected");
        // Hot-net warning fires for the every-cycle counter bit.
        assert!(
            r.warnings.iter().any(|w| w.message.contains("almost every cycle")),
            "{:?}",
            r.warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
        );
        assert!(r.total_mw > 0.0);
    }

    #[test]
    fn voltage_scales_dynamic_quadratically() {
        let d = counter_design();
        let vcd = helion_sim::vcd::dump_vcd(&d, 32, 10_000);
        let a = parse_vcd(&vcd).expect("parse");
        let r1 = analyze_power(&d, &cfg_100mhz(), Some(&a), None);
        let mut cfg2 = cfg_100mhz();
        cfg2.voltage_mv = 900;
        let r2 = analyze_power(&d, &cfg2, Some(&a), None);
        let ratio = r2.dynamic_mw / r1.dynamic_mw;
        assert!(
            (ratio - 0.81).abs() < 0.01,
            "dynamic ∝ V²: 0.9² = 0.81, got {ratio}"
        );
    }

    #[test]
    fn empty_design_gives_device_base_only() {
        let d = Design::new("empty");
        let r = analyze_power(&d, &cfg_100mhz(), None, None);
        assert!(r.vectorless);
        // Only device-base leakage.
        assert!((r.static_mw - 4.0).abs() < 1e-9, "static: {}", r.static_mw);
        assert_eq!(r.dynamic_mw, r.resource("clocks"), "only clock tree (no FFs)");
    }

    /// Mouse-invariance: the power view repaints on every mouse movement,
    /// and each paint recomputes the report via `analyze_power`. The report
    /// must be bit-identical across recomputes for identical inputs, or the
    /// displayed values visibly jump as the mouse moves. Regression test
    /// for Saksham's report: "moving the mouse in the power window changes
    /// the displayed power values."
    #[test]
    fn power_report_stable_across_repaints() {
        let d = counter_design();
        let cfg = cfg_100mhz();
        // Build a VCD with several nets at the same toggle count (ties).
        let vcd = "$timescale 1ns $end\n$scope module counter $end\n$var wire 1 ! clk $end\n$var wire 1 \" a $end\n$var wire 1 # b $end\n$var wire 1 $ c $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n0\"\n0#\n0$\n$end\n#5\n1\"\n#10\n1!\n0\"\n1#\n#15\n1\"\n#20\n0!\n0\"\n0#\n1$\n#25\n1\"\n#30\n1!\n0\"\n#35\n1\"\n#40\n0!\n0\"\n";
        let a = parse_vcd(vcd).expect("parse");
        let r1 = analyze_power(&d, &cfg, Some(&a), None);
        let mut order_changed = false;
        let mut value_changed = false;
        for _ in 0..20 {
            let r2 = analyze_power(&d, &cfg, Some(&a), None);
            let o1: Vec<&str> = r1.top_nets.iter().map(|n| n.name.as_str()).collect();
            let o2: Vec<&str> = r2.top_nets.iter().map(|n| n.name.as_str()).collect();
            if o1 != o2 {
                order_changed = true;
            }
            if r1.total_mw.to_bits() != r2.total_mw.to_bits()
                || r1.dynamic_mw.to_bits() != r2.dynamic_mw.to_bits()
            {
                value_changed = true;
            }
            let h1: Vec<&str> = r1.by_hierarchy.iter().map(|h| h.name.as_str()).collect();
            let h2: Vec<&str> = r2.by_hierarchy.iter().map(|h| h.name.as_str()).collect();
            if h1 != h2 {
                order_changed = true;
            }
        }
        println!("order_changed={order_changed} value_changed={value_changed}");
        assert!(!order_changed, "top_nets/by_hierarchy order changed between identical calls");
        assert!(!value_changed, "power values changed between identical calls");
    }

}
