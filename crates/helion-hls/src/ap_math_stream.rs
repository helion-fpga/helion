//! Vivado HLS (UG902) Arbitrary-Precision Types (`ap_int<W>`, `ap_uint<W>`,
//! `ap_fixed<W,I,Q,O>`, `ap_ufixed<W,I,Q,O>`), `hls::stream<T>` blocking/non-blocking
//! FIFO channels, and synthesizable `hls::math` intrinsics (`hls::sqrt`, `hls::abs`,
//! `hls::min`, `hls::max`, `hls::clz`, `hls::popcount`, `hls::sin`, `hls::cos`).

use std::collections::VecDeque;

/// Fixed-point quantization modes (UG902 Table 2-6: `ap_q_mode`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApQuantMode {
    /// Truncation to minus infinity (`AP_TRN` — default).
    ApTrn,
    /// Truncation toward zero (`AP_TRN_ZERO`).
    ApTrnZero,
    /// Round to plus infinity / half-up (`AP_RND`).
    ApRnd,
    /// Round toward zero (`AP_RND_ZERO`).
    ApRndZero,
    /// Round toward minus infinity (`AP_RND_MIN_INF`).
    ApRndMinInf,
    /// Round away from zero (`AP_RND_INF`).
    ApRndInf,
    /// Convergent rounding to nearest even (`AP_RND_CONV`).
    ApRndConv,
}

impl ApQuantMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "AP_TRN" => Some(Self::ApTrn),
            "AP_TRN_ZERO" => Some(Self::ApTrnZero),
            "AP_RND" => Some(Self::ApRnd),
            "AP_RND_ZERO" => Some(Self::ApRndZero),
            "AP_RND_MIN_INF" => Some(Self::ApRndMinInf),
            "AP_RND_INF" => Some(Self::ApRndInf),
            "AP_RND_CONV" => Some(Self::ApRndConv),
            _ => None,
        }
    }
}

/// Fixed-point overflow / saturation modes (UG902 Table 2-7: `ap_o_mode`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApOverflowMode {
    /// Two's-complement wrap-around (`AP_WRAP` — default).
    ApWrap,
    /// Saturate to `[MIN, MAX]` (`AP_SAT`).
    ApSat,
    /// Force overflowed values to zero (`AP_SAT_ZERO`).
    ApSatZero,
    /// Symmetrical saturation `[-MAX, +MAX]` (`AP_SAT_SYM`).
    ApSatSym,
}

impl ApOverflowMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "AP_WRAP" => Some(Self::ApWrap),
            "AP_SAT" => Some(Self::ApSat),
            "AP_SAT_ZERO" => Some(Self::ApSatZero),
            "AP_SAT_SYM" => Some(Self::ApSatSym),
            _ => None,
        }
    }
}

/// Bit-accurate arbitrary-precision integer (`ap_int<W>` / `ap_uint<W>`, $1 \le W \le 128$).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApInt {
    pub width: usize,
    pub signed: bool,
    pub raw_bits: u128,
}

impl ApInt {
    pub fn mask_for(width: usize) -> u128 {
        let w = width.clamp(1, 128);
        if w == 128 {
            u128::MAX
        } else {
            (1u128 << w) - 1
        }
    }

    pub fn new(width: usize, signed: bool, val: i128) -> Self {
        let w = width.clamp(1, 128);
        let mask = Self::mask_for(w);
        Self {
            width: w,
            signed,
            raw_bits: (val as u128) & mask,
        }
    }

    pub fn from_u128(width: usize, signed: bool, bits: u128) -> Self {
        let w = width.clamp(1, 128);
        Self {
            width: w,
            signed,
            raw_bits: bits & Self::mask_for(w),
        }
    }

    pub fn to_u128(&self) -> u128 {
        self.raw_bits & Self::mask_for(self.width)
    }

