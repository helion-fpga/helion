//! Pack stage: [`helion_ir::Design`] → [`PackedIce`].
//!
//! Maps the vendor-neutral IR onto iCE40 primitives:
//! - `Lut6` is Shannon-decomposed into LUT4s (iCE40 LUTs are 4-input).
//! - `Hff` becomes a logic-cell flip-flop, paired with its driver LUT when
//!   possible.
//! - `IobIn`/`IobOut` become `SB_IO` blocks; `ClockBuf` becomes a global
//!   clock net; `Bram18` becomes `SB_RAM40_4K`; a `BlackBox` named
//!   `SB_PLL40*` becomes a PLL.
//! - `Mac27` and `Ila` are loud errors: HX8K has no DSP blocks and no ILA.

use helion_ir::{CellKind, Design};

/// A packed iCE40 LUT4 (one logic cell, combinational).
#[derive(Clone, Debug)]
pub struct PackedLut {
    /// Source IR cell name (synthesized names for decomposition products).
    pub name: String,
    /// 16-bit truth table: `init[n]` is the output for input value `n`
    /// where `n = in_3*8 + in_2*4 + in_1*2 + in_0`.
    pub init: u16,
    /// Nets on in_0..in_3 (`None` = unconnected, tied low at emission).
    pub input_nets: [Option<String>; 4],
    /// Net on out.
    pub output_net: String,
    /// Set for carry-chain sum LUTs: placement must put this LUT in the
    /// chain's `seq`-th logic cell (vertically adjacent, same column).
    /// `in_3` is a placeholder carry net; the router skips it and asc.rs
    /// programs the dedicated in_3 carry buffer directly.
    pub carry: Option<CarryCell>,
}

/// Placement hint for a carry-chain sum LUT.
#[derive(Clone, Debug)]
pub struct CarryCell {
    /// Index into [`PackedIce::carry_chains`].
    pub chain: usize,
    /// Bit position within the chain (0 = bottom, carry flows upward).
    pub seq: usize,
}

/// A chain of one or more [`helion_ir::CellKind::Carry4`] cells, in bit
/// order. Placed as that many vertically adjacent logic cells.
#[derive(Clone, Debug)]
pub struct CarryChain {
    /// True when the bottom bit's carry_in is constant 1 (CarryInSet);
    /// false for constant 0. Chained cells share the head's setting.
    pub cin_one: bool,
    /// Number of carry bits (4 per Carry4 cell).
    pub bits: usize,
}

/// A packed iCE40 flip-flop (one logic cell, sequential).
#[derive(Clone, Debug)]
pub struct PackedFf {
    pub name: String,
    pub d_net: String,
    pub q_net: String,
    pub clk_net: String,
}

