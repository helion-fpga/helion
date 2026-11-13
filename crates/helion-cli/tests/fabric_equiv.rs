//! End-to-end proof that the HL10T flow implements the design it was given:
//! SystemVerilog -> netlist -> pack -> place -> route -> bitstream, the
//! bitstream programmed into the fabric model, and the fabric compared pad
//! by pad, cycle by cycle, against a simulation of the netlist under random
//! inputs.

use helion_bits::bitgen;
use helion_device::Device;
use helion_fabric::Fabric;
use helion_ir::{resolve, CellKind, Design, Spur};
use helion_pack::pack;
use helion_place::place;
use helion_route::route;
use std::collections::HashMap;

/// Reference simulation of the mapped netlist: LUTs, flops with their
/// clock nets and edges, block RAMs and multipliers, output pads.
struct NetSim {
    luts: Vec<(u64, [Option<Spur>; 6], Spur)>,
    /// (D, Q, clock net, falling edge, asynchronous reset: net and level)
    ffs: Vec<(Spur, Spur, Option<Spur>, bool, Option<(Spur, bool)>)>,
    rams: Vec<Ram>,
    muls: Vec<Mul>,
    val: HashMap<Spur, bool>,
    /// Clock net values when last looked at.
    prev: HashMap<Spur, bool>,
}

/// A `Bram18` as `helion_ir::CellKind::Bram18` defines it: 1024 x 16,
/// read-first, read latency 1, rising edge of CLK.
struct Ram {
    clk: Option<Spur>,
    di: Vec<Option<Spur>>,
    waddr: Vec<Option<Spur>>,
    raddr: Vec<Option<Spur>>,
    we: Option<Spur>,
    re: Option<Spur>,
    dout: Vec<Option<Spur>>,
    mem: Vec<u16>,
    q: u16,
}

/// A `Mac27`: P = A*B (+C), optionally registered.
struct Mul {
    a: Vec<Option<Spur>>,
    b: Vec<Option<Spur>>,
    c: Vec<Option<Spur>>,
    ce: Option<Spur>,
    rst: Option<Spur>,
    clk: Option<Spur>,
    p: Vec<Option<Spur>>,
    signed: bool,
    preg: bool,
    reg: u32,
}

