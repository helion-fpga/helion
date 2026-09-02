//! Graph STA: create_clock / create_generated_clock / set_bus_skew / group_path /
//! set_max_time_borrow / set_data_check / placed Manhattan.

use helion_ir::{CellKind, Design, PortDir};
use helion_place::Placed;
use helion_route::Routed;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Clock {
    pub name: String,
    pub period_ps: u64,
    pub source: String,
    pub generated: bool,
    pub master: Option<String>,
    pub divide_by: u32,
}

#[derive(Clone, Debug)]
pub struct TimingResult {
    pub clocks: Vec<Clock>,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub endpoints: usize,
    pub r2r_ps: i64,
    pub iob_ps: i64,
    pub setup_ps: i64,
    pub hold_ps: i64,
    pub hold_slack_ps: i64,
    pub route_ps: i64,
    /// Routed clock-network insertion (ps). Applied to WNS only with `set_propagated_clock`.
    pub clk_net_ps: i64,
}

/// UG949 Clock Interaction Report (`report_clock_interaction`) cell class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockRelation {
    /// Same clock or harmonic clocks sharing a root.
    Timed,
    /// Generated clock vs its master (same generated-clock tree).
    TimedGenerated,
    /// Unrelated clocks still timed — CDC without an exception.
    TimedUnsafe,
    /// `set_max_delay -datapath_only` covers the pair.
    TimedDatapath,
    /// `set_false_path` covers both clocks.
    FalsePath,
    /// Some but not all paths between the pair are excepted.
    PartialFalsePath,
    /// `set_clock_groups -asynchronous`.
    Asynchronous,
    /// `set_clock_groups -logically_exclusive` / `-physically_exclusive`.
    Exclusive,
    /// No timing paths between the pair.
    NoPaths,
}

impl ClockRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timed => "Timed",
            Self::TimedGenerated => "Timed (generated)",
            Self::TimedUnsafe => "Timed (unsafe)",
            Self::TimedDatapath => "Timed (datapath)",
            Self::FalsePath => "False Path",
            Self::PartialFalsePath => "Partial False Path",
            Self::Asynchronous => "Asynchronous",
            Self::Exclusive => "Exclusive",
            Self::NoPaths => "No Paths",
        }
    }

    /// Inter-clock CDC / exception (not intra-clock Timed).
    pub fn is_cdc(self) -> bool {
        matches!(
            self,
            Self::TimedUnsafe
                | Self::TimedDatapath
                | Self::FalsePath
                | Self::PartialFalsePath
                | Self::Asynchronous
                | Self::Exclusive
        )
    }
}

/// One From×To cell of the UG949 Clock Interaction matrix.
#[derive(Clone, Debug)]
pub struct ClockInteractionCell {
    pub from: String,
    pub to: String,
    pub relation: ClockRelation,
    pub common_period_ps: u64,
    pub requirement_ps: i64,
    pub wns_ps: Option<i64>,
    pub path_count: usize,
}

/// UG949 Clock Interaction Report: N×N matrix from STA clocks + XDC exceptions.
#[derive(Clone, Debug, Default)]
pub struct ClockInteraction {
    pub clocks: Vec<Clock>,
    pub cells: Vec<ClockInteractionCell>,
}

impl ClockInteraction {
    pub fn cell(&self, from: &str, to: &str) -> Option<&ClockInteractionCell> {
        self.cells.iter().find(|c| c.from == from && c.to == to)
    }

    pub fn timed_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| {
                matches!(
                    c.relation,
                    ClockRelation::Timed | ClockRelation::TimedGenerated
                )
            })
            .count()
    }

    pub fn unsafe_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| c.relation == ClockRelation::TimedUnsafe)
            .count()
    }

    pub fn cdc_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| c.from != c.to && c.relation.is_cdc())
            .count()
    }

    pub fn text(&self) -> String {
        if self.clocks.is_empty() {
            return "no clocks — create_clock / report_clock_interaction".into();
        }
        let mut lines = vec![format!(
            "report_clock_interaction clocks={} cells={} timed={} unsafe={} cdc={}",
            self.clocks.len(),
            self.cells.len(),
            self.timed_count(),
            self.unsafe_count(),
            self.cdc_count()
        )];
        for c in &self.clocks {
            lines.push(format!(
                "clock {} PERIOD_PS={} generated={} MASTER={}",
                c.name,
                c.period_ps,
                u8::from(c.generated),
                c.master.as_deref().unwrap_or("-")
            ));
        }
        for cell in &self.cells {
            let wns = match cell.wns_ps {
                Some(w) => format!("WNS_PS={w}"),
                None => "WNS_PS=n/a".into(),
            };
            lines.push(format!(
                "FROM={} TO={} {} COMMON_PS={} REQ_PS={} {wns} paths={}",
                cell.from,
                cell.to,
                cell.relation.as_str(),
                cell.common_period_ps,
                cell.requirement_ps,
                cell.path_count
            ));
        }
        lines.join("\n")
    }
}

pub fn create_clock(clocks: &mut Vec<Clock>, name: &str, period_ps: u64, source: &str) {
    clocks.push(Clock {
        name: name.into(),
        period_ps,
        source: source.into(),
        generated: false,
        master: None,
        divide_by: 1,
    });
}

pub fn create_generated_clock(
    clocks: &mut Vec<Clock>,
    name: &str,
    master: &str,
    divide_by: u32,
    source: &str,
) -> Result<(), String> {
    let m = clocks
        .iter()
        .find(|c| c.name == master)
        .ok_or_else(|| format!("unknown master clock {master}"))?;
    let period = m.period_ps.saturating_mul(divide_by.max(1) as u64);
    clocks.push(Clock {
        name: name.into(),
        period_ps: period,
        source: source.into(),
        generated: true,
        master: Some(master.into()),
        divide_by: divide_by.max(1),
    });
    Ok(())
}

const LUT_PS: i64 = 150;
const FF_CKQ_PS: i64 = 80;
const SETUP_PS: i64 = 50;
const PIN_PS: i64 = 20;
/// HAD default I/O pad (LVCMOS18). Gold WNS for counter uses this.
const IOB_PS: i64 = 100;
const HOP_PS: i64 = 40;
const HOLD_REQ_PS: i64 = 20;

/// I/O pad delay for a Helion IOSTANDARD. Unset / LVCMOS18 keeps gold STA.
pub fn iostandard_pad_ps(std: Option<&str>) -> i64 {
    match std.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        None | Some("") | Some("LVCMOS18") => IOB_PS,
        Some("LVCMOS12") => 60,
        Some("LVCMOS15") => 80,
        Some("LVCMOS25") => 200,
        Some("LVCMOS33") => 280,
        Some("SSTL15") | Some("SSTL15_I") => 160,
        Some("HSTL_I") | Some("HSTL_I_18") => 140,
        _ => IOB_PS,
    }
}

/// Drive vs default 12 mA. Unset / 12 keeps gold STA.
pub fn drive_pad_delta_ps(drive: Option<&str>) -> i64 {
    match drive.and_then(helion_device::Device::parse_drive) {
        None | Some(12) => 0,
        Some(2) => 160,
        Some(4) => 80,
        Some(6) => 40,
        Some(8) => 16,
        Some(16) => -16,
        Some(24) => -32,
        _ => 0,
    }
}

/// Slew vs default SLOW. Unset / SLOW keeps gold STA.
pub fn slew_pad_delta_ps(slew: Option<&str>) -> i64 {
    match slew.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        None | Some("") | Some("SLOW") => 0,
        Some("FAST") => -40,
        _ => 0,
    }
}

/// Pull vs default NONE. Unset / NONE keeps gold STA.
pub fn pulltype_pad_delta_ps(pull: Option<&str>) -> i64 {
    match pull.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        None | Some("") | Some("NONE") => 0,
        Some("PULLUP") | Some("PULLDOWN") => 20,
        Some("KEEPER") => 24,
        _ => 0,
    }
}

/// DIFF_TERM vs default FALSE. Unset / FALSE keeps gold STA.
pub fn diff_term_pad_delta_ps(term: Option<&str>) -> i64 {
    match helion_device::Device::parse_diff_term(term.unwrap_or("")) {
        Some("TRUE") => 16,
        _ => 0,
    }
}

/// IN_TERM vs default NONE. Unset / NONE keeps gold STA.
pub fn in_term_pad_delta_ps(term: Option<&str>) -> i64 {
    match helion_device::Device::parse_in_term(term.unwrap_or("")) {
        Some("UNTUNED_SPLIT_40") => 8,
        Some("UNTUNED_SPLIT_50") => 12,
        Some("UNTUNED_SPLIT_60") => 16,
        _ => 0,
    }
}

/// Combined HAD pad delay (IOSTANDARD + DRIVE + SLEW + PULLTYPE + DIFF_TERM + IN_TERM).
pub fn port_pad_ps(
    std: Option<&str>,
    drive: Option<&str>,
    slew: Option<&str>,
    pull: Option<&str>,
    diff_term: Option<&str>,
    in_term: Option<&str>,
) -> i64 {
    iostandard_pad_ps(std)
        + drive_pad_delta_ps(drive)
        + slew_pad_delta_ps(slew)
        + pulltype_pad_delta_ps(pull)
        + diff_term_pad_delta_ps(diff_term)
        + in_term_pad_delta_ps(in_term)
}

fn iob_pad_ps(design: &Design) -> i64 {
    design
        .ports
        .iter()
        .filter(|p| p.dir != PortDir::In)
        .map(|p| {
            port_pad_ps(
                p.attrs.get("IOSTANDARD"),
                p.attrs.get("DRIVE"),
                p.attrs.get("SLEW"),
                p.attrs.get("PULLTYPE"),
                p.attrs.get("DIFF_TERM"),
                p.attrs.get("IN_TERM"),
            )
        })
        .max()
        .unwrap_or(IOB_PS)
}

fn lut_fanin(design: &Design, lut: &str) -> i64 {
    (0..6)
        .filter(|p| design.net_on(lut, &format!("I{p}")).is_some())
        .count() as i64
}

fn r2r_ps(design: &Design) -> i64 {
    let mut max_ps = 0i64;
    for c in &design.cells {
        if !matches!(c.kind, CellKind::Lut6 { .. }) {
            continue;
        }
        let pins = lut_fanin(design, &c.name);
        max_ps = max_ps.max(FF_CKQ_PS + LUT_PS + pins * PIN_PS + SETUP_PS);
    }
    max_ps.max(FF_CKQ_PS + LUT_PS + SETUP_PS)
}

/// Unit-delay STA from netlist arity (no placement).
pub fn report_timing(design: &Design, clocks: &[Clock]) -> Result<TimingResult, String> {
    if clocks.is_empty() {
        return Err("no clocks".into());
    }
    let clk = &clocks[0];
    let ffs = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let r2r = r2r_ps(design);
    let wns = clk.period_ps as i64 - r2r;
    Ok(TimingResult {
        clocks: clocks.to_vec(),
        wns_ps: wns,
        tns_ps: wns.min(0),
        endpoints: ffs.max(1),
        r2r_ps: r2r,
        iob_ps: 0,
        setup_ps: r2r,
        hold_ps: FF_CKQ_PS,
        hold_slack_ps: FF_CKQ_PS - HOLD_REQ_PS,
        route_ps: 0,
        clk_net_ps: 0,
    })
}

