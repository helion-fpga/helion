//! Xilinx UltraScale+ (`xcku5p` / `xczu9eg` / `xcvu9p` / `xcvu13p`) and Multi-SLR SSI Architecture Model.
//!
//! Provides:
//! 1. **UltraScale+ Part Geometry & Capacities** (`UltraScalePart`, `UltraScaleDevice`).
//! 2. **UltraScale+ CLB (`SLICEL` / `SLICEM`) & `CARRY8`** (`UltraScaleClbSlice`, `UltraScaleSliceKind`,
//!    `Carry8Mode`, `Carry8Chain`, `carry8_delay_ps`) with 8 LUT6, 16 flip-flops (`AFF`/`AFF2`..`HFF`/`HFF2`),
//!    `F7MUX`/`F8MUX`/`F9MUX` wide multiplexers, and 8-stage lookahead carry evaluation.
//! 3. **`DSP48E2` Slice** (`Dsp48e2Config`, `Dsp48e2WideXorMode`, `dsp48e2_evaluate`, `dsp48e2_delay_ps`)
//!    with 27-bit pre-adder, 27×18 multiplier, 48-bit ALU/accumulator, pattern detector, and 96-bit wide XOR.
//! 4. **`RAMB36E2` & `URAM288` UltraRAM Column Cascade** (`Ramb36e2Config`, `Uram288Config`,
//!    `Uram288CascadePlan`, `plan_uram288_cascade`, `uram288_ecc_encode_64`, `uram288_ecc_decode_72`).
//! 5. **Multi-SLR SSI & Laguna Super Long Line (SLL) Interposer Crossings** (`SlrRegion`, `SlrTopology`,
//!    `LagunaCrossingMode`, `sll_crossing_delay_ps`, `PblockSlrSpec`, `SlrValidationReport`,
//!    `validate_pblock_slr_assignment`).

// ============================================================================
// 1. UltraScale+ Part Catalog & Geometry
// ============================================================================

/// Supported UltraScale+ monolithic and multi-SLR Stacked Silicon Interconnect (SSI) parts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum UltraScalePart {
    /// Kintex UltraScale+ `xcku5p` (monolithic 1-SLR, 27,120 CLB slices, 1,824 DSP48E2, 480 RAMB36E2, 64 URAM288).
    Xcku5p,
    /// Zynq UltraScale+ MPSoC `xczu9eg` (monolithic 1-SLR, 34,260 CLB slices, 2,520 DSP48E2, 912 RAMB36E2).
    Xczu9eg,
    /// Virtex UltraScale+ `xcvu9p` (3-SLR SSI device, 147,780 CLB slices, 6,840 DSP48E2, 2,160 RAMB36E2, 960 URAM288).
    Xcvu9p,
    /// Virtex UltraScale+ `xcvu13p` (4-SLR SSI device, 216,000 CLB slices, 12,288 DSP48E2, 2,688 RAMB36E2, 1,280 URAM288).
    Xcvu13p,
}

impl UltraScalePart {
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        if lower.starts_with("xcku5p") {
            Some(Self::Xcku5p)
        } else if lower.starts_with("xczu9eg") {
            Some(Self::Xczu9eg)
        } else if lower.starts_with("xcvu9p") {
            Some(Self::Xcvu9p)
        } else if lower.starts_with("xcvu13p") {
            Some(Self::Xcvu13p)
        } else {
            None
        }
    }

    pub fn part_name(self) -> &'static str {
        match self {
            Self::Xcku5p => "xcku5p-ffvb676-2-e",
            Self::Xczu9eg => "xczu9eg-ffvb1156-2-e",
            Self::Xcvu9p => "xcvu9p-flga2104-2L-e",
            Self::Xcvu13p => "xcvu13p-fhga2104-2L-e",
        }
    }
}

/// Complete UltraScale+ device architecture descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UltraScaleDevice {
    pub part: UltraScalePart,
    pub slr_count: u32,
    pub rows_per_slr: u32,
    pub cols: u32,
    pub clb_slices: u32,
    pub slicem_count: u32,
    pub lut6_count: u32,
    pub ff_count: u32,
    pub carry8_count: u32,
    pub dsp48e2_count: u32,
    pub ramb36e2_count: u32,
    pub uram288_count: u32,
    pub uram_per_column_per_slr: u32,
    pub sll_wires_per_slr_cut: u32,
}

impl UltraScaleDevice {
    pub fn for_part(part: UltraScalePart) -> Self {
        match part {
            UltraScalePart::Xcku5p => {
                let slices = 27_120;
                Self {
                    part,
                    slr_count: 1,
                    rows_per_slr: 240,
                    cols: 120,
                    clb_slices: slices,
                    slicem_count: 11_760,
                    lut6_count: slices * 8,
                    ff_count: slices * 16,
                    carry8_count: slices,
                    dsp48e2_count: 1_824,
                    ramb36e2_count: 480,
                    uram288_count: 64,
                    uram_per_column_per_slr: 64,
                    sll_wires_per_slr_cut: 0,
                }
            }
            UltraScalePart::Xczu9eg => {
                let slices = 34_260;
                Self {
                    part,
                    slr_count: 1,
                    rows_per_slr: 240,
                    cols: 150,
                    clb_slices: slices,
                    slicem_count: 14_400,
                    lut6_count: slices * 8,
                    ff_count: slices * 16,
                    carry8_count: slices,
                    dsp48e2_count: 2_520,
                    ramb36e2_count: 912,
                    uram288_count: 0,
                    uram_per_column_per_slr: 0,
                    sll_wires_per_slr_cut: 0,
                }
            }
            UltraScalePart::Xcvu9p => {
                let slices = 147_780;
                Self {
                    part,
                    slr_count: 3,
                    rows_per_slr: 240,
                    cols: 210,
                    clb_slices: slices,
                    slicem_count: 59_880,
                    lut6_count: slices * 8,
                    ff_count: slices * 16,
                    carry8_count: slices,
                    dsp48e2_count: 6_840,
                    ramb36e2_count: 2_160,
                    uram288_count: 960,
                    uram_per_column_per_slr: 64,
                    sll_wires_per_slr_cut: 23_040,
                }
            }
            UltraScalePart::Xcvu13p => {
                let slices = 216_000;
                Self {
                    part,
                    slr_count: 4,
                    rows_per_slr: 240,
                    cols: 240,
                    clb_slices: slices,
                    slicem_count: 86_400,
                    lut6_count: slices * 8,
                    ff_count: slices * 16,
                    carry8_count: slices,
                    dsp48e2_count: 12_288,
                    ramb36e2_count: 2_688,
                    uram288_count: 1_280,
                    uram_per_column_per_slr: 64,
                    sll_wires_per_slr_cut: 23_040,
                }
            }
        }
    }

