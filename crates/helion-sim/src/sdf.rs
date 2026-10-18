//! SDF (IEEE 1497 Standard Delay Format) parser and back-annotated gate-level timing
//! simulation engine (`SimTimingModel`).
//!
//! Parses standard `.sdf` cell `IOPATH` delays, `INTERCONNECT` delays, and
//! `SETUP` / `HOLD` / `SETUPHOLD` / `WIDTH` timing checks, propagating arrival
//! windows across combinational cones and detecting setup, hold, and pulse-width
//! violations during gate-level simulation.

use crate::fast::{CompiledSim, ConeNodeKind, LevelizedSchedule, Logic4, NET_UNCONNECTED};
use std::collections::HashMap;

/// Process / voltage / temperature delay corner selector for SDF triples `(min:typ:max)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SdfCorner {
    Min,
    Typ,
    Max,
}

/// Picosecond-resolved `(min, typ, max)` delay triple.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SdfDelayPs {
    pub min_ps: u64,
    pub typ_ps: u64,
    pub max_ps: u64,
}

impl SdfDelayPs {
    pub const fn uniform(ps: u64) -> Self {
        Self {
            min_ps: ps,
            typ_ps: ps,
            max_ps: ps,
        }
    }

    pub const fn new(min_ps: u64, typ_ps: u64, max_ps: u64) -> Self {
        Self {
            min_ps,
            typ_ps,
            max_ps,
        }
    }

    #[inline]
    pub const fn at_corner(self, corner: SdfCorner) -> u64 {
        match corner {
            SdfCorner::Min => self.min_ps,
            SdfCorner::Typ => self.typ_ps,
            SdfCorner::Max => self.max_ps,
        }
    }
}

/// One cell `(IOPATH from_pin to_pin (rise) (fall))` entry from an SDF file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdfIoPath {
    pub cell_type: String,
    pub instance: String,
    pub from_pin: String,
    pub to_pin: String,
    pub rise: SdfDelayPs,
    pub fall: SdfDelayPs,
}

/// One `(INTERCONNECT from_port to_port (rise) (fall))` routing delay entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdfInterconnect {
    pub from_port: String,
    pub to_port: String,
    pub rise: SdfDelayPs,
    pub fall: SdfDelayPs,
}

/// Kind of sequential timing check in an SDF `(TIMINGCHECK ...)` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SdfTimingCheckKind {
    Setup,
    Hold,
    SetupHold,
    Width,
    Recovery,
    Removal,
}

/// One sequential timing check parsed from `(TIMINGCHECK ...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdfTimingCheck {
    pub cell_type: String,
    pub instance: String,
    pub kind: SdfTimingCheckKind,
    pub data_pin: String,
    pub ref_pin: Option<String>,
    pub limit: SdfDelayPs,
    /// For `SETUPHOLD`, `limit` is the setup limit and `hold_limit` is the hold limit.
    pub hold_limit: Option<SdfDelayPs>,
}

/// Parsed IEEE 1497 Standard Delay Format (`.sdf`) document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SdfFile {
    pub sdf_version: String,
    pub design_name: Option<String>,
    /// Multiplier converting SDF file time units into picoseconds (e.g. `1ns` -> `1000.0`).
    pub timescale_ps: f64,
    pub iopaths: Vec<SdfIoPath>,
    pub interconnects: Vec<SdfInterconnect>,
    pub timing_checks: Vec<SdfTimingCheck>,
}

/// Normalize an SDF hierarchical identifier (`u_top\/u_ff0` -> `u_top/u_ff0`).
pub fn normalize_sdf_id(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"');
    let mut out = String::with_capacity(trimmed.len());
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if next == '/' || next == '.' || next == '[' || next == ']' || next == '\\' {
                    out.push(chars.next().unwrap());
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Split an SDF endpoint reference (`u_lut0/I0` or `u_lut0.I0`) into `(instance, pin)`.
pub fn split_sdf_endpoint(port_ref: &str) -> (String, String) {
    let norm = normalize_sdf_id(port_ref);
    if let Some((inst, pin)) = norm.rsplit_once('/') {
        (inst.to_string(), pin.to_string())
    } else if let Some((inst, pin)) = norm.rsplit_once('.') {
        (inst.to_string(), pin.to_string())
    } else {
        (String::new(), norm)
    }
}

/// S-expression AST node used by [`parse_sdf`].
#[derive(Clone, Debug)]
enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

impl Sexp {
    fn as_atom(&self) -> Option<&str> {
        match self {
            Sexp::Atom(s) => Some(s.as_str()),
            Sexp::List(_) => None,
        }
    }

    fn as_list(&self) -> Option<&[Sexp]> {
        match self {
            Sexp::List(v) => Some(v.as_slice()),
            Sexp::Atom(_) => None,
        }
    }
}

fn tokenize_sdf(src: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Skip `//` line comments and `/* ... */` block comments.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        if b == b'(' || b == b')' {
            tokens.push((b as char).to_string());
            i += 1;
            continue;
        }
        if b == b'"' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i >= bytes.len() {
                return Err("sdf parse error: unterminated string literal".to_string());
            }
            let s = String::from_utf8_lossy(&bytes[start..i]).to_string();
            tokens.push(s);
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'('
            && bytes[i] != b')'
        {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
            } else {
                i += 1;
            }
        }
        tokens.push(String::from_utf8_lossy(&bytes[start..i]).to_string());
    }
    Ok(tokens)
}

