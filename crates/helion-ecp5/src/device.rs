//! LFE5U-85F device model for the ECP5 backend.
//!
//! Built from the vendored Project Trellis database: `tilegrid.json` (tile
//! entries and sites), `iodb.json` (package pins, PIO metadata),
//! `globals.json` (clock quadrants/taps/spines), and the per-tile-type
//! `bits.db` routing/config data (see [`crate::db`], [`crate::tiledata`]).
//!
//! Coordinate systems: the tile *grid* uses `(row, col)` parsed from tilegrid
//! keys (`R11C100:PLC2`, `MIB_R10C105:MIB_DSP0`), rows `0..=95`, cols
//! `0..=126`. Primitive *sites* carry Trellis bel coordinates
//! (`pos_col`/`pos_row` in the JSON), a different space used for bel-level
//! placement. Several tile entries can share one grid position (stacked
//! CIB/MIB/TAP tiles), and Trellis lists some sites (EBR, DSP, PLL, DCC) in
//! more than one adjacent tile entry, so site counts below are reported both
//! as raw entries and as unique bel positions.
//!
//! Fabric facts (all measured from the vendored JSON, asserted by tests):
//! PFU = `PLC2` tile, 4 slices each with 2x LUT4 + 2x FF (8 LUT4 + 8 FF per
//! PFU). Slices support ALU54A carry mode; EBR = 18Kb sysMEM (`DP16KD`);
//! DSP = MULT18X18D/MULT18X36D blocks with pre-adders and ALU54; PLL =
//! `PLLB`; IOBs are DDR-capable. DCU/SERDES tiles exist in the DB but are
//! out of scope for v1.

use crate::db::{
    parse_bits_db, parse_globals, parse_iodb, parse_tilegrid, Globals, IoDb, PackagePin,
    PioMetadata, TileBits,
};
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

/// Grid rows on LFE5U-85F (0..=95), from the tilegrid keys.
pub const GRID_ROWS: u32 = 96;
/// Grid cols on LFE5U-85F (0..=126), from the tilegrid keys.
pub const GRID_COLS: u32 = 127;

/// One tile entry: a tilegrid.json record at a grid position.
#[derive(Clone, Debug)]
pub struct Tile {
    /// Full tilegrid key, e.g. `"R11C100:PLC2"`.
    pub key: String,
    /// Grid row parsed from the key.
    pub row: u32,
    /// Grid column parsed from the key.
    pub col: u32,
    /// Trellis tile type, e.g. `"PLC2"`.
    pub tile_type: String,
    /// Primitive sites hosted by this tile.
    pub sites: Vec<Site>,
    /// First config frame of this tile (bitstream emission).
    pub start_frame: u32,
    /// First config bit of this tile (bitstream emission).
    pub start_bit: u32,
    /// Config frames spanned.
    pub cols: u32,
    /// Config rows spanned.
    pub rows: u32,
}

/// One primitive site: a named bel at a bel-coordinate position.
#[derive(Clone, Debug)]
pub struct Site {
    /// Site name, e.g. `"SLICE_A"`, `"PIOA"`, `"EBR0"`, `"MULT18_0"`.
    pub name: String,
    /// Bel column (Trellis bel coordinates).
    pub bel_col: i32,
    /// Bel row (Trellis bel coordinates). Negative for sites Trellis lists
    /// in a tile adjacent to the bel's own position (e.g. aliased EBR
    /// sites in `EBR_CMUX_*` tiles).
    pub bel_row: i32,
    /// Classification of this site.
    pub class: SiteClass,
}

