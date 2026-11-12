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
    /// Power-on value (IR attr `INIT`, default 0).
    pub init: bool,
    /// Value loaded when SR is asserted (IR attr `SRVAL`, default 0:
    /// reset; 1: set).
    pub srval: bool,
    /// SR acts asynchronously (FDCE/FDPE) rather than on the clock edge
    /// (FDRE/FDSE). IR attr `SR_ASYNC=1`.
    pub sr_async: bool,
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

/// A packed carry chain (CARRY4).
///
/// The IR's [`helion_ir::CellKind::Carry4`] marks a 4-bit fast carry
/// chain; the 7-series backend places it in a SLICE and emits the CARRY4
/// configuration. Pin nets are captured here so place can keep chained
/// groups adjacent, route can treat the CO-to-CI cascade as a dedicated
/// wire, and emit can select the CYINIT source. Per-bit entries are
/// `None` for unused bits (partial top group); CI is required, CO is
/// optional (chain end may leave it unconnected).
#[derive(Clone, Debug)]
pub struct PackedCarry {
    /// Source IR cell name.
    pub name: String,
    /// Net on CI (carry in: prior group CO, or the CYINIT source net).
    /// Empty for a CYINIT-config head (`cyinit` is `Some`): the CI pin
    /// stays unconnected and no net routes to it.
    pub ci_net: String,
    /// Chain head with CYINIT from config: `Some(v)` selects the
    /// PRECYINIT source (`Some(true)` to C1, `Some(false)` to C0).
    /// `None` is a chained or legacy const-LUT head. C0 is const-0
    /// (the oracle synth netlist ties head CYINIT to const0); a
    /// CYINIT=1 intent must map to C1, never C0.
    pub cyinit: Option<bool>,
    /// Net on CO (carry out to the next group CI).
    pub co_net: Option<String>,
    /// Nets on S0..S3.
    pub s_nets: [Option<String>; 4],
    /// Nets on DI0..DI3.
    pub di_nets: [Option<String>; 4],
    /// Nets on O0..O3 (sum bits).
    pub o_nets: [Option<String>; 4],
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
    pub carries: Vec<PackedCarry>,
    /// Optional pinned site for the first carry-chain head (e.g.
    /// `SLICE_X113Y39`). General mechanism for convergence runs: the
    /// caller pins a golden column, the placer never invents one.
    /// `None` (the default) places the head near the anchor instead.
    pub carry_anchor: Option<String>,
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
                // An unconnected LUT pin reads 1 on silicon, while the IR
                // defines it as 0: copy the pin=0 half of the table over the
                // pin=1 half so the output never depends on that pin.
                let mut init = *init;
                for (i, net) in input_nets.iter().enumerate() {
                    if net.is_none() {
                        let mut t = 0u64;
                        for addr in 0..64u64 {
                            t |= ((init >> (addr & !(1 << i))) & 1) << addr;
                        }
                        init = t;
                    }
                }
                out.luts.push(PackedLut {
                    name: cell.name.clone(),
                    init,
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
                    init: cell.attrs.get("INIT") == Some("1"),
                    srval: cell.attrs.get("SRVAL") == Some("1"),
                    sr_async: cell.attrs.get("SR_ASYNC") == Some("1"),
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
            CellKind::Carry4 => {
                let pin = |p: &str| {
                    pins.net_on(&cell.name, p).map(|s| s.to_string())
                };
                let mut s_nets: [Option<String>; 4] = Default::default();
                let mut di_nets: [Option<String>; 4] = Default::default();
                let mut o_nets: [Option<String>; 4] = Default::default();
                for j in 0..4 {
                    s_nets[j] = pin(&format!("S{j}"));
                    di_nets[j] = pin(&format!("DI{j}"));
                    o_nets[j] = pin(&format!("O{j}"));
                }
                let cyinit = match cell.attrs.get("CYINIT") {
                    None => None,
                    Some(v) if ["1", "true", "TRUE", "yes"].contains(&v) => Some(true),
                    Some(v) if ["0", "false", "FALSE", "no"].contains(&v) => Some(false),
                    Some(v) => {
                        return Err(format!(
                            "pack: CARRY4 {} has bad CYINIT value `{v}`",
                            cell.name
                        ))
                    }
                };
                let ci_net = match pin("CI") {
                    Some(n) => n,
                    None if cyinit.is_some() => String::new(),
                    None => {
                        return Err(format!("pack: CARRY4 {} has no CI net", cell.name));
                    }
                };
                out.carries.push(PackedCarry {
                    name: cell.name.clone(),
                    ci_net,
                    cyinit,
                    co_net: pin("CO"),
                    s_nets,
                    di_nets,
                    o_nets,
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
    insert_input_buffers(netlist, &mut out)?;

    Ok(out)
}

/// Every net that some cell reads but nothing drives must be a top-level
/// input bit (`port` or `port_<bit>`): give it an IBUF. A clock port read
/// as data reads its existing IBUF output. Any other undriven net is
/// rejected by the router, never silently left floating.
fn insert_input_buffers(netlist: &Design, packed: &mut Packed7) -> Result<(), String> {
    use std::collections::{BTreeSet, HashMap, HashSet};
    let mut driven: HashSet<String> = HashSet::new();
    for l in &packed.luts {
        driven.insert(l.output_net.clone());
    }
    for f in &packed.ffs {
        driven.insert(f.q_net.clone());
    }
    for i in &packed.iobs {
        if i.dir == IobDir::In {
            driven.insert(i.net.clone());
        }
    }
    for b in &packed.bufgs {
        driven.insert(b.out_net.clone());
    }
    for c in &packed.carries {
        for o in c.o_nets.iter().flatten() {
            driven.insert(o.clone());
        }
        if let Some(co) = &c.co_net {
            driven.insert(co.clone());
        }
    }
    let mut read: BTreeSet<String> = BTreeSet::new();
    for l in &packed.luts {
        read.extend(l.input_nets.iter().flatten().cloned());
    }
    for f in &packed.ffs {
        read.insert(f.d_net.clone());
        read.extend(f.ce_net.iter().cloned());
        read.extend(f.sr_net.iter().cloned());
    }
    for i in &packed.iobs {
        if i.dir == IobDir::Out {
            read.insert(i.net.clone());
        }
    }
    for c in &packed.carries {
        // DI nets are not wired by the router (see route.rs carry notes).
        read.extend(c.s_nets.iter().flatten().cloned());
        if c.cyinit.is_none() && !c.ci_net.is_empty() {
            read.insert(c.ci_net.clone());
        }
    }
    // Clock ports already have an IBUF; data loads of the raw port net read
    // its output instead.
    let clock_ibuf: HashMap<String, String> = packed
        .bufgs
        .iter()
        .filter_map(|b| b.in_net.strip_suffix("_ibuf").map(|p| (p.to_string(), b.in_net.clone())))
        .collect();
    let mut rename: HashMap<String, String> = HashMap::new();
    let mut new_iobs = Vec::new();
    for net in read {
        if driven.contains(&net) {
            continue;
        }
        if let Some(ib) = clock_ibuf.get(&net) {
            rename.insert(net.clone(), ib.clone());
            continue;
        }
        let port = netlist.ports.iter().filter(|p| p.dir == PortDir::In).find(|p| {
            p.name == net
                || net
                    .strip_prefix(p.name.as_str())
                    .and_then(|r| r.strip_prefix('_'))
                    .map(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(false)
        });
        // Not an input bit: left for the router, which rejects an undriven
        // net loudly (pack also runs on fragments in unit tests).
        let Some(port) = port else { continue };
        let scalar = port.name == net;
        new_iobs.push(PackedIob {
            name: format!("ibuf_{net}"),
            loc: if scalar { port.attrs.get("LOC").map(|s| s.to_string()) } else { None },
            iostandard: port.attrs.get("IOSTANDARD").map(|s| s.to_string()).unwrap_or_else(|| "LVCMOS33".to_string()),
            drive: None,
            slew: None,
            port: port.name.clone(),
            dir: IobDir::In,
            net: net.clone(),
        });
    }
    packed.iobs.extend(new_iobs);
    if !rename.is_empty() {
        let r = |n: &mut String| {
            if let Some(x) = rename.get(n) {
                *n = x.clone();
            }
        };
        for l in packed.luts.iter_mut() {
            for n in l.input_nets.iter_mut().flatten() {
                r(n);
            }
        }
        for f in packed.ffs.iter_mut() {
            r(&mut f.d_net);
            if let Some(n) = f.ce_net.as_mut() {
                r(n);
            }
            if let Some(n) = f.sr_net.as_mut() {
                r(n);
            }
        }
        for i in packed.iobs.iter_mut() {
            if i.dir == IobDir::Out {
                r(&mut i.net);
            }
        }
    }
    Ok(())
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
        d.add_cell("u_ila", CellKind::Ila { net: "x".into() });
        let err = pack(&d).unwrap_err();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    /// Two chained Carry4 groups (second partial, no CO): pack must capture
    /// every pin net so place/route/emit can follow the chain, and fail
    /// loudly on a missing CI.
    fn carry_chain_design() -> Design {
        let mut d = Design::new("carry2");
        d.add_port("clk", PortDir::In);
        d.add_cell("u_c0", CellKind::Carry4);
        d.connect("one", "u_c0", "CI");
        d.connect("co0", "u_c0", "CO");
        for j in 0..4 {
            d.connect(format!("s{j}"), "u_c0", format!("S{j}"));
            d.connect("zero", "u_c0", format!("DI{j}"));
            d.connect(format!("o{j}"), "u_c0", format!("O{j}"));
        }
        // Partial top group: bits 0-1 only, chain end (no CO endpoint).
        d.add_cell("u_c1", CellKind::Carry4);
        d.connect("co0", "u_c1", "CI");
        for j in 0..2 {
            d.connect(format!("s{}", 4 + j), "u_c1", format!("S{j}"));
            d.connect("zero", "u_c1", format!("DI{j}"));
            d.connect(format!("o{}", 4 + j), "u_c1", format!("O{j}"));
        }
        d
    }

    #[test]
    fn pack_carry_captures_chain_pin_nets() {
        let packed = pack(&carry_chain_design()).unwrap();
        assert_eq!(packed.carries.len(), 2);
        let c0 = &packed.carries[0];
        assert_eq!(c0.ci_net, "one");
        assert_eq!(c0.co_net.as_deref(), Some("co0"));
        assert_eq!(c0.s_nets[0].as_deref(), Some("s0"));
        assert_eq!(c0.di_nets[3].as_deref(), Some("zero"));
        assert_eq!(c0.o_nets[2].as_deref(), Some("o2"));
        let c1 = &packed.carries[1];
        // Cascade linkage by net name, not cell naming.
        assert_eq!(c1.ci_net, "co0");
        assert_eq!(c1.co_net, None);
        assert_eq!(c1.s_nets[1].as_deref(), Some("s5"));
        assert_eq!(c1.s_nets[2], None);
        assert_eq!(c1.di_nets[3], None);
        assert_eq!(c1.o_nets[0].as_deref(), Some("o4"));
        assert_eq!(c1.o_nets[3], None);
    }

    #[test]
    fn pack_carry_without_ci_fails_loudly() {
        let mut d = Design::new("noci");
        d.add_cell("u_c", CellKind::Carry4);
        d.connect("co", "u_c", "CO");
        let err = pack(&d).unwrap_err();
        assert!(err.contains("no CI net"), "got: {err}");
    }
}
