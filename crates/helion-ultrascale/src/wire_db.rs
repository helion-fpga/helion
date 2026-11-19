//! Routing graph for UltraScale+ INT tiles, built from public sources only.
//!
//! The graph is built from the vendored BitFiltrator INT tile JSON
//! (`crates/helion-ultrascale/data/tile_type_INT.json`, MIT, extracted
//! from the BitFiltrator arch JSON): 1,475 wires and 3,774 PIPs per INT
//! tile, of which 3,770 are directional and 4 are bidirectional. The
//! INT PIP audit (data/INT_PIP_AUDIT.md) maps 3,774 of 3,774 tile PIPs
//! to prjuray-db segbits features. No RapidWright .dat and no FPGA
//! Interchange DB is ingested: both are under the Xilinx EULA.
//!
//! Node layout: one node per (tile instance, wire name). A tile instance
//! is supplied by the caller as a (tile name, x, y) triple; this module
//! never invents device tile coordinates. A full-device graph needs the
//! xcku3p tilegrid (US-01, no public source), so callers instantiate
//! the tiles they route over, e.g. the tiles covering the placed slices
//! plus their neighbours.
//!
//! Edges, all derived from the tile JSON:
//! - Intra-tile PIPs: a directed edge src to dst for every directional
//!   PIP, both directions for the 4 bidirectional PIPs.
//! - Inter-tile continuations: a span wire named `<DIR><LEN>[_E|_W]_BEG<n>`
//!   in tile T is the same physical conductor as
//!   `<DIR><LEN>[_E|_W]_END<n>` in the tile LEN steps away in direction
//!   DIR (NN north, SS south, EE east, WW west). The continuation edge
//!   runs both ways between those two nodes and carries no PIP: the
//!   translator skips it. This pairing is a naming-convention inference
//!   from the tile JSON itself (all 208 BEG wires match the pattern and
//!   every BEG has exactly one END partner form); it is not Vivado
//!   ground truth and needs the US-01 tilegrid confirmation. Wires that
//!   fit neither a PIP nor a continuation (e.g. the `_FT3` fragments,
//!   422 wires with no PIP) are isolated nodes.
//!
//! BEL-pin mapping: no public source maps a BEL pin (e.g. A6LUT O6) to
//! an INT wire. `wire_for_bel_pin` therefore returns `None` unless the
//! caller registers the mapping explicitly with [`IntTileWireDb::add_pin_wire`].
//! The router then fails loud (`NoPath`, naming the pin) instead of
//! routing from a guessed wire. The site-pin-to-wire table is US-01
//! data; until it exists, pin mappings are caller-supplied and must be
//! labelled as such wherever they are used.

use crate::fabric::{UltraSite, WireDb, WireId, WireInfo};
use crate::int_pip::BidirDir;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

/// Error building an [`IntTileWireDb`] from tile JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WireDbError {
    /// The JSON text does not parse.
    Json(String),
    /// The JSON is not an INT tile file.
    WrongTileType(String),
    /// A PIP references a wire that is not in the tile wire list.
    UnknownWireInPip { pip: String, wire: String },
    /// A PIP is marked pseudo: it is not a real mux, so it cannot be
    /// a routing edge. Fail loud rather than route through it.
    PseudoPip(String),
    /// No tile instances were supplied.
    EmptyTiles,
    /// Two tile instances share a name or coordinates.
    DuplicateTile(String),
    /// `add_pin_wire` named a tile or wire this DB does not have.
    UnknownPinWire { tile: String, wire: String },
}

