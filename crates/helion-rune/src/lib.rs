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

pub mod analysis;
pub mod bg;
pub mod cli;
pub mod showcase;
pub mod doctor;
pub mod ext_verbs;
pub mod eco_tcl;
pub mod jobs;
pub mod layout;
pub mod live;
pub mod pickers;
pub mod project;
pub mod runs;
pub mod runs_family;
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

/// Scene helper: mount the named leaf traces (in order) as the lanes.
fn scene_lanes(s: &mut AppState, leaves: &[&str]) {
    let Some(live) = s.wave.live.as_ref() else {
        return;
    };
    s.wave.live_lanes = leaves
        .iter()
        .filter_map(|l| live.traces.iter().position(|t| t.leaf() == *l))
        .collect();
    s.wave.lanes_custom = true;
}

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
    ("editor-outline", |s| {
        s.center = state::CenterPane::Editor;
        s.editor.outline_open = true;
        views::editor::sync_edit(s);
        views::editor::move_cursor_to(s, 26);
    }),
    ("editor-diagnostics", |s| scenes::editor_diagnostics(s)),
    ("editor-large", |s| scenes::editor_large(s)),
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
    ("runs-table", |s| {
        seed_runs_fixture(s);
    }),
    ("runs-create", |s| {
        seed_runs_fixture(s);
        s.runs.ui.create_open = true;
        s.runs.ui.name = rune::TextEdit::new("sweep");
        s.runs.ui.seeds = rune::TextEdit::new("4, 5, 6");
        s.runs.ui.preset = 1;
        s.runs.ui.preset_pop = true;
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
    ("timing-interaction", |s| {
        seed_two_clock_fixture(s);
        s.timing_view.selected = views::timing::TimingNav::ClockInteraction;
        s.timing_view.selected_interaction = Some(1);
    }),
    ("timing-inter", |s| {
        seed_two_clock_fixture(s);
        s.timing_view.selected = views::timing::TimingNav::InterClock;
    }),
    ("timing-clocks", |s| {
        seed_two_clock_fixture(s);
        s.timing_view.selected = views::timing::TimingNav::ClockSummary;
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
        views::part_picker::pick_board(s, Some("HB1".into()));
    }),
    ("constraints-wizard", |s| {
        s.dialogs.open(views::dialogs::DialogId::Constraints);
    }),
    ("constraints-review", |s| {
        scenes::xdc_wizard_design(s);
        scenes::xdc_wizard_rows(s);
        s.dialogs.open(views::dialogs::DialogId::Constraints);
        s.dialogs.xdc.step = views::xdc_wizard::XdcStep::Review;
        s.dialogs.xdc.visited = 4;
    }),
    ("xdc-wizard-clocks", |s| {
        scenes::xdc_wizard_design(s);
        scenes::xdc_wizard_rows(s);
        s.dialogs.open(views::dialogs::DialogId::Constraints);
    }),
    ("xdc-wizard-delays", |s| {
        scenes::xdc_wizard_design(s);
        scenes::xdc_wizard_rows(s);
        // One row with a real problem renders its error under the row.
        let id = s.dialogs.xdc.inputs[0].id;
        *s.dialogs
            .xdc
            .edit_mut(views::xdc_wizard::Field::InMin, id) = rune::TextEdit::new("3.0");
        s.dialogs.open(views::dialogs::DialogId::Constraints);
        s.dialogs.xdc.step = views::xdc_wizard::XdcStep::InputDelays;
        s.dialogs.xdc.visited = 1;
    }),
    ("xdc-wizard-exceptions", |s| {
        scenes::xdc_wizard_design(s);
        scenes::xdc_wizard_rows(s);
        s.dialogs.open(views::dialogs::DialogId::Constraints);
        s.dialogs.xdc.step = views::xdc_wizard::XdcStep::Exceptions;
        s.dialogs.xdc.visited = 3;
    }),
    ("xdc-wizard-review", |s| scenes::xdc_wizard_review(s)),
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
    ("schematic-cone", |s| {
        s.center = state::CenterPane::Schematic;
        let _ = live::seed_live_fixture(s);
        s.schematic.selected = Some(views::schematic::SchemSel::Cell("u_div_u_ff0".into()));
        crate::tcl::eval_command(s, "expand_fanout_cone u_div_u_ff0 2");
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
    ("device-heat", |s| {
        s.center = state::CenterPane::Device;
        let _ = views::device_layers::seed_heat_fixture(s);
        s.device.layers.criticality = true;
        s.device.layers.rulers = true;
        s.device.zoom = 0.35;
    }),
    ("device-pblocks", |s| {
        s.center = state::CenterPane::Device;
        let _ = live::seed_live_fixture(s);
        crate::tcl::eval_command(s, "create_pblock pblock_div -add CLB_X2Y14:CLB_X5Y18");
        crate::tcl::eval_command(s, "add_cells_to_pblock pblock_div u_div_u_ff0 u_div_u_ff1");
        crate::tcl::eval_command(s, "create_pblock pblock_led");
        s.device.pblock_sel = Some("pblock_div".into());
        // Frame rows 0..18 so the CLB pblock reads with the IOB row.
        s.device.zoom = 0.35;
    }),
    ("device-io-quickset", |s| {
        s.center = state::CenterPane::Device;
        let _ = live::seed_live_fixture(s);
        s.device.io_selected = Some("seg".into());
        if let Some(cell) = views::device::io_port_rows(s)
            .into_iter()
            .find(|r| r.name == "seg")
            .and_then(|r| r.iob_cell)
        {
            s.device.selected = Some(views::device::DeviceSel::Cell(cell));
        }
        crate::tcl::eval_command(s, "set_property IOSTANDARD SSTL15 [get_ports seg]");
        crate::tcl::eval_command(s, "set_property DRIVE 8 [get_ports seg]");
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
    ("wave-decoders", |s| {
        s.center = state::CenterPane::Wave;
        views::wave_demo::seed_protocol_fixture(s);
        let hdl = views::wave_demo::PROTOCOL_FIXTURE_HDL;
        use views::wave_decode::DecoderKind;
        for kind in [DecoderKind::Spi, DecoderKind::Uart, DecoderKind::Fsm, DecoderKind::AxiLite] {
            s.wave.add_decoder(kind, hdl);
        }
        scene_lanes(s, &["sclk", "mosi", "miso", "cs_n", "uart_tx", "state"]);
        s.wave.live_selected = s.wave.lanes().last().copied();
        s.wave.zoom = 0.4;
        s.wave.cursor = 44;
    }),
    ("wave-analog", |s| {
        use views::wave_style::{Analog, Density, TraceDisplay};
        s.center = state::CenterPane::Wave;
        views::wave_demo::seed_protocol_fixture(s);
        scene_lanes(s, &["dac", "saw", "cnt_0", "cnt_1", "cnt_2", "cnt_3"]);
        let lanes = s.wave.lanes();
        s.wave.live_selected = Some(lanes[2]);
        s.wave.style.multi = lanes[3..6].to_vec();
        let _ = s.wave.create_virtual_bus();
        s.wave.style.display.insert(
            "bus_tb.dac".into(),
            TraceDisplay {
                fixed: helion_verif::wave::FixedPoint::parse("Q.6"),
                analog: Some(Analog::Linear),
            },
        );
        s.wave.style.display.insert(
            "bus_tb.saw".into(),
            TraceDisplay {
                fixed: None,
                analog: Some(Analog::Step),
            },
        );
        s.wave.style.density = Density::Compact;
        s.wave.live_selected = Some(lanes[0]);
        s.wave.zoom = 0.4;
        s.wave.cursor = 30;
        s.wave.cursor_a = Some(30);
        s.wave.cursor_b = Some(90);
    }),
    // Real traces: bus_periph_tb through the testbench runner (decoders,
    // analog), counter.sv through the design run (virtual bus of its bits).
    ("wave-real-tools", views::wave_real::seed_real_tools),
    ("wave-real-bus", views::wave_real::seed_real_bus),
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
    ("sim-uvm", |s| {
        s.center = state::CenterPane::Sim;
        // examples/uvm_counter against examples/counter.sv (passes).
        let _ = views::tb_uvm::seed_uvm_fixture(s, false);
        s.toasts = state::ToastState::default();
    }),
    ("sim-uvm-fail", |s| {
        s.center = state::CenterPane::Sim;
        // Same environment against a broken counter (scoreboard errors).
        let _ = views::tb_uvm::seed_uvm_fixture(s, true);
        s.toasts = state::ToastState::default();
    }),
    ("sim-vhdl", |s| {
        s.center = state::CenterPane::Sim;
        // examples/counter_tb.vhd and a failing copy, run in a temp dir.
        let _ = views::tb::seed_vhdl_fixture(s);
        s.toasts = state::ToastState::default();
    }),
    ("ila", |s| {
        s.center = state::CenterPane::Ila;
        let _ = views::ila::seed_ila_fixture(s);
    }),
    ("ila-captured", |s| {
        s.center = state::CenterPane::Ila;
        if views::ila::seed_ila_fixture(s).is_ok() {
            let _ = views::ila::capture_now(s);
        }
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
    }),
    ("program-nousb", |s| {
        // A real implementation-run bitstream plus a scan with no USB
        // programmer: Automatic cable refuses board DONE with the reason.
        s.center = state::CenterPane::Program;
        let _ = views::bitstream::seed_bitstream_fixture(s);
        s.program.detect = Some(views::program::no_usb_fixture());
    }),
    ("program-done", |s| {
        // Real sim-cable program (Session::program_hw_cable) of a real
        // fixture bitstream, applied synchronously for the snapshot.
        s.center = state::CenterPane::Program;
        if views::bitstream::seed_bitstream_fixture(s).is_ok() {
            s.program.detect = Some(views::program::no_usb_fixture());
            s.program.cable = views::program::CableChoice::Sim;
            if let Some((bits, _)) = views::program::bits_source(s) {
                let job = views::program::ProgramJob {
                    cable: "sim".into(),
                    part: s.bitstream.part.clone(),
                    bits,
                };
                let done = views::program::program_blocking(&job, &|_| {});
                views::program::deliver(s, done);
                s.program.stat_selected = Some("DONE");
            }
        }
    }),
    ("hw-setup", |s| {
        // Real fixture design with ILA / VIO / AXI probes picked.
        s.center = state::CenterPane::Program;
        s.program.node = views::hw_tree::HwNode::SetupDebug;
        let _ = views::hw_debug::seed_debug_fixture(s, views::hw_debug::FixtureStage::Picked);
    }),
    ("hw-ila", |s| {
        // Real Set Up Debug build programmed on the sim cable, one value-
        // triggered hardware window uploaded.
        s.center = state::CenterPane::Program;
        if views::hw_debug::seed_debug_fixture(s, views::hw_debug::FixtureStage::Live).is_ok() {
            s.program.node = views::hw_tree::HwNode::Core(helion_debug::DebugCoreKind::Ila);
        }
    }),
    ("hw-ila-wave", |s| {
        if views::hw_debug::seed_debug_fixture(s, views::hw_debug::FixtureStage::Live).is_ok() {
            let _ = views::hw_debug::show_in_viewer(s);
        }
        s.center = state::CenterPane::Ila;
    }),
    ("hw-vio", |s| {
        s.center = state::CenterPane::Program;
        if views::hw_debug::seed_debug_fixture(s, views::hw_debug::FixtureStage::Live).is_ok() {
            s.program.node = views::hw_tree::HwNode::Core(helion_debug::DebugCoreKind::Vio);
        }
    }),
    ("hw-axi", |s| {
        s.center = state::CenterPane::Program;
        if views::hw_debug::seed_debug_fixture(s, views::hw_debug::FixtureStage::Live).is_ok() {
            s.program.node = views::hw_tree::HwNode::Core(helion_debug::DebugCoreKind::JtagAxi);
        }
    }),
    ("hw-cfgmem", |s| {
        // Real fixture bitstream → write_cfgmem → program/verify on the
        // sim cable's SPI flash model.
        s.center = state::CenterPane::Program;
        s.program.cable = views::program::CableChoice::Sim;
        if views::bitstream::seed_bitstream_fixture(s).is_ok() {
            let _ = views::hw_cfgmem::tcl_verb(s, "create_hw_cfgmem", &["mt25ql128".into()]);
            let _ = views::hw_cfgmem::tcl_verb(s, "write_cfgmem", &[]);
            let _ = views::hw_cfgmem::tcl_verb(s, "program_hw_cfgmem", &[]);
        }
        s.toasts = Default::default();
    }),
    ("hw-jtag-native", |s| {
        // Native FTDI cable wired to the fixture board (Helion between two
        // neighbours): real MPSSE chain scan + a USER2 round trip.
        s.center = state::CenterPane::Program;
        s.program.node = views::hw_tree::HwNode::Target;
        let board = views::hw_native::MockBoard::Chain {
            part: "HL10T-C32-1".into(),
        };
        if views::hw_native::use_mock_board(s, board).is_ok() {
            for (verb, args) in [
                ("refresh_hw_target", vec![]),
                ("scan_ir_hw_jtag", vec!["6", "-tdi", "22"]),
                ("scan_dr_hw_jtag", vec!["64", "-tdi", "0x1234ABCD"]),
                ("scan_dr_hw_jtag", vec!["64", "-tdi", "0"]),
            ] {
                let args: Vec<String> = args.into_iter().map(String::from).collect();
                let _ = views::hw_native::tcl_verb(s, verb, &args);
            }
        }
    }),
    ("hw-jtag-native-light", |s| {
        s.theme = theme::ThemeMode::Light;
        s.center = state::CenterPane::Program;
        s.program.node = views::hw_tree::HwNode::Target;
        let board = views::hw_native::MockBoard::Chain {
            part: "HL10T-C32-1".into(),
        };
        if views::hw_native::use_mock_board(s, board).is_ok() {
            let _ = views::hw_native::tcl_verb(s, "refresh_hw_target", &[]);
        }
    }),
    ("hw-jtag-usb", |s| {
        // Native FTDI with no fixture: the real USB path, not yet scanned.
        s.center = state::CenterPane::Program;
        s.program.node = views::hw_tree::HwNode::Target;
        s.program.cable = views::program::CableChoice::Native;
    }),
    ("hw-setup-native", |s| {
        // Set Up Debug with the Native FTDI cable picked (real USB path).
        s.center = state::CenterPane::Program;
        s.program.node = views::hw_tree::HwNode::SetupDebug;
        let _ = views::hw_debug::seed_debug_fixture(s, views::hw_debug::FixtureStage::Picked);
        s.program.cable = views::program::CableChoice::Native;
    }),
    ("hw-ila-native", |s| {
        // The full debug flow over the native engine and the fixture board.
        s.center = state::CenterPane::Program;
        let stage = views::hw_debug::FixtureStage::Live;
        if views::hw_debug::seed_debug_fixture_native(s, stage).is_ok() {
            s.program.node = views::hw_tree::HwNode::Core(helion_debug::DebugCoreKind::Ila);
        }
    }),
    ("hw-ila-native-light", |s| {
        s.theme = theme::ThemeMode::Light;
        s.center = state::CenterPane::Program;
        let stage = views::hw_debug::FixtureStage::Live;
        if views::hw_debug::seed_debug_fixture_native(s, stage).is_ok() {
            s.program.node = views::hw_tree::HwNode::Core(helion_debug::DebugCoreKind::Ila);
        }
    }),
    ("hw-cfgmem-native", |s| {
        // Program / verify / read back over the native engine into the
        // fixture board's SPI flash.
        s.center = state::CenterPane::Program;
        if views::bitstream::seed_bitstream_fixture(s).is_ok() {
            if let Some(p) = helion_hw::find_cfgmem_part("mt25ql128") {
                let board = views::hw_native::MockBoard::Helion {
                    part: s.bitstream.part.clone(),
                    flash: p.jedec_id.map(|id| (id, p.capacity_bytes())),
                };
                if views::hw_native::use_mock_board(s, board).is_ok() {
                    for verb in ["create_hw_cfgmem", "write_cfgmem", "program_hw_cfgmem", "readback_hw_cfgmem"] {
                        let args = if verb == "create_hw_cfgmem" { vec!["mt25ql128".to_string()] } else { vec![] };
                        let _ = views::hw_cfgmem::tcl_verb(s, verb, &args);
                    }
                }
            }
        }
        s.toasts = Default::default();
    }),
    ("hw-cfgmem-add", |s| {
        s.center = state::CenterPane::Program;
        s.program.node = views::hw_tree::HwNode::CfgMem;
    }),
    ("ip", |s| {
        s.center = state::CenterPane::Ip;
        let _ = views::ip::select_ip(s, "h_uart");
    }),
    ("ip-bd", |s| {
        s.center = state::CenterPane::Ip;
        for core in ["h_rv32_hb1", "h_uart", "h_gpio", "h_timer"] {
            let _ = views::ip::create_bd_cell(s, core);
        }
        let _ = views::ip::apply_automation(s);
        let _ = views::ip::select_cell(s, "h_gpio_0");
    }),
    ("hls", |s| {
        s.hls.source = rune::TextEdit::new(
            "// Running sum over a 4-tap window (scene fixture).\nint sum4(int a[4]) {\n  int s = 0;\n  for (int i = 0; i < 4; i++) {\n    #pragma HLS PIPELINE II=1\n    s += a[i];\n  }\n  return s;\n}\n",
        );
        let _ = views::hls::start_synth(s);
        views::hls::wait(s);
        s.toasts = Default::default();
        s.center = state::CenterPane::Hls;
    }),
    ("hls-cosim", |s| {
        s.hls.source = rune::TextEdit::new(views::hls::NEW_KERNEL);
        s.hls.vectors = rune::TextEdit::new("a=1 b=2 c=3\na=-5 b=7 c=100\na=40 b=2 c=0\n");
        let _ = views::hls::start_synth(s);
        views::hls::wait(s);
        let _ = views::hls::start_cosim(s);
        views::hls::wait(s);
        s.toasts = Default::default();
        s.center = state::CenterPane::Hls;
    }),
    ("export-hardware", |s| {
        s.center = state::CenterPane::Ip;
        for core in ["h_rv32_hb1", "h_uart", "h_gpio"] {
            let _ = views::ip::create_bd_cell(s, core);
        }
        let _ = views::ip::apply_automation(s);
        s.handoff.hw_path = rune::TextEdit::new("/projects/project_1/design_1.hwp");
        views::handoff::open_export(s);
    }),
    ("archive-project", |s| {
        s.handoff.ar_path = rune::TextEdit::new("/projects/project_1_archive.zip");
        views::handoff::open_archive(s);
    }),
    ("ip-automation", |s| {
        s.center = state::CenterPane::Ip;
        for core in ["h_rv32_hb1", "h_uart", "h_gpio", "h_sync_fifo"] {
            let _ = views::ip::create_bd_cell(s, core);
        }
        s.dialogs.open(views::dialogs::DialogId::IpAutomation);
    }),
    ("ip-customize", |s| {
        s.center = state::CenterPane::Ip;
        for core in ["h_rv32_hb1", "h_uart"] {
            let _ = views::ip::create_bd_cell(s, core);
        }
        let _ = views::ip::apply_automation(s);
        let _ = views::ip::set_cell_param(s, "h_rv32_hb1_0", "ENABLE_MUL", "1");
        let _ = views::ip::open_customize(s, "h_rv32_hb1_0");
    }),
    ("bitstream", |s| {
        s.center = state::CenterPane::Bitstream;
        if views::bitstream::seed_bitstream_fixture(s).is_ok() {
            let _ = views::bitstream::select_frame(s, "2");
        }
    }),
    ("bitstream-empty", |s| {
        s.center = state::CenterPane::Bitstream;
    }),
    ("find", |s| {
        s.center = state::CenterPane::Find;
        if live::seed_live_fixture(s).is_ok() {
            let _ = views::find::select_find(s, "cell:u_div_u_luts2");
        }
    }),
    ("find-empty", |s| {
        s.center = state::CenterPane::Find;
    }),
    ("reports", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
    }),
    ("reports-ice40", |s| {
        // Real iCE40 implementation of examples/counter.sv: LUT4/DFF/EBR
        // rows against HX8K capacities.
        s.center = state::CenterPane::Reports;
        let _ = runs_family::seed_family_fixture(s, "ice40-hx8k");
    }),
    ("device-ice40", |s| {
        s.center = state::CenterPane::Device;
        let _ = runs_family::seed_family_fixture(s, "ice40-hx8k");
    }),
    ("timing-ice40", |s| {
        s.center = state::CenterPane::Timing;
        let _ = runs_family::seed_family_fixture(s, "ice40-hx8k");
    }),
    ("hls-fir-cosim", |s| {
        // The FIR MAC kernel that used to read back 0 in co-simulation.
        s.hls.source = rune::TextEdit::new("void fir_mac(int a[4], int b[4], int bias, int *out) {\n  #pragma HLS PIPELINE II=1\n  #pragma HLS ARRAY_PARTITION variable=a complete\n  #pragma HLS ARRAY_PARTITION variable=b complete\n  int acc = bias;\n  for (int i = 0; i < 4; i++) {\n    acc += a[i] * b[i];\n  }\n  *out = acc;\n}\n");
        s.hls.vectors = rune::TextEdit::new("a=[1,2,3,4] b=[10,20,30,40] bias=5\na=[-3,7,0,9] b=[2,-5,11,1] bias=-1\n");
        let _ = views::hls::start_synth(s);
        views::hls::wait(s);
        let _ = views::hls::start_cosim(s);
        views::hls::wait(s);
        s.hls.tab = views::hls::HlsTab::Cosim;
        s.toasts = Default::default();
        s.center = state::CenterPane::Hls;
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
        s.reports_view.selected_drc = Some(0);
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
    ("reports-clockutil", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        views::reports::io::scene_fill(s, views::reports::ReportNav::ClockUtil);
    }),
    ("reports-ctrlsets", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        views::reports::io::scene_fill(s, views::reports::ReportNav::ControlSets);
    }),
    ("reports-ssn", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        views::reports::io::scene_fill(s, views::reports::ReportNav::Ssn);
    }),
    ("reports-ssn-light", |s| {
        s.center = state::CenterPane::Reports;
        s.theme = theme::ThemeMode::Light;
        views::reports::demo_impl_complete(s);
        views::reports::io::scene_fill(s, views::reports::ReportNav::Ssn);
    }),
    ("reports-power-vcd", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        s.reports_view.selected = views::reports::ReportNav::Power;
        views::reports::vec_power::scene_fill(s);
    }),
    ("reports-ibis", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        views::reports::io::scene_fill(s, views::reports::ReportNav::Ibis);
    }),
    ("eco", |s| {
        s.center = state::CenterPane::Eco;
        views::reports::demo_impl_complete(s);
        views::eco::scene_fill(s);
    }),
    ("coverage", |s| {
        s.center = state::CenterPane::Coverage;
        views::tb::seed_coverage_fixture(s);
    }),
    ("coverage-light", |s| {
        s.center = state::CenterPane::Coverage;
        s.theme = theme::ThemeMode::Light;
        views::tb::seed_coverage_fixture(s);
    }),
    ("eco-light", |s| {
        s.center = state::CenterPane::Eco;
        s.theme = theme::ThemeMode::Light;
        views::reports::demo_impl_complete(s);
        views::eco::scene_fill(s);
    }),
    ("coverage-empty", |s| {
        s.center = state::CenterPane::Coverage;
    }),
    ("coverage-dut", |s| {
        s.center = state::CenterPane::Coverage;
        views::coverage_dut::seed_fixture(s);
        s.coverage.tab = views::coverage::CovTab::DutLines;
    }),
    ("coverage-dut-light", |s| {
        s.center = state::CenterPane::Coverage;
        s.theme = theme::ThemeMode::Light;
        views::coverage_dut::seed_fixture(s);
        s.coverage.tab = views::coverage::CovTab::DutBranches;
    }),
    ("coverage-dut-gutter", |s| {
        views::coverage_dut::seed_fixture(s);
        views::coverage_dut::open_dut(s);
    }),
    ("coverage-dut-gutter-light", |s| {
        s.theme = theme::ThemeMode::Light;
        views::coverage_dut::seed_fixture(s);
        views::coverage_dut::open_dut(s);
    }),
    ("runs-ooc", |s| {
        s.console.tab = views::console::ConsoleTab::DesignRuns;
        s.layout.console_h = 520.0;
        views::ooc::scene_fill(s);
    }),
    ("dfx", |s| {
        s.center = state::CenterPane::Dfx;
        views::dfx::scene_fill(s);
    }),
    ("eco-empty", |s| {
        s.center = state::CenterPane::Eco;
    }),
    ("eco-restored", |s| scenes::eco_restored(s, false)),
    ("eco-restored-light", |s| scenes::eco_restored(s, true)),
    ("dfx-light", |s| {
        s.center = state::CenterPane::Dfx;
        s.theme = theme::ThemeMode::Light;
        views::dfx::scene_fill(s);
    }),
    ("device-pblocks-light", |s| {
        s.center = state::CenterPane::Device;
        s.theme = theme::ThemeMode::Light;
        let _ = live::seed_live_fixture(s);
        crate::tcl::eval_command(s, "create_pblock pblock_div -add CLB_X2Y14:CLB_X5Y18");
        crate::tcl::eval_command(s, "create_pblock pblock_led");
        s.device.pblock_sel = None;
        s.device.zoom = 0.35;
    }),
    ("xdc-wizard-synth-first", |s| scenes::xdc_wizard_synth_first(s, false)),
    ("xdc-wizard-synth-first-light", |s| scenes::xdc_wizard_synth_first(s, true)),
    ("reports-no-run", |s| {
        s.center = state::CenterPane::Reports;
    }),
    ("reports-methodology", |s| {
        s.center = state::CenterPane::Reports;
        views::reports::demo_impl_complete(s);
        s.reports_view.selected = views::reports::ReportNav::Methodology;
        s.timing.methodology = vec![
            state::MethodRow {
                id: "TIMING-6".into(),
                severity: "CriticalWarning".into(),
                category: "Timing".into(),
                objects: "clk uart_clk".into(),
                message: "No common primary clock between related clocks clk and uart_clk; the crossing is timed without an exception and the requirement is derived from the closest edges.".into(),
            },
            state::MethodRow {
                id: "XDC-1".into(),
                severity: "Warning".into(),
                category: "Constraints".into(),
                objects: "led[0] led[1] led[2] led[3]".into(),
                message: "Output ports have no set_output_delay.".into(),
            },
        ];
        s.reports_view.selected_method = Some(0);
    }),
    ("constraints", |s| {
        s.center = state::CenterPane::Constraints;
        s.editor.xdc_lines = [
            "# Clocks",
            "create_clock -period 10.000 -name clk [get_ports clk]",
            "create_generated_clock -name clk_div -source [get_ports clk] -divide_by 4 [get_pins divclk_reg/Q]",
            "",
            "# I/O delays",
            "set_input_delay -clock clk 2.000 [get_ports btn]",
            "set_output_delay -clock clk 1.500 [get_ports {led[0]}]",
            "",
            "# Exceptions",
            "set_false_path -from [get_ports btn] -to [get_cells sync_reg]",
            "set_max_delay -datapath_only 4.000 -from [get_cells cnt_reg] -to [get_cells led_reg]",
            "",
            "# Pins",
            "set_property PACKAGE_PIN E3 [get_ports clk]",
            "set_property IOSTANDARD LVCMOS33 [get_ports clk]",
            "set_property PACKAGE_PIN H6 [get_ports {led[0]}]",
            "set_property IOSTANDARD LVCMOS33 [get_ports {led[0]}]",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        s.constraints_view.selected = Some(1);
    }),
    ("summary-runs", |s| {
        views::reports::demo_impl_complete(s);
        seed_timing_fixture(s);
        s.runs.history = vec![
            runs::RunRecord {
                seq: 1,
                kind: runs::RunKind::Synth,
                outcome: runs::RunOutcome::Complete,
                wns_ps: Some(3120),
                lut_ff: Some(2310),
                duration_ms: 1840,
                ..Default::default()
            },
            runs::RunRecord {
                seq: 2,
                kind: runs::RunKind::Impl,
                outcome: runs::RunOutcome::Failed,
                wns_ps: None,
                lut_ff: None,
                duration_ms: 950,
                ..Default::default()
            },
            runs::RunRecord {
                seq: 3,
                kind: runs::RunKind::Impl,
                outcome: runs::RunOutcome::Complete,
                wns_ps: Some(2061),
                lut_ff: Some(2415),
                duration_ms: 64_300,
                ..Default::default()
            },
        ];
    }),
    ("palette-files", |s| {
        views::palette::open_palette(s, views::palette::PaletteMode::Files);
    }),
    ("palette-commands", |s| {
        views::palette::open_palette(s, views::palette::PaletteMode::Commands);
        s.palette.query = rune::TextEdit::new("run");
    }),
    ("templates", |s| {
        s.center = state::CenterPane::Editor;
        views::templates::open(s);
    }),
    ("cheatsheet", |s| {
        views::palette::open_cheatsheet(s);
    }),
];

/// Representative timing report output the timing scenes share (single
/// 10ns clock, one intra group, worst path first with UG903 pin rows),
/// shaped like real `TimingState` so snapshots render tables.
/// Design Runs fixture (scenes only): finished records with real-shaped
/// metrics, created runs (one never launched), and impl_1 active. Nothing
/// is queued: an idle worker would start a queued run at once.
fn seed_runs_fixture(s: &mut AppState) {
    use runs::{RunKind, RunOutcome, RunRecord, RunSpec};
    s.console.tab = views::console::ConsoleTab::DesignRuns;
    s.layout.console_h = 300.0;
    s.flow.synth = state::RunStatus::Complete;
    s.flow.impl_run = state::RunStatus::Complete;
    let strat = |p: &str, seed: u64| {
        let mut st = helion_proj::RunStrategy::preset(p).expect("preset");
        st.placer_seed = seed;
        st
    };
    for (name, st) in [
        ("impl_2", strat("TimingExplore", 0)),
        ("sweep_s1", strat("Default", 1)),
        ("sweep_s2", strat("Default", 2)),
        ("sweep_s3", strat("Default", 3)),
    ] {
        s.runs.defs.push(RunSpec {
            name: name.into(),
            kind: RunKind::Impl,
            strategy: st,
            incremental: None,
        });
    }
    let rec = |seq, name: &str, kind, outcome, wns: Option<i64>, tns, luts: Option<u32>, ms| RunRecord {
        seq,
        kind,
        outcome,
        wns_ps: wns,
        tns_ps: tns,
        luts,
        ffs: luts.map(|l| l / 2 + 7),
        bram: luts.map(|_| 2),
        dsp: luts.map(|_| 1),
        lut_ff: luts.map(|l| l + l / 2 + 7),
        duration_ms: ms,
        name: name.into(),
        artifacts: (outcome == RunOutcome::Complete)
            .then(|| Box::new(runs::RunArtifacts::default())),
        ..Default::default()
    };
    s.runs.history = vec![
        rec(1, "synth_1", RunKind::Synth, RunOutcome::Complete, Some(3120), Some(0), Some(398), 1840),
        rec(2, "impl_1", RunKind::Impl, RunOutcome::Complete, Some(2061), Some(0), Some(412), 6400),
        rec(3, "impl_2", RunKind::Impl, RunOutcome::Complete, Some(-120), Some(-480), Some(415), 9100),
        rec(4, "sweep_s1", RunKind::Impl, RunOutcome::Complete, Some(1874), Some(0), Some(412), 6900),
        rec(5, "sweep_s2", RunKind::Impl, RunOutcome::Failed, None, None, None, 2100),
    ];
    s.runs.active_result = Some("impl_1".into());
    s.runs.ui.selected = Some("impl_2".into());
    s.runs.ui.sort = Some((views::runs_table::RunCol::Wns, true));
}

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
        whs_ps: s.timing.whs_ps,
        ths_ps: s.timing.ths_ps,
        endpoints: s.timing.endpoints,
    }];
    s.timing.paths = vec![state::TimingPathRow {
        name: "cnt_3->cnt_7".into(),
        from: "cnt_3".into(),
        to: "cnt_7".into(),
        slack_ps: s.timing.wns_ps.unwrap_or(0),
        delay_ps: 7939,
        group: state::TimingPathGroup::Register,
        launch_clock: "clk".into(),
        capture_clock: "clk".into(),
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
        source: "clk".into(),
        net: "clk".into(),
        loads: 8,
        buffers: 1,
        fanout: 8,
        insertion_ps: 120,
    }];
    s.timing.design = "project1_demo".into();
    s.timing.voltage_mv = 1000;
    s.timing.temp_c = 25;
}

