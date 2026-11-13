//! Timing through block RAMs and multipliers on Helion's own fabric: a
//! RAM's inputs and a registered multiplier's inputs are setup endpoints
//! (a registered multiplier's A/B/C through its product), a RAM's outputs
//! start at its clock-to-DO, and a combinational multiplier is an arc from
//! A/B/C to P inside the path.

use helion_backend::TimingModel;
use helion_device::Device;
use helion_ir::{CellKind, Design};
use helion_sta::graph::{analyze_hl10t, GraphTiming};

struct Nl {
    d: Design,
    k: usize,
}

impl Nl {
    fn new() -> Self {
        let mut d = Design::new("top");
        d.add_port("clk", helion_ir::PortDir::In);
        Nl { d, k: 0 }
    }
    fn fresh(&mut self, p: &str) -> String {
        self.k += 1;
        format!("{p}{}", self.k)
    }
    fn inputs(&mut self, name: &str, n: usize) -> Vec<String> {
        self.d.add_port(name, helion_ir::PortDir::In);
        (0..n).map(|i| format!("{name}_{i}")).collect()
    }
    fn lut(&mut self, ins: &[&str], f: impl Fn(u32) -> bool) -> String {
        let mask = (1u32 << ins.len()) - 1;
        let init = (0..64u32).fold(0u64, |a, x| a | ((f(x & mask) as u64) << x));
        let (cell, out) = (self.fresh("lut"), self.fresh("n"));
        self.d.add_cell(&cell, CellKind::Lut6 { init });
        for (i, n) in ins.iter().enumerate() {
            self.d.connect(n, &cell, &format!("I{i}"));
        }
        self.d.connect(&out, &cell, "O");
        out
    }
    /// A chain of `k` two-input LUTs from `a` (mixed with `b` at each step).
    fn chain(&mut self, a: &str, b: &str, k: usize) -> String {
        let mut cur = a.to_string();
        for _ in 0..k {
            cur = self.lut(&[&cur, b], |x| (x & 1) ^ (x >> 1) == 1);
        }
        cur
    }
    fn ff(&mut self, d: &str) -> String {
        let (cell, q) = (self.fresh("ff"), self.fresh("q"));
        self.d.add_cell(&cell, CellKind::Hff);
        self.d.connect("clk", &cell, "CLK");
        self.d.connect(d, &cell, "D");
        self.d.connect(&q, &cell, "Q");
        q
    }
    fn output(&mut self, name: &str, net: &str) {
        self.d.add_port(name, helion_ir::PortDir::Out);
        let cell = format!("pad_{name}");
        self.d.add_cell(&cell, CellKind::IobOut);
        self.d.connect(net, &cell, "I");
        self.d.connect(name, &cell, "PAD");
    }
}

fn timing(d: &Design) -> GraphTiming {
    let dev = Device::load_part("HL10S-C64-1").unwrap();
    let packed = helion_pack::pack(d, &dev).unwrap();
    let placed = helion_place::place(&packed, &dev).unwrap();
    let routed = helion_route::route(&placed, &dev).unwrap();
    analyze_hl10t(&routed, 1_000_000).expect("routed from its connection list")
}

fn names(g: &GraphTiming) -> Vec<&str> {
    g.worst_path.iter().map(|p| p.0.as_str()).collect()
}

/// Arrival of the path step whose name contains `what`.
fn at(g: &GraphTiming, what: &str) -> i64 {
    g.worst_path.iter().find(|p| p.0.contains(what)).unwrap_or_else(|| panic!("no {what} in {:?}", g.worst_path)).1
}