impl fmt::Display for WireDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "INT tile JSON does not parse: {}", e),
            Self::WrongTileType(t) => {
                write!(f, "INT tile JSON has tile_type '{}', expected 'INT'", t)
            }
            Self::UnknownWireInPip { pip, wire } => {
                write!(f, "PIP '{}' references unknown wire '{}'", pip, wire)
            }
            Self::PseudoPip(pip) => write!(
                f,
                "PIP '{}' is pseudo (not a real mux); refusing to route through it",
                pip
            ),
            Self::EmptyTiles => write!(f, "no INT tile instances supplied"),
            Self::DuplicateTile(t) => write!(f, "duplicate INT tile instance '{}'", t),
            Self::UnknownPinWire { tile, wire } => {
                write!(f, "no wire '{}' in tile '{}'", wire, tile)
            }
        }
    }
}

impl std::error::Error for WireDbError {}

/// One INT tile instance in the graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntTile {
    /// Vivado tile name, e.g. `INT_X12Y45`.
    pub name: String,
    /// Tile X coordinate (Vivado tile grid).
    pub x: i32,
    /// Tile Y coordinate (Vivado tile grid).
    pub y: i32,
}

impl IntTile {
    pub fn new(name: &str, x: i32, y: i32) -> Self {
        Self {
            name: name.to_string(),
            x,
            y,
        }
    }
}

