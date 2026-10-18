//! VCD → [`WaveTrace`] importer for the waveform viewer.
//!
//! Parses a VCD dump (IEEE 1364 subset) into waveform traces sampled on the
//! viewer's uniform time grid. This is what backs `tb_wave`: the testbench
//! runner's VCD lands in the waveform pane as real traces.
//!
//! ## Supported subset
//!
//! Header: `$timescale` (1/10/100 × s/ms/us/ns/ps/fs), `$scope`/`$upscope`
//! (nested scopes joined with `.`), `$var <type> <width> <id> <reference>`
//! (scalar and vector), `$enddefinitions`, plus skipped
//! `$date`/`$version`/`$comment` blocks.
//!
//! Body: `#<time>` integer timestamps (monotonic), scalar changes
//! (`0`/`1`/`x`/`z` + id), vector changes (`b`/`B` + bits, then id),
//! `$dumpvars` initial values. `$dumpoff`/`$dumpon` markers are accepted and
//! ignored: all recorded changes are shown.
//!
//! ## Value mapping
//!
//! `WaveTrace` samples are `u64`; there is no unknown state. `x`/`z` bits map
//! to 0 (same convention as the power parser in `vcd.rs`). Vectors wider
//! than 64 bits are truncated to the low 64 bits.
//!
//! ## Sampling
//!
//! The viewer uses a uniform grid (`Waveform::time_ps = sample *
//! timescale_ps`). The importer picks the grid from the dump: the VCD's own
//! timescale, coarsened (to a 1/2/5 × 10ⁿ step) only when the dump would
//! exceed [`MAX_WAVE_SAMPLES`] samples. Values use zero-order hold between
//! events.
//!
//! ## Limits
//!
//! Dumps with more than [`MAX_WAVE_SIGNALS`] signals keep the first N in file
//! order and set [`VcdWaveImport::truncated`]. Malformed input yields `Err`
//! with a human-readable message; the caller must surface it, never a
//! silent empty view.

use crate::ide::{WaveRadix, WaveStyle, WaveTrace};
use std::collections::HashMap;

/// Upper bound on imported signals; larger dumps truncate deterministically.
pub const MAX_WAVE_SIGNALS: usize = 1024;
/// Upper bound on samples per trace.
pub const MAX_WAVE_SAMPLES: usize = 8192;

/// A VCD dump converted to waveform traces.
#[derive(Clone, Debug)]
pub struct VcdWaveImport {
    /// One trace per signal, file order.
    pub traces: Vec<WaveTrace>,
    /// Picoseconds per sample on the viewer grid.
    pub timescale_ps: u64,
    /// Last timestamp in the dump, picoseconds.
    pub duration_ps: u64,
    /// True when the signal cap truncated the dump.
    pub truncated: bool,
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

/// Round `v` up to a 1/2/5 × 10ⁿ step (nice grid spacing).
fn nice_ceil(v: u64) -> u64 {
    if v == 0 {
        return 1;
    }
    let mut pow10: u64 = 1;
    while pow10 < v {
        pow10 = pow10.saturating_mul(10);
    }
    // pow10 is now the smallest 10ⁿ >= v; try 5×10ⁿ⁻¹, 2×10ⁿ⁻¹, 1×10ⁿ⁻¹.
    let mut cand = pow10;
    for m in [5, 2, 1] {
        let c = pow10 / 10 * m;
        if c >= v && c > 0 {
            cand = c;
        }
    }
    cand.max(1)
}

/// Parse one scalar/vector value token into a u64. `x`/`z` bits become 0.
/// Returns None on characters outside 01xzXZ for the given form.
fn parse_value_bits(scalar: Option<char>, vector: Option<&str>, width: u32) -> Option<u64> {
    let mut val: u64 = 0;
    if let Some(c) = scalar {
        return match c {
            '0' => Some(0),
            '1' => Some(1),
            'x' | 'X' | 'z' | 'Z' => Some(0),
            _ => None,
        };
    }
    let bits = vector?;
    // VCD vectors are MSB-first; keep the low 64 bits.
    let w = (width as usize).min(64);
    for (i, ch) in bits.chars().rev().take(w).enumerate() {
        match ch {
            '1' => val |= 1u64 << i,
            '0' | 'x' | 'X' | 'z' | 'Z' => {}
            _ => return None,
        }
    }
    Some(val)
}

struct VarDecl {
    width: u32,
    name: String,
}

/// Parse VCD text into waveform traces. Errors are human-readable; no panics
/// on malformed input.
pub fn import_vcd_waves(text: &str) -> Result<VcdWaveImport, String> {
    if text.trim().is_empty() {
        return Err("empty VCD".to_string());
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0usize;
    let mut timescale_fs: u64 = 1_000; // default 1ps
    let mut scope: Vec<String> = Vec::new();
    // id -> (width, hierarchical name), insertion ordered.
    let mut vars: Vec<(String, VarDecl)> = Vec::new();
    let mut var_index: HashMap<String, usize> = HashMap::new();
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
                    scope.push(tokens[i + 1].to_string());
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
                    if var_index.contains_key(&id) {
                        return Err(format!("duplicate $var id: {id}"));
                    }
                    if vars.len() >= MAX_WAVE_SIGNALS {
                        truncated = true;
                    } else {
                        var_index.insert(id.clone(), vars.len());
                        vars.push((id, VarDecl { width, name }));
                    }
                }
            }
            "$enddefinitions" => {
                enddefs = true;
                if i < tokens.len() && tokens[i] == "$end" {
                    i += 1;
                }
                break;
            }
            "$date" | "$version" | "$comment" => {
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
        return Err("missing $enddefinitions".to_string());
    }
    if vars.is_empty() {
        return Err("no $var signals in VCD".to_string());
    }

