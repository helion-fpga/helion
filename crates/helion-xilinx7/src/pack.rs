//! Pack stage: [`helion_ir::Design`] → [`Packed7`].
//!
//! Groups logical IR cells and UG953 hardware primitives into 7-series
//! packable units and inserts the clocking spine (IBUF + BUFGCTRL) for
//! top-level clock inputs.

use helion_ir::{intern, resolve, Attrs, CellKind, Design, PinIndex, PortDir};
use std::collections::{BTreeMap, HashSet};

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

/// Global/regional clock buffer primitive kind (UG953).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClockBufKind {
    #[default]
    Bufg,
    Bufgce,
    Bufgctrl,
    Bufio,
    Bufr,
}

/// A packed clock buffer (`BUFG`, `BUFGCE`, `BUFGCTRL`, `BUFIO`, `BUFR`).
#[derive(Clone, Debug, Default)]
pub struct PackedBufg {
    /// Source IR cell name (synthesized for inferred clock buffers).
    pub name: String,
    /// Net into the primary input (`I` or `I0`).
    pub in_net: String,
    /// Secondary input net (`I1` for `BUFGCTRL`).
    pub in1_net: Option<String>,
    /// Clock enable (`CE` / `CE0` / `CE1`).
    pub ce0_net: Option<String>,
    pub ce1_net: Option<String>,
    /// Select inputs (`S0` / `S1` for `BUFGCTRL`).
    pub s0_net: Option<String>,
    pub s1_net: Option<String>,
    /// Ignore inputs (`IGNORE0` / `IGNORE1` for `BUFGCTRL`).
    pub ignore0_net: Option<String>,
    pub ignore1_net: Option<String>,
    /// Clear input (`CLR` for `BUFR`).
    pub clr_net: Option<String>,
    /// Global/regional clock net out of the buffer (`O`).
    pub out_net: String,
    /// Specific clock buffer kind.
    pub kind: ClockBufKind,
    /// `BUFR_DIVIDE` (`"BYPASS"`, `"1"`..`"8"`).
    pub bufr_divide: String,
    /// `INIT_OUT` (`0` or `1`).
    pub init_out: u8,
    /// `PRESELECT_I0` / `PRESELECT_I1` for `BUFGCTRL`.
    pub preselect_i0: bool,
    pub preselect_i1: bool,
    /// Optional bank constraint (for `BUFIO`/`BUFR`).
    pub bank: Option<String>,
}

/// Block RAM primitive variant (`RAMB18E1` vs `RAMB36E1`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BramKind {
    #[default]
    Ramb18e1,
    Ramb36e1,
}

/// Block RAM operating mode (`TDP` vs `SDP`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RamMode {
    #[default]
    Tdp,
    Sdp,
}

/// Write mode per port (`WRITE_FIRST`, `READ_FIRST`, `NO_CHANGE`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RamWriteMode {
    #[default]
    WriteFirst,
    ReadFirst,
    NoChange,
}

/// Address cascade order (`NONE`, `FIRST`, `MIDDLE`, `LAST`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CascadeOrder {
    #[default]
    None,
    First,
    Middle,
    Last,
}

/// A packed block RAM (`RAMB18E1` or `RAMB36E1`).
#[derive(Clone, Debug, Default)]
pub struct PackedBram {
    /// Source IR cell name.
    pub name: String,
    /// Legacy 64-bit initialization words.
    pub init: Vec<u64>,
    /// Read width port A (`0, 1, 2, 4, 9, 18, 36, 72`).
    pub read_width_a: u8,
    /// Read width port B (`0, 1, 2, 4, 9, 18, 36, 72`).
    pub read_width_b: u8,
    /// Write width port A (`0, 1, 2, 4, 9, 18, 36, 72`).
    pub write_width_a: u8,
    /// Write width port B (`0, 1, 2, 4, 9, 18, 36, 72`).
    pub write_width_b: u8,
    /// `RAMB18E1` or `RAMB36E1`.
    pub kind: BramKind,
    /// `TDP` or `SDP`.
    pub ram_mode: RamMode,
    /// Port A / B write modes.
    pub write_mode_a: RamWriteMode,
    pub write_mode_b: RamWriteMode,
    /// Output pipeline register enables (`0` or `1`).
    pub doa_reg: u8,
    pub dob_reg: u8,
    /// `INIT_00`..`INIT_7F` (256-bit hex rows stored as `[u64; 4]` LSB-first).
    pub init_rows: BTreeMap<u8, [u64; 4]>,
    /// `INITP_00`..`INITP_0F` parity rows (`[u64; 4]` LSB-first).
    pub initp_rows: BTreeMap<u8, [u64; 4]>,
    /// Initial output values (`INIT_A`, `INIT_B`, `SRVAL_A`, `SRVAL_B`).
    pub init_a: u64,
    pub init_b: u64,
    pub srval_a: u64,
    pub srval_b: u64,
    /// Cascade configuration (`RAM_EXTENSION_A/B`, `CASCADE_ORDER_A/B`).
    pub cascade_order_a: CascadeOrder,
    pub cascade_order_b: CascadeOrder,
    pub cascade_in_a: Option<String>,
    pub cascade_out_a: Option<String>,
    pub cascade_in_b: Option<String>,
    pub cascade_out_b: Option<String>,
    /// Per-byte write-enable nets (`WEA[3:0]`, `WEBWE[7:0]`).
    pub wea_nets: Vec<String>,
    pub web_nets: Vec<String>,
    /// Clock nets (`CLKARDCLK`, `CLKBWRCLK`).
    pub clka_net: Option<String>,
    pub clkb_net: Option<String>,
}

/// A packed DSP block (`DSP48E1`).
#[derive(Clone, Debug, Default)]
pub struct PackedDsp {
    /// Source IR cell name.
    pub name: String,
    /// 7-bit OPMODE.
    pub opmode: u8,
    /// 4-bit ALUMODE.
    pub alumode: u8,
    /// 5-bit INMODE (controls 25-bit pre-adder and A/B input reg selection).
    pub inmode: u8,
    /// Pipeline register stages (`AREG`: 0..=2, `BREG`: 0..=2, others 0..=1).
    pub areg: u8,
    pub breg: u8,
    pub creg: u8,
    pub dreg: u8,
    pub adreg: u8,
    pub mreg: u8,
    pub preg: u8,
    pub opmodereg: u8,
    pub alumodereg: u8,
    pub inmodereg: u8,
    pub carryinreg: u8,
    pub carryinselreg: u8,
    /// `USE_MULT` (`"MULTIPLY"`, `"DYNAMIC"`, `"NONE"`).
    pub use_mult: String,
    /// `USE_DPORT` (`true` enables 25-bit pre-adder D+A path).
    pub use_dport: bool,
    /// `USE_SIMD` (`"ONE48"`, `"TWO24"`, `"FOUR12"`).
    pub use_simd: String,
    /// Pattern detector configuration (`USE_PATTERN_DETECT`, `PATTERN`, `MASK`, `SEL_PATTERN`, `SEL_MASK`).
    pub use_pattern_detect: bool,
    pub pattern: u64,
    pub mask: u64,
    pub sel_pattern: String,
    pub sel_mask: String,
    /// `A_INPUT` / `B_INPUT` (`"DIRECT"` or `"CASCADE"`).
    pub a_input_cascade: bool,
    pub b_input_cascade: bool,
    /// Dedicated column cascade nets (`PCOUT`->`PCIN`, `ACOUT`->`ACIN`, `BCOUT`->`BCIN`, `CARRYCASCOUT`->`CARRYCASCIN`).
    pub pcin_net: Option<String>,
    pub pcout_net: Option<String>,
    pub acin_net: Option<String>,
    pub acout_net: Option<String>,
    pub bcin_net: Option<String>,
    pub bcout_net: Option<String>,
    pub carrycascin_net: Option<String>,
    pub carrycascout_net: Option<String>,
    /// Clock net (`CLK`).
    pub clk_net: Option<String>,
}

/// A packed carry chain (`CARRY4`).
#[derive(Clone, Debug, Default)]
pub struct PackedCarry {
    /// Source IR cell name.
    pub name: String,
    /// `CI` cascade input net (driven by previous `CARRY4.CO[3]`).
    pub ci_net: Option<String>,
    /// `CYINIT` initialization net (`"0"`, `"1"`, or fabric net).
    pub cyinit_net: Option<String>,
    /// `DI[3:0]` carry-generate input nets.
    pub di_nets: [Option<String>; 4],
    /// `S[3:0]` carry-propagate (LUT O6) input nets.
    pub s_nets: [Option<String>; 4],
    /// `O[3:0]` sum outputs.
    pub o_nets: [Option<String>; 4],
    /// `CO[3:0]` carry outputs (`CO[3]` feeds next slice's `CI`).
    pub co_nets: [Option<String>; 4],
}

/// CMT primitive type (`MMCME2_ADV` vs `PLLE2_ADV`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CmtKind {
    #[default]
    MmcmE2Adv,
    PllE2Adv,
}

/// A packed Clock Management Tile primitive (`MMCME2_ADV` or `PLLE2_ADV`).
#[derive(Clone, Debug)]
pub struct PackedCmt {
    pub name: String,
    pub kind: CmtKind,
    /// `BANDWIDTH` (`"OPTIMIZED"`, `"HIGH"`, `"LOW"`).
    pub bandwidth: String,
    /// `COMPENSATION` (`"ZHOLD"`, `"BUF_IN"`, `"EXTERNAL"`, `"INTERNAL"`).
    pub compensation: String,
    /// Input clock periods in ns (`CLKIN1_PERIOD`, `CLKIN2_PERIOD`).
    pub clkin1_period_ns: f64,
    pub clkin2_period_ns: f64,
    /// Master divider `DIVCLK_DIVIDE` (`1..=106` for MMCM, `1..=56` for PLL).
    pub divclk_divide: u32,
    /// Feedback multiplier (`CLKFBOUT_MULT_F` for MMCM: `2.0..=64.0`, `CLKFBOUT_MULT` for PLL: `2..=64`).
    pub clkfbout_mult_f: f64,
    pub clkfbout_phase: f64,
    /// Calculated VCO frequency in MHz (`f_in * M / D`).
    pub vco_freq_mhz: f64,
    /// Output dividers (`CLKOUT0_DIVIDE_F` for MMCM, `CLKOUT0..6_DIVIDE`: `1..=128`).
    pub clkout0_divide_f: f64,
    pub clkout_divide: [u32; 7],
    pub clkout_phase: [f64; 7],
    pub clkout_duty: [f64; 7],
    /// Clock nets.
    pub clkin1_net: Option<String>,
    pub clkin2_net: Option<String>,
    pub clkfbin_net: Option<String>,
    pub clkfbout_net: Option<String>,
    pub clkout_nets: [Option<String>; 7],
    pub locked_net: Option<String>,
    pub rst_net: Option<String>,
    pub pwrdwn_net: Option<String>,
}

/// DDR clock edge mode for `IDDR` and `ODDR` (UG953).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DdrClkEdge {
    #[default]
    OppositeEdge,
    SameEdge,
    SameEdgePipelined,
}

/// Set/Reset type for `IDDR` and `ODDR` (`SYNC` vs `ASYNC`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SrType {
    #[default]
    Sync,
    Async,
}

/// A packed `IDDR` or `ODDR` primitive in `ILOGICE2`/`OLOGICE2`.
#[derive(Clone, Debug)]
pub struct PackedIoddr {
    pub name: String,
    /// `true` for `IDDR` (`ILOGICE2`), `false` for `ODDR` (`OLOGICE2`).
    pub is_input: bool,
    pub ddr_clk_edge: DdrClkEdge,
    pub srtype: SrType,
    /// Initial values (`INIT_Q1`/`INIT_Q2` for `IDDR`, `INIT` for `ODDR`).
    pub init_q1: u8,
    pub init_q2: u8,
    /// Clock net (`C`), clock enable (`CE`), set/reset (`S`/`R`).
    pub c_net: String,
    pub ce_net: Option<String>,
    pub sr_net: Option<String>,
    /// Data nets: for `IDDR`, `d_net` is `D` and `q1_net`/`q2_net` are `Q1`/`Q2`;
    /// for `ODDR`, `d1_net`/`d2_net` are `D1`/`D2` and `q_net` is `Q`.
    pub d_net: Option<String>,
    pub d1_net: Option<String>,
    pub d2_net: Option<String>,
    pub q_net: Option<String>,
    pub q1_net: Option<String>,
    pub q2_net: Option<String>,
    /// Optional bank/LOC affinity derived from connected IOB/port.
    pub bank: Option<String>,
    pub loc: Option<String>,
}

