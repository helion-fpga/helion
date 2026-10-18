//! SVA checker sidecars for `helion-sim` (v3.0 verification track, Part B).
//!
//! Checkers are software, never synthesized hardware. Each [`PropChecker`]
//! owns a pending-attempt queue: an antecedent match pushes an attempt with
//! `countdown = delay`, and the attempt is checked exactly `delay` cycles
//! later (delay-0 attempts are checked the same cycle they are pushed).
//! Antecedent-false cycles are vacuous passes: counted per the LRM, never
//! failures. First-failure time is the signal; checking is report-and-continue
//! with a stored-failure cap (128) while the total count is unbounded.

use helion_ir::{intern, resolve, CellKind, Design, PortDir};
use helion_sv::sva::{eval, SvaExpr, SvaImmProp, SvaProbe, SvaProperty, SvaSeverity};
use std::collections::{HashMap, VecDeque};

/// Stored-failure cap per engine; [`SvaEngine::total_failures`] is unbounded.
pub const SVA_FAILURE_CAP: usize = 128;

/// What kind of assertion failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvaFailureKind {
    Concurrent,
    Immediate,
}

/// One recorded assertion failure.
#[derive(Clone, Debug)]
pub struct SvaFailure {
    /// Sim time in picoseconds.
    pub time: u64,
    /// Posedge count (cycle 1 = first `step_posedge`).
    pub cycle: u64,
    pub kind: SvaFailureKind,
    pub text: String,
    pub message: String,
    pub severity: SvaSeverity,
}

impl std::fmt::Display for SvaFailure {
    /// Canonical single-line format for console / Messages / Tcl:
    /// `[SVA][ERROR] t=20ps cycle=2 :: assert property (@(posedge clk) en |-> ##1 $stable(cnt)) :: antecedent matched but consequent failed`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[SVA][{}] t={}ps cycle={} :: {} :: {}",
            self.severity.tag(),
            self.time,
            self.cycle,
            self.text,
            self.message
        )
    }
}

/// How one probe slot reads live sim state.
#[derive(Clone, Debug)]
enum ProbeBind {
    /// FF cell name: read `ff_q`.
    Ff(String),
    /// Input port name: read the SVA stimulus map.
    Input(String),
}

/// Resolve elaborated probes against a design. Unresolvable probes are hard
/// errors naming the property and the net.
fn bind_probes(
    design: &Design,
    prop_text: &str,
    probes: &[SvaProbe],
) -> Result<Vec<ProbeBind>, String> {
    probes
        .iter()
        .map(|pr| match pr {
            SvaProbe::FfBit { net } => {
                let cell = design
                    .net(net)
                    .and_then(|n| n.endpoints.iter().find(|e| e.pin == intern("Q")))
                    .map(|e| resolve(e.cell).to_string())
                    .filter(|c| {
                        design
                            .cell(c)
                            .map(|cell| matches!(cell.kind, CellKind::Hff))
                            .unwrap_or(false)
                    });
                match cell {
                    Some(c) => Ok(ProbeBind::Ff(c)),
                    None => Err(format!(
                        "sva attach: property `{prop_text}`: probe net `{net}` has no Hff Q driver"
                    )),
                }
            }
            SvaProbe::InputBit { name } => {
                let is_input = design
                    .ports
                    .iter()
                    .any(|p| p.name == intern(name) && matches!(p.dir, PortDir::In | PortDir::Inout));
                if is_input {
                    Ok(ProbeBind::Input(name.clone()))
                } else {
                    Err(format!(
                        "sva attach: property `{prop_text}`: probe `{name}` is not an input port"
                    ))
                }
            }
        })
        .collect()
}

fn probe_values(
    bind: &[ProbeBind],
    ff_q: &HashMap<String, bool>,
    stim: &HashMap<String, bool>,
) -> Vec<bool> {
    bind.iter()
        .map(|b| match b {
            ProbeBind::Ff(c) => ff_q.get(c).copied().unwrap_or(false),
            ProbeBind::Input(n) => stim.get(n).copied().unwrap_or(false),
        })
        .collect()
}

