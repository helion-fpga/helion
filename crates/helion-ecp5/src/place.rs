//! Place stage: [`PackedEcp5`] (place-local mirror) -> [`PlacedEcp5`].
//!
//! ECP5-aware placement on the Trellis tile grid of the LFE5U-85F:
//!
//! - IOs go to the PIO site of their package pin. The pin -> PIO-bel rule
//!   (`pio_bel_candidates`) was derived empirically from the vendored
//!   `iodb.json`/`tilegrid.json` and verified: it maps 100% of the 947
//!   package pins across all four vendored packages (CABGA381, CABGA554,
//!   CABGA756, CSFBGA285) to real tilegrid PIO sites
//!   (`pin_bel_rule_covers_all_packages` test). Top edge (`row == 0`):
//!   bel_row = 19 - pio_index; bottom edge (`row == 95`): bel_row =
//!   114 - pio_index; left/right edge: bel_row = row + pio_index, except
//!   the two corner DQS groups at row 92 which use row - 3 + pio_index
//!   (pio_index: A=0, B=1, C=2, D=3).
//! - EBR instances take deduplicated unique EBR bels (212 on the 85F;
//!   the tilegrid aliases each bel across adjacent tile entries, so the
//!   pool is keyed by `(name, bel_col, bel_row)`).
//! - DSP instances take DSP bels grouped by owner tile: the two
//!   `MULT18_*` bels of one tile may be used together (two independent
//!   18x18 multipliers per sysDSP slice); any other bel-kind combination
//!   in one tile blocks the tile (v1 conservative rule).
//! - PLLs take the 6 unique PLL bels.
//! - ALU/carry chains occupy vertically adjacent slices: bit `i` goes to
//!   slice `i % 4` (SLICE_A..D) of PFU row `start + i / 4` in one grid
//!   column, one bit per slice with the whole slice locked (the slice's
//!   CCU2 carry unit spans both LUTs, so the second half is not safely
//!   reusable in v1). The longest consecutive PLC2 column run on the 85F
//!   is 11 PFUs, so v1 chains are limited to 44 bits; longer chains fail
//!   loudly.
//! - Remaining LUT/FF logic is seeded greedily near the IO centroid and
//!   improved by deterministic simulated annealing (xorshift64, fixed
//!   seed) on bounding-box half-perimeter wirelength, with incremental
//!   delta-cost evaluation. The two halves of one slice must agree on
//!   their FF control nets (clk, ce, sr): `SLICEx.CEMUX` is per-slice in
//!   the PLC2 bits.db, so sharing is required for CE and is the safe
//!   choice for CLK/LSR either way.
//! - Clock planning: every distinct FF clock net and every PLL output net
//!   is assigned to one of the 16 DCC clock muxes (DCC0..DCC15). The
//!   canonical tap is the top-middle one (TMID_0 tile); quadrant, tap
//!   column, and spine come from `globals.json` via [`Device::globals`].
//!   More than 16 clock nets fails loudly.
//!
//! ## Interface notes for the coordinator / route worker
//!
//! - `pub fn place(device: &Device, packed: &PackedEcp5) ->
//!   Result<PlacedEcp5, String>` is the exact entry point. The `Backend`
//!   trait wiring (`type Placed = place::PlacedEcp5`, calling
//!   `place(self.device(), packed)`) is left to the coordinator because
//!   `backend.rs` is owned by another worker.
//! - `place::PackedEcp5` is a MINIMAL LOCAL MIRROR of the pack worker's
//!   packed representation (`pack.rs`, parallel worker, not yet landed in
//!   this worktree). It covers exactly the documented concepts: LUT4+init,
//!   FF, ALU/carry chain, EBR/DSP/PLL instances with parameters, IOs with
//!   package pins. Field names are my choice; the coordinator must unify
//!   them with the real `pack::PackedEcp5` at integration.
//! - `place::PlacedEcp5` is the REAL placed type (re-exported as
//!   `helion_ecp5::PlacedEcp5` from `lib.rs`). The route worker reads
//!   [`PlacedEcp5`]: every primitive carries a [`SiteAddr`] (tile grid
//!   row/col, tile type, site name, bel coordinates taken verbatim from
//!   the tilegrid site entries). LUT/FF placements additionally carry
//!   `half` (0/1: which LUT/FF pair of the slice).
//! - Canonical host tile rule (v1): a bel listed in several tile entries
//!   is owned by the entry whose grid column equals the bel column;
//!   ties break by smallest grid row, then smallest grid column, then
//!   smallest tile type name. For DSP this selects the `MIB2_DSPx` tile
//!   over `MIB_DSPx`. This is deterministic but NOT verified against
//!   nextpnr-ecp5: the bitstream worker must confirm the tile choice
//!   against real nextpnr textcfg before hardware use. The route worker
//!   must use the recorded `tile_type` consistently for wire lookup.
//! - Site addresses use Trellis tilegrid keys: tile (row, col) with
//!   `tile_type`, e.g. `(11, 100, "PLC2", "SLICE_A")`.
//! - Clock mapping: [`PlacedEcp5::clocks`] maps each clock net to its DCC
//!   site, quadrant, tap, and spine. The route worker drives the clock
//!   source net into that DCC's `CLKI` (see the `G_TDCCnCLKI` muxes in the
//!   TMID_0 bits.db); note the bits.db names the site `DCC_Tn` while the
//!   tilegrid site is `DCCn`.

use crate::device::{Device, SiteClass};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Packed mirror (place-local; the coordinator unifies this with pack.rs)
// ---------------------------------------------------------------------------

/// Minimal mirror of the pack worker's `PackedEcp5`.
///
/// The pack worker (parallel, `pack.rs`) owns the real definition; this
/// mirror covers only the concepts placement needs. Field names are the
/// place worker's choice and must be reconciled at integration.
#[derive(Clone, Debug, Default)]
pub struct PackedEcp5 {
    /// Top module name, carried through the stages.
    pub top: String,
    /// Package name as in `iodb.json`, e.g. `"CABGA381"`.
    pub package: String,
    /// Packed LUT4s.
    pub luts: Vec<PackedLut>,
    /// Packed flip-flops.
    pub ffs: Vec<PackedFf>,
    /// Packed IOs (each bound to a package pin).
    pub ios: Vec<PackedIo>,
    /// Inferred block RAMs.
    pub ebrs: Vec<PackedEbr>,
    /// Inferred DSP instances.
    pub dsps: Vec<PackedDsp>,
    /// Inferred PLLs.
    pub plls: Vec<PackedPll>,
    /// ALU/carry chains; `PackedLut.carry` indexes into this vec.
    pub carry_chains: Vec<CarryChain>,
}

/// One packed LUT4.
#[derive(Clone, Debug)]
pub struct PackedLut {
    /// Instance name.
    pub name: String,
    /// LUT4 truth table.
    pub init: u16,
    /// Nets on LUT inputs A, B, C, D (`None` = unconnected).
    pub inputs: [Option<String>; 4],
    /// Output net.
    pub output: String,
    /// `Some` when this LUT is a bit of an ALU/carry chain.
    pub carry: Option<CarryBit>,
}

/// One bit of an ALU/carry chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CarryBit {
    /// Index into [`PackedEcp5::carry_chains`].
    pub chain: usize,
    /// Bit position in the chain (0 = LSB).
    pub seq: usize,
}

/// One ALU/carry chain.
#[derive(Clone, Debug)]
pub struct CarryChain {
    /// Chain name.
    pub name: String,
    /// Carry-in source.
    pub cin: CarryIn,
}

/// Carry-in source of a chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CarryIn {
    /// Constant 0.
    Zero,
    /// Constant 1.
    One,
}

/// One packed flip-flop.
#[derive(Clone, Debug)]
pub struct PackedFf {
    /// Instance name.
    pub name: String,
    /// D input net.
    pub d: String,
    /// Q output net.
    pub q: String,
    /// Clock net (must be non-empty; becomes a global clock).
    pub clk: String,
    /// Clock-enable net, if any.
    pub ce: Option<String>,
    /// Set/reset net, if any.
    pub sr: Option<String>,
}

/// IO direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoDir {
    /// Input buffer.
    In,
    /// Output buffer.
    Out,
    /// Bidirectional buffer.
    Inout,
}

/// One packed IO.
#[derive(Clone, Debug)]
pub struct PackedIo {
    /// Instance name.
    pub name: String,
    /// Top-level port name.
    pub port: String,
    /// Direction.
    pub dir: IoDir,
    /// Fabric net.
    pub net: String,
    /// Package pin name, e.g. `"A10"` (must exist in `iodb.json`).
    pub pin: String,
}

/// Block-RAM parameters (v1 minimal set).
#[derive(Clone, Debug)]
pub struct EbrParams {
    /// Port A data width.
    pub width_a: u32,
    /// Port B data width.
    pub width_b: u32,
}

