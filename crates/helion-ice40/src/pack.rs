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
    /// 16-bit write bit-mask nets (`MASK0`..`MASK15`, active when `write_mode == 0`).
    pub mask_net: [String; 16],
    /// Negative-edge read clock (`SB_RAM40_4KNR` / `SB_RAM40_4KNRNW` or `NEG_CLK_R=1`).
    pub neg_clk_r: bool,
    /// Negative-edge write clock (`SB_RAM40_4KNW` / `SB_RAM40_4KNRNW` or `NEG_CLK_W=1`).
    pub neg_clk_w: bool,
}

/// Feedback path selection for `SB_PLL40_*` primitives (`FEEDBACK_PATH` parameter).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PllFeedbackPath {
    /// `"DELAY"` (`3'b000`).
    Delay,
    /// `"SIMPLE"` (`3'b001`).
    #[default]
    Simple,
    /// `"PHASE_AND_DELAY"` (`3'b010`).
    PhaseAndDelay,
    /// `"EXTERNAL"` (`3'b110`).
    External,
}

impl PllFeedbackPath {
    /// 3-bit `FEEDBACK_PATH` field value in IceStorm `PLLCONFIG`.
    pub const fn code(self) -> u8 {
        match self {
            Self::Delay => 0b000,
            Self::Simple => 0b001,
            Self::PhaseAndDelay => 0b010,
            Self::External => 0b110,
        }
    }
}

/// A packed `SB_PLL40_*` primitive.
#[derive(Clone, Debug)]
pub struct PackedPll {
    pub name: String,
    pub kind: PllKind,
    pub ref_net: String,
    pub out_net: String,
    /// Secondary output net (`PLLOUTGLOBALB` / `PLLOUTCOREB`) for `2F` / `2_PAD` PLLs.
    pub out_net_b: Option<String>,
    /// Optional `LOCK` status output net.
    pub lock_net: Option<String>,
    /// Desired output frequency in Hz (from the `FREQ` cell attribute, or computed from dividers).
    pub out_freq_hz: u64,
    /// Reference clock frequency in Hz (from the `REF_FREQ` cell attribute).
    pub ref_freq_hz: u64,
    /// Divider settings (`DIVR`, `DIVF`, `DIVQ`, `FILTER_RANGE`) and `FEEDBACK_PATH`.
    pub divr: u8,
    pub divf: u8,
    pub divq: u8,
    pub filter_range: u8,
    pub feedback_path: PllFeedbackPath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PllKind {
    /// `SB_PLL40_PAD` (`PLLTYPE = 3'b010`).
    Pad,
    /// `SB_PLL40_CORE` (`PLLTYPE = 3'b011`).
    Core,
    /// `SB_PLL40_2_PAD` (`PLLTYPE = 3'b100`).
    Pad2,
    /// `SB_PLL40_2F_PAD` (`PLLTYPE = 3'b110`).
    Pad2F,
    /// `SB_PLL40_2F_CORE` (`PLLTYPE = 3'b111`).
    Core2F,
}

impl PllKind {
    /// 3-bit `PLLTYPE` field value in IceStorm `PLLCONFIG`.
    pub const fn plltype_code(self) -> u8 {
        match self {
            Self::Pad => 0b010,
            Self::Core => 0b011,
            Self::Pad2 => 0b100,
            Self::Pad2F => 0b110,
            Self::Core2F => 0b111,
        }
    }

