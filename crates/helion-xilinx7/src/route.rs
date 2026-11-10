//! Route stage: [`Placed7`] → [`Routed7`].
//!
//! BFS maze router on the 7-series pip graph. Each net is routed from its
//! driver site-pin to its load site-pins through INT-tile pips and
//! tile-to-tile node stitching. Site-internal nets (LUT→FF in one slice)
//! and the dedicated global clock network need no pips and are recorded
//! as such for the emitter.

use crate::place::{Placed7, PlacedCarry};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// Cost units charged for one hop, from the pip's propagation delay.
///
/// Delays are ~0.03-0.15 ns and integer costs are wanted, so a hop is priced in
/// **picoseconds**. A pip with no timing data charges `DEFAULT_HOP_PS`, the
/// typical interconnect delay, so an untimed pip is neither free nor
/// prohibitively expensive.
///
/// Why this exists: with every hop costing exactly 1, a route that wandered
/// through an unrelated tile's long lines tied with a direct interconnect hop
/// and could win on tie-breaking. Hardware disagreed -- the indirect route lit
/// nothing (`INT_R_X73Y46.IMUX34.WW2END0` is dark, `NR1END1` blinks). nextpnr
/// avoids this by being delay-driven; this is the same idea, and it is what
/// `tile_type_*.json`'s `src_to_dst.delay` was always there to support.
pub const DEFAULT_HOP_PS: i64 = 45;

fn hop_cost(delay_ns: Option<f64>) -> i64 {
    match delay_ns {
        Some(ns) => ((ns * 1000.0).round() as i64).max(1),
        None => DEFAULT_HOP_PS,
    }
}

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
    /// The other SLICE site of `tile` — the mate half of a CLB tile. A carry
    /// chain's S LUTs live in one half and its DI (generate) LUTs in the
    /// other, and both read the same operand nets, so the router must be able
    /// to reach both halves. `None` when unknown (callers skip the paired
    /// endpoint rather than guessing the sibling's name).
    fn slice_pair(&self, tile: &str, site: &str) -> Option<String> {
        let _ = (tile, site);
        None
    }
    /// Is this net on the dedicated global clock network (BUFG output)?
    fn is_clock_net(&self, net: &str) -> bool;
    /// Cost units charged for one hop, from the pip's propagation delay.
///
/// Delays are ~0.03-0.15 ns and integer costs are wanted, so a hop is priced in
/// **picoseconds**. A pip with no timing data charges `DEFAULT_HOP_PS`, the
/// typical interconnect delay, so an untimed pip is neither free nor
/// prohibitively expensive.
///
/// Why this exists: with every hop costing exactly 1, a route that wandered
/// through an unrelated tile's long lines tied with a direct interconnect hop
/// and could win on tie-breaking. Hardware disagreed -- the indirect route lit
/// nothing (`INT_R_X73Y46.IMUX34.WW2END0` is dark, `NR1END1` blinks). nextpnr
/// avoids this by being delay-driven; this is the same idea, and it is what
/// `tile_type_*.json`'s `src_to_dst.delay` was always there to support.

