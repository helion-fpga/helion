//! Constraints Wizard model checks: the design/XDC context, live per-row
//! validation (real ports, defined clocks, number ranges) with an engine
//! round trip of every row through [`helion_sta::load_xdc`], and the SDC
//! text generator (old `constraints_wizard::{validate, generate_sdc}`).

use helion_ir::PortDir;
use rune::TextEdit;

use super::{ClockRow, DelayRow, FalsePathRow, GenClockRow, List, MaxDelayRow, XdcWizard};
use crate::state::AppState;

// ---- design + project context ----

/// A clock the project's constraints already define.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownClock {
    pub name: String,
    pub source: String,
    pub period_ps: u64,
}

/// What validation checks against: the design's top-level ports (`None`
/// without a synthesized design) and the clocks the XDC already defines.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub ports: Option<Vec<(String, PortDir)>>,
    pub design: String,
    pub known: Vec<KnownClock>,
    /// Clock commands of the existing XDC (parse prelude for row checks).
    pub known_lines: Vec<String>,
}

impl Ctx {
    pub fn from_state(s: &AppState) -> Self {
        let design = crate::live::live_design(s);
        let ports = design.map(|d| {
            d.ports
                .iter()
                .map(|p| (helion_ir::resolve(p.name).to_string(), p.dir))
                .collect::<Vec<_>>()
        });
        let name = design
            .map(|d| helion_ir::resolve(d.name).to_string())
            .or_else(|| s.project.top.clone())
            .unwrap_or_else(|| s.project.name.clone());
        let (known, known_lines) = known_clocks(&crate::views::constraints::xdc_lines(s));
        Ctx {
            ports,
            design: name,
            known,
            known_lines,
        }
    }

    pub fn port(&self, name: &str) -> Option<PortDir> {
        self.ports
            .as_ref()?
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| *d)
    }

    fn ports_where(&self, f: impl Fn(PortDir) -> bool) -> Vec<String> {
        self.ports
            .iter()
            .flatten()
            .filter(|(_, d)| f(*d))
            .map(|(n, _)| n.clone())
            .collect()
    }

    pub fn inputs(&self) -> Vec<String> {
        self.ports_where(|d| matches!(d, PortDir::In | PortDir::Inout))
    }

    pub fn outputs(&self) -> Vec<String> {
        self.ports_where(|d| matches!(d, PortDir::Out | PortDir::Inout))
    }

    pub fn all_ports(&self) -> Vec<String> {
        self.ports_where(|_| true)
    }
}

/// Clocks of the existing XDC, loaded line by line through the real parser
/// so one bad line never hides the others.
fn known_clocks(lines: &[String]) -> (Vec<KnownClock>, Vec<String>) {
    let mut kept: Vec<String> = Vec::new();
    for raw in lines {
        let code = raw.split('#').next().unwrap_or("").trim();
        let first = code.split_whitespace().next().unwrap_or("");
        if !matches!(first, "create_clock" | "create_generated_clock") {
            continue;
        }
        let mut trial = kept.clone();
        trial.push(code.to_string());
        if helion_sta::load_xdc(&trial.join("\n")).is_ok() {
            kept = trial;
        }
    }
    let clocks = helion_sta::load_xdc(&kept.join("\n"))
        .map(|c| c.clocks)
        .unwrap_or_default();
    let known = clocks
        .into_iter()
        .map(|c| KnownClock {
            name: c.name,
            source: c.source,
            period_ps: c.period_ps,
        })
        .collect();
    (known, kept)
}

// ---- validation + SDC ----

/// One problem, owned by a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub list: List,
    pub id: u32,
    pub msg: String,
}

