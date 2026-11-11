//! IP Catalog + Block Design (IP Integrator) views: catalog table with
//! detail, the block-design canvas, address map and generated-HDL tabs, and
//! the toolbar (Create / Add / Customize / Remove / Run Connection
//! Automation / Assign Addresses / Generate / Package IP).
//!
//! The model and actions live in [`crate::views::ip_model`]; everything it
//! shows is the real `helion_bd::BdSystem` and its `emit_system` output, so
//! the canvas, address map and HDL table always agree.

use rune::{El, ScrollSpec, col, el, row};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
pub use crate::views::ip_canvas::{
    BdDrawing, BdPin, BdSymbol, BdWire, SymKind, bd_canvas, drawing,
};
pub use crate::views::ip_model::*;
use crate::views::sources::pane_header;

/// Catalog band height (header + ~8 rows + filter).
const CATALOG_H: f32 = 214.0;
/// IP detail column width.
const DETAIL_W: f32 = 264.0;
/// Detail `label` width.
const LABEL_W: f32 = 72.0;

const CATALOG_COLS: [ColSpec; 4] = [
    ColSpec::grow("Name", 1.0),
    ColSpec::grow("VLNV", 1.8),
    ColSpec::fixed("Bus", 72.0),
    ColSpec::fixed("Status", 74.0),
];
const ADDR_COLS: [ColSpec; 5] = [
    ColSpec::grow("Cell", 1.2),
    ColSpec::grow("Interface", 0.8),
    ColSpec::fixed("Offset", 110.0),
    ColSpec::num("Range", 60.0),
    ColSpec::fixed("High", 110.0),
];
const HDL_COLS: [ColSpec; 4] = [
    ColSpec::grow("Instance", 1.2),
    ColSpec::grow("Module", 1.0),
    ColSpec::fixed("Bus", 80.0),
    ColSpec::grow("Parameters", 1.4),
];

/// Tcl `bd_drawing` text (canvas symbols and wires).
pub fn drawing_text(s: &AppState) -> String {
    let Some(bd) = &s.ip.bd else {
        return "drawing symbols=0 wires=0 (create_bd first)".into();
    };
    let d = drawing(&s.ip);
    let mut out = format!(
        "drawing symbols={} wires={} nets={} canvas={}x{} bd={} ok={}",
        d.symbols.len(),
        d.wires.len(),
        d.nets,
        d.w as i32,
        d.h as i32,
        bd.name(),
        u8::from(bd.ok())
    );
    for sy in &d.symbols {
        out.push_str(&format!(
            "\n{} kind={:?} box={},{},{},{} bus={}",
            sy.name, sy.kind, sy.x as i32, sy.y as i32, sy.w as i32, sy.h as i32, sy.bus
        ));
        for p in &sy.pins {
            out.push_str(&format!(
                " {}.{}:{}{}",
                sy.name,
                p.name,
                if p.output { "out" } else { "in" },
                if p.iface { ":intf" } else { "" }
            ));
        }
    }
    for w in &d.wires {
        out.push_str(&format!(
            "\nwire {} {}->{} pts={}",
            w.net,
            w.src,
            w.dst,
            w.pts.len()
        ));
    }
    out
}

