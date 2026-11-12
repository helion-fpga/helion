//! UG903/UG906 Pblock region constraints for the placer.
//!
//! A [`RegionConstraint`] is a rectangle of HAD site coordinates plus the
//! cells it owns (exact cell names, or hierarchical instance prefixes).
//! [`place_with_regions`] runs the normal placer, then legalizes every owned
//! cell into its region: a cell already inside keeps its slot, others move
//! to the free slot of their region nearest to where the placer wanted them
//! (so the wirelength picture survives), and an unconstrained cell squatting
//! in a full region is evicted to the nearest free slot outside every region.
//! A region that cannot hold its cells is an error, never a silent overlap.
//!
//! [`region_violations`] checks a finished placement against the same
//! constraints (the DRC deck reports them as `UTLZ-PB` rows).
//!
//! Membership: a region with no `cells` and no `prefixes` owns the whole
//! design (UG893 `create_pblock` without `add_cells_to_pblock`). When several
//! regions claim a cell, the first region with explicit members wins over a
//! catch-all region; among explicit regions, list order wins.

use crate::{PlaceOpts, Placed, TimingGuide, place_with_guide};
use helion_device::{Device, Site, SiteKind};
use helion_pack::Packed;
use std::collections::HashSet;

/// One floorplan rectangle (inclusive HAD site coordinates) and its members.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionConstraint {
    pub name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// Exact cell names owned by the region.
    pub cells: Vec<String>,
    /// Hierarchical prefixes (`u_core_`, `u_core/`): every cell whose name
    /// starts with one is owned by the region.
    pub prefixes: Vec<String>,
    /// UG909 strict partition: no cell outside the region's membership may
    /// sit inside it (non-members are evicted).
    pub exclusive: bool,
}

impl RegionConstraint {
    pub fn new(name: impl Into<String>, x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Self {
            name: name.into(),
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
            cells: Vec::new(),
            prefixes: Vec::new(),
            exclusive: false,
        }
    }

    /// Mark the region exclusive (UG909 strict reconfigurable partition).
    pub fn exclusive(mut self) -> Self {
        self.exclusive = true;
        self
    }

    pub fn with_cells(mut self, cells: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.cells.extend(cells.into_iter().map(Into::into));
        self
    }

    pub fn with_prefixes(mut self, prefixes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.prefixes.extend(prefixes.into_iter().map(Into::into));
        self
    }

    /// True when the region owns the whole design.
    pub fn is_catch_all(&self) -> bool {
        self.cells.is_empty() && self.prefixes.is_empty()
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Whether `cell` is an explicit member (exact name or prefix match).
    pub fn lists(&self, cell: &str) -> bool {
        !cell.is_empty()
            && (self.cells.iter().any(|c| c == cell)
                || self
                    .prefixes
                    .iter()
                    .any(|p| !p.is_empty() && cell.starts_with(p.as_str())))
    }

    /// `CLB_XaYb:CLB_XcYd` range text.
    pub fn range_text(&self) -> String {
        format!("CLB_X{}Y{}:CLB_X{}Y{}", self.x0, self.y0, self.x1, self.y1)
    }
}

/// Index of the region that owns any of `names` (see module docs).
pub fn owning_region(regions: &[RegionConstraint], names: &[&str]) -> Option<usize> {
    regions
        .iter()
        .position(|r| !r.is_catch_all() && names.iter().any(|n| r.lists(n)))
        .or_else(|| {
            if names.iter().all(|n| n.is_empty()) {
                None
            } else {
                regions.iter().position(RegionConstraint::is_catch_all)
            }
        })
}

/// One placed cell outside the region that owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionViolation {
    pub region: String,
    pub range: String,
    pub cell: String,
    pub site: Site,
}

/// Per-region legalization summary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegionStats {
    pub name: String,
    /// Owned LUT/FF clusters (BLE slots needed).
    pub lutffs: usize,
    /// BLE slots in the region.
    pub capacity: usize,
    /// Owned clusters the placer had to move into the region.
    pub moved: usize,
    /// Unconstrained clusters evicted out of the region to make room.
    pub evicted: usize,
    /// Owned DSP / BRAM cells.
    pub hard_blocks: usize,
}

