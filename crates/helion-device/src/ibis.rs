//! IBIS (I/O Buffer Information Specification, ANSI/EIA-656-B, version 5.0)
//! model export (`write_ibis`) for a Helion package plan.
//!
//! ## What the models are
//!
//! Each I/O model is a *behavioral* buffer built from the device's own I/O
//! standard data ([`IoStandard`]: VCCO, input thresholds, legal drive
//! strengths) and the per-ball package parasitics of the
//! [`BgaPackageGrid`]:
//!
//! * Pulldown / pullup: a saturating resistive driver,
//!   `I(V) = Isat · tanh(V / (Ron · Isat))`, with `Ron = V_OL / I_drive`
//!   (`V_OL` = 0.4 V for LVCMOS, 20 % of VCCO otherwise) and
//!   `Isat = 2.5 · I_drive`; min/max corners scale current by 0.8 / 1.2.
//!   Fixed-current differential drivers (LVDS/TMDS) use 3.5 mA / 350 mV.
//! * Clamps: ideal diodes (`Is = 1e-14 A`, `n = 1.5`) to GND and VCCO.
//! * Ramp: 20–80 % edge into 50 Ω with a FAST/SLOW edge time.
//! * Package: per-ball R/L/C from the package model; `[Package]` holds the
//!   min/typ/max over the listed pins.
//!
//! These are not silicon-correlated transistor models (the HL10T has no
//! characterized I/O cells); the file says so in `[Notes]`. They are
//! syntactically valid IBIS 5.0 and electrically consistent with the
//! drive/slew/VCCO the design actually uses, which is what board-level
//! pre-layout signal-integrity checks consume.

use crate::io_plan::{BgaPackageGrid, IoStandard, PinFunction};

/// One driven or received design pin in the IBIS `[Pin]` table.
#[derive(Clone, Debug, PartialEq)]
pub struct IbisPin {
    /// Package ball (`A1`).
    pub ball: String,
    /// Design port name.
    pub signal: String,
    pub iostandard: String,
    /// Drive strength in mA (`None` = standard default).
    pub drive_ma: Option<u32>,
    /// `FAST` / `SLOW`.
    pub fast: bool,
}

/// One buffer model emitted into the file.
#[derive(Clone, Debug, PartialEq)]
pub struct IbisModel {
    pub name: String,
    pub std: IoStandard,
    pub drive_ma: u32,
    pub fast: bool,
}

impl IbisModel {
    pub fn for_pin(pin: &IbisPin) -> Result<Self, String> {
        let std = IoStandard::parse(&pin.iostandard).ok_or_else(|| {
            format!(
                "write_ibis: {} has IOSTANDARD {} with no Helion I/O model",
                pin.signal, pin.iostandard
            )
        })?;
        let drive_ma = if std.is_differential() {
            0
        } else {
            let legal = std.legal_drive_ma();
            match pin.drive_ma {
                Some(d) if legal.contains(&d) => d,
                Some(d) => {
                    return Err(format!(
                        "write_ibis: {} DRIVE {d} mA is not legal for {} ({legal:?})",
                        pin.signal,
                        std.as_str()
                    ));
                }
                None => {
                    if legal.contains(&12) {
                        12
                    } else {
                        legal.last().copied().unwrap_or(8)
                    }
                }
            }
        };
        let slew = if pin.fast { "F" } else { "S" };
        let name = if drive_ma == 0 {
            std.as_str().to_string()
        } else {
            format!("{}_{}{}", std.as_str(), drive_ma, slew)
        };
        Ok(Self {
            name,
            std,
            drive_ma,
            fast: pin.fast,
        })
    }

    fn vcc(&self) -> f64 {
        f64::from(self.std.required_vcco_mv()) / 1000.0
    }

    /// Typical driver current (A) and on-resistance (Ω).
    fn driver(&self) -> (f64, f64) {
        if self.std.is_differential() {
            let i = 0.0035;
            (i, 0.35 / i)
        } else {
            let i = f64::from(self.drive_ma) / 1000.0;
            let vol = if self.std.as_str().starts_with("LVCMOS") {
                0.4
            } else {
                0.2 * self.vcc()
            };
            (i, vol / i)
        }
    }