fn parse_sexp(tokens: &[String], pos: &mut usize) -> Result<Sexp, String> {
    if *pos >= tokens.len() {
        return Err("sdf parse error: unexpected end of input".to_string());
    }
    let tok = &tokens[*pos];
    *pos += 1;
    if tok == "(" {
        let mut items = Vec::new();
        while *pos < tokens.len() && tokens[*pos] != ")" {
            items.push(parse_sexp(tokens, pos)?);
        }
        if *pos >= tokens.len() || tokens[*pos] != ")" {
            return Err("sdf parse error: missing closing ')'".to_string());
        }
        *pos += 1;
        Ok(Sexp::List(items))
    } else if tok == ")" {
        Err("sdf parse error: unexpected ')'".to_string())
    } else {
        Ok(Sexp::Atom(tok.clone()))
    }
}

fn parse_timescale_unit(tokens: &[Sexp]) -> f64 {
    // Handles `(TIMESCALE 1ps)`, `(TIMESCALE 1 ps)`, `(TIMESCALE 100ps)`, `(TIMESCALE 1ns)`, etc.
    let joined: String = tokens
        .iter()
        .skip(1)
        .filter_map(|t| t.as_atom())
        .collect::<Vec<_>>()
        .join("");
    let lower = joined.to_ascii_lowercase();
    let split_idx = lower
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(lower.len());
    let num: f64 = lower[..split_idx].parse().unwrap_or(1.0);
    let unit = &lower[split_idx..];
    let scale = match unit {
        "fs" => 1e-3,
        "ps" | "" => 1.0,
        "ns" => 1_000.0,
        "us" => 1_000_000.0,
        "ms" => 1_000_000_000.0,
        "s" => 1_000_000_000_000.0,
        _ => 1.0,
    };
    (num * scale).max(1e-3)
}

fn parse_delay_triple(sexp: &Sexp, timescale_ps: f64) -> SdfDelayPs {
    let raw = match sexp {
        Sexp::Atom(s) => s.clone(),
        Sexp::List(items) => {
            if items.is_empty() {
                return SdfDelayPs::default();
            }
            if items.len() == 1 {
                if let Sexp::List(_) = &items[0] {
                    return parse_delay_triple(&items[0], timescale_ps);
                }
            }
            items
                .iter()
                .filter_map(|x| x.as_atom())
                .collect::<Vec<_>>()
                .join("")
        }
    };
    let to_ps = |s: &str| -> Option<u64> {
        let v: f64 = s.trim().parse().ok()?;
        Some((v.max(0.0) * timescale_ps).round() as u64)
    };
    let parts: Vec<&str> = raw.split(':').collect();
    match parts.len() {
        1 => {
            let v = to_ps(parts[0]).unwrap_or(0);
            SdfDelayPs::uniform(v)
        }
        2 => {
            let min = to_ps(parts[0]).unwrap_or(0);
            let max = to_ps(parts[1]).unwrap_or(min);
            SdfDelayPs::new(min, (min + max) / 2, max)
        }
        _ => {
            let min = to_ps(parts[0]).or_else(|| to_ps(parts[1])).unwrap_or(0);
            let typ = to_ps(parts[1]).unwrap_or(min);
            let max = to_ps(parts[2]).unwrap_or(typ);
            SdfDelayPs::new(min, typ, max)
        }
    }
}

/// Extract pin name from either an atom `I0` or a qualified list `(posedge CLK)`.
fn extract_pin_name(sexp: &Sexp) -> String {
    match sexp {
        Sexp::Atom(s) => normalize_sdf_id(s),
        Sexp::List(items) => items
            .last()
            .and_then(|x| x.as_atom())
            .map(normalize_sdf_id)
            .unwrap_or_default(),
    }
}

fn parse_delay_entries(
    items: &[Sexp],
    cell_type: &str,
    instance: &str,
    timescale_ps: f64,
    out: &mut SdfFile,
) {
    for item in items {
        let Some(list) = item.as_list() else {
            continue;
        };
        let Some(tag) = list.first().and_then(|x| x.as_atom()) else {
            continue;
        };
        match tag.to_ascii_uppercase().as_str() {
            "ABSOLUTE" | "INCREMENT" => {
                parse_delay_entries(&list[1..], cell_type, instance, timescale_ps, out);
            }
            "COND" => {
                // `(COND expr (IOPATH ...))` -> recurse into nested lists
                parse_delay_entries(&list[1..], cell_type, instance, timescale_ps, out);
            }
            "IOPATH" if list.len() >= 4 => {
                let from_pin = extract_pin_name(&list[1]);
                let to_pin = extract_pin_name(&list[2]);
                let rise = parse_delay_triple(&list[3], timescale_ps);
                let fall = if list.len() >= 5 {
                    parse_delay_triple(&list[4], timescale_ps)
                } else {
                    rise
                };
                out.iopaths.push(SdfIoPath {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    from_pin,
                    to_pin,
                    rise,
                    fall,
                });
            }
            "INTERCONNECT" if list.len() >= 4 => {
                let mut from_port = extract_pin_name(&list[1]);
                let mut to_port = extract_pin_name(&list[2]);
                if !instance.is_empty() && !from_port.contains('/') && !from_port.contains('.') {
                    from_port = format!("{instance}/{from_port}");
                }
                if !instance.is_empty() && !to_port.contains('/') && !to_port.contains('.') {
                    to_port = format!("{instance}/{to_port}");
                }
                let rise = parse_delay_triple(&list[3], timescale_ps);
                let fall = if list.len() >= 5 {
                    parse_delay_triple(&list[4], timescale_ps)
                } else {
                    rise
                };
                out.interconnects.push(SdfInterconnect {
                    from_port,
                    to_port,
                    rise,
                    fall,
                });
            }
            _ => {}
        }
    }
}

