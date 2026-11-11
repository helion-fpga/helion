//! Window chrome: menu bar, icon toolbar, Flow Navigator rail, menu overlays.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-1b93965a*.png` main window, `rd-61f871f8*.png` rail
//! close-up): 26px menu bar with Quick Access, dense icon toolbar, and the
//! caps-section Flow Navigator rail. All colors come from [`VivadoTheme`];
//! all dispatch is typed state plus Tcl-verb console echoes (no stringly
//! status, no manual geometry, no button-in-menu rows).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()/canvas()`.
//! - `crates/rune/src/el/anchor.rs`: `.anchored(key, Side, Align, gap)`
//!   out-of-flow overlay attached at the root.
//! - `crates/rune/src/shape.rs`: `Shape::fill/stroke/view_box` icons.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_RIGHT/DOWN`, `SEARCH`.

use rune::{Align, El, Path, ScrollSpec, Shape, Side, Tx, canvas, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev, RunStatus};
use crate::theme::VivadoTheme;
use crate::views::console::ConsoleTab;
use crate::views::dialogs::DialogId;
use crate::views::reports::ReportNav;

/// Top-level menus, in Vivado order. Typed so open-menu state is never a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuId {
    File,
    Edit,
    Flow,
    Tools,
    Window,
    Layout,
    View,
    Help,
}

impl MenuId {
    pub const ALL: [MenuId; 8] = [
        MenuId::File,
        MenuId::Edit,
        MenuId::Flow,
        MenuId::Tools,
        MenuId::Window,
        MenuId::Layout,
        MenuId::View,
        MenuId::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuId::File => "File",
            MenuId::Edit => "Edit",
            MenuId::Flow => "Flow",
            MenuId::Tools => "Tools",
            MenuId::Window => "Window",
            MenuId::Layout => "Layout",
            MenuId::View => "View",
            MenuId::Help => "Help",
        }
    }

    pub fn key(self) -> String {
        format!("menu-{}", self.label().to_lowercase())
    }
}

/// Real menu action behind a dropdown row. Every row does work; the Tcl
/// text is only the console journal of what ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAct {
    OpenProject,
    OpenExample,
    OpenRecent(usize),
    ClearRecent,
    CloseProject,
    Exit,
    OpenDialog(DialogId),
    Nav(CenterPane),
    Console(ConsoleTab),
    Undo,
    Redo,
    Save,
    Find,
    RunSynth,
    RunImpl,
    RunBitstream,
    RunSim,
    CancelRun,
    Palette(crate::views::palette::PaletteMode),
    Cheatsheet,
    SetTheme(crate::theme::ThemeMode),
    Layout(crate::layout::LayoutPreset),
    ToggleFlow,
    ToggleSources,
    ToggleConsole,
    ToggleFocus,
    Docs,
    About,
}

/// One dropdown row: a dispatchable item, a caps section caption, or a rule.
enum MenuRow {
    Item {
        label: String,
        tcl: &'static str,
        shortcut: Option<&'static str>,
        act: MenuAct,
        checked: bool,
        /// `Some(reason)` renders the row disabled with that tooltip.
        blocked: Option<&'static str>,
    },
    Caption(&'static str),
    Sep,
}

/// Why a flow run cannot start right now (`None` = it can).
pub(crate) fn run_blocked(s: &AppState) -> Option<&'static str> {
    if s.project.name.is_empty() {
        Some("Open or create a project first")
    } else if s.flow.synth == RunStatus::Running || s.flow.impl_run == RunStatus::Running {
        Some("A run is already in progress")
    } else {
        None
    }
}

/// The bundled example project (`examples/counter.prj`), when present.
pub(crate) fn example_project() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.prj");
    p.is_file().then(|| p.canonicalize().unwrap_or(p))
}

