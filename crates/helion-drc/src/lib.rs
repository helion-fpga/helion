//! Implementation DRC: overused sites, unrouted IO, missing clocks, occupancy.

use helion_device::Device;
use helion_ir::Design;
use helion_place::Placed;
use helion_route::Routed;
use std::collections::{BTreeMap, HashSet};

/// UG893 DRC violation severity (Error / Warning / Advisory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrcSeverity {
    Error,
    Warning,
    Advisory,
}

impl DrcSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Advisory => "Advisory",
        }
    }
}

/// One UG893 DRC row: rule id + objects + engine message, not a joined dump.
#[derive(Clone, Debug)]
pub struct DrcViolation {
    pub id: String,
    pub severity: DrcSeverity,
    pub objects: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Drc {
    pub violations: Vec<String>,
    pub items: Vec<DrcViolation>,
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

    pub fn add(&mut self, id: &str, objects: impl Into<String>, message: impl Into<String>) {
        let objects = objects.into();
        let message = message.into();
        self.violations.push(message.clone());
        self.items.push(DrcViolation {
            id: id.into(),
            severity: DrcSeverity::Error,
            objects,
            message,
        });
    }

    pub fn item(&self, id: &str) -> Option<&DrcViolation> {
        self.items
            .iter()
            .find(|v| v.id == id)
            .or_else(|| {
                self.items
                    .iter()
                    .find(|v| v.id.eq_ignore_ascii_case(id))
            })
    }

    pub fn error_count(&self) -> usize {
        self.items
            .iter()
            .filter(|v| v.severity == DrcSeverity::Error)
            .count()
    }

    /// UG893 DRC pane text: rule rows from helion-drc, not a one-line dump.
    pub fn text(&self) -> String {
        if self.ok() {
            return "report_drc violations=0 ok".into();
        }
        let n = self.violations.len();
        let mut lines = vec![format!(
            "report_drc violations={n} errors={}",
            self.error_count()
        )];
        for v in &self.items {
            let obj = if v.objects.is_empty() {
                "-"
            } else {
                v.objects.as_str()
            };
            lines.push(format!(
                "{} {} {} {}",
                v.id,
                v.severity.as_str(),
                obj,
                v.message
            ));
        }
        lines.join("\n")
    }
}

pub fn check_placed(design: &Design, placed: &Placed, dev: &Device) -> Drc {
    let mut d = Drc::default();
    if placed.lutff_sites.len() != placed.packed.lutffs.len() {
        d.add(
            "PLACE-1",
            "",
            format!(
                "placed {} LUTFF sites for {} clusters",
                placed.lutff_sites.len(),
                placed.packed.lutffs.len()
            ),
        );
    }
    if placed.packed.lutffs.len() as u32 > dev.lut6_count() {
        d.add("PLACE-2", "", "LUT occupancy exceeds device");
    }
    let mut used = HashSet::new();
    for (site, ble) in &placed.lutff_sites {
        let clb = format!("CLB_X{}Y{}", site.x, site.y);
        if *ble as u32 >= dev.n_ble {
            d.add(
                "PLACE-3",
                &clb,
                format!("BLE {ble} out of range at {clb}"),
            );
        }
        if !used.insert((site.x, site.y, *ble)) {
            d.add("PLACE-4", &clb, format!("overused {clb} BLE{ble}"));
        }
    }
    if placed.mac_sites.len() != placed.packed.macs.len() {
        d.add("DSP-1", "", "DSP placement mismatch");
    }
    if placed.bram_sites.len() != placed.packed.brams.len() {
        d.add("BRAM-1", "", "BRAM placement mismatch");
    }
    if placed.iob_sites.len() != placed.packed.iobs.len() {
        d.add("IOB-1", "", "IOB placement mismatch");
    }
    let has_clk = design.ports.iter().any(|p| p.name == "clk")
        || design
            .nets
            .iter()
            .any(|n| n.endpoints.iter().any(|e| e.pin == "CLK"));
    if !placed.packed.lutffs.is_empty() && !has_clk {
        d.add("CLK-1", "", "no clock on registered design");
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
            d.add(
                "IOSTD-1",
                &p.name,
                format!("IOSTANDARD {std} on {} is not a HAD I/O standard", p.name),
            );
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
            d.add(
                "IOSTD-2",
                format!("BANK{bank}"),
                format!("IOSTANDARD VCCO mix on BANK{bank}: {detail}"),
            );
        }
    }
}

