//! FASM to `.bit` with Helion's own encoder: tilegrid layout (including
//! the `_SING` tile alias), segbits/ppips loading, frame assembly with
//! per-frame ECC, and the packet stream with CRC.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::bitstream::{self, BitstreamOptions};
use crate::device::default_db_root;
use crate::fasm::parse_fasm;
use crate::frames::{full_device_addresses_from_dir, BlockType, FrameAddr, FrameStore, SegBitsDb, TileAlias, TileBitLayout};
use serde_json::Value;

pub struct TileEntry {
    tile_type: String,
    blocks: HashMap<BlockType, FrameAddr>,
    alias: Option<TileAlias>,
}

pub struct TilegridLayout {
    tiles: HashMap<String, TileEntry>,
}

impl TilegridLayout {
    pub fn load(tilegrid: &Path) -> Self {
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

/// Result of [`fasm_to_bit`].
pub struct BitgenReport {
    pub bit: Vec<u8>,
    pub applied: usize,
    /// FASM lines the encoder could not place, with the reason.
    pub rejected: Vec<String>,
    pub rejected_by_tile_type: BTreeMap<String, usize>,
}

/// Assemble FASM text into a `.bit` for `part` (a full part name such as
/// `xc7z020clg484-1` or `xc7a35tcsg324-1`; see [`crate::parts`]). The
/// family database, fabric, frame layout and IDCODE all come from the part.
pub fn fasm_to_bit(text: &str, part: &str, design: &str) -> Result<BitgenReport, String> {
    let p = crate::parts::resolve(&default_db_root(), part)?;
    let part = p.name.as_str();
    let root = p.family_dir.clone();
    let layout = TilegridLayout::load(&p.fabric_dir().join("tilegrid.json"));
    let lines = parse_fasm(text).map_err(|e| format!("{e:?}"))?;
    let types: BTreeSet<String> = lines
        .iter()
        .filter_map(|l| layout.alias(&l.tile, BlockType::ClbIoClk).map(|a| a.tile_type).or_else(|| layout.tile_type(&l.tile).map(str::to_string)))
        .collect();
    let mut segbits = SegBitsDb::new();
    for t in &types {
        let lower = t.to_lowercase();
        for (suffix, block) in [("", BlockType::ClbIoClk), (".block_ram", BlockType::BlockRam)] {
            let p = root.join(format!("segbits_{lower}{suffix}.db"));
            if p.is_file() {
                segbits.add_segbits(t, block, &std::fs::read_to_string(&p).map_err(|e| e.to_string())?).map_err(|e| format!("{e:?}"))?;
            }
        }
        if let Some(extra) = crate::dsp::extra_segbits(t) {
            segbits.add_segbits(t, BlockType::ClbIoClk, &extra).map_err(|e| format!("{e:?}"))?;
        }
        let pp = root.join(format!("ppips_{lower}.db"));
        if pp.is_file() {
            segbits.add_ppips(t, &std::fs::read_to_string(&pp).map_err(|e| e.to_string())?).map_err(|e| format!("{e:?}"))?;
        }
    }
    // Pseudo-pips of the tile's own type (`always` / `default` / `hint` in
    // ppips_<type>.db) carry no bits. A `_SING` tile's own ppips can differ
    // from its alias type's (RIOI3_SING has always-on ILOGIC exits).
    let mut bitless: std::collections::HashSet<String> = std::collections::HashSet::new();
    let own_types: BTreeSet<String> = lines.iter().filter_map(|l| layout.tile_type(&l.tile).map(str::to_string)).collect();
    for t in &own_types {
        let pp = root.join(format!("ppips_{}.db", t.to_lowercase()));
        if let Ok(text) = std::fs::read_to_string(&pp) {
            for l in text.lines() {
                if let Some(name) = l.split_whitespace().next() {
                    bitless.insert(name.to_string());
                }
            }
        }
    }
    let full = full_device_addresses_from_dir(&root.join(part)).map_err(|e| format!("{e:?}"))?;
    let mut store = FrameStore::new(&full);
    let mut applied = 0usize;
    let mut rejected = Vec::new();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for line in &lines {
        if let Some(t) = layout.tile_type(&line.tile) {
            if bitless.contains(&format!("{t}.{}", line.feature)) {
                continue;
            }
        }
        match store.apply_fasm(std::slice::from_ref(line), &segbits, &layout) {
            Ok(n) => applied += n,
            Err(e) => {
                let ty = layout.tile_type(&line.tile).unwrap_or("?").to_string();
                *by_type.entry(ty).or_default() += 1;
                rejected.push(format!("{}.{}: {e}", line.tile, line.feature));
            }
        }
    }
    // LUT RAM contents stay out of the ECC: the INIT bits of every LUT in RAM mode.
    let ram: std::collections::HashSet<(&str, &str)> = lines.iter().filter(|l| l.feature.ends_with("LUT.RAM")).map(|l| (l.tile.as_str(), l.feature.as_str())).collect();
    let ram_init: Vec<_> = lines
        .iter()
        .filter(|l| l.feature.ends_with("LUT.INIT") && ram.contains(&(l.tile.as_str(), format!("{}.RAM", &l.feature[..l.feature.len() - 5]).as_str())))
        .map(|l| {
            let mut all = l.clone();
            all.bit_range = Some((63, 0));
            all.value = crate::fasm::FasmValue::Bits { bits: u64::MAX, width: 64 };
            all
        })
        .collect();
    if !ram_init.is_empty() {
        let mut mask = FrameStore::new(&full);
        mask.apply_fasm(&ram_init, &segbits, &layout).map_err(|e| format!("{e:?}"))?;
        store.exclude_from_ecc(&mask);
    }
    let payload = store.fdri_payload();
    let opts = BitstreamOptions {
        idcode: p.idcode,
        part_name: part.to_string(),
        design_source: format!("{design};Generator=helion-xilinx7"),
        write_crc: true,
        ..BitstreamOptions::xc7frames2bit(part, &format!("{design}.frm"), "2026/11/11", "00:00:00")
    };
    let bit = bitstream::write_bit(&payload, &opts).map_err(|e| format!("{e:?}"))?;
    Ok(BitgenReport { bit, applied, rejected, rejected_by_tile_type: by_type })
}
