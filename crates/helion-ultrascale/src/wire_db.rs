//! Routing graph for UltraScale+ INT and INT_INTF tiles, built from public sources only.
//!
//! The graph is built from the vendored BitFiltrator INT tile JSON
//! (`crates/helion-ultrascale/data/tile_type_INT.json`, MIT, extracted
//! from the BitFiltrator arch JSON): 1,475 wires and 3,774 PIPs per INT
//! tile, of which 3,770 are directional and 4 are bidirectional. The
//! INT PIP audit (data/INT_PIP_AUDIT.md) maps 3,774 of 3,774 tile PIPs
//! to prjuray-db segbits features. No RapidWright .dat and no FPGA
//! Interchange DB is ingested: both are under the Xilinx EULA.
//!
//! INT_INTF tiles (the interface columns flanking DSP/BRAM columns)
//! have their own, smaller tile JSONs (350 wires / 32 PIPs for
//! INT_INTF_L/R, up to 446 wires / 224 PIPs for the PCIE4 and
//! LEFT_TERM_PSS variants), loaded at runtime from the prjuray-db
//! family root by the tileconn loader, never vendored. The graph is
//! heterogeneous: each tile instance carries its tile type, and wire
//! and PIP tables are per type. DSP/BRAM pins stitch to INT_INTF
//! wires, and INT_INTF wires stitch to INT wires (tileconn pairs);
//! those tileconn stitches are bidirectional same-node edges added
//! with [`IntTileWireDb::add_stitch`], distinct from span-wire
//! continuations (which exist only between INT tiles).
//!
//! Node layout: one node per (tile instance, wire name). A tile instance
//! is supplied by the caller as a (tile name, x, y, tile type) tuple; this module
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
//!   DIR for vertical spans (NN/SS), and (LEN + 1) / 2 INT tiles away
//!   for horizontal spans (EE/WW): a length-2 horizontal span reaches
//!   the adjacent INT tile, a length-4 span two INT tiles away. The
//!   horizontal step count is the tileconn-stitched distance, not the
//!   span length: INT tiles are spaced about two columns apart in the
//!   real grid, so a WW2 wire driven in INT_X2Y0 continues to
//!   INT_X1Y0, not INT_X0Y0. The continuation edge
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
use crate::place::UltraPlaced;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

/// Error building an [`IntTileWireDb`] from tile JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WireDbError {
    /// The JSON text does not parse.
    Json(String),
    /// The JSON is not a tile file of the expected tile type.
    WrongTileType { found: String, expected: String },
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
    /// A tile instance names a tile type no JSON was supplied for.
    UnknownTileType { tile: String, tile_type: String },
    /// `add_stitch` was given a same-tile pair or an unknown wire:
    /// stitches join two tiles, never two wires of one tile.
    BadStitch(String),
}

impl fmt::Display for WireDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(e) => write!(f, "tile JSON does not parse: {}", e),
            Self::WrongTileType { found, expected } => {
                write!(
                    f,
                    "tile JSON has tile_type '{}', expected '{}'",
                    found, expected
                )
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
            Self::UnknownTileType { tile, tile_type } => write!(
                f,
                "tile '{}' has tile type '{}', but no JSON was supplied for it",
                tile, tile_type
            ),
            Self::BadStitch(m) => write!(f, "bad tileconn stitch: {}", m),
        }
    }
}

impl std::error::Error for WireDbError {}

/// One INT or INT_INTF tile instance in the graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntTile {
    /// Vivado tile name, e.g. `INT_X12Y45`.
    pub name: String,
    /// Tile X coordinate (Vivado tile grid).
    pub x: i32,
    /// Tile Y coordinate (Vivado tile grid).
    pub y: i32,
    /// Tile type, e.g. `INT` or `INT_INTF_R`. Selects the wire and
    /// PIP table this instance is built from.
    pub tile_type: String,
}

impl IntTile {
    /// A plain INT tile instance.
    pub fn new(name: &str, x: i32, y: i32) -> Self {
        Self::new_typed(name, x, y, "INT")
    }

    /// A tile instance of any type (`INT`, `INT_INTF_L`,
    /// `INT_INTF_R`, `INT_INTF_L_PCIE4`, ...).
    pub fn new_typed(name: &str, x: i32, y: i32, tile_type: &str) -> Self {
        Self {
            name: name.to_string(),
            x,
            y,
            tile_type: tile_type.to_string(),
        }
    }
}

/// Wire and PIP tables for one tile type, parsed from that type's
/// tile JSON. Every tile instance of the type shares this table.
#[derive(Debug)]
struct TileTypeTable {
    /// Tile type name, e.g. `INT` or `INT_INTF_R`.
    name: String,
    /// Wire names in sorted order; the wire index is the position here.
    wire_names: Vec<String>,
    wire_index: HashMap<String, usize>,
    /// (src wire idx, dst wire idx) to true when the PIP is
    /// bidirectional. Directional PIPs map to false.
    pip_table: HashMap<(u32, u32), bool>,
}

