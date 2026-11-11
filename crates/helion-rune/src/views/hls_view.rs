//! High-Level Synthesis pane: kernel source editor, directives table with
//! add/remove, clock setting, Run C Synthesis / Run C/RTL Co-Simulation /
//! Add RTL to Project, and the result tabs (synthesis report, schedule,
//! generated SystemVerilog, co-simulation vectors with pass/fail).

use rune::{El, FileDialogOptions, ScrollSpec, Tx, col, el, row, text};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::hls::{self as h, HlsTab};
use crate::views::sources::pane_header;

const LEFT_W: f32 = 0.46;

const DIR_COLS: [ColSpec; 5] = [
    ColSpec::num("Line", 40.0),
    ColSpec::fixed("Directive", 112.0),
    ColSpec::grow("Options", 1.2),
    ColSpec::grow("Scope", 1.0),
    ColSpec::fixed("", 26.0),
];
const LOOP_COLS: [ColSpec; 6] = [
    ColSpec::grow("Loop", 1.2),
    ColSpec::num("Trip", 48.0),
    ColSpec::fixed("Pipelined", 70.0),
    ColSpec::num("II", 40.0),
    ColSpec::num("Depth", 48.0),
    ColSpec::num("Latency", 60.0),
];
const IF_COLS: [ColSpec; 4] = [
    ColSpec::grow("Argument", 0.8),
    ColSpec::fixed("Protocol", 92.0),
    ColSpec::num("Bits", 44.0),
    ColSpec::grow("RTL ports", 1.6),
];
const SCHED_COLS: [ColSpec; 6] = [
    ColSpec::num("#", 36.0),
    ColSpec::grow("Operation", 1.4),
    ColSpec::num("Bits", 44.0),
    ColSpec::num("Cycle", 50.0),
    ColSpec::num("Stage", 50.0),
    ColSpec::num("Delay ns", 66.0),
];
const COSIM_COLS: [ColSpec; 5] = [
    ColSpec::num("#", 32.0),
    ColSpec::grow("C output", 1.0),
    ColSpec::grow("RTL output", 1.0),
    ColSpec::num("Latency", 60.0),
    ColSpec::fixed("Result", 64.0),
];

fn editor(key: &'static str, edit: &rune::TextEdit, placeholder: &'static str, t: &VivadoTheme, on: impl for<'a> Fn(&'a mut AppState) -> &'a mut rune::TextEdit + 'static) -> El {
    rune::text_input(edit, cad::mono_style())
        .key(key)
        .label(placeholder)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(6.0)
        .fill(t.chrome_bg)
        .color(t.text)
        .placeholder(placeholder, t.text_faint)
        .caret(t.text, t.selection)
        .on_edit(on)
}

fn field(key: &'static str, edit: &rune::TextEdit, placeholder: &'static str, w: f32, t: &VivadoTheme, on: impl for<'a> Fn(&'a mut AppState) -> &'a mut rune::TextEdit + 'static) -> El {
    rune::text_input(edit, cad::mono_style())
        .single_line()
        .key(key)
        .label(placeholder)
        .w(w)
        .h(22.0)
        .px(6.0)
        .radius(3.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border_light)
        .color(t.text)
        .placeholder(placeholder, t.text_faint)
        .caret(t.text, t.selection)
        .on_edit(on)
}

fn console(s: &mut AppState, r: Result<String, String>) {
    let (text, sev) = match r {
        Ok(v) => (v, crate::state::MsgSev::Info),
        Err(e) => (format!("ERROR: {e}"), crate::state::MsgSev::Error),
    };
    s.console.lines.push(crate::state::ConsoleLine { text, sev });
}

fn open_kernel_picker(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open HLS Kernel".into(),
        ..Default::default()
    }
    .filter("C/C++", &["c", "cc", "cpp", "cxx", "h", "hpp"]);
    tx.open_file(options, |s: &mut AppState, result, _| match result {
        Ok(Some(p)) => {
            let r = h::open_kernel(s, &p);
            console(s, r);
        }
        Ok(None) => {}
        Err(e) => console(s, Err(format!("open_hls: {e}"))),
    });
}

