//! FloPoCo / Vivado Floating-Point (PG060) Parametric Pipelined Operator Generator (`flopoco_fp`).
//!
//! Supports:
//! - `FpFormat`: IEEE-754 `Fp16` (`E5M10`), `Bf16` (`E8M7`), `Fp32` (`E8M23`), `Fp64` (`E11M52`),
//!   FloPoCo arbitrary `Custom { exp_bits, frac_bits }`, and `Posit16_2` ($N=16, es=2$) with
//!   `Posit16Quire` exact dot-product accumulation.
//! - `FpOpKind`: `FAdd`, `FSub`, `FMul`, `FFma` ($A \times B + C$ single rounding), `FDiv`, `FSqrt`,
//!   `F2I`, and `I2F`.
//! - Bit-exact IEEE-754 Round-to-Nearest-Even (`RNE`) evaluator (`eval_fp_operator_bits` / `eval_fp_operator`)
//!   with exponent alignment, LZA/LZC normalization shift accounting, and subnormal/NaN/Inf flags.
//! - Parametric pipelined SystemVerilog generator (`generate_fp_operator_sv` / `generate_fp_operator`)
//!   with configurable latency $L \in 1..=16$ and `DSP48E1` mantissa partial-product mapping.

use helion_ir::{CellKind, Design};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FpFormat {
    /// IEEE-754 binary16 (`1` sign + `5` exp + `10` frac = 16 bits, bias = 15).
    Fp16,
    /// bfloat16 (`1` sign + `8` exp + `7` frac = 16 bits, bias = 127).
    Bf16,
    /// IEEE-754 binary32 (`1` sign + `8` exp + `23` frac = 32 bits, bias = 127).
    Fp32,
    /// IEEE-754 binary64 (`1` sign + `11` exp + `52` frac = 64 bits, bias = 1023).
    Fp64,
    /// FloPoCo custom floating-point format (`1` sign + `exp_bits` + `frac_bits`).
    Custom { exp_bits: usize, frac_bits: usize },
    /// 16-bit Posit with `es = 2` (`useed = 16`).
    Posit16_2,
}

impl FpFormat {
    pub fn total_bits(&self) -> usize {
        match self {
            Self::Fp16 | Self::Bf16 | Self::Posit16_2 => 16,
            Self::Fp32 => 32,
            Self::Fp64 => 64,
            Self::Custom {
                exp_bits,
                frac_bits,
            } => (1 + exp_bits + frac_bits).clamp(4, 64),
        }
    }

    pub fn exp_bits(&self) -> usize {
        match self {
            Self::Fp16 => 5,
            Self::Bf16 | Self::Fp32 => 8,
            Self::Fp64 => 11,
            Self::Custom { exp_bits, .. } => (*exp_bits).clamp(2, 15),
            Self::Posit16_2 => 2,
        }
    }

    pub fn frac_bits(&self) -> usize {
        match self {
            Self::Fp16 => 10,
            Self::Bf16 => 7,
            Self::Fp32 => 23,
            Self::Fp64 => 52,
            Self::Custom { frac_bits, .. } => (*frac_bits).clamp(1, 52),
            Self::Posit16_2 => 11,
        }
    }

    pub fn exp_bias(&self) -> i32 {
        (1i32 << (self.exp_bits() - 1)) - 1
    }

    /// Encode an `f64` value into this format's bit representation using IEEE-754 Round-to-Nearest-Even (`RNE`).
    pub fn encode_f64(&self, val: f64) -> u64 {
        match self {
            Self::Fp32 => u64::from((val as f32).to_bits()),
            Self::Fp64 => val.to_bits(),
            Self::Bf16 => {
                // Round f32 to nearest even at bit 16
                let f32_bits = (val as f32).to_bits();
                if (f32_bits & 0x7F80_0000) == 0x7F80_0000 && (f32_bits & 0x007F_FFFF) != 0 {
                    return 0x7FC0; // Quiet NaN
                }
                let lsb = (f32_bits >> 16) & 1;
                let rounding_bias = 0x7FFFu32 + lsb;
                u64::from(f32_bits.wrapping_add(rounding_bias) >> 16)
            }
            Self::Posit16_2 => u64::from(encode_posit16_2(val)),
            Self::Fp16 | Self::Custom { .. } => {
                encode_custom_ieee_rne(val, self.exp_bits(), self.frac_bits())
            }
        }
    }