/// Placement-aware STA: r2r plus IOB Manhattan.
pub fn report_timing_placed(
    design: &Design,
    placed: &Placed,
    clocks: &[Clock],
) -> Result<TimingResult, String> {
    let mut r = report_timing(design, clocks)?;
    let iob_ps = placed
        .lutff_sites
        .iter()
        .zip(placed.packed.lutffs.iter())
        .filter_map(|((site, _), lf)| {
            let iob = placed.iob_sites.first()?;
            if placed.packed.iobs.iter().any(|io| io.from_net == lf.q_net) {
                Some(FF_CKQ_PS + site.y.abs_diff(iob.y) as i64 * HOP_PS + iob_pad_ps(design))
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);
    r.iob_ps = iob_ps;
    let path = r.r2r_ps.max(iob_ps);
    r.setup_ps = path;
    r.hold_ps = FF_CKQ_PS + iob_ps.min(path);
    r.route_ps = 0;
    r.clk_net_ps = clock_network_delay_ps(placed);
    r.wns_ps = clocks[0].period_ps as i64 - path;
    r.tns_ps = r.wns_ps.min(0);
    r.hold_slack_ps = r.hold_ps - HOLD_REQ_PS;
    Ok(r)
}

/// STA using PathFinder hop delay so WNS/hold/setup move with placement.
pub fn report_timing_routed(
    design: &Design,
    routed: &Routed,
    clocks: &[Clock],
) -> Result<TimingResult, String> {
    let mut r = report_timing_placed(design, &routed.placed, clocks)?;
    let route_ps = routed
        .iob_src
        .iter()
        .map(|x| x.delay_ps)
        .max()
        .unwrap_or(0);
    r.route_ps = route_ps;
    r.clk_net_ps = clock_network_delay_ps(&routed.placed);
    r.iob_ps = FF_CKQ_PS + route_ps + iob_pad_ps(design);
    r.setup_ps = r.r2r_ps.max(r.iob_ps);
    r.hold_ps = FF_CKQ_PS + route_ps;
    r.wns_ps = clocks[0].period_ps as i64 - r.setup_ps;
    r.tns_ps = r.wns_ps.min(0);
    r.hold_slack_ps = r.hold_ps - HOLD_REQ_PS;
    Ok(r)
}

/// UG903 `set_propagated_clock`: fabric hop delay from the clock IOB row to each FF CLK pin.
/// Ideal clocks keep this at 0 in the WNS math; the value is always measured from place.
pub fn clock_network_delay_ps(placed: &Placed) -> i64 {
    let clk_y = placed.iob_sites.first().map(|s| s.y).unwrap_or(0);
    let clk_x = placed
        .lutff_sites
        .first()
        .map(|(s, _)| s.x)
        .or_else(|| placed.iob_sites.first().map(|s| s.x))
        .unwrap_or(0);
    placed
        .lutff_sites
        .iter()
        .map(|(s, _)| (s.x.abs_diff(clk_x) + s.y.abs_diff(clk_y)) as i64 * HOP_PS)
        .max()
        .unwrap_or(0)
}

#[derive(Clone, Debug, Default)]
pub struct IoLocs {
    pub pins: BTreeMap<String, String>,
}

impl IoLocs {
    pub fn set_pin_loc(&mut self, port: &str, site: &str) {
        self.pins.insert(port.into(), site.into());
    }
}

/// Vivado-style SDC subset: `create_clock -period <ns> [get_ports <name>]`.
pub fn load_sdc(text: &str, clocks: &mut Vec<Clock>) -> Result<(), String> {
    let x = load_xdc(text)?;
    clocks.extend(x.clocks);
    if clocks.is_empty() {
        return Err("SDC contained no create_clock".into());
    }
    Ok(())
}


fn tcl_name(joined: &str, key: &str) -> Option<String> {
    joined.split_once(key).and_then(|(_, r)| {
        r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .find(|s| !s.is_empty())
            .map(|s| s.to_string())
    })
}

fn tcl_object_name(joined: &str) -> Option<String> {
    tcl_name(joined, "get_ports")
        .or_else(|| tcl_name(joined, "get_pins"))
        .or_else(|| tcl_name(joined, "get_cells"))
        .or_else(|| tcl_name(joined, "get_clocks"))
        .or_else(|| tcl_name(joined, "get_nets"))
}

fn combine_milli(a: i64, b: i64) -> i64 {
    if a == 1000 && b == 1000 {
        1000
    } else {
        a.saturating_mul(b) / 1000
    }
}

fn scale_delay_ps(ps: i64, milli: i64) -> i64 {
    if milli == 1000 {
        ps
    } else {
        ps.saturating_mul(milli) / 1000
    }
}

fn milli_from_ratio(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

fn tcl_from_to(toks: &[&str]) -> (String, String) {
    let joined = toks.join(" ");
    let from = joined
        .split_once("-from")
        .and_then(|(_, r)| tcl_object_name(r))
        .unwrap_or_default();
    let to = joined
        .split_once("-to")
        .and_then(|(_, r)| tcl_object_name(r))
        .unwrap_or_default();
    (from, to)
}

/// UG903 `set_multicycle_path`: setup/hold path multipliers.
#[derive(Clone, Debug)]
pub struct MulticyclePath {
    pub from: String,
    pub to: String,
    pub setup_mult: u32,
    pub hold_mult: u32,
    pub start: bool,
    pub end: bool,
}

/// UG903 `set_max_delay`: absolute setup requirement (ps), replacing the period check.
#[derive(Clone, Debug)]
pub struct MaxDelay {
    pub from: String,
    pub to: String,
    pub delay_ps: i64,
    pub datapath_only: bool,
}

/// UG903 `set_min_delay`: absolute hold requirement (ps), replacing HOLD_REQ_PS.
#[derive(Clone, Debug)]
pub struct MinDelay {
    pub from: String,
    pub to: String,
    pub delay_ps: i64,
    pub datapath_only: bool,
}

/// UG903 `set_clock_groups`: CDC / exclusive groups (false path between groups).
#[derive(Clone, Debug)]
pub struct ClockGroups {
    pub asynchronous: bool,
    pub exclusive: bool,
    pub groups: Vec<Vec<String>>,
}

/// UG903 `set_clock_uncertainty`: subtracted from setup and/or hold slack.
#[derive(Clone, Debug)]
pub struct ClockUncertainty {
    pub from: String,
    pub to: String,
    pub setup_ps: i64,
    pub hold_ps: i64,
}

/// UG903 `set_clock_latency`: late adds to setup arrival; early reduces hold slack.
#[derive(Clone, Debug)]
pub struct ClockLatency {
    pub clock: String,
    pub late_ps: i64,
    pub early_ps: i64,
    pub source: bool,
}

/// UG903 `set_disable_timing`: drop timing arcs through a cell or pin pair.
#[derive(Clone, Debug)]
pub struct DisableTiming {
    pub from: String,
    pub to: String,
    pub object: String,
}

/// UG903 `set_case_analysis`: force a constant on a pin/port (disables unused arcs).
#[derive(Clone, Debug)]
pub struct CaseAnalysis {
    pub value: String,
    pub object: String,
}

/// UG903 `set_clock_sense`: rising / falling / stop_propagation on a clock pin.
#[derive(Clone, Debug)]
pub struct ClockSense {
    pub object: String,
    /// `positive`, `negative`, or `stop`.
    pub sense: String,
}

/// UG903 `set_input_jitter`: peak-to-peak primary-clock jitter (ps). Feeds STA like uncertainty.
#[derive(Clone, Debug)]
pub struct InputJitter {
    pub clock: String,
    pub jitter_ps: i64,
}

/// UG903 `set_timing_derate`: OCV scale on early (hold) / late (setup) path delay.
/// `*_milli` is parts-per-thousand (1100 = 1.100); 0 means this edge was not set.
#[derive(Clone, Debug, Default)]
pub struct TimingDerate {
    pub early_milli: i64,
    pub late_milli: i64,
    pub cell: bool,
    pub net: bool,
    pub clock: bool,
    pub data: bool,
}

/// UG903 `set_operating_conditions`: PVT overlay on HAD delays.
/// Unset voltage/temperature keeps the gold (1.00 V, 25 °C) corner.
#[derive(Clone, Debug, Default)]
pub struct OperatingConditions {
    pub voltage_mv: i64,
    pub temperature_c: i64,
    pub voltage_set: bool,
    pub temperature_set: bool,
}

impl OperatingConditions {
    pub fn is_set(&self) -> bool {
        self.voltage_set || self.temperature_set
    }

    /// Delay scale in parts-per-thousand. 1000 = gold HAD numbers.
    /// Voltage: `1000 * 1000 / V_mv` (0.95 V → 1052). Temperature: +2 milli / °C from 25 °C.
    pub fn scale_milli(&self) -> i64 {
        let mut s = 1000i64;
        if self.voltage_set && self.voltage_mv > 0 {
            s = s.saturating_mul(1000) / self.voltage_mv;
        }
        if self.temperature_set {
            s = s.saturating_add((self.temperature_c - 25).saturating_mul(2));
        }
        s.max(1)
    }
}

/// UG903 `set_bus_skew`: max allowed skew between bits (ps). Subtracted from slack.
#[derive(Clone, Debug)]
pub struct BusSkew {
    pub from: String,
    pub to: String,
    pub skew_ps: i64,
    pub setup: bool,
    pub hold: bool,
}

/// UG903 `group_path`: named path group; `-weight` scales setup delay (1000 = 1.0).
#[derive(Clone, Debug)]
pub struct PathGroup {
    pub name: String,
    pub from: String,
    pub to: String,
    pub weight_milli: i64,
    pub critical_range_ps: i64,
}

/// UG903 `set_max_time_borrow`: latch may steal this much from the next period (ps).
/// Added to setup slack. Empty list keeps gold WNS (FF / no-borrow).
#[derive(Clone, Debug)]
pub struct MaxTimeBorrow {
    pub object: String,
    pub borrow_ps: i64,
}

/// UG903 `set_data_check`: data-to-data setup/hold between `-from` and `-to` (ps).
/// Setup subtracts from WNS; hold subtracts from hold slack.
#[derive(Clone, Debug)]
pub struct DataCheck {
    pub from: String,
    pub to: String,
    pub setup_ps: i64,
    pub hold_ps: i64,
    pub clock: String,
}

#[derive(Clone, Debug, Default)]
pub struct Constraints {
    pub clocks: Vec<Clock>,
    pub input_delay_ps: BTreeMap<String, i64>,
    pub output_delay_ps: BTreeMap<String, i64>,
    pub false_paths: Vec<String>,
    pub multicycle_paths: Vec<MulticyclePath>,
    pub max_delays: Vec<MaxDelay>,
    pub min_delays: Vec<MinDelay>,
    pub clock_groups: Vec<ClockGroups>,
    pub clock_uncertainties: Vec<ClockUncertainty>,
    pub clock_latencies: Vec<ClockLatency>,
    pub disable_timings: Vec<DisableTiming>,
    pub case_analyses: Vec<CaseAnalysis>,
    pub propagated_clocks: Vec<String>,
    pub clock_senses: Vec<ClockSense>,
    pub input_jitters: Vec<InputJitter>,
    /// UG903 `set_system_jitter` in ps (0 if unset).
    pub system_jitter_ps: i64,
    pub timing_derates: Vec<TimingDerate>,
    pub operating_conditions: OperatingConditions,
    pub bus_skews: Vec<BusSkew>,
    pub path_groups: Vec<PathGroup>,
    pub max_time_borrows: Vec<MaxTimeBorrow>,
    pub data_checks: Vec<DataCheck>,
    pub package_pins: BTreeMap<String, String>,
    pub iostandards: BTreeMap<String, String>,
    pub drives: BTreeMap<String, String>,
    pub slews: BTreeMap<String, String>,
    pub pulltypes: BTreeMap<String, String>,
    pub diff_terms: BTreeMap<String, String>,
    pub in_terms: BTreeMap<String, String>,
}

impl Constraints {
    /// Setup path multiplier (Vivado default 1). Empty list keeps gold WNS.
    pub fn setup_mult(&self) -> u32 {
        self.multicycle_paths
            .iter()
            .map(|m| m.setup_mult)
            .max()
            .unwrap_or(1)
            .max(1)
    }

    /// Hold path multiplier (Vivado default 0).
    pub fn hold_mult(&self) -> u32 {
        self.multicycle_paths
            .iter()
            .map(|m| m.hold_mult)
            .max()
            .unwrap_or(0)
    }

    /// Tightest `set_max_delay` in ps, if any.
    pub fn max_delay_ps(&self) -> Option<i64> {
        self.max_delays.iter().map(|m| m.delay_ps).min()
    }

    /// Tightest `set_min_delay` in ps (largest required min delay), if any.
    pub fn min_delay_ps(&self) -> Option<i64> {
        self.min_delays.iter().map(|m| m.delay_ps).max()
    }

    /// UG903: two-or-more groups → false path between clocks in different groups.
    pub fn clock_groups_false_path(&self) -> bool {
        self.clock_groups.iter().any(|g| g.groups.len() >= 2)
    }

    /// First `set_clock_groups` that places `a` and `b` in different groups.
    pub fn clock_groups_for_pair(&self, a: &str, b: &str) -> Option<&ClockGroups> {
        self.clock_groups.iter().find(|g| {
            let ia = g.groups.iter().position(|grp| grp.iter().any(|n| n == a));
            let ib = g.groups.iter().position(|grp| grp.iter().any(|n| n == b));
            matches!((ia, ib), (Some(i), Some(j)) if i != j)
        })
    }

    /// Inter-clock `set_false_path` whose stored tokens name both clocks.
    pub fn false_path_covers_clocks(&self, from: &str, to: &str) -> bool {
        if from == to {
            return false;
        }
        self.false_paths
            .iter()
            .any(|fp| sdc_token_eq(fp, from) && sdc_token_eq(fp, to))
    }

    /// Tightest `set_max_delay -datapath_only` covering the clock pair (ps).
    pub fn datapath_max_delay_covers(&self, from: &str, to: &str) -> Option<i64> {
        self.max_delays
            .iter()
            .filter(|m| m.datapath_only)
            .filter(|m| {
                (!m.from.is_empty() && !m.to.is_empty())
                    && ((m.from == from && m.to == to)
                        || (sdc_token_eq(&m.from, from) && sdc_token_eq(&m.to, to)))
            })
            .map(|m| m.delay_ps)
            .min()
    }

    /// Largest setup `set_clock_uncertainty` in ps (0 if none).
    pub fn uncertainty_setup_ps(&self) -> i64 {
        self.clock_uncertainties
            .iter()
            .map(|u| u.setup_ps)
            .max()
            .unwrap_or(0)
    }

    /// Largest hold `set_clock_uncertainty` in ps (0 if none).
    pub fn uncertainty_hold_ps(&self) -> i64 {
        self.clock_uncertainties
            .iter()
            .map(|u| u.hold_ps)
            .max()
            .unwrap_or(0)
    }

    /// Largest late `set_clock_latency` in ps (0 if none).
    pub fn latency_late_ps(&self) -> i64 {
        self.clock_latencies.iter().map(|l| l.late_ps).max().unwrap_or(0)
    }

    /// Largest early `set_clock_latency` in ps (0 if none).
    pub fn latency_early_ps(&self) -> i64 {
        self.clock_latencies.iter().map(|l| l.early_ps).max().unwrap_or(0)
    }

    /// UG903: disable_timing arcs or case_analysis constants drop I/O paths like false path.
    pub fn arcs_disabled(&self) -> bool {
        !self.disable_timings.is_empty() || !self.case_analyses.is_empty()
    }

    /// UG903: `set_propagated_clock` is present (ideal clocks otherwise).
    pub fn clocks_propagated(&self) -> bool {
        !self.propagated_clocks.is_empty()
    }

    /// UG903: `set_clock_sense -stop_propagation` blocks clock-network insertion.
    pub fn clock_stopped(&self) -> bool {
        self.clock_senses.iter().any(|s| s.sense == "stop")
    }

    /// Largest `set_input_jitter` in ps (0 if none).
    pub fn input_jitter_ps(&self) -> i64 {
        self.input_jitters
            .iter()
            .map(|j| j.jitter_ps)
            .max()
            .unwrap_or(0)
    }

    /// Combined input+system jitter subtracted from setup slack (like uncertainty).
    pub fn jitter_setup_ps(&self) -> i64 {
        self.input_jitter_ps() + self.system_jitter_ps
    }

    /// Combined input+system jitter subtracted from hold slack (like uncertainty).
    pub fn jitter_hold_ps(&self) -> i64 {
        self.jitter_setup_ps()
    }

    /// Last `set_timing_derate` late scale (1000 = 1.0 if unset). Empty XDC keeps gold.
    pub fn late_derate_milli(&self) -> i64 {
        self.timing_derates
            .iter()
            .rev()
            .find_map(|d| (d.late_milli != 0).then_some(d.late_milli))
            .unwrap_or(1000)
    }

    /// Last `set_timing_derate` early scale (1000 = 1.0 if unset). Empty XDC keeps gold.
    pub fn early_derate_milli(&self) -> i64 {
        self.timing_derates
            .iter()
            .rev()
            .find_map(|d| (d.early_milli != 0).then_some(d.early_milli))
            .unwrap_or(1000)
    }

    /// Largest setup `set_bus_skew` in ps (0 if none).
    pub fn bus_skew_setup_ps(&self) -> i64 {
        self.bus_skews
            .iter()
            .filter(|b| b.setup)
            .map(|b| b.skew_ps)
            .max()
            .unwrap_or(0)
    }

    /// Largest hold `set_bus_skew` in ps (0 if none).
    pub fn bus_skew_hold_ps(&self) -> i64 {
        self.bus_skews
            .iter()
            .filter(|b| b.hold)
            .map(|b| b.skew_ps)
            .max()
            .unwrap_or(0)
    }

    /// `group_path -weight` scale in parts-per-thousand (1000 = 1.0, gold WNS).
    pub fn group_path_weight_milli(&self) -> i64 {
        self.path_groups
            .iter()
            .map(|g| g.weight_milli)
            .max()
            .filter(|&w| w > 0)
            .unwrap_or(1000)
    }

    /// Largest `group_path -critical_range` in ps (0 if unset).
    pub fn group_path_critical_range_ps(&self) -> i64 {
        self.path_groups
            .iter()
            .map(|g| g.critical_range_ps)
            .max()
            .unwrap_or(0)
    }

    /// Largest UG903 `set_max_time_borrow` in ps (0 if none — gold WNS).
    pub fn time_borrow_ps(&self) -> i64 {
        self.max_time_borrows
            .iter()
            .map(|b| b.borrow_ps)
            .max()
            .unwrap_or(0)
    }

    /// Largest setup `set_data_check` in ps (0 if none).
    pub fn data_check_setup_ps(&self) -> i64 {
        self.data_checks
            .iter()
            .map(|d| d.setup_ps)
            .max()
            .unwrap_or(0)
    }

    /// Largest hold `set_data_check` in ps (0 if none).
    pub fn data_check_hold_ps(&self) -> i64 {
        self.data_checks
            .iter()
            .map(|d| d.hold_ps)
            .max()
            .unwrap_or(0)
    }

    /// Late (setup) path scale: derate × operating-conditions × group_path weight.
    /// 1000 keeps gold WNS.
    pub fn path_late_milli(&self) -> i64 {
        combine_milli(
            combine_milli(
                self.late_derate_milli(),
                self.operating_conditions.scale_milli(),
            ),
            self.group_path_weight_milli(),
        )
    }

    /// Early (hold) path scale: derate × operating-conditions. 1000 keeps gold hold.
    pub fn path_early_milli(&self) -> i64 {
        combine_milli(self.early_derate_milli(), self.operating_conditions.scale_milli())
    }

    /// Routed clock insertion applied to STA. Ideal or stopped clocks keep 0 (gold WNS).
    pub fn propagated_network_ps(&self, clk_net_ps: i64) -> i64 {
        if self.clocks_propagated() && !self.clock_stopped() {
            clk_net_ps
        } else {
            0
        }
    }

    /// UG903: `-negative` uses the falling edge (half-cycle setup).
    pub fn clock_sense_setup_ps(&self, period_ps: u64) -> i64 {
        if self.clock_senses.iter().any(|s| s.sense == "negative") {
            (period_ps as i64) / 2
        } else {
            0
        }
    }
}

impl Constraints {
    pub fn apply(&self, design: &mut Design) -> Result<(), String> {
        for (port, site) in &self.package_pins {
            design.set_loc(port, site)?;
        }
        for (port, std) in &self.iostandards {
            design.set_iostandard(port, std)?;
        }
        for (port, ma) in &self.drives {
            design.set_drive(port, ma)?;
        }
        for (port, slew) in &self.slews {
            design.set_slew(port, slew)?;
        }
        for (port, pull) in &self.pulltypes {
            design.set_pulltype(port, pull)?;
        }
        for (port, term) in &self.diff_terms {
            design.set_diff_term(port, term)?;
        }
        for (port, term) in &self.in_terms {
            design.set_in_term(port, term)?;
        }
        Ok(())
    }
}

fn tcl_case_value(s: &str) -> Option<String> {
    match s
        .trim()
        .trim_matches(|c: char| c == '{' || c == '}')
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "zero" => Some("0".into()),
        "1" | "one" => Some("1".into()),
        "rising" => Some("rising".into()),
        "falling" => Some("falling".into()),
        _ => None,
    }
}

fn sdc_token_eq(hay: &str, name: &str) -> bool {
    hay.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|t| t == name)
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

fn lcm_u64(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        return 0;
    }
    a / gcd_u64(a, b) * b
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

fn classify_clock_pair(from: &Clock, to: &Clock, clocks: &[Clock], xdc: &Constraints) -> ClockRelation {
    if from.name != to.name {
        if let Some(g) = xdc.clock_groups_for_pair(&from.name, &to.name) {
            if g.exclusive {
                return ClockRelation::Exclusive;
            }
            if g.asynchronous {
                return ClockRelation::Asynchronous;
            }
        }
        if xdc.false_path_covers_clocks(&from.name, &to.name) {
            return ClockRelation::FalsePath;
        }
        if xdc.datapath_max_delay_covers(&from.name, &to.name).is_some() {
            return ClockRelation::TimedDatapath;
        }
    }
    if from.name == to.name {
        return ClockRelation::Timed;
    }
    let same_root = clock_root(clocks, from) == clock_root(clocks, to);
    if same_root && (from.generated || to.generated) {
        return ClockRelation::TimedGenerated;
    }
    if same_root && harmonic_periods(from.period_ps, to.period_ps) {
        return ClockRelation::Timed;
    }
    ClockRelation::TimedUnsafe
}

fn analysis_requirement_ps(clocks: &[Clock], xdc: &Constraints) -> i64 {
    xdc.max_delay_ps().unwrap_or_else(|| {
        (clocks.first().map(|c| c.period_ps).unwrap_or(0) as i64)
            .saturating_mul(xdc.setup_mult() as i64)
    })
}

/// UG949 `report_clock_interaction`: From×To matrix from STA clocks and XDC CDC exceptions.
/// Intra-clock WNS is the STA slack for that clock's period; async/false/exclusive cells
/// have no WNS. Empty clocks yield an empty report (not a canned matrix).
pub fn report_clock_interaction(
    clocks: &[Clock],
    xdc: &Constraints,
    timing: Option<&TimingResult>,
) -> ClockInteraction {
    let clocks = if clocks.is_empty() {
        xdc.clocks.as_slice()
    } else {
        clocks
    };
    if clocks.is_empty() {
        return ClockInteraction::default();
    }
    let analysis_clocks = timing
        .map(|t| t.clocks.as_slice())
        .filter(|c| !c.is_empty())
        .unwrap_or(clocks);
    let analysis_req = analysis_requirement_ps(analysis_clocks, xdc);
    let mut cells = Vec::with_capacity(clocks.len() * clocks.len());
    for from in clocks {
        for to in clocks {
            let relation = classify_clock_pair(from, to, clocks, xdc);
            let common_period_ps = lcm_u64(from.period_ps, to.period_ps);
            let requirement_ps = xdc
                .datapath_max_delay_covers(&from.name, &to.name)
                .unwrap_or(to.period_ps as i64);
            let excepted = matches!(
                relation,
                ClockRelation::FalsePath
                    | ClockRelation::Asynchronous
                    | ClockRelation::Exclusive
                    | ClockRelation::NoPaths
            );
            let (wns_ps, path_count) = if excepted {
                (None, 0)
            } else if let Some(t) = timing {
                let delay = analysis_req - t.wns_ps;
                (
                    Some(requirement_ps - delay),
                    if from.name == to.name {
                        t.endpoints
                    } else {
                        1
                    },
                )
            } else {
                (None, usize::from(from.name != to.name))
            };
            cells.push(ClockInteractionCell {
                from: from.name.clone(),
                to: to.name.clone(),
                relation,
                common_period_ps,
                requirement_ps,
                wns_ps,
                path_count,
            });
        }
    }
    ClockInteraction {
        clocks: clocks.to_vec(),
        cells,
    }
}

fn tcl_clock_group_names(joined: &str) -> Vec<String> {
    let src = if let Some((_, r)) = joined.split_once("get_clocks") {
        r
    } else {
        joined
    };
    src.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty() && *s != "get_clocks")
        .map(|s| s.to_string())
        .collect()
}

/// XDC/SDC: create_clock, create_generated_clock, set_input/output_delay,
/// set_false_path, set_multicycle_path, set_max_delay, set_min_delay,
/// set_clock_groups, set_clock_uncertainty, set_clock_latency,
/// set_disable_timing, set_case_analysis, set_propagated_clock, set_clock_sense,
/// set_input_jitter, set_system_jitter, set_timing_derate, set_operating_conditions,
/// set_bus_skew, group_path, set_max_time_borrow, set_data_check,
/// set_property PACKAGE_PIN / IOSTANDARD / DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM.
pub fn load_xdc(text: &str) -> Result<Constraints, String> {
    let mut c = Constraints::default();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        match toks[0] {
            "create_clock" => {
                let mut period_ns: Option<f64> = None;
                let mut name = String::new();
                let mut source = "clk".to_string();
                let mut i = 1;
                while i < toks.len() {
                    if toks[i] == "-period" {
                        period_ns = toks.get(i + 1).and_then(|s| s.parse().ok());
                        i += 2;
                        continue;
                    }
                    if toks[i] == "-name" {
                        name = toks.get(i + 1).unwrap_or(&"clk").to_string();
                        i += 2;
                        continue;
                    }
                    let joined = toks[i..].join(" ");
                    if let Some(n) = tcl_name(&joined, "get_ports") {
                        source = n;
                    }
                    i += 1;
                }
                let ns = period_ns.ok_or_else(|| format!("create_clock missing -period: {line}"))?;
                let ps = (ns * 1000.0).round() as u64;
                if name.is_empty() {
                    name = source.clone();
                }
                create_clock(&mut c.clocks, &name, ps.max(1), &source);
            }
            "create_generated_clock" => {
                let mut name = "genclk".to_string();
                let mut master = c.clocks.first().map(|k| k.name.clone()).unwrap_or_else(|| "clk".into());
                let mut divide_by = 2u32;
                let mut source = String::new();
                let mut i = 1;
                while i < toks.len() {
                    if toks[i] == "-name" {
                        name = toks.get(i + 1).unwrap_or(&"genclk").to_string();
                        i += 2;
                        continue;
                    }
                    if toks[i] == "-divide_by" {
                        divide_by = toks.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(2);
                        i += 2;
                        continue;
                    }
                    if toks[i] == "-source" {
                        let joined = toks[i..].join(" ");
                        if let Some(n) = tcl_name(&joined, "get_ports").or_else(|| tcl_name(&joined, "get_pins")) {
                            master = n;
                        }
                        i += 1;
                        continue;
                    }
                    let joined = toks[i..].join(" ");
                    if let Some(n) = tcl_name(&joined, "get_pins").or_else(|| tcl_name(&joined, "get_ports")) {
                        source = n;
                    }
                    i += 1;
                }
                if source.is_empty() {
                    source = name.clone();
                }
                if c.clocks.iter().all(|k| k.name != master) {
                    // fall back to first clock name for -source [get_ports clk]
                    if let Some(k) = c.clocks.first() {
                        master = k.name.clone();
                    }
                }
                create_generated_clock(&mut c.clocks, &name, &master, divide_by, &source)?;
            }
            "set_input_delay" | "set_output_delay" => {
                let is_out = toks[0] == "set_output_delay";
                let mut delay_ns: Option<f64> = None;
                let mut port = String::new();
                let mut i = 1;
                while i < toks.len() {
                    if toks[i] == "-clock" {
                        i += 2;
                        continue;
                    }
                    if delay_ns.is_none() {
                        if let Ok(v) = toks[i].parse::<f64>() {
                            delay_ns = Some(v);
                            i += 1;
                            continue;
                        }
                    }
                    let joined = toks[i..].join(" ");
                    if let Some(n) = tcl_name(&joined, "get_ports") {
                        port = n;
                    }
                    i += 1;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing delay"))?;
                let ps = (ns * 1000.0).round() as i64;
                if port.is_empty() {
                    return Err(format!("{line}: missing port"));
                }
                if is_out {
                    c.output_delay_ps.insert(port, ps);
                } else {
                    c.input_delay_ps.insert(port, ps);
                }
            }
            "set_false_path" => {
                let joined = toks.join(" ");
                if let Some(n) = tcl_name(&joined, "get_ports")
                    .or_else(|| tcl_name(&joined, "get_pins"))
                    .or_else(|| tcl_name(&joined, "get_cells"))
                {
                    c.false_paths.push(n);
                } else {
                    c.false_paths.push(joined);
                }
            }
            "set_multicycle_path" => {
                let mut want_setup = false;
                let mut want_hold = false;
                let mut setup_n: Option<u32> = None;
                let mut hold_n: Option<u32> = None;
                let mut bare: Option<u32> = None;
                let mut start = false;
                let mut end = false;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-setup" => {
                            want_setup = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                setup_n = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-hold" => {
                            want_hold = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                hold_n = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-start" => {
                            start = true;
                            i += 1;
                        }
                        "-end" => {
                            end = true;
                            i += 1;
                        }
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if bare.is_none() {
                                if let Ok(v) = other.parse::<u32>() {
                                    bare = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                if setup_n.is_none() && hold_n.is_none() && bare.is_none() {
                    return Err(format!("{line}: missing path multiplier"));
                }
                let (from, to) = tcl_from_to(&toks);
                let (setup_mult, hold_mult) = if want_hold && !want_setup {
                    (1, hold_n.or(bare).unwrap_or(0))
                } else if want_setup && want_hold {
                    (setup_n.or(bare).unwrap_or(1), hold_n.unwrap_or(0))
                } else if want_setup {
                    (setup_n.or(bare).unwrap_or(1), 0)
                } else {
                    (bare.unwrap_or(1), 0)
                };
                c.multicycle_paths.push(MulticyclePath {
                    from,
                    to,
                    setup_mult: setup_mult.max(1),
                    hold_mult,
                    start,
                    end,
                });
            }
            "set_max_delay" => {
                let mut delay_ns: Option<f64> = None;
                let mut datapath_only = false;
                let mut i = 1;
                while i < toks.len() {
                    let t = toks[i];
                    if t == "-datapath_only" {
                        datapath_only = true;
                        i += 1;
                        continue;
                    }
                    if t.starts_with('-') {
                        i += 1;
                        continue;
                    }
                    if delay_ns.is_none() {
                        if let Ok(v) = t.parse::<f64>() {
                            delay_ns = Some(v);
                        }
                    }
                    i += 1;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing delay"))?;
                let (from, to) = tcl_from_to(&toks);
                c.max_delays.push(MaxDelay {
                    from,
                    to,
                    delay_ps: (ns * 1000.0).round() as i64,
                    datapath_only,
                });
            }
            "set_min_delay" => {
                let mut delay_ns: Option<f64> = None;
                let mut datapath_only = false;
                let mut i = 1;
                while i < toks.len() {
                    let t = toks[i];
                    if t == "-datapath_only" {
                        datapath_only = true;
                        i += 1;
                        continue;
                    }
                    if t.starts_with('-') {
                        i += 1;
                        continue;
                    }
                    if delay_ns.is_none() {
                        if let Ok(v) = t.parse::<f64>() {
                            delay_ns = Some(v);
                        }
                    }
                    i += 1;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing delay"))?;
                let (from, to) = tcl_from_to(&toks);
                c.min_delays.push(MinDelay {
                    from,
                    to,
                    delay_ps: (ns * 1000.0).round() as i64,
                    datapath_only,
                });
            }
            "set_clock_groups" => {
                let mut asynchronous = false;
                let mut exclusive = false;
                let mut groups: Vec<Vec<String>> = Vec::new();
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-asynchronous" => {
                            asynchronous = true;
                            i += 1;
                        }
                        "-logically_exclusive" | "-physically_exclusive" | "-exclusive" => {
                            exclusive = true;
                            i += 1;
                        }
                        "-group" => {
                            i += 1;
                            let mut chunk = Vec::new();
                            while i < toks.len() && !toks[i].starts_with('-') {
                                chunk.push(toks[i]);
                                i += 1;
                            }
                            let names = tcl_clock_group_names(&chunk.join(" "));
                            if !names.is_empty() {
                                groups.push(names);
                            }
                        }
                        _ => i += 1,
                    }
                }
                if groups.len() < 2 {
                    return Err(format!("{line}: set_clock_groups needs two -group lists"));
                }
                if !asynchronous && !exclusive {
                    asynchronous = true;
                }
                c.clock_groups.push(ClockGroups {
                    asynchronous,
                    exclusive,
                    groups,
                });
            }
            "set_clock_uncertainty" => {
                let mut want_setup = false;
                let mut want_hold = false;
                let mut delay_ns: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-setup" => {
                            want_setup = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                delay_ns = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-hold" => {
                            want_hold = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                delay_ns = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing uncertainty"))?;
                let ps = (ns * 1000.0).round() as i64;
                let (from, mut to) = tcl_from_to(&toks);
                if from.is_empty() && to.is_empty() {
                    if let Some(n) = tcl_object_name(&toks.join(" ")) {
                        to = n;
                    }
                }
                let (setup_ps, hold_ps) = if want_hold && !want_setup {
                    (0, ps)
                } else if want_setup && !want_hold {
                    (ps, 0)
                } else {
                    (ps, ps)
                };
                c.clock_uncertainties.push(ClockUncertainty {
                    from,
                    to,
                    setup_ps,
                    hold_ps,
                });
            }
            "set_clock_latency" => {
                let mut source = false;
                let mut want_early = false;
                let mut want_late = false;
                let mut delay_ns: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-source" => {
                            source = true;
                            i += 1;
                        }
                        "-early" => {
                            want_early = true;
                            i += 1;
                        }
                        "-late" => {
                            want_late = true;
                            i += 1;
                        }
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing latency"))?;
                let ps = (ns * 1000.0).round() as i64;
                let joined = toks.join(" ");
                let clock = tcl_object_name(&joined).unwrap_or_default();
                let (late_ps, early_ps) = if want_late && !want_early {
                    (ps, 0)
                } else if want_early && !want_late {
                    (0, ps)
                } else {
                    (ps, ps)
                };
                c.clock_latencies.push(ClockLatency {
                    clock,
                    late_ps,
                    early_ps,
                    source,
                });
            }
            "set_disable_timing" => {
                let (from, to) = tcl_from_to(&toks);
                let object = tcl_object_name(&toks.join(" ")).unwrap_or_default();
                if from.is_empty() && to.is_empty() && object.is_empty() {
                    return Err(format!("{line}: set_disable_timing needs -from/-to or a cell/pin"));
                }
                c.disable_timings.push(DisableTiming { from, to, object });
            }
            "set_case_analysis" => {
                let mut value = String::new();
                let mut i = 1;
                while i < toks.len() {
                    let t = toks[i];
                    if t == "-quiet" || t == "-verbose" {
                        i += 1;
                        continue;
                    }
                    if value.is_empty() {
                        if let Some(v) = tcl_case_value(t) {
                            value = v;
                            i += 1;
                            continue;
                        }
                    }
                    i += 1;
                }
                let object = tcl_object_name(&toks.join(" ")).unwrap_or_default();
                if value.is_empty() {
                    return Err(format!(
                        "{line}: set_case_analysis missing 0/1/rising/falling"
                    ));
                }
                if object.is_empty() {
                    return Err(format!("{line}: set_case_analysis missing pin/port"));
                }
                c.case_analyses.push(CaseAnalysis { value, object });
            }
            "set_propagated_clock" => {
                let joined = toks.join(" ");
                let name = tcl_object_name(&joined)
                    .or_else(|| {
                        if joined.contains("all_clocks") {
                            Some("*".into())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                if name.is_empty() {
                    return Err(format!(
                        "{line}: set_propagated_clock needs a clock/port/pin"
                    ));
                }
                if !c.propagated_clocks.contains(&name) {
                    c.propagated_clocks.push(name);
                }
            }
            "set_clock_sense" => {
                let mut sense = String::new();
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-positive" | "-pos" => {
                            sense = "positive".into();
                            i += 1;
                        }
                        "-negative" | "-neg" => {
                            sense = "negative".into();
                            i += 1;
                        }
                        "-stop_propagation" | "-stop" => {
                            sense = "stop".into();
                            i += 1;
                        }
                        "-pulse" => i += 2,
                        _ => i += 1,
                    }
                }
                let object = tcl_object_name(&toks.join(" ")).unwrap_or_default();
                if sense.is_empty() {
                    return Err(format!(
                        "{line}: set_clock_sense needs -positive/-negative/-stop_propagation"
                    ));
                }
                if object.is_empty() {
                    return Err(format!("{line}: set_clock_sense missing pin/port"));
                }
                c.clock_senses.push(ClockSense { object, sense });
            }
            "set_input_jitter" => {
                let mut delay_ns: Option<f64> = None;
                let mut clock = String::new();
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-rise" | "-fall" | "-quiet" | "-verbose" => i += 1,
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if let Ok(v) = other.parse::<f64>() {
                                if delay_ns.is_none() {
                                    delay_ns = Some(v);
                                }
                            } else if clock.is_empty() {
                                let cleaned = other
                                    .trim_matches(|c: char| {
                                        !c.is_ascii_alphanumeric() && c != '_'
                                    })
                                    .to_string();
                                if !cleaned.is_empty() && cleaned != "get_clocks" {
                                    clock = cleaned;
                                }
                            }
                            i += 1;
                        }
                    }
                }
                if clock.is_empty() {
                    if let Some(n) = tcl_object_name(&toks.join(" ")) {
                        clock = n;
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing input jitter"))?;
                let ps = (ns * 1000.0).round() as i64;
                c.input_jitters.push(InputJitter {
                    clock,
                    jitter_ps: ps,
                });
            }
            "set_system_jitter" => {
                let mut delay_ns: Option<f64> = None;
                for t in toks.iter().skip(1) {
                    if t.starts_with('-') {
                        continue;
                    }
                    if let Ok(v) = t.parse::<f64>() {
                        delay_ns = Some(v);
                        break;
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing system jitter"))?;
                c.system_jitter_ps = (ns * 1000.0).round() as i64;
            }
            "set_timing_derate" => {
                let mut want_early = false;
                let mut want_late = false;
                let mut cell = false;
                let mut net = false;
                let mut clock = false;
                let mut data = false;
                let mut early_val: Option<f64> = None;
                let mut late_val: Option<f64> = None;
                let mut generic_val: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-early" | "-min" => {
                            want_early = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                early_val = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-late" | "-max" => {
                            want_late = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                late_val = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-cell_delay" | "-cell" => {
                            cell = true;
                            i += 1;
                        }
                        "-net_delay" | "-net" => {
                            net = true;
                            i += 1;
                        }
                        "-clock" => {
                            clock = true;
                            i += 1;
                        }
                        "-data" => {
                            data = true;
                            i += 1;
                        }
                        "-rise" | "-fall" | "-quiet" | "-verbose" => i += 1,
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if let Ok(v) = other.parse::<f64>() {
                                generic_val = Some(v);
                            }
                            i += 1;
                        }
                    }
                }
                let generic_m = generic_val.map(milli_from_ratio);
                let late_milli = if want_late || (!want_early && !want_late) {
                    late_val.map(milli_from_ratio).or(generic_m).unwrap_or(0)
                } else {
                    0
                };
                let early_milli = if want_early || (!want_early && !want_late) {
                    early_val.map(milli_from_ratio).or(generic_m).unwrap_or(0)
                } else {
                    0
                };
                if late_milli == 0 && early_milli == 0 {
                    return Err(format!("{line}: missing derate"));
                }
                c.timing_derates.push(TimingDerate {
                    early_milli,
                    late_milli,
                    cell,
                    net,
                    clock,
                    data,
                });
            }
            "set_bus_skew" => {
                let mut want_setup = false;
                let mut want_hold = false;
                let mut delay_ns: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-setup" => {
                            want_setup = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                delay_ns = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-hold" => {
                            want_hold = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                delay_ns = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing bus skew"))?;
                let (from, to) = tcl_from_to(&toks);
                let (setup, hold) = if want_hold && !want_setup {
                    (false, true)
                } else if want_setup && !want_hold {
                    (true, false)
                } else {
                    (true, true)
                };
                c.bus_skews.push(BusSkew {
                    from,
                    to,
                    skew_ps: (ns * 1000.0).round() as i64,
                    setup,
                    hold,
                });
            }
            "group_path" => {
                let mut name = String::new();
                let mut weight: Option<f64> = None;
                let mut critical_ns: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-name" => {
                            name = toks
                                .get(i + 1)
                                .unwrap_or(&"default")
                                .trim_matches(|ch: char| {
                                    ch == '{' || ch == '}' || ch == '"'
                                })
                                .to_string();
                            i += 2;
                        }
                        "-weight" => {
                            weight = toks.get(i + 1).and_then(|s| s.parse().ok());
                            i += 2;
                        }
                        "-critical_range" => {
                            critical_ns = toks.get(i + 1).and_then(|s| s.parse().ok());
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                let (from, to) = tcl_from_to(&toks);
                if name.is_empty() && from.is_empty() && to.is_empty() && weight.is_none() {
                    return Err(format!(
                        "{line}: group_path needs -name, -from/-to, or -weight"
                    ));
                }
                if name.is_empty() {
                    name = "default".into();
                }
                let weight_milli = match weight {
                    Some(v) if v > 0.0 => milli_from_ratio(v).max(1),
                    Some(_) => {
                        return Err(format!("{line}: group_path -weight must be > 0"));
                    }
                    None => 1000,
                };
                c.path_groups.push(PathGroup {
                    name,
                    from,
                    to,
                    weight_milli,
                    critical_range_ps: critical_ns
                        .map(|ns| (ns * 1000.0).round() as i64)
                        .unwrap_or(0),
                });
            }
            "set_max_time_borrow" => {
                let mut delay_ns: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing time borrow"))?;
                let object = tcl_object_name(&toks.join(" ")).unwrap_or_default();
                c.max_time_borrows.push(MaxTimeBorrow {
                    object,
                    borrow_ps: (ns * 1000.0).round() as i64,
                });
            }
            "set_data_check" => {
                let mut want_setup = false;
                let mut want_hold = false;
                let mut delay_ns: Option<f64> = None;
                let mut clock = String::new();
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-setup" => {
                            want_setup = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                delay_ns = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-hold" => {
                            want_hold = true;
                            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse().ok()) {
                                delay_ns = Some(v);
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        "-clock" => {
                            let joined = toks[i..].join(" ");
                            if let Some(n) = tcl_object_name(&joined) {
                                clock = n;
                            } else if let Some(n) = toks.get(i + 1) {
                                clock = n
                                    .trim_matches(|c: char| {
                                        !c.is_ascii_alphanumeric() && c != '_'
                                    })
                                    .to_string();
                            }
                            i += 2;
                        }
                        s if s.starts_with('-') => i += 1,
                        other => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                }
                            }
                            i += 1;
                        }
                    }
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing data check"))?;
                let (from, to) = tcl_from_to(&toks);
                if from.is_empty() && to.is_empty() {
                    return Err(format!("{line}: set_data_check needs -from/-to"));
                }
                let ps = (ns * 1000.0).round() as i64;
                let (setup_ps, hold_ps) = if want_hold && !want_setup {
                    (0, ps)
                } else if want_setup && !want_hold {
                    (ps, 0)
                } else {
                    (ps, ps)
                };
                c.data_checks.push(DataCheck {
                    from,
                    to,
                    setup_ps,
                    hold_ps,
                    clock,
                });
            }
            "set_operating_conditions" => {
                let mut voltage_v: Option<f64> = None;
                let mut temp: Option<i64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-voltage" | "-volt" => {
                            voltage_v = toks.get(i + 1).and_then(|s| s.parse().ok());
                            i += 2;
                        }
                        "-temperature" | "-temp" => {
                            temp = toks
                                .get(i + 1)
                                .and_then(|s| s.parse::<f64>().ok())
                                .map(|v| v.round() as i64);
                            i += 2;
                        }
                        "-process" | "-grade" | "-library" | "-airflow" | "-heatsink" => i += 2,
                        s if s.starts_with('-') => i += 1,
                        _ => i += 1,
                    }
                }
                if voltage_v.is_none() && temp.is_none() {
                    return Err(format!(
                        "{line}: set_operating_conditions needs -voltage or -temperature"
                    ));
                }
                if let Some(v) = voltage_v {
                    c.operating_conditions.voltage_mv = (v * 1000.0).round() as i64;
                    c.operating_conditions.voltage_set = true;
                }
                if let Some(t) = temp {
                    c.operating_conditions.temperature_c = t;
                    c.operating_conditions.temperature_set = true;
                }
            }
            "set_property" => {
                let key = toks.get(1).copied().unwrap_or("");
                if key.eq_ignore_ascii_case("PACKAGE_PIN") && toks.len() >= 3 {
                    let site = toks[2].to_string();
                    let joined = toks[3..].join(" ");
                    let port = tcl_name(&joined, "get_ports").unwrap_or_default();
                    if !port.is_empty() {
                        c.package_pins.insert(port, site);
                    }
                } else if (key.eq_ignore_ascii_case("IOSTANDARD")
                    || key.eq_ignore_ascii_case("DRIVE")
                    || key.eq_ignore_ascii_case("SLEW")
                    || key.eq_ignore_ascii_case("PULLTYPE")
                    || key.eq_ignore_ascii_case("DIFF_TERM")
                    || key.eq_ignore_ascii_case("IN_TERM"))
                    && toks.len() >= 3
                {
                    let val = toks[2].to_string();
                    let joined = toks[3..].join(" ");
                    let port = tcl_name(&joined, "get_ports")
                        .or_else(|| {
                            toks.get(3).map(|s| {
                                s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                    .to_string()
                            })
                            .filter(|s| !s.is_empty())
                        })
                        .unwrap_or_default();
                    if !port.is_empty() {
                        if key.eq_ignore_ascii_case("IOSTANDARD") {
                            c.iostandards.insert(port, val);
                        } else if key.eq_ignore_ascii_case("DRIVE") {
                            c.drives.insert(port, val);
                        } else if key.eq_ignore_ascii_case("SLEW") {
                            c.slews.insert(port, val);
                        } else if key.eq_ignore_ascii_case("PULLTYPE") {
                            c.pulltypes.insert(port, val);
                        } else if key.eq_ignore_ascii_case("DIFF_TERM") {
                            c.diff_terms.insert(port, val);
                        } else {
                            c.in_terms.insert(port, val);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(c)
}

pub fn apply_xdc(design: &mut Design, xdc: &Constraints) -> Result<(), String> {
    xdc.apply(design)
}

/// Apply `set_input_delay` / `set_output_delay` / `set_false_path` /
/// `set_multicycle_path` / `set_max_delay` / `set_min_delay` / `set_clock_groups`
/// / `set_clock_uncertainty` / `set_clock_latency` / `set_disable_timing`
/// / `set_case_analysis` / `set_propagated_clock` / `set_clock_sense`
/// / `set_input_jitter` / `set_system_jitter` / `set_timing_derate`
/// / `set_operating_conditions` / `set_bus_skew` / `group_path`
/// / `set_max_time_borrow` / `set_data_check` to an STA result.
/// False paths, async/exclusive clock groups, disable_timing, and case_analysis
/// drop the IOB contribution; I/O delays add to setup and move WNS.
/// Setup MCP N uses N×period as the requirement; `set_max_delay` replaces it.
/// Hold MCP M subtracts M×period from hold slack; `set_min_delay` replaces HOLD_REQ_PS.
/// Uncertainty, jitter, bus skew, data-check, and late/early latency subtract from
/// setup/hold slack. Latch `set_max_time_borrow` adds to setup slack.
/// Late derate, PVT, and `group_path -weight` scale setup delay; early derate and PVT
/// scale hold delay.
/// Propagated clocks add routed clock-network insertion; `-negative` sense is a
/// half-cycle setup; `-stop_propagation` keeps ideal (0) insertion. Empty
/// constraints keep gold WNS.
pub fn apply_xdc_delays(r: &mut TimingResult, xdc: &Constraints, period_ps: u64) {
    let false_out = !xdc.false_paths.is_empty()
        || xdc.clock_groups_false_path()
        || xdc.arcs_disabled();
    let out_d = xdc.output_delay_ps.values().copied().max().unwrap_or(0);
    let in_d = xdc.input_delay_ps.values().copied().max().unwrap_or(0);
    if false_out {
        r.iob_ps = 0;
        r.setup_ps = r.r2r_ps;
    } else if in_d != 0 || out_d != 0 {
        r.setup_ps += out_d + in_d;
    }
    let clk_net = xdc.propagated_network_ps(r.clk_net_ps);
    let sense = xdc.clock_sense_setup_ps(period_ps);
    let req_ps = xdc
        .max_delay_ps()
        .unwrap_or_else(|| (period_ps as i64).saturating_mul(xdc.setup_mult() as i64));
    let setup = scale_delay_ps(r.setup_ps, xdc.path_late_milli());
    r.wns_ps = req_ps
        - setup
        - xdc.uncertainty_setup_ps()
        - xdc.jitter_setup_ps()
        - xdc.latency_late_ps()
        - clk_net
        - sense
        - xdc.bus_skew_setup_ps()
        - xdc.data_check_setup_ps()
        + xdc.time_borrow_ps();
    r.tns_ps = r.wns_ps.min(0);
    let hold = scale_delay_ps(r.hold_ps, xdc.path_early_milli());
    if let Some(min_d) = xdc.min_delay_ps() {
        r.hold_slack_ps = hold - min_d;
    } else {
        let hold_mult = xdc.hold_mult();
        if hold_mult > 0 || xdc.path_early_milli() != 1000 {
            r.hold_slack_ps =
                hold - HOLD_REQ_PS - (period_ps as i64).saturating_mul(hold_mult as i64);
        }
    }
    r.hold_slack_ps -= xdc.uncertainty_hold_ps()
        + xdc.jitter_hold_ps()
        + xdc.latency_early_ps()
        + clk_net
        + xdc.bus_skew_hold_ps()
        + xdc.data_check_hold_ps();
}

/// STA with XDC delays/false paths applied.
pub fn report_timing_xdc(
    design: &Design,
    clocks: &[Clock],
    xdc: &Constraints,
) -> Result<TimingResult, String> {
    let clks = if xdc.clocks.is_empty() {
        clocks.to_vec()
    } else {
        xdc.clocks.clone()
    };
    let mut r = report_timing(design, &clks)?;
    apply_xdc_delays(&mut r, xdc, clks[0].period_ps);
    Ok(r)
}

/// Routed STA plus XDC I/O delay / false path / multicycle / max_delay /
/// min_delay / clock_groups / uncertainty / latency / disable_timing /
/// case_analysis / propagated_clock / clock_sense / input_jitter /
/// system_jitter / timing_derate / operating_conditions / bus_skew /
/// group_path / max_time_borrow / data_check (UG893 Timing Constraints Apply).
pub fn report_timing_routed_xdc(
    design: &Design,
    routed: &Routed,
    clocks: &[Clock],
    xdc: &Constraints,
) -> Result<TimingResult, String> {
    let mut r = report_timing_routed(design, routed, clocks)?;
    apply_xdc_delays(&mut r, xdc, clocks[0].period_ps);
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::{place, place_with, PlaceOpts};

    #[test]
    fn create_clock_and_generated() {
        let d = Design::structural_blinky();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        create_generated_clock(&mut clks, "clk_div2", "clk", 2, "u_ff/Q").unwrap();
        assert!(clks[1].generated);
        assert_eq!(clks[1].period_ps, 20_000);
        let r = report_timing(&d, &clks).unwrap();
        assert!(r.endpoints >= 1);
        assert_eq!(r.clocks.len(), 2);
        // non-vacuous: path delay is counted
        assert_ne!(r.wns_ps, clks[0].period_ps as i64);
    }

    #[test]
    fn counter_sta_worse_than_blinky() {
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let b = report_timing(&Design::structural_blinky(), &clks).unwrap();
        let c = report_timing(&Design::structural_counter(), &clks).unwrap();
        assert!(
            c.r2r_ps > b.r2r_ps,
            "4-input incrementer LUT must be slower than 1-input inverter ({} vs {})",
            c.r2r_ps,
            b.r2r_ps
        );
        assert!(c.wns_ps < b.wns_ps);
    }

    #[test]
    fn placed_timing_moves_with_iob_distance() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0 }).unwrap();
        let td = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_placed(&d, &wl, &clks).unwrap();
        let b = report_timing_placed(&d, &td, &clks).unwrap();
        assert!(
            b.iob_ps < a.iob_ps,
            "timing-driven must shorten IOB path (TD {} WL {})",
            b.iob_ps,
            a.iob_ps
        );
        let _ = place(&p, &dev);
    }

    #[test]
    fn routed_wns_hold_setup_move_with_placement() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0 }).unwrap();
        let td = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let r_wl = helion_route::route(&wl, &dev).unwrap();
        let r_td = helion_route::route(&td, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&d, &r_wl, &clks).unwrap();
        let b = report_timing_routed(&d, &r_td, &clks).unwrap();
        assert_ne!(a.wns_ps, b.wns_ps, "WNS must move with placement (WL {} TD {})", a.wns_ps, b.wns_ps);
        assert_ne!(a.hold_ps, b.hold_ps, "hold must move with placement");
        assert_ne!(a.setup_ps, b.setup_ps, "setup must move with placement");
        assert!(b.wns_ps > a.wns_ps, "timing-driven must improve WNS (TD {} WL {})", b.wns_ps, a.wns_ps);
        assert!(b.setup_ps < a.setup_ps);
        assert!(b.hold_ps < a.hold_ps, "shorter route → less hold delay");
        assert!(a.route_ps > 0 && b.route_ps > 0);
    }

    #[test]
    fn io_loc_binds_had_site() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let site = dev.iob_sites().next().unwrap();
        let loc_s = format!("IOB_X{}Y{}", site.x, site.y);
        let mut io = IoLocs::default();
        io.set_pin_loc("led", &loc_s);
        assert_eq!(io.pins["led"], loc_s);
        assert!(dev.iob_major(site.x, site.y).is_some());
    }

    #[test]
    fn sdc_create_clock_period_ns() {
        let mut clks = Vec::new();
        load_sdc("create_clock -period 10.000 [get_ports clk]\n", &mut clks).unwrap();
        assert_eq!(clks[0].period_ps, 10_000);
        assert_eq!(clks[0].source, "clk");
        let d = Design::structural_blinky();
        let r = report_timing(&d, &clks).unwrap();
        assert_ne!(r.wns_ps, 10_000);
    }

    #[test]
    fn xdc_delays_false_path_package_pin_bound_in_place() {
        let xdc = r#"
create_clock -period 10.000 [get_ports clk]
create_generated_clock -name clkdiv -source [get_ports clk] -divide_by 2 [get_pins u_ff/Q]
set_input_delay -clock clk 1.5 [get_ports clk]
set_output_delay -clock clk 2.0 [get_ports led]
set_false_path -from [get_ports clk] -to [get_ports led]
set_property PACKAGE_PIN IOB_X5Y0 [get_ports led]
"#;
        let c = load_xdc(xdc).unwrap();
        assert_eq!(c.clocks.len(), 2);
        assert!(c.clocks[1].generated);
        assert_eq!(c.clocks[1].period_ps, 20_000);
        assert_eq!(c.output_delay_ps["led"], 2000);
        assert_eq!(c.input_delay_ps["clk"], 1500);
        assert!(c.false_paths.iter().any(|p| p == "clk" || p == "led"));
        assert_eq!(c.package_pins["led"], "IOB_X5Y0");

        let mut d = Design::structural_blinky();
        apply_xdc(&mut d, &c).unwrap();
        assert_eq!(d.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("LOC"), Some("IOB_X5Y0"));
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.iob_sites[0].x, 5, "PACKAGE_PIN must bind IOB in place");
        assert_eq!(pl.lutff_sites[0].0.x, 5);

        let base = report_timing(&d, &c.clocks).unwrap();
        let mut only = c.clone();
        only.false_paths.clear();
        let with_d = report_timing_xdc(&d, &c.clocks, &only).unwrap();
        assert!(
            with_d.wns_ps < base.wns_ps,
            "output/input delay must worsen WNS ({} vs {})",
            with_d.wns_ps,
            base.wns_ps
        );
        let falsep = report_timing_xdc(&d, &c.clocks, &c).unwrap();
        assert_eq!(falsep.setup_ps, falsep.r2r_ps, "false path must drop IOB from setup");
        assert_ne!(with_d.wns_ps, falsep.wns_ps);

        let routed = helion_route::route(&pl, &dev).unwrap();
        let rbase = report_timing_routed(&d, &routed, &c.clocks).unwrap();
        let rdel = report_timing_routed_xdc(&d, &routed, &c.clocks, &only).unwrap();
        assert_eq!(
            rdel.wns_ps,
            rbase.wns_ps - 1500 - 2000,
            "routed I/O delay must subtract from WNS ({} vs {})",
            rdel.wns_ps,
            rbase.wns_ps
        );
        let rfp = report_timing_routed_xdc(&d, &routed, &c.clocks, &c).unwrap();
        assert_eq!(rfp.setup_ps, rfp.r2r_ps);
        assert_eq!(rfp.iob_ps, 0);
        assert_ne!(rdel.wns_ps, rfp.wns_ps);
    }

    #[test]
    fn iostandard_pad_delay_moves_iob_sta() {
        let mut d = Design::structural_counter();
        let xdc = load_xdc("set_property IOSTANDARD LVCMOS33 [get_ports led]\n").unwrap();
        assert_eq!(xdc.iostandards["led"], "LVCMOS33");
        apply_xdc(&mut d, &xdc).unwrap();
        assert_eq!(
            d.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("IOSTANDARD"),
            Some("LVCMOS33")
        );

        let mut plain = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&plain, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&plain, &routed, &clks).unwrap();
        plain.set_iostandard("led", "LVCMOS33").unwrap();
        let b = report_timing_routed(&plain, &routed, &clks).unwrap();
        assert!(
            b.iob_ps > a.iob_ps,
            "LVCMOS33 pad must slow IOB STA ({} vs {})",
            b.iob_ps,
            a.iob_ps
        );
        assert!(
            b.wns_ps < a.wns_ps,
            "slower I/O standard must worsen WNS ({} vs {})",
            b.wns_ps,
            a.wns_ps
        );
        assert_eq!(a.iob_ps, FF_CKQ_PS + a.route_ps + IOB_PS);
        assert_eq!(b.iob_ps, FF_CKQ_PS + b.route_ps + iostandard_pad_ps(Some("LVCMOS33")));
    }

    #[test]
    fn drive_slew_pulltype_move_iob_sta() {
        let xdc = load_xdc(
            "set_property DRIVE 4 [get_ports led]\nset_property SLEW FAST [get_ports led]\nset_property PULLTYPE PULLUP [get_ports led]\n",
        )
        .unwrap();
        assert_eq!(xdc.drives["led"], "4");
        assert_eq!(xdc.slews["led"], "FAST");
        assert_eq!(xdc.pulltypes["led"], "PULLUP");

        let mut d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&d, &routed, &clks).unwrap();
        d.set_drive("led", "4").unwrap();
        let b = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            b.iob_ps > a.iob_ps,
            "DRIVE 4 must slow IOB STA ({} vs {})",
            b.iob_ps,
            a.iob_ps
        );
        d.set_slew("led", "FAST").unwrap();
        let c = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            c.iob_ps < b.iob_ps,
            "FAST slew must speed IOB STA ({} vs {})",
            c.iob_ps,
            b.iob_ps
        );
        d.set_pulltype("led", "PULLUP").unwrap();
        let e = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            e.iob_ps > c.iob_ps,
            "PULLUP must add pad load ({} vs {})",
            e.iob_ps,
            c.iob_ps
        );
        d.set_drive("led", "12").unwrap();
        d.set_slew("led", "SLOW").unwrap();
        d.set_pulltype("led", "NONE").unwrap();
        let back = report_timing_routed(&d, &routed, &clks).unwrap();
        assert_eq!(back.iob_ps, a.iob_ps, "defaults must restore gold pad");
        assert_eq!(a.iob_ps, FF_CKQ_PS + a.route_ps + IOB_PS);
    }

    #[test]
    fn diff_term_in_term_move_iob_sta() {
        let xdc = load_xdc(
            "set_property DIFF_TERM TRUE [get_ports led]\nset_property IN_TERM UNTUNED_SPLIT_50 [get_ports led]\n",
        )
        .unwrap();
        assert_eq!(xdc.diff_terms["led"], "TRUE");
        assert_eq!(xdc.in_terms["led"], "UNTUNED_SPLIT_50");

        let mut d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let a = report_timing_routed(&d, &routed, &clks).unwrap();
        d.set_diff_term("led", "TRUE").unwrap();
        let b = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            b.iob_ps > a.iob_ps,
            "DIFF_TERM TRUE must add pad load ({} vs {})",
            b.iob_ps,
            a.iob_ps
        );
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let c = report_timing_routed(&d, &routed, &clks).unwrap();
        assert!(
            c.iob_ps > b.iob_ps,
            "IN_TERM UNTUNED_SPLIT_50 must add pad load ({} vs {})",
            c.iob_ps,
            b.iob_ps
        );
        d.set_diff_term("led", "FALSE").unwrap();
        d.set_in_term("led", "NONE").unwrap();
        let back = report_timing_routed(&d, &routed, &clks).unwrap();
        assert_eq!(back.iob_ps, a.iob_ps, "FALSE/NONE must restore gold pad");
    }

    #[test]
    fn xdc_multicycle_max_delay_move_setup_hold() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_multicycle_path 2 -from [get_ports clk] -to [get_ports led]