/// One inferred block RAM (one 18Kb sysMEM block).
#[derive(Clone, Debug)]
pub struct PackedEbr {
    /// Instance name.
    pub name: String,
    /// Parameters.
    pub params: EbrParams,
    /// Nets touching the block (for wirelength).
    pub nets: Vec<String>,
}

/// DSP instance kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DspKind {
    /// 18x18 multiplier (uses one `MULT18_*` bel).
    Mult18x18,
    /// 9x9 multiplier (uses one `MULT9_*` bel).
    Mult9x9,
    /// 54-bit ALU (uses one `ALU54_*` bel).
    Alu54,
    /// 24-bit ALU (uses one `ALU24_*` bel).
    Alu24,
}

impl DspKind {
    /// Trellis bel-name prefix for this kind.
    fn bel_prefix(self) -> &'static str {
        match self {
            DspKind::Mult18x18 => "MULT18_",
            DspKind::Mult9x9 => "MULT9_",
            DspKind::Alu54 => "ALU54_",
            DspKind::Alu24 => "ALU24_",
        }
    }
}

/// One inferred DSP instance.
#[derive(Clone, Debug)]
pub struct PackedDsp {
    /// Instance name.
    pub name: String,
    /// Kind (selects the bel).
    pub kind: DspKind,
    /// Nets touching the block (for wirelength).
    pub nets: Vec<String>,
}

/// One inferred PLL.
#[derive(Clone, Debug)]
pub struct PackedPll {
    /// Instance name.
    pub name: String,
    /// Reference clock input net.
    pub ref_net: String,
    /// Output clock nets (each becomes a global clock).
    pub out_nets: Vec<String>,
}

// ---------------------------------------------------------------------------
// Placed types (the real PlacedEcp5)
// ---------------------------------------------------------------------------

/// Concrete site address on the Trellis tile grid.
///
/// `tile_row`/`tile_col` are grid coordinates (rows `0..=95`, cols
/// `0..=126`), `tile_type` is the Trellis tile type (e.g. `"PLC2"`),
/// `site` the site name (e.g. `"SLICE_A"`, `"EBR0"`, `"PIOA"`), and
/// `bel_col`/`bel_row` the Trellis bel coordinates copied verbatim from
/// the tilegrid site entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SiteAddr {
    /// Grid row of the owning tile.
    pub tile_row: u32,
    /// Grid column of the owning tile.
    pub tile_col: u32,
    /// Trellis tile type, e.g. `"PLC2"`.
    pub tile_type: String,
    /// Site name, e.g. `"SLICE_A"`.
    pub site: String,
    /// Bel column from the tilegrid site entry.
    pub bel_col: i32,
    /// Bel row from the tilegrid site entry.
    pub bel_row: i32,
}

/// A placed LUT4.
#[derive(Clone, Debug)]
pub struct PlacedLut {
    /// Instance name.
    pub name: String,
    /// LUT4 truth table.
    pub init: u16,
    /// Nets on LUT inputs A, B, C, D.
    pub inputs: [Option<String>; 4],
    /// Output net.
    pub output: String,
    /// Carry-chain membership, if any.
    pub carry: Option<CarryBit>,
    /// Site address.
    pub site: SiteAddr,
    /// Which LUT of the slice (0 or 1).
    pub half: u8,
}

/// A placed flip-flop.
#[derive(Clone, Debug)]
pub struct PlacedFf {
    /// Instance name.
    pub name: String,
    /// D input net.
    pub d: String,
    /// Q output net.
    pub q: String,
    /// Clock net.
    pub clk: String,
    /// Clock-enable net, if any.
    pub ce: Option<String>,
    /// Set/reset net, if any.
    pub sr: Option<String>,
    /// Site address (same slice as its half).
    pub site: SiteAddr,
    /// Which FF of the slice (0 or 1).
    pub half: u8,
}

/// A placed IO.
#[derive(Clone, Debug)]
pub struct PlacedIo {
    /// Instance name.
    pub name: String,
    /// Top-level port name.
    pub port: String,
    /// Direction.
    pub dir: IoDir,
    /// Fabric net.
    pub net: String,
    /// Package pin name.
    pub pin: String,
    /// PIO site address.
    pub site: SiteAddr,
}

/// A placed block RAM.
#[derive(Clone, Debug)]
pub struct PlacedEbr {
    /// Instance name.
    pub name: String,
    /// Parameters (carried from packing).
    pub params: EbrParams,
    /// Nets touching the block.
    pub nets: Vec<String>,
    /// EBR site address.
    pub site: SiteAddr,
}

/// A placed DSP instance.
#[derive(Clone, Debug)]
pub struct PlacedDsp {
    /// Instance name.
    pub name: String,
    /// Kind.
    pub kind: DspKind,
    /// Nets touching the block.
    pub nets: Vec<String>,
    /// DSP bel site address.
    pub site: SiteAddr,
}

/// A placed PLL.
#[derive(Clone, Debug)]
pub struct PlacedPll {
    /// Instance name.
    pub name: String,
    /// Reference clock input net.
    pub ref_net: String,
    /// Output clock nets.
    pub out_nets: Vec<String>,
    /// PLL site address.
    pub site: SiteAddr,
}

/// A placed ALU/carry chain.
#[derive(Clone, Debug)]
pub struct PlacedCarryChain {
    /// Chain name.
    pub name: String,
    /// Carry-in source.
    pub cin: CarryIn,
    /// LUT instance names in bit order (bit 0 first); their sites are in
    /// [`PlacedEcp5::luts`].
    pub bits: Vec<String>,
}

/// One global clock assignment: clock net -> DCC clock mux.
#[derive(Clone, Debug)]
pub struct ClockAssignment {
    /// Clock net name.
    pub net: String,
    /// DCC site name, e.g. `"DCC0"`.
    pub dcc: String,
    /// Canonical DCC site address (top-middle tap, TMID_0 tile).
    pub site: SiteAddr,
    /// Clock quadrant from `globals.json` (`"UL"`, `"UR"`, `"LL"`, `"LR"`).
    pub quadrant: String,
    /// Tap column from `globals.json` (e.g. `"C58"`).
    pub tap: String,
    /// Spine name from `globals.json` (e.g. `"UL58"`).
    pub spine: String,
}

/// Placed packed netlist for the LFE5U-85F: a concrete device site for
/// every packed primitive, plus the clock-net -> global-clock mapping for
/// the route worker.
#[derive(Clone, Debug, Default)]
pub struct PlacedEcp5 {
    /// Top module name, carried through the stages.
    pub top: String,
    /// Package name, carried through the stages.
    pub package: String,
    /// The packed input (mirror), kept so later stages can read parameters.
    pub packed: PackedEcp5,
    /// Placed LUT4s.
    pub luts: Vec<PlacedLut>,
    /// Placed flip-flops.
    pub ffs: Vec<PlacedFf>,
    /// Placed IOs.
    pub ios: Vec<PlacedIo>,
    /// Placed block RAMs.
    pub ebrs: Vec<PlacedEbr>,
    /// Placed DSP instances.
    pub dsps: Vec<PlacedDsp>,
    /// Placed PLLs.
    pub plls: Vec<PlacedPll>,
    /// Placed carry chains.
    pub carry_chains: Vec<PlacedCarryChain>,
    /// Clock-net -> global clock (DCC) assignments for the route worker.
    pub clocks: Vec<ClockAssignment>,
}

// ---------------------------------------------------------------------------
// Site pools
// ---------------------------------------------------------------------------

/// One deduplicated bel with its canonical owner tile.
#[derive(Clone, Debug)]
struct BelSite {
    name: String,
    bel_col: i32,
    bel_row: i32,
    tile_row: u32,
    tile_col: u32,
    tile_type: String,
    used: bool,
}

impl BelSite {
    fn addr(&self) -> SiteAddr {
        SiteAddr {
            tile_row: self.tile_row,
            tile_col: self.tile_col,
            tile_type: self.tile_type.clone(),
            site: self.name.clone(),
            bel_col: self.bel_col,
            bel_row: self.bel_row,
        }
    }
}

/// Pick the canonical owner tile for a bel listed in several tile entries.
///
/// Preference order: (1) grid column equals the bel column (the bel's own
/// column, not an adjacent tile's); (2) smallest grid row; (3) smallest
/// grid column; (4) smallest tile type name. Deterministic; see the
/// module docs for the verification caveat.
fn pick_owner(bel_col: i32, hosts: &[HostTile]) -> HostTile {
    let mut best: Option<(bool, u32, u32, &str)> = None;
    for (row, col, tt) in hosts {
        let own_col = (*col as i32) == bel_col;
        let key = (own_col, *row, *col, tt.as_str());
        // `bool` orders false < true, so compare with reverse on own_col.
        let better = match best {
            None => true,
            Some((b_own, b_row, b_col, b_tt)) => {
                (key.0 && !b_own)
                    || (key.0 == b_own && (key.1, key.2, key.3) < (b_row, b_col, b_tt))
            }
        };
        if better {
            best = Some((own_col, *row, *col, tt.as_str()));
        }
    }
    // `hosts` is never empty: it was built from a real site entry.
    let (_, row, col, tt) = best.unwrap_or((false, 0, 0, ""));
    (row, col, tt.to_string())
}