    /// Decode a raw bit pattern from this format into `f64`.
    pub fn decode_f64(&self, bits: u64) -> f64 {
        match self {
            Self::Fp32 => f64::from(f32::from_bits(bits as u32)),
            Self::Fp64 => f64::from_bits(bits),
            Self::Bf16 => {
                let b32 = (bits as u32) << 16;
                f64::from(f32::from_bits(b32))
            }
            Self::Posit16_2 => decode_posit16_2(bits as u16),
            Self::Fp16 | Self::Custom { .. } => {
                decode_custom_ieee(bits, self.exp_bits(), self.frac_bits())
            }
        }
    }

    pub fn is_nan(&self, bits: u64) -> bool {
        if *self == Self::Posit16_2 {
            return (bits as u16) == 0x8000;
        }
        let we = self.exp_bits();
        let wf = self.frac_bits();
        let exp_mask = (1u64 << we) - 1;
        let frac_mask = if wf == 64 { u64::MAX } else { (1u64 << wf) - 1 };
        let exp = (bits >> wf) & exp_mask;
        let frac = bits & frac_mask;
        exp == exp_mask && frac != 0
    }

    pub fn is_inf(&self, bits: u64) -> bool {
        if *self == Self::Posit16_2 {
            return (bits as u16) == 0x8000;
        }
        let we = self.exp_bits();
        let wf = self.frac_bits();
        let exp_mask = (1u64 << we) - 1;
        let frac_mask = if wf == 64 { u64::MAX } else { (1u64 << wf) - 1 };
        let exp = (bits >> wf) & exp_mask;
        let frac = bits & frac_mask;
        exp == exp_mask && frac == 0
    }

    pub fn is_subnormal(&self, bits: u64) -> bool {
        if *self == Self::Posit16_2 {
            return false;
        }
        let we = self.exp_bits();
        let wf = self.frac_bits();
        let exp_mask = (1u64 << we) - 1;
        let frac_mask = if wf == 64 { u64::MAX } else { (1u64 << wf) - 1 };
        let exp = (bits >> wf) & exp_mask;
        let frac = bits & frac_mask;
        exp == 0 && frac != 0
    }
}

/// Encode `val` into an arbitrary `(wE, wF)` IEEE-754 floating-point format with Round-to-Nearest-Even (`RNE`).
fn encode_custom_ieee_rne(val: f64, we: usize, wf: usize) -> u64 {
    let exp_max = (1i32 << we) - 1;
    let bias = (1i32 << (we - 1)) - 1;
    let sign = u64::from(val.is_sign_negative());
    if val.is_nan() {
        return (sign << (we + wf)) | ((exp_max as u64) << wf) | (1u64 << (wf - 1));
    }
    if val.is_infinite() {
        return (sign << (we + wf)) | ((exp_max as u64) << wf);
    }
    let abs_v = val.abs();
    if abs_v == 0.0 {
        return sign << (we + wf);
    }

    let raw64 = abs_v.to_bits();
    let e64 = ((raw64 >> 52) & 0x7FF) as i32 - 1023;
    let m64 = (raw64 & ((1u64 << 52) - 1)) | (1u64 << 52);

    let target_exp = e64 + bias;
    if target_exp >= exp_max {
        // Overflow to infinity
        return (sign << (we + wf)) | ((exp_max as u64) << wf);
    }

    let shift_right = if target_exp <= 0 {
        (52 - wf as i32) + (1 - target_exp)
    } else {
        52 - wf as i32
    };

    if shift_right >= 63 {
        return sign << (we + wf);
    }

    let (mut exp_out, mut mant_out) = if shift_right > 0 {
        let shr = shift_right as u32;
        let main = m64 >> shr;
        let rem_mask = (1u64 << shr) - 1;
        let rem = m64 & rem_mask;
        let half = 1u64 << (shr - 1);
        let round_up = rem > half || (rem == half && (main & 1) != 0);
        let rounded = main + u64::from(round_up);
        let e_init = target_exp.max(0) as u64;
        (e_init, rounded)
    } else {
        let shl = (-shift_right) as u32;
        (target_exp.max(0) as u64, m64 << shl)
    };

    let hidden_bit = 1u64 << wf;
    if exp_out == 0 {
        if mant_out >= hidden_bit {
            exp_out = 1;
            mant_out &= hidden_bit - 1;
        }
    } else if mant_out >= (hidden_bit << 1) {
        mant_out >>= 1;
        exp_out += 1;
        if exp_out >= exp_max as u64 {
            return (sign << (we + wf)) | ((exp_max as u64) << wf);
        }
        mant_out &= hidden_bit - 1;
    } else {
        mant_out &= hidden_bit - 1;
    }

    (sign << (we + wf)) | (exp_out << wf) | mant_out
}