/// A packed `ISERDESE2` or `OSERDESE2` primitive.
#[derive(Clone, Debug)]
pub struct PackedSerdes {
    pub name: String,
    /// `true` for `ISERDESE2`, `false` for `OSERDESE2`.
    pub is_input: bool,
    /// `"SDR"` or `"DDR"`.
    pub data_rate: String,
    /// Serialization/deserialization width (`2, 3, 4, 5, 6, 7, 8, 10, 14`).
    pub data_width: u8,
    /// `INTERFACE_TYPE` (`"MEMORY"`, `"MEMORY_DDR3"`, `"MEMORY_QDR"`, `"NETWORKING"`, `"OVERSAMPLE"`).
    pub interface_type: String,
    /// `SERDES_MODE` (`"MASTER"` or `"SLAVE"` for 10:1 / 14:1 width expansion).
    pub serdes_mode: String,
    /// `IOBDELAY` for `ISERDESE2` (`"NONE"`, `"IBUF"`, `"IFD"`, `"BOTH"`).
    pub iobdelay: String,
    /// `TRISTATE_WIDTH` for `OSERDESE2` (`1` or `4`).
    pub tristate_width: u8,
    /// High-speed bit clock (`CLK`) and divided parallel clock (`CLKDIV`).
    pub clk_net: String,
    pub clkdiv_net: String,
    pub rst_net: Option<String>,
    /// Serial data net (`D` / `DDLY` for `ISERDESE2`, `OQ` / `OFB` for `OSERDESE2`).
    pub serial_net: Option<String>,
    /// Parallel data nets (`Q1`..`Q8` for `ISERDESE2`, `D1`..`D8` for `OSERDESE2`).
    pub parallel_nets: [Option<String>; 8],
    /// Cascade nets (`SHIFTIN1`/`SHIFTIN2`, `SHIFTOUT1`/`SHIFTOUT2`).
    pub shiftin1_net: Option<String>,
    pub shiftin2_net: Option<String>,
    pub shiftout1_net: Option<String>,
    pub shiftout2_net: Option<String>,
    /// I/O bank and package pin LOC affinity.
    pub bank: Option<String>,
    pub loc: Option<String>,
}

/// Delay primitive kind (`IDELAYE2`, `ODELAYE2`, `IDELAYCTRL`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IodelayKind {
    #[default]
    IdelayE2,
    OdelayE2,
    IdelayCtrl,
}

/// A packed 32-tap 78ps calibrated delay primitive (`IDELAYE2`, `ODELAYE2`, or `IDELAYCTRL`).
#[derive(Clone, Debug)]
pub struct PackedIodelay {
    pub name: String,
    pub kind: IodelayKind,
    /// `IDELAY_TYPE` / `ODELAY_TYPE` (`"FIXED"`, `"VARIABLE"`, `"VAR_LOAD"`, `"VAR_LOAD_PIPE"`).
    pub delay_type: String,
    /// Tap setting `0..=31` (`IDELAY_VALUE` / `ODELAY_VALUE`).
    pub delay_value: u8,
    /// `DELAY_SRC` (`"IDATAIN"`, `"DATAIN"`, `"ODATAIN"`, `"CLKIN"`).
    pub delay_src: String,
    /// Reference clock frequency in MHz (`190.0..=210.0`, `290.0..=310.0`, `390.0..=410.0`).
    pub refclk_frequency: f64,
    /// `HIGH_PERFORMANCE_MODE` (`true` or `false`).
    pub high_performance_mode: bool,
    /// `CINVCTRL_SEL` and `PIPE_SEL`.
    pub cinvctrl_sel: bool,
    pub pipe_sel: bool,
    /// `IODELAY_GROUP` name tying `IDELAYE2`/`ODELAYE2` to an `IDELAYCTRL`.
    pub iodelay_group: String,
    /// Clock/control/data nets (`C` / `REFCLK`, `RST`, `CE`, `INC`, `LD`, `IDATAIN`/`ODATAIN`, `DATAOUT`, `RDY`).
    pub c_net: Option<String>,
    pub rst_net: Option<String>,
    pub ce_net: Option<String>,
    pub inc_net: Option<String>,
    pub ld_net: Option<String>,
    pub data_in_net: Option<String>,
    pub data_out_net: Option<String>,
    pub cntvaluein_nets: [Option<String>; 5],
    pub cntvalueout_nets: [Option<String>; 5],
    pub rdy_net: Option<String>,
    /// Bank / LOC affinity.
    pub bank: Option<String>,
    pub loc: Option<String>,
}

/// Shift-register LUT kind (`SRL16E` vs `SRLC32E`) in `SLICEM`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SrlKind {
    #[default]
    Srl16e,
    Srlc32e,
}

/// A packed shift-register LUT (`SRL16E` or `SRLC32E`) requiring a `SLICEM` site.
#[derive(Clone, Debug)]
pub struct PackedSrl {
    pub name: String,
    pub kind: SrlKind,
    /// 16-bit (zero-extended) or 32-bit `INIT` value.
    pub init: u32,
    pub clk_net: String,
    pub ce_net: Option<String>,
    pub d_net: String,
    /// Address inputs `A0..A4` (`A4` is `None` for `SRL16E`).
    pub addr_nets: [Option<String>; 5],
    /// Primary output `Q`.
    pub q_net: String,
    /// Cascade output `Q31` (for `SRLC32E`).
    pub q31_net: Option<String>,
    pub is_clk_inverted: bool,
}

/// Wide multiplexer BEL kind (`MUXF7` vs `MUXF8`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MuxfKind {
    #[default]
    Muxf7,
    Muxf8,
}

/// A packed wide slice multiplexer (`MUXF7` or `MUXF8`).
#[derive(Clone, Debug)]
pub struct PackedMuxf {
    pub name: String,
    pub kind: MuxfKind,
    pub i0_net: String,
    pub i1_net: String,
    pub s_net: String,
    pub o_net: String,
}

/// A packed 7-Series GTP transceiver channel (`GTPE2_CHANNEL`, UG482).
#[derive(Clone, Debug, Default)]
pub struct PackedGtpChannel {
    pub name: String,
    pub rxout_div: u8,
    pub txout_div: u8,
    pub rx_data_width: u8,
    pub tx_data_width: u8,
    pub rx_int_datawidth: u8,
    pub tx_int_datawidth: u8,
    pub pll0clk_net: Option<String>,
    pub pll0refclk_net: Option<String>,
    pub pll1clk_net: Option<String>,
    pub pll1refclk_net: Option<String>,
    pub rxusrclk_net: Option<String>,
    pub txusrclk_net: Option<String>,
    pub txoutclk_net: Option<String>,
    pub rxoutclk_net: Option<String>,
    pub gtprxp_net: Option<String>,
    pub gtptxp_net: Option<String>,
}

/// A packed 7-Series GTP transceiver quad PLL (`GTPE2_COMMON`, UG482).
#[derive(Clone, Debug, Default)]
pub struct PackedGtpCommon {
    pub name: String,
    pub pll0_fbdiv: u8,
    pub pll0_fbdiv_45: u8,
    pub pll0_refclk_div: u8,
    pub pll1_fbdiv: u8,
    pub pll1_fbdiv_45: u8,
    pub pll1_refclk_div: u8,
    pub gtrefclk0_net: Option<String>,
    pub gtrefclk1_net: Option<String>,
    pub pll0outclk_net: Option<String>,
    pub pll0outrefclk_net: Option<String>,
    pub pll1outclk_net: Option<String>,
    pub pll1outrefclk_net: Option<String>,
    pub pll0lock_net: Option<String>,
    pub pll1lock_net: Option<String>,
}

/// A packed gigabit transceiver differential reference clock buffer (`IBUFDS_GTE2`).
#[derive(Clone, Debug, Default)]
pub struct PackedIbufdsGte2 {
    pub name: String,
    pub clkcm_cfg: bool,
    pub clkrcv_trst: bool,
    pub clkswing_cfg: u8,
    pub i_net: Option<String>,
    pub ib_net: Option<String>,
    pub ceb_net: Option<String>,
    pub o_net: String,
    pub odiv2_net: Option<String>,
}

/// A packed 7-Series Integrated Block for PCI Express (`PCIE_2_1`, PG054).
#[derive(Clone, Debug, Default)]
pub struct PackedPcie {
    pub name: String,
    pub bar: [u32; 6],
    pub max_payload_supported: u8,
    pub max_link_speed: u8,
    pub max_link_width: u8,
    pub userclk1_net: Option<String>,
    pub userclk2_net: Option<String>,
    pub pipeclk_net: Option<String>,
    pub user_lnk_up_net: Option<String>,
}

/// A packed 7-Series Dual 12-Bit 1MSPS Analog-to-Digital Converter (`XADC`, UG480).
#[derive(Clone, Debug, Default)]
pub struct PackedXadc {
    pub name: String,
    /// `INIT_40`..`INIT_5F` 16-bit configuration registers.
    pub init_regs: BTreeMap<u8, u16>,
    pub dclk_net: Option<String>,
    pub den_net: Option<String>,
    pub dwe_net: Option<String>,
    pub drdy_net: Option<String>,
    pub busy_net: Option<String>,
    pub vp_net: Option<String>,
    pub vn_net: Option<String>,
}

/// 7-Series configuration / JTAG / bitstream primitive kind (UG470 / UG953).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CfgPrimKind {
    #[default]
    Bscane2,
    Icape2,
    DnaPort,
    FrameEcce2,
    Startupe2,
    UsrAccesse2,
    Capturee2,
}

