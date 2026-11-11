//! Logic Analyzer surface (`CenterPane::Ila`).
//!
//! Layout (ports the old `paint_ila` / `paint_ila_wave` / `paint_ila_traces`
//! from `helion-gui/src/bin/helion-ide.rs` onto Rune):
//!
//! - Header toolbar: Arm Capture / Stop, Save session / Load session /
//!   Export VCD (native dialogs), and the live "capturing… N cycles" status.
//! - Status strip: Status · Probes · Trigger · Depth.
//! - Probes panel (left): net search, the net list with `+`, the configured
//!   probes (group check, trigger pick, name, `[width]`, Bin/Hex/Dec radix,
//!   `×`), and "Group checked as bus" with a bus name.
//! - Trigger & capture settings (right, top): kind (Immediate / Rising /
//!   Falling / Value with value + mask), Depth, Pre-trigger %, Max cycles,
//!   Timescale, and the clock net.
//! - Capture viewer (right): zoom −/+/Fit, pan, cursor + A/B/Δ readout,
//!   trigger marker, a Name/Value column, and the trace canvas (press or
//!   drag to place the cursor). The canvas reports its width through
//!   `on_layout` so the engine's `IlaViewer` maps samples to pixels.
//!
//! Colors are [`VivadoTheme`] tokens only (`wave_*` for traces).

use helion_verif::ila::{IlaCapture, IlaRadix, IlaTriggerKind, format_value};
use rune::tok::TextStyle;
use rune::{El, LineCap, Phase, ScrollSpec, Shape, Tx, canvas, col, el, row, text, text_input};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::ila::{self, IlaStatus, TrigChoice};
use crate::views::sources::pane_header;

const ROW_H: f32 = 20.0;
const TOOL_H: f32 = 26.0;
const LANE_H: f32 = 24.0;
const RULER_H: f32 = 20.0;
const LEFT_W: f32 = 320.0;
const NAMES_W: f32 = 200.0;
const NET_LIST_H: f32 = 132.0;
const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

fn label(content: &str, t: &VivadoTheme) -> El {
    text(content.to_string())
        .font(VivadoTheme::FONT_UI)
        .color(t.text)
        .truncate()
        .help_when_truncated(content.to_string())
}

fn caption(content: &str, t: &VivadoTheme) -> El {
    text(content.to_string()).font(FONT_PIN).color(t.text_dim)
}

/// Toolbar/inline button; disabled ones carry the reason.
fn btn(key: &str, text_label: &str, t: &VivadoTheme, enabled: bool, why: &str) -> El {
    let b = row()
        .key(key.to_string())
        .role("button")
        .label(text_label.to_string())
        .h(20.0)
        .px(6.0)
        .items_center()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            text(text_label.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text),
        );
    if enabled {
        b.focusable(true).hover(t.selection)
    } else {
        b.disabled(true)
            .description(why.to_string())
            .help(why.to_string())
            .layer_opacity(0.45)
    }
}

fn section_head(key: &str, title: &str, t: &VivadoTheme) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            text(title.to_string())
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text),
        )
}

/// Compact single-line field.
fn field(
    key: &str,
    a11y: &str,
    edit: &rune::TextEdit,
    w: f32,
    t: &VivadoTheme,
    on_edit: fn(&mut AppState) -> &mut rune::TextEdit,
) -> El {
    text_input(edit, VivadoTheme::FONT_MONO.mono())
        .single_line()
        .key(key.to_string())
        .label(a11y.to_string())
        .w(w)
        .h(20.0)
        .px(6.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border_light)
        .radius(3.0)
        .color(t.text)
        .caret(t.text, t.selection)
        .on_edit(on_edit)
        .on_submit(|s: &mut AppState, _tx: &mut Tx| {
            ila::act(s, "Invalid capture setting", |s| {
                s.ila
                    .apply_fields()
                    .map(|()| "ila settings applied".to_string())
            })
        })
}