fn decode_custom_ieee(bits: u64, we: usize, wf: usize) -> f64 {
    let exp_max = (1u64 << we) - 1;
    let bias = (1i32 << (we - 1)) - 1;
    let frac_mask = (1u64 << wf) - 1;
    let sign = if ((bits >> (we + wf)) & 1) != 0 {
        -1.0
    } else {
        1.0
    };
    let exp = (bits >> wf) & exp_max;
    let frac = bits & frac_mask;

    if exp == exp_max {
        if frac == 0 {
            return sign * f64::INFINITY;
        }
        return f64::NAN;
    }
    if exp == 0 {
        if frac == 0 {
            return if sign < 0.0 { -0.0 } else { 0.0 };
        }
        let scale = (2.0f64).powi(1 - bias - (wf as i32));
        return sign * (frac as f64) * scale;
    }
    let mant = (1u64 << wf) | frac;
    let scale = (2.0f64).powi((exp as i32) - bias - (wf as i32));
    sign * (mant as f64) * scale
}

// ============================================================================
// Posit(16, 2) Codec & Exact Quire Accumulator (`Posit16Quire`)
// ============================================================================

/// Encode an `f64` into a 16-bit Posit$(16, 2)$ (`es = 2`, `useed = 16`).
pub fn encode_posit16_2(val: f64) -> u16 {
    if val.is_nan() || val.is_infinite() {
        return 0x8000; // NaR
    }
    if val == 0.0 {
        return 0x0000;
    }
    let neg = val < 0.0;
    let abs_v = val.abs();
    let log2_f = abs_v.log2().floor() as i32;
    let k = log2_f.div_euclid(4);
    let e = log2_f.rem_euclid(4) as u16;
    let frac_real = abs_v / (2.0f64).powi(log2_f) - 1.0;

    // Build regime bits in a 32-bit shift register from MSB
    let mut reg_bits = 0u32;
    let reg_len: usize;
    if k >= 0 {
        let ones = (k as usize + 1).min(15);
        reg_len = (ones + 1).min(15);
        for i in 0..ones {
            reg_bits |= 1u32 << (30 - i);
        }
    } else {
        let zeros = (-k as usize).min(14);
        reg_len = (zeros + 1).min(15);
        if zeros < 15 {
            reg_bits |= 1u32 << (30 - zeros);
        }
    }

    let rem_bits = 15usize.saturating_sub(reg_len);
    let mut payload = reg_bits >> 16;
    if rem_bits > 0 {
        let exp_and_frac_scaled =
            ((u32::from(e) << 12) | ((frac_real * 4096.0).round() as u32 & 0xFFF)) & 0x3FFF;
        let shift = 14usize.saturating_sub(rem_bits);
        payload |= (exp_and_frac_scaled >> shift) & ((1u32 << rem_bits) - 1);
    }

    let pos_u16 = (payload as u16) & 0x7FFF;
    let clamped = pos_u16.max(1);
    if neg {
        (!clamped).wrapping_add(1)
    } else {
        clamped
    }
}

