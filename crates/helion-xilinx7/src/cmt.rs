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
//!   `PHASE_MUX` (CLKOUT6's repeats CLKFBOUT's).
//! * `LKTABLE`, `TABLE`, `FILTREG*`, `LOCKREG*`: Vivado's lock and
//!   loop-filter words per feedback multiplier and BANDWIDTH
//!   ([`crate::cmt_tables`]; HIGH equals OPTIMIZED).
//! * `POWER_REG`: see [`power_reg`].
//! * Internal feedback (CLKFBOUT wired straight to CLKFBIN, routed through
//!   the tile's `CLKFBOUT2IN` pip): MMCM `COMP.Z_ZHOLD`, PLL
//!   `COMPENSATION.Z_ZHOLD_OR_CLKIN_BUF`.
//! * RST, PWRDWN (and the MMCM's PSEN, PSINCDEC) tied low stay on the INT
//!   tile's default VCC and the site inverts them (`ZINV_*` set); CLKINSEL
//!   stays on VCC (CLKIN1); the DRP and phase-shift clocks and data
//!   (DCLK, DEN, DWE, DADDR, DI, PSCLK) and an unused CLKIN2 are tied to
//!   ground through the INT tile's `GFAN`.
//!
//! Refused (unmeasured): fractional dividers, CLKFBOUT phase, feedback
//! through a BUFG, logic-driven RST/PWRDWN/CLKINSEL/DRP/phase-shift pins.

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

/// Phase in eighths of a VCO period for `phase` degrees on a counter of `div`.
pub fn phase_eighths(phase: f64, div: u32) -> u32 {
    let p = if phase < 0.0 { phase + 360.0 } else { phase };
    (p / 360.0 * div.max(1) as f64 * 8.0).round() as u32
}

/// Vivado's POWER_REG: an MMCM writes 0x9900 when some counter's phase
/// needs an odd quarter of a VCO period (PHASE_MUX not 0 or 4), else
/// 0x0100; a PLL writes 0. Measured over 22 Vivado MMCMs (sweep_pwr*,
/// sweep_divs, ref_mmcm): PHASE_MUX 1, 2, 5, 6, 7 on any output gave 0x9900;
/// 0 and 4, any outputs and divides, gave 0x0100.
pub fn power_reg(c: &PackedCmt) -> u32 {
    if c.kind == CmtKind::PllE2Adv {
        return 0;
    }
    let n = 7;
    let odd = (0..n).any(|i| phase_eighths(c.clkout_phase[i], c.clkout_divide[i]) % 4 != 0);
    if odd {
        0x9900
    } else {
        0x0100
    }
}

/// Lock and loop-filter words for `c` (see [`crate::cmt_tables`]).
fn tables(c: &PackedCmt) -> Result<[u64; 7], String> {
    use crate::cmt_tables::*;
    let m = c.clkfbout_mult_f.round() as u32;
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

/// Everything the flow does not support yet, as one error.
fn check(c: &PackedCmt) -> Result<(), String> {
    let n = &c.name;
    if c.clkfbout_mult_f.fract() != 0.0 || c.clkout0_divide_f.fract() != 0.0 {
        return Err(format!("cmt: {n}: fractional CLKFBOUT_MULT_F / CLKOUT0_DIVIDE_F is not supported yet"));
    }
    if c.clkfbout_phase != 0.0 {
        return Err(format!("cmt: {n}: CLKFBOUT_PHASE is not supported yet"));
    }
    match (&c.clkfbout_net, &c.clkfbin_net) {
        (Some(o), Some(i)) if o == i => {}
        _ => {
            return Err(format!(
                "cmt: {n}: only internal feedback (CLKFBOUT wired straight to CLKFBIN) is supported; feedback through a BUFG is not measured yet"
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
        check(c)?;
        let mmcm = c.kind == CmtKind::MmcmE2Adv;
        let p = format!("{}.{}", pc.tile, if mmcm { "MMCME2_ADV" } else { "PLLE2_ADV" });
        let mut f = |s: String| lines.push(format!("{p}.{s}"));
        f("IN_USE".into());
        f(if mmcm { "COMP.Z_ZHOLD" } else { "COMPENSATION.Z_ZHOLD_OR_CLKIN_BUF" }.into());
        for z in if mmcm { &["ZINV_PSEN", "ZINV_PSINCDEC", "ZINV_PWRDWN", "ZINV_RST"][..] } else { &["ZINV_PWRDWN", "ZINV_RST"][..] } {
            f((*z).into());
        }
        // name, divide, duty, phase, register-2 prefix, output enabled
        let n_out = if mmcm { 7 } else { 6 };
        let mut counters: Vec<(String, u32, f64, f64, String, bool)> = Vec::new();
        let m = c.clkfbout_mult_f.round() as u32;
        counters.push(("CLKFBOUT".into(), m, 0.5, 0.0, "CLKFBOUT_CLKOUT2_".into(), true));
        for i in 0..n_out {
            let r2 = if mmcm && i >= 5 { format!("CLKOUT{i}_CLKOUT2_FRACTIONAL_") } else { format!("CLKOUT{i}_CLKOUT2_") };
            counters.push((format!("CLKOUT{i}"), c.clkout_divide[i], c.clkout_duty[i], c.clkout_phase[i], r2, c.clkout_nets[i].is_some()));
        }
        let mut pmux = [0u32; 2]; // CLKOUT0's and CLKFBOUT's PHASE_MUX
        for (name, div, duty, phase, r2, used) in &counters {
            let (hi, lo, edge, nc) = counter(*div, *duty);
            let ph = phase_eighths(*phase, *div);
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
            if *used {
                f(format!("{name}_CLKOUT1_OUTPUT_ENABLE[0]"));
            }
            match name.as_str() {
                "CLKOUT0" => pmux[0] = ph % 8,
                "CLKFBOUT" => pmux[1] = ph % 8,
                _ => {}
            }
        }
        if mmcm {
            for (i, owner) in [(5, pmux[0]), (6, pmux[1])] {
                if owner != 0 {
                    f(format!("CLKOUT{i}_CLKOUT2_FRACTIONAL_PHASE_MUX_F[2:0] = 3'd{owner}"));
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
            let ii = device.tile_by_name(&it).ok_or_else(|| err("no INT tile"))?;
            let ty = device.tile_type_name(ii).unwrap_or("").to_string();
            let has = |f: &str| device.segbits(&ty, &format!("{ty}.{f}")).is_some();
            let free = |w: &str| !routed.contains(&(it.clone(), w.to_string()));
            let mut ks: Vec<u32> = vec![5];
            ks.extend((0..8).filter(|k| *k != 5));
            for k in ks {
                for g in [1, 0] {
                    let (b, a) = (format!("FAN_BOUNCE{k}"), format!("FAN_ALT{k}"));
                    if has(&format!("{iw}.{b}")) && has(&format!("{a}.GFAN{g}")) && free(&iw) && free(&b) && free(&a) {
                        path.push(format!("{it}.{iw}.{b}"));
                        path.push(format!("{it}.{a}.GFAN{g}"));
                        path.push(format!("{it}.GFAN{g}.GND_WIRE"));
                        return Ok(path);
                    }
                }
            }
            return Err(err(&format!("no free FAN_BOUNCE/GFAN path in {it} for {iw}")));
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
        assert_eq!(phase_eighths(50.625, 40), 45);
        assert_eq!(phase_eighths(345.0, 3), 23);
        assert_eq!(phase_eighths(90.0, 13), 26);
        assert_eq!(phase_eighths(22.5, 2), 1);
    }
}