#[derive(Debug, Deserialize)]
struct IntTileJson {
    tile_type: String,
    pips: HashMap<String, IntTilePipJson>,
    wires: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct IntTilePipJson {
    src_wire: String,
    dst_wire: String,
    is_directional: String,
    is_pseudo: String,
}

/// Span-wire naming: `<DIR><LEN>[_E|_W]_(BEG|END)<n>`, e.g.
/// `NN12_BEG7`, `EE2_E_END3`, `SS1_W_BEG0`. DIR is NN/SS/EE/WW, LEN
/// is 1, 2, 4, or 12. Returns (direction unit, span, side suffix,
/// is_begin, index) on a match.
fn parse_span_wire(name: &str) -> Option<((i32, i32), i32, &str, bool, &str)> {
    let (head, kind_idx) = match name.find("_BEG") {
        Some(i) => (&name[..i], Some((i, true))),
        None => match name.find("_END") {
            Some(i) => (&name[..i], Some((i, false))),
            None => (name, None),
        },
    };
    let (i, is_beg) = kind_idx?;
    let idx = &name[i + 4..];
    if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Head is DIR + LEN with an optional _E / _W side suffix.
    let (dir_len, side) = match head.rsplit_once('_') {
        Some((d, s)) if s == "E" || s == "W" => (d, s),
        _ => (head, ""),
    };
    let (dir, len) = match dir_len {
        "NN1" => ((0, 1), 1),
        "NN2" => ((0, 1), 2),
        "NN4" => ((0, 1), 4),
        "NN12" => ((0, 1), 12),
        "SS1" => ((0, -1), 1),
        "SS2" => ((0, -1), 2),
        "SS4" => ((0, -1), 4),
        "SS12" => ((0, -1), 12),
        "EE1" => ((1, 0), 1),
        "EE2" => ((1, 0), 2),
        "EE4" => ((1, 0), 4),
        "EE12" => ((1, 0), 12),
        "WW1" => ((-1, 0), 1),
        "WW2" => ((-1, 0), 2),
        "WW4" => ((-1, 0), 4),
        "WW12" => ((-1, 0), 12),
        _ => return None,
    };
    Some((dir, len, side, is_beg, idx))
}

/// The partner wire of a span wire: same base, BEG swapped for END or
/// vice versa, plus the tile delta from this wire's tile to the
/// partner's tile, plus whether this wire is the BEG end. `None` when
/// the name is not a span wire.
fn continuation_partner(name: &str) -> Option<(String, (i32, i32), bool)> {
    let ((dx, dy), len, side, is_beg, idx) = parse_span_wire(name)?;
    // Rebuild the base from the parsed parts so the partner name is
    // exact: DIR + LEN + optional _E/_W + BEG/END + index.
    let base = format!(
        "{}{}{}",
        &name[..2],
        len,
        if side.is_empty() {
            String::new()
        } else {
            format!("_{}", side)
        }
    );
    let partner = if is_beg {
        format!("{}_END{}", base, idx)
    } else {
        format!("{}_BEG{}", base, idx)
    };
    let delta = if is_beg {
        (dx * len, dy * len)
    } else {
        (-dx * len, -dy * len)
    };
    Some((partner, delta, is_beg))
}

/// Routing graph over INT tiles, built from the vendored INT tile JSON.
pub struct IntTileWireDb {
    tiles: Vec<IntTile>,
    tile_by_name: HashMap<String, usize>,
    /// Wire names in sorted order; the wire index is the position here.
    wire_names: Vec<String>,
    wire_index: HashMap<String, usize>,
    fanout: Vec<Vec<WireId>>,
    /// (src wire idx, dst wire idx) to true when the PIP is
    /// bidirectional. Keyed by tile-type wire indices, shared by all
    /// tile instances. Directional PIPs map to false.
    pip_table: HashMap<(u32, u32), bool>,
    pin_wires: HashMap<(i32, i32, String, String), WireId>,
}

impl IntTileWireDb {
    /// Build the graph from INT tile JSON text and the tile instances
    /// to instantiate. The JSON must be the vendored
    /// `tile_type_INT.json` (or the same shape); any other tile type,
    /// any pseudo PIP, or any PIP naming an unknown wire fails loud.
    pub fn from_json(json: &str, tiles: &[IntTile]) -> Result<Self, WireDbError> {
        if tiles.is_empty() {
            return Err(WireDbError::EmptyTiles);
        }
        let mut tile_by_name: HashMap<String, usize> = HashMap::new();
        let mut tile_by_xy: HashMap<(i32, i32), usize> = HashMap::new();
        for (i, t) in tiles.iter().enumerate() {
            if tile_by_name.insert(t.name.clone(), i).is_some()
                || tile_by_xy.insert((t.x, t.y), i).is_some()
            {
                return Err(WireDbError::DuplicateTile(format!(
                    "{} at ({},{})",
                    t.name, t.x, t.y
                )));
            }
        }
        let parsed: IntTileJson =
            serde_json::from_str(json).map_err(|e| WireDbError::Json(e.to_string()))?;
        if parsed.tile_type != "INT" {
            return Err(WireDbError::WrongTileType(parsed.tile_type));
        }
        let mut wire_names: Vec<String> = parsed.wires.keys().cloned().collect();
        wire_names.sort();
        let wire_index: HashMap<String, usize> = wire_names
            .iter()
            .enumerate()
            .map(|(i, w)| (w.clone(), i))
            .collect();
        let wires_per_tile = wire_names.len();
        let mut pip_table: HashMap<(u32, u32), bool> = HashMap::new();
        for (key, pip) in &parsed.pips {
            if pip.is_pseudo != "0" {
                return Err(WireDbError::PseudoPip(key.clone()));
            }
            let src =
                wire_index
                    .get(&pip.src_wire)
                    .ok_or_else(|| WireDbError::UnknownWireInPip {
                        pip: key.clone(),
                        wire: pip.src_wire.clone(),
                    })?;
            let dst =
                wire_index
                    .get(&pip.dst_wire)
                    .ok_or_else(|| WireDbError::UnknownWireInPip {
                        pip: key.clone(),
                        wire: pip.dst_wire.clone(),
                    })?;
            pip_table.insert((*src as u32, *dst as u32), pip.is_directional == "0");
        }
        let node_count = tiles.len() * wires_per_tile;
        let mut fanout: Vec<Vec<WireId>> = vec![Vec::new(); node_count];
        let node = |tile: usize, wire: usize| (tile * wires_per_tile + wire) as WireId;
        // Intra-tile PIP edges.
        for (i, _) in tiles.iter().enumerate() {
            for ((src, dst), bidir) in &pip_table {
                fanout[node(i, *src as usize)].push(node(i, *dst as usize));
                if *bidir {
                    fanout[node(i, *dst as usize)].push(node(i, *src as usize));
                }
            }
        }
        // Inter-tile continuation edges, one per BEG wire per tile.
        for (i, tile) in tiles.iter().enumerate() {
            for (w, name) in wire_names.iter().enumerate() {
                let Some((partner, (dx, dy), is_beg)) = continuation_partner(name) else {
                    continue;
                };
                // Only the BEG end creates the edge; the END end is
                // reached from its own BEG partner tile.
                if !is_beg {
                    continue;
                }
                let Some(&j) = tile_by_xy.get(&(tile.x + dx, tile.y + dy)) else {
                    continue;
                };
                let Some(&partner_idx) = wire_index.get(&partner) else {
                    continue;
                };
                let a = node(i, w);
                let b = node(j, partner_idx);
                fanout[a as usize].push(b);
                fanout[b as usize].push(a);
            }
        }
        Ok(Self {
            tiles: tiles.to_vec(),
            tile_by_name,
            wire_names,
            wire_index,
            fanout,
            pip_table,
            pin_wires: HashMap::new(),
        })
    }

