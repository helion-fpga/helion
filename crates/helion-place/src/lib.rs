//! Place packed clusters onto Helion sites from the device database.
//!
//! `place_with_guide` / `place_in_region_with_guide` / `place_incremental_with_guide`
//! consume XDC IOB-arc exceptions (`TimingGuide`). helion-cli and helion-proj
//! still call unguided `place_with` / `place_in_region` / `place_incremental`
//! until a later crate lock wires `Constraints::timing_guide()` through.

use helion_device::{BelId, Device, Site, SiteKind};
use helion_pack::Packed;
use std::collections::HashSet;

pub use helion_pack::PackingSummary;

#[derive(Clone, Debug)]
pub struct Placed {
    pub packed: Packed,
    pub lutff_sites: Vec<(Site, u8)>,
    pub iob_sites: Vec<Site>,
    pub mac_sites: Vec<Site>,
    pub bram_sites: Vec<Site>,
    pub timing_weight: f64,
    pub cost: f64,
}

impl Placed {
    /// HAD site occupied by a packed HNF cell, if placed.
    pub fn cell_site(&self, cell: &str) -> Option<Site> {
        self.cell_bel(cell).map(|b| b.site)
    }

    /// HAD BEL for a packed HNF cell (`BLE{i}.LUT` / `BLE{i}.FF` / `IOB` / `MAC` / `RAM18`).
    pub fn cell_bel(&self, cell: &str) -> Option<BelId> {
        if cell.is_empty() {
            return None;
        }
        for (i, lf) in self.packed.lutffs.iter().enumerate() {
            let Some(&(site, ble)) = self.lutff_sites.get(i) else {
                continue;
            };
            if lf.lut_cell == cell {
                return Some(BelId::new(site, format!("BLE{ble}.LUT")));
            }
            if lf.ff_cell == cell {
                return Some(BelId::new(site, format!("BLE{ble}.FF")));
            }
        }
        for (i, iob) in self.packed.iobs.iter().enumerate() {
            if iob.cell == cell {
                return self.iob_sites.get(i).copied().map(|s| BelId::new(s, "IOB"));
            }
        }
        for (i, mac) in self.packed.macs.iter().enumerate() {
            if mac.cell == cell {
                return self.mac_sites.get(i).copied().map(|s| BelId::new(s, "MAC"));
            }
        }
        for (i, bram) in self.packed.brams.iter().enumerate() {
            if bram.cell == cell {
                return self
                    .bram_sites
                    .get(i)
                    .copied()
                    .map(|s| BelId::new(s, "RAM18"));
            }
        }
        None
    }
}

/// LUTFF counts per HAD CLB site ID, derived from placement.
pub fn packing_summary_from_placed(placed: &Placed) -> PackingSummary {
    PackingSummary::from_lutff_sites(&placed.lutff_sites)
}

#[derive(Clone, Copy, Debug)]
pub struct PlaceOpts {
    /// 0 = wirelength toward array center. >0 = timing: pull toward IOB.
    pub timing_weight: f64,
    /// Deterministic perturbation seed for design-run seed sweeps.
    /// 0 = legacy processing order (bit-identical to previous releases).
    /// Non-zero = seeded Fisher-Yates shuffle of the initial LUTFF
    /// processing order, so different seeds explore different placements.
    pub seed: u64,
}

impl Default for PlaceOpts {
    fn default() -> Self {
        Self {
            timing_weight: 0.0,
            seed: 0,
        }
    }
}

/// SplitMix64: tiny deterministic PRNG for seeded placement perturbation.
/// Hand-rolled so `helion-place` gains no new dependencies.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic Fisher-Yates shuffle of `0..n` driven by `seed`.
/// Exported for unit tests (seed sweep determinism).
pub fn seeded_order(n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    if seed == 0 {
        return order;
    }
    let mut state = seed;
    for i in (1..n).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    order
}

/// IOB-arc exceptions from XDC (`set_false_path` / `set_multicycle_path`).
/// Empty guide keeps gold south/mid pull from `timing_weight` alone.
/// False-path / multicycle flags apply only to clusters that drive an
/// excepted IOB/output net — internal LUT+FF keep `timing_weight` south pull.
#[derive(Clone, Debug, Default)]
pub struct TimingGuide {
    /// `set_false_path` covers an IOB/output arc — do not pull that driver toward IOB.
    pub false_path_iob: bool,
    /// Setup multicycle for IOB arcs. 0 or 1 = single cycle (gold).
    pub iob_setup_mult: u32,
    /// Per-net criticality (0.0 = non-critical, 1.0 = most critical).
    /// Used for timing-driven placement: critical nets get higher weight.
    pub net_criticality: std::collections::HashMap<String, f32>,
}

impl TimingGuide {
    /// PathFinder dogleg width (columns) for excepted IOB nets. 0 = gold
    /// shortest path. False path takes a wider detour than multicycle so the
    /// routed path/hops actually change (not a post-route hop pad).
    pub fn iob_detour_cols(&self) -> u32 {
        if self.false_path_iob {
            2
        } else if self.iob_setup_mult > 1 {
            1
        } else {
            0
        }
    }

    /// True when this cluster's Q net drives an IOB that the guide excepts.
    pub fn excepts_iob_driver(&self, drives_iob: bool) -> bool {
        drives_iob && (self.false_path_iob || self.iob_setup_mult > 1)
    }

    /// Get criticality for a net (0.0 = non-critical, 1.0 = most critical).
    /// Returns 0.0 if net not in map (default non-critical).
    pub fn net_criticality(&self, net_name: &str) -> f32 {
        self.net_criticality.get(net_name).copied().unwrap_or(0.0)
    }

    /// Get maximum criticality among a list of net names.
    pub fn max_criticality(&self, nets: &[String]) -> f32 {
        nets.iter()
            .map(|n| self.net_criticality(n))
            .fold(0.0f32, f32::max)
    }
}

/// Y used as the affinity base: south (timing), mid (wirelength / false path),
/// or quarter-from-south (multicycle slack).
/// `excepted_iob_cluster` is set only for clusters that drive an excepted IOB
/// net; other clusters keep timing_weight south pull.
fn affinity_base_y(
    col: &[Site],
    opts: PlaceOpts,
    guide: &TimingGuide,
    excepted_iob_cluster: bool,
) -> u32 {
    if col.is_empty() {
        return 0;
    }
    if opts.timing_weight <= 0.0 {
        col[col.len() / 2].y
    } else if excepted_iob_cluster && guide.false_path_iob {
        col[col.len() / 2].y
    } else if excepted_iob_cluster && guide.iob_setup_mult > 1 {
        col[col.len() / 4].y
    } else {
        col[0].y
    }
}

/// Bring-up IMUX reach: same CLB, axis ±1..±4, diag ±1/±2, knight
/// (±2,±1)/(±1,±2) — matches helion-route::imux_sel.
fn imux_local(from: Site, to: Site) -> bool {
    let dx = from.x.abs_diff(to.x);
    let dy = from.y.abs_diff(to.y);
    if dx == 0 && dy <= 4 {
        return true;
    }
    if dy == 0 && dx <= 3 {
        return true;
    }
    // Diagonal ±1 / ±2, knight (±2,±1)/(±1,±2)
    (dx == 1 && dy == 1) || (dx == 2 && dy == 2) || (dx == 2 && dy == 1) || (dx == 1 && dy == 2)
}

#[allow(dead_code)]
fn imux_illegal_pins(
    lf: &helion_pack::PackedLutFf,
    site: Site,
    ff_at: &std::collections::HashMap<String, Site>,
) -> u32 {
    imux_score(lf, site, ff_at).0
}

/// Single pin walk: (illegal_count, manhattan_spill). Spill is Manhattan over
/// out-of-reach arcs for gradient legalize. Kept as the HashMap reference
/// scorer; the budgeted hot path uses `imux_score_idx`.
#[allow(dead_code)]
fn imux_score(
    lf: &helion_pack::PackedLutFf,
    site: Site,
    ff_at: &std::collections::HashMap<String, Site>,
) -> (u32, u32) {
    let mut n = 0u32;
    let mut spill = 0u32;
    for (_, driver) in &lf.lut_pins {
        match ff_at.get(driver.as_str()) {
            Some(ds) if imux_local(*ds, site) => {}
            Some(ds) => {
                n += 1;
                spill += ds.x.abs_diff(site.x) + ds.y.abs_diff(site.y);
            }
            None => {}
        }
    }
    (n, spill)
}

/// Dense (x,y) occupancy index for bileg legalize. O(1) site, occupancy, BLE.
struct LegalizeGrid {
    gx: usize,
    gy: usize,
    n_ble: usize,
    sites: Vec<Option<Site>>,
    ble: Vec<Option<usize>>,
    occ: Vec<u8>,
}

impl LegalizeGrid {
    fn new(max_x: u32, max_y: u32, n_ble: usize) -> Self {
        let gx = max_x as usize + 1;
        let gy = max_y as usize + 1;
        let cells = gx.saturating_mul(gy);
        Self {
            gx,
            gy,
            n_ble: n_ble.max(1),
            sites: vec![None; cells],
            ble: vec![None; cells.saturating_mul(n_ble.max(1))],
            occ: vec![0; cells],
        }
    }

    #[inline]
    fn cell(&self, x: u32, y: u32) -> Option<usize> {
        let x = x as usize;
        let y = y as usize;
        if x < self.gx && y < self.gy {
            Some(y * self.gx + x)
        } else {
            None
        }
    }

    fn set_site(&mut self, s: Site) {
        if let Some(c) = self.cell(s.x, s.y) {
            self.sites[c] = Some(s);
        }
    }

    #[inline]
    fn site_xy(&self, x: u32, y: u32) -> Option<Site> {
        self.cell(x, y).and_then(|c| self.sites[c])
    }

    fn place_at(&mut self, x: u32, y: u32, ble: u8, idx: usize) {
        let Some(c) = self.cell(x, y) else {
            return;
        };
        let slot = c * self.n_ble + ble as usize;
        self.ble[slot] = Some(idx);
        self.occ[c] = self.occ[c].saturating_add(1);
    }

    fn take_ble(&mut self, x: u32, y: u32, ble: u8) -> Option<usize> {
        let c = self.cell(x, y)?;
        let slot = c * self.n_ble + ble as usize;
        let idx = self.ble[slot].take();
        if idx.is_some() {
            self.occ[c] = self.occ[c].saturating_sub(1);
        }
        idx
    }

    fn relocate(&mut self, from: (u32, u32, u8), to: (u32, u32, u8), idx: usize) {
        let _ = self.take_ble(from.0, from.1, from.2);
        self.place_at(to.0, to.1, to.2, idx);
    }

    fn swap_bles(&mut self, a: (u32, u32, u8), b: (u32, u32, u8), ia: usize, ib: usize) {
        let Some(ca) = self.cell(a.0, a.1) else {
            return;
        };
        let Some(cb) = self.cell(b.0, b.1) else {
            return;
        };
        let sa = ca * self.n_ble + a.2 as usize;
        let sb = cb * self.n_ble + b.2 as usize;
        self.ble[sa] = Some(ib);
        self.ble[sb] = Some(ia);
    }

    fn first_free_ble(&self, x: u32, y: u32, skip: (u32, u32, u8)) -> Option<u8> {
        let c = self.cell(x, y)?;
        if self.sites[c].is_none() {
            return None;
        }
        let same = skip.0 == x && skip.1 == y;
        if self.occ[c] as usize >= self.n_ble && !same {
            return None;
        }
        let base = c * self.n_ble;
        for b in 0..self.n_ble {
            if same && skip.2 as usize == b {
                continue;
            }
            if self.ble[base + b].is_none() {
                return Some(b as u8);
            }
        }
        None
    }

    #[inline]
    fn occ(&self, x: u32, y: u32) -> u8 {
        self.cell(x, y).map(|c| self.occ[c]).unwrap_or(0)
    }

    #[inline]
    fn occupant(&self, x: u32, y: u32, ble: u8) -> Option<usize> {
        let c = self.cell(x, y)?;
        self.ble[c * self.n_ble + ble as usize]
    }
}

/// Hypothetical cluster site during a swap (FF location override). Empty-BLE
/// scoring leaves driver sites unchanged — same as the HashMap `ff_at` path.
#[inline]
fn cluster_site(
    idx: usize,
    lutff_sites: &[(Site, u8)],
    ov_a: Option<(usize, Site)>,
    ov_b: Option<(usize, Site)>,
) -> Site {
    if let Some((i, s)) = ov_a {
        if i == idx {
            return s;
        }
    }
    if let Some((i, s)) = ov_b {
        if i == idx {
            return s;
        }
    }
    lutff_sites[idx].0
}

