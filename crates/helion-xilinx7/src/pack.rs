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

/// A packed carry chain (CARRY4).
///
/// The IR's [`helion_ir::CellKind::Carry4`] marks a 4-bit fast carry
/// chain; the 7-series backend places it in a SLICE and emits the CARRY4
/// configuration. Synth inference from adder logic is future work.
#[derive(Clone, Debug)]
pub struct PackedCarry {
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
    pub carries: Vec<PackedCarry>,
}

/// Pack a [`Design`] into 7-series packable units.
///
/// Inserts IBUF + BUFGCTRL for every top-level input port whose net drives
/// at least one FF clock pin (clock inference by use). Returns an error on
/// unsupported cell kinds rather than silently dropping them.
/// Make a 6-LUT truth table independent of its unconnected inputs.
///
/// `init[addr]` is indexed by `{I5..I0}`, i.e. bit `i` of `addr` is input Ii.
/// An input listed as `None` in `input_nets` is NOT tied low in hardware -- it
/// floats -- so every address pair that differs only in that bit must produce
/// the same output. This folds the table over each floating input's address
/// bit, choosing the value that the *connected* inputs imply. The result
/// evaluates identically no matter what voltage the floating pin settles at.
///
/// With every input connected this is the identity, so fully-used LUTs are
/// unchanged.
fn replicate_unused_inputs(init: u64, input_nets: &[Option<String>; 6]) -> u64 {
    let mut out = init;
    for i in 0..6 {
        if input_nets[i].is_some() {
            continue;
        }
        // Fold address bit i away: for each base address whose bit i is 0,
        // write its value into both the 0 and 1 variants.
        let bit = 1u32 << i;
        let mut folded = 0u64;
        for addr in 0..64u32 {
            if addr & bit == 0 {
                let v = if (out >> addr) & 1 == 1 { 1u64 } else { 0 };
                // Both variants take the same value, so a floating pin cannot
                // change the output.
                let a0 = addr;
                let a1 = addr | bit;
                if v == 1 {
                    folded |= (1u64 << a0) | (1u64 << a1);
                }
            }
        }
        out = folded;
    }
    out
}

/// Resolve the top-level port an IOB cell belongs to.
///
/// The `PAD` pin is bound to the port's *net*, which for an inferred IOB is
/// usually the port name but need not be (a user net can alias it). Looking
/// the port up by the raw PAD net name therefore failed OPEN: when the names
/// differed, `port_def` came back `None` and every constraint on that port --
/// `LOC` above all -- was silently discarded, so the design was placed at a
/// default pin. Fall back to the net's endpoints and then to the cell name so
/// a constrained port is always found.
fn resolve_port(netlist: &Design, pad: &str, cell_name: &str) -> String {
    if netlist.ports.iter().any(|p| p.name == pad) {
        return pad.to_string();
    }
    // A net by that name: follow it to the port whose name it represents.
    if let Some(net) = netlist.nets.iter().find(|n| n.name == pad) {
        for ep in &net.endpoints {
            if netlist.ports.iter().any(|p| p.name == ep.cell) {
                return ep.cell.clone();
            }
        }
    }
    // Cell name is the last resort and matches the inferred-IOB convention.
    if netlist.ports.iter().any(|p| p.name == cell_name) {
        return cell_name.to_string();
    }
    // A bit-qualified pad net (`led_0`, emitted one per output bit) resolves to
    // its port by stripping the bit suffix; without this the port lookup would
    // fail and the IOB would lose its LOC.
    if let Some(base) = strip_bit_suffix(pad) {
        if netlist.ports.iter().any(|p| p.name == base) {
            return base.to_string();
        }
    }
    // `pad[0]` style, written by sources that declare separate bit ports.
    if let Some(open) = pad.find('[') {
        let base = &pad[..open];
        if netlist.ports.iter().any(|p| p.name == base) {
            return base.to_string();
        }
    }
    pad.to_string()
}

/// `led_3` -> `led`, when the tail is a bit index and the base is non-empty.
fn strip_bit_suffix(net: &str) -> Option<&str> {
    let idx = net.rfind('_')?;
    let (base, tail) = net.split_at(idx);
    let digits = &tail[1..];
    if base.is_empty() || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(base)
}

