//! Timing Constraints pane: the project XDC/SDC parsed into the UG893
//! Timing Constraints sections (Clocks, I/O Delay, Exceptions, I/O
//! Placement), each a `Name | Command | From | To | Value` table.
//!
//! Mirrors the old IDE's `IdeModel::constraint_rows` (`ide.rs:21609`): the
//! rows come from `helion_sta::load_xdc`, never a concatenated dump. The
//! source is the editor's live XDC buffer (the live edit field when the XDC
//! tab is the one being typed into), parsed one command line at a time so
//! every row knows its source line; generated clocks parse with the clocks
//! defined above them so their master resolves exactly as in a whole-file
//! parse. Lines `load_xdc` rejects are listed with the engine's message.
//!
//! Header actions do real work: "Open in Editor" switches to the XDC tab
//! (at the selected row's line), "Constraints Wizard…" opens the existing
//! wizard dialog. Row taps select the row; the pinned footer shows its
//! source line.

use rune::{El, ScrollSpec, col, el, row, text};

use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::report_kit::{self as kit, header_row, select_row, table};
use crate::views::sources::pane_header;

/// Command column width (fits `set_property IOSTANDARD`).
const CMD_W: f32 = 176.0;
/// Value column width.
const VALUE_W: f32 = 168.0;

/// UG893 Timing Constraints section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintSection {
    Clocks,
    IoDelay,
    Exceptions,
    IoPlacement,
}

impl ConstraintSection {
    pub const ALL: [ConstraintSection; 4] = [
        ConstraintSection::Clocks,
        ConstraintSection::IoDelay,
        ConstraintSection::Exceptions,
        ConstraintSection::IoPlacement,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ConstraintSection::Clocks => "Clocks",
            ConstraintSection::IoDelay => "I/O Delay",
            ConstraintSection::Exceptions => "Exceptions",
            ConstraintSection::IoPlacement => "I/O Placement",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ConstraintSection::Clocks => "constraints-clocks",
            ConstraintSection::IoDelay => "constraints-io-delay",
            ConstraintSection::Exceptions => "constraints-exceptions",
            ConstraintSection::IoPlacement => "constraints-io-placement",
        }
    }

    fn empty(self) -> &'static str {
        match self {
            ConstraintSection::Clocks => {
                "No clocks defined — STA falls back to an implicit 10 ns clock."
            }
            ConstraintSection::IoDelay => "No input or output delays.",
            ConstraintSection::Exceptions => "No timing exceptions.",
            ConstraintSection::IoPlacement => "No pin or I/O standard assignments.",
        }
    }
}

/// One parsed constraint, with the 1-based XDC line it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintRow {
    pub section: ConstraintSection,
    pub name: String,
    pub command: String,
    pub from: String,
    pub to: String,
    pub value: String,
    pub line: usize,
}

/// Parse result: rows in file order plus the lines the parser rejected.
#[derive(Clone, Debug, Default)]
pub struct ParsedXdc {
    pub rows: Vec<ConstraintRow>,
    /// `(1-based line, engine message)`.
    pub errors: Vec<(usize, String)>,
}

/// Constraints pane UI state: the selected row (index into
/// [`ParsedXdc::rows`]).
#[derive(Clone, Debug, Default)]
pub struct ConstraintsView {
    pub selected: Option<usize>,
}

fn ns(ps: i64) -> String {
    format!("{:.3} ns", ps as f64 / 1000.0)
}

/// `-from`/`-to` objects of a raw exception command (mirrors
/// `ide.rs::constraint_from_to`).
fn from_to(cmd: &str) -> (String, String) {
    fn obj(after: &str) -> String {
        after
            .split([' ', '\t', '[', ']', '{', '}'])
            .map(str::trim)
            .find(|t| {
                !t.is_empty()
                    && !t.starts_with('-')
                    && !matches!(
                        t.to_ascii_lowercase().as_str(),
                        "get_ports" | "get_pins" | "get_clocks" | "get_cells" | "get_nets"
                    )
            })
            .unwrap_or("")
            .trim_matches(|c: char| matches!(c, '"' | '\''))
            .to_string()
    }
    let from = cmd
        .split_once("-from")
        .map(|(_, r)| obj(r))
        .unwrap_or_default();
    let to = cmd
        .split_once("-to")
        .map(|(_, r)| obj(r))
        .unwrap_or_default();
    (from, to)
}