fn parse_timingcheck_entries(
    items: &[Sexp],
    cell_type: &str,
    instance: &str,
    timescale_ps: f64,
    out: &mut SdfFile,
) {
    for item in items {
        let Some(list) = item.as_list() else {
            continue;
        };
        let Some(tag) = list.first().and_then(|x| x.as_atom()) else {
            continue;
        };
        match tag.to_ascii_uppercase().as_str() {
            "SETUP" if list.len() >= 4 => {
                let data_pin = extract_pin_name(&list[1]);
                let ref_pin = Some(extract_pin_name(&list[2]));
                let limit = parse_delay_triple(&list[3], timescale_ps);
                out.timing_checks.push(SdfTimingCheck {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    kind: SdfTimingCheckKind::Setup,
                    data_pin,
                    ref_pin,
                    limit,
                    hold_limit: None,
                });
            }
            "HOLD" if list.len() >= 4 => {
                let data_pin = extract_pin_name(&list[1]);
                let ref_pin = Some(extract_pin_name(&list[2]));
                let limit = parse_delay_triple(&list[3], timescale_ps);
                out.timing_checks.push(SdfTimingCheck {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    kind: SdfTimingCheckKind::Hold,
                    data_pin,
                    ref_pin,
                    limit,
                    hold_limit: None,
                });
            }
            "SETUPHOLD" if list.len() >= 5 => {
                let data_pin = extract_pin_name(&list[1]);
                let ref_pin = Some(extract_pin_name(&list[2]));
                let setup_limit = parse_delay_triple(&list[3], timescale_ps);
                let hold_limit = parse_delay_triple(&list[4], timescale_ps);
                out.timing_checks.push(SdfTimingCheck {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    kind: SdfTimingCheckKind::SetupHold,
                    data_pin,
                    ref_pin,
                    limit: setup_limit,
                    hold_limit: Some(hold_limit),
                });
            }
            "WIDTH" if list.len() >= 3 => {
                let data_pin = extract_pin_name(&list[1]);
                let limit = parse_delay_triple(&list[2], timescale_ps);
                out.timing_checks.push(SdfTimingCheck {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    kind: SdfTimingCheckKind::Width,
                    data_pin,
                    ref_pin: None,
                    limit,
                    hold_limit: None,
                });
            }
            "RECOVERY" if list.len() >= 4 => {
                let data_pin = extract_pin_name(&list[1]);
                let ref_pin = Some(extract_pin_name(&list[2]));
                let limit = parse_delay_triple(&list[3], timescale_ps);
                out.timing_checks.push(SdfTimingCheck {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    kind: SdfTimingCheckKind::Recovery,
                    data_pin,
                    ref_pin,
                    limit,
                    hold_limit: None,
                });
            }
            "REMOVAL" if list.len() >= 4 => {
                let data_pin = extract_pin_name(&list[1]);
                let ref_pin = Some(extract_pin_name(&list[2]));
                let limit = parse_delay_triple(&list[3], timescale_ps);
                out.timing_checks.push(SdfTimingCheck {
                    cell_type: cell_type.to_string(),
                    instance: instance.to_string(),
                    kind: SdfTimingCheckKind::Removal,
                    data_pin,
                    ref_pin,
                    limit,
                    hold_limit: None,
                });
            }
            _ => {}
        }
    }
}

