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

/// Reference simulation of the mapped netlist: LUTs, flops, output pads.
struct NetSim {
    luts: Vec<(u64, [Option<Spur>; 6], Spur)>,
    ffs: Vec<(Spur, Spur)>,
    val: HashMap<Spur, bool>,
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
                        ffs.push((dn, q));
                    }
                }
                _ => {}
            }
        }
        Self { luts, ffs, val: HashMap::new() }
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
    fn clock(&mut self) {
        let next: Vec<(Spur, bool)> = self.ffs.iter().map(|(d, q)| (*q, self.get(*d))).collect();
        for (q, v) in next {
            self.val.insert(q, v);
        }
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
    let fit = packed.fit_pads(dev.iob_sites().count());
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
    let ins: Vec<(Spur, (u32, u32))> = routed.tracks.iob_in.iter().map(|i| (helion_ir::intern(&i.net), (i.x, i.y))).collect();
    assert_eq!(ins.len(), packed.conn.inputs.len(), "an input pad was dropped");

    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for cycle in 0..cycles {
        for (net, (x, y)) in &ins {
            let v = rng.bit();
            sim.val.insert(*net, v);
            fab.set_pad_in(*x, *y, v);
        }
        sim.settle();
        fab.settle();
        for (cell, net, (x, y)) in &outs {
            if fab.led_at(*x, *y) != sim.get(*net) {
                return Err(format!("cycle {cycle}: pad {cell} (net {}) differs", resolve(*net)));
            }
        }
        sim.clock();
        fab.step_user();
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