/// Rows a single parsed command contributes (mirrors the old
/// `constraint_rows` section mapping).
fn rows_from(
    c: &helion_sta::Constraints,
    line: usize,
    prefix_clocks: &[String],
) -> Vec<ConstraintRow> {
    use ConstraintSection as S;
    let mut out = Vec::new();
    let mut push =
        |section: S, command: &str, name: String, from: String, to: String, value: String| {
            out.push(ConstraintRow {
                section,
                name,
                command: command.into(),
                from,
                to,
                value,
                line,
            });
        };
    for k in c.clocks.iter().filter(|k| !prefix_clocks.contains(&k.name)) {
        if k.generated {
            let mut value = format!("{} (", ns(k.period_ps as i64));
            if k.divide_by > 1 {
                value.push_str(&format!("÷{}", k.divide_by));
            }
            if k.multiply_by > 1 {
                value.push_str(&format!("×{}", k.multiply_by));
            }
            if k.divide_by <= 1 && k.multiply_by <= 1 {
                value.push_str("×1");
            }
            if k.invert {
                value.push_str(", inverted");
            }
            value.push(')');
            push(
                S::Clocks,
                "create_generated_clock",
                k.name.clone(),
                k.master.clone().unwrap_or_default(),
                k.source.clone(),
                value,
            );
        } else {
            push(
                S::Clocks,
                "create_clock",
                k.name.clone(),
                String::new(),
                k.source.clone(),
                ns(k.period_ps as i64),
            );
        }
    }
    for (port, ps) in &c.input_delay_ps {
        push(
            S::IoDelay,
            "set_input_delay",
            port.clone(),
            String::new(),
            port.clone(),
            ns(*ps),
        );
    }
    for (port, ps) in &c.output_delay_ps {
        push(
            S::IoDelay,
            "set_output_delay",
            port.clone(),
            String::new(),
            port.clone(),
            ns(*ps),
        );
    }
    for fp in &c.false_paths {
        let (from, to) = from_to(fp);
        let name = match (from.is_empty(), to.is_empty()) {
            (true, true) => "all".into(),
            (false, true) => from.clone(),
            (true, false) => to.clone(),
            (false, false) => format!("{from} → {to}"),
        };
        push(
            S::Exceptions,
            "set_false_path",
            name,
            from,
            to,
            "false path".into(),
        );
    }
    for m in &c.multicycle_paths {
        push(
            S::Exceptions,
            "set_multicycle_path",
            format!("{} → {}", m.from, m.to),
            m.from.clone(),
            m.to.clone(),
            format!("setup ×{} hold ×{}", m.setup_mult, m.hold_mult),
        );
    }
    for m in &c.max_delays {
        push(
            S::Exceptions,
            "set_max_delay",
            format!("{} → {}", m.from, m.to),
            m.from.clone(),
            m.to.clone(),
            if m.datapath_only {
                format!("{} datapath only", ns(m.delay_ps))
            } else {
                ns(m.delay_ps)
            },
        );
    }
    for m in &c.min_delays {
        push(
            S::Exceptions,
            "set_min_delay",
            format!("{} → {}", m.from, m.to),
            m.from.clone(),
            m.to.clone(),
            ns(m.delay_ps),
        );
    }
    for g in &c.clock_groups {
        let kind = if g.asynchronous {
            "asynchronous"
        } else if g.exclusive {
            "exclusive"
        } else {
            "groups"
        };
        push(
            S::Exceptions,
            "set_clock_groups",
            kind.into(),
            g.groups.first().map(|x| x.join(", ")).unwrap_or_default(),
            g.groups.get(1).map(|x| x.join(", ")).unwrap_or_default(),
            format!("{} groups", g.groups.len()),
        );
    }
    for u in &c.clock_uncertainties {
        push(
            S::Exceptions,
            "set_clock_uncertainty",
            format!("{} → {}", u.from, u.to),
            u.from.clone(),
            u.to.clone(),
            format!("setup {} hold {}", ns(u.setup_ps), ns(u.hold_ps)),
        );
    }
    for l in &c.clock_latencies {
        push(
            S::Exceptions,
            "set_clock_latency",
            l.clock.clone(),
            l.clock.clone(),
            String::new(),
            format!(
                "late {} early {}{}",
                ns(l.late_ps),
                ns(l.early_ps),
                if l.source { " (source)" } else { "" }
            ),
        );
    }
    for d in &c.disable_timings {
        let name = if d.object.is_empty() {
            format!("{} → {}", d.from, d.to)
        } else {
            d.object.clone()
        };
        push(
            S::Exceptions,
            "set_disable_timing",
            name,
            d.from.clone(),
            d.to.clone(),
            "disabled".into(),
        );
    }
    for a in &c.case_analyses {
        push(
            S::Exceptions,
            "set_case_analysis",
            a.object.clone(),
            String::new(),
            a.object.clone(),
            a.value.clone(),
        );
    }
    for p in &c.propagated_clocks {
        push(
            S::Exceptions,
            "set_propagated_clock",
            p.clone(),
            p.clone(),
            String::new(),
            "propagated".into(),
        );
    }
    for k in &c.clock_senses {
        push(
            S::Exceptions,
            "set_clock_sense",
            k.object.clone(),
            String::new(),
            k.object.clone(),
            k.sense.clone(),
        );
    }
    for j in &c.input_jitters {
        push(
            S::Exceptions,
            "set_input_jitter",
            j.clock.clone(),
            j.clock.clone(),
            String::new(),
            ns(j.jitter_ps),
        );
    }
    if c.system_jitter_ps != 0 {
        push(
            S::Exceptions,
            "set_system_jitter",
            "system".into(),
            String::new(),
            String::new(),
            ns(c.system_jitter_ps),
        );
    }
    for d in &c.timing_derates {
        push(
            S::Exceptions,
            "set_timing_derate",
            "derate".into(),
            String::new(),
            String::new(),
            format!(
                "late {:.3} early {:.3}",
                d.late_milli as f64 / 1000.0,
                d.early_milli as f64 / 1000.0
            ),
        );
    }
    if c.operating_conditions.is_set() {
        let oc = &c.operating_conditions;
        push(
            S::Exceptions,
            "set_operating_conditions",
            "pvt".into(),
            String::new(),
            String::new(),
            format!("{} mV, {} °C", oc.voltage_mv, oc.temperature_c),
        );
    }
    for b in &c.bus_skews {
        push(
            S::Exceptions,
            "set_bus_skew",
            format!("{} → {}", b.from, b.to),
            b.from.clone(),
            b.to.clone(),
            ns(b.skew_ps),
        );
    }
    for g in &c.path_groups {
        push(
            S::Exceptions,
            "group_path",
            g.name.clone(),
            g.from.clone(),
            g.to.clone(),
            format!("weight {:.3}", g.weight_milli as f64 / 1000.0),
        );
    }
    for b in &c.max_time_borrows {
        push(
            S::Exceptions,
            "set_max_time_borrow",
            b.object.clone(),
            String::new(),
            b.object.clone(),
            ns(b.borrow_ps),
        );
    }
    for d in &c.data_checks {
        push(
            S::Exceptions,
            "set_data_check",
            format!("{} → {}", d.from, d.to),
            d.from.clone(),
            d.to.clone(),
            format!("setup {} hold {}", ns(d.setup_ps), ns(d.hold_ps)),
        );
    }
    let props: [(&str, &std::collections::BTreeMap<String, String>); 7] = [
        ("set_property PACKAGE_PIN", &c.package_pins),
        ("set_property IOSTANDARD", &c.iostandards),
        ("set_property DRIVE", &c.drives),
        ("set_property SLEW", &c.slews),
        ("set_property PULLTYPE", &c.pulltypes),
        ("set_property DIFF_TERM", &c.diff_terms),
        ("set_property IN_TERM", &c.in_terms),
    ];
    for (command, map) in props {
        for (port, v) in map {
            push(
                S::IoPlacement,
                command,
                port.clone(),
                String::new(),
                port.clone(),
                v.clone(),
            );
        }
    }
    out
}