fn addr_table(s: &AppState, t: &VivadoTheme) -> El {
    let Some(bd) = &s.ip.bd else {
        return cad::empty_state("ip-addr-empty", "No block design.", None, t);
    };
    let rows = bd.sys.address_map();
    let unmapped: Vec<&str> = bd
        .sys
        .cells
        .iter()
        .filter(|c| c.role() == Some(helion_ipxact::MmRole::Slave) && !c.connected)
        .map(|c| c.inst.as_str())
        .collect();
    if rows.is_empty() {
        let msg = if unmapped.is_empty() {
            "No Helion-MM slaves yet. Add a Helion-MM core to assign an address.".to_string()
        } else {
            format!(
                "{} unconnected slave(s). Run Connection Automation to map them.",
                unmapped.len()
            )
        };
        return cad::empty_state("ip-addr-empty", &msg, None, t);
    }
    let master = bd
        .emit
        .as_ref()
        .ok()
        .and_then(|e| e.interconnect.as_ref())
        .map(|i| i.master.clone().unwrap_or_else(|| "external s_mm".into()))
        .unwrap_or_default();
    let mut list = col().w_full();
    for (i, r) in rows.iter().enumerate() {
        let selected = s.ip.selected_cell.as_deref() == Some(r.inst.as_str());
        let inst = r.inst.clone();
        let inst2 = r.inst.clone();
        list = list.child(
            cad::table_row(format!("ip-addr-row-{i}"), r.inst.clone(), selected, t)
                .on_press(move |s: &mut AppState| {
                    let _ = select_cell(s, &inst);
                })
                .on_double_press(move |s: &mut AppState| {
                    let _ = open_customize(s, &inst2);
                })
                .child(cad::cell(
                    &ADDR_COLS[0],
                    cad::cut(r.inst.clone(), cad::mono_style(), t.text),
                ))
                .child(cad::cell(
                    &ADDR_COLS[1],
                    cad::cut("s_mm", cad::mono_style(), t.text_dim),
                ))
                .child(cad::cell(&ADDR_COLS[2], cad::mono(cad::addr_hex(r.base), t)))
                .child(cad::cell(&ADDR_COLS[3], cad::mono(cad::range_label(r.range), t)))
                .child(cad::cell(
                    &ADDR_COLS[4],
                    cad::mono(cad::addr_hex(r.base + r.range - 1), t),
                )),
        );
    }
    let mut out = col()
        .w_full()
        .gap(4.0)
        .child(cad::dim(format!("Address space of {master}"), t).key("ip-addr-master"))
        .child(
            cad::table_frame("ip-addr", t)
                .child(cad::header("ip-addr-head", &ADDR_COLS, t))
                .child(list),
        );
    if !unmapped.is_empty() {
        out = out.child(cad::dim(
            format!("Unmapped (unconnected): {}", unmapped.join(", ")),
            t,
        ));
    }
    out
}