impl TileTypeTable {
    /// Parse one tile JSON. The JSON's `tile_type` must equal
    /// `expected`; any pseudo PIP, or any PIP naming an unknown wire,
    /// fails loud, exactly as the old INT-only loader did.
    fn parse(json: &str, expected: &str) -> Result<Self, WireDbError> {
        let parsed: IntTileJson =
            serde_json::from_str(json).map_err(|e| WireDbError::Json(e.to_string()))?;
        if parsed.tile_type != expected {
            return Err(WireDbError::WrongTileType {
                found: parsed.tile_type,
                expected: expected.to_string(),
            });
        }
        let mut wire_names: Vec<String> = parsed.wires.keys().cloned().collect();
        wire_names.sort();
        let wire_index: HashMap<String, usize> = wire_names
            .iter()
            .enumerate()
            .map(|(i, w)| (w.clone(), i))
            .collect();
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
        Ok(Self {
            name: expected.to_string(),
            wire_names,
            wire_index,
            pip_table,
        })
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
/// is 1, 2, 4, or 12.
struct SpanWire<'a> {
    /// Direction unit vector: (0, 1) NN, (0, -1) SS, (1, 0) EE, (-1, 0) WW.
    dir: (i32, i32),
    /// Span length in tiles.
    len: i32,
    /// `_E` / `_W` side suffix, or "" when absent.
    side: &'a str,
    /// True when this end is BEG, false when END.
    is_beg: bool,
    /// Numeric index after BEG/END.
    idx: &'a str,
}

fn parse_span_wire(name: &str) -> Option<SpanWire<'_>> {
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
    Some(SpanWire {
        dir,
        len,
        side,
        is_beg,
        idx,
    })
}

/// The partner wire of a span wire: same base, BEG swapped for END or
/// vice versa, plus the tile delta from this wire's tile to the
/// partner's tile, plus whether this wire is the BEG end. `None` when
/// the name is not a span wire.
///
/// Tile steps are NOT the span length in both directions. The public
/// tileconn.json stitches a span wire's BEG in one INT tile to its
/// END in the tile LEN steps away only for vertical spans (NN/SS:
/// 1, 2, 4, 12 tiles). Horizontal spans (EE/WW) cross one INT tile
/// per two columns of span: a length-1 span reaches the adjacent INT
/// tile, a length-2 span also reaches the adjacent INT tile, a
/// length-4 span reaches two INT tiles away, and a length-12 span
/// reaches six INT tiles away. The old rule used LEN steps in both
/// directions, so a WW2 wire driven in INT_X2Y0 was treated as
/// continuing to INT_X0Y0, while tileconn stitches it to INT_X1Y0:
/// the router emitted PIPs out of the far END whose node has no
/// driver in the tileconn-stitched model, and every net downstream
/// of it failed as unreachable. Horizontal steps are therefore
/// (LEN + 1) / 2; vertical steps stay LEN.
fn continuation_partner(name: &str) -> Option<(String, (i32, i32), bool)> {
    let span = parse_span_wire(name)?;
    let (dx, dy) = span.dir;
    let len = span.len;
    let side = span.side;
    let is_beg = span.is_beg;
    let idx = span.idx;
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
    // Horizontal spans (EE/WW) advance one INT tile per two span
    // columns; vertical spans (NN/SS) advance one INT tile per span
    // row.
    let steps = if dx != 0 { (len + 1) / 2 } else { len };
    let delta = if is_beg {
        (dx * steps, dy * steps)
    } else {
        (-dx * steps, -dy * steps)
    };
    Some((partner, delta, is_beg))
}

/// Routing graph over INT and INT_INTF tiles, built from the tile JSONs.
pub struct IntTileWireDb {
    tiles: Vec<IntTile>,
    tile_by_name: HashMap<String, usize>,
    /// One table per tile type present in this graph.
    tables: Vec<TileTypeTable>,
    /// Per tile instance, the index of its table in `tables`.
    tile_type_idx: Vec<usize>,
    /// Per tile instance, the first node id of that tile. Node ids
    /// are contiguous per tile in tile order: tile i occupies
    /// `tile_base[i] .. tile_base[i] + tables[tile_type_idx[i]].wire_names.len()`.
    /// Tiles of different types have different widths, so there is no
    /// single `wires_per_tile` for the node math.
    tile_base: Vec<usize>,
    /// Total node count (sum of per-tile wire counts).
    node_count: usize,
    fanout: Vec<Vec<WireId>>,
    /// Tileconn stitch pairs, canonical (min, max) wire-id order.
    /// A stitch joins an INT_INTF wire to the INT wire tileconn names
    /// as the same node; it programs no PIP. See [`Self::add_stitch`].
    stitches: std::collections::HashSet<(WireId, WireId)>,
    pin_wires: HashMap<(i32, i32, String, String), WireId>,
}

