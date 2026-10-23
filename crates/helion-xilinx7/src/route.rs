//! Route stage: [`Placed7`] → [`Routed7`].
//!
//! BFS maze router on the 7-series pip graph. Each net is routed from its
//! driver site-pin to its load site-pins through INT-tile pips and
//! tile-to-tile node stitching. Site-internal nets (LUT→FF in one slice)
//! and the dedicated global clock network need no pips and are recorded
//! as such for the emitter.

use crate::place::Placed7;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// What the router needs from the device model.
pub trait RouteQuery {
    /// Pips with `src_wire == wire` in `tile`: `(dst_wire, src_wire, is_pseudo)`.
    /// Includes `always` pseudo-pips as zero-cost edges (caller may weight);
    /// the router traverses them for connectivity but never emits them.
    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)>;
    /// `(tile, wire)` pairs in neighboring tiles electrically connected to
    /// `(tile, wire)` via tileconn stitching.
    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)>;
    /// The wire driven by a site output pin, e.g. `("IOB_X1Y76", "O")`.
    fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)>;
    /// All `(tile, wire)` that can drive a site input pin.
    fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)>;
    /// Is this net on the dedicated global clock network (BUFG output)?
    fn is_clock_net(&self, net: &str) -> bool;
    /// Real grid coordinates of a tile, for the A* heuristic. `None` disables
    /// the heuristic for that tile (the search degrades to plain BFS), so
    /// minimal test routers need not implement this.
    fn tile_xy(&self, _tile: &str) -> Option<(i32, i32)> {
        None
    }
    /// Tile type (e.g. `CLK_BUFG_BOT_R`) for a tile name, if known. The clock
    /// buffer rule keys on this: wire names inside a BUFG are position- and
    /// family-dependent, but the tile type is stable.
    fn tile_type_of(&self, _tile: &str) -> Option<String> {
        None
    }
    /// Fabric-side entry point of the path that drives an output IOB's pad.
    ///
    /// A pad is not fed from the fabric directly: the signal enters the tile
    /// next to the IOI on an `IMUX34` and crosses the OLOGIC. Resolving the
    /// IOB's own output pin (`site.O`) instead is wrong -- that wire sits on
    /// the *pad* side of the OBUF, so the search has to walk backwards through
    /// `IOB_O_OUT0` and then flood the fabric until it stumbles onto some
    /// `IMUX34`, which need not be the one wired to this pad. See
    /// `XDevice::olog_sink` for the concrete wiring.
    ///
    /// Returns every `(tile, wire)` that may legitimately drive this IOB's
    /// pad, or an empty vec when the model cannot resolve one (callers then
    /// fall back to the site pin).
    fn iob_output_sink(&self, _site: &str) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// A single programmed pip: `tile.dst_wire <- src_wire`.
/// Pseudo-pips are always-on hard connections: traversed for connectivity
/// but never emitted to FASM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedPip {
    pub tile: String,
    pub dst_wire: String,
    pub src_wire: String,
    pub is_pseudo: bool,
}

/// How a net is routed.
#[derive(Clone, Debug)]
pub enum RouteKind {
    /// Driver and loads are inside one slice (e.g. LUT.O → FF.D).
    SiteInternal,
    /// Dedicated clock spine (BUFG → global network → FF.CLK).
    ClockSpine,
    /// General fabric route through INT pips.
    Pips(Vec<RoutedPip>),
}

/// A routed net.
#[derive(Clone, Debug)]
pub struct RoutedNet7 {
    pub net: String,
    pub kind: RouteKind,
}

/// Backend-owned routed netlist for xc7a35t.
#[derive(Clone, Debug, Default)]
pub struct Routed7 {
    pub placed: Placed7,
    pub nets: Vec<RoutedNet7>,
}

/// A routing endpoint: site + pin + whether it's a driver.
#[derive(Clone, Debug)]
struct Endpoint {
    site: String,
    pin: String,
    is_driver: bool,
}

