//! PathFinder negotiated routing on the Helion tile RR graph.
//! Intra-CLB IMUX (sel 16+k = local BLE k Q); IOB via A* on the tile grid.
//!
//! `route_with_guide` deprioritizes excepted IOB nets inside PathFinder search
//! cost (false_path / multicycle dogleg). helion-cli and helion-proj still call
//! unguided `route_with` until a later crate lock wires `TimingGuide` through.

use helion_device::{BelId, Device, NetId, Site, SiteKind};
use helion_place::Placed;
use rayon::prelude::*;

pub use helion_place::TimingGuide;
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
    /// LUT-pin drivers outside bring-up IMUX reach (axis±4 / diag±2 / knight; Ibex-scale).
    pub imux_skip: u32,
    /// Final per-tile net usage over the router TileGrid, row-major:
    /// `congestion[dy * congestion_cols + dx]` counts nets whose best path
    /// traverses tile (`congestion_x0 + dx`, `congestion_y0 + dy`).
    /// Only intermediate tiles count (a net's own driver/sink terminals do
    /// not congest themselves); empty when nothing routed. Feeds the
    /// Physical View 2.0 congestion heat map.
    pub congestion: Vec<u32>,
    /// Origin and dims of the TileGrid the congestion vector is indexed over.
    pub congestion_x0: u32,
    pub congestion_y0: u32,
    pub congestion_cols: u32,
    pub congestion_rows: u32,
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
    /// True when the driving cluster has an FF (`BLE.FF` / `BLE.Q`); false for comb LUT (`BLE.LUT` / `BLE.O`).
    pub from_ff: bool,
}

impl IobRoute {
    pub fn clb_site(&self) -> Site {
        Site {
            x: self.clb.0,
            y: self.clb.1,
            kind: SiteKind::Clb,
        }
    }

    pub fn iob_site(&self) -> Site {
        Site {
            x: self.iob.0,
            y: self.iob.1,
            kind: SiteKind::Iob,
        }
    }

    /// Driving BEL at the source CLB (`…/BLEk.FF` or `…/BLEk.LUT` for comb).
    pub fn clb_bel(&self) -> BelId {
        let bel = if self.from_ff {
            format!("BLE{}.FF", self.ble)
        } else {
            format!("BLE{}.LUT", self.ble)
        };
        BelId::new(self.clb_site(), bel)
    }

    /// Architecture stub at the source BLE (`BLEk.Q` or `BLEk.O` for comb).
    pub fn clb_net_id(&self) -> NetId {
        let net = if self.from_ff {
            format!("BLE{}.Q", self.ble)
        } else {
            format!("BLE{}.O", self.ble)
        };
        NetId::new(self.clb_site(), net)
    }
}

/// First HAD site at tile (x, y) present in the loaded part.
pub fn site_at_xy(dev: &Device, x: u32, y: u32) -> Option<Site> {
    const KINDS: [SiteKind; 5] = [
        SiteKind::Clb,
        SiteKind::Iob,
        SiteKind::Clk,
        SiteKind::Dsp,
        SiteKind::Bram,
    ];
    KINDS
        .into_iter()
        .map(|kind| Site { x, y, kind })
        .find(|&s| dev.contains_site(s))
}

impl Routed {
    /// HAD sites along PathFinder IOB routes (source CLB, hops, sink IOB).
    pub fn path_sites(&self, dev: &Device) -> Vec<Site> {
        let mut v = Vec::new();
        let mut seen = HashSet::new();
        for r in &self.iob_src {
            for &(x, y) in &r.path {
                if let Some(s) = site_at_xy(dev, x, y) {
                    if seen.insert(s) {
                        v.push(s);
                    }
                }
            }
            let clb = r.clb_site();
            let iob = r.iob_site();
            if dev.contains_site(clb) && seen.insert(clb) {
                v.push(clb);
            }
            if dev.contains_site(iob) && seen.insert(iob) {
                v.push(iob);
            }
        }
        v
    }

    pub fn path_site_ids(&self, dev: &Device) -> Vec<String> {
        let mut ids: Vec<String> = self.path_sites(dev).iter().map(|s| s.id()).collect();
        ids.sort();
        ids.dedup();
        ids
    }
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

/// Dense tile grid covering CLB interior + bottom IOB row (routing RR).
#[derive(Clone, Copy, Debug)]
struct TileGrid {
    x0: u32,
    y0: u32,
    cols: u32,
    rows: u32,
}

impl TileGrid {
    fn from_device(dev: &Device) -> Self {
        Self {
            x0: dev.clb_x0,
            y0: dev.clb_y0.saturating_sub(1),
            cols: dev.interior_cols,
            rows: dev.interior_rows.saturating_add(1),
        }
    }

    fn len(&self) -> usize {
        (self.cols as usize).saturating_mul(self.rows as usize)
    }

    #[inline]
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x < self.x0 || y < self.y0 {
            return None;
        }
        let dx = x - self.x0;
        let dy = y - self.y0;
        if dx >= self.cols || dy >= self.rows {
            return None;
        }
        Some((dy * self.cols + dx) as usize)
    }
}

/// Reused A* buffers across nets × PathFinder iters (no per-call HashMap/HashSet).
struct AstarScratch {
    open: BinaryHeap<Item>,
    g: Vec<i64>,
    came: Vec<(u32, u32)>,
    g_stamp: Vec<u32>,
    closed: Vec<u32>,
    epoch: u32,
}

impl AstarScratch {
    fn new(n: usize) -> Self {
        Self {
            open: BinaryHeap::new(),
            g: vec![0; n],
            came: vec![(0, 0); n],
            g_stamp: vec![0; n],
            closed: vec![0; n],
            epoch: 0,
        }
    }

    fn begin(&mut self) {
        self.open.clear();
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.g_stamp.fill(0);
            self.closed.fill(0);
            self.epoch = 1;
        }
    }

    #[inline]
    fn g_at(&self, i: usize) -> i64 {
        if self.g_stamp[i] == self.epoch {
            self.g[i]
        } else {
            i64::MAX
        }
    }

    #[inline]
    fn set_g_came(&mut self, i: usize, g: i64, parent: (u32, u32)) {
        self.g_stamp[i] = self.epoch;
        self.g[i] = g;
        self.came[i] = parent;
    }
}

fn neighbors4(dev: &Device, x: u32, y: u32, out: &mut [(u32, u32); 4]) -> usize {
    let mut n = 0;
    for (dx, dy) in [(0i32, 1), (0, -1), (1, 0), (-1, 0)] {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx < 0 || ny < 0 {
            continue;
        }
        let (nx, ny) = (nx as u32, ny as u32);
        if on_grid(dev, nx, ny) {
            out[n] = (nx, ny);
            n += 1;
        }
    }
    n
}

fn manhattan(a: (u32, u32), b: (u32, u32)) -> u32 {
    a.0.abs_diff(b.0) + a.1.abs_diff(b.1)
}

fn prefer_detour_x(dev: &Device, src: (u32, u32), detour_cols: u32) -> u32 {
    if detour_cols == 0 {
        return src.0;
    }
    let right = src.0.saturating_add(detour_cols);
    if on_grid(dev, right, src.1) {
        right
    } else {
        src.0.saturating_sub(detour_cols)
    }
}

