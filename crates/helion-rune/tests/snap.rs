//! Harness: every `--scene` renders its surface, and cross-surface taps
//! stay coherent.
//!
//! Per-view tap tests live beside each view (`src/views/*/tests`); this
//! suite is the batch-level net: the [`helion_rune::scenes`] catalog must
//! cover every [`helion_rune::SCENES`] entry, each scene must mount its
//! keyed regions with sane rects, and journeys that cross surfaces (toolbar
//! run -> runs table, start -> wizard -> project, timing prompt -> log)
//! must hold end to end.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `examples/hello/src/main.rs`: `Runtime::new` + `find` tap pattern.
//! - `crates/rune/src/runtime/elements.rs`: `find/presented/elements`.
//! - `crates/rune/src/runtime/input/pointer.rs`: `pointer_down/pointer_up`.
//! - `crates/rune/src/runtime/frame.rs`: `frame(dt)` commits input.
//! - `crates/rune/src/tooling.rs`: `AppSpec::scenes`, `--snap/--tree/--scene`.

use helion_rune::runs::{self, RunKind};
use helion_rune::scenes;
use helion_rune::state::{AppState, CenterPane, RunStatus, ToastKind};
use helion_rune::views::console::LogRun;
use helion_rune::views::dialogs::{CreateStep, DialogId};
use rune::{Env, Runtime};
use std::time::{Duration, Instant};

fn rt_for(scene: &str) -> Runtime<AppState> {
    rt_for_env(scene, false)
}

fn rt_for_env(scene: &str, system_dark: bool) -> Runtime<AppState> {
    let spec = helion_rune::app_spec();
    let env = Env {
        system_dark,
        scale: 2.0,
    };
    let state = scenes::state_for(scene).unwrap_or_else(|| panic!("unknown scene: {scene}"));
    let mut rt = Runtime::new(state, helion_rune::view, spec.size, env);
    rt.frame(1.0 / 60.0);
    rt
}

fn tap(rt: &mut Runtime<AppState>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
    // Move first, like a live pointer: a press teleported onto a tree that
    // just changed (an overlay such as a toast mounting) lands on stale hover.
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn scene_catalog_covers_every_spec_scene_exactly() {
    let spec = scenes::spec_names();
    let catalog: Vec<&str> = scenes::ALL.iter().map(|e| e.name).collect();
    assert_eq!(
        catalog, spec,
        "catalog must list every SCENES entry in order"
    );
    assert!(
        catalog.len() >= 30,
        "harness must cover all surfaces, got {} scenes",
        catalog.len()
    );
}

#[test]
fn every_scene_renders_shell_chrome_and_its_surface() {
    for entry in scenes::ALL {
        let rt = rt_for(entry.name);
        for key in scenes::CHROME_KEYS {
            assert!(
                rt.find(key).is_some(),
                "[{}] missing chrome key: {key}",
                entry.name
            );
        }
        for key in entry.keys {
            assert!(
                rt.find(key).is_some(),
                "[{}] missing surface key: {key}",
                entry.name
            );
        }
        for key in entry.absent {
            assert!(
                rt.find(key).is_none(),
                "[{}] key should be unmounted: {key}",
                entry.name
            );
        }
    }
}

#[test]
fn every_scene_renders_shell_chrome_and_its_surface_in_dark_mode() {
    use helion_rune::ThemeMode;
    for entry in scenes::ALL {
        // Dark OS appearance with the default Follow-System choice.
        let rt = rt_for_env(entry.name, true);
        for key in scenes::CHROME_KEYS.iter().chain(entry.keys.iter()) {
            assert!(
                rt.find(key).is_some(),
                "[{}] dark: missing key: {key}",
                entry.name
            );
        }
        for key in entry.absent {
            assert!(
                rt.find(key).is_none(),
                "[{}] dark: key should be unmounted: {key}",
                entry.name
            );
        }
        // Manual dark pin under a light OS must mount the same surface.
        let spec = helion_rune::app_spec();
        let mut pinned = scenes::state_for(entry.name).unwrap();
        pinned.theme = ThemeMode::Dark;
        let mut rt = Runtime::new(
            pinned,
            helion_rune::view,
            spec.size,
            Env {
                system_dark: false,
                scale: 2.0,
            },
        );
        rt.frame(1.0 / 60.0);
        for key in scenes::CHROME_KEYS.iter().chain(entry.keys.iter()) {
            assert!(
                rt.find(key).is_some(),
                "[{}] pinned-dark: missing key: {key}",
                entry.name
            );
        }
    }
}

#[test]
fn keyed_regions_layout_inside_the_shell_with_nonzero_rects() {
    for entry in scenes::ALL {
        let rt = rt_for(entry.name);
        let (shell, _) = rt
            .presented("shell")
            .unwrap_or_else(|| panic!("[{}] no shell rect", entry.name));
        assert!(
            shell[2] > 0.0 && shell[3] > 0.0,
            "[{}] shell has no size: {shell:?}",
            entry.name
        );
        for key in scenes::CHROME_KEYS.iter().chain(entry.keys.iter()) {
            let (r, _) = rt
                .presented(key)
                .unwrap_or_else(|| panic!("[{}] no rect: {key}", entry.name));
            assert!(
                r[2] > 0.0 && r[3] > 0.0,
                "[{}] {key} has no size: {r:?}",
                entry.name
            );
            assert!(
                r[0] >= shell[0] - 1.0
                    && r[1] >= shell[1] - 1.0
                    && r[0] + r[2] <= shell[0] + shell[2] + 1.0
                    && r[1] + r[3] <= shell[1] + shell[3] + 1.0,
                "[{}] {key} escapes the shell: {r:?} vs {shell:?}",
                entry.name
            );
        }
    }
}

#[test]
fn toolbar_run_arms_rail_runs_table_and_console_echo() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "tool-run-synth");
    assert_eq!(rt.state.flow.synth, RunStatus::Running);
    assert!(
        rt.find("flow-synthesis-run").is_some(),
        "Run Synthesis stays on the rail (with its live status) so it can re-run"
    );
    assert!(
        rt.find("flow-synthesis-open-synthesized-design").is_some(),
        "Open Synthesized Design stays listed"
    );
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "launch_runs synth_1"),
        "run must echo its Tcl verb"
    );
    tap(&mut rt, "console-tab-runs");
    assert!(
        rt.find("console-run-synth").is_some(),
        "runs table must show synth_1"
    );
    assert!(
        rt.find("console-run-synth-status").is_some(),
        "runs table must show its status"
    );
}