/// Route all nets in the placed design.
///
/// Builds the net→endpoints map from placed cells, then routes each net.
/// Fails loudly on unroutable nets (names the net and endpoints).
pub fn route(placed: &Placed7, device: &dyn RouteQuery) -> Result<Routed7, String> {
    let packed = &placed.packed;
    // net -> endpoints
    let mut net_eps: HashMap<String, Vec<Endpoint>> = HashMap::new();
    let mut add_ep = |net: String, site: String, pin: &str, is_driver: bool| {
        net_eps.entry(net).or_default().push(Endpoint {
            site,
            pin: pin.to_string(),
            is_driver,
        });
    };

    // Map placed cells back to packed nets via name lookup.
    let lut_site: HashMap<&str, &str> = placed.luts.iter().map(|l| (l.name.as_str(), l.site.as_str())).collect();
    let lut_bel: HashMap<&str, &str> = placed.luts.iter().map(|l| (l.name.as_str(), l.bel.as_str())).collect();
    let ff_site: HashMap<&str, &str> = placed.ffs.iter().map(|f| (f.name.as_str(), f.site.as_str())).collect();
    let ff_bel: HashMap<&str, &str> = placed.ffs.iter().map(|f| (f.name.as_str(), f.bel.as_str())).collect();
    let iob_site: HashMap<&str, &str> = placed.iobs.iter().map(|i| (i.name.as_str(), i.site.as_str())).collect();
    let bufg_site: HashMap<&str, &str> = placed.bufgs.iter().map(|b| (b.name.as_str(), b.site.as_str())).collect();

    for lut in &packed.luts {
        let site = lut_site.get(lut.name.as_str()).copied().unwrap_or("");
        let bel = lut_bel.get(lut.name.as_str()).copied().unwrap_or("A6LUT");
        // Per-BEL pins: A6LUT -> A1-A6 inputs, AMUX output.
        let bel_letter = bel.chars().next().unwrap_or('A');
        for (i, net) in lut.input_nets.iter().enumerate() {
            if let Some(n) = net {
                add_ep(n.clone(), site.to_string(), &format!("{bel_letter}{}", i + 1), false);
            }
        }
        add_ep(lut.output_net.clone(), site.to_string(), &format!("{bel_letter}MUX"), true);
    }
    for ff in &packed.ffs {
        let site = ff_site.get(ff.name.as_str()).copied().unwrap_or("");
        let bel = ff_bel.get(ff.name.as_str()).copied().unwrap_or("AFF");
        // Q is per-BEL (AQ/BQ/CQ/DQ); D and CLK are slice-shared.
        let bel_letter = bel.chars().next().unwrap_or('A');
        add_ep(ff.d_net.clone(), site.to_string(), "D", false);
        add_ep(ff.q_net.clone(), site.to_string(), &format!("{bel_letter}Q"), true);
        add_ep(ff.clk_net.clone(), site.to_string(), "CLK", false);
    }
    for iob in &packed.iobs {
        let site = iob_site.get(iob.name.as_str()).copied().unwrap_or("");
        // X-Ray names IOB pins from the fabric's perspective: `I` is data
        // INTO the fabric (pad -> IBUF -> fabric, the input path), `O` is
        // data OUT OF the fabric (fabric -> OLOGIC -> OBUF -> pad, the
        // output path). The output path must go through the OLOGIC bypass.
        if iob.dir == crate::pack::IobDir::In {
            add_ep(iob.net.clone(), site.to_string(), "I", true);
        } else {
            add_ep(iob.net.clone(), site.to_string(), "O", false);
        }
    }
    for bufg in &packed.bufgs {
        let site = bufg_site.get(bufg.name.as_str()).copied().unwrap_or("");
        add_ep(bufg.in_net.clone(), site.to_string(), "I0", false);
        add_ep(bufg.out_net.clone(), site.to_string(), "O", true);
    }

    // Validate every net BEFORE routing anything, so a malformed netlist is
    // reported as such rather than as a routing failure.
    let mut net_names: Vec<String> = net_eps.keys().cloned().collect();
    net_names.sort();
    for net in &net_names {
        let eps = &net_eps[net];
        let nd = eps.iter().filter(|e| e.is_driver).count();
        if nd != 1 {
            return Err(format!(
                "route: net `{net}` has {nd} drivers (need exactly 1)"
            ));
        }
    }

    // Route nets one at a time against a SHARED occupancy map, ripping up and
    // re-routing earlier nets when a later one cannot find a free path.
    //
    // Routing each net independently is what produced illegal bitstreams: a
    // 7-series INT mux is one-hot, so two nets selecting different inputs of
    // the same mux output over-drive it, and prjxray writes some of those
    // selects with opposite polarity on the same configuration bit. The old
    // per-net parent tree prevented double-driving WITHIN a net and did
    // nothing across nets, while `route()` still returned Ok.
    let routable: Vec<&String> = net_names
        .iter()
        .filter(|n| net_eps[*n].iter().any(|e| !e.is_driver))
        .collect();
    let mut occupied: HashSet<(String, String)> = HashSet::new();
    // net -> (kind, wires it occupies)
    let mut routed: HashMap<String, (RouteKind, Vec<(String, String)>)> = HashMap::new();
    let mut order: Vec<String> = routable.iter().map(|s| (*s).clone()).collect();
    let mut ripped: usize = 0;
    let mut queue: VecDeque<String> = order.drain(..).collect();
    let mut attempts = 0usize;
    let max_attempts = net_eps.len().saturating_mul(24).max(256);
    while let Some(net) = queue.pop_front() {
        attempts += 1;
        if attempts > max_attempts {
            return Err(format!(
                "route: gave up after {attempts} attempts with {} net(s) unrouted ({} rip-ups); \
                 the design needs more routing resources than the search can free. Reduce \
                 congestion, or route with a lower logical effort.",
                queue.len() + 1,
                ripped
            ));
        }
        let eps = &net_eps[&net];
        let drivers: Vec<&Endpoint> = eps.iter().filter(|e| e.is_driver).collect();
        let loads: Vec<&Endpoint> = eps.iter().filter(|e| !e.is_driver).collect();
        let mut blockers: Vec<(String, String)> = Vec::new();
        match route_net(
            &net,
            drivers[0],
            &loads,
            device,
            &occupied,
            Some(&mut blockers),
        ) {
            Ok(kind) => {
                let wires = route_wires(&kind);
                for w in &wires {
                    occupied.insert(w.clone());
                }
                routed.insert(net, (kind, wires));
            }
            Err(_) => {
                // Rip up only the nets that actually blocked this one, then
                // retry this net and re-queue those.
                //
                // This used to discard EVERY routed net on any failure --
                // `for other in routed.keys()` with no reference to what was
                // actually in the way -- and re-queue all of them. Each failure
                // therefore threw away all completed work, so routing thrashed
                // instead of converging: an 86-LUT design took 153 s on zynq
                // and did not finish in 40 minutes on the Artix-35T fabric.
                ripped += 1;
                let blocked_set: HashSet<&(String, String)> = blockers.iter().collect();
                let mut displaced: Vec<String> = Vec::new();
                for (other, (_, wires)) in &routed {
                    if other != &net && wires.iter().any(|w| blocked_set.contains(w)) {
                        displaced.push(other.clone());
                    }
                }
                displaced.sort();
                for other in &displaced {
                    if let Some((_, wires)) = routed.remove(other) {
                        for w in &wires {
                            occupied.remove(w);
                        }
                    }
                }
                if displaced.is_empty() {
                    // Nothing in the way: genuinely unroutable, so report the
                    // real reason rather than looping.
                    route_net(&net, drivers[0], &loads, device, &occupied, None)?;
                    unreachable!("route_net returned Ok after Err");
                }
                queue.push_back(net);
                for other in displaced {
                    queue.push_back(other);
                }
            }
        }
    }

    let mut nets = Vec::new();
    for net in &net_names {
        match routed.remove(net) {
            Some((kind, _)) => nets.push(RoutedNet7 { net: net.clone(), kind }),
            None => {
                // No external loads: nothing to route.
                let eps = &net_eps[net];
                if eps.iter().all(|e| e.is_driver) {
                    continue;
                }
                unreachable!("net {net} missing from the routed set");
            }
        }
    }

    eprintln!(
        "route: {} nets, {} through the queue, {} rip-ups, {} occupied wires",
        net_names.len(),
        attempts,
        ripped,
        occupied.len()
    );
    Ok(Routed7 {
        placed: placed.clone(),
        nets,
    })
}

