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

/// Wire classes one route search may use.
#[derive(Clone, Copy, Debug, Default)]
pub struct RouteOpts {
    /// Tie-off nets only: power wires (`VCC_WIRE`, `GND_WIRE`, `GFAN*`,
    /// `GCLK_*`) are legal carriers.
    pub allow_power: bool,
    /// The load is a slice bypass pin (`AX`..`DX`): `BYP_ALT` muxes are legal.
    pub allow_byp: bool,
    /// Stay off general interconnect (INT tiles): the dedicated clock path
    /// from a clock-capable pad to a BUFG input.
    pub no_int: bool,
    /// A BUFG output on its way to slice CLK pins: global/row clock wires
    /// are legal, general interconnect is not.
    pub clock: bool,
    /// Last resort for parts with an incomplete database: also use pips
    /// the database gives no configuration bits for.
    pub any_pip: bool,
    /// A fabric signal on its way into a global buffer input: `GCLK_*`
    /// wires are legal (power wires and pips without bits still are not).
    pub allow_gclk: bool,
    /// Congestion negotiation: wires held by other nets are legal at a
    /// cost; the caller rips up the nets the path displaces.
    pub negotiate: bool,
    /// Keep the search within this many tiles of the box spanned by the
    /// source and the sinks (0: whole device). A search that cannot succeed
    /// then fails after a small region instead of the whole device.
    pub bbox: i32,
    /// Give up rather than accept a path much longer than the direct one
    /// (the caller then negotiates for a better one).
    pub cap_detour: bool,
}

