//! Sources tree + Source File Properties card over the real project files
//! ([`crate::project::ProjectFiles`]).
//!
//! The Sources pane groups files the Vivado way (Design Sources /
//! Constraints › constrs_1 / Simulation Sources › sim_1) with Hierarchy /
//! Libraries / Compile Order tabs, a name filter, expand/collapse all and Add
//! Sources. The Source File Properties card below shows the selected file's
//! real location, language, fileset, size and declared module, with working
//! previous/next stepping. Single click selects; double click opens the file
//! in the editor.
//!
//! Selection is a [`SourceFileId`]: the three roles the rest of the shell
//! cross-probes to (design top, first constraints file, first testbench) or
//! any file by index.

use rune::{El, ScrollSpec, TextEdit, col, el, icon, row, text, text_input};
use rune_kit::glyphs;

use crate::project::{FileSet, ProjectFile};
use crate::state::AppState;
use crate::theme::VivadoTheme;

/// Default width of the Sources + Properties column (see `layout`).
pub const SOURCES_W: f32 = crate::layout::SOURCES_W;
/// Tree indent per depth level.
const INDENT: f32 = 14.0;
/// Tree/file row height (compact CAD density).
const ROW_H: f32 = 20.0;
/// Pane header height.
const HEADER_H: f32 = 24.0;
/// HDL libraries are not modeled per file: everything compiles into `work`.
const LIBRARY: &str = "work";

/// A source selection: a cross-probe role or a file index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceFileId {
    /// The design top (file declaring the project top, else the first
    /// design source).
    #[default]
    Demo,
    /// The first constraints file.
    Xdc,
    /// The first simulation source.
    Tb,
    /// Any project file by index.
    Index(usize),
}

impl SourceFileId {
    /// The project file this selection points at, if it exists.
    pub fn resolve(self, s: &AppState) -> Option<usize> {
        let files = &s.project_files;
        match self {
            SourceFileId::Demo => files.top_index(s.project.top.as_deref()),
            SourceFileId::Xdc => files.in_set(FileSet::Constraints).map(|(i, _)| i).next(),
            SourceFileId::Tb => files.in_set(FileSet::Simulation).map(|(i, _)| i).next(),
            SourceFileId::Index(i) => (i < files.files.len()).then_some(i),
        }
    }

    /// The role (if any) file `i` plays, so rows keep stable keys and the
    /// selection compares equal to what cross-probes set.
    pub fn for_index(s: &AppState, i: usize) -> SourceFileId {
        for role in [SourceFileId::Demo, SourceFileId::Xdc, SourceFileId::Tb] {
            if role.resolve(s) == Some(i) {
                return role;
            }
        }
        SourceFileId::Index(i)
    }

    /// Stable tree-row key.
    pub fn key(self) -> String {
        match self {
            SourceFileId::Demo => "src-design-top".into(),
            SourceFileId::Xdc => "src-xdc".into(),
            SourceFileId::Tb => "src-tb".into(),
            SourceFileId::Index(i) => format!("src-file-{i}"),
        }
    }
}

/// Tree parents with children (each shows a `(n)` count of direct children).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceParent {
    Design,
    Constr,
    ConstrSet,
    Sim,
    SimSet,
}

impl SourceParent {
    pub const ALL: [SourceParent; 5] = [
        SourceParent::Design,
        SourceParent::Constr,
        SourceParent::ConstrSet,
        SourceParent::Sim,
        SourceParent::SimSet,
    ];

    pub fn index(self) -> usize {
        match self {
            SourceParent::Design => 0,
            SourceParent::Constr => 1,
            SourceParent::ConstrSet => 2,
            SourceParent::Sim => 3,
            SourceParent::SimSet => 4,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SourceParent::Design => "src-design",
            SourceParent::Constr => "src-constr",
            SourceParent::ConstrSet => "src-constr-set",
            SourceParent::Sim => "src-sim",
            SourceParent::SimSet => "src-sim-set",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SourceParent::Design => "Design Sources",
            SourceParent::Constr => "Constraints",
            SourceParent::ConstrSet => "constrs_1",
            SourceParent::Sim => "Simulation Sources",
            SourceParent::SimSet => "sim_1",
        }
    }

    /// Fileset nodes (folder icon) vs category roots (twisty only).
    pub fn is_fileset(self) -> bool {
        matches!(self, SourceParent::ConstrSet | SourceParent::SimSet)
    }
}

/// Bottom tabs of the Sources pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourcesTab {
    #[default]
    Hierarchy,
    Libraries,
    CompileOrder,
}

