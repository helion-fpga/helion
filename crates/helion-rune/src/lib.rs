//! Helion Vivado-style IDE shell on Rune, built from scratch.
//!
//! All views here are new Rune `El` code. `helion-gui` is read for domain
//! data shapes only (`IdeModel`/tcl/sta/vcd types mirrored in
//! [`state`]); no `helion-gui` paint code is copied or ported.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/tooling.rs`: [`rune::AppSpec`] (`title/size/init/view/
//!   scenes`), `--snap/--tree/--scene` headless flags.
//! - `examples/hello/src/main.rs`: hello `State`/`view`/`Runtime` tap pattern.
//! - `crates/rune/src/el.rs` + `el/layout.rs|visuals.rs|handlers.rs|text.rs`:
//!   `row()/col()/el()/text()`, `.key/.fill/.border/.on_press/...`.
//! - `crates/rune/src/tok.rs`: `motion::SNAPPY`, `TextStyle::new`.
//! - `crates/rune-kit/src/palette.rs`: `Kit::from_palette(PaletteSeed)`.
//! - `crates/rune/src/color.rs`: `color::hex` (used ONLY in [`theme`]).

pub mod doctor;
pub mod live;
pub mod pickers;
pub mod runs;
pub mod scenes;
pub mod state;
pub mod tcl;
pub mod theme;
pub mod views;

pub use state::AppState;
pub use theme::{ThemeMode, VivadoTheme};

use rune::{AppSpec, Cx, El, col, el, row, text};

const TITLE: &str = "project_1 - Helion";
const SIZE: (f32, f32) = (1440.0, 900.0);

