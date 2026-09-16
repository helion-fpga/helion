//! FM-HEL-L2-UI — Device/Timing highlight + packing English for the GUI.
//!
//! Re-exports PNR crate APIs (`helion_sta::HighlightSet`,
//! `highlight_set_from_path`, `critical_path_highlight`,
//! `helion_place::packing_summary_from_placed`) and keeps thin GUI wrappers
//! for DeviceView endpoint resolve + English packing lines. Soft→RTL span is
//! behind `learner_l2_soft_span` (default off). PIP highlight is out of scope.

use helion_device::{Device, Site, SiteKind};
use helion_place::Placed;

pub use helion_place::packing_summary_from_placed;
pub use helion_sta::{critical_path_highlight, highlight_set_from_path, HighlightSet};

/// Convenience methods on the crate [`HighlightSet`] for headless GUI dumps.
pub trait HighlightSetExt {
    fn is_empty(&self) -> bool;
    /// Headless dump crumb for tests (`SITE=… NET=…`).
    fn dump(&self) -> String;
}

impl HighlightSetExt for HighlightSet {
    fn is_empty(&self) -> bool {
        self.sites.is_empty() && self.nets.is_empty()
    }

    fn dump(&self) -> String {
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

/// Build a crate [`HighlightSet`] from DeviceView path cells/nets via live HAD.
///
/// Prefer [`highlight_set_from_path`] / [`critical_path_highlight`] when a
/// routed [`helion_route::Routed`] + STA [`helion_sta::TimingPath`] are in hand.
/// This adapter covers the GUI timing-table path (endpoints from occupancy).
pub fn highlight_set_from_endpoints(
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

/// English packing lines from placement via crate [`packing_summary_from_placed`].
/// Format: `CLB_XxYy: N LUTFF` per site. Sum of N == `placed.lutff_sites.len()`.
pub fn packing_summary_english(placed: &Placed) -> Vec<String> {
    let sum = packing_summary_from_placed(placed);
    sum.sites
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
        let crate_sum = packing_summary_from_placed(&placed);
        assert_eq!(crate_sum.lutff_in("CLB_X2Y1"), 2);
        assert_eq!(crate_sum.lutff_in("CLB_X3Y0"), 1);
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
        let hs =
            highlight_set_from_endpoints(&dev, &endpoints, &["clk".into(), "q".into()]).unwrap();
        assert_eq!(hs.sites, vec!["CLB_X2Y1".to_string()]);
        assert!(!hs.nets.is_empty());
        assert!(looks_like_had_site(&hs.sites[0]));
        assert!(hs.dump().contains("SITE=CLB_X2Y1"));
        // Bel / Pip / Net IDs exist on the live device API (smoke).
        assert!(!dev.bels_in_site(site).is_empty());
        assert!(dev.site_by_id("CLB_X2Y1") == Some(site));
    }

    #[test]
    fn crate_api_reexports_resolve() {
        // Type-path smoke: GUI re-exports resolve to helion_sta / helion_place.
        let _hl: HighlightSet = HighlightSet::default();
        assert!(_hl.sites.is_empty());
        let _: fn(&Placed) -> helion_pack::PackingSummary = packing_summary_from_placed;
        // critical_path_highlight / highlight_set_from_path are pub-used from helion_sta.
        let _ = std::any::type_name_of_val(&critical_path_highlight);
        let _ = std::any::type_name_of_val(&highlight_set_from_path);
    }
}
