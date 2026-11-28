//! Route stage: [`PlacedIce`] → [`RoutedIce`].
//!
//! A* router over the HX8K routing graph. The graph is built from the
//! IceStorm intra-tile databases (buffer/routing entries) plus the
//! precomputed span-wire aliasing and IO↔fabric span maps. Span wires that
//! are the same physical copper share one node id, as do the eight
//! `glb_netwk` wires (one canonical node each). Neighbor-output wires
//! (`neigh_op_*`/`logic_op_*`) are geometrically aliased to the driving
//! tile's LC output (or IO input) per IceStorm `follow_funcnet`.
//!
//! Global clocks ([`PackedClk`](crate::pack::PackedClk) markers, PLL
//! outputs) ride the global network: source -> global buffer (`fabout` of
//! a `gbufin` IO tile, or the `padin_glb_netwk` extra bit for PLLs) ->
//! `glb_netwk` -> per-tile `lutff_global/clk`. See [`crate::gclk`].

use crate::device::{span_aliases, tile_db, tile_kind, TileKind};
use crate::gclk;
use crate::place::PlacedIce;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Ordering;

/// Is `name` a span wire (whose copper [`span_aliases`] knows)?
fn is_span(name: &str) -> bool {
    ["sp4_", "sp12_", "span4_", "span12_"].iter().any(|p| name.starts_with(p))
}

/// A routing-graph node id. Ids below `span_count` are canonical span wires;
/// the next 8 are the canonical `glb_netwk_0`..`glb_netwk_7` wires (every
/// tile's `glb_netwk_N` is the same physical net); higher ids are interned
/// `(x, y, name)` triples.
pub type NodeId = u32;

/// A directed routing edge. `rel` indexes the tile-kind-relative edge table
/// for the bit pattern to program.
#[derive(Clone, Debug)]
struct Edge {
    to: NodeId,
    tile_x: u8,
    tile_y: u8,
    kind_idx: usize,
    rel_idx: usize,
}

/// A tile-kind-relative routing edge: names plus the config bits to set.
#[derive(Clone, Debug)]
struct RelEdge {
    src: String,
    dst: String,
    bits: Vec<(u8, u8)>, // (block, bit) to set in the source tile
}

/// The routing graph.
pub struct Router {
    span_count: u32,
    /// First node id of the 8 canonical `glb_netwk_N` wires (= span_count).
    glb_base: u32,
    /// wire id -> bounding box of members (for the A* heuristic).
    span_bbox: Vec<(i32, i32, i32, i32)>,
    /// interned (x, y, name) -> node id.
    node_ids: HashMap<(u8, u8, String), NodeId>,
    /// node id -> (x, y) for interned nodes.
    node_xy: Vec<(u8, u8)>,
    adj: Vec<Vec<Edge>>,
    rel_edges: Vec<Vec<RelEdge>>, // per TileKind discriminant
    /// Extra nodes an edge connects besides its destination, keyed by
    /// `(tile_x, tile_y, kind_idx, rel_idx)` (see [`Router::edge_claims`]).
    claims: HashMap<(u8, u8, u8, u32), Vec<NodeId>>,
}

fn kind_idx(k: TileKind) -> usize {
    match k {
        TileKind::Logic => 0,
        TileKind::Ramb => 1,
        TileKind::Ramt => 2,
        TileKind::IoTop => 3,
        TileKind::IoBottom => 4,
        TileKind::IoLeft => 5,
        TileKind::IoRight => 6,
    }
}

/// Parse a `glb_netwk_N` wire name to its index (0..7).
fn parse_glb_netwk(name: &str) -> Option<u8> {
    let n: u8 = name.strip_prefix("glb_netwk_")?.parse().ok()?;
    (n < 8).then_some(n)
}

/// Geometric alias for a neighbor-output wire: inverts IceStorm
/// `icebox.py::follow_funcnet`. Tile `(x, y)`'s `neigh_op_<dir>_<n>`
/// (logic tiles) or `logic_op_<dir>_<n>` (IO tiles) is driven by the
/// neighboring tile's `lutff_<n>/out` (logic neighbor) or `io_<m>/D_IN_<k>`
/// (IO neighbor, per `rlookup_funcnet`'s func mapping) or `ram/RDATA_*`
/// (RAM neighbor, per `lookup_funcnet`).
/// Returns `None` when there is no driver (off-chip edge, or an IO tile next
/// to an IO tile); the caller then keeps the wire as an unconnected node.
fn neigh_op_alias(x: u8, y: u8, name: &str) -> Option<(u8, u8, String)> {
    let rest = name
        .strip_prefix("neigh_op_")
        .or_else(|| name.strip_prefix("logic_op_"))?;
    let (dir, n) = rest.split_once('_')?;
    let n: u8 = n.parse().ok()?;
    if n > 7 {
        return None;
    }
    // Inverse of follow_funcnet's do_direction table.
    let (dx, dy) = match dir {
        "bot" => (x as i16, y as i16 - 1),
        "bnl" => (x as i16 - 1, y as i16 - 1),
        "bnr" => (x as i16 + 1, y as i16 - 1),
        "top" => (x as i16, y as i16 + 1),
        "tnl" => (x as i16 - 1, y as i16 + 1),
        "tnr" => (x as i16 + 1, y as i16 + 1),
        "lft" => (x as i16 - 1, y as i16),
        "rgt" => (x as i16 + 1, y as i16),
        _ => return None,
    };
    if !(0..=33).contains(&dx) || !(0..=33).contains(&dy) {
        return None;
    }
    let (dx, dy) = (dx as u8, dy as u8);
    match tile_kind(dx, dy) {
        Some(TileKind::Logic) => Some((dx, dy, format!("lutff_{n}/out"))),
        // icebox.py lookup_funcnet (8k): RAMB func n is RDATA_{15-n}, RAMT RDATA_{7-n}.
        Some(TileKind::Ramb) => Some((dx, dy, format!("ram/RDATA_{}", 15 - n))),
        Some(TileKind::Ramt) => Some((dx, dy, format!("ram/RDATA_{}", 7 - n))),
        // An IO tile's logic_op_* wires come from the fabric only.
        Some(TileKind::IoTop | TileKind::IoBottom | TileKind::IoLeft | TileKind::IoRight) if name.starts_with("neigh_op_") => {
            // icebox.py rlookup_funcnet: io_0/D_IN_0 <- {0,4},
            // io_0/D_IN_1 <- {1,5}, io_1/D_IN_0 <- {2,6}, io_1/D_IN_1 <- {3,7}.
            let (m, k) = match n {
                0 | 4 => (0, 0),
                1 | 5 => (0, 1),
                2 | 6 => (1, 0),
                3 | 7 => (1, 1),
                _ => return None,
            };
            Some((dx, dy, format!("io_{m}/D_IN_{k}")))
        }
        _ => None,
    }
}

