//! Block RAM pins on the device: which site pins a `RAMB18E1` pin uses, and
//! how a pin tied to a constant gets its value.
//!
//! A BRAM tile holds two RAMB18 halves. The lower one (`RAMB18_X*Y<even>`,
//! FASM `RAMB18_Y0`) is a `FIFO18E1` site whose pins carry FIFO names
//! (`RDCLK`, `DO0`, ...); the upper one (`RAMB18_Y1`) is a `RAMB18E1` site
//! with the primitive's own names. [`site_pins`] maps a primitive pin to the
//! site pin(s) of either half.
//!
//! Constants are not routed nets. A pin tied to 1 is left unrouted: every
//! INT-tile `IMUX` reads VCC when none of its pips is set (X-Ray ppip
//! `INT_*.IMUX*.VCC_WIRE default`). A pin tied to 0 is driven from the INT
//! tile's ground: `GFANn.GND_WIRE` and `IMUX/CTRL.GFANn` ([`tie_lines`]).
//! The router never uses `GFAN` for signals, and an INT tile beside a BRAM
//! serves only the BRAM, so these wires are free.

use crate::device::Device;
use crate::pack::BramKind;
use crate::place::{parse_site_xy, Placed7};
use crate::route::{RouteKind, RouteQuery};
use std::collections::HashSet;

/// Input pins of `RAMB18E1` that pack records (bus pins as `NAMEi`).
pub const RAMB18_INPUTS: [(&str, u32); 22] = [
    ("ADDRARDADDR", 14),
    ("ADDRBWRADDR", 14),
    ("CLKARDCLK", 0),
    ("CLKBWRCLK", 0),
    ("DIADI", 16),
    ("DIBDI", 16),
    ("DIPADIP", 2),
    ("DIPBDIP", 2),
    ("ENARDEN", 0),
    ("ENBWREN", 0),
    ("REGCEAREGCE", 0),
    ("REGCEB", 0),
    ("REGCLKARDRCLK", 0),
    ("REGCLKB", 0),
    ("RSTRAMARSTRAM", 0),
    ("RSTRAMB", 0),
    ("RSTREGARSTREG", 0),
    ("RSTREGB", 0),
    ("WEA", 2),
    ("WEBWE", 4),
    // `WEA` / `WEBWE` without an index (a 1-bit connection) are accepted too.
    ("WEA", 0),
    ("WEBWE", 0),
];

/// Output pins of `RAMB18E1`.
pub const RAMB18_OUTPUTS: [(&str, u32); 4] = [("DOADO", 16), ("DOBDO", 16), ("DOPADOP", 2), ("DOPBDOP", 2)];

/// Pins with a programmable inverter (`ZINV_<pin>`); emit always disables it.
pub const RAMB18_ZINV: [&str; 10] = [
    "CLKARDCLK",
    "CLKBWRCLK",
    "ENARDEN",
    "ENBWREN",
    "REGCLKARDRCLK",
    "REGCLKB",
    "RSTRAMARSTRAM",
    "RSTRAMB",
    "RSTREGARSTREG",
    "RSTREGB",
];

/// Every pin name in `list`, bus bits expanded.
pub fn pin_names(list: &[(&str, u32)]) -> Vec<String> {
    let mut v = Vec::new();
    for &(base, n) in list {
        if n == 0 {
            v.push(base.to_string());
        } else {
            v.extend((0..n).map(|i| format!("{base}{i}")));
        }
    }
    v
}

/// FASM prefix of a RAMB18 site: `RAMB18_Y0` (even Y, the FIFO18E1 half) or
/// `RAMB18_Y1`.
pub fn half(site: &str) -> Result<&'static str, String> {
    match parse_site_xy(site) {
        Some((_, y)) if site.starts_with("RAMB18_") => Ok(if y % 2 == 0 { "RAMB18_Y0" } else { "RAMB18_Y1" }),
        _ => Err(format!("bram: `{site}` is not a RAMB18 site")),
    }
}

fn split_bus(pin: &str) -> (&str, Option<u32>) {
    let cut = pin.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    (&pin[..cut], pin[cut..].parse().ok())
}

