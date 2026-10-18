//! Event-driven kernel driven by LUT INIT + FF in the netlist (not a hardcoded toggle).
//!
//! Overlay / behavioral LED+waveform completion is labeled **`overlay` / `sim_DONE`**
//! ([`OverlaySimReport`]) — never bare board DONE.

use helion_ir::{intern, resolve, CellKind, Design, PortDir};
use helion_sv::sva::{SvaImmProp, SvaProperty};
use std::collections::HashMap;

pub mod coverage;
pub mod fast;
pub mod sdf;
pub mod clocking;
pub mod cosim;
pub mod dpi;
pub mod crv_coverage;
pub mod sva;
pub mod svtb;
pub mod vcd;
pub mod wave;

pub use coverage::{
    BranchCoverageStats, CoverageCollector, FsmCoverageStats, FunctionalCoverageReport,
    NetToggleStats,
};
pub use fast::{
    CompiledFf, CompiledNode, CompiledSim, ConeNodeKind, LevelizedSchedule, Logic4, NET_ONE,
    NET_UNCONNECTED, NET_ZERO,
};
pub use sdf::{
    parse_sdf, SdfCorner, SdfDelayPs, SdfFile, SdfInterconnect, SdfIoPath, SdfTimingCheck,
    SdfTimingCheckKind, SimTimingModel, TimingViolation, TimingViolationKind,
};
pub use sva::{
    CoverStatus, ImmChecker, ImmStatus, PropChecker, PropStatus, SvaEngine, SvaFailure,
    SvaFailureKind, SvaReport, SVA_FAILURE_CAP,
};
pub use wave::{
    decode_axi_lite, decode_axi_stream, decode_i2c, decode_spi, decode_uart, decode_wishbone,
    AxiLiteSignals, AxiLiteTransaction, AxiLiteTxKind, AxiStreamPacket, AxiStreamSignals,
    CompactWaveStream, I2cByteTransfer, I2cSignals, I2cTransaction, ProtocolTransaction,
    SpiConfig, SpiTransaction, UartConfig, UartFrame, UartParity, WaveSample, WaveSignalDef,
    WaveSignalId, WaveSlice, WishboneSignals, WishboneTransaction,
};

pub use clocking::{
    ClockingBlock, ClockingSignal, ClockingSkew, ClockingVarDir, ScheduledDrive,
};
pub use cosim::{
    run_golden_cosim, run_golden_cosim_with_stimulus, CosimMismatch, GoldenCosimReport,
};
pub use dpi::{
    DpiCallRecord, DpiParamDecl, DpiParamDir, DpiRegistry, DpiSignature, DpiType, DpiValue,
};
pub use svtb::{ScoreboardMismatch, SimMailbox, SimScoreboard, SimSemaphore};
pub mod verilate;
pub use crv_coverage::{
    BinKind, BinSpec, BinStatus, CovergroupEngine, CovergroupReport, CoverpointReport,
    CoverpointSpec, CrossReport, CrossSpec, DistItem, DistWeightKind, DomainIntervals,
    IllegalBinViolation, NamedConstraintBlock, RandClass, RandConstraint, RandConstraintSolver,
    RandExpr, RandMode, RandVarSpec,
};
pub use verilate::{
    emit_cpp_cycle_model, emit_rust_cycle_model, sanitize_ident, VpiFormat, VpiHandle,
    VpiObjectInfo, VpiObjectKind, VpiPutFlags, VpiRegistry, VpiValue, VpiVecVal,
};
/// UG900 force vs deposit: hold until `release`, or one-shot poke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimForceKind {
    Force,
    Deposit,
}

/// One UG900 force/deposit on a helion-sim probe (LED or FF Q).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimForce {
    pub name: String,
    pub kind: SimForceKind,
    pub value: u64,
}

/// What drives a net, for ILA probing of arbitrary (incl. combinational) nets.
#[derive(Clone, Debug, PartialEq, Eq)]
enum NetDriver {
    /// Net driven by an HFF Q pin — value is the live FF state.
    Ff(String),
    /// Net driven by a LUT O pin — value is the INIT evaluated on live inputs.
    Lut(String),
    /// No resolvable driver (top-level input port, undriven, …) — probes read X.
    Undriven,
}

#[derive(Clone, Debug)]
pub struct Sim {
    pub time: u64,
    /// Posedge count (cycle 1 = first [`Sim::step_posedge`]). SVA failures
    /// timestamp against this.
    pub cycle: u64,
    lut_init: HashMap<String, u64>,
    ff_d_lut: HashMap<String, String>,
    iob_from_ff: Option<String>,
    pub ff_q: HashMap<String, bool>,
    pub led: bool,
    /// Continuous UG900 forces (name → value). Re-applied after each posedge.
    forces: HashMap<String, u64>,
    /// LUT → [(input pin, net on that pin)] for all 6 pins (built for every
    /// LUT I-pin, not just FF-driven ones like the old `lut_pins`).
    lut_pin_nets: HashMap<String, Vec<(u8, String)>>,
    /// Net → cell whose Q drives it (combinational resolution root).
    net_ff: HashMap<String, String>,
    /// Net → LUT whose O drives it (multi-level LUT chains evaluate exactly).
    net_lut_o: HashMap<String, String>,
    /// Testbench stimulus: top-level input port name (= its net) → driven bit.
    input_values: HashMap<String, bool>,
    /// SVA stimulus: input port name → driven bit (set via [`Sim::drive`]).
    /// Checkers sample `InputBit` probes from here.
    stim: HashMap<String, bool>,
    /// SVA checker sidecars (v3.0 verification track). Empty until
    /// [`Sim::attach_sva`].
    sva: SvaEngine,
    /// Top-level port name → direction, for [`Sim::drive_input`] validation.
    ports: HashMap<String, PortDir>,
    /// Output IOB PAD net → net on the IOB's I pin (output-port sampling).
    iob_pad_to_i: HashMap<String, String>,
    /// ILA probe support: driver per net + LUT input net per pin (all 6 pins).
    net_drivers: HashMap<String, NetDriver>,
    lut_input_nets: HashMap<String, [Option<String>; 6]>,
    /// Levelized flat 2-state / 4-state compiled simulator sidecar.
    compiled: CompiledSim,
    /// Optional SDF back-annotated gate-level timing model.
    timing: Option<SimTimingModel>,
    /// Runtime functional coverage collector (toggle, branch, FSM).
    coverage: CoverageCollector,
    /// DPI-C foreign function registry attached to this simulator.
    dpi: DpiRegistry,
    /// IEEE 1364 / 1800 VPI signal introspection registry.
    vpi: VpiRegistry,
}

