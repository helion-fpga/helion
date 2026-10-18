# Helion IDE Layout Grammar

The placement system for the Helion IDE. Every control in the IDE must live
in exactly one zone, for a reason stated here. No control gets to live
somewhere "because it fit." This document is phase 1 of the IDE bug-hunt:
grammar first, surface fixes later. It changes no UI code.

Status: ratified grammar, pending per-surface migration (see section 7).

---

## 1. Reference zone systems

Every claim below is checkable against the cited source. Helion's zones
(section 2) are mapped to these references, and every deliberate difference
is called out in section 8.

### 1.1 VS Code

Five zones, optimized to maximize editor space:

- **Activity Bar**: thin vertical strip on the far left. Switches what the
  Side Bar shows (Explorer, Search, Source Control, Run and Debug,
  Extensions). Clicking the active icon toggles the Side Bar.
- **Side Bar** (Primary Side Bar): left, next to the Activity Bar. Shows the
  current view, usually the file tree. Collapsible (`Ctrl+B`).
- **Editor**: the middle. Open files as tabs, splittable into Editor Groups.
  A breadcrumbs bar shows the file path under the tabs.
- **Panel**: bottom (or right). Terminal, Problems, Output, Debug Console.
  Toggled with `Ctrl+J`. Context-sensitive contents switch with the active
  view.
- **Status Bar**: the bottom edge. Git branch, error/warning counts, line and
  column, language mode. Background task progress (e.g. builds) surfaces here.
- Command Palette (`Ctrl+Shift+P`): keyboard access to all commands.

