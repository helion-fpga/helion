//! End-to-end: zedboard_blinky.sv -> pack -> place -> route -> FASM ->
//! frames -> `.bit` for xc7z020 (ZedBoard).
//!
//! Per-bit PACKAGE_PINs are parsed from examples/zedboard.xdc by this driver
//! itself: `load_xdc` collapses per-bit `get_ports {led[0]}` to whole-port
//! last-wins, and `place` has no duplicate-site error, so whole-port LOCs
//! would silently stack all 8 IOBs on one site. XDC entries for ports absent
//! from the RTL (btn) are filtered.
//!
//! Requires the X-Ray zynq7 DB at `~/workspace/xilinx-dbsrc` (or
//! `HELION_XILINX_DB`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{
    BlockType, FrameAddr, FrameStore, SegBitsDb, TileBitLayout,
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

/// Parse `set_property PACKAGE_PIN <pin> [get_ports {<port>}]` lines from an
/// XDC file, keeping per-bit ports (`led[0]`) distinct. Returns
/// port -> pin.
fn parse_package_pins(xdc: &str) -> HashMap<String, String> {
    let mut pins = HashMap::new();
    for line in xdc.lines() {
        let line = line.trim();
        if !line.starts_with("set_property PACKAGE_PIN") {
            continue;
        }
        // set_property PACKAGE_PIN <pin> [get_ports {<port>}]
        let mut parts = line.split_whitespace();
        parts.next(); // set_property
        parts.next(); // PACKAGE_PIN
        let pin = match parts.next() {
            Some(p) => p.to_string(),
            None => continue,
        };
        let rest: String = parts.collect::<Vec<_>>().join(" ");
        // Extract the port inside get_ports { ... }.
        if let Some(start) = rest.find('{') {
            if let Some(end) = rest[start..].find('}') {
                let port = rest[start + 1..start + end].trim().to_string();
                pins.insert(port, pin);
            }
        }
    }
    pins
}

/// IDCODE of xc7z020 (from zynq7/xc7z020clg484-1/part.json).
const IDCODE_XC7Z020: u32 = 0x0372_7093;

