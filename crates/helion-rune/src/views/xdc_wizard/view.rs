//! Constraints Wizard view: step rail, per-step row tables with port/clock
//! pickers and per-row problems, the Review preview with write/re-run
//! actions, and the WNS before/after card.

use rune::{El, ScrollSpec, TextEdit, Tx, col, edit_access, el, icon, row, text, text_input};
use rune_kit::{Dialog, Kit, Placement, Popover, glyphs};

use super::*;
use crate::theme::VivadoTheme;
use crate::views::dialogs::{DialogId, dim_label, dlg_button_tx, wiz_footer, wiz_rail_row};

/// Step-rail width.
const RAIL_W: f32 = 168.0;
/// Table row height (fits a compact text field).
const ROW_H: f32 = 26.0;
/// Table header height.
const HEAD_H: f32 = 22.0;
/// Field box height inside a row.
const FIELD_H: f32 = 22.0;
/// Step body viewport height (tables scroll inside it).
const BODY_H: f32 = 400.0;
/// Remove-row button column.
const X_W: f32 = 24.0;

// ---- view ----

/// Column width: fixed points or a grow share.
#[derive(Clone, Copy)]
enum W {
    Fixed(f32),
    Grow(f32),
}

fn sized(w: W, e: El) -> El {
    match w {
        W::Fixed(px) => e.w(px),
        W::Grow(g) => e.grow(g).min_w(0.0),
    }
}

fn head(key: String, cols: &[(&'static str, W)], t: &VivadoTheme) -> El {
    let mut r = row()
        .key(key)
        .w_full()
        .h(HEAD_H)
        .items_center()
        .gap(6.0)
        .px(6.0)
        .fill(t.header_bg);
    for (title, w) in cols {
        r = r.child(sized(
            *w,
            el().center_y().child(
                text(*title)
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim)
                    .truncate(),
            ),
        ));
    }
    r.child(el().w(X_W))
}

/// Inputs the view needs per cell.
struct Cx<'a> {
    s: &'a AppState,
    t: &'a VivadoTheme,
    kit: &'a Kit,
    ctx: &'a Ctx,
}

/// Bordered single-line field bound to `(field, id)`.
fn field_box(
    cx: &Cx,
    field: Field,
    id: u32,
    placeholder: &'static str,
    bad: bool,
    mono: bool,
) -> El {
    let t = cx.t;
    let key = format!("xdc-{field:?}-{id}").to_ascii_lowercase();
    let style = if mono {
        VivadoTheme::FONT_MONO.mono()
    } else {
        VivadoTheme::FONT_UI
    };
    let input = text_input(cx.s.dialogs.xdc.edit_ref(field, id), style)
        .single_line()
        .key(key.clone())
        .label(format!("{field:?}"))
        .grow(1.0)
        .min_w(0.0)
        .color(t.text)
        .placeholder(placeholder, t.text_faint)
        .caret(t.text, t.selection)
        .on_edit(edit_access(move |s: &mut AppState| {
            s.dialogs.xdc.edit_mut(field, id)
        }))
        .on_input_change(|s: &mut AppState, _tx: &mut Tx| s.dialogs.xdc.error = None);
    row()
        .key(format!("{key}-box"))
        .h(FIELD_H)
        .w_full()
        .items_center()
        .px(6.0)
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, if bad { t.fail } else { t.border })
        .child(input)
}