/// Collect `(history slot, arg expr)` for every sampled node in an expression.
fn collect_sampled(e: &SvaExpr, out: &mut Vec<(usize, SvaExpr)>) {
    match e {
        SvaExpr::Sampled { arg, slot, .. } => {
            out.push((*slot, (**arg).clone()));
            collect_sampled(arg, out);
        }
        SvaExpr::Not(x) => collect_sampled(x, out),
        SvaExpr::And(a, b)
        | SvaExpr::Or(a, b)
        | SvaExpr::Xor(a, b)
        | SvaExpr::Add(a, b)
        | SvaExpr::Sub(a, b)
        | SvaExpr::Mul(a, b)
        | SvaExpr::Shl(a, b)
        | SvaExpr::Shr(a, b)
        | SvaExpr::Eq(a, b)
        | SvaExpr::Ne(a, b)
        | SvaExpr::Lt(a, b) => {
            collect_sampled(a, out);
            collect_sampled(b, out);
        }
        SvaExpr::Mux(c, t, f) => {
            collect_sampled(c, out);
            collect_sampled(t, out);
            collect_sampled(f, out);
        }
        SvaExpr::Concat(parts) => {
            for p in parts {
                collect_sampled(p, out);
            }
        }
        SvaExpr::Slice { arg, .. } => collect_sampled(arg, out),
        SvaExpr::RedAnd(x) | SvaExpr::RedOr(x) | SvaExpr::RedXor(x) => collect_sampled(x, out),
        SvaExpr::Const(..) | SvaExpr::Bit(_) | SvaExpr::Bus(_) => {}
    }
}

#[derive(Clone, Debug)]
struct PendingAttempt {
    countdown: u32,
}

/// Checker for one concurrent `assert property`.
#[derive(Clone, Debug)]
pub struct PropChecker {
    prop: SvaProperty,
    bind: Vec<ProbeBind>,
    sampled: Vec<(usize, SvaExpr)>,
    hist: Vec<u64>,
    hist_valid: bool,
    pending: VecDeque<PendingAttempt>,
    pub attempts: u64,
    pub vacuous: u64,
    pub failures: u64,
    pub first_failure: Option<(u64, u64)>,
}

impl PropChecker {
    fn attach(design: &Design, prop: SvaProperty) -> Result<Self, String> {
        let bind = bind_probes(design, &prop.text, &prop.probes)?;
        let mut sampled = Vec::new();
        collect_sampled(&prop.antecedent, &mut sampled);
        collect_sampled(&prop.consequent, &mut sampled);
        sampled.sort_by_key(|(s, _)| *s);
        sampled.dedup_by_key(|(s, _)| *s);
        let hist = vec![0u64; prop.slots];
        Ok(PropChecker {
            prop,
            bind,
            sampled,
            hist,
            hist_valid: false,
            pending: VecDeque::new(),
            attempts: 0,
            vacuous: 0,
            failures: 0,
            first_failure: None,
        })
    }

    /// Refresh the sampled-value history from current probe values.
    fn refresh_hist(&mut self, probes: &[bool]) {
        let vals: Vec<(usize, u64)> = self
            .sampled
            .iter()
            .map(|(s, arg)| (*s, eval(arg, probes, &self.hist).0))
            .collect();
        for (s, v) in vals {
            if let Some(h) = self.hist.get_mut(s) {
                *h = v;
            }
        }
        self.hist_valid = true;
    }

