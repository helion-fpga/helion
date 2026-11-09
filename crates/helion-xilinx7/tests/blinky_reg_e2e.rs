//! End-to-end: examples/blinky_reg.sv -> pack -> place -> route -> FASM ->
//! frames -> `.bit` for xc7z020 (ZedBoard).
//!
//! blinky_reg.sv is the faithful SystemVerilog equivalent of the Vivado
//! oracle source ~/CDA4253_A1/blinky.vhd (GT-blinky.bit, md5
//! 3f6d7cfa8a5db206e5cf6fd0e3568324): 25-bit counter plus registered LD0,
//! 26 flops. Same part (xc7z020clg484-1), same pins (clk Y9, LD0 T22),
//! same 100 MHz clock. This is the Helion half of the ZB-10 Vivado diff
//! pair. Requires the X-Ray zynq7 DB (`HELION_XILINX_DB` or the
//! `default_db_root` fallbacks).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::clkdist::{self, HrowTap, HrowTile};
use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::route::RouteQuery;
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{
    full_device_addresses_from_dir, BlockType, FrameAddr, FrameStore, SegBitsDb,
    TileBitLayout,
};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};
use serde_json::Value;

struct TileEntry {
    tile_type: String,
    blocks: HashMap<BlockType, (FrameAddr, u32)>,
}

struct TilegridLayout {
    tiles: HashMap<String, TileEntry>,
}

impl TilegridLayout {
    fn load(tilegrid: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(tilegrid).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        let obj = json.as_object()?;
        let mut tiles = HashMap::new();
        for (name, info) in obj {
            let tile_type = info.get("type")?.as_str()?.to_string();
            let mut blocks = HashMap::new();
            if let Some(bits) = info.get("bits").and_then(|b| b.as_object()) {
                for (bus, b) in bits {
                    let block = match bus.as_str() {
                        "CLB_IO_CLK" => BlockType::ClbIoClk,
                        "BLOCK_RAM" => BlockType::BlockRam,
                        _ => continue,
                    };
                    let baseaddr = b.get("baseaddr")?.as_str()?;
                    let baseaddr =
                        u32::from_str_radix(baseaddr.strip_prefix("0x").unwrap_or(baseaddr), 16)
                            .ok()?;
                    let offset = b.get("offset")?.as_u64()? as u32;
                    let frames = b.get("frames")?.as_u64()? as u32;
                    blocks.insert(block, (FrameAddr { baseaddr, offset }, frames));
                }
            }
            tiles.insert(name.clone(), TileEntry { tile_type, blocks });
        }
        Some(TilegridLayout { tiles })
    }
}

impl TileBitLayout for TilegridLayout {
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr> {
        Some(self.tiles.get(tile)?.blocks.get(&block)?.0)
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        Some(self.tiles.get(tile)?.tile_type.as_str())
    }
}

fn load_segbits(zynq7: &Path, types: &[&str]) -> SegBitsDb {
    let mut db = SegBitsDb::new();
    for t in types {
        let lower = t.to_lowercase();
        let seg = zynq7.join(format!("segbits_{lower}.db"));
        if seg.is_file() {
            db.add_segbits(t, BlockType::ClbIoClk, &std::fs::read_to_string(&seg).unwrap())
                .unwrap();
        }
        let ppips = zynq7.join(format!("ppips_{lower}.db"));
        if ppips.is_file() {
            db.add_ppips(t, &std::fs::read_to_string(&ppips).unwrap())
                .unwrap();
        }
    }
    db
}

/// IDCODE of xc7z020 (from zynq7/xc7z020clg484-1/part.json).
const IDCODE_XC7Z020: u32 = 0x0372_7093;