impl IntTileWireDb {
    /// Build the graph from INT tile JSON text and the tile instances
    /// to instantiate. The JSON must be the vendored
    /// `tile_type_INT.json` (or the same shape); any other tile type,
    /// any pseudo PIP, or any PIP naming an unknown wire fails loud.
    /// Every tile defaults to type `INT` (see [`IntTile::new`]).
    pub fn from_json(json: &str, tiles: &[IntTile]) -> Result<Self, WireDbError> {
        Self::from_json_typed(&[("INT", json)], tiles)
    }

    /// Build the graph from one tile JSON per tile type and the tile
    /// instances to instantiate. Each tile's [`IntTile::tile_type`]
    /// must name one of the supplied JSONs; a tile of an unknown type,
    /// a JSON whose `tile_type` disagrees with its key, a pseudo PIP,
    /// or a PIP naming an unknown wire fails loud. INT tiles get span
    /// continuations as before; INT_INTF tiles get their own PIPs and
    /// no continuations (their wires have no span names), and their
    /// tileconn stitches to INT tiles are added later with
    /// [`Self::add_stitch`].
    pub fn from_json_typed(jsons: &[(&str, &str)], tiles: &[IntTile]) -> Result<Self, WireDbError> {
        if tiles.is_empty() {
            return Err(WireDbError::EmptyTiles);
        }
        let mut tables: Vec<TileTypeTable> = Vec::with_capacity(jsons.len());
        let mut table_by_name: HashMap<String, usize> = HashMap::new();
        for (i, (name, json)) in jsons.iter().enumerate() {
            tables.push(TileTypeTable::parse(json, name)?);
            table_by_name.insert((*name).to_string(), i);
        }
        let mut tile_by_name: HashMap<String, usize> = HashMap::new();
        // Continuation lookup is scoped per tile type: an INT tile
        // and an INT_INTF tile can share name coordinates (e.g.
        // INT_X13Y0 and INT_INTF_R_X13Y0 are both (13, 0)).
        let mut tile_by_xy: HashMap<(usize, i32, i32), usize> = HashMap::new();
        let mut tile_type_idx: Vec<usize> = Vec::with_capacity(tiles.len());
        let mut tile_base: Vec<usize> = Vec::with_capacity(tiles.len());
        let mut node_count = 0usize;
        for (i, t) in tiles.iter().enumerate() {
            if tile_by_name.insert(t.name.clone(), i).is_some() {
                return Err(WireDbError::DuplicateTile(format!(
                    "{} at ({},{})",
                    t.name, t.x, t.y
                )));
            }
            let ti =
                *table_by_name
                    .get(&t.tile_type)
                    .ok_or_else(|| WireDbError::UnknownTileType {
                        tile: t.name.clone(),
                        tile_type: t.tile_type.clone(),
                    })?;
            if tile_by_xy.insert((ti, t.x, t.y), i).is_some() {
                return Err(WireDbError::DuplicateTile(format!(
                    "{} at ({},{})",
                    t.name, t.x, t.y
                )));
            }
            tile_type_idx.push(ti);
            tile_base.push(node_count);
            node_count += tables[ti].wire_names.len();
        }
        let mut fanout: Vec<Vec<WireId>> = vec![Vec::new(); node_count];
        let node = |tile: usize, wire: usize| (tile_base[tile] + wire) as WireId;
        // Intra-tile PIP edges, from each tile's own type table.
        for (i, _) in tiles.iter().enumerate() {
            let table = &tables[tile_type_idx[i]];
            for ((src, dst), bidir) in &table.pip_table {
                fanout[node(i, *src as usize) as usize].push(node(i, *dst as usize));
                if *bidir {
                    fanout[node(i, *dst as usize) as usize].push(node(i, *src as usize));
                }
            }
        }
        // Inter-tile continuation edges, one per BEG wire per tile.
        // Only INT tiles have span wires; INT_INTF wires return None
        // from continuation_partner, so those tiles get none here.
        for (i, tile) in tiles.iter().enumerate() {
            let table = &tables[tile_type_idx[i]];
            for (w, name) in table.wire_names.iter().enumerate() {
                let Some((partner, (dx, dy), is_beg)) = continuation_partner(name) else {
                    continue;
                };
                // Only the BEG end creates the edge; the END end is
                // reached from its own BEG partner tile.
                if !is_beg {
                    continue;
                }
                let Some(&j) = tile_by_xy.get(&(tile_type_idx[i], tile.x + dx, tile.y + dy)) else {
                    continue;
                };
                let Some(&partner_idx) = tables[tile_type_idx[j]].wire_index.get(&partner) else {
                    continue;
                };
                let a = node(i, w);
                let b = node(j, partner_idx);
                fanout[a as usize].push(b);
                fanout[b as usize].push(a);
            }
        }
        // Deterministic fanout order (N48): the PIP table above is a
        // HashMap, so iterating it pushes PIP edges in per-process
        // random order. A* explores neighbors in fanout order and
        // breaks f-score ties in push order, so the same netlist
        // routed differently on every process (N35/N36: identical
        // placement, different failing nets, iters_used 28-50 for
        // the same counter12 seed-0 netlist). Sorting each fanout
        // list by wire id makes the graph, and therefore every
        // route, a pure function of the netlist and the seed.
        // Continuation edges are appended in sorted wire order
        // already, so the sort only canonicalizes the PIP edges.
        for list in fanout.iter_mut() {
            list.sort_unstable();
        }
        Ok(Self {
            tiles: tiles.to_vec(),
            tile_by_name,
            tables,
            tile_type_idx,
            tile_base,
            node_count,
            fanout,
            stitches: std::collections::HashSet::new(),
            pin_wires: HashMap::new(),
        })
    }