/// Dropdown contents per menu.
fn menu_rows(id: MenuId, s: &AppState) -> Vec<MenuRow> {
    use crate::layout::LayoutPreset;
    use crate::theme::ThemeMode;
    use MenuAct as A;
    use MenuRow::{Caption, Item, Sep};
    let item = |label: &str, tcl: &'static str, act: A| Item {
        label: label.to_string(),
        tcl,
        shortcut: None,
        act,
        checked: false,
        blocked: None,
    };
    let hot = |label: &str, tcl: &'static str, shortcut: &'static str, act: A| Item {
        label: label.to_string(),
        tcl,
        shortcut: Some(shortcut),
        act,
        checked: false,
        blocked: None,
    };
    let check = |label: &str, tcl: &'static str, act: A, checked: bool| Item {
        label: label.to_string(),
        tcl,
        shortcut: None,
        act,
        checked,
        blocked: None,
    };
    let gate = |row: MenuRow, reason: Option<&'static str>| match row {
        Item {
            label,
            tcl,
            shortcut,
            act,
            checked,
            ..
        } => Item {
            label,
            tcl,
            shortcut,
            act,
            checked,
            blocked: reason,
        },
        other => other,
    };
    let no_project = s.project.name.is_empty().then_some("No project is open");
    let run_gate = run_blocked(s);
    let running = s.flow.synth == RunStatus::Running || s.flow.impl_run == RunStatus::Running;
    match id {
        MenuId::File => {
            let mut rows = vec![
                hot(
                    "New Project…",
                    "create_project",
                    "⌘N",
                    A::OpenDialog(DialogId::NewProject),
                ),
                hot("Open Project…", "open_project", "⌘O", A::OpenProject),
                gate(
                    item("Open Example Project", "open_example counter", A::OpenExample),
                    example_project()
                        .is_none()
                        .then_some("The bundled examples are not installed"),
                ),
                Sep,
                Caption("RECENT PROJECTS"),
            ];
            if s.recent.is_empty() {
                rows.push(gate(
                    item("No recent projects", "recent_projects", A::ClearRecent),
                    Some("Projects you open appear here"),
                ));
            } else {
                for (i, path) in s.recent.iter().take(6).enumerate() {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    rows.push(item(&name, "open_recent", A::OpenRecent(i)));
                }
                rows.push(item("Clear Recent Projects", "clear_recent", A::ClearRecent));
            }
            rows.extend([
                Sep,
                hot("Save", "save_file", "⌘S", A::Save),
                gate(
                    item("Close Project", "close_project", A::CloseProject),
                    no_project,
                ),
                Sep,
                hot("Exit", "exit", "⌘Q", A::Exit),
            ]);
            rows
        }
        MenuId::Edit => vec![
            hot("Undo", "undo", "⌘Z", A::Undo),
            hot("Redo", "redo", "⇧⌘Z", A::Redo),
            Sep,
            hot("Find in File…", "find", "⌘F", A::Find),
            hot(
                "Go to File…",
                "palette_files",
                "⌘P",
                A::Palette(crate::views::palette::PaletteMode::Files),
            ),
            hot(
                "Command Palette…",
                "palette_commands",
                "⇧⌘P",
                A::Palette(crate::views::palette::PaletteMode::Commands),
            ),
        ],
        MenuId::Flow => {
            let mut rows = vec![
                gate(
                    item("Run Synthesis", "launch_runs synth_1", A::RunSynth),
                    run_gate,
                ),
                gate(
                    item("Run Implementation", "launch_runs impl_1", A::RunImpl),
                    run_gate,
                ),
                gate(
                    item(
                        "Generate Bitstream",
                        "launch_runs -to_step write_bitstream",
                        A::RunBitstream,
                    ),
                    run_gate,
                ),
            ];
            if running {
                rows.push(item("Cancel Run", "cancel_run", A::CancelRun));
            }
            rows.extend([
                Sep,
                gate(
                    item("Run Simulation", "launch_simulation", A::RunSim),
                    no_project,
                ),
                Sep,
                item(
                    "Open Elaborated Design",
                    "open_run -elaborated",
                    A::Nav(CenterPane::Schematic),
                ),
                item(
                    "Open Synthesized Design",
                    "open_run synth_1",
                    A::Nav(CenterPane::Schematic),
                ),
                item(
                    "Open Implemented Design",
                    "open_run impl_1",
                    A::Nav(CenterPane::Device),
                ),
                Sep,
                item(
                    "Report Timing Summary",
                    "report_timing_summary",
                    A::Nav(CenterPane::Timing),
                ),
                item("Open Reports", "show_view Reports", A::Nav(CenterPane::Reports)),
            ]);
            rows
        }
        MenuId::Tools => vec![
            hot(
                "Settings…",
                "show_settings",
                "⌘,",
                A::OpenDialog(DialogId::Settings),
            ),
            gate(
                item(
                    "Constraints Wizard…",
                    "constraints_wizard",
                    A::OpenDialog(DialogId::Constraints),
                ),
                no_project,
            ),
            gate(
                item(
                    "Add Sources…",
                    "add_files",
                    A::OpenDialog(DialogId::AddSources),
                ),
                no_project,
            ),
            Sep,
            item("Keyboard Shortcuts", "shortcuts", A::Cheatsheet),
        ],
        MenuId::Window => vec![
            check(
                "Flow Navigator",
                "toggle_flow_navigator",
                A::ToggleFlow,
                s.layout.show_flow,
            ),
            check(
                "Sources",
                "toggle_sources",
                A::ToggleSources,
                s.layout.show_sources,
            ),
            check(
                "Console",
                "toggle_console",
                A::ToggleConsole,
                s.layout.show_console,
            ),
            Item {
                label: if s.layout.focus {
                    "Restore Layout".into()
                } else {
                    "Maximize Center Pane".into()
                },
                tcl: "maximize_view",
                shortcut: Some("⇧⌘M"),
                act: A::ToggleFocus,
                checked: false,
                blocked: None,
            },
            Sep,
            Caption("CONSOLE"),
            item("Tcl Console", "show_view TclConsole", A::Console(ConsoleTab::Tcl)),
            item(
                "Messages",
                "show_view Messages",
                A::Console(ConsoleTab::Messages),
            ),
            item("Log", "show_view Log", A::Console(ConsoleTab::Log)),
            item(
                "Reports",
                "show_view Reports",
                A::Console(ConsoleTab::Reports),
            ),
            item(
                "Design Runs",
                "show_view DesignRuns",
                A::Console(ConsoleTab::DesignRuns),
            ),
        ],
        MenuId::Layout => {
            let mut rows: Vec<MenuRow> = LayoutPreset::ALL
                .into_iter()
                .map(|p| {
                    check(
                        p.label(),
                        "layout",
                        A::Layout(p),
                        s.layout.preset == p && !s.layout.customized,
                    )
                })
                .collect();
            rows.push(Sep);
            rows.push(item(
                "Reset Current Layout",
                "layout reset",
                A::Layout(s.layout.preset),
            ));
            rows
        }
        MenuId::View => vec![
            item("Start Page", "show_view Start", A::Nav(CenterPane::Start)),
            item(
                "Project Summary",
                "show_view Summary",
                A::Nav(CenterPane::Summary),
            ),
            Sep,
            hot("Editor", "show_view Editor", "⌘1", A::Nav(CenterPane::Editor)),
            item(
                "Schematic",
                "show_view Schematic",
                A::Nav(CenterPane::Schematic),
            ),
            hot("Device", "show_view Device", "⌘2", A::Nav(CenterPane::Device)),
            hot("Timing", "show_view Timing", "⌘3", A::Nav(CenterPane::Timing)),
            item("Reports", "show_view Reports", A::Nav(CenterPane::Reports)),
            Sep,
            item("Simulation", "show_view Sim", A::Nav(CenterPane::Sim)),
            item("Waveform", "show_view Wave", A::Nav(CenterPane::Wave)),
            item("Debug", "show_view Debug", A::Nav(CenterPane::Debug)),
            item(
                "Hardware Manager",
                "show_view Program",
                A::Nav(CenterPane::Program),
            ),
            Sep,
            Caption("THEME"),
            check(
                "Light",
                "set_theme light",
                A::SetTheme(ThemeMode::Light),
                s.theme == ThemeMode::Light,
            ),
            check(
                "Dark",
                "set_theme dark",
                A::SetTheme(ThemeMode::Dark),
                s.theme == ThemeMode::Dark,
            ),
            check(
                "Follow System",
                "set_theme system",
                A::SetTheme(ThemeMode::System),
                s.theme == ThemeMode::System,
            ),
        ],
        MenuId::Help => vec![
            item("Keyboard Shortcuts", "shortcuts", A::Cheatsheet),
            item("Documentation", "help docs", A::Docs),
            Sep,
            item("About Helion", "help about", A::About),
        ],
    }
}

/// Flow Navigator sections, in Vivado order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowSection {
    ProjectManager,
    IpIntegrator,
    Simulation,
    RtlAnalysis,
    Synthesis,
    Implementation,
    ProgramDebug,
}

impl FlowSection {
    pub const ALL: [FlowSection; 7] = [
        FlowSection::ProjectManager,
        FlowSection::IpIntegrator,
        FlowSection::Simulation,
        FlowSection::RtlAnalysis,
        FlowSection::Synthesis,
        FlowSection::Implementation,
        FlowSection::ProgramDebug,
    ];

    pub fn index(self) -> usize {
        match self {
            FlowSection::ProjectManager => 0,
            FlowSection::IpIntegrator => 1,
            FlowSection::Simulation => 2,
            FlowSection::RtlAnalysis => 3,
            FlowSection::Synthesis => 4,
            FlowSection::Implementation => 5,
            FlowSection::ProgramDebug => 6,
        }
    }

    pub fn caps(self) -> &'static str {
        match self {
            FlowSection::ProjectManager => "PROJECT MANAGER",
            FlowSection::IpIntegrator => "IP INTEGRATOR",
            FlowSection::Simulation => "SIMULATION",
            FlowSection::RtlAnalysis => "RTL ANALYSIS",
            FlowSection::Synthesis => "SYNTHESIS",
            FlowSection::Implementation => "IMPLEMENTATION",
            FlowSection::ProgramDebug => "PROGRAM AND DEBUG",
        }
    }

    pub fn key(self) -> String {
        format!("flow-{}", self.caps().to_lowercase().replace(' ', "-"))
    }
}