/// A packed SB_IO block.
#[derive(Clone, Debug)]
pub struct PackedIo {
    pub name: String,
    pub port: String,
    pub dir: IoDir,
    /// Fabric-side net (`D_OUT_0` for outputs, `D_IN_0` for inputs).
    pub net: String,
    /// CT256 package pin, e.g. `"B5"`. Resolved from the port name via the
    /// breakout-board map; explicit `LOC` attributes win when present.
    pub pin: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoDir {
    In,
    Out,
}

/// A packed global clock: the net driven onto `lutff_global/clk`.
#[derive(Clone, Debug)]
pub struct PackedClk {
    pub name: String,
    /// Net into the clock root (the IO pin net).
    pub in_net: String,
    /// Global clock net name.
    pub out_net: String,
}

/// A packed SB_RAM40_4K block.
#[derive(Clone, Debug)]
pub struct PackedRam {
    pub name: String,
    /// 16-bit write/read mode: 0 = 16-bit, 1 = 8-bit, 2 = 4-bit, 3 = 2-bit.
    pub write_mode: u8,
    pub read_mode: u8,
    pub rdata_net: [String; 16],
    pub wdata_net: [String; 16],
    pub raddr_net: [String; 11],
    pub waddr_net: [String; 11],
    pub rclk_net: String,
    pub wclk_net: String,
    pub re_net: Option<String>,
    pub we_net: Option<String>,
}

/// A packed SB_PLL40.
#[derive(Clone, Debug)]
pub struct PackedPll {
    pub name: String,
    pub kind: PllKind,
    pub ref_net: String,
    pub out_net: String,
    /// Desired output frequency in Hz (from the `FREQ` attribute, if any).
    pub out_freq_hz: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PllKind {
    Pad,
    Core,
}

/// Backend-owned packed netlist for the iCE40 HX8K.
#[derive(Clone, Debug, Default)]
pub struct PackedIce {
    pub luts: Vec<PackedLut>,
    pub ffs: Vec<PackedFf>,
    pub ios: Vec<PackedIo>,
    pub clks: Vec<PackedClk>,
    pub rams: Vec<PackedRam>,
    pub plls: Vec<PackedPll>,
    /// Carry chains, in the order their sum LUTs reference via
    /// [`CarryCell::chain`].
    pub carry_chains: Vec<CarryChain>,
}

/// Decompose a 6-LUT truth table into LUT4s via Shannon decomposition.
///
/// Returns `(luts, top_output_net)`: the list of LUT4s (with fresh internal
/// nets) and the net carrying the function output. Uses at most 7 LUT4s;
/// fewer when the function ignores inputs.
fn decompose_lut6(name: &str, init: u64, inputs: &[Option<String>; 6]) -> Result<Vec<PackedLut>, String> {
    // Collect the actually-used input positions.
    let used: Vec<usize> = (0..6).filter(|&i| inputs[i].is_some()).collect();
    if used.is_empty() {
        // Constant function: single LUT4 tied off.
        let out = (init & 1) != 0;
        return Ok(vec![PackedLut {
            name: format!("{name}$const"),
            init: if out { 0xFFFF } else { 0x0000 },
            input_nets: [None, None, None, None],
            output_net: format!("{name}$O"),
            carry: None,
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
        return Ok(vec![PackedLut {
            name: format!("{name}$lut"),
            init: lut_init,
            input_nets: in_nets,
            output_net: format!("{name}$O"),
            carry: None,
        }]);
    }
    // Shannon decomposition on the highest used input.
    // f = sel ? f1 : f0, each cofactor has one fewer input.
    let sel_pos = used[used.len() - 1];
    let sel_net = inputs[sel_pos].clone().ok_or_else(|| {
        format!("ice40: LUT {name} input I{sel_pos} vanished during decomposition")
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
        .ok_or_else(|| format!("ice40: decomposition of {name}$f0 produced no LUTs"))?
        .output_net
        .clone();
    let mut luts1 = decompose_lut6(&format!("{name}$f1"), c1, &in1)?;
    let out1 = luts1
        .last()
        .ok_or_else(|| format!("ice40: decomposition of {name}$f1 produced no LUTs"))?
        .output_net
        .clone();
    luts.append(&mut luts1);
    // Mux: sel ? out1 : out0. LUT4(sel, out0, out1) = (sel & out1) | (!sel & out0).
    // init bit n (n = in_2*4 + in_1*2 + in_0): in_0=sel, in_1=out0, in_2=out1.
    let mut mux_init: u16 = 0;
    for n in 0..16u16 {
        let sel = n & 1;
        let a = (n >> 1) & 1;
        let b = (n >> 2) & 1;
        let o = if sel == 1 { b } else { a };
        if o == 1 {
            mux_init |= 1 << n;
        }
    }
    luts.push(PackedLut {
        name: format!("{name}$mux"),
        init: mux_init,
        input_nets: [Some(sel_net), Some(out0), Some(out1), None],
        output_net: format!("{name}$O"),
        carry: None,
    });
    Ok(luts)
}

/// Pack a [`Design`] into iCE40 primitives.
pub fn pack(netlist: &Design) -> Result<PackedIce, String> {
    let pins = netlist.pin_index();
    let mut out = PackedIce::default();
    // Carry4 cells are collected first: chains reference each other via
    // CI/CO[3] regardless of cell order; emission happens after the loop.
    let mut carry_cells: Vec<Carry4Data> = Vec::new();

    for cell in &netlist.cells {
        match &cell.kind {
            CellKind::Carry4 => {
                carry_cells.push(parse_carry4(&cell.name, &pins, netlist)?);
            }
            CellKind::Lut6 { init } => {
                let mut inputs: [Option<String>; 6] = Default::default();
                for (i, slot) in inputs.iter_mut().enumerate() {
                    *slot = pins
                        .net_on(&cell.name, &format!("I{i}"))
                        .map(|s| s.to_string());
                }
                let output_net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: LUT {} has no O net", cell.name))?
                    .to_string();
                let mut luts = decompose_lut6(&cell.name, *init, &inputs)?;
                // Rename the top output net to the real one.
                if let Some(top) = luts.last_mut() {
                    top.output_net = output_net;
                }
                out.luts.append(&mut luts);
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
                });
            }
            CellKind::IobOut => {
                let net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: OBUF {} has no I net", cell.name))?
                    .to_string();
                let port = pins
                    .net_on(&cell.name, "PAD")
                    .unwrap_or(&cell.name)
                    .to_string();
                out.ios.push(PackedIo {
                    name: cell.name.clone(),
                    pin: resolve_pin(netlist, &port)?,
                    port,
                    dir: IoDir::Out,
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
                out.ios.push(PackedIo {
                    name: cell.name.clone(),
                    pin: resolve_pin(netlist, &port)?,
                    port,
                    dir: IoDir::In,
                    net,
                });
            }
            CellKind::ClockBuf => {
                let in_net = pins
                    .net_on(&cell.name, "I")
                    .ok_or_else(|| format!("pack: CLKBUF {} has no I net", cell.name))?
                    .to_string();
                let out_net = pins
                    .net_on(&cell.name, "O")
                    .ok_or_else(|| format!("pack: CLKBUF {} has no O net", cell.name))?
                    .to_string();
                out.clks.push(PackedClk {
                    name: cell.name.clone(),
                    in_net,
                    out_net,
                });
            }
            CellKind::Bram18 => {
                out.rams.push(pack_ram(&cell.name, &pins)?);
            }
            CellKind::BlackBox { module } if module.starts_with("SB_PLL40") => {
                let kind = if module.contains("PAD") {
                    PllKind::Pad
                } else {
                    PllKind::Core
                };
                let ref_net = pins
                    .net_on(&cell.name, "REFERENCECLK")
                    .or_else(|| pins.net_on(&cell.name, "I"))
                    .ok_or_else(|| format!("pack: PLL {} has no clock input", cell.name))?
                    .to_string();
                let out_net = pins
                    .net_on(&cell.name, "PLLOUTGLOBAL")
                    .or_else(|| pins.net_on(&cell.name, "O"))
                    .ok_or_else(|| format!("pack: PLL {} has no output", cell.name))?
                    .to_string();
                out.plls.push(PackedPll {
                    name: cell.name.clone(),
                    kind,
                    ref_net,
                    out_net,
                    out_freq_hz: 0,
                });
            }
            CellKind::Mac27 => {
                return Err(format!(
                    "pack: MAC27 {} unsupported: iCE40 HX8K has no DSP blocks",
                    cell.name
                ));
            }
            CellKind::Ila { .. } => {
                return Err(format!(
                    "pack: ILA {} unsupported on the iCE40 backend",
                    cell.name
                ));
            }
            other => {
                return Err(format!(
                    "pack: unsupported CellKind {other:?} for iCE40 (cell {})",
                    cell.name
                ));
            }
        }
    }

    // Emit carry chains after all cells are parsed (chains link cells in
    // any order).
    emit_carry_chains(netlist, &carry_cells, &mut out)?;

    // The ClockBuf is a logical global-clock marker. This backend routes
    // clocks as ordinary nets (via local tracks to lutff_global/clk), so
    // merge each ClockBuf's output net into its input net.
    for clk in &out.clks {
        for ff in out.ffs.iter_mut() {
            if ff.clk_net == clk.out_net {
                ff.clk_net = clk.in_net.clone();
            }
        }
    }

    // Pair each FF with its driver LUT when the driver is a packed LUT4 with
    // no other loads: they share one logic cell (LUT -> FF in the same LC).
    // (Pairing is recorded at place time; pack just keeps both lists.)

    Ok(out)
}

/// Resolve a top-level port to a CT256 package pin.
fn resolve_pin(netlist: &Design, port: &str) -> Result<String, String> {
    if let Some(def) = netlist.ports.iter().find(|p| p.name == port) {
        if let Some(loc) = def.attrs.get("LOC") {
            return Ok(loc.to_string());
        }
        // Breakout-board convention: clk -> J3, ledN -> LEDN pin.
        let map = crate::device::ct256_pin_map();
        if port == "clk" {
            return Ok("J3".to_string());
        }
        if let Some(led) = port.strip_prefix("led") {
            let names = ["B5", "B4", "A2", "A1", "C5", "C4", "B3", "C3"];
            if let Ok(n) = led.parse::<usize>() {
                if let Some(pin) = names.get(n) {
                    let _ = map;
                    return Ok(pin.to_string());
                }
            }
        }
        // Heuristic: port name matches a package pin directly.
        if map.contains_key(port) {
            return Ok(port.to_string());
        }
    }
    Err(format!(
        "pack: no package pin for port '{port}': add a LOC attribute or use the breakout-board naming (clk, led0..led7)"
    ))
}

/// Pack a Bram18 cell into SB_RAM40_4K shape.
fn pack_ram(
    name: &str,
    pins: &helion_ir::PinIndex,
) -> Result<PackedRam, String> {
    let bus = |prefix: &str, n: usize| -> Result<[String; 16], String> {
        let mut arr: [String; 16] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            if i < n {
                *slot = pins
                    .net_on(name, &format!("{prefix}{i}"))
                    .unwrap_or("gnd")
                    .to_string();
            } else {
                *slot = "gnd".to_string();
            }
        }
        Ok(arr)
    };
    let addr = |prefix: &str| -> Result<[String; 11], String> {
        let mut arr: [String; 11] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            *slot = pins
                .net_on(name, &format!("{prefix}{i}"))
                .unwrap_or("gnd")
                .to_string();
        }
        Ok(arr)
    };
    let need = |pin: &str| {
        pins.net_on(name, pin)
            .map(|s| s.to_string())
            .ok_or_else(|| format!("pack: RAM {name} has no {pin} net"))
    };
    Ok(PackedRam {
        name: name.to_string(),
        write_mode: 0,
        read_mode: 0,
        rdata_net: bus("DO", 16)?,
        wdata_net: bus("DI", 16)?,
        raddr_net: addr("RADDR")?,
        waddr_net: addr("WADDR")?,
        rclk_net: need("RCLK").unwrap_or_else(|_| "gnd".to_string()),
        wclk_net: need("WCLK").unwrap_or_else(|_| "gnd".to_string()),
        re_net: pins.net_on(name, "RE").map(|s| s.to_string()),
        we_net: pins.net_on(name, "WE").map(|s| s.to_string()),
    })
}

/// Carry-chain support: [`helion_ir::CellKind::Carry4`] on iCE40.
///
/// Pin convention follows Xilinx CARRY4 (the IR names it a "Xilinx 7-series
/// CARRY4 fast carry chain"): inputs `CI`, `CYINIT`, `DI0`..`DI3`,
/// `S0`..`S3`; outputs `O0`..`O3` (sums), `CO0`..`CO3` (carry outs).
///
/// iCE40 mapping (truthful to the IceStorm logic-tile documentation):
/// the LC carry unit computes `cout = majority(in_1, in_2, carry_in)`.
/// Xilinx's MUXCY computes `co[i] = S[i] ? carry[i] : DI[i]`. These agree
/// exactly when `in_1 = DI[i]` and `in_2 = S[i] ^ DI[i]` (when `S[i]=0`,
/// MUXCY = `DI[i]` = majority(`DI[i]`,`DI[i]`,c); when `S[i]=1`, MUXCY = `c`
/// = majority(`DI[i]`,`~DI[i]`,c)).
/// So each bit gets an (often optimized away) XOR LUT for `S[i] ^ DI[i]`,
/// and the LC's own LUT computes the sum `O[i] = S[i] ^ carry[i]`
/// (init `0x55AA` = `in_0 ^ in_3`, with `in_0 = S[i]`, `in_3 = carry[i]`).
///
/// `carry[0]` is the tile `carry_in_mux`: constant 0 (default), constant 1
/// (`CarryInSet`), or the tile-below `lutff_7/cout` for chained tiles.
/// `carry[i] = cout[i-1]` is a dedicated wire (no config bits).
/// `in_3` of each carry LC is driven from the carry (for the sum LUT) via
/// the documented `lutff_{i-1}/cout -> lutff_i/in_3` / `carry_in_mux ->
/// lutff_0/in_3` buffers, programmed directly by asc.rs.
///
/// `cout` is not routable to general fabric (IceStorm docs: only the
/// post-FF signal leaves the logic block), so `CO0`..`CO2` must be
/// unconnected and `CO3` may only drive another Carry4's `CI`.
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
    name: String,
    cin: CarryIn,
    s: [Option<String>; 4],
    di: [Option<String>; 4],
    o: [Option<String>; 4],
    co: [Option<String>; 4],
}

fn is_const1(net: Option<&str>) -> bool {
    matches!(net, Some("1" | "vcc" | "VCC" | "$true"))
}

fn is_const0(net: Option<&str>) -> bool {
    matches!(net, None | Some("0" | "gnd" | "GND" | "$false"))
}

/// Find the Carry4 cell driving `net` via its CO[3] pin.
fn carry_co3_driver(netlist: &Design, net: &str) -> Option<String> {
    let n = netlist.nets.iter().find(|n| n.name == net)?;
    for e in &n.endpoints {
        if e.pin != "CO3" {
            continue;
        }
        if let Some(cell) = netlist.cells.iter().find(|c| c.name == e.cell) {
            if matches!(cell.kind, CellKind::Carry4) {
                return Some(cell.name.clone());
            }
        }
    }
    None
}

/// Lazily-created constant drivers for carry logic.
#[derive(Default)]
struct ConstDrivers {
    zero: Option<String>,
    one: Option<String>,
}

impl ConstDrivers {
    fn zero(&mut self, out: &mut PackedIce) -> String {
        if let Some(n) = &self.zero {
            return n.clone();
        }
        let n = "$carry$zero".to_string();
        out.luts.push(PackedLut {
            name: n.clone(),
            init: 0x0000,
            input_nets: [None, None, None, None],
            output_net: n.clone(),
            carry: None,
        });
        self.zero = Some(n.clone());
        n
    }

    fn one(&mut self, out: &mut PackedIce) -> String {
        if let Some(n) = &self.one {
            return n.clone();
        }
        let n = "$carry$one".to_string();
        out.luts.push(PackedLut {
            name: n.clone(),
            init: 0xFFFF,
            input_nets: [None, None, None, None],
            output_net: n.clone(),
            carry: None,
        });
        self.one = Some(n.clone());
        n
    }

    /// Resolve a maybe-constant net to a driven net name.
    fn resolve(&mut self, net: Option<&str>, out: &mut PackedIce) -> String {
        match net {
            Some(n) if is_const1(Some(n)) => self.one(out),
            Some(n) if is_const0(Some(n)) => self.zero(out),
            Some(n) => n.to_string(),
            // Unconnected counts as constant 0.
            None => self.zero(out),
        }
    }
}

/// Parse and validate one Carry4 cell.
fn parse_carry4(
    name: &str,
    pins: &helion_ir::PinIndex,
    netlist: &Design,
) -> Result<Carry4Data, String> {
    let cyinit = pins.net_on(name, "CYINIT");
    let ci = pins.net_on(name, "CI");
    // The iCE40 carry_in_mux supports only constant 0/1 or the tile-below
    // carry; a general net on CYINIT/CI is unrepresentable.
    let cin = if is_const1(cyinit) {
        if !is_const0(ci) {
            return Err(format!(
                "pack: Carry4 {name}: CYINIT=1 with a driven CI is ambiguous"
            ));
        }
        CarryIn::One
    } else if is_const1(ci) {
        if !is_const0(cyinit) {
            return Err(format!(
                "pack: Carry4 {name}: CI=1 with a driven CYINIT is ambiguous"
            ));
        }
        CarryIn::One
    } else if let Some(ci_net) = ci {
        if !is_const0(cyinit) {
            return Err(format!(
                "pack: Carry4 {name}: chained CI with a driven CYINIT is unsupported"
            ));
        }
        let prev = carry_co3_driver(netlist, ci_net).ok_or_else(|| {
            format!(
                "pack: Carry4 {name}: CI net '{ci_net}' is not driven by a Carry4 CO[3]; \
                 iCE40 carry_in supports only constant 0/1 or a chained carry"
            )
        })?;
        if prev == name {
            return Err(format!("pack: Carry4 {name}: CI chained to itself"));
        }
        CarryIn::Prev(prev)
    } else {
        if !is_const0(cyinit) {
            return Err(format!(
                "pack: Carry4 {name}: CYINIT must be constant 0/1 or unconnected"
            ));
        }
        CarryIn::Zero
    };

    let mut s: [Option<String>; 4] = Default::default();
    let mut di: [Option<String>; 4] = Default::default();
    let mut o: [Option<String>; 4] = Default::default();
    let mut co: [Option<String>; 4] = Default::default();
    for i in 0..4 {
        s[i] = pins.net_on(name, &format!("S{i}")).map(|n| n.to_string());
        di[i] = pins.net_on(name, &format!("DI{i}")).map(|n| n.to_string());
        o[i] = pins.net_on(name, &format!("O{i}")).map(|n| n.to_string());
        co[i] = pins.net_on(name, &format!("CO{i}")).map(|n| n.to_string());
    }
    // CO[0..2] are internal to the dedicated chain; cout is not routable to
    // general fabric, so observing them is unrepresentable.
    for (i, co_i) in co.iter().enumerate().take(3) {
        if co_i.is_some() {
            return Err(format!(
                "pack: Carry4 {name}: CO{i} is internal to the carry chain and \
                 cannot be observed (iCE40 cout is not routable to fabric)"
            ));
        }
    }
    Ok(Carry4Data {
        name: name.to_string(),
        cin,
        s,
        di,
        o,
        co,
    })
}

/// Order Carry4 cells into chains, validate CO[3] usage, and emit the sum
/// LUTs plus [`CarryChain`] records.
fn emit_carry_chains(
    netlist: &Design,
    cells: &[Carry4Data],
    out: &mut PackedIce,
) -> Result<(), String> {
    let idx: std::collections::HashMap<&str, usize> = cells
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect();
    // next[i] = cell chained after i via CO[3] -> CI.
    let mut next: Vec<Option<usize>> = vec![None; cells.len()];
    for (i, cell) in cells.iter().enumerate() {
        if let CarryIn::Prev(prev_name) = &cell.cin {
            let &pi = idx.get(prev_name.as_str()).ok_or_else(|| {
                format!(
                    "pack: Carry4 {}: previous cell '{prev_name}' not found",
                    cell.name
                )
            })?;
            if next[pi].is_some() {
                return Err(format!(
                    "pack: Carry4 {}: CO[3] fans out to multiple carry chains; \
                     the dedicated carry wire is point-to-point",
                    cells[pi].name
                ));
            }
            next[pi] = Some(i);
        }
    }
    // CO[3] may only drive the chained Carry4's CI (or be unconnected).
    for (i, cell) in cells.iter().enumerate() {
        if let Some(co3) = &cell.co[3] {
            let net = netlist
                .nets
                .iter()
                .find(|n| &n.name == co3)
                .ok_or_else(|| format!("pack: Carry4 {}: CO3 net vanished", cell.name))?;
            let allowed_ci = next[i].map(|n| cells[n].name.as_str());
            for e in &net.endpoints {
                if e.cell == cell.name && e.pin == "CO3" {
                    continue;
                }
                if e.pin == "CI" && Some(e.cell.as_str()) == allowed_ci {
                    continue;
                }
                return Err(format!(
                    "pack: Carry4 {}: CO[3] net '{co3}' drives {}.{} which is not the \
                     chained carry; iCE40 cout is not routable to general fabric",
                    cell.name, e.cell, e.pin
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
                    "pack: Carry4 {}: carry chain cycle detected",
                    cells[nxt].name
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
                "pack: Carry4 {}: carry chain has no head (cycle or dangling CI)",
                cell.name
            ));
        }
    }

    let mut consts = ConstDrivers::default();
    for chain in &chains {
        let head = &cells[chain[0]];
        let cin_one = matches!(head.cin, CarryIn::One);
        let chain_idx = out.carry_chains.len();
        let mut seq = 0;
        for &ci in chain {
            let cell = &cells[ci];
            for bit in 0..4 {
                let s = cell.s[bit].as_deref();
                let di = cell.di[bit].as_deref();
                let s_net = consts.resolve(s, out);
                let di_net = consts.resolve(di, out);
                // in_2 = S[i] ^ DI[i]; constant-fold the trivial cases.
                let x_net = if is_const0(s) {
                    di_net.clone()
                } else if is_const0(di) {
                    s_net.clone()
                } else {
                    let xor_out = format!("{}${bit}$xor", cell.name);
                    out.luts.push(PackedLut {
                        name: format!("{}${bit}$xor", cell.name),
                        init: 0x6666, // in_0 ^ in_1
                        input_nets: [Some(di_net.clone()), Some(s_net.clone()), None, None],
                        output_net: xor_out.clone(),
                        carry: None,
                    });
                    xor_out
                };
                // Sum LUT: O[i] = S[i] ^ carry[i]; in_3 is a placeholder,
                // the router skips it and asc.rs programs the dedicated
                // in_3 carry buffer.
                let o_net = cell.o[bit]
                    .clone()
                    .unwrap_or_else(|| format!("{}${bit}$o", cell.name));
                out.luts.push(PackedLut {
                    name: format!("{}${bit}$sum", cell.name),
                    init: 0x55AA, // in_0 ^ in_3
                    input_nets: [
                        Some(s_net),
                        Some(di_net),
                        Some(x_net),
                        Some(format!("{}${bit}$cin", cell.name)),
                    ],
                    output_net: o_net,
                    carry: Some(CarryCell {
                        chain: chain_idx,
                        seq,
                    }),
                });
                seq += 1;
            }
        }
        out.carry_chains.push(CarryChain {
            cin_one,
            bits: chain.len() * 4,
        });
    }
    Ok(())
}

/// Map a LUT4 16-bit init to the 20 LC config bits.
///
/// Truth-table row `n` (n = in_3*8+in_2*4+in_1*2+in_0) lives at the LC bit
/// given by the IceStorm logic-tile documentation table.
pub fn lut4_init_to_lc_bits(init: u16) -> [bool; 20] {    // row n -> LC bit index, from the docs table.
    const ROW_TO_LC: [usize; 16] = [4, 14, 15, 5, 6, 16, 17, 7, 3, 13, 12, 2, 1, 11, 10, 0];
    let mut bits = [false; 20];
    for n in 0..16 {
        bits[ROW_TO_LC[n]] = (init >> n) & 1 == 1;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut4_passthrough_decomposition() {
        // f = in_0 (buffer). init bit n = bit0 of n.
        let init: u64 = 0xAAAAAAAAAAAAAAAA;
        let inputs: [Option<String>; 6] = [
            Some("a".into()),
            None,
            None,
            None,
            None,
            None,
        ];
        let luts = decompose_lut6("u", init, &inputs).unwrap();
        assert_eq!(luts.len(), 1);
        // LUT4(a) = a: init bit n = (n>>0)&1.
        assert_eq!(luts[0].init, 0xAAAA);
        assert_eq!(luts[0].input_nets[0], Some("a".to_string()));
    }

    #[test]
    fn lut6_xor_decomposition_is_correct() {
        // 6-input XOR: evaluate decomposed netlist in software.
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
        // Evaluate: map net name -> value.
        for a in 0..64u64 {
            let mut vals: std::collections::HashMap<String, bool> = (0..6)
                .map(|i| (format!("i{i}"), (a >> i) & 1 == 1))
                .collect();
            for lut in &luts {
                let mut n = 0u16;
                for (k, net) in lut.input_nets.iter().enumerate() {
                    if let Some(net) = net {
                        if vals.get(net).copied().unwrap_or(false) {
                            n |= 1 << k;
                        }
                    }
                }
                vals.insert(lut.output_net.clone(), (lut.init >> n) & 1 == 1);
            }
            let got = vals["x$O"];
            let want = a.count_ones() % 2 == 1;
            assert_eq!(got, want, "mismatch at input {a:06b}");
        }
    }

    #[test]
    fn lc_bit_mapping_spot_check() {
        // init = 1 only for n=0 -> LC[4] set.
        let bits = lut4_init_to_lc_bits(0x0001);
        assert!(bits[4]);
        assert_eq!(bits.iter().filter(|&&b| b).count(), 1);
        // init = all ones -> 16 bits set.
        let bits = lut4_init_to_lc_bits(0xFFFF);
        assert_eq!(bits.iter().filter(|&&b| b).count(), 16);
    }
}
