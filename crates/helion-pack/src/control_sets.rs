//! UG835 `report_control_sets`: unique (clock, enable, set/reset) triples
//! driving the design's flip-flops, their fanout histogram, and how many
//! slices each one occupies.
//!
//! Slice load counts come from real placement when the caller passes FF
//! sites (one slice = one HAD CLB site); otherwise they are the packing
//! minimum `ceil(ffs / ffs_per_slice)` and flagged as estimates.

use crate::FfControlSet;
use helion_ir::{CellKind, Design, resolve};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// One unique control set and its loads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlSetRow {
    pub clock: String,
    /// `None` = FF enable tied active.
    pub enable: Option<String>,
    /// `None` = no set/reset.
    pub set_reset: Option<String>,
    /// Flip-flops on this control set (Vivado "Bel Load Count").
    pub bel_loads: usize,
    /// Slices holding them (Vivado "Slice Load Count").
    pub slice_loads: usize,
    pub ff_cells: Vec<String>,
}

/// Vivado fanout histogram buckets for control sets.
pub const CONTROL_SET_BUCKETS: [&str; 9] = [
    "1", "2", "3", "4", "5", "6", "7", "8", ">= 9",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlSetReport {
    pub design: String,
    pub rows: Vec<ControlSetRow>,
    /// Count of control sets per [`CONTROL_SET_BUCKETS`] fanout bucket.
    pub histogram: [usize; 9],
    /// Unused FF slots in the slices the control sets occupy
    /// (`slices * ffs_per_slice - flip-flops`).
    pub unused_ff_slots: usize,
    pub ffs_per_slice: usize,
    /// True when slice counts come from placement, false when estimated.
    pub placed: bool,
}

impl ControlSetReport {
    pub fn total(&self) -> usize {
        self.rows.len()
    }

    pub fn total_ffs(&self) -> usize {
        self.rows.iter().map(|r| r.bel_loads).sum()
    }

    pub fn with_enable(&self) -> usize {
        self.rows.iter().filter(|r| r.enable.is_some()).count()
    }

    pub fn with_set_reset(&self) -> usize {
        self.rows.iter().filter(|r| r.set_reset.is_some()).count()
    }

    /// Vivado text report.
    pub fn text(&self) -> String {
        let mut out = format!(
            "report_control_sets design={} control_sets={} ffs={} unused_ff_slots={} slices={}\n",
            self.design,
            self.total(),
            self.total_ffs(),
            self.unused_ff_slots,
            if self.placed { "placed" } else { "estimated" }
        );
        out.push_str("  Fanout  Control sets\n");
        for (i, b) in CONTROL_SET_BUCKETS.iter().enumerate() {
            out.push_str(&format!("  {b:<6}  {}\n", self.histogram[i]));
        }
        out.push_str("  Clock Signal | Enable Signal | Set/Reset Signal | Slice Load Count | Bel Load Count\n");
        for r in &self.rows {
            out.push_str(&format!(
                "  {} | {} | {} | {} | {}\n",
                r.clock,
                r.enable.as_deref().unwrap_or("-"),
                r.set_reset.as_deref().unwrap_or("-"),
                r.slice_loads,
                r.bel_loads
            ));
        }
        out
    }
}

/// Every flip-flop's control set (clock, enable, set/reset nets).
pub fn ff_control_sets(design: &Design) -> Vec<(String, FfControlSet)> {
    let pins = design.pin_index();
    let mut out = Vec::new();
    for c in design.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)) {
        let name = resolve(c.name).to_string();
        let net = |pin: &str| pins.net_on(&name, pin).map(|s| resolve(s).to_string());
        let Some(clk) = net("CLK").or_else(|| net("C")) else {
            continue;
        };
        let ce = net("CE").or_else(|| c.attrs.get("CE_NET").map(|s| s.to_string()));
        let sr = net("SR")
            .or_else(|| net("RST"))
            .or_else(|| net("R"))
            .or_else(|| net("S"))
            .or_else(|| c.attrs.get("SR_NET").map(|s| s.to_string()));
        out.push((name, FfControlSet { clk, ce, sr }));
    }
    out
}