Sources: VS Code UI tour (garagedocs vscode-tour.mdx,
https://github.com/frc4451/garagedocs/blob/HEAD/src/content/tools/vscode/vscode-tour.mdx);
"Introduction to VS Code" (https://dev.to/fannynyayic/introduction-to-vs-code-4ke4);
O'Reilly "Visual Studio Code Distilled" ch. 2
(https://www.oreilly.com/library/view/visual-studio-code/9781484242247/html/474995_1_En_2_Chapter.xhtml).

### 1.2 Vivado (UG893, "Using the Vivado IDE")

The Vivado viewing environment has ten named components (UG893 Chapter 2,
"Using the Viewing Environment"):

1. Menu Bar, 2. Main Toolbar, 3. Flow Navigator, 4. Data Windows Area,
5. Menu Command Quick Access Search Field, 6. Workspace, 7. Project Status
Bar, 8. Layout Selector, 9. Status Bar, 10. Results Windows Area.

Zone semantics that matter for Helion:

- **Flow Navigator** (left): the push-button design flow, from HDL input to
  bitstream generation. Launches predefined steps (synthesis,
  implementation, Generate Bitstream). WP416 notes the design goal: an
  intuitive toolbar guides designers through setup and implementation steps.
- **Data Windows Area** (left of center): Sources window (Design Sources,
  Constraints, Simulation Sources, with hierarchy), Properties.
- **Workspace** (center): windows needing screen space: Text Editor,
  Schematic window, Device window, Package window.
- **Results Windows Area** (bottom): where command status and results land.
  Default windows: **Tcl Console** (enter Tcl commands, view history),
  **Messages** (by process and severity), **Log** (synthesis/implementation/
  simulation run logs), **Reports** (quick access to flow reports),
  **Design Runs** (manage runs for the current project).
- **Project Status Bar / Status Bar** (bottom edge): design state summary.
- **Layout Selector**: saved window arrangements, a first-class IDE control.

Every GUI action issues an equivalent Tcl command (WP416), so the console is
the automation backbone, not an accessory.

Sources: UG893 (v2020.1)
(https://www.xilinx.com:443/support/documents/sw_manuals/xilinx2020_1/ug893-vivado-ide.pdf);
UG892 design flows overview; UT Austin EE382N lecture notes
(https://users.ece.utexas.edu/~mcdermot/arch/lectures/Lecture_2.pdf).

### 1.3 Quartus Prime

- **Project Navigator** (left, top): design hierarchy tree; double-clicking a
  module opens its file and highlights it (cross-probing into the editor).
- **Tasks** (left, middle): one-click launch of common project tasks:
  creating design files, project settings, running compilation, debug,
  timing closure, device programming. The Compilation Dashboard gives per-
  stage controls, settings, and reports.
- **Messages** (bottom): compilation messages.
- **IP Catalog** (right): parameterizable IP list.
- **Compilation Report**: opens as a tab in the large central area after
  compilation, with sections listed on the left side of the report.
- The large area on the right is shared by the Text Editor and compilation/
  simulation results.

Sources: Intel Quartus Prime Pro Edition User Guide: Getting Started
(UG-20129); Compilation Dashboard
(https://www.intel.com/content/www/us/en/programmable/quartushelp/24.1/global/global/gl_tab_compilation_dashboard.htm);
UF EEL 3701 Quartus 22.1 tutorial
(https://mil.ufl.edu/3701/docs/quartus/quartus_22.1_tutorial.pdf).

### 1.4 Apple Human Interface Guidelines

Layout and scroll-view guidance (applies to macOS, Helion's primary target):

- **Group related items** with space, separators, or background; keep content
  and controls clearly distinct.
- **Extend content to fill the window.** Scrollable layouts continue to the
  bottom and sides; controls sit on top of content, not in carved-out slabs.
- **Place items by importance** in reading order (top to bottom, leading to
  trailing). Most important items go near the top and leading side.
- **Differentiate controls from content.** Use scroll edge effects for
  content-to-control transitions, not heavy backgrounds.
- **Progressive disclosure**: hint at hidden content (disclosure controls,
  partial item display) rather than crowding everything on screen.
- **Sidebars** are for navigation and organization of content; **inspectors**
  show details of the current selection.
- Scroll views: one scroll edge effect per view; in split layouts keep pane
  effects consistent in height to maintain alignment. Small or mini scroll
  bars are acceptable in tight panels.

Sources: Apple HIG "Layout"
(https://developer.apple.com/design/human-interface-guidelines/layout) and
"Scroll views"
(https://developer.apple.com/design/human-interface-guidelines/scroll-views);
mirrored in
https://github.com/milistu/agent-skills/blob/HEAD/skills/apple-human-interface-guidelines/reference/foundations/layout-guidelines.md.

### 1.5 GNOME HIG / libadwaita

- **Utility panes** pattern
  (https://developer.gnome.org/hig/patterns/containers/utility-panes.html):
  sidebars and inspectors are secondary to the main content view; the content
  view dominates.
- Adaptive breakpoints (libadwaita): at <= 400sp, split views collapse to a
  single navigable pane and sidebars become overlays; at <= 550sp, view
  switchers move from the header bar to a bottom bar; at <= 860sp, the outer
  pane of nested split views collapses. Sidebar sizing: ~25% of total width,
  180-280sp.
  (https://github.com/eljijuna/gnome-ui/blob/HEAD/ROADMAP.md)
- Dialog guidance (GNOME HIG 1.0): arrange controls in reading order, put the
  main interacting controls near the top-leading corner, give dialogs sensible
  defaults, and keep whole-dialog buttons in one bottom row.

Source: GNOME HIG 1.0
(https://blog-geofcrowl-static-images.s3.us-east-1.amazonaws.com/2020-02-17-collection-higs/gnome-hig-1.0.pdf).

### 1.6 Material Design 3 (adaptive)

- **Design the window, not the device.** The same app crosses size classes via
  split screen, freeform windows, rotation, folding. Window conditions drive
  runtime layout.
- Official window size classes: width Compact < 600dp, Medium 600-839dp,
  Expanded >= 840dp; height Compact < 480dp, Medium 480-899dp, Expanded
  >= 900dp.
- Adaptive behavior may change navigation form, pane count, density, or action
  placement, not just sizes. A large layout should expose useful relationships
  or reduce navigation cost rather than merely enlarging whitespace.

Source:
https://github.com/zsh2722/work-skill/blob/HEAD/skills/compose-adaptive/references/adaptive-design-guide.md
(summarizing Material 3 adaptive guidance).

---

## 2. Helion's zones

Nine zones plus the start-page state. Each zone has one job. A control lives
in a zone only if the zone's job is the control's job.

```
+------------------------------------------------------------------+
| Z1 Toolbar: window-wide commands (left) + flow cluster (right)   |
+------+-------------+--------------------------------+------------+
| Z2   | Z3          | Z5 Primary content (canvas)    | Z4         |
| Act. | Navigation  |                                | Inspector  |
| rail | sidebar     |  one document/view at a time   | (selection,|
|      | (one only)  |                                | on demand) |
|      |             +--------------------------------+            |
|      |             | Z6 Bottom panel: console/jobs  |            |
+------+-------------+--------------------------------+------------+
| Z7 Status bar: read-only summary + job progress                  |
+------------------------------------------------------------------+
  Z8 Toasts: overlay, bottom-right above Z7
  Z9 Dialogs: modal overlay, focused tasks
  Z10 Start page: Z5 state when no project is open
```

### Z1 Toolbar (top, 44px)

Window-wide commands and the flow launcher. Left cluster: project commands
(Open, Recent, Examples, New Project). Right-pinned cluster: the flow chips
(Synth, Opt, Place, Route) and Implement, plus the Layout preset selector.
Overflow goes into a `>>` menu (VS Code pattern, already implemented as
`overflow_toolbar`), never into truncation.

Reference mapping: Vivado Main Toolbar + Menu Bar (Helion has no menu bar;
window-wide commands live here instead, difference D4). The right-pinned
flow cluster is Helion's answer to Vivado's Flow Navigator (difference D2)
and WP416's "intuitive toolbar guides the designers" push-button flow.

Belongs here: project open/create, flow launch, layout presets, window-wide
search/command entry.
Never here: job progress (Z7, rule P5), canvas-specific editing controls
(they live in the canvas's own toolbar strip inside Z5), selection actions
(Z4).

### Z2 Activity rail (left, 88px)

Six activities: Files, Device, Timing, Simulate, Program, Reports. The rail
switches what Z3 shows and which canvas mode Z5 is in. Click-only; `⌘1/⌘2/⌘3`
belong to the three canvases. A `More ...` menu holds the remaining
`WorkspaceTab`s. File-opening example items never appear on the rail.

Reference mapping: VS Code Activity Bar. Vivado and Quartus have no rail;
their view switching is via the Flow Navigator / View menu (difference D1).

Belongs here: activity switching only.
Never here: file-open actions, flow launch buttons, status readouts.

### Z3 Navigation sidebar (left, 220px, resizable 160-420)

Exactly one sidebar. Its content switches with the rail activity:

- Files: flow navigator mini-list + source file tree + (moving out) nothing
  else. Selection detail moves to Z4.
- Device: I/O ports table, device navigation.
- Program: cables/targets, bitstream artifacts.
- Reports: report catalog (the report list that today lives here).
- Timing, Simulate: hidden. Timing analysis lives in the Timing canvas
  (no catalog twin); simulation scopes live in-canvas in Simulate mode.

Reference mapping: VS Code Primary Side Bar; Vivado Data Windows Area;
Quartus Project Navigator + Tasks stacked.

Belongs here: navigation trees, lists that select what Z5 shows, launch
lists (flow steps, cables, report catalog).
Never here: canvases, selection property grids (Z4), command output (Z6).

### Z4 Inspector (right, 220px, on selection only)

Shows details of the current selection: the Properties grid (Name/Value),
Mark Debug / Add Probe actions, pin detail. Hidden when there is no
selection. This is the `PROPERTIES_WIDTH` dock the code already names.

Reference mapping: Apple HIG inspector ("inspectors show details of the
current selection"); GNOME trailing utility pane; Vivado Properties window.

Belongs here: selection properties and selection-scoped actions.
Never here: navigation (Z3), always-on content, window-wide commands.

### Z5 Primary content (center canvas)

One document or view at a time, filling the leftover rect. Canvases:
Editor (`⌘1`), Device (`⌘2`), Timing (`⌘3`). Every other `WorkspaceTab`
(Schematic, Package, Hierarchy, IP, Bitstream, Hardware, report details,
Settings, Find, Summary, and all Simulate-mode panes) paints its real pane
here via `pane_for_workspace`. More destinations never fall through to
Timing; that is a routing contract, not a layout choice.

Reference mapping: VS Code Editor Groups; Vivado Workspace (Text Editor,
Schematic, Device, Package windows); Quartus large central area.

Belongs here: documents, viewers, report details, dashboards.
Never here: navigation chrome, command output, global status.

### Z6 Bottom panel (resizable, 40-420px, collapsible)

Tabbed: Console (Tcl), Messages, Sim log (Simulate only), Jobs (Runs +
Checkpoints; see review item R12/R13). Collapses to its tab strip; max
height is capped so it cannot steal the canvas. Context-sensitive tabs
follow the VS Code Panel pattern.

Reference mapping: VS Code Panel (Terminal, Problems, Output, Debug
Console); Vivado Results Windows Area (Tcl Console, Messages, Log, Reports,
**Design Runs**); Quartus Messages window.

Belongs here: command console, message/log streams, run/job management.
Never here: primary documents, navigation, window-wide launch buttons.

### Z7 Status bar (bottom edge, 22px)

Read-only summary: project breadcrumb (Activity › Canvas), timing honesty
label, error/warning counts, job progress strip, selection summary.
Progress lives here (VS Code pattern; the project AGENTS.md already states
"Job progress lives in the status bar"). No primary actions; items may open
menus or navigate but never execute flows.

Reference mapping: VS Code Status Bar; Vivado Status Bar + Project Status
Bar; Quartus Status window.

Belongs here: summary readouts, background-task progress.
Never here: primary action buttons, navigation trees, command input.

### Z8 Toasts (overlay, bottom-right above Z7)

Completions and errors only. Max 360px wide, max 4 stacked, deduped,
5s wall-clock expiry. No "Started" toasts, no progress. (Already the rule in
`bin/helion-ide/toasts.rs`; the grammar adopts it.)

Reference mapping: VS Code notifications; Apple HIG transient feedback.

### Z9 Dialogs (modal overlay)

Focused tasks: Create Project wizard, file open (OS dialog via rfd),
Examples picker. A dialog does one task and closes; it is never a
full-window replacement for a zone.

Reference mapping: GNOME HIG dialog guidance (reading order, main controls
top-leading, sensible defaults, whole-dialog buttons in one bottom row).

### Z10 Start page (Z5 state, no project open)

When no project is open, Z5 shows the start page: recent projects, examples,
new-project action (the `welcome::paint_welcome` view). It is a central-pane
state, not a rail item, not a dialog, not a canvas.

Reference mapping: VS Code welcome page (an editor tab); Vivado start page
/ Project Summary on open.

---

## 3. Placement rules (the laws)

- **P1. One sidebar, one job.** Z3 holds navigation only: trees, lists,
  launch items. Selection detail lives in Z4, never in Z3. A column never
  mixes navigation and inspector content.
- **P2. The rail switches, never acts.** Z2 changes what Z3 shows and which
  canvas mode Z5 is in. It holds no file-open actions, no flow buttons, no
  readouts.
- **P3. Every tab paints its real pane.** `pane_for_workspace` maps each
  `WorkspaceTab` to a real `WorkspacePane` painted in Z5. More destinations
  never collapse to Timing, never to a stub.
- **P4. Jobs live in Z6.** Run management (design runs, checkpoints, run
  logs) belongs in the bottom panel, tabbed next to the console that drives
  it. This is the Vivado Results Windows Area pattern: Design Runs sits
  beside Tcl Console and Messages.
- **P5. Progress lives in Z7.** Background job progress (the progress strip)
  is a status-bar element. Z1 launches flows; Z7 reports their progress.
- **P6. Toolbar discipline.** Z1: window-wide commands left, flow cluster
  right-pinned, overflow into `>>`. Nothing canvas-specific, no progress, no
  navigation.
- **P7. Toasts are completions.** Z8: completions/errors only. No "Started",
  no progress bars, no persistent state.
- **P8. Dialogs do one task.** Z9: focused task, then close. Never a zone
  replacement.
- **P9. Selection detail goes right.** Properties of the current selection go
  to Z4 (on selection only). Z3 must not grow a properties section.
- **P10. One scroll owner per zone.** Nested scroll lists must not chain to
  the outer scroll. A zone that overflows scrolls internally; the window
  never double-scrolls.
- **P11. Start page is a Z5 state.** With no project open, Z5 shows Z10. It
  is not reachable from the rail and it is not modal.
- **P12. Simulate is a mode.** Under the Simulate rail activity, Z3 hides
  and Z5 splits into the wave canvas plus in-canvas scoped navigation
  (testbenches, scopes). Debug panes (Source, Memory, Breakpoints, Locals,
  Forces, SimSettings, Testbench, ILA) are Z5 panes in this mode. This is a
  deliberate difference from VS Code's Run-and-Debug sidebar (D3).

---

## 4. Ordering rules within zones

- Z1, left to right: project commands (Open, Recent, Examples, New) |
  separator | Layout presets | spacer | flow cluster (Synth, Opt, Place,
  Route, Implement). Reading order = importance order (Apple HIG): the
  irreversible, project-scoped actions sit where the eye lands first; the
  flow cluster is right-pinned so it survives narrowing.
- Z2, top to bottom: Files, Device, Timing, Simulate, Program, Reports, then
  `More ...`. Frequency order: authoring first, analysis later, programming
  last (matches the design-flow direction).
- Z3, per activity, top to bottom: launch/navigation (what selects) before
  detail (what describes). Example Files: flow navigator, then file tree.
- Z5 canvas strip: Editor, Device, Timing (`⌘1/⌘2/⌘3`), then `More ...`.
  One row, never wrapped (wrap steals canvas; `OverflowMode::Wrap` is
  banned by the chrome contract).
- Z6 tabs, left to right: Console, Messages, Sim log (Simulate only), Jobs.
  Console first: the Tcl console is the automation backbone (Vivado WP416:
  every GUI action has a Tcl equivalent).
- Z7, left to right: breadcrumb (Activity › Canvas), timing honesty label,
  issue counts | spacer | job progress, selection summary. Summary left,
  transient right.
- Z4: "Properties" heading, selection-scoped actions (Mark Debug, Add
  Probe), then the Name/Value grid. Actions before data.

---

## 5. Responsive rules (window-size tiers)

Tiers are driven by window size, not device (Material: "design the window,
not the device"). Helion's existing chrome math (`share_available`,
`device_band_share`, `chrome_at`) already implements most of this; the
grammar states the intended end state.

| Tier | Width | Height | Behavior |
|---|---|---|---|
| Wide | >= 1280 | any | Full chrome: Z2 + Z3 (220) + Z5 + Z4 on selection + Z6. |
| Medium | 840-1279 | any | Z3 narrows toward its 160 floor; Z1 items overflow into `>>`; Z6 capped at 26% of body. |
| Narrow | < 840 | any | Z3 becomes an overlay drawer (rail stays); Z4 becomes an overlay; Z1 keeps project commands + `>>`, flow cluster stays right-pinned. Canvas keeps >= 80% width. |
| Short | any | < 560 | Z6 floors to its tab strip (~40px, user-draggable up); Z5 scrolls internally; nothing paints past the window (P10). |
| Tiny | < 480 | < 400 | Rail stays (88px, always selectable); Z3/Z4 overlays only; Z5 + Z6 tab strip; Z7 keeps breadcrumb + issues, progress collapses to a spinner. |

Reference anchors: Material 3 width classes (Compact < 600dp, Medium
600-839dp, Expanded >= 840dp; height Compact < 480dp) inform the tier
boundaries. GNOME libadwaita (<= 400sp single-pane + overlay sidebars,
<= 550sp switchers to bottom bar, <= 860sp collapse outer panes) informs the
overlay behavior. VS Code informs the collapse primitives: `Ctrl+B`-style
sidebar toggle, `Ctrl+J`-style panel toggle, panel movable bottom/right.

VS Code panel-reflow analog: like VS Code's Panel (bottom or right,
toggleable, resizable, context tabs), Z6 is toggleable and resizable, its
height is capped, and its tab set changes with context (Sim log appears only
under Simulate). Like VS Code's Side Bar, Z3 is collapsible per activity
(Timing and Simulate hide it today; Narrow tier generalizes that).

---

## 6. Zone reference map (summary)

| Helion zone | VS Code | Vivado (UG893) | Quartus | HIG anchor |
|---|---|---|---|---|
| Z1 Toolbar | Title bar + editor actions | 1 Menu Bar, 2 Main Toolbar | Toolbar | Apple: importance in reading order |
| Z2 Activity rail | Activity Bar | (none; D1) | (none; D1) | Material: navigation form adapts |
| Z3 Navigation sidebar | Primary Side Bar | 4 Data Windows Area | Project Navigator + Tasks | Apple: sidebars navigate; GNOME utility panes |
| Z4 Inspector | (none) | Properties window | (none) | Apple: inspectors detail selection |
| Z5 Primary content | Editor Groups | 6 Workspace | Central area | Apple: content fills window |
| Z6 Bottom panel | Panel | 10 Results Windows Area | Messages window | VS Code/Vivado precedent |
| Z7 Status bar | Status Bar | 7/9 Project/Status Bar | Status window | VS Code: summary + task progress |
| Z8 Toasts | Notifications | (none) | (none) | Apple: transient feedback |
| Z9 Dialogs | Dialogs | Dialogs | Dialogs | GNOME: dialog layout guidance |
| Z10 Start page | Welcome page | Start/Project Summary | (none) | VS Code welcome tab |

---

## 7. Per-surface placement review

Every current IDE surface, judged against sections 2-4. "OK" means the
current placement already follows the grammar. "MOVES" names the required
destination and the rule that requires it. Code anchors are the current
paint sites (`crates/helion-gui/src/bin/helion-ide.rs` unless noted).

### 7.1 Surfaces that follow the grammar (no move)

| Surface | Current placement | Zone | Why it follows |
|---|---|---|---|
| Start page (welcome) | Z5 state when no project (`welcome::paint_welcome`, helion-ide.rs:1003) | Z10 | P11: central-pane state, not a rail item or dialog. |
| Editor | Canvas `⌘1` | Z5 | Primary document; maximal space (VS Code Editor). |
| Device / physical view | Canvas `⌘2`; IO ports in Z3 under Device | Z5 + Z3 | Vivado Device window = workspace viewer; ports table is device navigation (Z3). |
| Schematic | More -> Z5 pane (`pane_for_workspace`) | Z5 | Vivado Schematic window = workspace viewer; P3 (real pane, no fall-through). |
| Waveform | Z5 under Simulate mode | Z5 | P12: waveform-class canvas in Simulate mode. |
| Timing canvas (summary + paths) | Canvas `⌘3` | Z5 | Current-design analysis document; no Z3 twin (P1). |
| Timing report details (Constraints, Clock Interaction, CDC, Clock Networks, Power, Methodology, QoR, DRC, Utilization) | More -> Z5 panes under Reports rail (`is_report_detail`) | Z5 | Vivado: reports generated by the flow are opened in workspace viewers; catalog lives in Z3 (Reports), details in Z5. |
| VCD power | Section inside the Power report pane | Z5 | Report-detail composition, not its own zone; no move. |
| Tcl console | Z6 tab "Console" (`paint_tcl_console`) | Z6 | Vivado Results Windows Area: Tcl Console; VS Code Panel terminal. |
| Messages | Z6 tab (`paint_messages`) | Z6 | Vivado Messages; Quartus Messages window; VS Code Problems. |
| Sim log | Z6 tab, Simulate only (`paint_sim_log`) | Z6 | Context-switched panel tabs (VS Code Panel pattern). |
| Testbench runner | Sim-only Z5 pane under Simulate | Z5 | P12: part of Simulate mode. |
| ILA | Sim-only Z5 pane under Simulate | Z5 | P12: waveform-class canvas in Simulate mode. |
| Sim debug panes (Source, Memory, Breakpoints, Locals, Forces, SimSettings) | Sim-only Z5 panes | Z5 | P12 (deliberate difference D3 documented). |
| Package view | More -> Z5 under Device | Z5 | Vivado Package window = workspace viewer. |
| IP integrator | More -> Z5 | Z5 | Workspace viewer. |
| Hardware manager (Program dashboard) | Z5 under Program rail | Z5 | Primary content for the Program activity. |
| Bitstream view | More -> Z5 under Program | Z5 | Bitstream generation view = document. |
| Summary dashboard | `WorkspaceTab::Summary` -> Z5 (More) | Z5 | Vivado Project Summary = workspace viewer; should be the project-open landing. |
| Settings | More -> Z5 | Z5 | VS Code settings = editor tab (primary content). |
| Find | More -> Z5 | Z5 | Results document; primary content. |
| Program sidebar (cables/targets) | Z3 under Program (`paint_program_side`) | Z3 | Quartus Tasks pattern: launch/navigation list in the side column. |
| Reports catalog | Z3 under Reports | Z3 | Navigation list selecting what Z5 shows (P1). |
| Toasts | Z8 bottom-right above Z7 (`toasts.rs`) | Z8 | P7 already enforced: completions/errors only. |
| Create Project wizard | Z9 `Window` overlay | Z9 | P8: focused task, then close. |
| Examples picker | Popup from Z1 | Z9-ish | Focused pick-one-task; fine as a popup. |
| Open HDL dialog (rfd) | OS dialog | Z9 | Platform file dialog; correct. |
| Layout preset selector | Z1 toolbar (`layouts::paint_layout_selector`) | Z1 | Vivado UG893 component 8, Layout Selector, is a first-class toolbar control. |
| Flow chips + Implement | Z1 right-pinned cluster | Z1 | P6; WP416 push-button flow (D2). |
| Recent / Examples / New menus | Z1 left cluster | Z1 | P6: window-wide commands. |
| Status bar contents (breadcrumb, honesty label, issue counts) | Z7 (`paint_status_bar`) | Z7 | VS Code Status Bar pattern. |

### 7.2 Surfaces that violate the grammar (must move)

**R12. Design Runs: Z5 -> Z6.** Currently `paint_runs` is a More -> Z5 pane
(`is_report_detail` includes `WorkspaceTab::Runs`). P4 requires run
management in the bottom panel: Vivado puts Design Runs in the Results
Windows Area, next to the Tcl Console and Messages that drive it. A runs
table is job management, not a document. Destination: a "Jobs" tab in Z6,
sharing the tab with checkpoints (R13).

**R13. Checkpoints: Z5 -> Z6.** Currently `paint_checkpoints` is a More ->
Z5 report-detail pane. Checkpoints are snapshots of runs; they are managed
alongside runs, not read as reports. P4 moves them to Z6 with Runs (same
"Jobs" tab or an adjacent tab). Vivado precedent: checkpoint/run state is
managed from the Design Runs area, not from a workspace viewer.

**R14. Properties dock: Z3 -> Z4.** Currently `paint_properties` is appended
at the bottom of the navigation sidebar (`paint_sidebar`, helion-ide.rs:1610
region). P1 forbids mixing inspector content into the navigation column, and
P9 sends selection detail to the right inspector. The code already defines
`PROPERTIES_WIDTH` as "shown on selection, not always-on": the grammar
adopts that as Z4, shown only when a selection exists. Destination: right
dock Z4 with the Properties grid and the Mark Debug / Add Probe actions.

**R15. Hierarchy: Z5 (More) -> Z3.** Currently `paint_hierarchy` is a More ->
Z5 central pane. Hierarchy is structural navigation: Vivado shows hierarchy
in the Sources window (Data Windows Area); Quartus shows the module tree in
the Project Navigator (top-left), and double-clicking cross-probes into the
editor. P1 places navigation in Z3. Destination: a hierarchy tree section in
Z3, under the Files activity (RTL hierarchy) and the Device activity
(netlist/placed hierarchy), cross-probing into the Z5 canvas on selection.

**R16. Progress strip: Z1 -> Z7.** Currently `paint_progress_strip` is called
from inside `paint_toolbar` (helion-ide.rs:1372). P5 requires job progress in
the status bar: Z1 launches flows, Z7 reports their progress (VS Code
pattern; the project AGENTS.md already states "Job progress lives in the
status bar" but the paint path contradicts it). Destination: Z7 status bar,
right side, next to the selection summary.

### 7.3 Notes (no move, but grammar constrains future work)

- **N1.** Testbench runner controls (Run/Step/Restart) must cluster in the
  Simulate canvas's own toolbar strip inside Z5, not duplicate in Z1 (P6).
- **N2.** The Timing rail activity keeps Z3 hidden only while the Timing
  canvas owns all timing navigation. If a timing navigation tree is ever
  needed, it goes in Z3 under Timing (P1), not in-canvas.
- **N3.** SimLog's Simulate-only visibility is the approved pattern for
  context tabs in Z6; do not generalize it into per-activity tab sets
  without a grammar amendment.
- **N4.** The bottom panel's tab order is fixed: Console, Messages, Sim log
  (Simulate only), Jobs. New output streams append after Messages; new job
  types go inside Jobs.

---

## 8. Deliberate differences from the references

- **D1. Activity rail instead of a Flow Navigator tree.** VS Code's Activity
  Bar pattern replaces Vivado's left Flow Navigator and Quartus's Tasks pane
  as the primary switcher. Justification: the IDE keeps exactly one sidebar
  (no 9-section vendor tree); the rail makes that sidebar's content
  switchable in one click. Flow launching moves to Z1 (D2).
- **D2. Flow actions in the toolbar, not a left navigator.** Vivado's Flow
  Navigator is a left tree; Helion right-pins Synth/Opt/Place/Route/Implement
  in Z1. Justification: WP416's push-button flow guidance ("an intuitive
  toolbar guides the designers through all the setup and implementation
  steps"); keeps Z3 free for data navigation.
- **D3. Simulate mode hides the sidebar; scopes live in-canvas.** VS Code
  puts Run-and-Debug scopes/variables in the Side Bar. Helion hides Z3 under
  Simulate and lays out scopes inside Z5. Justification: the wave canvas and
  scope tables both need maximal width; a 220px sidebar would crush them.
- **D4. No menu bar.** egui ships no native menu bar; window-wide commands
  live in Z1 as menus (Recent, Examples) and the Tcl console is the command
  palette equivalent. Justification: single cross-platform binary; every GUI
  action already has a Tcl equivalent (Vivado WP416 principle).
- **D5. Reports is its own rail activity, not a bottom tab.** Vivado's
  Results Windows Area includes a Reports window (bottom); Helion gives
  reports a rail activity with a Z3 catalog and Z5 details. Justification:
  report details are full documents (timing paths, power breakdowns) that
  need canvas space; the bottom panel keeps only streams and jobs.

---

## 9. Code anchors

- Zone routing: `crates/helion-gui/src/chrome.rs` (`pane_for_workspace`,
  `is_more_destination`, `is_report_detail`, `Canvas`, `Activity`,
  `WorkspacePane`, `share_available`, `chrome_at`).
- Chrome state machine: `crates/helion-gui/src/surface.rs` (`central_pane`,
  `apply_activity`, `apply_canvas`, `apply_more`).
- Paint sites: `crates/helion-gui/src/bin/helion-ide.rs`
  (`paint_toolbar`, `paint_activity_rail`, `paint_sidebar`,
  `paint_properties`, `paint_bottom`, `paint_tcl_console`,
  `paint_status_bar`, `paint_workspace`, `paint_sim_workspace`,
  `paint_more_pane`, `paint_runs`, `paint_checkpoints`, `paint_hierarchy`).
- Toasts: `crates/helion-gui/src/bin/helion-ide/toasts.rs`.
- Welcome: `crates/helion-gui/src/bin/helion-ide/welcome.rs`.
- Dashboard: `crates/helion-gui/src/bin/helion-ide/dashboard.rs`.

## 10. Amendment process

This grammar is a contract. Adding a zone, moving a surface across zones, or
adding a deliberate difference requires amending this document with the rule
and the reference-app justification. "It fit there" is never a justification.