/// UG893 I/O Ports DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM: HAD-legal values and vs IOSTANDARD.
fn check_io_electrical(design: &Design, d: &mut Drc) {
    for p in &design.ports {
        if let Some(drv) = p.attrs.get("DRIVE") {
            match Device::parse_drive(drv) {
                None => d.add(
                    "DRIVE-1",
                    &p.name,
                    format!("DRIVE {drv} on {} is not a HAD drive strength", p.name),
                ),
                Some(ma) => {
                    if !Device::drive_legal_for_iostandard(p.attrs.get("IOSTANDARD"), ma) {
                        let std = p
                            .attrs
                            .get("IOSTANDARD")
                            .unwrap_or(Device::DEFAULT_IOSTANDARD);
                        d.add(
                            "DRIVE-2",
                            &p.name,
                            format!(
                                "DRIVE {ma} not legal for IOSTANDARD {std} on {}",
                                p.name
                            ),
                        );
                    }
                }
            }
        }
        if let Some(s) = p.attrs.get("SLEW") {
            if !Device::legal_slew(s) {
                d.add(
                    "SLEW-1",
                    &p.name,
                    format!("SLEW {s} on {} is not a HAD slew (SLOW|FAST)", p.name),
                );
            }
        }
        if let Some(pt) = p.attrs.get("PULLTYPE") {
            if !Device::legal_pulltype(pt) {
                d.add(
                    "PULL-1",
                    &p.name,
                    format!(
                        "PULLTYPE {pt} on {} is not a HAD pull (NONE|PULLUP|PULLDOWN|KEEPER)",
                        p.name
                    ),
                );
            }
        }
        if let Some(dt) = p.attrs.get("DIFF_TERM") {
            if !Device::legal_diff_term(dt) {
                d.add(
                    "DIFF-1",
                    &p.name,
                    format!("DIFF_TERM {dt} on {} is not a HAD term (TRUE|FALSE)", p.name),
                );
            } else if !Device::diff_term_legal_for_iostandard(p.attrs.get("IOSTANDARD"), dt) {
                let std = p
                    .attrs
                    .get("IOSTANDARD")
                    .unwrap_or(Device::DEFAULT_IOSTANDARD);
                d.add(
                    "DIFF-2",
                    &p.name,
                    format!(
                        "DIFF_TERM {dt} not legal for IOSTANDARD {std} on {}",
                        p.name
                    ),
                );
            }
        }
        if let Some(it) = p.attrs.get("IN_TERM") {
            if !Device::legal_in_term(it) {
                d.add(
                    "INTERM-1",
                    &p.name,
                    format!(
                        "IN_TERM {it} on {} is not a HAD term (NONE|UNTUNED_SPLIT_40|UNTUNED_SPLIT_50|UNTUNED_SPLIT_60)",
                        p.name
                    ),
                );
            } else if !Device::in_term_legal_for_iostandard(p.attrs.get("IOSTANDARD"), it) {
                let std = p
                    .attrs
                    .get("IOSTANDARD")
                    .unwrap_or(Device::DEFAULT_IOSTANDARD);
                d.add(
                    "INTERM-2",
                    &p.name,
                    format!(
                        "IN_TERM {it} not legal for IOSTANDARD {std} on {}",
                        p.name
                    ),
                );
            }
        }
    }
}

pub fn check_routed(design: &Design, routed: &Routed, dev: &Device) -> Drc {
    let mut d = check_placed(design, &routed.placed, dev);
    if !routed.placed.packed.iobs.is_empty() && routed.iob_src.is_empty() {
        d.add("ROUTE-1", "", "unrouted IOB");
    }
    if routed.overused > 0 {
        d.add(
            "ROUTE-2",
            "",
            format!("PathFinder overused {} tiles", routed.overused),
        );
    }
    for iob in &routed.placed.packed.iobs {
        if !routed
            .placed
            .packed
            .lutffs
            .iter()
            .any(|l| l.q_net == iob.from_net)
        {
            d.add(
                "ROUTE-3",
                &iob.cell,
                format!("IOB {} net {} has no FF driver", iob.cell, iob.from_net),
            );
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
        let row = drc.item("IOSTD-2").expect("structured DRC row");
        assert_eq!(row.severity, DrcSeverity::Error);
        assert!(row.objects.contains("BANK"), "{}", row.objects);
        assert!(drc.text().contains("IOSTD-2"), "{}", drc.text());
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
