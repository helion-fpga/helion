//! Vivado UG899 I/O and Clock Planning (`io_plan` / `report_ssn`).
//!
//! Provides:
//! 1. BGA package ballout coordinate grids (`CPG236`, `CSG324`, `FGG484`, `CABGA381`) with
//!    JEDEC row lettering, user I/O banks, differential `P`/`N` pair mates, `MRCC`/`SRCC`
//!    clock-capable pins, `VREF` pins, and `VCCO`/`VCCINT`/`VCCAUX`/`GND` power/ground distribution.
//! 2. I/O bank `VCCO`/`VREF` voltage-standard compatibility & differential pair legality DRC
//!    (`BIVC-1`, `PLIO-1`..`PLIO-8`) covering `LVCMOS33`, `LVCMOS25`, `LVCMOS18`, `LVCMOS15`,
//!    `LVCMOS12`, `LVDS_25`, `LVDS`, `SSTL15`, `SSTL15_I`, `HSTL_I`, `HSTL_I_18`, and `TMDS_33`.
//! 3. Simultaneous Switching Noise (SSN) ground-bounce margin analyzer (`analyze_ssn` / `report_ssn`)
//!    computing per-pin and per-bank noise margins (%).

use crate::{Device, Site, SiteKind};
use std::collections::BTreeMap;

// ============================================================================
// 1. BGA Package Ballout Coordinate Grids (CPG236, CSG324, FGG484, CABGA381)
// ============================================================================

/// Standard JEDEC BGA row labels (omits I, O, Q, S, X, Z per JEDEC MO-275 / UG899).
pub const JEDEC_BGA_ROW_LABELS: [&str; 22] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "J", "K", "L", "M", "N", "P", "R", "T", "U", "V", "W",
    "Y", "AA", "AB",
];

/// Supported BGA packages in Helion UG899 I/O and Clock Planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PackageKind {
    /// 236-ball Chip-Scale BGA (16×16 outer matrix with central cavity/depopulated balls, 236 active balls).
    Cpg236,
    /// 324-ball Chip-Scale BGA (18×18 full ball matrix = 324 balls).
    Csg324,
    /// 484-ball Fine-Pitch BGA (22×22 full ball matrix = 484 balls).
    Fgg484,
    /// 381-ball Chip-Array BGA (20×20 outer matrix with 19 depopulated corner/center balls = 381 balls).
    Cabga381,
}

impl PackageKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpg236 => "CPG236",
            Self::Csg324 => "CSG324",
            Self::Fgg484 => "FGG484",
            Self::Cabga381 => "CABGA381",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_uppercase().as_str() {
            "CPG236" => Some(Self::Cpg236),
            "CSG324" => Some(Self::Csg324),
            "FGG484" | "FBG484" => Some(Self::Fgg484),
            "CABGA381" => Some(Self::Cabga381),
            _ => None,
        }
    }

    /// Grid dimensions `(rows, cols)` of the BGA ball matrix.
    pub fn dimensions(self) -> (usize, usize) {
        match self {
            Self::Cpg236 => (16, 16),
            Self::Csg324 => (18, 18),
            Self::Fgg484 => (22, 22),
            Self::Cabga381 => (20, 20),
        }
    }

    /// Nominal physical ball count for this package.
    pub fn ball_count(self) -> usize {
        match self {
            Self::Cpg236 => 236,
            Self::Csg324 => 324,
            Self::Fgg484 => 484,
            Self::Cabga381 => 381,
        }
    }

    /// Number of I/O banks exposed by this package.
    pub fn bank_count(self) -> u32 {
        match self {
            Self::Cpg236 => 4,
            Self::Csg324 => 6,
            Self::Fgg484 => 8,
            Self::Cabga381 => 6,
        }
    }
}

/// Functional role of a BGA package ball (UG899).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PinFunction {
    /// Single-ended or general user I/O ball.
    UserIo,
    /// Differential positive user I/O ball (`IO_LxxP_T...`).
    DiffPairP,
    /// Differential negative user I/O ball (`IO_LxxN_T...`).
    DiffPairN,
    /// Multi-Region / Single-Region Clock-Capable positive ball (`MRCC_P` / `SRCC_P`).
    ClockCapableP,
    /// Multi-Region / Single-Region Clock-Capable negative ball (`MRCC_N` / `SRCC_N`).
    ClockCapableN,
    /// Bank reference voltage pin (`VREF`, usable as user I/O when bank does not require external VREF).
    Vref,
    /// Bank output supply pin (`VCCO`).
    Vcco,
    /// Core supply pin (`VCCINT`).
    Vccint,
    /// Auxiliary supply pin (`VCCAUX`).
    Vccaux,
    /// Ground return pin (`GND`).
    Gnd,
    /// Dedicated configuration / JTAG pin (`TCK`, `TMS`, `TDI`, `TDO`, `DONE`, `PROGRAM_B`).
    Config,
}

impl PinFunction {
    /// True if this ball can be assigned to a user HDL port.
    pub fn is_user_assignable(self) -> bool {
        matches!(
            self,
            Self::UserIo
                | Self::DiffPairP
                | Self::DiffPairN
                | Self::ClockCapableP
                | Self::ClockCapableN
                | Self::Vref
        )
    }

    /// True if this ball is the positive (`_P`) half of a differential pair.
    pub fn is_diff_p(self) -> bool {
        matches!(self, Self::DiffPairP | Self::ClockCapableP)
    }

    /// True if this ball is the negative (`_N`) half of a differential pair.
    pub fn is_diff_n(self) -> bool {
        matches!(self, Self::DiffPairN | Self::ClockCapableN)
    }

    /// True if this ball is a global/regional clock-capable input (`MRCC`/`SRCC`).
    pub fn is_clock_capable(self) -> bool {
        matches!(self, Self::ClockCapableP | Self::ClockCapableN)
    }
}

/// Metadata for a single BGA ball in a [`BgaPackageGrid`].
#[derive(Clone, Debug, PartialEq)]
pub struct BgaBall {
    /// Canonical ball coordinate name, e.g. `"A1"`, `"B12"`, `"AA20"`.
    pub ball: String,
    /// JEDEC row label (`"A"`, `"B"`, ...).
    pub row_label: String,
    /// 0-based row index in the BGA matrix.
    pub row_idx: usize,
    /// 1-based column number in the BGA matrix (`1..=cols`).
    pub col: usize,
    /// Functional type of this ball.
    pub function: PinFunction,
    /// I/O bank number (`Some(bank)` for `UserIo` / `DiffPair*` / `ClockCapable*` / `Vref` / `Vcco`).
    pub bank: Option<u32>,
    /// Mapped HAD [`Site`] if this ball connects to an `IOB` site on the die.
    pub site: Option<Site>,
    /// Complementary differential mate ball name (e.g. `"A2"` for `"A1"`), if part of a diff pair.
    pub diff_pair_mate: Option<String>,
    ///UG899 pin package trace self-inductance in nanohenries (nH).
    pub pkg_inductance_nh: f64,
    /// Package pin series resistance in milliohms (mΩ).
    pub pkg_resistance_mohm: f64,
    /// Package pin parasitic capacitance in picofarads (pF).
    pub pkg_capacitance_pf: f64,
}

/// Complete BGA package ballout coordinate grid (UG899 Package Pins view).
#[derive(Clone, Debug, PartialEq)]
pub struct BgaPackageGrid {
    pub kind: PackageKind,
    pub rows: usize,
    pub cols: usize,
    /// Balls ordered by `(row_idx, col)`.
    pub balls: Vec<BgaBall>,
    ball_index: BTreeMap<String, usize>,
}