fn hdl_table(s: &AppState, t: &VivadoTheme) -> El {
    let rows = hdl_rows(s);
    if rows.is_empty() {
        let msg = match s.ip.bd.as_ref().map(|b| &b.emit) {
            Some(Err(e)) if !e.is_empty() => format!("HDL not emitted: {e}"),
            _ => "No instances yet. Add a core to emit its SystemVerilog instance.".into(),
        };
        return cad::empty_state("ip-hdl-empty", &msg, None, t);
    }
    let mut list = col().w_full();
    for (i, r) in rows.iter().enumerate() {
        let selected = s.ip.selected_cell.as_deref() == Some(r.instance.as_str());
        let inst = r.instance.clone();
        list = list.child(
            cad::table_row(format!("ip-hdl-row-{i}"), r.instance.clone(), selected, t)
                .on_press(move |s: &mut AppState| {
                    let _ = select_cell(s, &inst);
                })
                .child(cad::cell(
                    &HDL_COLS[0],
                    cad::cut(r.instance.clone(), cad::mono_style(), t.text),
                ))
                .child(cad::cell(
                    &HDL_COLS[1],
                    cad::cut(r.module.clone(), cad::mono_style(), t.text),
                ))
                .child(cad::cell(
                    &HDL_COLS[2],
                    cad::cut(r.bus.clone(), VivadoTheme::FONT_UI, t.text_dim),
                ))
                .child(cad::cell(
                    &HDL_COLS[3],
                    cad::cut(
                        if r.params.is_empty() { "defaults".to_string() } else { r.params.clone() },
                        cad::mono_style(),
                        t.text_dim,
                    ),
                )),
        );
    }
    let bd = s.ip.bd.as_ref().expect("rows imply a design");
    let lines = bd.sv().lines().count();
    let ports = bd.emit.as_ref().map(|e| e.ports.len()).unwrap_or(0);
    col()
        .w_full()
        .gap(4.0)
        .child(
            cad::table_frame("ip-hdl", t)
                .child(cad::header("ip-hdl-head", &HDL_COLS, t))
                .child(list),
        )
        .child(
            cad::dim(
                format!(
                    "{}.sv · {lines} lines · {ports} top-level ports · emitted by helion-bd",
                    bd.name()
                ),
                t,
            )
            .key("ip-hdl-footer"),
        )
}
fn catalog_table(s: &AppState, t: &VivadoTheme) -> El {
    let rows = s.ip.filtered();
    let mut list = col().w_full();
    for r in &rows {
        let selected = s.ip.selected.as_deref() == Some(r.name.as_str());
        let name = r.name.clone();
        let status_ink = match r.status {
            "In BD" => t.pass,
            "Generated" => t.link,
            "Generator" => t.text_faint,
            _ => t.text_dim,
        };
        let status = if r.user && r.status == "Available" { "User" } else { r.status };
        list = list.child(
            cad::table_row(
                format!("ip-row-{}", r.name),
                format!("{} {}", r.name, r.vlnv),
                selected,
                t,
            )
            .on_press(move |s: &mut AppState| {
                let _ = select_ip(s, &name);
            })
            .child(cad::cell(
                &CATALOG_COLS[0],
                cad::cut(r.name.clone(), cad::mono_style(), t.text),
            ))
            .child(cad::cell(
                &CATALOG_COLS[1],
                cad::cut(r.vlnv.clone(), cad::mono_style(), t.text_dim),
            ))
            .child(cad::cell(
                &CATALOG_COLS[2],
                cad::cut(r.bus.clone(), VivadoTheme::FONT_UI, t.text),
            ))
            .child(cad::cell(
                &CATALOG_COLS[3],
                cad::cut(status, VivadoTheme::FONT_UI, status_ink),
            )),
        );
    }
    if rows.is_empty() {
        list = list.child(
            row()
                .key("ip-catalog-nomatch")
                .h(cad::ROW_H)
                .px(8.0)
                .items_center()
                .child(cad::dim("No IP matches the filter.", t)),
        );
    }
    col()
        .key("ip-catalog")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .gap(6.0)
        .child(
            row()
                .w_full()
                .h(24.0)
                .items_center()
                .gap(8.0)
                .child(cad::title("IP Catalog", t))
                .child(
                    cad::dim(format!("{} of {} cores", rows.len(), s.ip.catalog.len()), t)
                        .key("ip-catalog-count"),
                )
                .child(el().grow(1.0))
                .child(
                    cad::search_input(
                        "ip-filter",
                        &s.ip.filter,
                        "Filter IP",
                        t,
                        |s: &mut AppState| &mut s.ip.filter,
                    )
                    .w(180.0),
                ),
        )
        .child(
            cad::table_frame("ip-catalog-table", t)
                .grow(1.0)
                .min_h(0.0)
                .child(cad::header("ip-catalog-head", &CATALOG_COLS, t))
                .child(
                    col()
                        .key("ip-catalog-scroll")
                        .w_full()
                        .grow(1.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(list),
                ),
        )
}

fn core_detail(s: &AppState, name: &str, t: &VivadoTheme) -> El {
    let Some(c) = s.ip.core(name) else {
        return cad::dim("Select a core to see its VLNV and outputs.", t);
    };
    let output = match s.ip.generated.get(&c.name) {
        Some(g) => cad::cut(g.path.display().to_string(), VivadoTheme::FONT_UI, t.text)
            .truncate_middle(),
        None => cad::dim("Not generated", t),
    };
    let source = if s.ip.user.contains(&c.name) {
        "User repository"
    } else if s.ip.packaged.contains(&c.name) {
        "Bundled RTL package"
    } else {
        "Generator only (no RTL)"
    };
    col()
        .key("ip-detail-body")
        .w_full()
        .child(cad::title(c.name.clone(), t).key("ip-detail-name"))
        .child(cad::kv("VLNV", cad::cut(c.vlnv(), cad::mono_style(), t.text), LABEL_W, t))
        .child(cad::kv("Bus", cad::ui(c.bus.clone(), t), LABEL_W, t))
        .child(cad::kv("Source", cad::cut(source, VivadoTheme::FONT_UI, t.text), LABEL_W, t))
        .child(cad::kv("Status", cad::ui(s.ip.status(&c.name), t), LABEL_W, t))
        .child(cad::kv("Output", output.key("ip-detail-output"), LABEL_W, t))
}

fn cell_detail(s: &AppState, inst: &str, t: &VivadoTheme) -> El {
    let Some(c) = s.ip.cell(inst) else {
        return cad::dim("Cell no longer exists.", t);
    };
    let role = match c.role() {
        Some(helion_ipxact::MmRole::Master) => "Helion-MM master",
        Some(helion_ipxact::MmRole::Slave) => "Helion-MM slave",
        None => "Helion-ST / no bus",
    };
    let mut body = col()
        .key("ip-detail-body")
        .w_full()
        .child(cad::title(c.inst.clone(), t).key("ip-detail-name"))
        .child(cad::kv(
            "IP",
            cad::cut(c.model.core.vlnv(), cad::mono_style(), t.text),
            LABEL_W,
            t,
        ))
        .child(cad::kv("Role", cad::ui(role, t), LABEL_W, t))
        .child(cad::kv(
            "Connected",
            cad::ui(if c.connected { "Yes" } else { "No (pins exported)" }, t),
            LABEL_W,
            t,
        ));
    if let Some(b) = c.base {
        body = body.child(cad::kv("Base", cad::mono(cad::addr_hex(b), t), LABEL_W, t));
    }
    if c.model.params.is_empty() {
        body = body.child(cad::dim("No configurable parameters.", t));
    } else {
        body = body.child(el().h(4.0)).child(cad::caps("Parameters", t));
        for p in &c.model.params {
            let v = c.params.get(&p.name).cloned().unwrap_or_else(|| p.default.clone());
            let changed = c.params.get(&p.name).is_some_and(|x| {
                p.validate(x).ok() != p.validate(&p.default).ok()
            });
            body = body.child(
                row()
                    .key(format!("ip-detail-param-{}", p.name))
                    .w_full()
                    .h(cad::ROW_H)
                    .items_center()
                    .gap(6.0)
                    .child(
                        el().grow(1.0)
                            .min_w(0.0)
                            .child(cad::cut(p.name.clone(), cad::mono_style(), t.text_dim)),
                    )
                    .child(cad::cut(v, cad::mono_style(), if changed { t.link } else { t.text })),
            );
        }
    }
    body
}

fn detail_panel(s: &AppState, t: &VivadoTheme) -> El {
    let body = match (&s.ip.selected_cell, &s.ip.selected) {
        (Some(inst), _) if s.ip.cell(inst).is_some() => cell_detail(s, inst, t),
        (_, Some(name)) => core_detail(s, name, t),
        _ => col()
            .w_full()
            .child(cad::dim("Select a core or block-design cell.", t)),
    };
    col()
        .key("ip-detail")
        .w(DETAIL_W)
        .h_full()
        .gap(6.0)
        .child(row().h(24.0).items_center().child(cad::title("Properties", t)))
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(8.0)
                .radius(4.0)
                .fill(t.panel_bg)
                .border(1.0, t.border)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(body),
        )
}