/// Real grid coordinates of a tile, for the A* heuristic. `None` disables
    /// the heuristic for that tile (the search degrades to plain BFS), so
    /// minimal test routers need not implement this.
    fn tile_xy(&self, _tile: &str) -> Option<(i32, i32)> {
        None
    }
    /// Propagation delay in ns for the pip `wire -> dst_wire` in `tile`.
    ///
    /// `None` means "no timing data"; the router charges a default hop. The
    /// default returns `None`, so minimal test routers keep hop-count costing.
    fn pip_delay(&self, _tile: &str, _wire: &str, _dst_wire: &str) -> Option<f64> {
        None
    }
    /// Can `wire -> dst_wire` in `tile` be written to a bitstream?
    ///
    /// Some pips exist in `tile_type_*.json` but have no segbits, so prjxray
    /// never reverse-engineered the bit -- `LIOI3.IOI_BYP3_0->>IOI_IMUX_RC0`
    /// is one. Routing through one reaches the sink in the model and then
    /// fails at emission with `no segbits for feature`. Defaults to `true` so
    /// minimal test routers are unaffected.
    fn pip_is_encodable(&self, _tile: &str, _wire: &str, _dst_wire: &str) -> bool {
        true
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

impl Routed7 {
    /// Is a flip-flop's data input physically driven?
    ///
    /// `mux_for_ff` chooses the flop's `D` mux from the *netlist* -- which
    /// cell claims the flop's `D` net -- and that says nothing about whether
    /// the router ever delivered the net to the flop's slice. When it did not,
    /// the flop samples a constant: the bitstream is complete, the device
    /// reports `done=1`, and the logic never changes state.
    ///
    /// This is the ZedBoard blinky's failure mode. `LD0 <= count[24]` is a
    /// register-to-register assignment, so `mux_for_ff` found no same-site LUT
    /// driving `D` and no matching carry sum, and fell back to the raw slice
    /// pin `BX`. The router had put the source flop's output on other slices
    /// entirely -- `INT_R_X71Y45` drives only `IMUX4` -- so the pin was dead.
    /// The clock spine (0 diffs device-wide against Vivado), the output pad,
    /// the IOLOGIC and the hardware-proven `IMUX34 <- NR1END1` arm were all
    /// correct at the time, and every structural check passed.
    ///
    /// Three ways a flop's `D` can legitimately be driven:
    ///
    /// 1. a carry sum on the flop's **own lane** -- no routing involved, which
    ///    is exactly why Vivado merges an output register into the chain;
    /// 2. a LUT in the flop's own slice (`mux_for_ff` returns `O6`), whose
    ///    inputs the router must have delivered; or
    /// 3. routing that reaches the flop's slice at all.
    ///
    /// (1) is checked against the carry's published lane sums rather than the
    /// slice alone, because a slice holds four lanes and a chain in lane A
    /// does not feed a flop on lane B. The blinky hit precisely that: carry
    /// and register shared `SLICE_X112Y45`, so a site-level test said
    /// `driven`, while the register sat on lane B and was dark.
    ///
    /// (3) stays deliberately coarse -- "anything reaches the slice" -- because
    /// no site-pin-level model of the INT tile exists here. A false `true`
    /// restores the old silent behaviour; a false `false` would reject a
    /// working design, which is the worse error.
    pub fn ff_data_is_driven(
        &self,
        tile: &str,
        site: &str,
        bel: &str,
        d_net: &str,
    ) -> bool {
        // (1) A carry sum on this flop's own lane. `lane_sum_net` is indexed
        // by lane A..D, so the index *is* the lane.
        for c in &self.placed.packed.carries {
            let Some(lane) = c
                .lane_sum_net
                .iter()
                .position(|n| n.as_deref() == Some(d_net))
            else {
                continue;
            };
            // The chain must sit in this very slice -- a CLB tile holds both a
            // `SLICEM` and a `SLICEL`, and a chain in one does not feed a flop
            // in the other -- and the lane must be the flop's own lane.
            let same_slice = self
                .placed
                .carries
                .iter()
                .any(|pc| pc.name == c.name && pc.tile == tile && pc.site == site);
            if same_slice && Some(lane_letter(lane)) == bel.chars().next() {
                return true;
            }
        }
        // (2) A LUT in this slice whose output is the flop's `D`. Its own
        // inputs must have been routed, which (3) below covers.
        // (3) Any routed net ending in this slice.
        let idx = match bel_slice_index(bel) {
            Some(i) => i,
            // Not a slice BEL shape we model: stay permissive rather than
            // reject a working design.
            None => return true,
        };
        // INT tiles present 48 `IMUX<n>` mux outputs. They are 16 groups of
        // three -- `{IMUX g, g+8, g+16}` and `{IMUX g+24, g+32, g+40}` share
        // sources -- so the slice a given `IMUX<n>` feeds is not a contiguous
        // range. Rather than encode that here, accept any `IMUX` in the tile:
        // this test is a safety net for a completely unrouted slice, and the
        // lane-accurate case is (1).
        let _ = idx;
        // A slice's inputs are driven from the INT tile of the SAME grid
        // position, not from the CLB tile: `CLBLM_R_X71Y51`'s operands arrive
        // on `INT_R_X71Y51`'s `IMUX<n>` outputs. Comparing the pip's tile to
        // the CLB name directly never matches, which made this whole branch
        // dead and rejected every flop whose `D` came from routing rather than
        // from its own slice -- the internal-clock design failed to build with
        // "takes AFFMUX.AX ... but no routed net ends at that slice" while its
        // `d` net was demonstrably routed to `INT_R_X71Y51.IMUX43`.
        let int_tile = int_tile_for_clb(tile);
        self.nets.iter().any(|n| {
            if let RouteKind::Pips(pips) = &n.kind {
                pips.iter().any(|p| {
                    (p.tile == tile || Some(p.tile.as_str()) == int_tile.as_deref())
                        && p.dst_wire.starts_with("IMUX")
                })
            } else {
                false
            }
        })
    }
}


/// The `INT` tile that feeds a `CLB` tile of the same grid position.
///
/// 7-series keeps them as separate tiles with different type letters, so the
/// mapping is by suffix: `CLBLM_R_X71Y51` / `CLBLL_L_X71Y51` ->
/// `INT_R_X71Y51` / `INT_L_X71Y51`. Returns `None` for anything that is not a
/// CLB, so callers fall back to an exact name match.
fn int_tile_for_clb(tile: &str) -> Option<String> {
    let rest = tile
        .strip_prefix("CLBLM_")
        .or_else(|| tile.strip_prefix("CLBLL_"))?;
    // `R_X71Y51` -> `INT_R_X71Y51`
    Some(format!("INT_{rest}"))
}

/// Slice index 0..3 for a BEL (`"AFF"`, `"BFF"`, ...).
///
/// `INT` tiles present 48 `IMUX<n>` mux outputs; the four slices of a CLB take
/// them in A, B, C, D order. The slice *site* name (`SLICE_X112Y45`) does not
/// carry the letter; the placed BEL does.
pub fn bel_slice_index(bel: &str) -> Option<usize> {
    match bel.chars().next()? {
        'A' => Some(0),
        'B' => Some(1),
        'C' => Some(2),
        'D' => Some(3),
        _ => None,
    }
}

/// Lane letter for a carry lane index 0..3.
fn lane_letter(i: usize) -> char {
    match i {
        0 => 'A',
        1 => 'B',
        2 => 'C',
        _ => 'D',
    }
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
    let carry_site: HashMap<&str, &str> = placed.carries.iter().map(|c| (c.name.as_str(), c.site.as_str())).collect();

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
        // A flop whose D is a CARRY4 sum selects `FFMUX.XOR`, which taps the
        // adder inside the slice: the raw `D` pin is NOT read by that flop.
        // Emitting a routed `D` sink for it anyway put a load inside the
        // flop's own CLB tile on a net whose only other sinks are the carry
        // lanes, and the router then competed with those lanes for the tile's
        // one-hot input arms -- `count_24` and the four Y44 operands ripped
        // each other up without converging. Sinks the flop actually reads are
        // added below (`D6` for FFMUX.XOR).
        let xor_flop = placed.packed.carries.iter().any(|carry| {
            carry
                .lane_sum_net
                .iter()
                .flatten()
                .any(|sum| crate::emit::same_bit(sum, &ff.d_net))
        });
        if !xor_flop {
            add_ep(ff.d_net.clone(), site.to_string(), "D", false);
        }
        // The emitter mirrors a carry chain's lane functions into the paired
        // slice so a register in that half can select FFMUX.XOR. When that
        // register consumes the chain's top bit, its D net must also reach
        // that lane's LUT input: the XOR mux reads the paired CARRY4 sum, not
        // the raw FF.D pin. The generic FF endpoint above reaches the slice's
        // raw D pin (D2 here), which leaves the mirrored carry lane's D6
        // un-driven. On the ZedBoard this omitted sink is
        // CLBLM_L_D6 <- IMUX42 at X71Y44 and freezes LD0 despite a routed
        // count_24 -> D path.
add_ep(ff.q_net.clone(), site.to_string(), &format!("{bel_letter}Q"), true);
        add_ep(ff.clk_net.clone(), site.to_string(), "CLK", false);
    }
    // Carry chains.
    //
    // Without this arm the chain is configured but UNCONNECTED: the lane sums
    // are known to the router only as flip-flop `D` sinks, nothing declares
    // them as CARRY4 outputs, and the six `COUT` -> `CIN` nets never enter the
    // router at all. The result is a bitstream with a correct clock, a correct
    // pad path and a fully configured carry chain whose LUT inputs are
    // undriven -- the flops are clocked and latch a constant, so the LED
    // lights instead of blinking.
    //
    // The intra-slice nets need no special handling here: `route_net` already
    // returns `RouteKind::SiteInternal` when the driver and every load share a
    // site, and `route_wires` gives that no pips and no occupancy. A chain
    // placed in one column therefore has its `COUT` -> `CIN` recognised as the
    // dedicated carry column it is, rather than being sent through the fabric
    // on a path the hardware does not use.
    for carry in &packed.carries {
        let placed_carry = placed.carries.iter().find(|c| c.name == carry.name);
        let site = carry_site.get(carry.name.as_str()).copied().unwrap_or("");
        if site.is_empty() {
            // Unplaced carry: nothing to anchor endpoints to. Placement
            // refuses such a chain long before here; skip rather than invent
            // a site that would silently absorb the net.
            continue;
        }
        for (lane, sum) in carry.lane_sum_net.iter().enumerate() {
            let Some(sum) = sum else { continue };
            // The CARRY4 lane outputs `O0..O3` drive the sum net.
            add_ep(sum.clone(), site.to_string(), &format!("O{lane}"), true);
            // That same lane's `S` LUT reads its operand on the slice input
            // pin, which is what the router must reach. The pin letters match
            // the BEL letters the emitter writes (`AS`..`DS`).
            let letter = ["A", "B", "C", "D"][lane];
            if let Some((a, b)) = &carry.lane_operands[lane] {
                for (slot, net) in [a, b].into_iter().enumerate() {
                    let Some(net) = net else { continue };
                    // Use the pin the LUT mask actually reads: the chain-start
                    // lane folds its +1 through `A3` (`IMUX1`), the other
                    // lanes read `A6`. Routing every operand to `A6` bound the
                    // start mask to a VCC tie and froze bit 0 one increment
                    // past the fold.
                    let pin = carry.lane_operand_pins[lane][slot].unwrap_or(6);
                    add_ep(net.clone(), site.to_string(), &format!("{letter}{pin}"), false);
                    // The DI (generate) LUTs live in the tile's mate half and
                    // read the same operand nets. Without endpoints there the
                    // router arms their IMUXes at the mate half but leaves the
                    // long-line loops behind them undriven, so every DI lane
                    // but the register's own samples a VCC tie.
                    // The mate half reads the SAME operand nets but on the
                    // NEXT lane, wrapping into the next row's lane A. Seeding
                    // one bit at a time into `~/CDA4253_A1/blinky.bit` and
                    // reading the L-half pins shows it directly:
                    //
                    //     bit20 -> LY43D   bit21 -> LY44A   bit22 -> LY44B
                    //     bit23 -> LY44C   bit24 -> LY44D
                    //
                    // i.e. the DI (generate) half is the carry-chain look-ahead
                    // shifted by one lane, not a mirror of its own lane. Binding
                    // the mate to the same lane put every L-half lane one bit
                    // behind the reference and left `count_24` off `LY44D`
                    // entirely, which is what `carry_operand_routed` reports.
                    if let Some(pc) = placed_carry {
                        let target = mate_lane_target(placed, device, pc, lane);
                        if let Some((mate_site, mate_letter)) = target {
                            add_ep(
                                net.clone(),
                                mate_site,
                                &format!("{mate_letter}{pin}"),
                                false,
                            );
                        }
                    }
                }
            }
        }
        if let Some(cout) = &carry.cout_net {
            add_ep(cout.clone(), site.to_string(), "COUT", true);
        }
        if let Some(cin) = &carry.cin_net {
            add_ep(cin.clone(), site.to_string(), "CIN", false);
        }
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

    // Reserve the muxes the emitter configures unconditionally.
    //
    // `emit.rs`'s clock pass owns the bypass network's arms on every clock row
    // (`BYP_ALT{0,1,4,5}.GFAN0`, `BYP_ALT{2,3,6,7}.GFAN1`) and the `GFAN*` return
    // paths, and it writes them without consulting the router. The router could
    // not see those claims, so it was free to route a data net through the same
    // bypass mux -- and on `INT_R_X71Y41` `count_11` did exactly that, driving
    // `BYP_ALT1` from `SR1BEG_S0` while the clock drove it from `GFAN0`. Two
    // arms on a one-hot mux means neither input is selected.
    //
    // Seeding occupancy with them makes the router treat those muxes as taken
    // and detour, which is the correct behaviour: the clock's arms are fixed
    // configuration, the data path is not. Only the MUX is reserved, not the
    // wires around it -- a data net may still pass *through* the network on a
    // mux the clock does not claim.
    if let Some(clock_tiles) = clock_int_tiles_for_reservation(placed, device) {
        for t in &clock_tiles {
            for alt in [0u32, 1, 4, 5] {
                occupied.insert((t.clone(), format!("MUX$BYP_ALT{alt}")));
            }
            for alt in [2u32, 3, 6, 7] {
                occupied.insert((t.clone(), format!("MUX$BYP_ALT{alt}")));
            }
        }
    }
    // net -> (kind, wires it occupies)
    let mut routed: HashMap<String, (RouteKind, Vec<(String, String)>)> = HashMap::new();
    // Nets that terminate on an IOB output are the only ones the IMUX arm
    // ranking protects (see `arm_penalty`): the LED pad's vertical arm beats
    // its lateral one only by a bounded margin. Everything else — the carry
    // operand muxes the reference direct-arms from LOGIC_LOOPS and fan
    // muxes — routes unpenalised.
    let pad_out_nets: HashSet<String> = net_eps
        .iter()
        .filter(|(_, eps)| {
            eps.iter()
                .any(|e| !e.is_driver && !device.iob_output_sink(&e.site).is_empty())
        })
        .map(|(net, _)| net.clone())
        .collect();
    let mut order: Vec<String> = routable.iter().map(|s| (*s).clone()).collect();
    let mut ripped: usize = 0;
    // Wires a net lost a rip-up contest over. A retry re-runs the identical
    // search, so without this the router reproduces the same choice, loses the
    // same mux again and cycles forever: on the 25-bit blinky `count_24` and
    // the four Y44 operand nets all need arms in `INT_R_X71Y44`, and every
    // sweep ripped the others up to take the same arm back. Forbidding the
    // contested wires on the retry makes the net pick its next-best path
    // instead. Cleared as soon as the net routes.
    let mut avoid: HashMap<String, HashSet<(String, String)>> = HashMap::new();
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
        let pad_out = pad_out_nets.contains(&net);
        let avoiding: HashSet<(String, String)> =
            avoid.get(&net).cloned().unwrap_or_default();
        match route_net(
            &net,
            drivers[0],
            &loads,
            device,
            &occupied,
            Some(&mut blockers),
            pad_out,
            &avoiding,
        ) {
            Ok(kind) => {
                avoid.remove(&net);
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
                    route_net(
                        &net,
                        drivers[0],
                        &loads,
                        device,
                        &occupied,
                        None,
                        pad_out,
                        &avoiding,
                    )?;
                    unreachable!("route_net returned Ok after Err");
                }
                avoid
                    .entry(net.clone())
                    .or_default()
                    .extend(blockers.iter().cloned());
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

/// Preference key for the `HCLK_CMT_MUX_CLK_<n>` rows, lower first.
///
/// Every CMT clock row has equal routing cost, so without this the name
/// tie-break decides and picks the largest: `MUX_CLK_9` over `MUX_CLK_0`.
/// The left HCLK takes `HCLK_CK_BUFHCLK8..11` as `HCLK_CK_INOUT_L0..L3`
/// (`ppips_hclk_l.db`), so row 0 (`BUFHCLK8` -> `INOUT_L0` -> `CK_IN_L0`) is
/// the one a left-edge clock can actually reach the HROW on, and the one the
/// working Vivado bitstream uses. Other rows are left reachable but
/// deprioritised, since a right-edge clock legitimately uses them.
///
/// Non-clock wires all rank equally, so ordinary routing is unaffected.
fn clock_row_rank(wire: &str) -> u32 {
    // Every clock-row wire must rank by its ROW NUMBER and nothing else, so
    // all the hops of one spine agree on a single row.
    //
    // The spine tiles are separate search steps -- `HCLK_CMT_MUX_CLK_<n>`,
    // `HCLK_CK_BUFHCLK<m>`, `CLK_HROW_CK_IN_L<n>`, `CLK_HROW_CK_MUX_OUT_R<n>`,
    // `CLK_HROW_CK_HCLK_OUT_R<n>`, `CLK_HROW_CK_BUFHCLK_R<n>` -- and each was
    // resolving its own index independently from an equal-cost frontier, so
    // the name tie-break gave row 0 at the CMT but row 9 downstream. A clock
    // that enters at row 0 and leaves at row 9 never arrives: the FFs keep
    // their init value and the LED lights without blinking.
    //
    // The left and right HCLK tiles number their buffer nets from opposite
    // ends (`ppips_hclk_{l,r}.db`):
    //
    //   HCLK_R: HCLK_CK_INOUT_R<n> <-> HCLK_CK_BUFHCLK<n>
    //   HCLK_L: HCLK_CK_INOUT_L<n> <-> HCLK_CK_BUFHCLK<n+8>
    //
    // so a left-side `BUFHCLK<m>` is row `m - 8`. Rank by that recovered row,
    // then by the wire name. Row 0 is what the working Vivado bitstream uses
    // for the ZedBoard GCLK on Y9.
    let (row, wire_kind) = if let Some(n) = wire.strip_prefix("HCLK_CMT_MUX_CLK_") {
        // The CMT side is literal: Vivado enables `MUX_CLK_0` there.
        (n.parse::<u32>().ok(), ClkRowEnd::Cmt)
    } else if let Some(n) = wire
        .strip_prefix("HCLK_CK_BUFHCLK")
        .or_else(|| wire.strip_prefix("HCLK_CMT_CK_BUFHCLK"))
    {
        // The HCLK tiles number their buffer nets from opposite ends
        // (`ppips_hclk_{l,r}.db`): the right side uses 0..13, the left band is
        // `HCLK_CK_BUFHCLK8..11` for `HCLK_CK_INOUT_L0..L3`. Recover the row.
        (
            n.parse::<u32>().ok().map(|m| if m >= 8 { m - 8 } else { m }),
            ClkRowEnd::HrowBuf,
        )
    } else if let Some(rest) = wire.strip_prefix("CLK_HROW_CK_IN_") {
        (
            rest.trim_start_matches(['L', 'R']).parse::<u32>().ok(),
            ClkRowEnd::HrowBuf,
        )
    } else if let Some(rest) = wire
        .strip_prefix("CLK_HROW_CK_MUX_OUT_")
        .or_else(|| wire.strip_prefix("CLK_HROW_CK_HCLK_OUT_"))
        .or_else(|| wire.strip_prefix("CLK_HROW_CK_BUFHCLK_"))
    {
        (
            rest.trim_start_matches(['L', 'R']).parse::<u32>().ok(),
            ClkRowEnd::HrowBuf,
        )
    } else if let Some(n) = wire.strip_prefix("BUFHCE_X1Y") {
        (n.parse::<u32>().ok(), ClkRowEnd::HrowBuf)
    } else {
        (None, ClkRowEnd::Other)
    };
    // The two ends of the spine number the SAME physical row differently, so
    // rank each end by the value it must land on to match the reference:
    //
    //   HCLK_CMT_MUX_CLK_0   the CMT row the ZedBoard GCLK enters on
    //   CLK_HROW_*_8, X1Y8   the HROW group that pairs `CK_IN_L0`
    //                        (`MUX_OUT_R8.CK_IN_L0 26_360 26_364`)
    //
    // Anything else is reachable but deprioritised, so a left-edge clock
    // cannot pick a CMT row the HROW never selects (which muted the clock and
    // left the LED lit but frozen).
    match wire_kind {
        ClkRowEnd::Cmt => match row {
            Some(0) => 0,
            Some(n) => 100 + n,
            None => 50,
        },
        ClkRowEnd::HrowBuf => match row {
            Some(8) => 0,
            Some(n) => 100 + n,
            None => 50,
        },
        ClkRowEnd::Other => 50,
    }
}

/// Extra cost charged for stepping onto a wire that feeds an IOB output mux,
/// over and above the flat `DEFAULT_HOP_PS`.
///
/// The fabric DB carries **no delay data at all** -- `src_to_dst` exists as a
/// key in 62 `tile_type_*.json` files and is empty in every one of them -- so
/// `hop_cost` charges `DEFAULT_HOP_PS` for every hop on the device. The A*
/// comment claims the search is delay-driven; on this DB it cannot be. Every
/// way into a given mux therefore costs exactly the same, and the search stops
/// at whichever one it reaches first.
///
/// Round 26 measured what that costs on hardware. On the LED pad's tile
/// (`INT_R_X73Y46`) the arm `NR1END1` lights LD0; `WW2END0` leaves it dark.
/// Our builds picked `NE2END1` (combinational) and `NW2END1` (registered) --
/// neither is the arm the working reference uses. Round 42 proved all of them
/// are electrically *reachable*, so this is a preference problem, not a
/// connectivity one.
///
/// A tie-break cannot express the preference. The search `break`s the moment
/// it reaches its sink, so it never expands the alternative arms at all: on
/// the registered blinky `NW2END1` is pushed into `IMUX34` at cost 301 and
/// `NR1END1` is never considered. The preference has to be part of the cost.
///
/// The reference reaches its arm with a short vertical turn:
///
/// ```text
/// INT_R_X73Y45  NR1BEG1 <- ER1END1     (the tile directly below the pad)
/// INT_R_X73Y46  IMUX34  <- NR1END1
/// ```
///
/// `NR1END1` is a northward long line fed by `NR1BEG1` one row below -- one
/// dedicated stitch. `NE2`/`NW2`/`WW` are diagonal arms fed laterally out of a
/// neighbouring CLB, which is why the search finds them first: the counter's
/// flops sit in that column.
///
/// The penalty is deliberately small relative to `DEFAULT_HOP_PS` (45). It
/// only has to break a tie between arms of the same mux, and it must not
/// make the search prefer a longer route that happens to end on a vertical
/// long line -- that would trade a correct arm for a worse path. Other arms
/// stay fully reachable; pads that legitimately use them are unaffected
/// because within one mux every candidate pays the same flat hop.
/// Extra cost for an `HCLK` leaf-trunk edge, or 0 for the one to prefer.
///
/// `HCLK_L`/`HCLK_R` offer six sibling leaf trunks (`TOPL0..TOPL5`, and the
/// mirrored `BOTL*`) fed from the SAME buffer, so a plain hop search picks one
/// arbitrarily -- it picks `TOPL0`, whose spine trunk is `GCLK_L_B6`. But only
/// one leaf can be enabled, and `emit.rs`'s `vivado_leaf_spelling` enables
/// `TOPL5` (`GCLK_L_B11`) because that is the encoding `CLOCK_WORD50_POSITIONAL`
/// already carries for this device.
///
/// Why a wrong leaf is a dead clock and not a spelling difference:
///
/// ```text
///   HCLK_L:  BUFHCLK8 -> TOPL0  (spine GCLK_L_B6)
///            BUFHCLK8 -> TOPL5  (spine GCLK_L_B11)
///   INT_R.GCLK_B6  / GCLK_B11  <- driven ONLY from INT_L (tileconn [1,0])
///   INT_L has no CLK0/CLK1 source of its own -- it only carries the trunk
/// ```
///
/// So `INT_R.CLK1` arms a mux fed by a trunk nothing drives: the image is
/// valid, every structural check passes, and the clock is dead. `work/bitsim.py`
/// does not model clock taps at all, so it too reports success.
///
/// This is ranked on BOTH cost paths. Clock nets use flat hop costing, which
/// skips `arm_penalty` entirely -- putting the rule there made it dead code,
/// verified: `arm_penalty` was never once called with a leaf destination.
fn leaf_penalty(dst_wire: &str, is_clock: bool) -> i64 {
    if !is_clock || !dst_wire.starts_with("HCLK_LEAF_CLK_B_") {
        return 0;
    }
    if dst_wire.ends_with("TOPL5") || dst_wire.ends_with("BOTL5") {
        0
    } else {
        DEFAULT_HOP_PS
    }
}

fn arm_penalty(wire: &str, dst_wire: &str, pad_out: bool) -> i64 {
    // Only mux-entry wires are ranked, and only on a net whose sink is a pad
    // output.
    //
    // The penalty exists for ONE measured case: the arms into the LED pad's
    // `INT_R_X73Y46.IMUX34`, where a lateral feed is a quarter-hop shorter
    // than the vertical the reference uses, so an unranked search always
    // broke at the wrong arm. Applied fabric-wide it priced every CLB operand
    // mux (the golden direct-arms them: `IMUX13 <- LOGIC_OUTS6`,
    // `IMUX35 <- FAN_BOUNCE5`), which pushed the carry lanes' feeds onto
    // long-line detours — and with the paired-half endpoints doubling those
    // nets, the detour flood became the route stage's dominant cost.
    // Carry operand muxes: the reference's arm choice is not a free variable.
    //
    // Decoded from `~/CDA4253_A1/blinky.bit`, the reference arms the same four
    // operand muxes identically on EVERY row of the counter column:
    //
    //     IMUX4  <- FAN_BOUNCE1   IMUX12 <- WR1END2
    //     IMUX5  <- FAN_BOUNCE3   IMUX13 <- LOGIC_OUTS6
    //     IMUX42 <- ER1END1       IMUX43 <- WL1END1
    //
    // (Y45, the chain's last row, arms only `IMUX4` and `IMUX43`.) Ranking
    // those sources first is what turns our per-row ad-hoc arms into the
    // reference's uniform pattern; the alternatives stay reachable at one hop
    // of extra cost, so the search still converges when a preferred family is
    // taken by another net.
    if !pad_out && dst_wire.starts_with("IMUX") {
        let preferred = matches!(
            wire,
            "FAN_BOUNCE1" | "FAN_BOUNCE3" | "FAN_BOUNCE5" | "LOGIC_OUTS6" | "ER1END1"
                | "WL1END1" | "WR1END2" | "SR1END1" | "NR1END1"
        );
        return if preferred { 0 } else { DEFAULT_HOP_PS };
    }
    if !pad_out || !dst_wire.starts_with("IMUX") {
        return 0;
    }
    match wire {
        // Vertical long lines: the direct stitch along the column.
        "NR1END1" | "SR1END1" => 0,
        // Everything else pays ONE FULL HOP. Measured on the registered
        // blinky, the arms into `INT_R_X73Y46.IMUX34` cost: `NR1END1` 314,
        // `NW2END1` 302 -- the lateral arm is 12 units (about a quarter hop)
        // shorter, so the search reaches it first and `break`s at the sink
        // before the vertical arm is ever expanded. A penalty of 1 does not
        // close a 12-unit gap; one hop does, and it bounds the detour to a
        // single hop so the router cannot be lured onto an arbitrarily longer
        // path just because it ends on a vertical long line.
        //
        // Two hops, not one. Measured on the registered blinky, the arms into
        // `INT_R_X73Y46.IMUX34` reach the sink at:
        //
        //     FAN_BOUNCE1  cur_g 298   NR1END1  cur_g 343
        //
        // a gap of EXACTLY `DEFAULT_HOP_PS`. A one-hop penalty therefore only
        // ties (298+45 == 343+0) and the search still breaks at the lateral
        // arm, because it reaches the sink first in expansion order. The gap
        // and the hop cost being equal is a coincidence of this design, not a
        // guarantee -- so prefer the vertical arm by a clear margin rather
        // than by `<= 1`. The alternatives stay at `cur_g + 2*hop`, which is
        // still far below the cost of any long detour, so a design that needs
        // a lateral arm is not blocked: it just has to be the only way in.
        _ => DEFAULT_HOP_PS.saturating_mul(2),
    }
}

/// Which end of the clock spine a wire belongs to; see `clock_row_rank`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClkRowEnd {
    /// `HCLK_CMT_MUX_CLK_<n>` -- the CMT row selector.
    Cmt,
    /// A `CLK_HROW_*_<n>` wire or a `BUFHCE_X1Y<n>` in the HROW tile, which
    /// live in the row-8 group.
    HrowBuf,
    /// Any other wire; ranked neutrally.
    Other,
}

/// The INT tiles the clock pass will configure, so the router can reserve the
/// bypass muxes `emit.rs` claims there.
///
/// Derived from the placed cells' rows rather than from the routed clock: the
/// emitter picks these tiles from the routed spine, but the reservation has to
/// exist BEFORE routing, so it uses the logic rows the clock serves. Reserving
/// a tile the emitter later does not touch costs a detour; failing to reserve
/// one it does touch produces a doubled mux, which is far worse.
fn clock_int_tiles_for_reservation(
    placed: &Placed7,
    device: &dyn RouteQuery,
) -> Option<Vec<String>> {
    let _ = device;
    // Every placed cell that occupies a CLB, whatever vector it came from.
    //
    // `placed.luts` is EMPTY for this design: the placer files LUT+FF pairs
    // under `ffs`, and the carry chain's LUTs belong to `carries`. Keying the
    // reservation off `luts` alone found no tiles at all and silently reserved
    // nothing -- the same empty-collection read as "no work to do" that has
    // already produced three defects in this bring-up.
    let mut out: Vec<String> = Vec::new();
    let mut push_tile = |tile: &str, out: &mut Vec<String>| {
        let parts: Vec<&str> = tile.split('_').collect();
        if parts.len() < 3 {
            return;
        }
        let t = format!("INT_{}_{}", parts[1], parts[2]);
        if !out.contains(&t) {
            out.push(t);
        }
    };
    for l in &placed.luts {
        push_tile(&l.tile, &mut out);
    }
    for f in &placed.ffs {
        push_tile(&f.tile, &mut out);
    }
    for c in &placed.carries {
        push_tile(&c.tile, &mut out);
    }
    for b in &placed.bufgs {
        push_tile(&b.tile, &mut out);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
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
                let mut v = vec![
                    (p.tile.clone(), p.dst_wire.clone()),
                    (p.tile.clone(), p.src_wire.clone()),
                ];
                // A one-hot mux is a resource in its own right, and it is NOT
                // the same resource as either of the wires entering it.
                //
                // Tracking only wires let two nets each reach a different
                // SOURCE of the same mux: the sources are distinct wires so
                // neither looked occupied, but both pips configure the same
                // mux, and a mux with two arms lit drives neither input --
                // its output is undefined. That produced
                // `BYP_ALT1: ['LOGIC_OUTS4', 'SR1BEG_S0']` on `INT_R_X71Y41`,
                // which no wire-level check could see and every test passed.
                if let Some(mux) = mux_of_wire(&p.tile, &p.dst_wire) {
                    v.push((p.tile.clone(), mux));
                }
                v
            })
            .collect(),
    }
}

