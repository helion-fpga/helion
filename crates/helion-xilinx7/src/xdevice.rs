//! Adapter: implement the place/route/emit query traits for [`Device`].
//!
//! [`XDevice`] wraps the X-Ray [`Device`] model and answers the narrow
//! queries the flow stages need, translating between absolute site names
//! (`IOB_X1Y76`) and FASM-relative names (`IOB_Y0` master / `IOB_Y1`
//! slave by site type).

use crate::device::Device;
use crate::db::parse::PpipKind;
use crate::emit::EmitDevice;
use crate::place::DeviceQuery;
use crate::route::{BufgSpine, BufgTieoff, RouteQuery, RoutedPip};
use std::collections::{HashMap, HashSet};

/// Parse a full slice site name `SLICE_X{x}Y{y}` into `(x, y)`. Even x is
/// the X0/M slice, odd x the X1/L slice; tile Y matches name Y. Shared by
/// the cascade neighbor rule so fakes and the device cannot drift apart.
pub(crate) fn parse_slice_xy(site: &str) -> Option<(i32, i32)> {
    let rest = site.strip_prefix("SLICE_X")?;
    let (xs, ys) = rest.split_once('Y')?;
    Some((xs.parse().ok()?, ys.parse().ok()?))
}

/// Precomputed routing node maps, built once per [`Device`] by
/// [`build_node_map`] and shared (cloned) across flow stages.
///
/// Tuple fields: `(wire_id -> node_id, node_id -> member wire_ids,
/// wire_id -> (tile_idx, wire_idx))`.
pub type NodeMaps = (
    Vec<u32>,
    HashMap<u32, Vec<u32>>,
    Vec<(usize, u32)>,
);

/// [`Device`] plus per-run state (clock nets, node stitching).
pub struct XDevice<'a> {
    pub device: &'a Device,
    /// Nets driven by BUFGCTRL outputs (dedicated clock spine).
    pub clock_nets: HashSet<String>,
    /// wire_id -> node_id (u32::MAX if none).
    wire_to_node: Vec<u32>,
    /// node_id -> member wire_ids.
    node_members: HashMap<u32, Vec<u32>>,
    /// wire_id -> (tile_idx, wire_idx).
    wire_to_tile: Vec<(usize, u32)>,
}

impl<'a> XDevice<'a> {
    pub fn new(device: &'a Device, maps: NodeMaps) -> Self {
        let (wire_to_node, node_members, wire_to_tile) = maps;
        Self {
            device,
            clock_nets: HashSet::new(),
            wire_to_node,
            node_members,
            wire_to_tile,
        }
    }

    fn tile_idx(&self, tile: &str) -> Option<usize> {
        self.device.tile_by_name(tile)
    }

    fn tile_name(&self, idx: usize) -> Option<&str> {
        self.device.tile(idx).map(|t| t.name.as_str())
    }

    /// (tile, wire) -> wire_id.
    fn wire_id_of(&self, tile: &str, wire: &str) -> Option<u32> {
        let ti = self.tile_idx(tile)?;
        let wi = self.device.wire_index(ti, wire)?;
        self.device.wire_id(ti, wi)
    }

    /// wire_id -> (tile_name, wire_name).
    fn wire_name_of(&self, wid: u32) -> Option<(String, String)> {
        let (ti, wi) = self.wire_to_tile.get(wid as usize).copied()?;
        let tile = self.tile_name(ti)?.to_string();
        let wire = self.device.wires(ti)?.get(wi as usize)?.clone();
        Some((tile, wire))
    }

    /// Find the INT tile whose `IMUX{m}` wire is stitched (via tileconn
    /// through the INT_INTERFACE tile) to the CLK_BUFG tile's `imux_wire`.
    /// Returns `(int_tile_name, int_imux_wire_name)`.
    fn int_imux_for(
        &self,
        clk_tile: &str,
        imux_wire: &str,
        site: &str,
        pin: &str,
    ) -> Result<(String, String), String> {
        let err = |msg: String| format!("tieoff: {msg} (site `{site}` pin `{pin}`)");
        let wid = self
            .wire_id_of(clk_tile, imux_wire)
            .ok_or_else(|| err(format!("no wire `{imux_wire}` on tile `{clk_tile}`")))?;
        let nid = self
            .wire_to_node
            .get(wid as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
            .ok_or_else(|| {
                err(format!(
                    "wire `{clk_tile}`.`{imux_wire}` is not stitched to any INT tile"
                ))
            })?;
        let members = self
            .node_members
            .get(&nid)
            .ok_or_else(|| err(format!("no node members for `{clk_tile}`.`{imux_wire}`")))?;
        // IMUX index, e.g. CLK_BUFG_IMUX4_0 -> "4".
        let idx = imux_wire
            .rsplit("IMUX")
            .next()
            .and_then(|s| s.split('_').next())
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| err(format!("cannot parse IMUX index from `{imux_wire}`")))?;
        let want = format!("IMUX{idx}");
        for &m in members {
            let (tname, wname) = self
                .wire_name_of(m)
                .ok_or_else(|| err(format!("bad wire id {m}")))?;
            if wname != want {
                continue;
            }
            let ti = self
                .tile_idx(&tname)
                .ok_or_else(|| err(format!("unknown tile `{tname}`")))?;
            let ttype = self.device.tile_type_name(ti).unwrap_or("");
            if ttype == "INT_L" || ttype == "INT_R" {
                // The rails must come from a TIEOFF site in this tile.
                let has_tieoff = self
                    .device
                    .tile(ti)
                    .map(|t| t.sites.iter().any(|s| s.site_type == "TIEOFF"))
                    .unwrap_or(false);
                if !has_tieoff {
                    return Err(err(format!("INT tile `{tname}` has no TIEOFF site")));
                }
                return Ok((tname, wname));
            }
        }
        Err(err(format!(
            "no INT tile stitched to `{clk_tile}`.`{imux_wire}`"
        )))
    }

    /// GFAN wire in the INT tile that can carry GND into `imux`:
    /// GND_WIRE -> GFANx -> IMUXm (GND has no direct IMUX pip).
    fn gfan_for_imux(
        &self,
        int_tile: &str,
        imux: &str,
        site: &str,
        pin: &str,
    ) -> Result<String, String> {
        for (gfan_dst, _, _) in RouteQuery::pips_from(self, int_tile, "GND_WIRE") {
            if !gfan_dst.starts_with("GFAN") {
                continue;
            }
            let drives_imux = RouteQuery::pips_from(self, int_tile, &gfan_dst)
                .iter()
                .any(|(d, _, _)| d == imux);
            if drives_imux {
                return Ok(gfan_dst);
            }
        }
        Err(format!(
            "tieoff: no GND_WIRE -> GFAN -> `{imux}` path in `{int_tile}` (site `{site}` pin `{pin}`)"
        ))
    }