impl NetSim {
    fn new(d: &Design) -> Self {
        let pins = d.pin_index();
        let mut luts = Vec::new();
        let mut ffs = Vec::new();
        let mut rams = Vec::new();
        let mut muls = Vec::new();
        for c in &d.cells {
            let name = resolve(c.name);
            match c.kind {
                CellKind::Lut6 { init } => {
                    let mut ins = [None; 6];
                    for (i, slot) in ins.iter_mut().enumerate() {
                        *slot = pins.net_on(name, &format!("I{i}"));
                    }
                    if let Some(o) = pins.net_on(name, "O") {
                        luts.push((init, ins, o));
                    }
                }
                CellKind::Hff => {
                    if let (Some(dn), Some(q)) = (pins.net_on(name, "D"), pins.net_on(name, "Q")) {
                        let arst = pins.net_on(name, "ARST").map(|n| (n, c.attrs.get("ARST_LEVEL") != Some("0")));
                        ffs.push((dn, q, pins.net_on(name, "CLK"), c.attrs.get("CLK_INV") == Some("1"), arst));
                    }
                }
                CellKind::Bram18 => {
                    let bus = |p: &str, n: usize| -> Vec<Option<Spur>> { (0..n).map(|i| pins.net_on(name, &format!("{p}{i}"))).collect() };
                    let mut mem = vec![0u16; 1024];
                    let init = c.attrs.get("INIT").unwrap_or("");
                    for (w, t) in init.split(',').map(str::trim).filter(|t| !t.is_empty()).enumerate() {
                        let v = u64::from_str_radix(t.trim_start_matches("0x"), 16).expect("INIT word");
                        for k in 0..4 {
                            mem[4 * w + k] = (v >> (16 * k)) as u16;
                        }
                    }
                    rams.push(Ram {
                        clk: pins.net_on(name, "CLK"),
                        di: bus("DI", 16),
                        waddr: bus("WADDR", 10),
                        raddr: bus("RADDR", 10),
                        we: pins.net_on(name, "WE"),
                        re: pins.net_on(name, "RE"),
                        dout: bus("DO", 16),
                        mem,
                        q: 0,
                    });
                }
                CellKind::Mac27 => {
                    let bus = |p: &str, n: usize| -> Vec<Option<Spur>> { (0..n).map(|i| pins.net_on(name, &format!("{p}{i}"))).collect() };
                    muls.push(Mul {
                        a: bus("A", 16),
                        b: bus("B", 16),
                        c: bus("C", 32),
                        ce: pins.net_on(name, "CE"),
                        rst: pins.net_on(name, "RST"),
                        clk: pins.net_on(name, "CLK"),
                        p: bus("P", 32),
                        signed: c.attrs.get("SIGNED") == Some("1"),
                        preg: c.attrs.get("PREG") == Some("1"),
                        reg: 0,
                    });
                }
                _ => {}
            }
        }
        Self { luts, ffs, rams, muls, val: HashMap::new(), prev: HashMap::new() }
    }
    /// Value of a bus; an unconnected bit reads 0.
    fn word(&self, nets: &[Option<Spur>]) -> u64 {
        nets.iter().enumerate().fold(0, |a, (i, n)| a | ((n.map(|n| self.get(n)).unwrap_or(false) as u64) << i))
    }
    /// Drive a bus; returns whether a net changed.
    fn drive(val: &mut HashMap<Spur, bool>, nets: &[Option<Spur>], v: u64) -> bool {
        let mut changed = false;
        for (i, n) in nets.iter().enumerate() {
            if let Some(n) = n {
                let b = (v >> i) & 1 == 1;
                changed |= val.insert(*n, b) != Some(b);
            }
        }
        changed
    }
    fn product(&self, m: &Mul) -> u32 {
        let (a, b, c) = (self.word(&m.a) as u32, self.word(&m.b) as u32, self.word(&m.c) as u32);
        let p = if m.signed { (a as u16 as i16 as i32 * b as u16 as i16 as i32) as u32 } else { a * b };
        p.wrapping_add(c)
    }
    fn get(&self, n: Spur) -> bool {
        self.val.get(&n).copied().unwrap_or(false)
    }
    fn settle(&mut self) {
        for _ in 0..10_000 {
            let mut changed = false;
            for r in &self.rams {
                changed |= Self::drive(&mut self.val, &r.dout, r.q as u64);
            }
            for i in 0..self.muls.len() {
                let m = &self.muls[i];
                let v = if m.preg { m.reg } else { self.product(m) };
                changed |= Self::drive(&mut self.val, &self.muls[i].p, v as u64);
            }
            for (init, ins, o) in &self.luts {
                let mut addr = 0u64;
                for (i, n) in ins.iter().enumerate() {
                    if n.map(|n| self.val.get(&n).copied().unwrap_or(false)).unwrap_or(false) {
                        addr |= 1 << i;
                    }
                }
                let v = (init >> addr) & 1 == 1;
                if self.val.insert(*o, v) != Some(v) {
                    changed = true;
                }
            }
            if !changed {
                return;
            }
        }
        panic!("netlist has a combinational loop");
    }
    /// Settle, and let every flop whose clock net moved in its direction
    /// capture; repeat while that moves another clock (a divided clock).
    fn advance(&mut self) {
        for _ in 0..1024 {
            self.settle();
            // Asynchronous resets act first: a flop held in reset is 0
            // (the netlist stores every register relative to its reset
            // value) and ignores its clock.
            let held: Vec<bool> = self.ffs.iter().map(|f| f.4.map(|(n, level)| self.get(n) == level).unwrap_or(false)).collect();
            let mut reset_changed = false;
            for (f, h) in self.ffs.iter().zip(&held) {
                if *h && self.val.get(&f.1).copied().unwrap_or(false) {
                    reset_changed = true;
                }
            }
            if reset_changed {
                let qs: Vec<Spur> = self.ffs.iter().zip(&held).filter(|(_, h)| **h).map(|(f, _)| f.1).collect();
                for q in qs {
                    self.val.insert(q, false);
                }
                continue;
            }
            let mut next: Vec<(Spur, bool)> = Vec::new();
            let mut moved = false;
            for ((d, q, clk, falling, _), h) in self.ffs.iter().zip(&held) {
                let Some(clk) = clk else { continue };
                if *h {
                    // Still note the clock's movement below.
                    let cur = self.get(*clk);
                    if cur != self.prev.get(clk).copied().unwrap_or(cur) {
                        moved = true;
                    }
                    continue;
                }
                let cur = self.get(*clk);
                let was = self.prev.get(clk).copied().unwrap_or(cur);
                if cur != was {
                    moved = true;
                    if cur != *falling {
                        next.push((*q, self.get(*d)));
                    }
                }
            }
            // Block RAMs and registered multipliers: rising edge of CLK.
            let edge = |s: &Self, clk: Option<Spur>, moved: &mut bool| -> bool {
                let Some(clk) = clk else { return false };
                let cur = s.get(clk);
                let was = s.prev.get(&clk).copied().unwrap_or(cur);
                *moved |= cur != was;
                cur && !was
            };
            let mut ram_next: Vec<(usize, u16, Option<(usize, u16)>)> = Vec::new();
            for (i, r) in self.rams.iter().enumerate() {
                if !edge(self, r.clk, &mut moved) {
                    continue;
                }
                let re = r.re.map(|n| self.get(n)).unwrap_or(true);
                let we = r.we.map(|n| self.get(n)).unwrap_or(false);
                let q = if re { r.mem[self.word(&r.raddr) as usize] } else { r.q };
                ram_next.push((i, q, we.then(|| (self.word(&r.waddr) as usize, self.word(&r.di) as u16))));
            }
            let mut mul_next: Vec<(usize, u32)> = Vec::new();
            for (i, m) in self.muls.iter().enumerate() {
                if !m.preg || !edge(self, m.clk, &mut moved) {
                    continue;
                }
                let v = if m.rst.map(|n| self.get(n)).unwrap_or(false) {
                    0
                } else if m.ce.map(|n| self.get(n)).unwrap_or(true) {
                    self.product(m)
                } else {
                    m.reg
                };
                mul_next.push((i, v));
            }
            let clocks: Vec<Spur> = self.ffs.iter().filter_map(|f| f.2).chain(self.rams.iter().filter_map(|r| r.clk)).chain(self.muls.iter().filter_map(|m| m.clk)).collect();
            for c in clocks {
                let v = self.get(c);
                self.prev.insert(c, v);
            }
            if !moved {
                return;
            }
            for (q, v) in next {
                self.val.insert(q, v);
            }
            for (i, q, w) in ram_next {
                self.rams[i].q = q;
                if let Some((a, d)) = w {
                    self.rams[i].mem[a] = d;
                }
            }
            for (i, v) in mul_next {
                self.muls[i].reg = v;
            }
        }
        panic!("clocks do not settle");
    }
}

struct Rng(u64);
impl Rng {
    fn bit(&mut self) -> bool {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 & 1 == 1
    }
}

/// Build `src` for the part and compare `cycles` clock cycles. Returns
/// (wire-routed nets, switch-box wires) so tests can require real routing.
fn check(src: &str, part: &str, cycles: usize) -> (u32, usize) {
    check_with(src, part, cycles, |_| {}).expect("fabric matches the netlist")
}

/// As [`check`], with a hook to alter the routed design before bitgen. A
/// pad that differs from the netlist is returned as an error.
fn check_with(src: &str, part: &str, cycles: usize, tamper: impl Fn(&mut helion_route::Routed)) -> Result<(u32, usize), String> {
    let design = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    check_design(&design, part, cycles, tamper)
}