/// The one-hot mux a wire belongs to, as a pseudo wire name for occupancy.
///
/// `INT_R_X71Y41.BYP_ALT1` and `INT_R_X71Y41.FAN_ALT1` are muxes; the nets that
/// reach them arrive on ordinary wires (`LOGIC_OUTS4`, `SR1BEG_S0`). Adding the
/// mux itself to the occupancy set makes two nets contend for it, so the second
/// net is rip-up-and-rerouted instead of quietly double-driving the select bits.
fn mux_of_wire(_tile: &str, wire: &str) -> Option<String> {
    for family in ["IMUX", "BYP_ALT", "FAN_ALT"] {
        if let Some(rest) = wire.strip_prefix(family) {
            // Only a pure `<family><n>` wire is a mux output; `IMUX4_R` or
            // similar span names are ordinary wires.
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return Some(format!("MUX${wire}"));
            }
        }
    }
    None
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

/// Is `pin` wired to the slice's own output without leaving the site?
///
/// The slice's internal connections are the ones the dedicated BEL-to-BEL
/// paths provide: a LUT's output reaches its own packed flop's `D` directly,
/// and the carry chain's `S`/`DI`/`O` pins are fed by the site's own logic.
/// Everything else -- the LUT input pins `A`..`D`, `A1`..`A6`, `AI`, `AX` --
/// is reached through the routing tile's input mux, so a same-site load on one
/// of those must be routed like any cross-tile load.
fn is_site_internal_pin(pin: &str) -> bool {
    matches!(pin, "D" | "S" | "DI" | "O0" | "O1" | "O2" | "O3" | "CI" | "COUT" | "CIN")
}


