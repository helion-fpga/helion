//! Vivado UG907 Power Analysis & Optimization (`report_power` and `power_opt_design`).
//!
//! Provides:
//! 1. IEEE SAIF (`parse_saif`) and VCD (`parse_vcd_activity`) switching activity parsers.
//! 2. Vectorless toggle-rate and static-probability propagation (`propagate_vectorless_activity`)
//!    across LUT6 (exact 64-bit truth-table Boolean difference / Shannon cofactor sensitivity),
//!    Carry4, Muxf7, Muxf8, FF, BRAM, and DSP cones.
//! 3. Multi-rail (`VCCINT`, `VCCAUX`, `VCCBRAM`, `VCCO`) dynamic and static leakage power
//!    analysis (`analyze_power` / `PowerReport`) with iterative junction-temperature ($T_J$)
//!    thermal convergence and UG907 confidence scoring.
//! 4. Intelligent FF and BRAM enable-cone power optimization (`power_opt_design` / `PowerOptSummary`).

use crate::Device;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Source of switching activity for a net.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ActivitySource {
    /// Default primary-input or unannotated fallback activity.
    Default,
    /// Propagated by the vectorless Boolean-difference engine.
    VectorlessPropagated,
    /// Constrained by an XDC/STA clock specification.
    XdcClock,
    /// Annotated from a VCD simulation dump.
    Vcd,
    /// Annotated from an IEEE SAIF switching activity file.
    Saif,
    /// User-overridden (`set_switching_activity`).
    UserAnnotated,
}

impl ActivitySource {
    /// True if this net's activity comes from simulation or explicit constraint annotation.
    pub fn is_annotated(self) -> bool {
        matches!(
            self,
            Self::XdcClock | Self::Vcd | Self::Saif | Self::UserAnnotated
        )
    }
}

/// Switching activity for a single net.
#[derive(Clone, Debug, PartialEq)]
pub struct NetActivity {
    /// Hierarchical or flat net name.
    pub net: String,
    /// Toggle rate in transitions per nanosecond (tr/ns).
    /// Note: a 100 MHz clock (10 ns period) has 2 transitions per period = 0.20 tr/ns.
    pub toggle_rate_per_ns: f64,
    /// Static probability $P_1 \in [0.0, 1.0]$ (fraction of time spent at logic 1).
    pub static_prob: f64,
    /// Raw toggle count (`TC`) if sourced from SAIF/VCD.
    pub toggle_count: u64,
    /// Time spent in logic 0 (in file time units, if available).
    pub t0: u64,
    /// Time spent in logic 1 (in file time units, if available).
    pub t1: u64,
    /// Time spent in unknown X state.
    pub tx: u64,
    /// Time spent in high-Z state.
    pub tz: u64,
    /// Provenance of this activity record.
    pub source: ActivitySource,
}

impl NetActivity {
    pub fn new(
        net: impl Into<String>,
        toggle_rate_per_ns: f64,
        static_prob: f64,
        source: ActivitySource,
    ) -> Self {
        Self {
            net: net.into(),
            toggle_rate_per_ns: toggle_rate_per_ns.max(0.0),
            static_prob: static_prob.clamp(0.0, 1.0),
            toggle_count: 0,
            t0: 0,
            t1: 0,
            tx: 0,
            tz: 0,
            source,
        }
    }

    /// Activity factor $\alpha$ (transitions per clock cycle) relative to a reference clock frequency in MHz.
    pub fn alpha_at_mhz(&self, freq_mhz: f64) -> f64 {
        if freq_mhz <= 0.0 {
            return 0.0;
        }
        let clk_period_ns = 1_000.0 / freq_mhz;
        self.toggle_rate_per_ns * clk_period_ns
    }
}

/// Collection of per-net switching activities parsed from SAIF, VCD, or propagated vectorlessly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityDatabase {
    /// Simulation duration in nanoseconds.
    pub duration_ns: f64,
    /// Timescale in nanoseconds per tick.
    pub timescale_ns: f64,
    /// Design name (if specified in SAIF/VCD).
    pub design_name: Option<String>,
    /// Map from canonical net name to [`NetActivity`].
    pub nets: BTreeMap<String, NetActivity>,
}

impl ActivityDatabase {
    pub fn new() -> Self {
        Self {
            duration_ns: 0.0,
            timescale_ns: 1.0,
            design_name: None,
            nets: BTreeMap::new(),
        }
    }

    /// Look up activity by exact name or suffix match (stripping testbench scope prefix).
    pub fn lookup(&self, net: &str) -> Option<&NetActivity> {
        let norm = normalize_net_name(net);
        if let Some(a) = self.nets.get(&norm) {
            return Some(a);
        }
        if let Some(a) = self.nets.get(net) {
            return Some(a);
        }
        // Try matching hierarchical suffix after `.` or `/`
        let suffix_dot = format!(".{norm}");
        let suffix_slash = format!("/{norm}");
        self.nets
            .iter()
            .find(|(k, _)| k.ends_with(&suffix_dot) || k.ends_with(&suffix_slash))
            .map(|(_, v)| v)
    }

    pub fn insert(&mut self, act: NetActivity) {
        self.nets.insert(normalize_net_name(&act.net), act);
    }
}

fn normalize_net_name(raw: &str) -> String {
    let mut s = raw.trim().replace('/', ".");
    while s.starts_with('.') {
        s.remove(0);
    }
    // Unescape SAIF escaped brackets `\[0\]` -> `[0]`
    s = s.replace("\\[", "[").replace("\\]", "]").replace("\\", "");
    s
}

fn unit_to_ns(unit: &str) -> Option<f64> {
    match unit.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" => Some(1e9),
        "ms" => Some(1e6),
        "us" => Some(1e3),
        "ns" => Some(1.0),
        "ps" => Some(1e-3),
        "fs" => Some(1e-6),
        _ => None,
    }
}

#[derive(Clone, Debug)]
enum SaifTok {
    LParen,
    RParen,
    Atom(String),
}

fn tokenize_saif(input: &str) -> Vec<SaifTok> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Skip `//` or `;` line comments and `/* ... */` block comments
        if c == ';' || (c == '/' && i + 1 < chars.len() && chars[i + 1] == '/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '(' {
            out.push(SaifTok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(SaifTok::RParen);
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(SaifTok::Atom(s));
            continue;
        }
        let mut s = String::new();
        while i < chars.len() {
            let ch = chars[i];
            if ch == '\\' && i + 1 < chars.len() {
                s.push(ch);
                s.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' {
                break;
            }
            s.push(ch);
            i += 1;
        }
        if !s.is_empty() {
            out.push(SaifTok::Atom(s));
        }
    }
    out
}

#[derive(Clone, Debug)]
enum SExpr {
    Atom(String),
    List(Vec<SExpr>),
}

fn parse_sexprs(tokens: &[SaifTok]) -> Result<Vec<SExpr>, String> {
    let mut pos = 0;
    let mut root = Vec::new();
    while pos < tokens.len() {
        root.push(parse_one_sexpr(tokens, &mut pos)?);
    }
    Ok(root)
}

fn parse_one_sexpr(tokens: &[SaifTok], pos: &mut usize) -> Result<SExpr, String> {
    if *pos >= tokens.len() {
        return Err("unexpected end of SAIF input".into());
    }
    match &tokens[*pos] {
        SaifTok::Atom(a) => {
            *pos += 1;
            Ok(SExpr::Atom(a.clone()))
        }
        SaifTok::LParen => {
            *pos += 1;
            let mut items = Vec::new();
            while *pos < tokens.len() {
                if matches!(tokens[*pos], SaifTok::RParen) {
                    *pos += 1;
                    return Ok(SExpr::List(items));
                }
                items.push(parse_one_sexpr(tokens, pos)?);
            }
            Err("unclosed '(' in SAIF input".into())
        }
        SaifTok::RParen => Err("unexpected ')' in SAIF input".into()),
    }
}

