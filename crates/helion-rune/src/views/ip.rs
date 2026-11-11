//! IP Catalog + Block Design (IP Integrator): catalog table with detail,
//! Create Block Design, Add to Block Design, Generate Output Products, a
//! block-design canvas, the Helion-MM address map, and the generated HDL.
//!
//! Ports the old IDE's IP workspace (`helion-ide.rs:13662 paint_ip`,
//! `:13892 paint_ip_catalog`, `:13926 paint_bd_hdl`) and its model logic
//! (`ide.rs:19025-19290`: `refresh_ip_catalog`, `create_block_design`,
//! `ip_catalog_rows`, `select_ip_core`, `generate_ip`, `create_bd_cell`,
//! `bd_hdl_rows`; `ide.rs:6040 BdView::drawing`) onto the real engines:
//! - the catalog is `helion_ipxact::catalog()` (VLNV + bus per core);
//! - the block design validates and emits SystemVerilog through
//!   `helion_bd::{validate, emit_sv}`; the HDL table is parsed from that
//!   emitted text, never a canned port list;
//! - the address map is `helion_bd::build_memory_map` (4 KiB Helion-MM
//!   apertures from `0x4000_0000`);
//! - Generate Output Products writes `helion_ipxact::to_xml` IP-XACT to
//!   disk and records the real path and size.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/shape.rs`: `Shape::stroke/fill` for wires and grid.
//! - `crates/rune/src/el/layout.rs`: `.absolute()` canvas symbols.
//! - `crates/rune/src/el/scroll.rs`: `.scroll_xy()` canvas pan.

use std::collections::BTreeMap;
use std::path::PathBuf;

use helion_ipxact::IpCore;
use rune::{El, ScrollSpec, col, el, row};

use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
pub use crate::views::ip_canvas::{
    BdDrawing, BdPin, BdSymbol, BdWire, SymKind, bd_canvas, drawing,
};
use crate::views::sources::pane_header;

/// Catalog band height (header + ~8 rows + filter).
const CATALOG_H: f32 = 214.0;
/// IP detail column width.
const DETAIL_W: f32 = 244.0;
/// Detail `label` width.
const LABEL_W: f32 = 62.0;
/// Default block-design name (Vivado's `create_bd_design` default).
pub const BD_NAME: &str = "design_1";

const CATALOG_COLS: [ColSpec; 4] = [
    ColSpec::grow("Name", 1.0),
    ColSpec::grow("VLNV", 1.8),
    ColSpec::fixed("Bus", 72.0),
    ColSpec::fixed("Status", 70.0),
];
const ADDR_COLS: [ColSpec; 4] = [
    ColSpec::grow("Slave", 1.4),
    ColSpec::fixed("Offset", 110.0),
    ColSpec::num("Range", 60.0),
    ColSpec::fixed("High", 110.0),
];
const HDL_COLS: [ColSpec; 4] = [
    ColSpec::grow("Instance", 1.2),
    ColSpec::grow("Module", 1.0),
    ColSpec::fixed("Bus", 80.0),
    ColSpec::grow("Ports", 1.2),
];

/// Block-design tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BdTab {
    #[default]
    Diagram,
    AddressMap,
    Hdl,
}

/// The open block design (old `BdView`): core names in add order plus the
/// real `helion_bd` validation and emitted SystemVerilog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BdModel {
    pub name: String,
    pub cores: Vec<String>,
    pub sv: String,
    pub ok: bool,
    pub errors: Vec<String>,
}

/// Generated output products for one core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedIp {
    pub path: PathBuf,
    pub bytes: usize,
}

/// IP Integrator state.
#[derive(Clone, Debug)]
pub struct IpState {
    pub catalog: Vec<IpCore>,
    pub filter: rune::TextEdit,
    pub selected: Option<String>,
    pub bd: Option<BdModel>,
    pub tab: BdTab,
    pub generated: BTreeMap<String, GeneratedIp>,
}

impl Default for IpState {
    fn default() -> Self {
        Self {
            catalog: helion_ipxact::catalog(),
            filter: rune::TextEdit::new(""),
            selected: None,
            bd: None,
            tab: BdTab::Diagram,
            generated: BTreeMap::new(),
        }
    }
}

