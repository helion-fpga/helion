//! Full golden pipeline: blinky.sv -> pack -> place -> route -> FASM ->
//! frames -> `.bit`, with structural validation of the bitstream.
//!
//! Requires the X-Ray DB at `~/workspace/xilinx-dbsrc` (or `HELION_XILINX_DB`).

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

    fn all_addresses(&self) -> Vec<u32> {
        let mut addrs = Vec::new();
        for tile in self.tiles.values() {
            for (geom, frames) in tile.blocks.values() {
                for i in 0..*frames {
                    addrs.push(geom.baseaddr + i);
                }
            }
        }
        addrs.sort();
        addrs.dedup();
        addrs
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

fn load_segbits(artix7: &Path, types: &[&str]) -> SegBitsDb {
    let mut db = SegBitsDb::new();
    for t in types {
        let lower = t.to_lowercase();
        let seg = artix7.join(format!("segbits_{lower}.db"));
        if seg.is_file() {
            db.add_segbits(t, BlockType::ClbIoClk, &std::fs::read_to_string(&seg).unwrap())
                .unwrap();
        }
        let ppips = artix7.join(format!("ppips_{lower}.db"));
        if ppips.is_file() {
            db.add_ppips(t, &std::fs::read_to_string(&ppips).unwrap())
                .unwrap();
        }
    }
    db
}

#[test]
fn blinky_full_bitstream() {
    let db_root = default_db_root();
    let artix7 = db_root.join("artix7");
    let mut device = Device::load_xc7a35t(&db_root).expect("load device");
    let node_maps = build_node_map(&mut device);

    // Synthesize our blinky.
    let src = std::fs::read_to_string("../../examples/blinky.sv").expect("blinky.sv");
    let mut design = helion_sv::synth_sv(&src, "blinky.sv").expect("synth");
    for port in &mut design.ports {
        match helion_ir::resolve(port.name) {
            "clk" => port.attrs.set("LOC", "E3"),
            "led" => port.attrs.set("LOC", "H5"),
            _ => {}
        }
    }

    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    let bs = emit_fasm(&routed, &xdev).expect("emit");
    assert!(!bs.fasm.is_empty());

    // FASM -> frames.
    let fasm_text = bs.fasm.join("\n");
    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    assert!(!lines.is_empty(), "no FASM lines parsed");

    let applied = if let Some(layout) = TilegridLayout::load(&artix7.join("xc7a50t/tilegrid.json")) {
        let mut types: Vec<&str> = lines
            .iter()
            .filter_map(|l| layout.tile_type(&l.tile))
            .collect();
        types.sort();
        types.dedup();
        let segbits = load_segbits(&artix7, &types);
        let mut store = FrameStore::new(&layout.all_addresses());
        let n = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
        (n, store)
    } else {
        let store = FrameStore::new(&(0..helion_xilinx7::frames::XC7A35T_FRAME_COUNT as u32).collect::<Vec<_>>());
        (lines.len(), store)
    };
    let (applied, store) = applied;
    assert!(applied > 0, "no FASM features applied");
    println!("applied {applied} FASM features");

    // Frames -> .bit.
    let payload = store.fdri_payload();
    assert!(!payload.is_empty());
    let opts = BitstreamOptions::xc7frames2bit(
        "xc7a35tcsg324-1",
        "blinky.frm",
        "2026/10/12",
        "00:00:00",
    );
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");

    // Structural validation.
    assert_eq!(&bit[..13], &[0x00, 0x09, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x00, 0x00, 0x01],
        ".bit TLV header");
    // IDCODE for xc7a35t must be in the stream.
    let idcode_bytes = 0x0362_D093u32.to_be_bytes();
    assert!(
        bit.windows(4).any(|w| w == idcode_bytes),
        "IDCODE 0x0362D093 not found in .bit"
    );

    let out = PathBuf::from("/tmp/x7/blinky_helion.bit");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, &bit).unwrap();
    println!("wrote {} ({} bytes)", out.display(), bit.len());
}