/// Parse an IEEE 1497 Standard Delay Format (`.sdf`) text into an [`SdfFile`].
pub fn parse_sdf(src: &str) -> Result<SdfFile, String> {
    let tokens = tokenize_sdf(src)?;
    if tokens.is_empty() {
        return Err("sdf parse error: empty input".to_string());
    }
    let mut pos = 0;
    let root = parse_sexp(&tokens, &mut pos)?;
    let Some(root_list) = root.as_list() else {
        return Err("sdf parse error: root must be a (DELAYFILE ...) list".to_string());
    };
    let Some(head) = root_list.first().and_then(|x| x.as_atom()) else {
        return Err("sdf parse error: missing root tag".to_string());
    };
    if !head.eq_ignore_ascii_case("DELAYFILE") {
        return Err(format!("sdf parse error: expected DELAYFILE, found {head}"));
    }

    let mut out = SdfFile {
        sdf_version: "3.0".to_string(),
        design_name: None,
        timescale_ps: 1.0,
        iopaths: Vec::new(),
        interconnects: Vec::new(),
        timing_checks: Vec::new(),
    };

    for entry in &root_list[1..] {
        let Some(list) = entry.as_list() else {
            continue;
        };
        let Some(tag) = list.first().and_then(|x| x.as_atom()) else {
            continue;
        };
        match tag.to_ascii_uppercase().as_str() {
            "SDFVERSION" => {
                if let Some(v) = list.get(1).and_then(|x| x.as_atom()) {
                    out.sdf_version = v.to_string();
                }
            }
            "DESIGN" => {
                if let Some(d) = list.get(1).and_then(|x| x.as_atom()) {
                    out.design_name = Some(d.to_string());
                }
            }
            "TIMESCALE" => {
                out.timescale_ps = parse_timescale_unit(list);
            }
            "INTERCONNECT" => {
                parse_delay_entries(std::slice::from_ref(entry), "", "", out.timescale_ps, &mut out);
            }
            "CELL" => {
                let mut cell_type = String::new();
                let mut instance = String::new();
                for sub in &list[1..] {
                    let Some(slist) = sub.as_list() else {
                        continue;
                    };
                    let Some(stag) = slist.first().and_then(|x| x.as_atom()) else {
                        continue;
                    };
                    match stag.to_ascii_uppercase().as_str() {
                        "CELLTYPE" => {
                            if let Some(ct) = slist.get(1).and_then(|x| x.as_atom()) {
                                cell_type = normalize_sdf_id(ct);
                            }
                        }
                        "INSTANCE" => {
                            if let Some(inst) = slist.get(1).and_then(|x| x.as_atom()) {
                                if inst != "*" {
                                    instance = normalize_sdf_id(inst);
                                }
                            }
                        }
                        "DELAY" => {
                            let ts = out.timescale_ps;
                            parse_delay_entries(&slist[1..], &cell_type, &instance, ts, &mut out);
                        }
                        "TIMINGCHECK" => {
                            let ts = out.timescale_ps;
                            parse_timingcheck_entries(
                                &slist[1..],
                                &cell_type,
                                &instance,
                                ts,
                                &mut out,
                            );
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    Ok(out)
}

/// Kind of timing violation detected during back-annotated gate-level simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimingViolationKind {
    Setup,
    Hold,
    PulseWidth,
}

impl TimingViolationKind {
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Setup => "SETUP",
            Self::Hold => "HOLD",
            Self::PulseWidth => "WIDTH",
        }
    }
}

/// One dynamic timing violation detected during SDF gate-level simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimingViolation {
    pub time_ps: u64,
    pub cycle: u64,
    pub kind: TimingViolationKind,
    pub instance: String,
    pub pin: String,
    pub arrival_ps: u64,
    pub clock_edge_ps: u64,
    pub limit_ps: u64,
    pub slack_ps: i64,
    pub message: String,
}

impl std::fmt::Display for TimingViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[SDF][{}] t={}ps cycle={} inst={} pin={} arrival={}ps limit={}ps slack={}ps :: {}",
            self.kind.tag(),
            self.time_ps,
            self.cycle,
            self.instance,
            self.pin,
            self.arrival_ps,
            self.limit_ps,
            self.slack_ps,
            self.message
        )
    }
}

/// Back-annotated gate-level timing model for [`Sim`](crate::Sim) and [`CompiledSim`].
#[derive(Clone, Debug)]
pub struct SimTimingModel {
    pub corner: SdfCorner,
    /// `(instance, from_pin, to_pin)` -> `(rise_ps, fall_ps)`.
    pub iopath_delays: HashMap<(String, String, String), (u64, u64)>,
    /// `(src_inst, src_pin, dst_inst, dst_pin)` -> `(rise_ps, fall_ps)`.
    pub interconnect_delays: HashMap<(String, String, String, String), (u64, u64)>,
    /// `(dst_inst, dst_pin)` -> `(rise_ps, fall_ps)` fallback interconnect lookup by load pin.
    pub load_pin_interconnect: HashMap<(String, String), (u64, u64)>,
    /// `(instance, pin)` -> setup time in picoseconds.
    pub setup_checks: HashMap<(String, String), u64>,
    /// `(instance, pin)` -> hold time in picoseconds.
    pub hold_checks: HashMap<(String, String), u64>,
    /// `(instance, pin)` -> minimum pulse width in picoseconds.
    pub width_checks: HashMap<(String, String), u64>,
    /// Whether setup/hold violations corrupt the destination flip-flop to `X` in 4-state mode.
    pub inject_x_on_violation: bool,
    /// Recorded dynamic timing violations.
    pub violations: Vec<TimingViolation>,
}