/// Pack, place, route and bitgen `design`, program the fabric model with
/// the bitstream and compare it with a simulation of the netlist.
fn check_design(design: &Design, part: &str, cycles: usize, tamper: impl Fn(&mut helion_route::Routed)) -> Result<(u32, usize), String> {
    let dev = Device::load_part(part).expect("part");
    let mut packed = pack(design, &dev).expect("pack");
    let fit = packed.fit_pads(dev.iob_sites_all().len());
    assert!(fit.undriven.is_empty() && fit.over_budget.is_empty(), "pads do not fit: {fit:?}");
    assert!(packed.conn.undriven.is_empty(), "undriven LUT inputs: {:?}", packed.conn.undriven);
    let placed = place(&packed, &dev).expect("place");
    let mut routed = route(&placed, &dev).expect("route");
    assert!(routed.tracks.active, "design was not routed from its connection list");
    tamper(&mut routed);
    let bits = bitgen(&dev, &routed).expect("bitgen");
    let mut fab = Fabric::new(&dev);
    fab.program(&bits).expect("program");
    fab.finish_startup();

    let mut sim = NetSim::new(design);
    let pins = design.pin_index();
    // Output pads: the netlist net on each IOB's I pin, and its site.
    let outs: Vec<(String, Spur, (u32, u32))> = routed
        .placed
        .packed
        .iobs
        .iter()
        .zip(&routed.placed.iob_sites)
        .map(|(io, site)| (io.cell.clone(), pins.net_on(&io.cell, "I").expect("pad net"), (site.x, site.y)))
        .collect();
    // Every distinct output net of the netlist has a pad.
    let want: std::collections::HashSet<Spur> = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .filter_map(|c| pins.net_on(resolve(c.name), "I"))
        .collect();
    let have: std::collections::HashSet<Spur> = outs.iter().map(|o| o.1).collect();
    assert_eq!(have, want, "an output pad was dropped");
    assert!(packed.conn.clock_problems.is_empty(), "clocking: {:?}", packed.conn.clock_problems);
    // Data pads, and the clock pads with the net they drive.
    let conn = &routed.placed.packed.conn;
    let mut ins: Vec<(Spur, (u32, u32))> = Vec::new();
    let mut clock_pads: Vec<(Spur, (u32, u32))> = Vec::new();
    for i in &routed.tracks.iob_in {
        let entry = (helion_ir::intern(&i.net), (i.x, i.y));
        if conn.inputs[i.input as usize].clock.is_some() {
            clock_pads.push(entry);
        } else {
            ins.push(entry);
        }
    }
    assert_eq!(routed.tracks.iob_in.len(), conn.inputs.len(), "an input pad was dropped");
    let implicit: Option<Spur> = conn.clocks.iter().find(|c| c.src == helion_pack::ClockSrc::Implicit).map(|c| helion_ir::intern(&c.net));

    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut clock_level = vec![false; clock_pads.len()];
    for cycle in 0..cycles {
        for (net, (x, y)) in &ins {
            let v = rng.bit();
            sim.val.insert(*net, v);
            fab.set_pad_in(*x, *y, v);
        }
        sim.advance();
        fab.settle();
        for (cell, net, (x, y)) in &outs {
            if fab.led_at(*x, *y) != sim.get(*net) {
                return Err(format!("cycle {cycle}: pad {cell} (net {}) differs", resolve(*net)));
            }
        }
        if let Some(clk) = implicit {
            // One cycle of the implicit clock.
            sim.val.insert(clk, true);
            sim.advance();
            sim.val.insert(clk, false);
            sim.advance();
            fab.step_user();
        }
        if !clock_pads.is_empty() {
            // One edge on one clock pad, chosen at random.
            let k = (0..4).fold(0usize, |a, _| a * 2 + rng.bit() as usize) % clock_pads.len();
            clock_level[k] = !clock_level[k];
            let (net, (x, y)) = clock_pads[k];
            sim.val.insert(net, clock_level[k]);
            fab.set_pad_in(x, y, clock_level[k]);
        }
    }
    Ok((routed.tracks.nets, routed.tracks.sb.len()))
}

const COUNTER: &str = r#"
module top(input logic clk, input logic en, input logic [3:0] a, output logic [7:0] q, output logic [3:0] y, output logic p);
  logic [7:0] cnt;
  always_ff @(posedge clk) if (en) cnt <= cnt + 8'd1;
  assign q = cnt;
  assign y = a ^ cnt[3:0];
  assign p = ^a;
endmodule
"#;

/// The comparison is not vacuous: a design with one wire cut, one wire
/// taken from the wrong neighbour, or one LUT input moved to another wire
/// no longer matches its netlist.
#[test]
fn broken_routes_are_detected() {
    let cut = |r: &mut helion_route::Routed| {
        r.tracks.sb.remove(0);
    };
    assert!(check_with(COUNTER, PART, 300, cut).is_err(), "a cut wire went unnoticed");
    let wrong_source = |r: &mut helion_route::Routed| {
        for w in r.tracks.sb.iter_mut() {
            w.sel ^= 1;
        }
    };
    assert!(check_with(COUNTER, PART, 300, wrong_source).is_err(), "wrong switch-box sources went unnoticed");
    let wrong_pin = |r: &mut helion_route::Routed| {
        for m in r.tracks.imux.iter_mut() {
            m.sel = (m.sel + 1) % 32;
        }
    };
    assert!(check_with(COUNTER, PART, 300, wrong_pin).is_err(), "LUT inputs on the wrong wires went unnoticed");
    let no_direct = |r: &mut helion_route::Routed| r.imux.clear();
    assert!(check_with(COUNTER, PART, 300, no_direct).is_err(), "missing direct connections went unnoticed");
}

const PART: &str = "HL10T-C32-1";

#[test]
fn counter_with_enable_and_comb_outputs() {
    let src = COUNTER;
    let (nets, wires) = check(src, PART, 300);
    assert!(nets > 0 && wires > 0, "inputs must arrive over wires");
}

#[test]
fn shift_register_and_input_logic() {
    let src = r#"
module top(input logic clk, input logic d, input logic sel, input logic [2:0] k, output logic [5:0] q, output logic m);
  logic [15:0] sr;
  always_ff @(posedge clk) sr <= {sr[14:0], d ^ (sel & sr[15])};
  assign q = {sr[15], sr[12], sr[9], sr[6], sr[3], sr[0]};
  assign m = sel ? sr[k] : (k[0] & k[1]) | (k[2] & d);
endmodule
"#;
    let (nets, _) = check(src, PART, 400);
    assert!(nets > 0);
}

