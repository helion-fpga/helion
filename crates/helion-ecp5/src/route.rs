//! Route stage: [`place::PlacedEcp5`] -> [`RoutedEcp5`] for the Lattice ECP5 (LFE5U-85F).
//!
//! A* router over the Trellis `bits.db` mux graph, plus dedicated handling
//! for the global clock network (DCC -> DCS -> quadrant PCLK spine).
//!
//! # Routing graph model
//!
//! A node is `(tile entry, wire name)`. Wire names carry Trellis directional
//! prefixes (`E1_`, `W3_`, `N1W1_`, `S13E2_`, ...); the prefix resolves to an
//! anchor tile offset, so `E1_H01E0001` in tile `(r, c)` is the node
//! `(entry at (r, c+1), "H01E0001")`. Wires without a prefix anchor in the
//! tile whose `bits.db` mentions them. This matches nextpnr-ecp5's own
//! model: every `(tile, wire)` pair is distinct and span-wire continuity is
//! expressed by explicit mux pips (verified against `nextpnr-ecp5
//! --textcfg` output, e.g. `arc: E1_H01E0001 E3_H06W0003` chains).
//!
//! Edges are mux inputs: `src -> dst`, programmed in the tile that owns the
//! mux. Mux inputs spelled `-` in `bits.db` (default, no bits) are real
//! edges with an empty bit list. `fixed_conn` lines need no graph edges:
//! every router endpoint is a fabric-side wire (a mux dst for loads, a mux
//! src for drivers), so the hardwired bel connections are implicit.
//!
//! # Scale strategy
//!
//! The 85F has 14231 tile entries; instantiating every wire would be
//! millions of nodes. Instead, per-tile-*type* mux data (3669 muxes total)
//! is parsed once into a process-wide cache, and per-tile nodes/edges are
//! interned lazily as A* expands. A blinky-scale route touches a few ten
//! thousand nodes.
//!
//! # Clocks
//!
//! Clock nets listed in [`place::PlacedEcp5::clocks`] ride the global network:
//! the DCC input mux (`G_TDCC{i}CLKI` in `TMID_0`) selects the placer-given
//! dedicated source wire (a clock-pad wire like `G_JTRQPCLKCIB1` or a PLL
//! output like `G_JULCPLL0CLKOP`); the DCC output (`G_VPFN{ii}00`) feeds the
//! per-quadrant DCS muxes (`G_DCS{q}CLK{j}`) and quadrant PCLK spines
//! (`G_{Q}PCLK{p}`); each clocked slice takes `CLK{0,1} <- G_HPBX{pp}00`
//! then `MUXCLK{slice} <- CLK{0,1}` plus the `CLKMUX` config enum. This
//! mirrors nextpnr-ecp5's single-clock reference output exactly (DCC0,
//! PCLK0, `G_HPBX0000`). v1 supports DCC 0..5 (the `TMID_0` inputs); ECLK
//! edge clocks are not routed (fail loud).
//!
//! # Carry chains
//!
//! ECP5 carry is a dedicated cascade inside the PFU tile (no routing muxes
//! exist for it in `bits.db`). Nets marked [`NetKind::Carry`] must have all
//! endpoints in one `PLC2` tile; they are recorded with no routing segments.
//! A carry net spanning tiles is a pack/place error and fails loud.
//!
//! # IO pads
//!
//! Input pads do not use the PIC tile's `JDI{L}` muxes (those are dead ends
//! in the mux graph). The PIO's IOLOGIC hardwires to the nearest CIB tile's
//! `JQ` wires (verified against nextpnr-ecp5 textcfg: `arc: V01S0100 JQ0`
//! with no PIC arcs). The router maps PIOA->JQ0, PIOB->JQ1, PIOC->JQ2,
//! PIOD->JQ3 in the nearest CIB (stacked at the same row/col for top/bottom
//! edges, orthogonally adjacent for left/right edges). The PIO->JQ link is
//! hardwired (no bits); the driver node is the `JQ` wire itself.
//!
//! # Fail-loud contract
//!
//! * Net with no driver -> `Err` naming the net.
//! * Net with no path (congestion or dead wire) -> `Err` naming the net.
//! * Unknown tile/bel/pin/wire, EBR/DSP cells, >2 global clocks in one
//!   `PLC2` tile, DCC index > 5, or a clock source wire that is not a real
//!   input of the DCC mux -> `Err`.
//! * Nets are never silently dropped: every [`PlacedNet`] appears in the
//!   output (signal nets with their segments, carry/const nets with none).
//!
//! # Integration notes
//!
//! * The public entry points take the real [`place::PlacedEcp5`].
//!   [`build_model`] lowers it to the router's internal [`RouteModel`]
//!   (cells, nets, clocks); the A* core below only ever sees the model.
//! * Nets starting with [`pack::CARRY_NET_PREFIX`] are dedicated CCU2
//!   wires: the model builder skips them on LUT inputs (no fabric load),
//!   and classifies them [`NetKind::Carry`] (or `Const0`/`Const1` for
//!   `$carry$zero`/`$carry$one`).
//! * `lib.rs` re-exports [`RoutedEcp5`] (and [`NetKind`], which appears in
//!   the public [`RoutedNet`]) from this module.
//! * Slice wiring assumptions (validated against nextpnr-ecp5 `--textcfg`):
//!   LUT/FF index `i` (0..7) uses fabric wires `A{i}`..`D{i}` (inputs),
//!   `F{i}_SLICE` (LUT output), `Q{i}` (FF output); slice `i/2` uses
//!   `MUXCLK{i/2}`, `MUXLSR{i/2}`, `CE{i/2}`; PIO letter `A`..`D` maps to
//!   pad wires `JPADDI{L}_PIO` (in) and `J{L}0` (out, muxed in the adjacent
//!   CIB); PCLK index `p` corresponds to slice wire `G_HPBX{pp}00`.
//! * Multi-clock PCLK assignment (`pclk = clock ordinal`) matches
//!   nextpnr's single-clock reference; the extension to several clocks is
//!   new but uses validated muxes.

use crate::device::Device;
use crate::pack::CARRY_NET_PREFIX;
use crate::place;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::OnceLock;

/// Re-exported for the bitstream worker: bit requirements are programmed
/// per [`RoutedSegment`].
pub use crate::db::BitReq;

// ---------------------------------------------------------------------------
// Public API: placed mirror (route worker's minimal view of placement)
// ---------------------------------------------------------------------------

/// The router's internal design model, lowered from [`place::PlacedEcp5`]
/// by [`build_model`]. Cells carry concrete tile/bel positions and pin->net
/// connections; the A* core only ever sees this model.
#[derive(Clone, Debug, Default)]
struct RouteModel {
    /// Top module name, carried through to [`RoutedEcp5`].
    top: String,
    /// Placed cells.
    cells: Vec<PlacedCell>,
    /// Nets (every net referenced by a pin is listed).
    nets: Vec<PlacedNet>,
    /// Clocks assigned to the global network.
    clocks: Vec<PlacedClock>,
}

/// One placed cell.
#[derive(Clone, Debug)]
struct PlacedCell {
    /// Instance name (for error messages).
    pub name: String,
    /// Cell kind.
    pub kind: CellKind,
    /// Tile grid row.
    pub tile_row: u32,
    /// Tile grid column.
    pub tile_col: u32,
    /// Bel name, e.g. `"SLICE_A"` or `"PIOA"`.
    pub bel: String,
    /// LUT/FF index within the `PLC2` tile (0..7). Unused for IO.
    pub index: u8,
    /// Pin -> net connections.
    ///
    /// Pin names: LUT uses `"A"`,`"B"`,`"C"`,`"D"` (inputs) and `"Q"`
    /// (output); FF uses `"Q"` (output), `"CLK"`, `"CE"`, `"LSR"`
    /// (optional inputs); a `"D"` pin appears only for unpaired FFs and is
    /// routed from fabric (a paired FF's D is the dedicated slice LUT
    /// output, so the model builder omits it); IO input uses `"O"`
    /// (output); IO output uses `"I"` (input).
    pub pins: Vec<CellPin>,
}

/// One pin->net connection.
#[derive(Clone, Debug)]
struct CellPin {
    /// Pin name (see [`PlacedCell::pins`]).
    pub pin: String,
    /// Net name.
    pub net: String,
}

/// Placed cell kinds supported by the v1 router.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellKind {
    /// 4-LUT.
    Lut,
    /// Flip-flop (paired with its slice LUT; `D` is dedicated).
    Ff,
    /// Input pad.
    IoIn,
    /// Output pad.
    IoOut,
}

/// One net.
#[derive(Clone, Debug)]
struct PlacedNet {
    /// Net name.
    pub name: String,
    /// Net kind.
    pub kind: NetKind,
}

/// Net kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    /// Ordinary signal net: routed through the fabric.
    Signal,
    /// Carry chain net: dedicated cascade, no fabric routing. All endpoints
    /// must be in one `PLC2` tile.
    Carry,
    /// Tied low (handled by pack config, not routed).
    Const0,
    /// Tied high (handled by pack config, not routed).
    Const1,
}

/// One clock assigned to the global network.
#[derive(Clone, Debug)]
struct PlacedClock {
    /// Clock net name (listed in the model's nets).
    pub net: String,
    /// Global DCC index, 0..5 (the `TMID_0` DCC inputs in v1).
    pub dcc: u8,
    /// Dedicated wire feeding the DCC input mux, e.g. `"G_JTRQPCLKCIB1"`
    /// (clock pad) or `"G_JULCPLL0CLKOP"` (PLL output). Must be a real
    /// input of the `G_TDCC{dcc}CLKI` mux.
    pub source_wire: String,
}

// ---------------------------------------------------------------------------
// Public API: routed output (consumed by the bitstream worker)
// ---------------------------------------------------------------------------

/// Routed placed netlist: every net assigned to concrete routing resources.
///
/// For the bitstream worker: each [`RoutedSegment`] is one
/// `arc: <dst> <src>` line in the tile named by `tile_key` (Trellis
/// textcfg format, exactly as `nextpnr-ecp5 --textcfg` emits), and `bits`
/// are the `F<frame>B<bit>` / `!F<frame>B<bit>` requirements to program.
/// An empty `bits` vec means the `-` default input (no bits; the bitstream
/// starts cleared, so nothing needs programming, but the arc line is still
/// emitted for diffability).
#[derive(Clone, Debug, Default)]
pub struct RoutedEcp5 {
    /// Top module name.
    pub top: String,
    /// Routed nets (one entry per [`PlacedNet`], same order).
    pub nets: Vec<RoutedNet>,
    /// Routed global clocks (same order as the model's clocks).
    pub clocks: Vec<RoutedClock>,
    /// Non-routing config enums the router programmed (currently the
    /// `CLK{0,1}.CLKMUX` settings for global clocks).
    pub config_enums: Vec<ConfigEnumSetting>,
}

/// One routed net.
#[derive(Clone, Debug)]
pub struct RoutedNet {
    /// Net name.
    pub name: String,
    /// Net kind.
    pub kind: NetKind,
    /// Programmed mux selections forming the routing tree. Empty for
    /// carry/const nets and for signal nets with no loads.
    pub segments: Vec<RoutedSegment>,
}

/// One programmed routing mux: in tile `tile_key`, connect `src` to `dst`.
#[derive(Clone, Debug)]
pub struct RoutedSegment {
    /// Full tilegrid key, e.g. `"CIB_R14C1:CIB_LR"` (textcfg `.tile` name).
    pub tile_key: String,
    /// Tile grid row.
    pub tile_row: u32,
    /// Tile grid column.
    pub tile_col: u32,
    /// Mux output wire, as written in `bits.db` (e.g. `"A0"`,
    /// `"E1_H01E0001"`, `"G_TDCC0CLKI"`).
    pub dst: String,
    /// Selected mux input wire, as written in `bits.db` (e.g.
    /// `"E3_H06W0003"`, `"F0_SLICE"`, `"G_HPBX0000"`).
    pub src: String,
    /// Bit requirements to program (`F<frame>B<bit>` set/clear). Empty for
    /// `-` default inputs.
    pub bits: Vec<BitReq>,
}

/// One routed global clock.
#[derive(Clone, Debug)]
pub struct RoutedClock {
    /// Clock net name.
    pub net: String,
    /// DCC index used.
    pub dcc: u8,
    /// Quadrant PCLK index used (clock ordinal).
    pub pclk: u8,
    /// DCC input mux programming (`G_TDCC{dcc}CLKI <- source_wire`).
    pub dcc_input: RoutedSegment,
    /// Spine programming: per quadrant, the DCS mux
    /// (`G_DCS{q}CLK{j} <- G_VPFN{ii}00`) and the PCLK mux
    /// (`G_{Q}PCLK{p} <- G_VPFS{ii}00`).
    pub spine: Vec<RoutedSegment>,
    /// Per clocked slice: `CLK{0,1} <- G_HPBX{pp}00` and
    /// `MUXCLK{slice} <- CLK{0,1}`.
    pub slices: Vec<RoutedSegment>,
}

