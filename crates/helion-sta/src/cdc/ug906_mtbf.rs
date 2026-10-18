//! Vivado UG906 `report_clock_interaction` 2D clock-domain pair classification matrix
//! (`Timed`, `Unsafe No Common Clock`, `Unsafe No Common Period`, `User Ignored Async Groups`,
//! `Partial False Path`) and multi-stage (2-FF to 5-FF `ASYNC_REG`) CDC synchronizer
//! metastability Mean Time Between Failures (`compute_cdc_mtbf_years`) across technology corners.

use helion_ir::{intern, resolve, CellKind, Design};

use crate::{Clock, ClockInteraction, ClockRelation, Constraints, TimingResult};

use super::crossings::{find_crossings, CrossingSource};
use super::domains::extract_domains;
use super::synchronizers::sync_chain;

/// Seconds per Julian year ($365.25 \times 86400$).
pub const SECONDS_PER_YEAR: f64 = 31_557_600.0;

/// Vivado UG906 Table 2-3 Clock Interaction 2D pair classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ug906ClockPairClass {
    /// Same clock or generated/harmonic clocks sharing a common primary clock root and expandable period (`Timed`).
    Timed,
    /// Clocks do not share a common primary clock root and have no covering CDC exception (`Unsafe No Common Clock`).
    UnsafeNoCommonClock,
    /// Clocks share a root or have unexpandable / non-harmonic fractional periods (`Unsafe No Common Period`).
    UnsafeNoCommonPeriod,
    /// Covered by `set_clock_groups -asynchronous` / `-exclusive` or full `set_false_path` (`User Ignored Async Groups`).
    UserIgnoredAsyncGroups,
    /// Partially covered by `set_false_path` or `set_max_delay -datapath_only` (`Partial False Path`).
    PartialFalsePath,
    /// No timing paths exist between the clock pair (`No Paths`).
    NoPaths,
}

impl Ug906ClockPairClass {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timed => "Timed",
            Self::UnsafeNoCommonClock => "Unsafe No Common Clock",
            Self::UnsafeNoCommonPeriod => "Unsafe No Common Period",
            Self::UserIgnoredAsyncGroups => "User Ignored Async Groups",
            Self::PartialFalsePath => "Partial False Path",
            Self::NoPaths => "No Paths",
        }
    }

    /// True when the clock pair is an unsafe unexcepted interaction (`Unsafe No Common Clock` or `Unsafe No Common Period`).
    #[inline]
    pub const fn is_unsafe(self) -> bool {
        matches!(
            self,
            Self::UnsafeNoCommonClock | Self::UnsafeNoCommonPeriod
        )
    }

    /// True when the clock pair is safely timed with a valid common period (`Timed`).
    #[inline]
    pub const fn is_timed(self) -> bool {
        matches!(self, Self::Timed)
    }

    /// True when covered by user async/exclusive clock groups or full false-path exception.
    #[inline]
    pub const fn is_user_ignored(self) -> bool {
        matches!(self, Self::UserIgnoredAsyncGroups)
    }
}

/// One `(launch_clock, capture_clock)` entry in the 2D [`ClockInteractionMatrix`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockInteractionEntry {
    pub from: String,
    pub to: String,
    pub class: Ug906ClockPairClass,
    pub relation: ClockRelation,
    pub common_period_ps: u64,
    pub requirement_ps: i64,
    pub wns_ps: Option<i64>,
    pub path_count: usize,
}

/// Vivado UG906 2D $N \times N$ Clock-Domain Pair Classification Matrix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClockInteractionMatrix {
    /// Ordered list of STA clocks forming the matrix rows (launch) and columns (capture).
    pub clocks: Vec<Clock>,
    /// 2D grid indexed by `grid[from_idx][to_idx]`.
    pub grid: Vec<Vec<ClockInteractionEntry>>,
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

fn lcm_u64(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        return 0;
    }
    (a / gcd_u64(a, b)).saturating_mul(b)
}

fn harmonic_periods(a: u64, b: u64) -> bool {
    if a == 0 || b == 0 {
        return false;
    }
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    hi % lo == 0
}