fn num(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn int(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok()
}

/// Tcl object reference: names with brackets/spaces are braced.
fn obj(cmd: &str, name: &str) -> String {
    let name = name.trim();
    if name.contains(['[', ']', ' ']) {
        format!("[{cmd} {{{name}}}]")
    } else {
        format!("[{cmd} {name}]")
    }
}

fn clock_line(r: &ClockRow) -> String {
    let period = num(&r.period_ns.text).unwrap_or(0.0);
    let duty = num(&r.duty_pct.text).unwrap_or(50.0);
    format!(
        "create_clock -period {:.3} -name {} -waveform {{0.000 {:.3}}} {}",
        period,
        r.name.text.trim(),
        period * duty / 100.0,
        obj("get_ports", &r.port.text)
    )
}

/// Source pin/port of clock `name` (for `-source`).
fn clock_source(w: &XdcWizard, ctx: &Ctx, name: &str) -> Option<String> {
    let name = name.trim();
    w.clocks
        .iter()
        .find(|c| c.name.text.trim() == name)
        .map(|c| c.port.text.trim().to_string())
        .or_else(|| {
            w.generated
                .iter()
                .find(|g| g.name.text.trim() == name)
                .map(|g| g.target.text.trim().to_string())
        })
        .or_else(|| {
            ctx.known
                .iter()
                .find(|k| k.name == name)
                .map(|k| k.source.clone())
        })
        .filter(|s| !s.is_empty())
}

fn gen_line(w: &XdcWizard, ctx: &Ctx, g: &GenClockRow) -> String {
    let master = g.master.text.trim();
    let source = clock_source(w, ctx, master).unwrap_or_else(|| master.to_string());
    let target = g.target.text.trim();
    let target_obj = if target.contains('/') {
        obj("get_pins", target)
    } else {
        obj("get_ports", target)
    };
    format!(
        "create_generated_clock -name {} -master_clock {} -source {} -divide_by {} -multiply_by {}{} {}",
        g.name.text.trim(),
        master,
        obj("get_ports", &source),
        int(&g.divide_by.text).unwrap_or(1),
        int(&g.multiply_by.text).unwrap_or(1),
        if g.invert { " -invert" } else { "" },
        target_obj
    )
}

fn delay_lines(r: &DelayRow, cmd: &str) -> Vec<String> {
    // Helion keeps one delay per port: -min first so -max (setup) wins the
    // parse, matching the old generator.
    let mut out = Vec::new();
    for (flag, raw) in [("-min", &r.min_ns), ("-max", &r.max_ns)] {
        if let Some(v) = num(&raw.text) {
            out.push(format!(
                "{cmd} -clock {} {flag} {:.3} {}",
                obj("get_clocks", &r.clock.text),
                v,
                obj("get_ports", &r.port.text)
            ));
        }
    }
    out
}

fn fp_line(r: &FalsePathRow) -> String {
    let end = |clock: bool, name: &TextEdit| {
        obj(if clock { "get_clocks" } else { "get_ports" }, &name.text)
    };
    format!(
        "set_false_path -from {} -to {}",
        end(r.from_clock, &r.from),
        end(r.to_clock, &r.to)
    )
}

fn md_line(r: &MaxDelayRow) -> String {
    format!(
        "set_max_delay {:.3} -from {} -to {}{}",
        num(&r.delay_ns.text).unwrap_or(0.0),
        obj("get_ports", &r.from.text),
        obj("get_ports", &r.to.text),
        if r.datapath_only {
            " -datapath_only"
        } else {
            ""
        }
    )
}

/// Every clock name a reference may use: existing XDC, then wizard rows.
pub(super) fn clock_names(w: &XdcWizard, ctx: &Ctx) -> Vec<String> {
    let mut out: Vec<String> = ctx.known.iter().map(|k| k.name.clone()).collect();
    for n in w
        .clocks
        .iter()
        .map(|c| &c.name)
        .chain(w.generated.iter().map(|g| &g.name))
    {
        let n = n.text.trim();
        if !n.is_empty() && !out.iter().any(|o| o == n) {
            out.push(n.to_string());
        }
    }
    out
}

/// Live validation of every row against the design and the engine parser.
pub fn issues(w: &XdcWizard, ctx: &Ctx) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut push = |list: List, id: u32, msg: String| out.push(Issue { list, id, msg });
    let design = &ctx.design;
    let mut defined: Vec<String> = ctx.known.iter().map(|k| k.name.clone()).collect();
    let mut prelude = ctx.known_lines.clone();
    let parses = |prelude: &[String], lines: &[String]| -> Result<(), String> {
        let mut text = prelude.join("\n");
        for l in lines {
            text.push('\n');
            text.push_str(l);
        }
        helion_sta::load_xdc(&text).map(|_| ())
    };

    // Clocks: names unique, a real input port, sane period and duty.
    for c in &w.clocks {
        let name = c.name.text.trim();
        let port = c.port.text.trim();
        let mut bad = false;
        if name.is_empty() {
            push(List::Clock, c.id, "Clock name is empty.".into());
            bad = true;
        } else if defined.iter().any(|d| d == name) {
            push(
                List::Clock,
                c.id,
                format!("Clock '{name}' is already defined."),
            );
            bad = true;
        }
        if port.is_empty() {
            push(List::Clock, c.id, "Source port is empty.".into());
            bad = true;
        } else if ctx.ports.is_some() {
            match ctx.port(port) {
                None => {
                    push(
                        List::Clock,
                        c.id,
                        format!("'{port}' is not a top-level port of {design}."),
                    );
                    bad = true;
                }
                Some(PortDir::Out) => {
                    push(
                        List::Clock,
                        c.id,
                        format!("'{port}' is an output; a clock enters on an input."),
                    );
                    bad = true;
                }
                Some(_) => {}
            }
        }
        match num(&c.period_ns.text) {
            Some(p) if p > 0.0 => {}
            _ => {
                push(
                    List::Clock,
                    c.id,
                    format!(
                        "Period '{}' must be a positive number of ns.",
                        c.period_ns.text.trim()
                    ),
                );
                bad = true;
            }
        }
        match num(&c.duty_pct.text) {
            Some(d) if d > 0.0 && d < 100.0 => {}
            _ => {
                push(
                    List::Clock,
                    c.id,
                    "Duty cycle must be between 0 and 100 %.".into(),
                );
                bad = true;
            }
        }
        if !name.is_empty() {
            defined.push(name.to_string());
        }
        if !bad {
            let line = clock_line(c);
            match parses(&prelude, std::slice::from_ref(&line)) {
                Ok(()) => prelude.push(line),
                Err(e) => push(List::Clock, c.id, format!("SDC check: {e}")),
            }
        }
    }

    // Generated clocks: a defined master, integer ratios, a target pin.
    for g in &w.generated {
        let name = g.name.text.trim();
        let master = g.master.text.trim();
        let mut bad = false;
        if name.is_empty() {
            push(List::Gen, g.id, "Clock name is empty.".into());
            bad = true;
        } else if defined.iter().any(|d| d == name) {
            push(
                List::Gen,
                g.id,
                format!("Clock '{name}' is already defined."),
            );
            bad = true;
        }
        if master.is_empty() || !defined.iter().any(|d| d == master) || master == name {
            push(
                List::Gen,
                g.id,
                format!("Master clock '{master}' is not defined above."),
            );
            bad = true;
        }
        if g.target.text.trim().is_empty() {
            push(List::Gen, g.id, "Target pin or port is empty.".into());
            bad = true;
        }
        for (what, raw) in [("÷", &g.divide_by), ("×", &g.multiply_by)] {
            if !int(&raw.text).is_some_and(|v| v >= 1) {
                push(
                    List::Gen,
                    g.id,
                    format!("{what} must be a whole number ≥ 1."),
                );
                bad = true;
            }
        }
        if !name.is_empty() {
            defined.push(name.to_string());
        }
        if !bad {
            let line = gen_line(w, ctx, g);
            match parses(&prelude, std::slice::from_ref(&line)) {
                Ok(()) => prelude.push(line),
                Err(e) => push(List::Gen, g.id, format!("SDC check: {e}")),
            }
        }
    }

    // I/O delays: a real port of the right direction, a defined clock, at
    // least one non-negative value, min ≤ max.
    for (list, rows, want_in) in [(List::In, &w.inputs, true), (List::Out, &w.outputs, false)] {
        let cmd = if want_in {
            "set_input_delay"
        } else {
            "set_output_delay"
        };
        for d in rows {
            let port = d.port.text.trim();
            let clock = d.clock.text.trim();
            let mut bad = false;
            if port.is_empty() {
                push(list, d.id, "Port is empty.".into());
                bad = true;
            } else if ctx.ports.is_some() {
                match ctx.port(port) {
                    None => {
                        push(
                            list,
                            d.id,
                            format!("'{port}' is not a top-level port of {design}."),
                        );
                        bad = true;
                    }
                    Some(dir) => {
                        let ok = matches!(
                            (want_in, dir),
                            (true, PortDir::In | PortDir::Inout)
                                | (false, PortDir::Out | PortDir::Inout)
                        );
                        if !ok {
                            let want = if want_in { "an input" } else { "an output" };
                            push(list, d.id, format!("'{port}' is not {want} port."));
                            bad = true;
                        }
                    }
                }
            }
            if !defined.iter().any(|c| c == clock) {
                push(
                    list,
                    d.id,
                    format!("Reference clock '{clock}' is not defined."),
                );
                bad = true;
            }
            let min = d.min_ns.text.trim();
            let max = d.max_ns.text.trim();
            if min.is_empty() && max.is_empty() {
                push(list, d.id, "Set a -min or -max delay.".into());
                bad = true;
            }
            for (flag, raw) in [("-min", min), ("-max", max)] {
                if !raw.is_empty() && !num(raw).is_some_and(|v| v >= 0.0) {
                    push(
                        list,
                        d.id,
                        format!("{flag} '{raw}' must be a number of ns ≥ 0."),
                    );
                    bad = true;
                }
            }
            if let (Some(lo), Some(hi)) = (num(min), num(max))
                && lo > hi
            {
                push(list, d.id, format!("-min {lo} ns exceeds -max {hi} ns."));
                bad = true;
            }
            if !bad && let Err(e) = parses(&prelude, &delay_lines(d, cmd)) {
                push(list, d.id, format!("SDC check: {e}"));
            }
        }
    }

    // False paths: each endpoint a real port or a defined clock.
    for f in &w.false_paths {
        let mut bad = false;
        for (label, name, is_clock) in [("From", &f.from, f.from_clock), ("To", &f.to, f.to_clock)]
        {
            let name = name.text.trim();
            let ok = if name.is_empty() {
                false
            } else if is_clock {
                defined.iter().any(|d| d == name)
            } else {
                ctx.ports.is_none() || ctx.port(name).is_some()
            };
            if !ok {
                let kind = if is_clock { "clock" } else { "port" };
                push(
                    List::Fp,
                    f.id,
                    format!("{label} '{name}' is not a known {kind}."),
                );
                bad = true;
            }
        }
        if !bad && let Err(e) = parses(&prelude, &[fp_line(f)]) {
            push(List::Fp, f.id, format!("SDC check: {e}"));
        }
    }

    // Max delays: real ports, a positive delay.
    for m in &w.max_delays {
        let mut bad = false;
        for (label, name) in [("From", &m.from), ("To", &m.to)] {
            let name = name.text.trim();
            if name.is_empty() || (ctx.ports.is_some() && ctx.port(name).is_none()) {
                push(
                    List::Md,
                    m.id,
                    format!("{label} '{name}' is not a top-level port."),
                );
                bad = true;
            }
        }
        if !num(&m.delay_ns.text).is_some_and(|v| v > 0.0) {
            push(
                List::Md,
                m.id,
                "Delay must be a positive number of ns.".into(),
            );
            bad = true;
        }
        if !bad && let Err(e) = parses(&prelude, &[md_line(m)]) {
            push(List::Md, m.id, format!("SDC check: {e}"));
        }
    }
    out
}

