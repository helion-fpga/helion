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
        let iob = iob_sites
            .first()
            .copied()
            .or_else(|| iob_all.first().copied())
            .ok_or_else(|| "need IOB column for LUTFF".to_string())?;
        let mut col: Vec<Site> = dev.clb_sites().filter(|s| s.x == iob.x).collect();
        if col.is_empty() {
            col = dev.clb_sites().collect();
        }
        col.sort_by_key(|s| s.y);
        let n_ble = dev.n_ble.max(1) as usize;
        let base = if opts.timing_weight > 0.0 {
            0usize
        } else {
            col.len() / 2
        };
        for i in 0..packed.lutffs.len() {
            let clb_off = i / n_ble;
            let ble = (i % n_ble) as u8;
            let idx = (base + clb_off).min(col.len() - 1);
            lutff_sites.push((col[idx], ble));
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