#[test]
fn menus_toggle_on_every_center_surface() {
    for (scene, center) in [
        ("summary", CenterPane::Summary),
        ("editor", CenterPane::Editor),
        ("timing-no-run", CenterPane::Timing),
        ("start", CenterPane::Start),
        ("schematic", CenterPane::Schematic),
        ("device", CenterPane::Device),
        ("wave", CenterPane::Wave),
        ("sim", CenterPane::Sim),
        ("program", CenterPane::Program),
        ("reports-no-run", CenterPane::Reports),
        ("ip", CenterPane::Ip),
        ("bitstream-empty", CenterPane::Bitstream),
        ("find-empty", CenterPane::Find),
        ("constraints", CenterPane::Constraints),
    ] {
        let mut rt = rt_for(scene);
        assert_eq!(rt.state.center, center, "[{scene}] wrong center pane");
        tap(&mut rt, "menu-edit");
        assert!(
            rt.state.chrome.open_menu.is_some(),
            "[{scene}] menu must open, center {center:?}"
        );
        tap(&mut rt, "menu-edit");
        assert_eq!(
            rt.state.chrome.open_menu, None,
            "[{scene}] menu must toggle shut"
        );
    }
}

#[test]
fn create_wizard_walks_to_summary_and_finish_applies_the_project() {
    let mut rt = rt_for("create-project");
    assert_eq!(rt.state.dialogs.create.step, CreateStep::Info);
    // Never write into the real projects folder from a test.
    let root = std::env::temp_dir().join(format!("helion-snap-create-{}", std::process::id()));
    rt.state.dialogs.create.location = rune::TextEdit::new(root.display().to_string());
    rt.invalidate();
    rt.frame(0.05);
    for _ in 0..4 {
        tap(&mut rt, "create-next");
    }
    assert_eq!(rt.state.dialogs.create.step, CreateStep::Summary);
    tap(&mut rt, "create-finish");
    assert_eq!(rt.state.dialogs.open, None, "finish must close the wizard");
    assert_eq!(rt.state.project.name, "project_2");
    let last = rt
        .state
        .console
        .lines
        .last()
        .map(|l| l.text.clone())
        .unwrap_or_default();
    assert!(last.starts_with("create_project project_2"), "tcl: {last}");
    assert!(
        rt.find("summary-name").is_some(),
        "summary must show the new project"
    );
    assert!(root.join("project_2/project_2.prj").is_file());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reports_nav_switches_detail_bodies_and_prompt_arms_impl() {
    use helion_rune::views::reports::ReportNav;
    let mut rt = rt_for("reports");
    assert_eq!(rt.state.center, CenterPane::Reports);
    tap(&mut rt, "reports-nav-power");
    assert_eq!(rt.state.reports_view.selected, ReportNav::Power);
    assert!(
        rt.find("reports-power-table").is_some(),
        "power body must show"
    );
    assert!(
        rt.find("reports-util-table").is_none(),
        "util body must leave"
    );
    assert!(
        rt.find("reports-power-split-dynamic").is_some(),
        "split bar must show"
    );
    tap(&mut rt, "reports-nav-drc");
    assert_eq!(rt.state.reports_view.selected, ReportNav::Drc);
    assert!(
        rt.find("reports-drc-verdict").is_some(),
        "verdict must show"
    );
    assert!(rt.find("reports-drc-hist").is_some(), "histogram must show");
    assert!(
        rt.find("reports-drc-table").is_none(),
        "clean demo run has no table"
    );
    let mut rt = rt_for("reports-drc");
    assert!(
        rt.find("reports-drc-row-1").is_some(),
        "seeded rows must mount"
    );
    tap(&mut rt, "reports-drc-row-1");
    assert_eq!(
        rt.state.reports_view.selected_drc,
        Some(1),
        "row tap selects the violation"
    );
    assert!(
        rt.find("reports-drc-detail").is_some(),
        "selection opens the detail panel"
    );
    let mut rt = rt_for("reports-no-run");
    tap(&mut rt, "reports-run-synth");
    assert_eq!(rt.state.flow.synth, RunStatus::Running);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "launch_runs synth_1")
    );
    runs::cancel_run(&mut rt.state);
    let start = Instant::now();
    while rt.state.runs.is_active() {
        runs::poll_runs(&mut rt.state);
        assert!(start.elapsed() < Duration::from_secs(60), "run timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn constraints_scene_opens_editor_and_wizard() {
    let mut rt = rt_for("constraints");
    assert_eq!(rt.state.center, CenterPane::Constraints);
    tap(&mut rt, "constraints-wizard");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::Constraints));
    let mut rt = rt_for("constraints");
    tap(&mut rt, "constraints-open-editor");
    assert_eq!(rt.state.center, CenterPane::Editor);
}