impl BgaPackageGrid {
    /// Build the normative BGA package ballout coordinate grid for `kind`, optionally binding
    /// user I/O balls to `dev`'s HAD IOB sites.
    pub fn new(kind: PackageKind, dev: Option<&Device>) -> Self {
        let (rows, cols) = kind.dimensions();
        let n_banks = kind.bank_count();
        let iob_sites: Vec<Site> = dev.map(|d| d.iob_sites().collect()).unwrap_or_default();

        // Collect valid (row_idx, col) coordinates (depopulating center/corners where needed to match exact ball count)
        let mut coords = Vec::with_capacity(kind.ball_count());
        for r in 0..rows {
            for c in 1..=cols {
                let keep = match kind {
                    PackageKind::Cpg236 => {
                        // 16x16 = 256 minus 20 inner cavity balls (4x5 in rows 6..10, cols 6..=10) = 236 balls
                        !( (6..10).contains(&r) && (6..=10).contains(&c) )
                    }
                    PackageKind::Csg324 => true, // 18x18 = 324
                    PackageKind::Fgg484 => true, // 22x22 = 484
                    PackageKind::Cabga381 => {
                        // 20x20 = 400 minus 19 balls (4 extreme corners + 3x5 center thermal island) = 381 balls
                        let is_corner = (r == 0 || r == rows - 1) && (c == 1 || c == cols);
                        let is_center = (8..11).contains(&r) && (8..=12).contains(&c);
                        !is_corner && !is_center
                    }
                };
                if keep {
                    coords.push((r, c));
                }
            }
        }
        debug_assert_eq!(coords.len(), kind.ball_count());

        let center_r = (rows as f64 - 1.0) * 0.5;
        let center_c = (cols as f64 + 1.0) * 0.5;
        let max_dist = (center_r.hypot(center_c)).max(1.0);

        let mut balls = Vec::with_capacity(coords.len());
        let mut user_io_indices = Vec::new();

        for (idx, &(r, c)) in coords.iter().enumerate() {
            let row_label = JEDEC_BGA_ROW_LABELS[r].to_string();
            let ball_name = format!("{row_label}{c}");

            //Normalized radial distance from die center (outer balls have longer wirebond/substrate traces)
            let radial = ((r as f64 - center_r).hypot(c as f64 - center_c)) / max_dist;
            let pkg_inductance_nh = 1.6 + 2.4 * radial;
            let pkg_resistance_mohm = 65.0 + 95.0 * radial;
            let pkg_capacitance_pf = 1.1 + 0.9 * radial;

            // Determine quadrant / bank for perimeter & mid-ring balls
            let angle_sector = {
                let dy = r as f64 - center_r;
                let dx = c as f64 - center_c;
                let ang = dy.atan2(dx) + std::f64::consts::PI; // [0, 2*PI]
                ((ang / (2.0 * std::f64::consts::PI)) * (n_banks as f64)).floor() as u32
                    % n_banks
            };
            let bank_id = angle_sector;

            // Core area has VCCINT, VCCAUX, GND, and Config balls; outer rings have User I/O + VCCO + GND
            let is_core_ring = radial < 0.34;
            let function = if is_core_ring {
                match (r + c) % 4 {
                    0 => PinFunction::Gnd,
                    1 => PinFunction::Vccint,
                    2 => PinFunction::Vccaux,
                    _ => PinFunction::Config,
                }
            } else if (r * 7 + c * 3) % 11 == 0 {
                PinFunction::Gnd
            } else if (r * 5 + c * 11) % 17 == 0 {
                PinFunction::Vcco
            } else {
                user_io_indices.push(idx);
                PinFunction::UserIo
            };

            let bank = match function {
                PinFunction::Gnd | PinFunction::Vccint | PinFunction::Vccaux | PinFunction::Config => {
                    None
                }
                _ => Some(bank_id),
            };

            balls.push(BgaBall {
                ball: ball_name,
                row_label,
                row_idx: r,
                col: c,
                function,
                bank,
                site: None,
                diff_pair_mate: None,
                pkg_inductance_nh,
                pkg_resistance_mohm,
                pkg_capacitance_pf,
            });
        }

        // Group user-assignable balls by bank so every bank gets:
        // - at least one VREF pin
        // - at least one ClockCapableP / ClockCapableN differential pair
        // - paired DiffPairP / DiffPairN balls within the same bank
        let mut by_bank: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for &b_idx in &user_io_indices {
            if let Some(bk) = balls[b_idx].bank {
                by_bank.entry(bk).or_default().push(b_idx);
            }
        }

        let mut site_cursor = 0usize;
        for (_bk, indices) in by_bank {
            if indices.is_empty() {
                continue;
            }
            // Assign HAD IOB sites in order when available
            for &i in &indices {
                if site_cursor < iob_sites.len() {
                    balls[i].site = Some(iob_sites[site_cursor]);
                    site_cursor += 1;
                } else {
                    // Synthesize deterministic IOB site coordinates when package has more user I/Os than part IOBs
                    balls[i].site = Some(Site {
                        x: 2 + (site_cursor as u32),
                        y: 0,
                        kind: SiteKind::Iob,
                    });
                    site_cursor += 1;
                }
            }

            // First pin in bank is VREF
            balls[indices[0]].function = PinFunction::Vref;

            // Pair up consecutive remaining pins in the bank as differential P/N pairs
            let mut k = 1usize;
            let mut assigned_cc = false;
            while k + 1 < indices.len() {
                let p_idx = indices[k];
                let n_idx = indices[k + 1];
                let p_name = balls[p_idx].ball.clone();
                let n_name = balls[n_idx].ball.clone();
                if !assigned_cc {
                    balls[p_idx].function = PinFunction::ClockCapableP;
                    balls[n_idx].function = PinFunction::ClockCapableN;
                    assigned_cc = true;
                } else {
                    balls[p_idx].function = PinFunction::DiffPairP;
                    balls[n_idx].function = PinFunction::DiffPairN;
                }
                balls[p_idx].diff_pair_mate = Some(n_name);
                balls[n_idx].diff_pair_mate = Some(p_name);
                k += 2;
            }
        }

        let mut ball_index = BTreeMap::new();
        for (idx, b) in balls.iter().enumerate() {
            ball_index.insert(b.ball.clone(), idx);
        }

        Self {
            kind,
            rows,
            cols,
            balls,
            ball_index,
        }
    }

    /// Look up a ball by its coordinate name (e.g. `"A1"`, case-insensitive).
    pub fn pin(&self, ball_name: &str) -> Option<&BgaBall> {
        let key = ball_name.trim().to_ascii_uppercase();
        self.ball_index.get(&key).map(|&i| &self.balls[i])
    }

    /// Return all user-assignable I/O balls in a specific bank.
    pub fn bank_user_pins(&self, bank: u32) -> Vec<&BgaBall> {
        self.balls
            .iter()
            .filter(|b| b.bank == Some(bank) && b.function.is_user_assignable())
            .collect()
    }

    /// Return all complementary `(P, N)` differential pairs in this package.
    pub fn differential_pairs(&self) -> Vec<(&BgaBall, &BgaBall)> {
        let mut pairs = Vec::new();
        for b in &self.balls {
            if b.function.is_diff_p() {
                if let Some(mate_name) = &b.diff_pair_mate {
                    if let Some(mate) = self.pin(mate_name) {
                        pairs.push((b, mate));
                    }
                }
            }
        }
        pairs
    }
}

// ============================================================================
// 2. I/O Standards & Bank VCCO/VREF + Differential Pair DRC (UG899)
// ============================================================================

/// Supported UG899 / UG471 I/O standards for bank VCCO/VREF & SSN analysis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum IoStandard {
    Lvcmos12,
    Lvcmos15,
    Lvcmos18,
    Lvcmos25,
    Lvcmos33,
    Lvds25,
    Lvds18,
    Sstl15,
    Sstl15I,
    Sstl135,
    HstlI,
    HstlI18,
    Tmds33,
}