    pub fn to_i128(&self) -> i128 {
        let u = self.to_u128();
        if !self.signed || self.width >= 128 {
            return u as i128;
        }
        let sign_bit = 1u128 << (self.width - 1);
        if (u & sign_bit) != 0 {
            let ext = u | (!Self::mask_for(self.width));
            ext as i128
        } else {
            u as i128
        }
    }

    /// Extract inclusive bit slice `[hi:lo]` (`x.range(hi, lo)` or `x(hi, lo)` in Vivado HLS).
    /// Supports both `hi >= lo` (standard slice) and `hi < lo` (bit-reversed slice per UG902!).
    pub fn range(&self, hi: usize, lo: usize) -> ApInt {
        if hi >= lo {
            let slice_w = (hi - lo + 1).clamp(1, 128);
            let shifted = (self.raw_bits >> lo) & Self::mask_for(slice_w);
            ApInt::from_u128(slice_w, false, shifted)
        } else {
            let slice_w = (lo - hi + 1).clamp(1, 128);
            let mut rev = 0u128;
            for i in 0..slice_w {
                let b = (self.raw_bits >> (hi + i)) & 1;
                rev = (rev << 1) | b;
            }
            ApInt::from_u128(slice_w, false, rev)
        }
    }

    /// Assign `val` into inclusive bit slice `[hi:lo]`.
    pub fn set_range(&mut self, hi: usize, lo: usize, val: u128) {
        let (low, high) = if hi >= lo { (lo, hi) } else { (hi, lo) };
        let slice_w = (high - low + 1).clamp(1, 128);
        let slice_mask = Self::mask_for(slice_w) << low;
        self.raw_bits = (self.raw_bits & !slice_mask) | ((val & Self::mask_for(slice_w)) << low);
        self.raw_bits &= Self::mask_for(self.width);
    }

    pub fn bit(&self, idx: usize) -> bool {
        ((self.raw_bits >> idx) & 1) != 0
    }

    pub fn set_bit(&mut self, idx: usize, val: bool) {
        if idx < self.width {
            if val {
                self.raw_bits |= 1u128 << idx;
            } else {
                self.raw_bits &= !(1u128 << idx);
            }
        }
    }

    /// Concatenate `(self, low)` into a `(self.width + low.width)`-bit `ApInt`.
    pub fn concat(&self, low: &ApInt) -> ApInt {
        let total_w = (self.width + low.width).clamp(1, 128);
        let bits = (self.to_u128() << low.width) | low.to_u128();
        ApInt::from_u128(total_w, false, bits)
    }
}

/// Bit-accurate arbitrary-precision fixed-point number (`ap_fixed<W, I, Q, O>` / `ap_ufixed<W, I, Q, O>`).
///
/// `width` ($W$) is total bits, `int_bits` ($I$) is integer bits, so fractional bits $F = W - I$.
/// The underlying fixed-point value is $\text{raw\_scaled} \times 2^{-F}$.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApFixed {
    pub width: usize,
    pub int_bits: isize,
    pub signed: bool,
    pub quant: ApQuantMode,
    pub overflow: ApOverflowMode,
    pub raw_scaled: i128,
}

impl ApFixed {
    pub fn frac_bits(&self) -> isize {
        (self.width as isize) - self.int_bits
    }