#[test]
fn report_verbs_navigate_and_dump() {
    let mut rt = rt_for("summary-impl-complete");
    helion_rune::tcl::eval_command(&mut rt.state, "report_clock_interaction");
    assert_eq!(rt.state.center, CenterPane::Timing);
    helion_rune::tcl::eval_command(&mut rt.state, "report_power");
    helion_rune::jobs::wait_idle(&mut rt.state);
    assert_eq!(rt.state.center, CenterPane::Reports);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text.starts_with("report_power part=")),
        "report_power dumps the power state"
    );
    helion_rune::tcl::eval_command(&mut rt.state, "report_constraints");
    assert_eq!(rt.state.center, CenterPane::Constraints);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text.starts_with("report_constraints rows=")),
        "report_constraints dumps the parsed XDC"
    );
}

#[test]
fn timing_prompt_starts_impl_and_log_follows_the_run() {
    let mut rt = rt_for("timing-no-run");
    tap(&mut rt, "timing-run-impl");
    assert_eq!(rt.state.flow.impl_run, RunStatus::Running);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "launch_runs impl_1")
    );
    tap(&mut rt, "console-tab-log");
    assert!(rt.find("console-log").is_some(), "log body must mount");
    tap(&mut rt, "console-logrun-impl");
    assert_eq!(rt.state.console.log_run, LogRun::Implementation);
}

#[test]
fn impl_failed_toast_dismisses_from_the_status_scene() {
    let mut rt = rt_for("status-impl-failed");
    assert!(
        rt.find("toast-0").is_some(),
        "failed run must raise a toast"
    );
    tap(&mut rt, "close");
    assert!(
        rt.state.toasts.items.is_empty(),
        "close must dismiss the toast"
    );
    assert!(rt.find("toast-0").is_none(), "dismissed toast must unmount");
}

#[test]
fn schematic_select_zoom_and_cross_probe_sources() {
    use helion_rune::views::schematic::{Lod, SchemSel};
    use helion_rune::views::sources::SourceFileId;
    let mut rt = rt_for("schematic");
    tap(&mut rt, "schematic-cell-u-div-u-ff0");
    assert_eq!(
        rt.state.schematic.selected,
        Some(SchemSel::Cell("u_div_u_ff0".into()))
    );
    assert!(
        rt.find("schematic-props-name").is_some(),
        "props must show the cell"
    );
    tap(&mut rt, "schematic-locate");
    // The fixture's top (counter_top) is not declared by any demo project
    // file: locate says so instead of selecting an unrelated file.
    assert_eq!(rt.state.sources.selected, SourceFileId::Demo);
    assert!(
        rt.state
            .toasts
            .items
            .iter()
            .any(|t| t.title == "Not in project sources"),
        "locate must report that no project file declares the top"
    );
    // Let the toast's entrance settle on the clock (the helper's frames all
    // run at the same instant) before the next press.
    for i in 0..20 {
        rt.frame(0.1 + i as f64 * 0.05);
    }
    tap(&mut rt, "schematic-hier-u-led");
    assert_eq!(
        rt.state.schematic.selected,
        Some(SchemSel::Hier("u_led".into()))
    );
    assert!(
        rt.find("schematic-cell-u-led-u-iob").is_none(),
        "collapse must unmount children"
    );
    tap(&mut rt, "schematic-hier-u-led");
    assert!(
        rt.find("schematic-cell-u-led-u-iob").is_some(),
        "expand must remount children"
    );
    for _ in 0..4 {
        tap(&mut rt, "schematic-zoom-out");
    }
    assert_eq!(rt.state.schematic.lod(), Lod::Blocks);
    assert!(
        rt.find("schematic-cell-u-div-u-ff0").is_none(),
        "Blocks LOD hides owned cells"
    );
    assert!(
        rt.find("schematic-cell-u-div").is_some(),
        "Blocks LOD keeps hierarchy"
    );
    tap(&mut rt, "schematic-zoom-fit");
    assert_ne!(
        rt.state.schematic.lod(),
        Lod::Blocks,
        "fit frames the sheet at a readable zoom"
    );
    assert!(
        rt.find("schematic-cell-u-div-u-ff0").is_some(),
        "fit restores the sheet"
    );
}

#[test]
fn device_select_layers_find_and_locate() {
    use helion_rune::views::device::DeviceSel;
    use helion_rune::views::sources::SourceFileId;
    let mut rt = rt_for("device-routed");
    assert_eq!(rt.state.device.selected, Some(DeviceSel::Net("clk".into())));
    assert!(
        rt.find("device-inspector-name").is_some(),
        "inspector must show the net"
    );
    tap(&mut rt, "device-layer-nets");
    assert!(!rt.state.device.layers.nets, "nets layer must toggle off");
    tap(&mut rt, "device-layer-nets");
    assert!(
        rt.state.device.layers.nets,
        "nets layer must toggle back on"
    );
    tap(&mut rt, "device-find-row-0");
    assert_eq!(
        rt.state.device.selected,
        Some(DeviceSel::Cell("u_div_u_ff0".into()))
    );
    assert!(
        rt.find("device-inspector-site").is_some(),
        "inspector must show the site"
    );
    tap(&mut rt, "device-locate");
    assert_eq!(rt.state.sources.selected, SourceFileId::Demo);
    assert!(
        rt.state
            .toasts
            .items
            .iter()
            .any(|t| t.title == "Not in project sources"),
        "locate must report that no project file declares the top"
    );
    for i in 0..20 {
        rt.frame(0.1 + i as f64 * 0.05);
    }
    tap(&mut rt, "device-find-toggle");
    assert!(
        rt.find("device-find").is_none(),
        "find palette must collapse"
    );
    tap(&mut rt, "device-find-toggle");
    assert!(
        rt.find("device-find-row-0").is_some(),
        "find palette must reopen"
    );
}