    /// True if this PLL takes its reference clock from a package pad (`PACKAGEPIN`).
    pub const fn is_pad(self) -> bool {
        matches!(self, Self::Pad | Self::Pad2 | Self::Pad2F)
    }
}

/// A packed `SB_MAC16` 16x16 multiplier + 32-bit accumulator DSP block (iCE40 UltraPlus).
#[derive(Clone, Debug)]
pub struct PackedMac16 {
    pub name: String,
    pub c_reg: bool,
    pub a_reg: bool,
    pub b_reg: bool,
    pub d_reg: bool,
    pub top_8x8_mult_reg: bool,
    pub bot_8x8_mult_reg: bool,
    pub pipeline_16x16_mult_reg1: bool,
    pub pipeline_16x16_mult_reg2: bool,
    pub topoutput_select: u8,
    pub topaddsub_lowerinput: u8,
    pub topaddsub_upperinput: bool,
    pub topaddsub_carryselect: u8,
    pub botoutput_select: u8,
    pub botaddsub_lowerinput: u8,
    pub botaddsub_upperinput: bool,
    pub botaddsub_carryselect: u8,
    pub mode_8x8: bool,
    pub a_signed: bool,
    pub b_signed: bool,
    pub neg_trigger: bool,
    /// 25-bit packed IceStorm `Mac16Config` / `IpConfig` configuration word (`C_REG`..`B_SIGNED`).
    pub config_bits_25: u32,
    pub clk_net: Option<String>,
    pub ce_net: Option<String>,
    pub a_nets: [Option<String>; 16],
    pub b_nets: [Option<String>; 16],
    pub c_nets: [Option<String>; 16],
    pub d_nets: [Option<String>; 16],
    pub o_nets: [Option<String>; 32],
    pub co_net: Option<String>,
    pub acc_ovf_net: Option<String>,
}

/// A packed `SB_SPRAM256KA` 256Kb (16K x 16-bit) Single-Port RAM block (iCE40 UltraPlus).
#[derive(Clone, Debug)]
pub struct PackedSpram {
    pub name: String,
    pub addr_nets: [Option<String>; 14],
    pub data_in_nets: [Option<String>; 16],
    pub mask_wren_nets: [Option<String>; 4],
    pub wren_net: Option<String>,
    pub chipselect_net: Option<String>,
    pub clock_net: Option<String>,
    pub standby_net: Option<String>,
    pub sleep_net: Option<String>,
    pub poweroff_net: Option<String>,
    pub data_out_nets: [Option<String>; 16],
}

/// Oscillator kind (`SB_HFOSC` 48 MHz or `SB_LFOSC` 10 kHz).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OscKind {
    HfOsc,
    LfOsc,
}

/// A packed internal oscillator (`SB_HFOSC` / `SB_LFOSC`).
#[derive(Clone, Debug)]
pub struct PackedOsc {
    pub name: String,
    pub kind: OscKind,
    /// `CLKHF_DIV` divider setting (`0` = 48 MHz, `1` = 24 MHz, `2` = 12 MHz, `3` = 6 MHz).
    pub clk_hf_div: u8,
    pub pu_net: Option<String>,
    pub en_net: Option<String>,
    pub out_net: String,
}

/// Kind of hard peripheral IP block (`SB_I2C`, `SB_SPI`, `SB_RGBA_DRV`, `SB_LEDDA_IP`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HardIpKind {
    /// Hard I2C controller (`BUS_ADDR74` 4-bit address prefix).
    I2c { bus_addr74: u8 },
    /// Hard SPI controller (`BUS_ADDR74` 4-bit address prefix).
    Spi { bus_addr74: u8 },
    /// High-current RGB LED driver (`CURRENT_MODE` and 6-bit `RGB0..2_CURRENT` settings).
    RgbaDrv {
        current_mode: u8,
        rgb0_current: u8,
        rgb1_current: u8,
        rgb2_current: u8,
    },
    /// Hard LED PWM fade/breathe generator (`SB_LEDDA_IP`).
    LeddaIp,
}

/// A packed hard IP block (`SB_I2C`, `SB_SPI`, `SB_RGBA_DRV`, `SB_LEDDA_IP`).
#[derive(Clone, Debug)]
pub struct PackedHardIp {
    pub name: String,
    pub kind: HardIpKind,
    pub input_nets: Vec<(String, String)>,
    pub output_nets: Vec<(String, String)>,
}

