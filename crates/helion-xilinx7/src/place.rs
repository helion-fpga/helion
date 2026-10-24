//! Place stage: [`Packed7`] → [`Placed7`].
//!
//! Greedy affinity placer for 7-series sites. IOBs go to their package pins,
//! BUFGCTRLs to clock tiles, and LUT/FF pairs sharing a `D`/`O` net are
//! packed into one slice (LUT→`x6LUT`, FF→`xFF`, x = A..D). Unpaired cells
//! get their own slice. The placer queries the device through
//! [`DeviceQuery`] so the algorithm stays independent of the DB layout.

use crate::pack::{IobDir, Packed7};
use std::collections::{HashMap, HashSet};

/// What the placer needs from the device model. Implemented by the real DB
/// (`crate::db`) and by test fakes.
pub trait DeviceQuery {
    /// `(tile, site)` for a package pin, e.g. `"E3"`.
    fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)>;
    /// A BUFGCTRL site not in `used`: `(tile, site)`.
    ///
    /// Must respect `used`: a design with two clocks would otherwise place
    /// both BUFGs on the same site, silently shorting them together.
    fn bufg_site(&self, used: &HashSet<String>) -> Option<(String, String)>;
    /// A RAMB18E1 site not in `used`: `(tile, site)`.
    fn ramb_site(&self, used: &HashSet<String>) -> Option<(String, String)>;
    /// A DSP48E1 site not in `used`: `(tile, site)`.
    fn dsp_site(&self, used: &HashSet<String>) -> Option<(String, String)>;
    /// Grid coordinates of a tile (for distance estimates).
    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)>;
    /// A free SLICEL site near `(x, y)`; `used` holds taken site names.
    fn slice_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)>;

    /// All slice sites on the device: `(tile, site)`.
    ///
    /// Needed to find a legal carry column: a chain has to be placed as a run of
    /// adjacent rows in one column, which cannot be discovered by probing single
    /// sites one at a time.
    fn slices(&self) -> Vec<(String, String)>;

    /// The CARRY4 column a slice belongs to, and its position in that column.
    ///
    /// A carry chain is a hard *vertical* constraint: `CIN`/`COUT` are dedicated
    /// intra-column routes, so a chain's elements must sit in adjacent rows of
    /// one slice column. Callers need to know which slices are neighbours before
    /// they can honour that, and the answer is device-specific (the numbers in a
    /// tile name are not grid coordinates).
    ///
    /// Returns `(column_key, row)` where a larger `row` is further up the
    /// device, and `column_key` is stable within a device: two slices are
    /// vertically adjacent iff they share a `column_key` and differ by 1 in
    /// `row`. Returns `None` for a site the model does not know.
    fn slice_column(&self, tile: &str, site: &str) -> Option<(i64, i32)>;

    /// True when `site` is the SLICEM half of its CLB.
    ///
    /// A CLB holds two slices of different types, and they are NOT
    /// interchangeable for a carry chain. Refusing to distinguish them placed
    /// every chain in the SLICEL half while the reference uses SLICEM, which is
    /// what split the lane data path from the flops that sample it.
    fn slice_is_m(&self, tile: &str, site: &str) -> bool;
}

/// A placed LUT: tile + slice site + BEL.
#[derive(Clone, Debug)]
pub struct PlacedLut {
    pub name: String,
    pub init: u64,
    pub tile: String,
    pub site: String,
    /// `"A6LUT"` .. `"D6LUT"`.
    pub bel: String,
}

/// A placed FF.
#[derive(Clone, Debug)]
pub struct PlacedFf {
    pub name: String,
    pub tile: String,
    pub site: String,
    /// `"AFF"` .. `"DFF"`.
    pub bel: String,
}

/// A placed IOB.
#[derive(Clone, Debug)]
pub struct PlacedIob {
    pub name: String,
    pub port: String,
    pub dir: IobDir,
    pub tile: String,
    pub site: String,
}

