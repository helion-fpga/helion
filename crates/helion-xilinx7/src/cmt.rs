//! MMCM and PLL (`MMCME2_ADV` / `PLLE2_ADV`) configuration, written the
//! way Vivado 2017.3 writes it (measured from its ZedBoard bitstreams, see
//! `tools/x7v/tests/cmt_sweep.py`; feature names from the Project X-Ray
//! CMT_TOP_* databases):
//!
//! * Each counter (DIVCLK, CLKFBOUT, CLKOUT0..6; PLL CLKOUT0..5) is
//!   `HIGH_TIME`/`LOW_TIME` (VCO periods, 6 bits, 64 written as 0), `EDGE`
//!   (half a period more high time: odd divides at 50% duty), `NO_COUNT`
//!   (divide 1, written with HIGH = LOW = 1), `DELAY_TIME` + `PHASE_MUX`
//!   (phase in whole and eighth VCO periods). Every counter is written from
//!   its attribute, used or not; `OUTPUT_ENABLE` only on the used outputs and
//!   on CLKFBOUT. The MMCM's CLKOUT5/6 second registers are named
//!   `..._FRACTIONAL_*`, and CLKOUT5's `PHASE_MUX_F` repeats CLKOUT0's
//!   `PHASE_MUX` (CLKOUT6's repeats CLKFBOUT's) for an integer divide.
//! * A fractional CLKFBOUT_MULT_F / CLKOUT0_DIVIDE_F (eighths): see
//!   [`frac_counter`]; its falling-edge words go in CLKOUT6's (CLKFBOUT) or
//!   CLKOUT5's (CLKOUT0) `FRACTIONAL_` register.
//! * `LKTABLE`, `TABLE`, `FILTREG*`, `LOCKREG*`: Vivado's lock and
//!   loop-filter words per feedback multiplier and BANDWIDTH
//!   ([`crate::cmt_tables`]; HIGH equals OPTIMIZED).
//! * `POWER_REG`: see [`power_reg`].
//! * Internal feedback (CLKFBOUT wired straight to CLKFBIN, routed through
//!   the tile's `CLKFBOUT2IN` pip): MMCM `COMP.Z_ZHOLD`, PLL
//!   `COMPENSATION.Z_ZHOLD_OR_CLKIN_BUF`. MMCM feedback through a BUFG:
//!   `COMP.ZHOLD`.
//! * RST, PWRDWN (and the MMCM's PSEN, PSINCDEC) tied low stay on the INT
//!   tile's default VCC and the site inverts them (`ZINV_*` set); CLKINSEL
//!   stays on VCC (CLKIN1); the DRP and phase-shift clocks and data
//!   (DCLK, DEN, DWE, DADDR, DI, PSCLK) and an unused CLKIN2 are tied to
//!   ground through the INT tile's `GFAN`.
//!
//! The fractional, phase and BUFG-feedback words were measured by a Vivado
//! 2017.3 sweep (x7_mmcm, xc7z020clg484-1, 2026-11-26; one property changed
//! per build against MULT_F 10.0 and every divide 10).
//!
//! Refused: a phase off Vivado's 45/DIVIDE grid (fine phase shift is not
//! supported), PLL feedback through a BUFG and every other feedback path
//! (unmeasured), logic-driven RST/PWRDWN/CLKINSEL/DRP/phase-shift pins.

use crate::backend::Placed7;
use crate::pack::{CmtKind, PackedCmt};

/// A pin of a CMT tied to a constant (no fabric net).
fn is_const(n: &Option<String>) -> bool {
    match n.as_deref() {
        None => true,
        Some(s) => matches!(s, "0" | "gnd" | "GND" | "$false" | "const0"),
    }
}

/// `(HIGH_TIME, LOW_TIME, EDGE, NO_COUNT)` for a counter dividing by `div`
/// with `duty` high (XAPP888's encoding; the 6-bit fields write 64 as 0).
pub fn counter(div: u32, duty: f64) -> (u32, u32, bool, bool) {
    if div <= 1 {
        return (1, 1, false, true);
    }
    // High time in half VCO periods, kept within one period of either end.
    let hh = ((duty * div as f64 * 2.0).round() as u32).clamp(2, 2 * div - 2);
    let high = hh / 2;
    (high & 63, (div - high) & 63, hh % 2 == 1, false)
}