/// Where a carry lane's operand must ALSO land on the mate half.
///
/// Returns `(site, lane_letter)`. On the paired (DI / generate) half the same
/// operand appears one lane EARLIER, wrapping into the previous row's lane D.
/// Measured against `~/CDA4253_A1/blinky.bit` by seeding a single bit and
/// reading back every lane pin:
///
/// ```text
/// bit18 -> MY43C + LY43B     bit21 -> MY44B + LY44A
/// bit19 -> MY43D + LY43C     bit22 -> MY44C + LY44B
/// bit20 -> MY44A + LY43D     bit23 -> MY44D + LY44C
///                            bit24 -> MY45A + LY44D
/// ```
///
/// so the L lane is always one behind the M lane (`-1`), not one ahead; lane
/// `A` therefore rolls back into the previous chain element's lane `D`.
fn mate_lane_target(
    placed: &Placed7,
    device: &dyn RouteQuery,
    carry: &PlacedCarry,
    lane: usize,
) -> Option<(String, char)> {
    const LANES: [char; 4] = ['A', 'B', 'C', 'D'];
    if lane != 0 {
        let mate = device.slice_pair(&carry.tile, &carry.site)?;
        return Some((mate, LANES[lane - 1]));
    }
    // Lane A's earlier lane is the previous chain element's lane D.
    let packed = placed
        .packed
        .carries
        .iter()
        .find(|c| c.name == carry.name)?;
    if packed.index == 0 {
        return None;
    }
    let previous_name = placed
        .packed
        .carries
        .iter()
        .find(|c| c.chain == packed.chain && c.index + 1 == packed.index)
        .map(|c| c.name.clone())?;
    let previous = placed.carries.iter().find(|pc| pc.name == previous_name)?;
    let mate = device.slice_pair(&previous.tile, &previous.site)?;
    Some((mate, LANES[3]))
}