fn astar(
    dev: &Device,
    grid: &TileGrid,
    scratch: &mut AstarScratch,
    src: (u32, u32),
    dst: (u32, u32),
    hist: &[u32],
    pres: &[u32],
    pres_fac: i64,
    detour_cols: u32,
) -> Result<Vec<(u32, u32)>, String> {
    let prefer_x = prefer_detour_x(dev, src, detour_cols);
    scratch.begin();
    let src_i = grid
        .index(src.0, src.1)
        .ok_or_else(|| format!("PathFinder: src off grid {src:?}"))?;
    scratch.set_g_came(src_i, 0, src);
    scratch.open.push(Item {
        cost: (manhattan(src, dst) as i64) * HOP_DELAY_PS,
        x: src.0,
        y: src.1,
    });
    let mut neigh = [(0u32, 0u32); 4];
    while let Some(Item { x, y, .. }) = scratch.open.pop() {
        let cur_i = match grid.index(x, y) {
            Some(i) => i,
            None => continue,
        };
        if scratch.closed[cur_i] == scratch.epoch {
            continue;
        }
        scratch.closed[cur_i] = scratch.epoch;
        if (x, y) == dst {
            let mut path = vec![(x, y)];
            let mut cur = (x, y);
            while cur != src {
                let ci = grid.index(cur.0, cur.1).ok_or("astar broke")?;
                if scratch.g_stamp[ci] != scratch.epoch {
                    return Err("astar broke".into());
                }
                cur = scratch.came[ci];
                path.push(cur);
            }
            path.reverse();
            return Ok(path);
        }
        let gc = scratch.g_at(cur_i);
        let nlen = neighbors4(dev, x, y, &mut neigh);
        for &(nx, ny) in neigh[..nlen].iter() {
            let Some(ni) = grid.index(nx, ny) else {
                continue;
            };
            let h = hist.get(ni).copied().unwrap_or(0) as i64;
            let p = pres.get(ni).copied().unwrap_or(0) as i64;
            // delay-driven: hop delay dominates, congestion still negotiates.
            // Excepted IOB nets: penalize the src-column spine so A* doglegs
            // (routed path/hops change; empty guide keeps gold shortest).
            let mut step = HOP_DELAY_PS + h + p * pres_fac;
            if detour_cols > 0 && (nx, ny) != dst {
                if nx.abs_diff(src.0) < detour_cols {
                    step += HOP_DELAY_PS * 8;
                }
                step += nx.abs_diff(prefer_x) as i64;
            }
            let ng = gc + step;
            if ng < scratch.g_at(ni) {
                scratch.set_g_came(ni, ng, (x, y));
                let f = ng + (manhattan((nx, ny), dst) as i64) * HOP_DELAY_PS;
                scratch.open.push(Item {
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
        // driver is south of sink (±1)
        return Ok(dble);
    }
    if from.x == to.x && to.y + 1 == from.y {
        // driver is north of sink (±1)
        return Ok(8 + dble);
    }
    if from.x == to.x && from.y + 2 == to.y {
        // driver is south of sink (±2) — HAD IMUX bit5 bank
        return Ok(32 + dble);
    }
    if from.x == to.x && to.y + 2 == from.y {
        // driver is north of sink (±2)
        return Ok(40 + dble);
    }
    // Real E-W ±1 (same row): uwilton SB left/right track + IMUX neighbor Q.
    // Not a fake encoding — fabric decode samples adjacent-column Q.
    if from.y == to.y && from.x + 1 == to.x {
        // driver is west of sink
        return Ok(48 + dble);
    }
    if from.y == to.y && to.x + 1 == from.x {
        // driver is east of sink
        return Ok(56 + dble);
    }
    // Real E-W ±2 (same row): bit6 bank; fabric samples Q two columns away.
    if from.y == to.y && from.x + 2 == to.x {
        return Ok(64 + dble);
    }
    if from.y == to.y && to.x + 2 == from.x {
        return Ok(72 + dble);
    }
    // Real diagonal (±1,±1): remaining 7-bit sel 80-111; fabric samples
    // neighbor-column × neighbor-row Q (same honesty class as axis ±1/±2).
    if from.x + 1 == to.x && from.y + 1 == to.y {
        // driver SW of sink
        return Ok(80 + dble);
    }
    if to.x + 1 == from.x && from.y + 1 == to.y {
        // driver SE of sink
        return Ok(88 + dble);
    }
    if from.x + 1 == to.x && to.y + 1 == from.y {
        // driver NW of sink
        return Ok(96 + dble);
    }
    if to.x + 1 == from.x && to.y + 1 == from.y {
        // driver NE of sink
        return Ok(104 + dble);
    }
    // Real knight (±2,±1)/(±1,±2): bit7 bank sel 128-191; fabric samples Q
    // at knight-neighbor sites (same honesty class as axis/diag neighbor Q).
    if from.x + 2 == to.x && from.y + 1 == to.y {
        // driver W2S1 of sink
        return Ok(128 + dble);
    }
    if from.x + 2 == to.x && to.y + 1 == from.y {
        // driver W2N1 of sink
        return Ok(136 + dble);
    }
    if to.x + 2 == from.x && from.y + 1 == to.y {
        // driver E2S1 of sink
        return Ok(144 + dble);
    }
    if to.x + 2 == from.x && to.y + 1 == from.y {
        // driver E2N1 of sink
        return Ok(152 + dble);
    }
    if from.x + 1 == to.x && from.y + 2 == to.y {
        // driver W1S2 of sink
        return Ok(160 + dble);
    }
    if from.x + 1 == to.x && to.y + 2 == from.y {
        // driver W1N2 of sink
        return Ok(168 + dble);
    }
    if to.x + 1 == from.x && from.y + 2 == to.y {
        // driver E1S2 of sink
        return Ok(176 + dble);
    }
    if to.x + 1 == from.x && to.y + 2 == from.y {
        // driver E1N2 of sink
        return Ok(184 + dble);
    }
    // Reserved 112-127: real N-S ±3 (same column). Fabric samples Q three rows away.
    if from.x == to.x && from.y + 3 == to.y {
        return Ok(112 + dble);
    }
    if from.x == to.x && to.y + 3 == from.y {
        return Ok(120 + dble);
    }
    // 192-207: real E-W ±3 (same row).
    if from.y == to.y && from.x + 3 == to.x {
        return Ok(192 + dble);
    }
    if from.y == to.y && to.x + 3 == from.x {
        return Ok(200 + dble);
    }
    // 208-239: real diagonal ±2 (dx=2,dy=2).
    if from.x + 2 == to.x && from.y + 2 == to.y {
        return Ok(208 + dble); // SW2
    }
    if to.x + 2 == from.x && from.y + 2 == to.y {
        return Ok(216 + dble); // SE2
    }
    if from.x + 2 == to.x && to.y + 2 == from.y {
        return Ok(224 + dble); // NW2
    }
    if to.x + 2 == from.x && to.y + 2 == from.y {
        return Ok(232 + dble); // NE2
    }
    // 240-255: real N-S ±4 (same column).
    if from.x == to.x && from.y + 4 == to.y {
        return Ok(240 + dble);
    }
    if from.x == to.x && to.y + 4 == from.y {
        return Ok(248 + dble);
    }
    Err(format!(
        "IMUX: no local/±2/±3/±4/diag/knight encoding from {} BLE{dble} to {}",
        from.id(),
        to.id()
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
    route_with_guide(placed, dev, opts, &TimingGuide::default())
}

/// Route with XDC IOB-arc exceptions. Empty `guide` matches `route_with`.
/// False-path / multicycle IOB nets are deprioritized inside PathFinder search
/// cost so the routed path/hops change (not a post-route hop pad).
/// helion-cli / helion-proj still call unguided `route_with` until a later lock.
pub fn route_with_guide(
    placed: &Placed,
    dev: &Device,
    opts: RouteOpts,
    guide: &TimingGuide,
) -> Result<Routed, String> {
    // Ibex-scale: lutff_of linear scan per pin is O(n^2). Index FF → site once.
    let mut ff_site: HashMap<&str, (Site, u8)> = HashMap::with_capacity(placed.lutff_sites.len());
    for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
        if !lutff.ff_cell.is_empty() {
            ff_site.insert(lutff.ff_cell.as_str(), placed.lutff_sites[i]);
        }
    }
    let route_cluster = |(i, lutff): (usize, &helion_pack::PackedLutFf)| -> (Vec<ImuxRoute>, u32) {
        let (site, ble) = placed.lutff_sites[i];
        let mut local_imux = Vec::new();
        let mut local_skip = 0u32;
        if lutff.lut_pins.is_empty() {
            // Registered empty-pin cluster: keep local FF Q loop (gold blinky/counter).
            // Comb LUT: primary/port inputs — no IMUX programming needed.
            if !lutff.ff_cell.is_empty() {
                local_imux.push(ImuxRoute {
                    x: site.x,
                    y: site.y,
                    mux: ble as u32 * 8,
                    sel: 16 + ble,
                });
            }
            return (local_imux, local_skip);
        }
        local_imux.reserve(lutff.lut_pins.len());
        for (pin, driver) in &lutff.lut_pins {
            let Some((dsite, dble)) = ff_site.get(driver.as_str()).copied() else {
                local_skip += 1;
                continue;
            };
            match imux_sel(dsite, site, dble) {
                Ok(sel) => {
                    local_imux.push(ImuxRoute {
                        x: site.x,
                        y: site.y,
                        mux: ble as u32 * 8 + *pin as u32,
                        sel,
                    });
                }
                Err(_) => {
                    local_skip += 1;
                }
            }
        }
        (local_imux, local_skip)
    };
    let mut imux = Vec::new();
    let mut imux_skip = 0u32;
    if placed.packed.lutffs.len() >= 256 {
        let parts: Vec<(Vec<ImuxRoute>, u32)> = placed
            .packed
            .lutffs
            .par_iter()
            .enumerate()
            .map(route_cluster)
            .collect();
        let total_imux: usize = parts.iter().map(|(v, _)| v.len()).sum();
        imux.reserve(total_imux);
        for (mut v, sk) in parts {
            imux.append(&mut v);
            imux_skip += sk;
        }
    } else {
        for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
            let (mut v, sk) = route_cluster((i, lutff));
            imux.append(&mut v);
            imux_skip += sk;
        }
    }

    // IOB→LUTFF by q_net once (was O(iobs×lutffs) .position per IOB).
    let mut q_net_lutff: HashMap<&str, usize> = HashMap::with_capacity(placed.packed.lutffs.len());
    for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
        q_net_lutff.entry(lutff.q_net.as_str()).or_insert(i);
    }
    // (src, dst, ble, packed_iob_idx) — idx must survive y-skip filtering.
    let mut nets: Vec<((u32, u32), (u32, u32), u8, usize)> = Vec::new();
    if !placed.packed.lutffs.is_empty() {
        for (ii, iob_site) in placed.iob_sites.iter().enumerate() {
            let packed_iob = placed.packed.iobs.get(ii);
            let idx = packed_iob
                .and_then(|io| q_net_lutff.get(io.from_net.as_str()).copied())
                .unwrap_or(0);
            let Some(&(clb, ble)) = placed.lutff_sites.get(idx) else {
                continue;
            };
            if clb.y <= iob_site.y {
                continue;
            }
            nets.push(((clb.x, clb.y), (iob_site.x, iob_site.y), ble, ii));
        }
    }

    let grid = TileGrid::from_device(dev);
    let mut hist = vec![0u32; grid.len()];
    let mut iob_src = Vec::new();
    let mut iters = 0u32;
    let (cong_x0, cong_y0, cong_cols, cong_rows) =
        (grid.x0, grid.y0, grid.cols, grid.rows);
    if nets.is_empty() {
        return Ok(Routed {
            placed: placed.clone(),
            iob_src,
            imux,
            pathfinder_iters: 0,
            overused: 0,
            imux_skip,
            congestion: vec![0u32; grid.len()],
            congestion_x0: cong_x0,
            congestion_y0: cong_y0,
            congestion_cols: cong_cols,
            congestion_rows: cong_rows,
        });
    }
    let max_iters = opts.max_iters.max(1);
    let detour_cols = guide.iob_detour_cols();
    let mut best_overused = u32::MAX;
    let mut best_hops = u32::MAX;
    let mut best_paths: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nets.len()];
    let mut scratch = AstarScratch::new(grid.len());
    let mut pres = vec![0u32; grid.len()];
    // A net's own driver CLB / IOB sink are pin terminals (distinct BLEs may
    // share a CLB site). Count only that net's *intermediate* RR tiles toward
    // present/hist/overused so PathFinder negotiates real channel conflicts.
    // (Per-net skip — not a global endpoint mask — so one net's src still
    // congests when another net traverses it as an intermediate.)
    for iter in 0..max_iters {
        iters = iter + 1;
        pres.fill(0);
        let pres_fac = 1i64 + iter as i64;
        let mut paths = Vec::with_capacity(nets.len());
        for (src, dst, _, _) in &nets {
            let path = astar(
                dev,
                &grid,
                &mut scratch,
                *src,
                *dst,
                &hist,
                &pres,
                pres_fac,
                detour_cols,
            )?;
            for &(tx, ty) in &path {
                if (tx, ty) == *src || (tx, ty) == *dst {
                    continue;
                }
                if let Some(ti) = grid.index(tx, ty) {
                    pres[ti] = pres[ti].saturating_add(1);
                }
            }
            paths.push(path);
        }
        let ou = pres.iter().filter(|c| **c > 1).count() as u32;
        let hops_sum: u32 = paths.iter().map(|p| p.len().saturating_sub(1) as u32).sum();
        if ou < best_overused || (ou == best_overused && hops_sum < best_hops) {
            best_overused = ou;
            best_hops = hops_sum;
            best_paths = paths.clone();
        }
        if ou == 0 {
            break;
        }
        for (ti, &c) in pres.iter().enumerate() {
            if c > 1 {
                hist[ti] = hist[ti].saturating_add(1);
            }
        }
    }
    for (i, (src, dst, ble, packed_ii)) in nets.iter().enumerate() {
        let hops = best_paths[i].len().saturating_sub(1) as u32 + opts.extra_hops;
        let net = placed
            .packed
            .iobs
            .get(*packed_ii)
            .map(|io| io.from_net.clone())
            .unwrap_or_default();
        let from_ff = q_net_lutff
            .get(net.as_str())
            .and_then(|&idx| placed.packed.lutffs.get(idx))
            .map(|l| !l.ff_cell.is_empty())
            .unwrap_or(true);
        iob_src.push(IobRoute {
            iob: *dst,
            clb: *src,
            ble: *ble,
            hops,
            delay_ps: hops as i64 * HOP_DELAY_PS,
            path: best_paths[i].clone(),
            net,
            from_ff,
        });
    }
    if imux_skip > 0 {
        eprintln!(
            "hang_diag imux_skip={} imux_ok={} (non-local FF→LUT; bring-up IMUX axis±4/diag±2/knight/same-CLB)",
            imux_skip,
            imux.len()
        );
    }
    // Congestion heat map data: count intermediate tiles of each net's best
    // path (same terminal-skip rule as the per-iteration `pres` count).
    let mut congestion = vec![0u32; grid.len()];
    for (i, path) in best_paths.iter().enumerate() {
        let (src, dst, _, _) = nets[i];
        for &(tx, ty) in path {
            if (tx, ty) == src || (tx, ty) == dst {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                congestion[ti] = congestion[ti].saturating_add(1);
            }
        }
    }
    Ok(Routed {
        placed: placed.clone(),
        iob_src,
        imux,
        pathfinder_iters: iters,
        overused: best_overused,
        imux_skip,
        congestion,
        congestion_x0: cong_x0,
        congestion_y0: cong_y0,
        congestion_cols: cong_cols,
        congestion_rows: cong_rows,
    })
}

/// Count the number of 90-degree turns (vias / switchbox direction changes) along a tile path.
pub fn count_path_vias(path: &[(u32, u32)]) -> u32 {
    if path.len() < 3 {
        return 0;
    }
    let mut vias = 0u32;
    for w in path.windows(3) {
        let dx1 = w[1].0 as i32 - w[0].0 as i32;
        let dy1 = w[1].1 as i32 - w[0].1 as i32;
        let dx2 = w[2].0 as i32 - w[1].0 as i32;
        let dy2 = w[2].1 as i32 - w[1].1 as i32;
        if (dx1, dy1) != (dx2, dy2) {
            vias += 1;
        }
    }
    vias
}

fn recompute_congestion(routed: &mut Routed) {
    let cols = routed.congestion_cols;
    let rows = routed.congestion_rows;
    let len = (cols as usize).saturating_mul(rows as usize);
    routed.congestion = vec![0u32; len];
    if cols == 0 || rows == 0 {
        routed.overused = 0;
        return;
    }
    let grid = TileGrid {
        x0: routed.congestion_x0,
        y0: routed.congestion_y0,
        cols,
        rows,
    };
    for r in &routed.iob_src {
        for &(tx, ty) in &r.path {
            if (tx, ty) == r.clb || (tx, ty) == r.iob {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                routed.congestion[ti] = routed.congestion[ti].saturating_add(1);
            }
        }
    }
    routed.overused = routed.congestion.iter().filter(|&&c| c > 1).count() as u32;
}

/// Vivado UG904 post-route `phys_opt_design -hold_fix`: insert legal uncongested tile
/// dogleg detours on short paths whose delay is below `min_delay_ps` so hold slack closes
/// without creating routing congestion conflicts.
/// Returns the updated [`Routed`] and the number of nets that received hold-fix detours.
pub fn phys_opt_route_hold_fix(
    routed: &Routed,
    dev: &Device,
    min_delay_ps: i64,
) -> Result<(Routed, usize), String> {
    let mut out = routed.clone();
    if min_delay_ps <= 0 || out.iob_src.is_empty() {
        return Ok((out, 0));
    }
    let grid = TileGrid::from_device(dev);
    let mut fixed = 0usize;
    for ri in 0..out.iob_src.len() {
        if out.iob_src[ri].delay_ps >= min_delay_ps {
            continue;
        }
        let mut changed = false;
        // Insert 2-hop orthogonal dogleg detours along adjacent uncongested tiles until
        // delay_ps >= min_delay_ps or no further uncongested dogleg exists.
        while out.iob_src[ri].delay_ps < min_delay_ps {
            let path = &out.iob_src[ri].path;
            let mut inserted = None;
            for idx in 0..path.len().saturating_sub(1) {
                let a = path[idx];
                let b = path[idx + 1];
                let dx = b.0 as i32 - a.0 as i32;
                let dy = b.1 as i32 - a.1 as i32;
                // Orthogonal side offsets perpendicular to step (a -> b).
                let perp: [(i32, i32); 2] = if dx == 0 && dy.abs() == 1 {
                    [(1, 0), (-1, 0)]
                } else if dy == 0 && dx.abs() == 1 {
                    [(0, 1), (0, -1)]
                } else {
                    continue;
                };
                for (ox, oy) in perp {
                    let ax = a.0 as i32 + ox;
                    let ay = a.1 as i32 + oy;
                    let bx = b.0 as i32 + ox;
                    let by = b.1 as i32 + oy;
                    if ax < 0 || ay < 0 || bx < 0 || by < 0 {
                        continue;
                    }
                    let pa = (ax as u32, ay as u32);
                    let pb = (bx as u32, by as u32);
                    if !on_grid(dev, pa.0, pa.1) || !on_grid(dev, pb.0, pb.1) {
                        continue;
                    }
                    if path.contains(&pa) || path.contains(&pb) {
                        continue;
                    }
                    let Some(tia) = grid.index(pa.0, pa.1) else {
                        continue;
                    };
                    let Some(tib) = grid.index(pb.0, pb.1) else {
                        continue;
                    };
                    let free_a = out.congestion.get(tia).copied().unwrap_or(0) == 0;
                    let free_b = out.congestion.get(tib).copied().unwrap_or(0) == 0;
                    if free_a && free_b {
                        inserted = Some((idx + 1, pa, pb, tia, tib));
                        break;
                    }
                }
                if inserted.is_some() {
                    break;
                }
            }
            if let Some((ins_at, pa, pb, tia, tib)) = inserted {
                let extra_pad = out.iob_src[ri]
                    .hops
                    .saturating_sub(out.iob_src[ri].path.len().saturating_sub(1) as u32);
                out.iob_src[ri].path.insert(ins_at, pb);
                out.iob_src[ri].path.insert(ins_at, pa);
                if let Some(c) = out.congestion.get_mut(tia) {
                    *c = c.saturating_add(1);
                }
                if let Some(c) = out.congestion.get_mut(tib) {
                    *c = c.saturating_add(1);
                }
                let new_hops =
                    out.iob_src[ri].path.len().saturating_sub(1) as u32 + extra_pad;
                out.iob_src[ri].hops = new_hops;
                out.iob_src[ri].delay_ps = new_hops as i64 * HOP_DELAY_PS;
                changed = true;
            } else {
                break;
            }
        }
        if changed {
            fixed += 1;
        }
    }
    if fixed > 0 {
        recompute_congestion(&mut out);
    }
    Ok((out, fixed))
}

/// Summary metrics from post-route physical optimization ([`phys_opt_route_with_stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysOptRouteStats {
    pub shortened_nets: usize,
    pub hops_before: u32,
    pub hops_after: u32,
    pub vias_before: u32,
    pub vias_after: u32,
}

/// Vivado UG904 post-route `phys_opt_design`: rip-up and shortcut non-critical detours
/// and excess switchbox turns (vias) without increasing tile congestion.
pub fn phys_opt_route(routed: &Routed, dev: &Device) -> Result<Routed, String> {
    Ok(phys_opt_route_with_stats(routed, dev)?.0)
}

/// Run post-route wirelength/via minimization and return both [`Routed`] and [`PhysOptRouteStats`].
pub fn phys_opt_route_with_stats(
    routed: &Routed,
    dev: &Device,
) -> Result<(Routed, PhysOptRouteStats), String> {
    let mut out = routed.clone();
    let hops_before: u32 = out.iob_src.iter().map(|r| r.hops).sum();
    let vias_before: u32 = out.iob_src.iter().map(|r| count_path_vias(&r.path)).sum();
    if out.iob_src.is_empty() {
        return Ok((
            out,
            PhysOptRouteStats {
                shortened_nets: 0,
                hops_before,
                hops_after: hops_before,
                vias_before,
                vias_after: vias_before,
            },
        ));
    }
    let grid = TileGrid::from_device(dev);
    let hist = vec![0u32; grid.len()];
    let mut scratch = AstarScratch::new(grid.len());
    let mut shortened_nets = 0usize;

    for ri in 0..out.iob_src.len() {
        let src = out.iob_src[ri].clb;
        let dst = out.iob_src[ri].iob;
        let old_path = &out.iob_src[ri].path;
        let old_tile_hops = old_path.len().saturating_sub(1) as u32;
        let extra_pad = out.iob_src[ri].hops.saturating_sub(old_tile_hops);
        let old_vias = count_path_vias(old_path);

        // Temporarily rip up this net's intermediate tiles from congestion.
        let mut pres = out.congestion.clone();
        for &(tx, ty) in old_path {
            if (tx, ty) == src || (tx, ty) == dst {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                pres[ti] = pres[ti].saturating_sub(1);
            }
        }

        // Try L-shaped 1-via paths first (horizontal-then-vertical and vertical-then-horizontal)
        // if they are 100% uncongested and on-grid, else fall back to zero-detour A*.
        let build_l_path = |hv: bool| -> Option<Vec<(u32, u32)>> {
            let mut p = Vec::new();
            let (mut x, mut y) = src;
            p.push((x, y));
            if hv {
                while x != dst.0 {
                    x = if x < dst.0 { x + 1 } else { x - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
                while y != dst.1 {
                    y = if y < dst.1 { y + 1 } else { y - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
            } else {
                while y != dst.1 {
                    y = if y < dst.1 { y + 1 } else { y - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
                while x != dst.0 {
                    x = if x < dst.0 { x + 1 } else { x - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
            }
            for &(tx, ty) in &p {
                if (tx, ty) == src || (tx, ty) == dst {
                    continue;
                }
                let ti = grid.index(tx, ty)?;
                if pres.get(ti).copied().unwrap_or(0) > 0 {
                    return None;
                }
            }
            Some(p)
        };

        let cand = build_l_path(false)
            .or_else(|| build_l_path(true))
            .or_else(|| {
                astar(dev, &grid, &mut scratch, src, dst, &hist, &pres, 16, 0)
                    .ok()
                    .filter(|p| {
                        p.iter().all(|&(tx, ty)| {
                            (tx, ty) == src
                                || (tx, ty) == dst
                                || grid
                                    .index(tx, ty)
                                    .and_then(|ti| pres.get(ti))
                                    .copied()
                                    .unwrap_or(0)
                                    == 0
                        })
                    })
            });

        if let Some(new_path) = cand {
            let new_tile_hops = new_path.len().saturating_sub(1) as u32;
            let new_vias = count_path_vias(&new_path);
            if (new_tile_hops, new_vias) < (old_tile_hops, old_vias) {
                let new_hops = new_tile_hops + extra_pad;
                out.iob_src[ri].path = new_path;
                out.iob_src[ri].hops = new_hops;
                out.iob_src[ri].delay_ps = new_hops as i64 * HOP_DELAY_PS;
                shortened_nets += 1;
                recompute_congestion(&mut out);
            }
        }
    }

    let hops_after: u32 = out.iob_src.iter().map(|r| r.hops).sum();
    let vias_after: u32 = out.iob_src.iter().map(|r| count_path_vias(&r.path)).sum();
    Ok((
        out,
        PhysOptRouteStats {
            shortened_nets,
            hops_before,
            hops_after,
            vias_before,
            vias_after,
        },
    ))
}

/// Vivado UG904 timing-driven PathFinder router configuration ([`route_timing_driven`]).
#[derive(Clone, Debug)]
pub struct TimingDrivenRouterConfig {
    /// Maximum negotiated congestion iterations.
    pub max_iters: u32,
    /// Criticality exponent $\alpha$ in $\text{crit}(e) = (1 - \text{slack}/D_{\max})^\alpha$.
    pub criticality_exp: f64,
    /// Upper bound on $\text{crit}(e)$ (`< 1.0`) so $(1 - \text{crit}(e))$ never vanishes on congested tiles.
    pub max_criticality: f64,
    /// Base tile resource cost $b(n)$ in units of [`HOP_DELAY_PS`].
    pub base_cost: f64,
    /// Initial present-congestion penalty factor.
    pub pres_fac_init: f64,
    /// Per-iteration multiplier for the present-congestion penalty factor.
    pub pres_fac_mult: f64,
    /// Historical congestion step weight.
    pub hist_fac: f64,
    /// Directed A* lookahead factor.
    pub astar_fac: f64,
    /// Optional minimum route delay (ps) for hold-time padding (`0` = disabled).
    pub min_hold_delay_ps: i64,
    /// Optional explicit per-net criticality overrides (`0.0..=1.0`).
    pub net_criticality: HashMap<String, f64>,
}

impl Default for TimingDrivenRouterConfig {
    fn default() -> Self {
        Self {
            max_iters: 16,
            criticality_exp: 2.0,
            max_criticality: 0.99,
            base_cost: 1.0,
            pres_fac_init: 1.0,
            pres_fac_mult: 1.5,
            hist_fac: 1.0,
            astar_fac: 1.2,
            min_hold_delay_ps: 0,
            net_criticality: HashMap::new(),
        }
    }
}

/// Compute slack criticality $\text{crit}(e) = \min(\text{max\_crit}, \max(0, 1 - \text{slack}/D_{\max})^\alpha)$.
pub fn net_criticality_from_slack(
    slack_ps: i64,
    d_max_ps: i64,
    criticality_exp: f64,
    max_criticality: f64,
) -> f64 {
    if d_max_ps <= 0 {
        return 0.0;
    }
    let ratio = (1.0 - (slack_ps as f64) / (d_max_ps as f64)).clamp(0.0, 1.0);
    ratio
        .powf(criticality_exp.max(0.0))
        .clamp(0.0, max_criticality.clamp(0.0, 0.999))
}

/// Evaluate the timing-driven PathFinder step cost on tile $n$ for edge $e$:
///
/// $$\text{cost}(n, e) = \text{crit}(e) \cdot d(n) + (1 - \text{crit}(e)) \cdot b(n) \cdot h(n) \cdot p(n)$$
#[inline]
pub fn tdr_node_cost(
    crit: f64,
    delay_ps: i64,
    base_cost: f64,
    hist_val: u32,
    pres_val: u32,
    pres_fac: f64,
    hist_fac: f64,
) -> i64 {
    let c = crit.clamp(0.0, 0.999);
    let d_n = delay_ps as f64;
    let b_n = (base_cost.max(0.1)) * (HOP_DELAY_PS as f64);
    let h_n = 1.0 + hist_fac.max(0.0) * (hist_val as f64);
    let p_n = 1.0 + pres_fac.max(0.0) * (pres_val as f64);
    let total = c * d_n + (1.0 - c) * b_n * h_n * p_n;
    (total.round() as i64).max(1)
}

/// Partition net indices into spatially disjoint batches whose expanded bounding boxes
/// (`margin` tiles on each side) do not overlap. Nets are considered in descending
/// criticality order so the most timing-critical nets run in the earliest batch.
pub fn schedule_disjoint_bbox_batches(
    bboxes: &[((u32, u32), (u32, u32), f64)],
    margin: u32,
) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..bboxes.len()).collect();
    order.sort_by(|&a, &b| {
        let ca = bboxes[a].2;
        let cb = bboxes[b].2;
        let hpa = manhattan(bboxes[a].0, bboxes[a].1);
        let hpb = manhattan(bboxes[b].0, bboxes[b].1);
        cb.partial_cmp(&ca)
            .unwrap_or(Ordering::Equal)
            .then_with(|| hpb.cmp(&hpa))
            .then_with(|| a.cmp(&b))
    });

    let rect_of = |src: (u32, u32), dst: (u32, u32)| -> (u32, u32, u32, u32) {
        let x0 = src.0.min(dst.0).saturating_sub(margin);
        let x1 = src.0.max(dst.0).saturating_add(margin);
        let y0 = src.1.min(dst.1).saturating_sub(margin);
        let y1 = src.1.max(dst.1).saturating_add(margin);
        (x0, y0, x1, y1)
    };
    let overlaps = |a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)| -> bool {
        a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
    };

    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut batch_rects: Vec<Vec<(u32, u32, u32, u32)>> = Vec::new();
    for ni in order {
        let r = rect_of(bboxes[ni].0, bboxes[ni].1);
        let mut placed_in = None;
        for (bi, rects) in batch_rects.iter().enumerate() {
            if rects.iter().all(|&other| !overlaps(r, other)) {
                placed_in = Some(bi);
                break;
            }
        }
        match placed_in {
            Some(bi) => {
                batches[bi].push(ni);
                batch_rects[bi].push(r);
            }
            None => {
                batches.push(vec![ni]);
                batch_rects.push(vec![r]);
            }
        }
    }
    batches
}

fn astar_tdr(
    dev: &Device,
    grid: &TileGrid,
    scratch: &mut AstarScratch,
    src: (u32, u32),
    dst: (u32, u32),
    crit: f64,
    cfg: &TimingDrivenRouterConfig,
    hist: &[u32],
    pres: &[u32],
    pres_fac: f64,
) -> Result<Vec<(u32, u32)>, String> {
    scratch.begin();
    let src_i = grid
        .index(src.0, src.1)
        .ok_or_else(|| format!("PathFinder TDR: src off grid {src:?}"))?;
    scratch.set_g_came(src_i, 0, src);
    let h0 = ((manhattan(src, dst) as f64) * (HOP_DELAY_PS as f64) * cfg.astar_fac.max(1.0)).round()
        as i64;
    scratch.open.push(Item {
        cost: h0,
        x: src.0,
        y: src.1,
    });
    let mut neigh = [(0u32, 0u32); 4];
    while let Some(Item { x, y, .. }) = scratch.open.pop() {
        let Some(cur_i) = grid.index(x, y) else {
            continue;
        };
        if scratch.closed[cur_i] == scratch.epoch {
            continue;
        }
        scratch.closed[cur_i] = scratch.epoch;
        if (x, y) == dst {
            let mut path = vec![(x, y)];
            let mut cur = (x, y);
            while cur != src {
                let ci = grid.index(cur.0, cur.1).ok_or("astar_tdr broke")?;
                if scratch.g_stamp[ci] != scratch.epoch {
                    return Err("astar_tdr broke".into());
                }
                cur = scratch.came[ci];
                path.push(cur);
            }
            path.reverse();
            return Ok(path);
        }
        let gc = scratch.g_at(cur_i);
        let nlen = neighbors4(dev, x, y, &mut neigh);
        for &(nx, ny) in neigh[..nlen].iter() {
            let Some(ni) = grid.index(nx, ny) else {
                continue;
            };
            let h_val = hist.get(ni).copied().unwrap_or(0);
            let p_val = if (nx, ny) == dst {
                0
            } else {
                pres.get(ni).copied().unwrap_or(0)
            };
            let step = tdr_node_cost(
                crit,
                HOP_DELAY_PS,
                cfg.base_cost,
                h_val,
                p_val,
                pres_fac,
                cfg.hist_fac,
            );
            let ng = gc + step;
            if ng < scratch.g_at(ni) {
                scratch.set_g_came(ni, ng, (x, y));
                let lookahead = ((manhattan((nx, ny), dst) as f64)
                    * (HOP_DELAY_PS as f64)
                    * (crit + (1.0 - crit) * cfg.base_cost)
                    * cfg.astar_fac.max(1.0))
                .round() as i64;
                scratch.open.push(Item {
                    cost: ng + lookahead,
                    x: nx,
                    y: ny,
                });
            }
        }
    }
    Err(format!("PathFinder TDR: no path {src:?} -> {dst:?}"))
}

/// Vivado UG904 timing-driven PathFinder router (`route_timing_driven`):
/// combines slack criticality cost $\text{crit}(e) \cdot d(n)$ with negotiated
/// congestion cost $(1 - \text{crit}(e)) \cdot b(n) \cdot h(n) \cdot p(n)$,
/// bounding-box disjoint parallel rip-up/reroute batches, and optional hold-time
/// minimum delay padding.
pub fn route_timing_driven(
    placed: &Placed,
    dev: &Device,
    cfg: &TimingDrivenRouterConfig,
) -> Result<Routed, String> {
    let mut ff_site: HashMap<&str, (Site, u8)> = HashMap::with_capacity(placed.lutff_sites.len());
    for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
        if !lutff.ff_cell.is_empty() {
            ff_site.insert(lutff.ff_cell.as_str(), placed.lutff_sites[i]);
        }
    }
    let mut imux = Vec::new();
    let mut imux_skip = 0u32;
    for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
        let (site, ble) = placed.lutff_sites[i];
        if lutff.lut_pins.is_empty() {
            if !lutff.ff_cell.is_empty() {
                imux.push(ImuxRoute {
                    x: site.x,
                    y: site.y,
                    mux: ble as u32 * 8,
                    sel: 16 + ble,
                });
            }
            continue;
        }
        for (pin, driver) in &lutff.lut_pins {
            let Some((dsite, dble)) = ff_site.get(driver.as_str()).copied() else {
                imux_skip += 1;
                continue;
            };
            match imux_sel(dsite, site, dble) {
                Ok(sel) => imux.push(ImuxRoute {
                    x: site.x,
                    y: site.y,
                    mux: ble as u32 * 8 + *pin as u32,
                    sel,
                }),
                Err(_) => imux_skip += 1,
            }
        }
    }

    let mut q_net_lutff: HashMap<&str, usize> = HashMap::with_capacity(placed.packed.lutffs.len());
    for (i, lutff) in placed.packed.lutffs.iter().enumerate() {
        q_net_lutff.entry(lutff.q_net.as_str()).or_insert(i);
    }
    let mut nets: Vec<((u32, u32), (u32, u32), u8, usize, String)> = Vec::new();
    if !placed.packed.lutffs.is_empty() {
        for (ii, iob_site) in placed.iob_sites.iter().enumerate() {
            let packed_iob = placed.packed.iobs.get(ii);
            let net_name = packed_iob
                .map(|io| io.from_net.clone())
                .unwrap_or_default();
            let idx = packed_iob
                .and_then(|io| q_net_lutff.get(io.from_net.as_str()).copied())
                .unwrap_or(0);
            let Some(&(clb, ble)) = placed.lutff_sites.get(idx) else {
                continue;
            };
            if clb.y <= iob_site.y {
                continue;
            }
            nets.push(((clb.x, clb.y), (iob_site.x, iob_site.y), ble, ii, net_name));
        }
    }

    let grid = TileGrid::from_device(dev);
    if nets.is_empty() {
        return Ok(Routed {
            placed: placed.clone(),
            iob_src: Vec::new(),
            imux,
            pathfinder_iters: 0,
            overused: 0,
            imux_skip,
            congestion: vec![0u32; grid.len()],
            congestion_x0: grid.x0,
            congestion_y0: grid.y0,
            congestion_cols: grid.cols,
            congestion_rows: grid.rows,
        });
    }

    let max_span_ps = nets
        .iter()
        .map(|(s, d, _, _, _)| (manhattan(*s, *d) as i64) * HOP_DELAY_PS)
        .max()
        .unwrap_or(HOP_DELAY_PS)
        .max(HOP_DELAY_PS);

    let mut crits: Vec<f64> = nets
        .iter()
        .map(|(s, d, _, _, name)| {
            if let Some(&c) = cfg.net_criticality.get(name) {
                c.clamp(0.0, cfg.max_criticality.clamp(0.0, 0.999))
            } else {
                let est_delay = (manhattan(*s, *d) as i64) * HOP_DELAY_PS;
                let slack = (max_span_ps - est_delay).max(0);
                net_criticality_from_slack(
                    slack,
                    max_span_ps,
                    cfg.criticality_exp,
                    cfg.max_criticality,
                )
            }
        })
        .collect();

    let bboxes: Vec<((u32, u32), (u32, u32), f64)> = nets
        .iter()
        .enumerate()
        .map(|(i, (s, d, _, _, _))| (*s, *d, crits[i]))
        .collect();
    let batches = schedule_disjoint_bbox_batches(&bboxes, 1);

    let max_iters = cfg.max_iters.max(1);
    let mut hist = vec![0u32; grid.len()];
    let mut pres = vec![0u32; grid.len()];
    let mut paths: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nets.len()];
    let mut best_paths = paths.clone();
    let mut best_overused = u32::MAX;
    let mut best_hops = u32::MAX;
    let mut pres_fac = cfg.pres_fac_init.max(0.5);
    let mut iters = 0u32;

    for iter in 0..max_iters {
        iters = iter + 1;
        // Route each batch of spatially disjoint nets; within a batch, nets have
        // non-overlapping bounding boxes so they can be routed in parallel with Rayon.
        for batch in &batches {
            // Rip up current paths of nets in this batch from `pres`.
            if iter > 0 {
                for &ni in batch {
                    let (src, dst, _, _, _) = nets[ni];
                    for &(tx, ty) in &paths[ni] {
                        if (tx, ty) == src || (tx, ty) == dst {
                            continue;
                        }
                        if let Some(ti) = grid.index(tx, ty) {
                            pres[ti] = pres[ti].saturating_sub(1);
                        }
                    }
                }
            }
            let batch_results: Result<Vec<(usize, Vec<(u32, u32)>)>, String> = if batch.len() >= 2 {
                std::thread::scope(|s| {
                    let handles: Vec<_> = batch
                        .iter()
                        .map(|&ni| {
                            let (src, dst, _, _, _) = nets[ni];
                            let crit = crits[ni];
                            let hist_ref = &hist;
                            let pres_ref = &pres;
                            s.spawn(move || {
                                let mut scratch = AstarScratch::new(grid.len());
                                let p = astar_tdr(
                                    dev,
                                    &grid,
                                    &mut scratch,
                                    src,
                                    dst,
                                    crit,
                                    cfg,
                                    hist_ref,
                                    pres_ref,
                                    pres_fac,
                                )?;
                                Ok::<(usize, Vec<(u32, u32)>), String>((ni, p))
                            })
                        })
                        .collect();
                    let mut out = Vec::with_capacity(handles.len());
                    for h in handles {
                        out.push(h.join().map_err(|_| "tdr worker panicked".to_string())??);
                    }
                    Ok(out)
                })
            } else {
                let mut out = Vec::with_capacity(batch.len());
                let mut scratch = AstarScratch::new(grid.len());
                for &ni in batch {
                    let (src, dst, _, _, _) = nets[ni];
                    let p = astar_tdr(
                        dev,
                        &grid,
                        &mut scratch,
                        src,
                        dst,
                        crits[ni],
                        cfg,
                        &hist,
                        &pres,
                        pres_fac,
                    )?;
                    out.push((ni, p));
                }
                Ok(out)
            };
            for (ni, p) in batch_results? {
                let (src, dst, _, _, _) = nets[ni];
                for &(tx, ty) in &p {
                    if (tx, ty) == src || (tx, ty) == dst {
                        continue;
                    }
                    if let Some(ti) = grid.index(tx, ty) {
                        pres[ti] = pres[ti].saturating_add(1);
                    }
                }
                paths[ni] = p;
            }
        }

        let ou = pres.iter().filter(|&&c| c > 1).count() as u32;
        let hops_sum: u32 = paths.iter().map(|p| p.len().saturating_sub(1) as u32).sum();
        if ou < best_overused || (ou == best_overused && hops_sum < best_hops) {
            best_overused = ou;
            best_hops = hops_sum;
            best_paths = paths.clone();
        }
        if ou == 0 {
            break;
        }
        for (ti, &c) in pres.iter().enumerate() {
            if c > 1 {
                hist[ti] = hist[ti].saturating_add(c - 1);
            }
        }
        // Update per-net criticality from current routed delay relative to D_max.
        let d_max_iter = paths
            .iter()
            .map(|p| (p.len().saturating_sub(1) as i64) * HOP_DELAY_PS)
            .max()
            .unwrap_or(HOP_DELAY_PS)
            .max(HOP_DELAY_PS);
        for (ni, (_, _, _, _, name)) in nets.iter().enumerate() {
            if !cfg.net_criticality.contains_key(name) {
                let d_net = (paths[ni].len().saturating_sub(1) as i64) * HOP_DELAY_PS;
                let slack = (d_max_iter - d_net).max(0);
                crits[ni] = net_criticality_from_slack(
                    slack,
                    d_max_iter,
                    cfg.criticality_exp,
                    cfg.max_criticality,
                );
            }
        }
        pres_fac *= cfg.pres_fac_mult.max(1.1);
    }

    let mut congestion = vec![0u32; grid.len()];
    for (i, path) in best_paths.iter().enumerate() {
        let (src, dst, _, _, _) = nets[i];
        for &(tx, ty) in path {
            if (tx, ty) == src || (tx, ty) == dst {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                congestion[ti] = congestion[ti].saturating_add(1);
            }
        }
    }

    let min_hops = if cfg.min_hold_delay_ps > 0 {
        ((cfg.min_hold_delay_ps + HOP_DELAY_PS - 1) / HOP_DELAY_PS).max(0) as u32
    } else {
        0
    };
    let mut iob_src = Vec::with_capacity(nets.len());
    for (i, (src, dst, ble, _, net)) in nets.into_iter().enumerate() {
        let raw_hops = best_paths[i].len().saturating_sub(1) as u32;
        let hops = raw_hops.max(min_hops);
        let from_ff = q_net_lutff
            .get(net.as_str())
            .and_then(|&idx| placed.packed.lutffs.get(idx))
            .map(|l| !l.ff_cell.is_empty())
            .unwrap_or(true);
        iob_src.push(IobRoute {
            iob: dst,
            clb: src,
            ble,
            hops,
            delay_ps: hops as i64 * HOP_DELAY_PS,
            path: best_paths[i].clone(),
            net,
            from_ff,
        });
    }

    Ok(Routed {
        placed: placed.clone(),
        iob_src,
        imux,
        pathfinder_iters: iters,
        overused: best_overused,
        imux_skip,
        congestion,
        congestion_x0: grid.x0,
        congestion_y0: grid.y0,
        congestion_cols: grid.cols,
        congestion_rows: grid.rows,
    })
}

/// Count the number of 90-degree turns (vias / switchbox direction changes) along a tile path.
pub fn count_route_vias(path: &[(u32, u32)]) -> u32 {
    if path.len() < 3 {
        return 0;
    }
    let mut vias = 0u32;
    for w in path.windows(3) {
        let dx1 = w[1].0 as i32 - w[0].0 as i32;
        let dy1 = w[1].1 as i32 - w[0].1 as i32;
        let dx2 = w[2].0 as i32 - w[1].0 as i32;
        let dy2 = w[2].1 as i32 - w[1].1 as i32;
        if (dx1, dy1) != (dx2, dy2) {
            vias += 1;
        }
    }
    vias
}

fn rebuild_congestion_map(routed: &mut Routed) {
    let cols = routed.congestion_cols;
    let rows = routed.congestion_rows;
    let len = (cols as usize).saturating_mul(rows as usize);
    routed.congestion = vec![0u32; len];
    if cols == 0 || rows == 0 {
        routed.overused = 0;
        return;
    }
    let grid = TileGrid {
        x0: routed.congestion_x0,
        y0: routed.congestion_y0,
        cols,
        rows,
    };
    for r in &routed.iob_src {
        for &(tx, ty) in &r.path {
            if (tx, ty) == r.clb || (tx, ty) == r.iob {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                routed.congestion[ti] = routed.congestion[ti].saturating_add(1);
            }
        }
    }
    routed.overused = routed.congestion.iter().filter(|&&c| c > 1).count() as u32;
}

/// Vivado UG904 `set_bus_skew` constraint for multi-bit Gray/CDC buses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusSkewConstraint {
    /// Logical bus name or prefix (e.g. `"gray_ptr"`).
    pub bus_name: String,
    /// Explicit net names belonging to this bus (if empty, matches nets starting with `bus_name`).
    pub bit_nets: Vec<String>,
    /// Maximum allowed bit-to-bit routing delay skew $\Delta_{\text{skew\_ps}}$ (ps).
    pub max_skew_ps: i64,
}

/// Per-bus skew summary produced by [`route_bus_skew_aware`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusSkewReport {
    pub bus_name: String,
    pub max_delay_ps: i64,
    pub min_delay_ps: i64,
    pub skew_before_ps: i64,
    pub skew_after_ps: i64,
    pub detoured_bits: usize,
    pub met: bool,
}

/// Route a placed design and enforce `set_bus_skew` constraints across multi-bit buses:
///
/// $$|\max_i d(b_i) - \min_j d(b_j)| \le \Delta_{\text{skew\_ps}}$$
///
/// Routes the slowest bit first to establish $T_{\text{target}} = \max_i d(b_i)$, then
/// inserts uncongested orthogonal tile dogleg detours (and directed delay padding if needed)
/// on fast bits whose delay falls below $T_{\text{target}} - \Delta_{\text{skew\_ps}}$.
pub fn route_bus_skew_aware(
    placed: &Placed,
    dev: &Device,
    constraints: &[BusSkewConstraint],
) -> Result<(Routed, Vec<BusSkewReport>), String> {
    let mut out = route(placed, dev)?;
    let grid = TileGrid::from_device(dev);
    let mut reports = Vec::with_capacity(constraints.len());

    for c in constraints {
        let max_skew = c.max_skew_ps.max(0);
        let member_indices: Vec<usize> = out
            .iob_src
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                if c.bit_nets.is_empty() {
                    r.net.starts_with(&c.bus_name)
                } else {
                    c.bit_nets.iter().any(|n| n == &r.net)
                }
            })
            .map(|(i, _)| i)
            .collect();
        if member_indices.len() < 2 {
            let d = member_indices
                .first()
                .map(|&i| out.iob_src[i].delay_ps)
                .unwrap_or(0);
            reports.push(BusSkewReport {
                bus_name: c.bus_name.clone(),
                max_delay_ps: d,
                min_delay_ps: d,
                skew_before_ps: 0,
                skew_after_ps: 0,
                detoured_bits: 0,
                met: true,
            });
            continue;
        }

        let target_delay = member_indices
            .iter()
            .map(|&i| out.iob_src[i].delay_ps)
            .max()
            .unwrap_or(0);
        let min_before = member_indices
            .iter()
            .map(|&i| out.iob_src[i].delay_ps)
            .min()
            .unwrap_or(0);
        let skew_before_ps = target_delay - min_before;
        let min_required_delay = (target_delay - max_skew).max(0);
        let mut detoured_bits = 0usize;

        for &ri in &member_indices {
            if out.iob_src[ri].delay_ps >= min_required_delay {
                continue;
            }
            detoured_bits += 1;
            while out.iob_src[ri].delay_ps < min_required_delay {
                // Only insert a 2-hop dogleg (+80ps) if it won't overshoot target_delay + max_skew.
                if out.iob_src[ri].delay_ps + 2 * HOP_DELAY_PS > target_delay + max_skew {
                    break;
                }
                let path = &out.iob_src[ri].path;
                let mut inserted = None;
                for idx in 0..path.len().saturating_sub(1) {
                    let a = path[idx];
                    let b = path[idx + 1];
                    let dx = b.0 as i32 - a.0 as i32;
                    let dy = b.1 as i32 - a.1 as i32;
                    let perp: [(i32, i32); 2] = if dx == 0 && dy.abs() == 1 {
                        [(1, 0), (-1, 0)]
                    } else if dy == 0 && dx.abs() == 1 {
                        [(0, 1), (0, -1)]
                    } else {
                        continue;
                    };
                    for (ox, oy) in perp {
                        let ax = a.0 as i32 + ox;
                        let ay = a.1 as i32 + oy;
                        let bx = b.0 as i32 + ox;
                        let by = b.1 as i32 + oy;
                        if ax < 0 || ay < 0 || bx < 0 || by < 0 {
                            continue;
                        }
                        let pa = (ax as u32, ay as u32);
                        let pb = (bx as u32, by as u32);
                        if !on_grid(dev, pa.0, pa.1) || !on_grid(dev, pb.0, pb.1) {
                            continue;
                        }
                        if path.contains(&pa) || path.contains(&pb) {
                            continue;
                        }
                        let (Some(tia), Some(tib)) = (grid.index(pa.0, pa.1), grid.index(pb.0, pb.1))
                        else {
                            continue;
                        };
                        if out.congestion.get(tia).copied().unwrap_or(0) == 0
                            && out.congestion.get(tib).copied().unwrap_or(0) == 0
                        {
                            inserted = Some((idx + 1, pa, pb, tia, tib));
                            break;
                        }
                    }
                    if inserted.is_some() {
                        break;
                    }
                }
                if let Some((ins_at, pa, pb, tia, tib)) = inserted {
                    out.iob_src[ri].path.insert(ins_at, pb);
                    out.iob_src[ri].path.insert(ins_at, pa);
                    if let Some(cg) = out.congestion.get_mut(tia) {
                        *cg = cg.saturating_add(1);
                    }
                    if let Some(cg) = out.congestion.get_mut(tib) {
                        *cg = cg.saturating_add(1);
                    }
                    let hops = out.iob_src[ri].path.len().saturating_sub(1) as u32;
                    out.iob_src[ri].hops = hops;
                    out.iob_src[ri].delay_ps = hops as i64 * HOP_DELAY_PS;
                } else {
                    // Fallback: directed 1-hop delay pad when physical dogleg would overshoot or saturate.
                    let needed_hops =
                        ((min_required_delay + HOP_DELAY_PS - 1) / HOP_DELAY_PS) as u32;
                    out.iob_src[ri].hops = out.iob_src[ri].hops.max(needed_hops);
                    out.iob_src[ri].delay_ps = out.iob_src[ri].hops as i64 * HOP_DELAY_PS;
                    break;
                }
            }
            if out.iob_src[ri].delay_ps < min_required_delay {
                let needed_hops = ((min_required_delay + HOP_DELAY_PS - 1) / HOP_DELAY_PS) as u32;
                out.iob_src[ri].hops = out.iob_src[ri].hops.max(needed_hops);
                out.iob_src[ri].delay_ps = out.iob_src[ri].hops as i64 * HOP_DELAY_PS;
            }
        }

        let max_after = member_indices
            .iter()
            .map(|&i| out.iob_src[i].delay_ps)
            .max()
            .unwrap_or(0);
        let min_after = member_indices
            .iter()
            .map(|&i| out.iob_src[i].delay_ps)
            .min()
            .unwrap_or(0);
        let skew_after_ps = max_after - min_after;
        reports.push(BusSkewReport {
            bus_name: c.bus_name.clone(),
            max_delay_ps: max_after,
            min_delay_ps: min_after,
            skew_before_ps,
            skew_after_ps,
            detoured_bits,
            met: skew_after_ps <= max_skew,
        });
    }
    rebuild_congestion_map(&mut out);
    Ok((out, reports))
}

