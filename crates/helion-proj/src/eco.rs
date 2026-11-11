//! UG904 Engineering Change Orders on an implemented [`Session`].
//!
//! [`EcoOp`] edits the implemented netlist: LUT `INIT` changes, pin
//! reconnects, buffer insertion, cell add/remove. [`Session::apply_eco`]
//! applies one op to the design (and, for a LUT `INIT` change on a routed
//! design, patches the bitstream in place without re-implementation);
//! [`Session::eco_implement`] then legalizes netlist changes with
//! incremental placement (previous sites are reused for every surviving
//! cell) plus a route, or only re-routes ([`EcoImpl::RouteOnly`]).
//! [`Session::write_eco_bitstream`] writes the new full bitstream and an
//! incremental partial bitstream holding only the frames that changed
//! versus the pre-ECO image (`helion_bits::diff_partial_bitstream`).

use crate::{ReuseReport, Session};
use helion_bits::{Bitstream, PartialBitstream, diff_partial_bitstream};
use helion_device::Device;
use helion_ir::{CellKind, Endpoint, Net, PortDir, intern, resolve};

/// One netlist edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EcoOp {
    /// Change a LUT's truth table.
    SetLutInit { cell: String, init: u64 },
    /// Move `cell.pin` off its current net onto `net` (created if new).
    ReconnectPin { cell: String, pin: String, net: String },
    /// Insert a LUT1 buffer named `name` after the driver of `net`: the
    /// driver keeps `net`, every load moves to `{net}_{name}`.
    InsertBuffer { net: String, name: String },
    /// Add a LUT (`init`) driving a new net `{name}_o`, inputs unconnected.
    AddLut { name: String, init: u64 },
    /// Add a flip-flop on clock net `clk` (Q net `{name}_q`).
    AddFf { name: String, clk: String },
    /// Delete a cell and every pin connection it had.
    RemoveCell { cell: String },
}

impl EcoOp {
    /// Tcl-style text of the op (logged and listed in the ECO table).
    pub fn text(&self) -> String {
        match self {
            EcoOp::SetLutInit { cell, init } => format!("set_property INIT 64'h{init:016X} [get_cells {cell}]"),
            EcoOp::ReconnectPin { cell, pin, net } => format!("connect_net -net {net} -objects [get_pins {cell}/{pin}]"),
            EcoOp::InsertBuffer { net, name } => format!("insert_buffer -net {net} -cell {name}"),
            EcoOp::AddLut { name, init } => format!("create_cell -reference LUT6 {name} ; INIT 64'h{init:016X}"),
            EcoOp::AddFf { name, clk } => format!("create_cell -reference FDRE {name} ; connect CLK {clk}"),
            EcoOp::RemoveCell { cell } => format!("remove_cell {cell}"),
        }
    }

    /// True when the op changes connectivity (needs place/route).
    pub fn is_structural(&self) -> bool {
        !matches!(self, EcoOp::SetLutInit { .. })
    }
}

/// How [`Session::eco_implement`] legalizes the edited netlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EcoImpl {
    /// Incremental place (reusing previous sites) then route.
    PlaceAndRoute,
    /// Keep the placement and re-route only.
    RouteOnly,
}

/// Result of [`Session::write_eco_bitstream`].
#[derive(Clone, Debug)]
pub struct EcoBitstream {
    pub full: Bitstream,
    /// Frames that differ from the pre-ECO image (explicit zero frames
    /// scrub cleared configuration).
    pub incremental: PartialBitstream,
}

fn disconnect(d: &mut helion_ir::Design, cell: &str, pin: Option<&str>) -> usize {
    let c = intern(cell);
    let p = pin.map(intern);
    let mut removed = 0;
    for n in &mut d.nets {
        let before = n.endpoints.len();
        n.endpoints.retain(|e| !(e.cell == c && p.is_none_or(|p| e.pin == p)));
        removed += before - n.endpoints.len();
    }
    removed
}

fn is_output_pin(pin: &str) -> bool {
    matches!(pin, "O" | "Q" | "CO" | "P" | "DO" | "DOA" | "DOB") || pin.starts_with("O") && pin.len() <= 3
}

