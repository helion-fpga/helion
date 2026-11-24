//! UltraScale+ PathFinder router scaffold.
//!
//! Implements negotiated-congestion PathFinder routing against the WireDb
//! trait seam. This scaffold develops the algorithm against a mock WireDb;
//! the production code moves into helion-ultrascale once device-model creates
//! the crate.
//!
//! Seed determinism: SplitMix64 (same as helion-place seeded_order) drives
//! net processing order, A* tie-breaking, and initial cost perturbation.
//! No HashMap iteration in the hot path.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

// ============================================================================
// WireDb seam: owned by crate::fabric (day-1 contract). The router codes
// against fabric::WireDb; the routing-graph agent provides the impl.
// ============================================================================

use crate::fabric::{UltraSite, WireDb, WireId};

// ============================================================================
// Router API
// ============================================================================

#[derive(Clone, Debug)]
pub struct RoutableNet {
    pub name: String,
    pub driver: (UltraSite, String, String),
    pub sinks: Vec<(UltraSite, String, String)>,
    pub is_clock: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct UltraRouteOpts {
    pub seed: u64,
    pub max_iters: u32,
}

impl Default for UltraRouteOpts {
    fn default() -> Self {
        Self {
            seed: 0,
            max_iters: 50,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UltraRouted {
    /// Per net: wire path from driver through all sinks (Steiner tree flattened).
    pub routes: Vec<Vec<WireId>>,
    pub iters_used: u32,
    pub overused_nodes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteErrorKind {
    NoPath,
    OveruseTimeout,
    ClockNetInPhase1,
}

#[derive(Clone, Debug)]
pub struct RouteError {
    pub net: String,
    pub location: String,
    pub kind: RouteErrorKind,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "route failed for net '{}' at {}: {:?}",
            self.net, self.location, self.kind
        )
    }
}

impl std::error::Error for RouteError {}

// ============================================================================
// SplitMix64 (same as helion-place)
// ============================================================================

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn seeded_order(n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    if seed == 0 {
        return order;
    }
    let mut state = seed;
    for i in (1..n).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

// ============================================================================
// A* search
// ============================================================================

struct AstarNode {
    wire: WireId,
    f_score: i64,
    tie: u64, // seed-derived tie-breaker for determinism
}

impl Eq for AstarNode {}

impl PartialEq for AstarNode {
    fn eq(&self, other: &Self) -> bool {
        self.f_score == other.f_score && self.tie == other.tie && self.wire == other.wire
    }
}

impl Ord for AstarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is max-heap; reverse for min-heap.
        other
            .f_score
            .cmp(&self.f_score)
            .then_with(|| other.tie.cmp(&self.tie))
            .then_with(|| other.wire.cmp(&self.wire))
    }
}

impl PartialOrd for AstarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct AstarScratch {
    g_score: Vec<i64>,
    came_from: Vec<WireId>,
    visited_gen: Vec<u32>,
    generation: u32,
}

impl AstarScratch {
    fn new(n: usize) -> Self {
        Self {
            g_score: vec![i64::MAX; n],
            came_from: vec![WireId::MAX; n],
            visited_gen: vec![0; n],
            generation: 0,
        }
    }
}

/// A* from src wire to dst wire.
/// Returns the wire path or None if unreachable.
/// Eleven arguments is the day-1 scaffold signature (db, scratch, src,
/// dst, congestion history, presence, own-use, presence factor, tie
/// seed, blocked BYPASS wires, this net's sink wires); a parameter
/// split would churn the router scaffold for no semantic gain.
///
/// N58: `blocked` holds the BYPASS wires that feed a used site pin
/// (a sink wire of some net in this routing call). A* never expands
/// from a blocked wire unless it is this leg's source or one of this
/// net's own sink wires: a BYPASS wire is physically the site pin's
/// feed, so passing through it drives that pin with this net. The
/// leg's destination is always one of its own sinks, so it is never
/// blocked for its own net.
///
/// Presence cost counts OTHER nets only: `own` marks the wires this
/// net already uses from its earlier sink legs this iteration, and
/// those wires cost no presence penalty. Sharing a wire within one
/// net is the Steiner-tree trunk, not congestion: the overuse metric
/// counts each wire once per net, so charging a net for its own trunk
/// makes the cost model disagree with the objective it optimizes.
/// Without this exclusion a multi-sink net's later legs detour around
/// their own trunk (at `pres_fac` per shared wire) and collide with
/// other nets instead, freezing PathFinder on a wire another net is
/// forced to use (counter: BYPASS_W6, see the router tests).
#[allow(clippy::too_many_arguments)]
fn astar(
    wires: &dyn WireDb,
    scratch: &mut AstarScratch,
    src: WireId,
    dst: WireId,
    hist: &[u32],
    pres: &[u32],
    own: &[bool],
    pres_fac: i64,
    tie_seed: u64,
    blocked: &HashSet<WireId>,
    own_sinks: &[WireId],
) -> Option<Vec<WireId>> {
    let n = wires.wire_count();
    scratch.generation = scratch.generation.wrapping_add(1);
    let generation = scratch.generation;

    let (dx, dy) = wires.wire_xy(dst);
    let heuristic = |w: WireId| -> i64 {
        let (x, y) = wires.wire_xy(w);
        ((x - dx).abs() + (y - dy).abs()) as i64
    };

    let mut heap = BinaryHeap::new();
    let mut tie_state = tie_seed ^ (src as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);

    scratch.g_score[src as usize] = 0;
    scratch.visited_gen[src as usize] = generation;
    heap.push(AstarNode {
        wire: src,
        f_score: heuristic(src),
        tie: splitmix64(&mut tie_state),
    });

    while let Some(node) = heap.pop() {
        let w = node.wire;
        if w == dst {
            // Reconstruct path.
            let mut path = Vec::new();
            let mut cur = dst;
            loop {
                path.push(cur);
                if cur == src {
                    break;
                }
                cur = scratch.came_from[cur as usize];
            }
            path.reverse();
            return Some(path);
        }
        let g = scratch.g_score[w as usize];
        // Skip stale entries.
        if node.f_score != g + heuristic(w) {
            continue;
        }
        // N58: a BYPASS wire feeding a used site pin is terminal-only.
        // Expanding from it would drive that pin with this net, so a
        // leg may end there (when it is this net's own sink) but never
        // pass through it. The leg's source and its own sink wires
        // are exempt: the source drives its own pin by construction,
        // and this net's own sink pin is meant to see this net.
        if w != src && blocked.contains(&w) && !own_sinks.contains(&w) {
            continue;
        }
        for &next in wires.fanout(w) {
            let ni = next as usize;
            if ni >= n {
                continue;
            }
            // PathFinder cost: base 1 + hist + pres * pres_fac, where
            // pres counts OTHER nets only (see astar docs): a wire
            // this net already uses costs no presence penalty.
            // `pres` counts every wire other nets use, terminals
            // included (the caller counts them), so a sink wire of
            // another net is expensive to pass through, as it must
            // be: one mux drives one wire.
            let pres_others = pres[ni].saturating_sub(u32::from(own[ni]));
            let cost = 1i64 + hist[ni] as i64 + pres_others as i64 * pres_fac;
            let ng = g + cost;
            if scratch.visited_gen[ni] != generation || ng < scratch.g_score[ni] {
                scratch.visited_gen[ni] = generation;
                scratch.g_score[ni] = ng;
                scratch.came_from[ni] = w;
                heap.push(AstarNode {
                    wire: next,
                    f_score: ng + heuristic(next),
                    tie: splitmix64(&mut tie_state),
                });
            }
        }
    }
    None
}

// ============================================================================
// PathFinder main loop
// ============================================================================

pub fn route_ultra(
    nets: &[RoutableNet],
    wires: &dyn WireDb,
    opts: UltraRouteOpts,
) -> Result<UltraRouted, RouteError> {
    // Phase 1: reject clock nets loudly.
    for net in nets {
        if net.is_clock {
            return Err(RouteError {
                net: net.name.clone(),
                location: format!("driver at {:?}", net.driver.0),
                kind: RouteErrorKind::ClockNetInPhase1,
            });
        }
    }

    let n_wires = wires.wire_count();
    if n_wires == 0 {
        return Err(RouteError {
            net: String::new(),
            location: "empty WireDb".to_string(),
            kind: RouteErrorKind::NoPath,
        });
    }

    // Resolve BEL pins to wires.
    let mut resolved: Vec<(String, WireId, Vec<WireId>)> = Vec::with_capacity(nets.len());
    for net in nets {
        let src_wire = wires
            .wire_for_bel_pin(net.driver.0, &net.driver.1, &net.driver.2)
            .ok_or_else(|| RouteError {
                net: net.name.clone(),
                location: format!(
                    "driver pin {}.{}.{} has no wire",
                    net.driver.0.x, net.driver.1, net.driver.2
                ),
                kind: RouteErrorKind::NoPath,
            })?;
        let mut sink_wires = Vec::with_capacity(net.sinks.len());
        for (site, bel, pin) in &net.sinks {
            let w = wires
                .wire_for_bel_pin(*site, bel, pin)
                .ok_or_else(|| RouteError {
                    net: net.name.clone(),
                    location: format!("sink pin {}.{}.{} has no wire", site.x, bel, pin),
                    kind: RouteErrorKind::NoPath,
                })?;
            sink_wires.push(w);
        }
        resolved.push((net.name.clone(), src_wire, sink_wires));
    }

    // N58: BYPASS wires that feed a used site pin are terminal-only,
    // never transit. A BYPASS wire is the feed of exactly one site
    // pin (tileconn: W1 BX, W3 DX, W4-W7 A_I-D_I, W8 EX, W9 FX,
    // W10 GX, W11 HX, W12 E_I, W14 G_I), so a net that passes
    // through one drives that pin with its own signal. The used
    // pins are exactly the routed sinks: every checker-consumed
    // BYPASS pin (a BYP FF D, via its X pin) is a sink of a routed
    // net, while D6 FF D pins are ignored by the FFMUX and the
    // _I pins are consumed by no model, so neither needs
    // protection. A wire that no net names as a sink stays usable
    // as transit. A net may still end at its own BYPASS sink wire
    // (and pass through it on a later leg to another sink): the pin
    // is meant to see this net. A wire whose name the DB does not
    // report (the mock WireDb) is never protected.
    let mut protected: HashSet<WireId> = HashSet::new();
    for (_, _, sinks) in &resolved {
        for &w in sinks {
            if wires
                .wire_info(w)
                .is_some_and(|i| i.name.starts_with("BYPASS_"))
            {
                protected.insert(w);
            }
        }
    }

    let mut hist = vec![0u32; n_wires];
    let mut pres = vec![0u32; n_wires];
    let mut scratch = AstarScratch::new(n_wires);

    // Env-gated trace for congestion diagnosis (HELION_ROUTE_DEBUG=1):
    // resolved pin-to-wire mapping and, per iteration, the overused
    // wires with their present-use count and historic cost. No output
    // unless the variable is set.
    let route_debug = std::env::var("HELION_ROUTE_DEBUG").is_ok();
    if route_debug {
        for (name, src, sinks) in &resolved {
            let src_name = wires
                .wire_info(*src)
                .map(|i| i.name.to_string())
                .unwrap_or_else(|| "?".to_string());
            let sink_names: Vec<String> = sinks
                .iter()
                .map(|w| {
                    format!(
                        "{}:{}",
                        w,
                        wires.wire_info(*w).map(|i| i.name).unwrap_or("?")
                    )
                })
                .collect();
            eprintln!(
                "route_debug resolved net={name} src={src}:{src_name} sinks=[{}]",
                sink_names.join(",")
            );
        }
    }

    let max_iters = opts.max_iters.max(1);
    let mut best_overused = u32::MAX;
    let mut best_wire_count = u64::MAX;
    let mut best_routes: Vec<Vec<WireId>> = vec![Vec::new(); nets.len()];
    let mut best_pres: Vec<u32> = vec![0; n_wires];
    let mut iters_used = 0u32;

    // Seed-derived net processing order. Fixed across iterations:
    // per-iteration re-draws were tried for N48 and REJECTED by
    // measurement (harness: counter16 seed 0 went from 22 iters
    // routed to OveruseTimeout, 76 iters at a 150 budget):
    // reshuffling spreads history costs thinly across the
    // contenders instead of accumulating them on the contested
    // wire, so no net ever feels enough pressure to detour. The
    // fixed order plus history-cost accumulation is the mechanism
    // that converges; seed variation comes from order +
    // tie-breaks + cost perturbation.
    let net_order = seeded_order(nets.len(), opts.seed);

    for iter in 0..max_iters {
        iters_used = iter + 1;
        pres.fill(0);
        let pres_fac = 1i64 + iter as i64;
        // Seed perturbs tie-breaks per iteration.
        let tie_seed = opts
            .seed
            .wrapping_mul(0xBF58_476D_1CE4_E5B9)
            .wrapping_add(iter as u64);

        let mut routes: Vec<Vec<WireId>> = Vec::with_capacity(nets.len());
        let mut failed: Option<RouteError> = None;

        for &ni in &net_order {
            let (name, src, sinks) = &resolved[ni];
            // Route to each sink; merge into a tree by reusing existing route wires.
            // Simple approach: route src->sink independently, then union.
            // (Steiner tree optimization is future work; correctness first.)
            let mut net_wires: Vec<WireId> = Vec::new();
            let mut seen = vec![false; n_wires];
            // Wires this net already contributes to `pres` this
            // iteration (its earlier sink legs). A* excludes them
            // from the presence cost: sharing its own trunk is free.
            let mut own = vec![false; n_wires];
            for &sink in sinks {
                match astar(
                    wires,
                    &mut scratch,
                    *src,
                    sink,
                    &hist,
                    &pres,
                    &own,
                    pres_fac,
                    tie_seed,
                    &protected,
                    sinks,
                ) {
                    Some(path) => {
                        for &w in &path {
                            let wi = w as usize;
                            if !seen[wi] {
                                seen[wi] = true;
                                net_wires.push(w);
                                // Count EVERY wire this net uses toward
                                // present congestion, terminals
                                // included. A wire is one physical
                                // conductor driven by one mux: a net
                                // that sinks at a wire (a FF D pin
                                // whose site pin stitches to BYPASS_W1)
                                // and a net that passes through the
                                // same wire each drive it through a
                                // different PIP, and bitgen rejects the
                                // second PIP (MustBeZero) even though
                                // the old code counted pres=1 and
                                // reported overused=0. The old code
                                // excluded src/sinks here, so that
                                // sink/through sharing was invisible
                                // to PathFinder. Each net still counts
                                // a wire once (`seen`), and `own`
                                // still exempts a net from its own
                                // wires, so multi-sink trunks stay
                                // free for their own net. Forced
                                // sharing stays impossible to clear:
                                // two nets that must sink at the same
                                // wire (the unrepaired counter's BQ
                                // and BQ2 both sink at BYPASS_W1)
                                // still time out loudly, as the
                                // router test requires.
                                pres[wi] = pres[wi].saturating_add(1);
                                own[wi] = true;
                            }
                        }
                    }
                    None => {
                        let (sx, sy) = wires.wire_xy(*src);
                        let (dx, dy) = wires.wire_xy(sink);
                        failed = Some(RouteError {
                            net: name.clone(),
                            location: format!(
                                "A* exhausted: src wire {} at ({},{}), sink wire {} at ({},{})",
                                src, sx, sy, sink, dx, dy
                            ),
                            kind: RouteErrorKind::NoPath,
                        });
                        break;
                    }
                }
            }
            if failed.is_some() {
                break;
            }
            routes.push(net_wires);
        }

        if let Some(e) = failed {
            return Err(e);
        }

        // Reorder routes back to net index order for stable output.
        let mut ordered: Vec<Vec<WireId>> = vec![Vec::new(); nets.len()];
        for (pos, &ni) in net_order.iter().enumerate() {
            ordered[ni] = std::mem::take(&mut routes[pos]);
        }

        let overused = pres.iter().filter(|c| **c > 1).count() as u32;
        let wire_count: u64 = ordered.iter().map(|r| r.len() as u64).sum();
        if route_debug {
            let mut over_list: Vec<(WireId, u32)> = pres
                .iter()
                .enumerate()
                .filter(|(_, c)| **c > 1)
                .map(|(i, &c)| (i as WireId, c))
                .collect();
            over_list.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
            let over_str: Vec<String> = over_list
                .iter()
                .map(|&(w, c)| {
                    let nm = wires
                        .wire_info(w)
                        .map(|i| i.name.to_string())
                        .unwrap_or_else(|| "?".to_string());
                    format!("{w}:{nm}x{c}(hist={})", hist[w as usize])
                })
                .collect();
            eprintln!(
                "route_debug iter={iter} pres_fac={pres_fac} overused={overused} wires=[{}]",
                over_str.join(",")
            );
            for (ni, route) in ordered.iter().enumerate() {
                let names: Vec<String> = route
                    .iter()
                    .map(|w| {
                        wires
                            .wire_info(*w)
                            .map(|i| i.name.to_string())
                            .unwrap_or_else(|| format!("{w}"))
                    })
                    .collect();
                eprintln!(
                    "route_debug iter={iter} net={} len={} path=[{}]",
                    nets[ni].name,
                    route.len(),
                    names.join(",")
                );
            }
        }
        if overused < best_overused || (overused == best_overused && wire_count < best_wire_count) {
            best_overused = overused;
            best_wire_count = wire_count;
            best_routes = ordered;
            best_pres.copy_from_slice(&pres);
        }
        if overused == 0 {
            break;
        }
        // Update historic congestion on overused nodes (N48): the
        // history cost grows by the present-use count, not by 1.
        // The first net in the fixed order routes with presence 0,
        // so history is the ONLY signal that ever moves it off a
        // contested wire, and it must outgrow that net's detour
        // length (tens of wires) before the net detours. Growing
        // by the use count doubles the pressure on the tail wires
        // every net pair shares (c = 2) and quadruples it on the
        // iter-0 trunk wires four nets share, so the grind that
        // took 34 iterations on counter12 seed 0 finishes sooner.
        // History still only grows on overused wires, so a wire
        // one net uses keeps history 0.
        for (i, &c) in pres.iter().enumerate() {
            if c > 1 {
                hist[i] = hist[i].saturating_add(c);
            }
        }
    }

    if best_overused > 0 {
        // Find the most congested location for the error report.
        let mut worst_net = String::new();
        let mut worst_loc = String::new();
        let mut worst_c = 0u32;
        for (ni, route) in best_routes.iter().enumerate() {
            for &w in route {
                let c = best_pres[w as usize];
                if c > worst_c {
                    worst_c = c;
                    worst_net = nets[ni].name.clone();
                    let (x, y) = wires.wire_xy(w);
                    worst_loc = format!("wire {} at tile ({},{})", w, x, y);
                }
            }
        }
        return Err(RouteError {
            net: worst_net,
            location: format!(
                "{} shared by {} nets after {} iters",
                worst_loc, worst_c, iters_used
            ),
            kind: RouteErrorKind::OveruseTimeout,
        });
    }

    Ok(UltraRouted {
        routes: best_routes,
        iters_used,
        overused_nodes: 0,
    })
}

#[cfg(test)]
mod tests;

pub mod mock;