/// One non-routing config enum setting: in tile `tile_key`, set
/// `name` to `value` (Trellis textcfg `enum:` line).
#[derive(Clone, Debug)]
pub struct ConfigEnumSetting {
    /// Full tilegrid key.
    pub tile_key: String,
    /// Enum name, e.g. `"CLK0.CLKMUX"`.
    pub name: String,
    /// Value name, e.g. `"CLK"`.
    pub value: String,
}

// ---------------------------------------------------------------------------
// Wire references: Trellis directional prefixes
// ---------------------------------------------------------------------------

/// A wire reference: stripped wire name plus the anchor tile offset.
///
/// `E1_H01E0001` -> `(dr=0, dc=+1, "H01E0001")`; `N1W1_JDIB` ->
/// `(dr=-1, dc=-1, "JDIB")`; `S13E2_X` -> `(dr=+13, dc=+2, "X")`.
/// Row 0 is the north edge. Wires without a directional prefix
/// (`G_HPBX0000`, `F0_SLICE`, `A0`) anchor in the referencing tile.
#[derive(Clone, Debug)]
struct WireRef {
    dr: i32,
    dc: i32,
    wire: String,
}

/// Parse one direction component: `neg_c` (e.g. `b'N'`) gives negative
/// offsets, `pos_c` (e.g. `b'S'`) positive. Returns `(0, s)` when absent.
fn parse_dir(s: &str, neg_c: u8, pos_c: u8) -> (i32, &str) {
    let first = s.as_bytes().first().copied().unwrap_or(0);
    let neg = if first == neg_c {
        true
    } else if first == pos_c {
        false
    } else {
        return (0, s);
    };
    let rest = &s[1..];
    let mut val: i32 = 0;
    let mut n: usize = 0;
    for c in rest.chars() {
        if !c.is_ascii_digit() {
            break;
        }
        val = val * 10 + (c as i32 - '0' as i32);
        n += 1;
    }
    if n == 0 {
        return (0, s);
    }
    (if neg { -val } else { val }, &rest[n..])
}

fn parse_wire_ref(name: &str) -> WireRef {
    let (dr, s) = parse_dir(name, b'N', b'S');
    let (dc, s) = parse_dir(s, b'W', b'E');
    if (dr != 0 || dc != 0) && s.starts_with('_') {
        WireRef {
            dr,
            dc,
            wire: s[1..].to_string(),
        }
    } else {
        WireRef {
            dr: 0,
            dc: 0,
            wire: name.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-tile-type mux data (process-wide cache)
// ---------------------------------------------------------------------------

/// One mux input: setting `bits` connects `src` to the mux output.
/// `bits` is empty for `-` (default, no bits to program).
#[derive(Clone, Debug)]
pub(crate) struct CachedMuxInput {
    pub(crate) src: String,
    src_ref: WireRef,
    pub(crate) bits: Vec<BitReq>,
}

/// One routing mux with its inputs.
#[derive(Clone, Debug)]
pub(crate) struct CachedMux {
    pub(crate) dst: String,
    dst_ref: WireRef,
    pub(crate) inputs: Vec<CachedMuxInput>,
}

/// Parsed mux data for one tile type.
#[derive(Clone, Debug, Default)]
struct TypeGraph {
    muxes: Vec<CachedMux>,
    /// mux dst (as written) -> mux index (first wins).
    mux_by_dst: HashMap<String, usize>,
    /// `(dr, dc, stripped src wire)` -> `(mux idx, input idx)`.
    src_index: HashMap<(i32, i32, String), Vec<(u32, u32)>>,
    /// Stripped wire names owned by this tile type (for anchor resolution).
    wires: HashSet<String>,
}

fn parse_bitreq(tok: &str) -> Option<BitReq> {
    let tok = tok.trim();
    let (tok, val) = match tok.strip_prefix('!') {
        Some(rest) => (rest, false),
        None => (tok, true),
    };
    let tok = tok.strip_prefix('F')?;
    let (frame, bit) = tok.split_once('B')?;
    Some(BitReq {
        frame: frame.parse().ok()?,
        bit: bit.parse().ok()?,
        val,
    })
}

fn parse_bits(toks: &[&str]) -> Option<Vec<BitReq>> {
    toks.iter().map(|t| parse_bitreq(t)).collect()
}

/// Parse the `.mux` sections of one `bits.db` text. Mirrors
/// `db::parse_bits_db`'s control flow exactly, except `-` inputs are kept
/// as empty bit lists instead of being dropped.
pub(crate) fn parse_muxes(text: &str) -> Vec<CachedMux> {
    let mut muxes = Vec::new();
    let mut cur: Option<CachedMux> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(dst) = line.strip_prefix(".mux ") {
            if let Some(m) = cur.take() {
                muxes.push(m);
            }
            let dst = dst.trim().to_string();
            cur = Some(CachedMux {
                dst_ref: parse_wire_ref(&dst),
                dst,
                inputs: Vec::new(),
            });
            continue;
        }
        if line.starts_with(".config_enum ")
            || line.starts_with(".fixed_conn ")
            || line.starts_with(".config ")
        {
            if let Some(m) = cur.take() {
                muxes.push(m);
            }
            cur = None;
            continue;
        }
        // Data line inside a .mux section. (Deliberate deviation from
        // db.rs: db.rs does not treat `.config ` as a section boundary, so
        // that section's bare bit lines are absorbed as phantom mux inputs
        // with empty bits. Those are not wires; the router must not see
        // them as graph edges.)
        let mut toks = line.split_whitespace();
        let name = match toks.next() {
            Some(n) => n,
            None => continue,
        };
        let rest: Vec<&str> = toks.collect();
        let bits = if rest == ["-"] {
            Some(Vec::new())
        } else {
            parse_bits(&rest)
        };
        if let (Some(m), Some(bits)) = (cur.as_mut(), bits) {
            let src = name.to_string();
            m.inputs.push(CachedMuxInput {
                src_ref: parse_wire_ref(&src),
                src,
                bits,
            });
        }
    }
    if let Some(m) = cur.take() {
        muxes.push(m);
    }
    muxes
}

fn build_type_graph(text: &str) -> TypeGraph {
    let muxes = parse_muxes(text);
    let mut g = TypeGraph {
        muxes,
        ..TypeGraph::default()
    };
    for (mi, m) in g.muxes.iter().enumerate() {
        g.mux_by_dst.entry(m.dst.clone()).or_insert(mi);
        g.wires.insert(m.dst_ref.wire.clone());
        for (ii, inp) in m.inputs.iter().enumerate() {
            let key = (
                inp.src_ref.dr,
                inp.src_ref.dc,
                inp.src_ref.wire.clone(),
            );
            g.src_index
                .entry(key)
                .or_default()
                .push((mi as u32, ii as u32));
            g.wires.insert(inp.src_ref.wire.clone());
        }
    }
    g
}

/// Process-wide per-tile-type mux cache, built once from the vendored
/// `bits.db` texts.
fn type_graphs() -> &'static Vec<(String, TypeGraph)> {
    static CACHE: OnceLock<Vec<(String, TypeGraph)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut v = Vec::new();
        for tt in crate::tiledata::TILE_TYPES {
            if let Some(text) = crate::tiledata::bits_db(tt) {
                v.push((tt.to_string(), build_type_graph(text)));
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    })
}



// ---------------------------------------------------------------------------
// Router: lazy routing graph + A*
// ---------------------------------------------------------------------------

/// Routing-graph node id: index into [`Router::nodes`].
type NodeId = u32;

/// A directed routing edge: `src -> to`, programmed by the mux
/// `(tile_entry, mux_idx, input_idx)`.
#[derive(Clone, Debug)]
struct Edge {
    to: NodeId,
    tile_entry: u32,
    mux_idx: u32,
    input_idx: u32,
}

/// Maximum A* node visits before giving up on one path.
const ASTAR_VISIT_CAP: usize = 500_000;
/// Cost adder for stepping onto an already-driven wire is infinite (hard
/// forbid); mux reuse with a different source is likewise forbidden.
///
/// The ECP5 router: lazy `(tile entry, wire)` graph over the Trellis mux
/// data, A* path search, congestion tracking.
pub struct Router<'a> {
    device: &'a Device,
    /// Tile entry index -> (row, col).
    entry_pos: Vec<(u32, u32)>,
    /// Tile entry index -> index into [`type_graphs`].
    entry_tgraph: Vec<usize>,
    /// Tile entry index -> full tilegrid key.
    entry_key: Vec<String>,
    /// (row, col) -> tile entry indices at that grid position.
    pos_index: HashMap<(u32, u32), Vec<u32>>,
    /// Distinct (dr, dc) prefix offsets occurring in the DB.
    prefix_offsets: Vec<(i32, i32)>,
    /// `(entry, wire)` -> node id.
    node_ids: HashMap<(u32, String), NodeId>,
    /// Node id -> `(entry, wire)`.
    nodes: Vec<(u32, String)>,
    /// Node id -> outgoing edges (built on first expansion).
    adj: Vec<Vec<Edge>>,
    adj_done: Vec<bool>,
}

/// Per-net endpoint collection: net kinds, driver/loads work items, and
/// clock-net -> FF load positions.
type EndpointMaps<'a> = (
    HashMap<&'a str, NetKind>,
    HashMap<&'a str, NetWork<'a>>,
    HashMap<&'a str, Vec<(u32, u8)>>,
);

/// Clock routing output: routed clocks, net name -> segments, config enums.
type ClockRouteOut = (
    Vec<RoutedClock>,
    HashMap<String, Vec<RoutedSegment>>,
    Vec<ConfigEnumSetting>,
);

impl<'a> Router<'a> {
    /// Build the router over `device`. Parses per-tile-type mux data via
    /// the process-wide cache; per-tile nodes/edges are interned lazily.
    pub fn new(device: &'a Device) -> Result<Self, String> {
        let tiles = device.tiles();
        let mut entry_pos = Vec::with_capacity(tiles.len());
        let mut entry_tgraph = Vec::with_capacity(tiles.len());
        let mut entry_key = Vec::with_capacity(tiles.len());
        let mut pos_index: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (i, t) in tiles.iter().enumerate() {
            let gi = type_graphs()
                .iter()
                .position(|(n, _)| n == &t.tile_type)
                .ok_or_else(|| {
                    format!(
                        "route: no mux data for tile type {} (tile {})",
                        t.tile_type, t.key
                    )
                })?;
            entry_pos.push((t.row, t.col));
            entry_tgraph.push(gi);
            entry_key.push(t.key.clone());
            pos_index
                .entry((t.row, t.col))
                .or_default()
                .push(i as u32);
        }
        // Distinct prefix offsets from the whole DB.
        let mut offs: HashSet<(i32, i32)> = HashSet::from([(0, 0)]);
        for (_, g) in type_graphs().iter() {
            for m in &g.muxes {
                offs.insert((m.dst_ref.dr, m.dst_ref.dc));
                for inp in &m.inputs {
                    offs.insert((inp.src_ref.dr, inp.src_ref.dc));
                }
            }
        }
        Ok(Self {
            device,
            entry_pos,
            entry_tgraph,
            entry_key,
            pos_index,
            prefix_offsets: offs.into_iter().collect(),
            node_ids: HashMap::new(),
            nodes: Vec::new(),
            adj: Vec::new(),
            adj_done: Vec::new(),
        })
    }

    /// Number of tile entries (for tests).
    pub fn entry_count(&self) -> usize {
        self.entry_pos.len()
    }