/// A packed 7-Series configuration / JTAG / silicon-ID primitive.
#[derive(Clone, Debug, Default)]
pub struct PackedCfgPrim {
    pub name: String,
    pub kind: CfgPrimKind,
    /// `JTAG_CHAIN` (`1..=4`) for `BSCANE2`.
    pub jtag_chain: u8,
    pub disable_jtag: bool,
    /// `ICAP_WIDTH` (`"X8"`, `"X16"`, `"X32"`) and `DEVICE_ID` for `ICAPE2`.
    pub icap_width: String,
    pub device_id: u32,
    /// `SIM_DNA_VALUE` (57-bit) for `DNA_PORT`.
    pub sim_dna_value: u64,
    /// `FARSRC` (`"FAR"` or `"EFAR"`) for `FRAME_ECCE2`.
    pub farsrc: String,
    /// `PROG_USR` for `STARTUPE2` and `ONESHOT` for `CAPTUREE2`.
    pub prog_usr: bool,
    pub oneshot: bool,
    pub clk_net: Option<String>,
    pub out_net: Option<String>,
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
    pub cmts: Vec<PackedCmt>,
    pub ioddrs: Vec<PackedIoddr>,
    pub serdes: Vec<PackedSerdes>,
    pub iodelays: Vec<PackedIodelay>,
    pub srls: Vec<PackedSrl>,
    pub muxfs: Vec<PackedMuxf>,
    pub gtp_channels: Vec<PackedGtpChannel>,
    pub gtp_commons: Vec<PackedGtpCommon>,
    pub ibufds_gte2s: Vec<PackedIbufdsGte2>,
    pub pcies: Vec<PackedPcie>,
    pub xadcs: Vec<PackedXadc>,
    pub cfg_prims: Vec<PackedCfgPrim>,
}
/// Pack a [`Design`] into 7-series packable units.
///
/// Inserts IBUF + BUFGCTRL for every top-level input port whose net drives
/// at least one clocked primitive pin (clock inference by use). Returns an
/// error on unsupported cell kinds or illegal UG953 parameters rather than
/// silently dropping them.
pub fn pack(netlist: &Design) -> Result<Packed7, String> {
    let pins = netlist.pin_index();
    let mut out = Packed7::default();

    for cell in &netlist.cells {
        let cell_name = resolve(cell.name);
        match &cell.kind {
            CellKind::Lut6 { init } => {
                let mut input_nets: [Option<String>; 6] = Default::default();
                for (i, slot) in input_nets.iter_mut().enumerate() {
                    let pin = format!("I{i}");
                    *slot = pins.net_on(cell_name, &pin).map(|s| resolve(s).to_string());
                }
                let output_net = resolve(
                    pins.net_on(cell_name, "O")
                        .ok_or_else(|| format!("pack: LUT {cell_name} has no O net"))?,
                )
                .to_string();
                out.luts.push(PackedLut {
                    name: cell_name.to_string(),
                    init: *init,
                    input_nets,
                    output_net,
                });
            }
            CellKind::Hff => {
                let need = |pin: &str| {
                    pins.net_on(cell_name, pin)
                        .map(|s| resolve(s).to_string())
                        .ok_or_else(|| format!("pack: FF {cell_name} has no {pin} net"))
                };
                out.ffs.push(PackedFf {
                    name: cell_name.to_string(),
                    d_net: need("D")?,
                    q_net: need("Q")?,
                    clk_net: need("CLK")?,
                    ce_net: pins.net_on(cell_name, "CE").map(|s| resolve(s).to_string()),
                    sr_net: pins
                        .net_on(cell_name, "SR")
                        .or_else(|| pins.net_on(cell_name, "RST"))
                        .map(|s| resolve(s).to_string()),
                });
            }
            CellKind::IobOut => {
                let net = resolve(
                    pins.net_on(cell_name, "I")
                        .ok_or_else(|| format!("pack: IOB {cell_name} has no I net"))?,
                )
                .to_string();
                let port = resolve(pins.net_on(cell_name, "PAD").unwrap_or(cell.name)).to_string();
                let port_def = netlist.ports.iter().find(|p| resolve(p.name) == port);
                out.iobs.push(PackedIob {
                    name: cell_name.to_string(),
                    loc: port_def
                        .and_then(|p| p.attrs.get("LOC"))
                        .or_else(|| cell.attrs.get("LOC"))
                        .map(|s| s.to_string()),
                    iostandard: port_def
                        .and_then(|p| p.attrs.get("IOSTANDARD"))
                        .or_else(|| cell.attrs.get("IOSTANDARD"))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "LVCMOS33".to_string()),
                    drive: port_def
                        .and_then(|p| p.attrs.get("DRIVE"))
                        .or_else(|| cell.attrs.get("DRIVE"))
                        .map(|s| s.to_string()),
                    slew: port_def
                        .and_then(|p| p.attrs.get("SLEW"))
                        .or_else(|| cell.attrs.get("SLEW"))
                        .map(|s| s.to_string()),
                    port,
                    dir: IobDir::Out,
                    net,
                });
            }
            CellKind::IobIn => {
                let net = resolve(
                    pins.net_on(cell_name, "O")
                        .ok_or_else(|| format!("pack: IBUF {cell_name} has no O net"))?,
                )
                .to_string();
                let port = resolve(pins.net_on(cell_name, "PAD").unwrap_or(cell.name)).to_string();
                let port_def = netlist.ports.iter().find(|p| resolve(p.name) == port);
                out.iobs.push(PackedIob {
                    name: cell_name.to_string(),
                    loc: port_def
                        .and_then(|p| p.attrs.get("LOC"))
                        .or_else(|| cell.attrs.get("LOC"))
                        .map(|s| s.to_string()),
                    iostandard: port_def
                        .and_then(|p| p.attrs.get("IOSTANDARD"))
                        .or_else(|| cell.attrs.get("IOSTANDARD"))
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
                out.bufgs
                    .push(pack_clock_buf(cell_name, "BUFG", &pins, &cell.attrs)?);
            }
            CellKind::Bram18 | CellKind::Bram18e1 { .. } => {
                let (ra, rb, wa, wb) = match &cell.kind {
                    CellKind::Bram18e1 {
                        read_width_a,
                        read_width_b,
                        write_width_a,
                        write_width_b,
                    } => (*read_width_a, *read_width_b, *write_width_a, *write_width_b),
                    _ => (18, 18, 18, 18),
                };
                out.brams.push(pack_bram(
                    cell_name,
                    BramKind::Ramb18e1,
                    Some((ra, rb, wa, wb)),
                    &pins,
                    &cell.attrs,
                )?);
            }
            CellKind::Mac27 | CellKind::Dsp48e1 { .. } => {
                let (opmode, alumode) = match &cell.kind {
                    CellKind::Dsp48e1 { opmode, alumode } => (*opmode, *alumode),
                    _ => (0b0000101, 0b0000),
                };
                out.dsps.push(pack_dsp(
                    cell_name,
                    Some((opmode, alumode)),
                    &pins,
                    &cell.attrs,
                )?);
            }
            CellKind::Carry4 => {
                out.carries.push(pack_carry4(cell_name, &pins)?);
            }
            CellKind::BlackBox { module } => {
                let m = resolve(*module);
                pack_blackbox_primitive(cell_name, m, netlist, &pins, &cell.attrs, &mut out)?;
            }
            other => {
                return Err(format!(
                    "pack: unsupported CellKind {other:?} for 7-series (cell {cell_name})"
                ));
            }
        }
    }

    validate_iodelay_groups(&out)?;
    insert_clock_spine(netlist, &mut out)?;

    Ok(out)
}

fn pack_blackbox_primitive(
    cell_name: &str,
    module: &str,
    netlist: &Design,
    pins: &PinIndex,
    attrs: &Attrs,
    out: &mut Packed7,
) -> Result<(), String> {
    let up = module.to_ascii_uppercase();
    match up.as_str() {
        "MMCME2_ADV" | "MMCME2_BASE" => {
            out.cmts
                .push(pack_cmt(cell_name, CmtKind::MmcmE2Adv, pins, attrs)?);
        }
        "PLLE2_ADV" | "PLLE2_BASE" => {
            out.cmts
                .push(pack_cmt(cell_name, CmtKind::PllE2Adv, pins, attrs)?);
        }
        "RAMB18E1" => {
            out.brams
                .push(pack_bram(cell_name, BramKind::Ramb18e1, None, pins, attrs)?);
        }
        "RAMB36E1" => {
            out.brams
                .push(pack_bram(cell_name, BramKind::Ramb36e1, None, pins, attrs)?);
        }
        "DSP48E1" => {
            out.dsps.push(pack_dsp(cell_name, None, pins, attrs)?);
        }
        "CARRY4" => {
            out.carries.push(pack_carry4(cell_name, pins)?);
        }
        "IDDR" => {
            out.ioddrs
                .push(pack_ioddr(cell_name, true, netlist, pins, attrs)?);
        }
        "ODDR" => {
            out.ioddrs
                .push(pack_ioddr(cell_name, false, netlist, pins, attrs)?);
        }
        "ISERDESE2" => {
            out.serdes
                .push(pack_serdes(cell_name, true, netlist, pins, attrs)?);
        }
        "OSERDESE2" => {
            out.serdes
                .push(pack_serdes(cell_name, false, netlist, pins, attrs)?);
        }
        "IDELAYE2" => {
            out.iodelays.push(pack_iodelay(
                cell_name,
                IodelayKind::IdelayE2,
                netlist,
                pins,
                attrs,
            )?);
        }
        "ODELAYE2" => {
            out.iodelays.push(pack_iodelay(
                cell_name,
                IodelayKind::OdelayE2,
                netlist,
                pins,
                attrs,
            )?);
        }
        "IDELAYCTRL" => {
            out.iodelays.push(pack_iodelay(
                cell_name,
                IodelayKind::IdelayCtrl,
                netlist,
                pins,
                attrs,
            )?);
        }
        "BUFG" | "BUFGCE" | "BUFGCTRL" | "BUFIO" | "BUFR" => {
            out.bufgs
                .push(pack_clock_buf(cell_name, &up, pins, attrs)?);
        }
        "SRL16E" => {
            out.srls
                .push(pack_srl(cell_name, SrlKind::Srl16e, pins, attrs)?);
        }
        "SRLC32E" => {
            out.srls
                .push(pack_srl(cell_name, SrlKind::Srlc32e, pins, attrs)?);
        }
        "MUXF7" => {
            out.muxfs
                .push(pack_muxf(cell_name, MuxfKind::Muxf7, pins)?);
        }
        "MUXF8" => {
            out.muxfs
                .push(pack_muxf(cell_name, MuxfKind::Muxf8, pins)?);
        }
        "GTPE2_CHANNEL" | "GTXE2_CHANNEL" => {
            out.gtp_channels
                .push(pack_gtp_channel(cell_name, pins, attrs)?);
        }
        "GTPE2_COMMON" | "GTXE2_COMMON" => {
            out.gtp_commons
                .push(pack_gtp_common(cell_name, pins, attrs)?);
        }
        "IBUFDS_GTE2" => {
            out.ibufds_gte2s
                .push(pack_ibufds_gte2(cell_name, pins, attrs)?);
        }
        "PCIE_2_1" => {
            out.pcies.push(pack_pcie(cell_name, pins, attrs)?);
        }
        "XADC" => {
            out.xadcs.push(pack_xadc(cell_name, pins, attrs)?);
        }
        "BSCANE2" | "ICAPE2" | "DNA_PORT" | "FRAME_ECCE2" | "STARTUPE2" | "USR_ACCESSE2"
        | "CAPTUREE2" => {
            out.cfg_prims
                .push(pack_cfg_prim(cell_name, &up, pins, attrs)?);
        }
        _ => {
            return Err(format!(
                "pack: unsupported BlackBox `{module}` for 7-series (cell {cell_name})"
            ));
        }
    }
    Ok(())
}

fn pack_gtp_channel(
    cell_name: &str,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedGtpChannel, String> {
    let parse_div = |key: &str| -> Result<u8, String> {
        let d = attrs
            .get(key)
            .map(parse_verilog_u64)
            .transpose()?
            .unwrap_or(2) as u8;
        if !matches!(d, 1 | 2 | 4 | 8 | 16) {
            return Err(format!(
                "pack: GTPE2_CHANNEL {cell_name}: illegal {key}={d} (must be 1, 2, 4, 8, or 16)"
            ));
        }
        Ok(d)
    };
    let parse_dw = |key: &str| -> Result<u8, String> {
        let w = attrs
            .get(key)
            .map(parse_verilog_u64)
            .transpose()?
            .unwrap_or(20) as u8;
        if !matches!(w, 16 | 20 | 32 | 40) {
            return Err(format!(
                "pack: GTPE2_CHANNEL {cell_name}: illegal {key}={w} (must be 16, 20, 32, or 40)"
            ));
        }
        Ok(w)
    };
    let rxout_div = parse_div("RXOUT_DIV")?;
    let txout_div = parse_div("TXOUT_DIV")?;
    let rx_data_width = parse_dw("RX_DATA_WIDTH")?;
    let tx_data_width = parse_dw("TX_DATA_WIDTH")?;
    let rx_int_datawidth = attrs
        .get("RX_INT_DATAWIDTH")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0) as u8
        & 1;
    let tx_int_datawidth = attrs
        .get("TX_INT_DATAWIDTH")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0) as u8
        & 1;
    Ok(PackedGtpChannel {
        name: cell_name.to_string(),
        rxout_div,
        txout_div,
        rx_data_width,
        tx_data_width,
        rx_int_datawidth,
        tx_int_datawidth,
        pll0clk_net: pins
            .net_on(cell_name, "PLL0CLK")
            .map(|s| resolve(s).to_string()),
        pll0refclk_net: pins
            .net_on(cell_name, "PLL0REFCLK")
            .map(|s| resolve(s).to_string()),
        pll1clk_net: pins
            .net_on(cell_name, "PLL1CLK")
            .map(|s| resolve(s).to_string()),
        pll1refclk_net: pins
            .net_on(cell_name, "PLL1REFCLK")
            .map(|s| resolve(s).to_string()),
        rxusrclk_net: pins
            .net_on(cell_name, "RXUSRCLK")
            .map(|s| resolve(s).to_string()),
        txusrclk_net: pins
            .net_on(cell_name, "TXUSRCLK")
            .map(|s| resolve(s).to_string()),
        txoutclk_net: pins
            .net_on(cell_name, "TXOUTCLK")
            .map(|s| resolve(s).to_string()),
        rxoutclk_net: pins
            .net_on(cell_name, "RXOUTCLK")
            .map(|s| resolve(s).to_string()),
        gtprxp_net: pins
            .net_on(cell_name, "GTPRXP")
            .map(|s| resolve(s).to_string()),
        gtptxp_net: pins
            .net_on(cell_name, "GTPTXP")
            .map(|s| resolve(s).to_string()),
    })
}

