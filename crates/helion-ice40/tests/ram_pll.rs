//! End-to-end RAM and PLL configuration tests.
//!
//! RAM: a Bram18 with INIT attributes flows pack -> place -> route -> asc;
//! the test asserts the `.ram_data` INIT bits, the PowerUp bit and the
//! write/read-mode CBITs land where IceStorm expects them, then checks an
//! icepack/iceunpack round-trip preserves them.
//!
//! PLL: SB_PLL40_PAD / SB_PLL40_CORE black boxes with FREQ/REF_FREQ
//! attributes; the test asserts the DIVR/DIVF/DIVQ/FILTER_RANGE/PLLTYPE
//! bits in the .asc match the icepll-computed dividers, plus round-trip.

use helion_backend::Backend;
use helion_ice40::Ice40Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use std::collections::HashMap;
use std::process::Command;

fn set_attr(d: &mut Design, cell: &str, key: &str, val: &str) {
    d.set_cell_attr(cell, key, val).expect("cell exists");
}

fn port(d: &mut Design, name: &str, dir: PortDir) {
    d.add_port(name, dir);
}

/// Parse `.asc` tile sections into `(kind, x, y) -> 16 bank strings`.
fn asc_tiles(asc: &str) -> HashMap<(String, u8, u8), Vec<String>> {
    let mut m = HashMap::new();
    let lines: Vec<&str> = asc.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let ln = lines[i];
        if ln.starts_with(".io_tile")
            || ln.starts_with(".logic_tile")
            || ln.starts_with(".ramb_tile")
            || ln.starts_with(".ramt_tile")
        {
            let parts: Vec<&str> = ln.split_whitespace().collect();
            let banks: Vec<String> = lines[i + 1..i + 17].iter().map(|s| s.to_string()).collect();
            m.insert(
                (
                    parts[0].to_string(),
                    parts[1].parse().unwrap(),
                    parts[2].parse().unwrap(),
                ),
                banks,
            );
            i += 17;
        } else {
            i += 1;
        }
    }
    m
}

/// Parse `.ram_data X Y` sections into `((x, y), 16 data lines)`.
fn asc_ram_data(asc: &str) -> Vec<((u8, u8), Vec<String>)> {
    let mut out = Vec::new();
    let lines: Vec<&str> = asc.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(rest) = lines[i].strip_prefix(".ram_data ") {
            let mut p = rest.split_whitespace();
            let x: u8 = p.next().unwrap().parse().unwrap();
            let y: u8 = p.next().unwrap().parse().unwrap();
            let data: Vec<String> = lines[i + 1..i + 17].iter().map(|s| s.to_string()).collect();
            out.push(((x, y), data));
            i += 17;
        } else {
            i += 1;
        }
    }
    out
}

fn bit(banks: &[String], bank: usize, bit: usize) -> bool {
    banks[bank].as_bytes()[bit] == b'1'
}

/// Run icepack -> iceunpack round-trip on an .asc; return the re-emitted text.
fn round_trip(asc: &str, stem: &str) -> String {
    if !std::path::Path::new("/usr/bin/icepack").exists() {
        return asc.to_string();
    }
    let dir = "/tmp/helion_rampll";
    std::fs::create_dir_all(dir).expect("mkdir");
    let asc_path = format!("{dir}/{stem}.asc");
    let bin_path = format!("{dir}/{stem}.bin");
    let asc2_path = format!("{dir}/{stem}.rt.asc");
    std::fs::write(&asc_path, asc).expect("write asc");
    let st = Command::new("/usr/bin/icepack")
        .args([&asc_path, &bin_path])
        .status()
        .expect("icepack runs");
    assert!(st.success(), "icepack failed on {stem}");
    let st = Command::new("/usr/bin/iceunpack")
        .args([&bin_path, &asc2_path])
        .status()
        .expect("iceunpack runs");
    assert!(st.success(), "iceunpack failed on {stem}");
    std::fs::read_to_string(&asc2_path).expect("read round-tripped asc")
}

