//! Static IOB test t01: synth -> pack -> place -> route -> FASM -> `.bit`.
//!
//! No clock and no FFs. If LD0-LD4 light and LD5 stays dark, the frame map
//! and the IOB output path are correct, independently of the clock network.
//! LD6/LD7 sit on RIOB33_SING tiles with no X-Ray segbits and are excluded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{BlockType, FrameAddr, FrameStore, SegBitsDb, TileBitLayout};
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

/// Parse `set_property PACKAGE_PIN <pin> [get_ports {<port>}]` lines,
/// keeping per-bit ports (`led[0]`) distinct.
fn parse_package_pins(xdc: &str) -> HashMap<String, String> {
    let mut pins = HashMap::new();
    for line in xdc.lines() {
        let line = line.trim();
        if !line.starts_with("set_property PACKAGE_PIN") {
            continue;
        }
        let mut parts = line.split_whitespace();
        parts.next();
        parts.next();
        let pin = match parts.next() {
            Some(p) => p.to_string(),
            None => continue,
        };
        let rest: String = parts.collect::<Vec<_>>().join(" ");
        if let Some(start) = rest.find('{') {
            if let Some(end) = rest[start..].find('}') {
                let port = rest[start + 1..start + end].trim().to_string();
                pins.insert(port, pin);
            }
        }
    }
    pins
}

const IDCODE_XC7Z020: u32 = 0x0372_7093;

#[test]
fn zedboard_static_t01_bitstream() {
    let db_root = default_db_root();
    let zynq7 = db_root.join("zynq7");
    let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
    let node_maps = build_node_map(&mut device);

    let src = std::fs::read_to_string("../../examples/t01_iob_static.sv")
        .expect("t01_iob_static.sv");
    let design = helion_sv::synth_sv(&src, "t01_iob_static.sv").expect("synth");

    let xdc = std::fs::read_to_string("../../examples/zedboard.xdc").expect("zedboard.xdc");
    let pins = parse_package_pins(&xdc);
    let led_pins = ["T22", "T21", "U22", "U21", "V22", "W22", "U19", "U14"];
    for (i, want) in led_pins.iter().enumerate() {
        let key = format!("led[{i}]");
        assert_eq!(pins.get(&key).map(String::as_str), Some(*want), "pin for {key}");
    }

    let mut packed = pack(&design).expect("pack");
    let mut led_count = 0;
    for iob in &mut packed.iobs {
        if iob.port != "led" {
            continue;
        }
        let bit: usize = iob
            .net
            .strip_prefix("led_")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unexpected led net {}", iob.net));
        iob.loc = Some(led_pins[bit].to_string());
        iob.iostandard = "LVCMOS33".to_string();
        led_count += 1;
    }
    assert_eq!(led_count, 8, "expected 8 led IOBs");
    for iob in &packed.iobs {
        println!("iob {} port {} net {} loc {:?}", iob.name, iob.port, iob.net, iob.loc);
    }

    let xdev = XDevice::new(&device, node_maps);
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    let bs = emit_fasm(&routed, &xdev).expect("emit");
    assert!(!bs.fasm.is_empty());
    let fasm_text = bs.fasm.join("\n");
    println!("FASM: {} lines", bs.fasm.len());
    std::fs::create_dir_all("/tmp/x7").ok();
    std::fs::write("/tmp/x7/t01_iob_static.fasm", &fasm_text).ok();

    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    let layout =
        TilegridLayout::load(&zynq7.join("xc7z020/tilegrid.json")).expect("tilegrid layout");
    let mut types: Vec<&str> = lines.iter().filter_map(|l| layout.tile_type(&l.tile)).collect();
    types.sort();
    types.dedup();
    let segbits = load_segbits(&zynq7, &types);
    // RIOB33_SING has no segbits; LD6/LD7 are excluded from the pass test.
    let (lines, skipped): (Vec<_>, Vec<_>) = lines
        .into_iter()
        .partition(|l| layout.tile_type(&l.tile).map(|t| !t.contains("_SING")).unwrap_or(true));
    if !skipped.is_empty() {
        println!(
            "WARNING: skipped {} FASM lines for *_SING tiles (no segbits)",
            skipped.len()
        );
    }

    let mut store = FrameStore::for_part(device.part()).expect("frame store");
    assert_eq!(store.frame_count(), 9_996);
    let applied = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
    assert!(applied > 0, "no FASM features applied");
    println!("applied {applied} FASM features");

    let payload = store.fdri_payload();
    let opts = BitstreamOptions {
        idcode: IDCODE_XC7Z020,
        part_name: "xc7z020clg484-1".to_string(),
        design_source: "t01_iob_static;Generator=helion-xilinx7".to_string(),
        ..BitstreamOptions::xc7frames2bit(
            "xc7z020clg484-1",
            "t01_iob_static.frm",
            "2026/10/22",
            "00:00:00",
        )
    };
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");
    let out = PathBuf::from("/tmp/x7/t01_iob_static.bit");
    std::fs::write(&out, &bit).expect("write t01 bit");
    println!("wrote {} ({} bytes)", out.display(), bit.len());
}