fn pack_gtp_common(
    cell_name: &str,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedGtpCommon, String> {
    let fbdiv = |key: &str| -> Result<u8, String> {
        let d = attrs
            .get(key)
            .map(parse_verilog_u64)
            .transpose()?
            .unwrap_or(4) as u8;
        if !matches!(d, 1 | 2 | 3 | 4 | 5) {
            return Err(format!(
                "pack: GTPE2_COMMON {cell_name}: illegal {key}={d} (must be 1..=5)"
            ));
        }
        Ok(d)
    };
    let fbdiv45 = |key: &str| -> Result<u8, String> {
        let d = attrs
            .get(key)
            .map(parse_verilog_u64)
            .transpose()?
            .unwrap_or(5) as u8;
        if !matches!(d, 4 | 5) {
            return Err(format!(
                "pack: GTPE2_COMMON {cell_name}: illegal {key}={d} (must be 4 or 5)"
            ));
        }
        Ok(d)
    };
    let refdiv = |key: &str| -> Result<u8, String> {
        let d = attrs
            .get(key)
            .map(parse_verilog_u64)
            .transpose()?
            .unwrap_or(1) as u8;
        if !matches!(d, 1 | 2) {
            return Err(format!(
                "pack: GTPE2_COMMON {cell_name}: illegal {key}={d} (must be 1 or 2)"
            ));
        }
        Ok(d)
    };
    Ok(PackedGtpCommon {
        name: cell_name.to_string(),
        pll0_fbdiv: fbdiv("PLL0_FBDIV")?,
        pll0_fbdiv_45: fbdiv45("PLL0_FBDIV_45")?,
        pll0_refclk_div: refdiv("PLL0_REFCLK_DIV")?,
        pll1_fbdiv: fbdiv("PLL1_FBDIV")?,
        pll1_fbdiv_45: fbdiv45("PLL1_FBDIV_45")?,
        pll1_refclk_div: refdiv("PLL1_REFCLK_DIV")?,
        gtrefclk0_net: pins
            .net_on(cell_name, "GTREFCLK0")
            .map(|s| resolve(s).to_string()),
        gtrefclk1_net: pins
            .net_on(cell_name, "GTREFCLK1")
            .map(|s| resolve(s).to_string()),
        pll0outclk_net: pins
            .net_on(cell_name, "PLL0OUTCLK")
            .map(|s| resolve(s).to_string()),
        pll0outrefclk_net: pins
            .net_on(cell_name, "PLL0OUTREFCLK")
            .map(|s| resolve(s).to_string()),
        pll1outclk_net: pins
            .net_on(cell_name, "PLL1OUTCLK")
            .map(|s| resolve(s).to_string()),
        pll1outrefclk_net: pins
            .net_on(cell_name, "PLL1OUTREFCLK")
            .map(|s| resolve(s).to_string()),
        pll0lock_net: pins
            .net_on(cell_name, "PLL0LOCK")
            .map(|s| resolve(s).to_string()),
        pll1lock_net: pins
            .net_on(cell_name, "PLL1LOCK")
            .map(|s| resolve(s).to_string()),
    })
}

fn pack_ibufds_gte2(
    cell_name: &str,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedIbufdsGte2, String> {
    let clkcm_cfg = parse_bool_attr(attrs, "CLKCM_CFG", true)?;
    let clkrcv_trst = parse_bool_attr(attrs, "CLKRCV_TRST", true)?;
    let clkswing_cfg = attrs
        .get("CLKSWING_CFG")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0b11) as u8
        & 0b11;
    let o_net = pins
        .net_on(cell_name, "O")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: IBUFDS_GTE2 {cell_name} has no O net"))?;
    Ok(PackedIbufdsGte2 {
        name: cell_name.to_string(),
        clkcm_cfg,
        clkrcv_trst,
        clkswing_cfg,
        i_net: pins.net_on(cell_name, "I").map(|s| resolve(s).to_string()),
        ib_net: pins.net_on(cell_name, "IB").map(|s| resolve(s).to_string()),
        ceb_net: pins
            .net_on(cell_name, "CEB")
            .map(|s| resolve(s).to_string()),
        o_net,
        odiv2_net: pins
            .net_on(cell_name, "ODIV2")
            .map(|s| resolve(s).to_string()),
    })
}

fn pack_pcie(cell_name: &str, pins: &PinIndex, attrs: &Attrs) -> Result<PackedPcie, String> {
    let mut bar = [0u32; 6];
    for (i, slot) in bar.iter_mut().enumerate() {
        let k = format!("BAR{i}");
        *slot = attrs
            .get(&k)
            .map(parse_verilog_u64)
            .transpose()?
            .unwrap_or(0) as u32;
    }
    let max_payload_supported = attrs
        .get("DEV_CAP_MAX_PAYLOAD_SUPPORTED")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(2) as u8;
    let max_link_speed = attrs
        .get("LINK_CAP_MAX_LINK_SPEED")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(1) as u8;
    if !matches!(max_link_speed, 1 | 2) {
        return Err(format!(
            "pack: PCIE_2_1 {cell_name}: illegal LINK_CAP_MAX_LINK_SPEED={max_link_speed} (must be 1=Gen1 or 2=Gen2)"
        ));
    }
    let max_link_width = attrs
        .get("LINK_CAP_MAX_LINK_WIDTH")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(1) as u8;
    if !matches!(max_link_width, 1 | 2 | 4 | 8) {
        return Err(format!(
            "pack: PCIE_2_1 {cell_name}: illegal LINK_CAP_MAX_LINK_WIDTH={max_link_width} (must be 1, 2, 4, or 8)"
        ));
    }
    Ok(PackedPcie {
        name: cell_name.to_string(),
        bar,
        max_payload_supported,
        max_link_speed,
        max_link_width,
        userclk1_net: pins
            .net_on(cell_name, "USERCLK1")
            .map(|s| resolve(s).to_string()),
        userclk2_net: pins
            .net_on(cell_name, "USERCLK2")
            .map(|s| resolve(s).to_string()),
        pipeclk_net: pins
            .net_on(cell_name, "PIPECLK")
            .map(|s| resolve(s).to_string()),
        user_lnk_up_net: pins
            .net_on(cell_name, "USERLNKUP")
            .map(|s| resolve(s).to_string()),
    })
}

fn pack_xadc(cell_name: &str, pins: &PinIndex, attrs: &Attrs) -> Result<PackedXadc, String> {
    let mut init_regs = BTreeMap::new();
    for (k, v) in &attrs.map {
        let up = k.to_ascii_uppercase();
        if let Some(hex_idx) = up.strip_prefix("INIT_") {
            if hex_idx.len() == 2 && hex_idx.chars().all(|c| c.is_ascii_hexdigit()) {
                let reg = u8::from_str_radix(hex_idx, 16).unwrap_or(0);
                if !(0x40..=0x5F).contains(&reg) {
                    return Err(format!(
                        "pack: XADC {cell_name}: {up} outside legal register range INIT_40..INIT_5F"
                    ));
                }
                let val = parse_verilog_u64(v)? as u16;
                init_regs.insert(reg, val);
            }
        }
    }
    Ok(PackedXadc {
        name: cell_name.to_string(),
        init_regs,
        dclk_net: pins
            .net_on(cell_name, "DCLK")
            .map(|s| resolve(s).to_string()),
        den_net: pins.net_on(cell_name, "DEN").map(|s| resolve(s).to_string()),
        dwe_net: pins.net_on(cell_name, "DWE").map(|s| resolve(s).to_string()),
        drdy_net: pins
            .net_on(cell_name, "DRDY")
            .map(|s| resolve(s).to_string()),
        busy_net: pins
            .net_on(cell_name, "BUSY")
            .map(|s| resolve(s).to_string()),
        vp_net: pins.net_on(cell_name, "VP").map(|s| resolve(s).to_string()),
        vn_net: pins.net_on(cell_name, "VN").map(|s| resolve(s).to_string()),
    })
}

fn pack_cfg_prim(
    cell_name: &str,
    prim: &str,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedCfgPrim, String> {
    let kind = match prim {
        "BSCANE2" => CfgPrimKind::Bscane2,
        "ICAPE2" => CfgPrimKind::Icape2,
        "DNA_PORT" => CfgPrimKind::DnaPort,
        "FRAME_ECCE2" => CfgPrimKind::FrameEcce2,
        "STARTUPE2" => CfgPrimKind::Startupe2,
        "USR_ACCESSE2" => CfgPrimKind::UsrAccesse2,
        "CAPTUREE2" => CfgPrimKind::Capturee2,
        _ => return Err(format!("pack: unknown config primitive `{prim}`")),
    };
    let jtag_chain = attrs
        .get("JTAG_CHAIN")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(1) as u8;
    if kind == CfgPrimKind::Bscane2 && !(1..=4).contains(&jtag_chain) {
        return Err(format!(
            "pack: BSCANE2 {cell_name}: JTAG_CHAIN={jtag_chain} out of range 1..=4"
        ));
    }
    let disable_jtag = parse_bool_attr(attrs, "DISABLE_JTAG", false)?;
    let icap_width = attrs
        .get("ICAP_WIDTH")
        .unwrap_or("X32")
        .trim()
        .to_ascii_uppercase();
    if kind == CfgPrimKind::Icape2 && !matches!(icap_width.as_str(), "X8" | "X16" | "X32") {
        return Err(format!(
            "pack: ICAPE2 {cell_name}: invalid ICAP_WIDTH `{icap_width}` (must be X8, X16, or X32)"
        ));
    }
    let device_id = attrs
        .get("DEVICE_ID")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0x0362_D093) as u32;
    let sim_dna_value = attrs
        .get("SIM_DNA_VALUE")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0)
        & ((1u64 << 57) - 1);
    let farsrc = attrs
        .get("FARSRC")
        .unwrap_or("EFAR")
        .trim()
        .to_ascii_uppercase();
    if kind == CfgPrimKind::FrameEcce2 && !matches!(farsrc.as_str(), "EFAR" | "FAR") {
        return Err(format!(
            "pack: FRAME_ECCE2 {cell_name}: invalid FARSRC `{farsrc}` (must be EFAR or FAR)"
        ));
    }
    let prog_usr = parse_bool_attr(attrs, "PROG_USR", false)?;
    let oneshot = parse_bool_attr(attrs, "ONESHOT", true)?;
    Ok(PackedCfgPrim {
        name: cell_name.to_string(),
        kind,
        jtag_chain,
        disable_jtag,
        icap_width,
        device_id,
        sim_dna_value,
        farsrc,
        prog_usr,
        oneshot,
        clk_net: pins
            .net_on(cell_name, "CLK")
            .or_else(|| pins.net_on(cell_name, "TCK"))
            .map(|s| resolve(s).to_string()),
        out_net: pins
            .net_on(cell_name, "DOUT")
            .or_else(|| pins.net_on(cell_name, "TDI"))
            .or_else(|| pins.net_on(cell_name, "CFGMCLK"))
            .or_else(|| pins.net_on(cell_name, "CRCERROR"))
            .or_else(|| pins.net_on(cell_name, "DATAVALID"))
            .map(|s| resolve(s).to_string()),
    })
}

fn parse_verilog_u64(raw: &str) -> Result<u64, String> {
    let s = raw.trim();
    if let Some(idx) = s.find('\'') {
        let rest = s[idx + 1..].trim();
        let mut chars = rest.chars();
        let radix = chars.next().ok_or_else(|| format!("bad constant `{raw}`"))?;
        let digits: String = chars.filter(|&c| c != '_').collect();
        match radix {
            'h' | 'H' => u64::from_str_radix(&digits, 16),
            'b' | 'B' => u64::from_str_radix(&digits, 2),
            'd' | 'D' => digits.parse::<u64>(),
            'o' | 'O' => u64::from_str_radix(&digits, 8),
            _ => return Err(format!("bad radix in `{raw}`")),
        }
        .map_err(|_| format!("bad integer constant `{raw}`"))
    } else if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let clean: String = hex.chars().filter(|&c| c != '_').collect();
        u64::from_str_radix(&clean, 16).map_err(|_| format!("bad hex constant `{raw}`"))
    } else {
        let clean: String = s.chars().filter(|&c| c != '_').collect();
        clean
            .parse::<u64>()
            .or_else(|_| u64::from_str_radix(&clean, 16))
            .map_err(|_| format!("bad numeric constant `{raw}`"))
    }
}

fn parse_hex_256(raw: &str) -> Result<[u64; 4], String> {
    let s = raw.trim();
    let hex_part = if let Some(idx) = s.find('\'') {
        let rest = s[idx + 1..].trim();
        let mut chars = rest.chars();
        let radix = chars.next().ok_or_else(|| format!("bad 256-bit constant `{raw}`"))?;
        if !matches!(radix, 'h' | 'H') {
            return Err(format!("expected hex 256-bit constant in `{raw}`"));
        }
        chars.collect::<String>()
    } else {
        s.strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s)
            .to_string()
    };
    let clean: String = hex_part
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_')
        .collect();
    if clean.is_empty() || clean.len() > 64 || !clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("bad 256-bit hex constant `{raw}`"));
    }
    let padded = format!("{clean:0>64}");
    let mut words = [0u64; 4];
    for (i, slot) in words.iter_mut().enumerate() {
        let end = 64 - i * 16;
        let start = end - 16;
        *slot = u64::from_str_radix(&padded[start..end], 16)
            .map_err(|_| format!("bad hex word in `{raw}`"))?;
    }
    Ok(words)
}

fn parse_bool_attr(attrs: &Attrs, key: &str, default: bool) -> Result<bool, String> {
    let Some(raw) = attrs.get(key) else {
        return Ok(default);
    };
    match raw.trim().to_ascii_uppercase().as_str() {
        "TRUE" | "1" | "1'B1" | "YES" => Ok(true),
        "FALSE" | "0" | "1'B0" | "NO" => Ok(false),
        other => Err(format!("pack: invalid boolean `{other}` for {key}")),
    }
}