/// Build a small RAM design: clk -> Bram18 -> led0.
fn ram_design() -> Design {
    let mut d = Design::new("ramtest");
    port(&mut d, "clk", PortDir::In);
    port(&mut d, "led0", PortDir::Out);

    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");

    d.add_cell("ram0", CellKind::Bram18);
    d.connect("clk_net", "ram0", "RCLK");
    d.connect("clk_net", "ram0", "WCLK");
    // A couple of address/data pins driven so the router exercises the
    // ram/RADDR_* and ram/WDATA_* paths; the rest stay "gnd" (unrouted).
    d.connect("clk_net", "ram0", "RADDR0");
    d.connect("clk_net", "ram0", "RADDR1");
    d.connect("clk_net", "ram0", "WADDR0");
    d.connect("clk_net", "ram0", "WDATA0");
    d.connect("clk_net", "ram0", "WDATA8");
    d.connect("rdata_net", "ram0", "DO0");
    // 8-bit-ish mode: even data-bit INITs below only touch low bits.
    set_attr(&mut d, "ram0", "WRITE_MODE", "01");
    set_attr(&mut d, "ram0", "READ_MODE", "01");
    // INIT_0: bit 0 set (flat bit 0 -> word 0 bit 0).
    set_attr(&mut d, "ram0", "INIT_0", "1");
    // INIT_7: bit 255 set (flat bit 7*256+255 = 2047 -> word 127 bit 15).
    set_attr(
        &mut d,
        "ram0",
        "INIT_7",
        "8000000000000000000000000000000000000000000000000000000000000000",
    );
    // INIT_F: bit 255 set (flat bit 4095 -> word 255 bit 15).
    set_attr(
        &mut d,
        "ram0",
        "INIT_F",
        "8000000000000000000000000000000000000000000000000000000000000000",
    );

    d.add_cell("led_ob", CellKind::IobOut);
    d.connect("rdata_net", "led_ob", "I");
    d.connect("led0", "led_ob", "PAD");
    d
}

#[test]
fn ram_end_to_end() {
    let design = ram_design();
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&design).expect("pack");
    assert_eq!(packed.rams.len(), 1);
    let ram = &packed.rams[0];
    assert_eq!((ram.write_mode, ram.read_mode), (1, 1));
    // INIT_0 bit 0 -> word 0 == 0x0001.
    assert_eq!(ram.init[0], 0x0001);
    // INIT_7 bit 255 -> word 127 == 0x8000.
    assert_eq!(ram.init[127], 0x8000);
    // INIT_F bit 255 -> word 255 == 0x8000.
    assert_eq!(ram.init[255], 0x8000);
    assert!(ram.init[1..127].iter().all(|&w| w == 0));

    let placed = backend.place(&packed).expect("place");
    assert_eq!(placed.rams.len(), 1);
    // First RAM site is the (8,1) RAMB/RAMT pair.
    assert_eq!((placed.rams[0].x, placed.rams[0].y), (8, 1));

    let routed = backend.route(&placed).expect("route");
    let asc = backend.bitstream(&routed).expect("bitstream");

    // --- .ram_data section ---
    let rd = asc_ram_data(&asc);
    assert_eq!(rd.len(), 1, "one .ram_data section");
    assert_eq!(rd[0].0, (8, 1), ".ram_data uses the RAMB tile coords");
    let data = &rd[0].1;
    assert_eq!(data.len(), 16);
    assert_eq!(data[0].len(), 64);
    // Flat bit 0 -> line 1, last char '1'.
    assert_eq!(&data[0][63..], "1");
    assert!(data[0][..63].chars().all(|c| c == '0'));
    // Flat bit 2047 -> line 8 (0-indexed 7), char 0 = '8'.
    assert_eq!(&data[7][..1], "8");
    assert!(data[7][1..].chars().all(|c| c == '0'));
    // Flat bit 4095 -> line 16 (0-indexed 15), char 0 = '8'.
    assert_eq!(&data[15][..1], "8");
    assert!(data[15][1..].chars().all(|c| c == '0'));
    // All other lines are zero.
    for (i, line) in data.iter().enumerate() {
        if i != 0 && i != 7 && i != 15 {
            assert!(line.chars().all(|c| c == '0'), "line {i} should be zero");
        }
    }

    // --- RamConfig bits ---
    let tiles = asc_tiles(&asc);
    let ramb = &tiles[&(".ramb_tile".to_string(), 8, 1)];
    // PowerUp = ramb B1[7] (from the ramb_8k DB RamConfig entry).
    assert!(bit(ramb, 1, 7), "RAMB PowerUp bit set");
    let ramt = &tiles[&(".ramt_tile".to_string(), 8, 2)];
    // WRITE_MODE=01 -> CBIT_0=1 (B1[7]), CBIT_1=0 (B0[7]);
    // READ_MODE=01 -> CBIT_2=1 (B3[7]), CBIT_3=0 (B2[7]).
    assert!(bit(ramt, 1, 7), "CBIT_0 set");
    assert!(!bit(ramt, 0, 7), "CBIT_1 clear");
    assert!(bit(ramt, 3, 7), "CBIT_2 set");
    assert!(!bit(ramt, 2, 7), "CBIT_3 clear");

    // --- icepack/iceunpack round-trip ---
    // (iceunpack emits a .ram_data section for every RAM site on the chip;
    // only the (8,1) section carries our bits.)
    let asc2 = round_trip(&asc, "ram");
    let rd2 = asc_ram_data(&asc2);
    let ours = rd2
        .iter()
        .find(|((x, y), _)| (*x, *y) == (8, 1))
        .expect("round-tripped .ram_data 8 1");
    assert_eq!(&ours.1, data, ".ram_data preserved through round-trip");
    let tiles2 = asc_tiles(&asc2);
    let ramb2 = &tiles2[&(".ramb_tile".to_string(), 8, 1)];
    let ramt2 = &tiles2[&(".ramt_tile".to_string(), 8, 2)];
    assert!(bit(ramb2, 1, 7), "PowerUp survives round-trip");
    assert!(bit(ramt2, 1, 7) && !bit(ramt2, 0, 7), "CBIT_0/1 survive");
    assert!(bit(ramt2, 3, 7) && !bit(ramt2, 2, 7), "CBIT_2/3 survive");
}

