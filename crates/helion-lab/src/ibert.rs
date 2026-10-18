//! IBERT High-Speed Transceiver 2D Eye Scan & BER Bathtub Analysis (UG908 parity).
//!
//! Provides:
//! - Standard ITU-T PRBS generators & bit-error checkers (`PRBS-7`, `PRBS-15`, `PRBS-23`, `PRBS-31`)
//! - 1D horizontal phase (`-0.5 UI .. +0.5 UI`) BER bathtub curve sweep (`sweep_bathtub`)
//! - 2D horizontal-phase vs vertical-voltage eye-diagram sweep (`run_ibert_eye_scan`)
//! - Eye opening metrics (UI width at target BER, mV vertical height, eye area, Q-factor)
//!   and ASCII heatmap visualization.

/// ITU-T O.150 PRBS polynomial pattern for IBERT transceiver link testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrbsPattern {
    /// $x^7 + x^6 + 1$ (period $2^7 - 1 = 127$).
    Prbs7,
    /// $x^{15} + x^{14} + 1$ (period $2^{15} - 1 = 32767$).
    Prbs15,
    /// $x^{23} + x^{18} + 1$ (period $2^{23} - 1$).
    Prbs23,
    /// $x^{31} + x^{28} + 1$ (period $2^{31} - 1$).
    Prbs31,
}

impl PrbsPattern {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prbs7 => "PRBS-7",
            Self::Prbs15 => "PRBS-15",
            Self::Prbs23 => "PRBS-23",
            Self::Prbs31 => "PRBS-31",
        }
    }

    pub fn taps(self) -> (u8, u8) {
        match self {
            Self::Prbs7 => (7, 6),
            Self::Prbs15 => (15, 14),
            Self::Prbs23 => (23, 18),
            Self::Prbs31 => (31, 28),
        }
    }

    /// Pattern-dependent ISI penalty factor (longer runs of identical bits increase ISI).
    pub fn isi_factor(self) -> f64 {
        match self {
            Self::Prbs7 => 1.00,
            Self::Prbs15 => 1.08,
            Self::Prbs23 => 1.15,
            Self::Prbs31 => 1.22,
        }
    }
}

/// Linear-feedback shift register (LFSR) PRBS bit generator and error checker.
#[derive(Clone, Debug)]
pub struct PrbsEngine {
    pub pattern: PrbsPattern,
    pub lfsr: u32,
    pub total_bits: u64,
    pub error_bits: u64,
}

impl PrbsEngine {
    pub fn new(pattern: PrbsPattern, seed: u32) -> Self {
        let (hi, _) = pattern.taps();
        let mask = if hi >= 32 {
            u32::MAX
        } else {
            (1u32 << hi) - 1
        };
        let init = if (seed & mask) == 0 { 1 } else { seed & mask };
        Self {
            pattern,
            lfsr: init,
            total_bits: 0,
            error_bits: 0,
        }
    }

    /// Generate the next PRBS bit.
    pub fn next_bit(&mut self) -> bool {
        let (tap_a, tap_b) = self.pattern.taps();
        let bit_a = (self.lfsr >> (tap_a - 1)) & 1;
        let bit_b = (self.lfsr >> (tap_b - 1)) & 1;
        let fb = bit_a ^ bit_b;
        let mask = if tap_a >= 32 {
            u32::MAX
        } else {
            (1u32 << tap_a) - 1
        };
        self.lfsr = ((self.lfsr << 1) | fb) & mask;
        self.total_bits += 1;
        fb != 0
    }

    /// Run a simulated link burst of `n_bits` with `injected_errors` bit flips.
    pub fn run_burst(&mut self, n_bits: u64, injected_errors: u64) -> f64 {
        let step_bits = n_bits.min(2048);
        for _ in 0..step_bits {
            let _ = self.next_bit();
        }
        if n_bits > step_bits {
            self.total_bits += n_bits - step_bits;
        }
        self.error_bits = self.error_bits.saturating_add(injected_errors.min(n_bits));
        self.measured_ber()
    }

    pub fn reset_counters(&mut self) {
        self.total_bits = 0;
        self.error_bits = 0;
    }

