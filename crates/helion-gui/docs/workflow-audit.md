# Helion IDE workflow audit (analysis phase)

Walked each core workflow as a new user would, through the headless
`IdeModel` (`exec` verbs = what the Tcl console runs; the GUI buttons call
the same model methods) plus reading the paint code in
`crates/helion-gui/src/bin/helion-ide.rs` (+ `flow_nav.rs`,
`constraints_wizard_ui.rs`, `welcome.rs`, `commands.rs`) for the exact
click-path a user takes today.

Driver: `crates/helion-gui/tests/workflow_walk.rs` (scratch, uncommitted).
Each finding below was reproduced with the steps shown, or read directly
off the code path cited. Nothing here is invented.

Severity key:
- BROKEN: the workflow does not complete; the user is stuck or misled.
- DEAD-END: no way forward from this point in the UI.
- UX GAP: works, but only via an undiscoverable action.
- MISLEAD: the UI says one thing and does another.
- CODE-HEALTH: structural problem in the workflow's code path (duplication,
  tangled state, God-function). Per the Oct 13 code-quality rule, fixes here
  must refactor the structure, not layer hacks.

## WF1: Open project -> synthesized -> placed -> routed -> bitstream

Click-path today: Welcome page ("Open file…" / "New project…" / example
cards / "Run gold flow") or File > Open… (`open_hdl_dialog`, rfd). Flow
menu > Implement, or Flow Navigator sidebar (per-stage Run / Settings /
Open results). After Implement succeeds the canvas auto-switches to Device
(`poll_job`: `set_canvas(Canvas::Device)` on `JobKind::Implement`).

### Findings

1. [MISLEAD] Welcome page "Run gold flow" hover text says
   "counter.sv → synth → place → route → bitstream", but `implement_inner`
   (ide.rs) runs Synthesis → Opt → Place → Route only; Bitstream is a
   separate manual step. Vivado parity would be fine (Generate Bitstream is
   separate there too) - the bug is the hover text over-promising.
   (welcome.rs:195, ide.rs:8632)

2. [UX GAP / divergence] `open_source` auto-runs Synthesis on every file
   open (ide.rs:8158 `open_source_impl` → `run_step_from(Synthesis)`).
   Vivado/Quartus never synthesize on file-add; the user explicitly launches
   it. Consequence: the Flow Navigator's Synthesis row shows Done before the
   user ever pressed Run, and "Re-run" is the only synth action. The staged
   mental model (add sources → review → synthesize) does not exist.

