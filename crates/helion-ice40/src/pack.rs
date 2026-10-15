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

use helion_ir::{intern, resolve, CellKind, Design};

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

/// Compute SB_PLL40 divider settings with the IceStorm `icepll` algorithm
/// (SIMPLE feedback path, `icepll.cc::analyze`):
/// `f_pfd = f_in/(divr+1)` in 10..133 MHz, `f_vco = f_pfd*(divf+1)` in
/// 533..1066 MHz, `divq` in 1..=6, minimizing `|fout - target|` where
/// `fout = f_vco / 2^divq`. Ties break toward smaller divr, then divf,
/// then divq (first strictly-better wins, as in icepll).
/// Returns `(divr, divf, divq, filter_range)`.
fn pll_dividers(name: &str, ref_hz: u64, out_hz: u64) -> Result<(u8, u8, u8, u8), String> {
    let f_in = ref_hz as f64 / 1e6;
    let f_target = out_hz as f64 / 1e6;
    if !(10.0..=133.0).contains(&f_in) {
        return Err(format!(
            "pack: PLL {name}: reference frequency {f_in:.3} MHz is outside the 10-133 MHz PLL input range"
        ));
    }
    if !(16.0..=275.0).contains(&f_target) {
        return Err(format!(
            "pack: PLL {name}: output frequency {f_target:.3} MHz is outside the 16-275 MHz PLL output range"
        ));
    }
    let mut best: Option<(f64, u8, u8, u8)> = None;
    for divr in 0..=15u8 {
        let f_pfd = f_in / f64::from(divr + 1);
        if !(10.0..=133.0).contains(&f_pfd) {
            continue;
        }
        for divf in 0..=127u8 {
            let f_vco = f_pfd * f64::from(divf + 1);
            if !(533.0..=1066.0).contains(&f_vco) {
                continue;
            }
            for divq in 1..=6u8 {
                let fout = f_vco / 2f64.powi(i32::from(divq));
                let better = match best {
                    None => true,
                    Some((best_fout, _, _, _)) => {
                        (fout - f_target).abs() < (best_fout - f_target).abs()
                    }
                };
                if better {
                    best = Some((fout, divr, divf, divq));
                }
            }
        }
    }
    let (_, divr, divf, divq) = best.ok_or_else(|| {
        format!("pack: PLL {name}: no valid divider setting for {f_in:.3} MHz -> {f_target:.3} MHz")
    })?;
    // Filter range from f_pfd, per icepll.cc.
    let f_pfd = f_in / f64::from(divr + 1);
    let filter_range = if f_pfd < 17.0 {
        1
    } else if f_pfd < 26.0 {
        2
    } else if f_pfd < 44.0 {
        3
    } else if f_pfd < 66.0 {
        4
    } else if f_pfd < 101.0 {
        5
    } else {
        6
    };
    Ok((divr, divf, divq, filter_range))
}