    /// `(Vinl, Vinh)` input thresholds (V).
    pub fn thresholds(&self) -> (f64, f64) {
        let v = self.vcc();
        if let Some(vref) = self.std.required_vref_mv() {
            let vr = f64::from(vref) / 1000.0;
            return (vr - 0.1, vr + 0.1);
        }
        match self.std {
            IoStandard::Lvcmos33 => (0.8, 2.0),
            IoStandard::Lvcmos25 => (0.7, 1.7),
            s if s.is_differential() => (v / 2.0 - 0.1, v / 2.0 + 0.1),
            _ => (0.35 * v, 0.65 * v),
        }
    }

    /// Driver current at `v` volts across the device for corner scale `k`.
    fn drive_current(&self, v: f64, k: f64) -> f64 {
        let (i, ron) = self.driver();
        let isat = 2.5 * i * k;
        isat * (v / (ron / k * isat)).tanh()
    }

    /// 20–80 % edge time (s) for corner scale `k` (faster when stronger).
    fn edge_s(&self, k: f64) -> f64 {
        let base = if self.std.is_differential() {
            0.3e-9
        } else if self.fast {
            0.6e-9
        } else {
            1.6e-9
        };
        // Stronger drivers charge the 50 Ω test load faster.
        let drive_scale = if self.drive_ma == 0 {
            1.0
        } else {
            (12.0 / f64::from(self.drive_ma)).sqrt().clamp(0.6, 1.8)
        };
        base * drive_scale / k
    }
}

/// Corner scales: typ, min (weak), max (strong).
const CORNERS: [f64; 3] = [1.0, 0.8, 1.2];
const TABLE_POINTS: usize = 25;

fn fmt_a(i: f64) -> String {
    format!("{:>11.4e}", i)
}

fn iv_table(out: &mut String, title: &str, vcc: f64, f: impl Fn(f64, f64) -> f64) {
    out.push_str(&format!("[{title}]\n"));
    out.push_str("| Voltage     I(typ)      I(min)      I(max)\n");
    for n in 0..TABLE_POINTS {
        // IBIS range: -VCC .. 2·VCC.
        let v = -vcc + 3.0 * vcc * n as f64 / (TABLE_POINTS - 1) as f64;
        out.push_str(&format!(
            "{:>8.3}V  {} {} {}\n",
            v,
            fmt_a(f(v, CORNERS[0])),
            fmt_a(f(v, CORNERS[1])),
            fmt_a(f(v, CORNERS[2]))
        ));
    }
}

fn diode(v: f64) -> f64 {
    const IS: f64 = 1e-14;
    const NVT: f64 = 1.5 * 0.025_85;
    // Clamp the exponent so the table stays finite (≈ 0.1 A ceiling).
    (IS * ((v / NVT).min(23.0).exp() - 1.0)).min(0.1)
}