impl SourcesTab {
    pub const ALL: [SourcesTab; 3] = [
        SourcesTab::Hierarchy,
        SourcesTab::Libraries,
        SourcesTab::CompileOrder,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SourcesTab::Hierarchy => "Hierarchy",
            SourcesTab::Libraries => "Libraries",
            SourcesTab::CompileOrder => "Compile Order",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SourcesTab::Hierarchy => "sources-tab-hierarchy",
            SourcesTab::Libraries => "sources-tab-libraries",
            SourcesTab::CompileOrder => "sources-tab-compile-order",
        }
    }
}

/// Bottom tabs of the Source File Properties card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PropsTab {
    #[default]
    General,
    Properties,
}

impl PropsTab {
    pub const ALL: [PropsTab; 2] = [PropsTab::General, PropsTab::Properties];

    pub fn label(self) -> &'static str {
        match self {
            PropsTab::General => "General",
            PropsTab::Properties => "Properties",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            PropsTab::General => "props-tab-general",
            PropsTab::Properties => "props-tab-properties",
        }
    }
}

/// Sources-owned UI state: collapsed parents, selection, tabs, filter.
#[derive(Clone, Debug)]
pub struct SourcesState {
    pub collapsed: [bool; 5],
    pub selected: SourceFileId,
    pub tab: SourcesTab,
    pub props_tab: PropsTab,
    /// Name filter field is showing.
    pub filter_open: bool,
    /// Case-insensitive name filter over every tab.
    pub filter: TextEdit,
}

impl Default for SourcesState {
    fn default() -> Self {
        // The brief shows sim_1 collapsed (twisty, no visible children).
        let mut collapsed = [false; 5];
        collapsed[SourceParent::SimSet.index()] = true;
        Self {
            collapsed,
            selected: SourceFileId::Demo,
            tab: SourcesTab::Hierarchy,
            props_tab: PropsTab::General,
            filter_open: false,
            filter: TextEdit::new(""),
        }
    }
}

impl SourcesState {
    pub fn toggle_parent(&mut self, parent: SourceParent) {
        let i = parent.index();
        self.collapsed[i] = !self.collapsed[i];
    }

    pub fn set_all(&mut self, collapsed: bool) {
        self.collapsed = [collapsed; 5];
    }

    /// Whether a file name passes the filter.
    fn shows(&self, f: &ProjectFile) -> bool {
        let q = self.filter.text.trim();
        q.is_empty()
            || f.name.to_lowercase().contains(&q.to_lowercase())
            || f
                .module
                .as_deref()
                .is_some_and(|m| m.to_lowercase().contains(&q.to_lowercase()))
    }
}

/// A Properties-tab row (`NAME`/`VALUE`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropRow {
    pub name: &'static str,
    pub value: String,
}

/// Size text: bytes under 1 KB, else one decimal in KB/MB.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `NAME`/`VALUE` rows for the Properties tab of the selected file.
pub fn property_rows(s: &AppState, id: SourceFileId) -> Vec<PropRow> {
    let Some(f) = id.resolve(s).and_then(|i| s.project_files.files.get(i)) else {
        return Vec::new();
    };
    let is_top = f.set == FileSet::Design
        && f.module.is_some()
        && f.module.as_deref() == s.project.top.as_deref();
    let mut rows = vec![
        PropRow {
            name: "NAME",
            value: f.name.clone(),
        },
        PropRow {
            name: "FILE_TYPE",
            value: f.lang().into(),
        },
        PropRow {
            name: "FILESET",
            value: f.set.label().into(),
        },
        PropRow {
            name: "LIBRARY",
            value: LIBRARY.into(),
        },
        PropRow {
            name: "SIZE",
            value: format_size(f.size),
        },
        PropRow {
            name: "LOCATION",
            value: f.location(),
        },
    ];
    if let Some(m) = &f.module {
        rows.push(PropRow {
            name: "MODULE",
            value: m.clone(),
        });
    }
    rows.push(PropRow {
        name: "IS_TOP",
        value: if is_top { "true" } else { "false" }.into(),
    });
    rows
}

/// Select a source file and bring its editor tab forward (no navigation).
pub fn select_source(s: &mut AppState, id: SourceFileId) {
    s.sources.selected = id;
    if let Some(i) = id.resolve(s) {
        crate::project::open_index(s, i);
    }
}

/// Open a source file in the editor: select + navigate to the Editor pane.
pub fn open_source(s: &mut AppState, id: SourceFileId) {
    select_source(s, id);
    s.navigate(crate::state::CenterPane::Editor);
}

/// Locate the file declaring `module` (schematic/device "Locate in
/// Sources"): select it and reveal the Sources column. Returns false when no
/// project file declares that module.
pub fn locate_module(s: &mut AppState, module: &str) -> bool {
    let Some(i) = s.project_files.defining(module) else {
        return false;
    };
    s.sources.selected = SourceFileId::for_index(s, i);
    s.sources.tab = SourcesTab::Hierarchy;
    s.sources.set_all(false);
    crate::layout::set_sources_visible(s, true);
    true
}