    /// Preponed-region sample: evaluate the antecedent on pre-edge state,
    /// queue attempts, and check due consequents.
    fn sample_pre(
        &mut self,
        time: u64,
        cycle: u64,
        ff_q: &HashMap<String, bool>,
        stim: &HashMap<String, bool>,
        out: &mut Vec<SvaFailure>,
    ) {
        let probes = probe_values(&self.bind, ff_q, stim);
        if !self.hist_valid {
            // First sample: prev := current, so `$stable` = 1 and
            // `$rose` = `$fell` = 0 at cycle 1 (no spurious time-0 failures).
            self.refresh_hist(&probes);
        }
        // Every evaluated cycle is an attempt; a false antecedent is a
        // vacuous pass (counted, never a failure).
        self.attempts += 1;
        let (av, _) = eval(&self.prop.antecedent, &probes, &self.hist);
        if av != 0 {
            self.pending.push_back(PendingAttempt {
                countdown: self.prop.delay,
            });
        } else {
            self.vacuous += 1;
        }
        // Attempts pushed above with delay 0 are checked in this same scan.
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].countdown == 0 {
                let (cv, _) = eval(&self.prop.consequent, &probes, &self.hist);
                if cv == 0 {
                    self.failures += 1;
                    if self.first_failure.is_none() {
                        self.first_failure = Some((time, cycle));
                    }
                    let message = match &self.prop.else_msg {
                        Some(m) => format!("antecedent matched but consequent failed: {m}"),
                        None => "antecedent matched but consequent failed".to_string(),
                    };
                    out.push(SvaFailure {
                        time,
                        cycle,
                        kind: SvaFailureKind::Concurrent,
                        text: self.prop.text.clone(),
                        message,
                        severity: self.prop.severity,
                    });
                }
                self.pending.remove(i);
            } else {
                self.pending[i].countdown -= 1;
                i += 1;
            }
        }
        self.refresh_hist(&probes);
    }
}

/// Checker for one immediate `assert` / `assert final`.
#[derive(Clone, Debug)]
pub struct ImmChecker {
    prop: SvaImmProp,
    bind: Vec<ProbeBind>,
    sampled: Vec<(usize, SvaExpr)>,
    hist: Vec<u64>,
    hist_valid: bool,
    pub checks: u64,
    pub failures: u64,
    pub first_failure: Option<(u64, u64)>,
}

impl ImmChecker {
    fn attach(design: &Design, prop: SvaImmProp) -> Result<Self, String> {
        let bind = bind_probes(design, &prop.text, &prop.probes)?;
        let mut sampled = Vec::new();
        collect_sampled(&prop.expr, &mut sampled);
        sampled.sort_by_key(|(s, _)| *s);
        sampled.dedup_by_key(|(s, _)| *s);
        let hist = vec![0u64; prop.slots];
        Ok(ImmChecker {
            prop,
            bind,
            sampled,
            hist,
            hist_valid: false,
            checks: 0,
            failures: 0,
            first_failure: None,
        })
    }

    fn refresh_hist(&mut self, probes: &[bool]) {
        let vals: Vec<(usize, u64)> = self
            .sampled
            .iter()
            .map(|(s, arg)| (*s, eval(arg, probes, &self.hist).0))
            .collect();
        for (s, v) in vals {
            if let Some(h) = self.hist.get_mut(s) {
                *h = v;
            }
        }
        self.hist_valid = true;
    }

    /// Evaluate the immediate assertion once against the given snapshot.
    /// Non-final asserts run pre-edge; `assert final` runs post-commit.
    fn sample(
        &mut self,
        time: u64,
        cycle: u64,
        ff_q: &HashMap<String, bool>,
        stim: &HashMap<String, bool>,
        out: &mut Vec<SvaFailure>,
    ) {
        let probes = probe_values(&self.bind, ff_q, stim);
        if !self.hist_valid {
            self.refresh_hist(&probes);
        }
        self.checks += 1;
        let (v, _) = eval(&self.prop.expr, &probes, &self.hist);
        if v == 0 {
            self.failures += 1;
            if self.first_failure.is_none() {
                self.first_failure = Some((time, cycle));
            }
            out.push(SvaFailure {
                time,
                cycle,
                kind: SvaFailureKind::Immediate,
                text: self.prop.text.clone(),
                message: self
                    .prop
                    .else_msg
                    .clone()
                    .unwrap_or_else(|| "immediate assertion failed".to_string()),
                severity: self.prop.severity,
            });
        }
        self.refresh_hist(&probes);
    }
}

/// Per-property status for [`SvaReport`].
#[derive(Clone, Debug)]
pub struct PropStatus {
    pub label: Option<String>,
    pub text: String,
    pub attempts: u64,
    pub vacuous: u64,
    pub failures: u64,
    pub first_failure: Option<(u64, u64)>,
}