set_multicycle_path -hold 1 -from [get_ports clk] -to [get_ports led]
set_max_delay 5.0 -from [get_ports clk] -to [get_ports led]
set_max_delay -datapath_only 2.5 -from [get_pins u_ff/Q] -to [get_ports led]
"#,
        )
        .unwrap();
        assert_eq!(xdc.multicycle_paths.len(), 2);
        assert_eq!(xdc.multicycle_paths[0].setup_mult, 2);
        assert_eq!(xdc.multicycle_paths[0].hold_mult, 0);
        assert_eq!(xdc.multicycle_paths[0].from, "clk");
        assert_eq!(xdc.multicycle_paths[0].to, "led");
        assert_eq!(xdc.multicycle_paths[1].setup_mult, 1);
        assert_eq!(xdc.multicycle_paths[1].hold_mult, 1);
        assert_eq!(xdc.setup_mult(), 2);
        assert_eq!(xdc.hold_mult(), 1);
        assert_eq!(xdc.max_delays.len(), 2);
        assert_eq!(xdc.max_delays[0].delay_ps, 5000);
        assert!(!xdc.max_delays[0].datapath_only);
        assert_eq!(xdc.max_delays[1].delay_ps, 2500);
        assert!(xdc.max_delays[1].datapath_only);
        assert_eq!(xdc.max_delay_ps(), Some(2500));

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut mcp = Constraints::default();
        mcp.multicycle_paths.push(xdc.multicycle_paths[0].clone());
        let with_mcp = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &mcp).unwrap();
        assert_eq!(
            with_mcp.wns_ps,
            base.wns_ps + 10_000,
            "setup MCP 2 must add one period to WNS ({} vs {})",
            with_mcp.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_mcp.hold_slack_ps, base.hold_slack_ps,
            "setup-only MCP must not move hold"
        );
        assert_eq!(with_mcp.setup_ps, base.setup_ps);

        let mut hold = mcp.clone();
        hold.multicycle_paths.push(xdc.multicycle_paths[1].clone());
        let with_hold = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &hold).unwrap();
        assert_eq!(with_hold.wns_ps, with_mcp.wns_ps, "hold MCP must not move setup WNS");
        assert_eq!(
            with_hold.hold_slack_ps,
            base.hold_slack_ps - 10_000,
            "hold MCP 1 must subtract one period from hold slack ({} vs {})",
            with_hold.hold_slack_ps,
            base.hold_slack_ps
        );

        let mut md = Constraints::default();
        md.max_delays.push(xdc.max_delays[0].clone());
        let with_md = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &md).unwrap();
        assert_eq!(
            with_md.wns_ps,
            5000 - base.setup_ps,
            "set_max_delay 5 ns replaces the period check ({} setup {})",
            with_md.wns_ps,
            base.setup_ps
        );
        assert_ne!(with_md.wns_ps, base.wns_ps);
        assert_ne!(with_md.wns_ps, with_mcp.wns_ps);
    }

    #[test]
    fn xdc_min_delay_clock_groups_move_hold_setup() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_min_delay 1.0 -from [get_ports clk] -to [get_ports led]