/// Backend-owned packed netlist for the iCE40 HX8K / UltraPlus.
#[derive(Clone, Debug, Default)]
pub struct PackedIce {
    pub luts: Vec<PackedLut>,
    pub ffs: Vec<PackedFf>,
    pub ios: Vec<PackedIo>,
    pub clks: Vec<PackedClk>,
    pub rams: Vec<PackedRam>,
    pub plls: Vec<PackedPll>,
    pub mac16s: Vec<PackedMac16>,
    pub sprams: Vec<PackedSpram>,
    pub oscs: Vec<PackedOsc>,
    pub hard_ips: Vec<PackedHardIp>,
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
                out.rams
                    .push(pack_ram(resolve(cell.name), "SB_RAM40_4K", &pins, &cell.attrs)?);
            }
            CellKind::BlackBox { module } if resolve(*module).starts_with("SB_RAM40_4K") => {
                out.rams.push(pack_ram(
                    resolve(cell.name),
                    resolve(*module),
                    &pins,
                    &cell.attrs,
                )?);
            }
            CellKind::BlackBox { module } if resolve(*module).starts_with("SB_PLL40") => {
                out.plls.push(pack_pll(
                    resolve(cell.name),
                    resolve(*module),
                    &pins,
                    &cell.attrs,
                )?);
            }
            CellKind::BlackBox { module } if resolve(*module) == "SB_GB" => {
                let cname = resolve(cell.name);
                let in_net = pins
                    .net_on(cname, "USER_SIGNAL_TO_GLOBAL_BUFFER")
                    .or_else(|| pins.net_on(cname, "I"))
                    .map(|s| resolve(s).to_string())
                    .ok_or_else(|| format!("pack: SB_GB {cname} has no input net"))?;
                let out_net = pins
                    .net_on(cname, "GLOBAL_BUFFER_OUTPUT")
                    .or_else(|| pins.net_on(cname, "O"))
                    .map(|s| resolve(s).to_string())
                    .ok_or_else(|| format!("pack: SB_GB {cname} has no output net"))?;
                out.clks.push(PackedClk {
                    name: cname.to_string(),
                    in_net,
                    out_net,
                });
            }
            CellKind::BlackBox { module } if resolve(*module) == "SB_GB_IO" => {
                let cname = resolve(cell.name);
                let port = pins
                    .net_on(cname, "PACKAGE_PIN")
                    .or_else(|| pins.net_on(cname, "PAD"))
                    .map(|s| resolve(s).to_string())
                    .unwrap_or_else(|| cname.to_string());
                let out_net = pins
                    .net_on(cname, "GLOBAL_BUFFER_OUTPUT")
                    .or_else(|| pins.net_on(cname, "O"))
                    .map(|s| resolve(s).to_string())
                    .ok_or_else(|| format!("pack: SB_GB_IO {cname} has no GLOBAL_BUFFER_OUTPUT net"))?;
                let pad_net = pins
                    .net_on(cname, "D_IN_0")
                    .map(|s| resolve(s).to_string())
                    .unwrap_or_else(|| format!("{cname}$gb_io_in"));
                let pin = cell
                    .attrs
                    .get("LOC")
                    .map(ToString::to_string)
                    .unwrap_or_else(|| resolve_pin(netlist, &port).unwrap_or_else(|_| "J3".to_string()));
                out.ios.push(PackedIo {
                    name: format!("{cname}$io"),
                    port,
                    dir: IoDir::In,
                    net: pad_net.clone(),
                    pin,
                });
                out.clks.push(PackedClk {
                    name: cname.to_string(),
                    in_net: pad_net,
                    out_net,
                });
            }
            CellKind::Mac27 | CellKind::Dsp48e1 { .. } => {
                out.mac16s
                    .push(pack_mac16(resolve(cell.name), &pins, &cell.attrs)?);
            }
            CellKind::BlackBox { module } if resolve(*module) == "SB_MAC16" => {
                out.mac16s
                    .push(pack_mac16(resolve(cell.name), &pins, &cell.attrs)?);
            }
            CellKind::BlackBox { module } if resolve(*module) == "SB_SPRAM256KA" => {
                out.sprams
                    .push(pack_spram(resolve(cell.name), &pins)?);
            }
            CellKind::BlackBox { module }
                if matches!(resolve(*module), "SB_HFOSC" | "SB_LFOSC") =>
            {
                out.oscs.push(pack_osc(
                    resolve(cell.name),
                    resolve(*module),
                    &pins,
                    &cell.attrs,
                )?);
            }
            CellKind::BlackBox { module }
                if matches!(
                    resolve(*module),
                    "SB_I2C" | "SB_SPI" | "SB_RGBA_DRV" | "SB_LEDDA_IP"
                ) =>
            {
                out.hard_ips.push(pack_hard_ip(
                    resolve(cell.name),
                    resolve(*module),
                    netlist,
                    &cell.attrs,
                )?);
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
    let raw = attr.trim();
    let s = if let Some((_, hex)) = raw.split_once("'h").or_else(|| raw.split_once("'H")) {
        hex.trim()
    } else if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        hex.trim()
    } else {
        raw
    };
    let cleaned: String = s.chars().filter(|&c| c != '_').collect();
    if cleaned.len() > 64 {
        return Err(format!(
            "pack: RAM {name}: INIT_{x:X} has {} hex chars, more than 256 bits",
            cleaned.len()
        ));
    }
    // Left-pad with zeros to 256 bits.
    let padded = format!("{:0>64}", cleaned);
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

/// Parse a `WRITE_MODE` / `READ_MODE` attribute: `0`..`3`, `00`..`11`, or `2'b00`..`2'b11`.
fn parse_ram_mode(name: &str, attr_name: &str, v: &str) -> Result<u8, String> {
    let s = v.trim();
    let s = s
        .strip_prefix("2'b")
        .or_else(|| s.strip_prefix("0b"))
        .unwrap_or(s);
    match s {
        "0" | "00" => Ok(0),
        "1" | "01" => Ok(1),
        "2" | "10" => Ok(2),
        "3" | "11" => Ok(3),
        other => Err(format!(
            "pack: RAM {name}: {attr_name}='{other}' is not a valid mode (0..3)"
        )),
    }
}