/// Site pins of a placed `RAMB18E1` that primitive pin `pin` drives or reads.
///
/// The site has four `WEA` and eight `WEBWE` pins; primitive bit `k` goes to
/// site pins `2k` and `2k+1`.
/// shortcut: the byte-lane order of the site WE pins is not proven against
/// Vivado (x7v accepts only whole-word writes, where all lanes are equal);
/// prove it before relying on byte enables.
pub fn site_pins(site: &str, pin: &str) -> Result<Vec<String>, String> {
    let y0 = half(site)? == "RAMB18_Y0";
    let (base, bit) = split_bus(pin);
    let one = |s: String| Ok(vec![s]);
    match (base, bit) {
        ("WEA" | "WEBWE", None) => {
            let n = if base == "WEA" { 4 } else { 8 };
            Ok((0..n).map(|i| format!("{base}{i}")).collect())
        }
        ("WEA", Some(k)) if k < 2 => Ok(vec![format!("WEA{}", 2 * k), format!("WEA{}", 2 * k + 1)]),
        ("WEBWE", Some(k)) if k < 4 => Ok(vec![format!("WEBWE{}", 2 * k), format!("WEBWE{}", 2 * k + 1)]),
        _ if !y0 => one(pin.to_string()),
        ("CLKARDCLK", None) => one("RDCLK".into()),
        ("CLKBWRCLK", None) => one("WRCLK".into()),
        ("ENARDEN", None) => one("RDEN".into()),
        ("ENBWREN", None) => one("WREN".into()),
        ("RSTRAMARSTRAM", None) => one("RST".into()),
        ("RSTREGARSTREG", None) => one("RSTREG".into()),
        ("REGCEAREGCE", None) => one("REGCE".into()),
        ("REGCLKARDRCLK", None) => one("RDRCLK".into()),
        ("DOADO", Some(k)) if k < 16 => one(format!("DO{k}")),
        ("DOBDO", Some(k)) if k < 16 => one(format!("DO{}", 16 + k)),
        ("DOPADOP", Some(k)) if k < 2 => one(format!("DOP{k}")),
        ("DOPBDOP", Some(k)) if k < 2 => one(format!("DOP{}", 2 + k)),
        _ => one(pin.to_string()),
    }
}

/// Every routed endpoint of the placed BRAMs: `(net, site, site pin, is_driver)`.
/// RAMB36E1 is refused: its pins split over the two halves (L/U) and no
/// path for that is written or checked yet.
pub fn endpoints(placed: &Placed7) -> Result<Vec<(String, String, String, bool)>, String> {
    let mut out = Vec::new();
    for b in &placed.packed.brams {
        let Some(pb) = placed.brams.iter().find(|p| p.name == b.name) else { continue };
        if b.kind == BramKind::Ramb36e1 {
            return Err(format!("bram: {}: RAMB36E1 pin routing is not supported yet (use RAMB18E1)", b.name));
        }
        for (pin, net, out_pin) in &b.pins {
            for sp in site_pins(&pb.site, pin)? {
                out.push((net.clone(), pb.site.clone(), sp, *out_pin));
            }
        }
    }
    Ok(out)
}

/// FASM lines that give every constant BRAM input its value (see the
/// module doc). Fails loudly when a tie would meet a routed net or a pin
/// has no INT-tile wire to tie.
pub fn tie_lines(placed: &Placed7, nets: &[crate::route::RoutedNet7], device: &Device, rq: &dyn RouteQuery) -> Result<Vec<String>, String> {
    let routed = routed_wires(nets);
    let mut lines = Vec::new();
    for b in &placed.packed.brams {
        if b.ties.is_empty() {
            continue;
        }
        let Some(pb) = placed.brams.iter().find(|p| p.name == b.name) else { continue };
        if b.kind == BramKind::Ramb36e1 {
            return Err(format!("bram: {}: RAMB36E1 constant pins are not supported yet", b.name));
        }
        for (pin, value) in &b.ties {
            // A clock tied to a constant never ticks; leaving it unrouted is the same.
            if pin.starts_with("CLK") || pin.starts_with("REGCLK") {
                continue;
            }
            for sp in site_pins(&pb.site, pin)? {
                lines.extend(tie(device, rq, &routed, &pb.site, &sp, *value, pin)?);
            }
        }
    }
    lines.sort();
    lines.dedup();
    Ok(lines)
}