#[test]
fn blinky_reg_e2e_bitstream() {
    let db_root = default_db_root();
    let zynq7 = db_root.join("zynq7");
    let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
    assert!(device.tile_count() > 10000, "tile_count = {}", device.tile_count());
    let node_maps = build_node_map(&mut device);

    // Synthesize the GT-equivalent blinky: 25-bit counter + registered LD0.
    let src =
        std::fs::read_to_string("../../examples/blinky_reg.sv").expect("blinky_reg.sv");
    let mut design = helion_sv::synth_sv(&src, "blinky_reg.sv").expect("synth");
    println!(
        "synth: {} cells, {} ports",
        design.cells.len(),
        design.ports.len()
    );

    // LOC on the clk port (single-bit). Same pin as the Vivado build (Y9).
    for port in &mut design.ports {
        if port.name == "clk" {
            port.attrs.set("LOC", "Y9");
            port.attrs.set("IOSTANDARD", "LVCMOS33");
        }
    }

    let mut packed = pack(&design).expect("pack");
    // 25 counter bits + 1 registered output, like the oracle's 26 registers.
    assert_eq!(packed.ffs.len(), 26, "expected 26 FFs");
    assert!(!packed.luts.is_empty(), "no LUTs packed");
    println!(
        "pack: {} luts, {} ffs, {} iobs, {} bufgs",
        packed.luts.len(),
        packed.ffs.len(),
        packed.iobs.len(),
        packed.bufgs.len()
    );
    for iob in &packed.iobs {
        println!(
            "  iob: name={} port={} dir={:?} net={} loc={:?}",
            iob.name, iob.port, iob.dir, iob.net, iob.loc
        );
    }
    assert_eq!(packed.iobs.len(), 2, "expected clk + LD0 IOBs");

    // LD0 on T22, the same pin the Vivado build uses.
    for iob in &mut packed.iobs {
        if iob.port == "LD0" {
            iob.loc = Some("T22".to_string());
            iob.iostandard = "LVCMOS33".to_string();
        }
    }

    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }

    let placed = place(&packed, &xdev).expect("place");
    // Every IOB must sit on a distinct site (no silent stacking).
    let mut sites: Vec<String> = placed.iobs.iter().map(|i| i.site.clone()).collect();
    sites.sort();
    sites.dedup();
    assert_eq!(sites.len(), placed.iobs.len(), "duplicate IOB sites");
    println!("place: {} iobs on distinct sites", placed.iobs.len());

    // Verify each IOB landed on the DB site for its PACKAGE_PIN.
    let mut pin_site: HashMap<String, String> = HashMap::new();
    let csv = std::fs::read_to_string(
        db_root.join("zynq7/xc7z020clg484-1/package_pins.csv"),
    )
    .expect("package_pins.csv");
    for line in csv.lines().skip(1) {
        let mut cols = line.split(',');
        if let (Some(pin), Some(_bank), Some(site)) = (cols.next(), cols.next(), cols.next()) {
            pin_site.insert(pin.to_string(), site.to_string());
        }
    }
    for iob in &placed.iobs {
        let (want_pin, want_site) = if iob.port == "clk" {
            ("Y9", pin_site.get("Y9").unwrap())
        } else if iob.port == "LD0" {
            ("T22", pin_site.get("T22").unwrap())
        } else {
            continue;
        };
        assert_eq!(
            &iob.site, want_site,
            "IOB {} (pin {want_pin}) on wrong site",
            iob.name
        );
        println!("  {} pin {want_pin} -> site {}", iob.name, iob.site);
    }

    let routed = route(&placed, &xdev).expect("route");
    assert!(!routed.nets.is_empty(), "no nets routed");

    let bs = emit_fasm(&routed, &xdev).expect("emit");
    assert!(!bs.fasm.is_empty(), "empty FASM");
    let fasm_text = bs.fasm.join("\n");
    assert!(fasm_text.contains(".ALUT.INIT[63:0]"), "missing LUT INIT");
    println!("FASM: {} lines", bs.fasm.len());

    // FASM -> frames.
    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    assert!(!lines.is_empty(), "no FASM lines parsed");
    let layout =
        TilegridLayout::load(&zynq7.join("xc7z020/tilegrid.json")).expect("tilegrid layout");
    let mut types: Vec<&str> = lines.iter().filter_map(|l| layout.tile_type(&l.tile)).collect();
    types.sort();
    types.dedup();
    let segbits = load_segbits(&zynq7, &types);
    // prjxray-db has no segbits for RIOB33_SING (DB gap). Filter out FASM
    // lines for SING tiles. Documented limitation for HW validation.
    let (lines, skipped): (Vec<_>, Vec<_>) = lines.into_iter().partition(|l| {
        layout.tile_type(&l.tile).map(|t| t != "RIOB33_SING").unwrap_or(true)
    });
    if !skipped.is_empty() {
        println!(
            "WARNING: skipped {} FASM lines for RIOB33_SING tiles (no segbits in DB)",
            skipped.len()
        );
    }
    // Full device frame list in FAR order: the FAR autoincrementer advances
    // on each written frame, so a partial payload would shift every later
    // frame onto the wrong address on silicon. Untouched frames stay zero,
    // exactly like Vivado.
    let full = full_device_addresses_from_dir(
        &db_root.join("zynq7/xc7z020clg484-1"),
    )
    .expect("part.json frame list");
    assert_eq!(full.len(), 9996, "xc7z020 frame count");
    let mut store = FrameStore::new(&full);
    let applied = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
    assert!(applied > 0, "no FASM features applied");
    println!("applied {applied} FASM features");

    // Frames -> .bit with the xc7z020 IDCODE.
    let payload = store.fdri_payload();
    assert!(!payload.is_empty());
    let opts = BitstreamOptions {
        idcode: IDCODE_XC7Z020,
        part_name: "xc7z020clg484-1".to_string(),
        design_source: "blinky_reg;Generator=helion-xilinx7".to_string(),
        ..BitstreamOptions::xc7frames2bit("xc7z020clg484-1", "blinky_reg.frm", "2026/11/08", "00:00:00")
    };
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");

    // Structural validation.
    assert_eq!(
        &bit[..13],
        &[0x00, 0x09, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x00, 0x00, 0x01],
        ".bit TLV header"
    );
    let idcode_bytes = IDCODE_XC7Z020.to_be_bytes();
    assert!(
        bit.windows(4).any(|w| w == idcode_bytes),
        "IDCODE 0x03727093 not found in .bit"
    );

    let out = PathBuf::from("/tmp/x7/blinky_reg_helion.bit");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, &bit).unwrap();
    println!("wrote {} ({} bytes)", out.display(), bit.len());
}

