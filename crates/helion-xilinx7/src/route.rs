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
    /// Tie-off pips driving a BUFGCTRL site's control pins to the constants
    /// required for I0 -> O (UG472 Fig. 2-5): S0=1, CE0=1, IGNORE1=1,
    /// S1=0, CE1=0, IGNORE0=0.
    ///
    /// The static config (emit.rs) sets pin polarity, but the pins are
    /// routing inputs: left undriven, the BUFGCTRL output freezes at
    /// INIT_OUT (UG472 Table 2-3). Fails loudly when the DB has no
    /// tie-off path for the site.
    fn bufg_tieoff_pips(&self, site: &str) -> Result<BufgTieoff, String>;
    /// Clock spine stitching for a BUFGCTRL site: O exit to the lowest
    /// available global clock line, REBUF BOT to TOP stitches, and the
    /// static rebuffer enables for every CLK_BUFG_REBUF tile on that
    /// line's node. Fails loudly when the DB has no spine path.
    fn bufg_spine(&self, site: &str) -> Result<BufgSpine, String>;
}

/// Tie-off pips for one BUFGCTRL site, split by driven constant.
#[derive(Clone, Debug, Default)]
pub struct BufgTieoff {
    /// Pips driving S0, CE0, IGNORE1 to constant 1 (VCC).
    pub ones: Vec<RoutedPip>,
    /// Pips driving S1, CE1, IGNORE0 to constant 0 (GND).
    pub zeros: Vec<RoutedPip>,
}

/// Clock spine stitching for one BUFGCTRL site: O to a global clock
/// line, through the rebuffer tiles. Real pips travel the normal Pips
/// channel; static rebuffer enables travel
/// [`crate::emit::EmitDevice::bufg_spine_static`].
#[derive(Clone, Debug, Default)]
pub struct BufgSpine {
    /// Global clock index, lowest with a real O-exit pip (Vivado uses 0).
    pub gclk: u8,
    /// Real pips: O exit plus REBUF BOT to TOP stitches.
    pub pips: Vec<RoutedPip>,
    /// Static lines: `(tile, feature)` rebuffer enables.
    pub statics: Vec<(String, String)>,
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

    // BUFGCTRL control-pin tie-offs. The static config (emit.rs) sets pin
    // polarity, but S0/S1/CE0/CE1/IGNORE0/IGNORE1 are routing inputs: left
    // undriven, the BUFGCTRL output freezes at INIT_OUT with no toggle
    // (UG472 Table 2-3), the observed "done=1, LEDs dark" failure.
    route_bufg_tieoffs(placed, device, &mut nets)?;

    // Clock spine stitching per BUFGCTRL: O exit to the global line plus
    // REBUF stitches. Without these the clock never leaves the BUFG site.
    for bufg in &placed.bufgs {
        let spine = device.bufg_spine(&bufg.site)?;
        if !spine.pips.is_empty() {
            nets.push(RoutedNet7 {
                net: format!("__spine_{}", bufg.site.to_ascii_lowercase()),
                kind: RouteKind::Pips(spine.pips),
            });
        }
    }

    Ok(Routed7 {
        placed: placed.clone(),
        nets,
    })
}