/// What the router needs from the device model.
pub trait RouteQuery {
    /// Pips with `src_wire == wire` in `tile`: `(dst_wire, src_wire, is_pseudo)`.
    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)>;
    /// `(tile, wire)` pairs in neighboring tiles electrically connected to
    /// `(tile, wire)` via tileconn stitching.
    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)>;
    /// The wire driven by a site output pin, e.g. `("IOB_X1Y76", "O")`.
    fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)>;
    /// Optional fast path for [`bfs_route`] with the same contract (same
    /// wire restrictions, same returned pips). `None`: not implemented, the
    /// caller runs the generic BFS. `Some(None)`: unroutable.
    #[allow(clippy::too_many_arguments)]
    /// Wires just claimed by `net` (`claimed_len` = size of the claim map
    /// afterwards), so a fast router can keep its own index current.
    fn note_claims(&self, _net: &str, _wires: &[(String, String)], _claimed_len: usize) {}
    /// Nets other than `net` holding a wire of `pips` (a negotiated path);
    /// the contested wires gain history cost so later searches avoid them.
    fn contested(&self, _net: &str, _pips: &[RoutedPip]) -> Vec<String> {
        Vec::new()
    }
    /// Claims were removed from the claim map: drop any derived index.
    fn release_claims(&self, _nets: &[String], _claimed_len: usize) {}
    /// The electrical node a tile wire belongs to (wires of one node are one
    /// conductor). `None`: no node model, and a ripped-up net is rerouted
    /// whole.
    fn node_key(&self, _tile: &str, _wire: &str) -> Option<u64> {
        None
    }
    /// Delay the search charges for a pip (its delay plus the fixed cost per
    /// pip), the same value it adds up into arrival times.
    fn pip_cost(&self, _tile: &str, _src: &str, _dst: &str) -> Option<u32> {
        None
    }
    /// PathFinder phase. `Some(ps)`: wires held by signal nets may be shared,
    /// and each net already on a wire's node adds `ps` to the cost of taking
    /// it. `None`: the phase is over.
    fn pf_present(&self, _ps: Option<u32>) {}
    /// Add (`1`) or remove (`-1`) one net's use of the nodes of `wires`.
    fn pf_occupy(&self, _wires: &[(String, String)], _delta: i32) {}
    /// Whether a node of `wires` has more than one net on it.
    fn pf_overused(&self, _wires: &[(String, String)]) -> bool {
        false
    }
    /// Add history cost to every overused wire; returns how many there are.
    fn pf_history(&self) -> usize {
        0
    }
    /// Returns the pips and, per pip, the arrival delay (ps) at its source
    /// and destination wires; `_tree` holds this net's wires with their
    /// known arrival delays.
    fn route_fast(
        &self,
        _src: (&str, &str),
        _tree: &[(String, String, u32)],
        _sinks: &[(String, String)],
        _opts: RouteOpts,
        _net: &str,
        _claimed: &HashMap<(String, String), String>,
    ) -> Option<Option<(Vec<RoutedPip>, Vec<(u32, u32)>)>> {
        None
    }
    /// All `(tile, wire)` that can drive a site input pin.
    fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)>;
    /// Is this net on the dedicated global clock network (BUFG/CMT output)?
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
    /// Routed interconnect delay (ps, slow corner) from each net's driver
    /// pin to each fabric load: `(net, load site, load pin) -> ps`. Filled
    /// by the delay-aware router; absent for site-internal loads.
    pub load_delays: HashMap<(String, String, String), u32>,
    /// Clock nets were routed pip by pip on the device graph (global
    /// line, HROW, BUFHCE, HCLK leaf, INT clock input); emit derives the
    /// static enables from those pips instead of the fixed spine recipe.
    pub graph_clocks: bool,
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
        // O6 leaves the slice on the letter's direct output pin (A..D), which
        // needs no OUTMUX configuration.
        add_ep(lut.output_net.clone(), site.to_string(), &bel_letter.to_string(), true);
    }
    let placed_ff: HashMap<&str, &crate::place::PlacedFf> = placed.ffs.iter().map(|f| (f.name.as_str(), f)).collect();
    for ff in &packed.ffs {
        let site = ff_site.get(ff.name.as_str()).copied().unwrap_or("");
        let bel = ff_bel.get(ff.name.as_str()).copied().unwrap_or("AFF");
        // Q is per-BEL (AQ/BQ/CQ/DQ); CLK/CE/SR are slice-shared.
        let bel_letter = bel.chars().next().unwrap_or('A');
        // D: the same-letter LUT O6 or carry sum stays inside the slice;
        // anything else arrives through the letter's bypass pin.
        let d_pin = match placed_ff.get(ff.name.as_str()).map(|p| crate::place::ff_d_source(p, placed)) {
            Some(crate::place::FfDSource::LutO6) | Some(crate::place::FfDSource::CarryXor) => format!("{bel_letter}FF_D"),
            _ => format!("{bel_letter}X"),
        };
        add_ep(ff.d_net.clone(), site.to_string(), &d_pin, false);
        add_ep(ff.q_net.clone(), site.to_string(), &format!("{bel_letter}Q"), true);
        add_ep(ff.clk_net.clone(), site.to_string(), "CLK", false);
        if let Some(ce) = &ff.ce_net {
            add_ep(ce.clone(), site.to_string(), "CE", false);
        }
        if let Some(sr) = &ff.sr_net {
            add_ep(sr.clone(), site.to_string(), "SR", false);
        }
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
        // CMT clocks route over the device graph like a pad clock into a
        // BUFG: dedicated CMT/HCLK/HROW wires (see the `no_int` loads below).
        if let Some(n) = &cmt.clkfbout_net {
            add_ep(n.clone(), site.to_string(), "CLKFBOUT", true);
        }
        for (i, n) in cmt.clkout_nets.iter().enumerate() {
            if let Some(net) = n {
                add_ep(net.clone(), site.to_string(), &format!("CLKOUT{i}"), true);
            }
        }
    }
    for (net, site, pin, is_driver) in crate::cmt::endpoints(placed) {
        add_ep(net, site, &pin, is_driver);
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
    // Block RAM and DSP data, address, clock, enable and reset pins, as site pins.
    for (net, site, pin, is_driver) in crate::bram::endpoints(placed)?.into_iter().chain(crate::dsp::endpoints(placed)).chain(crate::lutram::endpoints(placed)) {
        add_ep(net, site, &pin, is_driver);
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
        // The SRL's data enters on its LUT letter's site pin (AI..DI);
        // `DI1` is the BEL pin, which has no routing wire.
        let letter = placed
            .srls
            .iter()
            .find(|p| p.name == srl.name)
            .and_then(|p| p.bel.chars().next())
            .unwrap_or('D');
        add_ep(srl.d_net.clone(), site.to_string(), &format!("{letter}I"), false);
    }
    for muxf in &packed.muxfs {
        let site = muxf_site.get(muxf.name.as_str()).copied().unwrap_or("");
        // Inputs come from the slice's own LUTs or F7 muxes (inside the
        // slice). The output leaves on the slice pin emit selects for it:
        // F7AMUX on AMUX, F7BMUX on CMUX, F8MUX on BMUX.
        add_ep(muxf.i0_net.clone(), site.to_string(), "F7F8_I0", false);
        add_ep(muxf.i1_net.clone(), site.to_string(), "F7F8_I1", false);
        let bel = placed.muxfs.iter().find(|p| p.name == muxf.name).map(|p| p.bel.as_str()).unwrap_or("");
        let out_pin = match bel {
            "F7BMUX" => "CMUX",
            "F8MUX" => "BMUX",
            _ => "AMUX",
        };
        add_ep(muxf.o_net.clone(), site.to_string(), out_pin, true);
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

    // Carry endpoints (ZB-04). The placer co-places each bit S-LUT and FF
    // in the group slice. The s-net endpoints below are the LUT-output
    // to carry-S legs (dedicated site-internal path, pin S{j}); the
    // S-LUT *input* legs live on the Q nets via the placed LUT input
    // endpoints, and those route through fabric (the oracle feeds every
    // counter S input from INT). DI gets no endpoints and no FASM
    // lines in either bitstream: DI0 reads the undriven AX path high
    // (the blink-proven oracle needs DI0=1 for head CO0) while DI1-3
    // read low (LUTRAM MC defaults), which is what pads need to
    // transmit; sim models exactly this split. A CYINIT-config head
    // leaves CI unconnected (C0/C1 from config); chained CI joins its
    // co net (dedicated cascade, classified in the net loop).
    let carry_site: HashMap<&str, &str> = placed
        .carries
        .iter()
        .map(|c| (c.name.as_str(), c.site.as_str()))
        .collect();
    // Cascade nets: a CO3 that feeds the next CARRY4's CI. Any other CO3
    // leaves for fabric on DMUX (emit sets DOUTMUX=CY).
    let ci_nets: HashSet<&str> = packed.carries.iter().map(|c| c.ci_net.as_str()).collect();
    let co_nets: HashSet<&str> = packed
        .carries
        .iter()
        .filter_map(|c| c.co_net.as_deref())
        .filter(|co| ci_nets.contains(co))
        .collect();
    let prim = crate::pack::prim_carries(packed);
    // Nets with a driver in the packed design (the carry loop below may only
    // route a wired `DI` to a net that is really driven).
    let mut driven: HashSet<&str> = HashSet::new();
    for l in &packed.luts {
        driven.insert(l.output_net.as_str());
    }
    for f in &packed.ffs {
        driven.insert(f.q_net.as_str());
    }
    for c in &packed.carries {
        for o in c.o_nets.iter().flatten() {
            driven.insert(o.as_str());
        }
        if let Some(co) = &c.co_net {
            driven.insert(co.as_str());
        }
    }
    for carry in &packed.carries {
        let site = carry_site.get(carry.name.as_str()).copied().unwrap_or("");
        let is_prim = prim.contains(carry.name.as_str());
        for j in 0..4 {
            let letter = ['A', 'B', 'C', 'D'][j];
            if let Some(o) = &carry.o_nets[j] {
                // An adder's or primitive's sum bit leaves the slice on the
                // letter's multiplexed output (`AMUX`..`DMUX`, set to XOR).
                let pin = if is_prim || packed.adder_carries.contains(&carry.name) {
                    format!("{letter}MUX")
                } else {
                    format!("O{j}")
                };
                add_ep(o.clone(), site.to_string(), &pin, true);
            }
            if let Some(s) = &carry.s_nets[j] {
                add_ep(s.clone(), site.to_string(), &format!("S{j}"), false);
            }
            if is_prim {
                // A carry tap shares the letter's multiplexed output.
                if let Some(co) = carry.co_nets[j].as_ref().filter(|_| j < 3) {
                    if carry.o_nets[j].is_some() {
                        return Err(format!("route: CARRY4 {} uses both O{j} and CO{j}; one xMUX carries one", carry.name));
                    }
                    add_ep(co.clone(), site.to_string(), &format!("{letter}MUX"), true);
                }
                // A wired DI is the bypass input (emit leaves CARRY4.xCY0 off).
                //
                // Only if the net is actually driven. A `DI` pin tied to a
                // net that nothing drives is ignored, as it was before this
                // primitive support existed: routing it would add an `AX`
                // load to a net that had none, and the 0-driver check would
                // then reject a design that packs and places fine.
                if let Some(di) = carry.di_nets[j].as_ref().filter(|n| !is_const_net(n) && driven.contains(n.as_str())) {
                    if j == 0 && carry.cyinit_net.as_deref().is_some_and(|n| !is_const_net(n)) {
                        return Err(format!("route: CARRY4 {} needs AX for both DI0 and CYINIT (net `{}`)", carry.name, carry.cyinit_net.as_deref().unwrap_or("")));
                    }
                    add_ep(di.clone(), site.to_string(), &format!("{letter}X"), false);
                }
            }
        }
        if let Some(co) = &carry.co_net {
            if co_nets.contains(co.as_str()) {
                add_ep(co.clone(), site.to_string(), "CO", true);
            } else {
                if carry.o_nets[3].is_some() {
                    return Err(format!("route: CARRY4 {} uses both O3 and CO3 in fabric", carry.name));
                }
                add_ep(co.clone(), site.to_string(), "DMUX", true);
            }
        }
        if carry.cyinit.is_some() {
            // Chain head with CYINIT from config (emit selects C0/C1 by
            // value): no net routes to CI; the pin stays unconnected.
        } else if co_nets.contains(carry.ci_net.as_str()) {
            add_ep(carry.ci_net.clone(), site.to_string(), "CI", false);
        } else {
            // Chain head: CYINIT arrives at AX through fabric (legacy
            // const-LUT heads, or a primitive's fabric CYINIT pin).
            let n = if carry.ci_net.is_empty() { carry.cyinit_net.clone().unwrap_or_default() } else { carry.ci_net.clone() };
            add_ep(n, site.to_string(), "AX", false);
        }
    }

    for eps in net_eps.values_mut() {
        let mut seen: HashSet<(String, String, bool)> = HashSet::new();
        eps.retain(|e| seen.insert((e.site.clone(), e.pin.clone(), e.is_driver)));
    }
    let mut nets = Vec::new();
    // Wire ownership across nets in this run: (tile, wire) -> net.
    // A net that enters another net's wire would short against it or
    // double-drive a mux, so BFS routes around claimed wires and fails
    // loudly when no path remains. Power rails stay shareable (they
    // are excluded from claims at commit time).
    let mut claimed: HashMap<(String, String), String> = HashMap::new();
    let mut load_delays: HashMap<(String, String, String), u32> = HashMap::new();
    let mut graph_clocks = false;
    let mut net_names: Vec<String> = net_eps.keys().cloned().collect();
    net_names.sort();
    // Negotiated congestion: a net with no free path takes the cheapest
    // path through other nets' wires; those nets are ripped up and queued
    // again. History cost on contested wires steers later searches away.
    // Past this many rip-ups (and once every net has been routed), the
    // PathFinder phase takes the remaining congestion: on Ibex rip-up spent
    // 950 s at 2 x nets without shrinking its queue; PathFinder then took 250 s.
    let pf_after = net_names.len() / 4 + 500;
    let mut rips = 0usize;
    let total_nets = net_names.len();
    // HELION_ROUTE_RIPSTATS: with the progress line, the nets ripped up most
    // and the nets that ripped up others most.
    let rip_stats = std::env::var("HELION_ROUTE_RIPSTATS").is_ok();
    let mut rip_count: HashMap<String, usize> = HashMap::new();
    let mut ripper_count: HashMap<String, usize> = HashMap::new();
    let mut queued: HashSet<String> = net_names.iter().cloned().collect();
    let mut queue: std::collections::VecDeque<String> = net_names.into();
    // Nets cut back by a rip-up: their remaining pips and the loads to route.
    let mut partial: HashMap<String, (Vec<RoutedPip>, Vec<(String, String)>)> = HashMap::new();
    let t_start = std::time::Instant::now();
    let mut t_report = t_start;
    let mut routed_count = 0usize;
    loop {
        if rips > pf_after && routed_count >= total_nets {
            break;
        }
        let Some(net) = queue.pop_front() else { break };
        queued.remove(&net);
        // A net routed again replaces its previous (partial) entry.
        if partial.contains_key(&net) {
            nets.retain(|n: &RoutedNet7| n.net != net);
        }
        routed_count += 1;
        if t_report.elapsed().as_secs() >= 30 {
            t_report = std::time::Instant::now();
            eprintln!(
                "route: {:.0}s, {routed_count} net routes for {total_nets} nets, {} queued, {rips} rip-ups",
                t_start.elapsed().as_secs_f64(),
                queue.len() + 1
            );
            if rip_stats {
                let top = |m: &HashMap<String, usize>| {
                    let mut v: Vec<(&String, &usize)> = m.iter().collect();
                    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                    v.iter().take(8).map(|(k, c)| format!("{k}:{c}")).collect::<Vec<_>>().join(" ")
                };
                eprintln!("route: most ripped {}", top(&rip_count));
                eprintln!("route: most ripping {}", top(&ripper_count));
            }
        }
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
            // The cascade wire reaches only the next slice up the column.
            // (Adder chains run SLICE_XxYy to SLICE_XxY(y+1); the older
            // counter chains alternate the two slices of a tile.)
            let drv = eps.iter().find(|e| e.is_driver).map(|e| e.site.as_str()).unwrap_or("");
            let ld = eps.iter().find(|e| !e.is_driver).map(|e| e.site.as_str()).unwrap_or("");
            if let (Some((dx, dy)), Some((lx, ly))) = (parse_site_xy(drv), parse_site_xy(ld)) {
                let column = lx == dx && ly == dy + 1;
                let paired = if dx % 2 == 0 { lx == dx + 1 && ly == dy } else { lx + 1 == dx && ly == dy + 1 };
                if !column && !paired {
                    return Err(format!(
                        "route: carry cascade `{net}` ({drv} -> {ld}) violates column adjacency"
                    ));
                }
            }
            nets.push(RoutedNet7 { net, kind: RouteKind::Cascade });
            continue;
        }
        let drivers: Vec<&Endpoint> = eps.iter().filter(|e| e.is_driver).collect();
        let loads: Vec<&Endpoint> = eps.iter().filter(|e| !e.is_driver).collect();
        if loads.is_empty() {
            continue;
        }
        if drivers.is_empty() {
            // Nothing drives this net. Logic that reads it (a LUT input, a
            // flop pin, an output pad) would float on silicon: that is an
            // error, never a net quietly left out. Clock outputs of blocks
            // with no routing model, and inputs of clock buffers and CMTs,
            // are left to the dedicated network.
            let known_clock = device.is_clock_net(&net) || clock_spine_nets.contains(&net);
            if !known_clock {
                if let Some(l) = loads.iter().find(|l| l.site.starts_with("SLICE_") || l.site.starts_with("IOB_")) {
                    return Err(format!(
                        "route: net `{net}` has 0 drivers (need exactly 1); it is read at {}.{}",
                        l.site, l.pin
                    ));
                }
            }
            if known_clock || loads.iter().all(|l| matches!(l.pin.as_str(), "I0" | "I1" | "CLKIN1" | "CLKIN2" | "CLK")) {
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
        if loads.is_empty() {
            continue; // undriven net: nothing to route
        }
        let mut delays: Vec<(String, String, u32)> = Vec::new();
        // A net cut back by an earlier rip-up keeps its remaining tree and
        // routes only the loads that lost their connection.
        let (kept, todo): (Vec<RoutedPip>, Vec<&Endpoint>) = match partial.remove(&net) {
            Some((kept, lost)) => (kept, loads.iter().copied().filter(|l| lost.contains(&(l.site.clone(), l.pin.clone()))).collect()),
            None => (Vec::new(), loads.clone()),
        };
        let seed = if kept.is_empty() { Vec::new() } else { tree_arrivals(device, drivers[0], &kept)?.1 };
        let kind = if !kept.is_empty() && todo.is_empty() {
            // The cut only dropped branches no load needed.
            RouteKind::Pips(Vec::new())
        } else {
            route_net(&net, drivers[0], &todo, &clock_spine_nets, device, &claimed, &mut delays, &seed)?
        };
        let kind = match kind {
            RouteKind::Pips(mut p) if !kept.is_empty() => {
                p.extend(kept);
                p.sort_by(|a, b| (&a.tile, &a.dst_wire, &a.src_wire).cmp(&(&b.tile, &b.dst_wire, &b.src_wire)));
                p.dedup_by(|a, b| a.tile == b.tile && a.dst_wire == b.dst_wire && a.src_wire == b.src_wire && a.is_pseudo == b.is_pseudo);
                RouteKind::Pips(p)
            }
            k => k,
        };
        // Loads that found no free path took one through other nets'
        // wires: those nets are ripped up and queued again.
        let victims = match &kind {
            RouteKind::Pips(p) => device.contested(&net, p),
            _ => Vec::new(),
        };
        if !victims.is_empty() {
            rips += victims.len();
            // Each displaced net loses only the branches through the nodes
            // this path takes; the rest of its tree stays routed and claimed,
            // and only the loads cut off are routed again.
            let taken: Option<HashSet<u64>> = match &kind {
                RouteKind::Pips(p) => p
                    .iter()
                    .flat_map(|x| [(&x.tile, &x.src_wire), (&x.tile, &x.dst_wire)])
                    .map(|(t, w)| device.node_key(t, w))
                    .collect(),
                _ => None,
            };
            let mut kept_of: HashMap<String, Vec<RoutedPip>> = HashMap::new();
            let mut lost_of: HashMap<String, Vec<(String, String)>> = HashMap::new();
            for v in &victims {
                let cut = taken.as_ref().and_then(|taken| {
                    let RouteKind::Pips(vp) = &nets.iter().find(|n| &n.net == v)?.kind else { return None };
                    let veps = &net_eps[v];
                    let vdrv = veps.iter().find(|e| e.is_driver)?;
                    cut_tree(device, vdrv, veps, vp, taken)
                });
                if let Some((kept, lost)) = cut {
                    kept_of.insert(v.clone(), kept);
                    lost_of.insert(v.clone(), lost);
                }
            }
            let vset: HashSet<&str> = victims.iter().map(|v| v.as_str()).collect();
            nets.retain(|n: &RoutedNet7| !vset.contains(n.net.as_str()));
            claimed.retain(|_, o| !vset.contains(o.as_str()));
            load_delays.retain(|k, _| {
                if !vset.contains(k.0.as_str()) {
                    return true;
                }
                // Whole rip-up: every delay goes; partial: the lost loads'.
                match lost_of.get(&k.0) {
                    Some(lost) => !lost.contains(&(k.1.clone(), k.2.clone())),
                    None => false,
                }
            });
            device.release_claims(&victims, claimed.len());
            for v in &victims {
                if let Some(kept) = kept_of.remove(v) {
                    let mut wires = Vec::new();
                    for p in &kept {
                        for w in [&p.src_wire, &p.dst_wire] {
                            if is_power_wire(w) {
                                continue;
                            }
                            let key = (p.tile.clone(), w.clone());
                            if !claimed.contains_key(&key) {
                                wires.push(key.clone());
                                claimed.insert(key, v.clone());
                            }
                        }
                    }
                    device.note_claims(v, &wires, claimed.len());
                    // A net already waiting keeps the loads it waited for.
                    let entry = partial.entry(v.clone()).or_insert_with(|| (Vec::new(), Vec::new()));
                    entry.0 = kept.clone();
                    for l in lost_of.remove(v).unwrap_or_default() {
                        if !entry.1.contains(&l) {
                            entry.1.push(l);
                        }
                    }
                    nets.push(RoutedNet7 { net: v.clone(), kind: RouteKind::Pips(kept) });
                } else {
                    partial.remove(v);
                }
                if !queued.contains(v) {
                    queued.insert(v.clone());
                    queue.push_back(v.clone());
                }
                if rip_stats {
                    *rip_count.entry(v.clone()).or_insert(0usize) += 1;
                    *ripper_count.entry(net.clone()).or_insert(0usize) += 1;
                }
            }
        }
        if device.is_clock_net(&net) && matches!(kind, RouteKind::Pips(_)) {
            graph_clocks = true;
        }
        for (site, pin, d) in delays {
            load_delays.insert((net.clone(), site, pin), d);
        }
        if let RouteKind::Pips(pips) = &kind {
            let mut new_wires = Vec::new();
            for p in pips {
                for w in [&p.src_wire, &p.dst_wire] {
                    if is_power_wire(w) {
                        continue;
                    }
                    let key = (p.tile.clone(), w.clone());
                    if !claimed.contains_key(&key) {
                        new_wires.push(key.clone());
                        claimed.insert(key, net.clone());
                    }
                }
            }
            device.note_claims(&net, &new_wires, claimed.len());
        }
        nets.push(RoutedNet7 { net, kind });
    }

    if !queue.is_empty() {
        eprintln!("route: {} nets still queued after {rips} rip-ups; PathFinder", queue.len());
        pathfinder(&net_eps, &clock_spine_nets, device, &mut nets, &mut claimed, &mut load_delays, queue.into_iter().collect())?;
    }

    // BUFGCTRL control-pin tie-offs. The static config (emit.rs) sets pin
    // polarity, but S0/S1/CE0/CE1/IGNORE0/IGNORE1 are routing inputs: left
    // undriven, the BUFGCTRL output freezes at INIT_OUT with no toggle
    // (UG472 Table 2-3), the observed "done=1, LEDs dark" failure.
    route_bufg_tieoffs(placed, device)?;

    // Clock spine stitching per BUFGCTRL: O exit to the global line plus
    // REBUF stitches. Without these the clock never leaves the BUFG site.
    for bufg in &placed.bufgs {
        if graph_clocks {
            break;
        }
        // Regional buffers (BUFIO, BUFR) drive their own clock region, not
        // the global spine.
        if !bufg.site.starts_with("BUFGCTRL") {
            continue;
        }
        let spine = device.bufg_spine(&bufg.site)?;
        if !spine.pips.is_empty() {
            nets.push(RoutedNet7 {
                net: format!("__spine_{}", bufg.site.to_ascii_lowercase()),
                kind: RouteKind::Pips(spine.pips),
            });
        }
    }

    if rips > 0 {
        eprintln!("route: congestion resolved with {rips} rip-ups");
    }
    Ok(Routed7 {
        placed: placed.clone(),
        nets,
        load_delays,
        graph_clocks,
    })
}

/// PathFinder (McMurchie and Ebeling, 1995) over the signal nets: they may
/// share wires for a while; every iteration reroutes the nets on overused
/// nodes, with sharing priced higher each time and history cost added to
/// nodes that stay overused, until no node carries two nets. Clock nets
/// keep their wires (they stay claimed, so no search may enter them).
#[allow(clippy::too_many_arguments)]
fn pathfinder(
    net_eps: &HashMap<String, Vec<Endpoint>>,
    clock_spine_nets: &HashSet<String>,
    device: &dyn RouteQuery,
    nets: &mut Vec<RoutedNet7>,
    claimed: &mut HashMap<(String, String), String>,
    load_delays: &mut HashMap<(String, String, String), u32>,
    queued: Vec<String>,
) -> Result<(), String> {
    const MAX_ITERATIONS: usize = 100;
    let wires_of = |pips: &[RoutedPip]| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = pips
            .iter()
            .flat_map(|p| [(p.tile.clone(), p.src_wire.clone()), (p.tile.clone(), p.dst_wire.clone())])
            .filter(|(_, w)| !is_power_wire(w))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let mut pf: std::collections::BTreeMap<String, Vec<RoutedPip>> = std::collections::BTreeMap::new();
    for n in nets.iter() {
        if let RouteKind::Pips(p) = &n.kind {
            if !device.is_clock_net(&n.net) {
                pf.insert(n.net.clone(), p.clone());
            }
        }
    }
    let mut todo: std::collections::BTreeSet<String> = queued.into_iter().collect();
    for n in &todo {
        pf.entry(n.clone()).or_default();
    }
    let names: Vec<String> = pf.keys().cloned().collect();
    claimed.retain(|_, o| !pf.contains_key(o));
    device.release_claims(&names, claimed.len());
    let mut present: u32 = 200;
    device.pf_present(Some(present));
    for p in pf.values() {
        device.pf_occupy(&wires_of(p), 1);
    }
    let t0 = std::time::Instant::now();
    let mut done = false;
    for it in 1..=MAX_ITERATIONS {
        for (n, p) in &pf {
            if device.pf_overused(&wires_of(p)) {
                todo.insert(n.clone());
            }
        }
        let count = todo.len();
        for net in std::mem::take(&mut todo) {
            device.pf_occupy(&wires_of(&pf[&net]), -1);
            let eps = &net_eps[&net];
            let drv = eps.iter().find(|e| e.is_driver).ok_or_else(|| format!("route: PathFinder: net `{net}` has no driver"))?;
            let loads: Vec<&Endpoint> = eps.iter().filter(|e| !e.is_driver).collect();
            let mut delays = Vec::new();
            let p = match route_net(&net, drv, &loads, clock_spine_nets, device, claimed, &mut delays, &[])? {
                RouteKind::Pips(p) => p,
                k => return Err(format!("route: PathFinder: net `{net}` routed as {k:?}, not pips")),
            };
            device.pf_occupy(&wires_of(&p), 1);
            if count <= 4 && std::env::var("HELION_ROUTE_DEBUG").is_ok() {
                let hot = |t: &str, w: &str| device.pf_overused(&[(t.to_string(), w.to_string())]);
                let near: Vec<String> = p.iter().filter(|x| hot(&x.tile, &x.src_wire) || hot(&x.tile, &x.dst_wire)).map(|x| format!("{}.{}->{}", x.tile, x.src_wire, x.dst_wire)).collect();
                let ls: Vec<String> = loads.iter().map(|l| format!("{}.{}", l.site, l.pin)).collect();
                eprintln!("route_debug pf net={net} drv={}.{} loads={ls:?} shared pips={near:?}", drv.site, drv.pin);
            }
            for l in &loads {
                load_delays.remove(&(net.clone(), l.site.clone(), l.pin.clone()));
            }
            for (site, pin, d) in delays {
                load_delays.insert((net.clone(), site, pin), d);
            }
            pf.insert(net, p);
        }
        let over = device.pf_history();
        eprintln!("route: PathFinder iteration {it}: {count} nets rerouted, {over} wires overused, {:.0}s", t0.elapsed().as_secs_f64());
        if over == 0 {
            done = true;
            break;
        }
        present = present.saturating_mul(3).div_ceil(2).min(100_000);
        device.pf_present(Some(present));
    }
    device.pf_present(None);
    if !done {
        return Err(format!("route: PathFinder: wires still shared after {MAX_ITERATIONS} iterations"));
    }
    nets.retain(|n| !pf.contains_key(&n.net));
    for (net, p) in pf {
        let mut new = Vec::new();
        for k in wires_of(&p) {
            if !claimed.contains_key(&k) {
                claimed.insert(k.clone(), net.clone());
                new.push(k);
            }
        }
        device.note_claims(&net, &new, claimed.len());
        nets.push(RoutedNet7 { net, kind: RouteKind::Pips(p) });
    }
    Ok(())
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

/// Validate every placed BUFGCTRL's tie-off path, but emit no fabric
/// tie-off nets.
///
/// The ZB-10 oracle programs zero tie-offs device-wide (a power-IMUX
/// scan of golden finds none, not even for BUFG controls): IGNORE1
/// masks S1/CE1, and the HIGH pins match the IMUX default select, so
/// fabric drive is redundant programming. The device query still runs
/// so an unknown site (or a DB without a tie-off path) fails loudly
/// here instead of silently under-driving the clock mux.
fn route_bufg_tieoffs(placed: &Placed7, device: &dyn RouteQuery) -> Result<(), String> {
    for bufg in &placed.bufgs {
        // Only BUFGCTRL sites have the S/CE/IGNORE control pins; regional
        // buffers (BUFIO, BUFR) have none to tie off.
        if !bufg.site.starts_with("BUFGCTRL") {
            continue;
        }
        // Validation only: an unknown site (or a DB without a tie-off
        // path) still fails loudly. The paths themselves stay off
        // fabric (see fn docs).
        device.bufg_tieoff_pips(&bufg.site).map(|_| ())?;
    }
    Ok(())
}

/// Loads reached inside the slice with no routing: the FF D input from
/// its same-letter LUT or carry sum (`AFF_D`..`DFF_D`), and carry S inputs
/// from the same-letter LUT O6. Every other load, even in the driver's own
/// slice, arrives through the fabric.
fn is_site_internal_pin(pin: &str) -> bool {
    pin.ends_with("FF_D") || matches!(pin, "S0" | "S1" | "S2" | "S3" | "F7F8_I0" | "F7F8_I1")
}

/// Slice bypass inputs (`AX`..`DX`): reached through the INT tile's
/// `BYP_ALT` muxes, which only these loads may use.
fn is_bypass_pin(pin: &str) -> bool {
    matches!(pin, "AX" | "BX" | "CX" | "DX")
}

/// A DSP48E1 site (`DSP48_X0Y0`, ...): its non-`A` inputs are fed from
/// `DSP_BYP*`/`DSP_FAN*` wires, which are reached through the INT tile's
/// `BYP_ALT`/`FAN_ALT` muxes.
fn is_dsp_site(site: &str) -> bool {
    site.starts_with("DSP48")
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
    clock_spine_nets: &HashSet<String>,
    device: &dyn RouteQuery,
    claimed: &HashMap<(String, String), String>,
    delays: &mut Vec<(String, String, u32)>,
    seed: &[(String, String, u32)],
) -> Result<RouteKind, String> {
    if check_dedicated_cascade(net, driver, loads)? {
        return Ok(RouteKind::DedicatedCascade);
    }
    // Split loads into site-internal (same site as driver and not a
    // LUT input) and external. Slice LUT inputs (A1-A6 and friends)
    // always arrive through fabric IMUX, even from the same slice:
    // 7-series has no site-local LUT-input path, and the oracle feeds
    // every counter S input from INT. Routing one through the
    // site-internal shortcut would silently unconnect it on silicon.
    let (internal, external): (Vec<&&Endpoint>, Vec<&&Endpoint>) = loads
        .iter()
        .partition(|l| l.site == driver.site && is_site_internal_pin(&l.pin));
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
        // A device model without the driver's wire has no clock graph
        // (unit-test fakes): the fixed spine recipe applies.
        let Some((src_tile, src_wire)) = device.site_pin_wire(&driver.site, &driver.pin) else {
            return Ok(RouteKind::ClockSpine);
        };
        let opts = RouteOpts { clock: true, ..Default::default() };
        let mut tree: Vec<(String, String, u32)> = vec![(src_tile.clone(), src_wire.clone(), 0)];
        let mut all: Vec<RoutedPip> = Vec::new();
        for load in &external {
            let sinks = device.wire_for_site_pin(&load.site, &load.pin);
            match device.route_fast((&src_tile, &src_wire), &tree, &sinks, opts, net, claimed) {
                // No graph router (unit-test fakes): the fixed spine recipe.
                None => return Ok(RouteKind::ClockSpine),
                Some(None) => {
                    // The device graph has no clock path to this pin (an
                    // incomplete database for the part): fixed recipe.
                    eprintln!("route: clock `{net}` has no graph path to {}.{}; using the fixed spine recipe", load.site, load.pin);
                    return Ok(RouteKind::ClockSpine);
                }
                Some(Some((pips, arr))) => {
                    for (p, (sa, da)) in pips.iter().zip(&arr) {
                        tree.push((p.tile.clone(), p.src_wire.clone(), *sa));
                        tree.push((p.tile.clone(), p.dst_wire.clone(), *da));
                    }
                    all.extend(pips);
                }
            }
        }
        all.sort_by(|a, b| (&a.tile, &a.dst_wire, &a.src_wire).cmp(&(&b.tile, &b.dst_wire, &b.src_wire)));
        all.dedup_by(|a, b| a.tile == b.tile && a.dst_wire == b.dst_wire && a.src_wire == b.src_wire);
        return Ok(RouteKind::Pips(all));
    }
    // Clock outputs of primitives without a routing graph model (CMT, GT,
    // IBUFDS_GTE2) ride the dedicated network.
    if clock_spine_nets.contains(net) {
        return Ok(RouteKind::ClockSpine);
    }
    // IBUF into a regional buffer: dedicated clock input route (a CMT input
    // routes over the graph below).
    // (IBUF into a BUFG goes through general pips, see above.)
    let driver_is_ibuf = driver.pin == "I" && driver.site.starts_with("IOB_");
    let loads_are_clk = loads.iter().all(|l| l.site.starts_with("BUFIO_") || l.site.starts_with("BUFR_"));
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
    // Only tie-off nets (driven by a power wire) may travel on power and
    // clock-distribution wires. A signal let onto `GFAN`/`VCC_WIRE` shorts
    // against the tie-offs of the tile it crosses (a pad-to-BUFG net routed
    // that way killed the BUFG's control pins).
    let allow_power = is_power_wire(&src_wire);
    // Wires this net already reaches: later loads branch off the tree
    // instead of re-entering one of its wires from a second source (which
    // would double-drive that wire's mux).
    let mut tree: Vec<(String, String, u32)> = vec![(src_tile.clone(), src_wire.clone(), 0)];
    // A partly ripped-up net keeps the rest of its tree: new loads branch off
    // it, with the arrival times it already has.
    tree.extend(seed.iter().cloned());
    for load in &external_loads {
        let sinks = device.wire_for_site_pin(&load.site, &load.pin);
        if sinks.is_empty() {
            return Err(format!(
                "route: no wire for load {}.{} (net `{net}`)",
                load.site, load.pin
            ));
        }
        // A DSP48E1 input reached over `DSP_BYP*` also comes in through an
        // INT tile `BYP_ALT` mux, so it needs the same exemption as a slice
        // bypass pin (G35). Keying on the pin name alone would not catch it:
        // `D0` is a DSP data bit, not a slice `D` input.
        let allow_byp = is_bypass_pin(&load.pin) || is_dsp_site(&load.site);
        let opts = RouteOpts { allow_power, allow_byp, ..Default::default() };
        // A BUFG or CMT clock input takes the dedicated clock path
        // (clock-capable pad or CMT -> HCLK_CMT/HROW -> BUFG, CMT feedback)
        // when one exists, else general interconnect.
        let mut fast = None;
        if matches!(load.pin.as_str(), "CLKIN1" | "CLKIN2") {
            if let Some(p) = crate::cmt::dedicated_clock_route(device, (&src_tile, &src_wire), &sinks) {
                let n = p.len();
                fast = Some(Some((p, vec![(0, 0); n])));
            }
        }
        if fast.is_none() && matches!(load.pin.as_str(), "I0" | "I1" | "CLKIN1" | "CLKIN2" | "CLKFBIN") {
            if let Some(Some(r)) = device.route_fast((&src_tile, &src_wire), &tree, &sinks, RouteOpts { no_int: true, ..opts }, net, claimed) {
                fast = Some(Some(r));
            }
        }
        if fast.is_none() {
            // A free path near the net first (giving up on long detours);
            // else the cheapest path through other nets' wires, near the
            // net and then anywhere. The caller rips up the displaced nets.
            for (negotiate, bbox, cap_detour) in [(false, 8, true), (true, 8, false), (true, 0, false)] {
                fast = device.route_fast((&src_tile, &src_wire), &tree, &sinks, RouteOpts { negotiate, bbox, cap_detour, ..opts }, net, claimed);
                if !matches!(fast, Some(None)) {
                    break;
                }
            }
        }
        let bufg_input = matches!(load.pin.as_str(), "I0" | "I1") && load.site.starts_with("BUFGCTRL");
        let from_pad = driver.site.starts_with("IOB");
        if matches!(fast, Some(None)) && bufg_input && !from_pad {
            // A clock made in the fabric: global clock wires lead from the
            // interconnect to the buffer inputs.
            for negotiate in [false, true] {
                fast = device.route_fast((&src_tile, &src_wire), &tree, &sinks, RouteOpts { allow_gclk: true, negotiate, ..opts }, net, claimed);
                if !matches!(fast, Some(None)) {
                    break;
                }
            }
        }
        if matches!(fast, Some(None)) && matches!(load.pin.as_str(), "I0" | "I1") && from_pad {
            // Parts whose database lacks the clock-input wiring: the old
            // permissive search (clock wires allowed) as a last resort.
            // Only from a pad: from the fabric it took pips with no
            // configuration bits through unused buffers (x7v rejects that).
            eprintln!("route: `{net}` has no clean path to {}.{}; retrying with clock wires allowed", load.site, load.pin);
            fast = device.route_fast((&src_tile, &src_wire), &tree, &sinks, RouteOpts { allow_power: true, any_pip: true, ..opts }, net, claimed);
        }
        let (pips, arr) = match fast {
            Some(Some((p, a))) => (Some(p), Some(a)),
            Some(None) => (None, None),
            None => (bfs_route(&src_tile, &src_wire, &sinks, device, allow_power, allow_byp, net, claimed), None),
        };
        let pips = pips
            .ok_or_else(|| format!("route: net `{net}` unroutable to {}.{} (src {src_tile}.{src_wire})", load.site, load.pin))?;
        match &arr {
            Some(a) => {
                for (p, (sa, da)) in pips.iter().zip(a) {
                    tree.push((p.tile.clone(), p.src_wire.clone(), *sa));
                    tree.push((p.tile.clone(), p.dst_wire.clone(), *da));
                }
                // Loads with no new pips sit on the tree already.
                let d = a.last().map(|x| x.1).unwrap_or(0);
                delays.push((load.site.clone(), load.pin.clone(), d));
            }
            None => {
                for p in &pips {
                    tree.push((p.tile.clone(), p.src_wire.clone(), 0));
                    tree.push((p.tile.clone(), p.dst_wire.clone(), 0));
                }
            }
        }
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

/// Arrival time (ps) at every node the pips reach from the driver, the same
/// way the search adds them up, and the tree as search seeds
/// `(tile, wire, arrival)`.
fn tree_arrivals(
    device: &dyn RouteQuery,
    driver: &Endpoint,
    pips: &[RoutedPip],
) -> Result<(HashMap<u64, u32>, Vec<(String, String, u32)>), String> {
    let (st, sw) = device
        .site_pin_wire(&driver.site, &driver.pin)
        .ok_or_else(|| format!("route: no wire for driver {}.{}", driver.site, driver.pin))?;
    let key = |t: &str, w: &str| device.node_key(t, w).ok_or_else(|| format!("route: no node for {t}.{w}"));
    let mut by_src: HashMap<u64, Vec<&RoutedPip>> = HashMap::new();
    for p in pips {
        by_src.entry(key(&p.tile, &p.src_wire)?).or_default().push(p);
    }
    let mut arr: HashMap<u64, u32> = HashMap::new();
    let mut seeds = Vec::new();
    let start = key(&st, &sw)?;
    arr.insert(start, 0);
    let mut work = vec![start];
    while let Some(n) = work.pop() {
        let a = arr[&n];
        for p in by_src.get(&n).map(|v| v.as_slice()).unwrap_or(&[]) {
            let c = device
                .pip_cost(&p.tile, &p.src_wire, &p.dst_wire)
                .ok_or_else(|| format!("route: no pip {}.{}->{}", p.tile, p.src_wire, p.dst_wire))?;
            let d = key(&p.tile, &p.dst_wire)?;
            seeds.push((p.tile.clone(), p.src_wire.clone(), a));
            seeds.push((p.tile.clone(), p.dst_wire.clone(), a + c));
            if !arr.contains_key(&d) {
                arr.insert(d, a + c);
                work.push(d);
            }
        }
    }
    Ok((arr, seeds))
}

/// Cut a routed net back where another net's new path takes nodes from it.
/// Returns the pips still connected to the driver and the loads
/// `(site, pin)` they no longer reach; `None` when the device has no node
/// model (the net is then rerouted whole).
fn cut_tree(
    device: &dyn RouteQuery,
    driver: &Endpoint,
    eps: &[Endpoint],
    pips: &[RoutedPip],
    taken: &HashSet<u64>,
) -> Option<(Vec<RoutedPip>, Vec<(String, String)>)> {
    let mut rest = Vec::new();
    for p in pips {
        let s = device.node_key(&p.tile, &p.src_wire)?;
        let d = device.node_key(&p.tile, &p.dst_wire)?;
        if !taken.contains(&s) && !taken.contains(&d) {
            rest.push(p.clone());
        }
    }
    let (arr, _) = tree_arrivals(device, driver, &rest).ok()?;
    let mut kept = Vec::new();
    for p in rest {
        if arr.contains_key(&device.node_key(&p.tile, &p.src_wire)?) {
            kept.push(p);
        }
    }
    let mut lost = Vec::new();
    for l in eps.iter().filter(|e| !e.is_driver) {
        if l.site == driver.site && is_site_internal_pin(&l.pin) {
            continue;
        }
        let sinks = device.wire_for_site_pin(&l.site, &l.pin);
        let reached = sinks.iter().any(|(t, w)| device.node_key(t, w).is_some_and(|k| arr.contains_key(&k)));
        if !reached {
            lost.push((l.site.clone(), l.pin.clone()));
        }
    }
    Some((kept, lost))
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
    allow_byp: bool,
    net: &str,
    claimed: &HashMap<(String, String), String>,
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
            // CLB pseudo-pips are slice configuration (e.g. `A->>AMUX` is
            // AOUTMUX=O6), not wiring: the router never programs them, so a
            // path through one would leave the mux unset on silicon.
            if is_pseudo && tile.starts_with("CLB") {
                continue;
            }
            if !allow_power && is_power_wire(&dst_wire) {
                continue;
            }
            if !allow_power && !allow_byp && is_byp_wire(&dst_wire) {
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
        for (ntile, nwire) in device.node_wires(&tile, &wire) {
            if !allow_power && is_power_wire(&nwire) {
                continue;
            }
            if !allow_power && !allow_byp && is_byp_wire(&nwire) {
                continue;
            }
            if owned_elsewhere(claimed, net, &ntile, &nwire) {
                continue;
            }
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
    use crate::pack::{IobDir, Packed7, PackedBufg, PackedCarry, PackedFf, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedBufg, PlacedCarry, PlacedFf, PlacedIob, PlacedLut};

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
                ("SLICE_X0", "A") => Some(("INT_X0".into(), "B".into())),
                ("SLICE_X1", "A") => Some(("INT_X1".into(), "C".into())),
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
        assert!(FakeRouter.is_clock_net("clk_bufg"));
        assert!(!FakeRouter.is_clock_net("n1"));
    }

    #[test]
    fn dedicated_cascade_validates_column_alignment() {
        let mut packed = Packed7::default();
        packed.carries.push(PackedCarry {
            name: "c0".into(),
            cyinit: Some(false),
            co_net: Some("carry_chain".into()),
            ..Default::default()
        });
        packed.carries.push(PackedCarry {
            name: "c1".into(),
            ci_net: "carry_chain".into(),
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
        assert_eq!(r.nets[0].kind, RouteKind::Cascade);

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
                // Clocked from a clock-network net (an input pad drives it).
                PackedFf {
                    name: "u_ff0".into(), d_net: "o0".into(), q_net: "q0".into(),
                    clk_net: "clk_bufg".into(), ce_net: None, sr_net: None, init: false, srval: false, sr_async: false,
                },
                PackedFf {
                    name: "u_ff1".into(), d_net: "o1".into(), q_net: "q1".into(),
                    clk_net: "clk_bufg".into(), ce_net: None, sr_net: None, init: false, srval: false, sr_async: false,
                },
            ],
            iobs: vec![PackedIob {
                name: "ibuf_clk".into(), port: "clk".into(), dir: IobDir::In,
                net: "clk_bufg".into(), loc: None, iostandard: "LVCMOS33".into(),
                drive: None, slew: None,
            }],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![
                PackedCarry {
                    name: "u_c0".into(), ci_net: "one".into(), cyinit: None, co_net: Some("co0".into()),
                    s_nets: [Some("s0".into()), None, None, None],
                    di_nets: [None, None, None, None],
                    o_nets: [Some("o0".into()), None, None, None],
                    ..Default::default()
                },
                PackedCarry {
                    name: "u_c1".into(), ci_net: "co0".into(), cyinit: None, co_net: None,
                    s_nets: [Some("s1".into()), None, None, None],
                    di_nets: [None, None, None, None],
                    o_nets: [Some("o1".into()), None, None, None],
                    ..Default::default()
                },
            ],
            carry_anchor: None, ..Default::default()
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
            iobs: vec![PlacedIob {
                name: "ibuf_clk".into(), port: "clk".into(), dir: IobDir::In,
                tile: "IOB_T".into(), site: "IOB_CLK".into(),
            }],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![
                PlacedCarry { name: "u_c0".into(), tile: "CLB_X0".into(), site: "SLICE_X0".into() },
                PlacedCarry { name: "u_c1".into(), tile: "CLB_X1".into(), site: "SLICE_X1".into() },
            ],
            ..Default::default()
        }
    }

    fn kind_of(routed: &Routed7, net: &str) -> String {
        match &routed.nets.iter().find(|n| n.net == net).unwrap().kind {
            RouteKind::SiteInternal => "internal".into(),
            RouteKind::Cascade => "cascade".into(),
            RouteKind::DedicatedCascade => "dedicated".into(),
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
            if matches!(n.kind, RouteKind::Cascade | RouteKind::SiteInternal | RouteKind::ClockSpine) {
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
        let clean = bfs_route("INT_X0", "A", &sinks, &FakeRouter, false, false, "n", &empty).unwrap();
        assert_eq!(clean.len(), 3, "got {clean:?}");
        assert!(
            clean.iter().all(|p| !is_power_wire(&p.dst_wire) && !is_power_wire(&p.src_wire)),
            "power wire used, got {clean:?}"
        );
        // Exempted (tieoff and clock-path nets): the shortcut is taken.
        let exempt = bfs_route("INT_X0", "A", &sinks, &FakeRouter, true, false, "n", &empty).unwrap();
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
        let clean = bfs_route("INT_X0", "A", &sinks, &BypTrap, false, false, "n", &empty).unwrap();
        assert_eq!(clean.len(), 3, "got {clean:?}");
        assert!(
            clean.iter().all(|p| !is_byp_wire(&p.dst_wire) && !is_byp_wire(&p.src_wire)),
            "BYP wire used, got {clean:?}"
        );
        // Exempted (tieoff and clock-path nets): the shortcut is taken.
        let exempt = bfs_route("INT_X0", "A", &sinks, &BypTrap, true, false, "n", &empty).unwrap();
        assert!(
            exempt.iter().any(|p| p.dst_wire == "BYP_ALT0" || p.src_wire == "BYP_ALT0"),
            "shortcut not taken, got {exempt:?}"
        );
    }

    #[test]
    fn bufg_tieoffs_stay_off_fabric() {
        // ZB-10 oracle: a device-wide power-IMUX scan of golden shows
        // zero tieoff programming, not even for BUFG controls (IGNORE1
        // masks S1/CE1, HIGH pins match IMUX defaults). The router must
        // not emit __tieoff nets even when the device offers paths.
        struct TieoffDevice;
        impl RouteQuery for TieoffDevice {
            fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
                FakeRouter.pips_from(tile, wire)
            }
            fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
                FakeRouter.node_wires(tile, wire)
            }
            fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)> {
                FakeRouter.site_pin_wire(site, pin)
            }
            fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)> {
                if (site, pin) == ("BUFGCTRL_X0Y0", "I0") {
                    return vec![("INT_X0".into(), "B".into())];
                }
                FakeRouter.wire_for_site_pin(site, pin)
            }
            fn is_clock_net(&self, net: &str) -> bool {
                FakeRouter.is_clock_net(net)
            }
            fn bufg_tieoff_pips(&self, _: &str) -> Result<BufgTieoff, String> {
                Ok(BufgTieoff {
                    ones: vec![RoutedPip {
                        tile: "INT_X0".into(),
                        dst_wire: "IMUX0".into(),
                        src_wire: "VCC_WIRE".into(),
                        is_pseudo: false,
                    }],
                    zeros: vec![RoutedPip {
                        tile: "INT_X0".into(),
                        dst_wire: "GFAN0".into(),
                        src_wire: "GND_WIRE".into(),
                        is_pseudo: false,
                    }],
                })
            }
            fn bufg_spine(&self, site: &str) -> Result<BufgSpine, String> {
                FakeRouter.bufg_spine(site)
            }
        }
        let mut placed = placed_two_site();
        // BUFG taps the existing n1 net (driven by u_inv) and drives
        // clk_bufg into an output pad, so every net has a driver.
        placed.packed.bufgs.push(PackedBufg {
            name: "b".into(), in_net: "n1".into(), out_net: "clk_bufg".into(),
            ..Default::default()
        });
        placed.bufgs.push(PlacedBufg {
            name: "b".into(), tile: "CLK_X".into(), site: "BUFGCTRL_X0Y0".into(),
        });
        placed.packed.iobs.push(PackedIob {
            name: "obuf_clk".into(), port: "c".into(), dir: IobDir::Out,
            net: "clk_bufg".into(), loc: None, iostandard: "LVCMOS33".into(),
            drive: None, slew: None,
        });
        placed.iobs.push(PlacedIob {
            name: "obuf_clk".into(), port: "c".into(), dir: IobDir::Out,
            tile: "IOB_T".into(), site: "IOB_X1".into(),
        });
        let routed = route(&placed, &TieoffDevice).unwrap();
        assert!(
            routed.nets.iter().all(|n| !n.net.starts_with("__tieoff")),
            "tieoff nets on fabric: {:?}",
            routed.nets.iter().map(|n| &n.net).collect::<Vec<_>>()
        );
    }

    /// ZB-10 S24 feed on the real xc7z020 graph: with zero claims BFS
    /// must find the 2-pip SR1 path (LOGIC_OUTS3 -> SR1BEG_S0 ->
    /// IMUX42). This pins graph completeness (tileconn stitching plus
    /// INT_R pips): if stitching ever truncates a node or a pip goes
    /// missing, the shortest path changes and this test fails.
    /// Provenance: an independent Python BFS over tileconn.json plus
    /// tile_type_INT_R.json finds the same 2-edge path (ZB-10 run log).
    /// Note the golden bitstream does NOT take this path: SR1 carries
    /// the transit S feed (OUTS7) in both designs, so the S24 net takes
    /// a second choice (ours: FAN chain, golden: ER1 ring). That
    /// divergence is a cost-function question, not a graph gap.
    #[test]
    fn bfs_finds_shortest_s24_feed_on_real_graph() {
        use crate::device::{default_db_root, Device};
        use crate::xdevice::{build_node_map, XDevice};

        let db_root = default_db_root();
        let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
        let maps = build_node_map(&mut device);
        let xdev = XDevice::new(&device, maps);
        let sinks = vec![("INT_R_X71Y44".to_string(), "IMUX42".to_string())];
        let claimed: HashMap<(String, String), String> = HashMap::new();
        let pips = bfs_route(
            "INT_R_X71Y44",
            "LOGIC_OUTS3",
            &sinks,
            &xdev,
            false,
            false,
            "probe",
            &claimed,
        )
        .expect("no S24 path on real graph");
        let hops: Vec<(&str, &str)> = pips
            .iter()
            .map(|p| (p.src_wire.as_str(), p.dst_wire.as_str()))
            .collect();
        assert_eq!(
            hops,
            vec![
                ("LOGIC_OUTS3", "SR1BEG_S0"),
                ("SR1BEG_S0", "IMUX42"),
            ],
            "shortest S24 feed changed: {hops:?}"
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
            bfs_route("INT_X0", "A", &sinks, &FakeRouter, false, false, "mine", &claimed).is_none(),
            "routed through foreign wire"
        );
        // Same-net ownership stays legal across loads of one net.
        claimed.insert(("INT_X0".into(), "B".into()), "mine".into());
        let pips = bfs_route("INT_X0", "A", &sinks, &FakeRouter, false, false, "mine", &claimed).unwrap();
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
        assert!(!is_lut_input_pin("A"));
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