impl IoStandard {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "LVCMOS12" => Some(Self::Lvcmos12),
            "LVCMOS15" => Some(Self::Lvcmos15),
            "LVCMOS18" => Some(Self::Lvcmos18),
            "LVCMOS25" => Some(Self::Lvcmos25),
            "LVCMOS33" | "LVTTL" => Some(Self::Lvcmos33),
            "LVDS_25" | "LVDS25" => Some(Self::Lvds25),
            "LVDS" | "LVDS_18" | "LVDS18" => Some(Self::Lvds18),
            "SSTL15" => Some(Self::Sstl15),
            "SSTL15_I" | "SSTL15_II" => Some(Self::Sstl15I),
            "SSTL135" => Some(Self::Sstl135),
            "HSTL_I" | "HSTL_I_15" => Some(Self::HstlI),
            "HSTL_I_18" | "HSTL_II_18" => Some(Self::HstlI18),
            "TMDS_33" => Some(Self::Tmds33),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lvcmos12 => "LVCMOS12",
            Self::Lvcmos15 => "LVCMOS15",
            Self::Lvcmos18 => "LVCMOS18",
            Self::Lvcmos25 => "LVCMOS25",
            Self::Lvcmos33 => "LVCMOS33",
            Self::Lvds25 => "LVDS_25",
            Self::Lvds18 => "LVDS_18",
            Self::Sstl15 => "SSTL15",
            Self::Sstl15I => "SSTL15_I",
            Self::Sstl135 => "SSTL135",
            Self::HstlI => "HSTL_I",
            Self::HstlI18 => "HSTL_I_18",
            Self::Tmds33 => "TMDS_33",
        }
    }

    /// Required bank `VCCO` in millivolts (mV).
    pub fn required_vcco_mv(self) -> u32 {
        match self {
            Self::Lvcmos12 => 1200,
            Self::Sstl135 => 1350,
            Self::Lvcmos15 | Self::Sstl15 | Self::Sstl15I | Self::HstlI => 1500,
            Self::Lvcmos18 | Self::Lvds18 | Self::HstlI18 => 1800,
            Self::Lvcmos25 | Self::Lvds25 => 2500,
            Self::Lvcmos33 | Self::Tmds33 => 3300,
        }
    }

    /// Required bank `VREF` in millivolts (mV) for reference-based inputs (`SSTL` / `HSTL`).
    pub fn required_vref_mv(self) -> Option<u32> {
        match self {
            Self::Sstl135 => Some(675),
            Self::Sstl15 | Self::Sstl15I | Self::HstlI => Some(750),
            Self::HstlI18 => Some(900),
            _ => None,
        }
    }

    /// True if this standard requires differential `P`/`N` pin pairs.
    pub fn is_differential(self) -> bool {
        matches!(self, Self::Lvds25 | Self::Lvds18 | Self::Tmds33)
    }

    /// True if on-die differential termination (`DIFF_TERM`) is supported.
    pub fn supports_diff_term(self) -> bool {
        matches!(self, Self::Lvds25 | Self::Lvds18)
    }

    /// True if split on-die input termination (`IN_TERM`) is supported.
    pub fn supports_in_term(self) -> bool {
        matches!(
            self,
            Self::Sstl15 | Self::Sstl15I | Self::Sstl135 | Self::HstlI | Self::HstlI18
        )
    }

    /// Legal single-ended drive strengths in mA. Differential standards have fixed current-mode drivers.
    pub fn legal_drive_ma(self) -> &'static [u32] {
        match self {
            Self::Lvcmos12 => &[2, 4, 6, 8],
            Self::Lvcmos15 => &[2, 4, 6, 8, 12, 16],
            Self::Lvcmos18 => &[2, 4, 6, 8, 12, 16, 24],
            Self::Lvcmos25 => &[4, 8, 12, 16, 24],
            Self::Lvcmos33 => &[4, 8, 12, 16, 24],
            Self::Sstl15 | Self::Sstl15I | Self::Sstl135 | Self::HstlI | Self::HstlI18 => &[8, 12, 16],
            Self::Lvds25 | Self::Lvds18 | Self::Tmds33 => &[],
        }
    }

    /// Nominal receiver noise margin budget in mV before ground bounce / VCCO droop causes false switching.
    pub fn ssn_noise_budget_mv(self) -> f64 {
        match self {
            Self::Lvcmos12 => 280.0,
            Self::Sstl135 => 240.0,
            Self::Lvcmos15 => 350.0,
            Self::Sstl15 | Self::Sstl15I | Self::HstlI => 260.0,
            Self::Lvcmos18 => 420.0,
            Self::HstlI18 => 300.0,
            Self::Lvds18 | Self::Lvds25 => 220.0,
            Self::Lvcmos25 => 550.0,
            Self::Lvcmos33 | Self::Tmds33 => 700.0,
        }
    }
}

/// Signal direction of a top-level I/O port.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum IoDirection {
    Input,
    Output,
    Inout,
}

impl IoDirection {
    pub fn is_driver(self) -> bool {
        matches!(self, Self::Output | Self::Inout)
    }

    pub fn is_receiver(self) -> bool {
        matches!(self, Self::Input | Self::Inout)
    }
}

/// Differential polarity role of a port.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum DiffPolarity {
    SingleEnded,
    Positive,
    Negative,
}

/// User constraint / assignment for a single top-level I/O port (UG899 / XDC).
#[derive(Clone, Debug, PartialEq)]
pub struct IoPinAssignment {
    pub port: String,
    /// Assigned package ball coordinate (e.g. `Some("A1".into())`).
    pub package_pin: Option<String>,
    /// I/O electrical standard string (e.g. `"LVCMOS18"`, `"LVCMOS33"`, `"LVDS_25"`, `"SSTL15"`).
    pub iostandard: String,
    pub direction: IoDirection,
    pub polarity: DiffPolarity,
    /// Complementary differential port name (for `Positive` / `Negative` differential ports).
    pub diff_mate_port: Option<String>,
    /// Output drive strength in mA (`None` uses default for standard, e.g. 12 mA for LVCMOS).
    pub drive_ma: Option<u32>,
    /// Slew rate (`"SLOW"` or `"FAST"`, default `"SLOW"`).
    pub slew: Option<String>,
    /// Pull type (`"NONE"`, `"PULLUP"`, `"PULLDOWN"`, `"KEEPER"`).
    pub pulltype: Option<String>,
    /// On-die differential termination (`Some(true)` for `DIFF_TERM=TRUE`).
    pub diff_term: Option<bool>,
    /// Split input termination (`"NONE"`, `"UNTUNED_SPLIT_40"`, `"UNTUNED_SPLIT_50"`, `"UNTUNED_SPLIT_60"`).
    pub in_term: Option<String>,
    /// True if this port drives a global/regional clock tree (`BUFG` / `MMCM`).
    pub is_clock_input: bool,
    /// Expected switching toggle rate in MHz for SSN analysis (default 100.0 MHz).
    pub toggle_rate_mhz: f64,
    /// External board trace + load capacitance in pF for SSN analysis (default 10.0 pF).
    pub board_load_pf: f64,
}

impl IoPinAssignment {
    pub fn new(
        port: impl Into<String>,
        package_pin: Option<&str>,
        iostandard: impl Into<String>,
        direction: IoDirection,
    ) -> Self {
        Self {
            port: port.into(),
            package_pin: package_pin.map(|s| s.trim().to_ascii_uppercase()),
            iostandard: iostandard.into(),
            direction,
            polarity: DiffPolarity::SingleEnded,
            diff_mate_port: None,
            drive_ma: None,
            slew: None,
            pulltype: None,
            diff_term: None,
            in_term: None,
            is_clock_input: false,
            toggle_rate_mhz: 100.0,
            board_load_pf: 10.0,
        }
    }
}