/// Classification of a Trellis site by name.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SiteClass {
    /// `SLICE_A`..`SLICE_D`: 2x LUT4 + 2x FF.
    Slice,
    /// `PIOA`..`PIOD`: programmable IO block.
    Pio,
    /// `IOLOGICA`..`IOLOGICD`: DDR-capable IO logic.
    IoLogic,
    /// `EBR0`..`EBR3`: 18Kb sysMEM block RAM site.
    Ebr,
    /// `MULT18_*`, `MULT9_*`, `PRADD*_*`, `ALU54_*`, `ALU24_*`: DSP bels.
    Dsp,
    /// `PLL`: PLLB site.
    Pll,
    /// `DCC*`: primary clock mux.
    Dcc,
    /// `DQS`: DQS strobe.
    Dqs,
    /// `DDRDLL`: DDR delay-locked loop.
    DdrDll,
    /// `DCS*`: DDR clock mux.
    Dcs,
    /// `CLKDIV*`: edge-clock divider.
    ClkDiv,
    /// `ECLKSYNC*`, `ECLKBRIDGECS*`, `BRGECLKSYNC*`: edge clocking.
    Eclk,
    /// `GSR`, `JTAG`, `OSC`, `SED`, `START`, `DTR`, `CCLK`: device control.
    Control,
    /// `CIBTEST`, `CLKTEST`, `BREFTEST`, `PVTTEST`, `DLLDEL*`: test/delay.
    Test,
    /// Anything else (bank refs, DCU/SERDES aliases, ...).
    Other,
}

/// Classify a Trellis site name.
pub fn classify_site(name: &str) -> SiteClass {
    if matches!(name, "SLICE_A" | "SLICE_B" | "SLICE_C" | "SLICE_D") {
        SiteClass::Slice
    } else if matches!(name, "PIOA" | "PIOB" | "PIOC" | "PIOD") {
        SiteClass::Pio
    } else if matches!(name, "IOLOGICA" | "IOLOGICB" | "IOLOGICC" | "IOLOGICD") {
        SiteClass::IoLogic
    } else if matches!(name, "EBR0" | "EBR1" | "EBR2" | "EBR3") {
        SiteClass::Ebr
    } else if name.starts_with("MULT18_")
        || name.starts_with("MULT9_")
        || name.starts_with("PRADD18_")
        || name.starts_with("PRADD9_")
        || name.starts_with("ALU54_")
        || name.starts_with("ALU24_")
    {
        SiteClass::Dsp
    } else if name == "PLL" {
        SiteClass::Pll
    } else if name.starts_with("DCC") {
        SiteClass::Dcc
    } else if name == "DQS" {
        SiteClass::Dqs
    } else if name == "DDRDLL" {
        SiteClass::DdrDll
    } else if name.starts_with("DCS") {
        SiteClass::Dcs
    } else if name.starts_with("CLKDIV") {
        SiteClass::ClkDiv
    } else if name.starts_with("ECLKSYNC")
        || name.starts_with("ECLKBRIDGECS")
        || name.starts_with("BRGECLKSYNC")
    {
        SiteClass::Eclk
    } else if matches!(
        name,
        "GSR" | "JTAG" | "OSC" | "SED" | "START" | "DTR" | "CCLK" | "USRMCLK"
    ) {
        SiteClass::Control
    } else if name.ends_with("TEST") || name.starts_with("DLLDEL") {
        SiteClass::Test
    } else {
        SiteClass::Other
    }
}

/// Resource counts measured from the Trellis DB.
///
/// `*_entries` counts raw tilegrid site records; `*_bels` counts unique
/// `(name, bel_col, bel_row)` positions, deduplicating the sites Trellis
/// lists in several adjacent tile entries. The bel counts are the honest
/// physical numbers; the entry counts are useful for DB sanity checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceCounts {
    /// Raw tilegrid entries.
    pub tile_entries: usize,
    /// Distinct grid (row, col) positions.
    pub grid_positions: usize,
    /// `PLC2` (PFU) tile entries.
    pub pfu_tiles: usize,
    /// Slices (`pfu_tiles` x 4).
    pub slices: usize,
    /// LUT4s (`slices` x 2).
    pub lut4: usize,
    /// Flip-flops (`slices` x 2).
    pub ff: usize,
    /// Physical PIO site entries.
    pub pio_entries: usize,
    /// Physical IOLOGIC site entries.
    pub iologic_entries: usize,
    /// Unique EBR bel positions.
    pub ebr_bels: usize,
    /// Unique DSP bel positions.
    pub dsp_bels: usize,
    /// PLL site entries.
    pub pll_entries: usize,
    /// Unique PLL bel positions.
    pub pll_bels: usize,
    /// Unique DCC bel positions.
    pub dcc_bels: usize,
    /// Bonded PIOs (`iodb.json` pio_metadata rows).
    pub bonded_pios: usize,
}

