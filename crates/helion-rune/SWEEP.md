# egui → Rune sweep map

Loop job: every 2 min, port one uncovered old-IDE area into `helion-rune`
with tests, keep `cargo test -p helion-rune` green, rebuild + relaunch.

## Old module → new home (status)

| Old (`helion-gui`) | Lines | New home | Status |
|---|---|---|---|
| `bin/helion-ide/commands.rs` palette/menu/cheatsheet | 1048 | `views/palette.rs` | DONE (pass 1) |
| `bin/helion-ide/editor.rs` + `highlight.rs` | 1460+773 | `views/editor.rs` | done (earlier) |
| `chrome.rs` + `bin/.../flow_nav,layouts,theme,toasts,icons` | ~3.4K | `views/chrome.rs`, `theme.rs` | done (earlier) |
| run dispatch (`dispatch` fake echo) | — | `views/chrome.rs` launches real runs | FIXED (this session) |
| `bin/.../hierarchy.rs` | 516 | `views/sources.rs` | verify depth |
| `bin/.../welcome.rs` | 357 | `views/start.rs` | verify depth |
| `bin/.../dashboard.rs` | 1363 | `views/summary.rs` | verify depth |
| `constraints_wizard.rs` + `..._ui.rs` | 691+921 | `views/dialogs.rs` XDC | verify depth |
| `physview.rs` + `physview_panels.rs` | 977+569 | `views/device.rs` | verify depth |
| `bin/.../schem_lod.rs` | 238 | `views/schematic.rs` | verify depth |
| `power.rs` | 1036 | `views/reports.rs` power | verify depth |
| `qor.rs` | 1212 | `views/reports.rs`, `views/summary.rs` | verify depth |
| `ila.rs` | 1181 | partial? (`reports/summary/tcl` mention ILA) | AUDIT then port gap |
| `tb.rs` testbench | 3285 | `views/wave.rs` sim | verify depth |
| `vcd.rs` + `vcd_wave.rs` | 544+455 | `views/wave.rs` | verify depth |
| `tcl/design,interp,parse` | ~3.3K | `tcl.rs` | DONE (pass 3: full parity + `launch_simulation`) |
| `ide.rs` main panels | 45291 | distributed over views | audit per-surface |
| `surface.rs` | 1207 | n/a (test harness, not UI) | AUDITED (pass 3: nothing to port) |
| `doctor.rs` (`--doctor`/`--version`) | 123 | `doctor.rs` + `main.rs` + `build.rs` | DONE (pass 2) |
| `open_dialog.rs` | 55 | `pickers.rs` | done (earlier) |
| `lib.rs` (`GpuiShell`, synth dispatch) | 367 | `runs.rs`, `live.rs` | done (earlier) |
| `learner_l1.rs`, `learner_l2.rs` | 257+201 | none | AUDIT (likely low value) |
| `bin/.../fuzz.rs` | 1746 | none (harness, not UI) | skip unless asked |

## Known gaps (filed, not yet ported)

- `read_xdc` Tcl verb missing → palette `ExampleXdc` omitted; runs only
  discover sibling `.sdc`/`.xdc` on disk.
- Palette files mode lists open tabs only (no project-source inventory or
  recents in new IDE yet).
- `ToggleSidebar` / `ToggleDensity` omitted (no counterpart); `⌘J`, `Alt+Z`
  unbound for the same reason.
- `launch_runs` on a Tcl-read `.prj`: worker supports `.prj` (verify live).

## Pass log

- Pass 1: command palette + cheatsheet + global chords (`⌘P ⇧⌘P ⌘K ⌘O ⌘S
  ⌘1/2/3 ⌘\` ⌘↩`), Tools-menu rows, 3 scenes, 13 tests. Full suite
  276 lib + 38 snap green. Verified headless keyboard E2E
  (`Cmd+P`, type `>device`, `Enter` → Device mounted).
- Pass 2: headless `--version` / `-V` / `--doctor` (+ `version`/`doctor`
  spellings), `HELION_TARGET` build script, ported test. Full suite 277
  lib + 38 snap green. Verified live CLI output, no window. No relaunch
  (headless-only change).
- Pass 3: Tcl verb parity confirmed complete (`hds::flow` literal is
  faithful); rail Run Simulation now launches the real worker (was
  echo-only) + new `launch_simulation` verb + armed→Open relabel. 2 new
  tests, snap keys updated. Full suite 279 lib + 38 snap green.
  Relaunched (behavior change).
- Pass 4: I/O Planning slice 1 — read-only I/O Ports table in Device
  view (Name/Dir/Site/Pin/IOSTD/Drive/Slew/Pull/Diff/InTerm from live
  design + placed IOBs + port attrs; row tap cross-probes die; honest
  empty). Net-exact site match (old used substring + first-IOB
  fallback). 4 new tests, scene snap verified. Full suite 283 lib +
  38 snap green. Relaunched. REMAIN: pin-editing combos, package grid
  canvas (later passes); debugger (Memory/Breakpoints/Locals/Forces)
  is engine-ready but needs worker integration (multi-pass arc).
- Pass 5: I/O Planning slice 2 — Package Pin combos in the Device I/O
  table. The popover anchors to the row trigger (with fitted flip and
  scroll), a pick runs real `set_property LOC`, cross-probes the port,
  and constraints survive the next run via the worker snapshot.
  `set_property LOC|PACKAGE_PIN` and old `assign_package_pin` now use
  the old `set_package_pin` path: HAD IOB validation, LOC on every
  live design copy, live/flow re-place plus flow re-route, and a
  `loc=… replaced=… rerouted=…` placement result instead of a repeated
  verb. 5 new tests. Full suite 288 lib + 38 snap green. Open/pick
  snapshots verified in dark and light.
- Pass 6: I/O Planning slice 3 — HAD package grid. Added a Die/Package
  toolbar toggle; the Package view replaces die zoom/canvas/layers and
  keeps the I/O table, find, and inspector. Every HAD IOB pin is a
  tappable bank-colored ball with live placed-site/LOC occupancy; empty
  pins bind the selected port through real `set_property PACKAGE_PIN`,
  assigned pins cross-select the port, and pin selection reaches the
  inspector. Added old `package`, `package_drawing`, and
  `select_package_pin` verbs with the old drawing dump and error words.
  Also fixed two real gaps exposed by the flow: read-only object
  queries now read run-result designs, and PACKAGE_PIN `loc=` reports
  the assigned port's site/constraint rather than the first global IOB.
  5 new tests plus the `device-package` scene. Full suite 293 lib + 38
  snap green. Base/assign snapshots verified in dark and light.
- Pass 7: debugger slice 1 — persistent event-sim session and signal
  breakpoints. Added a Debug center pane with the old UG900 Id/Enabled/
  Signal/Condition/Hits table, led recipes, manual signal entry, Run 16,
  Step, Restart, and selection-gated Enable/Disable/Delete. All controls
  evaluate real Tcl verbs; runs reuse the old elaborate/start/HIT text,
  output to console/sim log, and validate against real `signal_value`.
  Added `sim_run`, `sim_step`, `sim_restart`, `breakpoints`, `add_bp`,
  `select_breakpoint`, `enable_bp`, `disable_bp`, and `delete_bp`. 7 new
  tests plus `debug` and `debug-breakpoints` scenes. Full suite 300 lib
  + 38 snap green. Empty/populated/run snapshots verified in dark and
  light. `sim_restart` preserves batch history and only appends its own
  marker. REMAIN: Memory/Locals/Forces panes, interactive-wave sync,
  fabric-kernel debugging, source-line gutter, then ILA surface audit.