/// Segmented choice strip (`on` marks the active segment).
fn segment(key: String, text_label: &str, on: bool, t: &VivadoTheme) -> El {
    el().key(key)
        .role("radio")
        .label(text_label.to_string())
        .focusable(true)
        .focus_selected(on)
        .h_full()
        .px(7.0)
        .center()
        .fill(if on { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .child(
            text(text_label.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text),
        )
}

fn segments(key: &str, a11y: &str, items: Vec<El>, t: &VivadoTheme) -> El {
    let mut r = row()
        .key(key.to_string())
        .role("radiogroup")
        .label(a11y.to_string())
        .h(20.0)
        .items_center()
        .border(1.0, t.border);
    for it in items {
        r = r.child(it);
    }
    r
}

/// Header toolbar: arm/stop, session files, VCD export, live status.
fn header(s: &AppState, t: &VivadoTheme) -> El {
    let capturing = s.ila.is_capturing();
    let arm_ok = ila::can_arm(s);
    let arm_why = arm_ok.clone().err().unwrap_or_default();
    let has_cap = s.ila.capture.is_some();
    let (status, ink) = if capturing {
        (
            format!("capturing… {} cycles", s.ila.progress_cycles()),
            t.link,
        )
    } else if let Some(e) = &s.ila.last_error {
        (e.clone(), t.fail)
    } else if let Some(p) = &s.ila.session_path {
        (format!("Session: {}", p.display()), t.text_faint)
    } else {
        (String::new(), t.text_faint)
    };
    let mut r = row()
        .key("ila-toolbar")
        .w_full()
        .h(TOOL_H)
        .px(4.0)
        .gap(3.0)
        .items_center()
        .fill(t.header_bg);
    r = if capturing {
        r.child(
            btn("ila-stop", "■ Stop", t, true, "")
                .help("Stop the capture (keeps the samples gathered so far)")
                .on_press(|s: &mut AppState| ila::act(s, "Cannot stop", ila::disarm)),
        )
    } else {
        r.child(
            btn("ila-arm", "▶ Arm Capture", t, arm_ok.is_ok(), &arm_why)
                .help("Run the helion-sim event kernel with the probes instrumented")
                .on_press(|s: &mut AppState| ila::act(s, "Cannot arm capture", ila::arm)),
        )
    };
    r.child(el().w(6.0))
        .child(
            btn(
                "ila-save",
                "Save session",
                t,
                !capturing,
                "Wait for the capture to finish",
            )
            .on_press_tx(|s: &mut AppState, tx: &mut Tx| {
                ila::pick_save_session(tx, ila::default_session_path(s))
            }),
        )
        .child(
            btn(
                "ila-load",
                "Load session",
                t,
                !capturing,
                "Wait for the capture to finish",
            )
            .on_press_tx(|_s: &mut AppState, tx: &mut Tx| ila::pick_load_session(tx)),
        )
        .child(
            btn(
                "ila-export",
                "Export VCD",
                t,
                has_cap && !capturing,
                "Capture something first",
            )
            .help("Save the capture as a VCD file (GTKWave, Surfer)")
            .on_press_tx(|_s: &mut AppState, tx: &mut Tx| ila::pick_export_vcd(tx)),
        )
        .child(
            el().key("ila-status-text")
                .grow(1.0)
                .min_w(0.0)
                .px(6.0)
                .child(
                    text(status.clone())
                        .font(VivadoTheme::FONT_UI)
                        .color(ink)
                        .truncate_middle()
                        .help_when_truncated(status),
                ),
        )
}

/// Status strip: Status · Probes · Trigger · Depth.
fn status_strip(s: &AppState, t: &VivadoTheme) -> El {
    let ila = &s.ila;
    let status_ink = match ila.status {
        IlaStatus::Done => t.pass,
        IlaStatus::Error => t.fail,
        IlaStatus::Capturing => t.link,
        IlaStatus::Idle => t.text,
    };
    let item = |key: &str, k: &str, v: String, ink: rune::Color| {
        row()
            .key(key.to_string())
            .gap(6.0)
            .items_center()
            .min_w(0.0)
            .child(caption(k, t))
            .child(
                text(v.clone())
                    .font(VivadoTheme::FONT_MONO)
                    .color(ink)
                    .truncate()
                    .help_when_truncated(v),
            )
    };
    row()
        .key("ila-strip")
        .w_full()
        .h(22.0)
        .px(10.0)
        .gap(18.0)
        .items_center()
        .fill(t.panel_bg)
        .child(item(
            "ila-strip-status",
            "Status",
            ila.status.label().into(),
            status_ink,
        ))
        .child(item(
            "ila-strip-probes",
            "Probes",
            ila.config.probes.len().to_string(),
            t.text,
        ))
        .child(item(
            "ila-strip-trigger",
            "Trigger",
            format!(
                "{} @ {}",
                ila.config.trigger.kind.label(),
                ila.trigger_probe_name()
            ),
            t.text,
        ))
        .child(item(
            "ila-strip-depth",
            "Depth",
            format!("{} samples", ila.config.depth),
            t.text,
        ))
}

/// One configured probe: group check, trigger pick, name, width, radix, ×.
fn probe_row(s: &AppState, t: &VivadoTheme, i: usize) -> El {
    let p = &s.ila.config.probes[i];
    let checked = s.ila.group.get(i).copied().unwrap_or(false);
    let is_trig = s.ila.config.trigger.probe == i;
    let check = el()
        .key(format!("ila-probe-group-{i}"))
        .role("checkbox")
        .label(format!("Group {}", p.name))
        .focusable(true)
        .focus_selected(checked)
        .w(16.0)
        .h(16.0)
        .center()
        .border(1.0, t.border_light)
        .fill(if checked { t.selection } else { t.chrome_bg })
        .hover(t.selection)
        .help("Check probes, then Group checked as bus")
        .on_press(move |s: &mut AppState| {
            if let Some(g) = s.ila.group.get_mut(i) {
                *g = !*g;
            }
        })
        .child(
            text(if checked { "✓" } else { "" })
                .font(FONT_PIN)
                .color(t.text),
        );
    let trig = el()
        .key(format!("ila-probe-trig-{i}"))
        .role("radio")
        .label(format!("Trigger on {}", p.name))
        .focusable(true)
        .focus_selected(is_trig)
        .w(18.0)
        .h(16.0)
        .center()
        .hover(t.selection)
        .help("Trigger on this probe")
        .on_press(move |s: &mut AppState| {
            ila::act(s, "Cannot set trigger", |s| ila::set_trigger_probe(s, i))
        })
        .child(
            text(if is_trig { "◉" } else { "○" })
                .font(VivadoTheme::FONT_UI)
                .color(if is_trig { t.wave_marker } else { t.text_faint }),
        );
    let radix: El = if p.is_bus() {
        let mut items = Vec::new();
        for r in [IlaRadix::Bin, IlaRadix::Hex, IlaRadix::Dec] {
            let on = p.radix == r;
            items.push(
                segment(
                    format!("ila-probe-radix-{i}-{}", r.label().to_lowercase()),
                    r.label(),
                    on,
                    t,
                )
                .on_press(move |s: &mut AppState| {
                    let verb = format!("ila radix {i} {}", r.label().to_lowercase());
                    crate::tcl::eval_command(s, &verb);
                }),
            );
        }
        segments(&format!("ila-probe-radix-{i}"), "Radix", items, t)
    } else {
        // Scalars read 0/1 in any radix: keep the column aligned.
        el().w(96.0)
    };
    row()
        .key(format!("ila-probe-{i}"))
        .w_full()
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .fill(if is_trig {
            t.selection.alpha(0.5)
        } else {
            t.panel_bg
        })
        .child(check)
        .child(trig)
        .child(el().grow(1.0).min_w(0.0).child(label(&p.name, t)))
        .child(caption(&format!("[{}]", p.width()), t))
        .child(radix)
        .child(
            el().key(format!("ila-probe-remove-{i}"))
                .role("button")
                .label(format!("Remove probe {}", p.name))
                .focusable(true)
                .w(18.0)
                .h(18.0)
                .center()
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    ila::act(s, "Cannot remove probe", |s| ila::remove_probe(s, i))
                })
                .child(text("×").font(VivadoTheme::FONT_UI).color(t.text_dim)),
        )
}