set_min_delay -datapath_only 0.5 -from [get_pins u_ff/Q] -to [get_ports led]
set_clock_groups -asynchronous -group [get_clocks clk] -group [get_clocks virt]
set_clock_groups -physically_exclusive -group [get_clocks sys] -group [get_clocks test]
"#,
        )
        .unwrap();
        assert_eq!(xdc.min_delays.len(), 2);
        assert_eq!(xdc.min_delays[0].delay_ps, 1000);
        assert!(!xdc.min_delays[0].datapath_only);
        assert_eq!(xdc.min_delays[0].from, "clk");
        assert_eq!(xdc.min_delays[0].to, "led");
        assert_eq!(xdc.min_delays[1].delay_ps, 500);
        assert!(xdc.min_delays[1].datapath_only);
        assert_eq!(xdc.min_delay_ps(), Some(1000));
        assert_eq!(xdc.clock_groups.len(), 2);
        assert!(xdc.clock_groups[0].asynchronous);
        assert!(!xdc.clock_groups[0].exclusive);
        assert_eq!(xdc.clock_groups[0].groups, vec![vec!["clk".to_string()], vec!["virt".to_string()]]);
        assert!(xdc.clock_groups[1].exclusive);
        assert!(!xdc.clock_groups[1].asynchronous);
        assert!(xdc.clock_groups_false_path());

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut mind = Constraints::default();
        mind.min_delays.push(xdc.min_delays[0].clone());
        let with_min = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &mind).unwrap();
        assert_eq!(
            with_min.hold_slack_ps,
            base.hold_ps - 1000,
            "set_min_delay 1 ns replaces HOLD_REQ_PS ({} vs hold {})",
            with_min.hold_slack_ps,
            base.hold_ps
        );
        assert_eq!(with_min.wns_ps, base.wns_ps, "min_delay must not move setup WNS");
        assert_ne!(with_min.hold_slack_ps, base.hold_slack_ps);

        let mut od = Constraints::default();
        od.output_delay_ps.insert("led".into(), 2000);
        let with_od = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &od).unwrap();
        assert_eq!(with_od.wns_ps, base.wns_ps - 2000);

        let mut cg = od.clone();
        cg.clock_groups.push(xdc.clock_groups[0].clone());
        let with_cg = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &cg).unwrap();
        assert_eq!(with_cg.iob_ps, 0, "clock groups must drop IOB like false path");
        assert_eq!(with_cg.setup_ps, with_cg.r2r_ps);
        assert_ne!(
            with_cg.wns_ps, with_od.wns_ps,
            "set_clock_groups must move WNS off the I/O-delay result"
        );
    }

    #[test]
    fn xdc_clock_uncertainty_latency_move_setup_hold() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_clock_uncertainty -setup 0.5 [get_clocks clk]
