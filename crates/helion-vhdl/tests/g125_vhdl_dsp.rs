//! G125: VHDL multiplies reuse the G3 DSP path. Landed on fm/qor-size-4.5
//! through the coverage lane (the G125 worker was stopped before pushing;
//! its probe findings and this test file were ported here verbatim).
//!
//! A plain VHDL multiply already reached the DSP path through the
//! VHDL-to-SV translation, but a combinational multiply-accumulate
//! (`p <= a * b + c`) or sum of two products (`p <= a * b + c * d`)
//! never got there: `flatten_leftover_signal` dropped the whole
//! module before the DSP pass ran. The coverage lane exempts exactly
//! those shapes from the flatten cap, lowers small sums of products to
//! LUTs in step 1 of the DSP pass, and lets the G3 cascade (step 4) or
//! `emit_mac27` take the rest. Counts prove the routing; the sim
//! tests prove the LUT lowering computes the exact sum.

use helion_ir::{CellKind, Design};
use helion_vhdl::map_vhdl;

fn counts(d: &Design) -> (usize, usize, usize, usize) {
    let luts = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let carry4s = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Carry4))
        .count();
    let hffs = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let dsps = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
        .count();
    (luts, carry4s, hffs, dsps)
}

const HDR: &str = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;
"#;

fn vhdl_mul(wa: usize, wb: usize, wo: usize) -> String {
    format!(
        r#"{HDR}
entity mul is
  port (
    a : in  unsigned({ahi} downto 0);
    b : in  unsigned({bhi} downto 0);
    p : out unsigned({ohi} downto 0)
  );
end entity mul;
architecture rtl of mul is
begin
  p <= a * b;
end architecture rtl;
"#,
        ahi = wa - 1,
        bhi = wb - 1,
        ohi = wo - 1
    )
}

fn vhdl_mac(wa: usize, wb: usize, wc: usize, wo: usize) -> String {
    format!(
        r#"{HDR}
entity mul is
  port (
    a : in  unsigned({ahi} downto 0);
    b : in  unsigned({bhi} downto 0);
    c : in  unsigned({chi} downto 0);
    p : out unsigned({ohi} downto 0)
  );
end entity mul;
architecture rtl of mul is
begin
  p <= a * b + c;
end architecture rtl;
"#,
        ahi = wa - 1,
        bhi = wb - 1,
        chi = wc - 1,
        ohi = wo - 1
    )
}

fn vhdl_sumprod(wa: usize, wb: usize, wc: usize, wd: usize, wo: usize) -> String {
    format!(
        r#"{HDR}
entity mul is
  port (
    a : in  unsigned({ahi} downto 0);
    b : in  unsigned({bhi} downto 0);
    c : in  unsigned({chi} downto 0);
    d : in  unsigned({dhi} downto 0);
    p : out unsigned({ohi} downto 0)
  );
end entity mul;
architecture rtl of mul is
begin
  p <= a * b + c * d;
end architecture rtl;
"#,
        ahi = wa - 1,
        bhi = wb - 1,
        chi = wc - 1,
        dhi = wd - 1,
        ohi = wo - 1
    )
}

fn vhdl_clk_sumprod(w: usize) -> String {
    format!(
        r#"{HDR}
entity mul is
  port (
    clk : in std_logic;
    a : in unsigned({hi} downto 0);
    b : in unsigned({hi} downto 0);
    c : in unsigned({hi} downto 0);
    d : in unsigned({hi} downto 0);
    p : out unsigned({ohi} downto 0)
  );
end entity mul;
architecture rtl of mul is
begin
  process(clk)
  begin
    if rising_edge(clk) then
      p <= a * b + c * d;
    end if;
  end process;
end architecture rtl;
"#,
        hi = w - 1,
        ohi = 2 * w - 1
    )
}

#[test]
fn g125_vhdl_comb_mul_16x16_maps_to_dsp() {
    let mr = map_vhdl(&vhdl_mul(16, 16, 32)).expect("map vhdl");
    let (luts, _, _, dsps) = counts(&mr.design);
    assert_eq!(dsps, 1, "16x16 VHDL multiply must use one DSP");
    assert_eq!(luts, 0, "16x16 VHDL multiply must not use LUTs");
}

#[test]
fn g125_vhdl_comb_mul_8x8_maps_to_luts() {
    let mr = map_vhdl(&vhdl_mul(8, 8, 16)).expect("map vhdl");
    let (luts, carry4s, hffs, dsps) = counts(&mr.design);
    assert_eq!(dsps, 0, "8x8 VHDL multiply must not use a DSP");
    assert_eq!(hffs, 0, "combinational 8x8 has no flops");
    assert_eq!(luts, 64, "8x8 Carry4 array multiplier is 64 LUTs, got {luts}");
    assert_eq!(carry4s, 14, "8x8 Carry4 array multiplier is 14 Carry4, got {carry4s}");
}

