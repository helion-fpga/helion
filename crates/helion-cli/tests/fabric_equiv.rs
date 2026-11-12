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
/// clock nets and edges, output pads.
struct NetSim {
    luts: Vec<(u64, [Option<Spur>; 6], Spur)>,
    /// (D, Q, clock net, falling edge, asynchronous reset: net and level)
    ffs: Vec<(Spur, Spur, Option<Spur>, bool, Option<(Spur, bool)>)>,
    val: HashMap<Spur, bool>,
    /// Clock net values when last looked at.
    prev: HashMap<Spur, bool>,
}

impl NetSim {
    fn new(d: &Design) -> Self {
        let pins = d.pin_index();
        let mut luts = Vec::new();
        let mut ffs = Vec::new();
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
                _ => {}
            }
        }
        Self { luts, ffs, val: HashMap::new(), prev: HashMap::new() }
    }
    fn get(&self, n: Spur) -> bool {
        self.val.get(&n).copied().unwrap_or(false)
    }
    fn settle(&mut self) {
        for _ in 0..10_000 {
            let mut changed = false;
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
            let clocks: Vec<Spur> = self.ffs.iter().filter_map(|f| f.2).collect();
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