    /// Quantize a real value `val` (or high-precision fixed value) into `ap_fixed<W, I, Q, O>`.
    pub fn from_f64(
        val: f64,
        width: usize,
        int_bits: isize,
        signed: bool,
        quant: ApQuantMode,
        overflow: ApOverflowMode,
    ) -> Self {
        let w = width.clamp(1, 64);
        let f_bits = (w as isize) - int_bits;
        let scale = (2.0f64).powi(f_bits as i32);
        let exact = val * scale;

        let floor_v = exact.floor();
        let rem = exact - floor_v;
        let q_int: i128 = match quant {
            ApQuantMode::ApTrn | ApQuantMode::ApRndMinInf => floor_v as i128,
            ApQuantMode::ApTrnZero => exact.trunc() as i128,
            ApQuantMode::ApRnd => (exact + 0.5).floor() as i128,
            ApQuantMode::ApRndZero => {
                if exact >= 0.0 {
                    if rem > 0.5 {
                        floor_v as i128 + 1
                    } else {
                        floor_v as i128
                    }
                } else {
                    (exact - 0.5).ceil() as i128
                }
            }
            ApQuantMode::ApRndInf => {
                if exact >= 0.0 {
                    (exact + 0.5).floor() as i128
                } else {
                    (exact - 0.5).ceil() as i128
                }
            }
            ApQuantMode::ApRndConv => {
                let fl = floor_v as i128;
                if (rem - 0.5).abs() < 1e-12 {
                    if fl % 2 == 0 {
                        fl
                    } else {
                        fl + 1
                    }
                } else {
                    (exact + 0.5).floor() as i128
                }
            }
        };

        let raw_scaled = Self::apply_overflow(q_int, w, signed, overflow);
        Self {
            width: w,
            int_bits,
            signed,
            quant,
            overflow,
            raw_scaled,
        }
    }

    /// Quantize from a higher-precision fixed-point integer `hi_val` with `extra_frac_bits` fractional LSBs.
    pub fn from_scaled_int(
        hi_val: i128,
        extra_frac_bits: usize,
        width: usize,
        int_bits: isize,
        signed: bool,
        quant: ApQuantMode,
        overflow: ApOverflowMode,
    ) -> Self {
        let w = width.clamp(1, 64);
        let q_int = if extra_frac_bits == 0 {
            hi_val
        } else {
            let half = 1i128 << (extra_frac_bits - 1);
            let mask = (1i128 << extra_frac_bits) - 1;
            let base = hi_val >> extra_frac_bits;
            let rem = hi_val & mask;
            match quant {
                ApQuantMode::ApTrn | ApQuantMode::ApRndMinInf => base,
                ApQuantMode::ApTrnZero => {
                    if hi_val < 0 && rem != 0 {
                        base + 1
                    } else {
                        base
                    }
                }
                ApQuantMode::ApRnd => {
                    if rem >= half {
                        base + 1
                    } else {
                        base
                    }
                }
                ApQuantMode::ApRndZero => {
                    if hi_val >= 0 {
                        if rem > half {
                            base + 1
                        } else {
                            base
                        }
                    } else if rem >= half {
                        base + 1
                    } else {
                        base
                    }
                }
                ApQuantMode::ApRndInf => {
                    if hi_val >= 0 {
                        if rem >= half {
                            base + 1
                        } else {
                            base
                        }
                    } else if rem > half {
                        base + 1
                    } else {
                        base
                    }
                }
                ApQuantMode::ApRndConv => {
                    if rem > half || (rem == half && (base & 1) != 0) {
                        base + 1
                    } else {
                        base
                    }
                }
            }
        };

        let raw_scaled = Self::apply_overflow(q_int, w, signed, overflow);
        Self {
            width: w,
            int_bits,
            signed,
            quant,
            overflow,
            raw_scaled,
        }
    }

    pub fn apply_overflow(val: i128, width: usize, signed: bool, mode: ApOverflowMode) -> i128 {
        let w = width.clamp(1, 64);
        let (min_v, max_v) = if signed {
            let min_s = -(1i128 << (w - 1));
            let max_s = (1i128 << (w - 1)) - 1;
            if mode == ApOverflowMode::ApSatSym {
                (-max_s, max_s)
            } else {
                (min_s, max_s)
            }
        } else {
            (0i128, (1i128 << w) - 1)
        };

        if val >= min_v && val <= max_v {
            return val;
        }

        match mode {
            ApOverflowMode::ApSat | ApOverflowMode::ApSatSym => val.clamp(min_v, max_v),
            ApOverflowMode::ApSatZero => 0,
            ApOverflowMode::ApWrap => {
                let mask = (1u128 << w) - 1;
                let wrapped = (val as u128) & mask;
                if signed {
                    let sign_bit = 1u128 << (w - 1);
                    if (wrapped & sign_bit) != 0 {
                        (wrapped | !mask) as i128
                    } else {
                        wrapped as i128
                    }
                } else {
                    wrapped as i128
                }
            }
        }
    }