fn imux_score_idx(
    pin_drv: &[Option<usize>],
    site: Site,
    lutff_sites: &[(Site, u8)],
    ov_a: Option<(usize, Site)>,
    ov_b: Option<(usize, Site)>,
) -> (u32, u32) {
    let mut n = 0u32;
    let mut spill = 0u32;
    for &d in pin_drv {
        let Some(di) = d else {
            continue;
        };
        let ds = cluster_site(di, lutff_sites, ov_a, ov_b);
        if imux_local(ds, site) {
            continue;
        }
        n += 1;
        spill += ds.x.abs_diff(site.x) + ds.y.abs_diff(site.y);
    }
    (n, spill)
}

/// Driver score: (illegal_fanout, fan_spill, own_input_illegal).
/// Sink *sites* stay at `lutff_sites` (pre-swap) — matches the historical
/// bileg scorer, which mutated `ff_at` but not sink coordinates.
fn drv_score_idx(
    d_idx: usize,
    d_site: Site,
    sinks: &[(usize, u32)],
    pin_drv: &[Vec<Option<usize>>],
    lutff_sites: &[(Site, u8)],
    ov_a: Option<(usize, Site)>,
    ov_b: Option<(usize, Site)>,
) -> (u32, u32, u32) {
    let mut fan_illegal = 0u32;
    let mut fan_spill = 0u32;
    for &(si, pins) in sinks {
        let ss = lutff_sites[si].0;
        if !imux_local(d_site, ss) {
            fan_illegal += pins;
            fan_spill += pins * (d_site.x.abs_diff(ss.x) + d_site.y.abs_diff(ss.y));
        }
    }
    (
        fan_illegal,
        fan_spill,
        imux_score_idx(&pin_drv[d_idx], d_site, lutff_sites, ov_a, ov_b).0,
    )
}

fn parse_iob_loc(loc: &str, sites: &[Site]) -> Option<Site> {
    let site = Site::parse_id(loc)?;
    if site.kind != SiteKind::Iob {
        return None;
    }
    sites.iter().copied().find(|s| *s == site)
}

/// ≥8 LUT+FF heartbeat fixture (Ibex pin-wrap class, not 108k P&R).
/// Shared by place/route/sta guided-timing tests.
pub fn hard_heartbeat() -> helion_ir::Design {
    use helion_ir::{CellKind, PortDir};
    let mut d = helion_ir::Design::new("hb8");
    d.add_port("clk", PortDir::In);
    d.add_port("led", PortDir::Out);
    for i in 0..8u32 {
        d.add_cell(
            &format!("u_lut{i}"),
            CellKind::Lut6 {
                init: 0x5555_5555_5555_5555,
            },
        );
        d.add_cell(&format!("u_ff{i}"), CellKind::Hff);
        d.connect("clk", &format!("u_ff{i}"), "CLK");
        d.connect(&format!("d{i}"), &format!("u_lut{i}"), "O");
        d.connect(&format!("d{i}"), &format!("u_ff{i}"), "D");
        d.connect(&format!("q{i}"), &format!("u_ff{i}"), "Q");
        d.connect(&format!("q{i}"), &format!("u_lut{i}"), "I0");
        if i > 0 {
            d.connect(&format!("q{}", i - 1), &format!("u_lut{i}"), "I1");
        }
    }
    d.add_cell("u_iob", CellKind::IobOut);
    d.connect("q7", "u_iob", "I");
    d.connect("led", "u_iob", "PAD");
    d
}

pub fn place(packed: &Packed, dev: &Device) -> Result<Placed, String> {
    place_with(packed, dev, PlaceOpts::default())
}

pub fn place_with(packed: &Packed, dev: &Device, opts: PlaceOpts) -> Result<Placed, String> {
    place_with_guide(packed, dev, opts, &TimingGuide::default())
}

