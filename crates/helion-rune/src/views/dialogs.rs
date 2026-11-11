//! Settings dialog + New Project and Constraints wizards (kit overlays).
//!
//! New Rune `El` code written from scratch for the Vivado brief. The Settings
//! dialog emulates `/tmp/vivado-ref/lab3-Fig1.PNG`: a left settings tree with
//! `Project Settings` (General/Simulation/Elaboration/Synthesis/Implementation/
//! Bitstream/IP) and `Tool Settings` sections, a right detail pane (the
//! Synthesis page shows Constraints, Report Settings, incremental/strategy
//! rows, the `Synth Design (helion)` option table, and the
//! `-flatten_hierarchy*` tooltip), and an `OK/Cancel/Apply/Restore...` footer.
//! The New Project wizard follows the Vivado page order (name/location, RTL
//! sources, constraints, default part, summary) and finishes with a real
//! `create_project` Tcl verb; the Constraints Wizard lives in
//! [`crate::views::xdc_wizard`] and only mounts through [`push_dialogs`].
//!
//! Modals are kit `Dialog` surfaces and every dropdown is a kit `Popover`
//! anchored to its value box; dismissal, scrim, and focus return belong to
//! Rune/kit (critique do-not-repeat: no hand-rolled wizard `Area` backdrop,
//! no `unwrap()` on UI option state, no stringly steps, no hardcoded hex).
//!
//! No `helion-gui` paint code is copied.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune-kit/src/dialog.rs`: `Dialog::new/open/width/fit/on_close/
//!   child/build` (modal panel + scrim pushed to the overlay layer).
//! - `crates/rune-kit/src/overlay.rs`: `Popover::new/trigger/open/placement/
//!   on_close/child/build`, `Placement::BottomStart`.
//! - `crates/rune-kit/src/actions.rs`: `Button::new/variant/disabled/on_press/
//!   build`, `ButtonVariant::{Submit,Neutral}`.
//! - `crates/rune-kit/src/checkbox.rs`: `Checkbox::new/label/on_toggle/build`.
//! - `crates/rune-kit/src/text_input.rs`: `TextInput::new/placeholder/on_edit/
//!   build` over `rune::TextEdit`.
//! - `crates/rune-kit/src/glyphs.rs`: `CHEVRON_DOWN`, `SEARCH`.
//! - `crates/rune-kit/src/lib.rs`: `root(base, overlays)` composes the shell
//!   with the dialog layer (wired in `crate::view`).
//! - `crates/rune/src/edit.rs`: `TextEdit::new`, its `text` field for reads.

use rune::{Align, El, ScrollSpec, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{
    Button, ButtonVariant, Checkbox, Dialog, Kit, Placement, Popover, TextInput, glyphs,
};

use crate::state::{AppState, ConsoleLine, MsgSev, ToastKind};
use crate::theme::VivadoTheme;

/// Settings-tree width (the brief shows a narrow nav beside a wide pane).
const NAV_W: f32 = 196.0;
/// Settings-tree and wizard-rail row height.
const NAV_ROW_H: f32 = 24.0;
/// Labeled control-row height.
const ROW_H: f32 = 26.0;
/// Settings field-label width (fits "Incremental synthesis:").
const LABEL_W: f32 = 150.0;
/// Wizard field-label width (fits "Project location:").
const WIZ_LABEL_W: f32 = 118.0;
/// Wizard step-rail width.
const RAIL_W: f32 = 168.0;

/// Which modal is open. `None` renders no dialog layer at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialogId {
    #[default]
    Settings,
    NewProject,
    Constraints,
    AddSources,
}

impl DialogId {
    pub(crate) fn key(self) -> &'static str {
        match self {
            DialogId::Settings => "settings",
            DialogId::NewProject => "create",
            DialogId::Constraints => "xdc",
            DialogId::AddSources => "add-sources",
        }
    }
}

/// Settings-tree pages, in `lab3-Fig1.PNG` order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    General,
    Simulation,
    Elaboration,
    #[default]
    Synthesis,
    Implementation,
    Bitstream,
    ToolProject,
    Display,
}

impl SettingsPage {
    const PROJECT: [SettingsPage; 6] = [
        SettingsPage::General,
        SettingsPage::Simulation,
        SettingsPage::Elaboration,
        SettingsPage::Synthesis,
        SettingsPage::Implementation,
        SettingsPage::Bitstream,
    ];
    const TOOL: [SettingsPage; 2] = [SettingsPage::ToolProject, SettingsPage::Display];

    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "General",
            SettingsPage::Simulation => "Simulation",
            SettingsPage::Elaboration => "Elaboration",
            SettingsPage::Synthesis => "Synthesis",
            SettingsPage::Implementation => "Implementation",
            SettingsPage::Bitstream => "Bitstream",
            SettingsPage::ToolProject => "Project",
            SettingsPage::Display => "Display",
        }
    }

    fn key(self) -> &'static str {
        match self {
            SettingsPage::General => "settings-nav-general",
            SettingsPage::Simulation => "settings-nav-simulation",
            SettingsPage::Elaboration => "settings-nav-elaboration",
            SettingsPage::Synthesis => "settings-nav-synthesis",
            SettingsPage::Implementation => "settings-nav-implementation",
            SettingsPage::Bitstream => "settings-nav-bitstream",
            SettingsPage::ToolProject => "settings-nav-tool-project",
            SettingsPage::Display => "settings-nav-display",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            SettingsPage::General => "Specify the project name, part, and top module.",
            SettingsPage::Simulation => "Simulator and simulation language.",
            SettingsPage::Elaboration => "Specify various settings associated to Elaboration.",
            SettingsPage::Synthesis => "Specify various settings associated to Synthesis",
            SettingsPage::Implementation => "Specify various settings associated to Implementation",
            SettingsPage::Bitstream => "Specify the bitstream generation options.",
            SettingsPage::ToolProject => "Where new projects are created by default.",
            SettingsPage::Display => "Theme and window layout.",
        }
    }
}

/// Which kit popover is open. One at a time; `None` closes all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopId {
    ConstrSet,
    SynthReports,
    SynthStrategy,
    Flatten,
    Gated,
    Directive,
    Fsm,
    SimSimulator,
    ImplStrategy,
    Theme,
    LayoutPreset,
    CreateConstrSet,
    CreatePart,
}

/// New Project wizard pages, in Vivado order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreateStep {
    #[default]
    Info,
    Sources,
    Constraints,
    Part,
    Summary,
}

impl CreateStep {
    const ALL: [CreateStep; 5] = [
        CreateStep::Info,
        CreateStep::Sources,
        CreateStep::Constraints,
        CreateStep::Part,
        CreateStep::Summary,
    ];

    fn title(self) -> &'static str {
        match self {
            CreateStep::Info => "Project Name",
            CreateStep::Sources => "Add Sources",
            CreateStep::Constraints => "Add Constraints",
            CreateStep::Part => "Default Part",
            CreateStep::Summary => "New Project Summary",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            CreateStep::Info => "Name the project and choose the folder that will hold it.",
            CreateStep::Sources => "Add RTL sources now, or later from the Sources pane.",
            CreateStep::Constraints => "Add XDC/SDC constraint files (clocks, pins, delays).",
            CreateStep::Part => "Choose the Helion device the project targets.",
            CreateStep::Summary => "Review the project, then create it on disk.",
        }
    }

    fn next(self) -> Option<CreateStep> {
        let i = Self::ALL.iter().position(|s| *s == self)?;
        Self::ALL.get(i + 1).copied()
    }

    fn prev(self) -> Option<CreateStep> {
        let i = Self::ALL.iter().position(|s| *s == self)?;
        i.checked_sub(1).and_then(|p| Self::ALL.get(p).copied())
    }
}

