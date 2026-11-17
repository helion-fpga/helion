//! DSP48E1 pins on the device: which pins pack records, how constant pins
//! get their value, and the programmable inverters (`ZIS_*_INVERTED`).
//!
//! Site pins carry the primitive's own names (`A0`, `OPMODE6`, `CEA1`), so
//! a pin is its own site pin. Cascade pins (`ACIN`, `PCOUT`, ...) are
//! dedicated wires handled by place/route, not here.
//!
//! Constants: 39 pins (`D`, `INMODE`, `OPMODE6`, `ALUMODE2/3`, ...) have a
//! `DSP_GND`/`DSP_VCC` mux in the DSP tile, which [`crate::bram::tie`]
//! takes. Other pins reach INT-tile wires that read VCC when unrouted. A
//! pin with an inverter that is tied to 0 is left at VCC and inverted, as
//! no ground has to be routed for it; any other 0 is tied to the INT
//! tile's ground (X-Ray `ZIS_<pin>_INVERTED` set means not inverted).

use crate::device::Device;
use crate::pack::PackedDsp;
use crate::place::Placed7;
use crate::route::RouteQuery;

/// Input pins of `DSP48E1` that pack records (bus pins as `NAMEi`).
pub const INPUTS: [(&str, u32); 33] = [
    ("A", 30),
    ("B", 18),
    ("C", 48),
    ("D", 25),
    ("CARRYIN", 0),
    ("CARRYINSEL", 3),
    ("OPMODE", 7),
    ("ALUMODE", 4),
    ("INMODE", 5),
    ("CEA1", 0),
    ("CEA2", 0),
    ("CEB1", 0),
    ("CEB2", 0),
    ("CEC", 0),
    ("CED", 0),
    ("CEM", 0),
    ("CEP", 0),
    ("CEAD", 0),
    ("CEALUMODE", 0),
    ("CECTRL", 0),
    ("CECARRYIN", 0),
    ("CEINMODE", 0),
    ("RSTA", 0),
    ("RSTB", 0),
    ("RSTC", 0),
    ("RSTD", 0),
    ("RSTM", 0),
    ("RSTP", 0),
    ("RSTCTRL", 0),
    ("RSTALLCARRYIN", 0),
    ("RSTALUMODE", 0),
    ("RSTINMODE", 0),
    ("CLK", 0),
];

/// Output pins of `DSP48E1` routed through the fabric.
pub const OUTPUTS: [(&str, u32); 6] = [("P", 48), ("CARRYOUT", 4), ("OVERFLOW", 0), ("UNDERFLOW", 0), ("PATTERNDETECT", 0), ("PATTERNBDETECT", 0)];

/// Pins with a constant mux in the DSP tile (X-Ray `DSP_x_<pin>.DSP_GND_L`).
const TILE_MUX: [&str; 14] = ["CEAD", "CEALUMODE", "CED", "CEINMODE", "RSTD", "ALUMODE2", "ALUMODE3", "CARRYINSEL2", "INMODE0", "INMODE1", "INMODE2", "INMODE3", "INMODE4", "OPMODE6"];

fn has_tile_mux(pin: &str) -> bool {
    TILE_MUX.contains(&pin) || (pin.starts_with('D') && pin[1..].parse::<u32>().is_ok())
}

/// Pins with a programmable inverter (besides `CLK`, never inverted).
fn invertible(pin: &str) -> bool {
    pin == "CARRYIN" || ["ALUMODE", "INMODE", "OPMODE"].iter().any(|b| pin.strip_prefix(b).is_some_and(|k| k.parse::<u32>().is_ok()))
}

/// Value an unconnected input takes: clock enables 1, the mode pins the
/// cell's `OPMODE`/`ALUMODE`/`INMODE` (the IR's DSP cells carry them as
/// attributes), everything else 0. `None`: leave it (the clock).
pub fn unconnected_value(pin: &str, opmode: u8, alumode: u8, inmode: u8) -> Option<bool> {
    let bit = |base: &str, v: u8| pin.strip_prefix(base).and_then(|k| k.parse::<u32>().ok()).map(|k| (v >> k) & 1 == 1);
    if pin == "CLK" {
        None
    } else if pin.starts_with("CE") {
        Some(true)
    } else {
        Some(bit("OPMODE", opmode).or_else(|| bit("ALUMODE", alumode)).or_else(|| bit("INMODE", inmode)).unwrap_or(false))
    }
}

