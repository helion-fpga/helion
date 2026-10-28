//! Pack stage: [`helion_ir::Design`] → [`Packed7`].
//!
//! Groups logical IR cells into 7-series packable units and inserts the
//! clocking spine (IBUF + BUFGCTRL) for top-level clock inputs. The output
//! is device-independent except for site-kind choices (SLICEL vs SLICEM is
//! resolved at place time from the device model).

use helion_ir::{CellKind, Design, PortDir};
use std::collections::HashSet;

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
/// The IR's [`helion_ir::CellKind::Carry4`] marks a 4-bit fast carry chain; the
/// 7-series backend places it in a SLICE and emits the CARRY4 configuration.
///
/// Everything emission needs is captured here. A CARRY4 is *not* configured by
/// its propagate muxes alone: each lane's `S`/`DI` come from the slice's
/// `O6`/`O5`, the chain entry is selected by `PRECYINIT`, and the FF that
/// consumes a sum bit must select the carry path (`AFFMUX = XOR`). Emitting the
/// mux bits with any of that missing configures a chain with an undefined
/// initial carry, which silently produces wrong sums -- so the pins are carried
/// explicitly rather than re-derived at emission time.
#[derive(Clone, Debug)]
pub struct PackedCarry {
    /// Source IR cell name.
    pub name: String,
    /// Chain identity and this element's position in it (from the IR attrs
    /// `chain` / `carry_index` / `chain_len`). Placement needs these to keep a
    /// chain in one column; emission needs them to pair lanes with sums.
    pub chain: String,
    pub index: usize,
    pub chain_len: usize,
    /// `PRECYINIT` source for the chain's first element: `"0"` or `"1"`.
    /// `None` for every element past the first, which is fed by `CIN`.
    pub cyinit: Option<String>,
    /// Per-lane operand nets, MSB-last (`[A, B, C, D]`), as `(a, b)` nets.
    /// `None` is a constant-0 operand bit, already folded into the LUT INITs.
    pub lane_operands: [Option<(Option<String>, Option<String>)>; 4],
    /// The slice-input pin (1..=6) each operand net lands on, per lane and
    /// operand slot (`[a, b]`), `None` when unknown (routers fall back to 6).
    /// The chain-start lane folds its +1 through `I2` (slice pin `A3`, fed by
    /// `IMUX1`) — the hard route to `A6` while the mask reads `A3` left bit 0
    /// sampling a VCC tie and froze the counter one increment past 2^24.
    pub lane_operand_pins: [[Option<u8>; 2]; 4],
    /// Per-lane `S` / `DI` LUT INITs (`O6` / `O5`).
    pub lane_s_init: [u64; 4],
    pub lane_di_init: [u64; 4],
    /// Sum nets for lanes A..D (`O0..O3`); `None` past the end of the chain.
    pub lane_sum_net: [Option<String>; 4],
    /// Net carrying the inter-element carry out (`COUT`) to the next element's
    /// `CIN`; `None` for the last element of a chain.
    pub cout_net: Option<String>,
    /// Net on this element's `CIN` (the previous element's `COUT`).
    pub cin_net: Option<String>,
}