/// Two-clock timing fixture for the clock-interaction / CDC / clock
/// scenes: `clk` (10 ns) and an unrelated `uart_clk` (20 ns) with one
/// unsafe crossing, shaped like real `TimingState` output.
fn seed_two_clock_fixture(s: &mut AppState) {
    use state::{
        CdcRow, ClockInteractionRow, ClockNetRow, ClockRel, TimingClock, TimingGroupKind,
        TimingGroupRow,
    };
    s.center = state::CenterPane::Timing;
    s.flow.synth = state::RunStatus::Complete;
    s.flow.impl_run = state::RunStatus::Complete;
    s.status = state::StatusVerb::ImplementationComplete;
    s.timing = state::TimingState {
        wns_ps: Some(-312),
        tns_ps: -936,
        whs_ps: Some(96),
        ths_ps: 0,
        failing_endpoints: 3,
        failing_setup: 3,
        failing_hold: 0,
        endpoints: 136,
        has_run: true,
        ..state::TimingState::default()
    };
    seed_timing_fixture(s);
    let clock = |name: &str, period_ps: u64| TimingClock {
        name: name.into(),
        period_ps,
        source: name.into(),
        generated: false,
        master: None,
    };
    s.timing.clocks = vec![clock("clk", 10_000), clock("uart_clk", 20_000)];
    let cell = |from: &str,
                to: &str,
                relation: ClockRel,
                common: u64,
                req: i64,
                wns: Option<i64>,
                paths: usize| {
        ClockInteractionRow {
            from: from.into(),
            to: to.into(),
            relation,
            common_period_ps: common,
            requirement_ps: req,
            wns_ps: wns,
            path_count: paths,
        }
    };
    s.timing.clock_interaction = vec![
        cell(
            "clk",
            "clk",
            ClockRel::Timed,
            10_000,
            10_000,
            Some(2061),
            112,
        ),
        cell(
            "clk",
            "uart_clk",
            ClockRel::TimedUnsafe,
            20_000,
            10_000,
            Some(-312),
            3,
        ),
        cell(
            "uart_clk",
            "clk",
            ClockRel::Asynchronous,
            20_000,
            10_000,
            None,
            0,
        ),
        cell(
            "uart_clk",
            "uart_clk",
            ClockRel::Timed,
            20_000,
            20_000,
            Some(12_408),
            24,
        ),
    ];
    s.timing.cdc = vec![
        CdcRow {
            from: "clk".into(),
            to: "uart_clk".into(),
            severity: "Critical".into(),
            check: "CDC-1".into(),
            endpoints: 3,
            sync: false,
            wns_ps: Some(-312),
            relation: "Timed (unsafe)".into(),
        },
        CdcRow {
            from: "uart_clk".into(),
            to: "clk".into(),
            severity: "Info".into(),
            check: "CDC-15".into(),
            endpoints: 2,
            sync: true,
            wns_ps: None,
            relation: "Asynchronous".into(),
        },
    ];
    let group = |name: &str,
                 from: &str,
                 to: &str,
                 kind: TimingGroupKind,
                 wns: i64,
                 tns: i64,
                 endpoints: usize| TimingGroupRow {
        name: name.into(),
        from: from.into(),
        to: to.into(),
        kind,
        wns_ps: Some(wns),
        tns_ps: tns,
        whs_ps: Some(96),
        ths_ps: 0,
        endpoints,
    };
    s.timing.groups = vec![
        group("clk", "clk", "clk", TimingGroupKind::Intra, 2061, 0, 112),
        group(
            "uart_clk",
            "uart_clk",
            "uart_clk",
            TimingGroupKind::Intra,
            12_408,
            0,
            24,
        ),
        group(
            "clk to uart_clk",
            "clk",
            "uart_clk",
            TimingGroupKind::Inter,
            -312,
            -936,
            3,
        ),
    ];
    s.timing.clock_networks = vec![
        ClockNetRow {
            name: "clk".into(),
            period_ps: 10_000,
            source: "clk".into(),
            net: "clk_IBUF_BUFG".into(),
            loads: 112,
            buffers: 1,
            fanout: 112,
            insertion_ps: 1_240,
        },
        ClockNetRow {
            name: "uart_clk".into(),
            period_ps: 20_000,
            source: "uart_clk".into(),
            net: "uart_clk_IBUF_BUFG".into(),
            loads: 24,
            buffers: 1,
            fanout: 24,
            insertion_ps: 980,
        },
    ];
}

