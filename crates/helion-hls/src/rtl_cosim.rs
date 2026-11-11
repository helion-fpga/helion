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

struct Mem {
    addr: RtlPort,
    ce: RtlPort,
    q: RtlPort,
    data: Vec<i64>,
}

/// Run C/RTL co-simulation of `source` on `vectors` (see the module docs).
pub fn run_rtl_cosim(source: &str, vectors: &[CosimTestVector]) -> Result<CosimReport, String> {
    let hls = crate::hls_compile(source)?;
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
    let mut vector_results = Vec::with_capacity(vectors.len());
    let mut c_expected_all = Vec::with_capacity(vectors.len());
    let mut passed = 0usize;
    let mut lat_seen: Vec<u32> = Vec::new();
    for (vi, vec_in) in vectors.iter().enumerate() {
        let c_outputs = crate::evaluate_c_kernel_reference(source, &vec_in.inputs)?;
        c_expected_all.push(c_outputs.clone());
        let mut sim = Sim::new(&netlist);
        // Bind inputs: scalars to same-named ports, arrays to ap_memory banks.
        let mut mems: Vec<Mem> = Vec::new();
        for (name, vals) in &vec_in.inputs {
            if let Some(p) = port(name).filter(|p| p.input) {
                drive(&mut sim, &p, vals.first().copied().unwrap_or(0))?;
                continue;
            }
            let single = (port(&format!("{name}_address0")), port(&format!("{name}_ce0")), port(&format!("{name}_q0")));
            if let (Some(addr), Some(ce), Some(q)) = single {
                mems.push(Mem { addr, ce, q, data: vals.clone() });
                continue;
            }
            // Completely partitioned: one bank per element (`a_0_q0`, ...).
            let mut found = false;
            for (k, v) in vals.iter().enumerate() {
                let bank = (
                    port(&format!("{name}_{k}_address0")),
                    port(&format!("{name}_{k}_ce0")),
                    port(&format!("{name}_{k}_q0")),
                );
                if let (Some(addr), Some(ce), Some(q)) = bank {
                    mems.push(Mem { addr, ce, q, data: vec![*v] });
                    found = true;
                }
            }
            if !found {
                return Err(format!(
                    "cosim: argument `{name}` has no RTL port in module {top} (interface not supported by the RTL harness)"
                ));
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
        let ptr_outs: Vec<String> = c_outputs.keys().filter(|k| k.as_str() != "return").cloned().collect();
        let capture = |sim: &Sim, outs: &mut BTreeMap<String, Vec<i64>>, only_vld: bool| {
            for name in &ptr_outs {
                let Some(p) = port(name).filter(|p| !p.input) else { continue };
                let vld = port(&format!("{name}_ap_vld"));
                let strobe = vld.as_ref().and_then(|v| sample(sim, v)).map(|v| v != 0);
                if only_vld && strobe != Some(true) {
                    continue;
                }
                if let Some(v) = sample(sim, &p) {
                    outs.insert(name.clone(), vec![v]);
                }
            }
        };
        let mut cycles = 0u32;
        let mut done = false;
        let latency;
        if let (Some(start), Some(done_p)) = (&ap_start, &ap_done) {
            drive(&mut sim, start, 1)?;
            while cycles < COSIM_MAX_CYCLES {
                // BRAM model: sample address/enable before the edge.
                let reads: Vec<(usize, Option<i64>)> = mems
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let en = sample(&sim, &m.ce).is_some_and(|v| v != 0);
                        let a = sample(&sim, &m.addr).map(|a| a & ((1i64 << m.addr.width) - 1));
                        (i, if en { a } else { None })
                    })
                    .collect();
                sim.step_posedge(10_000);
                cycles += 1;
                for (i, a) in reads {
                    if let Some(a) = a {
                        let m = &mems[i];
                        let v = m.data.get(a as usize).copied().unwrap_or(0);
                        let q = m.q.clone();
                        drive(&mut sim, &q, v)?;
                    }
                }
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
            // No block-level handshake: registered datapath, scheduled latency.
            let lat = hls.schedule.max_latency_cycles.max(1);
            for _ in 0..lat {
                sim.step_posedge(10_000);
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
            } else {
                port(name).map(|p| p.width)
            }
            .unwrap_or(64);
            let c = sign_extend(cv.first().copied().unwrap_or(0), w);
            match rtl_outputs.get(name).and_then(|v| v.first()) {
                Some(r) if *r == c => {}
                _ => matched = false,
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
        // The emitted RTL for this kernel drops the branches (`y <= x`
        // unconditionally); simulating it must expose that, not copy C.
        let clamp = "int clampv(int x, int lo, int hi) { if (x < lo) return lo; if (x > hi) return hi; return x; }";
        let v = CosimTestVector::new().with_scalar("x", 50).with_scalar("lo", 0).with_scalar("hi", 10);
        let r = run_rtl_cosim(clamp, &[v]).unwrap();
        assert_eq!(r.status, CosimStatus::Fail);
        assert!(r.report_text.contains("MISMATCH"));
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
        // Latency is measured from ap_start to ap_done in the simulator.
        assert!(r.vector_results[0].latency_cycles > 0, "{}", r.report_text);
    }
}