fn directives(s: &AppState, t: &VivadoTheme) -> El {
    let rows = s.hls.directives();
    let mut list = col().w_full();
    for (i, d) in rows.iter().enumerate() {
        let line_no = d.line;
        let ink = if d.error.is_some() { t.fail } else { t.text };
        let mut r = cad::table_row(format!("hls-dir-row-{i}"), format!("{} {}", d.kind, d.options), false, t)
            .child(cad::cell(&DIR_COLS[0], cad::mono((d.line + 1).to_string(), t)))
            .child(cad::cell(&DIR_COLS[1], cad::cut(d.kind.clone(), cad::mono_style(), ink)))
            .child(cad::cell(
                &DIR_COLS[2],
                cad::cut(
                    d.error.clone().map(|e| format!("{} — {e}", d.options)).unwrap_or_else(|| d.options.clone()),
                    cad::mono_style(),
                    if d.error.is_some() { t.fail } else { t.text_dim },
                ),
            ))
            .child(cad::cell(&DIR_COLS[3], cad::cut(d.scope.clone(), cad::mono_style(), t.text_dim)));
        r = r.child(cad::cell(
            &DIR_COLS[4],
            el().key(format!("hls-dir-del-{i}"))
                .role("button")
                .label(format!("Remove directive on line {}", d.line + 1))
                .focusable(true)
                .w(18.0)
                .h(18.0)
                .center()
                .radius(3.0)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| h::remove_directive(s, line_no))
                .child(text("×").font(VivadoTheme::FONT_UI).color(t.text_dim)),
        ));
        list = list.child(r);
    }
    if rows.is_empty() {
        list = list.child(
            row().h(cad::ROW_H).px(8.0).items_center().child(cad::dim("No #pragma HLS directives in the kernel.", t)),
        );
    }
    let kind = h::DIRECTIVE_KINDS.get(s.hls.new_kind).copied().unwrap_or("PIPELINE");
    let add_reason = if s.hls.top().is_none() { Some("The kernel has no top function yet.") } else { None };
    col()
        .key("hls-directives")
        .w_full()
        .gap(4.0)
        .child(
            row()
                .w_full()
                .h(24.0)
                .items_center()
                .gap(8.0)
                .child(cad::title("Directives", t))
                .child(cad::dim(format!("{} in source", rows.len()), t)),
        )
        .child(
            row()
                .key("hls-dir-form")
                .w_full()
                .items_center()
                .gap(6.0)
                .child(
                    cad::btn("hls-dir-kind", &format!("{kind} ▾"), t, None, |s: &mut AppState| {
                        s.hls.new_kind = (s.hls.new_kind + 1) % h::DIRECTIVE_KINDS.len();
                        s.hls.new_options = rune::TextEdit::new(match h::DIRECTIVE_KINDS[s.hls.new_kind] {
                            "PIPELINE" => "II=1",
                            "UNROLL" => "factor=2",
                            "ARRAY_PARTITION" => "variable=a complete",
                            "INTERFACE" => "mode=ap_none port=a",
                            _ => "",
                        });
                    })
                    .help("Cycle the directive kind"),
                )
                .child(
                    field("hls-dir-options", &s.hls.new_options, "options", 150.0, t, |s: &mut AppState| &mut s.hls.new_options)
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(cad::btn("hls-dir-add", "Add Directive", t, add_reason, |s: &mut AppState| {
                    let r = h::add_directive(s);
                    console(s, r);
                })),
        )
        .child(
            cad::table_frame("hls-dir-table", t)
                .child(cad::header("hls-dir-head", &DIR_COLS, t))
                .child(col().w_full().max_h(120.0).scroll(ScrollSpec { stick_bottom: false }).child(list)),
        )
}