/// Parse XDC text one command line at a time with `helion_sta::load_xdc`,
/// tagging every row with its 1-based source line.
pub fn parse_constraints(lines: &[String]) -> ParsedXdc {
    let mut out = ParsedXdc::default();
    let mut clock_lines: Vec<String> = Vec::new();
    let mut clock_names: Vec<String> = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let code = raw.split('#').next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let first = code.split_whitespace().next().unwrap_or("");
        let is_clock = matches!(first, "create_clock" | "create_generated_clock");
        let text = if first == "create_generated_clock" && !clock_lines.is_empty() {
            format!("{}\n{code}", clock_lines.join("\n"))
        } else {
            code.to_string()
        };
        let prefix: &[String] = if first == "create_generated_clock" {
            &clock_names
        } else {
            &[]
        };
        match helion_sta::load_xdc(&text) {
            Ok(c) => {
                let rows = rows_from(&c, i + 1, prefix);
                if is_clock {
                    clock_lines.push(code.to_string());
                    for r in &rows {
                        if !clock_names.contains(&r.name) {
                            clock_names.push(r.name.clone());
                        }
                    }
                }
                out.rows.extend(rows);
            }
            Err(e) => out.errors.push((i + 1, e)),
        }
    }
    out
}

/// The XDC buffer as the user currently sees it: the live edit field when
/// it mirrors an `.xdc` tab, else the stored XDC lines.
pub fn xdc_lines(s: &AppState) -> Vec<String> {
    let live = s
        .editor
        .edit_tab
        .and_then(|i| s.editor.tabs.get(i))
        .is_some_and(|t| t.ends_with(".xdc"));
    if live {
        s.editor.edit.text.lines().map(str::to_string).collect()
    } else {
        s.editor.xdc_lines.clone()
    }
}

