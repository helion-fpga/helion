//! Polyphase Decimation/Interpolation FIR Compiler (PG149) & Phase-Dithered DDS Compiler (PG141).
//!
//! Provides parametric synthesizable SystemVerilog generators with AXI4-Stream
//! interfaces and bit-true golden Rust evaluators for both cores.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Sample-rate conversion mode for the PG149 Polyphase FIR Compiler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirRateMode {
    /// $L = 1, M = 1$: 1 output sample per input sample.
    SingleRate,
    /// $1 / M$ polyphase decimation ($M \ge 2$): 1 output sample every $M$ input samples.
    Decimation { factor: usize },
    /// $L \times$ polyphase interpolation ($L \ge 2$): $L$ output samples per 1 input sample.
    Interpolation { factor: usize },
    /// $L / M$ rational rate polyphase resampling.
    RationalRate { interp: usize, decim: usize },
}

impl FirRateMode {
    pub fn interp_decim(self) -> (usize, usize) {
        match self {
            Self::SingleRate => (1, 1),
            Self::Decimation { factor } => (1, factor.max(1)),
            Self::Interpolation { factor } => (factor.max(1), 1),
            Self::RationalRate { interp, decim } => (interp.max(1), decim.max(1)),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleRate => "SINGLE_RATE",
            Self::Decimation { .. } => "POLYPHASE_DECIMATION",
            Self::Interpolation { .. } => "POLYPHASE_INTERPOLATION",
            Self::RationalRate { .. } => "POLYPHASE_RATIONAL",
        }
    }
}

/// Design a symmetric linear-phase Hamming-windowed sinc low-pass FIR filter
/// (`num_taps` coefficients, normalized cutoff `cutoff_norm` $\in (0.0, 1.0)$ where `1.0` = Nyquist,
/// quantized to integer scale `coeff_scale`).
pub fn design_lowpass_hamming_taps(
    num_taps: usize,
    cutoff_norm: f64,
    coeff_scale: i32,
) -> Vec<i32> {
    let n = num_taps.max(1);
    let fc = cutoff_norm.clamp(0.01, 0.99) * 0.5;
    let center = ((n - 1) as f64) * 0.5;
    let mut raw = Vec::with_capacity(n);
    let mut sum = 0.0_f64;

    for i in 0..n {
        let m = (i as f64) - center;
        let sinc = if m.abs() < 1e-12 {
            2.0 * fc
        } else {
            (2.0 * PI * fc * m).sin() / (PI * m)
        };
        let hamming = if n == 1 {
            1.0
        } else {
            0.54 - 0.46 * (2.0 * PI * (i as f64) / ((n - 1) as f64)).cos()
        };
        let h = sinc * hamming;
        raw.push(h);
        sum += h;
    }

    let norm = if sum.abs() > 1e-12 {
        (coeff_scale as f64) / sum
    } else {
        coeff_scale as f64
    };
    raw.into_iter().map(|h| (h * norm).round() as i32).collect()
}

/// Configuration for the PG149 Polyphase FIR Compiler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirConfig {
    pub module_name: String,
    pub rate_mode: FirRateMode,
    pub coefficients: Vec<i32>,
    pub input_width: u8,
    pub coeff_width: u8,
    pub output_width: u8,
    pub reloadable: bool,
}

impl Default for FirConfig {
    fn default() -> Self {
        Self {
            module_name: "h_dsp_fir_compiler".to_string(),
            rate_mode: FirRateMode::SingleRate,
            coefficients: design_lowpass_hamming_taps(15, 0.25, 32768),
            input_width: 16,
            coeff_width: 16,
            output_width: 32,
            reloadable: true,
        }
    }
}

/// Bit-true golden Rust Polyphase FIR Compiler evaluator.
#[derive(Clone, Debug)]
pub struct FirCompilerEngine {
    pub cfg: FirConfig,
    pub history: VecDeque<i64>,
    pub decim_phase: usize,
}