/// Named headless states (`--scene NAME`; see `tooling.rs::AppSpec::scenes`).
/// Per-surface scenes arrive with their view batches; the scaffold ships the
/// hello pair so `--snap/--tree` work from day one.
pub const SCENES: &[(&str, fn(&mut AppState))] = &[
    ("hello", |_| {}),
    ("on", |s| s.hello_on = true),
    ("chrome", |_| {}),
    ("chrome-file-menu", |s| {
        s.chrome.open_menu = Some(views::chrome::MenuId::File);
    }),
    ("chrome-synth-running", |s| {
        s.flow.synth = state::RunStatus::Running;
    }),
    ("sources", |_| {}),
    ("sources-xdc", |s| {
        s.sources.selected = views::sources::SourceFileId::Xdc;
    }),
    ("sources-libraries", |s| {
        s.sources.tab = views::sources::SourcesTab::Libraries;
    }),
    ("editor", |s| s.center = state::CenterPane::Editor),
    ("editor-xdc", |s| {
        s.center = state::CenterPane::Editor;
        s.editor.active = 1;
    }),
    ("editor-find", |s| {
        s.center = state::CenterPane::Editor;
        s.editor.find_open = true;
        s.editor.find_query = rune::TextEdit::new("divclk");
    }),
    ("editor-tb", |s| {
        s.center = state::CenterPane::Editor;
        s.editor.tabs.push("project1_tb.v".into());
        s.editor.active = 2;
    }),
    ("console", |_| {}),
    ("console-tcl", |s| {
        crate::tcl::eval_command(s, "set part xc7z007s");
        crate::tcl::eval_command(s, "expr {7 * 6}");
    }),
    ("console-messages", |s| {
        s.console.tab = views::console::ConsoleTab::Messages;
    }),
    ("console-log", |s| {
        s.console.tab = views::console::ConsoleTab::Log;
        s.flow.synth = state::RunStatus::Complete;
    }),
    ("console-log-impl", |s| {
        s.console.tab = views::console::ConsoleTab::Log;
        s.console.log_run = views::console::LogRun::Implementation;
        s.flow.impl_run = state::RunStatus::Running;
    }),
    ("console-reports", |s| {
        s.console.tab = views::console::ConsoleTab::Reports;
    }),
    ("console-runs", |s| {
        s.console.tab = views::console::ConsoleTab::DesignRuns;
        s.flow.synth = state::RunStatus::Complete;
    }),
    ("summary", |_| {}),
    ("summary-synth-complete", |s| {
        s.flow.synth = state::RunStatus::Complete;
        s.status = state::StatusVerb::SynthesisComplete;
    }),
    ("timing", |s| {
        s.center = state::CenterPane::Timing;
        s.flow.synth = state::RunStatus::Complete;
        s.flow.impl_run = state::RunStatus::Complete;
        s.status = state::StatusVerb::ImplementationComplete;
        s.timing = state::TimingState {
            wns_ps: Some(2061),
            tns_ps: 0,
            whs_ps: Some(-1545),
            ths_ps: -3069,
            failing_endpoints: 2,
            failing_setup: 0,
            failing_hold: 2,
            endpoints: 112,
            has_run: true,
            ..state::TimingState::default()
        };
        seed_timing_fixture(s);
    }),
    ("timing-met", |s| {
        s.center = state::CenterPane::Timing;
        s.timing = state::TimingState {
            wns_ps: Some(412),
            tns_ps: 0,
            whs_ps: Some(96),
            ths_ps: 0,
            failing_endpoints: 0,
            failing_setup: 0,
            failing_hold: 0,
            endpoints: 112,
            has_run: true,
            ..state::TimingState::default()
        };
        seed_timing_fixture(s);
    }),
    ("timing-paths", |s| {
        s.center = state::CenterPane::Timing;
        s.timing_view.selected = views::timing::TimingNav::IntraClock;
        s.timing = state::TimingState {
            wns_ps: Some(2061),
            tns_ps: 0,
            whs_ps: Some(-1545),
            ths_ps: -3069,
            failing_endpoints: 2,
            failing_setup: 0,
            failing_hold: 2,
            endpoints: 112,
            has_run: true,
            ..state::TimingState::default()
        };
        seed_timing_fixture(s);
    }),
    ("timing-no-run", |s| {
        s.center = state::CenterPane::Timing;
    }),
    ("settings", |s| {
        s.dialogs.open(views::dialogs::DialogId::Settings);
    }),
    ("settings-implementation", |s| {
        s.dialogs.open(views::dialogs::DialogId::Settings);
        s.dialogs.settings.page = views::dialogs::SettingsPage::Implementation;
    }),
    ("settings-pop", |s| {
        s.dialogs.open(views::dialogs::DialogId::Settings);
        s.dialogs.pop = Some(views::dialogs::PopId::SynthStrategy);
    }),
    ("create-project", |s| {
        s.dialogs.open(views::dialogs::DialogId::NewProject);
        s.dialogs.create.name = rune::TextEdit::new("project_2");
    }),
    ("create-part", |s| {
        s.dialogs.open(views::dialogs::DialogId::NewProject);
        s.dialogs.create.name = rune::TextEdit::new("project_2");
        s.dialogs.create.step = views::dialogs::CreateStep::Part;
        s.dialogs.pop = Some(views::dialogs::PopId::CreatePart);
    }),
    ("constraints-wizard", |s| {
        s.dialogs.open(views::dialogs::DialogId::Constraints);
    }),
    ("constraints-review", |s| {
        s.dialogs.open(views::dialogs::DialogId::Constraints);
        s.dialogs.xdc.step = views::dialogs::XdcStep::Review;
    }),
    ("add-sources", |s| {
        s.dialogs.open(views::dialogs::DialogId::AddSources);
    }),
    ("start", |s| {
        s.center = state::CenterPane::Start;
        s.empty_project();
    }),
    ("start-toast", |s| {
        s.center = state::CenterPane::Start;
        s.empty_project();
        s.toasts.push(
            state::ToastKind::Info,
            "Welcome to Helion",
            "Create a project to get started",
        );
    }),
    ("status-synth-complete", |s| {
        s.flow.synth = state::RunStatus::Complete;
        s.status = state::StatusVerb::SynthesisComplete;
        s.timing = state::TimingState {
            wns_ps: Some(2061),
            tns_ps: 0,
            whs_ps: Some(-1545),
            ths_ps: -3069,
            failing_endpoints: 2,
            failing_setup: 0,
            failing_hold: 2,
            endpoints: 112,
            has_run: true,
            ..state::TimingState::default()
        };
        s.toasts.push(
            state::ToastKind::Success,
            "Synthesis complete",
            "synth_1 finished with 0 Errors",
        );
    }),
    ("status-impl-failed", |s| {
        s.flow.synth = state::RunStatus::Complete;
        s.flow.impl_run = state::RunStatus::Failed;
        s.status = state::StatusVerb::Failed;
        s.timing = state::TimingState {
            wns_ps: Some(-312),
            tns_ps: -1240,
            whs_ps: Some(96),
            ths_ps: 0,
            failing_endpoints: 4,
            failing_setup: 4,
            failing_hold: 0,
            endpoints: 112,
            has_run: true,
            ..state::TimingState::default()
        };
        s.console.lines.push(state::ConsoleLine {
            text: "ERROR: [Common 17-69] Command failed".into(),
            sev: state::MsgSev::Error,
        });
        s.toasts.push(
            state::ToastKind::Error,
            "Implementation failed",
            "impl_1 failed with 1 Error",
        );
    }),
    ("summary-impl-complete", |s| {
        views::reports::demo_impl_complete(s);
    }),
    ("schematic", |s| {
        s.center = state::CenterPane::Schematic;
        let _ = live::seed_live_fixture(s);
    }),
    ("schematic-detail", |s| {
        s.center = state::CenterPane::Schematic;
        let _ = live::seed_live_fixture(s);
        s.schematic.zoom = 1.5;
        s.schematic.selected = Some(views::schematic::SchemSel::Cell("u_div_u_ff0".into()));
        s.schematic.collapsed.insert("u_led".into());
    }),
    ("device", |s| {
        s.center = state::CenterPane::Device;
        let _ = live::seed_live_fixture(s);
    }),
    ("device-routed", |s| {
        s.center = state::CenterPane::Device;
        let _ = live::seed_live_fixture(s);
        s.device.layers.nets = true;
        // The real 32-row die dwarfs the viewport: frame the whole die so
        // the spans read end to end.
        s.device.zoom = 0.5;
        s.device.selected = Some(views::device::DeviceSel::Net("clk".into()));
    }),
    ("device-package", |s| {
        s.center = state::CenterPane::Device;
        let _ = live::seed_live_fixture(s);
        s.device.view = views::device::DeviceView::Package;
    }),
    ("wave", |s| {
        s.center = state::CenterPane::Wave;
        views::wave_demo::seed_demo_wave(s);
    }),
    ("wave-empty", |s| {
        s.center = state::CenterPane::Wave;
    }),
    ("wave-cursors", |s| {
        s.center = state::CenterPane::Wave;
        views::wave_demo::seed_demo_wave(s);
        s.wave.cursor = 48;
        s.wave.cursor_a = Some(16);
        s.wave.cursor_b = Some(64);
        s.wave.markers = vec![
            views::wave::WaveMarker::new("M0", 24),
            views::wave::WaveMarker::new("M1", 72),
        ];
        s.wave.selected = Some(views::wave::WaveSigId::VbLed);
    }),
    ("wave-live", |s| {
        s.center = state::CenterPane::Wave;
        // Real engine import (synth + helion-sim kernel + VCD import) on an
        // inline counter so the snapshot exercises the live lanes. Two short
        // cycles keep the sheet inside the shell for the layout harness;
        // full-period runs are covered by the wave unit tests.
        let src = "module counter (input logic clk, output logic led);\n\
            logic [3:0] cnt;\n\
            always_ff @(posedge clk) begin cnt <= cnt + 1; end\n\
            assign led = cnt[3];\nendmodule\n";
        if let Ok((mut live, _)) = views::wave::run_engine_vcd(src, "counter.sv", 2, 64) {
            live.source = "counter_tb (2 cycles, helion-sim)".into();
            s.vcd.timescale_ps = live.timescale_ps.max(1);
            s.vcd.duration_ps = live.duration_ps;
            s.vcd.signals = live.traces.iter().map(|t| t.name.clone()).collect();
            s.wave.mount_live(live);
            s.wave.live_selected = Some(0);
            s.wave.cursor = 100;
            s.wave.cursor_a = Some(16);
            s.wave.cursor_b = Some(64);
            s.wave.markers = vec![views::wave::WaveMarker {
                name: "M0".into(),
                sample: 48,
            }];
            s.wave.clamp_to_grid();
        }
    }),
    ("sim", |s| {
        s.center = state::CenterPane::Sim;
        views::tb::seed_examples_fixture(s);
    }),
    ("sim-detail", |s| {
        s.center = state::CenterPane::Sim;
        // Real runs in a temp copy: counter_early_tb fails, counter_tb passes.
        if views::tb::seed_run_fixture(s).is_ok() {
            s.sim.selected = Some(0);
        }
        // The fixture's run toasts would cover the history table.
        s.toasts = state::ToastState::default();
    }),
    ("debug", |s| {
        s.center = state::CenterPane::Debug;
        let _ = views::debug::seed_debug_fixture(s);
    }),
    ("debug-breakpoints", |s| {
        s.center = state::CenterPane::Debug;
        if views::debug::seed_debug_fixture(s).is_ok() {
            crate::tcl::eval_command(s, "add_bp led 1");
        }
    }),
    ("debug-memory", |s| {
        s.center = state::CenterPane::Debug;
        if views::debug::seed_debug_fixture(s).is_ok() {
            crate::tcl::eval_command(s, "memory");
            crate::tcl::eval_command(s, "select_memory seq");
            crate::tcl::eval_command(s, "select_memory_word 0");
        }
    }),
    ("debug-locals", |s| {
        s.center = state::CenterPane::Debug;
        if views::debug::seed_debug_fixture(s).is_ok() {
            crate::tcl::eval_command(s, "locals");
            crate::tcl::eval_command(s, "select_local led");
        }
    }),
    ("debug-forces", |s| {
        s.center = state::CenterPane::Debug;
        if views::debug::seed_debug_fixture(s).is_ok() {
            crate::tcl::eval_command(s, "add_force led 1");
        }
    }),
    ("program", |s| {
        s.center = state::CenterPane::Program;
        s.flow.impl_run = state::RunStatus::Complete;
        s.status = state::StatusVerb::ImplementationComplete;
    }),
    ("program-done", |s| {
        s.center = state::CenterPane::Program;
        s.flow.impl_run = state::RunStatus::Complete;
        s.status = state::StatusVerb::ImplementationComplete;
        s.program.connected = true;
        s.program.phase = views::program::ProgramPhase::Done;
        s.program.progress = 100;
        let detail = s.program.done_detail();
        s.toasts
            .push(state::ToastKind::Success, "Device programmed", &detail);
    }),
    ("reports", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
    }),
    ("reports-power", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        s.reports_view.selected = views::reports::ReportNav::Power;
    }),
    ("reports-drc", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        s.reports_view.selected = views::reports::ReportNav::Drc;
        s.drc.items = vec![
            state::DrcItem {
                id: "PDRC-190".into(),
                severity: state::DrcSev::Warning,
                objects: "clk_pin".into(),
                message: "High fanout net: 50 loads".into(),
            },
            state::DrcItem {
                id: "REQP-144".into(),
                severity: state::DrcSev::Advisory,
                objects: "led_ctl0/led_o_reg[5]".into(),
                message: "Control set has no asynchronous reset".into(),
            },
        ];
    }),
    ("reports-no-run", |s| {
        s.center = state::CenterPane::Reports;
    }),
    ("palette-files", |s| {
        views::palette::open_palette(s, views::palette::PaletteMode::Files);
    }),
    ("palette-commands", |s| {
        views::palette::open_palette(s, views::palette::PaletteMode::Commands);
        s.palette.query = rune::TextEdit::new("run");
    }),
    ("cheatsheet", |s| {
        views::palette::open_cheatsheet(s);
    }),
];