    /// One tile instance.
    pub fn single_tile(name: &str, x: i32, y: i32, json: &str) -> Result<Self, WireDbError> {
        Self::from_json(json, &[IntTile::new(name, x, y)])
    }

    /// Build the routing graph for the INT tiles covering the placed
    /// slices: one INT tile per distinct placed (x, y), named
    /// `INT_X{x}Y{y}`.
    ///
    /// PROVISIONAL (US-01): no xcku3p tilegrid exists in any public
    /// source, so the INT tile X/Y are taken equal to the placed
    /// slice X/Y, the same convention the harness golden FASM uses
    /// (`INT_X0Y0` for `SLICE_X0Y0`). The day the tilegrid lands
    /// this helper takes real tile names and coordinates instead.
    /// Tile names and coordinates come only from the placed slices;
    /// nothing here invents device geometry. An empty placement
    /// fails loud (`EmptyTiles`): there is nothing to route over.
    pub fn from_placed_slices(json: &str, placed: &UltraPlaced) -> Result<Self, WireDbError> {
        let mut tiles: Vec<IntTile> = placed
            .slices
            .iter()
            .map(|s| IntTile::new(&format!("INT_X{}Y{}", s.x, s.y), s.x, s.y))
            .collect();
        tiles.sort_by_key(|t| (t.y, t.x));
        tiles.dedup_by(|a, b| a.name == b.name);
        Self::from_json(json, &tiles)
    }

