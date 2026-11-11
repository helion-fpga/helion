//! Assemble a FASM file into a 7-series `.bit` with Helion's own encoder
//! (`FrameStore::apply_fasm` + `bitstream::write_bit`).
//!
//! This is the encoder half of the golden round-trip check: decode a Vivado
//! bitstream to FASM (tools/x7v), assemble it here, and diff the result
//! against the Vivado original. Lines the encoder rejects are reported, not
//! silently dropped.
//!
//! ```text
//! cargo run --release -p helion-xilinx7 --example fasm2bit -- in.fasm out.bit [xc7z020clg484-1]
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::device::default_db_root;
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{
    full_device_addresses_from_dir, BlockType, FrameAddr, FrameStore, SegBitsDb, TileAlias,
    TileBitLayout,
};
use serde_json::Value;

struct TileEntry {
    tile_type: String,
    blocks: HashMap<BlockType, FrameAddr>,
    alias: Option<TileAlias>,
}

struct TilegridLayout {
    tiles: HashMap<String, TileEntry>,
}

impl TilegridLayout {
    fn load(tilegrid: &Path) -> Self {
        let json: Value =
            serde_json::from_str(&std::fs::read_to_string(tilegrid).expect("read tilegrid"))
                .expect("parse tilegrid");
        let mut tiles = HashMap::new();
        for (name, info) in json.as_object().expect("tilegrid object") {
            let tile_type = info["type"].as_str().unwrap_or_default().to_string();
            let mut blocks = HashMap::new();
            let mut alias = None;
            if let Some(bits) = info.get("bits").and_then(|b| b.as_object()) {
                for (bus, b) in bits {
                    let block = match bus.as_str() {
                        "CLB_IO_CLK" => BlockType::ClbIoClk,
                        "BLOCK_RAM" => BlockType::BlockRam,
                        _ => continue,
                    };
                    let base = b["baseaddr"].as_str().unwrap_or("0x0");
                    let baseaddr =
                        u32::from_str_radix(base.trim_start_matches("0x"), 16).unwrap_or(0);
                    let offset = b["offset"].as_u64().unwrap_or(0) as u32;
                    blocks.insert(block, FrameAddr { baseaddr, offset });
                    if let Some(a) = b.get("alias") {
                        alias = Some(TileAlias {
                            tile_type: a["type"].as_str().unwrap_or_default().to_string(),
                            start_offset: a["start_offset"].as_u64().unwrap_or(0) as u32,
                        });
                    }
                }
            }
            tiles.insert(name.clone(), TileEntry { tile_type, blocks, alias });
        }
        TilegridLayout { tiles }
    }
}

impl TileBitLayout for TilegridLayout {
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr> {
        self.tiles.get(tile)?.blocks.get(&block).copied()
    }
    fn tile_type(&self, tile: &str) -> Option<&str> {
        Some(self.tiles.get(tile)?.tile_type.as_str())
    }
    fn alias(&self, tile: &str, _block: BlockType) -> Option<TileAlias> {
        self.tiles.get(tile)?.alias.clone()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: fasm2bit <in.fasm> <out.bit> [part]");
        std::process::exit(2);
    }
    let part = args.get(3).map(String::as_str).unwrap_or("xc7z020clg484-1");
    let fabric = part.split("clg").next().unwrap_or(part);
    let root = default_db_root().join("zynq7");
    let layout = TilegridLayout::load(&root.join(format!("{fabric}/tilegrid.json")));
    let text = std::fs::read_to_string(&args[1]).expect("read fasm");
    let lines = parse_fasm(&text).expect("parse fasm");

    let types: BTreeSet<String> = lines
        .iter()
        .filter_map(|l| {
            layout
                .alias(&l.tile, BlockType::ClbIoClk)
                .map(|a| a.tile_type)
                .or_else(|| layout.tile_type(&l.tile).map(str::to_string))
        })
        .collect();
    let mut segbits = SegBitsDb::new();
    for t in &types {
        let lower = t.to_lowercase();
        for (suffix, block) in [("", BlockType::ClbIoClk), (".block_ram", BlockType::BlockRam)] {
            let p = root.join(format!("segbits_{lower}{suffix}.db"));
            if p.is_file() {
                segbits
                    .add_segbits(t, block, &std::fs::read_to_string(&p).unwrap())
                    .unwrap();
            }
        }
        let pp = root.join(format!("ppips_{lower}.db"));
        if pp.is_file() {
            segbits
                .add_ppips(t, &std::fs::read_to_string(&pp).unwrap())
                .unwrap();
        }
    }

    let full = full_device_addresses_from_dir(&root.join(part)).expect("part.json");
    let mut store = FrameStore::new(&full);
    let mut applied = 0usize;
    let mut rejected: BTreeMap<String, usize> = BTreeMap::new();
    for line in &lines {
        match store.apply_fasm(std::slice::from_ref(line), &segbits, &layout) {
            Ok(n) => applied += n,
            Err(e) => {
                let ty = layout.tile_type(&line.tile).unwrap_or("?").to_string();
                *rejected.entry(ty).or_default() += 1;
                eprintln!("REJECT {}.{}: {e}", line.tile, line.feature);
            }
        }
    }
    let payload = store.fdri_payload();
    let opts = BitstreamOptions {
        idcode: 0x0372_7093,
        part_name: part.to_string(),
        design_source: "fasm2bit;Generator=helion-xilinx7".to_string(),
        write_crc: true,
        ..BitstreamOptions::xc7frames2bit(part, "fasm2bit.frm", "2026/11/11", "00:00:00")
    };
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");
    std::fs::write(&args[2], &bit).expect("write out");
    println!(
        "applied={applied} rejected_lines={} by_tile_type={rejected:?} bytes={}",
        rejected.values().sum::<usize>(),
        bit.len()
    );
}