3. [UX GAP] No explicit top-module selection: the top is implicitly the
   last-opened file (`open_source` → synth that path; `tree.top` only
   mirrors the result, ide.rs:22647). Sibling `.sv`/`.v` files in the same
   directory ARE auto-discovered (`sibling_modules` in helion-sv), so
   multi-file works when opened in any order - reproduced: opening sub.sv
   then top2.sv, or top2.sv alone, both synthesize working "top2"
   (driver `wf1b_multifile_open`, `wf1c_multifile_top_first`). What is
   missing vs Vivado/Quartus: (a) no "Set as Top" action - with two
   candidate tops the design silently becomes whichever file was opened
   last; (b) sources in different directories are not elaborated together
   (only the opened file + its own directory's siblings).

4. [MISLEAD] "Implement" means two different things. The GUI's Implement
   button / Flow menu (`JobKind::Implement` → `implement()`) runs
   Synthesis → Opt → Place → Route, stopping before Bitstream. The
   runs-queue `launch_runs impl_1` runs Opt → Place → Route → Bitstream
   (ide.rs:13627). A user who runs impl_1 from Design Runs gets a bitstream;
   a user who clicks Implement does not. One verb should mean one flow.

5. [CODE-HEALTH] `submit_job` clones the whole `IdeModel` and `poll_job`
   adopts the finished clone, merging back only `run_queue` + `runs`.
   Anything the user does while a flow step runs (Tcl console input +
   journal rows, SDC editor edits, ILA probe setup, waveform cursors,
   workspace tab switches) is silently discarded on completion.
   (bin/helion-ide.rs:632 `submit_job`, :698 `poll_job`.) Vivado/Quartus
   stay fully interactive during runs. The structural fix is surgical
   state transfer (jobs operate on session/engine state; UI state never
   crosses the thread boundary), not a bigger merge list.

6. [UX GAP] Every Flow Navigator stage's "Settings" row opens the same
   generic Project Settings page (`paint_stage_body` →
   `open_more_workspace(Settings)`). There are no per-stage settings
   (synth effort, place strategy, etc.); those live in Design Runs
   strategies, undiscoverable from the stage row. Vivado's per-stage
   Settings open that stage's options.

## WF2: Synth -> timing report

Click-path today: Timing activity (rail) → "Timing Paths" section →
"Report timing" button (`model.exec("report_timing")`); empty state has the
same CTA. Dashboard → "Implemented Timing Report" link → Reports tab.
Flow/Tools menus → "Report timing" Tcl recipe. Console: `report_timing`,
`timing_summary`, `timing_path_table`.

### Findings

1. [UX GAP] `report_timing` before/without routing auto-pulls place+route
   (`report_timing_now`: pulls place/route if needed). That is friendly, but
   a new user clicking "Report timing" on the Timing canvas after only
   opening a file gets a full place+route with no indication that is what
   the button just did - the status bar shows job progress, but the button
   label does not say it will implement the design. (ide.rs:20531)

2. [UX GAP] No timing-staleness tracking. After constraints change + Save,
   the Timing view keeps showing the old report with no "out of date"
   marker; the user must know to press "Report timing" again. Vivado marks
   timing results out-of-date when constraints change. (No `timing_stale`
   state exists in ide.rs.)

## WF3: Testbench -> run -> waveform viewed

Click-path today: Simulate activity → Testbench pane (auto-rescans on first
paint). Rescan / Run all / per-TB Run / "VCD dump" checkbox / Stop.
Detail pane: failures (click → editor, "sch" → schematic), live log, and
"Open in waveform viewer" (only when a VCD exists from the last run).

### Findings

1. [BROKEN / DEAD-END] "Open in waveform viewer" and the `tb_wave` verb do
   not open anything. `tb_open_vcd_in_waveform` (ide.rs:21044) sets
   `tb_waveform_pending` and returns
   "waveform viewer (stub): queued <path> - the full viewer ships with VCD
   power". Nothing ever reads `tb_waveform_pending` (grep: written at
   21045, merged at 21069, asserted in one test at 41837, never consumed).
   The Waveform viewer never opens; the VCD is never loaded. GTKWave parity
   is the entire point of this workflow. Repro in driver `wf3_testbench_to_waveform`.
   Phase-2 fix needs a real VCD→WaveTrace importer (`vcd::parse_vcd`
   returns only aggregate `SignalActivity`, no time series; the TB's
   `write_vcd` samples are not retained). Do NOT hack this with a partial
   importer in this phase.

2. [UX GAP] `tb_vcd_enabled` defaults to false (ide.rs:5998). A new user
   runs the testbench, it passes, and there is no waveform path at all -
   the "Open in waveform viewer" button only appears after a run WITH the
   checkbox ticked beforehand. Vivado/Questa always produce waveforms from
   a sim run. The empty-state help does not mention the checkbox.

3. [MISLEAD, minor] `tb_last_vcd` is global, but the "Open in waveform
   viewer" button lives in the per-testbench detail pane. Run TB-A with VCD,
   select TB-B (never run with VCD): the button offers A's VCD under B's
   header.

4. [CODE-HEALTH] Testbench has two run paths: `tb_run_sync` (exec/tests,
   blocking) and GUI `tb_start_run` → `JobKind::TbRun` (engine thread,
   streamed). Both funnel into `tb::run_testbench`, which is the right
   shape; the hazard is the whole-model clone (WF1.5) wiping the
   streamed log merge edge cases. Keep the funnel; fix the state transfer.

## WF4: Constraints entry -> applied -> reflected in timing

