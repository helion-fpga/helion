//! I/O quick-set rows (old I/O Planning `io_prop_button_row`s,
//! `helion-ide.rs:5690-5715`): one row per pin property with the old value
//! presets. Each chip runs the real `set_property KEY VALUE [get_ports P]`
//! verb on the selected port; values the HAD model rejects for the port's
//! current IOSTANDARD (`Device::drive_legal_for_iostandard`,
//! `diff_term_legal_for_iostandard`, `in_term_legal_for_iostandard`)
//! render disabled with the reason.

use helion_device::Device;
use rune::{El, col, el, row, text};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::device_io::IoPortRow;

/// Property, presets (value, chip label), and the row's current value.
type Preset = (&'static str, &'static [(&'static str, &'static str)]);

/// Old preset table, in old row order.
pub const PRESETS: [Preset; 6] = [
    (
        "IOSTANDARD",
        &[
            ("LVCMOS18", "LVCMOS18"),
            ("LVCMOS33", "LVCMOS33"),
            ("LVCMOS12", "LVCMOS12"),
            ("SSTL15", "SSTL15"),
        ],
    ),
    (
        "DRIVE",
        &[("4", "4"), ("8", "8"), ("12", "12"), ("16", "16")],
    ),
    ("SLEW", &[("SLOW", "SLOW"), ("FAST", "FAST")]),
    (
        "PULLTYPE",
        &[
            ("NONE", "NONE"),
            ("PULLUP", "PULLUP"),
            ("PULLDOWN", "PULLDOWN"),
            ("KEEPER", "KEEPER"),
        ],
    ),
    ("DIFF_TERM", &[("FALSE", "FALSE"), ("TRUE", "TRUE")]),
    (
        "IN_TERM",
        &[
            ("NONE", "NONE"),
            ("UNTUNED_SPLIT_40", "40"),
            ("UNTUNED_SPLIT_50", "50"),
            ("UNTUNED_SPLIT_60", "60"),
        ],
    ),
];

fn current<'a>(r: &'a IoPortRow, key: &str) -> Option<&'a str> {
    match key {
        "IOSTANDARD" => r.iostandard.as_deref(),
        "DRIVE" => r.drive.as_deref(),
        "SLEW" => r.slew.as_deref(),
        "PULLTYPE" => r.pulltype.as_deref(),
        "DIFF_TERM" => r.diff_term.as_deref(),
        "IN_TERM" => r.in_term.as_deref(),
        _ => None,
    }
}

/// Why `key = value` is illegal for this port right now (`None` = legal).
pub fn illegal(r: &IoPortRow, key: &str, value: &str) -> Option<&'static str> {
    let std = r.iostandard.as_deref();
    match key {
        "DRIVE" => {
            let ma = value.parse().unwrap_or(0);
            (!Device::drive_legal_for_iostandard(std, ma))
                .then_some("Drive not legal for this IOSTANDARD")
        }
        "DIFF_TERM" => (!Device::diff_term_legal_for_iostandard(std, value))
            .then_some("DIFF_TERM TRUE needs an SSTL/HSTL standard"),
        "IN_TERM" => (!Device::in_term_legal_for_iostandard(std, value))
            .then_some("IN_TERM needs an SSTL/HSTL standard"),
        _ => None,
    }
}

fn chip_key(key: &str, value: &str) -> String {
    format!(
        "device-io-set-{}-{}",
        key.to_ascii_lowercase().replace('_', "-"),
        value.to_ascii_lowercase().replace('_', "-")
    )
}

fn chip(
    r: &IoPortRow,
    key: &'static str,
    value: &'static str,
    label: &'static str,
    t: &VivadoTheme,
) -> El {
    let on = current(r, key).is_some_and(|c| c.eq_ignore_ascii_case(value));
    let why = illegal(r, key, value);
    let port = r.name.clone();
    let mut b = el()
        .key(chip_key(key, value))
        .role("button")
        .label(format!("Set {key} {value} on {}", r.name))
        .focus_selected(on)
        .h(20.0)
        .px(6.0)
        .center()
        .radius(3.0)
        .fill(if on { t.selection } else { t.panel_bg })
        .border(1.0, if on { t.accent } else { t.border })
        .child(text(label).font(cad::mono_style()).color(t.text));
    match why {
        None => {
            b = b
                .focusable(true)
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    crate::tcl::eval_command(
                        s,
                        &format!("set_property {key} {value} [get_ports {port}]"),
                    );
                })
        }
        Some(reason) => {
            b = b
                .disabled(true)
                .description(reason)
                .help(reason)
                .layer_opacity(0.4)
        }
    }
    if label != value {
        b = b.help(value);
    }
    b
}