impl Session {
    /// Apply one ECO op to the design. A LUT `INIT` change on a routed
    /// session also patches the bitstream (no re-implementation needed).
    pub fn apply_eco(&mut self, dev: &Device, op: &EcoOp) -> Result<String, String> {
        let d = self.design.as_mut().ok_or("eco: no design")?;
        let has_cell = |d: &helion_ir::Design, n: &str| d.cell(n).is_some();
        match op {
            EcoOp::SetLutInit { cell, init } => {
                let c = d.cell_mut(cell).ok_or_else(|| format!("eco: no cell {cell}"))?;
                match &mut c.kind {
                    CellKind::Lut6 { init: i } => *i = *init,
                    _ => return Err(format!("eco: {cell} is not a LUT")),
                }
                if self.routed.is_some() {
                    self.eco(dev, cell, *init)?;
                    return Ok(format!("{} (bitstream patched in place)", op.text()));
                }
            }
            EcoOp::ReconnectPin { cell, pin, net } => {
                if !has_cell(d, cell) {
                    return Err(format!("eco: no cell {cell}"));
                }
                if d.net_on(intern(cell), intern(pin)).is_none() && pin.is_empty() {
                    return Err(format!("eco: {cell} has no pin {pin}"));
                }
                if is_output_pin(pin) {
                    let target = intern(net);
                    if let Some(n) = d.nets.iter().find(|n| n.name == target) {
                        let pins = d.pin_index();
                        let drivers = n
                            .endpoints
                            .iter()
                            .filter(|e| is_output_pin(resolve(e.pin)) && e.cell != intern(cell))
                            .count();
                        let _ = pins;
                        if drivers > 0 {
                            return Err(format!("eco: net {net} already has a driver"));
                        }
                    }
                }
                disconnect(d, cell, Some(pin));
                d.connect(net, cell, pin);
            }
            EcoOp::InsertBuffer { net, name } => {
                if has_cell(d, name) {
                    return Err(format!("eco: cell {name} exists"));
                }
                let target = intern(net);
                let idx = d
                    .nets
                    .iter()
                    .position(|n| n.name == target)
                    .ok_or_else(|| format!("eco: no net {net}"))?;
                let is_port = d.ports.iter().any(|p| p.name == target && p.dir == PortDir::Out);
                let (drv, loads): (Vec<Endpoint>, Vec<Endpoint>) = d.nets[idx]
                    .endpoints
                    .iter()
                    .cloned()
                    .partition(|e| is_output_pin(resolve(e.pin)));
                if drv.is_empty() && !d.ports.iter().any(|p| p.name == target && p.dir == PortDir::In) {
                    return Err(format!("eco: net {net} has no driver to buffer"));
                }
                if loads.is_empty() {
                    return Err(format!("eco: net {net} has no loads to buffer"));
                }
                if is_port {
                    return Err(format!("eco: {net} is a top-level output port net"));
                }
                let out = format!("{net}_{name}");
                d.nets[idx].endpoints = drv;
                d.add_cell(name, CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
                d.connect(net, name, "I0");
                let mut n = Net {
                    name: intern(&out),
                    endpoints: loads,
                    attrs: Default::default(),
                };
                n.endpoints.push(Endpoint {
                    cell: intern(name),
                    pin: intern("O"),
                });
                d.merge_net(n);
                d.rebuild_indexes();
            }
            EcoOp::AddLut { name, init } => {
                if has_cell(d, name) {
                    return Err(format!("eco: cell {name} exists"));
                }
                d.add_cell(name, CellKind::Lut6 { init: *init });
                d.connect(&format!("{name}_o"), name, "O");
            }
            EcoOp::AddFf { name, clk } => {
                if has_cell(d, name) {
                    return Err(format!("eco: cell {name} exists"));
                }
                if d.net(clk).is_none() && !d.ports.iter().any(|p| resolve(p.name) == clk) {
                    return Err(format!("eco: no clock net {clk}"));
                }
                d.add_cell(name, CellKind::Hff);
                d.connect(clk, name, "CLK");
                d.connect(&format!("{name}_q"), name, "Q");
            }
            EcoOp::RemoveCell { cell } => {
                let c = intern(cell);
                let ix = d
                    .cells
                    .iter()
                    .position(|x| x.name == c)
                    .ok_or_else(|| format!("eco: no cell {cell}"))?;
                if matches!(d.cells[ix].kind, CellKind::IobIn | CellKind::IobOut) {
                    return Err(format!("eco: {cell} is an I/O buffer (remove the port instead)"));
                }
                d.cells.remove(ix);
                disconnect(d, cell, None);
                d.nets.retain(|n| !n.endpoints.is_empty() || d.ports.iter().any(|p| p.name == n.name));
                d.rebuild_indexes();
            }
        }
        if op.is_structural() {
            // The netlist no longer matches the implementation.
            self.routed = None;
            self.bitstream = None;
        }
        Ok(op.text())
    }

    /// Legalize the edited netlist (see [`EcoImpl`]). Returns the reuse
    /// report of the incremental placement (`None` for route-only).
    pub fn eco_implement(&mut self, dev: &Device, how: EcoImpl) -> Result<Option<ReuseReport>, String> {
        let rep = match how {
            EcoImpl::PlaceAndRoute => {
                let prev = self
                    .placed
                    .clone()
                    .or_else(|| self.impl_checkpoint.clone())
                    .ok_or("eco: no previous placement (implement the design first)")?;
                Some(self.incremental_place(dev, &prev)?)
            }
            EcoImpl::RouteOnly => {
                if self.placed.is_none() {
                    return Err("eco: route-only needs a placed design".into());
                }
                if self.design.as_ref().zip(self.placed.as_ref()).is_some_and(|(d, p)| {
                    let n = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Hff)).count();
                    let have = p.packed.lutffs.iter().map(|l| usize::from(!l.lut_cell.is_empty()) + usize::from(!l.ff_cell.is_empty())).sum::<usize>();
                    n != have
                }) {
                    return Err("eco: cells were added or removed — route-only cannot place them (use place and route)".into());
                }
                None
            }
        };
        let opts = self.route_opts;
        self.route_design_with(dev, opts)?;
        Ok(rep)
    }

    /// Generate the post-ECO bitstream and its incremental partial versus
    /// `base` (the pre-ECO image).
    pub fn write_eco_bitstream(&mut self, dev: &Device, base: &Bitstream) -> Result<EcoBitstream, String> {
        let full = match &self.bitstream {
            Some(b) if self.routed.is_some() => b.clone(),
            _ => self.write_bitstream(dev)?.clone(),
        };
        let incremental = diff_partial_bitstream(dev, base, &full, None)?;
        Ok(EcoBitstream { full, incremental })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;
    use helion_ir::Design;

    fn implemented() -> (Device, Session, Bitstream) {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::Project);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        let base = s.bitstream.clone().unwrap();
        (dev, s, base)
    }

    #[test]
    fn lut_init_eco_patches_bitstream_without_reimplementation() {
        let (dev, mut s, base) = implemented();
        let placed_before = s.placed.clone().unwrap().lutff_sites;
        s.apply_eco(&dev, &EcoOp::SetLutInit { cell: "u_lut0".into(), init: 0x1 }).unwrap();
        assert!(s.routed.is_some(), "INIT ECO keeps the implementation");
        assert_eq!(s.placed.as_ref().unwrap().lutff_sites, placed_before);
        let out = s.write_eco_bitstream(&dev, &base).unwrap();
        assert!(!out.incremental.frames.is_empty(), "the LUT frame changed");
        assert!(out.incremental.frames.len() < out.full.frames.len());
    }

    #[test]
    fn buffer_insertion_rewires_loads_and_reimplements_incrementally() {
        let (dev, mut s, base) = implemented();
        s.apply_eco(&dev, &EcoOp::InsertBuffer { net: "q0".into(), name: "eco_buf".into() }).unwrap();
        let d = s.design.as_ref().unwrap();
        let buffered = d.net("q0_eco_buf").expect("new load net");
        assert!(buffered.endpoints.iter().any(|e| resolve(e.cell) == "eco_buf"));
        assert!(d.net("q0").unwrap().endpoints.iter().any(|e| resolve(e.cell) == "eco_buf"));
        assert!(s.routed.is_none());
        let rep = s.eco_implement(&dev, EcoImpl::PlaceAndRoute).unwrap().unwrap();
        assert!(rep.reused_cells > 0, "surviving cells keep their sites");
        let out = s.write_eco_bitstream(&dev, &base).unwrap();
        assert!(!out.incremental.frames.is_empty());
    }

    #[test]
    fn reconnect_and_remove_validate_and_route_only_refuses_new_cells() {
        let (dev, mut s, _) = implemented();
        assert!(s.apply_eco(&dev, &EcoOp::RemoveCell { cell: "nope".into() }).is_err());
        assert!(s.apply_eco(&dev, &EcoOp::RemoveCell { cell: "u_iob".into() }).is_err());
        s.apply_eco(&dev, &EcoOp::ReconnectPin { cell: "u_lut1".into(), pin: "I1".into(), net: "q2".into() }).unwrap();
        assert_eq!(
            s.design.as_ref().unwrap().net_on(intern("u_lut1"), intern("I1")).map(resolve),
            Some("q2")
        );
        // Pin-only change: route-only is legal.
        s.eco_implement(&dev, EcoImpl::RouteOnly).unwrap();
        s.apply_eco(&dev, &EcoOp::AddLut { name: "eco_lut".into(), init: 0x2 }).unwrap();
        assert!(s.eco_implement(&dev, EcoImpl::RouteOnly).unwrap_err().contains("route-only"));
        s.apply_eco(&dev, &EcoOp::AddFf { name: "eco_ff".into(), clk: "clk".into() }).unwrap();
        s.apply_eco(&dev, &EcoOp::RemoveCell { cell: "eco_lut".into() }).unwrap();
        assert!(s.design.as_ref().unwrap().cell("eco_lut").is_none());
    }
}