    /// One tile instance.
    pub fn single_tile(name: &str, x: i32, y: i32, json: &str) -> Result<Self, WireDbError> {
        Self::from_json(json, &[IntTile::new(name, x, y)])
    }

    /// Number of tile instances in the graph.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Wires per tile instance (1,475 for the vendored INT tile JSON).
    pub fn wires_per_tile(&self) -> usize {
        self.wire_names.len()
    }

    /// PIPs per tile instance (3,774 for the vendored INT tile JSON).
    pub fn pip_count(&self) -> usize {
        self.pip_table.len()
    }

    /// Wire ID for a (tile, wire name) pair, or `None` when either is
    /// unknown.
    pub fn wire_id(&self, tile: &str, wire: &str) -> Option<WireId> {
        let t = self.tile_by_name.get(tile)?;
        let w = self.wire_index.get(wire)?;
        Some((t * self.wire_names.len() + w) as WireId)
    }

    /// Tile index a wire belongs to.
    fn tile_of(&self, wire: WireId) -> Option<usize> {
        let t = wire as usize / self.wire_names.len();
        if t < self.tiles.len() {
            Some(t)
        } else {
            None
        }
    }

    /// Wire index within its tile.
    fn wire_idx_of(&self, wire: WireId) -> Option<usize> {
        self.tile_of(wire)?;
        Some(wire as usize % self.wire_names.len())
    }

    /// Register a BEL pin to wire mapping. This is caller-supplied
    /// data: no public source maps BEL pins to INT wires, so the DB
    /// cannot derive it. Fails loud on an unknown tile or wire.
    pub fn add_pin_wire(
        &mut self,
        site: UltraSite,
        bel: &str,
        pin: &str,
        tile: &str,
        wire: &str,
    ) -> Result<(), WireDbError> {
        let id = self
            .wire_id(tile, wire)
            .ok_or_else(|| WireDbError::UnknownPinWire {
                tile: tile.to_string(),
                wire: wire.to_string(),
            })?;
        self.pin_wires
            .insert((site.x, site.y, bel.to_string(), pin.to_string()), id);
        Ok(())
    }

    /// Translate one ordered route (driver to sink, physical
    /// direction) to INT PIPs, resolving bidirectional PIPs from this
    /// DB's PIP table. See [`crate::route_to_pip::translate_route`].
    pub fn translate_route(
        &self,
        route: &[WireId],
        design: &str,
    ) -> Result<crate::route_to_pip::TranslatedRoute, crate::route_to_pip::RouteToPipError> {
        crate::route_to_pip::translate_route_with(
            self,
            route,
            design,
            Some(&|a, b| self.pip_bidir(a, b)),
        )
    }

