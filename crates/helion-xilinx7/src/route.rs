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
    /// Dedicated carry cascade (COUT → CIN of the slice above). No fabric
    /// pips: CIN has no pip path (proven by the empty CIN pip set in the
    /// CLBLM DB), so this net must never emit pips.
    Cascade,
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

    // Carry endpoints (ZB-04). The placer co-places each bit S-LUT and FF
    // in the group slice. The s-net endpoints below are the LUT-output
    // to carry-S legs (dedicated site-internal path, pin S{j}); the
    // S-LUT *input* legs live on the Q nets via the placed LUT input
    // endpoints, and those route through fabric (the oracle feeds every
    // counter S input from INT). DI gets no endpoints: it reads the
    // S-LUT O5 internally by mux default. Head CI arrives at the slice
    // AX pin through fabric; chained CI joins its co net (dedicated
    // cascade, classified in the net loop).
    let carry_site: HashMap<&str, &str> = placed
        .carries
        .iter()
        .map(|c| (c.name.as_str(), c.site.as_str()))
        .collect();
    let mut co_nets: HashSet<&str> = HashSet::new();
    for carry in &packed.carries {
        if let Some(co) = &carry.co_net {
            co_nets.insert(co.as_str());
        }
    }
    for carry in &packed.carries {
        let site = carry_site.get(carry.name.as_str()).copied().unwrap_or("");
        for j in 0..4 {
            if let Some(o) = &carry.o_nets[j] {
                add_ep(o.clone(), site.to_string(), &format!("O{j}"), true);
            }
            if let Some(s) = &carry.s_nets[j] {
                add_ep(s.clone(), site.to_string(), &format!("S{j}"), false);
            }
        }
        if let Some(co) = &carry.co_net {
            add_ep(co.clone(), site.to_string(), "CO", true);
        }
        if carry.cyinit_c0 {
            // Chain head with CYINIT from config (golden PRECYINIT.C0):
            // no net routes to CI; the pin stays unconnected.
        } else if co_nets.contains(carry.ci_net.as_str()) {
            add_ep(carry.ci_net.clone(), site.to_string(), "CI", false);
        } else {
            // Chain head: CYINIT arrives at AX through fabric (legacy
            // const-LUT heads; the carry flow now uses C0 instead).
            add_ep(carry.ci_net.clone(), site.to_string(), "AX", false);
        }
    }

    let mut nets = Vec::new();
    // Wire ownership across nets in this run: (tile, wire) -> net.
    // A net that enters another net's wire would short against it or
    // double-drive a mux, so BFS routes around claimed wires and fails
    // loudly when no path remains. Power rails stay shareable (they
    // are excluded from claims at commit time).
    let mut claimed: HashMap<(String, String), String> = HashMap::new();
    let mut net_names: Vec<String> = net_eps.keys().cloned().collect();
    net_names.sort();
    for net in net_names {
        let eps = &net_eps[&net];
        if co_nets.contains(net.as_str()) {
            // Dedicated cascade: exactly one CO driver plus one CI load.
            // Any fabric fanout is a broken chain, not a route job.
            let ok = eps.len() == 2
                && eps.iter().any(|e| e.pin == "CO" && e.is_driver)
                && eps.iter().any(|e| e.pin == "CI" && !e.is_driver);
            if !ok {
                return Err(format!(
                    "route: cascade net `{net}` must be exactly CO-driver plus CI-load"
                ));
            }
            nets.push(RoutedNet7 { net, kind: RouteKind::Cascade });
            continue;
        }
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
        let kind = route_net(&net, drivers[0], &loads, device, &claimed)?;
        if let RouteKind::Pips(pips) = &kind {
            for p in pips {
                for w in [&p.src_wire, &p.dst_wire] {
                    if is_power_wire(w) {
                        continue;
                    }
                    claimed
                        .entry((p.tile.clone(), w.clone()))
                        .or_insert_with(|| net.clone());
                }
            }
        }
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

/// Slice LUT input pins (`A1`-`A6` through `D1`-`D6`): loads here
/// always route through fabric, never site-internal.
fn is_lut_input_pin(pin: &str) -> bool {
    let b = pin.as_bytes();
    b.len() == 2 && (b'A'..=b'D').contains(&b[0]) && b[1].is_ascii_digit()
}

/// Route one net from its driver to all loads.
///
/// `claimed` maps wires to the nets that already own them; BFS routes
/// around foreign-owned wires (see [`bfs_route`]).
fn route_net(
    net: &str,
    driver: &Endpoint,
    loads: &[&Endpoint],
    device: &dyn RouteQuery,
    claimed: &HashMap<(String, String), String>,
) -> Result<RouteKind, String> {
    // Split loads into site-internal (same site as driver and not a
    // LUT input) and external. Slice LUT inputs (A1-A6 and friends)
    // always arrive through fabric IMUX, even from the same slice:
    // 7-series has no site-local LUT-input path, and the oracle feeds
    // every counter S input from INT. Routing one through the
    // site-internal shortcut would silently unconnect it on silicon.
    let (internal, external): (Vec<&&Endpoint>, Vec<&&Endpoint>) = loads
        .iter()
        .partition(|l| l.site == driver.site && !is_lut_input_pin(&l.pin));
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
    // Tieoff nets (driven by power) and clock-path nets (into BUFG I
    // pins or slice CLK pins) may use power and clock wires; every
    // other net stays on signal wires.
    let allow_power = is_power_wire(&src_wire)
        || external_loads
            .iter()
            .any(|l| matches!(l.pin.as_str(), "I0" | "I1" | "CLK"));
    for load in &external_loads {
        let sinks = device.wire_for_site_pin(&load.site, &load.pin);
        if sinks.is_empty() {
            return Err(format!(
                "route: no wire for load {}.{} (net `{net}`)",
                load.site, load.pin
            ));
        }
        let pips = bfs_route(&src_tile, &src_wire, &sinks, device, allow_power, net, claimed)
            .ok_or_else(|| format!("route: net `{net}` unroutable to {}.{} (src {src_tile}.{src_wire})", load.site, load.pin))?;
        all_pips.extend(pips);
    }
    // Dedup shared pips.
    all_pips.sort_by(|a, b| (&a.tile, &a.dst_wire, &a.src_wire).cmp(&(&b.tile, &b.dst_wire, &b.src_wire)));
    all_pips.dedup_by(|a, b| a.tile == b.tile && a.dst_wire == b.dst_wire && a.src_wire == b.src_wire && a.is_pseudo == b.is_pseudo);
    Ok(RouteKind::Pips(all_pips))
}

/// Power and clock-distribution wires (`GND_WIRE`, `VCC_WIRE`, `GFAN*`,
/// `GCLK_*`): never signal carriers. Routing a signal through one
/// either shorts it against its tieoff driver or parks the load on a
/// constant (the oracle's S inputs read live Q copies, never ground).
fn is_power_wire(wire: &str) -> bool {
    wire == "GND_WIRE"
        || wire == "VCC_WIRE"
        || wire.starts_with("GFAN")
        || wire.starts_with("GCLK_")
}

/// Clock-bypass mux outputs (`BYP_ALT*`): owned by clock arrival (the
/// uniform BYP_ALT from GFAN program) and the clock input corridor.
/// A signal net that selects one forces arrival emit to drop the GFAN
/// program for the whole tile, so only tieoff and clock-path nets
/// (the `allow_power` exemption) may enter them.
fn is_byp_wire(wire: &str) -> bool {
    wire.starts_with("BYP_ALT")
}

/// Another net already owns this wire: entering it would short the two
/// nets (or double-drive a mux). Same-net sharing across loads of one
/// net stays legal.
fn owned_elsewhere(
    claimed: &HashMap<(String, String), String>,
    net: &str,
    tile: &str,
    wire: &str,
) -> bool {
    claimed
        .get(&(tile.to_string(), wire.to_string()))
        .map(|owner| owner != net)
        .unwrap_or(false)
}

/// BFS from `(src_tile, src_wire)` to any of `sinks: [(tile, wire)]`.
/// Returns the pip path in source→sink order. With `allow_power`
/// false, power wires are never entered: tieoff nets (driven by power)
/// and clock-path nets set it, every other net stays on signal wires.
fn bfs_route(
    src_tile: &str,
    src_wire: &str,
    sinks: &[(String, String)],
    device: &dyn RouteQuery,
    allow_power: bool,
    net: &str,
    claimed: &HashMap<(String, String), String>,
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
            if !allow_power && is_power_wire(&dst_wire) {
                continue;
            }
            if !allow_power && is_byp_wire(&dst_wire) {
                continue;
            }
            if owned_elsewhere(claimed, net, &tile, &dst_wire) {
                continue;
            }
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
            if !allow_power && is_power_wire(&nwire) {
                continue;
            }
            if !allow_power && is_byp_wire(&nwire) {
                continue;
            }
            if owned_elsewhere(claimed, net, &ntile, &nwire) {
                continue;
            }
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
    use crate::pack::{IobDir, Packed7, PackedCarry, PackedFf, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedCarry, PlacedFf, PlacedIob, PlacedLut};

    struct FakeRouter;
    impl RouteQuery for FakeRouter {
        fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
            // Tiny graph: INT_X0: A -> B -> {C, E} ; INT_X1: C -> {D, E},
            // plus a power-wire trap: A -> GFAN0 first (order must not
            // matter), GFAN0 stitched across, GFAN0 -> D. The E
            // branches give later nets room around earlier claims.
            match (tile, wire) {
                ("INT_X0", "A") => vec![
                    ("GFAN0".into(), "A".into(), false),
                    ("B".into(), "A".into(), false),
                ],
                ("INT_X0", "B") => vec![
                    ("C".into(), "B".into(), false),
                    ("E".into(), "B".into(), false),
                ],
                ("INT_X1", "C") => vec![
                    ("D".into(), "C".into(), false),
                    ("E".into(), "C".into(), false),
                ],
                ("INT_X1", "GFAN0") => vec![("D".into(), "GFAN0".into(), false)],
                _ => vec![],
            }
        }
        fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
            match (tile, wire) {
                ("INT_X0", "C") => vec![("INT_X1".into(), "C".into())],
                ("INT_X0", "GFAN0") => vec![("INT_X1".into(), "GFAN0".into())],
                _ => vec![],
            }
        }
        fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)> {
            match (site, pin) {
                ("SLICE_X0", "AMUX") => Some(("INT_X0".into(), "B".into())),
                ("SLICE_X1", "AMUX") => Some(("INT_X1".into(), "C".into())),
                ("SLICE_X0", "AQ") => Some(("INT_X0".into(), "A".into())),
                _ => None,
            }
        }
        fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)> {
            match (site, pin) {
                ("IOB_X1", "O") => vec![("INT_X1".into(), "D".into())],
                ("SLICE_X0", "AX") => vec![("INT_X1".into(), "D".into())],
                ("SLICE_X0", "A6") => vec![("INT_X0".into(), "E".into())],
                ("SLICE_X1", "A6") => vec![("INT_X1".into(), "E".into())],
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
            carry_anchor: None,
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

    /// Two chained groups with co-placed FFs/S-LUTs: co net takes the
    /// dedicated cascade (no pips), S/O nets stay site-internal, head CI
    /// routes through fabric to AX. FF clocks reuse o0 (fixture keeps the
    /// driver count exact without a BUFG).
    fn placed_carry_chain() -> Placed7 {
        let packed = Packed7 {
            luts: vec![
                PackedLut {
                    name: "u_one".into(), init: u64::MAX,
                    input_nets: [None, None, None, None, None, None],
                    output_net: "one".into(),
                },
                PackedLut {
                    name: "u_s0".into(), init: 1,
                    input_nets: [None, None, None, None, None, None],
                    output_net: "s0".into(),
                },
                PackedLut {
                    name: "u_s1".into(), init: 1,
                    input_nets: [None, None, None, None, None, None],
                    output_net: "s1".into(),
                },
            ],
            ffs: vec![
                PackedFf {
                    name: "u_ff0".into(), d_net: "o0".into(), q_net: "q0".into(),
                    clk_net: "o0".into(), ce_net: None, sr_net: None,
                },
                PackedFf {
                    name: "u_ff1".into(), d_net: "o1".into(), q_net: "q1".into(),
                    clk_net: "o1".into(), ce_net: None, sr_net: None,
                },
            ],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![
                PackedCarry {
                    name: "u_c0".into(), ci_net: "one".into(), cyinit_c0: false, co_net: Some("co0".into()),
                    s_nets: [Some("s0".into()), None, None, None],
                    di_nets: [None, None, None, None],
                    o_nets: [Some("o0".into()), None, None, None],
                },
                PackedCarry {
                    name: "u_c1".into(), ci_net: "co0".into(), cyinit_c0: false, co_net: None,
                    s_nets: [Some("s1".into()), None, None, None],
                    di_nets: [None, None, None, None],
                    o_nets: [Some("o1".into()), None, None, None],
                },
            ],
            carry_anchor: None,
        };
        Placed7 {
            packed,
            luts: vec![
                PlacedLut { name: "u_one".into(), init: u64::MAX, tile: "CLB_X1".into(), site: "SLICE_X1".into(), bel: "A6LUT".into() },
                PlacedLut { name: "u_s0".into(), init: 1, tile: "CLB_X0".into(), site: "SLICE_X0".into(), bel: "A6LUT".into() },
                PlacedLut { name: "u_s1".into(), init: 1, tile: "CLB_X1".into(), site: "SLICE_X1".into(), bel: "A6LUT".into() },
            ],
            ffs: vec![
                PlacedFf { name: "u_ff0".into(), tile: "CLB_X0".into(), site: "SLICE_X0".into(), bel: "AFF".into() },
                PlacedFf { name: "u_ff1".into(), tile: "CLB_X1".into(), site: "SLICE_X1".into(), bel: "AFF".into() },
            ],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![
                PlacedCarry { name: "u_c0".into(), tile: "CLB_X0".into(), site: "SLICE_X0".into() },
                PlacedCarry { name: "u_c1".into(), tile: "CLB_X1".into(), site: "SLICE_X1".into() },
            ],
        }
    }

    fn kind_of(routed: &Routed7, net: &str) -> String {
        match &routed.nets.iter().find(|n| n.net == net).unwrap().kind {
            RouteKind::SiteInternal => "internal".into(),
            RouteKind::Cascade => "cascade".into(),
            RouteKind::ClockSpine => "spine".into(),
            RouteKind::Pips(p) => format!("pips{}", p.len()),
        }
    }

    #[test]
    fn carry_chain_routes_cascade_plus_internal_plus_ax() {
        let routed = route(&placed_carry_chain(), &FakeRouter).unwrap();
        assert_eq!(kind_of(&routed, "co0"), "cascade");
        assert_eq!(kind_of(&routed, "o0"), "internal");
        assert_eq!(kind_of(&routed, "o1"), "internal");
        assert_eq!(kind_of(&routed, "s0"), "internal");
        assert_eq!(kind_of(&routed, "s1"), "internal");
        // Head CI: one-LUT at SLICE_X1 to AX at SLICE_X0, one pip.
        assert_eq!(kind_of(&routed, "one"), "pips1");
        // No fabric pips anywhere on carry-internal nets.
        for n in &routed.nets {
            if matches!(n.kind, RouteKind::Cascade | RouteKind::SiteInternal) {
                continue;
            }
            assert_eq!(n.net, "one", "only the head CI routes, got {}", n.net);
        }
    }

    #[test]
    fn bfs_avoids_power_wires() {
        let sinks = vec![("INT_X1".into(), "D".into())];
        let empty: HashMap<(String, String), String> = HashMap::new();
        // Restricted: the GFAN0 shortcut is skipped despite coming
        // first; the signal path A -> B -> C -> D goes out.
        let clean = bfs_route("INT_X0", "A", &sinks, &FakeRouter, false, "n", &empty).unwrap();
        assert_eq!(clean.len(), 3, "got {clean:?}");
        assert!(
            clean.iter().all(|p| !is_power_wire(&p.dst_wire) && !is_power_wire(&p.src_wire)),
            "power wire used, got {clean:?}"
        );
        // Exempted (tieoff and clock-path nets): the shortcut is taken.
        let exempt = bfs_route("INT_X0", "A", &sinks, &FakeRouter, true, "n", &empty).unwrap();
        assert!(
            exempt.iter().any(|p| p.dst_wire == "GFAN0" || p.src_wire == "GFAN0"),
            "shortcut not taken, got {exempt:?}"
        );
    }

    #[test]
    fn bfs_avoids_byp_wires() {
        // BYP_ALT muxes belong to clock arrival (the uniform BYP_ALT
        // from GFAN program) and the clock input corridor: signal nets
        // must never select them, or arrival emit has to drop the GFAN
        // program for the whole tile (ZB-10 oracle: golden chain tiles
        // program all eight BYP_ALT from GFAN, signals stay off).
        struct BypTrap;
        impl RouteQuery for BypTrap {
            fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
                // Shortcut A -> BYP_ALT0 -> D comes first (order must
                // not matter); the signal path A -> B -> C -> D is
                // longer but the only legal one for signals.
                match (tile, wire) {
                    ("INT_X0", "A") => vec![
                        ("BYP_ALT0".into(), "A".into(), false),
                        ("B".into(), "A".into(), false),
                    ],
                    ("INT_X0", "B") => vec![("C".into(), "B".into(), false)],
                    ("INT_X1", "C") => vec![("D".into(), "C".into(), false)],
                    ("INT_X1", "BYP_ALT0") => vec![("D".into(), "BYP_ALT0".into(), false)],
                    _ => vec![],
                }
            }
            fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
                match (tile, wire) {
                    ("INT_X0", "C") => vec![("INT_X1".into(), "C".into())],
                    ("INT_X0", "BYP_ALT0") => vec![("INT_X1".into(), "BYP_ALT0".into())],
                    _ => vec![],
                }
            }
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
        let sinks = vec![("INT_X1".into(), "D".into())];
        let empty: HashMap<(String, String), String> = HashMap::new();
        // Restricted: the BYP_ALT0 shortcut is skipped despite coming
        // first; the signal path A -> B -> C -> D goes out.
        let clean = bfs_route("INT_X0", "A", &sinks, &BypTrap, false, "n", &empty).unwrap();
        assert_eq!(clean.len(), 3, "got {clean:?}");
        assert!(
            clean.iter().all(|p| !is_byp_wire(&p.dst_wire) && !is_byp_wire(&p.src_wire)),
            "BYP wire used, got {clean:?}"
        );
        // Exempted (tieoff and clock-path nets): the shortcut is taken.
        let exempt = bfs_route("INT_X0", "A", &sinks, &BypTrap, true, "n", &empty).unwrap();
        assert!(
            exempt.iter().any(|p| p.dst_wire == "BYP_ALT0" || p.src_wire == "BYP_ALT0"),
            "shortcut not taken, got {exempt:?}"
        );
    }

    #[test]
    fn bfs_avoids_foreign_claimed_wires() {
        let sinks = vec![("INT_X1".into(), "D".into())];
        let mut claimed: HashMap<(String, String), String> = HashMap::new();
        claimed.insert(("INT_X0".into(), "B".into()), "other".into());
        // Foreign-owned B blocks the only signal path: no route, so
        // the caller fails loudly instead of shorting two nets.
        assert!(
            bfs_route("INT_X0", "A", &sinks, &FakeRouter, false, "mine", &claimed).is_none(),
            "routed through foreign wire"
        );
        // Same-net ownership stays legal across loads of one net.
        claimed.insert(("INT_X0".into(), "B".into()), "mine".into());
        let pips = bfs_route("INT_X0", "A", &sinks, &FakeRouter, false, "mine", &claimed).unwrap();
        assert_eq!(pips.len(), 3, "got {pips:?}");
    }

    #[test]
    fn lut_input_load_routes_through_fabric() {
        let mut placed = placed_carry_chain();
        // S-LUT u_s0 senses the co-sited FF output q0 on I5: the A6
        // load must route through fabric (7-series has no site-local
        // LUT-input path), while the D load next door stays internal.
        placed.packed.luts[1].input_nets[5] = Some("q0".into());
        let routed = route(&placed, &FakeRouter).unwrap();
        assert_eq!(kind_of(&routed, "q0"), "pips2");
        assert_eq!(kind_of(&routed, "o0"), "internal");
    }

    #[test]
    fn lut_input_pin_rule() {
        assert!(is_lut_input_pin("A6"));
        assert!(is_lut_input_pin("B1"));
        assert!(is_lut_input_pin("D6"));
        assert!(!is_lut_input_pin("D"));
        assert!(!is_lut_input_pin("AQ"));
        assert!(!is_lut_input_pin("CE"));
        assert!(!is_lut_input_pin("SR"));
        assert!(!is_lut_input_pin("AX"));
        assert!(!is_lut_input_pin("AMUX"));
        assert!(!is_lut_input_pin("S0"));
    }

    #[test]
    fn cascade_net_with_fabric_fanout_fails_loudly() {
        let mut p = placed_carry_chain();
        // A LUT sensing the co net: cascade fanout is illegal.
        p.packed.luts.push(PackedLut {
            name: "u_probe".into(), init: 1,
            input_nets: [Some("co0".into()), None, None, None, None, None],
            output_net: "probe".into(),
        });
        p.luts.push(PlacedLut { name: "u_probe".into(), init: 1, tile: "CLB_X1".into(), site: "SLICE_X1".into(), bel: "B6LUT".into() });
        let err = route(&p, &FakeRouter).unwrap_err();
        assert!(err.contains("co0") && err.contains("cascade"), "got: {err}");
    }
}
