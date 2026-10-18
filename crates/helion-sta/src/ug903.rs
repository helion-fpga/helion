//! Vivado UG903 XDC Timing Exceptions, Precedence Engine, `-through` Cone Filtering,
//! Multi-Clock `-start`/`-end` Multicycle, `-clock_fall`/`-add_delay` I/O Delays,
//! and `set_case_analysis` Constant-Mux LUT6 Cofactor Pruning.

use crate::{Clock, Constraints, TimingPath, TimingResult, create_clock};
use helion_backend::TimingModel;
use helion_ir::{CellKind, Design, intern, resolve};
use helion_route::Routed;
use std::collections::{BTreeMap, HashMap, HashSet};

// ---------------------------------------------------------------------------
// 1. UG903 Exception Types & Precedence Hierarchy
// ---------------------------------------------------------------------------

/// UG903 Timing Exception Priority Class (ordered from lowest to highest precedence).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExceptionPrecedence {
    /// Baseline `create_clock` / `create_generated_clock` single-cycle check.
    ClockDefault = 0,
    /// `set_multicycle_path -setup/-hold -start/-end`.
    MulticyclePath = 1,
    /// `set_max_delay` / `set_min_delay` (with optional `-datapath_only`).
    MaxMinDelay = 2,
    /// `set_false_path` / `set_clock_groups` / `set_case_analysis` pruned path (highest priority).
    FalsePath = 3,
}

/// UG903 `set_clock_groups` relationship kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockGroupKind {
    Asynchronous,
    PhysicallyExclusive,
    LogicallyExclusive,
}

impl ClockGroupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asynchronous => "asynchronous",
            Self::PhysicallyExclusive => "physically_exclusive",
            Self::LogicallyExclusive => "logically_exclusive",
        }
    }
}

/// Rich UG903 `set_false_path` rule with `-from`, ordered `-through` stages, and `-to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FalsePathRule {
    pub from: Vec<String>,
    /// Ordered sequence of `-through` stages; each stage is a list of pin/net/cell patterns.
    pub through: Vec<Vec<String>>,
    pub to: Vec<String>,
    pub setup: bool,
    pub hold: bool,
}

/// Rich UG903 `set_multicycle_path` rule supporting `-setup`/`-hold`, `-start`/`-end`, and `-through`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MulticycleRule {
    pub from: Vec<String>,
    pub through: Vec<Vec<String>>,
    pub to: Vec<String>,
    pub multiplier: u32,
    pub is_setup: bool,
    pub is_hold: bool,
    /// `-start`: reference clock is the launch clock; `-end`: reference clock is the capture clock.
    /// Per UG903: default for `-setup` is `-end` (capture clock); default for `-hold` is `-start` (launch clock).
    pub reference_start: bool,
    pub reference_end: bool,
}

/// Rich UG903 `set_max_delay` / `set_min_delay` rule supporting `-through` and `-datapath_only`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelayBoundRule {
    pub is_max: bool,
    pub from: Vec<String>,
    pub through: Vec<Vec<String>>,
    pub to: Vec<String>,
    pub delay_ps: i64,
    pub datapath_only: bool,
}

/// Rich UG903 `set_input_delay` / `set_output_delay` entry supporting `-clock_fall` and `-add_delay`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoDelayRule {
    pub is_output: bool,
    pub port: String,
    pub clock: String,
    pub delay_ps: i64,
    pub is_max: bool,
    pub is_min: bool,
    pub clock_fall: bool,
    pub add_delay: bool,
}

/// Parsed UG903 extended exception set extracted from XDC text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ug903ExceptionSet {
    pub false_paths: Vec<FalsePathRule>,
    pub multicycles: Vec<MulticycleRule>,
    pub delay_bounds: Vec<DelayBoundRule>,
    pub clock_groups: Vec<(ClockGroupKind, Vec<Vec<String>>)>,
    pub io_delays: Vec<IoDelayRule>,
    pub case_analyses: Vec<(String, u8)>,
}

fn match_obj_token(pat: &str, candidate: &str) -> bool {
    let p = pat.trim();
    if p.is_empty() || p == "*" {
        return true;
    }
    if p.contains('*') || p.contains('?') {
        glob_match_simple(p, candidate)
    } else {
        candidate == p
            || candidate.ends_with(&format!("/{p}"))
            || p.ends_with(&format!("/{candidate}"))
    }
}

fn glob_match_simple(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = text.chars().collect();
    fn rec(p: &[char], mut pi: usize, s: &[char], mut si: usize) -> bool {
        while pi < p.len() {
            match p[pi] {
                '*' => {
                    while pi < p.len() && p[pi] == '*' {
                        pi += 1;
                    }
                    if pi == p.len() {
                        return true;
                    }
                    for k in si..=s.len() {
                        if rec(p, pi, s, k) {
                            return true;
                        }
                    }
                    return false;
                }
                '?' => {
                    if si >= s.len() {
                        return false;
                    }
                    pi += 1;
                    si += 1;
                }
                c => {
                    if si >= s.len() || c != s[si] {
                        return false;
                    }
                    pi += 1;
                    si += 1;
                }
            }
        }
        si == s.len()
    }
    rec(&p, 0, &s, 0)
}

fn extract_tcl_targets(chunk: &str) -> Vec<String> {
    let cleaned = chunk
        .replace('[', " ")
        .replace(']', " ")
        .replace('{', " ")
        .replace('}', " ")
        .replace('"', " ");
    let mut out = Vec::new();
    for tok in cleaned.split_whitespace() {
        if matches!(
            tok,
            "get_ports"
                | "get_pins"
                | "get_cells"
                | "get_nets"
                | "get_clocks"
                | "-hierarchical"
                | "-hier"
                | "-of_objects"
                | "-filter"
                | "-quiet"
        ) {
            continue;
        }
        if !tok.starts_with('-') && !tok.is_empty() {
            out.push(tok.to_string());
        }
    }
    out
}