#[test]
fn wave_cursors_measure_delta_and_markers_echo_tcl() {
    use helion_rune::views::wave::WaveRadix;
    let mut rt = rt_for("wave-cursors");
    assert_eq!(rt.state.wave.cursor_a, Some(16));
    assert_eq!(rt.state.wave.cursor_b, Some(64));
    assert_eq!(rt.state.wave.markers.len(), 2);
    assert!(rt.find("wave-delta").is_some(), "A/B must show the delta");
    tap(&mut rt, "wave-set-a");
    assert_eq!(rt.state.wave.cursor_a, Some(48));
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "set_cursor A 48 ns"),
        "Set A must echo its Tcl verb"
    );
    tap(&mut rt, "wave-add-marker");
    assert_eq!(rt.state.wave.markers.len(), 3);
    assert!(
        rt.find("wave-marker-2").is_some(),
        "new marker flag must mount"
    );
    tap(&mut rt, "wave-radix-bin");
    assert_eq!(
        rt.state.wave.radix_for("vb_led[4:0]"),
        WaveRadix::Bin,
        "radix applies to the selected trace"
    );
    tap(&mut rt, "wave-scope-dut");
    tap(&mut rt, "wave-object-add-an");
    assert!(
        rt.find("wave-name-an").is_some(),
        "added bus must mount its row"
    );
    assert!(
        rt.find("wave-lane-an").is_some(),
        "added bus must mount its lane"
    );
}

#[test]
fn sim_run_arms_flow_and_opens_the_waveform() {
    use helion_rune::views::tb::TbStatus;
    let mut rt = rt_for("sim-detail");
    assert_eq!(rt.state.sim.selected, Some(0));
    assert_eq!(
        rt.state.sim.status(0),
        TbStatus::Failed,
        "fixture ran for real"
    );
    tap(&mut rt, "sim-detail-run");
    assert_eq!(rt.state.sim.status(0), TbStatus::Running);
    assert_eq!(rt.state.flow.sim, RunStatus::Running);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "launch_simulation -simset sim_1 -tb counter_early_tb"),
        "run must echo its Tcl verb"
    );
    helion_rune::views::tb::cancel_sim(&mut rt.state);
    let start = Instant::now();
    while rt.state.sim.is_active() {
        helion_rune::views::tb::poll_sim(&mut rt.state);
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut rt = Runtime::new(
        rt.state.clone(),
        helion_rune::view,
        helion_rune::app_spec().size,
        Env {
            system_dark: false,
            scale: 2.0,
        },
    );
    rt.frame(1.0 / 60.0);
    tap(&mut rt, "sim-tb-1");
    assert_eq!(rt.state.sim.selected, Some(1));
    tap(&mut rt, "sim-detail-open-wave");
    assert_eq!(rt.state.center, CenterPane::Wave);
    assert!(rt.find("wave-canvas").is_some(), "waveform must mount");
    assert!(rt.find("sim").is_none(), "sim surface must leave");
}

#[test]
fn program_flow_sim_cable_programs_the_real_bitstream() {
    use helion_rune::views::program::{self, CableChoice, ProgramPhase, StatSource};
    let mut rt = rt_for("program-nousb");
    assert_eq!(rt.state.program.cable, CableChoice::Auto);
    assert!(
        rt.find("program-blocked").is_some(),
        "no USB programmer must explain why Program is refused"
    );
    tap(&mut rt, "program-program");
    assert_eq!(
        rt.state.program.phase,
        ProgramPhase::Idle,
        "board cable without USB never programs"
    );
    tap(&mut rt, "program-cable-opt-1");
    assert_eq!(rt.state.program.cable, CableChoice::Sim);
    assert!(rt.find("program-blocked").is_none());
    tap(&mut rt, "program-program");
    assert_eq!(rt.state.program.phase, ProgramPhase::Programming);
    assert!(program::wait_idle(&mut rt.state, Duration::from_secs(60)));
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.program.phase, ProgramPhase::Done);
    let res = rt.state.program.result.clone().expect("result");
    assert!(res.ok && res.backend == "sim", "{}", res.line);
    assert_eq!(
        rt.state.program.stat.as_ref().map(|s| s.source),
        Some(StatSource::SimProgrammed)
    );
    assert!(rt.find("program-results").is_some(), "results must mount");
    assert!(rt.find("program-verdict").is_some(), "verdict must mount");
    assert!(rt.find("program-stat-done").is_some(), "STAT cards mount");
    assert!(
        rt.find("toast-0").is_some(),
        "completion must raise a toast"
    );
}