/// Every `(tile, wire)` a routed net claims, so occupancy can be tracked and
/// released on rip-up.
fn route_wires(kind: &RouteKind) -> Vec<(String, String)> {
    match kind {
        // The clock spine is a dedicated network: it does not contend with
        // ordinary fabric pips, and route_net builds it without consulting
        // the occupancy map.
        RouteKind::SiteInternal | RouteKind::ClockSpine => Vec::new(),
        RouteKind::Pips(pips) => pips
            .iter()
            .filter(|p| !p.is_pseudo && !p.tile.contains('~'))
            .flat_map(|p| {
                [
                    (p.tile.clone(), p.dst_wire.clone()),
                    (p.tile.clone(), p.src_wire.clone()),
                ]
            })
            .collect(),
    }
}

/// Is `tile` a clock buffer?
///
/// `emit.rs` owns every configuration bit inside these tiles, so the router may
/// only pass through them on its way into the buffer it is actually using.
fn is_clock_buffer_tile(device: &dyn RouteQuery, tile: &str) -> bool {
    device
        .tile_type_of(tile)
        .map(|t| t.contains("CLK_BUFG"))
        .unwrap_or(false)
}

/// Is `wire` the instance mux of a clock buffer in `tile`?
///
/// This is the one wire in a clock buffer tile that a route may step onto: the
/// input select `emit.rs` writes for the buffer. The buffer's *output* is a
/// valid source for the net that leaves it, which is why the rule is stated as
/// "allowed to enter" rather than "allowed to be here".
/// Does `wire` belong to `tile` itself, rather than to a neighbour reached
/// across a tile connector?
fn dst_inside_same_tile(device: &dyn RouteQuery, tile: &str, wire: &str) -> bool {
    device
        .node_wires(tile, wire)
        .iter()
        .any(|(t, w)| t == tile && w == wire)
}

/// Is the walk currently standing on a clock buffer's driven output?
///
/// These are the `*_O` mux outputs (`CLK_BUFG_BUFGCTRL0_O`) and the spine wire
/// they feed (`CLK_BUFG_CK_GCLK0`). From either one the buffer must be free to
/// leave for the fabric; nothing downstream belongs to the buffer any more.
fn is_clock_buffer_output(device: &dyn RouteQuery, tile: &str, wire: &str) -> bool {
    is_clock_buffer_tile(device, tile)
        && (wire.ends_with("_O") || wire.contains("CK_GCLK"))
}

