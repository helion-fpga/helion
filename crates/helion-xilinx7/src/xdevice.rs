//! Adapter: implement the place/route/emit query traits for [`Device`].
//!
//! [`XDevice`] wraps the X-Ray [`Device`] model and answers the narrow
//! queries the flow stages need, translating between absolute site names
//! (`IOB_X1Y76`) and FASM-relative names (`IOB_Y1`).

use crate::device::Device;
use crate::emit::EmitDevice;
use crate::place::DeviceQuery;
use crate::route::RouteQuery;
use std::collections::{HashMap, HashSet};

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
    pub fn new(
        device: &'a Device,
        maps: (Vec<u32>, HashMap<u32, Vec<u32>>, Vec<(usize, u32)>),
    ) -> Self {
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
pub fn build_node_map(
    device: &mut Device,
) -> (Vec<u32>, HashMap<u32, Vec<u32>>, Vec<(usize, u32)>) {
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
}

impl<'a> EmitDevice for XDevice<'a> {
    fn tile_type(&self, tile: &str) -> Option<String> {
        self.tile_idx(tile)
            .and_then(|i| self.device.tile_type_name(i))
            .map(|s| s.to_string())
    }

    fn site_rel(&self, tile: &str, site: &str) -> Option<String> {
        let idx = self.tile_idx(tile)?;
        let t = self.device.tile(idx)?;
        let pos = t.sites.iter().position(|s| s.name == site)?;
        let stype = &t.sites[pos].site_type;
        if stype.starts_with("IOB") {
            Some(format!("IOB_Y{pos}"))
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
