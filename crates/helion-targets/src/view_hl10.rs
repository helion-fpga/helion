//! Fill an [`ImplView`] from an HL10 (Helion's own fabric) implementation
//! run: the routed design of `helion-pack` / `helion-place` /
//! `helion-route` on a HAD device model.
//!
//! - **Tiles**: one per HAD site; a CLB holds `n_ble` logic elements.
//! - **Cells**: each cluster's LUT and flop on its CLB and BLE; pads,
//!   multipliers and block RAMs on their sites.
//! - **Nets**: one per driver of the packed connection list, with the
//!   interconnect wires the router put it on; a net with no wire is a
//!   direct neighbour connection.
//! - **Usage**: occupied logic elements per CLB; interconnect wires in use
//!   per tile by direction against `TRACKS` per direction.
//! - **Timing**: the run's routed analysis (`helion-sta`), every endpoint.
//!
//! HL10 has no clock regions and Helion keeps no board pin file for it;
//! both are named in [`ImplView::missing`].
//!
//! No em dashes anywhere.

use std::collections::{BTreeMap, HashMap, HashSet};

use helion_device::tracks as tk;
use helion_device::{Device, Site, SiteKind};
use helion_ir::{Design, resolve};
use helion_pack::NetSrc;
use helion_route::Routed;

use crate::view::{
    CellPlacement, ClockNet, ClockUtilization, EndpointSlack, ImplView, IoPin, IoPlan, Overflow, PowerBlock, PowerEstimate, Provenance, ResourceRow, RoutedNet, Stage, TileClass, TileUsage, TimingPath,
    TimingSummary, ViewTile, WireHop,
};
use crate::{Family, view_drc, view_fill};

const SCOPE: &str = "routed design of this run";

/// What the run computed beside the routed design.
pub struct Hl10Reports<'a> {
    pub timing: Option<&'a helion_sta::TimingResult>,
    pub clocks: &'a [helion_sta::Clock],
    pub power: Option<&'a helion_sta::PowerReport>,
}