fn clock_root<'a>(clocks: &'a [Clock], c: &'a Clock) -> &'a str {
    let mut cur = c;
    for _ in 0..8 {
        match cur.master.as_deref() {
            Some(m) => {
                if let Some(p) = clocks.iter().find(|k| k.name == m) {
                    cur = p;
                } else {
                    return m;
                }
            }
            None => break,
        }
    }
    &cur.name
}

/// Classify a `(from, to)` clock pair according to Vivado UG906 Table 2-3.
pub fn classify_ug906_clock_pair(
    from: &Clock,
    to: &Clock,
    clocks: &[Clock],
    xdc: &Constraints,
) -> Ug906ClockPairClass {
    if from.name != to.name {
        if let Some(g) = xdc.clock_groups_for_pair(&from.name, &to.name) {
            if g.asynchronous || g.exclusive {
                return Ug906ClockPairClass::UserIgnoredAsyncGroups;
            }
        }
        if xdc.false_path_covers_clocks(&from.name, &to.name) {
            return Ug906ClockPairClass::UserIgnoredAsyncGroups;
        }
        if xdc.false_path_partial_covers_clocks(&from.name, &to.name)
            || xdc
                .datapath_max_delay_covers(&from.name, &to.name)
                .is_some()
        {
            return Ug906ClockPairClass::PartialFalsePath;
        }
    }

    if from.name == to.name {
        return Ug906ClockPairClass::Timed;
    }

    let same_root = clock_root(clocks, from) == clock_root(clocks, to);
    let lcm = lcm_u64(from.period_ps, to.period_ps);
    let min_p = from.period_ps.min(to.period_ps).max(1);
    // UG906 1000-cycle expandability limit: if LCM exceeds 1000x the faster clock period,
    // or if clocks share a root but have non-harmonic periods, report `Unsafe No Common Period`.
    let unexpandable_period = (lcm / min_p) > 1000;

    if same_root {
        if harmonic_periods(from.period_ps, to.period_ps) && !unexpandable_period {
            Ug906ClockPairClass::Timed
        } else {
            Ug906ClockPairClass::UnsafeNoCommonPeriod
        }
    } else if unexpandable_period {
        Ug906ClockPairClass::UnsafeNoCommonPeriod
    } else {
        Ug906ClockPairClass::UnsafeNoCommonClock
    }
}

impl ClockInteractionMatrix {
    /// Build the 2D $N \times N$ UG906 [`ClockInteractionMatrix`] from STA clocks, XDC exceptions,
    /// and optional timing results.
    pub fn build(
        clocks: &[Clock],
        xdc: &Constraints,
        timing: Option<&TimingResult>,
    ) -> Self {
        let base = crate::report_clock_interaction(clocks, xdc, timing);
        Self::from_report(&base, xdc)
    }

    /// Construct a 2D [`ClockInteractionMatrix`] from an existing [`ClockInteraction`] report and [`Constraints`].
    pub fn from_report(report: &ClockInteraction, xdc: &Constraints) -> Self {
        let n = report.clocks.len();
        let mut grid = Vec::with_capacity(n);
        for (i, from_clk) in report.clocks.iter().enumerate() {
            let mut row = Vec::with_capacity(n);
            for (j, to_clk) in report.clocks.iter().enumerate() {
                let cell = &report.cells[i * n + j];
                let class = if cell.relation == ClockRelation::NoPaths {
                    Ug906ClockPairClass::NoPaths
                } else {
                    classify_ug906_clock_pair(from_clk, to_clk, &report.clocks, xdc)
                };
                row.push(ClockInteractionEntry {
                    from: cell.from.clone(),
                    to: cell.to.clone(),
                    class,
                    relation: cell.relation,
                    common_period_ps: cell.common_period_ps,
                    requirement_ps: cell.requirement_ps,
                    wns_ps: cell.wns_ps,
                    path_count: cell.path_count,
                });
            }
            grid.push(row);
        }
        Self {
            clocks: report.clocks.clone(),
            grid,
        }
    }

