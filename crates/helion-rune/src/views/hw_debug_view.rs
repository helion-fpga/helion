//! Hardware Manager debug panels: Set Up Debug, hardware ILA, VIO
//! dashboard, and the JTAG-to-AXI transaction console. All model work lives
//! in [`crate::views::hw_debug`] (helion-debug on a worker); this module
//! only renders it. Unavailable actions render disabled with the reason.

use helion_debug::{AxiResp, AxiTxnKind, DebugCoreKind, LtxProbeDirection};
use rune::{El, ScrollSpec, Tx, col, el, row, text_input};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::hw_debug::{self as dbg, DEPTHS, ProbeRole, TrigMode};

/// Rows shown in the Set Up Debug net table before "refine the search".
const NET_ROWS: usize = 200;
/// Net table viewport height.
const NET_TABLE_H: f32 = 200.0;
/// `label:` width in result cards.
const LABEL_W: f32 = 96.0;

/// Compact single-line mono field.
pub fn field(
    key: impl Into<String>,
    a11y: &str,
    edit: &rune::TextEdit,
    w: f32,
    t: &VivadoTheme,
    on_edit: impl for<'a> Fn(&'a mut AppState) -> &'a mut rune::TextEdit + 'static,
) -> El {
    text_input(edit, VivadoTheme::FONT_MONO.mono())
        .single_line()
        .key(key.into())
        .label(a11y.to_string())
        .w(w)
        .h(20.0)
        .px(6.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border_light)
        .radius(3.0)
        .color(t.text)
        .caret(t.text, t.selection)
        .on_edit(on_edit)
}

/// Labelled field: dim caption then the input.
fn labelled(caption: &str, input: El, t: &VivadoTheme) -> El {
    row()
        .items_center()
        .gap(6.0)
        .child(cad::dim(caption, t))
        .child(input)
}

/// One segment of a radio strip.
pub fn segment(
    key: impl Into<String>,
    label: &str,
    on: bool,
    t: &VivadoTheme,
    enabled: bool,
    press: impl Fn(&mut AppState) + 'static,
) -> El {
    let mut e = el()
        .key(key.into())
        .role("radio")
        .label(label.to_string())
        .focusable(enabled)
        .focus_selected(on)
        .h_full()
        .px(8.0)
        .center()
        .fill(if on { t.selection } else { t.panel_bg })
        .child(cad::ui(label, t));
    if enabled {
        e = e.hover(t.selection).on_press(press);
    } else {
        e = e.disabled(true).layer_opacity(0.45);
    }
    e
}

pub fn segments(key: &str, a11y: &str, items: Vec<El>, t: &VivadoTheme) -> El {
    let mut r = row()
        .key(key.to_string())
        .role("radiogroup")
        .label(a11y.to_string())
        .h(22.0)
        .items_center()
        .radius(3.0)
        .border(1.0, t.border);
    for it in items {
        r = r.child(it);
    }
    r
}

fn act(s: &mut AppState, r: Result<String, String>) {
    if let Err(e) = r {
        s.console.lines.push(crate::state::ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: crate::state::MsgSev::Error,
        });
        s.program.debug.last_error = Some(e);
    }
}

fn error_line(s: &AppState, key: &'static str, t: &VivadoTheme) -> Option<El> {
    s.program
        .debug
        .last_error
        .as_ref()
        .map(|e| cad::cut(e.clone(), VivadoTheme::FONT_UI, t.fail).key(key))
}

/// Disabled-reason line under an action row (only when not just busy).
fn why_line(reason: Option<&str>, key: &'static str, t: &VivadoTheme) -> Option<El> {
    reason
        .filter(|r| !r.starts_with("Busy"))
        .map(|r| cad::cut(r.to_string(), VivadoTheme::FONT_UI, t.warn).key(key))
}

/// Mono path that keeps the file name visible (middle truncation).
fn path_text(p: String, t: &VivadoTheme) -> El {
    rune::text(p.clone())
        .font(cad::mono_style())
        .color(t.text)
        .truncate_middle()
        .help_when_truncated(p)
}

fn hex(v: u64, width: usize) -> String {
    let digits = width.div_ceil(4).max(1);
    format!("0x{v:0digits$X}")
}

// ---- Set Up Debug ----

fn role_label(role: Option<ProbeRole>) -> &'static str {
    match role {
        Some(r) => r.label(),
        None => "",
    }
}

