//! PathFinder negotiated routing on the Helion tile RR graph.
//! Intra-CLB IMUX (sel 16+k = local BLE k Q); IOB via A* on the tile grid.

use helion_device::{Device, Site};
use helion_place::{lutff_of, Placed};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct Routed {
    pub placed: Placed,
    /// IOB (x,y) driven by CLB (cx,cy) BLE
    pub iob_src: Vec<IobRoute>,
    pub imux: Vec<ImuxRoute>,
    pub pathfinder_iters: u32,
    pub overused: u32,
}

#[derive(Clone, Debug)]
pub struct IobRoute {
    pub iob: (u32, u32),
    pub clb: (u32, u32),
    pub ble: u8,
    pub hops: u32,
    /// Path delay in ps (hops × HOP_DELAY_PS). Used by STA.
    pub delay_ps: i64,
    /// PathFinder tiles from CLB to IOB (inclusive). Extra hops are delay-only.
    pub path: Vec<(u32, u32)>,
    /// Packed IOB `from_net` this route drives (HNF net, not a chrome label).
    pub net: String,
}

/// One tile hop delay (ps). Folded into PathFinder negotiated cost.
pub const HOP_DELAY_PS: i64 = 40;

#[derive(Clone, Copy, Debug)]
pub struct ImuxRoute {
    pub x: u32,
    pub y: u32,
    pub mux: u32,
    pub sel: u8,
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct Item {
    cost: i64,
    x: u32,
    y: u32,
}

impl Ord for Item {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.cmp(&self.cost)
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn on_grid(dev: &Device, x: u32, y: u32) -> bool {
    let iob_y = dev.clb_y0.saturating_sub(1);
    if y == iob_y && x >= dev.clb_x0 && x < dev.clb_x0 + dev.interior_cols {
        return true;
    }
    dev.clb_major(x, y).is_some()
}

fn neighbors(dev: &Device, x: u32, y: u32) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for (dx, dy) in [(0i32, 1), (0, -1), (1, 0), (-1, 0)] {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx < 0 || ny < 0 {
            continue;
        }
        let (nx, ny) = (nx as u32, ny as u32);
        if on_grid(dev, nx, ny) {
            v.push((nx, ny));
        }
    }
    v
}

fn manhattan(a: (u32, u32), b: (u32, u32)) -> u32 {
    a.0.abs_diff(b.0) + a.1.abs_diff(b.1)
}

fn astar(
    dev: &Device,
    src: (u32, u32),
    dst: (u32, u32),
    hist: &HashMap<(u32, u32), u32>,
    pres: &HashMap<(u32, u32), u32>,
    pres_fac: i64,
) -> Result<Vec<(u32, u32)>, String> {
    let mut open = BinaryHeap::new();
    let mut came: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut g: HashMap<(u32, u32), i64> = HashMap::new();
    g.insert(src, 0);
    open.push(Item {
        cost: manhattan(src, dst) as i64,
        x: src.0,
        y: src.1,
    });
    let mut seen = HashSet::new();
    while let Some(Item { x, y, .. }) = open.pop() {
        if !seen.insert((x, y)) {
            continue;
        }
        if (x, y) == dst {
            let mut path = vec![(x, y)];
            let mut cur = (x, y);
            while cur != src {
                cur = *came.get(&cur).ok_or("astar broke")?;
                path.push(cur);
            }
            path.reverse();
            return Ok(path);
        }
        let gc = *g.get(&(x, y)).unwrap_or(&i64::MAX);
        for (nx, ny) in neighbors(dev, x, y) {
            let h = *hist.get(&(nx, ny)).unwrap_or(&0) as i64;
            let p = *pres.get(&(nx, ny)).unwrap_or(&0) as i64;
            // delay-driven: hop delay dominates, congestion still negotiates
            let step = HOP_DELAY_PS + h + p * pres_fac;
            let ng = gc + step;
            if ng < *g.get(&(nx, ny)).unwrap_or(&i64::MAX) {
                g.insert((nx, ny), ng);
                came.insert((nx, ny), (x, y));
                let f = ng + manhattan((nx, ny), dst) as i64;
                open.push(Item {
                    cost: f,
                    x: nx,
                    y: ny,
                });
            }
        }
    }
    Err(format!("PathFinder: no path {src:?} → {dst:?}"))
}

fn imux_sel(from: Site, to: Site, dble: u8) -> Result<u8, String> {
    if from.x == to.x && from.y == to.y {
        return Ok(16 + dble);
    }
    if from.x == to.x && from.y + 1 == to.y {
        // driver is south of sink
        return Ok(dble);
    }
    if from.x == to.x && to.y + 1 == from.y {
        // driver is north of sink
        return Ok(8 + dble);
    }
    Err(format!(
        "IMUX: no local/N-S encoding from CLB_X{}Y{} BLE{dble} to CLB_X{}Y{}",
        from.x, from.y, to.x, to.y
    ))
}

/// UG986 Lab 1/3: router effort and directed extra hops (Helion equivalent of
/// RuntimeOptimized vs FIXED_ROUTE detours). Default matches the gold PathFinder.
#[derive(Clone, Copy, Debug)]
pub struct RouteOpts {
    pub max_iters: u32,
    /// Added to every IOB route after PathFinder (Lab 3 directed delay).
    pub extra_hops: u32,
}

impl Default for RouteOpts {
    fn default() -> Self {
        Self {
            max_iters: 8,
            extra_hops: 0,
        }
    }
}

pub fn route(placed: &Placed, dev: &Device) -> Result<Routed, String> {
    route_with(placed, dev, RouteOpts::default())
}

pub fn route_with(placed: &Placed, dev: &Device, opts: RouteOpts) -> Result<Routed, String> {
    let mut imux = Vec::new();
    for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
        let (site, ble) = placed.lutff_sites[i];
        if lutff.lut_pins.is_empty() {
            imux.push(ImuxRoute {
                x: site.x,
                y: site.y,
                mux: ble as u32 * 8,
                sel: 16 + ble,
            });
            continue;
        }
        for (pin, driver) in &lutff.lut_pins {
            let (dsite, dble) = lutff_of(placed, driver)
                .ok_or_else(|| format!("driver FF {driver} not placed"))?;
            let sel = imux_sel(dsite, site, dble)?;
            imux.push(ImuxRoute {
                x: site.x,
                y: site.y,
                mux: ble as u32 * 8 + *pin as u32,
                sel,
            });
        }
    }