    pub fn total_rows(&self) -> u32 {
        self.slr_count * self.rows_per_slr
    }

    /// Build the [`SlrTopology`] for this UltraScale+ device.
    pub fn slr_topology(&self) -> SlrTopology {
        SlrTopology::from_device(self)
    }
}

// ============================================================================
// 2. UltraScale+ CLB (SLICEL / SLICEM, 8 LUT6, 16 FF, F7/F8/F9MUX, CARRY8)
// ============================================================================

/// UltraScale+ CLB slice flavor (UG574).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum UltraScaleSliceKind {
    /// Logic + Carry + Wide Multiplexer slice (8 LUT6, 16 FF, 1 CARRY8).
    SliceL,
    /// Distributed RAM (`RAM64M8` / `RAM512X1`) + SRL32 + Logic + Carry slice.
    SliceM,
}

/// BEL letters `A`..`H` inside an UltraScale+ CLB slice (8 BLE slots per slice).
pub const ULTRASCALE_BLE_LETTERS: [char; 8] = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'];

/// Complete list of BEL names inside an UltraScale+ CLB slice (`A6LUT`..`H6LUT`, `A5LUT`..`H5LUT`,
/// `AFF`/`AFF2`..`HFF`/`HFF2`, `F7MUX_AB`/`CD`/`EF`/`GH`, `F8MUX_BOT`/`TOP`, `F9MUX`, `CARRY8`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UltraScaleClbSlice {
    pub x: u32,
    pub y: u32,
    pub kind: UltraScaleSliceKind,
}

impl UltraScaleClbSlice {
    pub const LUT6_PER_SLICE: usize = 8;
    pub const FF_PER_SLICE: usize = 16;

    pub fn new(x: u32, y: u32, kind: UltraScaleSliceKind) -> Self {
        Self { x, y, kind }
    }

    pub fn site_name(&self) -> String {
        format!("SLICE_X{}Y{}", self.x, self.y)
    }

    /// Return all 32 logic/storage/mux/carry BEL names in this UltraScale+ CLB slice (UG574).
    pub fn bels(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(40);
        for ch in ULTRASCALE_BLE_LETTERS {
            out.push(format!("{ch}6LUT"));
            out.push(format!("{ch}5LUT"));
            out.push(format!("{ch}FF"));
            out.push(format!("{ch}FF2"));
        }
        out.push("F7MUX_AB".into());
        out.push("F7MUX_CD".into());
        out.push("F7MUX_EF".into());
        out.push("F7MUX_GH".into());
        out.push("F8MUX_BOT".into());
        out.push("F8MUX_TOP".into());
        out.push("F9MUX".into());
        out.push("CARRY8".into());
        out
    }

    /// Check whether two flip-flop BELs (`AFF`..`HFF2`) share the lower (`A..D`) or upper (`E..H`)
    /// clock / clock-enable / set-reset control group.
    pub fn same_half_slice(bel_a: &str, bel_b: &str) -> bool {
        let half_of = |b: &str| -> Option<u8> {
            let first = b.chars().next()?.to_ascii_uppercase();
            match first {
                'A'..='D' => Some(0),
                'E'..='H' => Some(1),
                _ => None,
            }
        };
        match (half_of(bel_a), half_of(bel_b)) {
            (Some(h0), Some(h1)) => h0 == h1,
            _ => false,
        }
    }
}

/// Operating mode of an UltraScale+ `CARRY8` primitive (`CARRY_TYPE` attribute, UG574).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Carry8Mode {
    /// Full 8-bit carry chain (`SINGLE_CY8`): `CI` ripples through stages `0..=7`.
    SingleCy8,
    /// Split dual 4-bit carry chains (`DUAL_CY4`): stages `0..=3` use `CI`, stages `4..=7` use `CI_TOP`.
    DualCy4,
}

impl Default for Carry8Mode {
    fn default() -> Self {
        Self::SingleCy8
    }
}

/// UltraScale+ 8-stage fast lookahead carry chain (`CARRY8`, UG574).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Carry8Chain {
    pub mode: Carry8Mode,
}

impl Carry8Chain {
    pub fn new(mode: Carry8Mode) -> Self {
        Self { mode }
    }

    /// Evaluate the 8-bit `CARRY8` primitive given carry inputs `ci` (bit 0) and `ci_top` (bit 4 in `DualCy4`),
    /// 8-bit data/generate input `di`, and 8-bit propagate select input `s`.
    ///
    /// Returns `(o, co)` where:
    /// - $O_i = S_i \oplus C_{\text{in},i}$
    /// - $CO_i = S_i ? C_{\text{in},i} : DI_i$
    pub fn evaluate(&self, ci: bool, ci_top: bool, di: u8, s: u8) -> (u8, u8) {
        let mut o: u8 = 0;
        let mut co: u8 = 0;
        let mut c_in = ci as u8;
        for i in 0..8u8 {
            if i == 4 && self.mode == Carry8Mode::DualCy4 {
                c_in = ci_top as u8;
            }
            let s_bit = (s >> i) & 1;
            let di_bit = (di >> i) & 1;
            let o_bit = s_bit ^ c_in;
            let co_bit = if s_bit == 1 { c_in } else { di_bit };
            o |= o_bit << i;
            co |= co_bit << i;
            c_in = co_bit;
        }
        (o, co)
    }
}