#[test]
fn new_surfaces_navigate_from_tcl() {
    let mut rt = rt_for("summary");
    rt.update(|s, _| helion_rune::tcl::eval_command(s, "ip_catalog"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.center, CenterPane::Ip);
    assert!(rt.find("ip").is_some());
    rt.update(|s, _| helion_rune::tcl::eval_command(s, "report_bitstream"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.center, CenterPane::Bitstream);
    assert!(rt.find("bitstream-empty").is_some());
    rt.update(|s, _| helion_rune::tcl::eval_command(s, "find clk"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.center, CenterPane::Find);
    assert!(rt.find("find-empty").is_some());
}

#[test]
fn start_page_opens_the_create_wizard_over_the_shell() {
    let mut rt = rt_for("start");
    tap(&mut rt, "start-create-project");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::NewProject));
    assert!(
        rt.find("create-split").is_some(),
        "wizard must mount over the shell"
    );
    assert!(
        rt.find("start-page").is_some(),
        "start page must stay mounted behind"
    );
}

// --- func-01-navigation journeys: every rail/menu/link/toolbar hop is live ---

#[test]
fn flow_rail_open_designs_navigate_to_their_views() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-rtl-analysis-open-elaborated-design");
    assert_eq!(
        rt.state.center,
        CenterPane::Schematic,
        "elaborated must open schematic"
    );
    assert!(rt.find("schematic").is_some());
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-synthesis-open-synthesized-design");
    assert_eq!(
        rt.state.center,
        CenterPane::Schematic,
        "synthesized must open schematic"
    );
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-implementation-open-implemented-design");
    assert_eq!(
        rt.state.center,
        CenterPane::Device,
        "the implemented (placed + routed) design opens on the device"
    );
}

/// Wheel the Flow Navigator to its end and let the scroll settle.
fn rail_to_bottom(rt: &mut Runtime<AppState>) {
    let (x, y) = rt.find("flow-rail-scroll").expect("rail scroll area");
    rt.pointer_move(x, y);
    rt.wheel(x, y, -2000.0);
    for i in 0..30 {
        rt.frame(0.1 + i as f64 * 0.05);
    }
}

#[test]
fn flow_rail_simulation_and_program_navigate() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-simulation-run");
    assert_eq!(
        rt.state.center,
        CenterPane::Sim,
        "run simulation must open sim"
    );
    assert!(rt.find("sim").is_some());
    assert_eq!(
        rt.state.flow.sim,
        RunStatus::Running,
        "run simulation must launch the worker, not just navigate"
    );
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "launch_simulation"),
        "run must echo its Tcl verb"
    );
    let mut rt = rt_for("summary");
    // At the default 900pt window the rail scrolls: wheel it down like a
    // user before reaching PROGRAM AND DEBUG.
    rail_to_bottom(&mut rt);
    tap(&mut rt, "flow-program-and-debug-generate-bitstream");
    assert_eq!(
        rt.state.center,
        CenterPane::Bitstream,
        "generate bitstream opens the bitstream report it fills"
    );
    let mut rt = rt_for("summary");
    rail_to_bottom(&mut rt);
    tap(&mut rt, "flow-program-and-debug-open-hardware-manager");
    assert_eq!(
        rt.state.center,
        CenterPane::Program,
        "hw manager must open program"
    );
}

#[test]
fn flow_rail_reports_navigate_to_timing_and_reports() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-synthesis-report-timing-summary");
    assert_eq!(
        rt.state.center,
        CenterPane::Timing,
        "synth report must open timing"
    );
    assert!(rt.find("timing-pane").is_some());
    let mut rt = rt_for("summary");
    rail_to_bottom(&mut rt);
    tap(&mut rt, "flow-implementation-report-utilization");
    assert_eq!(
        rt.state.center,
        CenterPane::Reports,
        "impl report must open reports"
    );
    assert!(rt.find("reports-pane").is_some(), "reports pane must mount");
}

#[test]
fn flow_rail_settings_and_add_sources_open_dialogs() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-project-manager-settings");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::Settings));
    assert!(rt.find("settings-dialog").is_some());
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-project-manager-add-sources");
    assert!(
        rt.state.dialogs.open.is_some(),
        "add sources must open a dialog"
    );
    assert!(
        rt.find("add-sources-dialog").is_some(),
        "add sources dialog must mount"
    );
}

#[test]
fn sources_select_loads_tab_and_double_open_navigates_to_editor() {
    use helion_rune::views::sources::SourceFileId;
    let mut rt = rt_for("summary");
    tap(&mut rt, "src-xdc");
    assert_eq!(rt.state.sources.selected, SourceFileId::Xdc);
    assert!(
        rt.state.editor.tabs.iter().any(|t| t == "project1.xdc"),
        "select must load the xdc tab"
    );
    assert_eq!(
        rt.state.center,
        CenterPane::Summary,
        "single select stays on summary"
    );
    tap(&mut rt, "src-xdc");
    assert_eq!(
        rt.state.center,
        CenterPane::Editor,
        "double open must navigate to editor"
    );
    assert!(rt.find("editor").is_some());
    // tb file needs sim_1 expanded, then loads a new tab.
    let mut rt = rt_for("summary");
    tap(&mut rt, "src-sim-set");
    tap(&mut rt, "src-tb");
    assert_eq!(rt.state.sources.selected, SourceFileId::Tb);
    assert!(
        rt.state.editor.tabs.iter().any(|t| t == "project1_tb.v"),
        "tb select must add its tab"
    );
    tap(&mut rt, "src-tb");
    assert_eq!(rt.state.center, CenterPane::Editor);
}

#[test]
fn file_menu_new_project_opens_wizard() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "menu-file");
    tap(&mut rt, "menu-file-item0");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::NewProject));
    assert!(rt.find("create-split").is_some());
}