/// A rail entry: label, Tcl verb, green run triangle, run-state flip,
/// and the live navigation (center pane / dialog / report tab) it opens.
struct FlowEntry {
    key: String,
    label: String,
    run_icon: bool,
    tcl: &'static str,
    starts: Option<RunTarget>,
    nav: Option<CenterPane>,
    dialog: Option<DialogId>,
    report: Option<ReportNav>,
}

#[derive(Clone, Copy)]
enum RunTarget {
    Synth,
    Impl,
    Bitstream,
    Sim,
}

/// Rail entries for a section; run entries reflect live [`RunStatus`] so the
/// rail reads "Open Synthesized Design" once synthesis has started.
/// Open design entries navigate to Schematic, simulation to Sim, program
/// entries to Program, report entries to Timing/Reports, manager entries
/// to dialogs.
fn flow_entries(section: FlowSection, s: &AppState) -> Vec<FlowEntry> {
    let link = |section: FlowSection,
                label: &'static str,
                tcl: &'static str,
                nav: Option<CenterPane>,
                dialog: Option<DialogId>,
                report: Option<ReportNav>| FlowEntry {
        key: format!(
            "{}-{}",
            section.key(),
            label.to_lowercase().replace(' ', "-")
        ),
        label: label.into(),
        run_icon: false,
        tcl,
        starts: None,
        nav,
        dialog,
        report,
    };
    let plain = |section: FlowSection, label: &'static str, tcl: &'static str| {
        link(section, label, tcl, None, None, None)
    };
    let run =
        |section: FlowSection, label: String, tcl: &'static str, target: RunTarget, armed: bool| {
            FlowEntry {
                key: format!("{}-{}", section.key(), if armed { "run" } else { "open" }),
                label,
                run_icon: armed,
                tcl,
                starts: armed.then_some(target),
                // The relabeled Open entry navigates; the armed Run entry stays.
                nav: (!armed).then_some(CenterPane::Schematic),
                dialog: None,
                report: None,
            }
        };
    match section {
        FlowSection::ProjectManager => vec![
            link(
                section,
                "Settings",
                "show_settings",
                None,
                Some(DialogId::Settings),
                None,
            ),
            link(
                section,
                "Add Sources",
                "add_sources",
                None,
                Some(DialogId::AddSources),
                None,
            ),
            plain(section, "Language Templates", "show_templates"),
            plain(section, "IP Catalog", "show_ip_catalog"),
        ],
        FlowSection::IpIntegrator => vec![
            plain(section, "Create Block Design", "create_bd_design"),
            plain(section, "Open Block Design", "open_bd_design"),
            plain(section, "Generate Block Design", "generate_target"),
        ],
        FlowSection::Simulation => {
            let armed = s.flow.sim == RunStatus::NotStarted;
            let mut entries = vec![
                link(
                    section,
                    "Simulation Settings",
                    "show_sim_settings",
                    None,
                    Some(DialogId::Settings),
                    None,
                ),
                // Unlike synth/impl the sim entry always navigates: the run's
                // progress and logs live on the Sim surface it opens.
                FlowEntry {
                    key: format!("{}-{}", section.key(), if armed { "run" } else { "open" }),
                    label: if armed {
                        "Run Simulation".into()
                    } else {
                        "Open Simulation".into()
                    },
                    run_icon: armed,
                    tcl: "launch_simulation",
                    starts: armed.then_some(RunTarget::Sim),
                    nav: Some(CenterPane::Sim),
                    dialog: None,
                    report: None,
                },
            ];
            entries.push(link(
                section,
                "Open Debug",
                "breakpoints",
                Some(CenterPane::Debug),
                None,
                None,
            ));
            entries
        }
        FlowSection::RtlAnalysis => vec![link(
            section,
            "Open Elaborated Design",
            "open_run -elaborated",
            Some(CenterPane::Schematic),
            None,
            None,
        )],
        FlowSection::Synthesis => {
            let armed = s.flow.synth == RunStatus::NotStarted;
            let mut entries = vec![run(
                section,
                if armed {
                    "Run Synthesis".into()
                } else {
                    "Open Synthesized Design".into()
                },
                "launch_runs synth_1",
                RunTarget::Synth,
                armed,
            )];
            // Once started the run entry itself becomes the Open entry; a
            // second static Open row would duplicate its label.
            if armed {
                entries.push(link(
                    section,
                    "Open Synthesized Design",
                    "open_run synth_1",
                    Some(CenterPane::Schematic),
                    None,
                    None,
                ));
            }
            entries.push(link(
                section,
                "Report Timing Summary",
                "report_timing_summary",
                Some(CenterPane::Timing),
                None,
                None,
            ));
            entries
        }
        FlowSection::Implementation => {
            let armed = s.flow.impl_run == RunStatus::NotStarted;
            let mut entries = vec![run(
                section,
                if armed {
                    "Run Implementation".into()
                } else {
                    "Open Implemented Design".into()
                },
                "launch_runs impl_1",
                RunTarget::Impl,
                armed,
            )];
            if armed {
                entries.push(link(
                    section,
                    "Open Implemented Design",
                    "open_run impl_1",
                    Some(CenterPane::Schematic),
                    None,
                    None,
                ));
            }
            entries.push(link(
                section,
                "Report Utilization",
                "report_utilization",
                Some(CenterPane::Reports),
                None,
                Some(ReportNav::Utilization),
            ));
            entries
        }
        FlowSection::ProgramDebug => vec![
            FlowEntry {
                key: format!("{}-{}", section.key(), "generate-bitstream".to_string()),
                label: "Generate Bitstream".into(),
                run_icon: true,
                tcl: "launch_runs -to_step write_bitstream",
                starts: Some(RunTarget::Bitstream),
                nav: Some(CenterPane::Program),
                dialog: None,
                report: None,
            },
            link(
                section,
                "Open Hardware Manager",
                "open_hw_manager",
                Some(CenterPane::Program),
                None,
                None,
            ),
        ],
    }
}

/// Toolbar tools: tooltip, Tcl verb, geometric icon, run-state flip,
/// and center-pane navigation for the program entries.
struct Tool {
    id: &'static str,
    tip: &'static str,
    tcl: &'static str,
    icon: ToolIcon,
    starts: Option<RunTarget>,
    nav: Option<CenterPane>,
}

#[derive(Clone, Copy)]
enum ToolIcon {
    Back,
    Forward,
    New,
    Open,
    Save,
    Undo,
    Redo,
    Play,
    PlayPlay,
    Chip,
    Hw,
}

const TOOLS_NAV: &[Tool] = &[
    Tool {
        id: "nav-back",
        tip: "Back",
        tcl: "",
        icon: ToolIcon::Back,
        starts: None,
        nav: None,
    },
    Tool {
        id: "nav-forward",
        tip: "Forward",
        tcl: "",
        icon: ToolIcon::Forward,
        starts: None,
        nav: None,
    },
];

