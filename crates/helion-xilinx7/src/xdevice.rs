//! Adapter: implement the place/route/emit query traits for [`Device`].
//!
//! [`XDevice`] wraps the X-Ray [`Device`] model and answers the narrow
//! queries the flow stages need, translating between absolute site names
//! (`IOB_X1Y76`) and FASM-relative names (`IOB_Y1`).

use crate::device::Device;
use crate::emit::EmitDevice;
use crate::place::DeviceQuery;
use crate::route::RouteQuery;
use crate::timing::TimingDevice;
use std::collections::{HashMap, HashSet};

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

    fn bufg_site(&self, used: &HashSet<String>) -> Option<(String, String)> {
        self.device
            .bufg_sites()
            .into_iter()
            .find(|(tile, site)| !used.contains(site) && !used.contains(tile))
    }

    fn ramb_site(&self, used: &HashSet<String>) -> Option<(String, String)> {
        self.device
            .ramb_sites()
            .into_iter()
            .find(|(tile, site)| !used.contains(site) && !used.contains(tile))
    }

    fn dsp_site(&self, used: &HashSet<String>) -> Option<(String, String)> {
        self.device
            .dsp_sites()
            .into_iter()
            .find(|(tile, site)| !used.contains(site) && !used.contains(tile))
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
                <Self as DeviceQuery>::tile_xy(self, &tile)
                    .map(|(tx, ty)| ((tx - x).abs() + (ty - y).abs(), tile, site))
            })
            .min_by_key(|(d, _, _)| *d)
            .map(|(_, tile, site)| (tile, site))
    }
}

impl<'a> TimingDevice for XDevice<'a> {
    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)> {
        // Real grid coordinates from the device model. The numbers embedded
        // in a tile name are NOT grid coordinates (see timing::tile_xy).
        self.tile_idx(tile)
            .and_then(|i| self.device.tile(i))
            .map(|t| (t.grid_x, t.grid_y))
    }
}

impl<'a> RouteQuery for XDevice<'a> {
    fn iob_output_sink(&self, site: &str) -> Vec<(String, String)> {
        // Trace, from the pad backwards, the ONLY path the fabric can use to
        // reach it:
        //
        //   INT_R_X73Y46.IMUX34            <- fabric arrives here
        //     -> RIOI3_X73Y45.IOI_IMUX34_1   (same node, tileconn)
        //     -> IOI_OLOGIC0_D1              (real pip, IOI_IMUX34_1 -> D1)
        //     -> IOI_OLOGIC0_OQ              (pseudo-pip)
        //     -> RIOI_O0                     (pseudo-pip)
        //     -> RIOB33_X73Y45.IOB_O0        (same node, tileconn)
        //
        // So the routable target is the *first* wire, in the neighbouring INT
        // tile -- NOT `site.O` (`IOB_O0`), which sits on the pad side of the
        // OBUF. Driving `IOB_O0` makes the search traverse `IOB_O_OUT0`
        // backwards and then flood the fabric until it chances on some
        // `IMUX34`, which is how LED0 on RIOB33_X73Y45 ended up fed from
        // `INT_R_X73Y45.IMUX34` while its OLOGIC was actually listening on
        // `INT_R_X73Y46.IMUX34` -- a configured, `done=1`, dark pad.
        //
        // Resolve it structurally rather than by hardcoding names: find the
        // IOB's OLOGIC site, take the pip that drives its `_D1` input, and
        // return that wire's node members. OLOGIC `Y<n>` is fed by
        // `IOI_IMUX34_{1-n}` on a full IOI tile, but deriving it keeps the
        // rule honest for tiles whose numbering differs.
        let Some((tile_idx, _site_idx)) = self.device.site_by_name(site) else {
            return Vec::new();
        };
        let Some(iob_tile) = self.tile_name(tile_idx).map(|s| s.to_string()) else {
            return Vec::new();
        };
        let Some(rel) = self.site_rel(&iob_tile, site) else {
            return Vec::new();
        };
        if !rel.starts_with("IOB_Y") {
            return Vec::new();
        }
        let Some(ioi) = self.ioi_tile_for(&iob_tile) else {
            return Vec::new();
        };
        // NOTE the two different spellings, which is the whole trap here:
        // the OLOGIC's *site* (and so its segbits prefix) is `OLOGIC_Y0`,
        // but the *wires* of that same instance are `IOI_OLOGIC0_*`.
        // OLOGIC `n` is fed by `IOI_IMUX34_{1-n}` (verified: on
        // RIOI3_X73Y45, `IOI_IMUX34_1 -> IOI_OLOGIC0_D1` and
        // `IOI_IMUX34_0 -> IOI_OLOGIC1_D1`), so derive it rather than
        // assuming `n`.
        //
        // Query from the IMUX side: `pips_from(tile, w)` returns pips whose
        // *source* is `w`, so asking from `..._D1` yields the OLOGIC's
        // internal fan-out, not its input.
        let ologic_idx: usize = rel
            .trim_start_matches("IOB_Y")
            .parse()
            .unwrap_or(usize::MAX);
        if ologic_idx == usize::MAX {
            return Vec::new();
        }
        let want = format!("IOI_OLOGIC{ologic_idx}_D1");
        let mut out: Vec<(String, String)> = Vec::new();
        for mux in ["IOI_IMUX34_0", "IOI_IMUX34_1"] {
            for (dst, _src, _pseudo) in self.pips_from(&ioi, mux) {
                if dst != want {
                    continue;
                }
                // `mux` is in the same node as the INT tile's IMUX34, which
                // is the wire the router must actually drive.
                out.extend(self.node_wires(&ioi, mux));
            }
        }
        // Deduplicate: a node yields the same set from either end.
        out.sort();
        out.dedup();
        out
    }