/// A bel key: (site name, bel_col, bel_row).
type BelKey = (String, i32, i32);
/// A host tile entry: (grid row, grid col, tile type).
type HostTile = (u32, u32, String);

/// Collect deduplicated bels of the wanted classes with canonical owners,
/// in a single pass over the tilegrid.
///
/// Returns one pool per requested class; each pool is sorted by
/// `(bel_col, bel_row, name)` for deterministic allocation order.
fn collect_bels(device: &Device, classes: &[SiteClass]) -> HashMap<SiteClass, Vec<BelSite>> {
    let mut hosts: HashMap<(SiteClass, BelKey), Vec<HostTile>> = HashMap::new();
    for tile in device.tiles() {
        for site in &tile.sites {
            if classes.contains(&site.class) {
                hosts
                    .entry((site.class, (site.name.clone(), site.bel_col, site.bel_row)))
                    .or_default()
                    .push((tile.row, tile.col, tile.tile_type.clone()));
            }
        }
    }
    let mut pools: HashMap<SiteClass, Vec<BelSite>> = HashMap::new();
    for ((class, (name, bel_col, bel_row)), hs) in hosts {
        let (tile_row, tile_col, tile_type) = pick_owner(bel_col, &hs);
        pools.entry(class).or_default().push(BelSite {
            name,
            bel_col,
            bel_row,
            tile_row,
            tile_col,
            tile_type,
            used: false,
        });
    }
    for bels in pools.values_mut() {
        bels.sort_by(|a, b| (a.bel_col, a.bel_row, &a.name).cmp(&(b.bel_col, b.bel_row, &b.name)));
    }
    pools
}

/// First free bel in the pool, marked used. Fails loudly when the pool is
/// exhausted.
fn alloc_bel(pool: &mut [BelSite], what: &str, name: &str) -> Result<SiteAddr, String> {
    let bel = pool
        .iter_mut()
        .find(|b| !b.used)
        .ok_or_else(|| format!("place: out of {what} sites for {name}"))?;
    bel.used = true;
    Ok(bel.addr())
}

/// Candidate PIO bel positions for a package pin's `(col, row, pio)` from
/// `iodb.json`, in preference order.
///
/// Derived empirically from the vendored DB (see module docs): top edge
/// (`row == 0`) bels sit at bel_row 19/18; bottom edge (`row == 95`) at
/// 114/113; left/right edge bels at `row + pio_index`, except the two
/// corner DQS groups at row 92 which use `row - 3 + pio_index`.
/// `pio_index`: A=0, B=1, C=2, D=3.
fn pio_bel_candidates(col: u32, row: u32, pio: &str) -> Vec<(i32, i32)> {
    let idx: i32 = match pio {
        "A" => 0,
        "B" => 1,
        "C" => 2,
        "D" => 3,
        _ => return Vec::new(),
    };
    let c = col as i32;
    if row == 0 {
        vec![(c, 19 - idx)]
    } else if row == 95 {
        vec![(c, 114 - idx)]
    } else {
        let r = row as i32;
        vec![(c, r + idx), (c, r - 3 + idx)]
    }
}

/// One physical sysDSP slice: 10 bels sharing one `(x, band)` key.
///
/// On the LFE5U-85F there are 78 such slices (verified by test): each
/// holds two `MULT18_*` (or `MULT9_*`) bels, two `PRADD*` bels, and one
/// `ALU54_*`/`ALU24_*` bel. The bels' numeric suffixes are 0..3 in one
/// slice column and 4..7 in the next; the slice key removes the suffix
/// offset so the 10 bels group together.
struct DspSlice {
    bels: Vec<BelSite>,
    /// Bel-name prefix claimed by placed instances (`None` = slice free).
    taken: Option<String>,
}

/// Physical-slice key for a DSP bel name like `MULT18_0` or `ALU54_7`.
/// `None` for names outside the known scheme (they become singleton
/// slices; the structure test fails loudly if the DB ever grows one).
fn dsp_slice_key(name: &str, bel_col: i32, bel_row: i32) -> Option<(i32, i32)> {
    let num: i32 = name.rsplit('_').next()?.parse().ok()?;
    let off = match num {
        0 | 4 => 0,
        1 | 5 => 1,
        2 | 6 => 2,
        3 | 7 => 3,
        _ => return None,
    };
    let x = bel_col - off;
    let band = bel_row - ((bel_row - 1) % 24);
    Some((x, band))
}

/// Group DSP bels by physical sysDSP slice, sorted by slice key for
/// deterministic allocation.
fn group_dsp_slices(bels: Vec<BelSite>) -> Vec<DspSlice> {
    let mut by_slice: HashMap<(i32, i32), Vec<BelSite>> = HashMap::new();
    let mut singletons: Vec<BelSite> = Vec::new();
    for b in bels {
        match dsp_slice_key(&b.name, b.bel_col, b.bel_row) {
            Some(key) => by_slice.entry(key).or_default().push(b),
            None => singletons.push(b),
        }
    }
    let mut keys: Vec<(i32, i32)> = by_slice.keys().copied().collect();
    keys.sort();
    let mut slices: Vec<DspSlice> = keys
        .into_iter()
        .map(|k| {
            let mut bels = by_slice.remove(&k).unwrap_or_default();
            bels.sort_by(|a, b| a.name.cmp(&b.name));
            DspSlice { bels, taken: None }
        })
        .collect();
    for b in singletons {
        slices.push(DspSlice {
            bels: vec![b],
            taken: None,
        });
    }
    slices
}

/// Allocate a DSP bel for `kind`.
///
/// The two `MULT18_*` bels of one slice may be used together (two
/// independent 18x18 multipliers per sysDSP slice); any other bel-kind
/// combination in one slice is blocked (v1 conservative rule). Fails
/// loudly when no slice has room.
fn alloc_dsp(slices: &mut [DspSlice], kind: DspKind, name: &str) -> Result<SiteAddr, String> {
    let prefix = kind.bel_prefix();
    for slice in slices.iter_mut() {
        // Skip slices already claimed by an incompatible kind.
        if let Some(taken) = &slice.taken {
            if taken != prefix {
                continue;
            }
        }
        if let Some(bel) = slice
            .bels
            .iter_mut()
            .find(|b| !b.used && b.name.starts_with(prefix))
        {
            bel.used = true;
            slice.taken = Some(prefix.to_string());
            return Ok(bel.addr());
        }
    }
    Err(format!(
        "place: out of DSP sites for {name} (kind {kind:?})"
    ))
}

// ---------------------------------------------------------------------------
// Logic placement (seed + simulated annealing)
// ---------------------------------------------------------------------------

/// Deterministic PRNG (xorshift64) so placement is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next() % n as u64) as usize
    }
}

/// One half-slice slot: PFU index, slice 0..3 (A..D), half 0..1.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct HalfSlot {
    pfu: usize,
    slice: u8,
    half: u8,
}

/// FF control nets of a unit: (clk, ce, sr). The two halves of a slice must
/// agree (`SLICEx.CEMUX` is per-slice; sharing is the safe choice for
/// CLK/LSR either way).
type Ctrl = (String, Option<String>, Option<String>);

/// One annealable unit: at most one LUT and one FF sharing a half-slice.
struct Unit {
    lut: Option<usize>,
    ff: Option<usize>,
    nets: Vec<String>,
    ctrl: Option<Ctrl>,
    locked: bool,
}