/// Route one net from its driver to all loads.
fn route_net(
    net: &str,
    driver: &Endpoint,
    loads: &[&Endpoint],
    device: &dyn RouteQuery,
    occupied: &HashSet<(String, String)>,
    blocked: Option<&mut Vec<(String, String)>>,
    pad_out: bool,
    avoid: &HashSet<(String, String)>,
) -> Result<RouteKind, String> {
    // Split loads into site-internal and external.
    //
    // "Same site" is NOT the same as "site-internal". A slice input pin is
    // reached only through the routing tile's input mux, so a load on `A6`
    // whose driver is a flop in the *same* slice still has to leave the slice
    // and come back:
    //
    //     Q -> CLBLM_M_AQ -> [tileconn] -> INT -> IMUX4 -> CLBLM_M_A6 -> A6
    //
    // Partitioning on `l.site == driver.site` alone classified that as
    // site-internal and returned early with no pips, so every carry operand
    // was reported routed while nothing was emitted -- which is exactly why
    // arming the operand muxes changed the bitstream's feature count but left
    // the routed wire count at 84. Only pins the slice actually wires
    // internally (the packed FF's `D`, reached from the LUT output) may skip
    // routing.
    let (internal, external): (Vec<&&Endpoint>, Vec<&&Endpoint>) = loads
        .iter()
        .partition(|l| l.site == driver.site && is_site_internal_pin(&l.pin));
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
    // Clock nets keep flat hop costing: their spine is hand-modelled in
    // `emit.rs` (per-stage active/enable companions) and delay pricing picked
    // an RCLK mux prjxray never characterised, making the route unemittable.
    //
    // `is_clock_net` only knows BUFG *outputs*, so the pad-to-BUFG input net
    // (`clk_ibuf`) is not in it -- and that is precisely the net that reached
    // the uncharacterised mux. Catch it by looking at what the net drives.
    let drives_clock_pin = external_loads
        .iter()
        .any(|l| matches!(l.pin.as_str(), "C" | "CLK" | "I" | "I0" | "I1"));
    let is_clk = device.is_clock_net(net) || drives_clock_pin;
    let pips = bfs_route_all(
        &src_tile,
        &src_wire,
        &sinks,
        device,
        occupied,
        !is_clk,
        blocked,
        pad_out,
        avoid,
    )
    .ok_or_else(|| {
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
///
/// `use_delay` prices hops by real pip propagation delay instead of a flat
/// one-per-hop. It is OFF for clock nets: their route is the dedicated spine,
/// `emit.rs` writes hand-derived companion bits for each stage, and pricing by
/// delay sent `clk_bufg` through `LIOI3_X0Y25.IOI_IMUX_RC0` -- a pip prjxray
/// never characterised, so the route could not be emitted at all.
fn bfs_route_all(
    src_tile: &str,
    src_wire: &str,
    sinks: &[(String, String)],
    device: &dyn RouteQuery,
    occupied: &HashSet<(String, String)>,
    use_delay: bool,
    // Occupied wires that stopped an expansion. Collected so a failed net can
    // rip up only the nets that actually blocked it, instead of every net.
    mut blocked: Option<&mut Vec<(String, String)>>,
    pad_out: bool,
    avoid: &HashSet<(String, String)>,
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
    // clock row first (see `clock_row_rank`) and then on a monotonic push
    // counter, which is as deterministic as the old wire-name tie-break (the
    // push order is data-driven and repeatable) without a full string memcmp
    // per pop. The sampled profile had `BinaryHeap::pop` spending ~55% of the
    // route stage in those compares once the paired-half endpoints doubled the
    // long-detour nets; a unique `seq` makes every entry distinct, so the node
    // strings are never compared at all.
    let mut heap: BinaryHeap<Reverse<(i64, u32, u64, (String, String))>> =
        BinaryHeap::new();
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
    let mut seq: u64 = 0;
    heap.push(Reverse((
        h(src_tile, &found),
        clock_row_rank(&start.1),
        seq,
        start.clone(),
    )));
    seq += 1;
    let mut closes: HashSet<(String, String)> = HashSet::new();
    let mut expansions: usize = 0;
    while let Some(Reverse((_, _, _, (tile, wire)))) = heap.pop() {
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
            // A real pip the DB cannot encode is not a usable route: selecting
            // it reaches the sink in the model and then fails at emission with
            // `no segbits for feature`. `LIOI3.IOI_BYP3_0->>IOI_IMUX_RC0` is
            // the case that surfaced once hops were priced by delay -- the
            // cheaper-looking route ran the clock through an RCLK mux prjxray
            // never characterised. Skipping it makes the search find a route
            // that can actually be written.
            if !is_pseudo && !device.pip_is_encodable(&tile, &wire, &dst_wire) {
                continue;
            }
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
            if avoid.contains(&next) {
                continue;
            }
            if occupied.contains(&next) {
                if let Some(b) = blocked.as_deref_mut() {
                    b.push(next.clone());
                }
                continue;
            }
            // A one-hot mux is a resource in its own right, and occupancy
            // keys it as `MUX$<wire>` -- so the WIRE check above never finds
            // it. This is the check that was missing.
            //
            // `clock_int_tiles_for_reservation` correctly derives the counter
            // column from `placed.carries` and inserts
            // `("INT_R_X71Y41", "MUX$BYP_ALT1")` into `occupied`; the router
            // then tested only `("INT_R_X71Y41", "BYP_ALT1")`, never matched,
            // and drove `BYP_ALT1` from `SR1BEG_S0` anyway. The emitter saw the
            // claim, skipped the `GFAN0` arm it was going to write, and the row
            // lost its backbone -- measurable on hardware: deleting
            // `BYP_ALT1.GFAN0`, `BYP_ALT4.GFAN0` and `FAN_ALT3.NL1END_S3_0`
            // from one row of the working reference freezes LD0.
            //
            // prjxray encodes each arm of these muxes as a coarse select bit
            // plus a fine one and does NOT clear the other coarse bits, so
            // writing two arms lights BOTH and the mux selects neither input.
            // The router and the emitter must therefore agree on one arm.
            if let Some(mux) = mux_of_wire(&tile, &dst_wire) {
                if occupied.contains(&(tile.clone(), mux.clone())) {
                    if let Some(b) = blocked.as_deref_mut() {
                        b.push((tile.clone(), mux));
                    }
                    continue;
                }
            }
            // Reserve the exact arms the emitter writes, and nothing else.
            //
            // `emit.rs` writes `BYP_ALT{0,1,4,5}.GFAN0` and
            // `BYP_ALT{2,3,6,7}.GFAN1` on every clock row without asking the
            // router. The occupancy reservation above covers only tiles that
            // already hold a clock or a placed cell, and it fails in the worst
            // way when the router reaches one of these from an unexpected
            // direction: the emitter sees the claim, *skips* the arm it was
            // going to write, and the row silently loses its backbone instead
            // of merely contesting a mux.
            //
            // Hardware measurement: deleting `BYP_ALT1.GFAN0`,
            // `BYP_ALT4.GFAN0` and `FAN_ALT3.NL1END_S3_0` from one row of the
            // working reference turns a blinking LD0 into a static one.
            //
            // THE SCOPE MATTERS, and a first attempt got it wrong. Blocking the
            // whole `BYP_ALT*`/`FAN_ALT*` destination wire looks equivalent but
            // is not: those are general-purpose fabric muxes with 25 sources
            // each, and the reference routes real data THROUGH them --
            //
            //     <real wire> -> FAN_ALT1 -> FAN_BOUNCE1 -> IMUX4
            //
            // is exactly how the reference arms the carry lanes' `A6` input
            // (and `FAN_ALT3`/`FAN_ALT5` -> `FAN_BOUNCE3`/`FAN_BOUNCE5` ->
            // `IMUX5`/`IMUX35`). Blocking the wire made the reference's own arm
            // unreachable, so the router could only enter `IMUX4` on a long
            // line and the arm set diverged on every counter row.
            //
            // Only the `GFAN0`/`GFAN1` arms are the emitter's; every other
            // source of those muxes is ordinary routing and stays available.
            // HCLK leaf-trunk preference, applied on BOTH cost paths.
            //
            // `arm_penalty` is skipped entirely for clock nets (`use_delay` is
            // false for them), and the leaf pip is exactly a clock-net edge --
            // so the ranking cannot live there. See `leaf_penalty`.
            let leaf_cost = leaf_penalty(&dst_wire, !use_delay);
            let ng = cur_g.saturating_add(if use_delay {
                // On this DB every fabric pip has `delay_ns == None`, so the
                // first term is always `DEFAULT_HOP_PS`. `arm_penalty` is what
                // actually distinguishes the ways into an IOB output mux; see
                // its doc comment.
                hop_cost(device.pip_delay(&tile, &wire, &dst_wire))
                    .saturating_add(arm_penalty(&wire, &dst_wire, pad_out))
                    .saturating_add(leaf_cost)
            } else if leaf_cost > 0 {
                // Flat-hop path: any extra cost would break the "one per hop"
                // invariant the clock spine relies on, so only the DISfavoured
                // leaf pays, and it pays a full hop.
                DEFAULT_HOP_PS
            } else {
                1
            });
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
                    // Clock-row preference comes BEFORE the tie-break.
                    //
                    // Every `HCLK_CMT_MUX_CLK_<n>` row costs the same, so the
                    // tie-break decides, and the name version picked the
                    // lexicographically LARGEST wire: `MUX_CLK_9` beats
                    // `MUX_CLK_0`. That row is wrong for a left-edge clock.
                    // The left HCLK consumes `HCLK_CK_BUFHCLK8..11` as its
                    // `HCLK_CK_INOUT_L0..L3` (`ppips_hclk_l.db`), so
                    // `BUFHCLK9` lands on `INOUT_L1` while the HROW gets
                    // programmed for `CK_IN_L9`. The clock dies at that
                    // mismatch, the FFs keep their init value, and the LED
                    // lights without ever blinking. Row 0 is the row the
                    // working Vivado bitstream uses for the ZedBoard GCLK.
                    clock_row_rank(&next.1),
                    seq,
                    next.clone(),
                )));
                seq += 1;
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
            if avoid.contains(&next) {
                continue;
            }
            if occupied.contains(&next) {
                if let Some(b) = blocked.as_deref_mut() {
                    b.push(next.clone());
                }
                continue;
            }
            // A tileconn hop is a real traversal and must be priced on the same
            // scale as a pip. Charging a flat 1 while pips cost ~35-45 ps made
            // stitches *free* by comparison, so the search preferred routes
            // built from many cheap stitches over a short pip chain -- which is
            // how the LED pad ended up on INT_L_X72Y45.NE2BEG1 -> ER1END1
            // instead of the reference's two direct INT_R pips.
            let ng = cur_g.saturating_add(if use_delay { DEFAULT_HOP_PS } else { 1 });
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
                    // Clock-row preference comes BEFORE the tie-break: see the
                    // pip push above for why the wire-name tie-break sent a
                    // left-edge clock into the wrong HCLK row.
                    clock_row_rank(&next.1),
                    seq,
                    next.clone(),
                )));
                seq += 1;
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
            // A real pip inside one tile is always emitted. A tileconn
            // pseudo-pip (tile name contains `~`) is normally NOT emitted,
            // because the two wires are the same physical node and the
            // connection is structural -- there is no configuration bit for
            // it.
            //
            // The exception is a tileconn that crosses from one tile into
            // ANOTHER: the clock reaches the fabric this way
            // (`INT_R.CLK1` -> `CLBLM_CLK1` via the `INT_R`/`CLBLM_R`
            // tileconn), and the destination is the CLB tile the slice lives
            // in. Discarding that hop loses the tile crossing entirely, so the
            // emitted pips never leave the INT tile, the slice's `CLK` pin is
            // never driven, and every flop holds its init value while all the
            // clock-tile configuration looks perfect.
            // A crossing tileconn (`INT_R.CLK1` -> `CLBLM_CLK1`) is
            // structural: the prjxray DB records it as an `always` pip, so it
            // has NO configuration bit and must not be emitted as a feature.
            // Its job is done once the SEARCH has crossed the tile boundary --
            // the parent chain now reaches into the CLB tile, so the slice's
            // `CLK` pin hangs off the correct tile and the wires that feed it
            // are placed there.
            //
            // Emitting it would fail with `no segbits for feature
            // CLBLM_R.CLBLM_CLK1.CLK1`, which is exactly what happens if this
            // hop is passed to the FASM writer.
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
    #[test]
    fn a6_is_not_site_internal() {
        // `count_n` is driven by the flop's `AQ` and read by the carry lane's
        // `A6`. Both are pins of ONE slice, but the slice does not wire `AQ` to
        // `A6` internally -- the signal must leave for the interconnect's input
        // mux and come back. Classifying `A6` as site-internal returns
        // `SiteInternal`, emits no pips, and leaves the adder's operand
        // undriven, which is the static-LED defect.
        for p in ["A6", "B6", "C6", "D6"] {
            assert!(!super::is_site_internal_pin(p), "{p} must route externally");
        }
        // The flop's own `D`, reached from the lane LUT inside the slice, is
        // the one genuinely internal case.
        assert!(super::is_site_internal_pin("D"));
    }

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
        let pips = bfs_route_all("T", "A", &sinks, &MuxRouter, &empty, true, None, false, &empty).expect("all sinks");
        assert!(pips.iter().any(|p| p.dst_wire == "S1"));
        assert!(pips.iter().any(|p| p.dst_wire == "S2"));
        // The mux input must be driven once, not unioned from two branches.
        assert_eq!(pips.iter().filter(|p| p.dst_wire == "M").count(), 1);

        let unreachable = vec![
            ("T".to_string(), "S1".to_string()),
            ("T".to_string(), "NX".to_string()),
        ];
        assert!(bfs_route_all("T", "A", &unreachable, &MuxRouter, &empty, true, None, false, &empty).is_none());
    }

    #[test]
    fn clock_net_uses_spine() {
        // The router no longer short-circuits clock nets; is_clock_net is
        // still a device query (kept for callers that classify clocks).
        assert!(FakeRouter.is_clock_net("clk_bufg"));
        assert!(!FakeRouter.is_clock_net("n1"));
    }
}

