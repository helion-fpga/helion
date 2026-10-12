//! Event-driven kernel driven by LUT INIT + FF in the netlist (not a hardcoded toggle).
//!
//! Overlay / behavioral LED+waveform completion is labeled **`overlay` / `sim_DONE`**
//! ([`OverlaySimReport`]) — never bare board DONE.

use helion_ir::{CellKind, Design, PortDir};
use helion_sv::sva::{SvaImmProp, SvaProperty};
use std::collections::HashMap;

pub mod sva;
pub mod vcd;
use sva::{SvaEngine, SvaFailure, SvaReport};

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
}

impl Sim {
    pub fn new(d: &Design) -> Self {
        let mut lut_init = HashMap::new();
        for c in &d.cells {
            if let CellKind::Lut6 { init } = c.kind {
                lut_init.insert(c.name.clone(), init);
            }
        }
        let mut lut_pin_nets: HashMap<String, Vec<(u8, String)>> = HashMap::new();
        let mut net_ff: HashMap<String, String> = HashMap::new();
        let mut net_lut_o: HashMap<String, String> = HashMap::new();
        let mut ff_d_lut = HashMap::new();
        for n in &d.nets {
            if let Some(q) = n.endpoints.iter().find(|e| e.pin == "Q") {
                net_ff.insert(n.name.clone(), q.cell.clone());
            }
            if let Some(o) = n.endpoints.iter().find(|e| e.pin == "O") {
                net_lut_o.insert(n.name.clone(), o.cell.clone());
            }
            for e in &n.endpoints {
                if let Some(rest) = e.pin.strip_prefix('I') {
                    if let Ok(pin) = rest.parse::<u8>() {
                        if pin < 6 {
                            lut_pin_nets
                                .entry(e.cell.clone())
                                .or_default()
                                .push((pin, n.name.clone()));
                        }
                    }
                }
            }
            let lut_o = n.endpoints.iter().find(|e| e.pin == "O");
            let ff_d = n.endpoints.iter().find(|e| e.pin == "D");
            if let (Some(l), Some(f)) = (lut_o, ff_d) {
                ff_d_lut.insert(f.cell.clone(), l.cell.clone());
            }
        }
        let pin_ix = d.pin_index();
        let iob_from_ff = d.cells.iter().find_map(|c| {
            if !matches!(c.kind, CellKind::IobOut) {
                return None;
            }
            let net = pin_ix.net_on(&c.name, "I")?;
            d.net(net)?
                .endpoints
                .iter()
                .find(|e| e.pin == "Q")
                .map(|e| e.cell.clone())
        });
        let mut ff_q = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                ff_q.insert(c.name.clone(), false);
            }
        }
        let mut ports = HashMap::new();
        for p in &d.ports {
            ports.insert(p.name.clone(), p.dir);
        }
        let mut iob_pad_to_i = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::IobOut) {
                if let (Some(pad), Some(i)) =
                    (pin_ix.net_on(&c.name, "PAD"), pin_ix.net_on(&c.name, "I"))
                {
                    iob_pad_to_i.insert(pad.to_string(), i.to_string());
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
                .find(|e| e.pin == "Q")
                .map(|e| NetDriver::Ff(e.cell.clone()))
                .or_else(|| {
                    n.endpoints
                        .iter()
                        .find(|e| e.pin == "O")
                        .map(|e| NetDriver::Lut(e.cell.clone()))
                })
                .unwrap_or(NetDriver::Undriven);
            net_drivers.insert(n.name.clone(), driver);
        }
        let mut lut_input_nets: HashMap<String, [Option<String>; 6]> = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Lut6 { .. }) {
                let mut pins: [Option<String>; 6] = Default::default();
                for (i, slot) in pins.iter_mut().enumerate() {
                    *slot = pin_ix
                        .net_on(&c.name, &format!("I{i}"))
                        .map(|s| s.to_string());
                }
                lut_input_nets.insert(c.name.clone(), pins);
            }
        }
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
        self.time += delay;
        self.cycle += 1;
        // SVA preponed region: concurrent properties + non-final immediate
        // asserts sample the pre-edge state.
        self.sva.sample_pre(self.time, self.cycle, &self.ff_q, &self.stim);
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
        for (k, v) in next_q {
            self.ff_q.insert(k, v);
        }
        self.led = self
            .iob_from_ff
            .as_ref()
            .and_then(|f| self.ff_q.get(f))
            .copied()
            .unwrap_or(false);
        self.apply_forces();
        // SVA postponed region: `assert final` sees the committed state.
        self.sva
            .sample_post(self.time, self.cycle, &self.ff_q, &self.stim);
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
                if e.pin == "Q" {
                    if let Some(q) = self.ff_q.get(&e.cell) {
                        val = Some(*q);
                        break;
                    }
                } else if e.pin == "O" {
                    if let Some(o) = self.lut_output(&e.cell) {
                        val = Some(o);
                        break;
                    }
                }
            }
            if let Some(v) = val {
                out.push((n.name.clone(), v));
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
    use helion_ir::{CellKind, Design, PortDir};

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
            d.add_cell(format!("lut{i}"), CellKind::Lut6 { init: 1 });
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
            d.add_cell(format!("u_lut{i}"), CellKind::Lut6 { init: inc_init(i) });
            d.add_cell(format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", format!("u_ff{i}"), "CLK");
            d.connect(format!("d{i}"), format!("u_lut{i}"), "O");
            d.connect(format!("d{i}"), format!("u_ff{i}"), "D");
            d.connect(format!("q{i}"), format!("u_ff{i}"), "Q");
            for pin in 0..=i {
                d.connect(format!("q{pin}"), format!("u_lut{i}"), format!("I{pin}"));
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
            .filter(|p| d.net_on("u_lut5", &format!("I{p}")).is_some())
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
}
