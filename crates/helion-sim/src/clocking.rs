//! IEEE 1800 SystemVerilog `clocking cb @(posedge clk)` synchronous input skew
//! sampling and output skew driving (`ClockingBlock`).
//!
//! Eliminates testbench/DUT clock-edge race conditions by sampling all clocking
//! `input` variables in the Preponed region (`#1step` before the active clock edge)
//! and applying synchronous `output` drives (`cb.sig <= ##N val`) after the clock
//! edge with configurable output skew timestamps.

use crate::Sim;
use std::collections::{HashMap, VecDeque};

/// Input or output skew specification for a [`ClockingBlock`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockingSkew {
    /// `#1step`: sample in the Preponed region immediately before the clock edge.
    Step1,
    /// `#0`: zero-skew (Observed / Re-NBA region at the clock edge).
    Zero,
    /// Explicit time offset in picoseconds (`#Nps` / `#Nns`).
    TimePs(u64),
}

impl ClockingSkew {
    pub const fn offset_ps(self) -> u64 {
        match self {
            Self::Step1 | Self::Zero => 0,
            Self::TimePs(ps) => ps,
        }
    }
}

/// Direction of a signal declared inside a [`ClockingBlock`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockingVarDir {
    Input,
    Output,
    Inout,
}

/// One signal bound inside a [`ClockingBlock`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockingSignal {
    pub name: String,
    pub dut_signal: String,
    pub dir: ClockingVarDir,
    pub input_skew: ClockingSkew,
    pub output_skew: ClockingSkew,
}

/// A synchronous drive scheduled via a clocking block (`cb.sig <= ##cycle_delay val`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledDrive {
    pub var_name: String,
    pub dut_port: String,
    pub value: u64,
    pub target_cycle: u64,
    pub skew_ps: u64,
    pub applied_time_ps: Option<u64>,
}

/// IEEE 1800 SystemVerilog synchronous `clocking` block controller.
#[derive(Clone, Debug)]
pub struct ClockingBlock {
    pub name: String,
    pub clk_port: String,
    pub default_input_skew: ClockingSkew,
    pub default_output_skew: ClockingSkew,
    signals: HashMap<String, ClockingSignal>,
    /// Preponed-region (`#1step`) sampled values of `input` / `inout` clocking variables.
    sampled_inputs: HashMap<String, u64>,
    /// History of `(cycle, time_ps, sampled_map)` snapshots.
    sample_history: Vec<(u64, u64, HashMap<String, u64>)>,
    /// Pending synchronous output drives (`##N` cycle delay queue).
    pending_drives: VecDeque<ScheduledDrive>,
    /// History of committed synchronous output drives with their skewed timestamps.
    applied_drives: Vec<ScheduledDrive>,
}

fn parse_skew_token(tok: &str) -> Result<ClockingSkew, String> {
    let s = tok.trim().trim_start_matches('#').trim();
    if s.eq_ignore_ascii_case("1step") {
        return Ok(ClockingSkew::Step1);
    }
    if s == "0" || s == "0ps" || s == "0ns" {
        return Ok(ClockingSkew::Zero);
    }
    let lower = s.to_ascii_lowercase();
    if let Some(num) = lower.strip_suffix("ps") {
        let v: u64 = num
            .trim()
            .parse()
            .map_err(|_| format!("clocking: invalid ps skew `{tok}`"))?;
        return Ok(ClockingSkew::TimePs(v));
    }
    if let Some(num) = lower.strip_suffix("ns") {
        let v: f64 = num
            .trim()
            .parse()
            .map_err(|_| format!("clocking: invalid ns skew `{tok}`"))?;
        return Ok(ClockingSkew::TimePs((v * 1000.0).round() as u64));
    }
    let v: u64 = lower
        .parse()
        .map_err(|_| format!("clocking: unrecognized skew `{tok}`"))?;
    Ok(ClockingSkew::TimePs(v))
}