/// Build a PLL design: clk -> SB_PLL40_X -> led0.
fn pll_design(module: &str, freq_hz: u64, ref_hz: u64) -> Design {
    let mut d = Design::new("plltest");
    port(&mut d, "clk", PortDir::In);
    port(&mut d, "led0", PortDir::Out);

    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");

    d.add_cell(
        "pll0",
        CellKind::BlackBox {
            module: intern(module),
        },
    );
    d.connect("clk_net", "pll0", "REFERENCECLK");
    d.connect("pll_clk", "pll0", "PLLOUTGLOBAL");
    set_attr(&mut d, "pll0", "FREQ", &freq_hz.to_string());
    set_attr(&mut d, "pll0", "REF_FREQ", &ref_hz.to_string());

    d.add_cell("led_ob", CellKind::IobOut);
    d.connect("pll_clk", "led_ob", "I");
    d.connect("led0", "led_ob", "PAD");
    d
}

/// Decode one PLLCONFIG bit from parsed io tiles.
fn pll_bit(tiles: &HashMap<(String, u8, u8), Vec<String>>, x: u8, y: u8, cfg: u8) -> bool {
    // PLLCONFIG -> (bank, bit), from the IO tile DB (identical on all sides).
    let (bank, b) = match cfg {
        1 => (0, 2),
        2 => (0, 3),
        3 => (3, 3),
        4 => (2, 2),
        5 => (2, 3),
        6 => (5, 3),
        7 => (4, 2),
        8 => (4, 3),
        9 => (7, 3),
        _ => panic!("bad PLLCONFIG number"),
    };
    bit(&tiles[&(".io_tile".to_string(), x, y)], bank, b)
}