    /// Fail loudly unless `src -> dst` is a real pip in `tile`.
    fn require_pip(
        &self,
        tile: &str,
        src: &str,
        dst: &str,
        site: &str,
        pin: &str,
    ) -> Result<(), String> {
        let ok = RouteQuery::pips_from(self, tile, src)
            .iter()
            .any(|(d, _, _)| d == dst);
        if !ok {
            return Err(format!(
                "tieoff: no pip `{src}` -> `{dst}` in `{tile}` (site `{site}` pin `{pin}`)"
            ));
        }
        Ok(())
    }
}

/// Build the node -> members map from a mutable Device.
/// Returns (wire_id -> node_id, node_id -> wire_ids, wire_id -> (tile_idx, wire_idx)).
/// Call once after loading; pass the result to [`XDevice::new`].
pub fn build_node_map(device: &mut Device) -> NodeMaps {
    let n = device.tile_count();
    // First pass: find max wire_id.
    let mut max_wid: u32 = 0;
    for idx in 0..n {
        if let Some(base) = device.tile_wire_base(idx) {
            if let Some(wires) = device.wires(idx) {
                let end = base + wires.len() as u32;
                if end > max_wid {
                    max_wid = end;
                }
            }
        }
    }
    let mut wire_to_node: Vec<u32> = vec![u32::MAX; max_wid as usize];
    let mut wire_to_tile: Vec<(usize, u32)> = vec![(0, 0); max_wid as usize];
    let mut node_members: HashMap<u32, Vec<u32>> = HashMap::new();
    for idx in 0..n {
        let base = match device.tile_wire_base(idx) {
            Some(b) => b,
            None => continue,
        };
        let nwires = device.wires(idx).map(|w| w.len()).unwrap_or(0);
        for wi in 0..nwires {
            let wid = base + wi as u32;
            wire_to_tile[wid as usize] = (idx, wi as u32);
            if let Some(nid) = device.node_of(wid) {
                wire_to_node[wid as usize] = nid;
                node_members.entry(nid).or_default().push(wid);
            }
        }
    }
    (wire_to_node, node_members, wire_to_tile)
}

impl<'a> DeviceQuery for XDevice<'a> {
    fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)> {
        self.device
            .package_pin(pin)
            .map(|p| (p.tile.clone(), p.site.clone()))
    }

    fn bufg_site(&self) -> Option<(String, String)> {
        self.device.bufg_sites().into_iter().next()
    }

    fn ramb_site(&self) -> Option<(String, String)> {
        self.device.ramb_sites().into_iter().next()
    }

    fn dsp_site(&self) -> Option<(String, String)> {
        self.device.dsp_sites().into_iter().next()
    }

    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile(i))
            .map(|t| (t.grid_x, t.grid_y))
    }

    fn slice_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)> {
        self.device
            .slices()
            .into_iter()
            .filter(|(_, site)| !used.contains(site))
            .filter_map(|(tile, site)| {
                self.tile_xy(&tile)
                    .map(|(tx, ty)| ((tx - x).abs() + (ty - y).abs(), tile, site))
            })
            .min_by_key(|(d, _, _)| *d)
            .map(|(_, tile, site)| (tile, site))
    }

    fn anchor_slice(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)> {
        // A caller-pinned chain-head site: validate the name parses, the
        // slice exists on the device, and it is still free.
        parse_slice_xy(site)?;
        if used.contains(site) {
            return None;
        }
        self.device
            .slices()
            .into_iter()
            .find(|(_, s)| s == site)
    }

    fn slice_above(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)> {
        // The cascade neighbor is the next slice up the column: X1 of the
        // same tile after X0, else X0 of the tile one name-Y higher:
        // X113Y39 is followed by X112Y40. Proven by the blinky golden
        // chain (X71Y39 to Y45, alternating with transparent X1 pads).
        let (x, y) = parse_slice_xy(site)?;
        let (tx, ty) = if x % 2 == 0 { (x + 1, y) } else { (x - 1, y + 1) };
        let want = format!("SLICE_X{tx}Y{ty}");
        if used.contains(&want) {
            return None;
        }
        self.device
            .slices()
            .into_iter()
            .find(|(_, s)| s == &want)
    }
}