/// A RAM whose write address bit 3 comes through a chain of eight LUTs:
/// that input is the slowest endpoint, and every connected RAM input is
/// an endpoint. Its DO0 reaches a flop through two LUTs, timed from the
/// RAM's clock-to-DO.
#[test]
fn ram_inputs_are_setup_endpoints() {
    let tm = TimingModel::hl10t();
    let mut n = Nl::new();
    let a = n.inputs("a", 12);
    n.d.add_cell("ram", CellKind::Bram18);
    n.d.connect("clk", "ram", "CLK");
    for i in 0..10 {
        n.d.connect(&a[i], "ram", &format!("RADDR{i}"));
        if i != 3 {
            n.d.connect(&a[i], "ram", &format!("WADDR{i}"));
        }
    }
    let slow = n.chain(&a[10], &a[11], 8);
    n.d.connect(&slow, "ram", "WADDR3");
    n.d.connect(&a[11], "ram", "WE");
    n.d.connect(&a[10], "ram", "DI0");
    n.d.connect("do0", "ram", "DO0");
    let x = n.lut(&["do0", &a[1]], |x| x == 3);
    let y = n.lut(&[&x, &a[2]], |x| x == 1);
    let q = n.ff(&y);
    n.output("o", &q);
    let g = timing(&n.d);
    // 22 RAM inputs (RADDR0..9, WADDR0..9, WE, DI0), one flop, one pad.
    assert_eq!(g.endpoints, 22 + 1 + 1, "{g:?}");
    let path = names(&g);
    assert_eq!(*path.last().unwrap(), "ram WADDR3 (setup)", "{path:?}");
    assert_eq!(path.iter().filter(|p| p.starts_with("lut")).count(), 8, "{path:?}");
    assert_eq!(path[0], "input a_10", "{path:?}");
    assert!(g.reg_ps >= tm.iob_ps + 8 * tm.lut_ps + tm.setup_ps, "{g:?}");
    assert_eq!(g.reg_ps, g.worst_path.last().unwrap().1);
    assert!(g.levels >= 8);
}

/// A RAM's output starts its path at the RAM's clock-to-DO.
#[test]
fn ram_output_starts_at_clock_to_do() {
    let tm = TimingModel::hl10t();
    let mut n = Nl::new();
    let a = n.inputs("a", 2);
    n.d.add_cell("ram", CellKind::Bram18);
    n.d.connect("clk", "ram", "CLK");
    n.d.connect(&a[0], "ram", "RADDR0");
    n.d.connect("do5", "ram", "DO5");
    let y = n.chain("do5", &a[1], 6);
    let q = n.ff(&y);
    n.output("o", &q);
    let g = timing(&n.d);
    let path = names(&g);
    assert_eq!(path[0], "ram (clock to DO5)", "{path:?}");
    assert_eq!(g.worst_path[0].1, tm.bram_ckq_ps);
    assert!(path.last().unwrap().ends_with("(setup)") && path.last().unwrap().starts_with("ff"), "{path:?}");
    assert!(g.reg_ps >= tm.bram_ckq_ps + 6 * tm.lut_ps + tm.setup_ps, "{g:?}");
}

fn mul(n: &mut Nl, preg: bool, a: &[String], b: &[String]) {
    n.d.add_cell("mul", CellKind::Mac27);
    n.d.set_cell_attr("mul", "PREG", if preg { "1" } else { "0" }).unwrap();
    if preg {
        n.d.connect("clk", "mul", "CLK");
    }
    for i in 0..a.len() {
        n.d.connect(&a[i], "mul", &format!("A{i}"));
    }
    for i in 0..b.len() {
        n.d.connect(&b[i], "mul", &format!("B{i}"));
    }
    for i in 0..4 {
        n.d.connect(&format!("p{i}"), "mul", &format!("P{i}"));
    }
}

/// A registered multiplier: A and B are endpoints through the product
/// (the multiplier's delay before the register's setup), CE is one
/// without it, and P starts a path at clock-to-Q.
#[test]
fn registered_multiplier_inputs_are_endpoints_through_the_product() {
    let tm = TimingModel::hl10t();
    let mut n = Nl::new();
    let a = n.inputs("a", 4);
    let b = n.inputs("b", 4);
    let ce = n.inputs("ce", 1);
    mul(&mut n, true, &a, &b);
    n.d.connect(&ce[0], "mul", "CE");
    for i in 0..4 {
        n.output(&format!("o{i}"), &format!("p{i}"));
    }
    let g = timing(&n.d);
    // A0..3, B0..3, CE; four pads.
    assert_eq!(g.endpoints, 9 + 4, "{g:?}");
    let path = names(&g);
    // A pad into A or B, plus the product, is slower than a pad into CE.
    assert!(path.last().unwrap().starts_with("mul A") || path.last().unwrap().starts_with("mul B"), "{path:?}");
    assert!(path.last().unwrap().ends_with("(setup)"), "{path:?}");
    let through = at(&g, "(multiplier, ");
    assert!(through >= tm.iob_ps + tm.mac_ps, "{g:?}");
    assert_eq!(g.reg_ps, through + tm.setup_ps);
    assert!(g.out_ps < tm.ff_ckq_ps + 64 * tm.hop_ps + tm.iob_ps, "P starts at clock to Q: {g:?}");
}