#[test]
fn edit_menu_undo_redo_act_on_editor() {
    let mut rt = rt_for("editor");
    tap(&mut rt, "editor-gutter-3");
    let before = rt.state.editor.lines[3].clone();
    tap(&mut rt, "editor-tool-comment");
    assert_ne!(rt.state.editor.lines[3], before);
    tap(&mut rt, "menu-edit");
    tap(&mut rt, "menu-edit-item0");
    assert_eq!(
        rt.state.editor.lines[3], before,
        "edit undo must restore the line"
    );
    tap(&mut rt, "menu-edit");
    tap(&mut rt, "menu-edit-item1");
    assert_ne!(
        rt.state.editor.lines[3], before,
        "edit redo must reapply the toggle"
    );
}

#[test]
fn flow_menu_runs_flip_and_opens_navigate() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "menu-flow");
    tap(&mut rt, "menu-flow-item0");
    assert_eq!(rt.state.flow.synth, RunStatus::Running);
    let mut rt = rt_for("summary");
    // Synth, Impl, Bitstream, rule, Run Simulation, rule, Open Elaborated (6).
    tap(&mut rt, "menu-flow");
    tap(&mut rt, "menu-flow-item6");
    assert_eq!(
        rt.state.center,
        CenterPane::Schematic,
        "flow open elaborated must navigate"
    );
}

#[test]
fn view_menu_switches_center_panes() {
    let mut rt = rt_for("summary");
    // Start, Summary, rule, Editor (3), Schematic (4), ...
    tap(&mut rt, "menu-view");
    tap(&mut rt, "menu-view-item3");
    assert_eq!(
        rt.state.center,
        CenterPane::Editor,
        "view editor must navigate"
    );
    let mut rt = rt_for("summary");
    tap(&mut rt, "menu-view");
    tap(&mut rt, "menu-view-item4");
    assert_eq!(
        rt.state.center,
        CenterPane::Schematic,
        "view schematic must navigate"
    );
}

#[test]
fn window_menu_switches_console_tabs() {
    use helion_rune::views::console::ConsoleTab;
    let mut rt = rt_for("summary");
    // Dock toggles (0-3), rule, CONSOLE caption, then Tcl (6), Messages (7),
    // Log, Reports, Design Runs (10).
    tap(&mut rt, "menu-window");
    tap(&mut rt, "menu-window-item7");
    assert_eq!(rt.state.console.tab, ConsoleTab::Messages);
    assert!(rt.find("console-messages").is_some());
    let mut rt = rt_for("summary");
    tap(&mut rt, "menu-window");
    tap(&mut rt, "menu-window-item10");
    assert_eq!(rt.state.console.tab, ConsoleTab::DesignRuns);
}

#[test]
fn summary_links_navigate_to_settings() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "summary-edit");
    assert_eq!(
        rt.state.dialogs.open,
        Some(DialogId::Settings),
        "summary edit must open settings"
    );
    let mut rt = rt_for("summary");
    tap(&mut rt, "summary-synth-strategy");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::Settings));
    let mut rt = rt_for("summary");
    tap(&mut rt, "summary-impl-strategy");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::Settings));
}

#[test]
fn toolbar_back_forward_navigate_history() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "flow-rtl-analysis-open-elaborated-design");
    assert_eq!(rt.state.center, CenterPane::Schematic);
    tap(&mut rt, "flow-simulation-run");
    assert_eq!(rt.state.center, CenterPane::Sim);
    tap(&mut rt, "tool-nav-back");
    assert_eq!(
        rt.state.center,
        CenterPane::Schematic,
        "back must return to schematic"
    );
    tap(&mut rt, "tool-nav-back");
    assert_eq!(
        rt.state.center,
        CenterPane::Summary,
        "back must return to summary"
    );
    tap(&mut rt, "tool-nav-forward");
    assert_eq!(
        rt.state.center,
        CenterPane::Schematic,
        "forward must redo schematic"
    );
}

#[test]
fn toolbar_save_undo_redo_act() {
    let mut rt = rt_for("editor");
    tap(&mut rt, "editor-gutter-3");
    let before = rt.state.editor.lines[3].clone();
    tap(&mut rt, "editor-tool-comment");
    assert_ne!(rt.state.editor.lines[3], before);
    tap(&mut rt, "tool-undo");
    assert_eq!(
        rt.state.editor.lines[3], before,
        "toolbar undo must restore"
    );
    tap(&mut rt, "tool-redo");
    assert_ne!(
        rt.state.editor.lines[3], before,
        "toolbar redo must reapply"
    );
    tap(&mut rt, "tool-save");
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text.starts_with("save_file ")),
        "toolbar save must write the file and journal its path"
    );
}

#[test]
fn console_reports_navigate_to_timing_and_reports() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "console-tab-reports");
    tap(&mut rt, "console-report-link-0");
    assert_eq!(
        rt.state.center,
        CenterPane::Timing,
        "timing report must open timing"
    );
    let mut rt = rt_for("summary");
    tap(&mut rt, "console-tab-reports");
    tap(&mut rt, "console-report-link-1");
    assert_eq!(
        rt.state.center,
        CenterPane::Reports,
        "util report must open reports"
    );
}

// --- func-02-dialogs-actions journeys: every action does its real UI job ---

/// Rendered text of a keyed element (`elements.rs`: `ElementInfo::text`).
fn shown_text(rt: &Runtime<AppState>, key: &str) -> String {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .and_then(|e| e.text)
        .unwrap_or_default()
}

