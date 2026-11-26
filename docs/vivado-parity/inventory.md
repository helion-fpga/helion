# Vivado parity inventory

Helion vs Vivado 2017.3, reporting surface only: every `report_` command in
UG835, every GUI window in UG893, and every analysis view and report in
UG906. Each row lists the fields Vivado shows and Helion's status, found by
grepping `crates/helion-rune`, `crates/helion-targets` and
`crates/helion-cli` on branch `fm/vivado-report-parsers` at `3d2ca22a`.

Sources:

- UG835 Vivado Design Suite Tcl Command Reference Guide, v2017.3,
  October 4, 2017. Chapter 2 lists commands by category; Chapter 3 lists
  them alphabetically with syntax and description.
- UG893 Vivado Design Suite User Guide: Using the Vivado IDE, v2017.2,
  June 7, 2017. Released without changes from v2017.1; this is the 2017.3
  lineage. Chapter 3 is the window list.
- UG906 Vivado Design Suite User Guide: Design Analysis and Closure
  Techniques, v2018.2, June 6, 2018. Its revision history says 2018.2 is
  editorial updates only, and 2018.1 added scoping notes plus three
  sections flagged below; the rest is the 2017.3 content.

Status:

- `have`: the command or view exists and runs on real Helion data.
- `partial`: it exists but covers a subset of Vivado's fields, or the data
  is shown only inside another view, or there is no standalone verb.
- `missing`: no matching verb, view or computation found in the three
  crates by grep.

Two scope notes. First, Helion computes its own reports from its own STA,
place and route; it does not read Vivado's report files. The only
Vivado-report reader in the tree is the Python parser in
`tools/vivado_reports/parse.py` (same branch), which parses Vivado 2017.3
text reports into JSON. Second, the power analysis window lives in UG912
(Power Analysis and Optimization), not in UG893 or UG906; power is covered
below only through `report_power` and the Helion power model.

## 1. UG835 report_ commands, v2017.3

The Report category in UG835 Chapter 2 holds 41 `report_` commands. A
further 13 `report_` commands sit in other categories (Power, Project,
PropertyAndParameter, Object, Simulation, Tcl/Tools, Waiver). `report_seu`
was in the 2016.3 Report list and is gone in 2017.3; `report_qor_assessment`
arrived in 2019.1 and is not a 2017.3 command. Total for 2017.3: 54.