/// Initial state for tests and headless scenes: the in-memory demo project,
/// so snapshots never depend on the machine they render on.
pub fn init() -> AppState {
    AppState::demo()
}

/// Initial state for an interactive launch: saved layout/theme/recents, then
/// a project from `HELION_OPEN` or the first non-flag argument, else the
/// Start page. `HELION_DEMO=1` boots the demo project instead.
pub fn boot() -> AppState {
    if std::env::var_os("HELION_DEMO").is_some() {
        let mut s = init();
        layout::load(&mut s);
        return s;
    }
    let mut s = AppState::demo();
    s.empty_project();
    s.project.location.clear();
    s.center = state::CenterPane::Start;
    s.nav_back.clear();
    layout::load(&mut s);
    let arg = std::env::var_os("HELION_OPEN")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::args()
                .skip(1)
                .find(|a| !a.starts_with('-'))
                .map(std::path::PathBuf::from)
        });
    if let Some(path) = arg {
        pickers::open_path(&mut s, &path);
    }
    s
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
        state::CenterPane::Ila => views::ila_view::ila_pane(s, t),
        state::CenterPane::Program => views::program::program_pane(s, t),
        state::CenterPane::Reports => views::reports::reports_pane(s, t),
        state::CenterPane::Ip => views::ip::ip_pane(s, t),
        state::CenterPane::Bitstream => views::bitstream::bitstream_pane(s, t),
        state::CenterPane::Find => views::find::find_pane(s, t),
        state::CenterPane::Constraints => views::constraints::constraints_pane(s, t),
        state::CenterPane::Eco => views::eco::eco_pane(s, t),
        state::CenterPane::Dfx => views::dfx::dfx_pane(s, t),
        state::CenterPane::Coverage => views::coverage::coverage_pane(s, t),
        state::CenterPane::Hls => views::hls_view::hls_pane(s, t),
    }
}