/// A BRAM tile that takes an address through `BRAM_CASCINBOT_*` reads the
/// tile below's address, which drives it only with `CASCOUT_ARD_ACTIVE`
/// (port A) or `CASCOUT_BWR_ACTIVE` (port B) set: Vivado sets it in every
/// such pair (x7v, 168 of 168 in its ZedBoard block RAM builds).
/// shortcut: `CASCINTOP` has no Vivado evidence yet; it gets nothing.
pub fn cascade_out_lines(fasm: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for l in fasm {
        let Some((tile, rest)) = l.split_once('.') else { continue };
        let port = if rest.contains(".BRAM_CASCINBOT_ADDRARDADDR") {
            "ARD"
        } else if rest.contains(".BRAM_CASCINBOT_ADDRBWRADDR") {
            "BWR"
        } else {
            continue;
        };
        // BRAM tiles are 5 rows apart: BRAM_L_X6Y45 sits on BRAM_L_X6Y40.
        let Some(yi) = tile.rfind('Y') else { continue };
        let Ok(y) = tile[yi + 1..].parse::<i32>() else { continue };
        out.push(format!("{}Y{}.CASCOUT_{port}_ACTIVE", &tile[..yi], y - 5));
    }
    out.sort();
    out.dedup();
    out
}

/// `(tile, wire)` of every pip destination the router set.
pub fn routed_wires(nets: &[crate::route::RoutedNet7]) -> HashSet<(String, String)> {
    let mut routed = HashSet::new();
    for n in nets {
        if let RouteKind::Pips(pips) = &n.kind {
            for p in pips {
                routed.insert((p.tile.clone(), p.dst_wire.clone()));
            }
        }
    }
    routed
}

/// FASM lines that hold site pin `sp` of `site` at `value` (`pin` names it
/// in errors). A pin with its own constant mux in the site's tile (DSP
/// `D`, `OPMODE6`, ...: `DSP_GND_L`/`DSP_VCC_L`) takes that; any other is
/// tied in the INT tile as the module doc says.
pub fn tie(device: &Device, rq: &dyn RouteQuery, routed: &HashSet<(String, String)>, site: &str, sp: &str, value: bool, pin: &str) -> Result<Vec<String>, String> {
    let (ti, si) = device.site_by_name(site).ok_or_else(|| format!("tie: no site {site}"))?;
    let tile = device.tile(ti).map(|t| t.name.clone()).unwrap_or_default();
    let ty = device.tile_type_name(ti).unwrap_or("").to_string();
    let wire = device
        .site_pins(ti, si)
        .ok_or_else(|| format!("tie: no pins for {site}"))?
        .iter()
        .find(|(n, _)| *n == sp)
        .map(|(_, w)| w.clone())
        .ok_or_else(|| format!("tie: {site} has no pin {sp}"))?;
    let names = device.wires(ti).unwrap_or(&[]);
    let rail = if value { "VCC" } else { "GND" };
    let own = device.pips_into(ti, &wire).into_iter().map(|p| names[p.src as usize].clone()).find(|src| {
        src.contains(rail) && device.segbits(&ty, &format!("{ty}.{wire}.{src}")).is_some()
    });
    if let Some(src) = own {
        return Ok(vec![format!("{tile}.{wire}.{src}")]);
    }
    let (mut path, int, int_wire) = to_int(device, rq, &tile, &wire)
        .ok_or_else(|| format!("tie: {site}.{sp} ({pin}): no INT-tile wire found to tie it to {}", value as u8))?;
    if routed.contains(&(int.clone(), int_wire.clone())) {
        return Err(format!("tie: {site}.{sp} is tied to {} but {int}.{int_wire} carries a routed net", value as u8));
    }
    let int_type = device.tile_by_name(&int).and_then(|i| device.tile_type_name(i)).unwrap_or("").to_string();
    // `BYP_Ln` is fed by `BYP_ALTn` through an always-on pip; the tie goes on `BYP_ALTn`.
    let always = device
        .ppips(&int_type)
        .and_then(|ps| ps.iter().find(|p| p.kind == crate::db::parse::PpipKind::Always && p.name.starts_with(&format!("{int_type}.{int_wire}."))))
        .map(|p| p.name.rsplit('.').next().unwrap_or("").to_string());
    let int_wire = match always {
        Some(w) => {
            if routed.contains(&(int.clone(), w.clone())) {
                return Err(format!("tie: {site}.{sp} is tied to {} but {int}.{w} carries a routed net", value as u8));
            }
            w
        }
        None => int_wire,
    };
    if value {
        let default = format!("{int_type}.{int_wire}.VCC_WIRE");
        if !device.ppips(&int_type).is_some_and(|ps| ps.iter().any(|p| p.name == default)) {
            return Err(format!("tie: {site}.{sp} ({pin}) tied to 1, but {int}.{int_wire} does not read VCC by default"));
        }
    } else {
        let g = (0..2)
            .find(|g| device.segbits(&int_type, &format!("{int_type}.{int_wire}.GFAN{g}")).is_some())
            .ok_or_else(|| format!("tie: {site}.{sp} ({pin}) tied to 0, but {int}.{int_wire} has no GFAN input"))?;
        path.push(format!("{int}.{int_wire}.GFAN{g}"));
        path.push(format!("{int}.GFAN{g}.GND_WIRE"));
    }
    Ok(path)
}

