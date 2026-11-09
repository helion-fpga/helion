//! Sources tree + Source File Properties card.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-f58667ba*.png`): the "Sources" pane with its Design
//! Sources / Constraints / Simulation Sources tree and Hierarchy / Libraries /
//! Compile Order tabs, stacked over the "Source File Properties" card (Enabled
//! checkbox, Location / Type / Library / Size rows, General / Properties
//! tabs). All colors come from [`VivadoTheme`]; rows use framework layout only
//! (no manual geometry, no hardcoded hex, no stringly selection).
//!
//! Data shapes mirror `helion-gui` read-only: [`PropRow`] mirrors
//! `ide.rs::PropertyRow` (`NAME`/`VALUE` rows for the Properties tab); file
//! extensions follow `open_dialog::HDL_EXTENSIONS` (`.v`/`.xdc` demo files).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()`.
//! - `crates/rune/src/el/text.rs`: `.truncate_middle()` for long paths.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_RIGHT/DOWN`, `FOLDER(_OPEN)`,
//!   `FILE(_CODE)`, `SEARCH`, `CHEVRON_UP/DOWN`.
//! - `crates/rune-kit/src/checkbox.rs`: `Checkbox::new().label().on_toggle()`.

use rune::{El, ScrollSpec, col, el, icon, row, text};
use rune_kit::Checkbox;
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;

/// Width of the Sources + Properties column in the shell.
pub const SOURCES_W: f32 = 300.0;
/// Tree indent per depth level.
const INDENT: f32 = 14.0;
/// Tree/file row height (compact CAD density).
const ROW_H: f32 = 20.0;
/// Pane header height.
const HEADER_H: f32 = 24.0;

/// Demo file kind, shown in the Type box. Typed, never a bare string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Verilog,
    Xdc,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Verilog => "Verilog",
            FileKind::Xdc => "XDC",
        }
    }
}

/// One demo source file. Static data; per-file UI state lives in
/// [`SourcesState::enabled`] indexed by [`SourceFileId`].
#[derive(Clone, Copy, Debug)]
pub struct SourceFile {
    pub name: &'static str,
    /// Design-top module name (`None` for plain files).
    pub module: Option<&'static str>,
    pub path: &'static str,
    pub kind: FileKind,
    pub library: &'static str,
    pub size_bytes: u64,
}

/// Selectable files. Typed so selection is never a string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceFileId {
    #[default]
    Demo,
    Xdc,
    Tb,
}

impl SourceFileId {
    pub const ALL: [SourceFileId; 3] = [SourceFileId::Demo, SourceFileId::Xdc, SourceFileId::Tb];

    pub fn index(self) -> usize {
        match self {
            SourceFileId::Demo => 0,
            SourceFileId::Xdc => 1,
            SourceFileId::Tb => 2,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SourceFileId::Demo => "src-design-top",
            SourceFileId::Xdc => "src-xdc",
            SourceFileId::Tb => "src-tb",
        }
    }

