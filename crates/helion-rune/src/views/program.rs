//! Hardware Manager view: cable picker, USB scan banner, Program Device
//! with real stage progress, and STAT register cards.
//!
//! Ports the old IDE's Hardware Manager (`helion-ide.rs:2076
//! paint_program_side`, `:13479` main panel, `:13539` STAT cards). All model
//! work (cable resolve, detect, program, STAT) lives in
//! [`crate::views::hw`] and runs on real `helion-hw` backends; this module
//! only renders it. Nothing here names a cable, server, or target the scan
//! did not report.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el/layout.rs`: `.w_frac()` progress fill.
//! - `crates/rune/src/el/a11y.rs`: `.disabled()/.description()` gating.

use rune::{El, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::sources::pane_header;

pub use crate::views::hw::*;

/// Connection-column width (the program column takes the rest).
const LEFT_W: f32 = 300.0;
/// Cable option row height.
const OPT_H: f32 = 22.0;
/// Result `label:` width.
const LABEL_W: f32 = 74.0;
/// STAT cards per row.
const STAT_COLS: usize = 4;

fn stat_color(row: &StatRow, t: &VivadoTheme) -> rune::Color {
    match (row.name, row.value) {
        ("CRC_ERR", true) => t.fail,
        ("CRC_ERR", false) => t.pass,
        ("GTS" | "GSR", true) => t.warn,
        ("GTS" | "GSR", false) => t.pass,
        (_, true) => t.pass,
        (_, false) => t.text_dim,
    }
}

/// Scan banner: honest USB state with the last scan line.
fn banner(s: &AppState, t: &VivadoTheme) -> El {
    let (glyph, ink, headline, sub) = match &s.program.detect {
        None => (
            &glyphs::CIRCLE_X,
            t.text_dim,
            "USB not scanned".to_string(),
            "Detect lists cables and USB programmers; it never claims a board DONE.".to_string(),
        ),
        Some(d) if !d.physical_had => (
            &glyphs::CIRCLE_X,
            t.warn,
            "No USB programmer detected".to_string(),
            format!("Last scan · {}", scan_line(d)),
        ),
        Some(d) => {
            let first = d
                .usb
                .probes
                .first()
                .map(|p| p.detail.clone())
                .unwrap_or_default();
            (
                &glyphs::CHECK,
                t.pass,
                format!(
                    "USB programmer detected · {} probe{} (detect only)",
                    d.usb.probes.len(),
                    if d.usb.probes.len() == 1 { "" } else { "s" }
                ),
                format!("{} · {first}", scan_line(d)),
            )
        }
    };
    row()
        .key("program-banner")
        .w_full()
        .gap(8.0)
        .px(10.0)
        .py(6.0)
        .items_center()
        .radius(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light)
        .child(icon(glyph, 14.0).color(ink).a11y_hidden())
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(headline)
                        .font(VivadoTheme::FONT_TITLE)
                        .color(ink)
                        .key("program-banner-title"),
                )
                .child(
                    cad::cut(sub, VivadoTheme::FONT_UI, t.text_dim).key("program-banner-detail"),
                ),
        )
}

fn cable_row(index: usize, cable: CableChoice, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.program.cable == cable;
    let busy = s.program.busy().is_some();
    let mut item = row()
        .key(format!("program-cable-opt-{index}"))
        .role("radio")
        .label(format!("Cable: {}", cable.label()))
        .focus_selected(selected)
        .focusable(!busy)
        .disabled(busy)
        .w_full()
        .h(OPT_H)
        .items_center()
        .gap(6.0)
        .px(6.0)
        .radius(3.0)
        .fill(if selected { t.selection } else { t.panel_bg })
        .child(if selected {
            icon(&glyphs::CHECK, 12.0).color(t.accent).a11y_hidden()
        } else {
            el().size(12.0, 12.0)
        })
        .child(cad::ui(cable.label(), t).grow(1.0).min_w(0.0).truncate())
        .child(cad::dim(cable.hint(), t).truncate());
    if !busy {
        item = item
            .hover(t.selection)
            .on_press(move |s: &mut AppState| s.program.cable = cable);
    }
    item
}