/// Phase in eighths of a VCO period for `phase` degrees on a counter of
/// `div` (fractional too): `DELAY_TIME` is `>> 3`, `PHASE_MUX` is `& 7`.
pub fn phase_eighths(phase: f64, div: f64) -> u32 {
    let p = if phase < 0.0 { phase + 360.0 } else { phase };
    (p / 360.0 * div.max(1.0) * 8.0).round() as u32
}

/// The words of a fractional counter `D.f` (f in eighths, `D >= 2`) whose
/// coarse phase is `eighths` (see [`phase_eighths`]). XAPP888's
/// `mmcm_frac_count_calc`; all 50 fractional builds of the Vivado sweep
/// match it.
#[derive(Debug, PartialEq, Eq)]
pub struct Frac {
    pub high: u32,
    pub low: u32,
    pub edge: bool,
    /// `FRAC`, the eighths.
    pub frac: u32,
    pub wf_r: bool,
    /// `FRAC_WF_F` and `PHASE_MUX_F`, in the CLKOUT5/6 `FRACTIONAL_` register.
    pub wf_f: bool,
    pub phase_mux_f: u32,
}

pub fn frac_counter(div: f64, eighths: u32) -> Frac {
    let d = div as u32;
    let f = ((div - d as f64) * 8.0).round() as u32;
    let oaf = 8 * (d & 1) + f;
    Frac {
        high: (d >> 1) - (oaf <= 8) as u32,
        low: (d >> 1) - (oaf <= 9) as u32,
        edge: oaf >= 9,
        frac: f,
        wf_r: (1..=8).contains(&oaf),
        wf_f: (2..=9).contains(&oaf) || (f == 1 && d == 2),
        phase_mux_f: ((eighths & 7) + (oaf >> 1)) & 7,
    }
}

/// CLKOUT0's divide (`CLKOUT0_DIVIDE_F`, fractional on an MMCM).
fn div0(c: &PackedCmt) -> f64 {
    if c.clkout0_divide_f > 0.0 { c.clkout0_divide_f } else { c.clkout_divide[0] as f64 }
}

/// Every counter as `(name, divide, phase)`: CLKFBOUT, then CLKOUT0..
fn phased(c: &PackedCmt) -> Vec<(String, f64, f64)> {
    let n_out = if c.kind == CmtKind::MmcmE2Adv { 7 } else { 6 };
    std::iter::once(("CLKFBOUT".to_string(), c.clkfbout_mult_f, c.clkfbout_phase))
        .chain((0..n_out).map(|i| (format!("CLKOUT{i}"), if i == 0 { div0(c) } else { c.clkout_divide[i] as f64 }, c.clkout_phase[i])))
        .collect()
}

/// Vivado's POWER_REG: an MMCM writes 0x9900 when some counter (CLKFBOUT
/// included) is fractional or its phase needs an odd quarter of a VCO
/// period (PHASE_MUX not 0 or 4), else 0x0100; a PLL writes 0. Measured
/// over 22 Vivado MMCMs (sweep_pwr*, sweep_divs, ref_mmcm): PHASE_MUX 1, 2,
/// 5, 6, 7 on any output gave 0x9900; 0 and 4, any outputs and divides,
/// gave 0x0100; the x7_mmcm sweep: every fractional build, and CLKFBOUT
/// at 45 degrees (PHASE_MUX 2), gave 0x9900.
pub fn power_reg(c: &PackedCmt) -> u32 {
    if c.kind == CmtKind::PllE2Adv {
        return 0;
    }
    let odd = phased(c).iter().any(|(_, d, p)| d.fract() != 0.0 || phase_eighths(*p, *d) % 4 != 0);
    if odd {
        0x9900
    } else {
        0x0100
    }
}

/// Lock and loop-filter words for `c` (see [`crate::cmt_tables`]).
fn tables(c: &PackedCmt) -> Result<[u64; 7], String> {
    use crate::cmt_tables::*;
    // A fractional multiplier uses its integer part's row (x7_mmcm sweep:
    // 6.125 and 6.5 wrote the 6 row, 7.5 the 7, 11.5 and 11.875 the 11).
    let m = c.clkfbout_mult_f as u32;
    let rows: &[(u32, [u64; 7])] = match (c.kind, c.bandwidth.as_str()) {
        (CmtKind::MmcmE2Adv, "LOW") => &MMCM_LOW,
        (CmtKind::MmcmE2Adv, _) => &MMCM_OPTIMIZED,
        (CmtKind::PllE2Adv, "LOW") => &PLL_LOW,
        (CmtKind::PllE2Adv, _) => &PLL_OPTIMIZED,
    };
    rows.iter()
        .find(|(k, _)| *k == m)
        .map(|r| r.1)
        .ok_or_else(|| format!("cmt: {}: no measured lock/filter table for CLKFBOUT_MULT {m}", c.name))
}

