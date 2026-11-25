//! BEL-pin to INT-wire map loaded from the public prjuray-db tileconn DB.
//!
//! US-01: the router's `wire_for_bel_pin` returned `None` for every pin
//! because `helion-ultrascale` never loaded `tileconn.json`, unlike
//! `helion-xilinx7`, which does. The map already exists in the public
//! DBs; this module loads it at runtime, following the `helion-xilinx7`
//! precedent (`crates/helion-xilinx7/src/db/load.rs`, `load_tileconn`).
//!
//! How a BEL pin resolves (verified against the xczu3eg DBs, see the
//! US-01 characterization report):
//! - The pin's site wire comes from the tile-type file's `site_pins`
//!   (e.g. SLICEL pin `A1` in tile type `CLEL_R` is wire
//!   `CLE_CLE_L_SITE_0_A1`). A trailing `_PIN` suffix names the
//!   tile-internal side of a site PIP, so it is stripped to get the
//!   external wire (e.g. `CIN_PIN` to `CIN`, `DSP_DSP48_0_ACIN_B0_PIN`
//!   to `DSP_DSP48_0_ACIN_B0`).
//! - `tileconn.json` stitches that site wire to exactly one INT wire
//!   (e.g. `CLEL_R` `A1` to `IMUX_E10` at grid delta `(-1, 0)`).
//! - `tilegrid.json` gives the site's tile (name, type, grid coords),
//!   so the INT tile name follows from the grid delta (e.g. site
//!   `SLICE_X0Y0` in `CLEL_R_X0Y0` partners `INT_X0Y0`).
//!
//! Data-file decision (US-01 wiredb lane): the DBs are loaded at
//! runtime from a prjuray-db family root (the same `--db-root` the
//! `helion-bitgen` decoder takes), NOT vendored into the repo.
//! `tileconn.json` alone is 81 MB, `tilegrid.json` 15 MB, and
//! `tile_types` 18 MB; vendoring that per part would bloat the repo
//! for data that already lives on disk in the public CC0 checkout.
//! A compact derived table (only the edges the flow needs) is future
//! work once this loader's coverage is critic-approved; it would be a
//! second source of truth and must be generated from, and verified
//! against, these same DBs.
//!
//! What is mapped: every site pin with exactly one INT partner. CLE
//! pins resolve 104/106 (SLICEL) and 106/108 (SLICEM); the two
//! unmapped pins per type are `CIN`/`COUT`, which stitch to the
//! neighboring slice, never to INT fabric (carry chain, not routing).
//! A further class of pins (IO, clock, and config pins) stitches to
//! the same INT wire in more than one neighboring tile, so no single
//! edge exists for them; the loader records them as ambiguous
//! (`PinCoverage::ambiguous`) instead of failing the whole load, and
//! the router never asks for them. No CLE, DSP, or BRAM pin is
//! ambiguous.
//! DSP and BRAM edges are loaded, counted, and registered into the
//! routing graph. DSP/BRAM pins stitch to INT_INTF tiles (the
//! interface columns flanking DSP/BRAM columns), not to plain INT
//! tiles, so the graph instantiates those tiles from their
//! `tile_type_INT_INTF_*.json` files (runtime-loaded from the DB
//! root, like the tile-type files above) alongside the plain INT
//! tiles, and joins each INT_INTF tile to its INT neighbor with
//! tileconn stitch edges (same node, no PIP). A DSP pin name is
//! ambiguous across the tile's two sites, so registration carries
//! the site index (see [`SiteTile::site_index`]): each site's pins
//! register under that site's own template, never under the other
//! site's wires. The flow still places no DSP/BRAM sites, so the
//! CLI flow stays CLE-only; the model is exercised through
//! [`TileconnWireDb::from_db_and_sites`] with explicit sites.
//!
//! Input pins need no INT_INTF PIP: their INT_INTF wire stitches
//! straight to an INT wire (586 of the 850 counted pins). Output
//! pins drive through the tile's LOGIC_OUTS pass-through PIP (264 of
//! 850); that PIP has no public segbits (no `segbits_int_intf_l.db`
//! or `segbits_int_intf_r.db` exists), so bitgen of an output-pin
//! route fails loud. The graph models the PIP anyway: routing and
//! translation are proven, emission is blocked on data, not on the
//! model.
//!
//! BEL-pin naming (the flow's names to site pins, for SLICEL/SLICEM):
//! - `({L}6LUT, O6)` to `{L}_O` (LUT O6 output), `({L}6LUT, O5)` to
//!   `{L}MUX` (LUT O5 output), `({L}6LUT, I0..I5)` to `{L}1..{L}6`.
//! - `({L}Q, Q)` to `{L}Q`, `({L}Q2, Q)` to `{L}Q2` (FF Q outputs).
//! - `({L}Q, D)` and `({L}Q2, D)` to `{L}X`: the FF D input arrives
//!   through the bypass `X` pin (FFMUX BYP). Both FFs of a letter
//!   share the one `X` pin.
//! - Every site pin is also registered under its own name
//!   (`(P, P)`), so direct site-pin queries resolve too.
//! - `CARRY8` pins are never registered: `CIN`/`COUT` have no INT
//!   partner, and carry routing is not implemented in the flow.

use crate::fabric::{UltraSite, WireDb, WireId, WireInfo};
use crate::int_pip::BidirDir;
use crate::place::UltraPlaced;
use crate::route_to_pip::{RouteToPipError, TranslatedRoute};
use crate::wire_db::{IntTile, IntTileWireDb, WireDbError};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Error loading the tileconn DB or building the wire database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TileconnError {
    /// File IO failure (path included in the message).
    Io(String),
    /// JSON parse failure (path included in the message).
    Json(String),
    /// No part directory for this part under the DB root.
    NoPartDir { part: String, root: PathBuf },
    /// More than one part directory matches the part prefix; the
    /// caller must name one with `HELION_TILECONN_PART`.
    AmbiguousPart { part: String, dirs: Vec<String> },
    /// A placed site is not in the part's tilegrid.
    UnknownSite(String),
    /// A tile-type file the tilegrid needs is missing.
    MissingTileType { tile_type: String, path: PathBuf },
    /// The INT tile at the stitched grid offset does not exist.
    UnknownIntTile { site: String, grid: (i32, i32) },
    /// The stitched partner tile is neither a plain INT tile nor an
    /// INT_INTF tile.
    WrongIntTileType { tile: String, tile_type: String },
    /// An INT tile name does not parse as `INT_X<num>Y<num>`.
    BadIntTileName(String),
    /// A site name does not end in `X<num>Y<num>`, so its rank
    /// among same-type sites cannot be computed.
    BadSiteName(String),
    /// A site's rank among same-type sites exceeds the tile-type
    /// file's same-type entries (tilegrid / tile_type mismatch).
    SiteIndexOutOfRange {
        site: String,
        tile_type: String,
        index: usize,
        count: usize,
    },
    /// No sites were supplied to build the graph from.
    NoSites,
    /// The underlying INT-tile graph build failed.
    WireDb(WireDbError),
}

impl fmt::Display for TileconnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "tileconn IO error: {}", e),
            Self::Json(e) => write!(f, "tileconn JSON error: {}", e),
            Self::NoPartDir { part, root } => write!(
                f,
                "no part directory for '{}' under {}",
                part,
                root.display()
            ),
            Self::AmbiguousPart { part, dirs } => write!(
                f,
                "more than one part directory matches '{}': {}; set HELION_TILECONN_PART to one of them",
                part,
                dirs.join(", ")
            ),
            Self::UnknownSite(s) => {
                write!(f, "site '{}' is not in the part tilegrid", s)
            }
            Self::MissingTileType { tile_type, path } => write!(
                f,
                "tile_type file for '{}' missing at {}",
                tile_type,
                path.display()
            ),
            Self::UnknownIntTile { site, grid } => write!(
                f,
                "no tile at grid ({},{}) stitched to site '{}'",
                grid.0, grid.1, site
            ),
            Self::WrongIntTileType { tile, tile_type } => write!(
                f,
                "stitched partner tile '{}' has type '{}', expected 'INT' or 'INT_INTF*'",
                tile, tile_type
            ),
            Self::BadIntTileName(n) => {
                write!(f, "INT tile name '{}' does not parse as INT_X<num>Y<num>", n)
            }
            Self::BadSiteName(n) => {
                write!(f, "site name '{}' does not end in X<num>Y<num>", n)
            }
            Self::SiteIndexOutOfRange {
                site,
                tile_type,
                index,
                count,
            } => write!(
                f,
                "site '{}' is rank {} among same-type sites but tile_type '{}' has only {} same-type entries",
                site, index, tile_type, count
            ),
            Self::NoSites => write!(f, "no sites supplied to build the tileconn wire DB"),
            Self::WireDb(e) => write!(f, "INT tile graph error: {}", e),
        }
    }
}

impl std::error::Error for TileconnError {}

impl From<WireDbError> for TileconnError {
    fn from(e: WireDbError) -> Self {
        Self::WireDb(e)
    }
}

fn read_text(path: &Path) -> Result<String, TileconnError> {
    fs::read_to_string(path)
        .map_err(|e| TileconnError::Io(format!("cannot read {}: {e}", path.display())))
}

fn parse_json<T: for<'de> Deserialize<'de>>(text: &str, what: &str) -> Result<T, TileconnError> {
    serde_json::from_str(text).map_err(|e| TileconnError::Json(format!("cannot parse {what}: {e}")))
}