/// Look up an indexed pin on `cell` supporting `PREFIX{i}` and `PREFIX[{i}]`.
fn pin_net_indexed(
    pins: &helion_ir::PinIndex,
    cell: &str,
    prefixes: &[&str],
    idx: usize,
) -> Option<String> {
    for prefix in prefixes {
        if let Some(s) = pins
            .net_on(cell, &format!("{prefix}{idx}"))
            .or_else(|| pins.net_on(cell, &format!("{prefix}[{idx}]")))
        {
            return Some(resolve(s).to_string());
        }
    }
    None
}

/// Parse an integer/bitstring attribute (`"1"`, `"0b10"`, `"4'b0101"`, `"16'h00FF"`).
fn parse_bit_param(cell_name: &str, param_name: &str, raw: &str, max_val: u32) -> Result<u32, String> {
    let s = raw.trim().trim_matches('"');
    let cleaned: String = s.chars().filter(|&c| c != '_').collect();
    let val = if let Some((_, bin)) = cleaned.split_once("'b").or_else(|| cleaned.split_once("'B")) {
        u32::from_str_radix(bin, 2)
    } else if let Some(bin) = cleaned.strip_prefix("0b").or_else(|| cleaned.strip_prefix("0B")) {
        u32::from_str_radix(bin, 2)
    } else if let Some((_, hex)) = cleaned.split_once("'h").or_else(|| cleaned.split_once("'H")) {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = cleaned.strip_prefix("0x").or_else(|| cleaned.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else if let Some((_, dec)) = cleaned.split_once("'d").or_else(|| cleaned.split_once("'D")) {
        dec.parse::<u32>()
    } else {
        cleaned.parse::<u32>()
    }
    .map_err(|_| format!("pack: {cell_name}: invalid parameter {param_name}='{raw}'"))?;
    if val > max_val {
        return Err(format!(
            "pack: {cell_name}: parameter {param_name}={val} exceeds max {max_val}"
        ));
    }
    Ok(val)
}

/// Pack a `Bram18` or `SB_RAM40_4K*` cell into [`PackedRam`].
fn pack_ram(
    name: &str,
    module: &str,
    pins: &helion_ir::PinIndex,
    attrs: &helion_ir::Attrs,
) -> Result<PackedRam, String> {
    let bus16 = |prefixes: &[&str]| -> [String; 16] {
        let mut arr: [String; 16] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            *slot = pin_net_indexed(pins, name, prefixes, i).unwrap_or_else(|| "gnd".to_string());
        }
        arr
    };
    let addr11 = |prefixes: &[&str]| -> [String; 11] {
        let mut arr: [String; 11] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            *slot = pin_net_indexed(pins, name, prefixes, i).unwrap_or_else(|| "gnd".to_string());
        }
        arr
    };
    let rclk_net = pins
        .net_on(name, "RCLK")
        .or_else(|| pins.net_on(name, "RCLKN"))
        .or_else(|| pins.net_on(name, "CLKARDCLK"))
        .map(|s| resolve(s).to_string())
        .unwrap_or_else(|| "gnd".to_string());
    let wclk_net = pins
        .net_on(name, "WCLK")
        .or_else(|| pins.net_on(name, "WCLKN"))
        .or_else(|| pins.net_on(name, "CLKBWRCLK"))
        .map(|s| resolve(s).to_string())
        .unwrap_or_else(|| "gnd".to_string());
    let re_net = pins
        .net_on(name, "RE")
        .or_else(|| pins.net_on(name, "RCLKE"))
        .map(|s| resolve(s).to_string());
    let we_net = pins
        .net_on(name, "WE")
        .or_else(|| pins.net_on(name, "WCLKE"))
        .map(|s| resolve(s).to_string());
    let neg_clk_r = module.contains("NR")
        || pins.net_on(name, "RCLKN").is_some()
        || attrs.flag("NEG_CLK_R");
    let neg_clk_w = module.contains("NW")
        || pins.net_on(name, "WCLKN").is_some()
        || attrs.flag("NEG_CLK_W");
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
        rdata_net: bus16(&["DO", "RDATA", "DOADO"]),
        wdata_net: bus16(&["DI", "WDATA", "DIADI"]),
        raddr_net: addr11(&["RADDR", "ADDRARDADDR"]),
        waddr_net: addr11(&["WADDR", "ADDRBWRADDR"]),
        rclk_net,
        wclk_net,
        re_net,
        we_net,
        mask_net: bus16(&["MASK"]),
        neg_clk_r,
        neg_clk_w,
    })
}