fn parse_from_through_to(toks: &[&str]) -> (Vec<String>, Vec<Vec<String>>, Vec<String>) {
    let mut from = Vec::new();
    let mut through = Vec::new();
    let mut to = Vec::new();
    let mut i = 1;
    while i < toks.len() {
        match toks[i] {
            "-from" | "-rise_from" | "-fall_from" => {
                i += 1;
                let mut chunk = Vec::new();
                while i < toks.len() && !toks[i].starts_with('-') {
                    chunk.push(toks[i]);
                    i += 1;
                }
                from.extend(extract_tcl_targets(&chunk.join(" ")));
            }
            "-through" | "-rise_through" | "-fall_through" => {
                i += 1;
                let mut chunk = Vec::new();
                while i < toks.len() && !toks[i].starts_with('-') {
                    chunk.push(toks[i]);
                    i += 1;
                }
                let stage = extract_tcl_targets(&chunk.join(" "));
                if !stage.is_empty() {
                    through.push(stage);
                }
            }
            "-to" | "-rise_to" | "-fall_to" => {
                i += 1;
                let mut chunk = Vec::new();
                while i < toks.len() && !toks[i].starts_with('-') {
                    chunk.push(toks[i]);
                    i += 1;
                }
                to.extend(extract_tcl_targets(&chunk.join(" ")));
            }
            _ => i += 1,
        }
    }
    (from, through, to)
}