fn cable_card(s: &AppState, t: &VivadoTheme) -> El {
    let mut options = col().key("program-cable").w_full().gap(1.0);
    for (i, c) in CableChoice::ALL.into_iter().enumerate() {
        options = options.child(cable_row(i, c, s, t));
    }
    let resolved = match s.program.resolved() {
        Some(info) => col()
            .w_full()
            .child(
                cad::mono(format!("{} · {}", info.id, info.backend.as_str()), t)
                    .key("program-cable-resolved"),
            )
            .child(cad::cut(info.detail, VivadoTheme::FONT_UI, t.text_dim)),
        None => col()
            .w_full()
            .child(cad::dim("Resolves after Detect scans USB.", t).key("program-cable-resolved")),
    };
    cad::card(
        "program-connection",
        "Cable",
        col().w_full().gap(6.0).child(options).child(resolved),
        t,
    )
}

fn bitstream_card(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().w_full().gap(4.0);
    match bits_source(s) {
        Some((_, label)) => {
            body = body
                .child(cad::cut(label, VivadoTheme::FONT_UI, t.text).key("program-bitstream-path"));
            if let Some(p) = &s.program.file {
                body = body.child(
                    cad::cut(p.display().to_string(), VivadoTheme::FONT_UI, t.text_dim)
                        .truncate_middle(),
                );
            }
        }
        None => {
            body = body.child(cad::dim("No bitstream yet.", t).key("program-bitstream-path"));
        }
    }
    let mut actions = row().w_full().items_center().gap(6.0).child(
        el().key("program-browse")
            .role("button")
            .label("Browse...")
            .focusable(true)
            .h(cad::BTN_H)
            .px(8.0)
            .center()
            .radius(3.0)
            .border(1.0, t.border)
            .hover(t.selection)
            .on_press_tx(|_: &mut AppState, tx: &mut Tx| crate::pickers::bitstream(tx))
            .child(cad::ui("Browse...", t)),
    );
    if s.program.file.is_some() {
        actions = actions.child(cad::link(
            "program-use-run",
            "Use generated bitstream",
            t,
            |s: &mut AppState| s.program.file = None,
        ));
    } else if bits_source(s).is_none() {
        let running = s.runs.active_kind();
        actions = actions.child(match running {
            Some(k) => cad::dim(format!("{} running…", k.run_name()), t),
            None => cad::link(
                "program-generate",
                "Generate Bitstream",
                t,
                |s: &mut AppState| crate::views::bitstream::generate(s),
            ),
        });
    }
    cad::card("program-bitstream", "Bitstream", body.child(actions), t)
}