/// `Some(false)` for internal feedback (CLKFBOUT wired straight to
/// CLKFBIN), `Some(true)` for CLKFBOUT -> BUFG -> CLKFBIN, else `None`.
fn bufg_feedback(c: &PackedCmt, bufgs: &[crate::pack::PackedBufg]) -> Option<bool> {
    let (o, i) = (c.clkfbout_net.as_ref()?, c.clkfbin_net.as_ref()?);
    if o == i {
        return Some(false);
    }
    bufgs.iter().any(|b| b.in_net == *o && b.out_net == *i).then_some(true)
}

/// Everything the flow does not support, as one error.
fn check(c: &PackedCmt, bufgs: &[crate::pack::PackedBufg]) -> Result<(), String> {
    let n = &c.name;
    let mmcm = c.kind == CmtKind::MmcmE2Adv;
    for (name, d, p) in phased(c) {
        if d.fract() != 0.0 {
            if !mmcm || !(name == "CLKFBOUT" || name == "CLKOUT0") {
                return Err(format!("cmt: {n}: {name} divide {d} must be an integer"));
            }
            if (d * 8.0).fract() != 0.0 || d < 2.0 {
                return Err(format!("cmt: {n}: fractional {name} {d} must be at least 2 in steps of 0.125"));
            }
            if name == "CLKOUT0" && c.clkout_duty[0] != 0.5 {
                return Err(format!("cmt: {n}: a fractional CLKOUT0_DIVIDE_F needs CLKOUT0_DUTY_CYCLE 0.5"));
            }
        }
        // Vivado DRC AVAL-139: without fine phase shift the phase steps by
        // an eighth of a VCO period, 45/DIVIDE degrees.
        let steps = p * d / 45.0;
        if (steps - steps.round()).abs() > 1e-6 {
            return Err(format!(
                "cmt: {n}: {name}_PHASE {p} is not a multiple of 45/{d} degrees (Vivado DRC AVAL-139; fine phase shift, USE_FINE_PS, is not supported)"
            ));
        }
    }
    match bufg_feedback(c, bufgs) {
        Some(false) => {}
        Some(true) if mmcm => {}
        _ => {
            return Err(format!(
                "cmt: {n}: feedback must be internal (CLKFBOUT wired straight to CLKFBIN) or, on an MMCM, through one BUFG; other paths are not measured"
            ))
        }
    }
    if c.clkin1_net.is_none() {
        return Err(format!("cmt: {n}: CLKIN1 is not connected"));
    }
    for (pin, net) in [("RST", &c.rst_net), ("PWRDWN", &c.pwrdwn_net), ("CLKIN2", &c.clkin2_net)] {
        if !is_const(net) {
            return Err(format!("cmt: {n}: {pin} driven by logic is not supported yet (only tied low)"));
        }
    }
    Ok(())
}

