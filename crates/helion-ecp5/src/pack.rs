//! Pack stage: [`helion_backend::NetlistIr`] → [`PackedEcp5`].
//!
//! Maps the vendor-neutral IR onto ECP5 (LFE5U-85F) primitives:
//! - `Lut6` → LUT4s via Shannon decomposition (an ECP5 slice holds 2x
//!   LUT4; the hard `MUX2_LUT5/6/7/8` wide-function muxes are not used in
//!   v1, plain Shannon is used instead).
//! - `Hff` → slice flip-flop, paired with its driver LUT when possible.
//! - `Carry4` → carry chains ("ALU mode" slices): one ALU LUT per bit
//!   computing `S[i] ^ carry[i]`; the dedicated CCU2 carry computes
//!   `S[i] ? carry[i] : DI[i]` (Xilinx MUXCY semantics, preserved
//!   bit-for-bit).
//! - `Bram18` → `DP16KD` (the INIT payload is carried for the bitstream
//!   worker; config bits are its job).
//! - `Mac27` → `MULT18X18D`.
//! - `BlackBox` `PLLB*`/`EHXPLL*` → PLL with divider parameters passed
//!   through.
//! - `IobIn`/`IobOut` → PIO sites resolved through the Trellis package pin
//!   map (a `LOC` port attribute naming a package pin is required).
//! - `ClockBuf` → global-clock marker.
//! - `Ila` and unrecognized `BlackBox` modules are loud errors.
//!
//! Fail-loud: anything unmappable (unsupported cell, missing pin, bad
//! attribute, over-capacity) returns `Err`. Nothing is silently dropped.

use crate::db::PackagePin;
use crate::device::Device;
use helion_backend::NetlistIr;
use helion_ir::{intern, resolve, Attrs, CellKind, Design, PinIndex, PortDir, Spur};
use std::collections::{HashMap, HashSet};

/// ECP5 sysMEM block size in bits (`DP16KD` = 18Kb).
const EBR_BITS: usize = 18_432;
/// Widest multiply that fits one `MULT18X18D`.
const DSP_MAX_WIDTH: u32 = 18;
/// Prefix for dedicated CCU2 carry nets. These are not fabric nets: the
/// router must skip them, and place/bitstream treat them as dedicated
/// carry wires. `$carry$zero` / `$carry$one` are the chain-head constant
/// carry-ins; `$carry$c{C}b{B}` is the carry out of bit `B` feeding bit
/// `B+1` of chain `C`.
pub const CARRY_NET_PREFIX: &str = "$carry$";

/// A packed ECP5 LUT4 (half of a slice).
#[derive(Clone, Debug)]
pub struct PackedEcp5Lut {
    /// Source IR cell name (synthesized names for decomposition products).
    pub name: String,
    /// 16-bit truth table: `init[n]` is the output for input value `n`
    /// where `n = D*8 + C*4 + B*2 + A` and the inputs are
    /// `input_nets[0..4] = [A, B, C, D]`.
    pub init: u16,
    /// Nets on A, B, C, D (`None` = unconnected, tied low at emission).
    /// For ALU LUTs, `input_nets[1]` is a dedicated carry net (see
    /// [`CARRY_NET_PREFIX`]); the router must skip it.
    pub input_nets: [Option<String>; 4],
    /// Net on F.
    pub output_net: String,
    /// Index into [`PackedEcp5::ffs`] of the FF paired into the same
    /// slice half, if any.
    pub paired_ff: Option<usize>,
    /// Set for ALU-mode (carry chain) LUTs: placement must put this LUT
    /// in the chain's `bit`-th position (consecutive slice halves in one
    /// column, carry rippling LUT0 → LUT1 → next slice upward).
    pub alu: Option<AluBit>,
}

/// Placement/mode data for an ALU-mode (carry chain) LUT.
#[derive(Clone, Debug)]
pub struct AluBit {
    /// Index into [`PackedEcp5::alu_chains`].
    pub chain: usize,
    /// Bit position within the chain (0 = LSB, carry flows upward).
    pub bit: usize,
    /// The carry-generate net (`DI[i]`): a dedicated CCU2 input, not a
    /// LUT input and not a fabric net. `None` = constant 0. The router
    /// must treat this net as terminated at the carry chain.
    pub di_net: Option<String>,
}

/// One carry chain: the ordered ALU LUTs of one or more chained
/// [`helion_ir::CellKind::Carry4`] cells. Placed as that many consecutive
/// slice halves in a single column.
#[derive(Clone, Debug)]
pub struct PackedEcp5AluChain {
    /// True when the chain-head carry-in is constant 1 (`CYINIT=1`,
    /// e.g. subtractors); false for constant 0. The bitstream worker
    /// programs the CCU2 `INJECT` from this.
    pub cin_one: bool,
    /// Number of carry bits (= number of ALU LUTs in the chain).
    pub bits: usize,
}

/// A packed ECP5 flip-flop (half of a slice).
#[derive(Clone, Debug)]
pub struct PackedEcp5Ff {
    pub name: String,
    pub d_net: String,
    pub q_net: String,
    pub clk_net: String,
    /// Index into [`PackedEcp5::luts`] of the driver LUT this FF shares a
    /// slice half with, if its D net is driven by exactly one packed LUT.
    pub packed_lut: Option<usize>,
}

/// Direction of a packed PIO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoDir {
    In,
    Out,
}

/// A packed PIO block.
#[derive(Clone, Debug)]
pub struct PackedEcp5Io {
    pub name: String,
    pub port: String,
    pub dir: IoDir,
    /// Fabric-side net.
    pub net: String,
    /// Package pin name, e.g. `"A10"` (from the port's `LOC` attribute,
    /// validated against the Trellis package pin map).
    pub pin: String,
    /// IO bank number (from `pio_metadata`).
    pub bank: u32,
    /// Bel column of the PIO site.
    pub pio_col: u32,
    /// Bel row of the PIO site.
    pub pio_row: u32,
    /// Which PIO in the tile: `"A"`, `"B"`, `"C"`, or `"D"`.
    pub pio: String,
}

/// A packed global clock: the net driven onto the global clock network.
#[derive(Clone, Debug)]
pub struct PackedEcp5Clk {
    pub name: String,
    /// Net into the clock root (the IO pin net).
    pub in_net: String,
    /// Global clock net name.
    pub out_net: String,
}

/// A packed `DP16KD` block RAM.
#[derive(Clone, Debug)]
pub struct PackedEcp5Ram {
    pub name: String,
    /// INIT words from the IR `INIT` attribute (comma-separated hex, word
    /// 0 = address 0). Empty = zero-initialized. The bitstream worker maps
    /// these onto the `DP16KD` INIT bits and `DATA_WIDTH`/`WRITEMODE`
    /// config; the IR carries no pin-level memory interface in v1.
    pub init: Vec<u64>,
}

impl PackedEcp5Ram {
    /// Number of INIT words (= memory depth in words).
    pub fn depth(&self) -> usize {
        self.init.len()
    }
}

/// A packed `MULT18X18D` DSP block.
///
/// The IR's [`helion_ir::CellKind::Mac27`] is a coarse multiply marker:
/// `A`/`B`/`P` are bus-level net names and widths are unknown unless the
/// optional `WIDTH_A`/`WIDTH_B` cell attributes are present.
#[derive(Clone, Debug)]
pub struct PackedEcp5Dsp {
    pub name: String,
    pub a_net: String,
    pub b_net: String,
    pub c_net: Option<String>,
    pub p_net: String,
}

/// A packed ECP5 PLL.
///
/// Divider parameters are passed through verbatim from the cell
/// attributes (the Trellis `.config` words: `CLKI_DIV`, `CLKOP_DIV`,
/// `CLKOS_DIV`, ...). `CLKI_DIV` and `CLKOP_DIV` are required; the rest
/// is the bitstream worker's domain.
#[derive(Clone, Debug)]
pub struct PackedEcp5Pll {
    pub name: String,
    /// Reference clock net (`CLKI` pin).
    pub ref_net: String,
    /// Primary output net (`CLKOP` pin).
    pub out_net: String,
    /// All cell attributes, sorted by name (divider parameters).
    pub params: Vec<(String, String)>,
}