fn net_table(s: &AppState, t: &VivadoTheme) -> El {
    const COLS: [ColSpec; 3] = [
        ColSpec::grow("Net (FF output)", 1.0),
        ColSpec::fixed("Role", 84.0),
        ColSpec::fixed("mark_debug", 74.0),
    ];
    let q = s.program.debug.search.text.to_ascii_lowercase();
    let marked = dbg::marked_nets(s);
    let all = dbg::candidate_nets(s);
    let hits: Vec<&String> = all
        .iter()
        .filter(|n| q.is_empty() || n.to_ascii_lowercase().contains(&q))
        .collect();
    let mut rows = col().w_full();
    if all.is_empty() {
        rows = rows.child(cad::empty_state(
            "hw-setup-nets-empty",
            "No design yet — run Synthesis to list probe-able nets.",
            None,
            t,
        ));
    }
    for n in hits.iter().take(NET_ROWS) {
        let role = s.program.debug.role_of(n);
        let net = (*n).clone();
        let slug = crate::live::slug(n);
        rows = rows.child(
            cad::table_row(
                format!("hw-setup-net-{slug}"),
                format!("{n} {}", role_label(role)),
                role.is_some(),
                t,
            )
            .on_press(move |s: &mut AppState| {
                if s.program.debug.busy().is_none() {
                    s.program.debug.toggle(&net);
                }
            })
            .child(cad::cell(
                &COLS[0],
                cad::cut((*n).clone(), cad::mono_style(), t.text),
            ))
            .child(cad::cell(
                &COLS[1],
                cad::cut(role_label(role), VivadoTheme::FONT_UI, t.text_dim),
            ))
            .child(cad::cell(
                &COLS[2],
                if marked.contains(n) {
                    cad::badge("marked", t.accent, t)
                } else {
                    el()
                },
            )),
        );
    }
    if hits.len() > NET_ROWS {
        rows = rows.child(cad::dim(
            format!("{} more — refine the search.", hits.len() - NET_ROWS),
            t,
        ));
    }
    cad::table_frame("hw-setup-nets", t)
        .child(cad::header("hw-setup-nets-head", &COLS, t))
        .child(
            col()
                .key("hw-setup-nets-scroll")
                .w_full()
                .h(NET_TABLE_H)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(rows),
        )
}

fn chosen_table(s: &AppState, t: &VivadoTheme) -> El {
    const COLS: [ColSpec; 4] = [
        ColSpec::grow("Probe net", 1.0),
        ColSpec::fixed("Core", 84.0),
        ColSpec::fixed("Port", 88.0),
        ColSpec::fixed("Trigger", 60.0),
    ];
    let d = &s.program.debug;
    let trig = d.trigger_net().map(str::to_string);
    let mut rows = col().w_full();
    let mut any = false;
    for (role, nets, port) in [
        (ProbeRole::Ila, &d.ila_nets, "PROBE"),
        (ProbeRole::VioIn, &d.vio_in, "PROBE_IN"),
        (ProbeRole::VioOut, &d.vio_out, "PROBE_OUT"),
    ] {
        for (i, n) in nets.iter().enumerate() {
            any = true;
            let is_trig = role == ProbeRole::Ila && trig.as_deref() == Some(n);
            let net = n.clone();
            let slug = crate::live::slug(n);
            let mut r = cad::table_row(
                format!("hw-setup-chosen-{slug}"),
                format!("{n} {}", role.label()),
                false,
                t,
            )
            .child(cad::cell(&COLS[0], cad::cut(n.clone(), cad::mono_style(), t.text)))
            .child(cad::cell(
                &COLS[1],
                cad::dim(
                    match role {
                        ProbeRole::Ila => "ILA",
                        _ => "VIO",
                    },
                    t,
                ),
            ))
            .child(cad::cell(&COLS[2], cad::mono(format!("{port}{i}"), t)));
            r = r.child(cad::cell(
                &COLS[3],
                if role == ProbeRole::Ila {
                    if is_trig {
                        cad::badge("edge", t.accent, t)
                    } else {
                        cad::link(
                            format!("hw-setup-trig-{slug}"),
                            "use",
                            t,
                            move |s: &mut AppState| {
                                s.program.debug.trigger_net = Some(net.clone())
                            },
                        )
                    }
                } else {
                    el()
                },
            ));
            rows = rows.child(r);
        }
    }
    if !any {
        rows = rows.child(cad::empty_state(
            "hw-setup-chosen-empty",
            "Pick nets above: each click adds the net with the selected role.",
            None,
            t,
        ));
    }
    cad::table_frame("hw-setup-chosen", t)
        .child(cad::header("hw-setup-chosen-head", &COLS, t))
        .child(rows)
}

