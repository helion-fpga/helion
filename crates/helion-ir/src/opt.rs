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

use crate::{CellKind, Design, PortDir};

/// `Lut6` INIT for `~I0` (I0 is the LSB of the LUT address).
const INV_INIT: u64 = 0x5555_5555_5555_5555;

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
#[derive(Clone, Copy, Debug, Default)]
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
    fn ila_nets(&self) -> HashSet<&str> {
        self.cells
            .iter()
            .filter_map(|c| match &c.kind {
                CellKind::Ila { net } => Some(net.as_str()),
                _ => None,
            })
            .collect()
    }

    /// True when `net_name` is a top-level output (or inout) port net.
    /// Guards the exact port name; bit-blasted port nets use the
    /// `{port}_{bit}` convention and are covered by their IOB loads in real
    /// flows, so only the exact name is guarded here.
    fn is_output_port_net(&self, net_name: &str) -> bool {
        self.ports.iter().any(|p| {
            matches!(p.dir, PortDir::Out | PortDir::Inout) && p.name == net_name
        })
    }

    /// Map cell name -> index of the net on its `O` pin (LUT drivers).
    fn lut_output_nets(&self) -> HashMap<&str, usize> {
        let mut map = HashMap::new();
        for (ni, n) in self.nets.iter().enumerate() {
            for e in &n.endpoints {
                if e.pin == "O" {
                    map.insert(e.cell.as_str(), ni);
                }
            }
        }
        map
    }

    /// Map cell name -> cell index.
    fn cell_index(&self) -> HashMap<&str, usize> {
        let mut map = HashMap::with_capacity(self.cells.len());
        for (i, c) in self.cells.iter().enumerate() {
            map.insert(c.name.as_str(), i);
        }
        map
    }

    /// Shared guard: the driver LUT `cell_idx` with output net `ni` must not
    /// be optimized away when the net is externally observable.
    fn driver_guarded(&self, cell_idx: usize, ni: usize, ila: &HashSet<&str>) -> bool {
        let c = &self.cells[cell_idx];
        if c.attrs.flag("DONT_TOUCH") {
            return true;
        }
        let n = &self.nets[ni];
        if n.attrs.flag("mark_debug") {
            return true;
        }
        if ila.contains(n.name.as_str()) {
            return true;
        }
        if self.is_output_port_net(&n.name) {
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
        let out_net = self.lut_output_nets();
        let ila = self.ila_nets();
        let mut dead: Vec<usize> = Vec::new();
        for (ci, c) in self.cells.iter().enumerate() {
            if !matches!(c.kind, CellKind::Lut6 { .. }) {
                continue;
            }
            let Some(&ni) = out_net.get(c.name.as_str()) else {
                continue;
            };
            if self.driver_guarded(ci, ni, &ila) {
                continue;
            }
            let n = &self.nets[ni];
            if n.endpoints.len() == 1 {
                let e = &n.endpoints[0];
                if e.cell == c.name && e.pin == "O" {
                    dead.push(ci);
                }
            }
        }
        self.remove_cells(&dead);
        dead.len()
    }

    /// Remove cells by index plus any nets left with no endpoints.
    /// Keeps the name indexes in sync.
    fn remove_cells(&mut self, dead: &[usize]) {
        if dead.is_empty() {
            return;
        }
        let dead_names: HashSet<&str> =
            dead.iter().map(|&i| self.cells[i].name.as_str()).collect();
        for net in self.nets.iter_mut() {
            net.endpoints.retain(|e| !dead_names.contains(e.cell.as_str()));
        }
        self.nets.retain(|n| !n.endpoints.is_empty());
        let dead_set: HashSet<usize> = dead.iter().copied().collect();
        let mut idx = 0usize;
        self.cells.retain(|_| {
            let keep = !dead_set.contains(&idx);
            idx += 1;
            keep
        });
        self.rebuild_indexes();
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
        let mut folds: Vec<ConstFold> = Vec::new();
        {
            let out_net = self.lut_output_nets();
            let cell_idx = self.cell_index();
            let ila = self.ila_nets();
            for (ci, c) in self.cells.iter().enumerate() {
                let init = match c.kind {
                    CellKind::Lut6 { init } => init,
                    _ => continue,
                };
                if init != 0 && init != u64::MAX {
                    continue;
                }
                let Some(&ni) = out_net.get(c.name.as_str()) else {
                    continue;
                };
                if self.driver_guarded(ci, ni, &ila) {
                    continue;
                }
                let val = init != 0;
                let mut fanouts: Vec<(usize, usize)> = Vec::new();
                let mut ok = true;
                for e in &self.nets[ni].endpoints {
                    if e.cell == c.name && e.pin == "O" {
                        continue;
                    }
                    let (fi, ii) = match cell_idx.get(e.cell.as_str()) {
                        Some(&fi)
                            if matches!(self.cells[fi].kind, CellKind::Lut6 { .. })
                                && !self.cells[fi].attrs.flag("DONT_TOUCH")
                                && fi != ci =>
                        {
                            match e.pin.strip_prefix('I').and_then(|s| s.parse::<usize>().ok()) {
                                Some(i) if i < 6 => (fi, i),
                                _ => {
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
            // Disconnect the folded pins from the const net.
            let pins: HashSet<(String, String)> = fanouts
                .iter()
                .map(|(fi, ii)| (self.cells[*fi].name.clone(), format!("I{ii}")))
                .collect();
            if let Some(net) = self.nets.get_mut(ni) {
                net.endpoints
                    .retain(|e| !pins.contains(&(e.cell.clone(), e.pin.clone())));
            }
            folded += 1;
        }
        if folded > 0 {
            self.rebuild_indexes();
        }
        folded
    }

    /// Absorb `~I0` inverter LUTs (`INIT = 0x5555…`) into fanout LUT INITs.
    ///
    /// Same shape as [`Design::fold_lut_consts`]: when every fanout is a LUT
    /// input pin, the inversion is pushed into each fanout INIT (address bit
    /// flipped) and the pin disconnected; the inverter is then dead. The
    /// inverter's `I0` must be connected (an undriven inverter has no defined
    /// output). Returns the number of inverters absorbed.
    ///
    /// Complexity: O(cells + nets + endpoints + 64 * absorbed pins).
    pub fn absorb_lut_inverters(&mut self) -> usize {
        self.rebuild_indexes();
        self.absorb_lut_inverters_inner()
    }

    fn absorb_lut_inverters_inner(&mut self) -> usize {
        let mut folds: Vec<InvFold> = Vec::new();
        {
            let out_net = self.lut_output_nets();
            let cell_idx = self.cell_index();
            let ila = self.ila_nets();
            // cell -> net idx for I0 pins, to check the inverter is driven
            let mut i0_net: HashMap<&str, usize> = HashMap::new();
            for (ni, n) in self.nets.iter().enumerate() {
                for e in &n.endpoints {
                    if e.pin == "I0" {
                        i0_net.insert(e.cell.as_str(), ni);
                    }
                }
            }
            for (ci, c) in self.cells.iter().enumerate() {
                let is_inv = matches!(c.kind, CellKind::Lut6 { init } if init == INV_INIT);
                if !is_inv {
                    continue;
                }
                if !i0_net.contains_key(c.name.as_str()) {
                    continue; // undriven inverter: output undefined, do not touch
                }
                let Some(&ni) = out_net.get(c.name.as_str()) else {
                    continue;
                };
                if self.driver_guarded(ci, ni, &ila) {
                    continue;
                }
                let mut fanouts: Vec<(usize, usize)> = Vec::new();
                let mut ok = true;
                for e in &self.nets[ni].endpoints {
                    if e.cell == c.name && e.pin == "O" {
                        continue;
                    }
                    let (fi, ii) = match cell_idx.get(e.cell.as_str()) {
                        Some(&fi)
                            if matches!(self.cells[fi].kind, CellKind::Lut6 { .. })
                                && !self.cells[fi].attrs.flag("DONT_TOUCH")
                                && fi != ci =>
                        {
                            match e.pin.strip_prefix('I').and_then(|s| s.parse::<usize>().ok()) {
                                Some(i) if i < 6 => (fi, i),
                                _ => {
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
            let pins: HashSet<(String, String)> = fanouts
                .iter()
                .map(|(fi, ii)| (self.cells[*fi].name.clone(), format!("I{ii}")))
                .collect();
            if let Some(net) = self.nets.get_mut(ni) {
                net.endpoints
                    .retain(|e| !pins.contains(&(e.cell.clone(), e.pin.clone())));
            }
            absorbed += 1;
        }
        if absorbed > 0 {
            self.rebuild_indexes();
        }
        absorbed
    }

    /// Run the LUT peephole passes to fixpoint: const fold and inverter
    /// absorption create dead LUTs, and const chains resolve one link per
    /// round. Bounded; returns per-pass statistics.
    ///
    /// Complexity: O(rounds * (cells + nets + endpoints)).
    pub fn peephole_opt_luts(&mut self) -> LutOptStats {
        let mut st = LutOptStats::default();
        for _ in 0..16 {
            // Rebuild indexes once at round start. Only rebuild between passes
            // if the previous pass actually modified the netlist.
            self.rebuild_indexes();
            let f = self.fold_lut_consts_inner();
            if f > 0 {
                self.rebuild_indexes();
            }
            let a = self.absorb_lut_inverters_inner();
            if a > 0 {
                self.rebuild_indexes();
            }
            let s = self.sweep_dead_luts_inner();
            st.consts_folded += f;
            st.inverters_absorbed += a;
            st.dead_swept += s;
            st.rounds += 1;
            if f + a + s == 0 {
                break;
            }
        }
        st
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
        assert!(d.net_on("c_and", "I1").is_none());
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
        assert!(d.net_on("c_and", "I2").is_none());
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
        d.add_cell("ila0", CellKind::Ila { net: "mon".into() });
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