/// Probes panel: search, net list with `+`, configured probes, bus grouping.
fn probes_panel(s: &AppState, t: &VivadoTheme) -> El {
    let has_design = crate::live::live_design(s).is_some();
    let mut body = col()
        .key("ila-probes")
        .w(LEFT_W)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(section_head("ila-probes-head", "Probes", t));
    if !has_design {
        return body.child(
            col()
                .key("ila-no-design")
                .w_full()
                .padding(12.0)
                .gap(8.0)
                .child(
                    text("Probe nets come from the synthesized design.")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_dim)
                        .wrap(),
                )
                .child(
                    btn(
                        "ila-run-synth",
                        "▶ Run Synthesis",
                        t,
                        !s.runs.is_active(),
                        "A run is in progress",
                    )
                    .on_press(|s: &mut AppState| {
                        crate::runs::launch(s, crate::runs::RunKind::Synth)
                    }),
                ),
        );
    }
    let nets = ila::probe_nets(s);
    let shown = nets.len().min(ila::NET_LIST_MAX);
    let mut list = col()
        .key("ila-nets")
        .role("list")
        .w_full()
        .h(NET_LIST_H)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if nets.is_empty() {
        list = list.child(
            row()
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(caption("No nets match the search.", t)),
        );
    }
    for (k, net) in nets.iter().take(shown).enumerate() {
        let added = s.ila.config.probes.iter().any(|p| p.nets == [net.clone()]);
        let n = net.clone();
        let mut r = row()
            .key(format!("ila-net-{k}"))
            .w_full()
            .h(ROW_H)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .hover(t.selection)
            .child(
                el().grow(1.0).min_w(0.0).child(
                    text(net.clone())
                        .font(VivadoTheme::FONT_MONO)
                        .color(if added { t.text_faint } else { t.text })
                        .truncate()
                        .help_when_truncated(net.clone()),
                ),
            );
        r = if added {
            r.child(caption("added", t))
        } else {
            r.child(
                el().key(format!("ila-net-add-{k}"))
                    .role("button")
                    .label(format!("Add probe {net}"))
                    .focusable(true)
                    .w(18.0)
                    .h(18.0)
                    .center()
                    .border(1.0, t.border)
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| {
                        let n = n.clone();
                        ila::act(s, "Cannot add probe", move |s| ila::add_probe(s, &n))
                    })
                    .child(text("+").font(VivadoTheme::FONT_UI).color(t.text)),
            )
        };
        list = list.child(r);
    }
    if nets.len() > shown {
        list = list.child(
            row()
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(caption(
                    &format!("+{} more — refine the search", nets.len() - shown),
                    t,
                )),
        );
    }
    let search = row().w_full().h(28.0).px(8.0).items_center().child(
        text_input(&s.ila.search, VivadoTheme::FONT_UI)
            .single_line()
            .key("ila-search")
            .label("Search nets")
            .w_full()
            .h(20.0)
            .px(6.0)
            .fill(t.chrome_bg)
            .border(1.0, t.border_light)
            .radius(3.0)
            .color(t.text)
            .placeholder("Search nets", t.text_faint)
            .caret(t.text, t.selection)
            .on_edit(|s: &mut AppState| &mut s.ila.search),
    );
    let mut probes = col()
        .key("ila-probe-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if s.ila.config.probes.is_empty() {
        probes = probes.child(
            row()
                .key("ila-probes-empty")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .child(caption("Add nets above to probe them.", t)),
        );
    }
    for i in 0..s.ila.config.probes.len() {
        probes = probes.child(probe_row(s, t, i));
    }
    let checked = s.ila.group.iter().filter(|g| **g).count();
    let group_bar = row()
        .key("ila-group-bar")
        .w_full()
        .h(30.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            text_input(&s.ila.bus_name, VivadoTheme::FONT_MONO.mono())
                .single_line()
                .key("ila-bus-name")
                .label("Bus name")
                .grow(1.0)
                .min_w(0.0)
                .h(20.0)
                .px(6.0)
                .fill(t.chrome_bg)
                .border(1.0, t.border_light)
                .radius(3.0)
                .color(t.text)
                .placeholder("bus name", t.text_faint)
                .caret(t.text, t.selection)
                .on_edit(|s: &mut AppState| &mut s.ila.bus_name),
        )
        .child(
            btn(
                "ila-group-bus",
                "Group checked as bus",
                t,
                checked >= 2,
                "Check at least two probes",
            )
            .on_press(|s: &mut AppState| {
                ila::act(s, "Cannot group probes", ila::group_checked_as_bus)
            }),
        );
    body = body
        .child(search)
        .child(list)
        .child(section_head(
            "ila-configured-head",
            &format!("Configured probes ({})", s.ila.config.probes.len()),
            t,
        ))
        .child(probes)
        .child(group_bar);
    body
}

