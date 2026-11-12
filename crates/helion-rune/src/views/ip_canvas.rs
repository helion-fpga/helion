//! IP Integrator block-design canvas: symbol layout (ports, Helion-MM
//! interconnect, cores), orthogonal interface/scalar wiring, and the
//! scrollable canvas view.
//!
//! Ports the old IDE's `ide.rs:6040 BdView::drawing` model (one trunk per
//! net so wires never overlap) and draws it fresh in Rune: boxes with pin
//! labels, instance names above blocks, thick interface wires. Selection is
//! shared with the IP catalog through [`crate::views::ip::select_ip`].
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/shape.rs`: `Shape::stroke/fill` for wires and grid.
//! - `crates/rune/src/el/layout.rs`: `.absolute()` canvas symbols.
//! - `crates/rune/src/el/scroll.rs`: `.scroll_xy()` canvas pan.

use rune::{El, Shape, canvas, col, el, text};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::FONT_PIN;
use crate::views::ip_model::{IpState, open_customize, select_cell};

// Canvas geometry (world points at 100%).
const MARGIN: f32 = 16.0;
const TITLE_H: f32 = 16.0;
const PORT_W: f32 = 64.0;
const PORT_H: f32 = 18.0;
const HUB_W: f32 = 136.0;
const HUB_H: f32 = 84.0;
const CORE_W: f32 = 160.0;
const CORE_H: f32 = 64.0;
const COL_GAP: f32 = 76.0;
const ROW_GAP: f32 = 14.0;
const STUB: f32 = 10.0;
const GRID: f32 = 16.0;

// ---- canvas drawing (old `BdView::drawing`) ----

/// Symbol class on the block-design canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Port,
    Interconnect,
    Core,
}

#[derive(Clone, Debug)]
pub struct BdPin {
    pub name: String,
    pub iface: bool,
    pub output: bool,
    /// Pin tip (stub end) in world points.
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub struct BdSymbol {
    pub name: String,
    pub kind: SymKind,
    /// Catalog core name for [`SymKind::Core`] (selection target).
    pub core: String,
    pub bus: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub pins: Vec<BdPin>,
}

impl BdSymbol {
    fn pin(&self, name: &str, output: bool) -> Option<(f32, f32)> {
        self.pins
            .iter()
            .find(|p| p.name == name && p.output == output)
            .map(|p| (p.x, p.y))
    }
}

#[derive(Clone, Debug)]
pub struct BdWire {
    pub net: String,
    pub iface: bool,
    pub src: String,
    pub dst: String,
    pub pts: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, Default)]
pub struct BdDrawing {
    pub symbols: Vec<BdSymbol>,
    pub wires: Vec<BdWire>,
    /// Distinct nets: clk, resetn, and one interface net per bus slave.
    pub nets: usize,
    pub w: f32,
    pub h: f32,
}

fn pin(name: &str, iface: bool, output: bool, x: f32, y: f32) -> BdPin {
    BdPin {
        name: name.into(),
        iface,
        output,
        x,
        y,
    }
}

/// Lay out what `helion_bd::emit_system` instantiated: input ports, the
/// Helion-MM master cell, the interconnect (only when the HDL has one), and
/// the remaining cells; route clk/resetn/interface nets orthogonally with one
/// trunk per net. Unconnected cells draw their pins without wires (their
/// pins are top-level ports in the HDL).
pub fn drawing(ip: &IpState) -> BdDrawing {
    let Some(bd) = &ip.bd else {
        return BdDrawing::default();
    };
    let emit = bd.emit.as_ref().ok();
    let ic = emit.and_then(|e| e.interconnect.clone());
    let ext_master = ic.as_ref().is_some_and(|i| i.master.is_none());
    let any_connected = bd.sys.cells.iter().any(|c| c.connected);
    let top = MARGIN + TITLE_H;
    let mut symbols = Vec::new();
    // Column 0: design input ports.
    let mut port_names: Vec<&str> = Vec::new();
    if any_connected {
        port_names.extend(["clk", "resetn"]);
    }
    if ext_master {
        port_names.push("s_mm");
    }
    for (i, name) in port_names.iter().enumerate() {
        let y = top + 20.0 + i as f32 * 24.0;
        symbols.push(BdSymbol {
            name: (*name).into(),
            kind: SymKind::Port,
            core: String::new(),
            bus: String::new(),
            x: MARGIN,
            y: y - PORT_H / 2.0,
            w: PORT_W,
            h: PORT_H,
            pins: vec![pin(name, *name == "s_mm", true, MARGIN + PORT_W + STUB, y)],
        });
    }
    let mut x = MARGIN + PORT_W + COL_GAP;
    let cell_h = |c: &helion_bd::SysCell| -> f32 {
        let left = 2 + usize::from(c.role() == Some(helion_ipxact::MmRole::Slave));
        let right = ext_pins(c).len().min(MAX_EXT) + usize::from(ext_pins(c).len() > MAX_EXT)
            + usize::from(c.role() == Some(helion_ipxact::MmRole::Master));
        (left.max(right) as f32 * 16.0 + 18.0).max(CORE_H)
    };
    let core_sym = |c: &helion_bd::SysCell, x: f32, y: f32| -> BdSymbol {
        let h = cell_h(c);
        let mut pins = Vec::new();
        let mut ly = y + 16.0;
        for p in ["clk", "resetn"] {
            if c.model.module.port(p).is_some() {
                pins.push(pin(p, false, false, x - STUB, ly));
                ly += 16.0;
            }
        }
        if c.role() == Some(helion_ipxact::MmRole::Slave) {
            pins.push(pin("S_MM", true, false, x - STUB, ly));
        }
        let mut ry = y + 16.0;
        if c.role() == Some(helion_ipxact::MmRole::Master) {
            pins.push(pin("M_MM", true, true, x + CORE_W + STUB, ry));
            ry += 16.0;
        }
        let ext = ext_pins(c);
        for e in ext.iter().take(MAX_EXT) {
            pins.push(pin(e, false, true, x + CORE_W + STUB, ry));
            ry += 16.0;
        }
        if ext.len() > MAX_EXT {
            pins.push(pin(&format!("+{} more", ext.len() - MAX_EXT), false, true, x + CORE_W + STUB, ry));
        }
        BdSymbol {
            name: c.inst.clone(),
            kind: SymKind::Core,
            core: c.model.core.name.clone(),
            bus: c.model.core.bus.clone(),
            x,
            y,
            w: CORE_W,
            h,
            pins,
        }
    };
    // Column 1: the bus master (processor) when connected.
    let master = ic.as_ref().and_then(|i| i.master.clone());
    if let Some(m) = master.as_ref().and_then(|m| bd.sys.cell(m)) {
        symbols.push(core_sym(m, x, top));
        x += CORE_W + COL_GAP;
    }
    // Column 2: the interconnect the HDL instantiates.
    let hub_h = HUB_H.max(32.0 + 16.0 * 2.0);
    if let Some(i) = &ic {
        symbols.push(BdSymbol {
            name: i.inst.clone(),
            kind: SymKind::Interconnect,
            core: i.module.clone(),
            bus: "Helion-MM".into(),
            x,
            y: top,
            w: HUB_W,
            h: hub_h,
            pins: vec![
                pin("clk", false, false, x - STUB, top + 16.0),
                pin("resetn", false, false, x - STUB, top + 32.0),
                pin("S_MM", true, false, x - STUB, top + 48.0),
                pin("M_MM", true, true, x + HUB_W + STUB, top + 48.0),
            ],
        });
        x += HUB_W + COL_GAP;
    }
    // Column 3: every other cell, stacked.
    let mut y = top;
    for c in &bd.sys.cells {
        if Some(&c.inst) == master.as_ref() {
            continue;
        }
        let sym = core_sym(c, x, y);
        y += sym.h + TITLE_H + ROW_GAP;
        symbols.push(sym);
    }
    // Wires.
    let mut wires = Vec::new();
    let find = |name: &str| symbols.iter().find(|s| s.name == name);
    let route = |a: (f32, f32), b: (f32, f32), trunk: f32| -> Vec<[f32; 2]> {
        if (a.1 - b.1).abs() < 0.5 {
            vec![[a.0, a.1], [b.0, b.1]]
        } else {
            vec![[a.0, a.1], [trunk, a.1], [trunk, b.1], [b.0, b.1]]
        }
    };
    let mut nets = 0;
    let connected: Vec<&str> = bd
        .sys
        .cells
        .iter()
        .filter(|c| c.connected)
        .map(|c| c.inst.as_str())
        .collect();
    for (net, offset) in [("clk", 16.0), ("resetn", 30.0)] {
        let Some(tip) = find(net).and_then(|p| p.pin(net, true)) else {
            continue;
        };
        nets += 1;
        let targets = symbols
            .iter()
            .filter(|s| s.kind == SymKind::Interconnect || connected.contains(&s.name.as_str()));
        for t in targets {
            if let Some(dst) = t.pin(net, false) {
                wires.push(BdWire {
                    net: net.into(),
                    iface: false,
                    src: net.into(),
                    dst: t.name.clone(),
                    pts: route(tip, dst, tip.0 + offset),
                });
            }
        }
    }
    if let Some(i) = &ic {
        let hub = find(&i.inst).cloned().expect("hub symbol");
        let s_in = hub.pin("S_MM", false).expect("hub S_MM");
        let src = match &master {
            Some(m) => find(m).and_then(|s| s.pin("M_MM", true)).map(|p| (m.clone(), p)),
            None => find("s_mm").and_then(|s| s.pin("s_mm", true)).map(|p| ("s_mm".to_string(), p)),
        };
        if let Some((name, tip)) = src {
            wires.push(BdWire {
                net: "mm_master".into(),
                iface: true,
                src: name,
                dst: hub.name.clone(),
                pts: route(tip, s_in, s_in.0 - 24.0),
            });
            nets += 1;
        }
        let m = hub.pin("M_MM", true).expect("hub M_MM");
        for sl in &i.slaves {
            if let Some(dst) = find(&sl.inst).and_then(|s| s.pin("S_MM", false)) {
                wires.push(BdWire {
                    net: format!("{}_mm", sl.inst),
                    iface: true,
                    src: hub.name.clone(),
                    dst: sl.inst.clone(),
                    pts: route(m, dst, m.0 + 28.0),
                });
                nets += 1;
            }
        }
    }
    let w = symbols
        .iter()
        .map(|s| s.x + s.w + MARGIN + 90.0)
        .fold(320.0f32, f32::max);
    let h = symbols
        .iter()
        .map(|s| s.y + s.h + MARGIN)
        .fold(120.0f32, f32::max);
    BdDrawing {
        symbols,
        wires,
        nets,
        w,
        h,
    }
}

/// Right-side pins a cell exports (non clock/reset, non Helion-MM bus).
fn ext_pins(c: &helion_bd::SysCell) -> Vec<String> {
    c.model
        .module
        .ports
        .iter()
        .filter(|p| !helion_ipxact::is_clock_or_reset(&p.name))
        .filter(|p| c.role().is_none() || !helion_ipxact::is_mm_signal(&p.name))
        .map(|p| p.name.clone())
        .collect()
}

/// Exported pins drawn per cell before collapsing into `+N more`.
const MAX_EXT: usize = 4;

// ---- canvas view ----

fn wires_shape(d: &BdDrawing, t: &VivadoTheme) -> Shape {
    let mut shape = Shape::new();
    let mut gx = GRID;
    while gx < d.w {
        let mut gy = GRID;
        while gy < d.h {
            shape = shape.fill(
                vec![
                    [gx, gy],
                    [gx + 1.2, gy],
                    [gx + 1.2, gy + 1.2],
                    [gx, gy + 1.2],
                ],
                t.border_light,
            );
            gy += GRID;
        }
        gx += GRID;
    }
    for w in &d.wires {
        shape = shape.stroke(
            w.pts.clone(),
            if w.iface { 3.0 } else { 1.25 },
            if w.iface { t.link } else { t.pass },
        );
    }
    // Pin stubs from each box edge to its tip.
    for s in &d.symbols {
        for p in &s.pins {
            let edge = if p.output { s.x + s.w } else { s.x };
            shape = shape.stroke(
                vec![[edge, p.y], [p.x, p.y]],
                if p.iface { 3.0 } else { 1.25 },
                if p.iface { t.link } else { t.text_dim },
            );
        }
        if s.kind == SymKind::Port {
            // Input port: pentagon pointing into the design.
            let (x, y, w, h) = (s.x, s.y, s.w, s.h);
            let outline = vec![
                [x, y],
                [x + w - 8.0, y],
                [x + w, y + h / 2.0],
                [x + w - 8.0, y + h],
                [x, y + h],
            ];
            shape = shape.fill(outline.clone(), t.header_bg);
            shape = shape.stroke_with(outline, true, 1.0, rune::LineCap::Butt, t.text_dim);
        }
    }
    shape
}

/// Canvas elements for one symbol: the block (with pin labels) plus its
/// instance name above it (Vivado IP Integrator layout).
fn symbol_nodes(s: &AppState, sy: &BdSymbol, t: &VivadoTheme) -> Vec<El> {
    if sy.kind == SymKind::Port {
        return vec![
            el().key(format!("ip-bd-port-{}", sy.name))
                .absolute(sy.x, sy.y)
                .size(sy.w - 8.0, sy.h)
                .center()
                .child(text(sy.name.clone()).font(FONT_PIN.mono()).color(t.text)),
        ];
    }
    let selected =
        sy.kind == SymKind::Core && s.ip.selected_cell.as_deref() == Some(sy.name.as_str());
    let caption = if sy.kind == SymKind::Interconnect {
        "Helion-MM".to_string()
    } else {
        sy.core.clone()
    };
    let mut node = el()
        .key(format!("ip-bd-sym-{}", sy.name))
        .absolute(sy.x, sy.y)
        .size(sy.w, sy.h)
        .radius(3.0)
        .fill(t.header_bg)
        .border(
            if selected { 2.0 } else { 1.0 },
            if selected { t.accent } else { t.text_dim },
        );
    for p in &sy.pins {
        let y = p.y - sy.y - 6.0;
        let label = text(p.name.clone())
            .font(FONT_PIN.mono())
            .color(if p.iface { t.link } else { t.text_dim });
        node = node.child(if p.output {
            el().absolute(sy.w * 0.5, y)
                .w(sy.w * 0.5 - 6.0)
                .h(12.0)
                .child(label.w_full().align(rune::Align::End))
        } else {
            el().absolute(6.0, y).h(12.0).child(label)
        });
    }
    // Caption in the pin-free corner: bottom-right on cores (pins enter on
    // the left), bottom-left on the interconnect (M_MM leaves on the right).
    let caption_x = if sy.kind == SymKind::Interconnect {
        6.0
    } else {
        sy.w * 0.4
    };
    let caption_align = if sy.kind == SymKind::Interconnect {
        rune::Align::Start
    } else {
        rune::Align::End
    };
    node = node.child(
        el().absolute(caption_x, sy.h - 15.0)
            .w(sy.w * 0.6 - 6.0)
            .h(12.0)
            .child(
                text(caption)
                    .font(FONT_PIN)
                    .color(t.text_faint)
                    .w_full()
                    .align(caption_align)
                    .truncate(),
            ),
    );
    if sy.kind == SymKind::Core {
        let inst = sy.name.clone();
        let inst2 = sy.name.clone();
        node = node
            .role("button")
            .label(format!("{} ({})", sy.name, sy.core))
            .description("Double-click to customize")
            .focus_selected(selected)
            .focusable(true)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                let _ = select_cell(s, &inst);
            })
            .on_double_press(move |s: &mut AppState| {
                let _ = open_customize(s, &inst2);
            });
    }
    vec![
        el().absolute(sy.x, sy.y - TITLE_H)
            .w(sy.w)
            .h(TITLE_H - 2.0)
            .child(
                text(sy.name.clone())
                    .font(VivadoTheme::FONT_TITLE)
                    .color(t.text)
                    .truncate(),
            ),
        node,
    ]
}

pub fn bd_canvas(s: &AppState, t: &VivadoTheme) -> El {
    let d = drawing(&s.ip);
    let mut sheet = el()
        .key("ip-bd-sheet")
        .w(d.w)
        .h(d.h)
        .fill(t.panel_bg)
        .child(canvas(wires_shape(&d, t)).w_full().h_full());
    for sy in &d.symbols {
        for node in symbol_nodes(s, sy, t) {
            sheet = sheet.child(node);
        }
    }
    col()
        .key("ip-bd-canvas")
        .role("application")
        .label("Block design canvas")
        .items_start()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .scroll_xy()
        .child(sheet)
}