    pub fn to_f64(&self) -> f64 {
        let scale = (2.0f64).powi(self.frac_bits() as i32);
        (self.raw_scaled as f64) / scale
    }

    pub fn range(&self, hi: usize, lo: usize) -> ApInt {
        let ap = ApInt::new(self.width, self.signed, self.raw_scaled);
        ap.range(hi, lo)
    }

    /// Add two fixed-point values and quantize/saturate into the target format of `self`.
    pub fn add_quantized(&self, other: &ApFixed) -> ApFixed {
        let f_self = self.frac_bits();
        let f_other = other.frac_bits();
        if f_self >= f_other {
            let shift = (f_self - f_other) as usize;
            let sum = self.raw_scaled + (other.raw_scaled << shift);
            Self::from_scaled_int(
                sum,
                0,
                self.width,
                self.int_bits,
                self.signed,
                self.quant,
                self.overflow,
            )
        } else {
            let shift = (f_other - f_self) as usize;
            let sum = (self.raw_scaled << shift) + other.raw_scaled;
            Self::from_scaled_int(
                sum,
                shift,
                self.width,
                self.int_bits,
                self.signed,
                self.quant,
                self.overflow,
            )
        }
    }

    /// Multiply two fixed-point values and quantize/saturate into the target format of `self`.
    pub fn mul_quantized(&self, other: &ApFixed) -> ApFixed {
        let prod = self.raw_scaled * other.raw_scaled;
        let extra_frac = other.frac_bits().max(0) as usize;
        Self::from_scaled_int(
            prod,
            extra_frac,
            self.width,
            self.int_bits,
            self.signed,
            self.quant,
            self.overflow,
        )
    }
}

/// Vivado HLS `hls::stream<T>` blocking and non-blocking hardware FIFO channel model.
#[derive(Clone, Debug)]
pub struct HlsStream<T> {
    pub name: String,
    pub depth: usize,
    queue: VecDeque<T>,
}

impl<T> HlsStream<T> {
    pub fn new(name: impl Into<String>, depth: usize) -> Self {
        let d = depth.max(1);
        Self {
            name: name.into(),
            depth: d,
            queue: VecDeque::with_capacity(d),
        }
    }

    pub fn empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn full(&self) -> bool {
        self.queue.len() >= self.depth
    }

    pub fn size(&self) -> usize {
        self.queue.len()
    }

    /// Blocking write (`s.write(val)` or `s << val`). Returns `Err` if bounded FIFO is full.
    pub fn write(&mut self, val: T) -> Result<(), String> {
        if self.full() {
            return Err(format!(
                "hls::stream '{}' overflow (depth={})",
                self.name, self.depth
            ));
        }
        self.queue.push_back(val);
        Ok(())
    }

    /// Non-blocking write (`s.write_nb(val)`): returns `true` if written, `false` if full.
    pub fn write_nb(&mut self, val: T) -> bool {
        if self.full() {
            false
        } else {
            self.queue.push_back(val);
            true
        }
    }

    /// Blocking read (`s.read()` or `s >> dst`). Returns `Err` if FIFO is empty.
    pub fn read(&mut self) -> Result<T, String> {
        self.queue
            .pop_front()
            .ok_or_else(|| format!("hls::stream '{}' underflow (read on empty stream)", self.name))
    }

    /// Non-blocking read (`s.read_nb(dst)`): returns `Some(val)` if non-empty, `None` if empty.
    pub fn read_nb(&mut self) -> Option<T> {
        self.queue.pop_front()
    }
}

// ============================================================================
// Synthesizable `hls::math` Intrinsics (`sqrt`, `abs`, `min`, `max`, `clz`,
// `popcount`, and CORDIC fixed-point `sin` / `cos`)
// ============================================================================