const TOOLS: &[Tool] = &[
    Tool {
        id: "new-project",
        tip: "New Project",
        tcl: "create_project",
        icon: ToolIcon::New,
        starts: None,
        nav: None,
    },
    Tool {
        id: "open-project",
        tip: "Open Project",
        tcl: "open_project",
        icon: ToolIcon::Open,
        starts: None,
        nav: None,
    },
    Tool {
        id: "save",
        tip: "Save",
        tcl: "save_project",
        icon: ToolIcon::Save,
        starts: None,
        nav: None,
    },
];

const TOOLS_EDIT: &[Tool] = &[
    Tool {
        id: "undo",
        tip: "Undo",
        tcl: "undo",
        icon: ToolIcon::Undo,
        starts: None,
        nav: None,
    },
    Tool {
        id: "redo",
        tip: "Redo",
        tcl: "redo",
        icon: ToolIcon::Redo,
        starts: None,
        nav: None,
    },
];

const TOOLS_RUN: &[Tool] = &[
    Tool {
        id: "run-synth",
        tip: "Run Synthesis",
        tcl: "launch_runs synth_1",
        icon: ToolIcon::Play,
        starts: Some(RunTarget::Synth),
        nav: None,
    },
    Tool {
        id: "run-impl",
        tip: "Run Implementation",
        tcl: "launch_runs impl_1",
        icon: ToolIcon::PlayPlay,
        starts: Some(RunTarget::Impl),
        nav: None,
    },
    Tool {
        id: "gen-bitstream",
        tip: "Generate Bitstream",
        tcl: "launch_runs -to_step write_bitstream",
        icon: ToolIcon::Chip,
        starts: Some(RunTarget::Bitstream),
        nav: Some(CenterPane::Program),
    },
    Tool {
        id: "hw-manager",
        tip: "Open Hardware Manager",
        tcl: "open_hw_manager",
        icon: ToolIcon::Hw,
        starts: None,
        nav: Some(CenterPane::Program),
    },
];

/// Chrome-owned UI state: which dropdown is open, which rail sections fold.
#[derive(Clone, Debug, Default)]
pub struct ChromeState {
    pub open_menu: Option<MenuId>,
    pub collapsed: [bool; 7],
    /// Help ▸ About dialog is showing.
    pub about_open: bool,
}

impl ChromeState {
    pub fn toggle_menu(&mut self, id: MenuId) {
        self.open_menu = if self.open_menu == Some(id) {
            None
        } else {
            Some(id)
        };
    }

    pub fn toggle_section(&mut self, section: FlowSection) {
        let i = section.index();
        self.collapsed[i] = !self.collapsed[i];
    }
}

/// Dispatch a Tcl verb: close any menu, launch the run, echo to the console.
fn dispatch(s: &mut AppState, tcl: &'static str, starts: Option<RunTarget>) {
    dispatch_full(s, tcl, starts, None, None, None);
}

/// Full dispatch: real run launch + center navigation + dialog + report
/// tab + echo. A run entry launches the engine worker (never a bare status
/// flip: the worker owns Running/Complete/Failed); the launch echoes its own
/// verb, so the manual echo below only fires for non-run entries.
fn dispatch_full(
    s: &mut AppState,
    tcl: &'static str,
    starts: Option<RunTarget>,
    nav: Option<CenterPane>,
    dialog: Option<DialogId>,
    report: Option<ReportNav>,
) {
    s.chrome.open_menu = None;
    match starts {
        Some(RunTarget::Synth) => crate::runs::launch(s, crate::runs::RunKind::Synth),
        Some(RunTarget::Impl) => crate::runs::launch(s, crate::runs::RunKind::Impl),
        Some(RunTarget::Bitstream) => crate::runs::launch_verb(
            s,
            crate::runs::RunKind::Impl,
            "launch_runs -to_step write_bitstream",
        ),
        Some(RunTarget::Sim) => crate::views::wave::launch(s),
        None => {}
    }
    if let Some(pane) = nav {
        s.navigate(pane);
    }
    if let Some(tab) = report {
        s.reports_view.selected = tab;
    }
    if let Some(id) = dialog {
        s.dialogs.open(id);
    }
    if starts.is_none() {
        s.console.lines.push(ConsoleLine {
            text: tcl.into(),
            sev: MsgSev::Info,
        });
    }
}

/// Journal a menu action's Tcl spelling to the console.
fn journal(s: &mut AppState, tcl: &str) {
    s.console.lines.push(ConsoleLine {
        text: tcl.into(),
        sev: MsgSev::Info,
    });
}

/// Dispatch a menu row's real action (then journal its Tcl spelling).
fn dispatch_menu(s: &mut AppState, tcl: &'static str, act: MenuAct, tx: &mut Tx) {
    use crate::layout;
    s.chrome.open_menu = None;
    match act {
        MenuAct::OpenProject => crate::pickers::open_project(tx),
        MenuAct::OpenExample => {
            if let Some(path) = example_project() {
                crate::pickers::open_path(s, &path);
            }
        }
        MenuAct::OpenRecent(i) => {
            if let Some(path) = s.recent.get(i).cloned() {
                crate::pickers::open_path(s, std::path::Path::new(&path));
            }
        }
        MenuAct::ClearRecent => {
            s.recent.clear();
            layout::save(s);
        }
        MenuAct::CloseProject => crate::pickers::close_project(s),
        MenuAct::Exit => tx.quit(),
        MenuAct::OpenDialog(id) => dispatch_full(s, tcl, None, None, Some(id), None),
        MenuAct::Nav(pane) => dispatch_full(s, tcl, None, Some(pane), None, None),
        MenuAct::Console(tab) => {
            layout::set_console_visible(s, true);
            s.console.tab = tab;
        }
        MenuAct::Undo => {
            if crate::views::editor::undo_synced(s) {
                journal(s, tcl);
            }
        }
        MenuAct::Redo => {
            if crate::views::editor::redo_synced(s) {
                journal(s, tcl);
            }
        }
        MenuAct::Save => save_active_file(s),
        MenuAct::Find => {
            s.navigate(CenterPane::Editor);
            s.editor.find_open = true;
        }
        MenuAct::RunSynth => dispatch(s, tcl, Some(RunTarget::Synth)),
        MenuAct::RunImpl => dispatch(s, tcl, Some(RunTarget::Impl)),
        MenuAct::RunBitstream => dispatch_full(
            s,
            tcl,
            Some(RunTarget::Bitstream),
            Some(CenterPane::Program),
            None,
            None,
        ),
        MenuAct::RunSim => dispatch_full(
            s,
            tcl,
            Some(RunTarget::Sim),
            Some(CenterPane::Wave),
            None,
            None,
        ),
        MenuAct::CancelRun => {
            crate::runs::cancel_run(s);
            journal(s, tcl);
        }
        MenuAct::Palette(mode) => crate::views::palette::open_palette(s, mode),
        MenuAct::Cheatsheet => crate::views::palette::open_cheatsheet(s),
        MenuAct::SetTheme(mode) => {
            s.theme = mode;
            layout::save(s);
            journal(s, tcl);
        }
        MenuAct::Layout(preset) => {
            layout::apply_preset(s, preset);
            journal(s, &format!("layout {}", preset.id()));
        }
        MenuAct::ToggleFlow => layout::set_flow_visible(s, !s.layout.show_flow),
        MenuAct::ToggleSources => layout::set_sources_visible(s, !s.layout.show_sources),
        MenuAct::ToggleConsole => layout::set_console_visible(s, !s.layout.show_console),
        MenuAct::ToggleFocus => layout::toggle_focus(s),
        MenuAct::Docs => {
            if tx.open_url(DOCS_URL).is_err() {
                s.toasts.push(
                    crate::state::ToastKind::Error,
                    "Couldn't open the documentation",
                    DOCS_URL,
                );
            }
        }
        MenuAct::About => s.chrome.about_open = true,
    }
}

