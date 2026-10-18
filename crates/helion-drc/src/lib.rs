//! Implementation DRC: overused sites, unrouted IO, missing clocks, occupancy.

use helion_device::Device;
use helion_ir::{intern, resolve, Design};
use helion_place::Placed;
use helion_route::Routed;
use std::collections::{BTreeMap, HashSet};

pub mod decks;
pub mod taxonomy;
pub mod waivers;

pub use decks::{
    check_all_ug906_ug949_decks, check_aval_deck, check_cdc_deck, check_methodology_deck,
    check_pdrc_deck, check_reqp_deck, check_rule_deck, check_synth_deck, check_timing_deck,
    emit_rule,
};
pub use taxonomy::{
    RuleDeckCategory, RuleDefinition, all_ug906_ug949_rules, lookup_rule, rules_in_deck,
};
pub use waivers::{
    DrcWaiver, RemediationReport, ViolationRemediation, WaivedDrcReport, WaivedViolation,
    WaiverSet, generate_remediation_script, remediate_violation,
};

pub mod signoff;

pub use signoff::{
    AntennaNetMetric, BitgenSignoffReport, DEFAULT_MAX_ANTENNA_RATIO, SignoffConfig,
    SignoffRuleInfo, check_bitstream_signoff, find_combinational_lut_loops, signoff_rule_info,
};

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
        self.fail_errors()
    }

    pub fn add(&mut self, id: &str, objects: impl Into<String>, message: impl Into<String>) {
        self.push(id, DrcSeverity::Error, objects, message);
    }

    pub fn add_warn(&mut self, id: &str, objects: impl Into<String>, message: impl Into<String>) {
        self.push(id, DrcSeverity::Warning, objects, message);
    }

    pub fn add_advisory(
        &mut self,
        id: &str,
        objects: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.push(id, DrcSeverity::Advisory, objects, message);
    }

    pub fn add_with_severity(
        &mut self,
        id: &str,
        severity: DrcSeverity,
        objects: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.push(id, severity, objects, message);
    }

    /// Generate an actionable Tcl / RTL remediation script for all violations in this [`Drc`].
    pub fn remediation_script(&self) -> String {
        generate_remediation_script(self).to_tcl_script()
    }

    pub fn add_with_severity(
        &mut self,
        id: &str,
        severity: DrcSeverity,
        objects: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.push(id, severity, objects, message);
    }

    fn push(
        &mut self,
        id: &str,
        severity: DrcSeverity,
        objects: impl Into<String>,
        message: impl Into<String>,
    ) {
        let objects = objects.into();
        let message = message.into();
        self.violations.push(message.clone());
        self.items.push(DrcViolation {
            id: id.into(),
            severity,
            objects,
            message,
        });
    }

    /// Errors only — warnings do not fail bitstream (UG893 Warning).
    pub fn fail_errors(&self) -> Result<(), String> {
        let errs: Vec<String> = self
            .items
            .iter()
            .filter(|v| v.severity == DrcSeverity::Error)
            .map(|v| v.message.clone())
            .collect();
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    pub fn item(&self, id: &str) -> Option<&DrcViolation> {
        self.items
            .iter()
            .find(|v| v.id == id)
            .or_else(|| self.items.iter().find(|v| v.id.eq_ignore_ascii_case(id)))
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
    let mut used = HashSet::with_capacity(placed.lutff_sites.len());
    for (site, ble) in &placed.lutff_sites {
        if *ble as u32 >= dev.n_ble {
            let clb = format!("CLB_X{}Y{}", site.x, site.y);
            d.add("PLACE-3", &clb, format!("BLE {ble} out of range at {clb}"));
        }
        if !used.insert((site.x, site.y, *ble)) {
            let clb = format!("CLB_X{}Y{}", site.x, site.y);
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
    let clk_port_spur = intern("clk");
    let clk_pin_spur = intern("CLK");
    let has_clk = design.ports.iter().any(|p| p.name == clk_port_spur)
        || design
            .nets
            .iter()
            .any(|n| n.endpoints.iter().any(|e| e.pin == clk_pin_spur));
    let has_ff = placed.packed.lutffs.iter().any(|l| !l.ff_cell.is_empty());
    if has_ff && !has_clk {
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
        let pname = resolve(p.name);
        let Some(vcco) = Device::iostandard_vcco_mv(std) else {
            d.add(
                "IOSTD-1",
                pname,
                format!("IOSTANDARD {std} on {pname} is not a HAD I/O standard"),
            );
            continue;
        };
        let port_loc = p.attrs.get("LOC");
        let loc = port_loc.and_then(parse_iob_xy).or_else(|| {
            let loc_str = port_loc.unwrap_or("");
            placed
                .iob_sites
                .iter()
                .zip(placed.packed.iobs.iter())
                .find_map(|(s, i)| {
                    let hit = i.cell.contains(pname)
                        || i.from_net == pname
                        || i.loc.as_deref() == Some(loc_str);
                    hit.then_some((s.x, s.y))
                })
        });
        if let Some((x, y)) = loc {
            if let Some(bank) = dev.iob_bank(x, y) {
                by_bank
                    .entry(bank)
                    .or_default()
                    .push((pname.to_string(), std.to_string(), vcco));
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
        let pname = resolve(p.name);
        let iostd = p.attrs.get("IOSTANDARD");
        if let Some(drv) = p.attrs.get("DRIVE") {
            match Device::parse_drive(drv) {
                None => d.add(
                    "DRIVE-1",
                    pname,
                    format!("DRIVE {drv} on {pname} is not a HAD drive strength"),
                ),
                Some(ma) => {
                    if !Device::drive_legal_for_iostandard(iostd, ma) {
                        let std = iostd.unwrap_or(Device::DEFAULT_IOSTANDARD);
                        d.add(
                            "DRIVE-2",
                            pname,
                            format!("DRIVE {ma} not legal for IOSTANDARD {std} on {pname}"),
                        );
                    }
                }
            }
        }
        if let Some(s) = p.attrs.get("SLEW") {
            if !Device::legal_slew(s) {
                d.add(
                    "SLEW-1",
                    pname,
                    format!("SLEW {s} on {pname} is not a HAD slew (SLOW|FAST)"),
                );
            }
        }
        if let Some(pt) = p.attrs.get("PULLTYPE") {
            if !Device::legal_pulltype(pt) {
                d.add(
                    "PULL-1",
                    pname,
                    format!(
                        "PULLTYPE {pt} on {pname} is not a HAD pull (NONE|PULLUP|PULLDOWN|KEEPER)"
                    ),
                );
            }
        }
        if let Some(dt) = p.attrs.get("DIFF_TERM") {
            if !Device::legal_diff_term(dt) {
                d.add(
                    "DIFF-1",
                    pname,
                    format!("DIFF_TERM {dt} on {pname} is not a HAD term (TRUE|FALSE)"),
                );
            } else if !Device::diff_term_legal_for_iostandard(iostd, dt) {
                let std = iostd.unwrap_or(Device::DEFAULT_IOSTANDARD);
                d.add(
                    "DIFF-2",
                    pname,
                    format!("DIFF_TERM {dt} not legal for IOSTANDARD {std} on {pname}"),
                );
            }
        }
        if let Some(it) = p.attrs.get("IN_TERM") {
            if !Device::legal_in_term(it) {
                d.add(
                    "INTERM-1",
                    pname,
                    format!(
                        "IN_TERM {it} on {pname} is not a HAD term (NONE|UNTUNED_SPLIT_40|UNTUNED_SPLIT_50|UNTUNED_SPLIT_60)"
                    ),
                );
            } else if !Device::in_term_legal_for_iostandard(iostd, it) {
                let std = iostd.unwrap_or(Device::DEFAULT_IOSTANDARD);
                d.add(
                    "INTERM-2",
                    pname,
                    format!("IN_TERM {it} not legal for IOSTANDARD {std} on {pname}"),
                );
            }
        }
    }
}

pub fn check_routed(design: &Design, routed: &Routed, dev: &Device) -> Drc {
    let mut d = check_placed(design, &routed.placed, dev);
    if !routed.placed.packed.iobs.is_empty() && routed.iob_src.is_empty() {
        d.add_warn("ROUTE-1", "", "unrouted IOB");
    }
    if routed.overused > 0 {
        d.add_warn(
            "ROUTE-2",
            "",
            format!("PathFinder overused {} tiles", routed.overused),
        );
    }
    let driver_nets: std::collections::HashSet<&str> = routed
        .placed
        .packed
        .lutffs
        .iter()
        .map(|l| l.q_net.as_str())
        .collect();
    for iob in &routed.placed.packed.iobs {
        if !driver_nets.contains(iob.from_net.as_str()) {
            d.add_warn(
                "ROUTE-3",
                &iob.cell,
                format!("IOB {} net {} has no LUT/FF driver", iob.cell, iob.from_net),
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
        assert!(
            !drc.ok(),
            "duplicate (site,BLE) must fail DRC: {:?}",
            drc.violations
        );
    }

    #[test]
    fn unknown_iostandard_fails() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVDS_25").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(
            !drc.ok(),
            "illegal IOSTANDARD must fail DRC: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("IOSTANDARD") && v.contains("HAD")),
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
        assert!(
            !drc.ok(),
            "mixed VCCO on one bank must fail: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("VCCO") && v.contains("BANK")),
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
        assert!(
            !drc.ok(),
            "illegal DRIVE must fail DRC: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("DRIVE") && v.contains("HAD")),
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
        assert!(
            !drc.ok(),
            "DRIVE 24 vs LVCMOS18 must fail: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("DRIVE") && v.contains("IOSTANDARD")),
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
        assert!(
            !drc.ok(),
            "illegal DIFF_TERM must fail DRC: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("DIFF_TERM") && v.contains("HAD")),
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
        assert!(
            !drc.ok(),
            "DIFF_TERM TRUE vs LVCMOS18 must fail: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("DIFF_TERM") && v.contains("IOSTANDARD")),
            "{:?}",
            drc.violations
        );

        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let drc = check_placed(&d, &pl, &dev);
        assert!(
            !drc.ok(),
            "IN_TERM vs LVCMOS18 must fail: {:?}",
            drc.violations
        );
        assert!(
            drc.violations
                .iter()
                .any(|v| v.contains("IN_TERM") && v.contains("IOSTANDARD")),
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

    #[test]
    fn ug906_ug949_taxonomy_covers_all_required_rule_ranges() {
        // Verify TIMING-1..50, SYNTH-1..15, METHODOLOGY-1..25, CDC-1..20,
        // PDRC-1..20, REQP-1..30, AVAL-1..20 all resolve with titles and remediation templates.
        let ranges: &[(&str, u16, RuleDeckCategory)] = &[
            ("TIMING", 50, RuleDeckCategory::Timing),
            ("SYNTH", 15, RuleDeckCategory::Synth),
            ("METHODOLOGY", 25, RuleDeckCategory::Methodology),
            ("CDC", 20, RuleDeckCategory::Cdc),
            ("PDRC", 20, RuleDeckCategory::Pdrc),
            ("REQP", 30, RuleDeckCategory::Reqp),
            ("AVAL", 20, RuleDeckCategory::Aval),
        ];
        for &(prefix, max_n, cat) in ranges {
            let deck = rules_in_deck(cat);
            assert_eq!(deck.len(), usize::from(max_n), "deck count for {prefix}");
            for n in 1..=max_n {
                let id = format!("{prefix}-{n}");
                let def = lookup_rule(&id).unwrap_or_else(|| panic!("missing {id}"));
                assert_eq!(def.id, id);
                assert_eq!(def.category, cat);
                assert!(!def.title.is_empty());
                assert!(!def.description.is_empty());
                assert!(!def.tcl_remediation_template.is_empty());
                assert!(!def.rtl_remediation_hint.is_empty());
            }
        }
        assert!(lookup_rule("TIMING-51").is_none());
        assert!(lookup_rule("SYNTH-16").is_none());
        assert_eq!(all_ug906_ug949_rules().len(), 201);
    }

    #[test]
    fn active_timing_synth_methodology_cdc_reqp_aval_decks_detect_violations() {
        use helion_ir::{CellKind, PortDir};
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("multi_deck_test");
        d.add_port("clk_a", PortDir::In);
        d.add_port("clk_b", PortDir::In);
        d.add_port("dangling_port", PortDir::Out);
        d.add_port("comb_out", PortDir::Out);

        // 1. Unclocked Hff (TIMING-17, REQP-1, REQP-2)
        d.add_cell("u_unclocked_ff", CellKind::Hff);

        // 2. LUT driving sequential CLK pin (TIMING-14) + self-loop (SYNTH-3) + constant 0 (AVAL-10)
        d.add_cell("u_clk_lut", CellKind::Lut6 { init: 0 });
        d.connect("gated_clk", "u_clk_lut", "O");
        d.connect("gated_clk", "u_clk_lut", "I0");
        d.add_cell("u_gated_ff", CellKind::Hff);
        d.connect("gated_clk", "u_gated_ff", "CLK");
        d.connect("gated_clk", "u_gated_ff", "D");

        // 3. CDC crossing from clk_a -> LUT -> clk_b across 2 bits without ASYNC_REG (CDC-1, CDC-4, CDC-10, TIMING-6)
        for bit in 0..2 {
            let src = format!("ff_a{bit}");
            let lut = format!("cdc_lut{bit}");
            let dst = format!("ff_b{bit}");
            d.add_cell(&src, CellKind::Hff);
            d.connect("clk_a", &src, "CLK");
            d.connect(&format!("da{bit}"), &src, "D");
            d.connect(&format!("qa{bit}"), &src, "Q");

            d.add_cell(&lut, CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
            d.connect(&format!("qa{bit}"), &lut, "I0");
            d.connect(&format!("mid{bit}"), &lut, "O");

            d.add_cell(&dst, CellKind::Hff);
            d.connect("clk_b", &dst, "CLK");
            d.connect(&format!("mid{bit}"), &dst, "D");
            d.connect(&format!("qb{bit}"), &dst, "Q");
        }

        // 4. Unregistered output IOB driven by LUT (METHODOLOGY-5)
        d.add_cell("u_out_iob", CellKind::IobOut);
        d.connect("mid0", "u_out_iob", "I");
        d.connect("comb_out", "u_out_iob", "PAD");

        // 5. Unpipelined DSP48E1 with illegal opmode (SYNTH-8, REQP-15, AVAL-5)
        d.add_cell(
            "u_dsp",
            CellKind::Dsp48e1 {
                opmode: 0xFF,
                alumode: 0x1F,
            },
        );

        // 6. Unpipelined RAMB18E1 with illegal width and missing CLKARDCLK (SYNTH-9, REQP-10, AVAL-1)
        d.add_cell(
            "u_bram",
            CellKind::Bram18e1 {
                read_width_a: 7,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
            },
        );

        // 7. BlackBox (SYNTH-7)
        d.add_cell(
            "u_bb",
            CellKind::BlackBox {
                module: intern("custom_ip"),
            },
        );

        let rep = check_all_ug906_ug949_decks(&d, None, None, &dev);
        for expected_id in [
            "TIMING-6",
            "TIMING-14",
            "TIMING-17",
            "TIMING-18",
            "SYNTH-2",
            "SYNTH-3",
            "SYNTH-7",
            "SYNTH-8",
            "SYNTH-9",
            "METHODOLOGY-5",
            "METHODOLOGY-8",
            "METHODOLOGY-9",
            "CDC-1",
            "CDC-4",
            "CDC-10",
            "REQP-1",
            "REQP-2",
            "REQP-10",
            "REQP-15",
            "AVAL-1",
            "AVAL-5",
            "AVAL-10",
        ] {
            assert!(
                rep.item(expected_id).is_some(),
                "expected violation {expected_id} in report:\n{}",
                rep.text()
            );
        }
    }

    #[test]
    fn create_waiver_report_waivers_and_remediation_script_generation() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_drive("led", "24").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();

        let full_drc = check_all_ug906_ug949_decks(&d, Some(&pl), None, &dev);
        assert!(full_drc.item("DRIVE-2").is_some());
        assert!(full_drc.item("TIMING-18").is_some());
        assert!(full_drc.item("METHODOLOGY-8").is_some());

        // Generate actionable Tcl/RTL remediation script and verify concrete commands.
        let rem = generate_remediation_script(&full_drc);
        let tcl = rem.to_tcl_script();
        assert!(
            tcl.contains("set_property DRIVE 12 [get_ports {led}]"),
            "remediation Tcl missing DRIVE fix:\n{tcl}"
        );
        assert!(
            tcl.contains("set_input_delay") && tcl.contains("[get_ports {led}]"),
            "remediation Tcl missing TIMING-18 fix:\n{tcl}"
        );
        assert!(
            tcl.contains("create_waiver -id DRIVE-2 -objects {led}"),
            "remediation Tcl missing waiver suggestion:\n{tcl}"
        );

        // Create waivers programmatically and via Vivado Tcl syntax.
        let mut waivers = WaiverSet::new();
        let w1 = waivers
            .parse_create_waiver_tcl(
                "create_waiver -id DRIVE-2 -objects [get_ports {led}] -description \"Board FET buffer accepts 24mA\"",
            )
            .unwrap();
        assert_eq!(w1, 1);
        let w2 = waivers
            .create_waiver("METHODOLOGY-*", vec!["*".into()], "Pre-pinout prototype")
            .unwrap();
        assert_eq!(w2, 2);

        // Missing description or unknown rule ID must fail.
        assert!(waivers.create_waiver("TIMING-18", vec![], "").is_err());
        assert!(
            waivers
                .create_waiver("BOGUS-999", vec![], "not a rule")
                .is_err()
        );

        let filtered = waivers.apply(&full_drc);
        assert!(
            filtered.active.item("DRIVE-2").is_none(),
            "DRIVE-2 on led must be waived"
        );
        assert!(
            filtered.active.item("METHODOLOGY-8").is_none(),
            "METHODOLOGY-* must be waived"
        );
        assert!(
            filtered.active.item("TIMING-18").is_some(),
            "unwaived TIMING-18 must remain active"
        );
        assert!(filtered.waived.len() >= 2);

        let w_rep = waivers.report_waivers();
        assert!(w_rep.contains("report_waivers total_waivers=2"));
        assert!(w_rep.contains("WAIVER#1 id=DRIVE-2 objects=led hits=1"));
        assert!(
            waivers
                .to_tcl_script()
                .contains("create_waiver -id DRIVE-2 -objects {led}")
        );
    }

    #[test]
    fn signoff_rule_table_covers_rtstat_1_to_12_and_bitgen_gates() {
        for n in 1..=12 {
            let id = format!("RTSTAT-{n}");
            let info = signoff_rule_info(&id).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(info.id, id);
            assert!(!info.title.is_empty());
            assert!(info.downgrade_tcl_hint.contains("set_property SEVERITY"));
        }
        for id in [
            "NSTD-1", "UCIO-1", "MDRV-1", "LUTLP-1", "ANT-1", "PLCK-1", "CIPS-1", "BIVC-1",
        ] {
            assert!(signoff_rule_info(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn bitstream_signoff_enforces_nstd_ucio_and_allows_severity_warning_downgrade() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_blinky();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();

        // By default, blinky has no explicit IOSTANDARD or LOC on clk/led -> NSTD-1 and UCIO-1 block bitgen!
        let cfg_strict = SignoffConfig::default();
        let rep_strict = check_bitstream_signoff(&d, &r, &dev, &cfg_strict);
        assert!(!rep_strict.bitgen_allowed);
        assert!(rep_strict.gate_write_bitstream().is_err());
        assert_eq!(rep_strict.nstd_ports, vec!["clk", "led"]);
        assert_eq!(rep_strict.ucio_ports, vec!["clk", "led"]);
        assert_eq!(
            rep_strict.drc.item("NSTD-1").unwrap().severity,
            DrcSeverity::Error
        );
        assert_eq!(
            rep_strict.drc.item("UCIO-1").unwrap().severity,
            DrcSeverity::Error
        );

        // Downgrade NSTD-1 and UCIO-1 to Warning via Vivado Tcl set_property SEVERITY {Warning}:
        let mut cfg_waived = SignoffConfig::default();
        let parsed = cfg_waived
            .parse_tcl_severity_overrides(
                "set_property SEVERITY {Warning} [get_drc_checks NSTD-1]\n\
                 set_property SEVERITY {Warning} [get_drc_checks UCIO-1]",
            )
            .unwrap();
        assert_eq!(parsed, 2);

        let rep_waived = check_bitstream_signoff(&d, &r, &dev, &cfg_waived);
        assert!(
            rep_waived.bitgen_allowed,
            "downgrading NSTD-1 & UCIO-1 to Warning must allow bitgen: {}",
            rep_waived.summary_line()
        );
        rep_waived.gate_write_bitstream().unwrap();
        assert_eq!(
            rep_waived.drc.item("NSTD-1").unwrap().severity,
            DrcSeverity::Warning
        );
        assert_eq!(
            rep_waived.drc.item("UCIO-1").unwrap().severity,
            DrcSeverity::Warning
        );
        assert_eq!(
            rep_waived.downgraded_rules,
            vec!["NSTD-1".to_string(), "UCIO-1".to_string()]
        );
    }

    #[test]
    fn bitstream_signoff_catches_rtstat_mdrv_lutlp_antenna_plck_bivc() {
        use helion_ir::CellKind;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        // Assign incompatible bank VCCO standards on same bank -> BIVC-1.
        d.set_loc("led", "IOB_X2Y0").unwrap();
        d.set_loc("clk", "IOB_X3Y0").unwrap();
        d.set_iostandard("led", "LVCMOS33").unwrap();
        d.set_iostandard("clk", "LVCMOS18").unwrap();

        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let mut r = route(&pl, &dev).unwrap();

        // Inject routing errors: overused tiles (RTSTAT-3 & RTSTAT-12), skipped IMUX (RTSTAT-2),
        // and excessive metal wirelength on net "q" (ANT-1).
        r.overused = 2;
        r.pathfinder_iters = 30;
        r.imux_skip = 1;
        r.iob_src[0].hops = 120;

        // Inject multi-driver net (MDRV-1) and combinational 2-LUT loop (LUTLP-1) and LUT-driven FF CLK (PLCK-1).
        d.add_cell("u_loop_a", CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
        d.add_cell("u_loop_b", CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
        d.connect("loop_ab", "u_loop_a", "O");
        d.connect("loop_ab", "u_loop_b", "I0");
        d.connect("loop_ba", "u_loop_b", "O");
        d.connect("loop_ba", "u_loop_a", "I0");
        // Second driver on loop_ab -> MDRV-1
        d.connect("loop_ab", "u_loop_b", "O");
        // LUT driving sequential FF CLK -> PLCK-1
        d.add_cell("u_bad_clk_ff", CellKind::Hff);
        d.connect("loop_ba", "u_bad_clk_ff", "CLK");
        d.connect("q", "u_bad_clk_ff", "D");

        let cfg = SignoffConfig::default();
        let rep = check_bitstream_signoff(&d, &r, &dev, &cfg);
        assert!(!rep.bitgen_allowed);
        for id in [
            "BIVC-1",
            "RTSTAT-2",
            "RTSTAT-3",
            "RTSTAT-10",
            "RTSTAT-12",
            "MDRV-1",
            "LUTLP-1",
            "ANT-1",
            "PLCK-1",
        ] {
            assert!(
                rep.drc.item(id).is_some(),
                "expected {id} in signoff report:\n{}",
                rep.drc.text()
            );
        }
        assert!(!rep.combinational_loops.is_empty());
        assert!(rep.antenna_metrics.iter().any(|a| a.violated));

        // Adding ANTENNA_DIODE="true" on net "q" must clear ANT-1!
        d.set_net_attr("q", "ANTENNA_DIODE", "true").unwrap();
        let rep_diode = check_bitstream_signoff(&d, &r, &dev, &cfg);
        assert!(
            rep_diode.drc.item("ANT-1").is_none(),
            "ANTENNA_DIODE must clear ANT-1 violation"
        );
        assert!(
            rep_diode
                .antenna_metrics
                .iter()
                .all(|a| a.has_diode_protection && !a.violated)
        );
    }
}