/// Place with XDC IOB-arc exceptions. Empty `guide` matches `place_with`.
/// helion-cli / helion-proj still call unguided `place_with` until a later lock.
pub fn place_with_guide(
    packed: &Packed,
    dev: &Device,
    opts: PlaceOpts,
    guide: &TimingGuide,
) -> Result<Placed, String> {
    let iob_all: Vec<Site> = dev.iob_sites().collect();
    let iob_take = packed.iobs.len().min(iob_all.len());
    if packed.iobs.len() > iob_all.len() {
        eprintln!(
            "hang_diag place_iob_cap {} -> {} (device IOB sites)",
            packed.iobs.len(),
            iob_take
        );
    }
    // Prefer PACKAGE_PIN / LOC IOBs when capping to the device budget so board
    // pins (led) survive mid-size fabrics that emit hundreds of tied IOBs.
    let mut order: Vec<usize> = (0..packed.iobs.len()).collect();
    order.sort_by_key(|&i| if packed.iobs[i].loc.is_some() { 0 } else { 1 });
    order.truncate(iob_take);
    let mut iob_sites = Vec::with_capacity(order.len());
    let mut claimed: HashSet<(u32, u32)> = HashSet::new();
    for &i in &order {
        let iob = &packed.iobs[i];
        let s = if let Some(loc) = &iob.loc {
            parse_iob_loc(loc, &iob_all).ok_or_else(|| format!("LOC {loc} is not an IOB site"))?
        } else {
            *iob_all
                .iter()
                .find(|s| !claimed.contains(&(s.x, s.y)))
                .ok_or_else(|| {
                    format!(
                        "need {} IOB sites, device has {}",
                        packed.iobs.len(),
                        iob_all.len()
                    )
                })?
        };
        claimed.insert((s.x, s.y));
        iob_sites.push(s);
    }
    // Keep packed.iobs aligned with iob_sites after the LOC-preferring cap.
    let mut packed = packed.clone();
    packed.iobs = order.iter().map(|&i| packed.iobs[i].clone()).collect();

    let mut lutff_sites: Vec<(Site, u8)> = Vec::new();
    if !packed.lutffs.is_empty() {
        let t_aff = std::time::Instant::now();
        // Prefer the IOB column that each cluster drives (multi-IOB comb mux).
        // Single-IOB designs (counter/blinky gold) still pack into one column.
        let fallback_iob = iob_sites
            .first()
            .copied()
            .or_else(|| iob_all.first().copied())
            .ok_or_else(|| "need IOB column for LUTFF".to_string())?;
        let n_ble = dev.n_ble.max(1) as usize;
        let mut cols: std::collections::HashMap<u32, Vec<Site>> = std::collections::HashMap::new();
        for s in dev.clb_sites() {
            cols.entry(s.x).or_default().push(s);
        }
        for v in cols.values_mut() {
            v.sort_by_key(|s| s.y);
        }
        // Precompute global column order once (Ibex-scale: avoid re-sort per LUTFF).
        let mut all_xs: Vec<u32> = cols.keys().copied().collect();
        all_xs.sort_unstable();
        let mut iob_for_net: std::collections::HashMap<&str, Site> =
            std::collections::HashMap::new();
        for (ii, iob) in packed.iobs.iter().enumerate() {
            if let Some(site) = iob_sites.get(ii) {
                iob_for_net.insert(iob.from_net.as_str(), *site);
            }
        }
        let mut used: HashSet<(u32, u32, u8)> = HashSet::new();
        // Free BLE counts: skip exhausted columns/sites without HashSet probes.
        let mut col_free: std::collections::HashMap<u32, usize> = cols
            .iter()
            .map(|(&x, col)| (x, col.len() * n_ble))
            .collect();
        let mut site_used_n: std::collections::HashMap<(u32, u32), u8> =
            std::collections::HashMap::new();
        let mut xs_seen: HashSet<u32> = HashSet::new();
        let mut y_seen: HashSet<u32> = HashSet::new();
        // FF cell → site as we place (IMUX: same-CLB / N-S±1/±2 / E-W±1/±2 / diag±1 / knight).
        let mut ff_at: std::collections::HashMap<&str, Site> = std::collections::HashMap::new();
        let mut excepted_iob: HashSet<usize> = HashSet::new();
        // Seed sweeps: process LUTFFs in a seeded order so different seeds
        // explore genuinely different placements. Seed 0 = legacy order.
        // Sites are recorded by PACKED index, never in visitation order:
        // lutff_sites[i] must match packed.lutffs[i] (INIT programming,
        // router IMUX, cell_bel, lutff_of all index by packed order).
        let lutff_order = {
            // Timing-driven order: sort by criticality (descending) so critical
            // cells are placed first in better locations. Falls back to seeded
            // order if no criticality info available.
            let mut order: Vec<usize> = (0..packed.lutffs.len()).collect();
            // Check if guide has any criticality info
            let has_criticality = !guide.net_criticality.is_empty();
            if has_criticality {
                order.sort_by(|&a, &b| {
                    let crit_a = guide.net_criticality(&packed.lutffs[a].q_net);
                    let crit_b = guide.net_criticality(&packed.lutffs[b].q_net);
                    // Sort descending by criticality (higher first)
                    crit_b.partial_cmp(&crit_a).unwrap_or(std::cmp::Ordering::Equal)
                });
            } else {
                order = seeded_order(packed.lutffs.len(), opts.seed);
            }
            order
        };
        let mut site_of: Vec<Option<(Site, u8)>> = vec![None; packed.lutffs.len()];
        for &li in &lutff_order {
            let lf = &packed.lutffs[li];
            let drives_iob = iob_for_net.contains_key(lf.q_net.as_str());
            let excepted_iob_cluster = guide.excepts_iob_driver(drives_iob);
            let preferred_x = iob_for_net
                .get(lf.q_net.as_str())
                .map(|s| s.x)
                .unwrap_or(fallback_iob.x);
            // Affinity: already-placed LUT-pin drivers (same CLB, then Y±1).
            let mut affinity: Vec<Site> = Vec::new();
            for (_, driver) in &lf.lut_pins {
                if let Some(s) = ff_at.get(driver.as_str()) {
                    affinity.push(*s);
                }
            }
            // Primary xs: affinity ±E-W then preferred IOB — same order as before,
            // but defer full-die all_xs until primary fails (equiv. via dedup).
            let mut try_xs: Vec<u32> = Vec::with_capacity(8 + affinity.len() * 5);
            for s in &affinity {
                try_xs.push(s.x);
                // Harder cluster: keep sink in E-W±1/±2 of drivers before sprawl.
                try_xs.push(s.x.saturating_add(1));
                try_xs.push(s.x.saturating_add(2));
                try_xs.push(s.x.saturating_add(3));
                if s.x > 0 {
                    try_xs.push(s.x - 1);
                }
                if s.x > 1 {
                    try_xs.push(s.x - 2);
                }
                if s.x > 2 {
                    try_xs.push(s.x - 3);
                }
            }
            try_xs.push(preferred_x);
            if fallback_iob.x != preferred_x {
                try_xs.push(fallback_iob.x);
            }
            xs_seen.clear();
            try_xs.retain(|x| xs_seen.insert(*x));
            let primary_xs = try_xs.len();
            for &x in &all_xs {
                if xs_seen.insert(x) {
                    try_xs.push(x);
                }
            }
            let try_slot = |col_x: u32,
                            affinity: &[Site],
                            cols: &std::collections::HashMap<u32, Vec<Site>>,
                            col_free: &std::collections::HashMap<u32, usize>,
                            site_used_n: &std::collections::HashMap<(u32, u32), u8>,
                            used: &mut HashSet<(u32, u32, u8)>,
                            y_seen: &mut HashSet<u32>,
                            full_y: bool|
             -> Option<(Site, u8)> {
                if col_free.get(&col_x).copied().unwrap_or(0) == 0 {
                    return None;
                }
                let Some(col) = cols.get(&col_x) else {
                    return None;
                };
                if col.is_empty() {
                    return None;
                }
                // Preferred Y order: affinity sites in this column, then ±1, then
                // south/mid wrap so the full 8192 BLE budget is reachable.
                let mut y_order: Vec<u32> =
                    Vec::with_capacity(if full_y { col.len() + 8 } else { 16 });
                for s in affinity {
                    let dx = s.x.abs_diff(col_x);
                    if dx == 0 {
                        y_order.push(s.y);
                        y_order.push(s.y.saturating_add(1));
                        y_order.push(s.y.saturating_add(2));
                        if s.y > 0 {
                            y_order.push(s.y - 1);
                        }
                        if s.y > 1 {
                            y_order.push(s.y - 2);
                        }
                    } else if dx == 1 {
                        // Diagonal / E-W±1 / knight (±1,±2): same Y, ±1, ±2.
                        y_order.push(s.y);
                        y_order.push(s.y.saturating_add(1));
                        y_order.push(s.y.saturating_add(2));
                        if s.y > 0 {
                            y_order.push(s.y - 1);
                        }
                        if s.y > 1 {
                            y_order.push(s.y - 2);
                        }
                    } else if dx == 2 {
                        // Knight / diag±2 / E-W±2: Y, ±1, ±2.
                        y_order.push(s.y);
                        y_order.push(s.y.saturating_add(1));
                        y_order.push(s.y.saturating_add(2));
                        if s.y > 0 {
                            y_order.push(s.y - 1);
                        }
                        if s.y > 1 {
                            y_order.push(s.y - 2);
                        }
                    } else if dx == 3 {
                        // E-W±3: prefer same Y (±1 slack).
                        y_order.push(s.y);
                        y_order.push(s.y.saturating_add(1));
                        if s.y > 0 {
                            y_order.push(s.y - 1);
                        }
                    }
                }
                let base_y = affinity_base_y(col, opts, guide, excepted_iob_cluster);
                // Excepted IOB drivers: mid/quarter Y first so they do not inherit
                // south affinity from internal clusters that keep timing pull.
                if excepted_iob_cluster {
                    y_order.insert(0, base_y);
                } else {
                    y_order.push(base_y);
                }
                y_seen.clear();
                y_order.retain(|y| y_seen.insert(*y));
                let try_y = |y: u32,
                             col: &[Site],
                             site_used_n: &std::collections::HashMap<(u32, u32), u8>,
                             used: &mut HashSet<(u32, u32, u8)>|
                 -> Option<(Site, u8)> {
                    let Ok(idx) = col.binary_search_by_key(&y, |s| s.y) else {
                        return None;
                    };
                    let site = col[idx];
                    if site_used_n.get(&(site.x, site.y)).copied().unwrap_or(0) as usize >= n_ble {
                        return None;
                    }
                    for ble in 0..n_ble as u8 {
                        if used.insert((site.x, site.y, ble)) {
                            return Some((site, ble));
                        }
                    }
                    None
                };
                for y in y_order {
                    if let Some(hit) = try_y(y, col, site_used_n, used) {
                        return Some(hit);
                    }
                }
                if full_y {
                    for s in col {
                        if y_seen.contains(&s.y) {
                            continue;
                        }
                        if let Some(hit) = try_y(s.y, col, site_used_n, used) {
                            return Some(hit);
                        }
                    }
                }
                None
            };
            // Per column: affinity+base Y first, then remaining column Ys — never
            // advance to the next column before exhausting the current (legacy order).
            // Primary xs first; die-wide all_xs tail only if primary misses.
            let mut place_xs = |xs: &[u32]| -> Option<(Site, u8)> {
                for &col_x in xs {
                    if let Some(hit) = try_slot(
                        col_x,
                        &affinity,
                        &cols,
                        &col_free,
                        &site_used_n,
                        &mut used,
                        &mut y_seen,
                        false,
                    ) {
                        return Some(hit);
                    }
                    if let Some(hit) = try_slot(
                        col_x,
                        &affinity,
                        &cols,
                        &col_free,
                        &site_used_n,
                        &mut used,
                        &mut y_seen,
                        true,
                    ) {
                        return Some(hit);
                    }
                }
                None
            };
            let mut placed = place_xs(&try_xs[..primary_xs]);
            if placed.is_none() {
                placed = place_xs(&try_xs[primary_xs..]);
            }
            let Some(site_ble) = placed else {
                break;
            };
            if let Some(c) = col_free.get_mut(&site_ble.0.x) {
                *c = c.saturating_sub(1);
            }
            *site_used_n.entry((site_ble.0.x, site_ble.0.y)).or_insert(0) += 1;
            if !lf.ff_cell.is_empty() {
                ff_at.insert(lf.ff_cell.as_str(), site_ble.0);
            }
            if excepted_iob_cluster {
                excepted_iob.insert(li);
            }
            site_of[li] = Some(site_ble);
        }
        // Compact to packed order, dropping unplaced LUTFFs (affinity-loop
        // break) from both sides together so the index pairing stays exact.
        let mut packed_order_sites: Vec<(Site, u8)> = Vec::with_capacity(packed.lutffs.len());
        let mut kept_lutffs = Vec::with_capacity(packed.lutffs.len());
        for (lf, slot) in packed.lutffs.drain(..).zip(site_of) {
            if let Some(sb) = slot {
                kept_lutffs.push(lf);
                packed_order_sites.push(sb);
            }
        }
        packed.lutffs = kept_lutffs;
        lutff_sites = packed_order_sites;

        let nplace = lutff_sites.len();
        eprintln!(
            "hang_diag place affinity lutffs={} ms={}",
            nplace,
            t_aff.elapsed().as_millis()
        );
        // FM-HEL-OPT-P0-1: 1113_5.v historically SIGTERM'd in *unbounded*
        // bileg legalize at 783 LUTFFs. Keep affinity; for n≥256 run a
        // *budgeted* legalize (limited passes + wall) so Ibex imux_skip can
        // drop honestly. Gold counter (n≪256) keeps the full 32-pass path.
        // Never invent E-W IMUX without wires; IBEX_IMPL_CAP_SEC=120 stays.
        let large_place = nplace >= 256;
        if large_place {
            eprintln!(
                "diagnostic place_affinity_cap lutffs={} (budgeted IMUX legalize after affinity; placement kept)",
                nplace
            );
        }
        let t_leg = std::time::Instant::now();
        // FM-HEL-TOP: bidirectional IMUX legalization — pull sinks toward
        // drivers AND drivers toward sinks onto real HAD reach (same-CLB /
        // N-S±1/±2 / E-W±1/±2 / diag±1 / knight); empty-BLE move then
        // pairwise swap when sites are full.
        // Spatial occupancy index: dense (x,y) site + BLE occupants.
        let (mut max_x, mut max_y) = (0u32, 0u32);
        for col in cols.values() {
            for &s in col {
                max_x = max_x.max(s.x);
                max_y = max_y.max(s.y);
            }
        }
        let mut grid = LegalizeGrid::new(max_x, max_y, n_ble);
        for col in cols.values() {
            for &s in col {
                grid.set_site(s);
            }
        }
        for (i, (s, ble)) in lutff_sites.iter().enumerate() {
            grid.place_at(s.x, s.y, *ble, i);
        }
        // Pin → driver cluster index. Hot-path scores walk this, not HashMap<&str>.
        let mut ff_idx: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::with_capacity(nplace);
        for (i, lf) in packed.lutffs.iter().enumerate().take(nplace) {
            if !lf.ff_cell.is_empty() {
                ff_idx.insert(lf.ff_cell.as_str(), i);
            }
        }
        let pin_drv: Vec<Vec<Option<usize>>> = packed
            .lutffs
            .iter()
            .take(nplace)
            .map(|lf| {
                lf.lut_pins
                    .iter()
                    .map(|(_, d)| ff_idx.get(d.as_str()).copied())
                    .collect()
            })
            .collect();
        // Reverse fanout: driver cluster → (sink cluster, pin_count).
        let mut sinks_of: Vec<Vec<(usize, u32)>> = vec![Vec::new(); nplace];
        for (i, pins) in pin_drv.iter().enumerate() {
            for &d in pins {
                let Some(di) = d else {
                    continue;
                };
                if let Some(slot) = sinks_of[di].iter_mut().find(|(s, _)| *s == i) {
                    slot.1 += 1;
                } else {
                    sinks_of[di].push((i, 1));
                }
            }
        }
        // Reused across cells/passes to cut HashSet/Vec alloc churn.
        let stamp_gx = (max_x as usize).saturating_add(16);
        let stamp_gy = (max_y as usize).saturating_add(16);
        let mut cand_stamp: Vec<u32> = vec![0; stamp_gx.saturating_mul(stamp_gy)];
        let mut cand_epoch: u32 = 0;
        let mut cand_xy: Vec<(u32, u32)> = Vec::with_capacity(128);
        let mut remaining_sinks: Vec<usize> = Vec::with_capacity(nplace);
        let mut remaining_drvs: Vec<usize> = Vec::with_capacity(nplace);
        let push_reach = |xy: &mut Vec<(u32, u32)>, x: u32, y: u32| {
            xy.push((x, y));
            // N-S ±1..±4
            for d in 1u32..=4 {
                xy.push((x, y.saturating_add(d)));
                if y >= d {
                    xy.push((x, y - d));
                }
            }
            // E-W ±1..±3
            for d in 1u32..=3 {
                xy.push((x.saturating_add(d), y));
                if x >= d {
                    xy.push((x - d, y));
                }
            }
            // Diagonal (±1,±1) and (±2,±2)
            for d in [1u32, 2] {
                xy.push((x.saturating_add(d), y.saturating_add(d)));
                if y >= d {
                    xy.push((x.saturating_add(d), y - d));
                }
                if x >= d {
                    xy.push((x - d, y.saturating_add(d)));
                }
                if x >= d && y >= d {
                    xy.push((x - d, y - d));
                }
            }
            // Knight (±2,±1) / (±1,±2)
            xy.push((x.saturating_add(2), y.saturating_add(1)));
            xy.push((x.saturating_add(1), y.saturating_add(2)));
            if y > 0 {
                xy.push((x.saturating_add(2), y - 1));
            }
            if y > 1 {
                xy.push((x.saturating_add(1), y - 2));
            }
            if x > 0 {
                xy.push((x - 1, y.saturating_add(2)));
            }
            if x > 1 {
                xy.push((x - 2, y.saturating_add(1)));
            }
            if x > 0 && y > 1 {
                xy.push((x - 1, y - 2));
            }
            if x > 1 && y > 0 {
                xy.push((x - 2, y - 1));
            }
        };
        // Gradient sites between a and b (inclusive steps) for long-arc walk.
        let push_gradient = |xy: &mut Vec<(u32, u32)>, ax: u32, ay: u32, bx: u32, by: u32| {
            xy.push((ax, ay));
            xy.push((bx, by));
            let mx = (ax + bx) / 2;
            let my = (ay + by) / 2;
            xy.push((mx, my));
            // Step ±1/±2 toward b along each axis from a.
            let toward = |from: u32, to: u32, step: u32| -> u32 {
                if from < to {
                    from.saturating_add(step).min(to)
                } else if from > to {
                    from.saturating_sub(step)
                } else {
                    from
                }
            };
            for step in [1u32, 2, 3, 4] {
                let nx = toward(ax, bx, step);
                let ny = toward(ay, by, step);
                xy.push((nx, ny));
                xy.push((nx, ay));
                xy.push((ax, ny));
                // also from b toward a (driver walk)
                let nx2 = toward(bx, ax, step);
                let ny2 = toward(by, ay, step);
                xy.push((nx2, ny2));
            }
            push_reach(xy, mx, my);
        };
        let mut moved = 0u32;
        let mut swapped = 0u32;
        let mut driver_moved = 0u32;
        let mut driver_swapped = 0u32;
        // Cheap early-out: if initial affinity place is already IMUX-legal,
        // skip the 32-pass bileg legalize (reduced Ibex / small designs).
        // Score real IMUX legality for every cluster, including IOB-excepted
        // drivers (affinity demotion must not freeze illegal mid-Y sites).
        remaining_sinks.clear();
        remaining_drvs.clear();
        for i in 0..nplace {
            let (site, _) = lutff_sites[i];
            if !packed.lutffs[i].lut_pins.is_empty()
                && imux_score_idx(&pin_drv[i], site, &lutff_sites, None, None).0 > 0
            {
                remaining_sinks.push(i);
            }
            if !packed.lutffs[i].ff_cell.is_empty()
                && sinks_of[i]
                    .iter()
                    .any(|&(si, _)| !imux_local(site, lutff_sites[si].0))
            {
                remaining_drvs.push(i);
            }
        }
        let already_legal = remaining_sinks.is_empty() && remaining_drvs.is_empty();
        // Budgeted legalize for large packs (n≥256 / device-full Ibex).
        // Prior tip also zeroed passes when nplace >= lut6_count (8192 on
        // HL10T), which silently skipped legalize on every device-full
        // design — that is the imux_skip≈1208–1360 QoR hole. Small designs
        // (nplace<256) still get 32 passes with no wall check.
        const LARGE_PASS_LIMIT: u32 = 8;
        const LARGE_LEGALIZE_MS: u128 = 5_000;
        const SINK_SAMPLE: usize = 16;
        let pass_limit = if already_legal {
            0
        } else if large_place {
            LARGE_PASS_LIMIT
        } else {
            32
        };
        let mut ill_xy: Vec<(u32, u32, u32)> = Vec::new();
        for _pass in 0..pass_limit {
            if large_place && t_leg.elapsed().as_millis() >= LARGE_LEGALIZE_MS {
                eprintln!(
                    "diagnostic place_legalize_budget lutffs={} ms={} (wall; affinity+partial legalize kept)",
                    nplace,
                    t_leg.elapsed().as_millis()
                );
                break;
            }
            // Rebuild remaining illegal arcs; empty set → done (no full re-scan work).
            remaining_sinks.clear();
            remaining_drvs.clear();
            for i in 0..nplace {
                let (site, _) = lutff_sites[i];
                if !packed.lutffs[i].lut_pins.is_empty()
                    && imux_score_idx(&pin_drv[i], site, &lutff_sites, None, None).0 > 0
                {
                    remaining_sinks.push(i);
                }
                if !packed.lutffs[i].ff_cell.is_empty()
                    && sinks_of[i]
                        .iter()
                        .any(|&(si, _)| !imux_local(site, lutff_sites[si].0))
                {
                    remaining_drvs.push(i);
                }
            }
            if remaining_sinks.is_empty() && remaining_drvs.is_empty() {
                break;
            }
            let mut pass_moved = 0u32;
            let mut pass_swapped = 0u32;
            let mut pass_drv_moved = 0u32;
            let mut pass_drv_swapped = 0u32;
            // --- Phase A: pull sinks toward drivers (existing) ---
            for &i in &remaining_sinks {
                let _lf = &packed.lutffs[i];
                let (cur_site, cur_ble) = lutff_sites[i];
                let before_sc = imux_score_idx(&pin_drv[i], cur_site, &lutff_sites, None, None);
                let before = before_sc.0;
                let before_sp = before_sc.1;
                if before == 0 {
                    continue;
                }
                cand_xy.clear();
                // Small designs: full IMUX-reach of every driver so the
                // intersection of multi-pin reach stays in the candidate set.
                // Large (budgeted) packs: full reach+gradient only on illegal
                // arcs; legal drivers contribute their exact site. Intersection
                // of all reaches is still ⊂ illegal-driver reach.
                for &d in &pin_drv[i] {
                    let Some(di) = d else {
                        continue;
                    };
                    let ds = lutff_sites[di].0;
                    if large_place && imux_local(ds, cur_site) {
                        cand_xy.push((ds.x, ds.y));
                        continue;
                    }
                    push_reach(&mut cand_xy, ds.x, ds.y);
                    if !imux_local(ds, cur_site) {
                        push_gradient(&mut cand_xy, cur_site.x, cur_site.y, ds.x, ds.y);
                    }
                }
                cand_epoch = cand_epoch.wrapping_add(1);
                if cand_epoch == 0 {
                    cand_stamp.fill(0);
                    cand_epoch = 1;
                }
                let ep = cand_epoch;
                cand_xy.retain(|&(x, y)| {
                    let ux = x as usize;
                    let uy = y as usize;
                    if ux < stamp_gx && uy < stamp_gy {
                        let slot = &mut cand_stamp[uy * stamp_gx + ux];
                        if *slot == ep {
                            false
                        } else {
                            *slot = ep;
                            true
                        }
                    } else {
                        false
                    }
                });
                let skip = (cur_site.x, cur_site.y, cur_ble);
                // 1) Prefer empty BLE: check free BLE in O(1) before scoring site.
                let mut best: Option<(Site, u8, (u32, u32))> = None;
                for (cx, cy) in &cand_xy {
                    let Some(site) = grid.site_xy(*cx, *cy) else {
                        continue;
                    };
                    let Some(ble) = grid.first_free_ble(site.x, site.y, skip) else {
                        continue;
                    };
                    let sc = imux_score_idx(&pin_drv[i], site, &lutff_sites, None, None);
                    if sc >= before_sc || best.as_ref().map(|b| sc >= b.2).unwrap_or(false) {
                        continue;
                    }
                    best = Some((site, ble, sc));
                    if sc.0 == 0 {
                        break;
                    }
                }
                if let Some((site, ble, _)) = best {
                    grid.relocate(skip, (site.x, site.y, ble), i);
                    lutff_sites[i] = (site, ble);
                    pass_moved += 1;
                    continue;
                }
                // 2) Pairwise swap with occupant when total illegal pins drop.
                let mut best_swap: Option<(usize, Site, u8, (u32, u32))> = None;
                for (cx, cy) in &cand_xy {
                    let Some(site) = grid.site_xy(*cx, *cy) else {
                        continue;
                    };
                    if grid.occ(site.x, site.y) == 0 {
                        continue;
                    }
                    for ble in 0..n_ble as u8 {
                        if site.x == cur_site.x && site.y == cur_site.y && ble == cur_ble {
                            continue;
                        }
                        let Some(j) = grid.occupant(site.x, site.y, ble) else {
                            continue;
                        };
                        if j == i {
                            continue;
                        }
                        let (osite, _oble) = lutff_sites[j];
                        let other_before_sc =
                            imux_score_idx(&pin_drv[j], osite, &lutff_sites, None, None);
                        let ov_a = Some((i, site));
                        let ov_b = Some((j, cur_site));
                        let sc_i = imux_score_idx(&pin_drv[i], site, &lutff_sites, ov_a, ov_b);
                        if sc_i.0 > before + other_before_sc.0 {
                            continue;
                        }
                        let sc_j = imux_score_idx(&pin_drv[j], cur_site, &lutff_sites, ov_a, ov_b);
                        let after = (sc_i.0 + sc_j.0, sc_i.1 + sc_j.1);
                        let before_tot =
                            (before + other_before_sc.0, before_sp + other_before_sc.1);
                        if after >= before_tot {
                            continue;
                        }
                        if best_swap.as_ref().map(|b| after < b.3).unwrap_or(true) {
                            best_swap = Some((j, site, ble, after));
                            if after.0 == 0 && after.1 == 0 {
                                break;
                            }
                        }
                    }
                    if best_swap.as_ref().map(|b| b.3.0) == Some(0) {
                        break;
                    }
                }
                if let Some((j, site, ble, _)) = best_swap {
                    let (osite, oble) = lutff_sites[j];
                    grid.swap_bles(
                        (cur_site.x, cur_site.y, cur_ble),
                        (osite.x, osite.y, oble),
                        i,
                        j,
                    );
                    lutff_sites[i] = (site, ble);
                    lutff_sites[j] = (cur_site, cur_ble);
                    // ff_at is not used after initial affinity placement.
                    pass_swapped += 1;
                }
            }
            // --- Phase B: pull drivers toward illegal sinks (bileg) ---
            // Fanout-primary: accept move if illegal fanout drops (lex), even
            // when driver inputs briefly worsen — sink-phase repairs next pass.
            for &i in &remaining_drvs {
                let lf = &packed.lutffs[i];
                if lf.ff_cell.is_empty() {
                    continue;
                }
                let sink_idxs = sinks_of[i].as_slice();
                if sink_idxs.is_empty() {
                    continue;
                }
                let (cur_site, cur_ble) = lutff_sites[i];
                let before =
                    drv_score_idx(i, cur_site, sink_idxs, &pin_drv, &lutff_sites, None, None);
                if before.0 == 0 {
                    continue;
                }
                // Candidates: IMUX-reach of sampled illegal sinks + gradient.
                // High-fanout nets: farthest + centroid only — scoring still
                // uses every sink. Does not invent E-W IMUX without wires.
                cand_xy.clear();
                ill_xy.clear();
                for &(si, _) in sink_idxs {
                    let ss = lutff_sites[si].0;
                    if !imux_local(cur_site, ss) {
                        let m = cur_site.x.abs_diff(ss.x) + cur_site.y.abs_diff(ss.y);
                        ill_xy.push((m, ss.x, ss.y));
                    }
                }
                if ill_xy.len() <= SINK_SAMPLE {
                    for &(_, x, y) in &ill_xy {
                        push_reach(&mut cand_xy, x, y);
                        push_gradient(&mut cand_xy, cur_site.x, cur_site.y, x, y);
                    }
                } else {
                    let n = ill_xy.len() as u64;
                    let sx: u64 = ill_xy.iter().map(|t| t.1 as u64).sum();
                    let sy: u64 = ill_xy.iter().map(|t| t.2 as u64).sum();
                    let cx = (sx / n) as u32;
                    let cy = (sy / n) as u32;
                    push_reach(&mut cand_xy, cx, cy);
                    push_gradient(&mut cand_xy, cur_site.x, cur_site.y, cx, cy);
                    ill_xy.select_nth_unstable_by(SINK_SAMPLE - 1, |a, b| b.0.cmp(&a.0));
                    for &(_, x, y) in ill_xy.iter().take(SINK_SAMPLE) {
                        push_reach(&mut cand_xy, x, y);
                        push_gradient(&mut cand_xy, cur_site.x, cur_site.y, x, y);
                    }
                }
                cand_epoch = cand_epoch.wrapping_add(1);
                if cand_epoch == 0 {
                    cand_stamp.fill(0);
                    cand_epoch = 1;
                }
                let ep = cand_epoch;
                cand_xy.retain(|&(x, y)| {
                    let ux = x as usize;
                    let uy = y as usize;
                    if ux < stamp_gx && uy < stamp_gy {
                        let slot = &mut cand_stamp[uy * stamp_gx + ux];
                        if *slot == ep {
                            false
                        } else {
                            *slot = ep;
                            true
                        }
                    } else {
                        false
                    }
                });
                let skip = (cur_site.x, cur_site.y, cur_ble);
                // 1) Empty BLE move for driver — check free BLE in O(1) before scoring site.
                let mut best: Option<(Site, u8, (u32, u32, u32))> = None;
                for (cx, cy) in &cand_xy {
                    let Some(site) = grid.site_xy(*cx, *cy) else {
                        continue;
                    };
                    let Some(ble) = grid.first_free_ble(site.x, site.y, skip) else {
                        continue;
                    };
                    let sc = drv_score_idx(
                        i,
                        site,
                        sink_idxs,
                        &pin_drv,
                        &lutff_sites,
                        Some((i, site)),
                        None,
                    );
                    if sc >= before || best.as_ref().map(|b| sc >= b.2).unwrap_or(false) {
                        continue;
                    }
                    best = Some((site, ble, sc));
                    if sc.0 == 0 {
                        break;
                    }
                }
                if let Some((site, ble, _)) = best {
                    grid.relocate(skip, (site.x, site.y, ble), i);
                    lutff_sites[i] = (site, ble);
                    pass_drv_moved += 1;
                    continue;
                }
                // 2) Pairwise swap: driver ↔ occupant. Lex score on
                // (drv_fanout + other_fanout, drv_inputs + other_inputs).
                let mut best_swap: Option<(usize, Site, u8, (u32, u32, u32))> = None;
                for (cx, cy) in &cand_xy {
                    let Some(site) = grid.site_xy(*cx, *cy) else {
                        continue;
                    };
                    if grid.occ(site.x, site.y) == 0 {
                        continue;
                    }
                    for ble in 0..n_ble as u8 {
                        if site.x == cur_site.x && site.y == cur_site.y && ble == cur_ble {
                            continue;
                        }
                        let Some(j) = grid.occupant(site.x, site.y, ble) else {
                            continue;
                        };
                        if j == i {
                            continue;
                        }
                        let (osite, _oble) = lutff_sites[j];
                        let j_sinks = sinks_of[j].as_slice();
                        let before_j = if !packed.lutffs[j].ff_cell.is_empty() {
                            drv_score_idx(j, osite, j_sinks, &pin_drv, &lutff_sites, None, None)
                        } else {
                            (
                                0,
                                0,
                                imux_score_idx(&pin_drv[j], osite, &lutff_sites, None, None).0,
                            )
                        };
                        let before_tot = (
                            before.0 + before_j.0,
                            before.1 + before_j.1,
                            before.2 + before_j.2,
                        );
                        let ov_a = Some((i, site));
                        let ov_b = Some((j, cur_site));
                        let after_i =
                            drv_score_idx(i, site, sink_idxs, &pin_drv, &lutff_sites, ov_a, ov_b);
                        let after_j = if !packed.lutffs[j].ff_cell.is_empty() {
                            drv_score_idx(j, cur_site, j_sinks, &pin_drv, &lutff_sites, ov_a, ov_b)
                        } else {
                            (
                                0,
                                0,
                                imux_score_idx(&pin_drv[j], cur_site, &lutff_sites, ov_a, ov_b).0,
                            )
                        };
                        let after_tot = (
                            after_i.0 + after_j.0,
                            after_i.1 + after_j.1,
                            after_i.2 + after_j.2,
                        );
                        if after_tot >= before_tot {
                            continue;
                        }
                        if best_swap.as_ref().map(|b| after_tot < b.3).unwrap_or(true) {
                            best_swap = Some((j, site, ble, after_tot));
                            if after_tot.0 == 0 {
                                break;
                            }
                        }
                    }
                    if best_swap.as_ref().map(|b| b.3.0) == Some(0) {
                        break;
                    }
                }
                if let Some((j, site, ble, _)) = best_swap {
                    let (osite, oble) = lutff_sites[j];
                    grid.swap_bles(
                        (cur_site.x, cur_site.y, cur_ble),
                        (osite.x, osite.y, oble),
                        i,
                        j,
                    );
                    lutff_sites[i] = (site, ble);
                    lutff_sites[j] = (cur_site, cur_ble);
                    // ff_at is not used after initial affinity placement.
                    pass_drv_swapped += 1;
                }
            }
            moved += pass_moved;
            swapped += pass_swapped;
            driver_moved += pass_drv_moved;
            driver_swapped += pass_drv_swapped;
            if pass_moved == 0 && pass_swapped == 0 && pass_drv_moved == 0 && pass_drv_swapped == 0
            {
                break;
            }
        }
        if moved > 0 || swapped > 0 || driver_moved > 0 || driver_swapped > 0 {
            eprintln!(
                "hang_bileg place imux_legalize sink_moved={moved} sink_swapped={swapped} drv_moved={driver_moved} drv_swapped={driver_swapped}"
            );
        }
        eprintln!(
            "hang_diag place legalize ms={}",
            t_leg.elapsed().as_millis()
        );
    }

    let mut mac_sites = Vec::new();
    let dsps: Vec<_> = dev.dsp_sites().collect();
    if packed.macs.len() > dsps.len() {
        eprintln!(
            "hang_diag place_dsp_cap {} -> {}",
            packed.macs.len(),
            dsps.len()
        );
    }
    for (i, _) in packed.macs.iter().enumerate().take(dsps.len()) {
        mac_sites.push(dsps[i]);
    }

    let mut bram_sites = Vec::new();
    let brams: Vec<_> = dev.bram_sites().collect();
    if packed.brams.len() > brams.len() {
        eprintln!(
            "hang_diag place_bram_cap {} -> {}",
            packed.brams.len(),
            brams.len()
        );
    }
    for (i, _) in packed.brams.iter().enumerate().take(brams.len()) {
        bram_sites.push(brams[i]);
    }

    let cost = if opts.timing_weight > 0.0 {
        lutff_sites
            .first()
            .zip(iob_sites.first())
            .map(|(l, i)| (l.0.y as f64 - i.y as f64).abs() * opts.timing_weight)
            .unwrap_or(0.0)
    } else {
        lutff_sites.first().map(|(s, _)| s.y as f64).unwrap_or(0.0)
    };

    packed.lutffs.truncate(lutff_sites.len());
    packed.iobs.truncate(iob_sites.len());
    packed.macs.truncate(mac_sites.len());
    packed.brams.truncate(bram_sites.len());
    Ok(Placed {
        packed,
        lutff_sites,
        iob_sites,
        mac_sites,
        bram_sites,
        timing_weight: opts.timing_weight,
        cost,
    })
}

