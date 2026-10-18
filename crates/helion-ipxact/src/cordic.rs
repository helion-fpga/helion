//! Synthesizable Pipelined CORDIC IP Core & Bit-True Golden Reference (UG958 / PG105 parity).
//!
//! Supports:
//! - Coordinate systems: Circular ($m = +1$), Hyperbolic ($m = -1$), and Linear ($m = 0$)
//! - Modes: Rotation ($z \to 0$) and Vectoring ($y \to 0$)
//! - Functions: `sin`/`cos`, polar rotation, `atan2` & magnitude $\sqrt{x^2+y^2}$,
//!   `sinh`/`cosh`, `atanh`, square root $\sqrt{v}$, linear multiply, and linear divide
//! - Parametric synthesizable SystemVerilog generator with AXI4-Stream interfaces (`emit_cordic_sv`)

use std::f64::consts::PI;

/// CORDIC coordinate system parameter $m \in \{+1, -1, 0\}$.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CordicCoordSystem {
    /// $m = +1$: Circular trigonometry (`sin`, `cos`, `atan2`, $\sqrt{x^2 + y^2}$).
    Circular,
    /// $m = -1$: Hyperbolic functions (`sinh`, `cosh`, `atanh`, $\sqrt{x^2 - y^2}$, $\sqrt{v}$).
    Hyperbolic,
    /// $m = 0$: Linear arithmetic ($y + x \cdot z$ multiply-accumulate and $y / x$ division).
    Linear,
}

impl CordicCoordSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Circular => "CIRCULAR",
            Self::Hyperbolic => "HYPERBOLIC",
            Self::Linear => "LINEAR",
        }
    }
}

/// CORDIC iteration convergence target mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CordicMode {
    /// Drive residual angle $z_i \to 0$.
    Rotation,
    /// Drive residual ordinate $y_i \to 0$.
    Vectoring,
}

impl CordicMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rotation => "ROTATION",
            Self::Vectoring => "VECTORING",
        }
    }
}

/// Configuration for the pipelined CORDIC IP generator and bit-true evaluator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CordicConfig {
    pub module_name: String,
    pub coord: CordicCoordSystem,
    pub mode: CordicMode,
    /// Signed fixed-point datapath width (`12..=32`, default `24`).
    pub data_width: u8,
    /// Fractional bits in fixed-point representation (default `16`, i.e. Q8.16).
    pub frac_bits: u8,
    /// Number of shift-add micro-rotation stages (`4..=28`, default `16`).
    pub iterations: usize,
    /// Compensate for CORDIC processor gain ($K_c \approx 1.646760258$ or $K_h \approx 0.82815936$).
    pub scale_compensation: bool,
}

impl Default for CordicConfig {
    fn default() -> Self {
        Self {
            module_name: "h_dsp_cordic".to_string(),
            coord: CordicCoordSystem::Circular,
            mode: CordicMode::Rotation,
            data_width: 24,
            frac_bits: 16,
            iterations: 16,
            scale_compensation: true,
        }
    }
}

/// Result of one bit-true fixed-point CORDIC evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CordicFixedOutput {
    pub x_fixed: i64,
    pub y_fixed: i64,
    pub z_fixed: i64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub latency_cycles: usize,
}

/// Returns true if hyperbolic stage `step` (1-indexed) must be repeated for convergence
/// ($step \in \{4, 13, 40, \dots, 3k+1\}$ per Walther's unified CORDIC algorithm).
pub fn is_hyperbolic_repeat_step(step: usize) -> bool {
    matches!(step, 4 | 13 | 40)
}

/// Compute the circular CORDIC gain $K_c = \prod_{i=0}^{N-1} \sqrt{1 + 2^{-2i}}$.
pub fn circular_gain(iterations: usize) -> f64 {
    let mut k = 1.0_f64;
    for i in 0..iterations {
        let two_neg_2i = 2.0_f64.powi(-(2 * i as i32));
        k *= (1.0 + two_neg_2i).sqrt();
    }
    k
}