/// Representative timing report output the timing scenes share (single
/// 10ns clock, one intra group, worst path first with UG903 pin rows),
/// shaped like real `TimingState` so snapshots render tables.
fn seed_timing_fixture(s: &mut AppState) {
    s.timing.clocks = vec![state::TimingClock {
        name: "clk".into(),
        period_ps: 10_000,
        source: "clk".into(),
        generated: false,
        master: None,
    }];
    s.timing.groups = vec![state::TimingGroupRow {
        name: "clk".into(),
        from: "clk".into(),
        to: "clk".into(),
        kind: state::TimingGroupKind::Intra,
        wns_ps: s.timing.wns_ps,
        tns_ps: s.timing.tns_ps,
        endpoints: s.timing.endpoints,
    }];
    s.timing.paths = vec![state::TimingPathRow {
        name: "cnt_3->cnt_7".into(),
        from: "cnt_3".into(),
        to: "cnt_7".into(),
        slack_ps: s.timing.wns_ps.unwrap_or(0),
        delay_ps: 7939,
        group: state::TimingPathGroup::Register,
        pins: vec![
            state::TimingPinRow {
                cell: "cnt_3".into(),
                pin: "cnt_3/CLK".into(),
                delay_type: "clk".into(),
                incr_ps: 0,
                path_ps: 0,
                net: "clk".into(),
                fanout: 8,
                location: "CLB_X1Y2".into(),
            },
            state::TimingPinRow {
                cell: "cnt_3".into(),
                pin: "cnt_3/Q".into(),
                delay_type: "cell".into(),
                incr_ps: 80,
                path_ps: 80,
                net: "cnt_q".into(),
                fanout: 2,
                location: "CLB_X1Y2".into(),
            },
            state::TimingPinRow {
                cell: "cnt_7".into(),
                pin: "cnt_7/D".into(),
                delay_type: "setup".into(),
                incr_ps: 50,
                path_ps: 7939,
                net: "cnt_d".into(),
                fanout: 1,
                location: "CLB_X1Y3".into(),
            },
        ],
    }];
    s.timing.clock_networks = vec![state::ClockNetRow {
        name: "clk".into(),
        period_ps: 10_000,
        net: "clk".into(),
        loads: 8,
        buffers: 1,
        insertion_ps: 120,
    }];
    s.timing.design = "project1_demo".into();
    s.timing.voltage_mv = 1000;
    s.timing.temp_c = 25;
}