/// One wire-stitching entry of `tileconn.json` (same shape as the
/// 7-series loader in `helion-xilinx7`).
#[derive(Clone, Debug, Deserialize)]
pub struct TileConnEntry {
    /// `[dx, dy]`: neighbor offset relative to the `tile_types[0]` tile.
    pub grid_deltas: [i32; 2],
    /// `[type_a, type_b]`: `wire_pairs[i][0]` lives in a `type_a` tile,
    /// `wire_pairs[i][1]` in the neighboring `type_b` tile.
    pub tile_types: [String; 2],
    /// `[wire_in_a, wire_in_b]` pairs that are the same electrical node.
    pub wire_pairs: Vec<[String; 2]>,
}

/// One tile entry of `tilegrid.json` (sites only; bits and pin
/// functions are not needed for the pin map).
#[derive(Clone, Debug, Deserialize)]
pub struct TileGridEntry {
    pub grid_x: i32,
    pub grid_y: i32,
    /// Tile type name, e.g. `CLEL_R` (JSON field `"type"`).
    #[serde(rename = "type")]
    pub tile_type: String,
    /// Site instance name -> site type, e.g. `SLICE_X0Y0 -> SLICEL`.
    #[serde(default)]
    pub sites: HashMap<String, String>,
}

/// One site pin of a `tile_type_*.json` file.
#[derive(Clone, Debug, Deserialize)]
pub struct TileTypeSitePin {
    /// Tile wire this site pin connects to.
    pub wire: String,
}

/// One site template of a `tile_type_*.json` file.
#[derive(Clone, Debug, Deserialize)]
pub struct TileTypeSite {
    pub name: String,
    /// Site type, e.g. `SLICEL` (JSON field `"type"`).
    #[serde(rename = "type")]
    pub site_type: String,
    /// Some pins are null in the JSON (unconnected); skipped during load.
    #[serde(default)]
    pub site_pins: HashMap<String, Option<TileTypeSitePin>>,
}

/// Raw `tile_type_*.json` contents (sites only; pips and wires are
/// not needed for the pin map).
#[derive(Clone, Debug, Deserialize)]
pub struct TileTypeFile {
    pub tile_type: String,
    #[serde(default)]
    pub sites: Vec<TileTypeSite>,
}

/// Strip the tile-internal `_PIN` suffix from a site wire to get the
/// external wire tileconn stitches (see the module docs).
fn external_wire(wire: &str) -> &str {
    wire.strip_suffix("_PIN").unwrap_or(wire)
}

/// Parse the trailing `X<num>Y<num>` of a site name (e.g.
/// `IOB_X0Y0` -> (0, 0), `SLICE_X10Y10` -> (10, 10)) into tile-local
/// or absolute coordinates. Returns `None` when the name has no
/// such tail. Every site name in the tilegrid ends in `X<num>Y<num>`.
fn parse_site_coords(name: &str) -> Option<(i32, i32)> {
    let (_, rest) = name.rsplit_once("_X")?;
    let (xs, ys) = rest.split_once('Y')?;
    let x: i32 = xs.parse().ok()?;
    let y: i32 = ys.parse().ok()?;
    Some((x, y))
}

/// The rank-th same-type site template of a tile-type file, or
/// `None` when the file has fewer same-type entries. The tile_type
/// sites list orders same-type sites by (X, Y), so the rank (see
/// [`SiteTile::site_index`]) selects the one template that belongs
/// to the site.
fn same_type_site<'a>(
    file: &'a TileTypeFile,
    site_type: &str,
    index: usize,
) -> Option<&'a TileTypeSite> {
    file.sites
        .iter()
        .filter(|s| s.site_type == site_type)
        .nth(index)
}

/// One resolved site-pin to INT/INT_INTF-wire edge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinWire {
    /// External site wire, e.g. `CLE_CLE_L_SITE_0_A1`.
    pub site_wire: String,
    /// Partner wire name in the partner tile, e.g. `IMUX_E10`.
    pub int_wire: String,
    /// Partner tile type: `INT` for CLE pins, `INT_INTF_L` /
    /// `INT_INTF_R` (or a PCIE4 / TERM variant) for DSP/BRAM pins.
    pub partner_type: String,
    /// Grid delta from the site tile to the partner tile, e.g. `(-1, 0)`.
    pub delta: (i32, i32),
}

/// Coverage of one site type: pins, pins with any tileconn partner,
/// pins with exactly one plain-INT partner (a unique edge), pins with
/// at least one INT_INTF partner (interface tile next to a DSP/BRAM
/// column; needs an INT_INTF tile model to route, counted not edged),
/// and pins whose plain-INT fan-out is ambiguous (the same pin
/// stitches to the same INT wire in more than one neighboring tile,
/// so no single edge exists).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PinCoverage {
    pub pins: usize,
    pub any_partner: usize,
    pub int_partner: usize,
    pub intf_partner: usize,
    pub ambiguous: usize,
}

/// The tile holding one site, from `tilegrid.json`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteTile {
    /// Tile name, e.g. `CLEL_R_X0Y0`.
    pub tile: String,
    /// Tile type, e.g. `CLEL_R`.
    pub tile_type: String,
    /// Site type, e.g. `SLICEL`.
    pub site_type: String,
    /// Rank among same-type sites in the tile (0-based), by (X, Y)
    /// parsed from the site name. A tile holds several sites of one
    /// type (6 HDIOB_M on an HDIO tile); the tile_type sites list
    /// holds one template per site, in the same (X, Y) order, so the
    /// rank selects the template that belongs to this site. Without
    /// it, pin resolution conflates the sites: every same-type
    /// site's pins register under one site and the last one's wires
    /// win. Computed at load time; `TileGridEntry.sites` is a
    /// HashMap, so its order is lost and the rank must be derived
    /// from the names.
    pub site_index: usize,
    pub grid_x: i32,
    pub grid_y: i32,
}

/// The loaded tileconn DB: the pin map plus the tilegrid site lookup.
///
/// Built by [`TileconnDb::load`] from a prjuray-db family root
/// (e.g. `~/workspace/prjuray-db-ref/zynqusp`) and a part directory
/// name (e.g. `xczu3eg-sbva484-1-e`). The 84 MB `tileconn.json` is
/// parsed once and dropped; only the resolved edges (hundreds) and
/// the tilegrid site lookup are kept.
#[derive(Debug)]
pub struct TileconnDb {
    /// Resolved part directory, e.g. `<root>/xczu3eg-sbva484-1-e`.
    pub part_dir: PathBuf,
    /// Part directory name as loaded.
    pub part: String,
    /// `(tile type, external site wire)` to its INT edge. Keyed by
    /// site wire, not pin name: a pin name is ambiguous across a
    /// tile's sites (DSP has two `DSP48E2` sites per tile), while a
    /// site wire is unique.
    pub edges: HashMap<(String, String), PinWire>,
    /// `(tile type, external site wire)` to its INT_INTF edge, for
    /// pins whose unique partner is an INT_INTF tile (all DSP/BRAM
    /// fabric pins: 472 DSP, 131 RAMB181, 81 RAMBFIFO36, 166
    /// RAMBFIFO18). Same keying as `edges`; the two maps are
    /// disjoint by construction (no pin has both a plain-INT and an
    /// INT_INTF partner in this DB).
    pub intf_edges: HashMap<(String, String), PinWire>,
    /// Tileconn stitch pairs between INT_INTF and INT tiles, by
    /// INT_INTF tile type: `(INT_INTF wire, INT wire, grid delta from
    /// the INT_INTF tile to the INT tile)`. 1,878 distinct pairs for
    /// xczu3eg. The graph adds one bidirectional same-node edge per
    /// pair for each instantiated INT_INTF tile. Retained from the
    /// load (the full partner index is dropped); the pairs are the
    /// only fabric-to-fabric stitches the graph needs.
    pub intf_stitches: HashMap<String, Vec<(String, String, (i32, i32))>>,
    /// Tile-type files as loaded, by tile type.
    pub tile_types: HashMap<String, TileTypeFile>,
    /// Site name to its tile.
    pub site_tiles: HashMap<String, SiteTile>,
    /// `(grid_x, grid_y)` to `(tile name, tile type)`.
    pub tile_by_grid: HashMap<(i32, i32), (String, String)>,
    /// Coverage per `(tile type, site type)`, computed at load.
    pub coverage: HashMap<(String, String), PinCoverage>,
    /// `tileconn.json` entry count (81,640 for xczu3eg-sbva484-1-e).
    pub tileconn_entries: usize,
    /// `tileconn.json` wire-pair count (658,732 for xczu3eg).
    pub tileconn_pairs: usize,
    /// Tile types whose `tile_type_*.json` file was loaded.
    pub loaded_tile_types: Vec<String>,
    /// Tile types with sites in the tilegrid but no tile-type file.
    pub missing_tile_types: Vec<String>,
}

