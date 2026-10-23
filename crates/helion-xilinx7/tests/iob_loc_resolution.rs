//! IOB constraints must survive packing.
//!
//! The `PAD` pin binds to a net whose name normally equals the port name; a
//! user net can alias it. The lookup used to take the raw PAD net name, and
//! when it did not match a port the result was `None` -- LOC and IOSTANDARD
//! were silently replaced by defaults and the pin landed somewhere else.
use helion_ir::{Attrs, CellKind, Design, PortDir};

fn design(pad_net: &str) -> Design {
    let mut d = Design::new("t");
    let mut pa = Attrs::default();
    pa.set("LOC", "E3");
    pa.set("IOSTANDARD", "LVCMOS18");
    d.ports.push(helion_ir::Port { name: "led0".into(), dir: PortDir::Out, attrs: pa });
    d.add_cell("u_lut", CellKind::Lut6 { init: 1 });
    d.connect("q", "u_lut", "O");
    d.add_cell("u_io", CellKind::IobOut);
    d.connect("q", "u_io", "I");
    d.connect(pad_net, "u_io", "PAD");
    d
}

#[test]
fn matching_pad_net_finds_constraints() {
    let packed = helion_xilinx7::pack::pack(&design("led0")).expect("pack");
    let io = &packed.iobs[0];
    assert_eq!(io.port, "led0");
    assert_eq!(io.loc.as_deref(), Some("E3"));
    assert_eq!(io.iostandard, "LVCMOS18");
}

#[test]
fn aliased_pad_net_still_finds_constraints() {
    // A net named `led0_pad` is added that also touches the port name, as a
    // real elaboration would when a port net is aliased.
    let mut d = design("led0_pad");
    d.connect("led0", "u_io2", "PAD");
    d.add_cell("u_io2", CellKind::IobOut);
    d.connect("q", "u_io2", "I");
    let packed = helion_xilinx7::pack::pack(&d).expect("pack");
    // The aliasing IOB resolves through the port-named net.
    let io = packed
        .iobs
        .iter()
        .find(|i| i.name == "u_io2")
        .expect("u_io2 packed");
    assert_eq!(io.port, "led0", "aliased PAD net must resolve to the port");
    assert_eq!(io.loc.as_deref(), Some("E3"), "LOC must not be dropped");
}