/// Backend-owned packed netlist for the ECP5 LFE5U-85F.
///
/// The place worker consumes this: `luts`/`ffs` go to `PLC2` slice
/// halves (`paired_ff`/`packed_lut` co-locate), `alu_chains` order the
/// ALU LUTs into dedicated carry columns, `ios` carry their PIO site
/// coordinates, and `rams`/`dsps`/`plls`/`clks` go to `EBR`/`DSP`/`PLL`
/// sites and the global clock network.
#[derive(Clone, Debug, Default)]
pub struct PackedEcp5 {
    /// Top module name, carried through the stages.
    pub top: String,
    /// Package the pin map was resolved against (design `PACKAGE`
    /// attribute, default `"CABGA381"`).
    pub package: String,
    pub luts: Vec<PackedEcp5Lut>,
    pub ffs: Vec<PackedEcp5Ff>,
    pub ios: Vec<PackedEcp5Io>,
    pub clks: Vec<PackedEcp5Clk>,
    pub rams: Vec<PackedEcp5Ram>,
    pub dsps: Vec<PackedEcp5Dsp>,
    pub plls: Vec<PackedEcp5Pll>,
    /// Carry chains, in the order their ALU LUTs reference via
    /// [`AluBit::chain`].
    pub alu_chains: Vec<PackedEcp5AluChain>,
}

/// Pack a [`NetlistIr`] into ECP5 primitives.
pub fn pack(netlist: &NetlistIr) -> Result<PackedEcp5, String> {
    let dev = Device::global()?;
    let package = netlist
        .attrs
        .get("PACKAGE")
        .unwrap_or("CABGA381")
        .to_string();
    let pkg_pins = dev.package_pins(&package).ok_or_else(|| {
        format!("ecp5 pack: unknown package '{package}': no pin data in the Trellis DB")
    })?;
    let mut bank_of: HashMap<(u32, String, u32), u32> = HashMap::new();
    for m in dev.pio_metadata() {
        bank_of.insert((m.col, m.pio.clone(), m.row), m.bank);
    }

    let pins = netlist.pin_index();
    let mut out = PackedEcp5 {
        top: resolve(netlist.name).to_string(),
        package: package.clone(),
        ..Default::default()
    };
    let mut consts = ConstDrivers::default();
    // Carry4 cells are collected first: chains reference each other via
    // CI/CO[3] regardless of cell order; emission happens after the loop.
    let mut carry_cells: Vec<Carry4Data> = Vec::new();

    for cell in &netlist.cells {
        match &cell.kind {
            CellKind::Lut6 { init } => {
                let mut inputs: [Option<String>; 6] = Default::default();
                for (i, slot) in inputs.iter_mut().enumerate() {
                    *slot = pins
                        .net_on(resolve(cell.name), &format!("I{i}"))
                        .map(|s| resolve(s).to_string());
                }
                let output_net = resolve(pins
                    .net_on(resolve(cell.name), "O")
                    .ok_or_else(|| format!("ecp5 pack: LUT {} has no O net", resolve(cell.name)))?)
                .to_string();
                let mut luts = decompose_lut6(resolve(cell.name), *init, &inputs)?;
                // Rename the top output net to the real one.
                if let Some(top) = luts.last_mut() {
                    top.output_net = output_net;
                }
                out.luts.append(&mut luts);
            }
            CellKind::Hff => {
                let need = |pin: &str| {
                    pins.net_on(resolve(cell.name), pin)
                        .map(|s| resolve(s).to_string())
                        .ok_or_else(|| format!("ecp5 pack: FF {} has no {pin} net", resolve(cell.name)))
                };
                out.ffs.push(PackedEcp5Ff {
                    name: resolve(cell.name).to_string(),
                    d_net: need("D")?,
                    q_net: need("Q")?,
                    clk_net: need("CLK")?,
                    packed_lut: None,
                });
            }
            CellKind::IobOut => {
                out.ios.push(pack_io(
                    netlist,
                    &pins,
                    resolve(cell.name),
                    IoDir::Out,
                    &package,
                    pkg_pins,
                    &bank_of,
                )?);
            }
            CellKind::IobIn => {
                out.ios.push(pack_io(
                    netlist,
                    &pins,
                    resolve(cell.name),
                    IoDir::In,
                    &package,
                    pkg_pins,
                    &bank_of,
                )?);
            }
            CellKind::ClockBuf => {
                let in_net = resolve(pins
                    .net_on(resolve(cell.name), "I")
                    .ok_or_else(|| format!("ecp5 pack: CLKBUF {} has no I net", resolve(cell.name)))?)
                .to_string();
                let out_net = resolve(pins
                    .net_on(resolve(cell.name), "O")
                    .ok_or_else(|| format!("ecp5 pack: CLKBUF {} has no O net", resolve(cell.name)))?)
                .to_string();
                out.clks.push(PackedEcp5Clk {
                    name: resolve(cell.name).to_string(),
                    in_net,
                    out_net,
                });
            }
            CellKind::Bram18 => {
                out.rams.push(pack_ram(resolve(cell.name), &cell.attrs)?);
            }
            CellKind::Mac27 | CellKind::Dsp48e1 { .. } => {
                out.dsps.push(pack_dsp(resolve(cell.name), &pins, &cell.attrs)?);
            }
            CellKind::Carry4 => {
                carry_cells.push(parse_carry4(resolve(cell.name), &pins, netlist)?);
            }
            CellKind::BlackBox { module }
                if resolve(*module).starts_with("PLLB") || resolve(*module).starts_with("EHXPLL") =>
            {
                out.plls.push(pack_pll(resolve(cell.name), resolve(*module), &pins, &cell.attrs)?);
            }
            CellKind::BlackBox { module } => {
                return Err(format!(
                    "ecp5 pack: unsupported black box '{}' (cell {}); \
                     only PLLB*/EHXPLL* black boxes map to ECP5 primitives",
                    resolve(*module),
                    resolve(cell.name)
                ));
            }
            CellKind::Ila { .. } => {
                return Err(format!(
                    "ecp5 pack: ILA {} unsupported on the ECP5 backend v1",
                    resolve(cell.name)
                ));
            }
        }
    }

    // Emit carry chains after all cells are parsed (chains link cells in
    // any order).
    emit_alu_chains(netlist, &carry_cells, &mut consts, &mut out)?;

    // Pair each FF with its driver LUT when the driver is a packed LUT4
    // with no other loads: they share one slice half (LUT -> FF in the
    // same half-slice). Pairing is recorded here; place honors it.
    pair_ffs(&mut out);

    // Fail loud on over-capacity instead of producing an unplaceable
    // netlist.
    check_capacity(dev, &package, &out)?;

    Ok(out)
}

