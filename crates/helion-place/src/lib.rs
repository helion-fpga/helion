//! Place packed clusters onto Helion sites from the device database.

use helion_device::{Device, Site};
use helion_pack::Packed;

#[derive(Clone, Debug)]
pub struct Placed {
    pub packed: Packed,
    pub lutff_sites: Vec<(Site, u8)>,
    pub iob_sites: Vec<Site>,
    pub mac_sites: Vec<Site>,
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

pub fn place(packed: &Packed, dev: &Device) -> Result<Placed, String> {
    place_with(packed, dev, PlaceOpts::default())
}

pub fn place_with(packed: &Packed, dev: &Device, opts: PlaceOpts) -> Result<Placed, String> {
    let mut iob_sites = Vec::new();
    let iob = dev.iob_sites().next();
    if let Some(s) = iob {
        if !packed.iobs.is_empty() {
            iob_sites.push(s);
        }
    } else if !packed.iobs.is_empty() {
        return Err("no IOB".into());
    }

    let mut lutff_sites = Vec::new();
    if !packed.lutffs.is_empty() {
        let iob = iob.ok_or_else(|| "need IOB column for LUTFF".to_string())?;
        let mut col: Vec<Site> = dev.clb_sites().filter(|s| s.x == iob.x).collect();
        if col.is_empty() {
            col = dev.clb_sites().collect();
        }
        col.sort_by_key(|s| s.y);
        let near = col[0];
        let mid = col[col.len() / 2];
        let user = if opts.timing_weight > 0.0 { near } else { mid };
        lutff_sites.push((user, 0));
        for (i, _) in packed.lutffs.iter().enumerate().skip(1) {
            lutff_sites.push((user, i as u8));
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
        timing_weight: opts.timing_weight,
        cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
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
        assert_eq!(pl.mac_sites[0].kind, helion_device::SiteKind::Dsp);
    }
}