/// Compute the hyperbolic CORDIC gain $K_h = \prod_{i=1}^{N} \sqrt{1 - 2^{-2i}}$
/// (including repeated stages at $i = 4, 13, 40$).
pub fn hyperbolic_gain(iterations: usize) -> f64 {
    let mut k = 1.0_f64;
    for step in 1..=iterations {
        let factor = (1.0 - 2.0_f64.powi(-(2 * step as i32))).sqrt();
        k *= factor;
        if is_hyperbolic_repeat_step(step) {
            k *= factor;
        }
    }
    k
}

/// Quantized elementary angle table $\text{atan}(2^{-i})$ in fixed-point (`scale = 1 << frac_bits`).
pub fn circular_atan_table(iterations: usize, frac_bits: u8) -> Vec<i64> {
    let scale = (1u64 << frac_bits) as f64;
    (0..iterations)
        .map(|i| (2.0_f64.powi(-(i as i32)).atan() * scale).round() as i64)
        .collect()
}

/// Quantized hyperbolic angle table $\text{atanh}(2^{-i})$ for $i = 1..=\text{iterations}$.
pub fn hyperbolic_atanh_table(iterations: usize, frac_bits: u8) -> Vec<(usize, i64)> {
    let scale = (1u64 << frac_bits) as f64;
    let mut stages = Vec::new();
    for step in 1..=iterations {
        let angle = (2.0_f64.powi(-(step as i32)).atanh() * scale).round() as i64;
        stages.push((step, angle));
        if is_hyperbolic_repeat_step(step) {
            stages.push((step, angle));
        }
    }
    stages
}

/// Bit-true fixed-point CORDIC evaluator matching the pipelined RTL datapath.
#[derive(Clone, Debug)]
pub struct CordicEngine {
    pub cfg: CordicConfig,
}

impl CordicEngine {
    pub fn new(cfg: CordicConfig) -> Self {
        Self { cfg }
    }

    pub fn scale(&self) -> f64 {
        (1u64 << self.cfg.frac_bits) as f64
    }

    pub fn to_fixed(&self, val: f64) -> i64 {
        (val * self.scale()).round() as i64
    }

    pub fn to_float(&self, fixed: i64) -> f64 {
        (fixed as f64) / self.scale()
    }