/// UG893 floorplanning: place into a Pblock rectangle (CLB_XxYy:CLB_XxYy).
/// Hits the same site picker as `place_with_guide`, then relocates LUTFF (and IOB if
/// the rectangle covers HAD IOB rows) into the region. IOB cells with a LOC /
/// PACKAGE_PIN keep their pin — fabric pblocks must not displace gold IOBs.
/// Unguided wrapper — helion-cli / helion-proj still call this until a later lock.
pub fn place_in_region(
    packed: &Packed,
    dev: &Device,
    opts: PlaceOpts,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Result<Placed, String> {
    place_in_region_with_guide(packed, dev, opts, x0, y0, x1, y1, &TimingGuide::default())
}

/// `place_in_region` with XDC IOB-arc exceptions. Empty `guide` matches `place_in_region`.
pub fn place_in_region_with_guide(
    packed: &Packed,
    dev: &Device,
    opts: PlaceOpts,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    guide: &TimingGuide,
) -> Result<Placed, String> {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    let mut placed = place_with_guide(packed, dev, opts, guide)?;
    let mut clbs: Vec<Site> = dev
        .clb_sites()
        .filter(|s| s.x >= x0 && s.x <= x1 && s.y >= y0 && s.y <= y1)
        .collect();
    if clbs.is_empty() {
        return Err(format!(
            "pblock CLB_X{x0}Y{y0}:CLB_X{x1}Y{y1} has no HAD CLB sites"
        ));
    }
    clbs.sort_by_key(|s| (s.x, s.y));
    let n_ble = dev.n_ble.max(1) as usize;
    for i in 0..placed.lutff_sites.len() {
        let clb_off = i / n_ble;
        let ble = (i % n_ble) as u8;
        let idx = clb_off.min(clbs.len() - 1);
        placed.lutff_sites[i] = (clbs[idx], ble);
    }
    let mut iobs: Vec<Site> = dev
        .iob_sites()
        .filter(|s| s.x >= x0 && s.x <= x1 && s.y >= y0 && s.y <= y1)
        .collect();
    if !iobs.is_empty() {
        iobs.sort_by_key(|s| (s.x, s.y));
        for (i, slot) in placed.iob_sites.iter_mut().enumerate() {
            // Keep PACKAGE_PIN / LOC from place_with_guide — pblock fabric move must
            // not displace gold IOB sites (clk=IOB_X3Y0, led=IOB_X2Y0).
            if packed.iobs.get(i).and_then(|io| io.loc.as_ref()).is_some() {
                continue;
            }
            *slot = iobs[i.min(iobs.len() - 1)];
        }
    }
    placed.cost = if opts.timing_weight > 0.0 {
        placed
            .lutff_sites
            .first()
            .zip(placed.iob_sites.first())
            .map(|(l, i)| (l.0.y as f64 - i.y as f64).abs() * opts.timing_weight)
            .unwrap_or(0.0)
    } else {
        placed
            .lutff_sites
            .first()
            .map(|(s, _)| s.y as f64)
            .unwrap_or(0.0)
    };
    Ok(placed)
}

/// UG986 Lab 2: reuse previous LUTFF/IOB sites for cells that kept their names.
/// Unguided wrapper — helion-cli / helion-proj still call this until a later lock.
pub fn place_incremental(
    packed: &Packed,
    dev: &Device,
    prev: &Placed,
    opts: PlaceOpts,
) -> Result<(Placed, usize), String> {
    place_incremental_with_guide(packed, dev, prev, opts, &TimingGuide::default())
}

/// `place_incremental` with XDC IOB-arc exceptions. Empty `guide` matches `place_incremental`.
pub fn place_incremental_with_guide(
    packed: &Packed,
    dev: &Device,
    prev: &Placed,
    opts: PlaceOpts,
    guide: &TimingGuide,
) -> Result<(Placed, usize), String> {
    let mut placed = place_with_guide(packed, dev, opts, guide)?;
    let mut reused = 0usize;
    let prev_lutff: std::collections::HashMap<&str, usize> = prev
        .packed
        .lutffs
        .iter()
        .enumerate()
        .map(|(j, p)| (p.lut_cell.as_str(), j))
        .collect();
    for (i, lf) in packed.lutffs.iter().enumerate() {
        if let Some(&j) = prev_lutff.get(lf.lut_cell.as_str()) {
            let site = prev.lutff_sites[j];
            placed.lutff_sites[i] = site;
            reused += 1;
        }
    }
    let prev_iob: std::collections::HashMap<&str, usize> = prev
        .packed
        .iobs
        .iter()
        .enumerate()
        .map(|(j, p)| (p.cell.as_str(), j))
        .collect();
    for (i, iob) in packed.iobs.iter().enumerate() {
        if let Some(&j) = prev_iob.get(iob.cell.as_str()) {
            placed.iob_sites[i] = prev.iob_sites[j];
        }
    }
    Ok((placed, reused))
}

pub fn lutff_of(placed: &Placed, ff_cell: &str) -> Option<(Site, u8)> {
    placed
        .packed
        .lutffs
        .iter()
        .position(|l| l.ff_cell == ff_cell)
        .map(|i| placed.lutff_sites[i])
}

/// Permute a 6-input LUT truth table according to `old_to_new`, where a signal
/// previously connected to input `I{p}` (`p` in `0..6`) is moved to `I{old_to_new[p]}`.
///
/// Preserves the exact boolean function of the LUT across all 64 input combinations.
pub fn permute_lut6_init(init: u64, old_to_new: &[u8; 6]) -> u64 {
    if old_to_new == &[0, 1, 2, 3, 4, 5] {
        return init;
    }
    let mut out = 0u64;
    for new_addr in 0..64u64 {
        let mut old_addr = 0u64;
        for (old_p, &new_p) in old_to_new.iter().enumerate() {
            let np = (new_p as u64).min(5);
            if ((new_addr >> np) & 1) == 1 {
                old_addr |= 1u64 << old_p;
            }
        }
        if ((init >> old_addr) & 1) == 1 {
            out |= 1u64 << new_addr;
        }
    }
    out
}

/// Reorder connected LUT input pins so longer-wire / higher-delay fanin arcs
/// land on lower-indexed (faster) LUT pins (`I0`, `I1`, ...), updating each
/// LUT's 64-bit `init` truth table via [`permute_lut6_init`] so logical behavior
/// is invariant. Returns the number of LUT clusters whose pin assignments changed.
pub fn permute_lut_pins_for_timing(placed: &mut Placed) -> usize {
    let n = placed.lutff_sites.len().min(placed.packed.lutffs.len());
    let mut ff_site: std::collections::HashMap<String, Site> =
        std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let lf = &placed.packed.lutffs[i];
        if !lf.ff_cell.is_empty() {
            ff_site.insert(lf.ff_cell.clone(), placed.lutff_sites[i].0);
        }
    }
    let mut permuted = 0usize;
    for i in 0..n {
        let site = placed.lutff_sites[i].0;
        let lf = &mut placed.packed.lutffs[i];
        if lf.lut_pins.is_empty() {
            continue;
        }
        // Rank each connected pin by (Manhattan distance from driver descending, original pin ascending).
        let mut entries: Vec<(u8, String, u32)> = lf
            .lut_pins
            .iter()
            .map(|(p, drv)| {
                let dist = ff_site
                    .get(drv.as_str())
                    .map(|ds| ds.x.abs_diff(site.x) + ds.y.abs_diff(site.y))
                    .unwrap_or(0);
                (*p, drv.clone(), dist)
            })
            .collect();
        // Deduplicate by pin index if any duplicate pin numbers exist.
        let mut seen_p = [false; 6];
        if entries.iter().any(|(p, _, _)| {
            let idx = *p as usize;
            if idx >= 6 || seen_p[idx] {
                true
            } else {
                seen_p[idx] = true;
                false
            }
        }) {
            continue;
        }
        entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let mut old_to_new: [u8; 6] = [0, 1, 2, 3, 4, 5];
        let mut used_new = [false; 6];
        let mut changed = false;
        let mut new_pins: Vec<(u8, String)> = Vec::with_capacity(entries.len());
        for (new_slot, (old_p, drv, _)) in entries.into_iter().enumerate() {
            let new_p = new_slot as u8;
            old_to_new[old_p as usize] = new_p;
            used_new[new_p as usize] = true;
            if old_p != new_p {
                changed = true;
            }
            new_pins.push((new_p, drv));
        }
        if !changed {
            continue;
        }
        // Assign remaining unused old pins bijectively to remaining unused new slots.
        let mut free_new = (0..6u8).filter(|&p| !used_new[p as usize]);
        for old_p in 0..6u8 {
            if !seen_p[old_p as usize] {
                if let Some(np) = free_new.next() {
                    old_to_new[old_p as usize] = np;
                }
            }
        }
        lf.init = permute_lut6_init(lf.init, &old_to_new);
        lf.lut_pins = new_pins;
        permuted += 1;
    }
    permuted
}

