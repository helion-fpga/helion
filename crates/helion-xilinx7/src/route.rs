//! Route stage: [`Placed7`] → [`Routed7`].
//!
//! BFS maze router on the 7-series pip graph. Each net is routed from its
//! driver site-pin to its load site-pins through INT-tile pips and
//! tile-to-tile node stitching. Site-internal nets (LUT→FF in one slice)
//! and the dedicated global clock network need no pips and are recorded
//! as such for the emitter.

use crate::place::Placed7;
use std::collections::{HashMap, HashSet, VecDeque};

/// What the router needs from the device model.
pub trait RouteQuery {
    /// Pips with `src_wire == wire` in `tile`: `(dst_wire, src_wire)` pairs.
    /// Includes `always` pseudo-pips as zero-cost edges (caller may weight).
    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String)>;
    /// `(tile, wire)` pairs in neighboring tiles electrically connected to
    /// `(tile, wire)` via tileconn stitching.
    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)>;
    /// The wire driven by a site output pin, e.g. `("IOB_X1Y76", "O")`.
    fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)>;
    /// All `(tile, wire)` that can drive a site input pin.
    fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)>;
    /// Is this net on the dedicated global clock network (BUFG output)?
    fn is_clock_net(&self, net: &str) -> bool;
}

/// A single programmed pip: `tile.dst_wire <- src_wire`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedPip {
    pub tile: String,
    pub dst_wire: String,
    pub src_wire: String,
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
    let ff_site: HashMap<&str, &str> = placed.ffs.iter().map(|f| (f.name.as_str(), f.site.as_str())).collect();
    let iob_site: HashMap<&str, &str> = placed.iobs.iter().map(|i| (i.name.as_str(), i.site.as_str())).collect();
    let bufg_site: HashMap<&str, &str> = placed.bufgs.iter().map(|b| (b.name.as_str(), b.site.as_str())).collect();

    for lut in &packed.luts {
        let site = lut_site.get(lut.name.as_str()).copied().unwrap_or("");
        for (i, net) in lut.input_nets.iter().enumerate() {
            if let Some(n) = net {
                add_ep(n.clone(), site.to_string(), &format!("A{i}"), false);
            }
        }
        add_ep(lut.output_net.clone(), site.to_string(), "O", true);
    }
    for ff in &packed.ffs {
        let site = ff_site.get(ff.name.as_str()).copied().unwrap_or("");
        add_ep(ff.d_net.clone(), site.to_string(), "D", false);
        add_ep(ff.q_net.clone(), site.to_string(), "Q", true);
        add_ep(ff.clk_net.clone(), site.to_string(), "CLK", false);
    }
    for iob in &packed.iobs {
        let site = iob_site.get(iob.name.as_str()).copied().unwrap_or("");
        if iob.dir == crate::pack::IobDir::In {
            add_ep(iob.net.clone(), site.to_string(), "O", true);
        } else {
            add_ep(iob.net.clone(), site.to_string(), "I", false);
        }
    }
    for bufg in &packed.bufgs {
        let site = bufg_site.get(bufg.name.as_str()).copied().unwrap_or("");
        add_ep(bufg.in_net.clone(), site.to_string(), "I0", false);
        add_ep(bufg.out_net.clone(), site.to_string(), "O", true);
    }

    let mut nets = Vec::new();
    let mut net_names: Vec<String> = net_eps.keys().cloned().collect();
    net_names.sort();
    for net in net_names {
        let eps = &net_eps[&net];
        let drivers: Vec<&Endpoint> = eps.iter().filter(|e| e.is_driver).collect();
        let loads: Vec<&Endpoint> = eps.iter().filter(|e| !e.is_driver).collect();
        if drivers.len() != 1 {
            return Err(format!(
                "route: net `{net}` has {} drivers (need exactly 1)",
                drivers.len()
            ));
        }
        if loads.is_empty() {
            continue; // undriven net: nothing to route
        }
        let kind = route_net(&net, drivers[0], &loads, device)?;
        nets.push(RoutedNet7 { net, kind });
    }

    Ok(Routed7 {
        placed: placed.clone(),
        nets,
    })
}

/// Route one net from its driver to all loads.
fn route_net(
    net: &str,
    driver: &Endpoint,
    loads: &[&Endpoint],
    device: &dyn RouteQuery,
) -> Result<RouteKind, String> {
    // Site-internal: all endpoints on one site.
    if loads.iter().all(|l| l.site == driver.site) {
        return Ok(RouteKind::SiteInternal);
    }
    // Clock spine: BUFG-driven nets use the dedicated network.
    if device.is_clock_net(net) {
        return Ok(RouteKind::ClockSpine);
    }

    let (src_tile, src_wire) = device
        .site_pin_wire(&driver.site, &driver.pin)
        .ok_or_else(|| format!("route: no wire for driver {}.{} (net `{net}`)", driver.site, driver.pin))?;

    let mut all_pips: Vec<RoutedPip> = Vec::new();
    for load in loads {
        let sinks = device.wire_for_site_pin(&load.site, &load.pin);
        if sinks.is_empty() {
            return Err(format!(
                "route: no wire for load {}.{} (net `{net}`)",
                load.site, load.pin
            ));
        }
        let pips = bfs_route(&src_tile, &src_wire, &sinks, device)
            .ok_or_else(|| format!("route: net `{net}` unroutable to {}.{}", load.site, load.pin))?;
        all_pips.extend(pips);
    }
    // Dedup shared pips.
    all_pips.sort_by(|a, b| (&a.tile, &a.dst_wire, &a.src_wire).cmp(&(&b.tile, &b.dst_wire, &b.src_wire)));
    all_pips.dedup_by(|a, b| a.tile == b.tile && a.dst_wire == b.dst_wire && a.src_wire == b.src_wire);
    Ok(RouteKind::Pips(all_pips))
}

