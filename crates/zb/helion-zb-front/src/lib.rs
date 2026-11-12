//! The SystemVerilog and VHDL front ends as they were when Helion first ran
//! on a ZedBoard, kept unchanged so that flow stays reproducible, with their
//! netlists converted to the current IR. `crates/zb/README.md` has the
//! background.
//!
//! Use the current front ends (`helion_sv::synth_sv_auto_files`) for new
//! work. This crate is the reference they are checked against, and the only
//! path that infers carry chains (`CARRY4`) from arithmetic today.

use helion_ir::{intern, Attrs, Cell, CellKind, Design, Endpoint, Instance, Net, Port, PortDir};

/// Convert a netlist from the frozen IR to the current one. Names, pins and
/// attributes carry over unchanged.
pub fn to_current(d: &zb_ir::Design) -> Design {
    let attrs = |a: &zb_ir::Attrs| Attrs { map: a.map.clone() };
    let mut out = Design::new(&d.name);
    out.attrs = attrs(&d.attrs);
    out.ports = d
        .ports
        .iter()
        .map(|p| Port {
            name: intern(&p.name),
            dir: match p.dir {
                zb_ir::PortDir::In => PortDir::In,
                zb_ir::PortDir::Out => PortDir::Out,
                zb_ir::PortDir::Inout => PortDir::Inout,
            },
            attrs: attrs(&p.attrs),
        })
        .collect();
    out.cells = d
        .cells
        .iter()
        .map(|c| Cell {
            name: intern(&c.name),
            kind: match &c.kind {
                zb_ir::CellKind::Lut6 { init } => CellKind::Lut6 { init: *init },
                zb_ir::CellKind::Hff => CellKind::Hff,
                zb_ir::CellKind::IobOut => CellKind::IobOut,
                zb_ir::CellKind::IobIn => CellKind::IobIn,
                zb_ir::CellKind::ClockBuf => CellKind::ClockBuf,
                zb_ir::CellKind::Mac27 => CellKind::Mac27,
                zb_ir::CellKind::Ila { net } => CellKind::Ila { net: intern(net) },
                zb_ir::CellKind::Bram18 => CellKind::Bram18,
                zb_ir::CellKind::Carry4 => CellKind::Carry4,
                zb_ir::CellKind::BlackBox { module } => CellKind::BlackBox { module: intern(module) },
            },
            attrs: attrs(&c.attrs),
        })
        .collect();
    out.nets = d
        .nets
        .iter()
        .map(|n| Net {
            name: intern(&n.name),
            endpoints: n.endpoints.iter().map(|e| Endpoint { cell: intern(&e.cell), pin: intern(&e.pin) }).collect(),
            attrs: attrs(&n.attrs),
        })
        .collect();
    out.instances = d
        .instances
        .iter()
        .map(|i| Instance {
            name: intern(&i.name),
            module: intern(&i.module),
            conns: i.conns.iter().map(|(a, b)| (intern(a), intern(b))).collect(),
            attrs: attrs(&i.attrs),
        })
        .collect();
    out.rebuild_indexes();
    out
}

/// Map multiplies to DSP blocks (`true`) or to LUT logic (`false`).
pub fn set_dsp_inference(on: bool) {
    zb_sv::set_dsp_inference(on);
}

/// Map adders, subtractors and comparators to carry chains (7-series
/// `CARRY4`) instead of LUT logic, for both languages.
pub fn set_carry_arith(on: bool) {
    zb_vhdl::set_carry_arith(on);
}

/// SystemVerilog source to a netlist.
pub fn synth_sv(source: &str, origin: &str) -> Result<Design, String> {
    zb_sv::synth_sv(source, origin).map(|d| to_current(&d))
}

/// SystemVerilog source to a netlist, with whole-register increments on
/// carry chains (the form the ZedBoard bring-up tests compare with Vivado).
pub fn synth_sv_carry(source: &str, origin: &str) -> Result<Design, String> {
    zb_sv::synth_sv_carry(source, origin).map(|d| to_current(&d))
}

/// VHDL source to a netlist. `top` names the top entity (`None`: the last
/// entity nothing else instantiates).
pub fn synth_vhdl_top(source: &str, top: Option<&str>) -> Result<Design, String> {
    zb_vhdl::synth_vhdl_top(source, top).map(|d| to_current(&d))
}

/// VHDL source to flat SystemVerilog, plus the top-level names that had to
/// be renamed because they are SystemVerilog keywords, as `(VHDL name, emitted name)`.
pub fn vhdl_to_sv(source: &str, top: Option<&str>) -> Result<(String, Vec<(String, String)>), String> {
    zb_vhdl::v2::translate_full(source, top)
}