fn status_bar(s: &AppState, t: &VivadoTheme) -> El {
    views::status::status_bar(s, t)
}

/// Middle band: Flow Navigator | Sources column | center pane, each dock
/// shown per [`layout::LayoutState`]. The Sources column carries a drag
/// splitter on its right edge; focus mode leaves the center pane alone.
fn workspace(s: &AppState, t: &VivadoTheme) -> El {
    // 6pt gutters throughout; the splitters *are* the gutters they sit in,
    // so a resizable edge never adds extra space.
    let mut band = row()
        .key("workspace")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(6.0)
        .pb(if s.layout.console_visible() { 0.0 } else { 6.0 });
    if s.layout.flow_visible() {
        band = band.child(flow_rail(s, t)).child(el().w(6.0).h_full());
    }
    if s.layout.sources_visible() {
        band = band
            .child(sources_column(s, t))
            .child(layout::sources_splitter(t));
    }
    // Center column: open-view tabs over the active surface (no tabs on
    // the empty-project Start page, which owns the whole column).
    let mut center = col().grow(1.0).h_full().min_w(0.0).min_h(0.0);
    if !s.project.name.is_empty() {
        center = center.child(layout::view_tabs(s, t));
    }
    band.child(
        center.child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(center_pane(s, t)),
        ),
    )
}

