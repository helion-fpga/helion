//! Find Results: search the live netlist for cells, ports, and nets, then
//! cross-probe the picked object into the Device and Schematic views.
//!
//! Ports the old IDE's Find Results pane (`helion-ide.rs:5340 paint_find`,
//! model `ide.rs:11818 sheet_find`, `:17125 find_rows`, `select_find`) onto
//! [`crate::live::LiveNetlist`]: every row is a real engine object of the
//! last run (or the Tcl elaboration). Nothing is listed before a design
//! exists. The table is virtualized so large netlists stay cheap.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el/virtual_rows.rs`: `virtual_rows` + `GrowthSpec`.
//! - `crates/rune/src/el/text.rs`: `text_input` + `.on_edit`.

use std::collections::HashMap;
use std::rc::Rc;

use rune::{El, GrowthSpec, ScrollSpec, col, el, row};

use crate::live::{LiveNetlist, live_netlist};
use crate::state::{AppState, CenterPane, ConsoleLine, MsgSev};
use crate::tcl::SelObj;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::device::DeviceSel;
use crate::views::schematic::SchemSel;
use crate::views::sources::pane_header;

const COLS: [ColSpec; 5] = [
    ColSpec::grow("Name", 1.6),
    ColSpec::fixed("Type", 44.0),
    ColSpec::fixed("Primitive", 92.0),
    ColSpec::grow("Parent", 1.0),
    ColSpec::fixed("Objects", 76.0),
];

/// Object class of a hit. Typed, never a bare string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindKind {
    Cell,
    Port,
    Net,
}

impl FindKind {
    pub fn label(self) -> &'static str {
        match self {
            FindKind::Cell => "cell",
            FindKind::Port => "port",
            FindKind::Net => "net",
        }
    }

    pub fn parse(spec: &str) -> Option<Self> {
        match spec.trim().to_ascii_lowercase().as_str() {
            "" | "cells" | "cell" => Some(FindKind::Cell),
            "ports" | "port" | "io" | "i/o" | "io_ports" | "i/o_ports" => Some(FindKind::Port),
            "nets" | "net" => Some(FindKind::Net),
            _ => None,
        }
    }
}

/// One Find Results row (old `FindHit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindHit {
    pub kind: FindKind,
    pub name: String,
    /// LUT / FDRE / IN / OUT / `fanout=N`.
    pub primitive: String,
    /// Owning instance, else the top design.
    pub parent: String,
    /// Connected objects (`3 nets`, `4 cells`).
    pub objects: String,
}

impl FindHit {
    pub fn row_text(&self) -> String {
        format!(
            "NAME={} TYPE={} PRIMITIVE={} PARENT={} OBJECTS={}",
            self.name,
            self.kind.label(),
            self.primitive,
            self.parent,
            self.objects
        )
    }
}

/// Find Results state: query, scope toggles, selected hit.
#[derive(Clone, Debug)]
pub struct FindState {
    pub query: rune::TextEdit,
    pub cells: bool,
    pub ports: bool,
    pub nets: bool,
    pub selected: Option<(FindKind, String)>,
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            query: rune::TextEdit::new(""),
            cells: true,
            ports: true,
            nets: true,
            selected: None,
        }
    }
}

impl FindState {
    fn scope_on(&self, kind: FindKind) -> bool {
        match kind {
            FindKind::Cell => self.cells,
            FindKind::Port => self.ports,
            FindKind::Net => self.nets,
        }
    }

    fn toggle(&mut self, kind: FindKind) {
        match kind {
            FindKind::Cell => self.cells = !self.cells,
            FindKind::Port => self.ports = !self.ports,
            FindKind::Net => self.nets = !self.nets,
        }
    }