    /// Dimension $N$ of the $N \times N$ clock interaction matrix.
    #[inline]
    pub fn dim(&self) -> usize {
        self.clocks.len()
    }

    /// Look up the matrix entry for `(from, to)` by clock name.
    pub fn get(&self, from: &str, to: &str) -> Option<&ClockInteractionEntry> {
        let r = self.clocks.iter().position(|c| c.name == from)?;
        let c = self.clocks.iter().position(|c| c.name == to)?;
        self.grid.get(r).and_then(|row| row.get(c))
    }

    /// Return the [`Ug906ClockPairClass`] for `(from, to)`.
    pub fn class_of(&self, from: &str, to: &str) -> Option<Ug906ClockPairClass> {
        self.get(from, to).map(|e| e.class)
    }

    /// Count matrix entries matching `class`.
    pub fn count_by_class(&self, class: Ug906ClockPairClass) -> usize {
        self.grid
            .iter()
            .flat_map(|r| r.iter())
            .filter(|e| e.class == class)
            .count()
    }

    /// Return all unsafe clock-pair entries (`Unsafe No Common Clock` or `Unsafe No Common Period`).
    pub fn unsafe_pairs(&self) -> Vec<&ClockInteractionEntry> {
        self.grid
            .iter()
            .flat_map(|r| r.iter())
            .filter(|e| e.class.is_unsafe())
            .collect()
    }

    /// Format the UG906 2D clock interaction matrix report as text.
    pub fn text(&self) -> String {
        if self.clocks.is_empty() {
            return "no clocks — create_clock / report_clock_interaction".into();
        }
        let mut lines = vec![format!(
            "ug906_clock_interaction_matrix clocks={} timed={} unsafe_no_common_clock={} unsafe_no_common_period={} user_ignored={} partial_false_path={}",
            self.clocks.len(),
            self.count_by_class(Ug906ClockPairClass::Timed),
            self.count_by_class(Ug906ClockPairClass::UnsafeNoCommonClock),
            self.count_by_class(Ug906ClockPairClass::UnsafeNoCommonPeriod),
            self.count_by_class(Ug906ClockPairClass::UserIgnoredAsyncGroups),
            self.count_by_class(Ug906ClockPairClass::PartialFalsePath),
        )];
        for row in &self.grid {
            for e in row {
                let wns = match e.wns_ps {
                    Some(w) => format!("WNS_PS={w}"),
                    None => "WNS_PS=n/a".into(),
                };
                lines.push(format!(
                    "FROM={} TO={} CLASS=\"{}\" COMMON_PS={} REQ_PS={} {wns} paths={}",
                    e.from,
                    e.to,
                    e.class.as_str(),
                    e.common_period_ps,
                    e.requirement_ps,
                    e.path_count
                ));
            }
        }
        lines.join("\n")
    }
}

/// Process-Voltage-Temperature (PVT) technology corner for CDC synchronizer metastability MTBF analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TechCorner {
    /// Best-case PVT corner (low temperature, high voltage): fastest regeneration ($\tau = 14.5\text{ ps}$).
    Fast,
    /// Nominal PVT corner ($25^\circ\text{C}$, nominal $V_{\text{CCINT}}$): typical regeneration ($\tau = 18.5\text{ ps}$).
    Typical,
    /// Worst-case PVT corner (high temperature, low voltage): slowest regeneration ($\tau = 24.0\text{ ps}$).
    Slow,
    /// Evaluates all corners and selects the worst-case (minimum) MTBF.
    MultiCornerWorst,
}

impl TechCorner {
    /// Metastability regeneration time constant $\tau$ in picoseconds.
    #[inline]
    pub const fn tau_ps(self) -> f64 {
        match self {
            Self::Fast => 14.5,
            Self::Typical => 18.5,
            Self::Slow | Self::MultiCornerWorst => 24.0,
        }
    }

    /// Asymptotic metastability aperture window $T_0$ in seconds.
    #[inline]
    pub const fn t0_seconds(self) -> f64 {
        match self {
            Self::Fast => 4.0e-12,
            Self::Typical => 6.5e-12,
            Self::Slow | Self::MultiCornerWorst => 9.5e-12,
        }
    }