/// Root view: Vivado shell skeleton (titlebar, menu, rail, summary, status).
/// Full surfaces arrive per batch; this proves theme + state + scenes render.
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    live::begin_frame();
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
        .child(workspace(s, &t));
    if s.layout.console_visible() {
        shell = shell
            .child(layout::console_splitter(&t))
            .child(
                el().w_full()
                    .px(6.0)
                    .pb(6.0)
                    .child(views::console::console_pane(s, &t)),
            );
    }
    shell = shell.child(status_bar(s, &t));
    if let Some(overlay) = views::chrome::menu_overlay(s, &t) {
        shell = shell.child(overlay);
    }
    let mut layer = Vec::new();
    views::dialogs::push_dialogs(&mut layer, s, &t);
    views::device::push_device_pops(&mut layer, s, &t);
    views::wave_decode::push_wave_pops(&mut layer, s, &t);
    views::runs_table::push_runs_pops(&mut layer, s, &t);
    views::status::push_toasts(&mut layer, s, &t);
    views::palette::push_palette(&mut layer, s, &t);
    views::palette::push_cheatsheet(&mut layer, s, &t);
    views::chrome::push_about(&mut layer, s, &t);
    views::templates::push_templates(&mut layer, s, &t);
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
    let ila_dirty = views::ila::poll(s);
    // Toasts expire on the poll clock; sleep until the next one is due.
    let (toasts_dirty, next) = s.toasts.tick(t);
    if let Some(secs) = next {
        tx.wake_after(std::time::Duration::from_secs_f64(secs.max(0.05)));
    }
    // Safety net: out-of-band buffer swaps (project reseed clears tabs)
    // re-mirror the live edit field; a no-op when already synced.
    views::editor::sync_edit(s);
    let hw_dirty = views::program::poll(s);
    let pblocks_dirty = views::device_floor::sync_from_project(s);
    let tabs_dirty = layout::sync_tabs(s);
    let jobs_dirty = jobs::poll(s, tx) | views::hls::poll(s, tx);
    let diag_dirty = views::editor_diag::poll(s, t, tx);
    let bg_dirty = bg::poll(s, Some(tx));
    runs_dirty
        || sim_dirty
        || ila_dirty
        || hw_dirty
        || toasts_dirty
        || pblocks_dirty
        || tabs_dirty
        || jobs_dirty
        || diag_dirty
        || bg_dirty
}

pub fn app_spec() -> AppSpec<AppState> {
    AppSpec::<AppState> {
        scenes: showcase::scenes(),
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