/// "Locate in Sources" from the schematic/device: select the file that
/// declares the elaborated design's top module (else the project top), and
/// say so plainly when no project file declares it.
pub fn locate_design_top(s: &mut AppState) {
    let module = s
        .runs
        .live_design
        .as_ref()
        .map(|d| helion_ir::resolve(d.name).to_string())
        .or_else(|| s.project.top.clone());
    let Some(module) = module else {
        s.toasts.push(
            crate::state::ToastKind::Info,
            "Nothing to locate",
            "Run synthesis to elaborate a design first",
        );
        return;
    };
    if !locate_module(s, &module) {
        s.toasts.push(
            crate::state::ToastKind::Info,
            "Not in project sources",
            &format!("No project file declares module {module}"),
        );
    }
}

/// Step the selection to the previous/next file in compile order.
fn step_selection(s: &mut AppState, delta: isize) {
    let n = s.project_files.files.len();
    if n == 0 {
        return;
    }
    let cur = s.sources.selected.resolve(s).unwrap_or(0) as isize;
    let next = (cur + delta).rem_euclid(n as isize) as usize;
    s.sources.selected = SourceFileId::for_index(s, next);
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// One pane-header window control (small square glyph button with a
/// tooltip). Every control changes the layout; none only prints a verb.
fn header_ctl(
    key: String,
    label: &str,
    help: &'static str,
    glyph: &'static str,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label.to_string())
        .help(help)
        .focusable(true)
        .size(20.0, 18.0)
        .radius(3.0)
        .center()
        .hover(t.selection)
        .on_press(on)
        .child(dim_label(glyph, t))
}

/// Pane header: title plus working window controls.
///
/// - Sources / Source File Properties: collapse the card to its header, and
///   hide the whole Sources column (View ▸ Sources or ⌘J brings it back).
/// - Center panes: maximize/restore (hides every dock) and close (back to
///   the previous surface, else the Project Summary).
pub fn pane_header(title: &'static str, key_prefix: &'static str, t: &VivadoTheme) -> El {
    let header = row()
        .w_full()
        .h(HEADER_H)
        .px(8.0)
        .items_center()
        .justify_between()
        .fill(t.header_bg)
        .child(
            text(title)
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .truncate()
                .help_when_truncated(title),
        );
    let mut right = row().items_center().gap(2.0);
    match key_prefix {
        "src-pane" | "props-pane" => {
            let tree = key_prefix == "src-pane";
            right = right
                .child(header_ctl(
                    format!("{key_prefix}-min"),
                    &format!("{title}: collapse"),
                    "Collapse or expand this panel",
                    "–",
                    t,
                    move |s: &mut AppState| {
                        if tree {
                            s.layout.sources_tree_collapsed = !s.layout.sources_tree_collapsed;
                        } else {
                            s.layout.props_collapsed = !s.layout.props_collapsed;
                        }
                    },
                ))
                .child(header_ctl(
                    format!("{key_prefix}-close"),
                    &format!("{title}: hide"),
                    "Hide the Sources column (View ▸ Sources, ⌘J)",
                    "×",
                    t,
                    |s: &mut AppState| crate::layout::set_sources_visible(s, false),
                ));
        }
        _ => {
            // Center panes: the shell's view tab strip is their header
            // (label, close, maximize), so no second title row is drawn.
            return el().key(format!("{key_prefix}-title-slot")).w_full().h(0.0).a11y_hidden();
        }
    }
    header.child(right)
}

/// Small square toolbar button with a tooltip.
fn tool_btn(
    key: &'static str,
    label: &'static str,
    t: &VivadoTheme,
    glyph: El,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .help(label)
        .focusable(true)
        .size(22.0, 22.0)
        .radius(3.0)
        .center()
        .hover(t.selection)
        .on_press(on)
        .child(glyph)
}

/// Sources toolbar: filter, expand/collapse all, add sources, file count.
fn sources_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let n = s.project_files.files.len();
    row()
        .key("sources-toolbar")
        .role("toolbar")
        .w_full()
        .h(26.0)
        .items_center()
        .gap(2.0)
        .px(6.0)
        .fill(t.panel_bg)
        .child(tool_btn(
            "src-search",
            "Filter sources by name",
            t,
            icon(&glyphs::SEARCH, 13.0)
                .color(if s.sources.filter_open { t.accent } else { t.text_dim })
                .a11y_hidden(),
            |s: &mut AppState| {
                s.sources.filter_open = !s.sources.filter_open;
                if !s.sources.filter_open {
                    s.sources.filter.clear();
                }
            },
        ))
        .child(tool_btn(
            "src-expand-all",
            "Expand all",
            t,
            icon(&glyphs::CHEVRON_DOWN, 13.0)
                .color(t.text_dim)
                .a11y_hidden(),
            |s: &mut AppState| s.sources.set_all(false),
        ))
        .child(tool_btn(
            "src-collapse-all",
            "Collapse all",
            t,
            icon(&glyphs::CHEVRON_UP, 13.0)
                .color(t.text_dim)
                .a11y_hidden(),
            |s: &mut AppState| s.sources.set_all(true),
        ))
        .child(tool_btn(
            "src-add",
            "Add sources…",
            t,
            text("+")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text_dim)
                .a11y_hidden(),
            |s: &mut AppState| s.dialogs.open(crate::views::dialogs::DialogId::AddSources),
        ))
        .child(el().grow(1.0))
        .child(
            text(format!("{n} file{}", if n == 1 { "" } else { "s" }))
                .key("src-count")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_faint),
        )
}