| Command | Fields Vivado shows | Helion | Helion file |
|---|---|---|---|
| report_bus_skew | Per-bus skew, skew requirement, worst skew nets | missing | - |
| report_carry_chains | Carry chain length and distribution, chain count | missing | - |
| report_cdc | Clock-domain crossings, synchronizer type and MTBF, unsafe crossings | partial | crates/helion-rune/src/analysis.rs (cdc_text), crates/helion-rune/src/views/timing.rs (InterClock page) |
| report_clock_interaction | Clock-pair matrix: timed / unsafe / pathologically related, common primary clock, common period | partial | crates/helion-rune/src/analysis.rs (clock_interaction_text), crates/helion-rune/src/views/timing.rs (ClockInteraction page) |
| report_clock_networks | Clock tree: source, buffers, MMCM/PLL, loads, insertion delay, skew | have | crates/helion-rune/src/views/timing/clocks.rs (networks_section), crates/helion-rune/src/tcl.rs |
| report_clock_utilization | Global clocks per clock region (X1Y0-style), BUFG/BUFH/MMCM/PLL use | have | crates/helion-rune/src/views/reports/io.rs (helion_proj evaluate_clock_utilization), crates/helion-rune/src/tcl.rs |
| report_clocks | Clock definitions: name, period, waveform, source pin | missing | - (the Clock Summary page in crates/helion-rune/src/views/timing.rs shows the same clock list, but there is no report_clocks verb) |
| report_compile_order | Compile order of design sources | missing | - |
| report_config_timing | Configuration (bitstream) timing | missing | - |
| report_control_sets | Total and unique control sets, unused register locations | have | crates/helion-rune/src/views/reports/io.rs (helion_proj report_control_sets_design), crates/helion-rune/src/tcl.rs |
| report_datasheet | I/O timing per port: setup, hold, clock-to-out | missing | - |
| report_debug_core | Debug cores (ILA, VIO) and their properties | missing | - |
| report_design_analysis | Timing / complexity / congestion analysis: logic-level distribution, fanout, congestion contributors | missing | - |
| report_disable_timing | Disabled timing arcs and their reasons | missing | - |
| report_drc | Rule ID, severity, description, violation count (REPORT SUMMARY key:values) | partial | crates/helion-targets/src/view_drc.rs (NSTD-1, UCIO-1, BIVC-1, MDRV-1, LUTLP-1, RTSTAT-1, ZPS7-1 plus HLN- rules; rules without view data are listed not run), crates/helion-rune/src/tcl.rs |
| report_environment | OS, Vivado version, memory, CPU | missing | - |
| report_exceptions | False paths, multicycle paths, max/min delay exceptions, ignored and overridden exceptions | missing | - |
| report_feasibility | Placement feasibility: I/O, clocking, Pblock fit | missing | - |
| report_frames | Simulation frames | missing | - |
| report_high_fanout_nets | Net, fanout, clock flag, driver; largest first | partial | crates/helion-targets/src/view_xilinx7.rs (fill_high_fanout, top 10 in the implementation view; also view_ecp5.rs, view_hl10.rs, view_ice40.rs); no standalone verb |
| report_hw_mig | MIG calibration and debug status | missing | - |
| report_incremental_reuse | Incremental reuse percentage per object | missing | - |
| report_io | Package pin, I/O standard, bank, drive strength, slew, direction | missing | - |
| report_methodology | Check, severity, objects, message (TIMING-*, XDCB-*, SYNTH-* checks) | partial | crates/helion-rune/src/analysis.rs (rerun_methodology), crates/helion-rune/src/views/timing.rs (method_table); check set is Helion's own, smaller than Vivado's deck |
| report_objects | Simulation objects | missing | - |
| report_operating_conditions | Voltage, temperature, process corner | missing | - (an OperatingConditions type exists in crates/helion-cli/src/main.rs; no report verb) |
| report_param | Parameter name and value | missing | - |
| report_phys_opt | Physical optimization results | missing | - |
| report_pipeline_analysis | Pipeline analysis results | missing | - |
| report_power | Total on-chip, dynamic, device static, junction temperature, confidence, on-chip breakdown by class | partial | crates/helion-targets/src/view_hl10.rs (fixed uW/MHz/resource model, no toggle rate, no per-net activity, no per-tile attribution), crates/helion-cli/src/main.rs (helion report_power), crates/helion-rune/src/tcl.rs |
| report_power_opt | Power optimization savings per technique | missing | - |
| report_pr_configuration_analysis | Partial reconfiguration configuration analysis | missing | - |
| report_property | Object property names and values, optional glob pattern | have | crates/helion-rune/src/tcl.rs (cmd_report_property) |
| report_pulse_width | Minimum pulse width per clock, high/low width, violations | missing | - |
| report_qor_suggestions | RQA score, suggestion list with severity, category and Tcl command | have | crates/helion-rune/src/views/reports.rs and crates/helion-rune/src/views/summary.rs (evaluation in crates/helion-proj/src/qor.rs) |
| report_ram_utilization | RAMB/URAM inference: depth, width, count, utilization | missing | - |
| report_route_status | Routed / unrouted nets, congestion, route DRC | missing | - |
| report_scopes | Simulation scopes | missing | - |
| report_sdx_utilization | SDx (HLS) utilization | missing | - |
| report_sim_device | Simulation device information | missing | - |
| report_simlib_info | Simulation library information | missing | - |
| report_ssn | Simultaneous switching noise per bank, SSO count, limits | have | crates/helion-rune/src/views/reports/io.rs (helion_proj report_ssn_design, helion-device analyze_ssn), crates/helion-rune/src/tcl.rs |
| report_stacks | Simulation stacks | missing | - |
| report_switching_activity | Toggle rate per net | missing | - |
| report_synchronizer_mtbf | Synchronizer MTBF | missing | - |
| report_timing | Per-path slack: setup/hold, source, destination, logic levels, delay breakdown | have | crates/helion-rune/src/tcl.rs (report_timing verb), crates/helion-cli/src/main.rs (helion report_timing) |
| report_timing_summary | WNS, TNS, WHS, THS, WPWS, TPWS, failing/total endpoints, per-clock table, intra-clock table | have | crates/helion-rune/src/tcl.rs (report_timing_summary verb), crates/helion-rune/src/views/timing.rs (Summary page); parsed from Vivado output by tools/vivado_reports/parse.py |
| report_transformed_primitives | Primitives transformed by optimization | missing | - |
| report_utilization | Slice LUTs, LUT as Logic, LUT as Memory, Slice Registers, F7/F8 muxes, Block RAM Tile, RAMB36/18, DSPs, Bonded IOB, BUFGCTRL, MMCME2_ADV; used / available / percent | have | crates/helion-targets/src/view_xilinx7.rs (fill_resources), crates/helion-targets/src/reports.rs (utilization), crates/helion-cli/src/main.rs (helion report_utilization) |
| report_values | Simulation values | missing | - |
| report_waivers | Waived violations and their waiver rules | missing | - |
| report_bps | Simulation breakpoints | missing | - |
| report_conditions | Simulation conditions | missing | - |
| report_drivers | Simulation drivers | missing | - |