/// Render the full IBIS file. `component` names the part, `file_name` the
/// `.ibs` file (lowercase, as IBIS requires).
pub fn write_ibis(
    pkg: &BgaPackageGrid,
    component: &str,
    file_name: &str,
    pins: &[IbisPin],
) -> Result<String, String> {
    if pins.is_empty() {
        return Err("write_ibis: no placed I/O pins to model".into());
    }
    let mut models: Vec<IbisModel> = Vec::new();
    let mut pin_models: Vec<String> = Vec::new();
    for p in pins {
        if pkg.pin(&p.ball).is_none() {
            return Err(format!(
                "write_ibis: {} is on ball {} which is not in package {}",
                p.signal,
                p.ball,
                pkg.kind.name()
            ));
        }
        let m = IbisModel::for_pin(p)?;
        pin_models.push(m.name.clone());
        if !models.iter().any(|x| x.name == m.name) {
            models.push(m);
        }
    }
    let used: Vec<&crate::io_plan::BgaBall> = pins.iter().filter_map(|p| pkg.pin(&p.ball)).collect();
    let stat = |f: &dyn Fn(&crate::io_plan::BgaBall) -> f64| {
        let vals: Vec<f64> = used.iter().map(|b| f(b)).collect();
        let typ = vals.iter().sum::<f64>() / vals.len() as f64;
        let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
        let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (typ, min, max)
    };
    let (rt, rmin, rmax) = stat(&|b| b.pkg_resistance_mohm / 1000.0);
    let (lt, lmin, lmax) = stat(&|b| b.pkg_inductance_nh);
    let (ct, cmin, cmax) = stat(&|b| b.pkg_capacitance_pf);

    let mut out = String::new();
    out.push_str("[IBIS Ver]      5.0\n");
    out.push_str(&format!("[File Name]     {}\n", file_name.to_ascii_lowercase()));
    out.push_str("[File Rev]      1.0\n");
    out.push_str("[Source]        Helion write_ibis from HAD I/O standard data\n");
    out.push_str("[Notes]         Behavioral buffers: saturating-resistor drivers derived\n");
    out.push_str("                from DRIVE/VCCO, ideal-diode clamps, 50 ohm ramps.\n");
    out.push_str("                Not silicon-correlated; use for pre-layout SI only.\n");
    out.push_str("[Disclaimer]    Modeled data, not measured.\n");
    out.push_str(&format!("[Component]     {component}\n"));
    out.push_str("[Manufacturer]  Helion\n");
    out.push_str("[Package]\n");
    out.push_str("| variable  typ          min          max\n");
    out.push_str(&format!("R_pkg       {rt:.4}ohm    {rmin:.4}ohm    {rmax:.4}ohm\n"));
    out.push_str(&format!("L_pkg       {lt:.3}nH      {lmin:.3}nH      {lmax:.3}nH\n"));
    out.push_str(&format!("C_pkg       {ct:.3}pF      {cmin:.3}pF      {cmax:.3}pF\n"));
    out.push_str("[Pin]  signal_name          model_name           R_pin    L_pin    C_pin\n");
    for (p, m) in pins.iter().zip(&pin_models) {
        let b = pkg.pin(&p.ball).expect("checked above");
        let sig: String = p.signal.chars().take(20).collect();
        out.push_str(&format!(
            "{:<6} {:<20} {:<20} {:.3}  {:.2}nH  {:.2}pF\n",
            b.ball,
            sig,
            m,
            b.pkg_resistance_mohm / 1000.0,
            b.pkg_inductance_nh,
            b.pkg_capacitance_pf
        ));
    }
    // Power and ground balls carry the reserved POWER / GND models.
    for b in &pkg.balls {
        let model = match b.function {
            PinFunction::Gnd => "GND",
            PinFunction::Vccint | PinFunction::Vccaux | PinFunction::Vcco => "POWER",
            _ => continue,
        };
        let sig = match b.function {
            PinFunction::Gnd => "GND".to_string(),
            PinFunction::Vccint => "VCCINT".to_string(),
            PinFunction::Vccaux => "VCCAUX".to_string(),
            _ => format!("VCCO_{}", b.bank.unwrap_or(0)),
        };
        out.push_str(&format!("{:<6} {:<20} {}\n", b.ball, sig, model));
    }

    for m in &models {
        let vcc = m.vcc();
        let (vinl, vinh) = m.thresholds();
        out.push('\n');
        out.push_str(&format!("[Model]         {}\n", m.name));
        out.push_str("Model_type      I/O\n");
        out.push_str("Polarity        Non-Inverting\n");
        out.push_str("Enable          Active-High\n");
        out.push_str(&format!("Vinl = {vinl:.3}V\n"));
        out.push_str(&format!("Vinh = {vinh:.3}V\n"));
        out.push_str("Vmeas = ");
        out.push_str(&format!("{:.3}V\n", vcc / 2.0));
        out.push_str("Cref = 0pF\nRref = 50ohm\n");
        out.push_str(&format!("Vref = {:.3}V\n", vcc / 2.0));
        out.push_str("C_comp          4.00pF   3.50pF   4.50pF\n");
        out.push_str(&format!(
            "[Voltage Range] {:.3}V   {:.3}V   {:.3}V\n",
            vcc,
            vcc * 0.95,
            vcc * 1.05
        ));
        iv_table(&mut out, "Pulldown", vcc, |v, k| m.drive_current(v, k));
        // Pullup is referenced to VCC: V = VCC - Vout, current flows out.
        iv_table(&mut out, "Pullup", vcc, |v, k| -m.drive_current(v, k));
        iv_table(&mut out, "GND Clamp", vcc, |v, _| -diode(-v));
        iv_table(&mut out, "POWER Clamp", vcc, |v, _| diode(-v));
        out.push_str("[Ramp]\n");
        out.push_str("| variable  typ              min              max\n");
        let dv = 0.6 * vcc * (50.0 / (50.0 + m.driver().1)).max(0.2);
        let edge = |k: f64| format!("{:.3}/{:.3}n", dv, m.edge_s(k) * 1e9);
        let row = format!(
            "{:<16} {:<16} {}",
            edge(CORNERS[0]),
            edge(CORNERS[1]),
            edge(CORNERS[2])
        );
        out.push_str(&format!("dV/dt_r    {row}\n"));
        out.push_str(&format!("dV/dt_f    {row}\n"));
        out.push_str("R_load = 50ohm\n");
    }
    out.push_str("\n[End]\n");
    if let Some(line) = out.lines().find(|l| l.len() > 80) {
        return Err(format!("write_ibis: line exceeds 80 columns: {line}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, PackageKind};

    fn pkg() -> BgaPackageGrid {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        dev.bga_package(PackageKind::Cpg236)
    }

    fn user_ball(pkg: &BgaPackageGrid, nth: usize) -> String {
        pkg.balls
            .iter()
            .filter(|b| b.function.is_user_assignable())
            .nth(nth)
            .unwrap()
            .ball
            .clone()
    }

    #[test]
    fn writes_a_structurally_valid_ibis_file() {
        let pkg = pkg();
        let pins = vec![
            IbisPin {
                ball: user_ball(&pkg, 0),
                signal: "led".into(),
                iostandard: "LVCMOS33".into(),
                drive_ma: Some(12),
                fast: false,
            },
            IbisPin {
                ball: user_ball(&pkg, 1),
                signal: "clk".into(),
                iostandard: "LVCMOS18".into(),
                drive_ma: None,
                fast: true,
            },
        ];
        let text = write_ibis(&pkg, "HL10T", "hl10t.ibs", &pins).unwrap();
        for kw in [
            "[IBIS Ver]",
            "[Component]",
            "[Package]",
            "[Pin]",
            "[Model]         LVCMOS33_12S",
            "[Model]         LVCMOS18_12F",
            "[Pulldown]",
            "[Pullup]",
            "[GND Clamp]",
            "[POWER Clamp]",
            "[Ramp]",
            "[End]",
        ] {
            assert!(text.contains(kw), "missing {kw}");
        }
        assert!(text.lines().all(|l| l.len() <= 80));
        assert!(text.contains(" GND"), "ground balls use the GND model");
    }

    #[test]
    fn pulldown_is_monotonic_and_corners_bracket_typ() {
        let m = IbisModel::for_pin(&IbisPin {
            ball: "A1".into(),
            signal: "x".into(),
            iostandard: "LVCMOS33".into(),
            drive_ma: Some(8),
            fast: false,
        })
        .unwrap();
        let mut prev = f64::NEG_INFINITY;
        for n in 0..30 {
            let v = -3.3 + 0.33 * n as f64;
            let i = m.drive_current(v, 1.0);
            assert!(i >= prev);
            prev = i;
            if v > 0.0 {
                assert!(m.drive_current(v, 0.8) < i && i < m.drive_current(v, 1.2));
            }
        }
        // Low-voltage slope ≈ 1/Ron with Ron = 0.4 V / 8 mA = 50 Ω.
        let g = m.drive_current(0.01, 1.0) / 0.01;
        assert!((g - 1.0 / 50.0).abs() < 1e-3, "{g}");
        assert!(m.edge_s(1.0) > m.edge_s(1.2));
    }

    #[test]
    fn illegal_drive_and_unknown_standard_are_errors() {
        let pkg = pkg();
        let bad_drive = IbisPin {
            ball: user_ball(&pkg, 0),
            signal: "led".into(),
            iostandard: "LVCMOS12".into(),
            drive_ma: Some(24),
            fast: false,
        };
        assert!(write_ibis(&pkg, "HL10T", "a.ibs", &[bad_drive]).unwrap_err().contains("not legal"));
        let unknown = IbisPin {
            ball: user_ball(&pkg, 0),
            signal: "led".into(),
            iostandard: "POD12".into(),
            drive_ma: None,
            fast: false,
        };
        assert!(write_ibis(&pkg, "HL10T", "a.ibs", &[unknown]).unwrap_err().contains("no Helion I/O model"));
        assert!(write_ibis(&pkg, "HL10T", "a.ibs", &[]).is_err());
    }
}