/// Project documentation (README + docs/ on the public repository).
pub const DOCS_URL: &str = "https://github.com/helion-fpga/helion#readme";

/// Save the active editor tab to disk, journaling the result. Shared by
/// File ▸ Save, the toolbar Save button and ⌘S.
pub(crate) fn save_active_file(s: &mut AppState) {
    match crate::views::editor::save_active(s) {
        Ok(path) => journal(s, &format!("save_file {}", path.display())),
        Err(e) => {
            s.console.lines.push(ConsoleLine {
                text: format!("ERROR: save_file: {e}"),
                sev: MsgSev::Error,
            });
            s.toasts
                .push(crate::state::ToastKind::Error, "Save failed", &e.to_string());
        }
    }
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

/// Geometric 16px toolbar/run glyphs drawn from scratch on a Rune canvas.
fn tool_glyph(icon_kind: ToolIcon, t: &VivadoTheme) -> El {
    let ink = t.text;
    let shape = match icon_kind {
        ToolIcon::Back => Shape::new()
            .stroke(vec![[10.5, 3.0], [5.0, 8.0], [10.5, 13.0]], 1.6, ink)
            .view_box(16.0, 16.0),
        ToolIcon::Forward => Shape::new()
            .stroke(vec![[5.5, 3.0], [11.0, 8.0], [5.5, 13.0]], 1.6, ink)
            .view_box(16.0, 16.0),
        ToolIcon::New => Shape::new()
            .stroke(vec![[3.0, 8.0], [13.0, 8.0]], 1.6, ink)
            .stroke(vec![[8.0, 3.0], [8.0, 13.0]], 1.6, ink)
            .view_box(16.0, 16.0),
        ToolIcon::Open => Shape::new()
            .fill(
                vec![
                    [2.0, 5.0],
                    [2.0, 13.0],
                    [14.0, 13.0],
                    [14.0, 5.0],
                    [8.0, 5.0],
                    [7.0, 3.5],
                    [2.0, 3.5],
                ],
                ink,
            )
            .view_box(16.0, 16.0),
        ToolIcon::Save => Shape::new()
            .stroke(
                vec![
                    [3.0, 3.0],
                    [11.0, 3.0],
                    [13.0, 5.0],
                    [13.0, 13.0],
                    [3.0, 13.0],
                    [3.0, 3.0],
                ],
                1.4,
                ink,
            )
            .fill(vec![[5.5, 3.0], [10.5, 3.0], [10.5, 7.0], [5.5, 7.0]], ink)
            .view_box(16.0, 16.0),
        ToolIcon::Undo | ToolIcon::Redo => {
            let head: [[f32; 2]; 3] = match icon_kind {
                ToolIcon::Undo => [[4.0, 8.0], [7.5, 5.0], [7.5, 11.0]],
                _ => [[12.0, 8.0], [8.5, 5.0], [8.5, 11.0]],
            };
            Shape::new()
                .stroke(vec![[4.5, 8.0], [11.5, 8.0]], 1.6, ink)
                .fill(head.to_vec(), ink)
                .view_box(16.0, 16.0)
        }
        ToolIcon::Play => Shape::new()
            .fill(vec![[4.5, 3.0], [12.5, 8.0], [4.5, 13.0]], t.pass)
            .view_box(16.0, 16.0),
        ToolIcon::PlayPlay => Shape::new()
            .fill(vec![[2.5, 3.0], [8.0, 8.0], [2.5, 13.0]], t.pass)
            .fill(vec![[8.0, 3.0], [13.5, 8.0], [8.0, 13.0]], t.pass)
            .view_box(16.0, 16.0),
        ToolIcon::Chip => Shape::new()
            .stroke(
                vec![
                    [4.0, 4.0],
                    [12.0, 4.0],
                    [12.0, 12.0],
                    [4.0, 12.0],
                    [4.0, 4.0],
                ],
                1.6,
                ink,
            )
            .stroke(vec![[6.5, 4.0], [6.5, 2.0]], 1.2, ink)
            .stroke(vec![[9.5, 4.0], [9.5, 2.0]], 1.2, ink)
            .stroke(vec![[6.5, 14.0], [6.5, 12.0]], 1.2, ink)
            .stroke(vec![[9.5, 14.0], [9.5, 12.0]], 1.2, ink)
            .view_box(16.0, 16.0),
        ToolIcon::Hw => {
            let ring = Path::new()
                .move_to([13.5, 8.0])
                .cubic_to([13.5, 11.0], [11.0, 13.5], [8.0, 13.5])
                .cubic_to([5.0, 13.5], [2.5, 11.0], [2.5, 8.0])
                .cubic_to([2.5, 5.0], [5.0, 2.5], [8.0, 2.5])
                .cubic_to([11.0, 2.5], [13.5, 5.0], [13.5, 8.0])
                .close();
            Shape::new()
                .stroke_path(&ring, 1.4, rune::LineCap::Round, ink)
                .fill(vec![[7.4, 5.5], [8.6, 5.5], [8.6, 10.5], [7.4, 10.5]], ink)
                .view_box(16.0, 16.0)
        }
    };
    canvas(shape).size(16.0, 16.0).a11y_hidden()
}

/// Menu bar: File..Help triggers plus the Quick Access search box.
pub fn menu_bar(s: &AppState, t: &VivadoTheme) -> El {
    let mut bar = row()
        .key("menubar")
        .role("menubar")
        .w_full()
        .h(26.0)
        .items_center()
        .px(6.0)
        .fill(t.chrome_bg);
    for id in MenuId::ALL {
        let open = s.chrome.open_menu == Some(id);
        bar = bar.child(
            el().key(id.key())
                .role("menuitem")
                .label(format!("{} menu", id.label()))
                .expanded(open)
                .focusable(true)
                .h_full()
                .px(8.0)
                .center_y()
                .fill(if open { t.selection } else { t.chrome_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| s.chrome.toggle_menu(id))
                .child(ui_label(id.label(), t)),
        );
    }
    bar.child(el().grow(1.0)).child(
        row()
            .key("quick-access")
            .role("button")
            .label("Quick Access: search commands, views and files")
            .help("Search commands, views and files")
            .focusable(true)
            .w(220.0)
            .h(20.0)
            .items_center()
            .gap(6.0)
            .px(6.0)
            .radius(4.0)
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .hover(t.selection)
            .on_press(|s: &mut AppState| {
                s.chrome.open_menu = None;
                crate::views::palette::open_palette(
                    s,
                    crate::views::palette::PaletteMode::Commands,
                );
            })
            .child(
                icon(&glyphs::SEARCH, 12.0)
                    .color(t.text_faint)
                    .a11y_hidden(),
            )
            .child(
                text("Quick Access")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint)
                    .grow(1.0),
            )
            .child(
                text("⇧⌘P")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint),
            ),
    )
}