    /// Traversal direction of a bidirectional PIP between two wires in
    /// the same tile, relative to the tile JSON's src to dst order:
    /// `Fwd` when the route pair (a, b) is in tile src to dst order,
    /// `Rev` when it is the reverse. `None` when the pair is not a
    /// bidirectional PIP (directional PIPs and non-edges alike).
    pub fn pip_bidir(&self, a: WireId, b: WireId) -> Option<BidirDir> {
        let ta = self.tile_of(a)?;
        let tb = self.tile_of(b)?;
        if ta != tb {
            return None;
        }
        let ia = self.wire_idx_of(a)? as u32;
        let ib = self.wire_idx_of(b)? as u32;
        if self.pip_table.get(&(ia, ib)) == Some(&true) {
            Some(BidirDir::Fwd)
        } else if self.pip_table.get(&(ib, ia)) == Some(&true) {
            Some(BidirDir::Rev)
        } else {
            None
        }
    }

    /// True when the consecutive pair (a, b) is a span-wire
    /// continuation (a BEG wire in one tile to its END partner in the
    /// tile LEN steps away, or the reverse). Used by the translator to
    /// tell PIPs apart from continuations.
    pub fn is_continuation(&self, a: WireId, b: WireId) -> bool {
        let (Some(ta), Some(tb)) = (self.tile_of(a), self.tile_of(b)) else {
            return false;
        };
        if ta == tb {
            return false;
        }
        let Some(na) = self.wire_idx_of(a).and_then(|i| self.wire_names.get(i)) else {
            return false;
        };
        let Some(nb) = self.wire_idx_of(b).and_then(|i| self.wire_names.get(i)) else {
            return false;
        };
        let Some((partner, (dx, dy), _)) = continuation_partner(na) else {
            return false;
        };
        if partner != *nb {
            return false;
        }
        let tile_a = &self.tiles[ta];
        let tile_b = &self.tiles[tb];
        tile_b.x - tile_a.x == dx && tile_b.y - tile_a.y == dy
    }
}

impl WireDb for IntTileWireDb {
    fn wire_count(&self) -> usize {
        self.tiles.len() * self.wire_names.len()
    }

    fn fanout(&self, wire: WireId) -> &[WireId] {
        // Callers (router, translator) only pass wire IDs this DB
        // handed out or that passed `wire_info` first. An out-of-range
        // ID is a caller bug; fail loud instead of indexing blindly.
        match self.fanout.get(wire as usize) {
            Some(f) => f,
            None => &[],
        }
    }

    fn wire_xy(&self, wire: WireId) -> (i32, i32) {
        match self.tile_of(wire) {
            Some(t) => (self.tiles[t].x, self.tiles[t].y),
            None => (0, 0),
        }
    }

    fn wire_for_bel_pin(&self, site: UltraSite, bel: &str, pin: &str) -> Option<WireId> {
        self.pin_wires
            .get(&(site.x, site.y, bel.to_string(), pin.to_string()))
            .copied()
    }