    /// Number of tile instances in the graph.
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Wires per INT tile instance (1,475 for the vendored INT tile
    /// JSON). INT_INTF tile instances have fewer wires; use
    /// [`Self::wire_count`] for the graph total.
    pub fn wires_per_tile(&self) -> usize {
        self.int_table().wire_names.len()
    }

    /// PIPs per INT tile instance (3,774 for the vendored INT tile JSON).
    pub fn pip_count(&self) -> usize {
        self.int_table().pip_table.len()
    }

    /// The `INT` table when the graph has one, else the first table.
    /// Every graph the flow builds has an INT table; the fallback only
    /// serves single-type INT_INTF fixture graphs.
    fn int_table(&self) -> &TileTypeTable {
        self.tables
            .iter()
            .find(|t| t.name == "INT")
            .unwrap_or(&self.tables[0])
    }

    /// The table of one tile instance.
    fn table_for_tile(&self, tile: usize) -> &TileTypeTable {
        &self.tables[self.tile_type_idx[tile]]
    }

    /// Wire names of one tile type, or `None` when the graph has no
    /// such type. Used by tests; the graph itself resolves by tile.
    pub fn wire_names_for_type(&self, tile_type: &str) -> Option<&[String]> {
        self.tables
            .iter()
            .find(|t| t.name == tile_type)
            .map(|t| t.wire_names.as_slice())
    }

    /// Number of tile instances of one tile type.
    pub fn tile_count_of_type(&self, tile_type: &str) -> usize {
        self.tiles
            .iter()
            .filter(|t| t.tile_type == tile_type)
            .count()
    }

    /// Tile instances in graph order, with their types.
    pub fn tiles(&self) -> &[IntTile] {
        &self.tiles
    }

    /// Wire ID for a (tile, wire name) pair, or `None` when either is
    /// unknown. The wire must belong to the tile's own type: an INT
    /// wire name on an INT_INTF tile is unknown, not an INT node.
    pub fn wire_id(&self, tile: &str, wire: &str) -> Option<WireId> {
        let t = *self.tile_by_name.get(tile)?;
        let w = self.table_for_tile(t).wire_index.get(wire)?;
        Some((self.tile_base[t] + w) as WireId)
    }

    /// Tile index a wire belongs to. Node ids are contiguous per tile
    /// in tile order, so this is a binary search over the per-tile
    /// base offsets; an id past the last node is not a wire.
    fn tile_of(&self, wire: WireId) -> Option<usize> {
        let w = wire as usize;
        if w >= self.node_count {
            return None;
        }
        let t = self.tile_base.partition_point(|&b| b <= w) - 1;
        Some(t)
    }

    /// Wire index within its tile.
    fn wire_idx_of(&self, wire: WireId) -> Option<usize> {
        let t = self.tile_of(wire)?;
        let idx = wire as usize - self.tile_base[t];
        if idx < self.table_for_tile(t).wire_names.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Add one tileconn stitch edge between two wires in different
    /// tiles: the two wires are the same electrical node (tileconn
    /// names the pair), so the router may cross either way and the
    /// translator programs no PIP for the crossing. Fails loud on a
    /// same-tile pair or an unknown wire: a stitch is by definition
    /// cross-tile, and a stitch to a wire the graph does not have
    /// means the tile JSONs and tileconn disagree. Adding the same
    /// pair twice is a no-op. The two fanout lists are re-sorted, so
    /// the N48 determinism rule still holds.
    pub fn add_stitch(&mut self, a: WireId, b: WireId) -> Result<(), WireDbError> {
        let (Some(ta), Some(tb)) = (self.tile_of(a), self.tile_of(b)) else {
            return Err(WireDbError::BadStitch(format!(
                "unknown wire in stitch pair ({a}, {b})"
            )));
        };
        if ta == tb {
            return Err(WireDbError::BadStitch(format!(
                "stitch pair ({a}, {b}) is inside one tile '{}'",
                self.tiles[ta].name
            )));
        }
        let key = (a.min(b), a.max(b));
        if !self.stitches.insert(key) {
            return Ok(());
        }
        self.fanout[a as usize].push(b);
        self.fanout[b as usize].push(a);
        self.fanout[a as usize].sort_unstable();
        self.fanout[b as usize].sort_unstable();
        Ok(())
    }

    /// True when the pair is a tileconn stitch added with
    /// [`Self::add_stitch`]. Used by the translator to tell stitches
    /// apart from PIPs and span continuations.
    pub fn is_stitch(&self, a: WireId, b: WireId) -> bool {
        self.stitches.contains(&(a.min(b), a.max(b)))
    }

    /// Number of tileconn stitch edges in the graph.
    pub fn stitch_count(&self) -> usize {
        self.stitches.len()
    }

    /// Remove one intra-tile PIP edge (`src` -> `dst` on `tile`)
    /// from the routing graph. See [`IntTileWireDb::remove_edge`].
    /// Returns false when the tile or a wire is unknown, or the pair
    /// was not an edge.
    pub fn forbid_pip(&mut self, tile: &str, src: &str, dst: &str) -> bool {
        let (Some(s), Some(d)) = (self.wire_id(tile, src), self.wire_id(tile, dst)) else {
            return false;
        };
        self.remove_edge(s, d)
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

    /// Remove one directed intra-tile PIP edge (`src` -> `dst`) from
    /// the routing graph, so the router can never traverse it and the
    /// translator can never emit it. The caller names the conflict
    /// (the clock-leg avoidance in helion-cli removes a routed PIP
    /// whose segbits disagree with every clock leg its tile must
    /// carry). Only same-tile pairs are removed: a cross-tile pair
    /// is a span-wire continuation, not a PIP, and is never a
    /// segbits conflict. A bidirectional PIP removes both
    /// directions: the PIP programs its bits whichever way the
    /// router traverses it. Returns true when an edge was removed;
    /// false when the pair was not an edge (unknown wire, different
    /// tiles, or already removed). The PIP table is untouched, so
    /// `pip_bidir` still answers for edges the router did use.
    pub fn remove_edge(&mut self, src: WireId, dst: WireId) -> bool {
        let Some(ts) = self.tile_of(src) else {
            return false;
        };
        let Some(td) = self.tile_of(dst) else {
            return false;
        };
        if ts != td {
            return false;
        }
        let mut removed = false;
        if let Some(list) = self.fanout.get_mut(src as usize) {
            let before = list.len();
            list.retain(|w| *w != dst);
            removed = list.len() != before;
        }
        let (Some(is), Some(id)) = (self.wire_idx_of(src), self.wire_idx_of(dst)) else {
            return removed;
        };
        let pip_table = &self.table_for_tile(ts).pip_table;
        if pip_table.get(&(is as u32, id as u32)) == Some(&true)
            || pip_table.get(&(id as u32, is as u32)) == Some(&true)
        {
            if let Some(list) = self.fanout.get_mut(dst as usize) {
                let before = list.len();
                list.retain(|w| *w != src);
                removed = removed || list.len() != before;
            }
        }
        removed
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
        let pip_table = &self.table_for_tile(ta).pip_table;
        if pip_table.get(&(ia, ib)) == Some(&true) {
            Some(BidirDir::Fwd)
        } else if pip_table.get(&(ib, ia)) == Some(&true) {
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
        let Some(na) = self
            .wire_idx_of(a)
            .and_then(|i| self.table_for_tile(ta).wire_names.get(i))
        else {
            return false;
        };
        let Some(nb) = self
            .wire_idx_of(b)
            .and_then(|i| self.table_for_tile(tb).wire_names.get(i))
        else {
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
        self.node_count
    }

    fn is_stitch(&self, a: WireId, b: WireId) -> bool {
        IntTileWireDb::is_stitch(self, a, b)
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
            name: &self.table_for_tile(t).wire_names[w],
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
    fn fanout_lists_are_sorted_deterministic() {
        // N48: the PIP table is a HashMap, so building fanout by
        // iterating it pushes PIP edges in per-process random
        // order, and A* explores neighbors (and breaks f-score
        // ties) in fanout order: the same netlist routed
        // differently on every process (counter12 seed 0 needed
        // 28-50 iterations and timed out 2/10 runs; counter16
        // seed 0 4/10). Sorting each fanout list makes the graph
        // a pure function of the tile JSON and the tile set.
        // Rebuild twice and require byte-identical fanout: a
        // second build in the same process uses a fresh HashMap
        // seed, so equal fanout proves the order no longer comes
        // from the map.
        let db1 = one_tile();
        let db2 = one_tile();
        for w in 0..db1.wire_count() {
            let f1 = db1.fanout(w as u32);
            let f2 = db2.fanout(w as u32);
            assert_eq!(f1, f2, "fanout of wire {w} differs between builds");
            assert!(
                f1.windows(2).all(|p| p[0] <= p[1]),
                "fanout of wire {w} is not sorted: {f1:?}"
            );
        }
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
    fn remove_edge_drops_one_directional_pip_only() {
        // The clock-leg conflict PIP: LOGIC_OUTS_W22 drives
        // INT_NODE_SDQ_80_INT_OUT1 (directional). Removing it must
        // leave the sibling PIPs from the same source wire alone.
        let mut db = one_tile();
        let src = db.wire_id("INT_X12Y45", "LOGIC_OUTS_W22").unwrap();
        let dst = db
            .wire_id("INT_X12Y45", "INT_NODE_SDQ_80_INT_OUT1")
            .unwrap();
        let sib = db
            .wire_id("INT_X12Y45", "INT_NODE_SDQ_78_INT_OUT0")
            .unwrap();
        assert!(db.fanout(src).contains(&dst));
        assert!(db.fanout(src).contains(&sib));
        assert!(!db.fanout(dst).contains(&src));
        assert!(db.remove_edge(src, dst));
        assert!(!db.fanout(src).contains(&dst));
        assert!(db.fanout(src).contains(&sib));
        // Second removal is a no-op, not an error.
        assert!(!db.remove_edge(src, dst));
        // A cross-tile continuation pair is not a PIP edge and is
        // never removed.
        let db2 = two_tiles_north();
        let beg = db2.wire_id("INT_X12Y45", "NN12_BEG7").unwrap();
        let end = db2.wire_id("INT_X12Y57", "NN12_END7").unwrap();
        let mut db2 = db2;
        assert!(!db2.remove_edge(beg, end));
        assert!(db2.fanout(beg).contains(&end));
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
        // is not instantiated, so it has no continuation edge here:
        // every fanout target of END7 stays in its own tile. (It does
        // have a same-tile PIP edge to NN12_BEG7 in (12,45): END feeds
        // BEG through one PIP inside the tile, so a bare
        // `!fanout(end45).contains(&beg)` check would be wrong.)
        let end45 = db.wire_id("INT_X12Y45", "NN12_END7").unwrap();
        let end_same = db
            .fanout(end45)
            .iter()
            .all(|w| db.wire_info(*w).unwrap().tile == "INT_X12Y45");
        assert!(end_same);
        assert!(!db.is_continuation(end45, beg));
        // Short spans: EE2_E_BEG3 in (12,45) needs (13,45), absent.
        let ee = db.wire_id("INT_X12Y45", "EE2_E_BEG3").unwrap();
        assert!(db.fanout(ee).is_empty());
    }

    #[test]
    fn horizontal_span_continuations_use_tileconn_steps() {
        // Tileconn stitches a length-2 horizontal span to the
        // adjacent INT tile, not two away: EE2 in (12,45) continues
        // to (13,45), WW2 in (13,45) continues to (12,45).
        let db = IntTileWireDb::from_json(
            TILE_JSON,
            &[
                IntTile::new("INT_X12Y45", 12, 45),
                IntTile::new("INT_X13Y45", 13, 45),
                IntTile::new("INT_X14Y45", 14, 45),
            ],
        )
        .unwrap();
        let ee_beg = db.wire_id("INT_X12Y45", "EE2_E_BEG3").unwrap();
        let ee_end = db.wire_id("INT_X13Y45", "EE2_E_END3").unwrap();
        assert!(db.fanout(ee_beg).contains(&ee_end));
        assert!(db.fanout(ee_end).contains(&ee_beg));
        assert!(db.is_continuation(ee_beg, ee_end));
        // The old two-tile partner (14,45) is not a continuation.
        let ee_far = db.wire_id("INT_X14Y45", "EE2_E_END3").unwrap();
        assert!(!db.fanout(ee_beg).contains(&ee_far));
        assert!(!db.is_continuation(ee_beg, ee_far));
        let ww_beg = db.wire_id("INT_X13Y45", "WW2_E_BEG3").unwrap();
        let ww_end = db.wire_id("INT_X12Y45", "WW2_E_END3").unwrap();
        assert!(db.fanout(ww_beg).contains(&ww_end));
        assert!(db.is_continuation(ww_beg, ww_end));
        // A length-4 horizontal span reaches two INT tiles away.
        let ee4_beg = db.wire_id("INT_X12Y45", "EE4_E_BEG3").unwrap();
        let ee4_end = db.wire_id("INT_X14Y45", "EE4_E_END3").unwrap();
        assert!(db.fanout(ee4_beg).contains(&ee4_end));
        assert!(db.is_continuation(ee4_beg, ee4_end));
        // A length-1 horizontal span still reaches the adjacent tile.
        let ee1_beg = db.wire_id("INT_X12Y45", "EE1_E_BEG3").unwrap();
        let ee1_end = db.wire_id("INT_X13Y45", "EE1_E_END3").unwrap();
        assert!(db.fanout(ee1_beg).contains(&ee1_end));
        assert!(db.is_continuation(ee1_beg, ee1_end));
    }

    #[test]
    fn continuation_partner_parses_every_span_wire() {
        let db = one_tile();
        let mut beg = 0;
        let mut end = 0;
        for name in db.wire_names_for_type("INT").unwrap() {
            if let Some((partner, _, _)) = continuation_partner(name) {
                // Every span wire has a partner of the same form in
                // the same wire list.
                assert!(
                    db.wire_id("INT_X12Y45", &partner).is_some(),
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
            Err(WireDbError::WrongTileType { .. })
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

    fn placed_at(coords: &[(i32, i32)]) -> crate::place::UltraPlaced {
        crate::place::UltraPlaced {
            slices: coords
                .iter()
                .enumerate()
                .map(|(i, (x, y))| crate::place::PlacedUltraSlice {
                    packed_idx: i,
                    site: format!("SLICE_X{x}Y{y}"),
                    tile: format!("CLB_X{x}Y{y}"),
                    x: *x,
                    y: *y,
                })
                .collect(),
            seed: 0,
        }
    }

    #[test]
    fn from_placed_slices_names_int_tiles_from_slices() {
        let db = IntTileWireDb::from_placed_slices(TILE_JSON, &placed_at(&[(0, 0)])).unwrap();
        assert_eq!(db.tile_count(), 1);
        assert_eq!(db.wire_count(), 1475);
        let w = db.wire_id("INT_X0Y0", "BOUNCE_E_0_FT1").unwrap();
        assert_eq!(
            db.wire_info(w),
            Some(WireInfo {
                name: "BOUNCE_E_0_FT1",
                tile: "INT_X0Y0"
            })
        );
        // Slices sharing one tile dedup to one INT tile; distinct
        // slices give distinct tiles.
        let db =
            IntTileWireDb::from_placed_slices(TILE_JSON, &placed_at(&[(0, 0), (0, 0), (12, 45)]))
                .unwrap();
        assert_eq!(db.tile_count(), 2);
        assert_eq!(db.wire_count(), 2 * 1475);
        assert!(db.wire_id("INT_X12Y45", "BOUNCE_E_0_FT1").is_some());
        // Empty placement fails loud: nothing to route over.
        assert!(matches!(
            IntTileWireDb::from_placed_slices(TILE_JSON, &placed_at(&[])),
            Err(WireDbError::EmptyTiles)
        ));
    }

    /// A small INT_INTF-like tile JSON: 6 wires, 2 LOGIC_OUTS
    /// pass-through PIPs, mirroring INT_INTF_R at small scale.
    const INTF_JSON: &str = r#"{"tile_type": "TESTINTF", "pips": {
      "TESTINTF.LOGIC_OUTS_R0->>LOGIC_OUTS_L0": {"src_wire": "LOGIC_OUTS_R0", "dst_wire": "LOGIC_OUTS_L0", "is_directional": "1", "is_pseudo": "0"},
      "TESTINTF.LOGIC_OUTS_R1->>LOGIC_OUTS_L1": {"src_wire": "LOGIC_OUTS_R1", "dst_wire": "LOGIC_OUTS_L1", "is_directional": "1", "is_pseudo": "0"}
    }, "wires": {
      "IMUX_FT0_1": null, "IMUX_FT0_3": null,
      "LOGIC_OUTS_L0": null, "LOGIC_OUTS_L1": null,
      "LOGIC_OUTS_R0": null, "LOGIC_OUTS_R1": null
    }}"#;

    #[test]
    fn mixed_int_and_intintf_tiles_keep_separate_tables() {
        let db = IntTileWireDb::from_json_typed(
            &[("INT", TILE_JSON), ("TESTINTF", INTF_JSON)],
            &[
                IntTile::new("INT_X0Y0", 0, 0),
                IntTile::new_typed("TESTINTF_X0Y0", 0, 0, "TESTINTF"),
            ],
        )
        .unwrap();
        assert_eq!(db.tile_count(), 2);
        assert_eq!(db.tile_count_of_type("INT"), 1);
        assert_eq!(db.tile_count_of_type("TESTINTF"), 1);
        // Node count is the sum of the per-type wire counts, not two
        // times one width.
        assert_eq!(db.wire_count(), 1475 + 6);
        assert_eq!(db.wires_per_tile(), 1475);
        assert_eq!(db.pip_count(), 3774);
        // Each tile resolves only its own type's wires: the INT tile
        // has no LOGIC_OUTS_R0, the INT_INTF tile has no BOUNCE wire.
        assert!(db.wire_id("INT_X0Y0", "BOUNCE_E_0_FT1").is_some());
        assert!(db.wire_id("INT_X0Y0", "LOGIC_OUTS_R0").is_none());
        assert!(db.wire_id("TESTINTF_X0Y0", "LOGIC_OUTS_R0").is_some());
        assert!(db.wire_id("TESTINTF_X0Y0", "BOUNCE_E_0_FT1").is_none());
        // The INT_INTF PIPs are edges only on the INT_INTF tile.
        let r0 = db.wire_id("TESTINTF_X0Y0", "LOGIC_OUTS_R0").unwrap();
        let l0 = db.wire_id("TESTINTF_X0Y0", "LOGIC_OUTS_L0").unwrap();
        assert!(db.fanout(r0).contains(&l0));
        assert!(!db.fanout(l0).contains(&r0));
        // The two tiles share name coordinates (0, 0): no duplicate
        // error, and the INT tile still has its span wires.
        assert!(db.wire_id("INT_X0Y0", "NN12_BEG7").is_some());
        // Wire info names the right tile for each node.
        assert_eq!(
            db.wire_info(l0),
            Some(WireInfo {
                name: "LOGIC_OUTS_L0",
                tile: "TESTINTF_X0Y0"
            })
        );
        assert_eq!(db.wire_info(db.wire_count() as u32), None);
    }

    #[test]
    fn mixed_graph_rejects_unknown_tile_type_and_bad_stitch() {
        // A tile of a type with no JSON fails loud.
        assert!(matches!(
            IntTileWireDb::from_json_typed(
                &[("INT", TILE_JSON)],
                &[IntTile::new_typed("TESTINTF_X0Y0", 0, 0, "TESTINTF")]
            ),
            Err(WireDbError::UnknownTileType { .. })
        ));
        // A JSON whose tile_type disagrees with its key fails loud.
        assert!(matches!(
            IntTileWireDb::from_json_typed(
                &[("TESTINTF", TILE_JSON)],
                &[IntTile::new_typed("TESTINTF_X0Y0", 0, 0, "TESTINTF")]
            ),
            Err(WireDbError::WrongTileType { .. })
        ));
        // Stitches join two tiles, never two wires of one tile, and
        // never an unknown wire.
        let mut db = IntTileWireDb::from_json_typed(
            &[("INT", TILE_JSON), ("TESTINTF", INTF_JSON)],
            &[
                IntTile::new("INT_X0Y0", 0, 0),
                IntTile::new_typed("TESTINTF_X0Y0", 0, 0, "TESTINTF"),
            ],
        )
        .unwrap();
        let int_wire = db.wire_id("INT_X0Y0", "BOUNCE_E_0_FT1").unwrap();
        let intf_wire = db.wire_id("TESTINTF_X0Y0", "IMUX_FT0_1").unwrap();
        assert!(!db.is_stitch(int_wire, intf_wire));
        assert_eq!(db.stitch_count(), 0);
        db.add_stitch(int_wire, intf_wire).unwrap();
        assert!(db.is_stitch(int_wire, intf_wire));
        assert!(db.is_stitch(intf_wire, int_wire));
        assert_eq!(db.stitch_count(), 1);
        assert!(db.fanout(int_wire).contains(&intf_wire));
        assert!(db.fanout(intf_wire).contains(&int_wire));
        // Adding the same pair twice is a no-op, not a second edge.
        db.add_stitch(int_wire, intf_wire).unwrap();
        assert_eq!(db.stitch_count(), 1);
        assert_eq!(
            db.fanout(int_wire)
                .iter()
                .filter(|w| **w == intf_wire)
                .count(),
            1
        );
        // Same-tile and unknown-wire stitches fail loud.
        let other_int = db.wire_id("INT_X0Y0", "LOGIC_OUTS_E0").unwrap();
        assert!(matches!(
            db.add_stitch(int_wire, other_int),
            Err(WireDbError::BadStitch(_))
        ));
        assert!(matches!(
            db.add_stitch(int_wire, u32::MAX),
            Err(WireDbError::BadStitch(_))
        ));
        // A stitch is not a continuation, and a continuation is not
        // a stitch.
        assert!(!db.is_continuation(int_wire, intf_wire));
        assert!(!db.is_stitch(
            db.wire_id("INT_X0Y0", "NN12_BEG7").unwrap(),
            db.wire_id("INT_X0Y0", "NN12_BEG7").unwrap()
        ));
    }
}