fn source_column(s: &AppState, t: &VivadoTheme) -> El {
    let file = s
        .hls
        .file
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unsaved kernel".into());
    let mut head = row()
        .w_full()
        .h(24.0)
        .items_center()
        .gap(8.0)
        .child(cad::title("Kernel", t))
        .child(el().grow(1.0).min_w(0.0).child(cad::cut(file, VivadoTheme::FONT_UI, t.text_dim).truncate_middle().key("hls-file")));
    if let Some(top) = s.hls.top() {
        head = head.child(cad::badge(format!("top {top}"), t.link, t).key("hls-top"));
    }
    if s.hls.stale() {
        head = head.child(cad::badge("modified", t.warn, t).key("hls-stale"));
    }
    col()
        .key("hls-source")
        .w_frac(LEFT_W)
        .h_full()
        .min_h(0.0)
        .gap(8.0)
        .child(head)
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .radius(4.0)
                .border(1.0, t.border)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(editor("hls-editor", &s.hls.source, "Open or create a C/C++ kernel", t, |s: &mut AppState| &mut s.hls.source)),
        )
        .child(directives(s, t))
}

fn kpis(r: &helion_hls::HlsResult, t: &VivadoTheme) -> El {
    let rep = &r.report;
    let lat = if rep.min_latency_cycles == rep.max_latency_cycles {
        rep.max_latency_cycles.to_string()
    } else {
        format!("{}–{}", rep.min_latency_cycles, rep.max_latency_cycles)
    };
    let slack_ink = if rep.estimated_clock_ns <= rep.target_clock_ns { t.pass } else { t.fail };
    row()
        .key("hls-kpis")
        .w_full()
        .gap(8.0)
        .child(cad::kpi("hls-kpi-latency", "Latency", lat, t.text, "cycles", t).grow(1.0))
        .child(cad::kpi("hls-kpi-ii", "II", rep.ii.to_string(), t.text, format!("min {}", rep.ii_min), t).grow(1.0))
        .child(cad::kpi("hls-kpi-clock", "Est. clock", format!("{:.2} ns", rep.estimated_clock_ns), slack_ink, format!("target {:.2}", rep.target_clock_ns), t).grow(1.0))
        .child(
            col()
                .key("hls-kpi-res")
                .grow(1.6)
                .min_w(0.0)
                .gap(2.0)
                .padding(8.0)
                .radius(4.0)
                .border(1.0, t.border)
                .child(cad::caps("Resources (netlist)", t))
                .child(cad::mono(format!("LUT {}  FF {}", rep.lut_count, rep.ff_count), t))
                .child(cad::mono(format!("DSP {}  BRAM18 {}", rep.dsp_count, rep.bram18_count), t)),
        )
}

fn report_tab(r: &helion_hls::HlsResult, t: &VivadoTheme) -> El {
    let rep = &r.report;
    let mut loops = col().w_full();
    for (i, l) in rep.loop_reports.iter().enumerate() {
        loops = loops.child(
            cad::table_row(format!("hls-loop-row-{i}"), l.label.clone(), false, t)
                .child(cad::cell(&LOOP_COLS[0], cad::cut(l.label.clone(), cad::mono_style(), t.text)))
                .child(cad::cell(&LOOP_COLS[1], cad::mono(l.trip_count.to_string(), t)))
                .child(cad::cell(&LOOP_COLS[2], cad::ui(if l.pipelined { "yes" } else { "no" }, t)))
                .child(cad::cell(&LOOP_COLS[3], cad::mono(l.achieved_ii.to_string(), t)))
                .child(cad::cell(&LOOP_COLS[4], cad::mono(l.pipeline_depth.to_string(), t)))
                .child(cad::cell(&LOOP_COLS[5], cad::mono(l.latency_cycles.to_string(), t))),
        );
    }
    if rep.loop_reports.is_empty() {
        loops = loops.child(row().h(cad::ROW_H).px(8.0).items_center().child(cad::dim("Straight-line kernel (no loops).", t)));
    }
    let mut ifs = col().w_full();
    for (i, p) in rep.interface_reports.iter().enumerate() {
        ifs = ifs.child(
            cad::table_row(format!("hls-if-row-{i}"), p.c_name.clone(), false, t)
                .child(cad::cell(&IF_COLS[0], cad::cut(p.c_name.clone(), cad::mono_style(), t.text)))
                .child(cad::cell(&IF_COLS[1], cad::cut(format!("{:?}", p.mode), VivadoTheme::FONT_UI, t.text_dim)))
                .child(cad::cell(&IF_COLS[2], cad::mono(p.width.to_string(), t)))
                .child(cad::cell(&IF_COLS[3], cad::cut(p.rtl_ports.join(", "), cad::mono_style(), t.text_dim))),
        );
    }
    col()
        .w_full()
        .gap(8.0)
        .child(cad::dim(
            format!(
                "{} · part {} · target {:.2} ns (uncertainty {:.2} ns) · II_min {} (ResII {}, RecII {})",
                rep.function_name, rep.part_name, rep.target_clock_ns, rep.uncertainty_ns, rep.ii_min, rep.res_ii, rep.rec_ii
            ),
            t,
        ))
        .child(cad::caps("Loops", t))
        .child(cad::table_frame("hls-loops", t).child(cad::header("hls-loops-head", &LOOP_COLS, t)).child(loops))
        .child(cad::caps("Interfaces", t))
        .child(cad::table_frame("hls-ifs", t).child(cad::header("hls-ifs-head", &IF_COLS, t)).child(ifs))
}