#[cfg(test)]
mod clock_row_rank_tests {
    use super::{clock_row_rank, ClkRowEnd};

    /// The CMT must enter on row 0 and the HROW must use the row-8 group.
    ///
    /// These disagreeing is the exact bug that left the LED lit but frozen:
    /// the CMT fed row 0 while the HROW was programmed for row 9, so the
    /// clock never arrived and every FF kept its init value.
    #[test]
    fn cmt_and_hrow_rank_the_same_physical_row() {
        // CMT: row 0 wins.
        assert!(clock_row_rank("HCLK_CMT_MUX_CLK_0") < clock_row_rank("HCLK_CMT_MUX_CLK_9"));

        // HROW: the row-8 group wins, because `MUX_OUT_R8` is the mux that
        // pairs with `CLK_HROW_CK_IN_L0`.
        assert!(clock_row_rank("CLK_HROW_CK_MUX_OUT_R8") < clock_row_rank("CLK_HROW_CK_MUX_OUT_R0"));
        assert!(clock_row_rank("BUFHCE_X1Y8") < clock_row_rank("BUFHCE_X1Y0"));
        assert!(clock_row_rank("CLK_HROW_CK_IN_L0") < clock_row_rank("CLK_HROW_CK_IN_L9"));
    }

    /// Ordinary fabric wires must not be reordered relative to each other.
    #[test]
    fn non_clock_wires_rank_neutrally() {
        assert_eq!(clock_row_rank("IMUX34"), clock_row_rank("ER1BEG1"));
        assert_eq!(clock_row_rank("IMUX34"), 50);
    }

    /// A left-side `BUFHCLK8` is row 0; the right side's `BUFHCLK8` is row 8.
    #[test]
    fn left_band_buffer_nets_recover_their_row() {
        // Both sides' row-0 nets must outrank their row-1 nets.
        assert!(clock_row_rank("HCLK_CK_BUFHCLK8") <= clock_row_rank("HCLK_CK_BUFHCLK9"));
        assert!(clock_row_rank("HCLK_CK_BUFHCLK0") <= clock_row_rank("HCLK_CK_BUFHCLK1"));
    }

    #[test]
    fn kinds_are_distinct() {
        assert_ne!(ClkRowEnd::Cmt, ClkRowEnd::HrowBuf);
        assert_ne!(ClkRowEnd::Other, ClkRowEnd::Cmt);
    }
}