/// Field plus a chevron that lists valid choices (real ports / clocks).
fn pick_box(
    cx: &Cx,
    layer: &mut Vec<El>,
    field: Field,
    id: u32,
    placeholder: &'static str,
    bad: bool,
    choices: Vec<String>,
    empty_why: &'static str,
) -> El {
    let t = cx.t;
    let pick = Pick { field, id };
    let open = cx.s.dialogs.xdc.pop == Some(pick);
    let key = format!("xdc-{field:?}-{id}-pick").to_ascii_lowercase();
    let enabled = !choices.is_empty();
    let mut chevron = el()
        .key(key.clone())
        .role("button")
        .label("Choose")
        .w(20.0)
        .h(FIELD_H)
        .center()
        .radius(3.0)
        .child(
            icon(&glyphs::CHEVRON_DOWN, 11.0)
                .color(if enabled { t.text_dim } else { t.text_faint })
                .a11y_hidden(),
        );
    if enabled {
        chevron = chevron
            .focusable(true)
            .hover(t.selection)
            .help("Choose from the design")
            .on_press(move |s: &mut AppState| {
                let w = &mut s.dialogs.xdc;
                w.pop = if w.pop == Some(pick) {
                    None
                } else {
                    Some(pick)
                };
            });
    } else {
        chevron = chevron
            .disabled(true)
            .description(empty_why)
            .help(empty_why);
    }
    let current = cx.s.dialogs.xdc.text(field, id).trim().to_string();
    let mut list = col().w_full().gap(0.0);
    for (i, c) in choices.iter().enumerate() {
        let value = c.clone();
        let mut opt = el()
            .key(format!("{key}-opt-{i}"))
            .role("button")
            .label(format!("Choose {c}"))
            .focusable(true)
            .w_full()
            .h(22.0)
            .px(8.0)
            .center_y()
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                let w = &mut s.dialogs.xdc;
                *w.edit_mut(field, id) = TextEdit::new(value.clone());
                w.pop = None;
                w.error = None;
            })
            .child(
                text(c.clone())
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(t.text)
                    .truncate(),
            );
        if *c == current {
            opt = opt.fill(t.selection);
        }
        list = list.child(opt);
    }
    let trigger = row()
        .w_full()
        .items_center()
        .gap(2.0)
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .child(field_box(cx, field, id, placeholder, bad, true)),
        )
        .child(chevron);
    Popover::new(key)
        .trigger(trigger)
        .open(open && enabled)
        .placement(Placement::BottomStart)
        .padding(4.0)
        .on_close(|s: &mut AppState| s.dialogs.xdc.pop = None)
        .child(
            col()
                .w(220.0)
                .max_h(220.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
        .build(cx.kit, layer)
}

fn remove_btn(list: List, id: u32, t: &VivadoTheme) -> El {
    el().key(format!("xdc-{}-del-{id}", list.key()))
        .role("button")
        .label(format!("Remove {}", list.noun().to_ascii_lowercase()))
        .focusable(true)
        .w(X_W)
        .h(FIELD_H)
        .center()
        .radius(3.0)
        .hover(t.selection)
        .help("Remove row")
        .on_press(move |s: &mut AppState| s.dialogs.xdc.remove(list, id))
        .child(icon(&glyphs::CLOSE, 9.0).color(t.text_dim).a11y_hidden())
}

/// Two-state endpoint toggle ("port" / "clock").
fn kind_toggle(
    key: String,
    is_clock: bool,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let label = if is_clock { "clock" } else { "port" };
    el().key(key)
        .role("button")
        .label(format!("Endpoint kind: {label}"))
        .help("Switch between a port and a clock endpoint")
        .focusable(true)
        .w_full()
        .h(FIELD_H)
        .center()
        .radius(3.0)
        .border(1.0, t.border_light)
        .fill(t.chrome_bg)
        .hover(t.selection)
        .on_press(on)
        .child(text(label).font(VivadoTheme::FONT_UI).color(t.text))
}

fn check_box(
    key: String,
    label: &'static str,
    on_state: bool,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    let mark = if on_state { "☑" } else { "☐" };
    row()
        .key(key)
        .role("checkbox")
        .label(label)
        .focus_selected(on_state)
        .focusable(true)
        .h(FIELD_H)
        .items_center()
        .gap(4.0)
        .px(4.0)
        .radius(3.0)
        .hover(t.selection)
        .on_press(on)
        .child(text(mark).font(VivadoTheme::FONT_UI).color(if on_state {
            t.accent
        } else {
            t.text_dim
        }))
        .child(
            text(label)
                .font(VivadoTheme::FONT_MONO.mono())
                .color(t.text)
                .truncate(),
        )
}

/// One table row plus its problem lines.
fn table_row(list: List, id: u32, cells: Vec<(W, El)>, probs: &[&Issue], t: &VivadoTheme) -> El {
    let mut r = row()
        .key(format!("xdc-{}-row-{id}", list.key()))
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(6.0)
        .px(6.0);
    for (w, c) in cells {
        r = r.child(sized(w, c));
    }
    r = r.child(remove_btn(list, id, t));
    let mut block = col()
        .w_full()
        .border_sides([0.0, 0.0, 1.0, 0.0], t.border_light)
        .child(r);
    if !probs.is_empty() {
        for (n, p) in probs.iter().enumerate() {
            block = block.child(
                row()
                    .key(format!("xdc-{}-err-{id}-{n}", list.key()))
                    .w_full()
                    .h(18.0)
                    .px(12.0)
                    .items_center()
                    .child(
                        text(p.msg.clone())
                            .font(VivadoTheme::FONT_UI)
                            .color(t.fail)
                            .truncate()
                            .help_when_truncated(p.msg.clone()),
                    ),
            );
        }
    }
    block
}

fn add_btn(
    key: &'static str,
    label: &'static str,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    crate::views::cad::btn(key, label, t, None, on)
}

/// A titled table: header, rows, and an add button; an empty table says so.
fn table(
    key: &'static str,
    title: &str,
    cols: &[(&'static str, W)],
    rows: Vec<El>,
    empty: &str,
    add: El,
    t: &VivadoTheme,
) -> El {
    let mut frame = col()
        .key(format!("{key}-table"))
        .w_full()
        .padding(1.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(head(format!("{key}-head"), cols, t));
    if rows.is_empty() {
        frame = frame.child(
            row()
                .key(format!("{key}-empty"))
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(dim_label(empty, t).truncate()),
        );
    }
    for r in rows {
        frame = frame.child(r);
    }
    col()
        .w_full()
        .gap(4.0)
        .child(
            row()
                .w_full()
                .h(24.0)
                .items_center()
                .gap(8.0)
                .child(
                    text(title.to_string())
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text)
                        .grow(1.0)
                        .min_w(0.0),
                )
                .child(add),
        )
        .child(frame)
}

fn probs_for<'a>(all: &'a [Issue], list: List, id: u32) -> Vec<&'a Issue> {
    all.iter()
        .filter(|i| i.list == list && i.id == id)
        .collect()
}

fn clocks_page(cx: &Cx, layer: &mut Vec<El>, all: &[Issue]) -> El {
    let (w, t) = (&cx.s.dialogs.xdc, cx.t);
    let ins = cx.ctx.inputs();
    let why = "Run Synthesis to list the design's ports";
    let cols = [
        ("Name", W::Fixed(120.0)),
        ("Source port", W::Grow(1.0)),
        ("Period (ns)", W::Fixed(84.0)),
        ("Duty (%)", W::Fixed(64.0)),
    ];
    let rows = w
        .clocks
        .iter()
        .map(|c| {
            let p = probs_for(all, List::Clock, c.id);
            let bad = !p.is_empty();
            table_row(
                List::Clock,
                c.id,
                vec![
                    (
                        cols[0].1,
                        field_box(cx, Field::ClkName, c.id, "clk", bad, true),
                    ),
                    (
                        cols[1].1,
                        pick_box(
                            cx,
                            layer,
                            Field::ClkPort,
                            c.id,
                            "port",
                            bad,
                            ins.clone(),
                            why,
                        ),
                    ),
                    (
                        cols[2].1,
                        field_box(cx, Field::ClkPeriod, c.id, "10.000", bad, true),
                    ),
                    (
                        cols[3].1,
                        field_box(cx, Field::ClkDuty, c.id, "50", bad, true),
                    ),
                ],
                &p,
                t,
            )
        })
        .collect();
    let primary = table(
        "xdc-clk",
        "Primary clocks",
        &cols,
        rows,
        "No clocks yet. Add one for each clock input.",
        add_btn("xdc-clk-add", "+ Add clock", t, add_clock_row),
        t,
    );
    let names = clock_names(w, cx.ctx);
    let gcols = [
        ("Name", W::Fixed(110.0)),
        ("Master", W::Fixed(130.0)),
        ("Target pin/port", W::Grow(1.0)),
        ("÷", W::Fixed(48.0)),
        ("×", W::Fixed(48.0)),
        ("", W::Fixed(70.0)),
    ];
    let grows = w
        .generated
        .iter()
        .map(|g| {
            let p = probs_for(all, List::Gen, g.id);
            let bad = !p.is_empty();
            let id = g.id;
            let masters: Vec<String> = names
                .iter()
                .filter(|n| *n != g.name.text.trim())
                .cloned()
                .collect();
            table_row(
                List::Gen,
                g.id,
                vec![
                    (
                        gcols[0].1,
                        field_box(cx, Field::GenName, id, "clk_div2", bad, true),
                    ),
                    (
                        gcols[1].1,
                        pick_box(
                            cx,
                            layer,
                            Field::GenMaster,
                            id,
                            "master",
                            bad,
                            masters,
                            "Define a clock first",
                        ),
                    ),
                    (
                        gcols[2].1,
                        field_box(cx, Field::GenTarget, id, "div_reg/Q", bad, true),
                    ),
                    (gcols[3].1, field_box(cx, Field::GenDiv, id, "2", bad, true)),
                    (gcols[4].1, field_box(cx, Field::GenMul, id, "1", bad, true)),
                    (
                        gcols[5].1,
                        check_box(
                            format!("xdc-gen-inv-{id}"),
                            "-invert",
                            g.invert,
                            t,
                            move |s: &mut AppState| {
                                if let Some(r) =
                                    s.dialogs.xdc.generated.iter_mut().find(|r| r.id == id)
                                {
                                    r.invert = !r.invert;
                                }
                            },
                        ),
                    ),
                ],
                &p,
                t,
            )
        })
        .collect();
    let derived = table(
        "xdc-gen",
        "Generated clocks",
        &gcols,
        grows,
        "No generated clocks. Derive one from a master clock (dividers, PLL outputs).",
        add_btn("xdc-gen-add", "+ Add generated clock", t, add_gen_row),
        t,
    );
    let mut page = col().w_full().gap(12.0);
    if !cx.ctx.known.is_empty() {
        let list: Vec<String> = cx
            .ctx
            .known
            .iter()
            .map(|k| {
                format!(
                    "{} ({} ns on {})",
                    k.name,
                    crate::views::report_kit::ns3(k.period_ps as i64),
                    k.source
                )
            })
            .collect();
        let line = format!("Already defined in the XDC: {}", list.join(", "));
        page = page.child(
            text(line.clone())
                .key("xdc-known-clocks")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate()
                .help_when_truncated(line),
        );
    }
    page.child(primary).child(derived)
}

fn delays_page(cx: &Cx, layer: &mut Vec<El>, all: &[Issue], output: bool) -> El {
    let (w, t) = (&cx.s.dialogs.xdc, cx.t);
    let (list, rows_src, ports) = if output {
        (List::Out, &w.outputs, cx.ctx.outputs())
    } else {
        (List::In, &w.inputs, cx.ctx.inputs())
    };
    let (f_port, f_clock, f_min, f_max) = if output {
        (
            Field::OutPort,
            Field::OutClock,
            Field::OutMin,
            Field::OutMax,
        )
    } else {
        (Field::InPort, Field::InClock, Field::InMin, Field::InMax)
    };
    let clocks = clock_names(w, cx.ctx);
    let cols = [
        ("Port", W::Grow(1.0)),
        ("Clock", W::Fixed(150.0)),
        ("-min (ns)", W::Fixed(84.0)),
        ("-max (ns)", W::Fixed(84.0)),
    ];
    let rows = rows_src
        .iter()
        .map(|d| {
            let p = probs_for(all, list, d.id);
            let bad = !p.is_empty();
            table_row(
                list,
                d.id,
                vec![
                    (
                        cols[0].1,
                        pick_box(
                            cx,
                            layer,
                            f_port,
                            d.id,
                            "port",
                            bad,
                            ports.clone(),
                            "Run Synthesis to list the design's ports",
                        ),
                    ),
                    (
                        cols[1].1,
                        pick_box(
                            cx,
                            layer,
                            f_clock,
                            d.id,
                            "clock",
                            bad,
                            clocks.clone(),
                            "Define a clock first",
                        ),
                    ),
                    (cols[2].1, field_box(cx, f_min, d.id, "hold", bad, true)),
                    (cols[3].1, field_box(cx, f_max, d.id, "setup", bad, true)),
                ],
                &p,
                t,
            )
        })
        .collect();
    let (key, title, empty, add_key, add_label): (
        &'static str,
        &str,
        &str,
        &'static str,
        &'static str,
    ) = if output {
        (
            "xdc-out",
            "Constrained outputs",
            "No output delays. Add one per output the board samples.",
            "xdc-out-add",
            "+ Add output delay",
        )
    } else {
        (
            "xdc-in",
            "Constrained inputs",
            "No input delays. Add one per input driven by an external device.",
            "xdc-in-add",
            "+ Add input delay",
        )
    };
    table(
        key,
        title,
        &cols,
        rows,
        empty,
        add_btn(add_key, add_label, t, move |s: &mut AppState| {
            add_delay_row(s, output)
        }),
        t,
    )
}

fn exceptions_page(cx: &Cx, layer: &mut Vec<El>, all: &[Issue]) -> El {
    let (w, t) = (&cx.s.dialogs.xdc, cx.t);
    let ports = cx.ctx.all_ports();
    let clocks = clock_names(w, cx.ctx);
    let why_ports = "Run Synthesis to list the design's ports";
    let fcols = [
        ("From", W::Fixed(58.0)),
        ("", W::Grow(1.0)),
        ("To", W::Fixed(58.0)),
        ("", W::Grow(1.0)),
    ];
    let frows = w
        .false_paths
        .iter()
        .map(|f| {
            let p = probs_for(all, List::Fp, f.id);
            let bad = !p.is_empty();
            let id = f.id;
            let choices = |clock: bool| if clock { clocks.clone() } else { ports.clone() };
            let why = |clock: bool| {
                if clock {
                    "Define a clock first"
                } else {
                    why_ports
                }
            };
            table_row(
                List::Fp,
                id,
                vec![
                    (
                        fcols[0].1,
                        kind_toggle(
                            format!("xdc-fp-fromkind-{id}"),
                            f.from_clock,
                            t,
                            move |s: &mut AppState| {
                                if let Some(r) =
                                    s.dialogs.xdc.false_paths.iter_mut().find(|r| r.id == id)
                                {
                                    r.from_clock = !r.from_clock;
                                }
                            },
                        ),
                    ),
                    (
                        fcols[1].1,
                        pick_box(
                            cx,
                            layer,
                            Field::FpFrom,
                            id,
                            "from",
                            bad,
                            choices(f.from_clock),
                            why(f.from_clock),
                        ),
                    ),
                    (
                        fcols[2].1,
                        kind_toggle(
                            format!("xdc-fp-tokind-{id}"),
                            f.to_clock,
                            t,
                            move |s: &mut AppState| {
                                if let Some(r) =
                                    s.dialogs.xdc.false_paths.iter_mut().find(|r| r.id == id)
                                {
                                    r.to_clock = !r.to_clock;
                                }
                            },
                        ),
                    ),
                    (
                        fcols[3].1,
                        pick_box(
                            cx,
                            layer,
                            Field::FpTo,
                            id,
                            "to",
                            bad,
                            choices(f.to_clock),
                            why(f.to_clock),
                        ),
                    ),
                ],
                &p,
                t,
            )
        })
        .collect();
    let mcols = [
        ("From port", W::Grow(1.0)),
        ("To port", W::Grow(1.0)),
        ("Delay (ns)", W::Fixed(84.0)),
        ("", W::Fixed(124.0)),
    ];
    let mrows = w
        .max_delays
        .iter()
        .map(|m| {
            let p = probs_for(all, List::Md, m.id);
            let bad = !p.is_empty();
            let id = m.id;
            table_row(
                List::Md,
                id,
                vec![
                    (
                        mcols[0].1,
                        pick_box(
                            cx,
                            layer,
                            Field::MdFrom,
                            id,
                            "from",
                            bad,
                            ports.clone(),
                            why_ports,
                        ),
                    ),
                    (
                        mcols[1].1,
                        pick_box(
                            cx,
                            layer,
                            Field::MdTo,
                            id,
                            "to",
                            bad,
                            ports.clone(),
                            why_ports,
                        ),
                    ),
                    (
                        mcols[2].1,
                        field_box(cx, Field::MdDelay, id, "8.000", bad, true),
                    ),
                    (
                        mcols[3].1,
                        check_box(
                            format!("xdc-md-dp-{id}"),
                            "-datapath_only",
                            m.datapath_only,
                            t,
                            move |s: &mut AppState| {
                                if let Some(r) =
                                    s.dialogs.xdc.max_delays.iter_mut().find(|r| r.id == id)
                                {
                                    r.datapath_only = !r.datapath_only;
                                }
                            },
                        ),
                    ),
                ],
                &p,
                t,
            )
        })
        .collect();
    col()
        .w_full()
        .gap(12.0)
        .child(table(
            "xdc-fp",
            "False paths",
            &fcols,
            frows,
            "No false paths. Add one for each path timing should ignore.",
            add_btn("xdc-fp-add", "+ Add false path", t, add_fp_row),
            t,
        ))
        .child(table(
            "xdc-md",
            "Max delays",
            &mcols,
            mrows,
            "No max-delay overrides.",
            add_btn("xdc-md-add", "+ Add max delay", t, add_md_row),
            t,
        ))
}

fn ns_or_dash(v: Option<i64>) -> String {
    v.map(|ps| format!("{} ns", crate::views::report_kit::ns3(ps)))
        .unwrap_or_else(|| "—".into())
}

fn review_page(cx: &Cx, all: &[Issue]) -> El {
    let (s, t, kit) = (cx.s, cx.t, cx.kit);
    let w = &s.dialogs.xdc;
    let sdc = sdc_text(w, cx.ctx);
    let mut page = col().w_full().gap(8.0);
    let counts = [
        (w.clocks.len(), "clock"),
        (w.generated.len(), "generated clock"),
        (w.inputs.len(), "input delay"),
        (w.outputs.len(), "output delay"),
        (w.false_paths.len(), "false path"),
        (w.max_delays.len(), "max delay"),
    ];
    let summary: Vec<String> = counts
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}{}", if *n == 1 { "" } else { "s" }))
        .collect();
    let summary = if summary.is_empty() {
        "Nothing to generate yet: add rows on the earlier steps.".to_string()
    } else {
        summary.join(" · ")
    };
    page = page.child(
        text(summary)
            .key("xdc-review-summary")
            .font(VivadoTheme::FONT_UI)
            .color(t.text),
    );
    if !all.is_empty() {
        let mut list = col()
            .key("xdc-review-issues")
            .w_full()
            .gap(0.0)
            .padding(6.0)
            .border(1.0, t.fail);
        for (n, i) in all.iter().take(6).enumerate() {
            let step = i.list.step();
            list = list.child(
                row()
                    .key(format!("xdc-review-issue-{n}"))
                    .role("button")
                    .label(format!("Go to {}", step.title()))
                    .focusable(true)
                    .w_full()
                    .h(20.0)
                    .items_center()
                    .gap(6.0)
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| {
                        s.dialogs.xdc.step = step;
                        s.dialogs.xdc.error = None;
                    })
                    .child(
                        text(i.list.noun())
                            .font(VivadoTheme::FONT_TITLE)
                            .color(t.fail),
                    )
                    .child(
                        text(i.msg.clone())
                            .font(VivadoTheme::FONT_UI)
                            .color(t.text)
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate()
                            .help_when_truncated(i.msg.clone()),
                    ),
            );
        }
        if all.len() > 6 {
            list = list.child(dim_label(&format!("…and {} more", all.len() - 6), t));
        }
        page = page.child(list);
    }
    let parse = validate_sdc(cx.ctx, &sdc);
    let mut preview = col().gap(0.0);
    for (i, line) in sdc.lines().enumerate() {
        let comment = line.starts_with('#');
        preview = preview.child(
            text(line.to_string())
                .key(format!("xdc-sdc-{i}"))
                .font(VivadoTheme::FONT_MONO.mono())
                .color(if comment { t.code_comment } else { t.text }),
        );
    }
    let parse_line = match &parse {
        Ok(()) => text("Generated SDC parses clean with the Helion SDC frontend.")
            .font(VivadoTheme::FONT_UI)
            .color(t.pass),
        Err(e) => text(format!("Generated SDC failed to parse: {e}"))
            .font(VivadoTheme::FONT_UI)
            .color(t.fail)
            .truncate(),
    };
    page = page.child(parse_line.key("xdc-sdc-parse")).child(
        el().key("xdc-sdc")
            .w_full()
            .h(170.0)
            .padding(8.0)
            .fill(t.console_bg)
            .border(1.0, t.border)
            .scroll_xy()
            .child(preview),
    );
    // Target + actions.
    let target = target_path(s);
    let written = w.current_write(&sdc).is_some();
    let target_label = match &target {
        Some(p) => p.display().to_string(),
        None => "No constraints file on disk (in-memory project)".into(),
    };
    page = page.child(
        row()
            .key("xdc-target")
            .w_full()
            .h(20.0)
            .items_center()
            .gap(6.0)
            .child(dim_label("Target file:", t))
            .child(
                text(target_label.clone())
                    .font(VivadoTheme::FONT_MONO.mono())
                    .color(if target.is_some() {
                        t.text
                    } else {
                        t.text_faint
                    })
                    .grow(1.0)
                    .min_w(0.0)
                    .truncate_middle()
                    .help_when_truncated(target_label),
            ),
    );
    let write_label = match &target {
        Some(p) => format!("Write to {}", file_name(p)),
        None => "Write to file".to_string(),
    };
    let wreason = if written {
        Some("Already written: edit a row to write again.".to_string())
    } else {
        write_reason(s, all.len(), w.row_count()).or_else(|| parse.as_ref().err().cloned())
    };
    let rerun = rerun_action(s, written);
    let rreason = rerun_reason(s, written);
    let rerun_label = if s.jobs.wizard_pending() {
        "Analyzing…"
    } else if w.timing == Some(TimingOutcome::AwaitingRun) {
        "Synthesizing…"
    } else if rerun == Rerun::SynthFirst {
        "Synthesize & re-run timing"
    } else {
        "Re-run timing"
    };
    let mut actions = row()
        .key("xdc-actions")
        .w_full()
        .h(28.0)
        .items_center()
        .gap(8.0)
        .child(crate::views::cad::primary_btn(
            "xdc-write",
            &write_label,
            t,
            wreason.as_deref(),
            |s: &mut AppState| {
                if let Err(e) = write_to_target(s) {
                    s.dialogs.xdc.error = Some(e);
                }
            },
        ))
        .child({
            let b = crate::views::cad::btn("xdc-rerun", rerun_label, t, rreason, rerun_pressed);
            if rerun == Rerun::SynthFirst {
                b.help("No synthesized design yet: runs synth_1, then re-runs timing with this file.")
            } else {
                b
            }
        })
        .child(el().grow(1.0))
        .child(dlg_button_tx(
            "xdc-export",
            "Export XDC…",
            false,
            kit,
            |s: &mut AppState, tx: &mut Tx| {
                let ctx = Ctx::from_state(s);
                let lines = sdc_text(&s.dialogs.xdc, &ctx)
                    .lines()
                    .map(str::to_string)
                    .collect();
                crate::pickers::export_xdc(tx, lines);
            },
        ));
    if written && let Some(wr) = &w.written {
        let note = if wr.added {
            "Written; the new file joined constrs_1."
        } else {
            "Written."
        };
        actions = actions.child(
            text(note)
                .key("xdc-written")
                .font(VivadoTheme::FONT_UI)
                .color(t.pass),
        );
    }
    page = page.child(actions);
    page.child(timing_card(s, t, written))
}