const CONSTR_SETS: &[&str] = &["constrs_1 (active)", "constrs_2"];
const SYNTH_REPORTS: &[&str] = &[
    "Helion Synthesis Default Reports (Helion Synthesis 2021)",
    "Helion Synthesis Minimal Reports",
];
/// Short report names persisted to `FlowState` (the summary cards show these).
const SYNTH_REPORTS_SHORT: &[&str] = &[
    "Helion Synthesis Default Reports",
    "Helion Synthesis Minimal Reports",
];
const SYNTH_STRATS: &[&str] = &[
    "Helion Synthesis Defaults (Helion Synthesis 2021)",
    "Helion Synthesis PerformanceOptimized",
    "Helion Synthesis AreaOptimized",
];
const SYNTH_STRAT_SHORT: &[&str] = &[
    "Helion Synthesis Defaults",
    "Helion Synthesis PerformanceOptimized",
    "Helion Synthesis AreaOptimized",
];
const FLATTEN_OPTS: &[&str] = &["rebuilt", "none", "full"];
const ON_OFF: &[&str] = &["off", "on"];
const DIRECTIVES: &[&str] = &["Default", "PerformanceOptimized", "AreaOptimized"];
const FSM_OPTS: &[&str] = &["auto", "one_hot", "gray", "sequential"];
/// Simulation language (the built-in helion-sim simulates all three).
const SIMULATORS: &[&str] = &["Mixed", "Verilog", "VHDL"];
const THEMES: &[&str] = &["Follow System", "Light", "Dark"];
/// Implementation strategies: the real `helion_proj::RunStrategy` presets
/// (each sets opt level, timing-driven placement, router effort, ...).
const IMPL_STRATS: &[&str] = &[
    "Default",
    "TimingExplore",
    "Performance_Explore",
    "Performance_NetDelay_high",
    "PhysOpt",
    "Area_Explore",
    "Congestion_SpreadLogic_high",
    "RuntimeOpt",
    "Flow_Quick",
];
/// Parts the New Project wizard offers: exactly the parts the backend can
/// place/route (`helion_device::Device::list_parts`; every entry must load —
/// see `wizard_parts_all_load_in_the_backend`).
const PARTS: &[&str] = &["HL10T-C32-1", "HL10M-C128-1", "HL10S-C64-1"];

/// Index into an option list that can never panic on UI state.
fn pick<'a>(opts: &[&'a str], i: usize) -> &'a str {
    opts.get(i).copied().unwrap_or("None")
}

/// Settings dialog state: tree selection plus one index per dropdown.
#[derive(Clone, Debug)]
pub struct SettingsState {
    pub page: SettingsPage,
    pub filter: TextEdit,
    pub constr_set: usize,
    pub synth_reports: usize,
    pub synth_strategy: usize,
    pub flatten: usize,
    pub gated: usize,
    pub bufg: TextEdit,
    pub directive: usize,
    pub retiming: bool,
    pub fsm: usize,
    pub simulator: usize,
    pub impl_strategy: usize,
    pub bin_file: bool,
    /// Incremental-synthesis checkpoint picked via the `...` browse button
    /// (empty = the automatic checkpoint, never a sentinel path).
    pub incr_checkpoint: String,
    /// `tcl.pre` / `tcl.post` hook scripts picked via their `...` buttons.
    pub tcl_pre: String,
    pub tcl_post: String,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            page: SettingsPage::Synthesis,
            filter: TextEdit::new(""),
            constr_set: 0,
            synth_reports: 0,
            synth_strategy: 0,
            flatten: 0,
            gated: 0,
            bufg: TextEdit::new("12"),
            directive: 0,
            retiming: false,
            fsm: 0,
            simulator: 0,
            impl_strategy: 0,
            bin_file: false,
            incr_checkpoint: String::new(),
            tcl_pre: String::new(),
            tcl_post: String::new(),
        }
    }
}

/// Add Sources dialog state: the picked file kind plus the files chosen via
/// the native picker (echoed as `add_files` by the Add footer).
#[derive(Clone, Debug, Default)]
pub struct AddSourcesState {
    pub kind: usize,
    pub files: Vec<String>,
}

/// New Project wizard state.
#[derive(Clone, Debug)]
pub struct CreateState {
    pub step: CreateStep,
    pub name: TextEdit,
    pub location: TextEdit,
    pub top: TextEdit,
    pub constr_set: usize,
    pub part: usize,
    pub error: Option<String>,
    /// RTL files picked on the Add Sources page.
    pub sources: Vec<String>,
    /// XDC/SDC files picked on the Add Constraints page.
    pub constraints: Vec<String>,
}

impl Default for CreateState {
    fn default() -> Self {
        Self {
            step: CreateStep::Info,
            name: TextEdit::new(""),
            location: TextEdit::new(
                crate::pickers::default_projects_dir()
                    .display()
                    .to_string(),
            ),
            top: TextEdit::new(""),
            constr_set: 0,
            part: 0,
            error: None,
            sources: Vec::new(),
            constraints: Vec::new(),
        }
    }
}

/// All dialog/wizard UI state. Composed into [`AppState`], never a god-model.
#[derive(Clone, Debug, Default)]
pub struct DialogState {
    pub open: Option<DialogId>,
    pub pop: Option<PopId>,
    pub settings: SettingsState,
    /// Last applied settings: OK/Apply commit the draft here (and to
    /// `FlowState`/project); Cancel and scrim-dismiss revert the draft back.
    pub committed: SettingsState,
    pub create: CreateState,
    /// Constraints Wizard drafts, validation and write/re-run outcome.
    pub xdc: crate::views::xdc_wizard::XdcWizard,
    pub add_sources: AddSourcesState,
}

impl DialogState {
    pub fn close(&mut self) {
        self.open = None;
        self.pop = None;
    }

    fn toggle_pop(&mut self, id: PopId) {
        if self.pop == Some(id) {
            self.pop = None;
        } else {
            self.pop = Some(id);
        }
    }

    pub fn open(&mut self, id: DialogId) {
        self.open = Some(id);
        self.pop = None;
    }
}

pub(crate) fn ui_label(content: String, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

pub(crate) fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

/// One `label: control` row; labels right-align like the Vivado brief.
fn field_row(label_text: &str, label_w: f32, control: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(8.0)
        .child(
            el().w(label_w).child(
                text(label_text)
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim)
                    .align(Align::End),
            ),
        )
        .child(control)
}

/// Incremental-checkpoint value text: the picked path, or the automatic
/// placeholder while nothing is picked.
fn incr_checkpoint_label(st: &SettingsState) -> &str {
    if st.incr_checkpoint.is_empty() {
        "Automatically selected checkpoint"
    } else {
        &st.incr_checkpoint
    }
}

/// Read-only value box (checkpoint path, description, part family).
fn value_box(content: &str, t: &VivadoTheme) -> El {
    el().w_full()
        .h(24.0)
        .px(8.0)
        .center_y()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(ui_label(content.into(), t).truncate())
}

/// The `...` browse button beside checkpoint/path fields. Raises a REAL
/// native dialog via [`Tx`]; `on` requests it (see [`crate::pickers`]).
fn dots_btn(
    key: &'static str,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label("Browse")
        .focusable(true)
        .w(30.0)
        .h(24.0)
        .center()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .hover(t.selection)
        .on_press_tx(on)
        .child(ui_label("...".into(), t))
}

pub(crate) fn dlg_button(
    key: impl Into<String>,
    label: &'static str,
    primary: bool,
    kit: &Kit,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let variant = if primary {
        ButtonVariant::Submit
    } else {
        ButtonVariant::Neutral
    };
    Button::new(label)
        .variant(variant)
        .on_press::<AppState>(on)
        .build(kit)
        .key(key)
}

/// A dialog footer button that raises a native dialog (Save pickers, ...).
pub(crate) fn dlg_button_tx(
    key: impl Into<String>,
    label: &'static str,
    primary: bool,
    kit: &Kit,
    on: impl Fn(&mut AppState, &mut Tx) + 'static,
) -> El {
    let variant = if primary {
        ButtonVariant::Submit
    } else {
        ButtonVariant::Neutral
    };
    Button::new(label)
        .variant(variant)
        .on_press_tx::<AppState>(on)
        .build(kit)
        .key(key)
}

fn text_field(
    key: impl Into<String>,
    edit: &TextEdit,
    placeholder: &'static str,
    kit: &Kit,
    on_edit: impl for<'a> Fn(&'a mut AppState) -> &'a mut TextEdit + 'static,
) -> El {
    TextInput::new(edit)
        .placeholder(placeholder)
        .on_edit(on_edit)
        .build(kit)
        .key(key)
        .grow(1.0)
        .min_w(0.0)
}