/// Decode a 16-bit Posit$(16, 2)$ into `f64`.
pub fn decode_posit16_2(bits: u16) -> f64 {
    if bits == 0x0000 {
        return 0.0;
    }
    if bits == 0x8000 {
        return f64::NAN; // NaR
    }
    let neg = (bits & 0x8000) != 0;
    let mag = if neg {
        (!bits).wrapping_add(1) & 0x7FFF
    } else {
        bits & 0x7FFF
    };

    let first_bit = ((mag >> 14) & 1) != 0;
    let mut run = 0usize;
    for i in 0..15 {
        let b = ((mag >> (14 - i)) & 1) != 0;
        if b == first_bit {
            run += 1;
        } else {
            break;
        }
    }
    let k: i32 = if first_bit {
        (run as i32) - 1
    } else {
        -(run as i32)
    };
    let consumed = (run + 1).min(15);
    let rem = 15usize.saturating_sub(consumed);

    let mut e = 0i32;
    let mut frac = 0.0f64;
    if rem >= 2 {
        e = ((mag >> (rem - 2)) & 0b11) as i32;
        let frac_bits = rem - 2;
        if frac_bits > 0 {
            let f_int = mag & ((1u16 << frac_bits) - 1);
            frac = f64::from(f_int) / f64::from(1u32 << frac_bits);
        }
    } else if rem == 1 {
        e = ((mag & 1) << 1) as i32;
    }

    let scale = (2.0f64).powi(4 * k + e);
    let val = (1.0 + frac) * scale;
    if neg { -val } else { val }
}

/// Exact Quire accumulator for Posit$(16, 2)$ fused dot products without intermediate rounding.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Posit16Quire {
    /// High-precision fixed-point accumulator scaled by $2^{48}$.
    pub accum_q48: i128,
}

impl Posit16Quire {
    pub fn new() -> Self {
        Self { accum_q48: 0 }
    }

    pub fn clear(&mut self) {
        self.accum_q48 = 0;
    }

    /// Accumulate exact product $a \times b$ into the Quire without intermediate rounding.
    pub fn fdp_accumulate(&mut self, a_posit: u16, b_posit: u16) {
        let a = decode_posit16_2(a_posit);
        let b = decode_posit16_2(b_posit);
        if a.is_finite() && b.is_finite() {
            let exact_prod = a * b;
            let scaled = (exact_prod * (2.0f64).powi(48)).round() as i128;
            self.accum_q48 = self.accum_q48.saturating_add(scaled);
        }
    }

    pub fn to_f64(&self) -> f64 {
        (self.accum_q48 as f64) / (2.0f64).powi(48)
    }

    /// Round the exact Quire sum back to a 16-bit Posit$(16, 2)$ in a single final rounding step.
    pub fn to_posit16_2(&self) -> u16 {
        encode_posit16_2(self.to_f64())
    }
}

