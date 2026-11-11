//! Device Package view: the old HAD IOB package grid (bank colors, live
//! port occupancy, pin/port cross-probe selection), split out of
//! [`crate::views::device`] so the die canvas module stays small.
//!
//! Pins come from the real HAD model (`Device::iob_sites`) and the live
//! I/O rows ([`crate::views::device_io::io_port_rows`]); a tap runs the real
//! `set_property PACKAGE_PIN` / `select_package_pin` verbs.

use rune::{El, col, el, row, text};

use crate::live::slug;
use crate::state::{AppState, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::device::DeviceSel;
use crate::views::device_io::io_port_rows;
use crate::views::sources::pane_header;

/// Canvas label type: smaller than CAD body text.
const FONT_PIN: rune::tok::TextStyle = crate::views::cad::FONT_PIN;

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    crate::views::cad::dim(content, t)
}

/// Package pin key: one stable button per HAD site id.
pub(crate) fn package_pin_key(pin: &str) -> String {
    format!("device-package-pin-{}", slug(pin))
}

/// One HAD IOB package pin: engine site plus the owning top-level port, if
/// any. Derived per frame from the live device and I/O rows (old
/// `PackagePin`, refreshed by `refresh_package`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagePin {
    pub pin: String,
    pub x: u32,
    pub y: u32,
    pub port: Option<String>,
    pub bank: u32,
}

/// HAD IOB bounding box behind the package grid (old `PackageDrawing`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageGrid {
    pub part: String,
    pub x0: u32,
    pub y0: u32,
    pub cols: u32,
    pub rows: u32,
}

/// Package pins plus their bounding box: every HAD IOB site, with placed
/// sites first and LOC constraints filling unplaced ports (old
/// `refresh_package` uses each port's site, else its package pin). Banks
/// group eight consecutive IOBs along the grid's long axis.
pub fn package_pins(s: &AppState) -> (PackageGrid, Vec<PackagePin>) {
    let (dev, part) = crate::live::live_device(s);
    let assigned: Vec<(String, String)> = io_port_rows(s)
        .into_iter()
        .filter_map(|r| r.site.or(r.package_pin).map(|site| (site, r.name)))
        .collect();
    let mut pins: Vec<PackagePin> = dev
        .iob_sites()
        .map(|site| {
            let pin = site.id();
            let port = assigned
                .iter()
                .find(|(site, _)| site == &pin)
                .map(|(_, name)| name.clone());
            PackagePin {
                pin,
                x: site.x,
                y: site.y,
                port,
                bank: 0,
            }
        })
        .collect();
    let (x0, y0, cols, rows) = match (
        pins.iter().map(|p| p.x).min(),
        pins.iter().map(|p| p.x).max(),
        pins.iter().map(|p| p.y).min(),
        pins.iter().map(|p| p.y).max(),
    ) {
        (Some(xmin), Some(xmax), Some(ymin), Some(ymax)) => {
            (xmin, ymin, xmax - xmin + 1, ymax - ymin + 1)
        }
        _ => (0, 0, 0, 0),
    };
    const BANK_PINS: u32 = 8;
    for p in &mut pins {
        let dx = p.x.saturating_sub(x0);
        let dy = p.y.saturating_sub(y0);
        p.bank = if cols >= rows {
            dx / BANK_PINS
        } else {
            dy / BANK_PINS
        };
    }
    (
        PackageGrid {
            part,
            x0,
            y0,
            cols,
            rows,
        },
        pins,
    )
}

/// One package pin by exact engine id.
pub fn package_pin(s: &AppState, pin: &str) -> Option<PackagePin> {
    package_pins(s).1.into_iter().find(|p| p.pin == pin)
}