/// Filter field under the toolbar (shown while the search toggle is on).
fn filter_row(s: &AppState, t: &VivadoTheme) -> El {
    row()
        .key("src-filter-row")
        .w_full()
        .h(28.0)
        .items_center()
        .px(6.0)
        .pb(4.0)
        .fill(t.panel_bg)
        .child(
            text_input(&s.sources.filter, VivadoTheme::FONT_UI)
                .single_line()
                .key("src-filter")
                .label("Filter sources")
                .w_full()
                .px(6.0)
                .py(3.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(4.0)
                .color(t.text)
                .placeholder("Filter by file or module name", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.sources.filter),
        )
}

/// One tree parent row: twisty, folder icon for filesets, `name (n)` label.
fn parent_row(
    s: &AppState,
    t: &VivadoTheme,
    parent: SourceParent,
    depth: u32,
    children: usize,
) -> El {
    let collapsed = s.sources.collapsed[parent.index()];
    let mut r = row()
        .key(parent.key())
        .role("treeitem")
        .label(format!("{} ({} files)", parent.name(), children))
        .level(depth + 1)
        .expanded(!collapsed)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(4.0)
        .pl(6.0 + depth as f32 * INDENT)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.sources.toggle_parent(parent))
        .child(
            icon(
                if collapsed {
                    &glyphs::TRIANGLE_RIGHT
                } else {
                    &glyphs::TRIANGLE_DOWN
                },
                9.0,
            )
            .color(t.text_dim)
            .a11y_hidden(),
        );
    if parent.is_fileset() {
        r = r.child(
            icon(
                if collapsed {
                    &glyphs::FOLDER
                } else {
                    &glyphs::FOLDER_OPEN
                },
                13.0,
            )
            .color(t.text_dim)
            .a11y_hidden(),
        );
    }
    r.child(ui_label(parent.name(), t)).child(
        text(format!("({children})"))
            .font(VivadoTheme::FONT_UI)
            .color(t.text_faint),
    )
}

/// One file row: glyph, `module (file)` label, a `top` tag on the design
/// top. Click selects; double click opens it in the editor.
fn file_row(s: &AppState, t: &VivadoTheme, i: usize, depth: u32, key_prefix: &str) -> El {
    let f = &s.project_files.files[i];
    let id = SourceFileId::for_index(s, i);
    let selected = s.sources.selected.resolve(s) == Some(i);
    let label = f.tree_label();
    let is_top = f.set == FileSet::Design
        && f.module.is_some()
        && f.module.as_deref() == s.project.top.as_deref();
    let glyph = if f.set == FileSet::Constraints {
        &glyphs::FILE
    } else {
        &glyphs::FILE_CODE
    };
    let mut r = row()
        .key(format!("{key_prefix}{}", id.key()))
        .role("treeitem")
        .label(label.clone())
        .level(depth + 1)
        .focus_selected(selected)
        .focusable(true)
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(4.0)
        .pl(6.0 + depth as f32 * INDENT + 13.0)
        .pr(6.0)
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| select_source(s, id))
        .on_double_press(move |s: &mut AppState| open_source(s, id))
        .child(icon(glyph, 13.0).color(t.text_dim).a11y_hidden())
        .child(
            text(label.clone())
                .font(if is_top {
                    VivadoTheme::FONT_TITLE
                } else {
                    VivadoTheme::FONT_UI
                })
                .color(t.text)
                .grow(1.0)
                .min_w(0.0)
                .truncate()
                .help_when_truncated(label),
        );
    if is_top {
        r = r.child(
            el().px(5.0)
                .h(14.0)
                .center()
                .radius(7.0)
                .border(1.0, t.accent)
                .child(
                    text("top")
                        .font(VivadoTheme::FONT_CAPS)
                        .color(t.accent),
                ),
        );
    }
    r
}