/// Pack an `SB_PLL40_*` cell (`SB_PLL40_CORE`, `SB_PLL40_PAD`, `SB_PLL40_2F_CORE`,
/// `SB_PLL40_2F_PAD`, `SB_PLL40_2_PAD`), supporting both automatic `FREQ` + `REF_FREQ`
/// divider synthesis and explicit `DIVR` / `DIVF` / `DIVQ` / `FILTER_RANGE` / `FEEDBACK_PATH`
/// parameters with VCO range validation.
fn pack_pll(
    name: &str,
    module: &str,
    pins: &helion_ir::PinIndex,
    attrs: &helion_ir::Attrs,
) -> Result<PackedPll, String> {
    let kind = match module {
        "SB_PLL40_2F_CORE" => PllKind::Core2F,
        "SB_PLL40_2F_PAD" => PllKind::Pad2F,
        "SB_PLL40_2_PAD" => PllKind::Pad2,
        m if m.contains("PAD") => PllKind::Pad,
        _ => PllKind::Core,
    };
    let ref_net = pins
        .net_on(name, "REFERENCECLK")
        .or_else(|| pins.net_on(name, "PACKAGEPIN"))
        .or_else(|| pins.net_on(name, "I"))
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: PLL {name} has no clock input"))?;
    let out_net = pins
        .net_on(name, "PLLOUTGLOBAL")
        .or_else(|| pins.net_on(name, "PLLOUTCORE"))
        .or_else(|| pins.net_on(name, "PLLOUTGLOBALA"))
        .or_else(|| pins.net_on(name, "PLLOUTCOREA"))
        .or_else(|| pins.net_on(name, "O"))
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: PLL {name} has no output"))?;
    let out_net_b = pins
        .net_on(name, "PLLOUTGLOBALB")
        .or_else(|| pins.net_on(name, "PLLOUTCOREB"))
        .map(|s| resolve(s).to_string());
    let lock_net = pins.net_on(name, "LOCK").map(|s| resolve(s).to_string());

    let feedback_path = match attrs.get("FEEDBACK_PATH").map(|s| s.trim().trim_matches('"')) {
        None | Some("SIMPLE") | Some("0b001") | Some("1") => PllFeedbackPath::Simple,
        Some("DELAY") | Some("0b000") | Some("0") => PllFeedbackPath::Delay,
        Some("PHASE_AND_DELAY") | Some("0b010") | Some("2") => PllFeedbackPath::PhaseAndDelay,
        Some("EXTERNAL") | Some("0b110") | Some("6") => PllFeedbackPath::External,
        Some(other) => {
            return Err(format!(
                "pack: PLL {name}: invalid FEEDBACK_PATH='{other}' (expected SIMPLE, DELAY, PHASE_AND_DELAY, or EXTERNAL)"
            ))
        }
    };

    let has_explicit_divs =
        attrs.get("DIVR").is_some() || attrs.get("DIVF").is_some() || attrs.get("DIVQ").is_some();
    if has_explicit_divs && attrs.get("FREQ").is_none() {
        let divr = match attrs.get("DIVR") {
            Some(v) => parse_bit_param(name, "DIVR", v, 15)? as u8,
            None => 0,
        };
        let divf = match attrs.get("DIVF") {
            Some(v) => parse_bit_param(name, "DIVF", v, 127)? as u8,
            None => return Err(format!("pack: PLL {name}: explicit divider mode requires DIVF")),
        };
        let divq = match attrs.get("DIVQ") {
            Some(v) => parse_bit_param(name, "DIVQ", v, 6)? as u8,
            None => return Err(format!("pack: PLL {name}: explicit divider mode requires DIVQ")),
        };
        if !(1..=6).contains(&divq) {
            return Err(format!(
                "pack: PLL {name}: DIVQ={divq} is outside valid range 1..=6"
            ));
        }
        let filter_range = match attrs.get("FILTER_RANGE") {
            Some(v) => parse_bit_param(name, "FILTER_RANGE", v, 7)? as u8,
            None => 1,
        };
        let ref_freq_hz = match attrs.get("REF_FREQ") {
            Some(v) => parse_pll_freq(name, "REF_FREQ", v)?,
            None => 12_000_000,
        };
        // Validate VCO and PFD frequencies against iCE40 PLL hardware limits.
        let f_in_mhz = ref_freq_hz as f64 / 1e6;
        let f_pfd_mhz = f_in_mhz / f64::from(divr + 1);
        if !(10.0..=133.0).contains(&f_pfd_mhz) {
            return Err(format!(
                "pack: PLL {name}: PFD frequency {f_pfd_mhz:.3} MHz is outside 10-133 MHz"
            ));
        }
        let f_vco_mhz = match feedback_path {
            PllFeedbackPath::Simple => f_pfd_mhz * f64::from(divf + 1),
            _ => f_pfd_mhz * f64::from(divf + 1) * 2f64.powi(i32::from(divq)),
        };
        if !(533.0..=1066.0).contains(&f_vco_mhz) {
            return Err(format!(
                "pack: PLL {name}: VCO frequency {f_vco_mhz:.3} MHz is outside 533-1066 MHz"
            ));
        }
        let out_freq_hz = ((f_vco_mhz / 2f64.powi(i32::from(divq))) * 1e6).round() as u64;
        Ok(PackedPll {
            name: name.to_string(),
            kind,
            ref_net,
            out_net,
            out_net_b,
            lock_net,
            out_freq_hz,
            ref_freq_hz,
            divr,
            divf,
            divq,
            filter_range,
            feedback_path,
        })
    } else {
        let out_freq_hz = match attrs.get("FREQ") {
            Some(v) => parse_pll_freq(name, "FREQ", v)?,
            None => {
                return Err(format!(
                    "pack: PLL {name} needs a FREQ attribute (desired output frequency in Hz)"
                ))
            }
        };
        let ref_freq_hz = match attrs.get("REF_FREQ") {
            Some(v) => parse_pll_freq(name, "REF_FREQ", v)?,
            None => {
                return Err(format!(
                    "pack: PLL {name} needs a REF_FREQ attribute (reference clock frequency in Hz)"
                ))
            }
        };
        let (divr, divf, divq, filter_range) = pll_dividers(name, ref_freq_hz, out_freq_hz)?;
        Ok(PackedPll {
            name: name.to_string(),
            kind,
            ref_net,
            out_net,
            out_net_b,
            lock_net,
            out_freq_hz,
            ref_freq_hz,
            divr,
            divf,
            divq,
            filter_range,
            feedback_path,
        })
    }
}