/// The generated SDC text (old `generate_sdc` layout). Callers check
/// [`issues`] first; unparseable numbers never panic here.
pub fn sdc_text(w: &XdcWizard, ctx: &Ctx) -> String {
    let mut out = String::new();
    out.push_str("# Helion Constraints Wizard - generated SDC\n");
    out.push_str(&format!("# Design: {}\n", ctx.design));
    if !w.clocks.is_empty() || !w.generated.is_empty() {
        out.push_str("\n# --- Clocks ---\n");
        for c in &w.clocks {
            out.push_str(&clock_line(c));
            out.push('\n');
        }
        for g in &w.generated {
            out.push_str(&gen_line(w, ctx, g));
            out.push('\n');
        }
    }
    for (rows, cmd, title) in [
        (&w.inputs, "set_input_delay", "Input delays"),
        (&w.outputs, "set_output_delay", "Output delays"),
    ] {
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("\n# --- {title} ---\n"));
        for d in rows {
            for l in delay_lines(d, cmd) {
                out.push_str(&l);
                out.push('\n');
            }
        }
    }
    if !w.false_paths.is_empty() || !w.max_delays.is_empty() {
        out.push_str("\n# --- Exceptions ---\n");
        for f in &w.false_paths {
            out.push_str(&fp_line(f));
            out.push('\n');
        }
        for m in &w.max_delays {
            out.push_str(&md_line(m));
            out.push('\n');
        }
    }
    out
}

/// Whole-file round trip: the existing clocks plus the generated text must
/// load through the engine parser.
pub fn validate_sdc(ctx: &Ctx, sdc: &str) -> Result<(), String> {
    let mut text = ctx.known_lines.join("\n");
    text.push('\n');
    text.push_str(sdc);
    helion_sta::load_xdc(&text).map(|_| ())
}