set_clock_uncertainty -hold 0.2 [get_clocks clk]
set_clock_latency -late 0.4 [get_clocks clk]
set_clock_latency -source -early 0.1 [get_clocks clk]
"#,
        )
        .unwrap();
        assert_eq!(xdc.clock_uncertainties.len(), 2);
        assert_eq!(xdc.clock_uncertainties[0].setup_ps, 500);
        assert_eq!(xdc.clock_uncertainties[0].hold_ps, 0);
        assert_eq!(xdc.clock_uncertainties[1].setup_ps, 0);
        assert_eq!(xdc.clock_uncertainties[1].hold_ps, 200);
        assert_eq!(xdc.uncertainty_setup_ps(), 500);
        assert_eq!(xdc.uncertainty_hold_ps(), 200);
        assert_eq!(xdc.clock_latencies.len(), 2);
        assert_eq!(xdc.clock_latencies[0].late_ps, 400);
        assert_eq!(xdc.clock_latencies[0].early_ps, 0);
        assert!(!xdc.clock_latencies[0].source);
        assert_eq!(xdc.clock_latencies[1].late_ps, 0);
        assert_eq!(xdc.clock_latencies[1].early_ps, 100);
        assert!(xdc.clock_latencies[1].source);
        assert_eq!(xdc.latency_late_ps(), 400);
        assert_eq!(xdc.latency_early_ps(), 100);

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut su = Constraints::default();
        su.clock_uncertainties.push(xdc.clock_uncertainties[0].clone());
        let with_su = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &su).unwrap();
        assert_eq!(
            with_su.wns_ps,
            base.wns_ps - 500,
            "setup uncertainty 0.5 ns must worsen WNS ({} vs {})",
            with_su.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_su.hold_slack_ps, base.hold_slack_ps,
            "setup-only uncertainty must not move hold"
        );

        let mut hu = su.clone();
        hu.clock_uncertainties.push(xdc.clock_uncertainties[1].clone());
        let with_hu = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &hu).unwrap();
        assert_eq!(with_hu.wns_ps, with_su.wns_ps, "hold uncertainty must not move setup WNS");
        assert_eq!(
            with_hu.hold_slack_ps,
            base.hold_slack_ps - 200,
            "hold uncertainty 0.2 ns must worsen hold slack ({} vs {})",
            with_hu.hold_slack_ps,
            base.hold_slack_ps
        );

        let mut late = Constraints::default();
        late.clock_latencies.push(xdc.clock_latencies[0].clone());
        let with_late = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &late).unwrap();
        assert_eq!(
            with_late.wns_ps,
            base.wns_ps - 400,
            "late latency 0.4 ns must worsen WNS ({} vs {})",
            with_late.wns_ps,
            base.wns_ps
        );
        assert_eq!(with_late.hold_slack_ps, base.hold_slack_ps);

        let mut early = late.clone();
        early.clock_latencies.push(xdc.clock_latencies[1].clone());
        let with_early = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &early).unwrap();
        assert_eq!(with_early.wns_ps, with_late.wns_ps, "early latency must not move setup WNS");
        assert_eq!(
            with_early.hold_slack_ps,
            base.hold_slack_ps - 100,
            "early latency 0.1 ns must worsen hold slack ({} vs {})",
            with_early.hold_slack_ps,
            base.hold_slack_ps
        );
    }

    #[test]
    fn xdc_disable_timing_case_analysis_drop_paths() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_disable_timing -from [get_ports clk] -to [get_ports led]