Click-path today: More … → Constraints tab (or `timing_constraints`):
"Constraints Wizard…" button (Clocks → Input delays → Output delays →
Exceptions → Review stepper, writes SDC next to the first source), Add…
menu (Clock 10ns / Input delay / Output delay / False path recipes +
"Read counter.sdc"), SDC text editor + Open/Save. Console:
`create_clock …`, `open_sdc_editor`, `save_sdc_editor`.

### Findings

1. [BROKEN] The `open_sdc_editor` Tcl verb hardcodes
   `examples/counter.sdc` (ide.rs:6927-6929), ignoring the open project.
   A user working on their own design who types `open_sdc_editor` gets the
   shipped example file - and `open_sdc_editor` CREATES it with stock
   content if missing, i.e. it will write into the examples dir. The GUI
   path resolves correctly (sibling of first source → `target_path` in
   constraints_wizard_ui.rs). Three different "which SDC?" resolutions
   exist: exec verb (hardcoded), GUI tab (`sdc_editor_path` → sibling
   fallback → counter.sdc), wizard `target_path` (sibling of first source).
   Repro in driver `wf4_constraints_to_timing`.

2. [UX GAP] Nothing tells the user timing is stale after Save (see WF2.2);
   the workflow "constraints applied -> reflected in timing" requires the
   user to know to re-run Report timing manually.

3. [CODE-HEALTH] The three SDC path resolutions above are the duplication
   to eliminate: one `resolve_sdc_target(&model) -> PathBuf` used by the
   verb, the tab, and the wizard. The wizard's `write_sdc` append path and
   the tab's Save path also both re-parse via `read_xdc_path` - fine, keep
   the funnel, unify the target.

## WF5: ILA setup capture -> run -> viewed capture

Click-path today: TWO parallel systems.
- GUI: More … → ILA ("Logic Analyzer" pane, `paint_ila`): net search list
  with "+" per net, probe list with radix, trigger config, "Arm Capture"
  (runs on a capture thread, never the egui frame), Export VCD / Save /
  Load session. Uses `model.sim_ila` → `WorkspaceTab::Ila`.
- Console: `ila_dashboard` → `add_probe <net>` → `ila_arm <net> [window]`
  → `capture_ila` (soft sim capture, backend "soft_ble_out") → lands on
  `WorkspaceTab::Hardware`, pushes an `ila:<net>` trace into the main
  waveform. Uses `model.ila` (`IlaDashboard`).

### Findings

1. [CODE-HEALTH / MISLEAD] Two ILA implementations with separate state
   (`IlaDashboard` at ide.rs:4056 vs `SimIla` at ide.rs:4101), separate
   verbs, separate landing tabs (Hardware vs Ila), separate capture
   viewers. A console user and a GUI user get different "ILA"s; `ila_dashboard`
   reports the old system's status while the GUI pane shows the new
   system's. Structural fix: one ILA backend (`ila.rs` already hosts
   `IlaConfig`/`IlaCapture`/viewer types); retire `IlaDashboard`+`capture_ila`
   or re-home the verbs onto `sim_ila`. Do not add a third.

2. [CODE-HEALTH] `SimIla` tangles view state into the model: `search`,
   `group: Vec<bool>`, `trig_value_text`, `trig_mask_text`,
   `session_path`, `bus_name` are text-field/checkbox states living on
   `IdeModel`. They get cloned across the engine-thread boundary on every
   job and wiped on adoption (see WF1.5). View state belongs in the binary
   (`HelionIde`), not the model.

## WF6: Tcl console - type a command -> visible result