impl TileconnDb {
    /// Resolve the part directory under a family root. An exact
    /// directory match wins; otherwise exactly one directory whose
    /// name starts with `{part}-` wins; zero or more than one fails
    /// loud (the ambiguous case names the candidates so the caller
    /// can pick one with `HELION_TILECONN_PART`).
    pub fn resolve_part_dir(db_root: &Path, part: &str) -> Result<PathBuf, TileconnError> {
        let exact = db_root.join(part);
        if exact.is_dir() {
            return Ok(exact);
        }
        let mut matches: Vec<String> = Vec::new();
        let entries = fs::read_dir(db_root).map_err(|e| {
            TileconnError::Io(format!("cannot read db root {}: {e}", db_root.display()))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                TileconnError::Io(format!("cannot read db root {}: {e}", db_root.display()))
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && name.starts_with(&format!("{part}-")) {
                matches.push(name);
            }
        }
        matches.sort();
        match matches.len() {
            0 => Err(TileconnError::NoPartDir {
                part: part.to_string(),
                root: db_root.to_path_buf(),
            }),
            1 => Ok(db_root.join(&matches[0])),
            _ => Err(TileconnError::AmbiguousPart {
                part: part.to_string(),
                dirs: matches,
            }),
        }
    }

    /// Load the tileconn DB for a part from a family root.
    pub fn load(db_root: &Path, part: &str) -> Result<Self, TileconnError> {
        let part_dir = Self::resolve_part_dir(db_root, part)?;
        let tc_path = part_dir.join("tileconn.json");
        let tg_path = part_dir.join("tilegrid.json");
        let entries: Vec<TileConnEntry> = parse_json(
            &read_text(&tc_path)?,
            &format!("tileconn {}", tc_path.display()),
        )?;
        let grid: HashMap<String, TileGridEntry> = parse_json(
            &read_text(&tg_path)?,
            &format!("tilegrid {}", tg_path.display()),
        )?;
        let tileconn_entries = entries.len();
        let tileconn_pairs: usize = entries.iter().map(|e| e.wire_pairs.len()).sum();

        // Partner index: (tile type, wire) to (partner type, partner
        // wire, delta from this tile). Built once, used for the edge
        // table and coverage, then dropped.
        let mut partners: HashMap<(String, String), Vec<(String, String, (i32, i32))>> =
            HashMap::new();
        // INT_INTF to INT stitch pairs, by INT_INTF tile type: kept,
        // unlike the partner index (the graph adds one edge per pair
        // per instantiated INT_INTF tile). Each pair is stored once,
        // normalized to (INT_INTF wire, INT wire, delta from the
        // INT_INTF tile to the INT tile), whichever orientation the
        // entry lists.
        let mut intf_stitches: HashMap<String, Vec<(String, String, (i32, i32))>> = HashMap::new();
        for e in &entries {
            let (a, b) = (e.tile_types[0].clone(), e.tile_types[1].clone());
            let (dx, dy) = (e.grid_deltas[0], e.grid_deltas[1]);
            for [wa, wb] in &e.wire_pairs {
                partners.entry((a.clone(), wa.clone())).or_default().push((
                    b.clone(),
                    wb.clone(),
                    (dx, dy),
                ));
                partners.entry((b.clone(), wb.clone())).or_default().push((
                    a.clone(),
                    wa.clone(),
                    (-dx, -dy),
                ));
                if a.starts_with("INT_INTF") && b == "INT" {
                    intf_stitches.entry(a.clone()).or_default().push((
                        wa.clone(),
                        wb.clone(),
                        (dx, dy),
                    ));
                } else if b.starts_with("INT_INTF") && a == "INT" {
                    intf_stitches.entry(b.clone()).or_default().push((
                        wb.clone(),
                        wa.clone(),
                        (-dx, -dy),
                    ));
                }
            }
        }
        // Stitch pairs are unique per (type, intf wire, INT wire,
        // delta): tileconn lists each pair once, and the two
        // orientations above never both fire for one entry.
        for pairs in intf_stitches.values_mut() {
            pairs.sort();
            pairs.dedup();
        }

        // Site lookup from the tilegrid.
        let mut site_tiles: HashMap<String, SiteTile> = HashMap::new();
        let mut tile_by_grid: HashMap<(i32, i32), (String, String)> = HashMap::new();
        let mut sited_types: Vec<String> = Vec::new();
        for (name, entry) in &grid {
            tile_by_grid.insert(
                (entry.grid_x, entry.grid_y),
                (name.clone(), entry.tile_type.clone()),
            );
            if !entry.sites.is_empty() && !sited_types.contains(&entry.tile_type) {
                sited_types.push(entry.tile_type.clone());
            }
            for (site_name, site_type) in &entry.sites {
                // Rank among same-type sites in this tile, by (X, Y)
                // parsed from the site name (see SiteTile::site_index).
                // The sites map is unordered, so the rank is counted
                // explicitly; a site name without coordinates fails
                // loud instead of mis-ranking.
                let (sx, sy) = parse_site_coords(site_name).ok_or_else(|| {
                    TileconnError::BadSiteName(site_name.clone())
                })?;
                let mut site_index = 0usize;
                for (other_name, other_type) in &entry.sites {
                    if other_type != site_type {
                        continue;
                    }
                    let (ox, oy) =
                        parse_site_coords(other_name).ok_or_else(|| {
                            TileconnError::BadSiteName(other_name.clone())
                        })?;
                    if (ox, oy) < (sx, sy) {
                        site_index += 1;
                    }
                }
                site_tiles.insert(
                    site_name.clone(),
                    SiteTile {
                        tile: name.clone(),
                        tile_type: entry.tile_type.clone(),
                        site_type: site_type.clone(),
                        site_index,
                        grid_x: entry.grid_x,
                        grid_y: entry.grid_y,
                    },
                );
            }
        }
        sited_types.sort();

        // Tile-type files: one per tile type that has sites. A
        // missing file is recorded, not fatal: the loader still maps
        // every tile type it can read.
        let mut tile_types: HashMap<String, TileTypeFile> = HashMap::new();
        let mut loaded_tile_types: Vec<String> = Vec::new();
        let mut missing_tile_types: Vec<String> = Vec::new();
        for t in &sited_types {
            let path = db_root.join("tile_types").join(format!("tile_type_{t}.json"));
            if !path.is_file() {
                missing_tile_types.push(t.clone());
                continue;
            }
            let file: TileTypeFile = parse_json(
                &read_text(&path)?,
                &format!("tile_type {}", path.display()),
            )?;
            loaded_tile_types.push(t.clone());
            tile_types.insert(t.clone(), file);
        }

        // Edge tables and coverage.
        let mut edges: HashMap<(String, String), PinWire> = HashMap::new();
        let mut intf_edges: HashMap<(String, String), PinWire> = HashMap::new();
        let mut coverage: HashMap<(String, String), PinCoverage> = HashMap::new();
        for (tile_type, file) in &tile_types {
            for site in &file.sites {
                let key = (tile_type.clone(), site.site_type.clone());
                let cov = coverage.entry(key).or_default();
                for (_pin, pin_info) in &site.site_pins {
                    let Some(info) = pin_info else {
                        continue;
                    };
                    cov.pins += 1;
                    let ext = external_wire(&info.wire).to_string();
                    let Some(ps) = partners.get(&(tile_type.clone(), ext.clone())) else {
                        continue;
                    };
                    cov.any_partner += 1;
                    let mut int_partners: Vec<(String, (i32, i32))> = ps
                        .iter()
                        .filter(|(pt, _, _)| pt == "INT")
                        .map(|(_, w, d)| (w.clone(), *d))
                        .collect();
                    int_partners.sort();
                    int_partners.dedup();
                    // DSP/BRAM columns are flanked by INT_INTF tiles,
                    // not plain INT tiles: their pins stitch to the
                    // interface tiles. A pin with exactly one
                    // INT_INTF partner gets an INT_INTF edge, the
                    // same as a plain-INT edge, and the graph
                    // registers it into that tile.
                    let mut intf_partners: Vec<(String, String, (i32, i32))> = ps
                        .iter()
                        .filter(|(pt, _, _)| pt.starts_with("INT_INTF"))
                        .map(|(pt, w, d)| (pt.clone(), w.clone(), *d))
                        .collect();
                    intf_partners.sort();
                    intf_partners.dedup();
                    if !intf_partners.is_empty() {
                        cov.intf_partner += 1;
                    }
                    if int_partners.len() == 1 {
                        // Unchanged from the INT-only loader: one
                        // plain-INT partner is the edge, whatever
                        // else the pin touches (no pin in this DB has
                        // both a plain-INT and an INT_INTF partner).
                        cov.int_partner += 1;
                        let (int_wire, delta) = int_partners.into_iter().next().unwrap();
                        edges.insert(
                            (tile_type.clone(), ext.clone()),
                            PinWire {
                                site_wire: ext,
                                int_wire,
                                partner_type: "INT".to_string(),
                                delta,
                            },
                        );
                        continue;
                    }
                    if int_partners.is_empty() && intf_partners.len() == 1 {
                        let (partner_type, int_wire, delta) =
                            intf_partners.into_iter().next().unwrap();
                        intf_edges.insert(
                            (tile_type.clone(), ext.clone()),
                            PinWire {
                                site_wire: ext,
                                int_wire,
                                partner_type,
                                delta,
                            },
                        );
                        continue;
                    }
                    if int_partners.is_empty() && intf_partners.is_empty() {
                        continue;
                    }
                    // Ambiguous fan-out: the pin stitches to the
                    // same wire in more than one neighboring tile
                    // (IO, clock, and config pins do this; no CLE,
                    // DSP, or BRAM pin does). No single edge exists,
                    // so no edge is recorded and the load does not
                    // fail: the router never asks for these pins.
                    // Counted separately.
                    cov.ambiguous += 1;
                }
            }
        }

        Ok(Self {
            part_dir,
            part: part.to_string(),
            edges,
            intf_edges,
            intf_stitches,
            tile_types,
            site_tiles,
            tile_by_grid,
            coverage,
            tileconn_entries,
            tileconn_pairs,
            loaded_tile_types,
            missing_tile_types,
        })
    }

    /// The tile holding a site, or `None` when the site is not in
    /// the tilegrid.
    pub fn site_tile(&self, site: &str) -> Option<&SiteTile> {
        self.site_tiles.get(site)
    }

    /// The INT edge for one (tile type, external site wire).
    pub fn int_wire_for_site_wire(
        &self,
        tile_type: &str,
        site_wire: &str,
    ) -> Option<&PinWire> {
        self.edges
            .get(&(tile_type.to_string(), site_wire.to_string()))
    }

    /// The INT edge for one (tile type, site pin), but only when the
    /// pin is unambiguous across the tile type's sites. CLE pins
    /// are unambiguous (one site per tile); DSP pins are not (two
    /// sites share the pin names), so this returns `None` for them
    /// and the caller must use the site wire instead.
    pub fn pin_wire_for_pin(&self, tile_type: &str, pin: &str) -> Option<&PinWire> {
        let file = self.tile_types.get(tile_type)?;
        let mut found: Option<&PinWire> = None;
        for site in &file.sites {
            let Some(Some(info)) = site.site_pins.get(pin) else {
                continue;
            };
            let ext = external_wire(&info.wire);
            let Some(edge) = self.edges.get(&(tile_type.to_string(), ext.to_string())) else {
                continue;
            };
            if found.is_some() {
                return None;
            }
            found = Some(edge);
        }
        found
    }

    /// `(partner tile name, partner wire name)` for one site pin, or
    /// `None` when the pin has no fabric partner (carry pins,
    /// unmapped pins). The partner tile is a plain INT tile for CLE
    /// pins and an INT_INTF tile for DSP/BRAM pins. The pin is read
    /// from the site's own template only (the rank-th same-type entry
    /// in the tile-type file): a tile with several sites of one type
    /// used to loop over all of them and return `None` on the
    /// ambiguity, so a pin present on more than one same-type site
    /// never resolved.
    pub fn int_tile_for_site(&self, site: &str, pin: &str) -> Option<(String, String)> {
        let st = self.site_tiles.get(site)?;
        let file = self.tile_types.get(&st.tile_type)?;
        let s = same_type_site(file, &st.site_type, st.site_index)?;
        let Some(Some(info)) = s.site_pins.get(pin) else {
            return None;
        };
        let ext = external_wire(&info.wire);
        let edge = partner_edge(self, &st.tile_type, ext)?;
        let (gx, gy) = (st.grid_x + edge.delta.0, st.grid_y + edge.delta.1);
        let (tile, _) = self.tile_by_grid.get(&(gx, gy))?;
        Some((tile.clone(), edge.int_wire.clone()))
    }

    /// The `tile_type_INT_INTF_*.json` text for one INT_INTF tile
    /// type, read from the DB root's `tile_types` directory (the
    /// runtime-loading pattern of [`TileconnDb::load`]: the DB files
    /// are never vendored). Fails loud when the file is missing: a
    /// graph that needs the tile cannot be built without its PIPs.
    pub fn intf_tile_json(&self, tile_type: &str) -> Result<String, TileconnError> {
        let Some(root) = self.part_dir.parent() else {
            return Err(TileconnError::Io(format!(
                "part dir {} has no parent",
                self.part_dir.display()
            )));
        };
        let path = root
            .join("tile_types")
            .join(format!("tile_type_{tile_type}.json"));
        if !path.is_file() {
            return Err(TileconnError::MissingTileType {
                tile_type: tile_type.to_string(),
                path,
            });
        }
        read_text(&path)
    }

    /// Coverage for one (tile type, site type).
    pub fn coverage(&self, tile_type: &str, site_type: &str) -> PinCoverage {
        self.coverage
            .get(&(tile_type.to_string(), site_type.to_string()))
            .copied()
            .unwrap_or_default()
    }

    /// Ambiguous pins across all loaded site types (pins with no
    /// single INT edge). Zero for every tile type the router uses.
    pub fn total_ambiguous(&self) -> usize {
        self.coverage.values().map(|c| c.ambiguous).sum()
    }
}

/// The edge for one (tile type, external site wire): the plain-INT
/// edge first, else the INT_INTF edge. `None` when the pin has no
/// single fabric edge (carry pins, cascade pins, ambiguous pins).
fn partner_edge<'a>(db: &'a TileconnDb, tile_type: &str, ext_wire: &str) -> Option<&'a PinWire> {
    let key = (tile_type.to_string(), ext_wire.to_string());
    db.edges.get(&key).or_else(|| db.intf_edges.get(&key))
}