Counts: have 13, partial 7, missing 34 of 54.

## 2. UG893 windows, v2017.2

Chapter 3, Using Windows. "Working with Windows", "Data Table Windows"
and "Workspace" are chrome behaviors rather than data views; they are
listed for completeness.

| Window | Fields Vivado shows | Helion | Helion file |
|---|---|---|---|
| Working with Windows | Dock, float, split, maximize, layout presets | have | crates/helion-rune/src/views/chrome.rs, crates/helion-rune/src/layout.rs |
| Data Table Windows | Sortable, filterable tables with column chooser | have | crates/helion-rune/src/views/report_kit.rs |
| Project Summary | Part, top, strategy, run status, utilization and timing headlines | have | crates/helion-rune/src/views/summary.rs |
| Sources Window | Source files, hierarchy, constraints, simulation sources | have | crates/helion-rune/src/views/sources.rs |
| Netlist Window | Design hierarchy as processed by synthesis | partial | crates/helion-rune/src/views/sources.rs (Hierarchy tab); no separate post-synthesis netlist window |
| Device Constraints Window | XDC constraints: clocks, I/O, placement | have | crates/helion-rune/src/views/constraints.rs, crates/helion-rune/src/views/xdc_wizard.rs |
| Properties Window | Properties of the selected object | partial | crates/helion-rune/src/views/sources.rs (property_rows); no dedicated Properties window |
| Run Properties Window | Run name, strategy, status, directory, report configs | have | crates/helion-rune/src/views/runs_table.rs, crates/helion-rune/src/runs.rs |
| Selection Window | Currently selected objects and their properties | partial | selection exists via crates/helion-rune/src/views/find.rs and select verbs; no dedicated Selection window file found |
| Workspace | Main canvas area holding the open windows | have | crates/helion-rune/src/views/chrome.rs, crates/helion-rune/src/layout.rs |
| Text Editor | Source text with syntax highlighting, diagnostics, gutter | have | crates/helion-rune/src/views/editor.rs |
| Device Window | Placed cells, routing, tiles, sites, congestion and criticality heat | have | crates/helion-rune/src/views/device.rs (congestion/criticality heat over tiles) |
| Package Window | Package pins, banks, I/O placement on the package | have | crates/helion-rune/src/views/device_package.rs |
| Schematic Window | Cells, nets, pins, hierarchy cones, timing paths | have | crates/helion-rune/src/views/schematic.rs, crates/helion-rune/src/views/schematic_cone.rs, crates/helion-rune/src/views/schematic_flow.rs |
| Hierarchy Window | Module hierarchy tree | have | crates/helion-rune/src/views/sources.rs (Hierarchy tab) |
| Timing Constraints Window | Clocks, exceptions, I/O delays | have | crates/helion-rune/src/views/constraints.rs, crates/helion-rune/src/views/xdc_wizard.rs |
| Waveform Window | Signals, values over time, cursors, dividers | have | crates/helion-rune/src/views/wave.rs |
| Tcl Console | Command entry, history, output | have | crates/helion-rune/src/views/console.rs (Tcl tab), crates/helion-rune/src/tcl.rs |
| Messages Window | Info, warning, critical warning, error messages | have | crates/helion-rune/src/views/console.rs (Messages tab) |
| Log Window | Full session log | have | crates/helion-rune/src/views/console.rs (Log tab) |
| Reports Window | Generated reports, report configs, open report | have | crates/helion-rune/src/views/reports.rs, crates/helion-rune/src/views/console.rs (Reports tab) |
| Design Runs Window | Runs, status, strategy, elapsed time, WNS/TNS per run | have | crates/helion-rune/src/views/runs_table.rs, crates/helion-rune/src/runs.rs, crates/helion-rune/src/views/console.rs (DesignRuns tab) |
| Package Pins Window | Package pins, signal, bank, I/O standard | have | crates/helion-rune/src/views/device_io.rs, crates/helion-rune/src/views/device_package.rs |
| I/O Ports Window | Ports, direction, standard, package pin, bank | have | crates/helion-rune/src/views/device_io.rs |