/// Compute UltraScale+ `CARRY8` propagation delay in picoseconds for `bits` cascaded stages.
///
/// Each `CARRY8` spans 8 bits inside a single CLB slice with ~48 ps entry + ~11 ps/stage lookahead
/// + ~28 ps vertical `CO[7] -> CI` slice-to-slice hop.
pub fn carry8_delay_ps(bits: u32) -> u32 {
    if bits == 0 {
        return 0;
    }
    let num_chains = (bits + 7) / 8;
    let entry_exit_ps = 48;
    let per_bit_ps = 11 * bits;
    let cascade_hops_ps = 28 * num_chains.saturating_sub(1);
    entry_exit_ps + per_bit_ps + cascade_hops_ps
}

// ============================================================================
// 3. UltraScale+ DSP48E2 (27x18 Multiplier + Pre-Adder + 48b ALU + Wide XOR)
// ============================================================================

/// Wide XOR mode of the UltraScale+ `DSP48E2` slice (`USE_WIDEXOR`, UG579).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Dsp48e2WideXorMode {
    Disabled,
    ///Eight 6-bit XOR outputs (`XOROUT[7:0]`).
    Xor12,
    /// Four 12-bit XOR outputs (`XOROUT[3:0]`).
    Xor24,
    /// Two 24-bit XOR outputs (`XOROUT[1:0]`).
    Xor48,
    /// Single 48/96-bit wide XOR reduction (`XOROUT[0]`).
    Xor96,
}

/// Configuration of an UltraScale+ `DSP48E2` primitive (UG579).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dsp48e2Config {
    /// Enable 27-bit pre-adder (`AD = D +/- A`).
    pub use_preadder: bool,
    /// When true, pre-adder computes `D - A`; otherwise `D + A`.
    pub preadder_subtract: bool,
    /// Enable 27×18 multiplier (`M = AD * B`).
    pub use_mult: bool,
    /// Number of pipeline register stages (`0..=4`: `AREG`/`DREG`, `ADREG`, `MREG`, `PREG`).
    pub pipeline_stages: u8,
    /// Wide XOR reduction mode.
    pub wide_xor: Dsp48e2WideXorMode,
    /// 48-bit pattern for `PATDET` comparison.
    pub pattern: u64,
    /// 48-bit mask for `PATDET` comparison (`1` bit = ignore bit position).
    pub mask: u64,
}

impl Default for Dsp48e2Config {
    fn default() -> Self {
        Self {
            use_preadder: true,
            preadder_subtract: false,
            use_mult: true,
            pipeline_stages: 3,
            wide_xor: Dsp48e2WideXorMode::Disabled,
            pattern: 0,
            mask: 0,
        }
    }
}

/// Result of evaluating an UltraScale+ `DSP48E2` operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dsp48e2Output {
    /// 48-bit signed/unsigned primary output `P[47:0]` (masked to lower 48 bits).
    pub p: i64,
    /// True if `(P ^ pattern) & !mask == 0` (48-bit pattern detector match).
    pub patdet: bool,
    /// 8-bit wide XOR output `XOROUT[7:0]`.
    pub xorout: u8,
}

/// Evaluate the UltraScale+ `DSP48E2` datapath:
/// 1. 27-bit pre-adder: $AD = \text{use\_preadder} ? (D \pm A) : A$ (clamped/masked to 27-bit signed).
/// 2. 27×18 multiplier: $M = AD_{27} \times B_{18}$ (45-bit product).
/// 3. 48-bit ALU: $P = C_{48} + M_{45} + \text{cin}$.
/// 4. 48-bit pattern detector (`PATDET`) and wide XOR (`XOROUT[7:0]`).
pub fn dsp48e2_evaluate(
    cfg: &Dsp48e2Config,
    a_27b: i64,
    b_18b: i64,
    c_48b: i64,
    d_27b: i64,
    cin: bool,
) -> Dsp48e2Output {
    // Sign-extend 27-bit A, D and 18-bit B
    let sign_ext = |v: i64, bits: u32| -> i64 {
        let shift = 64 - bits;
        (v << shift) >> shift
    };
    let a = sign_ext(a_27b, 27);
    let d = sign_ext(d_27b, 27);
    let b = sign_ext(b_18b, 18);
    let c = sign_ext(c_48b, 48);

    let ad = if cfg.use_preadder {
        let raw = if cfg.preadder_subtract { d - a } else { d + a };
        sign_ext(raw, 27)
    } else {
        a
    };

    let m = if cfg.use_mult { ad * b } else { ad };
    let p_full = c.wrapping_add(m).wrapping_add(cin as i64);
    let p_48 = sign_ext(p_full, 48);
    let p_u48 = (p_48 as u64) & 0xFFFF_FFFF_FFFF;

    let pat_u48 = cfg.pattern & 0xFFFF_FFFF_FFFF;
    let mask_u48 = cfg.mask & 0xFFFF_FFFF_FFFF;
    let patdet = ((p_u48 ^ pat_u48) & !mask_u48) == 0;

    // Compute 8x 6-bit slice XOR reductions first
    let mut x6 = [0u8; 8];
    for i in 0..8 {
        let slice6 = (p_u48 >> (i * 6)) & 0x3F;
        x6[i] = (slice6.count_ones() & 1) as u8;
    }
    let xorout = match cfg.wide_xor {
        Dsp48e2WideXorMode::Disabled => 0,
        Dsp48e2WideXorMode::Xor12 => {
            let mut out = 0u8;
            for i in 0..8 {
                out |= x6[i] << i;
            }
            out
        }
        Dsp48e2WideXorMode::Xor24 => {
            ((x6[0] ^ x6[1]))
                | ((x6[2] ^ x6[3]) << 1)
                | ((x6[4] ^ x6[5]) << 2)
                | ((x6[6] ^ x6[7]) << 3)
        }
        Dsp48e2WideXorMode::Xor48 => (x6[0] ^ x6[1] ^ x6[2] ^ x6[3])
            | ((x6[4] ^ x6[5] ^ x6[6] ^ x6[7]) << 1),
        Dsp48e2WideXorMode::Xor96 => (p_u48.count_ones() & 1) as u8,
    };

    Dsp48e2Output {
        p: p_48,
        patdet,
        xorout,
    }
}

