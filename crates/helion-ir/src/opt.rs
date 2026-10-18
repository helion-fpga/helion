//! FM-HEL-AREA-02: LUT-level peephole optimizations on the mapped netlist.
//!
//! Three passes over [`Design`] after technology mapping, driven to fixpoint
//! by [`Design::peephole_opt_luts`]:
//!
//! 1. [`Design::sweep_dead_luts`] — remove LUTs whose output net has no loads.
//!    A LUT with zero fanout cannot affect any observable signal, so this is
//!    safe by construction. No rewiring.
//! 2. [`Design::fold_lut_consts`] — a `Lut6` with `INIT = 0` / `INIT = MAX`
//!    drives a constant. When every fanout is a LUT input pin, the constant
//!    is pushed into each fanout's INIT and the fanout pin is disconnected;
//!    the const LUT is then dead and swept.
//! 3. [`Design::absorb_lut_inverters`] — a `Lut6` with `INIT = 0x5555…`
//!    computes `~I0`. When every fanout is a LUT input pin, the inversion is
//!    pushed into each fanout's INIT (address bit flipped); the inverter is
//!    then dead and swept. Logic depth shrinks by one on those paths.
//!
//! Hard safety invariants (all passes):
//! - Never touch a LUT whose output net is `mark_debug`, observed by an
//!   [`CellKind::Ila`], or a top-level output port net. Those are observable
//!   outside the netlist (debugger, ILA insertion, pins).
//! - Never touch `DONT_TOUCH` cells, and never fold/absorb through one.
//! - Fold/absorb only when EVERY fanout endpoint is a LUT `I0`–`I5` pin.
//!   Flip-flop `D`/`CLK`, IOB `I`, and other pins have no INIT to absorb
//!   into; rewiring them (bypass) is forbidden — round-1 showed bypassing the
//!   `~q0` inverter as a "buffer" silently drops the inversion and changes
//!   counter.sv simulation.
//! - `0xAAAA…` (`O = I0`, a buffer) is deliberately NOT touched: removing it
//!   would be bypass by another name.
//!
//! Complexity: each pass is O(cells + nets + endpoints) with hash maps; the
//! fixpoint driver is bounded (16 rounds; chains resolve one link per round).

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::{intern, resolve, Attrs, Cell, CellKind, Design, Endpoint, Net, PortDir};
use lasso::Spur;

/// `Lut6` INIT for `~I0` (I0 is the LSB of the LUT address).
pub const INV_INIT: u64 = 0x5555_5555_5555_5555;
/// `Lut6` INIT for identity buffer `O = I0`.
pub const BUF_INIT: u64 = 0xAAAA_AAAA_AAAA_AAAA;
/// `Lut6` INIT for 2:1 hardware mux (`O = I2 ? I1 : I0`).
pub const MUXF_INIT: u64 = 0xCACA_CACA_CACA_CACA;

/// Dedicated wide-mux hardware primitive kind (`MUXF7` / `MUXF8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MuxfKind {
    /// 7-input Shannon combine primitive (`O = S ? I1 : I0` over two LUT6 cofactors).
    Muxf7,
    /// 8-input Shannon combine primitive (`O = S ? I1 : I0` over two MUXF7 cofactors).
    Muxf8,
}

impl MuxfKind {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Muxf7 => "MUXF7",
            Self::Muxf8 => "MUXF8",
        }
    }

    #[inline]
    pub const fn bel_str(self) -> &'static str {
        match self {
            Self::Muxf7 => "F7AMUX",
            Self::Muxf8 => "F8MUX",
        }
    }
}

/// Wide-cone truth table specification for [`Design::decompose_wide_cone_muxf`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WideTruthTable {
    /// 7-input 128-bit truth table (`inputs[0..6]` on LUT6 pair, `inputs[6]` on `Muxf7` select).
    Lut7(u128),
    /// 7-input cofactor pair (`lo` when `inputs[6] == 0`, `hi` when `inputs[6] == 1`).
    Cofactors7 { lo: u64, hi: u64 },
    /// 8-input 256-bit truth table as four 64-bit cofactors `[c00, c01, c10, c11]`
    /// where `inputs[6]` is bit 0 of the cofactor index and `inputs[7]` is bit 1.
    Lut8([u64; 4]),
    /// 8-input 256-bit truth table as two 128-bit cofactors `[lo128, hi128]`.
    Lut8Words([u128; 2]),
}

impl From<u128> for WideTruthTable {
    fn from(v: u128) -> Self {
        Self::Lut7(v)
    }
}

impl From<(u64, u64)> for WideTruthTable {
    fn from((lo, hi): (u64, u64)) -> Self {
        Self::Cofactors7 { lo, hi }
    }
}

impl From<[u64; 2]> for WideTruthTable {
    fn from([lo, hi]: [u64; 2]) -> Self {
        Self::Cofactors7 { lo, hi }
    }
}

impl From<[u64; 4]> for WideTruthTable {
    fn from(c: [u64; 4]) -> Self {
        Self::Lut8(c)
    }
}

impl From<[u128; 2]> for WideTruthTable {
    fn from(w: [u128; 2]) -> Self {
        Self::Lut8Words(w)
    }
}

/// Result of lowering a 7-input or 8-input cone via [`Design::decompose_wide_cone_muxf`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WideConeDecomp {
    /// Leaf `Lut6` cells created (`2` for a 7-input cone, `4` for an 8-input cone).
    pub luts: Vec<Spur>,
    /// Intermediate/root `Muxf7` primitive cells created (`1` for 7-input, `2` for 8-input).
    pub muxf7s: Vec<Spur>,
    /// Root `Muxf8` primitive cell created (`Some` for 8-input, `None` for 7-input).
    pub muxf8: Option<Spur>,
    /// Driven output net.
    pub out_net: Spur,
}

/// Aggregate statistics from [`Design::opt_design`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptDesignStats {
    pub peephole: LutOptStats,
    pub buffers_coalesced: usize,
    pub cse_merged: usize,
    pub luts_merged: usize,
}

/// Aggregate statistics from [`Design::phys_opt_design`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysOptStats {
    pub retimed: usize,
    pub fanout_replicas: usize,
}

/// Low cofactor bitmasks for each of the 6 LUT input positions.
const COFACTOR_LO_MASK: [u64; 6] = [
    0x5555_5555_5555_5555,
    0x3333_3333_3333_3333,
    0x0F0F_0F0F_0F0F_0F0F,
    0x00FF_00FF_00FF_00FF,
    0x0000_FFFF_0000_FFFF,
    0x0000_0000_FFFF_FFFF,
];

/// Returns true when LUT6 `init` is independent of input pin `input` (low cofactor == high cofactor).
#[inline]
pub fn lut6_input_is_dont_care(init: u64, input: usize) -> bool {
    debug_assert!(input < 6);
    let shift = 1usize << input;
    let mask = COFACTOR_LO_MASK[input];
    (init & mask) == ((init >> shift) & mask)
}

/// Canonicalize a `Lut6` truth table and its 6 input nets:
/// 1. Fold unconnected (`None`) inputs to `0`.
/// 2. Coalesce duplicate input nets connected to multiple pins of the same LUT.
/// 3. Prune don't-care inputs where low and high cofactors of `init` are equal.
/// 4. Sort active inputs by [`Spur`] ID into `I0..I_{m-1}` and permute `init` to match.
pub fn canonicalize_lut6(
    mut init: u64,
    mut inputs: [Option<Spur>; 6],
) -> (u64, [Option<Spur>; 6]) {
    // Step 1: fold unconnected inputs to 0 so garbage high bits cannot pollute canonical INIT.
    for i in 0..6 {
        if inputs[i].is_none() && !lut6_input_is_dont_care(init, i) {
            init = fold_init_const(init, i, false);
        }
    }

    // Step 2: if the same net drives both pin i and pin j (i < j), tie bit j to bit i in `init`.
    for i in 0..6 {
        let Some(ni) = inputs[i] else { continue };
        for j in (i + 1)..6 {
            if inputs[j] == Some(ni) {
                let mut collapsed = 0u64;
                for addr in 0..64u64 {
                    let bit_i = (addr >> i) & 1;
                    let eff = (addr & !(1u64 << j)) | (bit_i << j);
                    collapsed |= ((init >> eff) & 1) << addr;
                }
                init = collapsed;
                inputs[j] = None;
            }
        }
    }

    // Step 3: prune don't-care inputs where low and high cofactors are equal.
    for i in 0..6 {
        if inputs[i].is_some() && lut6_input_is_dont_care(init, i) {
            inputs[i] = None;
        }
    }

    // Step 4: collect remaining active (Spur, old_pin) pairs and sort by Spur.
    let mut active: [(Spur, usize); 6] = [(Spur::default(), 0); 6];
    let mut m = 0usize;
    for (pin, net) in inputs.iter().enumerate() {
        if let Some(n) = *net {
            active[m] = (n, pin);
            m += 1;
        }
    }
    active[..m].sort_unstable_by_key(|(s, _)| *s);

    let mut canon_inputs: [Option<Spur>; 6] = [None; 6];
    for new_pos in 0..m {
        canon_inputs[new_pos] = Some(active[new_pos].0);
    }

    // Check if identity permutation with no gaps.
    let mut is_identity = true;
    for new_pos in 0..m {
        if active[new_pos].1 != new_pos {
            is_identity = false;
            break;
        }
    }
    if is_identity {
        return (init, canon_inputs);
    }

    let mut canon_init = 0u64;
    for new_addr in 0..64u64 {
        let mut old_addr = 0u64;
        for new_pos in 0..m {
            if ((new_addr >> new_pos) & 1) != 0 {
                old_addr |= 1u64 << active[new_pos].1;
            }
        }
        canon_init |= ((init >> old_addr) & 1) << new_addr;
    }
    (canon_init, canon_inputs)
}
/// Push a constant driver value into a LUT INIT: output bit at `addr` becomes
/// the old output with `input` fixed to `val`.
fn fold_init_const(init: u64, input: usize, val: bool) -> u64 {
    debug_assert!(input < 6);
    let mut out = 0u64;
    for addr in 0..64u64 {
        let fixed = if val {
            addr | (1 << input)
        } else {
            addr & !(1 << input)
        };
        out |= ((init >> fixed) & 1) << addr;
    }
    out
}

/// Push an inversion on `input` into a LUT INIT: output bit at `addr` becomes
/// the old output with `input` flipped.
fn absorb_init_invert(init: u64, input: usize) -> u64 {
    debug_assert!(input < 6);
    let mut out = 0u64;
    for addr in 0..64u64 {
        out |= ((init >> (addr ^ (1 << input))) & 1) << addr;
    }
    out
}