/// Parse a frequency attribute (Hz, integer) for a PLL cell.
fn parse_pll_freq(name: &str, attr: &str, v: &str) -> Result<u64, String> {
    v.trim().parse::<u64>().map_err(|_| {
        format!("pack: PLL {name}: {attr}='{v}' is not an integer frequency in Hz")
    })
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
    /// From the optional `WRITE_MODE` / `READ_MODE` cell attributes
    /// (yosys SB_RAM40_4K param spelling: `0`..`3` or `00`..`11`); default 0.
    pub write_mode: u8,
    pub read_mode: u8,
    /// 256x16 physical init words. `init[w]` is the 16-bit word at flat
    /// address `w`; flat bit `w*16+b` lands in the `.ram_data` bitstream at
    /// flat bit `w*16+b` (bit 0 = LSB of the word).
    ///
    /// Parsed from the optional `INIT_0`..`INIT_F` cell attributes, using
    /// yosys's SB_RAM40_4K param semantics: each attribute is a hex string
    /// of up to 256 bits, leftmost char = bit 255, and `INIT_x` bit `p` is
    /// flat bit `x*256+p`. `x`/`z` chars are treated as 0 (as arachne-pnr
    /// does); absent attributes mean all-zero init.
    pub init: [u16; 256],
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
    /// Desired output frequency in Hz (from the `FREQ` cell attribute).
    pub out_freq_hz: u64,
    /// Reference clock frequency in Hz (from the `REF_FREQ` cell attribute).
    pub ref_freq_hz: u64,
    /// Divider settings computed from the two frequencies with the
    /// IceStorm `icepll` algorithm (SIMPLE feedback path).
    pub divr: u8,
    pub divf: u8,
    pub divq: u8,
    pub filter_range: u8,
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
                carry_cells.push(parse_carry4(resolve(cell.name), &pins, netlist)?);
            }
            CellKind::Lut6 { init } => {
                let mut inputs: [Option<String>; 6] = Default::default();
                for (i, slot) in inputs.iter_mut().enumerate() {
                    *slot = pins
                        .net_on(resolve(cell.name), &format!("I{i}"))
                        .map(|s| resolve(s).to_string());
                }
                let output_net = resolve(pins
                    .net_on(resolve(cell.name), "O")
                    .ok_or_else(|| format!("pack: LUT {} has no O net", resolve(cell.name)))?)
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
                        .ok_or_else(|| format!("pack: FF {} has no {pin} net", resolve(cell.name)))
                };
                out.ffs.push(PackedFf {
                    name: resolve(cell.name).to_string(),
                    d_net: need("D")?,
                    q_net: need("Q")?,
                    clk_net: need("CLK")?,
                });
            }
            CellKind::IobOut => {
                let net = resolve(pins
                    .net_on(resolve(cell.name), "I")
                    .ok_or_else(|| format!("pack: OBUF {} has no I net", resolve(cell.name)))?)
                .to_string();
                let port = resolve(pins
                    .net_on(resolve(cell.name), "PAD")
                    .unwrap_or(cell.name))
                .to_string();
                out.ios.push(PackedIo {
                    name: resolve(cell.name).to_string(),
                    pin: resolve_pin(netlist, &port)?,
                    port,
                    dir: IoDir::Out,
                    net,
                });
            }
            CellKind::IobIn => {
                let net = resolve(pins
                    .net_on(resolve(cell.name), "O")
                    .ok_or_else(|| format!("pack: IBUF {} has no O net", resolve(cell.name)))?)
                .to_string();
                let port = resolve(pins
                    .net_on(resolve(cell.name), "PAD")
                    .unwrap_or(cell.name))
                .to_string();
                out.ios.push(PackedIo {
                    name: resolve(cell.name).to_string(),
                    pin: resolve_pin(netlist, &port)?,
                    port,
                    dir: IoDir::In,
                    net,
                });
            }
            CellKind::ClockBuf => {
                let in_net = resolve(pins
                    .net_on(resolve(cell.name), "I")
                    .ok_or_else(|| format!("pack: CLKBUF {} has no I net", resolve(cell.name)))?)
                .to_string();
                let out_net = resolve(pins
                    .net_on(resolve(cell.name), "O")
                    .ok_or_else(|| format!("pack: CLKBUF {} has no O net", resolve(cell.name)))?)
                .to_string();
                out.clks.push(PackedClk {
                    name: resolve(cell.name).to_string(),
                    in_net,
                    out_net,
                });
            }
            CellKind::Bram18 | CellKind::Bram18e1 { .. } => {
                out.rams.push(pack_ram(resolve(cell.name), &pins, &cell.attrs)?);
            }
            CellKind::BlackBox { module } if resolve(*module).starts_with("SB_PLL40") => {
                let kind = if resolve(*module).contains("PAD") {
                    PllKind::Pad
                } else {
                    PllKind::Core
                };
                let ref_net = resolve(pins
                    .net_on(resolve(cell.name), "REFERENCECLK")
                    .or_else(|| pins.net_on(resolve(cell.name), "I"))
                    .ok_or_else(|| format!("pack: PLL {} has no clock input", resolve(cell.name)))?)
                .to_string();
                let out_net = resolve(pins
                    .net_on(resolve(cell.name), "PLLOUTGLOBAL")
                    .or_else(|| pins.net_on(resolve(cell.name), "O"))
                    .ok_or_else(|| format!("pack: PLL {} has no output", resolve(cell.name)))?)
                .to_string();
                let out_freq_hz = match cell.attrs.get("FREQ") {
                    Some(v) => parse_pll_freq(resolve(cell.name), "FREQ", v)?,
                    None => {
                        return Err(format!(
                            "pack: PLL {} needs a FREQ attribute (desired output frequency in Hz)",
                            resolve(cell.name)
                        ))
                    }
                };
                let ref_freq_hz = match cell.attrs.get("REF_FREQ") {
                    Some(v) => parse_pll_freq(resolve(cell.name), "REF_FREQ", v)?,
                    None => {
                        return Err(format!(
                            "pack: PLL {} needs a REF_FREQ attribute (reference clock frequency in Hz)",
                            resolve(cell.name)
                        ))
                    }
                };
                let (divr, divf, divq, filter_range) =
                    pll_dividers(resolve(cell.name), ref_freq_hz, out_freq_hz)?;
                out.plls.push(PackedPll {
                    name: resolve(cell.name).to_string(),
                    kind,
                    ref_net,
                    out_net,
                    out_freq_hz,
                    ref_freq_hz,
                    divr,
                    divf,
                    divq,
                    filter_range,
                });
            }
            CellKind::Mac27 | CellKind::Dsp48e1 { .. } => {
                return Err(format!(
                    "pack: MAC27 {} unsupported: iCE40 HX8K has no DSP blocks",
                    resolve(cell.name)
                ));
            }
            CellKind::Ila { .. } => {
                return Err(format!(
                    "pack: ILA {} unsupported on the iCE40 backend",
                    resolve(cell.name)
                ));
            }
            other => {
                return Err(format!(
                    "pack: unsupported CellKind {other:?} for iCE40 (cell {})",
                    resolve(cell.name)
                ));
            }
        }
    }

    // Emit carry chains after all cells are parsed (chains link cells in
    // any order).
    emit_carry_chains(netlist, &carry_cells, &mut out)?;

    // Global-clock re-entry buffers (see below).
    insert_glb_reentry_buffers(&mut out);

    // ClockBufs stay as markers: route.rs assigns each to a global network
    // (source -> global buffer -> glb_netwk -> per-tile lutff_global/clk)
    // instead of merging the net into the fabric.

    // Pair each FF with its driver LUT when the driver is a packed LUT4 with
    // no other loads: they share one logic cell (LUT -> FF in the same LC).
    // (Pairing is recorded at place time; pack just keeps both lists.)

    Ok(out)
}