/// Package drawing dump: HAD bounding box plus the occupancy map, exactly
/// like old `package_drawing_text` (including its `L`/`P`/`.` marks).
pub fn package_grid_text(s: &AppState) -> String {
    let (grid, pins) = package_pins(s);
    let assigned = pins.iter().filter(|p| p.port.is_some()).count();
    let banks = {
        let mut seen = std::collections::BTreeSet::new();
        for p in &pins {
            seen.insert(p.bank);
        }
        seen.len()
    };
    let mut out = format!(
        "package drawing part={} cols={} rows={} pins={} assigned={} x0={} y0={} banks={}",
        grid.part,
        grid.cols,
        grid.rows,
        pins.len(),
        assigned,
        grid.x0,
        grid.y0,
        banks
    );
    if grid.cols == 0 || grid.rows == 0 {
        return out;
    }
    for dy in 0..grid.rows {
        let y = grid.y0 + dy;
        let mut row = format!(" row y={y}");
        let mut map = String::new();
        for dx in 0..grid.cols {
            let x = grid.x0 + dx;
            if let Some(p) = pins.iter().find(|p| p.x == x && p.y == y) {
                row.push_str(&format!(
                    " {}={}:bank={}",
                    p.pin,
                    p.port.as_deref().unwrap_or("-"),
                    p.bank
                ));
                map.push(match p.port.as_deref() {
                    Some("led") => 'L',
                    Some(_) => 'P',
                    None => '.',
                });
            } else {
                map.push(' ');
            }
        }
        out.push_str(&format!("{row} map={map}"));
    }
    out
}

/// Bank ink from theme tokens (no hardcoded package palette).
fn package_bank_color(t: &VivadoTheme, bank: u32) -> rune::Color {
    match bank % 4 {
        0 => t.die_region,
        1 => t.link,
        2 => t.pass,
        _ => t.warn,
    }
}
/// Package pin cell size: site id plus owner, dense enough to wrap the
/// HAD strip into a filled grid.
const PACKAGE_PIN_W: f32 = 104.0;
const PACKAGE_PIN_H: f32 = 64.0;
const PACKAGE_BALL: f32 = 18.0;

/// Package pin tap: an empty pin binds the selected I/O port through real
/// `set_property PACKAGE_PIN`; any other tap routes through real
/// `select_package_pin` (old paint's click branch).
fn package_pick(s: &mut AppState, pin: &str) {
    let Some(found) = package_pin(s, pin) else {
        return;
    };
    if found.port.is_none()
        && let Some(DeviceSel::Net(port)) = s.device.selected.clone()
        && io_port_rows(s).iter().any(|r| r.name == port)
    {
        let before = s.console.lines.len();
        crate::tcl::eval_command(
            s,
            &format!("set_property PACKAGE_PIN {pin} [get_ports {port}]"),
        );
        let failed = s.console.lines[before..]
            .iter()
            .any(|l| l.sev == MsgSev::Error);
        if !failed {
            s.device.selected = Some(DeviceSel::Net(port));
        }
        return;
    }
    crate::tcl::eval_command(s, &format!("select_package_pin {pin}"));
}

/// One tappable package ball: bank wash, assigned ink, selected ring, site
/// id plus owner (no hover-only state).
fn package_pin_cell(s: &AppState, t: &VivadoTheme, pin: &PackagePin) -> El {
    let assigned = pin.port.is_some();
    let selected = match &s.device.selected {
        Some(DeviceSel::Pin(n)) => n == &pin.pin,
        Some(DeviceSel::Net(n)) => pin.port.as_deref() == Some(n.as_str()),
        _ => false,
    };
    let owned = pin.pin.clone();
    let status = pin.port.clone().unwrap_or_else(|| "Unassigned".into());
    el().key(package_pin_key(&pin.pin))
        .role("button")
        .label(format!("{} ({}, BANK{})", pin.pin, status, pin.bank))
        .description(format!("X{}Y{}", pin.x, pin.y))
        .focus_selected(selected)
        .focusable(true)
        .size(PACKAGE_PIN_W, PACKAGE_PIN_H)
        .fill(package_bank_color(t, pin.bank).alpha(0.16))
        .border(
            if selected { 2.0 } else { 1.0 },
            if selected { t.warn } else { t.border },
        )
        .radius(8.0)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| package_pick(s, &owned))
        .child(
            col()
                .w_full()
                .h_full()
                .center()
                .gap(2.0)
                .child(
                    el().size(PACKAGE_BALL, PACKAGE_BALL)
                        .radius(PACKAGE_BALL / 2.0)
                        .fill(if assigned { t.link } else { t.text_faint })
                        .a11y_hidden(),
                )
                .child(
                    text(&pin.pin)
                        .font(FONT_PIN)
                        .color(t.text)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(
                    text(&status)
                        .font(FONT_PIN)
                        .color(if assigned { t.text } else { t.text_dim })
                        .truncate()
                        .a11y_hidden(),
                ),
        )
}