/// Vivado UG904 `Congestion_SpreadLogic_high` placement density inflation and cell spreading.
///
/// Computes effective pin/BLE density per CLB site and spreads excess clusters from sites
/// exceeding `max_ble_per_site` (clamped to `1..=dev.n_ble`) outward to nearby low-density
/// CLB sites while preserving IMUX reachability where possible.
/// Returns the spread [`Placed`] and the number of relocated LUTFF clusters.
pub fn place_congestion_spread(
    placed: &Placed,
    dev: &Device,
    max_ble_per_site: u8,
) -> Result<(Placed, usize), String> {
    let mut out = placed.clone();
    let n = out.lutff_sites.len().min(out.packed.lutffs.len());
    if n == 0 {
        return Ok((out, 0));
    }
    let n_ble = dev.n_ble.max(1) as u8;
    let cap = max_ble_per_site.clamp(1, n_ble);
    let mut clb_sites: Vec<Site> = dev.clb_sites().collect();
    if clb_sites.is_empty() {
        return Err("device has no CLB sites for congestion spreading".into());
    }
    clb_sites.sort_by_key(|s| (s.x, s.y));

    let mut site_occ: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();
    let mut used_ble: HashSet<(u32, u32, u8)> = HashSet::with_capacity(n);
    for i in 0..n {
        let (s, ble) = out.lutff_sites[i];
        site_occ.entry((s.x, s.y)).or_default().push(i);
        used_ble.insert((s.x, s.y, ble));
    }

    // Build driver/sink adjacency so spreading prefers IMUX-legal target sites
    // and moves high-pin-density clusters first when relieving hot spots.
    let mut ff_idx: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let lf = &out.packed.lutffs[i];
        if !lf.ff_cell.is_empty() {
            ff_idx.insert(lf.ff_cell.as_str(), i);
        }
    }
    let pin_drv: Vec<Vec<Option<usize>>> = (0..n)
        .map(|i| {
            out.packed.lutffs[i]
                .lut_pins
                .iter()
                .map(|(_, d)| ff_idx.get(d.as_str()).copied())
                .collect()
        })
        .collect();
    let mut sinks_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (si, drvs) in pin_drv.iter().enumerate() {
        for &d in drvs.iter().flatten() {
            if !sinks_of[d].contains(&si) {
                sinks_of[d].push(si);
            }
        }
    }

    // Deterministic site order: sort congested sites by (occupancy desc, x, y).
    let mut hot_sites: Vec<(u32, u32)> = site_occ
        .iter()
        .filter(|(_, members)| members.len() > cap as usize)
        .map(|(&xy, _)| xy)
        .collect();
    hot_sites.sort_by(|a, b| {
        site_occ[b]
            .len()
            .cmp(&site_occ[a].len())
            .then_with(|| a.cmp(b))
    });

    let mut occ_count: std::collections::HashMap<(u32, u32), u8> = site_occ
        .iter()
        .map(|(&xy, v)| (xy, v.len() as u8))
        .collect();
    let mut moved = 0usize;

    for (hx, hy) in hot_sites {
        let Some(mut members) = site_occ.get(&(hx, hy)).cloned() else {
            continue;
        };
        if members.len() <= cap as usize {
            continue;
        }
        // Inflate priority for high pin/fanout degree clusters; keep BLE0 anchor last.
        members.sort_by(|&a, &b| {
            let deg_a = out.packed.lutffs[a].lut_pins.len() + sinks_of[a].len();
            let deg_b = out.packed.lutffs[b].lut_pins.len() + sinks_of[b].len();
            deg_b
                .cmp(&deg_a)
                .then_with(|| out.lutff_sites[b].1.cmp(&out.lutff_sites[a].1))
        });
        let excess = members.len() - cap as usize;
        for &ci in members.iter().take(excess) {
            let (cur_site, cur_ble) = out.lutff_sites[ci];
            // Find nearest CLB site with occupancy < cap that keeps connected arcs IMUX-local if possible.
            let mut best_target: Option<(Site, u8, (u32, u32, u8))> = None;
            for &cand in &clb_sites {
                if cand.x == cur_site.x && cand.y == cur_site.y {
                    continue;
                }
                let c_occ = occ_count.get(&(cand.x, cand.y)).copied().unwrap_or(0);
                if c_occ >= cap {
                    continue;
                }
                let Some(free_ble) = (0..n_ble).find(|&b| !used_ble.contains(&(cand.x, cand.y, b)))
                else {
                    continue;
                };
                let mut illegal_arcs = 0u32;
                for &di in pin_drv[ci].iter().flatten() {
                    let ds = if di == ci { cand } else { out.lutff_sites[di].0 };
                    if !imux_local(ds, cand) {
                        illegal_arcs += 1;
                    }
                }
                for &si in &sinks_of[ci] {
                    let ss = if si == ci { cand } else { out.lutff_sites[si].0 };
                    if !imux_local(cand, ss) {
                        illegal_arcs += 1;
                    }
                }
                let dist = cand.x.abs_diff(cur_site.x) + cand.y.abs_diff(cur_site.y);
                let key = (illegal_arcs, dist, c_occ);
                if best_target.as_ref().map(|b| key < b.2).unwrap_or(true) {
                    best_target = Some((cand, free_ble, key));
                }
            }
            if let Some((target_site, target_ble, _)) = best_target {
                used_ble.remove(&(cur_site.x, cur_site.y, cur_ble));
                used_ble.insert((target_site.x, target_site.y, target_ble));
                if let Some(c) = occ_count.get_mut(&(cur_site.x, cur_site.y)) {
                    *c = c.saturating_sub(1);
                }
                *occ_count.entry((target_site.x, target_site.y)).or_insert(0) += 1;
                out.lutff_sites[ci] = (target_site, target_ble);
                moved += 1;
            }
        }
    }
    Ok((out, moved))
}

