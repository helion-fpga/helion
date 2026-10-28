//! A [`TileBitLayout`] backed by `tilegrid.json`.
//!
//! Frame assembly needs to know, for a tile *instance* name like
//! `RIOB33_X73Y1`, which segbits table applies (`RIOB33`) and where its words
//! live inside a frame (`baseaddr` + `offset`, with `frames` per tile). Both
//! facts are in `tilegrid.json`, so this is a direct reader for it.
//!
//! Three test files had grown their own private copy of this reader; it lives
//! here now so the library and the tests share one implementation.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::frames::{BlockType, FrameAddr, TileAlias, TileBitLayout};

/// One tile's geometry from `tilegrid.json`.
#[derive(Clone, Debug)]
pub struct TileEntry {
    /// Segbits table to use, e.g. `RIOB33`.
    pub tile_type: String,
    /// Per-bus frame geometry and frame count.
    pub blocks: HashMap<BlockType, (FrameAddr, u32)>,
    /// Grid column, when present.
    pub grid_x: Option<i64>,
    /// Grid row, when present.
    pub grid_y: Option<i64>,
    /// Borrowed layout for a tile type that ships no segbits of its own,
    /// e.g. `RIOB33_SING` hosting a slice of `RIOB33`.
    pub alias: Option<TileAlias>,
    /// Word offset into the borrowed (or own) block's window at which this
    /// instance's sites start.
    ///
    /// A row-pair of IO tiles shares one 4-word budget. A 2-word `_SING`
    /// variant placed at the bottom of the pair has `start_offset = 2` and so
    /// occupies words 2..3 of the *pair*, which is why its `offset` alone (0)
    /// does not describe where its bits go. Ignoring this field puts a
    /// `_SING` tile's configuration 2 words too low -- into its neighbour's
    /// half of the frame.
    pub start_offset: u32,
}

/// Every tile in a device, indexed by tile instance name.
#[derive(Clone, Debug, Default)]
pub struct TilegridLayout {
    tiles: HashMap<String, TileEntry>,
}

impl TilegridLayout {
    /// Parse `tilegrid.json`.
    ///
    /// Returns `Err` with the offending reason rather than a bare `None`, so a
    /// malformed database is distinguishable from a missing one.
    pub fn load(tilegrid: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(tilegrid)
            .map_err(|e| format!("{}: {e}", tilegrid.display()))?;
        Self::from_json(&text).map_err(|e| format!("{}: {e}", tilegrid.display()))
    }

    /// Parse `tilegrid.json` contents.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = json.as_object().ok_or("tilegrid root is not an object")?;
        let mut tiles = HashMap::with_capacity(obj.len());
        for (name, info) in obj {
            let tile_type = info
                .get("type")
                .and_then(|t| t.as_str())
                .ok_or_else(|| format!("tile {name}: missing \"type\""))?
                .to_string();
            let mut blocks = HashMap::new();
            let mut tile_alias: Option<TileAlias> = None;
            let mut start_offset: u32 = 0;
            if let Some(bits) = info.get("bits").and_then(|b| b.as_object()) {
                for (bus, b) in bits {
                    let block = match bus.as_str() {
                        "CLB_IO_CLK" => BlockType::ClbIoClk,
                        "BLOCK_RAM" => BlockType::BlockRam,
                        _ => continue,
                    };
                    let raw = b
                        .get("baseaddr")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| format!("tile {name}: missing baseaddr"))?;
                    let baseaddr = u32::from_str_radix(raw.strip_prefix("0x").unwrap_or(raw), 16)
                        .map_err(|e| format!("tile {name}: baseaddr {raw:?}: {e}"))?;
                    let offset = b.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let frames = b.get("frames").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    blocks.insert(block, (FrameAddr { baseaddr, offset }, frames));
                    // `alias` is per tile INSTANCE and nested under the block:
                    // one `_SING` pad maps its only site to `IOB33_Y0`, the
                    // other to `IOB33_Y1`, and they differ in whether the
                    // borrowed bits fit at all.
                    start_offset = b.get("start_offset").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    if let Some(alias) = b.get("alias").and_then(|a| a.as_object()) {
                        if let Some(base_type) = alias.get("type").and_then(|t| t.as_str()) {
                            let mut sites = std::collections::BTreeMap::new();
                            if let Some(map) = alias.get("sites").and_then(|s| s.as_object()) {
                                for (k, v) in map {
                                    if let Some(v) = v.as_str() {
                                        sites.insert(k.clone(), v.to_string());
                                    }
                                }
                            }
                            tile_alias = Some(TileAlias {
                                base_type: base_type.to_string(),
                                sites,
                            });
                        }
                    }
                }
            }
            tiles.insert(
                name.clone(),
                TileEntry {
                    tile_type,
                    blocks,
                    grid_x: info.get("grid_x").and_then(|v| v.as_i64()),
                    grid_y: info.get("grid_y").and_then(|v| v.as_i64()),
                    alias: tile_alias,
                    start_offset,
                },
            );
        }
        Ok(Self { tiles })
    }

    /// Number of tiles in the grid.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Whether the grid has no tiles.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Geometry for one tile.
    pub fn entry(&self, tile: &str) -> Option<&TileEntry> {
        self.tiles.get(tile)
    }

    /// Frame count for a tile on a bus, when known.
    pub fn frame_count(&self, tile: &str, block: BlockType) -> Option<u32> {
        Some(self.tiles.get(tile)?.blocks.get(&block)?.1)
    }

    /// Every frame address implied by the grid.
    ///
    /// This is the *union of tile bit ranges*, which is NOT the set a
    /// bitstream needs: the FDRI stream has no per-frame address, so a missing
    /// frame shifts every later frame. Use `FrameStore::for_part` for the real
    /// set and treat this only as a diagnostic. See `helion/AGENTS.md`.
    pub fn all_addresses(&self) -> Vec<u32> {
        let mut addrs = Vec::new();
        for tile in self.tiles.values() {
            for (geom, frames) in tile.blocks.values() {
                for i in 0..*frames {
                    addrs.push(geom.baseaddr + i);
                }
            }
        }
        addrs.sort_unstable();
        addrs.dedup();
        addrs
    }

    /// Distinct tile types in the grid, sorted.
    pub fn tile_types(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tiles.values().map(|t| t.tile_type.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

impl TileBitLayout for TilegridLayout {
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr> {
        let t = self.tiles.get(tile)?;
        let geom = t.blocks.get(&block)?.0;
        // A `_SING` variant shares a row-pair's word budget with its base
        // type and takes the half its `start_offset` selects. A base tile has
        // `start_offset = 0` (and no alias), so this is a no-op for it.
        let start = if t.alias.is_some() { t.start_offset } else { 0 };
        Some(FrameAddr {
            baseaddr: geom.baseaddr,
            offset: geom.offset.saturating_add(start),
        })
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        Some(self.tiles.get(tile)?.tile_type.as_str())
    }

    fn tile_alias(&self, tile: &str) -> Option<&TileAlias> {
        self.tiles.get(tile)?.alias.as_ref()
    }
}