fn pack_clock_buf(
    cell_name: &str,
    prim: &str,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedBufg, String> {
    let kind = match prim {
        "BUFG" => ClockBufKind::Bufg,
        "BUFGCE" => ClockBufKind::Bufgce,
        "BUFGCTRL" => ClockBufKind::Bufgctrl,
        "BUFIO" => ClockBufKind::Bufio,
        "BUFR" => ClockBufKind::Bufr,
        _ => return Err(format!("pack: unknown clock buffer `{prim}`")),
    };
    let in_net = pins
        .net_on(cell_name, "I")
        .or_else(|| pins.net_on(cell_name, "I0"))
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no I/I0 net"))?;
    let out_net = pins
        .net_on(cell_name, "O")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no O net"))?;
    let bufr_divide = attrs
        .get("BUFR_DIVIDE")
        .unwrap_or("BYPASS")
        .to_ascii_uppercase();
    if kind == ClockBufKind::Bufr
        && !matches!(
            bufr_divide.as_str(),
            "BYPASS" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8"
        )
    {
        return Err(format!(
            "pack: BUFR {cell_name} has illegal BUFR_DIVIDE `{bufr_divide}` (must be BYPASS or 1..8)"
        ));
    }
    let init_out = attrs
        .get("INIT_OUT")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0) as u8;
    if init_out > 1 {
        return Err(format!(
            "pack: {prim} {cell_name} has illegal INIT_OUT `{init_out}` (must be 0 or 1)"
        ));
    }
    let preselect_i0 = parse_bool_attr(attrs, "PRESELECT_I0", false)?;
    let preselect_i1 = parse_bool_attr(attrs, "PRESELECT_I1", false)?;
    if preselect_i0 && preselect_i1 {
        return Err(format!(
            "pack: BUFGCTRL {cell_name} cannot set both PRESELECT_I0 and PRESELECT_I1 to TRUE"
        ));
    }
    Ok(PackedBufg {
        name: cell_name.to_string(),
        in_net,
        in1_net: pins.net_on(cell_name, "I1").map(|s| resolve(s).to_string()),
        ce0_net: pins
            .net_on(cell_name, "CE0")
            .or_else(|| pins.net_on(cell_name, "CE"))
            .map(|s| resolve(s).to_string()),
        ce1_net: pins
            .net_on(cell_name, "CE1")
            .map(|s| resolve(s).to_string()),
        s0_net: pins.net_on(cell_name, "S0").map(|s| resolve(s).to_string()),
        s1_net: pins.net_on(cell_name, "S1").map(|s| resolve(s).to_string()),
        ignore0_net: pins
            .net_on(cell_name, "IGNORE0")
            .map(|s| resolve(s).to_string()),
        ignore1_net: pins
            .net_on(cell_name, "IGNORE1")
            .map(|s| resolve(s).to_string()),
        clr_net: pins
            .net_on(cell_name, "CLR")
            .map(|s| resolve(s).to_string()),
        out_net,
        kind,
        bufr_divide,
        init_out,
        preselect_i0,
        preselect_i1,
        bank: attrs.get("BANK").map(|s| s.to_string()),
    })
}

fn pack_cmt(
    cell_name: &str,
    kind: CmtKind,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedCmt, String> {
    let prim_name = match kind {
        CmtKind::MmcmE2Adv => "MMCME2_ADV",
        CmtKind::PllE2Adv => "PLLE2_ADV",
    };
    let bandwidth = attrs
        .get("BANDWIDTH")
        .unwrap_or("OPTIMIZED")
        .to_ascii_uppercase();
    if !matches!(bandwidth.as_str(), "OPTIMIZED" | "HIGH" | "LOW") {
        return Err(format!(
            "pack: {prim_name} {cell_name}: invalid BANDWIDTH `{bandwidth}`"
        ));
    }
    let compensation = attrs
        .get("COMPENSATION")
        .unwrap_or("ZHOLD")
        .to_ascii_uppercase();
    if !matches!(
        compensation.as_str(),
        "ZHOLD" | "BUF_IN" | "EXTERNAL" | "INTERNAL"
    ) {
        return Err(format!(
            "pack: {prim_name} {cell_name}: invalid COMPENSATION `{compensation}`"
        ));
    }

    let clkin1_period_ns: f64 = attrs
        .get("CLKIN1_PERIOD")
        .unwrap_or("10.0")
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim_name} {cell_name}: bad CLKIN1_PERIOD"))?;
    let clkin2_period_ns: f64 = attrs
        .get("CLKIN2_PERIOD")
        .unwrap_or("10.0")
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim_name} {cell_name}: bad CLKIN2_PERIOD"))?;
    if clkin1_period_ns <= 0.0 {
        return Err(format!(
            "pack: {prim_name} {cell_name}: CLKIN1_PERIOD ({clkin1_period_ns}) must be > 0"
        ));
    }

    let divclk_divide: u32 = attrs
        .get("DIVCLK_DIVIDE")
        .unwrap_or("1")
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim_name} {cell_name}: bad DIVCLK_DIVIDE"))?;
    let max_divclk = match kind {
        CmtKind::MmcmE2Adv => 106,
        CmtKind::PllE2Adv => 56,
    };
    if divclk_divide == 0 || divclk_divide > max_divclk {
        return Err(format!(
            "pack: {prim_name} {cell_name}: DIVCLK_DIVIDE {divclk_divide} out of range 1..={max_divclk}"
        ));
    }

    let mult_str = attrs
        .get("CLKFBOUT_MULT_F")
        .or_else(|| attrs.get("CLKFBOUT_MULT"))
        .unwrap_or("10.0");
    let clkfbout_mult_f: f64 = mult_str
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim_name} {cell_name}: bad CLKFBOUT_MULT `{mult_str}`"))?;
    if !(2.0..=64.0).contains(&clkfbout_mult_f) {
        return Err(format!(
            "pack: {prim_name} {cell_name}: CLKFBOUT_MULT {clkfbout_mult_f} out of range 2.0..=64.0"
        ));
    }
    if kind == CmtKind::PllE2Adv && (clkfbout_mult_f.fract() != 0.0) {
        return Err(format!(
            "pack: PLLE2_ADV {cell_name}: CLKFBOUT_MULT must be an integer, got {clkfbout_mult_f}"
        ));
    }

    let clkfbout_phase: f64 = attrs
        .get("CLKFBOUT_PHASE")
        .unwrap_or("0.0")
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim_name} {cell_name}: bad CLKFBOUT_PHASE"))?;

    // Validate VCO frequency: f_vco = (1000.0 / clkin1_period_ns) * clkfbout_mult_f / divclk_divide.
    // 7-Series Artix-7 speed grade -1/-2/-3: MMCM VCO 600..=1600 MHz, PLL VCO 800..=2133 MHz.
    let fin_mhz = 1000.0 / clkin1_period_ns;
    let vco_freq_mhz = fin_mhz * clkfbout_mult_f / (divclk_divide as f64);
    let (vco_min, vco_max) = match kind {
        CmtKind::MmcmE2Adv => (600.0, 1600.0),
        CmtKind::PllE2Adv => (800.0, 2133.0),
    };
    if !(vco_min..=vco_max).contains(&vco_freq_mhz) {
        return Err(format!(
            "pack: {prim_name} {cell_name}: VCO frequency {vco_freq_mhz:.2} MHz out of legal range {vco_min}..={vco_max} MHz"
        ));
    }

    let clkout0_str = attrs
        .get("CLKOUT0_DIVIDE_F")
        .or_else(|| attrs.get("CLKOUT0_DIVIDE"))
        .unwrap_or("1.0");
    let clkout0_divide_f: f64 = clkout0_str
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim_name} {cell_name}: bad CLKOUT0_DIVIDE `{clkout0_str}`"))?;
    if !(1.0..=128.0).contains(&clkout0_divide_f) {
        return Err(format!(
            "pack: {prim_name} {cell_name}: CLKOUT0_DIVIDE {clkout0_divide_f} out of range 1..=128"
        ));
    }

    let mut clkout_divide = [1u32; 7];
    clkout_divide[0] = clkout0_divide_f.round() as u32;
    let mut clkout_phase = [0.0f64; 7];
    let mut clkout_duty = [0.5f64; 7];
    for i in 0..7 {
        if i > 0 {
            let key = format!("CLKOUT{i}_DIVIDE");
            let div: u32 = attrs
                .get(&key)
                .unwrap_or("1")
                .trim()
                .parse()
                .map_err(|_| format!("pack: {prim_name} {cell_name}: bad {key}"))?;
            if !(1..=128).contains(&div) {
                return Err(format!(
                    "pack: {prim_name} {cell_name}: {key}={div} out of range 1..=128"
                ));
            }
            clkout_divide[i] = div;
        }
        let pkey = format!("CLKOUT{i}_PHASE");
        clkout_phase[i] = attrs
            .get(&pkey)
            .unwrap_or("0.0")
            .trim()
            .parse()
            .map_err(|_| format!("pack: {prim_name} {cell_name}: bad {pkey}"))?;
        let dkey = format!("CLKOUT{i}_DUTY_CYCLE");
        let duty: f64 = attrs
            .get(&dkey)
            .unwrap_or("0.5")
            .trim()
            .parse()
            .map_err(|_| format!("pack: {prim_name} {cell_name}: bad {dkey}"))?;
        if !(0.001..=0.999).contains(&duty) {
            return Err(format!(
                "pack: {prim_name} {cell_name}: {dkey}={duty} out of range (0.0, 1.0)"
            ));
        }
        clkout_duty[i] = duty;
    }

    let mut clkout_nets: [Option<String>; 7] = Default::default();
    for (i, slot) in clkout_nets.iter_mut().enumerate() {
        *slot = pins
            .net_on(cell_name, &format!("CLKOUT{i}"))
            .map(|s| resolve(s).to_string());
    }

    Ok(PackedCmt {
        name: cell_name.to_string(),
        kind,
        bandwidth,
        compensation,
        clkin1_period_ns,
        clkin2_period_ns,
        divclk_divide,
        clkfbout_mult_f,
        clkfbout_phase,
        vco_freq_mhz,
        clkout0_divide_f,
        clkout_divide,
        clkout_phase,
        clkout_duty,
        clkin1_net: pins
            .net_on(cell_name, "CLKIN1")
            .map(|s| resolve(s).to_string()),
        clkin2_net: pins
            .net_on(cell_name, "CLKIN2")
            .map(|s| resolve(s).to_string()),
        clkfbin_net: pins
            .net_on(cell_name, "CLKFBIN")
            .map(|s| resolve(s).to_string()),
        clkfbout_net: pins
            .net_on(cell_name, "CLKFBOUT")
            .map(|s| resolve(s).to_string()),
        clkout_nets,
        locked_net: pins
            .net_on(cell_name, "LOCKED")
            .map(|s| resolve(s).to_string()),
        rst_net: pins
            .net_on(cell_name, "RST")
            .map(|s| resolve(s).to_string()),
        pwrdwn_net: pins
            .net_on(cell_name, "PWRDWN")
            .map(|s| resolve(s).to_string()),
    })
}

fn parse_write_mode(raw: &str, cell_name: &str) -> Result<RamWriteMode, String> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "WRITE_FIRST" => Ok(RamWriteMode::WriteFirst),
        "READ_FIRST" => Ok(RamWriteMode::ReadFirst),
        "NO_CHANGE" => Ok(RamWriteMode::NoChange),
        other => Err(format!(
            "pack: BRAM {cell_name}: invalid WRITE_MODE `{other}`"
        )),
    }
}

fn parse_cascade_order(raw: &str, cell_name: &str) -> Result<CascadeOrder, String> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "NONE" | "" => Ok(CascadeOrder::None),
        "FIRST" | "LOWER" => Ok(CascadeOrder::First),
        "MIDDLE" | "BOTH" => Ok(CascadeOrder::Middle),
        "LAST" | "UPPER" => Ok(CascadeOrder::Last),
        other => Err(format!(
            "pack: BRAM {cell_name}: invalid CASCADE_ORDER / RAM_EXTENSION `{other}`"
        )),
    }
}