fn schedule_tab(r: &helion_hls::HlsResult, t: &VivadoTheme) -> El {
    let mut list = col().w_full();
    for (i, n) in r.cdfg.nodes.iter().enumerate().take(400) {
        let cycle = r.schedule.sdc_cycles.get(i).copied().unwrap_or(0);
        let stage = r.schedule.node_stage.get(i).copied().unwrap_or(0);
        let op = format!("{:?}", n.op);
        let op = op.split(['(', ' ', '{']).next().unwrap_or("").to_string();
        let label = match &n.output_var {
            Some(v) => format!("{op} → {v}"),
            None => op,
        };
        list = list.child(
            cad::table_row(format!("hls-sched-row-{i}"), label.clone(), false, t)
                .child(cad::cell(&SCHED_COLS[0], cad::mono(n.id.to_string(), t)))
                .child(cad::cell(&SCHED_COLS[1], cad::cut(label, cad::mono_style(), t.text)))
                .child(cad::cell(&SCHED_COLS[2], cad::mono(n.bitwidth.to_string(), t)))
                .child(cad::cell(&SCHED_COLS[3], cad::mono(cycle.to_string(), t)))
                .child(cad::cell(&SCHED_COLS[4], cad::mono(stage.to_string(), t)))
                .child(cad::cell(&SCHED_COLS[5], cad::mono(format!("{:.2}", n.delay_ns), t))),
        );
    }
    col()
        .w_full()
        .gap(4.0)
        .child(cad::dim(
            format!(
                "{} operations · depth {} · SDC modulo schedule at II={}",
                r.cdfg.nodes.len(),
                r.schedule.pipeline_depth,
                r.schedule.achieved_ii
            ),
            t,
        ))
        .child(cad::table_frame("hls-sched", t).child(cad::header("hls-sched-head", &SCHED_COLS, t)).child(list))
}

fn rtl_tab(r: &helion_hls::HlsResult, t: &VivadoTheme) -> El {
    let mut c = col().key("hls-rtl").w_full();
    for (i, l) in r.sv.lines().enumerate().take(2000) {
        c = c.child(
            row()
                .h(16.0)
                .gap(8.0)
                .child(el().w(36.0).child(text(format!("{:>4}", i + 1)).font(cad::mono_style()).color(t.text_faint)))
                .child(text(l.to_string()).font(cad::mono_style()).color(t.text)),
        );
    }
    c
}