fn toolbar_group(s: &AppState, tools: &[Tool], t: &VivadoTheme, first: bool) -> Vec<El> {
    let mut out = Vec::new();
    if !first {
        out.push(el().w(1.0).h(20.0).mx(2.0).fill(t.border).a11y_hidden());
    }
    for tool in tools {
        let (tcl, starts, nav) = (tool.tcl, tool.starts, tool.nav);
        // History/edit tools disable with a reason when they have nothing to do.
        // Undo/redo follow the active editor buffer (Verilog or XDC).
        let is_xdc = crate::views::editor::active_is_xdc(s);
        let (enabled, reason): (bool, &'static str) = match tool.id {
            "nav-back" => (!s.nav_back.is_empty(), "No backward history"),
            "nav-forward" => (!s.nav_forward.is_empty(), "No forward history"),
            "save" => (s.editor.dirty, "No unsaved changes"),
            "undo" => (s.editor.can_undo_active(is_xdc), "Nothing to undo"),
            "redo" => (s.editor.can_redo_active(is_xdc), "Nothing to redo"),
            "run-synth" | "run-impl" | "gen-bitstream" => match run_blocked(s) {
                Some(why) => (false, why),
                None => (true, ""),
            },
            _ => (true, ""),
        };
        let mut btn = el()
            .key(format!("tool-{}", tool.id))
            .role("button")
            .label(tool.tip)
            .help(if enabled { tool.tip } else { reason })
            .focusable(enabled)
            .disabled(!enabled)
            .size(24.0, 24.0)
            .radius(4.0)
            .center()
            .hover(t.selection)
            .child(tool_glyph(tool.icon, t));
        if !enabled {
            btn = btn.description(reason).layer_opacity(0.4);
        } else {
            let id = tool.id;
            btn = btn.on_press_tx(move |s: &mut AppState, tx: &mut Tx| match id {
                "nav-back" => {
                    s.go_back();
                }
                "nav-forward" => {
                    s.go_forward();
                }
                // Same paths as File ▸ Save / Edit ▸ Undo: write to disk and
                // keep the live edit field in step with the buffer.
                "save" => save_active_file(s),
                "undo" => {
                    if crate::views::editor::undo_synced(s) {
                        journal(s, tcl);
                    }
                }
                "redo" => {
                    if crate::views::editor::redo_synced(s) {
                        journal(s, tcl);
                    }
                }
                "new-project" => {
                    dispatch_full(s, tcl, None, None, Some(DialogId::NewProject), None)
                }
                "open-project" => crate::pickers::open_project(tx),
                _ => dispatch_full(s, tcl, starts, nav, None, None),
            });
        }
        out.push(btn);
    }
    out
}

/// Icon toolbar under the menu bar: nav, project, edit, and run tool groups.
pub fn icon_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let mut children = toolbar_group(s, TOOLS_NAV, t, true);
    children.extend(toolbar_group(s, TOOLS, t, false));
    children.extend(toolbar_group(s, TOOLS_EDIT, t, false));
    children.extend(toolbar_group(s, TOOLS_RUN, t, false));
    row()
        .key("toolbar")
        .role("toolbar")
        .w_full()
        .h(30.0)
        .items_center()
        .gap(2.0)
        .px(8.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border)
        .children(children)
}

/// Flow Navigator rail: header plus collapsible caps sections.
pub fn flow_rail(s: &AppState, t: &VivadoTheme) -> El {
    let mut rail = col()
        .key("flow-rail")
        .w(224.0)
        .h_full()
        .fill(t.rail_bg)
        .border(1.0, t.border);
    rail = rail.child(
        row()
            .w_full()
            .h(26.0)
            .px(10.0)
            .items_center()
            .justify_between()
            .fill(t.header_bg)
            .child(
                text("Flow Navigator")
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text),
            )
            .child(
                el().key("flow-collapse-all")
                    .role("button")
                    .label("Collapse all sections")
                    .focusable(true)
                    .size(20.0, 20.0)
                    .center()
                    .hover(t.selection)
                    .on_press(|s: &mut AppState| {
                        let any_open = s.chrome.collapsed.contains(&false);
                        s.chrome.collapsed = [any_open; 7];
                    })
                    .child(
                        icon(&glyphs::CHEVRON_UP, 12.0)
                            .color(t.text_dim)
                            .a11y_hidden(),
                    ),
            ),
    );
    let mut list = col()
        .key("flow-rail-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for section in FlowSection::ALL {
        let collapsed = s.chrome.collapsed[section.index()];
        list = list.child(
            row()
                .key(section.key())
                .role("button")
                .label(format!("{} section", section.caps()))
                .expanded(!collapsed)
                .focusable(true)
                .w_full()
                .h(22.0)
                .items_center()
                .gap(4.0)
                .px(6.0)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| s.chrome.toggle_section(section))
                .child(
                    icon(
                        if collapsed {
                            &glyphs::TRIANGLE_RIGHT
                        } else {
                            &glyphs::TRIANGLE_DOWN
                        },
                        10.0,
                    )
                    .color(t.text_dim)
                    .a11y_hidden(),
                )
                .child(
                    text(section.caps())
                        .font(VivadoTheme::FONT_CAPS)
                        .color(t.text_dim),
                ),
        );
        if !collapsed {
            for entry in flow_entries(section, s) {
                let (tcl, starts, nav, dialog, report) = (
                    entry.tcl,
                    entry.starts,
                    entry.nav,
                    entry.dialog,
                    entry.report,
                );
                let mut row_el = row()
                    .key(entry.key)
                    .role("button")
                    .label(entry.label.clone())
                    .focusable(true)
                    .w_full()
                    .h(20.0)
                    .items_center()
                    .gap(5.0)
                    .pl(20.0)
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| {
                        dispatch_full(s, tcl, starts, nav, dialog, report)
                    });
                if entry.run_icon {
                    row_el = row_el.child(
                        canvas(
                            Shape::new()
                                .fill(vec![[1.0, 0.5], [9.0, 5.0], [1.0, 9.5]], t.pass)
                                .view_box(10.0, 10.0),
                        )
                        .size(10.0, 10.0)
                        .a11y_hidden(),
                    );
                }
                list = list.child(row_el.child(ui_label(&entry.label, t)));
            }
        }
    }
    rail.child(list)
}