/// Summary metrics from post-placement physical optimization ([`phys_opt_place_with_stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysOptPlaceStats {
    pub relocated_cells: usize,
    pub swapped_cells: usize,
    pub permuted_luts: usize,
    pub wirelength_before: u64,
    pub wirelength_after: u64,
}

/// Compute total Manhattan wirelength across all LUT-pin fanins and IOB output arcs.
pub fn total_wirelength(placed: &Placed) -> u64 {
    let n = placed.lutff_sites.len().min(placed.packed.lutffs.len());
    let mut ff_site: std::collections::HashMap<&str, Site> =
        std::collections::HashMap::with_capacity(n);
    let mut q_site: std::collections::HashMap<&str, Site> =
        std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let lf = &placed.packed.lutffs[i];
        let s = placed.lutff_sites[i].0;
        if !lf.ff_cell.is_empty() {
            ff_site.insert(lf.ff_cell.as_str(), s);
        }
        if !lf.q_net.is_empty() {
            q_site.insert(lf.q_net.as_str(), s);
        }
    }
    let mut wl = 0u64;
    for i in 0..n {
        let s = placed.lutff_sites[i].0;
        for (_, drv) in &placed.packed.lutffs[i].lut_pins {
            if let Some(&ds) = ff_site.get(drv.as_str()) {
                wl += (ds.x.abs_diff(s.x) + ds.y.abs_diff(s.y)) as u64;
            }
        }
    }
    for (ii, iob) in placed.packed.iobs.iter().enumerate() {
        if let (Some(&isite), Some(&csite)) =
            (placed.iob_sites.get(ii), q_site.get(iob.from_net.as_str()))
        {
            wl += (csite.x.abs_diff(isite.x) + csite.y.abs_diff(isite.y)) as u64;
        }
    }
    wl
}

/// Vivado UG904 post-place `phys_opt_design`: critical-path local cell relocation/swapping
/// and LUT input pin timing permutation.
pub fn phys_opt_place(placed: &Placed, dev: &Device) -> Result<Placed, String> {
    Ok(phys_opt_place_with_stats(placed, dev)?.0)
}