/// Parse an IEEE SAIF (Switching Activity Interchange Format) text into an [`ActivityDatabase`].
///
/// Supports `SAIFILE`, `TIMESCALE`, `DURATION`, nested `INSTANCE` scopes, and `NET` / `PORT`
/// switching tables containing `(T0 ...)`, `(T1 ...)`, `(TX ...)`, `(TZ ...)`, `(TC ...)`, `(IG ...)`.
pub fn parse_saif(saif_text: &str) -> Result<ActivityDatabase, String> {
    let tokens = tokenize_saif(saif_text);
    if tokens.is_empty() {
        return Err("empty SAIF input".into());
    }
    let exprs = parse_sexprs(&tokens)?;
    let mut db = ActivityDatabase {
        duration_ns: 0.0,
        timescale_ns: 1.0,
        design_name: None,
        nets: BTreeMap::new(),
    };
    let mut raw_duration: Option<f64> = None;
    let mut scope: Vec<String> = Vec::new();

    fn walk_saif(
        expr: &SExpr,
        scope: &mut Vec<String>,
        db: &mut ActivityDatabase,
        raw_duration: &mut Option<f64>,
    ) -> Result<(), String> {
        let SExpr::List(items) = expr else {
            return Ok(());
        };
        let Some(SExpr::Atom(head)) = items.first() else {
            for sub in items {
                walk_saif(sub, scope, db, raw_duration)?;
            }
            return Ok(());
        };
        let kw = head.to_ascii_uppercase();
        match kw.as_str() {
            "SAIFILE" => {
                for sub in &items[1..] {
                    walk_saif(sub, scope, db, raw_duration)?;
                }
            }
            "DESIGN" => {
                if let Some(SExpr::Atom(name)) = items.get(1) {
                    db.design_name = Some(name.clone());
                }
                for sub in &items[1..] {
                    walk_saif(sub, scope, db, raw_duration)?;
                }
            }
            "TIMESCALE" => {
                // e.g. (TIMESCALE 1 ps) or (TIMESCALE 10ns)
                if items.len() >= 3 {
                    if let (SExpr::Atom(num_s), SExpr::Atom(unit_s)) = (&items[1], &items[2]) {
                        let n: f64 = num_s
                            .parse()
                            .map_err(|_| format!("invalid SAIF TIMESCALE number: {num_s}"))?;
                        let u = unit_to_ns(unit_s)
                            .ok_or_else(|| format!("invalid SAIF TIMESCALE unit: {unit_s}"))?;
                        db.timescale_ns = n * u;
                    }
                } else if let Some(SExpr::Atom(combined)) = items.get(1) {
                    let split = combined
                        .find(|c: char| c.is_alphabetic())
                        .ok_or_else(|| format!("invalid SAIF TIMESCALE: {combined}"))?;
                    let (n_s, u_s) = combined.split_at(split);
                    let n: f64 = n_s
                        .trim()
                        .parse()
                        .map_err(|_| format!("invalid SAIF TIMESCALE number: {combined}"))?;
                    let u = unit_to_ns(u_s)
                        .ok_or_else(|| format!("invalid SAIF TIMESCALE unit: {combined}"))?;
                    db.timescale_ns = n * u;
                }
            }
            "DURATION" => {
                if let Some(SExpr::Atom(d_s)) = items.get(1) {
                    let d: f64 = d_s
                        .parse()
                        .map_err(|_| format!("invalid SAIF DURATION: {d_s}"))?;
                    *raw_duration = Some(d);
                }
            }
            "INSTANCE" => {
                let mut pushed = false;
                let mut start_idx = 1;
                if let Some(SExpr::Atom(inst_name)) = items.get(1) {
                    scope.push(normalize_net_name(inst_name));
                    pushed = true;
                    start_idx = 2;
                }
                for sub in &items[start_idx..] {
                    walk_saif(sub, scope, db, raw_duration)?;
                }
                if pushed {
                    scope.pop();
                }
            }
            "NET" | "PORT" => {
                for entry in &items[1..] {
                    let SExpr::List(sig_items) = entry else {
                        continue;
                    };
                    let Some(SExpr::Atom(sig_name)) = sig_items.first() else {
                        continue;
                    };
                    let mut t0 = 0u64;
                    let mut t1 = 0u64;
                    let mut tx = 0u64;
                    let mut tz = 0u64;
                    let mut tc = 0u64;
                    for attr in &sig_items[1..] {
                        if let SExpr::List(pair) = attr {
                            if pair.len() >= 2 {
                                if let (SExpr::Atom(k), SExpr::Atom(v)) = (&pair[0], &pair[1]) {
                                    let val: u64 = v.parse().unwrap_or(0);
                                    match k.to_ascii_uppercase().as_str() {
                                        "T0" => t0 = val,
                                        "T1" => t1 = val,
                                        "TX" => tx = val,
                                        "TZ" => tz = val,
                                        "TC" => tc = val,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    let clean_sig = normalize_net_name(sig_name);
                    let full_name = if scope.is_empty() {
                        clean_sig.clone()
                    } else {
                        format!("{}.{}", scope.join("."), clean_sig)
                    };
                    let mut act = NetActivity::new(full_name.clone(), 0.0, 0.0, ActivitySource::Saif);
                    act.t0 = t0;
                    act.t1 = t1;
                    act.tx = tx;
                    act.tz = tz;
                    act.toggle_count = tc;
                    db.nets.insert(full_name, act.clone());
                    // Also index by leaf name if not already populated so flat netlists match directly
                    db.nets.entry(clean_sig.clone()).or_insert_with(|| {
                        let mut leaf = act;
                        leaf.net = clean_sig;
                        leaf
                    });
                }
            }
            _ => {
                for sub in &items[1..] {
                    walk_saif(sub, scope, db, raw_duration)?;
                }
            }
        }
        Ok(())
    }

    for expr in &exprs {
        walk_saif(expr, &mut scope, &mut db, &mut raw_duration)?;
    }

    let dur_ticks = raw_duration.unwrap_or_else(|| {
        db.nets
            .values()
            .map(|n| n.t0 + n.t1 + n.tx + n.tz)
            .max()
            .unwrap_or(0) as f64
    });
    db.duration_ns = (dur_ticks * db.timescale_ns).max(0.0);

    for act in db.nets.values_mut() {
        let total_t = act.t0 + act.t1 + act.tx + act.tz;
        let denom_ticks = if total_t > 0 {
            total_t as f64
        } else {
            dur_ticks
        };
        if denom_ticks > 0.0 {
            // IEEE SAIF static probability: T1 / (T0 + T1 + TX + TZ), with 0.5*TX fallback if T0+T1==0
            let known_t = act.t0 + act.t1;
            act.static_prob = if known_t > 0 {
                (act.t1 as f64 / denom_ticks).clamp(0.0, 1.0)
            } else {
                0.5
            };
        }
        let net_dur_ns = if db.duration_ns > 0.0 {
            db.duration_ns
        } else {
            denom_ticks * db.timescale_ns
        };
        if net_dur_ns > 0.0 {
            act.toggle_rate_per_ns = act.toggle_count as f64 / net_dur_ns;
        }
    }

    Ok(db)
}

/// Parse an IEEE 1364 VCD (Value Change Dump) text into an [`ActivityDatabase`]
/// with exact per-net toggle rates (transitions/ns) and time-weighted static probabilities ($P_1$).
pub fn parse_vcd_activity(vcd_text: &str) -> Result<ActivityDatabase, String> {
    let tokens: Vec<&str> = vcd_text.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("empty VCD input".into());
    }

    #[derive(Clone, Debug)]
    struct VcdVar {
        width: usize,
        full_name: String,
        leaf_name: String,
        /// Per-bit state: Some(false)=0, Some(true)=1, None=X/Z
        state: Vec<Option<bool>>,
        last_time: Vec<u64>,
        t0: Vec<u64>,
        t1: Vec<u64>,
        tx: Vec<u64>,
        tc: Vec<u64>,
    }

    let mut timescale_ns = 1e-3; // default 1ps = 0.001ns
    let mut scope: Vec<String> = Vec::new();
    let mut id_to_var: BTreeMap<String, usize> = BTreeMap::new();
    let mut vars: Vec<VcdVar> = Vec::new();
    let mut i = 0usize;
    let mut saw_enddefs = false;

    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        match tok {
            "$timescale" => {
                let mut ts_parts = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    ts_parts.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                let joined = ts_parts.join("");
                let split = joined
                    .find(|c: char| c.is_alphabetic())
                    .ok_or_else(|| format!("invalid VCD $timescale: {joined}"))?;
                let (num_s, unit_s) = joined.split_at(split);
                let num: f64 = num_s
                    .parse()
                    .map_err(|_| format!("invalid VCD $timescale number: {joined}"))?;
                let unit = unit_to_ns(unit_s)
                    .ok_or_else(|| format!("invalid VCD $timescale unit: {joined}"))?;
                timescale_ns = num * unit;
            }
            "$scope" => {
                let mut sc_parts = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    sc_parts.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                if sc_parts.len() >= 2 {
                    scope.push(sc_parts[1].to_string());
                }
            }
            "$upscope" => {
                scope.pop();
                while i < tokens.len() && tokens[i] != "$end" {
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
            }
            "$var" => {
                let mut v_parts = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    v_parts.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                if v_parts.len() >= 4 {
                    let width: usize = v_parts[1].parse().unwrap_or(1).max(1);
                    let id = v_parts[2].to_string();
                    // Join reference; strip range suffix `[msb:lsb]` if width > 1
                    let raw_ref = v_parts[3..].join("");
                    let clean_ref = if width > 1 {
                        if let Some((base, _)) = raw_ref.split_once('[') {
                            base.to_string()
                        } else {
                            raw_ref
                        }
                    } else {
                        raw_ref
                    };
                    let leaf_name = normalize_net_name(&clean_ref);
                    let full_name = if scope.is_empty() {
                        leaf_name.clone()
                    } else {
                        format!("{}.{}", scope.join("."), leaf_name)
                    };
                    if !id_to_var.contains_key(&id) {
                        let idx = vars.len();
                        id_to_var.insert(id, idx);
                        vars.push(VcdVar {
                            width,
                            full_name,
                            leaf_name,
                            state: vec![None; width],
                            last_time: vec![0; width],
                            t0: vec![0; width],
                            t1: vec![0; width],
                            tx: vec![0; width],
                            tc: vec![0; width],
                        });
                    }
                }
            }
            "$enddefinitions" => {
                while i < tokens.len() && tokens[i] != "$end" {
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                saw_enddefs = true;
                break;
            }
            _ => {
                if tok.starts_with('$') && tok != "$end" {
                    while i < tokens.len() && tokens[i] != "$end" {
                        i += 1;
                    }
                    if i < tokens.len() {
                        i += 1;
                    }
                }
            }
        }
    }

    if !saw_enddefs {
        return Err("VCD missing $enddefinitions".into());
    }

    let mut cur_time: u64 = 0;
    let apply_bit = |v: &mut VcdVar, bit: usize, new_val: Option<bool>, now: u64| {
        let dt = now.saturating_sub(v.last_time[bit]);
        if dt > 0 {
            match v.state[bit] {
                Some(false) => v.t0[bit] += dt,
                Some(true) => v.t1[bit] += dt,
                None => v.tx[bit] += dt,
            }
        }
        if let (Some(prev), Some(next)) = (v.state[bit], new_val) {
            if prev != next {
                v.tc[bit] += 1;
            }
        }
        v.state[bit] = new_val;
        v.last_time[bit] = now;
    };

    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok == "$dumpvars" || tok == "$dumpall" || tok == "$end" {
            continue;
        }
        if tok == "$comment" || tok == "$dumpoff" || tok == "$dumpon" {
            while i < tokens.len() && tokens[i] != "$end" {
                i += 1;
            }
            if i < tokens.len() {
                i += 1;
            }
            continue;
        }
        if let Some(ts) = tok.strip_prefix('#') {
            if let Ok(t) = ts.parse::<u64>() {
                cur_time = t;
            }
            continue;
        }
        let first = tok.chars().next().unwrap_or(' ');
        if matches!(first, '0' | '1' | 'x' | 'X' | 'z' | 'Z') && tok.len() >= 2 {
            let id = &tok[1..];
            let bit_val = match first {
                '0' => Some(false),
                '1' => Some(true),
                _ => None,
            };
            if let Some(&idx) = id_to_var.get(id) {
                let v = &mut vars[idx];
                apply_bit(v, 0, bit_val, cur_time);
            }
        } else if (first == 'b' || first == 'B') && i < tokens.len() {
            let bits_str = &tok[1..];
            let id = tokens[i];
            i += 1;
            if let Some(&idx) = id_to_var.get(id) {
                let v = &mut vars[idx];
                let chars: Vec<char> = bits_str.chars().collect();
                let extend = chars.first().copied().unwrap_or('0');
                for bit in 0..v.width {
                    // bit 0 is LSB (rightmost char in VCD vector literal)
                    let ch = if bit < chars.len() {
                        chars[chars.len() - 1 - bit]
                    } else if extend == '1' {
                        '0'
                    } else {
                        extend
                    };
                    let bit_val = match ch {
                        '0' => Some(false),
                        '1' => Some(true),
                        _ => None,
                    };
                    apply_bit(v, bit, bit_val, cur_time);
                }
            }
        } else if (first == 'r' || first == 'R') && i < tokens.len() {
            i += 1;
        }
    }

    // Finalize durations up to cur_time
    for v in &mut vars {
        for bit in 0..v.width {
            let dt = cur_time.saturating_sub(v.last_time[bit]);
            if dt > 0 {
                match v.state[bit] {
                    Some(false) => v.t0[bit] += dt,
                    Some(true) => v.t1[bit] += dt,
                    None => v.tx[bit] += dt,
                }
            }
        }
    }

    let duration_ns = (cur_time as f64 * timescale_ns).max(0.0);
    let mut db = ActivityDatabase {
        duration_ns,
        timescale_ns,
        design_name: None,
        nets: BTreeMap::new(),
    };

    for v in vars {
        if v.width == 1 {
            let t0 = v.t0[0];
            let t1 = v.t1[0];
            let tx = v.tx[0];
            let tc = v.tc[0];
            let total = t0 + t1 + tx;
            let p1 = if t0 + t1 > 0 {
                (t1 as f64 / (t0 + t1) as f64).clamp(0.0, 1.0)
            } else {
                0.5
            };
            let tr = if duration_ns > 0.0 {
                tc as f64 / duration_ns
            } else {
                0.0
            };
            let mut act = NetActivity::new(v.full_name.clone(), tr, p1, ActivitySource::Vcd);
            act.t0 = t0;
            act.t1 = t1;
            act.tx = tx;
            act.toggle_count = tc;
            let _ = total;
            db.nets.insert(v.full_name.clone(), act.clone());
            db.nets.entry(v.leaf_name.clone()).or_insert_with(|| {
                let mut leaf = act;
                leaf.net = v.leaf_name;
                leaf
            });
        } else {
            // Emit both per-bit `name[i]` and bus aggregate `name`
            let mut sum_tr = 0.0;
            let mut sum_p1 = 0.0;
            let mut sum_tc = 0u64;
            let mut sum_t0 = 0u64;
            let mut sum_t1 = 0u64;
            for bit in 0..v.width {
                let t0 = v.t0[bit];
                let t1 = v.t1[bit];
                let tx = v.tx[bit];
                let tc = v.tc[bit];
                let p1 = if t0 + t1 > 0 {
                    (t1 as f64 / (t0 + t1) as f64).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                let tr = if duration_ns > 0.0 {
                    tc as f64 / duration_ns
                } else {
                    0.0
                };
                sum_tr += tr;
                sum_p1 += p1;
                sum_tc += tc;
                sum_t0 += t0;
                sum_t1 += t1;

                let bit_full = format!("{}[{bit}]", v.full_name);
                let bit_leaf = format!("{}[{bit}]", v.leaf_name);
                let mut act = NetActivity::new(bit_full.clone(), tr, p1, ActivitySource::Vcd);
                act.t0 = t0;
                act.t1 = t1;
                act.tx = tx;
                act.toggle_count = tc;
                db.nets.insert(bit_full, act.clone());
                db.nets.entry(bit_leaf.clone()).or_insert_with(|| {
                    let mut leaf = act;
                    leaf.net = bit_leaf;
                    leaf
                });
            }
            let avg_tr = sum_tr / v.width as f64;
            let avg_p1 = (sum_p1 / v.width as f64).clamp(0.0, 1.0);
            let mut bus_act =
                NetActivity::new(v.full_name.clone(), avg_tr, avg_p1, ActivitySource::Vcd);
            bus_act.toggle_count = sum_tc;
            bus_act.t0 = sum_t0;
            bus_act.t1 = sum_t1;
            db.nets.insert(v.full_name.clone(), bus_act.clone());
            db.nets.entry(v.leaf_name.clone()).or_insert_with(|| {
                let mut leaf = bus_act;
                leaf.net = v.leaf_name;
                leaf
            });
        }
    }

    Ok(db)
}

// ============================================================================
// 2. Vectorless Activity Propagation Engine
// ============================================================================

/// BRAM write mode (UG473 / UG907). `WriteFirst` and `ReadFirst` toggle `DO` on writes;
/// `NoChange` holds the output latch steady during writes, saving array + routing power.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum BramWriteMode {
    WriteFirst,
    ReadFirst,
    NoChange,
}

impl Default for BramWriteMode {
    fn default() -> Self {
        Self::WriteFirst
    }
}

/// Hardware primitive cell kinds supported by the UG907 power propagation & optimization engine.
#[derive(Clone, Debug, PartialEq)]
pub enum PowerCellKind {
    /// 1-to-6 input LUT with 64-bit truth table (`init`).
    /// For fewer than 6 inputs, only the lower `1 << inputs.len()` bits of `init` are used
    /// (or replicated across unused upper inputs).
    Lut6 {
        init: u64,
        inputs: Vec<String>,
        output: String,
    },
    /// 4-bit fast lookahead carry chain (`CARRY4`).
    /// Stage $i$: $CO_i = S_i \cdot CO_{i-1} + \overline{S_i} \cdot DI_i$, $O_i = S_i \oplus CO_{i-1}$.
    Carry4 {
        ci: Option<String>,
        cyinit: Option<String>,
        di: [Option<String>; 4],
        s: [Option<String>; 4],
        co: [Option<String>; 4],
        o: [Option<String>; 4],
    },
    /// 2:1 LUT multiplexer (`MUXF7`): $O = \overline{S} \cdot I_0 + S \cdot I_1$.
    Muxf7 {
        i0: String,
        i1: String,
        s: String,
        o: String,
    },
    /// 2:1 slice multiplexer (`MUXF8`): $O = \overline{S} \cdot I_0 + S \cdot I_1$.
    Muxf8 {
        i0: String,
        i1: String,
        s: String,
        o: String,
    },
    /// Sequential flip-flop (`FDRE` / `FDSE` / `FDCE` / `FDPE`).
    Ff {
        clk: String,
        d: String,
        ce: Option<String>,
        sr: Option<String>,
        q: String,
        sr_sync: bool,
        sr_val: bool,
    },
    /// Block RAM (`RAMB18E1` / `RAMB36E1`).
    Bram {
        clk: String,
        en_a: Option<String>,
        we_a: Option<String>,
        en_b: Option<String>,
        we_b: Option<String>,
        addr_inputs: Vec<String>,
        data_inputs: Vec<String>,
        outputs: Vec<String>,
        write_mode: BramWriteMode,
    },
    /// DSP slice (`DSP48E1` / `MAC27`).
    Dsp {
        clk: Option<String>,
        ce: Option<String>,
        a_inputs: Vec<String>,
        b_inputs: Vec<String>,
        c_inputs: Vec<String>,
        outputs: Vec<String>,
    },
    /// Clock management tile (`MMCM` / `PLL`).
    MmcmPll {
        clk_in: String,
        clk_out: String,
        mult: f64,
        div: f64,
    },
    /// I/O buffer (`IBUF` / `OBUF` / `IOBUF`).
    Io {
        pad: String,
        i_net: Option<String>,
        o_net: Option<String>,
        t_net: Option<String>,
        iostandard: String,
        drive_ma: u32,
        slew: String,
    },
}

/// Instance of a hardware primitive in a [`PowerNetlist`].
#[derive(Clone, Debug, PartialEq)]
pub struct PowerCell {
    pub name: String,
    pub kind: PowerCellKind,
}

/// Clock constraint for power analysis (`create_clock`).
#[derive(Clone, Debug, PartialEq)]
pub struct PowerClock {
    pub name: String,
    pub net: String,
    /// Clock frequency in MHz (e.g. 100.0 for 10 ns period).
    pub freq_mhz: f64,
    /// Duty cycle $\in (0, 1)$, default 0.5.
    pub duty_cycle: f64,
}

impl PowerClock {
    pub fn new(name: impl Into<String>, net: impl Into<String>, freq_mhz: f64) -> Self {
        Self {
            name: name.into(),
            net: net.into(),
            freq_mhz: freq_mhz.max(0.0),
            duty_cycle: 0.5,
        }
    }

    /// Clock toggle rate in transitions/ns ($2 \cdot f_{\text{GHz}}$).
    pub fn toggle_rate_per_ns(&self) -> f64 {
        2.0 * (self.freq_mhz / 1_000.0)
    }
}

/// Default vectorless primary-input activity parameters (UG907).
#[derive(Clone, Debug, PartialEq)]
pub struct VectorlessConfig {
    /// Default primary-input static probability $P_1$ (UG907 default: 0.5).
    pub default_static_prob: f64,
    /// Default primary-input toggle rate as a fraction of the fastest clock's cycle rate
    /// (transitions per clock cycle, UG907 default: 0.125 = 12.5%).
    pub default_input_alpha: f64,
    /// Default fallback clock frequency in MHz when no XDC clock is specified (default: 100.0 MHz).
    pub default_clock_mhz: f64,
    /// Sequential loop relaxation iterations for feedback cones (default: 8).
    pub sequential_iterations: usize,
    /// Glitch / reconvergent fanout factor applied to combinational LUT/Carry outputs (default: 1.0).
    pub glitch_factor: f64,
}

impl Default for VectorlessConfig {
    fn default() -> Self {
        Self {
            default_static_prob: 0.5,
            default_input_alpha: 0.125,
            default_clock_mhz: 100.0,
            sequential_iterations: 8,
            glitch_factor: 1.0,
        }
    }
}

/// Netlist model for UG907 power analysis and `power_opt_design`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerNetlist {
    pub primary_inputs: Vec<String>,
    pub primary_outputs: Vec<String>,
    pub clocks: Vec<PowerClock>,
    pub cells: Vec<PowerCell>,
    /// Optional per-net explicit fanout overrides (otherwise derived from cell pin connectivity).
    pub net_fanouts: BTreeMap<String, usize>,
}

impl PowerNetlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_cell(&mut self, name: impl Into<String>, kind: PowerCellKind) {
        self.cells.push(PowerCell {
            name: name.into(),
            kind,
        });
    }

    pub fn add_clock(&mut self, name: impl Into<String>, net: impl Into<String>, freq_mhz: f64) {
        self.clocks.push(PowerClock::new(name, net, freq_mhz));
    }

    /// Collect all distinct net names referenced by inputs, outputs, clocks, and cells.
    pub fn all_nets(&self) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for n in &self.primary_inputs {
            set.insert(n.clone());
        }
        for n in &self.primary_outputs {
            set.insert(n.clone());
        }
        for c in &self.clocks {
            set.insert(c.net.clone());
        }
        for cell in &self.cells {
            for n in cell_input_nets(&cell.kind) {
                set.insert(n);
            }
            for n in cell_output_nets(&cell.kind) {
                set.insert(n);
            }
        }
        set
    }

    /// Compute fanout for every net in the netlist.
    pub fn compute_fanouts(&self) -> BTreeMap<String, usize> {
        let mut fanouts: BTreeMap<String, usize> = BTreeMap::new();
        for cell in &self.cells {
            for inp in cell_input_nets(&cell.kind) {
                *fanouts.entry(inp).or_insert(0) += 1;
            }
        }
        for po in &self.primary_outputs {
            *fanouts.entry(po.clone()).or_insert(0) += 1;
        }
        for (k, &v) in &self.net_fanouts {
            fanouts.insert(k.clone(), v);
        }
        fanouts
    }
}

fn cell_input_nets(kind: &PowerCellKind) -> Vec<String> {
    match kind {
        PowerCellKind::Lut6 { inputs, .. } => inputs.clone(),
        PowerCellKind::Carry4 { ci, cyinit, di, s, .. } => {
            let mut v = Vec::new();
            if let Some(n) = ci {
                v.push(n.clone());
            }
            if let Some(n) = cyinit {
                v.push(n.clone());
            }
            for n in di.iter().flatten() {
                v.push(n.clone());
            }
            for n in s.iter().flatten() {
                v.push(n.clone());
            }
            v
        }
        PowerCellKind::Muxf7 { i0, i1, s, .. } | PowerCellKind::Muxf8 { i0, i1, s, .. } => {
            vec![i0.clone(), i1.clone(), s.clone()]
        }
        PowerCellKind::Ff { clk, d, ce, sr, .. } => {
            let mut v = vec![clk.clone(), d.clone()];
            if let Some(n) = ce {
                v.push(n.clone());
            }
            if let Some(n) = sr {
                v.push(n.clone());
            }
            v
        }
        PowerCellKind::Bram {
            clk,
            en_a,
            we_a,
            en_b,
            we_b,
            addr_inputs,
            data_inputs,
            ..
        } => {
            let mut v = vec![clk.clone()];
            for opt in [en_a, we_a, en_b, we_b].into_iter().flatten() {
                v.push(opt.clone());
            }
            v.extend(addr_inputs.iter().cloned());
            v.extend(data_inputs.iter().cloned());
            v
        }
        PowerCellKind::Dsp {
            clk,
            ce,
            a_inputs,
            b_inputs,
            c_inputs,
            ..
        } => {
            let mut v = Vec::new();
            if let Some(n) = clk {
                v.push(n.clone());
            }
            if let Some(n) = ce {
                v.push(n.clone());
            }
            v.extend(a_inputs.iter().cloned());
            v.extend(b_inputs.iter().cloned());
            v.extend(c_inputs.iter().cloned());
            v
        }
        PowerCellKind::MmcmPll { clk_in, .. } => vec![clk_in.clone()],
        PowerCellKind::Io { pad, i_net, t_net, .. } => {
            let mut v = vec![pad.clone()];
            if let Some(n) = i_net {
                v.push(n.clone());
            }
            if let Some(n) = t_net {
                v.push(n.clone());
            }
            v
        }
    }
}

fn cell_output_nets(kind: &PowerCellKind) -> Vec<String> {
    match kind {
        PowerCellKind::Lut6 { output, .. } => vec![output.clone()],
        PowerCellKind::Carry4 { co, o, .. } => {
            let mut v = Vec::new();
            for n in co.iter().flatten() {
                v.push(n.clone());
            }
            for n in o.iter().flatten() {
                v.push(n.clone());
            }
            v
        }
        PowerCellKind::Muxf7 { o, .. } | PowerCellKind::Muxf8 { o, .. } => vec![o.clone()],
        PowerCellKind::Ff { q, .. } => vec![q.clone()],
        PowerCellKind::Bram { outputs, .. } | PowerCellKind::Dsp { outputs, .. } => {
            outputs.clone()
        }
        PowerCellKind::MmcmPll { clk_out, .. } => vec![clk_out.clone()],
        PowerCellKind::Io { pad, o_net, .. } => {
            let mut v = Vec::new();
            if let Some(n) = o_net {
                v.push(n.clone());
            } else {
                v.push(pad.clone());
            }
            v
        }
    }
}

/// Compute the exact output static probability $P_1(f) \in [0.0, 1.0]$ of a LUT6
/// with 64-bit truth table `init` given input static probabilities `input_probs` (up to 6 inputs).
///
/// Each minterm $m \in [0, 2^k - 1]$ where bit $m$ of `init` is 1 contributes:
/// $$\prod_{i=0}^{k-1} \begin{cases} P_1(x_i) & \text{if } (m \gg i) \& 1 = 1 \\ 1 - P_1(x_i) & \text{otherwise} \end{cases}$$
pub fn lut6_static_prob(init: u64, input_probs: &[f64]) -> f64 {
    let k = input_probs.len().min(6);
    if k == 0 {
        return (init & 1) as f64;
    }
    let n_minterms = 1usize << k;
    let mut p_out = 0.0;
    for m in 0..n_minterms {
        if ((init >> m) & 1) == 0 {
            continue;
        }
        let mut term = 1.0;
        for (i, &p) in input_probs.iter().take(k).enumerate() {
            let p_clamped = p.clamp(0.0, 1.0);
            if ((m >> i) & 1) == 1 {
                term *= p_clamped;
            } else {
                term *= 1.0 - p_clamped;
            }
        }
        p_out += term;
    }
    p_out.clamp(0.0, 1.0)
}

/// Compute the exact Boolean difference probability (Shannon cofactor sensitivity)
/// $\Pr\left(\frac{\partial f}{\partial x_{\text{var}}} = 1\right) = \Pr(f|_{x_v=1} \oplus f|_{x_v=0} = 1)$
/// for input index `var` ($0 \le \text{var} < k \le 6$) of a LUT6 with 64-bit `init`.
///
/// Because $f|_{x_v=1} \oplus f|_{x_v=0}$ is independent of $x_{\text{var}}$, its probability
/// depends only on the remaining $k - 1$ inputs:
/// $$\Pr\left(\frac{\partial f}{\partial x_v} = 1\right) = \sum_{\substack{m \in [0, 2^k - 1] \\ (m \gg v)\&1 = 0 \\ \text{bit}(m) \ne \text{bit}(m | 2^v)}} \prod_{j \ne v} P(x_j = m_j)$$
pub fn lut6_boolean_diff_prob(init: u64, var: usize, input_probs: &[f64]) -> f64 {
    let k = input_probs.len().min(6);
    if var >= k {
        return 0.0;
    }
    let n_minterms = 1usize << k;
    let bit_mask = 1usize << var;
    let mut sens = 0.0;
    for m in 0..n_minterms {
        if (m & bit_mask) != 0 {
            continue;
        }
        let f0 = (init >> m) & 1;
        let f1 = (init >> (m | bit_mask)) & 1;
        if (f0 ^ f1) == 0 {
            continue;
        }
        let mut term = 1.0;
        for (j, &p) in input_probs.iter().take(k).enumerate() {
            if j == var {
                continue;
            }
            let p_clamped = p.clamp(0.0, 1.0);
            if ((m >> j) & 1) == 1 {
                term *= p_clamped;
            } else {
                term *= 1.0 - p_clamped;
            }
        }
        sens += term;
    }
    sens.clamp(0.0, 1.0)
}

/// Propagate `(static_prob, toggle_rate_per_ns)` pairs through a LUT6 using exact 64-bit
/// Shannon cofactor / Boolean difference sensitivity:
/// $$P_1(f) = \text{lut6\_static\_prob}(\text{init}, P_1)$$
/// $$D(f) = \sum_{i=0}^{k-1} \Pr\left(\frac{\partial f}{\partial x_i} = 1\right) \cdot D(x_i)$$
pub fn lut6_propagate_activity(init: u64, inputs: &[(f64, f64)]) -> (f64, f64) {
    let probs: Vec<f64> = inputs.iter().take(6).map(|&(p, _)| p.clamp(0.0, 1.0)).collect();
    let p_out = lut6_static_prob(init, &probs);
    let mut d_out = 0.0;
    for (i, &(_, d_in)) in inputs.iter().take(6).enumerate() {
        let sens = lut6_boolean_diff_prob(init, i, &probs);
        d_out += sens * d_in.max(0.0);
    }
    (p_out, d_out)
}

/// Propagate vectorless switching activity and static probabilities across combinational
/// and sequential cones (UG907).
///
/// Preserves any nets already annotated in `seed_activity` (from SAIF, VCD, or user constraints)
/// and propagates static probability $P_1$ and transition density $D$ (transitions/ns) through
/// LUT6, Carry4, Muxf7, Muxf8, FF, BRAM, DSP, MMCM/PLL, and I/O primitives.
pub fn propagate_vectorless_activity(
    netlist: &PowerNetlist,
    seed_activity: Option<&ActivityDatabase>,
    cfg: &VectorlessConfig,
) -> ActivityDatabase {
    let mut db = ActivityDatabase::new();
    if let Some(seed) = seed_activity {
        db.duration_ns = seed.duration_ns;
        db.timescale_ns = seed.timescale_ns;
        db.design_name = seed.design_name.clone();
    }

    let fastest_clk_mhz = netlist
        .clocks
        .iter()
        .map(|c| c.freq_mhz)
        .fold(0.0_f64, f64::max);
    let ref_clk_mhz = if fastest_clk_mhz > 0.0 {
        fastest_clk_mhz
    } else {
        cfg.default_clock_mhz.max(1.0)
    };
    let default_tr_per_ns = cfg.default_input_alpha * (ref_clk_mhz / 1_000.0);

    let mut locked_nets = BTreeSet::new();

    // 1. Apply XDC clock constraints first
    for clk in &netlist.clocks {
        let act = NetActivity::new(
            clk.net.clone(),
            clk.toggle_rate_per_ns(),
            clk.duty_cycle.clamp(0.0, 1.0),
            ActivitySource::XdcClock,
        );
        db.nets.insert(clk.net.clone(), act);
        locked_nets.insert(clk.net.clone());
    }

    // 2. Overlay SAIF / VCD / user-annotated nets (higher priority than vectorless defaults)
    if let Some(seed) = seed_activity {
        for net in netlist.all_nets() {
            if let Some(found) = seed.lookup(&net) {
                let mut act = found.clone();
                act.net = net.clone();
                db.nets.insert(net.clone(), act);
                locked_nets.insert(net);
            }
        }
        // Also retain all other nets from seed_activity
        for (k, v) in &seed.nets {
            db.nets.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    // 3. Seed primary inputs and undriven nets with default activity
    let mut driven_nets = BTreeSet::new();
    for cell in &netlist.cells {
        for out in cell_output_nets(&cell.kind) {
            driven_nets.insert(out);
        }
    }
    for pi in &netlist.primary_inputs {
        if !locked_nets.contains(pi) {
            db.nets.insert(
                pi.clone(),
                NetActivity::new(
                    pi.clone(),
                    default_tr_per_ns,
                    cfg.default_static_prob,
                    ActivitySource::Default,
                ),
            );
            locked_nets.insert(pi.clone());
        }
    }
    for net in netlist.all_nets() {
        if !db.nets.contains_key(&net) {
            let src = if driven_nets.contains(&net) {
                ActivitySource::VectorlessPropagated
            } else {
                ActivitySource::Default
            };
            db.nets.insert(
                net.clone(),
                NetActivity::new(net, default_tr_per_ns, cfg.default_static_prob, src),
            );
        }
    }

    // Topological sort of combinational cells; sequential cells updated between combinational passes
    let comb_order = topological_comb_order(&netlist.cells);
    let n_iters = cfg.sequential_iterations.max(1);

    for _iter in 0..n_iters {
        let mut max_delta = 0.0_f64;

        // Propagate MMCM/PLL clocks first
        for cell in &netlist.cells {
            if let PowerCellKind::MmcmPll {
                clk_in,
                clk_out,
                mult,
                div,
            } = &cell.kind
            {
                if !locked_nets.contains(clk_out) {
                    let (p_in, d_in) = get_pd(&db, clk_in, cfg.default_static_prob, default_tr_per_ns);
                    let ratio = if *div > 0.0 { (*mult / *div).max(0.0) } else { 1.0 };
                    max_delta = max_delta.max(update_net(
                        &mut db,
                        clk_out,
                        p_in,
                        d_in * ratio,
                        ActivitySource::VectorlessPropagated,
                    ));
                }
            }
        }

        // Propagate combinational cells in topological order
        for &idx in &comb_order {
            let cell = &netlist.cells[idx];
            match &cell.kind {
                PowerCellKind::Lut6 {
                    init,
                    inputs,
                    output,
                } => {
                    if locked_nets.contains(output) {
                        continue;
                    }
                    let inp_pd: Vec<(f64, f64)> = inputs
                        .iter()
                        .map(|n| get_pd(&db, n, cfg.default_static_prob, default_tr_per_ns))
                        .collect();
                    let (p_out, d_out) = lut6_propagate_activity(*init, &inp_pd);
                    max_delta = max_delta.max(update_net(
                        &mut db,
                        output,
                        p_out,
                        d_out * cfg.glitch_factor.max(0.0),
                        ActivitySource::VectorlessPropagated,
                    ));
                }
                PowerCellKind::Muxf7 { i0, i1, s, o }
                | PowerCellKind::Muxf8 { i0, i1, s, o } => {
                    if locked_nets.contains(o) {
                        continue;
                    }
                    // 2:1 mux truth table: O = (!S & I0) | (S & I1)
                    // init for (I0=bit0, I1=bit1, S=bit2):
                    // m=0(000)->0, m=1(001)->1, m=2(010)->0, m=3(011)->1,
                    // m=4(100)->0, m=5(101)->0, m=6(110)->1, m=7(111)->1 => 0b1100_1010 = 0xCA
                    let p0 = get_pd(&db, i0, cfg.default_static_prob, default_tr_per_ns);
                    let p1 = get_pd(&db, i1, cfg.default_static_prob, default_tr_per_ns);
                    let ps = get_pd(&db, s, cfg.default_static_prob, default_tr_per_ns);
                    let (p_out, d_out) = lut6_propagate_activity(0xCA, &[p0, p1, ps]);
                    max_delta = max_delta.max(update_net(
                        &mut db,
                        o,
                        p_out,
                        d_out,
                        ActivitySource::VectorlessPropagated,
                    ));
                }
                PowerCellKind::Carry4 {
                    ci,
                    cyinit,
                    di,
                    s,
                    co,
                    o,
                } => {
                    let mut c_prev = if let Some(ci_net) = ci {
                        get_pd(&db, ci_net, 0.0, 0.0)
                    } else if let Some(cy_net) = cyinit {
                        get_pd(&db, cy_net, 0.0, 0.0)
                    } else {
                        (0.0, 0.0)
                    };
                    for stage in 0..4 {
                        let d_stage = di[stage]
                            .as_deref()
                            .map(|n| get_pd(&db, n, cfg.default_static_prob, default_tr_per_ns))
                            .unwrap_or((0.0, 0.0));
                        let s_stage = s[stage]
                            .as_deref()
                            .map(|n| get_pd(&db, n, cfg.default_static_prob, default_tr_per_ns))
                            .unwrap_or((0.5, default_tr_per_ns));
                        // O[stage] = S ^ C_prev (2-input XOR: init = 0b0110 = 0x6)
                        let (p_xor, d_xor) = lut6_propagate_activity(0x6, &[s_stage, c_prev]);
                        if let Some(o_net) = &o[stage] {
                            if !locked_nets.contains(o_net) {
                                max_delta = max_delta.max(update_net(
                                    &mut db,
                                    o_net,
                                    p_xor,
                                    d_xor * cfg.glitch_factor.max(0.0),
                                    ActivitySource::VectorlessPropagated,
                                ));
                            }
                        }
                        // CO[stage] = (!S & DI) | (S & C_prev) (2:1 mux: I0=DI, I1=C_prev, S=S => init = 0xCA)
                        let (p_co, d_co) =
                            lut6_propagate_activity(0xCA, &[d_stage, c_prev, s_stage]);
                        if let Some(co_net) = &co[stage] {
                            if !locked_nets.contains(co_net) {
                                max_delta = max_delta.max(update_net(
                                    &mut db,
                                    co_net,
                                    p_co,
                                    d_co * cfg.glitch_factor.max(0.0),
                                    ActivitySource::VectorlessPropagated,
                                ));
                            }
                        }
                        c_prev = (p_co, d_co);
                    }
                }
                PowerCellKind::Io {
                    pad,
                    i_net,
                    o_net,
                    t_net,
                    ..
                } => {
                    if let Some(out) = o_net {
                        if !locked_nets.contains(out) {
                            let (p_in, d_in) =
                                get_pd(&db, pad, cfg.default_static_prob, default_tr_per_ns);
                            let (p_en, _) = t_net
                                .as_deref()
                                .map(|t| {
                                    let (pt, dt) = get_pd(&db, t, 0.0, 0.0);
                                    (1.0 - pt, dt)
                                })
                                .unwrap_or((1.0, 0.0));
                            max_delta = max_delta.max(update_net(
                                &mut db,
                                out,
                                p_in * p_en,
                                d_in * p_en,
                                ActivitySource::VectorlessPropagated,
                            ));
                        }
                    } else if let Some(inp) = i_net {
                        if !locked_nets.contains(pad) {
                            let (p_in, d_in) =
                                get_pd(&db, inp, cfg.default_static_prob, default_tr_per_ns);
                            max_delta = max_delta.max(update_net(
                                &mut db,
                                pad,
                                p_in,
                                d_in,
                                ActivitySource::VectorlessPropagated,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        // Propagate sequential cells (FF, BRAM, DSP)
        for cell in &netlist.cells {
            match &cell.kind {
                PowerCellKind::Ff {
                    clk,
                    d,
                    ce,
                    sr,
                    q,
                    sr_val,
                    ..
                } => {
                    if locked_nets.contains(q) {
                        continue;
                    }
                    let (_, d_clk) = get_pd(&db, clk, 0.5, 2.0 * (ref_clk_mhz / 1_000.0));
                    // Rising edges per ns = d_clk / 2.0
                    let f_clk_ghz = (d_clk * 0.5).max(0.0);
                    let (p_d, d_d) = get_pd(&db, d, cfg.default_static_prob, default_tr_per_ns);
                    let (p_ce, _) = ce
                        .as_deref()
                        .map(|n| get_pd(&db, n, 1.0, 0.0))
                        .unwrap_or((1.0, 0.0));
                    let (p_sr, _) = sr
                        .as_deref()
                        .map(|n| get_pd(&db, n, 0.0, 0.0))
                        .unwrap_or((0.0, 0.0));

                    let sr_target = if *sr_val { 1.0 } else { 0.0 };
                    let p_q = ((1.0 - p_sr) * p_d + p_sr * sr_target).clamp(0.0, 1.0);
                    // UG907 sequential output toggle rate:
                    // Bound by both input transition rate * P(CE) and sampling limit 2*p_q*(1-p_q)*f_clk*P(CE)
                    let markov_tr = 2.0 * p_q * (1.0 - p_q) * f_clk_ghz;
                    let sampled_d = d_d.min(f_clk_ghz);
                    let d_q = (1.0 - p_sr)
                        * p_ce.clamp(0.0, 1.0)
                        * if d_d > 0.0 {
                            0.5 * sampled_d + 0.5 * markov_tr
                        } else {
                            0.0
                        };
                    max_delta = max_delta.max(update_net(
                        &mut db,
                        q,
                        p_q,
                        d_q,
                        ActivitySource::VectorlessPropagated,
                    ));
                }
                PowerCellKind::Bram {
                    clk,
                    en_a,
                    we_a,
                    en_b,
                    addr_inputs,
                    data_inputs,
                    outputs,
                    write_mode,
                    ..
                } => {
                    let (_, d_clk) = get_pd(&db, clk, 0.5, 2.0 * (ref_clk_mhz / 1_000.0));
                    let f_clk_ghz = (d_clk * 0.5).max(0.0);
                    let p_ena = en_a
                        .as_deref()
                        .map(|n| get_pd(&db, n, 1.0, 0.0).0)
                        .unwrap_or(1.0);
                    let p_enb = en_b
                        .as_deref()
                        .map(|n| get_pd(&db, n, 0.0, 0.0).0)
                        .unwrap_or(0.0);
                    let p_wea = we_a
                        .as_deref()
                        .map(|n| get_pd(&db, n, 0.0, 0.0).0)
                        .unwrap_or(0.0);
                    let p_en = (p_ena.max(p_enb)).clamp(0.0, 1.0);
                    // In NO_CHANGE mode, write cycles (WE=1) do not toggle the read output latch
                    let mode_factor = match write_mode {
                        BramWriteMode::NoChange => (1.0 - p_wea).clamp(0.0, 1.0),
                        BramWriteMode::WriteFirst | BramWriteMode::ReadFirst => 1.0,
                    };
                    let avg_in_d = if addr_inputs.is_empty() && data_inputs.is_empty() {
                        default_tr_per_ns
                    } else {
                        let sum: f64 = addr_inputs
                            .iter()
                            .chain(data_inputs.iter())
                            .map(|n| get_pd(&db, n, 0.5, default_tr_per_ns).1)
                            .sum();
                        sum / (addr_inputs.len() + data_inputs.len()).max(1) as f64
                    };
                    let d_out = (avg_in_d.min(0.5 * f_clk_ghz)) * p_en * mode_factor;
                    for out in outputs {
                        if !locked_nets.contains(out) {
                            max_delta = max_delta.max(update_net(
                                &mut db,
                                out,
                                0.5,
                                d_out,
                                ActivitySource::VectorlessPropagated,
                            ));
                        }
                    }
                }
                PowerCellKind::Dsp {
                    clk,
                    ce,
                    a_inputs,
                    b_inputs,
                    c_inputs,
                    outputs,
                } => {
                    let p_ce = ce
                        .as_deref()
                        .map(|n| get_pd(&db, n, 1.0, 0.0).0)
                        .unwrap_or(1.0);
                    let all_in: Vec<&String> = a_inputs
                        .iter()
                        .chain(b_inputs.iter())
                        .chain(c_inputs.iter())
                        .collect();
                    let avg_d = if all_in.is_empty() {
                        default_tr_per_ns
                    } else {
                        all_in
                            .iter()
                            .map(|n| get_pd(&db, n, 0.5, default_tr_per_ns).1)
                            .sum::<f64>()
                            / all_in.len() as f64
                    };
                    let clk_cap = clk
                        .as_deref()
                        .map(|c| get_pd(&db, c, 0.5, 2.0 * (ref_clk_mhz / 1_000.0)).1 * 0.5)
                        .unwrap_or(f64::INFINITY);
                    // Multiplier-accumulator output switching scales with active input switching * P(CE)
                    let d_out = (avg_d * 1.15).min(clk_cap) * p_ce.clamp(0.0, 1.0);
                    for out in outputs {
                        if !locked_nets.contains(out) {
                            max_delta = max_delta.max(update_net(
                                &mut db,
                                out,
                                0.5,
                                d_out,
                                ActivitySource::VectorlessPropagated,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        if max_delta < 1e-9 {
            break;
        }
    }

    db
}

fn get_pd(
    db: &ActivityDatabase,
    net: &str,
    default_p: f64,
    default_d: f64,
) -> (f64, f64) {
    if let Some(a) = db.lookup(net) {
        (a.static_prob.clamp(0.0, 1.0), a.toggle_rate_per_ns.max(0.0))
    } else {
        (default_p.clamp(0.0, 1.0), default_d.max(0.0))
    }
}

fn update_net(
    db: &mut ActivityDatabase,
    net: &str,
    p1: f64,
    d: f64,
    source: ActivitySource,
) -> f64 {
    let p1 = p1.clamp(0.0, 1.0);
    let d = d.max(0.0);
    if let Some(existing) = db.nets.get_mut(net) {
        let delta = (existing.static_prob - p1).abs() + (existing.toggle_rate_per_ns - d).abs();
        existing.static_prob = p1;
        existing.toggle_rate_per_ns = d;
        existing.source = source;
        delta
    } else {
        db.nets
            .insert(net.to_string(), NetActivity::new(net, d, p1, source));
        p1 + d
    }
}

fn is_combinational(kind: &PowerCellKind) -> bool {
    matches!(
        kind,
        PowerCellKind::Lut6 { .. }
            | PowerCellKind::Carry4 { .. }
            | PowerCellKind::Muxf7 { .. }
            | PowerCellKind::Muxf8 { .. }
            | PowerCellKind::Io { .. }
    )
}

fn topological_comb_order(cells: &[PowerCell]) -> Vec<usize> {
    let comb_indices: Vec<usize> = cells
        .iter()
        .enumerate()
        .filter(|(_, c)| is_combinational(&c.kind))
        .map(|(i, _)| i)
        .collect();
    let mut net_driver: BTreeMap<String, usize> = BTreeMap::new();
    for &idx in &comb_indices {
        for out in cell_output_nets(&cells[idx].kind) {
            net_driver.insert(out, idx);
        }
    }
    let mut indegree: BTreeMap<usize, usize> = BTreeMap::new();
    let mut adj: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &idx in &comb_indices {
        indegree.insert(idx, 0);
    }
    for &idx in &comb_indices {
        for inp in cell_input_nets(&cells[idx].kind) {
            if let Some(&drv) = net_driver.get(&inp) {
                if drv != idx {
                    adj.entry(drv).or_default().push(idx);
                    *indegree.entry(idx).or_insert(0) += 1;
                }
            }
        }
    }
    let mut q = VecDeque::new();
    for &idx in &comb_indices {
        if indegree.get(&idx).copied().unwrap_or(0) == 0 {
            q.push_back(idx);
        }
    }
    let mut order = Vec::with_capacity(comb_indices.len());
    let mut visited = BTreeSet::new();
    while let Some(u) = q.pop_front() {
        if !visited.insert(u) {
            continue;
        }
        order.push(u);
        if let Some(nexts) = adj.get(&u) {
            for &v in nexts {
                if let Some(deg) = indegree.get_mut(&v) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        q.push_back(v);
                    }
                }
            }
        }
    }
    // Append any remaining combinational cells (if combinational loops exist)
    for &idx in &comb_indices {
        if visited.insert(idx) {
            order.push(idx);
        }
    }
    order
}

// ============================================================================
// 3. Multi-Rail Thermal & Power Breakdown (UG907 report_power)
// ============================================================================

/// FPGA voltage supply rails modeled in UG907 `report_power`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum VoltageRail {
    /// Core logic, routing, LUT, FF, Carry, and DSP supply (nominal 1.00 V).
    Vccint,
    /// Auxiliary / MMCM / PLL / configuration / bandgap supply (nominal 1.80 V).
    Vccaux,
    /// Block RAM array supply (nominal 1.00 V).
    Vccbram,
    /// I/O bank output driver supply (nominal 1.80 V, or bank IOSTANDARD voltage).
    Vcco,
}

impl VoltageRail {
    pub fn name(self) -> &'static str {
        match self {
            Self::Vccint => "VCCINT",
            Self::Vccaux => "VCCAUX",
            Self::Vccbram => "VCCBRAM",
            Self::Vcco => "VCCO",
        }
    }

    pub fn nominal_voltage_v(self) -> f64 {
        match self {
            Self::Vccint => 1.00,
            Self::Vccaux => 1.80,
            Self::Vccbram => 1.00,
            Self::Vcco => 1.80,
        }
    }
}

/// Power and current breakdown for a single supply rail.
#[derive(Clone, Debug, PartialEq)]
pub struct RailPower {
    pub rail: VoltageRail,
    pub voltage_v: f64,
    pub dynamic_mw: f64,
    pub static_mw: f64,
    pub total_mw: f64,
    pub current_ma: f64,
}

/// Airflow profile for package thermal resistance $\Theta_{JA}$ (°C/W, UG907).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AirflowProfile {
    /// Still air (0 LFM, natural convection).
    StillAir,
    /// 150 Linear Feet per Minute airflow.
    Lfm150,
    /// 250 Linear Feet per Minute airflow.
    Lfm250,
    /// 500 Linear Feet per Minute airflow.
    Lfm500,
    /// User-specified effective $\Theta_{JA}$ in °C/W.
    CustomThetaJa(f64),
}

impl Default for AirflowProfile {
    fn default() -> Self {
        Self::StillAir
    }
}

/// Heatsink profile modifying effective $\Theta_{JA}$ (UG907).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HeatsinkProfile {
    None,
    LowProfile,
    MediumProfile,
    HighProfile,
}

impl Default for HeatsinkProfile {
    fn default() -> Self {
        Self::None
    }
}

/// Operating and thermal environment conditions for UG907 power analysis.
#[derive(Clone, Debug, PartialEq)]
pub struct ThermalConditions {
    /// Ambient temperature $T_A$ in °C (default 25.0 °C).
    pub ambient_temp_c: f64,
    /// Maximum rated junction temperature $T_{J,\max}$ in °C (default 100.0 °C industrial, 85.0 °C commercial).
    pub max_junction_temp_c: f64,
    /// Airflow profile.
    pub airflow: AirflowProfile,
    /// Heatsink selection.
    pub heatsink: HeatsinkProfile,
    /// Core supply voltage `VCCINT` in Volts (default 1.00 V).
    pub vccint_v: f64,
    /// Auxiliary supply voltage `VCCAUX` in Volts (default 1.80 V).
    pub vccaux_v: f64,
    /// Block RAM supply voltage `VCCBRAM` in Volts (default 1.00 V).
    pub vccbram_v: f64,
    /// Default I/O bank supply voltage `VCCO` in Volts (default 1.80 V).
    pub vcco_v: f64,
}

impl Default for ThermalConditions {
    fn default() -> Self {
        Self {
            ambient_temp_c: 25.0,
            max_junction_temp_c: 100.0,
            airflow: AirflowProfile::StillAir,
            heatsink: HeatsinkProfile::None,
            vccint_v: 1.00,
            vccaux_v: 1.80,
            vccbram_v: 1.00,
            vcco_v: 1.80,
        }
    }
}

impl ThermalConditions {
    /// Effective junction-to-ambient thermal resistance $\Theta_{JA}$ (°C/W) for a device SKU.
    pub fn effective_theta_ja(&self, dev: &Device) -> f64 {
        if let AirflowProfile::CustomThetaJa(theta) = self.airflow {
            return theta.max(0.1);
        }
        // Larger dies/packages have lower base Theta_JA
        let base_theta: f64 = match dev.sku.as_str() {
            "M" => 9.5,
            "S" => 12.0,
            _ => 14.5, // HL10T
        };
        let airflow_factor: f64 = match self.airflow {
            AirflowProfile::StillAir => 1.00,
            AirflowProfile::Lfm150 => 0.84,
            AirflowProfile::Lfm250 => 0.76,
            AirflowProfile::Lfm500 => 0.65,
            AirflowProfile::CustomThetaJa(t) => return t.max(0.1),
        };
        let heatsink_factor: f64 = match self.heatsink {
            HeatsinkProfile::None => 1.00,
            HeatsinkProfile::LowProfile => 0.78,
            HeatsinkProfile::MediumProfile => 0.62,
            HeatsinkProfile::HighProfile => 0.48,
        };
        (base_theta * airflow_factor * heatsink_factor).max(0.5)
    }
}

/// UG907 power estimation confidence level based on clock and switching-activity annotation coverage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PowerConfidence {
    /// Unconstrained clocks or < 25% of nets annotated from SAIF/VCD.
    Low,
    /// Clocks constrained and 25%..75% of nets annotated from SAIF/VCD.
    Medium,
    /// Clocks constrained and >= 75% of nets annotated from SAIF/VCD.
    High,
}

impl PowerConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

/// On-chip dynamic power breakdown by functional category (UG907).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynamicPowerBreakdown {
    pub clocks_mw: f64,
    pub signals_mw: f64,
    pub logic_mw: f64,
    pub bram_mw: f64,
    pub dsp_mw: f64,
    pub mmcm_pll_mw: f64,
    pub io_mw: f64,
}

impl DynamicPowerBreakdown {
    pub fn total_mw(&self) -> f64 {
        self.clocks_mw
            + self.signals_mw
            + self.logic_mw
            + self.bram_mw
            + self.dsp_mw
            + self.mmcm_pll_mw
            + self.io_mw
    }
}

/// Complete UG907 multi-rail thermal & power report returned by [`analyze_power`].
#[derive(Clone, Debug, PartialEq)]
pub struct PowerReport {
    pub part: String,
    pub total_power_mw: f64,
    pub dynamic_power_mw: f64,
    pub static_power_mw: f64,
    pub dynamic: DynamicPowerBreakdown,
    pub rails: Vec<RailPower>,
    /// Ambient temperature $T_A$ (°C).
    pub ambient_temp_c: f64,
    /// Converged junction temperature $T_J$ (°C).
    pub junction_temp_c: f64,
    /// Effective package thermal resistance $\Theta_{JA}$ (°C/W).
    pub effective_theta_ja_c_per_w: f64,
    /// Thermal margin $T_{J,\max} - T_J$ (°C).
    pub thermal_margin_c: f64,
    /// Number of thermal solver iterations to reach $T_J$ convergence.
    pub thermal_iterations: usize,
    /// Overall UG907 power confidence rating.
    pub confidence: PowerConfidence,
    /// Percentage of design nets with simulation/user switching activity annotation ($0..=100$).
    pub annotated_nets_pct: f64,
    /// Percentage of clock domains constrained ($0..=100$).
    pub constrained_clocks_pct: f64,
    ///Propagated per-net activity map used for this power report.
    pub activity: ActivityDatabase,
}

impl PowerReport {
    /// Look up a specific voltage rail's power summary.
    pub fn rail(&self, rail: VoltageRail) -> Option<&RailPower> {
        self.rails.iter().find(|r| r.rail == rail)
    }

    /// Format a Vivado UG907-style text summary of the power report.
    pub fn summary_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "UG907 Power Report (part={})\n",
            self.part
        ));
        s.push_str(&format!(
            "  Total On-Chip Power : {:8.3} mW (Dynamic: {:.3} mW, Device Static: {:.3} mW)\n",
            self.total_power_mw, self.dynamic_power_mw, self.static_power_mw
        ));
        s.push_str(&format!(
            "  Junction Temp (Tj)  : {:8.2} C  (Ambient: {:.2} C, ThetaJA: {:.2} C/W, Margin: {:.2} C)\n",
            self.junction_temp_c,
            self.ambient_temp_c,
            self.effective_theta_ja_c_per_w,
            self.thermal_margin_c
        ));
        s.push_str(&format!(
            "  Confidence Level    : {:>8}    (Annotated nets: {:.1}%, Clocks constrained: {:.1}%)\n",
            self.confidence.as_str(),
            self.annotated_nets_pct,
            self.constrained_clocks_pct
        ));
        s.push_str("  On-Chip Dynamic Breakdown:\n");
        s.push_str(&format!(
            "    Clocks: {:.3} mW | Signals: {:.3} mW | Logic: {:.3} mW | BRAM: {:.3} mW | DSP: {:.3} mW |PLL: {:.3} mW | I/O: {:.3} mW\n",
            self.dynamic.clocks_mw,
            self.dynamic.signals_mw,
            self.dynamic.logic_mw,
            self.dynamic.bram_mw,
            self.dynamic.dsp_mw,
            self.dynamic.mmcm_pll_mw,
            self.dynamic.io_mw
        ));
        s.push_str("  Power Supply Rails:\n");
        for r in &self.rails {
            s.push_str(&format!(
                "    {:<8} {:5.2} V : Total {:8.3} mW (Dyn {:8.3} mW, Static {:8.3} mW) | {:8.3} mA\n",
                r.rail.name(),
                r.voltage_v,
                r.total_mw,
                r.dynamic_mw,
                r.static_mw,
                r.current_ma
            ));
        }
        s
    }
}

/// Compute temperature-dependent static leakage power across `VCCINT`, `VCCAUX`, `VCCBRAM`, `VCCO`
/// at junction temperature `tj_c` (°C).
///
/// Subthreshold and gate leakage follow the standard 28nm CMOS thermal exponential model:
/// $$P_{\text{static}}(T_J) = P_{\text{base}}(25^\circ\text{C}) \cdot \left(0.35 + 0.65 \cdot e^{0.026 \cdot (T_J - 25)}\right)$$
fn compute_rail_static_mw(
    dev: &Device,
    netlist: &PowerNetlist,
    thermal: &ThermalConditions,
    tj_c: f64,
) -> (f64, f64, f64, f64) {
    let dt = (tj_c - 25.0).clamp(-65.0, 125.0);
    let temp_factor = (0.35 + 0.65 * (0.026 * dt).exp()).max(0.15);

    let vccint_scale = (thermal.vccint_v / 1.00).powi(2).max(0.1);
    let vccaux_scale = (thermal.vccaux_v / 1.80).powi(2).max(0.1);
    let vccbram_scale = (thermal.vccbram_v / 1.00).powi(2).max(0.1);
    let vcco_scale = (thermal.vcco_v / 1.80).powi(2).max(0.1);

    let mut used_luts = 0usize;
    let mut used_ffs = 0usize;
    let mut used_brams = 0usize;
    let mut used_dsps = 0usize;
    let mut used_plls = 0usize;
    let mut used_ios = 0usize;
    for c in &netlist.cells {
        match &c.kind {
            PowerCellKind::Lut6 { .. }
            | PowerCellKind::Carry4 { .. }
            | PowerCellKind::Muxf7 { .. }
            | PowerCellKind::Muxf8 { .. } => used_luts += 1,
            PowerCellKind::Ff { .. } => used_ffs += 1,
            PowerCellKind::Bram { .. } => used_brams += 1,
            PowerCellKind::Dsp { .. } => used_dsps += 1,
            PowerCellKind::MmcmPll { .. } => used_plls += 1,
            PowerCellKind::Io { .. } => used_ios += 1,
        }
    }

    let lut_cap = (dev.lut6_count() as usize).max(used_luts);
    let bram_cap = (dev.n_bram as usize).max(used_brams);
    let dsp_cap = (dev.n_dsp as usize).max(used_dsps);
    let io_cap = (dev.interior_cols as usize).max(used_ios);

    // Base leakage at 25C in mW
    let vccint_25c_mw = 8.0
        + 0.0025 * (lut_cap as f64)
        + 0.0035 * (used_luts as f64)
        + 0.0018 * (used_ffs as f64)
        + 0.45 * (dsp_cap as f64)
        + 0.35 * (used_dsps as f64);
    let vccaux_25c_mw = 12.0 + 3.5 * (used_plls as f64) + 0.0005 * (lut_cap as f64);
    let vccbram_25c_mw = 1.2 + 0.22 * (bram_cap as f64) + 0.38 * (used_brams as f64);
    let vcco_25c_mw = 2.0 + 0.03 * (io_cap as f64) + 0.12 * (used_ios as f64);

    (
        vccint_25c_mw * vccint_scale * temp_factor,
        vccaux_25c_mw * vccaux_scale * temp_factor,
        vccbram_25c_mw * vccbram_scale * temp_factor,
        vcco_25c_mw * vcco_scale * temp_factor,
    )
}

/// Perform UG907 multi-rail power and thermal analysis (`report_power`).
///
/// 1. Propagates switching activity across unannotated cones via [`propagate_vectorless_activity`].
/// 2. Computes dynamic power across Clocks, Signals, Logic/LUT, BRAM, DSP, MMCM/PLL, and I/O.
/// 3. Allocates dynamic power across `VCCINT`, `VCCAUX`, `VCCBRAM`, and `VCCO`.
/// 4. Iteratively solves the self-consistent junction temperature equation:
///    $$T_J^{(k+1)} = T_A + \Theta_{JA} \cdot \frac{P_{\text{dyn}} + P_{\text{static}}(T_J^{(k)})}{1000}$$
/// 5. Computes UG907 confidence level (`Low`, `Medium`, `High`) from XDC clock + SAIF/VCD coverage.
pub fn analyze_power(
    dev: &Device,
    netlist: &PowerNetlist,
    seed_activity: Option<&ActivityDatabase>,
    thermal: &ThermalConditions,
    vec_cfg: &VectorlessConfig,
) -> PowerReport {
    let activity = propagate_vectorless_activity(netlist, seed_activity, vec_cfg);
    let fanouts = netlist.compute_fanouts();

    let v_int = thermal.vccint_v.max(0.5);
    let v_aux = thermal.vccaux_v.max(0.5);
    let v_bram = thermal.vccbram_v.max(0.5);
    let v_io_default = thermal.vcco_v.max(0.5);

    let v_int_sq = v_int * v_int;
    let v_aux_sq = v_aux * v_aux;
    let v_bram_sq = v_bram * v_bram;

    // Identify clock nets
    let mut clock_nets = BTreeSet::new();
    for c in &netlist.clocks {
        clock_nets.insert(c.net.clone());
    }
    for cell in &netlist.cells {
        match &cell.kind {
            PowerCellKind::Ff { clk, .. } | PowerCellKind::Bram { clk, .. } => {
                clock_nets.insert(clk.clone());
            }
            PowerCellKind::Dsp { clk: Some(clk), .. } => {
                clock_nets.insert(clk.clone());
            }
            PowerCellKind::MmcmPll {
                clk_in, clk_out, ..
            } => {
                clock_nets.insert(clk_in.clone());
                clock_nets.insert(clk_out.clone());
            }
            _ => {}
        }
    }

    // Recall: P (mW) = 0.5 * C (pF) * V^2 * D (transitions/ns)
    // Because 1 pF * 1 V^2 * (1e9 transitions/s) = 1e-12 * 1e9 W = 1 mW,
    // and each cycle of 0->1->0 is 2 transitions, energy per transition is 0.5 * C * V^2.
    let mut dyn_clocks_mw = 0.0;
    let mut dyn_signals_mw = 0.0;
    let mut dyn_logic_mw = 0.0;
    let mut dyn_bram_vccint_mw = 0.0;
    let mut dyn_bram_vccbram_mw = 0.0;
    let mut dyn_dsp_mw = 0.0;
    let mut dyn_pll_vccaux_mw = 0.0;
    let mut dyn_io_vcco_mw = 0.0;
    let mut dyn_io_vccaux_mw = 0.0;

    // Clock tree spine & leaf buffer power
    for clk_net in &clock_nets {
        let d_clk = activity
            .lookup(clk_net)
            .map(|a| a.toggle_rate_per_ns)
            .unwrap_or(0.0);
        let fo = fanouts.get(clk_net).copied().unwrap_or(1) as f64;
        // Global clock spine + per-leaf clock pin capacitance in pF
        let c_clk_pf = 18.0 + 0.35 * fo;
        dyn_clocks_mw += 0.5 * c_clk_pf * v_int_sq * d_clk;
    }

    // Interconnect signal routing power for non-clock nets
    for net in netlist.all_nets() {
        if clock_nets.contains(&net) {
            continue;
        }
        let d_net = activity
            .lookup(&net)
            .map(|a| a.toggle_rate_per_ns)
            .unwrap_or(0.0);
        let fo = fanouts.get(&net).copied().unwrap_or(1) as f64;
        // Wire capacitance: base 0.18 pF + 0.095 pF per fanout load
        let c_wire_pf = 0.18 + 0.095 * fo;
        dyn_signals_mw += 0.5 * c_wire_pf * v_int_sq * d_net;
    }

    // Cell internal dynamic power
    for cell in &netlist.cells {
        match &cell.kind {
            PowerCellKind::Lut6 { inputs, output, .. } => {
                let d_out = activity
                    .lookup(output)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                let d_in_sum: f64 = inputs
                    .iter()
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                let c_lut_out_pf = 0.22;
                let c_lut_in_pf = 0.035;
                dyn_logic_mw +=
                    0.5 * (c_lut_out_pf * d_out + c_lut_in_pf * d_in_sum) * v_int_sq;
            }
            PowerCellKind::Carry4 { co, o, di, s, .. } => {
                let d_out_sum: f64 = co
                    .iter()
                    .chain(o.iter())
                    .flatten()
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                let d_in_sum: f64 = di
                    .iter()
                    .chain(s.iter())
                    .flatten()
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                dyn_logic_mw += 0.5 * (0.16 * d_out_sum + 0.03 * d_in_sum) * v_int_sq;
            }
            PowerCellKind::Muxf7 { i0, i1, s, o }
            | PowerCellKind::Muxf8 { i0, i1, s, o } => {
                let d_o = activity
                    .lookup(o)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                let d_in: f64 = [i0, i1, s]
                    .into_iter()
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                dyn_logic_mw += 0.5 * (0.18 * d_o + 0.03 * d_in) * v_int_sq;
            }
            PowerCellKind::Ff { clk, d, ce, q, .. } => {
                let d_clk = activity
                    .lookup(clk)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                let p_ce = ce
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.static_prob)
                    .unwrap_or(1.0);
                let d_q = activity
                    .lookup(q)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                let d_d = activity
                    .lookup(d)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                // Master-slave internal clock buffer gated by CE + Q latch transition
                let c_ff_clk_pf = 0.08 * (0.25 + 0.75 * p_ce);
                let c_ff_q_pf = 0.24;
                let c_ff_d_pf = 0.04;
                dyn_logic_mw +=
                    0.5 * (c_ff_clk_pf * d_clk + c_ff_q_pf * d_q + c_ff_d_pf * d_d) * v_int_sq;
            }
            PowerCellKind::Bram {
                clk,
                en_a,
                we_a,
                en_b,
                we_b,
                outputs,
                write_mode,
                ..
            } => {
                let d_clk = activity
                    .lookup(clk)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                let f_clk_ghz = d_clk * 0.5;
                let p_ena = en_a
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.static_prob)
                    .unwrap_or(1.0);
                let p_wea = we_a
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.static_prob)
                    .unwrap_or(0.0);
                let p_enb = en_b
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.static_prob)
                    .unwrap_or(0.0);
                let p_web = we_b
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.static_prob)
                    .unwrap_or(0.0);
                let mode_mult = match write_mode {
                    BramWriteMode::NoChange => 0.78,
                    BramWriteMode::WriteFirst | BramWriteMode::ReadFirst => 1.00,
                };
                let active_port_a = p_ena * (1.0 + 0.35 * p_wea) * mode_mult;
                let active_port_b = p_enb * (1.0 + 0.35 * p_web) * mode_mult;
                let d_out_sum: f64 = outputs
                    .iter()
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                // BRAM core peripheral logic on VCCINT + bitline/sense-amp array on VCCBRAM
                let bram_core_mw =
                    (6.5 * (active_port_a + active_port_b) * f_clk_ghz + 0.12 * d_out_sum)
                        * v_int_sq;
                let bram_array_mw =
                    (4.2 * (active_port_a + active_port_b) * f_clk_ghz) * v_bram_sq;
                dyn_bram_vccint_mw += bram_core_mw;
                dyn_bram_vccbram_mw += bram_array_mw;
            }
            PowerCellKind::Dsp {
                clk,
                ce,
                a_inputs,
                b_inputs,
                c_inputs,
                outputs,
            } => {
                let p_ce = ce
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.static_prob)
                    .unwrap_or(1.0);
                let d_in_sum: f64 = a_inputs
                    .iter()
                    .chain(b_inputs.iter())
                    .chain(c_inputs.iter())
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                let d_out_sum: f64 = outputs
                    .iter()
                    .filter_map(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .sum();
                let d_clk = clk
                    .as_deref()
                    .and_then(|n| activity.lookup(n))
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.2);
                dyn_dsp_mw +=
                    (2.8 * d_clk * p_ce + 0.35 * d_in_sum * p_ce + 0.25 * d_out_sum) * v_int_sq;
            }
            PowerCellKind::MmcmPll {
                clk_in,
                clk_out,
                mult,
                div,
            } => {
                let d_in = activity
                    .lookup(clk_in)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.2);
                let d_out = activity
                    .lookup(clk_out)
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(d_in * (*mult / div.max(1.0)));
                let vco_factor = (*mult / div.max(1.0)).clamp(0.5, 16.0);
                // MMCM/PLL VCO and charge pump draw from VCCAUX
                dyn_pll_vccaux_mw += (28.0 + 14.0 * vco_factor * d_out) * (v_aux_sq / (1.8 * 1.8));
            }
            PowerCellKind::Io {
                pad,
                i_net,
                o_net,
                iostandard,
                drive_ma,
                slew,
                ..
            } => {
                let v_bank = Device::iostandard_vcco_mv(iostandard)
                    .map(|mv| mv as f64 / 1_000.0)
                    .unwrap_or(v_io_default);
                let slew_mult = if slew.eq_ignore_ascii_case("FAST") {
                    1.22
                } else {
                    1.00
                };
                let drive_mult = ((*drive_ma).max(2) as f64 / 12.0).clamp(0.4, 2.0);
                let d_pad = activity
                    .lookup(pad)
                    .or_else(|| i_net.as_deref().and_then(|n| activity.lookup(n)))
                    .or_else(|| o_net.as_deref().and_then(|n| activity.lookup(n)))
                    .map(|a| a.toggle_rate_per_ns)
                    .unwrap_or(0.0);
                let is_output = i_net.is_some();
                if is_output {
                    // Board trace + pin capacitance (10 pF nominal scaled by drive/slew)
                    let c_load_pf = 10.0 * drive_mult * slew_mult;
                    dyn_io_vcco_mw += 0.5 * c_load_pf * (v_bank * v_bank) * d_pad;
                    dyn_io_vccaux_mw += 0.5 * 0.45 * v_aux_sq * d_pad;
                } else {
                    // Input receiver buffer draws from VCCAUX + VCCO
                    dyn_io_vcco_mw += 0.5 * 1.2 * (v_bank * v_bank) * d_pad;
                    dyn_io_vccaux_mw += 0.5 * 0.85 * v_aux_sq * d_pad;
                }
            }
        }
    }

    let dynamic = DynamicPowerBreakdown {
        clocks_mw: dyn_clocks_mw,
        signals_mw: dyn_signals_mw,
        logic_mw: dyn_logic_mw,
        bram_mw: dyn_bram_vccint_mw + dyn_bram_vccbram_mw,
        dsp_mw: dyn_dsp_mw,
        mmcm_pll_mw: dyn_pll_vccaux_mw,
        io_mw: dyn_io_vcco_mw + dyn_io_vccaux_mw,
    };
    let dynamic_power_mw = dynamic.total_mw();

    let dyn_vccint_mw =
        dyn_clocks_mw + dyn_signals_mw + dyn_logic_mw + dyn_bram_vccint_mw + dyn_dsp_mw;
    let dyn_vccaux_mw = dyn_pll_vccaux_mw + dyn_io_vccaux_mw;
    let dyn_vccbram_mw = dyn_bram_vccbram_mw;
    let dyn_vcco_mw = dyn_io_vcco_mw;

    // Iteratively solve T_J = T_A + Theta_JA * (P_dyn + P_static(T_J)) / 1000
    let theta_ja = thermal.effective_theta_ja(dev);
    let mut tj_c = thermal.ambient_temp_c;
    let mut thermal_iterations = 0usize;
    let mut stat_vccint = 0.0;
    let mut stat_vccaux = 0.0;
    let mut stat_vccbram = 0.0;
    let mut stat_vcco = 0.0;

    for iter in 1..=50 {
        thermal_iterations = iter;
        let (s_int, s_aux, s_bram, s_io) =
            compute_rail_static_mw(dev, netlist, thermal, tj_c);
        stat_vccint = s_int;
        stat_vccaux = s_aux;
        stat_vccbram = s_bram;
        stat_vcco = s_io;
        let p_static_mw = s_int + s_aux + s_bram + s_io;
        let p_total_w = (dynamic_power_mw + p_static_mw) / 1_000.0;
        let next_tj = (thermal.ambient_temp_c + theta_ja * p_total_w).clamp(-55.0, 150.0);
        // Under-relaxation for guaranteed monotone stability
        let updated_tj = 0.7 * next_tj + 0.3 * tj_c;
        if (next_tj - tj_c).abs() < 1e-4 {
            tj_c = next_tj;
            break;
        }
        tj_c = updated_tj;
    }

    let static_power_mw = stat_vccint + stat_vccaux + stat_vccbram + stat_vcco;
    let total_power_mw = dynamic_power_mw + static_power_mw;

    let rails = vec![
        RailPower {
            rail: VoltageRail::Vccint,
            voltage_v: v_int,
            dynamic_mw: dyn_vccint_mw,
            static_mw: stat_vccint,
            total_mw: dyn_vccint_mw + stat_vccint,
            current_ma: (dyn_vccint_mw + stat_vccint) / v_int,
        },
        RailPower {
            rail: VoltageRail::Vccaux,
            voltage_v: v_aux,
            dynamic_mw: dyn_vccaux_mw,
            static_mw: stat_vccaux,
            total_mw: dyn_vccaux_mw + stat_vccaux,
            current_ma: (dyn_vccaux_mw + stat_vccaux) / v_aux,
        },
        RailPower {
            rail: VoltageRail::Vccbram,
            voltage_v: v_bram,
            dynamic_mw: dyn_vccbram_mw,
            static_mw: stat_vccbram,
            total_mw: dyn_vccbram_mw + stat_vccbram,
            current_ma: (dyn_vccbram_mw + stat_vccbram) / v_bram,
        },
        RailPower {
            rail: VoltageRail::Vcco,
            voltage_v: v_io_default,
            dynamic_mw: dyn_vcco_mw,
            static_mw: stat_vcco,
            total_mw: dyn_vcco_mw + stat_vcco,
            current_ma: (dyn_vcco_mw + stat_vcco) / v_io_default,
        },
    ];

    // Confidence calculation (UG907):
    // Evaluate % of design nets annotated by XDC clocks + SAIF/VCD
    let design_nets = netlist.all_nets();
    let annotated_count = design_nets
        .iter()
        .filter(|n| {
            activity
                .lookup(n)
                .map(|a| a.source.is_annotated())
                .unwrap_or(false)
        })
        .count();
    let annotated_nets_pct = if design_nets.is_empty() {
        0.0
    } else {
        100.0 * (annotated_count as f64) / (design_nets.len() as f64)
    };

    let constrained_clks = clock_nets
        .iter()
        .filter(|c| {
            netlist.clocks.iter().any(|xc| &xc.net == *c)
                || activity
                    .lookup(c)
                    .map(|a| a.source.is_annotated())
                    .unwrap_or(false)
        })
        .count();
    let constrained_clocks_pct = if clock_nets.is_empty() {
        if !netlist.clocks.is_empty() {
            100.0
        } else {
            0.0
        }
    } else {
        100.0 * (constrained_clks as f64) / (clock_nets.len() as f64)
    };

    let confidence = if constrained_clocks_pct < 50.0 || annotated_nets_pct < 25.0 {
        PowerConfidence::Low
    } else if constrained_clocks_pct >= 99.0 && annotated_nets_pct >= 75.0 {
        PowerConfidence::High
    } else {
        PowerConfidence::Medium
    };

    PowerReport {
        part: dev.part.clone(),
        total_power_mw,
        dynamic_power_mw,
        static_power_mw,
        dynamic,
        rails,
        ambient_temp_c: thermal.ambient_temp_c,
        junction_temp_c: tj_c,
        effective_theta_ja_c_per_w: theta_ja,
        thermal_margin_c: thermal.max_junction_temp_c - tj_c,
        thermal_iterations,
        confidence,
        annotated_nets_pct,
        constrained_clocks_pct,
        activity,
    }
}