/// End-to-end carry flow (ZB-04 stage 2g, opt-in): the same RTL through
/// `synth_sv_carry` must pack 7 chained groups plus head transit plus LD0
/// extended stage, place them, route the cascade without fabric pips, and
/// emit golden-shaped chain features (PRECYINIT.C0 once, PRECYINIT.AX once
/// for the transit, PRECYINIT.CIN eleven times, one ACY0, 46 S-LUT
/// passthrough INITs, FFMUX.XOR on chain FFs). Features must resolve to
/// real segbits (apply_fasm), proving the paths are not typos. Writes a
/// separate bit file; the default-flow oracle pair file is untouched.
#[test]
fn blinky_reg_carry_e2e_fasm() {
    let db_root = default_db_root();
    let zynq7 = db_root.join("zynq7");
    let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
    let node_maps = build_node_map(&mut device);

    let src =
        std::fs::read_to_string("../../examples/blinky_reg.sv").expect("blinky_reg.sv");
    let mut design = helion_sv::synth_sv_carry(&src, "blinky_reg.sv").expect("carry synth");
    for port in &mut design.ports {
        if port.name == "clk" {
            port.attrs.set("LOC", "Y9");
            port.attrs.set("IOSTANDARD", "LVCMOS33");
        }
    }

    let mut packed = pack(&design).expect("pack");
    assert_eq!(packed.ffs.len(), 26, "expected 26 FFs");
    assert_eq!(
        packed.carries.len(),
        13,
        "expected 7 compute groups plus 4 pads plus transit plus LD0 stage"
    );
    for iob in &mut packed.iobs {
        if iob.port == "LD0" {
            iob.loc = Some("T22".to_string());
            iob.iostandard = "LVCMOS33".to_string();
        }
    }
    // Convergence run: pin the chain head on the golden head slice
    // (SLICE_X112Y39, tile CLBLM_R_X71Y39) so the first frames overlap
    // the oracle and the diff measures encodings, not positions.
    packed.carry_anchor = Some("SLICE_X112Y39".into());

    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    assert_eq!(placed.carries.len(), 13);
    assert_eq!(placed.carries[0].site, "SLICE_X112Y39", "anchor honored");
    assert_eq!(placed.carries[0].tile, "CLBLM_R_X71Y39", "golden head tile");
    // Golden slice order: head compute on X0, transit on X1, then
    // computes on X0 and pads on X1, tail on X1, LD0 stage on Y45 X0.
    let sites: Vec<&str> = placed.carries.iter().map(|c| c.site.as_str()).collect();
    assert_eq!(
        sites,
        vec![
            "SLICE_X112Y39", "SLICE_X113Y39",
            "SLICE_X112Y40", "SLICE_X113Y40",
            "SLICE_X112Y41", "SLICE_X113Y41",
            "SLICE_X112Y42", "SLICE_X113Y42",
            "SLICE_X112Y43", "SLICE_X113Y43",
            "SLICE_X112Y44", "SLICE_X113Y44",
            "SLICE_X112Y45",
        ],
        "golden slice order"
    );
    let routed = route(&placed, &xdev).expect("route");

    let bs = emit_fasm(&routed, &xdev).expect("emit");
    let fasm_text = bs.fasm.join("\n");
    assert_eq!(fasm_text.matches("PRECYINIT.C0").count(), 1, "CYINIT-config head");
    assert_eq!(fasm_text.matches("PRECYINIT.AX").count(), 1, "head-driven transit");
    assert_eq!(
        fasm_text.matches("PRECYINIT.CIN").count(),
        11,
        "six chained computes plus four pads plus LD0 stage"
    );
    assert_eq!(fasm_text.matches("CARRY4.ACY0").count(), 1, "head bit A only");
    assert_eq!(
        fasm_text.matches("64'hFFFFFFFF00000000").count(),
        47,
        "24 compute plus 16 pad-fill plus 3 tail-fill plus 3 transit B/C/D plus LD0 (head S0 and transit S0 have their own shapes)"
    );
    // Head S0 is the inverted propagate (NOT Q0 on A3, golden Y39X0
    // ALUT shape): CYINIT is const-0, so a passthrough would stick bit
    // 0 and freeze the chain on silicon.
    assert!(
        fasm_text.contains("CLBLM_R_X71Y39.SLICEM_X0.ALUT.INIT[63:0] = 64'h0F0F0F0F0F0F0F0F"),
        "head S0 NOT-I2 INIT"
    );
    // Transit S0 is AND(Q1 on I3/A4, float-high A6): golden Y39X1
    // ALUT shape, reading as a Q1 passthrough under float-high.
    assert!(
        fasm_text.contains("CLBLM_R_X71Y39.SLICEL_X1.ALUT.INIT[63:0] = 64'hFF00FF0000000000"),
        "transit S0 AND INIT"
    );
    // Part always-on bits from required_features.fasm: static, not
    // derivable from routing, emitted unconditionally.
    for prop in ["ALWAYS_ON_PROP1", "ALWAYS_ON_PROP2", "ALWAYS_ON_PROP3"] {
        assert!(
            fasm_text.contains(&format!("CFG_CENTER_MID_X123Y84.{prop}")),
            "always-on {prop}"
        );
    }
    assert_eq!(fasm_text.matches("FFMUX.XOR").count(), 26, "chain FF D muxes");
    // Tail placement: bit 24 on Y44 X1-D, LD0 on Y45 X0-A, no fabric LD0.
    assert!(
        fasm_text.contains("CLBLM_R_X71Y44.SLICEL_X1.DFF"),
        "bit 24 at Y44X1-D"
    );
    assert!(
        !fasm_text.contains("X71Y44.SLICEL_X1.BFF"),
        "no fabric LD0 at Y44X1-B"
    );
    assert!(
        fasm_text.contains("CLBLM_R_X71Y45.SLICEM_X0.AFF"),
        "LD0 stage at Y45X0-A"
    );
    assert!(!fasm_text.contains("BCY0"), "no BCY0:\n{fasm_text}");
    println!("carry FASM: {} lines", bs.fasm.len());

    // Every chain feature must resolve to real segbits.
    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    let layout =
        TilegridLayout::load(&zynq7.join("xc7z020/tilegrid.json")).expect("tilegrid layout");
    let mut types: Vec<&str> = lines.iter().filter_map(|l| layout.tile_type(&l.tile)).collect();
    types.sort();
    types.dedup();
    let segbits = load_segbits(&zynq7, &types);
    let (lines, _): (Vec<_>, Vec<_>) = lines.into_iter().partition(|l| {
        layout.tile_type(&l.tile).map(|t| t != "RIOB33_SING").unwrap_or(true)
    });
    let full = full_device_addresses_from_dir(
        &db_root.join("zynq7/xc7z020clg484-1"),
    )
    .expect("part.json frame list");
    let mut store = FrameStore::new(&full);
    let applied = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
    assert!(applied > 0, "no FASM features applied");
    // ZB-10 clock fuzz hook: HROW trunk-distribution raw codes from the
    // routed clock spine (measured Vivado behavior, see clkdist). Only
    // single-clock designs are in the measured domain; anything else
    // keeps today's behavior (codes unset) rather than guessed bits.
    if routed.placed.bufgs.len() == 1 {
        let site = routed.placed.bufgs[0].site.clone();
        let spine = xdev.bufg_spine(&site).expect("spine for clock hook");
        if let Some(hp) = spine
            .pips
            .iter()
            .find(|p| p.tile.contains("CLK_HROW"))
        {
            let tile = if hp.tile.contains("_BOT_") {
                HrowTile::Bot
            } else if hp.tile.contains("_TOP_") {
                HrowTile::Top
            } else {
                panic!("clock hook: unexpected HROW tile {}", hp.tile);
            };
            let tail =
                hp.dst_wire.rsplit("MUX_OUT_").next().expect("mux tail");
            let (side_r, mux): (bool, u8) = match tail.strip_prefix('R') {
                Some(n) => (true, n.parse().expect("R mux index")),
                None => (
                    false,
                    tail.strip_prefix('L').expect("L mux").parse().expect("L mux index"),
                ),
            };
            let tap = HrowTap { tile, side_r, mux };
            if let Some(codes) = clkdist::hrow_codes(spine.gclk, &tap) {
                let (bot_name, top_name) = clkdist::hrow_tile_names();
                let home_name = match tap.tile {
                    HrowTile::Bot => bot_name,
                    HrowTile::Top => top_name,
                };
                assert_eq!(hp.tile, home_name, "HROW tile moved; remeasure clkdist");
                let base = |t: &str| {
                    layout
                        .frame_addr(t, BlockType::ClbIoClk)
                        .expect("hrow geometry")
                        .baseaddr
                };
                let raw = vec![
                    (base(bot_name) + 26, 50, codes.bot_26),
                    (base(bot_name) + 27, 50, codes.bot_27),
                    (base(top_name) + 27, 50, codes.top_27),
                    (
                        base(home_name) + u32::from(codes.tap_minor),
                        50,
                        codes.tap_code,
                    ),
                ];
                let n = store.apply_raw_bits(&raw).expect("hrow raw codes");
                assert_eq!(n, 4, "hrow raw codes applied");
            }
        }
    }
    // ZB-10 clock fuzz: Vivado programs HROW trunk-distribution codes in
    // word 50 that no segbits feature names (12 single-trunk samples,
    // GCLK0-3 x BOT/TOP taps, agreeing across 2017.3, 2022.1, oracle).
    // The clkdist hook above must reproduce the oracle row exactly.
    assert_eq!(
        store.frame_words(0x00421080 + 26).expect("BOT m26")[50],
        0x00041cb2,
        "BOT HROW m26 trunk code"
    );
    assert_eq!(
        store.frame_words(0x00421080 + 27).expect("BOT m27")[50],
        0x000010cf,
        "BOT HROW m27 trunk code"
    );
    assert_eq!(
        store.frame_words(0x00421080 + 28).expect("BOT m28")[50],
        0x008019b7,
        "BOT HROW m28 tap code"
    );
    assert_eq!(
        store.frame_words(0x00401080 + 27).expect("TOP m27")[50],
        0x00000643,
        "TOP HROW m27 through code"
    );

    let payload = store.fdri_payload();
    let opts = BitstreamOptions {
        idcode: IDCODE_XC7Z020,
        part_name: "xc7z020clg484-1".to_string(),
        design_source: "blinky_reg_carry;Generator=helion-xilinx7".to_string(),
        ..BitstreamOptions::xc7frames2bit("xc7z020clg484-1", "blinky_reg_carry.frm", "2026/11/08", "00:00:00")
    };
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");
    let out = PathBuf::from("/tmp/x7/blinky_reg_carry.bit");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, &bit).unwrap();
    println!("wrote {} ({} bytes)", out.display(), bit.len());
    // Carry FASM artifact for the canonical (placement-independent) chain
    // comparison against the golden decode.
    let fasm_out = PathBuf::from("/tmp/x7/blinky_reg_carry.fasm");
    std::fs::write(&fasm_out, &fasm_text).unwrap();
    println!("wrote {} ({} lines)", fasm_out.display(), bs.fasm.len());
}
