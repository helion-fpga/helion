//! XC7S25 (Spartan-7, the Arty S7-25's `xc7s25csga324-1`): the part
//! resolves from the Project X-Ray database and loads with the frame count,
//! tile grid, resources and package pins below.
//!
//! Reference values are from openXC7/prjxray-db
//! @0c3504f26f5cf77b6a7605c7bc83ebe1ac67b26f (`spartan7/xc7s25/tilegrid.json`,
//! `spartan7/xc7s25csga324-1/part.json` and `package_pins.csv`); the slice,
//! block RAM, DSP and user I/O counts also match Xilinx DS180 (7 Series
//! Overview, XC7S25: 3,650 slices, 45 36Kb block RAMs, 80 DSP slices, 150 user
//! I/Os in CSGA324).
//!
//! Needs the X-Ray database (`HELION_XILINX_DB`, see `device.rs`); without
//! it the tests say SKIP and pass.

use helion_xilinx7::device::{default_db_root, Device};
use helion_xilinx7::parts;

const PART: &str = "xc7s25csga324-1";

fn part() -> Option<parts::Part> {
    let root = default_db_root();
    if !parts::families(&root).contains(&"spartan7".to_string()) {
        eprintln!(
            "SKIP: no Spartan-7 in the Project X-Ray database at {}",
            root.display()
        );
        return None;
    }
    Some(parts::resolve(&root, PART).unwrap())
}

#[test]
fn xc7s25_resolves() {
    let Some(p) = part() else { return };
    assert_eq!(
        (p.family.as_str(), p.device.as_str(), p.fabric.as_str()),
        ("spartan7", "xc7s25", "xc7s25")
    );
    assert_eq!(
        (p.package.as_str(), p.speedgrade.as_str()),
        ("csga324", "1")
    );
    assert_eq!(p.idcode, 0x037C_4093);
    assert_eq!(parts::board("arty-s7-25").unwrap().part, PART);
}

#[test]
fn xc7s25_device_model() {
    let Some(p) = part() else { return };
    let dev = Device::load_part(&p).unwrap();
    assert_eq!(dev.part().idcode, 0x037C_4093);
    assert_eq!(dev.part().frame_count().unwrap(), 3060);

    // Tile grid: 102 columns by 105 rows, every position filled.
    assert_eq!(dev.tile_count(), 10710);
    let (mut w, mut h) = (0, 0);
    for i in 0..dev.tile_count() {
        let t = dev.tile(i).unwrap();
        w = w.max(t.grid_x + 1);
        h = h.max(t.grid_y + 1);
    }
    assert_eq!((w, h), (102, 105));
    assert!(dev.tile_at(101, 104).is_some());

    assert_eq!(dev.slices().len(), 3650);
    assert_eq!(dev.bram36_sites().len(), 45);
    assert_eq!(dev.bram18_sites().len(), 90);
    assert_eq!(dev.dsp48_sites().len(), 80);
    assert_eq!(dev.bufg_sites().len(), 32);

    // No `required_features.fasm` for any spartan7 part: Vivado bitstreams
    // for the XC7S25 leave the CFG_CENTER_MID bits 26_2206, 26_2207 and
    // 27_2205 clear, so there is nothing to add. (At the pinned database
    // these bits decode as ICAP.IN_USE on spartan7 and artix7; on zynq7 they
    // are the ALWAYS_ON_PROP features.)
    assert!(
        dev.required_features().is_empty(),
        "{:?}",
        dev.required_features()
    );
}

/// 150 user I/O pins, 50 in each of banks 14, 15 and 34, all on IOB sites;
/// spot checks against the Arty S7-25 schematic.
#[test]
fn xc7s25_package_pins() {
    let Some(p) = part() else { return };
    let dev = Device::load_part(&p).unwrap();
    let pins: Vec<_> = dev.package_pins().collect();
    assert_eq!(pins.len(), 150);
    for bank in ["14", "15", "34"] {
        let n = pins
            .iter()
            .filter(|q| q.bank == bank && q.site.starts_with("IOB"))
            .count();
        assert_eq!(n, 50, "bank {bank}");
    }
    let spot = [
        ("F14", "15", "IOB_X0Y74", "LIOB33_X0Y73"), // 12 MHz clock
        ("E18", "15", "IOB_X0Y67", "LIOB33_X0Y67"), // led[0]
        ("R2", "34", "IOB_X1Y26", "RIOB33_X31Y25"), // 100 MHz DDR3L clock
        ("M5", "34", "IOB_X1Y37", "RIOB33_X31Y37"), // sw[3]
    ];
    for (pin, bank, site, tile) in spot {
        let q = dev
            .package_pin(pin)
            .unwrap_or_else(|| panic!("{pin} missing"));
        assert_eq!(
            (q.bank.as_str(), q.site.as_str(), q.tile.as_str()),
            (bank, site, tile),
            "{pin}"
        );
    }
}
