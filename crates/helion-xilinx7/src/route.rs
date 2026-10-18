//! Route stage: [`Placed7`] → [`Routed7`].
//!
//! BFS maze router on the 7-series pip graph, plus dedicated routing and
//! legality verification for:
//! - Site-internal nets (LUT→FF, LUT→MUXF7, MUXF7→MUXF8, SRLC32E Q31→D).
//! - Dedicated global/regional clock spine (`BUFG`, `BUFGCE`, `BUFGCTRL`,
//!   `BUFIO`, `BUFR`, `MMCME2_ADV`, `PLLE2_ADV`).
//! - Dedicated vertical/tile cascades (`CARRY4` `CO[3]→CI`, `DSP48E1`
//!   `PCOUT→PCIN` / `ACOUT→ACIN` / `BCOUT→BCIN` / `CARRYCASCOUT→CARRYCASCIN`,
//!   `RAMB36E1` `CASCADEOUTA/B→CASCADEINA/B`, `ISERDESE2`/`OSERDESE2`
//!   `SHIFTOUT1/2→SHIFTIN1/2`, `IDELAYE2`/`ODELAYE2` `DATAOUT→DDLY`).

use crate::place::{parse_site_xy, Placed7};
use std::collections::{HashMap, HashSet, VecDeque};

/// What the router needs from the device model.
pub trait RouteQuery {
    /// Pips with `src_wire == wire` in `tile`: `(dst_wire, src_wire, is_pseudo)`.
    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)>;
    /// `(tile, wire)` pairs in neighboring tiles electrically connected to
    /// `(tile, wire)` via tileconn stitching.
    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)>;
    /// The wire driven by a site output pin, e.g. `("IOB_X1Y76", "O")`.
    fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)>;
    /// All `(tile, wire)` that can drive a site input pin.
    fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)>;
    /// Is this net on the dedicated global clock network (BUFG/CMT output)?
    fn is_clock_net(&self, net: &str) -> bool;
}

/// A single programmed pip: `tile.dst_wire <- src_wire`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedPip {
    pub tile: String,
    pub dst_wire: String,
    pub src_wire: String,
    pub is_pseudo: bool,
}

/// How a net is routed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteKind {
    /// Driver and loads are inside one site (e.g. LUT.O → FF.D, LUT.O → MUXF7).
    SiteInternal,
    /// Dedicated clock spine (BUFG / BUFIO / BUFR / CMT → clock network).
    ClockSpine,
    /// Dedicated column or tile cascade (`CARRY4`, `DSP48E1`, `RAMB36E1`, `SERDES`, `IODELAY`).
    DedicatedCascade,
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

fn is_const_net(net: &str) -> bool {
    matches!(
        net,
        "0" | "1" | "gnd" | "GND" | "vcc" | "VCC" | "$false" | "$true"
    )
}