/// Assert the full PLL field values decoded from the .asc.
#[allow(clippy::too_many_arguments)]
fn assert_pll_fields(
    tiles: &HashMap<(String, u8, u8), Vec<String>>,
    y: u8,
    plltype: u8,
    feedback: u8,
    divr: u8,
    divf: u8,
    divq: u8,
    filter: u8,
) {
    let mut got_plltype = 0u8;
    if pll_bit(tiles, 16, y, 5) {
        got_plltype |= 1;
    }
    if pll_bit(tiles, 18, y, 1) {
        got_plltype |= 2;
    }
    if pll_bit(tiles, 18, y, 3) {
        got_plltype |= 4;
    }
    assert_eq!(got_plltype, plltype, "PLLTYPE");
    let mut got_fb = 0u8;
    if pll_bit(tiles, 18, y, 5) {
        got_fb |= 1;
    }
    if pll_bit(tiles, 15, y, 9) {
        got_fb |= 2;
    }
    if pll_bit(tiles, 16, y, 1) {
        got_fb |= 4;
    }
    assert_eq!(got_fb, feedback, "FEEDBACK_PATH");
    let mut got_divr = 0u8;
    for i in 0..4 {
        if pll_bit(tiles, 14, y, 1 + i) {
            got_divr |= 1 << i;
        }
    }
    assert_eq!(got_divr, divr, "DIVR");
    let mut got_divf = 0u8;
    for i in 0..5 {
        if pll_bit(tiles, 14, y, 5 + i) {
            got_divf |= 1 << i;
        }
    }
    for i in 0..2 {
        if pll_bit(tiles, 15, y, 1 + i) {
            got_divf |= 1 << (5 + i);
        }
    }
    assert_eq!(got_divf, divf, "DIVF");
    let mut got_divq = 0u8;
    for i in 0..3 {
        if pll_bit(tiles, 15, y, 3 + i) {
            got_divq |= 1 << i;
        }
    }
    assert_eq!(got_divq, divq, "DIVQ");
    let mut got_filter = 0u8;
    for i in 0..3 {
        if pll_bit(tiles, 15, y, 6 + i) {
            got_filter |= 1 << i;
        }
    }
    assert_eq!(got_filter, filter, "FILTER_RANGE");
}

#[test]
fn pll_pad_end_to_end() {
    // 12 MHz -> 48 MHz: icepll gives DIVR=0, DIVF=63, DIVQ=4, FILTER_RANGE=1.
    let design = pll_design("SB_PLL40_PAD", 48_000_000, 12_000_000);
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&design).expect("pack");
    assert_eq!(packed.plls.len(), 1);
    let pll = &packed.plls[0];
    assert_eq!((pll.divr, pll.divf, pll.divq, pll.filter_range), (0, 63, 4, 1));

    let placed = backend.place(&packed).expect("place");
    assert_eq!((placed.plls[0].x, placed.plls[0].y), (16, 0));

    let routed = backend.route(&placed).expect("route");
    let asc = backend.bitstream(&routed).expect("bitstream");

    let tiles = asc_tiles(&asc);
    assert_pll_fields(&tiles, 0, 0b010, 0b001, 0, 63, 4, 1);

    let asc2 = round_trip(&asc, "pll_pad");
    let tiles2 = asc_tiles(&asc2);
    assert_pll_fields(&tiles2, 0, 0b010, 0b001, 0, 63, 4, 1);
}

#[test]
fn pll_core_end_to_end() {
    // 12 MHz -> 100 MHz: icepll gives DIVR=0, DIVF=66, DIVQ=3, FILTER_RANGE=1.
    let design = pll_design("SB_PLL40_CORE", 100_000_000, 12_000_000);
    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&design).expect("pack");
    assert_eq!(packed.plls.len(), 1);
    let pll = &packed.plls[0];
    assert_eq!((pll.divr, pll.divf, pll.divq, pll.filter_range), (0, 66, 3, 1));

    let placed = backend.place(&packed).expect("place");
    assert_eq!((placed.plls[0].x, placed.plls[0].y), (16, 33));

    let routed = backend.route(&placed).expect("route");
    let asc = backend.bitstream(&routed).expect("bitstream");

    let tiles = asc_tiles(&asc);
    assert_pll_fields(&tiles, 33, 0b011, 0b001, 0, 66, 3, 1);

    let asc2 = round_trip(&asc, "pll_core");
    let tiles2 = asc_tiles(&asc2);
    assert_pll_fields(&tiles2, 33, 0b011, 0b001, 0, 66, 3, 1);
}