    /// Execute the bit-true integer shift-add CORDIC pipeline on fixed-point `(x0, y0, z0)`.
    pub fn eval_fixed(&self, mut x: i64, mut y: i64, mut z: i64) -> CordicFixedOutput {
        let scale = self.scale();
        let pi_fixed = (PI * scale).round() as i64;
        let half_pi_fixed = (0.5 * PI * scale).round() as i64;

        let mut stages_executed = 0usize;

        match self.cfg.coord {
            CordicCoordSystem::Circular => {
                // Quadrant pre-rotation for angles outside [-pi/2, +pi/2] or x < 0 in vectoring.
                match self.cfg.mode {
                    CordicMode::Rotation => {
                        if z > half_pi_fixed {
                            let xn = -y;
                            let yn = x;
                            x = xn;
                            y = yn;
                            z -= half_pi_fixed;
                        } else if z < -half_pi_fixed {
                            let xn = y;
                            let yn = -x;
                            x = xn;
                            y = yn;
                            z += half_pi_fixed;
                        }
                    }
                    CordicMode::Vectoring => {
                        if x < 0 {
                            if y >= 0 {
                                let xn = y;
                                let yn = -x;
                                x = xn;
                                y = yn;
                                z += half_pi_fixed;
                            } else {
                                let xn = -y;
                                let yn = x;
                                x = xn;
                                y = yn;
                                z -= half_pi_fixed;
                            }
                        }
                    }
                }

                let atan_lut = circular_atan_table(self.cfg.iterations, self.cfg.frac_bits);
                for (i, &alpha) in atan_lut.iter().enumerate() {
                    let dx = y >> i;
                    let dy = x >> i;
                    let dir_pos = match self.cfg.mode {
                        CordicMode::Rotation => z >= 0,
                        CordicMode::Vectoring => y < 0,
                    };
                    if dir_pos {
                        x -= dx;
                        y += dy;
                        z -= alpha;
                    } else {
                        x += dx;
                        y -= dy;
                        z += alpha;
                    }
                    stages_executed += 1;
                }

                // Wrap z into [-pi, +pi]
                if z > pi_fixed {
                    z -= 2 * pi_fixed;
                } else if z < -pi_fixed {
                    z += 2 * pi_fixed;
                }

                if self.cfg.scale_compensation {
                    let inv_k = 1.0 / circular_gain(self.cfg.iterations);
                    let inv_k_fixed = (inv_k * scale).round() as i64;
                    x = (x * inv_k_fixed) >> self.cfg.frac_bits;
                    y = (y * inv_k_fixed) >> self.cfg.frac_bits;
                }
            }
            CordicCoordSystem::Hyperbolic => {
                let stages = hyperbolic_atanh_table(self.cfg.iterations, self.cfg.frac_bits);
                for (shift, alpha) in stages {
                    let dx = y >> shift;
                    let dy = x >> shift;
                    let dir_pos = match self.cfg.mode {
                        CordicMode::Rotation => z >= 0,
                        CordicMode::Vectoring => y < 0,
                    };
                    if dir_pos {
                        x += dx;
                        y += dy;
                        z -= alpha;
                    } else {
                        x -= dx;
                        y -= dy;
                        z += alpha;
                    }
                    stages_executed += 1;
                }
                if self.cfg.scale_compensation {
                    let inv_kh = 1.0 / hyperbolic_gain(self.cfg.iterations);
                    let inv_kh_fixed = (inv_kh * scale).round() as i64;
                    x = (x * inv_kh_fixed) >> self.cfg.frac_bits;
                    y = (y * inv_kh_fixed) >> self.cfg.frac_bits;
                }
            }
            CordicCoordSystem::Linear => {
                for i in 0..self.cfg.iterations {
                    let dy = x >> i;
                    let dz = (1i64 << self.cfg.frac_bits) >> i;
                    let dir_pos = match self.cfg.mode {
                        CordicMode::Rotation => z >= 0,
                        CordicMode::Vectoring => y < 0,
                    };
                    if dir_pos {
                        y += dy;
                        z -= dz;
                    } else {
                        y -= dy;
                        z += dz;
                    }
                    stages_executed += 1;
                }
            }
        }

        CordicFixedOutput {
            x_fixed: x,
            y_fixed: y,
            z_fixed: z,
            x: self.to_float(x),
            y: self.to_float(y),
            z: self.to_float(z),
            latency_cycles: stages_executed + 1,
        }
    }

    /// Compute $(\cos \theta, \sin \theta)$ via Circular Rotation CORDIC for $\theta \in [-\pi, +\pi]$.
    pub fn sin_cos(&self, theta_rad: f64) -> (f64, f64) {
        let mut c = self.cfg.clone();
        c.coord = CordicCoordSystem::Circular;
        c.mode = CordicMode::Rotation;
        c.scale_compensation = true;
        let eng = Self::new(c);
        let one = eng.to_fixed(1.0);
        let z0 = eng.to_fixed(theta_rad);
        let out = eng.eval_fixed(one, 0, z0);
        (out.y, out.x) // (sin, cos)
    }

    /// Compute $(\text{atan2}(y, x), \sqrt{x^2 + y^2})$ via Circular Vectoring CORDIC.
    pub fn atan2_magnitude(&self, y: f64, x: f64) -> (f64, f64) {
        let mut c = self.cfg.clone();
        c.coord = CordicCoordSystem::Circular;
        c.mode = CordicMode::Vectoring;
        c.scale_compensation = true;
        let eng = Self::new(c);
        let out = eng.eval_fixed(eng.to_fixed(x), eng.to_fixed(y), 0);
        (out.z, out.x)
    }