    /// Exactly one scope on (old `sheet_find cells|ports|nets`).
    fn only(&mut self, kind: FindKind) {
        self.cells = kind == FindKind::Cell;
        self.ports = kind == FindKind::Port;
        self.nets = kind == FindKind::Net;
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Hits over the live netlist for the current query and scopes, in stable
/// order (cells, ports, nets; each sorted by name).
pub fn hits(state: &FindState, live: &LiveNetlist) -> Vec<FindHit> {
    let q = state.query.text.trim().to_ascii_lowercase();
    let matches = |name: &str| q.is_empty() || name.to_ascii_lowercase().contains(&q);
    let top = if live.design.is_empty() {
        "-".to_string()
    } else {
        live.design.clone()
    };
    let parent = |name: &str| live.owner_of(name).unwrap_or(top.as_str()).to_string();
    let mut nets_of: HashMap<&str, usize> = HashMap::new();
    for n in &live.nets {
        for c in &n.cells {
            *nets_of.entry(c.as_str()).or_insert(0) += 1;
        }
    }
    let mut out = Vec::new();
    if state.cells {
        for c in live.cells.iter().filter(|c| matches(&c.name)) {
            out.push(FindHit {
                kind: FindKind::Cell,
                name: c.name.clone(),
                primitive: c.kind.label().to_string(),
                parent: parent(&c.name),
                objects: plural(
                    nets_of.get(c.name.as_str()).copied().unwrap_or(0),
                    "net",
                    "nets",
                ),
            });
        }
    }
    if state.ports {
        let dirs = live
            .ports_in
            .iter()
            .map(|p| (p, "IN"))
            .chain(live.ports_out.iter().map(|p| (p, "OUT")));
        for (p, dir) in dirs.filter(|(p, _)| matches(p)) {
            let on_net = usize::from(live.net(p).is_some());
            out.push(FindHit {
                kind: FindKind::Port,
                name: p.clone(),
                primitive: dir.into(),
                parent: top.clone(),
                objects: plural(on_net, "net", "nets"),
            });
        }
        // Keep ports sorted by name across directions.
        let start = out
            .iter()
            .position(|h| h.kind == FindKind::Port)
            .unwrap_or(out.len());
        out[start..].sort_by(|a, b| a.name.cmp(&b.name));
    }
    if state.nets {
        for n in live.nets.iter().filter(|n| matches(&n.name)) {
            out.push(FindHit {
                kind: FindKind::Net,
                name: n.name.clone(),
                primitive: format!("fanout={}", n.loads.saturating_sub(1)),
                parent: parent(&n.name),
                objects: plural(n.cells.len(), "cell", "cells"),
            });
        }
    }
    out
}

fn echo(s: &mut AppState, text: String) {
    s.console.lines.push(ConsoleLine {
        text,
        sev: MsgSev::Info,
    });
}

/// Select a hit and cross-probe it: cells select on the Device die and the
/// Schematic, nets on the Device (net spans), ports on the Schematic; the
/// Tcl selection follows so `get_selected_objects` agrees.
pub fn select_hit(s: &mut AppState, hit: &FindHit) {
    s.find.selected = Some((hit.kind, hit.name.clone()));
    match hit.kind {
        FindKind::Cell => {
            s.device.selected = Some(DeviceSel::Cell(hit.name.clone()));
            s.schematic.selected = Some(SchemSel::Cell(hit.name.clone()));
        }
        FindKind::Net => {
            s.device.selected = Some(DeviceSel::Net(hit.name.clone()));
        }
        FindKind::Port => {
            s.schematic.selected = Some(SchemSel::Port(hit.name.clone()));
        }
    }
    s.tcl.selected = vec![SelObj {
        kind: hit.kind.label().into(),
        name: hit.name.clone(),
        highlighted: false,
    }];
    let getter = match hit.kind {
        FindKind::Cell => "get_cells",
        FindKind::Port => "get_ports",
        FindKind::Net => "get_nets",
    };
    echo(s, format!("select_objects [{getter} {{{}}}]", hit.name));
}

/// Results table dump (old `find_results_text`).
pub fn results_text(rows: &[FindHit]) -> String {
    let mut out = format!("find_results n={}", rows.len());
    if rows.is_empty() {
        out.push_str(" no hits");
    }
    for h in rows {
        out.push('\n');
        out.push_str(&h.row_text());
    }
    out
}

/// Tcl `find ?query?`: set the query (all scopes) and open the pane.
pub fn find(s: &mut AppState, query: &str) -> Result<String, String> {
    s.navigate(CenterPane::Find);
    let live = live_netlist(s).ok_or("find: no netlist (run Synthesis first)")?;
    s.find.query = rune::TextEdit::new(query.trim());
    let rows = hits(&s.find, &live);
    Ok(results_text(&rows))
}

/// Tcl `sheet_find cells|ports|nets`: one scope, no query.
pub fn sheet_find(s: &mut AppState, kind: &str) -> Result<String, String> {
    let k = FindKind::parse(kind).ok_or_else(|| format!("sheet_find: unknown {}", kind.trim()))?;
    s.navigate(CenterPane::Find);
    let live = live_netlist(s).ok_or("sheet_find: no netlist (run Synthesis first)")?;
    s.find.query = rune::TextEdit::new("");
    s.find.only(k);
    let rows = hits(&s.find, &live);
    if rows.is_empty() {
        return Err(format!("sheet_find {}: 0 hits", k.label()));
    }
    Ok(format!(
        "sheet_find {} hits={}\n{}",
        k.label(),
        rows.len(),
        results_text(&rows)
    ))
}

/// Tcl `select_find name|index|kind:name`: select and cross-probe.
pub fn select_find(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("select_find: missing name".into());
    }
    let live = live_netlist(s).ok_or("select_find: no netlist (run Synthesis first)")?;
    let rows = hits(&s.find, &live);
    if rows.is_empty() {
        return Err("select_find: no hits".into());
    }
    let hit = if let Ok(i) = spec.parse::<usize>() {
        rows.get(i)
            .cloned()
            .ok_or_else(|| format!("select_find: no row {spec}"))?
    } else {
        rows.iter()
            .find(|h| {
                h.name.eq_ignore_ascii_case(spec)
                    || format!("{}:{}", h.kind.label(), h.name).eq_ignore_ascii_case(spec)
            })
            .cloned()
            .ok_or_else(|| format!("select_find: no row {spec}"))?
    };
    select_hit(s, &hit);
    s.navigate(CenterPane::Find);
    Ok(hit.row_text())
}

fn scope_toggle(kind: FindKind, label: &str, n: usize, s: &AppState, t: &VivadoTheme) -> El {
    let on = s.find.scope_on(kind);
    el().key(format!("find-scope-{}", kind.label()))
        .role("checkbox")
        .label(format!("Search {label}"))
        .focus_selected(on)
        .focusable(true)
        .h(cad::BTN_H)
        .px(8.0)
        .center()
        .radius(3.0)
        .fill(if on { t.selection } else { t.panel_bg })
        .border(1.0, if on { t.accent } else { t.border })
        .hover(t.selection)
        .on_press(move |s: &mut AppState| s.find.toggle(kind))
        .child(
            row()
                .items_center()
                .gap(6.0)
                .child(cad::ui(label, t))
                .child(cad::mono(cad::grouped(n), t).color(t.text_dim)),
        )
}

fn hit_row(t: &VivadoTheme, hits: &[FindHit], s: &AppState, i: usize) -> El {
    let Some(h) = hits.get(i) else {
        return el().h(cad::ROW_H);
    };
    let selected = s.find.selected.as_ref() == Some(&(h.kind, h.name.clone()));
    let hit = h.clone();
    let kind_ink = match h.kind {
        FindKind::Cell => t.text,
        FindKind::Port => t.link,
        FindKind::Net => t.pass,
    };
    cad::table_row(
        format!("find-row-{i}"),
        format!("{} {}", h.kind.label(), h.name),
        selected,
        t,
    )
    .on_press(move |s: &mut AppState| select_hit(s, &hit))
    .child(cad::cell(
        &COLS[0],
        cad::cut(h.name.clone(), cad::mono_style(), t.text),
    ))
    .child(cad::cell(
        &COLS[1],
        cad::cut(
            h.kind.label(),
            rune::tok::TextStyle::new(11.0, 500.0),
            kind_ink,
        ),
    ))
    .child(cad::cell(
        &COLS[2],
        cad::cut(h.primitive.clone(), cad::mono_style(), t.text),
    ))
    .child(cad::cell(
        &COLS[3],
        cad::cut(h.parent.clone(), cad::mono_style(), t.text_dim),
    ))
    .child(cad::cell(
        &COLS[4],
        cad::cut(h.objects.clone(), VivadoTheme::FONT_UI, t.text_dim),
    ))
}

fn detail(s: &AppState, rows: &[FindHit], t: &VivadoTheme) -> El {
    let strip = row()
        .key("find-detail")
        .w_full()
        .h(30.0)
        .items_center()
        .gap(10.0)
        .px(10.0)
        .radius(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    let hit = s
        .find
        .selected
        .as_ref()
        .and_then(|(k, n)| rows.iter().find(|h| h.kind == *k && &h.name == n));
    let Some(h) = hit else {
        return strip.child(cad::dim(
            "Select a row to cross-probe it in Device and Schematic.",
            t,
        ));
    };
    let mut strip = strip
        .child(
            cad::title(h.name.clone(), t)
                .truncate()
                .min_w(0.0)
                .key("find-detail-name"),
        )
        .child(
            cad::dim(
                format!("{} · {} · {}", h.kind.label(), h.primitive, h.parent),
                t,
            )
            .truncate()
            .min_w(0.0),
        )
        .child(el().grow(1.0));
    if matches!(h.kind, FindKind::Cell | FindKind::Net) {
        strip = strip.child(cad::link(
            "find-show-device",
            "Show in Device",
            t,
            |s: &mut AppState| s.navigate(CenterPane::Device),
        ));
    }
    if matches!(h.kind, FindKind::Cell | FindKind::Port) {
        strip = strip.child(cad::link(
            "find-show-schematic",
            "Show in Schematic",
            t,
            |s: &mut AppState| s.navigate(CenterPane::Schematic),
        ));
    }
    strip
}

/// The center Find Results pane wired into the shell.
pub fn find_pane(s: &AppState, t: &VivadoTheme) -> El {
    let live = live_netlist(s);
    let shell = col()
        .key("find")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Find Results", "find", t));
    let Some(live) = live else {
        let cta = match s.runs.active_kind() {
            None => Some(cad::primary_btn(
                "find-run-synth",
                "Run Synthesis",
                t,
                None,
                |s: &mut AppState| crate::runs::launch(s, crate::runs::RunKind::Synth),
            )),
            Some(_) => None,
        };
        return shell.child(col().w_full().padding(8.0).child(cad::empty_state(
            "find-empty",
            "No netlist yet. Run Synthesis to search cells, ports, and nets.",
            cta,
            t,
        )));
    };
    let rows = Rc::new(hits(&s.find, &live));
    let all = FindState {
        query: s.find.query.clone(),
        cells: true,
        ports: true,
        nets: true,
        selected: None,
    };
    let counts = hits(&all, &live);
    let n_of = |k: FindKind| counts.iter().filter(|h| h.kind == k).count();
    let tools = cad::toolbar("find-toolbar", t)
        .child(
            cad::search_input(
                "find-query",
                &s.find.query,
                "Find cells, ports, or nets",
                t,
                |s: &mut AppState| &mut s.find.query,
            )
            .w(260.0),
        )
        .child(scope_toggle(
            FindKind::Cell,
            "Cells",
            n_of(FindKind::Cell),
            s,
            t,
        ))
        .child(scope_toggle(
            FindKind::Port,
            "Ports",
            n_of(FindKind::Port),
            s,
            t,
        ))
        .child(scope_toggle(
            FindKind::Net,
            "Nets",
            n_of(FindKind::Net),
            s,
            t,
        ))
        .child(el().grow(1.0))
        .child(
            cad::dim(
                format!("{} in {}", plural(rows.len(), "hit", "hits"), live.design),
                t,
            )
            .truncate()
            .key("find-count"),
        );
    let body = if rows.is_empty() {
        let q = s.find.query.text.trim();
        let sentence = if !(s.find.cells || s.find.ports || s.find.nets) {
            "Turn on Cells, Ports, or Nets to search.".to_string()
        } else if q.is_empty() {
            "No objects of the selected types in this design.".to_string()
        } else {
            format!("No matches for “{q}”.")
        };
        cad::empty_state("find-nomatch", &sentence, None, t)
    } else {
        let theme = *t;
        let count = rows.len();
        let key_rows = rows.clone();
        let build_rows = rows.clone();
        let list = col()
            .key("find-rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                count,
                cad::ROW_H,
                move |_: &AppState, i| {
                    let _ = &key_rows;
                    format!("find-row-{i}").into()
                },
                move |s: &AppState, i| hit_row(&theme, &build_rows, s, i),
            )
            .virtual_rows_version(
                rows.iter()
                    .map(|h| (h.kind, h.name.as_str()))
                    .collect::<Vec<_>>(),
            );
        cad::table_frame("find-table", t)
            .grow(1.0)
            .min_h(0.0)
            .child(cad::header("find-head", &COLS, t))
            .child(
                col()
                    .key("find-viewport")
                    .w_full()
                    .grow(1.0)
                    .min_h(0.0)
                    .scroll(ScrollSpec {
                        stick_bottom: false,
                    })
                    .child(list),
            )
    };
    shell.child(tools).child(
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .gap(8.0)
            .padding(8.0)
            .child(body)
            .child(detail(s, &rows, t)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        Runtime::new(
            state,
            crate::view,
            spec.size,
            Env {
                system_dark: false,
                scale: 2.0,
            },
        )
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn seeded() -> AppState {
        let mut s = init();
        s.center = CenterPane::Find;
        crate::live::seed_live_fixture(&mut s).expect("live fixture");
        s
    }

    #[test]
    fn honest_empty_state_before_any_design() {
        let mut s = init();
        s.center = CenterPane::Find;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("find-empty").is_some());
        assert!(rt.find("find-run-synth").is_some());
        assert!(rt.find("find-table").is_none());
        assert!(find(&mut rt.state, "clk").is_err());
    }

    #[test]
    fn hits_cover_every_live_object_with_real_columns() {
        let s = seeded();
        let live = live_netlist(&s).unwrap();
        let rows = hits(&s.find, &live);
        let n = live.cells.len() + live.ports_in.len() + live.ports_out.len() + live.nets.len();
        assert_eq!(rows.len(), n, "every live object is a row");
        let clk_net = rows
            .iter()
            .find(|h| h.kind == FindKind::Net && h.name == "clk")
            .expect("clk net");
        assert!(
            clk_net.primitive.starts_with("fanout="),
            "{}",
            clk_net.primitive
        );
        let port = rows
            .iter()
            .find(|h| h.kind == FindKind::Port && h.name == "rst")
            .unwrap();
        assert_eq!(port.primitive, "IN");
        assert_eq!(port.parent, "counter_top");
        let owned = rows
            .iter()
            .find(|h| h.kind == FindKind::Cell && h.name.starts_with("u_div"))
            .expect("u_div-owned cell");
        assert_eq!(owned.parent, "u_div");
    }

    #[test]
    fn query_and_scopes_filter_and_rows_cross_probe() {
        let mut rt = harness_with(seeded());
        rt.frame(0.0);
        for key in [
            "find-toolbar",
            "find-query",
            "find-scope-cell",
            "find-head",
            "find-row-0",
            "find-detail",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        tap(&mut rt, "find-scope-net");
        tap(&mut rt, "find-scope-port");
        assert!(!rt.state.find.nets && !rt.state.find.ports);
        let live = live_netlist(&rt.state).unwrap();
        assert!(
            hits(&rt.state.find, &live)
                .iter()
                .all(|h| h.kind == FindKind::Cell)
        );
        tap(&mut rt, "find-row-0");
        let (kind, name) = rt.state.find.selected.clone().expect("selected");
        assert_eq!(kind, FindKind::Cell);
        assert_eq!(
            rt.state.device.selected,
            Some(DeviceSel::Cell(name.clone()))
        );
        assert_eq!(
            rt.state.schematic.selected,
            Some(SchemSel::Cell(name.clone()))
        );
        assert_eq!(rt.state.tcl.selected[0].name, name);
        assert!(rt.find("find-show-device").is_some());
        tap(&mut rt, "find-show-device");
        assert_eq!(rt.state.center, CenterPane::Device);
    }

    #[test]
    fn tcl_find_sheet_find_and_select_find() {
        let mut s = seeded();
        s.center = CenterPane::Summary;
        let out = find(&mut s, "cnt").unwrap();
        assert_eq!(s.center, CenterPane::Find);
        assert!(out.starts_with("find_results n="), "{out}");
        assert!(out.contains("cnt"), "{out}");
        let out = sheet_find(&mut s, "nets").unwrap();
        assert!(out.starts_with("sheet_find net hits="), "{out}");
        assert!(s.find.nets && !s.find.cells && !s.find.ports);
        let r = select_find(&mut s, "net:clk").unwrap();
        assert!(r.contains("NAME=clk TYPE=net"), "{r}");
        assert_eq!(s.device.selected, Some(DeviceSel::Net("clk".into())));
        assert!(sheet_find(&mut s, "bogus").is_err());
        crate::tcl::eval_command(&mut s, "sheet_find ports");
        assert!(s.find.ports && !s.find.nets);
        crate::tcl::eval_command(&mut s, "select_find rst");
        assert_eq!(s.schematic.selected, Some(SchemSel::Port("rst".into())));
    }

    #[test]
    fn no_matches_reads_honestly() {
        let mut s = seeded();
        s.find.query = rune::TextEdit::new("zzz_not_here");
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("find-nomatch").is_some());
        assert!(rt.find("find-row-0").is_none());
    }
}