/// Trigger & capture settings: kind, value/mask, depth, pre %, cycles,
/// timescale, clock net.
fn trigger_panel(s: &AppState, t: &VivadoTheme) -> El {
    let choice = TrigChoice::of(&s.ila.config.trigger.kind);
    let mut kinds = Vec::new();
    for c in TrigChoice::ALL {
        kinds.push(
            segment(
                format!("ila-trig-{}", c.label().to_lowercase()),
                c.label(),
                c == choice,
                t,
            )
            .on_press(move |s: &mut AppState| {
                let line = ila::set_trigger_choice(s, c);
                ila::act(s, "Cannot set trigger", move |_| Ok(line));
            }),
        );
    }
    let is_value = choice == TrigChoice::Value;
    let mut row1 = row()
        .key("ila-trigger-row")
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .items_center()
        .child(el().w(64.0).child(caption("Trigger", t)))
        .child(segments("ila-trig-kind", "Trigger kind", kinds, t))
        .child(caption("on", t))
        .child(
            el().max_w(120.0).child(
                text(s.ila.trigger_probe_name())
                    .font(VivadoTheme::FONT_MONO)
                    .color(t.wave_marker)
                    .truncate()
                    .help_when_truncated(s.ila.trigger_probe_name()),
            ),
        );
    if is_value {
        row1 = row1
            .child(caption("Value", t))
            .child(field(
                "ila-trig-value",
                "Trigger value",
                &s.ila.trig_value,
                84.0,
                t,
                |s| &mut s.ila.trig_value,
            ))
            .child(caption("Mask", t))
            .child(field(
                "ila-trig-mask",
                "Trigger mask (empty = all bits)",
                &s.ila.trig_mask,
                84.0,
                t,
                |s| &mut s.ila.trig_mask,
            ));
    }
    let row2 = row()
        .key("ila-capture-row")
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .child(el().w(64.0).child(caption("Capture", t)))
        .child(caption("Depth", t))
        .child(field(
            "ila-depth",
            "Depth (samples)",
            &s.ila.depth,
            56.0,
            t,
            |s| &mut s.ila.depth,
        ))
        .child(caption("Pre %", t))
        .child(field(
            "ila-pre",
            "Pre-trigger percent",
            &s.ila.pre_pct,
            40.0,
            t,
            |s| &mut s.ila.pre_pct,
        ))
        .child(caption("Max cycles", t))
        .child(field(
            "ila-max-cycles",
            "Max cycles",
            &s.ila.max_cycles,
            64.0,
            t,
            |s| &mut s.ila.max_cycles,
        ))
        .child(caption("Timescale ps", t))
        .child(field(
            "ila-timescale",
            "Timescale (ps per cycle)",
            &s.ila.timescale,
            60.0,
            t,
            |s| &mut s.ila.timescale,
        ));
    let clocks = ila::clock_nets(s);
    let mut clock_items = Vec::new();
    let mut names: Vec<String> = clocks.into_iter().take(4).collect();
    if !names.contains(&s.ila.config.clock_net) {
        names.insert(0, s.ila.config.clock_net.clone());
    }
    for (k, c) in names.iter().enumerate() {
        let on = *c == s.ila.config.clock_net;
        let pick = c.clone();
        clock_items.push(segment(format!("ila-clock-{k}"), c, on, t).on_press(
            move |s: &mut AppState| {
                let verb = format!("ila clock {pick}");
                crate::tcl::eval_command(s, &verb);
            },
        ));
    }
    let row3 = row()
        .key("ila-clock-row")
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .items_center()
        .child(el().w(64.0).child(caption("Clock", t)))
        .child(segments("ila-clock", "Clock net", clock_items, t))
        .child(caption(
            "Enter applies a typed setting; Arm applies all.",
            t,
        ));
    col()
        .key("ila-trigger")
        .w_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(section_head("ila-trigger-head", "Trigger & capture", t))
        .child(row1)
        .child(row2)
        .child(row3)
}

/// Pixel x of fractional sample `i` in the current view.
fn x_of(s: &AppState, i: f64) -> f32 {
    s.ila.viewer.x_of(i, 0.0)
}

/// Visible integer sample range `[a, b)` (clamped to the capture).
fn visible_range(s: &AppState, n: usize) -> (usize, usize) {
    let (f, l) = s.ila.visible();
    let a = f.floor().max(0.0) as usize;
    let b = (l.ceil().max(0.0) as usize + 1).min(n);
    (a.min(n), b)
}

