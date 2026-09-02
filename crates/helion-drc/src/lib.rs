//! Implementation DRC: overused sites, unrouted IO, missing clocks, occupancy.

use helion_device::Device;
use helion_ir::Design;
use helion_place::Placed;
use helion_route::Routed;
use std::collections::{BTreeMap, HashSet};

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
    check_iostandard(design, placed, dev, &mut d);
    check_io_electrical(design, &mut d);
    d
}

fn parse_iob_xy(spec: &str) -> Option<(u32, u32)> {
    let t = spec.trim();
    let xoff = t.find('X')?;
    let rest = &t[xoff + 1..];
    let yoff = rest.find('Y')?;
    let x = rest[..yoff].parse().ok()?;
    let y = rest[yoff + 1..].parse().ok()?;
    Some((x, y))
}

/// UG893 I/O Ports `IOSTANDARD`: HAD-legal standards and one VCCO per I/O bank.
fn check_iostandard(design: &Design, placed: &Placed, dev: &Device, d: &mut Drc) {
    let mut by_bank: BTreeMap<u32, Vec<(String, String, u32)>> = BTreeMap::new();
    for p in &design.ports {
        let Some(std) = p.attrs.get("IOSTANDARD") else {
            continue;
        };
        let Some(vcco) = Device::iostandard_vcco_mv(std) else {
            d.violations.push(format!(
                "IOSTANDARD {std} on {} is not a HAD I/O standard",
                p.name
            ));
            continue;
        };
        let loc = p
            .attrs
            .get("LOC")
            .and_then(parse_iob_xy)
            .or_else(|| {
                placed
                    .iob_sites
                    .iter()
                    .zip(placed.packed.iobs.iter())
                    .find_map(|(s, i)| {
                        let hit = i.cell.contains(&p.name)
                            || i.from_net == p.name
                            || i.loc.as_deref() == Some(p.attrs.get("LOC").unwrap_or(""));
                        hit.then_some((s.x, s.y))
                    })
            });
        if let Some((x, y)) = loc {
            if let Some(bank) = dev.iob_bank(x, y) {
                by_bank
                    .entry(bank)
                    .or_default()
                    .push((p.name.clone(), std.to_string(), vcco));
            }
        }
    }
    for (bank, ports) in by_bank {
        let mut vccos: Vec<u32> = ports.iter().map(|p| p.2).collect();
        vccos.sort_unstable();
        vccos.dedup();
        if vccos.len() > 1 {
            let detail = ports
                .iter()
                .map(|(n, s, v)| format!("{n}={s}/{v}mV"))
                .collect::<Vec<_>>()
                .join(",");
            d.violations.push(format!(
                "IOSTANDARD VCCO mix on BANK{bank}: {detail}"
            ));
        }
    }
}