/// Per-design statistics from [`Design::peephole_opt_luts`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LutOptStats {
    /// LUTs removed by the dead sweep (includes const/inverter husks).
    pub dead_swept: usize,
    /// Const LUTs folded into fanout INITs.
    pub consts_folded: usize,
    /// Inverter LUTs absorbed into fanout INITs.
    pub inverters_absorbed: usize,
    /// Fixpoint rounds executed.
    pub rounds: usize,
}

impl LutOptStats {
    /// Total LUTs eliminated.
    pub fn total(&self) -> usize {
        self.dead_swept
    }
}

/// A fanout pin: (cell index, LUT input index `I0`–`I5`).
type FanoutPin = (usize, usize);
/// A pending const fold: (const cell idx, const net idx, const value, fanouts).
type ConstFold = (usize, usize, bool, Vec<FanoutPin>);
/// A pending inverter absorption: (inverter cell idx, inverter net idx, fanouts).
type InvFold = (usize, usize, Vec<FanoutPin>);

impl Design {
    /// Net names observed by ILA cores (referenced by name, not by endpoint).
    fn ila_nets(&self) -> HashSet<Spur> {
        self.cells
            .iter()
            .filter_map(|c| match &c.kind {
                CellKind::Ila { net } => Some(*net),
                _ => None,
            })
            .collect()
    }

    /// Top-level output (or inout) port net set for O(1) lookup.
    fn output_port_nets(&self) -> HashSet<Spur> {
        self.ports
            .iter()
            .filter(|p| matches!(p.dir, PortDir::Out | PortDir::Inout))
            .map(|p| p.name)
            .collect()
    }

    /// Map cell name -> index of the net on its `O` pin (LUT drivers).
    fn lut_output_nets(&self) -> HashMap<Spur, usize> {
        let o_pin = crate::intern("O");
        let mut map = HashMap::with_capacity(self.cells.len());
        for (ni, n) in self.nets.iter().enumerate() {
            for e in &n.endpoints {
                if e.pin == o_pin {
                    map.insert(e.cell, ni);
                }
            }
        }
        map
    }

    /// Map cell name -> cell index.
    fn cell_index(&self) -> HashMap<Spur, usize> {
        let mut map = HashMap::with_capacity(self.cells.len());
        for (i, c) in self.cells.iter().enumerate() {
            map.insert(c.name, i);
        }
        map
    }

    fn is_output_port_net(&self, net_name: Spur) -> bool {
        self.ports.iter().any(|p| {
            matches!(p.dir, PortDir::Out | PortDir::Inout) && p.name == net_name
        })
    }

    /// Shared guard: the driver LUT `cell_idx` with output net `ni` must not
    /// be optimized away when the net is externally observable.
    fn driver_guarded(&self, cell_idx: usize, ni: usize, ila: &HashSet<Spur>) -> bool {
        let c = &self.cells[cell_idx];
        if c.attrs.flag("DONT_TOUCH") {
            return true;
        }
        let n = &self.nets[ni];
        if n.attrs.flag("mark_debug") {
            return true;
        }
        if ila.contains(&n.name) {
            return true;
        }
        if self.is_output_port_net(n.name) {
            return true;
        }
        false
    }

    #[inline]
    fn driver_guarded_fast(
        &self,
        cell_idx: usize,
        ni: usize,
        ila: &HashSet<Spur>,
        out_ports: &HashSet<Spur>,
    ) -> bool {
        let c = &self.cells[cell_idx];
        if c.attrs.flag("DONT_TOUCH") {
            return true;
        }
        let n = &self.nets[ni];
        if n.attrs.flag("mark_debug") {
            return true;
        }
        if ila.contains(&n.name) {
            return true;
        }
        if out_ports.contains(&n.name) {
            return true;
        }
        false
    }

    /// Remove every LUT whose output net has no endpoint other than its own
    /// `O`. No rewiring: a zero-fanout LUT cannot affect any observable
    /// signal. Returns the number of LUTs removed.
    ///
    /// Complexity: O(cells + nets + endpoints).
    pub fn sweep_dead_luts(&mut self) -> usize {
        self.rebuild_indexes();
        self.sweep_dead_luts_inner()
    }

    fn sweep_dead_luts_inner(&mut self) -> usize {
        let o_pin = intern("O");
        let ila = self.ila_nets();
        let out_ports = self.output_port_nets();
        let mut dead: Vec<usize> = Vec::new();
        for (ni, n) in self.nets.iter().enumerate() {
            if n.endpoints.len() != 1 {
                continue;
            }
            let e = &n.endpoints[0];
            if e.pin != o_pin {
                continue;
            }
            if let Some(&ci) = self.cell_ix.get(&e.cell) {
                if matches!(self.cells[ci].kind, CellKind::Lut6 { .. })
                    && !self.driver_guarded_fast(ci, ni, &ila, &out_ports)
                {
                    dead.push(ci);
                }
            }
        }
        self.remove_cells_fast(&dead);
        dead.len()
    }

    /// Remove cells by index plus any nets left with no endpoints.
    /// Keeps `cell_ix` in sync without rebuilding `pin_ix` mid-round.
    fn remove_cells_fast(&mut self, dead: &[usize]) {
        if dead.is_empty() {
            return;
        }
        let dead_names: HashSet<Spur> =
            dead.iter().map(|&i| self.cells[i].name).collect();
        for net in self.nets.iter_mut() {
            net.endpoints.retain(|e| !dead_names.contains(&e.cell));
        }
        self.nets.retain(|n| !n.endpoints.is_empty());
        let dead_set: HashSet<usize> = dead.iter().copied().collect();
        let mut idx = 0usize;
        self.cells.retain(|_| {
            let keep = !dead_set.contains(&idx);
            idx += 1;
            keep
        });
        self.rebuild_cell_ix();
    }

    /// Remove cells by index plus any nets left with no endpoints.
    /// Keeps the name indexes in sync.
    fn remove_cells(&mut self, dead: &[usize]) {
        if dead.is_empty() {
            return;
        }
        self.remove_cells_fast(dead);
        self.rebuild_net_ix();
    }

    /// Fold const0/const1 LUTs into fanout LUT INITs.
    ///
    /// For each `Lut6` with `INIT` 0 / `u64::MAX` whose every fanout endpoint
    /// is a LUT `I0`–`I5` pin, the constant is pushed into each fanout INIT
    /// and the fanout pin disconnected; the const LUT is then dead (removed
    /// by [`Design::sweep_dead_luts`]). Returns the number of const LUTs
    /// folded. LUTs with any non-LUT fanout (FF `D`, IOB `I`, …) are left
    /// alone — there is no INIT to absorb into and rewiring is forbidden.
    ///
    /// Complexity: O(cells + nets + endpoints + 64 * folded pins).
    pub fn fold_lut_consts(&mut self) -> usize {
        self.rebuild_indexes();
        self.fold_lut_consts_inner()
    }