#[test]
fn settings_ok_persists_and_summary_shows_it() {
    let mut rt = rt_for("settings");
    tap(&mut rt, "settings-strategy-box");
    tap(&mut rt, "settings-strategy-opt-1");
    tap(&mut rt, "settings-ok");
    assert_eq!(rt.state.dialogs.open, None, "OK must close the dialog");
    assert_eq!(
        rt.state.flow.synth_strategy,
        "Helion Synthesis PerformanceOptimized"
    );
    // The summary card renders `flow.synth_strategy` directly, so the state
    // assertion plus the mounted link proves the persisted value shows.
    assert!(
        rt.find("summary-synth-strategy").is_some(),
        "summary must show the persisted strategy"
    );
}

#[test]
fn settings_cancel_drops_edits_and_keeps_committed_values() {
    let mut rt = rt_for("settings");
    tap(&mut rt, "settings-strategy-box");
    tap(&mut rt, "settings-strategy-opt-2");
    tap(&mut rt, "settings-cancel");
    assert_eq!(rt.state.flow.synth_strategy, "Helion Synthesis Defaults");
    assert_eq!(rt.state.dialogs.settings.synth_strategy, 0);
    // Let the dialog exit finish so the next tap reaches the summary link.
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    // Reopen and Apply a different value: only applied edits persist.
    tap(&mut rt, "summary-synth-strategy");
    assert_eq!(rt.state.dialogs.open, Some(DialogId::Settings));
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    tap(&mut rt, "settings-strategy-box");
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    tap(&mut rt, "settings-strategy-opt-1");
    assert_eq!(
        rt.state.dialogs.settings.synth_strategy, 1,
        "re-pick must stick"
    );
    tap(&mut rt, "settings-apply");
    assert_eq!(
        rt.state.flow.synth_strategy,
        "Helion Synthesis PerformanceOptimized"
    );
}

#[test]
fn xdc_wizard_finish_opens_generated_constraints_in_editor() {
    let mut rt = rt_for("constraints-review");
    tap(&mut rt, "xdc-finish");
    assert_eq!(rt.state.dialogs.open, None);
    assert_eq!(
        rt.state.center,
        CenterPane::Editor,
        "finish must open the constraints file"
    );
    assert!(
        rt.state
            .editor
            .tabs
            .get(rt.state.editor.active)
            .is_some_and(|t| t == "project1.xdc"),
        "the XDC tab must be active"
    );
    assert!(
        rt.state
            .editor
            .xdc_lines
            .iter()
            .any(|l| l.starts_with("create_clock -period 8.000 -name clk_sys")),
        "buffer must hold the generated clock"
    );
    assert!(
        rt.find("editor-field").is_some(),
        "generated buffer must render"
    );
    assert!(
        rt.state.editor.dirty,
        "generated text marks the buffer dirty"
    );
    // The write is undoable from the editor toolbar (after the dialog exit).
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    let len = rt.state.editor.xdc_lines.len();
    tap(&mut rt, "editor-tool-undo");
    assert!(
        rt.state.editor.xdc_lines.len() < len,
        "undo must remove the generated block"
    );
}

#[test]
fn editor_find_bar_counts_and_steps_through_matches() {
    let mut rt = rt_for("editor-find");
    let shown = shown_text(&rt, "editor-find-count");
    assert!(
        shown.contains("4 matches"),
        "count must label the hits, got: {shown:?}"
    );
    assert_eq!(
        rt.state.editor.cursor_line, 16,
        "demo cursor parks on line 16"
    );
    tap(&mut rt, "editor-find-next");
    assert_eq!(rt.state.editor.cursor_line, 19);
    tap(&mut rt, "editor-find-prev");
    assert_eq!(rt.state.editor.cursor_line, 10);
    tap(&mut rt, "editor-find-prev");
    assert_eq!(
        rt.state.editor.cursor_line, 28,
        "prev from the first match wraps to the last"
    );
    tap(&mut rt, "editor-find-close");
    assert!(
        rt.find("editor-find").is_none(),
        "close must unmount the bar"
    );
}