#[test]
fn zedboard_e2e_bitstream() {
    let db_root = default_db_root();
    let zynq7 = db_root.join("zynq7");
    let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
    assert!(device.tile_count() > 10000, "tile_count = {}", device.tile_count());
    let node_maps = build_node_map(&mut device);

    // Synthesize the blinky.
    let src =
        std::fs::read_to_string("../../examples/zedboard_blinky.sv").expect("zedboard_blinky.sv");
    let mut design = helion_sv::synth_sv(&src, "zedboard_blinky.sv").expect("synth");
    println!(
        "synth: {} cells, {} ports",
        design.cells.len(),
        design.ports.len()
    );

    // Per-bit pins from the XDC (parsed here, not via load_xdc).
    let xdc = std::fs::read_to_string("../../examples/zedboard.xdc").expect("zedboard.xdc");
    let pins = parse_package_pins(&xdc);
    println!("parsed {} PACKAGE_PINs from XDC", pins.len());
    // clk -> Y9; led[0..7] -> T22 T21 U22 U21 V22 W22 U19 U14.
    assert_eq!(pins.get("clk").map(String::as_str), Some("Y9"));
    let led_pins = ["T22", "T21", "U22", "U21", "V22", "W22", "U19", "U14"];
    for (i, want) in led_pins.iter().enumerate() {
        let key = format!("led[{i}]");
        assert_eq!(pins.get(&key).map(String::as_str), Some(*want), "pin for {key}");
    }

    // LOC on the clk port (single-bit).
    for port in &mut design.ports {
        if port.name == "clk" {
            port.attrs.set("LOC", "Y9");
            port.attrs.set("IOSTANDARD", "LVCMOS33");
        }
    }

    // Workaround for a synth gap: bit-select assigns to output ports
    // (`assign led[0] = cnt[26]`) name the IOB input net `led_<n>`, but the
    // real driver (FF Q) stays on `cnt_<26-n>` with no connection between
    // them. Move each led IOB's I-pin endpoint onto the driver net before
    // pack. (pack rebuilds its pin index from net endpoints, so this is
    // the authoritative fixup point.)
    for n in 0..8 {
        let cell = if n == 0 { "u_iob".to_string() } else { format!("u_iob{n}") };
        let from_net = format!("led_{n}");
        let to_net = format!("cnt_{}", 26 - n);
        let from = design
            .nets
            .iter_mut()
            .find(|x| x.name == from_net)
            .unwrap_or_else(|| panic!("net {from_net} not found"));
        let removed = from.endpoints.len();
        from.endpoints.retain(|e| !(e.cell == cell && e.pin == "I"));
        assert!(from.endpoints.len() < removed, "no I pin moved for {cell}");
        let to = design
            .nets
            .iter_mut()
            .find(|x| x.name == to_net)
            .unwrap_or_else(|| panic!("net {to_net} not found"));
        to.endpoints.push(helion_ir::Endpoint {
            cell,
            pin: "I".to_string(),
        });
    }

    let mut packed = pack(&design).expect("pack");
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

    // Per-bit LOCs for the led IOBs. After the endpoint fixup above, each
    // IOB's driver net is `cnt_<m>` with led[bit] = cnt[26 - bit].
    let mut led_count = 0;
    for iob in &mut packed.iobs {
        if iob.port != "led" {
            continue;
        }
        let m: u32 = iob
            .net
            .strip_prefix("cnt_")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unexpected led driver net: {}", iob.net));
        assert!((19..=26).contains(&m), "cnt bit out of range: {m}");
        let bit = (26 - m) as usize;
        iob.loc = Some(led_pins[bit].to_string());
        iob.iostandard = "LVCMOS33".to_string();
        led_count += 1;
    }
    assert_eq!(led_count, 8, "expected 8 led IOBs");

    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }

    // Map IOB name -> led bit, carried through place for pin verification.
    let mut iob_bit: HashMap<String, usize> = HashMap::new();
    for iob in &packed.iobs {
        if iob.port != "led" {
            continue;
        }
        let m: u32 = iob.net.strip_prefix("cnt_").and_then(|s| s.parse().ok()).unwrap();
        iob_bit.insert(iob.name.clone(), (26 - m) as usize);
    }

    let placed = place(&packed, &xdev).expect("place");
    // Every IOB must sit on a distinct site (no silent stacking).
    let mut sites: Vec<String> = placed.iobs.iter().map(|i| i.site.clone()).collect();
    sites.sort();
    sites.dedup();
    assert_eq!(sites.len(), placed.iobs.len(), "duplicate IOB sites");
    println!("place: {} iobs on distinct sites", placed.iobs.len());
    println!("place: {} ffs", placed.ffs.len());
    for ff in &placed.ffs {
        println!("  ff {} -> {}.{}", ff.name, ff.tile, ff.site);
    }

    // Verify each led IOB landed on the DB site for its PACKAGE_PIN.
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
        } else if let Some(bit) = iob_bit.get(&iob.name) {
            let pin = led_pins[*bit];
            (pin, pin_site.get(pin).unwrap())
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
    std::fs::write("/tmp/x7/zedboard_blinky.fasm", &fasm_text).ok();
    for l in &bs.fasm {
        if l.contains("CLK") || l.contains("BUFG") || l.contains("HROW") {
            println!("  clock: {l}");
        }
    }

    // FASM -> frames.
    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    assert!(!lines.is_empty(), "no FASM lines parsed");
    let layout =
        TilegridLayout::load(&zynq7.join("xc7z020/tilegrid.json")).expect("tilegrid layout");
    let mut types: Vec<&str> = lines.iter().filter_map(|l| layout.tile_type(&l.tile)).collect();
    types.sort();
    types.dedup();
    let segbits = load_segbits(&zynq7, &types);
    // prjxray-db ships no segbits for the `*_SING` tile types, and they must
    // NOT be borrowed from the base type: a SING tile is a physically smaller
    // tile with a different frame layout, so RIOB33's bit positions address
    // words that do not exist in it (`word 102 outside 0..101`). The correct
    // handling is to drop those lines with a loud, explicit warning rather
    // than to either silently apply wrong bits or silently ignore them.
    //
    // Impact on the ZedBoard: LD6 (U19) and LD7 (U14) sit on RIOB33_SING
    // tiles, so they get default IOB configuration. See findings/ for the
    // upstream X-Ray gap.
    let (lines, skipped): (Vec<_>, Vec<_>) = lines
        .into_iter()
        .partition(|l| {
            layout
                .tile_type(&l.tile)
                .map(|t| !t.contains("_SING"))
                .unwrap_or(true)
        });
    if !skipped.is_empty() {
        let mut tiles: Vec<&str> = skipped.iter().map(|l| l.tile.as_str()).collect();
        tiles.sort();
        tiles.dedup();
        println!(
            "WARNING: {} FASM line(s) on {} *_SING tile(s) were dropped: upstream \
             prjxray-db has no segbits for those types and their frame layout differs \
             from the base type. Affected tiles: {}",
            skipped.len(),
            tiles.len(),
            tiles.join(", ")
        );
    }
    // Every valid frame, not the tilegrid union: the FDRI stream is
    // contiguous and omitting a frame shifts all later frames.
    let mut store = FrameStore::for_part(device.part()).expect("frame store");
    assert_eq!(store.frame_count(), 9_996, "xc7z020 has 9996 frames");
    let applied = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
    assert!(applied > 0, "no FASM features applied");
    println!("applied {applied} FASM features");

    // Frames -> .bit with the xc7z020 IDCODE.
    let payload = store.fdri_payload();
    assert!(!payload.is_empty());
    let opts = BitstreamOptions {
        idcode: IDCODE_XC7Z020,
        part_name: "xc7z020clg484-1".to_string(),
        design_source: "zedboard_blinky;Generator=helion-xilinx7".to_string(),
        ..BitstreamOptions::xc7frames2bit("xc7z020clg484-1", "zedboard_blinky.frm", "2026/10/22", "00:00:00")
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

    let out = PathBuf::from("/tmp/x7/zedboard_blinky.bit");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, &bit).unwrap();
    println!("wrote {} ({} bytes)", out.display(), bit.len());
}