/// Resolve the (partner tile name, partner wire name, partner tile
/// type) for one site pin of one site tile, failing loud when the
/// stitched tile is missing or is neither a plain INT tile nor an
/// INT_INTF tile. CLE pins resolve to INT tiles; DSP/BRAM pins to
/// INT_INTF tiles.
fn resolve_partner_tile(
    db: &TileconnDb,
    st: &SiteTile,
    ext_wire: &str,
) -> Result<Option<(String, String, String)>, TileconnError> {
    let Some(edge) = partner_edge(db, &st.tile_type, ext_wire) else {
        return Ok(None);
    };
    let grid = (st.grid_x + edge.delta.0, st.grid_y + edge.delta.1);
    let Some((tile, tile_type)) = db.tile_by_grid.get(&grid) else {
        return Err(TileconnError::UnknownIntTile {
            site: st.tile.clone(),
            grid,
        });
    };
    if tile_type != "INT" && !tile_type.starts_with("INT_INTF") {
        return Err(TileconnError::WrongIntTileType {
            tile: tile.clone(),
            tile_type: tile_type.clone(),
        });
    }
    Ok(Some((
        tile.clone(),
        edge.int_wire.clone(),
        tile_type.clone(),
    )))
}

/// Parse `<TYPE>_X<num>Y<num>` into tile coordinates, where TYPE is
/// the tile type (e.g. `INT`, `INT_INTF_R`, `INT_INTF_L_PCIE4`).
/// The name coordinates are the graph coordinates: INT_X13Y0 and
/// INT_INTF_R_X13Y0 are both (13, 0), the same column and row.
fn parse_named_tile(name: &str, tile_type: &str) -> Result<(i32, i32), TileconnError> {
    let prefix = format!("{tile_type}_X");
    let rest = name
        .strip_prefix(&prefix)
        .ok_or_else(|| TileconnError::BadIntTileName(name.to_string()))?;
    let (xs, ys) = rest
        .split_once('Y')
        .ok_or_else(|| TileconnError::BadIntTileName(name.to_string()))?;
    let x: i32 = xs
        .parse()
        .map_err(|_| TileconnError::BadIntTileName(name.to_string()))?;
    let y: i32 = ys
        .parse()
        .map_err(|_| TileconnError::BadIntTileName(name.to_string()))?;
    Ok((x, y))
}

/// Routing graph over real INT tiles with BEL pins resolved from the
/// tileconn DB: an [`IntTileWireDb`] plus the pin map. Implements
/// [`WireDb`] by delegation, so the router and the router-to-PIP
/// translator use it exactly like the graph they already know.
///
/// Construction instantiates the real INT tiles from the tilegrid
/// (names like `INT_X5Y0`, coordinates parsed from the name), NOT
/// the provisional `INT_X{x}Y{y}` naming of
/// [`IntTileWireDb::from_placed_slices`]: a site's Vivado X/Y
/// (e.g. `SLICE_X7Y0`) is not its tile's X/Y (e.g. `CLEL_L_X5Y0`
/// partners `INT_X5Y0`). The two namings coincide only at the
/// origin, which is why the blinky golden FASM says `INT_X0Y0`.
pub struct TileconnWireDb {
    inner: IntTileWireDb,
    db: TileconnDb,
    sites: Vec<String>,
}

impl TileconnWireDb {
    /// Build from INT tile JSON text, a prjuray-db family root, a
    /// part (directory name, e.g. `xczu3eg-sbva484-1-e`, or a short
    /// part with exactly one matching directory), and the placed
    /// slices to route over.
    pub fn from_placed_slices(
        int_tile_json: &str,
        db_root: &Path,
        part: &str,
        placed: &UltraPlaced,
    ) -> Result<Self, TileconnError> {
        let db = TileconnDb::load(db_root, part)?;
        let sites: Vec<(UltraSite, String)> = placed
            .slices
            .iter()
            .map(|s| {
                (
                    UltraSite {
                        x: s.x,
                        y: s.y,
                        kind: 0,
                    },
                    s.site.clone(),
                )
            })
            .collect();
        Self::from_db_and_sites(int_tile_json, db, &sites)
    }