/// Decompose a 6-LUT truth table into LUT4s via Shannon decomposition.
///
/// Returns the list of LUT4s (emitted in dependency order); the last
/// LUT's `output_net` is `{name}$O` and the caller renames it to the real
/// net. Uses at most 7 LUT4s; fewer when the function ignores inputs.
fn decompose_lut6(
    name: &str,
    init: u64,
    inputs: &[Option<String>; 6],
) -> Result<Vec<PackedEcp5Lut>, String> {
    // Collect the actually-used input positions.
    let used: Vec<usize> = (0..6).filter(|&i| inputs[i].is_some()).collect();
    if used.is_empty() {
        // Constant function: single LUT4 tied off.
        let out = (init & 1) != 0;
        return Ok(vec![PackedEcp5Lut {
            name: format!("{name}$const"),
            init: if out { 0xFFFF } else { 0x0000 },
            input_nets: [None, None, None, None],
            output_net: format!("{name}$O"),
            paired_ff: None,
            alu: None,
        }]);
    }
    if used.len() <= 4 {
        // Direct map onto one LUT4, remapping the truth table.
        let mut lut_init: u16 = 0;
        for n in 0..16u16 {
            // n's bits address used[0..k]; build the 6-bit address.
            let mut addr6: u64 = 0;
            for (k, &pos) in used.iter().enumerate() {
                if (n >> k) & 1 == 1 {
                    addr6 |= 1 << pos;
                }
            }
            if (init >> addr6) & 1 == 1 {
                lut_init |= 1 << n;
            }
        }
        let mut in_nets: [Option<String>; 4] = [None, None, None, None];
        for (k, &pos) in used.iter().enumerate() {
            in_nets[k] = inputs[pos].clone();
        }
        return Ok(vec![PackedEcp5Lut {
            name: format!("{name}$lut"),
            init: lut_init,
            input_nets: in_nets,
            output_net: format!("{name}$O"),
            paired_ff: None,
            alu: None,
        }]);
    }
    // Shannon decomposition on the highest used input.
    // f = sel ? f1 : f0, each cofactor has one fewer input.
    let sel_pos = used[used.len() - 1];
    let sel_net = inputs[sel_pos].clone().ok_or_else(|| {
        format!("ecp5 pack: LUT {name} input I{sel_pos} vanished during decomposition")
    })?;
    // Cofactor truth tables, with the sel bit compressed out: for each
    // minterm where f = 1, drop bit sel_pos to get the 5-bit cofactor
    // address, and set it in c0 (sel = 0) or c1 (sel = 1).
    let mut c0: u64 = 0;
    let mut c1: u64 = 0;
    for a in 0..64u64 {
        if (init >> a) & 1 == 0 {
            continue;
        }
        let sel = (a >> sel_pos) & 1;
        let lo = a & ((1 << sel_pos) - 1);
        let hi = a >> (sel_pos + 1);
        let ca = lo | (hi << sel_pos);
        if sel == 0 {
            c0 |= 1 << ca;
        } else {
            c1 |= 1 << ca;
        }
    }
    let mut in0 = inputs.clone();
    let mut in1 = inputs.clone();
    in0[sel_pos] = None;
    in1[sel_pos] = None;
    let mut luts = decompose_lut6(&format!("{name}$f0"), c0, &in0)?;
    let out0 = luts
        .last()
        .ok_or_else(|| format!("ecp5 pack: decomposition of {name}$f0 produced no LUTs"))?
        .output_net
        .clone();
    let mut luts1 = decompose_lut6(&format!("{name}$f1"), c1, &in1)?;
    let out1 = luts1
        .last()
        .ok_or_else(|| format!("ecp5 pack: decomposition of {name}$f1 produced no LUTs"))?
        .output_net
        .clone();
    luts.append(&mut luts1);
    // Mux: sel ? out1 : out0. LUT4(sel, out0, out1) = (sel & out1) | (!sel & out0).
    // init bit n (n = C*4 + B*2 + A): A=sel, B=out0, C=out1.
    let mut mux_init: u16 = 0;
    for n in 0..16u16 {
        let sel = n & 1;
        let a = (n >> 1) & 1;
        let b = (n >> 2) & 1;
        if (if sel == 1 { b } else { a }) == 1 {
            mux_init |= 1 << n;
        }
    }
    luts.push(PackedEcp5Lut {
        name: format!("{name}$mux"),
        init: mux_init,
        input_nets: [Some(sel_net), Some(out0), Some(out1), None],
        output_net: format!("{name}$O"),
        paired_ff: None,
        alu: None,
    });
    Ok(luts)
}

/// Pack an `IobIn`/`IobOut` cell into a PIO site assignment.
#[allow(clippy::too_many_arguments)]
fn pack_io(
    netlist: &Design,
    pins: &PinIndex,
    cell_name: &str,
    dir: IoDir,
    package: &str,
    pkg_pins: &HashMap<String, PackagePin>,
    bank_of: &HashMap<(u32, String, u32), u32>,
) -> Result<PackedEcp5Io, String> {
    let (fab_pin, want_dir) = match dir {
        IoDir::Out => ("I", PortDir::Out),
        IoDir::In => ("O", PortDir::In),
    };
    let kind = match dir {
        IoDir::Out => "OBUF",
        IoDir::In => "IBUF",
    };
    let net = resolve(pins
        .net_on(cell_name, fab_pin)
        .ok_or_else(|| format!("ecp5 pack: {kind} {cell_name} has no {fab_pin} net"))?)
    .to_string();
    let port = resolve(pins
        .net_on(cell_name, "PAD")
        .unwrap_or(intern(cell_name)))
    .to_string();
    let pdef = netlist
        .ports
        .iter()
        .find(|p| p.name == intern(&port))
        .ok_or_else(|| {
            format!("ecp5 pack: {kind} {cell_name}: port '{port}' is not a top-level port")
        })?;
    if pdef.dir != want_dir {
        return Err(format!(
            "ecp5 pack: {kind} {cell_name}: port '{port}' is {:?}, not {want_dir:?}",
            pdef.dir
        ));
    }
    let loc = pdef.attrs.get("LOC").ok_or_else(|| {
        format!(
            "ecp5 pack: no package pin for port '{port}': add a LOC attribute \
             naming a {package} pin (e.g. set_port_attr(\"{port}\", \"LOC\", \"A10\"))"
        )
    })?;
    let pp = pkg_pins.get(loc).ok_or_else(|| {
        format!("ecp5 pack: port '{port}' LOC '{loc}' is not a {package} pin in the Trellis DB")
    })?;
    let bank = bank_of
        .get(&(pp.col, pp.pio.clone(), pp.row))
        .copied()
        .ok_or_else(|| format!("ecp5 pack: pin '{loc}' has no pio_metadata entry (DB inconsistency)"))?;
    Ok(PackedEcp5Io {
        name: cell_name.to_string(),
        port,
        dir,
        net,
        pin: loc.to_string(),
        bank,
        pio_col: pp.col,
        pio_row: pp.row,
        pio: pp.pio.clone(),
    })
}

/// Pack a `Bram18` cell into `DP16KD` shape.
fn pack_ram(name: &str, attrs: &Attrs) -> Result<PackedEcp5Ram, String> {
    let mut init = Vec::new();
    if let Some(s) = attrs.get("INIT") {
        for tok in s.split(',') {
            let t = tok.trim();
            if t.is_empty() {
                continue;
            }
            let w = u64::from_str_radix(t, 16)
                .map_err(|_| format!("ecp5 pack: RAM {name}: INIT word '{t}' is not hex"))?;
            init.push(w);
        }
    }
    if init.len() * 64 > EBR_BITS {
        return Err(format!(
            "ecp5 pack: RAM {name}: INIT is {} bits, exceeding one DP16KD (18Kb); \
             split the memory across multiple blocks",
            init.len() * 64
        ));
    }
    Ok(PackedEcp5Ram {
        name: name.to_string(),
        init,
    })
}

/// Pack a `Mac27` cell into `MULT18X18D` shape.
fn pack_dsp(name: &str, pins: &PinIndex, attrs: &Attrs) -> Result<PackedEcp5Dsp, String> {
    for (attr, what) in [("WIDTH_A", "A"), ("WIDTH_B", "B")] {
        if let Some(v) = attrs.get(attr) {
            let w: u32 = v.trim().parse().map_err(|_| {
                format!("ecp5 pack: DSP {name}: {attr}='{v}' is not an integer width")
            })?;
            if w > DSP_MAX_WIDTH {
                return Err(format!(
                    "ecp5 pack: DSP {name}: {what} width {w} exceeds one MULT18X18D \
                     (18 bits); cascading multipliers is future work"
                ));
            }
        }
    }
    let need = |pin: &str| {
        pins.net_on(name, pin)
            .map(|s| resolve(s).to_string())
            .ok_or_else(|| format!("ecp5 pack: DSP {name} has no {pin} net"))
    };
    Ok(PackedEcp5Dsp {
        name: name.to_string(),
        a_net: need("A")?,
        b_net: need("B")?,
        c_net: pins.net_on(name, "C").map(|s| resolve(s).to_string()),
        p_net: need("P")?,
    })
}

