# egui → Rune sweep map

Loop job: every 2 min, port one uncovered old-IDE area into `helion-rune`
with tests, keep `cargo test -p helion-rune` green, rebuild + relaunch.

## Old module → new home (status)

| Old (`helion-gui`) | New home | Status |
|---|---|---|
| `commands.rs` palette/menu/cheatsheet | `views/palette.rs`, `views/chrome.rs` | DONE — every menu row real; palette reaches all 16 panes, project files, report verbs |
| `editor.rs` + `highlight.rs` | `views/editor.rs`, `views/highlight.rs` | DONE — SV/VHDL/Tcl lexer, find+replace, Ln/Col, per-file buffers, cut/copy |
| `chrome.rs`, `flow_nav`, `layouts`, `toasts` | `views/chrome.rs`, `layout.rs`, `views/status.rs` | DONE — resizable/hideable docks, presets, focus mode, prefs; rail with live run status |
| `hierarchy.rs` (sources) | `project.rs`, `views/sources.rs` | DONE — real project files from `.prj`; Add Sources persists to the `.prj` |
| `welcome.rs` | `views/start.rs` | DONE — launcher, recents, bundled example; boot lands here |
| `dashboard.rs` | `views/summary.rs` | DONE — run history, worst endpoints, real Run Implementation |
| `constraints_wizard*` | `views/dialogs.rs`, `views/constraints.rs` | PARTIAL — Constraints pane DONE; wizard still single-row per kind |
| `physview*` | `views/device*.rs` | DONE — site-kind layers, congestion/criticality heat, pblocks overlay + table, rulers, canvas keys, I/O quick-set; no clock regions (HAD has no region table) |
| `schem_lod.rs` / schematic | `views/schematic*.rs` | DONE — fan-in/out cones (hops, stop-at-FF), expand/collapse cone + inside, selection history, Fit, sheet find, Mark Debug/Add Probe |
| `power.rs` | `views/reports.rs`, `views/reports/vec_power.rs`, `helion_sta::vcd_power` | DONE — vectorless rails/blocks plus VCD-driven power (engine moved to helion-sta) |
| `qor.rs` | `views/reports.rs` | DONE (helion-proj QoR engine) |
| `ila.rs` | `helion-verif`, `views/ila*.rs` | DONE |
| `tb.rs` | `helion-verif`, `views/tb*.rs` | DONE |
| `vcd.rs` + `vcd_wave.rs` | `views/wave*.rs`, `helion-verif::wave` | DONE — live VCD scopes/objects, radix + fixed-point + analog step/linear, AXI-Lite/SPI/UART/FSM decoder lanes, virtual buses, lane reorder/remove, density, object filter, Shift/Alt-click A/B |
| hardware / bitstream / IP-BD / find panes | `views/program.rs`, `views/bitstream.rs`, `views/ip*.rs`, `views/find.rs` | DONE |
| timing / CDC / clock interaction / reports | `views/timing.rs`, `views/reports.rs`, `analysis.rs` | DONE |
| `tcl/*` | `tcl.rs` | DONE |
| `doctor.rs`, `open_dialog.rs`, `lib.rs` | `doctor.rs`, `pickers.rs`, `runs.rs` | DONE |
| `surface.rs`, `fuzz.rs`, `learner_*` | — | not UI; skipped |

## Remaining gaps

- Device: no clock-region layer/table (no HAD region model); no pblock
  delete (the `.prj` format has no delete command). Ranged pblocks now
  steer placement (`helion_place::place_with_regions`) and `PBLK-1` DRC
  flags members outside their pblock.
- ECO: the ECO view edits the routed netlist of the last run (INIT,
  reconnect, buffer, add/remove cell, route-only) and writes a full plus
  incremental `.pbits` bitstream; the session is rebuilt from the run, not
  persisted across restarts. Checkpoints live under the temp dir.
- DFX: partitions live in the session (no `.prj` DFX commands); static
  routing is re-run per module (no locked static routes in helion-route).
- Coverage: testbench line/branch plus DUT netlist toggle/mux/FSM; DUT
  line coverage needs source-line attribution in helion-sv (absent).
- VHDL simulation: SV testbenches drive VHDL DUTs; VHDL testbenches
  (`wait for`, `report`, signal `after`) need a behavioral VHDL
  interpreter (helion-vhdl drops them during synthesis).