/// Complete I/O pin planning table for a package (`IoPinAssignment` collection).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IoPlan {
    pub assignments: Vec<IoPinAssignment>,
}

impl IoPlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, assignment: IoPinAssignment) {
        self.assignments.push(assignment);
    }

    /// Convenience helper to add a differential `P`/`N` port pair.
    pub fn add_diff_pair(
        &mut self,
        port_p: impl Into<String>,
        pin_p: &str,
        port_n: impl Into<String>,
        pin_n: &str,
        iostandard: impl Into<String>,
        direction: IoDirection,
        diff_term: Option<bool>,
    ) {
        let p_name = port_p.into();
        let n_name = port_n.into();
        let std_str = iostandard.into();

        let mut p_assign = IoPinAssignment::new(
            p_name.clone(),
            Some(pin_p),
            std_str.clone(),
            direction,
        );
        p_assign.polarity = DiffPolarity::Positive;
        p_assign.diff_mate_port = Some(n_name.clone());
        p_assign.diff_term = diff_term;

        let mut n_assign = IoPinAssignment::new(n_name, Some(pin_n), std_str, direction);
        n_assign.polarity = DiffPolarity::Negative;
        n_assign.diff_mate_port = Some(p_name);
        n_assign.diff_term = diff_term;

        self.assignments.push(p_assign);
        self.assignments.push(n_assign);
    }
}

/// Severity of a UG899 I/O DRC violation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum IoDrcSeverity {
    Error,
    Warning,
    Advisory,
}

impl IoDrcSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Advisory => "Advisory",
        }
    }
}

/// Single UG899 I/O and Clock Planning DRC violation (`BIVC-1`, `PLIO-1`..`PLIO-8`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoDrcViolation {
    /// Vivado UG899 rule ID (`"BIVC-1"`, `"PLIO-1"`, ..., `"PLIO-8"`).
    pub code: String,
    pub severity: IoDrcSeverity,
    pub objects: Vec<String>,
    pub message: String,
}

/// Result of running [`check_io_drc`] on a [`BgaPackageGrid`] and [`IoPlan`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoDrcReport {
    pub violations: Vec<IoDrcViolation>,
}

impl IoDrcReport {
    pub fn is_clean(&self) -> bool {
        !self
            .violations
            .iter()
            .any(|v| v.severity == IoDrcSeverity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == IoDrcSeverity::Error)
            .count()
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.violations
            .iter()
            .any(|v| v.code.eq_ignore_ascii_case(code))
    }

    pub fn by_code(&self, code: &str) -> Vec<&IoDrcViolation> {
        self.violations
            .iter()
            .filter(|v| v.code.eq_ignore_ascii_case(code))
            .collect()
    }

    pub fn summary_text(&self) -> String {
        if self.violations.is_empty() {
            return "report_io_drc violations=0 ok".into();
        }
        let mut lines = vec![format!(
            "report_io_drc violations={} errors={}",
            self.violations.len(),
            self.error_count()
        )];
        for v in &self.violations {
            lines.push(format!(
                "{} {} [{}] {}",
                v.code,
                v.severity.as_str(),
                v.objects.join(", "),
                v.message
            ));
        }
        lines.join("\n")
    }
}