/// Combinational / clock-to-out critical path delay in picoseconds for a `DSP48E2` configuration.
pub fn dsp48e2_delay_ps(cfg: &Dsp48e2Config) -> u32 {
    match cfg.pipeline_stages {
        0 => 1_850, // Fully unpipelined D->AD->M->P
        1 => 1_120,
        2 => 720,
        3 => 480,
        _ => 380, // Fully pipelined (DREG+ADREG+MREG+PREG) -> >750 MHz Fmax
    }
}

// ============================================================================
// 4. RAMB36E2 & URAM288 (288Kb UltraRAM + Dedicated Column Cascade + ECC)
// ============================================================================

/// Configuration of an UltraScale+ `RAMB36E2` 36Kb Block RAM primitive (UG573).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ramb36e2Config {
    pub read_width_a: u32,
    pub write_width_a: u32,
    pub read_width_b: u32,
    pub write_width_b: u32,
    pub enable_ecc: bool,
    pub cascade_order: bool,
}

impl Default for Ramb36e2Config {
    fn default() -> Self {
        Self {
            read_width_a: 36,
            write_width_a: 36,
            read_width_b: 36,
            write_width_b: 36,
            enable_ecc: false,
            cascade_order: false,
        }
    }
}

/// Configuration of an UltraScale+ `URAM288` (288Kb = $4096 \times 72$-bit UltraRAM) block (UG573).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uram288Config {
    /// Enable built-in 64-bit data + 8-bit SEC/DED Hamming ECC (`EN_ECC_RD` / `EN_ECC_WR`).
    pub enable_ecc: bool,
    /// Enable internal output pipeline register (`OREG_A` / `OREG_B`).
    pub output_reg: bool,
    /// Enable cascade output pipeline register (`OREG_ECC_CAS_A` / `B`).
    pub cascade_reg: bool,
    /// Auto-sleep power gating mode (`AUTO_SLEEP_LATENCY`).
    pub auto_sleep: bool,
}

impl Default for Uram288Config {
    fn default() -> Self {
        Self {
            enable_ecc: true,
            output_reg: true,
            cascade_reg: true,
            auto_sleep: false,
        }
    }
}

impl Uram288Config {
    pub const CAPACITY_BITS: u64 = 294_912; // 4096 * 72
    pub const DEPTH_WORDS: u64 = 4_096;
    pub const WIDTH_BITS: u32 = 72;
}

/// Encode a 64-bit word into a 72-bit `URAM288` word with 8-bit SEC/DED extended Hamming ECC (`[71:64]`).
pub fn uram288_ecc_encode_64(data: u64) -> u128 {
    let mut ecc: u8 = 0;
    // Standard 7 Hamming parity bits p[0..6] covering 1-based codeword positions 1..=71
    // Map 64 data bits to non-power-of-two codeword positions 3..=71
    let mut cw_bits = [0u8; 72]; // 1-indexed 1..=71
    let mut d_idx = 0usize;
    for pos in 1..=71usize {
        if !pos.is_power_of_two() && d_idx < 64 {
            cw_bits[pos] = ((data >> d_idx) & 1) as u8;
            d_idx += 1;
        }
    }
    for p in 0..7usize {
        let p_pos = 1usize << p;
        let mut parity = 0u8;
        for pos in 1..=71usize {
            if (pos & p_pos) != 0 {
                parity ^= cw_bits[pos];
            }
        }
        cw_bits[p_pos] = parity;
        ecc |= parity << p;
    }
    // Overall parity bit p[7] across all 71 bits for double-error detection (DED)
    let mut total_par = 0u8;
    for pos in 1..=71usize {
        total_par ^= cw_bits[pos];
    }
    ecc |= total_par << 7;
    (data as u128) | ((ecc as u128) << 64)
}

/// Decode and SEC/DED check a 72-bit `URAM288` word (`[63:0]` data, `[71:64]` ECC).
///
/// Returns `(corrected_data, single_bit_corrected, double_bit_error)`.
pub fn uram288_ecc_decode_72(word72: u128) -> (u64, bool, bool) {
    let data = (word72 & 0xFFFF_FFFF_FFFF_FFFF) as u64;
    let rx_ecc = ((word72 >> 64) & 0xFF) as u8;

    let mut cw_bits = [0u8; 72];
    let mut d_idx = 0usize;
    for pos in 1..=71usize {
        if pos.is_power_of_two() {
            let p = pos.trailing_zeros() as usize;
            cw_bits[pos] = (rx_ecc >> p) & 1;
        } else if d_idx < 64 {
            cw_bits[pos] = ((data >> d_idx) & 1) as u8;
            d_idx += 1;
        }
    }

    let mut syndrome = 0usize;
    for p in 0..7usize {
        let p_pos = 1usize << p;
        let mut parity = 0u8;
        for pos in 1..=71usize {
            if (pos & p_pos) != 0 {
                parity ^= cw_bits[pos];
            }
        }
        if parity != 0 {
            syndrome |= p_pos;
        }
    }

    let mut overall_par = (rx_ecc >> 7) & 1;
    for pos in 1..=71usize {
        overall_par ^= cw_bits[pos];
    }

    if syndrome == 0 && overall_par == 0 {
        return (data, false, false);
    }
    if syndrome == 0 && overall_par == 1 {
        // Single-bit flip in the overall parity bit p[7] itself
        return (data, true, false);
    }
    if syndrome != 0 && overall_par == 0 {
        // Double-bit error detected!
        return (data, false, true);
    }
    // Single-bit error at codeword position `syndrome`
    if syndrome <= 71 {
        cw_bits[syndrome] ^= 1;
    }
    let mut fixed_data = 0u64;
    let mut idx = 0usize;
    for pos in 1..=71usize {
        if !pos.is_power_of_two() && idx < 64 {
            fixed_data |= (cw_bits[pos] as u64) << idx;
            idx += 1;
        }
    }
    (fixed_data, true, false)
}