    fn fold_lut_consts_inner(&mut self) -> usize {
        let (i_pins, o_pin) = Self::lut_pin_spurs();
        let mut folds: Vec<ConstFold> = Vec::new();
        {
            let out_net = self.lut_output_nets();
            let ila = self.ila_nets();
            let out_ports = self.output_port_nets();
            for (ci, c) in self.cells.iter().enumerate() {
                let init = match c.kind {
                    CellKind::Lut6 { init } => init,
                    _ => continue,
                };
                if init != 0 && init != u64::MAX {
                    continue;
                }
                let Some(&ni) = out_net.get(&c.name) else {
                    continue;
                };
                if self.driver_guarded_fast(ci, ni, &ila, &out_ports) {
                    continue;
                }
                let val = init != 0;
                let mut fanouts: Vec<(usize, usize)> = Vec::new();
                let mut ok = true;
                for e in &self.nets[ni].endpoints {
                    if e.cell == c.name && e.pin == o_pin {
                        continue;
                    }
                    let (fi, ii) = match self.cell_ix.get(&e.cell) {
                        Some(&fi)
                            if matches!(self.cells[fi].kind, CellKind::Lut6 { .. })
                                && !self.cells[fi].attrs.flag("DONT_TOUCH")
                                && fi != ci =>
                        {
                            match i_pins.iter().position(|&p| p == e.pin) {
                                Some(i) => (fi, i),
                                None => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    };
                    fanouts.push((fi, ii));
                }
                if ok && !fanouts.is_empty() {
                    folds.push((ci, ni, val, fanouts));
                }
            }
        }
        let mut folded = 0usize;
        for (ci, ni, val, fanouts) in folds {
            let _ = ci;
            for (fi, ii) in &fanouts {
                if let CellKind::Lut6 { init } = &mut self.cells[*fi].kind {
                    *init = fold_init_const(*init, *ii, val);
                }
            }
            let pins: HashSet<(Spur, Spur)> = fanouts
                .iter()
                .map(|(fi, ii)| (self.cells[*fi].name, i_pins[*ii]))
                .collect();
            if let Some(net) = self.nets.get_mut(ni) {
                net.endpoints
                    .retain(|e| !pins.contains(&(e.cell, e.pin)));
            }
            for &(c, p) in &pins {
                self.unindex_pin(c, p);
            }
            folded += 1;
        }
        folded
    }

    /// Absorb `~I0` inverter LUTs (`INIT = 0x5555…`) into fanout LUT INITs.
    pub fn absorb_lut_inverters(&mut self) -> usize {
        self.rebuild_cell_ix();
        let n = self.absorb_lut_inverters_inner();
        if n > 0 {
            self.rebuild_net_ix();
        }
        n
    }

    fn absorb_lut_inverters_inner(&mut self) -> usize {
        let (i_pins, o_pin) = Self::lut_pin_spurs();
        let i0_pin = i_pins[0];
        let mut folds: Vec<InvFold> = Vec::new();
        {
            let out_net = self.lut_output_nets();
            let ila = self.ila_nets();
            let out_ports = self.output_port_nets();
            let mut i0_net: HashMap<Spur, usize> = HashMap::new();
            for (ni, n) in self.nets.iter().enumerate() {
                for e in &n.endpoints {
                    if e.pin == i0_pin {
                        i0_net.insert(e.cell, ni);
                    }
                }
            }
            for (ci, c) in self.cells.iter().enumerate() {
                let is_inv = matches!(c.kind, CellKind::Lut6 { init } if init == INV_INIT);
                if !is_inv {
                    continue;
                }
                if !i0_net.contains_key(&c.name) {
                    continue;
                }
                let Some(&ni) = out_net.get(&c.name) else {
                    continue;
                };
                if self.driver_guarded_fast(ci, ni, &ila, &out_ports) {
                    continue;
                }
                let mut fanouts: Vec<(usize, usize)> = Vec::new();
                let mut ok = true;
                for e in &self.nets[ni].endpoints {
                    if e.cell == c.name && e.pin == o_pin {
                        continue;
                    }
                    let (fi, ii) = match self.cell_ix.get(&e.cell) {
                        Some(&fi)
                            if matches!(self.cells[fi].kind, CellKind::Lut6 { .. })
                                && !self.cells[fi].attrs.flag("DONT_TOUCH")
                                && fi != ci =>
                        {
                            match i_pins.iter().position(|&p| p == e.pin) {
                                Some(i) => (fi, i),
                                None => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    };
                    fanouts.push((fi, ii));
                }
                if ok && !fanouts.is_empty() {
                    folds.push((ci, ni, fanouts));
                }
            }
        }
        let mut absorbed = 0usize;
        for (ci, ni, fanouts) in folds {
            let _ = ci;
            for (fi, ii) in &fanouts {
                if let CellKind::Lut6 { init } = &mut self.cells[*fi].kind {
                    *init = absorb_init_invert(*init, *ii);
                }
            }
            let pins: HashSet<(Spur, Spur)> = fanouts
                .iter()
                .map(|(fi, ii)| (self.cells[*fi].name, i_pins[*ii]))
                .collect();
            if let Some(net) = self.nets.get_mut(ni) {
                net.endpoints
                    .retain(|e| !pins.contains(&(e.cell, e.pin)));
            }
            for &(c, p) in &pins {
                self.unindex_pin(c, p);
            }
            absorbed += 1;
        }
        absorbed
    }

    /// Run the LUT peephole passes to fixpoint.
    pub fn peephole_opt_luts(&mut self) -> LutOptStats {
        let mut st = LutOptStats::default();
        self.rebuild_cell_ix();
        for _ in 0..16 {
            let f = self.fold_lut_consts_inner();
            let a = self.absorb_lut_inverters_inner();
            let s = self.sweep_dead_luts_inner();
            st.consts_folded += f;
            st.inverters_absorbed += a;
            st.dead_swept += s;
            st.rounds += 1;
            if f + a + s == 0 {
                break;
            }
        }
        self.rebuild_indexes();
        st
    }

    /// Count of [`CellKind::Lut6`] cells in the design (including any `Muxf7`/`Muxf8` LUTs).
    pub fn lut_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count()
    }

    /// Count of [`CellKind::Hff`] flip-flop cells in the design.
    pub fn ff_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .count()
    }

    /// Cached LUT pin `Spur`s `[I0..I5]` and `O`.
    #[inline]
    fn lut_pin_spurs() -> ([Spur; 6], Spur) {
        (
            [
                intern("I0"),
                intern("I1"),
                intern("I2"),
                intern("I3"),
                intern("I4"),
                intern("I5"),
            ],
            intern("O"),
        )
    }

    /// Extract the 6 input nets `[I0..I5]` and output net `O` for every cell index.
    fn build_lut_io_tables(&self) -> (Vec<[Option<Spur>; 6]>, Vec<Option< usize>>) {
        let (i_pins, o_pin) = Self::lut_pin_spurs();
        let mut lut_ins = vec![[None; 6]; self.cells.len()];
        let mut lut_out_ni = vec![None; self.cells.len()];
        for (ni, net) in self.nets.iter().enumerate() {
            for ep in &net.endpoints {
                let Some(&ci) = self.cell_ix.get(&ep.cell) else {
                    continue;
                };
                if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                    continue;
                }
                if ep.pin == o_pin {
                    lut_out_ni[ci] = Some(ni);
                } else {
                    for (idx, &p_spur) in i_pins.iter().enumerate() {
                        if ep.pin == p_spur {
                            lut_ins[ci][idx] = Some(net.name);
                            break;
                        }
                    }
                }
            }
        }
        (lut_ins, lut_out_ni)
    }

    /// Replace a LUT cell's `I0..I5` connections to match `new_inputs`.
    fn set_lut_inputs(&mut self, ci: usize, new_inputs: &[Option<Spur>; 6]) {
        let cell_name = self.cells[ci].name;
        let (i_pins, _) = Self::lut_pin_spurs();
        for net in &mut self.nets {
            net.endpoints
                .retain(|e| !(e.cell == cell_name && i_pins.contains(&e.pin)));
        }
        self.rebuild_net_ix();
        let cell_str = resolve(cell_name);
        for (pin_idx, net_opt) in new_inputs.iter().enumerate() {
            if let Some(net_spur) = *net_opt {
                let pin_name = match pin_idx {
                    0 => "I0",
                    1 => "I1",
                    2 => "I2",
                    3 => "I3",
                    4 => "I4",
                    _ => "I5",
                };
                self.connect(resolve(net_spur), cell_str, pin_name);
            }
        }
    }

    /// Coalesce single-input identity buffer LUT6 cells (`O = I0`, `INIT = 0xAAAA_AAAA_AAAA_AAAA`).
    ///
    /// Rewires all loads on an unguarded buffer LUT's output net directly to its `I0` input net
    /// and removes the buffer LUT. Returns the number of buffer LUTs eliminated.
    pub fn opt_coalesce_buffers(&mut self) -> usize {
        self.rebuild_indexes();
        let mut total = 0usize;
        for _ in 0..16 {
            let ila = self.ila_nets();
            let (lut_ins, lut_out_ni) = self.build_lut_io_tables();
            let (_, o_pin) = Self::lut_pin_spurs();
            let mut found = None;
            for (ci, c) in self.cells.iter().enumerate() {
                let CellKind::Lut6 { init } = c.kind else {
                    continue;
                };
                if c.is_muxf() {
                    continue;
                }
                let Some(out_ni) = lut_out_ni[ci] else {
                    continue;
                };
                if self.driver_guarded(ci, out_ni, &ila) {
                    continue;
                }
                let (canon_init, canon_ins) = canonicalize_lut6(init, lut_ins[ci]);
                if canon_init == BUF_INIT && canon_ins[0].is_some() && canon_ins[1].is_none() {
                    let src_net = canon_ins[0].unwrap();
                    if src_net != self.nets[out_ni].name {
                        found = Some((ci, out_ni, src_net));
                        break;
                    }
                }
            }
            let Some((ci, out_ni, src_net)) = found else {
                break;
            };
            let cell_name = self.cells[ci].name;
            let loads: Vec<Endpoint> = self.nets[out_ni]
                .endpoints
                .iter()
                .filter(|e| !(e.cell == cell_name && e.pin == o_pin))
                .cloned()
                .collect();
            self.nets[out_ni]
                .endpoints
                .retain(|e| e.cell == cell_name && e.pin == o_pin);
            if let Some(&src_ni) = self.net_ix.get(&src_net) {
                self.nets[src_ni].endpoints.extend(loads);
            }
            self.remove_cells(&[ci]);
            total += 1;
        }
        total
    }

    /// Structural LUT Hashing / Functional CSE (`opt_cse`).
    ///
    /// 1. Canonicalizes every movable `Lut6` (pruning don't-care inputs where low and high
    ///    cofactors of `init` are equal, coalescing duplicate pins, sorting active input
    ///    [`Spur`]s, and permuting the 64-bit `init` mask accordingly).
    /// 2. Merges functionally identical `Lut6` cells across the netlist to fixpoint so
    ///    multi-level cones with equivalent sub-cones share hardware LUTs.
    ///
    /// Returns the number of duplicate `Lut6` cells eliminated.
    pub fn opt_cse(&mut self) -> usize {
        self.rebuild_indexes();
        let mut total_merged = 0usize;
        let (_, o_pin) = Self::lut_pin_spurs();

        for _round in 0..16 {
            self.rebuild_indexes();
            let ila = self.ila_nets();
            let (lut_ins, _lut_out_ni) = self.build_lut_io_tables();

            // Compute canonical (init, inputs) for all LUTs; use rayon on larger netlists.
            let canon_vec: Vec<Option<(u64, [Option<Spur>; 6])>> = if self.cells.len() >= 128 {
                self.cells
                    .par_iter()
                    .enumerate()
                    .map(|(ci, c)| match c.kind {
                        CellKind::Lut6 { init } if !c.attrs.flag("DONT_TOUCH") && !c.is_muxf() => {
                            Some(canonicalize_lut6(init, lut_ins[ci]))
                        }
                        _ => None,
                    })
                    .collect()
            } else {
                self.cells
                    .iter()
                    .enumerate()
                    .map(|(ci, c)| match c.kind {
                        CellKind::Lut6 { init } if !c.attrs.flag("DONT_TOUCH") && !c.is_muxf() => {
                            Some(canonicalize_lut6(init, lut_ins[ci]))
                        }
                        _ => None,
                    })
                    .collect()
            };

            // Apply canonicalized init and pin connections where changed.
            for (ci, entry) in canon_vec.iter().enumerate() {
                let Some((c_init, c_ins)) = entry else {
                    continue;
                };
                if let CellKind::Lut6 { init } = &mut self.cells[ci].kind {
                    *init = *c_init;
                }
                if *c_ins != lut_ins[ci] {
                    self.set_lut_inputs(ci, c_ins);
                }
            }
            self.rebuild_indexes();
            let (lut_ins, lut_out_ni) = self.build_lut_io_tables();

            // Group by canonical key (init, inputs) and merge duplicates.
            let mut canonical_map: HashMap<(u64, [Option<Spur>; 6]), usize> =
                HashMap::with_capacity(self.cells.len());
            let mut merges: Vec<(usize, usize, usize)> = Vec::new(); // (dup_ci, dup_out_ni, rep_out_ni)

            for (ci, c) in self.cells.iter().enumerate() {
                let CellKind::Lut6 { init } = c.kind else {
                    continue;
                };
                if c.attrs.flag("DONT_TOUCH") || c.is_muxf() {
                    continue;
                }
                let Some(out_ni) = lut_out_ni[ci] else {
                    continue;
                };
                let key = (init, lut_ins[ci]);
                if let Some(&rep_out_ni) = canonical_map.get(&key) {
                    if !self.driver_guarded(ci, out_ni, &ila) && out_ni != rep_out_ni {
                        merges.push((ci, out_ni, rep_out_ni));
                    }
                } else {
                    canonical_map.insert(key, out_ni);
                }
            }

            if merges.is_empty() {
                break;
            }

            let mut dead_cells: Vec<usize> = Vec::with_capacity(merges.len());
            for (dup_ci, dup_out_ni, rep_out_ni) in merges {
                let dup_name = self.cells[dup_ci].name;
                let loads: Vec<Endpoint> = self.nets[dup_out_ni]
                    .endpoints
                    .iter()
                    .filter(|e| !(e.cell == dup_name && e.pin == o_pin))
                    .cloned()
                    .collect();
                self.nets[dup_out_ni]
                    .endpoints
                    .retain(|e| e.cell == dup_name && e.pin == o_pin);
                self.nets[rep_out_ni].endpoints.extend(loads);
                dead_cells.push(dup_ci);
            }

            total_merged += dead_cells.len();
            self.remove_cells(&dead_cells);
            // Sweep any upstream LUTs whose fanouts were pruned as don't-care inputs.
            total_merged += self.sweep_dead_luts_inner();
        }

        // Final dead sweep in case canonicalization pruned don't-care pins on the last round.
        self.rebuild_indexes();
        total_merged += self.sweep_dead_luts_inner();
        total_merged
    }

    /// LUT Cascade Fusion (`opt_lut_merge`).
    ///
    /// When a `Lut6` `A` drives a single fanout `Lut6` `B` and the union of their distinct
    /// active inputs has $\le 6$ nets, composes `A`'s truth table into `B`'s 64-bit `init`
    /// mask and eliminates `A` (reducing both logic depth and LUT count).
    ///
    /// Runs to fixpoint so multi-stage shallow cascades collapse completely.
    /// Returns the number of LUTs fused and eliminated.
    pub fn opt_lut_merge(&mut self) -> usize {
        self.rebuild_indexes();
        let mut total_fused = 0usize;
        let (i_pins, o_pin) = Self::lut_pin_spurs();

        for _round in 0..32 {
            self.rebuild_indexes();
            let ila = self.ila_nets();
            let (lut_ins, lut_out_ni) = self.build_lut_io_tables();
            let mut candidate: Option<(usize, usize, u64, [Option<Spur>; 6])> = None;

            for (ci_a, cell_a) in self.cells.iter().enumerate() {
                let CellKind::Lut6 { init: init_a } = cell_a.kind else {
                    continue;
                };
                if cell_a.is_muxf() {
                    continue;
                }
                let Some(net_a_ni) = lut_out_ni[ci_a] else {
                    continue;
                };
                if self.driver_guarded(ci_a, net_a_ni, &ila) {
                    continue;
                }
                let net_a_spur = self.nets[net_a_ni].name;

                // Verify that all loads of net_a land on input pins I0..I5 of a single Lut6 B.
                let mut ci_b_opt: Option<usize> = None;
                let mut load_count = 0usize;
                let mut valid_fanout = true;
                for ep in &self.nets[net_a_ni].endpoints {
                    if ep.cell == cell_a.name && ep.pin == o_pin {
                        continue;
                    }
                    let Some(&ci_b) = self.cell_ix.get(&ep.cell) else {
                        valid_fanout = false;
                        break;
                    };
                    if ci_b == ci_a
                        || !matches!(self.cells[ci_b].kind, CellKind::Lut6 { .. })
                        || self.cells[ci_b].attrs.flag("DONT_TOUCH")
                        || self.cells[ci_b].is_muxf()
                        || !i_pins.contains(&ep.pin)
                    {
                        valid_fanout = false;
                        break;
                    }
                    if let Some(prev_b) = ci_b_opt {
                        if prev_b != ci_b {
                            valid_fanout = false;
                            break;
                        }
                    } else {
                        ci_b_opt = Some(ci_b);
                    }
                    load_count += 1;
                }
                if !valid_fanout || load_count == 0 {
                    continue;
                }
                let ci_b = ci_b_opt.unwrap();
                let CellKind::Lut6 { init: init_b } = self.cells[ci_b].kind else {
                    continue;
                };

                // Canonicalize A and B first to drop any don't-care pins before counting union inputs.
                let (c_init_a, c_ins_a) = canonicalize_lut6(init_a, lut_ins[ci_a]);
                let (c_init_b, c_ins_b) = canonicalize_lut6(init_b, lut_ins[ci_b]);

                // If B didn't actually depend on net_a, canonicalizing B alone already disconnects A.
                let mut combined: Vec<Spur> = Vec::with_capacity(6);
                let mut fits = true;
                for net_opt in c_ins_b.iter().chain(c_ins_a.iter()) {
                    let Some(n) = *net_opt else { continue };
                    if n == net_a_spur {
                        continue;
                    }
                    if !combined.contains(&n) {
                        if combined.len() == 6 {
                            fits = false;
                            break;
                        }
                        combined.push(n);
                    }
                }
                if !fits {
                    continue;
                }

                // Compose A's truth table into B's 64-bit init mask over `combined`.
                let mut composed_init = 0u64;
                for addr in 0..64u64 {
                    let mut addr_a = 0u64;
                    for (pin_a, net_opt) in c_ins_a.iter().enumerate() {
                        if let Some(n) = *net_opt {
                            if let Some(pos) = combined.iter().position(|&x| x == n) {
                                if ((addr >> pos) & 1) != 0 {
                                    addr_a |= 1u64 << pin_a;
                                }
                            }
                        }
                    }
                    let val_a = (c_init_a >> addr_a) & 1;

                    let mut addr_b = 0u64;
                    for (pin_b, net_opt) in c_ins_b.iter().enumerate() {
                        if let Some(n) = *net_opt {
                            let bit = if n == net_a_spur {
                                val_a
                            } else if let Some(pos) = combined.iter().position(|&x| x == n) {
                                (addr >> pos) & 1
                            } else {
                                0
                            };
                            addr_b |= bit << pin_b;
                        }
                    }
                    let val_b = (c_init_b >> addr_b) & 1;
                    composed_init |= val_b << addr;
                }

                let mut raw_inputs = [None; 6];
                for (i, &n) in combined.iter().enumerate() {
                    raw_inputs[i] = Some(n);
                }
                let (final_init, final_inputs) = canonicalize_lut6(composed_init, raw_inputs);
                candidate = Some((ci_a, ci_b, final_init, final_inputs));
                break;
            }

            let Some((ci_a, ci_b, final_init, final_inputs)) = candidate else {
                break;
            };

            if let CellKind::Lut6 { init } = &mut self.cells[ci_b].kind {
                *init = final_init;
            }
            self.set_lut_inputs(ci_b, &final_inputs);
            self.remove_cells(&[ci_a]);
            total_fused += 1;
            // Sweep any newly orphaned upstream LUTs if composition canceled an input.
            total_fused += self.sweep_dead_luts_inner();
        }

        total_fused
    }

    /// Wide-Cone Shannon Decomposition Helper (`decompose_wide_cone_muxf`).
    ///
    /// Lowers a 7-input (128-bit truth table or cofactor pair) or 8-input (256-bit truth
    /// table / 4-cofactor array) boolean function into leaf [`CellKind::Lut6`] pairs
    /// combined by dedicated `Muxf7` / `Muxf8` hardware primitives.
    ///
    /// Pin conventions:
    /// - `inputs[0..6]` drive `I0..I5` of each leaf `Lut6`.
    /// - `inputs[6]` drives the select input (`I2`, with `S` alias attribute) of each `Muxf7`.
    /// - `inputs[7]` (for 8-input cones) drives the select input (`I2`) of the root `Muxf8`.
    pub fn decompose_wide_cone_muxf(
        &mut self,
        prefix: &str,
        inputs: &[&str],
        out_net: &str,
        table: impl Into<WideTruthTable>,
    ) -> Result<WideConeDecomp, String> {
        let spec = table.into();
        match spec {
            WideTruthTable::Lut7(init128) => {
                if inputs.len() != 7 {
                    return Err(format!(
                        "decompose_wide_cone_muxf: 7-input cone expects 7 inputs, got {}",
                        inputs.len()
                    ));
                }
                let lo = init128 as u64;
                let hi = (init128 >> 64) as u64;
                Ok(self.emit_muxf7_pair(prefix, inputs, out_net, lo, hi))
            }
            WideTruthTable::Cofactors7 { lo, hi } => {
                if inputs.len() != 7 {
                    return Err(format!(
                        "decompose_wide_cone_muxf: 7-input cofactor pair expects 7 inputs, got {}",
                        inputs.len()
                    ));
                }
                Ok(self.emit_muxf7_pair(prefix, inputs, out_net, lo, hi))
            }
            WideTruthTable::Lut8(c) => {
                if inputs.len() != 8 {
                    return Err(format!(
                        "decompose_wide_cone_muxf: 8-input cone expects 8 inputs, got {}",
                        inputs.len()
                    ));
                }
                Ok(self.emit_muxf8_tree(prefix, inputs, out_net, c))
            }
            WideTruthTable::Lut8Words([lo128, hi128]) => {
                if inputs.len() != 8 {
                    return Err(format!(
                        "decompose_wide_cone_muxf: 8-input cone expects 8 inputs, got {}",
                        inputs.len()
                    ));
                }
                let c = [
                    lo128 as u64,
                    (lo128 >> 64) as u64,
                    hi128 as u64,
                    (hi128 >> 64) as u64,
                ];
                Ok(self.emit_muxf8_tree(prefix, inputs, out_net, c))
            }
        }
    }

    /// Convenience wrapper for lowering a 7-input 128-bit truth table into 2 `Lut6` + 1 `Muxf7`.
    pub fn decompose_lut7_muxf7(
        &mut self,
        prefix: &str,
        inputs: &[&str; 7],
        out_net: &str,
        init128: u128,
    ) -> WideConeDecomp {
        self.emit_muxf7_pair(
            prefix,
            inputs.as_slice(),
            out_net,
            init128 as u64,
            (init128 >> 64) as u64,
        )
    }

    /// Convenience wrapper for lowering an 8-input 256-bit truth table into 4 `Lut6` + 2 `Muxf7` + 1 `Muxf8`.
    pub fn decompose_lut8_muxf8(
        &mut self,
        prefix: &str,
        inputs: &[&str; 8],
        out_net: &str,
        cofactors: [u64; 4],
    ) -> WideConeDecomp {
        self.emit_muxf8_tree(prefix, inputs.as_slice(), out_net, cofactors)
    }

    /// Add a dedicated `Muxf7` or `Muxf8` hardware primitive cell (`I0` = low, `I1` = high, `I2` = select `S`).
    pub fn add_muxf_cell(
        &mut self,
        name: &str,
        kind: MuxfKind,
        in0_net: &str,
        in1_net: &str,
        sel_net: &str,
        out_net: &str,
    ) -> Spur {
        let mut attrs = Attrs::default();
        attrs.set("PRIM", kind.as_str());
        attrs.set("BEL", kind.bel_str());
        let cell_spur = intern(name);
        self.push_cell(Cell {
            name: cell_spur,
            kind: CellKind::Lut6 { init: MUXF_INIT },
            attrs,
        });
        self.connect(in0_net, name, "I0");
        self.connect(in1_net, name, "I1");
        self.connect(sel_net, name, "I2");
        self.connect(out_net, name, "O");
        cell_spur
    }

    fn emit_muxf7_pair(
        &mut self,
        prefix: &str,
        inputs: &[&str],
        out_net: &str,
        lo: u64,
        hi: u64,
    ) -> WideConeDecomp {
        let lut_lo = format!("{prefix}_lut6_lo");
        let lut_hi = format!("{prefix}_lut6_hi");
        let muxf7 = format!("{prefix}_muxf7");
        let net_lo = format!("{prefix}_n_lo");
        let net_hi = format!("{prefix}_n_hi");

        self.add_cell(&lut_lo, CellKind::Lut6 { init: lo });
        self.add_cell(&lut_hi, CellKind::Lut6 { init: hi });
        for i in 0..6 {
            let pin = match i {
                0 => "I0",
                1 => "I1",
                2 => "I2",
                3 => "I3",
                4 => "I4",
                _ => "I5",
            };
            self.connect(inputs[i], &lut_lo, pin);
            self.connect(inputs[i], &lut_hi, pin);
        }
        self.connect(&net_lo, &lut_lo, "O");
        self.connect(&net_hi, &lut_hi, "O");
        let muxf7_spur =
            self.add_muxf_cell(&muxf7, MuxfKind::Muxf7, &net_lo, &net_hi, inputs[6], out_net);

        WideConeDecomp {
            luts: vec![intern(&lut_lo), intern(&lut_hi)],
            muxf7s: vec![muxf7_spur],
            muxf8: None,
            out_net: intern(out_net),
        }
    }

    fn emit_muxf8_tree(
        &mut self,
        prefix: &str,
        inputs: &[&str],
        out_net: &str,
        c: [u64; 4],
    ) -> WideConeDecomp {
        let lo_prefix = format!("{prefix}_lo");
        let hi_prefix = format!("{prefix}_hi");
        let net_f7_lo = format!("{prefix}_f7_lo");
        let net_f7_hi = format!("{prefix}_f7_hi");
        let muxf8_name = format!("{prefix}_muxf8");

        let d_lo = self.emit_muxf7_pair(&lo_prefix, &inputs[..7], &net_f7_lo, c[0], c[1]);
        let d_hi = self.emit_muxf7_pair(&hi_prefix, &inputs[..7], &net_f7_hi, c[2], c[3]);
        let muxf8_spur = self.add_muxf_cell(
            &muxf8_name,
            MuxfKind::Muxf8,
            &net_f7_lo,
            &net_f7_hi,
            inputs[7],
            out_net,
        );

        let mut luts = d_lo.luts;
        luts.extend(d_hi.luts);
        let mut muxf7s = d_lo.muxf7s;
        muxf7s.extend(d_hi.muxf7s);
        WideConeDecomp {
            luts,
            muxf7s,
            muxf8: Some(muxf8_spur),
            out_net: intern(out_net),
        }
    }

    /// Maximum combinational LUT depth across the design.
    ///
    /// Primary inputs and sequential outputs (`Hff/Q`, BRAM, DSP) have depth `0`.
    /// Each [`CellKind::Lut6`] cell has depth `1 + max(input_depths)`.
    pub fn max_lut_depth(&self) -> usize {
        let (lut_levels, _) = self.compute_lut_depths();
        lut_levels.into_iter().max().unwrap_or(0)
    }

    /// Compute `(arrival_level, required_reverse_depth)` for every cell in `self.cells`.
    ///
    /// - `arrival_level[ci]` is `0` for non-LUT cells and `1 + max_{fanin LUT} arrival_level` for `Lut6`.
    /// - `reverse_depth[ci]` is the longest combinational LUT chain from cell `ci`'s output
    ///   to a sequential/IOB/port sink (`0` if `ci` directly drives only FFs/IOBs/ports).
    fn compute_lut_depths(&self) -> (Vec<usize>, Vec<usize>) {
        let n = self.cells.len();
        let (_, o_pin) = Self::lut_pin_spurs();
        let (lut_ins, lut_out_ni) = self.build_lut_io_tables();

        // Map net Spur -> driving LUT cell index.
        let mut net_lut_driver: HashMap<Spur, usize> = HashMap::with_capacity(n);
        for (ci, out_ni_opt) in lut_out_ni.iter().enumerate() {
            if let Some(ni) = *out_ni_opt {
                net_lut_driver.insert(self.nets[ni].name, ci);
            }
        }

        // Memoized DFS for arrival depth (with cycle guard).
        let mut arrival = vec![0usize; n];
        let mut state = vec![0u8; n]; // 0 = unvisited, 1 = visiting, 2 = done
        fn dfs_arr(
            ci: usize,
            cells: &[Cell],
            lut_ins: &[[Option<Spur>; 6]],
            net_lut_driver: &HashMap<Spur, usize>,
            arrival: &mut [usize],
            state: &mut [u8],
        ) -> usize {
            if state[ci] == 2 {
                return arrival[ci];
            }
            if state[ci] == 1 {
                return 0; // break combinational cycle
            }
            if !matches!(cells[ci].kind, CellKind::Lut6 { .. }) {
                state[ci] = 2;
                return 0;
            }
            state[ci] = 1;
            let mut max_in = 0usize;
            for net_opt in &lut_ins[ci] {
                if let Some(net) = *net_opt {
                    if let Some(&drv_ci) = net_lut_driver.get(&net) {
                        let d = dfs_arr(drv_ci, cells, lut_ins, net_lut_driver, arrival, state);
                        if d > max_in {
                            max_in = d;
                        }
                    }
                }
            }
            let lvl = 1 + max_in;
            arrival[ci] = lvl;
            state[ci] = 2;
            lvl
        }

        for ci in 0..n {
            if matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) && state[ci] == 0 {
                dfs_arr(
                    ci,
                    &self.cells,
                    &lut_ins,
                    &net_lut_driver,
                    &mut arrival,
                    &mut state,
                );
            }
        }

        // Build LUT -> fanout LUTs adjacency for reverse combinational depth.
        let mut lut_fanouts: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (ci, ins) in lut_ins.iter().enumerate() {
            if !matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) {
                continue;
            }
            for net_opt in ins {
                if let Some(net) = *net_opt {
                    if let Some(&drv_ci) = net_lut_driver.get(&net) {
                        if !lut_fanouts[drv_ci].contains(&ci) {
                            lut_fanouts[drv_ci].push(ci);
                        }
                    }
                }
            }
        }