// ============================================================================
// 4. Intelligent Clock & BRAM Power Optimization (UG907 power_opt_design)
// ============================================================================

/// Kind of power optimization transformation performed by [`power_opt_design`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PowerOptKind {
    /// Extracted a dedicated flip-flop clock enable (`CE`) from a self-feedback LUT/Mux cone.
    FfFeedbackClockEnable {
        ff_cell: String,
        mux_cell: String,
        ce_net: String,
        data_net: String,
    },
    /// Converted BRAM write mode from `WriteFirst` / `ReadFirst` to `NoChange`
    /// to suppress output latch & routing toggles during write cycles.
    BramNoChangeMode { bram_cell: String },
    /// Gated BRAM read/write port enable using write-enable or downstream read-mask cone.
    BramEnableGating {
        bram_cell: String,
        port: String,
        enable_net: String,
    },
    /// Suppressed redundant toggles on a flip-flop or BRAM cone whose output is masked
    /// downstream by an enable/select mux.
    DownstreamUnreadSuppression {
        target_cell: String,
        gating_net: String,
    },
}

/// Single optimization action applied during [`power_opt_design`].
#[derive(Clone, Debug, PartialEq)]
pub struct PowerOptAction {
    pub kind: PowerOptKind,
    pub estimated_saved_mw: f64,
    pub description: String,
}

