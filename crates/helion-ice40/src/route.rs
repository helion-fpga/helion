//! Route stage: [`PlacedIce`] → [`RoutedIce`].
//!
//! A* router over the HX8K routing graph. The graph is built from the
//! IceStorm intra-tile databases (buffer/routing entries) plus the
//! precomputed span-wire aliasing and IO↔fabric span maps. Span wires that
//! are the same physical copper share one node id.

use crate::device::{io_span_map, span_aliases, tile_db, tile_kind, TileKind};
use crate::place::PlacedIce;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Ordering;

/// A routing-graph node id. Ids below `span_count` are canonical span wires;
/// higher ids are interned `(x, y, name)` triples.
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
    /// wire id -> bounding box of members (for the A* heuristic).
    span_bbox: Vec<(i32, i32, i32, i32)>,
    /// interned (x, y, name) -> node id.
    node_ids: HashMap<(u8, u8, String), NodeId>,
    /// node id -> (x, y) for interned nodes.
    node_xy: Vec<(u8, u8)>,
    adj: Vec<Vec<Edge>>,
    rel_edges: Vec<Vec<RelEdge>>, // per TileKind discriminant
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

impl Router {
    /// Build the full HX8K routing graph. Expensive; build once.
    pub fn build() -> Self {
        let aliases = span_aliases();
        let span_count = aliases.values().max().map(|v| v + 1).unwrap_or(0);
        let io_map = io_span_map();

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
        for &k in &kinds {
            let mut rel = Vec::new();
            for e in tile_db(k) {
                if !matches!(
                    e.func,
                    crate::db::DbFunc::Buffer | crate::db::DbFunc::Routing
                ) {
                    continue;
                }
                if e.src.contains("neigh_op") || e.dst.contains("neigh_op") {
                    continue; // v1: spans carry everything
                }
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
            }
            rel_edges.push(rel);
        }

        let mut r = Router {
            span_count,
            span_bbox,
            node_ids: HashMap::new(),
            node_xy: Vec::new(),
            adj: Vec::new(),
            rel_edges,
        };
        // Size adjacency for span wires + room for interned nodes.
        r.adj.resize(span_count as usize + 4096, Vec::new());

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
                    let src_id = r.resolve(x, y, src, aliases, io_map);
                    let dst_id = r.resolve(x, y, dst, aliases, io_map);
                    if src_id == dst_id {
                        continue;
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

    /// Resolve `(x, y, name)` to a node id.
    fn resolve(
        &mut self,
        x: u8,
        y: u8,
        name: &str,
        aliases: &HashMap<(u8, u8, String), u32>,
        io_map: &HashMap<(u8, u8, String), (u8, u8, String)>,
    ) -> NodeId {
        // Span wire in the fabric?
        if let Some(wid) = aliases.get(&(x, y, name.to_string())) {
            return *wid;
        }
        // IO tile span -> fabric span?
        if let Some((lx, ly, lname)) = io_map.get(&(x, y, name.to_string())) {
            if let Some(wid) = aliases.get(&(*lx, *ly, lname.clone())) {
                return *wid;
            }
        }
        // Otherwise an interned tile-local node.
        let key = (x, y, name.to_string());
        if let Some(id) = self.node_ids.get(&key) {
            return *id;
        }
        let id = self.span_count + self.node_xy.len() as u32;
        self.node_ids.insert(key, id);
        self.node_xy.push((x, y));
        id
    }

    pub fn node_count(&self) -> u32 {
        self.span_count + self.node_xy.len() as u32
    }

    /// Config bits for a routed edge (to program in the source tile).
    pub fn edge_bits(&self, kind_idx: usize, rel_idx: usize) -> &[(u8, u8)] {
        &self.rel_edges[kind_idx][rel_idx].bits
    }

    /// Position of a node for the A* heuristic.
    fn node_pos(&self, id: NodeId) -> (i32, i32) {
        if id < self.span_count {
            let b = self.span_bbox[id as usize];
            // Center of the bounding box.
            ((b.0 + b.1) / 2, (b.2 + b.3) / 2)
        } else {
            let (x, y) = self.node_xy[(id - self.span_count) as usize];
            (x as i32, y as i32)
        }
    }

    /// Public node lookup for net endpoints. Returns `None` for span wires
    /// that do not exist (should not happen for valid names).
    pub fn lookup(&self, x: u8, y: u8, name: &str) -> Option<NodeId> {
        let aliases = span_aliases();
        if let Some(wid) = aliases.get(&(x, y, name.to_string())) {
            return Some(*wid);
        }
        if let Some((lx, ly, lname)) = io_span_map().get(&(x, y, name.to_string())) {
            if let Some(wid) = aliases.get(&(*lx, *ly, lname.clone())) {
                return Some(*wid);
            }
        }
        self.node_ids.get(&(x, y, name.to_string())).copied()
    }

    /// A* from `src` to `dst`. `forbidden` holds muxes already used by other
    /// nets: `(tile_x, tile_y, dst_node)`. Returns the edge list.
    fn astar(
        &self,
        src: NodeId,
        dst: NodeId,
        forbidden: &HashSet<(u8, u8, NodeId)>,
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
                other.f.cmp(&self.f).then_with(|| self.node.cmp(&other.node))
            }
        }
        impl PartialOrd for State {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let (tx, ty) = self.node_pos(dst);
        let h = |n: NodeId| -> i64 {
            let (x, y) = self.node_pos(n);
            ((x - tx).abs() + (y - ty).abs()) as i64
        };

        let n = self.node_count() as usize;
        let mut g_score = vec![i64::MAX; n];
        let mut came_from: Vec<Option<(NodeId, Edge)>> = vec![None; n];
        let mut heap = BinaryHeap::new();
        g_score[src as usize] = 0;
        heap.push(State { f: h(src), g: 0, node: src });

        while let Some(State { g, node, .. }) = heap.pop() {
            if node == dst {
                // Reconstruct.
                let mut edges = Vec::new();
                let mut cur = dst;
                while cur != src {
                    let (prev, e) = came_from[cur as usize].clone()?;
                    edges.push(e);
                    cur = prev;
                }
                edges.reverse();
                return Some(edges);
            }
            if g > g_score[node as usize] {
                continue;
            }
            for e in &self.adj[node as usize] {
                // Congestion: using an already-used mux costs extra.
                let congestion = if forbidden.contains(&(e.tile_x, e.tile_y, e.to)) {
                    10_000
                } else {
                    0
                };
                let ng = g + 1 + congestion;
                if ng < g_score[e.to as usize] {
                    g_score[e.to as usize] = ng;
                    came_from[e.to as usize] = Some((node, e.clone()));
                    heap.push(State { f: ng + h(e.to), g: ng, node: e.to });
                }
            }
        }
        None
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
}

/// Route the placed design. Returns the routed nets with their edges.
pub fn route(router: &Router, placed: &PlacedIce) -> Result<RoutedIce, String> {
    // Build net -> (drivers, loads).
    // Driver/load endpoints as node ids.
    struct Endpoint {
        node: NodeId,
        is_driver: bool,
    }
    let mut net_eps: HashMap<String, Vec<Endpoint>> = HashMap::new();
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
        // Clock is a load on lutff_global/clk.
        let n = router.lookup(ff.x, ff.y, "lutff_global/clk");
        add_ep(&ff.clk_net, n, false, &format!("FF {} clk", ff.name))?;
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
        add_ram_ep(&ram_pack.rclk_net, n, false, &format!("RAM {} RCLK", ram_pack.name))?;
        let n = router.lookup(x, y + 1, "ram/WCLK");
        add_ram_ep(&ram_pack.wclk_net, n, false, &format!("RAM {} WCLK", ram_pack.name))?;
        if let Some(re) = &ram_pack.re_net {
            let n = router.lookup(x, y, "ram/RE");
            add_ram_ep(re, n, false, &format!("RAM {} RE", ram_pack.name))?;
        }
        if let Some(we) = &ram_pack.we_net {
            let n = router.lookup(x, y + 1, "ram/WE");
            add_ram_ep(we, n, false, &format!("RAM {} WE", ram_pack.name))?;
        }
        let _ = ram;
    }
    // PLLs: the reference clock is a load on the PLL site's REFERENCECLK
    // `fabout` net (io_tile (13,0) for the bottom PLL, (13,33) for the top
    // one; IceStorm icebox.py pllinfo_db "8k_0"/"8k_1"). The router programs
    // the local-track -> fabout buffer, as arachne-pnr does.
    //
    // The PLL output (PLLOUTGLOBAL) physically drives the dedicated global
    // clock network via the padin_glb_netwk extra bit, which this backend
    // does not emit yet. For v1 the output is driven as a plain net from a
    // logic-cell output node in the tile adjacent to the PLL site so the
    // design routes structurally; see asc.rs for the config-bit emission.
    for (pi, pll) in placed.packed.plls.iter().enumerate() {
        let placed_pll = &placed.plls[pi];
        let (ref_tile, drv_tile) = match placed_pll.kind {
            crate::pack::PllKind::Pad => ((13u8, 0u8), (16u8, 1u8)),
            crate::pack::PllKind::Core => ((13u8, 33u8), (16u8, 32u8)),
        };
        let n = router.lookup(ref_tile.0, ref_tile.1, "fabout");
        add_ep(
            &pll.ref_net,
            n,
            false,
            &format!("PLL {} REFERENCECLK", pll.name),
        )?;
        let n = router.lookup(drv_tile.0, drv_tile.1, "lutff_0/out");
        add_ep(
            &pll.out_net,
            n,
            true,
            &format!("PLL {} PLLOUTGLOBAL", pll.name),
        )?;
    }

    // Route each net: driver -> all loads via a shared tree.
    let mut routed = Vec::new();
    let mut forbidden: HashSet<(u8, u8, NodeId)> = HashSet::new();
    // Sort nets by load count descending (big nets first).
    let mut nets: Vec<(String, Vec<Endpoint>)> = net_eps.into_iter().collect();
    nets.sort_by_key(|(_, eps)| std::cmp::Reverse(eps.iter().filter(|e| !e.is_driver).count()));

    for (name, eps) in nets {
        let drivers: Vec<NodeId> = eps.iter().filter(|e| e.is_driver).map(|e| e.node).collect();
        let loads: Vec<NodeId> = eps.iter().filter(|e| !e.is_driver).map(|e| e.node).collect();
        if drivers.is_empty() {
            return Err(format!("route: net '{name}' has no driver"));
        }
        if drivers.len() > 1 {
            return Err(format!("route: net '{name}' has {} drivers", drivers.len()));
        }
        if loads.is_empty() {
            continue; // no loads: nothing to route
        }
        let driver = drivers[0];
        // Iterative tree: keep a set of reached nodes; route each load from
        // the nearest reached node.
        let mut reached: HashSet<NodeId> = HashSet::new();
        reached.insert(driver);
        let mut tree_edges: Vec<Edge> = Vec::new();
        // Route loads nearest-first for a better tree.
        let mut remaining = loads.clone();
        while !remaining.is_empty() {
            // Find the (load, source) pair with the shortest path.
            let mut best: Option<(usize, NodeId, Vec<Edge>)> = None;
            for (i, &load) in remaining.iter().enumerate() {
                if reached.contains(&load) {
                    best = Some((i, load, Vec::new()));
                    break;
                }
                // Try routing from the driver; then from tree nodes.
                // (Full nearest-tree search is O(L*T); driver-first is fine.)
                let sources: Vec<NodeId> = reached.iter().copied().collect();
                for src in sources {
                    if let Some(path) = router.astar(src, load, &forbidden) {
                        let better = best.as_ref().map(|(_, _, p)| path.len() < p.len()).unwrap_or(true);
                        if better {
                            best = Some((i, src, path));
                        }
                        break; // first reached source is fine (driver usually)
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            let (li, _src, path) = best
                .ok_or_else(|| format!("route: net '{name}': no path to load"))?;
            let load = remaining.remove(li);
            for e in &path {
                forbidden.insert((e.tile_x, e.tile_y, e.to));
                reached.insert(e.to);
            }
            reached.insert(load);
            tree_edges.extend(path);
        }
        routed.push(RoutedNet {
            name,
            edges: tree_edges
                .into_iter()
                .map(|e| RoutedEdge {
                    tile_x: e.tile_x,
                    tile_y: e.tile_y,
                    kind_idx: e.kind_idx,
                    rel_idx: e.rel_idx,
                })
                .collect(),
        });
    }

    Ok(RoutedIce {
        placed: placed.clone(),
        nets: routed,
    })
}