impl SimTimingModel {
    /// Build a [`SimTimingModel`] from a parsed [`SdfFile`] at `corner`.
    pub fn from_sdf(sdf: &SdfFile, corner: SdfCorner) -> Self {
        let mut model = Self {
            corner,
            iopath_delays: HashMap::new(),
            interconnect_delays: HashMap::new(),
            load_pin_interconnect: HashMap::new(),
            setup_checks: HashMap::new(),
            hold_checks: HashMap::new(),
            width_checks: HashMap::new(),
            inject_x_on_violation: true,
            violations: Vec::new(),
        };

        for io in &sdf.iopaths {
            let r = io.rise.at_corner(corner);
            let f = io.fall.at_corner(corner);
            model.iopath_delays.insert(
                (io.instance.clone(), io.from_pin.clone(), io.to_pin.clone()),
                (r, f),
            );
        }

        for ic in &sdf.interconnects {
            let (src_inst, src_pin) = split_sdf_endpoint(&ic.from_port);
            let (dst_inst, dst_pin) = split_sdf_endpoint(&ic.to_port);
            let r = ic.rise.at_corner(corner);
            let f = ic.fall.at_corner(corner);
            model
                .interconnect_delays
                .insert((src_inst, src_pin, dst_inst.clone(), dst_pin.clone()), (r, f));
            let entry = model
                .load_pin_interconnect
                .entry((dst_inst, dst_pin))
                .or_insert((0, 0));
            entry.0 = entry.0.max(r);
            entry.1 = entry.1.max(f);
        }

        for tc in &sdf.timing_checks {
            let lim = tc.limit.at_corner(corner);
            match tc.kind {
                SdfTimingCheckKind::Setup | SdfTimingCheckKind::Recovery => {
                    model
                        .setup_checks
                        .insert((tc.instance.clone(), tc.data_pin.clone()), lim);
                }
                SdfTimingCheckKind::Hold | SdfTimingCheckKind::Removal => {
                    model
                        .hold_checks
                        .insert((tc.instance.clone(), tc.data_pin.clone()), lim);
                }
                SdfTimingCheckKind::SetupHold => {
                    model
                        .setup_checks
                        .insert((tc.instance.clone(), tc.data_pin.clone()), lim);
                    if let Some(h) = tc.hold_limit {
                        model.hold_checks.insert(
                            (tc.instance.clone(), tc.data_pin.clone()),
                            h.at_corner(corner),
                        );
                    }
                }
                SdfTimingCheckKind::Width => {
                    model
                        .width_checks
                        .insert((tc.instance.clone(), tc.data_pin.clone()), lim);
                }
            }
        }

        model
    }

    /// Parse SDF text and construct a [`SimTimingModel`] at `corner`.
    pub fn parse(sdf_src: &str, corner: SdfCorner) -> Result<Self, String> {
        let sdf = parse_sdf(sdf_src)?;
        Ok(Self::from_sdf(&sdf, corner))
    }

    /// Query IOPATH delay `(rise_ps, fall_ps)` for `(instance, from_pin, to_pin)`.
    pub fn iopath_delay(&self, instance: &str, from_pin: &str, to_pin: &str, rising: bool) -> u64 {
        if let Some(&(r, f)) = self.iopath_delays.get(&(
            instance.to_string(),
            from_pin.to_string(),
            to_pin.to_string(),
        )) {
            return if rising { r } else { f };
        }
        0
    }

    /// Query INTERCONNECT delay arriving at `(dst_inst, dst_pin)` (optionally from `(src_inst, src_pin)`).
    pub fn interconnect_delay(
        &self,
        src_inst: Option<(&str, &str)>,
        dst_inst: &str,
        dst_pin: &str,
        rising: bool,
    ) -> u64 {
        if let Some((si, sp)) = src_inst {
            if let Some(&(r, f)) = self.interconnect_delays.get(&(
                si.to_string(),
                sp.to_string(),
                dst_inst.to_string(),
                dst_pin.to_string(),
            )) {
                return if rising { r } else { f };
            }
        }
        if let Some(&(r, f)) = self
            .load_pin_interconnect
            .get(&(dst_inst.to_string(), dst_pin.to_string()))
        {
            return if rising { r } else { f };
        }
        0
    }