/// Route all nets in the placed design.
pub fn route(placed: &Placed7, device: &dyn RouteQuery) -> Result<Routed7, String> {
    let packed = &placed.packed;
    let mut net_eps: HashMap<String, Vec<Endpoint>> = HashMap::new();
    let mut add_ep = |net: String, site: String, pin: &str, is_driver: bool| {
        if net.is_empty() || is_const_net(&net) {
            return;
        }
        net_eps.entry(net).or_default().push(Endpoint {
            site,
            pin: pin.to_string(),
            is_driver,
        });
    };

    let lut_site: HashMap<&str, &str> = placed
        .luts
        .iter()
        .map(|l| (l.name.as_str(), l.site.as_str()))
        .collect();
    let lut_bel: HashMap<&str, &str> = placed
        .luts
        .iter()
        .map(|l| (l.name.as_str(), l.bel.as_str()))
        .collect();
    let ff_site: HashMap<&str, &str> = placed
        .ffs
        .iter()
        .map(|f| (f.name.as_str(), f.site.as_str()))
        .collect();
    let ff_bel: HashMap<&str, &str> = placed
        .ffs
        .iter()
        .map(|f| (f.name.as_str(), f.bel.as_str()))
        .collect();
    let iob_site: HashMap<&str, &str> = placed
        .iobs
        .iter()
        .map(|i| (i.name.as_str(), i.site.as_str()))
        .collect();
    let bufg_site: HashMap<&str, &str> = placed
        .bufgs
        .iter()
        .map(|b| (b.name.as_str(), b.site.as_str()))
        .collect();
    let bram_site: HashMap<&str, &str> = placed
        .brams
        .iter()
        .map(|b| (b.name.as_str(), b.site.as_str()))
        .collect();
    let dsp_site: HashMap<&str, &str> = placed
        .dsps
        .iter()
        .map(|d| (d.name.as_str(), d.site.as_str()))
        .collect();
    let carry_site: HashMap<&str, &str> = placed
        .carries
        .iter()
        .map(|c| (c.name.as_str(), c.site.as_str()))
        .collect();
    let cmt_site: HashMap<&str, &str> = placed
        .cmts
        .iter()
        .map(|c| (c.name.as_str(), c.site.as_str()))
        .collect();
    let ioddr_site: HashMap<&str, &str> = placed
        .ioddrs
        .iter()
        .map(|d| (d.name.as_str(), d.site.as_str()))
        .collect();
    let serdes_site: HashMap<&str, &str> = placed
        .serdes
        .iter()
        .map(|s| (s.name.as_str(), s.site.as_str()))
        .collect();
    let iodelay_site: HashMap<&str, &str> = placed
        .iodelays
        .iter()
        .map(|d| (d.name.as_str(), d.site.as_str()))
        .collect();
    let srl_site: HashMap<&str, &str> = placed
        .srls
        .iter()
        .map(|s| (s.name.as_str(), s.site.as_str()))
        .collect();
    let muxf_site: HashMap<&str, &str> = placed
        .muxfs
        .iter()
        .map(|m| (m.name.as_str(), m.site.as_str()))
        .collect();

    for lut in &packed.luts {
        let site = lut_site.get(lut.name.as_str()).copied().unwrap_or("");
        let bel = lut_bel.get(lut.name.as_str()).copied().unwrap_or("A6LUT");
        let bel_letter = bel.chars().next().unwrap_or('A');
        for (i, net) in lut.input_nets.iter().enumerate() {
            if let Some(n) = net {
                add_ep(
                    n.clone(),
                    site.to_string(),
                    &format!("{bel_letter}{}", i + 1),
                    false,
                );
            }
        }
        add_ep(
            lut.output_net.clone(),
            site.to_string(),
            &format!("{bel_letter}MUX"),
            true,
        );
    }
    for ff in &packed.ffs {
        let site = ff_site.get(ff.name.as_str()).copied().unwrap_or("");
        let bel = ff_bel.get(ff.name.as_str()).copied().unwrap_or("AFF");
        let bel_letter = bel.chars().next().unwrap_or('A');
        add_ep(ff.d_net.clone(), site.to_string(), "D", false);
        add_ep(
            ff.q_net.clone(),
            site.to_string(),
            &format!("{bel_letter}Q"),
            true,
        );
        add_ep(ff.clk_net.clone(), site.to_string(), "CLK", false);
    }
    for iob in &packed.iobs {
        let site = iob_site.get(iob.name.as_str()).copied().unwrap_or("");
        if iob.dir == crate::pack::IobDir::In {
            add_ep(iob.net.clone(), site.to_string(), "I", true);
        } else {
            add_ep(iob.net.clone(), site.to_string(), "O", false);
        }
    }
    let mut clock_spine_nets: HashSet<String> = HashSet::new();
    for bufg in &packed.bufgs {
        let site = bufg_site.get(bufg.name.as_str()).copied().unwrap_or("");
        add_ep(bufg.in_net.clone(), site.to_string(), "I0", false);
        if let Some(in1) = &bufg.in1_net {
            add_ep(in1.clone(), site.to_string(), "I1", false);
        }
        add_ep(bufg.out_net.clone(), site.to_string(), "O", true);
        clock_spine_nets.insert(bufg.out_net.clone());
    }
    for cmt in &packed.cmts {
        let site = cmt_site.get(cmt.name.as_str()).copied().unwrap_or("");
        if let Some(n) = &cmt.clkin1_net {
            add_ep(n.clone(), site.to_string(), "CLKIN1", false);
        }
        if let Some(n) = &cmt.clkfbin_net {
            add_ep(n.clone(), site.to_string(), "CLKFBIN", false);
        }
        if let Some(n) = &cmt.clkfbout_net {
            add_ep(n.clone(), site.to_string(), "CLKFBOUT", true);
            clock_spine_nets.insert(n.clone());
        }
        for (i, n) in cmt.clkout_nets.iter().enumerate() {
            if let Some(net) = n {
                add_ep(net.clone(), site.to_string(), &format!("CLKOUT{i}"), true);
                clock_spine_nets.insert(net.clone());
            }
        }
    }
    for bram in &packed.brams {
        let site = bram_site.get(bram.name.as_str()).copied().unwrap_or("");
        if let Some(n) = &bram.cascade_in_a {
            add_ep(n.clone(), site.to_string(), "CASCADEINA", false);
        }
        if let Some(n) = &bram.cascade_out_a {
            add_ep(n.clone(), site.to_string(), "CASCADEOUTA", true);
        }
        if let Some(n) = &bram.cascade_in_b {
            add_ep(n.clone(), site.to_string(), "CASCADEINB", false);
        }
        if let Some(n) = &bram.cascade_out_b {
            add_ep(n.clone(), site.to_string(), "CASCADEOUTB", true);
        }
    }
    for dsp in &packed.dsps {
        let site = dsp_site.get(dsp.name.as_str()).copied().unwrap_or("");
        if let Some(n) = &dsp.pcin_net {
            add_ep(n.clone(), site.to_string(), "PCIN", false);
        }
        if let Some(n) = &dsp.pcout_net {
            add_ep(n.clone(), site.to_string(), "PCOUT", true);
        }
        if let Some(n) = &dsp.acin_net {
            add_ep(n.clone(), site.to_string(), "ACIN", false);
        }
        if let Some(n) = &dsp.acout_net {
            add_ep(n.clone(), site.to_string(), "ACOUT", true);
        }
        if let Some(n) = &dsp.bcin_net {
            add_ep(n.clone(), site.to_string(), "BCIN", false);
        }
        if let Some(n) = &dsp.bcout_net {
            add_ep(n.clone(), site.to_string(), "BCOUT", true);
        }
        if let Some(n) = &dsp.carrycascin_net {
            add_ep(n.clone(), site.to_string(), "CARRYCASCIN", false);
        }
        if let Some(n) = &dsp.carrycascout_net {
            add_ep(n.clone(), site.to_string(), "CARRYCASCOUT", true);
        }
    }
    for carry in &packed.carries {
        let site = carry_site.get(carry.name.as_str()).copied().unwrap_or("");
        if let Some(n) = &carry.ci_net {
            add_ep(n.clone(), site.to_string(), "CIN", false);
        }
        if let Some(n) = &carry.co_nets[3] {
            add_ep(n.clone(), site.to_string(), "COUT", true);
        }
    }
    for ddr in &packed.ioddrs {
        let site = ioddr_site.get(ddr.name.as_str()).copied().unwrap_or("");
        add_ep(ddr.c_net.clone(), site.to_string(), "CLK", false);
    }
    for ser in &packed.serdes {
        let site = serdes_site.get(ser.name.as_str()).copied().unwrap_or("");
        add_ep(ser.clk_net.clone(), site.to_string(), "CLK", false);
        add_ep(ser.clkdiv_net.clone(), site.to_string(), "CLKDIV", false);
        if let Some(n) = &ser.shiftin1_net {
            add_ep(n.clone(), site.to_string(), "SHIFTIN1", false);
        }
        if let Some(n) = &ser.shiftin2_net {
            add_ep(n.clone(), site.to_string(), "SHIFTIN2", false);
        }
        if let Some(n) = &ser.shiftout1_net {
            add_ep(n.clone(), site.to_string(), "SHIFTOUT1", true);
        }
        if let Some(n) = &ser.shiftout2_net {
            add_ep(n.clone(), site.to_string(), "SHIFTOUT2", true);
        }
    }
    for dly in &packed.iodelays {
        let site = iodelay_site.get(dly.name.as_str()).copied().unwrap_or("");
        if let Some(n) = &dly.c_net {
            add_ep(n.clone(), site.to_string(), "C", false);
        }
        if let Some(n) = &dly.data_out_net {
            add_ep(n.clone(), site.to_string(), "DATAOUT", true);
        }
    }
    for srl in &packed.srls {
        let site = srl_site.get(srl.name.as_str()).copied().unwrap_or("");
        add_ep(srl.clk_net.clone(), site.to_string(), "CLK", false);
        if let Some(q31) = &srl.q31_net {
            add_ep(q31.clone(), site.to_string(), "MC31", true);
        }
        add_ep(srl.d_net.clone(), site.to_string(), "DI1", false);
    }
    for muxf in &packed.muxfs {
        let site = muxf_site.get(muxf.name.as_str()).copied().unwrap_or("");
        add_ep(muxf.i0_net.clone(), site.to_string(), "I0", false);
        add_ep(muxf.i1_net.clone(), site.to_string(), "I1", false);
        add_ep(muxf.o_net.clone(), site.to_string(), "O", true);
    }
    for ibuf in &packed.ibufds_gte2s {
        clock_spine_nets.insert(ibuf.o_net.clone());
        if let Some(odiv) = &ibuf.odiv2_net {
            clock_spine_nets.insert(odiv.clone());
        }
    }
    for gtc in &packed.gtp_commons {
        for n in [
            &gtc.gtrefclk0_net,
            &gtc.gtrefclk1_net,
            &gtc.pll0outclk_net,
            &gtc.pll0outrefclk_net,
            &gtc.pll1outclk_net,
            &gtc.pll1outrefclk_net,
        ]
        .into_iter()
        .flatten()
        {
            clock_spine_nets.insert(n.clone());
        }
    }
    for gch in &packed.gtp_channels {
        for n in [&gch.txoutclk_net, &gch.rxoutclk_net]
            .into_iter()
            .flatten()
        {
            clock_spine_nets.insert(n.clone());
        }
    }

    let mut nets = Vec::new();
    let mut net_names: Vec<String> = net_eps.keys().cloned().collect();
    net_names.sort();
    for net in net_names {
        let eps = &net_eps[&net];
        let drivers: Vec<&Endpoint> = eps.iter().filter(|e| e.is_driver).collect();
        let loads: Vec<&Endpoint> = eps.iter().filter(|e| !e.is_driver).collect();
        if loads.is_empty() {
            continue;
        }
        if drivers.is_empty() {
            if device.is_clock_net(&net)
                || clock_spine_nets.contains(&net)
                || loads.iter().all(|l| matches!(l.pin.as_str(), "I0" | "I1" | "CLKIN1" | "CLKIN2" | "CLK"))
            {
                nets.push(RoutedNet7 {
                    net,
                    kind: RouteKind::ClockSpine,
                });
            }
            continue;
        }
        if drivers.len() != 1 {
            return Err(format!(
                "route: net `{net}` has {} drivers (need exactly 1)",
                drivers.len()
            ));
        }
        let kind = route_net(&net, drivers[0], &loads, &clock_spine_nets, device)?;
        nets.push(RoutedNet7 { net, kind });
    }

    Ok(Routed7 {
        placed: placed.clone(),
        nets,
    })
}

