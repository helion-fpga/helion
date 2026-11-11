//! Configuration memory panel: Add Configuration Memory Device (part
//! table), `write_cfgmem` image options, and Program / Verify / Erase /
//! Blank Check / Boot with their step log. Model: [`crate::views::hw_cfgmem`].

use helion_hw::{CFGMEM_PARTS, CfgMemFormat, CfgMemInterface};
use rune::{El, col, el, row};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::hw_cfgmem::{self as cm, CfgOp};
use crate::views::hw_debug_view::{field, segment, segments};

const LABEL_W: f32 = 96.0;

fn act(s: &mut AppState, r: Result<String, String>) {
    if let Err(e) = r {
        s.console.lines.push(crate::state::ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: crate::state::MsgSev::Error,
        });
        s.program.cfgmem.result = Some(cm::CfgResult {
            op: CfgOp::Program,
            ok: false,
            report: None,
            line: e,
        });
    }
}

fn parts_table(s: &AppState, t: &VivadoTheme) -> El {
    const COLS: [ColSpec; 5] = [
        ColSpec::grow("Part", 1.0),
        ColSpec::fixed("Vendor", 72.0),
        ColSpec::fixed("Interface", 64.0),
        ColSpec::num("Density", 64.0),
        ColSpec::fixed("JEDEC ID", 72.0),
    ];
    let c = &s.program.cfgmem;
    let mut rows = col().w_full();
    for (i, p) in CFGMEM_PARTS.iter().enumerate() {
        rows = rows.child(
            cad::table_row(format!("hw-cfg-part-{i}"), p.name.to_string(), c.pick == i, t)
                .on_press(move |s: &mut AppState| s.program.cfgmem.pick = i)
                .child(cad::cell(&COLS[0], cad::cut(p.name, cad::mono_style(), t.text)))
                .child(cad::cell(&COLS[1], cad::dim(p.vendor, t)))
                .child(cad::cell(&COLS[2], cad::mono(p.interface.as_str(), t)))
                .child(cad::cell(&COLS[3], cad::mono(format!("{} Mb", p.density_mbit), t)))
                .child(cad::cell(&COLS[4], cad::mono(p.jedec_hex(), t))),
        );
    }
    cad::table_frame("hw-cfg-parts", t)
        .child(cad::header("hw-cfg-parts-head", &COLS, t))
        .child(rows)
}

fn add_card(s: &AppState, t: &VivadoTheme) -> El {
    let busy = s.program.cfgmem.busy();
    let why = busy.map(|o| format!("Busy: {}", o.busy_label()));
    let pick = CFGMEM_PARTS[s.program.cfgmem.pick.min(CFGMEM_PARTS.len() - 1)];
    cad::card(
        "hw-cfg-add",
        "Add Configuration Memory Device",
        col()
            .w_full()
            .gap(6.0)
            .child(parts_table(s, t))
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(cad::primary_btn(
                        "hw-cfg-add-btn",
                        if s.program.cfgmem.part.is_some() {
                            "Replace Device"
                        } else {
                            "Add Device"
                        },
                        t,
                        why.as_deref(),
                        |s: &mut AppState| {
                            let p = &CFGMEM_PARTS[s.program.cfgmem.pick.min(CFGMEM_PARTS.len() - 1)];
                            let r = cm::add_device(s, p);
                            act(s, r)
                        },
                    ))
                    .child(cad::cut(
                        format!(
                            "{} · {} Mbit · {}",
                            pick.name,
                            pick.density_mbit,
                            if pick.jedec_id.is_some() {
                                "SPI programming on the Simulation cable"
                            } else {
                                "BPI: image generation only (no BPI flash model)"
                            }
                        ),
                        VivadoTheme::FONT_UI,
                        t.text_dim,
                    )),
            ),
        t,
    )
}