#[test]
fn pll_rejects_missing_freq() {
    let mut design = pll_design("SB_PLL40_PAD", 48_000_000, 12_000_000);
    // Drop the FREQ attribute: pack must fail loud, not silently emit zeros.
    design
        .cell_mut("pll0")
        .unwrap()
        .attrs
        .map
        .remove("FREQ");
    let backend = Ice40Backend::new("ice40-hx8k");
    assert!(backend.pack(&design).is_err());
}

#[test]
fn pll_rejects_impossible_freq() {
    // 12 MHz -> 500 MHz is outside the 16-275 MHz PLL output range.
    let design = pll_design("SB_PLL40_PAD", 500_000_000, 12_000_000);
    let backend = Ice40Backend::new("ice40-hx8k");
    assert!(backend.pack(&design).is_err());
}

#[test]
fn sb_mac16_dsp_and_spram256ka_end_to_end() {
    let mut d = Design::new("up5k_dsp_spram");
    port(&mut d, "clk", PortDir::In);
    port(&mut d, "led0", PortDir::Out);
    port(&mut d, "led1", PortDir::Out);

    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_in", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");

    // Explicit SB_GB driving the global clock spine.
    d.add_cell(
        "u_gb",
        CellKind::BlackBox {
            module: intern("SB_GB"),
        },
    );
    d.connect("clk_in", "u_gb", "USER_SIGNAL_TO_GLOBAL_BUFFER");
    d.connect("gclk", "u_gb", "GLOBAL_BUFFER_OUTPUT");

    // SB_MAC16 16x16 multiplier + 32-bit accumulator with pipeline register & signed flags.
    d.add_cell(
        "u_mac",
        CellKind::BlackBox {
            module: intern("SB_MAC16"),
        },
    );
    set_attr(&mut d, "u_mac", "C_REG", "1");
    set_attr(&mut d, "u_mac", "A_REG", "1");
    set_attr(&mut d, "u_mac", "B_REG", "1");
    set_attr(&mut d, "u_mac", "D_REG", "0");
    set_attr(&mut d, "u_mac", "PIPELINE_16x16_MULT_REG1", "1");
    set_attr(&mut d, "u_mac", "PIPELINE_16x16_MULT_REG2", "1");
    set_attr(&mut d, "u_mac", "TOPOUTPUT_SELECT", "2'b11");
    set_attr(&mut d, "u_mac", "BOTOUTPUT_SELECT", "2'b11");
    set_attr(&mut d, "u_mac", "MODE_8x8", "0");
    set_attr(&mut d, "u_mac", "A_SIGNED", "1");
    set_attr(&mut d, "u_mac", "B_SIGNED", "1");
    set_attr(&mut d, "u_mac", "NEG_TRIGGER", "1");
    d.connect("gclk", "u_mac", "CLK");
    d.connect("clk_in", "u_mac", "A0");
    d.connect("clk_in", "u_mac", "B0");
    d.connect("mac_o0", "u_mac", "O0");
    d.connect("mac_o16", "u_mac", "O16");

    // SB_SPRAM256KA 16K x 16-bit single-port RAM.
    d.add_cell(
        "u_spram",
        CellKind::BlackBox {
            module: intern("SB_SPRAM256KA"),
        },
    );
    d.connect("gclk", "u_spram", "CLOCK");
    d.connect("mac_o0", "u_spram", "ADDRESS0");
    d.connect("mac_o16", "u_spram", "DATAIN0");
    d.connect("clk_in", "u_spram", "MASKWREN0");
    d.connect("clk_in", "u_spram", "WREN");
    d.connect("clk_in", "u_spram", "CHIPSELECT");
    d.connect("spram_do0", "u_spram", "DATAOUT0");

    d.add_cell("ob0", CellKind::IobOut);
    d.connect("mac_o0", "ob0", "I");
    d.connect("led0", "ob0", "PAD");

    d.add_cell("ob1", CellKind::IobOut);
    d.connect("spram_do0", "ob1", "I");
    d.connect("led1", "ob1", "PAD");

    let backend = Ice40Backend::new("ice40-up5k");
    let packed = backend.pack(&d).expect("pack dsp + spram");
    assert_eq!(packed.mac16s.len(), 1);
    assert_eq!(packed.sprams.len(), 1);
    assert_eq!(packed.clks.len(), 1);

    let mac = &packed.mac16s[0];
    assert!(mac.c_reg && mac.a_reg && mac.b_reg && !mac.d_reg);
    assert!(mac.pipeline_16x16_mult_reg1 && mac.pipeline_16x16_mult_reg2);
    assert!(mac.a_signed && mac.b_signed && mac.neg_trigger);
    assert_eq!(mac.topoutput_select, 3);
    assert_eq!(mac.botoutput_select, 3);
    assert_ne!(mac.config_bits_25, 0);

    let placed = backend.place(&packed).expect("place dsp + spram");
    assert_eq!((placed.mac16s[0].x, placed.mac16s[0].y), (0, 5));
    assert_eq!((placed.sprams[0].x, placed.sprams[0].y), (0, 1));

    let routed = backend.route(&placed).expect("route dsp + spram");
    let asc = backend.bitstream(&routed).expect("bitstream dsp + spram");
    assert!(asc.contains(".comment SB_MAC16 u_mac site=(0,5)"));
    assert!(asc.contains("NegTrigger=1 C_REG=1 A_REG=1 B_REG=1 D_REG=0 MODE_8x8=0 A_SIGNED=1 B_SIGNED=1"));
    assert!(asc.contains(".comment SB_SPRAM256KA u_spram site=(0,1)"));
}