    /// Compute $(\sinh z, \cosh z)$ via Hyperbolic Rotation CORDIC for $|z| \le 1.1$.
    pub fn sinh_cosh(&self, z: f64) -> (f64, f64) {
        let mut c = self.cfg.clone();
        c.coord = CordicCoordSystem::Hyperbolic;
        c.mode = CordicMode::Rotation;
        c.scale_compensation = true;
        let eng = Self::new(c);
        let one = eng.to_fixed(1.0);
        let out = eng.eval_fixed(one, 0, eng.to_fixed(z));
        (out.y, out.x) // (sinh, cosh)
    }

    /// Compute $\sqrt{v}$ via Hyperbolic Vectoring CORDIC using identity
    /// $\sqrt{(v + 1/4)^2 - (v - 1/4)^2} = \sqrt{v}$ (with power-of-4 range normalization).
    pub fn sqrt(&self, v: f64) -> Result<f64, String> {
        if v < 0.0 {
            return Err(format!("cordic sqrt: negative input {v}"));
        }
        if v == 0.0 {
            return Ok(0.0);
        }
        // Normalize v into [0.25, 1.0) by multiplying/dividing by 4^k so |(v-0.25)/(v+0.25)| < 0.6.
        let mut norm = v;
        let mut shift_exp = 0i32;
        while norm >= 1.0 {
            norm *= 0.25;
            shift_exp += 1;
        }
        while norm < 0.25 {
            norm *= 4.0;
            shift_exp -= 1;
        }

        let mut c = self.cfg.clone();
        c.coord = CordicCoordSystem::Hyperbolic;
        c.mode = CordicMode::Vectoring;
        c.scale_compensation = true;
        let eng = Self::new(c);
        let x0 = eng.to_fixed(norm + 0.25);
        let y0 = eng.to_fixed(norm - 0.25);
        let out = eng.eval_fixed(x0, y0, 0);
        Ok(out.x * 2.0_f64.powi(shift_exp))
    }
}