    /// Number of interned graph nodes so far (for tests/reports).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn graph(&self, entry: u32) -> &'static TypeGraph {
        // entry_tgraph indexes are validated in new(); the cache is 'static.
        &type_graphs()[self.entry_tgraph[entry as usize]].1
    }

    /// Intern `(entry, wire)` to a node id.
    fn intern(&mut self, entry: u32, wire: String) -> NodeId {
        if let Some(&id) = self.node_ids.get(&(entry, wire.clone())) {
            return id;
        }
        let id = self.nodes.len() as NodeId;
        self.node_ids.insert((entry, wire.clone()), id);
        self.nodes.push((entry, wire));
        self.adj.push(Vec::new());
        self.adj_done.push(false);
        id
    }

    /// Resolve a wire reference to a node, anchoring at `from_entry`'s
    /// tile plus the reference offset. Returns `None` when the anchor is
    /// off-chip or no entry there owns the wire.
    fn resolve(&mut self, from_entry: u32, wr: &WireRef) -> Option<NodeId> {
        let (r, c) = self.entry_pos[from_entry as usize];
        let ar = r as i32 + wr.dr;
        let ac = c as i32 + wr.dc;
        if ar < 0 || ac < 0 {
            return None;
        }
        let owners: Vec<u32> = self
            .pos_index
            .get(&(ar as u32, ac as u32))
            .cloned()
            .unwrap_or_default();
        for e in owners {
            if self.graph(e).wires.contains(&wr.wire) {
                let w = wr.wire.clone();
                return Some(self.intern(e, w));
            }
        }
        None
    }

    /// Expand a node's outgoing edges: every mux, in this tile or in a
    /// neighbor tile, whose source resolves to this node.
    fn expand(&mut self, node: NodeId) {
        if self.adj_done[node as usize] {
            return;
        }
        self.adj_done[node as usize] = true;
        let (entry, wire) = self.nodes[node as usize].clone();
        let (r, c) = self.entry_pos[entry as usize];
        // Collect (tile_entry, mux_idx, input_idx, dst_ref) first to keep
        // the borrow checker happy (graph() borrows self immutably).
        let mut cands: Vec<(u32, u32, u32, WireRef)> = Vec::new();
        for &(dr, dc) in &self.prefix_offsets.clone() {
            let pr = r as i32 - dr;
            let pc = c as i32 - dc;
            if pr < 0 || pc < 0 {
                continue;
            }
            let entries: Vec<u32> = self
                .pos_index
                .get(&(pr as u32, pc as u32))
                .cloned()
                .unwrap_or_default();
            for e2 in entries {
                let key = (dr, dc, wire.clone());
                if let Some(list) = self.graph(e2).src_index.get(&key).cloned() {
                    for (mi, ii) in list {
                        let dst_ref = self.graph(e2).muxes[mi as usize].dst_ref.clone();
                        cands.push((e2, mi, ii, dst_ref));
                    }
                }
            }
        }
        let mut edges = Vec::with_capacity(cands.len());
        for (e2, mi, ii, dst_ref) in cands {
            if let Some(to) = self.resolve(e2, &dst_ref) {
                if to != node {
                    edges.push(Edge {
                        to,
                        tile_entry: e2,
                        mux_idx: mi,
                        input_idx: ii,
                    });
                }
            }
        }
        self.adj[node as usize] = edges;
    }

    /// Mux dst/src/bit info for an edge.
    fn edge_info(&self, e: &Edge) -> (&str, &str, &[BitReq]) {
        let g = self.graph(e.tile_entry);
        let m = &g.muxes[e.mux_idx as usize];
        let inp = &m.inputs[e.input_idx as usize];
        (m.dst.as_str(), inp.src.as_str(), inp.bits.as_slice())
    }

    /// Grid position of a node (A* heuristic).
    fn node_pos(&self, node: NodeId) -> (i32, i32) {
        let (entry, _) = self.nodes[node as usize];
        let (r, c) = self.entry_pos[entry as usize];
        (r as i32, c as i32)
    }

    /// A* heuristic: Manhattan distance, halved. Edges can hop several
    /// tiles per step, so full Manhattan overestimates (inadmissible but
    /// complete and much faster; optimality is not required for routing).
    fn heuristic(&self, n: NodeId, tr: i32, tc: i32) -> i64 {
        let (r, c) = self.node_pos(n);
        ((r - tr).abs() + (c - tc).abs()) as i64 / 2
    }

    /// A* from `src` to `dst`.
    ///
    /// `used_mux`: `(tile entry, mux dst as written)` -> `src as written`
    /// for already-programmed muxes; an edge needing the same mux with a
    /// different src is skipped. `driven`: nodes already driven (as mux
    /// dsts) by other nets; stepping onto one would short two nets.
    fn astar(
        &mut self,
        src: NodeId,
        dst: NodeId,
        used_mux: &HashMap<(u32, String), String>,
        driven: &HashSet<NodeId>,
    ) -> Option<Vec<Edge>> {
        #[derive(Clone, Copy, PartialEq)]
        struct State {
            f: i64,
            g: i64,
            node: NodeId,
        }
        impl Eq for State {}
        impl Ord for State {
            fn cmp(&self, other: &Self) -> Ordering {
                other
                    .f
                    .cmp(&self.f)
                    .then_with(|| self.node.cmp(&other.node))
            }
        }
        impl PartialOrd for State {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let (tr, tc) = self.node_pos(dst);

        let mut g_score: HashMap<NodeId, i64> = HashMap::new();
        let mut came_from: HashMap<NodeId, (NodeId, Edge)> = HashMap::new();
        let mut heap = BinaryHeap::new();
        g_score.insert(src, 0);
        heap.push(State {
            f: self.heuristic(src, tr, tc),
            g: 0,
            node: src,
        });
        let mut visits: usize = 0;

        while let Some(State { g, node, .. }) = heap.pop() {
            if node == dst {
                let mut edges = Vec::new();
                let mut cur = dst;
                while cur != src {
                    let (prev, e) = came_from.remove(&cur)?;
                    edges.push(e);
                    cur = prev;
                }
                edges.reverse();
                return Some(edges);
            }
            if g > *g_score.get(&node).unwrap_or(&i64::MAX) {
                continue;
            }
            visits += 1;
            if visits > ASTAR_VISIT_CAP {
                return None;
            }
            self.expand(node);
            let out: Vec<Edge> = self.adj[node as usize].clone();
            for e in out {
                // Congestion: mux already programmed with another src.
                let (mdst, msrc, _) = self.edge_info(&e);
                if e.to != dst {
                    if let Some(used_src) = used_mux.get(&(e.tile_entry, mdst.to_string())) {
                        if used_src != msrc {
                            continue;
                        }
                    }
                    // Short: wire already driven by another net.
                    if driven.contains(&e.to) {
                        continue;
                    }
                }
                let ng = g + 1;
                if ng < *g_score.get(&e.to).unwrap_or(&i64::MAX) {
                    let f = ng + self.heuristic(e.to, tr, tc);
                    g_score.insert(e.to, ng);
                    came_from.insert(e.to, (node, e.clone()));
                    heap.push(State { f, g: ng, node: e.to });
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Cell pin -> routing node mapping
// ---------------------------------------------------------------------------

/// Find the tile entry at `(row, col)` hosting bel `bel`.
fn find_entry(router: &Router, row: u32, col: u32, bel: &str) -> Result<u32, String> {
    let entries: &[u32] = router
        .pos_index
        .get(&(row, col))
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let tiles = router.device.tiles();
    for &e in entries {
        if tiles[e as usize].sites.iter().any(|s| s.name == bel) {
            return Ok(e);
        }
    }
    Err(format!("route: no site {bel} at R{row}C{col}"))
}

/// Validate a slice bel against the LUT/FF index; returns the slice 0..3.
fn check_slice_bel(cell: &PlacedCell) -> Result<u8, String> {
    let s = match cell.bel.as_str() {
        "SLICE_A" => 0,
        "SLICE_B" => 1,
        "SLICE_C" => 2,
        "SLICE_D" => 3,
        _ => {
            return Err(format!(
                "route: cell '{}': bel '{}' is not a slice",
                cell.name, cell.bel
            ))
        }
    };
    if cell.index > 7 || cell.index / 2 != s {
        return Err(format!(
            "route: cell '{}': index {} does not belong to bel {}",
            cell.name, cell.index, cell.bel
        ));
    }
    Ok(s)
}

/// PIO letter from a PIO bel name (`"PIOA"` -> `'A'`).
fn pio_letter(cell: &PlacedCell) -> Result<char, String> {
    match cell.bel.as_str() {
        "PIOA" => Ok('A'),
        "PIOB" => Ok('B'),
        "PIOC" => Ok('C'),
        "PIOD" => Ok('D'),
        _ => Err(format!(
            "route: cell '{}': bel '{}' is not a PIO",
            cell.name, cell.bel
        )),
    }
}

impl Router<'_> {
    /// Node for a load on a mux dst wire in `entry`'s tile. Fails loud if
    /// the wire is not a real mux output (catches wiring-map bugs).
    fn load_node(&mut self, entry: u32, wire: &str, what: &str) -> Result<NodeId, String> {
        let ok = self.graph(entry).mux_by_dst.contains_key(wire);
        if !ok {
            return Err(format!(
                "route: {what}: {wire} is not a routing mux in tile {}",
                self.entry_key[entry as usize]
            ));
        }
        Ok(self.intern(entry, wire.to_string()))
    }

    /// Node for a driver on a wire written `src_written` in `entry`'s tile
    /// (prefixes resolved). Fails loud when unresolvable.
    fn driver_node(
        &mut self,
        entry: u32,
        src_written: &str,
        what: &str,
    ) -> Result<NodeId, String> {
        let wr = parse_wire_ref(src_written);
        self.resolve(entry, &wr).ok_or_else(|| {
            format!(
                "route: {what}: cannot resolve driver wire {src_written} from tile {}",
                self.entry_key[entry as usize]
            )
        })
    }

    /// Driver node for an input pad: the pad-driven wire feeding the
    /// tile's `JDI{L}` input mux (data-driven: prefers the direct
    /// `JPADDI{L}_PIO` pad wire, else the IOLOGIC output).
    fn ioin_driver(&mut self, entry: u32, pio: char, cell_name: &str) -> Result<NodeId, String> {
        // Input pad: the PIO's IOLOGIC hardwires to the stacked CIB tile's
        // JQ wires (no configurable mux; nextpnr emits e.g.
        // `arc: V01S0100 JQ0` with no PIC arcs). Map PIOA->JQ0, PIOB->JQ1,
        // PIOC->JQ2, PIOD->JQ3.
        let jq_idx: u32 = match pio {
            'A' => 0,
            'B' => 1,
            'C' => 2,
            'D' => 3,
            _ => {
                return Err(format!(
                    "route: cell '{cell_name}': unsupported PIO '{pio}' (expected A-D)"
                ))
            }
        };
        let (r, c) = self.entry_pos[entry as usize];
        // Find the nearest CIB: stacked at (r,c) for top/bottom edges,
        // or orthogonally adjacent for left/right edges.
        let mut cib_entry: Option<u32> = None;
        for (dr, dc) in [(0, 0), (0, 1), (0, -1), (1, 0), (-1, 0)] {
            let (nr, nc) = (r as i32 + dr, c as i32 + dc);
            if nr < 0 || nc < 0 {
                continue;
            }
            if let Some(pos) = self.device.tiles().iter().position(|t| {
                t.row == nr as u32
                    && t.col == nc as u32
                    && t.tile_type.starts_with("CIB")
            }) {
                cib_entry = Some(pos as u32);
                break;
            }
        }
        let cib_entry = cib_entry.ok_or_else(|| {
            format!("route: cell '{cell_name}': no CIB tile near R{r}C{c}")
        })?;
        let jq_wire = format!("JQ{jq_idx}");
        // The PIO->JQ connection is hardwired (no bits to program); the
        // driver node is the JQ wire itself. Documented in module docs.
        self.driver_node(cib_entry, &jq_wire, &format!("input pad {cell_name}"))
    }

    /// Load node for an output pad: the `J{L}0` wire muxed in a neighboring
    /// tile (data-driven search over the four orthogonal neighbors).
    fn ioout_load(&mut self, entry: u32, pio: char, cell_name: &str) -> Result<NodeId, String> {
        let want = format!("J{pio}0");
        let (r, c) = self.entry_pos[entry as usize];
        for (dr, dc) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
            let (nr, nc) = (r as i32 + dr, c as i32 + dc);
            if nr < 0 || nc < 0 {
                continue;
            }
            let entries: Vec<u32> = self
                .pos_index
                .get(&(nr as u32, nc as u32))
                .cloned()
                .unwrap_or_default();
            for e2 in entries {
                let hit = self
                    .graph(e2)
                    .muxes
                    .iter()
                    .find(|m| {
                        m.dst_ref.dr == 0 && m.dst_ref.dc == 0 && m.dst_ref.wire == want
                    })
                    .map(|m| m.dst_ref.wire.clone());
                if let Some(w) = hit {
                    return Ok(self.intern(e2, w));
                }
            }
        }
        Err(format!(
            "route: cell '{cell_name}': no adjacent mux driving J{pio}0 for PIO{pio} at R{r}C{c}"
        ))
    }
}

// ---------------------------------------------------------------------------
// Netlist -> endpoint collection
// ---------------------------------------------------------------------------

/// Working state for one net during routing.
struct NetWork<'n> {
    name: &'n str,
    driver: Option<(NodeId, String)>,
    loads: Vec<(NodeId, String)>,
}

/// Per-design routing state shared across nets and clocks.
#[derive(Default)]
struct RouteState {
    /// (tile entry, mux dst as written) -> src as written.
    used_mux: HashMap<(u32, String), String>,
    /// Nodes driven (as mux dsts) by already-routed nets.
    driven: HashSet<NodeId>,
    /// Global-clock spine programming still to record per tile:
    /// tile entry -> [(pclk, CLK mux name)].
    clk_assign: HashMap<u32, Vec<(u8, String)>>,
}

fn describe(cell: &PlacedCell, pin: &str) -> String {
    format!("{}.{} @ R{}C{}", cell.name, pin, cell.tile_row, cell.tile_col)
}

// ---------------------------------------------------------------------------
// Signal net routing
// ---------------------------------------------------------------------------

impl Router<'_> {
    /// Route one signal net: driver -> all loads as a tree. Records mux
    /// usage in `state`; fails loud naming the net when unroutable.
    fn route_signal_net(
        &mut self,
        net: &NetWork,
        state: &mut RouteState,
    ) -> Result<Vec<RoutedSegment>, String> {
        let driver = net.driver.as_ref().map(|(d, _)| *d).ok_or_else(|| {
            format!("route: net '{}' has no driver", net.name)
        })?;
        let mut reached: HashSet<NodeId> = HashSet::from([driver]);
        let mut reached_vec: Vec<NodeId> = vec![driver];
        let mut segments = Vec::new();

        for (load, desc) in &net.loads {
            if reached.contains(load) {
                continue;
            }
            // Nearest reached source first.
            let (lr, lc) = {
                let (e, _) = self.nodes[*load as usize];
                self.entry_pos[e as usize]
            };
            let mut sources = reached_vec.clone();
            sources.sort_by_key(|&s| {
                let (e, _) = self.nodes[s as usize];
                let (r, c) = self.entry_pos[e as usize];
                (r as i32 - lr as i32).abs() + (c as i32 - lc as i32).abs()
            });
            let mut path: Option<Vec<Edge>> = None;
            for src in sources {
                if let Some(p) = self.astar(src, *load, &state.used_mux, &state.driven) {
                    path = Some(p);
                    break;
                }
            }
            let path = path.ok_or_else(|| {
                format!("route: net '{}': no path to load {desc}", net.name)
            })?;
            for e in &path {
                let (dst, src, bits) = self.edge_info(e);
                let key = (e.tile_entry, dst.to_string());
                match state.used_mux.get(&key) {
                    Some(prev) if prev != src => {
                        return Err(format!(
                            "route: net '{}': mux conflict at {}:{} (already {})",
                            net.name,
                            self.entry_key[e.tile_entry as usize],
                            dst,
                            prev
                        ));
                    }
                    Some(_) => {}
                    None => {
                        state.used_mux.insert(key, src.to_string());
                    }
                }
                state.driven.insert(e.to);
                reached.insert(e.to);
                reached_vec.push(e.to);
                let (tr, tc) = self.entry_pos[e.tile_entry as usize];
                segments.push(RoutedSegment {
                    tile_key: self.entry_key[e.tile_entry as usize].clone(),
                    tile_row: tr,
                    tile_col: tc,
                    dst: dst.to_string(),
                    src: src.to_string(),
                    bits: bits.to_vec(),
                });
            }
            reached.insert(*load);
            reached_vec.push(*load);
        }
        Ok(segments)
    }

    /// Program one mux directly (clock network): validate `(dst, src)`
    /// against the DB, record usage, return the segment. Fails loud on any
    /// mismatch or conflict.
    fn program_mux(
        &mut self,
        entry: u32,
        dst: &str,
        src: &str,
        what: &str,
        state: &mut RouteState,
    ) -> Result<RoutedSegment, String> {
        let key_desc = self.entry_key[entry as usize].clone();
        let (dst_ref, bits) = {
            let g = self.graph(entry);
            let mi = g.mux_by_dst.get(dst).copied().ok_or_else(|| {
                format!("route: {what}: no mux {dst} in tile {key_desc}")
            })?;
            let m = &g.muxes[mi];
            let inp = m.inputs.iter().find(|i| i.src == src).ok_or_else(|| {
                format!("route: {what}: mux {dst} has no input {src} in tile {key_desc}")
            })?;
            (m.dst_ref.clone(), inp.bits.clone())
        };
        let key = (entry, dst.to_string());
        match state.used_mux.get(&key) {
            Some(prev) if prev != src => {
                return Err(format!(
                    "route: {what}: mux {dst} in tile {key_desc} already programmed to {prev}"
                ));
            }
            Some(_) => {}
            None => {
                state.used_mux.insert(key, src.to_string());
            }
        }
        let dst_node = self.resolve(entry, &dst_ref).ok_or_else(|| {
            format!("route: {what}: cannot resolve mux dst {dst} in tile {key_desc}")
        })?;
        state.driven.insert(dst_node);
        let (tr, tc) = self.entry_pos[entry as usize];
        Ok(RoutedSegment {
            tile_key: key_desc,
            tile_row: tr,
            tile_col: tc,
            dst: dst.to_string(),
            src: src.to_string(),
            bits,
        })
    }

    /// Validate a config enum name/value against the DB.
    fn validate_config_enum(&self, entry: u32, name: &str, value: &str) -> Result<(), String> {
        let tiles = self.device.tiles();
        let ttype = &tiles[entry as usize].tile_type;
        let tb = self
            .device
            .tile_bits(ttype)
            .map_err(|e| format!("route: {e}"))?;
        let ce = tb
            .config_enums
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| {
                format!("route: no config enum {name} in tile type {ttype}")
            })?;
        if !ce.options.iter().any(|o| o.value == value) {
            return Err(format!(
                "route: config enum {name} has no value {value} in tile type {ttype}"
            ));
        }
        Ok(())
    }

    /// The single `TMID_0` tile entry (hosts DCC 0..5 inputs in v1).
    fn tmid0_entry(&self) -> Result<u32, String> {
        let tiles = self.device.tiles();
        let mut found = None;
        for (i, t) in tiles.iter().enumerate() {
            if t.tile_type == "TMID_0" {
                if found.is_some() {
                    return Err("route: expected exactly one TMID_0 tile".to_string());
                }
                found = Some(i as u32);
            }
        }
        found.ok_or_else(|| "route: no TMID_0 tile in device".to_string())
    }

    /// The single tile entry of a CMUX type (one per quadrant).
    fn cmux_entry(&self, tile_type: &str) -> Result<u32, String> {
        let tiles = self.device.tiles();
        let mut found = None;
        for (i, t) in tiles.iter().enumerate() {
            if t.tile_type == tile_type {
                if found.is_some() {
                    return Err(format!("route: expected exactly one {tile_type} tile"));
                }
                found = Some(i as u32);
            }
        }
        found.ok_or_else(|| format!("route: no {tile_type} tile in device"))
    }

    /// Route one global clock: DCC input mux, per-quadrant PCLK spine
    /// muxes, and per-slice `CLK{0,1} <- G_HPBX{pp}00` /
    /// `MUXCLK{slice} <- CLK{0,1}` plus the `CLKMUX` config enum.
    fn route_clock(
        &mut self,
        clock: &PlacedClock,
        ordinal: u8,
        ff_loads: &[(u32, u8)],
        state: &mut RouteState,
        config_enums: &mut Vec<ConfigEnumSetting>,
    ) -> Result<RoutedClock, String> {
        if clock.dcc > 5 {
            return Err(format!(
                "route: clock '{}': DCC {} not supported in v1 (0..5, TMID_0 inputs)",
                clock.net, clock.dcc
            ));
        }
        if ordinal >= 16 {
            return Err(format!(
                "route: clock '{}': too many global clocks (max 16)",
                clock.net
            ));
        }
        let what = format!("clock '{}'", clock.net);

        // 1. DCC input mux: G_TDCC{dcc}CLKI <- source_wire.
        let tmid = self.tmid0_entry()?;
        let dcc_dst = format!("G_TDCC{}CLKI", clock.dcc);
        let dcc_input =
            self.program_mux(tmid, &dcc_dst, &clock.source_wire, &what, state)?;

        // 2. Quadrant PCLK spines: G_{Q}PCLK{p} <- G_VPFS{ii}00.
        // (Direct DCC->PCLK; the DCS mux is not used in v1. This matches
        // nextpnr-ecp5's own two-clock reference output.)
        let vpfs = format!("G_VPFS{:02}00", clock.dcc);
        let quads = [
            ("CMUX_UL_0", "G_ULPCLK"),
            ("CMUX_UR_0", "G_URPCLK"),
            ("CMUX_LL_0", "G_LLPCLK"),
            ("CMUX_LR_0", "G_LRPCLK"),
        ];
        let mut spine = Vec::new();
        for (tt, pclk_base) in quads {
            let entry = self.cmux_entry(tt)?;
            let pclk_dst = format!("{pclk_base}{ordinal}");
            spine.push(self.program_mux(entry, &pclk_dst, &vpfs, &what, state)?);
        }

        // 3. Slices: CLK{0,1} <- G_HPBX{pp}00, MUXCLK{slice} <- CLK{0,1}.
        let hpbx = format!("G_HPBX{:02}00", ordinal);
        let mut slices = Vec::new();
        // Dedupe (entry, slice): several FFs share one slice.
        let mut seen: HashSet<(u32, u8)> = HashSet::new();
        for &(entry, ff_idx) in ff_loads {
            let slice = ff_idx / 2;
            if !seen.insert((entry, slice)) {
                continue;
            }
            // Per-tile CLK mux assignment (max 2 global clocks per tile).
            let clk_mux_name: String = match state
                .clk_assign
                .get(&entry)
                .and_then(|v| v.iter().find(|(p, _)| *p == ordinal))
            {
                Some((_, name)) => name.clone(),
                None => {
                    let v = state.clk_assign.entry(entry).or_default();
                    if v.len() >= 2 {
                        return Err(format!(
                            "route: {what}: tile {} needs more than 2 global clocks",
                            self.entry_key[entry as usize]
                        ));
                    }
                    let name = if v.is_empty() { "CLK0" } else { "CLK1" }.to_string();
                    v.push((ordinal, name.clone()));
                    name
                }
            };
            slices.push(self.program_mux(entry, &clk_mux_name, &hpbx, &what, state)?);
            let muxclk = format!("MUXCLK{slice}");
            slices.push(self.program_mux(entry, &muxclk, &clk_mux_name, &what, state)?);
            let enum_name = format!("{clk_mux_name}.CLKMUX");
            self.validate_config_enum(entry, &enum_name, "CLK")?;
            config_enums.push(ConfigEnumSetting {
                tile_key: self.entry_key[entry as usize].clone(),
                name: enum_name,
                value: "CLK".to_string(),
            });
        }

        Ok(RoutedClock {
            net: clock.net.clone(),
            dcc: clock.dcc,
            pclk: ordinal,
            dcc_input,
            spine,
            slices,
        })
    }
}