set_disable_timing [get_cells u_lut0]
set_case_analysis 0 [get_ports clk]
set_case_analysis 1 [get_pins u_lut0/I0]
"#,
        )
        .unwrap();
        assert_eq!(xdc.disable_timings.len(), 2);
        assert_eq!(xdc.disable_timings[0].from, "clk");
        assert_eq!(xdc.disable_timings[0].to, "led");
        assert_eq!(xdc.disable_timings[1].object, "u_lut0");
        assert_eq!(xdc.case_analyses.len(), 2);
        assert_eq!(xdc.case_analyses[0].value, "0");
        assert_eq!(xdc.case_analyses[0].object, "clk");
        assert_eq!(xdc.case_analyses[1].value, "1");
        assert_eq!(xdc.case_analyses[1].object, "u_lut0");
        assert!(xdc.arcs_disabled());
        assert!(load_xdc("set_disable_timing\n").is_err());
        assert!(load_xdc("set_case_analysis 0\n").is_err());
        assert!(load_xdc("set_case_analysis [get_ports clk]\n").is_err());

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut od = Constraints::default();
        od.output_delay_ps.insert("led".into(), 2000);
        let with_od = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &od).unwrap();
        assert_eq!(with_od.wns_ps, base.wns_ps - 2000);

        let mut dt = od.clone();
        dt.disable_timings.push(xdc.disable_timings[0].clone());
        let with_dt = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &dt).unwrap();
        assert_eq!(with_dt.iob_ps, 0, "disable_timing must drop IOB like false path");
        assert_eq!(with_dt.setup_ps, with_dt.r2r_ps);
        assert_ne!(
            with_dt.wns_ps, with_od.wns_ps,
            "set_disable_timing must move WNS off the I/O-delay result"
        );

        let mut ca = od.clone();
        ca.case_analyses.push(xdc.case_analyses[0].clone());
        let with_ca = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &ca).unwrap();
        assert_eq!(with_ca.iob_ps, 0, "case_analysis must drop IOB like false path");
        assert_eq!(with_ca.setup_ps, with_ca.r2r_ps);
        assert_ne!(
            with_ca.wns_ps, with_od.wns_ps,
            "set_case_analysis must force-drop WNS off the I/O-delay result"
        );
        assert_eq!(with_ca.wns_ps, with_dt.wns_ps);
    }

    #[test]
    fn xdc_propagated_clock_sense_moves_sta() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_propagated_clock [get_clocks clk]