    /// Step a [`CompiledSim`] by one clock cycle of `period_ps` with full SDF back-annotated
    /// path arrival propagation and dynamic `SETUP` / `HOLD` / `WIDTH` violation checking.
    ///
    /// Returns any timing violations detected on this clock edge.
    pub fn step_timed(&mut self, sim: &mut CompiledSim, period_ps: u64) -> Vec<TimingViolation> {
        let next_cycle = sim.cycle + 1;
        let next_time_ps = sim.time + period_ps;
        let mut step_violations = Vec::new();

        // 1. Check clock pulse width (`WIDTH` check on `CLK` pins).
        let pulse_width_ps = period_ps / 2;
        for ((inst, pin), &min_width_ps) in &self.width_checks {
            if pulse_width_ps < min_width_ps {
                let slack = pulse_width_ps as i64 - min_width_ps as i64;
                let v = TimingViolation {
                    time_ps: next_time_ps,
                    cycle: next_cycle,
                    kind: TimingViolationKind::PulseWidth,
                    instance: inst.clone(),
                    pin: pin.clone(),
                    arrival_ps: pulse_width_ps,
                    clock_edge_ps: period_ps,
                    limit_ps: min_width_ps,
                    slack_ps: slack,
                    message: format!(
                        "clock pulse width {pulse_width_ps}ps is shorter than minimum width {min_width_ps}ps"
                    ),
                };
                self.violations.push(v.clone());
                step_violations.push(v);
            }
        }

        // 2. Snapshot pre-step net and FF values to identify transitions launched on this edge.
        let num_nets = sim.schedule.num_nets();
        let prev_nets: Vec<Logic4> = (0..num_nets as u32)
            .map(|id| sim.net_logic4(id))
            .collect();
        let prev_ffs: Vec<Logic4> = sim
            .schedule
            .ffs
            .iter()
            .map(|ff| sim.get_ff_4state(&ff.name).unwrap_or(Logic4::Zero))
            .collect();

        // Advance functional 4-state simulation by one posedge.
        sim.step_4state(period_ps);

        // 3. Compute earliest and latest transition arrival times (offset in ps from launch edge)
        //    for every net that changed during this cycle.
        let mut net_min_arr: Vec<Option<u64>> = vec![None; num_nets];
        let mut net_max_arr: Vec<Option<u64>> = vec![None; num_nets];
        let mut net_src_pin: Vec<Option<(String, String)>> = vec![None; num_nets];

        // Primary inputs that changed launch at offset 0ps.
        for (port_name, &net_id) in &sim.schedule.input_ports {
            let cur = sim.net_logic4(net_id);
            if cur != prev_nets[net_id as usize] {
                net_min_arr[net_id as usize] = Some(0);
                net_max_arr[net_id as usize] = Some(0);
                net_src_pin[net_id as usize] = Some((port_name.clone(), "PAD".to_string()));
            }
        }

        // Flip-flops whose Q output transitioned launch at `CLK -> Q` IOPATH delay (or if D was
        // already changing before the edge from combinational inputs, also seed from FF Q).
        for (idx, ff) in sim.schedule.ffs.iter().enumerate() {
            if ff.q_net == NET_UNCONNECTED {
                continue;
            }
            let cur_q = sim.get_ff_4state(&ff.name).unwrap_or(Logic4::Zero);
            let rising = cur_q == Logic4::One;
            let clk_to_q = self.iopath_delay(&ff.name, "CLK", "Q", rising);
            // Seed arrival on Q net if the FF toggled or if its Q net has an annotated CLK->Q arc.
            if cur_q != prev_ffs[idx] || clk_to_q > 0 {
                net_min_arr[ff.q_net as usize] = Some(clk_to_q);
                net_max_arr[ff.q_net as usize] = Some(clk_to_q);
            }
            net_src_pin[ff.q_net as usize] = Some((ff.name.clone(), "Q".to_string()));
        }

        // Propagate arrival windows through topological combinational ops.
        for node in &sim.schedule.ops {
            match &node.kind {
                ConeNodeKind::Lut6 {
                    inputs, out_net, ..
                } => {
                    if *out_net == NET_UNCONNECTED {
                        continue;
                    }
                    let rising = sim.net_logic4(*out_net) == Logic4::One;
                    let mut min_a: Option<u64> = None;
                    let mut max_a: Option<u64> = None;
                    for (pin_idx, &in_net) in inputs.iter().enumerate() {
                        if in_net < 2 || in_net == NET_UNCONNECTED {
                            continue;
                        }
                        if let (Some(in_min), Some(in_max)) =
                            (net_min_arr[in_net as usize], net_max_arr[in_net as usize])
                        {
                            let pin_name = format!("I{pin_idx}");
                            let src = net_src_pin[in_net as usize]
                                .as_ref()
                                .map(|(si, sp)| (si.as_str(), sp.as_str()));
                            let ic = self.interconnect_delay(src, &node.cell_name, &pin_name, rising);
                            let cell_d = self.iopath_delay(&node.cell_name, &pin_name, "O", rising);
                            let cand_min = in_min + ic + cell_d;
                            let cand_max = in_max + ic + cell_d;
                            min_a = Some(min_a.map_or(cand_min, |m| m.min(cand_min)));
                            max_a = Some(max_a.map_or(cand_max, |m| m.max(cand_max)));
                        }
                    }
                    net_min_arr[*out_net as usize] = min_a;
                    net_max_arr[*out_net as usize] = max_a;
                    net_src_pin[*out_net as usize] = Some((node.cell_name.clone(), "O".to_string()));
                }
                ConeNodeKind::Carry4 {
                    ci_net,
                    cyinit_net,
                    di_nets,
                    s_nets,
                    o_nets,
                    co_nets,
                } => {
                    let mut carry_min: Option<u64> = None;
                    let mut carry_max: Option<u64> = None;
                    for (pin_name, &net_id) in [("CI", ci_net), ("CYINIT", cyinit_net)] {
                        if net_id >= 2 && net_id != NET_UNCONNECTED {
                            if let (Some(imn), Some(imx)) =
                                (net_min_arr[net_id as usize], net_max_arr[net_id as usize])
                            {
                                let src = net_src_pin[net_id as usize]
                                    .as_ref()
                                    .map(|(si, sp)| (si.as_str(), sp.as_str()));
                                let ic =
                                    self.interconnect_delay(src, &node.cell_name, pin_name, true);
                                carry_min = Some(imn + ic);
                                carry_max = Some(imx + ic);
                            }
                        }
                    }
                    for i in 0..4 {
                        let mut bit_min = carry_min;
                        let mut bit_max = carry_max;
                        for (prefix, net_id) in [("DI", di_nets[i]), ("S", s_nets[i])] {
                            if net_id >= 2 && net_id != NET_UNCONNECTED {
                                if let (Some(imn), Some(imx)) =
                                    (net_min_arr[net_id as usize], net_max_arr[net_id as usize])
                                {
                                    let pin_name = format!("{prefix}{i}");
                                    let src = net_src_pin[net_id as usize]
                                        .as_ref()
                                        .map(|(si, sp)| (si.as_str(), sp.as_str()));
                                    let ic = self.interconnect_delay(
                                        src,
                                        &node.cell_name,
                                        &pin_name,
                                        true,
                                    );
                                    let cand_min = imn + ic;
                                    let cand_max = imx + ic;
                                    bit_min = Some(bit_min.map_or(cand_min, |m| m.min(cand_min)));
                                    bit_max = Some(bit_max.map_or(cand_max, |m| m.max(cand_max)));
                                }
                            }
                        }
                        let o_pin = format!("O{i}");
                        let co_pin = format!("CO{i}");
                        let o_iopath = self
                            .iopath_delay(&node.cell_name, &format!("S{i}"), &o_pin, true)
                            .max(self.iopath_delay(&node.cell_name, "CI", &o_pin, true));
                        let co_iopath = self
                            .iopath_delay(&node.cell_name, &format!("S{i}"), &co_pin, true)
                            .max(self.iopath_delay(&node.cell_name, "CI", &co_pin, true));
                        if o_nets[i] != NET_UNCONNECTED {
                            net_min_arr[o_nets[i] as usize] = bit_min.map(|m| m + o_iopath);
                            net_max_arr[o_nets[i] as usize] = bit_max.map(|m| m + o_iopath);
                            net_src_pin[o_nets[i] as usize] =
                                Some((node.cell_name.clone(), o_pin));
                        }
                        carry_min = bit_min.map(|m| m + co_iopath);
                        carry_max = bit_max.map(|m| m + co_iopath);
                        if co_nets[i] != NET_UNCONNECTED {
                            net_min_arr[co_nets[i] as usize] = carry_min;
                            net_max_arr[co_nets[i] as usize] = carry_max;
                            net_src_pin[co_nets[i] as usize] =
                                Some((node.cell_name.clone(), co_pin));
                        }
                    }
                }
                ConeNodeKind::Muxf7 {
                    i0_net,
                    i1_net,
                    s_net,
                    o_net,
                }
                | ConeNodeKind::Muxf8 {
                    i0_net,
                    i1_net,
                    s_net,
                    o_net,
                } => {
                    if *o_net == NET_UNCONNECTED {
                        continue;
                    }
                    let rising = sim.net_logic4(*o_net) == Logic4::One;
                    let mut min_a: Option<u64> = None;
                    let mut max_a: Option<u64> = None;
                    for (pin_name, &in_net) in [("I0", i0_net), ("I1", i1_net), ("S", s_net)] {
                        if in_net < 2 || in_net == NET_UNCONNECTED {
                            continue;
                        }
                        if let (Some(imn), Some(imx)) =
                            (net_min_arr[in_net as usize], net_max_arr[in_net as usize])
                        {
                            let src = net_src_pin[in_net as usize]
                                .as_ref()
                                .map(|(si, sp)| (si.as_str(), sp.as_str()));
                            let ic =
                                self.interconnect_delay(src, &node.cell_name, pin_name, rising);
                            let cell_d = self.iopath_delay(&node.cell_name, pin_name, "O", rising);
                            let cand_min = imn + ic + cell_d;
                            let cand_max = imx + ic + cell_d;
                            min_a = Some(min_a.map_or(cand_min, |m| m.min(cand_min)));
                            max_a = Some(max_a.map_or(cand_max, |m| m.max(cand_max)));
                        }
                    }
                    net_min_arr[*o_net as usize] = min_a;
                    net_max_arr[*o_net as usize] = max_a;
                    net_src_pin[*o_net as usize] = Some((node.cell_name.clone(), "O".to_string()));
                }
            }
        }

        // 4. Evaluate SETUP and HOLD checks at each sequential FF.
        let mut violated_ffs = Vec::new();
        for (idx, ff) in sim.schedule.ffs.iter().enumerate() {
            let d_net = ff.d_net;
            if d_net < 2 || d_net == NET_UNCONNECTED {
                continue;
            }
            // Check if the FF toggled or if its D net carried an active arrival on this cycle.
            let cur_q = sim.get_ff_4state(&ff.name).unwrap_or(Logic4::Zero);
            let d_changed = cur_q != prev_ffs[idx]
                || sim.net_logic4(d_net) != prev_nets[d_net as usize]
                || net_max_arr[d_net as usize].is_some();
            if !d_changed {
                continue;
            }

            let src = net_src_pin[d_net as usize]
                .as_ref()
                .map(|(si, sp)| (si.as_str(), sp.as_str()));
            let ic_d = self.interconnect_delay(src, &ff.name, "D", cur_q == Logic4::One);
            let min_arr = net_min_arr[d_net as usize].unwrap_or(0) + ic_d;
            let max_arr = net_max_arr[d_net as usize].unwrap_or(0) + ic_d;

            if let Some(&setup_ps) = self.setup_checks.get(&(ff.name.clone(), "D".to_string())) {
                let required_ps = period_ps.saturating_sub(setup_ps);
                let slack_ps = (period_ps as i64) - ((max_arr + setup_ps) as i64);
                if slack_ps < 0 {
                    let v = TimingViolation {
                        time_ps: next_time_ps,
                        cycle: next_cycle,
                        kind: TimingViolationKind::Setup,
                        instance: ff.name.clone(),
                        pin: "D".to_string(),
                        arrival_ps: max_arr,
                        clock_edge_ps: period_ps,
                        limit_ps: setup_ps,
                        slack_ps,
                        message: format!(
                            "setup violation on {}/D: data arrived at {}ps (required <= {}ps for T={}ps, setup={}ps)",
                            ff.name, max_arr, required_ps, period_ps, setup_ps
                        ),
                    };
                    self.violations.push(v.clone());
                    step_violations.push(v);
                    violated_ffs.push(ff.name.clone());
                }
            }

            if let Some(&hold_ps) = self.hold_checks.get(&(ff.name.clone(), "D".to_string())) {
                let slack_ps = (min_arr as i64) - (hold_ps as i64);
                if slack_ps < 0 {
                    let v = TimingViolation {
                        time_ps: next_time_ps,
                        cycle: next_cycle,
                        kind: TimingViolationKind::Hold,
                        instance: ff.name.clone(),
                        pin: "D".to_string(),
                        arrival_ps: min_arr,
                        clock_edge_ps: period_ps,
                        limit_ps: hold_ps,
                        slack_ps,
                        message: format!(
                            "hold violation on {}/D: data arrived at {}ps (required >= {}ps)",
                            ff.name, min_arr, hold_ps
                        ),
                    };
                    self.violations.push(v.clone());
                    step_violations.push(v);
                    violated_ffs.push(ff.name.clone());
                }
            }
        }

        // 5. Inject `Logic4::X` into any flip-flop that suffered a setup/hold violation.
        if self.inject_x_on_violation && !violated_ffs.is_empty() {
            for ff_name in violated_ffs {
                let _ = sim.set_ff_4state(&ff_name, Logic4::X);
            }
            sim.eval_comb_4state();
        }

        step_violations
    }