/// Name of the project's XDC tab, if it has one.
fn xdc_tab(s: &AppState) -> Option<(usize, &str)> {
    s.editor
        .tabs
        .iter()
        .enumerate()
        .find(|(_, t)| t.ends_with(".xdc"))
        .map(|(i, t)| (i, t.as_str()))
}

/// Switch the editor to the XDC tab, parking the caret on `line` (1-based)
/// when given.
pub fn open_in_editor(s: &mut AppState, line: Option<usize>) {
    let Some((i, _)) = xdc_tab(s) else {
        return;
    };
    crate::views::editor::flush_edit(s);
    crate::views::editor::select_tab(s, i);
    s.navigate(CenterPane::Editor);
    if let Some(l) = line {
        crate::views::editor::move_cursor_to(s, l.saturating_sub(1));
    }
}

/// Text dump for the `report_constraints` verb.
pub fn constraints_text(parsed: &ParsedXdc) -> String {
    let mut lines = vec![format!(
        "report_constraints rows={} errors={}",
        parsed.rows.len(),
        parsed.errors.len()
    )];
    for section in ConstraintSection::ALL {
        let rows: Vec<&ConstraintRow> = parsed
            .rows
            .iter()
            .filter(|r| r.section == section)
            .collect();
        lines.push(format!("{} ({})", section.title(), rows.len()));
        for r in rows {
            lines.push(format!(
                "  line {}: {} {} from={} to={} value={}",
                r.line,
                r.command,
                r.name,
                if r.from.is_empty() { "-" } else { &r.from },
                if r.to.is_empty() { "-" } else { &r.to },
                r.value
            ));
        }
    }
    for (line, e) in &parsed.errors {
        lines.push(format!("ERROR line {line}: {e}"));
    }
    lines.join("\n")
}