/// FASM lines for every placed CMT's site configuration.
pub fn emit_lines(placed: &Placed7) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for pc in &placed.cmts {
        let c = placed
            .packed
            .cmts
            .iter()
            .find(|c| c.name == pc.name)
            .ok_or_else(|| format!("cmt: placed {} is not packed", pc.name))?;
        check(c, &placed.packed.bufgs)?;
        let mmcm = c.kind == CmtKind::MmcmE2Adv;
        let p = format!("{}.{}", pc.tile, if mmcm { "MMCME2_ADV" } else { "PLLE2_ADV" });
        let mut f = |s: String| lines.push(format!("{p}.{s}"));
        f("IN_USE".into());
        let via_bufg = bufg_feedback(c, &placed.packed.bufgs) == Some(true);
        f(match (mmcm, via_bufg) {
            (true, true) => "COMP.ZHOLD",
            (true, false) => "COMP.Z_ZHOLD",
            _ => "COMPENSATION.Z_ZHOLD_OR_CLKIN_BUF",
        }
        .into());
        for z in if mmcm { &["ZINV_PSEN", "ZINV_PSINCDEC", "ZINV_PWRDWN", "ZINV_RST"][..] } else { &["ZINV_PWRDWN", "ZINV_RST"][..] } {
            f((*z).into());
        }
        // name, divide, duty, phase, register-2 prefix, output enabled
        let mut counters: Vec<(String, f64, f64, f64, String, bool)> = Vec::new();
        for (k, (name, div, phase)) in phased(c).into_iter().enumerate() {
            let (duty, r2, used) = match k {
                0 => (0.5, "CLKFBOUT_CLKOUT2_".to_string(), true),
                _ => {
                    let i = k - 1;
                    let r2 = if mmcm && i >= 5 { format!("CLKOUT{i}_CLKOUT2_FRACTIONAL_") } else { format!("CLKOUT{i}_CLKOUT2_") };
                    (c.clkout_duty[i], r2, c.clkout_nets[i].is_some())
                }
            };
            counters.push((name, div, duty, phase, r2, used));
        }
        // CLKOUT0's and CLKFBOUT's (PHASE_MUX_F, FRAC_WF_F) for CLKOUT5/6.
        let mut second = [(0u32, false); 2];
        for (name, div, duty, phase, r2, used) in &counters {
            let ph = phase_eighths(*phase, *div);
            let frac = (div.fract() != 0.0).then(|| frac_counter(*div, ph));
            let (hi, lo, edge, nc) = match &frac {
                Some(fr) => (fr.high, fr.low, fr.edge, false),
                None => counter(*div as u32, *duty),
            };
            f(format!("{name}_CLKOUT1_HIGH_TIME[5:0] = 6'd{hi}"));
            f(format!("{name}_CLKOUT1_LOW_TIME[5:0] = 6'd{lo}"));
            if ph % 8 != 0 {
                f(format!("{name}_CLKOUT1_PHASE_MUX[2:0] = 3'd{}", ph % 8));
            }
            if ph / 8 != 0 {
                f(format!("{r2}DELAY_TIME[5:0] = 6'd{}", (ph / 8) & 63));
            }
            if edge {
                f(format!("{r2}EDGE[0]"));
            }
            if nc {
                f(format!("{r2}NO_COUNT[0]"));
            }
            if let Some(fr) = &frac {
                f(format!("{r2}FRAC[2:0] = 3'd{}", fr.frac));
                f(format!("{r2}FRAC_EN[0]"));
                if fr.wf_r {
                    f(format!("{r2}FRAC_WF_R[0]"));
                }
            }
            if *used {
                f(format!("{name}_CLKOUT1_OUTPUT_ENABLE[0]"));
            }
            let words = frac.as_ref().map_or((ph % 8, false), |fr| (fr.phase_mux_f, fr.wf_f));
            match name.as_str() {
                "CLKOUT0" => second[0] = words,
                "CLKFBOUT" => second[1] = words,
                _ => {}
            }
        }
        if mmcm {
            for (i, (pmux_f, wf_f)) in [(5, second[0]), (6, second[1])] {
                if pmux_f != 0 {
                    f(format!("CLKOUT{i}_CLKOUT2_FRACTIONAL_PHASE_MUX_F[2:0] = 3'd{pmux_f}"));
                }
                if wf_f {
                    f(format!("CLKOUT{i}_CLKOUT2_FRACTIONAL_FRAC_WF_F[0]"));
                }
            }
        }
        let d = c.divclk_divide;
        let (hi, lo, edge, nc) = counter(d, 0.5);
        f(format!("DIVCLK_DIVCLK_HIGH_TIME[5:0] = 6'd{hi}"));
        f(format!("DIVCLK_DIVCLK_LOW_TIME[5:0] = 6'd{lo}"));
        if edge {
            f("DIVCLK_DIVCLK_EDGE[0]".into());
        }
        if nc {
            f("DIVCLK_DIVCLK_NO_COUNT[0]".into());
        }
        let t = tables(c)?;
        for ((name, width), v) in [("LKTABLE", 40), ("TABLE", 10), ("FILTREG1_RESERVED", 12), ("FILTREG2_RESERVED", 10),
            ("LOCKREG1_RESERVED", 6), ("LOCKREG2_RESERVED", 1), ("LOCKREG3_RESERVED", 1)].iter().zip(t)
        {
            if v != 0 {
                f(format!("{name}[{}:0] = {width}'h{v:X}", width - 1));
            }
        }
        let pwr = power_reg(c);
        if pwr != 0 {
            f(format!("POWER_REG_POWER_REG_POWER_REG[15:0] = 16'h{pwr:04X}"));
        }
    }
    Ok(lines)
}