/// Which cable Program uses, and how far that path is verified.
fn cable_line(s: &AppState, t: &VivadoTheme) -> El {
    use helion_hw::CableKind;
    let kind = dbg::planned_cable(s);
    let (ink, note) = match kind {
        CableKind::Sim => (t.text_dim, "in-process Helion TAP — the verified path".to_string()),
        CableKind::NativeMock => (
            t.text_dim,
            "MPSSE engine → simulated FTDI + Helion TAP (fixture board)".to_string(),
        ),
        CableKind::NativeUsb => (t.warn, crate::views::hw_native::NATIVE_NOTE.to_string()),
    };
    row()
        .key("hw-setup-cable")
        .w_full()
        .h(cad::ROW_H)
        .items_center()
        .gap(8.0)
        .child(cad::badge(kind.label(), ink, t))
        .child(el().grow(1.0).min_w(0.0).child(cad::cut(note, VivadoTheme::FONT_UI, t.text_dim)))
}

fn build_card(s: &AppState, t: &VivadoTheme) -> El {
    let d = &s.program.debug;
    let impl_why = dbg::implement_block(s);
    let prog_why = dbg::program_block(s);
    let mut actions = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::primary_btn(
            "hw-setup-implement",
            "Implement Debug Cores",
            t,
            impl_why.as_deref(),
            |s: &mut AppState| {
                let r = dbg::implement(s);
                act(s, r)
            },
        ))
        .child(cad::btn(
            "hw-setup-program",
            "Program Device",
            t,
            prog_why.as_deref(),
            |s: &mut AppState| {
                let r = dbg::program(s);
                act(s, r)
            },
        ));
    if let Some(k) = d.busy() {
        actions = actions.child(cad::dim(k.busy_label(), t).key("hw-setup-busy"));
    }
    let mut body = col().w_full().gap(4.0).child(actions).child(cable_line(s, t));
    if let Some(w) = why_line(impl_why.as_deref().or(prog_why.as_deref()), "hw-setup-why", t) {
        body = body.child(w);
    }
    if let Some(e) = error_line(s, "hw-setup-error", t) {
        body = body.child(e);
    }
    if let Some(b) = &d.build {
        let cores: Vec<String> = b
            .report
            .hub
            .slots
            .iter()
            .map(|c| format!("{} ({})", c.core_name, c.kind.as_str()))
            .collect();
        body = body
            .child(cad::kv(
                "LUT+FF",
                cad::mono(
                    format!(
                        "{} → {} (+{} debug)",
                        b.baseline_lutffs,
                        b.debug_lutffs,
                        b.debug_lutffs.saturating_sub(b.baseline_lutffs)
                    ),
                    t,
                ),
                LABEL_W,
                t,
            ))
            .child(cad::kv(
                "Cores",
                cad::cut(cores.join(" · "), VivadoTheme::FONT_UI, t.text),
                LABEL_W,
                t,
            ))
            .child(cad::kv(
                "Hub",
                cad::mono(
                    format!(
                        "{} · BSCANE2 USER{} · clock {}",
                        b.report.hub_cell_name, b.report.bscan_chain, b.report.clock_net
                    ),
                    t,
                ),
                LABEL_W,
                t,
            ));
        if let Some(f) = &d.files {
            body = body
                .child(cad::kv(
                    "Probes file",
                    path_text(f.ltx.display().to_string(), t).key("hw-setup-ltx"),
                    LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Bitstream",
                    path_text(f.hbits.display().to_string(), t),
                    LABEL_W,
                    t,
                ));
        }
    }
    cad::card("hw-setup-build", "Implement & Program", body, t)
}

/// Set Up Debug panel.
pub fn setup_panel(s: &AppState, t: &VivadoTheme) -> El {
    let d = &s.program.debug;
    let busy = d.busy().is_some();
    let mut roles = Vec::new();
    for (i, r) in ProbeRole::ALL.into_iter().enumerate() {
        roles.push(segment(
            format!("hw-setup-role-{i}"),
            r.label(),
            d.role == r,
            t,
            !busy,
            move |s: &mut AppState| s.program.debug.role = r,
        ));
    }
    let mut depths = Vec::new();
    for dep in DEPTHS {
        depths.push(segment(
            format!("hw-setup-depth-{dep}"),
            &dep.to_string(),
            d.depth == dep,
            t,
            !busy,
            move |s: &mut AppState| s.program.debug.depth = dep,
        ));
    }
    let axi_on = d.jtag_axi;
    let pick = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(segments("hw-setup-role", "Add picked nets as", roles, t))
        .child(
            el().grow(1.0).min_w(0.0).child(cad::search_input(
                "hw-setup-search",
                &d.search,
                "Filter nets",
                t,
                |s: &mut AppState| &mut s.program.debug.search,
            )
            .w_full()),
        );
    let options = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::dim("Sample depth", t))
        .child(segments("hw-setup-depth", "Sample depth", depths, t))
        .child(el().w(8.0))
        .child(segments(
            "hw-setup-axi",
            "JTAG-to-AXI master",
            vec![segment(
                "hw-setup-axi-toggle",
                if axi_on {
                    "JTAG-to-AXI: on"
                } else {
                    "JTAG-to-AXI: off"
                },
                axi_on,
                t,
                !busy,
                |s: &mut AppState| s.program.debug.jtag_axi = !s.program.debug.jtag_axi,
            )],
            t,
        ));
    col()
        .key("hw-setup")
        .w_full()
        .gap(10.0)
        .child(cad::card(
            "hw-setup-nets-card",
            "Set Up Debug",
            col()
                .w_full()
                .gap(6.0)
                .child(cad::cut(
                    "Inserts an ILA (and optional VIO / JTAG-to-AXI) behind a BSCANE2 debug hub \
                     into the synthesized netlist, then re-implements it.",
                    VivadoTheme::FONT_UI,
                    t.text_dim,
                ))
                .child(pick)
                .child(net_table(s, t))
                .child(options),
            t,
        ))
        .child(cad::card(
            "hw-setup-chosen-card",
            "Debug Probes",
            chosen_table(s, t),
            t,
        ))
        .child(build_card(s, t))
}

