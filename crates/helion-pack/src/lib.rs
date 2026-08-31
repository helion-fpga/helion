//! Pack logical cells into Helion site primitives (LUTFF + IOB + MAC27 + ILA).

use helion_device::Device;
use helion_ir::{CellKind, Design};

#[derive(Clone, Debug)]
pub struct Packed {
    pub lutffs: Vec<PackedLutFf>,
    pub iobs: Vec<PackedIob>,
    pub macs: Vec<PackedMac>,
}

#[derive(Clone, Debug)]
pub struct PackedLutFf {
    pub lut_cell: String,
    pub ff_cell: String,
    pub init: u64,
}

#[derive(Clone, Debug)]
pub struct PackedIob {
    pub cell: String,
    pub from_net: String,
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
        let Some(o_net) = net_on(design, &c.name, "O") else {
            continue;
        };
        let Some(ff) = design.cells.iter().find(|f| {
            matches!(f.kind, CellKind::Hff)
                && !used_ff.contains(&f.name)
                && net_on(design, &f.name, "D").as_deref() == Some(o_net.as_str())
        }) else {
            return Err(format!("LUT {} has no FF on D", c.name));
        };
        used_ff.insert(ff.name.clone());
        lutffs.push(PackedLutFf {
            lut_cell: c.name.clone(),
            ff_cell: ff.name.clone(),
            init,
        });
    }
    let mut iobs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            let net = net_on(design, &c.name, "I")
                .ok_or_else(|| format!("IOB {} has no I net", c.name))?;
            iobs.push(PackedIob {
                cell: c.name.clone(),
                from_net: net,
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
    if lutffs.is_empty() && macs.is_empty() {
        return Err("nothing to pack".into());
    }
    Ok(Packed { lutffs, iobs, macs })
}

fn net_on(d: &Design, cell: &str, pin: &str) -> Option<String> {
    d.nets.iter().find_map(|n| {
        n.endpoints
            .iter()
            .any(|e| e.cell == cell && e.pin == pin)
            .then(|| n.name.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;

    #[test]
    fn packs_blinky_lutff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert_eq!(p.iobs.len(), 1);
        assert_eq!(p.lutffs[0].init, 0x5555_5555_5555_5555);
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
}
