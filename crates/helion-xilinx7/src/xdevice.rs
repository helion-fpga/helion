//! Adapter: implement the place/route/emit query traits for [`Device`].
//!
//! [`XDevice`] wraps the X-Ray [`Device`] model and answers the narrow
//! queries the flow stages need, translating between absolute site names
//! (`IOB_X1Y76`) and FASM-relative names (`IOB_Y0` master / `IOB_Y1`
//! slave by site type).

use crate::device::Device;
use crate::db::parse::PpipKind;
use crate::emit::EmitDevice;
use crate::place::DeviceQuery;
use crate::route::{BufgSpine, BufgTieoff, RouteQuery, RoutedPip};
use std::collections::{HashMap, HashSet};

/// Parse a full slice site name `SLICE_X{x}Y{y}` into `(x, y)`. Even x is
/// the X0/M slice, odd x the X1/L slice; tile Y matches name Y. Shared by
/// the cascade neighbor rule so fakes and the device cannot drift apart.
pub(crate) fn parse_slice_xy(site: &str) -> Option<(i32, i32)> {
    let rest = site.strip_prefix("SLICE_X")?;
    let (xs, ys) = rest.split_once('Y')?;
    Some((xs.parse().ok()?, ys.parse().ok()?))
}

/// The group id of a replicated hard-block driver copy, from its `_rep{g}`
/// suffix written by `pack::replicate_hard_block_drivers`. `None` for any
/// other cell. Used to place a copy near the RAMs/DSPs its group feeds (G60).
pub(crate) fn rep_group(name: &str) -> Option<usize> {
    let (_, g) = name.rsplit_once("_rep")?;
    g.parse().ok()
}

/// Precomputed routing node maps, built once per [`Device`] by
/// [`build_node_map`] and shared (cloned) across flow stages.
///
/// Tuple fields: `(wire_id -> node_id, node_id -> member wire_ids,
/// wire_id -> (tile_idx, wire_idx))`.
pub type NodeMaps = (
    Vec<u32>,
    HashMap<u32, Vec<u32>>,
    Vec<(usize, u32)>,
);

/// [`Device`] plus per-run state (clock nets, node stitching).
pub struct XDevice<'a> {
    pub device: &'a Device,
    /// Nets driven by BUFGCTRL outputs (dedicated clock spine).
    pub clock_nets: HashSet<String>,
    /// wire_id -> node_id (u32::MAX if none).
    wire_to_node: Vec<u32>,
    /// node_id -> member wire_ids.
    node_members: HashMap<u32, Vec<u32>>,
    /// The same, flat for the router's inner loop: node `n`'s members are
    /// `node_list[node_off[n]..node_off[n + 1]]`.
    node_off: Vec<u32>,
    node_list: Vec<u32>,
    /// wire_id -> (tile_idx, wire_idx).
    wire_to_tile: Vec<(usize, u32)>,
    /// Router claims by wire id, rebuilt when the claim map grows.
    claim_cache: std::cell::RefCell<(usize, HashMap<u32, String>)>,
    /// Claim owner per wire id (0 free) and interned net ids.
    owner: std::cell::RefCell<(Vec<u32>, HashMap<String, u32>)>,
    /// Per-wire routing flags (see `wire_flags`).
    flags: std::cell::RefCell<Vec<u8>>,
    /// Reusable A* state.
    search: std::cell::RefCell<SearchBuf>,
    /// Congestion history per wire id (times a wire was fought over).
    history: std::cell::RefCell<Vec<u16>>,
    /// PathFinder phase: nets on each wire's node (per wire id), and the
    /// cost each of them adds to taking it.
    pf: std::cell::RefCell<Option<(Vec<u16>, u32)>>,
    /// Signal nets on `BYP_ALT*` (the bounce into the same tile's inputs):
    /// `None` keeps them off everywhere; `Some(closed)` opens them except in
    /// the INT tiles listed (see `RouteQuery::open_bypass_bounce`).
    byp_closed: std::cell::RefCell<Option<HashSet<usize>>>,
    /// Per search, by node: whether a dead-end node leads into the target
    /// (stamp of the search that computed it, and the answer).
    approach: std::cell::RefCell<(Vec<u32>, Vec<u8>)>,
    /// PathFinder's cost-to-go estimate (see [`Lookahead`]), built on first use.
    lookahead: std::cell::OnceCell<Lookahead>,
}

/// Per wire class (tile type, wire index) and tile offset to a CLB input
/// pin: the cheapest path's cost (pip delay + `PIP_BASE`) on the empty
/// device, as VPR's map lookahead and nextpnr router2 do. The flat
/// 30 ps/tile estimate is about half the real cost of a short route (each
/// pip is about 160 ps), so PathFinder's searches flooded every wire near
/// the load (G77).
struct Lookahead {
    /// Per tile type id, per wire index: class (`u32::MAX`: none).
    cls: Vec<Vec<u32>>,
    /// `[class][target kind][dy + R][dx + R]`, `u16::MAX` where no path was found.
    table: Vec<u16>,
}

/// Target tile types the lookahead covers (the table's target kinds).
const LA_KINDS: [&str; 4] = ["CLBLL_L", "CLBLL_R", "CLBLM_L", "CLBLM_R"];
/// Lookahead reach in grid tiles each way; past it the flat estimate adds on.
const LA_R: i32 = 16;
const LA_S: usize = (2 * LA_R + 1) as usize;
/// Search per sample target, in grid tiles each way, so paths may stray
/// past the reach.
const LA_WINDOW: i32 = LA_R + 6;
const PIP_BASE: u32 = 15;
// Estimated cost to go per tile. A hex costs about 27 ps per tile
// (pip delay + PIP_BASE over 6 tiles); at 90 the search was close to
// greedy and extended whichever tree wire lay nearest the load, so
// the loads of a chip-wide net were reached one after another (G38).
const EST_TILE_PS: u32 = 30;
// Past the lookahead's reach a route rides long lines: LH12 crosses about 28
// grid columns and LV18 18 rows for one 301 ps pip (+ PIP_BASE), 11.3
// and 17.6 ps per tile. Charging 30 there overestimated a long hop 2-3x, and
// the search (each wire expanded once) settled on a hex chain about 50%
// slower than the long-line route (G95).
// Only for nets near the critical path: on a chip-wide non-critical net
// (Ibex's reset, 931 loads) the honest estimate let far tree seeds compete
// with near ones and the search expanded 4.5x the wires, for no timing gain.
const FAR_X_PS: u32 = 11;
const FAR_Y_PS: u32 = 17;
/// Criticality (`RouteOpts::crit`) from which a net's searches use the long-line rates.
const LONG_LINE_CRIT: u8 = 64;

/// Estimated cost of the part of an offset past the lookahead's reach, at
/// long-line rates (`long`) or `EST_TILE_PS`.
fn far_ps(dx: i32, dy: i32, long: bool) -> u32 {
    let (rx, ry) = if long { (FAR_X_PS, FAR_Y_PS) } else { (EST_TILE_PS, EST_TILE_PS) };
    rx * (dx.abs() - LA_R).max(0) as u32 + ry * (dy.abs() - LA_R).max(0) as u32
}

/// Flat estimate for an offset: `EST_TILE_PS` within the reach, [`far_ps`] past it.
fn flat_ps(dx: i32, dy: i32, long: bool) -> u32 {
    EST_TILE_PS * (dx.abs().min(LA_R) + dy.abs().min(LA_R)) as u32 + far_ps(dx, dy, long)
}

impl Lookahead {
    /// Estimated cost from wire `wi` of a tile of type `ty` to a pin of a
    /// `kind` tile `(dx, dy)` grid tiles away.
    fn est(&self, kind: usize, ty: u32, wi: u32, dx: i32, dy: i32, long: bool) -> Option<u32> {
        let c = *self.cls.get(ty as usize)?.get(wi as usize)?;
        if c == u32::MAX {
            return None;
        }
        let (cx, cy) = ((dx.clamp(-LA_R, LA_R) + LA_R) as usize, (dy.clamp(-LA_R, LA_R) + LA_R) as usize);
        let v = self.table[((c as usize * LA_KINDS.len() + kind) * LA_S + cy) * LA_S + cx];
        (v != u16::MAX).then(|| v as u32 + far_ps(dx, dy, long))
    }
}

#[derive(Default)]
struct SearchBuf {
    generation: u32,
    stamp: Vec<u32>,
    best: Vec<u32>,
    parent: Vec<u32>,
    /// Stamp of the search that expanded the wire: a wire is expanded once
    /// (the distance estimate is not exact on long wires, and reopening
    /// wires made failing searches visit each one many times).
    closed: Vec<u32>,
}

impl<'a> XDevice<'a> {
    pub fn new(device: &'a Device, maps: NodeMaps) -> Self {
        let (wire_to_node, node_members, wire_to_tile) = maps;
        let n = wire_to_tile.len();
        let mut node_off = vec![0u32; n + 1];
        for (&nid, m) in &node_members {
            if (nid as usize) < n {
                node_off[nid as usize + 1] = m.len() as u32;
            }
        }
        for i in 0..n {
            node_off[i + 1] += node_off[i];
        }
        let mut node_list = vec![0u32; node_off[n] as usize];
        for (&nid, m) in &node_members {
            if (nid as usize) < n {
                let o = node_off[nid as usize] as usize;
                node_list[o..o + m.len()].copy_from_slice(m);
            }
        }
        Self {
            device,
            clock_nets: HashSet::new(),
            byp_closed: std::cell::RefCell::new(None),
            wire_to_node,
            node_members,
            node_off,
            node_list,
            wire_to_tile,
            claim_cache: std::cell::RefCell::new((usize::MAX, HashMap::new())),
            owner: std::cell::RefCell::new((Vec::new(), HashMap::new())),
            flags: std::cell::RefCell::new(Vec::new()),
            search: std::cell::RefCell::new(SearchBuf::default()),
            history: std::cell::RefCell::new(Vec::new()),
            pf: std::cell::RefCell::new(None),
            approach: std::cell::RefCell::new((Vec::new(), Vec::new())),
            lookahead: std::cell::OnceCell::new(),
        }
    }

    /// Build the [`Lookahead`]: for two tiles of each kind (a third and two
    /// thirds across the device, mid height), a Dijkstra backwards from all
    /// of the tile's input pins over the wires the router may take, keeping
    /// the cheapest cost per wire class and offset (the minimum over the two
    /// samples, so columns of other tile types near one sample do not
    /// inflate it). Deterministic: fixed samples, minimum costs.
    /// `flags`: [`Self::wire_flags`] (the caller holds them borrowed).
    fn build_lookahead(&self, flags: &[u8]) -> Lookahead {
        use std::cmp::Reverse;
        let t0 = std::time::Instant::now();
        let dev = self.device;
        let tiles: Vec<&crate::device::Tile> = (0..dev.tile_count()).filter_map(|i| dev.tile(i)).collect();
        let ntypes = tiles.iter().map(|t| t.type_id as usize + 1).max().unwrap_or(0);
        let (gx, gy) = tiles.iter().fold((0, 0), |(x, y), t| (x.max(t.grid_x), y.max(t.grid_y)));
        // Pips into each wire, per tile type: (source wire index, cost, pseudo).
        type PipsInto = Vec<Vec<(u32, u32, bool)>>;
        let mut into: Vec<Option<PipsInto>> = vec![None; ntypes];
        let mut la = Lookahead { cls: vec![Vec::new(); ntypes], table: Vec::new() };
        let n = self.wire_to_tile.len();
        let mut dist = vec![u32::MAX; n];
        let mut touched: Vec<u32> = Vec::new();
        // A wire the router may take (power, GCLK and clock-buffer wires excluded).
        let open = |id: u32| flags[id as usize] & (1 | 16) == 0;
        for (kind, ty) in LA_KINDS.iter().enumerate() {
            for third in [1, 2] {
                let (fx, fy) = (gx * third / 3, gy / 2);
                let Some(ci) = (0..tiles.len())
                    .filter(|&i| dev.tile_type_name(i) == Some(*ty))
                    .min_by_key(|&i| ((tiles[i].grid_x - fx).abs() + (tiles[i].grid_y - fy).abs(), i))
                else {
                    continue;
                };
                let (cx, cy) = (tiles[ci].grid_x, tiles[ci].grid_y);
                let inside = |id: u32| {
                    let (x, y) = self.wire_xy(id);
                    (x - cx).abs() <= LA_WINDOW && (y - cy).abs() <= LA_WINDOW
                };
                for &t in &touched {
                    dist[t as usize] = u32::MAX;
                }
                touched.clear();
                let mut heap = std::collections::BinaryHeap::new();
                let base = dev.wire_id(ci, 0).unwrap_or(0);
                for wi in 0..dev.wires(ci).map_or(0, |w| w.len()) as u32 {
                    let id = base + wi;
                    if flags[id as usize] & 128 != 0 && open(id) {
                        dist[id as usize] = 0;
                        touched.push(id);
                        heap.push(Reverse((0u32, id)));
                    }
                }
                while let Some(Reverse((c, v))) = heap.pop() {
                    if c > dist[v as usize] {
                        continue;
                    }
                    let (ti, wi) = self.wire_to_tile[v as usize];
                    let t = tiles[ti];
                    let (dx, dy) = (cx - t.grid_x, cy - t.grid_y);
                    // Interconnect and CLB wires only: other tile types keep the flat estimate.
                    if dx.abs() <= LA_R && dy.abs() <= LA_R && flags[v as usize] & (4 | 8) != 0 {
                        let cl = &mut la.cls[t.type_id as usize];
                        if cl.is_empty() {
                            cl.resize(dev.wires(ti).map_or(0, |w| w.len()), u32::MAX);
                        }
                        if cl[wi as usize] == u32::MAX {
                            cl[wi as usize] = (la.table.len() / (LA_KINDS.len() * LA_S * LA_S)) as u32;
                            la.table.resize(la.table.len() + LA_KINDS.len() * LA_S * LA_S, u16::MAX);
                        }
                        let k = ((cl[wi as usize] as usize * LA_KINDS.len() + kind) * LA_S + (dy + LA_R) as usize) * LA_S + (dx + LA_R) as usize;
                        la.table[k] = la.table[k].min(c.min(u16::MAX as u32 - 1) as u16);
                    }
                    let mut relax = |u: u32, cu: u32, heap: &mut std::collections::BinaryHeap<Reverse<(u32, u32)>>| {
                        if cu < dist[u as usize] && open(u) && inside(u) {
                            if dist[u as usize] == u32::MAX {
                                touched.push(u);
                            }
                            dist[u as usize] = cu;
                            heap.push(Reverse((cu, u)));
                        }
                    };
                    // Node siblings: free (the forward search's node hop).
                    let nid = self.wire_to_node[v as usize];
                    if nid != u32::MAX {
                        for &m in self.node_wires(nid) {
                            relax(m, c, &mut heap);
                        }
                    }
                    // Pips into v: the forward search takes usable pips, no
                    // pseudo-pip in a CLB tile, none out of a global buffer input.
                    let lists = into[t.type_id as usize].get_or_insert_with(|| {
                        let nw = dev.wires(ti).map_or(0, |w| w.len());
                        let mut l = vec![Vec::new(); nw];
                        for sw in 0..nw as u32 {
                            for p in dev.pips_from_idx(ti, sw).filter(|p| p.usable) {
                                if let Some(e) = l.get_mut(p.dst as usize) {
                                    e.push((sw, p.delay_ps as u32 + PIP_BASE, p.is_pseudo));
                                }
                            }
                        }
                        l
                    });
                    let tb = dev.wire_id(ti, 0).unwrap_or(0);
                    for &(sw, cost, pseudo) in &lists[wi as usize] {
                        let u = tb + sw;
                        if flags[u as usize] & 64 != 0 || (pseudo && flags[u as usize] & 4 != 0) {
                            continue;
                        }
                        relax(u, c + cost, &mut heap);
                    }
                }
            }
        }
        if debug_env("HELION_ROUTE_STATS") {
            eprintln!("route_stat lookahead classes {} in {:.1}s", la.table.len() / (LA_KINDS.len() * LA_S * LA_S), t0.elapsed().as_secs_f64());
        }
        la
    }

