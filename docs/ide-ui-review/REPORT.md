# Helion IDE UI review

Branch `wip/ide-integration`, HEAD `67c5f17255313bc703c3e015b709d1e5eec0e465`. No merge or push was performed.

## Example run

Ran `./target/debug/helion-ide --headless examples/blinky.sv`. It parsed and synthesized `blinky`, placed and routed it, then reported WNS 9700 ps, TNS 0, one timing endpoint, and no failing setup path. The same example and implement flow were used for actual GUI framebuffer captures. The UI shows the expected USB-programmer-unavailable state on this machine; no hardware programming was attempted.

## Fixes made

- Expanded the Files/Flow Navigator tree to use its available sidebar height and removed the arbitrary nested height cap. Improved the hierarchy action layout and closed actual seams between the rail, sidebar, and work surface.
- Reworked Device actions into labeled groups with separators, removed duplicate zoom actions, corrected inspector/status sizing, removed the `???` value, and fixed the device content/scroll extent so the thumb follows real content rather than a fabricated tail.
- Corrected editor minimap thumb geometry against real scroll content and added repeatability/size regression coverage.
- Replaced narrow wrapped cable buttons with a full-width selector; made Program hardware cards use their available columns and gave the empty Logic Analyzer state a direct navigation action.
- Fixed responsive Project Summary columns, compact utilization bars, and responsive power report bars. Power resource ordering remains deterministic.
- Wrapped Methodology messages, made Clock Networks summary labels readable, removed a missing-glyph placeholder in the Constraints Wizard, and fixed the Timing Summary path grid from forcing each column to one-eighth of the full pane.
- Restricted fake native file pickers to the fuzz replay test context so crash regressions can replay without opening native dialogs.

## Screenshots

The `audit-*.png` captures are a real GUI sweep of all 27 non-duplicate workspace pages using `examples/blinky.sv` and the implement flow. A `files-after.png` and `device-after.png` are available alongside the earlier `files-before.png` and `device-before.png`. Fresh captures after the final fixes are included for Clock Networks, Methodology, QoR Suggestions, Power, and Reports. Each is a framebuffer screenshot; no mockups were used.

| Surface | Before | After / audit |
| --- | --- | --- |
| Files / Flow Navigator | [files-before](screenshots/files-before.png) | [files-after](screenshots/files-after.png) |
| Device | [device-before](screenshots/device-before.png) | [device-after](screenshots/device-after.png) |
| Clock Networks | [audit baseline](screenshots/audit-Clock_Networks.png) | [fresh render](screenshots/Clock_Networks-after.png) |
| Methodology | [audit baseline](screenshots/audit-Methodology.png) | [fresh render](screenshots/Methodology-after.png) |
| QoR Suggestions | [audit baseline](screenshots/audit-QoR_Suggestions.png) | [fresh render](screenshots/QoR_Suggestions-after.png) |
| Power | [audit baseline](screenshots/audit-Power.png) | [fresh render](screenshots/Power-after.png) |
| Reports / Timing Summary | [audit baseline](screenshots/audit-Reports.png) | [fresh render](screenshots/Reports-after.png) |

All page captures:

- [Bitstream](screenshots/audit-Bitstream.png), [Breakpoints](screenshots/audit-Breakpoints.png), [CDC](screenshots/audit-CDC.png), [Clock Interaction](screenshots/audit-Clock_Interaction.png), [Clock Networks](screenshots/audit-Clock_Networks.png), [DRC](screenshots/audit-DRC.png), [Find Results](screenshots/audit-Find_Results.png), [Force Constants](screenshots/audit-Force_Constants.png), [Hardware](screenshots/audit-Hardware.png), [IP / BD](screenshots/audit-IP___BD.png), [Locals](screenshots/audit-Locals.png), [Logic Analyzer](screenshots/audit-Logic_Analyzer.png), [Memory](screenshots/audit-Memory.png), [Methodology](screenshots/audit-Methodology.png), [Package](screenshots/audit-Package.png), [Power](screenshots/audit-Power.png), [Project Settings](screenshots/audit-Project_Settings.png), [Project Summary](screenshots/audit-Project_Summary.png), [QoR Suggestions](screenshots/audit-QoR_Suggestions.png), [Reports](screenshots/audit-Reports.png), [Schematic](screenshots/audit-Schematic.png), [Simulation Settings](screenshots/audit-Simulation_Settings.png), [Source](screenshots/audit-Source.png), [Testbench](screenshots/audit-Testbench.png), [Timing Constraints](screenshots/audit-Timing_Constraints.png), [Utilization](screenshots/audit-Utilization.png), [Waveform](screenshots/audit-Waveform.png).

## Verification

- `cargo build -p helion-gui --bin helion-ide` — passed.
- `cargo test -p helion-gui --bin helion-ide` — 27 passed, 0 failed.
- `cargo check --release -p helion-gui --bin helion-ide` — passed before the final small Timing Summary grid sizing change; that change was compiled by the successful build and test above, but the release check was not repeated afterward.
- Headless `blinky.sv` run — passed with the timing result above.
- Fuzz replay regressions are included in the 27 passing binary tests.

## Still open / not verified

- Reports' top Design Timing Summary metric strip still extends past the visible right edge in the fresh screenshot. The intra-clock path grid is fixed, but the metrics strip needs its own responsive treatment.
- The report and QoR tables are still information-dense; the QoR suggestion row is readable but not yet visually polished to an industry reference standard.
- The screenshot sweep shows every page in a representative example state. It is not the brief's exhaustive runtime click-through of all 219 controls, every resize/minimize/restore sequence, or a complete workflow acceptance pass.
- Simulation pages have no live scope/capture because this run did not execute a simulation. Hardware pages cannot verify physical programming because this machine reports no USB programmer.
- Before/after pairs are not available for every audited page; the audit sweep provides actual post-change captures, but several panels had no filed baseline screenshot.
- The pre-existing power-report library test target could not be compiled in the earlier attempt because unrelated `Spur`/`String`/lasso API errors occur in `constraints_wizard.rs`, `ide.rs`, and `qor.rs`; deterministic ordering is visible in the code but that separate target is not verified here.

No merge was made. The remaining report-strip overflow and open runtime audit mean this review is not a merge-ready sign-off.