fn is_clock_buffer_entry(device: &dyn RouteQuery, tile: &str, wire: &str) -> bool {
    is_clock_buffer_tile(device, tile) && wire.contains("BUFGCTRL") && wire.contains("_I")
}

/// Route one net from its driver to all loads.
fn route_net(
    net: &str,
    driver: &Endpoint,
    loads: &[&Endpoint],
    device: &dyn RouteQuery,
    occupied: &HashSet<(String, String)>,
    blocked: Option<&mut Vec<(String, String)>>,
) -> Result<RouteKind, String> {
    // Split loads into site-internal (same site as driver) and external.
    let (internal, external): (Vec<&&Endpoint>, Vec<&&Endpoint>) =
        loads.iter().partition(|l| l.site == driver.site);
    // If all loads are site-internal, no pips needed.
    if external.is_empty() {
        return Ok(RouteKind::SiteInternal);
    }
    // Clock nets are not free: the spine is a real chain of PIPs
    // (IOI/HCLK_CMT -> BUFG_REBUF -> HROW -> INT CLK -> slice CLK), with
    // per-stage active/enable bits that emit.rs adds alongside each PIP.
    // Returning `ClockSpine` here (the old behaviour) left the FFs without
    // a clock even when the BUFG itself was configured correctly.
    // Route to external loads via pips. (Internal loads are site-local.)
    let external_loads: Vec<&Endpoint> = external.into_iter().copied().collect();
    let _ = internal; // site-internal loads need no pips

    let (src_tile, src_wire) = device
        .site_pin_wire(&driver.site, &driver.pin)
        .ok_or_else(|| format!("route: no wire for driver {}.{} (net `{net}`)", driver.site, driver.pin))?;

    // Route all loads from ONE BFS tree. A per-load BFS union is unsafe:
    // different loads can select different sources for the same mux (e.g.
    // HROW MUX_OUT_R0 fed by both CK_IN_L0 and CK_GCLK0), and the union bit
    // pattern matches no valid mux feature. A single parent tree guarantees
    // exactly one driver per wire, and is one BFS instead of one per load.
    let mut sinks: Vec<(String, String)> = Vec::new();
    for load in &external_loads {
        // An output IOB is not driven through its own pin: the pad hangs off
        // the far side of the OBUF/OLOGIC, so the routable target is the
        // OLOGIC's fabric-side entry (an `IMUX34` in the neighbouring INT
        // tile). Targeting `site.O` instead makes the search traverse the
        // OBUF backwards and then flood, which silently lands on whichever
        // `IMUX34` it reaches first -- not necessarily this pad's. Fall back
        // to the site pin only when the device model cannot resolve it, so
        // minimal test routers keep working.
        let s = if load.pin == "O" {
            device.iob_output_sink(&load.site)
        } else {
            Vec::new()
        };
        let s = if s.is_empty() {
            device.wire_for_site_pin(&load.site, &load.pin)
        } else {
            s
        };
        if s.is_empty() {
            return Err(format!(
                "route: no wire for load {}.{} (net `{net}`)",
                load.site, load.pin
            ));
        }
        sinks.extend(s);
    }
    sinks.sort();
    sinks.dedup();
    let pips = bfs_route_all(&src_tile, &src_wire, &sinks, device, occupied, blocked).ok_or_else(|| {
        format!(
            "route: net `{net}` unroutable to one or more of {} loads",
            external_loads.len()
        )
    })?;
    Ok(RouteKind::Pips(pips))
}