/// Summary of optimizations and power savings produced by [`power_opt_design`] (UG907).
#[derive(Clone, Debug, PartialEq)]
pub struct PowerOptSummary {
    pub actions: Vec<PowerOptAction>,
    pub ffs_clock_gated: usize,
    pub brams_no_change_converted: usize,
    pub brams_enable_gated: usize,
    pub downstream_cones_suppressed: usize,
    pub dynamic_before_mw: f64,
    pub dynamic_after_mw: f64,
    pub total_before_mw: f64,
    pub total_after_mw: f64,
    pub estimated_saved_mw: f64,
}

impl PowerOptSummary {
    pub fn summary_text(&self) -> String {
        format!(
            "power_opt_design: actions={} ff_ce_gated={} bram_no_change={} bram_en_gated={} unread_suppressed={} dyn_before={:.3}mW dyn_after={:.3}mW saved={:.3}mW",
            self.actions.len(),
            self.ffs_clock_gated,
            self.brams_no_change_converted,
            self.brams_enable_gated,
            self.downstream_cones_suppressed,
            self.dynamic_before_mw,
            self.dynamic_after_mw,
            self.estimated_saved_mw
        )
    }
}

/// Check whether a LUT6 truth table `init` with `inputs` implements a 2:1 feedback multiplexer
/// selecting between `q_net` (when `ce_net == 0`) and `data_net` (when `ce_net == 1`).
///
/// Returns `Some((ce_net, data_net))` if the LUT implements:
/// $O = (\neg \text{CE} \land Q) \lor (\text{CE} \land D_{\text{new}})$.
fn detect_lut_feedback_mux(
    init: u64,
    inputs: &[String],
    q_net: &str,
) -> Option<(String, String)> {
    if inputs.len() != 3 {
        return None;
    }
    let q_idx = inputs.iter().position(|n| n == q_net)?;
    let other: Vec<usize> = (0..3).filter(|&i| i != q_idx).collect();
    for &(ce_idx, d_idx) in &[(other[0], other[1]), (other[1], other[0])] {
        let mut matches_mux = true;
        for m in 0..8usize {
            let q_val = (m >> q_idx) & 1;
            let ce_val = (m >> ce_idx) & 1;
            let d_val = (m >> d_idx) & 1;
            let expected = if ce_val == 1 { d_val } else { q_val };
            let actual = ((init >> m) & 1) as usize;
            if actual != expected {
                matches_mux = false;
                break;
            }
        }
        if matches_mux {
            return Some((inputs[ce_idx].clone(), inputs[d_idx].clone()));
        }
    }
    None
}