/// Routing endpoints of every CMT's fabric pins: LOCKED (a driver). The
/// clock pins are added by the router with the clock nets.
pub fn endpoints(placed: &Placed7) -> Vec<(String, String, String, bool)> {
    let mut out = Vec::new();
    for pc in &placed.cmts {
        let Some(c) = placed.packed.cmts.iter().find(|c| c.name == pc.name) else { continue };
        if let Some(n) = &c.locked_net {
            out.push((n.clone(), pc.site.clone(), "LOCKED".to_string(), true));
        }
    }
    out
}

/// FASM lines holding every CMT's unused configuration pins at their
/// constants (see the module doc): ground on the DRP and phase-shift
/// clock/data pins and an unused CLKIN2, the INT tile's default VCC (checked
/// free) on CLKINSEL and the inverted active-high pins.
pub fn tie_lines(
    placed: &Placed7,
    nets: &[crate::route::RoutedNet7],
    device: &crate::device::Device,
    rq: &dyn crate::route::RouteQuery,
) -> Result<Vec<String>, String> {
    let routed = crate::bram::routed_wires(nets);
    let mut lines = Vec::new();
    for pc in &placed.cmts {
        let Some(c) = placed.packed.cmts.iter().find(|c| c.name == pc.name) else { continue };
        let mmcm = c.kind == CmtKind::MmcmE2Adv;
        let mut clocks: Vec<&str> = vec!["DCLK", "CLKIN2"];
        if mmcm {
            clocks.push("PSCLK");
        }
        for pin in clocks {
            lines.extend(tie_clock_low(device, rq, &routed, &pc.site, pin)?);
        }
        let mut zero: Vec<String> = vec!["DEN".into(), "DWE".into()];
        zero.extend((0..7).map(|i| format!("DADDR{i}")));
        zero.extend((0..16).map(|i| format!("DI{i}")));
        let mut one: Vec<&str> = vec!["CLKINSEL", "RST", "PWRDWN"];
        if mmcm {
            one.extend(["PSEN", "PSINCDEC"]);
        }
        for pin in &zero {
            lines.extend(crate::bram::tie(device, rq, &routed, &pc.site, pin, false, pin)?);
        }
        for pin in one {
            lines.extend(crate::bram::tie(device, rq, &routed, &pc.site, pin, true, pin)?);
        }
    }
    lines.sort();
    lines.dedup();
    Ok(lines)
}

/// Ground a CMT clock pin (DCLK, PSCLK, unused CLKIN2) the way Vivado
/// does: the CMT tile's `*_INT` input pip where there is a choice, the INT
/// tile's `CLKn <- FAN_BOUNCEk <- FAN_ALTk <- GFANg <- GND_WIRE` (Vivado:
/// FAN_BOUNCE5 / GFAN1; another free one when that is routed).
fn tie_clock_low(
    device: &crate::device::Device,
    rq: &dyn crate::route::RouteQuery,
    routed: &std::collections::HashSet<(String, String)>,
    site: &str,
    pin: &str,
) -> Result<Vec<String>, String> {
    let err = |m: &str| format!("cmt: tie {site}.{pin} low: {m}");
    let (ti, si) = device.site_by_name(site).ok_or_else(|| err("no such site"))?;
    let mut tile = device.tile(ti).map(|t| t.name.clone()).unwrap_or_default();
    let mut wire = device
        .site_pins(ti, si)
        .and_then(|ps| ps.iter().find(|(n, _)| n == pin).map(|(_, w)| w.clone()))
        .ok_or_else(|| err("no pin wire"))?;
    let mut path = Vec::new();
    for _ in 0..4 {
        let int = std::iter::once((tile.clone(), wire.clone())).chain(rq.node_wires(&tile, &wire)).find(|(t, _)| {
            device.tile_by_name(t).and_then(|i| device.tile_type_name(i)).is_some_and(|ty| ty == "INT_L" || ty == "INT_R")
        });
        if let Some((it, iw)) = int {
            path.extend(crate::bram::ground_via_fan(device, routed, &it, &iw).ok_or_else(|| err(&format!("no free FAN_BOUNCE/GFAN path in {it} for {iw}")))?);
            return Ok(path);
        }
        let ti = device.tile_by_name(&tile).ok_or_else(|| err("no tile"))?;
        let ty = device.tile_type_name(ti).unwrap_or("").to_string();
        let names = device.wires(ti).ok_or_else(|| err("no wires"))?;
        let srcs: Vec<String> = device.pips_into(ti, &wire).into_iter().map(|p| names[p.src as usize].clone()).collect();
        let always = |src: &str| {
            device.ppips(&ty).is_some_and(|ps| {
                ps.iter().any(|pp| pp.name == format!("{ty}.{wire}.{src}") && pp.kind == crate::db::parse::PpipKind::Always)
            })
        };
        let next = if let Some(s) = srcs.iter().find(|s| always(s)) {
            s.clone()
        } else {
            let s = srcs.iter().find(|s| s.ends_with("_INT")).ok_or_else(|| err(&format!("no INT input into {tile}.{wire}")))?;
            path.push(format!("{tile}.{wire}.{s}"));
            s.clone()
        };
        wire = next;
        tile = tile.clone();
    }
    Err(err("no INT tile within 4 hops"))
}