/// Open dropdown, anchored under its menu trigger. Attach at the root.
pub fn menu_overlay(s: &AppState, t: &VivadoTheme) -> Option<El> {
    let id = s.chrome.open_menu?;
    let mut list = col()
        .key(format!("{}-rows", id.key()))
        .items_stretch()
        .py(4.0);
    for (i, item) in menu_rows(id, s).into_iter().enumerate() {
        list = list.child(match item {
            MenuRow::Sep => el()
                .key(format!("{}-sep{i}", id.key()))
                .h(1.0)
                .my(4.0)
                .fill(t.border_light)
                .a11y_hidden(),
            MenuRow::Caption(caption) => row()
                .key(format!("{}-cap{i}", id.key()))
                .w_full()
                .h(20.0)
                .px(12.0)
                .items_center()
                .child(
                    text(caption)
                        .font(VivadoTheme::FONT_CAPS)
                        .color(t.text_faint),
                ),
            MenuRow::Item {
                label,
                tcl,
                shortcut,
                act,
                checked,
                blocked,
            } => {
                // A fixed check column keeps labels aligned whether or not
                // the row is a toggle.
                let mark = text(if checked { "✓" } else { "" })
                    .font(VivadoTheme::FONT_UI)
                    .color(t.accent);
                let mut item_row = row()
                    .key(format!("{}-item{i}", id.key()))
                    .role("menuitem")
                    .label(label.clone())
                    .checked(checked)
                    .focusable(blocked.is_none())
                    .disabled(blocked.is_some())
                    .w_full()
                    .h(24.0)
                    .items_center()
                    .gap(24.0)
                    .pl(6.0)
                    .pr(12.0)
                    .justify_between()
                    .child(
                        row()
                            .items_center()
                            .min_w(0.0)
                            .child(el().w(16.0).center().child(mark))
                            .child(
                                text(label)
                                    .font(VivadoTheme::FONT_UI)
                                    .color(if blocked.is_some() { t.text_faint } else { t.text })
                                    .truncate(),
                            ),
                    );
                if let Some(reason) = blocked {
                    item_row = item_row.description(reason).help(reason);
                } else {
                    item_row = item_row.hover(t.selection).on_press_tx(
                        move |s: &mut AppState, tx: &mut Tx| dispatch_menu(s, tcl, act, tx),
                    );
                }
                if let Some(hot) = shortcut {
                    item_row =
                        item_row.child(text(hot).font(VivadoTheme::FONT_UI).color(t.text_dim));
                }
                item_row
            }
        });
    }
    Some(
        col()
            .key(format!("{}-menu", id.key()))
            .role("menu")
            .min_w(220.0)
            .items_stretch()
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .anchored(id.key(), Side::Bottom, Align::Start, 2.0)
            .modal(true, |s: &mut AppState| s.chrome.open_menu = None)
            .child(list),
    )
}