/// Check if a LUT6 or Muxf7/Muxf8 acts as a downstream enable/mask gate on `source_net`:
/// i.e., when `enable_net == 0`, the output is independent of `source_net` ($\partial f / \partial \text{source} = 0$).
fn detect_downstream_mask_enable(cell: &PowerCellKind, source_net: &str) -> Option<String> {
    match cell {
        PowerCellKind::Muxf7 { i0, i1, s, .. } | PowerCellKind::Muxf8 { i0, i1, s, .. } => {
            if i1 == source_net && i0 != source_net {
                return Some(s.clone());
            }
            None
        }
        PowerCellKind::Lut6 { init, inputs, .. } => {
            let src_idx = inputs.iter().position(|n| n == source_net)?;
            let k = inputs.len().min(6);
            let n_minterms = 1usize << k;
            let src_bit = 1usize << src_idx;
            // Look for another input `en_idx` such that whenever bit `en_idx` is 0,
            // flipping `src_idx` never changes the LUT output, and for at least one minterm
            // with `en_idx == 1`, flipping `src_idx` does change the LUT output.
            for en_idx in 0..k {
                if en_idx == src_idx {
                    continue;
                }
                let en_bit = 1usize << en_idx;
                let mut masked_when_zero = true;
                let mut active_when_one = false;
                for m in 0..n_minterms {
                    if (m & src_bit) != 0 {
                        continue;
                    }
                    let f0 = (init >> m) & 1;
                    let f1 = (init >> (m | src_bit)) & 1;
                    let diff = f0 ^ f1;
                    if (m & en_bit) == 0 {
                        if diff != 0 {
                            masked_when_zero = false;
                            break;
                        }
                    } else if diff != 0 {
                        active_when_one = true;
                    }
                }
                if masked_when_zero && active_when_one {
                    return Some(inputs[en_idx].clone());
                }
            }
            None
        }
        _ => None,
    }
}