    pub fn measured_ber(&self) -> f64 {
        if self.total_bits == 0 {
            return 1.0;
        }
        // Floor at 1 / total_bits when 0 errors observed (standard IBERT confidence floor).
        ((self.error_bits as f64).max(1.0)) / (self.total_bits as f64)
    }
}

/// Transceiver channel configuration for IBERT eye-scan and bathtub analysis.
#[derive(Clone, Debug)]
pub struct IbertLinkConfig {
    pub quad_channel: String,
    pub line_rate_gbps: f64,
    pub prbs: PrbsPattern,
    /// Peak-to-peak differential TX output swing in mV (e.g. `800.0`).
    pub tx_diff_swing_mv: f64,
    /// TX pre-cursor emphasis in dB (`0.0..=6.0`).
    pub tx_pre_cursor_db: f64,
    /// TX post-cursor emphasis in dB (`0.0..=9.0`).
    pub tx_post_cursor_db: f64,
    /// RX CTLE peaking gain in dB (`0.0..=12.0`).
    pub rx_ctle_gain_db: f64,
    /// RX Decision Feedback Equalizer (DFE) enabled.
    pub rx_dfe_enabled: bool,
    /// Channel insertion loss at Nyquist frequency in dB (`0.0..=30.0`).
    pub channel_loss_db: f64,
    /// Random jitter RMS in Unit Intervals (`UI`, e.g. `0.015`).
    pub rj_rms_ui: f64,
    /// Deterministic jitter peak-to-peak in Unit Intervals (`UI`, e.g. `0.10`).
    pub dj_pp_ui: f64,
    /// Vertical receiver thermal + crosstalk noise RMS in mV (`e.g. 14.0`).
    pub vertical_noise_rms_mv: f64,
    /// Target Bit Error Rate threshold for eye-opening contour (e.g. `1e-9` or `1e-12`).
    pub target_ber: f64,
}

impl Default for IbertLinkConfig {
    fn default() -> Self {
        Self {
            quad_channel: "MGT_X0Y0".to_string(),
            line_rate_gbps: 10.3125,
            prbs: PrbsPattern::Prbs31,
            tx_diff_swing_mv: 800.0,
            tx_pre_cursor_db: 1.5,
            tx_post_cursor_db: 3.5,
            rx_ctle_gain_db: 6.0,
            rx_dfe_enabled: true,
            channel_loss_db: 10.0,
            rj_rms_ui: 0.014,
            dj_pp_ui: 0.10,
            vertical_noise_rms_mv: 12.0,
            target_ber: 1e-9,
        }
    }
}