fn section_table(
    s: &AppState,
    parsed: &ParsedXdc,
    section: ConstraintSection,
    t: &VivadoTheme,
) -> El {
    let rows: Vec<(usize, &ConstraintRow)> = parsed
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.section == section)
        .collect();
    let title = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(kit::section_title(section.title(), t))
        .child(
            text(rows.len().to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .key(format!("{}-count", section.key())),
        );
    let mut block = col().key(section.key()).w_full().gap(2.0).child(title);
    if rows.is_empty() {
        return block.child(kit::empty_state(
            format!("{}-empty", section.key()),
            section.empty(),
            None,
            t,
        ));
    }
    let head = header_row(t)
        .child(kit::header_cell("Name", t).grow(1.0).min_w(0.0))
        .child(el().w(CMD_W).child(kit::header_cell("Command", t)))
        .child(kit::header_cell("From", t).grow(1.0).min_w(0.0))
        .child(kit::header_cell("To", t).grow(1.0).min_w(0.0))
        .child(el().w(VALUE_W).child(kit::header_cell("Value", t)));
    let mut body = col().w_full();
    for (idx, r) in rows {
        let dash = |v: &str| {
            if v.is_empty() {
                "–".to_string()
            } else {
                v.to_string()
            }
        };
        body = body.child(
            select_row(
                format!("constraints-row-{idx}"),
                format!("{} {} (line {})", r.command, r.name, r.line),
                s.constraints_view.selected == Some(idx),
                t,
                move |s: &mut AppState| s.constraints_view.selected = Some(idx),
            )
            .child(kit::text_cell(r.name.clone(), 1.0, t.text))
            .child(kit::fixed_cell(r.command.clone(), CMD_W, t.text_dim))
            .child(kit::text_cell(dash(&r.from), 1.0, t.text))
            .child(kit::text_cell(dash(&r.to), 1.0, t.text))
            .child(
                el().w(VALUE_W).child(
                    text(r.value.clone())
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text)
                        .truncate()
                        .help_when_truncated(r.value.clone()),
                ),
            ),
        );
    }
    block = block.child(table(head, body, t));
    block
}

/// Pinned footer: the selected row's source line (or a hint).
fn source_footer(s: &AppState, parsed: &ParsedXdc, lines: &[String], t: &VivadoTheme) -> El {
    let bar = row()
        .key("constraints-source")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.header_bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border_light);
    match s.constraints_view.selected.and_then(|i| parsed.rows.get(i)) {
        Some(r) => {
            let src = lines
                .get(r.line.saturating_sub(1))
                .cloned()
                .unwrap_or_default();
            let line = r.line;
            bar.child(
                text(format!("Line {line}"))
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .key("constraints-source-line"),
            )
            .child(
                text(src.trim().to_string())
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text)
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate()
                    .help_when_truncated(src.trim().to_string())
                    .key("constraints-source-text"),
            )
            .child(kit::link(
                "constraints-goto-line",
                "Go to line",
                t,
                move |s: &mut AppState| open_in_editor(s, Some(line)),
            ))
        }
        None => bar.child(
            text("Select a constraint to see its source line.")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_faint)
                .key("constraints-source-hint"),
        ),
    }
}