// ---------------------------------------------------------------------------
// Design-level orchestration
// ---------------------------------------------------------------------------

impl<'a> Router<'a> {
    /// Collect per-net endpoints from the placed cells. Also returns
    /// clock-net -> FF loads for the global clock path.
    fn collect_endpoints(
        &mut self,
        model: &'a RouteModel,
    ) -> Result<EndpointMaps<'a>, String> {
        let mut kinds: HashMap<&'a str, NetKind> = HashMap::new();
        for n in &model.nets {
            if kinds.insert(n.name.as_str(), n.kind).is_some() {
                return Err(format!("route: duplicate net '{}'", n.name));
            }
        }
        let mut clock_dccs: HashSet<u8> = HashSet::new();
        for c in &model.clocks {
            if !kinds.contains_key(c.net.as_str()) {
                return Err(format!(
                    "route: clock '{}': net not listed in placed nets",
                    c.net
                ));
            }
            if !clock_dccs.insert(c.dcc) {
                return Err(format!(
                    "route: clock '{}': DCC {} already used by another clock",
                    c.net, c.dcc
                ));
            }
        }
        let clock_set: HashSet<&'a str> =
            model.clocks.iter().map(|c| c.net.as_str()).collect();

        let mut works: HashMap<&'a str, NetWork<'a>> = HashMap::new();
        let mut clock_ff_loads: HashMap<&'a str, Vec<(u32, u8)>> = HashMap::new();