fn dist(a: (u32, u32), b: (u32, u32)) -> u32 {
    a.0.abs_diff(b.0) + a.1.abs_diff(b.1)
}

/// Place `packed`, then legalize region-owned cells into their regions.
/// Returns the placement plus per-region statistics (same order as
/// `regions`).
pub fn place_with_regions(
    packed: &Packed,
    dev: &Device,
    opts: PlaceOpts,
    regions: &[RegionConstraint],
    guide: &TimingGuide,
) -> Result<(Placed, Vec<RegionStats>), String> {
    let mut placed = place_with_guide(packed, dev, opts, guide)?;
    let stats = legalize_into_regions(&mut placed, dev, regions)?;
    Ok((placed, stats))
}

/// Move region-owned cells of an existing placement into their regions.
pub fn legalize_into_regions(
    placed: &mut Placed,
    dev: &Device,
    regions: &[RegionConstraint],
) -> Result<Vec<RegionStats>, String> {
    let n_ble = dev.n_ble.max(1) as u8;
    let mut stats: Vec<RegionStats> = regions
        .iter()
        .map(|r| RegionStats {
            name: r.name.clone(),
            ..RegionStats::default()
        })
        .collect();
    if regions.is_empty() {
        return Ok(stats);
    }
    let clbs: Vec<Site> = dev.clb_sites().collect();
    let region_slots: Vec<Vec<(Site, u8)>> = regions
        .iter()
        .map(|r| {
            clbs.iter()
                .filter(|s| r.contains(s.x, s.y))
                .flat_map(|s| (0..n_ble).map(move |b| (*s, b)))
                .collect()
        })
        .collect();
    for (i, r) in regions.iter().enumerate() {
        if region_slots[i].is_empty() {
            return Err(format!(
                "pblock {} {} has no HAD CLB sites",
                r.name,
                r.range_text()
            ));
        }
        stats[i].capacity = region_slots[i].len();
    }

    // Owner of every LUTFF cluster.
    let owner: Vec<Option<usize>> = placed
        .packed
        .lutffs
        .iter()
        .map(|lf| owning_region(regions, &[lf.lut_cell.as_str(), lf.ff_cell.as_str()]))
        .collect();
    for (i, o) in owner.iter().enumerate() {
        if let Some(r) = o {
            stats[*r].lutffs += 1;
            let _ = i;
        }
    }
    for (ri, st) in stats.iter().enumerate() {
        if st.lutffs > st.capacity {
            return Err(format!(
                "pblock {} {} is full ({} LUT/FF clusters, {} BLE slots)",
                regions[ri].name,
                regions[ri].range_text(),
                st.lutffs,
                st.capacity
            ));
        }
    }

    let key = |s: &(Site, u8)| (s.0.x, s.0.y, s.1);
    let in_any_region = |x: u32, y: u32| regions.iter().any(|r| r.contains(x, y));
    let mut occupied: HashSet<(u32, u32, u8)> = HashSet::new();
    // Unconstrained clusters hold their slots; owned clusters already
    // inside their region keep theirs.
    let mut pending: Vec<usize> = Vec::new();
    for (i, slot) in placed.lutff_sites.iter().enumerate() {
        match owner[i] {
            None => {
                occupied.insert(key(slot));
            }
            Some(r) if regions[r].contains(slot.0.x, slot.0.y) && occupied.insert(key(slot)) => {}
            Some(_) => pending.push(i),
        }
    }
    // Exclusive regions: clusters not owned by the region leave it first.
    for (ri, r) in regions.iter().enumerate().filter(|(_, r)| r.exclusive) {
        let squatters: Vec<usize> = (0..placed.lutff_sites.len())
            .filter(|&j| owner[j] != Some(ri))
            .filter(|&j| r.contains(placed.lutff_sites[j].0.x, placed.lutff_sites[j].0.y))
            .collect();
        for j in squatters {
            let taken = placed.lutff_sites[j];
            let from = (taken.0.x, taken.0.y);
            let refuge = clbs
                .iter()
                .filter(|s| !regions.iter().any(|x| x.exclusive && x.contains(s.x, s.y)))
                .filter(|s| owner[j].is_none_or(|o| regions[o].contains(s.x, s.y)))
                .flat_map(|s| (0..n_ble).map(move |b| (*s, b)))
                .filter(|s| !occupied.contains(&key(s)))
                .min_by_key(|s| (dist(from, (s.0.x, s.0.y)), s.0.x, s.0.y, s.1))
                .ok_or_else(|| {
                    format!(
                        "pblock {}: no free BLE slot outside it for {}",
                        r.name, placed.packed.lutffs[j].lut_cell
                    )
                })?;
            if occupied.remove(&key(&taken)) {
                occupied.insert(key(&refuge));
            }
            placed.lutff_sites[j] = refuge;
            stats[ri].evicted += 1;
        }
    }
    // Owned clusters move to the nearest free slot of their region,
    // evicting an unconstrained squatter when the region is otherwise full.
    for i in pending {
        let r = owner[i].unwrap_or(0);
        let want = (placed.lutff_sites[i].0.x, placed.lutff_sites[i].0.y);
        let free = region_slots[r]
            .iter()
            .filter(|s| !occupied.contains(&key(s)))
            .min_by_key(|s| (dist(want, (s.0.x, s.0.y)), s.0.x, s.0.y, s.1))
            .copied();
        let slot = match free {
            Some(s) => s,
            None => {
                // Evict the unconstrained cluster closest to `want`.
                let victim = (0..placed.lutff_sites.len())
                    .filter(|&j| owner[j].is_none())
                    .filter(|&j| regions[r].contains(placed.lutff_sites[j].0.x, placed.lutff_sites[j].0.y))
                    .min_by_key(|&j| {
                        let s = placed.lutff_sites[j];
                        dist(want, (s.0.x, s.0.y))
                    })
                    .ok_or_else(|| {
                        format!(
                            "pblock {} {} is full (no free BLE slot for {})",
                            regions[r].name,
                            regions[r].range_text(),
                            placed.packed.lutffs[i].lut_cell
                        )
                    })?;
                let taken = placed.lutff_sites[victim];
                let from = (taken.0.x, taken.0.y);
                let refuge = clbs
                    .iter()
                    .filter(|s| !in_any_region(s.x, s.y))
                    .flat_map(|s| (0..n_ble).map(move |b| (*s, b)))
                    .filter(|s| !occupied.contains(&key(s)))
                    .min_by_key(|s| (dist(from, (s.0.x, s.0.y)), s.0.x, s.0.y, s.1))
                    .ok_or_else(|| {
                        format!(
                            "pblock {}: no free BLE slot outside the pblocks to evict {}",
                            regions[r].name, placed.packed.lutffs[victim].lut_cell
                        )
                    })?;
                occupied.remove(&key(&taken));
                occupied.insert(key(&refuge));
                placed.lutff_sites[victim] = refuge;
                stats[r].evicted += 1;
                taken
            }
        };
        occupied.insert(key(&slot));
        placed.lutff_sites[i] = slot;
        stats[r].moved += 1;
    }

    // Hard blocks (DSP / BRAM): move owned cells onto a free site of the
    // matching kind inside the region when the region covers one.
    legalize_hard(
        &placed.packed.macs.iter().map(|m| m.cell.clone()).collect::<Vec<_>>(),
        &mut placed.mac_sites,
        dev.dsp_sites().collect(),
        SiteKind::Dsp,
        regions,
        &mut stats,
    )?;
    legalize_hard(
        &placed.packed.brams.iter().map(|b| b.cell.clone()).collect::<Vec<_>>(),
        &mut placed.bram_sites,
        dev.bram_sites().collect(),
        SiteKind::Bram,
        regions,
        &mut stats,
    )?;
    Ok(stats)
}