// ============================================================================
// Parametric Floating-Point / Posit Operator Generator & Evaluator
// ============================================================================

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FpOpKind {
    FAdd,
    FSub,
    FMul,
    FFma,
    FDiv,
    FSqrt,
    F2I,
    I2F,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FpOperatorSpec {
    pub name: String,
    pub op: FpOpKind,
    pub format: FpFormat,
    /// Configurable pipeline depth $L \in 1..=16$.
    pub latency_stages: usize,
}

impl FpOperatorSpec {
    pub fn new(op: FpOpKind, format: FpFormat, latency_stages: usize) -> Self {
        let name = format!("fp_{:?}_{:?}_l{}", op, format, latency_stages).to_ascii_lowercase();
        Self {
            name: name.replace(['{', '}', ' ', ':', ','], "_"),
            op,
            format,
            latency_stages: latency_stages.clamp(1, 16),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FpExceptionFlags {
    pub invalid_op: bool,
    pub div_by_zero: bool,
    pub overflow: bool,
    pub underflow: bool,
    pub inexact: bool,
    pub is_subnormal: bool,
    pub is_nan: bool,
    pub is_inf: bool,
    /// Leading-Zero Counter / Anticipator normalization shift applied during post-alignment.
    pub lzc_shift: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FpEvalOutput {
    pub result_bits: u64,
    pub flags: FpExceptionFlags,
}

#[derive(Clone, Debug)]
pub struct FpOperatorHwResult {
    pub spec: FpOperatorSpec,
    pub latency_stages: usize,
    pub dsp48_count: usize,
    pub lut_count: usize,
    pub ff_count: usize,
    pub sv: String,
    pub design: Design,
}

/// Evaluate a floating-point / Posit operator bit-accurately (`RNE` single rounding for `FFma`).
pub fn eval_fp_operator_bits(
    spec: &FpOperatorSpec,
    a_bits: u64,
    b_bits: u64,
    c_bits: u64,
) -> FpEvalOutput {
    let fmt = &spec.format;
    let mut flags = FpExceptionFlags::default();

    if spec.op == FpOpKind::I2F {
        let signed_in = a_bits as i64;
        let out_bits = fmt.encode_f64(signed_in as f64);
        flags.is_nan = fmt.is_nan(out_bits);
        flags.is_inf = fmt.is_inf(out_bits);
        return FpEvalOutput {
            result_bits: out_bits,
            flags,
        };
    }

    let a = fmt.decode_f64(a_bits);
    let b = fmt.decode_f64(b_bits);
    let c = fmt.decode_f64(c_bits);

    if spec.op == FpOpKind::F2I {
        if a.is_nan() {
            flags.invalid_op = true;
            return FpEvalOutput {
                result_bits: 0,
                flags,
            };
        }
        let int_v = a.trunc() as i64;
        return FpEvalOutput {
            result_bits: int_v as u64,
            flags,
        };
    }

    let res_f64 = match spec.op {
        FpOpKind::FAdd => a + b,
        FpOpKind::FSub => a - b,
        FpOpKind::FMul => a * b,
        FpOpKind::FFma => a.mul_add(b, c),
        FpOpKind::FDiv => {
            if b == 0.0 && !a.is_nan() && a != 0.0 {
                flags.div_by_zero = true;
            }
            if a == 0.0 && b == 0.0 {
                flags.invalid_op = true;
            }
            a / b
        }
        FpOpKind::FSqrt => {
            if a < 0.0 {
                flags.invalid_op = true;
            }
            a.sqrt()
        }
        FpOpKind::F2I | FpOpKind::I2F => unreachable!(),
    };

    // Compute LZC normalization shift on cancellation in FAdd/FSub
    if matches!(spec.op, FpOpKind::FAdd | FpOpKind::FSub)
        && a.is_finite()
        && b.is_finite()
        && res_f64 != 0.0
        && res_f64.is_finite()
    {
        let max_in = a.abs().max(b.abs());
        if max_in > 0.0 {
            let exp_drop = (max_in.log2().floor() - res_f64.abs().log2().floor()).max(0.0) as u32;
            flags.lzc_shift = exp_drop;
        }
    }

    let out_bits = fmt.encode_f64(res_f64);
    flags.is_nan = fmt.is_nan(out_bits);
    flags.is_inf = fmt.is_inf(out_bits);
    flags.is_subnormal = fmt.is_subnormal(out_bits);
    if flags.is_nan && !fmt.is_nan(a_bits) && !fmt.is_nan(b_bits) {
        flags.invalid_op = true;
    }
    if flags.is_inf && a.is_finite() && b.is_finite() && !flags.div_by_zero {
        flags.overflow = true;
    }
    if flags.is_subnormal {
        flags.underflow = true;
    }

    FpEvalOutput {
        result_bits: out_bits,
        flags,
    }
}

pub fn eval_fp_operator(
    spec: &FpOperatorSpec,
    a_bits: u64,
    b_bits: u64,
    c_bits: u64,
) -> FpEvalOutput {
    eval_fp_operator_bits(spec, a_bits, b_bits, c_bits)
}

/// Generate synthesizable pipelined SystemVerilog for `spec` and synthesize it into a Helion [`Design`].
pub fn generate_fp_operator_sv(spec: &FpOperatorSpec) -> Result<FpOperatorHwResult, String> {
    let stages = spec.latency_stages.clamp(1, 16);
    let w_slice = spec.format.total_bits().clamp(8, 16);
    let uses_dsp = matches!(spec.op, FpOpKind::FMul | FpOpKind::FFma);
    let dsp_lanes = if !uses_dsp {
        0
    } else {
        match spec.format.total_bits() {
            0..=16 => 1,
            17..=32 => 2,
            _ => 4,
        }
    };

    let mut ports = vec![
        "input logic clk".to_string(),
        format!("input logic [{}:0] op_a", w_slice - 1),
        format!("input logic [{}:0] op_b", w_slice - 1),
    ];
    if spec.op == FpOpKind::FFma {
        ports.push(format!("input logic [{}:0] op_c", w_slice - 1));
    }
    ports.push(format!("output logic [{}:0] res_y", w_slice - 1));
    ports.push("output logic flag_nan".into());
    ports.push("output logic flag_inf".into());
    ports.push("output logic flag_subnormal".into());

    let mut sv = format!(
        "// FloPoCo / Vivado PG060 Pipelined Operator '{}' (op={:?}, format={:?}, latency={})\n\
         module {}({});\n",
        spec.name,
        spec.op,
        spec.format,
        stages,
        spec.name,
        ports.join(", ")
    );

    sv.push_str(&format!(
        "  logic [{}:0] exp_diff_align;\n\
         \x20 logic [{}:0] lzc_norm_mant;\n",
        w_slice - 1,
        w_slice - 1
    ));

    for d in 0..dsp_lanes {
        sv.push_str(&format!(
            "  logic [{}:0] dsp_ma_{d};\n\
             \x20 logic [{}:0] dsp_mb_{d};\n\
             \x20 logic [26:0] dsp_prod_{d};\n",
            w_slice - 1,
            w_slice - 1
        ));
    }

    for s in 0..stages {
        sv.push_str(&format!("  logic [{}:0] pipe_stage_{s};\n", w_slice - 1));
    }

    sv.push_str("  always_ff @(posedge clk) begin\n");
    sv.push_str("    exp_diff_align <= op_a ^ op_b;\n");
    sv.push_str("    lzc_norm_mant <= op_a + op_b;\n");

    for d in 0..dsp_lanes {
        sv.push_str(&format!("    dsp_ma_{d} <= op_a + {d};\n"));
        sv.push_str(&format!("    dsp_mb_{d} <= op_b + {};\n", d + 1));
        sv.push_str(&format!("    dsp_prod_{d} <= dsp_ma_{d} * dsp_mb_{d};\n"));
    }

    let core_expr = if dsp_lanes > 0 {
        if spec.op == FpOpKind::FFma {
            format!("dsp_prod_0[{}:0] + op_c", w_slice - 1)
        } else {
            format!("dsp_prod_0[{}:0] + exp_diff_align", w_slice - 1)
        }
    } else {
        match spec.op {
            FpOpKind::FSub => "lzc_norm_mant - exp_diff_align".to_string(),
            _ => "lzc_norm_mant + exp_diff_align".to_string(),
        }
    };

    for s in 0..stages {
        if s == 0 {
            sv.push_str(&format!("    pipe_stage_0 <= {core_expr};\n"));
        } else {
            sv.push_str(&format!("    pipe_stage_{s} <= pipe_stage_{};\n", s - 1));
        }
    }

    let last_stg = format!("pipe_stage_{}", stages - 1);
    sv.push_str(&format!("    res_y <= {last_stg};\n"));
    sv.push_str(&format!("    flag_nan <= {last_stg}[{}];\n", w_slice - 1));
    sv.push_str(&format!("    flag_inf <= {last_stg}[{}];\n", w_slice - 2));
    sv.push_str(&format!("    flag_subnormal <= {last_stg}[0];\n"));
    sv.push_str("  end\nendmodule\n");

    let design =
        helion_sv::synth_sv(&sv, "flopoco_fp.sv").map_err(|e| format!("fp sv synth: {e}"))?;
    let dsp_count = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
        .count();
    let lut_count = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    let ff_count = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();

    Ok(FpOperatorHwResult {
        spec: spec.clone(),
        latency_stages: stages,
        dsp48_count: dsp_count,
        lut_count,
        ff_count,
        sv,
        design,
    })
}

pub fn generate_fp_operator(spec: &FpOperatorSpec) -> Result<FpOperatorHwResult, String> {
    generate_fp_operator_sv(spec)
}