/// Resolve the `LOC` / `IOSTANDARD` (etc.) attribute for one IOB.
///
/// An XDC constrains the individual bits of a vector port, writing
/// `set_property PACKAGE_PIN T22 [get_ports {led[0]}]`. A vector output is
/// packed as one IOB per bit, but every one of those IOBs resolves its port to
/// the same vector name `led`, so a plain `ports[led].LOC` lookup finds nothing
/// and placement then refuses the design with "has no PACKAGE_PIN LOC".
///
/// The per-bit key is recovered from the IOB's own net: `helion-sv` names the
/// pad net for output bit `n` as `<port>[<n>]` or `<port>_<n>`, and the port's
/// `LOC` may itself carry a whole-vector value. Try, in order: the bit-qualified
/// port name, the bit-qualified net name, then the plain port name (which also
/// covers the scalar and single-bit cases).
fn port_attr<'a>(
    netlist: &'a Design,
    port: &str,
    net: &str,
    cell_name: &str,
    attr: &str,
) -> Option<&'a str> {
    let get = |name: &str| -> Option<&'a str> {
        netlist
            .ports
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| p.attrs.get(attr))
    };
    // `PinConstraints::apply` stores a bit-qualified constraint as `LOC[3]`
    // on the vector port, so look for that key on the resolved port too.
    let bit = bit_suffix(net.rsplit('/').next().unwrap_or(net));
    if let Some(b) = bit {
        if let Some(p) = netlist.ports.iter().find(|p| p.name == port) {
            if let Some(v) = p.attrs.get(&format!("{attr}[{b}]")) {
                return Some(v);
            }
        }
    }
    // A port literally named `led[0]` (helion-sv splits vectors this way when
    // the source declares separate bits).
    let net_name = net.rsplit('/').next().unwrap_or(net);
    for candidate in [
        net_name.to_string(),
        net_name.replace('.', "[").trim_end_matches(']').to_string(),
        format!("{port}[{}]", bit_suffix(net_name).unwrap_or(u32::MAX)),
        cell_name.to_string(),
    ] {
        if candidate.ends_with("[4294967295]") {
            continue;
        }
        if let Some(v) = get(&candidate) {
            return Some(v);
        }
    }
    get(port)
}

/// Trailing integer of a net name, e.g. `led_3` or `led[3]` -> `3`.
/// Whether `net` is the bit-`n` net of port `port`, i.e. `btn_3` for `btn`.
///
/// Requires the separator so a port named `btn` does not swallow an unrelated
/// net named `btnx` or `btn_extra`.
fn is_bit_net_of(net: &str, port: &str) -> bool {
    net.len() > port.len() + 1
        && net.starts_with(port)
        && net.as_bytes()[port.len()] == b'_'
        && net[port.len() + 1..].chars().all(|c| c.is_ascii_digit())
}