pub fn init() -> AppState {
    AppState::demo()
}

fn menu_bar(s: &AppState, t: &VivadoTheme) -> El {
    views::chrome::menu_bar(s, t)
}

fn flow_rail(s: &AppState, t: &VivadoTheme) -> El {
    views::chrome::flow_rail(s, t)
}

fn sources_column(s: &AppState, t: &VivadoTheme) -> El {
    views::sources::sources_column(s, t)
}

fn center_pane(s: &AppState, t: &VivadoTheme) -> El {
    match s.center {
        state::CenterPane::Summary => views::summary::summary_pane(s, t),
        state::CenterPane::Editor => views::editor::editor_pane(s, t),
        state::CenterPane::Timing => views::timing::timing_pane(s, t),
        state::CenterPane::Start => views::start::start_pane(s, t),
        state::CenterPane::Schematic => views::schematic::schematic_pane(s, t),
        state::CenterPane::Device => views::device::device_pane(s, t),
        state::CenterPane::Wave => views::wave::wave_pane(s, t),
        state::CenterPane::Sim => views::tb_view::sim_pane(s, t),
        state::CenterPane::Debug => views::debug::debug_pane(s, t),
        state::CenterPane::Program => views::program::program_pane(s, t),
        state::CenterPane::Reports => views::reports::reports_pane(s, t),
    }
}

