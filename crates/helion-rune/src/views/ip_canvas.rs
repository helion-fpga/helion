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
use crate::views::ip::{IpState, select_ip};

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

/// Lay out ports, the Helion-MM interconnect (when a Helion-MM slave
/// exists), and the cores; route clk/resetn/interface nets orthogonally
/// with one vertical trunk per net so lines never overlap.
pub fn drawing(ip: &IpState) -> BdDrawing {
    let Some(bd) = &ip.bd else {
        return BdDrawing::default();
    };
    let cores: Vec<(String, String)> = bd
        .cores
        .iter()
        .map(|n| {
            let bus = ip.core(n).map(|c| c.bus.clone()).unwrap_or_default();
            (n.clone(), bus)
        })
        .collect();
    let has_hub = cores.iter().any(|(_, b)| b == "Helion-MM");
    let top = MARGIN + TITLE_H;
    let hub_x = MARGIN + PORT_W + COL_GAP;
    let clk_y = top + 20.0;
    let rst_y = top + 44.0;
    let mut symbols = Vec::new();
    for (name, y) in [("clk", clk_y), ("resetn", rst_y)] {
        symbols.push(BdSymbol {
            name: name.into(),
            kind: SymKind::Port,
            core: String::new(),
            bus: String::new(),
            x: MARGIN,
            y: y - PORT_H / 2.0,
            w: PORT_W,
            h: PORT_H,
            pins: vec![pin(name, false, true, MARGIN + PORT_W + STUB, y)],
        });
    }
    if has_hub {
        symbols.push(BdSymbol {
            name: "mm_interconnect".into(),
            kind: SymKind::Interconnect,
            core: String::new(),
            bus: "Helion-MM".into(),
            x: hub_x,
            y: top,
            w: HUB_W,
            h: HUB_H,
            pins: vec![
                pin("clk", false, false, hub_x - STUB, clk_y),
                pin("resetn", false, false, hub_x - STUB, rst_y),
                pin("clk", false, true, hub_x + HUB_W + STUB, clk_y),
                pin("resetn", false, true, hub_x + HUB_W + STUB, rst_y),
                pin("M_MM", true, true, hub_x + HUB_W + STUB, top + 66.0),
            ],
        });
    }
    let core_x = if has_hub {
        hub_x + HUB_W + COL_GAP
    } else {
        hub_x
    };
    for (i, (name, bus)) in cores.iter().enumerate() {
        let y = top + i as f32 * (CORE_H + TITLE_H + ROW_GAP);
        let iface = if bus == "Helion-ST" { "S_ST" } else { "S_MM" };
        symbols.push(BdSymbol {
            name: format!("u_{name}"),
            kind: SymKind::Core,
            core: name.clone(),
            bus: bus.clone(),
            x: core_x,
            y,
            w: CORE_W,
            h: CORE_H,
            pins: vec![
                pin("clk", false, false, core_x - STUB, y + 16.0),
                pin("resetn", false, false, core_x - STUB, y + 32.0),
                pin(iface, true, false, core_x - STUB, y + 48.0),
            ],
        });
    }
    // Route: ports -> hub (straight), then hub (or ports) -> each core.
    let mut wires = Vec::new();
    let find = |name: &str| symbols.iter().find(|s| s.name == name);
    let route = |a: (f32, f32), b: (f32, f32), trunk: f32| -> Vec<[f32; 2]> {
        if (a.1 - b.1).abs() < 0.5 {
            vec![[a.0, a.1], [b.0, b.1]]
        } else {
            vec![[a.0, a.1], [trunk, a.1], [trunk, b.1], [b.0, b.1]]
        }
    };
    let hub = find("mm_interconnect").cloned();
    for (net, offset) in [("clk", 16.0), ("resetn", 30.0)] {
        let port = find(net).cloned().expect("port symbol");
        let port_tip = port.pin(net, true).expect("port pin");
        let src = match &hub {
            Some(h) => {
                let dst = h.pin(net, false).expect("hub input");
                wires.push(BdWire {
                    net: net.into(),
                    iface: false,
                    src: port.name.clone(),
                    dst: h.name.clone(),
                    pts: route(port_tip, dst, port_tip.0 + offset),
                });
                (h.name.clone(), h.pin(net, true).expect("hub output"))
            }
            None => (port.name.clone(), port_tip),
        };
        for c in symbols.iter().filter(|s| s.kind == SymKind::Core) {
            let dst = c.pin(net, false).expect("core pin");
            wires.push(BdWire {
                net: net.into(),
                iface: false,
                src: src.0.clone(),
                dst: c.name.clone(),
                pts: route(src.1, dst, src.1.0 + offset),
            });
        }
    }
    let mut nets = 2;
    if let Some(h) = &hub {
        let m = h.pin("M_MM", true).expect("hub master");
        for c in symbols
            .iter()
            .filter(|s| s.kind == SymKind::Core && s.bus == "Helion-MM")
        {
            let dst = c.pin("S_MM", false).expect("slave pin");
            wires.push(BdWire {
                net: format!("{}_mm", c.name),
                iface: true,
                src: h.name.clone(),
                dst: c.name.clone(),
                pts: route(m, dst, m.0 + 44.0),
            });
            nets += 1;
        }
    }
    let w = symbols
        .iter()
        .map(|s| s.x + s.w + MARGIN)
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
    let selected = sy.kind == SymKind::Core && s.ip.selected.as_deref() == Some(sy.core.as_str());
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
        let core = sy.core.clone();
        node = node
            .role("button")
            .label(format!("{} ({})", sy.name, sy.core))
            .focus_selected(selected)
            .focusable(true)
            .hover(t.selection)
            .on_press(move |s: &mut AppState| {
                let _ = select_ip(s, &core);
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