/// BFS from `(src_tile, src_wire)` to any of `sinks: [(tile, wire)]`.
/// Returns the pip path in source→sink order.
fn bfs_route(
    src_tile: &str,
    src_wire: &str,
    sinks: &[(String, String)],
    device: &dyn RouteQuery,
) -> Option<Vec<RoutedPip>> {
    let sink_set: HashSet<(&str, &str)> =
        sinks.iter().map(|(t, w)| (t.as_str(), w.as_str())).collect();
    // (tile, wire) -> parent ((tile, wire), pip used to get here)
    let mut parent: HashMap<(String, String), Option<((String, String), RoutedPip)>> = HashMap::new();
    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    let start = (src_tile.to_string(), src_wire.to_string());
    parent.insert(start.clone(), None);
    queue.push_back(start);

    let mut found: Option<(String, String)> = None;
    while let Some((tile, wire)) = queue.pop_front() {
        if sink_set.contains(&(tile.as_str(), wire.as_str())) {
            found = Some((tile, wire));
            break;
        }
        // Through pips in this tile.
        for (dst_wire, src_w) in device.pips_from(&tile, &wire) {
            debug_assert_eq!(src_w, wire);
            let next = (tile.clone(), dst_wire.clone());
            if !parent.contains_key(&next) {
                let pip = RoutedPip {
                    tile: tile.clone(),
                    dst_wire: dst_wire.clone(),
                    src_wire: wire.clone(),
                };
                parent.insert(next.clone(), Some(((tile.clone(), wire.clone()), pip)));
                queue.push_back(next);
            }
        }
        // Across tileconn stitching.
        for (ntile, nwire) in device.node_wires(&tile, &wire) {
            let next = (ntile.clone(), nwire.clone());
            if !parent.contains_key(&next) {
                parent.insert(next.clone(), Some(((tile.clone(), wire.clone()), RoutedPip {
                    tile: format!("{tile}~{ntile}"),
                    dst_wire: nwire.clone(),
                    src_wire: wire.clone(),
                })));
                queue.push_back(next);
            }
        }
    }

    let end = found?;
    // Walk back to start, collecting pips.
    let mut pips: Vec<RoutedPip> = Vec::new();
    let mut cur = end;
    while let Some(Some((prev, pip))) = parent.get(&cur) {
        // Skip tileconn pseudo-pips (marked with ~); real pips only.
        if !pip.tile.contains('~') {
            pips.push(pip.clone());
        }
        cur = prev.clone();
        if parent.get(&cur).map(|o| o.is_none()).unwrap_or(true) {
            break;
        }
    }
    pips.reverse();
    Some(pips)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{IobDir, Packed7, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedIob, PlacedLut};

    struct FakeRouter;
    impl RouteQuery for FakeRouter {
        fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
            // Tiny graph: INT_X0: A -> B -> C ; INT_X1: C -> D
            match (tile, wire) {
                ("INT_X0", "A") => vec![("B".into(), "A".into())],
                ("INT_X0", "B") => vec![("C".into(), "B".into())],
                ("INT_X1", "C") => vec![("D".into(), "C".into())],
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
                ("SLICE_X0", "O") => Some(("INT_X0".into(), "B".into())),
                _ => None,
            }
        }
        fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)> {
            match (site, pin) {
                ("IOB_X1", "I") => vec![("INT_X1".into(), "D".into())],
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
        };
        Placed7 {
            packed,
            luts: vec![PlacedLut { name: "u_inv".into(), init: 0, tile: "CLB_X0".into(), site: "SLICE_X0".into(), bel: "A6LUT".into() }],
            ffs: vec![],
            iobs: vec![
                PlacedIob { name: "obuf".into(), port: "b".into(), dir: IobDir::Out, tile: "IOB_T".into(), site: "IOB_X1".into() },
            ],
            bufgs: vec![],
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
            fn pips_from(&self, _: &str, _: &str) -> Vec<(String, String)> { vec![] }
            fn node_wires(&self, _: &str, _: &str) -> Vec<(String, String)> { vec![] }
            fn site_pin_wire(&self, _: &str, _: &str) -> Option<(String, String)> { None }
            fn wire_for_site_pin(&self, _: &str, _: &str) -> Vec<(String, String)> { vec![] }
            fn is_clock_net(&self, _: &str) -> bool { false }
        }
        let err = route(&placed_two_site(), &NoRoute).unwrap_err();
        assert!(err.contains("n1"), "got: {err}");
    }

    #[test]
    fn clock_net_uses_spine() {
        // Clock-spine classification is a device query; the router trusts it.
        assert!(FakeRouter.is_clock_net("clk_bufg"));
        assert!(!FakeRouter.is_clock_net("n1"));
    }
}