    /// Members of node `nid` (empty for a wire with no node).
    fn node_wires(&self, nid: u32) -> &[u32] {
        match (self.node_off.get(nid as usize), self.node_off.get(nid as usize + 1)) {
            (Some(&a), Some(&b)) => &self.node_list[a as usize..b as usize],
            _ => &[],
        }
    }

    fn wire_name_ref(&self, wid: u32) -> Option<&str> {
        let (ti, wi) = self.wire_to_tile.get(wid as usize).copied()?;
        self.device.wires(ti)?.get(wi as usize).map(|s| s.as_str())
    }

    fn wire_xy(&self, wid: u32) -> (i32, i32) {
        self.wire_to_tile
            .get(wid as usize)
            .and_then(|&(ti, _)| self.device.tile(ti))
            .map(|t| (t.grid_x, t.grid_y))
            .unwrap_or((0, 0))
    }

    /// Mark `id` and every wire of its node as owned by `oid` (a node is one
    /// electrical net: two nets on different wires of it short together).
    fn mark_owner(&self, owner: &mut [u32], id: u32, oid: u32) {
        let nid = self.wire_to_node.get(id as usize).copied().unwrap_or(u32::MAX);
        if nid != u32::MAX {
            if let Some(members) = self.node_members.get(&nid) {
                for &m in members {
                    if let Some(slot) = owner.get_mut(m as usize) {
                        if *slot == 0 {
                            *slot = oid;
                        }
                    }
                }
            }
        }
        if let Some(slot) = owner.get_mut(id as usize) {
            *slot = oid;
        }
    }