/// FASM features of the inverters of a packed DSP (prefix `<tile>.DSP48.DSP_k.`).
pub fn zis_features(d: &PackedDsp) -> Vec<String> {
    let mut v = vec!["ZIS_CLK_INVERTED".to_string()];
    for (pin, val) in crate::bram::pin_names(&INPUTS).iter().filter(|p| invertible(p)).map(|p| (p, d.ties.iter().find(|(t, _)| t == p).map(|t| t.1))) {
        // Inverted only for a 0 kept at the INT tile's VCC.
        if val == Some(false) && !has_tile_mux(pin) {
            continue;
        }
        v.push(match pin.find(|c: char| c.is_ascii_digit()) {
            Some(i) => format!("ZIS_{}_INVERTED[{}]", &pin[..i], &pin[i..]),
            None => format!("ZIS_{pin}_INVERTED"),
        });
    }
    v
}

/// Bits X-Ray has no feature for, seen in Vivado's bitstreams: AREG = 1 and
/// BREG = 1 set the one frame-27 bit of `AREG_0`/`BREG_0` alone (Vivado
/// 2017.3 ZedBoard builds of the DSP fill, all 384 slices; read by x7v).
/// bitgen adds them to the `DSP_L`/`DSP_R` segbits.
pub fn extra_segbits(tile_type: &str) -> Option<String> {
    if tile_type != "DSP_L" && tile_type != "DSP_R" {
        return None;
    }
    Some(
        [("DSP_0", "AREG_1", "27_111"), ("DSP_0", "BREG_1", "27_38"), ("DSP_1", "AREG_1", "27_271"), ("DSP_1", "BREG_1", "27_198")]
            .iter()
            .map(|(site, f, bit)| format!("{tile_type}.DSP48.{site}.{f} {bit}\n"))
            .collect(),
    )
}

/// Every routed endpoint of the placed DSPs: `(net, site, site pin, is_driver)`.
pub fn endpoints(placed: &Placed7) -> Vec<(String, String, String, bool)> {
    let mut out = Vec::new();
    for d in &placed.packed.dsps {
        let Some(pd) = placed.dsps.iter().find(|p| p.name == d.name) else { continue };
        for (pin, net, is_out) in &d.pins {
            out.push((net.clone(), pd.site.clone(), pin.clone(), *is_out));
        }
    }
    out
}

/// FASM lines that give every constant DSP input its value.
pub fn tie_lines(placed: &Placed7, nets: &[crate::route::RoutedNet7], device: &Device, rq: &dyn RouteQuery) -> Result<Vec<String>, String> {
    let routed = crate::bram::routed_wires(nets);
    let mut lines = Vec::new();
    for d in &placed.packed.dsps {
        let Some(pd) = placed.dsps.iter().find(|p| p.name == d.name) else { continue };
        for (pin, value) in &d.ties {
            // An inverted pin is held at VCC (see `zis_features`).
            let v = *value || (invertible(pin) && !has_tile_mux(pin));
            lines.extend(crate::bram::tie(device, rq, &routed, &pd.site, pin, v, pin)?);
        }
    }
    lines.sort();
    lines.dedup();
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverters_hold_constant_modes() {
        let d = PackedDsp {
            ties: vec![("OPMODE0".into(), true), ("OPMODE1".into(), false), ("OPMODE6".into(), false), ("CARRYIN".into(), false)],
            ..Default::default()
        };
        let z = zis_features(&d);
        assert!(z.contains(&"ZIS_OPMODE_INVERTED[0]".to_string()));
        assert!(!z.contains(&"ZIS_OPMODE_INVERTED[1]".to_string()), "a 0 on an IMUX pin is inverted VCC");
        assert!(z.contains(&"ZIS_OPMODE_INVERTED[6]".to_string()), "OPMODE6 is tied by its tile mux");
        assert!(!z.contains(&"ZIS_CARRYIN_INVERTED".to_string()));
        assert!(z.contains(&"ZIS_CLK_INVERTED".to_string()));
        assert_eq!(unconnected_value("OPMODE2", 0b0000101, 0, 0), Some(true));
        assert_eq!(unconnected_value("CEP", 0, 0, 0), Some(true));
        assert_eq!(unconnected_value("RSTP", 0, 0, 0), Some(false));
        assert!(has_tile_mux("D24") && !has_tile_mux("RSTD0") && !has_tile_mux("A3"));
        // AREG/BREG = 1: the one frame-27 bit Vivado sets (not in X-Ray).
        let x = extra_segbits("DSP_R").unwrap();
        assert!(x.contains("DSP_R.DSP48.DSP_0.AREG_1 27_111\n") && x.contains("DSP_R.DSP48.DSP_1.BREG_1 27_198\n"));
        assert!(extra_segbits("BRAM_L").is_none());
    }
}