    /// Intrinsic flip-flop setup + clock-to-Q metastability overhead ($t_{\text{setup}} + t_{\text{ckq}}$) in picoseconds.
    #[inline]
    pub const fn ff_overhead_ps(self) -> f64 {
        match self {
            Self::Fast => 180.0,
            Self::Typical => 240.0,
            Self::Slow | Self::MultiCornerWorst => 320.0,
        }
    }

    /// Inter-stage routing delay $t_{\text{route}}$ in picoseconds:
    /// `ASYNC_REG = TRUE` packs synchronizer FFs into the same CLB slice (intra-slice fast routing),
    /// whereas unpacked FFs incur general fabric routing delay.
    #[inline]
    pub const fn inter_stage_route_ps(self, async_reg: bool) -> f64 {
        match (self, async_reg) {
            (Self::Fast, true) => 55.0,
            (Self::Typical, true) => 75.0,
            (Self::Slow | Self::MultiCornerWorst, true) => 105.0,
            (Self::Fast, false) => 260.0,
            (Self::Typical, false) => 350.0,
            (Self::Slow | Self::MultiCornerWorst, false) => 480.0,
        }
    }
}

/// Compute the CDC synchronizer Mean Time Between Failures (MTBF in **years**) for an
/// $N$-stage (`stages` $\in 2..=5$) `ASYNC_REG` synchronizer chain across technology corners.
///
/// Uses the standard Kleeman-Cantoni / Xilinx UG906 exponential metastability model:
/// $$\text{MTBF}_{\text{years}} = \frac{\exp\!\left(\frac{(N - 1) \cdot t_{\text{met}}}{\tau}\right)}{f_{\text{capt}} \cdot f_{\text{data}} \cdot T_0 \cdot 31,557,600}$$
/// where:
/// - $N = \text{stages}$ ($2 \le N \le 5$; values $< 2$ have $0$ resolving stages and return $0.0$),
/// - $t_{\text{met}} = \max\!\left(0,\ T_{\text{capt, ps}} - t_{\text{ff\_overhead}} - t_{\text{route}}(\text{async\_reg})\right)$,
/// - $f_{\text{capt}} = \frac{10^{12}}{T_{\text{capt, ps}}}\text{ Hz}$,
/// - $f_{\text{data}} = 0.5 \cdot \frac{10^{12}}{T_{\text{launch, ps}}}\text{ Hz}$ (50% launch transition activity).
pub fn compute_cdc_mtbf_years(
    stages: u8,
    launch_period_ps: u64,
    capture_period_ps: u64,
    async_reg: bool,
    corner: TechCorner,
) -> f64 {
    if stages < 2 || launch_period_ps == 0 || capture_period_ps == 0 {
        return 0.0;
    }
    if corner == TechCorner::MultiCornerWorst {
        return [TechCorner::Fast, TechCorner::Typical, TechCorner::Slow]
            .into_iter()
            .map(|c| {
                compute_cdc_mtbf_years(stages, launch_period_ps, capture_period_ps, async_reg, c)
            })
            .fold(f64::INFINITY, f64::min);
    }

    let clamped_stages = stages.clamp(2, 5);
    let resolving_hops = f64::from(clamped_stages - 1);
    let t_capt_ps = capture_period_ps as f64;
    let t_launch_ps = launch_period_ps as f64;

    let stage_slack_ps = (t_capt_ps
        - corner.ff_overhead_ps()
        - corner.inter_stage_route_ps(async_reg))
    .max(0.0);
    let total_resolve_ps = resolving_hops * stage_slack_ps;

    let f_capt_hz = 1.0e12 / t_capt_ps;
    let f_data_hz = 0.5 * (1.0e12 / t_launch_ps);
    let denom = f_capt_hz * f_data_hz * corner.t0_seconds() * SECONDS_PER_YEAR;
    if denom <= 0.0 {
        return f64::INFINITY;
    }

    let exponent = (total_resolve_ps / corner.tau_ps()).min(700.0);
    exponent.exp() / denom
}