Click-path today: bottom "Console" tab (the inline Tcl console; the
floating "Tcl" window is Cmd+Backtick), `helion%` prompt, Enter submits
(`submit_input` → `exec` → `journal` → row in the #/Status/Cmd/Out grid).
Up/Down walks history. Find box searches the journal. Edit menu → Tcl
console, Tools menu → Tcl console.

### Findings

1. Works: typed commands journal with status pills and truncated output;
   errors show red with the message; history works. The console is the most
   complete workflow of the six.

2. [UX GAP] While a flow job runs, the console accepts input but the whole
   model is replaced on job completion (WF1.5) - typed-but-unsubmitted input
   and any commands run mid-job vanish. The console should either disable
   input while busy or survive adoption.

## Cross-cutting code-health (engine-crate standard)

For reference, `helion-gui` vs the engine crates (small focused crates,
narrow APIs):

- `ide.rs`: ~25k lines production + ~17k lines tests (197 tests) in ONE
  module. `exec()` alone is ~1040 lines / 306 `else if` branches
  (ide.rs:6287-7327) - a dispatch God-function. Engine standard: a command
  table/registry (verb → handler fn), one verb per small function, verbs
  grouped by domain module (flow, timing, tb, ila, wave, constraints).
- `bin/helion-ide.rs`: 12,365 lines - every pane's paint code in one file.
  `paint_testbench`/`paint_ila`/`paint_timing_paths` etc. are already
  extractable (the `bin/helion-ide/*.rs` split has started: flow_nav,
  welcome, dashboard…); finish it per-pane.
- View state on the model: `SimIla` text fields (above), `console_find`,
  `input`, wizard UI state partly in `app.constraints_wizard` (good - that
  one is already in the binary). Rule: `IdeModel` = session/design/engine
  state; `HelionIde` = view state. The engine-thread clone boundary is the
  forcing function - anything view-only must not cross it.
- `tb_waveform_pending`: write-only state (see WF3.1). Delete or wire;
  never leave write-only model fields.
- Positive examples already at engine standard: `tb.rs` (discovery →
  parse → interpret → run → VCD → history, narrow pub API) and `ila.rs`
  (pure `find_trigger`/`apply_window`, `IlaConfig`/`IlaCapture` types).
  Phase-2 refactors should converge ON these modules, not around them:
  the ILA unification keeps `ila.rs` as the single backend and retires
  `IlaDashboard` + `capture_ila`/`insert_arm_capture` from ide.rs.

## Vivado / Quartus / VS Code / GTKWave comparison notes

- WF1: Vivado separates add-sources from launch-runs; Helion synthesizes on
  open (friendlier, but the Flow Navigator should then not present synth as
  a user-run stage). Bitstream as a separate explicit step matches Vivado.
- WF2: Quartus/Vivado both have one-click timing reports post-route and
  stale markers; Helion has the button but no stale marker, and the button
  silently runs place+route when they are missing.
- WF3: VS Code's test explorer + Vivado's sim flow always yield waveforms;
  Helion's dead-ends at a stub message. This is the largest parity gap.
- WF4: Vivado's constraints wizard + SDC editor parity is actually good in
  Helion's GUI; the Tcl verb is the broken part.
- WF5: Vivado has ONE ILA flow (set up debug cores → program → trigger →
  capture → waveform). Helion has two that disagree.
- WF6: Vivado's Tcl console journals every command including flow steps;
  Helion matches this (flow jobs journal via the adopted clone), except
  mid-run console activity is lost to the model adoption.

## Status

- Analysis phase: no UI/production code changed, no modules refactored.
  Per the phase rule (fixes land after the layout grammar), every
  structural problem below is logged for phase 2 WITH the refactor shape,
  so the fix is never a hack on messy code:
  - WF3.1 (VCD→waveform importer): new `vcd_to_wave` importer in `vcd.rs`
    (extend `parse_vcd` to retain time series or parse on demand), wired
    into `tb_open_vcd_in_waveform`; delete the write-only
    `tb_waveform_pending` field.
  - WF1.5 (surgical job state transfer): jobs take `Session`/engine state,
    not a whole-`IdeModel` clone; `HelionIde` keeps view state
    (`input`, `console`, `sdc_editor_text`, `SimIla` text fields…).
  - WF5.1 (unify ILA): retire `IlaDashboard` + `capture_ila` from ide.rs;
    re-home `add_probe`/`ila_arm`/`ila_dashboard` verbs onto `sim_ila`
    (`ila.rs` stays the single backend).
  - WF4.1 (unify SDC target): one `resolve_sdc_target()` on the model used
    by the exec verb, the Constraints tab, and the wizard's `target_path`.
  - `exec()` God-function (306 branches): command table/registry, one verb
    per small fn, grouped by domain module.
- Driver: `crates/helion-gui/tests/workflow_walk.rs` (scratch; not committed).
- Fixes: none landed in this phase.