    /// Real grid coordinates, so the router's A* heuristic has a distance.
    ///
    /// `RouteQuery::tile_xy` defaults to `None`, and nothing overrode it, so
    /// the heuristic evaluated to 0 for every tile and A* had no direction at
    /// all: it degenerated into flooding the entire fabric. Routing one blinky
    /// net with 4 sinks expanded 4,534,484 distinct nodes and 14,647,731
    /// edges, and the 90-LUT design took 164 s -- over 99% of the whole run.
    ///
    /// Tile *names* are not coordinates (`CLBLM_R_X71Y45` is grid (176, 109)
    /// while `CLBLL_L_X40Y77` is grid (103, 75)), so this asks the device
    /// instead of parsing the name.
    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile(i))
            .map(|t| (t.grid_x, t.grid_y))
    }

    fn tile_type_of(&self, tile: &str) -> Option<String> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile_type_name(i))
            .map(|s| s.to_string())
    }

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
}

impl<'a> EmitDevice for XDevice<'a> {
    fn tile_type(&self, tile: &str) -> Option<String> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile_type_name(i))
            .map(|s| s.to_string())
    }

    fn ioi_tile_for(&self, iob_tile: &str) -> Option<String> {
        let idx = self.tile_idx(iob_tile)?;
        let t = self.device.tile(idx)?;
        // The IOI tile is adjacent in x (right for LIOB, left for RIOB) and
        // shares the IOB's grid row.
        for dx in [-1, 1, -2, 2] {
            let Some(n) = self.device.tile_at(t.grid_x + dx, t.grid_y) else {
                continue;
            };
            let Some(tt) = self.device.tile_type_name(n) else {
                continue;
            };
            if tt.starts_with("LIOI3") || tt.starts_with("RIOI3") {
                return self.tile_name(n).map(|s| s.to_string());
            }
        }
        None
    }


    fn ologic_feature_available(&self, ioi_tile: &str, site: &str, feature: &str) -> bool {
        // The segbits DB is keyed by tile TYPE and the feature name is
        // prefixed with that type, e.g.
        //   type "RIOI3_SING", feature "RIOI3_SING.OLOGIC_Y0.OMUX.D1".
        // Upstream X-Ray ships *_SING / *_TBYTESRC / *_TBYTETERM IOI tiles
        // with ppips only, so this returns false there and the emitter fails
        // loudly instead of producing an unassemblable FASM line.
        let Some(idx) = self.tile_idx(ioi_tile) else {
            return false;
        };
        let Some(tt) = self.device.tile_type_name(idx) else {
            return false;
        };
        let namespaced = format!("{tt}.{site}.{feature}");
        self.device.segbits(tt, &namespaced).is_some()
    }

    fn iob_feature_available(&self, iob_tile: &str, site: &str, feature: &str) -> bool {
        // Same namespacing rule as ologic_feature_available: the segbits DB is
        // keyed by tile TYPE and the feature carries that type as its prefix,
        // e.g. type "RIOB33", feature "RIOB33.IOB_Y1.PULLTYPE.NONE". Tiles
        // whose segbits lack the row (e.g. RIOB33_SING, which upstream X-Ray
        // ships with ppips only) return false so the emitter skips the line
        // instead of writing FASM nothing can resolve.
        let Some(idx) = self.tile_idx(iob_tile) else {
            return false;
        };
        let Some(tt) = self.device.tile_type_name(idx) else {
            return false;
        };
        let namespaced = format!("{tt}.{site}.{feature}");
        self.device.segbits(tt, &namespaced).is_some()
    }

    fn site_rel(&self, tile: &str, site: &str) -> Option<String> {
        let idx = self.tile_idx(tile)?;
        let t = self.device.tile(idx)?;
        let pos = t.sites.iter().position(|s| s.name == site)?;
        let stype = &t.sites[pos].site_type;
        if stype.starts_with("IOB") {
            // X-Ray numbers the IOBs in a tile top-down: `IOB_Y0` is the
            // highest-y site. Tilegrid instance names (and `t.sites`, which
            // is sorted by name) run bottom-up, so reverse the rank. Getting
            // this backwards configures the neighbouring pin: e.g. the
            // ZedBoard 100 MHz clock on Y9 (IOB_X0Y26) must land in IOB_Y0,
            // as it does in a Vivado bitstream.
            let iob_positions: Vec<usize> = t
                .sites
                .iter()
                .enumerate()
                .filter(|(_, s)| s.site_type.starts_with("IOB"))
                .map(|(i, _)| i)
                .collect();
            let rank = iob_positions.iter().position(|&i| i == pos)?;
            let y = iob_positions.len().saturating_sub(1) - rank;
            Some(format!("IOB_Y{y}"))
        } else if stype.starts_with("SLICE") {
            // segbits: SLICEL_X0 / SLICEM_X1 (X index within tile).
            Some(format!("{stype}_X{pos}"))
        } else if stype.starts_with("BUFG") {
            // segbits: BUFGCTRL.BUFGCTRL_X0Y0 (full site name).
            Some(site.to_string())
        } else if stype.starts_with("RAMB") {
            // segbits_bram_{l,r}.db names the sites `RAMB18_Y0` / `RAMB18_Y1`
            // -- keyed by the site's Y index inside the tile, with NO `X`.
            // Deriving it from the site TYPE (as the generic fallback did)
            // produced `RAMB18E1_X1`, which matches no segbits entry at all,
            // so the block was written with no configuration bits and
            // silently held nothing.
            Some(format!("RAMB18_Y{pos}"))
        } else if stype.starts_with("DSP") {
            // segbits_dsp_{l,r}.db uses `DSP48.DSP_<x>`.
            Some(format!("DSP48.DSP_{pos}"))
        } else {
            Some(format!("{stype}_X{pos}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{default_db_root, Device};

    /// `site_rel` must produce names the segbits DB actually defines, for
    /// every site class. The generic `{type}_X{pos}` fallback used to invent
    /// `RAMB18E1_X1` / `DSP48E1_X0`, which resolve to nothing, so those
    /// blocks were emitted with no configuration at all.
    #[test]
    fn site_rel_matches_segbits_names() {
        let db = default_db_root();
        if !db.join("zynq7/xc7z020/tilegrid.json").is_file() {
            return;
        }
        let mut dev = Device::load_xc7z020(&db).expect("load xc7z020");
        let nodes = build_node_map(&mut dev);
        let xdev = XDevice::new(&dev, nodes);

        // Block RAM: segbits_bram_l.db spells the site `RAMB18_Y<n>`.
        let (tile, site) = xdev
            .device
            .ramb_sites()
            .into_iter()
            .next()
            .expect("a RAMB18E1 site");
        let tt = xdev.device.tile_type_name(xdev.tile_idx(&tile).unwrap()).unwrap();
        let rel = xdev.site_rel(&tile, &site).expect("RAMB18 site rel");
        assert!(
            rel.starts_with("RAMB18_Y"),
            "BRAM rel name `{rel}` must match the segbits spelling RAMB18_Y<n>"
        );
        assert!(
            xdev.device
                .segbits(tt, &format!("{tt}.{rel}.IN_USE"))
                .is_some(),
            "segbits must define {tt}.{rel}.IN_USE"
        );

        // DSP: segbits_dsp_l.db uses `DSP48.DSP_<x>`.
        let (dtile, dsite) = xdev
            .device
            .dsp_sites()
            .into_iter()
            .next()
            .expect("a DSP48E1 site");
        let dtt = xdev.device.tile_type_name(xdev.tile_idx(&dtile).unwrap()).unwrap();
        let drel = xdev.site_rel(&dtile, &dsite).expect("DSP site rel");
        assert!(
            drel.starts_with("DSP48.DSP_"),
            "DSP rel name `{drel}` must match the segbits spelling DSP48.DSP_<x>"
        );
        assert!(
            xdev.device
                .segbits(dtt, &format!("{dtt}.{drel}.USE_DPORT[0]"))
                .is_some(),
            "segbits must define {dtt}.{drel}.USE_DPORT[0]"
        );
    }

    #[test]
    fn iob_site_rel_is_top_down() {
        // ZedBoard: clk Y9 = IOB_X0Y26 (upper site of LIOB33_X0Y25),
        // LD0 T22 = IOB_X1Y46 (upper site of RIOB33_X73Y45). X-Ray calls
        // the upper site IOB_Y0, matching Vivado; numbering from the
        // tilegrid's bottom-up list would put the clock on the wrong pin.
        let db = default_db_root();
        if !db.join("zynq7/xc7z020/tilegrid.json").is_file() {
            return;
        }
        let mut dev = Device::load_xc7z020(&db).expect("load xc7z020");
        let nodes = build_node_map(&mut dev);
        let xdev = XDevice::new(&dev, nodes);
        assert_eq!(
            xdev.site_rel("LIOB33_X0Y25", "IOB_X0Y26").as_deref(),
            Some("IOB_Y0")
        );
        assert_eq!(
            xdev.site_rel("LIOB33_X0Y25", "IOB_X0Y25").as_deref(),
            Some("IOB_Y1")
        );
        assert_eq!(
            xdev.site_rel("RIOB33_X73Y45", "IOB_X1Y46").as_deref(),
            Some("IOB_Y0")
        );
        assert_eq!(
            xdev.site_rel("RIOB33_X73Y45", "IOB_X1Y45").as_deref(),
            Some("IOB_Y1")
        );
    }
}