/// Complementary error function $\text{erfc}(x)$ approximation (Abramowitz & Stegun 7.1.26, max error $1.5 \times 10^{-7}$).
pub fn erfc_approx(x: f64) -> f64 {
    if x < 0.0 {
        return 2.0 - erfc_approx(-x);
    }
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let poly = t
        * (0.254829592
            + t * (-0.284496736
                + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    (poly * (-x * x).exp()).clamp(1e-18, 2.0)
}

/// Gaussian tail probability $Q(x) = \frac{1}{2}\text{erfc}\left(\frac{x}{\sqrt{2}}\right)$.
pub fn q_tail(q: f64) -> f64 {
    0.5 * erfc_approx(q / std::f64::consts::SQRT_2)
}

impl IbertLinkConfig {
    /// Effective vertical eye half-height (mV) at `phase_ui == 0.0` after channel loss and TX/RX equalization.
    pub fn effective_vertical_half_eye_mv(&self) -> f64 {
        let eq_boost_db = self.tx_pre_cursor_db * 0.6
            + self.tx_post_cursor_db * 0.8
            + self.rx_ctle_gain_db
            + if self.rx_dfe_enabled { 4.5 } else { 0.0 };
        let residual_loss_db =
            (self.channel_loss_db * self.prbs.isi_factor() - eq_boost_db).max(0.5);
        let atten = 10.0_f64.powf(-residual_loss_db / 20.0);
        // Half of peak-to-peak differential swing after attenuation.
        (0.5 * self.tx_diff_swing_mv * atten).max(5.0)
    }

    /// Effective deterministic jitter (UI) including residual ISI from uncompensated channel loss.
    pub fn effective_dj_pp_ui(&self) -> f64 {
        let eq_boost_db = self.tx_pre_cursor_db * 0.6
            + self.tx_post_cursor_db * 0.8
            + self.rx_ctle_gain_db
            + if self.rx_dfe_enabled { 4.5 } else { 0.0 };
        let uncomp_db = (self.channel_loss_db * self.prbs.isi_factor() - eq_boost_db).max(0.0);
        (self.dj_pp_ui + uncomp_db * 0.012).clamp(0.0, 0.85)
    }

    /// Dual-Dirac BER at `(phase_ui, voltage_mv)` where `phase_ui` $\in [-0.5, +0.5]$
    /// and `voltage_mv` is the vertical slicer threshold offset.
    pub fn ber_at(&self, phase_ui: f64, voltage_mv: f64) -> f64 {
        let rj = self.rj_rms_ui.max(0.002);
        let dj_half = 0.5 * self.effective_dj_pp_ui();
        let dist_from_edge_ui = (0.5 - phase_ui.abs() - dj_half).max(-0.5);
        let q_horiz = dist_from_edge_ui / rj;
        let ber_horiz = q_tail(q_horiz);

        // Cosine-shaped vertical eye envelope across the horizontal opening.
        let horiz_open_frac = (1.0 - (phase_ui.abs() / (0.5 - dj_half * 0.5).max(0.1)).powi(2))
            .max(0.0);
        let v_eye_at_phase = self.effective_vertical_half_eye_mv() * horiz_open_frac;
        let v_margin = v_eye_at_phase - voltage_mv.abs();
        let sigma_v = self.vertical_noise_rms_mv.max(1.0);
        let q_vert = v_margin / sigma_v;
        let ber_vert = q_tail(q_vert);

        // Combined BER in the 2D phase-voltage eye interior.
        (ber_horiz + ber_vert - ber_horiz * ber_vert).clamp(1e-18, 0.5)
    }
}

/// One sample on a 1D horizontal phase BER bathtub curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BathtubPoint {
    pub phase_ui: f64,
    pub ber: f64,
    pub log10_ber: f64,
    pub sampled_bits: u64,
    pub error_count: u64,
}

/// 1D Horizontal Phase BER Bathtub Curve analysis result.
#[derive(Clone, Debug)]
pub struct BathtubCurve {
    pub quad_channel: String,
    pub points: Vec<BathtubPoint>,
    pub target_ber: f64,
    pub horizontal_opening_ui: f64,
    pub center_ber: f64,
    pub q_factor: f64,
    pub estimated_rj_rms_ui: f64,
    pub estimated_dj_pp_ui: f64,
}

/// Sweep a 1D horizontal phase (`-0.5 UI .. +0.5 UI`) BER bathtub curve across `phase_steps`.
pub fn sweep_bathtub(
    cfg: &IbertLinkConfig,
    phase_steps: usize,
    bits_per_point: u64,
) -> Result<BathtubCurve, String> {
    if phase_steps < 3 {
        return Err("ibert: phase_steps must be >= 3".into());
    }
    let bits = bits_per_point.max(1_000_000);
    let mut points = Vec::with_capacity(phase_steps);
    let mut min_open_ui: Option<f64> = None;
    let mut max_open_ui: Option<f64> = None;

    for i in 0..phase_steps {
        let frac = (i as f64) / ((phase_steps - 1) as f64);
        let phase_ui = -0.5 + frac;
        let ber = cfg.ber_at(phase_ui, 0.0);
        let err_cnt = ((ber * (bits as f64)).round() as u64).min(bits);
        if ber <= cfg.target_ber {
            min_open_ui = Some(min_open_ui.map_or(phase_ui, |m| m.min(phase_ui)));
            max_open_ui = Some(max_open_ui.map_or(phase_ui, |m| m.max(phase_ui)));
        }
        points.push(BathtubPoint {
            phase_ui,
            ber,
            log10_ber: ber.log10(),
            sampled_bits: bits,
            error_count: err_cnt,
        });
    }

    let horizontal_opening_ui = match (min_open_ui, max_open_ui) {
        (Some(lo), Some(hi)) if hi >= lo => hi - lo,
        _ => 0.0,
    };
    let center_ber = cfg.ber_at(0.0, 0.0);
    let dj = cfg.effective_dj_pp_ui();
    let rj = cfg.rj_rms_ui.max(0.002);
    let q_factor = ((0.5 - 0.5 * dj) / rj).max(0.0);

    Ok(BathtubCurve {
        quad_channel: cfg.quad_channel.clone(),
        points,
        target_ber: cfg.target_ber,
        horizontal_opening_ui,
        center_ber,
        q_factor,
        estimated_rj_rms_ui: rj,
        estimated_dj_pp_ui: dj,
    })
}

/// Summary metrics extracted from a 2D IBERT eye-diagram scan.
#[derive(Clone, Debug, PartialEq)]
pub struct EyeMetrics {
    /// Horizontal eye opening in Unit Intervals (`0.0..=1.0 UI`) at `voltage_mv = 0` and `target_ber`.
    pub horizontal_opening_ui: f64,
    /// Vertical eye opening in mV at `phase_ui = 0` and `target_ber`.
    pub vertical_opening_mv: f64,
    /// Approximate open eye area in `UI * mV` where `ber <= target_ber`.
    pub eye_area_ui_mv: f64,
    /// Number of 2D grid points with `ber <= target_ber`.
    pub open_points: usize,
    /// Total 2D grid points scanned (`horiz_steps * vert_steps`).
    pub total_points: usize,
    /// BER at the eye center `(0.0 UI, 0.0 mV)`.
    pub center_ber: f64,
    /// Signal-to-noise Q-factor at eye center.
    pub q_factor: f64,
    /// True when the eye is open both horizontally (`>= 0.20 UI`) and vertically (`>= 40.0 mV`).
    pub margin_pass: bool,
}

/// Complete 2D horizontal-phase vs vertical-voltage IBERT eye-diagram scan.
#[derive(Clone, Debug)]
pub struct IbertEyeScan {
    pub quad_channel: String,
    pub line_rate_gbps: f64,
    pub prbs: PrbsPattern,
    pub target_ber: f64,
    /// Horizontal phase coordinates in UI (`-0.5 .. +0.5`, length `horiz_steps`).
    pub phase_axis_ui: Vec<f64>,
    /// Vertical voltage coordinates in mV (`-v_max .. +v_max`, length `vert_steps`).
    pub voltage_axis_mv: Vec<f64>,
    /// 2D BER matrix indexed as `ber_grid[v_idx][h_idx]`.
    pub ber_grid: Vec<Vec<f64>>,
    pub metrics: EyeMetrics,
}

impl IbertEyeScan {
    /// Render an ASCII heatmap of the 2D eye diagram (`' '` = open eye `BER <= target_ber`,
    /// `'.'`, `':'`, `'+'`, `'#'`, `'@'` = progressively higher BER toward eye edges).
    pub fn render_ascii(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "IBERT Eye Scan {} @ {:.4} Gbps ({}) horiz={:.3} UI vert={:.1} mV area={:.1} UI*mV pass={}\n",
            self.quad_channel,
            self.line_rate_gbps,
            self.prbs.as_str(),
            self.metrics.horizontal_opening_ui,
            self.metrics.vertical_opening_mv,
            self.metrics.eye_area_ui_mv,
            self.metrics.margin_pass
        ));
        for (v_idx, row) in self.ber_grid.iter().enumerate().rev() {
            let v_mv = self.voltage_axis_mv[v_idx];
            out.push_str(&format!("{v_mv:+6.1}mV |"));
            for &ber in row {
                let ch = if ber <= self.target_ber {
                    ' '
                } else if ber <= 1e-6 {
                    '.'
                } else if ber <= 1e-4 {
                    ':'
                } else if ber <= 1e-2 {
                    '+'
                } else if ber <= 1e-1 {
                    '#'
                } else {
                    '@'
                };
                out.push(ch);
            }
            out.push_str("|\n");
        }
        out
    }
}