/// Per-immediate status for [`SvaReport`].
#[derive(Clone, Debug)]
pub struct ImmStatus {
    pub label: Option<String>,
    pub text: String,
    pub is_final: bool,
    pub checks: u64,
    pub failures: u64,
    pub first_failure: Option<(u64, u64)>,
}

/// Aggregate SVA status: per-assertion counters plus the unbounded total.
#[derive(Clone, Debug, Default)]
pub struct SvaReport {
    pub properties: Vec<PropStatus>,
    pub immediates: Vec<ImmStatus>,
    pub total_failures: u64,
}

impl SvaReport {
    /// One status line per assertion, in the `assert_report` Tcl shape:
    /// `assertion p1: PASS attempts=38 vacuous=12 failures=0 :: <text>`.
    pub fn tcl_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for p in &self.properties {
            let name = p.label.as_deref().unwrap_or(&p.text);
            let verdict = if p.failures == 0 { "PASS" } else { "FAIL" };
            let first = p
                .first_failure
                .map(|(t, _)| format!(", first t={t}ps"))
                .unwrap_or_default();
            lines.push(format!(
                "assertion {name}: {verdict} attempts={} vacuous={} failures={}{first} :: {}",
                p.attempts, p.vacuous, p.failures, p.text
            ));
        }
        for im in &self.immediates {
            let name = im.label.as_deref().unwrap_or(&im.text);
            let verdict = if im.failures == 0 { "PASS" } else { "FAIL" };
            let first = im
                .first_failure
                .map(|(t, _)| format!(", first t={t}ps"))
                .unwrap_or_default();
            lines.push(format!(
                "immediate {name}: {verdict} checks={} failures={}{first} :: {}",
                im.checks, im.failures, im.text
            ));
        }
        lines
    }
}

/// Owns the checker sidecars for one [`Sim`](crate::Sim).
#[derive(Clone, Debug)]
pub struct SvaEngine {
    checkers: Vec<PropChecker>,
    imms: Vec<ImmChecker>,
    failures: Vec<SvaFailure>,
    pub total_failures: u64,
}

impl SvaEngine {
    pub fn new() -> Self {
        SvaEngine {
            checkers: Vec::new(),
            imms: Vec::new(),
            failures: Vec::new(),
            total_failures: 0,
        }
    }

    pub fn is_attached(&self) -> bool {
        !self.checkers.is_empty() || !self.imms.is_empty()
    }

    /// Attach elaborated checkers, resolving probes against `design`.
    /// Replaces any previous attachment. Unresolvable probes are `Err`,
    /// naming the property and the net.
    pub fn attach(
        &mut self,
        design: &Design,
        props: Vec<SvaProperty>,
        imms: Vec<SvaImmProp>,
    ) -> Result<(), String> {
        let mut checkers = Vec::with_capacity(props.len());
        for p in props {
            checkers.push(PropChecker::attach(design, p)?);
        }
        let mut imm_checkers = Vec::with_capacity(imms.len());
        for im in imms {
            imm_checkers.push(ImmChecker::attach(design, im)?);
        }
        self.checkers = checkers;
        self.imms = imm_checkers;
        self.failures.clear();
        self.total_failures = 0;
        Ok(())
    }

    /// Preponed-region sample (before the FF next-state computation):
    /// concurrent properties plus non-final immediate asserts.
    pub fn sample_pre(
        &mut self,
        time: u64,
        cycle: u64,
        ff_q: &HashMap<String, bool>,
        stim: &HashMap<String, bool>,
    ) {
        let mut fresh = Vec::new();
        for c in &mut self.checkers {
            c.sample_pre(time, cycle, ff_q, stim, &mut fresh);
        }
        for ic in &mut self.imms {
            if !ic.prop.is_final {
                ic.sample(time, cycle, ff_q, stim, &mut fresh);
            }
        }
        self.record(fresh);
    }