/// A dropdown: value box trigger plus a kit popover option list.
/// `options` are `(index, label)` pairs; `on_pick` stores the choice.
fn pop_select(
    layer: &mut Vec<El>,
    kit: &Kit,
    t: &VivadoTheme,
    key: &'static str,
    label: &'static str,
    pop: PopId,
    current: &str,
    options: &[(usize, String)],
    is_open: bool,
    on_pick: impl Fn(&mut AppState, usize) + Clone + 'static,
) -> El {
    let trigger = el()
        .key(format!("{key}-box"))
        .role("button")
        .label(format!("{label}, current {current}"))
        .focusable(true)
        .w_full()
        .h(24.0)
        .px(8.0)
        .center_y()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.dialogs.toggle_pop(pop))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(8.0)
                .child(ui_label(current.into(), t).grow(1.0).min_w(0.0).truncate())
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 12.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        );
    let mut opts = col().w_full().gap(0.0);
    for (index, opt_label) in options.iter() {
        let i = *index;
        let pick = on_pick.clone();
        let selected = opt_label == current;
        let mut opt_row = el()
            .key(format!("{key}-opt-{i}"))
            .role("button")
            .label(format!("Choose {opt_label}"))
            .focusable(true)
            .w_full()
            .h(24.0)
            .px(8.0)
            .center_y()
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                pick(s, i);
                s.dialogs.pop = None;
            })
            .child(ui_label(opt_label.clone(), t).truncate());
        if selected {
            opt_row = opt_row.fill(t.selection);
        }
        opts = opts.child(opt_row);
    }
    Popover::new(key)
        .trigger(trigger)
        .open(is_open)
        .placement(Placement::BottomStart)
        .padding(4.0)
        .on_close(|s: &mut AppState| s.dialogs.pop = None)
        .child(opts)
        .build(kit, layer)
}

/// Options for a static `&[&str]` list.
fn static_opts(opts: &[&str]) -> Vec<(usize, String)> {
    opts.iter()
        .enumerate()
        .map(|(i, o)| (i, o.to_string()))
        .collect()
}

/// One settings-tree row.
fn settings_nav_row(page: SettingsPage, s: &AppState, t: &VivadoTheme) -> El {
    let selected = s.dialogs.settings.page == page;
    let mut item = el()
        .key(page.key())
        .role("button")
        .label(format!("Settings page: {}", page.label()))
        .focusable(true)
        .w_full()
        .h(NAV_ROW_H)
        .px(8.0)
        .center_y()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.dialogs.settings.page = page)
        .child(ui_label(page.label().into(), t).truncate());
    if selected {
        item = item.fill(t.selection);
    }
    item
}

/// The left settings tree with its filter box (`lab3-Fig1.PNG` left pane).
fn settings_nav(s: &AppState, t: &VivadoTheme, kit: &Kit) -> El {
    let q = s.dialogs.settings.filter.text.trim().to_lowercase();
    let mut nav = col()
        .key("settings-nav")
        .w(NAV_W)
        .gap(2.0)
        .py(8.0)
        .fill(t.rail_bg)
        .child(
            el().px(8.0).pb(4.0).child(
                row().w_full().items_center().gap(4.0).child(
                    text_field(
                        "settings-filter",
                        &s.dialogs.settings.filter,
                        "Q",
                        kit,
                        |s: &mut AppState| &mut s.dialogs.settings.filter,
                    )
                    .grow(1.0)
                    .min_w(0.0),
                ),
            ),
        );
    for (section, pages) in [
        ("Project Settings", &SettingsPage::PROJECT[..]),
        ("Tool Settings", &SettingsPage::TOOL[..]),
    ] {
        let shown: Vec<SettingsPage> = pages
            .iter()
            .copied()
            .filter(|p| q.is_empty() || p.label().to_lowercase().contains(&q))
            .collect();
        if shown.is_empty() {
            continue;
        }
        nav = nav.child(
            el().px(8.0).child(
                text(section)
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .truncate(),
            ),
        );
        for page in shown {
            nav = nav.child(settings_nav_row(page, s, t));
        }
    }
    nav
}

/// Bold section header inside a settings page.
fn page_section(title: &'static str, t: &VivadoTheme) -> El {
    text(title).font(VivadoTheme::FONT_TITLE).color(t.text)
}

