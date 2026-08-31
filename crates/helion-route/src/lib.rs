//! Route placed nets: LUTFF Q to IOB (same column, any Y).

use helion_device::Device;
use helion_place::Placed;

#[derive(Clone, Debug)]
pub struct Routed {
    pub placed: Placed,
    /// IOB (x,y) driven by CLB (cx,cy) BLE
    pub iob_src: Vec<IobRoute>,
}

#[derive(Clone, Copy, Debug)]
pub struct IobRoute {
    pub iob: (u32, u32),
    pub clb: (u32, u32),
    pub ble: u8,
}

pub fn route(placed: &Placed, _dev: &Device) -> Result<Routed, String> {
    if placed.lutff_sites.is_empty() {
        return Ok(Routed {
            placed: placed.clone(),
            iob_src: vec![],
        });
    }
    if placed.iob_sites.is_empty() {
        return Err("no IOB to route".into());
    }
    let (clb, ble) = placed.lutff_sites[0];
    let iob = placed.iob_sites[0];
    if iob.x != clb.x {
        return Err(format!(
            "router requires same column, CLB x={} IOB x={}",
            clb.x, iob.x
        ));
    }
    if clb.y <= iob.y {
        return Err("CLB must be north of IOB".into());
    }
    Ok(Routed {
        placed: placed.clone(),
        iob_src: vec![IobRoute {
            iob: (iob.x, iob.y),
            clb: (clb.x, clb.y),
            ble,
        }],
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
    }

    #[test]
    fn routes_wl_mid_column() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        assert!(r.iob_src[0].clb.1 > r.iob_src[0].iob.1);
    }
}