/// The dedicated path from a clock source wire to a CMT clock input
/// (CLKIN1/CLKIN2): breadth-first over CMT, HCLK_CMT and I/O tile pips,
/// never through INT tiles or the CMT frequency backbone (`*FREQ*` wires,
/// which need enables Vivado's builds do not show). Vivado's ZedBoard build
/// takes pad -> `HCLK_CMT_CCIO0` -> `HCLK_CMT_MUX_MMCM_CLKIN1` ->
/// `CMT_*_CLK_IN1_HCLK` -> CLKIN1. `None` when there is no such path.
pub fn dedicated_clock_route(
    rq: &dyn crate::route::RouteQuery,
    src: (&str, &str),
    sinks: &[(String, String)],
) -> Option<Vec<crate::route::RoutedPip>> {
    use std::collections::{HashMap, VecDeque};
    // Never through a global buffer: its I0 -> O pip is a site route-through
    // that nothing configures (and a placed BUFG may own the site).
    let ok = |t: &str, w: &str| !t.starts_with("INT_") && !w.contains("FREQ") && !(w.contains("BUFGCTRL") && w.ends_with("_O"));
    // (tile, wire) -> pip that reached it (None for the source node).
    let mut prev: HashMap<(String, String), Option<crate::route::RoutedPip>> = HashMap::new();
    let mut q = VecDeque::new();
    let start = (src.0.to_string(), src.1.to_string());
    for (t, w) in std::iter::once(start.clone()).chain(rq.node_wires(src.0, src.1)) {
        prev.insert((t.clone(), w.clone()), None);
        q.push_back((t, w));
    }
    while let Some((t, w)) = q.pop_front() {
        if sinks.iter().any(|(st, sw)| *st == t && *sw == w) {
            let mut path = Vec::new();
            let mut cur = (t, w);
            while let Some(Some(p)) = prev.get(&cur) {
                let back = (p.tile.clone(), p.src_wire.clone());
                path.push(p.clone());
                cur = back;
            }
            path.reverse();
            return Some(path);
        }
        if prev.len() > 20_000 {
            return None;
        }
        for (dst, srcw, pseudo) in rq.pips_from(&t, &w) {
            if !ok(&t, &dst) {
                continue;
            }
            let pip = crate::route::RoutedPip { tile: t.clone(), dst_wire: dst.clone(), src_wire: srcw, is_pseudo: pseudo };
            for (nt, nw) in std::iter::once((t.clone(), dst.clone())).chain(rq.node_wires(&t, &dst)) {
                if !ok(&nt, &nw) || prev.contains_key(&(nt.clone(), nw.clone())) {
                    continue;
                }
                // Node siblings are reached through the same pip.
                prev.insert((nt.clone(), nw.clone()), Some(pip.clone()));
                q.push_back((nt, nw));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counter words Vivado wrote in the ZedBoard builds (x7v decode of
    /// ref_mmcm, sweep_divs, the table sweep).
    #[test]
    fn counters_match_vivado() {
        assert_eq!(counter(1, 0.5), (1, 1, false, true));
        assert_eq!(counter(2, 0.5), (1, 1, false, false));
        assert_eq!(counter(3, 0.5), (1, 2, true, false));
        assert_eq!(counter(4, 0.25), (1, 3, false, false));
        assert_eq!(counter(7, 0.5), (3, 4, true, false));
        assert_eq!(counter(8, 0.75), (6, 2, false, false));
        assert_eq!(counter(10, 0.5), (5, 5, false, false));
        assert_eq!(counter(16, 0.25), (4, 12, false, false));
        assert_eq!(counter(40, 0.5), (20, 20, false, false));
        assert_eq!(counter(128, 0.5), (0, 0, false, false));
        assert_eq!(phase_eighths(50.625, 40.0), 45);
        assert_eq!(phase_eighths(345.0, 3.0), 23);
        assert_eq!(phase_eighths(90.0, 13.0), 26);
        assert_eq!(phase_eighths(22.5, 2.0), 1);
    }

    /// The x7_mmcm sweep's base: MULT_F 10.0, every divide 10, internal feedback.
    fn base() -> PackedCmt {
        PackedCmt {
            name: "u".into(),
            clkin1_period_ns: 10.0,
            divclk_divide: 1,
            clkfbout_mult_f: 10.0,
            clkout0_divide_f: 10.0,
            clkout_divide: [10; 7],
            clkout_duty: [0.5; 7],
            clkin1_net: Some("ci".into()),
            clkfbout_net: Some("fb".into()),
            clkfbin_net: Some("fb".into()),
            ..Default::default()
        }
    }

    /// Fractional counters and coarse phase, rows of decoded.tsv (Vivado
    /// 2017.3, x7_mmcm sweep, xc7z020clg484-1, 2026-11-26): fb_* rows are
    /// CLKFBOUT_MULT_F (falling-edge words in CLKOUT6), o0_* CLKOUT0_DIVIDE_F
    /// (in CLKOUT5).
    #[test]
    fn fractional_counters_match_vivado() {
        // (D.f, phase, HIGH, LOW, EDGE, FRAC, WF_R, WF_F, PHASE_MUX_F, DELAY_TIME, PHASE_MUX)
        let rows: &[(f64, f64, u32, u32, bool, u32, bool, bool, u32, u32, u32)] = &[
            (10.125, 0.0, 4, 4, false, 1, true, false, 0, 0, 0), // fb_10_125, o0_10_125
            (10.25, 0.0, 4, 4, false, 2, true, true, 1, 0, 0),   // fb_10_250
            (10.5, 0.0, 4, 4, false, 4, true, true, 2, 0, 0),    // fb_10_500
            (10.875, 0.0, 4, 4, false, 7, true, true, 3, 0, 0),  // fb_10_875
            (6.125, 0.0, 2, 2, false, 1, true, false, 0, 0, 0),  // fb_6_125
            (6.5, 0.0, 2, 2, false, 4, true, true, 2, 0, 0),     // fb_6_500
            (7.5, 0.0, 3, 3, true, 4, false, false, 6, 0, 0),    // fb_7_500
            (9.5, 0.0, 4, 4, true, 4, false, false, 6, 0, 0),    // fb_9_500
            (11.5, 0.0, 5, 5, true, 4, false, false, 6, 0, 0),   // fb_11_500
            (11.875, 0.0, 5, 5, true, 7, false, false, 7, 0, 0), // fb_11_875
            (2.125, 0.0, 0, 0, false, 1, true, true, 0, 0, 0),   // o0_2_125: WF_F by the D=2, f=1 case
            (2.875, 0.0, 0, 0, false, 7, true, true, 3, 0, 0),   // o0_2_875
            (3.125, 0.0, 1, 0, true, 1, false, true, 4, 0, 0),   // o0_3_125
            (3.25, 0.0, 1, 1, true, 2, false, false, 5, 0, 0),   // o0_3_250
            (3.875, 0.0, 1, 1, true, 7, false, false, 7, 0, 0),  // o0_3_875
            (64.5, 0.0, 31, 31, false, 4, true, true, 2, 0, 0),  // o0_64_500
            (127.5, 0.0, 63, 63, true, 4, false, false, 6, 0, 0), // o0_127_500
            (10.5, 90.0, 4, 4, false, 4, true, true, 7, 2, 5),   // o0_10_500_ph9000, fb_10_500_ph9000
            (10.5, 180.0, 4, 4, false, 4, true, true, 4, 5, 2),  // o0_10_500_ph18000, fb_10_500_ph18000
            (10.5, 270.0, 4, 4, false, 4, true, true, 1, 7, 7),  // o0_10_500_ph27000
        ];
        for &(d, p, high, low, edge, frac, wf_r, wf_f, phase_mux_f, delay, mux) in rows {
            let e = phase_eighths(p, d);
            assert_eq!((e >> 3, e & 7), (delay, mux), "phase {d} @ {p}");
            assert_eq!(frac_counter(d, e), Frac { high, low, edge, frac, wf_r, wf_f, phase_mux_f }, "{d} @ {p}");
        }
        // Integer coarse phase (o0_10_000_ph*, fb_10_000_ph*): eighths 10, 20, 40, 60.
        for (p, e) in [(45.0, 10), (90.0, 20), (180.0, 40), (270.0, 60)] {
            assert_eq!(phase_eighths(p, 10.0), e);
        }
    }

    /// POWER_REG 0x9900 for any fractional counter and for CLKFBOUT's own
    /// odd-quarter phase (fb_10_000_ph4500); 90 degrees on 10 (PHASE_MUX 4,
    /// o0_10_000_ph9000) stays 0x0100.
    #[test]
    fn power_reg_fractional_and_feedback_phase() {
        assert_eq!(power_reg(&base()), 0x0100);
        assert_eq!(power_reg(&PackedCmt { clkfbout_mult_f: 10.125, ..base() }), 0x9900);
        assert_eq!(power_reg(&PackedCmt { clkout0_divide_f: 64.5, ..base() }), 0x9900);
        assert_eq!(power_reg(&PackedCmt { clkfbout_phase: 45.0, ..base() }), 0x9900);
        let mut c = base();
        c.clkout_phase[0] = 90.0;
        assert_eq!(power_reg(&c), 0x0100);
    }

    /// LKTABLE / TABLE of a fractional multiplier: the integer part's row
    /// (fb_6_500 = fb_6_125 = fb_int_6, fb_7_500, fb_11_875).
    #[test]
    fn fractional_mult_uses_integer_part_table() {
        let t = |m: f64| tables(&PackedCmt { clkfbout_mult_f: m, ..base() }).unwrap();
        assert_eq!(t(10.0)[..2], [993187046401, 980]);
        assert_eq!(t(6.0)[..2], [603418764289, 940]);
        assert_eq!(t(6.5)[..2], [603418764289, 940]);
        assert_eq!(t(6.125)[..2], [603418764289, 940]);
        assert_eq!(t(7.5)[..2], [674285724673, 948]);
        assert_eq!(t(11.875)[..2], [1099382629377, 996]);
    }

    #[test]
    fn check_accepts_measured_and_refuses_the_rest() {
        let bufg = |i: &str, o: &str| crate::pack::PackedBufg { in_net: i.into(), out_net: o.into(), ..Default::default() };
        assert!(check(&PackedCmt { clkfbout_mult_f: 10.5, clkout0_divide_f: 2.125, ..base() }, &[]).is_ok());
        assert!(check(&PackedCmt { clkfbout_phase: 45.0, ..base() }, &[]).is_ok());
        // 90 degrees on 10.5 is 21 eighths: on the grid.
        assert!(check(&PackedCmt { clkout0_divide_f: 10.5, clkout_phase: [90.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], ..base() }, &[]).is_ok());
        // AVAL-139: 10 degrees is not a multiple of 45/10.
        let e = check(&PackedCmt { clkfbout_phase: 10.0, ..base() }, &[]).unwrap_err();
        assert!(e.contains("AVAL-139"), "{e}");
        let e = check(&PackedCmt { clkout0_divide_f: 10.1, ..base() }, &[]).unwrap_err();
        assert!(e.contains("0.125"), "{e}");
        let e = check(&PackedCmt { clkout0_divide_f: 10.5, clkout_duty: [0.4, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5], ..base() }, &[]).unwrap_err();
        assert!(e.contains("DUTY"), "{e}");
        // Feedback through one BUFG: MMCM yes (COMP.ZHOLD), PLL unmeasured.
        let fb = PackedCmt { clkfbin_net: Some("fb_buf".into()), ..base() };
        assert_eq!(bufg_feedback(&fb, &[bufg("fb", "fb_buf")]), Some(true));
        assert!(check(&fb, &[bufg("fb", "fb_buf")]).is_ok());
        assert!(check(&fb, &[]).is_err());
        assert!(check(&PackedCmt { kind: CmtKind::PllE2Adv, ..fb.clone() }, &[bufg("fb", "fb_buf")]).is_err());
        assert!(check(&PackedCmt { kind: CmtKind::PllE2Adv, clkout0_divide_f: 10.5, ..base() }, &[]).is_err());
    }
}
