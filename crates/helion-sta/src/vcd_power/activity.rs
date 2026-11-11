//! VCD (IEEE 1364) parser → per-signal switching activity.
//!
//! ## Supported subset
//!
//! Header: `$timescale` (1/10/100 × s/ms/us/ns/ps/fs), `$scope`/`$upscope`
//! (nested scopes joined with `.`), `$var <type> <width> <id> <reference>`
//! (scalar and vector; bit-select suffixes kept on the reference name),
//! `$enddefinitions`, plus skipped `$date`/`$version`/`$comment` blocks.
//!
//! Body: `#<time>` integer timestamps (timescale units), scalar changes
//! (`0`/`1` + id), vector changes (`b`/`B` + bits, then id), `$dumpvars`
//! initial values, `$dumpall` (treated as normal changes). `$dumpoff`/`$dumpon`
//! regions are skipped.
//!
//! ## Not supported
//!
//! * Real-valued changes (`r...`) are ignored for activity.
//! * `x`/`z` bits are sticky-unknown: they are treated as 0 for the stored
//!   value and transitions *to/from* `x`/`z` do **not** count as toggles.
//!   Only clean 0↔1 bit flips count.
//!
//! ## Limits
//!
//! Files with more than [`MAX_VCD_PARSE_SIGNALS`] distinct signals set
//! [`VcdActivity::truncated`]; the first N signals (file order) are kept.

use std::collections::HashMap;

/// Upper bound on parsed signals; larger dumps are truncated deterministically.
pub const MAX_VCD_PARSE_SIGNALS: usize = 100_000;

/// Per-signal switching activity extracted from a VCD.
#[derive(Clone, Debug)]
pub struct SignalActivity {
    /// Hierarchical reference name (`top.cnt[0]` style).
    pub name: String,
    /// Declared `$var` width in bits.
    pub width: u32,
    /// Clean 0↔1 bit flips over the dump.
    pub toggles: u64,
    /// 0→1 bit flips (for P(0→1) activity factors).
    pub rising: u64,
    /// Number of timestamps at which this signal changed.
    pub events: u64,
    pub first_change_ps: Option<u64>,
    pub last_change_ps: Option<u64>,
    /// Smallest gap between consecutive changes of this signal.
    pub min_period_ps: Option<u64>,
}

/// Switching activity for a whole VCD dump.
#[derive(Clone, Debug, Default)]
pub struct VcdActivity {
    /// Timescale in picoseconds (rounded down from the `$timescale` unit).
    pub timescale_ps: u64,
    /// Last timestamp seen, in picoseconds.
    pub duration_ps: u64,
    /// Per-signal activity, sorted by name.
    pub signals: Vec<SignalActivity>,
    /// True when the signal cap truncated the dump.
    pub truncated: bool,
}

impl VcdActivity {
    pub fn get(&self, name: &str) -> Option<&SignalActivity> {
        self.signals.iter().find(|s| s.name == name)
    }

    /// Toggles per second for a signal (0 when duration is 0).
    pub fn toggle_rate_hz(&self, name: &str) -> f64 {
        match (self.get(name), self.duration_ps) {
            (Some(s), d) if d > 0 => s.toggles as f64 / (d as f64 * 1e-12),
            _ => 0.0,
        }
    }

    /// P(0→1) per clock cycle: `rising / cycles`. Returns 0 for cycles == 0.
    pub fn activity_factor(name_activity: Option<&SignalActivity>, cycles: u64) -> f64 {
        match (name_activity, cycles) {
            (Some(s), c) if c > 0 => s.rising as f64 / c as f64,
            _ => 0.0,
        }
    }
}

fn unit_to_fs(unit: &str) -> Option<u64> {
    match unit {
        "s" => Some(1_000_000_000_000_000),
        "ms" => Some(1_000_000_000_000),
        "us" => Some(1_000_000_000),
        "ns" => Some(1_000_000),
        "ps" => Some(1_000),
        "fs" => Some(1),
        _ => None,
    }
}

/// Parse `$timescale` value+unit tokens into femtoseconds per tick.
fn parse_timescale(tokens: &[&str]) -> Result<u64, String> {
    // Forms: "1ps" | "1 ps" | "10 ns".
    let joined: String = tokens.join("");
    let split = joined
        .find(|c: char| c.is_alphabetic())
        .ok_or_else(|| format!("bad $timescale: {joined}"))?;
    let (num, unit) = joined.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("bad $timescale number: {joined}"))?;
    if n != 1 && n != 10 && n != 100 {
        return Err(format!("$timescale must be 1/10/100: {joined}"));
    }
    let fs = unit_to_fs(unit).ok_or_else(|| format!("bad $timescale unit: {joined}"))?;
    Ok(n * fs)
}