/// Crosstalk spacing & grounded `GND` shield policy for timing-critical nets
/// ([`apply_crosstalk_spacing`]).
#[derive(Clone, Debug)]
pub struct CrosstalkSpacingPolicy {
    /// Net names classified as timing-critical victims/aggressors.
    pub critical_nets: HashSet<String>,
    /// Extra step penalty (in [`HOP_DELAY_PS`] multiples) on tiles immediately adjacent
    /// ($(x \pm 1, y)$ or $(x, y \pm 1)$) to a critical net's intermediate segments.
    pub adjacent_penalty_hops: u32,
    /// Reserve uncongested adjacent $(x \pm 1, y)$ tiles as grounded `GND` shield tracks.
    pub reserve_gnd_shields: bool,
}

impl Default for CrosstalkSpacingPolicy {
    fn default() -> Self {
        Self {
            critical_nets: HashSet::new(),
            adjacent_penalty_hops: 6,
            reserve_gnd_shields: true,
        }
    }
}

/// Telemetry from [`apply_crosstalk_spacing`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrosstalkShieldReport {
    pub critical_nets_routed: usize,
    pub shield_tiles_reserved: Vec<(u32, u32)>,
    pub rerouted_aggressors: usize,
    pub adjacent_couplings_before: usize,
    pub adjacent_couplings_after: usize,
}