    /// Build from an already-loaded [`TileconnDb`] and explicit
    /// `(site, site name)` pairs. The test-friendly constructor:
    /// unit tests pass fixture DBs here. Besides the INT/INT_INTF
    /// tiles the placed pins stitch to, the graph also instantiates
    /// the INT tiles neighboring them (see the expansion in the
    /// body), so continuation edges exist and the router can spill
    /// across INT tiles. Each INT_INTF tile is joined to its INT
    /// neighbor with tileconn stitch edges (same node, no PIP).
    pub fn from_db_and_sites(
        int_tile_json: &str,
        db: TileconnDb,
        sites: &[(UltraSite, String)],
    ) -> Result<Self, TileconnError> {
        if sites.is_empty() {
            return Err(TileconnError::NoSites);
        }
        // Resolve the partner tiles and pin registrations first, so
        // a bad site fails loud before any graph is built.
        let mut tiles: Vec<IntTile> = Vec::new();
        let mut tile_seen: HashMap<String, ()> = HashMap::new();
        // (site, bel, pin, partner tile, partner wire).
        let mut regs: Vec<(UltraSite, String, String, String, String)> = Vec::new();
        for (site, site_name) in sites {
            let st = db
                .site_tiles
                .get(site_name)
                .ok_or_else(|| TileconnError::UnknownSite(site_name.clone()))?;
            let file = db.tile_types.get(&st.tile_type).ok_or_else(|| {
                TileconnError::MissingTileType {
                    tile_type: st.tile_type.clone(),
                    path: db.part_dir.join("tile_types"),
                }
            })?;
            // Register this site's own pins only (the rank-th
            // same-type template): looping over every same-type
            // template registered all of them under this one site
            // and add_pin_wire kept the last one's wires.
            let s = same_type_site(file, &st.site_type, st.site_index).ok_or_else(|| {
                TileconnError::SiteIndexOutOfRange {
                    site: site_name.clone(),
                    tile_type: st.tile_type.clone(),
                    index: st.site_index,
                    count: file
                        .sites
                        .iter()
                        .filter(|s| s.site_type == st.site_type)
                        .count(),
                }
            })?;
            for (pin, pin_info) in &s.site_pins {
                let Some(info) = pin_info else {
                    continue;
                };
                let ext = external_wire(&info.wire);
                let Some((tile, wire, tile_type)) = resolve_partner_tile(&db, st, ext)? else {
                    continue;
                };
                let (tx, ty) = parse_named_tile(&tile, &tile_type)?;
                if tile_seen.insert(tile.clone(), ()).is_none() {
                    tiles.push(IntTile::new_typed(&tile, tx, ty, &tile_type));
                }
                regs.push((*site, pin.clone(), pin.clone(), tile, wire));
            }
            // Flow BEL-pin aliases for CLE slices (see module docs).
            if st.site_type == "SLICEL" || st.site_type == "SLICEM" {
                let mut alias = |regs: &mut Vec<(UltraSite, String, String, String, String)>,
                                 bel: String,
                                 pin: String,
                                 site_pin: &str|
                 -> Result<(), TileconnError> {
                    let file = db.tile_types.get(&st.tile_type).ok_or_else(|| {
                        TileconnError::MissingTileType {
                            tile_type: st.tile_type.clone(),
                            path: db.part_dir.join("tile_types"),
                        }
                    })?;
                    // The site's own template only: the old loop
                    // over every same-type template kept the last
                    // one's wire.
                    let s = same_type_site(file, &st.site_type, st.site_index);
                    let ext_wire: Option<String> = match s {
                        Some(s) => match s.site_pins.get(site_pin) {
                            Some(Some(info)) => {
                                Some(external_wire(&info.wire).to_string())
                            }
                            _ => None,
                        },
                        None => None,
                    };
                    let Some(ext) = ext_wire else {
                        return Ok(());
                    };
                    let Some((tile, wire, tile_type)) = resolve_partner_tile(&db, st, &ext)? else {
                        return Ok(());
                    };
                    let (tx, ty) = parse_named_tile(&tile, &tile_type)?;
                    if tile_seen.insert(tile.clone(), ()).is_none() {
                        tiles.push(IntTile::new_typed(&tile, tx, ty, &tile_type));
                    }
                    regs.push((*site, bel, pin, tile, wire));
                    Ok(())
                };
                for letter in 'A'..='H' {
                    let l = letter.to_string();
                    for i in 1..=6 {
                        alias(
                            &mut regs,
                            format!("{l}6LUT"),
                            format!("I{}", i - 1),
                            &format!("{l}{i}"),
                        )?;
                        alias(
                            &mut regs,
                            format!("{l}6LUT"),
                            format!("{l}{i}"),
                            &format!("{l}{i}"),
                        )?;
                    }
                    alias(&mut regs, format!("{l}6LUT"), "O6".to_string(), &format!("{l}_O"))?;
                    alias(
                        &mut regs,
                        format!("{l}6LUT"),
                        "O5".to_string(),
                        &format!("{l}MUX"),
                    )?;
                    alias(&mut regs, format!("{l}Q"), "Q".to_string(), &format!("{l}Q"))?;
                    alias(
                        &mut regs,
                        format!("{l}Q2"),
                        "Q".to_string(),
                        &format!("{l}Q2"),
                    )?;
                    alias(&mut regs, format!("{l}Q"), "D".to_string(), &format!("{l}X"))?;
                    alias(&mut regs, format!("{l}Q2"), "D".to_string(), &format!("{l}X"))?;
                }
            }
        }
        // Tile name to tilegrid grid coordinates. Tile NAME
        // coordinates and tilegrid GRID coordinates differ
        // (INT_X13Y0 is name (13, 0) but grid (217, 185)), and
        // tileconn deltas are grid deltas, so every stitch lookup
        // below goes through this map, never through name coords.
        let grid_of: HashMap<String, (i32, i32)> = db
            .tile_by_grid
            .iter()
            .map(|((gx, gy), (name, _))| (name.clone(), (*gx, *gy)))
            .collect();
        // Instantiate each INT_INTF tile's stitch-partner INT tile:
        // the INT tile tileconn names at the pair's grid delta. The
        // pin loop above only instantiated tiles pins stitch to, so
        // without this the stitch edges below would have no INT end.
        // A pair whose INT tile is missing from the tilegrid fails
        // loud: the DB and the tilegrid disagree.
        let intf_tiles: Vec<IntTile> = tiles
            .iter()
            .filter(|t| t.tile_type.starts_with("INT_INTF"))
            .cloned()
            .collect();
        for t in &intf_tiles {
            let Some(pairs) = db.intf_stitches.get(&t.tile_type) else {
                continue;
            };
            let (igx, igy) = grid_of[&t.name];
            let mut partner_grids: std::collections::HashSet<(i32, i32)> =
                std::collections::HashSet::new();
            for (_, _, (dx, dy)) in pairs {
                partner_grids.insert((igx + dx, igy + dy));
            }
            for grid in partner_grids {
                let Some((name, tile_type)) = db.tile_by_grid.get(&grid) else {
                    return Err(TileconnError::UnknownIntTile {
                        site: t.name.clone(),
                        grid,
                    });
                };
                if tile_type != "INT" {
                    return Err(TileconnError::WrongIntTileType {
                        tile: name.clone(),
                        tile_type: tile_type.clone(),
                    });
                }
                if tile_seen.insert(name.clone(), ()).is_none() {
                    tiles.push(IntTile::new(name, grid.0, grid.1));
                }
            }
        }
        // Expand the graph to the INT tiles neighboring each seed INT
        // tile in tile-name coordinates (dx, dy in -1..=1). A seed
        // tile alone has zero continuation edges, so the router can
        // never leave it: every inter-slice net must fit inside one
        // tile and PathFinder deadlocks on the same BYPASS/IMUX wires.
        // Neighbor tiles give the router somewhere to spill: span
        // wires (EE1, EE2, NN1, WW1, and so on) continue into the
        // tile one name step away, so a net can detour
        // through INT_X2+ and back. Only tiles that exist in this
        // part's tilegrid (type INT) are added, so no invented tile
        // ever reaches FASM or bitgen; a tile at a negative or
        // missing coordinate is skipped (for example INT_X1Y-1 does
        // not exist). The seed tiles are unchanged, so single-slice
        // designs keep exactly their old graph plus neighbors they
        // can also use. INT_INTF tiles are not expanded: they have
        // no span wires, so their only neighbors are the stitch
        // partners added above.
        let existing_int: std::collections::HashSet<String> = db
            .tile_by_grid
            .values()
            .filter(|(_, t)| t == "INT")
            .map(|(n, _)| n.clone())
            .collect();
        let seed_tiles: Vec<IntTile> = tiles
            .iter()
            .filter(|t| t.tile_type == "INT")
            .cloned()
            .collect();
        for t in &seed_tiles {
            for dx in -1..=1i32 {
                for dy in -1..=1i32 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (t.x + dx, t.y + dy);
                    if nx < 0 || ny < 0 {
                        continue;
                    }
                    let name = format!("INT_X{nx}Y{ny}");
                    if !existing_int.contains(&name) {
                        continue;
                    }
                    if tile_seen.insert(name.clone(), ()).is_none() {
                        tiles.push(IntTile::new(&name, nx, ny));
                    }
                }
            }
        }
        tiles.sort_by_key(|t| (t.y, t.x));
        // Load one JSON per INT_INTF tile type present, from the DB
        // root (runtime loading, never vendored).
        let mut intf_types: Vec<String> = tiles
            .iter()
            .filter(|t| t.tile_type.starts_with("INT_INTF"))
            .map(|t| t.tile_type.clone())
            .collect();
        intf_types.sort();
        intf_types.dedup();
        let mut intf_jsons: Vec<(String, String)> = Vec::with_capacity(intf_types.len());
        for t in &intf_types {
            intf_jsons.push((t.clone(), db.intf_tile_json(t)?));
        }
        let mut json_refs: Vec<(&str, &str)> = vec![("INT", int_tile_json)];
        for (t, j) in &intf_jsons {
            json_refs.push((t.as_str(), j.as_str()));
        }
        let mut inner = IntTileWireDb::from_json_typed(&json_refs, &tiles)?;
        for (site, bel, pin, tile, wire) in &regs {
            inner.add_pin_wire(*site, bel, pin, tile, wire)?;
        }
        // Tileconn stitch edges: one bidirectional same-node edge per
        // (INT_INTF wire, INT wire) pair, for each instantiated
        // INT_INTF tile. A pair whose INT tile was not instantiated
        // fails loud: the graph and tileconn disagree.
        for t in &intf_tiles {
            let Some(pairs) = db.intf_stitches.get(&t.tile_type) else {
                continue;
            };
            let (igx, igy) = grid_of[&t.name];
            for (intf_wire, int_wire, (dx, dy)) in pairs {
                let grid = (igx + dx, igy + dy);
                let Some((int_tile, _)) = db.tile_by_grid.get(&grid) else {
                    return Err(TileconnError::UnknownIntTile {
                        site: t.name.clone(),
                        grid,
                    });
                };
                let (Some(a), Some(b)) = (
                    inner.wire_id(&t.name, intf_wire),
                    inner.wire_id(int_tile, int_wire),
                ) else {
                    return Err(TileconnError::UnknownIntTile {
                        site: format!(
                            "{}.{} stitch to {}.{}",
                            t.name, intf_wire, int_tile, int_wire
                        ),
                        grid,
                    });
                };
                inner.add_stitch(a, b).map_err(|e| {
                    TileconnError::Io(format!(
                        "stitch {}.{} to {}.{}: {}",
                        t.name, intf_wire, int_tile, int_wire, e
                    ))
                })?;
            }
        }
        Ok(Self {
            inner,
            db,
            sites: sites.iter().map(|(_, n)| n.clone()).collect(),
        })
    }

    /// The loaded tileconn DB (coverage, edges, site lookup).
    pub fn tileconn_db(&self) -> &TileconnDb {
        &self.db
    }

    /// Site names this graph was built for.
    pub fn sites(&self) -> &[String] {
        &self.sites
    }

    /// Number of INT tile instances in the graph.
    pub fn tile_count(&self) -> usize {
        self.inner.tile_count()
    }

    /// Number of INT_INTF tile instances in the graph.
    pub fn intf_tile_count(&self) -> usize {
        self.inner
            .tiles()
            .iter()
            .filter(|t| t.tile_type.starts_with("INT_INTF"))
            .count()
    }

    /// Number of tileconn stitch edges in the graph.
    pub fn stitch_count(&self) -> usize {
        self.inner.stitch_count()
    }

    /// Wires per tile instance (1,475 for the vendored INT tile JSON).
    pub fn wires_per_tile(&self) -> usize {
        self.inner.wires_per_tile()
    }

    /// PIPs per tile instance (3,774 for the vendored INT tile JSON).
    pub fn pip_count(&self) -> usize {
        self.inner.pip_count()
    }

    /// Wire ID for a (tile, wire name) pair.
    pub fn wire_id(&self, tile: &str, wire: &str) -> Option<WireId> {
        self.inner.wire_id(tile, wire)
    }

    /// Remove one intra-tile PIP edge (`src` -> `dst` on `tile`)
    /// from the routing graph. See
    /// [`IntTileWireDb::remove_edge`]: the router can never traverse
    /// the PIP again, so a design whose clock leg conflicts with it
    /// routes around it instead of failing at bitgen. Returns true
    /// when the edge existed and was removed; false when the tile or
    /// a wire is unknown, or the pair was not an edge. Callers fail
    /// loud on false: a forbidden PIP the graph never had means the
    /// segbits table and the tile JSON disagree, and silently
    /// routing on would reintroduce the bitgen failure.
    pub fn forbid_pip(&mut self, tile: &str, src: &str, dst: &str) -> bool {
        let (Some(s), Some(d)) = (self.wire_id(tile, src), self.wire_id(tile, dst)) else {
            return false;
        };
        self.inner.remove_edge(s, d)
    }

    /// Translate one ordered route to INT PIPs, resolving
    /// bidirectional PIPs from the PIP table.
    pub fn translate_route(
        &self,
        route: &[WireId],
        design: &str,
    ) -> Result<TranslatedRoute, RouteToPipError> {
        self.inner.translate_route(route, design)
    }

    /// Traversal direction of a bidirectional PIP between two wires.
    pub fn pip_bidir(&self, a: WireId, b: WireId) -> Option<BidirDir> {
        self.inner.pip_bidir(a, b)
    }
}

