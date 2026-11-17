//! Block RAM and DSP fill on the ZedBoard part (xc7z020), pack and place
//! only: every RAMB18 and DSP48E1 gets a site of its own across all the
//! columns, their clock pins move onto the global buffer, every data pin is
//! a route endpoint, and a driver shared by many blocks is copied and
//! placed beside its blocks.
//!
//! Guards HELION_GAPS.md G9 (placement stopped after 16 blocks), G10 (no
//! BRAM/DSP pin endpoints), and the clock-pin and fanout gaps found with them.

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;
use std::collections::HashSet;

const RAMS: usize = 240;
const DSPS: usize = 192;

fn fill() -> Design {
    let mut d = Design::new("fill");
    d.add_port("clk", PortDir::In);
    for port in d.ports.iter_mut() {
        port.attrs.set("LOC", "Y9"); // the ZedBoard's 100 MHz oscillator
    }
    // One flop drives an address bit of every RAM.
    d.add_cell("a_reg", CellKind::Hff);
    d.connect("a_d", "a_reg", "D");
    d.connect("a", "a_reg", "Q");
    d.connect("clk", "a_reg", "CLK");
    d.add_cell("u_inv", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
    d.connect("a", "u_inv", "I0");
    d.connect("a_d", "u_inv", "O");
    for i in 0..RAMS {
        let r = format!("ram{i}");
        d.add_cell(&r, CellKind::BlackBox { module: intern("RAMB18E1") });
        d.set_cell_attr(&r, "READ_WIDTH_A", "18").unwrap();
        d.set_cell_attr(&r, "WRITE_WIDTH_B", "18").unwrap();
        d.connect("clk", &r, "CLKARDCLK");
        d.connect("a", &r, "ADDRARDADDR4");
        d.connect(&format!("do{i}"), &r, "DOADO0");
    }
    for i in 0..DSPS {
        let m = format!("dsp{i}");
        d.add_cell(&m, CellKind::BlackBox { module: intern("DSP48E1") });
        d.connect("clk", &m, "CLK");
        d.connect(&format!("p{i}"), &m, "P0");
    }
    d
}

#[test]
fn zedboard_fill_places_every_ramb18_and_dsp() {
    let backend = Xilinx7Backend::open("xc7z020").unwrap();
    let packed = backend.pack(&fill()).unwrap();
    assert_eq!(packed.brams.len(), RAMS);
    assert_eq!(packed.dsps.len(), DSPS);
    for b in &packed.brams {
        assert!(b.pins.iter().any(|(p, n, _)| p == "CLKARDCLK" && n == "clk_bufg"), "{}: clock not on the BUFG: {:?}", b.name, b.pins);
    }
    for m in &packed.dsps {
        assert!(m.pins.iter().any(|(p, n, _)| p == "CLK" && n == "clk_bufg"), "{}: clock not on the BUFG", m.name);
    }
    let placed = backend.place(&packed).unwrap();
    let sites: HashSet<&str> = placed.brams.iter().map(|b| b.site.as_str()).chain(placed.dsps.iter().map(|m| m.site.as_str())).collect();
    assert_eq!(placed.brams.len(), RAMS);
    assert_eq!(placed.dsps.len(), DSPS);
    assert_eq!(sites.len(), RAMS + DSPS, "two blocks on one site");

    // Every RAM's address, clock and output pin is a route endpoint, and
    // every DSP's clock and output.
    let eps = helion_xilinx7::bram::endpoints(&placed).unwrap();
    for b in &placed.brams {
        let on = |pin_ok: &dyn Fn(&str) -> bool| eps.iter().any(|(_, s, p, _)| *s == b.site && pin_ok(p));
        assert!(on(&|p| p == "ADDRARDADDR4"), "{}: no address endpoint", b.site);
        assert!(on(&|p| p == "RDCLK" || p == "CLKARDCLK"), "{}: no clock endpoint", b.site);
        assert!(on(&|p| p == "DO0" || p == "DOADO0"), "{}: no output endpoint", b.site);
    }
    let deps = helion_xilinx7::dsp::endpoints(&placed);
    for m in &placed.dsps {
        assert!(deps.iter().any(|(_, s, p, out)| *s == m.site && p == "P0" && *out), "{}: no P0 endpoint", m.site);
        assert!(deps.iter().any(|(n, s, p, _)| *s == m.site && p == "CLK" && n == "clk_bufg"), "{}: no CLK endpoint", m.site);
    }

    // The address flop is copied per group of RAMs, and each copy sits near
    // the RAMs it drives: within 20 tiles of the farthest of them (a group
    // of 8 RAMB18 is 20 rows tall; copies ended up 40 to 157 tiles away
    // when they shared slices or groups spanned two columns).
    let dev = backend.device();
    let gxy = |tile: &str| {
        let t = dev.tile(dev.tile_by_name(tile).unwrap()).unwrap();
        (t.grid_x, t.grid_y)
    };
    let mut copies = 0;
    for f in placed.packed.ffs.iter().filter(|f| f.name.starts_with("a_reg_rep")) {
        let pf = placed.ffs.iter().find(|p| p.name == f.name).unwrap();
        let (fx, fy) = gxy(&pf.tile);
        let far = placed
            .packed
            .brams
            .iter()
            .filter(|b| b.pins.iter().any(|(p, n, _)| p == "ADDRARDADDR4" && *n == f.q_net))
            .map(|b| {
                let (bx, by) = gxy(&placed.brams.iter().find(|p| p.name == b.name).unwrap().tile);
                (bx - fx).abs() + (by - fy).abs()
            })
            .max()
            .unwrap();
        assert!(far <= 20, "{} at {} is {far} tiles from one of its RAMs", f.name, pf.tile);
        copies += 1;
    }
    // Groups of at most 8 RAMs, never spanning two columns.
    assert!(copies >= RAMS / 8 && copies <= RAMS / 8 + 8, "{copies} copies");
}

/// Inputs held at 0 are routed from the INT tile's TIEOFF, so no signal can
/// pass through the wires that feed them (a signal on `FAN_ALT` once drove
/// a RAM's write enable behind its always-on pip), and the design goes all
/// the way to FASM.
#[test]
fn ramb18_and_dsp_inputs_tied_to_0_are_routed_tieoffs() {
    let mut d = Design::new("ties");
    d.add_port("clk", PortDir::In);
    for port in d.ports.iter_mut() {
        port.attrs.set("LOC", "Y9");
    }
    d.add_cell("a_reg", CellKind::Hff);
    d.connect("a_d", "a_reg", "D");
    d.connect("a", "a_reg", "Q");
    d.connect("clk", "a_reg", "CLK");
    d.add_cell("u_inv", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
    d.connect("a", "u_inv", "I0");
    d.connect("do0", "u_inv", "I1");
    d.connect("p0", "u_inv", "I2");
    d.connect("a_d", "u_inv", "O");
    d.add_cell("ram", CellKind::BlackBox { module: intern("RAMB18E1") });
    d.set_cell_attr("ram", "READ_WIDTH_A", "18").unwrap();
    d.set_cell_attr("ram", "WRITE_WIDTH_B", "18").unwrap();
    d.connect("clk", "ram", "CLKARDCLK");
    d.connect("a", "ram", "ADDRARDADDR4");
    d.connect("do0", "ram", "DOADO0");
    for pin in ["WEBWE0", "WEBWE1", "DIADI0", "RSTRAMARSTRAM"] {
        d.connect("const0", "ram", pin);
    }
    d.add_cell("dsp", CellKind::BlackBox { module: intern("DSP48E1") });
    d.connect("clk", "dsp", "CLK");
    d.connect("a", "dsp", "A0");
    d.connect("p0", "dsp", "P0");

    let backend = Xilinx7Backend::open("xc7z020").unwrap();
    let placed = backend.place(&backend.pack(&d).unwrap()).unwrap();
    let ram_site = &placed.brams[0].site;
    let loads: Vec<&(String, String, String, bool)> = placed.tie_eps.iter().filter(|e| !e.3).collect();
    // WEBWE0/1 cover four site lanes; DIADI0 and the reset one pin each.
    assert_eq!(loads.iter().filter(|e| e.1 == *ram_site).count(), 6, "{:?}", placed.tie_eps);
    // The DSP's unconnected C, D-less pins and resets held at 0 are tied too.
    assert!(loads.iter().any(|e| e.1 == placed.dsps[0].site && e.2 == "C0"));
    for e in placed.tie_eps.iter().filter(|e| e.3) {
        assert!(e.0.starts_with("tie0_INT_") && e.1.starts_with("TIEOFF_") && e.2 == "HARD0", "{e:?}");
    }
    let routed = backend.route(&placed).unwrap();
    let tie_nets: Vec<_> = routed.nets.iter().filter(|n| n.net.starts_with("tie0_")).collect();
    assert!(!tie_nets.is_empty());
    let bs = backend.bitstream(&routed).unwrap();
    assert!(bs.fasm.iter().any(|l| l.contains(".GFAN") && l.ends_with("GND_WIRE")), "no ground in the FASM");
}
