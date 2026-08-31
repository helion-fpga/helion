//! Implementation DRC: overused sites, unrouted IO, missing clocks, occupancy.

use helion_device::Device;
use helion_ir::Design;
use helion_place::Placed;
use helion_route::Routed;
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct Drc {
    pub violations: Vec<String>,
}

impl Drc {
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn fail(&self) -> Result<(), String> {
        if self.ok() {
            Ok(())
        } else {
            Err(self.violations.join("; "))
        }
    }
}

pub fn check_placed(design: &Design, placed: &Placed, dev: &Device) -> Drc {
    let mut d = Drc::default();
    if placed.lutff_sites.len() != placed.packed.lutffs.len() {
        d.violations.push(format!(
            "placed {} LUTFF sites for {} clusters",
            placed.lutff_sites.len(),
            placed.packed.lutffs.len()
        ));
    }
    if placed.packed.lutffs.len() as u32 > dev.lut6_count() {
        d.violations.push("LUT occupancy exceeds device".into());
    }
    let mut used = HashSet::new();
    for (site, ble) in &placed.lutff_sites {
        if *ble as u32 >= dev.n_ble {
            d.violations.push(format!("BLE {ble} out of range at CLB_X{}Y{}", site.x, site.y));
        }
        if !used.insert((site.x, site.y, *ble)) {
            d.violations.push(format!("overused CLB_X{}Y{} BLE{ble}", site.x, site.y));
        }
    }
    if placed.mac_sites.len() != placed.packed.macs.len() {
        d.violations.push("DSP placement mismatch".into());
    }
    if placed.bram_sites.len() != placed.packed.brams.len() {
        d.violations.push("BRAM placement mismatch".into());
    }
    if placed.iob_sites.len() != placed.packed.iobs.len() {
        d.violations.push("IOB placement mismatch".into());
    }
    let has_clk = design.ports.iter().any(|p| p.name == "clk")
        || design
            .nets
            .iter()
            .any(|n| n.endpoints.iter().any(|e| e.pin == "CLK"));
    if !placed.packed.lutffs.is_empty() && !has_clk {
        d.violations.push("no clock on registered design".into());
    }
    d
}

pub fn check_routed(design: &Design, routed: &Routed, dev: &Device) -> Drc {
    let mut d = check_placed(design, &routed.placed, dev);
    if !routed.placed.packed.iobs.is_empty() && routed.iob_src.is_empty() {
        d.violations.push("unrouted IOB".into());
    }
    if routed.overused > 0 {
        d.violations.push(format!("PathFinder overused {} tiles", routed.overused));
    }
    for iob in &routed.placed.packed.iobs {
        if !routed
            .placed
            .packed
            .lutffs
            .iter()
            .any(|l| l.q_net == iob.from_net)
        {
            d.violations.push(format!("IOB {} net {} has no FF driver", iob.cell, iob.from_net));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::place;
    use helion_route::route;

    #[test]
    fn blinky_is_clean() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        check_routed(&d, &r, &dev).fail().unwrap();
    }

    #[test]
    fn overused_ble_fails() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let mut pl = place(&p, &dev).unwrap();
        let s = pl.lutff_sites[0];
        pl.lutff_sites.push(s);
        pl.packed.lutffs.push(pl.packed.lutffs[0].clone());
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "duplicate (site,BLE) must fail DRC: {:?}", drc.violations);
    }
}