/// Per-synchronizer-chain MTBF entry in [`CdcMtbfReport`].
#[derive(Clone, Debug, PartialEq)]
pub struct CdcSyncMtbfEntry {
    pub from_clock: String,
    pub to_clock: String,
    pub stages: Vec<String>,
    pub stage_count: u8,
    pub async_reg: bool,
    pub corner: TechCorner,
    pub mtbf_years: f64,
}

/// Aggregate design-level CDC synchronizer MTBF report across all detected 2-FF to 5-FF chains.
#[derive(Clone, Debug, PartialEq)]
pub struct CdcMtbfReport {
    pub corner: TechCorner,
    pub chains: Vec<CdcSyncMtbfEntry>,
    /// Composite design MTBF in years ($1 / \sum (1 / \text{MTBF}_i)$), or `f64::INFINITY` if no chains.
    pub composite_mtbf_years: f64,
}

/// Analyze all 2-FF to 5-FF CDC synchronizer chains in `design` and compute per-chain and
/// composite design metastability MTBF in years at `corner`.
pub fn report_cdc_mtbf(
    design: &Design,
    clocks: &[Clock],
    xdc: &Constraints,
    corner: TechCorner,
) -> CdcMtbfReport {
    let active_clocks = if clocks.is_empty() {
        xdc.clocks.as_slice()
    } else {
        clocks
    };
    let domains = extract_domains(design, active_clocks, xdc);
    let raw_crossings = find_crossings(design, &domains);

    let mut entries: Vec<CdcSyncMtbfEntry> = Vec::new();
    let mut failure_rate_sum = 0.0f64;

    for rc in &raw_crossings {
        let Some(to_dom) = domains.domain_of_ff(&rc.load_ff) else {
            continue;
        };
        let chain = sync_chain(design, &rc.load_ff, to_dom, &domains);
        let stage_count = (chain.stages.len() as u8).max(chain.depth);
        if stage_count < 2 {
            continue;
        }
        let from_clk_name = match rc.from {
            CrossingSource::Domain(idx) => domains
                .domains
                .get(idx)
                .map(|d| d.name.as_str())
                .unwrap_or("clk_src"),
            CrossingSource::PortInput => "input_port",
        };
        let to_clk_name = domains
            .domains
            .get(rc.to)
            .map(|d| d.name.as_str())
            .unwrap_or("clk_dst");

        let launch_period_ps = active_clocks
            .iter()
            .find(|c| c.name == from_clk_name || c.source == from_clk_name)
            .map(|c| c.period_ps)
            .unwrap_or(10_000);
        let capture_period_ps = active_clocks
            .iter()
            .find(|c| c.name == to_clk_name || c.source == to_clk_name)
            .map(|c| c.period_ps)
            .unwrap_or(10_000);

        let async_reg = chain.stages.iter().all(|ff_name| {
            design
                .cell(ff_name)
                .is_some_and(|c| c.attrs.flag("ASYNC_REG"))
        });

        let mtbf_years = compute_cdc_mtbf_years(
            stage_count.clamp(2, 5),
            launch_period_ps,
            capture_period_ps,
            async_reg,
            corner,
        );
        if mtbf_years > 0.0 && mtbf_years.is_finite() {
            failure_rate_sum += 1.0 / mtbf_years;
        }
        let _ = (intern, resolve, CellKind::Hff);

        entries.push(CdcSyncMtbfEntry {
            from_clock: from_clk_name.to_string(),
            to_clock: to_clk_name.to_string(),
            stages: chain.stages,
            stage_count: stage_count.clamp(2, 5),
            async_reg,
            corner,
            mtbf_years,
        });
    }

    let composite_mtbf_years = if entries.is_empty() || failure_rate_sum <= 0.0 {
        f64::INFINITY
    } else {
        1.0 / failure_rate_sum
    };

    CdcMtbfReport {
        corner,
        chains: entries,
        composite_mtbf_years,
    }
}