impl Router {
    /// Build the full HX8K routing graph. Expensive; build once.
    pub fn build() -> Self {
        let aliases = span_aliases();
        let span_count = aliases.values().max().map(|v| v + 1).unwrap_or(0);

        // Span bounding boxes for the A* heuristic.
        let mut span_bbox = vec![(99i32, -99i32, 99i32, -99i32); span_count as usize];
        for ((x, y, _), wid) in aliases.iter() {
            let b = &mut span_bbox[*wid as usize];
            b.0 = b.0.min(*x as i32);
            b.1 = b.1.max(*x as i32);
            b.2 = b.2.min(*y as i32);
            b.3 = b.3.max(*y as i32);
        }

        // Per-kind relative edge tables.
        let kinds = [
            TileKind::Logic,
            TileKind::Ramb,
            TileKind::Ramt,
            TileKind::IoTop,
            TileKind::IoBottom,
            TileKind::IoLeft,
            TileKind::IoRight,
        ];
        let mut rel_edges: Vec<Vec<RelEdge>> = Vec::new();
        // Per kind: rel index -> dst names of other entries whose full bit
        // pattern holds whenever this entry's set bits are the only ones set.
        let mut co_dsts: Vec<Vec<Vec<String>>> = Vec::new();
        for &k in &kinds {
            let mut rel = Vec::new();
            let mut pats: Vec<(Vec<(u8, u8, bool)>, String)> = Vec::new();
            for e in tile_db(k) {
                if !matches!(
                    e.func,
                    crate::db::DbFunc::Buffer | crate::db::DbFunc::Routing
                ) {
                    continue;
                }
                // Neighbor-output (`neigh_op_*`/`logic_op_*`) edges are
                // included: `resolve` aliases them to the driving tile's
                // LC output (or IO input), so they are live short-hop
                // edges, not dead ends.
                let bits: Vec<(u8, u8)> = e
                    .bits
                    .iter()
                    .filter(|b| b.val)
                    .map(|b| (b.row, b.bit))
                    .collect();
                rel.push(RelEdge {
                    src: e.src.clone(),
                    dst: e.dst.clone(),
                    bits,
                });
                pats.push((e.bits.iter().map(|b| (b.row, b.bit, b.val)).collect(), e.dst.clone()));
            }
            let co: Vec<Vec<String>> = pats
                .iter()
                .enumerate()
                .map(|(i, (pa, da))| {
                    let on: HashSet<(u8, u8)> = pa.iter().filter(|b| b.2).map(|b| (b.0, b.1)).collect();
                    pats.iter()
                        .enumerate()
                        .filter(|(j, (pb, db))| *j != i && db != da && pb.iter().all(|&(r, c, v)| on.contains(&(r, c)) == v))
                        .map(|(_, (_, db))| db.clone())
                        .collect()
                })
                .collect();
            co_dsts.push(co);
            rel_edges.push(rel);
        }

        let mut r = Router {
            span_count,
            glb_base: span_count,
            span_bbox,
            node_ids: HashMap::new(),
            node_xy: Vec::new(),
            adj: Vec::new(),
            rel_edges,
            claims: HashMap::new(),
        };
        // Size adjacency for span wires + 8 canonical glb wires + room for
        // interned nodes.
        r.adj.resize(span_count as usize + 8 + 4096, Vec::new());

        // Helper closures need &mut r; inline the logic instead.
        // Resolve (x, y, name) -> NodeId, creating interned nodes.
        // We do this in two passes: first collect all (x,y,name) that appear,
        // then build edges. Simpler: resolve on demand with a function.
        for y in 0..=33u8 {
            for x in 0..=33u8 {
                let Some(k) = tile_kind(x, y) else { continue };
                let ki = kind_idx(k);
                // Clone the relative edges for this kind to satisfy the borrow checker.
                let rels: Vec<(String, String)> = r.rel_edges[ki]
                    .iter()
                    .map(|re| (re.src.clone(), re.dst.clone()))
                    .collect();
                for (ri, (src, dst)) in rels.iter().enumerate() {
                    // A span name the tile does not have (IceStorm's
                    // tile_has_net) is no wire: skip the entry, as icebox does.
                    let (Some(src_id), Some(dst_id)) = (r.resolve(x, y, src, aliases), r.resolve(x, y, dst, aliases)) else {
                        continue;
                    };
                    if src_id == dst_id {
                        continue;
                    }
                    if !co_dsts[ki][ri].is_empty() {
                        let extra: Vec<NodeId> = co_dsts[ki][ri]
                            .clone()
                            .iter()
                            .filter_map(|d| r.resolve(x, y, d, aliases))
                            .filter(|&n| n != dst_id && n != src_id)
                            .collect();
                        if !extra.is_empty() {
                            r.claims.insert((x, y, ki as u8, ri as u32), extra);
                        }
                    }
                    let edge = Edge {
                        to: dst_id,
                        tile_x: x,
                        tile_y: y,
                        kind_idx: ki,
                        rel_idx: ri,
                    };
                    let idx = src_id as usize;
                    if idx >= r.adj.len() {
                        r.adj.resize(idx + 1024, Vec::new());
                    }
                    r.adj[idx].push(edge);
                }
            }
        }
        r.adj.truncate(r.node_count() as usize);
        r
    }