fn cosim_tab(s: &AppState, t: &VivadoTheme) -> El {
    let mut out = col()
        .w_full()
        .gap(6.0)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(cad::title("Test vectors", t))
                .child(cad::dim("one per line: a=5 b=7 · arrays x=[1,2,3]", t)),
        )
        .child(
            el().w_full()
                .h(84.0)
                .radius(4.0)
                .border(1.0, t.border)
                .child(editor("hls-vectors", &s.hls.vectors, "a=5 b=7", t, |s: &mut AppState| &mut s.hls.vectors)),
        );
    match &s.hls.cosim {
        None => out = out.child(cad::dim("Run C/RTL co-simulation to compare the RTL netlist against the C model.", t).key("hls-cosim-empty")),
        Some(Err(e)) => out = out.child(cad::cut(e.clone(), VivadoTheme::FONT_UI, t.fail).key("hls-cosim-error")),
        Some(Ok(c)) => {
            let pass = c.status == helion_hls::CosimStatus::Pass;
            out = out.child(
                row()
                    .items_center()
                    .gap(8.0)
                    .child(cad::badge(if pass { "PASS" } else { "FAIL" }, if pass { t.pass } else { t.fail }, t).key("hls-cosim-status"))
                    .child(cad::dim(
                        format!(
                            "{}/{} vectors · latency {}–{} cycles (measured in helion-sim) · II {}",
                            c.vectors_passed, c.vectors_tested, c.min_latency_cycles, c.max_latency_cycles, c.measured_ii_cycles
                        ),
                        t,
                    )),
            );
            let fmt = |m: &std::collections::BTreeMap<String, Vec<i64>>| {
                m.iter()
                    .map(|(k, v)| format!("{k}={}", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let mut list = col().w_full();
            for r in &c.vector_results {
                list = list.child(
                    cad::table_row(format!("hls-cosim-row-{}", r.vector_index), format!("vector {}", r.vector_index), false, t)
                        .child(cad::cell(&COSIM_COLS[0], cad::mono(r.vector_index.to_string(), t)))
                        .child(cad::cell(&COSIM_COLS[1], cad::cut(fmt(&r.c_outputs), cad::mono_style(), t.text)))
                        .child(cad::cell(
                            &COSIM_COLS[2],
                            cad::cut(
                                if r.rtl_outputs.is_empty() { "no output".into() } else { fmt(&r.rtl_outputs) },
                                cad::mono_style(),
                                if r.matched { t.text } else { t.fail },
                            ),
                        ))
                        .child(cad::cell(&COSIM_COLS[3], cad::mono(r.latency_cycles.to_string(), t)))
                        .child(cad::cell(
                            &COSIM_COLS[4],
                            cad::cut(if r.matched { "Pass" } else { "Fail" }, VivadoTheme::FONT_UI, if r.matched { t.pass } else { t.fail }),
                        )),
                );
            }
            out = out.child(cad::table_frame("hls-cosim", t).child(cad::header("hls-cosim-head", &COSIM_COLS, t)).child(list));
        }
    }
    out
}

fn results_column(s: &AppState, t: &VivadoTheme) -> El {
    let mut c = col().key("hls-results").grow(1.0).min_w(0.0).h_full().min_h(0.0).gap(8.0);
    c = c.child(
        row()
            .w_full()
            .h(24.0)
            .items_center()
            .gap(8.0)
            .child(cad::title("Solution", t))
            .child(cad::dim("Clock (ns)", t))
            .child(field("hls-clock", &s.hls.clock_ns, "10.0", 64.0, t, |s: &mut AppState| &mut s.hls.clock_ns))
            .child(cad::dim(format!("Part {}", s.project.part), t))
            .child(el().grow(1.0))
            .child(match s.hls.busy() {
                Some(j) => cad::badge(j.busy(), t.link, t).key("hls-busy"),
                None => el(),
            }),
    );
    let body = match (&s.hls.synth, s.hls.tab) {
        (_, HlsTab::Cosim) => cosim_tab(s, t),
        (None, _) => cad::empty_state(
            "hls-synth-empty",
            "No synthesis results yet. Run C Synthesis to schedule, bind and emit RTL for the kernel.",
            None,
            t,
        ),
        (Some(Err(e)), _) => cad::empty_state("hls-synth-error", &format!("C synthesis failed: {e}"), None, t),
        (Some(Ok(r)), HlsTab::Report) => report_tab(r, t),
        (Some(Ok(r)), HlsTab::Schedule) => schedule_tab(r, t),
        (Some(Ok(r)), HlsTab::Rtl) => rtl_tab(r, t),
    };
    if let Some(r) = s.hls.result() {
        c = c.child(kpis(r, t));
    }
    let mut tabs = row().key("hls-tabs").role("tablist").w_full().h(26.0).items_end().gap(2.0).px(4.0).fill(t.header_bg);
    for (tab, key, label) in [
        (HlsTab::Report, "hls-tab-report", "Synthesis Report"),
        (HlsTab::Schedule, "hls-tab-schedule", "Schedule"),
        (HlsTab::Rtl, "hls-tab-rtl", "Generated RTL"),
        (HlsTab::Cosim, "hls-tab-cosim", "Co-Simulation"),
    ] {
        tabs = tabs.child(cad::tab(key, label, s.hls.tab == tab, t, move |s: &mut AppState| s.hls.tab = tab));
    }
    c.child(
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(1.0)
            .radius(4.0)
            .border(1.0, t.border)
            .child(tabs)
            .child(col().w_full().grow(1.0).min_h(0.0).padding(8.0).scroll(ScrollSpec { stick_bottom: false }).child(body)),
    )
}

/// The HLS center pane.
pub fn hls_pane(s: &AppState, t: &VivadoTheme) -> El {
    let synth_reason = h::synth_reason(s);
    let cosim_reason = h::cosim_reason(s);
    let add_reason = h::add_rtl_reason(s);
    let busy_reason = s.hls.busy().map(|j| format!("{}.", j.busy()));
    let save_reason = if s.hls.source.text.trim().is_empty() { Some("Nothing to save.") } else { None };
    let tools = cad::toolbar("hls-toolbar", t)
        .child(cad::btn("hls-new", "New Kernel", t, busy_reason.as_deref(), h::new_kernel))
        .child(
            el().key("hls-open")
                .role("button")
                .label("Open…")
                .focusable(true)
                .h(cad::BTN_H)
                .px(8.0)
                .center()
                .radius(3.0)
                .border(1.0, t.border)
                .fill(t.panel_bg)
                .hover(t.selection)
                .on_press_tx(|_: &mut AppState, tx: &mut Tx| open_kernel_picker(tx))
                .child(cad::ui("Open…", t)),
        )
        .child(cad::btn("hls-save", "Save", t, save_reason, |s: &mut AppState| {
            let r = h::save_kernel(s);
            console(s, r);
        }))
        .child(cad::tool_sep(t))
        .child(cad::btn("hls-csynth", "Run C Synthesis", t, synth_reason.as_deref(), |s: &mut AppState| {
            let r = h::start_synth(s);
            console(s, r);
        }))
        .child(cad::btn("hls-cosim", "Run C/RTL Co-Simulation", t, cosim_reason.as_deref(), |s: &mut AppState| {
            let r = h::start_cosim(s);
            console(s, r);
        }))
        .child(cad::tool_sep(t))
        .child(cad::btn("hls-add-rtl", "Add RTL to Project", t, add_reason.as_deref(), |s: &mut AppState| {
            let r = h::add_rtl_to_project(s);
            console(s, r);
        }));
    col()
        .key("hls")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("High-Level Synthesis", "hls", t))
        .child(tools)
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(10.0)
                .padding(8.0)
                .child(source_column(s, t))
                .child(results_column(s, t)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    #[test]
    fn pane_mounts_and_buttons_gate_on_state() {
        let mut s = init();
        s.center = crate::state::CenterPane::Hls;
        s.hls.source = rune::TextEdit::new(h::NEW_KERNEL);
        h::start_synth(&mut s).unwrap();
        h::wait(&mut s);
        let spec = app_spec();
        let mut rt = Runtime::new(s, crate::view, spec.size, Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        for k in ["hls", "hls-toolbar", "hls-editor", "hls-directives", "hls-kpis", "hls-tabs", "hls-loops", "hls-ifs", "hls-top"] {
            assert!(rt.find(k).is_some(), "missing {k}");
        }
        assert!(h::cosim_reason(&rt.state).unwrap().contains("test vector"));
    }
}