/// One catalog row (old `IpCatalogRow`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpRow {
    pub name: String,
    pub vlnv: String,
    pub bus: String,
    pub status: &'static str,
}

/// One generated-HDL instance row (old `BdHdlRow`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdlRow {
    pub instance: String,
    pub module: String,
    pub bus: String,
    pub ports: String,
}

/// One address-editor row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrRow {
    pub slave: String,
    pub core: String,
    pub base: u64,
    pub high: u64,
    pub range: u64,
}

impl IpState {
    pub fn core(&self, name: &str) -> Option<&IpCore> {
        self.catalog.iter().find(|c| c.name == name)
    }

    fn in_bd(&self, name: &str) -> bool {
        self.bd
            .as_ref()
            .is_some_and(|b| b.cores.iter().any(|c| c == name))
    }

    fn status(&self, name: &str) -> &'static str {
        if self.in_bd(name) {
            "In BD"
        } else if self.generated.contains_key(name) {
            "Generated"
        } else {
            "Available"
        }
    }

    pub fn rows(&self) -> Vec<IpRow> {
        self.catalog
            .iter()
            .map(|c| IpRow {
                name: c.name.clone(),
                vlnv: c.vlnv(),
                bus: c.bus.clone(),
                status: self.status(&c.name),
            })
            .collect()
    }

    /// Rows matching the filter (name, VLNV, or bus; case-insensitive).
    pub fn filtered(&self) -> Vec<IpRow> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        self.rows()
            .into_iter()
            .filter(|r| {
                q.is_empty()
                    || r.name.to_ascii_lowercase().contains(&q)
                    || r.vlnv.to_ascii_lowercase().contains(&q)
                    || r.bus.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    fn bd_cores(&self) -> Vec<IpCore> {
        self.bd
            .as_ref()
            .map(|b| {
                b.cores
                    .iter()
                    .filter_map(|n| self.core(n).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Generated HDL rows parsed from the emitted SystemVerilog.
    pub fn hdl_rows(&self) -> Vec<HdlRow> {
        let Some(bd) = &self.bd else {
            return Vec::new();
        };
        bd.sv
            .lines()
            .filter_map(|line| {
                let l = line.trim();
                let mut words = l.split_whitespace();
                let module = words.next()?;
                if matches!(module, "module" | "endmodule" | "input" | "output" | ");") {
                    return None;
                }
                let inst = words.find(|w| w.starts_with("u_"))?;
                let ports = port_names(l);
                if ports.is_empty() {
                    return None;
                }
                let bus = self.core(module).map(|c| c.bus.clone()).unwrap_or_default();
                Some(HdlRow {
                    instance: inst.to_string(),
                    module: module.to_string(),
                    bus,
                    ports: ports.join(", "),
                })
            })
            .collect()
    }

    /// Helion-MM address map from `helion_bd::build_memory_map`.
    pub fn addr_rows(&self) -> Result<Vec<AddrRow>, String> {
        let Some(bd) = &self.bd else {
            return Ok(Vec::new());
        };
        let design = helion_bd::BlockDesign {
            name: bd.name.clone(),
            cores: self.bd_cores(),
        };
        let map = helion_bd::build_memory_map(&design)?;
        Ok(map
            .peripherals
            .iter()
            .map(|p| AddrRow {
                slave: format!("u_{}/s_mm", p.core_name),
                core: p.core_name.clone(),
                base: p.base_addr,
                high: p.high_addr,
                range: p.size_bytes,
            })
            .collect())
    }
}

/// `.port(net)` names in one instance line.
fn port_names(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(dot) = rest.find('.') {
        let after = &rest[dot + 1..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        if end > 0 && after[end..].starts_with('(') {
            out.push(after[..end].to_string());
        }
        rest = &after[end..];
    }
    out
}

// ---- model actions (old `ide.rs:19025-19290`) ----

fn commit(s: &mut AppState, name: String, cores: Vec<String>) -> Result<String, String> {
    let ip_cores: Vec<IpCore> = cores.iter().filter_map(|n| s.ip.core(n).cloned()).collect();
    let design = helion_bd::BlockDesign {
        name: name.clone(),
        cores: ip_cores,
    };
    let v = helion_bd::validate(&design);
    let sv = if v.ok {
        helion_bd::emit_sv(&design)?
    } else {
        String::new()
    };
    let msg = if v.ok {
        format!("create_bd_design {name} cores={}", cores.join(","))
    } else {
        format!("create_bd_design failed {}", v.errors.join("; "))
    };
    s.ip.bd = Some(BdModel {
        name,
        cores,
        sv,
        ok: v.ok,
        errors: v.errors,
    });
    s.navigate(CenterPane::Ip);
    if v.ok { Ok(msg) } else { Err(msg) }
}

/// Reload the catalog from `helion_ipxact::catalog()`.
pub fn refresh_catalog(s: &mut AppState) -> String {
    s.ip.catalog = helion_ipxact::catalog();
    catalog_text(s)
}

/// Create Block Design: an empty validated design (`design_1`).
pub fn create_block_design(s: &mut AppState) -> Result<String, String> {
    if let Some(bd) = &s.ip.bd {
        return Err(format!("create_bd_design: {} is already open", bd.name));
    }
    commit(s, BD_NAME.into(), Vec::new())
}

fn find_core(s: &AppState, verb: &str, spec: &str) -> Result<IpCore, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(format!("{verb}: missing name"));
    }
    if let Ok(i) = spec.parse::<usize>() {
        return s
            .ip
            .catalog
            .get(i)
            .cloned()
            .ok_or_else(|| format!("{verb}: no row {spec}"));
    }
    s.ip.catalog
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(spec) || c.vlnv().eq_ignore_ascii_case(spec))
        .cloned()
        .ok_or_else(|| format!("{verb}: no IP {spec}"))
}

fn spec_or_selected(s: &AppState, spec: &str) -> String {
    if spec.trim().is_empty() {
        s.ip.selected.clone().unwrap_or_default()
    } else {
        spec.trim().to_string()
    }
}

/// IP Catalog click / Tcl `select_ip`.
pub fn select_ip(s: &mut AppState, spec: &str) -> Result<String, String> {
    let core = find_core(s, "select_ip", spec)?;
    if core.bus.eq_ignore_ascii_case("AXI") {
        return Err(format!(
            "select_ip: {} bus AXI is not Helion-MM/ST",
            core.name
        ));
    }
    let status = s.ip.status(&core.name);
    s.ip.selected = Some(core.name.clone());
    s.navigate(CenterPane::Ip);
    Ok(format!(
        "ip {} VLNV={} BUS={} STATUS={status}",
        core.name,
        core.vlnv(),
        core.bus
    ))
}

/// Where Generate Output Products writes a core's IP-XACT.
pub fn output_dir(s: &AppState, core: &str) -> PathBuf {
    let loc = PathBuf::from(&s.project.location);
    let project = if s.project.name.is_empty() {
        "project".to_string()
    } else {
        s.project.name.clone()
    };
    let base = if !s.project.location.is_empty() && loc.is_dir() {
        loc.join(format!("{project}.gen"))
    } else {
        std::env::temp_dir().join("helion-ip").join(&project)
    };
    base.join("sources_1").join("ip").join(core)
}

/// Why Generate Output Products is unavailable (`None` = enabled).
pub fn generate_block_reason(s: &AppState) -> Option<String> {
    let Some(name) = &s.ip.selected else {
        return Some("Select an IP core in the catalog first.".into());
    };
    match s.ip.core(name) {
        Some(c) if c.bus.eq_ignore_ascii_case("AXI") => Some(format!(
            "{} uses AXI; Helion emits Helion-MM/ST only.",
            c.name
        )),
        Some(_) => None,
        None => Some(format!("{name} is no longer in the catalog.")),
    }
}

/// Generate Output Products: write the core's IP-XACT component XML.
pub fn generate_ip(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec_or_selected(s, spec);
    let core = find_core(s, "generate_ip", &spec)?;
    if core.bus.eq_ignore_ascii_case("AXI") {
        return Err(format!(
            "generate_ip: {} bus AXI is not Helion-MM/ST",
            core.name
        ));
    }
    let xml = helion_ipxact::to_xml(&core);
    if xml.contains("AXI") {
        return Err("generate_ip: IP-XACT must not emit AXI".into());
    }
    let dir = output_dir(s, &core.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("generate_ip: {}: {e}", dir.display()))?;
    let path = dir.join("component.xml");
    std::fs::write(&path, &xml).map_err(|e| format!("generate_ip: {}: {e}", path.display()))?;
    s.ip.generated.insert(
        core.name.clone(),
        GeneratedIp {
            path: path.clone(),
            bytes: xml.len(),
        },
    );
    s.ip.selected = Some(core.name.clone());
    s.navigate(CenterPane::Ip);
    Ok(format!(
        "generate_target {} VLNV={} BUS={} xml_bytes={} path={}",
        core.name,
        core.vlnv(),
        core.bus,
        xml.len(),
        path.display()
    ))
}

/// Why Add to Block Design is unavailable (`None` = enabled).
pub fn add_block_reason(s: &AppState) -> Option<String> {
    let Some(name) = &s.ip.selected else {
        return Some("Select an IP core in the catalog first.".into());
    };
    let core = s.ip.core(name)?;
    if core.bus != "Helion-MM" && core.bus != "Helion-ST" {
        return Some(format!(
            "{} uses {}; only Helion-MM/ST cores join a block design.",
            core.name, core.bus
        ));
    }
    if s.ip.in_bd(name) {
        return Some(format!("{name} is already on the canvas."));
    }
    None
}

/// Add to Block Design / Tcl `create_bd_cell` (creates the BD if needed).
pub fn create_bd_cell(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec_or_selected(s, spec);
    let core = find_core(s, "create_bd_cell", &spec)?;
    if core.bus != "Helion-MM" && core.bus != "Helion-ST" {
        return Err(format!(
            "create_bd_cell: {} bus {} not Helion-MM/ST",
            core.name, core.bus
        ));
    }
    if s.ip.bd.is_none() {
        create_block_design(s)?;
    }
    let (name, mut cores) =
        s.ip.bd
            .as_ref()
            .map(|b| (b.name.clone(), b.cores.clone()))
            .unwrap_or_else(|| (BD_NAME.into(), Vec::new()));
    if cores.iter().any(|n| n == &core.name) {
        s.navigate(CenterPane::Ip);
        return Ok(format!("create_bd_cell {} already on canvas", core.name));
    }
    cores.push(core.name.clone());
    let out = commit(s, name, cores)?;
    s.ip.selected = Some(core.name.clone());
    Ok(format!("create_bd_cell {} {out}", core.name))
}

/// Tcl `ip_catalog` text (old `ip_catalog_text`).
pub fn catalog_text(s: &AppState) -> String {
    let rows = s.ip.rows();
    let mut out = format!("ip_catalog n={}", rows.len());
    for r in &rows {
        out.push_str(&format!(
            "\nNAME={} VLNV={} BUS={} STATUS={}",
            r.name, r.vlnv, r.bus, r.status
        ));
    }
    out
}

/// Tcl `bd_hdl` text (old `bd_hdl_text`).
pub fn hdl_text(s: &AppState) -> String {
    let rows = s.ip.hdl_rows();
    let mut out = format!("bd_hdl n={}", rows.len());
    for r in &rows {
        out.push_str(&format!(
            "\nINSTANCE={} MODULE={} BUS={} PORTS={}",
            r.instance, r.module, r.bus, r.ports
        ));
    }
    out
}

/// Tcl `bd_drawing` text (old `bd_drawing_text`).
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
        bd.name,
        u8::from(bd.ok)
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

/// Ensure a block design exists (Tcl `bd_drawing`/`bd_hdl`).
pub fn ensure_bd(s: &mut AppState) {
    if s.ip.bd.is_none() {
        let _ = create_block_design(s);
    }
    s.navigate(CenterPane::Ip);
}

fn addr_table(s: &AppState, t: &VivadoTheme) -> El {
    let rows = match s.ip.addr_rows() {
        Ok(r) => r,
        Err(e) => {
            return cad::empty_state(
                "ip-addr-empty",
                &format!("Address map unavailable: {e}"),
                None,
                t,
            );
        }
    };
    if rows.is_empty() {
        return cad::empty_state(
            "ip-addr-empty",
            "No Helion-MM slaves yet. Add a Helion-MM core to assign an address.",
            None,
            t,
        );
    }
    let mut list = col().w_full();
    for (i, r) in rows.iter().enumerate() {
        let selected = s.ip.selected.as_deref() == Some(r.core.as_str());
        let core = r.core.clone();
        list = list.child(
            cad::table_row(format!("ip-addr-row-{i}"), r.slave.clone(), selected, t)
                .on_press(move |s: &mut AppState| {
                    let _ = select_ip(s, &core);
                })
                .child(cad::cell(
                    &ADDR_COLS[0],
                    cad::cut(r.slave.clone(), cad::mono_style(), t.text),
                ))
                .child(cad::cell(
                    &ADDR_COLS[1],
                    cad::mono(cad::addr_hex(r.base), t),
                ))
                .child(cad::cell(
                    &ADDR_COLS[2],
                    cad::mono(cad::range_label(r.range), t),
                ))
                .child(cad::cell(
                    &ADDR_COLS[3],
                    cad::mono(cad::addr_hex(r.high), t),
                )),
        );
    }
    cad::table_frame("ip-addr", t)
        .child(cad::header("ip-addr-head", &ADDR_COLS, t))
        .child(list)
}

fn hdl_table(s: &AppState, t: &VivadoTheme) -> El {
    let rows = s.ip.hdl_rows();
    if rows.is_empty() {
        return cad::empty_state(
            "ip-hdl-empty",
            "No instances yet. Add a core to emit its SystemVerilog instance.",
            None,
            t,
        );
    }
    let mut list = col().w_full();
    for (i, r) in rows.iter().enumerate() {
        let selected = s.ip.selected.as_deref() == Some(r.module.as_str());
        let core = r.module.clone();
        list = list.child(
            cad::table_row(format!("ip-hdl-row-{i}"), r.instance.clone(), selected, t)
                .on_press(move |s: &mut AppState| {
                    let _ = select_ip(s, &core);
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
                    cad::cut(r.ports.clone(), cad::mono_style(), t.text_dim),
                )),
        );
    }
    let lines = s.ip.bd.as_ref().map(|b| b.sv.lines().count()).unwrap_or(0);
    col()
        .w_full()
        .gap(4.0)
        .child(
            cad::table_frame("ip-hdl", t)
                .child(cad::header("ip-hdl-head", &HDL_COLS, t))
                .child(list),
        )
        .child(cad::dim(
            format!(
                "{}.sv · {} lines · emitted by helion-bd",
                s.ip.bd.as_ref().map(|b| b.name.as_str()).unwrap_or(BD_NAME),
                lines
            ),
            t,
        ))
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
            _ => t.text_dim,
        };
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
                cad::cut(r.status, VivadoTheme::FONT_UI, status_ink),
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

fn detail_panel(s: &AppState, t: &VivadoTheme) -> El {
    let body = match s.ip.selected.as_deref().and_then(|n| s.ip.core(n)) {
        None => col()
            .w_full()
            .child(cad::dim("Select a core to see its VLNV and outputs.", t)),
        Some(c) => {
            let output = match s.ip.generated.get(&c.name) {
                Some(g) => cad::cut(g.path.display().to_string(), VivadoTheme::FONT_UI, t.text)
                    .truncate_middle(),
                None => cad::dim("Not generated", t),
            };
            col()
                .key("ip-detail-body")
                .w_full()
                .child(cad::title(c.name.clone(), t).key("ip-detail-name"))
                .child(cad::kv(
                    "VLNV",
                    cad::cut(c.vlnv(), cad::mono_style(), t.text),
                    LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Vendor",
                    cad::cut(c.vendor.clone(), VivadoTheme::FONT_UI, t.text),
                    LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Library",
                    cad::cut(c.library.clone(), VivadoTheme::FONT_UI, t.text),
                    LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Version",
                    cad::mono(c.version.clone(), t),
                    LABEL_W,
                    t,
                ))
                .child(cad::kv("Bus", cad::ui(c.bus.clone(), t), LABEL_W, t))
                .child(cad::kv(
                    "Status",
                    cad::ui(s.ip.status(&c.name), t),
                    LABEL_W,
                    t,
                ))
                .child(cad::kv(
                    "Output",
                    output.key("ip-detail-output"),
                    LABEL_W,
                    t,
                ))
        }
    };
    col()
        .key("ip-detail")
        .w(DETAIL_W)
        .h_full()
        .gap(6.0)
        .child(
            row()
                .h(24.0)
                .items_center()
                .child(cad::title("Properties", t)),
        )
        .child(
            col()
                .w_full()
                .grow(1.0)
                .padding(8.0)
                .radius(4.0)
                .fill(t.panel_bg)
                .border(1.0, t.border)
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
    let summary = format!("{} · {} IP · {} nets", bd.name, bd.cores.len(), d.nets);
    let (badge_text, badge_ink) = if bd.ok {
        ("Valid".to_string(), t.pass)
    } else {
        (format!("{} errors", bd.errors.len()), t.fail)
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
        tabs = tabs.child(cad::tab(
            key,
            label,
            s.ip.tab == tab,
            t,
            move |s: &mut AppState| s.ip.tab = tab,
        ));
    }
    let body = match s.ip.tab {
        BdTab::Diagram => bd_canvas(s, t),
        BdTab::AddressMap => col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(8.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(addr_table(s, t)),
        BdTab::Hdl => col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(8.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(hdl_table(s, t)),
    };
    let mut head = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::title("Block Design", t))
        .child(cad::dim(summary, t).key("ip-bd-summary"))
        .child(cad::badge(badge_text, badge_ink, t).key("ip-bd-valid"));
    if !bd.ok {
        head = head.child(cad::cut(bd.errors.join("; "), VivadoTheme::FONT_UI, t.fail));
    }
    col()
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
        )
}

/// The center IP Integrator pane wired into the shell.
pub fn ip_pane(s: &AppState, t: &VivadoTheme) -> El {
    let create_reason =
        s.ip.bd
            .as_ref()
            .map(|b| format!("Block design {} is already open.", b.name));
    let add_reason = add_block_reason(s);
    let gen_reason = generate_block_reason(s);
    let tools = cad::toolbar("ip-toolbar", t)
        .child(cad::btn(
            "ip-refresh",
            "Refresh Catalog",
            t,
            None,
            |s: &mut AppState| {
                let n = {
                    refresh_catalog(s);
                    s.ip.catalog.len()
                };
                s.console.lines.push(crate::state::ConsoleLine {
                    text: format!("ip_catalog n={n}"),
                    sev: crate::state::MsgSev::Info,
                });
            },
        ))
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
        .child(cad::btn(
            "ip-generate",
            "Generate Output Products",
            t,
            gen_reason.as_deref(),
            |s: &mut AppState| crate::tcl::eval_command(s, "generate_ip"),
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

    fn ip_state() -> AppState {
        let mut s = init();
        s.center = CenterPane::Ip;
        s
    }

    #[test]
    fn catalog_comes_from_helion_ipxact() {
        let s = ip_state();
        let rows = s.ip.rows();
        assert_eq!(rows.len(), helion_ipxact::catalog().len());
        let uart = rows.iter().find(|r| r.name == "h_uart").expect("h_uart");
        let core = helion_ipxact::catalog()
            .into_iter()
            .find(|c| c.name == "h_uart")
            .unwrap();
        assert_eq!(uart.vlnv, core.vlnv());
        assert_eq!(uart.bus, "Helion-MM");
        assert_eq!(uart.status, "Available");
    }

    #[test]
    fn buttons_disable_until_an_ip_is_selected() {
        let mut rt = harness_with(ip_state());
        rt.frame(0.0);
        for key in [
            "ip",
            "ip-toolbar",
            "ip-catalog",
            "ip-catalog-head",
            "ip-row-h_uart",
            "ip-detail",
            "ip-bd-empty",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        assert!(generate_block_reason(&rt.state).is_some());
        assert!(add_block_reason(&rt.state).is_some());
        tap(&mut rt, "ip-generate");
        assert!(
            rt.state.ip.generated.is_empty(),
            "disabled generate must not run"
        );
        tap(&mut rt, "ip-row-h_uart");
        assert_eq!(rt.state.ip.selected.as_deref(), Some("h_uart"));
        assert!(rt.find("ip-detail-name").is_some());
        assert_eq!(generate_block_reason(&rt.state), None);
        assert_eq!(add_block_reason(&rt.state), None);
    }

    #[test]
    fn add_to_bd_emits_real_sv_and_address_map() {
        let mut s = ip_state();
        create_bd_cell(&mut s, "h_uart").expect("uart");
        create_bd_cell(&mut s, "h_gpio").expect("gpio");
        let bd = s.ip.bd.clone().unwrap();
        assert_eq!(bd.name, BD_NAME);
        assert!(bd.ok);
        assert!(bd.sv.contains("u_h_uart"), "{}", bd.sv);
        let hdl = s.ip.hdl_rows();
        assert_eq!(hdl.len(), 2);
        assert_eq!(hdl[0].instance, "u_h_uart");
        assert_eq!(hdl[0].ports, "clk, resetn");
        let addr = s.ip.addr_rows().unwrap();
        assert_eq!(addr.len(), 2);
        assert_eq!(addr[0].base, 0x4000_0000);
        assert_eq!(addr[1].base, 0x4000_1000);
        assert_eq!(addr[0].range, 0x1000);
        let d = drawing(&s.ip);
        assert_eq!(
            d.symbols.iter().filter(|s| s.kind == SymKind::Core).count(),
            2
        );
        assert_eq!(d.nets, 4, "clk, resetn, two MM slaves");
        assert!(
            d.wires.iter().all(|w| w
                .pts
                .windows(2)
                .all(|p| p[0][0] == p[1][0] || p[0][1] == p[1][1])),
            "orthogonal wires"
        );
        assert!(
            create_bd_cell(&mut s, "h_uart")
                .unwrap()
                .contains("already")
        );
        assert!(add_block_reason(&s).is_some());
        assert!(create_block_design(&mut s).is_err(), "no silent BD reset");
    }

    #[test]
    fn generate_writes_ipxact_to_disk() {
        let mut s = ip_state();
        s.project.name = format!("iptest-{}", std::process::id());
        select_ip(&mut s, "h_gpio").unwrap();
        let out = generate_ip(&mut s, "").expect("generate");
        let g = s.ip.generated.get("h_gpio").expect("recorded");
        let xml = std::fs::read_to_string(&g.path).expect("written");
        assert_eq!(xml.len(), g.bytes);
        assert!(xml.contains("<ipxact:name>h_gpio</ipxact:name>"));
        assert!(out.contains("xml_bytes="));
        assert_eq!(s.ip.status("h_gpio"), "Generated");
        let _ = std::fs::remove_dir_all(output_dir(&s, "h_gpio"));
    }

    #[test]
    fn bd_tabs_and_canvas_mount() {
        let mut s = ip_state();
        create_bd_cell(&mut s, "h_uart").unwrap();
        create_bd_cell(&mut s, "h_debounce").ok();
        let mut rt = harness_with(s);
        rt.frame(0.0);
        for key in [
            "ip-bd",
            "ip-bd-summary",
            "ip-bd-tabs",
            "ip-bd-canvas",
            "ip-bd-sym-u_h_uart",
            "ip-bd-sym-mm_interconnect",
            "ip-bd-port-clk",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        tap(&mut rt, "ip-tab-address");
        assert!(rt.find("ip-addr-row-0").is_some());
        tap(&mut rt, "ip-tab-hdl");
        assert!(rt.find("ip-hdl-row-0").is_some());
        // Let the freshly mounted tab body settle before hit-testing it.
        for i in 1..=30 {
            rt.frame(0.1 + i as f64 / 60.0);
        }
        tap(&mut rt, "ip-hdl-row-0");
        assert_eq!(rt.state.ip.selected.as_deref(), Some("h_uart"));
    }

    #[test]
    fn tcl_verbs_drive_the_model() {
        let mut s = init();
        crate::tcl::eval_command(&mut s, "ip_catalog");
        assert_eq!(s.center, CenterPane::Ip);
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.starts_with("ip_catalog n="))
        );
        crate::tcl::eval_command(&mut s, "select_ip h_timer");
        assert_eq!(s.ip.selected.as_deref(), Some("h_timer"));
        crate::tcl::eval_command(&mut s, "create_bd_cell");
        assert!(s.ip.in_bd("h_timer"));
        crate::tcl::eval_command(&mut s, "bd_drawing");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.starts_with("drawing symbols="))
        );
        crate::tcl::eval_command(&mut s, "bd_hdl");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.contains("INSTANCE=u_h_timer"))
        );
        crate::tcl::eval_command(&mut s, "select_ip nope_core");
        assert!(s.console.lines.last().unwrap().text.contains("no IP"));
        assert_eq!(
            port_names("  a #() u_a (.clk(clk), .resetn(resetn));"),
            vec!["clk", "resetn"]
        );
    }
}
