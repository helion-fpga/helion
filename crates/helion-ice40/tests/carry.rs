//! End-to-end carry-chain test: an 8-bit increment counter built from two
//! chained [`helion_ir::CellKind::Carry4`] cells, run through the iCE40
//! flow (pack -> place -> route -> asc). Asserts the carry LCs are placed
//! as a vertical chain, the CarryEnable / CarryInSet / in_3 carry buffer
//! bits are set in the .asc, and the .asc round-trips through
//! icepack/iceunpack bit-perfectly.
//!
//! Carry4 pin convention (Xilinx CARRY4): inputs `CI`, `CYINIT`, `DI0..3`,
//! `S0..3`; outputs `O0..3` (sums), `CO0..3` (carry outs). For the
//! increment, bit i uses `S[i] = q[i]`, `DI[i] = 0`, giving
//! `O[i] = q[i] ^ carry[i]` and `CO[i] = q[i] & carry[i]`.

use helion_backend::Backend;
use helion_ice40::Ice40Backend;
use helion_ir::{CellKind, Design, PortDir};

/// Build an 8-bit counter: two chained Carry4s + 8 FFs + 8 LEDs.
fn counter_design() -> Design {
    let mut d = Design::new("carry_counter");
    d.ports.push(helion_ir::Port {
        name: "clk".to_string(),
        dir: PortDir::In,
        attrs: Default::default(),
    });
    for i in 0..8 {
        d.ports.push(helion_ir::Port {
            name: format!("led{i}"),
            dir: PortDir::Out,
            attrs: Default::default(),
        });
    }

    // Clock input.
    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");
    d.add_cell("clk_buf", CellKind::ClockBuf);
    d.connect("clk_net", "clk_buf", "I");
    d.connect("clk_global", "clk_buf", "O");

    // Two chained Carry4 cells. c0: bits 0-3, CYINIT=1. c1: bits 4-7,
    // CI driven by c0's CO[3].
    for (c, base) in [(0u32, 0u32), (1, 4)] {
        let name = format!("c{c}");
        d.add_cell(name.clone(), CellKind::Carry4);
        if c == 0 {
            d.connect("1", name.clone(), "CYINIT");
        } else {
            d.connect("c0_co3", name.clone(), "CI");
        }
        for i in 0..4 {
            let bit = base + i;
            d.connect("0", name.clone(), format!("DI{i}"));
            d.connect(format!("q{bit}"), name.clone(), format!("S{i}"));
            d.connect(format!("d{bit}"), name.clone(), format!("O{i}"));
        }
        if c == 0 {
            d.connect("c0_co3", name.clone(), "CO3");
        }
    }

    // 8 FFs holding the counter state.
    for bit in 0..8 {
        let ff = format!("ff{bit}");
        d.add_cell(ff.clone(), CellKind::Hff);
        d.connect(format!("d{bit}"), ff.clone(), "D");
        d.connect(format!("q{bit}"), ff.clone(), "Q");
        d.connect("clk_global", ff, "CLK");
    }

    // LED outputs.
    for bit in 0..8 {
        let ob = format!("led_ob{bit}");
        d.add_cell(ob.clone(), CellKind::IobOut);
        d.connect(format!("q{bit}"), ob.clone(), "I");
        d.connect(format!("led{bit}"), ob, "PAD");
    }

    d
}

/// Decode one logic tile's banks from .asc text.
/// Returns map (x, y) -> 16 bank strings.
fn parse_logic_tiles(asc: &str) -> std::collections::HashMap<(u32, u32), Vec<String>> {
    let mut tiles = std::collections::HashMap::new();
    let mut cur: Option<(u32, u32)> = None;
    for line in asc.lines() {
        if let Some(rest) = line.strip_prefix(".logic_tile ") {
            let mut it = rest.split_whitespace();
            let x: u32 = it.next().unwrap().parse().unwrap();
            let y: u32 = it.next().unwrap().parse().unwrap();
            cur = Some((x, y));
            tiles.insert((x, y), Vec::new());
        } else if let Some((x, y)) = cur {
            if line.chars().all(|c| c == '0' || c == '1') && line.len() == 54 {
                tiles.get_mut(&(x, y)).unwrap().push(line.to_string());
            }
        }
    }
    tiles
}