#[test]
fn impl_run_executes_counter_end_to_end_with_real_artifacts() {
    let src =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
    assert!(
        src.is_file(),
        "counter example must exist: {}",
        src.display()
    );
    let mut s = AppState::demo();
    // The demo project targets a Xilinx part with no Helion HAD model; the
    // run must loudly fall back to HL10T, never fake the part's results.
    s.runs.source = Some(src);
    runs::launch(&mut s, RunKind::Impl);
    assert_eq!(s.flow.impl_run, RunStatus::Running);
    assert!(s.runs.is_active(), "worker must be armed");
    let start = Instant::now();
    let timeout = Duration::from_secs(300);
    while s.runs.is_active() {
        runs::poll_runs(&mut s);
        if !s.runs.is_active() {
            break;
        }
        assert!(start.elapsed() < timeout, "impl run timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    runs::poll_runs(&mut s);
    assert_eq!(
        s.flow.impl_run,
        RunStatus::Complete,
        "counter impl must succeed"
    );
    assert_eq!(
        s.status,
        helion_rune::state::StatusVerb::ImplementationComplete
    );
    assert!(s.timing.has_run, "real timing must fill");
    assert!(s.timing.wns_ps.is_some(), "real WNS must fill");
    assert!(s.timing.endpoints > 0, "real endpoints must fill");
    assert!(s.util.luts_used > 0, "real utilization must fill");
    assert!(s.util.luts_avail > 0, "real device capacity must fill");
    let blocks = s.power.clocks_uw
        + s.power.logic_uw
        + s.power.signals_uw
        + s.power.io_uw
        + s.power.bram_uw
        + s.power.dsp_uw;
    assert!(blocks > 0, "real power blocks must fill");
    for stage in ["synth_design", "place_design", "route_design"] {
        assert!(
            s.runs.impl_log.iter().any(|l| l.text.starts_with(stage)),
            "impl log must carry the real {stage} stage line"
        );
    }
    assert!(
        s.console
            .lines
            .iter()
            .any(|l| l.text == "launch_runs impl_1"),
        "console must carry the launch verb"
    );
    assert!(
        s.toasts.items.iter().any(|t| t.kind == ToastKind::Success),
        "completion must toast success"
    );
    // The finished run is recorded and leaves a routed re-analysis context.
    assert_eq!(s.runs.history.len(), 1, "run history records the run");
    assert_eq!(s.runs.history[0].wns_ps, s.timing.wns_ps);
    assert!(
        s.runs.analysis.as_ref().is_some_and(|a| a.routed.is_some()),
        "impl runs keep the routed result for re-analysis"
    );
    assert!(
        !s.timing.clock_interaction.is_empty(),
        "report_clock_interaction runs with the summary"
    );
    // Report verbs re-run the real engines over the stored routed design.
    let wns = s.timing.wns_ps;
    s.timing = Default::default();
    helion_rune::tcl::eval_command(&mut s, "report_timing_summary");
    helion_rune::jobs::wait_idle(&mut s);
    assert_eq!(s.center, CenterPane::Timing);
    assert!(s.timing.has_run, "re-analysis refills timing");
    assert_eq!(
        s.timing.wns_ps, wns,
        "same design and constraints, same WNS"
    );
    s.drc = Default::default();
    helion_rune::tcl::eval_command(&mut s, "report_drc");
    helion_rune::jobs::wait_idle(&mut s);
    assert!(s.drc.has_run && !s.drc.pre_route, "routed DRC re-runs");
    assert!(s.drc.bitgen_allowed.is_some(), "signoff re-evaluates");
    s.power = Default::default();
    helion_rune::tcl::eval_command(&mut s, "report_power");
    helion_rune::jobs::wait_idle(&mut s);
    assert!(s.power.total_uw > 0, "power re-runs with exact rails");
    helion_rune::tcl::eval_command(&mut s, "report_methodology");
    helion_rune::jobs::wait_idle(&mut s);
    assert_eq!(s.center, CenterPane::Reports);
    assert!(
        !s.console
            .lines
            .iter()
            .any(|l| l.text.starts_with("ERROR: report_")),
        "no report verb may fail on a routed run: {:?}",
        s.console.lines.iter().rev().take(6).collect::<Vec<_>>()
    );
}

// --- func-04-tcl-editor journeys: real Tcl eval, tb buffers, disk round-trips ---

#[test]
fn tcl_console_evaluates_and_keeps_history() {
    use helion_rune::state::MsgSev;
    use helion_rune::views::console::submit_input;
    let mut rt = rt_for("summary");
    rt.state.console.input = rune::TextEdit::new("expr {7 * 6}");
    submit_input(&mut rt.state);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.text == "expr {7 * 6}"),
        "the verb must echo"
    );
    assert!(
        rt.state.console.lines.iter().any(|l| l.text == "42"),
        "the result must echo"
    );
    assert_eq!(rt.state.tcl.history, vec!["expr {7 * 6}".to_string()]);
    rt.state.console.input = rune::TextEdit::new("bogus_cmd");
    submit_input(&mut rt.state);
    assert!(
        rt.state
            .console
            .lines
            .iter()
            .any(|l| l.sev == MsgSev::Error),
        "unknown verbs must error, not silently echo"
    );
    assert_eq!(
        rt.state.tcl.history.len(),
        2,
        "history must keep every verb"
    );
}

#[test]
fn sources_tb_opens_its_buffer_and_save_round_trips_disk() {
    let mut rt = rt_for("summary");
    tap(&mut rt, "src-sim-set");
    tap(&mut rt, "src-tb");
    assert!(
        rt.state.editor.tabs.iter().any(|t| t == "project1_tb.v"),
        "tb must open a tab"
    );
    tap(&mut rt, "src-tb");
    assert_eq!(
        rt.state.center,
        CenterPane::Editor,
        "double open must navigate to editor"
    );
    assert_eq!(
        rt.state.editor.edit.text.lines().nth(2).unwrap_or(""),
        "module project1_tb;"
    );
    let path =
        std::env::temp_dir().join(format!("helion-rune-journey-tb-{}.v", std::process::id()));
    rt.state
        .editor
        .paths
        .insert("project1_tb.v".into(), path.clone());
    tap(&mut rt, "editor-tool-save");
    let disk = std::fs::read_to_string(&path).expect("save must write the file");
    assert_eq!(
        disk,
        rt.state.editor.tb_lines.join("\n"),
        "disk must match the tb buffer"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn xdc_wizard_finish_round_trips_review_content_to_disk() {
    let mut rt = rt_for("constraints-review");
    tap(&mut rt, "xdc-finish");
    assert_eq!(rt.state.dialogs.open, None);
    assert_eq!(rt.state.center, CenterPane::Editor);
    for _ in 0..8 {
        rt.frame(1.0 / 60.0);
    }
    let path = std::env::temp_dir().join(format!(
        "helion-rune-journey-xdc-{}.xdc",
        std::process::id()
    ));
    rt.state
        .editor
        .paths
        .insert("project1.xdc".into(), path.clone());
    tap(&mut rt, "editor-tool-save");
    let disk = std::fs::read_to_string(&path).expect("save must write the xdc file");
    assert!(
        disk.contains("create_clock -period 8.000 -name clk_sys"),
        "disk must hold the review content: {disk:?}"
    );
    let _ = std::fs::remove_file(&path);
}