Counts: have 20, partial 4, missing 0 of 24.

## 3. UG906 analysis views and reports, v2018.2

Chapter 1 is logic analysis, Chapter 2 timing analysis, Chapter 3
implementation results, Chapter 4 reports and messages, Chapter 5 timing
analysis basics, Chapter 6 synthesis closure, Chapter 7 implementation
closure, Appendix A the TIMING-1 to TIMING-30 methodology checks.

| View / report | Fields Vivado shows | Helion | Helion file |
|---|---|---|---|
| Netlist Window (Ch.1) | Post-synthesis hierarchy | partial | crates/helion-rune/src/views/sources.rs (Hierarchy tab) |
| Hierarchy Window (Ch.1) | Module hierarchy tree | have | crates/helion-rune/src/views/sources.rs (Hierarchy tab) |
| Utilization Report (Ch.1) | Used / available / percent per site type | have | crates/helion-targets/src/reports.rs (utilization), crates/helion-targets/src/view_xilinx7.rs |
| Schematic Window (Ch.1) | Cells, nets, pins, cones | have | crates/helion-rune/src/views/schematic.rs |
| Find Dialog Box (Ch.1) | Object search by type, name, property | have | crates/helion-rune/src/views/find.rs |
| Device Utilization Statistics (Ch.1) | Utilization per device region and site type | have | crates/helion-rune/src/views/device.rs, crates/helion-targets/src/reports.rs |
| Report DRC (Ch.1) | DRC violations by rule and severity | partial | crates/helion-targets/src/view_drc.rs |
| Methodology DRCs (Ch.1) | TIMING-1 to TIMING-30 checks, severity, objects | partial | crates/helion-rune/src/analysis.rs (methodology rows), crates/helion-rune/src/views/timing.rs |
| Report Timing Summary (Ch.2) | WNS/TNS/WHS/THS/WPWS/TPWS, per-clock and intra-clock tables | have | crates/helion-rune/src/views/timing.rs, crates/helion-rune/src/tcl.rs |
| Report Clock Networks (Ch.2) | Clock tree, insertion delay, skew, loads | have | crates/helion-rune/src/views/timing/clocks.rs |
| Report Clock Interaction (Ch.2) | Clock-pair matrix, common primary clock, common period | partial | crates/helion-rune/src/analysis.rs, crates/helion-rune/src/views/timing.rs |
| Report Pulse Width (Ch.2) | Min pulse width per clock, high/low width, violations | missing | - |
| Report Timing (Ch.2) | Per-path slack, delay breakdown | have | crates/helion-rune/src/tcl.rs, crates/helion-cli/src/main.rs |
| Report Datasheet (Ch.2) | I/O setup/hold/clock-to-out per port | missing | - |
| Report Exceptions (Ch.2) | False paths, multicycle, max/min delay, ignored exceptions | missing | - |
| Report Exceptions in the Vivado IDE (Ch.2) | Exceptions table in the IDE | missing | - (added in UG906 2018.1, not in 2017.3) |
| Report Clock Domain Crossings (Ch.2) | CDC paths, synchronizer type, MTBF | partial | crates/helion-rune/src/analysis.rs (cdc_text), crates/helion-rune/src/views/timing.rs (InterClock page) |
| Report Bus Skew (Ch.2) | Bus skew per bus, skew requirement | missing | - |
| Design Runs Window (Ch.3) | Runs, status, strategy, WNS/TNS per run | have | crates/helion-rune/src/views/runs_table.rs, crates/helion-rune/src/runs.rs |
| Placement Analysis (Ch.3) | Placement density, congestion, utilization by region | partial | crates/helion-rune/src/views/device.rs (congestion/criticality heat); no placement density or congestion report |
| Routing Analysis (Ch.3) | Routed/unrouted nets, congestion, route DRC | partial | crates/helion-rune/src/views/device.rs (routed nets shown); no route-status report |
| Report Design Analysis (Ch.3) | Timing / complexity / congestion analysis | missing | - |
| Report QoR Suggestions (Ch.3) | RQA score, suggestions with severity and Tcl command | have | crates/helion-rune/src/views/reports.rs, crates/helion-rune/src/views/summary.rs (evaluation in crates/helion-proj/src/qor.rs) |
| Reports and Messages (Ch.4) | Report list, message severities, message management | have | crates/helion-rune/src/views/reports.rs, crates/helion-rune/src/views/console.rs |
| Generating and Waiving Design Checks (Ch.4) | Waive DRC/methodology checks, waiver rules | missing | - (added in UG906 2018.1, not in 2017.3; report_waivers is also missing) |
| Configurable Report Strategies (Ch.4) | Report configs per run step | missing | - |
| Creating Design Related Reports (Ch.4) | Report generation per design stage | partial | crates/helion-rune/src/views/reports.rs, crates/helion-cli/src/main.rs (helion reports) |
| Reading a Timing Path Report (Ch.5) | Path slack, arrival/required time, logic and net delay | have | crates/helion-rune/src/views/timing/paths.rs |
| Verifying Timing Signoff (Ch.5) | Signoff summary: WNS/TNS, unconstrained paths, exceptions | partial | crates/helion-rune/src/views/timing.rs (Summary page); no separate signoff checklist |
| Elaborated View (Ch.6) | RTL as elaborated, before synthesis | missing | - (Helion schematic shows the synthesized netlist only) |
| Longest Logic Delay Paths (Ch.7) | Worst logic-delay paths, levels, delay | partial | crates/helion-rune/src/views/timing/paths.rs (critical paths); no dedicated longest-delay analysis |
| High Fanout Net Drivers (Ch.7) | Nets by fanout, driver, clock flag | partial | crates/helion-targets/src/view_xilinx7.rs (fill_high_fanout) |
| Hold-Fixing Impact (Ch.7) | Hold-fixing cost vs setup | missing | - |
| Analyzing All Failing Paths (Ch.7) | Every failing path, grouped by clock | partial | crates/helion-rune/src/views/timing.rs (failing paths by clock) |
| Floorplanning (Ch.7) | Pblocks, placement constraints on the device | partial | crates/helion-rune/src/views/device_floor.rs (create/resize/add-cells/get pblocks, report_pblocks) |
| TIMING-1 to TIMING-30 checks (App. A) | Methodology check ID, severity, description | partial | crates/helion-rune/src/analysis.rs (Helion's own methodology rows); Vivado's full TIMING-* deck is not reproduced |

Counts: have 14, partial 15, missing 9 of 38 (the two 2018.1 additions
excluded from the 2017.3 count).

## 4. Notes

- The parser in `tools/vivado_reports/parse.py` reads Vivado 2017.3
  text for seven reports: timing summary, utilization, power, clock
  utilization, control sets, DRC, methodology. Of the 54 UG835 commands,
  those seven are the only ones whose Vivado output Helion can ingest;
  the rest of Helion's reports are computed from its own data.
- The biggest missing clusters are I/O and pin planning (report_io,
  report_datasheet, report_ssn is have), timing exceptions and pulse
  width (report_exceptions, report_pulse_width, report_bus_skew,
  report_disable_timing), route status (report_route_status), design
  analysis (report_design_analysis, report_ram_utilization,
  report_carry_chains), simulation reports (report_frames,
  report_stacks, report_bps, report_conditions, report_drivers,
  report_objects, report_scopes, report_simlib_info, report_values),
  and waivers (report_waivers, generate/waive checks).
- UG893 windows are the strongest area: 20 of 24 have, the rest partial,
  none missing. The IDE chrome (device, package, schematic, timing,
  runs, reports, console) is the part of Vivado Helion has most fully
  reproduced.
- UG906 is mixed: timing and runs are have, while pulse width,
  datasheet, exceptions, bus skew, design analysis and the elaborated
  view are missing, and clock interaction, CDC, placement and routing
  analysis are partial.