/// The LFE5U-85F device model.
#[derive(Clone, Debug)]
pub struct Device {
    tiles: Vec<Tile>,
    by_type: HashMap<String, Vec<usize>>,
    by_key: HashMap<String, usize>,
    by_pos: HashMap<(u32, u32), Vec<usize>>,
    counts: ResourceCounts,
    iod: IoDb,
    globals: Globals,
    bits: HashMap<String, TileBits>,
}

static GLOBAL_DEV: LazyLock<Result<Device, String>> = LazyLock::new(Device::open_uncached);

impl Device {
    fn open_uncached() -> Result<Self, String> {
        let tg = parse_tilegrid(include_str!("../data/tilegrid.json"))?;
        let iod = parse_iodb(include_str!("../data/iodb.json"))?;
        let globals = parse_globals(include_str!("../data/globals.json"))?;

        let mut tiles: Vec<Tile> = Vec::with_capacity(tg.0.len());
        for (key, e) in &tg.0 {
            let (row, col) = parse_key_pos(key)?;
            let mut sites = Vec::with_capacity(e.sites.len());
            for s in &e.sites {
                sites.push(Site {
                    name: s.name.clone(),
                    bel_col: s
                        .pos_col
                        .parse()
                        .map_err(|_| format!("bad pos_col {:?} in tile {key}", s.pos_col))?,
                    bel_row: s
                        .pos_row
                        .parse()
                        .map_err(|_| format!("bad pos_row {:?} in tile {key}", s.pos_row))?,
                    class: classify_site(&s.name),
                });
            }
            tiles.push(Tile {
                key: key.clone(),
                row,
                col,
                tile_type: e.tile_type.clone(),
                sites,
                start_frame: e.start_frame,
                start_bit: e.start_bit,
                cols: e.cols,
                rows: e.rows,
            });
        }
        // Deterministic order: sort by (row, col, tile_type).
        tiles.sort_by(|a, b| {
            (a.row, a.col, &a.tile_type).cmp(&(b.row, b.col, &b.tile_type))
        });

        let mut by_type: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_key: HashMap<String, usize> = HashMap::with_capacity(tiles.len());
        let mut by_pos: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, t) in tiles.iter().enumerate() {
            by_type.entry(t.tile_type.clone()).or_default().push(i);
            by_key.insert(t.key.clone(), i);
            by_pos.entry((t.row, t.col)).or_default().push(i);
        }

        let counts = count_resources(&tiles, &iod);

        // Parse every vendored tile type's routing/config data in parallel.
        let parsed_bits: Result<Vec<(String, TileBits)>, String> = crate::tiledata::TILE_TYPES
            .par_iter()
            .map(|&tt| {
                let text = crate::tiledata::bits_db(tt)
                    .ok_or_else(|| format!("no vendored bits.db for tile type {tt}"))?;
                Ok((tt.to_string(), parse_bits_db(text)))
            })
            .collect();
        let bits: HashMap<String, TileBits> = parsed_bits?.into_iter().collect();

