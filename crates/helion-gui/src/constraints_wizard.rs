//! Constraints Wizard data model.
//!
//! Pure (egui-free) logic behind the IDE's guided timing-constraints wizard:
//! entry types, validation against the loaded [`helion_ir::Design`], SDC text
//! generation, and a parse round-trip through [`helion_sta::load_xdc`] that
//! proves the generated file is real, loadable constraint syntax.
//!
//! The egui presentation lives in the `helion-ide` binary
//! (`bin/helion-ide/constraints_wizard.rs`); this module is what the golden
//! tests exercise.

use helion_ir::{Design, PortDir};

/// Wizard pages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardStep {
    Clocks,
    InputDelays,
    OutputDelays,
    Exceptions,
    Review,
}

impl WizardStep {
    pub const ALL: [WizardStep; 5] = [
        WizardStep::Clocks,
        WizardStep::InputDelays,
        WizardStep::OutputDelays,
        WizardStep::Exceptions,
        WizardStep::Review,
    ];

    pub fn title(self) -> &'static str {
        match self {
            WizardStep::Clocks => "Clocks",
            WizardStep::InputDelays => "Input delays",
            WizardStep::OutputDelays => "Output delays",
            WizardStep::Exceptions => "Exceptions",
            WizardStep::Review => "Review & generate",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            WizardStep::Clocks => {
                "Define each clock: the port it enters on, its period, and duty cycle."
            }
            WizardStep::InputDelays => {
                "Constrain input ports relative to a clock (-max for setup, -min for hold)."
            }
            WizardStep::OutputDelays => {
                "Constrain output ports relative to a clock (-max for setup, -min for hold)."
            }
            WizardStep::Exceptions => {
                "False paths and max-delay overrides for paths the clocks don't cover."
            }
            WizardStep::Review => "Check the generated SDC, write it, then re-run timing.",
        }
    }
}

/// One clock definition: primary (`create_clock`) or generated
/// (`create_generated_clock`).
#[derive(Clone, Debug)]
pub struct WizardClock {
    pub name: String,
    pub port: String,
    pub period_ns: f64,
    /// Duty cycle percent, exclusive (0, 100). Recorded in `-waveform`.
    pub duty_pct: f64,
    pub generated: bool,
    /// Master clock name (generated clocks only).
    pub master: String,
    pub divide_by: u32,
    pub multiply_by: u32,
    pub invert: bool,
}

impl WizardClock {
    pub fn primary(name: &str, port: &str, period_ns: f64, duty_pct: f64) -> Self {
        Self {
            name: name.into(),
            port: port.into(),
            period_ns,
            duty_pct,
            generated: false,
            master: String::new(),
            divide_by: 1,
            multiply_by: 1,
            invert: false,
        }
    }
}

/// One `set_input_delay` / `set_output_delay` row.
#[derive(Clone, Debug)]
pub struct WizardIoDelay {
    pub port: String,
    /// Reference clock name.
    pub clock: String,
    pub min_ns: Option<f64>,
    pub max_ns: Option<f64>,
}

/// One exception row: false path or max delay.
#[derive(Clone, Debug)]
pub enum WizardException {
    FalsePath {
        from: String,
        from_is_clock: bool,
        to: String,
        to_is_clock: bool,
    },
    MaxDelay {
        from: String,
        to: String,
        delay_ns: f64,
        datapath_only: bool,
    },
}

/// Everything the wizard collects.
#[derive(Clone, Debug, Default)]
pub struct WizardInput {
    pub clocks: Vec<WizardClock>,
    pub input_delays: Vec<WizardIoDelay>,
    pub output_delays: Vec<WizardIoDelay>,
    pub exceptions: Vec<WizardException>,
}

impl WizardInput {
    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty()
            && self.input_delays.is_empty()
            && self.output_delays.is_empty()
            && self.exceptions.is_empty()
    }

    /// All clock names this input defines, in order.
    pub fn clock_names(&self) -> Vec<String> {
        self.clocks.iter().map(|c| c.name.clone()).collect()
    }
}

/// One validation failure, tagged with the step that owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WizardError {
    pub step: WizardStep,
    pub message: String,
}

impl WizardError {
    fn at(step: WizardStep, message: impl Into<String>) -> Self {
        Self {
            step,
            message: message.into(),
        }
    }
}

