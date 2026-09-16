//! FM-HEL-L2-UI — Device/Timing highlight + packing English for the GUI.
//!
//! Consumes live `helion-device` Site/Bel/Pip/Net + `resolve_path_sites` /
//! `resolve_cell_site` (HAD tip). `HighlightSet` / packing English stay GUI-local
//! until `wip/learner-L2-pnr` tips those helpers. PIP highlight is out of scope
//! this ship. Soft→RTL span is behind `learner_l2_soft_span` (default off).

use helion_device::{Device, Site, SiteKind};
use helion_place::Placed;
use std::collections::BTreeMap;

/// Sites + nets selected for schematic/device paint after a timing-path pick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighlightSet {
    pub sites: Vec<String>,
    pub nets: Vec<String>,
}

impl HighlightSet {
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty() && self.nets.is_empty()
    }

    /// Headless dump crumb for tests (`SITE=… NET=…`).
    pub fn dump(&self) -> String {
        let mut parts = Vec::new();
        for s in &self.sites {
            parts.push(format!("SITE={s}"));
        }
        for n in &self.nets {
            parts.push(format!("NET={n}"));
        }
        parts.join(" ")
    }
}

/// Build a HighlightSet from path cells/nets using live HAD resolve.
///
/// `endpoints` are `(cell_name, placed Site)` pairs from DeviceView occupancy.
/// Sites are verified via [`Device::resolve_path_sites`]. Nets pass through as
/// design net names (PNR NetId tip not required for this ship).
pub fn highlight_set_from_path(
    device: &Device,
    endpoints: &[(String, Site)],
    nets: &[String],
) -> Result<HighlightSet, String> {
    let sites = device.resolve_path_sites(endpoints)?;
    // Keep only CLB_/IOB_ (and DSP_/BRAM_/CLK_ if present) — all HAD Site::id forms.
    let sites: Vec<String> = sites
        .into_iter()
        .filter(|id| {
            id.starts_with("CLB_")
                || id.starts_with("IOB_")
                || id.starts_with("DSP_")
                || id.starts_with("BRAM_")
                || id.starts_with("CLK_")
        })
        .collect();
    let mut nets: Vec<String> = nets.to_vec();
    nets.sort();
    nets.dedup();
    Ok(HighlightSet { sites, nets })
}

/// Resolve one DeviceView occupant cell → verified SiteId (live HAD).
pub fn resolve_occupant_site(
    device: &Device,
    cell: &str,
    x: u32,
    y: u32,
    kind: SiteKind,
) -> Result<String, String> {
    let site = Site { x, y, kind };
    device.resolve_cell_site(cell, site)
}

/// English packing lines from placement: `CLB_XxYy: N LUTFF` per site.
/// Sum of N over lines == `placed.lutff_sites.len()`.
pub fn packing_summary_english(placed: &Placed) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (site, _ble) in &placed.lutff_sites {
        // Live HAD Site::id (not a GUI string formatter).
        *counts.entry(site.id()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(id, n)| format!("{id}: {n} LUTFF"))
        .collect()
}

/// Soft diagnostic → RTL file:line. No-op unless `learner_l2_soft_span` is on
/// **and** a real span exists — never invents fake spans.
#[cfg(feature = "learner_l2_soft_span")]
pub fn soft_span_jump(_diag: &str) -> Option<(String, usize)> {
    // W-L3 / SV soft IR carries spans later; until then refuse to fake.
    None
}

#[cfg(not(feature = "learner_l2_soft_span"))]
pub fn soft_span_jump(_diag: &str) -> Option<(String, usize)> {
    None
}

/// True if `id` looks like a HAD site id (`CLB_X\d+Y\d+` / `IOB_…`).
pub fn looks_like_had_site(id: &str) -> bool {
    Site::parse_id(id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_summary_groups_by_site_id() {
        let placed = Placed {
            packed: helion_pack::Packed {
                lutffs: vec![],
                iobs: vec![],
                macs: vec![],
                brams: vec![],
            },
            lutff_sites: vec![
                (
                    Site {
                        x: 2,
                        y: 1,
                        kind: SiteKind::Clb,
                    },
                    0,
                ),
                (
                    Site {
                        x: 2,
                        y: 1,
                        kind: SiteKind::Clb,
                    },
                    1,
                ),
                (
                    Site {
                        x: 3,
                        y: 0,
                        kind: SiteKind::Clb,
                    },
                    0,
                ),
            ],
            iob_sites: vec![],
            mac_sites: vec![],
            bram_sites: vec![],
            timing_weight: 0.0,
            cost: 0.0,
        };
        let lines = packing_summary_english(&placed);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|l| l == "CLB_X2Y1: 2 LUTFF"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "CLB_X3Y0: 1 LUTFF"), "{lines:?}");
        let sum: usize = lines
            .iter()
            .filter_map(|l| l.split_once(": ").and_then(|(_, r)| r.strip_suffix(" LUTFF")))
            .filter_map(|n| n.parse::<usize>().ok())
            .sum();
        assert_eq!(sum, placed.lutff_sites.len());
    }

    #[test]
    fn highlight_set_uses_resolve_path_sites() {
        let dev = Device::load_part("HL10T-C32-1").expect("HAD");
        let site = Site {
            x: 2,
            y: 1,
            kind: SiteKind::Clb,
        };
        assert!(dev.contains_site(site), "fixture site in HAD");
        let endpoints = vec![("u_lut0".into(), site)];
        let hs = highlight_set_from_path(&dev, &endpoints, &["clk".into(), "q".into()]).unwrap();
        assert_eq!(hs.sites, vec!["CLB_X2Y1".to_string()]);
        assert!(!hs.nets.is_empty());
        assert!(looks_like_had_site(&hs.sites[0]));
        assert!(hs.dump().contains("SITE=CLB_X2Y1"));
        // Bel / Pip / Net IDs exist on the live device API (smoke).
        assert!(!dev.bels_in_site(site).is_empty());
        assert!(dev.site_by_id("CLB_X2Y1") == Some(site));
    }
}