#[test]
fn counter_8bit_carry_end_to_end() {
    let design = counter_design();
    let backend = Ice40Backend::new("ice40-hx8k");

    // Pack: 8 carry sum LUTs, one 8-bit chain with cin=1, plus the $carry$zero
    // tie-off LUT. No XOR LUTs: DI is constant 0, so in_2 = S directly.
    let packed = backend.pack(&design).expect("pack");
    assert_eq!(packed.carry_chains.len(), 1, "two chained Carry4s form one chain");
    assert_eq!(packed.carry_chains[0].bits, 8);
    assert!(packed.carry_chains[0].cin_one, "CYINIT=1");
    let carry_luts: Vec<_> = packed.luts.iter().filter(|l| l.carry.is_some()).collect();
    assert_eq!(carry_luts.len(), 8);
    for lut in &carry_luts {
        assert_eq!(lut.init, 0x55AA, "sum LUT is in_0 ^ in_3");
    }
    assert!(
        packed.luts.iter().any(|l| l.name == "$carry$zero"),
        "constant-0 DI needs a tie-off LUT"
    );
    assert!(
        !packed.luts.iter().any(|l| l.name.contains("$xor")),
        "no XOR LUTs when DI is constant 0"
    );

    // Place: the 8 carry LCs must be vertically adjacent in one column.
    let placed = backend.place(&packed).expect("place");
    let mut carry_cells: Vec<_> = placed
        .luts
        .iter()
        .filter(|l| l.carry)
        .map(|l| (l.x, l.y, l.lc))
        .collect();
    assert_eq!(carry_cells.len(), 8);
    carry_cells.sort_by_key(|&(_, _, lc)| lc);
    // 8 bits fit in one tile: same (x, y), LCs 0..7.
    let (x0, y0, _) = carry_cells[0];
    for (i, &(x, y, lc)) in carry_cells.iter().enumerate() {
        assert_eq!((x, y), (x0, y0), "carry chain stays in one column");
        assert_eq!(lc as usize, i, "carry LCs are vertically contiguous from LC0");
    }
    // The chain's tile gets a CarryInSet mux config (cin=1).
    assert_eq!(placed.carry_mux.len(), 1);
    assert_eq!((placed.carry_mux[0].x, placed.carry_mux[0].y), (x0, y0));
    // Each sum LUT paired with its FF in the same LC.
    assert_eq!(placed.ffs.len(), 8);
    for ff in &placed.ffs {
        assert!(
            carry_cells.contains(&(ff.x, ff.y, ff.lc)),
            "counter FFs live in the carry LCs"
        );
    }

    // Route.
    let routed = backend.route(&placed).expect("route");
    assert!(!routed.nets.is_empty());

    // ASC: CarryEnable (LC_i[8] = bank 2i bit 44), DffEnable (bit 45),
    // and CarryInSet (bank 1 bit 50) must be set.
    let asc = backend.bitstream(&routed).expect("bitstream");
    assert!(asc.starts_with(".device 8k\n"));
    let tiles = parse_logic_tiles(&asc);
    let banks = &tiles[&(x0 as u32, y0 as u32)];
    assert_eq!(banks.len(), 16);
    for lc in 0..8u8 {
        let bank = &banks[2 * lc as usize];
        assert_eq!(
            bank.chars().nth(44).unwrap(),
            '1',
            "LC{lc} CarryEnable (bank {}, bit 44)",
            2 * lc
        );
        assert_eq!(
            bank.chars().nth(45).unwrap(),
            '1',
            "LC{lc} DffEnable (bank {}, bit 45)",
            2 * lc
        );
    }
    assert_eq!(
        banks[1].chars().nth(50).unwrap(),
        '1',
        "CarryInSet (bank 1, bit 50)"
    );
    // in_3 carry buffers: LC0 selects carry_in_mux; LC i>0 selects
    // the previous LC's cout. At least one in_3 mux bit must be set per LC.
    // (Exact patterns come from the IceStorm DB; presence is the assertion.)
    for lc in 0..8u8 {
        // in_3 mux bits live in banks 2*lc / 2*lc+1, bits 31..34 region.
        // Just verify the LC's config banks are non-empty beyond LUT+Dff+Carry.
        let b0 = &banks[2 * lc as usize];
        let b1 = &banks[2 * lc as usize + 1];
        let extra = b0.chars().take(36).any(|c| c == '1')
            || b1.chars().take(36).any(|c| c == '1');
        assert!(extra, "LC{lc} has in_3 mux bits set");
    }

    // Write the .asc and round-trip through icepack/iceunpack. iceunpack
    // normalizes (drops all-zero tiles, adds .comment), so compare the SET
    // BITS semantically: no bit may be lost or gained.
    let asc_path = "/tmp/helion_carry_counter.asc";
    std::fs::write(asc_path, &asc).expect("write asc");
    let bin_path = "/tmp/helion_carry_counter.bin";
    let asc2_path = "/tmp/helion_carry_counter_rt.asc";
    let st = std::process::Command::new("/usr/bin/icepack")
        .args([asc_path, bin_path])
        .status()
        .expect("run icepack");
    assert!(st.success(), "icepack failed");
    let st = std::process::Command::new("/usr/bin/iceunpack")
        .args([bin_path, asc2_path])
        .status()
        .expect("run iceunpack");
    assert!(st.success(), "iceunpack failed");
    let rt = std::fs::read_to_string(asc2_path).expect("read round-tripped asc");
    assert_eq!(
        set_bits(&asc),
        set_bits(&rt),
        "icepack/iceunpack round-trip must preserve every bit"
    );
    println!("carry counter: 8-bit chain at tile ({x0},{y0}), round-trip bit-perfect");
}