fn port_named<'d>(design: &'d Design, name: &str) -> Option<&'d helion_ir::Port> {
    design.ports.iter().find(|p| p.name == name)
}

/// Validate the whole input against the loaded design and the clocks the
/// project already knows (existing constraint clocks).
///
/// `known_clocks` are clock names defined outside the wizard (already-loaded
/// SDC); wizard-defined names are added on top for reference checks.
pub fn validate(
    input: &WizardInput,
    design: &Design,
    known_clocks: &[String],
) -> Vec<WizardError> {
    let mut errs = Vec::new();
    let mut clock_names: Vec<&str> = known_clocks.iter().map(|s| s.as_str()).collect();

    for (i, c) in input.clocks.iter().enumerate() {
        let tag = format!("Clock #{}: ", i + 1);
        if c.name.trim().is_empty() {
            errs.push(WizardError::at(
                WizardStep::Clocks,
                format!("{tag}clock name is empty"),
            ));
        } else if clock_names.contains(&c.name.as_str()) {
            errs.push(WizardError::at(
                WizardStep::Clocks,
                format!("{tag}clock name '{}' is already defined", c.name),
            ));
        } else {
            clock_names.push(c.name.as_str());
        }
        if !c.generated && port_named(design, &c.port).is_none() {
            errs.push(WizardError::at(
                WizardStep::Clocks,
                format!("{tag}port '{}' is not a top-level port of {}", c.port, design.name),
            ));
        }
        if !(c.period_ns > 0.0) || !c.period_ns.is_finite() {
            errs.push(WizardError::at(
                WizardStep::Clocks,
                format!("{tag}period must be positive (got {})", c.period_ns),
            ));
        }
        if !(c.duty_pct > 0.0) || !(c.duty_pct < 100.0) || !c.duty_pct.is_finite() {
            errs.push(WizardError::at(
                WizardStep::Clocks,
                format!("{tag}duty cycle must be in (0, 100) (got {})", c.duty_pct),
            ));
        }
        if c.generated {
            if !clock_names.contains(&c.master.as_str()) {
                errs.push(WizardError::at(
                    WizardStep::Clocks,
                    format!("{tag}master clock '{}' is not defined", c.master),
                ));
            }
            if c.divide_by < 1 {
                errs.push(WizardError::at(
                    WizardStep::Clocks,
                    format!("{tag}divide_by must be >= 1"),
                ));
            }
            if c.multiply_by < 1 {
                errs.push(WizardError::at(
                    WizardStep::Clocks,
                    format!("{tag}multiply_by must be >= 1"),
                ));
            }
        }
    }

    let check_io = |list: &[WizardIoDelay],
                    want_input: bool,
                    step: WizardStep,
                    errs: &mut Vec<WizardError>| {
        for (i, d) in list.iter().enumerate() {
            let tag = format!("{} #{}: ", step.title(), i + 1);
            match port_named(design, &d.port) {
                None => errs.push(WizardError::at(
                    step,
                    format!("{tag}port '{}' is not a top-level port of {}", d.port, design.name),
                )),
                Some(p) => {
                    let ok = match (want_input, p.dir) {
                        (true, PortDir::In) | (true, PortDir::Inout) => true,
                        (false, PortDir::Out) | (false, PortDir::Inout) => true,
                        _ => false,
                    };
                    if !ok {
                        errs.push(WizardError::at(
                            step,
                            format!(
                                "{tag}port '{}' is {:?}, not an {}put port",
                                d.port,
                                p.dir,
                                if want_input { "in" } else { "out" }
                            ),
                        ));
                    }
                }
            }
            if !clock_names.contains(&d.clock.as_str()) {
                errs.push(WizardError::at(
                    step,
                    format!("{tag}reference clock '{}' is not defined", d.clock),
                ));
            }
            if d.min_ns.is_none() && d.max_ns.is_none() {
                errs.push(WizardError::at(
                    step,
                    format!("{tag}set at least one of min / max delay"),
                ));
            }
            for (label, v) in [("min", d.min_ns), ("max", d.max_ns)] {
                if let Some(v) = v {
                    if !v.is_finite() || v < 0.0 {
                        errs.push(WizardError::at(
                            step,
                            format!("{tag}{label} delay must be >= 0 (got {v})"),
                        ));
                    }
                }
            }
            if let (Some(lo), Some(hi)) = (d.min_ns, d.max_ns) {
                if lo > hi {
                    errs.push(WizardError::at(
                        step,
                        format!("{tag}min delay {lo} ns exceeds max delay {hi} ns"),
                    ));
                }
            }
        }
    };
    check_io(&input.input_delays, true, WizardStep::InputDelays, &mut errs);
    check_io(&input.output_delays, false, WizardStep::OutputDelays, &mut errs);

    for (i, e) in input.exceptions.iter().enumerate() {
        let tag = format!("Exception #{}: ", i + 1);
        match e {
            WizardException::FalsePath {
                from,
                from_is_clock,
                to,
                to_is_clock,
            } => {
                for (label, name, is_clock) in
                    [("from", from, from_is_clock), ("to", to, to_is_clock)]
                {
                    let ok = if *is_clock {
                        clock_names.contains(&name.as_str())
                    } else {
                        port_named(design, name).is_some()
                    };
                    if !ok {
                        errs.push(WizardError::at(
                            WizardStep::Exceptions,
                            format!(
                                "{tag}{label} '{}' is not a known {}",
                                name,
                                if *is_clock { "clock" } else { "port" }
                            ),
                        ));
                    }
                }
            }
            WizardException::MaxDelay {
                from,
                to,
                delay_ns,
                ..
            } => {
                for (label, name) in [("from", from), ("to", to)] {
                    if port_named(design, name).is_none() {
                        errs.push(WizardError::at(
                            WizardStep::Exceptions,
                            format!("{tag}{label} port '{name}' is not a top-level port"),
                        ));
                    }
                }
                if !delay_ns.is_finite() || *delay_ns <= 0.0 {
                    errs.push(WizardError::at(
                        WizardStep::Exceptions,
                        format!("{tag}delay must be positive (got {delay_ns})"),
                    ));
                }
            }
        }
    }

    errs
}