    /// Postponed-region sample (after FF commit): `assert final` only.
    pub fn sample_post(
        &mut self,
        time: u64,
        cycle: u64,
        ff_q: &HashMap<String, bool>,
        stim: &HashMap<String, bool>,
    ) {
        let mut fresh = Vec::new();
        for ic in &mut self.imms {
            if ic.prop.is_final {
                ic.sample(time, cycle, ff_q, stim, &mut fresh);
            }
        }
        self.record(fresh);
    }

    /// Report-and-continue: store up to the cap, count without bound.
    fn record(&mut self, fresh: Vec<SvaFailure>) {
        for f in fresh {
            self.total_failures += 1;
            if self.failures.len() < SVA_FAILURE_CAP {
                self.failures.push(f);
            }
        }
    }

    /// Drain stored failures (what the IDE turns into markers/log/messages).
    pub fn drain_failures(&mut self) -> Vec<SvaFailure> {
        std::mem::take(&mut self.failures)
    }

    /// Clear stored failures and the total count (checker stats are kept).
    pub fn clear_failures(&mut self) {
        self.failures.clear();
        self.total_failures = 0;
    }

    pub fn report(&self) -> SvaReport {
        SvaReport {
            properties: self
                .checkers
                .iter()
                .map(|c| PropStatus {
                    label: c.prop.label.clone(),
                    text: c.prop.text.clone(),
                    attempts: c.attempts,
                    vacuous: c.vacuous,
                    failures: c.failures,
                    first_failure: c.first_failure,
                })
                .collect(),
            immediates: self
                .imms
                .iter()
                .map(|ic| ImmStatus {
                    label: ic.prop.label.clone(),
                    text: ic.prop.text.clone(),
                    is_final: ic.prop.is_final,
                    checks: ic.checks,
                    failures: ic.failures,
                    first_failure: ic.first_failure,
                })
                .collect(),
            total_failures: self.total_failures,
        }
    }
}