    /// Compute static worst-case combinational arrival time (in ps) at every FF `D` pin
    /// using the levelized schedule and annotated SDF `IOPATH` + `INTERCONNECT` delays.
    pub fn analyze_static_arrivals(&self, schedule: &LevelizedSchedule) -> HashMap<String, u64> {
        let num_nets = schedule.num_nets();
        let mut net_max_arr = vec![0u64; num_nets];
        let mut net_src_pin: Vec<Option<(String, String)>> = vec![None; num_nets];

        for ff in &schedule.ffs {
            if ff.q_net != NET_UNCONNECTED {
                let clk_q = self
                    .iopath_delay(&ff.name, "CLK", "Q", true)
                    .max(self.iopath_delay(&ff.name, "CLK", "Q", false));
                net_max_arr[ff.q_net as usize] = clk_q;
                net_src_pin[ff.q_net as usize] = Some((ff.name.clone(), "Q".to_string()));
            }
        }

        for node in &schedule.ops {
            match &node.kind {
                ConeNodeKind::Lut6 {
                    inputs, out_net, ..
                } => {
                    if *out_net == NET_UNCONNECTED {
                        continue;
                    }
                    let mut best = 0u64;
                    for (pin_idx, &in_net) in inputs.iter().enumerate() {
                        if in_net < 2 || in_net == NET_UNCONNECTED {
                            continue;
                        }
                        let pin_name = format!("I{pin_idx}");
                        let src = net_src_pin[in_net as usize]
                            .as_ref()
                            .map(|(si, sp)| (si.as_str(), sp.as_str()));
                        let ic = self
                            .interconnect_delay(src, &node.cell_name, &pin_name, true)
                            .max(self.interconnect_delay(src, &node.cell_name, &pin_name, false));
                        let cell_d = self
                            .iopath_delay(&node.cell_name, &pin_name, "O", true)
                            .max(self.iopath_delay(&node.cell_name, &pin_name, "O", false));
                        best = best.max(net_max_arr[in_net as usize] + ic + cell_d);
                    }
                    net_max_arr[*out_net as usize] = best;
                    net_src_pin[*out_net as usize] = Some((node.cell_name.clone(), "O".to_string()));
                }
                _ => {}
            }
        }

        let mut ff_arrivals = HashMap::new();
        for ff in &schedule.ffs {
            if ff.d_net >= 2 && ff.d_net != NET_UNCONNECTED {
                let src = net_src_pin[ff.d_net as usize]
                    .as_ref()
                    .map(|(si, sp)| (si.as_str(), sp.as_str()));
                let ic = self
                    .interconnect_delay(src, &ff.name, "D", true)
                    .max(self.interconnect_delay(src, &ff.name, "D", false));
                ff_arrivals.insert(ff.name.clone(), net_max_arr[ff.d_net as usize] + ic);
            }
        }
        ff_arrivals
    }
}