fn pack_bram(
    cell_name: &str,
    kind: BramKind,
    widths_override: Option<(u8, u8, u8, u8)>,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedBram, String> {
    let init: Vec<u64> = attrs
        .get("INIT")
        .unwrap_or("")
        .split(',')
        .filter(|t| !t.trim().is_empty())
        .map(|t| u64::from_str_radix(t.trim(), 16).unwrap_or(0))
        .collect();

    let parse_w = |key: &str, def: u8| -> Result<u8, String> {
        match attrs.get(key) {
            Some(v) => v
                .trim()
                .parse::<u8>()
                .map_err(|_| format!("pack: BRAM {cell_name}: bad {key}=`{v}`")),
            None => Ok(def),
        }
    };
    let (ra, rb, wa, wb) = match widths_override {
        Some((ra, rb, wa, wb)) => (
            parse_w("READ_WIDTH_A", ra)?,
            parse_w("READ_WIDTH_B", rb)?,
            parse_w("WRITE_WIDTH_A", wa)?,
            parse_w("WRITE_WIDTH_B", wb)?,
        ),
        None => (
            parse_w("READ_WIDTH_A", 18)?,
            parse_w("READ_WIDTH_B", 18)?,
            parse_w("WRITE_WIDTH_A", 18)?,
            parse_w("WRITE_WIDTH_B", 18)?,
        ),
    };

    let ram_mode = match attrs
        .get("RAM_MODE")
        .unwrap_or("TDP")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "TDP" => RamMode::Tdp,
        "SDP" => RamMode::Sdp,
        other => {
            return Err(format!(
                "pack: BRAM {cell_name}: invalid RAM_MODE `{other}` (expected TDP or SDP)"
            ))
        }
    };

    let valid_width = |w: u8| -> bool {
        match (kind, ram_mode) {
            (BramKind::Ramb18e1, RamMode::Tdp) => matches!(w, 0 | 1 | 2 | 4 | 9 | 18),
            (BramKind::Ramb18e1, RamMode::Sdp) => matches!(w, 0 | 1 | 2 | 4 | 9 | 18 | 36),
            (BramKind::Ramb36e1, RamMode::Tdp) => matches!(w, 0 | 1 | 2 | 4 | 9 | 18 | 36),
            (BramKind::Ramb36e1, RamMode::Sdp) => matches!(w, 0 | 1 | 2 | 4 | 9 | 18 | 36 | 72),
        }
    };
    for (wname, w) in [
        ("READ_WIDTH_A", ra),
        ("READ_WIDTH_B", rb),
        ("WRITE_WIDTH_A", wa),
        ("WRITE_WIDTH_B", wb),
    ] {
        if !valid_width(w) {
            return Err(format!(
                "pack: BRAM {cell_name} ({kind:?}/{ram_mode:?}): illegal {wname}={w}"
            ));
        }
    }

    let write_mode_a = parse_write_mode(attrs.get("WRITE_MODE_A").unwrap_or("WRITE_FIRST"), cell_name)?;
    let write_mode_b = parse_write_mode(attrs.get("WRITE_MODE_B").unwrap_or("WRITE_FIRST"), cell_name)?;
    let doa_reg = parse_w("DOA_REG", 0)?;
    let dob_reg = parse_w("DOB_REG", 0)?;
    if doa_reg > 1 || dob_reg > 1 {
        return Err(format!(
            "pack: BRAM {cell_name}: DOA_REG/DOB_REG must be 0 or 1"
        ));
    }

    let max_init_rows: u8 = match kind {
        BramKind::Ramb18e1 => 0x3F,
        BramKind::Ramb36e1 => 0x7F,
    };
    let max_initp_rows: u8 = match kind {
        BramKind::Ramb18e1 => 0x07,
        BramKind::Ramb36e1 => 0x0F,
    };
    let mut init_rows = BTreeMap::new();
    let mut initp_rows = BTreeMap::new();
    for (k, v) in &attrs.map {
        let up = k.to_ascii_uppercase();
        if let Some(hex_idx) = up.strip_prefix("INIT_") {
            if hex_idx.len() == 2 && hex_idx.chars().all(|c| c.is_ascii_hexdigit()) {
                let row = u8::from_str_radix(hex_idx, 16).unwrap_or(0);
                if row > max_init_rows {
                    return Err(format!(
                        "pack: BRAM {cell_name}: {up} exceeds max row INIT_{max_init_rows:02X}"
                    ));
                }
                init_rows.insert(row, parse_hex_256(v)?);
            }
        } else if let Some(hex_idx) = up.strip_prefix("INITP_") {
            if hex_idx.len() == 2 && hex_idx.chars().all(|c| c.is_ascii_hexdigit()) {
                let row = u8::from_str_radix(hex_idx, 16).unwrap_or(0);
                if row > max_initp_rows {
                    return Err(format!(
                        "pack: BRAM {cell_name}: {up} exceeds max parity row INITP_{max_initp_rows:02X}"
                    ));
                }
                initp_rows.insert(row, parse_hex_256(v)?);
            }
        }
    }

    let init_a = attrs
        .get("INIT_A")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0);
    let init_b = attrs
        .get("INIT_B")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0);
    let srval_a = attrs
        .get("SRVAL_A")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0);
    let srval_b = attrs
        .get("SRVAL_B")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0);

    let cascade_order_a = parse_cascade_order(
        attrs
            .get("CASCADE_ORDER_A")
            .or_else(|| attrs.get("RAM_EXTENSION_A"))
            .unwrap_or("NONE"),
        cell_name,
    )?;
    let cascade_order_b = parse_cascade_order(
        attrs
            .get("CASCADE_ORDER_B")
            .or_else(|| attrs.get("RAM_EXTENSION_B"))
            .unwrap_or("NONE"),
        cell_name,
    )?;

    let mut wea_nets = Vec::new();
    let mut web_nets = Vec::new();
    if let Some(n) = pins.net_on(cell_name, "WEA") {
        wea_nets.push(resolve(n).to_string());
    }
    for i in 0..4 {
        if let Some(n) = pins
            .net_on(cell_name, &format!("WEA{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("WEA[{i}]")))
        {
            wea_nets.push(resolve(n).to_string());
        }
    }
    if let Some(n) = pins
        .net_on(cell_name, "WEBWE")
        .or_else(|| pins.net_on(cell_name, "WEB"))
    {
        web_nets.push(resolve(n).to_string());
    }
    for i in 0..8 {
        if let Some(n) = pins
            .net_on(cell_name, &format!("WEBWE{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("WEBWE[{i}]")))
            .or_else(|| pins.net_on(cell_name, &format!("WEB{i}")))
            .or_else(|| pins.net_on(cell_name, &format!("WEB[{i}]")))
        {
            web_nets.push(resolve(n).to_string());
        }
    }

    Ok(PackedBram {
        name: cell_name.to_string(),
        init,
        read_width_a: ra,
        read_width_b: rb,
        write_width_a: wa,
        write_width_b: wb,
        kind,
        ram_mode,
        write_mode_a,
        write_mode_b,
        doa_reg,
        dob_reg,
        init_rows,
        initp_rows,
        init_a,
        init_b,
        srval_a,
        srval_b,
        cascade_order_a,
        cascade_order_b,
        cascade_in_a: pins
            .net_on(cell_name, "CASCADEINA")
            .map(|s| resolve(s).to_string()),
        cascade_out_a: pins
            .net_on(cell_name, "CASCADEOUTA")
            .map(|s| resolve(s).to_string()),
        cascade_in_b: pins
            .net_on(cell_name, "CASCADEINB")
            .map(|s| resolve(s).to_string()),
        cascade_out_b: pins
            .net_on(cell_name, "CASCADEOUTB")
            .map(|s| resolve(s).to_string()),
        wea_nets,
        web_nets,
        clka_net: pins
            .net_on(cell_name, "CLKARDCLK")
            .or_else(|| pins.net_on(cell_name, "CLKA"))
            .map(|s| resolve(s).to_string()),
        clkb_net: pins
            .net_on(cell_name, "CLKBWRCLK")
            .or_else(|| pins.net_on(cell_name, "CLKB"))
            .map(|s| resolve(s).to_string()),
    })
}

fn pack_dsp(
    cell_name: &str,
    modes_override: Option<(u8, u8)>,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedDsp, String> {
    let (def_opmode, def_alumode) = modes_override.unwrap_or((0b0000101, 0b0000));
    let opmode = attrs
        .get("OPMODE")
        .map(parse_verilog_u64)
        .transpose()?
        .map(|v| v as u8)
        .unwrap_or(def_opmode)
        & 0x7F;
    let alumode = attrs
        .get("ALUMODE")
        .map(parse_verilog_u64)
        .transpose()?
        .map(|v| v as u8)
        .unwrap_or(def_alumode)
        & 0x0F;
    let inmode = attrs
        .get("INMODE")
        .map(parse_verilog_u64)
        .transpose()?
        .map(|v| v as u8)
        .unwrap_or(0)
        & 0x1F;

    let parse_reg = |key: &str, def: u8, max: u8| -> Result<u8, String> {
        let val = match attrs.get(key) {
            Some(v) => parse_verilog_u64(v)? as u8,
            None => def,
        };
        if val > max {
            return Err(format!(
                "pack: DSP48E1 {cell_name}: {key}={val} exceeds max {max}"
            ));
        }
        Ok(val)
    };

    let areg = parse_reg("AREG", 0, 2)?;
    let breg = parse_reg("BREG", 0, 2)?;
    let creg = parse_reg("CREG", 0, 1)?;
    let dreg = parse_reg("DREG", 0, 1)?;
    let adreg = parse_reg("ADREG", 0, 1)?;
    let mreg = parse_reg("MREG", 0, 1)?;
    let preg = parse_reg("PREG", 0, 1)?;
    let opmodereg = parse_reg("OPMODEREG", 0, 1)?;
    let alumodereg = parse_reg("ALUMODEREG", 0, 1)?;
    let inmodereg = parse_reg("INMODEREG", 0, 1)?;
    let carryinreg = parse_reg("CARRYINREG", 0, 1)?;
    let carryinselreg = parse_reg("CARRYINSELREG", 0, 1)?;

    let use_mult = attrs
        .get("USE_MULT")
        .unwrap_or("MULTIPLY")
        .trim()
        .to_ascii_uppercase();
    if !matches!(use_mult.as_str(), "MULTIPLY" | "DYNAMIC" | "NONE") {
        return Err(format!(
            "pack: DSP48E1 {cell_name}: invalid USE_MULT `{use_mult}`"
        ));
    }
    let use_dport = parse_bool_attr(attrs, "USE_DPORT", false)?;
    let use_simd = attrs
        .get("USE_SIMD")
        .unwrap_or("ONE48")
        .trim()
        .to_ascii_uppercase();
    if !matches!(use_simd.as_str(), "ONE48" | "TWO24" | "FOUR12") {
        return Err(format!(
            "pack: DSP48E1 {cell_name}: invalid USE_SIMD `{use_simd}`"
        ));
    }
    if use_simd != "ONE48" && use_mult != "NONE" {
        return Err(format!(
            "pack: DSP48E1 {cell_name}: USE_SIMD `{use_simd}` requires USE_MULT=\"NONE\""
        ));
    }

    let use_pattern_detect = match attrs
        .get("USE_PATTERN_DETECT")
        .unwrap_or("NO_PATDET")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "PATDET" | "TRUE" | "1" => true,
        "NO_PATDET" | "FALSE" | "0" => false,
        other => {
            return Err(format!(
                "pack: DSP48E1 {cell_name}: invalid USE_PATTERN_DETECT `{other}`"
            ))
        }
    };
    let pattern = attrs
        .get("PATTERN")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0)
        & 0xFFFF_FFFF_FFFF;
    let mask = attrs
        .get("MASK")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0x3FFF_FFFF_FFFF)
        & 0xFFFF_FFFF_FFFF;
    let sel_pattern = attrs
        .get("SEL_PATTERN")
        .unwrap_or("PATTERN")
        .trim()
        .to_ascii_uppercase();
    let sel_mask = attrs
        .get("SEL_MASK")
        .unwrap_or("MASK")
        .trim()
        .to_ascii_uppercase();

    let a_input_cascade = match attrs
        .get("A_INPUT")
        .unwrap_or("DIRECT")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "DIRECT" => false,
        "CASCADE" => true,
        other => return Err(format!("pack: DSP48E1 {cell_name}: invalid A_INPUT `{other}`")),
    };
    let b_input_cascade = match attrs
        .get("B_INPUT")
        .unwrap_or("DIRECT")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "DIRECT" => false,
        "CASCADE" => true,
        other => return Err(format!("pack: DSP48E1 {cell_name}: invalid B_INPUT `{other}`")),
    };

    Ok(PackedDsp {
        name: cell_name.to_string(),
        opmode,
        alumode,
        inmode,
        areg,
        breg,
        creg,
        dreg,
        adreg,
        mreg,
        preg,
        opmodereg,
        alumodereg,
        inmodereg,
        carryinreg,
        carryinselreg,
        use_mult,
        use_dport,
        use_simd,
        use_pattern_detect,
        pattern,
        mask,
        sel_pattern,
        sel_mask,
        a_input_cascade,
        b_input_cascade,
        pcin_net: pins.net_on(cell_name, "PCIN").map(|s| resolve(s).to_string()),
        pcout_net: pins
            .net_on(cell_name, "PCOUT")
            .map(|s| resolve(s).to_string()),
        acin_net: pins.net_on(cell_name, "ACIN").map(|s| resolve(s).to_string()),
        acout_net: pins
            .net_on(cell_name, "ACOUT")
            .map(|s| resolve(s).to_string()),
        bcin_net: pins.net_on(cell_name, "BCIN").map(|s| resolve(s).to_string()),
        bcout_net: pins
            .net_on(cell_name, "BCOUT")
            .map(|s| resolve(s).to_string()),
        carrycascin_net: pins
            .net_on(cell_name, "CARRYCASCIN")
            .map(|s| resolve(s).to_string()),
        carrycascout_net: pins
            .net_on(cell_name, "CARRYCASCOUT")
            .map(|s| resolve(s).to_string()),
        clk_net: pins.net_on(cell_name, "CLK").map(|s| resolve(s).to_string()),
    })
}

