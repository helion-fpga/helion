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
use helion_ir::{CellKind, Design, PortDir};
use std::collections::HashMap;
use std::process::Command;

fn set_attr(d: &mut Design, cell: &str, key: &str, val: &str) {
    d.cells
        .iter_mut()
        .find(|c| c.name == cell)
        .expect("cell exists")
        .attrs
        .set(key, val);
}

fn port(d: &mut Design, name: &str, dir: PortDir) {
    d.ports.push(helion_ir::Port {
        name: name.to_string(),
        dir,
        attrs: Default::default(),
    });
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
            module: module.to_string(),
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
        .cells
        .iter_mut()
        .find(|c| c.name == "pll0")
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