impl Default for SvaEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, Design, PortDir};
    use helion_sv::sva::synth_sv_with_sva;

    /// Hand-built CORRECT 4-bit enable counter netlist (bypasses the
    /// pre-existing `helion-sv` adder/mux LUT-init bug). Net names match the
    /// elaborator's `bit_name` convention (`cnt_0`..`cnt_3`, `en`) so
    /// elaborated properties attach cleanly.
    fn good_counter_design() -> Design {
        let mut d = Design::new("top");
        d.add_port("clk", PortDir::In);
        d.add_port("en", PortDir::In);
        d.add_port("cnt_o", PortDir::Out);
        // Bit i increment: q[i] ^ AND(q[0..i-1]); I0=en(mux sel), I1=q[i], I2=q[i-1], ...
        // D[i] = en ? (q[i] ^ carry) : q[i].
        for i in 0..4u32 {
            let mut init = 0u64;
            for addr in 0..64u64 {
                let en = (addr >> 0) & 1;
                let qi = (addr >> 1) & 1;
                let mut cin = 1u64;
                for j in 0..i {
                    // I_{j+2} = q[j]
                    cin &= (addr >> (j + 2)) & 1;
                }
                let inc = qi ^ cin;
                let dval = if en == 1 { inc } else { qi };
                init |= dval << addr;
            }
            d.add_cell(&format!("u_lut{i}"), CellKind::Lut6 { init });
            d.add_cell(&format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", &format!("u_ff{i}"), "CLK");
            d.connect(&format!("d{i}"), &format!("u_lut{i}"), "O");
            d.connect(&format!("d{i}"), &format!("u_ff{i}"), "D");
            // Q net uses the elaborator's bit_name convention ("cnt_{i}").
            let qnet = format!("cnt_{i}");
            d.connect(&qnet, &format!("u_ff{i}"), "Q");
            d.connect("en", &format!("u_lut{i}"), "I0");
            d.connect(&qnet, &format!("u_lut{i}"), "I1");
            for j in 0..i {
                d.connect(&format!("cnt_{j}"), &format!("u_lut{i}"), &format!("I{}", j + 2));
            }
        }
        d
    }

    /// Elaborate SVA via `synth_sv_with_sva` (parser/elaborator under test),
    /// but simulate the hand-built correct counter (bypasses the
    /// pre-existing adder LUT-init synthesis bug).
    fn en_counter(extra_sva: &str) -> crate::Sim {
        let src = format!(
            r#"
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
{extra_sva}
endmodule
"#
        );
        let out = synth_sv_with_sva(&src, "top.sv").expect("synth with sva");
        let design = good_counter_design();
        let mut sim = crate::Sim::new(&design);
        sim.attach_sva(&design, out.properties, out.immediates)
            .expect("attach");
        sim
    }

    /// Repeating `0,0,1,1,1,0` stimulus; returns the sim after `cycles` steps.
    fn run_pattern(sim: &mut crate::Sim, cycles: usize) {
        let pat = [false, false, true, true, true, false];
        for i in 0..cycles {
            sim.drive("en", pat[i % pat.len()]).expect("drive en");
            sim.step_posedge(10);
        }
    }

    #[test]
    fn sva_positive_enable_counter() {
        // NOTE on the immediate below: the design doc's `$stable(cnt) || en`
        // alone fails at every 1->0 `en` transition under this repeating
        // pattern (the counter visibly advanced on the previous edge while
        // `en` is already 0), so the `|| $fell(en)` disjunct closes that
        // exact gap. `assert final` below keeps the doc's literal text and
        // exercises the postponed region.
        let mut sim = en_counter(
            r#"
    assert property (@(posedge clk) !en |-> ##1 $stable(cnt));
    assert property (@(posedge clk) $rose(en) |-> ##1 !$stable(cnt));
    always_ff @(posedge clk) begin
        assert ($stable(cnt) || en || $fell(en));
        assert final ($stable(cnt) || en);
    end"#,
        );
        run_pattern(&mut sim, 40);
        let failures = sim.sva_failures();
        assert!(
            failures.is_empty(),
            "positive test must not fail: {failures:?}"
        );
        let rep = sim.sva_report();
        assert_eq!(rep.properties.len(), 2);
        assert_eq!(rep.immediates.len(), 2);
        for p in &rep.properties {
            assert!(p.attempts > 0, "{p:?}");
            assert!(p.vacuous > 0, "{p:?}");
            assert_eq!(p.failures, 0);
        }
        // `!en` is false while en=1: those cycles are vacuous, not failures.
        assert_eq!(rep.properties[0].attempts, 40);
        assert_eq!(rep.properties[0].vacuous, 20, "{:?}", rep.properties[0]);
        for im in &rep.immediates {
            assert_eq!(im.checks, 40);
            assert_eq!(im.failures, 0);
        }
        assert_eq!(rep.total_failures, 0);
        // Tcl-shaped lines render.
        let lines = rep.tcl_lines();
        assert_eq!(lines.len(), 4);
        assert!(
            lines[0].contains("PASS attempts=40 vacuous=20 failures=0"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn sva_negative_known_time_delay1() {
        let mut sim = en_counter(
            "assert property (@(posedge clk) en |-> ##1 $stable(cnt));",
        );
        for _ in 0..5 {
            sim.drive("en", true).expect("drive en");
            sim.step_posedge(10);
        }
        let failures = sim.sva_failures();
        let times: Vec<u64> = failures.iter().map(|f| f.time).collect();
        assert_eq!(times, vec![20, 30, 40, 50], "{failures:?}");
        let cycles: Vec<u64> = failures.iter().map(|f| f.cycle).collect();
        assert_eq!(cycles, vec![2, 3, 4, 5]);
        assert_eq!(
            failures[0].to_string(),
            "[SVA][ERROR] t=20ps cycle=2 :: \
             assert property (@(posedge clk) en |-> ##1 $stable(cnt)) :: \
             antecedent matched but consequent failed"
        );
        let rep = sim.sva_report();
        assert_eq!(rep.properties[0].failures, 4);
        assert_eq!(rep.properties[0].first_failure, Some((20, 2)));
        assert_eq!(rep.total_failures, 4);
    }

    #[test]
    fn sva_negative_multi_attempt_delay2() {
        let mut sim = en_counter(
            "assert property (@(posedge clk) en |-> ##2 $stable(cnt));",
        );
        for _ in 0..5 {
            sim.drive("en", true).expect("drive en");
            sim.step_posedge(10);
        }
        let failures = sim.sva_failures();
        // Interleaved attempts land exactly 2 cycles after their antecedent.
        let times: Vec<u64> = failures.iter().map(|f| f.time).collect();
        assert_eq!(times, vec![30, 40, 50], "{failures:?}");
    }

    #[test]
    fn sva_negative_immediate_known_time() {
        let mut sim = en_counter(
            "always_ff @(posedge clk) begin assert (cnt < 4'h8); end",
        );
        for _ in 0..12 {
            sim.drive("en", true).expect("drive en");
            sim.step_posedge(10);
        }
        let failures = sim.sva_failures();
        assert!(!failures.is_empty());
        // cnt reaches 8 at the preponed sample of cycle 9.
        assert_eq!(failures[0].time, 90, "{failures:?}");
        assert_eq!(failures[0].cycle, 9);
        assert_eq!(failures[0].kind, SvaFailureKind::Immediate);
        let rep = sim.sva_report();
        assert_eq!(rep.immediates[0].failures, 4, "{rep:?}");
        assert_eq!(rep.immediates[0].first_failure, Some((90, 9)));
    }

    #[test]
    fn sva_vacuous_counts_not_failures() {
        let mut sim = en_counter(
            "assert property (@(posedge clk) en |-> ##1 $stable(cnt));",
        );
        for _ in 0..10 {
            sim.drive("en", false).expect("drive en");
            sim.step_posedge(10);
        }
        assert!(sim.sva_failures().is_empty());
        let rep = sim.sva_report();
        let p = &rep.properties[0];
        // Antecedent never true: every attempt is vacuous, none fail.
        assert_eq!(p.attempts, p.vacuous);
        assert_eq!(p.attempts, 10);
        assert_eq!(p.failures, 0);
    }

    #[test]
    fn sva_attach_unresolvable_probe_is_loud() {
        let src = r#"
module top (
    input  logic clk,
    input  logic en,
    output logic [3:0] cnt_o
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign cnt_o = cnt;
    assert property (@(posedge clk) en |-> ##1 $stable(cnt));
endmodule
"#;
        let out = synth_sv_with_sva(src, "top.sv").expect("synth");
        let mut sim = crate::Sim::new(&helion_ir::Design::structural_blinky());
        let e = sim
            .attach_sva(&helion_ir::Design::structural_blinky(), out.properties, out.immediates)
            .expect_err("must fail");
        assert!(
            e.contains("assert property (@(posedge clk) en |-> ##1 $stable(cnt))"),
            "{e}"
        );
        assert!(e.contains("en"), "{e}");
    }

    #[test]
    fn sva_drive_validates_ports_and_clear_works() {
        let mut sim = en_counter("");
        assert!(sim.drive("nope", true).is_err());
        // `cnt_o` is an output port: not drivable.
        assert!(sim.drive("cnt_o", true).is_err());
        sim.drive("en", true).expect("drive en");
        sim.step_posedge(10);
        assert!(sim.sva_failures().is_empty());
        sim.clear_sva_failures();
        assert_eq!(sim.sva_report().total_failures, 0);
    }

    #[test]
    fn sva_failure_cap_bounds_storage_not_count() {
        let mut sim = en_counter(
            "assert property (@(posedge clk) en |-> ##1 $stable(cnt));",
        );
        // 301 cycles: the cycle-1 attempt is first checked at cycle 2.
        for _ in 0..301 {
            sim.drive("en", true).expect("drive en");
            sim.step_posedge(10);
        }
        let rep = sim.sva_report();
        assert_eq!(rep.total_failures, 300);
        let drained = sim.sva_failures();
        assert_eq!(drained.len(), SVA_FAILURE_CAP);
        // Draining twice yields nothing new.
        assert!(sim.sva_failures().is_empty());
    }
}