/// Route constant 1/0 to every placed BUFGCTRL's control pins.
///
/// Appends one synthetic net per BUFGCTRL per constant
/// (`__tieoff_<site>_1`, `__tieoff_<site>_0`). Fails loudly on any
/// conflict: a tie-off IMUX/GFAN wire already used by other logic would
/// short the constant (or be shorted by it).
fn route_bufg_tieoffs(
    placed: &Placed7,
    device: &dyn RouteQuery,
    nets: &mut Vec<RoutedNet7>,
) -> Result<(), String> {
    // Wires already used by routed pips, per tile (rails excluded:
    // VCC_WIRE/GND_WIRE are always-on and shareable).
    let mut used: HashSet<(String, String)> = HashSet::new();
    for net in nets.iter() {
        if let RouteKind::Pips(pips) = &net.kind {
            for p in pips {
                for w in [&p.src_wire, &p.dst_wire] {
                    if w != "VCC_WIRE" && w != "GND_WIRE" {
                        used.insert((p.tile.clone(), w.clone()));
                    }
                }
            }
        }
    }
    for bufg in &placed.bufgs {
        let tieoff = device.bufg_tieoff_pips(&bufg.site)?;
        let mut claim = |pips: &[RoutedPip]| -> Result<(), String> {
            for p in pips {
                for w in [&p.src_wire, &p.dst_wire] {
                    if w == "VCC_WIRE" || w == "GND_WIRE" {
                        continue;
                    }
                    let key = (p.tile.clone(), w.clone());
                    if used.contains(&key) {
                        return Err(format!(
                            "route: BUFGCTRL {} tie-off conflicts: wire {}.{} already used",
                            bufg.site, p.tile, w
                        ));
                    }
                }
            }
            for p in pips {
                for w in [&p.src_wire, &p.dst_wire] {
                    if w != "VCC_WIRE" && w != "GND_WIRE" {
                        used.insert((p.tile.clone(), w.clone()));
                    }
                }
            }
            Ok(())
        };
        claim(&tieoff.ones)?;
        claim(&tieoff.zeros)?;
        let tag = bufg.site.to_ascii_lowercase();
        if !tieoff.ones.is_empty() {
            nets.push(RoutedNet7 {
                net: format!("__tieoff_{tag}_1"),
                kind: RouteKind::Pips(tieoff.ones),
            });
        }
        if !tieoff.zeros.is_empty() {
            nets.push(RoutedNet7 {
                net: format!("__tieoff_{tag}_0"),
                kind: RouteKind::Pips(tieoff.zeros),
            });
        }
    }
    Ok(())
}

/// Route one net from its driver to all loads.
fn route_net(
    net: &str,
    driver: &Endpoint,
    loads: &[&Endpoint],
    device: &dyn RouteQuery,
) -> Result<RouteKind, String> {
    // Split loads into site-internal (same site as driver) and external.
    let (internal, external): (Vec<&&Endpoint>, Vec<&&Endpoint>) =
        loads.iter().partition(|l| l.site == driver.site);
    // If all loads are site-internal, no pips needed.
    if external.is_empty() {
        return Ok(RouteKind::SiteInternal);
    }
    // Clock spine: BUFG-driven nets use the dedicated network.
    // (The spine exit pips are emitted with the BUFG site stitching, not
    // here. The IBUF -> BUFG input path below routes through general
    // pips on purpose: Vivado drives it through CLK_BUFG input-mux
    // pips, and a bitless special case could never match that.)
    if device.is_clock_net(net) {
        return Ok(RouteKind::ClockSpine);
    }
    // Route to external loads via pips. (Internal loads are site-local.)
    let external_loads: Vec<&Endpoint> = external.into_iter().copied().collect();
    let _ = internal; // site-internal loads need no pips

    let (src_tile, src_wire) = device
        .site_pin_wire(&driver.site, &driver.pin)
        .ok_or_else(|| format!("route: no wire for driver {}.{} (net `{net}`)", driver.site, driver.pin))?;

    let mut all_pips: Vec<RoutedPip> = Vec::new();
    for load in &external_loads {
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
    all_pips.dedup_by(|a, b| a.tile == b.tile && a.dst_wire == b.dst_wire && a.src_wire == b.src_wire && a.is_pseudo == b.is_pseudo);
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
        // Through pips in this tile (including pseudo-pips: always-on
        // connections like the OLOGIC bypass, traversed but not emitted).
        for (dst_wire, src_w, is_pseudo) in device.pips_from(&tile, &wire) {
            debug_assert_eq!(src_w, wire);
            let next = (tile.clone(), dst_wire.clone());
            if !parent.contains_key(&next) {
                let pip = RoutedPip {
                    tile: tile.clone(),
                    dst_wire: dst_wire.clone(),
                    src_wire: wire.clone(),
                    is_pseudo,
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
                    is_pseudo: false,
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
        fn bufg_tieoff_pips(&self, _site: &str) -> Result<BufgTieoff, String> {
            Ok(BufgTieoff::default())
        }
        fn bufg_spine(&self, _site: &str) -> Result<BufgSpine, String> {
            Ok(BufgSpine::default())
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
            fn bufg_tieoff_pips(&self, _: &str) -> Result<BufgTieoff, String> {
                Ok(BufgTieoff::default())
            }
            fn bufg_spine(&self, _: &str) -> Result<BufgSpine, String> {
                Ok(BufgSpine::default())
            }
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