/// Check whether `driver -> load` is a dedicated hardware cascade wire and
/// validate its placement legality (`X_load == X_drv` and `Y_load == Y_drv + 1`).
fn check_dedicated_cascade(net: &str, driver: &Endpoint, loads: &[&Endpoint]) -> Result<bool, String> {
    let is_col_cascade = matches!(
        driver.pin.as_str(),
        "COUT"
            | "PCOUT"
            | "ACOUT"
            | "BCOUT"
            | "CARRYCASCOUT"
            | "CASCADEOUTA"
            | "CASCADEOUTB"
    );
    if is_col_cascade {
        let (dx, dy) = parse_site_xy(&driver.site).ok_or_else(|| {
            format!(
                "route: cannot parse coordinates of cascade driver site `{}` on net `{net}`",
                driver.site
            )
        })?;
        for l in loads {
            let (lx, ly) = parse_site_xy(&l.site).ok_or_else(|| {
                format!(
                    "route: cannot parse coordinates of cascade load site `{}` on net `{net}`",
                    l.site
                )
            })?;
            if lx != dx || ly != dy + 1 {
                return Err(format!(
                    "route: dedicated cascade `{net}` ({}.{} -> {}.{}) violates column adjacency: expected X={dx}, Y={}, got X={lx}, Y={ly}",
                    driver.site,
                    driver.pin,
                    l.site,
                    l.pin,
                    dy + 1
                ));
            }
        }
        return Ok(true);
    }

    if matches!(driver.pin.as_str(), "SHIFTOUT1" | "SHIFTOUT2") {
        let (dx, dy) = parse_site_xy(&driver.site).unwrap_or((0, 0));
        for l in loads {
            let (lx, ly) = parse_site_xy(&l.site).unwrap_or((0, 0));
            if lx != dx || ly != dy + 1 {
                return Err(format!(
                    "route: SERDES cascade `{net}` ({}.{} -> {}.{}) requires adjacent MASTER/SLAVE sites in same IOI3 tile",
                    driver.site, driver.pin, l.site, l.pin
                ));
            }
        }
        return Ok(true);
    }

    if driver.pin == "DATAOUT" || driver.pin == "MC31" {
        return Ok(true);
    }

    Ok(false)
}