fn status_bar(s: &AppState, t: &VivadoTheme) -> El {
    views::status::status_bar(s, t)
}

/// Root view: Vivado shell skeleton (titlebar, menu, rail, summary, status).
/// Full surfaces arrive per batch; this proves theme + state + scenes render.
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let t = s.theme.resolve(cx.system_dark());
    let title = if s.project.name.is_empty() {
        "Helion".to_string()
    } else {
        format!("{} - Helion", s.project.name)
    };
    let mut shell = col()
        .key("shell")
        .w_full()
        .h_full()
        .fill(t.chrome_bg)
        .child(
            el().key("titlebar")
                .w_full()
                .h(28.0)
                .pl(cx.titlebar_inset() + 10.0)
                .pr(10.0)
                .center_y()
                .fill(t.titlebar_bg)
                .child(
                    text(title)
                        .key("titlebar-title")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                ),
        )
        .child(menu_bar(s, &t))
        .child(views::chrome::icon_toolbar(s, &t))
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(flow_rail(s, &t))
                .child(sources_column(s, &t))
                .child(center_pane(s, &t)),
        )
        .child(
            el().w_full()
                .px(6.0)
                .pb(6.0)
                .child(views::console::console_pane(s, &t)),
        )
        .child(status_bar(s, &t));
    if let Some(overlay) = views::chrome::menu_overlay(s, &t) {
        shell = shell.child(overlay);
    }
    let mut layer = Vec::new();
    views::dialogs::push_dialogs(&mut layer, s, &t);
    views::device::push_device_pops(&mut layer, s, &t);
    views::status::push_toasts(&mut layer, s, &t);
    views::palette::push_palette(&mut layer, s, &t);
    views::palette::push_cheatsheet(&mut layer, s, &t);
    if layer.is_empty() {
        shell
    } else {
        rune_kit::root(shell, layer)
    }
}