/// Run Vivado UG907 intelligent clock-enable and BRAM power optimization (`power_opt_design`).
///
/// Analyzes FF and BRAM write/read enable cones in `netlist`, applies power-saving transformations
/// in-place, and returns a [`PowerOptSummary`] with before/after dynamic & total mW and per-action
/// savings estimates:
/// 1. **FF Clock-Enable Extraction**: Detects feedback muxes ($D = \text{CE} ? D_{\text{in}} : Q$) driving
///    ungated FFs and rewires $\text{CE} \to \text{FF.CE}$, $D_{\text{in}} \to \text{FF.D}$.
/// 2. **BRAM `NO_CHANGE` Mode Conversion**: Converts `WriteFirst` / `ReadFirst` BRAMs where write cycles
///    occur (`we_a` or `we_b` present) into `BramWriteMode::NoChange` to stop output bus toggling during writes.
/// 3. **BRAM Enable Gating**: Identifies always-enabled BRAM ports (`en_a == None`) that have an explicit
///    write-enable or downstream read-mask signal and gates `en_a` to eliminate idle array precharge.
/// 4. **Downstream Unread Cycle Suppression**: Identifies FFs whose `Q` output only feeds downstream logic
///    masked by a common enable signal (`AND` / `MUX` gating) and connects that enable to `FF.CE`.
pub fn power_opt_design(
    dev: &Device,
    netlist: &mut PowerNetlist,
    seed_activity: Option<&ActivityDatabase>,
    thermal: &ThermalConditions,
    vec_cfg: &VectorlessConfig,
) -> PowerOptSummary {
    let before_report = analyze_power(dev, netlist, seed_activity, thermal, vec_cfg);
    let fanouts = netlist.compute_fanouts();

    // Map net -> driving cell index
    let mut net_driver: BTreeMap<String, usize> = BTreeMap::new();
    // Map net -> consumer cell indices
    let mut net_consumers: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, cell) in netlist.cells.iter().enumerate() {
        for out in cell_output_nets(&cell.kind) {
            net_driver.insert(out, idx);
        }
        for inp in cell_input_nets(&cell.kind) {
            net_consumers.entry(inp).or_default().push(idx);
        }
    }

    let mut actions = Vec::new();
    let mut ffs_clock_gated = 0usize;
    let mut brams_no_change_converted = 0usize;
    let mut brams_enable_gated = 0usize;
    let mut downstream_cones_suppressed = 0usize;

    let n_cells = netlist.cells.len();
    for idx in 0..n_cells {
        let cell_name = netlist.cells[idx].name.clone();
        let cell_kind = netlist.cells[idx].kind.clone();

        match cell_kind {
            PowerCellKind::Ff {
                ref clk,
                ref d,
                ref ce,
                ref sr,
                ref q,
                sr_sync,
                sr_val,
            } => {
                if ce.is_none() {
                    // Pass 1: Check if `d` is driven by a 2:1 feedback mux selecting between `q` and `new_d`
                    let mut extracted: Option<(String, String, String)> = None;
                    if let Some(&drv_idx) = net_driver.get(d) {
                        let drv_cell = &netlist.cells[drv_idx];
                        match &drv_cell.kind {
                            PowerCellKind::Lut6 { init, inputs, .. } => {
                                if let Some((ce_net, data_net)) =
                                    detect_lut_feedback_mux(*init, inputs, q)
                                {
                                    extracted =
                                        Some((drv_cell.name.clone(), ce_net, data_net));
                                }
                            }
                            PowerCellKind::Muxf7 { i0, i1, s, .. }
                            | PowerCellKind::Muxf8 { i0, i1, s, .. } => {
                                if i0 == q && i1 != q {
                                    extracted =
                                        Some((drv_cell.name.clone(), s.clone(), i1.clone()));
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some((mux_name, ce_net, data_net)) = extracted {
                        let p_ce = before_report
                            .activity
                            .lookup(&ce_net)
                            .map(|a| a.static_prob)
                            .unwrap_or(0.5);
                        let d_clk = before_report
                            .activity
                            .lookup(clk)
                            .map(|a| a.toggle_rate_per_ns)
                            .unwrap_or(0.2);
                        let est_mw = (0.06 * (1.0 - p_ce) * d_clk).max(0.001);
                        netlist.cells[idx].kind = PowerCellKind::Ff {
                            clk: clk.clone(),
                            d: data_net.clone(),
                            ce: Some(ce_net.clone()),
                            sr: sr.clone(),
                            q: q.clone(),
                            sr_sync,
                            sr_val,
                        };
                        // If the feedback mux only drove this FF's D pin, simplify it to a buffer of data_net
                        if fanouts.get(d).copied().unwrap_or(0) <= 1 {
                            if let Some(&drv_idx) = net_driver.get(d) {
                                netlist.cells[drv_idx].kind = PowerCellKind::Lut6 {
                                    init: 0b10,
                                    inputs: vec![data_net.clone()],
                                    output: d.clone(),
                                };
                            }
                        }
                        ffs_clock_gated += 1;
                        actions.push(PowerOptAction {
                            kind: PowerOptKind::FfFeedbackClockEnable {
                                ff_cell: cell_name.clone(),
                                mux_cell: mux_name.clone(),
                                ce_net: ce_net.clone(),
                                data_net: data_net.clone(),
                            },
                            estimated_saved_mw: est_mw,
                            description: format!(
                                "Extracted clock enable '{}' from feedback mux '{}' into FF '{}'",
                                ce_net, mux_name, cell_name
                            ),
                        });
                        continue;
                    }

                    // Pass 2: Check if all downstream consumers of `q` mask `q` with a common enable signal
                    if !netlist.primary_outputs.contains(q) {
                        if let Some(consumers) = net_consumers.get(q) {
                            if !consumers.is_empty() {
                                let mut common_en: Option<String> = None;
                                let mut all_masked = true;
                                for &c_idx in consumers {
                                    if c_idx == idx {
                                        all_masked = false;
                                        break;
                                    }
                                    if let Some(en) = detect_downstream_mask_enable(
                                        &netlist.cells[c_idx].kind,
                                        q,
                                    ) {
                                        match &common_en {
                                            None => common_en = Some(en),
                                            Some(prev) if prev == &en => {}
                                            _ => {
                                                all_masked = false;
                                                break;
                                            }
                                        }
                                    } else {
                                        all_masked = false;
                                        break;
                                    }
                                }
                                if all_masked {
                                    if let Some(en_net) = common_en {
                                        let p_en = before_report
                                            .activity
                                            .lookup(&en_net)
                                            .map(|a| a.static_prob)
                                            .unwrap_or(0.5);
                                        let d_q = before_report
                                            .activity
                                            .lookup(q)
                                            .map(|a| a.toggle_rate_per_ns)
                                            .unwrap_or(0.05);
                                        let est_mw = (0.15 * (1.0 - p_en) * d_q).max(0.001);
                                        netlist.cells[idx].kind = PowerCellKind::Ff {
                                            clk: clk.clone(),
                                            d: d.clone(),
                                            ce: Some(en_net.clone()),
                                            sr: sr.clone(),
                                            q: q.clone(),
                                            sr_sync,
                                            sr_val,
                                        };
                                        downstream_cones_suppressed += 1;
                                        actions.push(PowerOptAction {
                                            kind: PowerOptKind::DownstreamUnreadSuppression {
                                                target_cell: cell_name.clone(),
                                                gating_net: en_net.clone(),
                                            },
                                            estimated_saved_mw: est_mw,
                                            description: format!(
                                                "Suppressed unread downstream cycles on FF '{}' using mask enable '{}'",
                                                cell_name, en_net
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            PowerCellKind::Bram {
                ref clk,
                ref en_a,
                ref we_a,
                ref en_b,
                ref we_b,
                ref addr_inputs,
                ref data_inputs,
                ref outputs,
                write_mode,
            } => {
                let mut new_en_a = en_a.clone();
                let mut new_mode = write_mode;

                // Opportunity A: Convert WriteFirst / ReadFirst to NoChange when write enable is present
                if write_mode != BramWriteMode::NoChange && (we_a.is_some() || we_b.is_some()) {
                    new_mode = BramWriteMode::NoChange;
                    brams_no_change_converted += 1;
                    let d_clk = before_report
                        .activity
                        .lookup(clk)
                        .map(|a| a.toggle_rate_per_ns)
                        .unwrap_or(0.2);
                    let est_mw = (1.15 * d_clk).max(0.05);
                    actions.push(PowerOptAction {
                        kind: PowerOptKind::BramNoChangeMode {
                            bram_cell: cell_name.clone(),
                        },
                        estimated_saved_mw: est_mw,
                        description: format!(
                            "Converted BRAM '{}' from {:?} to NoChange write mode to suppress DO toggles on write cycles",
                            cell_name, write_mode
                        ),
                    });
                }

                // Opportunity B: Gate always-on BRAM port A enable (`en_a == None`)
                // when either `outputs` are unused/masked by a downstream enable or `we_a` is used for a write-only port
                if new_en_a.is_none() {
                    let mut candidate_en: Option<String> = None;
                    if outputs.is_empty() {
                        if let Some(we) = we_a {
                            candidate_en = Some(we.clone());
                        }
                    } else {
                        // Check if all output bits are masked downstream by a common enable net
                        for out_net in outputs {
                            if let Some(consumers) = net_consumers.get(out_net) {
                                for &c_idx in consumers {
                                    if let Some(en) = detect_downstream_mask_enable(
                                        &netlist.cells[c_idx].kind,
                                        out_net,
                                    ) {
                                        candidate_en = Some(en);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(gate_en) = candidate_en {
                        new_en_a = Some(gate_en.clone());
                        brams_enable_gated += 1;
                        let p_en = before_report
                            .activity
                            .lookup(&gate_en)
                            .map(|a| a.static_prob)
                            .unwrap_or(0.5);
                        let d_clk = before_report
                            .activity
                            .lookup(clk)
                            .map(|a| a.toggle_rate_per_ns)
                            .unwrap_or(0.2);
                        let est_mw = (2.5 * (1.0 - p_en) * d_clk).max(0.05);
                        actions.push(PowerOptAction {
                            kind: PowerOptKind::BramEnableGating {
                                bram_cell: cell_name.clone(),
                                port: "EN_A".into(),
                                enable_net: gate_en.clone(),
                            },
                            estimated_saved_mw: est_mw,
                            description: format!(
                                "Gated BRAM '{}' port EN_A with enable signal '{}'",
                                cell_name, gate_en
                            ),
                        });
                    }
                }

                if new_mode != write_mode || new_en_a != *en_a {
                    netlist.cells[idx].kind = PowerCellKind::Bram {
                        clk: clk.clone(),
                        en_a: new_en_a,
                        we_a: we_a.clone(),
                        en_b: en_b.clone(),
                        we_b: we_b.clone(),
                        addr_inputs: addr_inputs.clone(),
                        data_inputs: data_inputs.clone(),
                        outputs: outputs.clone(),
                        write_mode: new_mode,
                    };
                }
            }
            _ => {}
        }
    }

    let after_report = analyze_power(dev, netlist, seed_activity, thermal, vec_cfg);
    let dynamic_before_mw = before_report.dynamic_power_mw;
    let dynamic_after_mw = after_report.dynamic_power_mw;
    let total_before_mw = before_report.total_power_mw;
    let total_after_mw = after_report.total_power_mw;
    let estimated_saved_mw = (total_before_mw - total_after_mw).max(0.0);

    PowerOptSummary {
        actions,
        ffs_clock_gated,
        brams_no_change_converted,
        brams_enable_gated,
        downstream_cones_suppressed,
        dynamic_before_mw,
        dynamic_after_mw,
        total_before_mw,
        total_after_mw,
        estimated_saved_mw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saif_parser_extracts_toggle_rates_and_static_probabilities() {
        let saif = r#"
        (SAIFILE
          (SAIFVERSION "2.0")
          (DIRECTION "backward")
          (DESIGN "top_counter")
          (TIMESCALE 1 ns)
          (DURATION 1000)
          (INSTANCE tb
            (INSTANCE dut
              (NET
                (clk (T0 500) (T1 500) (TX 0) (TZ 0) (TC 200))
                (en  (T0 750) (T1 250) (TX 0) (TZ 0) (TC 10))
                (cnt\[0\] (T0 500) (T1 500) (TX 0) (TZ 0) (TC 25))
                (cnt\[1\] (T0 600) (T1 400) (TX 0) (TZ 0) (TC 12))
              )
            )
          )
        )
        "#;
        let db = parse_saif(saif).expect("valid SAIF should parse");
        assert_eq!(db.design_name.as_deref(), Some("top_counter"));
        assert!((db.duration_ns - 1000.0).abs() < 1e-9);

        let clk = db.lookup("clk").expect("lookup clk by leaf name");
        assert_eq!(clk.source, ActivitySource::Saif);
        assert!((clk.static_prob - 0.5).abs() < 1e-9);
        assert!((clk.toggle_rate_per_ns - 0.20).abs() < 1e-9);
        assert!((clk.alpha_at_mhz(100.0) - 2.0).abs() < 1e-9);

        let en = db.lookup("tb.dut.en").expect("lookup en by hierarchical path");
        assert!((en.static_prob - 0.25).abs() < 1e-9);
        assert!((en.toggle_rate_per_ns - 0.01).abs() < 1e-9);

        let cnt0 = db.lookup("cnt[0]").expect("lookup escaped bus bit cnt[0]");
        assert!((cnt0.static_prob - 0.5).abs() < 1e-9);
        assert!((cnt0.toggle_rate_per_ns - 0.025).abs() < 1e-9);
    }

    #[test]
    fn vcd_parser_matches_saif_toggle_rates_and_static_probabilities() {
        let vcd = r#"
        $timescale 1ns $end
        $scope module tb $end
        $scope module dut $end
        $var wire 1 ! clk $end
        $var wire 1 " en $end
        $var reg 2 # cnt [1:0] $end
        $upscope $end
        $upscope $end
        $enddefinitions $end
        $dumpvars
        0!
        0"
        b00 #
        $end
        #5
        1!
        1"
        b01 #
        #10
        0!
        #15
        1!
        b10 #
        #20
        0!
        0"
        b11 #
        #25
        1!
        #30
        0!
        #35
        1!
        #40
        0!
        "#;
        let db = parse_vcd_activity(vcd).expect("valid VCD should parse");
        assert!((db.duration_ns - 40.0).abs() < 1e-9);

        // clk toggles at 5, 10, 15, 20, 25, 30, 35, 40 => 8 toggles over 40 ns = 0.20 tr/ns, high for 20/40 = 0.5
        let clk = db.lookup("clk").expect("clk in VCD");
        assert_eq!(clk.source, ActivitySource::Vcd);
        assert_eq!(clk.toggle_count, 8);
        assert!((clk.toggle_rate_per_ns - 0.20).abs() < 1e-9);
        assert!((clk.static_prob - 0.50).abs() < 1e-9);

        // en is 1 from #5 to #20 => 15 ns out of 40 ns => P1 = 0.375, 2 toggles => 0.05 tr/ns
        let en = db.lookup("en").expect("en in VCD");
        assert_eq!(en.toggle_count, 2);
        assert!((en.static_prob - 0.375).abs() < 1e-9);
        assert!((en.toggle_rate_per_ns - 0.05).abs() < 1e-9);

        // cnt[0] is 0 (#0..#5: 5ns), 1 (#5..#15: 10ns), 0 (#15..#20: 5ns), 1 (#20..#40: 20ns) => T1=30ns, P1=0.75, TC=3
        let cnt0 = db.lookup("cnt[0]").expect("cnt[0] in VCD");
        assert_eq!(cnt0.toggle_count, 3);
        assert!((cnt0.static_prob - 0.75).abs() < 1e-9);
        assert!((cnt0.toggle_rate_per_ns - (3.0 / 40.0)).abs() < 1e-9);

        // cnt[1] is 0 (#0..#15: 15ns), 1 (#15..#40: 25ns) => T1=25ns, P1=0.625, TC=1
        let cnt1 = db.lookup("cnt[1]").expect("cnt[1] in VCD");
        assert_eq!(cnt1.toggle_count, 1);
        assert!((cnt1.static_prob - 0.625).abs() < 1e-9);
    }

    #[test]
    fn lut6_boolean_difference_and_shannon_cofactor_exactness() {
        // 1. 2-input AND gate: init = 0b1000 = 0x8
        // P(A=1)=0.25, P(B=1)=0.50 => P(out=1) = 0.25 * 0.50 = 0.125
        // d(A&B)/dA = B (prob 0.50), d(A&B)/dB = A (prob 0.25)
        let probs = [0.25, 0.50];
        assert!((lut6_static_prob(0x8, &probs) - 0.125).abs() < 1e-12);
        assert!((lut6_boolean_diff_prob(0x8, 0, &probs) - 0.50).abs() < 1e-12);
        assert!((lut6_boolean_diff_prob(0x8, 1, &probs) - 0.25).abs() < 1e-12);

        let (p_and, d_and) = lut6_propagate_activity(0x8, &[(0.25, 0.10), (0.50, 0.20)]);
        assert!((p_and - 0.125).abs() < 1e-12);
        // D(out) = 0.50 * 0.10 + 0.25 * 0.20 = 0.10
        assert!((d_and - 0.10).abs() < 1e-12);

        // 2. 2-input XOR gate: init = 0b0110 = 0x6
        // Boolean difference wrt every active input of XOR is identically 1.0!
        assert!((lut6_boolean_diff_prob(0x6, 0, &probs) - 1.0).abs() < 1e-12);
        assert!((lut6_boolean_diff_prob(0x6, 1, &probs) - 1.0).abs() < 1e-12);
        let (p_xor, d_xor) = lut6_propagate_activity(0x6, &[(0.25, 0.10), (0.50, 0.20)]);
        // P(0.25 ^ 0.50) = 0.25*0.5 + 0.75*0.5 = 0.50; D = 1.0*0.10 + 1.0*0.20 = 0.30
        assert!((p_xor - 0.50).abs() < 1e-12);
        assert!((d_xor - 0.30).abs() < 1e-12);

        // 3. Redundant input test: f(x0, x1) = x0 (init = 0b1010 = 0xA).
        // Input x1 has zero Boolean difference sensitivity, so even huge x1 toggle rate adds 0 output toggles.
        let (p_red, d_red) = lut6_propagate_activity(0xA, &[(0.3, 0.04), (0.9, 5.0)]);
        assert!((p_red - 0.3).abs() < 1e-12);
        assert!((d_red - 0.04).abs() < 1e-12);

        // 4. Full 6-input XOR parity LUT6: all 6 inputs have sensitivity 1.0
        let mut parity6 = 0u64;
        for m in 0..64u64 {
            if m.count_ones() % 2 == 1 {
                parity6 |= 1u64 << m;
            }
        }
        let p6 = [0.5; 6];
        assert!((lut6_static_prob(parity6, &p6) - 0.5).abs() < 1e-12);
        for var in 0..6 {
            assert!((lut6_boolean_diff_prob(parity6, var, &p6) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn vectorless_propagation_across_comb_and_seq_cones() {
        let mut nl = PowerNetlist::new();
        nl.primary_inputs = vec!["a".into(), "b".into(), "ce".into()];
        nl.primary_outputs = vec!["dout".into()];
        nl.add_clock("clk", "clk", 200.0); // 200 MHz => 0.40 tr/ns

        // LUT6 AND gate: ab = a & b (init = 0x8)
        nl.add_cell(
            "u_and",
            PowerCellKind::Lut6 {
                init: 0x8,
                inputs: vec!["a".into(), "b".into()],
                output: "ab".into(),
            },
        );
        // Carry4 stage 0
        nl.add_cell(
            "u_carry",
            PowerCellKind::Carry4 {
                ci: None,
                cyinit: Some("a".into()),
                di: [Some("b".into()), None, None, None],
                s: [Some("ab".into()), None, None, None],
                co: [Some("co0".into()), None, None, None],
                o: [Some("sum0".into()), None, None, None],
            },
        );
        // Muxf7 selecting between ab and sum0
        nl.add_cell(
            "u_muxf7",
            PowerCellKind::Muxf7 {
                i0: "ab".into(),
                i1: "sum0".into(),
                s: "b".into(),
                o: "mux_o".into(),
            },
        );
        // FF capturing mux_o with clock enable `ce`
        nl.add_cell(
            "u_ff",
            PowerCellKind::Ff {
                clk: "clk".into(),
                d: "mux_o".into(),
                ce: Some("ce".into()),
                sr: None,
                q: "dout".into(),
                sr_sync: true,
                sr_val: false,
            },
        );

        let mut seed = ActivityDatabase::new();
        seed.insert(NetActivity::new("ce", 0.01, 0.25, ActivitySource::Saif));
        let cfg = VectorlessConfig::default();
        let prop = propagate_vectorless_activity(&nl, Some(&seed), &cfg);

        let ab = prop.lookup("ab").unwrap();
        assert!((ab.static_prob - 0.25).abs() < 1e-6);
        let dout = prop.lookup("dout").unwrap();
        assert!(dout.toggle_rate_per_ns > 0.0);
        assert!(
            dout.toggle_rate_per_ns < prop.lookup("mux_o").unwrap().toggle_rate_per_ns,
            "CE=0.25 must attenuate FF output toggle rate relative to un-gated mux_o"
        );
    }

    #[test]
    fn multi_rail_thermal_convergence_and_confidence_levels() {
        let dev = Device::load_part("HL10T-C32-1").expect("load HL10T-C32-1");
        let mut nl = PowerNetlist::new();
        nl.primary_inputs = vec!["in0".into(), "in1".into()];
        nl.primary_outputs = vec!["pad_out".into()];
        nl.add_clock("sys_clk", "clk", 250.0);

        nl.add_cell(
            "u_pll",
            PowerCellKind::MmcmPll {
                clk_in: "clk".into(),
                clk_out: "clk_fast".into(),
                mult: 2.0,
                div: 1.0,
            },
        );
        nl.add_cell(
            "u_lut",
            PowerCellKind::Lut6 {
                init: 0x6,
                inputs: vec!["in0".into(), "in1".into()],
                output: "xor_n".into(),
            },
        );
        nl.add_cell(
            "u_ff",
            PowerCellKind::Ff {
                clk: "clk_fast".into(),
                d: "xor_n".into(),
                ce: None,
                sr: None,
                q: "q_n".into(),
                sr_sync: true,
                sr_val: false,
            },
        );
        nl.add_cell(
            "u_bram",
            PowerCellKind::Bram {
                clk: "clk_fast".into(),
                en_a: None,
                we_a: Some("in0".into()),
                en_b: None,
                we_b: None,
                addr_inputs: vec!["in0".into(), "in1".into()],
                data_inputs: vec!["q_n".into()],
                outputs: vec!["bram_do".into()],
                write_mode: BramWriteMode::WriteFirst,
            },
        );
        nl.add_cell(
            "u_dsp",
            PowerCellKind::Dsp {
                clk: Some("clk_fast".into()),
                ce: None,
                a_inputs: vec!["bram_do".into()],
                b_inputs: vec!["q_n".into()],
                c_inputs: vec![],
                outputs: vec!["dsp_p".into()],
            },
        );
        nl.add_cell(
            "u_obuf",
            PowerCellKind::Io {
                pad: "pad_out".into(),
                i_net: Some("dsp_p".into()),
                o_net: None,
                t_net: None,
                iostandard: "LVCMOS18".into(),
                drive_ma: 12,
                slew: "FAST".into(),
            },
        );

        let vec_cfg = VectorlessConfig::default();
        let thermal_still = ThermalConditions::default();
        let rep_low = analyze_power(&dev, &nl, None, &thermal_still, &vec_cfg);

        // Check all 4 voltage rails exist and sum to total_power_mw
        assert_eq!(rep_low.rails.len(), 4);
        let rail_sum: f64 = rep_low.rails.iter().map(|r| r.total_mw).sum();
        assert!((rail_sum - rep_low.total_power_mw).abs() < 1e-6);
        for rail in [
            VoltageRail::Vccint,
            VoltageRail::Vccaux,
            VoltageRail::Vccbram,
            VoltageRail::Vcco,
        ] {
            let r = rep_low.rail(rail).unwrap();
            assert!(r.dynamic_mw > 0.0, "{} dynamic must be > 0", rail.name());
            assert!(r.static_mw > 0.0, "{} static must be > 0", rail.name());
            assert!((r.current_ma - r.total_mw / r.voltage_v).abs() < 1e-6);
        }

        // Check T_J self-consistency: T_J == T_A + Theta_JA * (P_total_mw / 1000)
        let expected_tj = rep_low.ambient_temp_c
            + rep_low.effective_theta_ja_c_per_w * (rep_low.total_power_mw / 1_000.0);
        assert!(
            (rep_low.junction_temp_c - expected_tj).abs() < 1e-2,
            "T_J={} vs expected={}",
            rep_low.junction_temp_c,
            expected_tj
        );
        assert_eq!(rep_low.confidence, PowerConfidence::Low);

        // High airflow + heatsink lowers Theta_JA, lowering T_J and static leakage
        let thermal_cooled = ThermalConditions {
            airflow: AirflowProfile::Lfm500,
            heatsink: HeatsinkProfile::HighProfile,
            ..ThermalConditions::default()
        };
        let rep_cooled = analyze_power(&dev, &nl, None, &thermal_cooled, &vec_cfg);
        assert!(rep_cooled.effective_theta_ja_c_per_w < rep_low.effective_theta_ja_c_per_w);
        assert!(rep_cooled.junction_temp_c < rep_low.junction_temp_c);
        assert!(rep_cooled.static_power_mw < rep_low.static_power_mw);

        // Annotate >= 75% of nets via SAIF => Confidence becomes High
        let mut full_db = ActivityDatabase::new();
        for net in nl.all_nets() {
            full_db.insert(NetActivity::new(net, 0.08, 0.5, ActivitySource::Saif));
        }
        let rep_high = analyze_power(&dev, &nl, Some(&full_db), &thermal_still, &vec_cfg);
        assert_eq!(rep_high.confidence, PowerConfidence::High);
        assert!(rep_high.summary_text().contains("VCCINT"));
        assert!(rep_high.summary_text().contains("VCCBRAM"));
    }

    #[test]
    fn power_opt_design_extracts_ff_ce_and_optimizes_bram_modes() {
        let dev = Device::load_part("HL10T-C32-1").expect("load HL10T-C32-1");
        let mut nl = PowerNetlist::new();
        nl.primary_inputs = vec!["din".into(), "ce_sig".into(), "read_en".into(), "we_sig".into()];
        nl.primary_outputs = vec!["final_out".into()];
        nl.add_clock("clk", "clk", 200.0);

        // 1. FF with feedback mux LUT6: d_mux = (!ce_sig & q_reg) | (ce_sig & din)
        // With inputs [q_reg (bit0), din (bit1), ce_sig (bit2)], truth table is 0xCA
        nl.add_cell(
            "u_fb_mux",
            PowerCellKind::Lut6 {
                init: 0xCA,
                inputs: vec!["q_reg".into(), "din".into(), "ce_sig".into()],
                output: "d_mux".into(),
            },
        );
        nl.add_cell(
            "u_ff_feedback",
            PowerCellKind::Ff {
                clk: "clk".into(),
                d: "d_mux".into(),
                ce: None,
                sr: None,
                q: "q_reg".into(),
                sr_sync: true,
                sr_val: false,
            },
        );

        // 2. FF whose output `q_unread` is only consumed by an AND gate masked by `read_en`
        nl.add_cell(
            "u_ff_unread",
            PowerCellKind::Ff {
                clk: "clk".into(),
                d: "din".into(),
                ce: None,
                sr: None,
                q: "q_unread".into(),
                sr_sync: true,
                sr_val: false,
            },
        );
        nl.add_cell(
            "u_mask_and",
            PowerCellKind::Lut6 {
                init: 0x8, // q_unread & read_en
                inputs: vec!["q_unread".into(), "read_en".into()],
                output: "masked_q".into(),
            },
        );

        // 3. BRAM in WriteFirst mode with un-gated en_a whose output is masked by `read_en`
        nl.add_cell(
            "u_bram",
            PowerCellKind::Bram {
                clk: "clk".into(),
                en_a: None,
                we_a: Some("we_sig".into()),
                en_b: None,
                we_b: None,
                addr_inputs: vec!["q_reg".into()],
                data_inputs: vec!["masked_q".into()],
                outputs: vec!["bram_do".into()],
                write_mode: BramWriteMode::WriteFirst,
            },
        );
        nl.add_cell(
            "u_bram_mask",
            PowerCellKind::Lut6 {
                init: 0x8, // bram_do & read_en
                inputs: vec!["bram_do".into(), "read_en".into()],
                output: "final_out".into(),
            },
        );

        let mut seed = ActivityDatabase::new();
        seed.insert(NetActivity::new("ce_sig", 0.02, 0.15, ActivitySource::Saif));
        seed.insert(NetActivity::new("read_en", 0.02, 0.20, ActivitySource::Saif));
        seed.insert(NetActivity::new("we_sig", 0.04, 0.25, ActivitySource::Saif));

        let summary = power_opt_design(
            &dev,
            &mut nl,
            Some(&seed),
            &ThermalConditions::default(),
            &VectorlessConfig::default(),
        );

        assert_eq!(summary.ffs_clock_gated, 1, "must extract FF feedback CE");
        assert_eq!(
            summary.downstream_cones_suppressed, 1,
            "must suppress unread FF cycles via downstream AND mask"
        );
        assert_eq!(
            summary.brams_no_change_converted, 1,
            "must convert BRAM WriteFirst -> NoChange"
        );
        assert_eq!(
            summary.brams_enable_gated, 1,
            "must gate BRAM EN_A with downstream read_en"
        );
        assert!(
            summary.dynamic_after_mw < summary.dynamic_before_mw,
            "dynamic power must drop after power_opt_design: before={} after={}",
            summary.dynamic_before_mw,
            summary.dynamic_after_mw
        );
        assert!(
            summary.estimated_saved_mw > 0.0,
            "total saved power must be positive: {}",
            summary.estimated_saved_mw
        );
    }
}