/// UG893 I/O Ports DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM: HAD-legal values and vs IOSTANDARD.
fn check_io_electrical(design: &Design, d: &mut Drc) {
    for p in &design.ports {
        if let Some(drv) = p.attrs.get("DRIVE") {
            match Device::parse_drive(drv) {
                None => d.violations.push(format!(
                    "DRIVE {drv} on {} is not a HAD drive strength",
                    p.name
                )),
                Some(ma) => {
                    if !Device::drive_legal_for_iostandard(p.attrs.get("IOSTANDARD"), ma) {
                        let std = p
                            .attrs
                            .get("IOSTANDARD")
                            .unwrap_or(Device::DEFAULT_IOSTANDARD);
                        d.violations.push(format!(
                            "DRIVE {ma} not legal for IOSTANDARD {std} on {}",
                            p.name
                        ));
                    }
                }
            }
        }
        if let Some(s) = p.attrs.get("SLEW") {
            if !Device::legal_slew(s) {
                d.violations.push(format!(
                    "SLEW {s} on {} is not a HAD slew (SLOW|FAST)",
                    p.name
                ));
            }
        }
        if let Some(pt) = p.attrs.get("PULLTYPE") {
            if !Device::legal_pulltype(pt) {
                d.violations.push(format!(
                    "PULLTYPE {pt} on {} is not a HAD pull (NONE|PULLUP|PULLDOWN|KEEPER)",
                    p.name
                ));
            }
        }
        if let Some(dt) = p.attrs.get("DIFF_TERM") {
            if !Device::legal_diff_term(dt) {
                d.violations.push(format!(
                    "DIFF_TERM {dt} on {} is not a HAD term (TRUE|FALSE)",
                    p.name
                ));
            } else if !Device::diff_term_legal_for_iostandard(p.attrs.get("IOSTANDARD"), dt) {
                let std = p
                    .attrs
                    .get("IOSTANDARD")
                    .unwrap_or(Device::DEFAULT_IOSTANDARD);
                d.violations.push(format!(
                    "DIFF_TERM {dt} not legal for IOSTANDARD {std} on {}",
                    p.name
                ));
            }
        }
        if let Some(it) = p.attrs.get("IN_TERM") {
            if !Device::legal_in_term(it) {
                d.violations.push(format!(
                    "IN_TERM {it} on {} is not a HAD term (NONE|UNTUNED_SPLIT_40|UNTUNED_SPLIT_50|UNTUNED_SPLIT_60)",
                    p.name
                ));
            } else if !Device::in_term_legal_for_iostandard(p.attrs.get("IOSTANDARD"), it) {
                let std = p
                    .attrs
                    .get("IOSTANDARD")
                    .unwrap_or(Device::DEFAULT_IOSTANDARD);
                d.violations.push(format!(
                    "IN_TERM {it} not legal for IOSTANDARD {std} on {}",
                    p.name
                ));
            }
        }
    }
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

    #[test]
    fn unknown_iostandard_fails() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVDS_25").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "illegal IOSTANDARD must fail DRC: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("IOSTANDARD") && v.contains("HAD")),
            "{:?}",
            drc.violations
        );
    }

    #[test]
    fn iostandard_vcco_mix_on_bank_fails() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_loc("led", "IOB_X2Y0").unwrap();
        d.set_loc("clk", "IOB_X3Y0").unwrap();
        d.set_iostandard("led", "LVCMOS33").unwrap();
        d.set_iostandard("clk", "LVCMOS18").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "mixed VCCO on one bank must fail: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("VCCO") && v.contains("BANK")),
            "{:?}",
            drc.violations
        );
    }

    #[test]
    fn lvcmos18_on_led_is_clean() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        check_routed(&d, &r, &dev).fail().unwrap();
    }

    #[test]
    fn unknown_drive_slew_pulltype_fail() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_drive("led", "99").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "illegal DRIVE must fail DRC: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("DRIVE") && v.contains("HAD")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_slew("led", "MEDIUM").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(
            drc.violations.iter().any(|v| v.contains("SLEW")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_pulltype("led", "PULL").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(
            drc.violations.iter().any(|v| v.contains("PULLTYPE")),
            "{:?}",
            drc.violations
        );
    }

    #[test]
    fn drive_24_on_lvcmos18_fails_and_defaults_are_clean() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_drive("led", "24").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "DRIVE 24 vs LVCMOS18 must fail: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("DRIVE") && v.contains("IOSTANDARD")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_drive("led", "12").unwrap();
        d.set_slew("led", "SLOW").unwrap();
        d.set_pulltype("led", "NONE").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        check_routed(&d, &r, &dev).fail().unwrap();

        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS25").unwrap();
        d.set_drive("led", "24").unwrap();
        d.set_slew("led", "FAST").unwrap();
        d.set_pulltype("led", "PULLUP").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        check_placed(&d, &pl, &dev).fail().unwrap();
    }

    #[test]
    fn unknown_diff_term_in_term_fail() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_diff_term("led", "YES").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "illegal DIFF_TERM must fail DRC: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("DIFF_TERM") && v.contains("HAD")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_in_term("led", "50").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(
            drc.violations.iter().any(|v| v.contains("IN_TERM")),
            "{:?}",
            drc.violations
        );
    }

    #[test]
    fn diff_term_in_term_vs_lvcmos_fail_and_sstl_is_clean() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_diff_term("led", "TRUE").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "DIFF_TERM TRUE vs LVCMOS18 must fail: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("DIFF_TERM") && v.contains("IOSTANDARD")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(!drc.ok(), "IN_TERM vs LVCMOS18 must fail: {:?}", drc.violations);
        assert!(
            drc.violations.iter().any(|v| v.contains("IN_TERM") && v.contains("IOSTANDARD")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "SSTL15").unwrap();
        d.set_diff_term("led", "TRUE").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        check_placed(&d, &pl, &dev).fail().unwrap();

        let mut d = Design::structural_blinky();
        d.set_diff_term("led", "FALSE").unwrap();
        d.set_in_term("led", "NONE").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        check_routed(&d, &r, &dev).fail().unwrap();
    }
}