/// The view of a routed HL10 run.
pub fn from_routed(design: &Design, routed: &Routed, dev: &Device, rep: &Hl10Reports) -> ImplView {
    let mut v = ImplView::empty(Family::Helion, dev.part.clone(), Stage::Routed);
    let placed = &routed.placed;
    let k = &placed.packed;
    let conn = &k.conn;
    let tr = &routed.tracks;

    // Tiles: one per position. The left pad column shares its positions
    // with the clock spine; such a tile holds both sites and is named for
    // the pad.
    let mut at: BTreeMap<(u32, u32), ViewTile> = BTreeMap::new();
    for s in dev.clb_sites().chain(dev.iob_sites_all()).chain(dev.clk_sites()).chain(dev.dsp_sites()).chain(dev.bram_sites()) {
        let id = s.id();
        let class = match s.kind {
            SiteKind::Clb => TileClass::Logic,
            SiteKind::Iob => TileClass::Io,
            SiteKind::Clk => TileClass::Clock,
            SiteKind::Dsp => TileClass::Dsp,
            SiteKind::Bram => TileClass::Ram,
        };
        let t = at.entry((s.x, s.y)).or_insert_with(|| ViewTile { x: s.x, y: s.y, class, name: id.clone(), clock_region: String::new(), sites: Vec::new(), io_bank: None });
        if s.kind == SiteKind::Clb {
            t.sites.extend((0..dev.n_ble).map(|b| format!("{id}/BLE{b}")));
        } else {
            t.sites.push(id);
        }
        if s.kind == SiteKind::Iob {
            t.io_bank = dev.iob_bank(s.x, s.y);
        }
    }
    v.tiles = at.into_values().collect();
    let tile_names: HashMap<(u32, u32), String> = v.tiles.iter().map(|t| ((t.x, t.y), t.name.clone())).collect();
    let tile_id = |x: u32, y: u32| tile_names.get(&(x, y)).cloned().unwrap_or_else(|| format!("X{x}Y{y}"));
    v.cols = v.tiles.iter().map(|t| t.x + 1).max().unwrap_or(0);
    v.rows = v.tiles.iter().map(|t| t.y + 1).max().unwrap_or(0);
    v.row0_top = false;
    v.set_provenance("tiles", Provenance::new("helion-device HAD", "one tile per HAD site position (CLB, the pad ring, CLK spine, DSP, BRAM); a CLB's sites are its logic elements; IO bank from the HAD pad table", dev.part.clone()));
    v.mark_missing("clock-regions", format!("HL10 has no clock regions: each of its {} clock lines runs the whole fabric", helion_device::clocks::LINES));

    // Placed cells.
    let mut cell = |name: &str, site: String, tile: &Site, bel: Option<&str>, kind: &str| {
        if !name.is_empty() {
            v.cells.push(CellPlacement { cell: name.to_string(), site, bel: bel.map(str::to_string), tile: Some(tile_id(tile.x, tile.y)), kind: kind.to_string() });
        }
    };
    for (lf, (site, ble)) in k.lutffs.iter().zip(&placed.lutff_sites) {
        let s = format!("{}/BLE{ble}", site.id());
        cell(&lf.lut_cell, s.clone(), site, Some("LUT"), "LUT");
        cell(&lf.ff_cell, s, site, Some("FF"), "FF");
    }
    for (io, site) in k.iobs.iter().zip(&placed.iob_sites) {
        cell(&io.cell, site.id(), site, None, "IOB");
    }
    let pad_of_input: HashMap<u32, Site> = tr.iob_in.iter().map(|i| (i.input, Site { x: i.x, y: i.y, kind: SiteKind::Iob })).collect();
    for (i, inp) in conn.inputs.iter().enumerate() {
        if let Some(site) = pad_of_input.get(&(i as u32)) {
            cell(&format!("{}$pad", inp.net), site.id(), site, None, "IOB");
        }
    }
    for (m, site) in k.macs.iter().zip(&placed.mac_sites) {
        cell(&m.cell, site.id(), site, None, "MAC27");
    }
    for (b, site) in k.brams.iter().zip(&placed.bram_sites) {
        cell(&b.cell, site.id(), site, None, "RAM18");
    }
    v.set_provenance("placement", Provenance::new("helion-place", "each cluster's LUT and flop on its CLB and logic element; output pads, input pads, multipliers and block RAMs on their HAD sites", SCOPE));

    // Nets: one per driver of the connection list.
    let name_of = |s: NetSrc| -> (String, String) {
        match s {
            NetSrc::BleQ(i) => k.lutffs.get(i as usize).map(|l| (l.q_net.clone(), format!("{}.Q", l.ff_cell))).unwrap_or_default(),
            NetSrc::BleO(i) => k.lutffs.get(i as usize).map(|l| (format!("{}.O", l.lut_cell), format!("{}.O", l.lut_cell))).unwrap_or_default(),
            NetSrc::Input(i) => conn.inputs.get(i as usize).map(|p| (p.net.clone(), format!("{}$pad.O", p.net))).unwrap_or_default(),
        }
    };
    let mut loads: HashMap<NetSrc, Vec<String>> = HashMap::new();
    for pc in &conn.pins {
        if let Some(l) = k.lutffs.get(pc.lutff as usize) {
            loads.entry(pc.src).or_default().push(format!("{}.I{}", l.lut_cell, pc.pin));
        }
    }
    for (cell, src) in &conn.iob_src {
        loads.entry(*src).or_default().push(format!("{cell}.I"));
    }
    let mut hops: HashMap<NetSrc, Vec<WireHop>> = HashMap::new();
    for w in &tr.net_wires {
        let dir = ["N", "E", "S", "W"].get(w.dir as usize).copied().unwrap_or("?");
        hops.entry(w.src).or_default().push(WireHop { tile: tile_id(w.x, w.y), wire: format!("{dir}{}", w.track), pip: None, is_pseudo: false });
    }
    let mut srcs: Vec<NetSrc> = loads.keys().copied().collect();
    srcs.sort_by_key(|s| name_of(*s).0);
    let mut nets = Vec::new();
    for s in srcs {
        let (net, driver) = name_of(s);
        let h = hops.remove(&s).unwrap_or_default();
        nets.push(RoutedNet { net, driver: Some(driver), loads: loads.remove(&s).unwrap_or_default(), kind: if h.is_empty() { "direct" } else { "pips" }, hops: h });
    }
    if tr.active {
        v.nets = nets;
        v.set_provenance(
            "routing",
            Provenance::new("helion-route tracks", "one net per driver of the packed connection list (a flop output, a LUT output, an input pad); hops are the interconnect wires PathFinder put the net on (tile, direction and track); a net with no wire is a direct neighbour or same-CLB connection", SCOPE),
        );
    } else {
        v.mark_missing("routing", "this design was not routed from its full connection list, so the router kept no per-net wires");
    }

    // Usage.
    let mut site_use: HashMap<&str, HashSet<&str>> = HashMap::new();
    for c in &v.cells {
        if let Some(t) = &c.tile {
            site_use.entry(t.as_str()).or_default().insert(c.site.as_str());
        }
    }
    let mut wires: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    for w in &tr.net_wires {
        let e = wires.entry((w.x, w.y)).or_default();
        if w.dir as u32 == tk::E || w.dir as u32 == tk::W { e.0 += 1 } else { e.1 += 1 }
    }
    let mut usage = Vec::new();
    for t in &v.tiles {
        // The router's grid: the CLB array and the ring of pad tiles around it.
        let routable = tr.active && matches!(t.class, TileClass::Logic | TileClass::Io);
        let (uh, uv) = wires.get(&(t.x, t.y)).copied().unwrap_or((0, 0));
        usage.push(TileUsage {
            x: t.x,
            y: t.y,
            sites_used: site_use.get(t.name.as_str()).map(|s| s.len() as u32).unwrap_or(0),
            sites_capacity: t.sites.len() as u32,
            // shortcut: every routable tile is given both directions' full track count; an edge tile has fewer wires. Upgrade when the router exports its wire list per tile.
            overflow: routable.then_some(Overflow { h_used: uh, h_capacity: 2 * tk::TRACKS, v_used: uv, v_capacity: 2 * tk::TRACKS }),
        });
    }
    v.usage = usage;
    v.set_provenance(
        "usage",
        Provenance::new("helion-route tracks", format!("occupied logic elements and pads per tile; interconnect wires a net occupies in the tile, east/west and north/south apart, against {} wires per direction pair ({} tracks each way); a wire carries one net, so a routed design is never over capacity", 2 * tk::TRACKS, tk::TRACKS), SCOPE),
    );
    if tr.active {
        view_fill::fill_congestion(&mut v, "helion-route tracks", crate::view_xilinx7::CONGESTION_THRESHOLD_PCT);
    }

    // Utilization rows.
    let row = |name: &str, used: usize, available: u32| ResourceRow { name: name.into(), depth: 0, used: used as f64, available };
    let ble = dev.n_clb() * dev.n_ble;
    v.resources = vec![
        row("Logic element (LUT6 + flop)", k.lutffs.len(), ble),
        ResourceRow { depth: 1, ..row("LUT6", k.lutffs.iter().filter(|l| !l.lut_cell.is_empty()).count(), ble) },
        ResourceRow { depth: 1, ..row("Flip-flop", k.lutffs.iter().filter(|l| !l.ff_cell.is_empty()).count(), ble) },
        row("IOB", k.iobs.len() + pad_of_input.len(), dev.iob_sites_all().len() as u32),
        row("RAM18", k.brams.len(), dev.n_bram),
        row("MAC27", k.macs.len(), dev.n_dsp),
        row("Clock line", conn.clocks.len() + conn.resets.len(), helion_device::clocks::LINES),
    ];
    v.set_provenance("resources", Provenance::new("helion-pack", "packed clusters, pads, block RAMs and multipliers of this run against the HAD part's sites; clock lines carry clocks and asynchronous resets", SCOPE));

    // Clocks.
    let period = |net: &str| rep.clocks.iter().find(|c| c.name == net || c.source == net).map(|c| c.period_ps as u32);
    let nets: Vec<ClockNet> = conn
        .clocks
        .iter()
        .map(|c| ClockNet {
            name: c.net.clone(),
            // A single unnamed clock is the design's only clock: the first constraint is its period.
            period_ps: period(&c.net).or_else(|| (conn.clocks.len() == 1).then(|| rep.clocks.first().map(|x| x.period_ps as u32)).flatten()).unwrap_or(0),
            loads: conn.ff_clock.iter().filter(|f| f.is_some_and(|(line, _)| line == c.line)).count(),
            regions: Vec::new(),
            buffer: Some(format!("clock line {}", c.line)),
        })
        .collect();
    let clock_names: HashSet<String> = nets.iter().map(|c| c.name.clone()).collect();
    v.clocks = Some(ClockUtilization { clocks: nets, loads_by_region: BTreeMap::new(), bufgs_used: conn.clocks.len() + conn.resets.len(), bufgs_available: helion_device::clocks::LINES as usize });
    v.set_provenance("clocks", Provenance::new("helion-pack", "the design's clocks, one per fabric clock line, with the flops on each line; period from the constraint on the net (0 = unconstrained); lines in use include asynchronous resets", SCOPE));

    // Timing.
    match rep.timing.filter(|t| t.endpoints > 0) {
        Some(t) => {
            let req = t.clocks.first().map(|c| c.period_ps as i64).unwrap_or(0);
            let clock = t.clocks.first().map(|c| c.name.clone()).unwrap_or_default();
            let site_of: HashMap<&str, &str> = v.cells.iter().map(|c| (c.cell.as_str(), c.site.as_str())).collect();
            let graph = helion_sta::graph_timing_routed(design, routed, rep.clocks);
            let endpoints: Vec<EndpointSlack> = graph
                .iter()
                .flat_map(|g| &g.endpoint_arrivals)
                .map(|e| EndpointSlack { endpoint: e.cell.clone(), slack_ps: req - e.arrival_ps, clock: clock.clone(), site: site_of.get(e.cell.as_str()).map(|s| s.to_string()), x: Some(e.x), y: Some(e.y) })
                .collect();
            if graph.is_none() {
                v.mark_missing("endpoint-slack", "this design was not routed from its full connection list, so only the worst register and pad paths were timed");
            }
            let worst_path = graph.as_ref().and_then(|g| Some((g.worst_path.first()?, g.worst_path.last()?))).map(|(f, l)| TimingPath { from: f.0.clone(), to: l.0.clone(), clock: clock.clone(), slack_ps: t.wns_ps, requirement_ps: req, kind: "setup" });
            v.timing = Some(TimingSummary {
                wns_ps: t.wns_ps,
                tns_ps: t.tns_ps,
                whs_ps: Some(t.hold_slack_ps),
                ths_ps: Some(t.hold_slack_ps.min(0)),
                failing_endpoints: graph.as_ref().map(|g| g.failing).unwrap_or(usize::from(t.wns_ps < 0)),
                total_endpoints: graph.as_ref().map(|g| g.endpoints).unwrap_or(t.endpoints),
                worst_path,
                worst_hold_path: None,
                endpoints,
                fmax_mhz: (req - t.wns_ps > 0).then(|| 1.0e6 / (req - t.wns_ps) as f64),
            });
            v.set_provenance(
                "timing",
                Provenance::new(
                    "helion-sta",
                    "routed analysis with the HL10T model: every path of the packed connection list, each connection as it was routed; WNS and TNS carry the XDC adjustments (IO delays, uncertainty, exceptions), endpoint slacks are period minus arrival before them; hold is the shortest clock-to-Q plus route against the hold requirement, one figure for the design",
                    SCOPE,
                ),
            );
        }
        None => v.mark_missing("timing", if rep.clocks.is_empty() { "no clock is constrained (no create_clock)" } else { "the design has no flop, so no timed endpoint" }),
    }

    // IO.
    let attr = |port: &str, key: &str| -> Option<String> {
        let base = port.rsplit_once('_').map(|x| x.0).unwrap_or(port);
        design.ports.iter().find(|p| resolve(p.name) == port).or_else(|| design.ports.iter().find(|p| resolve(p.name) == base)).and_then(|p| p.attrs.get(key)).map(|s| s.to_string())
    };
    let pin = |port: &str, site: &Site, loc: &Option<String>, direction: &'static str| {
        let std = attr(port, "IOSTANDARD");
        IoPin {
            port: port.to_string(),
            package_pin: site.id(),
            bank: dev.iob_bank(site.x, site.y),
            iostandard_default: std.is_none(),
            iostandard: std.unwrap_or_else(|| Device::DEFAULT_IOSTANDARD.to_string()),
            direction,
            pin_default: loc.is_none(),
            clock_capable: None,
        }
    };
    let pad_net: HashMap<&str, &str> = design.nets.iter().flat_map(|n| n.endpoints.iter().filter(|e| resolve(e.pin) == "PAD").map(|e| (resolve(e.cell), resolve(n.name)))).collect();
    let mut pins: Vec<IoPin> = k.iobs.iter().zip(&placed.iob_sites).map(|(io, site)| pin(pad_net.get(io.cell.as_str()).copied().unwrap_or(io.cell.as_str()), site, &io.loc, "output")).collect();
    pins.extend(conn.inputs.iter().enumerate().filter_map(|(i, inp)| Some(pin(&inp.net, pad_of_input.get(&(i as u32))?, &inp.loc, "input"))));
    // The implicit user clock has no pad: the configuration port steps it.
    pins.extend(conn.clocks.iter().filter(|c| c.src == helion_pack::ClockSrc::Implicit).map(|c| IoPin {
        port: c.net.clone(),
        package_pin: "configuration port".into(),
        bank: None,
        iostandard: String::new(),
        direction: "input",
        iostandard_default: false,
        pin_default: false,
        clock_capable: None,
    }));
    pins.sort_by(|a, b| a.package_pin.cmp(&b.package_pin));
    v.io = Some(IoPlan { pins, package_pins: dev.iob_sites_all().len() });
    v.set_provenance("io", Provenance::new("helion-place", "placed pads by HAD IOB site (HL10 names a pin by its site); standard from the port's IOSTANDARD or the HAD default; a pad with no LOC was placed by the flow; a single clock with no pad is the implicit user clock the configuration port steps", SCOPE));
    v.mark_missing("clock-capable", "any HL10 input pad can drive a clock line (IOB_CLOCK), so there is no pin that is not clock capable");

    // Control sets: clock line and asynchronous reset line of each flop.
    let line_net = |doms: &[helion_pack::ClockDom], line: u8| doms.iter().find(|d| d.line == line).map(|d| d.net.clone()).unwrap_or_default();
    let sets: Vec<(String, String, String, String, String)> = k
        .lutffs
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.ff_cell.is_empty())
        .map(|(i, l)| {
            let clk = conn.ff_clock.get(i).copied().flatten().map(|(line, _)| line_net(&conn.clocks, line)).unwrap_or_default();
            let (rst, kind) = match conn.ff_reset.get(i).copied().flatten() {
                Some((line, _, value)) => (line_net(&conn.resets, line), if value { "async preset" } else { "async clear" }.to_string()),
                None => (String::new(), String::new()),
            };
            (clk, String::new(), rst, kind, l.ff_cell.clone())
        })
        .collect();
    view_fill::fill_control_sets(&mut v, sets.into_iter(), "helion-pack", "packed flops grouped by (clock line, asynchronous reset line); HL10 flops have no enable or synchronous reset pin, the packer folds those into the LUT");
    let names: HashSet<&str> = clock_names.iter().map(String::as_str).collect();
    view_fill::fill_high_fanout(&mut v, &names, 10, "helion-pack");

    // Power.
    match rep.power.filter(|p| p.total_uw > 0) {
        Some(p) => {
            let mw = |uw: i64| uw as f64 / 1000.0;
            let block = |name: &str, used: usize, uw: i64| PowerBlock { block: name.into(), used, total_mw: mw(uw) };
            v.power = Some(PowerEstimate {
                total_mw: mw(p.total_uw),
                static_mw: mw(p.static_uw),
                dynamic_mw: mw(p.dynamic_uw),
                toggle_rate: 0.0,
                blocks: vec![
                    block("Clocks", 0, p.clocks_uw),
                    block("Logic", p.lutff, p.logic_uw),
                    block("Signals", 0, p.signals_uw),
                    block("I/O", p.iob, p.io_uw),
                    block("Block RAM", p.bram, p.bram_uw),
                    block("DSP", p.dsp, p.dsp_uw),
                    block("Device static", 0, p.static_uw),
                ],
                tiles: Vec::new(),
                method: format!("helion-sta report_power: fixed microwatts per MHz per used resource at {} MHz, scaled by V squared ({} mV) and a leakage term ({} C); no toggle rate and no per-net activity in this model", p.f_mhz, p.voltage_mv, p.temperature_c),
            });
            v.set_provenance("power", Provenance::new("helion-sta report_power", "per-class totals from the placed design's resource counts", SCOPE));
            v.mark_missing("power-tiles", "the HL10 power model totals each resource class for the whole design and attributes nothing to a tile");
        }
        None => v.mark_missing("power", "the HL10 power model returned no power: the design has no clock frequency or no placed logic"),
    }
    v.mark_missing("board", "Helion's board table (boards/*.xdc) has no HL10 board");
    view_fill::fill_schematic(
        &mut v,
        Err("the helion-sv front end that builds HL10 designs bit-blasts while it elaborates and keeps no word-level netlist"),
        design,
        |c, pins| view_fill::primitive_name(c, pins, ["HFF", "IOB", "IOB", "CLKBUF", "RAM18", "MAC27", "CARRY4"]),
        view_fill::is_output_pin,
        ("helion-sv", "helion-place"),
        SCOPE,
    );
    view_drc::fill(&mut v);
    v
}