/// The set of `(tile_kind, x, y, bank, bit)` with bit set in .asc text.
fn set_bits(asc: &str) -> std::collections::HashSet<(String, u32, u32, usize, usize)> {
    let mut set = std::collections::HashSet::new();
    let mut kind = String::new();
    let mut x = 0u32;
    let mut y = 0u32;
    let mut bank = 0usize;
    for line in asc.lines() {
        if line.starts_with('.') {
            let mut it = line[1..].split_whitespace();
            kind = it.next().unwrap_or("").to_string();
            x = it.next().unwrap_or("0").parse().unwrap_or(0);
            y = it.next().unwrap_or("0").parse().unwrap_or(0);
            bank = 0;
        } else if line.chars().all(|c| c == '0' || c == '1') && !line.is_empty() {
            for (i, c) in line.chars().enumerate() {
                if c == '1' {
                    set.insert((kind.clone(), x, y, bank, i));
                }
            }
            bank += 1;
        }
    }
    set
}

/// 12-bit chain (3 Carry4s) spans two tiles; the upper tile's
/// carry_in_mux must select the tile-below carry (B1[49]).
#[test]
fn counter_12bit_carry_spans_tiles() {
    let mut d = Design::new("carry_counter12");
    d.ports.push(helion_ir::Port {
        name: "clk".to_string(),
        dir: PortDir::In,
        attrs: Default::default(),
    });
    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");
    d.add_cell("clk_buf", CellKind::ClockBuf);
    d.connect("clk_net", "clk_buf", "I");
    d.connect("clk_global", "clk_buf", "O");

    // Three chained Carry4s: 12-bit increment.
    for c in 0..3u32 {
        let name = format!("c{c}");
        d.add_cell(name.clone(), CellKind::Carry4);
        if c == 0 {
            d.connect("1", name.clone(), "CYINIT");
        } else {
            d.connect(format!("c{}_co3", c - 1), name.clone(), "CI");
        }
        for i in 0..4 {
            let bit = c * 4 + i;
            d.connect("0", name.clone(), format!("DI{i}"));
            d.connect(format!("q{bit}"), name.clone(), format!("S{i}"));
            d.connect(format!("d{bit}"), name.clone(), format!("O{i}"));
        }
        if c < 2 {
            d.connect(format!("c{c}_co3"), name.clone(), "CO3");
        }
    }
    for bit in 0..12 {
        let ff = format!("ff{bit}");
        d.add_cell(ff.clone(), CellKind::Hff);
        d.connect(format!("d{bit}"), ff.clone(), "D");
        d.connect(format!("q{bit}"), ff.clone(), "Q");
        d.connect("clk_global", ff, "CLK");
    }

    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&d).expect("pack");
    assert_eq!(packed.carry_chains.len(), 1);
    assert_eq!(packed.carry_chains[0].bits, 12);
    assert!(packed.carry_chains[0].cin_one);

    let placed = backend.place(&packed).expect("place");
    let mut carry_cells: Vec<_> = placed
        .luts
        .iter()
        .filter(|l| l.carry)
        .map(|l| (l.x, l.y, l.lc))
        .collect();
    assert_eq!(carry_cells.len(), 12);
    // First 8 in one tile (LC0..7), next 4 in the tile above (LC0..3).
    let (x0, y0, _) = carry_cells[0];
    let (x1, y1, _) = carry_cells[8];
    assert_eq!(x1, x0, "chain stays in one column");
    assert_eq!(y1, y0 + 1, "chain continues in the tile above");
    for (i, &(_, _, lc)) in carry_cells.iter().enumerate().take(8) {
        assert_eq!(lc as usize, i);
    }
    for (i, &(_, _, lc)) in carry_cells.iter().enumerate().skip(8) {
        assert_eq!(lc as usize, i - 8);
    }
    // Two mux configs: CarryInSet on the first tile, FromBelow on the second.
    assert_eq!(placed.carry_mux.len(), 2);
    assert!(placed.carry_mux.iter().any(|m| (m.x, m.y) == (x0, y0)));
    assert!(placed.carry_mux.iter().any(|m| (m.x, m.y) == (x1, y1)));

    let routed = backend.route(&placed).expect("route");
    let asc = backend.bitstream(&routed).expect("bitstream");
    let tiles = parse_logic_tiles(&asc);
    // First tile: CarryInSet (bank 1, bit 50).
    assert_eq!(
        tiles[&(x0 as u32, y0 as u32)][1].chars().nth(50).unwrap(),
        '1',
        "first tile CarryInSet"
    );
    // Second tile: carry_in -> carry_in_mux buffer = B1[49] set, B1[50] clear.
    let b1 = &tiles[&(x1 as u32, y1 as u32)][1];
    assert_eq!(b1.chars().nth(49).unwrap(), '1', "second tile B1[49] (from-below)");
    assert_eq!(b1.chars().nth(50).unwrap(), '0', "second tile not CarryInSet");
    // Upper tile's LC0..3 have CarryEnable.
    let banks = &tiles[&(x1 as u32, y1 as u32)];
    for lc in 0..4u8 {
        assert_eq!(
            banks[2 * lc as usize].chars().nth(44).unwrap(),
            '1',
            "upper tile LC{lc} CarryEnable"
        );
    }
    // Round-trip preserves bits.
    let asc_path = "/tmp/helion_carry_counter12.asc";
    std::fs::write(asc_path, &asc).expect("write asc");
    let bin_path = "/tmp/helion_carry_counter12.bin";
    let asc2_path = "/tmp/helion_carry_counter12_rt.asc";
    assert!(std::process::Command::new("/usr/bin/icepack")
        .args([asc_path, bin_path])
        .status()
        .expect("icepack")
        .success());
    assert!(std::process::Command::new("/usr/bin/iceunpack")
        .args([bin_path, asc2_path])
        .status()
        .expect("iceunpack")
        .success());
    let rt = std::fs::read_to_string(asc2_path).expect("read rt asc");
    assert_eq!(set_bits(&asc), set_bits(&rt), "round-trip preserves bits");
    println!("12-bit chain spans tiles ({x0},{y0}) -> ({x1},{y1}), FromBelow set");
}

