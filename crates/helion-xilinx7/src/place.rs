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
    /// A BUFGCTRL site: `(tile, site)`.
    fn bufg_site(&self) -> Option<(String, String)>;
    /// A RAMB18E1 site: `(tile, site)`.
    fn ramb_site(&self) -> Option<(String, String)>;
    /// Grid coordinates of a tile (for distance estimates).
    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)>;
    /// A free SLICEL site near `(x, y)`; `used` holds taken site names.
    fn slice_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)>;
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

/// Backend-owned placed netlist for xc7a35t.
#[derive(Clone, Debug, Default)]
pub struct Placed7 {
    pub packed: Packed7,
    pub luts: Vec<PlacedLut>,
    pub ffs: Vec<PlacedFf>,
    pub iobs: Vec<PlacedIob>,
    pub bufgs: Vec<PlacedBufg>,
    pub brams: Vec<PlacedBram>,
}

/// BEL letters within a slice: A, B, C, D.
const BEL_LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];

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
    let mut anchor: Option<(i32, i32)> = None;
    for iob in &packed.iobs {
        let pin = iob.loc.as_deref().ok_or_else(|| {
            format!("place: IOB {} (port {}) has no PACKAGE_PIN LOC", iob.name, iob.port)
        })?;
        let (tile, site) = device.iob_site_for_pin(pin).ok_or_else(|| {
            format!("place: no IOB site for package pin {pin} (port {})", iob.port)
        })?;
        if anchor.is_none() {
            anchor = device.tile_xy(&tile);
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
            .bufg_site()
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
            .ramb_site()
            .ok_or_else(|| format!("place: no RAMB18E1 site for {}", bram.name))?;
        used_sites.insert(site.clone());
        out.brams.push(PlacedBram {
            name: bram.name.clone(),
            tile,
            site,
        });
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

    let (ax, ay) = anchor.unwrap_or((0, 0));

    // Allocate a (tile, site, BEL-letter), reusing a slice with free BELs
    // before opening a new one.
    let mut site_tile: HashMap<String, String> = HashMap::new();
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
        // Find driving LUT, if any, and not yet placed.
        let lut_idx = lut_by_output.get(ff.d_net.as_str()).copied().filter(|i| !lut_placed.contains(i));
        let (tile, site, letter) = alloc(&mut used_sites, &mut site_bels, &mut site_tile, device)?;
        site_bels.get_mut(&site).unwrap().insert(letter);
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
        fn bufg_site(&self) -> Option<(String, String)> {
            Some(("CLK_BUFG_X0Y0".into(), "BUFGCTRL_X0Y0".into()))
        }
        fn ramb_site(&self) -> Option<(String, String)> {
            Some(("BRAM_X0Y0".into(), "RAMB18_X0Y0".into()))
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
}