/// Pack an `SB_MAC16` (or `CellKind::Mac27` / `CellKind::Dsp48e1`) DSP cell.
fn pack_mac16(
    name: &str,
    pins: &helion_ir::PinIndex,
    attrs: &helion_ir::Attrs,
) -> Result<PackedMac16, String> {
    let get_param = |key: &str, max_v: u32| -> Result<u32, String> {
        match attrs.get(key) {
            Some(v) => parse_bit_param(name, key, v, max_v),
            None => Ok(0),
        }
    };
    let c_reg = get_param("C_REG", 1)? != 0;
    let a_reg = get_param("A_REG", 1)? != 0;
    let b_reg = get_param("B_REG", 1)? != 0;
    let d_reg = get_param("D_REG", 1)? != 0;
    let top_8x8_mult_reg = get_param("TOP_8x8_MULT_REG", 1)? != 0;
    let bot_8x8_mult_reg = get_param("BOT_8x8_MULT_REG", 1)? != 0;
    let pipeline_16x16_mult_reg1 = get_param("PIPELINE_16x16_MULT_REG1", 1)? != 0;
    let pipeline_16x16_mult_reg2 = get_param("PIPELINE_16x16_MULT_REG2", 1)? != 0;
    let topoutput_select = get_param("TOPOUTPUT_SELECT", 3)? as u8;
    let topaddsub_lowerinput = get_param("TOPADDSUB_LOWERINPUT", 3)? as u8;
    let topaddsub_upperinput = get_param("TOPADDSUB_UPPERINPUT", 1)? != 0;
    let topaddsub_carryselect = get_param("TOPADDSUB_CARRYSELECT", 3)? as u8;
    let botoutput_select = get_param("BOTOUTPUT_SELECT", 3)? as u8;
    let botaddsub_lowerinput = get_param("BOTADDSUB_LOWERINPUT", 3)? as u8;
    let botaddsub_upperinput = get_param("BOTADDSUB_UPPERINPUT", 1)? != 0;
    let botaddsub_carryselect = get_param("BOTADDSUB_CARRYSELECT", 3)? as u8;
    let mode_8x8 = get_param("MODE_8x8", 1)? != 0;
    let a_signed = get_param("A_SIGNED", 1)? != 0;
    let b_signed = get_param("B_SIGNED", 1)? != 0;
    let neg_trigger = get_param("NEG_TRIGGER", 1)? != 0;

    let config_bits_25 = (u32::from(c_reg))
        | (u32::from(a_reg) << 1)
        | (u32::from(b_reg) << 2)
        | (u32::from(d_reg) << 3)
        | (u32::from(top_8x8_mult_reg) << 4)
        | (u32::from(bot_8x8_mult_reg) << 5)
        | (u32::from(pipeline_16x16_mult_reg1) << 6)
        | (u32::from(pipeline_16x16_mult_reg2) << 7)
        | (u32::from(topoutput_select & 0b11) << 8)
        | (u32::from(topaddsub_lowerinput & 0b11) << 10)
        | (u32::from(topaddsub_upperinput) << 12)
        | (u32::from(topaddsub_carryselect & 0b11) << 13)
        | (u32::from(botoutput_select & 0b11) << 15)
        | (u32::from(botaddsub_lowerinput & 0b11) << 17)
        | (u32::from(botaddsub_upperinput) << 19)
        | (u32::from(botaddsub_carryselect & 0b11) << 20)
        | (u32::from(mode_8x8) << 22)
        | (u32::from(a_signed) << 23)
        | (u32::from(b_signed) << 24);

    let bus16 = |prefixes: &[&str]| -> [Option<String>; 16] {
        let mut arr: [Option<String>; 16] = Default::default();
        for (i, slot) in arr.iter_mut().enumerate() {
            *slot = pin_net_indexed(pins, name, prefixes, i);
        }
        arr
    };
    let mut o_nets: [Option<String>; 32] = Default::default();
    for (i, slot) in o_nets.iter_mut().enumerate() {
        *slot = pin_net_indexed(pins, name, &["O", "P"], i);
    }

    Ok(PackedMac16 {
        name: name.to_string(),
        c_reg,
        a_reg,
        b_reg,
        d_reg,
        top_8x8_mult_reg,
        bot_8x8_mult_reg,
        pipeline_16x16_mult_reg1,
        pipeline_16x16_mult_reg2,
        topoutput_select,
        topaddsub_lowerinput,
        topaddsub_upperinput,
        topaddsub_carryselect,
        botoutput_select,
        botaddsub_lowerinput,
        botaddsub_upperinput,
        botaddsub_carryselect,
        mode_8x8,
        a_signed,
        b_signed,
        neg_trigger,
        config_bits_25,
        clk_net: pins.net_on(name, "CLK").map(|s| resolve(s).to_string()),
        ce_net: pins.net_on(name, "CE").map(|s| resolve(s).to_string()),
        a_nets: bus16(&["A"]),
        b_nets: bus16(&["B"]),
        c_nets: bus16(&["C"]),
        d_nets: bus16(&["D"]),
        o_nets,
        co_net: pins.net_on(name, "CO").map(|s| resolve(s).to_string()),
        acc_ovf_net: pins.net_on(name, "ACCOVF").map(|s| resolve(s).to_string()),
    })
}

