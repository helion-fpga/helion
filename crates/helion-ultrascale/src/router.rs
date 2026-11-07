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
use std::collections::BinaryHeap;

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
fn astar(
    wires: &dyn WireDb,
    scratch: &mut AstarScratch,
    src: WireId,
    dst: WireId,
    hist: &[u32],
    pres: &[u32],
    pres_fac: i64,
    tie_seed: u64,
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
        for &next in wires.fanout(w) {
            let ni = next as usize;
            if ni >= n {
                continue;
            }
            // PathFinder cost: base 1 + hist + pres * pres_fac.
            // Skip src/dst terminals in congestion accounting (handled by caller).
            let cost = 1i64 + hist[ni] as i64 + pres[ni] as i64 * pres_fac;
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
            let w = wires.wire_for_bel_pin(*site, bel, pin).ok_or_else(|| RouteError {
                net: net.name.clone(),
                location: format!("sink pin {}.{}.{} has no wire", site.x, bel, pin),
                kind: RouteErrorKind::NoPath,
            })?;
            sink_wires.push(w);
        }
        resolved.push((net.name.clone(), src_wire, sink_wires));
    }

    let mut hist = vec![0u32; n_wires];
    let mut pres = vec![0u32; n_wires];
    let mut scratch = AstarScratch::new(n_wires);

    let max_iters = opts.max_iters.max(1);
    let mut best_overused = u32::MAX;
    let mut best_wire_count = u64::MAX;
    let mut best_routes: Vec<Vec<WireId>> = vec![Vec::new(); nets.len()];
    let mut best_pres: Vec<u32> = vec![0; n_wires];
    let mut iters_used = 0u32;

    // Seed-derived net processing order. Same every iteration for stability;
    // seed variation comes from order + tie-breaks + cost perturbation.
    let net_order = seeded_order(nets.len(), opts.seed);

    for iter in 0..max_iters {
        iters_used = iter + 1;
        pres.fill(0);
        let pres_fac = 1i64 + iter as i64;
        // Seed perturbs tie-breaks per iteration.
        let tie_seed = opts.seed.wrapping_mul(0xBF58_476D_1CE4_E5B9).wrapping_add(iter as u64);

        let mut routes: Vec<Vec<WireId>> = Vec::with_capacity(nets.len());
        let mut failed: Option<RouteError> = None;

        for &ni in &net_order {
            let (name, src, sinks) = &resolved[ni];
            // Route to each sink; merge into a tree by reusing existing route wires.
            // Simple approach: route src->sink independently, then union.
            // (Steiner tree optimization is future work; correctness first.)
            let mut net_wires: Vec<WireId> = Vec::new();
            let mut seen = vec![false; n_wires];
            for &sink in sinks {
                match astar(wires, &mut scratch, *src, sink, &hist, &pres, pres_fac, tie_seed) {
                    Some(path) => {
                        for &w in &path {
                            let wi = w as usize;
                            if !seen[wi] {
                                seen[wi] = true;
                                net_wires.push(w);
                                // Count intermediate nodes toward present congestion.
                                // Terminals (src/sink of THIS net) do not congest
                                // themselves. Each net counts a wire once.
                                if w != *src && !sinks.contains(&w) {
                                    pres[wi] = pres[wi].saturating_add(1);
                                }
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
        if overused < best_overused
            || (overused == best_overused && wire_count < best_wire_count)
        {
            best_overused = overused;
            best_wire_count = wire_count;
            best_routes = ordered;
            best_pres.copy_from_slice(&pres);
        }
        if overused == 0 {
            break;
        }
        // Update historic congestion on overused nodes.
        for (i, &c) in pres.iter().enumerate() {
            if c > 1 {
                hist[i] = hist[i].saturating_add(1);
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
            location: format!("{} shared by {} nets after {} iters", worst_loc, worst_c, iters_used),
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