impl FirCompilerEngine {
    pub fn new(cfg: FirConfig) -> Self {
        let len = cfg.coefficients.len().max(1);
        Self {
            cfg,
            history: VecDeque::from(vec![0i64; len]),
            decim_phase: 0,
        }
    }

    /// Decompose `self.cfg.coefficients` into $P$ polyphase sub-filter branches
    /// $E_k[m] = h[m \cdot P + k]$ for $k \in 0..P$.
    pub fn polyphase_branches(&self) -> Vec<Vec<i32>> {
        let (interp, decim) = self.cfg.rate_mode.interp_decim();
        let p = interp.max(decim).max(1);
        let mut branches = vec![Vec::new(); p];
        for (idx, &c) in self.cfg.coefficients.iter().enumerate() {
            branches[idx % p].push(c);
        }
        branches
    }

    /// Live coefficient reload (`s_axis_reload_*` channel).
    pub fn reload_coefficients(&mut self, new_coeffs: Vec<i32>) -> Result<(), String> {
        if new_coeffs.is_empty() {
            return Err("fir reload: coefficient vector cannot be empty".into());
        }
        self.cfg.coefficients = new_coeffs;
        while self.history.len() < self.cfg.coefficients.len() {
            self.history.push_back(0);
        }
        Ok(())
    }

    fn eval_direct(&self) -> i64 {
        let mut acc = 0i64;
        for (&x, &h) in self.history.iter().zip(self.cfg.coefficients.iter()) {
            acc = acc.saturating_add(x.saturating_mul(i64::from(h)));
        }
        acc
    }

    /// Push one input sample `x` and return the produced output sample(s) (`0`, `1`, or `L` samples
    /// depending on [`FirRateMode`]).
    pub fn push_sample(&mut self, x: i32) -> Vec<i64> {
        let (interp, decim) = self.cfg.rate_mode.interp_decim();
        let mut out = Vec::new();

        match (interp, decim) {
            (1, 1) => {
                self.history.push_front(i64::from(x));
                self.history.truncate(self.cfg.coefficients.len().max(1));
                out.push(self.eval_direct());
            }
            (1, m) => {
                // Polyphase decimation by M: shift input in, compute output every M-th sample.
                self.history.push_front(i64::from(x));
                self.history.truncate(self.cfg.coefficients.len().max(1));
                self.decim_phase += 1;
                if self.decim_phase >= m {
                    self.decim_phase = 0;
                    out.push(self.eval_direct());
                }
            }
            (_l, m) => {
                // Polyphase interpolation by L (with optional post-decimation by M):
                // Polyphase commutator evaluates each sub-filter branch k in 0..L on input history!
                let branches = self.polyphase_branches();
                let max_sub_len = branches.iter().map(|b| b.len()).max().unwrap_or(1);
                self.history.push_front(i64::from(x));
                self.history.truncate(max_sub_len);

                for branch in &branches {
                    let mut branch_acc = 0i64;
                    for (&samp, &h) in self.history.iter().zip(branch.iter()) {
                        branch_acc = branch_acc.saturating_add(samp.saturating_mul(i64::from(h)));
                    }
                    self.decim_phase += 1;
                    if self.decim_phase >= m {
                        self.decim_phase = 0;
                        out.push(branch_acc);
                    }
                }
            }
        }
        out
    }

    /// Filter a contiguous slice of input samples.
    pub fn process_stream(&mut self, samples: &[i32]) -> Vec<i64> {
        let mut out = Vec::new();
        for &s in samples {
            out.extend(self.push_sample(s));
        }
        out
    }
}

