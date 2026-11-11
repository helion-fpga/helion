//! Device I/O Ports planning table: one row per top-level port with its
//! placed IOB site, Package Pin combo (real `set_property LOC`), electrical
//! pin constraints, and quick-set rows that run the real `set_property`
//! verb on the selected port. Split out of [`crate::views::device`].

use rune::{El, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::live::slug;
use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::device::DeviceSel;

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    crate::views::cad::ui(content, t)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    crate::views::cad::dim(content, t)
}

/// One I/O Planning row: a top-level port, its placed IOB site (once
/// implementation runs), and its `set_property` pin constraints (old
/// `IoPortView`; the Package Pin cell edits LOC through a popover).
#[derive(Clone, Debug)]
pub struct IoPortRow {
    pub name: String,
    pub dir: &'static str,
    pub site: Option<String>,
    pub iob_cell: Option<String>,
    pub package_pin: Option<String>,
    pub iostandard: Option<String>,
    pub drive: Option<String>,
    pub slew: Option<String>,
    pub pulltype: Option<String>,
    pub diff_term: Option<String>,
    pub in_term: Option<String>,
}

impl IoPortRow {
    fn cell(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("-")
    }
}

/// I/O port rows from the live design: top-level ports, the placed IOB
/// site on each port's net, and the port's pin-constraint attrs. Empty
/// until a design is synthesized.
///
/// The site match finds the placed IOB-kind cell on the port's net (old
/// `refresh_io_ports` matched packed-cell names by substring, with a
/// first-IOB fallback for outputs that could attribute a wrong site;
/// unmapped ports honestly show `-`).
pub fn io_port_rows(s: &AppState) -> Vec<IoPortRow> {
    let Some(design) = crate::live::live_design(s) else {
        return Vec::new();
    };
    let nets = crate::live::live_netlist(s)
        .map(|n| n.nets)
        .unwrap_or_default();
    let placed = crate::live::live_placed(s)
        .map(|p| p.cells)
        .unwrap_or_default();
    design
        .ports
        .iter()
        .map(|p| {
            let name = helion_ir::resolve(p.name).to_string();
            let dir = match p.dir {
                helion_ir::PortDir::In => "IN",
                helion_ir::PortDir::Out => "OUT",
                helion_ir::PortDir::Inout => "INOUT",
            };
            let on_net: &[String] = nets
                .iter()
                .find(|n| n.name == name)
                .map(|n| n.cells.as_slice())
                .unwrap_or(&[]);
            let iob = placed
                .iter()
                .find(|c| c.site_kind == "IOB" && on_net.iter().any(|n| n == &c.cell));
            let attr = |k: &str| p.attrs.get(k).map(str::to_string);
            IoPortRow {
                name,
                dir,
                site: iob.map(|c| c.site.clone()),
                iob_cell: iob.map(|c| c.cell.clone()),
                package_pin: attr("LOC"),
                iostandard: attr("IOSTANDARD"),
                drive: attr("DRIVE"),
                slew: attr("SLEW"),
                pulltype: attr("PULLTYPE"),
                diff_term: attr("DIFF_TERM"),
                in_term: attr("IN_TERM"),
            }
        })
        .collect()
}