/// Planned `URAM288` column cascade layout for a deep memory array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uram288CascadePlan {
    pub requested_depth_words: u64,
    pub requested_width_bits: u32,
    pub depth_cascade_len: u32,
    pub width_stripes: u32,
    pub total_uram288_blocks: u32,
    /// Number of vertical column segments required (since a single `URAM288` cascade cannot cross an SLR cut).
    pub slr_columns_needed: u32,
    /// Estimated read latency in clock cycles (`1` base + `1` OREG + pipeline stages along cascade).
    pub read_latency_cycles: u32,
    /// Maximum achievable clock frequency in MHz along the dedicated `CAS_OUT -> CAS_IN` column path.
    pub estimated_fmax_mhz: u32,
}

/// Plan a dedicated `URAM288` column cascade for a memory of `depth_words` × `width_bits` on `dev`.
///
/// Respects the UltraScale+ architectural rule that dedicated `URAM288` column cascades cannot cross
/// an SLR interposer boundary (`uram_per_column_per_slr`, typically 64 `URAM288` blocks per SLR column).
pub fn plan_uram288_cascade(
    dev: &UltraScaleDevice,
    depth_words: u64,
    width_bits: u32,
    pipeline_every_n: u32,
) -> Result<Uram288CascadePlan, String> {
    if dev.uram288_count == 0 {
        return Err(format!(
            "Part {} has 0 URAM288 blocks",
            dev.part.part_name()
        ));
    }
    let depth_words = depth_words.max(1);
    let width_bits = width_bits.max(1);

    let depth_cascade_len = ((depth_words + Uram288Config::DEPTH_WORDS - 1)
        / Uram288Config::DEPTH_WORDS) as u32;
    let width_stripes = (width_bits + Uram288Config::WIDTH_BITS - 1) / Uram288Config::WIDTH_BITS;
    let total_uram288_blocks = depth_cascade_len * width_stripes;

    if total_uram288_blocks > dev.uram288_count {
        return Err(format!(
            "Requested {} URAM288 blocks ({}x{}) exceeds device capacity {} on {}",
            total_uram288_blocks,
            depth_cascade_len,
            width_stripes,
            dev.uram288_count,
            dev.part.part_name()
        ));
    }

    let max_per_slr_col = dev.uram_per_column_per_slr.max(1);
    let slr_columns_needed =
        ((depth_cascade_len + max_per_slr_col - 1) / max_per_slr_col) * width_stripes;
    let pipe_step = pipeline_every_n.max(1);
    let cascade_regs = (depth_cascade_len.saturating_sub(1)) / pipe_step;
    let read_latency_cycles = 2 + cascade_regs;
    // Unregistered hop adds ~115 ps per cascaded URAM288; registered every N keeps Fmax high
    let combinational_span = depth_cascade_len.min(pipe_step);
    let period_ps = 1_250 + 115 * combinational_span.saturating_sub(1);
    let estimated_fmax_mhz = 1_000_000 / period_ps;

    Ok(Uram288CascadePlan {
        requested_depth_words: depth_words,
        requested_width_bits: width_bits,
        depth_cascade_len,
        width_stripes,
        total_uram288_blocks,
        slr_columns_needed,
        read_latency_cycles,
        estimated_fmax_mhz,
    })
}

// ============================================================================
// 5. Multi-SLR SSI & Laguna Super Long Line (SLL) Interposer Crossings
// ============================================================================

/// Single Super Logic Region (SLR) die slice in an UltraScale+ device (UG574 / UG949).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlrRegion {
    pub slr_id: u32,
    pub y_min: u32,
    pub y_max: u32,
    pub clb_slices: u32,
    pub dsp48e2: u32,
    pub ramb36e2: u32,
    pub uram288: u32,
    /// True if this SLR has a lower interposer boundary (`slr_id > 0`) with Laguna TX/RX tiles.
    pub has_lower_laguna: bool,
    /// True if this SLR has an upper interposer boundary (`slr_id + 1 < slr_count`) with Laguna TX/RX tiles.
    pub has_upper_laguna: bool,
}

/// Pipeline register mode for a Laguna Super Long Line (SLL) interposer crossing (UG949).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LagunaCrossingMode {
    /// Combinational SLL hop straight from fabric interconnect in `SLR_a` to fabric in `SLR_b` (~1,180 ps/hop).
    Unregistered,
    /// Registered at the launching Laguna `TX_REG` or receiving `RX_REG` (~790 ps combinational exposure).
    SingleRegistered,
    /// Registered at both `TX_REG` (source SLR Laguna tile) and `RX_REG` (destination SLR Laguna tile) (~490 ps SLL wire-only hop).
    TxRxRegistered,
}

impl Default for LagunaCrossingMode {
    fn default() -> Self {
        Self::TxRxRegistered
    }
}

/// Multi-SLR Stacked Silicon Interconnect (SSI) topology for an UltraScale+ device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlrTopology {
    pub part: UltraScalePart,
    pub slrs: Vec<SlrRegion>,
    /// Total SLL interposer wires across each adjacent SLR boundary cut (`SLR_i <-> SLR_{i+1}`).
    pub sll_wires_per_cut: u32,
    /// Number of Laguna columns across the die width.
    pub laguna_columns: u32,
}

impl SlrTopology {
    pub fn from_device(dev: &UltraScaleDevice) -> Self {
        let n = dev.slr_count.max(1);
        let mut slrs = Vec::with_capacity(n as usize);
        for i in 0..n {
            let y_min = i * dev.rows_per_slr;
            let y_max = (i + 1) * dev.rows_per_slr - 1;
            slrs.push(SlrRegion {
                slr_id: i,
                y_min,
                y_max,
                clb_slices: dev.clb_slices / n,
                dsp48e2: dev.dsp48e2_count / n,
                ramb36e2: dev.ramb36e2_count / n,
                uram288: dev.uram288_count / n,
                has_lower_laguna: i > 0,
                has_upper_laguna: i + 1 < n,
            });
        }
        let laguna_columns = if n > 1 {
            dev.sll_wires_per_slr_cut / 1_440
        } else {
            0
        };
        Self {
            part: dev.part,
            slrs,
            sll_wires_per_cut: dev.sll_wires_per_slr_cut,
            laguna_columns,
        }
    }