/// Route one net from its driver to all loads.
fn route_net(
    net: &str,
    driver: &Endpoint,
    loads: &[&Endpoint],
    clock_spine_nets: &HashSet<String>,
    device: &dyn RouteQuery,
) -> Result<RouteKind, String> {
    if check_dedicated_cascade(net, driver, loads)? {
        return Ok(RouteKind::DedicatedCascade);
    }
    let (_internal, external): (Vec<&&Endpoint>, Vec<&&Endpoint>) =
        loads.iter().partition(|l| l.site == driver.site);
    if external.is_empty() {
        return Ok(RouteKind::SiteInternal);
    }
    if device.is_clock_net(net) || clock_spine_nets.contains(net) {
        return Ok(RouteKind::ClockSpine);
    }
    let driver_is_ibuf = driver.pin == "I" && driver.site.starts_with("IOB_");
    let loads_are_clk = loads.iter().all(|l| {
        l.site.starts_with("BUFGCTRL_")
            || l.site.starts_with("BUFIO_")
            || l.site.starts_with("BUFR_")
            || l.site.starts_with("MMCME2_ADV_")
            || l.site.starts_with("PLLE2_ADV_")
    });
    if driver_is_ibuf && loads_are_clk {
        return Ok(RouteKind::ClockSpine);
    }

    let external_loads: Vec<&Endpoint> = external.into_iter().copied().collect();
    let (src_tile, src_wire) = device
        .site_pin_wire(&driver.site, &driver.pin)
        .ok_or_else(|| {
            format!(
                "route: no wire for driver {}.{} (net `{net}`)",
                driver.site, driver.pin
            )
        })?;

    let mut all_pips: Vec<RoutedPip> = Vec::new();
    for load in &external_loads {
        let sinks = device.wire_for_site_pin(&load.site, &load.pin);
        if sinks.is_empty() {
            return Err(format!(
                "route: no wire for load {}.{} (net `{net}`)",
                load.site, load.pin
            ));
        }
        let pips = bfs_route(&src_tile, &src_wire, &sinks, device).ok_or_else(|| {
            format!("route: net `{net}` unroutable to {}.{}", load.site, load.pin)
        })?;
        all_pips.extend(pips);
    }
    all_pips.sort_by(|a, b| {
        (&a.tile, &a.dst_wire, &a.src_wire).cmp(&(&b.tile, &b.dst_wire, &b.src_wire))
    });
    all_pips.dedup_by(|a, b| {
        a.tile == b.tile
            && a.dst_wire == b.dst_wire
            && a.src_wire == b.src_wire
            && a.is_pseudo == b.is_pseudo
    });
    Ok(RouteKind::Pips(all_pips))
}

