//! Adapter: implement the place/route/emit query traits for [`Device`].
//!
//! [`XDevice`] wraps the X-Ray [`Device`] model and answers the narrow
//! queries the flow stages need, translating between absolute site names
//! (`IOB_X1Y76`) and FASM-relative names (`IOB_Y1`).

use crate::device::Device;
use crate::emit::EmitDevice;
use crate::place::DeviceQuery;
use crate::route::RouteQuery;
use std::collections::HashSet;

/// [`Device`] plus per-run state (clock nets).
pub struct XDevice<'a> {
    pub device: &'a Device,
    /// Nets driven by BUFGCTRL outputs (dedicated clock spine).
    pub clock_nets: HashSet<String>,
}

impl<'a> XDevice<'a> {
    pub fn new(device: &'a Device) -> Self {
        Self {
            device,
            clock_nets: HashSet::new(),
        }
    }

    fn tile_idx(&self, tile: &str) -> Option<usize> {
        self.device.tile_by_name(tile)
    }

    fn tile_name(&self, idx: usize) -> Option<&str> {
        self.device.tile(idx).map(|t| t.name.as_str())
    }
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
    fn pips_from(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
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
            .filter(|p| !p.is_pseudo)
            .filter_map(|p| {
                let dst = wires.get(p.dst as usize)?.clone();
                let src = wires.get(p.src as usize)?.clone();
                Some((dst, src))
            })
            .collect()
    }

    fn node_wires(&self, tile: &str, wire: &str) -> Vec<(String, String)> {
        // Tileconn stitching: for tonight, return empty (single-tile routes
        // cover the blinky; multi-tile stitching lands with the full router).
        // The BFS still works within a tile and via pips.
        let _ = (tile, wire);
        Vec::new()
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