// ---- hardware ILA ----

fn core_head(s: &AppState, kind: DebugCoreKind, t: &VivadoTheme) -> El {
    let snap = s.program.debug.snap.as_ref();
    let core = snap.and_then(|sn| sn.cores.iter().find(|c| c.kind == kind));
    let (name, uuid, slot) = core
        .map(|c| (c.name.clone(), c.uuid.clone(), c.slot.to_string()))
        .unwrap_or_default();
    row()
        .key("hw-core-head")
        .w_full()
        .gap(8.0)
        .child(cad::kpi("hw-core-name", "Core", name, t.text, format!("slot {slot}"), t))
        .child(cad::kpi(
            "hw-core-target",
            "Target",
            snap.map(|s| s.part.clone()).unwrap_or_default(),
            t.text,
            snap.map(|s| s.cable.label()).unwrap_or("Simulation cable"),
            t,
        ))
        .child(cad::kpi(
            "hw-core-clocks",
            "User clocks",
            cad::grouped(snap.map(|s| s.cycles as usize).unwrap_or(0)),
            t.text,
            format!(
                "{} hub DR scans",
                cad::grouped(snap.map(|s| s.hub_frames).unwrap_or(0))
            ),
            t,
        ))
        .child(cad::kpi("hw-core-uuid", "UUID", uuid, t.text_dim, "BSCAN hub discovery", t))
}

/// Hardware ILA panel.
pub fn ila_panel(s: &AppState, t: &VivadoTheme) -> El {
    let d = &s.program.debug;
    let why = dbg::ila_block(s);
    let busy = d.busy().is_some();
    let mut modes = Vec::new();
    for (i, m) in TrigMode::ALL.into_iter().enumerate() {
        modes.push(segment(
            format!("hw-ila-trig-{i}"),
            m.label(),
            d.trig == m,
            t,
            !busy,
            move |s: &mut AppState| s.program.debug.trig = m,
        ));
    }
    let trig_net = d
        .build
        .as_ref()
        .map(|b| b.report.trigger_net.clone())
        .unwrap_or_default();
    let note = match d.trig {
        TrigMode::Immediate => "Fires on the first sample after arming.".to_string(),
        TrigMode::Rising | TrigMode::Falling => format!(
            "Edge on {trig_net} (match-unit LUT, reprogrammed at arm)."
        ),
        TrigMode::Value => format!(
            "Compares the first {} probe bits (one LUT6 comparator).",
            helion_debug::VALUE_COMPARE_BITS
        ),
    };
    let mut trig_row = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(segments("hw-ila-trig", "Trigger", modes, t));
    if d.trig == TrigMode::Value {
        trig_row = trig_row
            .child(labelled(
                "Value",
                field("hw-ila-value", "Trigger value", &d.trig_value, 90.0, t, |s| {
                    &mut s.program.debug.trig_value
                }),
                t,
            ))
            .child(labelled(
                "Mask",
                field("hw-ila-mask", "Trigger mask", &d.trig_mask, 90.0, t, |s| {
                    &mut s.program.debug.trig_mask
                }),
                t,
            ));
    }
    let depth = d
        .build
        .as_ref()
        .and_then(|b| b.core("ila").map(|c| c.sample_depth))
        .unwrap_or(0);
    let window_row = row()
        .w_full()
        .items_center()
        .gap(12.0)
        .child(labelled(
            "Window",
            field("hw-ila-window", "Capture window", &d.window, 56.0, t, |s| {
                &mut s.program.debug.window
            }),
            t,
        ))
        .child(labelled(
            "Pre-trigger",
            field("hw-ila-pre", "Pre-trigger samples", &d.pre, 56.0, t, |s| {
                &mut s.program.debug.pre
            }),
            t,
        ))
        .child(labelled(
            "Max clocks",
            field("hw-ila-max", "Max clocks", &d.max_cycles, 72.0, t, |s| {
                &mut s.program.debug.max_cycles
            }),
            t,
        ))
        .child(cad::dim(format!("depth {depth}"), t));
    let show_why = if d.last_capture.is_none() {
        Some("Run the trigger first.")
    } else {
        None
    };
    let mut actions = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::primary_btn(
            "hw-ila-run",
            "Run Trigger",
            t,
            why.as_deref(),
            |s: &mut AppState| {
                let r = dbg::run_ila(s);
                act(s, r)
            },
        ))
        .child(cad::btn(
            "hw-ila-show",
            "Show in Waveform",
            t,
            show_why,
            |s: &mut AppState| {
                let r = dbg::show_in_viewer(s);
                act(s, r)
            },
        ));
    if let Some(k) = d.busy() {
        actions = actions.child(cad::dim(k.busy_label(), t).key("hw-ila-busy"));
    }
    let mut setup = col()
        .w_full()
        .gap(6.0)
        .child(trig_row)
        .child(cad::dim(note, t).key("hw-ila-note"))
        .child(window_row)
        .child(actions);
    if let Some(w) = why_line(why.as_deref(), "hw-ila-why", t) {
        setup = setup.child(w);
    }
    if let Some(e) = error_line(s, "hw-ila-error", t) {
        setup = setup.child(e);
    }
    col()
        .key("hw-ila")
        .w_full()
        .gap(10.0)
        .child(core_head(s, DebugCoreKind::Ila, t))
        .child(cad::card("hw-ila-setup", "Trigger & Capture", setup, t))
        .child(capture_card(s, t))
}