#[test]
fn outputs_from_far_columns_and_lut_to_lut() {
    // Many outputs from few registers: pads sit under columns their drivers
    // are not in, and the mixing logic is several LUTs deep.
    let src = r#"
module top(input logic clk, input logic [3:0] a, input logic [3:0] b, output logic [11:0] o);
  logic [7:0] r, s;
  always_ff @(posedge clk) begin
    r <= {r[6:0], a[0] ^ b[3]} ^ {a, b};
    s <= s + r;
  end
  assign o[3:0] = (a & b) | (r[3:0] ^ s[7:4]);
  assign o[7:4] = s[3:0] ^ {r[7], r[5], r[3], r[1]};
  assign o[11:8] = (a + b) ^ r[7:4] ^ s[3:0];
endmodule
"#;
    let (nets, wires) = check(src, PART, 500);
    assert!(nets > 4 && wires > 8);
}

/// `examples/complex.sv` connects instance ports to bit-selects of a bus,
/// which the first-generation front end left without drivers.
#[test]
fn hierarchy_through_bus_bit_selects() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/complex.sv")).expect("examples/complex.sv");
    check(&src, PART, 300);
}

/// The whole Ibex core behind `examples/ibex_obs_wrap.sv`: 11.5k LUTs and
/// 2.2k flops on HL10S-C64-1, every connection routed, the bitstream run in
/// the fabric model against the netlist. (`tools/svcheck` shows that this
/// netlist equals the RTL.) Takes a few minutes: run with `--ignored`.
#[test]
#[ignore]
fn ibex_bitstream_equals_its_netlist() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/");
    helion_sv::set_sv_defines(&["VERILATOR"]);
    let files = [format!("{root}ysyx_ibex.sv"), format!("{root}ibex_obs_wrap.sv")];
    let paths: Vec<&std::path::Path> = files.iter().map(|f| std::path::Path::new(f.as_str())).collect();
    let design = helion_sv::synth_sv_v2_files(&paths, Some("ibex_obs_wrap")).expect("synth").design;
    let (nets, wires) = check_design(&design, "HL10S-C64-1", 60, |_| {}).expect("fabric matches the netlist");
    assert!(nets > 5000 && wires > 20000, "{nets} nets on {wires} wires");
}