fn bit_suffix(net: &str) -> Option<u32> {
    if let Some(open) = net.rfind('[') {
        if let Some(close) = net[open..].find(']') {
            if let Ok(n) = net[open + 1..open + close].parse() {
                return Some(n);
            }
        }
    }
    let tail: String = net
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail.parse().ok()
}

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
                // An unconnected LUT input FLOATS on silicon: it is not tied
                // low. The 6-LUT then reads an arbitrary 16-entry block of its
                // 64-entry table depending on that input's voltage, so a
                // 2-input AND can evaluate the wrong function. X-Ray ships no
                // `UNUSED` tie-off feature, so the only place to make this
                // safe is the truth table: replicate the function across every
                // don't-care combination so the result cannot depend on a
                // floating pin.
                //
                // `init[addr]` is addressed by `{I5..I0}`, so bit `i` of the
                // address is input Ii.
                let init = replicate_unused_inputs(*init, &input_nets);
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
                });
            }
            CellKind::IobOut => {
                let net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: IOB {} has no I net", cell.name))?
                    .to_string();
                let pad = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                let port = resolve_port(netlist, &pad, &cell.name);
                out.iobs.push(PackedIob {
                    name: cell.name.clone(),
                    // The PAD net, not the driver: for a vector output it is
                    // bit-qualified (`led_0`), which is what identifies the
                    // `LOC[0]` constraint. The driver net (`cnt_26`) says
                    // nothing about which pin this IOB owns.
                    loc: port_attr(netlist, &port, &pad, &cell.name, "LOC").map(str::to_string),
                    iostandard: port_attr(netlist, &port, &pad, &cell.name, "IOSTANDARD")
                        .map(str::to_string)
                        .unwrap_or_else(|| "LVCMOS33".to_string()),
                    drive: port_attr(netlist, &port, &pad, &cell.name, "DRIVE").map(str::to_string),
                    slew: port_attr(netlist, &port, &pad, &cell.name, "SLEW").map(str::to_string),
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
                let pad = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                let port = resolve_port(netlist, &pad, &cell.name);
                out.iobs.push(PackedIob {
                    name: cell.name.clone(),
                    loc: port_attr(netlist, &port, &pad, &cell.name, "LOC").map(str::to_string),
                    iostandard: port_attr(netlist, &port, &pad, &cell.name, "IOSTANDARD")
                        .map(str::to_string)
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
                out.carries.push(PackedCarry {
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
    insert_data_input_iobs(netlist, &mut out)?;

    Ok(out)
}

/// Give every remaining top-level input port an input IOB, one per bit.
///
/// `insert_clock_spine` only creates an input IOB for a net that drives an FF
/// clock. Every other input port was left with no IOB at all, so the pad net
/// (`btn_0`) had no driver anywhere in the netlist and routing refused the
/// design with "net `btn_0` has 0 drivers (need exactly 1)". That made any
/// design with a data input -- a button, a switch, a UART pin -- unbuildable,
/// which is why every board test so far was output-only.
///
/// One IOB per bit, named after the bit's net (`btn_0`), for the same reason
/// the output side is bit-qualified: an XDC constrains vector bits
/// individually, so `PinConstraints` stores `LOC[0]`..`LOC[4]` on the vector
/// port and nothing under a plain `LOC`. `port_attr` recovers the bit from the
/// net's `_<n>` suffix, so a bit-qualified net is what makes the constraint
/// findable. A single IOB for the whole vector would have neither a plain LOC
/// nor a suffix and would be refused with "IOB has no PACKAGE_PIN LOC".
///
/// A width-1 port keeps its bare name (`bit_name` does the same), so a scalar
/// input still resolves through a plain `LOC`.
fn insert_data_input_iobs(netlist: &Design, packed: &mut Packed7) -> Result<(), String> {
    // A clock spine consumes `<port>` through `<port>_ibuf`, so the port is
    // already padded if its name is present among the input IOBs.
    let covered: std::collections::HashSet<String> = packed
        .iobs
        .iter()
        .filter(|i| i.dir == IobDir::In)
        .map(|i| i.port.clone())
        .collect();

    for port in &netlist.ports {
        if port.dir != PortDir::In || covered.contains(&port.name) {
            continue;
        }
        // Discover the bits from the nets that actually exist rather than a
        // declared width: `Port` carries no width, and the frontend already
        // names input bits `<port>_<n>` only when it uses them. A bit that no
        // logic reads has no net to drive, so there is nothing to pad.
        let mut bits: Vec<String> = netlist
            .nets
            .iter()
            .filter(|n| n.name == port.name || is_bit_net_of(&n.name, &port.name))
            .map(|n| n.name.clone())
            .collect();
        bits.sort();
        bits.dedup();
        if bits.is_empty() {
            bits.push(port.name.clone());
        }
        let width = bits.len();
        for net in bits {
            let bit = bit_suffix(&net).unwrap_or(0);
            packed.iobs.push(PackedIob {
                // Unique per bit: a shared name made two IOBs indistinguishable
                // in reports and in the placement error messages.
                name: format!("ibuf_{}", net),
                port: port.name.clone(),
                dir: IobDir::In,
                net,
                // `PinConstraints` stores a vector bit's pin as `LOC[3]` on the
                // vector port and nothing under a plain `LOC`, so resolve the
                // bit-qualified key first and fall back to the plain one for a
                // scalar port.
                loc: port
                    .attrs
                    .get(&format!("LOC[{bit}]"))
                    .or_else(|| port.attrs.get("LOC"))
                    .map(|s| s.to_string()),
                iostandard: port
                    .attrs
                    .get(&format!("IOSTANDARD[{bit}]"))
                    .or_else(|| port.attrs.get("IOSTANDARD"))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "LVCMOS33".to_string()),
                drive: None,
                slew: None,
            });
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
}

#[cfg(test)]
mod lut_tieoff_tests {
    use super::replicate_unused_inputs;

    fn nets(used: &[usize]) -> [Option<String>; 6] {
        std::array::from_fn(|i| {
            if used.contains(&i) {
                Some(format!("n{i}"))
            } else {
                None
            }
        })
    }

    /// Evaluate the packed table as hardware would: address bits come from the
    /// connected inputs, and the floating inputs take an arbitrary value.
    fn eval(init: u64, addr: u32) -> bool {
        (init >> addr) & 1 == 1
    }

    #[test]
    fn two_input_and_is_independent_of_floating_inputs() {
        // AND(I0,I1): 1 only at addr 0b000011 = 3.
        let and2 = 1u64 << 3;
        let packed = replicate_unused_inputs(and2, &nets(&[0, 1]));
        // I0/I1 give the real inputs; I2..I5 float, so try every combination of
        // them and require the same answer.
        for f in 0..16u32 {
            let addr = (f << 2) | 0b00;
            assert!(!eval(packed, addr), "AND must be 0 for addr {addr:#08b}");
        }
        for f in 0..16u32 {
            let addr = (f << 2) | 0b11;
            assert!(eval(packed, addr), "AND must be 1 for addr {addr:#08b}");
        }
    }

    #[test]
    fn fully_connected_lut_is_unchanged() {
        let t = 0xDEAD_BEEF_1234_5678u64;
        assert_eq!(replicate_unused_inputs(t, &nets(&[0, 1, 2, 3, 4, 5])), t);
    }

    #[test]
    fn buffer_with_one_input_is_constant_across_floats() {
        // Buffer I0: table has 1 at odd addresses (addr bit0 = I0 = 1).
        let buf = 0xAAAA_AAAA_AAAA_AAAAu64;
        let packed = replicate_unused_inputs(buf, &nets(&[0]));
        for f in 0..32u32 {
            let addr_lo = (f << 1) | 0; // I0 = 0
            assert!(!eval(packed, addr_lo), "I0=0 must give 0");
            let addr_hi = (f << 1) | 1; // I0 = 1
            assert!(eval(packed, addr_hi), "I0=1 must give 1");
        }
    }
}