/// WNS before/after card (old `TimingOutcome` rendering).
fn timing_card(s: &AppState, t: &VivadoTheme, written: bool) -> El {
    let w = &s.dialogs.xdc;
    let body: El = match (&w.timing, written) {
        (Some(TimingOutcome::AwaitingRun), _) => {
            text("Synthesis (synth_1) is running: timing re-runs with the new constraints when it finishes.")
                .key("xdc-awaiting-run")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate()
        }
        (Some(TimingOutcome::Running), _) => text("Timing analysis is running on a worker…")
            .font(VivadoTheme::FONT_UI)
            .color(t.text_dim),
        (Some(TimingOutcome::Done { before, after }), true) => {
            let ink = |v: &Option<i64>| match v {
                Some(ps) if *ps < 0 => t.fail,
                Some(_) => t.pass,
                None => t.text_dim,
            };
            let verdict = match after {
                Some(ps) if *ps < 0 => "VIOLATED",
                Some(_) => "met",
                None => "no constrained paths",
            };
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .child(dim_label("WNS before", t))
                .child(
                    text(ns_or_dash(*before))
                        .key("xdc-wns-before")
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(ink(before)),
                )
                .child(text("→").font(VivadoTheme::FONT_UI).color(t.text_dim))
                .child(dim_label("after", t))
                .child(
                    text(ns_or_dash(*after))
                        .key("xdc-wns-after")
                        .font(VivadoTheme::FONT_MONO.mono())
                        .color(ink(after)),
                )
                .child(
                    text(verdict)
                        .font(VivadoTheme::FONT_TITLE)
                        .color(ink(after)),
                )
        }
        (Some(TimingOutcome::Failed(e)), _) => text(format!("Timing run failed: {e}"))
            .font(VivadoTheme::FONT_UI)
            .color(t.fail)
            .truncate()
            .help_when_truncated(e.clone()),
        _ if !written => dim_label("Write the file, then re-run timing to compare WNS.", t),
        _ if rerun_action(s, written) == Rerun::SynthFirst => dim_label(
            "No synthesized design yet: Synthesize & re-run timing runs synth_1, then compares WNS.",
            t,
        ),
        _ => dim_label("Re-run timing to compare WNS with the new constraints.", t),
    };
    col()
        .key("xdc-timing")
        .w_full()
        .gap(4.0)
        .padding(8.0)
        .fill(t.rail_bg)
        .border(1.0, t.border_light)
        .child(
            text("Timing with the new constraints")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(body)
}

/// No-design notice with the one action that fixes it.
fn design_banner(s: &AppState, t: &VivadoTheme) -> El {
    let running = s.runs.is_active();
    let mut r = row()
        .key("xdc-no-design")
        .w_full()
        .h(26.0)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.rail_bg)
        .border(1.0, t.border_light)
        .child(
            text(if running {
                "Synthesis is running: ports will be checked when it finishes."
            } else {
                "No synthesized design: port names are not checked yet."
            })
            .font(VivadoTheme::FONT_UI)
            .color(t.warn)
            .grow(1.0)
            .min_w(0.0)
            .truncate(),
        );
    if !running {
        r = r.child(crate::views::cad::link(
            "xdc-run-synth",
            "Run Synthesis",
            t,
            |s: &mut AppState| crate::runs::launch(s, crate::runs::RunKind::Synth),
        ));
    }
    r
}

