//! Pack logical cells into Helion site primitives (LUTFF + IOB + MAC27 + BRAM18 + ILA).

use helion_device::Device;
use helion_ir::{CellKind, Design};

#[derive(Clone, Debug)]
pub struct Packed {
    pub lutffs: Vec<PackedLutFf>,
    pub iobs: Vec<PackedIob>,
    pub macs: Vec<PackedMac>,
    pub brams: Vec<PackedBram>,
}

#[derive(Clone, Debug)]
pub struct PackedBram {
    pub cell: String,
    /// INIT words from IR attr `INIT` (comma-separated hex).
    pub init: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct PackedLutFf {
    pub lut_cell: String,
    pub ff_cell: String,
    pub init: u64,
    /// LUT pin → FF cell whose Q drives it (local cluster).
    pub lut_pins: Vec<(u8, String)>,
    /// Q net of the packed FF (IOB matching).
    pub q_net: String,
}

#[derive(Clone, Debug)]
pub struct PackedIob {
    pub cell: String,
    pub from_net: String,
    /// Optional `IOB_XxYy` from IR port LOC.
    pub loc: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PackedMac {
    pub cell: String,
}

pub fn pack(design: &Design, _dev: &Device) -> Result<Packed, String> {
    let mut lutffs = Vec::new();
    let mut used_ff = std::collections::HashSet::new();
    for c in &design.cells {
        let CellKind::Lut6 { init } = c.kind else {
            continue;
        };
        let Some(o_net) = design.net_on(&c.name, "O") else {
            continue;
        };
        let Some(ff) = design.cells.iter().find(|f| {
            matches!(f.kind, CellKind::Hff)
                && !used_ff.contains(&f.name)
                && design.net_on(&f.name, "D") == Some(o_net)
        }) else {
            return Err(format!("LUT {} has no FF on D", c.name));
        };
        used_ff.insert(ff.name.clone());
        let mut lut_pins = Vec::new();
        for pin in 0u8..6 {
            if let Some(net) = design.net_on(&c.name, &format!("I{pin}")) {
                if let Some(src) = design.cells.iter().find(|f| {
                    matches!(f.kind, CellKind::Hff) && design.net_on(&f.name, "Q") == Some(net)
                }) {
                    lut_pins.push((pin, src.name.clone()));
                }
            }
        }
        let q_net = design
            .net_on(&ff.name, "Q")
            .unwrap_or("")
            .to_string();
        lutffs.push(PackedLutFf {
            lut_cell: c.name.clone(),
            ff_cell: ff.name.clone(),
            init,
            lut_pins,
            q_net,
        });
    }
    let mut iobs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            let net = design
                .net_on(&c.name, "I")
                .ok_or_else(|| format!("IOB {} has no I net", c.name))?;
            let pad = design.net_on(&c.name, "PAD").unwrap_or("");
            let loc = design
                .ports
                .iter()
                .find(|p| p.name == pad)
                .and_then(|p| p.attrs.get("LOC").map(|s| s.to_string()));
            iobs.push(PackedIob {
                cell: c.name.clone(),
                from_net: net.to_string(),
                loc,
            });
        }
    }
    let mut macs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Mac27) {
            macs.push(PackedMac {
                cell: c.name.clone(),
            });
        }
    }
    let mut brams = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Bram18) {
            let init = c
                .attrs
                .get("INIT")
                .unwrap_or("")
                .split(',')
                .filter(|t| !t.is_empty())
                .map(|t| u64::from_str_radix(t.trim(), 16).unwrap_or(0))
                .collect();
            brams.push(PackedBram {
                cell: c.name.clone(),
                init,
            });
        }
    }
    if lutffs.is_empty() && macs.is_empty() && brams.is_empty() {
        return Err("nothing to pack".into());
    }
    Ok(Packed {
        lutffs,
        iobs,
        macs,
        brams,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design};

    #[test]
    fn packs_blinky_lutff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert_eq!(p.iobs.len(), 1);
        assert_eq!(p.lutffs[0].init, 0x5555_5555_5555_5555);
        assert_eq!(p.lutffs[0].lut_pins, vec![(0, "u_ff".into())]);
        assert_eq!(p.lutffs[0].q_net, "q");
    }

    #[test]
    fn packs_counter_four_lutffs_with_pins() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 4);
        assert_eq!(p.lutffs[3].lut_pins.len(), 4);
        assert_eq!(p.iobs[0].from_net, "q3");
        assert_eq!(p.lutffs[3].q_net, "q3");
    }

    #[test]
    fn packs_mac27() {
        let dev = Device::load_part("HL10T-DSP1").unwrap();
        let mut d = Design::new("mac");
        d.add_cell("u_mac", CellKind::Mac27);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.macs.len(), 1);
        assert!(p.lutffs.is_empty());
    }

    #[test]
    fn packs_bram18() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("ram");
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.brams.len(), 1);
    }
}