fn capture_card(s: &AppState, t: &VivadoTheme) -> El {
    let d = &s.program.debug;
    let (Some(cap), Some(build)) = (&d.last_capture, &d.build) else {
        return cad::card(
            "hw-ila-capture",
            "Last Hardware Capture",
            cad::empty_state(
                "hw-ila-capture-empty",
                "No hardware capture yet. Run Trigger to arm the ILA and upload a window.",
                None,
                t,
            ),
            t,
        );
    };
    let view = dbg::to_viewer(cap, build);
    const COLS: [ColSpec; 4] = [
        ColSpec::grow("Probe", 1.0),
        ColSpec::num("Width", 48.0),
        ColSpec::num("@ trigger", 96.0),
        ColSpec::num("Last", 96.0),
    ];
    let mut rows = col().w_full();
    let last = view.len().saturating_sub(1);
    for (i, p) in view.probes.iter().enumerate() {
        let at = view.value_at(i, cap.trigger_at).unwrap_or(0);
        let end = view.value_at(i, last).unwrap_or(0);
        rows = rows.child(
            cad::table_row(format!("hw-ila-probe-{i}"), p.name.clone(), false, t)
                .child(cad::cell(&COLS[0], cad::cut(p.name.clone(), cad::mono_style(), t.text)))
                .child(cad::cell(&COLS[1], cad::mono(p.width().to_string(), t)))
                .child(cad::cell(&COLS[2], cad::mono(hex(at, p.width()), t)))
                .child(cad::cell(&COLS[3], cad::mono(hex(end, p.width()), t))),
        );
    }
    let summary = format!(
        "{} samples · trigger @ {} ({}) · {} clocks · {} USR1 scans (raw {}) · {} hub frames",
        cap.words.len(),
        cap.trigger_at,
        cap.trigger.label(),
        cad::grouped(cap.cycles),
        cap.upload_dr_scans,
        cap.upload_raw_scans,
        cap.hub_frames
    );
    cad::card(
        "hw-ila-capture",
        "Last Hardware Capture",
        col()
            .w_full()
            .gap(6.0)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(cad::badge("HARDWARE", t.accent, t))
                    .child(cad::cut(summary, VivadoTheme::FONT_UI, t.text_dim).key("hw-ila-summary")),
            )
            .child(
                cad::table_frame("hw-ila-probes", t)
                    .child(cad::header("hw-ila-probes-head", &COLS, t))
                    .child(rows),
            ),
        t,
    )
}

// ---- VIO ----