    let mut nets: Vec<((u32, u32), (u32, u32), u8)> = Vec::new();
    if !placed.packed.lutffs.is_empty() {
        for (ii, iob_site) in placed.iob_sites.iter().enumerate() {
            let packed_iob = placed.packed.iobs.get(ii);
            let idx = packed_iob
                .and_then(|io| {
                    placed
                        .packed
                        .lutffs
                        .iter()
                        .position(|l| l.q_net == io.from_net)
                })
                .unwrap_or(0);
            let (clb, ble) = placed.lutff_sites[idx];
            if clb.y <= iob_site.y {
                return Err("CLB must be north of IOB".into());
            }
            nets.push(((clb.x, clb.y), (iob_site.x, iob_site.y), ble));
        }
    }

    let mut hist: HashMap<(u32, u32), u32> = HashMap::new();
    let mut iob_src = Vec::new();
    let mut overused = 0u32;
    let mut iters = 0u32;
    if nets.is_empty() {
        return Ok(Routed {
            placed: placed.clone(),
            iob_src,
            imux,
            pathfinder_iters: 0,
            overused: 0,
        });
    }
    let max_iters = opts.max_iters.max(1);
    let mut last_paths: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nets.len()];
    for iter in 0..max_iters {
        iters = iter + 1;
        let mut pres: HashMap<(u32, u32), u32> = HashMap::new();
        let pres_fac = 1i64 + iter as i64;
        let mut paths = Vec::new();
        for (src, dst, _) in &nets {
            let path = astar(dev, *src, *dst, &hist, &pres, pres_fac)?;
            for tile in &path {
                *pres.entry(*tile).or_insert(0) += 1;
            }
            paths.push(path);
        }
        overused = pres.values().filter(|c| **c > 1).count() as u32;
        last_paths = paths;
        if overused == 0 {
            break;
        }
        for (tile, c) in &pres {
            if *c > 1 {
                *hist.entry(*tile).or_insert(0) += 1;
            }
        }
    }
    for (i, (src, dst, ble)) in nets.iter().enumerate() {
        let hops = last_paths[i].len().saturating_sub(1) as u32 + opts.extra_hops;
        let net = placed
            .packed
            .iobs
            .get(i)
            .map(|io| io.from_net.clone())
            .unwrap_or_default();
        iob_src.push(IobRoute {
            iob: *dst,
            clb: *src,
            ble: *ble,
            hops,
            delay_ps: hops as i64 * HOP_DELAY_PS,
            path: last_paths[i].clone(),
            net,
        });
    }
    Ok(Routed {
        placed: placed.clone(),
        iob_src,
        imux,
        pathfinder_iters: iters,
        overused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::{place, place_with, PlaceOpts};

    #[test]
    fn routes_south_to_iob() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let r = route(&pl, &dev).unwrap();
        assert_eq!(r.iob_src.len(), 1);
        assert_eq!(r.iob_src[0].clb.1, pl.lutff_sites[0].0.y);
        assert!(r.pathfinder_iters >= 1);
        assert_eq!(r.overused, 0);
        assert!(r.iob_src[0].hops >= 1);
        assert_eq!(r.iob_src[0].path.first().copied(), Some(r.iob_src[0].clb));
        assert_eq!(r.iob_src[0].path.last().copied(), Some(r.iob_src[0].iob));
        assert_eq!(
            r.iob_src[0].path.len().saturating_sub(1) as u32,
            r.iob_src[0].hops,
            "PathFinder hops are tile steps, not a canned count"
        );
        assert!(!r.iob_src[0].net.is_empty(), "IOB route names the packed net");
    }

    #[test]
    fn routes_wl_mid_column() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        assert!(r.iob_src[0].clb.1 > r.iob_src[0].iob.1);
    }

    #[test]
    fn delay_in_cost_makes_td_faster_than_wl() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0 }).unwrap();
        let td = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let r_wl = route(&wl, &dev).unwrap();
        let r_td = route(&td, &dev).unwrap();
        assert!(
            r_td.iob_src[0].delay_ps < r_wl.iob_src[0].delay_ps,
            "PathFinder delay must track placement (TD {}ps WL {}ps hops {} vs {})",
            r_td.iob_src[0].delay_ps,
            r_wl.iob_src[0].delay_ps,
            r_td.iob_src[0].hops,
            r_wl.iob_src[0].hops
        );
        assert_eq!(r_td.iob_src[0].delay_ps, r_td.iob_src[0].hops as i64 * HOP_DELAY_PS);
        assert!(r_td.iob_src[0].hops >= 1);
    }

    #[test]
    fn extra_hops_add_directed_delay() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let base = route(&pl, &dev).unwrap();
        let detour = route_with(
            &pl,
            &dev,
            RouteOpts {
                max_iters: 8,
                extra_hops: 3,
            },
        )
        .unwrap();
        assert_eq!(
            detour.iob_src[0].hops,
            base.iob_src[0].hops + 3,
            "Lab 3 FIXED_ROUTE extra hops"
        );
        assert_eq!(
            detour.iob_src[0].delay_ps,
            base.iob_src[0].delay_ps + 3 * HOP_DELAY_PS
        );
        assert_eq!(
            detour.iob_src[0].path, base.iob_src[0].path,
            "extra hops are directed delay, not a restyled PathFinder path"
        );
    }

    #[test]
    fn counter_iob_from_msb_not_bit0() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        assert_eq!(r.iob_src[0].ble, 3, "LED is cnt[3] = BLE3, not BLE0");
        assert!(r.imux.len() >= 1 + 2 + 3 + 4);
        let msb = r.imux.iter().find(|m| m.mux == 3 * 8 + 3).unwrap();
        assert_eq!(msb.sel, 16 + 3);
    }
}