/// Files of a fileset that pass the filter.
fn visible(s: &AppState, set: FileSet) -> Vec<usize> {
    s.project_files
        .in_set(set)
        .filter(|(_, f)| s.sources.shows(f))
        .map(|(i, _)| i)
        .collect()
}

/// Hierarchy tree body: category roots, filesets, and files.
fn hierarchy_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("sources-tree")
        .role("tree")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let design = visible(s, FileSet::Design);
    let constr = visible(s, FileSet::Constraints);
    let sim = visible(s, FileSet::Simulation);
    body = body.child(parent_row(s, t, SourceParent::Design, 0, design.len()));
    if !s.sources.collapsed[SourceParent::Design.index()] {
        for &i in &design {
            body = body.child(file_row(s, t, i, 1, ""));
        }
    }
    body = body.child(parent_row(s, t, SourceParent::Constr, 0, 1));
    if !s.sources.collapsed[SourceParent::Constr.index()] {
        body = body.child(parent_row(s, t, SourceParent::ConstrSet, 1, constr.len()));
        if !s.sources.collapsed[SourceParent::ConstrSet.index()] {
            for &i in &constr {
                body = body.child(file_row(s, t, i, 2, ""));
            }
        }
    }
    body = body.child(parent_row(s, t, SourceParent::Sim, 0, 1));
    if !s.sources.collapsed[SourceParent::Sim.index()] {
        body = body.child(parent_row(s, t, SourceParent::SimSet, 1, sim.len()));
        if !s.sources.collapsed[SourceParent::SimSet.index()] {
            for &i in &sim {
                body = body.child(file_row(s, t, i, 2, ""));
            }
        }
    }
    if !s.sources.filter.text.trim().is_empty() && design.is_empty() && constr.is_empty() && sim.is_empty() {
        body = body.child(
            row()
                .key("src-filter-empty")
                .w_full()
                .h(ROW_H + 4.0)
                .px(8.0)
                .items_center()
                .child(dim_label("No files match the filter.", t)),
        );
    }
    body
}

/// Libraries tab: every HDL file compiles into the one `work` library.
fn libraries_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("sources-libraries")
        .role("tree")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let hdl: Vec<usize> = s
        .project_files
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.set != FileSet::Constraints && s.sources.shows(f))
        .map(|(i, _)| i)
        .collect();
    body = body.child(
        row()
            .key("src-lib-row")
            .role("treeitem")
            .label(format!("{LIBRARY} ({} files)", hdl.len()))
            .level(1)
            .expanded(true)
            .w_full()
            .h(ROW_H)
            .items_center()
            .gap(4.0)
            .pl(6.0)
            .child(
                icon(&glyphs::TRIANGLE_DOWN, 9.0)
                    .color(t.text_dim)
                    .a11y_hidden(),
            )
            .child(
                icon(&glyphs::FOLDER_OPEN, 13.0)
                    .color(t.text_dim)
                    .a11y_hidden(),
            )
            .child(ui_label(LIBRARY, t))
            .child(
                text(format!("({})", hdl.len()))
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint),
            ),
    );
    for i in hdl {
        body = body.child(file_row(s, t, i, 1, "src-lib-"));
    }
    body
}

/// Compile Order tab: design sources in the order the flow elaborates them.
fn compile_order_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("sources-compile-order")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let order: Vec<usize> = s
        .project_files
        .in_set(FileSet::Design)
        .chain(s.project_files.in_set(FileSet::Simulation))
        .filter(|(_, f)| s.sources.shows(f))
        .map(|(i, _)| i)
        .collect();
    for (n, i) in order.into_iter().enumerate() {
        let f = &s.project_files.files[i];
        let id = SourceFileId::for_index(s, i);
        let selected = s.sources.selected.resolve(s) == Some(i);
        body = body.child(
            row()
                .key(format!("src-order-{n}"))
                .role("button")
                .label(format!("{}: {}", n + 1, f.name))
                .focusable(true)
                .w_full()
                .h(ROW_H)
                .items_center()
                .gap(6.0)
                .px(6.0)
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| select_source(s, id))
                .on_double_press(move |s: &mut AppState| open_source(s, id))
                .child(
                    el().w(18.0)
                        .child(
                            text(format!("{}", n + 1))
                                .font(VivadoTheme::FONT_MONO.mono())
                                .color(t.text_faint),
                        )
                        .a11y_hidden(),
                )
                .child(
                    ui_label(&f.name, t)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(
                    text(f.set.label())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_faint),
                ),
        );
    }
    body
}