fn program_card(s: &AppState, t: &VivadoTheme) -> El {
    let reason = program_block_reason(s);
    let target = match s.program.resolved() {
        Some(info) => format!("{} via {}", program_part(s), info.id),
        None => program_part(s),
    };
    let mut body = col().w_full().gap(6.0).child(
        row()
            .w_full()
            .items_center()
            .gap(8.0)
            .child(cad::primary_btn(
                "program-program",
                "Program Device",
                t,
                reason.as_deref(),
                program,
            ))
            .child(cad::cut(target, VivadoTheme::FONT_UI, t.text_dim).key("program-target")),
    );
    if let Some(why) = &reason {
        if s.program.busy().is_none() {
            body = body
                .child(cad::cut(why.clone(), VivadoTheme::FONT_UI, t.warn).key("program-blocked"));
        }
    }
    let frac = f32::from(s.program.stage) / f32::from(STAGES);
    let progress_label = match s.program.phase {
        ProgramPhase::Idle => "Idle".to_string(),
        ProgramPhase::Programming => format!(
            "Step {}/{} · {}",
            s.program.stage.max(1),
            STAGES,
            s.program.stage_label
        ),
        ProgramPhase::Done => "Complete".into(),
        ProgramPhase::Failed => "Failed".into(),
    };
    let bar_ink = match s.program.phase {
        ProgramPhase::Failed => t.fail,
        ProgramPhase::Done => t.pass,
        _ => t.accent,
    };
    body = body
        .child(
            el().key("program-progress-bar")
                .w_full()
                .h(6.0)
                .radius(3.0)
                .fill(t.border_light)
                .child(
                    el().key("program-progress-fill")
                        .w_frac(frac)
                        .h_full()
                        .radius(3.0)
                        .fill(bar_ink),
                ),
        )
        .child(cad::dim(progress_label, t).key("program-progress-label"));
    if let Some(res) = &s.program.result {
        let ink = if res.ok { t.pass } else { t.fail };
        let mut detail = col().key("program-results-detail").w_full();
        if !res.backend.is_empty() {
            detail = detail.child(cad::kv(
                "Backend",
                cad::mono(res.backend.clone(), t),
                LABEL_W,
                t,
            ));
        }
        if res.frames > 0 || res.bytes > 0 {
            detail = detail.child(cad::kv(
                "Payload",
                cad::mono(
                    format!(
                        "{} frames · {} B",
                        cad::grouped(res.frames),
                        cad::grouped(res.bytes)
                    ),
                    t,
                ),
                LABEL_W,
                t,
            ));
        }
        detail = detail.child(cad::kv(
            if res.ok { "Engine" } else { "Error" },
            cad::cut(
                res.line.clone(),
                cad::mono_style(),
                if res.ok { t.text } else { t.fail },
            ),
            LABEL_W,
            t,
        ));
        body = body.child(
            col()
                .key("program-results")
                .w_full()
                .gap(2.0)
                .pt(4.0)
                .child(
                    cad::cut(
                        res.verdict(s.program.stat.as_ref()),
                        VivadoTheme::FONT_TITLE,
                        ink,
                    )
                    .key("program-verdict"),
                )
                .child(detail),
        );
    }
    cad::card("program-progress", "Program", body, t)
}

fn stat_card(row_: StatRow, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.program.stat_selected == Some(row_.name);
    let ink = stat_color(&row_, t);
    let name = row_.name;
    col()
        .key(format!(
            "program-stat-{}",
            name.to_ascii_lowercase().replace('_', "-")
        ))
        .role("button")
        .label(format!(
            "{} {}",
            name,
            if row_.value { "HIGH" } else { "LOW" }
        ))
        .focus_selected(selected)
        .focusable(true)
        .grow(1.0)
        .min_w(0.0)
        .gap(4.0)
        .px(10.0)
        .py(8.0)
        .radius(4.0)
        .fill(if selected { t.selection } else { t.panel_bg })
        .border(1.0, if selected { t.accent } else { t.border })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            let _ = select_hw_stat(s, name);
        })
        .child(
            row()
                .w_full()
                .items_center()
                .gap(6.0)
                .child(
                    text(name)
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(t.text)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(cad::badge(
                    if row_.value { "1 HIGH" } else { "0 LOW" },
                    ink,
                    t,
                )),
        )
        .child(cad::cut(
            row_.description(),
            VivadoTheme::FONT_UI,
            t.text_dim,
        ))
}