/// One probe lane: stepped scalar or bus envelope over the visible range.
fn lane_shape(s: &AppState, t: &VivadoTheme, cap: &IlaCapture, p: usize, w: f32) -> Shape {
    let n = cap.len();
    let (a, b) = visible_range(s, n);
    let hi = 4.0;
    let lo = LANE_H - 4.0;
    let mid = LANE_H / 2.0;
    let clampx = |x: f32| x.clamp(0.0, w);
    let at = |i: usize| cap.samples[p].get(i).copied().unwrap_or(0);
    let mut shape = Shape::new();
    if b <= a {
        return shape;
    }
    if !cap.probes[p].is_bus() {
        let lvl = |v: u64| if v & 1 == 0 { lo } else { hi };
        let mut pts = vec![[clampx(x_of(s, a as f64)), lvl(at(a))]];
        for i in a + 1..b {
            if at(i) != at(i - 1) {
                let x = clampx(x_of(s, i as f64));
                pts.push([x, lvl(at(i - 1))]);
                pts.push([x, lvl(at(i))]);
            }
        }
        pts.push([clampx(x_of(s, b as f64)), lvl(at(b - 1))]);
        shape = shape.stroke(pts, 1.5, t.wave_trace);
    } else {
        let mut top = Vec::new();
        let mut bot = Vec::new();
        let x0 = clampx(x_of(s, a as f64));
        top.push([x0, mid]);
        bot.push([x0, mid]);
        for i in a + 1..b {
            if at(i) != at(i - 1) {
                let x = clampx(x_of(s, i as f64));
                top.push([(x - 2.0).max(0.0), hi]);
                top.push([x, mid]);
                top.push([x + 2.0, hi]);
                bot.push([(x - 2.0).max(0.0), lo]);
                bot.push([x, mid]);
                bot.push([x + 2.0, lo]);
            }
        }
        let x1 = clampx(x_of(s, b as f64));
        top.insert(1, [x0 + 2.0, hi]);
        bot.insert(1, [x0 + 2.0, lo]);
        top.push([x1, hi]);
        bot.push([x1, lo]);
        shape = shape
            .stroke(top, 1.5, t.wave_trace)
            .stroke(bot, 1.5, t.wave_trace);
    }
    // Invalid (unknown) samples: a faint band over the span.
    if let Some(valid) = cap.valid.get(p) {
        for i in a..b {
            if !valid.get(i).copied().unwrap_or(true) {
                let x0 = clampx(x_of(s, i as f64));
                let x1 = clampx(x_of(s, i as f64 + 1.0));
                shape = shape.fill(
                    vec![[x0, hi], [x1, hi], [x1, lo], [x0, lo]],
                    t.fail.alpha(0.25),
                );
            }
        }
    }
    shape
}

/// Ruler + grid shape across the visible range.
fn ruler_shape(s: &AppState, t: &VivadoTheme, n: usize, w: f32) -> (Shape, Vec<(f32, usize)>) {
    let spp = s.ila.viewer.spp.max(1e-6);
    // Pick a tick step so labels sit ≥ 70px apart.
    let raw = (70.0 * spp).max(1.0);
    let mut step = 1usize;
    for cand in [
        1usize, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000, 50000,
    ] {
        step = cand;
        if cand as f64 >= raw {
            break;
        }
    }
    let (a, b) = visible_range(s, n);
    let mut shape = Shape::new().line(
        [0.0, RULER_H - 0.5],
        [w, RULER_H - 0.5],
        1.0,
        LineCap::Butt,
        t.border,
    );
    let mut labels = Vec::new();
    let mut i = a.div_ceil(step) * step;
    while i <= b.min(n) {
        let x = x_of(s, i as f64);
        if (0.0..=w).contains(&x) {
            shape = shape.line([x, 10.0], [x, RULER_H], 1.0, LineCap::Butt, t.text_faint);
            labels.push((x, i));
        }
        i += step;
    }
    (shape, labels)
}

/// Grid lines through the lanes at the ruler ticks.
fn grid_shape(t: &VivadoTheme, ticks: &[(f32, usize)], h: f32) -> Shape {
    let mut shape = Shape::new();
    for (x, _) in ticks {
        shape = shape.line([*x, 0.0], [*x, h], 1.0, LineCap::Butt, t.wave_grid);
    }
    shape
}

fn time_label(ps: u64) -> String {
    if ps >= 1_000_000 && ps.is_multiple_of(1_000_000) {
        format!("{} us", ps / 1_000_000)
    } else if ps.is_multiple_of(1000) {
        format!("{} ns", ps / 1000)
    } else {
        format!("{ps} ps")
    }
}