fn image_card(s: &AppState, t: &VivadoTheme) -> El {
    let c = &s.program.cfgmem;
    let part = c.part.expect("caller checks");
    let busy = c.busy().is_some();
    let mut ifaces = Vec::new();
    for (i, f) in CfgMemInterface::ALL.into_iter().enumerate() {
        let ok = part.supports(f) && !busy;
        ifaces.push(segment(
            format!("hw-cfg-iface-{i}"),
            f.as_str(),
            c.interface == f,
            t,
            ok,
            move |s: &mut AppState| s.program.cfgmem.interface = f,
        ));
    }
    let mut formats = Vec::new();
    for (i, f) in [CfgMemFormat::Mcs, CfgMemFormat::Bin].into_iter().enumerate() {
        formats.push(segment(
            format!("hw-cfg-format-{i}"),
            &f.as_str().to_ascii_uppercase(),
            c.format == f,
            t,
            !busy,
            move |s: &mut AppState| s.program.cfgmem.format = f,
        ));
    }
    let why = cm::write_block(s);
    let mut body = col()
        .w_full()
        .gap(6.0)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(segments("hw-cfg-iface", "Interface", ifaces, t))
                .child(segments("hw-cfg-format", "Format", formats, t))
                .child(cad::dim("Start", t))
                .child(field("hw-cfg-start", "Start address", &c.start, 88.0, t, |s| {
                    &mut s.program.cfgmem.start
                })),
        )
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(cad::btn(
                    "hw-cfg-write",
                    "Generate Image",
                    t,
                    why.as_deref(),
                    |s: &mut AppState| {
                        let r = cm::write_image(s, None);
                        act(s, r)
                    },
                ))
                .child(cad::dim("from the current bitstream (write_cfgmem)", t)),
        );
    if let Some(w) = why.filter(|w| !w.starts_with("Busy")) {
        body = body.child(cad::cut(w, VivadoTheme::FONT_UI, t.warn).key("hw-cfg-write-why"));
    }
    if let Some(img) = &c.image {
        body = body
            .child(cad::kv(
                "Image",
                {
                    let p = c
                        .image_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    rune::text(p.clone())
                        .font(cad::mono_style())
                        .color(t.text)
                        .truncate_middle()
                        .help_when_truncated(p)
                        .key("hw-cfg-image-path")
                },
                LABEL_W,
                t,
            ))
            .child(cad::kv(
                "Range",
                cad::mono(
                    format!(
                        "{} – {} · {} B · CRC32 {:08X} · {} {}",
                        cad::addr_hex(u64::from(img.start_addr)),
                        cad::addr_hex(u64::from(img.end_addr())),
                        cad::grouped(img.payload.len()),
                        img.crc32,
                        img.interface.as_str(),
                        img.format.as_str().to_ascii_uppercase()
                    ),
                    t,
                ),
                LABEL_W,
                t,
            ));
    }
    cad::card("hw-cfg-image", "Configuration File", body, t)
}

fn step_toggle(key: &'static str, label: &'static str, on: bool, t: &VivadoTheme, busy: bool, f: fn(&mut AppState)) -> El {
    segments(key, label, vec![segment(format!("{key}-t"), label, on, t, !busy, f)], t)
}