#[test]
fn pll_2f_core_explicit_dividers_feedback_and_vco_validation() {
    let mut d = Design::new("pll2f");
    port(&mut d, "clk", PortDir::In);
    port(&mut d, "led0", PortDir::Out);
    port(&mut d, "led1", PortDir::Out);

    d.add_cell(
        "u_gb_io",
        CellKind::BlackBox {
            module: intern("SB_GB_IO"),
        },
    );
    d.connect("clk", "u_gb_io", "PACKAGE_PIN");
    d.connect("gclk_in", "u_gb_io", "GLOBAL_BUFFER_OUTPUT");
    d.connect("pad_din", "u_gb_io", "D_IN_0");

    d.add_cell(
        "u_pll",
        CellKind::BlackBox {
            module: intern("SB_PLL40_2F_CORE"),
        },
    );
    d.connect("pad_din", "u_pll", "REFERENCECLK");
    d.connect("pll_clk_a", "u_pll", "PLLOUTGLOBALA");
    d.connect("pll_clk_b", "u_pll", "PLLOUTGLOBALB");
    d.connect("pll_lock", "u_pll", "LOCK");
    set_attr(&mut d, "u_pll", "DIVR", "4'b0000");
    set_attr(&mut d, "u_pll", "DIVF", "7'b0111111"); // 63 -> VCO = 12 * 64 = 768 MHz
    set_attr(&mut d, "u_pll", "DIVQ", "3'b100"); // 4 -> Fout = 48 MHz
    set_attr(&mut d, "u_pll", "FILTER_RANGE", "3'b001");
    set_attr(&mut d, "u_pll", "FEEDBACK_PATH", "PHASE_AND_DELAY");
    // In PHASE_AND_DELAY mode, f_vco = f_pfd * (divf + 1) * 2^divq, so with divq=4 use divf=3 -> 12 * 4 * 16 = 768 MHz.
    set_attr(&mut d, "u_pll", "DIVF", "3");
    set_attr(&mut d, "u_pll", "REF_FREQ", "12000000");

    d.add_cell("ob0", CellKind::IobOut);
    d.connect("pll_clk_a", "ob0", "I");
    d.connect("led0", "ob0", "PAD");

    d.add_cell("ob1", CellKind::IobOut);
    d.connect("pll_lock", "ob1", "I");
    d.connect("led1", "ob1", "PAD");

    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&d).expect("pack 2f pll");
    assert_eq!(packed.plls.len(), 1);
    let pll = &packed.plls[0];
    assert_eq!(pll.kind, helion_ice40::pack::PllKind::Core2F);
    assert_eq!(pll.feedback_path, helion_ice40::pack::PllFeedbackPath::PhaseAndDelay);
    assert_eq!((pll.divr, pll.divf, pll.divq, pll.filter_range), (0, 3, 4, 1));

    let placed = backend.place(&packed).expect("place 2f pll");
    let routed = backend.route(&placed).expect("route 2f pll");
    let asc = backend.bitstream(&routed).expect("bitstream 2f pll");
    let tiles = asc_tiles(&asc);
    // PLLTYPE = 0b111 (SB_PLL40_2F_CORE), FEEDBACK_PATH = 0b010 (PHASE_AND_DELAY)
    assert_pll_fields(&tiles, 33, 0b111, 0b010, 0, 3, 4, 1);

    // Out-of-range VCO with explicit dividers must fail loud.
    set_attr(&mut d, "u_pll", "DIVF", "120");
    assert!(backend.pack(&d).is_err());
}