/// Run post-place `phys_opt_design` and return both the optimized [`Placed`] and [`PhysOptPlaceStats`].
pub fn phys_opt_place_with_stats(
    placed: &Placed,
    dev: &Device,
) -> Result<(Placed, PhysOptPlaceStats), String> {
    let mut out = placed.clone();
    let wl_before = total_wirelength(&out);
    let n = out.lutff_sites.len().min(out.packed.lutffs.len());
    if n == 0 {
        return Ok((
            out,
            PhysOptPlaceStats {
                wirelength_before: wl_before,
                wirelength_after: wl_before,
                ..Default::default()
            },
        ));
    }
    let n_ble = dev.n_ble.max(1) as usize;
    let mut clb_sites: Vec<Site> = dev.clb_sites().collect();
    clb_sites.sort_by_key(|s| (s.x, s.y));
    let (mut max_x, mut max_y) = (0u32, 0u32);
    for &s in &clb_sites {
        max_x = max_x.max(s.x);
        max_y = max_y.max(s.y);
    }
    let mut grid = LegalizeGrid::new(max_x, max_y, n_ble);
    for &s in &clb_sites {
        grid.set_site(s);
    }
    for (i, &(s, ble)) in out.lutff_sites.iter().enumerate().take(n) {
        grid.place_at(s.x, s.y, ble, i);
    }

    let mut ff_idx: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let lf = &out.packed.lutffs[i];
        if !lf.ff_cell.is_empty() {
            ff_idx.insert(lf.ff_cell.as_str(), i);
        }
    }
    let pin_drv: Vec<Vec<Option<usize>>> = (0..n)
        .map(|i| {
            out.packed.lutffs[i]
                .lut_pins
                .iter()
                .map(|(_, d)| ff_idx.get(d.as_str()).copied())
                .collect()
        })
        .collect();
    let mut sinks_of: Vec<Vec<(usize, u32)>> = vec![Vec::new(); n];
    for (si, pins) in pin_drv.iter().enumerate() {
        for &d in pins.iter().flatten() {
            if let Some(slot) = sinks_of[d].iter_mut().find(|(s, _)| *s == si) {
                slot.1 += 1;
            } else {
                sinks_of[d].push((si, 1));
            }
        }
    }
    let mut iob_target: Vec<Option<Site>> = vec![None; n];
    for (ii, iob) in out.packed.iobs.iter().enumerate() {
        if let Some(&isite) = out.iob_sites.get(ii) {
            for i in 0..n {
                if out.packed.lutffs[i].q_net == iob.from_net {
                    iob_target[i] = Some(isite);
                }
            }
        }
    }

    // Score a cluster at `site`: (illegal_imux_arcs, weighted_wirelength).
    let cluster_cost = |ci: usize,
                        site: Site,
                        sites: &[(Site, u8)],
                        ov_a: Option<(usize, Site)>,
                        ov_b: Option<(usize, Site)>|
     -> (u32, u64) {
        let mut ill = 0u32;
        let mut wl = 0u64;
        for &d in &pin_drv[ci] {
            let Some(di) = d else {
                continue;
            };
            let ds = if di == ci {
                site
            } else {
                cluster_site(di, sites, ov_a, ov_b)
            };
            if !imux_local(ds, site) {
                ill += 1;
            }
            wl += (ds.x.abs_diff(site.x) + ds.y.abs_diff(site.y)) as u64;
        }
        for &(si, pins) in &sinks_of[ci] {
            if si == ci {
                continue;
            }
            let ss = cluster_site(si, sites, ov_a, ov_b);
            if !imux_local(site, ss) {
                ill += pins;
            }
            wl += (pins as u64) * (site.x.abs_diff(ss.x) + site.y.abs_diff(ss.y)) as u64;
        }
        if let Some(isite) = iob_target[ci] {
            // Critical IOB driver arc weighted 2x to pull toward IOB pad without breaking IMUX.
            wl += 2 * (site.x.abs_diff(isite.x) + site.y.abs_diff(isite.y)) as u64;
        }
        (ill, wl)
    };

    let mut relocated_cells = 0usize;
    let mut swapped_cells = 0usize;

    for _pass in 0..4 {
        let mut pass_changed = 0usize;
        // Rank clusters by descending current local wirelength so critical paths move first.
        let mut order: Vec<(usize, (u32, u64))> = (0..n)
            .map(|i| {
                let c = cluster_cost(i, out.lutff_sites[i].0, &out.lutff_sites, None, None);
                (i, c)
            })
            .collect();
        order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        for (ci, _) in order {
            let (cur_site, cur_ble) = out.lutff_sites[ci];
            let before = cluster_cost(ci, cur_site, &out.lutff_sites, None, None);
            if before.0 == 0 && before.1 == 0 {
                continue;
            }
            // Candidate neighborhood: centroid of connected terminals and ±2 window around current site.
            let mut cands: Vec<(u32, u32)> = Vec::with_capacity(32);
            let (mut sx, mut sy, mut cnt) = (0u64, 0u64, 0u64);
            for &di in pin_drv[ci].iter().flatten() {
                let ds = out.lutff_sites[di].0;
                cands.push((ds.x, ds.y));
                sx += ds.x as u64;
                sy += ds.y as u64;
                cnt += 1;
            }
            for &(si, _) in &sinks_of[ci] {
                let ss = out.lutff_sites[si].0;
                cands.push((ss.x, ss.y));
                sx += ss.x as u64;
                sy += ss.y as u64;
                cnt += 1;
            }
            if let Some(isite) = iob_target[ci] {
                cands.push((isite.x, isite.y.max(dev.clb_y0)));
                sx += isite.x as u64;
                sy += isite.y.max(dev.clb_y0) as u64;
                cnt += 1;
            }
            if cnt > 0 {
                let mx = (sx / cnt) as u32;
                let my = (sy / cnt) as u32;
                for dx in -2i32..=2 {
                    for dy in -2i32..=2 {
                        let nx = (mx as i32 + dx).max(0) as u32;
                        let ny = (my as i32 + dy).max(0) as u32;
                        cands.push((nx, ny));
                    }
                }
            }
            for dx in -2i32..=2 {
                for dy in -2i32..=2 {
                    let nx = (cur_site.x as i32 + dx).max(0) as u32;
                    let ny = (cur_site.y as i32 + dy).max(0) as u32;
                    cands.push((nx, ny));
                }
            }
            cands.sort_unstable();
            cands.dedup();

            let skip = (cur_site.x, cur_site.y, cur_ble);
            let mut best_move: Option<(Site, u8, (u32, u64))> = None;
            for &(cx, cy) in &cands {
                if cx == cur_site.x && cy == cur_site.y {
                    continue;
                }
                let Some(site) = grid.site_xy(cx, cy) else {
                    continue;
                };
                let Some(ble) = grid.first_free_ble(site.x, site.y, skip) else {
                    continue;
                };
                let after = cluster_cost(ci, site, &out.lutff_sites, Some((ci, site)), None);
                if after < before && best_move.as_ref().map(|b| after < b.2).unwrap_or(true) {
                    best_move = Some((site, ble, after));
                }
            }
            if let Some((site, ble, _)) = best_move {
                grid.relocate(skip, (site.x, site.y, ble), ci);
                out.lutff_sites[ci] = (site, ble);
                relocated_cells += 1;
                pass_changed += 1;
                continue;
            }

            // Try pairwise swap with occupant when no free BLE exists at a better site.
            let mut best_swap: Option<(usize, Site, u8, (u32, u64))> = None;
            for &(cx, cy) in &cands {
                if cx == cur_site.x && cy == cur_site.y {
                    continue;
                }
                let Some(site) = grid.site_xy(cx, cy) else {
                    continue;
                };
                if grid.occ(site.x, site.y) == 0 {
                    continue;
                }
                for ble in 0..n_ble as u8 {
                    let Some(cj) = grid.occupant(site.x, site.y, ble) else {
                        continue;
                    };
                    if cj == ci {
                        continue;
                    }
                    let before_j = cluster_cost(cj, site, &out.lutff_sites, None, None);
                    let before_tot = (before.0 + before_j.0, before.1 + before_j.1);
                    let ov_a = Some((ci, site));
                    let ov_b = Some((cj, cur_site));
                    let after_i = cluster_cost(ci, site, &out.lutff_sites, ov_a, ov_b);
                    if after_i.0 > before_tot.0 {
                        continue;
                    }
                    let after_j = cluster_cost(cj, cur_site, &out.lutff_sites, ov_a, ov_b);
                    let after_tot = (after_i.0 + after_j.0, after_i.1 + after_j.1);
                    if after_tot < before_tot
                        && best_swap.as_ref().map(|b| after_tot < b.3).unwrap_or(true)
                    {
                        best_swap = Some((cj, site, ble, after_tot));
                    }
                }
            }
            if let Some((cj, site, ble, _)) = best_swap {
                let (osite, oble) = out.lutff_sites[cj];
                grid.swap_bles(
                    (cur_site.x, cur_site.y, cur_ble),
                    (osite.x, osite.y, oble),
                    ci,
                    cj,
                );
                out.lutff_sites[ci] = (site, ble);
                out.lutff_sites[cj] = (cur_site, cur_ble);
                swapped_cells += 1;
                pass_changed += 1;
            }
        }
        if pass_changed == 0 {
            break;
        }
    }

    let permuted_luts = permute_lut_pins_for_timing(&mut out);
    let wl_after = total_wirelength(&out);
    if out.timing_weight > 0.0 {
        out.cost = out
            .lutff_sites
            .first()
            .zip(out.iob_sites.first())
            .map(|(l, i)| (l.0.y as f64 - i.y as f64).abs() * out.timing_weight)
            .unwrap_or(0.0);
    } else {
        out.cost = out.lutff_sites.first().map(|(s, _)| s.y as f64).unwrap_or(0.0);
    }
    Ok((
        out,
        PhysOptPlaceStats {
            relocated_cells,
            swapped_cells,
            permuted_luts,
            wirelength_before: wl_before,
            wirelength_after: wl_after,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::{Device, SiteKind};
    use helion_ir::{CellKind, Design};
    use helion_pack::pack;

    #[test]
    fn place_caps_lutffs_over_device_without_panic() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut p = pack(&Design::structural_blinky(), &dev).unwrap();
        let proto = p.lutffs[0].clone();
        let n = dev.lut6_count() as usize + 16;
        while p.lutffs.len() < n {
            let mut lf = proto.clone();
            lf.lut_cell = format!("u_lut{}", p.lutffs.len());
            lf.ff_cell = format!("u_ff{}", p.lutffs.len());
            lf.q_net = format!("q{}", p.lutffs.len());
            p.lutffs.push(lf);
        }
        let pl = place(&p, &dev).expect("over-capacity LUTFF must cap, not panic");
        assert_eq!(pl.lutff_sites.len(), dev.lut6_count() as usize);
        assert_eq!(pl.packed.lutffs.len(), pl.lutff_sites.len());
    }

    #[test]
    fn place_caps_iobs_to_device_budget() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut p = pack(&Design::structural_blinky(), &dev).unwrap();
        let proto = p.iobs[0].clone();
        let n_dev = dev.iob_sites().count();
        while p.iobs.len() < n_dev + 8 {
            let mut io = proto.clone();
            io.cell = format!("u_iob{}", p.iobs.len());
            p.iobs.push(io);
        }
        let pl = place(&p, &dev).expect("over-width I/O must place on the device budget");
        assert_eq!(
            pl.iob_sites.len(),
            n_dev,
            "place must cap IOBs to HAD sites, got {}",
            pl.iob_sites.len()
        );
    }

    #[test]
    fn places_on_had_sites() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.lutff_sites[0].0.x, dev.clb_x0);
        assert!(pl.lutff_sites[0].0.y >= dev.clb_y0);
        assert_eq!(pl.iob_sites[0].y, 0);
        assert_eq!(pl.iob_sites[0].x, dev.clb_x0);
    }

    #[test]
    fn timing_driven_differs_from_wirelength() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0, seed: 0 }).unwrap();
        let td = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        assert_ne!(
            wl.lutff_sites[0].0.y, td.lutff_sites[0].0.y,
            "criticality must move the LUTFF (WL y={} TD y={})",
            wl.lutff_sites[0].0.y, td.lutff_sites[0].0.y
        );
        assert!(td.lutff_sites[0].0.y < wl.lutff_sites[0].0.y);
        assert_ne!(wl.cost, td.cost);
    }

    #[test]
    fn places_mac_on_dsp_part() {
        let dev = Device::load_part("HL10T-DSP1").unwrap();
        let mut d = Design::new("m");
        d.add_cell("u_mac", CellKind::Mac27);
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.mac_sites.len(), 1);
        assert_eq!(pl.mac_sites[0].kind, SiteKind::Dsp);
    }

    #[test]
    fn pblock_region_places_lutff_inside() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let def = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        let pl = place_in_region_with_guide(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
            5,
            1,
            8,
            8,
            &TimingGuide::default(),
        )
        .unwrap();
        assert!(!pl.lutff_sites.is_empty());
        for (s, _) in &pl.lutff_sites {
            assert!(
                s.x >= 5 && s.x <= 8 && s.y >= 1 && s.y <= 8,
                "LUTFF must sit in the pblock: {s:?}"
            );
        }
        assert_ne!(
            def.lutff_sites[0].0.x, pl.lutff_sites[0].0.x,
            "pblock must move LUTFF off the default IOB column"
        );
    }

    #[test]
    fn loc_attr_selects_iob_site() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.set_loc("led", "IOB_X5Y0").unwrap();
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.iobs[0].loc.as_deref(), Some("IOB_X5Y0"));
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.iob_sites[0].x, 5);
        assert_eq!(pl.iob_sites[0].y, 0);
        assert_eq!(pl.lutff_sites[0].0.x, 5, "LUTFF follows LOC column");
    }

    #[test]
    fn places_bram_and_counter_distinct_bles() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.bram_sites.len(), 1);
        assert_eq!(pl.bram_sites[0].kind, SiteKind::Bram);
        let bles: Vec<u8> = pl.lutff_sites.iter().map(|(_, b)| *b).collect();
        assert_eq!(bles, vec![0, 1, 2, 3]);
        let sites: Vec<_> = pl.lutff_sites.iter().map(|(s, _)| (s.x, s.y)).collect();
        assert!(
            sites.windows(2).all(|w| w[0] == w[1]),
            "4 LUTFFs fit one CLB"
        );
    }

    #[test]
    fn overflow_uses_next_clb() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("big");
        d.add_port("clk", helion_ir::PortDir::In);
        d.add_port("led", helion_ir::PortDir::Out);
        for i in 0..9u32 {
            d.add_cell(
                &format!("u_lut{i}"),
                CellKind::Lut6 {
                    init: 0x5555_5555_5555_5555,
                },
            );
            d.add_cell(&format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", &format!("u_ff{i}"), "CLK");
            d.connect(&format!("d{i}"), &format!("u_lut{i}"), "O");
            d.connect(&format!("d{i}"), &format!("u_ff{i}"), "D");
            d.connect(&format!("q{i}"), &format!("u_ff{i}"), "Q");
            d.connect(&format!("q{i}"), &format!("u_lut{i}"), "I0");
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q0", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        let p = pack(&d, &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        assert_eq!(pl.lutff_sites.len(), 9);
        assert_eq!(pl.lutff_sites[8].1, 0);
        assert_ne!(
            (pl.lutff_sites[0].0.x, pl.lutff_sites[0].0.y),
            (pl.lutff_sites[8].0.x, pl.lutff_sites[8].0.y)
        );
    }

    #[test]
    fn incremental_reuses_named_lutff_sites() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let p = pack(&d, &dev).unwrap();
        let prev = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        let (next, reused) = place_incremental_with_guide(
            &p,
            &dev,
            &prev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
            &TimingGuide::default(),
        )
        .unwrap();
        assert_eq!(reused, prev.lutff_sites.len());
        assert_eq!(next.lutff_sites, prev.lutff_sites);
    }

    // hard_heartbeat: see crate::hard_heartbeat

    fn sites_of(pl: &Placed) -> Vec<(u32, u32, u8)> {
        pl.lutff_sites
            .iter()
            .map(|(s, ble)| (s.x, s.y, *ble))
            .collect()
    }

    #[test]
    fn empty_guide_matches_place_with() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let opts = PlaceOpts {
            timing_weight: 0.75, seed: 0 };
        let a = place_with(&p, &dev, opts).unwrap();
        let b = place_with_guide(&p, &dev, opts, &TimingGuide::default()).unwrap();
        assert_eq!(
            sites_of(&a),
            sites_of(&b),
            "empty guide must keep gold sites"
        );
    }

    #[test]
    fn false_path_guide_moves_sites_on_hard_fixture() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        assert!(p.lutffs.len() >= 8, "fixture must be ≥8 LUT+FF");
        let opts = PlaceOpts {
            timing_weight: 0.75, seed: 0 };
        let unconstrained = place_with_guide(&p, &dev, opts, &TimingGuide::default()).unwrap();
        let fp = place_with_guide(
            &p,
            &dev,
            opts,
            &TimingGuide {
                false_path_iob: true,
                iob_setup_mult: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(
            sites_of(&unconstrained),
            sites_of(&fp),
            "set_false_path must stop IOB pull (unconstrained y={} false_path y={})",
            unconstrained.lutff_sites[0].0.y,
            fp.lutff_sites[0].0.y
        );
    }

    #[test]
    fn multicycle_guide_moves_sites_on_hard_fixture() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        let opts = PlaceOpts {
            timing_weight: 0.75, seed: 0 };
        let unconstrained = place_with_guide(&p, &dev, opts, &TimingGuide::default()).unwrap();
        let mcp = place_with_guide(
            &p,
            &dev,
            opts,
            &TimingGuide {
                false_path_iob: false,
                iob_setup_mult: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(
            sites_of(&unconstrained),
            sites_of(&mcp),
            "set_multicycle_path must weaken IOB pull (unconstrained y={} mcp y={})",
            unconstrained.lutff_sites[0].0.y,
            mcp.lutff_sites[0].0.y
        );
        let fp = place_with_guide(
            &p,
            &dev,
            opts,
            &TimingGuide {
                false_path_iob: true,
                iob_setup_mult: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(
            sites_of(&mcp),
            sites_of(&fp),
            "false_path must not match multicycle sites"
        );
    }

    #[test]
    fn false_path_mid_y_only_for_iob_driving_cluster() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        assert!(p.lutffs.len() >= 8);
        let iob_net = &p.iobs[0].from_net;
        let iob_idx = p
            .lutffs
            .iter()
            .position(|l| l.q_net == *iob_net)
            .expect("IOB-driving cluster");
        assert_ne!(iob_idx, 0, "fixture must have an internal cluster");
        let opts = PlaceOpts {
            timing_weight: 0.75, seed: 0 };
        let unconstrained = place_with_guide(&p, &dev, opts, &TimingGuide::default()).unwrap();
        let fp = place_with_guide(
            &p,
            &dev,
            opts,
            &TimingGuide {
                false_path_iob: true,
                iob_setup_mult: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            unconstrained.lutff_sites[0].0.y, fp.lutff_sites[0].0.y,
            "internal cluster must keep timing_weight south pull (y={} vs {})",
            unconstrained.lutff_sites[0].0.y, fp.lutff_sites[0].0.y
        );
        assert_ne!(
            unconstrained.lutff_sites[iob_idx].0.y, fp.lutff_sites[iob_idx].0.y,
            "IOB-driving cluster must drop south pull (unconstrained y={} false_path y={})",
            unconstrained.lutff_sites[iob_idx].0.y, fp.lutff_sites[iob_idx].0.y
        );
        assert!(
            fp.lutff_sites[iob_idx].0.y > fp.lutff_sites[0].0.y,
            "excepted IOB driver must sit north of internal south pull"
        );
    }

    #[test]
    fn pblock_and_incremental_accept_timing_guide() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        let opts = PlaceOpts {
            timing_weight: 0.75, seed: 0 };
        let g = TimingGuide {
            false_path_iob: true,
            iob_setup_mult: 1,
            ..Default::default()
        };
        let pl = place_in_region_with_guide(&p, &dev, opts, 5, 1, 8, 8, &g).unwrap();
        for (s, _) in &pl.lutff_sites {
            assert!(s.x >= 5 && s.x <= 8 && s.y >= 1 && s.y <= 8, "pblock {s:?}");
        }
        let prev = place_with_guide(&p, &dev, opts, &g).unwrap();
        let (next, reused) = place_incremental_with_guide(&p, &dev, &prev, opts, &g).unwrap();
        assert_eq!(reused, prev.lutff_sites.len());
        assert_eq!(next.lutff_sites, prev.lutff_sites);
    }

    #[test]
    fn packing_summary_matches_placed_lutff_sites_grouping() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let sum = packing_summary_from_placed(&pl);
        let mut expect: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for (s, _) in &pl.lutff_sites {
            *expect.entry(s.id()).or_insert(0) += 1;
        }
        assert_eq!(
            sum.sites, expect,
            "summary must be lutff_sites grouped by Site::id"
        );
        assert!(!sum.sites.is_empty());
        let total: usize = sum.sites.values().sum();
        assert_eq!(total, pl.lutff_sites.len());
        for id in sum.sites.keys() {
            assert!(
                dev.site_by_id(id).is_some(),
                "packing site {id} must resolve in HAD"
            );
        }
        let first = pl.lutff_sites[0].0;
        assert_eq!(sum.lutff_in(&first.id()), expect[&first.id()]);
    }

    #[test]
    fn placed_cells_map_to_had_bel_ids() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let (site, ble) = pl.lutff_sites[3];
        let ff = pl.cell_bel("u_ff3").expect("placed FF");
        assert_eq!(ff.site, site);
        assert_eq!(ff.bel, format!("BLE{ble}.FF"));
        assert_eq!(ff.id(), format!("{}/BLE{ble}.FF", site.id()));
        assert_eq!(dev.site_by_id(&site.id()), Some(site));
        assert!(dev.contains_site(site));
        assert_eq!(dev.resolve_cell_site("u_ff3", site).unwrap(), site.id());
        let lut = pl.cell_bel("u_lut3").unwrap();
        assert_eq!(lut, BelId::new(site, format!("BLE{ble}.LUT")));
        let iob = pl.cell_bel("u_iob").unwrap();
        assert_eq!(iob.bel, "IOB");
        assert_eq!(iob.site, pl.iob_sites[0]);
        assert_eq!(dev.site_by_id(&iob.site.id()), Some(iob.site));
        assert!(pl.cell_bel("ghost").is_none());
        assert_eq!(pl.cell_site("u_ff3"), Some(site));
    }

    /// FM-HEL-OPT-P0-1: device-full / n≥256 must not hard-skip legalize.
    /// Smoke: ≥256 connected LUTFF clusters place without panic (budgeted path).
    #[test]
    fn budgeted_legalize_large_place_without_panic() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut p = pack(&Design::structural_blinky(), &dev).unwrap();
        let proto = p.lutffs[0].clone();
        // Enough to hit large_place (≥256) but well under device to keep the
        // unit test fast; wire each cluster to the previous FF so affinity
        // leaves illegal arcs for budgeted bileg to chew.
        let n = 280usize;
        while p.lutffs.len() < n {
            let i = p.lutffs.len();
            let mut lf = proto.clone();
            lf.lut_cell = format!("u_lut{i}");
            lf.ff_cell = format!("u_ff{i}");
            lf.q_net = format!("q{i}");
            let prev_ff = format!("u_ff{}", i - 1);
            lf.lut_pins = vec![(0u8, prev_ff)];
            p.lutffs.push(lf);
        }
        let pl = place(&p, &dev).expect("budgeted legalize must not panic on n≥256");
        assert_eq!(pl.lutff_sites.len(), n);
        assert_eq!(pl.packed.lutffs.len(), n);
    }

    #[test]
    fn seeded_order_zero_is_identity() {
        assert_eq!(seeded_order(16, 0), (0..16).collect::<Vec<_>>());
    }

    #[test]
    fn seeded_order_is_deterministic_permutation() {
        let a = seeded_order(64, 12345);
        let b = seeded_order(64, 12345);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..64).collect::<Vec<_>>());
        // Different seeds (almost surely) shuffle differently.
        assert_ne!(a, seeded_order(64, 999));
    }

    /// Seed sweeps must not disturb the lutff_sites[i] <-> packed.lutffs[i]
    /// pairing that INIT programming, the router, cell_bel and lutff_of rely
    /// on. Eight pin-less LUTFFs share one site (n_ble = 8), so the k-th
    /// visited LUTFF takes BLE k; in packed order that means
    /// lutff_sites[i].1 == position of i in seeded_order(n, seed).
    /// Recording sites in visitation order (the old bug) yields ble == i
    /// for every seed instead.
    #[test]
    fn seeded_place_keeps_lutff_sites_in_packed_order() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut p = pack(&Design::structural_blinky(), &dev).unwrap();
        let proto = p.lutffs[0].clone();
        p.lutffs.clear();
        let n = 8usize;
        for i in 0..n {
            let mut lf = proto.clone();
            lf.lut_cell = format!("u_lut{i}");
            lf.ff_cell = format!("u_ff{i}");
            lf.q_net = format!("q{i}");
            lf.lut_pins = Vec::new();
            lf.init = 0xAAAAAAAAAAAAAAAAu64 ^ (i as u64);
            p.lutffs.push(lf);
        }
        for seed in [12345u64, 999u64] {
            let perm = seeded_order(n, seed);
            assert_ne!(perm, (0..n).collect::<Vec<_>>());
            let mut pos = vec![0usize; n];
            for (k, &li) in perm.iter().enumerate() {
                pos[li] = k;
            }
            let opts = PlaceOpts {
                timing_weight: 0.0,
                seed,
            };
            let pl = place_with(&p, &dev, opts).expect("seeded place must succeed");
            assert_eq!(pl.lutff_sites.len(), n);
            assert_eq!(pl.packed.lutffs.len(), n);
            let site0 = pl.lutff_sites[0].0;
            for i in 0..n {
                let (site, ble) = pl.lutff_sites[i];
                assert_eq!(site, site0, "seed {seed}: all 8 share one site");
                assert_eq!(
                    ble as usize, pos[i],
                    "seed {seed}: lutff_sites[{i}] must hold packed.lutffs[{i}]'s site"
                );
                // The packed-index pairing must survive the public lookups too.
                let lf = &pl.packed.lutffs[i];
                assert_eq!(pl.cell_bel(&lf.lut_cell).unwrap().site, site);
                assert_eq!(lutff_of(&pl, &lf.ff_cell), Some((site, ble)));
            }
        }
    }

    #[test]
    fn congestion_spread_relieves_dense_clb_sites() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        // All 8 heartbeat LUTFFs pack into a single CLB site initially.
        let initial_max = packing_summary_from_placed(&pl)
            .sites
            .values()
            .copied()
            .max()
            .unwrap_or(0);
        assert_eq!(initial_max, 8);
        let (spread, moved) = place_congestion_spread(&pl, &dev, 4).unwrap();
        assert_eq!(moved, 4, "4 of 8 LUTFFs must spread out when cap=4");
        let after_max = packing_summary_from_placed(&spread)
            .sites
            .values()
            .copied()
            .max()
            .unwrap_or(0);
        assert!(after_max <= 4, "max BLE occupancy per CLB must obey cap=4, got {after_max}");
    }

    #[test]
    fn permute_lut6_init_preserves_truth_table_and_reorders_critical_pins() {
        // Function: f(I0, I1) = I0 & ~I1 (truth table over 6 inputs).
        let mut init = 0u64;
        for addr in 0..64u64 {
            let i0 = addr & 1;
            let i1 = (addr >> 1) & 1;
            if i0 == 1 && i1 == 0 {
                init |= 1u64 << addr;
            }
        }
        // Swap I0 and I1: old I0 -> new I1, old I1 -> new I0.
        let perm = [1, 0, 2, 3, 4, 5];
        let permuted = permute_lut6_init(init, &perm);
        for addr in 0..64u64 {
            let new_i0 = addr & 1;
            let new_i1 = (addr >> 1) & 1;
            let bit = (permuted >> addr) & 1;
            assert_eq!(bit == 1, new_i1 == 1 && new_i0 == 0);
        }
        // Involution: applying the swap again restores the exact original INIT.
        assert_eq!(permute_lut6_init(permuted, &perm), init);
    }

    #[test]
    fn phys_opt_place_reduces_wirelength_and_permutes_critical_lut_pins() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let mut pl = place(&p, &dev).unwrap();
        // Displace cluster 0 far north and put its critical fanin on pin I3 of cluster 3.
        let north_site = dev
            .clb_sites()
            .find(|s| s.x == pl.lutff_sites[0].0.x && s.y == pl.lutff_sites[0].0.y + 3)
            .unwrap();
        pl.lutff_sites[0] = (north_site, 0);
        let (opt, stats) = phys_opt_place_with_stats(&pl, &dev).unwrap();
        assert!(
            stats.wirelength_after < stats.wirelength_before,
            "phys_opt_place must reduce wirelength ({} -> {})",
            stats.wirelength_before,
            stats.wirelength_after
        );
        assert!(stats.relocated_cells + stats.swapped_cells >= 1);
        assert_eq!(opt.lutff_sites.len(), pl.lutff_sites.len());
    }
}