        for cell in &model.cells {
            let entry = find_entry(self, cell.tile_row, cell.tile_col, &cell.bel)?;
            let tile_type = self.device.tiles()[entry as usize].tile_type.clone();
            match cell.kind {
                CellKind::Lut => {
                    if tile_type != "PLC2" {
                        return Err(format!(
                            "route: cell '{}': LUT not in a PLC2 tile ({} @ {})",
                            cell.name,
                            cell.bel,
                            self.entry_key[entry as usize]
                        ));
                    }
                    let _slice = check_slice_bel(cell)?;
                    for pin in &cell.pins {
                        if !kinds.contains_key(pin.net.as_str()) {
                            return Err(format!(
                                "route: cell '{}': net '{}' not listed in placed nets",
                                cell.name, pin.net
                            ));
                        }
                        let work =
                            works.entry(pin.net.as_str()).or_insert_with(|| NetWork {
                                name: pin.net.as_str(),
                                driver: None,
                                loads: Vec::new(),
                            });
                        match pin.pin.as_str() {
                            "A" | "B" | "C" | "D" => {
                                let wire = format!("{}{}", pin.pin, cell.index);
                                let node = self.load_node(
                                    entry,
                                    &wire,
                                    &describe(cell, &pin.pin),
                                )?;
                                work.loads.push((node, describe(cell, &pin.pin)));
                            }
                            "Q" => {
                                if work.driver.is_some() {
                                    return Err(format!(
                                        "route: net '{}' has multiple drivers",
                                        pin.net
                                    ));
                                }
                                let node = self.driver_node(
                                    entry,
                                    &format!("F{}_SLICE", cell.index),
                                    &describe(cell, &pin.pin),
                                )?;
                                work.driver =
                                    Some((node, describe(cell, &pin.pin)));
                            }
                            _ => {
                                return Err(format!(
                                    "route: cell '{}': unknown LUT pin '{}'",
                                    cell.name, pin.pin
                                ))
                            }
                        }
                    }
                }
                CellKind::Ff => {
                    if tile_type != "PLC2" {
                        return Err(format!(
                            "route: cell '{}': FF not in a PLC2 tile ({} @ {})",
                            cell.name,
                            cell.bel,
                            self.entry_key[entry as usize]
                        ));
                    }
                    let slice = check_slice_bel(cell)?;
                    for pin in &cell.pins {
                        if !kinds.contains_key(pin.net.as_str()) {
                            return Err(format!(
                                "route: cell '{}': net '{}' not listed in placed nets",
                                cell.name, pin.net
                            ));
                        }
                        match pin.pin.as_str() {
                            "Q" => {
                                let work = works
                                    .entry(pin.net.as_str())
                                    .or_insert_with(|| NetWork {
                                        name: pin.net.as_str(),
                                        driver: None,
                                        loads: Vec::new(),
                                    });
                                if work.driver.is_some() {
                                    return Err(format!(
                                        "route: net '{}' has multiple drivers",
                                        pin.net
                                    ));
                                }
                                let node = self.driver_node(
                                    entry,
                                    &format!("Q{}", cell.index),
                                    &describe(cell, &pin.pin),
                                )?;
                                work.driver =
                                    Some((node, describe(cell, &pin.pin)));
                            }
                            "CLK" => {
                                if clock_set.contains(pin.net.as_str()) {
                                    clock_ff_loads
                                        .entry(pin.net.as_str())
                                        .or_default()
                                        .push((entry, cell.index));
                                } else {
                                    let work = works
                                        .entry(pin.net.as_str())
                                        .or_insert_with(|| NetWork {
                                            name: pin.net.as_str(),
                                            driver: None,
                                            loads: Vec::new(),
                                        });
                                    let node = self.load_node(
                                        entry,
                                        &format!("MUXCLK{slice}"),
                                        &describe(cell, &pin.pin),
                                    )?;
                                    work.loads.push((node, describe(cell, &pin.pin)));
                                }
                            }
                            "CE" => {
                                let work = works
                                    .entry(pin.net.as_str())
                                    .or_insert_with(|| NetWork {
                                        name: pin.net.as_str(),
                                        driver: None,
                                        loads: Vec::new(),
                                    });
                                let node = self.load_node(
                                    entry,
                                    &format!("CE{slice}"),
                                    &describe(cell, &pin.pin),
                                )?;
                                work.loads.push((node, describe(cell, &pin.pin)));
                            }
                            "LSR" => {
                                let work = works
                                    .entry(pin.net.as_str())
                                    .or_insert_with(|| NetWork {
                                        name: pin.net.as_str(),
                                        driver: None,
                                        loads: Vec::new(),
                                    });
                                let node = self.load_node(
                                    entry,
                                    &format!("MUXLSR{slice}"),
                                    &describe(cell, &pin.pin),
                                )?;
                                work.loads.push((node, describe(cell, &pin.pin)));
                            }
                            "D" => {
                                // The model builder only emits a `D` pin for
                                // unpaired FFs (a paired FF's D is the
                                // dedicated slice connection to its LUT).
                                // Route it from fabric through the `D{index}`
                                // mux.
                                let work = works
                                    .entry(pin.net.as_str())
                                    .or_insert_with(|| NetWork {
                                        name: pin.net.as_str(),
                                        driver: None,
                                        loads: Vec::new(),
                                    });
                                let node = self.load_node(
                                    entry,
                                    &format!("D{}", cell.index),
                                    &describe(cell, &pin.pin),
                                )?;
                                work.loads.push((node, describe(cell, &pin.pin)));
                            }
                            _ => {
                                return Err(format!(
                                    "route: cell '{}': unknown FF pin '{}'",
                                    cell.name, pin.pin
                                ))
                            }
                        }
                    }
                }
                CellKind::IoIn => {
                    let pio = pio_letter(cell)?;
                    for pin in &cell.pins {
                        if !kinds.contains_key(pin.net.as_str()) {
                            return Err(format!(
                                "route: cell '{}': net '{}' not listed in placed nets",
                                cell.name, pin.net
                            ));
                        }
                        match pin.pin.as_str() {
                            "O" => {
                                let work = works
                                    .entry(pin.net.as_str())
                                    .or_insert_with(|| NetWork {
                                        name: pin.net.as_str(),
                                        driver: None,
                                        loads: Vec::new(),
                                    });
                                if work.driver.is_some() {
                                    return Err(format!(
                                        "route: net '{}' has multiple drivers",
                                        pin.net
                                    ));
                                }
                                let node =
                                    self.ioin_driver(entry, pio, &cell.name)?;
                                work.driver =
                                    Some((node, describe(cell, &pin.pin)));
                            }
                            _ => {
                                return Err(format!(
                                    "route: cell '{}': unknown IO input pin '{}'",
                                    cell.name, pin.pin
                                ))
                            }
                        }
                    }
                }
                CellKind::IoOut => {
                    let pio = pio_letter(cell)?;
                    for pin in &cell.pins {
                        if !kinds.contains_key(pin.net.as_str()) {
                            return Err(format!(
                                "route: cell '{}': net '{}' not listed in placed nets",
                                cell.name, pin.net
                            ));
                        }
                        match pin.pin.as_str() {
                            "I" => {
                                let work = works
                                    .entry(pin.net.as_str())
                                    .or_insert_with(|| NetWork {
                                        name: pin.net.as_str(),
                                        driver: None,
                                        loads: Vec::new(),
                                    });
                                let node =
                                    self.ioout_load(entry, pio, &cell.name)?;
                                work.loads.push((node, describe(cell, &pin.pin)));
                            }
                            _ => {
                                return Err(format!(
                                    "route: cell '{}': unknown IO output pin '{}'",
                                    cell.name, pin.pin
                                ))
                            }
                        }
                    }
                }
            }
        }

        // Carry nets: all endpoints must share one PLC2 tile (dedicated
        // cascade); anything else is a pack/place error.
        for (name, work) in &works {
            if kinds[*name] != NetKind::Carry {
                continue;
            }
            let mut entries: HashSet<u32> = HashSet::new();
            if let Some((d, _)) = work.driver {
                entries.insert(self.nodes[d as usize].0);
            }
            for (l, _) in &work.loads {
                entries.insert(self.nodes[*l as usize].0);
            }
            if entries.len() > 1 {
                return Err(format!(
                    "route: carry net '{name}' spans {} tiles (dedicated cascade is per-tile)",
                    entries.len()
                ));
            }
        }

