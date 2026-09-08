//! Place packed clusters onto Helion sites from the device database.

use helion_device::{Device, Site};
use helion_pack::Packed;
use std::collections::HashSet;

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

#[derive(Clone, Copy, Debug)]
pub struct PlaceOpts {
    /// 0 = wirelength toward array center. >0 = timing: pull toward IOB.
    pub timing_weight: f64,
}

impl Default for PlaceOpts {
    fn default() -> Self {
        Self { timing_weight: 0.0 }
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
    (dx == 1 && dy == 1)
        || (dx == 2 && dy == 2)
        || (dx == 2 && dy == 1)
        || (dx == 1 && dy == 2)
}

fn imux_illegal_pins(
    lf: &helion_pack::PackedLutFf,
    site: Site,
    ff_at: &std::collections::HashMap<&str, Site>,
) -> u32 {
    imux_score(lf, site, ff_at).0
}

/// Single pin walk: (illegal_count, manhattan_spill). Spill is Manhattan over
/// out-of-reach arcs for gradient legalize. Hot path — one walk not two.
fn imux_score(
    lf: &helion_pack::PackedLutFf,
    site: Site,
    ff_at: &std::collections::HashMap<&str, Site>,
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

fn parse_iob_loc(loc: &str, sites: &[Site]) -> Option<Site> {
    let rest = loc.strip_prefix("IOB_X")?;
    let (xs, ys) = rest.split_once('Y')?;
    let x: u32 = xs.parse().ok()?;
    let y: u32 = ys.parse().ok()?;
    sites.iter().copied().find(|s| s.x == x && s.y == y)
}

pub fn place(packed: &Packed, dev: &Device) -> Result<Placed, String> {
    place_with(packed, dev, PlaceOpts::default())
}

pub fn place_with(packed: &Packed, dev: &Device, opts: PlaceOpts) -> Result<Placed, String> {
    let iob_all: Vec<Site> = dev.iob_sites().collect();
    let mut iob_sites = Vec::new();
    let iob_take = packed.iobs.len().min(iob_all.len());
    if packed.iobs.len() > iob_all.len() {
        eprintln!(
            "hang_diag place_iob_cap {} -> {} (device IOB sites)",
            packed.iobs.len(),
            iob_take
        );
    }
    for (i, iob) in packed.iobs.iter().enumerate().take(iob_take) {
        let s = if let Some(loc) = &iob.loc {
            parse_iob_loc(loc, &iob_all)
                .ok_or_else(|| format!("LOC {loc} is not an IOB site"))?
        } else {
            *iob_all.get(i).ok_or_else(|| {
                format!("need {} IOB sites, device has {}", packed.iobs.len(), iob_all.len())
            })?
        };
        iob_sites.push(s);
    }

    let mut lutff_sites = Vec::new();
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
        let prefer_south = opts.timing_weight > 0.0;
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
        let mut iob_for_net: std::collections::HashMap<&str, Site> = std::collections::HashMap::new();
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
        for lf in &packed.lutffs {
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
                let mut y_order: Vec<u32> = Vec::with_capacity(if full_y {
                    col.len() + 8
                } else {
                    16
                });
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
                let base_y = if prefer_south {
                    col.first().map(|s| s.y).unwrap_or(0)
                } else {
                    col[col.len() / 2].y
                };
                y_order.push(base_y);
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
            lutff_sites.push(site_ble);
        }

            let nplace = lutff_sites.len();
            eprintln!(
                "hang_diag place affinity lutffs={} ms={}",
                nplace,
                t_aff.elapsed().as_millis()
            );
            let t_leg = std::time::Instant::now();
            // FM-HEL-TOP: bidirectional IMUX legalization — pull sinks toward
            // drivers AND drivers toward sinks onto real HAD reach (same-CLB /
            // N-S±1/±2 / E-W±1/±2 / diag±1 / knight); empty-BLE move then
            // pairwise swap when sites are full.
            let mut site_of: std::collections::HashMap<(u32, u32, u8), usize> =
                std::collections::HashMap::new();
            for (i, (s, ble)) in lutff_sites.iter().enumerate() {
                site_of.insert((s.x, s.y, *ble), i);
            }
            // O(1) (x,y)→Site for legalize candidate resolution (vs linear col scan).
            let mut site_xy: std::collections::HashMap<(u32, u32), Site> =
                std::collections::HashMap::new();
            for col in cols.values() {
                for &s in col {
                    site_xy.insert((s.x, s.y), s);
                }
            }
            // Reused across cells/passes to cut HashSet alloc churn on cand_xy dedup.
            let mut cand_seen: HashSet<(u32, u32)> = HashSet::new();
            // Reverse fanout: driver FF cell → unique sink LUTFF indices (bileg).
            let mut sinks_of: std::collections::HashMap<&str, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, lf) in packed.lutffs.iter().enumerate().take(nplace) {
                for (_, driver) in &lf.lut_pins {
                    let v = sinks_of.entry(driver.as_str()).or_default();
                    if !v.contains(&i) {
                        v.push(i);
                    }
                }
            }
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
            // Driver move score: (fanout, fan_spill, own_inputs). One sink walk
            // (was illegal_fanout + spill). Lex better = less fanout, then spill,
            // then own inputs — prefer collapsing long arcs even if inputs
            // briefly worsen (sink-phase repairs inputs next pass).
            let drv_score = |d_idx: usize,
                             d_ff: &str,
                             d_site: Site,
                             sink_idxs: &[usize],
                             lutff_sites: &[(Site, u8)],
                             ff_at: &std::collections::HashMap<&str, Site>|
             -> (u32, u32, u32) {
                let mut fan_illegal = 0u32;
                let mut fan_spill = 0u32;
                for &si in sink_idxs {
                    let (ss, _) = lutff_sites[si];
                    for (_, driver) in &packed.lutffs[si].lut_pins {
                        if driver.as_str() == d_ff && !imux_local(d_site, ss) {
                            fan_illegal += 1;
                            fan_spill += d_site.x.abs_diff(ss.x) + d_site.y.abs_diff(ss.y);
                        }
                    }
                }
                (
                    fan_illegal,
                    fan_spill,
                    imux_illegal_pins(&packed.lutffs[d_idx], d_site, ff_at),
                )
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
            let already_legal = packed.lutffs.iter().enumerate().take(nplace).all(|(i, lf)| {
                let (site, _) = lutff_sites[i];
                imux_illegal_pins(lf, site, &ff_at) == 0
            });
            let pass_limit = if already_legal || nplace >= dev.lut6_count() as usize {
                0
            } else {
                32
            };
            for _pass in 0..pass_limit {
                let mut pass_moved = 0u32;
                let mut pass_swapped = 0u32;
                let mut pass_drv_moved = 0u32;
                let mut pass_drv_swapped = 0u32;
                // --- Phase A: pull sinks toward drivers (existing) ---
                for (i, lf) in packed.lutffs.iter().enumerate().take(nplace) {
                    if lf.lut_pins.is_empty() {
                        continue;
                    }
                    let (cur_site, cur_ble) = lutff_sites[i];
                    let before_sc = imux_score(lf, cur_site, &ff_at);
                    let before = before_sc.0;
                    let before_sp = before_sc.1;
                    if before == 0 {
                        continue;
                    }
                    let mut cand_xy: Vec<(u32, u32)> = Vec::new();
                    for (_, driver) in &lf.lut_pins {
                        if let Some(ds) = ff_at.get(driver.as_str()).copied() {
                            push_reach(&mut cand_xy, ds.x, ds.y);
                            if !imux_local(ds, cur_site) {
                                push_gradient(
                                    &mut cand_xy,
                                    cur_site.x,
                                    cur_site.y,
                                    ds.x,
                                    ds.y,
                                );
                            }
                        }
                    }
                    cand_seen.clear();
                    cand_xy.retain(|xy| cand_seen.insert(*xy));
                    // 1) Prefer empty BLE: score is site-only (BLE-independent) — once/site.
                    let mut best: Option<(Site, u8, (u32, u32))> = None;
                    for (cx, cy) in &cand_xy {
                        let Some(&site) = site_xy.get(&(*cx, *cy)) else {
                            continue;
                        };
                        let sc = imux_score(lf, site, &ff_at);
                        if sc >= before_sc || best.as_ref().map(|b| sc >= b.2).unwrap_or(false) {
                            continue;
                        }
                        let mut found_ble: Option<u8> = None;
                        for ble in 0..n_ble as u8 {
                            let key = (site.x, site.y, ble);
                            if key == (cur_site.x, cur_site.y, cur_ble) {
                                continue;
                            }
                            if used.contains(&key) {
                                continue;
                            }
                            found_ble = Some(ble);
                            break;
                        }
                        if let Some(ble) = found_ble {
                            best = Some((site, ble, sc));
                            if sc.0 == 0 {
                                break;
                            }
                        }
                    }
                    if let Some((site, ble, _)) = best {
                        used.remove(&(cur_site.x, cur_site.y, cur_ble));
                        used.insert((site.x, site.y, ble));
                        site_of.remove(&(cur_site.x, cur_site.y, cur_ble));
                        site_of.insert((site.x, site.y, ble), i);
                        lutff_sites[i] = (site, ble);
                        if !lf.ff_cell.is_empty() {
                            ff_at.insert(lf.ff_cell.as_str(), site);
                        }
                        pass_moved += 1;
                        continue;
                    }
                    // 2) Pairwise swap with occupant when total illegal pins drop.
                    let mut best_swap: Option<(usize, Site, u8, (u32, u32))> = None;
                    for (cx, cy) in &cand_xy {
                        let Some(&site) = site_xy.get(&(*cx, *cy)) else {
                            continue;
                        };
                        for ble in 0..n_ble as u8 {
                            let key = (site.x, site.y, ble);
                            if key == (cur_site.x, cur_site.y, cur_ble) {
                                continue;
                            }
                            let Some(&j) = site_of.get(&key) else {
                                continue;
                            };
                            if j == i {
                                continue;
                            }
                            let other = &packed.lutffs[j];
                            let (osite, _oble) = lutff_sites[j];
                            let other_before_sc = imux_score(other, osite, &ff_at);
                            let other_before = other_before_sc.0;
                            let i_ff = lf.ff_cell.as_str();
                            let j_ff = other.ff_cell.as_str();
                            let i_prev = if !i_ff.is_empty() {
                                ff_at.insert(i_ff, site)
                            } else {
                                None
                            };
                            let j_prev = if !j_ff.is_empty() {
                                ff_at.insert(j_ff, cur_site)
                            } else {
                                None
                            };
                            let sc_i = imux_score(lf, site, &ff_at);
                            let sc_j = imux_score(other, cur_site, &ff_at);
                            // restore
                            if !i_ff.is_empty() {
                                match i_prev {
                                    Some(s) => {
                                        ff_at.insert(i_ff, s);
                                    }
                                    None => {
                                        ff_at.remove(i_ff);
                                    }
                                }
                            }
                            if !j_ff.is_empty() {
                                match j_prev {
                                    Some(s) => {
                                        ff_at.insert(j_ff, s);
                                    }
                                    None => {
                                        ff_at.remove(j_ff);
                                    }
                                }
                            }
                            let after = (sc_i.0 + sc_j.0, sc_i.1 + sc_j.1);
                            let before_tot = (
                                before + other_before,
                                before_sp + other_before_sc.1,
                            );
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
                        if best_swap.as_ref().map(|b| b.3 .0) == Some(0) {
                            break;
                        }
                    }
                    if let Some((j, site, ble, _)) = best_swap {
                        let (osite, oble) = lutff_sites[j];
                        site_of.remove(&(cur_site.x, cur_site.y, cur_ble));
                        site_of.remove(&(osite.x, osite.y, oble));
                        lutff_sites[i] = (site, ble);
                        lutff_sites[j] = (cur_site, cur_ble);
                        site_of.insert((site.x, site.y, ble), i);
                        site_of.insert((cur_site.x, cur_site.y, cur_ble), j);
                        if !lf.ff_cell.is_empty() {
                            ff_at.insert(lf.ff_cell.as_str(), site);
                        }
                        if !packed.lutffs[j].ff_cell.is_empty() {
                            ff_at.insert(packed.lutffs[j].ff_cell.as_str(), cur_site);
                        }
                        pass_swapped += 1;
                    }
                }
                // --- Phase B: pull drivers toward illegal sinks (bileg) ---
                // Fanout-primary: accept move if illegal fanout drops (lex), even
                // when driver inputs briefly worsen — sink-phase repairs next pass.
                for (i, lf) in packed.lutffs.iter().enumerate().take(nplace) {
                    if lf.ff_cell.is_empty() {
                        continue;
                    }
                    let d_ff = lf.ff_cell.as_str();
                    let Some(sink_idxs) = sinks_of.get(d_ff).map(|v| v.as_slice()) else {
                        continue;
                    };
                    if sink_idxs.is_empty() {
                        continue;
                    }
                    let (cur_site, cur_ble) = lutff_sites[i];
                    let before = drv_score(i, d_ff, cur_site, sink_idxs, &lutff_sites, &ff_at);
                    if before.0 == 0 {
                        continue;
                    }
                    // Candidates: IMUX-reach of illegal sinks + gradient walk toward them.
                    let mut cand_xy: Vec<(u32, u32)> = Vec::new();
                    for &si in sink_idxs {
                        let (ss, _) = lutff_sites[si];
                        if !imux_local(cur_site, ss) {
                            push_reach(&mut cand_xy, ss.x, ss.y);
                            push_gradient(
                                &mut cand_xy,
                                cur_site.x,
                                cur_site.y,
                                ss.x,
                                ss.y,
                            );
                        }
                    }
                    cand_seen.clear();
                    cand_xy.retain(|xy| cand_seen.insert(*xy));
                    // 1) Empty BLE move for driver — score site-only once (BLE-independent).
                    let mut best: Option<(Site, u8, (u32, u32, u32))> = None;
                    for (cx, cy) in &cand_xy {
                        let Some(&site) = site_xy.get(&(*cx, *cy)) else {
                            continue;
                        };
                        let prev = ff_at.insert(d_ff, site);
                        let sc = drv_score(i, d_ff, site, sink_idxs, &lutff_sites, &ff_at);
                        match prev {
                            Some(s) => {
                                ff_at.insert(d_ff, s);
                            }
                            None => {
                                ff_at.remove(d_ff);
                            }
                        }
                        if sc >= before || best.as_ref().map(|b| sc >= b.2).unwrap_or(false) {
                            continue;
                        }
                        let mut found_ble: Option<u8> = None;
                        for ble in 0..n_ble as u8 {
                            let key = (site.x, site.y, ble);
                            if key == (cur_site.x, cur_site.y, cur_ble) {
                                continue;
                            }
                            if used.contains(&key) {
                                continue;
                            }
                            found_ble = Some(ble);
                            break;
                        }
                        if let Some(ble) = found_ble {
                            best = Some((site, ble, sc));
                            if sc.0 == 0 {
                                break;
                            }
                        }
                    }
                    if let Some((site, ble, _)) = best {
                        used.remove(&(cur_site.x, cur_site.y, cur_ble));
                        used.insert((site.x, site.y, ble));
                        site_of.remove(&(cur_site.x, cur_site.y, cur_ble));
                        site_of.insert((site.x, site.y, ble), i);
                        lutff_sites[i] = (site, ble);
                        ff_at.insert(d_ff, site);
                        pass_drv_moved += 1;
                        continue;
                    }
                    // 2) Pairwise swap: driver ↔ occupant. Lex score on
                    // (drv_fanout + other_fanout, drv_inputs + other_inputs).
                    let mut best_swap: Option<(usize, Site, u8, (u32, u32, u32))> = None;
                    for (cx, cy) in &cand_xy {
                        let Some(&site) = site_xy.get(&(*cx, *cy)) else {
                            continue;
                        };
                        for ble in 0..n_ble as u8 {
                            let key = (site.x, site.y, ble);
                            if key == (cur_site.x, cur_site.y, cur_ble) {
                                continue;
                            }
                            let Some(&j) = site_of.get(&key) else {
                                continue;
                            };
                            if j == i {
                                continue;
                            }
                            let other = &packed.lutffs[j];
                            let (osite, _oble) = lutff_sites[j];
                            let j_ff = other.ff_cell.as_str();
                            let j_sinks: &[usize] = if !j_ff.is_empty() {
                                sinks_of.get(j_ff).map(|v| v.as_slice()).unwrap_or(&[])
                            } else {
                                &[]
                            };
                            let before_j = if !j_ff.is_empty() {
                                drv_score(j, j_ff, osite, j_sinks, &lutff_sites, &ff_at)
                            } else {
                                (0, 0, imux_illegal_pins(other, osite, &ff_at))
                            };
                            let before_tot = (
                                before.0 + before_j.0,
                                before.1 + before_j.1,
                                before.2 + before_j.2,
                            );
                            let i_prev = ff_at.insert(d_ff, site);
                            let j_prev = if !j_ff.is_empty() {
                                ff_at.insert(j_ff, cur_site)
                            } else {
                                None
                            };
                            let after_i = drv_score(i, d_ff, site, sink_idxs, &lutff_sites, &ff_at);
                            let after_j = if !j_ff.is_empty() {
                                drv_score(j, j_ff, cur_site, j_sinks, &lutff_sites, &ff_at)
                            } else {
                                (0, 0, imux_illegal_pins(other, cur_site, &ff_at))
                            };
                            match i_prev {
                                Some(s) => {
                                    ff_at.insert(d_ff, s);
                                }
                                None => {
                                    ff_at.remove(d_ff);
                                }
                            }
                            if !j_ff.is_empty() {
                                match j_prev {
                                    Some(s) => {
                                        ff_at.insert(j_ff, s);
                                    }
                                    None => {
                                        ff_at.remove(j_ff);
                                    }
                                }
                            }
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
                        if best_swap.as_ref().map(|b| b.3 .0) == Some(0) {
                            break;
                        }
                    }
                    if let Some((j, site, ble, _)) = best_swap {
                        let (osite, oble) = lutff_sites[j];
                        site_of.remove(&(cur_site.x, cur_site.y, cur_ble));
                        site_of.remove(&(osite.x, osite.y, oble));
                        lutff_sites[i] = (site, ble);
                        lutff_sites[j] = (cur_site, cur_ble);
                        site_of.insert((site.x, site.y, ble), i);
                        site_of.insert((cur_site.x, cur_site.y, cur_ble), j);
                        ff_at.insert(d_ff, site);
                        if !packed.lutffs[j].ff_cell.is_empty() {
                            ff_at.insert(packed.lutffs[j].ff_cell.as_str(), cur_site);
                        }
                        pass_drv_swapped += 1;
                    }
                }
                moved += pass_moved;
                swapped += pass_swapped;
                driver_moved += pass_drv_moved;
                driver_swapped += pass_drv_swapped;
                if pass_moved == 0
                    && pass_swapped == 0
                    && pass_drv_moved == 0
                    && pass_drv_swapped == 0
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
        lutff_sites
            .first()
            .map(|(s, _)| s.y as f64)
            .unwrap_or(0.0)
    };

    let mut packed = packed.clone();
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
/// Hits the same site picker as `place_with`, then relocates LUTFF (and IOB if
/// the rectangle covers HAD IOB rows) into the region.
pub fn place_in_region(
    packed: &Packed,
    dev: &Device,
    opts: PlaceOpts,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Result<Placed, String> {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    let mut placed = place_with(packed, dev, opts)?;
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
pub fn place_incremental(
    packed: &Packed,
    dev: &Device,
    prev: &Placed,
    opts: PlaceOpts,
) -> Result<(Placed, usize), String> {
    let mut placed = place_with(packed, dev, opts)?;
    let mut reused = 0usize;
    let mut used: HashSet<(u32, u32, u8)> = HashSet::new();
    for (i, lf) in packed.lutffs.iter().enumerate() {
        if let Some(j) = prev
            .packed
            .lutffs
            .iter()
            .position(|p| p.lut_cell == lf.lut_cell)
        {
            let site = prev.lutff_sites[j];
            placed.lutff_sites[i] = site;
            used.insert((site.0.x, site.0.y, site.1));
            reused += 1;
        }
    }
    for (i, iob) in packed.iobs.iter().enumerate() {
        if let Some(j) = prev.packed.iobs.iter().position(|p| p.cell == iob.cell) {
            placed.iob_sites[i] = prev.iob_sites[j];
        }
    }
    let _ = used;
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
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0 }).unwrap();
        let td = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
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
        let def = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let pl = place_in_region(&p, &dev, PlaceOpts { timing_weight: 0.75 }, 5, 1, 8, 8).unwrap();
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
        assert!(sites.windows(2).all(|w| w[0] == w[1]), "4 LUTFFs fit one CLB");
    }

    #[test]
    fn overflow_uses_next_clb() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("big");
        d.add_port("clk", helion_ir::PortDir::In);
        d.add_port("led", helion_ir::PortDir::Out);
        for i in 0..9u32 {
            d.add_cell(format!("u_lut{i}"), CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
            d.add_cell(format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", format!("u_ff{i}"), "CLK");
            d.connect(format!("d{i}"), format!("u_lut{i}"), "O");
            d.connect(format!("d{i}"), format!("u_ff{i}"), "D");
            d.connect(format!("q{i}"), format!("u_ff{i}"), "Q");
            d.connect(format!("q{i}"), format!("u_lut{i}"), "I0");
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q0", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        let p = pack(&d, &dev).unwrap();
        let pl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
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
        let prev = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let (next, reused) = place_incremental(&p, &dev, &prev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        assert_eq!(reused, prev.lutff_sites.len());
        assert_eq!(next.lutff_sites, prev.lutff_sites);
    }
}