fn bd_section(s: &AppState, t: &VivadoTheme) -> El {
    let Some(bd) = &s.ip.bd else {
        return col()
            .key("ip-bd")
            .w_full()
            .grow(1.0)
            .child(cad::empty_state(
                "ip-bd-empty",
                "No block design yet. Create one to place Helion-MM and Helion-ST cores.",
                Some(cad::primary_btn(
                    "ip-bd-empty-create",
                    "Create Block Design",
                    t,
                    None,
                    |s: &mut AppState| {
                        let _ = create_block_design(s);
                    },
                )),
                t,
            ));
    };
    let d = drawing(&s.ip);
    let unconnected = bd.sys.cells.iter().filter(|c| !c.connected).count();
    let summary = format!(
        "{} · {} cells · {} nets{}",
        bd.name(),
        bd.sys.cells.len(),
        d.nets,
        if unconnected > 0 { format!(" · {unconnected} unconnected") } else { String::new() }
    );
    let errors = bd.errors();
    let warnings = bd.warnings();
    let (badge_text, badge_ink) = if !errors.is_empty() {
        (format!("{} errors", errors.len()), t.fail)
    } else if !warnings.is_empty() {
        (
            format!("{} warning{}", warnings.len(), if warnings.len() == 1 { "" } else { "s" }),
            t.warn,
        )
    } else {
        ("Valid".to_string(), t.pass)
    };
    let mut tabs = row()
        .key("ip-bd-tabs")
        .role("tablist")
        .w_full()
        .h(26.0)
        .items_end()
        .gap(2.0)
        .px(4.0)
        .fill(t.header_bg);
    for (tab, key, label) in [
        (BdTab::Diagram, "ip-tab-diagram", "Diagram"),
        (BdTab::AddressMap, "ip-tab-address", "Address Map"),
        (BdTab::Hdl, "ip-tab-hdl", "Generated HDL"),
    ] {
        tabs = tabs.child(cad::tab(key, label, s.ip.tab == tab, t, move |s: &mut AppState| {
            s.ip.tab = tab
        }));
    }
    let scroll_body = |inner: El| {
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(8.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(inner)
    };
    let body = match s.ip.tab {
        BdTab::Diagram if bd.sys.cells.is_empty() => cad::empty_state(
            "ip-bd-canvas-empty",
            "Empty block design. Select a packaged core above and Add to Block Design.",
            None,
            t,
        ),
        BdTab::Diagram => bd_canvas(s, t),
        BdTab::AddressMap => scroll_body(addr_table(s, t)),
        BdTab::Hdl => scroll_body(hdl_table(s, t)),
    };
    let first = errors.first().or(warnings.first()).cloned().unwrap_or_default();
    let all = errors.iter().chain(warnings.iter()).cloned().collect::<Vec<_>>().join("\n");
    let head = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::title("Block Design", t))
        .child(
            el().min_w(0.0)
                .child(cad::cut(summary, VivadoTheme::FONT_UI, t.text_dim).key("ip-bd-summary")),
        )
        .child(cad::badge(badge_text, badge_ink, t).key("ip-bd-valid"))
        .child(el().grow(1.0))
        .child(bd_actions(s, t));
    let diag = (!first.is_empty()).then(|| {
        cad::cut(first, VivadoTheme::FONT_UI, if errors.is_empty() { t.text_dim } else { t.fail })
            .key("ip-bd-diag")
            .help(all)
    });
    let mut out = col()
        .key("ip-bd")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(6.0)
        .child(head)
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(1.0)
                .radius(4.0)
                .border(1.0, t.border)
                .child(tabs)
                .child(body),
        );
    if let Some(d) = diag {
        out = out.child(d);
    }
    out
}