/// Pack an `SB_SPRAM256KA` 16K x 16-bit Single-Port RAM cell.
fn pack_spram(name: &str, pins: &helion_ir::PinIndex) -> Result<PackedSpram, String> {
    let mut addr_nets: [Option<String>; 14] = Default::default();
    for (i, slot) in addr_nets.iter_mut().enumerate() {
        *slot = pin_net_indexed(pins, name, &["ADDRESS", "ADDR"], i);
    }
    let mut data_in_nets: [Option<String>; 16] = Default::default();
    let mut data_out_nets: [Option<String>; 16] = Default::default();
    for i in 0..16 {
        data_in_nets[i] = pin_net_indexed(pins, name, &["DATAIN", "DI"], i);
        data_out_nets[i] = pin_net_indexed(pins, name, &["DATAOUT", "DO"], i);
    }
    let mut mask_wren_nets: [Option<String>; 4] = Default::default();
    for (i, slot) in mask_wren_nets.iter_mut().enumerate() {
        *slot = pin_net_indexed(pins, name, &["MASKWREN"], i);
    }
    Ok(PackedSpram {
        name: name.to_string(),
        addr_nets,
        data_in_nets,
        mask_wren_nets,
        wren_net: pins.net_on(name, "WREN").map(|s| resolve(s).to_string()),
        chipselect_net: pins
            .net_on(name, "CHIPSELECT")
            .map(|s| resolve(s).to_string()),
        clock_net: pins
            .net_on(name, "CLOCK")
            .or_else(|| pins.net_on(name, "CLK"))
            .map(|s| resolve(s).to_string()),
        standby_net: pins.net_on(name, "STANDBY").map(|s| resolve(s).to_string()),
        sleep_net: pins.net_on(name, "SLEEP").map(|s| resolve(s).to_string()),
        poweroff_net: pins
            .net_on(name, "POWEROFF")
            .map(|s| resolve(s).to_string()),
        data_out_nets,
    })
}

