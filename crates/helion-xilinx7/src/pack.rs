//! Pack stage: [`helion_ir::Design`] → [`Packed7`].
//!
//! Groups logical IR cells into 7-series packable units and inserts the
//! clocking spine (IBUF + BUFGCTRL) for top-level clock inputs. The output
//! is device-independent except for site-kind choices (SLICEL vs SLICEM is
//! resolved at place time from the device model).

use helion_ir::{CellKind, Design, PortDir};

/// A packed 6-LUT.
#[derive(Clone, Debug)]
pub struct PackedLut {
    /// Source IR cell name.
    pub name: String,
    /// Truth table: `init[addr]` is the output for inputs `{I5..I0}` as addr.
    pub init: u64,
    /// Nets on I0..I5 (`None` = unconnected → tied low at emission).
    pub input_nets: [Option<String>; 6],
    /// Net on O.
    pub output_net: String,
}

/// A packed flip-flop.
#[derive(Clone, Debug)]
pub struct PackedFf {
    /// Source IR cell name.
    pub name: String,
    pub d_net: String,
    pub q_net: String,
    pub clk_net: String,
    pub ce_net: Option<String>,
    pub sr_net: Option<String>,
}

/// A packed I/O buffer (IBUF or OBUF in an IOB33 site).
#[derive(Clone, Debug)]
pub struct PackedIob {
    /// Source IR cell name (synthesized for inferred input IOBs).
    pub name: String,
    /// Top-level port name.
    pub port: String,
    pub dir: IobDir,
    /// Fabric-side net (`I` pin for outputs, `O` pin for inputs).
    pub net: String,
    /// XDC `PACKAGE_PIN` value, e.g. `"E3"`.
    pub loc: Option<String>,
    pub iostandard: String,
    pub drive: Option<String>,
    pub slew: Option<String>,
}

/// IOB direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IobDir {
    In,
    Out,
}

/// A packed global clock buffer (BUFGCTRL).
#[derive(Clone, Debug)]
pub struct PackedBufg {
    /// Source IR cell name (synthesized for inferred clock buffers).
    pub name: String,
    /// Net into the BUFG (`IBUF.O`).
    pub in_net: String,
    /// Global clock net out of the BUFG.
    pub out_net: String,
}

/// A packed block RAM (RAMB18E1).
///
/// The IR's [`helion_ir::CellKind::Bram18`] is a memory block with INIT data
/// but no pin-level interface yet; the 7-series backend places it on a
/// RAMB18E1 site and programs the INIT features. Pin-level memory interface
/// modeling (address/data/clock/enable routing) is future work at the IR
/// level.
#[derive(Clone, Debug)]
pub struct PackedBram {
    /// Source IR cell name.
    pub name: String,
    /// Initialization words (up to 1024 64-bit words for 18Kb).
    pub init: Vec<u64>,
}

/// A packed DSP block (DSP48E1).
///
/// The IR's [`helion_ir::CellKind::Mac27`] is a multiply-accumulate marker
/// with no pin-level interface yet; the 7-series backend places it on a
/// DSP48E1 site configured for combinational multiply-add. Pin-level
/// DSP interface modeling (A/B/C/P routing) is future work at the IR level.
#[derive(Clone, Debug)]
pub struct PackedDsp {
    /// Source IR cell name.
    pub name: String,
}

/// Backend-owned packed netlist for xc7a35t.
#[derive(Clone, Debug, Default)]
pub struct Packed7 {
    pub luts: Vec<PackedLut>,
    pub ffs: Vec<PackedFf>,
    pub iobs: Vec<PackedIob>,
    pub bufgs: Vec<PackedBufg>,
    pub brams: Vec<PackedBram>,
    pub dsps: Vec<PackedDsp>,
}