- UVM: needs classes, inheritance, parameterized classes, virtual
  methods, macros, queues/dynamic/associative arrays, fork/join, virtual
  interfaces and a randomize() front end; none exist in helion-sv or the
  testbench interpreter (helion-sim's CRV solver is a Rust API only).
  `$urandom`, `$urandom_range` and `randcase` are supported. helion-proj bug: reopening a seeded `.hckp` resets
  the seed to 0 and fails its hash check.
- Waveform tools are verified on real runs (`examples/wave_tools`
  bus_periph_tb: SPI/UART/FSM/AXI-Lite decode; counter.sv netlist bits as a
  virtual bus); on the demo waveform they stay disabled with a reason.
  helion-sv: a later nonblocking assignment in a nested `if` did not
  override an earlier one to the same register (bus_periph is written
  with if/else to avoid it).
- VHDL diagnostics are rare (the VHDL frontend reports few errors).
- Program over openFPGALoader/FTDI is wired but untested on a board; no
  STAT readback over openFPGALoader.
- Re-analysis reads the last run's design: the Constraints Wizard's
  re-run needs a Synthesis/Implementation run first.

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
  marker.
- Pass 8: debugger slice 2 — Memory/Locals/Forces panes. The Debug
  surface is now four UG900 tabs over one shared Run/Step/Restart
  transport: Breakpoints (unchanged table), Memory (LUT INIT ROMs plus
  the packed `seq` word, with a Contents section and word selection),
  Locals (live led/register probes with scope filtering), and Force
  Constants (Force/Deposit/Run recipes, selection-gated Remove). Tab
  taps evaluate the real open verbs, so `memory`/`locals` elaborate
  the engine instead of showing hollow panes. Added `memory`,
  `memories`, `select_memory`, `select_memory_word`, `locals`,
  `select_local`, `forces`, `force_table`, `force_constants`,
  `add_force`, `deposit`, `remove_force`, and `select_force` with the
  old dump text, selection rules, value/radix/time parsing, and error
  words. Forces drive the real engine: holds flatten the LED waveform
  until released, deposits poke once, `-start_time`/`-cancel_after`
  flip Pending/Active/Released across runs, and restart re-arms
  surviving holds. 10 new tests plus `debug-memory`, `debug-locals`,
  and `debug-forces` scenes. Full suite 310 lib + 38 snap green.
  All three panes snap-verified in dark and light. REMAIN:
  source-gutter line breakpoints, interactive-wave sync (trace
  mounting, scopes/objects/add_wave), fabric-kernel debugging, then
  ILA surface audit.
- Pass 9 (shell + parallel ports): real shell (layout/prefs, menus,
  status bar, toasts, Start page, project files, per-file editor buffers,
  SV/VHDL/Tcl highlighting, console filter, honest flow rail), then three
  merged ports: analysis (clock interaction, path table/detail, histogram,
  CDC/clock nets, report re-runs, Constraints pane, run history),
  hardware (helion-hw Hardware Manager, Bitstream FAR table, IP Integrator,
  Find Results) and verification (helion-verif ILA + testbench runner, sim
  settings, honest wave pane). Implementation strategy presets drive the
  run worker. 401 lib + 41 snap + 32 helion-verif green.
- Pass 10 (physical): Schematic cones (`expand_fanin_cone`,
  `expand_fanout_cone`, `expand_cone`, `collapse_cone`, `expand_inside`,
  `collapse_inside`, `schematic_previous/next`, `schematic_zoom_fit`) with a
  Name/Type/Hops results strip, find, history, Mark Debug/Add Probe. Device
  layers split by site kind, routed congestion + STA criticality heat maps,
  pblocks (`create_pblock`, `resize_pblock`, `add_cells_to_pblock`,
  `get_pblocks`, `report_pblocks`, `select_pblock`; persisted to the open
  `.prj`), rulers, canvas keys, Zoom to Selection, real Fit, I/O quick-set
  and Placed/Objects columns. Scenes `schematic-cone`, `device-heat`,
  `device-pblocks`, `device-io-quickset`.
- Pass 10 (round two, merged): schematic cones/history/find/fit + mark
  debug; device site/heat-map/pblock/ruler layers, pblock table, canvas
  keys, I/O quick-set; waveform fixed-point/analog, SPI/UART/AXI-Lite/FSM
  decoders, virtual bus, trace edit, density; Design Runs table, run
  creation/queue/seed sweep, checkpoints, incremental; five-step
  Constraints Wizard with port validation, write-to-file and WNS
  before/after; editor virtualization, outline, parse diagnostics,
  bookmarks, go to definition, match case; report re-runs and program_hw
  on worker jobs; runs read a `.prj`'s declared constraint files.
  466 lib + 41 snap + 42 helion-verif green.