#[derive(Debug)]
struct VarDecl {
    width: u32,
    name: String,
}

/// Parse VCD text into [`VcdActivity`]. Errors are human-readable; no panics
/// on malformed input (truncated/odd files yield `Err`, never a crash).
pub fn parse_vcd(text: &str) -> Result<VcdActivity, String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0usize;
    let mut timescale_fs: u64 = 1_000; // default 1ps
    let mut scope: Vec<String> = Vec::new();
    // id -> (width, hierarchical name), insertion ordered.
    let mut vars: Vec<(String, VarDecl)> = Vec::new();
    let mut var_index: HashMap<String, usize> = HashMap::new();
    // Set when the header declares more signals than MAX_VCD_PARSE_SIGNALS.
    let mut truncated = false;

    // --- header ---
    let mut enddefs = false;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        match tok {
            "$timescale" => {
                let mut args = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    args.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                } // $end
                timescale_fs = parse_timescale(&args)?;
            }
            "$scope" => {
                // $scope module <name> $end
                if i + 2 < tokens.len() {
                    let name = tokens[i + 1].to_string();
                    scope.push(name);
                    i += 3; // kind, name, $end
                }
            }
            "$upscope" => {
                scope.pop();
                if i < tokens.len() && tokens[i] == "$end" {
                    i += 1;
                }
            }
            "$var" => {
                // $var <type> <width> <id> <reference...> $end
                let mut args = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    args.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                if args.len() >= 4 {
                    let width: u32 = args[1].parse().unwrap_or(1).max(1);
                    let id = args[2].to_string();
                    let reference = args[3..].join("");
                    let mut name = String::new();
                    for (k, s) in scope.iter().enumerate() {
                        if k > 0 {
                            name.push('.');
                        }
                        name.push_str(s);
                    }
                    if !name.is_empty() {
                        name.push('.');
                    }
                    name.push_str(&reference);
                    if !var_index.contains_key(&id) {
                        if vars.len() < MAX_VCD_PARSE_SIGNALS {
                            var_index.insert(id.clone(), vars.len());
                            vars.push((id, VarDecl { width, name }));
                        } else {
                            truncated = true;
                        }
                    }
                }
            }
            "$enddefinitions" => {
                if i < tokens.len() && tokens[i] == "$end" {
                    i += 1;
                }
                enddefs = true;
                break;
            }
            "$comment" | "$date" | "$version" => {
                while i < tokens.len() && tokens[i] != "$end" {
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
            }
            _ => {}
        }
    }
    if !enddefs {
        return Err("VCD: missing $enddefinitions".into());
    }

    // --- body ---
    // Per-signal runtime state, parallel to `vars`.
    struct State {
        bits: Vec<u8>, // 0/1/2(x/z); init 0
        toggles: u64,
        rising: u64,
        events: u64,
        first_ps: Option<u64>,
        last_ps: Option<u64>,
        min_period_ps: Option<u64>,
        prev_change_ps: Option<u64>,
    }
    let mut states: Vec<State> = vars
        .iter()
        .map(|(_, v)| State {
            bits: vec![0u8; v.width.min(4096) as usize],
            toggles: 0,
            rising: 0,
            events: 0,
            first_ps: None,
            last_ps: None,
            min_period_ps: None,
            prev_change_ps: None,
        })
        .collect();

    let mut time_fs: u64 = 0;
    let mut max_fs: u64 = 0;
    let mut in_dumpvars = false;
    let mut skip_region = false; // $dumpoff..$dumpon

    // Apply one value change; unknown ids are ignored (never an error).
    let apply = |idx: usize, new_bits: &[u8], t_ps: u64, count: bool, states: &mut [State]| {
        let w = states[idx].bits.len();
        let st = &mut states[idx];
        let n = new_bits.len().min(w);
        let mut changed_bits = 0u64;
        let mut rose = 0u64;
        for b in 0..n {
            let old = st.bits[b];
            let new = new_bits[b];
            if old != new {
                if old <= 1 && new <= 1 {
                    changed_bits += 1;
                    if old == 0 && new == 1 {
                        rose += 1;
                    }
                }
                st.bits[b] = new;
            }
        }
        if count && changed_bits > 0 {
            st.toggles += changed_bits;
            st.rising += rose;
            st.events += 1;
            if st.first_ps.is_none() {
                st.first_ps = Some(t_ps);
            }
            st.last_ps = Some(t_ps);
            if let Some(prev) = st.prev_change_ps {
                let gap = t_ps.saturating_sub(prev);
                st.min_period_ps = Some(st.min_period_ps.map_or(gap, |m| m.min(gap)));
            }
            st.prev_change_ps = Some(t_ps);
        } else if !count {
            // $dumpvars initial values: set without counting.
            for b in 0..n {
                st.bits[b] = new_bits[b];
            }
        }
    };

    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok.is_empty() {
            continue;
        }
        if skip_region {
            if tok == "$dumpon" || tok == "$dumpall" {
                skip_region = false;
                if tok == "$dumpall" {
                    // $dumpall <changes> $end - fall through to change parsing below
                    // by rewinding one token.
                    i -= 1;
                    skip_region = false;
                } else if i < tokens.len() && tokens[i] == "$end" {
                    i += 1;
                }
            }
            continue;
        }
        if tok == "$dumpvars" {
            in_dumpvars = true;
            continue;
        }
        if tok == "$end" {
            in_dumpvars = false;
            continue;
        }
        if tok == "$dumpoff" {
            skip_region = true;
            continue;
        }
        if tok == "$dumpall" {
            // changes until $end, counted normally
            continue;
        }
        if tok == "$dumpon" {
            if i < tokens.len() && tokens[i] == "$end" {
                i += 1;
            }
            continue;
        }
        if tok.starts_with('#') {
            if let Ok(t) = tok[1..].parse::<u64>() {
                time_fs = t.saturating_mul(timescale_fs);
                max_fs = max_fs.max(time_fs);
            }
            continue;
        }
        let t_ps = time_fs / 1000;
        let b = tok.as_bytes();
        if b[0] == b'b' || b[0] == b'B' {
            // Vector: next token is the id.
            if i >= tokens.len() {
                break;
            }
            let id = tokens[i];
            i += 1;
            if let Some(&idx) = var_index.get(id) {
                let bits: Vec<u8> = tok[1..]
                    .bytes()
                    .rev() // VCD lists MSB first; store LSB first
                    .map(|c| match c {
                        b'0' => 0,
                        b'1' => 1,
                        _ => 2,
                    })
                    .collect();
                apply(idx, &bits, t_ps, !in_dumpvars, &mut states);
            }
            continue;
        }
        if b[0] == b'r' || b[0] == b'R' {
            // Real value: skip id token.
            if i < tokens.len() {
                i += 1;
            }
            continue;
        }
        // Scalar: first char is the value, rest is the id.
        let (val, id) = (b[0], &tok[1..]);
        if matches!(val, b'0' | b'1' | b'x' | b'X' | b'z' | b'Z') && !id.is_empty() {
            if let Some(&idx) = var_index.get(id) {
                let bit = match val {
                    b'1' => 1,
                    b'0' => 0,
                    _ => 2,
                };
                apply(idx, &[bit], t_ps, !in_dumpvars, &mut states);
            }
        }
    }

    let mut signals: Vec<SignalActivity> = vars
        .into_iter()
        .zip(states.into_iter())
        .map(|((_, decl), st)| SignalActivity {
            name: decl.name,
            width: decl.width,
            toggles: st.toggles,
            rising: st.rising,
            events: st.events,
            first_change_ps: st.first_ps,
            last_change_ps: st.last_ps,
            min_period_ps: st.min_period_ps,
        })
        .collect();
    signals.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(VcdActivity {
        timescale_ps: timescale_fs / 1000,
        duration_ps: max_fs / 1000,
        signals,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = r#"
$date today $end
$version test $end
$timescale 1ns $end
$scope module top $end
$var wire 1 ! clk $end
$var wire 1 " d $end
$var wire 4 # cnt [3:0] $end
$upscope $end
$enddefinitions $end
#0
$dumpvars
0!
0"
b0000 #
$end
#5
1!
#10
0!
1"
b0001 #
#15
1!
b0010 #
#20
0!
#25
1!
#30
0!
"#;

    #[test]
    fn golden_vcd_toggle_counts() {
        let a = parse_vcd(GOLDEN).expect("parse");
        assert_eq!(a.timescale_ps, 1000, "1ns");
        assert_eq!(a.duration_ps, 30_000, "last timestamp #30");
        let clk = a.get("top.clk").expect("clk");
        assert_eq!(clk.toggles, 6, "clk 0→1→0→1→0→1→0");
        assert_eq!(clk.rising, 3);
        assert_eq!(clk.min_period_ps, Some(5_000));
        let d = a.get("top.d").expect("d");
        assert_eq!(d.toggles, 1, "d rises once");
        assert_eq!(d.rising, 1);
        let cnt = a.get("top.cnt[3:0]").expect("cnt");
        assert_eq!(cnt.width, 4);
        assert_eq!(cnt.toggles, 3, "cnt bit flips: 0000→0001→0010");
        assert_eq!(cnt.rising, 2);
    }

    #[test]
    fn xz_transitions_do_not_count() {
        let vcd = "$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n$end\n#10\nx!\n#20\n1!\n#30\nz!\n#40\n0!\n";
        let a = parse_vcd(vcd).expect("parse");
        let s = a.get("t.a").expect("a");
        // 0→x (no), x→1 (no), 1→z (no), z→0 (no): zero clean toggles.
        assert_eq!(s.toggles, 0, "x/z transitions never count: {s:?}");
    }

    #[test]
    fn dumpvars_initial_values_not_counted() {
        let vcd = "$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n1!\n$end\n";
        let a = parse_vcd(vcd).expect("parse");
        assert_eq!(a.get("t.a").unwrap().toggles, 0);
    }

    #[test]
    fn missing_enddefinitions_errors() {
        assert!(parse_vcd("$timescale 1ps $end\n#0\n").is_err());
    }

    #[test]
    fn small_vcd_not_truncated() {
        let a = parse_vcd(GOLDEN).expect("parse");
        assert!(!a.truncated, "under-cap VCD must not report truncated");
    }

    #[test]
    fn signal_cap_reports_truncated() {
        let mut vcd = String::from("$timescale 1ps $end\n$scope module t $end\n");
        for i in 0..MAX_VCD_PARSE_SIGNALS + 2 {
            vcd.push_str(&format!("$var wire 1 id{i} s{i} $end\n"));
        }
        vcd.push_str("$upscope $end\n$enddefinitions $end\n");
        let a = parse_vcd(&vcd).expect("parse");
        assert_eq!(a.signals.len(), MAX_VCD_PARSE_SIGNALS);
        assert!(a.truncated, "over-cap VCD must report truncated");
    }

    #[test]
    fn empty_vcd_parses() {
        let a = parse_vcd("$timescale 1ps $end\n$enddefinitions $end\n").expect("parse");
        assert!(a.signals.is_empty());
        assert_eq!(a.duration_ps, 0);
    }

    #[test]
    fn activity_factor_math() {
        let vcd = "$timescale 10ps $end\n$scope module t $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\n$end\n#10\n1!\n#20\n0!\n";
        let a = parse_vcd(vcd).expect("parse");
        let s = a.get("t.a").unwrap();
        // 1 rising edge over 2 cycles → α = 0.5
        assert_eq!(VcdActivity::activity_factor(Some(s), 2), 0.5);
        assert_eq!(VcdActivity::activity_factor(Some(s), 0), 0.0);
    }

    #[test]
    fn helion_sim_dump_roundtrips() {
        // helion-sim's own dumper output must parse cleanly.
        let src = r#"
module counter (input logic clk, output logic led);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin cnt <= cnt + 1; end
    assign led = cnt[3];
endmodule
"#;
        let d = helion_sv::synth_sv(src, "counter.sv").expect("synth");
        let vcd = helion_sim::vcd::dump_vcd(&d, 16, 10);
        let a = parse_vcd(&vcd).expect("parse sim dump");
        assert_eq!(a.timescale_ps, 1, "dumper uses a fixed 1ps timescale");
        assert_eq!(a.duration_ps, 155, "16 cycles × 10ps, last negedge at 155ps");
        let clk = a.signals.iter().find(|s| s.name.ends_with(".clk") || s.name == "clk").expect("clk signal");
        assert_eq!(clk.toggles, 32, "clk toggles 2× per cycle");
        // At least one counter net toggles.
        let max_tog = a.signals.iter().map(|s| s.toggles).max().unwrap_or(0);
        assert!(max_tog >= 16, "cnt[0] toggles every cycle: {max_tog}");
    }
}