impl Sim {
    pub fn new(d: &Design) -> Self {
        let mut lut_init = HashMap::new();
        for c in &d.cells {
            if let CellKind::Lut6 { init } = c.kind {
                lut_init.insert(resolve(c.name).to_string(), init);
            }
        }
        let mut lut_pin_nets: HashMap<String, Vec<(u8, String)>> = HashMap::new();
        let mut net_ff: HashMap<String, String> = HashMap::new();
        let mut net_lut_o: HashMap<String, String> = HashMap::new();
        let mut ff_d_lut = HashMap::new();
        for n in &d.nets {
            if let Some(q) = n.endpoints.iter().find(|e| e.pin == intern("Q")) {
                net_ff.insert(resolve(n.name).to_string(), resolve(q.cell).to_string());
            }
            if let Some(o) = n
                .endpoints
                .iter()
                .find(|e| e.pin == intern("O") && lut_init.contains_key(resolve(e.cell)))
            {
                net_lut_o.insert(resolve(n.name).to_string(), resolve(o.cell).to_string());
            }
            for e in &n.endpoints {
                if let Some(rest) = resolve(e.pin).strip_prefix('I') {
                    if let Ok(pin) = rest.parse::<u8>() {
                        if pin < 6 {
                            lut_pin_nets
                                .entry(resolve(e.cell).to_string())
                                .or_default()
                                .push((pin, resolve(n.name).to_string()));
                        }
                    }
                }
            }
            let lut_o = n
                .endpoints
                .iter()
                .find(|e| e.pin == intern("O") && lut_init.contains_key(resolve(e.cell)));
            let ff_d = n.endpoints.iter().find(|e| e.pin == intern("D"));
            if let (Some(l), Some(f)) = (lut_o, ff_d) {
                ff_d_lut.insert(resolve(f.cell).to_string(), resolve(l.cell).to_string());
            }
        }
        let pin_ix = d.pin_index();
        let iob_from_ff = d.cells.iter().find_map(|c| {
            if !matches!(c.kind, CellKind::IobOut) {
                return None;
            }
            let net = pin_ix.net_on(resolve(c.name), "I")?;
            d.net(resolve(net))?
                .endpoints
                .iter()
                .find(|e| e.pin == intern("Q"))
                .map(|e| resolve(e.cell).to_string())
        });
        let mut ff_q = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                ff_q.insert(resolve(c.name).to_string(), false);
            }
        }
        let mut ports = HashMap::new();
        for p in &d.ports {
            ports.insert(resolve(p.name).to_string(), p.dir);
        }
        let mut iob_pad_to_i = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::IobOut) {
                if let (Some(pad), Some(i)) =
                    (pin_ix.net_on(resolve(c.name), "PAD"), pin_ix.net_on(resolve(c.name), "I"))
                {
                    iob_pad_to_i.insert(resolve(pad).to_string(), resolve(i).to_string());
                }
            }
        }
        // ILA probe maps: driver per net (Q beats O on multi-endpoint nets) and
        // the net on every LUT input pin so chained LUTs evaluate exactly.
        let mut net_drivers = HashMap::new();
        for n in &d.nets {
            let driver = n
                .endpoints
                .iter()
                .find(|e| e.pin == intern("Q"))
                .map(|e| NetDriver::Ff(resolve(e.cell).to_string()))
                .or_else(|| {
                    n.endpoints
                        .iter()
                        .find(|e| e.pin == intern("O"))
                        .map(|e| NetDriver::Lut(resolve(e.cell).to_string()))
                })
                .unwrap_or(NetDriver::Undriven);
            net_drivers.insert(resolve(n.name).to_string(), driver);
        }
        let mut lut_input_nets: HashMap<String, [Option<String>; 6]> = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Lut6 { .. }) {
                let mut pins: [Option<String>; 6] = Default::default();
                for (i, slot) in pins.iter_mut().enumerate() {
                    *slot = pin_ix
                        .net_on(resolve(c.name), &format!("I{i}"))
                        .map(|s| resolve(s).to_string());
                }
                lut_input_nets.insert(resolve(c.name).to_string(), pins);
            }
        }
        let compiled = CompiledSim::new(d);
        let coverage = CoverageCollector::from_design(d, &compiled);
        let vpi = VpiRegistry::new(d);
        Self {
            time: 0,
            cycle: 0,
            lut_init,
            ff_d_lut,
            iob_from_ff,
            ff_q,
            led: false,
            forces: HashMap::new(),
            lut_pin_nets,
            net_ff,
            net_lut_o,
            input_values: HashMap::new(),
            stim: HashMap::new(),
            sva: SvaEngine::new(),
            ports,
            iob_pad_to_i,
            net_drivers,
            lut_input_nets,
            compiled,
            timing: None,
            coverage,
            dpi: DpiRegistry::new(),
            vpi,
        }
    }

    /// Combinational value of a net: driven input → FF Q → LUT chain → false.
    /// `memo`/`stack` guard combinational loops (read as 0, as before).
    fn pin_net_value(
        &self,
        net: &str,
        memo: &mut HashMap<String, bool>,
        stack: &mut Vec<String>,
    ) -> bool {
        if let Some(&v) = memo.get(net) {
            return v;
        }
        if let Some(&v) = self.input_values.get(net) {
            memo.insert(net.to_string(), v);
            return v;
        }
        if let Some(ff) = self.net_ff.get(net) {
            let v = self.ff_q.get(ff).copied().unwrap_or(false);
            memo.insert(net.to_string(), v);
            return v;
        }
        if stack.iter().any(|s| s == net) {
            return false;
        }
        if let Some(lut) = self.net_lut_o.get(net).cloned() {
            stack.push(net.to_string());
            let v = self.lut_output_live(&lut, memo, stack);
            stack.pop();
            memo.insert(net.to_string(), v);
            return v;
        }
        memo.insert(net.to_string(), false);
        false
    }

    /// Live LUT output with driven inputs and multi-level chains resolved.
    fn lut_output_live(
        &self,
        lut: &str,
        memo: &mut HashMap<String, bool>,
        stack: &mut Vec<String>,
    ) -> bool {
        let init = self.lut_init.get(lut).copied().unwrap_or(0);
        let mut addr = 0u64;
        if let Some(pins) = self.lut_pin_nets.get(lut) {
            for (pin, net) in pins {
                if self.pin_net_value(net, memo, stack) {
                    addr |= 1 << pin;
                }
            }
        }
        (init >> addr) & 1 == 1
    }

    /// Drive a top-level input (or inout) port. The value flows through
    /// combinational LUT chains on the next [`Sim::step_posedge`], and
    /// [`Sim::sample`] reads it back immediately.
    pub fn drive_input(&mut self, port: &str, value: bool) -> Result<(), String> {
        match self.ports.get(port) {
            Some(PortDir::In) | Some(PortDir::Inout) => {
                self.input_values.insert(port.to_string(), value);
                let _ = self.compiled.drive_input(port, value);
                self.compiled.eval_comb_2state();
                Ok(())
            }
            Some(_) => Err(format!("drive_input: port {port} is not an input")),
            None => {
                let mut names: Vec<&str> =
                    self.ports.keys().map(String::as_str).collect();
                names.sort_unstable();
                Err(format!(
                    "drive_input: no port {port} (ports: {})",
                    names.join(", ")
                ))
            }
        }
    }

    /// Drive a top-level input (or inout) port for SVA testbenches. Like
    /// [`Sim::drive_input`], and additionally records the value in the SVA
    /// stimulus map that checker `InputBit` probes sample.
    pub fn drive(&mut self, name: &str, value: bool) -> Result<(), String> {
        self.drive_input(name, value)?;
        self.stim.insert(name.to_string(), value);
        Ok(())
    }

    /// Attach SVA checkers elaborated by `helion_sv::sva` (see
    /// [`synth_sv_with_sva`](helion_sv::sva::synth_sv_with_sva)). Probe
    /// resolution failures name the property and the net.
    pub fn attach_sva(
        &mut self,
        design: &Design,
        props: Vec<SvaProperty>,
        imms: Vec<SvaImmProp>,
    ) -> Result<(), String> {
        self.sva.attach(design, props, imms)
    }

    /// Drain stored SVA failures (cap 128 stored; the total count in
    /// [`Sim::sva_report`] is unbounded). The IDE turns these into
    /// waveform markers, sim-log rows, and messages.
    pub fn sva_failures(&mut self) -> Vec<SvaFailure> {
        self.sva.drain_failures()
    }

    /// Per-assertion SVA status: attempts, vacuous passes, failures, and
    /// first-failure times.
    pub fn sva_report(&self) -> SvaReport {
        self.sva.report()
    }

    /// Clear stored SVA failures and the total failure count.
    pub fn clear_sva_failures(&mut self) {
        self.sva.clear_failures();
    }

    /// Top-level input (or inout) port names, sorted.
    pub fn input_ports(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ports
            .iter()
            .filter(|(_, d)| matches!(d, PortDir::In | PortDir::Inout))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// Top-level output port names, sorted.
    pub fn output_ports(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ports
            .iter()
            .filter(|(_, d)| matches!(d, PortDir::Out))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// Sample a signal by name: `led`, an FF Q, a driven input port, or any
    /// net / output port (output IOBs are followed PAD → I). Returns `None`
    /// for undriven/unknown names instead of panicking.
    pub fn sample(&self, name: &str) -> Option<u64> {
        if name.eq_ignore_ascii_case("led") {
            return Some(u64::from(self.led));
        }
        if let Some(q) = self.ff_q.get(name) {
            return Some(u64::from(*q));
        }
        if let Some(&v) = self.input_values.get(name) {
            return Some(u64::from(v));
        }
        let net = self
            .iob_pad_to_i
            .get(name)
            .map(String::as_str)
            .unwrap_or(name);
        let driven = self.net_ff.contains_key(net)
            || self.net_lut_o.contains_key(net)
            || self.input_values.contains_key(net);
        if !driven {
            if self.compiled.schedule.input_ports.contains_key(net) {
                return None;
            }
            if self
                .compiled
                .schedule
                .ops
                .iter()
                .any(|op| op.output_nets().iter().any(|&id| self.compiled.schedule.net_names.get(id as usize).map(String::as_str) == Some(net)))
            {
                return self.compiled.sample(net);
            }
            return None;
        }
        let mut memo = HashMap::new();
        let mut stack = Vec::new();
        Some(u64::from(self.pin_net_value(net, &mut memo, &mut stack)))
    }

    fn resolve_signal(&self, name: &str) -> Option<String> {
        if name.eq_ignore_ascii_case("led") {
            return Some("led".into());
        }
        self.ff_q
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned()
    }

    fn write_signal(&mut self, name: &str, value: u64) -> Result<(), String> {
        let bit = value != 0;
        if name == "led" {
            self.led = bit;
            return Ok(());
        }
        if let Some(q) = self.ff_q.get_mut(name) {
            *q = bit;
            return Ok(());
        }
        Err(format!("force: no signal {name}"))
    }

    fn apply_forces(&mut self) {
        let held: Vec<(String, u64)> = self.forces.iter().map(|(n, v)| (n.clone(), *v)).collect();
        for (name, value) in held {
            let _ = self.write_signal(&name, value);
        }
    }

    /// One-shot UG900 deposit: poke now, HDL drivers take the next posedge.
    pub fn deposit(&mut self, name: &str, value: u64) -> Result<(), String> {
        let key = self
            .resolve_signal(name)
            .ok_or_else(|| format!("deposit: no signal {name}"))?;
        self.forces.remove(&key);
        self.write_signal(&key, value)
    }

    /// UG900 force: hold `name` at `value` across posedges until `release`.
    pub fn force(&mut self, name: &str, value: u64) -> Result<(), String> {
        let key = self
            .resolve_signal(name)
            .ok_or_else(|| format!("force: no signal {name}"))?;
        self.write_signal(&key, value)?;
        self.forces.insert(key, value);
        Ok(())
    }

    /// Drop a UG900 force so the LUT/FF/IOB drivers resume.
    pub fn release(&mut self, name: &str) -> Result<(), String> {
        let key = self
            .resolve_signal(name)
            .ok_or_else(|| format!("remove_force: no signal {name}"))?;
        self.forces.remove(&key);
        Ok(())
    }

    /// Live UG900 force table (continuous holds only; deposits do not persist).
    pub fn force_list(&self) -> Vec<SimForce> {
        let mut v: Vec<SimForce> = self
            .forces
            .iter()
            .map(|(name, value)| SimForce {
                name: name.clone(),
                kind: SimForceKind::Force,
                value: *value,
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn step_posedge(&mut self, delay: u64) {
        let mut vpi = std::mem::take(&mut self.vpi);
        let _ = vpi.flush_inertial_puts(self);
        self.vpi = vpi;
        self.time += delay;
        self.cycle += 1;
        // SVA preponed region: concurrent properties + non-final immediate
        // asserts sample the pre-edge state.
        self.sva.sample_pre(self.time, self.cycle, &self.ff_q, &self.stim);

        // Sync any deposit/force/input updates into the compiled fast engine.
        for (port, &val) in &self.input_values {
            let _ = self.compiled.drive_input(port, val);
        }
        for (ff, &val) in &self.ff_q {
            let _ = self.compiled.set_ff_4state(ff, Logic4::from_bool(val));
        }
        if let Some(tm) = &mut self.timing {
            tm.step_timed(&mut self.compiled, delay);
        } else {
            self.compiled.step_2state(delay);
        }

        let mut next_q = HashMap::new();
        // One memo per step: net values are stable across this edge.
        let mut memo: HashMap<String, bool> = HashMap::new();
        let mut stack: Vec<String> = Vec::new();
        let luts: Vec<(String, String)> = self
            .ff_d_lut
            .iter()
            .map(|(f, l)| (f.clone(), l.clone()))
            .collect();
        for (ff, lut) in &luts {
            let o = self.lut_output_live(lut, &mut memo, &mut stack);
            next_q.insert(ff.clone(), o);
        }
        // Also propagate FFs driven by Carry4 / Muxf7 / Muxf8 from the compiled schedule.
        for ff_desc in &self.compiled.schedule.ffs {
            if !self.ff_d_lut.contains_key(&ff_desc.name) {
                if let Some(v) = self.compiled.get_ff_4state(&ff_desc.name) {
                    next_q.insert(ff_desc.name.clone(), v == Logic4::One);
                }
            }
        }
        for (k, v) in next_q {
            self.ff_q.insert(k, v);
        }
        self.led = self
            .iob_from_ff
            .as_ref()
            .and_then(|f| self.ff_q.get(f))
            .copied()
            .unwrap_or(self.compiled.led);
        self.apply_forces();
        for (ff, &val) in &self.ff_q {
            let _ = self.compiled.set_ff_4state(ff, Logic4::from_bool(val));
        }
        self.coverage.sample_compiled(&self.compiled);
        // SVA postponed region: `assert final` sees the committed state.
        self.sva
            .sample_post(self.time, self.cycle, &self.ff_q, &self.stim);
    }

    /// Access the levelized flat compiled 2-state/4-state engine.
    pub fn compiled(&self) -> &CompiledSim {
        &self.compiled
    }

    /// Mutably access the levelized flat compiled 2-state/4-state engine.
    pub fn compiled_mut(&mut self) -> &mut CompiledSim {
        &mut self.compiled
    }

    /// Run `cycles` clock posedges using the levelized flat 2-state compiled fast path
    /// (`Vec<u64>` state words, zero per-cycle allocation) and synchronize `Sim` state.
    pub fn run_fast_2state(&mut self, cycles: u64, period_ps: u64) -> Vec<bool> {
        for (port, &val) in &self.input_values {
            let _ = self.compiled.drive_input(port, val);
        }
        let wave = self.compiled.run_2state_cycles(cycles, period_ps);
        self.time = self.compiled.time;
        self.cycle = self.compiled.cycle;
        self.led = self.compiled.led;
        for ff in &self.compiled.schedule.ffs {
            if let Some(v) = self.compiled.get_ff_4state(&ff.name) {
                self.ff_q.insert(ff.name.clone(), v == Logic4::One);
            }
        }
        self.apply_forces();
        wave
    }

    /// Execute one clock posedge in 4-state (`0, 1, X, Z`) mode.
    pub fn step_4state(&mut self, delay: u64) {
        self.time += delay;
        self.cycle += 1;
        if let Some(tm) = &mut self.timing {
            tm.step_timed(&mut self.compiled, delay);
        } else {
            self.compiled.step_4state(delay);
        }
        for ff in &self.compiled.schedule.ffs {
            if let Some(v) = self.compiled.get_ff_4state(&ff.name) {
                self.ff_q.insert(ff.name.clone(), v == Logic4::One);
            }
        }
        self.led = self.compiled.led;
        self.coverage.sample_compiled(&self.compiled);
    }

    /// Drive a primary input port with a 4-state [`Logic4`] value (`0, 1, X, Z`).
    pub fn drive_input_4state(&mut self, port: &str, value: Logic4) -> Result<(), String> {
        match self.ports.get(port) {
            Some(PortDir::In) | Some(PortDir::Inout) => {
                if let Some(b) = value.as_bool() {
                    self.input_values.insert(port.to_string(), b);
                } else {
                    self.input_values.remove(port);
                }
                self.compiled.drive_input_4state(port, value)?;
                self.compiled.eval_comb_4state();
                Ok(())
            }
            Some(_) => Err(format!("drive_input_4state: port {port} is not an input")),
            None => Err(format!("drive_input_4state: no port {port}")),
        }
    }

    /// Sample a net, port, or FF in 4-state (`0, 1, X, Z`) mode.
    pub fn sample_4state(&self, name: &str) -> Option<Logic4> {
        self.compiled.sample_4state(name)
    }

    /// Attach an SDF back-annotated gate-level timing model to this simulator.
    pub fn attach_sdf(&mut self, sdf: &SdfFile, corner: SdfCorner) {
        self.timing = Some(SimTimingModel::from_sdf(sdf, corner));
    }

    /// Parse an SDF text and attach the resulting [`SimTimingModel`] at `corner`.
    pub fn attach_sdf_str(&mut self, sdf_src: &str, corner: SdfCorner) -> Result<(), String> {
        let model = SimTimingModel::parse(sdf_src, corner)?;
        self.timing = Some(model);
        Ok(())
    }

    /// Return all SDF timing violations recorded so far.
    pub fn timing_violations(&self) -> &[TimingViolation] {
        self.timing
            .as_ref()
            .map(|t| t.violations.as_slice())
            .unwrap_or(&[])
    }

    /// Clear recorded SDF timing violations.
    pub fn clear_timing_violations(&mut self) {
        if let Some(t) = &mut self.timing {
            t.violations.clear();
        }
    }

    /// Attach SVA concurrent `cover property` items to this simulator.
    pub fn attach_sva_covers(
        &mut self,
        design: &Design,
        covers: Vec<SvaProperty>,
    ) -> Result<(), String> {
        self.sva.attach_covers(design, covers)
    }

    /// Register an explicit branch condition net for branch coverage tracking.
    pub fn register_branch_coverage(
        &mut self,
        name: impl Into<String>,
        cond_net: impl Into<String>,
    ) {
        self.coverage.register_branch(name, cond_net);
    }

    /// Register an explicit multi-bit FSM state bus for FSM state/transition coverage.
    pub fn register_fsm_coverage(
        &mut self,
        name: impl Into<String>,
        bit_nets: Vec<String>,
        expected_states: Option<usize>,
    ) {
        self.coverage.register_fsm(name, bit_nets, expected_states);
    }

    /// Build a unified [`FunctionalCoverageReport`] (toggle + branch + FSM + SVA assertions/covers).
    pub fn coverage_report(&self) -> FunctionalCoverageReport {
        self.coverage.report(self.sva.report())
    }

    /// Access the DPI-C foreign function registry (`DpiRegistry`).
    pub fn dpi(&self) -> &DpiRegistry {
        &self.dpi
    }

    /// Mutably access the DPI-C foreign function registry (`DpiRegistry`).
    pub fn dpi_mut(&mut self) -> &mut DpiRegistry {
        &mut self.dpi
    }

    /// Parse and register a SystemVerilog `import "DPI-C" function ...` callback on this [`Sim`].
    pub fn register_dpi_import<F>(&mut self, import_decl: &str, callback: F) -> Result<(), String>
    where
        F: Fn(&[DpiValue]) -> Result<DpiValue, String> + Send + Sync + 'static,
    {
        self.dpi.register_import(import_decl, callback)
    }

    /// Invoke a registered DPI-C function by SV name or C linkage name.
    pub fn call_dpi(&mut self, name: &str, args: &[DpiValue]) -> Result<DpiValue, String> {
        self.dpi.call(name, args)
    }
    /// Look up the HFF cell name driving `net`, if `net` is driven by a flip-flop `Q` pin.
    pub fn net_ff_cell(&self, net: &str) -> Option<String> {
        self.net_ff.get(net).cloned()
    }

    /// Access the IEEE 1800 VPI signal introspection registry.
    pub fn vpi(&self) -> &VpiRegistry {
        &self.vpi
    }

    /// Look up a [`VpiHandle`] by hierarchical or local signal name.
    pub fn vpi_handle_by_name(&self, name: &str) -> Option<VpiHandle> {
        self.vpi.vpi_handle_by_name(name)
    }

    /// Read the current value of a VPI handle in the requested [`VpiFormat`].
    pub fn vpi_get_value(&self, handle: VpiHandle, format: VpiFormat) -> Result<VpiValue, String> {
        self.vpi.vpi_get_value(self, handle, format)
    }

    /// Write, schedule, force, or release a VPI signal handle with [`VpiPutFlags`].
    pub fn vpi_put_value(
        &mut self,
        handle: VpiHandle,
        value: &VpiValue,
        flags: VpiPutFlags,
    ) -> Result<(), String> {
        let mut vpi = std::mem::take(&mut self.vpi);
        let res = vpi.vpi_put_value(self, handle, value, flags);
        self.vpi = vpi;
        res
    }

    /// Return VPI handles for all top-level ports (`vpi_iterate(vpiPort, top)`).
    pub fn vpi_iterate_ports(&self) -> Vec<VpiHandle> {
        self.vpi.vpi_iterate_ports()
    }

    /// Return VPI handles for all nets, registers, and vector buses (`vpi_iterate(vpiNet, top)`).
    pub fn vpi_iterate_nets(&self) -> Vec<VpiHandle> {
        self.vpi.vpi_iterate_nets()
    }

    /// UG900 Objects pane: live helion-sim probes (LED + each FF Q), not a static name list.
    pub fn object_values(&self) -> Vec<(String, String)> {
        self.locals()
            .into_iter()
            .map(|l| (l.name, l.value))
            .collect()
    }

    /// UG900 Memory: LUT INIT as 64×1 ROM. Cap the dump — Ibex is 100k LUTs.
    pub fn memory_cells(&self) -> Vec<SimMemory> {
        const CAP: usize = 32;
        let mut v: Vec<SimMemory> = Vec::new();
        for (name, init) in &self.lut_init {
            if v.len() >= CAP {
                break;
            }
            v.push(SimMemory {
                name: name.clone(),
                kind: "lut_init".into(),
                width: 1,
                words: (0..64).map(|a| (init >> a) & 1).collect(),
            });
        }
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// UG900 Locals: current-scope sequential state from the event kernel.
    pub fn locals(&self) -> Vec<SimLocal> {
        let bit = |b: bool| if b { "1".to_string() } else { "0".to_string() };
        let mut v = vec![SimLocal {
            name: "led".into(),
            kind: "port".into(),
            value: bit(self.led),
        }];
        let mut ffs: Vec<_> = self.ff_q.iter().collect();
        ffs.sort_by_key(|(n, _)| *n);
        for (n, q) in ffs.into_iter().take(256) {
            v.push(SimLocal {
                name: n.clone(),
                kind: "reg".into(),
                value: bit(*q),
            });
        }
        v
    }

    /// Evaluate one LUT's combinational output from current net values (no
    /// clock advance). Used by the VCD sampler; delegates to the kernel's
    /// memoized multi-level resolver so LUT chains evaluate exactly.
    fn lut_output(&self, lut: &str) -> Option<bool> {
        if !self.lut_init.contains_key(lut) {
            return None;
        }
        let mut memo = HashMap::new();
        let mut stack = Vec::new();
        Some(self.lut_output_live(lut, &mut memo, &mut stack))
    }

    /// Current logic value of every net with a resolvable driver.
    ///
    /// A net resolves through its first driver endpoint: FF `Q` → current
    /// register state, LUT `O` → combinational evaluation from current FF
    /// state. Nets with no FF/LUT driver (undriven, IOB-only) are skipped.
    /// Used by the VCD dumper ([`vcd::dump_vcd`]); sorted by net name so
    /// dumps are deterministic.
    pub fn sample_nets(&self, d: &Design) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for n in &d.nets {
            let mut val: Option<bool> = None;
            for e in &n.endpoints {
                if e.pin == intern("Q") {
                    if let Some(q) = self.ff_q.get(resolve(e.cell)) {
                        val = Some(*q);
                        break;
                    }
                } else if e.pin == intern("O") {
                    if let Some(o) = self.lut_output(resolve(e.cell)) {
                        val = Some(o);
                        break;
                    }
                }
            }
            if let Some(v) = val {
                out.push((resolve(n.name).to_string(), v));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// ILA probe: live value of any net, including combinational (LUT-driven)
    /// nets. Returns `None` for undriven/unknown nets and combinational loops —
    /// the ILA renders those as X. `"led"` resolves to the LED state; `"clk"`
    /// is the sampling event itself, so it reads 1 at every posedge sample.
    pub fn net_value(&self, net: &str) -> Option<bool> {
        if net.eq_ignore_ascii_case("led") {
            return Some(self.led);
        }
        if net.eq_ignore_ascii_case("clk") {
            return Some(true);
        }
        let mut memo: HashMap<&str, bool> = HashMap::new();
        let mut stack: Vec<&str> = Vec::new();
        self.eval_net(net, &mut memo, &mut stack)
    }

    fn eval_net<'s>(
        &'s self,
        net: &'s str,
        memo: &mut HashMap<&'s str, bool>,
        stack: &mut Vec<&'s str>,
    ) -> Option<bool> {
        if let Some(&v) = memo.get(net) {
            return Some(v);
        }
        if stack.contains(&net) {
            return None; // combinational loop — probe reads X
        }
        let v = match self.net_drivers.get(net)? {
            NetDriver::Undriven => None,
            NetDriver::Ff(cell) => self.ff_q.get(cell).copied(),
            NetDriver::Lut(cell) => {
                let init = self.lut_init.get(cell).copied().unwrap_or(0);
                let inputs = self.lut_input_nets.get(cell);
                stack.push(net);
                let mut addr = 0u64;
                let mut unknown = false;
                for pin in 0..6usize {
                    let net_on_pin = inputs
                        .and_then(|ins| ins.get(pin))
                        .and_then(|o| o.as_deref());
                    // Unconnected LUT inputs tie low — same as `step_posedge`,
                    // which only drives addr bits from connected FF Qs. Only a
                    // connected-but-unresolvable net (undriven, loop) yields X.
                    let bit = match net_on_pin {
                        None => Some(false),
                        Some(n) => self.eval_net(n, memo, stack),
                    };
                    match bit {
                        Some(true) => addr |= 1 << pin,
                        Some(false) => {}
                        None => unknown = true,
                    }
                }
                stack.pop();
                if unknown {
                    None
                } else {
                    Some((init >> addr) & 1 == 1)
                }
            }
        };
        if let Some(b) = v {
            memo.insert(net, b);
        }
        v

    }

    /// Probe a named signal for UG900 breakpoints (LED, FF Q, or LUT INIT word).
    pub fn signal_value(&self, name: &str) -> Option<u64> {
        if name.eq_ignore_ascii_case("led") {
            return Some(u64::from(self.led));
        }
        if let Some(q) = self.ff_q.get(name) {
            return Some(u64::from(*q));
        }
        self.lut_init.get(name).copied()
    }
}

/// One UG900 Memory array: LUT INIT as a 64×1 ROM, or a packed sequential word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimMemory {
    pub name: String,
    pub kind: String,
    pub width: u8,
    pub words: Vec<u64>,
}

impl SimMemory {
    pub fn packed_word(&self) -> u64 {
        if self.width <= 1 {
            self.words
                .iter()
                .enumerate()
                .fold(0u64, |acc, (i, b)| acc | ((*b & 1) << i))
        } else {
            self.words.first().copied().unwrap_or(0)
        }
    }
}

/// One UG900 Locals row: live sequential probe from helion-sim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimLocal {
    pub name: String,
    pub kind: String,
    pub value: String,
}

/// Gold empty-XDC structural-counter overlay LED (`cnt[3]` over 16 posedges).
/// Matches lab/hw `COUNTER_OVERLAY_LED` waveform shape — event-sim path, not board.
pub const COUNTER_OVERLAY_SIM_LED: &str = "0000000111111110";

/// Overlay / behavioral-sim LED waveform completion.
///
/// Types and summary strings say **`overlay`** and **`sim_DONE`** — never bare board DONE.
/// This is the helion-sim surface for W-L4 (CI without USB).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlaySimReport {
    /// LED samples over `cycles` posedges (`'0'`/`'1'`).
    pub led: String,
    pub cycles: u32,
    /// Always `true` when the overlay run finished — sim completion, not TAP STAT DONE.
    pub sim_done: bool,
}

impl OverlaySimReport {
    /// Machine-readable overlay line. Contains `overlay` + `sim_DONE=` — never ` DONE=`.
    pub fn summary_line(&self) -> String {
        format!(
            "overlay LED={} cycles={} sim_DONE={} (overlay; not board DONE)",
            self.led,
            self.cycles,
            u8::from(self.sim_done),
        )
    }
}

/// Run the event-driven kernel and return an **overlay**-labeled LED waveform.
///
/// Completes with `sim_DONE=1` in [`OverlaySimReport::summary_line`] — cannot be
/// mistaken for board program DONE (no USB / no TAP STAT).
pub fn run_overlay_led(design: &Design, cycles: u32) -> OverlaySimReport {
    let wave = run_tb(design, cycles);
    let led: String = wave.iter().map(|b| if *b { '1' } else { '0' }).collect();
    OverlaySimReport {
        led,
        cycles,
        sim_done: true,
    }
}

pub fn run_tb(design: &Design, cycles: u32) -> Vec<bool> {
    let mut s = Sim::new(design);
    let mut wave = Vec::new();
    for _ in 0..cycles {
        s.step_posedge(10);
        wave.push(s.led);
    }
    wave
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{intern, CellKind, Design, PortDir};

    #[test]
    fn overlay_sim_counter_led_gold_labeled_sim_done() {
        let r = run_overlay_led(&Design::structural_counter(), 16);
        assert_eq!(r.led, COUNTER_OVERLAY_SIM_LED, "overlay sim LED gold {r:?}");
        assert!(r.sim_done, "overlay completion sets sim_done");
        assert_eq!(r.cycles, 16);
        assert!(
            r.led.contains('0') && r.led.contains('1'),
            "blink waveform must include 0 and 1: {}",
            r.led
        );

        let s = r.summary_line();
        assert!(s.contains("overlay"), "{s}");
        assert!(s.contains("sim_DONE=1"), "{s}");
        assert!(s.contains("not board DONE"), "{s}");
        // Board-style bare DONE= must not appear; sim_DONE= is OK.
        assert!(!s.contains(" DONE="), "{s}");
        assert!(!s.to_ascii_lowercase().contains("board done=1"), "{s}");
        assert!(!s.contains("DONE=1") || s.contains("sim_DONE=1"), "{s}");
        // Explicit: the only DONE token is sim_DONE=
        assert!(
            !s.replace("sim_DONE=", "SIM_DONE_TOKEN=").contains("DONE="),
            "must not emit bare DONE=: {s}"
        );
    }

    #[test]
    fn inverter_toggles_const0_does_not() {
        let inv = run_tb(&Design::structural_blinky(), 4);
        assert!(inv.contains(&true) && inv.contains(&false));

        let mut z = Design::new("z");
        z.add_port("clk", PortDir::In);
        z.add_port("led", PortDir::Out);
        z.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        z.add_cell("u_ff", CellKind::Hff);
        z.add_cell("u_iob", CellKind::IobOut);
        z.connect("d", "u_lut", "O");
        z.connect("d", "u_ff", "D");
        z.connect("q", "u_ff", "Q");
        z.connect("q", "u_lut", "I0");
        z.connect("q", "u_iob", "I");
        let w = run_tb(&z, 4);
        assert!(
            w.iter().all(|&b| !b),
            "const0 LUT must not toggle LED: {w:?}"
        );
    }

    #[test]
    fn synth_counter_sim_matches_gold_led_pattern() {
        // FM-HEL-AREA-02: LUT peephole opts (dead sweep / const fold /
        // inverter absorption) must not change counter.sv simulation. The
        // synthesized netlist (not just the hand-built structural one) must
        // produce the gold LED pattern.
        let src = r#"
module counter (
    input  logic clk,
    output logic led
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign led = cnt[3];
endmodule
"#;
        let d = helion_sv::synth_sv(src, "counter.sv").expect("synth counter");
        let wave = run_tb(&d, 16);
        let led: String = wave.iter().map(|b| if *b { '1' } else { '0' }).collect();
        assert_eq!(
            led, COUNTER_OVERLAY_SIM_LED,
            "synthesized counter.sv must simulate to the gold LED pattern"
        );
    }

    #[test]
    fn event_sim_counter_matches_fabric_gold() {
        let w = run_tb(&Design::structural_counter(), 16);
        assert!(w[0..7].iter().all(|b| !b), "{w:?}");
        assert!(w[7..15].iter().all(|b| *b), "{w:?}");
        assert!(!w[15], "{w:?}");
    }

    #[test]
    fn memory_locals_are_lut_init_and_ff_q() {
        let mut s = Sim::new(&Design::structural_counter());
        let mem = s.memory_cells();
        let lut0 = mem.iter().find(|m| m.name == "u_lut0").expect("u_lut0");
        assert_eq!(lut0.kind, "lut_init");
        assert_eq!(lut0.words.len(), 64);
        assert_eq!(lut0.packed_word(), helion_ir::INC4_INIT[0]);
        assert_eq!(lut0.words[0], 1, "0x5555… bit0 is 1");
        let lut1 = mem.iter().find(|m| m.name == "u_lut1").expect("u_lut1");
        assert_ne!(lut0.packed_word(), lut1.packed_word());

        let blinky = Sim::new(&Design::structural_blinky());
        let bmem = blinky.memory_cells();
        let blut = bmem.iter().find(|m| m.name == "u_lut").expect("u_lut");
        assert_eq!(blut.name, "u_lut");
        assert!(bmem.iter().all(|m| m.name != "u_lut0"));
        let lut3 = mem.iter().find(|m| m.name == "u_lut3").expect("u_lut3");
        assert_ne!(lut0.packed_word(), lut3.packed_word());

        s.step_posedge(10);
        let loc = s.locals();
        assert!(
            loc.iter()
                .any(|l| l.name == "led" && l.kind == "port" && (l.value == "0" || l.value == "1")),
            "{loc:?}"
        );
        assert!(
            loc.iter()
                .any(|l| l.kind == "reg" && (l.value == "0" || l.value == "1")),
            "{loc:?}"
        );
        assert_eq!(s.signal_value("led"), Some(u64::from(s.led)));
    }

    #[test]
    fn memory_cells_capped_on_wide_netlist() {
        let mut d = Design::new("wide");
        for i in 0..400u32 {
            d.add_cell(&format!("lut{i}"), CellKind::Lut6 { init: 1 });
        }
        let s = Sim::new(&d);
        assert_eq!(s.lut_init.len(), 400, "kernel still holds every INIT");
        let mem = s.memory_cells();
        assert!(
            mem.len() <= 32,
            "Memory pane must not dump 400 LUT ROMs: {}",
            mem.len()
        );
        assert!(!mem.is_empty());
    }

    #[test]
    fn ila_net_value_probes_ff_and_combinational_nets() {
        // Structural counter: q0..q3 are FF Q nets, d0..d3 are LUT O nets.
        let mut s = Sim::new(&Design::structural_counter());
        assert_eq!(s.net_value("q0"), Some(false));
        assert_eq!(s.net_value("led"), Some(false));
        assert_eq!(s.net_value("clk"), Some(true));
        assert_eq!(s.net_value("no_such_net"), None);
        // d0 = ~q0 while q0 = 0 → d0 must read 1 combinationally.
        assert_eq!(s.net_value("d0"), Some(true));
        s.step_posedge(10);
        assert_eq!(s.net_value("q0"), Some(true));
        assert_eq!(s.net_value("d0"), Some(false));
        // A full count cycle: q reads as a binary counter on the Q nets.
        let mut s = Sim::new(&Design::structural_counter());
        for expect in 0..16u64 {
            let mut q = 0u64;
            for (i, n) in ["q0", "q1", "q2", "q3"].iter().enumerate() {
                if s.net_value(n) == Some(true) {
                    q |= 1 << i;
                }
            }
            assert_eq!(q, expect, "Q-net bus must count 0..15");
            s.step_posedge(10);
        }
    }

    #[test]
    fn ila_net_value_handles_chained_luts_and_loops() {
        // LUT→LUT chain: e = d0 AND q1 (2-level combo), then probe e.
        let mut d = Design::structural_counter();
        d.add_cell("u_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d.connect("d0", "u_and", "I0");
        d.connect("q1", "u_and", "I1");
        d.connect("e", "u_and", "O");
        let mut s = Sim::new(&d);
        // q0=0,q1=0 → d0=1, e = 1&0 = 0.
        assert_eq!(s.net_value("e"), Some(false));
        s.step_posedge(10);
        s.step_posedge(10);
        // q=2: q0=0,q1=1 → d0=1, e = 1&1 = 1.
        assert_eq!(s.net_value("q0"), Some(false));
        assert_eq!(s.net_value("q1"), Some(true));
        assert_eq!(s.net_value("e"), Some(true));
        // Self-loop LUT (O feeds back to I0): probe must read X, not hang.
        let mut l = Design::new("loop");
        l.add_cell("u_l", CellKind::Lut6 { init: !0 });
        l.add_cell("u_ff", CellKind::Hff);
        l.connect("w", "u_l", "O");
        l.connect("w", "u_l", "I0");
        l.connect("w", "u_ff", "D");
        let s2 = Sim::new(&l);
        assert_eq!(s2.net_value("w"), None);
    }

    #[test]
    fn force_holds_led_deposit_is_oneshot() {
        let mut s = Sim::new(&Design::structural_counter());
        s.force("led", 1).unwrap();
        assert_eq!(s.force_list().len(), 1);
        for _ in 0..16 {
            s.step_posedge(10);
            assert!(s.led, "force must hold LED across posedges");
        }
        s.release("led").unwrap();
        assert!(s.force_list().is_empty());
        s.deposit("led", 1).unwrap();
        assert!(s.led, "deposit pokes immediately");
        assert!(s.force_list().is_empty(), "deposit is not a hold");
        s.step_posedge(10);
        assert!(!s.led, "next posedge is HDL cnt[3], still 0");
        assert!(s.force("no_such", 1).unwrap_err().contains("no signal"));
        assert!(s.deposit("no_such", 1).unwrap_err().contains("no signal"));
    }

    fn fabric_wave(d: &Design, cycles: u32) -> Vec<bool> {
        use helion_bits::bitgen;
        use helion_device::Device;
        use helion_fabric::Fabric;
        use helion_pack::pack;
        use helion_place::place;
        use helion_route::route;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let bits = bitgen(&dev, &r).unwrap();
        let mut fab = Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut w = Vec::new();
        for _ in 0..cycles {
            fab.step_user();
            w.push(fab.led_at(iob.0, iob.1));
        }
        w
    }

    fn six_bit_counter() -> Design {
        let mut d = Design::new("inc6");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        // bit i INIT: q[i] ^ AND(q[0..i-1]); I0=LSB
        fn inc_init(i: u32) -> u64 {
            let mut w = 0u64;
            for addr in 0..64u64 {
                let qi = (addr >> i) & 1;
                let mut cin = 1u64;
                for j in 0..i {
                    cin &= (addr >> j) & 1;
                }
                if i == 0 {
                    w |= ((1 - qi) & 1) << addr;
                } else {
                    w |= (qi ^ cin) << addr;
                }
            }
            w
        }
        for i in 0..6u32 {
            d.add_cell(&format!("u_lut{i}"), CellKind::Lut6 { init: inc_init(i) });
            d.add_cell(&format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", &format!("u_ff{i}"), "CLK");
            d.connect(&format!("d{i}"), &format!("u_lut{i}"), "O");
            d.connect(&format!("d{i}"), &format!("u_ff{i}"), "D");
            d.connect(&format!("q{i}"), &format!("u_ff{i}"), "Q");
            for pin in 0..=i {
                d.connect(&format!("q{pin}"), &format!("u_lut{i}"), &format!("I{pin}"));
            }
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q5", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    #[test]
    fn six_pin_lut_is_occupied_and_matches_fabric() {
        let d = six_bit_counter();
        let lut5 = d.cell("u_lut5").unwrap();
        let pins: Vec<_> = (0..6u8)
            .filter(|p| d.net_on(intern("u_lut5"), intern(&format!("I{p}"))).is_some())
            .collect();
        assert_eq!(
            pins,
            vec![0, 1, 2, 3, 4, 5],
            "MSB incrementer must use all 6 LUT pins"
        );
        match lut5.kind {
            CellKind::Lut6 { init } => {
                assert_ne!(init, 0x5555_5555_5555_5555, "not a 1-pin inverter")
            }
            _ => panic!("lut"),
        }
        let ev = run_tb(&d, 64);
        let fab = fabric_wave(&d, 64);
        assert_eq!(ev, fab, "event sim 6-pin must agree with fabric");
        assert!(ev[0..31].iter().all(|b| !b), "cnt 1..31 LED=0 {ev:?}");
        assert!(ev[31..63].iter().all(|b| *b), "cnt 32..63 LED=1");
        assert!(!ev[63], "wrap");
    }

    #[test]
    fn event_sim_agrees_with_fabric_on_counter_and_hier() {
        let c = Design::structural_counter();
        let ev = run_tb(&c, 16);
        let fab = fabric_wave(&c, 16);
        assert_eq!(ev, fab, "counter event vs fabric {ev:?} {fab:?}");

        let hier =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/hier.sv");
        let src = std::fs::read_to_string(&hier).unwrap();
        let d = helion_sv::synth_sv(&src, "hier.sv").unwrap();
        let evh = run_tb(&d, 8);
        let fabh = fabric_wave(&d, 8);
        assert_eq!(evh, fabh, "hier event vs fabric {evh:?} {fabh:?}");
        assert!(
            evh.contains(&true) && evh.contains(&false),
            "hier must toggle {evh:?}"
        );
    }

    /// DUT with driven input ports: `y = reg(a & b)` through one LUT.
    fn and_dut() -> Design {
        let mut d = Design::new("and_dut");
        d.add_port("clk", PortDir::In);
        d.add_port("a", PortDir::In);
        d.add_port("b", PortDir::In);
        d.add_port("y", PortDir::Out);
        d.add_cell("u_lut", CellKind::Lut6 { init: 0x8 }); // I1 & I0
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("a", "u_lut", "I0");
        d.connect("b", "u_lut", "I1");
        d.connect("d", "u_lut", "O");
        d.connect("d", "u_ff", "D");
        d.connect("clk", "u_ff", "CLK");
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_iob", "I");
        d.connect("y", "u_iob", "PAD");
        d
    }

    #[test]
    fn drive_input_flows_through_lut_to_ff_and_sample_reads_output_port() {
        let mut s = Sim::new(&and_dut());
        assert_eq!(s.input_ports(), vec!["a", "b", "clk"]);
        assert_eq!(s.output_ports(), vec!["y"]);
        // Undriven inputs read back as driven-0 only after drive; sampling an
        // output before any clock is fine (Q=0).
        assert_eq!(s.sample("y"), Some(0));
        s.drive_input("a", true).unwrap();
        s.drive_input("b", true).unwrap();
        assert_eq!(s.sample("a"), Some(1));
        s.step_posedge(10);
        assert_eq!(s.sample("y"), Some(1), "1&1 must clock into the FF");
        s.drive_input("b", false).unwrap();
        s.step_posedge(10);
        assert_eq!(s.sample("y"), Some(0), "1&0 must clock in as 0");
        // Driving is sticky until changed.
        s.step_posedge(10);
        assert_eq!(s.sample("y"), Some(0));
        s.drive_input("b", true).unwrap();
        s.step_posedge(10);
        assert_eq!(s.sample("y"), Some(1));
    }

    #[test]
    fn drive_input_rejects_unknown_and_output_ports() {
        let mut s = Sim::new(&and_dut());
        let e = s.drive_input("nope", true).unwrap_err();
        assert!(e.contains("no port nope"), "{e}");
        assert!(e.contains('a'), "error lists available ports: {e}");
        let e2 = s.drive_input("y", true).unwrap_err();
        assert!(e2.contains("not an input"), "{e2}");
        assert_eq!(s.sample("nope"), None, "unknown signal samples as None");
    }

    /// Two-level LUT chain: `q <= (a & b) & c`. The old pin→FF-only step
    /// read the chained pin as 0; live resolution must evaluate the chain.
    #[test]
    fn step_posedge_evaluates_multi_level_lut_chains() {
        let mut d = Design::new("chain_dut");
        d.add_port("clk", PortDir::In);
        d.add_port("a", PortDir::In);
        d.add_port("b", PortDir::In);
        d.add_port("c", PortDir::In);
        d.add_port("y", PortDir::Out);
        d.add_cell("u_l1", CellKind::Lut6 { init: 0x8 }); // a & b
        d.add_cell("u_l2", CellKind::Lut6 { init: 0x8 }); // n1 & c
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("a", "u_l1", "I0");
        d.connect("b", "u_l1", "I1");
        d.connect("n1", "u_l1", "O");
        d.connect("n1", "u_l2", "I0");
        d.connect("c", "u_l2", "I1");
        d.connect("d", "u_l2", "O");
        d.connect("d", "u_ff", "D");
        d.connect("clk", "u_ff", "CLK");
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_iob", "I");
        d.connect("y", "u_iob", "PAD");
        let mut s = Sim::new(&d);
        for (a, b, c) in [(true, true, true), (true, true, false), (false, true, true)] {
            s.drive_input("a", a).unwrap();
            s.drive_input("b", b).unwrap();
            s.drive_input("c", c).unwrap();
            s.step_posedge(10);
            let want = u64::from(a && b && c);
            assert_eq!(s.sample("y"), Some(want), "a={a} b={b} c={c}");
        }
    }

    #[test]
    fn sample_reads_ff_q_and_driven_inputs_directly() {
        let mut s = Sim::new(&and_dut());
        s.drive_input("a", true).unwrap();
        assert_eq!(s.sample("a"), Some(1));
        assert_eq!(s.sample("b"), None, "undriven input samples as None (X)");
        s.step_posedge(10);
        assert_eq!(s.sample("u_ff"), Some(0));
        s.drive_input("b", true).unwrap();
        s.step_posedge(10);
        assert_eq!(s.sample("u_ff"), Some(1));
    }

    // ========================================================================
    // Levelized Flat Compiled 2-State & 4-State Fast Path Tests
    // ========================================================================

    #[test]
    fn levelized_fast_2state_matches_event_sim_and_runs_multi_thousand_cycles() {
        let d = six_bit_counter();
        let mut ev_sim = Sim::new(&d);
        let mut fast_sim = CompiledSim::new(&d);
        assert_eq!(fast_sim.schedule.num_levels(), 1);
        assert_eq!(fast_sim.schedule.ffs.len(), 6);
        assert!(!fast_sim.schedule.has_combinational_loop);

        for _ in 0..128 {
            ev_sim.step_posedge(10);
            fast_sim.step_2state(10);
            assert_eq!(fast_sim.led, ev_sim.led);
            for i in 0..6 {
                let qname = format!("q{i}");
                assert_eq!(fast_sim.sample(&qname), ev_sim.sample(&qname));
            }
        }

        // High-speed compiled batch run (10,000 cycles, zero per-cycle allocation).
        let mut batch_sim = Sim::new(&d);
        let wave = batch_sim.run_fast_2state(10_000, 10);
        assert_eq!(wave.len(), 10_000);
        assert_eq!(batch_sim.cycle, 10_000);
        assert_eq!(batch_sim.time, 100_000);
        // 10_000 % 64 = 16 -> bit 5 (MSB of 6-bit counter) is 0 at count 16.
        assert!(!batch_sim.led);
    }

    #[test]
    fn levelized_fast_evaluates_carry4_chains_and_muxf7_muxf8_trees() {
        // Build an 8-bit Carry4 counter + MUXF7/MUXF8 selector tree.
        let mut d = Design::new("carry_mux_dut");
        d.add_port("clk", PortDir::In);
        d.add_port("sel0", PortDir::In);
        d.add_port("sel1", PortDir::In);
        d.add_port("y", PortDir::Out);

        for c in 0..2u32 {
            let cname = format!("u_carry{c}");
            d.add_cell(&cname, CellKind::Carry4);
            if c == 0 {
                d.connect("1", &cname, "CYINIT");
            } else {
                d.connect("co_chain", &cname, "CI");
            }
            for i in 0..4u32 {
                let bit = c * 4 + i;
                let ff_name = format!("u_ff{bit}");
                let q_net = format!("q_{bit}");
                let o_net = format!("sum_{bit}");
                d.add_cell(&ff_name, CellKind::Hff);
                d.connect("clk", &ff_name, "CLK");
                d.connect(&o_net, &cname, &format!("O{i}"));
                d.connect(&o_net, &ff_name, "D");
                d.connect(&q_net, &ff_name, "Q");
                d.connect(&q_net, &cname, &format!("S{i}"));
                d.connect("0", &cname, &format!("DI{i}"));
            }
            if c == 0 {
                d.connect("co_chain", &cname, "CO3");
            }
        }

        // Two MUXF7 cells feeding one MUXF8 cell to mux {q_0, q_1, q_2, q_3}.
        d.add_cell(
            "u_mux7_lo",
            CellKind::BlackBox {
                module: intern("MUXF7"),
            },
        );
        d.connect("q_0", "u_mux7_lo", "I0");
        d.connect("q_1", "u_mux7_lo", "I1");
        d.connect("sel0", "u_mux7_lo", "S");
        d.connect("m7_lo", "u_mux7_lo", "O");

        d.add_cell(
            "u_mux7_hi",
            CellKind::BlackBox {
                module: intern("MUXF7"),
            },
        );
        d.connect("q_2", "u_mux7_hi", "I0");
        d.connect("q_3", "u_mux7_hi", "I1");
        d.connect("sel0", "u_mux7_hi", "S");
        d.connect("m7_hi", "u_mux7_hi", "O");

        d.add_cell(
            "u_mux8",
            CellKind::BlackBox {
                module: intern("MUXF8"),
            },
        );
        d.connect("m7_lo", "u_mux8", "I0");
        d.connect("m7_hi", "u_mux8", "I1");
        d.connect("sel1", "u_mux8", "S");
        d.connect("y", "u_mux8", "O");

        let mut sim = Sim::new(&d);
        assert!(
            sim.compiled().schedule.num_levels() >= 2,
            "Carry0->Carry1 and MUXF7->MUXF8 must form at least 2 topological levels"
        );

        for count in 0..20u64 {
            // Check all 8 counter bits match `count`.
            let mut val = 0u64;
            for b in 0..8 {
                if sim.sample(&format!("q_{b}")) == Some(1) {
                    val |= 1 << b;
                }
            }
            assert_eq!(val, count, "8-bit Carry4 counter must equal {count}");

            // Verify MUXF7 + MUXF8 4:1 mux selects q_{0..3}.
            for sel in 0..4u64 {
                sim.drive_input("sel0", (sel & 1) != 0).unwrap();
                sim.drive_input("sel1", ((sel >> 1) & 1) != 0).unwrap();
                sim.compiled_mut().eval_comb_2state();
                let expected_bit = (count >> sel) & 1;
                assert_eq!(sim.sample("y"), Some(expected_bit));
            }

            sim.step_posedge(10);
        }
    }

    #[test]
    fn four_state_logic_propagates_x_z_and_optimistically_resolves_dominated_cones() {
        let d = and_dut();
        let mut csim = CompiledSim::new_4state(&d, true);
        // FFs start in X when init_ffs_x = true.
        assert_eq!(csim.sample_4state("u_ff"), Some(Logic4::X));

        // Drive a = X, b = 0 into an AND LUT (`init = 0x8`): `0 & X` must resolve to known `0`!
        csim.drive_input_4state("a", Logic4::X).unwrap();
        csim.drive_input_4state("b", Logic4::Zero).unwrap();
        csim.step_4state(10);
        assert_eq!(
            csim.sample_4state("y"),
            Some(Logic4::Zero),
            "0 & X must resolve to deterministic 0"
        );

        // Drive a = X, b = 1 into the AND LUT: `1 & X` is unknown `X`!
        csim.drive_input_4state("a", Logic4::X).unwrap();
        csim.drive_input_4state("b", Logic4::One).unwrap();
        csim.step_4state(10);
        assert_eq!(
            csim.sample_4state("y"),
            Some(Logic4::X),
            "1 & X must propagate X"
        );

        // Drive a = 1, b = 1: resolves to known `1`.
        csim.drive_input_4state("a", Logic4::One).unwrap();
        csim.drive_input_4state("b", Logic4::One).unwrap();
        csim.step_4state(10);
        assert_eq!(csim.sample_4state("y"), Some(Logic4::One));
    }

    // ========================================================================
    // SDF Back-Annotated Gate-Level Timing Simulation Tests
    // ========================================================================

    #[test]
    fn sdf_parser_and_gate_level_timing_detects_setup_hold_and_width_violations() {
        let d = Design::structural_blinky();
        let sdf_src = r#"
(DELAYFILE
  (SDFVERSION "3.0")
  (DESIGN "blinky")
  (TIMESCALE 1ps)
  (INTERCONNECT u_ff/Q u_lut/I0 (120:150:180) (110:140:170))
  (INTERCONNECT u_lut/O u_ff/D (200:250:320) (190:240:300))
  (CELL
    (CELLTYPE "LUT6")
    (INSTANCE u_lut)
    (DELAY
      (ABSOLUTE
        (IOPATH I0 O (90:110:140) (85:105:135))
      )
    )
  )
  (CELL
    (CELLTYPE "FDRE")
    (INSTANCE u_ff)
    (DELAY
      (ABSOLUTE
        (IOPATH (posedge CLK) Q (100:120:160) (100:120:160))
      )
    )
    (TIMINGCHECK
      (SETUPHOLD D (posedge CLK) (80:100:120) (50:60:70))
      (WIDTH (posedge CLK) (200:250:300))
    )
  )
)
"#;
        let sdf = parse_sdf(sdf_src).expect("parse valid SDF");
        assert_eq!(sdf.design_name.as_deref(), Some("blinky"));
        assert_eq!(sdf.iopaths.len(), 2);
        assert_eq!(sdf.interconnects.len(), 2);
        assert_eq!(sdf.timing_checks.len(), 2);

        // Total Max corner path delay:
        // CLK->Q (160ps) + IC(u_ff/Q->u_lut/I0 = 180ps) + LUT(I0->O = 140ps) + IC(u_lut/O->u_ff/D = 320ps) = 800ps.
        // Setup @ Max = 120ps -> minimum safe clock period = 920ps.
        let mut sim = Sim::new(&d);
        sim.attach_sdf(&sdf, SdfCorner::Max);

        // Static arrival analysis must report 800ps at `u_ff`.
        let arrivals = sim
            .timing
            .as_ref()
            .unwrap()
            .analyze_static_arrivals(&sim.compiled().schedule);
        assert_eq!(arrivals.get("u_ff"), Some(&800));

        // 1. Run at 1000ps period (slack = +80ps, pulse width = 500ps >= 300ps): NO violations!
        sim.step_posedge(1000);
        assert!(
            sim.timing_violations().is_empty(),
            "1000ps period has +80ps setup slack: {:?}",
            sim.timing_violations()
        );

        // 2. Run at 850ps period (arrival 800ps + setup 120ps = 920ps > 850ps -> slack = -70ps): SETUP violation!
        sim.step_posedge(850);
        let viols = sim.timing_violations();
        assert_eq!(viols.len(), 1, "{viols:?}");
        assert_eq!(viols[0].kind, TimingViolationKind::Setup);
        assert_eq!(viols[0].instance, "u_ff");
        assert_eq!(viols[0].arrival_ps, 780);
        assert_eq!(viols[0].limit_ps, 120);
        assert_eq!(viols[0].slack_ps, -50);
        // 3. Run at 500ps period (pulse width = 250ps < 300ps min width): both WIDTH and SETUP violations!
        sim.clear_timing_violations();
        sim.step_posedge(500);
        let viols = sim.timing_violations();
        assert!(
            viols.iter().any(|v| v.kind == TimingViolationKind::PulseWidth),
            "must flag pulse width violation at 500ps period: {viols:?}"
        );
        assert!(
            viols.iter().any(|v| v.kind == TimingViolationKind::Setup),
            "must flag setup violation at 500ps period: {viols:?}"
        );

        // 4. Hold violation test: fast zero-delay path with 150ps HOLD requirement.
        let hold_sdf = r#"
(DELAYFILE
  (SDFVERSION "3.0")
  (TIMESCALE 1ps)
  (CELL
    (CELLTYPE "FDRE")
    (INSTANCE u_ff)
    (DELAY (ABSOLUTE (IOPATH (posedge CLK) Q (20:20:20) (20:20:20))))
    (TIMINGCHECK (HOLD D (posedge CLK) (150:150:150)))
  )
)
"#;
        let mut csim = CompiledSim::new(&d);
        let mut hold_tm = SimTimingModel::parse(hold_sdf, SdfCorner::Max).unwrap();
        let h_viols = hold_tm.step_timed(&mut csim, 1000);
        assert_eq!(h_viols.len(), 1);
        assert_eq!(h_viols[0].kind, TimingViolationKind::Hold);
        assert_eq!(h_viols[0].slack_ps, -130);
        // With inject_x_on_violation = true, the FF state becomes X!
        assert_eq!(csim.sample_4state("u_ff"), Some(Logic4::X));
    }

    // ========================================================================
    // Functional Coverage & SVA Property Summary Report Tests
    // ========================================================================

    #[test]
    fn functional_coverage_tracks_toggles_branches_fsm_states_and_sva_covers() {
        let src = r#"
module top (
    input  logic clk,
    input  logic en,
    output logic [3:0] cnt_o
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        if (en)
            cnt <= cnt + 1;
    end
    assign cnt_o = cnt;
    assert property (@(posedge clk) !en |-> ##1 $stable(cnt));
    assert property (@(posedge clk) en |-> ##1 !$stable(cnt));
endmodule
"#;
        let out = helion_sv::sva::synth_sv_with_sva(src, "top.sv").expect("synth sva");
        let d = Design::structural_counter();
        let mut sim = Sim::new(&d);
        sim.register_branch_coverage("counter_lsb_branch", "q0");

        // Also attach a cover property on q0 -> q1 transition.
        let mut cover_prop = out.properties[1].clone();
        cover_prop.label = Some("cov_inc".to_string());
        cover_prop.text = "cover property (@(posedge clk) q0 |-> ##1 q1)".to_string();
        cover_prop.probes = vec![
            helion_sv::sva::SvaProbe::FfBit {
                net: "q0".to_string(),
            },
            helion_sv::sva::SvaProbe::FfBit {
                net: "q1".to_string(),
            },
        ];
        cover_prop.antecedent = helion_sv::sva::SvaExpr::Bit(0);
        cover_prop.consequent = helion_sv::sva::SvaExpr::Bit(1);
        cover_prop.delay = 1;
        sim.attach_sva_covers(&d, vec![cover_prop]).unwrap();

        // Run 16 cycles so the 4-bit counter visits all 16 states (0..15) and wraps.
        for _ in 0..16 {
            sim.step_posedge(10);
        }

        let rep = sim.coverage_report();
        assert_eq!(rep.cycles_sampled, 16);
        assert_eq!(
            rep.fully_toggled_nets(),
            rep.total_nets(),
            "every net in 4-bit counter toggles both 0->1 and 1->0 over 16 cycles: {:?}",
            rep.net_toggles
        );
        assert!((rep.toggle_bin_coverage_pct() - 100.0).abs() < 1e-6);
        assert!((rep.branch_coverage_pct() - 100.0).abs() < 1e-6);

        // Auto-discovered 4-bit FSM/counter `q` (`q0..q3`) visited all 16 states.
        let fsm = rep.fsms.iter().find(|f| f.name == "q").expect("auto fsm q");
        assert_eq!(fsm.visited_states.len(), 16);
        assert_eq!(fsm.transitions.len(), 16);
        assert!((fsm.state_coverage_pct() - 100.0).abs() < 1e-6);

        // Cover property hit multiple times when q0=1 is followed by q1=1.
        assert_eq!(rep.sva.covers.len(), 1);
        assert!(rep.sva.covers[0].is_covered());
        assert!(rep.sva.covers[0].hits >= 4);
        assert!(rep.summary_line().contains("covers=1/1"));
        assert!(
            rep.tcl_lines()
                .iter()
                .any(|l| l.starts_with("cover cov_inc: COVERED")),
            "{:?}",
            rep.tcl_lines()
        );
    }

    // ========================================================================
    // Compact Waveform Delta Streaming & Protocol Decoders Tests
    // ========================================================================

    #[test]
    fn compact_wave_stream_fst_roundtrip_search_and_slice() {
        let d = Design::structural_counter();
        let stream = vcd::dump_wave_stream(&d, 32, 100);
        let vcd_str = vcd::dump_vcd(&d, 32, 100);
        let fst_bytes = stream.to_fst_bytes();
        assert!(
            fst_bytes.len() < vcd_str.len(),
            "binary FST stream ({} B) must be more compact than ASCII VCD ({} B)",
            fst_bytes.len(),
            vcd_str.len()
        );

        let decoded = CompactWaveStream::from_fst_bytes(&fst_bytes).expect("decode HFST");
        assert_eq!(decoded.signals().len(), stream.signals().len());
        assert_eq!(decoded.duration_ps, stream.duration_ps);

        // Glob and substring signal search.
        let q_sigs = decoded.search_signals("q*");
        assert_eq!(q_sigs.len(), 4);

        let q0 = decoded.find_signal("q0").expect("q0");
        let slice = decoded.slice(q0, 200, 750).expect("slice");
        assert_eq!(slice.start_ps, 200);
        assert_eq!(slice.end_ps, 750);
        assert!(!slice.transitions.is_empty());

        // Next rising edge query.
        let first_rise = decoded.next_edge(q0, 0, Some(true)).expect("rising edge");
        assert!(first_rise <= 200);
    }

    #[test]
    fn protocol_decoders_reconstruct_axilite_axistream_uart_spi_i2c_wishbone() {
        // 1. AXI4-Lite Write + Read transaction decoding
        let mut axi_wave = CompactWaveStream::new(1);
        let aclk = axi_wave.add_signal("aclk", 1);
        let awvalid = axi_wave.add_signal("awvalid", 1);
        let awready = axi_wave.add_signal("awready", 1);
        let awaddr = axi_wave.add_signal("awaddr", 32);
        let wvalid = axi_wave.add_signal("wvalid", 1);
        let wready = axi_wave.add_signal("wready", 1);
        let wdata = axi_wave.add_signal("wdata", 32);
        let wstrb = axi_wave.add_signal("wstrb", 4);
        let bvalid = axi_wave.add_signal("bvalid", 1);
        let bready = axi_wave.add_signal("bready", 1);
        let bresp = axi_wave.add_signal("bresp", 2);
        let arvalid = axi_wave.add_signal("arvalid", 1);
        let arready = axi_wave.add_signal("arready", 1);
        let araddr = axi_wave.add_signal("araddr", 32);
        let rvalid = axi_wave.add_signal("rvalid", 1);
        let rready = axi_wave.add_signal("rready", 1);
        let rdata = axi_wave.add_signal("rdata", 32);
        let rresp = axi_wave.add_signal("rresp", 2);

        axi_wave.record_change(aclk, 0, 0);
        // Cycle 1 (t=10): AW + W handshake to 0x1000_0004 with 0xCAFE_BABE
        axi_wave.record_change(awvalid, 5, 1);
        axi_wave.record_change(awready, 5, 1);
        axi_wave.record_change(awaddr, 5, 0x1000_0004);
        axi_wave.record_change(wvalid, 5, 1);
        axi_wave.record_change(wready, 5, 1);
        axi_wave.record_change(wdata, 5, 0xCAFE_BABE);
        axi_wave.record_change(wstrb, 5, 0xF);
        axi_wave.record_change(aclk, 10, 1);
        axi_wave.record_change(aclk, 15, 0);
        // Cycle 2 (t=20): B response + AR handshake to 0x1000_0004
        axi_wave.record_change(awvalid, 16, 0);
        axi_wave.record_change(wvalid, 16, 0);
        axi_wave.record_change(bvalid, 16, 1);
        axi_wave.record_change(bready, 16, 1);
        axi_wave.record_change(bresp, 16, 0);
        axi_wave.record_change(arvalid, 16, 1);
        axi_wave.record_change(arready, 16, 1);
        axi_wave.record_change(araddr, 16, 0x1000_0004);
        axi_wave.record_change(aclk, 20, 1);
        axi_wave.record_change(aclk, 25, 0);
        // Cycle 3 (t=30): R response with 0xCAFE_BABE
        axi_wave.record_change(bvalid, 26, 0);
        axi_wave.record_change(arvalid, 26, 0);
        axi_wave.record_change(rvalid, 26, 1);
        axi_wave.record_change(rready, 26, 1);
        axi_wave.record_change(rdata, 26, 0xCAFE_BABE);
        axi_wave.record_change(rresp, 26, 0);
        axi_wave.record_change(aclk, 30, 1);

        let axi_txs = decode_axi_lite(
            &axi_wave,
            &AxiLiteSignals {
                aclk,
                aresetn: None,
                awvalid,
                awready,
                awaddr,
                wvalid,
                wready,
                wdata,
                wstrb: Some(wstrb),
                bvalid,
                bready,
                bresp: Some(bresp),
                arvalid,
                arready,
                araddr,
                rvalid,
                rready,
                rdata,
                rresp: Some(rresp),
            },
        );
        assert_eq!(axi_txs.len(), 2);
        assert_eq!(axi_txs[0].kind, AxiLiteTxKind::Write);
        assert_eq!(axi_txs[0].addr, 0x1000_0004);
        assert_eq!(axi_txs[0].data, 0xCAFE_BABE);
        assert_eq!(axi_txs[1].kind, AxiLiteTxKind::Read);
        assert_eq!(axi_txs[1].addr, 0x1000_0004);
        assert_eq!(axi_txs[1].data, 0xCAFE_BABE);

        // 2. AXI4-Stream multi-beat packet decoding
        let mut axis_wave = CompactWaveStream::new(1);
        let s_clk = axis_wave.add_signal("aclk", 1);
        let tvalid = axis_wave.add_signal("tvalid", 1);
        let tready = axis_wave.add_signal("tready", 1);
        let tdata = axis_wave.add_signal("tdata", 8);
        let tlast = axis_wave.add_signal("tlast", 1);
        axis_wave.record_change(s_clk, 0, 0);
        for (i, &(byte, last)) in [(0x11u64, 0u64), (0x22, 0), (0x33, 1)].iter().enumerate() {
            let t_setup = (i as u64) * 10 + 5;
            let t_edge = (i as u64 + 1) * 10;
            axis_wave.record_change(tvalid, t_setup, 1);
            axis_wave.record_change(tready, t_setup, 1);
            axis_wave.record_change(tdata, t_setup, byte);
            axis_wave.record_change(tlast, t_setup, last);
            axis_wave.record_change(s_clk, t_edge, 1);
            axis_wave.record_change(s_clk, t_edge + 5, 0);
        }
        let pkts = decode_axi_stream(
            &axis_wave,
            &AxiStreamSignals {
                aclk: s_clk,
                tvalid,
                tready,
                tdata,
                tlast: Some(tlast),
                tkeep: None,
                tuser: None,
            },
        );
        assert_eq!(pkts.len(), 1);
        assert_eq!(pkts[0].beats, vec![0x11, 0x22, 0x33]);
        assert!(pkts[0].terminated_by_tlast);

        // 3. UART 8N1 frame decoding ('A' = 0x41)
        let mut uart_wave = CompactWaveStream::new(1);
        let tx = uart_wave.add_signal("tx", 1);
        let bit_ps = 100u64;
        uart_wave.record_change(tx, 0, 1); // idle high
        uart_wave.record_change(tx, 100, 0); // start bit
        let byte = 0x41u8;
        for b in 0..8u64 {
            uart_wave.record_change(tx, 200 + b * bit_ps, u64::from((byte >> b) & 1));
        }
        uart_wave.record_change(tx, 1000, 1); // stop bit
        let uart_frames = decode_uart(&uart_wave, &UartConfig::new_8n1(tx, bit_ps));
        assert_eq!(uart_frames.len(), 1);
        assert_eq!(uart_frames[0].data, 0x41);
        assert!(!uart_frames[0].parity_error);
        assert!(!uart_frames[0].framing_error);

        // 4. SPI Mode 0 8-bit full-duplex decoding (MOSI = 0xA5, MISO = 0x5A)
        let mut spi_wave = CompactWaveStream::new(1);
        let sclk = spi_wave.add_signal("sclk", 1);
        let cs_n = spi_wave.add_signal("cs_n", 1);
        let mosi = spi_wave.add_signal("mosi", 1);
        let miso = spi_wave.add_signal("miso", 1);
        spi_wave.record_change(sclk, 0, 0);
        spi_wave.record_change(cs_n, 0, 1);
        spi_wave.record_change(cs_n, 5, 0);
        for b in 0..8u64 {
            let bit_idx = 7 - b;
            let t_setup = 10 + b * 20;
            let t_rise = 20 + b * 20;
            let t_fall = 30 + b * 20;
            spi_wave.record_change(mosi, t_setup, (0xA5 >> bit_idx) & 1);
            spi_wave.record_change(miso, t_setup, (0x5A >> bit_idx) & 1);
            spi_wave.record_change(sclk, t_rise, 1);
            spi_wave.record_change(sclk, t_fall, 0);
        }
        spi_wave.record_change(cs_n, 200, 1);
        let spi_txs = decode_spi(
            &spi_wave,
            &SpiConfig::mode0_8bit(sclk, cs_n, mosi, Some(miso)),
        );
        assert_eq!(spi_txs.len(), 1);
        assert_eq!(spi_txs[0].mosi_words, vec![0xA5]);
        assert_eq!(spi_txs[0].miso_words, vec![0x5A]);

        // 5. I2C START + 7-bit address (0x50 Write, ACK) + Data (0x3C, ACK) + STOP
        let mut i2c_wave = CompactWaveStream::new(1);
        let scl = i2c_wave.add_signal("scl", 1);
        let sda = i2c_wave.add_signal("sda", 1);
        i2c_wave.record_change(scl, 0, 1);
        i2c_wave.record_change(sda, 0, 1);
        // START: SDA falls at t=10 while SCL=1
        i2c_wave.record_change(sda, 10, 0);
        i2c_wave.record_change(scl, 15, 0);
        // 9 bits for addr (0x50 << 1 | 0 = 0xA0, ACK = 0) + 9 bits for data (0x3C, ACK = 0)
        let i2c_bits: [u64; 18] = [
            1, 0, 1, 0, 0, 0, 0, 0, 0, // 0x50 write + ACK(0)
            0, 0, 1, 1, 1, 1, 0, 0, 0, // 0x3C + ACK(0)
        ];
        for (idx, &bit) in i2c_bits.iter().enumerate() {
            let base = 20 + (idx as u64) * 20;
            i2c_wave.record_change(sda, base, bit);
            i2c_wave.record_change(scl, base + 5, 1);
            i2c_wave.record_change(scl, base + 15, 0);
        }
        // STOP: SDA rises while SCL=1
        i2c_wave.record_change(sda, 400, 0);
        i2c_wave.record_change(scl, 405, 1);
        i2c_wave.record_change(sda, 410, 1);
        let i2c_txs = decode_i2c(&i2c_wave, &I2cSignals { scl, sda });
        assert_eq!(i2c_txs.len(), 1);
        assert_eq!(i2c_txs[0].address, 0x50);
        assert!(!i2c_txs[0].is_read);
        assert!(i2c_txs[0].address_ack);
        assert_eq!(i2c_txs[0].data_bytes.len(), 1);
        assert_eq!(i2c_txs[0].data_bytes[0].byte, 0x3C);
        assert!(i2c_txs[0].data_bytes[0].ack);

        // 6. Wishbone Classic Write + Read cycle decoding
        let mut wb_wave = CompactWaveStream::new(1);
        let wb_clk = wb_wave.add_signal("clk", 1);
        let cyc = wb_wave.add_signal("cyc", 1);
        let stb = wb_wave.add_signal("stb", 1);
        let we = wb_wave.add_signal("we", 1);
        let adr = wb_wave.add_signal("adr", 32);
        let dat_w = wb_wave.add_signal("dat_w", 32);
        let dat_r = wb_wave.add_signal("dat_r", 32);
        let ack = wb_wave.add_signal("ack", 1);
        wb_wave.record_change(wb_clk, 0, 0);
        wb_wave.record_change(cyc, 5, 1);
        wb_wave.record_change(stb, 5, 1);
        wb_wave.record_change(we, 5, 1);
        wb_wave.record_change(adr, 5, 0x2000);
        wb_wave.record_change(dat_w, 5, 0x1234_5678);
        wb_wave.record_change(ack, 5, 1);
        wb_wave.record_change(wb_clk, 10, 1);
        wb_wave.record_change(wb_clk, 15, 0);
        let wb_txs = decode_wishbone(
            &wb_wave,
            &WishboneSignals {
                clk: wb_clk,
                cyc,
                stb,
                we,
                adr,
                dat_w,
                dat_r,
                ack,
                err: None,
                sel: None,
            },
        );
        assert_eq!(wb_txs.len(), 1);
        assert!(wb_txs[0].is_write);
        assert_eq!(wb_txs[0].addr, 0x2000);
        assert_eq!(wb_txs[0].data, 0x1234_5678);
    }

    #[test]
    fn dpi_registry_parses_imports_validates_types_and_invokes_callbacks() {
        let mut sim = Sim::new(&and_dut());
        sim.register_dpi_import(
            r#"import "DPI-C" pure function int c_mac(input int a, input int b, input int acc);"#,
            |args| {
                let a = args[0].as_i64().unwrap();
                let b = args[1].as_i64().unwrap();
                let acc = args[2].as_i64().unwrap();
                Ok(DpiValue::Int(a * b + acc))
            },
        )
        .expect("register c_mac");

        sim.register_dpi_import(
            r#"import "DPI-C" context c_golden_hash = function bit [63:0] sv_hash(input longint word, input string tag);"#,
            |args| {
                let w = args[0].as_u64().unwrap();
                let tag = args[1].as_str().unwrap();
                Ok(DpiValue::UInt(w ^ (tag.len() as u64 * 0x5555)))
            },
        )
        .expect("register sv_hash");

        let sig = sim.dpi().signature("sv_hash").expect("sv_hash signature");
        assert_eq!(sig.c_name, "c_golden_hash");
        assert!(sig.is_context && !sig.is_pure);
        assert_eq!(sig.return_type, DpiType::UnsignedLongInt);

        let r1 = sim
            .call_dpi(
                "c_mac",
                &[DpiValue::Int(6), DpiValue::Int(7), DpiValue::Int(10)],
            )
            .expect("call c_mac");
        assert_eq!(r1, DpiValue::Int(52));

        // Invoke by C linkage alias `c_golden_hash` as well as SV name `sv_hash`.
        let r2 = sim
            .call_dpi(
                "c_golden_hash",
                &[DpiValue::UInt(0x1234), DpiValue::Str("pkt".to_string())],
            )
            .expect("call c_golden_hash");
        assert_eq!(r2, DpiValue::UInt(0x1234 ^ (3 * 0x5555)));
        assert_eq!(sim.dpi().call_count("c_mac"), 1);
        assert_eq!(sim.dpi().call_count("sv_hash"), 1);
        assert_eq!(sim.dpi().total_calls(), 2);
        assert_eq!(sim.dpi().call_trace().len(), 2);

        // Wrong argument count or mismatched argument type must fail loud.
        assert!(sim.call_dpi("c_mac", &[DpiValue::Int(1)]).is_err());
        assert!(sim
            .call_dpi(
                "c_mac",
                &[
                    DpiValue::Str("bad".to_string()),
                    DpiValue::Int(1),
                    DpiValue::Int(2)
                ]
            )
            .is_err());
    }

    #[test]
    fn clocking_block_samples_preponed_1step_and_drives_skewed_outputs() {
        let mut sim = Sim::new(&and_dut());
        let sv_cb = r#"
clocking cb @(posedge clk);
    default input #1step output #2ns;
    input y;
    output a, b;
endclocking
"#;
        let mut cb = ClockingBlock::parse_sv(sv_cb).expect("parse clocking block");
        assert_eq!(cb.name, "cb");
        assert_eq!(cb.clk_port, "clk");
        assert_eq!(cb.default_input_skew, ClockingSkew::Step1);
        assert_eq!(cb.default_output_skew, ClockingSkew::TimePs(2000));

        // Driving an input-only clocking variable must fail loud.
        assert!(cb.schedule_drive(sim.cycle, "y", 1, 0).is_err());

        // Schedule `cb.a <= 1; cb.b <= 1;` for the upcoming clock edge (cycle 1),
        // and `cb.b <= ##1 0;` for cycle 2.
        cb.schedule_drive(sim.cycle, "a", 1, 0).unwrap();
        cb.schedule_drive(sim.cycle, "b", 1, 0).unwrap();
        cb.schedule_drive(sim.cycle, "b", 0, 1).unwrap();

        // Edge 1 (t=10_000ps):
        // - Preponed `#1step` samples `y = 0` before the edge.
        // - Due drives (`a=1, b=1`) apply after the edge with timestamp `10_000 + 2_000 = 12_000ps`.
        let applied_1 = cb.step_clock_edge(&mut sim, 10_000).unwrap();
        assert_eq!(cb.read_sampled("y"), Some(0));
        assert_eq!(applied_1.len(), 2);
        assert!(applied_1.iter().all(|d| d.applied_time_ps == Some(12_000)));

        // Edge 2 (t=20_000ps):
        // - `a=1, b=1` were driven on edge 1, so during edge 2 the FF clocks in `1 & 1 = 1`!
        // - Preponed `#1step` before edge 2 still sees the pre-edge FF value `y = 0` (no race!).
        // - Due drive `b <= ##1 0` applies after edge 2 at `22_000ps`.
        let applied_2 = cb.step_clock_edge(&mut sim, 10_000).unwrap();
        assert_eq!(
            cb.read_sampled("y"),
            Some(0),
            "#1step preponed sample before edge 2 sees pre-edge y=0"
        );
        assert_eq!(
            sim.sample("y"),
            Some(1),
            "post-edge 2 DUT output y is now 1"
        );
        assert_eq!(applied_2.len(), 1);
        assert_eq!(applied_2[0].var_name, "b");
        assert_eq!(applied_2[0].value, 0);
        assert_eq!(applied_2[0].applied_time_ps, Some(22_000));

        // Edge 3 (t=30_000ps):
        // - Preponed `#1step` before edge 3 samples the settled `y = 1` from edge 2!
        // - At edge 3, the FF clocks in `a=1 & b=0 -> 0`.
        cb.step_clock_edge(&mut sim, 10_000).unwrap();
        assert_eq!(cb.read_sampled("y"), Some(1));
        assert_eq!(sim.sample("y"), Some(0));
    }

    #[test]
    fn svtb_mailbox_semaphore_and_scoreboard_verify_transactions() {
        // 1. Bounded and unbounded SimMailbox
        let mut mbx: SimMailbox<u32> = SimMailbox::new(2);
        assert!(mbx.is_empty());
        assert!(mbx.try_put(10));
        assert!(mbx.try_put(20));
        assert!(mbx.is_full());
        assert!(!mbx.try_put(30), "bounded mailbox rejects overflow");
        assert!(mbx.put(30).is_err());
        assert_eq!(mbx.peek().unwrap(), &10);
        assert_eq!(mbx.get().unwrap(), 10);
        assert_eq!(mbx.get().unwrap(), 20);
        assert!(mbx.get().is_err());

        // 2. SimSemaphore
        let mut sem = SimSemaphore::new(2);
        assert_eq!(sem.available_keys(), 2);
        assert!(sem.try_get(2));
        assert_eq!(sem.available_keys(), 0);
        assert!(!sem.try_get(1));
        assert!(sem.get(1).is_err());
        sem.put(3);
        assert_eq!(sem.available_keys(), 3);

        // 3. SimScoreboard
        let mut sb: SimScoreboard<u64> = SimScoreboard::new("alu_sb");
        sb.push_expected(0xAA);
        sb.push_expected(0xBB);
        sb.push_actual(10, 1, 0xAA);
        sb.push_actual(20, 2, 0xBB);
        assert!(sb.is_clean());
        assert_eq!(sb.matches(), 2);
        assert!(sb.summary_line().contains("PASS"));

        sb.push_expected(0xCC);
        sb.push_actual(30, 3, 0xDD);
        assert!(!sb.is_clean());
        assert_eq!(sb.mismatches().len(), 1);
        assert_eq!(sb.mismatches()[0].cycle, 3);
        assert_eq!(sb.mismatches()[0].expected, 0xCC);
        assert_eq!(sb.mismatches()[0].actual, 0xDD);
        assert!(sb.summary_line().contains("FAIL"));
    }

    #[test]
    fn golden_cosim_verifies_equivalence_and_pinpoints_first_diverging_cycle() {
        let src = r#"
module counter (
    input  logic clk,
    output logic led
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign led = cnt[3];
endmodule
"#;
        let rtl_synth = helion_sv::synth_sv(src, "counter.sv").expect("synth counter");
        let structural = Design::structural_counter();

        // 1. Equivalent designs pass co-simulation over 32 cycles with 0 mismatches.
        let pass_rep = run_golden_cosim(&structural, &rtl_synth, 32);
        assert!(pass_rep.is_equivalent(), "{}", pass_rep.summary_line());
        assert_eq!(pass_rep.mismatches.len(), 0);
        assert!(pass_rep.summary_line().contains("cosim PASS cycles=32"));

        // 2. Inject a LUT truth-table bug on `u_lut3` (MSB toggle LUT) in a candidate netlist:
        //    the counter's `led` output (`cnt[3]`) first rises at cycle 8 (`t=80ps`).
        let mut buggy = Design::structural_counter();
        if let Some(c) = buggy.cell_mut("u_lut3") {
            c.kind = CellKind::Lut6 { init: 0 };
        }
        let fail_rep = run_golden_cosim(&structural, &buggy, 16);
        assert!(!fail_rep.is_equivalent());
        let first = fail_rep.first_divergence.as_ref().expect("first divergence");
        assert_eq!(first.cycle, 8, "cnt[3] first diverges on cycle 8: {first}");
        assert_eq!(first.time_ps, 80);
        assert_eq!(first.signal, "led");
        assert_eq!(first.golden_value, Some(1));
        assert_eq!(first.candidate_value, Some(0));
        assert!(fail_rep.summary_line().contains("cosim FAIL"));
    }

    #[test]
    fn verilate_cpp20_and_rust_codegen_levelizes_cones_and_matches_sim() {
        let mut d = Design::structural_counter();
        // Add a Carry4, MUXF7, MUXF8, and Bram18 to exercise all levelized codegen primitives.
        d.add_cell("u_carry", CellKind::Carry4);
        d.connect("1", "u_carry", "CYINIT");
        d.connect("q0", "u_carry", "S0");
        d.connect("0", "u_carry", "DI0");
        d.connect("carry_o0", "u_carry", "O0");
        d.connect("carry_co3", "u_carry", "CO3");

        d.add_cell(
            "u_mux7",
            CellKind::BlackBox {
                module: intern("MUXF7"),
            },
        );
        d.connect("q0", "u_mux7", "I0");
        d.connect("q1", "u_mux7", "I1");
        d.connect("q2", "u_mux7", "S");
        d.connect("m7_o", "u_mux7", "O");

        d.add_cell(
            "u_mux8",
            CellKind::BlackBox {
                module: intern("MUXF8"),
            },
        );
        d.connect("m7_o", "u_mux8", "I0");
        d.connect("carry_o0", "u_mux8", "I1");
        d.connect("q3", "u_mux8", "S");
        d.connect("m8_o", "u_mux8", "O");

        d.add_cell("u_ram", CellKind::Bram18);
        d.connect("q0", "u_ram", "RADDR0");
        d.connect("q0", "u_ram", "WADDR0");
        d.connect("q1", "u_ram", "DI0");
        d.connect("q2", "u_ram", "WE");
        d.connect("ram_do0", "u_ram", "DO0");

        let cpp = emit_cpp_cycle_model(&d);
        assert!(cpp.contains("struct VHelionModel {"));
        assert!(cpp.contains("constexpr void eval_comb() noexcept"));
        assert!(cpp.contains("constexpr void tick() noexcept"));
        assert!(
            cpp.contains("ULL >> idx_u_lut0) & 1ULL"),
            "must emit branchless 64-bit LUT shift: {cpp}"
        );
        assert!(cpp.contains("std::array<uint16_t, 1024> ram_u_ram{};"));

        let rs = emit_rust_cycle_model(&d);
        assert!(rs.contains("pub struct VHelionModel {"));
        assert!(rs.contains("pub fn eval_comb(&mut self)"));
        assert!(rs.contains("pub fn tick(&mut self)"));
        assert!(
            rs.contains("u64 >> idx_u_lut0) & 1u64"),
            "must emit branchless 64-bit Rust LUT shift: {rs}"
        );

        // Compile & execute the emitted C++20 counter model if `clang++` is available,
        // verifying bit-exact agreement with `COUNTER_OVERLAY_SIM_LED`.
        let counter_cpp = emit_cpp_cycle_model(&Design::structural_counter());
        if std::process::Command::new("clang++")
            .arg("--version")
            .output()
            .is_ok()
        {
            let tmp_dir = std::env::temp_dir();
            let cpp_path = tmp_dir.join("helion_verilate_test.cpp");
            let bin_path = tmp_dir.join("helion_verilate_test_bin");
            let main_src = format!(
                "{counter_cpp}\n#include <iostream>\nint main() {{\n    VHelionModel m{{}};\n    for (int i = 0; i < 16; ++i) {{\n        m.tick();\n        std::cout << m.led;\n    }}\n    return 0;\n}}\n"
            );
            std::fs::write(&cpp_path, main_src).expect("write cpp");
            let status = std::process::Command::new("clang++")
                .args([
                    "-std=c++20",
                    "-O2",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    cpp_path.to_str().unwrap(),
                    "-o",
                    bin_path.to_str().unwrap(),
                ])
                .status()
                .expect("invoke clang++");
            assert!(status.success(), "clang++ -std=c++20 compilation failed");
            let out = std::process::Command::new(&bin_path)
                .output()
                .expect("run compiled VHelionModel");
            assert!(out.status.success());
            let led_str = String::from_utf8_lossy(&out.stdout);
            assert_eq!(led_str.trim(), COUNTER_OVERLAY_SIM_LED);
        }
    }

    #[test]
    fn vpi_registry_introspects_signals_formats_and_puts_values() {
        let d = Design::structural_counter();
        let mut sim = Sim::new(&d);

        let ports = sim.vpi_iterate_ports();
        assert!(!ports.is_empty());
        let nets = sim.vpi_iterate_nets();
        assert!(!nets.is_empty());

        // Look up both single-bit (`counter.q0`) and reconstructed 4-bit bus (`q` = `q0..q3`).
        let h_q0 = sim.vpi_handle_by_name("counter.q0").expect("counter.q0");
        let h_q_bus = sim.vpi_handle_by_name("q").expect("4-bit bus q");
        let h_led = sim.vpi_handle_by_name("led").expect("led");

        assert_eq!(sim.vpi().vpi_object_info(h_q_bus).unwrap().width, 4);

        // Advance 10 cycles (`q` counts to 10 = `4'b1010` = `0xa`).
        for _ in 0..10 {
            sim.step_posedge(10);
        }
        assert_eq!(
            sim.vpi_get_value(h_q_bus, VpiFormat::IntVal).unwrap(),
            VpiValue::IntVal(10)
        );
        assert_eq!(
            sim.vpi_get_value(h_q_bus, VpiFormat::BinStr).unwrap(),
            VpiValue::BinStr("1010".to_string())
        );
        assert_eq!(
            sim.vpi_get_value(h_q_bus, VpiFormat::HexStr).unwrap(),
            VpiValue::HexStr("a".to_string())
        );
        assert_eq!(
            sim.vpi_get_value(h_q_bus, VpiFormat::DecStr).unwrap(),
            VpiValue::DecStr("10".to_string())
        );
        assert_eq!(
            sim.vpi_get_value(h_q_bus, VpiFormat::VectorVal).unwrap(),
            VpiValue::VectorVal(vec![VpiVecVal { aval: 10, bval: 0 }])
        );

        // VpiPutFlags::NoDelay deposits a new 4-bit value (`4'b0101` = 5) onto `q`.
        sim.vpi_put_value(
            h_q_bus,
            &VpiValue::BinStr("0101".to_string()),
            VpiPutFlags::NoDelay,
        )
        .unwrap();
        assert_eq!(
            sim.vpi_get_value(h_q_bus, VpiFormat::IntVal).unwrap(),
            VpiValue::IntVal(5)
        );
        assert_eq!(
            sim.vpi_get_value(h_q0, VpiFormat::IntVal).unwrap(),
            VpiValue::IntVal(1)
        );

        // VpiPutFlags::ForceFlag holds `led` at 1 across clock edges until ReleaseFlag.
        sim.vpi_put_value(h_led, &VpiValue::IntVal(1), VpiPutFlags::ForceFlag)
            .unwrap();
        sim.step_posedge(10); // q = 6 (cnt[3] = 0), but led is forced to 1!
        assert_eq!(
            sim.vpi_get_value(h_led, VpiFormat::IntVal).unwrap(),
            VpiValue::IntVal(1)
        );
        sim.vpi_put_value(h_led, &VpiValue::IntVal(0), VpiPutFlags::ReleaseFlag)
            .unwrap();
        sim.step_posedge(10); // q = 7 (cnt[3] = 0), force released -> led = 0!
        assert_eq!(
            sim.vpi_get_value(h_led, VpiFormat::IntVal).unwrap(),
            VpiValue::IntVal(0)
        );

        // VpiPutFlags::InertialDelay schedules an update applied on the next step_posedge.
        let mut dut_sim = Sim::new(&and_dut());
        let h_a = dut_sim.vpi_handle_by_name("a").unwrap();
        let h_b = dut_sim.vpi_handle_by_name("b").unwrap();
        let h_y = dut_sim.vpi_handle_by_name("y").unwrap();
        dut_sim
            .vpi_put_value(h_a, &VpiValue::IntVal(1), VpiPutFlags::InertialDelay)
            .unwrap();
        dut_sim
            .vpi_put_value(h_b, &VpiValue::IntVal(1), VpiPutFlags::InertialDelay)
            .unwrap();
        dut_sim.step_posedge(10);
        assert_eq!(
            dut_sim.vpi_get_value(h_y, VpiFormat::IntVal).unwrap(),
            VpiValue::IntVal(1)
        );
    }

    #[test]
    fn crv_solver_randc_period_inside_dist_implication_and_backtracking() {
        // 1. `randc` cyclic permutation: 3-bit variable (`0..=7`, period 8) must visit all
        //    8 values without repetition in each 8-call epoch!
        let mut rc_cls = RandClass::new("RandcSeq");
        rc_cls.add_randc("id", 3);
        let mut rc_solver = RandConstraintSolver::new(rc_cls, 0x1234_5678);
        for _epoch in 0..3 {
            let mut seen = std::collections::HashSet::new();
            for _ in 0..8 {
                let vals = rc_solver.randomize().expect("randc randomize");
                assert!(
                    seen.insert(vals["id"]),
                    "randc repeated value before exhausting period 8"
                );
            }
            assert_eq!(seen.len(), 8);
        }

        // 2. Complex constrained-random class with `inside`, negated `inside`, `dist`,
        //    implication `->`, `solve a before b`, `unique`, and arithmetic `a + b == 20`.
        let mut pkt = RandClass::new("BusPacket");
        pkt.add_rand("mode", 2)
            .add_rand("addr", 8)
            .add_rand("len", 8)
            .add_rand("tag_a", 4)
            .add_rand("tag_b", 4);
        pkt.add_constraint(
            "c_valid",
            vec![
                RandConstraint::SolveBefore {
                    before: "mode".to_string(),
                    after: "addr".to_string(),
                },
                RandConstraint::Dist {
                    var: "mode".to_string(),
                    items: vec![
                        DistItem::val_per_value(1, 90),
                        DistItem::range_per_range(2, 3, 10),
                    ],
                },
                RandConstraint::Inside {
                    var: "addr".to_string(),
                    ranges: vec![(0x10, 0x2F), (0x80, 0x8F)],
                    negated: false,
                },
                RandConstraint::Inside {
                    var: "addr".to_string(),
                    ranges: vec![(0x18, 0x1F)],
                    negated: true,
                },
                RandConstraint::Implies {
                    cond: RandExpr::Eq(
                        Box::new(RandExpr::var("mode")),
                        Box::new(RandExpr::val(1)),
                    ),
                    body: vec![RandConstraint::Inside {
                        var: "addr".to_string(),
                        ranges: vec![(0x80, 0x8F)],
                        negated: false,
                    }],
                },
                RandConstraint::Unique(vec!["tag_a".to_string(), "tag_b".to_string()]),
                RandConstraint::Expr(RandExpr::Eq(
                    Box::new(RandExpr::Add(
                        Box::new(RandExpr::var("tag_a")),
                        Box::new(RandExpr::var("tag_b")),
                    )),
                    Box::new(RandExpr::val(20)),
                )),
                RandConstraint::Expr(RandExpr::Lt(
                    Box::new(RandExpr::var("tag_a")),
                    Box::new(RandExpr::var("tag_b")),
                )),
            ],
        );

        let mut solver = RandConstraintSolver::new(pkt, 0xCAFE_BABE);
        let mut mode1_hits = 0usize;
        for _ in 0..100 {
            let sample = solver.randomize().expect("solve BusPacket");
            let mode = sample["mode"];
            let addr = sample["addr"];
            let ta = sample["tag_a"];
            let tb = sample["tag_b"];

            assert!((1..=3).contains(&mode));
            assert!(!((0x18..=0x1F).contains(&addr)), "excluded range hit: {addr:#x}");
            assert!(
                (0x10..=0x2F).contains(&addr) || (0x80..=0x8F).contains(&addr),
                "outside allowed inside range: {addr:#x}"
            );
            if mode == 1 {
                mode1_hits += 1;
                assert!(
                    (0x80..=0x8F).contains(&addr),
                    "implication mode==1 -> addr in [0x80:0x8F] violated: {addr:#x}"
                );
            }
            assert_ne!(ta, tb, "unique constraint violated");
            assert_eq!(ta + tb, 20, "sum constraint violated");
            assert!(ta < tb, "ordering constraint violated");
        }
        assert!(
            mode1_hits >= 65,
            "weighted dist (90:10) should heavily favor mode=1, got {mode1_hits}/100"
        );

        // Deterministic seed reproducibility:
        let s1 = solver.randomize_with_seed(42).unwrap();
        let s2 = solver.randomize_with_seed(42).unwrap();
        assert_eq!(s1, s2, "randomize_with_seed must be deterministic");

        // Unsatisfiable inline constraint fails loud:
        let unsat = solver.randomize_with(&[RandConstraint::Inside {
            var: "mode".to_string(),
            ranges: vec![(0, 0)],
            negated: false,
        }]);
        assert!(unsat.is_err());
    }

    #[test]
    fn covergroup_engine_tracks_value_transition_ignore_illegal_and_cross_bins() {
        let mut cp_op = CoverpointSpec::new("cp_op", "op");
        cp_op
            .add_value_bin("read", vec![(0, 0)], 2)
            .add_value_bin("write", vec![(1, 1)], 2)
            .add_transition_bin("rd_to_wr_to_rd", vec![0, 1, 0], 1)
            .add_ignore_bin("nop", vec![(2, 2)])
            .add_illegal_bin("bad_op", vec![(3, 3)])
            .add_illegal_transition_bin("wr_to_bad", vec![1, 3]);

        let mut cp_size = CoverpointSpec::new("cp_size", "size");
        cp_size
            .add_value_bin("byte", vec![(0, 0)], 1)
            .add_value_bin("word", vec![(1, 3)], 1);

        let mut cg = CovergroupEngine::new("bus_cg");
        cg.add_coverpoint(cp_op);
        cg.add_coverpoint(cp_size);
        cg.add_cross(
            CrossSpec::new(
                "op_x_size",
                vec!["cp_op".to_string(), "cp_size".to_string()],
                1,
            )
            .with_ignore_tuple(vec!["rd_to_wr_to_rd".to_string(), "byte".to_string()])
            .with_ignore_tuple(vec!["rd_to_wr_to_rd".to_string(), "word".to_string()]),
        )
        .expect("add cross");

        let sample_pair = |cg: &mut CovergroupEngine, op: u64, size: u64| -> Vec<IllegalBinViolation> {
            let mut m = HashMap::new();
            m.insert("op".to_string(), op);
            m.insert("size".to_string(), size);
            cg.sample(&m)
        };

        // Hit (read, byte), (write, byte), (read, word), (write, word) -> also triggers `0 => 1 => 0`!
        assert!(sample_pair(&mut cg, 0, 0).is_empty());
        assert!(sample_pair(&mut cg, 1, 0).is_empty());
        assert!(sample_pair(&mut cg, 0, 1).is_empty());
        assert!(sample_pair(&mut cg, 1, 2).is_empty());
        // Ignored value `op = 2` is skipped without affecting transition history or bins.
        assert!(sample_pair(&mut cg, 2, 0).is_empty());

        let rep = cg.report();
        assert_eq!(rep.coverpoints[0].covered_bins, 3);
        assert_eq!(rep.coverpoints[0].total_active_bins, 3);
        assert!((rep.coverpoints[0].coverage_pct - 100.0).abs() < 1e-6);
        assert_eq!(rep.crosses[0].covered_tuples, 4);
        assert_eq!(rep.crosses[0].total_tuples, 4);
        assert!((rep.overall_coverage_pct - 100.0).abs() < 1e-6);
        assert!(rep.illegal_violations.is_empty());

        // Sampling `op = 3` after `op = 1` triggers BOTH `illegal_bins bad_op` AND `illegal_bins wr_to_bad`!
        let viols = sample_pair(&mut cg, 3, 0);
        assert_eq!(viols.len(), 2, "{viols:?}");
        assert_eq!(cg.violations().len(), 2);
    }
}