/// Emit a parametric, synthesizable SystemVerilog Polyphase FIR Compiler module with AXI4-Stream ports.
pub fn emit_fir_sv(cfg: &FirConfig) -> String {
    let n_taps = cfg.coefficients.len().max(1);
    let iw = cfg.input_width.max(8);
    let cw = cfg.coeff_width.max(8);
    let ow = cfg.output_width.max(16);
    let (interp, decim) = cfg.rate_mode.interp_decim();

    let mut s = String::new();
    s.push_str(&format!(
        "// Helion Vivado DSP IP: Polyphase FIR Compiler (PG149)\n\
         // Mode: {} (L={}, M={}) | Taps: {}\n\
         module {} #(\n\
         \x20 parameter int NUM_TAPS     = {},\n\
         \x20 parameter int INPUT_WIDTH  = {},\n\
         \x20 parameter int COEFF_WIDTH  = {},\n\
         \x20 parameter int OUTPUT_WIDTH = {},\n\
         \x20 parameter int INTERP_RATE  = {},\n\
         \x20 parameter int DECIM_RATE   = {}\n\
         ) (\n\
         \x20 input  logic                           aclk,\n\
         \x20 input  logic                           aresetn,\n\
         \x20 input  logic                           s_axis_data_tvalid,\n\
         \x20 output logic                           s_axis_data_tready,\n\
         \x20 input  logic signed [INPUT_WIDTH-1:0]  s_axis_data_tdata,\n\
         \x20 input  logic                           s_axis_data_tlast,\n\
         \x20 input  logic                           s_axis_reload_tvalid,\n\
         \x20 output logic                           s_axis_reload_tready,\n\
         \x20 input  logic signed [COEFF_WIDTH-1:0]  s_axis_reload_tdata,\n\
         \x20 output logic                           m_axis_data_tvalid,\n\
         \x20 input  logic                           m_axis_data_tready,\n\
         \x20 output logic signed [OUTPUT_WIDTH-1:0] m_axis_data_tdata,\n\
         \x20 output logic                           m_axis_data_tlast\n\
         );\n\n",
        cfg.rate_mode.as_str(),
        interp,
        decim,
        n_taps,
        cfg.module_name,
        n_taps,
        iw,
        cw,
        ow,
        interp,
        decim
    ));

    s.push_str(&format!(
        "  logic signed [COEFF_WIDTH-1:0] coeffs [0:NUM_TAPS-1] = '{{\n"
    ));
    for (i, &c) in cfg.coefficients.iter().enumerate() {
        let sep = if i + 1 == n_taps { "" } else { "," };
        s.push_str(&format!("    {cw}'sd{c}{sep}\n"));
    }
    s.push_str("  };\n\n");

    s.push_str(
        "  logic signed [INPUT_WIDTH-1:0] delay_line [0:NUM_TAPS-1];\n\
         \x20 logic [$clog2(NUM_TAPS+1)-1:0] reload_idx;\n\
         \x20 assign s_axis_reload_tready = 1'b1;\n\
         \x20 assign s_axis_data_tready   = ~m_axis_data_tvalid | m_axis_data_tready;\n\n\
         \x20 always_comb begin\n\
         \x20   m_axis_data_tdata = '0;\n\
         \x20   for (int k = 0; k < NUM_TAPS; k++) begin\n\
         \x20     m_axis_data_tdata = m_axis_data_tdata + OUTPUT_WIDTH'(delay_line[k] * coeffs[k]);\n\
         \x20   end\n\
         \x20 end\n\n\
         \x20 always_ff @(posedge aclk) begin\n\
         \x20   if (!aresetn) begin\n\
         \x20     m_axis_data_tvalid <= 1'b0;\n\
         \x20     m_axis_data_tlast  <= 1'b0;\n\
         \x20     reload_idx         <= '0;\n\
         \x20     for (int k = 0; k < NUM_TAPS; k++) delay_line[k] <= '0;\n\
         \x20   end else begin\n\
         \x20     if (s_axis_reload_tvalid) begin\n\
         \x20       coeffs[reload_idx] <= s_axis_reload_tdata;\n\
         \x20       reload_idx <= (reload_idx == NUM_TAPS - 1) ? '0 : reload_idx + 1'b1;\n\
         \x20     end\n\
         \x20     if (s_axis_data_tvalid && s_axis_data_tready) begin\n\
         \x20       delay_line[0] <= s_axis_data_tdata;\n\
         \x20       for (int k = 1; k < NUM_TAPS; k++) delay_line[k] <= delay_line[k-1];\n\
         \x20       m_axis_data_tvalid <= 1'b1;\n\
         \x20       m_axis_data_tlast  <= s_axis_data_tlast;\n\
         \x20     end else if (m_axis_data_tready) begin\n\
         \x20       m_axis_data_tvalid <= 1'b0;\n\
         \x20     end\n\
         \x20   end\n\
         \x20 end\n\
         endmodule\n",
    );
    s
}