        Ok(Self {
            tiles,
            by_type,
            by_key,
            by_pos,
            counts,
            iod,
            globals,
            bits,
        })
    }

    /// Parse the vendored Trellis database and build the device model.
    ///
    /// Fails loudly if any JSON file or any tile type's `bits.db` is
    /// malformed or missing. Shares the process-wide parsed model from
    /// [`Device::global`].
    pub fn open() -> Result<Self, String> {
        Self::global().cloned()
    }

    /// Process-wide device model, parsed once. Fails loudly if the vendored
    /// Trellis DB does not parse.
    pub fn global() -> Result<&'static Device, String> {
        GLOBAL_DEV.as_ref().map_err(|e| e.clone())
    }

    /// Look up a tile by its full tilegrid key in O(1).
    pub fn tile_by_key(&self, key: &str) -> Option<&Tile> {
        self.by_key.get(key).map(|&i| &self.tiles[i])
    }

    /// Indexes of tiles at grid `(row, col)` in O(1).
    pub fn tiles_at(&self, row: u32, col: u32) -> &[usize] {
        self.by_pos.get(&(row, col)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// All tile entries, sorted by (row, col, tile_type).
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Indexes of tiles with the given tile type.
    pub fn tiles_of_type(&self, tile_type: &str) -> &[usize] {
        self.by_type.get(tile_type).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Resource counts measured from the DB.
    pub fn counts(&self) -> ResourceCounts {
        self.counts
    }

    /// Package pin map: pin name -> [`PackagePin`]. `None` for unknown
    /// packages. Vendored packages: CABGA381, CABGA554, CABGA756, CSFBGA285.
    pub fn package_pins(&self, package: &str) -> Option<&HashMap<String, PackagePin>> {
        self.iod.packages.get(package)
    }

    /// Per-PIO metadata (bank, DQS group, special function).
    pub fn pio_metadata(&self) -> &[PioMetadata] {
        &self.iod.pio_metadata
    }

    /// Clock quadrants, tap columns, spine positions.
    pub fn globals(&self) -> &Globals {
        &self.globals
    }

    /// Routing muxes, config enums, and fixed connections for a tile type.
    /// Fails loudly for tile types with no vendored data.
    pub fn tile_bits(&self, tile_type: &str) -> Result<&TileBits, String> {
        self.bits
            .get(tile_type)
            .ok_or_else(|| format!("no bits.db data for tile type {tile_type}"))
    }

    /// All named wires of a tile type: every mux output, every mux input
    /// source, and every fixed-connection endpoint. This is the routing
    /// graph's node inventory for the tile; the route worker builds edges.
    pub fn wire_names(&self, tile_type: &str) -> Result<BTreeSet<String>, String> {
        let tb = self.tile_bits(tile_type)?;
        let mut names = BTreeSet::new();
        for m in &tb.muxes {
            names.insert(m.dst.clone());
            for inp in &m.inputs {
                names.insert(inp.src.clone());
            }
        }
        for (a, b) in &tb.fixed_conns {
            names.insert(a.clone());
            names.insert(b.clone());
        }
        Ok(names)
    }

    /// All sites of a classification across the device.
    pub fn sites_of_class(&self, class: SiteClass) -> Vec<&Site> {
        self.tiles
            .iter()
            .flat_map(|t| t.sites.iter())
            .filter(|s| s.class == class)
            .collect()
    }
}

/// Parse `(row, col)` from a tilegrid key like `"R11C100:PLC2"` or
/// `"MIB_R10C105:MIB_DSP0"`.
fn parse_key_pos(key: &str) -> Result<(u32, u32), String> {
    let rc = key
        .split(':')
        .next()
        .ok_or_else(|| format!("bad tilegrid key {key:?}"))?;
    // Strip the optional region prefix: "MIB_R10C105" -> "R10C105".
    // split_once (not rsplit): "TAP_R94C114" must keep "R94C114".
    let rc = match rc.split_once('_') {
        Some((_, rest)) => rest,
        None => rc,
    };
    let rc = rc.strip_prefix('R').ok_or_else(|| format!("bad tilegrid key {key:?}"))?;
    let (r, c) = rc
        .split_once('C')
        .ok_or_else(|| format!("bad tilegrid key {key:?}"))?;
    let row: u32 = r.parse().map_err(|_| format!("bad row in tilegrid key {key:?}"))?;
    let col: u32 = c.parse().map_err(|_| format!("bad col in tilegrid key {key:?}"))?;
    Ok((row, col))
}

fn count_resources(tiles: &[Tile], iod: &IoDb) -> ResourceCounts {
    let mut positions: HashSet<(u32, u32)> = HashSet::new();
    let mut pfu_tiles = 0usize;
    let mut pio_entries = 0usize;
    let mut iologic_entries = 0usize;
    let mut pll_entries = 0usize;
    let mut ebr_bels: HashSet<(String, i32, i32)> = HashSet::new();
    let mut dsp_bels: HashSet<(String, i32, i32)> = HashSet::new();
    let mut pll_bels: HashSet<(i32, i32)> = HashSet::new();
    let mut dcc_bels: HashSet<(String, i32, i32)> = HashSet::new();

    for t in tiles {
        positions.insert((t.row, t.col));
        if t.tile_type == "PLC2" {
            pfu_tiles += 1;
        }
        for s in &t.sites {
            match s.class {
                SiteClass::Pio => pio_entries += 1,
                SiteClass::IoLogic => iologic_entries += 1,
                SiteClass::Pll => {
                    pll_entries += 1;
                    pll_bels.insert((s.bel_col, s.bel_row));
                }
                SiteClass::Ebr => {
                    ebr_bels.insert((s.name.clone(), s.bel_col, s.bel_row));
                }
                SiteClass::Dsp => {
                    dsp_bels.insert((s.name.clone(), s.bel_col, s.bel_row));
                }
                SiteClass::Dcc => {
                    dcc_bels.insert((s.name.clone(), s.bel_col, s.bel_row));
                }
                _ => {}
            }
        }
    }

    let slices = pfu_tiles * 4;
    ResourceCounts {
        tile_entries: tiles.len(),
        grid_positions: positions.len(),
        pfu_tiles,
        slices,
        lut4: slices * 2,
        ff: slices * 2,
        pio_entries,
        iologic_entries,
        ebr_bels: ebr_bels.len(),
        dsp_bels: dsp_bels.len(),
        pll_entries,
        pll_bels: pll_bels.len(),
        dcc_bels: dcc_bels.len(),
        bonded_pios: iod.pio_metadata.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_pos_parses() {
        assert_eq!(parse_key_pos("R11C100:PLC2"), Ok((11, 100)));
        assert_eq!(parse_key_pos("MIB_R10C105:MIB_DSP0"), Ok((10, 105)));
        assert_eq!(parse_key_pos("TAP_R94C114:TAP_DRIVE_CIB"), Ok((94, 114)));
        assert!(parse_key_pos("bogus").is_err());
    }

    #[test]
    fn site_classification() {
        assert_eq!(classify_site("SLICE_A"), SiteClass::Slice);
        assert_eq!(classify_site("SLICE_D"), SiteClass::Slice);
        assert_eq!(classify_site("PIOC"), SiteClass::Pio);
        assert_eq!(classify_site("IOLOGICB"), SiteClass::IoLogic);
        assert_eq!(classify_site("EBR2"), SiteClass::Ebr);
        assert_eq!(classify_site("MULT18_0"), SiteClass::Dsp);
        assert_eq!(classify_site("MULT9_1"), SiteClass::Dsp);
        assert_eq!(classify_site("PRADD18_0"), SiteClass::Dsp);
        assert_eq!(classify_site("ALU54_3"), SiteClass::Dsp);
        assert_eq!(classify_site("ALU24_2"), SiteClass::Dsp);
        assert_eq!(classify_site("PLL"), SiteClass::Pll);
        assert_eq!(classify_site("DCC7"), SiteClass::Dcc);
        assert_eq!(classify_site("DQS"), SiteClass::Dqs);
        assert_eq!(classify_site("DDRDLL"), SiteClass::DdrDll);
        assert_eq!(classify_site("GSR"), SiteClass::Control);
        assert_eq!(classify_site("CIBTEST"), SiteClass::Test);
    }

    #[test]
    fn device_opens_and_counts_match_trellis_json() {
        let dev = Device::open().expect("Device::open");
        let c = dev.counts();
        // All numbers below were measured directly from the vendored
        // Trellis JSON (tilegrid.json + iodb.json), not invented.
        assert_eq!(c.tile_entries, 14231);
        // The tilegrid covers the full 96x127 grid: every (row, col) has at
        // least one tile entry.
        assert_eq!(c.grid_positions, 12192);
        assert_eq!(c.grid_positions, (GRID_ROWS * GRID_COLS) as usize);
        assert_eq!(c.pfu_tiles, 10455);
        assert_eq!(c.slices, 41820);
        assert_eq!(c.lut4, 83640);
        assert_eq!(c.ff, 83640);
        assert_eq!(c.pio_entries, 1217);
        assert_eq!(c.iologic_entries, 1217);
        assert_eq!(c.ebr_bels, 212);
        assert_eq!(c.dsp_bels, 780);
        assert_eq!(c.pll_entries, 8);
        assert_eq!(c.pll_bels, 6);
        assert_eq!(c.dcc_bels, 60);
        assert_eq!(c.bonded_pios, 365);
        // Every PLC2 tile really does host exactly SLICE_A..D.
        for &i in dev.tiles_of_type("PLC2") {
            let names: BTreeSet<&str> =
                dev.tiles[i].sites.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(
                names,
                BTreeSet::from(["SLICE_A", "SLICE_B", "SLICE_C", "SLICE_D"])
            );
        }
    }

    #[test]
    fn grid_extents() {
        let dev = Device::global().unwrap();
        let (mut max_r, mut max_c) = (0u32, 0u32);
        for t in dev.tiles() {
            max_r = max_r.max(t.row);
            max_c = max_c.max(t.col);
        }
        assert_eq!((max_r, max_c), (GRID_ROWS - 1, GRID_COLS - 1));
    }

    #[test]
    fn package_pins_present() {
        let dev = Device::global().unwrap();
        let cabga381 = dev.package_pins("CABGA381").expect("CABGA381 in iodb.json");
        assert!(!cabga381.is_empty());
        assert!(dev.package_pins("NOPE").is_none());
        assert_eq!(dev.pio_metadata().len(), 365);
    }

    #[test]
    fn globals_parsed() {
        let dev = Device::global().unwrap();
        assert_eq!(dev.globals().quadrants.len(), 4);
        assert_eq!(dev.globals().taps.len(), 6);
        // 12 spines: 6 tap columns (C13..C114) x upper/lower.
        assert_eq!(dev.globals().spines.len(), 12);
    }

    #[test]
    fn routing_data_available_for_all_tile_types() {
        let dev = Device::global().unwrap();
        // Tile types whose bits.db is legitimately empty in the Trellis DB
        // (dummy placeholders and special tiles like OSC/POR).
        let expected_empty: BTreeSet<&str> = BTreeSet::from([
            "DUMMY_TILE_0",
            "DUMMY_TILE_1",
            "DUMMY_TILE_2",
            "DUMMY_TILE_4",
            "DUMMY_TILE_5",
            "DUMMY_TILE_6",
            "DUMMY_TILE_7",
            "DUMMY_TILE_8",
            "DUMMY_TILE_A",
            "DUMMY_TILE_E",
            "DUMMY_TILE_F",
            "DUMMY_TILE_S",
            "DUMMY_TILE_T",
            "MIB_CIB_LRC",
            "MIB_CIB_LRC_A",
            "MIB_CIB_LX",
            "MIB_CIB_RX",
            "OSC",
            "POR",
            "PVT_COUNT2",
        ]);
        let mut actual_empty = BTreeSet::new();
        for tt in crate::tiledata::TILE_TYPES {
            let tb = dev.tile_bits(tt).expect("tile_bits");
            if tb.muxes.is_empty() && tb.config_enums.is_empty() && tb.fixed_conns.is_empty() {
                actual_empty.insert(*tt);
            }
        }
        assert_eq!(actual_empty, expected_empty);
        assert!(dev.tile_bits("NOT_A_TILE").is_err());
    }

    #[test]
    fn wire_names_cover_plc2() {
        let dev = Device::global().unwrap();
        let names = dev.wire_names("PLC2").expect("wire_names");
        // Slice inputs, span wires, and clock/reset nets are all present.
        assert!(names.contains("A0"));
        assert!(names.contains("D7"));
        assert!(names.iter().any(|n| n.starts_with("E1_H")));
        assert!(names.iter().any(|n| n.starts_with("V0")));
        assert!(names.len() > 500);
    }
}