fn get_obj(name: &str, is_clock: bool) -> String {
    if is_clock {
        format!("[get_clocks {name}]")
    } else {
        format!("[get_ports {name}]")
    }
}

/// Render the wizard input as SDC text in the repo's XDC/SDC dialect.
///
/// The output is validated with [`validate_sdc`] (parse round-trip through
/// `helion_sta::load_xdc`) before it is ever written to disk.
pub fn generate_sdc(input: &WizardInput, design_name: &str) -> String {
    let mut out = String::new();
    out.push_str("# Helion Constraints Wizard - generated SDC\n");
    out.push_str(&format!("# Design: {design_name}\n"));
    out.push_str("# Edit freely; the Constraints tab re-parses this file on save.\n");

    if !input.clocks.is_empty() {
        out.push_str("\n# --- Clocks ---\n");
        for c in &input.clocks {
            if c.generated {
                out.push_str(&format!(
                    "create_generated_clock -name {} -source [get_ports {}] -divide_by {} -multiply_by {}{} [get_ports {}]\n",
                    c.name,
                    c.master,
                    c.divide_by,
                    c.multiply_by,
                    if c.invert { " -invert" } else { "" },
                    c.port,
                ));
            } else {
                // Waveform is recorded for Vivado export; Helion STA keys off period.
                let fall = c.period_ns * c.duty_pct / 100.0;
                out.push_str(&format!(
                    "create_clock -period {:.3} -name {} -waveform {{0.000 {:.3}}} [get_ports {}]\n",
                    c.period_ns, c.name, fall, c.port
                ));
            }
        }
    }

    let emit_io = |out: &mut String, list: &[WizardIoDelay], cmd: &str, title: &str| {
        if list.is_empty() {
            return;
        }
        out.push_str(&format!("\n# --- {title} ---\n"));
        for d in list {
            // Helion stores one delay value per port; emit -min first so the
            // -max (setup) line wins the parse, matching STA's setup focus.
            if let Some(v) = d.min_ns {
                out.push_str(&format!(
                    "{cmd} -clock {} -min {:.3} [get_ports {}]\n",
                    d.clock, v, d.port
                ));
            }
            if let Some(v) = d.max_ns {
                out.push_str(&format!(
                    "{cmd} -clock {} -max {:.3} [get_ports {}]\n",
                    d.clock, v, d.port
                ));
            }
        }
    };
    emit_io(&mut out, &input.input_delays, "set_input_delay", "Input delays");
    emit_io(&mut out, &input.output_delays, "set_output_delay", "Output delays");

    if !input.exceptions.is_empty() {
        out.push_str("\n# --- Exceptions ---\n");
        for e in &input.exceptions {
            match e {
                WizardException::FalsePath {
                    from,
                    from_is_clock,
                    to,
                    to_is_clock,
                } => {
                    out.push_str(&format!(
                        "set_false_path -from {} -to {}\n",
                        get_obj(from, *from_is_clock),
                        get_obj(to, *to_is_clock)
                    ));
                }
                WizardException::MaxDelay {
                    from,
                    to,
                    delay_ns,
                    datapath_only,
                } => {
                    out.push_str(&format!(
                        "set_max_delay {:.3} -from [get_ports {from}] -to [get_ports {to}]{}",
                        delay_ns,
                        if *datapath_only { " -datapath_only" } else { "" }
                    ));
                    out.push('\n');
                }
            }
        }
    }

    out
}