/// Execute a 2D horizontal-phase vs vertical-voltage IBERT eye-diagram sweep.
pub fn run_ibert_eye_scan(
    cfg: &IbertLinkConfig,
    horiz_steps: usize,
    vert_steps: usize,
) -> Result<IbertEyeScan, String> {
    if horiz_steps < 3 || vert_steps < 3 {
        return Err("ibert: horiz_steps and vert_steps must both be >= 3".into());
    }
    let v_max = (0.5 * cfg.tx_diff_swing_mv).max(100.0);
    let phase_axis_ui: Vec<f64> = (0..horiz_steps)
        .map(|i| -0.5 + (i as f64) / ((horiz_steps - 1) as f64))
        .collect();
    let voltage_axis_mv: Vec<f64> = (0..vert_steps)
        .map(|j| -v_max + (2.0 * v_max * (j as f64)) / ((vert_steps - 1) as f64))
        .collect();

    let d_ui = 1.0 / ((horiz_steps - 1) as f64);
    let d_mv = (2.0 * v_max) / ((vert_steps - 1) as f64);

    let mut ber_grid = Vec::with_capacity(vert_steps);
    let mut open_points = 0usize;
    for &v_mv in &voltage_axis_mv {
        let mut row = Vec::with_capacity(horiz_steps);
        for &p_ui in &phase_axis_ui {
            let ber = cfg.ber_at(p_ui, v_mv);
            if ber <= cfg.target_ber {
                open_points += 1;
            }
            row.push(ber);
        }
        ber_grid.push(row);
    }

    // Center row (closest to 0 mV) horizontal eye opening:
    let center_v_idx = vert_steps / 2;
    let mut h_lo: Option<f64> = None;
    let mut h_hi: Option<f64> = None;
    for (h_idx, &ber) in ber_grid[center_v_idx].iter().enumerate() {
        if ber <= cfg.target_ber {
            let p = phase_axis_ui[h_idx];
            h_lo = Some(h_lo.map_or(p, |m| m.min(p)));
            h_hi = Some(h_hi.map_or(p, |m| m.max(p)));
        }
    }
    let horizontal_opening_ui = match (h_lo, h_hi) {
        (Some(lo), Some(hi)) if hi >= lo => hi - lo,
        _ => 0.0,
    };

    // Center column (closest to 0 UI) vertical eye opening:
    let center_h_idx = horiz_steps / 2;
    let mut v_lo: Option<f64> = None;
    let mut v_hi: Option<f64> = None;
    for (v_idx, row) in ber_grid.iter().enumerate() {
        if row[center_h_idx] <= cfg.target_ber {
            let v = voltage_axis_mv[v_idx];
            v_lo = Some(v_lo.map_or(v, |m| m.min(v)));
            v_hi = Some(v_hi.map_or(v, |m| m.max(v)));
        }
    }
    let vertical_opening_mv = match (v_lo, v_hi) {
        (Some(lo), Some(hi)) if hi >= lo => hi - lo,
        _ => 0.0,
    };

    let eye_area_ui_mv = (open_points as f64) * d_ui * d_mv;
    let center_ber = cfg.ber_at(0.0, 0.0);
    let dj = cfg.effective_dj_pp_ui();
    let rj = cfg.rj_rms_ui.max(0.002);
    let q_horiz = ((0.5 - 0.5 * dj) / rj).max(0.0);
    let q_vert = (cfg.effective_vertical_half_eye_mv() / cfg.vertical_noise_rms_mv.max(1.0)).max(0.0);
    let q_factor = q_horiz.min(q_vert);
    let margin_pass = horizontal_opening_ui >= 0.20 && vertical_opening_mv >= 40.0;

    Ok(IbertEyeScan {
        quad_channel: cfg.quad_channel.clone(),
        line_rate_gbps: cfg.line_rate_gbps,
        prbs: cfg.prbs,
        target_ber: cfg.target_ber,
        phase_axis_ui,
        voltage_axis_mv,
        ber_grid,
        metrics: EyeMetrics {
            horizontal_opening_ui,
            vertical_opening_mv,
            eye_area_ui_mv,
            open_points,
            total_points: horiz_steps * vert_steps,
            center_ber,
            q_factor,
            margin_pass,
        },
    })
}