/// Large enough for the placer's wirelength refinement and for real
/// congestion: a few hundred LUTs of mixing logic, several levels deep,
/// between wide registers.
#[test]
fn large_design_is_equivalent_after_refinement() {
    let src = r#"
module top(input logic clk, input logic [7:0] a, input logic [3:0] k, output logic [15:0] o);
  logic [31:0] r, s, t, u, v, w;
  always_ff @(posedge clk) begin
    r <= {r[30:0], a[0] ^ r[31] ^ r[21]} ^ {4{a}};
    s <= (s + r) ^ {t[15:0], t[31:16]};
    t <= (r & s) | (~r & {s[7:0], s[31:8]}) | ({32{k[0]}} & t);
    u <= (u + t) ^ {v[4:0], v[31:5]} ^ {8{k}};
    v <= (u & w) | (~u & r) | (s ^ {w[0], w[31:1]});
    w <= (w + v + {24'd0, a}) ^ t;
  end
  assign o = r[15:0] ^ s[31:16] ^ t[23:8] ^ u[15:0] ^ v[31:16] ^ w[20:5] ^ {4{k}};
endmodule
"#;
    let design = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    let dev = Device::load_part(PART).expect("part");
    let packed = pack(&design, &dev).expect("pack");
    assert!(packed.lutffs.len() >= helion_place::wl::MIN_CLUSTERS, "only {} clusters", packed.lutffs.len());
    let (nets, wires) = check(src, PART, 150);
    assert!(nets > 50 && wires > 100);
}

const TWO_CLOCKS: &str = r#"
module top(input logic clk_a, input logic clk_b, input logic [3:0] d, output logic [3:0] qa, output logic [3:0] qb, output logic [5:0] cnt);
  logic [3:0] a, s1, s2;
  logic [5:0] c;
  always_ff @(posedge clk_a) a <= a + d;
  always_ff @(posedge clk_b) begin
    s1 <= a;
    s2 <= s1;
    c <= c + {5'd0, ^s2};
  end
  assign qa = a; assign qb = s2; assign cnt = c;
endmodule
"#;

/// Two clock pins with a register crossing between them: each clock is its
/// own line with its own pad, and each CLB holds flops of one clock.
#[test]
fn two_clock_domains() {
    check(TWO_CLOCKS, PART, 600);
}

const DIVIDED: &str = r#"

module top(input logic clk, input logic [2:0] d, output logic [3:0] slow, output logic [2:0] fall, output logic [2:0] rise);
  logic [1:0] div;
  logic [3:0] s;
  logic [2:0] r, f;
  always_ff @(posedge clk) begin
    div <= div + 2'd1;
    r <= d;
  end
  always_ff @(negedge clk) f <= r ^ d;
  always_ff @(posedge div[1]) s <= s + {1'b0, d};
  assign slow = s; assign fall = f; assign rise = r;
endmodule
"#;

/// A clock divided by logic drives its own line; falling-edge flops sit on
/// the same line as rising-edge ones.
#[test]
fn divided_clock_and_falling_edge() {
    check(DIVIDED, PART, 800);
}

/// The clock tests are not vacuous: flops put on the wrong clock line, or
/// on the wrong edge, no longer match the netlist.
#[test]
fn wrong_clocking_is_detected() {
    let one_line = |r: &mut helion_route::Routed| {
        for f in r.placed.packed.conn.ff_clock.iter_mut().flatten() {
            f.0 = 0;
        }
    };
    assert!(check_with(TWO_CLOCKS, PART, 600, one_line).is_err(), "all flops on one clock went unnoticed");
    assert!(check_with(DIVIDED, PART, 800, one_line).is_err(), "the divided clock's flops on the main clock went unnoticed");
    let wrong_edge = |r: &mut helion_route::Routed| {
        for f in r.placed.packed.conn.ff_clock.iter_mut().flatten() {
            f.1 = false;
        }
    };
    assert!(check_with(DIVIDED, PART, 800, wrong_edge).is_err(), "falling-edge flops built as rising went unnoticed");
}

/// Falling-edge flops on the design's only clock: no pad is spent on it.
#[test]
fn falling_edge_on_the_implicit_clock() {
    let src = r#"
module top(input logic clk, input logic [3:0] d, output logic [3:0] p, output logic [3:0] n);
  logic [3:0] a, b;
  always_ff @(posedge clk) a <= a + d;
  always_ff @(negedge clk) b <= a ^ d;
  assign p = a; assign n = b;
endmodule
"#;
    let design = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    let dev = Device::load_part(PART).expect("part");
    let packed = pack(&design, &dev).expect("pack");
    assert_eq!(packed.conn.clocks.len(), 1);
    assert!(packed.conn.inputs.iter().all(|i| i.net != "clk"), "the implicit clock needs no pad");
    check(src, PART, 400);
}

/// More clocks than the fabric has lines is refused, by name.
#[test]
fn too_many_clocks_is_an_error() {
    let src = r#"
module top(input logic [4:0] ck, input logic d, output logic [4:0] q);
  for (genvar i = 0; i < 5; i++) begin : g
    always_ff @(posedge ck[i]) q[i] <= d ^ q[i];
  end
endmodule
"#;
    let design = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    let dev = Device::load_part(PART).expect("part");
    let packed = pack(&design, &dev).expect("pack");
    assert!(packed.conn.clock_problems.iter().any(|p| p.contains("5 clocks")), "{:?}", packed.conn.clock_problems);
}

/// An asynchronous reset is a real one: asserted between clock edges it
/// clears the flops at once, in the fabric as in the netlist, and flops
/// without a reset in the same design are left alone.
#[test]
fn asynchronous_reset() {
    let src = r#"
module top(input logic clk, input logic rst_n, input logic [3:0] d, output logic [3:0] q, output logic [3:0] free, output logic [3:0] pre);
  logic [3:0] a, f, p;
  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      a <= 4'd0;
      p <= 4'hA;
    end else begin
      a <= a + d;
      p <= p ^ d;
    end
  end
  always_ff @(posedge clk) f <= f + a;
  assign q = a; assign free = f; assign pre = p;
endmodule
"#;
    let design = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    assert!(design.cells.iter().any(|c| c.attrs.get("ARST_LEVEL") == Some("0")), "the reset is asynchronous in the netlist");
    check(src, PART, 800);
    // Flops that ignore the reset no longer match.
    let no_reset = |r: &mut helion_route::Routed| {
        for f in r.placed.packed.conn.ff_reset.iter_mut() {
            *f = None;
        }
    };
    assert!(check_with(src, PART, 800, no_reset).is_err(), "a missing asynchronous reset went unnoticed");
}

/// More pins than the bottom row has pads: outputs and inputs spill onto
/// the top, left and right edges, which connect through the interconnect.
#[test]
fn pads_on_all_four_sides() {
    let src = r#"
module top(input logic clk, input logic [39:0] a, output logic [47:0] o);
  logic [47:0] r;
  always_ff @(posedge clk) r <= {r[46:0], ^a} ^ {8'd0, a} ^ {a[7:0], 40'd0};
  assign o = r ^ {a, a[7:0]};
endmodule
"#;
    let design = helion_sv::synth_sv_v2(src, "t.sv", None).expect("synth").design;
    let dev = Device::load_part(PART).expect("part");
    let bottom = dev.iob_sites().count();
    assert!(dev.iob_sites_all().len() > bottom, "the part has pads off the bottom row");
    let mut packed = pack(&design, &dev).expect("pack");
    let fit = packed.fit_pads(dev.iob_sites_all().len());
    assert!(fit.over_budget.is_empty() && fit.undriven.is_empty(), "88 pins fit: {fit:?}");
    let routed = route(&place(&packed, &dev).expect("place"), &dev).expect("route");
    let pad_row = dev.clb_y0 - 1;
    let outs_off = routed.placed.iob_sites.iter().filter(|s| s.y != pad_row).count();
    let ins_off = routed.tracks.iob_in.iter().filter(|i| i.y != pad_row).count();
    assert!(outs_off > 0 && ins_off > 0, "{outs_off} outputs and {ins_off} inputs off the bottom row");
    check(src, PART, 300);
}

/// A netlist built by hand, for designs the front end does not make yet
/// (block RAMs and multipliers): one clock `clk`, LUTs from truth
/// functions, flops, pads.
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
    /// Input bus `name`: nets `name_0` .. (pads).
    fn inputs(&mut self, name: &str, n: usize) -> Vec<String> {
        self.d.add_port(name, helion_ir::PortDir::In);
        (0..n).map(|i| format!("{name}_{i}")).collect()
    }
    /// A LUT computing `f` of its inputs (bit i of the argument = input i).
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
    /// A flop on `clk` with output net `q`.
    fn ff_to(&mut self, d: &str, q: &str) {
        let cell = self.fresh("ff");
        self.d.add_cell(&cell, CellKind::Hff);
        self.d.connect("clk", &cell, "CLK");
        self.d.connect(d, &cell, "D");
        self.d.connect(q, &cell, "Q");
    }
    fn ff(&mut self, d: &str) -> String {
        let q = self.fresh("q");
        self.ff_to(d, &q);
        q
    }
    /// An `n`-bit up counter.
    fn counter(&mut self, n: usize) -> Vec<String> {
        let q: Vec<String> = (0..n).map(|_| self.fresh("cnt")).collect();
        let mut carry: Option<String> = None;
        for i in 0..n {
            let next = match &carry {
                None => self.lut(&[&q[i]], |x| x & 1 == 0),
                Some(c) => self.lut(&[&q[i], c], |x| (x & 1) ^ (x >> 1) == 1),
            };
            let c2 = match &carry {
                None => q[i].clone(),
                Some(c) => self.lut(&[&q[i], c], |x| x == 3),
            };
            self.ff_to(&next, &q[i]);
            carry = Some(c2);
        }
        q
    }
    /// An output pad `name` reading `net`.
    fn output(&mut self, name: &str, net: &str) {
        self.d.add_port(name, helion_ir::PortDir::Out);
        let cell = format!("pad_{name}");
        self.d.add_cell(&cell, CellKind::IobOut);
        self.d.connect(net, &cell, "I");
        self.d.connect(name, &cell, "PAD");
    }
}

/// Deterministic INIT words for a RAM.
fn ram_init(seed: u64) -> String {
    let mut x = seed | 1;
    (0..256)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            format!("{x:x}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A RAM written at a counter's address with random data and a write
/// enable mixing an input with the counter; read at random addresses
/// near the writes (same-address collisions included) and in the
/// unwritten upper half (INIT contents), with a random read enable. DO0..7
/// go to pads directly, DO8..15 through flops, and a LUT mixes two of them.
fn ram_design() -> Design {
    let mut n = Nl::new();
    let d = n.inputs("d", 16);
    let rsel = n.inputs("rsel", 4);
    let ctl = n.inputs("ctl", 3);
    let cnt = n.counter(10);
    n.d.add_cell("ram", CellKind::Bram18);
    n.d.set_cell_attr("ram", "INIT", ram_init(0x9E37_79B9)).unwrap();
    n.d.connect("clk", "ram", "CLK");
    for i in 0..16 {
        n.d.connect(&d[i], "ram", &format!("DI{i}"));
    }
    for i in 0..10 {
        n.d.connect(&cnt[i], "ram", &format!("WADDR{i}"));
    }
    for i in 0..4 {
        n.d.connect(&rsel[i], "ram", &format!("RADDR{i}"));
    }
    for i in 4..9 {
        n.d.connect(&cnt[i], "ram", &format!("RADDR{i}"));
    }
    let hi = n.lut(&[&cnt[9], &ctl[2]], |x| (x & 1) ^ (x >> 1) == 1);
    n.d.connect(&hi, "ram", "RADDR9");
    let we = n.lut(&[&ctl[0], &cnt[1], &cnt[2]], |x| (x & 1) ^ ((x >> 1) & (x >> 2) & 1) == 1);
    n.d.connect(&we, "ram", "WE");
    n.d.connect(&ctl[1], "ram", "RE");
    for i in 0..16 {
        n.d.connect(&format!("do{i}"), "ram", &format!("DO{i}"));
    }
    for i in 0..8 {
        n.output(&format!("o{i}"), &format!("do{i}"));
    }
    for i in 8..16 {
        let q = n.ff(&format!("do{i}"));
        n.output(&format!("o{i}"), &q);
    }
    let mix = n.lut(&["do3", "do15", &d[0]], |x| x.count_ones() % 2 == 1);
    n.output("mix", &mix);
    n.d
}

/// A RAM read and written at the same address, every cycle: what is read
/// is the word before the write.
fn collision_design() -> Design {
    let mut n = Nl::new();
    let d = n.inputs("d", 16);
    let ctl = n.inputs("we", 1);
    let cnt = n.counter(4);
    n.d.add_cell("ram", CellKind::Bram18);
    n.d.connect("clk", "ram", "CLK");
    for i in 0..16 {
        n.d.connect(&d[i], "ram", &format!("DI{i}"));
        n.d.connect(&format!("do{i}"), "ram", &format!("DO{i}"));
        n.output(&format!("o{i}"), &format!("do{i}"));
    }
    for i in 0..4 {
        n.d.connect(&cnt[i], "ram", &format!("WADDR{i}"));
        n.d.connect(&cnt[i], "ram", &format!("RADDR{i}"));
    }
    n.d.connect(&ctl[0], "ram", "WE");
    n.d
}

/// A ROM: no write port, a counter on the low address bits and an input
/// on bit 4; every one of the first 32 INIT words is read again and again.
fn rom_design() -> Design {
    let mut n = Nl::new();
    let a = n.inputs("a", 1);
    let cnt = n.counter(4);
    n.d.add_cell("rom", CellKind::Bram18);
    n.d.set_cell_attr("rom", "INIT", ram_init(77)).unwrap();
    n.d.connect("clk", "rom", "CLK");
    for i in 0..4 {
        n.d.connect(&cnt[i], "rom", &format!("RADDR{i}"));
    }
    n.d.connect(&a[0], "rom", "RADDR4");
    for i in 0..16 {
        n.d.connect(&format!("do{i}"), "rom", &format!("DO{i}"));
        n.output(&format!("o{i}"), &format!("do{i}"));
    }
    n.d
}

#[test]
fn block_ram_equals_its_netlist() {
    for part in ["HL10S-C64-1", "HL10T-C32-1"] {
        let (nets, _) = check_design(&ram_design(), part, 700, |_| {}).unwrap_or_else(|e| panic!("{part}: {e}"));
        assert!(nets > 30, "{part}: {nets} nets");
        check_design(&collision_design(), part, 400, |_| {}).unwrap_or_else(|e| panic!("{part} collision: {e}"));
        check_design(&rom_design(), part, 300, |_| {}).unwrap_or_else(|e| panic!("{part} rom: {e}"));
    }
}

/// The netlist simulation is read-first, the same as the fabric: on the
/// collision design some cycle reads a word that the same edge overwrote
/// with something else.
#[test]
fn collision_design_exercises_read_first() {
    let design = collision_design();
    let mut sim = NetSim::new(&design);
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut differs = 0;
    for _ in 0..200 {
        for i in 0..16 {
            sim.val.insert(helion_ir::intern(&format!("d_{i}")), rng.bit());
        }
        sim.val.insert(helion_ir::intern("we_0"), rng.bit());
        sim.advance();
        let (we, di) = (sim.get(helion_ir::intern("we_0")), sim.word(&sim.rams[0].di));
        let clk = helion_ir::intern("clk");
        sim.val.insert(clk, true);
        sim.advance();
        sim.val.insert(clk, false);
        sim.advance();
        if we && sim.rams[0].q as u64 != di {
            differs += 1;
        }
    }
    assert!(differs > 20, "{differs}");
}

/// A multiplier with A and B (and C) from pads, P to pads directly and
/// through flops.
fn mul_design(signed: bool, preg: bool, with_c: bool) -> Design {
    let mut n = Nl::new();
    let a = n.inputs("a", 16);
    let b = n.inputs("b", 16);
    n.d.add_cell("mul", CellKind::Mac27);
    n.d.set_cell_attr("mul", "SIGNED", if signed { "1" } else { "0" }).unwrap();
    n.d.set_cell_attr("mul", "PREG", if preg { "1" } else { "0" }).unwrap();
    for i in 0..16 {
        n.d.connect(&a[i], "mul", &format!("A{i}"));
        n.d.connect(&b[i], "mul", &format!("B{i}"));
    }
    if with_c {
        let c = n.inputs("c", 32);
        for i in 0..32 {
            n.d.connect(&c[i], "mul", &format!("C{i}"));
        }
    }
    if preg {
        let ctl = n.inputs("ctl", 3);
        n.d.connect("clk", "mul", "CLK");
        // Enabled most of the time, reset now and then. The two are
        // independent, so RST also comes with CE high (RST wins) and
        // with CE low (still a reset, not a hold).
        let ce = n.lut(&[&ctl[0], &ctl[1]], |x| x != 0);
        let rst = n.lut(&[&ctl[2], &a[0]], |x| x == 0b11);
        n.d.connect(&ce, "mul", "CE");
        n.d.connect(&rst, "mul", "RST");
    }
    for i in 0..32 {
        let p = format!("p{i}");
        n.d.connect(&p, "mul", &format!("P{i}"));
        if i % 2 == 0 {
            n.output(&format!("o{i}"), &p);
        } else {
            let q = n.ff(&p);
            n.output(&format!("o{i}"), &q);
        }
    }
    n.d
}

/// A registered multiplier with CE and RST left unconnected: the
/// register loads on every rising edge (an unconnected CE reads 1, an
/// unconnected RST 0).
fn mul_free_running_design() -> Design {
    let mut n = Nl::new();
    let a = n.inputs("a", 16);
    n.d.add_cell("mul", CellKind::Mac27);
    n.d.set_cell_attr("mul", "PREG", "1").unwrap();
    n.d.connect("clk", "mul", "CLK");
    for i in 0..16 {
        n.d.connect(&a[i], "mul", &format!("A{i}"));
        n.d.connect(&a[15 - i], "mul", &format!("B{i}"));
    }
    for i in 0..32 {
        n.d.connect(&format!("p{i}"), "mul", &format!("P{i}"));
        n.output(&format!("o{i}"), &format!("p{i}"));
    }
    n.d
}

#[test]
fn multiplier_equals_its_netlist() {
    for part in ["HL10S-C64-1", "HL10T-DSP1"] {
        check_design(&mul_free_running_design(), part, 250, |_| {}).unwrap_or_else(|e| panic!("{part} free-running: {e}"));
    }
    for part in ["HL10S-C64-1", "HL10T-DSP1"] {
        for signed in [false, true] {
            for preg in [false, true] {
                for with_c in [false, true] {
                    check_design(&mul_design(signed, preg, with_c), part, 250, |_| {})
                        .unwrap_or_else(|e| panic!("{part} signed={signed} preg={preg} c={with_c}: {e}"));
                }
            }
        }
    }
}

/// The hard-block comparisons are not vacuous: a RAM input switched off
/// in PIN_EN, two address bits swapped, one INIT word changed or the
/// multiplier's sign mode flipped no longer match the netlist.
#[test]
fn broken_hard_blocks_are_detected() {
    let part = "HL10S-C64-1";
    let no_we = |r: &mut helion_route::Routed| r.placed.packed.conn.hard_pins.retain(|h| h.m != 42);
    assert!(check_design(&ram_design(), part, 700, no_we).is_err(), "WE off in PIN_EN went unnoticed");
    let swap_addr = |r: &mut helion_route::Routed| {
        let s = r.placed.bram_sites[0];
        for m in r.tracks.imux.iter_mut().filter(|m| (m.x, m.y) == (s.x, s.y)) {
            m.mux = match m.mux {
                32 => 33,
                33 => 32,
                k => k,
            };
        }
    };
    assert!(check_design(&ram_design(), part, 700, swap_addr).is_err(), "swapped RADDR0/RADDR1 went unnoticed");
    let init = |r: &mut helion_route::Routed| r.placed.packed.brams[0].init[5] ^= 1 << 20;
    assert!(check_design(&rom_design(), part, 300, init).is_err(), "a wrong INIT word went unnoticed");
    assert!(check_design(&rom_design(), part, 300, |_| {}).is_ok());
    let sign = |r: &mut helion_route::Routed| r.placed.packed.macs[0].signed ^= true;
    assert!(check_design(&mul_design(true, false, false), part, 250, sign).is_err(), "a flipped SIGNED went unnoticed");
}

/// From RTL to the fabric: with hard-block inference on, the v2 front end
/// puts a memory on a Bram18 and products on Mac27s (`tools/svcheck`, run
/// with HELION_V2_HARD=1 on `tools/svcheck/hard`, shows those netlists
/// equal their RTL), and their bitstreams equal those netlists.
#[test]
fn inferred_ram_and_multipliers_on_the_fabric() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/svcheck/hard/");
    helion_sv::v2::set_hard_blocks(Some(true));
    let synth = |f: &str| {
        let src = std::fs::read_to_string(format!("{root}{f}")).expect("source");
        helion_sv::synth_sv_v2(&src, f, None).expect("synth").design
    };
    let ram = synth("ram_1k.sv");
    let small = synth("ram_small.sv");
    let mul = synth("mul.sv");
    helion_sv::v2::set_hard_blocks(None);
    let count = |d: &Design, k: fn(&CellKind) -> bool| d.cells.iter().filter(|c| k(&c.kind)).count();
    assert_eq!(count(&ram, |k| matches!(k, CellKind::Bram18)), 1);
    assert_eq!(count(&ram, |k| matches!(k, CellKind::Hff)), 0, "the memory and its read register are the block RAM");
    assert_eq!(count(&small, |k| matches!(k, CellKind::Bram18)), 1);
    assert_eq!(count(&mul, |k| matches!(k, CellKind::Mac27)), 4);
    check_design(&ram, "HL10S-C64-1", 500, |_| {}).expect("ram_1k");
    check_design(&small, "HL10T-C32-1", 500, |_| {}).expect("ram_small");
    check_design(&mul, "HL10S-C64-1", 200, |_| {}).expect("mul");
    // Off (the default), the same sources are built from LUTs and flops.
    let plain = {
        let src = std::fs::read_to_string(format!("{root}ram_small.sv")).unwrap();
        helion_sv::synth_sv_v2(&src, "ram_small.sv", None).unwrap().design
    };
    assert_eq!(count(&plain, |k| matches!(k, CellKind::Bram18 | CellKind::Mac27)), 0);
}

/// A RAM clocked by a second clock pad (line 1) or by `clk` (line 0),
/// with a flop on `clk2` so that it is a clock of the design either way.
fn ram_two_clocks_design(ram_on_second: bool) -> Design {
    let mut n = Nl::new();
    n.d.add_port("clk2", helion_ir::PortDir::In);
    let d = n.inputs("d", 16);
    let ctl = n.inputs("ctl", 2);
    let cnt = n.counter(5);
    n.d.add_cell("ram", CellKind::Bram18);
    n.d.set_cell_attr("ram", "INIT", ram_init(0x5151)).unwrap();
    n.d.connect(if ram_on_second { "clk2" } else { "clk" }, "ram", "CLK");
    for i in 0..16 {
        n.d.connect(&d[i], "ram", &format!("DI{i}"));
    }
    for i in 0..5 {
        n.d.connect(&cnt[i], "ram", &format!("WADDR{i}"));
        n.d.connect(&cnt[i], "ram", &format!("RADDR{i}"));
    }
    n.d.connect(&ctl[0], "ram", "WE");
    n.d.connect(&ctl[1], "ram", "RADDR5");
    for i in 0..16 {
        n.d.connect(&format!("do{i}"), "ram", &format!("DO{i}"));
        n.output(&format!("o{i}"), &format!("do{i}"));
    }
    n.d.add_cell("ff_c2", CellKind::Hff);
    n.d.connect("clk2", "ff_c2", "CLK");
    n.d.connect(&d[0], "ff_c2", "D");
    n.d.connect("q_c2", "ff_c2", "Q");
    n.output("oc2", "q_c2");
    n.d
}

/// Hard blocks on a second clock: each clock pad toggles on its own, so a
/// block on the wrong line, or one that also captured on falling edges,
/// would read or load at the wrong time.
#[test]
fn hard_blocks_on_a_second_clock() {
    for part in ["HL10S-C64-1", "HL10T-C32-1"] {
        check_design(&ram_two_clocks_design(true), part, 600, |_| {}).unwrap_or_else(|e| panic!("{part} ram on clk2: {e}"));
        check_design(&ram_two_clocks_design(false), part, 600, |_| {}).unwrap_or_else(|e| panic!("{part} ram on clk: {e}"));
    }
    let mut n = Nl::new();
    n.d.add_port("clk2", helion_ir::PortDir::In);
    let a = n.inputs("a", 16);
    let b = n.inputs("b", 16);
    n.d.add_cell("mul", CellKind::Mac27);
    n.d.set_cell_attr("mul", "SIGNED", "1").unwrap();
    n.d.set_cell_attr("mul", "PREG", "1").unwrap();
    n.d.connect("clk2", "mul", "CLK");
    for i in 0..16 {
        n.d.connect(&a[i], "mul", &format!("A{i}"));
        n.d.connect(&b[i], "mul", &format!("B{i}"));
    }
    for i in 0..32 {
        n.d.connect(&format!("p{i}"), "mul", &format!("P{i}"));
        let q = n.ff(&format!("p{i}"));
        n.output(&format!("o{i}"), &q);
    }
    for part in ["HL10S-C64-1", "HL10T-DSP1"] {
        check_design(&n.d, part, 400, |_| {}).unwrap_or_else(|e| panic!("{part} mac on clk2: {e}"));
    }
}

/// A RAM clocked by a divided clock that a flop makes.
#[test]
fn block_ram_on_a_divided_clock() {
    let mut n = Nl::new();
    let d = n.inputs("d", 4);
    let t = n.fresh("tog");
    let nt = n.lut(&[&t], |x| x & 1 == 0);
    n.ff_to(&nt, &t);
    n.d.add_cell("ram", CellKind::Bram18);
    n.d.connect(&t, "ram", "CLK");
    for i in 0..4 {
        n.d.connect(&d[i], "ram", &format!("DI{i}"));
        n.d.connect(&d[i], "ram", &format!("RADDR{i}"));
        n.d.connect(&d[(i + 1) % 4], "ram", &format!("WADDR{i}"));
    }
    n.d.connect(&d[0], "ram", "WE");
    for i in 0..4 {
        n.d.connect(&format!("do{i}"), "ram", &format!("DO{i}"));
        n.output(&format!("o{i}"), &format!("do{i}"));
    }
    for part in ["HL10S-C64-1", "HL10T-C32-1"] {
        check_design(&n.d, part, 400, |_| {}).unwrap_or_else(|e| panic!("{part}: {e}"));
    }
}

/// The base tile's clock select is what the fabric reads: a RAM on
/// `clk2` with its block clock moved to the other line no longer matches.
#[test]
fn hard_block_on_the_wrong_clock_line_is_detected() {
    let part = "HL10S-C64-1";
    let d = ram_two_clocks_design(true);
    let dev = Device::load_part(part).unwrap();
    let p = pack(&d, &dev).unwrap();
    assert_eq!(p.conn.hard_clock.iter().flatten().count(), 1, "{:?}", p.conn.hard_clock);
    let wrong = |r: &mut helion_route::Routed| {
        for c in r.placed.packed.conn.hard_clock.iter_mut().flatten() {
            *c ^= 1;
        }
    };
    assert!(check_design(&d, part, 600, wrong).is_err(), "a hard block on the wrong clock line went unnoticed");
}