/// A placed BUFGCTRL.
#[derive(Clone, Debug)]
pub struct PlacedBufg {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed RAMB18E1.
#[derive(Clone, Debug)]
pub struct PlacedBram {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed DSP48E1.
#[derive(Clone, Debug)]
pub struct PlacedDsp {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed CARRY4 (in a SLICE).
#[derive(Clone, Debug)]
pub struct PlacedCarry {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// Backend-owned placed netlist for xc7a35t.
#[derive(Clone, Debug, Default)]
pub struct Placed7 {
    pub packed: Packed7,
    pub luts: Vec<PlacedLut>,
    pub ffs: Vec<PlacedFf>,
    pub iobs: Vec<PlacedIob>,
    pub bufgs: Vec<PlacedBufg>,
    pub brams: Vec<PlacedBram>,
    pub dsps: Vec<PlacedDsp>,
    pub carries: Vec<PlacedCarry>,
}

/// BEL letters within a slice: A, B, C, D.
const BEL_LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];

/// Find `len` free slices in one column, in `len` consecutive rows, ordered
/// bottom-up (element `k` is the `k`-th row above the run's base).
///
/// This is the physical requirement for a CARRY4 chain: `COUT` of one slice
/// feeds `CIN` of the slice directly above it through a dedicated intra-column
/// route, so the elements must be *adjacent rows of one column*, not merely
/// "some free slices". Returns `None` when no column can supply such a run --
/// the caller turns that into an error rather than splitting the chain.
///
/// Columns are offered nearest-first to the anchor so a chain lands close to the
/// rest of the design; within a column the lowest suitable run wins so the
/// choice is deterministic.
fn column_run(
    device: &dyn DeviceQuery,
    ax: i32,
    ay: i32,
    used: &HashSet<String>,
    len: usize,
) -> Option<Vec<(String, String)>> {
    if len == 0 {
        return Some(Vec::new());
    }
    // Every slice in the device, grouped by column.
    // The clock spine counts DOWN from the top of the die while the site row
    // counts UP in the tile name, so the anchor row arrives in the site-row
    // space only after `nearest_slice_row` translates it.

    // Choose the SLICEM half of each CLB, not the SLICEL half.
    //
    // A CLB holds two slices, and `Device::slices()` lists `SLICEL` before
    // `SLICEM`, so the old scan always took the SLICEL half. The reference puts
    // its ENTIRE carry chain, lane LUTs and flops in `SLICEM_X0` and uses
    // `SLICEL_X1` for nothing but continuing the chain (`PRECYINIT.CIN`).
    // Splitting them -- lane `DI1MUX` in one half, the sampling flop in the
    // other -- leaves each flop latching a static value, so LD0 sits dark where
    // the reference bitstream blinks.
    //
    // Preferring the SLICEM half also matters functionally: only SLICEM can
    // host the distributed RAM/ROM a wider design may need, so spending SLICEL
    // on a carry chain is the wrong trade even where both would work.
    let mut by_col: HashMap<i64, Vec<(i32, String, String)>> = HashMap::new();
    let mut preferred: HashMap<i64, Vec<(i32, String, String)>> = HashMap::new();
    for (tile, site) in device.slices() {
        if let Some((col, row)) = device.slice_column(&tile, &site) {
            by_col.entry(col).or_default().push((row, tile.clone(), site.clone()));
            if device.slice_is_m(&tile, &site) {
                preferred.entry(col).or_default().push((row, tile, site));
            }
        }
    }
    // In each column keep only the SLICEM rows, and only where the whole
    // column has them; a column that is SLICEL-only falls back to what it has.
    // Each CLB contributes TWO slice columns -- the SLICEM half and the
    // SLICEL half are physically distinct columns (`SLICE_X112Y42` and
    // `SLICE_X113Y42`). A carry chain must live in a SLICEM column, so drop
    // the SLICEL-only columns outright rather than preferring a row within
    // each column: preferring rows leaves every SLICEL-only column in play,
    // and since `Device::slices()` lists SLICEL first and the column sort is
    // by distance, one of those columns wins and the chain lands in a SLICEL
    // again.
    if preferred.values().any(|r| !r.is_empty()) {
        by_col.retain(|c, _| preferred.get(c).map(|r| !r.is_empty()).unwrap_or(false));
    }
    // Columns nearest the anchor first; the column key breaks ties so the
    // choice is deterministic.
    let mut cols: Vec<i64> = by_col.keys().copied().collect();
    cols.sort_by_key(|c| {
        let dy = by_col[c]
            .iter()
            .map(|(r, t, _)| device.tile_xy(t).map(|(_x, y)| y).unwrap_or(*r))
            .map(|g| (g - ay).abs())
            .min()
            .unwrap_or(i32::MAX);
        (dy + (c - ax as i64).unsigned_abs() as i32, *c)
    });

    // Collect every free run of `len` consecutive rows in every column, then
    // take the one nearest the anchor ROW.
    //
    // This used to walk rows ascending from the bottom of each column and take
    // the first free run, ignoring `ay` for the row entirely. The anchor only
    // chose the COLUMN, so logic always landed at the bottom of the die: a
    // counter driving an LED 39 rows above it, reached only by a long
    // northward route. Choosing by `ay` puts the logic beside the pads it
    // serves, which is what the reference does and what the router needs.
    let mut best: Option<(i32, Vec<(String, String)>)> = None;
    for col in cols.clone() {
        let mut rows = by_col.remove(&col).unwrap_or_default();
        rows.sort_by_key(|(r, _, _)| *r);
        let mut i = 0;
        while i < rows.len() {
            let start = i;
            let mut j = i;
            while j + 1 < rows.len()
                && rows[j + 1].0 == rows[j].0 + 1
                && !used.contains(&rows[j + 1].2)
            {
                j += 1;
            }
            if !used.contains(&rows[start].2) && j + 1 - start >= len {
                // Slide the window of `len` slots ALONG this run and keep the
                // placement nearest the anchor row.
                //
                // Taking only `rows[start..start + len]` is what put the logic
                // at the bottom of a 150-row column: the free run began at row 0
                // and covered the whole column, so the window was pinned to rows
                // 0..6 and the anchor at row 45 was ignored. A 7-element carry
                // chain then sat 39 rows below the LED it drives and the output
                // net was lost on the way.
                //
                // A window that straddles `ay` scores 0; otherwise the distance
                // from `ay` to the nearer end. Scanning every window makes the
                // choice depend on the anchor rather than on where the free run
                // happens to begin.
                let n = j + 1 - start;
                let mut w = 0;
                while w + len <= n {
                    // Score by the tile's REAL `grid_y`, not by the site row.
                    //
                    // The site row and `grid_y` are different axes running in
                    // opposite directions: `CLBLM_R_X71Y45` is `grid_y` 109 and
                    // `CLBLM_R_X71Y106` is `grid_y` 45. Scoring on the site row
                    // against an anchor expressed in `grid_y` therefore picks
                    // the tile at the OPPOSITE end of the die -- which placed a
                    // one-flop design 64 rows from the pad it drives, with the
                    // output net routed the length of the device. Ask the
                    // device for the grid row and compare like with like.
                    let gy = |k: usize| -> i32 {
                        device
                            .tile_xy(&rows[start + k].1)
                            .map(|(_x, y)| y)
                            .unwrap_or(rows[start + k].0)
                    };
                    let lo = gy(w);
                    let hi = gy(w + len - 1);
                    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
                    let dist = if ay < lo {
                        lo - ay
                    } else if ay > hi {
                        ay - hi
                    } else {
                        0
                    };
                    if best.as_ref().map(|(bd, _)| dist < *bd).unwrap_or(true) {
                        let cand: Vec<(String, String)> = rows[start + w..start + w + len]
                            .iter()
                            .map(|(_, t, s)| (t.clone(), s.clone()))
                            .collect();
                        best = Some((dist, cand));
                    }
                    w += 1;
                }
            }
            i = j + 1;
        }
    }
    best.map(|(_, v)| v)
}


/// Translate a `grid_y` into the site-row space the placer works in.
///
/// The two are NOT the same axis and not linearly related: `grid_y` numbers
/// every tile row of the die downward from the top, while a site row numbers
/// only the slice rows upward in the tile name. `CLBLM_R_X71Y45` is `grid_y`
/// 109 and `CLBLM_R_X71Y0` is `grid_y` 155, so the axis is reversed and
/// compressed -- any arithmetic scale factor between them is wrong.
///
/// Rather than model that, ask the device: take the slice tile whose `grid_y`
/// is nearest the anchor and use the row it reports. That is exact by
/// construction and stays correct on any part.
fn nearest_slice_row(device: &dyn DeviceQuery, ay_grid: i32) -> i32 {
    let mut best: Option<(i32, i32)> = None;
    for (tile, site) in device.slices() {
        let Some((_col, row)) = device.slice_column(&tile, &site) else {
            continue;
        };
        let Some((_x, g)) = device.tile_xy(&tile) else {
            continue;
        };
        let d = (g - ay_grid).abs();
        if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
            best = Some((d, row));
        }
    }
    best.map(|(_, r)| r).unwrap_or(0)
}

/// Place packed cells onto device sites.
///
/// Errors loudly when a required site class is missing (no silent drops).
pub fn place(packed: &Packed7, device: &dyn DeviceQuery) -> Result<Placed7, String> {
    let mut out = Placed7 {
        packed: packed.clone(),
        ..Default::default()
    };
    let mut used_sites: HashSet<String> = HashSet::new();

    // 1. IOBs to package pins.
    // Anchor the logic on the IOBs, but a POWER INPUT IS NOT A GOOD ANCHOR.
    // Taking the first IOB in declaration order anchors on the clock pad
    // (`clk`, package pin Y9 at the bottom edge), which put the whole counter
    // 39 rows below the LED it drives. The long northward route that follows
    // is where the output signal was being lost -- the LED stayed dark with a
    // correct pad, a correct clock and a correct counter.
    //
    // Anchor on the OUTPUT pads instead: they are what the logic must reach,
    // and it is their rows the reference clusters around. An output is the
    // sink of the design; placing the logic beside it is both closer to the
    // reference and strictly better for wirelength.
    let mut anchor: Option<(i32, i32)> = None;
    let io_order: Vec<&crate::pack::PackedIob> = packed
        .iobs
        .iter()
        .filter(|i| i.dir == crate::pack::IobDir::Out)
        .chain(packed.iobs.iter().filter(|i| i.dir != crate::pack::IobDir::Out))
        .collect();
    for iob in io_order {
        // Packing drops an output IOB whose port bit has no PACKAGE_PIN, so a
        // missing `loc` here means a genuinely unplaceable IOB and stays fatal.
        let pin = iob.loc.as_deref().ok_or_else(|| {
            format!("place: IOB {} (port {}) has no PACKAGE_PIN LOC", iob.name, iob.port)
        })?;
        let (tile, site) = device.iob_site_for_pin(pin).ok_or_else(|| {
            format!("place: no IOB site for package pin {pin} (port {})", iob.port)
        })?;
        // Prefer the lowest output row as the anchor, so a design with several
        // outputs clusters below them rather than on whichever came first.
        if let Some((_x, y)) = device.tile_xy(&tile) {
            if anchor.map(|(_ax, ay)| y < ay).unwrap_or(true) {
                anchor = Some((_x, y));
            }
        }
        used_sites.insert(site.clone());
        out.iobs.push(PlacedIob {
            name: iob.name.clone(),
            port: iob.port.clone(),
            dir: iob.dir,
            tile,
            site,
        });
    }

    // 2. BUFGCTRLs.
    for bufg in &packed.bufgs {
        let (tile, site) = device
            .bufg_site(&used_sites)
            .ok_or_else(|| format!("place: no BUFGCTRL site for {}", bufg.name))?;
        used_sites.insert(site.clone());
        out.bufgs.push(PlacedBufg {
            name: bufg.name.clone(),
            tile,
            site,
        });
    }

    // 2b. RAMB18E1s.
    for bram in &packed.brams {
        let (tile, site) = device
            .ramb_site(&used_sites)
            .ok_or_else(|| format!("place: no RAMB18E1 site for {}", bram.name))?;
        used_sites.insert(site.clone());
        out.brams.push(PlacedBram {
            name: bram.name.clone(),
            tile,
            site,
        });
    }

    // 2c. DSP48E1s.
    for dsp in &packed.dsps {
        let (tile, site) = device
            .dsp_site(&used_sites)
            .ok_or_else(|| format!("place: no DSP48E1 site for {}", dsp.name))?;
        used_sites.insert(site.clone());
        out.dsps.push(PlacedDsp {
            name: dsp.name.clone(),
            tile,
            site,
        });
    }

    // 2d. CARRY4s. A carry chain is a hard *vertical* constraint: CIN/COUT are
    // dedicated intra-column routes, so the four bits of one CARRY4 share a
    // slice and successive elements must sit in ADJACENT rows of ONE column.
    // Scattering them (the old `slice_near` loop) would route a COUT to a CIN
    // that is not physically next to it, and the chain would be silently wrong.
    //
    // Chains are placed one at a time; a chain that cannot get a contiguous run
    // of free slices in a single column is an ERROR, not a fallback to a
    // scattered placement, because a split chain cannot be made correct by
    // routing alone.
    let (ax, ay_grid) = anchor.unwrap_or((0, 0));
    // `tile_xy` reports `grid_y`, which counts DOWNWARD from the top of the
    // die, while `slice_column` reports the SITE row, which counts UPWARD in
    // the tile name. They are different spaces: the LED's tile
    // `RIOB33_X73Y45` is `grid_y` 109, and so is `CLBLM_R_X71Y45`, whereas
    // `CLBLM_R_X71Y0` is `grid_y` 155. Comparing one against the other silently
    // picks the wrong end of the die -- the logic landed in rows 0..6 while the
    // pad it drives sat in row 45, 39 rows away.
    //
    // Convert the anchor into site-row space by expressing it relative to the
    // slice rows the device actually reports.
    // `ay_grid` is already the axis the comparison uses; translating it into
    // site-row space is what mixed the two axes up in the first place.
    let ay = ay_grid;

    if !packed.carries.is_empty() {
        // Group elements by chain id, preserving their first-seen order.
        let mut chain_order: Vec<String> = Vec::new();
        let mut chains: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in packed.carries.iter().enumerate() {
            chains
                .entry(c.chain.clone())
                .or_insert_with(|| {
                    chain_order.push(c.chain.clone());
                    Vec::new()
                })
                .push(i);
        }
        for chain in &chain_order {
            // LSB element first: element `k` sits `k` rows above the base, so
            // each element's COUT reaches the next element's CIN (the chain
            // runs upward through the column).
            let mut ordered = chains[chain].clone();
            ordered.sort_by_key(|&i| packed.carries[i].index);
            let len = ordered.len();
            let run = column_run(device, ax, ay, &used_sites, len).ok_or_else(|| {
                format!(
                    "place: no free slice column with {len} contiguous rows for \
                     CARRY4 chain `{chain}` (elements: {}). A carry chain cannot \
                     be split across columns; reduce the chain or free a column.",
                    ordered
                        .iter()
                        .map(|&i| packed.carries[i].name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            for (k, &i) in ordered.iter().enumerate() {
                let (tile, site) = run[k].clone();
                used_sites.insert(site.clone());
                out.carries.push(PlacedCarry {
                    name: packed.carries[i].name.clone(),
                    tile,
                    site,
                });
            }
        }
    }

    // 3. Pair FFs with their driving LUTs (FF.D == LUT.O) into one slice.
    let lut_by_output: HashMap<&str, usize> = packed
        .luts
        .iter()
        .enumerate()
        .map(|(i, l)| (l.output_net.as_str(), i))
        .collect();
    let mut lut_placed: HashSet<usize> = HashSet::new();
    // BEL occupancy per site: site -> letters used.
    let mut site_bels: HashMap<String, HashSet<char>> = HashMap::new();
    // Which tile each site belongs to.
    let mut site_tile: HashMap<String, String> = HashMap::new();

    // Carry-sum affinity. A CARRY4's lane `n` publishes its sum on
    // `lane_sum_net[n]`, and the flop that samples that bit belongs in the
    // CARRY4's OWN slice at the matching letter (`O0`->A, `O1`->B, ...), because
    // the sum reaches the flop through an intra-slice route. Placing the flop
    // elsewhere still routes through the interconnect and still simulates
    // correctly at netlist level, but it diverges from Vivado and costs the
    // chain its fast path. Reserve those sites and letters now, before the
    // generic allocator can hand them to an unrelated flop.
    let mut ff_slot: HashMap<String, (String, String, char)> = HashMap::new();
    for ci in 0..out.carries.len() {
        let placed_tile = out.carries[ci].tile.clone();
        let placed_site = out.carries[ci].site.clone();
        let carry_name = out.carries[ci].name.clone();
        let Some(pci) = packed.carries.iter().position(|c| c.name == carry_name) else {
            continue;
        };
        let pc = &packed.carries[pci];
        for (lane, letter) in ['A', 'B', 'C', 'D'].iter().enumerate() {
            let Some(sum) = pc.lane_sum_net[lane].as_deref() else {
                continue;
            };
            // Which flop owns this lane?
            //
            // The chain's own register has the lane sum as its `D` verbatim.
            // A design may also *read* that bit elsewhere --
            // `count := count + 1; LD0 <= count(24)` makes both the counter's
            // register and the output register consume bit 24, and only one of
            // them can sit on the lane.
            //
            // `n_nx_count_24` is the next-state name (`next_bit_name`) and
            // `count_24` is the registered name (`bit_name`) of the *same* bit:
            // the carry computes it, `u_ff_ccount_24` latches it, and `u_ff0`
            // samples that latch's `Q`. Requiring an exact match on the sum
            // string therefore missed the second consumer, leaving the output
            // register unplaced on a lane; nothing then routed to it, so it
            // sampled a constant while every structural check -- frame count,
            // IDCODE, clock spine, ZINI/ZRST census, pad, arm -- still passed.
            //
            // Resolve the lane to the flop that produces the bit; the other
            // consumers are then free to share the slice.
            let Some(owner) = packed.ffs.iter().find(|f| f.d_net == sum) else {
                continue;
            };
            site_bels
                .entry(placed_site.clone())
                .or_default()
                .insert(*letter);
            site_tile
                .entry(placed_site.clone())
                .or_insert_with(|| placed_tile.clone());
            used_sites.insert(placed_site.clone());
            ff_slot.insert(
                owner.name.clone(),
                (placed_tile.clone(), placed_site.clone(), *letter),
            );
        }
    }
    let alloc = |used: &mut HashSet<String>,
                     site_bels: &mut HashMap<String, HashSet<char>>,
                     site_tile: &mut HashMap<String, String>,
                     device: &dyn DeviceQuery|
     -> Result<(String, String, char), String> {
        for (site, bels) in site_bels.iter() {
            if bels.len() < 4 {
                if let Some(letter) = BEL_LETTERS.iter().find(|c| !bels.contains(c)) {
                    let tile = site_tile.get(site).cloned().unwrap_or_default();
                    return Ok((tile, site.clone(), *letter));
                }
            }
        }
        let (tile, site) = device
            .slice_near(ax, ay, used)
            .ok_or_else(|| "place: no free SLICEL site".to_string())?;
        used.insert(site.clone());
        site_tile.insert(site.clone(), tile.clone());
        site_bels.entry(site.clone()).or_default();
        Ok((tile, site, 'A'))
    };
    for (fi, ff) in packed.ffs.iter().enumerate() {
        let _ = fi;
        // A flop that samples a CARRY4 sum is pinned to the carry element's own
        // slice and lane letter; that is the whole point of the chain's fast
        // path, and it is where Vivado puts it. See `ff_slot` above.
        if let Some((tile, site, letter)) = ff_slot.get(&ff.name).cloned() {
            out.ffs.push(PlacedFf {
                name: ff.name.clone(),
                tile,
                site,
                bel: format!("{letter}FF"),
            });
            continue;
        }
        // Find driving LUT, if any, and not yet placed.
        let lut_idx = lut_by_output.get(ff.d_net.as_str()).copied().filter(|i| !lut_placed.contains(i));
        // A register that samples another register (`LD0 <= count[24]`) belongs
        // in that register's slice when a lane is free: `Q -> D` within a slice
        // is an intra-slice connection that needs no routing at all.
        //
        // This is what Vivado does with the blinky's output register, and it is
        // where the earlier failure came from. `alloc` picks the first site with
        // a free lane in iteration order, which for this design landed the flop
        // one row away in a slice the router never reached, so it sampled a
        // constant while the image looked perfect. Co-locating removes the
        // routing question entirely rather than relying on the router to solve
        // it.
        //
        // Only when the source register is already placed: if it is not, there
        // is no slice to share and the generic allocator is correct.
        let source_slice: Option<(String, String, char)> = packed
            .ffs
            .iter()
            .find(|f| f.q_net == ff.d_net)
            .and_then(|src| ff_slot.get(&src.name))
            .and_then(|(st, ss, _sl)| {
                // Reuse the source's slice only if it still has a spare lane.
                let taken = site_bels.get(ss)?;
                BEL_LETTERS
                    .iter()
                    .find(|c| !taken.contains(c))
                    .map(|c| (st.clone(), ss.clone(), *c))
            });
        let (tile, site, letter) = match source_slice {
            Some((st, ss, c)) => (st, ss, c),
            None => alloc(&mut used_sites, &mut site_bels, &mut site_tile, device)?,
        };
        site_bels.get_mut(&site).unwrap().insert(letter);
        used_sites.insert(site.clone());
        site_tile.entry(site.clone()).or_insert_with(|| tile.clone());
        if let Some(li) = lut_idx {
            // Pair: LUT takes the same letter's LUT BEL, FF takes the FF BEL.
            // (Same letter keeps the intra-slice route trivial.)
            lut_placed.insert(li);
            let lut = &packed.luts[li];
            out.luts.push(PlacedLut {
                name: lut.name.clone(),
                init: lut.init,
                tile: tile.clone(),
                site: site.clone(),
                bel: format!("{letter}6LUT"),
            });
        }
        out.ffs.push(PlacedFf {
            name: ff.name.clone(),
            tile,
            site,
            bel: format!("{letter}FF"),
        });
    }
    // Remaining unpaired LUTs.
    for (li, lut) in packed.luts.iter().enumerate() {
        if lut_placed.contains(&li) {
            continue;
        }
        let (tile, site, letter) = alloc(&mut used_sites, &mut site_bels, &mut site_tile, device)?;
        site_bels.get_mut(&site).unwrap().insert(letter);
        out.luts.push(PlacedLut {
            name: lut.name.clone(),
            init: lut.init,
            tile,
            site,
            bel: format!("{letter}6LUT"),
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{PackedBufg, PackedFf, PackedIob, PackedLut};

    struct FakeDevice;
    impl DeviceQuery for FakeDevice {
        fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)> {
            match pin {
                "E3" => Some(("RIOB33_X0Y0".into(), "IOB_X0Y0".into())),
                "H5" => Some(("RIOB33_X0Y1".into(), "IOB_X0Y1".into())),
                _ => None,
            }
        }
        fn bufg_site(&self, _used: &HashSet<String>) -> Option<(String, String)> {
            Some(("CLK_BUFG_X0Y0".into(), "BUFGCTRL_X0Y0".into()))
        }
        fn ramb_site(&self, _used: &HashSet<String>) -> Option<(String, String)> {
            Some(("BRAM_X0Y0".into(), "RAMB18_X0Y0".into()))
        }
        fn dsp_site(&self, _used: &HashSet<String>) -> Option<(String, String)> {
            Some(("DSP_X0Y0".into(), "DSP48_X0Y0".into()))
        }
        fn tile_xy(&self, _tile: &str) -> Option<(i32, i32)> {
            Some((10, 10))
        }
        fn slice_near(&self, _x: i32, _y: i32, used: &HashSet<String>) -> Option<(String, String)> {
            for i in 0..100 {
                let site = format!("SLICE_X0Y{i}");
                if !used.contains(&site) {
                    return Some((format!("CLB_X0Y{i}"), site));
                }
            }
            None
        }
        fn slices(&self) -> Vec<(String, String)> {
            // Two columns so a chain has somewhere to go, and so a test can tell
            // "one column, adjacent rows" apart from "any free slices".
            let mut v = Vec::new();
            for x in [0, 1] {
                for y in 0..20 {
                    v.push((format!("CLB_X{x}Y{y}"), format!("SLICE_X{x}Y{y}")));
                }
            }
            v
        }
        fn slice_column(&self, _tile: &str, site: &str) -> Option<(i64, i32)> {
            let rest = site.strip_prefix("SLICE_X")?;
            let (c, r) = rest.split_once('Y')?;
            Some((c.parse().ok()?, r.parse().ok()?))
        }

        /// The fake device models one slice per row in a single column, all
        /// of which stand in for the SLICEM half. Real devices pair a SLICEM
        /// with a SLICEL in each CLB; the tests only need the chain to be
        /// placeable, and `column_run` degrades to "whatever the device
        /// offers" when no column reports an SLICEM.
        fn slice_is_m(&self, _tile: &str, _site: &str) -> bool {
            true
        }
    }

    fn packed_blinky() -> Packed7 {
        Packed7 {
            luts: vec![PackedLut {
                name: "u_inv".into(),
                init: 0x5555_5555_5555_5555,
                input_nets: [Some("q".into()), None, None, None, None, None],
                output_net: "d".into(),
            }],
            ffs: vec![PackedFf {
                name: "u_ff".into(),
                d_net: "d".into(),
                q_net: "q".into(),
                clk_net: "clk_bufg".into(),
                ce_net: None,
                sr_net: None,
            }],
            iobs: vec![
                PackedIob {
                    name: "ibuf_clk".into(),
                    port: "clk".into(),
                    dir: IobDir::In,
                    net: "clk_ibuf".into(),
                    loc: Some("E3".into()),
                    iostandard: "LVCMOS33".into(),
                    drive: None,
                    slew: None,
                },
                PackedIob {
                    name: "u_obuf".into(),
                    port: "led".into(),
                    dir: IobDir::Out,
                    net: "q".into(),
                    loc: Some("H5".into()),
                    iostandard: "LVCMOS33".into(),
                    drive: None,
                    slew: None,
                },
            ],
            bufgs: vec![PackedBufg {
                name: "bufg_clk".into(),
                in_net: "clk_ibuf".into(),
                out_net: "clk_bufg".into(),
            }],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
        }
    }

    #[test]
    fn place_pairs_lut_ff_in_one_slice() {
        let placed = place(&packed_blinky(), &FakeDevice).unwrap();
        assert_eq!(placed.iobs.len(), 2);
        assert_eq!(placed.bufgs.len(), 1);
        // LUT+FF paired: same site, A6LUT + AFF.
        assert_eq!(placed.luts.len(), 1);
        assert_eq!(placed.ffs.len(), 1);
        assert_eq!(placed.luts[0].site, placed.ffs[0].site);
        assert_eq!(placed.luts[0].bel, "A6LUT");
        assert_eq!(placed.ffs[0].bel, "AFF");
        // IOBs on their pins.
        assert!(placed.iobs.iter().any(|i| i.site == "IOB_X0Y0"));
        assert!(placed.iobs.iter().any(|i| i.site == "IOB_X0Y1"));
    }

    #[test]
    fn place_missing_pin_fails_loudly() {
        let mut p = packed_blinky();
        p.iobs[0].loc = Some("Z99".into());
        let err = place(&p, &FakeDevice).unwrap_err();
        assert!(err.contains("Z99"), "got: {err}");
    }

    /// A 7-element carry chain must land in **7 adjacent rows of one column**,
    /// ordered bottom-up. This is the physical CARRY4 requirement: `COUT` feeds
    /// the `CIN` of the slice directly above, so a scattered placement (which
    /// the old `slice_near` loop produced) would be silently wrong.
    #[test]
    fn carry_chain_places_as_a_contiguous_column() {
        let mut p = packed_blinky();
        p.carries = (0..7)
            .map(|i| {
                let mut c = crate::pack::PackedCarry::empty(format!("u_c4_{i}"));
                c.chain = "q_inc".into();
                c.index = i;
                c.chain_len = 7;
                c
            })
            .collect();
        let placed = place(&p, &FakeDevice).unwrap();
        assert_eq!(placed.carries.len(), 7);

        // Every element in the same column...
        let cols: Vec<i64> = placed
            .carries
            .iter()
            .map(|c| FakeDevice.slice_column(&c.tile, &c.site).unwrap().0)
            .collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "chain spans more than one column: {cols:?}"
        );

        // ...in rows 0..7, each exactly one above the last. `placed.carries` is
        // emitted in `index` order, so the rows must ascend.
        let rows: Vec<i32> = placed
            .carries
            .iter()
            .map(|c| FakeDevice.slice_column(&c.tile, &c.site).unwrap().1)
            .collect();
        assert_eq!(
            rows,
            (0..7).collect::<Vec<_>>(),
            "chain rows must be 7 consecutive ascending rows in one column"
        );
        // Distinct slices: one element per row, not stacked.
        let sites: HashSet<&str> = placed.carries.iter().map(|c| c.site.as_str()).collect();
        assert_eq!(sites.len(), 7, "each chain element needs its own slice");
    }

    /// Two independent chains must not be given the same slices.
    #[test]
    fn two_chains_do_not_share_slices() {
        let mut p = packed_blinky();
        p.carries = (0..2)
            .flat_map(|ch| {
                (0..3).map(move |i| {
                    let mut c = crate::pack::PackedCarry::empty(format!("u_c{ch}_{i}"));
                    c.chain = format!("chain{ch}");
                    c.index = i;
                    c.chain_len = 3;
                    c
                })
            })
            .collect();
        let placed = place(&p, &FakeDevice).unwrap();
        let sites: HashSet<&str> = placed.carries.iter().map(|c| c.site.as_str()).collect();
        assert_eq!(sites.len(), 6, "chains must not overlap slices");
        for ch in 0..2 {
            let name = format!("chain{ch}");
            let mut rows: Vec<i32> = placed
                .carries
                .iter()
                .filter(|c| c.name.starts_with(&format!("u_c{ch}_")))
                .map(|c| FakeDevice.slice_column(&c.tile, &c.site).unwrap().1)
                .collect();
            rows.sort_unstable();
            assert!(
                rows.windows(2).all(|w| w[1] == w[0] + 1),
                "{name} is not contiguous: {rows:?}"
            );
        }
    }

    /// A chain longer than any free column must fail loudly, not be scattered.
    #[test]
    fn unplaceable_chain_fails_loudly() {
        let mut p = packed_blinky();
        p.carries = (0..50)
            .map(|i| {
                let mut c = crate::pack::PackedCarry::empty(format!("u_c4_{i}"));
                c.chain = "huge".into();
                c.index = i;
                c.chain_len = 50;
                c
            })
            .collect();
        let err = place(&p, &FakeDevice).unwrap_err();
        assert!(err.contains("huge"), "error must name the chain: {err}");
        assert!(
            err.contains("contiguous"),
            "error must explain the column constraint: {err}"
        );
    }
}