fn stat_section(s: &AppState, t: &VivadoTheme) -> El {
    let mut head = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::title("Configuration Status Register (STAT)", t));
    let mut section = col().key("program-stat").w_full().gap(6.0);
    let Some(stat) = &s.program.stat else {
        section = section.child(head);
        let body = if let Some(note) = &s.program.stat_note {
            cad::empty_state("program-stat-empty", note, None, t)
        } else if stat_block_reason(s).is_some() && s.program.busy().is_none() {
            cad::empty_state(
                "program-stat-empty",
                "STAT readback runs on the Simulation cable; openFPGALoader has no TAP readback.",
                Some(cad::btn(
                    "program-stat-use-sim",
                    "Use Simulation cable",
                    t,
                    None,
                    |s: &mut AppState| s.program.cable = CableChoice::Sim,
                )),
                t,
            )
        } else {
            cad::empty_state(
                "program-stat-empty",
                "STAT not read yet.",
                Some(cad::btn(
                    "program-stat-read",
                    "Refresh STAT",
                    t,
                    stat_block_reason(s).as_deref(),
                    refresh_stat,
                )),
                t,
            )
        };
        return section.child(body);
    };
    head = head
        .child(el().grow(1.0))
        .child(cad::badge(stat.source.label(), t.text_dim, t).key("program-stat-source"))
        .child(cad::mono(format!("word {:#010x}", stat.word), t).key("program-stat-word"));
    section = section.child(head);
    let mut cards: Vec<El> = stat.bits.iter().map(|b| stat_card(*b, s, t)).collect();
    cards.push(
        col()
            .key("program-stat-idcode")
            .grow(1.0)
            .min_w(0.0)
            .gap(4.0)
            .px(10.0)
            .py(8.0)
            .radius(4.0)
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .child(
                        text("IDCODE")
                            .font(cad::mono_style())
                            .color(t.text)
                            .grow(1.0),
                    )
                    .child(cad::mono(format!("{:#010x}", stat.idcode), t)),
            )
            .child(cad::cut(
                format!("{} · IR {:#04x}", stat.part, stat.ir),
                VivadoTheme::FONT_UI,
                t.text_dim,
            )),
    );
    let mut grid = col().w_full().gap(6.0);
    let mut it = cards.into_iter().peekable();
    let mut r = 0;
    while it.peek().is_some() {
        let mut line = row().key(format!("program-stat-row-{r}")).w_full().gap(6.0);
        for _ in 0..STAT_COLS {
            line = line.child(it.next().unwrap_or_else(|| el().grow(1.0).min_w(0.0)));
        }
        grid = grid.child(line);
        r += 1;
    }
    section = section.child(grid);
    if let Some(sel) = s.program.stat_selected.and_then(|n| stat.bit(n)) {
        section = section.child(
            cad::cut(
                format!(
                    "{} · bit {} · value {} · {}",
                    sel.name,
                    sel.bit,
                    u8::from(sel.value),
                    sel.description()
                ),
                VivadoTheme::FONT_UI,
                t.text,
            )
            .key("program-stat-detail"),
        );
    }
    section
}

/// Server node: this process's hardware layer and the last USB scan.
fn server_panel(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .w_full()
        .gap(0.0)
        .child(cad::kv(
            "Server",
            cad::mono("localhost · in-process helion-hw (no hw_server daemon)", t),
            LABEL_W + 30.0,
            t,
        ));
    match &s.program.detect {
        None => {
            body = body.child(cad::kv(
                "USB scan",
                cad::dim("Not scanned — Detect lists USB programmers.", t),
                LABEL_W + 30.0,
                t,
            ));
        }
        Some(d) => {
            let ofl = d
                .usb
                .ofl_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not on PATH".into());
            body = body
                .child(cad::kv(
                    "openFPGALoader",
                    cad::cut(ofl, cad::mono_style(), t.text),
                    LABEL_W + 30.0,
                    t,
                ))
                .child(cad::kv(
                    "USB probes",
                    cad::mono(d.usb.probes.len().to_string(), t),
                    LABEL_W + 30.0,
                    t,
                ))
                .child(cad::kv(
                    "Native FTDI",
                    cad::cut(d.usb.native_note.clone(), VivadoTheme::FONT_UI, t.text_dim),
                    LABEL_W + 30.0,
                    t,
                ))
                .child(cad::kv(
                    "Note",
                    cad::cut(d.note.clone(), VivadoTheme::FONT_UI, t.text_dim),
                    LABEL_W + 30.0,
                    t,
                ));
            for (i, p) in d.usb.probes.iter().enumerate() {
                body = body.child(
                    cad::cut(p.detail.clone(), cad::mono_style(), t.text)
                        .key(format!("program-usb-probe-{i}")),
                );
            }
        }
    }
    col()
        .key("program-server")
        .w_full()
        .gap(10.0)
        .child(cad::card("program-server-card", "Hardware Server", body, t))
}

/// Target node: cable pick and the JTAG chain it reaches.
fn target_panel(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("program-target-panel")
        .w_full()
        .gap(10.0)
        .child(cable_card(s, t))
        .child(crate::views::hw_jtag_view::chain_card(s, t))
}