#[test]
fn g125_vhdl_comb_mac_16x16_maps_to_dsp() {
    let mr = map_vhdl(&vhdl_mac(16, 16, 32, 32)).expect("map vhdl");
    let (luts, _, _, dsps) = counts(&mr.design);
    assert_eq!(dsps, 1, "16x16 VHDL MAC must use one DSP, got {dsps}");
    assert_eq!(luts, 0, "16x16 VHDL MAC must not use LUTs");
}

#[test]
fn g125_vhdl_comb_mac_8x8_maps_to_luts() {
    let mr = map_vhdl(&vhdl_mac(8, 8, 16, 16)).expect("map vhdl");
    let (luts, carry4s, hffs, dsps) = counts(&mr.design);
    assert_eq!(dsps, 0, "8x8 VHDL MAC must not use a DSP");
    assert_eq!(hffs, 0, "combinational 8x8 MAC has no flops");
    // 8x8 product (64 LUTs, 14 Carry4) plus a 16-bit add (16 LUTs, 4 Carry4).
    assert_eq!(luts, 80, "8x8 VHDL MAC is 80 LUTs, got {luts}");
    assert_eq!(carry4s, 18, "8x8 VHDL MAC is 18 Carry4, got {carry4s}");
}

#[test]
fn g125_vhdl_comb_sumprod_16x16_maps_to_two_dsps() {
    let mr = map_vhdl(&vhdl_sumprod(16, 16, 16, 16, 32)).expect("map vhdl");
    let (luts, _, _, dsps) = counts(&mr.design);
    assert_eq!(dsps, 2, "16x16 VHDL sum of products must use two DSPs, got {dsps}");
    assert_eq!(luts, 0, "16x16 VHDL sum of products must not use LUTs");
}

#[test]
fn g125_vhdl_comb_sumprod_8x8_maps_to_luts() {
    let mr = map_vhdl(&vhdl_sumprod(8, 8, 8, 8, 16)).expect("map vhdl");
    let (luts, carry4s, hffs, dsps) = counts(&mr.design);
    assert_eq!(dsps, 0, "8x8 VHDL sum of products must not use a DSP");
    assert_eq!(hffs, 0, "combinational 8x8 sum of products has no flops");
    // Two 8x8 products (2x64 LUTs, 2x14 Carry4) plus a 16-bit add
    // (16 LUTs, 4 Carry4).
    assert_eq!(luts, 144, "8x8 VHDL sum of products is 144 LUTs, got {luts}");
    assert_eq!(carry4s, 32, "8x8 VHDL sum of products is 32 Carry4, got {carry4s}");
}

#[test]
fn g125_vhdl_clk_sumprod_8x8_maps_to_luts() {
    let mr = map_vhdl(&vhdl_clk_sumprod(8)).expect("map vhdl");
    let (luts, carry4s, hffs, dsps) = counts(&mr.design);
    assert_eq!(dsps, 0, "clocked 8x8 VHDL sum of products must not use a DSP");
    assert_eq!(luts, 144, "clocked 8x8 VHDL sum of products is 144 LUTs, got {luts}");
    assert_eq!(carry4s, 32, "clocked 8x8 VHDL sum of products is 32 Carry4, got {carry4s}");
    assert_eq!(hffs, 16, "clocked 8x8 sum registers all 16 sum bits, got {hffs}");
}

#[test]
fn g125_vhdl_clk_sumprod_16x16_stays_two_dsps() {
    let mr = map_vhdl(&vhdl_clk_sumprod(16)).expect("map vhdl");
    let (luts, _, _, dsps) = counts(&mr.design);
    assert_eq!(dsps, 2, "clocked 16x16 VHDL sum of products must use two DSPs, got {dsps}");
    assert_eq!(luts, 0, "clocked 16x16 VHDL sum of products must not use LUTs");
}

// G125 value guard: simulate the synthesized LUT netlist with
// helion-sim (which models Carry4 CO pins) over deterministic vectors
// and compare against the expected unsigned sum. This is the value
// guard the external svcheck script cannot currently provide for
// Carry4 designs: svcheck models a Carry4 carry-out only on a pin
// literally named `CO`, while Helion emits per-bit `CO0`-`CO3` pins.
fn g125_bit_blast_ports(d: &mut Design) {
    use helion_ir::{Port, intern, resolve};
    let nets: std::collections::HashSet<String> =
        d.nets.iter().map(|n| resolve(n.name).to_string()).collect();
    let mut out = Vec::new();
    for p in &d.ports {
        let name = resolve(p.name).to_string();
        if nets.contains(&name) || !nets.contains(&format!("{name}_0")) {
            out.push(p.clone());
        }
        let mut i = 0;
        while nets.contains(&format!("{name}_{i}")) {
            out.push(Port {
                name: intern(&format!("{name}_{i}")),
                dir: p.dir,
                attrs: p.attrs.clone(),
            });
            i += 1;
        }
    }
    d.ports = out;
}