fn count_adjacent_couplings(routes: &[IobRoute], critical_nets: &HashSet<String>) -> usize {
    let mut crit_tiles: HashSet<(u32, u32)> = HashSet::new();
    for r in routes {
        if critical_nets.contains(&r.net) {
            for &pt in &r.path {
                if pt != r.clb && pt != r.iob {
                    crit_tiles.insert(pt);
                }
            }
        }
    }
    if crit_tiles.is_empty() {
        return 0;
    }
    let mut couplings = 0usize;
    for r in routes {
        if critical_nets.contains(&r.net) {
            continue;
        }
        for &(x, y) in &r.path {
            if (x, y) == r.clb || (x, y) == r.iob {
                continue;
            }
            for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 && crit_tiles.contains(&(nx as u32, ny as u32)) {
                    couplings += 1;
                }
            }
        }
    }
    couplings
}

/// Apply critical-net crosstalk spacing and optional `GND` track shielding
/// ([`CrosstalkSpacingPolicy`]).
///
/// Penalizes tiles immediately adjacent to critical nets' intermediate segments and
/// reroutes non-critical aggressor nets away from the critical halo when an uncongested
/// lower-coupling path exists.
pub fn apply_crosstalk_spacing(
    routed: &Routed,
    dev: &Device,
    policy: &CrosstalkSpacingPolicy,
) -> Result<(Routed, CrosstalkShieldReport), String> {
    let mut out = routed.clone();
    let couplings_before = count_adjacent_couplings(&out.iob_src, &policy.critical_nets);
    let grid = TileGrid::from_device(dev);
    let mut halo_penalty = vec![0u32; grid.len()];
    let mut crit_count = 0usize;
    let mut shield_set: HashSet<(u32, u32)> = HashSet::new();

    for r in &out.iob_src {
        if !policy.critical_nets.contains(&r.net) {
            continue;
        }
        crit_count += 1;
        for &(x, y) in &r.path {
            if (x, y) == r.clb || (x, y) == r.iob {
                continue;
            }
            for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 {
                    continue;
                }
                let (ux, uy) = (nx as u32, ny as u32);
                if !on_grid(dev, ux, uy) {
                    continue;
                }
                if let Some(ti) = grid.index(ux, uy) {
                    halo_penalty[ti] = halo_penalty[ti]
                        .saturating_add(policy.adjacent_penalty_hops.max(1) * HOP_DELAY_PS as u32);
                    if policy.reserve_gnd_shields && dy == 0 {
                        shield_set.insert((ux, uy));
                    }
                }
            }
        }
    }

    let mut scratch = AstarScratch::new(grid.len());
    let mut rerouted_aggressors = 0usize;
    for ri in 0..out.iob_src.len() {
        if policy.critical_nets.contains(&out.iob_src[ri].net) {
            continue;
        }
        let src = out.iob_src[ri].clb;
        let dst = out.iob_src[ri].iob;
        let old_path = &out.iob_src[ri].path;
        let touches_halo = old_path.iter().any(|&(x, y)| {
            (x, y) != src
                && (x, y) != dst
                && grid
                    .index(x, y)
                    .map(|ti| halo_penalty[ti] > 0)
                    .unwrap_or(false)
        });
        if !touches_halo {
            continue;
        }
        let mut pres = out.congestion.clone();
        for &(tx, ty) in old_path {
            if (tx, ty) == src || (tx, ty) == dst {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                pres[ti] = pres[ti].saturating_sub(1);
            }
        }
        if let Ok(new_path) = astar(dev, &grid, &mut scratch, src, dst, &halo_penalty, &pres, 8, 0)
        {
            if new_path != *old_path {
                let extra_pad = out.iob_src[ri]
                    .hops
                    .saturating_sub(old_path.len().saturating_sub(1) as u32);
                let new_hops = new_path.len().saturating_sub(1) as u32 + extra_pad;
                out.iob_src[ri].path = new_path;
                out.iob_src[ri].hops = new_hops;
                out.iob_src[ri].delay_ps = new_hops as i64 * HOP_DELAY_PS;
                rebuild_congestion_map(&mut out);
                rerouted_aggressors += 1;
            }
        }
    }

    let mut shield_tiles_reserved: Vec<(u32, u32)> = shield_set
        .into_iter()
        .filter(|&(x, y)| {
            grid.index(x, y)
                .and_then(|ti| out.congestion.get(ti))
                .copied()
                .unwrap_or(1)
                == 0
        })
        .collect();
    shield_tiles_reserved.sort_unstable();
    let couplings_after = count_adjacent_couplings(&out.iob_src, &policy.critical_nets);

    Ok((
        out,
        CrosstalkShieldReport {
            critical_nets_routed: crit_count,
            shield_tiles_reserved,
            rerouted_aggressors,
            adjacent_couplings_before: couplings_before,
            adjacent_couplings_after: couplings_after,
        },
    ))
}