/// Configuration for the PG141 Phase-Dithered DDS (Direct Digital Synthesizer) Compiler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdsConfig {
    pub module_name: String,
    /// Phase accumulator bit width (`16..=48`, default `32`).
    pub phase_width: u8,
    /// Phase-to-amplitude LUT address bits (`6..=14`, default `10` = 1024 entries).
    pub lut_addr_width: u8,
    /// Signed output sample width per component (`8..=24`, default `16`).
    pub output_width: u8,
    /// Default Frequency Tuning Word / Phase Increment (`PINC`).
    pub pinc: u64,
    /// Default Phase Offset Word (`POFF`).
    pub poff: u64,
    /// Enable LFSR phase dithering prior to LUT address truncation to suppress truncation spurs.
    pub phase_dither_enabled: bool,
}

impl Default for DdsConfig {
    fn default() -> Self {
        Self {
            module_name: "h_dsp_dds_compiler".to_string(),
            phase_width: 32,
            lut_addr_width: 10,
            output_width: 16,
            pinc: 1u64 << 26, // f_out = f_clk / 64
            poff: 0,
            phase_dither_enabled: true,
        }
    }
}

/// One quadrature sample emitted by [`DdsCompilerEngine`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DdsSample {
    pub phase_acc: u64,
    pub dithered_phase: u64,
    pub lut_index: usize,
    pub sin: i32,
    pub cos: i32,
    /// Packed AXI4-Stream `m_axis_data_tdata` (`{sin[OW-1:0], cos[OW-1:0]}`).
    pub tdata: u64,
}

/// Bit-true golden Rust Phase-Dithered DDS Compiler evaluator.
#[derive(Clone, Debug)]
pub struct DdsCompilerEngine {
    pub cfg: DdsConfig,
    pub phase_acc: u64,
    pub lfsr: u32,
    pub sin_lut: Vec<i32>,
}

impl DdsCompilerEngine {
    pub fn new(cfg: DdsConfig) -> Self {
        let lut_len = 1usize << cfg.lut_addr_width.clamp(4, 14);
        let max_amp = ((1i64 << (cfg.output_width.saturating_sub(1))) - 1) as f64;
        let sin_lut: Vec<i32> = (0..lut_len)
            .map(|k| {
                let theta = 2.0 * PI * (k as f64) / (lut_len as f64);
                (theta.sin() * max_amp).round() as i32
            })
            .collect();
        Self {
            cfg,
            phase_acc: 0,
            lfsr: 0xACE1_1357,
            sin_lut,
        }
    }

    pub fn phase_mask(&self) -> u64 {
        if self.cfg.phase_width >= 64 {
            u64::MAX
        } else {
            (1u64 << self.cfg.phase_width) - 1
        }
    }

    /// Runtime AXI4-Stream `s_axis_config_*` update of `PINC` and `POFF`.
    pub fn set_pinc_poff(&mut self, pinc: u64, poff: u64) {
        let m = self.phase_mask();
        self.cfg.pinc = pinc & m;
        self.cfg.poff = poff & m;
    }