set_propagated_clock [all_clocks]
set_clock_sense -positive [get_pins u_ff/CLK]
set_clock_sense -negative [get_pins u_lut0/I0]
set_clock_sense -stop_propagation [get_pins clk_buf/O]
"#,
        )
        .unwrap();
        assert_eq!(xdc.propagated_clocks, vec!["clk".to_string(), "*".to_string()]);
        assert!(xdc.clocks_propagated());
        assert_eq!(xdc.clock_senses.len(), 3);
        assert_eq!(xdc.clock_senses[0].sense, "positive");
        assert_eq!(xdc.clock_senses[0].object, "u_ff");
        assert_eq!(xdc.clock_senses[1].sense, "negative");
        assert_eq!(xdc.clock_senses[2].sense, "stop");
        assert!(xdc.clock_stopped());
        assert_eq!(xdc.clock_sense_setup_ps(10_000), 5_000);
        assert!(load_xdc("set_propagated_clock\n").is_err());
        assert!(load_xdc("set_clock_sense -negative\n").is_err());
        assert!(load_xdc("set_clock_sense [get_pins u_ff/CLK]\n").is_err());

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);
        assert!(
            base.clk_net_ps > 0,
            "placed clock network must have hop delay, got {}",
            base.clk_net_ps
        );
        assert_eq!(empty.clk_net_ps, base.clk_net_ps, "ideal clocks still measure insertion");
        assert_eq!(
            empty.wns_ps,
            base.wns_ps,
            "ideal clocks must not apply clk_net_ps to WNS"
        );

        let mut prop = Constraints::default();
        prop.propagated_clocks.push("clk".into());
        let with_prop = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &prop).unwrap();
        assert_eq!(
            with_prop.wns_ps,
            base.wns_ps - base.clk_net_ps,
            "set_propagated_clock must add routed clock-network delay ({} vs {} net {})",
            with_prop.wns_ps,
            base.wns_ps,
            base.clk_net_ps
        );
        assert_eq!(
            with_prop.hold_slack_ps,
            base.hold_slack_ps - base.clk_net_ps,
            "propagated clocks must move hold by insertion delay"
        );
        assert_ne!(with_prop.wns_ps, base.wns_ps);

        let mut stop = prop.clone();
        stop.clock_senses.push(xdc.clock_senses[2].clone());
        let with_stop = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &stop).unwrap();
        assert_eq!(
            with_stop.wns_ps, base.wns_ps,
            "stop_propagation must keep ideal insertion (gold WNS)"
        );
        assert_eq!(with_stop.hold_slack_ps, base.hold_slack_ps);

        let mut neg = Constraints::default();
        neg.clock_senses.push(xdc.clock_senses[1].clone());
        let with_neg = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &neg).unwrap();
        assert_eq!(
            with_neg.wns_ps,
            base.wns_ps - 5_000,
            "negative sense is a half-cycle setup ({} vs {})",
            with_neg.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_neg.hold_slack_ps, base.hold_slack_ps,
            "negative sense must not move hold"
        );

        let mut pos = Constraints::default();
        pos.clock_senses.push(xdc.clock_senses[0].clone());
        let with_pos = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &pos).unwrap();
        assert_eq!(with_pos.wns_ps, base.wns_ps, "positive sense is the default edge");

        let mut both = prop.clone();
        both.clock_senses.push(xdc.clock_senses[1].clone());
        let with_both = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &both).unwrap();
        assert_eq!(
            with_both.wns_ps,
            base.wns_ps - base.clk_net_ps - 5_000,
            "propagated + negative must stack"
        );
    }

    #[test]
    fn xdc_input_system_jitter_move_setup_hold() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_input_jitter [get_clocks clk] 0.2
set_system_jitter 0.1
"#,
        )
        .unwrap();
        assert_eq!(xdc.input_jitters.len(), 1);
        assert_eq!(xdc.input_jitters[0].clock, "clk");
        assert_eq!(xdc.input_jitters[0].jitter_ps, 200);
        assert_eq!(xdc.input_jitter_ps(), 200);
        assert_eq!(xdc.system_jitter_ps, 100);
        assert_eq!(xdc.jitter_setup_ps(), 300);
        assert_eq!(xdc.jitter_hold_ps(), 300);
        assert!(load_xdc("set_input_jitter [get_clocks clk]\n").is_err());
        assert!(load_xdc("set_system_jitter\n").is_err());

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut ij = Constraints::default();
        ij.input_jitters.push(xdc.input_jitters[0].clone());
        let with_ij = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &ij).unwrap();
        assert_eq!(
            with_ij.wns_ps,
            base.wns_ps - 200,
            "input jitter 0.2 ns must worsen WNS ({} vs {})",
            with_ij.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_ij.hold_slack_ps,
            base.hold_slack_ps - 200,
            "input jitter 0.2 ns must worsen hold slack"
        );

        let mut sj = Constraints::default();
        sj.system_jitter_ps = xdc.system_jitter_ps;
        let with_sj = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &sj).unwrap();
        assert_eq!(
            with_sj.wns_ps,
            base.wns_ps - 100,
            "system jitter 0.1 ns must worsen WNS ({} vs {})",
            with_sj.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_sj.hold_slack_ps,
            base.hold_slack_ps - 100,
            "system jitter 0.1 ns must worsen hold slack"
        );

        let with_both = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &xdc).unwrap();
        assert_eq!(
            with_both.wns_ps,
            base.wns_ps - 300,
            "input+system jitter must stack on setup"
        );
        assert_eq!(
            with_both.hold_slack_ps,
            base.hold_slack_ps - 300,
            "input+system jitter must stack on hold"
        );
    }

    #[test]
    fn xdc_timing_derate_operating_conditions_move_setup_hold() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_timing_derate -late 1.1