    // --- body: per-signal event timelines ---
    // events[k] = list of (time_ps, value); initial value from $dumpvars or 0.
    let mut events: Vec<Vec<(u64, u64)>> = vec![Vec::new(); vars.len()];
    let mut tick: u64 = 0;
    let mut last_tick: Option<u64> = None;
    let tick_to_ps = |t: u64| t.saturating_mul(timescale_fs) / 1_000;

    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok == "$dumpvars" || tok == "$dumpall" {
            continue;
        }
        if tok == "$end" {
            continue;
        }
        if tok == "$dumpoff" || tok == "$dumpon" {
            continue;
        }
        if let Some(stripped) = tok.strip_prefix('#') {
            let t: u64 = stripped
                .parse()
                .map_err(|_| format!("bad timestamp: {tok}"))?;
            if let Some(last) = last_tick {
                if t < last {
                    return Err(format!("timestamps go backwards: #{t} after #{last}"));
                }
            }
            last_tick = Some(t);
            tick = t;
            continue;
        }
        // Value change: scalar `0id` or vector `b<bits> id`.
        let (scalar, vector, id): (Option<char>, Option<&str>, &str) = if let Some(bits) = tok.strip_prefix('b').or_else(|| tok.strip_prefix('B')) {
            if i >= tokens.len() {
                return Err("vector value missing id".to_string());
            }
            let id = tokens[i];
            i += 1;
            (None, Some(bits), id)
        } else {
            let mut chars = tok.chars();
            let val = chars.next().ok_or_else(|| "empty value token".to_string())?;
            let rest = chars.as_str();
            if rest.is_empty() {
                return Err(format!("scalar value missing id: {tok}"));
            }
            (Some(val), None, rest)
        };
        let Some(&k) = var_index.get(id) else { continue };
        let width = vars[k].1.width;
        let value = parse_value_bits(scalar, vector, width)
            .ok_or_else(|| format!("bad value for {id}: {tok}"))?;
        let t_ps = tick_to_ps(tick);
        // Coalesce with the previous event at the same tick (e.g. $dumpvars
        // followed by an explicit change at #0): last write wins.
        if let Some(last) = events[k].last_mut() {
            if last.0 == t_ps {
                last.1 = value;
                continue;
            }
        }
        events[k].push((t_ps, value));
    }

    let duration_ps = tick_to_ps(last_tick.unwrap_or(0));
    let vcd_ts_ps = (timescale_fs / 1_000).max(1);

    // --- resample onto the viewer grid ---
    let min_grid = duration_ps.div_ceil(MAX_WAVE_SAMPLES as u64).max(1);
    let grid_ps = nice_ceil(vcd_ts_ps.max(min_grid));
    let n_samples = (duration_ps / grid_ps) as usize + 1;
    if n_samples > MAX_WAVE_SAMPLES + 1 {
        return Err(format!(
            "dump too long for the viewer grid: {n_samples} samples"
        ));
    }

    let mut traces = Vec::with_capacity(vars.len());
    for (k, (_, decl)) in vars.iter().enumerate() {
        let width = (decl.width as usize).min(64).max(1) as u8;
        let mut trace = if decl.width == 1 {
            let mut t = WaveTrace::scalar(decl.name.clone());
            t.style = WaveStyle::Digital;
            t.radix = WaveRadix::Binary;
            t
        } else {
            let mut t = WaveTrace::bus(decl.name.clone(), width);
            t.style = WaveStyle::Digital;
            t.radix = WaveRadix::Hexadecimal;
            t
        };
        // Zero-order hold of the event timeline onto the grid.
        let evs = &events[k];
        let mut ev_idx = 0usize;
        let mut cur: u64 = 0;
        // Initial value: last event at t=0 ($dumpvars) applies from the start.
        let mut samples = Vec::with_capacity(n_samples);
        for s in 0..n_samples {
            let t = s as u64 * grid_ps;
            while ev_idx < evs.len() && evs[ev_idx].0 <= t {
                cur = evs[ev_idx].1;
                ev_idx += 1;
            }
            let mask = if width >= 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            samples.push(cur & mask);
        }
        trace.samples = samples;
        traces.push(trace);
    }

    Ok(VcdWaveImport {
        traces,
        timescale_ps: grid_ps,
        duration_ps,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_VCD: &str = "\
$date\n  test\n$end\n$version\n  helion test\n$end\n$timescale 1ns $end\n\
$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" cnt $end\n\
$upscope $end\n$enddefinitions $end\n\
#0\n$dumpvars\n0!\nb0000 \"\n$end\n#5\n1!\n#10\n0!\nb0001 \"\n#15\n1!\n#20\n0!\nb0010 \"\n";

    #[test]
    fn parses_scalar_and_bus() {
        let imp = import_vcd_waves(SIMPLE_VCD).expect("parse");
        assert_eq!(imp.traces.len(), 2);
        assert_eq!(imp.traces[0].name, "top.clk");
        assert_eq!(imp.traces[0].width, 1);
        assert_eq!(imp.traces[1].name, "top.cnt");
        assert_eq!(imp.traces[1].width, 4);
        // 1ns timescale, 20ns duration -> 21 samples at 1ns.
        assert_eq!(imp.timescale_ps, 1_000);
        assert_eq!(imp.traces[0].samples.len(), 21);
        // clk: 0 until 5ns, 1 until 10ns, 0 until 15ns, 1 until 20ns, 0 after.
        assert_eq!(imp.traces[0].samples[0], 0);
        assert_eq!(imp.traces[0].samples[5], 1);
        assert_eq!(imp.traces[0].samples[10], 0);
        assert_eq!(imp.traces[0].samples[15], 1);
        assert_eq!(imp.traces[0].samples[20], 0);
        // cnt bus holds 0, 1, 2.
        assert_eq!(imp.traces[1].samples[0], 0);
        assert_eq!(imp.traces[1].samples[10], 1);
        assert_eq!(imp.traces[1].samples[20], 2);
    }

    #[test]
    fn helion_sim_dump_imports() {
        let src = r#"
module counter (input logic clk, output logic led);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin cnt <= cnt + 1; end
    assign led = cnt[3];
endmodule
"#;
        let d = helion_sv::synth_sv(src, "counter.sv").expect("synth");
        let vcd = helion_sim::vcd::dump_vcd(&d, 16, 10);
        let imp = import_vcd_waves(&vcd).expect("import sim dump");
        assert!(!imp.traces.is_empty());
        let clk = imp.traces.iter().find(|t| t.name.ends_with(".clk") || t.name == "clk").expect("clk");
        // clk toggles: samples contain both 0 and 1.
        assert!(clk.samples.contains(&0) && clk.samples.contains(&1));
        // 16 cycles * 10ps = 160ps span; 1ps grid -> <= 161 samples.
        assert!(imp.traces[0].samples.len() <= 161);
        assert_eq!(imp.timescale_ps, 1);
    }

    #[test]
    fn malformed_is_loud() {
        assert!(import_vcd_waves("").is_err());
        assert!(import_vcd_waves("not a vcd at all").is_err());
        assert!(import_vcd_waves("$timescale 1ps $end\n").is_err()); // no enddefinitions
        let no_vars = "$timescale 1ps $end\n$enddefinitions $end\n";
        assert!(import_vcd_waves(no_vars).is_err());
        let backwards = "$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#10\n1!\n#5\n0!\n";
        assert!(import_vcd_waves(backwards).is_err());
    }

    #[test]
    fn xz_maps_to_zero() {
        let vcd = "$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\nx!\n#10\nz!\n#20\n1!\n";
        let imp = import_vcd_waves(vcd).expect("parse");
        assert_eq!(imp.traces[0].samples[0], 0);
        assert_eq!(imp.traces[0].samples[10], 0);
        assert_eq!(imp.traces[0].samples[20], 1);
    }

    #[test]
    fn long_dump_coarsens_grid() {
        // 1ps timescale, 100us duration -> must coarsen, not emit 100M samples.
        let mut vcd = String::from("$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\n0!\n");
        vcd.push_str("#100000000\n1!\n");
        let imp = import_vcd_waves(&vcd).expect("parse");
        assert!(imp.traces[0].samples.len() <= MAX_WAVE_SAMPLES + 1);
        assert!(imp.timescale_ps >= 100_000_000 / MAX_WAVE_SAMPLES as u64);
        // Value still lands: last sample is 1.
        assert_eq!(*imp.traces[0].samples.last().expect("samples"), 1);
    }
}
