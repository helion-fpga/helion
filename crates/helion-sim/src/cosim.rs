//! Automated RTL-vs-Post-Synth / Post-Route Golden Co-Simulation (`run_golden_cosim`).
//!
//! Steps a golden reference [`Design`] and a candidate transformed/post-synth/post-route
//! [`Design`] in lockstep cycle-by-cycle under identical input stimulus, comparing all
//! primary outputs (`PortDir::Out`) and `led` on every cycle and pinpointing the exact
//! first diverging cycle and signal.

use crate::Sim;
use helion_ir::Design;

/// One primary-output divergence between the golden and candidate designs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CosimMismatch {
    pub cycle: u64,
    pub time_ps: u64,
    pub signal: String,
    pub golden_value: Option<u64>,
    pub candidate_value: Option<u64>,
}

impl std::fmt::Display for CosimMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[COSIM][MISMATCH] t={}ps cycle={} signal={} golden={:?} candidate={:?}",
            self.time_ps, self.cycle, self.signal, self.golden_value, self.candidate_value
        )
    }
}

/// Result of a cycle-by-cycle golden co-simulation run ([`run_golden_cosim`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoldenCosimReport {
    pub cycles_run: u32,
    pub compared_outputs: Vec<String>,
    pub total_comparisons: u64,
    pub first_divergence: Option<CosimMismatch>,
    pub mismatches: Vec<CosimMismatch>,
}

impl GoldenCosimReport {
    /// True if every primary output agreed across all simulated cycles.
    pub fn is_equivalent(&self) -> bool {
        self.first_divergence.is_none()
    }

    /// Machine-readable summary line for logs and Tcl `verify_cosim`.
    pub fn summary_line(&self) -> String {
        match &self.first_divergence {
            None => format!(
                "cosim PASS cycles={} outputs={} comparisons={} mismatches=0",
                self.cycles_run,
                self.compared_outputs.len(),
                self.total_comparisons
            ),
            Some(first) => format!(
                "cosim FAIL cycles={} outputs={} mismatches={} first_cycle={} first_signal={} golden={:?} candidate={:?}",
                self.cycles_run,
                self.compared_outputs.len(),
                self.mismatches.len(),
                first.cycle,
                first.signal,
                first.golden_value,
                first.candidate_value
            ),
        }
    }
}

/// Run automated cycle-by-cycle co-simulation comparing `golden` and `candidate` for
/// `cycles` clock posedges (default 10ps clock period, deterministic auto-generated stimulus
/// on shared non-clock primary inputs).
pub fn run_golden_cosim(golden: &Design, candidate: &Design, cycles: u32) -> GoldenCosimReport {
    let g_sim = Sim::new(golden);
    let c_sim = Sim::new(candidate);

    // Discover shared non-clock input ports so combinational/sequential inputs are exercised
    // deterministically when no explicit stimulus vector is supplied.
    let c_inputs: std::collections::HashSet<String> = c_sim.input_ports().into_iter().collect();
    let shared_inputs: Vec<String> = g_sim
        .input_ports()
        .into_iter()
        .filter(|p| !p.eq_ignore_ascii_case("clk") && c_inputs.contains(p))
        .collect();

    let mut stimulus = Vec::with_capacity(cycles as usize);
    for cyc in 0..cycles as usize {
        let mut row = Vec::with_capacity(shared_inputs.len());
        for (idx, port) in shared_inputs.iter().enumerate() {
            // Deterministic LFSR/Gray-like stimulus pattern covering input combinations.
            let bit = (((cyc + 1).wrapping_mul(0x9E37_79B9usize) >> (idx % 16)) ^ (cyc >> idx)) & 1 != 0;
            row.push((port.clone(), bit));
        }
        stimulus.push(row);
    }

    run_golden_cosim_with_stimulus(golden, candidate, cycles, 10, &stimulus)
        .expect("shared inputs exist on both designs")
}

/// Run cycle-by-cycle golden co-simulation with explicit per-cycle input stimulus
/// (`stimulus[cycle_idx]` = list of `(port_name, value)` pairs applied before that posedge).
pub fn run_golden_cosim_with_stimulus(
    golden: &Design,
    candidate: &Design,
    cycles: u32,
    period_ps: u64,
    stimulus: &[Vec<(String, bool)>],
) -> Result<GoldenCosimReport, String> {
    let period = period_ps.max(1);
    let mut g_sim = Sim::new(golden);
    let mut c_sim = Sim::new(candidate);

    // Collect union of primary output ports present on both designs, plus `"led"`.
    let c_outs: std::collections::HashSet<String> = c_sim.output_ports().into_iter().collect();
    let mut compared_outputs: Vec<String> = g_sim
        .output_ports()
        .into_iter()
        .filter(|p| c_outs.contains(p))
        .collect();
    if !compared_outputs.iter().any(|s| s.eq_ignore_ascii_case("led")) {
        compared_outputs.insert(0, "led".to_string());
    }
    compared_outputs.sort();
    compared_outputs.dedup();

    let mut total_comparisons = 0u64;
    let mut first_divergence: Option<CosimMismatch> = None;
    let mut mismatches = Vec::new();

    for cyc in 0..cycles as usize {
        if let Some(assignments) = stimulus.get(cyc) {
            for (port, val) in assignments {
                g_sim.drive_input(port, *val)?;
                c_sim.drive_input(port, *val)?;
            }
        }

        g_sim.step_posedge(period);
        c_sim.step_posedge(period);

        for sig in &compared_outputs {
            total_comparisons += 1;
            let gv = if sig == "led" {
                Some(u64::from(g_sim.led))
            } else {
                g_sim.sample(sig)
            };
            let cv = if sig == "led" {
                Some(u64::from(c_sim.led))
            } else {
                c_sim.sample(sig)
            };
            if gv != cv {
                let mm = CosimMismatch {
                    cycle: g_sim.cycle,
                    time_ps: g_sim.time,
                    signal: sig.clone(),
                    golden_value: gv,
                    candidate_value: cv,
                };
                if first_divergence.is_none() {
                    first_divergence = Some(mm.clone());
                }
                mismatches.push(mm);
            }
        }
    }

    Ok(GoldenCosimReport {
        cycles_run: cycles,
        compared_outputs,
        total_comparisons,
        first_divergence,
        mismatches,
    })
}