/// Summary metrics from post-route via minimization ([`minimize_route_vias`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViaMinimizationStats {
    pub nets_simplified: usize,
    pub vias_before: u32,
    pub vias_after: u32,
    pub hops_before: u32,
    pub hops_after: u32,
}

/// Post-route via minimization (`minimize_route_vias`):
/// scans routed net paths for redundant `H -> V -> H` / `V -> H -> V` staircase jogs
/// and replaces them with direct 0-via straight lines or 1-via L-shaped corners when
/// the replacement is congestion-free and does not increase path delay (hops).
pub fn minimize_route_vias(
    routed: &Routed,
    dev: &Device,
) -> Result<(Routed, ViaMinimizationStats), String> {
    let mut out = routed.clone();
    let vias_before: u32 = out.iob_src.iter().map(|r| count_route_vias(&r.path)).sum();
    let hops_before: u32 = out.iob_src.iter().map(|r| r.hops).sum();
    let grid = TileGrid::from_device(dev);
    let mut nets_simplified = 0usize;

    for ri in 0..out.iob_src.len() {
        let src = out.iob_src[ri].clb;
        let dst = out.iob_src[ri].iob;
        let old_path = &out.iob_src[ri].path;
        let old_vias = count_route_vias(old_path);
        if old_vias <= 1 && old_path.len().saturating_sub(1) as u32 == manhattan(src, dst) {
            continue;
        }
        let old_tile_hops = old_path.len().saturating_sub(1) as u32;
        let extra_pad = out.iob_src[ri].hops.saturating_sub(old_tile_hops);

        // Rip up current net's intermediate tiles from congestion map.
        let mut pres = out.congestion.clone();
        for &(tx, ty) in old_path {
            if (tx, ty) == src || (tx, ty) == dst {
                continue;
            }
            if let Some(ti) = grid.index(tx, ty) {
                pres[ti] = pres[ti].saturating_sub(1);
            }
        }

        let try_l_corner = |hv: bool| -> Option<Vec<(u32, u32)>> {
            let mut p = Vec::with_capacity((manhattan(src, dst) + 1) as usize);
            let (mut x, mut y) = src;
            p.push((x, y));
            if hv {
                while x != dst.0 {
                    x = if x < dst.0 { x + 1 } else { x - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
                while y != dst.1 {
                    y = if y < dst.1 { y + 1 } else { y - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
            } else {
                while y != dst.1 {
                    y = if y < dst.1 { y + 1 } else { y - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
                while x != dst.0 {
                    x = if x < dst.0 { x + 1 } else { x - 1 };
                    if !on_grid(dev, x, y) {
                        return None;
                    }
                    p.push((x, y));
                }
            }
            for &(tx, ty) in &p {
                if (tx, ty) == src || (tx, ty) == dst {
                    continue;
                }
                let ti = grid.index(tx, ty)?;
                if pres.get(ti).copied().unwrap_or(0) > 0 {
                    return None;
                }
            }
            Some(p)
        };

        if let Some(new_path) = try_l_corner(false).or_else(|| try_l_corner(true)) {
            let new_tile_hops = new_path.len().saturating_sub(1) as u32;
            let new_vias = count_route_vias(&new_path);
            if new_tile_hops <= old_tile_hops
                && (new_vias < old_vias || new_tile_hops < old_tile_hops)
            {
                let new_hops = new_tile_hops + extra_pad;
                out.iob_src[ri].path = new_path;
                out.iob_src[ri].hops = new_hops;
                out.iob_src[ri].delay_ps = new_hops as i64 * HOP_DELAY_PS;
                rebuild_congestion_map(&mut out);
                nets_simplified += 1;
            }
        }
    }

    let vias_after: u32 = out.iob_src.iter().map(|r| count_route_vias(&r.path)).sum();
    let hops_after: u32 = out.iob_src.iter().map(|r| r.hops).sum();
    Ok((
        out,
        ViaMinimizationStats {
            nets_simplified,
            vias_before,
            vias_after,
            hops_before,
            hops_after,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::Design;
    use helion_pack::pack;
    use helion_place::hard_heartbeat;
    use helion_place::{PlaceOpts, place, place_with, place_with_guide};

    #[test]
    fn routes_south_to_iob() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
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
        assert!(
            !r.iob_src[0].net.is_empty(),
            "IOB route names the packed net"
        );
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
    fn congestion_counts_intermediate_tiles() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        assert_eq!(
            r.congestion.len(),
            r.congestion_cols as usize * r.congestion_rows as usize
        );
        // Grid origin matches the router tile grid.
        assert!(r.congestion_cols > 0 && r.congestion_rows > 0);
        // Every intermediate tile of the routed path counts exactly once;
        // driver and sink terminals never count toward themselves.
        let io = &r.iob_src[0];
        let mut expect = vec![0u32; r.congestion.len()];
        for &(tx, ty) in &io.path {
            if (tx, ty) == io.clb || (tx, ty) == io.iob {
                continue;
            }
            let dx = tx - r.congestion_x0;
            let dy = ty - r.congestion_y0;
            expect[(dy * r.congestion_cols + dx) as usize] += 1;
        }
        assert_eq!(r.congestion, expect);
        // Sanity: an unrouted (empty) Routed has an all-zero congestion map.
        let r2 = Routed {
            placed: pl.clone(),
            iob_src: Vec::new(),
            imux: Vec::new(),
            pathfinder_iters: 0,
            overused: 0,
            imux_skip: 0,
            congestion: vec![0u32; r.congestion.len()],
            congestion_x0: r.congestion_x0,
            congestion_y0: r.congestion_y0,
            congestion_cols: r.congestion_cols,
            congestion_rows: r.congestion_rows,
        };
        assert!(r2.congestion.iter().all(|&c| c == 0));
    }

    #[test]
    fn delay_in_cost_makes_td_faster_than_wl() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let wl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.0, seed: 0 }).unwrap();
        let td = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
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
        assert_eq!(
            r_td.iob_src[0].delay_ps,
            r_td.iob_src[0].hops as i64 * HOP_DELAY_PS
        );
        assert!(r_td.iob_src[0].hops >= 1);
    }

    #[test]
    fn extra_hops_add_directed_delay() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
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

    /// ≥8 LUT+FF heartbeat (Ibex pin-wrap class, not 108k P&R).
    fn hops_of(r: &Routed) -> Vec<u32> {
        r.iob_src.iter().map(|io| io.hops).collect()
    }

    fn sites_of(r: &Routed) -> Vec<(u32, u32, u8)> {
        r.placed
            .lutff_sites
            .iter()
            .map(|(s, ble)| (s.x, s.y, *ble))
            .collect()
    }

    #[test]
    fn empty_guide_matches_route_with() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let a = route(&pl, &dev).unwrap();
        let b = route_with_guide(&pl, &dev, RouteOpts::default(), &TimingGuide::default()).unwrap();
        assert_eq!(hops_of(&a), hops_of(&b), "empty guide must keep gold hops");
        assert_eq!(a.iob_src[0].path, b.iob_src[0].path);
    }

    #[test]
    fn false_path_guide_changes_hops_or_sites_on_hard_fixture() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        assert!(p.lutffs.len() >= 8);
        let opts = PlaceOpts {
            timing_weight: 0.75,
                seed: 0,
        };
        let g0 = TimingGuide::default();
        let gfp = TimingGuide {
            false_path_iob: true,
            iob_setup_mult: 1,
            ..Default::default()
        };
        let pl0 = place_with_guide(&p, &dev, opts, &g0).unwrap();
        let plfp = place_with_guide(&p, &dev, opts, &gfp).unwrap();
        let r0 = route_with_guide(&pl0, &dev, RouteOpts::default(), &g0).unwrap();
        let rfp = route_with_guide(&plfp, &dev, RouteOpts::default(), &gfp).unwrap();
        assert!(
            sites_of(&r0) != sites_of(&rfp) || hops_of(&r0) != hops_of(&rfp),
            "false_path must change placed sites or routed hops (sites {:?} vs {:?} hops {:?} vs {:?})",
            sites_of(&r0),
            sites_of(&rfp),
            hops_of(&r0),
            hops_of(&rfp)
        );
        assert_ne!(
            r0.iob_src[0].hops, rfp.iob_src[0].hops,
            "false_path IOB must not be delay-optimized (hops {} vs {})",
            r0.iob_src[0].hops, rfp.iob_src[0].hops
        );
        assert_eq!(
            rfp.iob_src[0].path.len().saturating_sub(1) as u32,
            rfp.iob_src[0].hops,
            "false_path hops must be PathFinder tiles, not a post-route pad"
        );
        assert_ne!(
            r0.iob_src[0].path, rfp.iob_src[0].path,
            "false_path must change the routed path"
        );
    }

    #[test]
    fn multicycle_guide_changes_hops_or_sites_on_hard_fixture() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        let opts = PlaceOpts {
            timing_weight: 0.75,
                seed: 0,
        };
        let g0 = TimingGuide::default();
        let gm = TimingGuide {
            false_path_iob: false,
            iob_setup_mult: 2,
            ..Default::default()
        };
        let pl0 = place_with_guide(&p, &dev, opts, &g0).unwrap();
        let plm = place_with_guide(&p, &dev, opts, &gm).unwrap();
        let r0 = route_with_guide(&pl0, &dev, RouteOpts::default(), &g0).unwrap();
        let rm = route_with_guide(&plm, &dev, RouteOpts::default(), &gm).unwrap();
        assert!(
            sites_of(&r0) != sites_of(&rm) || hops_of(&r0) != hops_of(&rm),
            "multicycle must change placed sites or routed hops (sites {:?} vs {:?} hops {:?} vs {:?})",
            sites_of(&r0),
            sites_of(&rm),
            hops_of(&r0),
            hops_of(&rm)
        );
        assert_ne!(r0.iob_src[0].hops, rm.iob_src[0].hops);
        assert_eq!(
            rm.iob_src[0].path.len().saturating_sub(1) as u32,
            rm.iob_src[0].hops,
            "multicycle hops must be PathFinder tiles, not a post-route pad"
        );
        assert_ne!(r0.iob_src[0].path, rm.iob_src[0].path);
        let gfp = TimingGuide {
            false_path_iob: true,
            iob_setup_mult: 2,
            ..Default::default()
        };
        let rfp = route_with_guide(
            &place_with_guide(&p, &dev, opts, &gfp).unwrap(),
            &dev,
            RouteOpts::default(),
            &gfp,
        )
        .unwrap();
        assert_ne!(rm.iob_src[0].hops, rfp.iob_src[0].hops);
        assert_ne!(rm.iob_src[0].path, rfp.iob_src[0].path);
    }

    #[test]
    fn routed_path_sites_resolve_in_had() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let r = route(&pl, &dev).unwrap();
        assert!(!r.iob_src.is_empty());
        let ids = r.path_site_ids(&dev);
        assert!(!ids.is_empty(), "IOB route must yield HAD sites");
        for id in &ids {
            assert!(
                dev.site_by_id(id).is_some(),
                "path site {id} must be in HAD"
            );
        }
        let clb = r.iob_src[0].clb_site();
        let iob = r.iob_src[0].iob_site();
        assert!(dev.contains_site(clb));
        assert!(dev.contains_site(iob));
        assert_eq!(
            r.iob_src[0].clb_bel().id(),
            format!("{}/BLE{}.FF", clb.id(), r.iob_src[0].ble)
        );
        assert_eq!(
            r.iob_src[0].clb_net_id().id(),
            format!("{}/BLE{}.Q", clb.id(), r.iob_src[0].ble)
        );
        let endpoints = [("u_ff3", clb), ("u_iob", iob)];
        let resolved = dev.resolve_path_sites(&endpoints).unwrap();
        assert!(resolved.contains(&clb.id()));
        assert!(resolved.contains(&iob.id()));
    }

    /// Shared CLB driver sites must not inflate PathFinder overused: occupancy
    /// counts intermediate RR tiles only (per-net terminal skip).
    #[test]
    fn overused_ignores_shared_clb_terminals() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        assert!(p.lutffs.len() >= 8);
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let r = route(&pl, &dev).unwrap();
        assert!(r.pathfinder_iters >= 1);
        // hard_heartbeat is a single-IOB fixture after place; still must be clean.
        assert_eq!(
            r.overused, 0,
            "terminal-only overlap must not count as overused"
        );
        assert!(!r.iob_src.is_empty());
        assert_eq!(
            r.iob_src[0].path.len().saturating_sub(1) as u32,
            r.iob_src[0].hops
        );
    }

    #[test]
    fn phys_opt_route_hold_fix_inserts_real_tile_detours_and_phys_opt_route_reclaims_them() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let base = route(&pl, &dev).unwrap();
        let base_delay = base.iob_src[0].delay_ps;
        let target_delay = base_delay + 4 * HOP_DELAY_PS;
        let (hold_fixed, fixed_count) =
            phys_opt_route_hold_fix(&base, &dev, target_delay).unwrap();
        assert_eq!(fixed_count, 1);
        assert!(
            hold_fixed.iob_src[0].delay_ps >= target_delay,
            "hold fix must raise delay to >= {target_delay}ps, got {}ps",
            hold_fixed.iob_src[0].delay_ps
        );
        assert_eq!(
            hold_fixed.iob_src[0].path.len().saturating_sub(1) as u32,
            hold_fixed.iob_src[0].hops,
            "hold-fix hops must be contiguous tile steps"
        );
        for w in hold_fixed.iob_src[0].path.windows(2) {
            assert_eq!(
                manhattan(w[0], w[1]),
                1,
                "every step in hold-fixed path must be a unit Manhattan neighbor"
            );
        }
        assert_eq!(hold_fixed.overused, 0);

        // Post-route wirelength/via minimization reclaims unnecessary detours back to shortest path.
        let (reclaimed, stats) = phys_opt_route_with_stats(&hold_fixed, &dev).unwrap();
        assert_eq!(stats.shortened_nets, 1);
        assert!(stats.hops_after < stats.hops_before);
        assert!(stats.vias_after < stats.vias_before);
        assert_eq!(reclaimed.iob_src[0].hops, base.iob_src[0].hops);
    }

    #[test]
    fn tdr_cost_prioritizes_delay_for_critical_nets_and_congestion_for_non_critical() {
        // On a congested tile (hist=3, pres=2), a critical net (crit=0.95) pays far less
        // congestion penalty than a non-critical net (crit=0.05), steering non-critical
        // nets around the hot spot while letting the critical net take the direct path.
        let crit_cost = tdr_node_cost(0.95, HOP_DELAY_PS, 1.0, 3, 2, 2.0, 1.0);
        let non_crit_cost = tdr_node_cost(0.05, HOP_DELAY_PS, 1.0, 3, 2, 2.0, 1.0);
        assert!(
            crit_cost < non_crit_cost,
            "critical net cost ({crit_cost}) must be lower on congested tile than non-critical ({non_crit_cost})"
        );
        let c_hi = net_criticality_from_slack(500, 10_000, 2.0, 0.99);
        let c_lo = net_criticality_from_slack(8_000, 10_000, 2.0, 0.99);
        assert!(c_hi > c_lo);
        assert!(c_hi <= 0.99);
    }

    #[test]
    fn disjoint_bbox_scheduler_groups_non_overlapping_nets_and_separates_overlaps() {
        let bboxes = vec![
            ((2, 4), (2, 0), 0.9),  // col 2
            ((10, 4), (10, 0), 0.8), // col 10 (disjoint from col 2 with margin=1)
            ((3, 4), (3, 0), 0.5),  // col 3 (overlaps col 2 when margin=1)
        ];
        let batches = schedule_disjoint_bbox_batches(&bboxes, 1);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![0, 1], "disjoint high-crit nets share batch 0");
        assert_eq!(batches[1], vec![2], "overlapping net scheduled in batch 1");
    }

    #[test]
    fn route_timing_driven_routes_cleanly_and_honors_hold_min_delay() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let mut cfg = TimingDrivenRouterConfig::default();
        cfg.net_criticality.insert("q7".to_string(), 0.95);
        let r = route_timing_driven(&pl, &dev, &cfg).unwrap();
        assert_eq!(r.overused, 0);
        assert!(!r.iob_src.is_empty());
        let base_delay = r.iob_src[0].delay_ps;

        cfg.min_hold_delay_ps = base_delay + 3 * HOP_DELAY_PS;
        let r_hold = route_timing_driven(&pl, &dev, &cfg).unwrap();
        assert!(r_hold.iob_src[0].delay_ps >= cfg.min_hold_delay_ps);
    }

    #[test]
    fn bus_skew_aware_router_bounds_multi_bit_skew_within_tolerance() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("bus2");
        d.add_port("clk", helion_ir::PortDir::In);
        d.add_port("o0", helion_ir::PortDir::Out);
        d.add_port("o1", helion_ir::PortDir::Out);
        for i in 0..2 {
            let lut = format!("lut{i}");
            let ff = format!("ff{i}");
            let io = format!("iob{i}");
            d.add_cell(&lut, helion_ir::CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
            d.add_cell(&ff, helion_ir::CellKind::Hff);
            d.add_cell(&io, helion_ir::CellKind::IobOut);
            d.connect("clk", &ff, "CLK");
            d.connect(&format!("d{i}"), &lut, "O");
            d.connect(&format!("d{i}"), &ff, "D");
            d.connect(&format!("bus_{i}"), &ff, "Q");
            d.connect(&format!("bus_{i}"), &lut, "I0");
            d.connect(&format!("bus_{i}"), &io, "I");
            d.connect(&format!("o{i}"), &io, "PAD");
        }
        let p = pack(&d, &dev).unwrap();
        let mut pl = place(&p, &dev).unwrap();
        // Place bit 0 near south IOB (y=1) and bit 1 far north (y=6) so initial skew is 5 hops (200ps).
        let s_south = dev.clb_sites().find(|s| s.x == dev.clb_x0 && s.y == dev.clb_y0).unwrap();
        let s_north = dev.clb_sites().find(|s| s.x == dev.clb_x0 + 1 && s.y == dev.clb_y0 + 5).unwrap();
        pl.lutff_sites[0] = (s_south, 0);
        pl.lutff_sites[1] = (s_north, 0);

        let constraints = vec![BusSkewConstraint {
            bus_name: "bus_".to_string(),
            bit_nets: vec!["bus_0".to_string(), "bus_1".to_string()],
            max_skew_ps: 80,
        }];
        let (routed, reports) = route_bus_skew_aware(&pl, &dev, &constraints).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].skew_before_ps >= 200);
        assert!(
            reports[0].skew_after_ps <= 80,
            "bus skew must be bounded to <= 80ps, got {}ps",
            reports[0].skew_after_ps
        );
        assert!(reports[0].met);
        assert!(reports[0].detoured_bits >= 1);
        assert_eq!(routed.overused, 0);
    }

    #[test]
    fn crosstalk_spacing_reserves_shields_and_via_minimization_removes_jogs() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&hard_heartbeat(), &dev).unwrap();
        let opts = PlaceOpts {
            timing_weight: 0.75,
            seed: 0,
        };
        // Route with a false-path dogleg so the path has a multi-via jog (`V -> H -> V`).
        let gfp = TimingGuide {
            false_path_iob: true,
            iob_setup_mult: 1,
            ..Default::default()
        };
        let pl = place_with_guide(&p, &dev, opts, &gfp).unwrap();
        let dogleg_routed = route_with_guide(&pl, &dev, RouteOpts::default(), &gfp).unwrap();
        assert!(count_route_vias(&dogleg_routed.iob_src[0].path) >= 2);

        // Crosstalk spacing policy on the routed net reserves adjacent GND shield tiles.
        let mut policy = CrosstalkSpacingPolicy::default();
        policy.critical_nets.insert(dogleg_routed.iob_src[0].net.clone());
        let (xtalk_routed, xtalk_rep) =
            apply_crosstalk_spacing(&dogleg_routed, &dev, &policy).unwrap();
        assert_eq!(xtalk_rep.critical_nets_routed, 1);
        assert!(!xtalk_rep.shield_tiles_reserved.is_empty());

        // Post-route via minimization collapses the dogleg jog back to a 0-via or 1-via path.
        let (min_routed, via_stats) = minimize_route_vias(&xtalk_routed, &dev).unwrap();
        assert_eq!(via_stats.nets_simplified, 1);
        assert!(
            via_stats.vias_after < via_stats.vias_before,
            "vias must drop ({} -> {})",
            via_stats.vias_before,
            via_stats.vias_after
        );
        assert!(via_stats.hops_after <= via_stats.hops_before);
        assert_eq!(min_routed.overused, 0);
    }
}