/// Run Vivado UG899 I/O and Clock Planning DRC checks (`BIVC-1`, `PLIO-1`..`PLIO-8`).
///
/// Rules enforced:
/// - **`BIVC-1`**: Bank `VCCO` / `VREF` voltage conflict (incompatible `VCCO` or `VREF` requirements
///   within the same I/O bank).
/// - **`PLIO-1`**: Invalid package ball coordinate or assignment of a user port to a dedicated
///   non-user ball (`GND`, `VCCINT`, `VCCAUX`, `VCCO`, `Config`).
/// - **`PLIO-2`**: Duplicate package ball assignment (multiple ports assigned to the same ball).
/// - **`PLIO-3`**: Differential `IOSTANDARD` (`LVDS_25`, `LVDS_18`, `TMDS_33`) placed on a
///   non-differential ball or with inverted `P`/`N` ball polarity (`Positive` on `_N` or `Negative` on `_P`).
/// - **`PLIO-4`**: Differential pair `P`/`N` ports not placed on true complementary mate balls
///   (`diff_pair_mate` mismatch or missing mate port).
/// - **`PLIO-5`**: Unknown `IOSTANDARD` or illegal `DRIVE` / `SLEW` / `PULLTYPE` for the standard
///   (e.g., setting `DRIVE` on current-mode differential `LVDS_25`, or 24 mA on `LVCMOS12`).
/// - **`PLIO-6`**: `VREF`-dependent receiver (`SSTL15`, `HSTL_I`) placed in a bank whose dedicated
///   `VREF` ball is occupied by a user port.
/// - **`PLIO-7`**: Illegal on-die termination (`DIFF_TERM` or `IN_TERM`) for the port direction or `IOSTANDARD`.
/// - **`PLIO-8`**: Clock input port (`is_clock_input = true`) placed on a non-clock-capable (`MRCC`/`SRCC`) ball.
pub fn check_io_drc(pkg: &BgaPackageGrid, plan: &IoPlan) -> IoDrcReport {
    let mut report = IoDrcReport::default();

    // Map package ball -> ports assigned to it
    let mut ball_to_ports: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Map port name -> assignment index
    let mut port_index: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, a) in plan.assignments.iter().enumerate() {
        port_index.insert(a.port.clone(), idx);
        if let Some(pin) = &a.package_pin {
            ball_to_ports
                .entry(pin.trim().to_ascii_uppercase())
                .or_default()
                .push(a.port.clone());
        }
    }

    // PLIO-2: Duplicate ball assignments
    for (ball_name, ports) in &ball_to_ports {
        if ports.len() > 1 {
            report.violations.push(IoDrcViolation {
                code: "PLIO-2".into(),
                severity: IoDrcSeverity::Error,
                objects: ports.clone(),
                message: format!(
                    "Multiple ports ({}) are assigned to the same package pin {}",
                    ports.join(", "),
                    ball_name
                ),
            });
        }
    }

    // Track per-bank VCCO and VREF demands for BIVC-1 and PLIO-6
    // bank -> Vec<(port, std_name, required_vcco_mv, is_driver)>
    let mut bank_vcco: BTreeMap<u32, Vec<(String, String, u32, bool)>> = BTreeMap::new();
    // bank -> Vec<(port, std_name, required_vref_mv)>
    let mut bank_vref: BTreeMap<u32, Vec<(String, String, u32)>> = BTreeMap::new();
    // bank -> Vec<(port, ball)> occupying a VREF pin
    let mut bank_vref_ball_users: BTreeMap<u32, Vec<(String, String)>> = BTreeMap::new();

    for a in &plan.assignments {
        let std_opt = IoStandard::parse(&a.iostandard);
        let Some(std) = std_opt else {
            report.violations.push(IoDrcViolation {
                code: "PLIO-5".into(),
                severity: IoDrcSeverity::Error,
                objects: vec![a.port.clone()],
                message: format!(
                    "Port '{}' specifies unsupported IOSTANDARD '{}'",
                    a.port, a.iostandard
                ),
            });
            continue;
        };

        // PLIO-5: Drive strength / slew / pulltype legality
        if let Some(ma) = a.drive_ma {
            if std.is_differential() {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-5".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![a.port.clone()],
                    message: format!(
                        "Differential port '{}' ({}) cannot specify single-ended DRIVE strength ({} mA)",
                        a.port,
                        std.as_str(),
                        ma
                    ),
                });
            } else if !std.legal_drive_ma().contains(&ma) {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-5".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![a.port.clone()],
                    message: format!(
                        "DRIVE {} mA on port '{}' is not legal for IOSTANDARD {} (allowed: {:?})",
                        ma,
                        a.port,
                        std.as_str(),
                        std.legal_drive_ma()
                    ),
                });
            }
        }
        if let Some(slew) = &a.slew {
            if !Device::legal_slew(slew) {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-5".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![a.port.clone()],
                    message: format!(
                        "Invalid SLEW '{}' on port '{}' (expected SLOW or FAST)",
                        slew, a.port
                    ),
                });
            }
        }
        if let Some(pull) = &a.pulltype {
            if !Device::legal_pulltype(pull) {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-5".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![a.port.clone()],
                    message: format!("Invalid PULLTYPE '{}' on port '{}'", pull, a.port),
                });
            }
        }

        // PLIO-7: DIFF_TERM and IN_TERM legality
        if a.diff_term == Some(true) {
            if !std.supports_diff_term() || !a.direction.is_receiver() {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-7".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![a.port.clone()],
                    message: format!(
                        "DIFF_TERM=TRUE on port '{}' is only legal on differential LVDS receiver ports (found {:?} {})",
                        a.port,
                        a.direction,
                        std.as_str()
                    ),
                });
            }
        }
        if let Some(in_term) = &a.in_term {
            if !in_term.eq_ignore_ascii_case("NONE") {
                if !Device::legal_in_term(in_term) || !std.supports_in_term() || !a.direction.is_receiver() {
                    report.violations.push(IoDrcViolation {
                        code: "PLIO-7".into(),
                        severity: IoDrcSeverity::Error,
                        objects: vec![a.port.clone()],
                        message: format!(
                            "IN_TERM='{}' on port '{}' is not legal for {:?} {}",
                            in_term,
                            a.port,
                            a.direction,
                            std.as_str()
                        ),
                    });
                }
            }
        }

        // Validate package ball if assigned
        let Some(pin_name) = &a.package_pin else {
            continue;
        };
        let Some(ball) = pkg.pin(pin_name) else {
            report.violations.push(IoDrcViolation {
                code: "PLIO-1".into(),
                severity: IoDrcSeverity::Error,
                objects: vec![a.port.clone()],
                message: format!(
                    "Port '{}' is assigned to non-existent ball '{}' on package {}",
                    a.port,
                    pin_name,
                    pkg.kind.name()
                ),
            });
            continue;
        };

        // PLIO-1: Dedicated power/ground/config pin cannot be assigned to user port
        if !ball.function.is_user_assignable() {
            report.violations.push(IoDrcViolation {
                code: "PLIO-1".into(),
                severity: IoDrcSeverity::Error,
                objects: vec![a.port.clone(), ball.ball.clone()],
                message: format!(
                    "Port '{}' cannot be placed on dedicated {:?} ball '{}'",
                    a.port, ball.function, ball.ball
                ),
            });
            continue;
        }

        if let Some(bank) = ball.bank {
            // Record bank VCCO requirement (UG899: outputs, inouts, DIFF_TERM inputs, and standard inputs require compatible VCCO)
            bank_vcco.entry(bank).or_default().push((
                a.port.clone(),
                std.as_str().to_string(),
                std.required_vcco_mv(),
                a.direction.is_driver(),
            ));
            // Record VREF requirement for SSTL/HSTL inputs/inouts
            if a.direction.is_receiver() {
                if let Some(vref_mv) = std.required_vref_mv() {
                    bank_vref.entry(bank).or_default().push((
                        a.port.clone(),
                        std.as_str().to_string(),
                        vref_mv,
                    ));
                }
            }
            if ball.function == PinFunction::Vref {
                bank_vref_ball_users
                    .entry(bank)
                    .or_default()
                    .push((a.port.clone(), ball.ball.clone()));
            }
        }

        // PLIO-3 & PLIO-4: Differential standard / polarity / mate pairing checks
        if std.is_differential() || a.polarity != DiffPolarity::SingleEnded {
            if !std.is_differential() {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-3".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![a.port.clone(), ball.ball.clone()],
                    message: format!(
                        "Port '{}' has differential polarity {:?} but single-ended IOSTANDARD {}",
                        a.port,
                        a.polarity,
                        std.as_str()
                    ),
                });
            } else {
                match a.polarity {
                    DiffPolarity::SingleEnded => {
                        report.violations.push(IoDrcViolation {
                            code: "PLIO-3".into(),
                            severity: IoDrcSeverity::Error,
                            objects: vec![a.port.clone(), ball.ball.clone()],
                            message: format!(
                                "Differential IOSTANDARD {} on port '{}' requires Positive or Negative polarity",
                                std.as_str(),
                                a.port
                            ),
                        });
                    }
                    DiffPolarity::Positive => {
                        if !ball.function.is_diff_p() {
                            report.violations.push(IoDrcViolation {
                                code: "PLIO-3".into(),
                                severity: IoDrcSeverity::Error,
                                objects: vec![a.port.clone(), ball.ball.clone()],
                                message: format!(
                                    "Differential positive port '{}' must be placed on a _P capable ball (found {:?} at {})",
                                    a.port, ball.function, ball.ball
                                ),
                            });
                        }
                    }
                    DiffPolarity::Negative => {
                        if !ball.function.is_diff_n() {
                            report.violations.push(IoDrcViolation {
                                code: "PLIO-3".into(),
                                severity: IoDrcSeverity::Error,
                                objects: vec![a.port.clone(), ball.ball.clone()],
                                message: format!(
                                    "Differential negative port '{}' must be placed on a _N capable ball (found {:?} at {})",
                                    a.port, ball.function, ball.ball
                                ),
                            });
                        }
                    }
                }

                // PLIO-4: Check mate port & complementary ball pairing
                if a.polarity == DiffPolarity::Positive {
                    match &a.diff_mate_port {
                        None => {
                            report.violations.push(IoDrcViolation {
                                code: "PLIO-4".into(),
                                severity: IoDrcSeverity::Error,
                                objects: vec![a.port.clone()],
                                message: format!(
                                    "Differential positive port '{}' is missing its complementary negative mate port",
                                    a.port
                                ),
                            });
                        }
                        Some(mate_port_name) => {
                            if let Some(&mate_idx) = port_index.get(mate_port_name) {
                                let mate_assign = &plan.assignments[mate_idx];
                                let expected_mate_ball = ball.diff_pair_mate.as_deref();
                                let actual_mate_ball = mate_assign.package_pin.as_deref();
                                if expected_mate_ball.is_none()
                                    || expected_mate_ball != actual_mate_ball
                                    || mate_assign.polarity != DiffPolarity::Negative
                                {
                                    report.violations.push(IoDrcViolation {
                                        code: "PLIO-4".into(),
                                        severity: IoDrcSeverity::Error,
                                        objects: vec![
                                            a.port.clone(),
                                            mate_port_name.clone(),
                                            ball.ball.clone(),
                                        ],
                                        message: format!(
                                            "Differential pair ('{}' at {}, '{}' at {:?}) are not placed on true complementary P/N balls (expected mate {:?})",
                                            a.port,
                                            ball.ball,
                                            mate_port_name,
                                            actual_mate_ball,
                                            expected_mate_ball
                                        ),
                                    });
                                }
                            } else {
                                report.violations.push(IoDrcViolation {
                                    code: "PLIO-4".into(),
                                    severity: IoDrcSeverity::Error,
                                    objects: vec![a.port.clone(), mate_port_name.clone()],
                                    message: format!(
                                        "Differential positive port '{}' references missing mate port '{}'",
                                        a.port, mate_port_name
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        // PLIO-8: Clock-capable pin check
        if a.is_clock_input && !ball.function.is_clock_capable() {
            report.violations.push(IoDrcViolation {
                code: "PLIO-8".into(),
                severity: IoDrcSeverity::Warning,
                objects: vec![a.port.clone(), ball.ball.clone()],
                message: format!(
                    "Clock input port '{}' is placed on non-MRCC/SRCC ball '{}' ({:?}); suboptimal clock routing skew",
                    a.port, ball.ball, ball.function
                ),
            });
        }
    }

    // BIVC-1: Check bank VCCO and VREF compatibility
    for (bank, entries) in &bank_vcco {
        let mut vccos: Vec<u32> = entries.iter().map(|e| e.2).collect();
        vccos.sort_unstable();
        vccos.dedup();
        if vccos.len() > 1 {
            let objs: Vec<String> = std::iter::once(format!("BANK{bank}"))
                .chain(entries.iter().map(|e| e.0.clone()))
                .collect();
            let detail: Vec<String> = entries
                .iter()
                .map(|(p, s, v, _)| format!("{p}({s}={v}mV)"))
                .collect();
            report.violations.push(IoDrcViolation {
                code: "BIVC-1".into(),
                severity: IoDrcSeverity::Error,
                objects: objs,
                message: format!(
                    "Bank VCCO voltage conflict in BANK{bank}: incompatible VCCO levels {:?} mV across ports [{}]",
                    vccos,
                    detail.join(", ")
                ),
            });
        }
    }

    for (bank, vrefs) in &bank_vref {
        let mut levels: Vec<u32> = vrefs.iter().map(|e| e.2).collect();
        levels.sort_unstable();
        levels.dedup();
        if levels.len() > 1 {
            let objs: Vec<String> = std::iter::once(format!("BANK{bank}"))
                .chain(vrefs.iter().map(|e| e.0.clone()))
                .collect();
            report.violations.push(IoDrcViolation {
                code: "BIVC-1".into(),
                severity: IoDrcSeverity::Error,
                objects: objs,
                message: format!(
                    "Bank VREF voltage conflict in BANK{bank}: incompatible VREF levels {:?} mV",
                    levels
                ),
            });
        }
        // PLIO-6: If this bank requires VREF, check that its dedicated VREF pin is not occupied by a user signal
        if let Some(users) = bank_vref_ball_users.get(bank) {
            for (user_port, vref_ball) in users {
                report.violations.push(IoDrcViolation {
                    code: "PLIO-6".into(),
                    severity: IoDrcSeverity::Error,
                    objects: vec![format!("BANK{bank}"), user_port.clone(), vref_ball.clone()],
                    message: format!(
                        "Bank BANK{bank} requires VREF ({} mV) for {}, but dedicated VREF ball {} is occupied by port '{}'",
                        levels[0],
                        vrefs[0].0,
                        vref_ball,
                        user_port
                    ),
                });
            }
        }
    }

    report
}

// ============================================================================
// 3. Simultaneous Switching Noise (SSN) Ground-Bounce Margin Analysis (UG899 report_ssn)
// ============================================================================

/// Per-pin Simultaneous Switching Noise (SSN) analysis row (`report_ssn`).
#[derive(Clone, Debug, PartialEq)]
pub struct PinSsnEntry {
    pub port: String,
    pub ball: String,
    pub bank: u32,
    pub iostandard: String,
    pub direction: IoDirection,
    pub drive_ma: u32,
    pub slew: String,
    /// Self + mutual ground-bounce / supply-droop noise voltage in millivolts (mV).
    pub induced_ssn_mv: f64,
    /// Maximum allowable SSN noise threshold in millivolts (mV) for this `IOSTANDARD`.
    pub noise_budget_mv: f64,
    /// Remaining SSN noise margin percentage: $100 \times (1 - V_{\text{SSN}} / V_{\text{budget}})$.
    /// Values $< 0\%$ indicate an SSN ground-bounce failure.
    pub margin_pct: f64,
    /// Number of nearby simultaneous switching aggressor outputs contributing mutual inductance coupling.
    pub coupled_aggressors: usize,
    /// True if `margin_pct >= 0.0`.
    pub passes: bool,
}

/// Per-bank SSN summary in [`SsnReport`].
#[derive(Clone, Debug, PartialEq)]
pub struct BankSsnSummary {
    pub bank: u32,
    pub vcco_mv: u32,
    pub switching_outputs: usize,
    pub total_pins: usize,
    pub worst_ssn_mv: f64,
    pub worst_margin_pct: f64,
    pub passes: bool,
}

/// Full UG899 Simultaneous Switching Noise (`report_ssn`) report.
#[derive(Clone, Debug, PartialEq)]
pub struct SsnReport {
    pub package: PackageKind,
    pub pins: Vec<PinSsnEntry>,
    pub banks: Vec<BankSsnSummary>,
    pub worst_margin_pct: f64,
    pub failing_pins: usize,
}

impl SsnReport {
    pub fn is_clean(&self) -> bool {
        self.failing_pins == 0
    }

    pub fn pin_entry(&self, port: &str) -> Option<&PinSsnEntry> {
        self.pins.iter().find(|p| p.port == port)
    }

    pub fn bank_summary(&self, bank: u32) -> Option<&BankSsnSummary> {
        self.banks.iter().find(|b| b.bank == bank)
    }

    /// Format a Vivado UG899 `report_ssn` text table.
    pub fn summary_text(&self) -> String {
        let mut s = format!(
            "report_ssn package={} pins={} banks={} worst_margin={:.1}% failing_pins={}\n",
            self.package.name(),
            self.pins.len(),
            self.banks.len(),
            self.worst_margin_pct,
            self.failing_pins
        );
        for b in &self.banks {
            s.push_str(&format!(
                "  BANK{} VCCO={}mV switching_outputs={} worst_ssn={:.1}mV worst_margin={:.1}% status={}\n",
                b.bank,
                b.vcco_mv,
                b.switching_outputs,
                b.worst_ssn_mv,
                b.worst_margin_pct,
                if b.passes { "PASS" } else { "FAIL" }
            ));
        }
        for p in &self.pins {
            s.push_str(&format!(
                "  PIN {} ball={} BANK{} {} drive={}mA slew={} ssn={:.1}mV budget={:.1}mV margin={:.1}% {}\n",
                p.port,
                p.ball,
                p.bank,
                p.iostandard,
                p.drive_ma,
                p.slew,
                p.induced_ssn_mv,
                p.noise_budget_mv,
                p.margin_pct,
                if p.passes { "OK" } else { "VIOLATION" }
            ));
        }
        s
    }
}

/// Compute Euclidean BGA ball grid distance between two balls.
fn ball_distance(a: &BgaBall, b: &BgaBall) -> f64 {
    let dr = a.row_idx as f64 - b.row_idx as f64;
    let dc = a.col as f64 - b.col as f64;
    dr.hypot(dc)
}

/// Distance from `ball` to the nearest `GND` return ball in `pkg`.
fn nearest_gnd_distance(pkg: &BgaPackageGrid, ball: &BgaBall) -> f64 {
    pkg.balls
        .iter()
        .filter(|b| b.function == PinFunction::Gnd)
        .map(|g| ball_distance(ball, g))
        .fold(f64::INFINITY, f64::min)
        .min(6.0)
}

/// Perform Vivado UG899 Simultaneous Switching Noise (SSN) ground-bounce margin analysis (`report_ssn`).
///
/// Models:
/// - Output driver edge rate $dI/dt$ (A/ns) as a function of `IOSTANDARD`, `drive_ma`, `slew` (`FAST` vs `SLOW`),
///   switching frequency, and board capacitive load (`board_load_pf`). Balanced differential pairs (`LVDS_25` /
///   `LVDS_18`) have complementary current cancellation ($0.18\times$ net loop $dI/dt$).
/// - Package return-loop self inductance $L_{\text{self}}$ scaled by distance to the nearest `GND` ball.
/// - Mutual inductive coupling $M_{ij} = k(d_{ij}) \cdot \sqrt{L_i L_j}$ from all other simultaneously switching
///   aggressor outputs in the same bank within BGA coupling radius.
/// - Per-pin ground bounce $\Delta V_{\text{SSN}} = L_{\text{self}} \frac{dI_i}{dt} + \sum_{j \ne i} M_{ij} \frac{dI_j}{dt}$
///   and noise margin percentage $100 \times \left(1 - \frac{\Delta V_{\text{SSN}}}{V_{\text{budget}}}\right)$.
pub fn analyze_ssn(pkg: &BgaPackageGrid, plan: &IoPlan) -> SsnReport {
    #[derive(Clone, Debug)]
    struct ResolvedPin<'a> {
        assign: &'a IoPinAssignment,
        ball: &'a BgaBall,
        bank: u32,
        std: IoStandard,
        drive_ma: u32,
        slew: String,
        l_eff_nh: f64,
        di_dt_a_per_ns: f64,
    }

    let mut resolved = Vec::new();
    for a in &plan.assignments {
        let Some(pin_name) = &a.package_pin else {
            continue;
        };
        let Some(ball) = pkg.pin(pin_name) else {
            continue;
        };
        let Some(bank) = ball.bank else {
            continue;
        };
        let std = IoStandard::parse(&a.iostandard).unwrap_or(IoStandard::Lvcmos18);
        let drive_ma = a.drive_ma.unwrap_or_else(|| {
            if std.is_differential() {
                4
            } else {
                Device::DEFAULT_DRIVE_MA
            }
        });
        let slew = a
            .slew
            .as_deref()
            .unwrap_or(Device::DEFAULT_SLEW)
            .trim()
            .to_ascii_uppercase();

        let gnd_dist = nearest_gnd_distance(pkg, ball);
        // Longer return path to GND increases effective loop inductance
        let l_eff_nh = ball.pkg_inductance_nh * (0.85 + 0.12 * gnd_dist);

        let di_dt_a_per_ns = if a.direction.is_driver() {
            let vcco_v = std.required_vcco_mv() as f64 / 1_000.0;
            let slew_factor = if slew == "FAST" { 1.85 } else { 1.00 };
            let load_factor = (a.board_load_pf.max(1.0) / 10.0).powf(0.35);
            let freq_factor = (a.toggle_rate_mhz.max(1.0) / 100.0).powf(0.15).clamp(0.6, 1.8);
            let diff_cancellation = if std.is_differential() { 0.18 } else { 1.00 };
            // Base dI/dt in mA/ns -> A/ns: e.g. 12 mA driver at 1.8V SLOW into 10 pF is ~0.018 A/ns (18 mV/nH)
            (drive_ma as f64 / 1_000.0)
                * (0.65 + 0.45 * vcco_v)
                * slew_factor
                * load_factor
                * freq_factor
                * diff_cancellation
        } else {
            0.0
        };

        resolved.push(ResolvedPin {
            assign: a,
            ball,
            bank,
            std,
            drive_ma,
            slew,
            l_eff_nh,
            di_dt_a_per_ns,
        });
    }

    let mut pin_entries = Vec::with_capacity(resolved.len());
    let mut bank_groups: BTreeMap<u32, Vec<usize>> = BTreeMap::new();

    for (i, rp) in resolved.iter().enumerate() {
        // Self ground bounce (1 nH * 1 A/ns = 1 V = 1000 mV)
        let self_ssn_mv = 1_000.0 * rp.l_eff_nh * rp.di_dt_a_per_ns;
        let mut mutual_ssn_mv = 0.0;
        let mut coupled_aggressors = 0usize;

        for (j, agg) in resolved.iter().enumerate() {
            if i == j || agg.bank != rp.bank || agg.di_dt_a_per_ns <= 0.0 {
                continue;
            }
            // Complementary P/N pair mates cancel each other's magnetic flux rather than adding
            if rp.assign.diff_mate_port.as_deref() == Some(agg.assign.port.as_str()) {
                continue;
            }
            let dist = ball_distance(rp.ball, agg.ball);
            // Mutual inductive coupling decays with BGA ball separation + shared bank VCCO/GND plane bounce
            let k_spatial = 0.34 / (1.0 + 0.45 * dist);
            let k_bank_plane = 0.065;
            let m_ij_nh = (k_spatial + k_bank_plane) * (rp.l_eff_nh * agg.l_eff_nh).sqrt();
            mutual_ssn_mv += 1_000.0 * m_ij_nh * agg.di_dt_a_per_ns;
            coupled_aggressors += 1;
        }

        let induced_ssn_mv = self_ssn_mv + mutual_ssn_mv;
        let noise_budget_mv = rp.std.ssn_noise_budget_mv();
        let margin_pct = 100.0 * (1.0 - (induced_ssn_mv / noise_budget_mv));
        let passes = margin_pct >= 0.0;

        let entry_idx = pin_entries.len();
        pin_entries.push(PinSsnEntry {
            port: rp.assign.port.clone(),
            ball: rp.ball.ball.clone(),
            bank: rp.bank,
            iostandard: rp.std.as_str().to_string(),
            direction: rp.assign.direction,
            drive_ma: rp.drive_ma,
            slew: rp.slew.clone(),
            induced_ssn_mv,
            noise_budget_mv,
            margin_pct,
            coupled_aggressors,
            passes,
        });
        bank_groups.entry(rp.bank).or_default().push(entry_idx);
    }

    let mut banks = Vec::new();
    for (bank, indices) in bank_groups {
        let vcco_mv = indices
            .first()
            .and_then(|&idx| IoStandard::parse(&pin_entries[idx].iostandard))
            .map(|s| s.required_vcco_mv())
            .unwrap_or(1800);
        let switching_outputs = indices
            .iter()
            .filter(|&&idx| pin_entries[idx].direction.is_driver())
            .count();
        let worst_ssn_mv = indices
            .iter()
            .map(|&idx| pin_entries[idx].induced_ssn_mv)
            .fold(0.0_f64, f64::max);
        let worst_margin_pct = indices
            .iter()
            .map(|&idx| pin_entries[idx].margin_pct)
            .fold(100.0_f64, f64::min);
        let passes = worst_margin_pct >= 0.0;
        banks.push(BankSsnSummary {
            bank,
            vcco_mv,
            switching_outputs,
            total_pins: indices.len(),
            worst_ssn_mv,
            worst_margin_pct,
            passes,
        });
    }

    let worst_margin_pct = pin_entries
        .iter()
        .map(|p| p.margin_pct)
        .fold(100.0_f64, f64::min);
    let failing_pins = pin_entries.iter().filter(|p| !p.passes).count();

    SsnReport {
        package: pkg.kind,
        pins: pin_entries,
        banks,
        worst_margin_pct,
        failing_pins,
    }
}

/// Alias for [`analyze_ssn`] matching the Vivado UG899 Tcl command `report_ssn`.
pub fn report_ssn(pkg: &BgaPackageGrid, plan: &IoPlan) -> SsnReport {
    analyze_ssn(pkg, plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bga_package_grids_match_exact_ball_counts_and_jedec_labels() {
        let dev = Device::load_part("HL10T-C32-1").expect("load HL10T");
        for (kind, expected_balls, expected_banks) in [
            (PackageKind::Cpg236, 236, 4),
            (PackageKind::Csg324, 324, 6),
            (PackageKind::Fgg484, 484, 8),
            (PackageKind::Cabga381, 381, 6),
        ] {
            assert_eq!(PackageKind::parse(kind.name()), Some(kind));
            let grid = BgaPackageGrid::new(kind, Some(&dev));
            assert_eq!(grid.balls.len(), expected_balls, "{} ball count", kind.name());
            assert_eq!(grid.kind.bank_count(), expected_banks);

            // JEDEC forbidden row letters I, O, Q, S, X, Z must never appear
            for ball in &grid.balls {
                assert!(
                    !matches!(ball.row_label.as_str(), "I" | "O" | "Q" | "S" | "X" | "Z"),
                    "illegal JEDEC row label {} on {}",
                    ball.row_label,
                    ball.ball
                );
                assert_eq!(grid.pin(&ball.ball), Some(ball));
            }

            // Every bank must have user pins, a VREF pin, a clock-capable P/N pair, and diff pairs
            for bank in 0..expected_banks {
                let pins = grid.bank_user_pins(bank);
                assert!(!pins.is_empty(), "{} BANK{bank} must have user pins", kind.name());
                assert!(
                    pins.iter().any(|p| p.function == PinFunction::Vref),
                    "{} BANK{bank} must have a VREF pin",
                    kind.name()
                );
                assert!(
                    pins.iter().any(|p| p.function == PinFunction::ClockCapableP),
                    "{} BANK{bank} must have MRCC/SRCC P pin",
                    kind.name()
                );
            }

            // All differential pairs must be symmetric P/N mates in the same bank
            let pairs = grid.differential_pairs();
            assert!(!pairs.is_empty());
            for (p, n) in pairs {
                assert!(p.function.is_diff_p());
                assert!(n.function.is_diff_n());
                assert_eq!(p.bank, n.bank);
                assert_eq!(p.diff_pair_mate.as_deref(), Some(n.ball.as_str()));
                assert_eq!(n.diff_pair_mate.as_deref(), Some(p.ball.as_str()));
            }
        }
    }

    #[test]
    fn bivc1_and_plio_drc_rules_detect_bank_vcco_vref_and_lvds_conflicts() {
        let grid = BgaPackageGrid::new(PackageKind::Csg324, None);
        let bank0_pins = grid.bank_user_pins(0);
        assert!(bank0_pins.len() >= 6);

        // 1. Clean plan in BANK0 (all LVCMOS18 + valid LVDS_18 diff pair)
        let mut clean_plan = IoPlan::new();
        let vref_ball = bank0_pins
            .iter()
            .find(|b| b.function == PinFunction::Vref)
            .unwrap();
        let (diff_p, diff_n) = grid
            .differential_pairs()
            .into_iter()
            .find(|(p, _)| p.bank == Some(0))
            .unwrap();
        clean_plan.add(IoPinAssignment::new(
            "led0",
            Some(&vref_ball.ball),
            "LVCMOS18",
            IoDirection::Output,
        ));
        clean_plan.add_diff_pair(
            "clk_p",
            &diff_p.ball,
            "clk_n",
            &diff_n.ball,
            "LVDS_18",
            IoDirection::Input,
            Some(true),
        );
        let drc_ok = check_io_drc(&grid, &clean_plan);
        assert!(drc_ok.is_clean(), "expected clean DRC: {}", drc_ok.summary_text());

        // 2. BIVC-1: Mix LVCMOS33 (3300 mV) and LVCMOS18 (1800 mV) in BANK0
        let mut conflict_plan = clean_plan.clone();
        conflict_plan.assignments[0].iostandard = "LVCMOS33".into();
        let drc_bivc = check_io_drc(&grid, &conflict_plan);
        assert!(drc_bivc.has_code("BIVC-1"), "must flag BIVC-1 VCCO conflict");

        // 3. PLIO-1 (non-existent / GND ball) & PLIO-2 (duplicate ball)
        let gnd_ball = grid
            .balls
            .iter()
            .find(|b| b.function == PinFunction::Gnd)
            .unwrap();
        let mut bad_loc = IoPlan::new();
        bad_loc.add(IoPinAssignment::new(
            "p_nonexistent",
            Some("Z99"),
            "LVCMOS18",
            IoDirection::Input,
        ));
        bad_loc.add(IoPinAssignment::new(
            "p_on_gnd",
            Some(&gnd_ball.ball),
            "LVCMOS18",
            IoDirection::Output,
        ));
        bad_loc.add(IoPinAssignment::new(
            "dup_a",
            Some(&diff_p.ball),
            "LVCMOS18",
            IoDirection::Output,
        ));
        bad_loc.add(IoPinAssignment::new(
            "dup_b",
            Some(&diff_p.ball),
            "LVCMOS18",
            IoDirection::Output,
        ));
        let drc_loc = check_io_drc(&grid, &bad_loc);
        assert_eq!(drc_loc.by_code("PLIO-1").len(), 2);
        assert_eq!(drc_loc.by_code("PLIO-2").len(), 1);

        // 4. PLIO-3 (swapped P/N polarity) & PLIO-4 (mismatched diff mate)
        let (_, other_diff_n) = grid
            .differential_pairs()
            .into_iter()
            .filter(|(p, _)| p.bank == Some(0))
            .nth(1)
            .unwrap();
        let mut bad_diff = IoPlan::new();
        bad_diff.add_diff_pair(
            "swapped_p",
            &diff_n.ball,       // P placed on N ball -> PLIO-3
            "swapped_n",
            &other_diff_n.ball, // N placed on a different pair's N ball -> PLIO-4
            "LVDS_25",
            IoDirection::Output,
            None,
        );
        let drc_diff = check_io_drc(&grid, &bad_diff);
        assert!(drc_diff.has_code("PLIO-3"));
        assert!(drc_diff.has_code("PLIO-4"));

        // 5. PLIO-6: SSTL15 input in BANK0 when BANK0's VREF ball is occupied by a user output
        let mut vref_conflict = IoPlan::new();
        vref_conflict.add(IoPinAssignment::new(
            "occupy_vref",
            Some(&vref_ball.ball),
            "SSTL15",
            IoDirection::Output,
        ));
        vref_conflict.add(IoPinAssignment::new(
            "ddr_dq0",
            Some(&diff_p.ball),
            "SSTL15",
            IoDirection::Input,
        ));
        let drc_vref = check_io_drc(&grid, &vref_conflict);
        assert!(drc_vref.has_code("PLIO-6"));
    }

    #[test]
    fn ssn_ground_bounce_analyzer_computes_pin_and_bank_margins() {
        let grid = BgaPackageGrid::new(PackageKind::Fgg484, None);
        let bank1_pins = grid.bank_user_pins(1);
        assert!(bank1_pins.len() >= 12);

        // Case A: Low drive (4 mA), SLOW slew, few outputs => generous positive SSN margin
        let mut quiet_plan = IoPlan::new();
        for i in 0..4 {
            let mut a = IoPinAssignment::new(
                format!("q_out{i}"),
                Some(&bank1_pins[i].ball),
                "LVCMOS18",
                IoDirection::Output,
            );
            a.drive_ma = Some(4);
            a.slew = Some("SLOW".into());
            quiet_plan.add(a);
        }
        let quiet_ssn = analyze_ssn(&grid, &quiet_plan);
        assert!(quiet_ssn.is_clean());
        assert!(quiet_ssn.worst_margin_pct > 70.0, "quiet margin={}", quiet_ssn.worst_margin_pct);

        // Case B: High drive (24 mA), FAST slew, heavy capacitive load across many simultaneous outputs => lower margin
        let mut noisy_plan = IoPlan::new();
        for (i, pin) in bank1_pins.iter().take(16).enumerate() {
            let mut a = IoPinAssignment::new(
                format!("bus_out{i}"),
                Some(&pin.ball),
                "LVCMOS18",
                IoDirection::Output,
            );
            a.drive_ma = Some(24);
            a.slew = Some("FAST".into());
            a.board_load_pf = 35.0;
            a.toggle_rate_mhz = 400.0;
            noisy_plan.add(a);
        }
        let noisy_ssn = report_ssn(&grid, &noisy_plan);
        assert!(
            noisy_ssn.worst_margin_pct < quiet_ssn.worst_margin_pct,
            "high drive + FAST slew + 16 aggressors must reduce SSN margin: noisy={} vs quiet={}",
            noisy_ssn.worst_margin_pct,
            quiet_ssn.worst_margin_pct
        );
        assert!(noisy_ssn.summary_text().contains("report_ssn package=FGG484"));
    }
}