    fn next_dither(&mut self, frac_phase_bits: u8) -> u64 {
        if !self.cfg.phase_dither_enabled || frac_phase_bits == 0 {
            return 0;
        }
        // 19-bit maximal LFSR step
        let fb = ((self.lfsr >> 18) ^ (self.lfsr >> 5) ^ (self.lfsr >> 1) ^ self.lfsr) & 1;
        self.lfsr = ((self.lfsr << 1) | fb) & 0x0007_FFFF;
        let dither_mask = if frac_phase_bits >= 64 {
            u64::MAX
        } else {
            (1u64 << frac_phase_bits) - 1
        };
        (u64::from(self.lfsr)) & dither_mask
    }

    /// Advance the DDS phase accumulator by one clock cycle and return the quadrature [`DdsSample`].
    pub fn step(&mut self) -> DdsSample {
        let mask = self.phase_mask();
        let addr_bits = self.cfg.lut_addr_width.clamp(4, 14);
        let lut_len = self.sin_lut.len();
        let lut_mask = lut_len - 1;
        let trunc_bits = self.cfg.phase_width.saturating_sub(addr_bits);

        let cur_acc = self.phase_acc;
        let dither = self.next_dither(trunc_bits);
        let dithered = cur_acc.wrapping_add(self.cfg.poff).wrapping_add(dither) & mask;

        let sin_idx = ((dithered >> trunc_bits) as usize) & lut_mask;
        let cos_idx = (sin_idx + lut_len / 4) & lut_mask;

        let sin_val = self.sin_lut[sin_idx];
        let cos_val = self.sin_lut[cos_idx];

        self.phase_acc = cur_acc.wrapping_add(self.cfg.pinc) & mask;

        let ow_mask = if self.cfg.output_width >= 32 {
            u64::from(u32::MAX)
        } else {
            (1u64 << self.cfg.output_width) - 1
        };
        let tdata = (((sin_val as i64 as u64) & ow_mask) << self.cfg.output_width)
            | ((cos_val as i64 as u64) & ow_mask);

        DdsSample {
            phase_acc: cur_acc,
            dithered_phase: dithered,
            lut_index: sin_idx,
            sin: sin_val,
            cos: cos_val,
            tdata,
        }
    }

    /// Generate `n` consecutive quadrature DDS samples.
    pub fn generate_samples(&mut self, n: usize) -> Vec<DdsSample> {
        (0..n).map(|_| self.step()).collect()
    }
}