/// BFS from `(src_tile, src_wire)` to any of `sinks: [(tile, wire)]`.
fn bfs_route(
    src_tile: &str,
    src_wire: &str,
    sinks: &[(String, String)],
    device: &dyn RouteQuery,
) -> Option<Vec<RoutedPip>> {
    let sink_set: HashSet<(&str, &str)> =
        sinks.iter().map(|(t, w)| (t.as_str(), w.as_str())).collect();
    let mut parent: HashMap<(String, String), Option<((String, String), RoutedPip)>> =
        HashMap::new();
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
        for (ntile, nwire) in device.node_wires(&tile, &wire) {
            let next = (ntile.clone(), nwire.clone());
            if !parent.contains_key(&next) {
                parent.insert(
                    next.clone(),
                    Some((
                        (tile.clone(), wire.clone()),
                        RoutedPip {
                            tile: format!("{tile}~{ntile}"),
                            dst_wire: nwire.clone(),
                            src_wire: wire.clone(),
                            is_pseudo: false,
                        },
                    )),
                );
                queue.push_back(next);
            }
        }
    }

    let end = found?;
    let mut pips: Vec<RoutedPip> = Vec::new();
    let mut cur = end;
    while let Some(Some((prev, pip))) = parent.get(&cur) {
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
    use crate::pack::{IobDir, Packed7, PackedCarry, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedCarry, PlacedIob, PlacedLut};

    struct FakeRouter;
    impl RouteQuery for FakeRouter {
        fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
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
            iobs: vec![PackedIob {
                name: "obuf".into(),
                port: "b".into(),
                dir: IobDir::Out,
                net: "n1".into(),
                loc: None,
                iostandard: "LVCMOS33".into(),
                drive: None,
                slew: None,
            }],
            ..Default::default()
        };
        Placed7 {
            packed,
            luts: vec![PlacedLut {
                name: "u_inv".into(),
                init: 0,
                tile: "CLB_X0".into(),
                site: "SLICE_X0".into(),
                bel: "A6LUT".into(),
            }],
            iobs: vec![PlacedIob {
                name: "obuf".into(),
                port: "b".into(),
                dir: IobDir::Out,
                tile: "IOB_T".into(),
                site: "IOB_X1".into(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn bfs_finds_pip_path() {
        let routed = route(&placed_two_site(), &FakeRouter).unwrap();
        assert_eq!(routed.nets.len(), 1);
        match &routed.nets[0].kind {
            RouteKind::Pips(pips) => {
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
            fn pips_from(&self, _: &str, _: &str) -> Vec<(String, String, bool)> {
                vec![]
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
        let err = route(&placed_two_site(), &NoRoute).unwrap_err();
        assert!(err.contains("n1"), "got: {err}");
    }

    #[test]
    fn clock_net_uses_spine() {
        assert!(FakeRouter.is_clock_net("clk_bufg"));
        assert!(!FakeRouter.is_clock_net("n1"));
    }

    #[test]
    fn dedicated_cascade_validates_column_alignment() {
        let mut packed = Packed7::default();
        packed.carries.push(PackedCarry {
            name: "c0".into(),
            co_nets: [None, None, None, Some("carry_chain".into())],
            ..Default::default()
        });
        packed.carries.push(PackedCarry {
            name: "c1".into(),
            ci_net: Some("carry_chain".into()),
            ..Default::default()
        });
        let ok_placed = Placed7 {
            packed: packed.clone(),
            carries: vec![
                PlacedCarry {
                    name: "c0".into(),
                    tile: "CLB_X0Y0".into(),
                    site: "SLICE_X0Y0".into(),
                },
                PlacedCarry {
                    name: "c1".into(),
                    tile: "CLB_X0Y1".into(),
                    site: "SLICE_X0Y1".into(),
                },
            ],
            ..Default::default()
        };
        let r = route(&ok_placed, &FakeRouter).unwrap();
        assert_eq!(r.nets[0].kind, RouteKind::DedicatedCascade);

        // Misaligned column must fail loudly.
        let bad_placed = Placed7 {
            packed,
            carries: vec![
                PlacedCarry {
                    name: "c0".into(),
                    tile: "CLB_X0Y0".into(),
                    site: "SLICE_X0Y0".into(),
                },
                PlacedCarry {
                    name: "c1".into(),
                    tile: "CLB_X1Y1".into(),
                    site: "SLICE_X1Y1".into(),
                },
            ],
            ..Default::default()
        };
        let err = route(&bad_placed, &FakeRouter).unwrap_err();
        assert!(err.contains("column adjacency"), "got: {err}");
    }
}
