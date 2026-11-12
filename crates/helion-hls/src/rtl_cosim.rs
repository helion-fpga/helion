//! Real C/RTL co-simulation: the synthesized RTL netlist of an HLS kernel
//! runs cycle by cycle in the `helion-sim` gate simulator against the C
//! reference interpreter ([`crate::evaluate_c_kernel_reference`]).
//!
//! Per vector the harness resets the design, drives scalar inputs, serves
//! `ap_memory` array ports from a one-cycle-latency BRAM model, pulses
//! `ap_start`, and waits for `ap_done` (or, for a kernel emitted without
//! block-level handshake, for its scheduled latency). Outputs are sampled
//! from the netlist (`ap_return`/`y` for the return value, the pointer port
//! for `*out`, latched on its `_ap_vld` strobe) and compared with the C
//! results at the RTL port width. Latency is measured in simulated cycles.

use std::collections::BTreeMap;

use helion_sim::Sim;

use crate::cosim::{CosimReport, CosimStatus, CosimTestVector, CosimVectorResult};

/// Cycle cap per vector (a kernel that never raises `ap_done` fails).
pub const COSIM_MAX_CYCLES: u32 = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RtlPort {
    name: String,
    input: bool,
    width: u32,
}

/// Ports of the first module header in emitted SystemVerilog.
fn header_ports(sv: &str) -> Result<(String, Vec<RtlPort>), String> {
    let at = sv
        .lines()
        .position(|l| l.trim_start().starts_with("module "))
        .ok_or("cosim: no module in emitted RTL")?;
    let text: String = sv.lines().skip(at).collect::<Vec<_>>().join(" ");
    let open = text.find('(').ok_or("cosim: module without ports")?;
    let close = text.find(");").ok_or("cosim: unterminated port list")?;
    let name = text["module ".len()..open].trim().to_string();
    let mut ports = Vec::new();
    for decl in text[open + 1..close].split(',') {
        let d = decl.trim();
        let input = d.starts_with("input");
        if !input && !d.starts_with("output") {
            return Err(format!("cosim: unsupported port declaration `{d}`"));
        }
        let width = match (d.find('['), d.find(']')) {
            (Some(a), Some(b)) => {
                let (hi, lo) = d[a + 1..b].split_once(':').ok_or("cosim: bad range")?;
                let hi: i64 = hi.trim().parse().map_err(|_| "cosim: non-constant range")?;
                let lo: i64 = lo.trim().parse().map_err(|_| "cosim: non-constant range")?;
                (hi - lo).unsigned_abs() as u32 + 1
            }
            _ => 1,
        };
        let pname = d.split_whitespace().last().unwrap_or("").to_string();
        ports.push(RtlPort {
            name: pname,
            input,
            width,
        });
    }
    Ok((name, ports))
}

fn bit(name: &str, width: u32, i: u32) -> String {
    if width == 1 { name.to_string() } else { format!("{name}_{i}") }
}

fn drive(sim: &mut Sim, p: &RtlPort, v: i64) -> Result<(), String> {
    for i in 0..p.width.min(64) {
        let b = bit(&p.name, p.width, i);
        let r = sim.drive_input(&b, (v >> i) & 1 == 1);
        // Bit 0 of a vector may keep the bare port name.
        if r.is_err() && i == 0 {
            sim.drive_input(&p.name, v & 1 == 1)?;
        } else if r.is_err() {
            // Bits synthesis proved unused have no net: nothing to drive.
            continue;
        }
    }
    Ok(())
}

/// Sample an output port, sign-extended from its width. `None` when any bit
/// is undriven (X).
fn sample(sim: &Sim, p: &RtlPort) -> Option<i64> {
    let mut v: u64 = 0;
    for i in 0..p.width.min(64) {
        let b = sim
            .sample(&bit(&p.name, p.width, i))
            .or_else(|| if i == 0 { sim.sample(&p.name) } else { None })?;
        v |= (b & 1) << i;
    }
    Some(sign_extend(v as i64, p.width))
}