fn ctrl_compatible(a: Option<&Ctrl>, b: Option<&Ctrl>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// Pair each FF with the LUT driving its D net (no exclusivity required:
/// an ECP5 FF's D may come from routing, so sharing the half-slice never
/// conflicts with the LUT output fanning out elsewhere).
fn pair_lut_ff(packed: &PackedEcp5) -> (Vec<Option<usize>>, Vec<bool>) {
    let mut lut_by_output: HashMap<&str, usize> = HashMap::new();
    for (li, lut) in packed.luts.iter().enumerate() {
        lut_by_output.entry(lut.output.as_str()).or_insert(li);
    }
    let mut ff_lut = vec![None; packed.ffs.len()];
    let mut lut_paired = vec![false; packed.luts.len()];
    for (fi, ff) in packed.ffs.iter().enumerate() {
        if let Some(&li) = lut_by_output.get(ff.d.as_str()) {
            if !lut_paired[li] {
                ff_lut[fi] = Some(li);
                lut_paired[li] = true;
            }
        }
    }
    (ff_lut, lut_paired)
}

/// Consecutive PLC2 column runs: (col, start_row, len).
fn pfu_column_runs(pfu_tiles: &[(u32, u32)]) -> Vec<(u32, u32, u32)> {
    let mut by_col: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(row, col) in pfu_tiles {
        by_col.entry(col).or_default().push(row);
    }
    let mut runs = Vec::new();
    let mut cols: Vec<u32> = by_col.keys().copied().collect();
    cols.sort();
    for col in cols {
        let mut rows = by_col[&col].clone();
        rows.sort();
        let mut start = rows[0];
        let mut prev = rows[0];
        for &r in &rows[1..] {
            if r == prev + 1 {
                prev = r;
            } else {
                runs.push((col, start, prev - start + 1));
                start = r;
                prev = r;
            }
        }
        runs.push((col, start, prev - start + 1));
    }
    runs
}

/// Bounding-box half-perimeter cost of one net's units, in grid units.
fn net_cost(members: &[usize], slot_of: &[HalfSlot], pfu_pos: &[(u32, u32)]) -> i64 {
    if members.len() < 2 {
        return 0;
    }
    let (mut r0, mut r1, mut c0, mut c1) = (u32::MAX, 0u32, u32::MAX, 0u32);
    for &ui in members {
        let (row, col) = pfu_pos[slot_of[ui].pfu];
        r0 = r0.min(row);
        r1 = r1.max(row);
        c0 = c0.min(col);
        c1 = c1.max(col);
    }
    (r1 - r0) as i64 + (c1 - c0) as i64
}

// ---------------------------------------------------------------------------
// Clock planning
// ---------------------------------------------------------------------------

/// Number of DCC clock muxes (DCC0..DCC15) available for global clocks.
const NUM_DCC: usize = 16;

/// Numeric index of a `"DCCn"` site name.
fn dcc_index(name: &str) -> Option<usize> {
    name.strip_prefix("DCC")?.parse().ok()
}

fn quadrant_of(globals: &crate::db::Globals, col: u32, row: u32) -> Result<String, String> {
    for (name, q) in &globals.quadrants {
        if col >= q.x0 && col <= q.x1 && row >= q.y0 && row <= q.y1 {
            return Ok(name.clone());
        }
    }
    Err(format!(
        "place: tile ({row}, {col}) is in no clock quadrant"
    ))
}

fn tap_of(globals: &crate::db::Globals, col: u32) -> Result<String, String> {
    for (name, t) in &globals.taps {
        if col >= t.lx0 && col <= t.rx1 {
            return Ok(name.clone());
        }
    }
    Err(format!("place: column {col} is in no clock tap range"))
}

/// Distinct clock nets: FF clock nets in first-seen order, then PLL output
/// nets. An empty FF clock net is a pack bug and fails loudly.
fn clock_net_list(packed: &PackedEcp5) -> Result<Vec<String>, String> {
    let mut clock_nets: Vec<String> = Vec::new();
    for ff in &packed.ffs {
        if ff.clk.is_empty() {
            return Err(format!("place: FF {} has an empty clock net", ff.name));
        }
        if !clock_nets.contains(&ff.clk) {
            clock_nets.push(ff.clk.clone());
        }
    }
    for pll in &packed.plls {
        for n in &pll.out_nets {
            if !clock_nets.contains(n) {
                clock_nets.push(n.clone());
            }
        }
    }
    Ok(clock_nets)
}

/// Assign each clock net to a DCC clock mux.
///
/// Each net from [`clock_net_list`] gets the next free DCC0..DCC15 at its
/// top-middle tap; quadrant, tap, and spine come from `globals.json`.
/// Fails loudly when there are more clock nets than DCCs.
fn plan_clocks(device: &Device, clock_nets: Vec<String>) -> Result<Vec<ClockAssignment>, String> {
    if clock_nets.len() > NUM_DCC {
        return Err(format!(
            "place: {} clock nets exceed the {NUM_DCC} DCC global clocks ({:?})",
            clock_nets.len(),
            clock_nets
        ));
    }

    // Canonical bel per DCC name: the tap whose owner tile has the
    // smallest grid row (the top-middle TMID_0 tap).
    let dcc_bels = collect_bels(device, &[SiteClass::Dcc])
        .remove(&SiteClass::Dcc)
        .unwrap_or_default();
    let mut by_name: HashMap<String, Vec<&BelSite>> = HashMap::new();
    for b in &dcc_bels {
        by_name.entry(b.name.clone()).or_default().push(b);
    }
    let mut dcc_names: Vec<String> = by_name.keys().cloned().collect();
    dcc_names.sort_by_key(|n| dcc_index(n).unwrap_or(usize::MAX));
    if dcc_names.len() < NUM_DCC {
        return Err(format!(
            "place: device has only {} DCC sites, need {NUM_DCC}",
            dcc_names.len()
        ));
    }

    let globals = device.globals();
    let mut out = Vec::new();
    for (net, dcc_name) in clock_nets.into_iter().zip(dcc_names.into_iter()) {
        let bels = &by_name[&dcc_name];
        let bel = bels
            .iter()
            .min_by_key(|b| (b.tile_row, b.tile_col))
            .ok_or_else(|| format!("place: DCC {dcc_name} has no bel"))?;
        let quadrant = quadrant_of(globals, bel.tile_col, bel.tile_row)?;
        let tap = tap_of(globals, bel.tile_col)?;
        let tap_num = tap.strip_prefix('C').unwrap_or(&tap);
        let spine = format!("{quadrant}{tap_num}");
        if !globals.spines.contains_key(&spine) {
            return Err(format!(
                "place: spine {spine} for DCC {dcc_name} missing from globals.json"
            ));
        }
        out.push(ClockAssignment {
            net,
            dcc: dcc_name.clone(),
            site: bel.addr(),
            quadrant,
            tap,
            spine,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// place()
// ---------------------------------------------------------------------------

/// Place a packed ECP5 netlist onto the LFE5U-85F tile grid.
///
/// `packed` is the place-local mirror of the pack worker's `PackedEcp5`
/// (see the module docs); the coordinator unifies it with `pack.rs` at
/// integration. Every failure mode (unknown package/pin, double-used pin,
/// exhausted EBR/DSP/PLL/DCC sites, unplaceable carry chain, empty clock
/// net) returns `Err`: nothing is silently clipped or dropped.
///
/// Placement is deterministic: pools iterate in sorted order and the
/// annealer uses a fixed-seed xorshift64.
pub fn place(device: &Device, packed: &PackedEcp5) -> Result<PlacedEcp5, String> {
    let mut out = PlacedEcp5 {
        top: packed.top.clone(),
        package: packed.package.clone(),
        packed: packed.clone(),
        ..Default::default()
    };

    // ---- PFU tile inventory -------------------------------------------------
    let mut pfu_idx: Vec<usize> = device.tiles_of_type("PLC2").to_vec();
    pfu_idx.sort_by(|&a, &b| {
        let ta = &device.tiles()[a];
        let tb = &device.tiles()[b];
        (ta.col, ta.row).cmp(&(tb.col, tb.row))
    });
    let mut pfu_tiles: Vec<(u32, u32)> = Vec::with_capacity(pfu_idx.len());
    // Slice bel coordinates per PFU, slices A..D.
    let mut slice_bels: Vec<[(i32, i32); 4]> = Vec::with_capacity(pfu_idx.len());
    for &i in &pfu_idx {
        let tile = &device.tiles()[i];
        pfu_tiles.push((tile.row, tile.col));
        let mut bels = [(0i32, 0i32); 4];
        for (si, sname) in ["SLICE_A", "SLICE_B", "SLICE_C", "SLICE_D"]
            .iter()
            .enumerate()
        {
            let site = tile
                .sites
                .iter()
                .find(|s| s.name == *sname)
                .ok_or_else(|| {
                    format!(
                        "place: PLC2 tile ({}, {}) lacks {sname}",
                        tile.row, tile.col
                    )
                })?;
            bels[si] = (site.bel_col, site.bel_row);
        }
        slice_bels.push(bels);
    }
    let pfu_index: HashMap<(u32, u32), usize> = pfu_tiles
        .iter()
        .enumerate()
        .map(|(i, &rc)| (rc, i))
        .collect();
    let slice_site = |pfu: usize, slice: u8| -> SiteAddr {
        let (row, col) = pfu_tiles[pfu];
        let (bel_col, bel_row) = slice_bels[pfu][slice as usize];
        SiteAddr {
            tile_row: row,
            tile_col: col,
            tile_type: "PLC2".to_string(),
            site: ["SLICE_A", "SLICE_B", "SLICE_C", "SLICE_D"][slice as usize].to_string(),
            bel_col,
            bel_row,
        }
    };

    // ---- 1. IOs to package pins ---------------------------------------------
    let pin_map: HashMap<String, crate::db::PackagePin> = if packed.ios.is_empty() {
        HashMap::new()
    } else {
        device
            .package_pins(&packed.package)
            .ok_or_else(|| {
                format!(
                    "place: unknown package {:?}; vendored packages: CABGA381, CABGA554, CABGA756, CSFBGA285",
                    packed.package
                )
            })?
            .clone()
    };
    // One tilegrid pass builds every hard-macro pool.
    let mut pools = collect_bels(
        device,
        &[
            SiteClass::Pio,
            SiteClass::Ebr,
            SiteClass::Dsp,
            SiteClass::Pll,
        ],
    );
    let mut pio_bels = pools.remove(&SiteClass::Pio).unwrap_or_default();
    let mut pio_by_bel: HashMap<(i32, i32, String), usize> = HashMap::new();
    for (i, b) in pio_bels.iter().enumerate() {
        pio_by_bel.insert((b.bel_col, b.bel_row, b.name.clone()), i);
    }
    let mut anchor_sum = (0u64, 0u64);
    for io in &packed.ios {
        let pin = pin_map.get(&io.pin).ok_or_else(|| {
            format!(
                "place: package pin {} (port {}) not in package {:?}",
                io.pin, io.port, packed.package
            )
        })?;
        let mut addr = None;
        for (bc, br) in pio_bel_candidates(pin.col, pin.row, &pin.pio) {
            let key = (bc, br, format!("PIO{}", pin.pio));
            if let Some(&bi) = pio_by_bel.get(&key) {
                if pio_bels[bi].used {
                    return Err(format!(
                        "place: package pin {} (PIO site {}) used twice",
                        io.pin, pio_bels[bi].name
                    ));
                }
                pio_bels[bi].used = true;
                addr = Some(pio_bels[bi].addr());
                break;
            }
        }
        let site = addr.ok_or_else(|| {
            format!(
                "place: no PIO site for package pin {} (col {}, row {}, pio {})",
                io.pin, pin.col, pin.row, pin.pio
            )
        })?;
        anchor_sum.0 += site.tile_row as u64;
        anchor_sum.1 += site.tile_col as u64;
        out.ios.push(PlacedIo {
            name: io.name.clone(),
            port: io.port.clone(),
            dir: io.dir,
            net: io.net.clone(),
            pin: io.pin.clone(),
            site,
        });
    }
    let n_io = out.ios.len();
    let (anchor_row, anchor_col) = if n_io == 0 {
        (48.0f64, 63.0f64)
    } else {
        (
            anchor_sum.0 as f64 / n_io as f64,
            anchor_sum.1 as f64 / n_io as f64,
        )
    };

    // ---- 2. EBR / DSP / PLL ---------------------------------------------------
    let mut ebr_pool = pools.remove(&SiteClass::Ebr).unwrap_or_default();
    for ebr in &packed.ebrs {
        let site = alloc_bel(&mut ebr_pool, "EBR", &ebr.name)?;
        out.ebrs.push(PlacedEbr {
            name: ebr.name.clone(),
            params: ebr.params.clone(),
            nets: ebr.nets.clone(),
            site,
        });
    }
    let mut dsp_slices = group_dsp_slices(pools.remove(&SiteClass::Dsp).unwrap_or_default());
    for dsp in &packed.dsps {
        let site = alloc_dsp(&mut dsp_slices, dsp.kind, &dsp.name)?;
        out.dsps.push(PlacedDsp {
            name: dsp.name.clone(),
            kind: dsp.kind,
            nets: dsp.nets.clone(),
            site,
        });
    }
    let mut pll_pool = pools.remove(&SiteClass::Pll).unwrap_or_default();
    for pll in &packed.plls {
        let site = alloc_bel(&mut pll_pool, "PLL", &pll.name)?;
        out.plls.push(PlacedPll {
            name: pll.name.clone(),
            ref_net: pll.ref_net.clone(),
            out_nets: pll.out_nets.clone(),
            site,
        });
    }

    // ---- 3. Carry chains into vertically adjacent slices ----------------------
    // Bit i -> slice (i % 4) of PFU row (start + i / 4), one bit per slice,
    // whole slice locked (the CCU2 carry unit spans both LUTs).
    const MAX_CHAIN_BITS: usize = 44; // 11-PFU longest PLC2 column run x 4
    let mut chain_luts: HashMap<usize, Vec<usize>> = HashMap::new();
    for (li, lut) in packed.luts.iter().enumerate() {
        if let Some(cb) = lut.carry {
            if cb.chain >= packed.carry_chains.len() {
                return Err(format!(
                    "place: LUT {} references carry chain {} but only {} chains exist",
                    lut.name,
                    cb.chain,
                    packed.carry_chains.len()
                ));
            }
            chain_luts.entry(cb.chain).or_default().push(li);
        }
    }
    let mut chain_ids: Vec<usize> = chain_luts.keys().copied().collect();
    chain_ids.sort();
    let runs = pfu_column_runs(&pfu_tiles);
    // lut index -> locked half-slot, for chain bits.
    let mut carry_slot: HashMap<usize, HalfSlot> = HashMap::new();
    let mut locked_slices: HashSet<(usize, u8)> = HashSet::new();
    for cid in chain_ids {
        let chain = &packed.carry_chains[cid];
        let mut lis = chain_luts[&cid].clone();
        lis.sort_by_key(|&li| packed.luts[li].carry.map(|c| c.seq).unwrap_or(usize::MAX));
        for (s, &li) in lis.iter().enumerate() {
            if packed.luts[li].carry != Some(CarryBit { chain: cid, seq: s }) {
                return Err(format!(
                    "place: carry chain {} ({}) has non-contiguous bit sequence",
                    cid, chain.name
                ));
            }
        }
        let nbits = lis.len();
        if nbits > MAX_CHAIN_BITS {
            return Err(format!(
                "place: carry chain {} ({}) has {nbits} bits, max {MAX_CHAIN_BITS} fit one PLC2 column run",
                cid, chain.name
            ));
        }
        let need_pfu = nbits.div_ceil(4) as u32;
        let mut best: Option<(f64, u32, u32)> = None;
        for &(col, start, len) in &runs {
            if len < need_pfu {
                continue;
            }
            let center = start as f64 + (need_pfu as f64 - 1.0) / 2.0;
            let score = (center - anchor_row).abs() + (col as f64 - anchor_col).abs();
            let better = match best {
                None => true,
                Some((bs, bc, br)) => score < bs || (score == bs && (col, start) < (bc, br)),
            };
            if better {
                best = Some((score, col, start));
            }
        }
        let (_, col, start_row) = best.ok_or_else(|| {
            format!(
                "place: no PLC2 column run of {need_pfu} PFUs for carry chain {} ({})",
                cid, chain.name
            )
        })?;
        let mut bits = Vec::with_capacity(nbits);
        for (i, &li) in lis.iter().enumerate() {
            let row = start_row + (i / 4) as u32;
            let slice = (i % 4) as u8;
            let pfu = *pfu_index.get(&(row, col)).ok_or_else(|| {
                format!(
                    "place: PLC2 tile ({row}, {col}) missing for carry chain {}",
                    chain.name
                )
            })?;
            let slot = HalfSlot {
                pfu,
                slice,
                half: 0,
            };
            if !locked_slices.insert((pfu, slice)) {
                return Err(format!(
                    "place: carry chain {} ({}) collides on slice {slice} of PFU ({row}, {col})",
                    cid, chain.name
                ));
            }
            carry_slot.insert(li, slot);
            bits.push(packed.luts[li].name.clone());
        }
        out.carry_chains.push(PlacedCarryChain {
            name: chain.name.clone(),
            cin: chain.cin,
            bits,
        });
    }

    // ---- 4. Logic units: seed + simulated annealing ---------------------------
    // Clock nets ride the dedicated DCC global network; excluding them
    // from the wirelength cost is both principled and what keeps
    // annealing linear (a shared clock would otherwise put every unit in
    // one net's bbox).
    let clock_nets = clock_net_list(packed)?;
    let clock_set: HashSet<String> = clock_nets.iter().cloned().collect();
    let (ff_lut, lut_paired) = pair_lut_ff(packed);
    let mut units: Vec<Unit> = Vec::new();
    let mut lut_unit = vec![usize::MAX; packed.luts.len()];
    // FF units first (a carry LUT's paired FF rides the locked half-slice).
    for (fi, ff) in packed.ffs.iter().enumerate() {
        let mut nets = vec![ff.d.clone(), ff.q.clone(), ff.clk.clone()];
        nets.extend(ff.ce.clone());
        nets.extend(ff.sr.clone());
        let mut lut = None;
        let mut locked = false;
        if let Some(li) = ff_lut[fi] {
            let plut = &packed.luts[li];
            nets.extend(plut.inputs.iter().flatten().cloned());
            nets.push(plut.output.clone());
            lut = Some(li);
            lut_unit[li] = units.len();
            locked = plut.carry.is_some();
        }
        units.push(Unit {
            lut,
            ff: Some(fi),
            nets,
            ctrl: Some((ff.clk.clone(), ff.ce.clone(), ff.sr.clone())),
            locked,
        });
    }
    // Unpaired LUTs (including unpaired carry LUTs: locked).
    for (li, lut) in packed.luts.iter().enumerate() {
        if lut_paired[li] {
            continue;
        }
        let mut nets: Vec<String> = lut.inputs.iter().flatten().cloned().collect();
        nets.push(lut.output.clone());
        lut_unit[li] = units.len();
        units.push(Unit {
            lut: Some(li),
            ff: None,
            nets,
            ctrl: None,
            locked: lut.carry.is_some(),
        });
    }

    let n_units = units.len();
    let mut slot_of: Vec<HalfSlot> = vec![
        HalfSlot {
            pfu: 0,
            slice: 0,
            half: 0
        };
        n_units
    ];
    // Slice occupancy: slice index (pfu*4 + slice) -> [half0 unit, half1 unit].
    let mut slice_units: Vec<[Option<usize>; 2]> = vec![[None, None]; pfu_tiles.len() * 4];
    // Place locked (carry) units.
    for (ui, u) in units.iter().enumerate() {
        if !u.locked {
            continue;
        }
        let li = u
            .lut
            .ok_or_else(|| format!("place: internal error: locked unit {ui} has no LUT"))?;
        let slot = *carry_slot.get(&li).ok_or_else(|| {
            format!(
                "place: internal error: carry LUT {} has no locked slot",
                packed.luts[li].name
            )
        })?;
        slot_of[ui] = slot;
        let si = slot.pfu * 4 + slot.slice as usize;
        slice_units[si][slot.half as usize] = Some(ui);
    }

    // Free half-slots, anchor-ordered, with a rotating cursor for the seed.
    let mut free: Vec<HalfSlot> = Vec::new();
    for pfu in 0..pfu_tiles.len() {
        for slice in 0..4u8 {
            if locked_slices.contains(&(pfu, slice)) {
                continue;
            }
            for half in 0..2u8 {
                free.push(HalfSlot { pfu, slice, half });
            }
        }
    }
    free.sort_by_key(|s| {
        let (row, col) = pfu_tiles[s.pfu];
        ((row as f64 - anchor_row).abs() + (col as f64 - anchor_col).abs()) as i64
    });
    let slice_ok = |slice_units: &[[Option<usize>; 2]], si: usize, units: &[Unit]| -> bool {
        match slice_units[si] {
            [Some(a), Some(b)] => ctrl_compatible(units[a].ctrl.as_ref(), units[b].ctrl.as_ref()),
            _ => true,
        }
    };

    let free_units: Vec<usize> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| !u.locked)
        .map(|(ui, _)| ui)
        .collect();
    if free_units.len() > free.len() {
        return Err(format!(
            "place: design needs {} logic half-slices but only {} are free ({} PFU tiles)",
            free_units.len(),
            free.len(),
            pfu_tiles.len()
        ));
    }
    let mut cursor = 0usize;
    for &ui in &free_units {
        let mut placed_at = None;
        for i in 0..free.len() {
            let idx = (cursor + i) % free.len();
            let s = free[idx];
            let si = s.pfu * 4 + s.slice as usize;
            // Tentatively occupy to test the slice-mate constraint.
            slice_units[si][s.half as usize] = Some(ui);
            if slice_ok(&slice_units, si, &units) {
                slot_of[ui] = s;
                placed_at = Some(idx);
                break;
            }
            slice_units[si][s.half as usize] = None;
        }
        let idx = placed_at.ok_or_else(|| {
            format!("place: no compatible half-slice for unit {ui} (slice control-net conflict)")
        })?;
        // O(1) removal; the anchor ordering is only a seed heuristic, so
        // disturbing the tail is harmless.
        free.swap_remove(idx);
        cursor = idx % free.len().max(1);
    }

    // Net -> units, for wirelength (clock nets excluded, see above).
    let mut net_units: HashMap<String, Vec<usize>> = HashMap::new();
    for (ui, u) in units.iter().enumerate() {
        for n in &u.nets {
            if clock_set.contains(n) {
                continue;
            }
            net_units.entry(n.clone()).or_default().push(ui);
        }
    }
    // Simulated annealing with incremental delta-cost, deterministic seed.
    let mut rng = Rng(0xEC85A1);
    let iters = (free_units.len() * 200).clamp(2000, 60_000);
    let mut temp = 4.0f64;
    // Reusable buffer for affected nets of a swap.
    let mut affected: Vec<String> = Vec::new();
    for it in 0..iters {
        if free_units.len() < 2 {
            break;
        }
        let a = free_units[rng.below(free_units.len())];
        let b = free_units[rng.below(free_units.len())];
        if a == b {
            continue;
        }
        affected.clear();
        for n in units[a].nets.iter().chain(units[b].nets.iter()) {
            if clock_set.contains(n) || affected.contains(n) {
                continue;
            }
            affected.push(n.clone());
        }
        let old: i64 = affected
            .iter()
            .map(|n| net_cost(&net_units[n], &slot_of, &pfu_tiles))
            .sum();
        let sa = slot_of[a];
        let sb = slot_of[b];
        // Tentative swap.
        slot_of[a] = sb;
        slot_of[b] = sa;
        let sia = sa.pfu * 4 + sa.slice as usize;
        let sib = sb.pfu * 4 + sb.slice as usize;
        slice_units[sia][sa.half as usize] = Some(b);
        slice_units[sib][sb.half as usize] = Some(a);
        let ok = slice_ok(&slice_units, sia, &units) && slice_ok(&slice_units, sib, &units);
        let new: i64 = if ok {
            affected
                .iter()
                .map(|n| net_cost(&net_units[n], &slot_of, &pfu_tiles))
                .sum()
        } else {
            i64::MAX
        };
        let delta = new.saturating_sub(old);
        let accept = ok
            && (delta <= 0 || {
                let p = (-(delta as f64) / temp).exp();
                (rng.next() % 1000) as f64 / 1000.0 < p
            });
        if !accept {
            slot_of[a] = sa;
            slot_of[b] = sb;
            slice_units[sia][sa.half as usize] = Some(a);
            slice_units[sib][sb.half as usize] = Some(b);
        }
        if it % 1000 == 999 && temp > 0.05 {
            temp *= 0.9;
        }
    }

    // ---- 5. Clock planning ------------------------------------------------------
    out.clocks = plan_clocks(device, clock_nets)?;

    // ---- 6. Emit ---------------------------------------------------------------
    for (ui, u) in units.iter().enumerate() {
        let s = slot_of[ui];
        let site = slice_site(s.pfu, s.slice);
        if let Some(li) = u.lut {
            let lut = &packed.luts[li];
            out.luts.push(PlacedLut {
                name: lut.name.clone(),
                init: lut.init,
                inputs: lut.inputs.clone(),
                output: lut.output.clone(),
                carry: lut.carry,
                site: site.clone(),
                half: s.half,
            });
        }
        if let Some(fi) = u.ff {
            let ff = &packed.ffs[fi];
            out.ffs.push(PlacedFf {
                name: ff.name.clone(),
                d: ff.d.clone(),
                q: ff.q.clone(),
                clk: ff.clk.clone(),
                ce: ff.ce.clone(),
                sr: ff.sr.clone(),
                site,
                half: s.half,
            });
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn device() -> &'static Device {
        Device::global().unwrap()
    }

    /// A blinky-scale design: 2 FFs, 2 LUTs, a 4-bit carry chain, 2 IOs,
    /// one clock.
    fn blinky_packed() -> PackedEcp5 {
        let mut luts = vec![
            PackedLut {
                name: "xor0".into(),
                init: 0x6666,
                inputs: [Some("q0".into()), Some("one".into()), None, None],
                output: "d1".into(),
                carry: None,
            },
            PackedLut {
                name: "buf0".into(),
                init: 0xAAAA,
                inputs: [Some("d1".into()), None, None, None],
                output: "led".into(),
                carry: None,
            },
        ];
        for seq in 0..4 {
            luts.push(PackedLut {
                name: format!("alu{seq}"),
                init: 0x9999,
                inputs: [Some(format!("a{seq}")), Some(format!("b{seq}")), None, None],
                output: format!("s{seq}"),
                carry: Some(CarryBit { chain: 0, seq }),
            });
        }
        PackedEcp5 {
            top: "blinky".into(),
            package: "CABGA381".into(),
            luts,
            ffs: vec![
                PackedFf {
                    name: "ff0".into(),
                    d: "d1".into(),
                    q: "q0".into(),
                    clk: "clk".into(),
                    ce: None,
                    sr: None,
                },
                PackedFf {
                    name: "ff1".into(),
                    d: "q0".into(),
                    q: "q1".into(),
                    clk: "clk".into(),
                    ce: None,
                    sr: None,
                },
            ],
            ios: vec![
                PackedIo {
                    name: "ib_clk".into(),
                    port: "clk".into(),
                    dir: IoDir::In,
                    net: "clk".into(),
                    pin: "A10".into(),
                },
                PackedIo {
                    name: "ob_led".into(),
                    port: "led".into(),
                    dir: IoDir::Out,
                    net: "led".into(),
                    pin: "A12".into(),
                },
            ],
            carry_chains: vec![CarryChain {
                name: "alu0".into(),
                cin: CarryIn::Zero,
            }],
            ..Default::default()
        }
    }

    /// Every placed site must name a real tilegrid tile, a real tile type,
    /// and a real site entry with matching bel coordinates.
    fn assert_site_legal(device: &Device, site: &SiteAddr) {
        let tile = device
            .tiles()
            .iter()
            .find(|t| {
                t.row == site.tile_row && t.col == site.tile_col && t.tile_type == site.tile_type
            })
            .unwrap_or_else(|| panic!("no such tile: {site:?}"));
        assert!(
            tile.sites.iter().any(|s| {
                s.name == site.site && s.bel_col == site.bel_col && s.bel_row == site.bel_row
            }),
            "tile ({}, {}) {} has no site {:?}",
            site.tile_row,
            site.tile_col,
            site.tile_type,
            site.site
        );
    }

    #[test]
    fn blinky_places_legally() {
        let dev = device();
        let placed = place(dev, &blinky_packed()).unwrap();
        assert_eq!(placed.top, "blinky");
        // 2 logic LUTs + 4 carry LUTs; 2 FFs; 2 IOs.
        assert_eq!(placed.luts.len(), 6);
        assert_eq!(placed.ffs.len(), 2);
        assert_eq!(placed.ios.len(), 2);

        // Every site legal against the Trellis tilegrid.
        for lut in &placed.luts {
            assert_site_legal(dev, &lut.site);
            assert!(lut.half < 2);
        }
        for ff in &placed.ffs {
            assert_site_legal(dev, &ff.site);
            assert!(ff.half < 2);
        }
        for io in &placed.ios {
            assert_site_legal(dev, &io.site);
            assert!(io.site.site.starts_with("PIO"));
        }

        // No two logic primitives share a half-slice; no two IOs share a PIO.
        let mut slots = BTreeSet::new();
        for lut in &placed.luts {
            let s = &lut.site;
            assert!(
                slots.insert((s.tile_row, s.tile_col, s.site.clone(), lut.half)),
                "half-slice shared: {s:?} half {}",
                lut.half
            );
        }
        for ff in &placed.ffs {
            let s = &ff.site;
            // An FF paired with its driver LUT shares that LUT's half-slice
            // by design; only unpaired FFs must be unique.
            let paired = placed
                .luts
                .iter()
                .any(|l| l.site == *s && l.half == ff.half && l.output == ff.d);
            if !paired {
                assert!(
                    slots.insert((s.tile_row, s.tile_col, s.site.clone(), ff.half)),
                    "half-slice shared by FF {}: {s:?}",
                    ff.name
                );
            }
        }
        let mut pios = BTreeSet::new();
        for io in &placed.ios {
            let s = &io.site;
            assert!(
                pios.insert((s.tile_row, s.tile_col, s.site.clone())),
                "PIO shared: {s:?}"
            );
        }

        // Paired FF shares its driver LUT's half-slice.
        let lut0 = placed.luts.iter().find(|l| l.name == "xor0").unwrap();
        let ff0 = placed.ffs.iter().find(|f| f.name == "ff0").unwrap();
        assert_eq!(lut0.site, ff0.site);
        assert_eq!(lut0.half, ff0.half);

        // IOs landed on their package pins' PIOs.
        let ib = placed.ios.iter().find(|i| i.port == "clk").unwrap();
        assert_eq!(ib.pin, "A10");
        // A10 = (col 63, row 0, pio A) -> bel (63, 19), tile (0, 63) PIOT0.
        assert_eq!((ib.site.bel_col, ib.site.bel_row), (63, 19));
        assert_eq!(ib.site.site, "PIOA");
        assert_eq!((ib.site.tile_row, ib.site.tile_col), (0, 63));
    }

    #[test]
    fn carry_chain_lands_in_adjacent_slices() {
        let dev = device();
        let mut packed = PackedEcp5 {
            top: "adder".into(),
            package: "CABGA381".into(),
            carry_chains: vec![CarryChain {
                name: "add8".into(),
                cin: CarryIn::One,
            }],
            ..Default::default()
        };
        for seq in 0..8 {
            packed.luts.push(PackedLut {
                name: format!("bit{seq}"),
                init: 0x9999,
                inputs: [Some(format!("a{seq}")), Some(format!("b{seq}")), None, None],
                output: format!("s{seq}"),
                carry: Some(CarryBit { chain: 0, seq }),
            });
        }
        let placed = place(dev, &packed).unwrap();
        assert_eq!(placed.carry_chains.len(), 1);
        let chain = &placed.carry_chains[0];
        assert_eq!(chain.cin, CarryIn::One);
        assert_eq!(chain.bits.len(), 8);

        // Bits in seq order occupy vertically adjacent slices: same grid
        // column, and (row * 4 + slice) consecutive.
        let mut prev: Option<(u32, u32, u8)> = None;
        for (seq, name) in chain.bits.iter().enumerate() {
            let lut = placed.luts.iter().find(|l| &l.name == name).unwrap();
            assert_eq!(lut.carry, Some(CarryBit { chain: 0, seq }));
            let slice_idx: u8 = match lut.site.site.as_str() {
                "SLICE_A" => 0,
                "SLICE_B" => 1,
                "SLICE_C" => 2,
                "SLICE_D" => 3,
                s => panic!("carry bit not in a slice: {s}"),
            };
            let pos = (lut.site.tile_col, lut.site.tile_row, slice_idx);
            if let Some((pc, pr, ps)) = prev {
                assert_eq!(pos.0, pc, "chain left its column at bit {seq}");
                assert_eq!(
                    pos.1 * 4 + pos.2 as u32,
                    pr * 4 + ps as u32 + 1,
                    "chain bits not vertically adjacent at bit {seq}"
                );
            }
            prev = Some(pos);
        }
    }

    #[test]
    fn clocks_get_dcc_assignments() {
        let dev = device();
        let mut packed = blinky_packed();
        // Second clock domain.
        packed.ffs.push(PackedFf {
            name: "ff2".into(),
            d: "d1".into(),
            q: "q2".into(),
            clk: "clk2".into(),
            ce: None,
            sr: None,
        });
        let placed = place(dev, &packed).unwrap();
        assert_eq!(placed.clocks.len(), 2);
        assert_eq!(placed.clocks[0].net, "clk");
        assert_eq!(placed.clocks[1].net, "clk2");
        // Distinct DCCs.
        assert_ne!(placed.clocks[0].dcc, placed.clocks[1].dcc);
        for ca in &placed.clocks {
            assert!(ca.dcc.starts_with("DCC"), "unexpected DCC {}", ca.dcc);
            assert_site_legal(dev, &ca.site);
            // Quadrant / tap / spine consistent with globals.json.
            let g = dev.globals();
            assert!(g.quadrants.contains_key(&ca.quadrant));
            assert!(g.taps.contains_key(&ca.tap));
            assert!(g.spines.contains_key(&ca.spine));
            // Spine name is quadrant + tap number, and the spine exists.
            let tap_num = ca.tap.strip_prefix('C').unwrap();
            assert_eq!(ca.spine, format!("{}{}", ca.quadrant, tap_num));
        }
    }

    #[test]
    fn pll_outputs_become_clocks() {
        let dev = device();
        let mut packed = blinky_packed();
        packed.plls.push(PackedPll {
            name: "pll0".into(),
            ref_net: "clk".into(),
            out_nets: vec!["pll_clk".into()],
        });
        let placed = place(dev, &packed).unwrap();
        // clk + pll_clk.
        assert_eq!(placed.clocks.len(), 2);
        assert!(placed.clocks.iter().any(|c| c.net == "pll_clk"));
        assert_eq!(placed.plls.len(), 1);
        assert_site_legal(dev, &placed.plls[0].site);
        assert_eq!(placed.plls[0].site.site, "PLL");
    }

    #[test]
    fn macros_place_on_legal_bels() {
        let dev = device();
        let packed = PackedEcp5 {
            top: "m".into(),
            package: "CABGA381".into(),
            ebrs: vec![PackedEbr {
                name: "ram0".into(),
                params: EbrParams {
                    width_a: 18,
                    width_b: 18,
                },
                nets: vec!["clk".into()],
            }],
            dsps: vec![
                PackedDsp {
                    name: "mult0".into(),
                    kind: DspKind::Mult18x18,
                    nets: vec!["clk".into()],
                },
                PackedDsp {
                    name: "mult1".into(),
                    kind: DspKind::Mult18x18,
                    nets: vec!["clk".into()],
                },
            ],
            ..Default::default()
        };
        let placed = place(dev, &packed).unwrap();
        assert_eq!(placed.ebrs.len(), 1);
        assert_site_legal(dev, &placed.ebrs[0].site);
        assert!(placed.ebrs[0].site.site.starts_with("EBR"));
        assert_eq!(placed.dsps.len(), 2);
        for dsp in &placed.dsps {
            assert_site_legal(dev, &dsp.site);
            assert!(dsp.site.site.starts_with("MULT18_"));
        }
        // The two multipliers share one DSP slice (the two MULT18 bels).
        let key = |d: &PlacedDsp| dsp_slice_key(&d.site.site, d.site.bel_col, d.site.bel_row);
        assert_eq!(key(&placed.dsps[0]), key(&placed.dsps[1]));
        assert_ne!(
            (placed.dsps[0].site.bel_col, placed.dsps[0].site.bel_row),
            (placed.dsps[1].site.bel_col, placed.dsps[1].site.bel_row)
        );
    }

    #[test]
    fn dsp_slice_structure_matches_db() {
        // The LFE5U-85F has 78 sysDSP slices of 10 bels each (2x MULT18,
        // 2x MULT9, 2x PRADD18, 2x PRADD9, 1x ALU54, 1x ALU24). This is the
        // physical grouping `alloc_dsp` relies on; fail loudly if the DB
        // ever disagrees.
        let dev = device();
        let dsp_bels = collect_bels(dev, &[SiteClass::Dsp])
            .remove(&SiteClass::Dsp)
            .unwrap_or_default();
        let slices = group_dsp_slices(dsp_bels);
        assert_eq!(slices.len(), 78);
        for s in &slices {
            assert_eq!(s.bels.len(), 10, "slice bels: {:?}", s.bels);
        }
        let total: usize = slices.iter().map(|s| s.bels.len()).sum();
        assert_eq!(total, dev.counts().dsp_bels);
    }

    #[test]
    fn over_capacity_fails_loudly() {
        let dev = device();

        // 213 EBRs > 212 bels.
        let mut packed = PackedEcp5 {
            top: "t".into(),
            package: "CABGA381".into(),
            ..Default::default()
        };
        for i in 0..213 {
            packed.ebrs.push(PackedEbr {
                name: format!("ram{i}"),
                params: EbrParams {
                    width_a: 18,
                    width_b: 18,
                },
                nets: vec![],
            });
        }
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("EBR"), "unexpected error: {err}");

        // 7 PLLs > 6 bels.
        let mut packed = PackedEcp5 {
            top: "t".into(),
            package: "CABGA381".into(),
            ..Default::default()
        };
        for i in 0..7 {
            packed.plls.push(PackedPll {
                name: format!("pll{i}"),
                ref_net: "clk".into(),
                out_nets: vec![format!("c{i}")],
            });
        }
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("PLL"), "unexpected error: {err}");

        // Unknown package pin.
        let mut packed = blinky_packed();
        packed.ios[0].pin = "ZZ99".into();
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("ZZ99"), "unexpected error: {err}");

        // Pin used twice.
        let mut packed = blinky_packed();
        packed.ios[1].pin = "A10".into();
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("twice"), "unexpected error: {err}");

        // 17 clock nets > 16 DCCs.
        let mut packed = PackedEcp5 {
            top: "t".into(),
            package: "CABGA381".into(),
            ..Default::default()
        };
        for i in 0..17 {
            packed.ffs.push(PackedFf {
                name: format!("ff{i}"),
                d: "d".into(),
                q: format!("q{i}"),
                clk: format!("clk{i}"),
                ce: None,
                sr: None,
            });
        }
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("DCC"), "unexpected error: {err}");

        // More logic than the 83640 half-slices.
        let mut packed = PackedEcp5 {
            top: "t".into(),
            package: "CABGA381".into(),
            ..Default::default()
        };
        for i in 0..83641 {
            packed.luts.push(PackedLut {
                name: format!("l{i}"),
                init: 0,
                inputs: [None, None, None, None],
                output: format!("w{i}"),
                carry: None,
            });
        }
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("half-slice"), "unexpected error: {err}");

        // Carry chain longer than the 44-bit column-run limit.
        let mut packed = PackedEcp5 {
            top: "t".into(),
            package: "CABGA381".into(),
            carry_chains: vec![CarryChain {
                name: "big".into(),
                cin: CarryIn::Zero,
            }],
            ..Default::default()
        };
        for seq in 0..45 {
            packed.luts.push(PackedLut {
                name: format!("b{seq}"),
                init: 0,
                inputs: [None, None, None, None],
                output: format!("s{seq}"),
                carry: Some(CarryBit { chain: 0, seq }),
            });
        }
        let err = place(dev, &packed).unwrap_err();
        assert!(err.contains("44"), "unexpected error: {err}");
    }

    #[test]
    fn pin_bel_rule_covers_all_packages() {
        // Cross-check against the Trellis DB: every package pin of every
        // vendored package must resolve to a real PIO site bel.
        let dev = device();
        let bels = collect_bels(dev, &[SiteClass::Pio])
            .remove(&SiteClass::Pio)
            .unwrap_or_default();
        let by_bel: BTreeSet<(i32, i32, String)> = bels
            .iter()
            .map(|b| (b.bel_col, b.bel_row, b.name.clone()))
            .collect();
        for pkg in ["CABGA381", "CABGA554", "CABGA756", "CSFBGA285"] {
            let pins = dev.package_pins(pkg).unwrap();
            assert!(!pins.is_empty());
            for (pin_name, pin) in pins {
                let mut ok = false;
                for (bc, br) in pio_bel_candidates(pin.col, pin.row, &pin.pio) {
                    if by_bel.contains(&(bc, br, format!("PIO{}", pin.pio))) {
                        ok = true;
                        break;
                    }
                }
                assert!(
                    ok,
                    "package {pkg} pin {pin_name} (col {}, row {}, pio {}) maps to no PIO bel",
                    pin.col, pin.row, pin.pio
                );
            }
        }
    }

    #[test]
    fn placement_is_deterministic() {
        let dev = device();
        let packed = blinky_packed();
        let a = place(dev, &packed).unwrap();
        let b = place(dev, &packed).unwrap();
        let sites = |p: &PlacedEcp5| {
            let mut v: Vec<(String, u32, u32, String, u8)> = p
                .luts
                .iter()
                .map(|l| {
                    (
                        l.name.clone(),
                        l.site.tile_row,
                        l.site.tile_col,
                        l.site.site.clone(),
                        l.half,
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(sites(&a), sites(&b));
        assert_eq!(a.clocks[0].dcc, b.clocks[0].dcc);
    }

    #[test]
    fn slice_control_nets_agree_within_slice() {
        // Two FFs with different clocks must never share a slice.
        let dev = device();
        let mut packed = PackedEcp5 {
            top: "t".into(),
            package: "CABGA381".into(),
            ..Default::default()
        };
        for i in 0..40 {
            packed.ffs.push(PackedFf {
                name: format!("ff{i}"),
                d: format!("d{i}"),
                q: format!("q{i}"),
                clk: format!("clk{}", i % 2),
                ce: None,
                sr: None,
            });
            packed.luts.push(PackedLut {
                name: format!("l{i}"),
                init: 0xAAAA,
                inputs: [Some(format!("d{i}")), None, None, None],
                output: format!("d{i}"),
                carry: None,
            });
        }
        let placed = place(dev, &packed).unwrap();
        // Group FFs by slice; each slice's FFs must share clk/ce/sr.
        let mut by_slice: HashMap<(u32, u32, String), Vec<&PlacedFf>> = HashMap::new();
        for ff in &placed.ffs {
            by_slice
                .entry((ff.site.tile_row, ff.site.tile_col, ff.site.site.clone()))
                .or_default()
                .push(ff);
        }
        for (slice, ffs) in &by_slice {
            for w in ffs.windows(2) {
                assert_eq!(
                    (w[0].clk.clone(), w[0].ce.clone(), w[0].sr.clone()),
                    (w[1].clk.clone(), w[1].ce.clone(), w[1].sr.clone()),
                    "slice {slice:?} mixes control nets"
                );
            }
        }
    }
}