/// I/O Ports planning table under the die canvas (old Package tab's port
/// grid, read-only): name/dir/site/pin + pin constraints. Row taps
/// cross-probe the die (placed IOB cell, else the port net).
pub(crate) fn io_ports_table(s: &AppState, t: &VivadoTheme) -> El {
    let rows = io_port_rows(s);
    let mut body = col().w_full();
    body = body.child(
        row()
            .key("device-io-head")
            .w_full()
            .items_center()
            .px(8.0)
            .child(el().grow(1.0).min_w(0.0).child(dim_label("Name", t)))
            .child(el().w(44.0).child(dim_label("Dir", t)))
            .child(el().w(96.0).child(dim_label("Site", t)))
            .child(el().w(128.0).child(dim_label("Package Pin", t)))
            .child(el().w(96.0).child(dim_label("IOSTANDARD", t)))
            .child(el().w(56.0).child(dim_label("Drive", t)))
            .child(el().w(56.0).child(dim_label("Slew", t)))
            .child(el().w(72.0).child(dim_label("Pull Type", t)))
            .child(el().w(64.0).child(dim_label("Diff Term", t)))
            .child(el().w(96.0).child(dim_label("In Term", t))),
    );
    if rows.is_empty() {
        body = body.child(
            dim_label("Synth a design to list ports.", t)
                .key("device-io-empty")
                .px(8.0),
        );
    }
    for (i, r) in rows.iter().enumerate() {
        let sel = match (&r.iob_cell, s.device.selected.as_ref()) {
            (Some(cell), Some(DeviceSel::Cell(n))) => n == cell,
            (None, Some(DeviceSel::Net(n))) => n == &r.name,
            _ => false,
        };
        let pick = match r.iob_cell.clone() {
            Some(cell) => DeviceSel::Cell(cell),
            None => DeviceSel::Net(r.name.clone()),
        };
        let mut line = row()
            .key(format!("device-io-row-{i}"))
            .role("row")
            .w_full()
            .h(22.0)
            .items_center()
            .px(8.0)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                s.device.selected = Some(pick.clone());
            })
            .child(
                el().grow(1.0)
                    .min_w(0.0)
                    .child(ui_label(&r.name, t).truncate()),
            )
            .child(el().w(44.0).child(ui_label(r.dir, t)))
            .child(el().w(96.0).child(mono_cell(IoPortRow::cell(&r.site), t)))
            .child(
                el().w(128.0)
                    .child(pin_trigger(&r.name, IoPortRow::cell(&r.package_pin), t)),
            )
            .child(
                el().w(96.0)
                    .child(mono_cell(IoPortRow::cell(&r.iostandard), t)),
            )
            .child(el().w(56.0).child(mono_cell(IoPortRow::cell(&r.drive), t)))
            .child(el().w(56.0).child(mono_cell(IoPortRow::cell(&r.slew), t)))
            .child(
                el().w(72.0)
                    .child(mono_cell(IoPortRow::cell(&r.pulltype), t)),
            )
            .child(
                el().w(64.0)
                    .child(mono_cell(IoPortRow::cell(&r.diff_term), t)),
            )
            .child(
                el().w(96.0)
                    .child(mono_cell(IoPortRow::cell(&r.in_term), t)),
            );
        if sel {
            line = line.fill(t.selection);
        }
        body = body.child(line);
    }
    col()
        .key("device-io-ports")
        .w_full()
        .max_h(190.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .items_center()
                .gap(8.0)
                .px(8.0)
                .py(4.0)
                .child(ui_label("I/O Ports", t))
                .child(dim_label(&format!("{} ports", rows.len()), t)),
        )
        .child(
            col()
                .w_full()
                .max_h(156.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        )
}

fn mono_cell(content: &str, t: &VivadoTheme) -> El {
    text(content)
        .font(VivadoTheme::FONT_MONO.mono())
        .color(t.text)
        .truncate()
}

/// Package Pin options: every IOB die site `(id, label)`, annotated with
/// the owning port when one is assigned (old combo's `{pin} ({port})`).
fn pin_options(s: &AppState) -> Vec<(String, String)> {
    let (dev, _) = crate::live::live_device(s);
    let rows = io_port_rows(s);
    dev.iob_sites()
        .map(|site| {
            let id = site.id();
            let owner = rows.iter().find_map(|r| {
                if r.package_pin.as_deref() == Some(id.as_str())
                    || r.site.as_deref() == Some(id.as_str())
                {
                    Some(r.name.clone())
                } else {
                    None
                }
            });
            let label = owner
                .map(|port| format!("{id} ({port})"))
                .unwrap_or_else(|| id.clone());
            (id, label)
        })
        .collect()
}

/// Apply a Package Pin pick: the same `set_property LOC` the old combo
/// execs, then close the popover.
fn choose_pin(s: &mut AppState, port: &str, site: &str) {
    crate::tcl::eval_command(s, &format!("set_property LOC {site} {port}"));
    s.device.io_pin_pop = None;
    s.device.selected = Some(DeviceSel::Net(port.to_string()));
}