/// Pack a [`Design`] into 7-series packable units.
///
/// Inserts IBUF + BUFGCTRL for every top-level input port whose net drives
/// at least one FF clock pin (clock inference by use). Returns an error on
/// unsupported cell kinds rather than silently dropping them.
pub fn pack(netlist: &Design) -> Result<Packed7, String> {
    let pins = netlist.pin_index();
    let mut out = Packed7::default();

    for cell in &netlist.cells {
        match &cell.kind {
            CellKind::Lut6 { init } => {
                let mut input_nets: [Option<String>; 6] = Default::default();
                for (i, slot) in input_nets.iter_mut().enumerate() {
                    let pin = format!("I{i}");
                    *slot = pins.net_on(&cell.name, &pin).map(|s| s.to_string());
                }
                let output_net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: LUT {} has no O net", cell.name))?
                    .to_string();
                out.luts.push(PackedLut {
                    name: cell.name.clone(),
                    init: *init,
                    input_nets,
                    output_net,
                });
            }
            CellKind::Hff => {
                let need = |pin: &str| {
                    pins.net_on(&cell.name, pin)
                        .map(|s| s.to_string())
                        .ok_or_else(|| format!("pack: FF {} has no {pin} net", cell.name))
                };
                out.ffs.push(PackedFf {
                    name: cell.name.clone(),
                    d_net: need("D")?,
                    q_net: need("Q")?,
                    clk_net: need("CLK")?,
                    ce_net: pins.net_on(&cell.name, "CE").map(|s| s.to_string()),
                    sr_net: pins
                        .net_on(&cell.name, "SR")
                        .or_else(|| pins.net_on(&cell.name, "RST"))
                        .map(|s| s.to_string()),
                });
            }
            CellKind::IobOut => {
                let net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: IOB {} has no I net", cell.name))?
                    .to_string();
                let port = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                let port_def = netlist.ports.iter().find(|p| p.name == port);
                out.iobs.push(PackedIob {
                    name: cell.name.clone(),
                    loc: port_def.and_then(|p| p.attrs.get("LOC").map(|s| s.to_string())),
                    iostandard: port_def
                        .and_then(|p| p.attrs.get("IOSTANDARD"))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "LVCMOS33".to_string()),
                    drive: port_def.and_then(|p| p.attrs.get("DRIVE").map(|s| s.to_string())),
                    slew: port_def.and_then(|p| p.attrs.get("SLEW").map(|s| s.to_string())),
                    port,
                    dir: IobDir::Out,
                    net,
                });
            }
            CellKind::IobIn => {
                let net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: IBUF {} has no O net", cell.name))?
                    .to_string();
                let port = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                let port_def = netlist.ports.iter().find(|p| p.name == port);
                out.iobs.push(PackedIob {
                    name: cell.name.clone(),
                    loc: port_def.and_then(|p| p.attrs.get("LOC").map(|s| s.to_string())),
                    iostandard: port_def
                        .and_then(|p| p.attrs.get("IOSTANDARD"))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "LVCMOS33".to_string()),
                    drive: None,
                    slew: None,
                    port,
                    dir: IobDir::In,
                    net,
                });
            }
            CellKind::ClockBuf => {
                let in_net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: BUFG {} has no I net", cell.name))?
                    .to_string();
                let out_net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: BUFG {} has no O net", cell.name))?
                    .to_string();
                out.bufgs.push(PackedBufg {
                    name: cell.name.clone(),
                    in_net,
                    out_net,
                });
            }
            CellKind::Bram18 => {
                let init = cell
                    .attrs
                    .get("INIT")
                    .unwrap_or("")
                    .split(',')
                    .filter(|t| !t.is_empty())
                    .map(|t| u64::from_str_radix(t.trim(), 16).unwrap_or(0))
                    .collect();
                out.brams.push(PackedBram {
                    name: cell.name.clone(),
                    init,
                });
            }
            CellKind::Mac27 => {
                out.dsps.push(PackedDsp {
                    name: cell.name.clone(),
                });
            }
            other => {
                return Err(format!(
                    "pack: unsupported CellKind {other:?} for 7-series (cell {})",
                    cell.name
                ));
            }
        }
    }

    insert_clock_spine(netlist, &mut out)?;

    Ok(out)
}

/// Insert IBUF + BUFGCTRL for clock inputs.
///
/// For every net that drives at least one FF `CLK` pin and is not already
/// buffered, find the matching top-level input port and synthesize a
/// [`PackedIob`] (IBUF) plus a [`PackedBufg`] (BUFGCTRL). FF clock pins are
/// rewired onto the BUFG output net (`<port>_bufg`).
fn insert_clock_spine(netlist: &Design, packed: &mut Packed7) -> Result<(), String> {
    let buffered: std::collections::HashSet<String> =
        packed.bufgs.iter().map(|b| b.in_net.clone()).collect();

    let mut clk_nets: Vec<String> = Vec::new();
    for ff in &packed.ffs {
        if !clk_nets.iter().any(|n| n == &ff.clk_net) {
            clk_nets.push(ff.clk_net.clone());
        }
    }

    for clk_net in clk_nets {
        if buffered.contains(clk_net.as_str()) {
            continue;
        }
        let port = netlist
            .ports
            .iter()
            .find(|p| p.dir == PortDir::In && p.name == clk_net)
            .ok_or_else(|| {
                format!("pack: clock net `{clk_net}` has no matching top-level input port")
            })?;
        let bufg_out = format!("{}_bufg", port.name);
        for ff in &mut packed.ffs {
            if ff.clk_net == clk_net {
                ff.clk_net = bufg_out.clone();
            }
        }
        packed.iobs.push(PackedIob {
            name: format!("ibuf_{}", port.name),
            loc: port.attrs.get("LOC").map(|s| s.to_string()),
            iostandard: port
                .attrs
                .get("IOSTANDARD")
                .map(|s| s.to_string())
                .unwrap_or_else(|| "LVCMOS33".to_string()),
            drive: None,
            slew: None,
            port: port.name.clone(),
            dir: IobDir::In,
            net: format!("{}_ibuf", port.name),
        });
        packed.bufgs.push(PackedBufg {
            name: format!("bufg_{}", port.name),
            in_net: format!("{}_ibuf", port.name),
            out_net: bufg_out,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blinky_design() -> Design {
        let mut d = Design::new("blinky");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        d.add_cell("u_inv", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_obuf", CellKind::IobOut);
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_inv", "I0");
        d.connect("d", "u_inv", "O");
        d.connect("d", "u_ff", "D");
        d.connect("clk", "u_ff", "CLK");
        d.connect("q", "u_obuf", "I");
        d.connect("led", "u_obuf", "PAD");
        d
    }

    #[test]
    fn pack_blinky_inserts_clock_spine() {
        let packed = pack(&blinky_design()).unwrap();
        assert_eq!(packed.luts.len(), 1);
        assert_eq!(packed.ffs.len(), 1);
        assert_eq!(packed.iobs.iter().filter(|i| i.dir == IobDir::In).count(), 1);
        assert_eq!(packed.bufgs.len(), 1);
        assert_eq!(packed.ffs[0].clk_net, "clk_bufg");
        assert_eq!(packed.bufgs[0].in_net, "clk_ibuf");
        assert_eq!(packed.bufgs[0].out_net, "clk_bufg");
    }

    #[test]
    fn pack_rejects_unsupported_loudly() {
        let mut d = Design::new("bad");
        d.add_cell("u_mac", CellKind::Mac27);
        let err = pack(&d).unwrap_err();
        assert!(err.contains("unsupported"), "got: {err}");
    }
}