        let mut rev = vec![0usize; n];
        let mut rev_state = vec![0u8; n];
        fn dfs_rev(
            ci: usize,
            lut_fanouts: &[Vec<usize>],
            rev: &mut [usize],
            state: &mut [u8],
        ) -> usize {
            if state[ci] == 2 {
                return rev[ci];
            }
            if state[ci] == 1 {
                return 0;
            }
            state[ci] = 1;
            let mut max_out = 0usize;
            for &fo_ci in &lut_fanouts[ci] {
                let d = 1 + dfs_rev(fo_ci, lut_fanouts, rev, state);
                if d > max_out {
                    max_out = d;
                }
            }
            rev[ci] = max_out;
            state[ci] = 2;
            max_out
        }

        for ci in 0..n {
            if matches!(self.cells[ci].kind, CellKind::Lut6 { .. }) && rev_state[ci] == 0 {
                let _ = o_pin;
                dfs_rev(ci, &lut_fanouts, &mut rev, &mut rev_state);
            }
        }

        (arrival, rev)
    }

    /// Forward and Backward Register Retiming (`phys_opt_retime`).
    ///
    /// Balances combinational [`CellKind::Lut6`] depth across [`CellKind::Hff`] pipeline stages
    /// (Vivado `phys_opt_design -retime` equivalent):
    /// - **Backward retiming**: pulls an [`CellKind::Hff`] register backward across a deep combinational
    ///   LUT `L` (`arrival_depth >= 2`) when downstream depth from the FF's `Q` output is smaller,
    ///   strictly reducing maximum combinational LUT depth while preserving cycle-accurate ($t \ge 0$)
    ///   semantics under zero-initialized FFs via automatic LUT input/output polarity inversion.
    /// - **Forward retiming**: pushes synchronous input [`CellKind::Hff`] registers forward across a
    ///   combinational LUT `L` when `L` feeds deeper combinational logic (or coalesces multiple dedicated
    ///   input registers into one), preserving $t = 0$ initial values via fanout LUT polarity inversion.
    ///
    /// Returns the total number of retiming transformations applied.
    pub fn phys_opt_retime(&mut self) -> usize {
        self.rebuild_indexes();
        let mut total_retimed = 0usize;
        let (i_pins, o_pin) = Self::lut_pin_spurs();
        let d_pin = intern("D");
        let q_pin = intern("Q");
        let clk_pin = intern("CLK");

        for round in 0..16 {
            self.rebuild_indexes();
            let ila = self.ila_nets();
            let (arrival, rev_depth) = self.compute_lut_depths();
            let (lut_ins, lut_out_ni) = self.build_lut_io_tables();

            // Map net Spur -> driving cell index (for LUT O or Hff Q).
            let mut net_driver_cell: HashMap<Spur, (usize, Spur)> =
                HashMap::with_capacity(self.nets.len());
            for (ni, net) in self.nets.iter().enumerate() {
                let _ = ni;
                for ep in &net.endpoints {
                    if ep.pin == o_pin || ep.pin == q_pin {
                        if let Some(&ci) = self.cell_ix.get(&ep.cell) {
                            net_driver_cell.insert(net.name, (ci, ep.pin));
                        }
                    }
                }
            }

            // 1. Try Backward Retiming across a deep LUT L driving an Hff F_out.
            // Find the highest-arrival-depth LUT L -> F_out where moving F_out backward reduces local max depth.
            let mut best_bwd: Option<(usize, usize, Spur, Spur, usize)> = None; // (lut_ci, ff_ci, clk_net, q_net, lut_arr)
            for (ci_l, cell_l) in self.cells.iter().enumerate() {
                let CellKind::Lut6 { init: init_l } = cell_l.kind else {
                    continue;
                };
                if cell_l.attrs.flag("DONT_TOUCH") || arrival[ci_l] < 2 {
                    continue;
                }
                let Some(out_ni) = lut_out_ni[ci_l] else {
                    continue;
                };
                if self.driver_guarded(ci_l, out_ni, &ila) {
                    continue;
                }
                // net_o must have exactly 2 endpoints: (L, "O") and (F_out, "D").
                if self.nets[out_ni].endpoints.len() != 2 {
                    continue;
                }
                let Some(ff_ep) = self.nets[out_ni]
                    .endpoints
                    .iter()
                    .find(|e| !(e.cell == cell_l.name && e.pin == o_pin))
                else {
                    continue;
                };
                if ff_ep.pin != d_pin {
                    continue;
                }
                let Some(&ci_ff) = self.cell_ix.get(&ff_ep.cell) else {
                    continue;
                };
                if !matches!(self.cells[ci_ff].kind, CellKind::Hff)
                    || self.cells[ci_ff].attrs.flag("DONT_TOUCH")
                {
                    continue;
                }
                let ff_name = self.cells[ci_ff].name;
                let Some(clk_net) = self.net_on(ff_name, clk_pin) else {
                    continue;
                };
                let Some(q_net) = self.net_on(ff_name, q_pin) else {
                    continue;
                };
                let Some(&q_ni) = self.net_ix.get(&q_net) else {
                    continue;
                };
                if self.nets[q_ni].attrs.flag("mark_debug") || ila.contains(&q_net) {
                    continue;
                }

                // Compute downstream combinational LUT depth starting from F_out.Q.
                let mut q_downstream_depth = 0usize;
                for ep in &self.nets[q_ni].endpoints {
                    if ep.cell == ff_name && ep.pin == q_pin {
                        continue;
                    }
                    if let Some(&load_ci) = self.cell_ix.get(&ep.cell) {
                        if matches!(self.cells[load_ci].kind, CellKind::Lut6 { .. }) {
                            let d = 1 + rev_depth[load_ci];
                            if d > q_downstream_depth {
                                q_downstream_depth = d;
                            }
                        }
                    }
                }

                // Backward retiming changes (in_depth = arrival[ci_l], out_depth = q_downstream_depth)
                // into (in_depth - 1, out_depth + 1). Require strict reduction in max(in_depth, out_depth).
                let cur_max = arrival[ci_l].max(q_downstream_depth);
                let new_max = (arrival[ci_l] - 1).max(q_downstream_depth + 1);
                if new_max >= cur_max {
                    continue;
                }

                // Check zero-init ($t=0$) preservation:
                // Either (init_l & 1) == 0, OR there is an active input pin k of L driven by an unguarded
                // single-fanout Lut6 U_k such that flipping U_k's output and L's input k makes (init_l_new & 1) == 0.
                let can_zero_init = if (init_l & 1) == 0 {
                    true
                } else {
                    self.find_backward_polarity_mask(ci_l, init_l, &lut_ins[ci_l], &net_driver_cell, &ila)
                        .is_some()
                };
                if !can_zero_init {
                    continue;
                }

                if best_bwd
                    .as_ref()
                    .map_or(true, |b| arrival[ci_l] > b.4)
                {
                    best_bwd = Some((ci_l, ci_ff, clk_net, q_net, arrival[ci_l]));
                }
            }

            if let Some((ci_l, ci_ff, clk_net, q_net, _)) = best_bwd {
                let CellKind::Lut6 { mut init } = self.cells[ci_l].kind else {
                    unreachable!()
                };
                let ins = lut_ins[ci_l];
                if (init & 1) != 0 {
                    if let Some(mask) =
                        self.find_backward_polarity_mask(ci_l, init, &ins, &net_driver_cell, &ila)
                    {
                        for k in 0..6 {
                            if ((mask >> k) & 1) != 0 {
                                let in_net = ins[k].unwrap();
                                let (u_ci, _) = net_driver_cell[&in_net];
                                if let CellKind::Lut6 { init: u_init } = &mut self.cells[u_ci].kind
                                {
                                    *u_init = !*u_init;
                                }
                                init = absorb_init_invert(init, k);
                            }
                        }
                        if let CellKind::Lut6 { init: l_init } = &mut self.cells[ci_l].kind {
                            *l_init = init;
                        }
                    }
                }

                // Insert (or reuse within this LUT) an Hff on each connected input of L.
                let l_name_str = resolve(self.cells[ci_l].name).to_string();
                let clk_str = resolve(clk_net).to_string();
                let mut new_ins: [Option<Spur>; 6] = [None; 6];
                let mut net_to_reg_q: HashMap<Spur, Spur> = HashMap::new();

                for k in 0..6 {
                    let Some(in_net) = ins[k] else { continue };
                    let reg_q = if let Some(&existing_q) = net_to_reg_q.get(&in_net) {
                        existing_q
                    } else {
                        let ff_new_name = format!("{l_name_str}_retime_bwd_r{round}_p{k}");
                        let q_new_name = format!("{l_name_str}_retime_bwd_q_r{round}_p{k}");
                        self.add_cell(&ff_new_name, CellKind::Hff);
                        self.connect(&clk_str, &ff_new_name, "CLK");
                        self.connect(resolve(in_net), &ff_new_name, "D");
                        self.connect(&q_new_name, &ff_new_name, "Q");
                        let q_spur = intern(&q_new_name);
                        net_to_reg_q.insert(in_net, q_spur);
                        q_spur
                    };
                    new_ins[k] = Some(reg_q);
                }
                self.set_lut_inputs(ci_l, &new_ins);

                // Move L's output from old `out_ni` directly onto `q_net`, and remove `ci_ff`.
                let l_spur = self.cells[ci_l].name;
                let ff_spur = self.cells[ci_ff].name;
                for net in &mut self.nets {
                    net.endpoints
                        .retain(|e| !(e.cell == l_spur && e.pin == o_pin) && e.cell != ff_spur);
                }
                self.rebuild_net_ix();
                self.connect(resolve(q_net), &l_name_str, "O");
                self.remove_cells(&[ci_ff]);
                total_retimed += 1;
                continue;
            }

            // 2. Try Forward Retiming across a LUT L whose inputs are all driven by Hff Qs on the same clock.
            let mut best_fwd: Option<(usize, Spur, Vec<(usize, usize, Spur, Spur)>)> = None;
            for (ci_l, cell_l) in self.cells.iter().enumerate() {
                let CellKind::Lut6 { init: init_l } = cell_l.kind else {
                    continue;
                };
                if cell_l.attrs.flag("DONT_TOUCH") {
                    continue;
                }
                let Some(out_ni) = lut_out_ni[ci_l] else {
                    continue;
                };
                if self.driver_guarded(ci_l, out_ni, &ila) {
                    continue;
                }
                let ins = lut_ins[ci_l];
                let mut active_ffs: Vec<(usize, usize, Spur, Spur)> = Vec::new(); // (pin_k, ff_ci, d_net, q_net)
                let mut common_clk: Option<Spur> = None;
                let mut all_ff_inputs = true;
                let mut max_ff_in_depth = 0usize;

                for (k, net_opt) in ins.iter().enumerate() {
                    let Some(q_net) = *net_opt else { continue };
                    let Some(&(drv_ci, drv_pin)) = net_driver_cell.get(&q_net) else {
                        all_ff_inputs = false;
                        break;
                    };
                    if drv_pin != q_pin
                        || !matches!(self.cells[drv_ci].kind, CellKind::Hff)
                        || self.cells[drv_ci].attrs.flag("DONT_TOUCH")
                    {
                        all_ff_inputs = false;
                        break;
                    }
                    let ff_name = self.cells[drv_ci].name;
                    let Some(clk_net) = self.net_on(ff_name, clk_pin) else {
                        all_ff_inputs = false;
                        break;
                    };
                    let Some(d_net) = self.net_on(ff_name, d_pin) else {
                        all_ff_inputs = false;
                        break;
                    };
                    if let Some(c) = common_clk {
                        if c != clk_net {
                            all_ff_inputs = false;
                            break;
                        }
                    } else {
                        common_clk = Some(clk_net);
                    }
                    if let Some(&(d_drv_ci, _)) = net_driver_cell.get(&d_net) {
                        if arrival[d_drv_ci] > max_ff_in_depth {
                            max_ff_in_depth = arrival[d_drv_ci];
                        }
                    }
                    active_ffs.push((k, drv_ci, d_net, q_net));
                }

                if !all_ff_inputs || active_ffs.is_empty() {
                    continue;
                }
                let clk_net = common_clk.unwrap();

                // Check if forward retiming reduces critical path depth OR merges multiple dedicated input FFs.
                let cur_path_max = max_ff_in_depth.max(1 + rev_depth[ci_l]);
                let new_path_max = (max_ff_in_depth + 1).max(rev_depth[ci_l]);
                let improves_depth = new_path_max < cur_path_max;
                let merges_ffs = active_ffs.len() >= 2
                    && new_path_max <= cur_path_max
                    && active_ffs.iter().all(|&(_, ff_ci, _, q_net)| {
                        self.net_ix.get(&q_net).is_some_and(|&q_ni| {
                            !self.driver_guarded(ff_ci, q_ni, &ila)
                                && self.nets[q_ni]
                                    .endpoints
                                    .iter()
                                    .all(|e| e.cell == self.cells[ff_ci].name || e.cell == cell_l.name)
                        })
                    });

                if !improves_depth && !merges_ffs {
                    continue;
                }

                // Check zero-init ($t=0$) preservation:
                // Either (init_l & 1) == 0, or all fanout loads of L's output net are movable LUT6 input pins.
                if (init_l & 1) != 0 {
                    let can_invert_fanouts = self.nets[out_ni].endpoints.iter().all(|e| {
                        if e.cell == cell_l.name && e.pin == o_pin {
                            return true;
                        }
                        self.cell_ix.get(&e.cell).is_some_and(|&fo_ci| {
                            fo_ci != ci_l
                                && matches!(self.cells[fo_ci].kind, CellKind::Lut6 { .. })
                                && !self.cells[fo_ci].attrs.flag("DONT_TOUCH")
                                && i_pins.contains(&e.pin)
                        })
                    });
                    if !can_invert_fanouts {
                        continue;
                    }
                }

                best_fwd = Some((ci_l, clk_net, active_ffs));
                break;
            }

            if let Some((ci_l, clk_net, active_ffs)) = best_fwd {
                let out_ni = lut_out_ni[ci_l].unwrap();
                let l_spur = self.cells[ci_l].name;
                let l_name_str = resolve(l_spur).to_string();
                let out_net_str = resolve(self.nets[out_ni].name).to_string();
                let CellKind::Lut6 { init: init_l } = self.cells[ci_l].kind else {
                    unreachable!()
                };

                if (init_l & 1) != 0 {
                    // Invert L's output so L(0, ..., 0) == 0 (matching zero-initialized F_new),
                    // and push the inversion into all fanout LUT input pins of out_ni.
                    if let CellKind::Lut6 { init } = &mut self.cells[ci_l].kind {
                        *init = !*init;
                    }
                    let fanout_pins: Vec<(usize, usize)> = self.nets[out_ni]
                        .endpoints
                        .iter()
                        .filter(|e| !(e.cell == l_spur && e.pin == o_pin))
                        .map(|e| {
                            let fo_ci = self.cell_ix[&e.cell];
                            let pin_i = i_pins.iter().position(|&p| p == e.pin).unwrap();
                            (fo_ci, pin_i)
                        })
                        .collect();
                    for (fo_ci, pin_i) in fanout_pins {
                        if let CellKind::Lut6 { init } = &mut self.cells[fo_ci].kind {
                            *init = absorb_init_invert(*init, pin_i);
                        }
                    }
                }

                // Rewire L's inputs from F_k.Q to F_k.D.
                let mut new_ins = [None; 6];
                for &(k, _, d_net, _) in &active_ffs {
                    new_ins[k] = Some(d_net);
                }
                self.set_lut_inputs(ci_l, &new_ins);

                // Disconnect L.O from `out_net` and insert `F_new` between L.O and `out_net`.
                self.rebuild_net_ix();
                if let Some(&cur_out_ni) = self.net_ix.get(&intern(&out_net_str)) {
                    self.nets[cur_out_ni]
                        .endpoints
                        .retain(|e| !(e.cell == l_spur && e.pin == o_pin));
                }
                let d_new_net = format!("{l_name_str}_retime_fwd_d_r{round}");
                let ff_new_name = format!("{l_name_str}_retime_fwd_ff_r{round}");
                self.add_cell(&ff_new_name, CellKind::Hff);
                self.connect(resolve(clk_net), &ff_new_name, "CLK");
                self.connect(&d_new_net, &l_name_str, "O");
                self.connect(&d_new_net, &ff_new_name, "D");
                self.connect(&out_net_str, &ff_new_name, "Q");

                // Remove any input FFs whose Q nets now have zero load endpoints and are unguarded.
                self.rebuild_indexes();
                let mut dead_ffs: Vec<usize> = Vec::new();
                for &(_, ff_ci, _, q_net) in &active_ffs {
                    if dead_ffs.contains(&ff_ci) {
                        continue;
                    }
                    if let Some(&q_ni) = self.net_ix.get(&q_net) {
                        if self.nets[q_ni].endpoints.len() == 1
                            && !self.driver_guarded(ff_ci, q_ni, &ila)
                        {
                            dead_ffs.push(ff_ci);
                        }
                    }
                }
                self.remove_cells(&dead_ffs);
                total_retimed += 1;
                continue;
            }

            break;
        }

        total_retimed
    }

    /// Find a 6-bit input inversion mask `mask` for backward retiming of LUT `ci_l` such that:
    /// 1. Every set bit `k` in `mask` corresponds to an input `ins[k]` driven by an unguarded,
    ///    single-fanout `Lut6` `U_k`.
    /// 2. `((init_l >> mask) & 1) == 0`, so inverting `U_k`'s output and `L`'s input `k` for
    ///    each bit `k` in `mask` makes `L_new(0, ..., 0) == 0`.
    fn find_backward_polarity_mask(
        &self,
        ci_l: usize,
        init_l: u64,
        ins: &[Option<Spur>; 6],
        net_driver_cell: &HashMap<Spur, (usize, Spur)>,
        ila: &HashSet<Spur>,
    ) -> Option<u64> {
        let (_, o_pin) = Self::lut_pin_spurs();
        let mut invertible_mask = 0u64;
        let mut seen_nets: [Option<Spur>; 6] = [None; 6];
        for k in 0..6 {
            let Some(in_net) = ins[k] else { continue };
            // Ensure this net only appears on a single pin of L.
            if seen_nets.iter().flatten().any(|&n| n == in_net) {
                continue;
            }
            seen_nets[k] = Some(in_net);
            let Some(&(u_ci, drv_pin)) = net_driver_cell.get(&in_net) else {
                continue;
            };
            if u_ci == ci_l
                || drv_pin != o_pin
                || !matches!(self.cells[u_ci].kind, CellKind::Lut6 { .. })
                || self.cells[u_ci].is_muxf()
            {
                continue;
            }
            let Some(&u_ni) = self.net_ix.get(&in_net) else {
                continue;
            };
            if self.driver_guarded(u_ci, u_ni, ila) || self.nets[u_ni].endpoints.len() != 2 {
                continue;
            }
            invertible_mask |= 1u64 << k;
        }
        if invertible_mask == 0 {
            return None;
        }
        for cand in 1..64u64 {
            if (cand & !invertible_mask) == 0 && ((init_l >> cand) & 1) == 0 {
                return Some(cand);
            }
        }
        None
    }

    /// High-Fanout Register Replication (`phys_opt_fanout_split` / Vivado `phys_opt_design -fanout_opt`).
    ///
    /// Splits any unguarded [`CellKind::Hff`] whose `Q` output net drives more than `max_fanout`
    /// load endpoints by cloning replica [`CellKind::Hff`] cells sharing the same `CLK` and `D`
    /// nets, partitioning the loads so the original register and every replica drive at most
    /// `max_fanout` loads.
    ///
    /// Returns the number of replica [`CellKind::Hff`] cells created.
    pub fn phys_opt_fanout_split(&mut self, max_fanout: usize) -> usize {
        let cap = max_fanout.max(1);
        self.rebuild_indexes();
        let ila = self.ila_nets();
        let d_pin = intern("D");
        let q_pin = intern("Q");
        let clk_pin = intern("CLK");

        // Collect (ff_ci, q_ni, clk_net, d_net) candidates.
        let mut candidates: Vec<(usize, usize, Spur, Spur)> = Vec::new();
        for (ci, cell) in self.cells.iter().enumerate() {
            if !matches!(cell.kind, CellKind::Hff) || cell.attrs.flag("DONT_TOUCH") {
                continue;
            }
            let Some(q_net) = self.net_on(cell.name, q_pin) else {
                continue;
            };
            let Some(&q_ni) = self.net_ix.get(&q_net) else {
                continue;
            };
            if self.driver_guarded(ci, q_ni, &ila) {
                continue;
            }
            let load_count = self.nets[q_ni]
                .endpoints
                .iter()
                .filter(|e| !(e.cell == cell.name && e.pin == q_pin))
                .count();
            if load_count <= cap {
                continue;
            }
            let Some(clk_net) = self.net_on(cell.name, clk_pin) else {
                continue;
            };
            let Some(d_net) = self.net_on(cell.name, d_pin) else {
                continue;
            };
            candidates.push((ci, q_ni, clk_net, d_net));
        }

        let mut replicas_created = 0usize;
        for (ci, q_ni, clk_net, d_net) in candidates {
            let ff_name = self.cells[ci].name;
            let ff_name_str = resolve(ff_name).to_string();
            let q_net_str = resolve(self.nets[q_ni].name).to_string();
            let clk_str = resolve(clk_net).to_string();
            let d_str = resolve(d_net).to_string();
            let base_attrs = self.cells[ci].attrs.clone();

            let mut driver_eps: Vec<Endpoint> = Vec::with_capacity(1 + cap);
            let mut loads: Vec<Endpoint> = Vec::new();
            for ep in self.nets[q_ni].endpoints.drain(..) {
                if ep.cell == ff_name && ep.pin == q_pin {
                    driver_eps.push(ep);
                } else {
                    loads.push(ep);
                }
            }

            // Keep the first `cap` loads on the original FF's Q net.
            driver_eps.extend(loads.drain(..cap));
            self.nets[q_ni].endpoints = driver_eps;

            // Partition remaining loads into chunks of size `cap`.
            let chunks: Vec<Vec<Endpoint>> =
                loads.chunks(cap).map(|s| s.to_vec()).collect();
            for (idx, chunk) in chunks.into_iter().enumerate() {
                let rep_num = idx + 1;
                let rep_cell_name = format!("{ff_name_str}_rep{rep_num}");
                let rep_net_name = format!("{q_net_str}_rep{rep_num}");
                let rep_cell_spur = intern(&rep_cell_name);
                let rep_net_spur = intern(&rep_net_name);

                self.push_cell(Cell {
                    name: rep_cell_spur,
                    kind: CellKind::Hff,
                    attrs: base_attrs.clone(),
                });
                self.connect(&clk_str, &rep_cell_name, "CLK");
                self.connect(&d_str, &rep_cell_name, "D");

                let mut eps = Vec::with_capacity(1 + chunk.len());
                eps.push(Endpoint {
                    cell: rep_cell_spur,
                    pin: q_pin,
                });
                eps.extend(chunk);
                self.merge_net(Net {
                    name: rep_net_spur,
                    endpoints: eps,
                    attrs: Attrs::default(),
                });
                replicas_created += 1;
            }
        }

        if replicas_created > 0 {
            self.rebuild_indexes();
        }
        replicas_created
    }

    /// Alias for [`Design::phys_opt_fanout_split`] (Vivado `-fanout_opt`).
    #[inline]
    pub fn fanout_opt(&mut self, max_fanout: usize) -> usize {
        self.phys_opt_fanout_split(max_fanout)
    }

    /// Run Vivado-style `opt_design` netlist optimization pipeline:
    /// const/inverter peephole folding, identity buffer coalescing, structural/functional CSE,
    /// and shallow LUT cascade fusion.
    pub fn opt_design(&mut self) -> OptDesignStats {
        let peephole = self.peephole_opt_luts();
        let buffers_coalesced = self.opt_coalesce_buffers();
        let cse_merged = self.opt_cse();
        let luts_merged = self.opt_lut_merge();
        let post_cse = self.opt_cse();
        OptDesignStats {
            peephole,
            buffers_coalesced,
            cse_merged: cse_merged + post_cse,
            luts_merged,
        }
    }

    /// Run Vivado-style `phys_opt_design` post-synthesis timing/fanout optimization pipeline:
    /// register retiming (`-retime`) followed by high-fanout register replication (`-fanout_opt`).
    pub fn phys_opt_design(&mut self, max_fanout: usize) -> PhysOptStats {
        let retimed = self.phys_opt_retime();
        let fanout_replicas = self.phys_opt_fanout_split(max_fanout);
        PhysOptStats {
            retimed,
            fanout_replicas,
        }
    }

    /// Evaluate the combinational network of `self` for a given map of net/port values.
    ///
    /// Propagates [`CellKind::IobIn`], [`CellKind::Lut6`] (including `Muxf7`/`Muxf8`), and
    /// [`CellKind::IobOut`] to fixpoint and returns the resulting boolean value on every net.
    pub fn eval_comb(&self, primary_inputs: &HashMap<Spur, bool>) -> HashMap<Spur, bool> {
        let mut values: HashMap<Spur, bool> = primary_inputs.clone();
        let (i_pins, o_pin) = Self::lut_pin_spurs();
        let pad_pin = intern("PAD");
        let i_pin = intern("I");

        // Propagate IobIn PAD -> O first.
        for c in &self.cells {
            if matches!(c.kind, CellKind::IobIn) {
                if let (Some(pad), Some(o)) =
                    (self.net_on(c.name, pad_pin), self.net_on(c.name, o_pin))
                {
                    let v = values.get(&pad).copied().unwrap_or(false);
                    values.insert(o, v);
                }
            }
        }

        // Propagate LUT6 cells in topological/fixpoint rounds.
        let max_rounds = self.cells.len() + 2;
        for _ in 0..max_rounds {
            let mut changed = false;
            for c in &self.cells {
                let CellKind::Lut6 { init } = c.kind else {
                    continue;
                };
                let Some(out_net) = self.net_on(c.name, o_pin) else {
                    continue;
                };
                let mut addr = 0u64;
                for (idx, &p_spur) in i_pins.iter().enumerate() {
                    if let Some(in_net) = self.net_on(c.name, p_spur) {
                        if values.get(&in_net).copied().unwrap_or(false) {
                            addr |= 1u64 << idx;
                        }
                    }
                }
                let out_val = ((init >> addr) & 1) != 0;
                if values.insert(out_net, out_val) != Some(out_val) {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Propagate IobOut I -> PAD.
        for c in &self.cells {
            if matches!(c.kind, CellKind::IobOut) {
                if let (Some(i_net), Some(pad_net)) =
                    (self.net_on(c.name, i_pin), self.net_on(c.name, pad_pin))
                {
                    let v = values.get(&i_net).copied().unwrap_or(false);
                    values.insert(pad_net, v);
                }
            }
        }

        values
    }

    /// Cycle-accurate synchronous simulation of [`Design`] over a sequence of primary input maps.
    ///
    /// All [`CellKind::Hff`] flip-flops initialize to `false` (`0`) at $t = 0$.
    /// For each cycle $t$, returns the full net value map before the clock edge, then updates
    /// all [`CellKind::Hff`] outputs `Q <= D` simultaneously.
    pub fn simulate_cycles(
        &self,
        cycle_inputs: &[HashMap<Spur, bool>],
    ) -> Vec<HashMap<Spur, bool>> {
        let d_pin = intern("D");
        let q_pin = intern("Q");
        let mut ff_state: HashMap<Spur, bool> = HashMap::new();
        for c in &self.cells {
            if matches!(c.kind, CellKind::Hff) {
                ff_state.insert(c.name, false);
            }
        }

        let mut trace = Vec::with_capacity(cycle_inputs.len());
        for p_in in cycle_inputs {
            let mut state = p_in.clone();
            for c in &self.cells {
                if matches!(c.kind, CellKind::Hff) {
                    if let Some(q_net) = self.net_on(c.name, q_pin) {
                        state.insert(q_net, ff_state.get(&c.name).copied().unwrap_or(false));
                    }
                }
            }
            let comb = self.eval_comb(&state);
            for c in &self.cells {
                if matches!(c.kind, CellKind::Hff) {
                    let next_q = self
                        .net_on(c.name, d_pin)
                        .and_then(|dn| comb.get(&dn).copied())
                        .unwrap_or(false);
                    ff_state.insert(c.name, next_q);
                }
            }
            trace.push(comb);
        }
        trace
    }
}

impl Cell {
    /// Returns `Some(MuxfKind)` when this cell is a dedicated `Muxf7` or `Muxf8` primitive.
    #[inline]
    pub fn muxf_kind(&self) -> Option<MuxfKind> {
        match self.attrs.get("PRIM") {
            Some("MUXF7") => Some(MuxfKind::Muxf7),
            Some("MUXF8") => Some(MuxfKind::Muxf8),
            _ => None,
        }
    }

    /// True when this cell is a `Muxf7` or `Muxf8` hardware primitive.
    #[inline]
    pub fn is_muxf(&self) -> bool {
        self.muxf_kind().is_some()
    }

    /// True when this cell is a `Muxf7` primitive.
    #[inline]
    pub fn is_muxf7(&self) -> bool {
        self.muxf_kind() == Some(MuxfKind::Muxf7)
    }

    /// True when this cell is a `Muxf8` primitive.
    #[inline]
    pub fn is_muxf8(&self) -> bool {
        self.muxf_kind() == Some(MuxfKind::Muxf8)
    }
}

impl CellKind {
    /// Construct a `Lut6`-backed `Muxf7` hardware mux primitive (`O = I2 ? I1 : I0`).
    #[inline]
    pub const fn muxf7() -> Self {
        Self::Lut6 { init: MUXF_INIT }
    }

    /// Construct a `Lut6`-backed `Muxf8` hardware mux primitive (`O = I2 ? I1 : I0`).
    #[inline]
    pub const fn muxf8() -> Self {
        Self::Lut6 { init: MUXF_INIT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lut_design() -> Design {
        // inv -> and -> out ; dead const ; dead lut
        let mut d = Design::new("t");
        d.add_port("a", PortDir::In);
        d.add_port("y", PortDir::Out);
        d.add_cell("c_const", CellKind::Lut6 { init: 0 });
        d.add_cell("c_inv", CellKind::Lut6 { init: INV_INIT });
        d.add_cell("c_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d.add_cell("c_dead", CellKind::Lut6 { init: 0x1234 });
        d.add_cell("c_obuf", CellKind::IobOut);
        d.connect("nc", "c_const", "O");
        d.connect("ni", "c_inv", "O");
        d.connect("a", "c_inv", "I0");
        d.connect("na", "c_and", "O");
        d.connect("na", "c_and", "I0");
        d.connect("nc", "c_and", "I1");
        d.connect("ni", "c_and", "I2");
        d.connect("na", "c_obuf", "I");
        d.connect("y", "c_obuf", "PAD");
        d.connect("nd", "c_dead", "O");
        d
    }

    #[test]
    fn dead_sweep_removes_only_zero_fanout() {
        let mut d = lut_design();
        let n = d.sweep_dead_luts();
        assert_eq!(n, 1, "only c_dead is zero-fanout");
        assert!(d.cell("c_dead").is_none());
        assert!(d.cell("c_const").is_some());
        assert!(d.cell("c_inv").is_some());
        assert!(d.net("nd").is_none(), "empty net dropped");
    }

    #[test]
    fn const_fold_pushes_into_fanout_init() {
        let mut d = lut_design();
        // c_const(0) -> c_and.I1 ; c_and = 0x8888.. (AND of I0&I1&I2-ish)
        let n = d.fold_lut_consts();
        assert_eq!(n, 1);
        let init = match d.cell("c_and").unwrap().kind {
            CellKind::Lut6 { init } => init,
            _ => panic!(),
        };
        // I1 fixed to 0: out[addr] = old[addr & !0b010]
        assert_eq!(init, fold_init_const(0x8888_8888_8888_8888, 1, false));
        // pin disconnected, const now dead (plus the pre-existing dead lut)
        assert!(d.net_on(intern("c_and"), intern("I1")).is_none());
        assert_eq!(d.sweep_dead_luts(), 2);
        assert!(d.cell("c_const").is_none());
        assert!(d.cell("c_dead").is_none());
    }

    #[test]
    fn inverter_absorb_flips_fanout_addr_bit() {
        let mut d = lut_design();
        let n = d.absorb_lut_inverters();
        assert_eq!(n, 1);
        let init = match d.cell("c_and").unwrap().kind {
            CellKind::Lut6 { init } => init,
            _ => panic!(),
        };
        assert_eq!(init, absorb_init_invert(0x8888_8888_8888_8888, 2));
        assert!(d.net_on(intern("c_and"), intern("I2")).is_none());
        assert_eq!(d.sweep_dead_luts(), 2);
        assert!(d.cell("c_inv").is_none());
        assert!(d.cell("c_dead").is_none());
    }

    #[test]
    fn guards_keep_observable_luts() {
        let mut d = Design::new("g");
        d.add_port("y", PortDir::Out);
        // dead LUT driving a top-level output port net: kept
        d.add_cell("c_port", CellKind::Lut6 { init: 0 });
        d.connect("y", "c_port", "O");
        // dead LUT on a mark_debug net: kept
        d.add_cell("c_dbg", CellKind::Lut6 { init: 0 });
        d.connect("dbg", "c_dbg", "O");
        d.mark_debug("dbg").unwrap();
        // dead LUT on an ILA-observed net: kept
        d.add_cell("c_ila", CellKind::Lut6 { init: 0 });
        d.add_cell("ila0", CellKind::Ila { net: intern("mon") });
        d.connect("mon", "c_ila", "O");
        assert_eq!(d.sweep_dead_luts(), 0);
        assert_eq!(d.fold_lut_consts(), 0);
    }

    #[test]
    fn fold_skips_non_lut_fanout() {
        let mut d = Design::new("f");
        d.add_port("clk", PortDir::In);
        d.add_cell("c_const", CellKind::Lut6 { init: 0 });
        d.add_cell("u_ff", CellKind::Hff);
        d.connect("nc", "c_const", "O");
        d.connect("nc", "u_ff", "D");
        d.connect("clk", "u_ff", "CLK");
        assert_eq!(d.fold_lut_consts(), 0, "FF D has no INIT to fold into");
        assert!(d.cell("c_const").is_some());
    }
}