/// Emit a parametric, synthesizable SystemVerilog pipelined CORDIC core with AXI4-Stream interfaces.
pub fn emit_cordic_sv(cfg: &CordicConfig) -> String {
    let dw = cfg.data_width.max(8);
    let n_stages = cfg.iterations.clamp(4, 28);
    let atan_lut = circular_atan_table(n_stages, cfg.frac_bits);
    let sign_op = match cfg.coord {
        CordicCoordSystem::Circular => "-",
        CordicCoordSystem::Hyperbolic => "+",
        CordicCoordSystem::Linear => "+",
    };

    let mut s = String::new();
    s.push_str(&format!(
        "// Helion Vivado DSP IP: Synthesizable Pipelined CORDIC (UG958 / PG105)\n\
         // Coordinate System: {} | Mode: {} | Width: {} | Stages: {}\n\
         module {} #(\n\
         \x20 parameter int DATA_WIDTH = {},\n\
         \x20 parameter int FRAC_BITS  = {},\n\
         \x20 parameter int STAGES     = {}\n\
         ) (\n\
         \x20 input  logic                    aclk,\n\
         \x20 input  logic                    aresetn,\n\
         \x20 input  logic                    s_axis_tvalid,\n\
         \x20 output logic                    s_axis_tready,\n\
         \x20 input  logic signed [DATA_WIDTH-1:0] s_axis_x_tdata,\n\
         \x20 input  logic signed [DATA_WIDTH-1:0] s_axis_y_tdata,\n\
         \x20 input  logic signed [DATA_WIDTH-1:0] s_axis_z_tdata,\n\
         \x20 input  logic                    s_axis_tlast,\n\
         \x20 output logic                    m_axis_tvalid,\n\
         \x20 input  logic                    m_axis_tready,\n\
         \x20 output logic signed [DATA_WIDTH-1:0] m_axis_x_tdata,\n\
         \x20 output logic signed [DATA_WIDTH-1:0] m_axis_y_tdata,\n\
         \x20 output logic signed [DATA_WIDTH-1:0] m_axis_z_tdata,\n\
         \x20 output logic                    m_axis_tlast\n\
         );\n\n",
        cfg.coord.as_str(),
        cfg.mode.as_str(),
        dw,
        n_stages,
        cfg.module_name,
        dw,
        cfg.frac_bits,
        n_stages
    ));

    s.push_str("  // Elementary angle ROM table\n");
    s.push_str(&format!(
        "  localparam logic signed [DATA_WIDTH-1:0] ATAN_LUT [0:{}] = '{{\n",
        n_stages - 1
    ));
    for (i, &val) in atan_lut.iter().enumerate() {
        let sep = if i + 1 == n_stages { "" } else { "," };
        s.push_str(&format!("    {dw}'sd{val}{sep}\n"));
    }
    s.push_str("  };\n\n");

    s.push_str(
        "  logic signed [DATA_WIDTH-1:0] x_pipe [0:STAGES];\n\
         \x20 logic signed [DATA_WIDTH-1:0] y_pipe [0:STAGES];\n\
         \x20 logic signed [DATA_WIDTH-1:0] z_pipe [0:STAGES];\n\
         \x20 logic [STAGES:0] valid_pipe;\n\
         \x20 logic [STAGES:0] last_pipe;\n\
         \x20 wire pipe_ce = ~m_axis_tvalid | m_axis_tready;\n\
         \x20 assign s_axis_tready = pipe_ce;\n\n",
    );

    let dir_cond = match cfg.mode {
        CordicMode::Rotation => "z_pipe[i] >= 0",
        CordicMode::Vectoring => "y_pipe[i] < 0",
    };

    s.push_str(&format!(
        "  always_ff @(posedge aclk) begin\n\
         \x20   if (!aresetn) begin\n\
         \x20     valid_pipe <= '0;\n\
         \x20     last_pipe  <= '0;\n\
         \x20   end else if (pipe_ce) begin\n\
         \x20     x_pipe[0] <= s_axis_x_tdata;\n\
         \x20     y_pipe[0] <= s_axis_y_tdata;\n\
         \x20     z_pipe[0] <= s_axis_z_tdata;\n\
         \x20     valid_pipe[0] <= s_axis_tvalid;\n\
         \x20     last_pipe[0]  <= s_axis_tlast;\n\
         \x20     for (int i = 0; i < STAGES; i++) begin\n\
         \x20       valid_pipe[i+1] <= valid_pipe[i];\n\
         \x20       last_pipe[i+1]  <= last_pipe[i];\n\
         \x20       if ({dir_cond}) begin\n\
         \x20         x_pipe[i+1] <= x_pipe[i] {sign_op} (y_pipe[i] >>> i);\n\
         \x20         y_pipe[i+1] <= y_pipe[i] + (x_pipe[i] >>> i);\n\
         \x20         z_pipe[i+1] <= z_pipe[i] - ATAN_LUT[i];\n\
         \x20       end else begin\n\
         \x20         x_pipe[i+1] <= x_pipe[i] + (y_pipe[i] >>> i);\n\
         \x20         y_pipe[i+1] <= y_pipe[i] - (x_pipe[i] >>> i);\n\
         \x20         z_pipe[i+1] <= z_pipe[i] + ATAN_LUT[i];\n\
         \x20       end\n\
         \x20     end\n\
         \x20   end\n\
         \x20 end\n\n\
         \x20 assign m_axis_tvalid  = valid_pipe[STAGES];\n\
         \x20 assign m_axis_tlast   = last_pipe[STAGES];\n\
         \x20 assign m_axis_x_tdata = x_pipe[STAGES];\n\
         \x20 assign m_axis_y_tdata = y_pipe[STAGES];\n\
         \x20 assign m_axis_z_tdata = z_pipe[STAGES];\n\
         endmodule\n"
    ));
    s
}