/// Bottom tab strip shared by both panes (Vivado underline-tab look).
fn tab_strip(
    tabs: &[(String, String, bool)],
    t: &VivadoTheme,
    on_pick: impl Fn(usize) -> Box<dyn Fn(&mut AppState)> + Clone + 'static,
) -> El {
    let mut strip = row()
        .w_full()
        .h(24.0)
        .items_center()
        .px(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    for (i, (key, label, active)) in tabs.iter().enumerate() {
        let pick = on_pick(i);
        strip = strip.child(
            col()
                .key(key.clone())
                .role("tab")
                .label(label.clone())
                .focus_selected(*active)
                .focusable(true)
                .h_full()
                .px(8.0)
                .justify_center()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| pick(s))
                .child(
                    text(label.clone())
                        .font(if *active {
                            VivadoTheme::FONT_TITLE
                        } else {
                            VivadoTheme::FONT_UI
                        })
                        .color(if *active { t.text } else { t.text_dim }),
                )
                .child(
                    el().w_full()
                        .h(2.0)
                        .fill(if *active { t.link } else { t.header_bg }),
                ),
        );
    }
    strip
}

/// The Sources pane: header, toolbar, optional filter, tabbed body, tabs.
pub fn sources_pane(s: &AppState, t: &VivadoTheme) -> El {
    if s.layout.sources_tree_collapsed {
        return col()
            .key("sources-pane")
            .w_full()
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .child(pane_header("Sources", "src-pane", t));
    }
    let body = match s.sources.tab {
        SourcesTab::Hierarchy => hierarchy_body(s, t),
        SourcesTab::Libraries => libraries_body(s, t),
        SourcesTab::CompileOrder => compile_order_body(s, t),
    };
    let tabs: Vec<(String, String, bool)> = SourcesTab::ALL
        .iter()
        .map(|tab| (tab.key().into(), tab.label().into(), *tab == s.sources.tab))
        .collect();
    let mut pane = col()
        .key("sources-pane")
        .w_full()
        .grow(1.2)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Sources", "src-pane", t))
        .child(sources_toolbar(s, t));
    if s.sources.filter_open {
        pane = pane.child(filter_row(s, t));
    }
    pane.child(body).child(tab_strip(&tabs, t, |i| {
        let tab = SourcesTab::ALL[i];
        Box::new(move |s: &mut AppState| s.sources.tab = tab)
    }))
}

/// One `label: value` row in the General tab.
fn prop_row(label: &'static str, value: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(22.0)
        .items_center()
        .gap(6.0)
        .child(el().w(64.0).child(dim_label(label, t)))
        .child(el().grow(1.0).min_w(0.0).child(value))
}

/// General tab: real location, type, fileset, library, size, module.
fn props_general(s: &AppState, t: &VivadoTheme, f: &ProjectFile) -> El {
    let location = f.location();
    let mut body = col()
        .key("props-general")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(2.0)
        .px(8.0)
        .py(6.0)
        .child(prop_row(
            "Location:",
            text(location.clone())
                .key("props-location")
                .font(VivadoTheme::FONT_UI)
                .color(if f.path.is_some() { t.text } else { t.text_dim })
                .truncate_middle()
                .help_when_truncated(location),
            t,
        ))
        .child(prop_row("Type:", ui_label(f.lang(), t).key("props-type"), t))
        .child(prop_row("Fileset:", ui_label(f.set.label(), t), t))
        .child(prop_row("Library:", ui_label(LIBRARY, t), t))
        .child(prop_row(
            "Size:",
            ui_label(&format_size(f.size), t).key("props-size"),
            t,
        ));
    if let Some(m) = &f.module {
        let is_top = f.set == FileSet::Design && Some(m.as_str()) == s.project.top.as_deref();
        body = body.child(prop_row(
            "Module:",
            ui_label(
                &if is_top {
                    format!("{m}  (top)")
                } else {
                    m.clone()
                },
                t,
            )
            .key("props-module"),
            t,
        ));
    }
    body
}