/// Page title + blurb + rule, as in the brief.
fn page_head(page: SettingsPage, t: &VivadoTheme) -> El {
    col()
        .w_full()
        .gap(2.0)
        .child(
            text(page.label())
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(dim_label(page.blurb(), t).truncate())
        .child(
            el().w_full()
                .h(1.0)
                .mt(4.0)
                .fill(t.border_light)
                .a11y_hidden(),
        )
}

/// One `Synth Design (helion)` option row: name plus control.
fn synth_opt_row(name: &'static str, control: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(8.0)
        .child(el().w(LABEL_W).child(ui_label(name.into(), t).truncate()))
        .child(control.grow(1.0).min_w(0.0))
}

/// The Synthesis page: Constraints, Report Settings, strategy rows, the
/// `Synth Design (helion)` option table, and the flatten tooltip.
fn synthesis_page(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) -> El {
    let st = &s.dialogs.settings;
    let pop = s.dialogs.pop;
    col()
        .key("settings-body")
        .w_full()
        .gap(4.0)
        .child(page_head(SettingsPage::Synthesis, t))
        .child(page_section("Constraints", t))
        .child(field_row(
            "Constraints",
            LABEL_W,
            pop_select(
                layer,
                kit,
                t,
                "settings-constrs",
                "Constraints",
                PopId::ConstrSet,
                pick(CONSTR_SETS, st.constr_set),
                &static_opts(CONSTR_SETS),
                pop == Some(PopId::ConstrSet),
                |s: &mut AppState, i| s.dialogs.settings.constr_set = i,
            )
            .grow(1.0)
            .min_w(0.0),
            t,
        ))
        .child(page_section("Report Settings", t))
        .child(field_row(
            "Strategy:",
            LABEL_W,
            pop_select(
                layer,
                kit,
                t,
                "settings-reports",
                "Report strategy",
                PopId::SynthReports,
                pick(SYNTH_REPORTS, st.synth_reports),
                &static_opts(SYNTH_REPORTS),
                pop == Some(PopId::SynthReports),
                |s: &mut AppState, i| s.dialogs.settings.synth_reports = i,
            )
            .grow(1.0)
            .min_w(0.0),
            t,
        ))
        .child(page_section("Settings", t))
        .child(field_row(
            "Incremental synthesis:",
            LABEL_W,
            row()
                .grow(1.0)
                .min_w(0.0)
                .items_center()
                .gap(4.0)
                .child(value_box(incr_checkpoint_label(st), t).grow(1.0).min_w(0.0))
                .child(dots_btn(
                    "settings-incr-browse",
                    t,
                    |_: &mut AppState, tx: &mut Tx| {
                        crate::pickers::checkpoint(tx);
                    },
                )),
            t,
        ))
        .child(field_row(
            "Strategy:",
            LABEL_W,
            row().grow(1.0).min_w(0.0).items_center().gap(4.0).child(
                pop_select(
                    layer,
                    kit,
                    t,
                    "settings-strategy",
                    "Synthesis strategy",
                    PopId::SynthStrategy,
                    pick(SYNTH_STRATS, st.synth_strategy),
                    &static_opts(SYNTH_STRATS),
                    pop == Some(PopId::SynthStrategy),
                    |s: &mut AppState, i| s.dialogs.settings.synth_strategy = i,
                )
                .grow(1.0)
                .min_w(0.0),
            ),
            t,
        ))
        .child(field_row(
            "Description:",
            LABEL_W,
            value_box(pick(SYNTH_STRAT_SHORT, st.synth_strategy), t)
                .grow(1.0)
                .min_w(0.0),
            t,
        ))
        .child(
            row()
                .w_full()
                .h(ROW_H)
                .items_center()
                .gap(4.0)
                .child(
                    icon(&glyphs::TRIANGLE_DOWN, 10.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                )
                .child(
                    text("Synth Design (helion)")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                ),
        )
        .child(synth_opt_row(
            "tcl.pre",
            row()
                .items_center()
                .gap(4.0)
                .child(value_box(&st.tcl_pre, t).grow(1.0).min_w(0.0))
                .child(dots_btn(
                    "settings-tclpre-browse",
                    t,
                    |_: &mut AppState, tx: &mut Tx| {
                        crate::pickers::tcl_hook(tx, true);
                    },
                )),
            t,
        ))
        .child(synth_opt_row(
            "tcl.post",
            row()
                .items_center()
                .gap(4.0)
                .child(value_box(&st.tcl_post, t).grow(1.0).min_w(0.0))
                .child(dots_btn(
                    "settings-tclpost-browse",
                    t,
                    |_: &mut AppState, tx: &mut Tx| {
                        crate::pickers::tcl_hook(tx, false);
                    },
                )),
            t,
        ))
        .child(synth_opt_row(
            "-flatten_hierarchy*",
            pop_select(
                layer,
                kit,
                t,
                "settings-flatten",
                "Flatten hierarchy",
                PopId::Flatten,
                pick(FLATTEN_OPTS, st.flatten),
                &static_opts(FLATTEN_OPTS),
                pop == Some(PopId::Flatten),
                |s: &mut AppState, i| s.dialogs.settings.flatten = i,
            ),
            t,
        ))
        .child(synth_opt_row(
            "-gated_clock_conversion",
            pop_select(
                layer,
                kit,
                t,
                "settings-gated",
                "Gated clock conversion",
                PopId::Gated,
                pick(ON_OFF, st.gated),
                &static_opts(ON_OFF),
                pop == Some(PopId::Gated),
                |s: &mut AppState, i| s.dialogs.settings.gated = i,
            ),
            t,
        ))
        .child(synth_opt_row(
            "-bufg",
            text_field("settings-bufg", &st.bufg, "12", kit, |s: &mut AppState| {
                &mut s.dialogs.settings.bufg
            }),
            t,
        ))
        .child(synth_opt_row(
            "-directive",
            pop_select(
                layer,
                kit,
                t,
                "settings-directive",
                "Directive",
                PopId::Directive,
                pick(DIRECTIVES, st.directive),
                &static_opts(DIRECTIVES),
                pop == Some(PopId::Directive),
                |s: &mut AppState, i| s.dialogs.settings.directive = i,
            ),
            t,
        ))
        .child(synth_opt_row(
            "-retiming",
            Checkbox::new(st.retiming)
                .label("on")
                .on_toggle::<AppState>(|s: &mut AppState| {
                    s.dialogs.settings.retiming = !s.dialogs.settings.retiming;
                })
                .build(kit)
                .key("settings-retiming"),
            t,
        ))
        .child(synth_opt_row(
            "-fsm_extraction",
            pop_select(
                layer,
                kit,
                t,
                "settings-fsm",
                "FSM extraction",
                PopId::Fsm,
                pick(FSM_OPTS, st.fsm),
                &static_opts(FSM_OPTS),
                pop == Some(PopId::Fsm),
                |s: &mut AppState, i| s.dialogs.settings.fsm = i,
            ),
            t,
        ))
        .child(
            col()
                .key("settings-flatten-tip")
                .w_full()
                .gap(0.0)
                .padding(6.0)
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .child(
                    text("-flatten_hierarchy*")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                )
                .child(dim_label("Flatten hierarchy during LUT mapping.", t)),
        )
}

/// A compact fallback page: title, blurb, and one representative control.
fn simple_page(
    layer: &mut Vec<El>,
    page: SettingsPage,
    s: &AppState,
    t: &VivadoTheme,
    kit: &Kit,
) -> El {
    if page == SettingsPage::Synthesis {
        return synthesis_page(layer, s, t, kit);
    }
    let st = &s.dialogs.settings;
    let pop = s.dialogs.pop;
    let mut body = col()
        .key("settings-body")
        .w_full()
        .gap(4.0)
        .child(page_head(page, t));
    match page {
        SettingsPage::General => {
            body = body
                .child(field_row(
                    "Project name:",
                    LABEL_W,
                    value_box(&s.project.name, t).grow(1.0).min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Project location:",
                    LABEL_W,
                    value_box(&s.project.location, t).grow(1.0).min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Project part:",
                    LABEL_W,
                    value_box(&s.project.part, t).grow(1.0).min_w(0.0),
                    t,
                ));
        }
        SettingsPage::Simulation => {
            body = body.child(field_row(
                "Simulator:",
                LABEL_W,
                value_box("helion-sim (built-in event-driven)", t)
                    .grow(1.0)
                    .min_w(0.0),
                t,
            ));
            body = body.child(field_row(
                "Language:",
                LABEL_W,
                pop_select(
                    layer,
                    kit,
                    t,
                    "settings-sim",
                    "Simulator",
                    PopId::SimSimulator,
                    pick(SIMULATORS, st.simulator),
                    &static_opts(SIMULATORS),
                    pop == Some(PopId::SimSimulator),
                    |s: &mut AppState, i| s.dialogs.settings.simulator = i,
                )
                .grow(1.0)
                .min_w(0.0),
                t,
            ));
        }
        SettingsPage::Elaboration => {
            body = body.child(field_row(
                "Top module:",
                LABEL_W,
                value_box(s.project.top.as_deref().unwrap_or("Not defined"), t)
                    .grow(1.0)
                    .min_w(0.0),
                t,
            ));
        }
        SettingsPage::Implementation => {
            body = body.child(field_row(
                "Strategy:",
                LABEL_W,
                pop_select(
                    layer,
                    kit,
                    t,
                    "settings-impl",
                    "Implementation strategy",
                    PopId::ImplStrategy,
                    pick(IMPL_STRATS, st.impl_strategy),
                    &static_opts(IMPL_STRATS),
                    pop == Some(PopId::ImplStrategy),
                    |s: &mut AppState, i| s.dialogs.settings.impl_strategy = i,
                )
                .grow(1.0)
                .min_w(0.0),
                t,
            ));
        }
        SettingsPage::Bitstream => {
            body = body.child(field_row(
                "Bin file:",
                LABEL_W,
                Checkbox::new(st.bin_file)
                    .label("Write binary configuration file")
                    .on_toggle::<AppState>(|s: &mut AppState| {
                        s.dialogs.settings.bin_file = !s.dialogs.settings.bin_file;
                    })
                    .build(kit)
                    .key("settings-binfile"),
                t,
            ));
        }
        SettingsPage::ToolProject => {
            body = body
                .child(field_row(
                    "Default directory:",
                    LABEL_W,
                    value_box(
                        &crate::pickers::default_projects_dir().display().to_string(),
                        t,
                    )
                    .grow(1.0)
                    .min_w(0.0),
                    t,
                ))
                .child(dim_label(
                    "New Project proposes this folder; each project gets its own subfolder.",
                    t,
                ));
        }
        SettingsPage::Display => {
            use crate::theme::ThemeMode;
            let theme_ix = match s.theme {
                ThemeMode::System => 0,
                ThemeMode::Light => 1,
                ThemeMode::Dark => 2,
            };
            let presets: Vec<&'static str> = crate::layout::LayoutPreset::ALL
                .iter()
                .map(|p| p.label())
                .collect();
            let preset_ix = crate::layout::LayoutPreset::ALL
                .iter()
                .position(|p| *p == s.layout.preset)
                .unwrap_or(0);
            body = body
                .child(field_row(
                    "Theme:",
                    LABEL_W,
                    pop_select(
                        layer,
                        kit,
                        t,
                        "settings-theme",
                        "Theme",
                        PopId::Theme,
                        pick(THEMES, theme_ix),
                        &static_opts(THEMES),
                        pop == Some(PopId::Theme),
                        |s: &mut AppState, i| {
                            s.theme = match i {
                                1 => ThemeMode::Light,
                                2 => ThemeMode::Dark,
                                _ => ThemeMode::System,
                            };
                            crate::layout::save(s);
                        },
                    )
                    .grow(1.0)
                    .min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Layout:",
                    LABEL_W,
                    pop_select(
                        layer,
                        kit,
                        t,
                        "settings-layout",
                        "Layout preset",
                        PopId::LayoutPreset,
                        pick(&presets, preset_ix),
                        &static_opts(&presets),
                        pop == Some(PopId::LayoutPreset),
                        |s: &mut AppState, i| {
                            if let Some(p) = crate::layout::LayoutPreset::ALL.get(i) {
                                crate::layout::apply_preset(s, *p);
                            }
                        },
                    )
                    .grow(1.0)
                    .min_w(0.0),
                    t,
                ));
        }
        SettingsPage::Synthesis => {}
    }
    body
}

/// Commit the settings draft: persist strategies/reports to `FlowState` and the
/// simulator to the project (the summary cards read these), snapshot the draft
/// as `committed`, and echo the real `set_property` verbs.
fn apply_settings(s: &mut AppState) {
    let st = &s.dialogs.settings;
    let strat = pick(SYNTH_STRAT_SHORT, st.synth_strategy).to_string();
    let reports = pick(SYNTH_REPORTS_SHORT, st.synth_reports).to_string();
    let impl_strat = pick(IMPL_STRATS, st.impl_strategy).to_string();
    let sim = pick(SIMULATORS, st.simulator).to_string();
    let flatten = pick(FLATTEN_OPTS, st.flatten).to_string();
    s.flow.synth_strategy = strat.clone();
    s.flow.synth_reports = reports;
    s.flow.impl_strategy = impl_strat.clone();
    s.project.sim_lang = sim;
    s.dialogs.committed = s.dialogs.settings.clone();
    echo(
        s,
        format!("set_property strategy {{{strat}}} [get_runs synth_1]"),
    );
    echo(
        s,
        format!(
            "set_property STEPS.SYNTH_DESIGN.ARGS.FLATTEN_HIERARCHY {flatten} [get_runs synth_1]"
        ),
    );
    echo(
        s,
        format!("set_property strategy {{{impl_strat}}} [get_runs impl_1]"),
    );
}

/// Cancel the settings draft: drop unapplied edits back to the last applied
/// values (tree page and filter are chrome, not settings, so they stay).
fn cancel_settings(s: &mut AppState) {
    let page = s.dialogs.settings.page;
    let filter = s.dialogs.settings.filter.clone();
    s.dialogs.settings = s.dialogs.committed.clone();
    s.dialogs.settings.page = page;
    s.dialogs.settings.filter = filter;
    s.dialogs.close();
}

fn settings_footer(t: &VivadoTheme, kit: &Kit) -> El {
    row()
        .key("settings-footer")
        .w_full()
        .h(36.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .child(
            el().key("settings-help")
                .role("button")
                .label("Settings help")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .hover(t.selection)
                .on_press(|s: &mut AppState| echo(s, "help settings".into()))
                .child(ui_label("?".into(), t)),
        )
        .child(el().grow(1.0))
        .child(dlg_button(
            "settings-ok",
            "OK",
            true,
            kit,
            |s: &mut AppState| {
                apply_settings(s);
                s.dialogs.close();
            },
        ))
        .child(dlg_button(
            "settings-cancel",
            "Cancel",
            false,
            kit,
            cancel_settings,
        ))
        .child(dlg_button(
            "settings-apply",
            "Apply",
            false,
            kit,
            apply_settings,
        ))
        .child(dlg_button(
            "settings-restore",
            "Restore...",
            false,
            kit,
            |s: &mut AppState| {
                s.dialogs.settings = SettingsState::default();
            },
        ))
}

fn settings_dialog(
    layer: &mut Vec<El>,
    pops: &mut Vec<El>,
    s: &AppState,
    t: &VivadoTheme,
    kit: &Kit,
) {
    let page = s.dialogs.settings.page;
    let body = col()
        .w_full()
        .child(
            row()
                .key("settings-split")
                .w_full()
                .child(settings_nav(s, t, kit))
                .child(el().w(1.0).a11y_hidden().fill(t.border))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .min_h(0.0)
                        .max_h(480.0)
                        .padding(8.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(simple_page(pops, page, s, t, kit)),
                ),
        )
        .child(el().w_full().h(1.0).fill(t.border_light).a11y_hidden())
        .child(settings_footer(t, kit));
    Dialog::new(DialogId::Settings.key(), "Settings")
        .open(true)
        .width(880.0)
        .fit()
        .on_close(cancel_settings)
        .child(body)
        .build(kit, layer);
}

/// New Project validation: the name is required, the part index must exist.
fn create_error(c: &CreateState) -> Option<String> {
    let name = c.name.text.trim();
    if name.is_empty() {
        return Some("Project name is required.".into());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Some("Use letters, digits, '_' or '-' in the project name.".into());
    }
    if c.location.text.trim().is_empty() {
        return Some("Project location is required.".into());
    }
    if PARTS.get(c.part).is_none() {
        return Some("Choose a default part.".into());
    }
    None
}

/// Finish the New Project wizard: validate, apply to the project, echo Tcl.
fn finish_create(s: &mut AppState) {
    let err = create_error(&s.dialogs.create);
    if let Some(msg) = err {
        s.dialogs.create.step = CreateStep::Info;
        s.dialogs.create.error = Some(msg);
        return;
    }
    let name = s.dialogs.create.name.text.trim().to_string();
    let location = s.dialogs.create.location.text.trim().to_string();
    let top = s.dialogs.create.top.text.trim().to_string();
    let part = pick(PARTS, s.dialogs.create.part).to_string();
    match write_new_project(&s.dialogs.create, &name, &location, &part, &top) {
        Ok(prj) => {
            s.dialogs.close();
            s.dialogs.create = CreateState::default();
            // Opening resets the session (console included), so journal after.
            crate::pickers::open_path(s, &prj);
            echo(s, format!("create_project {name} {location} -part {part}"));
        }
        Err(e) => {
            s.dialogs.create.step = CreateStep::Summary;
            s.dialogs.create.error = Some(e);
        }
    }
}

/// Create `<location>/<name>/<name>.prj` listing the part, top, and the
/// picked sources and constraints. Refuses to overwrite an existing project.
fn write_new_project(
    c: &CreateState,
    name: &str,
    location: &str,
    part: &str,
    top: &str,
) -> Result<std::path::PathBuf, String> {
    let dir = std::path::PathBuf::from(location).join(name);
    let prj = dir.join(format!("{name}.prj"));
    if prj.exists() {
        return Err(format!("A project already exists at {}", prj.display()));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let mut text = format!(
        "# Helion project\ncreate_project {name} {}\npart {part}\n",
        dir.display()
    );
    if !top.is_empty() {
        text.push_str(&format!("top {top}\n"));
    }
    for f in &c.sources {
        let verb = if f.to_ascii_lowercase().ends_with(".vhd")
            || f.to_ascii_lowercase().ends_with(".vhdl")
        {
            "read_vhdl"
        } else {
            "read_sv"
        };
        text.push_str(&format!("{verb} {f}\n"));
    }
    for f in &c.constraints {
        let verb = if f.to_ascii_lowercase().ends_with(".sdc") {
            "read_sdc"
        } else {
            "read_xdc"
        };
        text.push_str(&format!("{verb} {f}\n"));
    }
    std::fs::write(&prj, text).map_err(|e| format!("Couldn't write {}: {e}", prj.display()))?;
    Ok(prj)
}

/// A wizard file list with per-row remove, plus the Add Files button.
fn wizard_files(
    key: &'static str,
    files: &[String],
    constraints: bool,
    empty: &'static str,
    kit: &Kit,
    t: &VivadoTheme,
) -> El {
    let mut list = col().key(key).w_full().gap(2.0);
    if files.is_empty() {
        list = list.child(dim_label(empty, t));
    }
    for (i, f) in files.iter().enumerate() {
        let name = std::path::Path::new(f)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| f.clone());
        list = list.child(
            row()
                .key(format!("{key}-{i}"))
                .w_full()
                .h(22.0)
                .items_center()
                .gap(6.0)
                .child(ui_label(name.clone(), t))
                .child(
                    text(f.clone())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_faint)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate_middle(),
                )
                .child(
                    el().key(format!("{key}-remove-{i}"))
                        .role("button")
                        .label(format!("Remove {name}"))
                        .focusable(true)
                        .size(20.0, 20.0)
                        .radius(3.0)
                        .center()
                        .hover(t.selection)
                        .on_press(move |s: &mut AppState| {
                            let l = if constraints {
                                &mut s.dialogs.create.constraints
                            } else {
                                &mut s.dialogs.create.sources
                            };
                            if i < l.len() {
                                l.remove(i);
                            }
                        })
                        .child(dim_label("×", t)),
                ),
        );
    }
    col()
        .w_full()
        .gap(6.0)
        .child(list)
        .child(row().child(dlg_button_tx(
            format!("{key}-add"),
            "Add Files…",
            false,
            kit,
            move |_: &mut AppState, tx: &mut Tx| crate::pickers::wizard_file(tx, constraints),
        )))
}

/// One wizard-rail row: step number, title, current highlight.
pub(crate) fn wiz_rail_row(
    key: String,
    index: usize,
    title: &'static str,
    current: bool,
    done: bool,
    t: &VivadoTheme,
    go: impl Fn(&mut AppState) + 'static,
) -> El {
    let ink = if current { t.text } else { t.text_dim };
    let mut item = el()
        .key(key)
        .role("button")
        .label(format!("Wizard step {}: {title}", index + 1))
        .focusable(true)
        .w_full()
        .h(NAV_ROW_H)
        .px(8.0)
        .center_y()
        .hover(t.selection)
        .on_press(go)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(6.0)
                .child(ui_label(format!("{}.", index + 1), t))
                .child(text(title).font(VivadoTheme::FONT_UI).color(ink).truncate()),
        );
    if current {
        item = item.fill(t.selection);
    } else if done {
        item = item.fill(t.rail_bg);
    }
    item
}

/// Wizard footer: Back / Next (or Finish) / Cancel plus the error line.
pub(crate) fn wiz_footer(
    key_prefix: &'static str,
    has_prev: bool,
    is_last: bool,
    error: Option<&str>,
    t: &VivadoTheme,
    kit: &Kit,
    back: impl Fn(&mut AppState) + 'static,
    next: impl Fn(&mut AppState) + 'static,
    finish: impl Fn(&mut AppState) + 'static,
) -> El {
    let mut footer = row()
        .key(format!("{key_prefix}-footer"))
        .w_full()
        .h(36.0)
        .items_center()
        .gap(8.0)
        .px(8.0);
    if let Some(msg) = error {
        footer = footer.child(
            text(msg)
                .font(VivadoTheme::FONT_UI)
                .color(t.fail)
                .truncate()
                .key(format!("{key_prefix}-error"))
                .grow(1.0)
                .min_w(0.0),
        );
    } else {
        footer = footer.child(el().grow(1.0));
    }
    let mut back_btn = Button::new("Back")
        .variant(ButtonVariant::Neutral)
        .disabled(!has_prev);
    if has_prev {
        back_btn = back_btn.on_press::<AppState>(back);
    }
    footer = footer.child(back_btn.build(kit).key(format!("{key_prefix}-back")));
    if is_last {
        footer = footer.child(dlg_button(
            format!("{key_prefix}-finish"),
            "Finish",
            true,
            kit,
            finish,
        ));
    } else {
        footer = footer.child(dlg_button(
            format!("{key_prefix}-next"),
            "Next",
            true,
            kit,
            next,
        ));
    }
    footer.child(dlg_button(
        format!("{key_prefix}-cancel"),
        "Cancel",
        false,
        kit,
        |s: &mut AppState| {
            s.dialogs.close();
        },
    ))
}

fn create_page(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) -> El {
    let c = &s.dialogs.create;
    let pop = s.dialogs.pop;
    let mut page = col()
        .key("create-body")
        .w_full()
        .gap(4.0)
        .child(
            text(c.step.title())
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(dim_label(c.step.hint(), t));
    match c.step {
        CreateStep::Info => {
            page = page
                .child(field_row(
                    "Project name:",
                    WIZ_LABEL_W,
                    text_field(
                        "create-name",
                        &c.name,
                        "project_1",
                        kit,
                        |s: &mut AppState| &mut s.dialogs.create.name,
                    ),
                    t,
                ))
                .child(field_row(
                    "Project location:",
                    WIZ_LABEL_W,
                    row()
                        .grow(1.0)
                        .min_w(0.0)
                        .items_center()
                        .gap(4.0)
                        .child(
                            text_field(
                                "create-location",
                                &c.location,
                                "~/helion-projects",
                                kit,
                                |s: &mut AppState| &mut s.dialogs.create.location,
                            )
                            .grow(1.0)
                            .min_w(0.0),
                        )
                        .child(dots_btn(
                            "create-browse",
                            t,
                            |_: &mut AppState, tx: &mut Tx| {
                                crate::pickers::project_directory(tx);
                            },
                        )),
                    t,
                ));
        }
        CreateStep::Sources => {
            page = page
                .child(wizard_files(
                    "create-sources",
                    &c.sources,
                    false,
                    "No sources yet. You can also add them later.",
                    kit,
                    t,
                ))
                .child(field_row(
                    "Top module:",
                    WIZ_LABEL_W,
                    text_field(
                        "create-top",
                        &c.top,
                        "auto-detect",
                        kit,
                        |s: &mut AppState| &mut s.dialogs.create.top,
                    ),
                    t,
                ));
        }
        CreateStep::Constraints => {
            page = page.child(wizard_files(
                "create-constraints",
                &c.constraints,
                true,
                "No constraints yet. Without a clock, timing uses an implicit 10 ns clock.",
                kit,
                t,
            ));
        }
        CreateStep::Part => {
            page = page
                .child(field_row(
                    "Part:",
                    WIZ_LABEL_W,
                    pop_select(
                        layer,
                        kit,
                        t,
                        "create-part",
                        "Default part",
                        PopId::CreatePart,
                        pick(PARTS, c.part),
                        &static_opts(PARTS),
                        pop == Some(PopId::CreatePart),
                        |s: &mut AppState, i| s.dialogs.create.part = i,
                    )
                    .grow(1.0)
                    .min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Family:",
                    WIZ_LABEL_W,
                    value_box("Helion", t).grow(1.0).min_w(0.0),
                    t,
                ));
        }
        CreateStep::Summary => {
            let name = if c.name.text.trim().is_empty() {
                "(unnamed)"
            } else {
                c.name.text.trim()
            };
            page = page
                .child(field_row(
                    "Name:",
                    WIZ_LABEL_W,
                    value_box(name, t).grow(1.0).min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Location:",
                    WIZ_LABEL_W,
                    value_box(c.location.text.trim(), t).grow(1.0).min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Part:",
                    WIZ_LABEL_W,
                    value_box(pick(PARTS, c.part), t).grow(1.0).min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Sources:",
                    WIZ_LABEL_W,
                    value_box(&format!("{} file(s)", c.sources.len()), t)
                        .grow(1.0)
                        .min_w(0.0),
                    t,
                ))
                .child(field_row(
                    "Constraints:",
                    WIZ_LABEL_W,
                    value_box(&format!("{} file(s)", c.constraints.len()), t)
                        .grow(1.0)
                        .min_w(0.0),
                    t,
                ))
                .child(
                    dim_label(
                        &format!(
                            "Creates {}/{}/{}.prj",
                            c.location.text.trim(),
                            name,
                            name
                        ),
                        t,
                    )
                    .key("create-summary-path")
                    .truncate_middle(),
                );
        }
    }
    page
}

fn create_dialog(
    layer: &mut Vec<El>,
    pops: &mut Vec<El>,
    s: &AppState,
    t: &VivadoTheme,
    kit: &Kit,
) {
    let step = s.dialogs.create.step;
    let at = CreateStep::ALL.iter().position(|x| *x == step).unwrap_or(0);
    let mut rail = col()
        .key("create-rail")
        .w(RAIL_W)
        .gap(2.0)
        .py(8.0)
        .fill(t.rail_bg);
    for (i, st) in CreateStep::ALL.iter().enumerate() {
        rail = rail.child(wiz_rail_row(
            format!("create-rail-{i}"),
            i,
            st.title(),
            *st == step,
            i < at,
            t,
            move |s: &mut AppState| s.dialogs.create.step = *st,
        ));
    }
    let err = s.dialogs.create.error.clone();
    let body = col()
        .w_full()
        .child(
            row()
                .key("create-split")
                .w_full()
                .child(rail)
                .child(el().w(1.0).a11y_hidden().fill(t.border))
                .child(
                    el().grow(1.0)
                        .min_w(0.0)
                        .padding(8.0)
                        .child(create_page(pops, s, t, kit)),
                ),
        )
        .child(el().w_full().h(1.0).fill(t.border_light).a11y_hidden())
        .child(wiz_footer(
            "create",
            step.prev().is_some(),
            step.next().is_none(),
            err.as_deref(),
            t,
            kit,
            |s: &mut AppState| {
                if let Some(p) = s.dialogs.create.step.prev() {
                    s.dialogs.create.step = p;
                    s.dialogs.create.error = None;
                }
            },
            |s: &mut AppState| {
                if let Some(n) = s.dialogs.create.step.next() {
                    s.dialogs.create.step = n;
                    s.dialogs.create.error = None;
                }
            },
            finish_create,
        ));
    Dialog::new(DialogId::NewProject.key(), "New Project")
        .open(true)
        .width(680.0)
        .fit()
        .on_close(|s: &mut AppState| s.dialogs.close())
        .child(body)
        .build(kit, layer);
}

/// Add Sources dialog: the Vivado file-type picker (RTL, constraints,
/// simulation). The kind rows select the picker's filter; Add Files raises a
/// REAL native dialog and lists each chosen path; the Add footer echoes the
/// full `add_files -fileset ...` verb and closes like a real dispatch.
fn add_sources_dialog(s: &AppState, t: &VivadoTheme, kit: &Kit, layer: &mut Vec<El>) {
    let kinds = crate::pickers::ADD_SOURCES_KINDS;
    let selected = s.dialogs.add_sources.kind.min(kinds.len() - 1);
    let mut list = col().key("add-sources-body").w_full().gap(4.0).padding(8.0);
    for (i, (label, _, _)) in kinds.iter().enumerate() {
        let mut kind_row = row()
            .key(format!("add-sources-kind-{i}"))
            .role("button")
            .label(format!("{label} files"))
            .focusable(true)
            .w_full()
            .h(24.0)
            .items_center()
            .gap(6.0)
            .px(8.0)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| s.dialogs.add_sources.kind = i)
            .child(ui_label(format!("{}. {label}", i + 1), t));
        if i == selected {
            kind_row = kind_row.fill(t.selection);
        }
        list = list.child(kind_row);
    }
    let mut body = col().w_full().child(list);
    if s.dialogs.add_sources.files.is_empty() {
        body = body.child(
            el().w_full().px(8.0).pb(4.0).child(
                dim_label("No files yet — Add Files opens the system picker.", t)
                    .key("add-sources-empty"),
            ),
        );
    } else {
        let mut files = col()
            .key("add-sources-files")
            .w_full()
            .gap(2.0)
            .px(8.0)
            .pb(4.0);
        for (i, f) in s.dialogs.add_sources.files.iter().enumerate() {
            files = files.child(
                row()
                    .key(format!("add-sources-file-{i}"))
                    .w_full()
                    .h(20.0)
                    .items_center()
                    .child(ui_label(f.clone(), t).truncate()),
            );
        }
        body = body.child(files);
    }
    body = body.child(
        row()
            .key("add-sources-footer")
            .w_full()
            .h(36.0)
            .items_center()
            .gap(8.0)
            .px(8.0)
            .child(dlg_button_tx(
                "add-sources-browse",
                "Add Files...",
                false,
                kit,
                |s: &mut AppState, tx: &mut Tx| {
                    let kind = s.dialogs.add_sources.kind;
                    crate::pickers::add_sources(tx, kind);
                },
            ))
            .child(el().grow(1.0))
            .child(dlg_button(
                "add-sources-add",
                "Add",
                true,
                kit,
                |s: &mut AppState| {
                    let (_, fileset, _) =
                        crate::pickers::add_sources_kind(s.dialogs.add_sources.kind);
                    let files = s.dialogs.add_sources.files.clone();
                    if files.is_empty() {
                        s.dialogs.close();
                        return;
                    }
                    let set = match s.dialogs.add_sources.kind {
                        1 => crate::project::FileSet::Constraints,
                        2 => crate::project::FileSet::Simulation,
                        _ => crate::project::FileSet::Design,
                    };
                    let paths: Vec<std::path::PathBuf> =
                        files.iter().map(std::path::PathBuf::from).collect();
                    let list = files
                        .iter()
                        .map(|f| format!("{{{f}}}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    match crate::project::commit_add_sources(s, &paths, set) {
                        Ok((n, prj)) => {
                            echo(s, format!("add_files -fileset {fileset} {list}"));
                            let detail = match prj {
                                Some(p) => format!("{n} file(s) · saved to {}", p.display()),
                                None => format!("{n} file(s) added to {fileset}"),
                            };
                            s.toasts.push(ToastKind::Success, "Sources added", &detail);
                            s.dialogs.add_sources.files.clear();
                            s.dialogs.close();
                        }
                        Err(e) => {
                            s.toasts.push(ToastKind::Error, "Add Sources failed", &e);
                        }
                    }
                },
            ))
            .child(dlg_button(
                "add-sources-close",
                "Close",
                false,
                kit,
                |s: &mut AppState| {
                    s.dialogs.close();
                },
            )),
    );
    Dialog::new(DialogId::AddSources.key(), "Add Sources")
        .open(true)
        .width(520.0)
        .fit()
        .on_close(|s: &mut AppState| s.dialogs.close())
        .child(body)
        .build(kit, layer);
}

/// Push the open dialog (if any) onto the overlay layer. Called from
/// `crate::view`; the shell stays dialog-free when `open` is `None`.
///
/// Popover surfaces anchor to triggers INSIDE the dialog panel, so the panel
/// must precede them in the layer: popover content is collected separately
/// and appended after `Dialog::build` pushes the scrim + panel.
pub fn push_dialogs(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    let Some(id) = s.dialogs.open else { return };
    let kit = t.kit();
    let mut pops = Vec::new();
    match id {
        DialogId::Settings => settings_dialog(layer, &mut pops, s, t, &kit),
        DialogId::NewProject => create_dialog(layer, &mut pops, s, t, &kit),
        DialogId::Constraints => {
            crate::views::xdc_wizard::xdc_dialog(layer, &mut pops, s, t, &kit)
        }
        DialogId::AddSources => add_sources_dialog(s, t, &kit, layer),
    }
    layer.extend(pops);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    /// A runtime whose state is configured BEFORE the first frame: `frame`
    /// only re-runs the view when handlers mark state dirty
    /// (`runtime/frame.rs`), so mutating `rt.state` after the fact leaves a
    /// stale tree.
    fn rt_with(f: impl Fn(&mut AppState)) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut s = init();
        f(&mut s);
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(1.0 / 60.0);
        rt
    }

    fn open(id: DialogId) -> Runtime<AppState> {
        rt_with(|s| s.dialogs.open(id))
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn settings_tree_exposes_nav_body_and_footer() {
        let mut rt = open(DialogId::Settings);
        rt.frame(0.0);
        for key in [
            "settings-dialog",
            "settings-nav",
            "settings-nav-synthesis",
            "settings-nav-bitstream",
            "settings-body",
            "settings-constrs-box",
            "settings-flatten-tip",
            "settings-ok",
            "settings-cancel",
            "settings-apply",
            "settings-restore",
        ] {
            assert!(rt.find(key).is_some(), "missing keyed region: {key}");
        }
    }

    #[test]
    fn settings_constrs_popover_pick_stores_index() {
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-constrs-box");
        assert!(rt.find("settings-constrs-content").is_some());
        tap(&mut rt, "settings-constrs-opt-1");
        assert_eq!(rt.state.dialogs.settings.constr_set, 1);
        assert!(rt.find("settings-constrs-content").is_none());
    }

    #[test]
    fn settings_ok_applies_tcl_and_closes() {
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-ok");
        assert_eq!(rt.state.dialogs.open, None);
        assert!(rt.find("settings-dialog").is_none());
        let last: Vec<&str> = rt
            .state
            .console
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        assert!(
            last.iter().any(|l| l.contains("set_property strategy")),
            "tcl: {last:?}"
        );
    }

    #[test]
    fn create_finish_requires_a_name() {
        let mut rt = rt_with(|s| {
            s.dialogs.open(DialogId::NewProject);
            s.dialogs.create.step = CreateStep::Summary;
        });
        tap(&mut rt, "create-finish");
        assert!(rt.find("create-error").is_some());
        assert_eq!(rt.state.dialogs.open, Some(DialogId::NewProject));
        assert_eq!(rt.state.dialogs.create.step, CreateStep::Info);
    }

    #[test]
    fn create_finish_applies_project_and_echoes_tcl() {
        let mut rt = rt_with(|s| {
            s.dialogs.open(DialogId::NewProject);
            s.dialogs.create.name = TextEdit::new("proj2");
            s.dialogs.create.step = CreateStep::Summary;
        });
        let name = format!("proj2_{}", std::process::id());
        rt.state.dialogs.create.name = TextEdit::new(name.clone());
        rt.invalidate();
        rt.frame(0.05);
        tap(&mut rt, "create-finish");
        assert_eq!(rt.state.dialogs.open, None, "{:?}", rt.state.dialogs.create.error);
        assert_eq!(rt.state.project.name, name);
        let prj = crate::pickers::default_projects_dir()
            .join(&name)
            .join(format!("{name}.prj"));
        assert!(prj.is_file(), "project file must exist on disk");
        assert_eq!(rt.state.runs.source.as_deref(), Some(prj.as_path()));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with(&format!("create_project {name}"))),
            "create journals its Tcl"
        );
        let _ = std::fs::remove_dir_all(prj.parent().unwrap());
    }

    #[test]
    fn wizard_parts_all_load_in_the_backend() {
        for part in PARTS {
            assert!(
                helion_device::Device::load_part(part).is_ok(),
                "wizard part {part} must have a backend HAD model"
            );
        }
    }

    #[test]
    fn create_next_walks_pages_and_back_returns() {
        let mut rt = open(DialogId::NewProject);
        tap(&mut rt, "create-next");
        assert_eq!(rt.state.dialogs.create.step, CreateStep::Sources);
        tap(&mut rt, "create-back");
        assert_eq!(rt.state.dialogs.create.step, CreateStep::Info);
    }

    #[test]
    fn settings_apply_persists_strategies_and_stays_open() {
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-strategy-box");
        tap(&mut rt, "settings-strategy-opt-1");
        tap(&mut rt, "settings-apply");
        assert_eq!(
            rt.state.dialogs.open,
            Some(DialogId::Settings),
            "Apply keeps the dialog open"
        );
        assert_eq!(
            rt.state.flow.synth_strategy,
            "Helion Synthesis PerformanceOptimized"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.contains("[get_runs synth_1]")),
            "apply must echo the set_property verbs"
        );
    }

    #[test]
    fn settings_cancel_reverts_the_draft_without_persisting() {
        let mut rt = open(DialogId::Settings);
        let before = rt.state.flow.synth_strategy.clone();
        tap(&mut rt, "settings-strategy-box");
        tap(&mut rt, "settings-strategy-opt-1");
        assert_eq!(rt.state.dialogs.settings.synth_strategy, 1);
        tap(&mut rt, "settings-cancel");
        assert_eq!(rt.state.dialogs.open, None);
        assert_eq!(
            rt.state.flow.synth_strategy, before,
            "Cancel must not persist"
        );
        assert_eq!(
            rt.state.dialogs.settings.synth_strategy, 0,
            "Cancel must revert the draft to the last applied value"
        );
    }

    #[test]
    fn settings_restore_resets_the_draft_without_committing() {
        let mut rt = rt_with(|s| {
            s.dialogs.open(DialogId::Settings);
            s.dialogs.settings.synth_strategy = 1;
            s.dialogs.settings.flatten = 2;
        });
        tap(&mut rt, "settings-restore");
        assert_eq!(rt.state.dialogs.settings.synth_strategy, 0);
        assert_eq!(rt.state.dialogs.settings.flatten, 0);
        assert_eq!(
            rt.state.dialogs.open,
            Some(DialogId::Settings),
            "Restore keeps the dialog open"
        );
    }

    #[test]
    fn create_finish_reseeds_runs_status_and_reports() {
        use crate::state::{RunStatus, StatusVerb, ToastKind};
        let mut rt = rt_with(|s| {
            s.dialogs.open(DialogId::NewProject);
            s.dialogs.create.name = TextEdit::new("proj2");
            s.dialogs.create.step = CreateStep::Summary;
            s.flow.synth = RunStatus::Complete;
            s.flow.impl_run = RunStatus::Failed;
            s.status = StatusVerb::Failed;
            s.timing.has_run = true;
            s.timing.wns_ps = Some(2061);
            s.toasts
                .push(ToastKind::Error, "Implementation failed", "impl_1 failed");
            s.nav_back.push(crate::state::CenterPane::Editor);
        });
        tap(&mut rt, "create-finish");
        assert_eq!(rt.state.dialogs.open, None);
        assert_eq!(rt.state.project.name, "proj2");
        assert_eq!(
            rt.state.flow.synth,
            RunStatus::NotStarted,
            "new project resets runs"
        );
        assert_eq!(rt.state.flow.impl_run, RunStatus::NotStarted);
        assert_eq!(rt.state.status, StatusVerb::Ready);
        assert!(!rt.state.timing.has_run, "new project clears timing");
        assert!(
            !rt.state
                .toasts
                .items
                .iter()
                .any(|t| t.title == "Implementation failed"),
            "new project clears the old project's toasts"
        );
        let _ = std::fs::remove_dir_all(
            crate::pickers::default_projects_dir().join("proj2"),
        );
        assert!(
            rt.state.nav_back.is_empty(),
            "new project clears nav history"
        );
    }

    #[test]
    fn settings_popover_outside_press_dismisses_without_closing_the_dialog() {
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-constrs-box");
        assert!(rt.find("settings-constrs-content").is_some());
        // The next press lands outside the popover content: the kit scrim
        // consumes it and closes the popover, not the dialog.
        tap(&mut rt, "settings-cancel");
        assert_eq!(
            rt.state.dialogs.pop, None,
            "outside press must dismiss the popover"
        );
        assert_eq!(
            rt.state.dialogs.open,
            Some(DialogId::Settings),
            "dialog must stay open"
        );
        assert!(rt.find("settings-dialog").is_some());
        tap(&mut rt, "settings-cancel");
        assert_eq!(
            rt.state.dialogs.open, None,
            "the dialog still closes on its own button"
        );
    }

    #[test]
    fn create_location_browse_consumes_the_picked_folder() {
        use rune::FileDialogKind;
        let mut rt = open(DialogId::NewProject);
        tap(&mut rt, "create-browse");
        let (kind, opts) = rt
            .file_dialog()
            .expect("location Browse must raise a folder dialog");
        assert_eq!(kind, FileDialogKind::Folder);
        assert_eq!(opts.title, "Project Directory");
        let dir = std::env::temp_dir().join(format!("helion-pick-{}", std::process::id()));
        rt.resolve_file_dialog(Some(dir.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.dialogs.create.location.text,
            dir.display().to_string()
        );
    }

    #[test]
    fn settings_checkpoint_browse_consumes_the_picked_file() {
        use rune::FileDialogKind;
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-incr-browse");
        let (kind, opts) = rt
            .file_dialog()
            .expect("checkpoint Browse must raise a dialog");
        assert_eq!(kind, FileDialogKind::Open);
        assert!(
            opts.filters
                .iter()
                .any(|(n, exts)| n == "Checkpoint" && exts.contains(&"hck2".to_string())),
            "filter must accept checkpoints: {:?}",
            opts.filters
        );
        let picked = std::env::temp_dir().join("post_route.hck2");
        rt.resolve_file_dialog(Some(picked.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.dialogs.settings.incr_checkpoint,
            picked.display().to_string()
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("open_checkpoint ")),
            "checkpoint pick must echo its Tcl verb"
        );
    }

    #[test]
    fn settings_tcl_hook_browse_consumes_the_picked_script() {
        use rune::FileDialogKind;
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-tclpre-browse");
        let (kind, opts) = rt
            .file_dialog()
            .expect("tcl.pre Browse must raise a dialog");
        assert_eq!(kind, FileDialogKind::Open);
        assert!(
            opts.filters
                .iter()
                .any(|(n, exts)| n == "Tcl" && exts.contains(&"tcl".to_string())),
            "filter must accept Tcl: {:?}",
            opts.filters
        );
        let picked = std::env::temp_dir().join("hook_pre.tcl");
        rt.resolve_file_dialog(Some(picked.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.dialogs.settings.tcl_pre,
            picked.display().to_string()
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.contains("add_files -fileset utils_1")),
            "hook pick must echo add_files"
        );
    }

    #[test]
    fn add_sources_browse_consumes_picked_files_per_kind() {
        use rune::FileDialogKind;
        let mut rt = open(DialogId::AddSources);
        tap(&mut rt, "add-sources-browse");
        let (kind, opts) = rt.file_dialog().expect("Add Files must raise a dialog");
        assert_eq!(kind, FileDialogKind::Open);
        assert!(
            opts.filters
                .iter()
                .any(|(_, exts)| exts.contains(&"sv".to_string())),
            "design kind must filter RTL: {:?}",
            opts.filters
        );
        rt.resolve_file_dialog(Some(std::env::temp_dir().join("uart_rx.v")))
            .expect("answer");
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state
                .dialogs
                .add_sources
                .files
                .iter()
                .any(|f| f.ends_with("uart_rx.v"))
        );
        tap(&mut rt, "add-sources-kind-1");
        tap(&mut rt, "add-sources-browse");
        let (_, opts) = rt.file_dialog().expect("second pick must raise again");
        assert!(
            opts.filters
                .iter()
                .any(|(_, exts)| exts.contains(&"xdc".to_string())),
            "constraints kind must filter XDC: {:?}",
            opts.filters
        );
        rt.resolve_file_dialog(Some(std::env::temp_dir().join("pins.xdc")))
            .expect("answer");
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.dialogs.add_sources.files.len(), 2);
    }

    #[test]
    fn add_sources_add_echoes_the_fileset_and_closes() {
        let mut rt = rt_with(|s| {
            s.dialogs.open(DialogId::AddSources);
            s.dialogs.add_sources.files.push("/tmp/uart_rx.v".into());
        });
        tap(&mut rt, "add-sources-add");
        assert_eq!(rt.state.dialogs.open, None);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.contains("add_files") && l.text.contains("uart_rx.v")),
            "Add must echo the picked files"
        );
    }

    #[test]
    fn settings_popover_content_anchors_below_its_trigger() {
        let mut rt = open(DialogId::Settings);
        tap(&mut rt, "settings-strategy-box");
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        let (trigger, _) = rt.presented("settings-strategy-box").expect("trigger rect");
        let (content, _) = rt
            .presented("settings-strategy-content")
            .expect("content rect");
        assert!(
            content[1] >= trigger[1] + trigger[3] - 1.0,
            "BottomStart content must sit below the trigger: trigger {trigger:?} content {content:?}"
        );
        assert!(
            (content[0] - trigger[0]).abs() < 2.0,
            "BottomStart content must align to the trigger start: trigger {trigger:?} content {content:?}"
        );
    }
}