/// Digit-by-digit / restoring integer square root (`hls::sqrt(x)` = $\lfloor \sqrt{x} \rfloor$).
pub fn hls_sqrt_u64(x: u64) -> u64 {
    if x <= 1 {
        return x;
    }
    let mut rem = 0u64;
    let mut root = 0u64;
    for i in (0..32).rev() {
        rem = (rem << 2) | ((x >> (i * 2)) & 0b11);
        let cand = (root << 2) | 1;
        root <<= 1;
        if rem >= cand {
            rem -= cand;
            root |= 1;
        }
    }
    root
}

/// Synthesizable `hls::abs(x)`.
pub fn hls_abs_i64(x: i64) -> i64 {
    x.wrapping_abs()
}

/// Synthesizable `hls::min(a, b)`.
pub fn hls_min_i64(a: i64, b: i64) -> i64 {
    a.min(b)
}

/// Synthesizable `hls::max(a, b)`.
pub fn hls_max_i64(a: i64, b: i64) -> i64 {
    a.max(b)
}

/// Synthesizable `hls::clz(x)` (Count Leading Zeros within a `width`-bit integer).
pub fn hls_clz_u64(x: u64, width: usize) -> u32 {
    let w = width.clamp(1, 64);
    let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
    let masked = x & mask;
    if masked == 0 {
        return w as u32;
    }
    let lz64 = masked.leading_zeros();
    lz64.saturating_sub((64 - w) as u32)
}

/// Synthesizable `hls::popcount(x)` (Hamming weight within a `width`-bit integer).
pub fn hls_popcount_u64(x: u64, width: usize) -> u32 {
    let w = width.clamp(1, 64);
    let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
    (x & mask).count_ones()
}

/// 14-entry CORDIC elementary angles $\arctan(2^{-i})$ in Q2.14 radians ($1.0\text{ rad} = 16384$).
const CORDIC_ATAN_Q14: [i32; 14] = [
    12868, 7596, 4014, 2037, 1023, 512, 256, 128, 64, 32, 16, 8, 4, 2,
];

/// CORDIC inverse gain $K \approx 0.607252935 \times 16384 = 9949$ in Q2.14.
const CORDIC_K_Q14: i32 = 9949;
const HALF_PI_Q14: i32 = 25736; // (pi / 2) * 16384
const PI_Q14: i32 = 51472; // pi * 16384
const TWO_PI_Q14: i32 = 102944; // 2 * pi * 16384

/// Synthesizable 14-stage iterative CORDIC `sin` and `cos` in Q2.14 fixed-point
/// (`16384` = $1.0$). Returns `(sin_q14, cos_q14)`.
pub fn hls_cordic_sincos_q14(angle_q14: i32) -> (i32, i32) {
    // Wrap angle into [-PI, +PI]
    let mut z = angle_q14 % TWO_PI_Q14;
    if z > PI_Q14 {
        z -= TWO_PI_Q14;
    } else if z < -PI_Q14 {
        z += TWO_PI_Q14;
    }

    // Quadrant folding into [-PI/2, +PI/2]
    let mut sign_cos = 1i32;
    if z > HALF_PI_Q14 {
        z = PI_Q14 - z;
        sign_cos = -1;
    } else if z < -HALF_PI_Q14 {
        z = -PI_Q14 - z;
        sign_cos = -1;
    }

    let mut x = CORDIC_K_Q14;
    let mut y = 0i32;
    for (i, &atan_i) in CORDIC_ATAN_Q14.iter().enumerate() {
        let dx = y >> i;
        let dy = x >> i;
        if z >= 0 {
            x -= dx;
            y += dy;
            z -= atan_i;
        } else {
            x += dx;
            y -= dy;
            z += atan_i;
        }
    }
    (y, x * sign_cos)
}

pub fn hls_sin_q14(angle_q14: i32) -> i32 {
    hls_cordic_sincos_q14(angle_q14).0
}

pub fn hls_cos_q14(angle_q14: i32) -> i32 {
    hls_cordic_sincos_q14(angle_q14).1
}