    pub fn num_slrs(&self) -> u32 {
        self.slrs.len() as u32
    }

    pub fn num_interposer_cuts(&self) -> usize {
        self.slrs.len().saturating_sub(1)
    }

    /// Map a global tile row coordinate `y` to its containing `SLR` index.
    pub fn slr_of_y(&self, y: u32) -> Option<u32> {
        self.slrs
            .iter()
            .find(|s| y >= s.y_min && y <= s.y_max)
            .map(|s| s.slr_id)
    }

    /// Number of interposer die-crossing hops between row `src_y` and row `dst_y`.
    pub fn slr_hops_between_y(&self, src_y: u32, dst_y: u32) -> Option<u32> {
        let s0 = self.slr_of_y(src_y)?;
        let s1 = self.slr_of_y(dst_y)?;
        Some(s0.abs_diff(s1))
    }
}

/// Compute the Laguna Super Long Line (SLL) interposer crossing delay in picoseconds
/// between `src_slr` and `dst_slr` under `mode` and interposer cut utilization `cut_utilization_pct` ($0..=100\%$).
///
/// - Same-SLR (`src_slr == dst_slr`) returns `0` ps.
/// - Each interposer boundary hop adds the base SLL micro-bump + silicon interposer trace delay:
///   - `TxRxRegistered`: `490` ps per hop (pure Laguna-TX FF $\to$ SLL $\to$ Laguna-RX FF wire + clock uncertainty).
///   - `SingleRegistered`: `790` ps per hop.
///   - `Unregistered`: `1_180` ps per hop (includes CLB switchbox $\to$ Laguna mux entry/exit).
/// - When SLL cut utilization exceeds $80\%$, routing detour adds a congestion penalty up to $+35\%$.
pub fn sll_crossing_delay_ps(
    src_slr: u32,
    dst_slr: u32,
    mode: LagunaCrossingMode,
    cut_utilization_pct: f64,
) -> u32 {
    let hops = src_slr.abs_diff(dst_slr);
    if hops == 0 {
        return 0;
    }
    let per_hop_base_ps: f64 = match mode {
        LagunaCrossingMode::TxRxRegistered => 490.0,
        LagunaCrossingMode::SingleRegistered => 790.0,
        LagunaCrossingMode::Unregistered => 1_180.0,
    };
    let util = cut_utilization_pct.clamp(0.0, 150.0);
    let congestion_mult = if util <= 80.0 {
        1.0
    } else {
        1.0 + 0.012 * (util - 80.0)
    };
    ((hops as f64) * per_hop_base_ps * congestion_mult).round() as u32
}

/// Kind of dedicated column cascade chain subject to SLR boundary rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SlrCascadeKind {
    Carry8,
    Dsp48e2,
    Uram288,
    Ramb36e2,
}

impl SlrCascadeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Carry8 => "CARRY8",
            Self::Dsp48e2 => "DSP48E2",
            Self::Uram288 => "URAM288",
            Self::Ramb36e2 => "RAMB36E2",
        }
    }
}

/// Dedicated hardware cascade chain spanning `[y_start, y_end]`.
/// Dedicated column cascades (`CARRY8`, `DSP48E2`, `URAM288`, `RAMB36E2`) do NOT have dedicated
/// wires across the silicon interposer cut between SLRs!
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlrCascadeSpan {
    pub name: String,
    pub kind: SlrCascadeKind,
    pub y_start: u32,
    pub y_end: u32,
}

/// Pblock floorplanning constraint assigning logic/memory/DSP resources to one or more SLRs
/// or a `[y_min, y_max]` coordinate rectangle (UG906 / UG949).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PblockSlrSpec {
    pub name: String,
    /// Explicit target SLR ID (`Some(slr_id)` if pinned to a single SLR).
    pub target_slr: Option<u32>,
    pub y_min: u32,
    pub y_max: u32,
    pub req_clb_slices: u32,
    pub req_dsp48e2: u32,
    pub req_ramb36e2: u32,
    pub req_uram288: u32,
}

/// Single DRC/methodology violation found during [`validate_pblock_slr_assignment`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlrViolation {
    pub code: String,
    pub object: String,
    pub message: String,
}

/// Report produced by [`validate_pblock_slr_assignment`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlrValidationReport {
    pub violations: Vec<SlrViolation>,
}

impl SlrValidationReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.violations
            .iter()
            .any(|v| v.code.eq_ignore_ascii_case(code))
    }
}