/// Quick-set block for the selected port: two columns of property rows.
pub fn quickset(s: &AppState, t: &VivadoTheme, rows: &[IoPortRow]) -> El {
    let sel = s
        .device
        .io_selected
        .as_deref()
        .and_then(|p| rows.iter().find(|r| r.name == p));
    let head = row()
        .w_full()
        .h(cad::HEAD_H)
        .items_center()
        .gap(8.0)
        .px(8.0)
        .fill(t.header_bg)
        .child(cad::caps("Quick Set", t))
        .child(match sel {
            Some(r) => cad::mono(r.name.clone(), t).key("device-io-quick-port"),
            None => cad::dim("Select a port row to set its pin properties", t),
        });
    let mut body = col().key("device-io-quick").w_full().child(head);
    let Some(r) = sel else {
        return body;
    };
    let mut grid = row().w_full().gap(16.0).px(8.0).py(4.0).items_start();
    for half in PRESETS.chunks(3) {
        let mut c = col().gap(4.0);
        for (key, values) in half {
            let mut line = row()
                .gap(4.0)
                .items_center()
                .child(el().w(78.0).child(cad::dim(*key, t)));
            for (value, label) in values.iter() {
                line = line.child(chip(r, key, value, label, t));
            }
            c = c.child(line);
        }
        grid = grid.child(c);
    }
    body = body.child(grid);
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness() -> Runtime<AppState> {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(s, crate::view, app_spec().size, env)
    }

    thread_local!(static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) });

    /// Advance the test clock so layout springs settle between presses.
    fn tick(rt: &mut Runtime<AppState>) {
        let t = CLOCK.with(|c| {
            c.set(c.get() + 0.5);
            c.get()
        });
        rt.frame(t);
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        tick(rt);
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        tick(rt);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        tick(rt);
    }

    #[test]
    fn quickset_needs_a_port_then_runs_set_property() {
        let mut rt = harness();
        rt.frame(0.0);
        assert!(rt.find("device-io-quick").is_some());
        assert!(
            rt.find("device-io-set-iostandard-lvcmos33").is_none(),
            "no port selected"
        );
        tap(&mut rt, "device-io-row-2");
        let port = rt
            .state
            .device
            .io_selected
            .clone()
            .expect("row picks the port");
        tap(&mut rt, "device-io-set-iostandard-lvcmos33");
        tap(&mut rt, "device-io-set-drive-12");
        tap(&mut rt, "device-io-set-slew-fast");
        let row = crate::views::device_io::io_port_rows(&rt.state)
            .into_iter()
            .find(|r| r.name == port)
            .unwrap();
        assert_eq!(row.iostandard.as_deref(), Some("LVCMOS33"));
        assert_eq!(row.drive.as_deref(), Some("12"));
        assert_eq!(
            row.slew.as_deref(),
            Some("FAST"),
            "{:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == format!("set_property IOSTANDARD LVCMOS33 [get_ports {port}]"))
        );
    }

    #[test]
    fn termination_presets_follow_had_legality() {
        let mut r = IoPortRow {
            name: "p".into(),
            dir: "IN",
            site: None,
            iob_cell: None,
            package_pin: None,
            iostandard: Some("LVCMOS33".into()),
            drive: None,
            slew: None,
            pulltype: None,
            diff_term: None,
            in_term: None,
        };
        assert!(illegal(&r, "DIFF_TERM", "TRUE").is_some());
        assert!(illegal(&r, "IN_TERM", "UNTUNED_SPLIT_50").is_some());
        assert!(illegal(&r, "IN_TERM", "NONE").is_none());
        r.iostandard = Some("SSTL15".into());
        assert!(illegal(&r, "DIFF_TERM", "TRUE").is_none());
        assert!(illegal(&r, "IN_TERM", "UNTUNED_SPLIT_50").is_none());
        r.iostandard = Some("LVCMOS12".into());
        assert!(illegal(&r, "DRIVE", "12").is_some());
        assert!(illegal(&r, "DRIVE", "8").is_none());
    }
}