/// Pack an `SB_HFOSC` or `SB_LFOSC` oscillator cell.
fn pack_osc(
    name: &str,
    module: &str,
    pins: &helion_ir::PinIndex,
    attrs: &helion_ir::Attrs,
) -> Result<PackedOsc, String> {
    let kind = if module == "SB_LFOSC" {
        OscKind::LfOsc
    } else {
        OscKind::HfOsc
    };
    let clk_hf_div = match attrs.get("CLKHF_DIV") {
        Some(v) => parse_bit_param(name, "CLKHF_DIV", v, 3)? as u8,
        None => 0,
    };
    let out_net = pins
        .net_on(name, "CLKHF")
        .or_else(|| pins.net_on(name, "CLKLF"))
        .or_else(|| pins.net_on(name, "O"))
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {module} {name} has no output clock net (CLKHF/CLKLF)"))?;
    let pu_net = pins
        .net_on(name, "CLKHFPU")
        .or_else(|| pins.net_on(name, "CLKLFPU"))
        .map(|s| resolve(s).to_string());
    let en_net = pins
        .net_on(name, "CLKHFEN")
        .or_else(|| pins.net_on(name, "CLKLFEN"))
        .map(|s| resolve(s).to_string());
    Ok(PackedOsc {
        name: name.to_string(),
        kind,
        clk_hf_div,
        pu_net,
        en_net,
        out_net,
    })
}

/// Pack a hard peripheral IP cell (`SB_I2C`, `SB_SPI`, `SB_RGBA_DRV`, `SB_LEDDA_IP`).
fn pack_hard_ip(
    name: &str,
    module: &str,
    netlist: &Design,
    attrs: &helion_ir::Attrs,
) -> Result<PackedHardIp, String> {
    let kind = match module {
        "SB_I2C" => {
            let bus_addr74 = match attrs.get("BUS_ADDR74") {
                Some(v) => parse_bit_param(name, "BUS_ADDR74", v, 0b1111)? as u8,
                None => 0b0001,
            };
            HardIpKind::I2c { bus_addr74 }
        }
        "SB_SPI" => {
            let bus_addr74 = match attrs.get("BUS_ADDR74") {
                Some(v) => parse_bit_param(name, "BUS_ADDR74", v, 0b1111)? as u8,
                None => 0b0000,
            };
            HardIpKind::Spi { bus_addr74 }
        }
        "SB_RGBA_DRV" => {
            let current_mode = match attrs.get("CURRENT_MODE") {
                Some(v) => parse_bit_param(name, "CURRENT_MODE", v, 1)? as u8,
                None => 0,
            };
            let rgb0_current = match attrs.get("RGB0_CURRENT") {
                Some(v) => parse_bit_param(name, "RGB0_CURRENT", v, 0x3F)? as u8,
                None => 0,
            };
            let rgb1_current = match attrs.get("RGB1_CURRENT") {
                Some(v) => parse_bit_param(name, "RGB1_CURRENT", v, 0x3F)? as u8,
                None => 0,
            };
            let rgb2_current = match attrs.get("RGB2_CURRENT") {
                Some(v) => parse_bit_param(name, "RGB2_CURRENT", v, 0x3F)? as u8,
                None => 0,
            };
            HardIpKind::RgbaDrv {
                current_mode,
                rgb0_current,
                rgb1_current,
                rgb2_current,
            }
        }
        _ => HardIpKind::LeddaIp,
    };

    let is_output_pin = |pin: &str| -> bool {
        pin.starts_with("SBDATO")
            || pin == "SBACKO"
            || pin == "I2CIRQ"
            || pin == "I2CWKUP"
            || pin == "SCLO"
            || pin == "SCLOE"
            || pin == "SDAO"
            || pin == "SDAOE"
            || pin == "SPIIRQ"
            || pin == "SPIWKUP"
            || pin == "SO"
            || pin == "SOE"
            || pin == "MO"
            || pin == "MOE"
            || pin == "SCKO"
            || pin == "SCKOE"
            || pin.starts_with("MCSNO")
            || pin == "RGB0"
            || pin == "RGB1"
            || pin == "RGB2"
            || pin.starts_with("PWMOUT")
            || pin == "LEDDON"
    };

    let mut input_nets = Vec::new();
    let mut output_nets = Vec::new();
    let cell_spur = intern(name);
    for net in &netlist.nets {
        let nname = resolve(net.name);
        if nname == "0" || nname == "1" || nname == "gnd" || nname == "vcc" {
            continue;
        }
        for ep in &net.endpoints {
            if ep.cell == cell_spur {
                let pname = resolve(ep.pin).to_string();
                if is_output_pin(&pname) {
                    output_nets.push((pname, nname.to_string()));
                } else {
                    input_nets.push((pname, nname.to_string()));
                }
            }
        }
    }
    input_nets.sort();
    output_nets.sort();

    Ok(PackedHardIp {
        name: name.to_string(),
        kind,
        input_nets,
        output_nets,
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