/// Walk from a BRAM pin wire up to the INT-tile wire feeding it: the
/// configurable BRAM-tile pips on the way (as FASM lines) and the INT tile
/// and wire. Always-on pips are followed; of the configurable ones, the
/// `IMUX` input is taken (address pins choose between it and the cascade).
fn to_int(device: &Device, rq: &dyn RouteQuery, tile: &str, wire: &str) -> Option<(Vec<String>, String, String)> {
    let mut path = Vec::new();
    let t = tile.to_string();
    let mut w = wire.to_string();
    for _ in 0..6 {
        for (nt, nw) in std::iter::once((t.clone(), w.clone())).chain(rq.node_wires(&t, &w)) {
            let is_int = device.tile_by_name(&nt).and_then(|i| device.tile_type_name(i)).is_some_and(|ty| ty == "INT_L" || ty == "INT_R");
            if is_int {
                return Some((path, nt, nw));
            }
        }
        let ti = device.tile_by_name(&t)?;
        let ty = device.tile_type_name(ti)?.to_string();
        let names = device.wires(ti)?;
        let srcs: Vec<(String, bool)> = device
            .pips_into(ti, &w)
            .into_iter()
            .map(|p| {
                let src = names[p.src as usize].clone();
                let always = device
                    .ppips(&ty)
                    .is_some_and(|ps| ps.iter().any(|pp| pp.name == format!("{ty}.{w}.{src}") && pp.kind == crate::db::parse::PpipKind::Always));
                (src, always)
            })
            .collect();
        let next = if let Some((s, _)) = srcs.iter().find(|(_, a)| *a) {
            s.clone()
        } else {
            let (s, _) = srcs.iter().find(|(s, _)| s.contains("IMUX"))?;
            path.push(format!("{t}.{w}.{s}"));
            s.clone()
        };
        w = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_half_uses_fifo_pin_names() {
        assert_eq!(site_pins("RAMB18_X0Y4", "CLKARDCLK").unwrap(), vec!["RDCLK"]);
        assert_eq!(site_pins("RAMB18_X0Y4", "DOBDO3").unwrap(), vec!["DO19"]);
        assert_eq!(site_pins("RAMB18_X0Y4", "DOPBDOP1").unwrap(), vec!["DOP3"]);
        assert_eq!(site_pins("RAMB18_X0Y4", "ADDRARDADDR7").unwrap(), vec!["ADDRARDADDR7"]);
        assert_eq!(site_pins("RAMB18_X0Y5", "CLKARDCLK").unwrap(), vec!["CLKARDCLK"]);
        assert_eq!(site_pins("RAMB18_X0Y5", "DOBDO3").unwrap(), vec!["DOBDO3"]);
    }

    #[test]
    fn cascade_in_from_below_enables_its_cascade_out() {
        let fasm = vec![
            "BRAM_L_X6Y45.BRAM_ADDRARDADDRL4.BRAM_CASCINBOT_ADDRARDADDRU4".to_string(),
            "BRAM_L_X6Y45.BRAM_ADDRBWRADDRU7.BRAM_CASCINBOT_ADDRBWRADDRU7".to_string(),
            "BRAM_L_X6Y45.BRAM_ADDRARDADDRL5.BRAM_IMUX_ADDRARDADDRL5".to_string(),
        ];
        assert_eq!(cascade_out_lines(&fasm), vec!["BRAM_L_X6Y40.CASCOUT_ARD_ACTIVE", "BRAM_L_X6Y40.CASCOUT_BWR_ACTIVE"]);
    }

    #[test]
    fn write_enables_fill_every_site_lane() {
        assert_eq!(site_pins("RAMB18_X0Y5", "WEA1").unwrap(), vec!["WEA2", "WEA3"]);
        assert_eq!(site_pins("RAMB18_X0Y4", "WEBWE0").unwrap(), vec!["WEBWE0", "WEBWE1"]);
        assert_eq!(site_pins("RAMB18_X0Y4", "WEA").unwrap().len(), 4);
        assert!(site_pins("RAMB36_X0Y2", "WEA0").is_err());
        assert_eq!(half("RAMB18_X3Y10").unwrap(), "RAMB18_Y0");
        assert_eq!(half("RAMB18_X3Y11").unwrap(), "RAMB18_Y1");
    }
}