/// Properties tab: `NAME`/`VALUE` table.
fn props_table(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("props-table")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .px(8.0)
        .py(6.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    body = body.child(
        row()
            .w_full()
            .h(20.0)
            .items_center()
            .fill(t.header_bg)
            .child(el().w(96.0).px(6.0).child(dim_label("Name", t)))
            .child(dim_label("Value", t)),
    );
    for row_data in property_rows(s, s.sources.selected) {
        let name = row_data.name;
        body = body.child(
            row()
                .key(format!("props-row-{name}"))
                .w_full()
                .h(20.0)
                .items_center()
                .child(
                    el().w(96.0).px(6.0).child(
                        text(name)
                            .font(VivadoTheme::FONT_MONO.mono())
                            .color(t.text_dim),
                    ),
                )
                .child(
                    text(row_data.value.clone())
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate_middle()
                        .help_when_truncated(row_data.value),
                ),
        );
    }
    body
}

/// The Source File Properties card: header, file row, tabbed body, tabs.
pub fn props_card(s: &AppState, t: &VivadoTheme) -> El {
    if s.layout.props_collapsed {
        return col()
            .key("source-props")
            .w_full()
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .child(pane_header("Source File Properties", "props-pane", t));
    }
    let card = col()
        .key("source-props")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Source File Properties", "props-pane", t));
    let Some(f) = s
        .sources
        .selected
        .resolve(s)
        .and_then(|i| s.project_files.files.get(i))
    else {
        return card.child(
            col()
                .key("props-empty")
                .w_full()
                .grow(1.0)
                .center()
                .child(dim_label("Select a file to see its properties.", t)),
        );
    };
    let body = match s.sources.props_tab {
        PropsTab::General => props_general(s, t, f),
        PropsTab::Properties => props_table(s, t),
    };
    let tabs: Vec<(String, String, bool)> = PropsTab::ALL
        .iter()
        .map(|tab| {
            (
                tab.key().into(),
                tab.label().into(),
                *tab == s.sources.props_tab,
            )
        })
        .collect();
    let many = s.project_files.files.len() > 1;
    let step = |key: &'static str, label: &'static str, glyph: &'static str, delta: isize| {
        let mut b = el()
            .key(key)
            .role("button")
            .label(label)
            .help(label)
            .focusable(many)
            .disabled(!many)
            .size(22.0, 22.0)
            .radius(3.0)
            .center()
            .child(dim_label(glyph, t));
        if many {
            b = b
                .hover(t.selection)
                .on_press(move |s: &mut AppState| step_selection(s, delta));
        } else {
            b = b.layer_opacity(0.4);
        }
        b
    };
    let name = f.name.clone();
    card.child(
        row()
            .w_full()
            .h(24.0)
            .items_center()
            .gap(4.0)
            .px(8.0)
            .child(
                icon(
                    if f.set == FileSet::Constraints {
                        &glyphs::FILE
                    } else {
                        &glyphs::FILE_CODE
                    },
                    13.0,
                )
                .color(t.text_dim)
                .a11y_hidden(),
            )
            .child(
                text(name.clone())
                    .key("props-file")
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate()
                    .help_when_truncated(name),
            )
            .child(step("props-prev", "Previous file", "‹", -1))
            .child(step("props-next", "Next file", "›", 1)),
    )
    .child(body)
    .child(tab_strip(&tabs, t, |i| {
        let tab = PropsTab::ALL[i];
        Box::new(move |s: &mut AppState| s.sources.props_tab = tab)
    }))
}

/// Empty-project Sources pane: header over a "no project" placeholder with
/// the one action that fixes it.
fn empty_sources_pane(t: &VivadoTheme) -> El {
    col()
        .key("sources-pane")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Sources", "src-pane", t))
        .child(
            col()
                .key("sources-empty")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .items_center()
                .justify_center()
                .gap(8.0)
                .padding(12.0)
                .child(dim_label("No project open", t))
                .child(
                    el().key("sources-empty-open")
                        .role("button")
                        .label("Open Project…")
                        .focusable(true)
                        .h(24.0)
                        .px(10.0)
                        .center()
                        .radius(4.0)
                        .border(1.0, t.border)
                        .hover(t.selection)
                        .on_press_tx(|_s: &mut AppState, tx: &mut rune::Tx| {
                            crate::pickers::open_project(tx)
                        })
                        .child(text("Open Project…").font(VivadoTheme::FONT_UI).color(t.link)),
                ),
        )
}