/// Resolve a top-level port to a CT256 package pin.
fn resolve_pin(netlist: &Design, port: &str) -> Result<String, String> {
    if let Some(def) = netlist.ports.iter().find(|p| p.name == intern(port)) {
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

/// Parse one SB_RAM40_4K `INIT_x` attribute (yosys param semantics: hex
/// string of up to 256 bits, leftmost char = bit 255) into the 16 init
/// words it covers (`INIT_x` bit `p` = flat bit `x*256+p`).
/// `x`/`z` chars count as 0, matching arachne-pnr.
fn parse_init_attr(name: &str, attr: &str, x: usize, init: &mut [u16; 256]) -> Result<(), String> {
    let s = attr.trim();
    if s.len() > 64 {
        return Err(format!(
            "pack: RAM {name}: INIT_{x:X} has {} hex chars, more than 256 bits",
            s.len()
        ));
    }
    // Left-pad with zeros to 256 bits.
    let padded = format!("{:0>64}", s);
    for (ci, ch) in padded.chars().enumerate() {
        let digit = match ch {
            '0'..='9' => ch as u8 - b'0',
            'a'..='f' => ch as u8 - b'a' + 10,
            'A'..='F' => ch as u8 - b'A' + 10,
            'x' | 'X' | 'z' | 'Z' => 0,
            _ => {
                return Err(format!(
                    "pack: RAM {name}: INIT_{x:X} has invalid char '{ch}'"
                ))
            }
        };
        // Char ci (0 = leftmost) covers bits 255-4*ci-3 .. 255-4*ci;
        // digit bit b (0 = LSB) is flat bit 255-4*ci-3+b.
        for b in 0..4 {
            if (digit >> b) & 1 == 1 {
                let p = 255 - 4 * ci - 3 + b;
                let word = x * 16 + p / 16;
                init[word] |= 1 << (p % 16);
            }
        }
    }
    Ok(())
}

/// Parse a `WRITE_MODE` / `READ_MODE` attribute: `0`..`3` or `00`..`11`.
fn parse_ram_mode(name: &str, attr_name: &str, v: &str) -> Result<u8, String> {
    match v.trim() {
        "0" | "00" => Ok(0),
        "1" | "01" => Ok(1),
        "2" | "10" => Ok(2),
        "3" | "11" => Ok(3),
        other => Err(format!(
            "pack: RAM {name}: {attr_name}='{other}' is not a valid mode (0..3)"
        )),
    }
}

/// Pack a Bram18 cell into SB_RAM40_4K shape.
fn pack_ram(
    name: &str,
    pins: &helion_ir::PinIndex,
    attrs: &helion_ir::Attrs,
) -> Result<PackedRam, String> {
    let bus = |prefix: &str, n: usize| -> Result<[String; 16], String> {
        let mut arr: [String; 16] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            if i < n {
                *slot = pins
                    .net_on(name, &format!("{prefix}{i}"))
                    .map(|s| resolve(s).to_string())
                    .unwrap_or("gnd".to_string());
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
                .map(|s| resolve(s).to_string())
                .unwrap_or("gnd".to_string());
        }
        Ok(arr)
    };
    let need = |pin: &str| {
        pins.net_on(name, pin)
            .map(|s| resolve(s).to_string())
            .ok_or_else(|| format!("pack: RAM {name} has no {pin} net"))
    };
    Ok(PackedRam {
        name: name.to_string(),
        write_mode: match attrs.get("WRITE_MODE") {
            Some(v) => parse_ram_mode(name, "WRITE_MODE", v)?,
            None => 0,
        },
        read_mode: match attrs.get("READ_MODE") {
            Some(v) => parse_ram_mode(name, "READ_MODE", v)?,
            None => 0,
        },
        init: {
            let mut init = [0u16; 256];
            for x in 0..16 {
                let attr = format!("INIT_{x:X}");
                if let Some(v) = attrs.get(&attr) {
                    parse_init_attr(name, v, x, &mut init)?;
                }
            }
            init
        },
        rdata_net: bus("DO", 16)?,
        wdata_net: bus("DI", 16)?,
        raddr_net: addr("RADDR")?,
        waddr_net: addr("WADDR")?,
        rclk_net: need("RCLK").unwrap_or_else(|_| "gnd".to_string()),
        wclk_net: need("WCLK").unwrap_or_else(|_| "gnd".to_string()),
        re_net: pins.net_on(name, "RE").map(|s| resolve(s).to_string()),
        we_net: pins.net_on(name, "WE").map(|s| resolve(s).to_string()),
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
    name: helion_ir::Spur,
    cin: CarryIn,
    s: [Option<String>; 4],
    di: [Option<String>; 4],
    o: [Option<String>; 4],
    co: [Option<String>; 4],
}

fn is_const1(net: Option<helion_ir::Spur>) -> bool {
    matches!(net, Some(s) if {
        let r = resolve(s);
        r == "1" || r == "vcc" || r == "VCC" || r == "$true"
    })
}

fn is_const0(net: Option<helion_ir::Spur>) -> bool {
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
    fn resolve(&mut self, net: Option<helion_ir::Spur>, out: &mut PackedIce) -> String {
        match net {
            Some(n) if is_const1(Some(n)) => self.one(out),
            Some(n) if is_const0(Some(n)) => self.zero(out),
            Some(n) => resolve(n).to_string(),
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
                "pack: Carry4 {name}: CI net '{}' is not driven by a Carry4 CO[3]; \
                 iCE40 carry_in supports only constant 0/1 or a chained carry",
                resolve(ci_net)
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
        s[i] = pins.net_on(name, &format!("S{i}")).map(|n| resolve(n).to_string());
        di[i] = pins.net_on(name, &format!("DI{i}")).map(|n| resolve(n).to_string());
        o[i] = pins.net_on(name, &format!("O{i}")).map(|n| resolve(n).to_string());
        co[i] = pins.net_on(name, &format!("CO{i}")).map(|n| resolve(n).to_string());
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
        name: intern(name),
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
        .map(|(i, c)| (resolve(c.name), i))
        .collect();
    // next[i] = cell chained after i via CO[3] -> CI.
    let mut next: Vec<Option<usize>> = vec![None; cells.len()];
    for (i, cell) in cells.iter().enumerate() {
        if let CarryIn::Prev(prev_name) = &cell.cin {
            let &pi = idx.get(prev_name.as_str()).ok_or_else(|| {
                format!(
                    "pack: Carry4 {}: previous cell '{prev_name}' not found",
                    resolve(cell.name)
                )
            })?;
            if next[pi].is_some() {
                return Err(format!(
                    "pack: Carry4 {}: CO[3] fans out to multiple carry chains; \
                     the dedicated carry wire is point-to-point",
                    resolve(cells[pi].name)
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
                .find(|n| n.name == intern(co3))
                .ok_or_else(|| format!("pack: Carry4 {}: CO3 net vanished", resolve(cell.name)))?;
            let allowed_ci = next[i].map(|n| resolve(cells[n].name));
            for e in &net.endpoints {
                if e.cell == cell.name && e.pin == intern("CO3") {
                    continue;
                }
                if e.pin == intern("CI") && Some(resolve(e.cell)) == allowed_ci {
                    continue;
                }
                return Err(format!(
                    "pack: Carry4 {}: CO[3] net '{co3}' drives {}.{} which is not the \
                     chained carry; iCE40 cout is not routable to general fabric",
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
                    "pack: Carry4 {}: carry chain cycle detected",
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
                "pack: Carry4 {}: carry chain has no head (cycle or dangling CI)",
                resolve(cell.name)
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
                let s = cell.s[bit].as_deref().map(|s| intern(s));
                let di = cell.di[bit].as_deref().map(|s| intern(s));
                let s_net = consts.resolve(s, out);
                let di_net = consts.resolve(di, out);
                // in_2 = S[i] ^ DI[i]; constant-fold the trivial cases.
                let x_net = if is_const0(s) {
                    di_net.clone()
                } else if is_const0(di) {
                    s_net.clone()
                } else {
                    let xor_out = format!("{}${bit}$xor", resolve(cell.name));
                    out.luts.push(PackedLut {
                        name: format!("{}${bit}$xor", resolve(cell.name)),
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
                    .unwrap_or_else(|| format!("{}${bit}$o", resolve(cell.name)));
                out.luts.push(PackedLut {
                    name: format!("{}${bit}$sum", resolve(cell.name)),
                    init: 0x55AA, // in_0 ^ in_3
                    input_nets: [
                        Some(s_net),
                        Some(di_net),
                        Some(x_net),
                        Some(format!("{}${bit}$cin", resolve(cell.name))),
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

/// Insert a feedthrough buffer LUT for each global clock net (ClockBuf
/// `out_net`, PLL `out_net`) that has non-clock fabric loads.
///
/// A `glb_netwk` wire only reaches fabric through `glb2local` -> local
/// tracks (which feed LC inputs, never spans or IO pins) and through the
/// per-tile clock buffers (FF `lutff_global/clk`, RAM `RCLK`/`WCLK`).
/// A global clock feeding anything else (LUT inputs, IO outputs, FF D,
/// RAM data/address inputs) therefore needs one shared re-entry buffer:
/// the buffer's input routes from the glb wire, its output drives the
/// original loads. Clock pins keep the global net directly and need no
/// buffer.
fn insert_glb_reentry_buffers(out: &mut PackedIce) {
    let global_nets: Vec<String> = out
        .clks
        .iter()
        .map(|c| c.out_net.clone())
        .chain(out.plls.iter().map(|p| p.out_net.clone()))
        .collect();
    for net in &global_nets {
        let mut loaded = false;
        for lut in &out.luts {
            if lut.input_nets.iter().flatten().any(|n| n == net) {
                loaded = true;
                break;
            }
        }
        if !loaded {
            for io in &out.ios {
                if io.dir == IoDir::Out && io.net == *net {
                    loaded = true;
                    break;
                }
            }
        }
        if !loaded {
            for ff in &out.ffs {
                if ff.d_net == *net {
                    loaded = true;
                    break;
                }
            }
        }
        if !loaded {
            for ram in &out.rams {
                let mut data_addr = ram
                    .wdata_net
                    .iter()
                    .chain(ram.waddr_net.iter())
                    .chain(ram.raddr_net.iter());
                if data_addr.any(|n| n == net)
                    || ram.re_net.as_deref() == Some(net.as_str())
                    || ram.we_net.as_deref() == Some(net.as_str())
                {
                    loaded = true;
                    break;
                }
            }
        }
        if !loaded {
            continue;
        }
        let buf_out = format!("{net}$glb");
        // Rewrite the non-clock loads onto the buffer output (before the
        // buffer exists, so its own input keeps the global net).
        for lut in out.luts.iter_mut() {
            for slot in lut.input_nets.iter_mut() {
                if slot.as_deref() == Some(net.as_str()) {
                    *slot = Some(buf_out.clone());
                }
            }
        }
        for io in out.ios.iter_mut() {
            if io.dir == IoDir::Out && io.net == *net {
                io.net = buf_out.clone();
            }
        }
        for ff in out.ffs.iter_mut() {
            if ff.d_net == *net {
                ff.d_net = buf_out.clone();
            }
        }
        for ram in out.rams.iter_mut() {
            for slot in ram
                .wdata_net
                .iter_mut()
                .chain(ram.waddr_net.iter_mut())
                .chain(ram.raddr_net.iter_mut())
            {
                if slot == net {
                    *slot = buf_out.clone();
                }
            }
            if ram.re_net.as_deref() == Some(net.as_str()) {
                ram.re_net = Some(buf_out.clone());
            }
            if ram.we_net.as_deref() == Some(net.as_str()) {
                ram.we_net = Some(buf_out.clone());
            }
        }
        out.luts.push(PackedLut {
            name: buf_out.clone(),
            init: 0xAAAA, // f(in_0) = in_0
            input_nets: [Some(net.clone()), None, None, None],
            output_net: buf_out,
            carry: None,
        });
    }
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

    #[test]
    fn pll_dividers_match_icepll() {
        // Vectors verified with the real `icepll` tool (SIMPLE feedback).
        // (ref_hz, out_hz, divr, divf, divq, filter_range).
        for (r, o, dr, df, dq, fr) in [
            (12_000_000u64, 48_000_000u64, 0u8, 63u8, 4u8, 1u8),
            (12_000_000, 100_000_000, 0, 66, 3, 1),
            (12_000_000, 60_000_000, 0, 79, 4, 1),
            (25_000_000, 100_000_000, 0, 31, 3, 2),
            (16_000_000, 50_000_000, 0, 49, 4, 1),
            (48_000_000, 120_000_000, 0, 19, 3, 4),
        ] {
            assert_eq!(
                pll_dividers("test", r, o).unwrap(),
                (dr, df, dq, fr),
                "mismatch for {r} -> {o}"
            );
        }
        // Out-of-range frequencies fail loud.
        assert!(pll_dividers("test", 12_000_000, 500_000_000).is_err());
        assert!(pll_dividers("test", 5_000_000, 48_000_000).is_err());
    }

    #[test]
    fn ram_init_attr_parsing() {
        let mut init = [0u16; 256];
        // "1" = bit 0 -> word 0 bit 0.
        parse_init_attr("r", "1", 0, &mut init).unwrap();
        assert_eq!(init[0], 0x0001);
        // Leftmost char = bit 255.
        parse_init_attr(
            "r",
            "8000000000000000000000000000000000000000000000000000000000000000",
            3,
            &mut init,
        )
        .unwrap();
        assert_eq!(init[3 * 16 + 15], 0x8000);
        // x chars count as 0.
        parse_init_attr("r", "x", 5, &mut init).unwrap();
        assert_eq!(init[5 * 16], 0x0000);
        // Too long / bad char fail loud.
        assert!(parse_init_attr("r", &"0".repeat(65), 0, &mut init).is_err());
        assert!(parse_init_attr("r", "qq", 0, &mut init).is_err());
        // Mode parsing.
        assert_eq!(parse_ram_mode("r", "WRITE_MODE", "01").unwrap(), 1);
        assert_eq!(parse_ram_mode("r", "READ_MODE", "3").unwrap(), 3);
        assert!(parse_ram_mode("r", "READ_MODE", "4").is_err());
    }
}