/// Package Pin combo trigger cell (built identically in the table and in
/// [`push_device_pops`]; the popover anchors to its key).
fn pin_trigger(port: &str, current: &str, t: &VivadoTheme) -> El {
    let name = port.to_string();
    el().key(format!("device-io-pin-{}", slug(port)))
        .role("button")
        .label(format!("Package pin for {port}, current {current}"))
        .focusable(true)
        .w_full()
        .h(22.0)
        .px(6.0)
        .center_y()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .radius(4.0)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            s.device.io_pin_pop = if s.device.io_pin_pop.as_deref() == Some(&name) {
                None
            } else {
                Some(name.clone())
            };
        })
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .gap(4.0)
                .child(mono_cell(current, t).grow(1.0).min_w(0.0))
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 12.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
}

/// Push the open Package Pin popover (called with the overlay layer from
/// the root view; the trigger cell lives in [`io_ports_table`]).
pub fn push_device_pops(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    let Some(port) = s.device.io_pin_pop.clone() else {
        return;
    };
    let kit = t.kit();
    let options = pin_options(s);
    let current = io_port_rows(s)
        .iter()
        .find(|r| r.name == port)
        .and_then(|r| r.package_pin.clone())
        .unwrap_or_else(|| "- set site -".to_string());
    let mut opts = col().w_full().gap(0.0);
    for (j, (id, label)) in options.iter().enumerate() {
        let site = id.clone();
        let name = port.clone();
        let mut opt_row = el()
            .key(format!("device-io-pin-opt-{j}"))
            .role("button")
            .label(format!("Choose {label}"))
            .focusable(true)
            .w_full()
            .h(24.0)
            .px(8.0)
            .center_y()
            .hover(t.selection)
            .on_press(move |s: &mut AppState| choose_pin(s, &name, &site))
            .child(mono_cell(label, t));
        if label == &current || id == &current {
            opt_row = opt_row.fill(t.selection);
        }
        opts = opts.child(opt_row);
    }
    // Anchor to the trigger already mounted in the table. A second trigger
    // built here would never mount (its keyed anchor wrapper is dropped),
    // leaving the list unanchored at the window corner.
    let pop = format!("device-io-pin-pop-{}", slug(&port));
    let anchor = format!("device-io-pin-{}", slug(&port));
    let content = col()
        .key(format!("{pop}-content"))
        .role("dialog")
        .label(format!("Package pin for {port}"))
        .focus_return(&anchor)
        .min_w(220.0)
        .max_w(320.0)
        .child(
            col()
                .key("body")
                .padding(4.0)
                .shrink()
                .min_h(0.0)
                .child(rune_kit::popover::fit_scroll(opts)),
        );
    layer.push(rune_kit::scrim(
        format!("{pop}-scrim"),
        |s: &mut AppState| s.device.io_pin_pop = None,
    ));
    layer.push(rune_kit::popover::anchored(
        &kit,
        rune_kit::popover::raised(&kit, content, kit.radius.md),
        &anchor,
        rune_kit::Placement::BottomStart,
        4.0,
    ));
}
#[cfg(test)]
mod tests {
    use super::*;
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

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn io_port_rows_empty_without_design() {
        let s = init();
        assert!(io_port_rows(&s).is_empty(), "no design means no port rows");
    }