fn page(cx: &Cx, layer: &mut Vec<El>) -> El {
    let (s, t) = (cx.s, cx.t);
    let w = &s.dialogs.xdc;
    let all = issues(w, cx.ctx);
    let mut page = col()
        .key("xdc-body")
        .w_full()
        .gap(8.0)
        .child(
            text(w.step.title())
                .key("xdc-step-title")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
        .child(dim_label(w.step.hint(), t).wrap());
    if cx.ctx.ports.is_none() && w.step != XdcStep::Review {
        page = page.child(design_banner(s, t));
    }
    let body = match w.step {
        XdcStep::Clocks => clocks_page(cx, layer, &all),
        XdcStep::InputDelays => delays_page(cx, layer, &all, false),
        XdcStep::OutputDelays => delays_page(cx, layer, &all, true),
        XdcStep::Exceptions => exceptions_page(cx, layer, &all),
        XdcStep::Review => review_page(cx, &all),
    };
    page.child(body)
}

/// The Constraints Wizard dialog (mounted by `dialogs::push_dialogs`).
pub fn xdc_dialog(
    layer: &mut Vec<El>,
    pops: &mut Vec<El>,
    s: &AppState,
    t: &VivadoTheme,
    kit: &Kit,
) {
    let ctx = Ctx::from_state(s);
    let cx = Cx {
        s,
        t,
        kit,
        ctx: &ctx,
    };
    let w = &s.dialogs.xdc;
    let step = w.step;
    let at = step.index();
    let all = issues(w, &ctx);
    let mut rail = col()
        .key("xdc-rail")
        .w(RAIL_W)
        .gap(2.0)
        .py(8.0)
        .fill(t.rail_bg);
    for (i, st) in XdcStep::ALL.iter().enumerate() {
        let st = *st;
        let reachable = i <= w.visited.max(at);
        let mut item = wiz_rail_row(
            format!("xdc-rail-{i}"),
            i,
            st.title(),
            st == step,
            i < at,
            t,
            move |s: &mut AppState| {
                let w = &mut s.dialogs.xdc;
                if i <= w.visited.max(w.step.index()) {
                    w.step = st;
                    w.error = None;
                    w.pop = None;
                }
            },
        );
        let n = all.iter().filter(|x| st.lists().contains(&x.list)).count();
        if st != XdcStep::Review && n > 0 && reachable {
            item = item.help(format!("{n} problem{}", if n == 1 { "" } else { "s" }));
        }
        if !reachable {
            item = item
                .layer_opacity(0.5)
                .description("Reach this step with Next");
        }
        rail = rail.child(item);
    }
    let err = w.error.clone();
    let body = col()
        .w_full()
        .child(
            row()
                .key("xdc-split")
                .w_full()
                .h(BODY_H)
                .child(rail)
                .child(el().w(1.0).h_full().a11y_hidden().fill(t.border))
                .child(
                    col()
                        .key("xdc-scroll")
                        .grow(1.0)
                        .min_w(0.0)
                        .h_full()
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(el().w_full().padding(12.0).child(page(&cx, pops))),
                ),
        )
        .child(el().w_full().h(1.0).fill(t.border_light).a11y_hidden())
        .child(wiz_footer(
            "xdc",
            step.prev().is_some(),
            step.next().is_none(),
            err.as_deref(),
            t,
            kit,
            go_back,
            go_next,
            finish,
        ));
    Dialog::new(DialogId::Constraints.key(), "Constraints Wizard")
        .open(true)
        .width(940.0)
        .fit()
        .on_close(|s: &mut AppState| {
            s.dialogs.xdc.pop = None;
            s.dialogs.close();
        })
        .child(body)
        .build(kit, layer);
}