    fn wire_info(&self, wire: WireId) -> Option<WireInfo<'_>> {
        let t = self.tile_of(wire)?;
        let w = self.wire_idx_of(wire)?;
        Some(WireInfo {
            name: &self.wire_names[w],
            tile: &self.tiles[t].name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vendored INT tile JSON (BitFiltrator, MIT).
    const TILE_JSON: &str = include_str!("../data/tile_type_INT.json");

    fn one_tile() -> IntTileWireDb {
        IntTileWireDb::single_tile("INT_X12Y45", 12, 45, TILE_JSON).unwrap()
    }

    fn two_tiles_north() -> IntTileWireDb {
        IntTileWireDb::from_json(
            TILE_JSON,
            &[
                IntTile::new("INT_X12Y45", 12, 45),
                IntTile::new("INT_X12Y57", 12, 57),
            ],
        )
        .unwrap()
    }

    #[test]
    fn tile_json_loads_with_audit_counts() {
        let db = one_tile();
        assert_eq!(db.tile_count(), 1);
        assert_eq!(db.wires_per_tile(), 1475);
        assert_eq!(db.pip_count(), 3774);
        assert_eq!(db.wire_count(), 1475);
    }

    #[test]
    fn wire_info_names_every_wire_and_rejects_out_of_range() {
        let db = one_tile();
        let w = db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap();
        assert_eq!(
            db.wire_info(w),
            Some(WireInfo {
                name: "BOUNCE_E_0_FT1",
                tile: "INT_X12Y45"
            })
        );
        assert_eq!(db.wire_info(1475), None);
        assert_eq!(db.wire_info(u32::MAX), None);
        assert_eq!(db.wire_id("INT_X12Y45", "NOPE"), None);
        assert_eq!(db.wire_id("INT_X99Y99", "BOUNCE_E_0_FT1"), None);
    }

    #[test]
    fn intra_tile_pip_edges_follow_physical_direction() {
        let db = one_tile();
        let inode = db.wire_id("INT_X12Y45", "INODE_E_1_FT1").unwrap();
        let bounce = db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap();
        // PIP INODE_E_1_FT1 -> BOUNCE_E_0_FT1 (the golden blinky PIP).
        assert!(db.fanout(inode).contains(&bounce));
        assert!(!db.fanout(bounce).contains(&inode));
        // LOGIC_OUTS_E0 drives INODE_E_1_FT1 directly.
        let logic = db.wire_id("INT_X12Y45", "LOGIC_OUTS_E0").unwrap();
        assert!(db.fanout(logic).contains(&inode));
    }

    #[test]
    fn continuation_pairs_beg_to_end_across_span() {
        let db = two_tiles_north();
        // NN12_BEG7 in tile (12,45) continues as NN12_END7 in (12,57).
        let beg = db.wire_id("INT_X12Y45", "NN12_BEG7").unwrap();
        let end = db.wire_id("INT_X12Y57", "NN12_END7").unwrap();
        assert!(db.fanout(beg).contains(&end));
        assert!(db.fanout(end).contains(&beg));
        assert!(db.is_continuation(beg, end));
        assert!(db.is_continuation(end, beg));
        // A BEG wire has no same-tile fanout: it is driven only by
        // local PIPs and continues out of the tile.
        let beg_same = db
            .fanout(beg)
            .iter()
            .all(|w| db.wire_info(*w).unwrap().tile == "INT_X12Y57");
        assert!(beg_same);
        // NN12_END7 in (12,45) is fed by NN12_BEG7 in (12,33), which
        // is not instantiated, so it has no continuation edge here.
        let end45 = db.wire_id("INT_X12Y45", "NN12_END7").unwrap();
        assert!(!db.fanout(end45).contains(&beg));
        // Short spans: EE2_E_BEG3 in (12,45) needs (14,45), absent.
        let ee = db.wire_id("INT_X12Y45", "EE2_E_BEG3").unwrap();
        assert!(db.fanout(ee).is_empty());
    }

    #[test]
    fn continuation_partner_parses_every_span_wire() {
        let db = one_tile();
        let mut beg = 0;
        let mut end = 0;
        for name in &db.wire_names {
            if let Some((partner, _, _)) = continuation_partner(name) {
                // Every span wire has a partner of the same form in
                // the same wire list.
                assert!(
                    db.wire_index.contains_key(&partner),
                    "{} partner {} missing",
                    name,
                    partner
                );
                if name.contains("_BEG") {
                    beg += 1;
                } else {
                    end += 1;
                }
            }
        }
        assert_eq!(beg, 208);
        assert_eq!(end, 208);
        // Non-span wires have no partner.
        assert_eq!(continuation_partner("BOUNCE_E_0_FT1"), None);
        assert_eq!(continuation_partner("VCC_WIRE"), None);
        assert_eq!(continuation_partner("INT_NODE_IMUX_5_INT_OUT0"), None);
    }

    #[test]
    fn bidir_pip_direction_is_known() {
        let db = one_tile();
        let imux = db
            .wire_id("INT_X12Y45", "INT_NODE_IMUX_5_INT_OUT0")
            .unwrap();
        let bypass = db.wire_id("INT_X12Y45", "BYPASS_E8").unwrap();
        // Tile-canonical direction is IMUX -> BYPASS.
        assert_eq!(db.pip_bidir(imux, bypass), Some(BidirDir::Fwd));
        assert_eq!(db.pip_bidir(bypass, imux), Some(BidirDir::Rev));
        // A directional PIP is not bidirectional either way.
        let inode = db.wire_id("INT_X12Y45", "INODE_E_1_FT1").unwrap();
        let bounce = db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").unwrap();
        assert_eq!(db.pip_bidir(inode, bounce), None);
        assert_eq!(db.pip_bidir(bounce, inode), None);
    }

    #[test]
    fn pin_mapping_is_caller_supplied_and_fails_loud() {
        let mut db = one_tile();
        let site = UltraSite {
            x: 12,
            y: 45,
            kind: 0,
        };
        // No mapping registered: the router must fail loud, not guess.
        assert_eq!(db.wire_for_bel_pin(site, "A6LUT", "O6"), None);
        db.add_pin_wire(site, "A6LUT", "O6", "INT_X12Y45", "LOGIC_OUTS_E0")
            .unwrap();
        let w = db.wire_id("INT_X12Y45", "LOGIC_OUTS_E0").unwrap();
        assert_eq!(db.wire_for_bel_pin(site, "A6LUT", "O6"), Some(w));
        // Unknown tile or wire fails loud at registration.
        assert!(matches!(
            db.add_pin_wire(site, "A6LUT", "O6", "INT_X99Y99", "LOGIC_OUTS_E0"),
            Err(WireDbError::UnknownPinWire { .. })
        ));
        assert!(matches!(
            db.add_pin_wire(site, "A6LUT", "O6", "INT_X12Y45", "NOPE"),
            Err(WireDbError::UnknownPinWire { .. })
        ));
    }

    #[test]
    fn bad_tile_json_fails_loud() {
        assert!(matches!(
            IntTileWireDb::from_json("not json", &[IntTile::new("INT_X0Y0", 0, 0)]),
            Err(WireDbError::Json(_))
        ));
        assert!(matches!(
            IntTileWireDb::from_json(
                r#"{"tile_type": "CLB", "pips": {}, "wires": {}}"#,
                &[IntTile::new("INT_X0Y0", 0, 0)]
            ),
            Err(WireDbError::WrongTileType(_))
        ));
        assert!(matches!(
            IntTileWireDb::from_json(r#"{"tile_type": "INT", "pips": {}, "wires": {}}"#, &[]),
            Err(WireDbError::EmptyTiles)
        ));
        assert!(matches!(
            IntTileWireDb::from_json(
                TILE_JSON,
                &[
                    IntTile::new("INT_X0Y0", 0, 0),
                    IntTile::new("INT_X0Y0", 0, 0)
                ]
            ),
            Err(WireDbError::DuplicateTile(_))
        ));
        // A PIP naming an unknown wire fails loud.
        let bad = r#"{"tile_type": "INT", "pips": {"INT.A->>B": {"src_wire": "A", "dst_wire": "B", "is_directional": "1", "is_pseudo": "0"}}, "wires": {"A": null}}"#;
        assert!(matches!(
            IntTileWireDb::from_json(bad, &[IntTile::new("INT_X0Y0", 0, 0)]),
            Err(WireDbError::UnknownWireInPip { .. })
        ));
        // A pseudo PIP fails loud: it is not a real mux.
        let pseudo = r#"{"tile_type": "INT", "pips": {"INT.A->>B": {"src_wire": "A", "dst_wire": "B", "is_directional": "1", "is_pseudo": "1"}}, "wires": {"A": null, "B": null}}"#;
        assert!(matches!(
            IntTileWireDb::from_json(pseudo, &[IntTile::new("INT_X0Y0", 0, 0)]),
            Err(WireDbError::PseudoPip(_))
        ));
    }
}