set_timing_derate -early 0.9
set_operating_conditions -voltage 0.95 -temperature 85
"#,
        )
        .unwrap();
        assert_eq!(xdc.timing_derates.len(), 2);
        assert_eq!(xdc.late_derate_milli(), 1100);
        assert_eq!(xdc.early_derate_milli(), 900);
        assert!(xdc.operating_conditions.voltage_set);
        assert_eq!(xdc.operating_conditions.voltage_mv, 950);
        assert!(xdc.operating_conditions.temperature_set);
        assert_eq!(xdc.operating_conditions.temperature_c, 85);
        assert_eq!(xdc.operating_conditions.scale_milli(), 1052 + 120);
        assert!(load_xdc("set_timing_derate -late\n").is_err());
        assert!(load_xdc("set_operating_conditions\n").is_err());
        let both = load_xdc("set_timing_derate 1.08\n").unwrap();
        assert_eq!(both.late_derate_milli(), 1080);
        assert_eq!(both.early_derate_milli(), 1080);

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty =
            report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut late = Constraints::default();
        late.timing_derates.push(xdc.timing_derates[0].clone());
        let with_late = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &late).unwrap();
        let setup_late = base.setup_ps * 1100 / 1000;
        assert_eq!(
            with_late.wns_ps,
            base.wns_ps - (setup_late - base.setup_ps),
            "late derate 1.1 must scale setup ({} vs {})",
            with_late.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_late.hold_slack_ps, base.hold_slack_ps,
            "late derate must not move hold"
        );

        let mut early = Constraints::default();
        early.timing_derates.push(xdc.timing_derates[1].clone());
        let with_early = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &early).unwrap();
        let hold_early = base.hold_ps * 900 / 1000;
        assert_eq!(
            with_early.wns_ps, base.wns_ps,
            "early derate must not move setup WNS"
        );
        assert_eq!(
            with_early.hold_slack_ps,
            base.hold_slack_ps - (base.hold_ps - hold_early),
            "early derate 0.9 must scale hold slack"
        );

        let mut oc_v = Constraints::default();
        oc_v.operating_conditions.voltage_mv = 950;
        oc_v.operating_conditions.voltage_set = true;
        let with_v = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &oc_v).unwrap();
        let setup_v = base.setup_ps * 1052 / 1000;
        let hold_v = base.hold_ps * 1052 / 1000;
        assert_eq!(
            with_v.wns_ps,
            base.wns_ps - (setup_v - base.setup_ps),
            "0.95 V must scale setup WNS"
        );
        assert_eq!(
            with_v.hold_slack_ps,
            base.hold_slack_ps + (hold_v - base.hold_ps),
            "0.95 V slow corner must increase hold delay"
        );

        let mut oc_t = Constraints::default();
        oc_t.operating_conditions.temperature_c = 85;
        oc_t.operating_conditions.temperature_set = true;
        let with_t = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &oc_t).unwrap();
        let setup_t = base.setup_ps * 1120 / 1000;
        assert_eq!(
            with_t.wns_ps,
            base.wns_ps - (setup_t - base.setup_ps),
            "85 C must scale setup WNS"
        );

        let with_all = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &xdc).unwrap();
        let late_m = 1100i64 * (1052 + 120) / 1000;
        let early_m = 900i64 * (1052 + 120) / 1000;
        let setup_all = base.setup_ps * late_m / 1000;
        let hold_all = base.hold_ps * early_m / 1000;
        assert_eq!(
            with_all.wns_ps,
            base.wns_ps - (setup_all - base.setup_ps),
            "derate × PVT must stack on setup"
        );
        assert_eq!(
            with_all.hold_slack_ps,
            base.hold_slack_ps - (base.hold_ps - hold_all),
            "derate × PVT must stack on hold"
        );
        assert_ne!(with_all.wns_ps, base.wns_ps);
        assert_ne!(with_late.wns_ps, with_v.wns_ps);
    }

    #[test]
    fn xdc_bus_skew_group_path_move_sta() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_bus_skew -from [get_ports clk] -to [get_ports led] 0.5
set_bus_skew -setup 0.3 -from [get_ports clk] -to [get_ports led]
set_bus_skew -hold 0.2 -from [get_ports clk] -to [get_ports led]
group_path -name extra -weight 2 -from [get_ports clk] -to [get_ports led]
group_path -name holdg -critical_range 0.4 -from [get_ports clk] -to [get_ports led]
"#,
        )
        .unwrap();
        assert_eq!(xdc.bus_skews.len(), 3);
        assert_eq!(xdc.bus_skews[0].skew_ps, 500);
        assert!(xdc.bus_skews[0].setup && xdc.bus_skews[0].hold);
        assert_eq!(xdc.bus_skews[1].skew_ps, 300);
        assert!(xdc.bus_skews[1].setup && !xdc.bus_skews[1].hold);
        assert_eq!(xdc.bus_skews[2].skew_ps, 200);
        assert!(!xdc.bus_skews[2].setup && xdc.bus_skews[2].hold);
        assert_eq!(xdc.bus_skew_setup_ps(), 500);
        assert_eq!(xdc.bus_skew_hold_ps(), 500);
        assert_eq!(xdc.path_groups.len(), 2);
        assert_eq!(xdc.path_groups[0].name, "extra");
        assert_eq!(xdc.path_groups[0].weight_milli, 2000);
        assert_eq!(xdc.path_groups[1].critical_range_ps, 400);
        assert_eq!(xdc.group_path_weight_milli(), 2000);
        assert_eq!(xdc.group_path_critical_range_ps(), 400);
        assert!(load_xdc("set_bus_skew -from [get_ports clk]\n").is_err());
        assert!(load_xdc("group_path\n").is_err());
        assert!(load_xdc("group_path -weight 0\n").is_err());

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty =
            report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut bs = Constraints::default();
        bs.bus_skews.push(xdc.bus_skews[1].clone());
        let with_bs = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &bs).unwrap();
        assert_eq!(
            with_bs.wns_ps,
            base.wns_ps - 300,
            "setup bus skew 0.3 ns must worsen WNS"
        );
        assert_eq!(
            with_bs.hold_slack_ps, base.hold_slack_ps,
            "setup-only bus skew must not move hold"
        );

        let mut bh = Constraints::default();
        bh.bus_skews.push(xdc.bus_skews[2].clone());
        let with_bh = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &bh).unwrap();
        assert_eq!(with_bh.wns_ps, base.wns_ps, "hold bus skew must not move WNS");
        assert_eq!(
            with_bh.hold_slack_ps,
            base.hold_slack_ps - 200,
            "hold bus skew 0.2 ns must worsen hold slack"
        );

        let mut gp = Constraints::default();
        gp.path_groups.push(xdc.path_groups[0].clone());
        let with_gp = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &gp).unwrap();
        assert_eq!(
            with_gp.wns_ps,
            base.wns_ps - base.setup_ps,
            "group_path -weight 2 must double setup ({} vs {})",
            with_gp.wns_ps,
            base.wns_ps
        );
        assert_eq!(
            with_gp.hold_slack_ps, base.hold_slack_ps,
            "group_path weight must not move hold"
        );

        let mut both = Constraints::default();
        both.bus_skews.push(xdc.bus_skews[1].clone());
        both.path_groups.push(xdc.path_groups[0].clone());
        let with_both = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &both).unwrap();
        assert_eq!(
            with_both.wns_ps,
            base.wns_ps - base.setup_ps - 300,
            "bus skew and group_path weight must stack"
        );
    }

    #[test]
    fn xdc_max_time_borrow_data_check_move_sta() {
        let xdc = load_xdc(
            r#"
create_clock -period 10.000 [get_ports clk]
set_max_time_borrow 1.0 [get_cells u_ff]
set_data_check -setup 0.5 -from [get_ports clk] -to [get_ports led]
set_data_check -hold 0.2 -from [get_ports clk] -to [get_ports led]
set_data_check -from [get_pins A] -to [get_pins B] 0.3
"#,
        )
        .unwrap();
        assert_eq!(xdc.max_time_borrows.len(), 1);
        assert_eq!(xdc.max_time_borrows[0].borrow_ps, 1000);
        assert_eq!(xdc.max_time_borrows[0].object, "u_ff");
        assert_eq!(xdc.time_borrow_ps(), 1000);
        assert_eq!(xdc.data_checks.len(), 3);
        assert_eq!(xdc.data_checks[0].setup_ps, 500);
        assert_eq!(xdc.data_checks[0].hold_ps, 0);
        assert_eq!(xdc.data_checks[1].setup_ps, 0);
        assert_eq!(xdc.data_checks[1].hold_ps, 200);
        assert_eq!(xdc.data_checks[2].setup_ps, 300);
        assert_eq!(xdc.data_checks[2].hold_ps, 300);
        assert_eq!(xdc.data_check_setup_ps(), 500);
        assert_eq!(xdc.data_check_hold_ps(), 300);
        assert!(load_xdc("set_max_time_borrow [get_cells u_ff]\n").is_err());
        assert!(load_xdc("set_data_check -from [get_ports clk]\n").is_err());
        assert!(load_xdc("set_data_check 0.5\n").is_err());

        let d = Design::structural_counter();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let routed = helion_route::route(&pl, &dev).unwrap();
        let base = report_timing_routed(&d, &routed, &xdc.clocks).unwrap();
        let empty =
            report_timing_routed_xdc(&d, &routed, &xdc.clocks, &Constraints::default()).unwrap();
        assert_eq!(empty.wns_ps, base.wns_ps, "empty XDC must keep gold WNS");
        assert_eq!(empty.hold_slack_ps, base.hold_slack_ps);

        let mut tb = Constraints::default();
        tb.max_time_borrows.push(xdc.max_time_borrows[0].clone());
        let with_tb = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &tb).unwrap();
        assert_eq!(
            with_tb.wns_ps,
            base.wns_ps + 1000,
            "latch borrow 1 ns must improve setup WNS"
        );
        assert_eq!(
            with_tb.hold_slack_ps, base.hold_slack_ps,
            "time borrow must not move hold"
        );

        let mut dc = Constraints::default();
        dc.data_checks.push(xdc.data_checks[0].clone());
        let with_dc = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &dc).unwrap();
        assert_eq!(
            with_dc.wns_ps,
            base.wns_ps - 500,
            "setup data check 0.5 ns must worsen WNS"
        );
        assert_eq!(
            with_dc.hold_slack_ps, base.hold_slack_ps,
            "setup-only data check must not move hold"
        );

        let mut dh = Constraints::default();
        dh.data_checks.push(xdc.data_checks[1].clone());
        let with_dh = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &dh).unwrap();
        assert_eq!(with_dh.wns_ps, base.wns_ps, "hold data check must not move WNS");
        assert_eq!(
            with_dh.hold_slack_ps,
            base.hold_slack_ps - 200,
            "hold data check 0.2 ns must worsen hold slack"
        );

        let mut both = Constraints::default();
        both.max_time_borrows.push(xdc.max_time_borrows[0].clone());
        both.data_checks.push(xdc.data_checks[0].clone());
        let with_both = report_timing_routed_xdc(&d, &routed, &xdc.clocks, &both).unwrap();
        assert_eq!(
            with_both.wns_ps,
            base.wns_ps + 1000 - 500,
            "time borrow and data-check setup must stack"
        );
    }

    #[test]
    fn report_clock_interaction_matrix_from_sta_clocks() {
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        create_generated_clock(&mut clks, "clkdiv", "clk", 2, "u_ff/Q").unwrap();
        create_clock(&mut clks, "virt", 8_000, "virt");
        let empty = report_clock_interaction(&[], &Constraints::default(), None);
        assert!(empty.clocks.is_empty());
        assert!(empty.cells.is_empty());
        assert!(empty.text().contains("no clocks"));

        let r = report_clock_interaction(&clks, &Constraints::default(), None);
        assert_eq!(r.clocks.len(), 3);
        assert_eq!(r.cells.len(), 9);
        assert_eq!(r.cell("clk", "clk").unwrap().relation, ClockRelation::Timed);
        assert_eq!(
            r.cell("clk", "clkdiv").unwrap().relation,
            ClockRelation::TimedGenerated
        );
        assert_eq!(
            r.cell("clkdiv", "clk").unwrap().relation,
            ClockRelation::TimedGenerated
        );
        assert_eq!(
            r.cell("clk", "virt").unwrap().relation,
            ClockRelation::TimedUnsafe
        );
        assert!(r.cdc_count() >= 2, "{}", r.text());

        let mut async_xdc = Constraints::default();
        async_xdc.clock_groups.push(ClockGroups {
            asynchronous: true,
            exclusive: false,
            groups: vec![vec!["clk".into()], vec!["virt".into()]],
        });
        let ra = report_clock_interaction(&clks, &async_xdc, None);
        assert_eq!(
            ra.cell("clk", "virt").unwrap().relation,
            ClockRelation::Asynchronous
        );
        assert!(ra.cell("clk", "virt").unwrap().wns_ps.is_none());
        assert_eq!(ra.cell("clk", "clk").unwrap().relation, ClockRelation::Timed);

        let mut ex = Constraints::default();
        ex.clock_groups.push(ClockGroups {
            asynchronous: false,
            exclusive: true,
            groups: vec![vec!["clkdiv".into()], vec!["virt".into()]],
        });
        let re = report_clock_interaction(&clks, &ex, None);
        assert_eq!(
            re.cell("clkdiv", "virt").unwrap().relation,
            ClockRelation::Exclusive
        );

        let mut fp = Constraints::default();
        fp.false_paths
            .push("set_false_path -from [get_clocks clk] -to [get_clocks virt]".into());
        let rf = report_clock_interaction(&clks, &fp, None);
        assert_eq!(
            rf.cell("clk", "virt").unwrap().relation,
            ClockRelation::FalsePath
        );

        let mut dp = Constraints::default();
        dp.max_delays.push(MaxDelay {
            from: "clk".into(),
            to: "virt".into(),
            delay_ps: 2_000,
            datapath_only: true,
        });
        let rd = report_clock_interaction(&clks, &dp, None);
        assert_eq!(
            rd.cell("clk", "virt").unwrap().relation,
            ClockRelation::TimedDatapath
        );
        assert_eq!(rd.cell("clk", "virt").unwrap().requirement_ps, 2_000);

        let d = Design::structural_counter();
        let mut sta_clks = Vec::new();
        create_clock(&mut sta_clks, "clk", 10_000, "clk");
        let t = report_timing(&d, &sta_clks).unwrap();
        let ri = report_clock_interaction(&sta_clks, &Constraints::default(), Some(&t));
        let cell = ri.cell("clk", "clk").unwrap();
        assert_eq!(cell.wns_ps, Some(t.wns_ps));
        assert_eq!(cell.path_count, t.endpoints);
        assert_ne!(t.wns_ps, 0);
    }
}