    #[test]
    fn io_port_rows_map_ports_sites_and_props() {
        let mut s = device_state();
        let rows = io_port_rows(&s);
        assert_eq!(rows.len(), 4, "fixture has 4 top ports: {rows:?}");
        assert_eq!(rows[0].name, "clk");
        assert_eq!(rows[0].dir, "IN");
        assert_eq!(rows[3].name, "divclk");
        assert_eq!(rows[3].dir, "OUT");
        for r in &rows {
            if let Some(site) = r.site.as_deref() {
                assert!(
                    site.starts_with("IOB_"),
                    "placed ports map to IOB sites: {r:?}"
                );
                assert!(r.iob_cell.is_some(), "sites keep the IOB cell: {r:?}");
            }
            assert_eq!(r.package_pin, None, "no LOC yet: {r:?}");
        }
        assert!(
            rows.iter().any(|r| r.site.is_some()),
            "outputs must map to placed IOBs: {rows:?}"
        );
        let seg = rows.iter().find(|r| r.name == "seg").unwrap();
        assert_eq!(seg.site.as_deref(), Some("IOB_X2Y0"));
        assert_eq!(seg.iob_cell.as_deref(), Some("u_iob"));
        let div = rows.iter().find(|r| r.name == "divclk").unwrap();
        assert_eq!(div.site.as_deref(), Some("IOB_X3Y0"));
        assert_eq!(div.iob_cell.as_deref(), Some("u_div_u_iob"));
        for name in ["clk", "rst"] {
            let r = rows.iter().find(|r| r.name == name).unwrap();
            assert_eq!(r.site, None, "unmapped inputs stay honest: {r:?}");
        }
        s.runs
            .live_design
            .as_mut()
            .unwrap()
            .set_loc("clk", "IOB_X0Y0")
            .unwrap();
        s.runs
            .live_design
            .as_mut()
            .unwrap()
            .set_iostandard("clk", "LVCMOS33")
            .unwrap();
        let rows = io_port_rows(&s);
        assert_eq!(rows[0].package_pin.as_deref(), Some("IOB_X0Y0"));
        assert_eq!(rows[0].iostandard.as_deref(), Some("LVCMOS33"));
    }

    #[test]
    fn io_row_tap_cross_probes_the_placed_iob() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        assert!(rt.find("device-io-ports").is_some());
        assert!(rt.find("device-io-head").is_some());
        assert!(rt.find("device-io-row-3").is_some());
        tap(&mut rt, "device-io-row-3");
        assert!(
            matches!(rt.state.device.selected, Some(DeviceSel::Cell(_))),
            "tap must select the placed IOB cell: {:?}",
            rt.state.device.selected
        );
    }

    #[test]
    fn pin_popover_anchors_to_its_table_trigger() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-io-pin-clk");
        for _ in 0..8 {
            rt.frame(1.0 / 60.0);
        }
        let (trigger, _) = rt.presented("device-io-pin-clk").expect("trigger rect");
        let (content, _) = rt
            .presented("device-io-pin-pop-clk-content")
            .expect("content rect");
        // The I/O table sits near the window bottom, so the long site list
        // flips above its trigger; either fitted side must stay adjacent.
        let below = content[1] >= trigger[1] + trigger[3] - 1.0;
        let above = content[1] + content[3] <= trigger[1] + 1.0;
        assert!(
            below || above,
            "content must hug its trigger: trigger {trigger:?} content {content:?}"
        );
        assert!(
            (content[0] - trigger[0]).abs() < 2.0,
            "content must align to the trigger start: trigger {trigger:?} content {content:?}"
        );
    }

    #[test]
    fn pin_combo_pick_sets_loc_and_shows_it() {
        let mut rt = harness_with(device_state());
        rt.frame(0.0);
        tap(&mut rt, "device-io-pin-clk");
        assert_eq!(rt.state.device.io_pin_pop.as_deref(), Some("clk"));
        assert!(
            rt.find("device-io-pin-opt-0").is_some(),
            "pin options must pop over"
        );
        tap(&mut rt, "device-io-pin-opt-0");
        assert_eq!(rt.state.device.io_pin_pop, None, "pick closes the pop");
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Net("clk".into())),
            "a pin pick cross-probes the assigned port"
        );
        let rows = io_port_rows(&rt.state);
        let clk = rows.iter().find(|r| r.name == "clk").unwrap();
        assert!(
            clk.package_pin
                .as_deref()
                .is_some_and(|p| p.starts_with("IOB_")),
            "pick must set LOC on the port: {clk:?}"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("set_property LOC IOB_")),
            "pick must echo its Tcl verb"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("set_property PACKAGE_PIN IOB_")
                    && l.text.contains(" replaced=")),
            "PACKAGE_PIN must report its placement outcome, not repeat the verb: {:?}",
            rt.state
                .console
                .lines
                .iter()
                .map(|l| &l.text)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn io_table_is_honestly_empty_without_design() {
        let mut s = init();
        s.center = crate::state::CenterPane::Device;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("device-io-empty").is_some());
        assert!(rt.find("device-io-row-0").is_none());
    }}