/// VIO dashboard panel.
pub fn vio_panel(s: &AppState, t: &VivadoTheme) -> El {
    let d = &s.program.debug;
    let why = dbg::vio_block(s);
    let mut tools = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::primary_btn(
            "hw-vio-refresh",
            "Refresh",
            t,
            why.as_deref(),
            |s: &mut AppState| {
                let r = dbg::refresh_vio(s);
                act(s, r)
            },
        ))
        .child(labelled(
            "Run clocks",
            field("hw-vio-cycles", "Clocks per refresh", &d.vio_cycles, 64.0, t, |s| {
                &mut s.program.debug.vio_cycles
            }),
            t,
        ))
        .child(cad::btn(
            "hw-vio-reset",
            "Reset Activity",
            t,
            why.as_deref(),
            |s: &mut AppState| {
                let r = dbg::reset_activity(s);
                act(s, r)
            },
        ));
    if let Some(k) = d.busy() {
        tools = tools.child(cad::dim(k.busy_label(), t).key("hw-vio-busy"));
    }
    let probes = d.snap.as_ref().map(|s| s.vio.clone()).unwrap_or_default();
    const IN_COLS: [ColSpec; 5] = [
        ColSpec::grow("Input probe", 1.0),
        ColSpec::num("Width", 44.0),
        ColSpec::num("Value", 92.0),
        ColSpec::fixed("Activity", 72.0),
        ColSpec::num("Toggles", 60.0),
    ];
    let mut ins = col().w_full();
    for (i, p) in probes
        .iter()
        .filter(|p| p.direction == LtxProbeDirection::ProbeIn)
        .enumerate()
    {
        let ink = match p.activity {
            "NONE" => t.text_dim,
            "BOTH" => t.accent,
            _ => t.pass,
        };
        ins = ins.child(
            cad::table_row(format!("hw-vio-in-{i}"), p.name.clone(), false, t)
                .child(cad::cell(
                    &IN_COLS[0],
                    cad::cut(p.name.clone(), cad::mono_style(), t.text)
                        .help(p.nets.join(" ")),
                ))
                .child(cad::cell(&IN_COLS[1], cad::mono(p.width.to_string(), t)))
                .child(cad::cell(&IN_COLS[2], cad::mono(hex(p.value, p.width), t)))
                .child(cad::cell(&IN_COLS[3], cad::badge(p.activity, ink, t)))
                .child(cad::cell(&IN_COLS[4], cad::mono(p.transitions.to_string(), t))),
        );
    }
    const OUT_COLS: [ColSpec; 5] = [
        ColSpec::grow("Output probe", 1.0),
        ColSpec::num("Committed", 92.0),
        ColSpec::fixed("New value", 96.0),
        ColSpec::fixed("", 116.0),
        ColSpec::fixed("State", 72.0),
    ];
    let mut outs = col().w_full();
    for (i, p) in probes
        .iter()
        .filter(|p| p.direction == LtxProbeDirection::ProbeOut)
        .enumerate()
    {
        let name = p.name.clone();
        let edit = d
            .vio_edits
            .iter()
            .find(|(n, _)| *n == p.name)
            .map(|(_, e)| e.clone())
            .unwrap_or_else(|| rune::TextEdit::new("0x0"));
        let n_edit = name.clone();
        let n_apply = name.clone();
        let n_rel = name.clone();
        outs = outs.child(
            cad::table_row(format!("hw-vio-out-{i}"), p.name.clone(), false, t)
                .child(cad::cell(
                    &OUT_COLS[0],
                    cad::cut(p.name.clone(), cad::mono_style(), t.text)
                        .help(p.nets.join(" ")),
                ))
                .child(cad::cell(&OUT_COLS[1], cad::mono(hex(p.value, p.width), t)))
                .child(cad::cell(
                    &OUT_COLS[2],
                    field(
                        format!("hw-vio-out-{i}-value"),
                        "OUTPUT_VALUE",
                        &edit,
                        88.0,
                        t,
                        move |s| s.program.debug.vio_edit_mut(&n_edit),
                    ),
                ))
                .child(cad::cell(
                    &OUT_COLS[3],
                    row()
                        .gap(4.0)
                        .child(cad::btn(
                            format!("hw-vio-out-{i}-apply"),
                            "Drive",
                            t,
                            why.as_deref(),
                            move |s: &mut AppState| {
                                let text = s.program.debug.vio_edit_mut(&n_apply).text.clone();
                                let r = dbg::parse_num(&text)
                                    .and_then(|v| dbg::set_output(s, &n_apply, v));
                                act(s, r)
                            },
                        ))
                        .child(cad::btn(
                            format!("hw-vio-out-{i}-release"),
                            "Release",
                            t,
                            if p.driving {
                                why.as_deref()
                            } else {
                                Some("Not driving this net.")
                            },
                            move |s: &mut AppState| {
                                let r = dbg::release_output(s, &n_rel);
                                act(s, r)
                            },
                        )),
                ))
                .child(cad::cell(
                    &OUT_COLS[4],
                    if p.driving {
                        cad::badge("DRIVING", t.warn, t)
                    } else {
                        cad::badge("released", t.text_dim, t)
                    },
                )),
        );
    }
    let in_body = if probes.iter().any(|p| p.direction == LtxProbeDirection::ProbeIn) {
        cad::table_frame("hw-vio-ins", t)
            .child(cad::header("hw-vio-ins-head", &IN_COLS, t))
            .child(ins)
    } else {
        cad::empty_state("hw-vio-ins-empty", "No VIO input probes in this build.", None, t)
    };
    let out_body = if probes.iter().any(|p| p.direction == LtxProbeDirection::ProbeOut) {
        cad::table_frame("hw-vio-outs", t)
            .child(cad::header("hw-vio-outs-head", &OUT_COLS, t))
            .child(outs)
    } else {
        cad::empty_state("hw-vio-outs-empty", "No VIO output probes in this build.", None, t)
    };
    let mut body = col().w_full().gap(6.0).child(tools);
    if let Some(w) = why_line(why.as_deref(), "hw-vio-why", t) {
        body = body.child(w);
    }
    if let Some(e) = error_line(s, "hw-vio-error", t) {
        body = body.child(e);
    }
    col()
        .key("hw-vio")
        .w_full()
        .gap(10.0)
        .child(core_head(s, DebugCoreKind::Vio, t))
        .child(cad::card("hw-vio-inputs", "Inputs (PROBE_IN)", body.child(in_body), t))
        .child(cad::card(
            "hw-vio-outputs",
            "Outputs (PROBE_OUT)",
            col()
                .w_full()
                .gap(6.0)
                .child(cad::cut(
                    format!(
                        "Drive overrides the flop that feeds the net on every clock until \
                         Release ({}).",
                        s.program
                            .debug
                            .snap
                            .as_ref()
                            .map(|sn| sn.cable.label())
                            .unwrap_or("Simulation cable")
                    ),
                    VivadoTheme::FONT_UI,
                    t.text_dim,
                ))
                .child(out_body),
            t,
        ))
}