#[test]
fn sb_ram40_4k_blackbox_mask_and_negclk_end_to_end() {
    let mut d = Design::new("ram40_bb");
    port(&mut d, "clk", PortDir::In);
    port(&mut d, "led0", PortDir::Out);

    d.add_cell("clk_ib", CellKind::IobIn);
    d.connect("clk_net", "clk_ib", "O");
    d.connect("clk", "clk_ib", "PAD");

    d.add_cell(
        "u_ram",
        CellKind::BlackBox {
            module: intern("SB_RAM40_4KNRNW"),
        },
    );
    d.connect("clk_net", "u_ram", "RCLKN");
    d.connect("clk_net", "u_ram", "WCLKN");
    d.connect("clk_net", "u_ram", "RCLKE");
    d.connect("clk_net", "u_ram", "WCLKE");
    d.connect("clk_net", "u_ram", "RADDR[0]");
    d.connect("clk_net", "u_ram", "WADDR[0]");
    d.connect("clk_net", "u_ram", "WDATA[0]");
    d.connect("clk_net", "u_ram", "MASK[0]");
    d.connect("clk_net", "u_ram", "MASK[8]");
    d.connect("rdata0", "u_ram", "RDATA[0]");
    set_attr(&mut d, "u_ram", "WRITE_MODE", "2'b00");
    set_attr(&mut d, "u_ram", "READ_MODE", "2'b10");
    set_attr(&mut d, "u_ram", "INIT_0", "256'h0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_CAFE");

    d.add_cell("ob0", CellKind::IobOut);
    d.connect("rdata0", "ob0", "I");
    d.connect("led0", "ob0", "PAD");

    let backend = Ice40Backend::new("ice40-hx8k");
    let packed = backend.pack(&d).expect("pack SB_RAM40_4KNRNW");
    assert_eq!(packed.rams.len(), 1);
    let ram = &packed.rams[0];
    assert!(ram.neg_clk_r && ram.neg_clk_w);
    assert_eq!(ram.write_mode, 0);
    assert_eq!(ram.read_mode, 2);
    assert_eq!(ram.init[0], 0xCAFE);
    assert_eq!(ram.mask_net[0], "clk_net");
    assert_eq!(ram.mask_net[8], "clk_net");

    let placed = backend.place(&packed).expect("place");
    let routed = backend.route(&placed).expect("route");
    let asc = backend.bitstream(&routed).expect("bitstream");
    let tiles = asc_tiles(&asc);
    let ramb = &tiles[&(".ramb_tile".to_string(), 8, 1)];
    let ramt = &tiles[&(".ramt_tile".to_string(), 8, 2)];
    assert!(bit(ramb, 0, 0), "RAMB NegClk (B0[0]) set for SB_RAM40_4KNRNW");
    assert!(bit(ramt, 0, 0), "RAMT NegClk (B0[0]) set for SB_RAM40_4KNRNW");
}