/// `AppSpec::poll` wiring: pump the synth/impl run channel and the sim
/// channel every frame so both workers stream without blocking the UI.
pub fn poll_all(s: &mut AppState, t: f64, tx: &mut rune::Tx) -> bool {
    let runs_dirty = runs::poll(s, t, tx);
    let sim_dirty = views::tb::poll(s, t, tx);
    // Safety net: out-of-band buffer swaps (project reseed clears tabs)
    // re-mirror the live edit field; a no-op when already synced.
    views::editor::sync_edit(s);
    runs_dirty || sim_dirty
}

pub fn app_spec() -> AppSpec<AppState> {
    AppSpec::<AppState> {
        scenes: SCENES,
        min_size: Some((900.0, 600.0)),
        poll: Some(poll_all),
        on_key: Some(views::palette::app_key),
        ..AppSpec::new(TITLE, SIZE, init, view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    fn harness() -> Runtime<AppState> {
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(init(), view, SIZE, env)
    }

    #[test]
    fn shell_tree_exposes_the_vivado_regions() {
        let mut rt = harness();
        rt.frame(0.0);
        for key in [
            "shell",
            "titlebar",
            "menubar",
            "toolbar",
            "flow-rail",
            "sources-column",
            "summary",
            "summary-settings",
            "summary-synth",
            "summary-impl",
            "statusbar",
        ] {
            assert!(rt.find(key).is_some(), "missing keyed region: {key}");
        }
    }

    #[test]
    fn titlebar_title_clears_the_traffic_lights() {
        let mut rt = harness();
        rt.set_titlebar_inset(67.0);
        rt.frame(0.0);
        let x = rt.presented("titlebar-title").unwrap().0[0];
        assert_eq!(x, 77.0, "title starts past the lights + 10pt gap");
        rt.set_titlebar_inset(0.0);
        // Padding changes animate: settle before asserting the rest position.
        for i in 0..40 {
            rt.frame(0.1 + i as f64 * (1.0 / 60.0));
        }
        let x = rt.presented("titlebar-title").unwrap().0[0];
        assert!(
            (x - 10.0).abs() < 0.5,
            "fullscreen keeps the plain 10pt gap, got {x}"
        );
    }
}