fn bd_actions(s: &AppState, t: &VivadoTheme) -> El {
    let cell_reason = match &s.ip.selected_cell {
        Some(c) if s.ip.cell(c).is_some() => None,
        _ => Some("Select a block-design cell (canvas or tables) first.".to_string()),
    };
    let auto_reason = automation_block_reason(s);
    let assign_reason = match &s.ip.bd {
        None => Some("Create a block design first.".to_string()),
        Some(b) if !b.sys.cells.iter().any(|c| c.role() == Some(helion_ipxact::MmRole::Slave) && c.base.is_none()) => {
            Some("Every Helion-MM slave already has an address.".to_string())
        }
        _ => None,
    };
    let gen_bd_reason = generate_bd_reason(s);
    row()
        .key("ip-bd-actions")
        .items_center()
        .gap(4.0)
        .child(cad::btn(
            "ip-customize",
            "Customize…",
            t,
            cell_reason.as_deref(),
            |s: &mut AppState| {
                if let Some(c) = s.ip.selected_cell.clone() {
                    let r = open_customize(s, &c).map(|_| format!("customize {c}"));
                    if r.is_err() {
                        console_result(s, r);
                    }
                }
            },
        ))
        .child(cad::btn(
            "ip-remove-cell",
            "Remove",
            t,
            cell_reason.as_deref(),
            |s: &mut AppState| {
                let r = delete_bd_cell(s, "");
                console_result(s, r);
            },
        ))
        .child(cad::tool_sep(t))
        .child(cad::btn(
            "ip-automation",
            "Connection Automation…",
            t,
            auto_reason.as_deref(),
            |s: &mut AppState| s.dialogs.open(crate::views::dialogs::DialogId::IpAutomation),
        ))
        .child(cad::btn(
            "ip-assign-address",
            "Assign Addresses",
            t,
            assign_reason.as_deref(),
            |s: &mut AppState| crate::tcl::eval_command(s, "assign_bd_address"),
        ))
        .child(cad::tool_sep(t))
        .child(cad::btn(
            "ip-generate-bd",
            "Generate BD",
            t,
            gen_bd_reason.as_deref(),
            |s: &mut AppState| crate::tcl::eval_command(s, "generate_bd"),
        ))
}