/// Validate pblock SLR floorplanning assignments, dedicated column cascade spans, and Laguna SLL
/// interposer cut utilization on `topology` (UG949 / UG906):
///
/// - **`SLR-1`**: Pblock coordinate range `[y_min, y_max]` or `target_slr` lies outside device SLR bounds
///   or mismatches `target_slr`.
/// - **`SLR-2`**: Pblock resource demand (`CLB`, `DSP48E2`, `RAMB36E2`, `URAM288`) exceeds the capacity
///   of the spanned SLR(s).
/// - **`SLR-3`**: Dedicated hardware cascade (`CARRY8`, `DSP48E2`, `URAM288`, `RAMB36E2`) crosses an
///   SLR silicon interposer cut (`slr_of_y(y_start) != slr_of_y(y_end)`).
/// - **`SLR-4`**: SLL interposer boundary cut demand exceeds physical Laguna SLL wire capacity (`sll_wires_per_cut`).
pub fn validate_pblock_slr_assignment(
    topology: &SlrTopology,
    pblocks: &[PblockSlrSpec],
    cascades: &[SlrCascadeSpan],
    sll_cut_demands: &[u32],
) -> SlrValidationReport {
    let mut report = SlrValidationReport::default();

    for pb in pblocks {
        let slr_lo = topology.slr_of_y(pb.y_min);
        let slr_hi = topology.slr_of_y(pb.y_max);
        if pb.y_min > pb.y_max || slr_lo.is_none() || slr_hi.is_none() {
            report.violations.push(SlrViolation {
                code: "SLR-1".into(),
                object: pb.name.clone(),
                message: format!(
                    "Pblock '{}' Y range [{}..={}] is outside device SLR bounds",
                    pb.name, pb.y_min, pb.y_max
                ),
            });
            continue;
        }
        let lo = slr_lo.unwrap();
        let hi = slr_hi.unwrap();
        if let Some(target) = pb.target_slr {
            if target >= topology.num_slrs() || lo != target || hi != target {
                report.violations.push(SlrViolation {
                    code: "SLR-1".into(),
                    object: pb.name.clone(),
                    message: format!(
                        "Pblock '{}' pinned to SLR{} but Y range [{}..={}] spans SLR{}..SLR{}",
                        pb.name, target, pb.y_min, pb.y_max, lo, hi
                    ),
                });
            }
        }

        // Sum capacity across spanned SLRs lo..=hi
        let mut cap_clb = 0u32;
        let mut cap_dsp = 0u32;
        let mut cap_bram = 0u32;
        let mut cap_uram = 0u32;
        for s_id in lo..=hi {
            let r = &topology.slrs[s_id as usize];
            cap_clb += r.clb_slices;
            cap_dsp += r.dsp48e2;
            cap_bram += r.ramb36e2;
            cap_uram += r.uram288;
        }
        if pb.req_clb_slices > cap_clb
            || pb.req_dsp48e2 > cap_dsp
            || pb.req_ramb36e2 > cap_bram
            || pb.req_uram288 > cap_uram
        {
            report.violations.push(SlrViolation {
                code: "SLR-2".into(),
                object: pb.name.clone(),
                message: format!(
                    "Pblock '{}' resource demand (CLB={}, DSP={}, BRAM36={}, URAM288={}) exceeds SLR{}..SLR{} capacity (CLB={}, DSP={}, BRAM36={}, URAM288={})",
                    pb.name,
                    pb.req_clb_slices,
                    pb.req_dsp48e2,
                    pb.req_ramb36e2,
                    pb.req_uram288,
                    lo,
                    hi,
                    cap_clb,
                    cap_dsp,
                    cap_bram,
                    cap_uram
                ),
            });
        }
    }

    // SLR-3: Dedicated column cascades cannot cross an SLR interposer cut
    for cas in cascades {
        match (topology.slr_of_y(cas.y_start), topology.slr_of_y(cas.y_end)) {
            (Some(s0), Some(s1)) if s0 != s1 => {
                report.violations.push(SlrViolation {
                    code: "SLR-3".into(),
                    object: cas.name.clone(),
                    message: format!(
                        "Dedicated {} cascade '{}' (Y={}..{}) crosses interposer boundary between SLR{} and SLR{}",
                        cas.kind.as_str(),
                        cas.name,
                        cas.y_start,
                        cas.y_end,
                        s0,
                        s1
                    ),
                });
            }
            (None, _) | (_, None) => {
                report.violations.push(SlrViolation {
                    code: "SLR-1".into(),
                    object: cas.name.clone(),
                    message: format!(
                        "Cascade '{}' Y span [{}..{}] lies outside device SLR bounds",
                        cas.name, cas.y_start, cas.y_end
                    ),
                });
            }
            _ => {}
        }
    }

    // SLR-4: Check SLL cut utilization against physical Laguna SLL capacity
    for (cut_idx, &demand) in sll_cut_demands.iter().enumerate() {
        if cut_idx >= topology.num_interposer_cuts() {
            report.violations.push(SlrViolation {
                code: "SLR-4".into(),
                object: format!("CUT{cut_idx}"),
                message: format!(
                    "Interposer cut index {} does not exist on {}-SLR device",
                    cut_idx,
                    topology.num_slrs()
                ),
            });
        } else if demand > topology.sll_wires_per_cut {
            report.violations.push(SlrViolation {
                code: "SLR-4".into(),
                object: format!("SLR{cut_idx}<->SLR{}", cut_idx + 1),
                message: format!(
                    "SLL crossing demand ({}) between SLR{} and SLR{} exceeds Laguna SLL capacity ({})",
                    demand,
                    cut_idx,
                    cut_idx + 1,
                    topology.sll_wires_per_cut
                ),
            });
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ultrascale_clb_slice_and_carry8_single_and_dual_cy4_evaluation() {
        let slice = UltraScaleClbSlice::new(12, 45, UltraScaleSliceKind::SliceM);
        assert_eq!(slice.site_name(), "SLICE_X12Y45");
        let bels = slice.bels();
        // 8 * 4 (6LUT, 5LUT, FF, FF2) + 7 MUX + 1 CARRY8 = 40 BELs
        assert_eq!(bels.len(), 40);
        assert!(bels.contains(&"A6LUT".to_string()));
        assert!(bels.contains(&"HFF2".to_string()));
        assert!(bels.contains(&"F9MUX".to_string()));
        assert!(bels.contains(&"CARRY8".to_string()));
        assert!(UltraScaleClbSlice::same_half_slice("AFF", "DFF2"));
        assert!(!UltraScaleClbSlice::same_half_slice("DFF", "EFF"));

        // CARRY8 SingleCy8: 8-bit ripple addition 0xFF + 1 (S=0xFF, DI=0x00, CI=1) -> O=0x00, CO=0xFF
        let cy8 = Carry8Chain::new(Carry8Mode::SingleCy8);
        let (o, co) = cy8.evaluate(true, false, 0x00, 0xFF);
        assert_eq!(o, 0x00);
        assert_eq!(co, 0xFF);

        // CARRY8 DualCy4: lower nibble uses CI=1, upper nibble uses CI_TOP=0 independently!
        let dual = Carry8Chain::new(Carry8Mode::DualCy4);
        let (o_dual, co_dual) = dual.evaluate(true, false, 0x00, 0xFF);
        assert_eq!(o_dual, 0xF0);
        assert_eq!(co_dual, 0x0F);
        assert!(carry8_delay_ps(16) > carry8_delay_ps(8));
    }

    #[test]
    fn dsp48e2_preadder_multiplier_alu_patdet_and_wide_xor() {
        let cfg = Dsp48e2Config {
            use_preadder: true,
            preadder_subtract: false,
            use_mult: true,
            pipeline_stages: 4,
            wide_xor: Dsp48e2WideXorMode::Xor96,
            pattern: (10 + 5) * 4 + 100,
            mask: 0,
        };
        // P = C + (D + A) * B = 100 + (5 + 10) * 4 = 160
        let out = dsp48e2_evaluate(&cfg, 10, 4, 100, 5, false);
        assert_eq!(out.p, 160);
        assert!(out.patdet, "exact match with pattern=160 must assert PATDET");
        // 160 = 0b1010_0000 has two 1-bits -> even parity -> Xor96 = 0
        assert_eq!(out.xorout, 0);

        let out_odd = dsp48e2_evaluate(&cfg, 10, 4, 101, 5, false); // P = 161 = 0b1010_0001 (three 1-bits)
        assert!(!out_odd.patdet);
        assert_eq!(out_odd.xorout, 1);
        assert!(dsp48e2_delay_ps(&cfg) < 450);
    }

    #[test]
    fn uram288_secded_ecc_and_column_cascade_planning() {
        let word: u64 = 0xDEAD_BEEF_CAFE_BABE;
        let cw = uram288_ecc_encode_64(word);
        assert_eq!(uram288_ecc_decode_72(cw), (word, false, false));

        // Flip any single bit in the 72-bit codeword -> corrected cleanly!
        for bit in 0..72u32 {
            let corrupted = cw ^ (1u128 << bit);
            let (fixed, sbe, dbe) = uram288_ecc_decode_72(corrupted);
            assert_eq!(fixed, word, "must correct single-bit flip at bit {bit}");
            assert!(sbe);
            assert!(!dbe);
        }

        // Flip two bits -> double-bit error (DBE) detected!
        let double_corrupted = cw ^ (1u128 << 3) ^ (1u128 << 19);
        let (_, _, dbe) = uram288_ecc_decode_72(double_corrupted);
        assert!(dbe, "two-bit error must assert double_bit_error");

        // Plan a 128K x 72-bit memory on xcvu9p (requires 32 URAM288 blocks in 1 SLR column)
        let vu9p = UltraScaleDevice::for_part(UltraScalePart::Xcvu9p);
        let plan = plan_uram288_cascade(&vu9p, 131_072, 72, 4).expect("valid URAM288 cascade");
        assert_eq!(plan.depth_cascade_len, 32);
        assert_eq!(plan.width_stripes, 1);
        assert_eq!(plan.total_uram288_blocks, 32);
        assert_eq!(plan.slr_columns_needed, 1);

        // xczu9eg has 0 URAM288 -> error
        let zu9eg = UltraScaleDevice::for_part(UltraScalePart::Xczu9eg);
        assert!(plan_uram288_cascade(&zu9eg, 4096, 72, 1).is_err());
    }

    #[test]
    fn multi_slr_laguna_sll_delays_and_pblock_validation() {
        let vu9p = UltraScaleDevice::for_part(UltraScalePart::Xcvu9p);
        let topo = vu9p.slr_topology();
        assert_eq!(topo.num_slrs(), 3);
        assert_eq!(topo.num_interposer_cuts(), 2);
        assert_eq!(topo.slr_of_y(0), Some(0));
        assert_eq!(topo.slr_of_y(250), Some(1));
        assert_eq!(topo.slr_of_y(500), Some(2));

        // Same-SLR delay is 0; 2-hop SLR0->SLR2 is 2x single-hop SLR0->SLR1; TxRxRegistered < Unregistered
        assert_eq!(
            sll_crossing_delay_ps(1, 1, LagunaCrossingMode::TxRxRegistered, 50.0),
            0
        );
        let d_reg = sll_crossing_delay_ps(0, 1, LagunaCrossingMode::TxRxRegistered, 50.0);
        let d_unreg = sll_crossing_delay_ps(0, 1, LagunaCrossingMode::Unregistered, 50.0);
        let d_2hop = sll_crossing_delay_ps(0, 2, LagunaCrossingMode::TxRxRegistered, 50.0);
        let d_congested = sll_crossing_delay_ps(0, 1, LagunaCrossingMode::TxRxRegistered, 95.0);
        assert!(d_reg < d_unreg);
        assert_eq!(d_2hop, d_reg * 2);
        assert!(d_congested > d_reg);

        // Validate clean pblock vs SLR-crossing URAM288 cascade and overloaded SLL cut
        let clean_pb = PblockSlrSpec {
            name: "pblock_slr0".into(),
            target_slr: Some(0),
            y_min: 10,
            y_max: 200,
            req_clb_slices: 10_000,
            req_dsp48e2: 500,
            req_ramb36e2: 200,
            req_uram288: 128,
        };
        let clean_cas = SlrCascadeSpan {
            name: "uram_cas_slr0".into(),
            kind: SlrCascadeKind::Uram288,
            y_start: 20,
            y_end: 180,
        };
        let ok_rep =
            validate_pblock_slr_assignment(&topo, &[clean_pb.clone()], &[clean_cas], &[8_000, 9_000]);
        assert!(ok_rep.is_clean());

        let cross_cas = SlrCascadeSpan {
            name: "illegal_cross_slr_uram".into(),
            kind: SlrCascadeKind::Uram288,
            y_start: 200, // SLR0
            y_end: 260,   // SLR1 -> crosses interposer cut!
        };
        let bad_rep = validate_pblock_slr_assignment(
            &topo,
            &[clean_pb],
            &[cross_cas],
            &[30_000, 5_000], // Cut 0 exceeds 23,040 SLL wires!
        );
        assert!(bad_rep.has_code("SLR-3"));
        assert!(bad_rep.has_code("SLR-4"));
    }
}