fn program_card(s: &AppState, t: &VivadoTheme) -> El {
    let c = &s.program.cfgmem;
    let busy = c.busy().is_some();
    let o = c.opts;
    let steps = row()
        .w_full()
        .items_center()
        .gap(6.0)
        .child(cad::dim("Steps", t))
        .child(step_toggle("hw-cfg-opt-erase", "Erase", o.erase, t, busy, |s| {
            s.program.cfgmem.opts.erase ^= true
        }))
        .child(step_toggle(
            "hw-cfg-opt-blank",
            "Blank check",
            o.blank_check,
            t,
            busy,
            |s| s.program.cfgmem.opts.blank_check ^= true,
        ))
        .child(step_toggle("hw-cfg-opt-program", "Program", o.program, t, busy, |s| {
            s.program.cfgmem.opts.program ^= true
        }))
        .child(step_toggle("hw-cfg-opt-verify", "Verify", o.verify, t, busy, |s| {
            s.program.cfgmem.opts.verify ^= true
        }));
    let op_btn = |key: &'static str, label: &str, op: CfgOp, primary: bool| -> El {
        let why = cm::op_block(s, op);
        let on = move |s: &mut AppState| {
            let r = cm::run(s, op);
            act(s, r)
        };
        if primary {
            cad::primary_btn(key, label, t, why.as_deref(), on)
        } else {
            cad::btn(key, label, t, why.as_deref(), on)
        }
    };
    let mut actions = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(op_btn("hw-cfg-program", "Program", CfgOp::Program, true))
        .child(op_btn("hw-cfg-verify", "Verify", CfgOp::Verify, false))
        .child(op_btn("hw-cfg-blank", "Blank Check", CfgOp::BlankCheck, false))
        .child(op_btn("hw-cfg-erase", "Erase", CfgOp::Erase, false))
        .child(op_btn("hw-cfg-boot", "Boot from Flash", CfgOp::Boot, false));
    if let Some(op) = c.busy() {
        actions = actions.child(cad::dim(op.busy_label(), t).key("hw-cfg-busy"));
    }
    let mut body = col().w_full().gap(6.0).child(steps).child(actions);
    if let Some(why) = cm::op_block(s, CfgOp::Program).filter(|w| !w.starts_with("Busy")) {
        body = body.child(cad::cut(why, VivadoTheme::FONT_UI, t.warn).key("hw-cfg-why"));
    }
    if let Some(r) = &c.result {
        let ink = if r.ok { t.pass } else { t.fail };
        let mut res = col().key("hw-cfg-result").w_full().gap(2.0).child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(cad::badge(if r.ok { "OK" } else { "FAILED" }, ink, t))
                .child(cad::title(r.op.done_label(), t)),
        );
        match &r.report {
            Some(rep) => {
                for (i, step) in rep.steps.iter().enumerate() {
                    res = res.child(
                        cad::cut(step.clone(), cad::mono_style(), t.text)
                            .key(format!("hw-cfg-step-{i}")),
                    );
                }
                res = res.child(cad::dim(
                    format!(
                        "{} sector(s) erased · {} page(s) programmed · {} USER1 DR scans",
                        rep.sectors_erased, rep.pages_programmed, rep.jtag_dr_scans
                    ),
                    t,
                ));
            }
            None => {
                res = res.child(cad::cut(
                    r.line.clone(),
                    cad::mono_style(),
                    if r.ok { t.text } else { t.fail },
                ));
            }
        }
        body = body.child(res);
    }
    if let Some((start, len, crc)) = c.flashed {
        body = body.child(cad::kv(
            "In flash",
            cad::mono(
                format!(
                    "{} · {} B · CRC32 {crc:08X}",
                    cad::addr_hex(u64::from(start)),
                    cad::grouped(len)
                ),
                t,
            ),
            LABEL_W,
            t,
        ));
    }
    cad::card("hw-cfg-program-card", "Program Configuration Memory", body, t)
}

/// The configuration memory panel.
pub fn cfgmem_panel(s: &AppState, t: &VivadoTheme) -> El {
    let mut c = col().key("hw-cfg").w_full().gap(10.0);
    if let Some(p) = s.program.cfgmem.part {
        let path = if s.program.cable == crate::views::hw::CableChoice::Sim {
            "Simulation cable · TAP USER1 → JEDEC SPI flash model"
        } else {
            "Board cable · openFPGALoader -f (writes the bitstream; no readback)"
        };
        c = c.child(
            row()
                .key("hw-cfg-head")
                .w_full()
                .gap(8.0)
                .child(cad::kpi(
                    "hw-cfg-kpi-part",
                    "Configuration memory",
                    p.name,
                    t.text,
                    format!("{} · {} Mbit · JEDEC {}", p.vendor, p.density_mbit, p.jedec_hex()),
                    t,
                ))
                .child(cad::kpi("hw-cfg-kpi-path", "Programming path", if s.program.cable == crate::views::hw::CableChoice::Sim { "Indirect SPI" } else { "openFPGALoader" }, t.text, path, t)),
        );
        c = c.child(image_card(s, t)).child(program_card(s, t)).child(
            row().child(cad::btn(
                "hw-cfg-remove",
                "Remove Device",
                t,
                s.program.cfgmem.busy().map(|_| "Busy"),
                cm::remove_device,
            )),
        );
    }
    c.child(add_card(s, t)).child(el())
}