impl ClockingBlock {
    /// Create a new [`ClockingBlock`] triggered on `@(posedge clk_port)` with default
    /// `input #1step` and `output #0`.
    pub fn new(name: impl Into<String>, clk_port: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            clk_port: clk_port.into(),
            default_input_skew: ClockingSkew::Step1,
            default_output_skew: ClockingSkew::Zero,
            signals: HashMap::new(),
            sampled_inputs: HashMap::new(),
            sample_history: Vec::new(),
            pending_drives: VecDeque::new(),
            applied_drives: Vec::new(),
        }
    }

    /// Configure default input and output skews (`default input <in_skew> output <out_skew>;`).
    pub fn with_default_skews(mut self, input_skew: ClockingSkew, output_skew: ClockingSkew) -> Self {
        self.default_input_skew = input_skew;
        self.default_output_skew = output_skew;
        self
    }

    /// Add a clocking `input` variable sampling `dut_signal`.
    pub fn add_input(&mut self, name: impl Into<String>, dut_signal: impl Into<String>) {
        let name = name.into();
        self.signals.insert(
            name.clone(),
            ClockingSignal {
                name,
                dut_signal: dut_signal.into(),
                dir: ClockingVarDir::Input,
                input_skew: self.default_input_skew,
                output_skew: self.default_output_skew,
            },
        );
    }

    /// Add a clocking `output` variable driving `dut_port` with optional explicit skew.
    pub fn add_output(&mut self, name: impl Into<String>, dut_port: impl Into<String>) {
        let name = name.into();
        self.signals.insert(
            name.clone(),
            ClockingSignal {
                name,
                dut_signal: dut_port.into(),
                dir: ClockingVarDir::Output,
                input_skew: self.default_input_skew,
                output_skew: self.default_output_skew,
            },
        );
    }

    /// Add a bidirectional `inout` clocking variable.
    pub fn add_inout(&mut self, name: impl Into<String>, dut_signal: impl Into<String>) {
        let name = name.into();
        self.signals.insert(
            name.clone(),
            ClockingSignal {
                name,
                dut_signal: dut_signal.into(),
                dir: ClockingVarDir::Inout,
                input_skew: self.default_input_skew,
                output_skew: self.default_output_skew,
            },
        );
    }

    /// Parse a SystemVerilog `clocking <name> @(posedge <clk>); ... endclocking` block.
    pub fn parse_sv(src: &str) -> Result<Self, String> {
        let trimmed = src.trim();
        let body = trimmed
            .strip_prefix("clocking")
            .ok_or_else(|| "clocking parse: must start with `clocking`".to_string())?
            .trim();
        let body = body
            .split("endclocking")
            .next()
            .ok_or_else(|| "clocking parse: missing `endclocking`".to_string())?
            .trim();

        let (header, stmts) = body
            .split_once(';')
            .ok_or_else(|| "clocking parse: missing `;` after clocking header".to_string())?;
        let (cb_name, event_part) = header
            .split_once('@')
            .ok_or_else(|| "clocking parse: missing `@(posedge ...)` clocking event".to_string())?;
        let cb_name = cb_name.trim();
        let ev = event_part
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim();
        let clk_port = ev
            .strip_prefix("posedge")
            .unwrap_or(ev)
            .trim()
            .to_string();

        let mut cb = Self::new(cb_name, clk_port);

        for raw_stmt in stmts.split(';') {
            let stmt = raw_stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            if let Some(rest) = stmt.strip_prefix("default") {
                let tokens: Vec<&str> = rest.split_whitespace().collect();
                let mut i = 0;
                while i + 1 < tokens.len() {
                    match tokens[i] {
                        "input" => {
                            cb.default_input_skew = parse_skew_token(tokens[i + 1])?;
                            i += 2;
                        }
                        "output" => {
                            cb.default_output_skew = parse_skew_token(tokens[i + 1])?;
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                continue;
            }
            let (dir, mut rest) = if let Some(r) = stmt.strip_prefix("inout") {
                (ClockingVarDir::Inout, r.trim())
            } else if let Some(r) = stmt.strip_prefix("input") {
                (ClockingVarDir::Input, r.trim())
            } else if let Some(r) = stmt.strip_prefix("output") {
                (ClockingVarDir::Output, r.trim())
            } else {
                return Err(format!("clocking parse: unknown statement `{stmt}`"));
            };

            let mut in_skew = cb.default_input_skew;
            let mut out_skew = cb.default_output_skew;
            if rest.starts_with('#') {
                if let Some((skew_tok, after_skew)) = rest.split_once(char::is_whitespace) {
                    let sk = parse_skew_token(skew_tok)?;
                    if matches!(dir, ClockingVarDir::Input | ClockingVarDir::Inout) {
                        in_skew = sk;
                    }
                    if matches!(dir, ClockingVarDir::Output | ClockingVarDir::Inout) {
                        out_skew = sk;
                    }
                    rest = after_skew.trim();
                }
            }

            for var_part in rest.split(',') {
                let vp = var_part.trim();
                if vp.is_empty() {
                    continue;
                }
                let (vname, dut_sig) = if let Some((lhs, rhs)) = vp.split_once('=') {
                    (lhs.trim().to_string(), rhs.trim().to_string())
                } else {
                    (vp.to_string(), vp.to_string())
                };
                cb.signals.insert(
                    vname.clone(),
                    ClockingSignal {
                        name: vname,
                        dut_signal: dut_sig,
                        dir,
                        input_skew: in_skew,
                        output_skew: out_skew,
                    },
                );
            }
        }

        Ok(cb)
    }

    /// Sample all `input` and `inout` clocking variables in the Preponed region
    /// (`#1step` prior to `sim.step_posedge`).
    pub fn sample_preponed(&mut self, sim: &Sim) {
        let mut snap = HashMap::new();
        for sig in self.signals.values() {
            if matches!(sig.dir, ClockingVarDir::Input | ClockingVarDir::Inout) {
                if let Some(val) = sim.sample(&sig.dut_signal) {
                    self.sampled_inputs.insert(sig.name.clone(), val);
                    snap.insert(sig.name.clone(), val);
                }
            }
        }
        self.sample_history.push((sim.cycle + 1, sim.time, snap));
    }

    /// Read the most recent Preponed (`#1step`) sampled value of a clocking `input` variable.
    pub fn read_sampled(&self, var_name: &str) -> Option<u64> {
        self.sampled_inputs.get(var_name).copied()
    }

    /// Schedule a synchronous clocking drive (`cb.<var_name> <= ##<cycle_delay> <value>`).
    ///
    /// A `cycle_delay` of `0` applies on the next active clock edge (`sim.cycle + 1`);
    /// a `cycle_delay` of `N` applies `N` clock edges later (`sim.cycle + 1 + N`).
    pub fn schedule_drive(
        &mut self,
        current_cycle: u64,
        var_name: &str,
        value: u64,
        cycle_delay: u64,
    ) -> Result<(), String> {
        let sig = self
            .signals
            .get(var_name)
            .ok_or_else(|| format!("clocking {}: unknown variable `{var_name}`", self.name))?;
        if sig.dir == ClockingVarDir::Input {
            return Err(format!(
                "clocking {}: cannot drive input-only variable `{var_name}`",
                self.name
            ));
        }
        let target_cycle = current_cycle + 1 + cycle_delay;
        // Last-drive-wins semantics for multiple drives targeting the same variable and cycle.
        self.pending_drives
            .retain(|d| !(d.var_name == var_name && d.target_cycle == target_cycle));
        self.pending_drives.push_back(ScheduledDrive {
            var_name: var_name.to_string(),
            dut_port: sig.dut_signal.clone(),
            value,
            target_cycle,
            skew_ps: sig.output_skew.offset_ps(),
            applied_time_ps: None,
        });
        Ok(())
    }

    /// Apply all scheduled synchronous drives due at `sim.cycle` (Re-NBA region after clock edge).
    pub fn apply_due_drives(&mut self, sim: &mut Sim) -> Result<Vec<ScheduledDrive>, String> {
        let mut applied = Vec::new();
        let mut i = 0;
        while i < self.pending_drives.len() {
            if self.pending_drives[i].target_cycle <= sim.cycle {
                let mut drv = self.pending_drives.remove(i).unwrap();
                sim.drive(&drv.dut_port, drv.value != 0)?;
                drv.applied_time_ps = Some(sim.time + drv.skew_ps);
                self.applied_drives.push(drv.clone());
                applied.push(drv);
            } else {
                i += 1;
            }
        }
        Ok(applied)
    }

    /// Execute one complete IEEE 1800 clocking-block clock cycle:
    /// 1. Sample `input` / `inout` variables in the Preponed region (`#1step` before edge).
    /// 2. Advance `sim.step_posedge(period_ps)`.
    /// 3. Apply due synchronous `output` drives (`cb.sig <= ...`) with output skew timestamps.
    pub fn step_clock_edge(
        &mut self,
        sim: &mut Sim,
        period_ps: u64,
    ) -> Result<Vec<ScheduledDrive>, String> {
        self.sample_preponed(sim);
        sim.step_posedge(period_ps);
        self.apply_due_drives(sim)
    }

    /// All committed synchronous drives with their skewed application timestamps.
    pub fn applied_drives(&self) -> &[ScheduledDrive] {
        &self.applied_drives
    }

    /// Preponed sample history `(cycle, pre_edge_time_ps, values)`.
    pub fn sample_history(&self) -> &[(u64, u64, HashMap<String, u64>)] {
        &self.sample_history
    }
}