/// Viewer toolbar: zoom, pan, cursors, trigger-at-cursor, readout.
fn viewer_toolbar(s: &AppState, t: &VivadoTheme, cap: &IlaCapture) -> El {
    let n = cap.len();
    let v = &s.ila.viewer;
    let cursor = v.cursor;
    let mut readout = match cursor {
        Some(c) => format!("Cursor {c} · {}", time_label(cap.time_ps(c))),
        None => "Click the traces to place the cursor".into(),
    };
    if let Some(a) = v.cursor_a {
        readout.push_str(&format!("   A {a}"));
    }
    if let Some(b) = v.cursor_b {
        readout.push_str(&format!("   B {b}"));
    }
    if let Some((_, _, d)) = v.delta_samples() {
        readout.push_str(&format!(
            "   Δ {d} samples ({})",
            time_label(d as u64 * cap.timescale_ps.max(1))
        ));
    }
    let has_cursor = cursor.is_some();
    let no_cursor = "Place the cursor on the traces first";
    row()
        .key("ila-wave-toolbar")
        .w_full()
        .h(TOOL_H)
        .px(4.0)
        .gap(3.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            btn("ila-zoom-out", "−", t, true, "")
                .help("Zoom out")
                .on_press(move |s: &mut AppState| {
                    s.ila.viewer.zoom_center(1.5, n);
                }),
        )
        .child(
            btn("ila-zoom-in", "+", t, true, "")
                .help("Zoom in")
                .on_press(move |s: &mut AppState| {
                    s.ila.viewer.zoom_center(1.0 / 1.5, n);
                }),
        )
        .child(
            btn("ila-zoom-fit", "Fit", t, true, "")
                .help("Fit the whole capture")
                .on_press(|s: &mut AppState| s.ila.zoom_fit()),
        )
        .child(
            btn("ila-pan-left", "◀", t, true, "")
                .help("Pan left")
                .on_press(move |s: &mut AppState| {
                    let d = -(s.ila.viewer.view_w as f64 * s.ila.viewer.spp) / 3.0;
                    s.ila.viewer.pan_samples(d, n);
                }),
        )
        .child(
            btn("ila-pan-right", "▶", t, true, "")
                .help("Pan right")
                .on_press(move |s: &mut AppState| {
                    let d = (s.ila.viewer.view_w as f64 * s.ila.viewer.spp) / 3.0;
                    s.ila.viewer.pan_samples(d, n);
                }),
        )
        .child(
            btn("ila-cursor-prev", "‹", t, has_cursor, no_cursor).on_press(|s: &mut AppState| {
                if let Some(c) = s.ila.viewer.cursor {
                    s.ila.viewer.cursor = Some(c.saturating_sub(1));
                }
            }),
        )
        .child(
            btn("ila-cursor-next", "›", t, has_cursor, no_cursor).on_press(
                move |s: &mut AppState| {
                    if let Some(c) = s.ila.viewer.cursor {
                        s.ila.viewer.cursor = Some((c + 1).min(n.saturating_sub(1)));
                    }
                },
            ),
        )
        .child(
            btn("ila-set-a", "Set A", t, has_cursor, no_cursor).on_press(|s: &mut AppState| {
                s.ila.viewer.cursor_a = s.ila.viewer.cursor;
            }),
        )
        .child(
            btn("ila-set-b", "Set B", t, has_cursor, no_cursor).on_press(|s: &mut AppState| {
                s.ila.viewer.cursor_b = s.ila.viewer.cursor;
            }),
        )
        .child(
            btn("ila-trigger-here", "Trigger here", t, has_cursor, no_cursor)
                .help("Set the trigger from the edge or value at the cursor (selected probe first)")
                .on_press(|s: &mut AppState| {
                    let Some(c) = s.ila.viewer.cursor else { return };
                    match s.ila.selected_probe {
                        Some(p) => ila::act(s, "No trigger at the cursor", |s| {
                            ila::set_trigger_from_wave(s, p, c)
                        }),
                        None => {
                            ila::act(s, "No edge at the cursor", |s| ila::suggest_trigger(s, c))
                        }
                    }
                }),
        )
        .child(
            el().key("ila-readout").grow(1.0).min_w(0.0).px(8.0).child(
                text(readout.clone())
                    .font(VivadoTheme::FONT_MONO)
                    .color(t.text_dim)
                    .truncate()
                    .help_when_truncated(readout),
            ),
        )
}

/// Name/Value column for the capture lanes.
fn names_column(s: &AppState, t: &VivadoTheme, cap: &IlaCapture) -> El {
    let mut c = col()
        .key("ila-names")
        .w(NAMES_W)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(RULER_H)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(el().w(104.0).child(caption("Name", t)))
                .child(caption("Value", t)),
        );
    let at = s.ila.viewer.cursor.or(cap.trigger_sample).unwrap_or(0);
    for (p, probe) in cap.probes.iter().enumerate() {
        let selected = s.ila.selected_probe == Some(p);
        let radix = s
            .ila
            .config
            .probes
            .get(p)
            .filter(|cp| cp.nets == probe.nets)
            .map(|cp| cp.radix)
            .unwrap_or(probe.radix);
        let value = match (
            cap.value_at(p, at),
            cap.valid.get(p).and_then(|v| v.get(at)).copied(),
        ) {
            (Some(_), Some(false)) => "x".to_string(),
            (Some(v), _) => format_value(v, probe.width(), radix),
            (None, _) => "-".into(),
        };
        c = c.child(
            row()
                .key(format!("ila-name-{p}"))
                .role("option")
                .label(probe.name.clone())
                .focusable(true)
                .focus_selected(selected)
                .w_full()
                .h(LANE_H)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    s.ila.selected_probe = if s.ila.selected_probe == Some(p) {
                        None
                    } else {
                        Some(p)
                    };
                })
                .child(el().w(104.0).child(label(&probe.name, t)))
                .child(
                    el().key(format!("ila-value-{p}"))
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            text(value.clone())
                                .font(VivadoTheme::FONT_MONO)
                                .color(t.text)
                                .truncate()
                                .help_when_truncated(value),
                        ),
                ),
        );
    }
    c
}