    /// Resolve `(x, y, name)` to a node id; `None` for a span name the
    /// tile does not have.
    fn resolve(&mut self, x: u8, y: u8, name: &str, aliases: &HashMap<(u8, u8, String), u32>) -> Option<NodeId> {
        // Canonical global-network wires: every tile's glb_netwk_N is the
        // same physical net (IceStorm icebox_vlog joins them the same way).
        if let Some(n) = parse_glb_netwk(name) {
            return Some(self.glb_base + u32::from(n));
        }
        // Neighbor-output wires are geometric aliases of the neighboring
        // tile's LC output (or IO input); see `neigh_op_alias`.
        if let Some((dx, dy, dname)) = neigh_op_alias(x, y, name) {
            return self.resolve(dx, dy, &dname, aliases);
        }
        // Span wires, including IO-tile spans, are in the alias table.
        if is_span(name) {
            return aliases.get(&(x, y, name.to_string())).copied();
        }
        // Otherwise an interned tile-local node.
        let key = (x, y, name.to_string());
        if let Some(id) = self.node_ids.get(&key) {
            return Some(*id);
        }
        let id = self.glb_base + 8 + self.node_xy.len() as u32;
        self.node_ids.insert(key, id);
        self.node_xy.push((x, y));
        Some(id)
    }

    pub fn node_count(&self) -> u32 {
        self.glb_base + 8 + self.node_xy.len() as u32
    }

    /// Config bits for a routed edge (to program in the source tile).
    pub fn edge_bits(&self, kind_idx: usize, rel_idx: usize) -> &[(u8, u8)] {
        &self.rel_edges[kind_idx][rel_idx].bits
    }

    /// Source net name of a routing-graph edge (for asc emission and tests).
    pub fn rel_src(&self, kind_idx: usize, rel_idx: usize) -> &str {
        &self.rel_edges[kind_idx][rel_idx].src
    }

    /// Destination net name of a routing-graph edge (for tests).
    pub fn rel_dst(&self, kind_idx: usize, rel_idx: usize) -> &str {
        &self.rel_edges[kind_idx][rel_idx].dst
    }

    /// Canonical node id of the `glb_netwk_N` wire.
    pub fn glb_node(&self, n: u8) -> NodeId {
        self.glb_base + u32::from(n)
    }

    /// Position of a node for the A* heuristic.
    fn node_pos(&self, id: NodeId) -> (i32, i32) {
        if id < self.span_count {
            let b = self.span_bbox[id as usize];
            // Center of the bounding box.
            ((b.0 + b.1) / 2, (b.2 + b.3) / 2)
        } else if id < self.span_count + 8 {
            // A global-network wire spans the whole chip.
            (16, 16)
        } else {
            let (x, y) = self.node_xy[(id - self.span_count - 8) as usize];
            (x as i32, y as i32)
        }
    }

    /// Public node lookup for net endpoints. Returns `None` for span wires
    /// that do not exist (should not happen for valid names).
    pub fn lookup(&self, x: u8, y: u8, name: &str) -> Option<NodeId> {
        if let Some(n) = parse_glb_netwk(name) {
            return Some(self.glb_base + u32::from(n));
        }
        if let Some((dx, dy, dname)) = neigh_op_alias(x, y, name) {
            return self.lookup(dx, dy, &dname);
        }
        if is_span(name) {
            return span_aliases().get(&(x, y, name.to_string())).copied();
        }
        self.node_ids.get(&(x, y, name.to_string())).copied()
    }

    /// Edges out of `n`.
    fn edges_from(&self, n: NodeId) -> &[Edge] {
        &self.adj[n as usize]
    }

    /// Nodes besides `e.to` that programming edge `e` also connects: other
    /// DB entries of the same tile kind with exactly the same bit pattern
    /// (the IceStorm IO-tile database lists one buffer bit under both a
    /// horizontal and a vertical span name). Using `e` claims them too.
    fn edge_claims(&self, e: &Edge) -> &[NodeId] {
        match self.claims.get(&(e.tile_x, e.tile_y, e.kind_idx as u8, e.rel_idx as u32)) {
            Some(v) => v,
            None => &[],
        }
    }
}

/// Reusable search state for [`Negotiator::search`].
struct Scratch {
    g: Vec<f32>,
    stamp: Vec<u32>,
    prev: Vec<(NodeId, u32)>,
    epoch: u32,
}

#[derive(Clone, Copy, PartialEq)]
struct HeapItem {
    f: f32,
    g: f32,
    node: NodeId,
}
impl Eq for HeapItem {}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then_with(|| self.node.cmp(&other.node))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One net to route: driver node and load nodes.
struct NetReq {
    name: String,
    driver: NodeId,
    loads: Vec<NodeId>,
}

/// PathFinder-style negotiated congestion router (McMurchie & Ebeling).
///
/// Every wire (graph node) holds at most one net. Nets are routed one at a
/// time against the present cost of wires other nets hold and a history
/// cost that grows on wires that stay contended; nets on contended wires
/// are ripped up and rerouted until no wire is shared. A net's tree grows
/// from all of its already-reached wires (multi-source A*), so no wire is
/// driven twice inside one net either.
struct Negotiator<'a> {
    router: &'a Router,
    occ: Vec<u16>,
    hist: Vec<f32>,
    pres_fac: f32,
    scratch: Scratch,
}

impl<'a> Negotiator<'a> {
    fn new(router: &'a Router) -> Self {
        let n = router.node_count() as usize;
        Negotiator {
            router,
            occ: vec![0; n],
            hist: vec![0.0; n],
            pres_fac: 0.5,
            scratch: Scratch { g: vec![0.0; n], stamp: vec![0; n], prev: vec![(0, 0); n], epoch: 0 },
        }
    }

    /// Cost of claiming `n` for a net that does not hold it yet.
    fn node_cost(&self, n: NodeId, owned: &HashSet<NodeId>) -> f32 {
        if owned.contains(&n) {
            return 0.0;
        }
        let i = n as usize;
        (1.0 + self.hist[i]) * (1.0 + self.pres_fac * f32::from(self.occ[i]))
    }

