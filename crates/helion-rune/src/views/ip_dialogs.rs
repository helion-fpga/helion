//! IP Integrator dialogs: Run Connection Automation (preview of the
//! `helion_bd::BdSystem::plan_automation` steps, then apply), Customize IP
//! (per-cell `CONFIG.*` parameters from the IP-XACT model with live
//! validation, instance name, base address) and Package IP (a project module
//! into the user IP repository). Also the small dialog building blocks the
//! other ecosystem dialogs reuse.

use rune::{El, ScrollSpec, TextEdit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, TextInput};

use crate::state::{AppState, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::dialogs::DialogId;
use crate::views::ip_model::{self as m};

/// Dialog field-label width.
pub(crate) const FIELD_LABEL_W: f32 = 120.0;

/// `label: control` row.
pub(crate) fn field(label: &str, control: El, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(26.0)
        .items_center()
        .gap(8.0)
        .child(
            el().w(FIELD_LABEL_W)
                .child(text(label.to_string()).font(VivadoTheme::FONT_UI).color(t.text_dim).truncate()),
        )
        .child(el().grow(1.0).min_w(0.0).child(control))
}

/// Kit single-line text input with a stable key.
pub(crate) fn edit_box(
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
        .w_full()
        .min_w(0.0)
}

/// Footer: status/error text, then the primary action (disabled with a
/// reason when `ok_reason` is set) and Cancel.
pub(crate) fn footer(
    key: &'static str,
    ok_label: &'static str,
    ok_reason: Option<&str>,
    status: Option<(&str, bool)>,
    t: &VivadoTheme,
    kit: &Kit,
    on_ok: impl Fn(&mut AppState) + 'static,
) -> El {
    let mut f = row()
        .key(format!("{key}-footer"))
        .w_full()
        .h(36.0)
        .items_center()
        .gap(8.0)
        .px(8.0);
    match status {
        Some((msg, is_err)) => {
            f = f.child(
                text(msg.to_string())
                    .font(VivadoTheme::FONT_UI)
                    .color(if is_err { t.fail } else { t.text_dim })
                    .truncate()
                    .help_when_truncated(msg.to_string())
                    .key(format!("{key}-status"))
                    .grow(1.0)
                    .min_w(0.0),
            )
        }
        None => f = f.child(el().grow(1.0)),
    }
    let mut ok = Button::new(ok_label)
        .variant(ButtonVariant::Submit)
        .disabled(ok_reason.is_some());
    if ok_reason.is_none() {
        ok = ok.on_press::<AppState>(on_ok);
    }
    let mut ok = ok.build(kit).key(format!("{key}-ok"));
    if let Some(why) = ok_reason {
        ok = ok.description(why.to_string()).help(why.to_string());
    }
    f.child(ok).child(
        Button::new("Cancel")
            .variant(ButtonVariant::Neutral)
            .on_press::<AppState>(|s: &mut AppState| s.dialogs.close())
            .build(kit)
            .key(format!("{key}-cancel")),
    )
}

pub(crate) fn hrule(t: &VivadoTheme) -> El {
    el().w_full().h(1.0).fill(t.border_light).a11y_hidden()
}

fn console(s: &mut AppState, r: &Result<String, String>) {
    let (text, sev) = match r {
        Ok(v) => (v.clone(), crate::state::MsgSev::Info),
        Err(e) => (format!("ERROR: {e}"), crate::state::MsgSev::Error),
    };
    s.console.lines.push(crate::state::ConsoleLine { text, sev });
}

const STEP_COLS: [ColSpec; 4] = [
    ColSpec::grow("Cell", 0.9),
    ColSpec::fixed("Kind", 64.0),
    ColSpec::grow("From", 1.1),
    ColSpec::grow("To", 1.1),
];

/// Run Connection Automation: every step the backend will take, then apply.
pub fn automation_dialog(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    let plan = s
        .ip
        .bd
        .as_ref()
        .map(|b| b.sys.plan_automation())
        .unwrap_or_default();
    let mut list = col().w_full();
    for (i, st) in plan.steps.iter().enumerate() {
        let ink = match st.kind {
            "bus" => t.link,
            "address" => t.text,
            _ => t.text_dim,
        };
        list = list.child(
            cad::table_row(format!("ip-auto-step-{i}"), format!("{} {}", st.from, st.to), false, t)
                .child(cad::cell(
                    &STEP_COLS[0],
                    cad::cut(
                        if st.cell.is_empty() { "(design)".to_string() } else { st.cell.clone() },
                        cad::mono_style(),
                        t.text,
                    ),
                ))
                .child(cad::cell(&STEP_COLS[1], cad::cut(st.kind, VivadoTheme::FONT_UI, ink)))
                .child(cad::cell(&STEP_COLS[2], cad::cut(st.from.clone(), cad::mono_style(), t.text)))
                .child(cad::cell(&STEP_COLS[3], cad::cut(st.to.clone(), cad::mono_style(), t.text))),
        );
    }
    let mut body = col()
        .key("ip-auto-body")
        .w_full()
        .gap(6.0)
        .padding(10.0)
        .child(cad::dim(
            format!(
                "Connection automation will wire {} cell(s): clock and reset, the Helion-MM \
                 interconnect with 4 KiB apertures, and every other pin as a top-level port.",
                plan.cells.len()
            ),
            t,
        )
        .wrap()
        .w_full())
        .child(
            cad::table_frame("ip-auto-table", t)
                .child(cad::header("ip-auto-head", &STEP_COLS, t))
                .child(
                    col()
                        .w_full()
                        .max_h(300.0)
                        .scroll(ScrollSpec { stick_bottom: false })
                        .child(list),
                ),
        );
    for (i, (cell, why)) in plan.skipped.iter().enumerate() {
        body = body.child(
            cad::cut(format!("Skipped {cell}: {why}"), VivadoTheme::FONT_UI, t.warn)
                .key(format!("ip-auto-skip-{i}")),
        );
    }
    let reason = if plan.cells.is_empty() {
        Some("Nothing left to connect.")
    } else {
        None
    };
    let panel = col()
        .w_full()
        .child(body)
        .child(hrule(t))
        .child(footer(
            "ip-auto",
            "OK",
            reason,
            Some((&format!("{} step(s)", plan.steps.len()), false)),
            t,
            kit,
            |s: &mut AppState| {
                let r = m::apply_automation(s);
                console(s, &r);
                match &r {
                    Ok(v) => s.toasts.push(ToastKind::Success, "Connection automation", v),
                    Err(e) => s.toasts.push(ToastKind::Error, "Connection automation failed", e),
                }
                s.dialogs.close();
            },
        ));
    Dialog::new("ip-auto", "Run Connection Automation")
        .open(true)
        .width(720.0)
        .fit()
        .on_close(|s: &mut AppState| s.dialogs.close())
        .child(panel)
        .build(kit, layer);
}

/// Customize IP: `CONFIG.*` parameters, instance name, base address.
pub fn customize_dialog(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    let Some(d) = &s.ip.customize else { return };
    let Some(cell) = s.ip.cell(&d.inst) else { return };
    let mut body = col()
        .key("ip-cust-body")
        .w_full()
        .gap(4.0)
        .padding(10.0)
        .child(
            cad::cut(cell.model.core.vlnv(), cad::mono_style(), t.text_dim).key("ip-cust-vlnv"),
        )
        .child(field(
            "Component name",
            edit_box("ip-cust-name", &d.name, "instance", kit, |s: &mut AppState| {
                match s.ip.customize.as_mut() {
                    Some(c) => &mut c.name,
                    None => &mut s.ip.scratch,
                }
            }),
            t,
        ));
    if cell.role() == Some(helion_ipxact::MmRole::Slave) {
        body = body.child(field(
            "Base address",
            edit_box("ip-cust-base", &d.base, "auto (0x4000_0000+)", kit, |s: &mut AppState| {
                match s.ip.customize.as_mut() {
                    Some(c) => &mut c.base,
                    None => &mut s.ip.scratch,
                }
            }),
            t,
        ));
    }
    body = body.child(el().h(4.0)).child(cad::caps("Parameters (IP-XACT)", t));
    if d.params.is_empty() {
        body = body.child(
            cad::dim(
                format!("{} declares no configurable parameters.", cell.model.core.name),
                t,
            )
            .key("ip-cust-noparams"),
        );
    }
    for (i, (name, edit)) in d.params.iter().enumerate() {
        let Some(p) = cell.model.param(name) else { continue };
        let invalid = p.validate(&edit.text).err();
        let input = edit_box(
            format!("ip-cust-param-{name}"),
            edit,
            "value",
            kit,
            move |s: &mut AppState| match s.ip.customize.as_mut().and_then(|c| c.params.get_mut(i)) {
                Some((_, e)) => e,
                None => &mut s.ip.scratch,
            },
        );
        body = body.child(
            row()
                .key(format!("ip-cust-row-{name}"))
                .w_full()
                .h(26.0)
                .items_center()
                .gap(8.0)
                .child(
                    el().w(FIELD_LABEL_W).child(
                        cad::cut(name.clone(), cad::mono_style(), t.text)
                            .help(p.description.clone()),
                    ),
                )
                .child(el().w(150.0).child(input))
                .child(
                    el().grow(1.0).min_w(0.0).child(
                        cad::cut(
                            invalid.clone().unwrap_or_else(|| {
                                format!("{} · {}", p.kind.label(), p.description)
                            }),
                            VivadoTheme::FONT_UI,
                            if invalid.is_some() { t.fail } else { t.text_dim },
                        ),
                    ),
                ),
        );
    }
    let err = m::customize_error(s).or_else(|| d.error.clone());
    let panel = col()
        .w_full()
        .child(
            col()
                .w_full()
                .max_h(420.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(body),
        )
        .child(hrule(t))
        .child(footer(
            "ip-cust",
            "OK",
            err.as_deref(),
            err.as_deref().map(|e| (e, true)),
            t,
            kit,
            |s: &mut AppState| {
                let r = m::apply_customize(s);
                console(s, &r);
            },
        ));
    Dialog::new("ip-cust", &format!("Customize IP — {}", d.inst))
        .open(true)
        .width(620.0)
        .fit()
        .on_close(|s: &mut AppState| {
            s.ip.customize = None;
            s.dialogs.close();
        })
        .child(panel)
        .build(kit, layer);
}

/// Package IP: pick a project design source, analyze its header (ports,
/// parameters, inferred bus interfaces), name the VLNV, and package it into
/// the user repository the catalog loads.
pub fn package_dialog(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    let pk = &s.ip.package;
    let mut sources = col().key("ip-pkg-sources").w_full();
    let design: Vec<(usize, &crate::project::ProjectFile)> = s
        .project_files
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.set == crate::project::FileSet::Design && f.path.is_some())
        .collect();
    for (i, f) in &design {
        let i = *i;
        let picked = pk.source == Some(i);
        sources = sources.child(
            cad::table_row(format!("ip-pkg-src-{i}"), f.name.clone(), picked, t)
                .on_press(move |s: &mut AppState| {
                    s.ip.package.source = Some(i);
                    s.ip.package.top = TextEdit::new("");
                    m::analyze_package_source(s);
                })
                .child(el().px(8.0).grow(1.0).min_w(0.0).child(
                    cad::cut(f.location(), VivadoTheme::FONT_UI, t.text).truncate_middle(),
                )),
        );
    }
    if design.is_empty() {
        sources = sources.child(cad::dim("No design sources on disk in this project.", t));
    }
    let preview = match &pk.preview {
        None => cad::dim("Pick a source to analyze its module header.", t),
        Some(Err(e)) => cad::cut(e.clone(), VivadoTheme::FONT_UI, t.fail),
        Some(Ok(module)) => {
            let buses = module.infer_buses();
            let bus_txt = if buses.is_empty() {
                "none inferred (no clk/resetn/mm_*/wr_*/rd_* naming)".to_string()
            } else {
                buses
                    .iter()
                    .map(|b| format!("{} ({} {})", b.name, b.bus, b.mode))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            col()
                .w_full()
                .child(cad::kv(
                    "Module",
                    cad::mono(format!("{} · {} ports · {} params", module.name, module.ports.len(), module.params.len()), t),
                    FIELD_LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Interfaces",
                    cad::cut(bus_txt, VivadoTheme::FONT_UI, t.text).key("ip-pkg-buses"),
                    FIELD_LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Catalog bus",
                    cad::ui(module.primary_bus().unwrap_or("None (cannot join a block design)"), t),
                    FIELD_LABEL_W,
                    t,
                ))
        }
    };
    let repo = m::user_repo_dir(s);
    let body = col()
        .key("ip-pkg-body")
        .w_full()
        .gap(4.0)
        .padding(10.0)
        .child(cad::caps("Source", t))
        .child(
            cad::table_frame("ip-pkg-src-frame", t).child(
                col()
                    .w_full()
                    .max_h(140.0)
                    .scroll(ScrollSpec { stick_bottom: false })
                    .child(sources),
            ),
        )
        .child(field(
            "Top module",
            edit_box("ip-pkg-top", &pk.top, "module name", kit, |s: &mut AppState| {
                &mut s.ip.package.top
            }),
            t,
        ))
        .child(field(
            "Vendor",
            edit_box("ip-pkg-vendor", &pk.vendor, "user", kit, |s: &mut AppState| {
                &mut s.ip.package.vendor
            }),
            t,
        ))
        .child(field(
            "Library",
            edit_box("ip-pkg-library", &pk.library, "ip", kit, |s: &mut AppState| {
                &mut s.ip.package.library
            }),
            t,
        ))
        .child(field(
            "Version",
            edit_box("ip-pkg-version", &pk.version, "1.0", kit, |s: &mut AppState| {
                &mut s.ip.package.version
            }),
            t,
        ))
        .child(el().h(4.0))
        .child(preview.key("ip-pkg-preview"))
        .child(
            cad::cut(
                format!("Repository: {}", repo.display()),
                VivadoTheme::FONT_UI,
                t.text_dim,
            )
            .truncate_middle()
            .key("ip-pkg-repo"),
        );
    let reason = match &pk.preview {
        Some(Ok(_)) if !pk.top.text.trim().is_empty() => None,
        Some(Err(_)) => Some("The selected source has no packageable module header."),
        _ => Some("Pick a source file to analyze first."),
    };
    let panel = col()
        .w_full()
        .child(body)
        .child(hrule(t))
        .child(footer(
            "ip-pkg",
            "Package IP",
            reason,
            pk.error.as_deref().map(|e| (e, true)).or(reason.map(|r| (r, false))),
            t,
            kit,
            |s: &mut AppState| {
                let r = m::package_ip(s);
                console(s, &r);
                match r {
                    Ok(v) => {
                        s.toasts.push(ToastKind::Success, "IP packaged", &v);
                        s.dialogs.close();
                    }
                    Err(e) => s.ip.package.error = Some(e),
                }
            },
        ));
    Dialog::new("ip-pkg", "Package IP")
        .open(true)
        .width(640.0)
        .fit()
        .on_close(|s: &mut AppState| s.dialogs.close())
        .child(panel)
        .build(kit, layer);
}

/// Dispatch for the IP dialog ids (called from `dialogs::push_dialogs`).
pub fn push(id: DialogId, layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    match id {
        DialogId::IpAutomation => automation_dialog(layer, s, t, kit),
        DialogId::IpCustomize => customize_dialog(layer, s, t, kit),
        DialogId::PackageIp => package_dialog(layer, s, t, kit),
        _ => {}
    }
}