/// Package header: part/grid/pin counts plus the live assignment target.
fn package_summary(s: &AppState, t: &VivadoTheme, grid: &PackageGrid, pins: &[PackagePin]) -> El {
    let assigned = pins.iter().filter(|p| p.port.is_some()).count();
    let mut head = col().key("device-package-summary").w_full().gap(2.0).child(
        row()
            .w_full()
            .items_center()
            .gap(8.0)
            .px(8.0)
            .py(4.0)
            .child(dim_label(
                &format!(
                    "Part {} · {}×{} grid · {} pins ({} assigned)",
                    grid.part,
                    grid.cols,
                    grid.rows,
                    pins.len(),
                    assigned
                ),
                t,
            )),
    );
    if let Some(DeviceSel::Net(port)) = &s.device.selected
        && io_port_rows(s).iter().any(|r| &r.name == port)
    {
        head = head.child(
            dim_label(&format!("Assigning {port} — tap an empty pin"), t)
                .key("device-package-assigning")
                .px(8.0),
        );
    }
    head
}

/// Package legend: occupancy inks plus every bank present on this part.
fn package_legend(t: &VivadoTheme, pins: &[PackagePin]) -> El {
    let mut legend = row()
        .key("device-package-legend")
        .w_full()
        .items_center()
        .gap(8.0)
        .px(8.0)
        .py(2.0);
    for (label, ink) in [("Unassigned", t.text_faint), ("Assigned port", t.link)] {
        legend = legend.child(
            row()
                .items_center()
                .gap(4.0)
                .child(el().size(10.0, 10.0).radius(5.0).fill(ink).a11y_hidden())
                .child(dim_label(label, t)),
        );
    }
    let mut banks = std::collections::BTreeSet::new();
    for p in pins {
        banks.insert(p.bank);
    }
    for bank in banks {
        legend = legend.child(
            row()
                .key(format!("device-package-legend-bank-{bank}"))
                .items_center()
                .gap(4.0)
                .child(
                    el().size(12.0, 12.0)
                        .radius(2.0)
                        .fill(package_bank_color(t, bank))
                        .a11y_hidden(),
                )
                .child(dim_label(&format!("BANK{bank}"), t)),
        );
    }
    legend
}