/// The stacked Sources + Properties column wired into the shell.
pub fn sources_column(s: &AppState, t: &VivadoTheme) -> El {
    let width = s.layout.sources_w;
    if s.project.name.is_empty() {
        return col()
            .key("sources-column")
            .w(width)
            .h_full()
            .gap(6.0)
            .child(empty_sources_pane(t));
    }
    col()
        .key("sources-column")
        .w(width)
        .h_full()
        .gap(6.0)
        .child(sources_pane(s, t))
        .child(props_card(s, t))
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

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn sources_tree_exposes_pane_tree_and_props() {
        let mut rt = harness();
        rt.frame(0.0);
        for key in [
            "sources-column",
            "sources-pane",
            "sources-toolbar",
            "src-design",
            "src-design-top",
            "src-constr",
            "src-constr-set",
            "src-xdc",
            "src-sim",
            "src-sim-set",
            "sources-tab-hierarchy",
            "source-props",
            "props-file",
            "props-location",
            "props-size",
            "props-tab-general",
        ] {
            assert!(rt.find(key).is_some(), "missing sources key: {key}");
        }
        assert!(
            rt.find("src-tb").is_none(),
            "sim_1 starts collapsed like the brief"
        );
    }

    #[test]
    fn collapse_hides_children_and_expand_restores() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "src-constr");
        assert!(rt.state.sources.collapsed[SourceParent::Constr.index()]);
        assert!(
            rt.find("src-xdc").is_none(),
            "collapsed children leave the tree"
        );
        tap(&mut rt, "src-constr");
        assert!(rt.find("src-xdc").is_some(), "children return on reopen");
    }

    #[test]
    fn collapse_all_hides_everything_and_expand_all_restores() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "src-collapse-all");
        assert!(rt.find("src-design-top").is_none());
        assert!(rt.find("src-xdc").is_none());
        tap(&mut rt, "src-expand-all");
        assert!(rt.find("src-design-top").is_some());
        assert!(
            rt.find("src-tb").is_some(),
            "even sim_1 opens on expand-all"
        );
    }

    #[test]
    fn selecting_xdc_updates_selection() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "src-xdc");
        assert_eq!(rt.state.sources.selected, SourceFileId::Xdc);
        assert!(
            rt.find("props-size").is_some(),
            "props card follows the selection"
        );
    }

    #[test]
    fn empty_project_shows_placeholder_not_populated_sources() {
        let mut s = init();
        s.empty_project();
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(
            rt.find("sources-column").is_some(),
            "column keeps its chrome slot"
        );
        assert!(rt.find("sources-empty").is_some(), "placeholder must mount");
        for key in ["src-design-top", "src-xdc", "source-props", "props-file"] {
            assert!(rt.find(key).is_none(), "populated source {key} must leave");
        }
    }

    #[test]
    fn prev_next_step_through_project_files() {
        let mut rt = harness();
        rt.frame(0.0);
        assert_eq!(rt.state.sources.selected, SourceFileId::Demo);
        tap(&mut rt, "props-next");
        assert_eq!(rt.state.sources.selected, SourceFileId::Xdc);
        tap(&mut rt, "props-next");
        assert_eq!(rt.state.sources.selected, SourceFileId::Tb);
        tap(&mut rt, "props-next");
        assert_eq!(rt.state.sources.selected, SourceFileId::Demo, "wraps");
        tap(&mut rt, "props-prev");
        assert_eq!(rt.state.sources.selected, SourceFileId::Tb);
    }

    #[test]
    fn filter_narrows_the_tree_and_reports_no_match() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "src-search");
        assert!(rt.find("src-filter").is_some());
        rt.state.sources.filter = TextEdit::new("xdc");
        rt.invalidate();
        rt.frame(0.1);
        assert!(rt.find("src-xdc").is_some());
        assert!(rt.find("src-design-top").is_none());
        rt.state.sources.filter = TextEdit::new("zzz");
        rt.invalidate();
        rt.frame(0.2);
        assert!(rt.find("src-filter-empty").is_some());
        tap(&mut rt, "src-search");
        assert!(rt.state.sources.filter.text.is_empty(), "closing clears");
        assert!(rt.find("src-design-top").is_some());
    }

    #[test]
    fn demo_files_are_marked_in_memory_not_given_fake_paths() {
        let s = init();
        let rows = property_rows(&s, SourceFileId::Demo);
        let loc = rows.iter().find(|r| r.name == "LOCATION").unwrap();
        assert!(loc.value.starts_with("In memory"), "{}", loc.value);
        assert!(!loc.value.contains("C:/"));
        let top = rows.iter().find(|r| r.name == "IS_TOP").unwrap();
        assert_eq!(top.value, "false", "demo project declares no top yet");
    }

    #[test]
    fn sources_tabs_switch_bodies() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "sources-tab-libraries");
        assert_eq!(rt.state.sources.tab, SourcesTab::Libraries);
        assert!(rt.find("src-lib-row").is_some());
        tap(&mut rt, "sources-tab-compile-order");
        assert_eq!(rt.state.sources.tab, SourcesTab::CompileOrder);
        assert!(rt.find("src-order-0").is_some());
        tap(&mut rt, "sources-tab-compile-order");
        tap(&mut rt, "sources-tab-hierarchy");
        assert!(rt.find("src-design-top").is_some(), "hierarchy returns");
    }

    #[test]
    fn props_tabs_switch_general_and_table() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "props-tab-properties");
        assert_eq!(rt.state.sources.props_tab, PropsTab::Properties);
        assert!(rt.find("props-row-NAME").is_some());
        assert!(rt.find("props-row-LOCATION").is_some());
        tap(&mut rt, "props-tab-general");
        assert!(rt.find("props-location").is_some(), "general returns");
    }

    #[test]
    fn format_size_matches_vivado() {
        assert_eq!(format_size(1126), "1.1 KB");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn src_add_opens_the_add_sources_dialog() {
        use crate::views::dialogs::DialogId;
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "src-add");
        assert_eq!(rt.state.dialogs.open, Some(DialogId::AddSources));
        assert!(
            rt.find("add-sources-dialog").is_some(),
            "Add Sources dialog must mount"
        );
    }
}