fn g125_drive(sim: &mut helion_sim::Sim, name: &str, width: usize, v: u64) {
    for i in 0..width {
        let b = if width == 1 {
            name.to_string()
        } else {
            format!("{name}_{i}")
        };
        let r = sim.drive_input(&b, (v >> i) & 1 == 1);
        if r.is_err() && i == 0 {
            let _ = sim.drive_input(name, v & 1 == 1);
        }
    }
}

fn g125_sample(sim: &helion_sim::Sim, name: &str, width: usize) -> Option<u64> {
    let mut v: u64 = 0;
    for i in 0..width {
        let b = if width == 1 {
            name.to_string()
        } else {
            format!("{name}_{i}")
        };
        let bit = sim
            .sample(&b)
            .or_else(|| if i == 0 { sim.sample(name) } else { None })?;
        v |= (bit & 1) << i;
    }
    Some(v)
}

fn g125_vectors() -> Vec<(u64, u64, u64, u64)> {
    // Corners plus a deterministic pseudo-random sweep (xorshift, fixed seed).
    let mut v = vec![
        (0, 0, 0, 0),
        (0, 255, 0, 0),
        (255, 0, 0, 0),
        (1, 1, 1, 1),
        (255, 255, 255, 255),
        (255, 1, 0, 0),
        (1, 255, 0, 0),
        (128, 128, 128, 128),
        (255, 255, 0, 65535),
        (0, 0, 255, 255),
        (200, 100, 50, 25),
    ];
    let mut s: u64 = 0x1234_5678_9abc_def0;
    for _ in 0..64 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.push((
            (s & 0xff) as u64,
            ((s >> 8) & 0xff) as u64,
            ((s >> 16) & 0xff) as u64,
            ((s >> 24) & 0xff) as u64,
        ));
    }
    v
}

#[test]
fn g125_comb_sumprod_8x8_sim_values() {
    let mr = map_vhdl(&vhdl_sumprod(8, 8, 8, 8, 16)).expect("map vhdl");
    let mut d = mr.design;
    g125_bit_blast_ports(&mut d);
    let mut sim = helion_sim::Sim::new(&d);
    for (a, b, c, e) in g125_vectors() {
        g125_drive(&mut sim, "a", 8, a);
        g125_drive(&mut sim, "b", 8, b);
        g125_drive(&mut sim, "c", 8, c);
        g125_drive(&mut sim, "d", 8, e);
        let got = g125_sample(&sim, "p", 16).expect("p must be driven");
        let want = (a.wrapping_mul(b).wrapping_add(c.wrapping_mul(e))) & 0xffff;
        assert_eq!(got, want, "8x8 sumprod: {a}*{b} + {c}*{e} = {want}, sim gave {got}");
    }
}

#[test]
fn g125_comb_mac_8x8_sim_values() {
    let mr = map_vhdl(&vhdl_mac(8, 8, 16, 16)).expect("map vhdl");
    let mut d = mr.design;
    g125_bit_blast_ports(&mut d);
    let mut sim = helion_sim::Sim::new(&d);
    for (a, b, c, _) in g125_vectors() {
        g125_drive(&mut sim, "a", 8, a);
        g125_drive(&mut sim, "b", 8, b);
        g125_drive(&mut sim, "c", 16, c);
        let got = g125_sample(&sim, "p", 16).expect("p must be driven");
        let want = (a.wrapping_mul(b).wrapping_add(c)) & 0xffff;
        assert_eq!(got, want, "8x8 MAC: {a}*{b} + {c} = {want}, sim gave {got}");
    }
}

#[test]
fn g125_clk_sumprod_8x8_sim_values() {
    let mr = map_vhdl(&vhdl_clk_sumprod(8)).expect("map vhdl");
    let mut d = mr.design;
    g125_bit_blast_ports(&mut d);
    let mut sim = helion_sim::Sim::new(&d);
    for (a, b, c, e) in g125_vectors() {
        g125_drive(&mut sim, "a", 8, a);
        g125_drive(&mut sim, "b", 8, b);
        g125_drive(&mut sim, "c", 8, c);
        g125_drive(&mut sim, "d", 8, e);
        sim.step_posedge(10);
        let got = g125_sample(&sim, "p", 16).expect("p must be driven");
        let want = (a.wrapping_mul(b).wrapping_add(c.wrapping_mul(e))) & 0xffff;
        assert_eq!(got, want, "clocked 8x8 sumprod: {a}*{b} + {c}*{e} = {want}, sim gave {got}");
    }
}