fn legalize_hard(
    cells: &[String],
    sites: &mut [Site],
    all: Vec<Site>,
    kind: SiteKind,
    regions: &[RegionConstraint],
    stats: &mut [RegionStats],
) -> Result<(), String> {
    let mut used: HashSet<(u32, u32)> = sites.iter().map(|s| (s.x, s.y)).collect();
    for (i, cell) in cells.iter().enumerate() {
        let Some(r) = owning_region(regions, &[cell.as_str()]) else {
            continue;
        };
        // A catch-all region only binds hard blocks when it covers a site
        // of their kind (fabric pblocks need not span a DSP/BRAM column).
        let in_region: Vec<Site> = all
            .iter()
            .filter(|s| s.kind == kind && regions[r].contains(s.x, s.y))
            .copied()
            .collect();
        if in_region.is_empty() {
            if regions[r].is_catch_all() {
                continue;
            }
            return Err(format!(
                "pblock {} {} covers no {:?} site for {cell}",
                regions[r].name,
                regions[r].range_text(),
                kind
            ));
        }
        stats[r].hard_blocks += 1;
        let Some(cur) = sites.get(i).copied() else {
            continue;
        };
        if regions[r].contains(cur.x, cur.y) {
            continue;
        }
        let pick = in_region
            .iter()
            .filter(|s| !used.contains(&(s.x, s.y)))
            .min_by_key(|s| (dist((cur.x, cur.y), (s.x, s.y)), s.x, s.y))
            .copied()
            .ok_or_else(|| {
                format!(
                    "pblock {} {} has no free {:?} site for {cell}",
                    regions[r].name,
                    regions[r].range_text(),
                    kind
                )
            })?;
        used.remove(&(cur.x, cur.y));
        used.insert((pick.x, pick.y));
        sites[i] = pick;
    }
    Ok(())
}