/// BFS from `(src_tile, src_wire)` to every sink in `sinks: [(tile, wire)]`.
///
/// Builds a single parent tree, so every wire — and therefore every mux —
/// has exactly one driver. The returned pip set is the union of the tree
/// paths, sorted for deterministic output. Returns `None` unless every sink
/// is reachable.
fn bfs_route_all(
    src_tile: &str,
    src_wire: &str,
    sinks: &[(String, String)],
    device: &dyn RouteQuery,
    occupied: &HashSet<(String, String)>,
    // Occupied wires that stopped an expansion. Collected so a failed net can
    // rip up only the nets that actually blocked it, instead of every net.
    mut blocked: Option<&mut Vec<(String, String)>>,
) -> Option<Vec<RoutedPip>> {
    let sink_set: HashSet<(&str, &str)> =
        sinks.iter().map(|(t, w)| (t.as_str(), w.as_str())).collect();
    // (tile, wire) -> parent ((tile, wire), pip used to get here)
    let mut parent: HashMap<(String, String), Option<((String, String), RoutedPip)>> = HashMap::new();
    let mut pops: usize = 0;
    let start = (src_tile.to_string(), src_wire.to_string());

    // A* instead of plain BFS. Unbounded BFS expanded ~363,000 pips per net
    // (~160M edge expansions for a 119-net design) because it explores the
    // whole ~3.3M-node graph uniformly, with wall time swinging between 77 s
    // and 667 s on identical input depending on hash order. Ordering the
    // frontier by `g + manhattan_distance_to_nearest_remaining_sink` drives
    // the search at the target instead of flooding the fabric.
    //
    // `BinaryHeap` is a max-heap, so costs are negated. Ties break on the
    // node name to keep the result deterministic across runs.
    let mut heap: BinaryHeap<Reverse<(i64, Reverse<String>, Reverse<String>)>> = BinaryHeap::new();
    let mut g: HashMap<(String, String), i64> = HashMap::new();
    g.insert(start.clone(), 0);
    parent.insert(start.clone(), None);

    // Grid coordinates of every sink, tagged with the sink so the heuristic
    // can skip the ones already reached.
    let sink_xy: Vec<((String, String), (i32, i32))> = sinks
        .iter()
        .filter_map(|(t, w)| device.tile_xy(t).map(|xy| ((t.clone(), w.clone()), xy)))
        .collect();
    // Distance to the nearest sink NOT yet found.
    //
    // This used to be the distance to the nearest sink of ANY kind, which is
    // the wrong estimate once that sink has been reached: the search still has
    // to reach the others, but the heuristic kept steering it back toward the
    // one already found. A* then stopped being goal-directed and flooded the
    // fabric. One net needed 4,534,494 pops to reach 4 sinks, all of them
    // inside one or two tiles of the driver; routing the 90-LUT blinky took
    // 153 s, 99.9% of the whole run.
    //
    // Ignoring `found` keeps the estimate admissible (distance to the nearest
    // still-missing sink is a lower bound on the remaining tree) and restores
    // the goal direction. When every sink is found the search breaks anyway,
    // so the zero estimate at that point costs nothing.
    let h = |tile: &str, found: &HashSet<(String, String)>| -> i64 {
        let (x, y) = match device.tile_xy(tile) {
            Some(xy) => xy,
            // No coordinates available: behave exactly like BFS.
            None => return 0,
        };
        sink_xy
            .iter()
            .filter(|(k, _)| !found.contains(k))
            .map(|(_, (sx, sy))| ((x - sx).abs() + (y - sy).abs()) as i64)
            .min()
            .unwrap_or(0)
    };
    let mut found: HashSet<(String, String)> = HashSet::new();
    heap.push(Reverse((h(src_tile, &found), Reverse(start.0.clone()), Reverse(start.1.clone()))));
    let mut closes: HashSet<(String, String)> = HashSet::new();
    let mut expansions: usize = 0;
    while let Some(Reverse((_, Reverse(tile), Reverse(wire)))) = heap.pop() {
        pops += 1;
        let cur = (tile.clone(), wire.clone());
        let cur_g = g.get(&cur).copied().unwrap_or(i64::MAX);
        if !closes.insert(cur.clone()) {
            continue;
        }
        if sink_set.contains(&(tile.as_str(), wire.as_str())) {
            found.insert((tile.clone(), wire.clone()));
            if found.len() == sink_set.len() {
                break;
            }
        }
        // Through pips in this tile (including pseudo-pips: always-on
        // connections like the OLOGIC bypass, traversed but not emitted).
        for (dst_wire, src_w, is_pseudo) in device.pips_from(&tile, &wire) {
            debug_assert_eq!(src_w, wire);
            expansions += 1;
            // A clock buffer tile may only ever be crossed on one of two
            // kinds of transition:
            //
            //   out   `CLK_BUFG_BUFGCTRL0_O -> CLK_BUFG_CK_GCLK0`  (the
            //         buffer's output leaves for the spine) -- any edge whose
            //         SOURCE is a `*_O` mux output, since that is the buffer's
            //         driven side.
            //   in    `... -> CLK_BUFG_BUFGCTRL0_I0`  (a route arriving at the
            //         instance mux `emit.rs` selected for this buffer).
            //
            // Everything else is forbidden, and the reason is concrete rather
            // than tidy-mindedness. The tile also offers
            // `BUFGCTRL1_O <- BUFGCTRL1_I0`, `BUFGCTRL2_O <- BUFGCTRL2_I0`, ...
            // a chain *within* the tile whose last link lands on any
            // `BUFGCTRL<n>_I0`. Searching `clk_ibuf` -- which starts at the Y9
            // pad, `LIOB33_X0Y25.IOB_IBUF0`, and must reach
            // `CLK_BUFG_BOT_R_X87Y48.CLK_BUFG_BUFGCTRL0_I0` -- took that chain
            // and "arrived" without travelling: the FASM came out with 19 pips
            // cascading BUFGCTRL1..3 and **not a single pip leaving the pad**.
            // The FPGA configured (`done` read 1, the CRC was fine) and every
            // flip-flop sat on a buffer with no input, so nothing ever blinked.
            //
            // Matching wire names cannot express this. The earlier guards here
            // tested `dst_wire` for `FBG_OUT`; in `BUFGCTRL1_I0 <- FBG_OUT2`
            // the string is in the *source*, so the cascade passed straight
            // through them.
            // Never cross a clock buffer tile internally. The buffer has
            // exactly one wire this walk may step onto from outside -- the
            // instance mux `emit.rs` selects -- and everything else in the
            // tile is a chain between slots:
            //
            //     BUFGCTRL1_O <- BUFGCTRL1_I0 <- FBG_OUT0 <- BUFGCTRL0_O
            //
            // Searching `clk_ibuf`, which starts at the Y9 pad
            // (`LIOB33_X0Y25.IOB_IBUF0`) and must reach
            // `CLK_BUFG_BOT_R_X87Y48.CLK_BUFG_BUFGCTRL0_I0`, took that chain
            // and "arrived" without travelling: the FASM came out with 19 pips
            // cascading BUFGCTRL1..3 and **not one pip leaving the pad**. The
            // FPGA configured -- `done` read 1 and the CRC was fine -- and
            // every flip-flop sat on a buffer with no input, so nothing ever
            // blinked.
            //
            // Wire names cannot express this. The guards that used to be here
            // tested `dst_wire` for `FBG_OUT`; in `BUFGCTRL1_I0 <- FBG_OUT2`
            // that string is the *source*, so the cascade went straight
            // through. Leaving the tile is unrestricted, because the buffer's
            // output must reach the spine (`BUFGCTRL0_O -> CK_GCLK0`) and its
            // source is the wire we are already on.
            // Inside a clock buffer the only transition this walk may make is
            // onto the instance mux `emit.rs` selected. Everything else is the
            // slot-to-slot chain
            //
            //     BUFGCTRL1_I0 <- FBG_OUT1 <- BUFGCTRL1_O <- BUFGCTRL1_I0
            //
            // which is *entirely within the tile*. `clk_ibuf` starts at the Y9
            // pad (`LIOB33_X0Y25.IOB_IBUF0`) and must reach
            // `CLK_BUFG_BOT_R_X87Y48.CLK_BUFG_BUFGCTRL0_I0`; it took this chain
            // and "arrived" without travelling, emitting 19 pips that cascade
            // BUFGCTRL1..4 and **not one pip leaving the pad**. The FPGA
            // configured -- `done` read 1, the CRC was fine -- and every FF sat
            // on a buffer with no input, so the board never blinked.
            //
            // Testing `dst_wire` for `FBG_OUT` cannot work: in
            // `BUFGCTRL1_I0 <- FBG_OUT1` the string is the SOURCE. The tile
            // itself is the reliable boundary.
            // Leaving the buffer is unrestricted: its output must reach the
            // spine (`BUFGCTRL0_O -> CK_GCLK0`), and that edge is emitted from
            // the source side. Only *arriving* somewhere inside the tile is
            // restricted.
            let arriving_inside = is_clock_buffer_tile(device, &tile)
                && !wire.starts_with("CLK_BUFG_")
                    .then_some(false)
                    .unwrap_or(false);
            let _ = arriving_inside;
            if is_clock_buffer_tile(device, &tile)
                && !is_clock_buffer_entry(device, &tile, &dst_wire)
                && !is_clock_buffer_output(device, &tile, &wire)
            {
                continue;
            }
            let next = (tile.clone(), dst_wire.clone());
            // A wire that already drives another net must not be selected
            // again. A 7-series INT mux is one-hot: two nets choosing
            // different inputs of the same mux output is an electrical
            // over-drive, not an OR, and X-Ray writes some of those selects
            // with opposite polarity on the same bit, so the reference
            // assembler rejects the whole bitstream:
            //
            //   FASM line "INT_R_X71Y42.FAN_ALT6.NR1END1" wanted to set bit
            //   (4334486, 85, 24) but was cleared by
            //   "INT_R_X71Y42.FAN_ALT6.LOGIC_OUTS23"
            //
            // This check existed only on the `node_wires` stitch path below,
            // never here, so a net could route straight through a mux another
            // net already drove. The two nets that met were `u_wshare_n32` and
            // `u_wshare_n80`, on the fanout feeding the clock-distribution
            // network -- which is why the design configured cleanly and still
            // did nothing.
            if occupied.contains(&next) {
                if let Some(b) = blocked.as_deref_mut() {
                    b.push(next.clone());
                }
                continue;
            }
            let ng = cur_g.saturating_add(1);
            if g.get(&next).map(|old| ng < *old).unwrap_or(true) {
                let pip = RoutedPip {
                    tile: tile.clone(),
                    dst_wire: dst_wire.clone(),
                    src_wire: wire.clone(),
                    is_pseudo,
                };
                parent.insert(next.clone(), Some(((tile.clone(), wire.clone()), pip)));
                g.insert(next.clone(), ng);
                heap.push(Reverse((
                    ng.saturating_add(h(&next.0, &found)),
                    Reverse(next.0.clone()),
                    Reverse(next.1.clone()),
                )));
            }
        }
        // Across tileconn stitching. This is a real resource traversal, so it
        // costs one hop exactly like a pip. Charging 0 made a chain of
        // same-node wires look free, and with the A* heuristic working the
        // search then preferred to hop the clock through five BUFGCTRL stages
        // (BUFGCTRL1_O -> FBG_OUT2 -> BUFGCTRL2_I0 -> ...) instead of the
        // dedicated spine: a shorter hop count, an illegal cascade, and the
        // reference assembler refuses it as a bit set by two conflicting
        // lines.
        for (ntile, nwire) in device.node_wires(&tile, &wire) {
            // `node_wires` unifies a wire across every instance in a tile, and
            // the BUFG `_CK_MUXED*` inputs share a node across all 17 slots.
            // The search therefore steps from BUFGCTRL0's input to
            // BUFGCTRL1's input at no cost, leaves through `FBG_OUT1`, and
            // cascades -- reaching the clock spine in fewer hops than the
            // dedicated path. `emit.rs` writes those input selects for the
            // net's own buffer only, so the cascade makes one configuration
            // bit carry two sources and prjxray refuses the bitstream:
            //
            //   FASM line "...BUFGCTRL0_I0.CLK_BUFG_R_FBG_OUT1" wanted to set
            //   bit (4329626, 93, 7) but was cleared by
            //   "...BUFGCTRL0_I0.CLK_BUFG_BOT_R_CK_MUXED0"
            //
            // A clock reaches its buffer only by the path `route_net` builds,
            // so skipping this stitch removes the shortcut and nothing legal.

            // Same two-transition rule as the pip walk above: a buffer may be
            // left from its `_O` side and entered at its instance mux, but
            // never crossed internally. This stitch is where the cascade is
            // easiest to fall into, because a wire of this name exists in
            // *every* BUFG tile, so the stitch alone wires this buffer's
            // internals to the next one's.
            // The stitch LANDING here must follow the same rule. `leaving`
            // is decided by the buffer we are coming *from*, not by the tile
            // being entered: once the walk has stepped off `BUFGCTRL0_O` onto
            // `CLK_BUFG_CK_GCLK0`, the wire name no longer mentions BUFGCTRL,
            // and testing the current wire would block the buffer's own output
            // from ever reaching the spine.
            // Same rule for a stitch: it may enter a buffer only at the
            // instance mux, and only when it is not hopping between slots of
            // the tile we are already inside. A stitch to a *different* tile
            // is how the spine is built (`CK_GCLK0` reaches `CLK_FEED`,
            // `BRKH_CLK`, `CLK_BUFG_REBUF`, ...), so it must stay allowed.
            if is_clock_buffer_tile(device, &ntile)
                && ntile == tile
                && !is_clock_buffer_entry(device, &ntile, &nwire)
            {
                continue;
            }
            let next = (ntile.clone(), nwire.clone());
            if occupied.contains(&next) {
                if let Some(b) = blocked.as_deref_mut() {
                    b.push(next.clone());
                }
                continue;
            }
            let ng = cur_g.saturating_add(1);
            if g.get(&next).map(|old| ng < *old).unwrap_or(true) {
                parent.insert(next.clone(), Some(((tile.clone(), wire.clone()), RoutedPip {
                    tile: format!("{tile}~{ntile}"),
                    dst_wire: nwire.clone(),
                    src_wire: wire.clone(),
                    is_pseudo: false,
                })));
                g.insert(next.clone(), ng);
                heap.push(Reverse((
                    ng.saturating_add(h(&next.0, &found)),
                    Reverse(next.0.clone()),
                    Reverse(next.1.clone()),
                )));
            }
        }
    }

    if found.len() != sink_set.len() {
        return None;
    }
    // Walk every sink back to the root. The parent tree gives each wire a
    // single driver, so the union cannot over-drive a mux.
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let mut pips: Vec<RoutedPip> = Vec::new();
    for sink in found {
        let mut cur = sink;
        while let Some(Some((prev, pip))) = parent.get(&cur) {
            // Skip tileconn pseudo-pips (marked with ~); real pips only.
            if !pip.tile.contains('~')
                && seen.insert((
                    pip.tile.clone(),
                    pip.dst_wire.clone(),
                    pip.src_wire.clone(),
                ))
            {
                pips.push(pip.clone());
            }
            cur = prev.clone();
            if parent.get(&cur).map(|o| o.is_none()).unwrap_or(true) {
                break;
            }
        }
    }
    pips.sort_by(|a, b| {
        (&a.tile, &a.dst_wire, &a.src_wire).cmp(&(&b.tile, &b.dst_wire, &b.src_wire))
    });
    Some(pips)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{IobDir, Packed7, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedIob, PlacedLut};

    struct FakeRouter;
    impl RouteQuery for FakeRouter {
        fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
            // Tiny graph: INT_X0: A -> B -> C ; INT_X1: C -> D
            match (tile, wire) {
                ("INT_X0", "A") => vec![("B".into(), "A".into(), false)],
                ("INT_X0", "B") => vec![("C".into(), "B".into(), false)],
                ("INT_X1", "C") => vec![("D".into(), "C".into(), false)],
                _ => vec![],
            }
        }
        fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
            match (tile, wire) {
                ("INT_X0", "C") => vec![("INT_X1".into(), "C".into())],
                _ => vec![],
            }
        }
        fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)> {
            match (site, pin) {
                ("SLICE_X0", "AMUX") => Some(("INT_X0".into(), "B".into())),
                _ => None,
            }
        }
        fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)> {
            match (site, pin) {
                ("IOB_X1", "O") => vec![("INT_X1".into(), "D".into())],
                _ => vec![],
            }
        }
        fn is_clock_net(&self, net: &str) -> bool {
            net == "clk_bufg"
        }
    }

    fn placed_two_site() -> Placed7 {
        let packed = Packed7 {
            luts: vec![PackedLut {
                name: "u_inv".into(),
                init: 0,
                input_nets: [None, None, None, None, None, None],
                output_net: "n1".into(),
            }],
            ffs: vec![],
            iobs: vec![
                PackedIob {
                    name: "obuf".into(), port: "b".into(), dir: IobDir::Out,
                    net: "n1".into(), loc: None, iostandard: "LVCMOS33".into(),
                    drive: None, slew: None,
                },
            ],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
        };
        Placed7 {
            packed,
            luts: vec![PlacedLut { name: "u_inv".into(), init: 0, tile: "CLB_X0".into(), site: "SLICE_X0".into(), bel: "A6LUT".into() }],
            ffs: vec![],
            iobs: vec![
                PlacedIob { name: "obuf".into(), port: "b".into(), dir: IobDir::Out, tile: "IOB_T".into(), site: "IOB_X1".into() },
            ],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
        }
    }

    #[test]
    fn bfs_finds_pip_path() {
        let routed = route(&placed_two_site(), &FakeRouter).unwrap();
        assert_eq!(routed.nets.len(), 1);
        match &routed.nets[0].kind {
            RouteKind::Pips(pips) => {
                // B -> C (pip), C -> C across tiles (node, skipped), C -> D (pip).
                assert_eq!(pips.len(), 2);
                assert_eq!(pips[0].dst_wire, "C");
                assert_eq!(pips[1].dst_wire, "D");
            }
            other => panic!("expected Pips, got {other:?}"),
        }
    }

    #[test]
    fn unroutable_net_fails_loudly() {
        struct NoRoute;
        impl RouteQuery for NoRoute {
            fn pips_from(&self, _: &str, _: &str) -> Vec<(String, String, bool)> { vec![] }
            fn node_wires(&self, _: &str, _: &str) -> Vec<(String, String)> { vec![] }
            fn site_pin_wire(&self, _: &str, _: &str) -> Option<(String, String)> { None }
            fn wire_for_site_pin(&self, _: &str, _: &str) -> Vec<(String, String)> { vec![] }
            fn is_clock_net(&self, _: &str) -> bool { false }
        }
        let err = route(&placed_two_site(), &NoRoute).unwrap_err();
        assert!(err.contains("n1"), "got: {err}");
    }

    #[test]
    fn multi_sink_bfs_uses_one_parent_per_wire() {
        struct MuxRouter;
        impl RouteQuery for MuxRouter {
            fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
                match (tile, wire) {
                    ("T", "A") => vec![("M".into(), "A".into(), false)],
                    ("T", "M") => vec![
                        ("S1".into(), "M".into(), false),
                        ("S2".into(), "M".into(), false),
                    ],
                    _ => vec![],
                }
            }
            fn node_wires(&self, _: &str, _: &str) -> Vec<(String, String)> {
                vec![]
            }
            fn site_pin_wire(&self, _: &str, _: &str) -> Option<(String, String)> {
                None
            }
            fn wire_for_site_pin(&self, _: &str, _: &str) -> Vec<(String, String)> {
                vec![]
            }
            fn is_clock_net(&self, _: &str) -> bool {
                false
            }
        }
        let sinks = vec![
            ("T".to_string(), "S1".to_string()),
            ("T".to_string(), "S2".to_string()),
        ];
        let empty = HashSet::new();
        let pips = bfs_route_all("T", "A", &sinks, &MuxRouter, &empty, None).expect("all sinks");
        assert!(pips.iter().any(|p| p.dst_wire == "S1"));
        assert!(pips.iter().any(|p| p.dst_wire == "S2"));
        // The mux input must be driven once, not unioned from two branches.
        assert_eq!(pips.iter().filter(|p| p.dst_wire == "M").count(), 1);

        let unreachable = vec![
            ("T".to_string(), "S1".to_string()),
            ("T".to_string(), "NX".to_string()),
        ];
        assert!(bfs_route_all("T", "A", &unreachable, &MuxRouter, &empty, None).is_none());
    }

    #[test]
    fn clock_net_uses_spine() {
        // The router no longer short-circuits clock nets; is_clock_net is
        // still a device query (kept for callers that classify clocks).
        assert!(FakeRouter.is_clock_net("clk_bufg"));
        assert!(!FakeRouter.is_clock_net("n1"));
    }
}