/// Prove generated text is loadable: parse it with the real SDC frontend.
pub fn validate_sdc(text: &str) -> Result<(), String> {
    helion_sta::load_xdc(text).map(|_| ())
}

/// Extract `WNS_PS=<n>` from a `report_timing` result string.
pub fn parse_wns(report: &str) -> Option<i64> {
    report.split_whitespace().find_map(|tok| {
        tok.strip_prefix("WNS_PS=")
            .and_then(|v| v.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '-').parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_design() -> Design {
        let mut d = Design::new("sample");
        for (name, dir) in [
            ("clk", PortDir::In),
            ("rst_n", PortDir::In),
            ("ena", PortDir::In),
            ("led", PortDir::Out),
        ] {
            d.ports.push(helion_ir::Port {
                name: name.into(),
                dir,
                attrs: Default::default(),
            });
        }
        d
    }

    fn sample_input() -> WizardInput {
        WizardInput {
            clocks: vec![WizardClock::primary("clk", "clk", 10.0, 50.0)],
            input_delays: vec![WizardIoDelay {
                port: "rst_n".into(),
                clock: "clk".into(),
                min_ns: Some(0.5),
                max_ns: Some(1.5),
            }],
            output_delays: vec![WizardIoDelay {
                port: "led".into(),
                clock: "clk".into(),
                min_ns: None,
                max_ns: Some(2.0),
            }],
            exceptions: vec![
                WizardException::FalsePath {
                    from: "rst_n".into(),
                    from_is_clock: false,
                    to: "clk".into(),
                    to_is_clock: true,
                },
                WizardException::MaxDelay {
                    from: "ena".into(),
                    to: "led".into(),
                    delay_ns: 8.0,
                    datapath_only: true,
                },
            ],
        }
    }

    #[test]
    fn golden_sdc_generation() {
        let text = generate_sdc(&sample_input(), "sample");
        let expected = "# Helion Constraints Wizard - generated SDC\n\
# Design: sample\n\
# Edit freely; the Constraints tab re-parses this file on save.\n\
\n\
# --- Clocks ---\n\
create_clock -period 10.000 -name clk -waveform {0.000 5.000} [get_ports clk]\n\
\n\
# --- Input delays ---\n\
set_input_delay -clock clk -min 0.500 [get_ports rst_n]\n\
set_input_delay -clock clk -max 1.500 [get_ports rst_n]\n\
\n\
# --- Output delays ---\n\
set_output_delay -clock clk -max 2.000 [get_ports led]\n\
\n\
# --- Exceptions ---\n\
set_false_path -from [get_ports rst_n] -to [get_clocks clk]\n\
set_max_delay 8.000 -from [get_ports ena] -to [get_ports led] -datapath_only\n";
        assert_eq!(text, expected, "generated SDC changed:\n{text}");
    }

    #[test]
    fn sample_input_validates_clean() {
        let d = sample_design();
        let errs = validate(&sample_input(), &d, &[]);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    #[test]
    fn generated_sdc_round_trips_through_parser() {
        let text = generate_sdc(&sample_input(), "sample");
        validate_sdc(&text).expect("generated SDC must parse");
        let c = helion_sta::load_xdc(&text).unwrap();
        assert_eq!(c.clocks.len(), 1);
        assert_eq!(c.clocks[0].name, "clk");
        assert_eq!(c.clocks[0].period_ps, 10_000);
        assert_eq!(c.input_delay_ps.get("rst_n"), Some(&1500));
        assert_eq!(c.output_delay_ps.get("led"), Some(&2000));
        assert_eq!(c.false_paths.len(), 1);
        assert_eq!(c.max_delays.len(), 1);
        assert!(c.max_delays[0].datapath_only);
    }

    #[test]
    fn validate_allows_reference_to_wizard_and_known_clocks() {
        let d = sample_design();
        // known_clocks holds only externally-defined clocks; the wizard's own
        // "clk" must not be flagged as a duplicate of itself.
        let errs = validate(&sample_input(), &d, &["sys_clk".to_string()]);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        // Referencing the external clock is fine too.
        let mut input = sample_input();
        input.input_delays[0].clock = "sys_clk".into();
        let errs = validate(&input, &d, &["sys_clk".to_string()]);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    #[test]
    fn validation_rejects_bad_input() {
        let d = sample_design();
        // Negative period.
        let mut bad = sample_input();
        bad.clocks[0].period_ns = -5.0;
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.step == WizardStep::Clocks && e.message.contains("period")),
            "negative period must be rejected: {errs:?}"
        );
        // Unknown port.
        let mut bad = sample_input();
        bad.clocks[0].port = "nope".into();
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("not a top-level port")),
            "unknown port must be rejected: {errs:?}"
        );
        // min > max.
        let mut bad = sample_input();
        bad.input_delays[0].min_ns = Some(3.0);
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("exceeds max")),
            "min>max must be rejected: {errs:?}"
        );
        // Bad duty.
        let mut bad = sample_input();
        bad.clocks[0].duty_pct = 0.0;
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("duty")),
            "duty 0 must be rejected: {errs:?}"
        );
        // Unknown reference clock.
        let mut bad = sample_input();
        bad.input_delays[0].clock = "ghost".into();
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("reference clock")),
            "unknown clock must be rejected: {errs:?}"
        );
        // Duplicate clock name vs existing.
        let errs = validate(&sample_input(), &d, &["clk".to_string()]);
        assert!(
            errs.iter().any(|e| e.message.contains("already defined")),
            "duplicate clock name must be rejected: {errs:?}"
        );
        // Output delay on an input port.
        let mut bad = sample_input();
        bad.output_delays[0].port = "rst_n".into();
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("not an output port")),
            "wrong-direction port must be rejected: {errs:?}"
        );
        // Non-positive max delay.
        let mut bad = sample_input();
        if let WizardException::MaxDelay { delay_ns, .. } = &mut bad.exceptions[1] {
            *delay_ns = 0.0;
        }
        let errs = validate(&bad, &d, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("positive")),
            "zero max_delay must be rejected: {errs:?}"
        );
    }

    #[test]
    fn generated_clock_emits_and_parses() {
        let input = WizardInput {
            clocks: vec![
                WizardClock::primary("clk", "clk", 10.0, 50.0),
                WizardClock {
                    name: "clk_div2".into(),
                    port: "ena".into(),
                    period_ns: 20.0,
                    duty_pct: 50.0,
                    generated: true,
                    master: "clk".into(),
                    divide_by: 2,
                    multiply_by: 1,
                    invert: false,
                },
            ],
            ..Default::default()
        };
        let d = sample_design();
        assert!(validate(&input, &d, &[]).is_empty());
        let text = generate_sdc(&input, "counter");
        assert!(
            text.contains("create_generated_clock -name clk_div2 -source [get_ports clk] -divide_by 2 -multiply_by 1 [get_ports ena]"),
            "unexpected generated clock line:\n{text}"
        );
        validate_sdc(&text).expect("generated clock SDC must parse");
        let c = helion_sta::load_xdc(&text).unwrap();
        assert_eq!(c.clocks.len(), 2);
        assert!(c.clocks[1].generated);
    }

    #[test]
    fn parse_wns_extracts_value() {
        let r = "report_timing counter WNS_PS=9640 TNS_PS=0 SETUP_PS=360 HOLD_PS=120";
        assert_eq!(parse_wns(r), Some(9640));
        let r2 = "report_timing counter WNS_PS=-120 TNS_PS=-360";
        assert_eq!(parse_wns(r2), Some(-120));
        assert_eq!(parse_wns("no timing here"), None);
    }
}