/// Trace canvas: ruler, grid, lanes, trigger marker, cursors. Press/drag
/// places the cursor; the element reports its width to the viewer.
fn trace_canvas(s: &AppState, t: &VivadoTheme, cap: &IlaCapture) -> El {
    let n = cap.len();
    let w = s.ila.viewer.view_w.max(1.0);
    let lanes_h = cap.probes.len() as f32 * LANE_H;
    let (ruler, ticks) = ruler_shape(s, t, n, w);
    let mut ruler_el = el()
        .key("ila-ruler")
        .w_full()
        .h(RULER_H)
        .fill(t.panel_bg)
        .child(canvas(ruler).w_full().h_full());
    for (x, i) in &ticks {
        if *x + 40.0 <= w {
            ruler_el = ruler_el.child(
                text(i.to_string())
                    .font(FONT_PIN)
                    .color(t.text_dim)
                    .absolute(*x + 3.0, 1.0),
            );
        }
    }
    let mut sheet = el()
        .key("ila-lanes")
        .w_full()
        .h(lanes_h)
        .fill(t.panel_bg)
        .child(
            canvas(grid_shape(t, &ticks, lanes_h))
                .w_full()
                .h_full()
                .absolute(0.0, 0.0),
        );
    let (va, vb) = visible_range(s, n);
    for (p, probe) in cap.probes.iter().enumerate() {
        let selected = s.ila.selected_probe == Some(p);
        let mut lane = el()
            .key(format!("ila-lane-{p}"))
            .w_full()
            .h(LANE_H)
            .absolute(0.0, p as f32 * LANE_H)
            .fill(if selected {
                t.selection.alpha(0.6)
            } else {
                t.panel_bg.alpha(0.0)
            })
            .child(canvas(lane_shape(s, t, cap, p, w)).w_full().h_full());
        if probe.is_bus() {
            // Value labels on runs wide enough to hold them.
            let mut start = va;
            for i in va + 1..=vb {
                let edge = i == vb || cap.samples[p][i] != cap.samples[p][i - 1];
                if edge {
                    let x0 = x_of(s, start as f64).max(0.0);
                    let x1 = x_of(s, i as f64).min(w);
                    let v = cap.samples[p][start];
                    let txt = format_value(v, probe.width(), probe.radix);
                    if x1 - x0 >= 14.0 + txt.chars().count() as f32 * 6.5 {
                        lane = lane.child(
                            text(txt)
                                .font(FONT_PIN)
                                .color(t.text)
                                .absolute(x0 + 5.0, 6.0),
                        );
                    }
                    start = i;
                }
            }
        }
        sheet = sheet.child(lane);
    }
    let mut marks: Vec<(String, usize, f32, rune::Color)> = Vec::new();
    if let Some(tr) = cap.trigger_sample {
        marks.push(("ila-trigger-marker".into(), tr, 2.0, t.wave_marker));
    }
    if let Some(a) = s.ila.viewer.cursor_a {
        marks.push(("ila-cursor-a".into(), a, 1.0, t.wave_cursor.alpha(0.55)));
    }
    if let Some(b) = s.ila.viewer.cursor_b {
        marks.push(("ila-cursor-b".into(), b, 1.0, t.wave_cursor.alpha(0.55)));
    }
    if let Some(c) = s.ila.viewer.cursor {
        marks.push(("ila-cursor".into(), c, 2.0, t.wave_cursor));
    }
    for (key, i, thick, ink) in marks {
        let x = x_of(s, i as f64 + 0.5);
        if (0.0..=w).contains(&x) {
            sheet = sheet.child(
                el().key(key)
                    .role("separator")
                    .absolute(x - thick / 2.0, 0.0)
                    .size(thick, lanes_h)
                    .fill(ink),
            );
        }
    }
    if let Some(tr) = cap.trigger_sample {
        let x = x_of(s, tr as f64 + 0.5);
        if (0.0..=w - 10.0).contains(&x) {
            ruler_el = ruler_el.child(
                el().key("ila-trigger-flag")
                    .absolute(x - 6.0, 2.0)
                    .size(12.0, 14.0)
                    .center()
                    .fill(t.wave_marker)
                    .child(text("T").font(FONT_PIN).color(t.panel_bg)),
            );
        }
    }
    col()
        .key("ila-canvas")
        .role("img")
        .label("Capture waveform")
        .grow(1.0)
        .min_w(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .on_layout(|s: &mut AppState, r: [f32; 4]| s.ila.set_view_width(r[2]))
        .on_drag(move |s: &mut AppState, d: &mut rune::Drag, _tx: &mut Tx| {
            if matches!(d.phase, Phase::Down | Phase::Move) && d.y >= RULER_H {
                let i = s.ila.viewer.sample_at(d.x, 0.0).floor().max(0.0) as usize;
                s.ila.viewer.cursor = Some(i.min(n.saturating_sub(1)));
            }
        })
        .child(ruler_el)
        .child(sheet)
}

/// Capture viewer: toolbar plus names + canvas, or its empty state.
fn capture_viewer(s: &AppState, t: &VivadoTheme) -> El {
    let Some(cap) = s.ila.capture.as_ref().filter(|c| !c.is_empty()) else {
        let msg = if s.ila.is_capturing() {
            format!(
                "Capturing — waiting for the trigger ({} cycles so far).",
                s.ila.progress_cycles()
            )
        } else {
            "No capture yet — arm a capture to see the probed nets here.".into()
        };
        return el()
            .key("ila-viewer-empty")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(t.panel_bg)
            .border(1.0, t.border)
            .center()
            .child(text(msg).font(VivadoTheme::FONT_UI).color(t.text_dim));
    };
    let banner = if cap.found_trigger {
        None
    } else {
        Some(
            row()
                .key("ila-no-trigger")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .fill(t.header_bg)
                .child(
                    text(format!(
                        "Trigger not found in {} cycles — showing the last {} samples",
                        cap.cycles_run,
                        cap.len()
                    ))
                    .font(VivadoTheme::FONT_UI)
                    .color(t.warn),
                ),
        )
    };
    let mut c = col()
        .key("ila-viewer")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(viewer_toolbar(s, t, cap));
    if let Some(b) = banner {
        c = c.child(b);
    }
    c.child(
        row()
            .key("ila-viewer-body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .items_start()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(names_column(s, t, cap))
            .child(trace_canvas(s, t, cap)),
    )
}

/// Logic Analyzer center surface.
pub fn ila_pane(s: &AppState, t: &VivadoTheme) -> El {
    let _ = IlaTriggerKind::Immediate;
    col()
        .key("ila")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Logic Analyzer - hw_ila_1", "ila-pane", t))
        .child(header(s, t))
        .child(status_strip(s, t))
        .child(
            row()
                .key("ila-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(probes_panel(s, t))
                .child(
                    col()
                        .key("ila-right")
                        .grow(1.0)
                        .h_full()
                        .min_w(0.0)
                        .min_h(0.0)
                        .gap(6.0)
                        .child(trigger_panel(s, t))
                        .child(capture_viewer(s, t)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CenterPane;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, app_spec().size, env)
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn ila_state(capture: bool) -> AppState {
        let mut s = init();
        s.center = CenterPane::Ila;
        ila::seed_ila_fixture(&mut s).expect("fixture");
        if capture {
            ila::capture_now(&mut s).expect("capture");
        }
        s
    }

    #[test]
    fn no_design_offers_synthesis_and_disables_arm() {
        let mut s = init();
        s.center = CenterPane::Ila;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "ila",
            "ila-toolbar",
            "ila-arm",
            "ila-strip",
            "ila-no-design",
            "ila-run-synth",
            "ila-viewer-empty",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        tap(&mut rt, "ila-arm");
        assert!(!rt.state.ila.is_capturing(), "disabled Arm does nothing");
    }

    #[test]
    fn setup_lists_nets_probes_and_trigger_controls() {
        let mut rt = harness_with(ila_state(false));
        rt.frame(0.0);
        for key in [
            "ila-search",
            "ila-nets",
            "ila-net-0",
            "ila-probe-0",
            "ila-probe-trig-0",
            "ila-probe-remove-0",
            "ila-group-bus",
            "ila-trig-rising",
            "ila-depth",
            "ila-pre",
            "ila-max-cycles",
            "ila-timescale",
            "ila-clock",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        tap(&mut rt, "ila-trig-falling");
        assert_eq!(rt.state.ila.config.trigger.kind, IlaTriggerKind::Falling);
        tap(&mut rt, "ila-probe-group-1");
        tap(&mut rt, "ila-probe-group-2");
        tap(&mut rt, "ila-group-bus");
        assert!(
            rt.state.ila.config.probes.iter().any(|p| p.is_bus()),
            "grouping builds a bus"
        );
    }

    #[test]
    fn captured_view_shows_lanes_trigger_and_cursor_readout() {
        let mut rt = harness_with(ila_state(true));
        rt.frame(0.0);
        for key in [
            "ila-viewer",
            "ila-wave-toolbar",
            "ila-names",
            "ila-name-0",
            "ila-value-0",
            "ila-canvas",
            "ila-ruler",
            "ila-lane-0",
            "ila-trigger-marker",
            "ila-cursor",
            "ila-readout",
            "ila-export",
        ] {
            assert!(rt.find(key).is_some(), "missing {key}");
        }
        let c0 = rt.state.ila.viewer.cursor;
        tap(&mut rt, "ila-cursor-next");
        assert_eq!(rt.state.ila.viewer.cursor, c0.map(|c| c + 1));
        tap(&mut rt, "ila-set-a");
        tap(&mut rt, "ila-cursor-prev");
        tap(&mut rt, "ila-set-b");
        assert!(rt.state.ila.viewer.delta_samples().is_some());
        let spp = rt.state.ila.viewer.spp;
        tap(&mut rt, "ila-zoom-in");
        assert!(
            rt.state.ila.viewer.spp < spp,
            "zoom in shrinks samples per pixel"
        );
        tap(&mut rt, "ila-zoom-fit");
    }

    #[test]
    fn canvas_reports_its_width_and_refits_the_capture() {
        let mut rt = harness_with(ila_state(true));
        for i in 0..4 {
            rt.frame(i as f64 / 60.0);
        }
        let v = &rt.state.ila.viewer;
        assert!(
            v.view_w > 50.0,
            "canvas width reaches the viewer: {}",
            v.view_w
        );
        assert!(!rt.state.ila.fit_pending, "the first layout refits");
        let n = rt.state.ila.capture.as_ref().unwrap().len() as f64;
        assert!(
            (v.spp * v.view_w as f64 - n).abs() < 1.0,
            "fit shows the whole capture: spp={} w={}",
            v.spp,
            v.view_w
        );
    }

    #[test]
    fn radix_segments_reformat_bus_values() {
        let mut s = ila_state(false);
        s.ila.group[1] = true;
        s.ila.group[2] = true;
        ila::group_checked_as_bus(&mut s).unwrap();
        ila::capture_now(&mut s).unwrap();
        let bus = s.ila.config.probes.len() - 1;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, &format!("ila-probe-radix-{bus}-dec"));
        assert_eq!(rt.state.ila.config.probes[bus].radix, IlaRadix::Dec);
    }
}