/// The center Constraints pane wired into the shell.
pub fn constraints_pane(s: &AppState, t: &VivadoTheme) -> El {
    let lines = xdc_lines(s);
    let parsed = parse_constraints(&lines);
    let file = xdc_tab(s).map(|(_, n)| n.to_string());
    let editor_reason = file.is_none().then_some("The project has no XDC file");
    let header = row()
        .key("constraints-header")
        .w_full()
        .h(30.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border_light)
        .child(
            text(match &file {
                Some(f) => format!("{f} · {} constraints", parsed.rows.len()),
                None => "No constraints file".to_string(),
            })
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim)
            .truncate()
            .key("constraints-file"),
        )
        .child(el().grow(1.0))
        .child(kit::action_button(
            "constraints-open-editor",
            "Open in Editor",
            editor_reason,
            t,
            |s: &mut AppState| {
                let parsed = parse_constraints(&xdc_lines(s));
                let line = s
                    .constraints_view
                    .selected
                    .and_then(|i| parsed.rows.get(i))
                    .map(|r| r.line);
                open_in_editor(s, line);
            },
        ))
        .child(kit::action_button(
            "constraints-wizard",
            "Constraints Wizard…",
            None,
            t,
            |s: &mut AppState| s.dialogs.open(crate::views::dialogs::DialogId::Constraints),
        ));
    let mut content = col().w_full().gap(12.0).padding(8.0);
    if !parsed.errors.is_empty() {
        let mut errs = col().key("constraints-errors").w_full().gap(0.0);
        for (line, e) in &parsed.errors {
            let msg = format!("Line {line}: {e}");
            errs = errs.child(
                text(msg.clone())
                    .font(VivadoTheme::FONT_UI)
                    .color(t.warn)
                    .truncate()
                    .help_when_truncated(msg),
            );
        }
        content = content.child(errs);
    }
    for section in ConstraintSection::ALL {
        content = content.child(section_table(s, &parsed, section, t));
    }
    col()
        .key("constraints-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Timing Constraints", "constraints", t))
        .child(header)
        .child(
            col()
                .key("constraints-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(content),
        )
        .child(source_footer(s, &parsed, &lines, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn rt(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let mut rt = Runtime::new(
            state,
            crate::view,
            spec.size,
            Env {
                system_dark: false,
                scale: 2.0,
            },
        );
        rt.frame(0.0);
        rt
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rows_carry_sections_and_source_lines() {
        let parsed = parse_constraints(&lines(&[
            "# clocks",
            "create_clock -period 10.000 -name clk [get_ports clk]",
            "create_generated_clock -name div2 -divide_by 2 [get_pins u/Q]",
            "set_input_delay -clock clk 2.0 [get_ports din]",
            "set_false_path -from [get_ports rst] -to [get_cells r_reg]",
            "set_property PACKAGE_PIN E3 [get_ports clk]",
            "set_property IOSTANDARD LVCMOS33 [get_ports clk]",
        ]));
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let summary: Vec<(ConstraintSection, &str, usize)> = parsed
            .rows
            .iter()
            .map(|r| (r.section, r.command.as_str(), r.line))
            .collect();
        assert_eq!(
            summary,
            vec![
                (ConstraintSection::Clocks, "create_clock", 2),
                (ConstraintSection::Clocks, "create_generated_clock", 3),
                (ConstraintSection::IoDelay, "set_input_delay", 4),
                (ConstraintSection::Exceptions, "set_false_path", 5),
                (
                    ConstraintSection::IoPlacement,
                    "set_property PACKAGE_PIN",
                    6
                ),
                (ConstraintSection::IoPlacement, "set_property IOSTANDARD", 7),
            ]
        );
        let generated = &parsed.rows[1];
        assert_eq!(
            generated.from, "clk",
            "generated clock master resolves from the clocks above"
        );
        assert_eq!(parsed.rows[0].value, "10.000 ns");
        assert_eq!(parsed.rows[3].from, "rst");
        assert_eq!(parsed.rows[3].to, "r_reg");
    }

    #[test]
    fn rejected_lines_are_reported_not_dropped() {
        let parsed = parse_constraints(&lines(&["create_clock -name clk [get_ports clk]"]));
        assert!(parsed.rows.is_empty());
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].0, 1);
    }

    #[test]
    fn pane_lists_demo_xdc_and_row_tap_shows_its_source_line() {
        let mut s = init();
        s.center = CenterPane::Constraints;
        let mut rt = rt(s);
        for key in [
            "constraints-pane",
            "constraints-header",
            "constraints-open-editor",
            "constraints-wizard",
            "constraints-clocks",
            "constraints-io-delay",
            "constraints-exceptions",
            "constraints-io-placement",
            "constraints-row-0",
            "constraints-source-hint",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        tap(&mut rt, "constraints-row-0");
        assert_eq!(rt.state.constraints_view.selected, Some(0));
        assert!(rt.find("constraints-source-line").is_some());
        tap(&mut rt, "constraints-goto-line");
        assert_eq!(
            rt.state.center,
            CenterPane::Editor,
            "go to line opens the editor"
        );
        assert!(
            rt.state
                .editor
                .tabs
                .get(rt.state.editor.active)
                .is_some_and(|t| t.ends_with(".xdc")),
            "the XDC tab is active"
        );
        assert_eq!(
            rt.state.editor.cursor_line, 1,
            "caret on the create_clock line"
        );
    }

    #[test]
    fn wizard_action_opens_the_constraints_dialog() {
        let mut s = init();
        s.center = CenterPane::Constraints;
        let mut rt = rt(s);
        tap(&mut rt, "constraints-wizard");
        assert_eq!(
            rt.state.dialogs.open,
            Some(crate::views::dialogs::DialogId::Constraints)
        );
    }

    #[test]
    fn missing_xdc_disables_open_in_editor() {
        let mut s = init();
        s.center = CenterPane::Constraints;
        s.editor.tabs.retain(|t| !t.ends_with(".xdc"));
        s.editor.active = 0;
        s.editor.edit_tab = None;
        let mut rt = rt(s);
        tap(&mut rt, "constraints-open-editor");
        assert_eq!(
            rt.state.center,
            CenterPane::Constraints,
            "disabled action must not navigate"
        );
    }
}