/// Build the control-set report. `ff_sites` maps FF cell → placed slice
/// (HAD CLB `(x, y)`); pass `None` before placement.
pub fn report_control_sets(
    design: &Design,
    ff_sites: Option<&HashMap<String, (u32, u32)>>,
    ffs_per_slice: usize,
) -> ControlSetReport {
    let per_slice = ffs_per_slice.max(1);
    let mut groups: BTreeMap<(String, Option<String>, Option<String>), Vec<String>> =
        BTreeMap::new();
    for (ff, cs) in ff_control_sets(design) {
        groups.entry((cs.clk, cs.ce, cs.sr)).or_default().push(ff);
    }
    let mut rows = Vec::new();
    let mut histogram = [0usize; 9];
    let mut slots = 0usize;
    for ((clock, enable, set_reset), ffs) in groups {
        let slice_loads = match ff_sites {
            Some(sites) => {
                let s: BTreeSet<(u32, u32)> =
                    ffs.iter().filter_map(|f| sites.get(f).copied()).collect();
                s.len().max(usize::from(!ffs.is_empty()))
            }
            None => ffs.len().div_ceil(per_slice),
        };
        let bucket = ffs.len().clamp(1, 9) - 1;
        histogram[bucket] += 1;
        slots += slice_loads * per_slice;
        rows.push(ControlSetRow {
            clock,
            enable,
            set_reset,
            bel_loads: ffs.len(),
            slice_loads,
            ff_cells: ffs,
        });
    }
    rows.sort_by(|a, b| b.bel_loads.cmp(&a.bel_loads).then(a.clock.cmp(&b.clock)));
    let total_ffs: usize = rows.iter().map(|r| r.bel_loads).sum();
    ControlSetReport {
        design: resolve(design.name).to_string(),
        rows,
        histogram,
        unused_ff_slots: slots.saturating_sub(total_ffs),
        ffs_per_slice: per_slice,
        placed: ff_sites.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::PortDir;

    fn design() -> Design {
        let mut d = Design::new("cs");
        d.add_port("clk", PortDir::In);
        d.add_port("clk2", PortDir::In);
        d.add_port("en", PortDir::In);
        d.add_port("rst", PortDir::In);
        for i in 0..5 {
            let ff = format!("a{i}");
            d.add_cell(&ff, CellKind::Hff);
            d.connect("clk", &ff, "CLK");
        }
        for i in 0..3 {
            let ff = format!("b{i}");
            d.add_cell(&ff, CellKind::Hff);
            d.connect("clk", &ff, "CLK");
            d.connect("en", &ff, "CE");
            d.connect("rst", &ff, "SR");
        }
        d.add_cell("c0", CellKind::Hff);
        d.connect("clk2", "c0", "CLK");
        d
    }

    #[test]
    fn groups_flip_flops_by_clock_enable_and_reset() {
        let r = report_control_sets(&design(), None, 4);
        assert_eq!(r.total(), 3);
        assert_eq!(r.total_ffs(), 9);
        assert_eq!(r.with_enable(), 1);
        assert_eq!(r.with_set_reset(), 1);
        let a = &r.rows[0];
        assert_eq!((a.clock.as_str(), a.bel_loads, a.slice_loads), ("clk", 5, 2));
        let b = r.rows.iter().find(|r| r.enable.is_some()).unwrap();
        assert_eq!(b.set_reset.as_deref(), Some("rst"));
        assert_eq!(b.slice_loads, 1);
        // Buckets: fanout 1 (c0), 3 (b*), 5 (a*).
        assert_eq!(r.histogram[0], 1);
        assert_eq!(r.histogram[2], 1);
        assert_eq!(r.histogram[4], 1);
        // 2 + 1 + 1 slices * 4 slots - 9 FFs.
        assert_eq!(r.unused_ff_slots, 7);
        assert!(!r.placed);
        assert!(r.text().contains("control_sets=3"));
    }

    #[test]
    fn placed_sites_drive_slice_counts() {
        let mut sites = HashMap::new();
        for i in 0..5 {
            sites.insert(format!("a{i}"), (i, 0));
        }
        let r = report_control_sets(&design(), Some(&sites), 4);
        assert!(r.placed);
        assert_eq!(r.rows[0].slice_loads, 5, "five FFs spread over five slices");
    }
}