impl WireDb for TileconnWireDb {
    fn wire_count(&self) -> usize {
        self.inner.wire_count()
    }

    fn fanout(&self, wire: WireId) -> &[WireId] {
        self.inner.fanout(wire)
    }

    fn wire_xy(&self, wire: WireId) -> (i32, i32) {
        self.inner.wire_xy(wire)
    }

    fn wire_for_bel_pin(&self, site: UltraSite, bel: &str, pin: &str) -> Option<WireId> {
        self.inner.wire_for_bel_pin(site, bel, pin)
    }

    fn wire_info(&self, wire: WireId) -> Option<WireInfo<'_>> {
        self.inner.wire_info(wire)
    }

    fn is_stitch(&self, a: WireId, b: WireId) -> bool {
        self.inner.is_stitch(a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{RoutableNet, UltraRouteOpts, route_ultra};

    /// The vendored INT tile JSON (BitFiltrator, MIT).
    const TILE_JSON: &str = include_str!("../data/tile_type_INT.json");

    const TINY_TILECONN: &str = r#"[
      {"grid_deltas": [-1, 0], "tile_types": ["CLEL_R", "INT"], "wire_pairs": [
        ["CLE_CLE_L_SITE_0_A1", "IMUX_E10"],
        ["CLE_CLE_L_SITE_0_A_O", "LOGIC_OUTS_E0"],
        ["CLE_CLE_L_SITE_0_AQ", "LOGIC_OUTS_E14"],
        ["CLE_CLE_L_SITE_0_AX", "BOUNCE_E_0_FT1"]
      ]},
      {"grid_deltas": [0, 1], "tile_types": ["CLEL_R", "CLEL_R"], "wire_pairs": [
        ["CLE_CLE_L_SITE_0_CIN", "CLE_CLE_L_SITE_0_COUT"]
      ]}
    ]"#;

    const TINY_TILEGRID: &str = r#"{
      "CLEL_R_X0Y0": {"grid_x": 161, "grid_y": 185, "type": "CLEL_R",
        "sites": {"SLICE_X0Y0": "SLICEL"}},
      "INT_X0Y0": {"grid_x": 160, "grid_y": 185, "type": "INT", "sites": {}}
    }"#;

    const TINY_TILE_TYPE_CLEL_R: &str = r#"{
      "tile_type": "CLEL_R",
      "sites": [{"name": "X0Y0", "type": "SLICEL", "site_pins": {
        "A1": {"wire": "CLE_CLE_L_SITE_0_A1"},
        "A_O": {"wire": "CLE_CLE_L_SITE_0_A_O"},
        "AQ": {"wire": "CLE_CLE_L_SITE_0_AQ"},
        "AX": {"wire": "CLE_CLE_L_SITE_0_AX"},
        "CIN": {"wire": "CLE_CLE_L_SITE_0_CIN_PIN"},
        "COUT": {"wire": "CLE_CLE_L_SITE_0_COUT"}
      }}]
    }"#;

    /// Two-site fixture (separate from TINY, whose counts it must
    /// not disturb): one tile type with TWO sites of the same type
    /// and distinct OP wires. Before the site_index fix,
    /// int_tile_for_site returned None for OP on both sites (the
    /// ambiguity) and from_db_and_sites registered both OP wires
    /// under one site with the last (highest-Y) wire winning.
    const TWO_SITE_TILECONN: &str = r#"[
      {"grid_deltas": [-1, 0], "tile_types": ["TESTIO", "INT"], "wire_pairs": [
        ["TEST_IOBPAIR_0_OP", "IMUX_E10"],
        ["TEST_IOBPAIR_2_OP", "IMUX_E12"]
      ]}
    ]"#;

    const TWO_SITE_TILEGRID: &str = r#"{
      "TESTIO_X0Y0": {"grid_x": 10, "grid_y": 10, "type": "TESTIO",
        "sites": {"IOB_X0Y0": "TESTIOB", "IOB_X0Y2": "TESTIOB"}},
      "INT_X0Y0": {"grid_x": 9, "grid_y": 10, "type": "INT", "sites": {}}
    }"#;

    const TWO_SITE_TILE_TYPE_TESTIO: &str = r#"{
      "tile_type": "TESTIO",
      "sites": [
        {"name": "X0Y0", "type": "TESTIOB", "site_pins": {
          "OP": {"wire": "TEST_IOBPAIR_0_OP_PIN"}
        }},
        {"name": "X0Y2", "type": "TESTIOB", "site_pins": {
          "OP": {"wire": "TEST_IOBPAIR_2_OP_PIN"}
        }}
      ]
    }"#;

    fn fixture_db() -> (TempfileDir, TileconnDb) {
        // Fixture files live in a temp dir (tests never write
        // outside the temp dir). The dir name is unique per call:
        // lib tests run in parallel threads of one process, so a
        // plain per-process name would let one test's cleanup delete
        // another test's fixture mid-load.
        use std::sync::atomic::{AtomicU64, Ordering};
        static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);
        let n = FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "helion-tileconn-fixture-{}-{n}",
            std::process::id()
        ));
        let part_dir = dir.join("xczu3eg-sbva484-1-e");
        let _ = fs::create_dir_all(&part_dir);
        fs::write(part_dir.join("tileconn.json"), TINY_TILECONN).unwrap();
        fs::write(part_dir.join("tilegrid.json"), TINY_TILEGRID).unwrap();
        // tile_types live at the family root, not under the part dir.
        let root_types = dir.join("tile_types");
        let _ = fs::create_dir_all(&root_types);
        fs::write(
            root_types.join("tile_type_CLEL_R.json"),
            TINY_TILE_TYPE_CLEL_R,
        )
        .unwrap();
        let db = TileconnDb::load(&dir, "xczu3eg-sbva484-1-e").unwrap();
        (TempfileDir(dir), db)
    }

    // Keep the fixture dir alive until the test ends.
    struct TempfileDir(PathBuf);
    impl Drop for TempfileDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn site0() -> UltraSite {
        UltraSite {
            x: 0,
            y: 0,
            kind: 0,
        }
    }

    fn site1() -> UltraSite {
        UltraSite {
            x: 0,
            y: 1,
            kind: 0,
        }
    }

    /// Two-site fixture DB (see TWO_SITE_* above). Separate builder
    /// so TINY stays untouched.
    fn fixture_db_two_sites() -> (TempfileDir, TileconnDb) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static FIXTURE_SEQ_2: AtomicU64 = AtomicU64::new(0);
        let n = FIXTURE_SEQ_2.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "helion-tileconn-fixture2-{}-{n}",
            std::process::id()
        ));
        let part_dir = dir.join("xczu3eg-sbva484-1-e");
        let _ = fs::create_dir_all(&part_dir);
        fs::write(part_dir.join("tileconn.json"), TWO_SITE_TILECONN).unwrap();
        fs::write(part_dir.join("tilegrid.json"), TWO_SITE_TILEGRID).unwrap();
        let root_types = dir.join("tile_types");
        let _ = fs::create_dir_all(&root_types);
        fs::write(
            root_types.join("tile_type_TESTIO.json"),
            TWO_SITE_TILE_TYPE_TESTIO,
        )
        .unwrap();
        let db = TileconnDb::load(&dir, "xczu3eg-sbva484-1-e").unwrap();
        (TempfileDir(dir), db)
    }

    #[test]
    fn multi_site_type_resolves_each_sites_own_wire() {
        let (_dir, db) = fixture_db_two_sites();
        // Rank among same-type sites, by (X, Y): IOB_X0Y0 is first,
        // IOB_X0Y2 second.
        assert_eq!(db.site_tiles["IOB_X0Y0"].site_index, 0);
        assert_eq!(db.site_tiles["IOB_X0Y2"].site_index, 1);
        // Each site's OP resolves to its own INT wire (before the
        // fix both returned None: the pin was present on both
        // same-type templates, so the loop hit the ambiguity arm).
        assert_eq!(
            db.int_tile_for_site("IOB_X0Y0", "OP"),
            Some(("INT_X0Y0".to_string(), "IMUX_E10".to_string()))
        );
        assert_eq!(
            db.int_tile_for_site("IOB_X0Y2", "OP"),
            Some(("INT_X0Y0".to_string(), "IMUX_E12".to_string()))
        );
        // The wire DB registers each OP under its own UltraSite with
        // its own wire (before the fix both registered under one
        // site and the last wire won).
        let wdb = TileconnWireDb::from_db_and_sites(
            TILE_JSON,
            db,
            &[(site0(), "IOB_X0Y0".into()), (site1(), "IOB_X0Y2".into())],
        )
        .unwrap();
        assert_eq!(
            wdb.wire_for_bel_pin(site0(), "OP", "OP"),
            wdb.wire_id("INT_X0Y0", "IMUX_E10")
        );
        assert_eq!(
            wdb.wire_for_bel_pin(site1(), "OP", "OP"),
            wdb.wire_id("INT_X0Y0", "IMUX_E12")
        );
    }

    #[test]
    fn fixture_db_loads_with_known_edges() {
        let (_dir, db) = fixture_db();
        assert_eq!(db.tileconn_entries, 2);
        assert_eq!(db.tileconn_pairs, 5);
        let cov = db.coverage("CLEL_R", "SLICEL");
        assert_eq!(cov.pins, 6);
        assert_eq!(cov.any_partner, 6);
        // CIN/COUT stitch to the neighbor slice, never to INT.
        assert_eq!(cov.int_partner, 4);
        assert_eq!(db.loaded_tile_types, vec!["CLEL_R".to_string()]);
        assert!(db.missing_tile_types.is_empty());
        // Known BEL-pin resolutions, verbatim from the real DB.
        let a1 = db.pin_wire_for_pin("CLEL_R", "A1").unwrap();
        assert_eq!(a1.int_wire, "IMUX_E10");
        assert_eq!(a1.delta, (-1, 0));
        assert_eq!(
            db.pin_wire_for_pin("CLEL_R", "A_O").unwrap().int_wire,
            "LOGIC_OUTS_E0"
        );
        assert_eq!(
            db.pin_wire_for_pin("CLEL_R", "AX").unwrap().int_wire,
            "BOUNCE_E_0_FT1"
        );
        // Carry pins have no INT partner.
        assert_eq!(db.pin_wire_for_pin("CLEL_R", "CIN"), None);
        assert_eq!(db.pin_wire_for_pin("CLEL_R", "COUT"), None);
        // Site to INT tile resolution.
        assert_eq!(
            db.int_tile_for_site("SLICE_X0Y0", "A1"),
            Some(("INT_X0Y0".to_string(), "IMUX_E10".to_string()))
        );
        assert_eq!(db.int_tile_for_site("SLICE_X0Y0", "CIN"), None);
        assert_eq!(db.int_tile_for_site("SLICE_X9Y9", "A1"), None);
    }

    #[test]
    fn fixture_wire_db_resolves_flow_bel_pins() {
        let (_dir, db) = fixture_db();
        let wdb =
            TileconnWireDb::from_db_and_sites(TILE_JSON, db, &[(site0(), "SLICE_X0Y0".into())])
                .unwrap();
        assert_eq!(wdb.tile_count(), 1);
        assert_eq!(wdb.wires_per_tile(), 1475);
        assert_eq!(wdb.pip_count(), 3774);
        let o6 = wdb.wire_id("INT_X0Y0", "LOGIC_OUTS_E0").unwrap();
        assert_eq!(wdb.wire_for_bel_pin(site0(), "A6LUT", "O6"), Some(o6));
        assert_eq!(
            wdb.wire_for_bel_pin(site0(), "A6LUT", "I0"),
            wdb.wire_id("INT_X0Y0", "IMUX_E10")
        );
        assert_eq!(
            wdb.wire_for_bel_pin(site0(), "AQ", "D"),
            wdb.wire_id("INT_X0Y0", "BOUNCE_E_0_FT1")
        );
        assert_eq!(
            wdb.wire_for_bel_pin(site0(), "AQ", "Q"),
            wdb.wire_id("INT_X0Y0", "LOGIC_OUTS_E14")
        );
        // Raw site-pin form resolves too.
        assert_eq!(
            wdb.wire_for_bel_pin(site0(), "A1", "A1"),
            wdb.wire_id("INT_X0Y0", "IMUX_E10")
        );
        // Negative cases: unmapped pin, carry pin, other site.
        assert_eq!(wdb.wire_for_bel_pin(site0(), "A6LUT", "I9"), None);
        assert_eq!(wdb.wire_for_bel_pin(site0(), "CARRY8", "CIN"), None);
        assert_eq!(
            wdb.wire_for_bel_pin(
                UltraSite {
                    x: 1,
                    y: 0,
                    kind: 0
                },
                "A6LUT",
                "O6"
            ),
            None
        );
        // wire_info names the resolved wires for the translator.
        assert_eq!(
            wdb.wire_info(o6),
            Some(WireInfo {
                name: "LOGIC_OUTS_E0",
                tile: "INT_X0Y0"
            })
        );
    }

    #[test]
    fn forbid_pip_removes_one_edge_from_the_tile() {
        // The N46 conflict edge on its tile: LOGIC_OUTS_W22 drives
        // INT_NODE_SDQ_80_INT_OUT1. Forbidding it leaves the sibling
        // PIPs from the same source wire alone; a second forbid, an
        // unknown tile, and an unknown wire all report false instead
        // of removing anything.
        let (_dir, db) = fixture_db();
        let mut wdb =
            TileconnWireDb::from_db_and_sites(TILE_JSON, db, &[(site0(), "SLICE_X0Y0".into())])
                .unwrap();
        let src = wdb.wire_id("INT_X0Y0", "LOGIC_OUTS_W22").unwrap();
        let dst = wdb
            .wire_id("INT_X0Y0", "INT_NODE_SDQ_80_INT_OUT1")
            .unwrap();
        let sib = wdb
            .wire_id("INT_X0Y0", "INT_NODE_SDQ_78_INT_OUT0")
            .unwrap();
        assert!(wdb.fanout(src).contains(&dst));
        assert!(wdb.fanout(src).contains(&sib));
        assert!(wdb.forbid_pip("INT_X0Y0", "LOGIC_OUTS_W22", "INT_NODE_SDQ_80_INT_OUT1"));
        assert!(!wdb.fanout(src).contains(&dst));
        assert!(wdb.fanout(src).contains(&sib));
        assert!(!wdb.forbid_pip("INT_X0Y0", "LOGIC_OUTS_W22", "INT_NODE_SDQ_80_INT_OUT1"));
        assert!(!wdb.forbid_pip("INT_X9Y9", "LOGIC_OUTS_W22", "INT_NODE_SDQ_80_INT_OUT1"));
        assert!(!wdb.forbid_pip("INT_X0Y0", "NOPE", "INT_NODE_SDQ_80_INT_OUT1"));
    }

    #[test]
    fn fixture_blinky_q_net_routes_and_translates() {
        let (_dir, db) = fixture_db();
        let wdb =
            TileconnWireDb::from_db_and_sites(TILE_JSON, db, &[(site0(), "SLICE_X0Y0".into())])
                .unwrap();
        // The blinky q net exactly as build_routable_nets builds it:
        // LUT O6 drives FF D, one sink.
        let nets = vec![RoutableNet {
            name: "q".to_string(),
            driver: (site0(), "A6LUT".to_string(), "O6".to_string()),
            sinks: vec![(site0(), "AQ".to_string(), "D".to_string())],
            is_clock: false,
        }];
        let routed = route_ultra(&nets, &wdb, UltraRouteOpts::default()).unwrap();
        assert_eq!(routed.routes.len(), 1);
        let route = &routed.routes[0];
        let names: Vec<&str> = route
            .iter()
            .map(|w| wdb.wire_info(*w).unwrap().name)
            .collect();
        assert_eq!(
            names,
            vec!["LOGIC_OUTS_E0", "INODE_E_1_FT1", "BOUNCE_E_0_FT1"]
        );
        let t = wdb.translate_route(route, "blinky").unwrap();
        assert_eq!(t.pips.len(), 2);
        let fasm = crate::route_to_pip::translated_to_fasm(std::slice::from_ref(&t));
        // The golden blinky PIP, verbatim from harness/golden/blinky.xczu3eg.fasm.
        assert!(
            fasm.contains("INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"),
            "translated FASM:\n{fasm}"
        );
        assert!(
            fasm.contains("INT_X0Y0.PIP.INODE_E_1_FT1.LOGIC_OUTS_E0"),
            "translated FASM:\n{fasm}"
        );
    }

    #[test]
    fn loader_errors_are_loud() {
        // Unknown site.
        let (_dir, db) = fixture_db();
        assert!(matches!(
            TileconnWireDb::from_db_and_sites(TILE_JSON, db, &[(site0(), "SLICE_X9Y9".into())]),
            Err(TileconnError::UnknownSite(_))
        ));
        // No sites.
        let (_dir, db) = fixture_db();
        assert!(matches!(
            TileconnWireDb::from_db_and_sites(TILE_JSON, db, &[]),
            Err(TileconnError::NoSites)
        ));
        // Missing part directory.
        let dir = std::env::temp_dir().join(format!("helion-tileconn-nodb-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let err = TileconnDb::load(&dir, "xczu3eg-sbva484-1-e").unwrap_err();
        assert!(matches!(err, TileconnError::NoPartDir { .. }), "{err}");
        let _ = fs::remove_dir_all(&dir);
        // Ambiguous part prefix: two matching directories.
        let dir =
            std::env::temp_dir().join(format!("helion-tileconn-ambig-{}", std::process::id()));
        let _ = fs::create_dir_all(dir.join("xczu3eg-sbva484-1-e"));
        let _ = fs::create_dir_all(dir.join("xczu3eg-sfvc784-1-e"));
        let err = TileconnDb::load(&dir, "xczu3eg").unwrap_err();
        assert!(matches!(err, TileconnError::AmbiguousPart { .. }), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_wire_strips_pin_suffix_only() {
        assert_eq!(external_wire("CLE_CLE_L_SITE_0_CIN_PIN"), "CLE_CLE_L_SITE_0_CIN");
        assert_eq!(
            external_wire("DSP_DSP48_0_ACIN_B0_PIN"),
            "DSP_DSP48_0_ACIN_B0"
        );
        assert_eq!(external_wire("CLE_CLE_L_SITE_0_A1"), "CLE_CLE_L_SITE_0_A1");
        assert_eq!(external_wire("VCC_WIRE"), "VCC_WIRE");
    }

    /// Neighbor expansion: the TINY fixture's tilegrid gains the
    /// INT tiles one name step east and north of the seed INT_X0Y0
    /// (INT_X1Y0, INT_X2Y0, INT_X0Y1, INT_X1Y1). Before the fix the
    /// graph held only the seed tile (tile_count 1, zero
    /// continuation edges); with the fix it holds the seed plus its
    /// existing neighbors, and a span wire continues from the seed
    /// into INT_X1Y0. INT_X2Y0 is two steps away, so it is not
    /// added from this seed.
    const NEIGHBOR_TILEGRID: &str = r#"{
      "CLEL_R_X0Y0": {"grid_x": 161, "grid_y": 185, "type": "CLEL_R",
        "sites": {"SLICE_X0Y0": "SLICEL"}},
      "INT_X0Y0": {"grid_x": 160, "grid_y": 185, "type": "INT", "sites": {}},
      "INT_X1Y0": {"grid_x": 165, "grid_y": 185, "type": "INT", "sites": {}},
      "INT_X2Y0": {"grid_x": 169, "grid_y": 185, "type": "INT", "sites": {}},
      "INT_X0Y1": {"grid_x": 160, "grid_y": 184, "type": "INT", "sites": {}},
      "INT_X1Y1": {"grid_x": 165, "grid_y": 184, "type": "INT", "sites": {}}
    }"#;

    fn fixture_db_neighbors() -> (TempfileDir, TileconnDb) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static FIXTURE_SEQ_3: AtomicU64 = AtomicU64::new(0);
        let n = FIXTURE_SEQ_3.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "helion-tileconn-fixture3-{}-{n}",
            std::process::id()
        ));
        let part_dir = dir.join("xczu3eg-sbva484-1-e");
        let _ = fs::create_dir_all(&part_dir);
        fs::write(part_dir.join("tileconn.json"), TINY_TILECONN).unwrap();
        fs::write(part_dir.join("tilegrid.json"), NEIGHBOR_TILEGRID).unwrap();
        let root_types = dir.join("tile_types");
        let _ = fs::create_dir_all(&root_types);
        fs::write(
            root_types.join("tile_type_CLEL_R.json"),
            TINY_TILE_TYPE_CLEL_R,
        )
        .unwrap();
        let db = TileconnDb::load(&dir, "xczu3eg-sbva484-1-e").unwrap();
        (TempfileDir(dir), db)
    }

    #[test]
    fn wire_db_expands_to_neighboring_int_tiles() {
        let (_dir, db) = fixture_db_neighbors();
        let wdb =
            TileconnWireDb::from_db_and_sites(TILE_JSON, db, &[(site0(), "SLICE_X0Y0".into())])
                .unwrap();
        assert_eq!(wdb.tile_count(), 4);
        assert_eq!(wdb.wire_count(), 4 * 1475);
        // The east neighbor exists in the graph, so a length-1 span
        // continues out of the seed tile instead of dead-ending.
        let beg = wdb.wire_id("INT_X0Y0", "EE1_E_BEG0").unwrap();
        let end = wdb.wire_id("INT_X1Y0", "EE1_E_END0").unwrap();
        assert!(wdb.fanout(beg).contains(&end));
        assert!(wdb.fanout(end).contains(&beg));
        // The seed tile's own pin wires are unchanged by expansion.
        assert_eq!(
            wdb.wire_for_bel_pin(site0(), "A6LUT", "O6"),
            wdb.wire_id("INT_X0Y0", "LOGIC_OUTS_E0")
        );
    }

    #[test]
    fn int_tile_name_parses() {
        assert_eq!(parse_named_tile("INT_X0Y0", "INT"), Ok((0, 0)));
        assert_eq!(parse_named_tile("INT_X12Y45", "INT"), Ok((12, 45)));
        // INT and INT_INTF tiles in one column share name
        // coordinates: INT_X13Y0 and INT_INTF_R_X13Y0 are both
        // (13, 0).
        assert_eq!(
            parse_named_tile("INT_INTF_R_X13Y0", "INT_INTF_R"),
            Ok((13, 0))
        );
        assert_eq!(
            parse_named_tile("INT_INTF_L_PCIE4_X12Y0", "INT_INTF_L_PCIE4"),
            Ok((12, 0))
        );
        assert!(parse_named_tile("INT_INTF_L_X0Y0", "INT").is_err());
        assert!(parse_named_tile("CLEL_R_X0Y0", "INT").is_err());
        assert!(parse_named_tile("INT_X0Y0", "INT_INTF_R").is_err());
    }
}