impl PackedCarry {
    /// A single-element carry with no pins.
    ///
    /// Only for tests that need a carry cell to exist without describing a real
    /// chain (e.g. the emitter's refusal check). Real carries come from
    /// [`pack_carry4`], which errors rather than leaving pins unset.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            chain: String::new(),
            index: 0,
            chain_len: 1,
            cyinit: None,
            lane_operands: Default::default(),
            lane_operand_pins: Default::default(),
            lane_s_init: [0; 4],
            lane_di_init: [0; 4],
            lane_sum_net: Default::default(),
            cout_net: None,
            cin_net: None,
        }
    }
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
    // Output IOBs dropped because their port bit has no PACKAGE_PIN.
    let mut unconstrained_outputs: Vec<String> = Vec::new();

    // Cells that a CARRY4 owns, and that must therefore NOT be packed as
    // free-standing LUTs.
    //
    // A carry lane's `S` (and `DI`) function is implemented by a LUT that lives
    // in the CARRY4's OWN slice -- an intra-slice connection. `pack_carry4`
    // reads those LUTs' INITs into `PackedCarry::lane_s_init` and `emit` writes
    // them into the carry element's slice, so packing them again as ordinary
    // LUTs would place a duplicate copy somewhere else. That duplicate is not
    // merely wasteful: it was placed 37 rows away and the design was dead on
    // hardware while every netlist-level check passed.
    //
    // Identify them by what actually drives the carry's lane pins, rather than
    // by name, so the rule holds however the front end names its cells.
    let mut carry_owned: HashSet<String> = HashSet::new();
    for cell in &netlist.cells {
        if !matches!(cell.kind, CellKind::Carry4) {
            continue;
        }
        for letter in ["A", "B", "C", "D"] {
            for pin in [format!("{letter}S"), format!("{letter}DI")] {
                let Some(net) = pins.net_on(&cell.name, &pin) else {
                    continue;
                };
                if let Some(lut) = netlist.cells.iter().find(|c| {
                    matches!(c.kind, CellKind::Lut6 { .. })
                        && pins.net_on(&c.name, "O") == Some(net)
                }) {
                    carry_owned.insert(lut.name.clone());
                }
            }
        }
    }

    for cell in &netlist.cells {
        match &cell.kind {
            CellKind::Lut6 { init } => {
                // Owned by a CARRY4's lane: emitted into that carry element's
                // slice, not placed independently. See `carry_owned` above.
                if carry_owned.contains(&cell.name) {
                    continue;
                }
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
                // An output IOB whose port bit has no PACKAGE_PIN is DROPPED,
                // matching Vivado, which leaves an unconstrained port unbonded
                // and still completes the build.
                //
                // A vector output is packed one IOB per bit, so `output reg
                // [7:0] led` with an XDC that constrains only `led[0]` -- the
                // common case, and what nearly every board tutorial writes --
                // used to abort the whole build. Keeping the IOB but not
                // placing it is not enough: its net has a load endpoint and no
                // driver, so routing then failed with "no wire for load .O".
                // Dropping it here removes the pad from the netlist entirely,
                // so the unconstrained bit is never built.
                let loc = port_attr(netlist, &port, &pad, &cell.name, "LOC").map(str::to_string);
                if loc.is_none() {
                    unconstrained_outputs.push(format!("{} (port {})", cell.name, port));
                    continue;
                }
                out.iobs.push(PackedIob {
                    name: cell.name.clone(),
                    loc,
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
                out.carries.push(pack_carry4(netlist, cell)?);
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

    if !unconstrained_outputs.is_empty() {
        eprintln!(
            "pack: WARNING: {} output IOB(s) have no PACKAGE_PIN and were dropped: {}",
            unconstrained_outputs.len(),
            unconstrained_outputs.join(", ")
        );
    }

    Ok(out)
}

/// Extract one IR `Carry4` cell into a [`PackedCarry`] with its real pins.
///
/// The IR cell carries its configuration on its own pins and on the pins of the
/// per-lane LUT pair that synth emitted alongside it:
///
/// * `{A,B,C,D}S` / `{A,B,C,D}DI` — connected to the lane LUT outputs
/// * `O0..O3` — the sum bits
/// * `CIN` / `COUT` — the inter-element carry
/// * attrs `chain`, `carry_index`, `chain_len`, `cyinit`
///
/// The lane LUT INITs are read back from those LUT cells (not re-derived), so
/// packing cannot disagree with what synthesis actually emitted. Returns an
/// error rather than a half-populated carry, because a carry missing its
/// `PRECYINIT` would be emitted with an undefined chain entry.
fn pack_carry4(netlist: &Design, cell: &helion_ir::Cell) -> Result<PackedCarry, String> {
    let pins = netlist.pin_index();
    let attr = |k: &str| cell.attrs.get(k).map(|s| s.to_string());
    let index: usize = attr("carry_index")
        .ok_or_else(|| format!("pack: Carry4 {} has no carry_index attr", cell.name))?
        .parse()
        .map_err(|_| format!("pack: Carry4 {} has a non-numeric carry_index", cell.name))?;
    let chain_len: usize = attr("chain_len")
        .ok_or_else(|| format!("pack: Carry4 {} has no chain_len attr", cell.name))?
        .parse()
        .map_err(|_| format!("pack: Carry4 {} has a non-numeric chain_len", cell.name))?;
    let chain = attr("chain").unwrap_or_else(|| cell.name.clone());

    // The lane LUTs are named after the cell plus the lane letter, with an `s`
    // or `d` suffix for the S (O6) and DI (O5) functions respectively.
    let lane_letters = ["A", "B", "C", "D"];
    let mut lane_operands: [Option<(Option<String>, Option<String>)>; 4] = Default::default();
    let mut lane_operand_pins: [[Option<u8>; 2]; 4] = Default::default();
    let mut lane_s_init = [0u64; 4];
    let mut lane_di_init = [0u64; 4];
    let mut lane_sum_net: [Option<String>; 4] = Default::default();

    for (lane, letter) in lane_letters.iter().enumerate() {
        // S LUT drives the `{letter}S` pin; DI LUT drives `{letter}DI`.
        for (suffix, is_s) in [("s", true), ("d", false)] {
            let pin = format!("{letter}{}", if is_s { "S" } else { "DI" });
            let Some(lut_out) = pins.net_on(&cell.name, &pin) else {
                // An unconnected lane S/DI is legal (a lane past the end of the
                // chain); leave its INIT at 0 so the lane is a defined 0.
                continue;
            };
            // The LUT driving that net, and its stored INIT.
            let Some(lut) = netlist.cells.iter().find(|c| {
                matches!(c.kind, CellKind::Lut6 { .. }) && pins.net_on(&c.name, "O") == Some(lut_out)
            }) else {
                return Err(format!(
                    "pack: Carry4 {} lane {letter} {pin} is driven by net {lut_out}, \
                     which has no LUT driver",
                    cell.name
                ));
            };
            let CellKind::Lut6 { init } = lut.kind else {
                unreachable!("filtered to Lut6 above")
            };
            if is_s {
                lane_s_init[lane] = init;
            } else {
                lane_di_init[lane] = init;
            }
            // Operand nets: every LUT input pin that carries a net, with the
            // slice pin (1..=6) it reaches (`I0`..`I5` -> `A1`..`A6`).
            //
            // The plain lanes take their operand on `I5` (`A6`), the DI LUT
            // its operand on `I0` (`A1`), and the chain-start lane folds its
            // +1 through `I2` (`A3`/IMUX1). Probing only some spellings left
            // the fold lane with no operand at all, so the router was never
            // asked to drive `A3` and the mask sampled a VCC tie.
            let mut ins: Vec<(u8, String)> = ["I5", "I4", "I3", "I2", "I1", "I0"]
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    pins.net_on(&lut.name, p).map(|s| ((6 - i) as u8, s.to_string()))
                })
                .collect();
            ins.sort_by_key(|(p, _)| std::cmp::Reverse(*p));
            let the_pin = |slot: usize| ins.get(slot).map(|(p, _)| *p);
            let the_net = |slot: usize| ins.get(slot).map(|(_, n)| n.clone());
            let a = the_net(0);
            let b = the_net(1);
            // Record operands once, from the S LUT (both lanes see the same pair).
            if is_s {
                lane_operand_pins[lane] = [the_pin(0), the_pin(1)];
                lane_operands[lane] = Some((a, b));
            }
        }
        lane_sum_net[lane] = pins
            .net_on(&cell.name, &format!("O{lane}"))
            .map(|s| s.to_string());
    }

    Ok(PackedCarry {
        name: cell.name.clone(),
        chain,
        index,
        chain_len,
        cyinit: attr("cyinit"),
        lane_operands,
        lane_operand_pins,
        lane_s_init,
        lane_di_init,
        lane_sum_net,
        cout_net: pins.net_on(&cell.name, "COUT").map(|s| s.to_string()),
        cin_net: pins.net_on(&cell.name, "CIN").map(|s| s.to_string()),
    })
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
        // The clock may come from a top-level input, or from logic inside the
        // design (a LUT driving an FF CLK, which `examples/timing_lut_as_clock.sv`
        // exercises). An input port needs an IBUF in front of its BUFG; an
        // internal net is already a fabric signal, so it drives the BUFG
        // directly. Rejecting the latter made `pack` refuse a design that
        // Vivado accepts, and the message ("no matching top-level input port")
        // described an internal implementation detail rather than the problem.
        let port = netlist
            .ports
            .iter()
            .find(|p| p.dir == PortDir::In && p.name == clk_net);
        let (clk_src, bufg_out) = match port {
            Some(port) => {
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
                (
                    format!("{}_ibuf", port.name),
                    format!("{}_bufg", port.name),
                )
            }
            None => {
                // Internal clock. Name the buffer after the net so two
                // different internal clocks cannot collide.
                let stem = clk_net.trim_start_matches('\\').to_string();
                (clk_net.clone(), format!("{stem}_bufg"))
            }
        };
        for ff in &mut packed.ffs {
            if ff.clk_net == clk_net {
                ff.clk_net = bufg_out.clone();
            }
        }
        packed.bufgs.push(PackedBufg {
            name: format!("bufg_{}", clk_net.trim_start_matches('\\')),
            in_net: clk_src,
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