/// Device node: the original Program Device surface.
fn device_panel(s: &AppState, t: &VivadoTheme) -> El {
    let left = col()
        .w(LEFT_W)
        .gap(10.0)
        .child(cable_card(s, t))
        .child(bitstream_card(s, t));
    let right = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(10.0)
        .child(program_card(s, t));
    col()
        .w_full()
        .gap(10.0)
        .child(row().w_full().gap(10.0).child(left).child(right))
        .child(stat_section(s, t))
}

/// The center Hardware Manager pane wired into the shell: the hardware
/// tree on the left drives which panel shows on the right.
pub fn program_pane(s: &AppState, t: &VivadoTheme) -> El {
    use crate::views::hw_tree::HwNode;
    let busy = s.program.busy();
    let detect_reason = busy.map(|k| format!("Busy: {}", k.busy_label()));
    let stat_reason = stat_block_reason(s);
    let mut tools = cad::toolbar("program-toolbar", t)
        .child(cad::btn(
            "program-detect",
            "Detect",
            t,
            detect_reason.as_deref(),
            detect,
        ))
        .child(cad::btn(
            "program-refresh-stat",
            "Refresh STAT",
            t,
            stat_reason.as_deref(),
            refresh_stat,
        ))
        .child(cad::tool_sep(t))
        .child(cad::btn(
            "program-setup-debug",
            "Set Up Debug",
            t,
            None,
            |s: &mut AppState| s.program.node = HwNode::SetupDebug,
        ))
        .child(cad::btn(
            "program-add-cfgmem",
            "Add Configuration Memory",
            t,
            None,
            |s: &mut AppState| s.program.node = HwNode::CfgMem,
        ))
        .child(el().grow(1.0));
    if let Some(k) = busy {
        tools = tools.child(cad::dim(k.busy_label(), t).key("program-busy"));
    } else if let Some(k) = s.program.debug.busy() {
        tools = tools.child(cad::dim(k.busy_label(), t).key("program-busy"));
    } else if let Some(op) = s.program.cfgmem.busy() {
        tools = tools.child(cad::dim(op.busy_label(), t).key("program-busy"));
    }
    let panel = match s.program.node {
        HwNode::Server => server_panel(s, t),
        HwNode::Target => target_panel(s, t),
        HwNode::Device => device_panel(s, t),
        HwNode::SetupDebug => crate::views::hw_debug_view::setup_panel(s, t),
        HwNode::Core(helion_debug::DebugCoreKind::Vio) => crate::views::hw_debug_view::vio_panel(s, t),
        HwNode::Core(helion_debug::DebugCoreKind::JtagAxi) => {
            crate::views::hw_debug_view::axi_panel(s, t)
        }
        HwNode::Core(_) => crate::views::hw_debug_view::ila_panel(s, t),
        HwNode::CfgMem => crate::views::hw_cfgmem_view::cfgmem_panel(s, t),
    };
    col()
        .key("program")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Hardware Manager", "program", t))
        .child(tools)
        .child(
            row()
                .key("program-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(crate::views::hw_tree::tree(s, t))
                .child(
                    col()
                        .key("program-scroll")
                        .grow(1.0)
                        .min_w(0.0)
                        .h_full()
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(
                            col()
                                .w_full()
                                .gap(10.0)
                                .padding(8.0)
                                .child(banner(s, t))
                                .child(panel),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};
    use std::time::Duration;

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn program_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Program;
        s
    }

    fn with_bits(s: &mut AppState) {
        crate::views::bitstream::seed_bitstream_fixture(s).expect("fixture bitstream");
    }

    #[test]
    fn no_fake_hardware_in_the_default_pane() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        for key in [
            "program",
            "program-toolbar",
            "program-detect",
            "program-refresh-stat",
            "program-banner",
            "program-cable-opt-0",
            "program-cable-opt-3",
            "program-bitstream-path",
            "program-browse",
            "program-generate",
            "program-program",
            "program-blocked",
            "program-stat-empty",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert!(rt.find("program-results").is_none());
        let tree = format!("{:?}", rt.state.program);
        for fake in ["localhost:3121", "000012345678", "HS3", "xc7z007s"] {
            assert!(!tree.contains(fake), "fake hardware leaked: {fake}");
        }
        // Without a bitstream, Program refuses with a reason (no handler).
        tap(&mut rt, "program-program");
        assert_eq!(rt.state.program.phase, ProgramPhase::Idle);
        assert!(rt.state.program.busy().is_none());
    }

    #[test]
    fn no_usb_refuses_board_cables_honestly() {
        let mut s = program_state();
        with_bits(&mut s);
        s.program.detect = Some(no_usb_fixture());
        let why = program_block_reason(&s).expect("auto without USB must block");
        assert!(why.contains("No USB programmer"), "{why}");
        s.program.cable = CableChoice::Sim;
        assert_eq!(program_block_reason(&s), None, "sim needs no USB");
        let mut rt = harness_with(s);
        rt.frame(0.0);
        let title = rt.state.program.detect.as_ref().map(scan_line).unwrap();
        assert!(title.contains("USB=0"), "{title}");
        assert!(rt.find("program-blocked").is_none());
    }

    #[test]
    fn detect_runs_off_thread_and_reports_the_scan() {
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-detect");
        assert_eq!(rt.state.program.busy(), Some(HwJobKind::Detect));
        assert!(wait_idle(&mut rt.state, Duration::from_secs(60)));
        let d = rt.state.program.detect.as_ref().expect("scan result");
        assert_eq!(d.physical_had, !d.usb.probes.is_empty());
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("detect USB=")),
            "scan line must reach the console"
        );
    }

    #[test]
    fn sim_program_runs_the_real_session_path_with_stat() {
        let mut s = program_state();
        with_bits(&mut s);
        s.program.cable = CableChoice::Sim;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "program-program");
        assert_eq!(rt.state.program.phase, ProgramPhase::Programming);
        assert!(wait_idle(&mut rt.state, Duration::from_secs(60)));
        // The worker mutated state outside a handler: rebuild the view.
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.program.phase, ProgramPhase::Done);
        assert_eq!(rt.state.program.stage, STAGES);
        let res = rt.state.program.result.clone().expect("result");
        assert!(res.ok, "{}", res.line);
        assert_eq!(res.backend, "sim");
        assert!(res.line.contains("not board DONE"), "{}", res.line);
        let stat = rt.state.program.stat.clone().expect("sim STAT");
        assert_eq!(stat.source, StatSource::SimProgrammed);
        assert_eq!(stat.bits.len(), 7);
        assert!(res.verdict(Some(&stat)).contains("not a physical board"));
        for key in [
            "program-results",
            "program-verdict",
            "program-stat-done",
            "program-stat-idcode",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        // Let the freshly mounted STAT cards settle before hit-testing.
        for i in 1..=30 {
            rt.frame(0.1 + i as f64 / 60.0);
        }
        tap(&mut rt, "program-stat-done");
        assert_eq!(rt.state.program.stat_selected, Some("DONE"));
        assert!(rt.find("program-stat-detail").is_some());
    }

    #[test]
    fn bitstream_browse_consumes_the_picked_file() {
        use rune::FileDialogKind;
        let mut rt = harness_with(program_state());
        rt.frame(0.0);
        tap(&mut rt, "program-browse");
        let (kind, opts) = rt
            .file_dialog()
            .expect("Browse must raise a native file dialog");
        assert_eq!(kind, FileDialogKind::Open);
        assert!(
            opts.filters
                .iter()
                .any(|(n, exts)| n == "Bitstream" && exts.contains(&"hbits".to_string())),
            "filter must accept .hbits: {:?}",
            opts.filters
        );
        let picked =
            std::env::temp_dir().join(format!("helion-pick-{}-top.hbits", std::process::id()));
        rt.resolve_file_dialog(Some(picked.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.program.file.as_ref(), Some(&picked));
        assert!(rt.find("program-use-run").is_some());
        tap(&mut rt, "program-use-run");
        assert!(rt.state.program.file.is_none());
    }
}