    /// Cheapest path from any node of `tree` to `target`; returns its edges.
    fn search(&mut self, tree: &[NodeId], owned: &HashSet<NodeId>, target: NodeId) -> Option<Vec<Edge>> {
        let r = self.router;
        self.scratch.epoch = self.scratch.epoch.wrapping_add(1);
        if self.scratch.epoch == 0 {
            self.scratch.stamp.iter_mut().for_each(|s| *s = 0);
            self.scratch.epoch = 1;
        }
        let epoch = self.scratch.epoch;
        let (tx, ty) = r.node_pos(target);
        let h = |n: NodeId| -> f32 {
            let (x, y) = r.node_pos(n);
            0.5 * ((x - tx).abs() + (y - ty).abs()) as f32
        };
        let mut heap = BinaryHeap::new();
        for &s in tree {
            let si = s as usize;
            self.scratch.stamp[si] = epoch;
            self.scratch.g[si] = 0.0;
            self.scratch.prev[si] = (s, u32::MAX);
            heap.push(HeapItem { f: h(s), g: 0.0, node: s });
        }
        while let Some(HeapItem { g, node, .. }) = heap.pop() {
            if node == target {
                let mut edges = Vec::new();
                let mut cur = target;
                loop {
                    let (p, ei) = self.scratch.prev[cur as usize];
                    if ei == u32::MAX {
                        break;
                    }
                    edges.push(r.edges_from(p)[ei as usize].clone());
                    cur = p;
                }
                edges.reverse();
                return Some(edges);
            }
            if self.scratch.stamp[node as usize] == epoch && g > self.scratch.g[node as usize] {
                continue;
            }
            for (ei, e) in r.edges_from(node).iter().enumerate() {
                let to = e.to as usize;
                let mut c = self.node_cost(e.to, owned);
                for &m in r.edge_claims(e) {
                    c += self.node_cost(m, owned);
                }
                let ng = g + c;
                if self.scratch.stamp[to] != epoch || ng < self.scratch.g[to] {
                    self.scratch.stamp[to] = epoch;
                    self.scratch.g[to] = ng;
                    self.scratch.prev[to] = (node, ei as u32);
                    heap.push(HeapItem { f: ng + h(e.to), g: ng, node: e.to });
                }
            }
        }
        None
    }

    /// Route one net; returns its edges and every node it holds.
    fn route_net(&mut self, net: &NetReq) -> Result<(Vec<Edge>, Vec<NodeId>), String> {
        let mut owned: HashSet<NodeId> = HashSet::new();
        owned.insert(net.driver);
        let mut tree = vec![net.driver];
        let mut edges = Vec::new();
        let (dx, dy) = self.router.node_pos(net.driver);
        let mut loads = net.loads.clone();
        loads.sort_by_key(|&l| {
            let (x, y) = self.router.node_pos(l);
            ((x - dx).abs() + (y - dy).abs(), l)
        });
        loads.dedup();
        for load in loads {
            if owned.contains(&load) {
                continue;
            }
            let path = self
                .search(&tree, &owned, load)
                .ok_or_else(|| format!("route: net '{}': no path to a load", net.name))?;
            for e in &path {
                if owned.insert(e.to) {
                    tree.push(e.to);
                }
                for &m in self.router.edge_claims(e) {
                    owned.insert(m);
                }
            }
            edges.extend(path);
        }
        let mut nodes: Vec<NodeId> = owned.into_iter().collect();
        nodes.sort_unstable();
        Ok((edges, nodes))
    }

    /// Route all nets until no wire is shared.
    fn run(&mut self, nets: &[NetReq]) -> Result<Vec<Vec<Edge>>, String> {
        const MAX_ITERS: usize = 80;
        let mut routes: Vec<Option<(Vec<Edge>, Vec<NodeId>)>> = vec![None; nets.len()];
        for iter in 0..MAX_ITERS {
            for (i, net) in nets.iter().enumerate() {
                if let Some((_, nodes)) = &routes[i] {
                    if nodes.iter().all(|&n| self.occ[n as usize] <= 1) {
                        continue;
                    }
                    for &n in nodes {
                        self.occ[n as usize] -= 1;
                    }
                }
                let (edges, nodes) = self.route_net(net)?;
                for &n in &nodes {
                    self.occ[n as usize] += 1;
                }
                routes[i] = Some((edges, nodes));
            }
            let mut shared = 0usize;
            for (i, o) in self.occ.iter().enumerate() {
                if *o > 1 {
                    shared += 1;
                    self.hist[i] += 0.3 * f32::from(*o - 1);
                }
            }
            if shared == 0 {
                return Ok(routes.into_iter().map(|r| r.map(|(e, _)| e).unwrap_or_default()).collect());
            }
            if iter + 1 == MAX_ITERS {
                return Err(format!(
                    "route: congestion not resolved after {MAX_ITERS} negotiation passes: {shared} wire(s) still wanted by more than one net"
                ));
            }
            self.pres_fac *= 1.5;
        }
        unreachable!()
    }
}

/// A routed net: the tree of edges plus endpoints.
#[derive(Clone, Debug)]
pub struct RoutedNet {
    pub name: String,
    pub edges: Vec<RoutedEdge>,
}

/// A single programmed connection.
#[derive(Clone, Debug)]
pub struct RoutedEdge {
    pub tile_x: u8,
    pub tile_y: u8,
    pub kind_idx: usize,
    pub rel_idx: usize,
}

/// Backend-owned routed netlist.
#[derive(Clone, Debug, Default)]
pub struct RoutedIce {
    pub placed: PlacedIce,
    pub nets: Vec<RoutedNet>,
    /// Global clocks: per-tile `glb_netwk -> lutff_global/clk` programming,
    /// `ColBufCtrl` bits, and `padin_glb_netwk` extra bits are emitted from
    /// this (see [`asc`](crate::asc)).
    pub glb_clocks: Vec<GlbClock>,
}

/// One routed global clock.
#[derive(Clone, Debug)]
pub struct GlbClock {
    /// Global net index 0..7.
    pub glb: u8,
    /// Logic tiles whose FFs take this clock: program
    /// `glb_netwk_{glb} -> lutff_global/clk` here, plus the `ColBufCtrl`
    /// bit in the covering column-buffer tile.
    pub tiles: Vec<(u8, u8)>,
    /// RAM clock pins on this clock: `(x, y, is_wclk)`; program
    /// `glb_netwk_{glb} -> ram/RCLK` (RAMB tile) or `-> ram/WCLK`
    /// (RAMT tile).
    pub ram_clks: Vec<(u8, u8, bool)>,
    /// `padin_glb_netwk` extra bit `(bank, row, bit)` for PLL-driven clocks.
    pub extra_bit: Option<(u8, u16, u16)>,
}

