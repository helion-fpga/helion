//! The compiled simulator runs Helion's per-bit `Mac27` and `Bram18` cells
//! (what v2 emits with hard-block inference on) as helion-ir defines them,
//! checked against a direct model of that definition under random inputs.

use helion_ir::{CellKind, Design, PortDir};
use helion_sim::fast::CompiledSim;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Input ports `name0..name{n-1}`, each straight on pin `pin{i}` of `cell`.
fn bus_in(d: &mut Design, name: &str, cell: &str, pin: &str, n: usize) {
    for i in 0..n {
        let p = format!("{name}{i}");
        d.add_port(&p, PortDir::In);
        d.connect(&p, cell, &format!("{pin}{i}"));
    }
}

fn drive(sim: &mut CompiledSim, name: &str, n: usize, v: u64) {
    for i in 0..n {
        sim.drive_input(&format!("{name}{i}"), (v >> i) & 1 == 1).unwrap();
    }
}

fn read(sim: &CompiledSim, name: &str, n: usize) -> u64 {
    (0..n).map(|i| sim.sample(&format!("{name}{i}")).expect("known") << i).sum()
}

fn mul_design(signed: bool, preg: bool, ce_rst: bool) -> Design {
    let mut d = Design::new("m");
    d.add_cell("mul", CellKind::Mac27);
    d.set_cell_attr("mul", "SIGNED", if signed { "1" } else { "0" }).unwrap();
    d.set_cell_attr("mul", "PREG", if preg { "1" } else { "0" }).unwrap();
    bus_in(&mut d, "a", "mul", "A", 16);
    bus_in(&mut d, "b", "mul", "B", 16);
    bus_in(&mut d, "c", "mul", "C", 32);
    if preg {
        d.add_port("clk", PortDir::In);
        d.connect("clk", "mul", "CLK");
        if ce_rst {
            for p in ["CE", "RST"] {
                d.add_port(&p.to_lowercase(), PortDir::In);
                d.connect(&p.to_lowercase(), "mul", p);
            }
        }
    }
    for i in 0..32 {
        d.connect(&format!("p{i}"), "mul", &format!("P{i}"));
    }
    d
}

#[test]
fn per_bit_multiplier() {
    for signed in [false, true] {
        for (preg, ce_rst) in [(false, false), (true, true), (true, false)] {
            let d = mul_design(signed, preg, ce_rst);
            let mut sim = CompiledSim::new(&d);
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let mut p_reg = 0u32;
            for cycle in 0..400 {
                let (a, b, c) = (rng.next() & 0xffff, rng.next() & 0xffff, rng.next() & 0xffff_ffff);
                let (ce, rst) = (rng.next() % 4 != 0, rng.next() % 5 == 0);
                drive(&mut sim, "a", 16, a);
                drive(&mut sim, "b", 16, b);
                drive(&mut sim, "c", 32, c);
                if ce_rst {
                    sim.drive_input("ce", ce).unwrap();
                    sim.drive_input("rst", rst).unwrap();
                }
                let prod = if signed {
                    (a as u16 as i16 as i32).wrapping_mul(b as u16 as i16 as i32) as u32
                } else {
                    (a as u32).wrapping_mul(b as u32)
                };
                let comb = prod.wrapping_add(c as u32);
                sim.eval_comb_2state();
                if !preg {
                    assert_eq!(read(&sim, "p", 32) as u32, comb, "signed={signed} cycle {cycle}");
                    continue;
                }
                assert_eq!(read(&sim, "p", 32) as u32, p_reg, "signed={signed} ce_rst={ce_rst} cycle {cycle}");
                sim.step_2state(10);
                // RST wins over CE; unconnected, CE reads 1 and RST 0.
                p_reg = if ce_rst && rst {
                    0
                } else if !ce_rst || ce {
                    comb
                } else {
                    p_reg
                };
                assert_eq!(read(&sim, "p", 32) as u32, p_reg, "signed={signed} ce_rst={ce_rst} after edge {cycle}");
            }
        }
    }
}

#[test]
fn per_bit_block_ram() {
    for with_re in [true, false] {
        let mut d = Design::new("r");
        d.add_cell("ram", CellKind::Bram18);
        let init: Vec<String> = (0..256u64).map(|w| format!("{:#x}", w.wrapping_mul(0x9E37_79B9_7F4A_7C15))).collect();
        d.set_cell_attr("ram", "INIT", init.join(",")).unwrap();
        d.add_port("clk", PortDir::In);
        d.connect("clk", "ram", "CLK");
        bus_in(&mut d, "di", "ram", "DI", 16);
        // Five address bits each (the rest read 0): collisions are common.
        bus_in(&mut d, "wa", "ram", "WADDR", 5);
        bus_in(&mut d, "ra", "ram", "RADDR", 5);
        d.add_port("we", PortDir::In);
        d.connect("we", "ram", "WE");
        if with_re {
            d.add_port("re", PortDir::In);
            d.connect("re", "ram", "RE");
        }
        for i in 0..16 {
            d.connect(&format!("do{i}"), "ram", &format!("DO{i}"));
        }
        let mut mem: Vec<u16> = (0..1024).map(|a: usize| ((a as u64 / 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (16 * (a % 4))) as u16).collect();
        let mut sim = CompiledSim::new(&d);
        assert_eq!(sim.ram_word("ram", 5), Some(mem[5]));
        let mut out = 0u16;
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let mut collisions = 0;
        for cycle in 0..1500 {
            let (di, wa, ra) = (rng.next() & 0xffff, rng.next() & 31, rng.next() & 31);
            let (we, re) = (rng.next() % 2 == 0, !with_re || rng.next() % 4 != 0);
            drive(&mut sim, "di", 16, di);
            drive(&mut sim, "wa", 5, wa);
            drive(&mut sim, "ra", 5, ra);
            sim.drive_input("we", we).unwrap();
            if with_re {
                sim.drive_input("re", re).unwrap();
            }
            sim.step_2state(10);
            // Read first: the word before this edge's write.
            if re {
                out = mem[ra as usize];
            }
            if we {
                collisions += (wa == ra && re && mem[wa as usize] != di as u16) as usize;
                mem[wa as usize] = di as u16;
            }
            assert_eq!(read(&sim, "do", 16) as u16, out, "with_re={with_re} cycle {cycle}");
        }
        assert!(collisions > 8, "{collisions}");
    }
}

/// A name-only `Bram18` (the host-access aperture) and a pinless `Mac27`
/// compile to nothing.
#[test]
fn pinless_hard_cells_are_ignored() {
    let mut d = Design::structural_counter();
    d.add_cell("aperture", CellKind::Bram18);
    d.add_cell("site", CellKind::Mac27);
    let sim = CompiledSim::new(&d);
    assert!(sim.schedule.rams.is_empty());
    assert_eq!(sim.schedule.ffs.len(), CompiledSim::new(&Design::structural_counter()).schedule.ffs.len());
}