fn sign_extend(v: i64, width: u32) -> i64 {
    if width >= 64 {
        return v;
    }
    let shift = 64 - width;
    (v << shift) >> shift
}

/// helion-sv keeps vector ports as one `Port` while their nets are
/// bit-blasted (`a_0`, `a_1`, ...). Give the simulator one port per bit so
/// every input bit can be driven and every output bit sampled.
fn bit_blast_ports(d: &mut helion_ir::Design) {
    use helion_ir::{Port, intern, resolve};
    let nets: std::collections::HashSet<String> =
        d.nets.iter().map(|n| resolve(n.name).to_string()).collect();
    let mut out = Vec::new();
    for p in &d.ports {
        let name = resolve(p.name).to_string();
        if nets.contains(&name) || !nets.contains(&format!("{name}_0")) {
            out.push(p.clone());
        }
        let mut i = 0;
        while nets.contains(&format!("{name}_{i}")) {
            out.push(Port {
                name: intern(&format!("{name}_{i}")),
                dir: p.dir,
                attrs: p.attrs.clone(),
            });
            i += 1;
        }
    }
    d.ports = out;
}

/// One `ap_memory` bank: registered address/enable in, one-cycle `q0` out,
/// `we0`/`d0` writes committed at the clock edge.
struct Bank {
    addr: RtlPort,
    ce: RtlPort,
    q: RtlPort,
    we: Option<RtlPort>,
    d: Option<RtlPort>,
    data: Vec<i64>,
}

/// An array argument split over its partition banks.
struct ArrMem {
    name: String,
    cfg: crate::schedule::ArrayBankConfig,
    banks: Vec<Bank>,
}

fn coords(mut flat: usize, dims: &[usize]) -> Vec<usize> {
    let mut out = vec![0usize; dims.len()];
    for i in (0..dims.len()).rev() {
        let d = dims[i].max(1);
        out[i] = flat % d;
        flat /= d;
    }
    out
}

impl ArrMem {
    fn total(&self) -> usize {
        self.cfg.dims.iter().product::<usize>().max(1)
    }

    fn load(&mut self, vals: &[i64]) {
        for b in &mut self.banks {
            b.data = vec![0; self.cfg.bank_depth.max(1)];
        }
        for e in 0..self.total() {
            let (b, o) = self.cfg.map_indices(&coords(e, &self.cfg.dims));
            if let Some(bank) = self.banks.get_mut(b) {
                if o >= bank.data.len() {
                    bank.data.resize(o + 1, 0);
                }
                bank.data[o] = vals.get(e).copied().unwrap_or(0);
            }
        }
    }

    fn contents(&self) -> Vec<i64> {
        (0..self.total())
            .map(|e| {
                let (b, o) = self.cfg.map_indices(&coords(e, &self.cfg.dims));
                self.banks.get(b).and_then(|bk| bk.data.get(o)).copied().unwrap_or(0)
            })
            .collect()
    }
}

/// Run C/RTL co-simulation of `source` on `vectors` (see the module docs).
pub fn run_rtl_cosim(source: &str, vectors: &[CosimTestVector]) -> Result<CosimReport, String> {
    run_rtl_cosim_with(source, &crate::HlsConfig::default(), vectors)
}