    pub fn file(self) -> SourceFile {
        match self {
            SourceFileId::Demo => SourceFile {
                name: "project1_demo.v",
                module: Some("project1_demo"),
                path: "C:/EE214/project_1/project_1.srcs/sources_1/new/project1_demo.v",
                kind: FileKind::Verilog,
                library: "xil_defaultlib",
                size_bytes: 1126,
            },
            SourceFileId::Xdc => SourceFile {
                name: "project1.xdc",
                module: None,
                path: "C:/EE214/project_1/project_1.srcs/constrs_1/new/project1.xdc",
                kind: FileKind::Xdc,
                library: "xil_defaultlib",
                size_bytes: 512,
            },
            SourceFileId::Tb => SourceFile {
                name: "project1_tb.v",
                module: Some("project1_tb"),
                path: "C:/EE214/project_1/project_1.srcs/sim_1/new/project1_tb.v",
                kind: FileKind::Verilog,
                library: "xil_defaultlib",
                size_bytes: 640,
            },
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

/// Sources-owned UI state: collapsed parents, file selection, both tab strips,
/// and the per-file Enabled flags.
#[derive(Clone, Debug)]
pub struct SourcesState {
    pub collapsed: [bool; 5],
    pub selected: SourceFileId,
    pub tab: SourcesTab,
    pub props_tab: PropsTab,
    pub enabled: [bool; 3],
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
            enabled: [true; 3],
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

    pub fn toggle_enabled(&mut self, id: SourceFileId) {
        let i = id.index();
        self.enabled[i] = !self.enabled[i];
    }
}

/// A Properties-tab row, mirroring `helion-gui` `ide::PropertyRow`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropRow {
    pub name: &'static str,
    pub value: String,
}

/// Vivado-style size text: one decimal in KB (`1126 B` -> `"1.1 KB"`).
pub fn format_size(bytes: u64) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

/// `NAME`/`VALUE` rows for the Properties tab of one file.
pub fn property_rows(id: SourceFileId) -> Vec<PropRow> {
    let f = id.file();
    vec![
        PropRow {
            name: "NAME",
            value: f.name.into(),
        },
        PropRow {
            name: "LIBRARY",
            value: f.library.into(),
        },
        PropRow {
            name: "FILE_TYPE",
            value: f.kind.label().into(),
        },
        PropRow {
            name: "SIZE",
            value: format_size(f.size_bytes),
        },
        PropRow {
            name: "LOCATION",
            value: f.path.into(),
        },
    ]
}

/// Echo a Tcl verb to the console (chrome-style dispatch without run flips).
fn echo(s: &mut AppState, tcl: &'static str) {
    s.console.lines.push(ConsoleLine {
        text: tcl.into(),
        sev: MsgSev::Info,
    });
}

/// Select a source file and ensure its editor tab exists (active, no navigation).
pub fn select_source(s: &mut AppState, id: SourceFileId) {
    s.sources.selected = id;
    let name = id.file().name.to_string();
    if let Some(i) = s.editor.tabs.iter().position(|t| *t == name) {
        s.editor.active = i;
    } else {
        s.editor.tabs.push(name);
        s.editor.active = s.editor.tabs.len() - 1;
    }
    s.editor.cursor_line = s
        .editor
        .cursor_line
        .min(s.editor.lines.len().saturating_sub(1));
    crate::views::editor::sync_edit(s);
}

/// Open a source file in the editor: select + navigate to the Editor pane.
pub fn open_source(s: &mut AppState, id: SourceFileId) {
    select_source(s, id);
    s.navigate(crate::state::CenterPane::Editor);
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

/// Pane header: title plus Vivado window controls (help/min/max/float/close).
pub fn pane_header(title: &'static str, key_prefix: &'static str, t: &VivadoTheme) -> El {
    let controls = [
        ("?", "help"),
        ("_", "min"),
        ("□", "max"),
        ("⤢", "float"),
        ("×", "close"),
    ];
    let header = row()
        .w_full()
        .h(HEADER_H)
        .px(8.0)
        .items_center()
        .justify_between()
        .fill(t.header_bg)
        .child(text(title).font(VivadoTheme::FONT_TITLE).color(t.text));
    let mut right = row().items_center();
    for (_i, (glyph, what)) in controls.into_iter().enumerate() {
        let tcl: &'static str = match what {
            "help" => "help",
            "min" => "minimize_view",
            "max" => "maximize_view",
            "float" => "float_view",
            _ => "close_view",
        };
        let key = format!("{key_prefix}-{what}");
        right = right.child(
            el().key(key)
                .role("button")
                .label(format!("{title}: {what}"))
                .focusable(true)
                .size(20.0, 18.0)
                .center()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| echo(s, tcl))
                .child(dim_label(glyph, t)),
        );
    }
    header.child(right)
}

/// Small toolbar icon button echoing a Tcl verb.
fn tool_btn(key: String, label: String, tcl: &'static str, t: &VivadoTheme, glyph: El) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .size(22.0, 22.0)
        .center()
        .hover(t.selection)
        .on_press(move |s: &mut AppState| echo(s, tcl))
        .child(glyph)
}

/// Sources toolbar: search, expand/collapse all, add, warnings badge, options.
fn sources_toolbar(t: &VivadoTheme) -> El {
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
            "src-search".into(),
            "Search sources".into(),
            "search_sources",
            t,
            icon(&glyphs::SEARCH, 13.0).color(t.text_dim).a11y_hidden(),
        ))
        .child(
            el().key("src-expand-all")
                .role("button")
                .label("Expand all")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| s.sources.set_all(false))
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 13.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
        .child(
            el().key("src-collapse-all")
                .role("button")
                .label("Collapse all")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| s.sources.set_all(true))
                .child(
                    icon(&glyphs::CHEVRON_UP, 13.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
        .child(
            el().key("src-add")
                .role("button")
                .label("Add sources")
                .focusable(true)
                .size(22.0, 22.0)
                .center()
                .hover(t.selection)
                .on_press(|s: &mut AppState| {
                    echo(s, "add_sources");
                    s.dialogs.open(crate::views::dialogs::DialogId::AddSources);
                })
                .child(
                    text("+")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
        .child(tool_btn(
            "src-help".into(),
            "Sources help".into(),
            "help sources",
            t,
            text("?")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .a11y_hidden(),
        ))
        .child(
            row()
                .key("src-warnings")
                .role("status")
                .label("2 warnings")
                .h(18.0)
                .items_center()
                .gap(3.0)
                .px(5.0)
                .radius(9.0)
                .fill(t.panel_bg)
                .border(1.0, t.warn)
                .child(text("2").font(VivadoTheme::FONT_UI).color(t.warn)),
        )
        .child(el().grow(1.0))
        .child(tool_btn(
            "src-options".into(),
            "View options".into(),
            "show_view_options Sources",
            t,
            text("⚙")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .a11y_hidden(),
        ))
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
    r.child(ui_label(&format!("{} ({})", parent.name(), children), t))
}

/// One file row: file glyph, module + file label, selection wash on select.
fn file_row(
    s: &AppState,
    t: &VivadoTheme,
    id: SourceFileId,
    depth: u32,
    key_override: Option<String>,
) -> El {
    let f = id.file();
    let selected = s.sources.selected == id;
    let label = match f.module {
        Some(m) => format!("{m} ({})", f.name),
        None => f.name.into(),
    };
    let glyph = match f.kind {
        FileKind::Verilog => &glyphs::FILE_CODE,
        FileKind::Xdc => &glyphs::FILE,
    };
    row()
        .key(key_override.unwrap_or_else(|| id.key().into()))
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
        .fill(if selected { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| select_source(s, id))
        .on_double_press(move |s: &mut AppState| open_source(s, id))
        .child(icon(glyph, 13.0).color(t.text_dim).a11y_hidden())
        .child(ui_label(&label, t))
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
    body = body.child(parent_row(s, t, SourceParent::Design, 0, 1));
    if !s.sources.collapsed[SourceParent::Design.index()] {
        body = body.child(file_row(s, t, SourceFileId::Demo, 1, None));
    }
    body = body.child(parent_row(s, t, SourceParent::Constr, 0, 1));
    if !s.sources.collapsed[SourceParent::Constr.index()] {
        body = body.child(parent_row(s, t, SourceParent::ConstrSet, 1, 1));
        if !s.sources.collapsed[SourceParent::ConstrSet.index()] {
            body = body.child(file_row(s, t, SourceFileId::Xdc, 2, None));
        }
    }
    body = body.child(parent_row(s, t, SourceParent::Sim, 0, 1));
    if !s.sources.collapsed[SourceParent::Sim.index()] {
        body = body.child(parent_row(s, t, SourceParent::SimSet, 1, 1));
        if !s.sources.collapsed[SourceParent::SimSet.index()] {
            body = body.child(file_row(s, t, SourceFileId::Tb, 2, None));
        }
    }
    body
}

/// Libraries tab: the one demo library with its files.
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
    body = body.child(
        row()
            .key("src-lib-row")
            .role("treeitem")
            .label("xil_defaultlib (3 files)")
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
            .child(ui_label("xil_defaultlib (3)", t)),
    );
    for id in SourceFileId::ALL {
        body = body.child(file_row(s, t, id, 1, Some(format!("src-lib-{}", id.key()))));
    }
    body
}

/// Compile Order tab: files in compile sequence with index badges.
fn compile_order_body(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("sources-compile-order")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for (i, id) in SourceFileId::ALL.into_iter().enumerate() {
        let f = id.file();
        let selected = s.sources.selected == id;
        body = body.child(
            row()
                .key(format!("src-order-{i}"))
                .role("button")
                .label(format!("{}: {}", i + 1, f.name))
                .focusable(true)
                .w_full()
                .h(ROW_H)
                .items_center()
                .gap(6.0)
                .pl(6.0)
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| select_source(s, id))
                .on_double_press(move |s: &mut AppState| open_source(s, id))
                .child(
                    el().w(16.0)
                        .h(16.0)
                        .center()
                        .radius(8.0)
                        .fill(t.header_bg)
                        .border(1.0, t.border)
                        .child(dim_label(&format!("{}", i + 1), t))
                        .a11y_hidden(),
                )
                .child(ui_label(f.name, t)),
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

/// The Sources pane: header, toolbar, tabbed tree body, tab strip.
pub fn sources_pane(s: &AppState, t: &VivadoTheme) -> El {
    let body = match s.sources.tab {
        SourcesTab::Hierarchy => hierarchy_body(s, t),
        SourcesTab::Libraries => libraries_body(s, t),
        SourcesTab::CompileOrder => compile_order_body(s, t),
    };
    let tabs: Vec<(String, String, bool)> = SourcesTab::ALL
        .iter()
        .map(|tab| (tab.key().into(), tab.label().into(), *tab == s.sources.tab))
        .collect();
    col()
        .key("sources-pane")
        .w_full()
        .grow(1.2)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Sources", "src-pane", t))
        .child(sources_toolbar(t))
        .child(body)
        .child(tab_strip(&tabs, t, |i| {
            let tab = SourcesTab::ALL[i];
            Box::new(move |s: &mut AppState| s.sources.tab = tab)
        }))
}

/// General tab: Enabled checkbox plus Location / Type / Library / Size rows.
fn props_general(s: &AppState, t: &VivadoTheme) -> El {
    let id = s.sources.selected;
    let f = id.file();
    let kit = t.kit();
    let mut body = col()
        .key("props-general")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(4.0)
        .padding(8.0);
    body = body.child(
        Checkbox::new(s.sources.enabled[id.index()])
            .label("Enabled")
            .on_toggle::<AppState>(move |s: &mut AppState| s.sources.toggle_enabled(id))
            .build(&kit)
            .key("props-enabled"),
    );
    body = body.child(prop_row(
        "Location:",
        text(f.path)
            .font(VivadoTheme::FONT_UI)
            .color(t.text)
            .grow(1.0)
            .shrink()
            .truncate_middle(),
        t,
    ));
    body = body.child(prop_row(
        "Type:",
        row()
            .gap(4.0)
            .items_center()
            .child(value_box(f.kind.label(), t))
            .child(more_btn(
                "props-type-more",
                "Change file type",
                "set_property file_type",
                t,
            )),
        t,
    ));
    body = body.child(prop_row(
        "Library:",
        row()
            .gap(4.0)
            .items_center()
            .child(value_box(f.library, t))
            .child(more_btn(
                "props-lib-more",
                "Change library",
                "set_property library",
                t,
            )),
        t,
    ));
    body = body.child(prop_row(
        "Size:",
        ui_label(&format_size(f.size_bytes), t).key("props-size"),
        t,
    ));
    body
}

/// One `label: value` row in the General tab.
fn prop_row(label: &'static str, value: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(22.0)
        .items_center()
        .gap(6.0)
        .child(el().w(64.0).child(dim_label(label, t)))
        .child(value)
}

/// Vivado value box (Type / Library): bordered field with the value.
fn value_box(value: &str, t: &VivadoTheme) -> El {
    el().h(20.0)
        .px(6.0)
        .center_y()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(ui_label(value, t))
}

/// The "..." picker button beside Type / Library.
fn more_btn(key: &'static str, label: &'static str, tcl: &'static str, t: &VivadoTheme) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .w(24.0)
        .h(20.0)
        .center()
        .border(1.0, t.border)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| echo(s, tcl))
        .child(dim_label("...", t))
}

/// Properties tab: `NAME`/`VALUE` table mirroring `ide::PropertyRow` rows.
fn props_table(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("props-table")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(8.0)
        .gap(2.0);
    body = body.child(
        row()
            .w_full()
            .h(20.0)
            .items_center()
            .fill(t.header_bg)
            .child(el().w(96.0).px(6.0).child(dim_label("Name", t)))
            .child(dim_label("Value", t)),
    );
    for row_data in property_rows(s.sources.selected) {
        let name = row_data.name;
        body = body.child(
            row()
                .key(format!("props-row-{name}"))
                .w_full()
                .h(20.0)
                .items_center()
                .child(el().w(96.0).px(6.0).child(dim_label(name, t)))
                .child(
                    text(row_data.value)
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text)
                        .grow(1.0)
                        .shrink()
                        .truncate_middle(),
                ),
        );
    }
    body
}

/// The Source File Properties card: header, file row, tabbed body, tab strip.
pub fn props_card(s: &AppState, t: &VivadoTheme) -> El {
    let id = s.sources.selected;
    let f = id.file();
    let body = match s.sources.props_tab {
        PropsTab::General => props_general(s, t),
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
    col()
        .key("source-props")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Source File Properties", "props-pane", t))
        .child(
            row()
                .w_full()
                .h(24.0)
                .items_center()
                .gap(4.0)
                .px(8.0)
                .child(
                    icon(&glyphs::FILE_CODE, 13.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                )
                .child(
                    ui_label(f.name, t)
                        .key("props-file")
                        .grow(1.0)
                        .shrink()
                        .truncate(),
                )
                .child(tool_btn(
                    "props-prev".into(),
                    "Previous file".into(),
                    "prev_file",
                    t,
                    dim_label("<", t),
                ))
                .child(tool_btn(
                    "props-next".into(),
                    "Next file".into(),
                    "next_file",
                    t,
                    dim_label(">", t),
                )),
        )
        .child(body)
        .child(tab_strip(&tabs, t, |i| {
            let tab = PropsTab::ALL[i];
            Box::new(move |s: &mut AppState| s.sources.props_tab = tab)
        }))
}

/// Empty-project Sources pane: header over a "no project" placeholder. Mounts
/// when no project is open (the start page) so the column keeps its chrome
/// slot without showing populated demo sources.
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
                .gap(4.0)
                .padding(12.0)
                .child(dim_label("No project open", t))
                .child(dim_label("Create or open a project to see sources.", t)),
        )
}

/// The stacked Sources + Properties column wired into the shell.
pub fn sources_column(s: &AppState, t: &VivadoTheme) -> El {
    if s.project.name.is_empty() {
        return col()
            .key("sources-column")
            .w(SOURCES_W)
            .h_full()
            .gap(6.0)
            .child(empty_sources_pane(t));
    }
    col()
        .key("sources-column")
        .w(SOURCES_W)
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
            "props-enabled",
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
    fn enabled_toggle_flips_per_file() {
        let mut rt = harness();
        rt.frame(0.0);
        tap(&mut rt, "props-enabled");
        assert!(!rt.state.sources.enabled[SourceFileId::Demo.index()]);
        tap(&mut rt, "src-xdc");
        assert!(
            rt.state.sources.enabled[SourceFileId::Xdc.index()],
            "other files keep their own flag"
        );
        tap(&mut rt, "props-enabled");
        assert!(!rt.state.sources.enabled[SourceFileId::Xdc.index()]);
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
        assert!(rt.find("props-enabled").is_some(), "general returns");
    }

    #[test]
    fn format_size_matches_vivado() {
        assert_eq!(format_size(1126), "1.1 KB");
        assert_eq!(format_size(512), "0.5 KB");
        assert_eq!(format_size(0), "0.0 KB");
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