/// Pack a `PLLB*`/`EHXPLL*` black box into a PLL with divider parameters
/// passed through from the cell attributes.
fn pack_pll(
    name: &str,
    module: &str,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedEcp5Pll, String> {
    let ref_net = resolve(pins
        .net_on(name, "CLKI")
        .or_else(|| pins.net_on(name, "REFERENCECLK"))
        .or_else(|| pins.net_on(name, "I"))
        .ok_or_else(|| format!("ecp5 pack: PLL {name}: no CLKI net"))?)
    .to_string();
    let out_net = resolve(pins
        .net_on(name, "CLKOP")
        .or_else(|| pins.net_on(name, "PLLOUTGLOBAL"))
        .or_else(|| pins.net_on(name, "O"))
        .ok_or_else(|| format!("ecp5 pack: PLL {name}: no CLKOP net"))?)
    .to_string();
    for req in ["CLKI_DIV", "CLKOP_DIV"] {
        if attrs.get(req).is_none() {
            return Err(format!(
                "ecp5 pack: PLL {name} ({module}): missing {req} attribute; \
                 ECP5 PLLs need divider parameters (CLKI_DIV, CLKOP_DIV, ...)"
            ));
        }
    }
    let mut params: Vec<(String, String)> = attrs
        .map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    params.sort();
    Ok(PackedEcp5Pll {
        name: name.to_string(),
        ref_net,
        out_net,
        params,
    })
}

/// Carry input source for a Carry4 cell.
#[derive(Clone, Debug)]
enum CarryIn {
    Zero,
    One,
    /// Chained from the named Carry4 cell's CO[3].
    Prev(String),
}

/// A parsed Carry4 cell; per-bit nets are `None` when unconnected
/// (treated as constant 0).
struct Carry4Data {
    name: Spur,
    cin: CarryIn,
    s: [Option<String>; 4],
    di: [Option<String>; 4],
    o: [Option<String>; 4],
    co: [Option<String>; 4],
}

fn is_const1(net: Option<Spur>) -> bool {
    matches!(net, Some(s) if {
        let r = resolve(s);
        r == "1" || r == "vcc" || r == "VCC" || r == "$true"
    })
}

fn is_const0(net: Option<Spur>) -> bool {
    match net {
        None => true,
        Some(s) => {
            let r = resolve(s);
            r == "0" || r == "gnd" || r == "GND" || r == "$false"
        }
    }
}

/// Find the Carry4 cell driving `net` via its CO[3] pin.
fn carry_co3_driver(netlist: &Design, net: helion_ir::Spur) -> Option<String> {
    let n = netlist.nets.iter().find(|n| n.name == net)?;
    for e in &n.endpoints {
        if e.pin != intern("CO3") {
            continue;
        }
        if let Some(cell) = netlist.cells.iter().find(|c| c.name == e.cell) {
            if matches!(cell.kind, CellKind::Carry4) {
                return Some(resolve(cell.name).to_string());
            }
        }
    }
    None
}

/// Lazily-created constant drivers shared by carry logic.
#[derive(Default)]
struct ConstDrivers {
    zero: Option<String>,
    one: Option<String>,
}

impl ConstDrivers {
    fn net(&mut self, one: bool, out: &mut PackedEcp5) -> String {
        let slot = if one { &mut self.one } else { &mut self.zero };
        if let Some(n) = slot {
            return n.clone();
        }
        let n = if one { "$const$1" } else { "$const$0" }.to_string();
        out.luts.push(PackedEcp5Lut {
            name: n.clone(),
            init: if one { 0xFFFF } else { 0x0000 },
            input_nets: [None, None, None, None],
            output_net: n.clone(),
            paired_ff: None,
            alu: None,
        });
        *slot = Some(n.clone());
        n
    }
}

/// Parse and validate one Carry4 cell.
///
/// Pin convention follows Xilinx CARRY4 (the IR's backend-neutral marker):
/// inputs `CI`, `CYINIT`, `DI0`..`DI3`, `S0`..`S3`; outputs `O0`..`O3`
/// (sums), `CO0`..`CO3` (carry outs).
fn parse_carry4(name: &str, pins: &PinIndex, netlist: &Design) -> Result<Carry4Data, String> {
    let cyinit = pins.net_on(name, "CYINIT");
    let ci = pins.net_on(name, "CI");
    // The CCU2 carry_in supports only constant 0/1 or the dedicated
    // carry from the slice below; a general net on CYINIT/CI is
    // unrepresentable.
    let cin = if is_const1(cyinit) {
        if !is_const0(ci) {
            return Err(format!(
                "ecp5 pack: Carry4 {name}: CYINIT=1 with a driven CI is ambiguous"
            ));
        }
        CarryIn::One
    } else if is_const1(ci) {
        if !is_const0(cyinit) {
            return Err(format!(
                "ecp5 pack: Carry4 {name}: CI=1 with a driven CYINIT is ambiguous"
            ));
        }
        CarryIn::One
    } else if let Some(ci_net) = ci {
        if !is_const0(cyinit) {
            return Err(format!(
                "ecp5 pack: Carry4 {name}: chained CI with a driven CYINIT is unsupported"
            ));
        }
        let prev = carry_co3_driver(netlist, ci_net).ok_or_else(|| {
            format!(
                "ecp5 pack: Carry4 {name}: CI net '{}' is not driven by a Carry4 CO[3]; \
                 the CCU2 carry_in supports only constant 0/1 or a chained carry",
                resolve(ci_net)
            )
        })?;
        if prev == name {
            return Err(format!("ecp5 pack: Carry4 {name}: CI chained to itself"));
        }
        CarryIn::Prev(prev)
    } else if !is_const0(cyinit) {
        return Err(format!(
            "ecp5 pack: Carry4 {name}: CYINIT must be constant 0/1 or unconnected"
        ));
    } else {
        CarryIn::Zero
    };

    let mut s: [Option<String>; 4] = Default::default();
    let mut di: [Option<String>; 4] = Default::default();
    let mut o: [Option<String>; 4] = Default::default();
    let mut co: [Option<String>; 4] = Default::default();
    for i in 0..4 {
        s[i] = pins.net_on(name, &format!("S{i}")).map(|n| resolve(n).to_string());
        di[i] = pins.net_on(name, &format!("DI{i}")).map(|n| resolve(n).to_string());
        o[i] = pins.net_on(name, &format!("O{i}")).map(|n| resolve(n).to_string());
        co[i] = pins.net_on(name, &format!("CO{i}")).map(|n| resolve(n).to_string());
    }
    // CO[0..2] are internal to the dedicated chain: the CCU2 carry is not
    // routable to general fabric, so observing them is unrepresentable.
    for (i, co_i) in co.iter().enumerate().take(3) {
        if co_i.is_some() {
            return Err(format!(
                "ecp5 pack: Carry4 {name}: CO{i} is internal to the carry chain and \
                 cannot be observed (the CCU2 carry is not routable to fabric)"
            ));
        }
    }
    Ok(Carry4Data {
        name: intern(name),
        cin,
        s,
        di,
        o,
        co,
    })
}

/// Order Carry4 cells into chains, validate CO[3] usage, and emit one ALU
/// LUT per bit plus [`PackedEcp5AluChain`] records.
///
/// Each bit's ALU LUT computes the sum `O[i] = S[i] ^ carry[i]`
/// (`init = 0x6666` over `A = S[i]`, `B = carry[i]`); the dedicated CCU2
/// carry computes `carry[i+1] = S[i] ? carry[i] : DI[i]`, with `DI[i]`
/// carried on [`AluBit::di_net`]. Xilinx MUXCY semantics are preserved
/// exactly, so an adder built from `S = a^b`, `DI = a` keeps its truth
/// table bit-for-bit.
fn emit_alu_chains(
    netlist: &Design,
    cells: &[Carry4Data],
    consts: &mut ConstDrivers,
    out: &mut PackedEcp5,
) -> Result<(), String> {
    let idx: HashMap<&str, usize> = cells
        .iter()
        .enumerate()
        .map(|(i, c)| (resolve(c.name), i))
        .collect();
    // next[i] = cell chained after i via CO[3] -> CI.
    let mut next: Vec<Option<usize>> = vec![None; cells.len()];
    for (i, cell) in cells.iter().enumerate() {
        if let CarryIn::Prev(prev_name) = &cell.cin {
            let &pi = idx.get(prev_name.as_str()).ok_or_else(|| {
                format!(
                    "ecp5 pack: Carry4 {}: previous cell '{prev_name}' not found",
                    resolve(cell.name)
                )
            })?;
            if next[pi].is_some() {
                return Err(format!(
                    "ecp5 pack: Carry4 {}: CO[3] fans out to multiple carry chains; \
                     the dedicated carry wire is point-to-point",
                    resolve(cells[pi].name)
                ));
            }
            next[pi] = Some(i);
        }
    }
    // CO[3] may only drive the chained Carry4's CI (or be unconnected):
    // the chain-top carry is not routable to general fabric in v1.
    for (i, cell) in cells.iter().enumerate() {
        if let Some(co3) = &cell.co[3] {
            let net = netlist
                .nets
                .iter()
                .find(|n| n.name == intern(co3))
                .ok_or_else(|| format!("ecp5 pack: Carry4 {}: CO3 net vanished", resolve(cell.name)))?;
            let allowed_ci = next[i].map(|n| resolve(cells[n].name));
            for e in &net.endpoints {
                if e.cell == cell.name && e.pin == intern("CO3") {
                    continue;
                }
                if e.pin == intern("CI") && Some(resolve(e.cell)) == allowed_ci {
                    continue;
                }
                return Err(format!(
                    "ecp5 pack: Carry4 {}: CO[3] net '{co3}' drives {}.{} which is not the \
                     chained carry; the chain-top carry is not routable to fabric in v1",
                    resolve(cell.name), resolve(e.cell), resolve(e.pin)
                ));
            }
        }
    }
    // Order into chains from heads (Zero/One). A cell with no head
    // reachable is a cycle.
    let mut in_chain = vec![false; cells.len()];
    let mut chains: Vec<Vec<usize>> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        if !matches!(cell.cin, CarryIn::Zero | CarryIn::One) {
            continue;
        }
        let mut chain = vec![i];
        in_chain[i] = true;
        let mut cur = i;
        while let Some(nxt) = next[cur] {
            if in_chain[nxt] {
                return Err(format!(
                    "ecp5 pack: Carry4 {}: carry chain cycle detected",
                    resolve(cells[nxt].name)
                ));
            }
            chain.push(nxt);
            in_chain[nxt] = true;
            cur = nxt;
        }
        chains.push(chain);
    }
    for (i, cell) in cells.iter().enumerate() {
        if !in_chain[i] {
            return Err(format!(
                "ecp5 pack: Carry4 {}: carry chain has no head (cycle or dangling CI)",
                resolve(cell.name)
            ));
        }
    }

    for chain in &chains {
        let head = &cells[chain[0]];
        let cin_one = matches!(head.cin, CarryIn::One);
        let chain_idx = out.alu_chains.len();
        let mut bit = 0usize;
        for &ci in chain {
            let cell = &cells[ci];
            for b in 0..4 {
                let s_net = match cell.s[b].as_deref().map(|s| intern(s)) {
                    Some(n) if is_const1(Some(n)) => consts.net(true, out),
                    Some(n) if is_const0(Some(n)) => consts.net(false, out),
                    Some(n) => resolve(n).to_string(),
                    None => consts.net(false, out),
                };
                let carry_net = if bit == 0 {
                    if cin_one {
                        format!("{CARRY_NET_PREFIX}one")
                    } else {
                        format!("{CARRY_NET_PREFIX}zero")
                    }
                } else {
                    format!("{CARRY_NET_PREFIX}c{chain_idx}b{}", bit - 1)
                };
                // Sum LUT: O[i] = S[i] ^ carry[i]; the B input is a
                // dedicated-carry placeholder (see CARRY_NET_PREFIX), the
                // router skips it and the bitstream worker programs the
                // CCU2 carry path directly.
                let o_net = cell.o[b]
                    .clone()
                    .unwrap_or_else(|| format!("{}${b}$o", resolve(cell.name)));
                out.luts.push(PackedEcp5Lut {
                    name: format!("{}${b}$alu", resolve(cell.name)),
                    init: 0x6666, // A ^ B
                    input_nets: [Some(s_net), Some(carry_net), None, None],
                    output_net: o_net,
                    paired_ff: None,
                    alu: Some(AluBit {
                        chain: chain_idx,
                        bit,
                        di_net: cell.di[b].clone(),
                    }),
                });
                bit += 1;
            }
        }
        out.alu_chains.push(PackedEcp5AluChain {
            cin_one,
            bits: chain.len() * 4,
        });
    }
    Ok(())
}