    /// Per-wire flags: bit 0 power/clock wire, bit 1 BYP_ALT, bit 2 in a CLB
    /// tile (pseudo-pips there are slice configuration), bit 3 INT tile,
    /// bit 4 GCLK_ wire, bit 5 clock wire, bit 6 global buffer input, bit 7
    /// dead end (no member of the wire's node has an outgoing pip, so a
    /// route can only end there), computed once.
    fn wire_flags(&self) -> std::cell::Ref<'_, Vec<u8>> {
        {
            let mut f = self.flags.borrow_mut();
            if f.is_empty() {
                let n = self.wire_to_tile.len();
                f.resize(n, 0);
                for id in 0..n {
                    let (ti, wi) = self.wire_to_tile[id];
                    let Some(w) = self.device.wires(ti).and_then(|ws| ws.get(wi as usize)) else { continue };
                    let mut b = 0u8;
                    if w == "GND_WIRE" || w == "VCC_WIRE" || w.starts_with("GFAN") {
                        b |= 1;
                    }
                    if w.starts_with("GCLK_") {
                        b |= 16;
                    }
                    if w.starts_with("GCLK") || w.starts_with("CLK") {
                        b |= 32;
                    }
                    if w.starts_with("BYP_ALT") {
                        b |= 2;
                    }
                    // A global buffer's input (CLK_BUFG_BUFGCTRL<n>_I0/I1):
                    // a route ends there. Leaving it means passing through
                    // the buffer, which only works when that buffer is used.
                    if w.starts_with("CLK_BUFG_BUFGCTRL") && (w.ends_with("_I0") || w.ends_with("_I1")) {
                        b |= 64;
                    }
                    if self.device.tile(ti).map(|t| t.name.starts_with("CLB")).unwrap_or(false) {
                        b |= 4;
                    }
                    if matches!(self.device.tile_type_name(ti), Some("INT_L") | Some("INT_R")) {
                        b |= 8;
                    }
                    f[id] = b;
                }
                // Dead ends, per node: every pip the search could take out of
                // the node leads only to dead nodes (a LUT pin has none; the
                // interconnect input feeding it has one, to that pin).
                // Only pips the search takes count: ones with configuration
                // bits, and no pseudo-pip inside a CLB tile (those are LUT
                // route-throughs, slice configuration).
                let has_out: Vec<bool> = (0..n)
                    .map(|id| {
                        let (ti, wi) = self.wire_to_tile[id];
                        let clb = f[id] & 4 != 0;
                        self.device.pips_from_idx(ti, wi).any(|p| p.usable && !(p.is_pseudo && clb))
                    })
                    .collect();
                for id in 0..n {
                    let nid = self.wire_to_node[id];
                    let out = if nid == u32::MAX {
                        has_out[id]
                    } else {
                        self.node_wires(nid).iter().any(|&m| has_out[m as usize])
                    };
                    if !out {
                        f[id] |= 128;
                    }
                }
                // Then nodes whose every exit is dead, until nothing changes
                // (chains are short: interconnect input, LUT pin).
                for _round in 0..6 {
                    let mut changed = 0usize;
                    for id in 0..n {
                        if f[id] & 128 != 0 {
                            continue;
                        }
                        let nid = self.wire_to_node[id];
                        let one = [id as u32];
                        let members: &[u32] = if nid == u32::MAX { &one } else { self.node_wires(nid) };
                        let alive = members.iter().any(|&m| {
                            let (ti, wi) = self.wire_to_tile[m as usize];
                            let clb = f[m as usize] & 4 != 0;
                            let base = self.device.wire_id(ti, 0).unwrap_or(0);
                            self.device
                                .pips_from_idx(ti, wi)
                                .any(|p| p.usable && !(p.is_pseudo && clb) && f[(base + p.dst) as usize] & 128 == 0)
                        });
                        if !alive {
                            for &m in members {
                                f[m as usize] |= 128;
                            }
                            changed += 1;
                        }
                    }
                    if changed == 0 {
                        break;
                    }
                }
                if debug_env("HELION_ROUTE_STATS") {
                    let dead = f.iter().filter(|&&b| b & 128 != 0).count();
                    eprintln!("route_stat dead-end wires {dead} of {n}");
                }
            }
        }
        self.flags.borrow()
    }

    /// A* over integer wire ids: same wire rules as `route::bfs_route`
    /// (power/clock and BYP_ALT wires, foreign claims, CLB pseudo-pips).
    /// Search state lives in flat per-wire arrays stamped per search.
    fn astar(
        &self,
        src: (&str, &str),
        tree: &[(String, String, u32)],
        sinks: &[(String, String)],
        opts: crate::route::RouteOpts,
        net: &str,
        claimed: &HashMap<(String, String), String>,
    ) -> Option<Option<(Vec<RoutedPip>, Vec<(u32, u32)>)>> {
        use std::cmp::Reverse;
        let (allow_power, allow_byp, no_int, clock) = (opts.allow_power, opts.allow_byp, opts.no_int, opts.clock);
        use std::collections::BinaryHeap;
        let start = self.wire_id_of(src.0, src.1)?;
        let targets: Vec<u32> = sinks.iter().filter_map(|(t, w)| self.wire_id_of(t, w)).collect();
        if targets.is_empty() {
            return None;
        }
        let n = self.wire_to_tile.len();
        // Claims: owner id per wire (0 = free), nets interned.
        {
            let mut cache = self.claim_cache.borrow_mut();
            if cache.0 != claimed.len() {
                let mut m = HashMap::with_capacity(claimed.len());
                for ((t, w), nn) in claimed {
                    if let Some(id) = self.wire_id_of(t, w) {
                        m.insert(id, nn.clone());
                    }
                }
                *cache = (claimed.len(), m);
                let mut own = self.owner.borrow_mut();
                own.0.clear();
                own.0.resize(n, 0);
                own.1.clear();
                own.1.insert(String::new(), 0);
                let entries: Vec<(u32, String)> = cache.1.iter().map(|(k, v)| (*k, v.clone())).collect();
                for (id, nn) in entries {
                    let len = own.1.len() as u32;
                    let oid = *own.1.entry(nn).or_insert(len);
                    let (vec, _) = &mut *own;
                    self.mark_owner(vec, id, oid);
                }
            }
        }
        let own = self.owner.borrow();
        let my = own.1.get(net).copied().unwrap_or(u32::MAX);
        // Clock nets keep their wires: they are never displaced.
        let locked: Vec<u32> = self.clock_nets.iter().filter_map(|c| own.1.get(c).copied()).collect();
        let flags = self.wire_flags();
        let byp_closed = self.byp_closed.borrow();
        let allowed = |id: u32| -> bool {
            let f = flags[id as usize];
            if !allow_power && f & 1 != 0 {
                return false;
            }
            if !allow_power && !clock && !opts.allow_gclk && f & 16 != 0 {
                return false;
            }
            // A clock stays on clock wires inside INT tiles.
            if clock && f & 8 != 0 && f & 32 == 0 {
                return false;
            }
            if !allow_power && !allow_byp && f & 2 != 0 {
                match byp_closed.as_ref() {
                    Some(closed) if !closed.contains(&self.wire_to_tile[id as usize].0) => {}
                    _ => return false,
                }
            }
            if no_int && f & 8 != 0 {
                return false;
            }
            let o = own.0.get(id as usize).copied().unwrap_or(0);
            o == 0 || o == my || (opts.negotiate && !locked.contains(&o))
        };
        // Extra cost of taking `id`: congestion history, plus the price of
        // displacing another net when negotiating.
        const PRESENT_PS: u32 = 1500;
        const HISTORY_PS: u32 = 300;
        let hist = self.history.borrow();
        let pf = self.pf.borrow();
        let extra = |id: u32| -> u32 {
            let h = hist.get(id as usize).copied().unwrap_or(0) as u32;
            let mut c = h * HISTORY_PS;
            if let Some((occ, ps)) = pf.as_ref() {
                // PathFinder's (base + history) x present: history scales with
                // the sharing price, so a wire that stays contested ends up
                // dearer than sharing two fresh ones.
                c = c.saturating_add((occ[id as usize] as u32).saturating_mul(*ps).saturating_mul(1 + h)).min(1_000_000_000);
            }
            if opts.negotiate {
                let o = own.0.get(id as usize).copied().unwrap_or(0);
                if o != 0 && o != my {
                    c = c.saturating_add(PRESENT_PS);
                }
            }
            // A critical net pays down to a fifth of the congestion price.
            (c as u64 * (320 - opts.crit as u64) / 320) as u32
        };
        let is_target = |id: u32| targets.contains(&id);
        // A dead-end wire is worth visiting only on the way into a target:
        // the target itself or a wire of the target's node.
        let target_nodes: Vec<u32> = targets.iter().map(|&t| self.wire_to_node[t as usize]).filter(|&n| n != u32::MAX).collect();
        let in_target = |w: u32| is_target(w) || target_nodes.contains(&self.wire_to_node[w as usize]);
        // Whether a dead-end wire's node leads into a target within a few
        // pips (memoized per node for this search).
        fn reaches(x: &XDevice<'_>, w: u32, depth: u32, flags: &[u8], in_target: &dyn Fn(u32) -> bool) -> bool {
            let nid = x.wire_to_node[w as usize];
            let one = [w];
            let members: &[u32] = if nid == u32::MAX { &one } else { x.node_wires(nid) };
            if members.iter().any(|&m| in_target(m)) {
                return true;
            }
            if depth == 0 {
                return false;
            }
            members.iter().any(|&m| {
                let (ti, wi) = x.wire_to_tile[m as usize];
                let clb = flags[m as usize] & 4 != 0;
                let base = x.device.wire_id(ti, 0).unwrap_or(0);
                x.device
                    .pips_from_idx(ti, wi)
                    .any(|p| p.usable && !(p.is_pseudo && clb) && reaches(x, base + p.dst, depth - 1, flags, in_target))
            })
        }
        let tgt_xy: Vec<(i32, i32)> = targets.iter().map(|&t| self.wire_xy(t)).collect();

        // PathFinder's exact searches estimate by the lookahead table (into a
        // CLB pin; other targets and classes keep the flat estimate).
        let la = (pf.is_some() && !opts.negotiate && !clock && !allow_power && !opts.any_pip).then(|| self.lookahead.get_or_init(|| self.build_lookahead(&flags)));
        let tgt_kind: Vec<Option<usize>> = targets
            .iter()
            .map(|&t| la.and_then(|_| self.device.tile_type_name(self.wire_to_tile[t as usize].0)).and_then(|n| LA_KINDS.iter().position(|k| *k == n)))
            .collect();
        let long = opts.crit >= LONG_LINE_CRIT;
        let h = |id: u32| -> u32 {
            let (ti, wi) = self.wire_to_tile[id as usize];
            let Some(t) = self.device.tile(ti) else { return 0 };
            let (x, y) = (t.grid_x, t.grid_y);
            tgt_xy
                .iter()
                .zip(&tgt_kind)
                .map(|(&(tx, ty), k)| {
                    // Negotiating adds penalties near the sinks; a steeper
                    // estimate keeps that search aimed at them instead of flooding
                    // everything within the penalty's reach. Not for a critical
                    // net: at 220 ps per tile the search was near greedy and took a
                    // 10.3 ns chain of single hops around taken hexes (G95).
                    if opts.negotiate && !long {
                        return ((x - tx).abs() + (y - ty).abs()) as u32 * 220;
                    }
                    k.and_then(|k| la?.est(k, t.type_id, wi, tx - x, ty - y, long)).unwrap_or_else(|| flat_ps(tx - x, ty - y, long))
                })
                .min()
                .unwrap_or(0)
        };
        let mut sb = self.search.borrow_mut();
        if sb.stamp.len() != n {
            sb.stamp = vec![0; n];
            sb.best = vec![0; n];
            sb.parent = vec![u32::MAX; n];
            sb.closed = vec![0; n];
            sb.generation = 0;
        }
        sb.generation = sb.generation.wrapping_add(1);
        if sb.generation == 0 {
            sb.stamp.iter_mut().for_each(|x| *x = 0);
            sb.closed.iter_mut().for_each(|x| *x = 0);
            sb.generation = 1;
        }
        let stampv = sb.generation;
        // (The permissive search takes pips without bits too: no pruning.)
        let mut ap = self.approach.borrow_mut();
        if ap.0.len() != n || stampv == 1 {
            // New buffer, or the stamps wrapped around: forget old answers.
            ap.0 = vec![0; n];
            ap.1 = vec![0; n];
        }
        let mut leads_to_target = |id: u32| -> bool {
            if opts.any_pip || flags[id as usize] & 128 == 0 {
                return true;
            }
            let nid = self.wire_to_node[id as usize];
            let key = if nid == u32::MAX { id } else { nid } as usize;
            if ap.0[key] == stampv {
                return ap.1[key] != 0;
            }
            let b = reaches(self, id, 3, &flags, &in_target);
            ap.0[key] = stampv;
            ap.1[key] = b as u8;
            b
        };
        let mut heap: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::new();
        macro_rules! seen {
            ($id:expr) => {
                sb.stamp[$id as usize] == stampv
            };
        }
        sb.stamp[start as usize] = stampv;
        sb.best[start as usize] = 0;
        sb.parent[start as usize] = u32::MAX;
        heap.push(Reverse((h(start), 0, start)));
        let mut near = (h(start), start);
        for (t, w, d) in tree {
            if let Some(id) = self.wire_id_of(t, w) {
                // Branch only from general interconnect: wires past a site
                // pin (IOI/OLOGIC route-throughs, CLB pins) are not fanout
                // points.
                if !clock && flags[id as usize] & 8 == 0 {
                    continue;
                }
                if clock && flags[id as usize] & 4 != 0 {
                    continue;
                }
                if !seen!(id) || *d < sb.best[id as usize] {
                    sb.stamp[id as usize] = stampv;
                    sb.best[id as usize] = *d;
                    sb.parent[id as usize] = u32::MAX;
                    heap.push(Reverse((d + h(id), *d, id)));
                }
                let dist = h(id);
                if dist < near.0 {
                    near = (dist, id);
                }
            }
        }
        // Search region: the box spanned by the sinks, the net's source and
        // the nearest wire of its tree. Without the source a far load could
        // only branch off the tree near it.
        // PathFinder widens the box per net that keeps sharing (route::pf_margin).
        let bbox = opts.bbox;
        let region = (bbox > 0).then(|| {
            let (sx, sy) = self.wire_xy(near.1);
            let (qx, qy) = self.wire_xy(start);
            let (mut x0, mut x1, mut y0, mut y1) = (sx.min(qx), sx.max(qx), sy.min(qy), sy.max(qy));
            for &(x, y) in &tgt_xy {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
            (x0 - bbox, x1 + bbox, y0 - bbox, y1 + bbox)
        });
        let inside = |id: u32| -> bool {
            match region {
                None => true,
                Some((x0, x1, y0, y1)) => {
                    let (x, y) = self.wire_xy(id);
                    x >= x0 && x <= x1 && y >= y0 && y <= y1
                }
            }
        };
        let mut found = None;
        let mut expanded = 0usize;
        let limit = match (opts.cap_detour, heap.peek()) {
            (true, Some(Reverse((f0, _, _)))) => f0.saturating_mul(2).saturating_add(1500),
            _ => u32::MAX,
        };
        while let Some(Reverse((f, g, id))) = heap.pop() {
            if f > limit {
                break;
            }
            if g > sb.best[id as usize] || sb.closed[id as usize] == stampv {
                continue;
            }
            sb.closed[id as usize] = stampv;
            if is_target(id) {
                found = Some(id);
                break;
            }
            expanded += 1;
            // The exact search closes each wire once, so it ends by itself
            // after at most the device's wire count (any cap below that,
            // on any part, would fail a net that has a path).
            if opts.negotiate && expanded > 6_000_000 {
                break;
            }
            // Node siblings (tileconn stitching): free.
            let nid = self.wire_to_node[id as usize];
            if nid != u32::MAX {
                {
                    for &m in self.node_wires(nid) {
                        if m == id || !allowed(m) || !inside(m) || !leads_to_target(m) {
                            continue;
                        }
                        // Tree wires (seeds) are fixed: a cheaper second
                        // path into one would give that wire two drivers.
                        if seen!(m) && sb.parent[m as usize] == u32::MAX {
                            continue;
                        }
                        let gm = g.saturating_add(extra(m));
                        if sb.closed[m as usize] == stampv {
                            continue;
                        }
                        if !seen!(m) || gm < sb.best[m as usize] {
                            sb.stamp[m as usize] = stampv;
                            sb.best[m as usize] = gm;
                            sb.parent[m as usize] = id;
                            heap.push(Reverse((gm.saturating_add(h(m)), gm, m)));
                        }
                    }
                }
            }
            // Never through a global buffer (see wire_flags).
            if flags[id as usize] & 64 != 0 {
                continue;
            }
            let (ti, wi) = self.wire_to_tile[id as usize];
            let clb = flags[id as usize] & 4 != 0;
            let base = self.device.wire_id(ti, 0).unwrap_or(0);
            for pip in self.device.pips_from_idx(ti, wi) {
                if (!pip.usable && !opts.any_pip) || (pip.is_pseudo && clb) {
                    continue;
                }
                let dst = base + pip.dst;
                if !allowed(dst) || !inside(dst) || !leads_to_target(dst) {
                    continue;
                }
                let ng = g.saturating_add(pip.delay_ps as u32 + PIP_BASE).saturating_add(extra(dst));
                if seen!(dst) && sb.parent[dst as usize] == u32::MAX {
                    continue;
                }
                if sb.closed[dst as usize] == stampv {
                    continue;
                }
                if !seen!(dst) || ng < sb.best[dst as usize] {
                    sb.stamp[dst as usize] = stampv;
                    sb.best[dst as usize] = ng;
                    sb.parent[dst as usize] = id;
                    heap.push(Reverse((ng.saturating_add(h(dst)), ng, dst)));
                }
            }
        }
        LAST_EXPANDED.with(|e| e.set(expanded));
        if debug_env("HELION_ROUTE_STATS") {
            let (cost, dist) = match found {
                Some(e) => (sb.best[e as usize], h(start) / EST_TILE_PS),
                None => (0, 0),
            };
            eprintln!("route_stat net={net} expanded={expanded} found={} negotiate={} bbox={} cost={cost} dist={dist}", found.is_some(), opts.negotiate, opts.bbox);
        }
        let Some(end) = found else {
            if debug_env("HELION_ROUTE_DEBUG") {
                let blocked: Vec<String> = targets.iter().map(|&t| format!("{:?} owner={} flags={}", self.wire_name_of(t), own.0.get(t as usize).copied().unwrap_or(0), flags[t as usize])).collect();
                eprintln!("route_debug net={net} expanded={expanded} my={my} negotiate={} bbox={} cap={} limit={limit} targets={blocked:?}", opts.negotiate, opts.bbox, opts.cap_detour);
            }
            return Some(None);
        };
        // Walk back to the seed, then forward again adding up real delay
        // (search cost also carries congestion penalties).
        let mut chain = vec![end];
        let mut cur = end;
        while sb.parent[cur as usize] != u32::MAX {
            cur = sb.parent[cur as usize];
            chain.push(cur);
        }
        chain.reverse();
        let mut delay = sb.best[chain[0] as usize];
        let mut pips = Vec::new();
        let mut arr = Vec::new();
        for pair in chain.windows(2) {
            let (prev, cur) = (pair[0], pair[1]);
            let (pt, pw) = self.wire_to_tile[prev as usize];
            let (ct, cw) = self.wire_to_tile[cur as usize];
            if pt == ct {
                // A pip inside the tile (else a node hop).
                if let Some(pip) = self.device.pips_from_idx(pt, pw).find(|p| p.dst == cw) {
                    let wires = self.device.wires(pt)?;
                    pips.push(RoutedPip {
                        tile: self.tile_name(pt)?.to_string(),
                        dst_wire: wires.get(cw as usize)?.clone(),
                        src_wire: wires.get(pw as usize)?.clone(),
                        is_pseudo: pip.is_pseudo,
                    });
                    let next = delay + pip.delay_ps as u32 + PIP_BASE;
                    arr.push((delay, next));
                    delay = next;
                }
            }
        }
        if pips.is_empty() {
            return Some(Some((pips, vec![(delay, delay)])));
        }
        Some(Some((pips, arr)))
    }

    fn tile_idx(&self, tile: &str) -> Option<usize> {
        self.device.tile_by_name(tile)
    }

    fn tile_name(&self, idx: usize) -> Option<&str> {
        self.device.tile(idx).map(|t| t.name.as_str())
    }

    /// (tile, wire) -> wire_id.
    fn wire_id_of(&self, tile: &str, wire: &str) -> Option<u32> {
        let ti = self.tile_idx(tile)?;
        let wi = self.device.wire_index(ti, wire)?;
        self.device.wire_id(ti, wi)
    }

    /// wire_id -> (tile_name, wire_name).
    fn wire_name_of(&self, wid: u32) -> Option<(String, String)> {
        let (ti, wi) = self.wire_to_tile.get(wid as usize).copied()?;
        let tile = self.tile_name(ti)?.to_string();
        let wire = self.device.wires(ti)?.get(wi as usize)?.clone();
        Some((tile, wire))
    }

    /// Find the INT tile whose `IMUX{m}` wire is stitched (via tileconn
    /// through the INT_INTERFACE tile) to the CLK_BUFG tile's `imux_wire`.
    /// Returns `(int_tile_name, int_imux_wire_name)`.
    fn int_imux_for(
        &self,
        clk_tile: &str,
        imux_wire: &str,
        site: &str,
        pin: &str,
    ) -> Result<(String, String), String> {
        let err = |msg: String| format!("tieoff: {msg} (site `{site}` pin `{pin}`)");
        let wid = self
            .wire_id_of(clk_tile, imux_wire)
            .ok_or_else(|| err(format!("no wire `{imux_wire}` on tile `{clk_tile}`")))?;
        let nid = self
            .wire_to_node
            .get(wid as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
            .ok_or_else(|| {
                err(format!(
                    "wire `{clk_tile}`.`{imux_wire}` is not stitched to any INT tile"
                ))
            })?;
        let members = self
            .node_members
            .get(&nid)
            .ok_or_else(|| err(format!("no node members for `{clk_tile}`.`{imux_wire}`")))?;
        // IMUX index, e.g. CLK_BUFG_IMUX4_0 -> "4".
        let idx = imux_wire
            .rsplit("IMUX")
            .next()
            .and_then(|s| s.split('_').next())
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| err(format!("cannot parse IMUX index from `{imux_wire}`")))?;
        let want = format!("IMUX{idx}");
        for &m in members {
            let (tname, wname) = self
                .wire_name_of(m)
                .ok_or_else(|| err(format!("bad wire id {m}")))?;
            if wname != want {
                continue;
            }
            let ti = self
                .tile_idx(&tname)
                .ok_or_else(|| err(format!("unknown tile `{tname}`")))?;
            let ttype = self.device.tile_type_name(ti).unwrap_or("");
            if ttype == "INT_L" || ttype == "INT_R" {
                // The rails must come from a TIEOFF site in this tile.
                let has_tieoff = self
                    .device
                    .tile(ti)
                    .map(|t| t.sites.iter().any(|s| s.site_type == "TIEOFF"))
                    .unwrap_or(false);
                if !has_tieoff {
                    return Err(err(format!("INT tile `{tname}` has no TIEOFF site")));
                }
                return Ok((tname, wname));
            }
        }
        Err(err(format!(
            "no INT tile stitched to `{clk_tile}`.`{imux_wire}`"
        )))
    }

    /// GFAN wire in the INT tile that can carry GND into `imux`:
    /// GND_WIRE -> GFANx -> IMUXm (GND has no direct IMUX pip).
    fn gfan_for_imux(
        &self,
        int_tile: &str,
        imux: &str,
        site: &str,
        pin: &str,
    ) -> Result<String, String> {
        for (gfan_dst, _, _) in RouteQuery::pips_from(self, int_tile, "GND_WIRE") {
            if !gfan_dst.starts_with("GFAN") {
                continue;
            }
            let drives_imux = RouteQuery::pips_from(self, int_tile, &gfan_dst)
                .iter()
                .any(|(d, _, _)| d == imux);
            if drives_imux {
                return Ok(gfan_dst);
            }
        }
        Err(format!(
            "tieoff: no GND_WIRE -> GFAN -> `{imux}` path in `{int_tile}` (site `{site}` pin `{pin}`)"
        ))
    }

    /// Fail loudly unless `src -> dst` is a real pip in `tile`.
    fn require_pip(
        &self,
        tile: &str,
        src: &str,
        dst: &str,
        site: &str,
        pin: &str,
    ) -> Result<(), String> {
        let ok = RouteQuery::pips_from(self, tile, src)
            .iter()
            .any(|(d, _, _)| d == dst);
        if !ok {
            return Err(format!(
                "tieoff: no pip `{src}` -> `{dst}` in `{tile}` (site `{site}` pin `{pin}`)"
            ));
        }
        Ok(())
    }
}