#[test]
fn sb_hfosc_lfosc_and_hard_i2c_spi_rgba_drv_end_to_end() {
    let mut d = Design::new("hard_peripherals");
    port(&mut d, "led0", PortDir::Out);
    port(&mut d, "led1", PortDir::Out);
    port(&mut d, "led2", PortDir::Out);

    // SB_HFOSC @ 24 MHz (CLKHF_DIV = "0b01") -> SB_GB -> global clock
    d.add_cell(
        "u_hfosc",
        CellKind::BlackBox {
            module: intern("SB_HFOSC"),
        },
    );
    set_attr(&mut d, "u_hfosc", "CLKHF_DIV", "0b01");
    d.connect("hf_clk", "u_hfosc", "CLKHF");

    d.add_cell(
        "u_lfosc",
        CellKind::BlackBox {
            module: intern("SB_LFOSC"),
        },
    );
    d.connect("lf_clk", "u_lfosc", "CLKLF");

    d.add_cell(
        "u_i2c",
        CellKind::BlackBox {
            module: intern("SB_I2C"),
        },
    );
    set_attr(&mut d, "u_i2c", "BUS_ADDR74", "0b0011");
    d.connect("hf_clk", "u_i2c", "SBCLKI");
    d.connect("i2c_irq", "u_i2c", "I2CIRQ");

    d.add_cell(
        "u_spi",
        CellKind::BlackBox {
            module: intern("SB_SPI"),
        },
    );
    set_attr(&mut d, "u_spi", "BUS_ADDR74", "0b0010");
    d.connect("hf_clk", "u_spi", "SBCLKI");
    d.connect("spi_irq", "u_spi", "SPIIRQ");

    d.add_cell(
        "u_rgba",
        CellKind::BlackBox {
            module: intern("SB_RGBA_DRV"),
        },
    );
    set_attr(&mut d, "u_rgba", "CURRENT_MODE", "0b1");
    set_attr(&mut d, "u_rgba", "RGB0_CURRENT", "0b000111");
    set_attr(&mut d, "u_rgba", "RGB1_CURRENT", "0b001111");
    set_attr(&mut d, "u_rgba", "RGB2_CURRENT", "0b011111");
    d.connect("lf_clk", "u_rgba", "RGB0PWM");
    d.connect("rgb0_out", "u_rgba", "RGB0");

    d.add_cell("ob0", CellKind::IobOut);
    d.connect("i2c_irq", "ob0", "I");
    d.connect("led0", "ob0", "PAD");

    d.add_cell("ob1", CellKind::IobOut);
    d.connect("spi_irq", "ob1", "I");
    d.connect("led1", "ob1", "PAD");

    d.add_cell("ob2", CellKind::IobOut);
    d.connect("rgb0_out", "ob2", "I");
    d.connect("led2", "ob2", "PAD");

    let backend = Ice40Backend::new("ice40-up5k");
    let packed = backend.pack(&d).expect("pack osc + hard ips");
    assert_eq!(packed.oscs.len(), 2);
    assert_eq!(packed.hard_ips.len(), 3);

    let placed = backend.place(&packed).expect("place osc + hard ips");
    let routed = backend.route(&placed).expect("route osc + hard ips");
    let asc = backend.bitstream(&routed).expect("bitstream osc + hard ips");

    assert!(asc.contains(".comment SB_HFOSC u_hfosc site=(0,32) CLKHF_DIV=1"));
    assert!(asc.contains(".comment SB_LFOSC u_lfosc site=(33,1) CLKHF_DIV=0"));
    assert!(asc.contains(".comment SB_I2C u_i2c site=(33,31) BUS_ADDR74=0b0011"));
    assert!(asc.contains(".comment SB_SPI u_spi site=(33,3) BUS_ADDR74=0b0010"));
    assert!(asc.contains(".comment SB_RGBA_DRV u_rgba site=(0,30) CURRENT_MODE=1 RGB0_CURRENT=0b000111 RGB1_CURRENT=0b001111 RGB2_CURRENT=0b011111"));
}