/// Parse UG903 timing exceptions, generated clocks, clock groups, I/O delays, and case analysis from XDC text.
pub fn parse_ug903_exceptions(text: &str) -> Result<Ug903ExceptionSet, String> {
    let mut set = Ug903ExceptionSet::default();
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
            "set_false_path" => {
                let (from, through, to) = parse_from_through_to(&toks);
                let mut setup = false;
                let mut hold = false;
                for &t in &toks[1..] {
                    if t == "-setup" {
                        setup = true;
                    } else if t == "-hold" {
                        hold = true;
                    }
                }
                if !setup && !hold {
                    setup = true;
                    hold = true;
                }
                set.false_paths.push(FalsePathRule {
                    from,
                    through,
                    to,
                    setup,
                    hold,
                });
            }
            "set_multicycle_path" => {
                let (from, through, to) = parse_from_through_to(&toks);
                let mut is_setup = false;
                let mut is_hold = false;
                let mut start = false;
                let mut end = false;
                let mut mult: Option<u32> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-setup" => is_setup = true,
                        "-hold" => is_hold = true,
                        "-start" => start = true,
                        "-end" => end = true,
                        "-from" | "-to" | "-through" | "-rise_from" | "-fall_from"
                        | "-rise_to" | "-fall_to" | "-rise_through" | "-fall_through" => {
                            i += 1;
                            while i < toks.len() && !toks[i].starts_with('-') {
                                i += 1;
                            }
                            continue;
                        }
                        other if !other.starts_with('-') => {
                            if mult.is_none() {
                                if let Ok(v) = other.parse::<u32>() {
                                    mult = Some(v);
                                }
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let m = mult.ok_or_else(|| format!("{line}: missing multicycle multiplier"))?;
                if !is_setup && !is_hold {
                    is_setup = true;
                }
                // UG903 defaults: setup defaults to -end; hold defaults to -start
                let (ref_start, ref_end) = if start || end {
                    (start, end)
                } else if is_hold && !is_setup {
                    (true, false)
                } else {
                    (false, true)
                };
                set.multicycles.push(MulticycleRule {
                    from,
                    through,
                    to,
                    multiplier: m,
                    is_setup,
                    is_hold,
                    reference_start: ref_start,
                    reference_end: ref_end,
                });
            }
            "set_max_delay" | "set_min_delay" => {
                let is_max = toks[0] == "set_max_delay";
                let (from, through, to) = parse_from_through_to(&toks);
                let mut datapath_only = false;
                let mut delay_ns: Option<f64> = None;
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-datapath_only" => datapath_only = true,
                        "-from" | "-to" | "-through" | "-rise_from" | "-fall_from"
                        | "-rise_to" | "-fall_to" | "-rise_through" | "-fall_through" => {
                            i += 1;
                            while i < toks.len() && !toks[i].starts_with('-') {
                                i += 1;
                            }
                            continue;
                        }
                        other if !other.starts_with('-') => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                }
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing delay value"))?;
                set.delay_bounds.push(DelayBoundRule {
                    is_max,
                    from,
                    through,
                    to,
                    delay_ps: (ns * 1000.0).round() as i64,
                    datapath_only,
                });
            }
            "set_clock_groups" => {
                let mut kind = ClockGroupKind::Asynchronous;
                let mut groups = Vec::new();
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-asynchronous" => {
                            kind = ClockGroupKind::Asynchronous;
                            i += 1;
                        }
                        "-physically_exclusive" => {
                            kind = ClockGroupKind::PhysicallyExclusive;
                            i += 1;
                        }
                        "-logically_exclusive" | "-exclusive" => {
                            kind = ClockGroupKind::LogicallyExclusive;
                            i += 1;
                        }
                        "-group" => {
                            i += 1;
                            let mut chunk = Vec::new();
                            while i < toks.len() && !toks[i].starts_with('-') {
                                chunk.push(toks[i]);
                                i += 1;
                            }
                            let names = extract_tcl_targets(&chunk.join(" "));
                            if !names.is_empty() {
                                groups.push(names);
                            }
                        }
                        _ => i += 1,
                    }
                }
                if groups.len() >= 2 {
                    set.clock_groups.push((kind, groups));
                }
            }
            "set_input_delay" | "set_output_delay" => {
                let is_output = toks[0] == "set_output_delay";
                let mut clock = "clk".to_string();
                let mut is_max = false;
                let mut is_min = false;
                let mut clock_fall = false;
                let mut add_delay = false;
                let mut delay_ns: Option<f64> = None;
                let mut ports = Vec::new();
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-clock" => {
                            i += 1;
                            let mut chunk = Vec::new();
                            while i < toks.len() && !toks[i].starts_with('-') {
                                chunk.push(toks[i]);
                                let done = toks[i].ends_with(']')
                                    || (!chunk[0].starts_with('[') && chunk.len() == 1);
                                i += 1;
                                if done {
                                    break;
                                }
                            }
                            if let Some(c_name) = extract_tcl_targets(&chunk.join(" ")).into_iter().next() {
                                clock = c_name;
                            }
                            continue;
                        }
                        "-max" => is_max = true,
                        "-min" => is_min = true,
                        "-clock_fall" => clock_fall = true,
                        "-add_delay" => add_delay = true,
                        other if !other.starts_with('-') => {
                            if delay_ns.is_none() {
                                if let Ok(v) = other.parse::<f64>() {
                                    delay_ns = Some(v);
                                    i += 1;
                                    continue;
                                }
                            }
                            ports.extend(extract_tcl_targets(other));
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if !is_max && !is_min {
                    is_max = true;
                    is_min = true;
                }
                let ns = delay_ns.ok_or_else(|| format!("{line}: missing I/O delay"))?;
                let delay_ps = (ns * 1000.0).round() as i64;
                for port in ports {
                    if !add_delay {
                        set.io_delays.retain(|r| {
                            !(r.is_output == is_output
                                && r.port == port
                                && ((is_max && r.is_max) || (is_min && r.is_min)))
                        });
                    }
                    set.io_delays.push(IoDelayRule {
                        is_output,
                        port,
                        clock: clock.clone(),
                        delay_ps,
                        is_max,
                        is_min,
                        clock_fall,
                        add_delay,
                    });
                }
            }
            "set_case_analysis" => {
                if toks.len() >= 3 {
                    let val_bit = match toks[1].to_ascii_lowercase().as_str() {
                        "0" | "zero" | "falling" | "fall" => Some(0u8),
                        "1" | "one" | "rising" | "rise" => Some(1u8),
                        _ => None,
                    };
                    if let Some(bit) = val_bit {
                        let targets = extract_tcl_targets(&toks[2..].join(" "));
                        for t in targets {
                            set.case_analyses.push((t, bit));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(set)
}

// ---------------------------------------------------------------------------
// 2. `-through` Cone & Path Matching + Specificity Scoring
// ---------------------------------------------------------------------------

fn path_nodes_in_order(path: &TimingPath) -> Vec<String> {
    let mut seq = Vec::new();
    if !path.startpoint.cell.is_empty() {
        seq.push(path.startpoint.cell.clone());
    }
    for c in path.cells.iter().rev() {
        if !seq.contains(c) {
            seq.push(c.clone());
        }
    }
    for n in &path.nets {
        seq.push(n.clone());
    }
    if !path.endpoint.cell.is_empty() && !seq.contains(&path.endpoint.cell) {
        seq.push(path.endpoint.cell.clone());
    }
    seq
}

/// Check whether `path` satisfies `-from`, ordered `-through` stages, and `-to` filters.
pub fn path_matches_exception(
    path: &TimingPath,
    launch_clock: &str,
    capture_clock: &str,
    from: &[String],
    through: &[Vec<String>],
    to: &[String],
) -> bool {
    if !from.is_empty() {
        let start_ok = from.iter().any(|f| {
            match_obj_token(f, &path.startpoint.cell)
                || match_obj_token(f, launch_clock)
                || path.cells.last().is_some_and(|c| match_obj_token(f, c))
        });
        if !start_ok {
            return false;
        }
    }
    if !to.is_empty() {
        let end_ok = to.iter().any(|t| {
            match_obj_token(t, &path.endpoint.cell)
                || match_obj_token(t, capture_clock)
                || path.cells.first().is_some_and(|c| match_obj_token(t, c))
                || path.nets.last().is_some_and(|n| match_obj_token(t, n))
        });
        if !end_ok {
            return false;
        }
    }
    if !through.is_empty() {
        let seq = path_nodes_in_order(path);
        let mut search_from = 0usize;
        for stage in through {
            let mut found_at = None;
            for (idx, node) in seq.iter().enumerate().skip(search_from) {
                if stage.iter().any(|pat| {
                    let base_pat = pat.split('/').next().unwrap_or(pat);
                    match_obj_token(pat, node) || match_obj_token(base_pat, node)
                }) {
                    found_at = Some(idx);
                    break;
                }
            }
            if let Some(pos) = found_at {
                search_from = pos;
            } else {
                return false;
            }
        }
    }
    true
}

/// UG903 specificity score within the same exception class:
/// pin/cell `-from` + `-through` + `-to` beats `-from` + `-to` beats single-ended or clock-level.
fn exception_specificity(from: &[String], through: &[Vec<String>], to: &[String]) -> u32 {
    let mut score = 0u32;
    if !from.is_empty() {
        score += 4;
    }
    if !to.is_empty() {
        score += 4;
    }
    if !through.is_empty() {
        score += 2 + (through.len() as u32);
    }
    score
}

// ---------------------------------------------------------------------------
// 3. `set_case_analysis` Constant Propagation & Constant-Mux LUT6 Pruning
// ---------------------------------------------------------------------------

/// Result of `set_case_analysis` constant propagation and LUT6 cofactor pruning on a [`Design`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaseAnalysisPruneResult {
    /// Net name -> constant logic value (`0` or `1`).
    pub constant_nets: BTreeMap<String, u8>,
    /// Pruned `(lut_cell, input_pin)` arcs whose cofactor is sensitized out by constants.
    pub disabled_lut_pins: Vec<(String, String)>,
    /// Cells whose outputs are forced constant (`0` or `1`) and therefore cannot propagate toggles.
    pub constant_cells: BTreeMap<String, u8>,
}

impl CaseAnalysisPruneResult {
    pub fn is_arc_pruned(&self, cell: &str, pin: &str) -> bool {
        self.disabled_lut_pins
            .iter()
            .any(|(c, p)| c == cell && p == pin)
            || self.constant_cells.contains_key(cell)
    }

    pub fn is_path_pruned(&self, path: &TimingPath) -> bool {
        path.cells
            .iter()
            .any(|c| self.constant_cells.contains_key(c))
            || path
                .nets
                .iter()
                .any(|n| self.constant_nets.contains_key(n))
    }
}

/// Propagate `set_case_analysis` constants (`0`/`1`) through nets and `CellKind::Lut6 { init }` truth tables,
/// identifying disabled mux/logic input pins and constant-output LUTs.
pub fn propagate_case_analysis(
    design: &Design,
    case_rules: &[(String, u8)],
) -> CaseAnalysisPruneResult {
    let mut const_nets: BTreeMap<String, u8> = BTreeMap::new();
    let mut explicit_pin_consts: HashMap<(String, String), u8> = HashMap::new();
    let pins = design.pin_index();

    for (obj, val) in case_rules {
        let bit = val & 1;
        if let Some((cell, pin)) = obj.split_once('/') {
            explicit_pin_consts.insert((cell.to_string(), pin.to_string()), bit);
            if pin == "O" || pin == "Q" {
                if let Some(net) = pins.net_on(cell, pin) {
                    const_nets.insert(resolve(net).to_string(), bit);
                }
            }
        } else {
            const_nets.insert(obj.clone(), bit);
        }
    }

    let mut constant_cells: BTreeMap<String, u8> = BTreeMap::new();
    let mut disabled_lut_pins: Vec<(String, String)> = Vec::new();

    // Fixed-point propagation across combinational LUTs
    for _pass in 0..4 {
        let mut changed = false;
        for cell in &design.cells {
            let CellKind::Lut6 { init } = cell.kind else {
                continue;
            };
            let cname = resolve(cell.name).to_string();
            let mut fixed_inputs: [Option<u8>; 6] = [None; 6];
            let mut connected_inputs: [bool; 6] = [false; 6];

            for k in 0..6usize {
                let pin_name = format!("I{k}");
                if let Some(&b) = explicit_pin_consts.get(&(cname.clone(), pin_name.clone())) {
                    fixed_inputs[k] = Some(b);
                    connected_inputs[k] = true;
                } else if let Some(net_spur) = pins.net_on(&cname, &pin_name) {
                    connected_inputs[k] = true;
                    let nname = resolve(net_spur);
                    if let Some(&b) = const_nets.get(nname) {
                        fixed_inputs[k] = Some(b);
                    }
                }
            }

            // Evaluate active minterms compatible with `fixed_inputs` (unconnected pins default to 0)
            let mut possible_Zero = false;
            let mut possible_One = false;
            for idx in 0..64u64 {
                let mut matches_fixed = true;
                for k in 0..6usize {
                    let bit_k = ((idx >> k) & 1) as u8;
                    if let Some(req) = fixed_inputs[k] {
                        if bit_k != req {
                            matches_fixed = false;
                            break;
                        }
                    } else if !connected_inputs[k] && bit_k != 0 {
                        matches_fixed = false;
                        break;
                    }
                }
                if !matches_fixed {
                    continue;
                }
                let out_bit = ((init >> idx) & 1) as u8;
                if out_bit == 0 {
                    possible_Zero = true;
                } else {
                    possible_One = true;
                }
            }

            if possible_Zero ^ possible_One {
                let const_out = u8::from(possible_One);
                if constant_cells.insert(cname.clone(), const_out) != Some(const_out) {
                    changed = true;
                }
                if let Some(out_net) = pins.net_on(&cname, "O") {
                    let oname = resolve(out_net).to_string();
                    if const_nets.insert(oname, const_out) != Some(const_out) {
                        changed = true;
                    }
                }
            }

            // Check each unfixed connected input pin `j`: if flipping bit `j` never changes the output
            // under the current `fixed_inputs`, then arc `I{j} -> O` is pruned by case analysis!
            for j in 0..6usize {
                if !connected_inputs[j] {
                    continue;
                }
                let pin_name = format!("I{j}");
                if fixed_inputs[j].is_some() {
                    let pair = (cname.clone(), pin_name);
                    if !disabled_lut_pins.contains(&pair) {
                        disabled_lut_pins.push(pair);
                    }
                    continue;
                }
                let mut depends_on_j = false;
                for idx in 0..64u64 {
                    if ((idx >> j) & 1) != 0 {
                        continue;
                    }
                    let mut compat = true;
                    for k in 0..6usize {
                        if k == j {
                            continue;
                        }
                        let bit_k = ((idx >> k) & 1) as u8;
                        if let Some(req) = fixed_inputs[k] {
                            if bit_k != req {
                                compat = false;
                                break;
                            }
                        } else if !connected_inputs[k] && bit_k != 0 {
                            compat = false;
                            break;
                        }
                    }
                    if !compat {
                        continue;
                    }
                    let idx_hi = idx | (1u64 << j);
                    let out0 = (init >> idx) & 1;
                    let out1 = (init >> idx_hi) & 1;
                    if out0 != out1 {
                        depends_on_j = true;
                        break;
                    }
                }
                if !depends_on_j {
                    let pair = (cname.clone(), pin_name);
                    if !disabled_lut_pins.contains(&pair) {
                        disabled_lut_pins.push(pair);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    CaseAnalysisPruneResult {
        constant_nets: const_nets,
        disabled_lut_pins,
        constant_cells,
    }
}

// ---------------------------------------------------------------------------
// 4. Path-Level UG903 Exception Resolution & Slack Evaluation
// ---------------------------------------------------------------------------

/// Resolved exception & slack evaluation for a single [`TimingPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathExceptionResolution {
    pub path_name: String,
    pub winning_precedence: ExceptionPrecedence,
    pub is_false_path: bool,
    pub pruned_by_case_analysis: bool,
    pub datapath_only: bool,
    pub setup_requirement_ps: i64,
    pub hold_requirement_ps: i64,
    pub setup_slack_ps: Option<i64>,
    pub hold_slack_ps: Option<i64>,
}

/// Evaluate UG903 exception precedence, `-through` filtering, `-start`/`-end` multicycle,
/// `-clock_fall`/`-add_delay` I/O delays, and `set_case_analysis` constant-mux pruning on `path`.
pub fn evaluate_path_with_ug903(
    design: &Design,
    path: &TimingPath,
    launch_clock: &Clock,
    capture_clock: &Clock,
    exc: &Ug903ExceptionSet,
    uncertainty_setup_ps: i64,
    uncertainty_hold_ps: i64,
) -> PathExceptionResolution {
    let tm = TimingModel::hl10t();
    let prune = propagate_case_analysis(design, &exc.case_analyses);

    // Check if path is pruned by set_case_analysis (either whole LUT constant or disabled LUT input pin)
    let mut pruned_by_case = prune.is_path_pruned(path);
    if !pruned_by_case {
        let pins = design.pin_index();
        for (lut_cell, dis_pin) in &prune.disabled_lut_pins {
            if path.cells.iter().any(|c| c == lut_cell) {
                if let Some(net_sp) = pins.net_on(lut_cell, dis_pin) {
                    let net_name = resolve(net_sp);
                    if path.nets.iter().any(|n| n == net_name) {
                        pruned_by_case = true;
                        break;
                    }
                }
            }
        }
    }
    if pruned_by_case {
        return PathExceptionResolution {
            path_name: path.name.clone(),
            winning_precedence: ExceptionPrecedence::FalsePath,
            is_false_path: true,
            pruned_by_case_analysis: true,
            datapath_only: false,
            setup_requirement_ps: 0,
            hold_requirement_ps: 0,
            setup_slack_ps: None,
            hold_slack_ps: None,
        };
    }

    // 1. Highest precedence: set_clock_groups & set_false_path
    if launch_clock.name != capture_clock.name {
        for (_kind, groups) in &exc.clock_groups {
            let g_l = groups
                .iter()
                .position(|g| g.iter().any(|c| c == &launch_clock.name));
            let g_c = groups
                .iter()
                .position(|g| g.iter().any(|c| c == &capture_clock.name));
            if matches!((g_l, g_c), (Some(a), Some(b)) if a != b) {
                return PathExceptionResolution {
                    path_name: path.name.clone(),
                    winning_precedence: ExceptionPrecedence::FalsePath,
                    is_false_path: true,
                    pruned_by_case_analysis: false,
                    datapath_only: false,
                    setup_requirement_ps: 0,
                    hold_requirement_ps: 0,
                    setup_slack_ps: None,
                    hold_slack_ps: None,
                };
            }
        }
    }

    for fp in &exc.false_paths {
        if path_matches_exception(
            path,
            &launch_clock.name,
            &capture_clock.name,
            &fp.from,
            &fp.through,
            &fp.to,
        ) {
            return PathExceptionResolution {
                path_name: path.name.clone(),
                winning_precedence: ExceptionPrecedence::FalsePath,
                is_false_path: true,
                pruned_by_case_analysis: false,
                datapath_only: false,
                setup_requirement_ps: 0,
                hold_requirement_ps: 0,
                setup_slack_ps: None,
                hold_slack_ps: None,
            };
        }
    }

    // Account for -clock_fall and -add_delay I/O delays on ports along this path
    let mut io_setup_extra_ps = 0i64;
    let mut io_hold_extra_ps = 0i64;
    for io in &exc.io_delays {
        let hits_port = path.nets.iter().any(|n| n == &io.port)
            || path.startpoint.cell == io.port
            || path.endpoint.cell == io.port;
        if !hits_port {
            continue;
        }
        let fall_penalty = if io.clock_fall {
            (capture_clock.period_ps as i64) / 2
        } else {
            0
        };
        if io.is_max {
            io_setup_extra_ps = io_setup_extra_ps.max(io.delay_ps + fall_penalty);
        }
        if io.is_min {
            io_hold_extra_ps = io_hold_extra_ps.max(io.delay_ps);
        }
    }

    // 2. Second precedence: set_max_delay / set_min_delay (pick highest specificity, then tightest bound)
    let matching_max = exc
        .delay_bounds
        .iter()
        .filter(|b| {
            b.is_max
                && path_matches_exception(
                    path,
                    &launch_clock.name,
                    &capture_clock.name,
                    &b.from,
                    &b.through,
                    &b.to,
                )
        })
        .max_by_key(|b| {
            (
                exception_specificity(&b.from, &b.through, &b.to),
                std::cmp::Reverse(b.delay_ps),
            )
        });

    let matching_min = exc
        .delay_bounds
        .iter()
        .filter(|b| {
            !b.is_max
                && path_matches_exception(
                    path,
                    &launch_clock.name,
                    &capture_clock.name,
                    &b.from,
                    &b.through,
                    &b.to,
                )
        })
        .max_by_key(|b| (exception_specificity(&b.from, &b.through, &b.to), b.delay_ps));

    if matching_max.is_some() || matching_min.is_some() {
        let datapath_only = matching_max.is_some_and(|m| m.datapath_only)
            || matching_min.is_some_and(|m| m.datapath_only);
        let setup_req = matching_max
            .map(|m| m.delay_ps)
            .unwrap_or(capture_clock.period_ps as i64);
        let hold_req = matching_min.map(|m| m.delay_ps).unwrap_or(tm.hold_req_ps);
        let eff_unc_setup = if datapath_only {
            0
        } else {
            uncertainty_setup_ps
        };
        let eff_unc_hold = if datapath_only {
            0
        } else {
            uncertainty_hold_ps
        };
        let setup_slack = setup_req - (path.delay_ps + io_setup_extra_ps) - eff_unc_setup;
        let hold_slack = if datapath_only && matching_min.is_none() {
            None
        } else {
            Some((path.delay_ps - io_hold_extra_ps) - hold_req - eff_unc_hold)
        };
        return PathExceptionResolution {
            path_name: path.name.clone(),
            winning_precedence: ExceptionPrecedence::MaxMinDelay,
            is_false_path: false,
            pruned_by_case_analysis: false,
            datapath_only,
            setup_requirement_ps: setup_req,
            hold_requirement_ps: hold_req,
            setup_slack_ps: Some(setup_slack),
            hold_slack_ps: hold_slack,
        };
    }

    // 3. Third precedence: set_multicycle_path (-setup/-hold, -start/-end)
    let matching_setup_mcp = exc
        .multicycles
        .iter()
        .filter(|m| {
            m.is_setup
                && path_matches_exception(
                    path,
                    &launch_clock.name,
                    &capture_clock.name,
                    &m.from,
                    &m.through,
                    &m.to,
                )
        })
        .max_by_key(|m| exception_specificity(&m.from, &m.through, &m.to));

    let matching_hold_mcp = exc
        .multicycles
        .iter()
        .filter(|m| {
            m.is_hold
                && path_matches_exception(
                    path,
                    &launch_clock.name,
                    &capture_clock.name,
                    &m.from,
                    &m.through,
                    &m.to,
                )
        })
        .max_by_key(|m| exception_specificity(&m.from, &m.through, &m.to));

    if matching_setup_mcp.is_some() || matching_hold_mcp.is_some() {
        let setup_mult = matching_setup_mcp.map(|m| m.multiplier.max(1)).unwrap_or(1);
        // -start uses launch_clock period; -end (default for setup) uses capture_clock period
        let setup_ref_period = if matching_setup_mcp.is_some_and(|m| m.reference_start) {
            launch_clock.period_ps as i64
        } else {
            capture_clock.period_ps as i64
        };
        let setup_req = setup_ref_period.saturating_mul(setup_mult as i64);

        let hold_mult = matching_hold_mcp.map(|m| m.multiplier).unwrap_or(0);
        // -start (default for hold) uses launch_clock period; -end uses capture_clock period
        let hold_ref_period = if matching_hold_mcp.is_some_and(|m| m.reference_end) {
            capture_clock.period_ps as i64
        } else {
            launch_clock.period_ps as i64
        };
        let hold_req = tm.hold_req_ps + hold_ref_period.saturating_mul(hold_mult as i64);

        let setup_slack = setup_req - (path.delay_ps + io_setup_extra_ps) - uncertainty_setup_ps;
        let hold_slack = (path.delay_ps - io_hold_extra_ps) - hold_req - uncertainty_hold_ps;
        return PathExceptionResolution {
            path_name: path.name.clone(),
            winning_precedence: ExceptionPrecedence::MulticyclePath,
            is_false_path: false,
            pruned_by_case_analysis: false,
            datapath_only: false,
            setup_requirement_ps: setup_req,
            hold_requirement_ps: hold_req,
            setup_slack_ps: Some(setup_slack),
            hold_slack_ps: Some(hold_slack),
        };
    }

    // 4. Baseline: single-cycle clock relationship (accounting for generated clock -invert)
    let base_period = capture_clock.period_ps as i64;
    let setup_req = if capture_clock.invert ^ launch_clock.invert {
        base_period / 2
    } else {
        base_period
    };
    let hold_req = tm.hold_req_ps;
    let setup_slack = setup_req - (path.delay_ps + io_setup_extra_ps) - uncertainty_setup_ps;
    let hold_slack = (path.delay_ps - io_hold_extra_ps) - hold_req - uncertainty_hold_ps;

    PathExceptionResolution {
        path_name: path.name.clone(),
        winning_precedence: ExceptionPrecedence::ClockDefault,
        is_false_path: false,
        pruned_by_case_analysis: false,
        datapath_only: false,
        setup_requirement_ps: setup_req,
        hold_requirement_ps: hold_req,
        setup_slack_ps: Some(setup_slack),
        hold_slack_ps: Some(hold_slack),
    }
}

/// Evaluate all routed timing paths in `design` under full UG903 XDC exception rules.
pub fn analyze_paths_ug903(
    design: &Design,
    routed: &Routed,
    xdc_text: &str,
) -> Result<(TimingResult, Vec<PathExceptionResolution>), String> {
    let xdc = crate::load_xdc(xdc_text)?;
    let exc = parse_ug903_exceptions(xdc_text)?;
    let mut clks = xdc.clocks.clone();
    if clks.is_empty() {
        create_clock(&mut clks, "clk", 10_000, "clk");
    }
    let mut tr = crate::report_timing_routed_xdc(design, routed, &clks, &xdc)?;
    let paths = crate::timing_paths_routed(design, routed, &tr);
    let launch_clk = &clks[0];
    let capture_clk = clks.last().unwrap_or(&clks[0]);
    let unc_s = xdc.uncertainty_setup_ps();
    let unc_h = xdc.uncertainty_hold_ps();

    let mut resolutions = Vec::with_capacity(paths.len());
    let mut active_wns = i64::MAX;
    let mut active_tns = 0i64;
    let mut active_whs = i64::MAX;

    for p in &paths {
        let res = evaluate_path_with_ug903(
            design,
            p,
            launch_clk,
            capture_clk,
            &exc,
            unc_s,
            unc_h,
        );
        if let Some(ss) = res.setup_slack_ps {
            active_wns = active_wns.min(ss);
            if ss < 0 {
                active_tns += ss;
            }
        }
        if let Some(hs) = res.hold_slack_ps {
            active_whs = active_whs.min(hs);
        }
        resolutions.push(res);
    }

    if active_wns != i64::MAX {
        tr.wns_ps = active_wns;
        tr.tns_ps = active_tns;
    }
    if active_whs != i64::MAX {
        tr.hold_slack_ps = active_whs;
    }
    let _ = HashSet::<String>::new();
    let _ = intern("clk");
    Ok((tr, resolutions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TimingPathEndpoint, load_xdc};
    use helion_device::Device;
    use helion_ir::PortDir;
    use helion_pack::pack;
    use helion_place::place;
    use helion_route::route;

    fn dummy_path(start: &str, cells: &[&str], nets: &[&str], end: &str, delay_ps: i64) -> TimingPath {
        TimingPath {
            name: format!("{start}->{end}"),
            startpoint: TimingPathEndpoint {
                cell: start.to_string(),
                site_id: "CLB_X1Y1".into(),
                bel_id: "CLB_X1Y1/BLE0.FF".into(),
            },
            endpoint: TimingPathEndpoint {
                cell: end.to_string(),
                site_id: "CLB_X2Y1".into(),
                bel_id: "CLB_X2Y1/BLE0.FF".into(),
            },
            cells: cells.iter().map(|s| (*s).to_string()).collect(),
            nets: nets.iter().map(|s| (*s).to_string()).collect(),
            delay_ps,
            slack_ps: 0,
        }
    }

    #[test]
    fn ug903_exception_precedence_false_beats_max_beats_mcp_beats_clock() {
        let d = Design::structural_counter();
        let clk = Clock {
            name: "clk".into(),
            period_ps: 10_000,
            source: "clk".into(),
            generated: false,
            master: None,
            divide_by: 1,
            multiply_by: 1,
            invert: false,
            edges: Vec::new(),
        };
        let path = dummy_path("u_ff0", &["u_ff1", "u_lut1", "u_ff0"], &["q0", "d1"], "u_ff1", 2_000);

        // 1. Only multicycle_path -> MulticyclePath wins (req = 20_000 ps)
        let exc_mcp = parse_ug903_exceptions(
            "set_multicycle_path 2 -setup -from [get_cells u_ff0] -to [get_cells u_ff1]\n",
        )
        .unwrap();
        let r_mcp = evaluate_path_with_ug903(&d, &path, &clk, &clk, &exc_mcp, 100, 50);
        assert_eq!(r_mcp.winning_precedence, ExceptionPrecedence::MulticyclePath);
        assert_eq!(r_mcp.setup_requirement_ps, 20_000);
        assert_eq!(r_mcp.setup_slack_ps, Some(17_900));

        // 2. Add set_max_delay -datapath_only -> MaxMinDelay overrides MulticyclePath (and zeroes uncertainty!)
        let exc_max = parse_ug903_exceptions(
            "set_multicycle_path 2 -setup -from [get_cells u_ff0] -to [get_cells u_ff1]\n\
             set_max_delay 5.000 -datapath_only -from [get_cells u_ff0] -to [get_cells u_ff1]\n",
        )
        .unwrap();
        let r_max = evaluate_path_with_ug903(&d, &path, &clk, &clk, &exc_max, 100, 50);
        assert_eq!(r_max.winning_precedence, ExceptionPrecedence::MaxMinDelay);
        assert!(r_max.datapath_only);
        assert_eq!(r_max.setup_requirement_ps, 5_000);
        assert_eq!(r_max.setup_slack_ps, Some(3_000)); // 5000 - 2000 (uncertainty omitted for datapath_only)
        assert_eq!(r_max.hold_slack_ps, None);

        // 3. Add set_false_path -> FalsePath overrides both set_max_delay and set_multicycle_path
        let exc_fp = parse_ug903_exceptions(
            "set_multicycle_path 2 -setup -from [get_cells u_ff0] -to [get_cells u_ff1]\n\
             set_max_delay 5.000 -datapath_only -from [get_cells u_ff0] -to [get_cells u_ff1]\n\
             set_false_path -from [get_cells u_ff0] -through [get_cells u_lut1] -to [get_cells u_ff1]\n",
        )
        .unwrap();
        let r_fp = evaluate_path_with_ug903(&d, &path, &clk, &clk, &exc_fp, 100, 50);
        assert_eq!(r_fp.winning_precedence, ExceptionPrecedence::FalsePath);
        assert!(r_fp.is_false_path);
        assert_eq!(r_fp.setup_slack_ps, None);
    }

    #[test]
    fn through_ordered_cone_filtering_and_specificity_tiebreak() {
        let d = Design::structural_counter();
        let clk = Clock {
            name: "clk".into(),
            period_ps: 10_000,
            source: "clk".into(),
            generated: false,
            master: None,
            divide_by: 1,
            multiply_by: 1,
            invert: false,
            edges: Vec::new(),
        };
        let path = dummy_path(
            "u_ff0",
            &["u_ff1", "u_lut_b", "u_lut_a", "u_ff0"],
            &["q0", "mid_net", "d1"],
            "u_ff1",
            1_500,
        );

        // Ordered -through: u_lut_a then u_lut_b matches; non-existent u_lut_z does not match
        let exc = parse_ug903_exceptions(
            "set_max_delay 8.0 -from [get_cells u_ff0] -to [get_cells u_ff1]\n\
             set_max_delay 4.0 -from [get_cells u_ff0] -through [get_cells u_lut_a] -through [get_cells u_lut_b] -to [get_cells u_ff1]\n\
             set_false_path -from [get_cells u_ff0] -through [get_cells u_lut_z] -to [get_cells u_ff1]\n",
        )
        .unwrap();
        let res = evaluate_path_with_ug903(&d, &path, &clk, &clk, &exc, 0, 0);
        assert!(!res.is_false_path, "non-matching -through u_lut_z must not false-path");
        // More specific -from -through -through -to (4.0 ns = 4000 ps) beats less specific -from -to (8.0 ns)
        assert_eq!(res.setup_requirement_ps, 4_000);
        assert_eq!(res.setup_slack_ps, Some(2_500));
    }

    #[test]
    fn multicycle_start_vs_end_uses_launch_or_capture_clock_period() {
        let d = Design::structural_counter();
        let slow_launch = Clock {
            name: "clk_slow".into(),
            period_ps: 10_000,
            source: "clk_slow".into(),
            generated: false,
            master: None,
            divide_by: 1,
            multiply_by: 1,
            invert: false,
            edges: Vec::new(),
        };
        let fast_capture = Clock {
            name: "clk_fast".into(),
            period_ps: 5_000,
            source: "clk_fast".into(),
            generated: false,
            master: None,
            divide_by: 1,
            multiply_by: 1,
            invert: false,
            edges: Vec::new(),
        };
        let path = dummy_path("u_ff0", &["u_ff1", "u_lut0", "u_ff0"], &["q0", "d0"], "u_ff1", 3_000);

        // -start multiplies launch clock (10_000 ps * 2 = 20_000 ps)
        let exc_start = parse_ug903_exceptions(
            "set_multicycle_path 2 -setup -start -from [get_clocks clk_slow] -to [get_clocks clk_fast]\n",
        )
        .unwrap();
        let r_start = evaluate_path_with_ug903(&d, &path, &slow_launch, &fast_capture, &exc_start, 0, 0);
        assert_eq!(r_start.setup_requirement_ps, 20_000);

        // -end multiplies capture clock (5_000 ps * 2 = 10_000 ps)
        let exc_end = parse_ug903_exceptions(
            "set_multicycle_path 2 -setup -end -from [get_clocks clk_slow] -to [get_clocks clk_fast]\n",
        )
        .unwrap();
        let r_end = evaluate_path_with_ug903(&d, &path, &slow_launch, &fast_capture, &exc_end, 0, 0);
        assert_eq!(r_end.setup_requirement_ps, 10_000);
    }

    #[test]
    fn generated_clock_cascading_clock_groups_and_io_delay_clock_fall_add_delay() {
        let xdc_text = r#"
create_clock -period 10.000 -name clk_in [get_ports clk]
create_generated_clock -name clk_div2 -source [get_ports clk] -divide_by 2 [get_pins div2_ff/Q]
create_generated_clock -name clk_div4 -source [get_pins div2_ff/Q] -divide_by 2 [get_pins div4_ff/Q]
create_generated_clock -name clk_edges -source [get_ports clk] -edges {1 2 5} -invert [get_pins edge_ff/Q]
set_clock_groups -physically_exclusive -group [get_clocks clk_div2] -group [get_clocks clk_div4]
set_input_delay -clock [get_clocks clk_in] -max 1.500 [get_ports din]
set_input_delay -clock [get_clocks clk_in] -clock_fall -add_delay -max 2.250 [get_ports din]
set_output_delay -clock [get_clocks clk_in] -clock_fall -add_delay -max 1.750 [get_ports led]
"#;
        let c = load_xdc(xdc_text).unwrap();
        assert_eq!(c.clocks.len(), 4);
        assert_eq!(c.clocks[1].period_ps, 20_000);
        assert_eq!(c.clocks[2].period_ps, 40_000, "cascaded generated clock must divide clk_div2 (20ns) by 2 -> 40ns");
        assert!(c.clocks[3].invert);
        assert_eq!(c.input_delay_ps["din"], 2250, "-add_delay must retain worst-case max input delay");
        assert_eq!(c.output_delay_ps["led"], 1750);

        let exc = parse_ug903_exceptions(xdc_text).unwrap();
        assert_eq!(exc.clock_groups.len(), 1);
        assert_eq!(exc.clock_groups[0].0, ClockGroupKind::PhysicallyExclusive);
        assert_eq!(exc.io_delays.len(), 3, "-add_delay must preserve both rising and falling I/O delay rules");
        assert!(exc.io_delays.iter().any(|r| r.clock_fall && r.add_delay));
    }

    #[test]
    fn set_case_analysis_constant_mux_lut6_cofactor_pruning() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("mux_top");
        d.add_port("clk", PortDir::In);
        d.add_port("sel", PortDir::In);
        d.add_port("led", PortDir::Out);

        // 2:1 mux LUT6: O = I2 ? I1 : I0 (I0 = bit 0, I1 = bit 1, I2 = sel = bit 2)
        let mut mux_init = 0u64;
        for idx in 0..64u64 {
            let i0 = idx & 1;
            let i1 = (idx >> 1) & 1;
            let sel = (idx >> 2) & 1;
            let out = if sel == 1 { i1 } else { i0 };
            mux_init |= out << idx;
        }
        d.add_cell("u_ff_a", CellKind::Hff);
        d.add_cell("u_ff_b", CellKind::Hff);
        d.add_cell("u_mux", CellKind::Lut6 { init: mux_init });
        d.add_cell("u_ff_out", CellKind::Hff);
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("clk", "u_ff_a", "CLK");
        d.connect("clk", "u_ff_b", "CLK");
        d.connect("clk", "u_ff_out", "CLK");
        d.connect("qa", "u_ff_a", "Q");
        d.connect("qa", "u_ff_a", "D");
        d.connect("qb", "u_ff_b", "Q");
        d.connect("qb", "u_ff_b", "D");
        d.connect("qa", "u_mux", "I0");
        d.connect("qb", "u_mux", "I1");
        d.connect("sel", "u_mux", "I2");
        d.connect("mux_o", "u_mux", "O");
        d.connect("mux_o", "u_ff_out", "D");
        d.connect("q_out", "u_ff_out", "Q");
        d.connect("q_out", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");

        // Force sel = 0 -> mux selects I0 (qa), so I1 (qb) is cofactor-independent and pruned!
        let prune_sel0 = propagate_case_analysis(&d, &[("sel".into(), 0)]);
        assert!(
            prune_sel0.is_arc_pruned("u_mux", "I1"),
            "sel=0 must prune unselected mux input I1: {:?}",
            prune_sel0.disabled_lut_pins
        );
        assert!(
            !prune_sel0.is_arc_pruned("u_mux", "I0"),
            "sel=0 must keep active mux input I0"
        );

        // Force sel = 1 -> mux selects I1 (qb), so I0 (qa) is pruned!
        let prune_sel1 = propagate_case_analysis(&d, &[("sel".into(), 1)]);
        assert!(
            prune_sel1.is_arc_pruned("u_mux", "I0"),
            "sel=1 must prune unselected mux input I0: {:?}",
            prune_sel1.disabled_lut_pins
        );
        assert!(
            !prune_sel1.is_arc_pruned("u_mux", "I1"),
            "sel=1 must keep active mux input I1"
        );

        let packed = pack(&d, &dev).unwrap();
        let placed = place(&packed, &dev).unwrap();
        let routed = route(&placed, &dev).unwrap();
        let (_, resolutions) = analyze_paths_ug903(
            &d,
            &routed,
            "create_clock -period 10.000 [get_ports clk]\nset_case_analysis 0 [get_ports sel]\n",
        )
        .unwrap();
        assert!(!resolutions.is_empty());
    }
}