/// Pair each FF with its driver LUT when the driver is a single packed
/// LUT4: they share one slice half.
fn pair_ffs(out: &mut PackedEcp5) {
    let mut driver: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, lut) in out.luts.iter().enumerate() {
        driver.entry(lut.output_net.as_str()).or_default().push(i);
    }
    // Collect pairs first: `driver` borrows `out.luts`, so the mutation
    // happens after the borrow ends.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (fi, ff) in out.ffs.iter().enumerate() {
        let single = match driver.get(ff.d_net.as_str()) {
            Some(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        };
        if let Some(li) = single {
            if !pairs.iter().any(|(taken, _)| *taken == li) {
                pairs.push((li, fi));
            }
        }
    }
    for (li, fi) in pairs {
        out.luts[li].paired_ff = Some(fi);
        out.ffs[fi].packed_lut = Some(li);
    }
}

/// Fail loud when the packed netlist does not fit the LFE5U-85F.
fn check_capacity(dev: &Device, package: &str, out: &PackedEcp5) -> Result<(), String> {
    let c = dev.counts();
    // Unique MULT18_* bel positions: one MULT18X18D per position.
    let mut mult18: HashSet<(String, i32, i32)> = HashSet::new();
    for t in dev.tiles() {
        for s in &t.sites {
            if s.name.starts_with("MULT18_") {
                mult18.insert((s.name.clone(), s.bel_col, s.bel_row));
            }
        }
    }
    let pios = dev
        .package_pins(package)
        .map(|m| m.len())
        .unwrap_or(0);
    let checks = [
        (out.luts.len(), c.lut4, "LUT4"),
        (out.ffs.len(), c.ff, "flip-flop"),
        (out.rams.len(), c.ebr_bels, "DP16KD EBR"),
        (out.dsps.len(), mult18.len(), "MULT18X18D DSP"),
        (out.plls.len(), c.pll_bels, "PLL"),
        (out.ios.len(), pios, "PIO"),
    ];
    for (used, cap, what) in checks {
        if used > cap {
            return Err(format!(
                "ecp5 pack: over capacity: {used} {what} used but the LFE5U-85F \
                 ({package}) has only {cap}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate packed LUTs in software. `inputs` maps net names to
    /// values; LUTs must be in dependency order. Nets starting with
    /// `$carry$` are resolved through `carry_vals`.
    fn eval_packed(
        luts: &[PackedEcp5Lut],
        inputs: &HashMap<String, bool>,
        carry_vals: &HashMap<String, bool>,
        out_net: &str,
    ) -> bool {
        let mut vals: HashMap<String, bool> = inputs.clone();
        for lut in luts {
            let mut n = 0u16;
            for (k, net) in lut.input_nets.iter().enumerate() {
                let v = match net {
                    Some(net) if net.starts_with(CARRY_NET_PREFIX) => {
                        *carry_vals.get(net).unwrap_or(&false)
                    }
                    Some(net) => *vals.get(net).unwrap_or(&false),
                    None => false,
                };
                if v {
                    n |= 1 << k;
                }
            }
            vals.insert(lut.output_net.clone(), (lut.init >> n) & 1 == 1);
        }
        vals[out_net]
    }

    fn lut6_cell(d: &mut Design, name: &str, init: u64, inputs: &[&str]) {
        d.add_cell(name, CellKind::Lut6 { init });
        for (i, net) in inputs.iter().enumerate() {
            d.connect(*net, name, format!("I{i}"));
        }
        d.connect(format!("{name}$o"), name, "O");
    }

    #[test]
    fn lut6_xor6_decomposes_correctly() {
        let mut init: u64 = 0;
        for a in 0..64u64 {
            if a.count_ones() % 2 == 1 {
                init |= 1 << a;
            }
        }
        let inputs: [Option<String>; 6] = [
            Some("i0".into()),
            Some("i1".into()),
            Some("i2".into()),
            Some("i3".into()),
            Some("i4".into()),
            Some("i5".into()),
        ];
        let luts = decompose_lut6("x", init, &inputs).unwrap();
        assert!(luts.len() <= 7, "Shannon needs at most 7 LUT4s");
        let carry_vals = HashMap::new();
        for a in 0..64u64 {
            let vals: HashMap<String, bool> = (0..6)
                .map(|i| (format!("i{i}"), (a >> i) & 1 == 1))
                .collect();
            let got = eval_packed(&luts, &vals, &carry_vals, "x$O");
            let want = a.count_ones() % 2 == 1;
            assert_eq!(got, want, "mismatch at input {a:06b}");
        }
    }

    #[test]
    fn lut6_decomposition_exhaustive_spot_functions() {
        // (name, init): AND6, OR6, a "random" function, 5-input AND,
        // 4-input passthrough, constant 1.
        let fns: &[(&str, u64)] = &[
            ("and6", 0x8000_0000_0000_0000),
            ("or6", 0xFFFF_FFFF_FFFF_FFFE),
            ("rand", 0x1234_5678_9ABC_DEF0),
            ("and5", 0x8000_0000_0000_0000 & 0xFFFF_FFFF),
            ("and4", 0x8000),
            ("const1", 0xFFFF_FFFF_FFFF_FFFF),
        ];
        for (fname, init) in fns {
            let inputs: [Option<String>; 6] = [
                Some("i0".into()),
                Some("i1".into()),
                Some("i2".into()),
                Some("i3".into()),
                Some("i4".into()),
                Some("i5".into()),
            ];
            let luts = decompose_lut6(fname, *init, &inputs).unwrap();
            let carry_vals = HashMap::new();
            for a in 0..64u64 {
                let vals: HashMap<String, bool> = (0..6)
                    .map(|i| (format!("i{i}"), (a >> i) & 1 == 1))
                    .collect();
                let got = eval_packed(&luts, &vals, &carry_vals, &format!("{fname}$O"));
                let want = (init >> a) & 1 == 1;
                assert_eq!(got, want, "{fname}: mismatch at input {a:06b}");
            }
        }
        // 4-input AND maps to exactly one LUT4.
        let inputs4: [Option<String>; 6] = [
            Some("i0".into()),
            Some("i1".into()),
            Some("i2".into()),
            Some("i3".into()),
            None,
            None,
        ];
        let luts = decompose_lut6("and4b", 0x8000, &inputs4).unwrap();
        assert_eq!(luts.len(), 1);
        assert_eq!(luts[0].init, 0x8000);
        // Constant folds to one tied LUT4.
        let inputs0: [Option<String>; 6] = [None, None, None, None, None, None];
        let luts = decompose_lut6("c0", 0x0, &inputs0).unwrap();
        assert_eq!(luts.len(), 1);
        assert_eq!(luts[0].init, 0x0000);
    }

    #[test]
    fn ff_pairs_with_driver_lut() {
        let mut d = Design::new("t");
        // g = a & b (init 0x8 over I0,I1).
        lut6_cell(&mut d, "g", 0x8, &["a", "b"]);
        d.add_cell("f", CellKind::Hff);
        d.connect("g$o", "f", "D");
        d.connect("q", "f", "Q");
        d.connect("clk", "f", "CLK");
        // f2's D is undriven by any LUT: no pairing.
        d.add_cell("f2", CellKind::Hff);
        d.connect("din", "f2", "D");
        d.connect("q2", "f2", "Q");
        d.connect("clk", "f2", "CLK");
        let p = pack(&d).unwrap();
        assert_eq!(p.luts.len(), 1);
        assert_eq!(p.ffs.len(), 2);
        assert_eq!(p.luts[0].paired_ff, Some(0));
        assert_eq!(p.ffs[0].packed_lut, Some(0));
        assert_eq!(p.ffs[1].packed_lut, None);
    }

    #[test]
    fn ff_pairing_is_exclusive() {
        let mut d = Design::new("t");
        lut6_cell(&mut d, "g", 0x8, &["a", "b"]);
        for (i, f) in ["f1", "f2"].iter().enumerate() {
            d.add_cell(*f, CellKind::Hff);
            d.connect("g$o", *f, "D");
            d.connect(format!("q{i}"), *f, "Q");
            d.connect("clk", *f, "CLK");
        }
        let p = pack(&d).unwrap();
        // Only the first FF pairs; the second shares the net but gets no
        // slice half of its own from this LUT.
        assert_eq!(p.luts[0].paired_ff, Some(0));
        assert_eq!(p.ffs[0].packed_lut, Some(0));
        assert_eq!(p.ffs[1].packed_lut, None);
    }

    /// Build a 4-bit adder: S[i] = a[i]^b[i] LUTs + one Carry4 with
    /// DI[i] = a[i].
    fn adder4(d: &mut Design, cin_one: bool) {
        for i in 0..4 {
            lut6_cell(d, &format!("s{i}"), 0x6666, &[&format!("a{i}"), &format!("b{i}")]);
        }
        d.add_cell("c0", CellKind::Carry4);
        for i in 0..4 {
            d.connect(format!("s{i}$o"), "c0", format!("S{i}"));
            d.connect(format!("a{i}"), "c0", format!("DI{i}"));
            d.connect(format!("sum{i}"), "c0", format!("O{i}"));
        }
        if cin_one {
            d.connect("1", "c0", "CYINIT");
        }
        // One FF captures sum[0]: it should pair with the ALU sum LUT.
        d.add_cell("f", CellKind::Hff);
        d.connect("sum0", "f", "D");
        d.connect("sq0", "f", "Q");
        d.connect("clk", "f", "CLK");
    }

    #[test]
    fn alu_chain_packs_and_is_correct() {
        let mut d = Design::new("adder");
        adder4(&mut d, false);
        let p = pack(&d).unwrap();
        assert_eq!(p.alu_chains.len(), 1);
        assert_eq!(p.alu_chains[0].bits, 4);
        assert!(!p.alu_chains[0].cin_one);
        let alu_luts: Vec<&PackedEcp5Lut> =
            p.luts.iter().filter(|l| l.alu.is_some()).collect();
        assert_eq!(alu_luts.len(), 4);
        for (i, lut) in alu_luts.iter().enumerate() {
            let ab = lut.alu.as_ref().unwrap();
            assert_eq!((ab.chain, ab.bit), (0, i));
            assert_eq!(lut.init, 0x6666);
            assert_eq!(ab.di_net.as_deref(), Some(format!("a{i}").as_str()));
        }
        // The FF on sum0 pairs with the bit-0 ALU sum LUT.
        assert_eq!(p.ffs.len(), 1);
        let li = p.ffs[0].packed_lut.expect("FF should pair with ALU sum LUT");
        assert!(p.luts[li].alu.is_some());
        assert_eq!(p.luts[li].paired_ff, Some(0));

        // Exhaustive functional check: ripple the expected sums/carry in
        // software and compare against evaluating the packed netlist.
        for a in 0..16u32 {
            for b in 0..16u32 {
                let mut vals: HashMap<String, bool> = HashMap::new();
                for i in 0..4 {
                    vals.insert(format!("a{i}"), (a >> i) & 1 == 1);
                    vals.insert(format!("b{i}"), (b >> i) & 1 == 1);
                }
                // Expected ripple.
                let mut carry = false;
                let mut want_sum = 0u32;
                for i in 0..4 {
                    let ai = (a >> i) & 1 == 1;
                    let bi = (b >> i) & 1 == 1;
                    let si = ai ^ bi;
                    want_sum |= u32::from(si ^ carry) << i;
                    carry = if si { carry } else { ai };
                }
                // Walk bits, threading the carry explicitly.
                let mut c = false;
                let mut got_sum = 0u32;
                for i in 0..4 {
                    let mut cv: HashMap<String, bool> = HashMap::new();
                    cv.insert(
                        if i == 0 {
                            format!("{CARRY_NET_PREFIX}zero")
                        } else {
                            format!("{CARRY_NET_PREFIX}c0b{}", i - 1)
                        },
                        c,
                    );
                    let bit_luts: Vec<PackedEcp5Lut> = p
                        .luts
                        .iter()
                        .filter(|l| {
                            l.name == format!("s{i}$lut")
                                || l.alu.as_ref().is_some_and(|ab| ab.bit == i)
                        })
                        .cloned()
                        .collect();
                    assert_eq!(bit_luts.len(), 2, "bit {i}: want S LUT + ALU LUT");
                    let s = eval_packed(&bit_luts, &vals, &cv, &format!("s{i}$o"));
                    let sum = eval_packed(&bit_luts, &vals, &cv, &format!("sum{i}"));
                    got_sum |= u32::from(sum) << i;
                    let ai = (a >> i) & 1 == 1;
                    assert_eq!(s, ai ^ ((b >> i) & 1 == 1), "S{i} wrong");
                    c = if s { c } else { ai };
                }
                assert_eq!(got_sum, want_sum, "a={a} b={b}");
                assert_eq!(c, carry, "final carry a={a} b={b}");
            }
        }
    }

    #[test]
    fn alu_chain_cin_one_for_subtract() {
        let mut d = Design::new("sub");
        adder4(&mut d, true);
        let p = pack(&d).unwrap();
        assert_eq!(p.alu_chains.len(), 1);
        assert!(p.alu_chains[0].cin_one);
        // Bit 0's carry input is the dedicated constant-1 net.
        let bit0 = p
            .luts
            .iter()
            .find(|l| l.alu.as_ref().is_some_and(|ab| ab.bit == 0))
            .unwrap();
        assert_eq!(
            bit0.input_nets[1].as_deref(),
            Some(format!("{CARRY_NET_PREFIX}one").as_str())
        );
    }

    #[test]
    fn alu_chain_two_carry4s_link() {
        let mut d = Design::new("adder8");
        for i in 0..8 {
            lut6_cell(&mut d, &format!("s{i}"), 0x6666, &[&format!("a{i}"), &format!("b{i}")]);
        }
        for (idx, nm) in ["c0", "c1"].iter().enumerate() {
            d.add_cell(*nm, CellKind::Carry4);
            for b in 0..4 {
                let i = idx * 4 + b;
                d.connect(format!("s{i}$o"), *nm, format!("S{b}"));
                d.connect(format!("a{i}"), *nm, format!("DI{b}"));
                d.connect(format!("sum{i}"), *nm, format!("O{b}"));
            }
        }
        // c1.CI <- c0.CO3.
        d.connect("cout0", "c0", "CO3");
        d.connect("cout0", "c1", "CI");
        let p = pack(&d).unwrap();
        assert_eq!(p.alu_chains.len(), 1);
        assert_eq!(p.alu_chains[0].bits, 8);
        for i in 0..8 {
            let lut = p
                .luts
                .iter()
                .find(|l| l.alu.as_ref().is_some_and(|ab| ab.bit == i))
                .unwrap();
            assert_eq!(lut.alu.as_ref().unwrap().chain, 0);
        }
        // Bit 4's carry comes from bit 3 of the same chain.
        let bit4 = p
            .luts
            .iter()
            .find(|l| l.alu.as_ref().is_some_and(|ab| ab.bit == 4))
            .unwrap();
        assert_eq!(
            bit4.input_nets[1].as_deref(),
            Some(format!("{CARRY_NET_PREFIX}c0b3").as_str())
        );
    }

    #[test]
    fn carry4_co0_observed_is_loud_error() {
        let mut d = Design::new("bad");
        d.add_cell("c0", CellKind::Carry4);
        d.connect("s0", "c0", "S0");
        d.connect("co0x", "c0", "CO0");
        let e = pack(&d).unwrap_err();
        assert!(e.contains("CO0"), "unexpected error: {e}");
    }

    #[test]
    fn rom_packs_to_dp16kd_with_init() {
        let mut d = Design::new("r");
        d.add_cell("rom", CellKind::Bram18);
        d.set_cell_attr("rom", "INIT", "1,2,ff,10").unwrap();
        let p = pack(&d).unwrap();
        assert_eq!(p.rams.len(), 1);
        assert_eq!(p.rams[0].name, "rom");
        assert_eq!(p.rams[0].init, vec![1u64, 2, 0xff, 0x10]);
        assert_eq!(p.rams[0].depth(), 4);
    }

    #[test]
    fn ram_init_too_big_is_loud_error() {
        let mut d = Design::new("r");
        d.add_cell("big", CellKind::Bram18);
        // 289 words x 64 bits = 18496 bits > 18432 (DP16KD).
        let init = vec!["1"; 289].join(",");
        d.set_cell_attr("big", "INIT", init).unwrap();
        let e = pack(&d).unwrap_err();
        assert!(e.contains("DP16KD"), "unexpected error: {e}");
    }

    #[test]
    fn ram_bad_init_hex_is_loud_error() {
        let mut d = Design::new("r");
        d.add_cell("bad", CellKind::Bram18);
        d.set_cell_attr("bad", "INIT", "zz").unwrap();
        assert!(pack(&d).is_err());
    }

    #[test]
    fn multiplier_packs_to_mult18x18d() {
        let mut d = Design::new("m");
        d.add_cell("u_mac0", CellKind::Mac27);
        d.connect("abus", "u_mac0", "A");
        d.connect("bbus", "u_mac0", "B");
        d.connect("pbus", "u_mac0", "P");
        let p = pack(&d).unwrap();
        assert_eq!(p.dsps.len(), 1);
        let dsp = &p.dsps[0];
        assert_eq!(dsp.a_net, "abus");
        assert_eq!(dsp.b_net, "bbus");
        assert_eq!(dsp.c_net, None);
        assert_eq!(dsp.p_net, "pbus");
    }

    #[test]
    fn multiplier_too_wide_is_loud_error() {
        let mut d = Design::new("m");
        d.add_cell("u_mac0", CellKind::Mac27);
        d.connect("abus", "u_mac0", "A");
        d.connect("bbus", "u_mac0", "B");
        d.connect("pbus", "u_mac0", "P");
        d.set_cell_attr("u_mac0", "WIDTH_A", "19").unwrap();
        let e = pack(&d).unwrap_err();
        assert!(e.contains("MULT18X18D"), "unexpected error: {e}");
    }

    #[test]
    fn pll_passes_dividers_through() {
        let mut d = Design::new("p");
        d.add_cell("pll0", CellKind::BlackBox { module: "EHXPLLL".to_string() });
        d.connect("clki", "pll0", "CLKI");
        d.connect("clkop", "pll0", "CLKOP");
        d.set_cell_attr("pll0", "CLKI_DIV", "1").unwrap();
        d.set_cell_attr("pll0", "CLKOP_DIV", "8").unwrap();
        d.set_cell_attr("pll0", "FEEDBK_PATH", "CLKOP").unwrap();
        let p = pack(&d).unwrap();
        assert_eq!(p.plls.len(), 1);
        let pll = &p.plls[0];
        assert_eq!(pll.ref_net, "clki");
        assert_eq!(pll.out_net, "clkop");
        let get = |k: &str| {
            pll.params
                .iter()
                .find(|(kk, _)| kk == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("CLKI_DIV").as_deref(), Some("1"));
        assert_eq!(get("CLKOP_DIV").as_deref(), Some("8"));
        assert_eq!(get("FEEDBK_PATH").as_deref(), Some("CLKOP"));
    }

    #[test]
    fn pll_missing_dividers_is_loud_error() {
        let mut d = Design::new("p");
        d.add_cell("pll0", CellKind::BlackBox { module: "PLLB".to_string() });
        d.connect("clki", "pll0", "CLKI");
        d.connect("clkop", "pll0", "CLKOP");
        let e = pack(&d).unwrap_err();
        assert!(e.contains("CLKI_DIV"), "unexpected error: {e}");
    }

    #[test]
    fn unknown_blackbox_is_loud_error() {
        let mut d = Design::new("b");
        d.add_cell("x", CellKind::BlackBox { module: "MYSTERY".to_string() });
        let e = pack(&d).unwrap_err();
        assert!(e.contains("MYSTERY"), "unexpected error: {e}");
    }

    #[test]
    fn ila_is_loud_error() {
        let mut d = Design::new("i");
        d.add_cell("ila0", CellKind::Ila { net: "probe".to_string() });
        assert!(pack(&d).is_err());
    }

    fn io_design(port: &str, dir: PortDir, loc: Option<&str>, cell_kind: CellKind) -> Design {
        let mut d = Design::new("io");
        d.add_port(port, dir);
        if let Some(l) = loc {
            d.set_port_attr(port, "LOC", l).unwrap();
        }
        let cell = "iob0";
        // Wire the fabric pin from the *cell* kind so a direction
        // mismatch between cell and port reaches the dir check.
        let is_out = matches!(cell_kind, CellKind::IobOut);
        d.add_cell(cell, cell_kind);
        if is_out {
            d.connect("fabric_out", cell, "I");
            d.connect(port, cell, "PAD");
        } else {
            d.connect("fabric_in", cell, "O");
            d.connect(port, cell, "PAD");
        }
        d
    }

    #[test]
    fn io_packs_with_loc_pin() {
        let d = io_design("led0", PortDir::Out, Some("A10"), CellKind::IobOut);
        let p = pack(&d).unwrap();
        assert_eq!(p.ios.len(), 1);
        let io = &p.ios[0];
        assert_eq!(io.port, "led0");
        assert_eq!(io.dir, IoDir::Out);
        assert_eq!(io.pin, "A10");
        assert_eq!(io.net, "fabric_out");
        // A10's site coordinates come from the Trellis package pin map.
        assert!(io.bank < 8, "bank out of range: {}", io.bank);
    }

    #[test]
    fn io_in_packs() {
        let d = io_design("clk", PortDir::In, Some("B5"), CellKind::IobIn);
        let p = pack(&d).unwrap();
        assert_eq!(p.ios.len(), 1);
        assert_eq!(p.ios[0].dir, IoDir::In);
        assert_eq!(p.ios[0].net, "fabric_in");
    }

    #[test]
    fn io_missing_loc_is_loud_error() {
        let d = io_design("led0", PortDir::Out, None, CellKind::IobOut);
        let e = pack(&d).unwrap_err();
        assert!(e.contains("LOC"), "unexpected error: {e}");
    }

    #[test]
    fn io_bad_pin_is_loud_error() {
        let d = io_design("led0", PortDir::Out, Some("ZZ99"), CellKind::IobOut);
        let e = pack(&d).unwrap_err();
        assert!(e.contains("ZZ99"), "unexpected error: {e}");
    }

    #[test]
    fn io_dir_mismatch_is_loud_error() {
        // IobOut cell on an input port.
        let d = io_design("clk", PortDir::In, Some("B5"), CellKind::IobOut);
        let e = pack(&d).unwrap_err();
        assert!(e.contains("not Out"), "unexpected error: {e}");
    }

    #[test]
    fn clockbuf_packs() {
        let mut d = Design::new("c");
        d.add_cell("clkbuf", CellKind::ClockBuf);
        d.connect("clk_in", "clkbuf", "I");
        d.connect("clk_glb", "clkbuf", "O");
        let p = pack(&d).unwrap();
        assert_eq!(p.clks.len(), 1);
        assert_eq!(p.clks[0].in_net, "clk_in");
        assert_eq!(p.clks[0].out_net, "clk_glb");
    }

    #[test]
    fn tiny_counter_bit_packs_end_to_end() {
        // notq -> FF -> led0, the smallest real flow through pack.
        let mut d = Design::new("top");
        d.add_port("clk", PortDir::In);
        d.set_port_attr("clk", "LOC", "B5").unwrap();
        d.add_port("led0", PortDir::Out);
        d.set_port_attr("led0", "LOC", "A10").unwrap();
        d.add_cell("clkb", CellKind::IobIn);
        d.connect("clk_fab", "clkb", "O");
        d.connect("clk", "clkb", "PAD");
        d.add_cell("notq", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
        d.connect("q", "notq", "I0");
        d.connect("notq_o", "notq", "O");
        d.add_cell("ff", CellKind::Hff);
        d.connect("notq_o", "ff", "D");
        d.connect("q", "ff", "Q");
        d.connect("clk_fab", "ff", "CLK");
        d.add_cell("ob", CellKind::IobOut);
        d.connect("q", "ob", "I");
        d.connect("led0", "ob", "PAD");
        let p = pack(&d).unwrap();
        assert_eq!(p.top, "top");
        assert_eq!(p.package, "CABGA381");
        assert_eq!(p.luts.len(), 1);
        assert_eq!(p.ffs.len(), 1);
        assert_eq!(p.ios.len(), 2);
        // The FF pairs with the inverter LUT.
        assert_eq!(p.ffs[0].packed_lut, Some(0));
        assert_eq!(p.luts[0].paired_ff, Some(0));
    }

    #[test]
    fn unknown_package_is_loud_error() {
        let mut d = Design::new("t");
        d.attrs.set("PACKAGE", "NOPE");
        let e = pack(&d).unwrap_err();
        assert!(e.contains("NOPE"), "unexpected error: {e}");
    }

    #[test]
    fn over_capacity_ebr_is_loud_error() {
        let mut d = Design::new("t");
        for i in 0..213 {
            d.add_cell(format!("r{i}"), CellKind::Bram18);
        }
        let e = pack(&d).unwrap_err();
        assert!(e.contains("over capacity"), "unexpected error: {e}");
        assert!(e.contains("DP16KD"), "unexpected error: {e}");
    }

    #[test]
    fn over_capacity_io_is_loud_error() {
        let mut d = Design::new("t");
        // CABGA381 has 205 pins; 206 IOs overflow.
        for i in 0..206 {
            let port = format!("p{i}");
            d.add_port(&port, PortDir::Out);
            d.set_port_attr(&port, "LOC", "A10").unwrap();
            let cell = format!("ob{i}");
            d.add_cell(&cell, CellKind::IobOut);
            d.connect(format!("n{i}"), &cell, "I");
            d.connect(&port, &cell, "PAD");
        }
        let e = pack(&d).unwrap_err();
        assert!(e.contains("over capacity"), "unexpected error: {e}");
        assert!(e.contains("PIO"), "unexpected error: {e}");
    }

    #[test]
    fn over_capacity_pll_is_loud_error() {
        let mut d = Design::new("t");
        for i in 0..7 {
            let cell = format!("pll{i}");
            d.add_cell(&cell, CellKind::BlackBox { module: "EHXPLLL".to_string() });
            d.connect(format!("r{i}"), &cell, "CLKI");
            d.connect(format!("o{i}"), &cell, "CLKOP");
            d.set_cell_attr(&cell, "CLKI_DIV", "1").unwrap();
            d.set_cell_attr(&cell, "CLKOP_DIV", "8").unwrap();
        }
        let e = pack(&d).unwrap_err();
        assert!(e.contains("over capacity"), "unexpected error: {e}");
        assert!(e.contains("PLL"), "unexpected error: {e}");
    }

    #[test]
    fn over_capacity_lut_is_loud_error() {
        let mut d = Design::new("t");
        // LFE5U-85F has 83640 LUT4s; constant LUT6s decompose 1:1.
        for i in 0..83_641 {
            let cell = format!("c{i}");
            d.add_cell(&cell, CellKind::Lut6 { init: 0 });
            d.connect(format!("o{i}"), &cell, "O");
        }
        let e = pack(&d).unwrap_err();
        assert!(e.contains("over capacity"), "unexpected error: {e}");
        assert!(e.contains("LUT4"), "unexpected error: {e}");
    }

    #[test]
    fn empty_design_packs() {
        let d = Design::new("empty");
        let p = pack(&d).unwrap();
        assert_eq!(p.top, "empty");
        assert!(p.luts.is_empty() && p.ffs.is_empty());
    }
}