fn console_result(s: &mut AppState, r: Result<String, String>) {
    let (text, sev) = match r {
        Ok(v) => (v, crate::state::MsgSev::Info),
        Err(e) => (format!("ERROR: {e}"), crate::state::MsgSev::Error),
    };
    s.console.lines.push(crate::state::ConsoleLine { text, sev });
}

/// The center IP Integrator pane wired into the shell.
pub fn ip_pane(s: &AppState, t: &VivadoTheme) -> El {
    let create_reason = s
        .ip
        .bd
        .as_ref()
        .map(|b| format!("Block design {} is already open.", b.name()));
    let add_reason = add_block_reason(s);
    let gen_reason = generate_block_reason(s);
    let pkg_reason = if s
        .project_files
        .files
        .iter()
        .any(|f| f.set == crate::project::FileSet::Design && f.path.is_some())
    {
        None
    } else {
        Some("Open a project with design sources on disk to package one as IP.".to_string())
    };
    let tools = cad::toolbar("ip-toolbar", t)
        .child(cad::btn("ip-refresh", "Refresh", t, None, |s: &mut AppState| {
            let n = {
                refresh_catalog(s);
                s.ip.catalog.len()
            };
            console_result(s, Ok(format!("ip_catalog n={n}")));
        }))
        .child(cad::btn(
            "ip-create-bd",
            "Create Block Design",
            t,
            create_reason.as_deref(),
            |s: &mut AppState| crate::tcl::eval_command(s, "create_bd"),
        ))
        .child(cad::tool_sep(t))
        .child(cad::btn(
            "ip-add-to-bd",
            "Add to Block Design",
            t,
            add_reason.as_deref(),
            |s: &mut AppState| crate::tcl::eval_command(s, "create_bd_cell"),
        ))
        .child(cad::tool_sep(t))
        .child(cad::btn(
            "ip-generate",
            "Generate IP-XACT",
            t,
            gen_reason.as_deref(),
            |s: &mut AppState| crate::tcl::eval_command(s, "generate_ip"),
        ))
        .child(cad::btn(
            "ip-package",
            "Package IP…",
            t,
            pkg_reason.as_deref(),
            |s: &mut AppState| s.dialogs.open(crate::views::dialogs::DialogId::PackageIp),
        ))
        .child(cad::tool_sep(t))
        .child(cad::btn(
            "ip-export-hw",
            "Export Hardware…",
            t,
            crate::views::handoff::export_reason(s).as_deref(),
            crate::views::handoff::open_export,
        ));
    col()
        .key("ip")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("IP Integrator", "ip", t))
        .child(tools)
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(10.0)
                .padding(8.0)
                .child(
                    row()
                        .w_full()
                        .h(CATALOG_H)
                        .gap(10.0)
                        .child(catalog_table(s, t))
                        .child(detail_panel(s, t)),
                )
                .child(bd_section(s, t)),
        )
}

#[cfg(test)]
#[path = "ip_tests.rs"]
mod tests;