/// Help ▸ About: product, version, build target and license, with the one
/// useful action (documentation).
pub fn push_about(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    use rune_kit::{Button, ButtonVariant, Dialog};
    if !s.chrome.about_open {
        return;
    }
    let kit = t.kit();
    let line = |label: &str, value: String| {
        row()
            .w_full()
            .h(20.0)
            .items_center()
            .child(
                el().w(88.0).child(
                    text(label.to_string())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_dim),
                ),
            )
            .child(
                text(value)
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text)
                    .truncate(),
            )
    };
    let body = col()
        .key("about-body")
        .w_full()
        .gap(10.0)
        .padding(16.0)
        .child(
            text("Helion")
                .font(VivadoTheme::FONT_DISPLAY)
                .color(t.text),
        )
        .child(
            text("Open FPGA toolchain: synthesis, place and route, timing, simulation, debug and programming.")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .max_lines(3),
        )
        .child(
            col()
                .w_full()
                .gap(2.0)
                .child(line("Version", env!("CARGO_PKG_VERSION").to_string()))
                .child(line("Target", crate::doctor::target_triple().to_string()))
                .child(line("Device", s.project.part.clone()))
                .child(line("License", "AGPL-3.0-or-later".to_string())),
        )
        .child(
            row()
                .w_full()
                .gap(8.0)
                .pt(4.0)
                .child(el().grow(1.0))
                .child(
                    Button::new("Documentation")
                        .variant(ButtonVariant::Neutral)
                        .on_press_tx::<AppState>(|s: &mut AppState, tx: &mut Tx| {
                            if tx.open_url(DOCS_URL).is_err() {
                                s.toasts.push(
                                    crate::state::ToastKind::Error,
                                    "Couldn't open the documentation",
                                    DOCS_URL,
                                );
                            }
                        })
                        .build(&kit)
                        .key("about-docs"),
                )
                .child(
                    Button::new("Close")
                        .variant(ButtonVariant::Submit)
                        .on_press::<AppState>(|s: &mut AppState| s.chrome.about_open = false)
                        .build(&kit)
                        .key("about-close"),
                ),
        );
    Dialog::new("about-dialog", "About Helion")
        .open(true)
        .width(440.0)
        .fit()
        .on_close(|s: &mut AppState| s.chrome.about_open = false)
        .child(body)
        .build(&kit, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness() -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(init(), crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn chrome_tree_exposes_menus_toolbar_and_rail() {
        let mut rt = harness();
        rt.frame(0.0);
        for key in [
            "menubar",
            "menu-file",
            "menu-help",
            "quick-access",
            "toolbar",
            "tool-run-synth",
            "tool-hw-manager",
            "flow-rail",
            "flow-synthesis",
            "flow-program-and-debug",
        ] {
            assert!(rt.find(key).is_some(), "missing chrome key: {key}");
        }
        assert!(
            rt.find("menu-file-menu").is_none(),
            "no dropdown before click"
        );
    }

    #[test]
    fn menu_opens_and_its_item_dispatches_tcl_and_closes() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "menu-file");
        assert_eq!(rt.state.chrome.open_menu, Some(MenuId::File));
        assert!(
            rt.find("menu-file-menu").is_some(),
            "dropdown anchors under trigger"
        );
        tap(&mut rt, "menu-file-item0");
        assert_eq!(rt.state.chrome.open_menu, None);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "create_project"),
            "item echoes its Tcl verb"
        );
    }

    #[test]
    fn flow_section_collapses_and_reopens() {
        let mut rt = harness();
        rt.frame(0.0);
        let entry = "flow-synthesis-run";
        assert!(rt.find(entry).is_some());
        tap(&mut rt, "flow-synthesis");
        assert!(rt.state.chrome.collapsed[FlowSection::Synthesis.index()]);
        assert!(rt.find(entry).is_none(), "collapsed entries leave the tree");
        tap(&mut rt, "flow-synthesis");
        assert!(rt.find(entry).is_some(), "entries return on reopen");
    }

    #[test]
    fn toolbar_run_flips_synth_running_and_echoes() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "tool-run-synth");
        assert_eq!(rt.state.flow.synth, RunStatus::Running);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs synth_1")
        );
        assert!(
            rt.find("flow-synthesis-open").is_some(),
            "rail relabels to Open Synthesized Design once started"
        );
    }

    #[test]
    fn run_entries_arm_without_faking_completion() {
        use crate::state::StatusVerb;
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "tool-run-synth");
        assert_eq!(rt.state.flow.synth, RunStatus::Running);
        assert_eq!(
            rt.state.status,
            StatusVerb::SynthesisRunning,
            "a real run is active, so the status verb must say so"
        );
        assert!(
            !rt.state.timing.has_run,
            "arming must not invent timing numbers"
        );
        assert!(
            rt.state.toasts.items.is_empty(),
            "arming must not raise a completion toast"
        );
        tap(&mut rt, "console-tab-runs");
        assert!(
            rt.find("console-run-synth-status").is_some(),
            "runs table must show the armed run"
        );
    }

    #[test]
    fn rail_run_implementation_finishes_a_real_run_end_to_end() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "flow-implementation-run");
        assert_eq!(rt.state.flow.impl_run, RunStatus::Running);
        let start = std::time::Instant::now();
        while rt.state.runs.is_active() {
            crate::runs::poll_runs(&mut rt.state);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(180),
                "the impl run must finish, not hang at Running"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::runs::poll_runs(&mut rt.state);
        assert_eq!(
            rt.state.flow.impl_run,
            RunStatus::Complete,
            "tapping Run Implementation must finish implementation"
        );
        assert!(
            rt.state.timing.has_run,
            "the timing report must come from the finished run"
        );
        assert!(
            rt.state.bitstream.has_run,
            "the bitstream must come from the finished run"
        );
        assert!(
            rt.state.runs.live_placed.is_some(),
            "the placement must land for the device view"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .filter(|l| l.text == "launch_runs impl_1")
                .count()
                == 1,
            "the verb must echo exactly once"
        );
    }

    #[test]
    fn rail_run_simulation_finishes_a_real_sim_end_to_end() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "flow-simulation-run");
        assert_eq!(rt.state.flow.sim, RunStatus::Running);
        assert_eq!(rt.state.center, CenterPane::Sim);
        let start = std::time::Instant::now();
        while rt.state.sim.is_active() {
            crate::views::wave::poll_sim(&mut rt.state);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(180),
                "the sim run must finish, not hang at Running"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::views::wave::poll_sim(&mut rt.state);
        assert_eq!(
            rt.state.flow.sim,
            RunStatus::Complete,
            "tapping Run Simulation must finish simulation"
        );
        assert!(
            !rt.state.sim.log.is_empty(),
            "the engine must stream sim log lines"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .filter(|l| l.text == "launch_simulation")
                .count()
                == 1,
            "the verb must echo exactly once"
        );
        assert!(
            rt.find("flow-simulation-open").is_some(),
            "rail relabels to Open Simulation once started"
        );
    }

    #[test]
    fn toolbar_bitstream_launches_impl_and_opens_program() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "tool-gen-bitstream");
        assert_eq!(rt.state.flow.impl_run, RunStatus::Running);
        assert_eq!(rt.state.center, CenterPane::Program);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_runs -to_step write_bitstream"),
            "bitstream must echo its own verb, not the normalized one"
        );
        crate::runs::cancel_run(&mut rt.state);
        let start = std::time::Instant::now();
        while rt.state.runs.is_active() {
            crate::runs::poll_runs(&mut rt.state);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "cancelled run must wind down"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn view_menu_theme_rows_pin_and_release_the_theme() {
        use crate::theme::ThemeMode;
        let mut rt = harness();
        rt.frame(0.0);
        assert_eq!(rt.state.theme, ThemeMode::System);
        tap(&mut rt, "menu-view");
        assert!(
            rt.find("menu-view-menu").is_some(),
            "View dropdown must mount"
        );
        // Start, Summary, rule, 5 design views, rule, 4 sim/hw views, rule,
        // THEME caption: Light 15, Dark 16, Follow System 17.
        tap(&mut rt, "menu-view-item16");
        assert_eq!(rt.state.theme, ThemeMode::Dark, "Dark Theme pins dark");
        assert_eq!(rt.state.chrome.open_menu, None);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "set_theme dark"),
            "theme pin must echo its Tcl verb"
        );
        tap(&mut rt, "menu-view");
        tap(&mut rt, "menu-view-item15");
        assert_eq!(rt.state.theme, ThemeMode::Light, "Light Theme pins light");
        tap(&mut rt, "menu-view");
        tap(&mut rt, "menu-view-item17");
        assert_eq!(
            rt.state.theme,
            ThemeMode::System,
            "Follow System releases the pin"
        );
    }

    #[test]
    fn file_menu_open_project_raises_a_real_open_dialog() {
        use rune::FileDialogKind;
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "menu-file");
        tap(&mut rt, "menu-file-item1");
        let (kind, opts) = rt
            .file_dialog()
            .expect("Open Project must raise a native file dialog");
        assert_eq!(kind, FileDialogKind::Open);
        assert_eq!(opts.title, "Open HDL / Project");
        assert!(
            opts.filters
                .iter()
                .any(|(n, exts)| n == "HDL" && exts.contains(&"prj".to_string())),
            "HDL filter must accept .prj: {:?}",
            opts.filters
        );
        assert_eq!(rt.state.chrome.open_menu, None);
    }

    #[test]
    fn toolbar_open_project_consumes_a_picked_hdl_file_into_the_editor() {
        use rune::FileDialogKind;
        let path =
            std::env::temp_dir().join(format!("helion-pick-{}-counter.v", std::process::id()));
        std::fs::write(&path, "module counter;\nendmodule\n").expect("fixture");
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "tool-open-project");
        assert_eq!(rt.file_dialog().map(|(k, _)| k), Some(FileDialogKind::Open));
        rt.resolve_file_dialog(Some(path.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        let tab = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            rt.state.editor.tabs.iter().any(|t| *t == tab),
            "picked file must open a tab: {:?}",
            rt.state.editor.tabs
        );
        assert!(
            rt.state
                .editor
                .lines
                .iter()
                .any(|l| l.contains("module counter")),
            "tab must hold the file contents"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == format!("open_project {}", path.display())),
            "open must echo its Tcl verb"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_project_cancel_leaves_state_untouched() {
        let mut rt = harness();
        rt.frame(0.0);
        let tabs = rt.state.editor.tabs.clone();
        tap(&mut rt, "tool-open-project");
        assert!(rt.file_dialog().is_some(), "dialog must be open to cancel");
        rt.resolve_file_dialog(None).expect("cancel the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.editor.tabs, tabs);
        assert!(rt.state.toasts.items.is_empty(), "cancel must not toast");
    }

    #[test]
    fn toolbar_undo_redo_follow_the_xdc_buffer() {
        use crate::state::CenterPane;
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut s = init();
        s.center = CenterPane::Editor;
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(0.0);
        tap(&mut rt, "editor-tab-1");
        tap(&mut rt, "editor-gutter-2");
        let before = rt.state.editor.xdc_lines[2].clone();
        tap(&mut rt, "editor-tool-comment");
        assert_ne!(rt.state.editor.xdc_lines[2], before);
        tap(&mut rt, "tool-undo");
        assert_eq!(
            rt.state.editor.xdc_lines[2], before,
            "toolbar undo must restore the xdc line"
        );
        tap(&mut rt, "tool-redo");
        assert_ne!(
            rt.state.editor.xdc_lines[2], before,
            "toolbar redo must reapply the xdc toggle"
        );
    }
}