/// Every region-owned cell placed outside its region.
pub fn region_violations(placed: &Placed, regions: &[RegionConstraint]) -> Vec<RegionViolation> {
    let mut out = Vec::new();
    let mut push = |r: usize, cell: &str, site: Site| {
        if !regions[r].contains(site.x, site.y) {
            out.push(RegionViolation {
                region: regions[r].name.clone(),
                range: regions[r].range_text(),
                cell: cell.to_string(),
                site,
            });
        }
    };
    for (i, lf) in placed.packed.lutffs.iter().enumerate() {
        let Some(&(site, _)) = placed.lutff_sites.get(i) else {
            continue;
        };
        if let Some(r) = owning_region(regions, &[lf.lut_cell.as_str(), lf.ff_cell.as_str()]) {
            let name = if lf.lut_cell.is_empty() { &lf.ff_cell } else { &lf.lut_cell };
            push(r, name, site);
        }
    }
    for (i, m) in placed.packed.macs.iter().enumerate() {
        if let (Some(r), Some(site)) = (owning_region(regions, &[m.cell.as_str()]), placed.mac_sites.get(i)) {
            if !regions[r].is_catch_all() {
                push(r, &m.cell, *site);
            }
        }
    }
    for (i, b) in placed.packed.brams.iter().enumerate() {
        if let (Some(r), Some(site)) = (owning_region(regions, &[b.cell.as_str()]), placed.bram_sites.get(i)) {
            if !regions[r].is_catch_all() {
                push(r, &b.cell, *site);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::Design;
    use helion_pack::pack;

    fn dev() -> Device {
        Device::load_part("HL10T-C32-1").unwrap()
    }

    fn opts() -> PlaceOpts {
        PlaceOpts {
            timing_weight: 0.75,
            seed: 0,
        }
    }

    #[test]
    fn named_cells_land_inside_their_pblock_and_others_stay() {
        let dev = dev();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let free = place_with_guide(&p, &dev, opts(), &TimingGuide::default()).unwrap();
        let r = RegionConstraint::new("pb_hi", 6, 6, 8, 8).with_cells(["u_lut3", "u_lut2"]);
        let (pl, stats) =
            place_with_regions(&p, &dev, opts(), &[r.clone()], &TimingGuide::default()).unwrap();
        assert!(region_violations(&pl, &[r.clone()]).is_empty());
        assert_eq!(stats[0].lutffs, 2);
        for (i, lf) in pl.packed.lutffs.iter().enumerate() {
            let s = pl.lutff_sites[i].0;
            if lf.lut_cell == "u_lut3" || lf.lut_cell == "u_lut2" {
                assert!(r.contains(s.x, s.y), "{} at {s:?}", lf.lut_cell);
            } else {
                assert_eq!(pl.lutff_sites[i], free.lutff_sites[i], "unowned {} moved", lf.lut_cell);
            }
        }
        // No two clusters share a BLE.
        let set: HashSet<_> = pl.lutff_sites.iter().map(|(s, b)| (s.x, s.y, *b)).collect();
        assert_eq!(set.len(), pl.lutff_sites.len());
    }

    #[test]
    fn two_regions_split_the_design() {
        let dev = dev();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let a = RegionConstraint::new("pb_a", 1, 1, 2, 2).with_cells(["u_lut0", "u_lut1"]);
        let b = RegionConstraint::new("pb_b", 7, 7, 8, 8).with_prefixes(["u_ff2", "u_lut3"]);
        let regions = vec![a.clone(), b.clone()];
        let (pl, stats) =
            place_with_regions(&p, &dev, opts(), &regions, &TimingGuide::default()).unwrap();
        assert!(region_violations(&pl, &regions).is_empty());
        assert_eq!(stats[0].lutffs, 2);
        assert_eq!(stats[1].lutffs, 2);
    }

    #[test]
    fn catch_all_region_holds_every_cluster() {
        let dev = dev();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let r = RegionConstraint::new("pb_all", 4, 4, 5, 5);
        let (pl, _) = place_with_regions(&p, &dev, opts(), &[r.clone()], &TimingGuide::default())
            .unwrap();
        for (s, _) in &pl.lutff_sites {
            assert!(r.contains(s.x, s.y));
        }
    }

    #[test]
    fn overfull_region_is_an_error_not_an_overlap() {
        let dev = dev();
        let mut d = Design::new("wide");
        d.add_port("clk", helion_ir::PortDir::In);
        let n = dev.n_ble as usize + 1;
        for i in 0..n {
            d.add_cell(&format!("u_lut{i}"), helion_ir::CellKind::Lut6 { init: 2 });
            d.add_cell(&format!("u_ff{i}"), helion_ir::CellKind::Hff);
            d.connect("clk", &format!("u_ff{i}"), "CLK");
            d.connect(&format!("d{i}"), &format!("u_lut{i}"), "O");
            d.connect(&format!("d{i}"), &format!("u_ff{i}"), "D");
            d.connect(&format!("q{i}"), &format!("u_ff{i}"), "Q");
            d.connect(&format!("q{i}"), &format!("u_lut{i}"), "I0");
        }
        let p = pack(&d, &dev).unwrap();
        let r = RegionConstraint::new("pb_tiny", 3, 3, 3, 3);
        let err = place_with_regions(&p, &dev, opts(), &[r], &TimingGuide::default()).unwrap_err();
        assert!(err.contains("is full"), "{err}");
    }

    #[test]
    fn violations_report_cells_outside_their_region() {
        let dev = dev();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place_with_guide(&p, &dev, opts(), &TimingGuide::default()).unwrap();
        // A region far from where the unconstrained placer puts u_lut0.
        let s0 = pl.lutff_sites[0].0;
        let (x, y) = if s0.x > 4 { (1, 1) } else { (8, 8) };
        let r = RegionConstraint::new("pb_far", x, y, x, y).with_cells([pl.packed.lutffs[0].lut_cell.clone()]);
        let v = region_violations(&pl, &[r]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].region, "pb_far");
    }

    #[test]
    fn exclusive_region_evicts_non_members() {
        let dev = dev();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let free = place_with_guide(&p, &dev, opts(), &TimingGuide::default()).unwrap();
        // Cover where the placer put the first cluster, but own a different cell.
        let s0 = free.lutff_sites[0].0;
        let other = free.packed.lutffs[1].lut_cell.clone();
        let r = RegionConstraint::new("rp", s0.x, s0.y, s0.x, s0.y).with_cells([other]).exclusive();
        let (pl, st) = place_with_regions(&p, &dev, opts(), &[r.clone()], &TimingGuide::default()).unwrap();
        for (i, (s, _)) in pl.lutff_sites.iter().enumerate() {
            let owned = i == 1;
            assert_eq!(r.contains(s.x, s.y), owned, "cluster {i} at {s:?}");
        }
        assert!(st[0].evicted >= 1);
    }

    #[test]
    fn empty_region_rectangle_is_rejected() {
        let dev = dev();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let r = RegionConstraint::new("pb_void", 500, 500, 501, 501).with_cells(["u_lut0"]);
        let err = place_with_regions(&p, &dev, opts(), &[r], &TimingGuide::default()).unwrap_err();
        assert!(err.contains("no HAD CLB sites"), "{err}");
    }
}
