//! Build ANY SystemVerilog design to a `.bit`, for whole-design diffing against
//! Vivado's output for the SAME source.
//!
//! The blinky has been the only design this flow is checked against, which has
//! made it easy to mistake a blinky-specific accident for a general rule. This
//! harness exists so a block can be re-pointed at another circuit without
//! touching any code:
//!
//! ```text
//! HELION_XILINX_DB=/path/prjxray-db \
//! HELION_SRC=/path/design.sv HELION_XDC=/path/design.xdc \
//! HELION_OUT=/tmp/x7/design.bit \
//!   cargo test --release -p helion-xilinx7 --test any_design_bit -- --nocapture
//! ```
//!
//! `HELION_XDC` may be omitted for a design with no constrained pads (the image
//! is then still assembled and diffable tile-for-tile, which is enough to
//! compare routing and CLB configuration).
//!
//! Deliberately dumb: it does not assert anything about the content. Its whole
//! job is to produce an artifact that `rawdiff.py --json` can compare against a
//! Vivado build of the same source. Asserting here would bake in today's
//! expectations and defeat the point of the diff.
//!
//! Ignores when `HELION_SRC` is unset so it stays out of the normal suite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::device::Device;
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{BlockType, FrameAddr, FrameStore, SegBitsDb, TileBitLayout};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};
use serde_json::Value;

const IDCODE_XC7Z020: u32 = 0x0372_7093;

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
            db.add_ppips(t, &std::fs::read_to_string(&ppips).unwrap()).unwrap();
        }
    }
    db
}

#[test]
fn build_any_design_to_bitstream() {
    let Ok(src_path) = std::env::var("HELION_SRC") else {
        eprintln!("any_design_bit: HELION_SRC unset, skipping");
        return;
    };
    let db_root = std::env::var("HELION_XILINX_DB")
        .map(PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout");
    let zynq7 = db_root.join("zynq7");

    let src = std::fs::read_to_string(&src_path).expect("read HELION_SRC");
    let xdc = std::env::var("HELION_XDC")
        .ok()
        .map(|p| std::fs::read_to_string(p).expect("read HELION_XDC"));

    let device = Device::load_part_from_db(&db_root, "xc7z020").expect("load xc7z020");
    let nm = build_node_map(&mut device.clone());

    let mut design = helion_sv::synth_sv(&src, &src_path).expect("synth");
    if let Some(xdc) = xdc {
        helion_xilinx7::build::PinConstraints::parse(&xdc).apply(&mut design);
    }

    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&device, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    let bs = emit_fasm(&routed, &xdev).expect("emit");

    let fasm_text = bs.fasm.join("\n");
    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    let layout = TilegridLayout::load(&zynq7.join("xc7z020/tilegrid.json")).expect("tilegrid");
    let mut types: Vec<&str> = lines.iter().filter_map(|l| layout.tile_type(&l.tile)).collect();
    types.sort();
    types.dedup();
    let segbits = load_segbits(&zynq7, &types);
    // `*_SING` tiles ship no segbits (AGENTS: LED7 is unprogrammable).
    let lines: Vec<_> = lines
        .into_iter()
        .filter(|l| layout.tile_type(&l.tile).map(|t| !t.contains("_SING")).unwrap_or(true))
        .collect();

    let mut store = FrameStore::for_part(device.part()).expect("frame store");
    assert_eq!(store.frame_count(), 9_996, "xc7z020 has 9996 frames");
    let applied = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
    assert!(applied > 0, "no FASM features applied");
    let _word50 = helion_xilinx7::frames::apply_clock_word_50(&mut store);

    let payload = store.fdri_payload();
    let opts = BitstreamOptions {
        idcode: IDCODE_XC7Z020,
        part_name: "xc7z020clg484-1".to_string(),
        design_source: "any_design;Generator=helion-xilinx7".to_string(),
        ..BitstreamOptions::xc7frames2bit(
            "xc7z020clg484-1",
            "any_design.frm",
            "2026/10/24",
            "00:00:00",
        )
    };
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");
    let idcode_bytes = IDCODE_XC7Z020.to_be_bytes();
    assert!(bit.windows(4).any(|w| w == idcode_bytes), "IDCODE missing");

    let out = std::env::var("HELION_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/x7/any_design.bit"));
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&out, &bit).unwrap();
    println!(
        "wrote {} ({} bytes, {} fasm lines, {} features)",
        out.display(),
        bit.len(),
        bs.fasm.len(),
        applied
    );
}
