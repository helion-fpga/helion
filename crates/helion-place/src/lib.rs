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

/// Bring-up IMUX reach: same CLB, N-S ±1/±2, or E-W ±1/±2 (matches helion-route::imux_sel).
fn imux_local(from: Site, to: Site) -> bool {
    if from.x == to.x
        && (from.y == to.y
            || from.y + 1 == to.y
            || to.y + 1 == from.y
            || from.y + 2 == to.y
            || to.y + 2 == from.y)
    {
        return true;
    }
    // Real E-W ±1/±2 same-row neighbor Q (uwilton / bit6 bank).
    from.y == to.y
        && (from.x + 1 == to.x
            || to.x + 1 == from.x
            || from.x + 2 == to.x
            || to.x + 2 == from.x)
}

fn imux_illegal_pins(
    lf: &helion_pack::PackedLutFf,
    site: Site,
    ff_at: &std::collections::HashMap<&str, Site>,
) -> u32 {
    let mut n = 0u32;
    for (_, driver) in &lf.lut_pins {
        match ff_at.get(driver.as_str()) {
            Some(ds) if imux_local(*ds, site) => {}
            Some(_) => n += 1,
            None => {}
        }
    }
    n
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
    for (i, iob) in packed.iobs.iter().enumerate() {
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
        // FF cell → site as we place (IMUX encodes same-CLB / N-S ±1/±2 / E-W ±1/±2).
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
            let mut try_xs: Vec<u32> = Vec::with_capacity(8 + affinity.len() * 5 + all_xs.len());
            for s in &affinity {
                try_xs.push(s.x);
                // Harder cluster: keep sink in E-W±1/±2 of drivers before sprawl.
                try_xs.push(s.x.saturating_add(1));
                try_xs.push(s.x.saturating_add(2));
                if s.x > 0 {
                    try_xs.push(s.x - 1);
                }
                if s.x > 1 {
                    try_xs.push(s.x - 2);
                }
            }
            try_xs.push(preferred_x);
            if fallback_iob.x != preferred_x {
                try_xs.push(fallback_iob.x);
            }
            for &x in &all_xs {
                try_xs.push(x);
            }
            // dedup preserving order
            {
                let mut seen = HashSet::new();
                try_xs.retain(|x| seen.insert(*x));
            }
            let mut placed = None;
            'cols: for col_x in try_xs {
                let Some(col) = cols.get(&col_x) else { continue };
                if col.is_empty() {
                    continue;
                }
                // Preferred Y order: affinity sites in this column, then ±1, then
                // south/mid wrap so the full 8192 BLE budget is reachable.
                let mut y_order: Vec<u32> = Vec::with_capacity(col.len());
                for s in &affinity {
                    if s.x == col_x {
                        y_order.push(s.y);
                        y_order.push(s.y.saturating_add(1));
                        y_order.push(s.y.saturating_add(2));
                        if s.y > 0 {
                            y_order.push(s.y - 1);
                        }
                        if s.y > 1 {
                            y_order.push(s.y - 2);
                        }
                    }
                }
                let base_y = if prefer_south {
                    col.first().map(|s| s.y).unwrap_or(0)
                } else {
                    col[col.len() / 2].y
                };
                y_order.push(base_y);
                for s in col {
                    y_order.push(s.y);
                }
                {
                    let mut seen = HashSet::new();
                    y_order.retain(|y| seen.insert(*y));
                }
                for y in y_order {
                    let Some(&site) = col.iter().find(|s| s.y == y) else { continue };
                    for ble in 0..n_ble as u8 {
                        if used.insert((site.x, site.y, ble)) {
                            placed = Some((site, ble));
                            break 'cols;
                        }
                    }
                }
            }
            let site_ble = placed.ok_or_else(|| "no CLB/BLE site left for LUTFF".to_string())?;
            if !lf.ff_cell.is_empty() {
                ff_at.insert(lf.ff_cell.as_str(), site_ble.0);
            }
            lutff_sites.push(site_ble);
        }

            // FM-HEL-TOP: stronger IMUX legalization — pull sinks onto driver
            // same-CLB / N-S±1/±2 / E-W±1/±2 (real HAD reach); empty-BLE move then
            // pairwise swap when sites are full.
            let mut site_of: std::collections::HashMap<(u32, u32, u8), usize> =
                std::collections::HashMap::new();
            for (i, (s, ble)) in lutff_sites.iter().enumerate() {
                site_of.insert((s.x, s.y, *ble), i);
            }
            let push_reach = |xy: &mut Vec<(u32, u32)>, x: u32, y: u32| {
                xy.push((x, y));
                xy.push((x, y.saturating_add(1)));
                xy.push((x, y.saturating_add(2)));
                if y > 0 {
                    xy.push((x, y - 1));
                }
                if y > 1 {
                    xy.push((x, y - 2));
                }
                // E-W ±1/±2 same row (matches IMUX reach)
                xy.push((x.saturating_add(1), y));
                xy.push((x.saturating_add(2), y));
                if x > 0 {
                    xy.push((x - 1, y));
                }
                if x > 1 {
                    xy.push((x - 2, y));
                }
            };
            let mut moved = 0u32;
            let mut swapped = 0u32;
            for _pass in 0..12 {
                let mut pass_moved = 0u32;
                let mut pass_swapped = 0u32;
                for (i, lf) in packed.lutffs.iter().enumerate() {
                    if lf.lut_pins.is_empty() {
                        continue;
                    }
                    let (cur_site, cur_ble) = lutff_sites[i];
                    let before = imux_illegal_pins(lf, cur_site, &ff_at);
                    if before == 0 {
                        continue;
                    }
                    let mut cand_xy: Vec<(u32, u32)> = Vec::new();
                    for (_, driver) in &lf.lut_pins {
                        if let Some(ds) = ff_at.get(driver.as_str()).copied() {
                            push_reach(&mut cand_xy, ds.x, ds.y);
                        }
                    }
                    {
                        let mut seen = HashSet::new();
                        cand_xy.retain(|xy| seen.insert(*xy));
                    }
                    // 1) Prefer empty BLE on a legal candidate.
                    let mut best: Option<(Site, u8, u32)> = None;
                    for (cx, cy) in &cand_xy {
                        let Some(site) = cols
                            .get(cx)
                            .and_then(|c| c.iter().find(|s| s.y == *cy).copied())
                        else {
                            continue;
                        };
                        for ble in 0..n_ble as u8 {
                            let key = (site.x, site.y, ble);
                            if key == (cur_site.x, cur_site.y, cur_ble) {
                                continue;
                            }
                            if used.contains(&key) {
                                continue;
                            }
                            let ill = imux_illegal_pins(lf, site, &ff_at);
                            if ill < before && best.as_ref().map(|b| ill < b.2).unwrap_or(true) {
                                best = Some((site, ble, ill));
                                if ill == 0 {
                                    break;
                                }
                            }
                        }
                        if best.map(|b| b.2) == Some(0) {
                            break;
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
                    // Mutate/restore ff_at (no HashMap clone — Ibex-scale).
                    let mut best_swap: Option<(usize, Site, u8, u32)> = None;
                    for (cx, cy) in &cand_xy {
                        let Some(site) = cols
                            .get(cx)
                            .and_then(|c| c.iter().find(|s| s.y == *cy).copied())
                        else {
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
                            let other_before = imux_illegal_pins(other, osite, &ff_at);
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
                            let ill_i = imux_illegal_pins(lf, site, &ff_at);
                            let ill_j = imux_illegal_pins(other, cur_site, &ff_at);
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
                            let after = ill_i + ill_j;
                            let before_tot = before + other_before;
                            if after >= before_tot {
                                continue;
                            }
                            if best_swap.as_ref().map(|b| after < b.3).unwrap_or(true) {
                                best_swap = Some((j, site, ble, after));
                                if after == 0 {
                                    break;
                                }
                            }
                        }
                        if best_swap.map(|b| b.3) == Some(0) {
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
                        // used keys unchanged (swap)
                        if !lf.ff_cell.is_empty() {
                            ff_at.insert(lf.ff_cell.as_str(), site);
                        }
                        if !packed.lutffs[j].ff_cell.is_empty() {
                            ff_at.insert(packed.lutffs[j].ff_cell.as_str(), cur_site);
                        }
                        pass_swapped += 1;
                    }
                }
                moved += pass_moved;
                swapped += pass_swapped;
                if pass_moved == 0 && pass_swapped == 0 {
                    break;
                }
            }
            if moved > 0 || swapped > 0 {
                eprintln!(
                    "hang_diag place imux_legalize moved={moved} swapped={swapped}"
                );
            }
    }

    let mut mac_sites = Vec::new();
    let dsps: Vec<_> = dev.dsp_sites().collect();
    if packed.macs.len() > dsps.len() {
        return Err(format!(
            "need {} DSP sites, device has {}",
            packed.macs.len(),
            dsps.len()
        ));
    }
    for (i, _) in packed.macs.iter().enumerate() {
        mac_sites.push(dsps[i]);
    }

    let mut bram_sites = Vec::new();
    let brams: Vec<_> = dev.bram_sites().collect();
    if packed.brams.len() > brams.len() {
        return Err(format!(
            "need {} BRAM sites, device has {}",
            packed.brams.len(),
            brams.len()
        ));
    }
    for (i, _) in packed.brams.iter().enumerate() {
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

    Ok(Placed {
        packed: packed.clone(),
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