/// Package view: old HAD IOB drawing as tappable bank-colored balls over a
/// scroll viewport. Empty only if the part exposes no IOB sites.
pub(crate) fn package_pane(s: &AppState, t: &VivadoTheme) -> El {
    let (grid, pins) = package_pins(s);
    let pane = col()
        .key("device-package")
        .role("application")
        .label("Package pin grid")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Package", "device-package-pane", t))
        .child(package_summary(s, t, &grid, &pins))
        .child(package_legend(t, &pins));
    if pins.is_empty() {
        return pane.child(
            dim_label("This part exposes no IOB sites.", t)
                .key("device-package-empty")
                .px(8.0),
        );
    }
    let mut balls = row()
        .key("device-package-grid")
        .w_full()
        .flex_wrap()
        .gap(6.0)
        .padding(8.0);
    for pin in &pins {
        balls = balls.child(package_pin_cell(s, t, pin));
    }
    pane.child(
        col()
            .w_full()
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(balls),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::device::DeviceView;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn device_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        s
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
    fn package_pins_cover_every_had_iob_with_bank_occupancy() {
        let mut s = device_state();
        let (dev, part) = crate::live::live_device(&s);
        let (grid, pins) = package_pins(&s);
        let n = dev.iob_sites().count();
        assert!(n > 1, "HAD must list real IOB sites");
        assert_eq!(pins.len(), n, "drawing pins are HAD IOB sites");
        assert_eq!(grid.part, part);
        assert_eq!(grid.cols * grid.rows, n as u32, "cells are the IOB box");
        for p in &pins {
            assert!(
                dev.iob_major(p.x, p.y).is_some(),
                "{} is not a HAD IOB site",
                p.pin
            );
            assert_eq!(p.pin, format!("IOB_X{}Y{}", p.x, p.y));
            assert_eq!(p.bank, p.x.saturating_sub(grid.x0) / 8, "8 pins/bank");
        }
        let owner = |pin: &str| {
            pins.iter()
                .find(|p| p.pin == pin)
                .and_then(|p| p.port.clone())
        };
        assert_eq!(owner("IOB_X2Y0").as_deref(), Some("seg"));
        assert_eq!(owner("IOB_X3Y0").as_deref(), Some("divclk"));
        let free = pins
            .iter()
            .find(|p| p.port.is_none())
            .expect("fixture leaves unassigned pins")
            .pin
            .clone();
        crate::tcl::eval_command(&mut s, &format!("set_property LOC {free} rst"));
        let pins = package_pins(&s).1;
        assert_eq!(
            pins.iter()
                .find(|p| p.pin == free)
                .and_then(|p| p.port.as_deref()),
            Some("rst"),
            "LOC constraints join the occupancy map"
        );
        assert!(
            s.console.lines.iter().any(|l| l.text
                == format!("set_property PACKAGE_PIN {free} rst loc={free} replaced=1 rerouted=0")),
            "loc= must name this port's site/constraint: {:?}",
            s.console.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        let dump = package_grid_text(&s);
        assert!(dump.contains(&format!("pins={n} assigned=3")), "{dump}");
        assert!(dump.contains(&format!("{free}=rst")), "{dump}");
        assert!(dump.contains("map="), "{dump}");
    }

    #[test]
    fn package_view_toggle_mounts_grid_and_selects_ports() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        assert!(rt.find("device-canvas").is_some());
        assert!(rt.find("device-package").is_none());
        tap(&mut rt, "device-view-package");
        assert_eq!(rt.state.device.view, DeviceView::Package);
        for key in [
            "device-package",
            "device-package-summary",
            "device-package-legend",
            "device-package-legend-bank-0",
            "device-package-grid",
            "device-package-pin-iob-x2y0",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert!(rt.find("device-canvas").is_none(), "package replaces die");
        assert!(rt.find("device-layers").is_none(), "no die layers here");
        assert!(
            rt.find("device-zoom-label").is_none(),
            "package needs no die zoom"
        );
        tap(&mut rt, "device-package-pin-iob-x2y0");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Net("seg".into())),
            "assigned pins cross-select the port"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.contains("package_pin IOB_X2Y0 port=seg")),
            "pin taps route through select_package_pin: {:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
        tap(&mut rt, "device-view-die");
        assert_eq!(rt.state.device.view, DeviceView::Die);
        assert!(rt.find("device-canvas").is_some());
        assert!(rt.find("device-zoom-label").is_some());
        assert!(rt.find("device-package").is_none());
    }

    #[test]
    fn package_empty_pin_assigns_the_selected_port() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-view-package");
        tap(&mut rt, "device-io-row-0");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Net("clk".into())),
            "row tap chooses the assignment target"
        );
        assert!(rt.find("device-package-assigning").is_some());
        let free = package_pins(&rt.state)
            .1
            .into_iter()
            .find(|p| p.port.is_none())
            .expect("fixture leaves unassigned pins")
            .pin;
        let key = package_pin_key(&free);
        tap(&mut rt, &key);
        assert_eq!(
            package_pin(&rt.state, &free).and_then(|p| p.port),
            Some("clk".to_string()),
            "empty package taps bind the selected port: pin={free} key={key} selected={:?} console={:?}",
            rt.state.device.selected,
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
                .any(|l| l.text == format!("set_property PACKAGE_PIN {free} [get_ports clk]")),
            "assignment must use the real verb: {:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
    }}