        Ok((kinds, works, clock_ff_loads))
    }

    /// One full attempt: route clocks, then signal nets in `order`.
    /// Returns (routed clocks, net name -> segments, config enums).
    fn try_order(
        &mut self,
        model: &'a RouteModel,
        works: &HashMap<&'a str, NetWork<'a>>,
        clock_ff_loads: &HashMap<&'a str, Vec<(u32, u8)>>,
        order: &[&'a str],
    ) -> Result<ClockRouteOut, String> {
        let mut state = RouteState::default();
        let mut config_enums = Vec::new();
        let mut routed_clocks = Vec::new();
        for (ord, clock) in model.clocks.iter().enumerate() {
            let ff: &[(u32, u8)] = clock_ff_loads
                .get(clock.net.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            routed_clocks.push(self.route_clock(
                clock,
                ord as u8,
                ff,
                &mut state,
                &mut config_enums,
            )?);
        }
        let mut seg_map: HashMap<String, Vec<RoutedSegment>> = HashMap::new();
        for name in order {
            let net = &works[*name];
            // Nets with no driver and no loads need nothing (e.g. a
            // PLL-driven clock with no fabric loads).
            if net.driver.is_none() && net.loads.is_empty() {
                seg_map.insert(net.name.to_string(), Vec::new());
                continue;
            }
            let segs = self.route_signal_net(net, &mut state)?;
            seg_map.insert(net.name.to_string(), segs);
        }
        Ok((routed_clocks, seg_map, config_enums))
    }

    /// Route the whole design: clocks first, then signal nets ordered by
    /// decreasing fanout (one rip-up retry in reverse order on failure).
    fn route_design(&mut self, model: &'a RouteModel) -> Result<RoutedEcp5, String> {
        let (kinds, works, clock_ff_loads) = self.collect_endpoints(model)?;

        let mut sig_names: Vec<&'a str> = works
            .keys()
            .filter(|n| kinds[*n] == NetKind::Signal)
            .copied()
            .collect();
        sig_names.sort_by_key(|n| (std::cmp::Reverse(works[*n].loads.len()), *n));

        let (routed_clocks, seg_map, config_enums) =
            match self.try_order(model, &works, &clock_ff_loads, &sig_names) {
                Ok(r) => r,
                Err(_) => {
                    // Rip-up retry: reverse net order.
                    let mut rev = sig_names.clone();
                    rev.reverse();
                    self.try_order(model, &works, &clock_ff_loads, &rev)?
                }
            };

        let mut nets = Vec::with_capacity(model.nets.len());
        for pn in &model.nets {
            let segments = match pn.kind {
                NetKind::Signal => seg_map.get(&pn.name).cloned().unwrap_or_default(),
                // Carry/const nets: validated/collected above, no segments.
                NetKind::Carry | NetKind::Const0 | NetKind::Const1 => Vec::new(),
            };
            nets.push(RoutedNet {
                name: pn.name.clone(),
                kind: pn.kind,
                segments,
            });
        }

        Ok(RoutedEcp5 {
            top: model.top.clone(),
            nets,
            clocks: routed_clocks,
            config_enums,
        })
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// place::PlacedEcp5 -> RouteModel bridge
// ---------------------------------------------------------------------------

/// Slice index (0..3) from a Trellis slice site name (`"SLICE_A"`..`"SLICE_D"`).
fn slice_index(site: &str, what: &str) -> Result<u8, String> {
    match site {
        "SLICE_A" => Ok(0),
        "SLICE_B" => Ok(1),
        "SLICE_C" => Ok(2),
        "SLICE_D" => Ok(3),
        _ => Err(format!("route: {what}: site '{site}' is not a slice")),
    }
}

/// PIO letter from a PIO site name (`"PIOA"` -> `'A'`).
fn pio_letter_of(site: &str, what: &str) -> Result<char, String> {
    match site {
        "PIOA" => Ok('A'),
        "PIOB" => Ok('B'),
        "PIOC" => Ok('C'),
        "PIOD" => Ok('D'),
        _ => Err(format!("route: {what}: site '{site}' is not a PIO")),
    }
}

/// Classify a net for the model. Nets starting with
/// [`CARRY_NET_PREFIX`] are dedicated CCU2 wires: `$carry$zero`/`$carry$one`
/// are the chain-head constant carry-ins, the rest are carry-cascade nets.
/// The router never routes fabric to any of them.
fn classify_net(name: &str) -> NetKind {
    if name == "$carry$zero" {
        NetKind::Const0
    } else if name == "$carry$one" {
        NetKind::Const1
    } else if name.starts_with(CARRY_NET_PREFIX) {
        NetKind::Carry
    } else {
        NetKind::Signal
    }
}

impl Router<'_> {
    /// Input wires of the `G_TDCC{dcc}CLKI` mux in the `TMID_0` tile,
    /// straight from the vendored `bits.db` (no hardcoded table).
    fn dcc_clki_inputs(&self, dcc: u8) -> Result<Vec<String>, String> {
        let entry = self.tmid0_entry()?;
        let g = self.graph(entry);
        let dst = format!("G_TDCC{dcc}CLKI");
        let mi = g.mux_by_dst.get(&dst).ok_or_else(|| {
            format!("route: TMID_0 tile has no mux {dst}")
        })?;
        Ok(g.muxes[*mi].inputs.iter().map(|i| i.src.clone()).collect())
    }
}

/// The DCC input wire for a clock net, derived from the clock's driver.
///
/// * Input pad: the pad's `pio_metadata` `function` names its PCLK tap
///   (e.g. `"PCLKT0_1"`); the DCC input wire is `G_JPCLKT01`. Only `PCLKT`
///   pins are supported in v1 (other PCLK functions fail loud).
/// * Anything else (PLL output, fabric-driven clock) fails loud: PLL
///   reference-clock routing is not modeled in v1, and v1 cannot feed a
///   fabric net into a DCC input mux.
fn clock_source_wire(
    device: &Device,
    placed: &place::PlacedEcp5,
    net: &str,
) -> Result<String, String> {
    // Through a ClockBuf, the pad drives the buffer's input net.
    let mut pad_net = net;
    if let Some(cb) = placed.packed.clks.iter().find(|c| c.out_net == net) {
        pad_net = cb.in_net.as_str();
    }
    if let Some(io) = placed
        .ios
        .iter()
        .find(|io| io.dir == crate::pack::IoDir::In && io.net == pad_net)
    {
        // pio_metadata is keyed by the package pin's (col, pio, row), the
        // same key the pack worker uses.
        let pkg_pins = device.package_pins(&placed.package).ok_or_else(|| {
            format!(
                "route: clock pad {}: unknown package {:?}",
                io.name, placed.package
            )
        })?;
        let pp = pkg_pins.get(&io.pin).ok_or_else(|| {
            format!(
                "route: clock pad {}: pin {} not in package {:?}",
                io.name, io.pin, placed.package
            )
        })?;
        let meta = device
            .pio_metadata()
            .iter()
            .find(|m| m.col == pp.col && m.row == pp.row && m.pio == pp.pio)
            .ok_or_else(|| {
                format!(
                    "route: clock pad {} (pin {}) has no pio_metadata row",
                    io.name, io.pin
                )
            })?;
        let func = meta.function.as_deref().ok_or_else(|| {
            format!(
                "route: clock pad {} (pin {}) is not a PCLK-capable pin",
                io.name, io.pin
            )
        })?;
        let tap = func.strip_prefix("PCLKT").ok_or_else(|| {
            format!(
                "route: clock pad {} (pin {}): PCLK function {func:?} is not PCLKT (v1 limitation)",
                io.name, io.pin
            )
        })?;
        let (a, b) = tap.split_once('_').ok_or_else(|| {
            format!(
                "route: clock pad {} (pin {}): bad PCLKT function {func:?}",
                io.name, io.pin
            )
        })?;
        return Ok(format!("G_JPCLKT{a}{b}"));
    }
    // (PLL outputs: PLL blocks fail loud in build_model, since their
    // reference-clock input has no v1 routing model.)
    Err(format!(
        "route: clock '{net}': v1 needs an input-pad driver for DCC promotion"
    ))
}

/// Lower [`place::PlacedEcp5`] to the router's [`RouteModel`].
///
/// Fail-loud v1 limits: EBR/DSP/PLL blocks have no routing model yet
/// (their nets cannot be routed), and clocks need a pad/PLL driver with a
/// DCC input wire resolvable from the DB.
fn build_model(router: &Router, placed: &place::PlacedEcp5) -> Result<RouteModel, String> {
    if !placed.ebrs.is_empty() {
        return Err(format!(
            "route: {} EBR block(s): hard-block routing is not supported in v1",
            placed.ebrs.len()
        ));
    }
    if !placed.dsps.is_empty() {
        return Err(format!(
            "route: {} DSP block(s): hard-block routing is not supported in v1",
            placed.dsps.len()
        ));
    }
    if !placed.plls.is_empty() {
        return Err(format!(
            "route: {} PLL block(s): PLL reference-clock routing is not supported in v1",
            placed.plls.len()
        ));
    }

    let mut cells: Vec<PlacedCell> = Vec::new();
    let mut net_names: HashSet<String> = HashSet::new();

    // LUTs.
    for lut in &placed.luts {
        if lut.half > 1 {
            return Err(format!(
                "route: LUT {}: bad half {} (want 0/1)",
                lut.name, lut.half
            ));
        }
        let slice = slice_index(&lut.site.site, &format!("LUT {}", lut.name))?;
        let index = slice * 2 + lut.half;
        let mut pins = Vec::new();
        for (i, net) in lut.inputs.iter().enumerate() {
            if let Some(n) = net {
                // Dedicated CCU2 carry wires are listed but get no fabric
                // pin: the router must skip them.
                net_names.insert(n.clone());
                if n.starts_with(CARRY_NET_PREFIX) {
                    continue;
                }
                let pin = ["A", "B", "C", "D"][i].to_string();
                pins.push(CellPin {
                    pin,
                    net: n.clone(),
                });
                net_names.insert(n.clone());
            }
        }
        pins.push(CellPin {
            pin: "Q".to_string(),
            net: lut.output.clone(),
        });
        net_names.insert(lut.output.clone());
        cells.push(PlacedCell {
            name: lut.name.clone(),
            kind: CellKind::Lut,
            tile_row: lut.site.tile_row,
            tile_col: lut.site.tile_col,
            bel: lut.site.site.clone(),
            index,
            pins,
        });
    }

    // FFs.
    for ff in &placed.ffs {
        if ff.half > 1 {
            return Err(format!(
                "route: FF {}: bad half {} (want 0/1)",
                ff.name, ff.half
            ));
        }
        let slice = slice_index(&ff.site.site, &format!("FF {}", ff.name))?;
        let index = slice * 2 + ff.half;
        // Paired with the driver LUT in the same half-slice: D is the
        // dedicated slice connection and needs no routing. Otherwise D
        // comes from fabric through the D{index} mux.
        let paired = placed.luts.iter().any(|l| {
            l.site == ff.site && l.half == ff.half && l.output == ff.d
        });
        let mut pins = vec![
            CellPin {
                pin: "Q".to_string(),
                net: ff.q.clone(),
            },
            CellPin {
                pin: "CLK".to_string(),
                net: ff.clk.clone(),
            },
        ];
        net_names.insert(ff.q.clone());
        net_names.insert(ff.clk.clone());
        net_names.insert(ff.d.clone());
        if !paired {
            pins.push(CellPin {
                pin: "D".to_string(),
                net: ff.d.clone(),
            });
        }
        cells.push(PlacedCell {
            name: ff.name.clone(),
            kind: CellKind::Ff,
            tile_row: ff.site.tile_row,
            tile_col: ff.site.tile_col,
            bel: ff.site.site.clone(),
            index,
            pins,
        });
    }

    // IOs.
    for io in &placed.ios {
        let pio = pio_letter_of(&io.site.site, &format!("IO {}", io.name))?;
        let _ = pio;
        let (kind, pin) = match io.dir {
            crate::pack::IoDir::In => (CellKind::IoIn, "O"),
            crate::pack::IoDir::Out => (CellKind::IoOut, "I"),
        };
        cells.push(PlacedCell {
            name: io.name.clone(),
            kind,
            tile_row: io.site.tile_row,
            tile_col: io.site.tile_col,
            bel: io.site.site.clone(),
            index: 0,
            pins: vec![CellPin {
                pin: pin.to_string(),
                net: io.net.clone(),
            }],
        });
        net_names.insert(io.net.clone());
    }

    // Clocks: resolve each to a (dcc, source_wire), preferring place's DCC
    // assignment, then the lowest free DCC whose CLKI mux lists the wire.
    let mut used_dccs: HashSet<u8> = HashSet::new();
    let mut clocks = Vec::with_capacity(placed.clocks.len());
    for clock in &placed.clocks {
        let wire = clock_source_wire(router.device, placed, &clock.net)?;
        let preferred: Option<u8> = clock
            .dcc
            .strip_prefix("DCC")
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|&n| n <= 5);
        let mut candidates: Vec<u8> = Vec::new();
        if let Some(d) = preferred {
            candidates.push(d);
        }
        for d in 0..6u8 {
            if Some(d) != preferred {
                candidates.push(d);
            }
        }
        let mut picked = None;
        for dcc in candidates {
            if used_dccs.contains(&dcc) {
                continue;
            }
            if router.dcc_clki_inputs(dcc)?.iter().any(|s| s == &wire) {
                picked = Some(dcc);
                break;
            }
        }
        let dcc = picked.ok_or_else(|| {
            format!(
                "route: clock '{}': source wire '{wire}' is an input of no DCC0..5 CLKI mux",
                clock.net
            )
        })?;
        used_dccs.insert(dcc);
        clocks.push(PlacedClock {
            net: clock.net.clone(),
            dcc,
            source_wire: wire,
        });
        net_names.insert(clock.net.clone());
    }

    let mut nets: Vec<PlacedNet> = net_names
        .into_iter()
        .map(|name| {
            let kind = classify_net(&name);
            PlacedNet { name, kind }
        })
        .collect();
    nets.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(RouteModel {
        top: placed.top.clone(),
        cells,
        nets,
        clocks,
    })
}

/// Route a placed ECP5 design on the process-wide device model.
pub fn route(placed: &place::PlacedEcp5) -> Result<RoutedEcp5, String> {
    let device = Device::global()?;
    route_on_device(device, placed)
}

/// Route a placed ECP5 design on an explicit device model.
pub fn route_on_device(
    device: &Device,
    placed: &place::PlacedEcp5,
) -> Result<RoutedEcp5, String> {
    let mut router = Router::new(device)?;
    let model = build_model(&router, placed)?;
    router.route_design(&model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::parse_bits_db;

    fn dev() -> &'static Device {
        Device::global().unwrap()
    }

    /// (row, col) of the first PLC2 tile.
    fn plc2_rc(d: &Device) -> (u32, u32) {
        let t = d
            .tiles()
            .iter()
            .find(|t| t.tile_type == "PLC2")
            .unwrap();
        (t.row, t.col)
    }

    /// Site address for a slice of the PLC2 tile at `(row, col)`, with bel
    /// coordinates from the device model.
    fn slice_site(d: &Device, row: u32, col: u32, slice: &str) -> place::SiteAddr {
        let t = d
            .tiles()
            .iter()
            .find(|t| t.row == row && t.col == col && t.tile_type == "PLC2")
            .unwrap();
        let s = t.sites.iter().find(|s| s.name == slice).unwrap();
        place::SiteAddr {
            tile_row: row,
            tile_col: col,
            tile_type: "PLC2".to_string(),
            site: slice.to_string(),
            bel_col: s.bel_col,
            bel_row: s.bel_row,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn placed_lut(
        d: &Device,
        name: &str,
        row: u32,
        col: u32,
        slice: &str,
        half: u8,
        inputs: [Option<&str>; 4],
        output: &str,
    ) -> place::PlacedLut {
        place::PlacedLut {
            name: name.to_string(),
            init: 0xAAAA,
            inputs: inputs.map(|n| n.map(str::to_string)),
            output: output.to_string(),
            carry: None,
            site: slice_site(d, row, col, slice),
            half,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn placed_ff(
        d: &Device,
        name: &str,
        row: u32,
        col: u32,
        slice: &str,
        half: u8,
        dm: &str,
        q: &str,
        clk: &str,
    ) -> place::PlacedFf {
        place::PlacedFf {
            name: name.to_string(),
            d: dm.to_string(),
            q: q.to_string(),
            clk: clk.to_string(),
            ce: None,
            sr: None,
            site: slice_site(d, row, col, slice),
            half,
        }
    }

    /// A real PCLK-capable package pin and its PIO site address:
    /// (pin name, site). Uses pin A10 (`PCLKT0_1`): the bridge derives the
    /// DCC input wire `G_JPCLKT01`, a real input of the `G_TDCC0CLKI` mux.
    fn pclk_pio(d: &Device) -> (String, place::SiteAddr) {
        let meta = d
            .pio_metadata()
            .iter()
            .find(|m| m.function.as_deref() == Some("PCLKT0_1"))
            .unwrap();
        let (pin_name, _) = d
            .package_pins("CABGA381")
            .unwrap()
            .iter()
            .find(|(_, p)| p.col == meta.col && p.row == meta.row && p.pio == meta.pio)
            .unwrap();
        let (site, _) = pio_site_for_pin(d, pin_name);
        (pin_name.clone(), site)
    }

    /// A PIO site for an arbitrary (non-clock) package pin.
    fn pio_site_for_pin(d: &Device, pin: &str) -> (place::SiteAddr, String) {
        let (_, p) = d
            .package_pins("CABGA381")
            .unwrap()
            .iter()
            .find(|(name, _)| name == &pin)
            .unwrap();
        let pio_name = format!("PIO{}", p.pio);
        // Bel position from the place worker's rule (mirrors
        // place::pio_bel_candidates for the common edges).
        let (bc, br) = if p.row == 0 {
            let idx = match p.pio.as_str() {
                "A" => 0,
                "B" => 1,
                "C" => 2,
                _ => 3,
            };
            (p.col as i32, 19 - idx)
        } else if p.row == 95 {
            let idx = match p.pio.as_str() {
                "A" => 0,
                "B" => 1,
                "C" => 2,
                _ => 3,
            };
            (p.col as i32, 114 - idx)
        } else {
            panic!("test helper only handles top/bottom edge pins");
        };
        let t = d
            .tiles()
            .iter()
            .find(|t| {
                t.sites
                    .iter()
                    .any(|s| s.name == pio_name && s.bel_col == bc && s.bel_row == br)
            })
            .unwrap();
        let s = t
            .sites
            .iter()
            .find(|s| s.name == pio_name && s.bel_col == bc && s.bel_row == br)
            .unwrap();
        (
            place::SiteAddr {
                tile_row: t.row,
                tile_col: t.col,
                tile_type: t.tile_type.clone(),
                site: s.name.clone(),
                bel_col: s.bel_col,
                bel_row: s.bel_row,
            },
            pio_name,
        )
    }

    fn placed_io(
        d: &Device,
        name: &str,
        port: &str,
        dir: crate::pack::IoDir,
        net: &str,
        pin: &str,
    ) -> place::PlacedIo {
        let (site, _) = pio_site_for_pin(d, pin);
        place::PlacedIo {
            name: name.to_string(),
            port: port.to_string(),
            dir,
            net: net.to_string(),
            pin: pin.to_string(),
            site,
        }
    }

    fn placed_io_on_site(
        name: &str,
        port: &str,
        dir: crate::pack::IoDir,
        net: &str,
        pin: &str,
        site: place::SiteAddr,
    ) -> place::PlacedIo {
        place::PlacedIo {
            name: name.to_string(),
            port: port.to_string(),
            dir,
            net: net.to_string(),
            pin: pin.to_string(),
            site,
        }
    }

    fn clock_assign(net: &str, dcc: &str) -> place::ClockAssignment {
        place::ClockAssignment {
            net: net.to_string(),
            dcc: dcc.to_string(),
            // Unused by the route bridge (it derives everything from the
            // net's driver); filled for struct completeness.
            site: place::SiteAddr {
                tile_row: 0,
                tile_col: 0,
                tile_type: "TMID_0".to_string(),
                site: "DCC0".to_string(),
                bel_col: 0,
                bel_row: 0,
            },
            quadrant: "UL".to_string(),
            tap: "C58".to_string(),
            spine: "UL58".to_string(),
        }
    }

    /// Validate one routed segment against the canonical DB parser
    /// (`db::parse_bits_db`), not against the router's own structures.
    fn validate_segment(d: &Device, seg: &RoutedSegment) -> Result<(), String> {
        // Tile key format: "[REGION_]R<row>C<col>:<type>".
        let ty = seg
            .tile_key
            .rsplit(':')
            .next()
            .ok_or_else(|| format!("bad tile key {}", seg.tile_key))?;
        let text = crate::tiledata::bits_db(ty)
            .ok_or_else(|| format!("no bits.db for {ty}"))?;
        // Use the router's parser (keeps `-` default inputs); db.rs drops
        // them, which would false-positive on e.g. F0 <- F0_SLICE.
        let tb = parse_muxes(text);
        let m = tb
            .iter()
            .find(|m| m.dst == seg.dst)
            .ok_or_else(|| format!("DB has no mux {} in {ty}", seg.dst))?;
        let inp = m
            .inputs
            .iter()
            .find(|i| i.src == seg.src)
            .ok_or_else(|| {
                format!("DB mux {} has no input {} in {ty}", seg.dst, seg.src)
            })?;
        // `-` inputs were dropped by db.rs; the router records empty bits.
        if seg.bits.is_empty() {
            return Ok(());
        }
        if inp.bits != seg.bits {
            return Err(format!(
                "bit mismatch for {} <- {} in {ty}",
                seg.dst, seg.src
            ));
        }
        // The tile key's row/col must match the segment's.
        let rc = seg.tile_key.split(':').next().unwrap_or("");
        let rc = rc.split('_').last().unwrap_or(rc);
        let rc = rc.strip_prefix('R').unwrap_or(rc);
        let (r, c) = rc.split_once('C').unwrap_or(("", ""));
        let (pr, pc): (u32, u32) = (
            r.parse().map_err(|_| "bad row".to_string())?,
            c.parse().map_err(|_| "bad col".to_string())?,
        );
        if pr != seg.tile_row || pc != seg.tile_col {
            return Err(format!("tile key/row-col mismatch: {}", seg.tile_key));
        }
        let _ = d;
        Ok(())
    }

    fn validate_routed(d: &Device, r: &RoutedEcp5) -> Result<(), String> {
        for n in &r.nets {
            for s in &n.segments {
                validate_segment(d, s)?;
            }
        }
        for c in &r.clocks {
            validate_segment(d, &c.dcc_input)?;
            for s in &c.spine {
                validate_segment(d, s)?;
            }
            for s in &c.slices {
                validate_segment(d, s)?;
            }
        }
        Ok(())
    }

    #[test]
    fn wire_ref_parses() {
        let r = parse_wire_ref("E1_H01E0001");
        assert_eq!((r.dr, r.dc), (0, 1));
        assert_eq!(r.wire, "H01E0001");
        let r = parse_wire_ref("W3_H06W0103");
        assert_eq!((r.dr, r.dc), (0, -3));
        assert_eq!(r.wire, "H06W0103");
        let r = parse_wire_ref("N1W1_JDIB");
        assert_eq!((r.dr, r.dc), (-1, -1));
        assert_eq!(r.wire, "JDIB");
        let r = parse_wire_ref("S13E2_X");
        assert_eq!((r.dr, r.dc), (13, 2));
        assert_eq!(r.wire, "X");
        let r = parse_wire_ref("N10W2_Z");
        assert_eq!((r.dr, r.dc), (-10, -2));
        // No prefix: anchored locally.
        let r = parse_wire_ref("G_HPBX0000");
        assert_eq!((r.dr, r.dc), (0, 0));
        assert_eq!(r.wire, "G_HPBX0000");
        let r = parse_wire_ref("F0_SLICE");
        assert_eq!((r.dr, r.dc), (0, 0));
        assert_eq!(r.wire, "F0_SLICE");
        let r = parse_wire_ref("A0");
        assert_eq!((r.dr, r.dc), (0, 0));
        // Bounce wires are not prefixes.
        let r = parse_wire_ref("EBOUNCE");
        assert_eq!((r.dr, r.dc), (0, 0));
    }

    #[test]
    fn mux_parser_agrees_with_db_rs() {
        // The router's mux parser must match db::parse_bits_db on every
        // tile type, with two deliberate deviations:
        // 1. `-` (default) inputs are kept with empty bits (db.rs drops them).
        // 2. `.config ` sections are treated as section boundaries (db.rs
        //    absorbs their bare bit lines as phantom mux inputs).
        for tt in crate::tiledata::TILE_TYPES {
            let text = crate::tiledata::bits_db(tt).unwrap();
            let mine = parse_muxes(text);
            let canon = parse_bits_db(text);
            assert_eq!(mine.len(), canon.muxes.len(), "mux count for {tt}");
            for (m, c) in mine.iter().zip(canon.muxes.iter()) {
                assert_eq!(m.dst, c.dst, "dst for {tt}");
                for inp in &m.inputs {
                    let ci = c.inputs.iter().find(|i| i.src == inp.src);
                    match ci {
                        Some(ci) => {
                            assert_eq!(
                                inp.bits, ci.bits,
                                "bits for {} <- {} ({tt})",
                                m.dst, inp.src
                            );
                        }
                        None => {
                            // Must be a `-` default (empty bits, real wire).
                            assert!(
                                inp.bits.is_empty(),
                                "{}: input {} missing in db.rs ({tt})",
                                m.dst,
                                inp.src
                            );
                        }
                    }
                }
                // Every db.rs input must be present in mine, except the
                // `.config` phantom inputs (bare bit tokens or bare `-`,
                // not wires).
                for ci in &c.inputs {
                    let is_phantom = ci.bits.is_empty()
                        && (ci.src == "-" || parse_bitreq(&ci.src).is_some());
                    if is_phantom {
                        continue;
                    }
                    let mi = m
                        .inputs
                        .iter()
                        .find(|i| i.src == ci.src)
                        .unwrap_or_else(|| {
                            panic!("{}: input {} missing in router ({tt})", m.dst, ci.src)
                        });
                    assert_eq!(mi.bits, ci.bits);
                }
            }
        }
    }

    #[test]
    fn router_builds_over_full_device() {
        let d = dev();
        let r = Router::new(d).unwrap();
        assert_eq!(r.entry_count(), 14231);
        assert_eq!(r.node_count(), 0);
        // Prefix offsets cover the observed directional shapes.
        assert!(r.prefix_offsets.contains(&(0, 1))); // E1
        assert!(r.prefix_offsets.contains(&(0, -3))); // W3
        assert!(r.prefix_offsets.contains(&(-1, -1))); // N1W1
    }

    /// Small multi-LUT design: 2 LUTs + FF + IO, local clock. Routes 100%
    /// of nets; every segment validates against bits.db.
    ///
    /// `f0` shares its half-slice with its driver LUT `l1`, so `n2` needs
    /// no fabric segments (dedicated slice connection). `clocks` is empty,
    /// so the clock routes as an ordinary fabric net to `MUXCLK0`.
    #[test]
    fn route_small_design() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            luts: vec![
                placed_lut(d, "l0", pr, pc, "SLICE_A", 0, [Some("a"), Some("b"), None, None], "n1"),
                placed_lut(d, "l1", pr, pc, "SLICE_A", 1, [Some("n1"), Some("c"), None, None], "n2"),
            ],
            ffs: vec![placed_ff(d, "f0", pr, pc, "SLICE_A", 1, "n2", "q", "clk")],
            ios: vec![
                placed_io(d, "ina", "a", crate::pack::IoDir::In, "a", "A10"),
                placed_io(d, "inb", "b", crate::pack::IoDir::In, "b", "A11"),
                placed_io(d, "inc", "c", crate::pack::IoDir::In, "c", "A12"),
                placed_io(d, "clkpad", "clk", crate::pack::IoDir::In, "clk", "A13"),
                placed_io(d, "outq", "q", crate::pack::IoDir::Out, "q", "A14"),
            ],
            clocks: vec![],
            ..Default::default()
        };
        let r = route_on_device(d, &placed).unwrap();
        assert_eq!(r.top, "top");
        assert_eq!(r.nets.len(), 7);
        // Every signal net with fabric loads got segments; 100% routed.
        // (n2 drives only f0.D, which is a dedicated slice connection, so
        // it needs no fabric segments.)
        for n in &r.nets {
            if n.name == "n2" {
                assert!(n.segments.is_empty());
                continue;
            }
            assert!(
                !n.segments.is_empty(),
                "net '{}' routed with no segments",
                n.name
            );
        }
        validate_routed(d, &r).unwrap();
        // The local clock reached the slice clock mux.
        let clk = r.nets.iter().find(|n| n.name == "clk").unwrap();
        assert!(
            clk.segments.iter().any(|s| s.dst == "MUXCLK0"),
            "local clock did not reach MUXCLK0"
        );
    }

    /// Global clock: DCC input mux, quadrant PCLK spines, slice CLK muxes.
    ///
    /// The bridge derives the DCC input wire from the clock pad's PCLK
    /// function (`PCLKT0_1` -> `G_JPCLKT01`) and picks DCC0, whose CLKI mux
    /// lists it.
    #[test]
    fn global_clock_uses_dcc_and_spine() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let (clk_pin, clk_site) = pclk_pio(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            luts: vec![
                placed_lut(d, "d0src", pr, pc, "SLICE_A", 0, [Some("a"), None, None, None], "d0"),
            ],
            ffs: vec![placed_ff(d, "f0", pr, pc, "SLICE_A", 0, "d0", "q", "gclk")],
            ios: vec![
                placed_io_on_site("clkpad", "gclk", crate::pack::IoDir::In, "gclk", &clk_pin, clk_site),
                placed_io(d, "ina", "a", crate::pack::IoDir::In, "a", "A12"),
            ],
            clocks: vec![clock_assign("gclk", "DCC0")],
            ..Default::default()
        };
        let r = route_on_device(d, &placed).unwrap();
        assert_eq!(r.clocks.len(), 1);
        let c = &r.clocks[0];
        assert_eq!(c.dcc, 0);
        assert_eq!(c.pclk, 0);
        // DCC input mux.
        assert_eq!(c.dcc_input.dst, "G_TDCC0CLKI");
        assert_eq!(c.dcc_input.src, "G_JPCLKT01");
        assert!(c.dcc_input.tile_key.ends_with(":TMID_0"));
        // Four quadrant PCLK spines, all from DCC0's output wire.
        assert_eq!(c.spine.len(), 4);
        let mut quadrants: Vec<&str> = c.spine.iter().map(|s| s.dst.as_str()).collect();
        quadrants.sort();
        assert_eq!(quadrants, vec!["G_LLPCLK0", "G_LRPCLK0", "G_ULPCLK0", "G_URPCLK0"]);
        for s in &c.spine {
            assert_eq!(s.src, "G_VPFS0000");
        }
        // Slice takes the global spine through CLK0.
        assert!(c.slices.iter().any(|s| s.dst == "CLK0" && s.src == "G_HPBX0000"));
        assert!(c.slices.iter().any(|s| s.dst == "MUXCLK0" && s.src == "CLK0"));
        assert!(r.config_enums.iter().any(|e| e.name == "CLK0.CLKMUX" && e.value == "CLK"));
        validate_routed(d, &r).unwrap();
        // The clock net itself needs no fabric segments (no fabric loads).
        let gclk = r.nets.iter().find(|n| n.name == "gclk").unwrap();
        assert!(gclk.segments.is_empty());
    }

    /// Two nets contending for one mux: the loser fails loud, naming it.
    #[test]
    fn congestion_fails_loud() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        // Two FFs in the same slice share MUXCLK0; their clocks are
        // different local nets driven by different pads (`clocks` is
        // empty, so both route as fabric).
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            ffs: vec![
                placed_ff(d, "f0", pr, pc, "SLICE_A", 0, "d0", "q0", "clk1"),
                placed_ff(d, "f1", pr, pc, "SLICE_A", 1, "d1", "q1", "clk2"),
            ],
            ios: vec![
                placed_io(d, "p1", "clk1", crate::pack::IoDir::In, "clk1", "A10"),
                placed_io(d, "p2", "clk2", crate::pack::IoDir::In, "clk2", "A11"),
                placed_io(d, "pd0", "d0", crate::pack::IoDir::In, "d0", "A12"),
                placed_io(d, "pd1", "d1", crate::pack::IoDir::In, "d1", "A13"),
            ],
            clocks: vec![],
            ..Default::default()
        };
        let err = route_on_device(d, &placed).unwrap_err();
        // One of the two clock nets must lose the MUXCLK0 mux.
        assert!(
            err.contains("'clk1'") || err.contains("'clk2'"),
            "unexpected error: {err}"
        );
    }

    /// Net with loads but no driver fails loud, naming the net.
    #[test]
    fn driverless_net_fails_loud() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            luts: vec![
                placed_lut(d, "l0", pr, pc, "SLICE_A", 0, [Some("a"), None, None, None], "n1"),
            ],
            ..Default::default()
        };
        let err = route_on_device(d, &placed).unwrap_err();
        assert!(err.contains("'a'"), "unexpected error: {err}");
    }

    /// Carry nets stay on the dedicated cascade: the bridge skips
    /// `$carry$` nets on LUT inputs (no fabric loads), so they route with
    /// no segments. `$carry$zero`/`$carry$one` classify as const nets.
    #[test]
    fn carry_nets_use_dedicated_cascade() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            luts: vec![
                placed_lut(
                    d, "l0", pr, pc, "SLICE_A", 0,
                    [Some("a0"), Some("$carry$zero"), None, None], "s0",
                ),
                placed_lut(
                    d, "l1", pr, pc, "SLICE_A", 1,
                    [Some("a1"), Some("$carry$c0b0"), None, None], "s1",
                ),
            ],
            ios: vec![
                placed_io(d, "pa0", "a0", crate::pack::IoDir::In, "a0", "A10"),
                placed_io(d, "pa1", "a1", crate::pack::IoDir::In, "a1", "A11"),
                placed_io(d, "ps0", "s0", crate::pack::IoDir::Out, "s0", "A12"),
                placed_io(d, "ps1", "s1", crate::pack::IoDir::Out, "s1", "A13"),
            ],
            ..Default::default()
        };
        let r = route_on_device(d, &placed).unwrap();
        let cy0 = r.nets.iter().find(|n| n.name == "$carry$c0b0").unwrap();
        assert_eq!(cy0.kind, NetKind::Carry);
        assert!(cy0.segments.is_empty());
        let zero = r.nets.iter().find(|n| n.name == "$carry$zero").unwrap();
        assert_eq!(zero.kind, NetKind::Const0);
        assert!(zero.segments.is_empty());
        // No segment was programmed for a carry input: the router never
        // touched the dedicated cascade.
        for n in &r.nets {
            for s in &n.segments {
                assert!(
                    !s.dst.starts_with('B'),
                    "carry input got fabric routing: {} <- {}",
                    s.dst, s.src
                );
            }
        }
        validate_routed(d, &r).unwrap();
    }

    /// A clock with no input-pad driver cannot be promoted to the DCC
    /// network: fail loud, naming the net.
    #[test]
    fn clock_without_pad_driver_fails_loud() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            ffs: vec![placed_ff(d, "f0", pr, pc, "SLICE_A", 0, "d0", "q", "gclk")],
            ios: vec![
                placed_io(d, "pd0", "d0", crate::pack::IoDir::In, "d0", "A10"),
            ],
            clocks: vec![clock_assign("gclk", "DCC0")],
            ..Default::default()
        };
        let err = route_on_device(d, &placed).unwrap_err();
        assert!(err.contains("'gclk'"), "unexpected: {err}");
    }

    /// A clock pad that is not PCLK-capable cannot feed a DCC: fail loud.
    /// (Pin A12 has no PCLK function in the Trellis DB.)
    #[test]
    fn non_pclk_clock_pin_fails_loud() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            ffs: vec![placed_ff(d, "f0", pr, pc, "SLICE_A", 0, "d0", "q", "gclk")],
            ios: vec![
                placed_io(d, "clkpad", "gclk", crate::pack::IoDir::In, "gclk", "A12"),
                placed_io(d, "pd0", "d0", crate::pack::IoDir::In, "d0", "A10"),
            ],
            clocks: vec![clock_assign("gclk", "DCC0")],
            ..Default::default()
        };
        let err = route_on_device(d, &placed).unwrap_err();
        assert!(err.contains("PCLK"), "unexpected: {err}");
    }

    /// An unpaired FF (D driven by another FF's Q, not a LUT in its
    /// half-slice) gets its D routed from fabric through the `D{index}`
    /// mux.
    #[test]
    fn unpaired_ff_d_routes_from_fabric() {
        let d = dev();
        let (pr, pc) = plc2_rc(d);
        let placed = place::PlacedEcp5 {
            top: "top".to_string(),
            package: "CABGA381".to_string(),
            // f1.D = f0.Q: no LUT drives it in f1's half-slice, so f1 is
            // unpaired and D1 must come from fabric.
            ffs: vec![
                placed_ff(d, "f0", pr, pc, "SLICE_A", 0, "d0", "q0", "clk"),
                placed_ff(d, "f1", pr, pc, "SLICE_A", 1, "q0", "q1", "clk"),
            ],
            ios: vec![
                placed_io(d, "pd0", "d0", crate::pack::IoDir::In, "d0", "A10"),
                placed_io(d, "pclk", "clk", crate::pack::IoDir::In, "clk", "A11"),
                placed_io(d, "pq1", "q1", crate::pack::IoDir::Out, "q1", "A12"),
            ],
            clocks: vec![],
            ..Default::default()
        };
        let r = route_on_device(d, &placed).unwrap();
        let q0 = r.nets.iter().find(|n| n.name == "q0").unwrap();
        assert!(
            q0.segments.iter().any(|s| s.dst == "D1"),
            "unpaired FF D did not route from fabric: {:?}",
            q0.segments.iter().map(|s| &s.dst).collect::<Vec<_>>()
        );
        validate_routed(d, &r).unwrap();
    }

    /// Hard blocks have no v1 routing model: fail loud, not silent.
    #[test]
    fn hard_blocks_fail_loud() {
        let d = dev();
        let dummy_site = || place::SiteAddr {
            tile_row: 0,
            tile_col: 0,
            tile_type: "MIB_EBR0".to_string(),
            site: "EBR0".to_string(),
            bel_col: 0,
            bel_row: 0,
        };
        for (what, mut placed) in [
            (
                "EBR",
                place::PlacedEcp5 {
                    ebrs: vec![place::PlacedEbr {
                        name: "ram0".into(),
                        params: place::EbrParams { width_a: 9, width_b: 9 },
                        nets: vec![],
                        site: dummy_site(),
                    }],
                    ..Default::default()
                },
            ),
            (
                "DSP",
                place::PlacedEcp5 {
                    dsps: vec![place::PlacedDsp {
                        name: "dsp0".into(),
                        kind: place::DspKind::Mult18x18,
                        nets: vec![],
                        site: dummy_site(),
                    }],
                    ..Default::default()
                },
            ),
            (
                "PLL",
                place::PlacedEcp5 {
                    plls: vec![place::PlacedPll {
                        name: "pll0".into(),
                        ref_net: "clk".into(),
                        out_nets: vec!["pll_clk".into()],
                        site: dummy_site(),
                    }],
                    ..Default::default()
                },
            ),
        ] {
            let err = route_on_device(d, &placed).unwrap_err();
            assert!(err.contains(what), "unexpected error for {what}: {err}");
            placed.top = "t".to_string();
        }
    }

    /// Reference check: every one of these (tile type, dst, src) triples
    /// was emitted by nextpnr-ecp5 --textcfg on real designs; the router's
    /// graph must contain them all.
    #[test]
    fn nextpnr_reference_arcs_exist_in_graph() {
        let d = dev();
        let r = Router::new(d).unwrap();
        // (tile type, dst as written, src as written)
        let refs = [
            ("PLC2", "V00T0000", "H02W0201"),
            ("PLC2", "CLK0", "G_HPBX0000"),
            ("PLC2", "D0", "Q0"),
            ("PLC2", "F0", "F0_SLICE"),
            ("PLC2", "LSR0", "V00T0000"),
            ("PLC2", "MUXCLK0", "CLK0"),
            ("PLC2", "A0", "H00L0000"),
            ("CIB", "H01W0000", "JQ0"),
            ("CIB_LR", "JA0", "E1_H01E0001"),
            ("TMID_0", "G_TDCC0CLKI", "G_JTRQPCLKCIB1"),
            ("CMUX_UL_0", "G_DCS0CLK0", "G_VPFN0000"),
            ("CMUX_UL_0", "G_ULPCLK0", "G_VPFS0000"),
            ("CMUX_UR_0", "G_URPCLK0", "G_VPFS0100"),
            ("PICB0", "JDIA", "JPADDIA_PIO"),
        ];
        for (tt, dst, src) in refs {
            // Find any entry of this type and check the mux.
            let entry = (0..r.entry_count() as u32)
                .find(|&e| d.tiles()[e as usize].tile_type == tt)
                .unwrap_or_else(|| panic!("no {tt} tile"));
            let g = r.graph(entry);
            let mi = g
                .mux_by_dst
                .get(dst)
                .unwrap_or_else(|| panic!("{tt}: no mux {dst}"));
            assert!(
                g.muxes[*mi].inputs.iter().any(|i| i.src == src),
                "{tt}: mux {dst} has no input {src}"
            );
        }
    }
}