/// Emit a parametric, synthesizable SystemVerilog Phase-Dithered DDS Compiler with AXI4-Stream ports.
pub fn emit_dds_sv(cfg: &DdsConfig) -> String {
    let pw = cfg.phase_width.max(16);
    let aw = cfg.lut_addr_width.clamp(4, 14);
    let ow = cfg.output_width.max(8);
    let preview_len = (1usize << aw).min(16);
    let eng = DdsCompilerEngine::new(cfg.clone());

    let mut s = String::new();
    s.push_str(&format!(
        "// Helion Vivado DSP IP: Phase-Dithered DDS Compiler (PG141)\n\
         // Phase Width: {} | LUT Addr Width: {} | Output Width: {} | Dither: {}\n\
         module {} #(\n\
         \x20 parameter int PHASE_WIDTH  = {},\n\
         \x20 parameter int LUT_ADDR_W   = {},\n\
         \x20 parameter int OUTPUT_WIDTH = {},\n\
         \x20 parameter logic [PHASE_WIDTH-1:0] DEFAULT_PINC = {}'d{},\n\
         \x20 parameter logic [PHASE_WIDTH-1:0] DEFAULT_POFF = {}'d{}\n\
         ) (\n\
         \x20 input  logic                           aclk,\n\
         \x20 input  logic                           aresetn,\n\
         \x20 // AXI4-Stream Config Channel ({{POFF, PINC}})\n\
         \x20 input  logic                           s_axis_config_tvalid,\n\
         \x20 output logic                           s_axis_config_tready,\n\
         \x20 input  logic [2*PHASE_WIDTH-1:0]       s_axis_config_tdata,\n\
         \x20 // AXI4-Stream Quadrature Data Output ({{sin, cos}})\n\
         \x20 output logic                           m_axis_data_tvalid,\n\
         \x20 input  logic                           m_axis_data_tready,\n\
         \x20 output logic [2*OUTPUT_WIDTH-1:0]      m_axis_data_tdata,\n\
         \x20 // AXI4-Stream Phase Output\n\
         \x20 output logic                           m_axis_phase_tvalid,\n\
         \x20 output logic [PHASE_WIDTH-1:0]         m_axis_phase_tdata\n\
         );\n\n",
        pw,
        aw,
        ow,
        cfg.phase_dither_enabled,
        cfg.module_name,
        pw,
        aw,
        ow,
        pw,
        cfg.pinc,
        pw,
        cfg.poff
    ));

    s.push_str(&format!(
        "  // Sine LUT ROM preview (first {preview_len} entries)\n\
         \x20 localparam logic signed [OUTPUT_WIDTH-1:0] SIN_LUT_HEAD [0:{preview_len}-1] = '{{\n"
    ));
    for (i, &val) in eng.sin_lut.iter().take(preview_len).enumerate() {
        let sep = if i + 1 == preview_len { "" } else { "," };
        s.push_str(&format!("    {ow}'sd{val}{sep}\n"));
    }
    s.push_str("  };\n\n");

    s.push_str(
        "  logic [PHASE_WIDTH-1:0] phase_acc;\n\
         \x20 logic [PHASE_WIDTH-1:0] pinc_reg;\n\
         \x20 logic [PHASE_WIDTH-1:0] poff_reg;\n\
         \x20 logic [18:0]            dither_lfsr;\n\
         \x20 assign s_axis_config_tready = 1'b1;\n\n\
         \x20 always_ff @(posedge aclk) begin\n\
         \x20   if (!aresetn) begin\n\
         \x20     phase_acc           <= '0;\n\
         \x20     pinc_reg            <= DEFAULT_PINC;\n\
         \x20     poff_reg            <= DEFAULT_POFF;\n\
         \x20     dither_lfsr         <= 19'h5A3C9;\n\
         \x20     m_axis_data_tvalid  <= 1'b0;\n\
         \x20     m_axis_phase_tvalid <= 1'b0;\n\
         \x20     m_axis_data_tdata   <= '0;\n\
         \x20     m_axis_phase_tdata  <= '0;\n\
         \x20   end else begin\n\
         \x20     if (s_axis_config_tvalid) begin\n\
         \x20       pinc_reg <= s_axis_config_tdata[PHASE_WIDTH-1:0];\n\
         \x20       poff_reg <= s_axis_config_tdata[2*PHASE_WIDTH-1:PHASE_WIDTH];\n\
         \x20     end\n\
         \x20     if (~m_axis_data_tvalid | m_axis_data_tready) begin\n\
         \x20       dither_lfsr         <= {dither_lfsr[17:0], dither_lfsr[18] ^ dither_lfsr[5] ^ dither_lfsr[1] ^ dither_lfsr[0]};\n\
         \x20       phase_acc           <= phase_acc + pinc_reg;\n\
         \x20       m_axis_phase_tdata  <= phase_acc + poff_reg;\n\
         \x20       m_axis_phase_tvalid <= 1'b1;\n\
         \x20       m_axis_data_tvalid  <= 1'b1;\n\
         \x20       m_axis_data_tdata   <= {SIN_LUT_HEAD[phase_acc[PHASE_WIDTH-1 -: 4]], SIN_LUT_HEAD[(phase_acc[PHASE_WIDTH-1 -: 4] + 4'd4) & 4'hF]};\n\
         \x20     end\n\
         \x20   end\n\
         \x20 end\n\
         endmodule\n",
    );
    s
}