fn pack_carry4(cell_name: &str, pins: &PinIndex) -> Result<PackedCarry, String> {
    let mut di_nets: [Option<String>; 4] = Default::default();
    let mut s_nets: [Option<String>; 4] = Default::default();
    let mut o_nets: [Option<String>; 4] = Default::default();
    let mut co_nets: [Option<String>; 4] = Default::default();
    for i in 0..4 {
        di_nets[i] = pins
            .net_on(cell_name, &format!("DI{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("DI[{i}]")))
            .map(|s| resolve(s).to_string());
        s_nets[i] = pins
            .net_on(cell_name, &format!("S{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("S[{i}]")))
            .map(|s| resolve(s).to_string());
        o_nets[i] = pins
            .net_on(cell_name, &format!("O{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("O[{i}]")))
            .map(|s| resolve(s).to_string());
        co_nets[i] = pins
            .net_on(cell_name, &format!("CO{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("CO[{i}]")))
            .map(|s| resolve(s).to_string());
    }
    Ok(PackedCarry {
        name: cell_name.to_string(),
        ci_net: pins.net_on(cell_name, "CI").map(|s| resolve(s).to_string()),
        cyinit_net: pins
            .net_on(cell_name, "CYINIT")
            .map(|s| resolve(s).to_string()),
        di_nets,
        s_nets,
        o_nets,
        co_nets,
    })
}

fn infer_io_affinity(netlist: &Design, attrs: &Attrs, net: Option<&str>) -> (Option<String>, Option<String>) {
    let mut bank = attrs.get("BANK").map(|s| s.to_string());
    let mut loc = attrs.get("LOC").map(|s| s.to_string());
    if let Some(n) = net {
        for p in &netlist.ports {
            if resolve(p.name) == n {
                if loc.is_none() {
                    loc = p.attrs.get("LOC").map(|s| s.to_string());
                }
                if bank.is_none() {
                    bank = p.attrs.get("BANK").map(|s| s.to_string());
                }
            }
        }
    }
    (bank, loc)
}

fn pack_ioddr(
    cell_name: &str,
    is_input: bool,
    netlist: &Design,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedIoddr, String> {
    let prim = if is_input { "IDDR" } else { "ODDR" };
    let ddr_clk_edge = match attrs
        .get("DDR_CLK_EDGE")
        .unwrap_or("OPPOSITE_EDGE")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "OPPOSITE_EDGE" => DdrClkEdge::OppositeEdge,
        "SAME_EDGE" => DdrClkEdge::SameEdge,
        "SAME_EDGE_PIPELINED" => {
            if !is_input {
                return Err(format!(
                    "pack: ODDR {cell_name}: SAME_EDGE_PIPELINED is only valid on IDDR"
                ));
            }
            DdrClkEdge::SameEdgePipelined
        }
        other => {
            return Err(format!(
                "pack: {prim} {cell_name}: invalid DDR_CLK_EDGE `{other}`"
            ))
        }
    };
    let srtype = match attrs
        .get("SRTYPE")
        .unwrap_or("SYNC")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "SYNC" => SrType::Sync,
        "ASYNC" => SrType::Async,
        other => return Err(format!("pack: {prim} {cell_name}: invalid SRTYPE `{other}`")),
    };
    let init_q1 = attrs
        .get(if is_input { "INIT_Q1" } else { "INIT" })
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0) as u8
        & 1;
    let init_q2 = attrs
        .get("INIT_Q2")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0) as u8
        & 1;

    let c_net = pins
        .net_on(cell_name, "C")
        .or_else(|| pins.net_on(cell_name, "CLK"))
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no C clock net"))?;
    let ce_net = pins.net_on(cell_name, "CE").map(|s| resolve(s).to_string());
    let sr_net = pins
        .net_on(cell_name, "R")
        .or_else(|| pins.net_on(cell_name, "S"))
        .or_else(|| pins.net_on(cell_name, "SR"))
        .map(|s| resolve(s).to_string());

    let d_net = pins.net_on(cell_name, "D").map(|s| resolve(s).to_string());
    let d1_net = pins.net_on(cell_name, "D1").map(|s| resolve(s).to_string());
    let d2_net = pins.net_on(cell_name, "D2").map(|s| resolve(s).to_string());
    let q_net = pins.net_on(cell_name, "Q").map(|s| resolve(s).to_string());
    let q1_net = pins.net_on(cell_name, "Q1").map(|s| resolve(s).to_string());
    let q2_net = pins.net_on(cell_name, "Q2").map(|s| resolve(s).to_string());

    if is_input && d_net.is_none() {
        return Err(format!("pack: IDDR {cell_name} has no D input net"));
    }
    if !is_input && q_net.is_none() {
        return Err(format!("pack: ODDR {cell_name} has no Q output net"));
    }

    let pad_side = if is_input {
        d_net.as_deref()
    } else {
        q_net.as_deref()
    };
    let (bank, loc) = infer_io_affinity(netlist, attrs, pad_side);

    Ok(PackedIoddr {
        name: cell_name.to_string(),
        is_input,
        ddr_clk_edge,
        srtype,
        init_q1,
        init_q2,
        c_net,
        ce_net,
        sr_net,
        d_net,
        d1_net,
        d2_net,
        q_net,
        q1_net,
        q2_net,
        bank,
        loc,
    })
}

fn pack_serdes(
    cell_name: &str,
    is_input: bool,
    netlist: &Design,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedSerdes, String> {
    let prim = if is_input { "ISERDESE2" } else { "OSERDESE2" };
    let data_rate = attrs
        .get("DATA_RATE")
        .or_else(|| attrs.get("DATA_RATE_OQ"))
        .unwrap_or("DDR")
        .trim()
        .to_ascii_uppercase();
    if !matches!(data_rate.as_str(), "SDR" | "DDR") {
        return Err(format!(
            "pack: {prim} {cell_name}: invalid DATA_RATE `{data_rate}` (must be SDR or DDR)"
        ));
    }
    let data_width = attrs
        .get("DATA_WIDTH")
        .unwrap_or("4")
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("pack: {prim} {cell_name}: bad DATA_WIDTH"))?;
    let interface_type = attrs
        .get("INTERFACE_TYPE")
        .unwrap_or("NETWORKING")
        .trim()
        .to_ascii_uppercase();
    let serdes_mode = attrs
        .get("SERDES_MODE")
        .unwrap_or("MASTER")
        .trim()
        .to_ascii_uppercase();
    if !matches!(serdes_mode.as_str(), "MASTER" | "SLAVE") {
        return Err(format!(
            "pack: {prim} {cell_name}: invalid SERDES_MODE `{serdes_mode}`"
        ));
    }

    let valid_width = match (is_input, data_rate.as_str(), interface_type.as_str()) {
        (true, "SDR", "NETWORKING") => matches!(data_width, 2..=8),
        (true, "DDR", "NETWORKING") => matches!(data_width, 4 | 6 | 8 | 10 | 14),
        (true, "DDR", "MEMORY" | "MEMORY_DDR3" | "MEMORY_QDR" | "OVERSAMPLE") => {
            matches!(data_width, 4)
        }
        (false, "SDR", _) => matches!(data_width, 2..=8),
        (false, "DDR", _) => matches!(data_width, 4 | 6 | 8 | 10 | 14),
        _ => false,
    };
    if !valid_width {
        return Err(format!(
            "pack: {prim} {cell_name}: illegal (DATA_RATE={data_rate}, DATA_WIDTH={data_width}, INTERFACE_TYPE={interface_type})"
        ));
    }

    let iobdelay = attrs
        .get("IOBDELAY")
        .unwrap_or("NONE")
        .trim()
        .to_ascii_uppercase();
    if !matches!(iobdelay.as_str(), "NONE" | "IBUF" | "IFD" | "BOTH") {
        return Err(format!(
            "pack: {prim} {cell_name}: invalid IOBDELAY `{iobdelay}`"
        ));
    }
    let tristate_width = attrs
        .get("TRISTATE_WIDTH")
        .unwrap_or("1")
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("pack: {prim} {cell_name}: bad TRISTATE_WIDTH"))?;
    if !matches!(tristate_width, 1 | 4) {
        return Err(format!(
            "pack: {prim} {cell_name}: invalid TRISTATE_WIDTH={tristate_width} (must be 1 or 4)"
        ));
    }

    let clk_net = pins
        .net_on(cell_name, "CLK")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no CLK net"))?;
    let clkdiv_net = pins
        .net_on(cell_name, "CLKDIV")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no CLKDIV net"))?;
    let rst_net = pins.net_on(cell_name, "RST").map(|s| resolve(s).to_string());

    let serial_net = if is_input {
        pins.net_on(cell_name, "DDLY")
            .or_else(|| pins.net_on(cell_name, "D"))
            .map(|s| resolve(s).to_string())
    } else {
        pins.net_on(cell_name, "OQ")
            .or_else(|| pins.net_on(cell_name, "OFB"))
            .map(|s| resolve(s).to_string())
    };
    let mut parallel_nets: [Option<String>; 8] = Default::default();
    for i in 0..8 {
        let p = if is_input {
            format!("Q{}", i + 1)
        } else {
            format!("D{}", i + 1)
        };
        parallel_nets[i] = pins.net_on(cell_name, &p).map(|s| resolve(s).to_string());
    }

    let (bank, loc) = infer_io_affinity(netlist, attrs, serial_net.as_deref());

    Ok(PackedSerdes {
        name: cell_name.to_string(),
        is_input,
        data_rate,
        data_width,
        interface_type,
        serdes_mode,
        iobdelay,
        tristate_width,
        clk_net,
        clkdiv_net,
        rst_net,
        serial_net,
        parallel_nets,
        shiftin1_net: pins
            .net_on(cell_name, "SHIFTIN1")
            .map(|s| resolve(s).to_string()),
        shiftin2_net: pins
            .net_on(cell_name, "SHIFTIN2")
            .map(|s| resolve(s).to_string()),
        shiftout1_net: pins
            .net_on(cell_name, "SHIFTOUT1")
            .map(|s| resolve(s).to_string()),
        shiftout2_net: pins
            .net_on(cell_name, "SHIFTOUT2")
            .map(|s| resolve(s).to_string()),
        bank,
        loc,
    })
}

fn pack_iodelay(
    cell_name: &str,
    kind: IodelayKind,
    netlist: &Design,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedIodelay, String> {
    let prim = match kind {
        IodelayKind::IdelayE2 => "IDELAYE2",
        IodelayKind::OdelayE2 => "ODELAYE2",
        IodelayKind::IdelayCtrl => "IDELAYCTRL",
    };
    let iodelay_group = attrs
        .get("IODELAY_GROUP")
        .unwrap_or("IODELAY_DEFAULT")
        .trim()
        .to_string();

    if kind == IodelayKind::IdelayCtrl {
        let refclk = pins
            .net_on(cell_name, "REFCLK")
            .map(|s| resolve(s).to_string())
            .ok_or_else(|| format!("pack: IDELAYCTRL {cell_name} has no REFCLK net"))?;
        let (bank, loc) = infer_io_affinity(netlist, attrs, None);
        return Ok(PackedIodelay {
            name: cell_name.to_string(),
            kind,
            delay_type: "FIXED".to_string(),
            delay_value: 0,
            delay_src: "REFCLK".to_string(),
            refclk_frequency: 200.0,
            high_performance_mode: false,
            cinvctrl_sel: false,
            pipe_sel: false,
            iodelay_group,
            c_net: Some(refclk),
            rst_net: pins.net_on(cell_name, "RST").map(|s| resolve(s).to_string()),
            ce_net: None,
            inc_net: None,
            ld_net: None,
            data_in_net: None,
            data_out_net: None,
            cntvaluein_nets: Default::default(),
            cntvalueout_nets: Default::default(),
            rdy_net: pins.net_on(cell_name, "RDY").map(|s| resolve(s).to_string()),
            bank,
            loc,
        });
    }

    let type_key = if kind == IodelayKind::IdelayE2 {
        "IDELAY_TYPE"
    } else {
        "ODELAY_TYPE"
    };
    let val_key = if kind == IodelayKind::IdelayE2 {
        "IDELAY_VALUE"
    } else {
        "ODELAY_VALUE"
    };
    let delay_type = attrs
        .get(type_key)
        .unwrap_or("FIXED")
        .trim()
        .to_ascii_uppercase();
    if !matches!(
        delay_type.as_str(),
        "FIXED" | "VARIABLE" | "VAR_LOAD" | "VAR_LOAD_PIPE"
    ) {
        return Err(format!(
            "pack: {prim} {cell_name}: invalid {type_key} `{delay_type}`"
        ));
    }
    let delay_value = attrs
        .get(val_key)
        .unwrap_or("0")
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("pack: {prim} {cell_name}: bad {val_key}"))?;
    if delay_value > 31 {
        return Err(format!(
            "pack: {prim} {cell_name}: {val_key}={delay_value} exceeds 32-tap range 0..=31"
        ));
    }

    let def_src = if kind == IodelayKind::IdelayE2 {
        "IDATAIN"
    } else {
        "ODATAIN"
    };
    let delay_src = attrs
        .get("DELAY_SRC")
        .unwrap_or(def_src)
        .trim()
        .to_ascii_uppercase();
    if !matches!(
        delay_src.as_str(),
        "IDATAIN" | "DATAIN" | "ODATAIN" | "CLKIN"
    ) {
        return Err(format!(
            "pack: {prim} {cell_name}: invalid DELAY_SRC `{delay_src}`"
        ));
    }

    let refclk_frequency: f64 = attrs
        .get("REFCLK_FREQUENCY")
        .unwrap_or("200.0")
        .trim()
        .parse()
        .map_err(|_| format!("pack: {prim} {cell_name}: bad REFCLK_FREQUENCY"))?;
    let valid_refclk = (190.0..=210.0).contains(&refclk_frequency)
        || (290.0..=310.0).contains(&refclk_frequency)
        || (390.0..=410.0).contains(&refclk_frequency);
    if !valid_refclk {
        return Err(format!(
            "pack: {prim} {cell_name}: REFCLK_FREQUENCY {refclk_frequency} MHz outside calibrated ranges (200/300/400 MHz +/- 10 MHz)"
        ));
    }

    let high_performance_mode = parse_bool_attr(attrs, "HIGH_PERFORMANCE_MODE", false)?;
    let cinvctrl_sel = parse_bool_attr(attrs, "CINVCTRL_SEL", false)?;
    let pipe_sel = parse_bool_attr(attrs, "PIPE_SEL", false)?;
    if pipe_sel && delay_type != "VAR_LOAD_PIPE" {
        return Err(format!(
            "pack: {prim} {cell_name}: PIPE_SEL=TRUE requires {type_key}=VAR_LOAD_PIPE"
        ));
    }

    let data_in_net = pins
        .net_on(cell_name, "IDATAIN")
        .or_else(|| pins.net_on(cell_name, "ODATAIN"))
        .or_else(|| pins.net_on(cell_name, "DATAIN"))
        .map(|s| resolve(s).to_string());
    let data_out_net = pins
        .net_on(cell_name, "DATAOUT")
        .map(|s| resolve(s).to_string());

    let mut cntvaluein_nets: [Option<String>; 5] = Default::default();
    let mut cntvalueout_nets: [Option<String>; 5] = Default::default();
    for i in 0..5 {
        cntvaluein_nets[i] = pins
            .net_on(cell_name, &format!("CNTVALUEIN{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("CNTVALUEIN[{i}]")))
            .map(|s| resolve(s).to_string());
        cntvalueout_nets[i] = pins
            .net_on(cell_name, &format!("CNTVALUEOUT{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("CNTVALUEOUT[{i}]")))
            .map(|s| resolve(s).to_string());
    }

    let (bank, loc) = infer_io_affinity(netlist, attrs, data_in_net.as_deref());

    Ok(PackedIodelay {
        name: cell_name.to_string(),
        kind,
        delay_type,
        delay_value,
        delay_src,
        refclk_frequency,
        high_performance_mode,
        cinvctrl_sel,
        pipe_sel,
        iodelay_group,
        c_net: pins.net_on(cell_name, "C").map(|s| resolve(s).to_string()),
        rst_net: pins
            .net_on(cell_name, "REGRST")
            .or_else(|| pins.net_on(cell_name, "LDRST"))
            .map(|s| resolve(s).to_string()),
        ce_net: pins.net_on(cell_name, "CE").map(|s| resolve(s).to_string()),
        inc_net: pins.net_on(cell_name, "INC").map(|s| resolve(s).to_string()),
        ld_net: pins.net_on(cell_name, "LD").map(|s| resolve(s).to_string()),
        data_in_net,
        data_out_net,
        cntvaluein_nets,
        cntvalueout_nets,
        rdy_net: None,
        bank,
        loc,
    })
}

fn validate_iodelay_groups(packed: &Packed7) -> Result<(), String> {
    let mut ctrl_groups: HashSet<&str> = HashSet::new();
    for d in &packed.iodelays {
        if d.kind == IodelayKind::IdelayCtrl {
            ctrl_groups.insert(d.iodelay_group.as_str());
        }
    }
    if ctrl_groups.is_empty() {
        return Ok(());
    }
    for d in &packed.iodelays {
        if d.kind != IodelayKind::IdelayCtrl && !ctrl_groups.contains(d.iodelay_group.as_str()) {
            return Err(format!(
                "pack: {:?} {} belongs to IODELAY_GROUP `{}` which has no matching IDELAYCTRL",
                d.kind, d.name, d.iodelay_group
            ));
        }
    }
    Ok(())
}

fn pack_srl(
    cell_name: &str,
    kind: SrlKind,
    pins: &PinIndex,
    attrs: &Attrs,
) -> Result<PackedSrl, String> {
    let prim = match kind {
        SrlKind::Srl16e => "SRL16E",
        SrlKind::Srlc32e => "SRLC32E",
    };
    let init_u64 = attrs
        .get("INIT")
        .map(parse_verilog_u64)
        .transpose()?
        .unwrap_or(0);
    if kind == SrlKind::Srl16e && init_u64 > 0xFFFF {
        return Err(format!(
            "pack: SRL16E {cell_name}: INIT {init_u64:#x} exceeds 16 bits"
        ));
    }
    if init_u64 > 0xFFFF_FFFF {
        return Err(format!(
            "pack: SRLC32E {cell_name}: INIT {init_u64:#x} exceeds 32 bits"
        ));
    }
    let clk_net = pins
        .net_on(cell_name, "CLK")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no CLK net"))?;
    let d_net = pins
        .net_on(cell_name, "D")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no D net"))?;
    let q_net = pins
        .net_on(cell_name, "Q")
        .map(|s| resolve(s).to_string())
        .ok_or_else(|| format!("pack: {prim} {cell_name} has no Q net"))?;

    let mut addr_nets: [Option<String>; 5] = Default::default();
    let n_addr = match kind {
        SrlKind::Srl16e => 4,
        SrlKind::Srlc32e => 5,
    };
    for i in 0..n_addr {
        addr_nets[i] = pins
            .net_on(cell_name, &format!("A{i}"))
            .or_else(|| pins.net_on(cell_name, &format!("A[{i}]")))
            .map(|s| resolve(s).to_string());
    }
    let is_clk_inverted = parse_bool_attr(attrs, "IS_CLK_INVERTED", false)?;

    Ok(PackedSrl {
        name: cell_name.to_string(),
        kind,
        init: init_u64 as u32,
        clk_net,
        ce_net: pins.net_on(cell_name, "CE").map(|s| resolve(s).to_string()),
        d_net,
        addr_nets,
        q_net,
        q31_net: pins.net_on(cell_name, "Q31").map(|s| resolve(s).to_string()),
        is_clk_inverted,
    })
}

fn pack_muxf(cell_name: &str, kind: MuxfKind, pins: &PinIndex) -> Result<PackedMuxf, String> {
    let prim = match kind {
        MuxfKind::Muxf7 => "MUXF7",
        MuxfKind::Muxf8 => "MUXF8",
    };
    let need = |pin: &str| {
        pins.net_on(cell_name, pin)
            .map(|s| resolve(s).to_string())
            .ok_or_else(|| format!("pack: {prim} {cell_name} has no {pin} net"))
    };
    Ok(PackedMuxf {
        name: cell_name.to_string(),
        kind,
        i0_net: need("I0")?,
        i1_net: need("I1")?,
        s_net: need("S")?,
        o_net: need("O")?,
    })
}

/// Insert IBUF + BUFGCTRL for clock inputs.
///
/// For every net that drives at least one FF `CLK` pin and is not already
/// buffered, find the matching top-level input port and synthesize a
/// [`PackedIob`] (IBUF) plus a [`PackedBufg`] (BUFGCTRL). FF clock pins are
/// rewired onto the BUFG output net (`<port>_bufg`).
fn insert_clock_spine(netlist: &Design, packed: &mut Packed7) -> Result<(), String> {
    let mut buffered: HashSet<String> = packed
        .bufgs
        .iter()
        .flat_map(|b| [b.in_net.clone(), b.out_net.clone()])
        .collect();
    for cmt in &packed.cmts {
        for out in cmt.clkout_nets.iter().flatten() {
            buffered.insert(out.clone());
        }
        if let Some(fb) = &cmt.clkfbout_net {
            buffered.insert(fb.clone());
        }
    }

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
            .find(|p| p.dir == PortDir::In && p.name == intern(&clk_net))
            .ok_or_else(|| {
                format!("pack: clock net `{clk_net}` has no matching top-level input port")
            })?;
        let bufg_out = format!("{}_bufg", resolve(port.name));
        for ff in &mut packed.ffs {
            if ff.clk_net == clk_net {
                ff.clk_net = bufg_out.clone();
            }
        }
        for srl in &mut packed.srls {
            if srl.clk_net == clk_net {
                srl.clk_net = bufg_out.clone();
            }
        }
        packed.iobs.push(PackedIob {
            name: format!("ibuf_{}", resolve(port.name)),
            loc: port.attrs.get("LOC").map(|s| s.to_string()),
            iostandard: port
                .attrs
                .get("IOSTANDARD")
                .map(|s| s.to_string())
                .unwrap_or_else(|| "LVCMOS33".to_string()),
            drive: None,
            slew: None,
            port: resolve(port.name).to_string(),
            dir: IobDir::In,
            net: format!("{}_ibuf", resolve(port.name)),
        });
        packed.bufgs.push(PackedBufg {
            name: format!("bufg_{}", resolve(port.name)),
            in_net: format!("{}_ibuf", resolve(port.name)),
            out_net: bufg_out,
            ..Default::default()
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
        d.add_cell(
            "u_inv",
            CellKind::Lut6 {
                init: 0x5555_5555_5555_5555,
            },
        );
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
        assert_eq!(
            packed.iobs.iter().filter(|i| i.dir == IobDir::In).count(),
            1
        );
        assert_eq!(packed.bufgs.len(), 1);
        assert_eq!(packed.ffs[0].clk_net, "clk_bufg");
        assert_eq!(packed.bufgs[0].in_net, "clk_ibuf");
        assert_eq!(packed.bufgs[0].out_net, "clk_bufg");
    }

    #[test]
    fn pack_rejects_unsupported_loudly() {
        let mut d = Design::new("bad");
        d.add_cell("u_ila", CellKind::Ila { net: intern("x") });
        let err = pack(&d).unwrap_err();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    #[test]
    fn pack_mmcm_and_pll_validates_vco_range() {
        let mut d = Design::new("cmt_test");
        d.add_cell(
            "u_mmcm",
            CellKind::BlackBox {
                module: intern("MMCME2_ADV"),
            },
        );
        d.set_cell_attr("u_mmcm", "CLKIN1_PERIOD", "10.0").unwrap(); // 100 MHz
        d.set_cell_attr("u_mmcm", "CLKFBOUT_MULT_F", "10.0").unwrap(); // VCO = 1000 MHz
        d.set_cell_attr("u_mmcm", "DIVCLK_DIVIDE", "1").unwrap();
        d.set_cell_attr("u_mmcm", "CLKOUT0_DIVIDE_F", "5.0").unwrap();
        d.connect("clk_in", "u_mmcm", "CLKIN1");
        d.connect("clk_out0", "u_mmcm", "CLKOUT0");
        let packed = pack(&d).unwrap();
        assert_eq!(packed.cmts.len(), 1);
        assert!((packed.cmts[0].vco_freq_mhz - 1000.0).abs() < 1e-6);

        // Illegal VCO (100 MHz * 2 / 1 = 200 MHz < 600 MHz min)
        d.set_cell_attr("u_mmcm", "CLKFBOUT_MULT_F", "2.0").unwrap();
        let err = pack(&d).unwrap_err();
        assert!(err.contains("VCO frequency"), "got: {err}");
    }
}