/// A combinational multiplier fed by flops, its P through a LUT into a
/// flop: one path, flop -> multiplier -> LUT -> flop, the multiplier's
/// delay in it, and no endpoint at the multiplier.
#[test]
fn combinational_multiplier_is_an_arc() {
    let tm = TimingModel::hl10t();
    let mut n = Nl::new();
    let x = n.inputs("x", 4);
    let a: Vec<String> = x.iter().map(|v| n.ff(v)).collect();
    let b: Vec<String> = x.iter().rev().map(|v| n.ff(v)).collect();
    mul(&mut n, false, &a, &b);
    let y = n.lut(&["p2", "p3"], |v| v == 1);
    let q = n.ff(&y);
    n.output("o", &q);
    let g = timing(&n.d);
    // Nine flops and one pad; nothing at the multiplier.
    assert_eq!(g.endpoints, 9 + 1, "{g:?}");
    let path = names(&g);
    assert_eq!(path.len(), 4, "{path:?}");
    assert!(path[0].starts_with("ff") && path[0].ends_with("(clock to Q)"), "{path:?}");
    assert!(path[1].starts_with("mul (multiplier, to P"), "{path:?}");
    assert!(path[2].starts_with("lut"), "{path:?}");
    assert!(path[3].starts_with("ff") && path[3].ends_with("(setup)"), "{path:?}");
    let p = g.worst_path[1].1;
    assert!(p >= tm.ff_ckq_ps + tm.mac_ps, "{g:?}");
    assert!(g.reg_ps >= p + tm.lut_ps + tm.setup_ps, "{g:?}");
    assert_eq!(g.levels, 1);
}

/// The reports on a design whose only registers are blocks: a registered
/// multiplier between pads, and a RAM between pads. They have timing (it
/// used to be "no timing", WNS 0 and no endpoints, as for an empty
/// shell), it is the graph's, and so is the endpoint count TNS sums over.
#[test]
fn designs_whose_only_registers_are_blocks_have_timing() {
    let dev = Device::load_part("HL10S-C64-1").unwrap();
    let mut m = Nl::new();
    let a = m.inputs("a", 4);
    let b = m.inputs("b", 4);
    mul(&mut m, true, &a, &b);
    for i in 0..4 {
        m.output(&format!("o{i}"), &format!("p{i}"));
    }
    let mut r = Nl::new();
    let a = r.inputs("a", 10);
    r.d.add_cell("ram", CellKind::Bram18);
    r.d.connect("clk", "ram", "CLK");
    for i in 0..10 {
        r.d.connect(&a[i], "ram", &format!("RADDR{i}"));
        r.d.connect(&a[9 - i], "ram", &format!("WADDR{i}"));
    }
    r.d.connect(&a[0], "ram", "WE");
    r.d.connect(&a[1], "ram", "DI0");
    r.d.connect("do0", "ram", "DO0");
    r.output("o", "do0");
    for (what, d) in [("mul", &m.d), ("ram", &r.d)] {
        assert!(!d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
        let packed = helion_pack::pack(d, &dev).unwrap();
        let placed = helion_place::place(&packed, &dev).unwrap();
        let routed = helion_route::route(&placed, &dev).unwrap();
        let mut clks = Vec::new();
        helion_sta::create_clock(&mut clks, "clk", 500, "clk");
        let g = helion_sta::graph::analyze_hl10t(&routed, 500).unwrap();
        assert!(g.failing > 0, "{what}: {g:?}");
        let t = helion_sta::report_timing_routed(d, &routed, &clks).unwrap();
        assert_eq!(t.endpoints, g.endpoints, "{what}");
        assert!(t.wns_ps < 0 && t.tns_ps < 0, "{what}: {t:?}");
        assert!(t.setup_ps >= g.reg_ps.min(g.out_ps), "{what}: {t:?} {g:?}");
        // Without placement too: the blocks are endpoints.
        assert!(helion_sta::report_timing(d, &clks).unwrap().endpoints > 0, "{what}");
    }
}