/// Route the placed design. Returns the routed nets with their edges.
///
/// Global clocks ([`PackedClk`](crate::pack::PackedClk) markers and PLL
/// outputs) are assigned to `glb_netwk` wires by [`gclk::plan`]: their FF
/// clock pins bypass fabric routing (the per-tile
/// `glb_netwk -> lutff_global/clk` buffer is programmed from
/// [`RoutedIce::glb_clocks`]), and any other loads route from the canonical
/// `glb_netwk` node.
pub fn route(router: &Router, placed: &PlacedIce) -> Result<RoutedIce, String> {
    // Assign global clocks first: each PackedClk marker and each PLL output
    // gets one of the 8 glb nets (fails loud when over-subscribed).
    let plans = gclk::plan(placed)?;
    // Net name -> plan index.
    let mut net_plan: HashMap<String, usize> = HashMap::new();
    for (i, p) in plans.iter().enumerate() {
        for n in &p.nets {
            net_plan.insert(n.clone(), i);
        }
    }

    // Build net -> (drivers, loads).
    // Driver/load endpoints as node ids.
    struct Endpoint {
        node: NodeId,
        is_driver: bool,
    }
    let mut net_eps: HashMap<String, Vec<Endpoint>> = HashMap::new();
    // Logic tiles taking each global clock (for glb_clocks).
    let mut glb_tiles: HashMap<usize, Vec<(u8, u8)>> = HashMap::new();
    // RAM clock pins taking each global clock: (x, y, is_wclk).
    let mut glb_ram_clks: HashMap<usize, Vec<(u8, u8, bool)>> = HashMap::new();
    let mut add_ep = |net: &str, node: Option<NodeId>, is_driver: bool, what: &str| -> Result<(), String> {
        let node = node.ok_or_else(|| format!("route: no routing node for {what}"))?;
        net_eps
            .entry(net.to_string())
            .or_default()
            .push(Endpoint { node, is_driver });
        Ok(())
    };

    // LUT outputs and FF outputs drive nets. A paired LC's lutff_N/out node
    // carries the FF's Q (DffEnable); an unpaired LUT's carries its output.
    // Build a map from (x, y, lc) -> FF q_net for paired LCs.
    let mut lc_ff_q: HashMap<(u8, u8, u8), &str> = HashMap::new();
    for ff in &placed.ffs {
        lc_ff_q.insert((ff.x, ff.y, ff.lc), ff.q_net.as_str());
    }
    for lut in &placed.luts {
        let key = (lut.x, lut.y, lut.lc);
        // If a FF shares this LC, the LUT output is internal (feeds FF D);
        // the FF's Q is the driver. Otherwise the LUT output drives.
        // (LUT inputs are loads in either case.)
        if !lc_ff_q.contains_key(&key) {
            let n = router.lookup(lut.x, lut.y, &format!("lutff_{}/out", lut.lc));
            add_ep(&lut.output_net, n, true, &format!("LUT {} out", lut.name))?;
        }
        for (k, net) in lut.input_nets.iter().enumerate() {
            if let Some(net) = net {
                // Carry LCs: in_3 is the dedicated carry (previous cout /
                // carry_in_mux), programmed directly by asc.rs, not routed.
                if lut.carry && k == 3 {
                    continue;
                }
                let n = router.lookup(lut.x, lut.y, &format!("lutff_{}/in_{k}", lut.lc));
                add_ep(net, n, false, &format!("LUT {} in_{k}", lut.name))?;
            }
        }
    }
    for ff in &placed.ffs {
        // Q driver at the LC output.
        let n = router.lookup(ff.x, ff.y, &format!("lutff_{}/out", ff.lc));
        add_ep(&ff.q_net, n, true, &format!("FF {} q", ff.name))?;
        // Clock: on a global net the pin bypasses fabric routing; the
        // per-tile glb_netwk -> lutff_global/clk buffer is programmed from
        // glb_clocks instead. Other clocks keep the local-track path.
        if let Some(&pi) = net_plan.get(&ff.clk_net) {
            glb_tiles.entry(pi).or_default().push((ff.x, ff.y));
        } else {
            let n = router.lookup(ff.x, ff.y, "lutff_global/clk");
            add_ep(&ff.clk_net, n, false, &format!("FF {} clk", ff.name))?;
        }
    }
    for io in &placed.ios {
        match io.dir {
            crate::pack::IoDir::In => {
                let n = router.lookup(io.x, io.y, &format!("io_{}/D_IN_0", io.iob));
                add_ep(&io.net, n, true, &format!("IO {} in", io.name))?;
            }
            crate::pack::IoDir::Out => {
                let n = router.lookup(io.x, io.y, &format!("io_{}/D_OUT_0", io.iob));
                add_ep(&io.net, n, false, &format!("IO {} out", io.name))?;
            }
        }
    }
    // RAM blocks: SB_RAM40_4K spans a RAMB tile (x, y) and RAMT tile (x, y+1).
    // Port split per the IceStorm 8k RAM DBs (verified against arachne-pnr):
    // RAMB (odd y) carries RADDR/RCLK/RE and RDATA/WDATA[15:8];
    // RAMT (even y) carries WADDR/WCLK/WE and RDATA/WDATA[7:0].
    // (The ram_tile.rst doc table has these columns swapped; the DB and
    // arachne-pnr's own output agree on the layout above.)
    //
    // pack_ram marks unconnected RAM pins with the "gnd" placeholder net;
    // those endpoints are skipped (their muxes stay unprogrammed) instead
    // of failing as undriven nets.
    for (ri, ram) in placed.packed.rams.iter().enumerate() {
        let placed_ram = &placed.rams[ri];
        let (x, y) = (placed_ram.x, placed_ram.y);
        let ram_pack = &placed.packed.rams[ri];
        let mut add_ram_ep =
            |net: &str, node: Option<NodeId>, is_driver: bool, what: &str| -> Result<(), String> {
                if net == "gnd" {
                    return Ok(());
                }
                add_ep(net, node, is_driver, what)
            };
        // Read data outputs.
        for i in 0..8 {
            let n = router.lookup(x, y + 1, &format!("ram/RDATA_{i}"));
            add_ram_ep(&ram_pack.rdata_net[i], n, true, &format!("RAM {} RDATA{i}", ram_pack.name))?;
        }
        for i in 8..16 {
            let n = router.lookup(x, y, &format!("ram/RDATA_{i}"));
            add_ram_ep(&ram_pack.rdata_net[i], n, true, &format!("RAM {} RDATA{i}", ram_pack.name))?;
        }
        // Write data inputs.
        for i in 0..8 {
            let n = router.lookup(x, y + 1, &format!("ram/WDATA_{i}"));
            add_ram_ep(&ram_pack.wdata_net[i], n, false, &format!("RAM {} WDATA{i}", ram_pack.name))?;
        }
        for i in 8..16 {
            let n = router.lookup(x, y, &format!("ram/WDATA_{i}"));
            add_ram_ep(&ram_pack.wdata_net[i], n, false, &format!("RAM {} WDATA{i}", ram_pack.name))?;
        }
        // Addresses and clocks.
        for i in 0..11 {
            let n = router.lookup(x, y, &format!("ram/RADDR_{i}"));
            add_ram_ep(&ram_pack.raddr_net[i], n, false, &format!("RAM {} RADDR{i}", ram_pack.name))?;
            let n = router.lookup(x, y + 1, &format!("ram/WADDR_{i}"));
            add_ram_ep(&ram_pack.waddr_net[i], n, false, &format!("RAM {} WADDR{i}", ram_pack.name))?;
        }
        let n = router.lookup(x, y, "ram/RCLK");
        // A global clock drives RCLK straight from the glb wire (the RAM
        // tile has a glb_netwk -> ram/RCLK buffer); other clocks route as
        // ordinary nets.
        if ram_pack.rclk_net != "gnd" {
            if let Some(&pi) = net_plan.get(&ram_pack.rclk_net) {
                glb_ram_clks.entry(pi).or_default().push((x, y, false));
            } else {
                add_ram_ep(&ram_pack.rclk_net, n, false, &format!("RAM {} RCLK", ram_pack.name))?;
            }
        }
        let n = router.lookup(x, y + 1, "ram/WCLK");
        if ram_pack.wclk_net != "gnd" {
            if let Some(&pi) = net_plan.get(&ram_pack.wclk_net) {
                glb_ram_clks.entry(pi).or_default().push((x, y + 1, true));
            } else {
                add_ram_ep(&ram_pack.wclk_net, n, false, &format!("RAM {} WCLK", ram_pack.name))?;
            }
        }
        if let Some(re) = &ram_pack.re_net {
            let n = router.lookup(x, y, "ram/RE");
            add_ram_ep(re, n, false, &format!("RAM {} RE", ram_pack.name))?;
        }
        if let Some(we) = &ram_pack.we_net {
            let n = router.lookup(x, y + 1, "ram/WE");
            add_ram_ep(we, n, false, &format!("RAM {} WE", ram_pack.name))?;
        }
        // Write mask bits (MASK_0..7 in RAMT, MASK_8..15 in RAMB).
        for i in 0..8 {
            let n = router.lookup(x, y + 1, &format!("ram/MASK_{i}"));
            add_ram_ep(
                &ram_pack.mask_net[i],
                n,
                false,
                &format!("RAM {} MASK{i}", ram_pack.name),
            )?;
        }
        for i in 8..16 {
            let n = router.lookup(x, y, &format!("ram/MASK_{i}"));
            add_ram_ep(
                &ram_pack.mask_net[i],
                n,
                false,
                &format!("RAM {} MASK{i}", ram_pack.name),
            )?;
        }
        let _ = ram;
    }
    // PLLs: the reference clock is a load on the PLL site's REFERENCECLK
    // `fabout` net (io_tile (13,0) for the bottom PLL, (13,33) for the top
    // one; IceStorm icebox.py pllinfo_db "8k_0"/"8k_1").
    for (pi, pll) in placed.packed.plls.iter().enumerate() {
        let placed_pll = &placed.plls[pi];
        let ref_tile = if placed_pll.kind.is_pad() {
            (13u8, 0u8)
        } else {
            (13u8, 33u8)
        };
        let n = router.lookup(ref_tile.0, ref_tile.1, "fabout");
        add_ep(
            &pll.ref_net,
            n,
            false,
            &format!("PLL {} REFERENCECLK", pll.name),
        )?;
        if let Some(lock_net) = &pll.lock_net {
            let lock_node = router.lookup(14, ref_tile.1, "io_0/D_IN_0");
            add_ep(
                lock_net,
                lock_node,
                true,
                &format!("PLL {} LOCK", pll.name),
            )?;
        }
    }

    // Global-clock sources: route each source net to the `fabout` of its
    // assigned global-buffer IO tile (arachne-pnr's path). PLL-driven
    // clocks need no routing (padin_glb_netwk extra bit).
    for p in &plans {
        if let (Some(src_net), gclk::ClockSource::GlobalBuffer { gx, gy }) =
            (&p.source_net, p.source)
        {
            let n = router.lookup(gx, gy, "fabout");
            add_ep(
                src_net,
                n,
                false,
                &format!("global clock source -> fabout {gx},{gy}"),
            )?;
        }
    }

    // Internal oscillators & Hard IP blocks (SB_HFOSC, SB_LFOSC, SB_MAC16,
    // SB_SPRAM256KA, SB_I2C, SB_SPI, SB_RGBA_DRV, SB_LEDDA_IP):
    // 1. Register all output drivers first.
    for (oi, osc) in placed.packed.oscs.iter().enumerate() {
        let posc = &placed.oscs[oi];
        let drv = router.lookup(posc.x, posc.y, "io_0/D_IN_0");
        add_ep(&osc.out_net, drv, true, &format!("OSC {} out", osc.name))?;
    }

    let mut used_lc_out: HashSet<(u8, u8, u8)> = placed
        .luts
        .iter()
        .map(|l| (l.x, l.y, l.lc))
        .chain(placed.ffs.iter().map(|f| (f.x, f.y, f.lc)))
        .collect();
    let mut alloc_ip_driver = |site_x: u8, site_y: u8| -> Option<NodeId> {
        let lx = if site_x == 0 { 1u8 } else { 32u8 };
        for dy in 0..32u8 {
            let y = ((site_y.saturating_sub(1) + dy) % 32) + 1;
            if !matches!(tile_kind(lx, y), Some(TileKind::Logic)) {
                continue;
            }
            for lc in 0..8u8 {
                if used_lc_out.insert((lx, y, lc)) {
                    return router.lookup(lx, y, &format!("lutff_{lc}/out"));
                }
            }
        }
        None
    };

    for (mi, mac) in placed.packed.mac16s.iter().enumerate() {
        let pmac = &placed.mac16s[mi];
        for out_net in mac
            .o_nets
            .iter()
            .flatten()
            .chain(mac.co_net.iter())
            .chain(mac.acc_ovf_net.iter())
        {
            let drv = alloc_ip_driver(pmac.x, pmac.y);
            add_ep(out_net, drv, true, &format!("SB_MAC16 {} out", mac.name))?;
        }
    }
    for (si, spram) in placed.packed.sprams.iter().enumerate() {
        let pspram = &placed.sprams[si];
        for out_net in spram.data_out_nets.iter().flatten() {
            let drv = alloc_ip_driver(pspram.x, pspram.y);
            add_ep(
                out_net,
                drv,
                true,
                &format!("SB_SPRAM256KA {} DATAOUT", spram.name),
            )?;
        }
    }
    for (hi, ip) in placed.packed.hard_ips.iter().enumerate() {
        let pip = &placed.hard_ips[hi];
        for (pin, out_net) in &ip.output_nets {
            let drv = alloc_ip_driver(pip.x, pip.y);
            add_ep(out_net, drv, true, &format!("HardIP {} {pin}", ip.name))?;
        }
    }

    // 2. Register driven input loads on oscillators, DSPs, SPRAMs, and hard IPs.
    let mut ip_in_cursor = 0usize;
    let mut alloc_ip_load = |site_x: u8, site_y: u8| -> Option<NodeId> {
        let lx = if site_x == 0 { 1u8 } else { 32u8 };
        let y = site_y.clamp(1, 32);
        let lc = ((ip_in_cursor / 4) % 8) as u8;
        let pin = (ip_in_cursor % 4) as u8;
        ip_in_cursor += 1;
        router.lookup(lx, y, &format!("lutff_{lc}/in_{pin}"))
    };
    let push_load_if_driven =
        |eps: &mut HashMap<String, Vec<Endpoint>>, net: &str, node: Option<NodeId>, what: &str| -> Result<(), String> {
            if eps.get(net).is_some_and(|v| v.iter().any(|e| e.is_driver)) {
                let node = node.ok_or_else(|| format!("route: no routing node for {what}"))?;
                eps.entry(net.to_string()).or_default().push(Endpoint {
                    node,
                    is_driver: false,
                });
            }
            Ok(())
        };

    for (oi, osc) in placed.packed.oscs.iter().enumerate() {
        let posc = &placed.oscs[oi];
        for (pin_label, opt_net) in [("PU", &osc.pu_net), ("EN", &osc.en_net)] {
            if let Some(net) = opt_net {
                let iob = if pin_label == "PU" { 0 } else { 1 };
                let ld = router.lookup(posc.x, posc.y, &format!("io_{iob}/D_OUT_0"));
                push_load_if_driven(&mut net_eps, net, ld, &format!("OSC {} {pin_label}", osc.name))?;
            }
        }
    }
    for (mi, mac) in placed.packed.mac16s.iter().enumerate() {
        let pmac = &placed.mac16s[mi];
        if let Some(clk) = &mac.clk_net {
            if let Some(&pi) = net_plan.get(clk) {
                let lx = if pmac.x == 0 { 1u8 } else { 32u8 };
                glb_tiles.entry(pi).or_default().push((lx, pmac.y.clamp(1, 32)));
            }
        }
        for in_net in mac
            .a_nets
            .iter()
            .chain(mac.b_nets.iter())
            .chain(mac.c_nets.iter())
            .chain(mac.d_nets.iter())
            .chain(std::iter::once(&mac.ce_net))
            .flatten()
        {
            let ld = alloc_ip_load(pmac.x, pmac.y);
            push_load_if_driven(&mut net_eps, in_net, ld, &format!("SB_MAC16 {} in", mac.name))?;
        }
    }
    for (si, spram) in placed.packed.sprams.iter().enumerate() {
        let pspram = &placed.sprams[si];
        if let Some(clk) = &spram.clock_net {
            if let Some(&pi) = net_plan.get(clk) {
                let lx = if pspram.x == 0 { 1u8 } else { 32u8 };
                glb_tiles
                    .entry(pi)
                    .or_default()
                    .push((lx, pspram.y.clamp(1, 32)));
            }
        }
        for in_net in spram
            .addr_nets
            .iter()
            .chain(spram.data_in_nets.iter())
            .chain(spram.mask_wren_nets.iter())
            .chain([
                &spram.wren_net,
                &spram.chipselect_net,
                &spram.standby_net,
                &spram.sleep_net,
                &spram.poweroff_net,
            ])
            .flatten()
        {
            let ld = alloc_ip_load(pspram.x, pspram.y);
            push_load_if_driven(
                &mut net_eps,
                in_net,
                ld,
                &format!("SB_SPRAM256KA {} in", spram.name),
            )?;
        }
    }
    for (hi, ip) in placed.packed.hard_ips.iter().enumerate() {
        let pip = &placed.hard_ips[hi];
        for (pin, in_net) in &ip.input_nets {
            if let Some(&pi) = net_plan.get(in_net) {
                let lx = if pip.x == 0 { 1u8 } else { 32u8 };
                glb_tiles.entry(pi).or_default().push((lx, pip.y.clamp(1, 32)));
            } else {
                let ld = alloc_ip_load(pip.x, pip.y);
                push_load_if_driven(&mut net_eps, in_net, ld, &format!("HardIP {} {pin}", ip.name))?;
            }
        }
    }

    // Route every net with negotiated congestion: no wire carries two nets.
    let mut reqs: Vec<NetReq> = Vec::new();
    let mut names: Vec<(String, Vec<NodeId>)> = net_eps
        .into_iter()
        .map(|(n, eps)| {
            let mut v: Vec<NodeId> = eps.iter().filter(|e| e.is_driver).map(|e| e.node).collect();
            v.push(u32::MAX);
            v.extend(eps.iter().filter(|e| !e.is_driver).map(|e| e.node));
            (n, v)
        })
        .collect();
    // Big nets first, then by name: deterministic.
    names.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    for (name, v) in names {
        let split = v.iter().position(|&n| n == u32::MAX).unwrap_or(0);
        let drivers: Vec<NodeId> = v[..split].to_vec();
        let loads: Vec<NodeId> = v[split + 1..].to_vec();
        // A global clock net is driven by its canonical glb_netwk wire
        // (the source reached it via the global buffer / padin bit); a
        // fabric driver on the same net is a design bug.
        let driver = if let Some(&pi) = net_plan.get(&name) {
            if !drivers.is_empty() {
                return Err(format!(
                    "route: global clock net '{name}' has {} fabric drivers",
                    drivers.len()
                ));
            }
            router.glb_node(plans[pi].glb)
        } else {
            match drivers[..] {
                [] => return Err(format!("route: net '{name}' has no driver")),
                [d] => d,
                _ => return Err(format!("route: net '{name}' has {} drivers", drivers.len())),
            }
        };
        if loads.is_empty() {
            continue; // no loads: nothing to route
        }
        reqs.push(NetReq { name, driver, loads });
    }
    let mut neg = Negotiator::new(router);
    let edge_lists = neg.run(&reqs)?;
    if let Ok(want) = std::env::var("HELION_ICE40_ROUTE_DUMP") {
        for (req, edges) in reqs.iter().zip(&edge_lists) {
            if req.name.contains(&want) {
                eprintln!("net {}:", req.name);
                for e in edges {
                    eprintln!("  ({},{}) {} -> {}", e.tile_x, e.tile_y, router.rel_src(e.kind_idx, e.rel_idx), router.rel_dst(e.kind_idx, e.rel_idx));
                }
            }
        }
    }
    let routed: Vec<RoutedNet> = reqs
        .into_iter()
        .zip(edge_lists)
        .map(|(req, edges)| RoutedNet {
            name: req.name,
            edges: edges
                .into_iter()
                .map(|e| RoutedEdge {
                    tile_x: e.tile_x,
                    tile_y: e.tile_y,
                    kind_idx: e.kind_idx,
                    rel_idx: e.rel_idx,
                })
                .collect(),
        })
        .collect();

    let mut glb_clocks: Vec<GlbClock> = Vec::new();
    for (i, p) in plans.iter().enumerate() {
        let mut tiles = glb_tiles.remove(&i).unwrap_or_default();
        tiles.sort();
        tiles.dedup();
        let mut ram_clks = glb_ram_clks.remove(&i).unwrap_or_default();
        ram_clks.sort();
        ram_clks.dedup();
        glb_clocks.push(GlbClock {
            glb: p.glb,
            tiles,
            ram_clks,
            extra_bit: p.extra_bit,
        });
    }

    Ok(RoutedIce {
        placed: placed.clone(),
        nets: routed,
        glb_clocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neigh_op_edges_alias_neighbor_lc_outputs() {
        // IceStorm follow_funcnet inverse: tile (5,6)'s neigh_op_bot_0 is
        // tile (5,5)'s lutff_0/out. The graph must contain live edges from
        // that driver node, programmed in tile (5,6).
        let r = Router::build();
        let driver = r.lookup(5, 5, "lutff_0/out").expect("driver node");
        // lookup aliases the same way the graph builder does.
        assert_eq!(r.lookup(5, 6, "neigh_op_bot_0"), Some(driver));
        assert_eq!(r.lookup(6, 6, "neigh_op_bnl_0"), Some(driver));
        assert_eq!(r.lookup(5, 4, "neigh_op_top_0"), Some(driver));
        // An edge from the driver node must exist with the neigh_op buffer
        // bits in the neighbor tile.
        let li = kind_idx(TileKind::Logic);
        let mut found = false;
        for (ri, re) in r.rel_edges[li].iter().enumerate() {
            if re.src == "neigh_op_bot_0" {
                found |= r.adj[driver as usize].iter().any(|e| {
                    e.tile_x == 5 && e.tile_y == 6 && e.kind_idx == li && e.rel_idx == ri
                });
            }
        }
        assert!(found, "no live neigh_op_bot_0 edge from (5,5) lutff_0/out");
    }

    #[test]
    fn neigh_op_alias_rejects_unrepresentable_drivers() {
        // Off-chip drivers stay unaliased (dead nodes): no panic, no bogus tile.
        assert_eq!(neigh_op_alias(5, 0, "neigh_op_bot_0"), None);
        // RAM neighbor: icebox lookup_funcnet's RDATA mapping.
        assert_eq!(neigh_op_alias(7, 5, "neigh_op_rgt_0"), Some((8, 5, "ram/RDATA_15".to_string())));
        assert_eq!(neigh_op_alias(7, 6, "neigh_op_rgt_2"), Some((8, 6, "ram/RDATA_5".to_string())));
        // An IO tile's logic_op_ wires never come from another IO tile.
        assert_eq!(neigh_op_alias(1, 33, "logic_op_bnl_0"), None);
        assert_eq!(neigh_op_alias(5, 5, "neigh_op_xx_0"), None);
        assert_eq!(neigh_op_alias(5, 5, "slf_op_0"), None);
        // IO-tile neighbor: logic_op_ aliases to the logic LC output.
        let a = neigh_op_alias(0, 16, "logic_op_rgt_2").expect("aliased");
        assert_eq!(a, (1, 16, "lutff_2/out".to_string()));
    }

    #[test]
    fn glb_wires_are_canonical() {
        let r = Router::build();
        // Every tile's glb_netwk_6 is the same node.
        let a = r.lookup(5, 5, "glb_netwk_6").expect("glb node");
        let b = r.lookup(20, 20, "glb_netwk_6").expect("glb node");
        assert_eq!(a, b);
        assert_eq!(a, r.glb_node(6));
    }
}