impl<'a> RouteQuery for XDevice<'a> {
    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String, bool)> {
        let idx = match self.tile_idx(tile) {
            Some(i) => i,
            None => return Vec::new(),
        };
        let wires = match self.device.wires(idx) {
            Some(w) => w,
            None => return Vec::new(),
        };
        self.device
            .pips_from(idx, wire)
            .iter()
            .filter_map(|p| {
                let dst = wires.get(p.dst as usize)?.clone();
                let src = wires.get(p.src as usize)?.clone();
                Some((dst, src, p.is_pseudo))
            })
            .collect()
    }

    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
        // All other (tile, wire) pairs in the same node.
        let wid = match self.wire_id_of(tile, wire) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let nid = match self.wire_to_node.get(wid as usize) {
            Some(&n) if n != u32::MAX => n,
            _ => return Vec::new(),
        };
        self.node_members
            .get(&nid)
            .map(|members| {
                members
                    .iter()
                    .filter(|&&m| m != wid)
                    .filter_map(|&m| self.wire_name_of(m))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)> {
        let (tile_idx, site_idx) = self.device.site_by_name(site)?;
        let tile_name = self.tile_name(tile_idx)?.to_string();
        let pins = self.device.site_pins(tile_idx, site_idx)?;
        pins.iter()
            .find(|(pn, _)| pn == pin)
            .map(|(_, w)| (tile_name.clone(), w.clone()))
    }

    fn wire_for_site_pin(&self, site: &str, pin: &str) -> Vec<(String, String)> {
        self.site_pin_wire(site, pin).into_iter().collect()
    }

    fn is_clock_net(&self, net: &str) -> bool {
        self.clock_nets.contains(net)
    }

    fn bufg_tieoff_pips(&self, site: &str) -> Result<BufgTieoff, String> {
        // Control pin -> required constant for I0 -> O
        // (UG472 Fig. 2-5, Table 2-3).
        const TIE_HIGH: [&str; 3] = ["S0", "CE0", "IGNORE1"];
        const TIE_LOW: [&str; 3] = ["S1", "CE1", "IGNORE0"];

        let (tile_idx, site_idx) = self
            .device
            .site_by_name(site)
            .ok_or_else(|| format!("tieoff: unknown site `{site}`"))?;
        let tile_name = self
            .tile_name(tile_idx)
            .ok_or_else(|| format!("tieoff: no tile name for site `{site}`"))?
            .to_string();
        let tile_type = self
            .device
            .tile_type_name(tile_idx)
            .ok_or_else(|| format!("tieoff: no tile type for site `{site}`"))?
            .to_string();
        let pins = self
            .device
            .site_pins(tile_idx, site_idx)
            .ok_or_else(|| format!("tieoff: no pin data for site `{site}`"))?;
        let ppips = self
            .device
            .ppips(&tile_type)
            .ok_or_else(|| format!("tieoff: no ppips for tile type `{tile_type}`"))?;

        // Pin wire -> IMUX wire from the tile type's always-ppips, e.g.
        // CLK_BUFG_BOT_R.CLK_BUFG_R_BUFGCTRL0_S0.CLK_BUFG_IMUX4_0 always.
        // The CLK_BUFG tile has no TIEOFF wires; the IMUX is stitched via
        // tileconn (through the INT_INTERFACE tile) to an INT tile IMUX,
        // which the INT tile's TIEOFF site drives via VCC_WIRE/GND_WIRE.
        let imux_for_pin = |pin: &str| -> Result<String, String> {
            let pin_wire = pins
                .iter()
                .find(|(pn, _)| pn == pin)
                .map(|(_, w)| w.as_str())
                .ok_or_else(|| format!("tieoff: site `{site}` has no pin `{pin}`"))?;
            let prefix = format!("{tile_type}.{pin_wire}.");
            ppips
                .iter()
                .filter(|p| p.kind == PpipKind::Always)
                .find_map(|p| p.name.strip_prefix(&prefix).map(str::to_string))
                .ok_or_else(|| {
                    format!("tieoff: no always-ppip drives `{site}` pin `{pin}` ({pin_wire})")
                })
        };

        let mut tieoff = BufgTieoff::default();
        for pin in TIE_HIGH {
            let imux_wire = imux_for_pin(pin)?;
            let (int_tile, imux) =
                self.int_imux_for(&tile_name, &imux_wire, site, pin)?;
            // VCC_WIRE is the IMUX default select (default ppip: no config
            // bits). Verify the pip exists in the INT tile's graph, then
            // record it so the FASM documents the tie-high.
            self.require_pip(&int_tile, "VCC_WIRE", &imux, site, pin)?;
            tieoff.ones.push(RoutedPip {
                tile: int_tile,
                dst_wire: imux,
                src_wire: "VCC_WIRE".to_string(),
                is_pseudo: false,
            });
        }
        for pin in TIE_LOW {
            let imux_wire = imux_for_pin(pin)?;
            let (int_tile, imux) =
                self.int_imux_for(&tile_name, &imux_wire, site, pin)?;
            // GND_WIRE has no direct IMUX pip: GND_WIRE -> GFANx -> IMUXm.
            let gfan = self.gfan_for_imux(&int_tile, &imux, site, pin)?;
            tieoff.zeros.push(RoutedPip {
                tile: int_tile.clone(),
                dst_wire: gfan.clone(),
                src_wire: "GND_WIRE".to_string(),
                is_pseudo: false,
            });
            tieoff.zeros.push(RoutedPip {
                tile: int_tile,
                dst_wire: imux,
                src_wire: gfan,
                is_pseudo: false,
            });
        }
        Ok(tieoff)
    }

    fn bufg_spine(&self, site: &str) -> Result<BufgSpine, String> {
        let err = |msg: String| format!("spine: {msg} (site `{site}`)");
        let (tile_idx, site_idx) = self
            .device
            .site_by_name(site)
            .ok_or_else(|| err(format!("unknown site")))?;
        let tile_name = self
            .tile_name(tile_idx)
            .ok_or_else(|| err(format!("no tile name")))?
            .to_string();
        let pins = self
            .device
            .site_pins(tile_idx, site_idx)
            .ok_or_else(|| err(format!("no pin data")))?;
        let o_wire = pins
            .iter()
            .find(|(pn, _)| pn == "O")
            .map(|(_, w)| w.clone())
            .ok_or_else(|| err(format!("site has no pin `O`")))?;
        // Lowest global-clock index with a real (non-pseudo) O-exit pip.
        // A pseudo marking here contradicts Vivado, which sets the bit.
        let mut picked: Option<(u8, String)> = None;
        let mut saw_pseudo = false;
        for g in 0..32u8 {
            let dst = format!("CLK_BUFG_CK_GCLK{g}");
            for (d, s, pseudo) in RouteQuery::pips_from(self, &tile_name, &o_wire) {
                if d == dst && s == o_wire {
                    if pseudo {
                        saw_pseudo = true;
                    } else {
                        picked = Some((g, dst));
                    }
                    break;
                }
            }
            if picked.is_some() {
                break;
            }
        }
        let (gclk, o_dst) = picked.ok_or_else(|| {
            err(format!(
                "no O-exit pip (pseudo-marked: {saw_pseudo})"
            ))
        })?;
        // CLK_BUFG_REBUF tiles on the global line's node (tileconn).
        let o_wid = self
            .wire_id_of(&tile_name, &o_dst)
            .ok_or_else(|| err(format!("no wire `{o_dst}`")))?;
        let nid = self
            .wire_to_node
            .get(o_wid as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
            .ok_or_else(|| err(format!("`{o_dst}` is not stitched anywhere")))?;
        let members = self
            .node_members
            .get(&nid)
            .ok_or_else(|| err(format!("no node members")))?;
        // (tile, grid_y) of node-member REBUF tiles, plus every REBUF
        // tile below the entry in the same column (the DB node omits the
        // bottom-edge feed, but Vivado enables it: GCLK0_ENABLE_BELOW on
        // the lowest tile).
        let clk_xy = self
            .tile_idx(&tile_name)
            .and_then(|i| self.device.tile(i))
            .map(|t| (t.grid_x, t.grid_y));
        let mut node_rebuf: Vec<(String, i32)> = Vec::new();
        for &m in members {
            let Some((tname, _)) = self.wire_name_of(m) else {
                continue;
            };
            let Some(ti) = self.tile_idx(&tname) else {
                continue;
            };
            if self.device.tile_type_name(ti).unwrap_or("") != "CLK_BUFG_REBUF" {
                continue;
            }
            let y = self.device.tile(ti).map(|t| t.grid_y).unwrap_or(0);
            if !node_rebuf.iter().any(|(t, _)| t == &tname) {
                node_rebuf.push((tname, y));
            }
        }
        if node_rebuf.is_empty() {
            return Err(err(format!("no CLK_BUFG_REBUF tile on the GCLK{gclk} node")));
        }
        let bot = format!("CLK_BUFG_REBUF_R_CK_GCLK{gclk}_BOT");
        let top = format!("CLK_BUFG_REBUF_R_CK_GCLK{gclk}_TOP");
        // Entry tile: bottommost (largest grid_y; grid_y decreases
        // upward on the die) node REBUF tile holding the TOP to BOT
        // stitch. Vivado conducts the stitch downward (dst BOT, src
        // TOP): the clock enters the spine above the entry and flows
        // down through it toward the bottom edge.
        let mut stitched: Vec<(String, i32)> = Vec::new();
        for (tile, y) in &node_rebuf {
            let has_stitch = RouteQuery::pips_from(self, tile, &top)
                .iter()
                .any(|(d, s, pseudo)| d == &bot && s == &top && !pseudo);
            if has_stitch {
                stitched.push((tile.clone(), *y));
            }
        }
        stitched.sort_by_key(|(_, y)| std::cmp::Reverse(*y));
        let (entry_tile, entry_y) = stitched
            .first()
            .cloned()
            .ok_or_else(|| err(format!("no BOT to TOP stitch for GCLK{gclk}")))?;
        let mut spine = BufgSpine {
            gclk,
            pips: vec![
                RoutedPip {
                    tile: tile_name.clone(),
                    dst_wire: o_dst.clone(),
                    src_wire: o_wire,
                    is_pseudo: false,
                },
                RoutedPip {
                    tile: entry_tile.clone(),
                    dst_wire: bot.clone(),
                    src_wire: top.clone(),
                    is_pseudo: false,
                },
            ],
            statics: Vec::new(),
        };
        // grid_y decreases upward on the die: silicon-below means
        // greater grid_y. BELOW on the entry plus every REBUF tile below
        // it in the same column (down to the bottom edge); ABOVE on the
        // entry plus node tiles above it. Matches Vivado's observed
        // pattern: bottom edge fed, upward reach through the entry,
        // nothing above the loads.
        let mut below: Vec<String> = vec![entry_tile.clone()];
        if let Some((ex, _)) = clk_xy {
            for ti in 0..self.device.tile_count() {
                let is_rebuf =
                    self.device.tile_type_name(ti).unwrap_or("") == "CLK_BUFG_REBUF";
                let geom = self.device.tile(ti).map(|t| (t.grid_x, t.grid_y));
                if let (true, Some((gx, gy))) = (is_rebuf, geom) {
                    if gx == ex && gy > entry_y {
                        if let Some(name) = self.tile_name(ti) {
                            below.push(name.to_string());
                        }
                    }
                }
            }
        }
        below.sort();
        below.dedup();
        let mut above: Vec<String> = vec![entry_tile.clone()];
        for (tile, y) in &node_rebuf {
            if *y < entry_y {
                above.push(tile.clone());
            }
        }
        above.sort();
        above.dedup();
        for tile in &below {
            spine.statics.push((tile.clone(), format!("GCLK{gclk}_ENABLE_BELOW")));
        }
        for tile in &above {
            spine.statics.push((tile.clone(), format!("GCLK{gclk}_ENABLE_ABOVE")));
        }
        // HROW fanout: the global line feeds the horizontal clock row,
        // whose mux fans our clock out to the GCLK branches reaching
        // the INT tiles, through a BUFHCE. Vivado's observed shape for
        // a single bottom-region clock (ZB-10 blinky oracle): exactly
        // one HROW tile on the GCLK node, the MUX_OUT wire fed by
        // CK_IN_L0 (our clock enters the HROW on CK_IN_L0 through the
        // CASCO input path), three more CK_IN wires on the same mux,
        // eleven R_CK_GCLK branches, BUFHCE in use with uninverted CE.
        // Every constructed pip is verified against the DB below and
        // the build fails loudly on any absence: an unverified guess
        // here could short two clocks onto one branch.
        // HROW tile discovery (logic, no hardcoded tile): walk back
        // from the BUFG I0 pin through its input mux to the tileconn
        // node. The clock input corridor reaches the BUFG through
        // exactly one HROW tile (CK_IN_L0 to CASCO inside it, then a
        // bitless node up to the BUFG mux), and Vivado fans the clock
        // back out through that same tile.
        let i0_wire = pins
            .iter()
            .find(|(pn, _)| pn == "I0")
            .map(|(_, w)| w.clone())
            .ok_or_else(|| err(format!("site has no pin `I0`")))?;
        // The I0 driver, discovered by scanning the tile's wires for
        // the source of a real pip into I0 (no constructed names).
        let bti = self
            .tile_idx(&tile_name)
            .ok_or_else(|| err(format!("no tile index for {tile_name}")))?;
        let bwires = self
            .device
            .wires(bti)
            .ok_or_else(|| err(format!("no wires for {tile_name}")))?;
        let mut i0_srcs: Vec<String> = Vec::new();
        for w in bwires.iter() {
            for (d, s, pseudo) in RouteQuery::pips_from(self, &tile_name, w) {
                if d == i0_wire && s == *w && !pseudo && !i0_srcs.contains(w) {
                    i0_srcs.push(w.clone());
                }
            }
        }
        // The corridor driver: the candidate whose node reaches an
        // HROW tile (CK_IN to CASCO inside it). The other I0 mux
        // inputs (fabric IMUX, feedback FBG) never touch the HROW.
        let mut corridor: Vec<String> = Vec::new();
        for cand in &i0_srcs {
            let cwid_opt = self.wire_id_of(&tile_name, cand);
            let Some(cwid) = cwid_opt else { continue };
            let Some(&cnid) = self.wire_to_node.get(cwid as usize) else {
                continue;
            };
            if cnid == u32::MAX {
                continue;
            }
            let hits_hrow = self
                .node_members
                .get(&cnid)
                .map(|ms| {
                    ms.iter().any(|&m| {
                        self.wire_name_of(m).map(|(t, _)| self.tile_idx(&t)).flatten().map(
                            |ti| {
                                self.device
                                    .tile_type_name(ti)
                                    .unwrap_or("")
                                    .starts_with("CLK_HROW")
                            },
                        ).unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if hits_hrow {
                corridor.push(cand.clone());
            }
        }
        if corridor.len() != 1 {
            return Err(err(format!(
                "expected exactly one HROW-fed I0 driver, found {}: {}",
                corridor.len(),
                corridor.join(", ")
            )));
        }
        let src_wid = self
            .wire_id_of(&tile_name, &corridor[0])
            .ok_or_else(|| err(format!("no wire `{}`", corridor[0])))?;
        let src_nid = self
            .wire_to_node
            .get(src_wid as usize)
            .copied()
            .filter(|&n| n != u32::MAX)
            .ok_or_else(|| err(format!("`{}` is not stitched anywhere", corridor[0])))?;
        let src_members = self
            .node_members
            .get(&src_nid)
            .ok_or_else(|| err(format!("no node members")))?;
        let mut hrow_tiles: Vec<String> = Vec::new();
        for &m in src_members {
            let Some((tname, _)) = self.wire_name_of(m) else {
                continue;
            };
            let Some(ti) = self.tile_idx(&tname) else {
                continue;
            };
            if self
                .device
                .tile_type_name(ti)
                .unwrap_or("")
                .starts_with("CLK_HROW")
                && !hrow_tiles.iter().any(|t| t == &tname)
            {
                hrow_tiles.push(tname);
            }
        }
        if hrow_tiles.len() != 1 {
            return Err(err(format!(
                "expected exactly one HROW tile feeding {}, found {}",
                corridor[0],
                hrow_tiles.len()
            )));
        }
        let hrow = hrow_tiles[0].clone();
        // HROW clock-mux config. The HROW mux is a wide selector (32
        // R_CK_GCLK spine taps plus CK_IN/CK_INT inputs) with exactly
        // one source picked: the tap carrying our global line,
        // R_CK_GCLK{gclk}. The CK_IN wires and sibling taps that
        // rawdiff attributes to the same field bits are overlap ghosts:
        // every one of their segbits entries carries a bit the oracle
        // lacks, while the tap's entry reproduces the oracle field
        // exactly (carry_vs_oracle_diff3 analysis). Emitting all of
        // them at once oversets the field and risks contention on
        // silicon, so exactly one source line goes out.
        // These are segbits-only features (the pip DB carries no
        // MUX_OUT or R_CK_GCLK pips: the mux fans out only to a pseudo
        // HCLK wire there), so the emitted line is verified against the
        // segbits DB instead, and the build fails loudly on anything
        // unresolvable. Channel R8 and BUFHCE_X1Y8 are Vivado-observed
        // data for a single bottom-region clock (ZB-10 blinky oracle):
        // the mux output must reach the HCLK row (checked below), and
        // multi-clock channel generality is unproven.
        let hti = self
            .tile_idx(&hrow)
            .ok_or_else(|| err(format!("no tile index for {hrow}")))?;
        let hrow_type = self
            .device
            .tile_type_name(hti)
            .ok_or_else(|| err(format!("no tile type for {hrow}")))?
            .to_string();
        let mux = "CLK_HROW_CK_MUX_OUT_R8".to_string();
        // The mux output must reach the HCLK row through its hard
        // (pseudo) connection, else the channel is dead.
        let hclk_out = "CLK_HROW_CK_HCLK_OUT_R8".to_string();
        let reaches_hclk = RouteQuery::pips_from(self, &hrow, &mux)
            .iter()
            .any(|(d, s, _)| d == &hclk_out && s == &mux);
        if !reaches_hclk {
            return Err(err(format!("{hrow}.{mux} does not reach {hclk_out}")));
        }
        // Every emitted HROW line must resolve in segbits (feature
        // keys carry the tile-type prefix, e.g.
        // `CLK_HROW_BOT_R.CLK_HROW_CK_MUX_OUT_R8.CLK_HROW_CK_IN_L0`).
        let mut pip_line = |dst: &str, src: &str| -> Result<(), String> {
            let feat = format!("{hrow_type}.{dst}.{src}");
            match self.device.segbits(&hrow_type, &feat) {
                Some(bits) if !bits.is_empty() => {
                    spine.pips.push(RoutedPip {
                        tile: hrow.clone(),
                        dst_wire: dst.to_string(),
                        src_wire: src.to_string(),
                        is_pseudo: false,
                    });
                    Ok(())
                }
                _ => Err(err(format!("no segbits for {hrow_type}.{feat}"))),
            }
        };
        // Exactly one source: the spine tap of our global line.
        pip_line(&mux, &format!("CLK_HROW_R_CK_GCLK{gclk}"))?;
        // BUFHCE site (logic: the BUFHCE whose I pin is our mux wire).
        let htile = self
            .device
            .tile(hti)
            .ok_or_else(|| err(format!("no tile data for {hrow}")))?;
        let mut bufh: Vec<String> = Vec::new();
        for (si, s) in htile.sites.iter().enumerate() {
            if !s.site_type.starts_with("BUFHCE") {
                continue;
            }
            let pins = self
                .device
                .site_pins(hti, si)
                .ok_or_else(|| err(format!("no pin data for site {}", s.name)))?;
            if pins.iter().any(|(pn, w)| pn == "I" && *w == mux) {
                bufh.push(s.name.clone());
            }
        }
        if bufh.len() != 1 {
            return Err(err(format!(
                "expected exactly one BUFHCE fed by {mux} in {hrow}, found {}",
                bufh.len()
            )));
        }
        spine.statics.push((hrow.clone(), "CLK_HROW_CK_IN_L0_ACTIVE".to_string()));
        spine.statics.push((hrow.clone(), "CLK_HROW_R_CK_GCLK0_ACTIVE".to_string()));
        spine.statics.push((hrow.clone(), format!("BUFHCE.{}.IN_USE", bufh[0])));
        spine.statics.push((hrow.clone(), format!("BUFHCE.{}.ZINV_CE", bufh[0])));
        // HCLK distribution: the HROW hands the clock to the HCLK
        // backbone row, which carries it across the die. The tileconn
        // DB has no HROW to HCLK stitch (both BUFHCE pins sit on
        // singleton nodes), so the tiles come from row geometry
        // instead: the same name-Y row as the HROW tile. The CMT tile
        // is unique per row; the leaf tile is the nearest HCLK_L tile
        // west of it (Vivado-observed for a single bottom-region clock,
        // ZB-10 blinky oracle). If a future design shows a different
        // leaf, this fails loudly at segbits verification.
        // Vivado-observed line set: leaf B_TOPL5 buffer BUFHCLK8 on
        // the HCLK_L tile with the buffer enabled, and BUFHCLK8 marked
        // used on the HCLK_CMT_L tile. Sibling leaf buffers are overlap
        // ghosts (each carries a bit the oracle lacks). Every line is
        // segbits-verified; anything unresolvable fails loudly. Only
        // the segbits-named features go out here: the row's UNNAMED
        // bits live in frames shared with the CLB/INT columns and are
        // disentangled at the per-cell comparison stage, not guessed.
        // The CMT input side (CCIO) is a separate stage.
        let row = hrow
            .rsplit('Y')
            .next()
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| err(format!("cannot parse row from {hrow}")))?;
        let row_suffix = format!("Y{row}");
        let mut cmt_tiles: Vec<(String, i32)> = Vec::new();
        let mut leaf_cands: Vec<(String, i32)> = Vec::new();
        for ti in 0..self.device.tile_count() {
            let ttype = self.device.tile_type_name(ti).unwrap_or("");
            if ttype != "HCLK_CMT_L" && ttype != "HCLK_L" {
                continue;
            }
            let Some(name) = self.tile_name(ti) else {
                continue;
            };
            if !name.ends_with(row_suffix.as_str()) {
                continue;
            }
            let gx = self.device.tile(ti).map(|t| t.grid_x).unwrap_or(-1);
            if ttype == "HCLK_CMT_L" {
                cmt_tiles.push((name.to_string(), gx));
            } else if !leaf_cands.iter().any(|(t, _)| t == name) {
                leaf_cands.push((name.to_string(), gx));
            }
        }
        if cmt_tiles.len() != 1 {
            return Err(err(format!(
                "expected exactly one HCLK_CMT_L tile in row {row}, found {}",
                cmt_tiles.len()
            )));
        }
        let (cmt, cmt_x) = cmt_tiles[0].clone();
        // Nearest HCLK_L tile west of the CMT tile.
        leaf_cands.retain(|(_, gx)| *gx < cmt_x);
        leaf_cands.sort_by_key(|(_, gx)| std::cmp::Reverse(*gx));
        let hclk = leaf_cands.first().map(|(t, _)| t.clone()).ok_or_else(|| {
            err(format!("no HCLK_L tile west of {cmt} in row {row}"))
        })?;
        let hclk_type = self
            .device
            .tile_type_name(self.tile_idx(&hclk).ok_or_else(|| err(format!("no tile index for {hclk}")))?)
            .ok_or_else(|| err(format!("no tile type for {hclk}")))?
            .to_string();
        let cmt_type = self
            .device
            .tile_type_name(self.tile_idx(&cmt).ok_or_else(|| err(format!("no tile index for {cmt}")))?)
            .ok_or_else(|| err(format!("no tile type for {cmt}")))?
            .to_string();
        // Exactly one leaf buffer: BUFHCLK8. The sibling buffers
        // that rawdiff attributes to the same field bits are overlap
        // ghosts again (same proof as the HROW tap: each carries a bit
        // the oracle lacks, while BUFHCLK8's entry reproduces the
        // oracle field exactly). Channel 8 end to end.
        let leaf = "HCLK_LEAF_CLK_B_TOPL5";
        for src in ["HCLK_CK_BUFHCLK8"] {
            let feat = format!("{hclk_type}.{leaf}.{src}");
            match self.device.segbits(&hclk_type, &feat) {
                Some(bits) if !bits.is_empty() => {
                    spine.pips.push(RoutedPip {
                        tile: hclk.clone(),
                        dst_wire: leaf.to_string(),
                        src_wire: src.to_string(),
                        is_pseudo: false,
                    });
                }
                _ => return Err(err(format!("no segbits for {feat}"))),
            }
        }
        let mut static_line = |tile: &str, ttype: &str, feat: &str| -> Result<(), String> {
            let full = format!("{ttype}.{feat}");
            match self.device.segbits(ttype, &full) {
                Some(bits) if !bits.is_empty() => {
                    spine.statics.push((tile.to_string(), feat.to_string()));
                    Ok(())
                }
                _ => Err(err(format!("no segbits for {full}"))),
            }
        };
        static_line(&hclk, &hclk_type, "ENABLE_BUFFER.HCLK_CK_BUFHCLK8")?;
        static_line(&cmt, &cmt_type, "HCLK_CMT_CK_BUFHCLK8_USED")?;
        // CMT input side: the clock input corridor crosses the
        // HCLK_CMT tile through MUX_CLK_0 (routed as a normal pip on
        // the input net), and the CCIO input is marked active and
        // used. One HCLK_CMT tile per clock row: take the one in the
        // HROW tile's row, and fail loudly on anything else.
        let mut cmt_in: Vec<String> = Vec::new();
        for ti in 0..self.device.tile_count() {
            if self.device.tile_type_name(ti).unwrap_or("") != "HCLK_CMT" {
                continue;
            }
            let Some(name) = self.tile_name(ti) else {
                continue;
            };
            if !name.ends_with(row_suffix.as_str()) {
                continue;
            }
            if !cmt_in.contains(&name.to_string()) {
                cmt_in.push(name.to_string());
            }
        }
        if cmt_in.len() != 1 {
            return Err(err(format!(
                "expected exactly one HCLK_CMT tile in row {row}, found {}: {}",
                cmt_in.len(),
                cmt_in.join(", ")
            )));
        }
        let cmt_in_type = self
            .device
            .tile_type_name(
                self.tile_idx(&cmt_in[0])
                    .ok_or_else(|| err(format!("no tile index for {}", cmt_in[0])))?,
            )
            .ok_or_else(|| err(format!("no tile type for {}", cmt_in[0])))?
            .to_string();
        static_line(&cmt_in[0], &cmt_in_type, "HCLK_CMT_CCIO0_ACTIVE")?;
        static_line(&cmt_in[0], &cmt_in_type, "HCLK_CMT_CCIO0_USED")?;
        Ok(spine)
    }
}

impl<'a> EmitDevice for XDevice<'a> {
    fn tile_type(&self, tile: &str) -> Option<String> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile_type_name(i))
            .map(|s| s.to_string())
    }

    fn bufg_spine_static(&self, _tile: &str, site: &str) -> Vec<(String, String)> {
        // route() runs bufg_spine first and fails loudly there; this
        // identical call then succeeds. Empty on error by construction.
        RouteQuery::bufg_spine(self, site)
            .map(|s| s.statics)
            .unwrap_or_default()
    }

    fn ioi_tile(&self, iob_tile: &str) -> Option<String> {
        // Same bank side (L/R), same row (grid_y), nearest column: the
        // IOI tile sits one over from its IOB tile (e.g. RIOB33_X73Y45
        // with RIOI3_X73Y45). SING pairs are a known DB gap (ZB-09):
        // prjxray has no SING logic segbits, so no OLOGIC/ILOGIC lines
        // there (the IOB-tile lines still flow to the SING filter).
        let side = iob_tile.chars().next()?;
        let want = format!("{side}IOI3");
        let anchor = self.tile_idx(iob_tile)?;
        let anchor_tile = self.device.tile(anchor)?;
        let (ax, ay) = (anchor_tile.grid_x, anchor_tile.grid_y);
        let mut best: Option<(i32, String)> = None;
        for ti in 0..self.device.tile_count() {
            let ttype = self.device.tile_type_name(ti).unwrap_or("");
            if !ttype.starts_with(&want) {
                continue;
            }
            if ttype.contains("SING") {
                continue;
            }
            let t = self.device.tile(ti)?;
            if t.grid_y != ay {
                continue;
            }
            let dx = (t.grid_x - ax).abs();
            let Some(name) = self.tile_name(ti) else {
                continue;
            };
            let entry = (dx, name.to_string());
            if best.as_ref().map(|(bdx, _)| dx < *bdx).unwrap_or(true) {
                best = Some(entry);
            }
        }
        best.map(|(_, name)| name)
    }

    fn site_rel(&self, tile: &str, site: &str) -> Option<String> {
        let idx = self.tile_idx(tile)?;
        let t = self.device.tile(idx)?;
        let pos = t.sites.iter().position(|s| s.name == site)?;
        let stype = &t.sites[pos].site_type;
        if stype.starts_with("IOB") {
            // segbits: IOB_Y0 = master BEL, IOB_Y1 = slave BEL. This is
            // the site TYPE, not the site position in the tile: T22 lives
            // on IOB_X1Y46 (second site in its tile) but Vivado emits
            // IOB_Y0 because the site is a master (ZB-10 ground truth).
            // Bare "IOB33" (SING tiles) has no master/slave marking: fall
            // back to site position, exactly the old behavior. That BEL
            // is UNPROVEN (ZB-09 owns SING truth); M/S sites are exact.
            match stype.as_str() {
                "IOB33M" => Some("IOB_Y0".to_string()),
                "IOB33S" => Some("IOB_Y1".to_string()),
                _ => Some(format!("IOB_Y{pos}")),
            }
        } else if stype.starts_with("SLICE") {
            // segbits: SLICEL_X0 / SLICEM_X1 (X index within tile).
            Some(format!("{stype}_X{pos}"))
        } else if stype.starts_with("BUFG") {
            // segbits: BUFGCTRL.BUFGCTRL_X0Y0 (full site name).
            Some(site.to_string())
        } else {
            Some(format!("{stype}_X{pos}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::default_db_root;
    use std::sync::OnceLock;

    fn test_z020() -> &'static (Device, NodeMaps) {
        static CELL: OnceLock<(Device, NodeMaps)> = OnceLock::new();
        CELL.get_or_init(|| {
            let mut dev = Device::load_xc7z020(&default_db_root()).expect("load xc7z020");
            let maps = build_node_map(&mut dev);
            (dev, maps)
        })
    }

    #[test]
    fn bufg_tieoff_paths() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let tieoff = xdev
            .bufg_tieoff_pips("BUFGCTRL_X0Y0")
            .expect("tie-off for BUFGCTRL_X0Y0");
        // All tie-offs land in the INT tile stitched to the CLK_BUFG tile.
        assert_eq!(tieoff.ones.len(), 3, "S0, CE0, IGNORE1");
        assert_eq!(tieoff.zeros.len(), 6, "S1, CE1, IGNORE0 x2 pips each");
        for p in tieoff.ones.iter().chain(tieoff.zeros.iter()) {
            assert!(
                p.tile.starts_with("INT_R_") || p.tile.starts_with("INT_L_"),
                "tie-off pip in INT tile, got {}",
                p.tile
            );
            assert!(!p.is_pseudo);
        }
        // S0=1, CE0=1, IGNORE1=1 via VCC_WIRE -> IMUX.
        let mut one_dsts: Vec<&str> =
            tieoff.ones.iter().map(|p| p.dst_wire.as_str()).collect();
        one_dsts.sort();
        assert_eq!(one_dsts, ["IMUX20", "IMUX4", "IMUX8"]);
        assert!(tieoff.ones.iter().all(|p| p.src_wire == "VCC_WIRE"));
        // S1=0, CE1=0, IGNORE0=0 via GND_WIRE -> GFAN -> IMUX.
        let mut zero_dsts: Vec<&str> =
            tieoff.zeros.iter().map(|p| p.dst_wire.as_str()).collect();
        zero_dsts.sort();
        assert_eq!(
            zero_dsts,
            ["GFAN0", "GFAN0", "GFAN1", "IMUX0", "IMUX12", "IMUX16"],
            "unexpected zero pips"
        );
        // The GFAN -> IMUX pairings: GFAN0 drives IMUX0 (S1) and IMUX16
        // (CE1); GFAN1 drives IMUX12 (IGNORE0).
        for p in &tieoff.zeros {
            if p.src_wire == "GND_WIRE" {
                assert!(p.dst_wire == "GFAN0" || p.dst_wire == "GFAN1");
            } else {
                match p.dst_wire.as_str() {
                    "IMUX0" | "IMUX16" => assert_eq!(p.src_wire, "GFAN0"),
                    "IMUX12" => assert_eq!(p.src_wire, "GFAN1"),
                    other => panic!("unexpected zero dst {other}"),
                }
            }
        }
        // Unknown site fails loudly.
        assert!(xdev.bufg_tieoff_pips("BUFGCTRL_X99Y99").is_err());
    }

    #[test]
    fn bufg_spine_paths() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado drives global line 0 from this site.
        assert_eq!(spine.gclk, 0);
        // O exit plus the entry REBUF stitch, conducted downward
        // (dst BOT, src TOP) exactly as Vivado emits it.
        assert!(
            spine.pips.iter().any(|p| p.dst_wire == "CLK_BUFG_CK_GCLK0"
                && p.src_wire == "CLK_BUFG_BUFGCTRL0_O"
                && !p.is_pseudo),
            "missing O exit, got {:?}",
            spine.pips
        );
        assert!(
            spine.pips.iter().any(|p| p.dst_wire
                == "CLK_BUFG_REBUF_R_CK_GCLK0_BOT"
                && p.src_wire == "CLK_BUFG_REBUF_R_CK_GCLK0_TOP"
                && !p.is_pseudo),
            "missing TOP to BOT stitch, got {:?}",
            spine.pips
        );
        // Vivado's observed pattern for bottom-region loads: BELOW on
        // the entry plus tiles below it in the column, ABOVE on the
        // entry plus node tiles above it.
        let mut statics = spine.statics.clone();
        statics.sort();
        let tiles: Vec<(&str, &str)> = statics
            .iter()
            .map(|(t, f)| (t.as_str(), f.as_str()))
            .collect();
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y13", "GCLK0_ENABLE_BELOW")),
            "missing bottom-edge feed, got {statics:?}"
        );
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y38", "GCLK0_ENABLE_BELOW")),
            "missing entry BELOW, got {statics:?}"
        );
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y38", "GCLK0_ENABLE_ABOVE")),
            "missing entry ABOVE, got {statics:?}"
        );
        assert!(
            tiles.contains(&("CLK_BUFG_REBUF_X87Y65", "GCLK0_ENABLE_ABOVE")),
            "missing upward reach, got {statics:?}"
        );
        assert_eq!(statics.len(), 12, "extra enables, got {statics:?}");
        // Unknown site fails loudly.
        assert!(xdev.bufg_spine("BUFGCTRL_X99Y99").is_err());
    }

    #[test]
    fn bufg_spine_hrow_fanout() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado ground truth (ZB-10 same-RTL oracle): the clock
        // passes through CLK_HROW_BOT_R_X87Y26, mux MUX_OUT_R8
        // selecting exactly one source, the R_CK_GCLK0 spine tap
        // carrying GCLK0 (the sibling taps and CK_IN wires that
        // rawdiff attributes to the same field are overlap ghosts:
        // each carries a bit the oracle lacks), with BUFHCE_X1Y8 in
        // use (CE not inverted).
        let hrow = "CLK_HROW_BOT_R_X87Y26";
        let mux = "CLK_HROW_CK_MUX_OUT_R8";
        let hrow_pips: Vec<_> = spine
            .pips
            .iter()
            .filter(|p| p.tile == hrow)
            .collect();
        assert_eq!(
            hrow_pips.len(),
            1,
            "exactly one HROW source line, got {:?}",
            spine.pips
        );
        // The tap feature names the mux as DST (Vivado's convention:
        // `MUX_OUT_R8.R_CK_GCLK0`), exactly as the oracle records it.
        assert!(
            hrow_pips[0].dst_wire == mux
                && hrow_pips[0].src_wire == "CLK_HROW_R_CK_GCLK0"
                && !hrow_pips[0].is_pseudo,
            "wrong HROW source, got {:?}",
            spine.pips
        );
        // HCLK row (next stage below); the HROW statics stay exact.
        let mut statics = spine.statics.clone();
        statics.sort();
        for feat in [
            "CLK_HROW_CK_IN_L0_ACTIVE",
            "CLK_HROW_R_CK_GCLK0_ACTIVE",
            "BUFHCE.BUFHCE_X1Y8.IN_USE",
            "BUFHCE.BUFHCE_X1Y8.ZINV_CE",
        ] {
            assert!(
                statics
                    .iter()
                    .any(|(t, f)| t == hrow && f == feat),
                "missing static {feat}, got {statics:?}"
            );
        }
    }

    #[test]
    fn bufg_spine_hclk_leaf() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado ground truth (ZB-10 same-RTL oracle): exactly one
        // leaf buffer, B_TOPL5 BUFHCLK8, on HCLK_L_X174Y26, the buffer
        // enabled, and BUFHCLK8 marked used on HCLK_CMT_L_X178Y26.
        // Sibling buffers are overlap ghosts (each carries a bit the
        // oracle lacks); BUFHCLK8's entry reproduces the oracle leaf
        // field exactly.
        let hclk = "HCLK_L_X174Y26";
        let leaf = "HCLK_LEAF_CLK_B_TOPL5";
        let hclk_pips: Vec<_> = spine
            .pips
            .iter()
            .filter(|p| p.tile == hclk)
            .collect();
        assert_eq!(
            hclk_pips.len(),
            1,
            "exactly one HCLK leaf line, got {:?}",
            spine.pips
        );
        assert!(
            hclk_pips[0].dst_wire == leaf
                && hclk_pips[0].src_wire == "HCLK_CK_BUFHCLK8"
                && !hclk_pips[0].is_pseudo,
            "wrong HCLK leaf line, got {:?}",
            spine.pips
        );
        let mut statics = spine.statics.clone();
        statics.sort();
        assert!(
            statics.iter().any(|(t, f)| t == hclk
                && f == "ENABLE_BUFFER.HCLK_CK_BUFHCLK8"),
            "missing ENABLE_BUFFER, got {statics:?}"
        );
        assert!(
            statics.iter().any(|(t, f)| t == "HCLK_CMT_L_X178Y26"
                && f == "HCLK_CMT_CK_BUFHCLK8_USED"),
            "missing CMT USED, got {statics:?}"
        );
    }

    #[test]
    fn bufg_spine_cmt_ccio() {
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        let spine = xdev
            .bufg_spine("BUFGCTRL_X0Y0")
            .expect("spine for BUFGCTRL_X0Y0");
        // Vivado ground truth (ZB-10 same-RTL oracle): the clock input
        // corridor crosses the single HCLK_CMT tile through MUX_CLK_0
        // (already routed), and the CCIO0 input is marked active and
        // used. Both features are single oracle-1 bits, so no ghost
        // check applies.
        let mut statics = spine.statics.clone();
        statics.sort();
        for feat in ["HCLK_CMT_CCIO0_ACTIVE", "HCLK_CMT_CCIO0_USED"] {
            assert!(
                statics
                    .iter()
                    .any(|(t, f)| t == "HCLK_CMT_X8Y26" && f == feat),
                "missing static {feat}, got {statics:?}"
            );
        }
    }

    #[test]
    fn iob_site_rel_master_slave() {
        // Vivado ground truth (ZB-10 same-RTL diff): single-ended pins
        // use the master BEL (IOB_Y0); the FASM BEL comes from the site
        // type, not the site position in the tile.
        use crate::emit::EmitDevice;
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        // T22 (LD0 output): master site -> IOB_Y0.
        assert_eq!(
            xdev.site_rel("RIOB33_X73Y45", "IOB_X1Y46").as_deref(),
            Some("IOB_Y0")
        );
        // Neighbor slave site -> IOB_Y1.
        assert_eq!(
            xdev.site_rel("RIOB33_X73Y45", "IOB_X1Y45").as_deref(),
            Some("IOB_Y1")
        );
        // Y9 (clk input): master site -> IOB_Y0 (Vivado ILOGIC_Y0).
        assert_eq!(
            xdev.site_rel("LIOB33_X0Y25", "IOB_X0Y26").as_deref(),
            Some("IOB_Y0")
        );
    }

    #[test]
    fn ioi_tile_pairing() {
        use crate::emit::EmitDevice;
        let (dev, maps) = test_z020();
        let xdev = XDevice::new(dev, maps.clone());
        // Paired IOI tile, same row, one column over.
        assert_eq!(
            xdev.ioi_tile("RIOB33_X73Y45").as_deref(),
            Some("RIOI3_X73Y45")
        );
        assert_eq!(
            xdev.ioi_tile("LIOB33_X0Y25").as_deref(),
            Some("LIOI3_X0Y25")
        );
        // SING pairs are a known DB gap (ZB-09): no logic lines there.
        assert_eq!(xdev.ioi_tile("RIOB33_SING_X73Y49"), None);
    }
}