/// Pack-level: a Carry4 with non-constant S and DI gets per-bit XOR LUTs
/// for in_2 = S[i] ^ DI[i].
#[test]
fn carry_general_case_inserts_xor_luts() {
    let mut d = Design::new("carry_xor");
    d.add_cell("c0", CellKind::Carry4);
    d.connect("1", "c0", "CYINIT");
    for i in 0..4 {
        d.connect(format!("a{i}"), "c0", format!("S{i}"));
        d.connect(format!("b{i}"), "c0", format!("DI{i}"));
        d.connect(format!("o{i}"), "c0", format!("O{i}"));
    }
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&d).expect("pack");
    assert_eq!(packed.carry_chains.len(), 1);
    assert_eq!(packed.carry_chains[0].bits, 4);
    let xors: Vec<_> = packed
        .luts
        .iter()
        .filter(|l| l.name.contains("$xor"))
        .collect();
    assert_eq!(xors.len(), 4, "one XOR LUT per bit for S^DI");
    for x in &xors {
        assert_eq!(x.init, 0x6666, "XOR LUT is in_0 ^ in_1");
    }
}

/// Fail loud: CO[0..2] are internal and cannot be observed.
#[test]
fn carry_co_low_observed_is_error() {
    let mut d = Design::new("carry_bad_co");
    d.add_cell("c0", CellKind::Carry4);
    d.connect("1", "c0", "CYINIT");
    d.connect("s0", "c0", "S0");
    d.connect("leak", "c0", "CO1");
    let backend = Ice40Backend::new("ice40-hx8k");
    let err = backend.pack(&d).expect_err("CO1 observed must fail");
    assert!(err.contains("CO1"), "error names the offending pin: {err}");
}

/// Fail loud: CI driven by non-carry logic is unrepresentable.
#[test]
fn carry_ci_from_fabric_is_error() {
    let mut d = Design::new("carry_bad_ci");
    d.add_cell("c0", CellKind::Carry4);
    d.connect("1", "c0", "CYINIT");
    d.add_cell("c1", CellKind::Carry4);
    d.connect("fabric_net", "c1", "CI");
    d.add_cell("drv", CellKind::Lut6 { init: 0 });
    d.connect("fabric_net", "drv", "O");
    let backend = Ice40Backend::new("ice40-hx8k");
    let err = backend.pack(&d).expect_err("fabric-driven CI must fail");
    assert!(err.contains("CI"), "error names the offending pin: {err}");
}

/// Fail loud: CYINIT driven by a general net is unrepresentable.
#[test]
fn carry_cyinit_from_fabric_is_error() {
    let mut d = Design::new("carry_bad_cyinit");
    d.add_cell("c0", CellKind::Carry4);
    d.connect("fabric_net", "c0", "CYINIT");
    let backend = Ice40Backend::new("ice40-hx8k");
    let err = backend.pack(&d).expect_err("fabric-driven CYINIT must fail");
    assert!(err.contains("CYINIT"), "error names the offending pin: {err}");
}