/// [`run_rtl_cosim`] with an explicit synthesis configuration (clock, part).
pub fn run_rtl_cosim_with(
    source: &str,
    config: &crate::HlsConfig,
    vectors: &[CosimTestVector],
) -> Result<CosimReport, String> {
    let hls = crate::hls_compile_with_config(source, config)?;
    if let crate::RtlKind::InterfaceShell { reason } = &hls.rtl {
        return Err(format!(
            "cosim: the RTL for `{}` is an interface shell, not a datapath ({reason} is not supported by the FSMD emitter); \
             co-simulating it would not check the C function",
            hls.cdfg.function_name
        ));
    }
    let (top, ports) = header_ports(&hls.sv)?;
    let mut netlist = hls.design.clone();
    bit_blast_ports(&mut netlist);
    let port = |n: &str| ports.iter().find(|p| p.name == n).cloned();
    let ap_start = port("ap_start");
    let ap_done = port("ap_done");
    let rst = port("rst");
    let rst_n = port("ap_rst_n");
    // Return value port: `ap_return` with handshake, `y` without.
    let ret_port = port("ap_return").or_else(|| port("y"));
    // Array arguments on ap_memory banks.
    let mut arr_template: Vec<ArrMem> = Vec::new();
    for cfg in hls.cdfg.array_configs.values().filter(|c| c.is_arg) {
        let n = cfg.num_banks.max(1);
        let mut banks = Vec::new();
        for b in 0..n {
            let p = if n <= 1 { cfg.name.clone() } else { format!("{}_{b}", cfg.name) };
            let (Some(addr), Some(ce), Some(q)) =
                (port(&format!("{p}_address0")), port(&format!("{p}_ce0")), port(&format!("{p}_q0")))
            else {
                return Err(format!(
                    "cosim: argument `{}` has no ap_memory port `{p}_address0` in module {top} (interface not supported by the RTL harness)",
                    cfg.name
                ));
            };
            banks.push(Bank { addr, ce, q, we: port(&format!("{p}_we0")), d: port(&format!("{p}_d0")), data: Vec::new() });
        }
        arr_template.push(ArrMem { name: cfg.name.clone(), cfg: cfg.clone(), banks });
    }
    let mut vector_results = Vec::with_capacity(vectors.len());
    let mut c_expected_all = Vec::with_capacity(vectors.len());
    let mut passed = 0usize;
    let mut lat_seen: Vec<u32> = Vec::new();
    for (vi, vec_in) in vectors.iter().enumerate() {
        let c_outputs = crate::evaluate_c_kernel_reference(source, &vec_in.inputs)?;
        c_expected_all.push(c_outputs.clone());
        let mut sim = Sim::new(&netlist);
        let mut mems: Vec<ArrMem> = arr_template
            .iter()
            .map(|m| ArrMem {
                name: m.name.clone(),
                cfg: m.cfg.clone(),
                banks: m
                    .banks
                    .iter()
                    .map(|b| Bank { addr: b.addr.clone(), ce: b.ce.clone(), q: b.q.clone(), we: b.we.clone(), d: b.d.clone(), data: Vec::new() })
                    .collect(),
            })
            .collect();
        for m in &mut mems {
            let vals = vec_in.inputs.get(&m.name).cloned().unwrap_or_default();
            m.load(&vals);
        }
        // Scalars to same-named input ports.
        for (name, vals) in &vec_in.inputs {
            if mems.iter().any(|m| &m.name == name) {
                continue;
            }
            match port(name) {
                Some(p) if p.input => drive(&mut sim, &p, vals.first().copied().unwrap_or(0))?,
                _ => {
                    return Err(format!(
                        "cosim: argument `{name}` has no RTL port in module {top} (interface not supported by the RTL harness)"
                    ));
                }
            }
        }
        // Reset.
        if let Some(r) = &rst {
            drive(&mut sim, r, 1)?;
        }
        if let Some(r) = &rst_n {
            drive(&mut sim, r, 0)?;
        }
        for _ in 0..2 {
            sim.step_posedge(10_000);
        }
        if let Some(r) = &rst {
            drive(&mut sim, r, 0)?;
        }
        if let Some(r) = &rst_n {
            drive(&mut sim, r, 1)?;
        }
        let mut rtl_outputs: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        let ptr_outs: Vec<String> = c_outputs
            .keys()
            .filter(|k| k.as_str() != "return" && !mems.iter().any(|m| &m.name == *k))
            .cloned()
            .collect();
        let capture = |sim: &Sim, outs: &mut BTreeMap<String, Vec<i64>>, only_vld: bool| {
            for name in &ptr_outs {
                let Some(p) = port(name).filter(|p| !p.input) else { continue };
                let vld = port(&format!("{name}_ap_vld"));
                let strobe = vld.as_ref().and_then(|v| sample(sim, v)).map(|v| v != 0);
                if only_vld && strobe != Some(true) {
                    continue;
                }
                if !only_vld && vld.is_some() && outs.contains_key(name) {
                    // A strobed output keeps the value it was written with.
                    continue;
                }
                if let Some(v) = sample(sim, &p) {
                    outs.insert(name.clone(), vec![v]);
                }
            }
        };
        // One clock: sample the memory requests, step, then commit writes
        // and return read data (one-cycle BRAM latency).
        let clock = |sim: &mut Sim, mems: &mut Vec<ArrMem>| -> Result<(), String> {
            let mut reads: Vec<(usize, usize, usize)> = Vec::new();
            let mut writes: Vec<(usize, usize, usize, i64)> = Vec::new();
            for (mi, m) in mems.iter().enumerate() {
                for (bi, b) in m.banks.iter().enumerate() {
                    if !sample(sim, &b.ce).is_some_and(|v| v != 0) {
                        continue;
                    }
                    let a = sample(sim, &b.addr).map(|a| (a as u64 & ((1u64 << b.addr.width.min(63)) - 1)) as usize).unwrap_or(0);
                    let we = b.we.as_ref().and_then(|w| sample(sim, w)).is_some_and(|v| v != 0);
                    if we {
                        let d = b.d.as_ref().and_then(|d| sample(sim, d)).unwrap_or(0);
                        writes.push((mi, bi, a, d));
                    } else {
                        reads.push((mi, bi, a));
                    }
                }
            }
            sim.step_posedge(10_000);
            for (mi, bi, a, d) in writes {
                let data = &mut mems[mi].banks[bi].data;
                if a >= data.len() {
                    data.resize(a + 1, 0);
                }
                data[a] = d;
            }
            for (mi, bi, a) in reads {
                let b = &mems[mi].banks[bi];
                let v = b.data.get(a).copied().unwrap_or(0);
                let q = b.q.clone();
                drive(sim, &q, v)?;
            }
            Ok(())
        };
        let mut cycles = 0u32;
        let mut done = false;
        let latency;
        // An FSMD has a known worst-case latency: past twice that, ap_done
        // is not coming (fail fast instead of burning the global cap).
        let cap = match &hls.rtl {
            crate::RtlKind::Fsmd { cycle_bound, .. } => (cycle_bound.saturating_mul(2) + 64).min(COSIM_MAX_CYCLES as u64) as u32,
            _ => COSIM_MAX_CYCLES,
        };
        if let (Some(start), Some(done_p)) = (&ap_start, &ap_done) {
            drive(&mut sim, start, 1)?;
            while cycles < cap {
                clock(&mut sim, &mut mems)?;
                cycles += 1;
                if cycles == 1 {
                    drive(&mut sim, start, 0)?;
                }
                capture(&sim, &mut rtl_outputs, true);
                if sample(&sim, done_p).is_some_and(|v| v != 0) {
                    done = true;
                    break;
                }
            }
            latency = cycles;
        } else {
            // No block-level handshake: run the RTL's worst-case latency
            // (the FSMD bound, or the direct datapath's scheduled latency).
            let lat = match &hls.rtl {
                crate::RtlKind::Fsmd { cycle_bound, .. } => (*cycle_bound).min(COSIM_MAX_CYCLES as u64) as u32,
                crate::RtlKind::DirectDatapath { depth } => (*depth).max(hls.schedule.max_latency_cycles).max(1),
                _ => hls.schedule.max_latency_cycles.max(1),
            };
            for _ in 0..lat {
                clock(&mut sim, &mut mems)?;
                capture(&sim, &mut rtl_outputs, true);
            }
            cycles = lat;
            done = true;
            latency = lat;
        }
        capture(&sim, &mut rtl_outputs, false);
        if c_outputs.contains_key("return") {
            if let Some(v) = ret_port.as_ref().and_then(|p| sample(&sim, p)) {
                rtl_outputs.insert("return".into(), vec![v]);
            }
        }
        for m in &mems {
            if c_outputs.contains_key(&m.name) {
                rtl_outputs.insert(m.name.clone(), m.contents());
            }
        }
        if vec_in.inject_rtl_fault_delta != 0 {
            for vals in rtl_outputs.values_mut() {
                if let Some(first) = vals.first_mut() {
                    *first = first.wrapping_add(vec_in.inject_rtl_fault_delta);
                }
            }
        }
        // Compare at each RTL output's width.
        let mut matched = done;
        for (name, cv) in &c_outputs {
            let w = if name == "return" {
                ret_port.as_ref().map(|p| p.width)
            } else if let Some(m) = mems.iter().find(|m| &m.name == name) {
                m.banks.first().and_then(|b| b.d.as_ref().or(Some(&b.q))).map(|p| p.width)
            } else {
                port(name).map(|p| p.width)
            }
            .unwrap_or(64);
            let want: Vec<i64> = cv.iter().map(|v| sign_extend(*v, w)).collect();
            let got: Option<Vec<i64>> = rtl_outputs.get(name).map(|v| v.iter().map(|x| sign_extend(*x, w)).collect());
            if got.as_deref() != Some(want.as_slice()) {
                matched = false;
            }
        }
        if matched {
            passed += 1;
        }
        lat_seen.push(latency);
        let _ = cycles;
        vector_results.push(CosimVectorResult {
            vector_index: vi,
            c_outputs,
            rtl_outputs,
            latency_cycles: latency,
            ii_cycles: hls.schedule.achieved_ii,
            matched,
        });
    }
    let status = if passed == vectors.len() && !vectors.is_empty() {
        CosimStatus::Pass
    } else {
        CosimStatus::Fail
    };
    let min_lat = lat_seen.iter().copied().min().unwrap_or(0);
    let max_lat = lat_seen.iter().copied().max().unwrap_or(0);
    let avg_lat = if lat_seen.is_empty() {
        0
    } else {
        (lat_seen.iter().map(|&l| l as u64).sum::<u64>() / lat_seen.len() as u64) as u32
    };
    let ii = hls.schedule.achieved_ii;
    let testbench_sv = crate::cosim::generate_cosim_testbench(&hls, vectors, &c_expected_all);
    let mut report_text = format!(
        "================================================================\n\
         == C/RTL Co-Simulation Report for '{}' (helion-sim, gate netlist)\n\
         ================================================================\n\
         | RTL       | Status | min Latency | avg Latency | max Latency | II |\n\
         | Verilog   | {:<6} | {:>11} | {:>11} | {:>11} | {:>2} |\n\
         ================================================================\n",
        hls.cdfg.function_name,
        if status == CosimStatus::Pass { "PASS" } else { "FAIL" },
        min_lat,
        avg_lat,
        max_lat,
        ii
    );
    for r in &vector_results {
        if !r.matched {
            report_text.push_str(&format!(
                "MISMATCH vector #{}: C {:?} RTL {:?}\n",
                r.vector_index, r.c_outputs, r.rtl_outputs
            ));
        }
    }
    Ok(CosimReport {
        function_name: hls.cdfg.function_name.clone(),
        status,
        vectors_tested: vectors.len(),
        vectors_passed: passed,
        min_latency_cycles: min_lat,
        max_latency_cycles: max_lat,
        avg_latency_cycles: avg_lat,
        measured_ii_cycles: ii,
        vector_results,
        m_axi_bursts: crate::extract_kernel_m_axi_bursts(&hls, 0),
        testbench_sv,
        report_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_parser_reads_widths() {
        let (n, p) = header_ports("// x\nmodule k(input logic clk, input logic [26:0] a, output logic [26:0] y);\nendmodule").unwrap();
        assert_eq!(n, "k");
        assert_eq!(p.len(), 3);
        assert_eq!(p[1], RtlPort { name: "a".into(), input: true, width: 27 });
        assert!(!p[2].input);
    }

    #[test]
    fn scalar_kernel_matches_c_in_the_gate_simulator() {
        let src = "int addab(int a, int b) { return a + b; }";
        let v = vec![
            CosimTestVector::new().with_scalar("a", 5).with_scalar("b", 7),
            CosimTestVector::new().with_scalar("a", -20).with_scalar("b", 4),
        ];
        let r = run_rtl_cosim(src, &v).unwrap();
        assert_eq!(r.vector_results[0].rtl_outputs["return"], vec![12], "{}", r.report_text);
        assert_eq!(r.vector_results[1].rtl_outputs["return"], vec![-16]);
        assert_eq!(r.status, CosimStatus::Pass);
    }

    #[test]
    fn real_simulation_passes_good_rtl_and_flags_bad_rtl() {
        let ok = ["int k1(int a, int b, int c) { return a + b + c; }", "int k4(int a, int b) { return (a ^ b) & 255; }"];
        for src in ok {
            let mut t = CosimTestVector::new().with_scalar("a", 6).with_scalar("b", 7);
            if src.contains("int c") {
                t = t.with_scalar("c", 9);
            }
            assert_eq!(run_rtl_cosim(src, &[t]).unwrap().status, CosimStatus::Pass, "{src}");
        }
        // A deliberately corrupted RTL output must be flagged, not copied
        // from C.
        let mut bad = CosimTestVector::new().with_scalar("a", 6).with_scalar("b", 7).with_scalar("c", 1);
        bad.inject_rtl_fault_delta = 3;
        let r = run_rtl_cosim(ok[0], &[bad]).unwrap();
        assert_eq!(r.status, CosimStatus::Fail);
        assert!(r.report_text.contains("MISMATCH"));
    }

    /// Regression: the clamp kernel's `if (...) return ...;` branches used to
    /// be dropped by the flat datapath emitter (`y <= x` unconditionally).
    #[test]
    fn clamp_branches_are_if_converted_into_the_fsm() {
        let clamp = "int clampv(int x, int lo, int hi) { if (x < lo) return lo; if (x > hi) return hi; return x; }";
        let hls = crate::hls_compile(clamp).unwrap();
        assert!(matches!(hls.rtl, crate::RtlKind::Fsmd { .. }), "{:?}", hls.rtl);
        let v: Vec<CosimTestVector> = [(50, 0, 10, 10), (-5, 0, 10, 0), (7, 0, 10, 7), (-20, -30, -10, -20), (-40, -30, -10, -30)]
            .iter()
            .map(|&(x, lo, hi, _)| CosimTestVector::new().with_scalar("x", x).with_scalar("lo", lo).with_scalar("hi", hi))
            .collect();
        let r = run_rtl_cosim(clamp, &v).unwrap();
        assert_eq!(r.status, CosimStatus::Pass, "{}\n{}", r.report_text, hls.sv);
        assert_eq!(r.vector_results[0].rtl_outputs["return"], vec![10]);
        assert_eq!(r.vector_results[4].rtl_outputs["return"], vec![-30]);
    }

    #[test]
    fn handshake_kernel_runs_to_ap_done_with_memory_banks() {
        let src = r#"
void fir_mac_cosim(int a[4], int b[4], int bias, int *out) {
  #pragma HLS PIPELINE II=1
  #pragma HLS ARRAY_PARTITION variable=a complete
  #pragma HLS ARRAY_PARTITION variable=b complete
  int acc = bias;
  for (int i = 0; i < 4; i++) {
    acc += a[i] * b[i];
  }
  *out = acc;
}
"#;
        let v = CosimTestVector::new()
            .with_array("a", vec![1, 2, 3, 4])
            .with_array("b", vec![10, 20, 30, 40])
            .with_scalar("bias", 5);
        let r = run_rtl_cosim(src, &[v]).unwrap();
        assert_eq!(r.vector_results[0].c_outputs["out"], vec![305]);
        // Regression: this FIR MAC used to read back 0 (the shell datapath
        // never multiplied the banks); the netlist must now compute it.
        assert_eq!(r.vector_results[0].rtl_outputs["out"], vec![305], "{}", r.report_text);
        assert_eq!(r.status, CosimStatus::Pass);
        // Latency is measured from ap_start to ap_done in the simulator.
        assert!(r.vector_results[0].latency_cycles > 0, "{}", r.report_text);
    }
}