thread_local! {
    /// Wires the last [`XDevice::astar`] on this thread expanded (for tests).
    static LAST_EXPANDED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Build the node -> members map from a mutable Device.
/// Returns (wire_id -> node_id, node_id -> wire_ids, wire_id -> (tile_idx, wire_idx)).
/// Call once after loading; pass the result to [`XDevice::new`].
/// Router debug switches, read once (a lookup per search showed up at 6% of
/// routing time).
fn debug_env(name: &'static str) -> bool {
    use std::sync::OnceLock;
    static STATS: OnceLock<bool> = OnceLock::new();
    static DEBUG: OnceLock<bool> = OnceLock::new();
    let cell = match name {
        "HELION_ROUTE_STATS" => &STATS,
        "HELION_ROUTE_DEBUG" => &DEBUG,
        _ => return std::env::var(name).is_ok(),
    };
    *cell.get_or_init(|| std::env::var(name).is_ok())
}

pub fn build_node_map(device: &mut Device) -> NodeMaps {
    let n = device.tile_count();
    // First pass: find max wire_id.
    let mut max_wid: u32 = 0;
    for idx in 0..n {
        if let Some(base) = device.tile_wire_base(idx) {
            if let Some(wires) = device.wires(idx) {
                let end = base + wires.len() as u32;
                if end > max_wid {
                    max_wid = end;
                }
            }
        }
    }
    let mut wire_to_node: Vec<u32> = vec![u32::MAX; max_wid as usize];
    let mut wire_to_tile: Vec<(usize, u32)> = vec![(0, 0); max_wid as usize];
    let mut node_members: HashMap<u32, Vec<u32>> = HashMap::new();
    for idx in 0..n {
        let base = match device.tile_wire_base(idx) {
            Some(b) => b,
            None => continue,
        };
        let nwires = device.wires(idx).map(|w| w.len()).unwrap_or(0);
        for wi in 0..nwires {
            let wid = base + wi as u32;
            wire_to_tile[wid as usize] = (idx, wi as u32);
            if let Some(nid) = device.node_of(wid) {
                wire_to_node[wid as usize] = nid;
                node_members.entry(nid).or_default().push(wid);
            }
        }
    }
    (wire_to_node, node_members, wire_to_tile)
}

impl<'a> DeviceQuery for XDevice<'a> {
    fn slicem_sites(&self) -> HashSet<String> {
        (0..self.device.tile_count()).filter_map(|ti| self.device.tile(ti)).flat_map(|t| t.sites.iter().filter(|s| s.site_type == "SLICEM").map(|s| s.name.clone())).collect()
    }

    /// CMT sites of the loaded part, from the database (the trait's default
    /// table names tiles no supported part has).
    fn cmt_sites(&self, kind: crate::pack::CmtKind) -> Vec<(String, String)> {
        let want = match kind {
            crate::pack::CmtKind::MmcmE2Adv => "MMCME2_ADV",
            crate::pack::CmtKind::PllE2Adv => "PLLE2_ADV",
        };
        let mut out = Vec::new();
        for ti in 0..self.device.tile_count() {
            if let Some(t) = self.device.tile(ti) {
                for s in &t.sites {
                    if s.site_type == want {
                        out.push((t.name.clone(), s.name.clone()));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)> {
        self.device
            .package_pin(pin)
            .map(|p| (p.tile.clone(), p.site.clone()))
    }

    fn bufg_site(&self) -> Option<(String, String)> {
        self.device.bufg_sites().into_iter().next()
    }

    fn bufg_sites(&self) -> Vec<(String, String)> {
        self.device.bufg_sites()
    }

    fn ramb_site(&self) -> Option<(String, String)> {
        self.device.ramb_sites().into_iter().next()
    }

    fn dsp_site(&self) -> Option<(String, String)> {
        self.device.dsp_sites().into_iter().next()
    }

    fn pin_bank(&self, pin: &str) -> Option<String> {
        self.device.package_pin(pin).map(|p| p.bank.clone())
    }

    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile(i))
            .map(|t| (t.grid_x, t.grid_y))
    }

    fn site_tile(&self, site: &str) -> Option<(String, String)> {
        let (ti, _) = self.device.site_by_name(site)?;
        Some((self.device.tile(ti)?.name.clone(), site.to_string()))
    }

    fn hard_pin_tile(&self, site: &str, pin: &str, output: bool) -> Option<String> {
        crate::ps7::int_wire(self.device, self, site, pin, output).map(|(t, _)| t)
    }

    fn slice_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)> {
        self.device
            .slices()
            .into_iter()
            .filter(|(_, site)| !used.contains(site))
            .filter_map(|(tile, site)| {
                self.tile_xy(&tile)
                    .map(|(tx, ty)| ((tx - x).abs() + (ty - y).abs(), tile, site))
            })
            .min_by_key(|(d, _, _)| *d)
            .map(|(_, tile, site)| (tile, site))
    }

    fn all_slices(&self) -> Vec<(String, String, i32, i32)> {
        self.device
            .slices()
            .into_iter()
            .filter_map(|(tile, site)| {
                let (x, y) = self.tile_xy(&tile)?;
                Some((tile, site, x, y))
            })
            .collect()
    }

    fn anchor_slice(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)> {
        // A caller-pinned chain-head site: validate the name parses, the
        // slice exists on the device, and it is still free.
        parse_slice_xy(site)?;
        if used.contains(site) {
            return None;
        }
        self.device
            .slices()
            .into_iter()
            .find(|(_, s)| s == site)
    }

    fn slice_above(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)> {
        // The cascade neighbor is the next slice up the column: X1 of the
        // same tile after X0, else X0 of the tile one name-Y higher:
        // X113Y39 is followed by X112Y40. Proven by the blinky golden
        // chain (X71Y39 to Y45, alternating with transparent X1 pads).
        let (x, y) = parse_slice_xy(site)?;
        let (tx, ty) = if x % 2 == 0 { (x + 1, y) } else { (x - 1, y + 1) };
        let want = format!("SLICE_X{tx}Y{ty}");
        if used.contains(&want) {
            return None;
        }
        self.device
            .slices()
            .into_iter()
            .find(|(_, s)| s == &want)
    }
}

impl<'a> RouteQuery for XDevice<'a> {
    fn note_claims(&self, net: &str, wires: &[(String, String)], claimed_len: usize) {
        let mut cache = self.claim_cache.borrow_mut();
        if cache.0 + wires.len() != claimed_len && cache.0 != usize::MAX {
            // Out of step: let the next search rebuild from the full map.
            cache.0 = usize::MAX;
            return;
        }
        if cache.0 == usize::MAX {
            return;
        }
        let mut own = self.owner.borrow_mut();
        let len = own.1.len() as u32;
        let oid = *own.1.entry(net.to_string()).or_insert(len);
        for (t, w) in wires {
            if let Some(id) = self.wire_id_of(t, w) {
                cache.1.insert(id, net.to_string());
                let (vec, _) = &mut *own;
                self.mark_owner(vec, id, oid);
            }
        }
        cache.0 = claimed_len;
    }

    fn node_key(&self, tile: &str, wire: &str) -> Option<u64> {
        let id = self.wire_id_of(tile, wire)?;
        let nid = self.wire_to_node[id as usize];
        // Wires with no node are their own conductor.
        Some(if nid == u32::MAX { (1u64 << 32) | id as u64 } else { nid as u64 })
    }

    fn pip_cost(&self, tile: &str, src: &str, dst: &str) -> Option<u32> {
        let s = self.wire_id_of(tile, src)?;
        let d = self.wire_id_of(tile, dst)?;
        let (ti, wi) = self.wire_to_tile[s as usize];
        let (_, dwi) = self.wire_to_tile[d as usize];
        // Same constant the search adds per pip (`astar`'s PIP_BASE).
        self.device.pips_from_idx(ti, wi).find(|p| p.dst == dwi).map(|p| p.delay_ps as u32 + 15)
    }

    fn contested(&self, net: &str, pips: &[RoutedPip]) -> Vec<String> {
        let own = self.owner.borrow();
        let my = own.1.get(net).copied().unwrap_or(u32::MAX);
        let mut hist = self.history.borrow_mut();
        if hist.len() != self.wire_to_tile.len() {
            hist.resize(self.wire_to_tile.len(), 0);
        }
        let mut oids: Vec<u32> = Vec::new();
        for p in pips {
            for w in [&p.src_wire, &p.dst_wire] {
                let Some(id) = self.wire_id_of(&p.tile, w) else { continue };
                let o = own.0.get(id as usize).copied().unwrap_or(0);
                if o != 0 && o != my {
                    hist[id as usize] = hist[id as usize].saturating_add(1);
                    if !oids.contains(&o) {
                        oids.push(o);
                    }
                }
            }
        }
        // Sorted: the rip-up queue follows this order, and hash order differs per run.
        let mut v: Vec<String> = own.1.iter().filter(|(_, v)| oids.contains(v)).map(|(k, _)| k.clone()).collect();
        v.sort();
        v
    }

    fn pf_present(&self, ps: Option<u32>) {
        let mut pf = self.pf.borrow_mut();
        match ps {
            None => *pf = None,
            Some(ps) => match pf.as_mut() {
                Some(st) => st.1 = ps,
                None => *pf = Some((vec![0; self.wire_to_tile.len()], ps)),
            },
        }
    }

    fn pf_occupy(&self, wires: &[(String, String)], delta: i32) {
        let mut pf = self.pf.borrow_mut();
        let Some((occ, _)) = pf.as_mut() else { return };
        // A net counts once per node, however many of its wires it uses.
        let mut keys: Vec<u64> = wires.iter().filter_map(|(t, w)| self.node_key(t, w)).collect();
        keys.sort_unstable();
        keys.dedup();
        for k in keys {
            let one = [k as u32];
            let members: &[u32] = if k >> 32 != 0 { &one } else { self.node_wires(k as u32) };
            for &m in members {
                let slot = &mut occ[m as usize];
                *slot = (*slot as i32 + delta).clamp(0, u16::MAX as i32) as u16;
            }
        }
    }

    fn pf_overused(&self, wires: &[(String, String)]) -> bool {
        let pf = self.pf.borrow();
        let Some((occ, _)) = pf.as_ref() else { return false };
        wires.iter().filter_map(|(t, w)| self.wire_id_of(t, w)).any(|id| occ[id as usize] > 1)
    }

    fn pf_history(&self) -> usize {
        let pf = self.pf.borrow();
        let Some((occ, _)) = pf.as_ref() else { return 0 };
        let mut hist = self.history.borrow_mut();
        if hist.len() != occ.len() {
            hist.resize(occ.len(), 0);
        }
        let mut over = 0;
        for (i, (h, &o)) in hist.iter_mut().zip(occ).enumerate() {
            if o > 1 {
                *h = h.saturating_add(1);
                over += 1;
                if debug_env("HELION_ROUTE_DEBUG") {
                    eprintln!("route_debug shared wire {:?} by {o} nets", self.wire_name_of(i as u32));
                }
            }
        }
        over
    }

    fn release_claims(&self, nets: &[String], claimed_len: usize) {
        let mut cache = self.claim_cache.borrow_mut();
        if cache.0 == usize::MAX {
            return;
        }
        let mut own = self.owner.borrow_mut();
        let oids: Vec<u32> = nets.iter().filter_map(|n| own.1.get(n).copied()).collect();
        // A net owns the wires it claimed and their node siblings (see
        // `mark_owner`): clear those, not the whole device.
        let mine: Vec<u32> = cache.1.iter().filter(|(_, n)| nets.contains(n)).map(|(&w, _)| w).collect();
        for w in mine {
            let nid = self.wire_to_node.get(w as usize).copied().unwrap_or(u32::MAX);
            let one = [w];
            let members: &[u32] = if nid == u32::MAX { &one } else { self.node_wires(nid) };
            for &m in members.iter().chain(one.iter()) {
                if let Some(slot) = own.0.get_mut(m as usize) {
                    if *slot != 0 && oids.contains(slot) {
                        *slot = 0;
                    }
                }
            }
        }
        cache.1.retain(|_, n| !nets.contains(n));
        cache.0 = claimed_len;
    }

    fn route_fast(
        &self,
        src: (&str, &str),
        tree: &[(String, String, u32)],
        sinks: &[(String, String)],
        opts: crate::route::RouteOpts,
        net: &str,
        claimed: &HashMap<(String, String), String>,
    ) -> Option<Option<(Vec<RoutedPip>, Vec<(u32, u32)>)>> {
        if std::env::var("HELION_ROUTE_BFS").is_ok() {
            return None;
        }
        self.astar(src, tree, sinks, opts, net, claimed)
    }

    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
        let idx = match self.tile_idx(tile) {
            Some(i) => i,
            None => return Vec::new(),
        };
        let wires = match self.device.wires(idx) {
            Some(w) => w,
            None => return Vec::new(),
        };
        self.device
            .pips_from(idx, wire)
            .iter()
            .filter_map(|p| {
                let dst = wires.get(p.dst as usize)?.clone();
                let src = wires.get(p.src as usize)?.clone();
                Some((dst, src, p.is_pseudo))
            })
            .collect()
    }

    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
        // All other (tile, wire) pairs in the same node.
        let wid = match self.wire_id_of(tile, wire) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let nid = match self.wire_to_node.get(wid as usize) {
            Some(&n) if n != u32::MAX => n,
            _ => return Vec::new(),
        };
        self.node_members
            .get(&nid)
            .map(|members| {
                members
                    .iter()
                    .filter(|&&m| m != wid)
                    .filter_map(|&m| self.wire_name_of(m))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)> {
        let (tile_idx, site_idx) = self.device.site_by_name(site)?;
        let tile_name = self.tile_name(tile_idx)?.to_string();
        let pins = self.device.site_pins(tile_idx, site_idx)?;
        pins.iter()
            .find(|(pn, _)| pn == pin)
            .map(|(_, w)| (tile_name.clone(), w.clone()))
    }

    fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)> {
        self.site_pin_wire(site, pin).into_iter().collect()
    }

    fn open_bypass_bounce(&self, closed_int_tiles: &[String]) {
        *self.byp_closed.borrow_mut() = Some(closed_int_tiles.iter().filter_map(|t| self.tile_idx(t)).collect());
    }

    fn is_clock_net(&self, net: &str) -> bool {
        self.clock_nets.contains(net)
    }

    fn bufg_tieoff_pips(&self, site: &str) -> Result<BufgTieoff, String> {
        // Control pin -> required constant for I0 -> O
        // (UG472 Fig. 2-5, Table 2-3).
        const TIE_HIGH: [&str; 3] = ["S0", "CE0", "IGNORE1"];
        const TIE_LOW: [&str; 3] = ["S1", "CE1", "IGNORE0"];

        let (tile_idx, site_idx) = self
            .device
            .site_by_name(site)
            .ok_or_else(|| format!("tieoff: unknown site `{site}`"))?;
        let tile_name = self
            .tile_name(tile_idx)
            .ok_or_else(|| format!("tieoff: no tile name for site `{site}`"))?
            .to_string();
        let tile_type = self
            .device
            .tile_type_name(tile_idx)
            .ok_or_else(|| format!("tieoff: no tile type for site `{site}`"))?
            .to_string();
        let pins = self
            .device
            .site_pins(tile_idx, site_idx)
            .ok_or_else(|| format!("tieoff: no pin data for site `{site}`"))?;
        let ppips = self
            .device
            .ppips(&tile_type)
            .ok_or_else(|| format!("tieoff: no ppips for tile type `{tile_type}`"))?;

        // Pin wire -> IMUX wire from the tile type's always-ppips, e.g.
        // CLK_BUFG_BOT_R.CLK_BUFG_R_BUFGCTRL0_S0.CLK_BUFG_IMUX4_0 always.
        // The CLK_BUFG tile has no TIEOFF wires; the IMUX is stitched via
        // tileconn (through the INT_INTERFACE tile) to an INT tile IMUX,
        // which the INT tile's TIEOFF site drives via VCC_WIRE/GND_WIRE.
        let imux_for_pin = |pin: &str| -> Result<String, String> {
            let pin_wire = pins
                .iter()
                .find(|(pn, _)| pn == pin)
                .map(|(_, w)| w.as_str())
                .ok_or_else(|| format!("tieoff: site `{site}` has no pin `{pin}`"))?;
            let prefix = format!("{tile_type}.{pin_wire}.");
            ppips
                .iter()
                .filter(|p| p.kind == PpipKind::Always)
                .find_map(|p| p.name.strip_prefix(&prefix).map(str::to_string))
                .ok_or_else(|| {
                    format!("tieoff: no always-ppip drives `{site}` pin `{pin}` ({pin_wire})")
                })
        };

        let mut tieoff = BufgTieoff::default();
        for pin in TIE_HIGH {
            let imux_wire = imux_for_pin(pin)?;
            let (int_tile, imux) =
                self.int_imux_for(&tile_name, &imux_wire, site, pin)?;
            // VCC_WIRE is the IMUX default select (default ppip: no config
            // bits). Verify the pip exists in the INT tile's graph, then
            // record it so the FASM documents the tie-high.
            self.require_pip(&int_tile, "VCC_WIRE", &imux, site, pin)?;
            tieoff.ones.push(RoutedPip {
                tile: int_tile,
                dst_wire: imux,
                src_wire: "VCC_WIRE".to_string(),
                is_pseudo: false,
            });
        }
        for pin in TIE_LOW {
            let imux_wire = imux_for_pin(pin)?;
            let (int_tile, imux) =
                self.int_imux_for(&tile_name, &imux_wire, site, pin)?;
            // GND_WIRE has no direct IMUX pip: GND_WIRE -> GFANx -> IMUXm.
            let gfan = self.gfan_for_imux(&int_tile, &imux, site, pin)?;
            tieoff.zeros.push(RoutedPip {
                tile: int_tile.clone(),
                dst_wire: gfan.clone(),
                src_wire: "GND_WIRE".to_string(),
                is_pseudo: false,
            });
            tieoff.zeros.push(RoutedPip {
                tile: int_tile,
                dst_wire: imux,
                src_wire: gfan,
                is_pseudo: false,
            });
        }
        Ok(tieoff)
    }

    fn bufg_spine(&self, site: &str) -> Result<BufgSpine, String> {
        let err = |msg: String| format!("spine: {msg} (site `{site}`)");
        let (tile_idx, site_idx) = self
            .device
            .site_by_name(site)
            .ok_or_else(|| err(format!("unknown site")))?;
        let tile_name = self
            .tile_name(tile_idx)
            .ok_or_else(|| err(format!("no tile name")))?
            .to_string();
        let pins = self
            .device
            .site_pins(tile_idx, site_idx)
            .ok_or_else(|| err(format!("no pin data")))?;
        let o_wire = pins
            .iter()
            .find(|(pn, _)| pn == "O")
            .map(|(_, w)| w.clone())
            .ok_or_else(|| err(format!("site has no pin `O`")))?;
        // Lowest global-clock index with a real (non-pseudo) O-exit pip.
        // A pseudo marking here contradicts Vivado, which sets the bit.
        let mut picked: Option<(u8, String)> = None;
        let mut saw_pseudo = false;
        for g in 0..32u8 {
            let dst = format!("CLK_BUFG_CK_GCLK{g}");
            for (d, s, pseudo) in RouteQuery::pips_from(self, &tile_name, &o_wire) {
                if d == dst && s == o_wire {
                    if pseudo {
                        saw_pseudo = true;
                    } else {
                        picked = Some((g, dst));
                    }
                    break;
                }
            }
            if picked.is_some() {
                break;
            }
        }
        let (gclk, o_dst) = picked.ok_or_else(|| {
            err(format!(
                "no O-exit pip (pseudo-marked: {saw_pseudo})"
            ))
        })?;
        // CLK_BUFG_REBUF tiles on the global line's node (tileconn).
        let o_wid = self
            .wire_id_of(&tile_name, &o_dst)
            .ok_or_else(|| err(format!("no wire `{o_dst}`")))?;
        let nid = self
            .wire_to_node
            .get(o_wid as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
            .ok_or_else(|| err(format!("`{o_dst}` is not stitched anywhere")))?;
        let members = self
            .node_members
            .get(&nid)
            .ok_or_else(|| err(format!("no node members")))?;
        // (tile, grid_y) of node-member REBUF tiles, plus every REBUF
        // tile below the entry in the same column (the DB node omits the
        // bottom-edge feed, but Vivado enables it: GCLK0_ENABLE_BELOW on
        // the lowest tile).
        let clk_xy = self
            .tile_idx(&tile_name)
            .and_then(|i| self.device.tile(i))
            .map(|t| (t.grid_x, t.grid_y));
        let mut node_rebuf: Vec<(String, i32)> = Vec::new();
        for &m in members {
            let Some((tname, _)) = self.wire_name_of(m) else {
                continue;
            };
            let Some(ti) = self.tile_idx(&tname) else {
                continue;
            };
            if self.device.tile_type_name(ti).unwrap_or("") != "CLK_BUFG_REBUF" {
                continue;
            }
            let y = self.device.tile(ti).map(|t| t.grid_y).unwrap_or(0);
            if !node_rebuf.iter().any(|(t, _)| t == &tname) {
                node_rebuf.push((tname, y));
            }
        }
        if node_rebuf.is_empty() {
            return Err(err(format!("no CLK_BUFG_REBUF tile on the GCLK{gclk} node")));
        }
        let bot = format!("CLK_BUFG_REBUF_R_CK_GCLK{gclk}_BOT");
        let top = format!("CLK_BUFG_REBUF_R_CK_GCLK{gclk}_TOP");
        // Entry tile: bottommost (largest grid_y; grid_y decreases
        // upward on the die) node REBUF tile holding the TOP to BOT
        // stitch. Vivado conducts the stitch downward (dst BOT, src
        // TOP): the clock enters the spine above the entry and flows
        // down through it toward the bottom edge.
        let mut stitched: Vec<(String, i32)> = Vec::new();
        for (tile, y) in &node_rebuf {
            let has_stitch = RouteQuery::pips_from(self, tile, &top)
                .iter()
                .any(|(d, s, pseudo)| d == &bot && s == &top && !pseudo);
            if has_stitch {
                stitched.push((tile.clone(), *y));
            }
        }
        stitched.sort_by_key(|(_, y)| std::cmp::Reverse(*y));
        let (entry_tile, entry_y) = stitched
            .first()
            .cloned()
            .ok_or_else(|| err(format!("no BOT to TOP stitch for GCLK{gclk}")))?;
        let mut spine = BufgSpine {
            gclk,
            pips: vec![
                RoutedPip {
                    tile: tile_name.clone(),
                    dst_wire: o_dst.clone(),
                    src_wire: o_wire,
                    is_pseudo: false,
                },
                RoutedPip {
                    tile: entry_tile.clone(),
                    dst_wire: bot.clone(),
                    src_wire: top.clone(),
                    is_pseudo: false,
                },
            ],
            statics: Vec::new(),
        };
        // grid_y decreases upward on the die: silicon-below means
        // greater grid_y. BELOW on the entry plus every REBUF tile below
        // it in the same column (down to the bottom edge); ABOVE on the
        // entry plus node tiles above it. Matches Vivado's observed
        // pattern: bottom edge fed, upward reach through the entry,
        // nothing above the loads.
        let mut below: Vec<String> = vec![entry_tile.clone()];
        if let Some((ex, _)) = clk_xy {
            for ti in 0..self.device.tile_count() {
                let is_rebuf =
                    self.device.tile_type_name(ti).unwrap_or("") == "CLK_BUFG_REBUF";
                let geom = self.device.tile(ti).map(|t| (t.grid_x, t.grid_y));
                if let (true, Some((gx, gy))) = (is_rebuf, geom) {
                    if gx == ex && gy > entry_y {
                        if let Some(name) = self.tile_name(ti) {
                            below.push(name.to_string());
                        }
                    }
                }
            }
        }
        below.sort();
        below.dedup();
        let mut above: Vec<String> = vec![entry_tile.clone()];
        for (tile, y) in &node_rebuf {
            if *y < entry_y {
                above.push(tile.clone());
            }
        }
        above.sort();
        above.dedup();
        for tile in &below {
            spine.statics.push((tile.clone(), format!("GCLK{gclk}_ENABLE_BELOW")));
        }
        for tile in &above {
            spine.statics.push((tile.clone(), format!("GCLK{gclk}_ENABLE_ABOVE")));
        }
        // HROW fanout: the global line feeds the horizontal clock row,
        // whose mux fans our clock out to the GCLK branches reaching
        // the INT tiles, through a BUFHCE. Vivado's observed shape for
        // a single bottom-region clock (ZB-10 blinky oracle): exactly
        // one HROW tile on the GCLK node, the MUX_OUT wire fed by
        // CK_IN_L0 (our clock enters the HROW on CK_IN_L0 through the
        // CASCO input path), three more CK_IN wires on the same mux,
        // eleven R_CK_GCLK branches, BUFHCE in use with uninverted CE.
        // Every constructed pip is verified against the DB below and
        // the build fails loudly on any absence: an unverified guess
        // here could short two clocks onto one branch.
        // HROW tile discovery (logic, no hardcoded tile): walk back
        // from the BUFG I0 pin through its input mux to the tileconn
        // node. The clock input corridor reaches the BUFG through
        // exactly one HROW tile (CK_IN_L0 to CASCO inside it, then a
        // bitless node up to the BUFG mux), and Vivado fans the clock
        // back out through that same tile.
        let i0_wire = pins
            .iter()
            .find(|(pn, _)| pn == "I0")
            .map(|(_, w)| w.clone())
            .ok_or_else(|| err(format!("site has no pin `I0`")))?;
        // The I0 driver, discovered by scanning the tile's wires for
        // the source of a real pip into I0 (no constructed names).
        let bti = self
            .tile_idx(&tile_name)
            .ok_or_else(|| err(format!("no tile index for {tile_name}")))?;
        let bwires = self
            .device
            .wires(bti)
            .ok_or_else(|| err(format!("no wires for {tile_name}")))?;
        let mut i0_srcs: Vec<String> = Vec::new();
        for w in bwires.iter() {
            for (d, s, pseudo) in RouteQuery::pips_from(self, &tile_name, w) {
                if d == i0_wire && s == *w && !pseudo && !i0_srcs.contains(w) {
                    i0_srcs.push(w.clone());
                }
            }
        }
        // The corridor driver: the candidate whose node reaches an
        // HROW tile (CK_IN to CASCO inside it). The other I0 mux
        // inputs (fabric IMUX, feedback FBG) never touch the HROW.
        let mut corridor: Vec<String> = Vec::new();
        for cand in &i0_srcs {
            let cwid_opt = self.wire_id_of(&tile_name, cand);
            let Some(cwid) = cwid_opt else { continue };
            let Some(&cnid) = self.wire_to_node.get(cwid as usize) else {
                continue;
            };
            if cnid == u32::MAX {
                continue;
            }
            let hits_hrow = self
                .node_members
                .get(&cnid)
                .map(|ms| {
                    ms.iter().any(|&m| {
                        self.wire_name_of(m).map(|(t, _)| self.tile_idx(&t)).flatten().map(
                            |ti| {
                                self.device
                                    .tile_type_name(ti)
                                    .unwrap_or("")
                                    .starts_with("CLK_HROW")
                            },
                        ).unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if hits_hrow {
                corridor.push(cand.clone());
            }
        }
        if corridor.len() != 1 {
            return Err(err(format!(
                "expected exactly one HROW-fed I0 driver, found {}: {}",
                corridor.len(),
                corridor.join(", ")
            )));
        }
        let src_wid = self
            .wire_id_of(&tile_name, &corridor[0])
            .ok_or_else(|| err(format!("no wire `{}`", corridor[0])))?;
        let src_nid = self
            .wire_to_node
            .get(src_wid as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
            .ok_or_else(|| err(format!("`{}` is not stitched anywhere", corridor[0])))?;
        let src_members = self
            .node_members
            .get(&src_nid)
            .ok_or_else(|| err(format!("no node members")))?;
        let mut hrow_tiles: Vec<String> = Vec::new();
        for &m in src_members {
            let Some((tname, _)) = self.wire_name_of(m) else {
                continue;
            };
            let Some(ti) = self.tile_idx(&tname) else {
                continue;
            };
            if self
                .device
                .tile_type_name(ti)
                .unwrap_or("")
                .starts_with("CLK_HROW")
                && !hrow_tiles.iter().any(|t| t == &tname)
            {
                hrow_tiles.push(tname);
            }
        }
        if hrow_tiles.len() != 1 {
            return Err(err(format!(
                "expected exactly one HROW tile feeding {}, found {}",
                corridor[0],
                hrow_tiles.len()
            )));
        }
        let hrow = hrow_tiles[0].clone();
        // HROW clock-mux config. The HROW mux is a wide selector (32
        // R_CK_GCLK spine taps plus CK_IN/CK_INT inputs) with exactly
        // one source picked: the tap carrying our global line,
        // R_CK_GCLK{gclk}. The CK_IN wires and sibling taps that
        // rawdiff attributes to the same field bits are overlap ghosts:
        // every one of their segbits entries carries a bit the oracle
        // lacks, while the tap's entry reproduces the oracle field
        // exactly (carry_vs_oracle_diff3 analysis). Emitting all of
        // them at once oversets the field and risks contention on
        // silicon, so exactly one source line goes out.
        // These are segbits-only features (the pip DB carries no
        // MUX_OUT or R_CK_GCLK pips: the mux fans out only to a pseudo
        // HCLK wire there), so the emitted line is verified against the
        // segbits DB instead, and the build fails loudly on anything
        // unresolvable. Channel R8 and BUFHCE_X1Y8 are Vivado-observed
        // data for a single bottom-region clock (ZB-10 blinky oracle):
        // the mux output must reach the HCLK row (checked below), and
        // multi-clock channel generality is unproven.
        let hti = self
            .tile_idx(&hrow)
            .ok_or_else(|| err(format!("no tile index for {hrow}")))?;
        let hrow_type = self
            .device
            .tile_type_name(hti)
            .ok_or_else(|| err(format!("no tile type for {hrow}")))?
            .to_string();
        let mux = "CLK_HROW_CK_MUX_OUT_R8".to_string();
        // The mux output must reach the HCLK row through its hard
        // (pseudo) connection, else the channel is dead.
        let hclk_out = "CLK_HROW_CK_HCLK_OUT_R8".to_string();
        let reaches_hclk = RouteQuery::pips_from(self, &hrow, &mux)
            .iter()
            .any(|(d, s, _)| d == &hclk_out && s == &mux);
        if !reaches_hclk {
            return Err(err(format!("{hrow}.{mux} does not reach {hclk_out}")));
        }
        // Every emitted HROW line must resolve in segbits (feature
        // keys carry the tile-type prefix, e.g.
        // `CLK_HROW_BOT_R.CLK_HROW_CK_MUX_OUT_R8.CLK_HROW_CK_IN_L0`).
        let mut pip_line = |dst: &str, src: &str| -> Result<(), String> {
            let feat = format!("{hrow_type}.{dst}.{src}");
            match self.device.segbits(&hrow_type, &feat) {
                Some(bits) if !bits.is_empty() => {
                    spine.pips.push(RoutedPip {
                        tile: hrow.clone(),
                        dst_wire: dst.to_string(),
                        src_wire: src.to_string(),
                        is_pseudo: false,
                    });
                    Ok(())
                }
                _ => Err(err(format!("no segbits for {hrow_type}.{feat}"))),
            }
        };
        // Exactly one source: the spine tap of our global line.
        pip_line(&mux, &format!("CLK_HROW_R_CK_GCLK{gclk}"))?;
        // BUFHCE site (logic: the BUFHCE whose I pin is our mux wire).
        let htile = self
            .device
            .tile(hti)
            .ok_or_else(|| err(format!("no tile data for {hrow}")))?;
        let mut bufh: Vec<String> = Vec::new();
        for (si, s) in htile.sites.iter().enumerate() {
            if !s.site_type.starts_with("BUFHCE") {
                continue;
            }
            let pins = self
                .device
                .site_pins(hti, si)
                .ok_or_else(|| err(format!("no pin data for site {}", s.name)))?;
            if pins.iter().any(|(pn, w)| pn == "I" && *w == mux) {
                bufh.push(s.name.clone());
            }
        }
        if bufh.len() != 1 {
            return Err(err(format!(
                "expected exactly one BUFHCE fed by {mux} in {hrow}, found {}",
                bufh.len()
            )));
        }
        spine.statics.push((hrow.clone(), "CLK_HROW_CK_IN_L0_ACTIVE".to_string()));
        spine.statics.push((hrow.clone(), "CLK_HROW_R_CK_GCLK0_ACTIVE".to_string()));
        spine.statics.push((hrow.clone(), format!("BUFHCE.{}.IN_USE", bufh[0])));
        spine.statics.push((hrow.clone(), format!("BUFHCE.{}.ZINV_CE", bufh[0])));
        // HCLK distribution: the HROW hands the clock to the HCLK
        // backbone row, which carries it across the die. The tileconn
        // DB has no HROW to HCLK stitch (both BUFHCE pins sit on
        // singleton nodes), so the tiles come from row geometry
        // instead: the same name-Y row as the HROW tile. The CMT tile
        // is unique per row; the leaf tile is the nearest HCLK_L tile
        // west of it (Vivado-observed for a single bottom-region clock,
        // ZB-10 blinky oracle). If a future design shows a different
        // leaf, this fails loudly at segbits verification.
        // Vivado-observed line set: leaf B_TOPL5 buffer BUFHCLK8 on
        // the HCLK_L tile with the buffer enabled, and BUFHCLK8 marked
        // used on the HCLK_CMT_L tile. Sibling leaf buffers are overlap
        // ghosts (each carries a bit the oracle lacks). Every line is
        // segbits-verified; anything unresolvable fails loudly. Only
        // the segbits-named features go out here: the row's UNNAMED
        // bits live in frames shared with the CLB/INT columns and are
        // disentangled at the per-cell comparison stage, not guessed.
        // The CMT input side (CCIO) is a separate stage.
        let row = hrow
            .rsplit('Y')
            .next()
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| err(format!("cannot parse row from {hrow}")))?;
        let row_suffix = format!("Y{row}");
        let mut cmt_tiles: Vec<(String, i32)> = Vec::new();
        let mut leaf_cands: Vec<(String, i32)> = Vec::new();
        for ti in 0..self.device.tile_count() {
            let ttype = self.device.tile_type_name(ti).unwrap_or("");
            if ttype != "HCLK_CMT_L" && ttype != "HCLK_L" {
                continue;
            }
            let Some(name) = self.tile_name(ti) else {
                continue;
            };
            if !name.ends_with(row_suffix.as_str()) {
                continue;
            }
            let gx = self.device.tile(ti).map(|t| t.grid_x).unwrap_or(-1);
            if ttype == "HCLK_CMT_L" {
                cmt_tiles.push((name.to_string(), gx));
            } else if !leaf_cands.iter().any(|(t, _)| t == name) {
                leaf_cands.push((name.to_string(), gx));
            }
        }
        if cmt_tiles.len() != 1 {
            return Err(err(format!(
                "expected exactly one HCLK_CMT_L tile in row {row}, found {}",
                cmt_tiles.len()
            )));
        }
        let (cmt, cmt_x) = cmt_tiles[0].clone();
        // Nearest HCLK_L tile west of the CMT tile.
        leaf_cands.retain(|(_, gx)| *gx < cmt_x);
        leaf_cands.sort_by_key(|(_, gx)| std::cmp::Reverse(*gx));
        let hclk = leaf_cands.first().map(|(t, _)| t.clone()).ok_or_else(|| {
            err(format!("no HCLK_L tile west of {cmt} in row {row}"))
        })?;
        let hclk_type = self
            .device
            .tile_type_name(self.tile_idx(&hclk).ok_or_else(|| err(format!("no tile index for {hclk}")))?)
            .ok_or_else(|| err(format!("no tile type for {hclk}")))?
            .to_string();
        let cmt_type = self
            .device
            .tile_type_name(self.tile_idx(&cmt).ok_or_else(|| err(format!("no tile index for {cmt}")))?)
            .ok_or_else(|| err(format!("no tile type for {cmt}")))?
            .to_string();
        // Exactly one leaf buffer: BUFHCLK8. The sibling buffers
        // that rawdiff attributes to the same field bits are overlap
        // ghosts again (same proof as the HROW tap: each carries a bit
        // the oracle lacks, while BUFHCLK8's entry reproduces the
        // oracle field exactly). Channel 8 end to end.
        let leaf = "HCLK_LEAF_CLK_B_TOPL5";
        for src in ["HCLK_CK_BUFHCLK8"] {
            let feat = format!("{hclk_type}.{leaf}.{src}");
            match self.device.segbits(&hclk_type, &feat) {
                Some(bits) if !bits.is_empty() => {
                    spine.pips.push(RoutedPip {
                        tile: hclk.clone(),
                        dst_wire: leaf.to_string(),
                        src_wire: src.to_string(),
                        is_pseudo: false,
                    });
                }
                _ => return Err(err(format!("no segbits for {feat}"))),
            }
        }
        let mut static_line = |tile: &str, ttype: &str, feat: &str| -> Result<(), String> {
            let full = format!("{ttype}.{feat}");
            match self.device.segbits(ttype, &full) {
                Some(bits) if !bits.is_empty() => {
                    spine.statics.push((tile.to_string(), feat.to_string()));
                    Ok(())
                }
                _ => Err(err(format!("no segbits for {full}"))),
            }
        };
        static_line(&hclk, &hclk_type, "ENABLE_BUFFER.HCLK_CK_BUFHCLK8")?;
        static_line(&cmt, &cmt_type, "HCLK_CMT_CK_BUFHCLK8_USED")?;
        // CMT input side: the clock input corridor crosses the
        // HCLK_CMT tile through MUX_CLK_0 (routed as a normal pip on
        // the input net), and the CCIO input is marked active and
        // used. One HCLK_CMT tile per clock row: take the one in the
        // HROW tile's row, and fail loudly on anything else.
        let mut cmt_in: Vec<String> = Vec::new();
        for ti in 0..self.device.tile_count() {
            if self.device.tile_type_name(ti).unwrap_or("") != "HCLK_CMT" {
                continue;
            }
            let Some(name) = self.tile_name(ti) else {
                continue;
            };
            if !name.ends_with(row_suffix.as_str()) {
                continue;
            }
            if !cmt_in.contains(&name.to_string()) {
                cmt_in.push(name.to_string());
            }
        }
        if cmt_in.len() != 1 {
            return Err(err(format!(
                "expected exactly one HCLK_CMT tile in row {row}, found {}: {}",
                cmt_in.len(),
                cmt_in.join(", ")
            )));
        }
        let cmt_in_type = self
            .device
            .tile_type_name(
                self.tile_idx(&cmt_in[0])
                    .ok_or_else(|| err(format!("no tile index for {}", cmt_in[0])))?,
            )
            .ok_or_else(|| err(format!("no tile type for {}", cmt_in[0])))?
            .to_string();
        static_line(&cmt_in[0], &cmt_in_type, "HCLK_CMT_CCIO0_ACTIVE")?;
        static_line(&cmt_in[0], &cmt_in_type, "HCLK_CMT_CCIO0_USED")?;
        Ok(spine)
    }
}

impl<'a> EmitDevice for XDevice<'a> {
    fn tile_type(&self, tile: &str) -> Option<String> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile_type_name(i))
            .map(|s| s.to_string())
    }

    fn bufg_spine_static(&self, _tile: &str, site: &str) -> Vec<(String, String)> {
        // route() runs bufg_spine first and fails loudly there; this
        // identical call then succeeds. Empty on error by construction.
        RouteQuery::bufg_spine(self, site)
            .map(|s| s.statics)
            .unwrap_or_default()
    }

    fn required_features(&self) -> Vec<String> {
        self.device.required_features().to_vec()
    }

    fn clock_statics(&self, pips: &[&RoutedPip]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let has = |ttype: &str, feat: &str| self.device.segbits(ttype, &format!("{ttype}.{feat}")).is_some();
        let num_after = |w: &str, prefix: &str| -> Option<String> {
            let r = w.strip_prefix(prefix)?;
            let d: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
            (!d.is_empty()).then_some(d)
        };
        let mut node_done: HashSet<u32> = HashSet::new();
        for p in pips {
            let Some(ti) = self.tile_idx(&p.tile) else { continue };
            let ttype = self.device.tile_type_name(ti).unwrap_or("").to_string();
            if ttype.starts_with("CLK_HROW") {
                if p.src_wire.starts_with("CLK_HROW_R_CK_GCLK") || p.src_wire.starts_with("CLK_HROW_CK_IN_") {
                    let f = format!("{}_ACTIVE", p.src_wire);
                    if has(&ttype, &f) {
                        out.push(format!("{}.{f}", p.tile));
                    }
                }
                if p.dst_wire.starts_with("CLK_HROW_CK_MUX_OUT_") {
                    // The BUFHCE whose I pin is this mux output.
                    if let Some(t) = self.device.tile(ti) {
                        for (si, site) in t.sites.iter().enumerate() {
                            if !site.site_type.starts_with("BUFHCE") {
                                continue;
                            }
                            let fed = self.device.site_pins(ti, si).map(|ps| ps.iter().any(|(pn, w)| pn == "I" && *w == p.dst_wire)).unwrap_or(false);
                            if fed {
                                // Features name the site relative to its
                                // tile: 12 BUFHCEs per side per clock row.
                                let rel = match site.name.strip_prefix("BUFHCE_X").and_then(|r| r.split_once('Y')) {
                                    Some((x, y)) => format!("BUFHCE_X{}Y{}", x, y.parse::<u32>().unwrap_or(0) % 12),
                                    None => site.name.clone(),
                                };
                                out.push(format!("{}.BUFHCE.{rel}.IN_USE", p.tile));
                                out.push(format!("{}.BUFHCE.{rel}.ZINV_CE", p.tile));
                            }
                        }
                    }
                }
            }
            // The row clock's buffer sits in the tile it enters. A leaf in the
            // neighbouring HCLK tile takes it through `HCLK_CK_INOUT_*`, and the
            // buffer still has to be on (G69: without it every INT_L column's
            // flops and block RAMs had no clock on silicon).
            if (ttype == "HCLK_L" || ttype == "HCLK_R") && (p.dst_wire.starts_with("HCLK_LEAF_CLK_B_") || p.dst_wire.starts_with("HCLK_CK_INOUT_")) {
                let f = format!("ENABLE_BUFFER.{}", p.src_wire);
                if has(&ttype, &f) {
                    out.push(format!("{}.{f}", p.tile));
                }
            }
            if ttype == "HCLK_CMT" || ttype == "HCLK_CMT_L" {
                if let Some(n) = num_after(&p.src_wire, "HCLK_CMT_CCIO") {
                    for suffix in ["ACTIVE", "USED"] {
                        let f = format!("HCLK_CMT_CCIO{n}_{suffix}");
                        if has(&ttype, &f) {
                            out.push(format!("{}.{f}", p.tile));
                        }
                    }
                }
            }
            // Node-level markers: rebuffers at the ends of a global-line
            // segment, CMT tiles a row clock passes.
            for w in [&p.dst_wire, &p.src_wire] {
                let Some(wid) = self.wire_id_of(&p.tile, w) else { continue };
                let nid = self.wire_to_node.get(wid as usize).copied().unwrap_or(u32::MAX);
                if nid == u32::MAX || !node_done.insert(nid) {
                    continue;
                }
                let Some(members) = self.node_members.get(&nid) else { continue };
                for &m in members {
                    let Some((mt, mw)) = self.wire_name_of(m) else { continue };
                    let Some(mti) = self.tile_idx(&mt) else { continue };
                    let mtype = self.device.tile_type_name(mti).unwrap_or("");
                    if mtype == "CLK_BUFG_REBUF" {
                        if let Some(g) = num_after(&mw, "CLK_BUFG_REBUF_R_CK_GCLK") {
                            // A segment's top-end rebuffer holds its _BOT
                            // wire, the bottom-end one its _TOP wire.
                            let f = if mw.ends_with("_BOT") { format!("GCLK{g}_ENABLE_ABOVE") } else { format!("GCLK{g}_ENABLE_BELOW") };
                            if has(mtype, &f) {
                                out.push(format!("{mt}.{f}"));
                            }
                        }
                    }
                    if mtype == "HCLK_CMT_L" || mtype == "HCLK_CMT" {
                        if let Some(k) = num_after(&mw, "HCLK_CMT_CK_BUFHCLK") {
                            let f = format!("HCLK_CMT_CK_BUFHCLK{k}_USED");
                            if has(mtype, &f) {
                                out.push(format!("{mt}.{f}"));
                            }
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn pudc_b(&self, low_v_tiles: &[String], used: &[(String, String)]) -> Vec<String> {
        // Vivado (the seven reference Vivado builds using bank 34): when the bank holding
        // the PUDC_B pin runs a <=1.8 V standard and the design leaves the pin
        // unused, the pin is a pulled-up input of that bank's standard.
        let pins: Vec<&crate::db::parse::PackagePin> = self.device.package_pins().collect();
        let Some(p) = pins.iter().find(|p| p.pin_function.contains("PUDC_B")) else { return Vec::new() };
        let bank_low_v = low_v_tiles.iter().any(|t| pins.iter().any(|q| &q.tile == t && q.bank == p.bank));
        if !bank_low_v || used.iter().any(|(t, s)| t == &p.tile && s == &p.site) {
            return Vec::new();
        }
        let Some(rel) = crate::emit::EmitDevice::site_rel(self, &p.tile, &p.site) else { return Vec::new() };
        vec![
            format!("{}.{rel}.LVCMOS12_LVCMOS15_LVCMOS18.IN", p.tile),
            format!("{}.{rel}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVDS_25_LVTTL_SSTL135_SSTL15_TMDS_33.IN_ONLY", p.tile),
            format!("{}.{rel}.PULLTYPE.PULLUP", p.tile),
        ]
    }

    fn bank_stepdown(&self, iob_tiles: &[String]) -> Vec<String> {
        // Vivado: a bank running a <=1.8V standard gets STEPDOWN on every
        // IOB site of the bank and on the bank's HCLK_IOI3 tile.
        let pins: Vec<&crate::db::parse::PackagePin> = self.device.package_pins().collect();
        let mut banks: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for t in iob_tiles {
            if let Some(p) = pins.iter().find(|p| &p.tile == t) {
                banks.insert(p.bank.clone());
            }
        }
        let mut out = Vec::new();
        for bank in banks {
            let mut ys: Vec<i32> = Vec::new();
            let mut xs: Vec<i32> = Vec::new();
            for p in pins.iter().filter(|p| p.bank == bank) {
                if let Some(rel) = crate::emit::EmitDevice::site_rel(self, &p.tile, &p.site) {
                    if rel.starts_with("IOB_") {
                        out.push(format!("{}.{rel}.LVCMOS12_LVCMOS15_LVCMOS18_SSTL135_SSTL15.STEPDOWN", p.tile));
                    }
                }
                if let Some(t) = self.tile_idx(&p.tile).and_then(|i| self.device.tile(i)) {
                    ys.push(t.grid_y);
                    xs.push(t.grid_x);
                }
            }
            let (Some(&y0), Some(&y1)) = (ys.iter().min(), ys.iter().max()) else { continue };
            let x = xs.first().copied().unwrap_or(0);
            let mut best: Option<(i32, String)> = None;
            for ti in 0..self.device.tile_count() {
                if self.device.tile_type_name(ti) != Some("HCLK_IOI3") {
                    continue;
                }
                let Some(t) = self.device.tile(ti) else { continue };
                if t.grid_y < y0 || t.grid_y > y1 {
                    continue;
                }
                let dx = (t.grid_x - x).abs();
                if best.as_ref().map(|(b, _)| dx < *b).unwrap_or(true) {
                    best = Some((dx, t.name.clone()));
                }
            }
            if let Some((_, h)) = best {
                out.push(format!("{h}.STEPDOWN"));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn ioi_tile(&self, iob_tile: &str) -> Option<String> {
        // Same bank side (L/R), same row (grid_y), nearest column: the
        // IOI tile sits one over from its IOB tile (e.g. RIOB33_X73Y45
        // with RIOI3_X73Y45). SING pairs are a known DB gap (ZB-09):
        // prjxray has no SING logic segbits, so no OLOGIC/ILOGIC lines
        // there (the IOB-tile lines still flow to the SING filter).
        let side = iob_tile.chars().next()?;
        let want = format!("{side}IOI3");
        // A SING IOB pairs with the SING IOI tile of its row (its logic
        // features encode through the tilegrid alias to the full IOI type).
        let sing = iob_tile.contains("_SING_");
        let anchor = self.tile_idx(iob_tile)?;
        let anchor_tile = self.device.tile(anchor)?;
        let (ax, ay) = (anchor_tile.grid_x, anchor_tile.grid_y);
        let mut best: Option<(i32, String)> = None;
        for ti in 0..self.device.tile_count() {
            let ttype = self.device.tile_type_name(ti).unwrap_or("");
            if !ttype.starts_with(&want) {
                continue;
            }
            if ttype.contains("SING") != sing {
                continue;
            }
            let t = self.device.tile(ti)?;
            if t.grid_y != ay {
                continue;
            }
            let dx = (t.grid_x - ax).abs();
            let Some(name) = self.tile_name(ti) else {
                continue;
            };
            let entry = (dx, name.to_string());
            if best.as_ref().map(|(bdx, _)| dx < *bdx).unwrap_or(true) {
                best = Some(entry);
            }
        }
        best.map(|(_, name)| name)
    }

    fn site_rel(&self, tile: &str, site: &str) -> Option<String> {
        let idx = self.tile_idx(tile)?;
        let t = self.device.tile(idx)?;
        let pos = t.sites.iter().position(|s| s.name == site)?;
        let stype = &t.sites[pos].site_type;
        if stype.starts_with("IOB") {
            // segbits: IOB_Y0 = master BEL, IOB_Y1 = slave BEL. This is
            // the site TYPE, not the site position in the tile: T22 lives
            // on IOB_X1Y46 (second site in its tile) but Vivado emits
            // IOB_Y0 because the site is a master (ZB-10 ground truth).
            // Bare "IOB33" (SING tiles) has no master/slave marking: fall
            // back to site position, exactly the old behavior. That BEL
            // is UNPROVEN (ZB-09 owns SING truth); M/S sites are exact.
            match stype.as_str() {
                "IOB33M" => Some("IOB_Y0".to_string()),
                "IOB33S" => Some("IOB_Y1".to_string()),
                _ => Some(format!("IOB_Y{pos}")),
            }
        } else if stype.starts_with("SLICE") {
            // segbits: SLICEL_X0 / SLICEM_X1 (X index within tile).
            Some(format!("{stype}_X{pos}"))
        } else if stype.starts_with("BUFG") {
            // segbits: BUFGCTRL.BUFGCTRL_X0Y0 (full site name).
            Some(site.to_string())
        } else {
            Some(format!("{stype}_X{pos}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::default_db_root;
    use std::sync::OnceLock;

    fn test_z020() -> &'static (Device, NodeMaps) {
        static CELL: OnceLock<(Device, NodeMaps)> = OnceLock::new();
        CELL.get_or_init(|| {
            let mut dev = Device::load_xc7z020(&default_db_root()).expect("load xc7z020");
            let maps = build_node_map(&mut dev);
            (dev, maps)
        })
    }

    /// G77: PathFinder's searches estimate by the lookahead table. On the
    /// empty device (same costs as the flat-estimate search) they find
    /// routes as fast (in delay) while expanding a fraction of the wires.
    #[test]
    fn lookahead_finds_as_fast_a_route_with_fewer_expansions() {
        let (dev, maps) = test_z020();
        let x = XDevice::new(dev, maps.clone());
        let claimed = HashMap::new();
        let opts = crate::route::RouteOpts { bbox: 8, ..Default::default() };
        let (mut flat_e, mut la_e) = (0, 0);
        for (from, to) in [("SLICE_X40Y40", "SLICE_X41Y41"), ("SLICE_X40Y40", "SLICE_X45Y46"), ("SLICE_X60Y40", "SLICE_X56Y30"), ("SLICE_X40Y40", "SLICE_X74Y40")] {
            let src = x.site_pin_wire(from, "AQ").expect("src");
            let dst = x.site_pin_wire(to, "A1").expect("dst");
            let mut run = |pf: bool| {
                x.pf_present(pf.then_some(200));
                let r = x.astar((&src.0, &src.1), &[], &[dst.clone()], opts, "probe", &claimed);
                let (_, arr) = r.flatten().expect("routes");
                (arr.last().map(|a| a.1).unwrap_or(0), LAST_EXPANDED.with(|e| e.get()))
            };
            let ((fd, fe), (ld, le)) = (run(false), run(true));
            assert!(ld * 100 <= fd * 103, "{from} -> {to}: {ld} ps with the lookahead, {fd} ps flat");
            (flat_e, la_e) = (flat_e + fe, la_e + le);
        }
        x.pf_present(None);
        assert!(la_e * 2 < flat_e, "expanded {la_e} with the lookahead, {flat_e} flat");
    }

    /// G95: a long hop rides long lines. Past the lookahead's reach the
    /// estimate charged 30 ps per tile, two to three times a long line's
    /// rate, and both searches settled on hex chains: 2988-3017 ps for the
    /// bram_fill hop and 2160-2576 ps for the dsp_fill_prims one, where
    /// the cheapest paths (Dijkstra) are 1986 and 1962 ps.
    #[test]
    fn long_hops_take_long_lines() {
        let (dev, maps) = test_z020();
        let x = XDevice::new(dev, maps.clone());
        let claimed = HashMap::new();
        for (from, fp, to, tp) in [("SLICE_X50Y53", "D", "SLICE_X108Y25", "BX"), ("SLICE_X94Y129", "AQ", "SLICE_X95Y61", "D1")] {
            let src = x.site_pin_wire(from, fp).expect("src");
            let dst = x.site_pin_wire(to, tp).expect("dst");
            for pf in [false, true] {
                x.pf_present(pf.then_some(200));
                let opts = crate::route::RouteOpts { allow_byp: tp.ends_with('X'), crit: 255, ..Default::default() };
                let (_, arr) = x.astar((&src.0, &src.1), &[], &[dst.clone()], opts, "probe", &claimed).flatten().expect("routes");
                let d = arr.last().map(|a| a.1).unwrap_or(0);
                assert!(d <= 2100, "{from} -> {to} (lookahead {pf}): {d} ps");
            }
        }
        x.pf_present(None);
    }

    /// G76 probe: the fastest route the search finds for one long hop on an
    /// empty device (no other nets), to compare with a routed design.
    /// `cargo test --release -p helion-xilinx7 --lib long_hop -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn long_hop_delay_on_an_empty_device() {
        let (dev, maps) = test_z020();
        let x = XDevice::new(dev, maps.clone());
        let claimed = HashMap::new();
        for (from, to) in [("SLICE_X93Y10", "SLICE_X36Y84"), ("SLICE_X60Y40", "SLICE_X60Y74"), ("SLICE_X40Y40", "SLICE_X74Y40"), ("SLICE_X40Y40", "SLICE_X41Y41")] {
            let src = x.site_pin_wire(from, "AQ").expect("src");
            let dst = x.site_pin_wire(to, "A1").expect("dst");
            for crit in [0u8, 255] {
                let opts = crate::route::RouteOpts { crit, ..Default::default() };
                let r = x.astar((&src.0, &src.1), &[], &[dst.clone()], opts, "probe", &claimed);
                let (pips, arr) = r.flatten().expect("routes");
                let wires: Vec<String> = pips.iter().filter(|p| !p.dst_wire.starts_with("IMUX") && !p.dst_wire.contains("LOGIC_OUTS")).map(|p| p.dst_wire.clone()).collect();
                eprintln!("{from} -> {to} crit {crit}: {} ps, {} pips: {}", arr.last().map(|a| a.1).unwrap_or(0), pips.len(), wires.join(" "));
            }
        }
    }

    #[test]
    fn bufg_tieoff_paths() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let tieoff = xdev
            .bufg_tieoff_pips("BUFGCTRL_X0Y0")
            .expect("tie-off for BUFGCTRL_X0Y0");
        // All tie-offs land in the INT tile stitched to the CLK_BUFG tile.
        assert_eq!(tieoff.ones.len(), 3, "S0, CE0, IGNORE1");
        assert_eq!(tieoff.zeros.len(), 6, "S1, CE1, IGNORE0 x2 pips each");
        for p in tieoff.ones.iter().chain(tieoff.zeros.iter()) {
            assert!(
                p.tile.starts_with("INT_R_") || p.tile.starts_with("INT_L_"),
                "tie-off pip in INT tile, got {}",
                p.tile
            );
            assert!(!p.is_pseudo);
        }
        // S0=1, CE0=1, IGNORE1=1 via VCC_WIRE -> IMUX.
        let mut one_dsts: Vec<&str> =
            tieoff.ones.iter().map(|p| p.dst_wire.as_str()).collect();
        one_dsts.sort();
        assert_eq!(one_dsts, ["IMUX20", "IMUX4", "IMUX8"]);
        assert!(tieoff.ones.iter().all(|p| p.src_wire == "VCC_WIRE"));
        // S1=0, CE1=0, IGNORE0=0 via GND_WIRE -> GFAN -> IMUX.
        let mut zero_dsts: Vec<&str> =
            tieoff.zeros.iter().map(|p| p.dst_wire.as_str()).collect();
        zero_dsts.sort();
        assert_eq!(
            zero_dsts,
            ["GFAN0", "GFAN0", "GFAN1", "IMUX0", "IMUX12", "IMUX16"],
            "unexpected zero pips"
        );
        // The GFAN -> IMUX pairings: GFAN0 drives IMUX0 (S1) and IMUX16
        // (CE1); GFAN1 drives IMUX12 (IGNORE0).
        for p in &tieoff.zeros {
            if p.src_wire == "GND_WIRE" {
                assert!(p.dst_wire == "GFAN0" || p.dst_wire == "GFAN1");
            } else {
                match p.dst_wire.as_str() {
                    "IMUX0" | "IMUX16" => assert_eq!(p.src_wire, "GFAN0"),
                    "IMUX12" => assert_eq!(p.src_wire, "GFAN1"),
                    other => panic!("unexpected zero dst {other}"),
                }
            }
        }
        // Unknown site fails loudly.
        assert!(xdev.bufg_tieoff_pips("BUFGCTRL_X99Y99").is_err());
    }

    #[test]
    fn bufg_spine_paths() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado drives global line 0 from this site.
        assert_eq!(spine.gclk, 0);
        // O exit plus the entry REBUF stitch, conducted downward
        // (dst BOT, src TOP) exactly as Vivado emits it.
        assert!(
            spine.pips.iter().any(|p| p.dst_wire == "CLK_BUFG_CK_GCLK0"
                && p.src_wire == "CLK_BUFG_BUFGCTRL0_O"
                && !p.is_pseudo),
            "missing O exit, got {:?}",
            spine.pips
        );
        assert!(
            spine.pips.iter().any(|p| p.dst_wire
                == "CLK_BUFG_REBUF_R_CK_GCLK0_BOT"
                && p.src_wire == "CLK_BUFG_REBUF_R_CK_GCLK0_TOP"
                && !p.is_pseudo),
            "missing TOP to BOT stitch, got {:?}",
            spine.pips
        );
        // Vivado's observed pattern for bottom-region loads: BELOW on
        // the entry plus tiles below it in the column, ABOVE on the
        // entry plus node tiles above it.
        let mut statics = spine.statics.clone();
        statics.sort();
        let tiles: Vec<(&str, &str)> = statics
            .iter()
            .map(|(t, f)| (t.as_str(), f.as_str()))
            .collect();
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y13", "GCLK0_ENABLE_BELOW")),
            "missing bottom-edge feed, got {statics:?}"
        );
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y38", "GCLK0_ENABLE_BELOW")),
            "missing entry BELOW, got {statics:?}"
        );
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y38", "GCLK0_ENABLE_ABOVE")),
            "missing entry ABOVE, got {statics:?}"
        );
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y65", "GCLK0_ENABLE_ABOVE")),
            "missing upward reach, got {statics:?}"
        );
        assert_eq!(statics.len(), 12, "extra enables, got {statics:?}");
        // Unknown site fails loudly.
        assert!(xdev.bufg_spine("BUFGCTRL_X99Y99").is_err());
    }

    #[test]
    fn bufg_spine_hrow_fanout() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado ground truth (ZB-10 same-RTL oracle): the clock
        // passes through CLK_HROW_BOT_R_X87Y26, mux MUX_OUT_R8
        // selecting exactly one source, the R_CK_GCLK0 spine tap
        // carrying GCLK0 (the sibling taps and CK_IN wires that
        // rawdiff attributes to the same field are overlap ghosts:
        // each carries a bit the oracle lacks), with BUFHCE_X1Y8 in
        // use (CE not inverted).
        let hrow = "CLK_HROW_BOT_R_X87Y26";
        let mux = "CLK_HROW_CK_MUX_OUT_R8";
        let hrow_pips: Vec<_> = spine
            .pips
            .iter()
            .filter(|p| p.tile == hrow)
            .collect();
        assert_eq!(
            hrow_pips.len(),
            1,
            "exactly one HROW source line, got {:?}",
            spine.pips
        );
        // The tap feature names the mux as DST (Vivado's convention:
        // `MUX_OUT_R8.R_CK_GCLK0`), exactly as the oracle records it.
        assert!(
            hrow_pips[0].dst_wire == mux
                && hrow_pips[0].src_wire == "CLK_HROW_R_CK_GCLK0"
                && !hrow_pips[0].is_pseudo,
            "wrong HROW source, got {:?}",
            spine.pips
        );
        // HCLK row (next stage below); the HROW statics stay exact.
        let mut statics = spine.statics.clone();
        statics.sort();
        for feat in [
            "CLK_HROW_CK_IN_L0_ACTIVE",
            "CLK_HROW_R_CK_GCLK0_ACTIVE",
            "BUFHCE.BUFHCE_X1Y8.IN_USE",
            "BUFHCE.BUFHCE_X1Y8.ZINV_CE",
        ] {
            assert!(
                statics
                    .iter()
                    .any(|(t, f)| t == hrow && f == feat),
                "missing static {feat}, got {statics:?}"
            );
        }
    }

    #[test]
    fn bufg_spine_hclk_leaf() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado ground truth (ZB-10 same-RTL oracle): exactly one
        // leaf buffer, B_TOPL5 BUFHCLK8, on HCLK_L_X174Y26, the buffer
        // enabled, and BUFHCLK8 marked used on HCLK_CMT_L_X178Y26.
        // Sibling buffers are overlap ghosts (each carries a bit the
        // oracle lacks); BUFHCLK8's entry reproduces the oracle leaf
        // field exactly.
        let hclk = "HCLK_L_X174Y26";
        let leaf = "HCLK_LEAF_CLK_B_TOPL5";
        let hclk_pips: Vec<_> = spine
            .pips
            .iter()
            .filter(|p| p.tile == hclk)
            .collect();
        assert_eq!(
            hclk_pips.len(),
            1,
            "exactly one HCLK leaf line, got {:?}",
            spine.pips
        );
        assert!(
            hclk_pips[0].dst_wire == leaf
                && hclk_pips[0].src_wire == "HCLK_CK_BUFHCLK8"
                && !hclk_pips[0].is_pseudo,
            "wrong HCLK leaf line, got {:?}",
            spine.pips
        );
        let mut statics = spine.statics.clone();
        statics.sort();
        assert!(
            statics.iter().any(|(t, f)| t == hclk
                && f == "ENABLE_BUFFER.HCLK_CK_BUFHCLK8"),
            "missing ENABLE_BUFFER, got {statics:?}"
        );
        assert!(
            statics.iter().any(|(t, f)| t == "HCLK_CMT_L_X178Y26"
                && f == "HCLK_CMT_CK_BUFHCLK8_USED"),
            "missing CMT USED, got {statics:?}"
        );
    }

    /// G69: a row clock taken into the neighbouring HCLK tile still needs
    /// its buffer enabled in the tile it enters.
    #[test]
    fn row_clock_buffer_is_enabled_for_a_neighbour_tile_leaf() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let pip = |tile: &str, dst: &str, src: &str| RoutedPip { tile: tile.into(), src_wire: src.into(), dst_wire: dst.into(), is_pseudo: false };
        let pips = [pip("HCLK_R_X22Y26", "HCLK_CK_INOUT_R0", "HCLK_CK_BUFHCLK0"), pip("HCLK_L_X21Y26", "HCLK_LEAF_CLK_B_BOTL0", "HCLK_CK_OUTIN_L0")];
        let statics = xdev.clock_statics(&pips.iter().collect::<Vec<_>>());
        assert!(statics.iter().any(|f| f == "HCLK_R_X22Y26.ENABLE_BUFFER.HCLK_CK_BUFHCLK0"), "got {statics:?}");
    }

    #[test]
    fn bufg_spine_cmt_ccio() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado ground truth (ZB-10 same-RTL oracle): the clock input
        // corridor crosses the single HCLK_CMT tile through MUX_CLK_0
        // (already routed), and the CCIO0 input is marked active and
        // used. Both features are single oracle-1 bits, so no ghost
        // check applies.
        let mut statics = spine.statics.clone();
        statics.sort();
        for feat in ["HCLK_CMT_CCIO0_ACTIVE", "HCLK_CMT_CCIO0_USED"] {
            assert!(
                statics
                    .iter()
                    .any(|(t, f)| t == "HCLK_CMT_X8Y26" && f == feat),
                "missing static {feat}, got {statics:?}"
            );
        }
    }

    #[test]
    fn iob_site_rel_master_slave() {
        // Vivado ground truth (ZB-10 same-RTL diff): single-ended pins
        // use the master BEL (IOB_Y0); the FASM BEL comes from the site
        // type, not the site position in the tile.
        use crate::emit::EmitDevice;
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        // T22 (LD0 output): master site -> IOB_Y0.
        assert_eq!(
            xdev.site_rel("RIOB33_X73Y45", "IOB_X1Y46").as_deref(),
            Some("IOB_Y0")
        );
        // Neighbor slave site -> IOB_Y1.
        assert_eq!(
            xdev.site_rel("RIOB33_X73Y45", "IOB_X1Y45").as_deref(),
            Some("IOB_Y1")
        );
        // Y9 (clk input): master site -> IOB_Y0 (Vivado ILOGIC_Y0).
        assert_eq!(
            xdev.site_rel("LIOB33_X0Y25", "IOB_X0Y26").as_deref(),
            Some("IOB_Y0")
        );
    }

    #[test]
    fn ioi_tile_pairing() {
        use crate::emit::EmitDevice;
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        // Paired IOI tile, same row, one column over.
        assert_eq!(
            xdev.ioi_tile("RIOB33_X73Y45").as_deref(),
            Some("RIOI3_X73Y45")
        );
        assert_eq!(
            xdev.ioi_tile("LIOB33_X0Y25").as_deref(),
            Some("LIOI3_X0Y25")
        );
        // SING IOBs pair with the SING IOI tile of the same row.
        assert_eq!(xdev.ioi_tile("RIOB33_SING_X73Y49").as_deref(), Some("RIOI3_SING_X73Y49"));
    }

    /// G77: the exact search (PathFinder's) is not capped below the device.
    /// With the load walled off by a clock net it expands every wire it can
    /// reach before it gives up: 6.86M on the xc7a100t, past the 6M the
    /// negotiating search stops at (the xc7z020 reaches 5.77M). A cap below
    /// that would fail PathFinder nets that do have a path.
    #[test]
    fn exact_search_can_expand_the_whole_device() {
        let part = crate::parts::resolve(&default_db_root(), "xc7a100tcsg324-1").expect("part");
        let mut dev = Device::load_part(&part).expect("load");
        let maps = build_node_map(&mut dev);
        let dev = &dev;
        let mut x = XDevice::new(dev, maps);
        x.clock_nets.insert("clk".into());
        let src = x.site_pin_wire("SLICE_X40Y40", "AQ").expect("src");
        let dst = x.site_pin_wire("SLICE_X56Y40", "A1").expect("dst");
        let mut claimed = HashMap::new();
        // The load's tile and its interconnect tile (the next column).
        let d = dev.tile(dev.tile_by_name(&dst.0).expect("tile")).expect("tile");
        for i in 0..dev.tile_count() {
            let t = dev.tile(i).expect("tile");
            if (t.grid_x - d.grid_x).abs() <= 1 && t.grid_y == d.grid_y {
                for w in dev.wires(i).unwrap_or(&[]) {
                    claimed.insert((t.name.clone(), w.clone()), "clk".to_string());
                }
            }
        }
        let opts = crate::route::RouteOpts { bbox: 0, ..Default::default() };
        let r = x.astar((&src.0, &src.1), &[], &[dst.clone()], opts, "n", &claimed);
        let e = LAST_EXPANDED.with(|e| e.get());
        eprintln!("expanded {e} of {} wires", x.wire_to_tile.len());
        assert!(matches!(r, Some(None)), "found a path through the clock wires");
        assert!(e > 6_500_000, "gave up after {e} expansions");
    }

    /// PathFinder bookkeeping: two nets on different wires of one node
    /// share it; one leaving ends the overuse.
    #[test]
    fn pathfinder_occupancy_is_per_node() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let a = ("INT_L_X2Y10".to_string(), "NN6BEG0".to_string());
        let members = RouteQuery::node_wires(&xdev, &a.0, &a.1); // the others
        let b = members.iter().find(|m| **m != a).expect("multi-wire node").clone();
        xdev.pf_present(Some(100));
        xdev.pf_occupy(std::slice::from_ref(&a), 1);
        assert!(!xdev.pf_overused(std::slice::from_ref(&b)));
        // The same net twice on one node still counts once.
        xdev.pf_occupy(&[b.clone()], 1);
        xdev.pf_occupy(std::slice::from_ref(&a), -1);
        xdev.pf_occupy(&[a.clone(), b.clone()], 1);
        assert!(xdev.pf_overused(std::slice::from_ref(&a)));
        assert_eq!(xdev.pf_history(), members.len() + 1, "every wire of the node");
        xdev.pf_occupy(&[b.clone()], -1);
        assert!(!xdev.pf_overused(&[a, b]));
        xdev.pf_present(None);
    }
}