// ---- JTAG-to-AXI ----

fn resp_color(r: &Result<AxiResp, String>, t: &VivadoTheme) -> rune::Color {
    match r {
        Ok(AxiResp::Okay) | Ok(AxiResp::ExOkay) => t.pass,
        Ok(_) => t.warn,
        Err(_) => t.fail,
    }
}

/// JTAG-to-AXI transaction console.
pub fn axi_panel(s: &AppState, t: &VivadoTheme) -> El {
    let d = &s.program.debug;
    let why = dbg::axi_block(s);
    let busy = d.busy().is_some();
    let regions = d
        .snap
        .as_ref()
        .map(|s| s.axi_regions.clone())
        .unwrap_or_default();
    const REG_COLS: [ColSpec; 3] = [
        ColSpec::grow("Aperture (fabric BRAM)", 1.0),
        ColSpec::num("Base", 104.0),
        ColSpec::num("Range", 64.0),
    ];
    let mut reg_rows = col().w_full();
    for (i, (name, base, size)) in regions.iter().enumerate() {
        let b = *base;
        reg_rows = reg_rows.child(
            cad::table_row(format!("hw-axi-region-{i}"), name.clone(), false, t)
                .on_press(move |s: &mut AppState| {
                    s.program.debug.axi_addr = rune::TextEdit::new(format!("{b:#010x}"))
                })
                .child(cad::cell(&REG_COLS[0], cad::cut(name.clone(), cad::mono_style(), t.text)))
                .child(cad::cell(&REG_COLS[1], cad::mono(cad::addr_hex(u64::from(*base)), t)))
                .child(cad::cell(
                    &REG_COLS[2],
                    cad::mono(format!("{} words", size / 4), t),
                )),
        );
    }
    let kinds = vec![
        segment(
            "hw-axi-read",
            "Read",
            d.axi_kind == AxiTxnKind::Read,
            t,
            !busy,
            |s: &mut AppState| s.program.debug.axi_kind = AxiTxnKind::Read,
        ),
        segment(
            "hw-axi-write",
            "Write",
            d.axi_kind == AxiTxnKind::Write,
            t,
            !busy,
            |s: &mut AppState| s.program.debug.axi_kind = AxiTxnKind::Write,
        ),
    ];
    let mut console_row = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(segments("hw-axi-kind", "Transaction type", kinds, t))
        .child(labelled(
            "Address",
            field("hw-axi-addr", "Address", &d.axi_addr, 96.0, t, |s| {
                &mut s.program.debug.axi_addr
            }),
            t,
        ))
        .child(labelled(
            "Burst",
            field("hw-axi-len", "Burst length", &d.axi_len, 40.0, t, |s| {
                &mut s.program.debug.axi_len
            }),
            t,
        ));
    if d.axi_kind == AxiTxnKind::Write {
        console_row = console_row.child(
            el().grow(1.0).min_w(0.0).child(
                text_input(&d.axi_data, VivadoTheme::FONT_MONO.mono())
                    .single_line()
                    .key("hw-axi-data")
                    .label("Write data words")
                    .w_full()
                    .h(20.0)
                    .px(6.0)
                    .fill(t.chrome_bg)
                    .border(1.0, t.border_light)
                    .radius(3.0)
                    .color(t.text)
                    .placeholder("data words: 0x1 0x2 …", t.text_faint)
                    .caret(t.text, t.selection)
                    .on_edit(|s: &mut AppState| &mut s.program.debug.axi_data)
                    .on_submit(|s: &mut AppState, _tx: &mut Tx| dbg::run_console_txn(s)),
            ),
        );
    } else {
        console_row = console_row.child(el().grow(1.0));
    }
    console_row = console_row.child(cad::primary_btn(
        "hw-axi-run",
        "Run",
        t,
        why.as_deref(),
        dbg::run_console_txn,
    ));
    const LOG_COLS: [ColSpec; 7] = [
        ColSpec::fixed("Txn", 64.0),
        ColSpec::fixed("Type", 44.0),
        ColSpec::num("Address", 104.0),
        ColSpec::num("Len", 32.0),
        ColSpec::fixed("Resp", 64.0),
        ColSpec::grow("Data", 1.0),
        ColSpec::num("DR", 40.0),
    ];
    let mut log = col().w_full();
    for (i, r) in d.axi_log.iter().enumerate() {
        let data = r
            .data
            .iter()
            .map(|w| format!("{w:08x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let (resp, ink) = match &r.resp {
            Ok(x) => (x.as_str().to_string(), resp_color(&r.resp, t)),
            Err(_) => ("ERROR".to_string(), t.fail),
        };
        log = log.child(
            cad::table_row(
                format!("hw-axi-log-{i}"),
                r.name.clone(),
                d.axi_selected == Some(i),
                t,
            )
            .on_press(move |s: &mut AppState| s.program.debug.axi_selected = Some(i))
            .child(cad::cell(&LOG_COLS[0], cad::cut(r.name.clone(), cad::mono_style(), t.text)))
            .child(cad::cell(
                &LOG_COLS[1],
                cad::dim(if r.kind == AxiTxnKind::Read { "R" } else { "W" }, t),
            ))
            .child(cad::cell(&LOG_COLS[2], cad::mono(cad::addr_hex(u64::from(r.addr)), t)))
            .child(cad::cell(&LOG_COLS[3], cad::mono(r.len.to_string(), t)))
            .child(cad::cell(&LOG_COLS[4], cad::badge(resp, ink, t)))
            .child(cad::cell(&LOG_COLS[5], cad::cut(data, cad::mono_style(), t.text_dim)))
            .child(cad::cell(&LOG_COLS[6], cad::mono(r.dr_scans.to_string(), t))),
        );
    }
    let log_body = if d.axi_log.is_empty() {
        cad::empty_state(
            "hw-axi-log-empty",
            "No transactions yet. Pick an aperture, set the address, and Run.",
            None,
            t,
        )
    } else {
        cad::table_frame("hw-axi-log", t)
            .child(cad::header("hw-axi-log-head", &LOG_COLS, t))
            .child(log)
    };
    let mut console_body = col().w_full().gap(6.0).child(console_row);
    if let Some(w) = why_line(why.as_deref(), "hw-axi-why", t) {
        console_body = console_body.child(w);
    }
    if let Some(e) = error_line(s, "hw-axi-error", t) {
        console_body = console_body.child(e);
    }
    if let Some(r) = d.axi_selected.and_then(|i| d.axi_log.get(i)) {
        if let Err(e) = &r.resp {
            console_body = console_body.child(cad::cut(e.clone(), VivadoTheme::FONT_UI, t.fail));
        }
    }
    col()
        .key("hw-axi")
        .w_full()
        .gap(10.0)
        .child(core_head(s, DebugCoreKind::JtagAxi, t))
        .child(cad::card(
            "hw-axi-map",
            "Address Map",
            col()
                .w_full()
                .gap(6.0)
                .child(cad::cut(
                    "Apertures are the debug build's fabric BRAMs; HAD netlists have no AXI \
                     interconnect, so other addresses answer DECERR.",
                    VivadoTheme::FONT_UI,
                    t.text_dim,
                ))
                .child(
                    cad::table_frame("hw-axi-regions", t)
                        .child(cad::header("hw-axi-regions-head", &REG_COLS, t))
                        .child(reg_rows),
                ),
            t,
        ))
        .child(cad::card("hw-axi-console", "Transaction", console_body, t))
        .child(cad::card("hw-axi-log-card", "Transaction Log", log_body, t))
}
