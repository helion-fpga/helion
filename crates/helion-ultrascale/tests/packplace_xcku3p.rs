//! Pack and place on the real xcku3p device file (GAP-F2/F3).
//!
//! Packs and places blinky and counter netlists on the `DeviceFileGrid`
//! built from the vendored BitFiltrator `xcku3p.json`, checks legality,
//! and renders the placed netlists that feed the router (GAP-F4).
//!
//! Netlist shapes are hand-built post-synth shapes, not synth output:
//! the real synth mapping is GAP-F1's job, and
//! `tests/corpus_legality.rs` already covers the helion-sv
//! synth-derived path on the fake grid. LUT INIT values here are
//! placeholders for the pack/place test, except blinky's inverter INIT
//! (0x5555555555555555), which matches the golden FASM. The CARRY8 in
//! the counter stands for the 4-bit incrementer's carry chain (one
//! CARRY8 covers 8 bits).
//!
//! Provisional, stated plainly: site X is the logical CLB column index
//! from the device file, not a verified Vivado tile X (see
//! PACKPLACE_INVENTORY.md, G-F2a).

use helion_ultrascale::device_file::DeviceFile;
use helion_ultrascale::device_grid::DeviceFileGrid;
use helion_ultrascale::legality::check_legality;
use helion_ultrascale::pack::{ControlSetLimits, pack_ultra};
use helion_ultrascale::place::{UltraPlaceOpts, place_ultra, render_placed_netlist};
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn xcku3p_grid() -> DeviceFileGrid {
    let file =
        DeviceFile::from_path(&data_dir().join("xcku3p.json")).expect("xcku3p.json must load");
    DeviceFileGrid::from_device_file(&file).expect("device-file grid must build")
}

#[test]
fn blinky_places_on_slice_x0y0_like_the_golden_fasm() {
    let grid = xcku3p_grid();
    let limits = ControlSetLimits::default();
    // Blinky: registered inverter, led = q. One LUT, one FF.
    let luts = vec![("lut_q_inv".to_string(), 0x5555_5555_5555_5555u64, false)];
    let ffs = vec![("ff_q".to_string(), "clk".to_string(), None, None)];
    let packed = pack_ultra(&luts, &ffs, &[], &limits).unwrap();
    assert_eq!(packed.stats.lut_count, 1);
    assert_eq!(packed.stats.ff_count, 1);

    let placed = place_ultra(&packed.slices, &grid, &UltraPlaceOpts::default()).unwrap();
    assert_eq!(placed.slices.len(), 1);
    // First-fit in grid order lands on the first Logic site, which is
    // the golden FASM placement.
    assert_eq!(placed.slices[0].site, "SLICE_X0Y0");

    let violations = check_legality(&packed.slices, &placed.slices, &grid, &limits);
    assert!(violations.is_empty(), "violations: {violations:?}");

    let netlist = render_placed_netlist(&packed.slices, &placed.slices);
    assert!(netlist.contains("SLICE_X0Y0"));
    assert!(netlist.contains("lut_q_inv"));
    assert!(netlist.contains("ff_q"));
    println!("{netlist}");
}

#[test]
fn counter_carries_carry8_through_pack_and_place() {
    let grid = xcku3p_grid();
    let limits = ControlSetLimits::default();
    // Counter: 4-bit incrementer. Four sum LUTs, four FFs, one CARRY8.
    // INIT values are placeholders for the pack/place test, not synth
    // output (see the module docs).
    let luts = vec![
        ("lut_sum0".to_string(), 0x5555_5555_5555_5555u64, false),
        ("lut_sum1".to_string(), 0x6666_6666_6666_6666u64, false),
        ("lut_sum2".to_string(), 0x7878_7878_7878_7878u64, false),
        ("lut_sum3".to_string(), 0x7f80_7f80_7f80_7f80u64, false),
    ];
    let ffs = vec![
        ("ff_cnt0".to_string(), "clk".to_string(), None, None),
        ("ff_cnt1".to_string(), "clk".to_string(), None, None),
        ("ff_cnt2".to_string(), "clk".to_string(), None, None),
        ("ff_cnt3".to_string(), "clk".to_string(), None, None),
    ];
    let chains = vec![vec!["carry_cnt".to_string()]];
    let packed = pack_ultra(&luts, &ffs, &chains, &limits).unwrap();
    assert_eq!(packed.stats.lut_count, 4);
    assert_eq!(packed.stats.ff_count, 4);
    assert_eq!(packed.stats.carry8_count, 1);
    // The CARRY8 survives packing: exactly one packed slice holds it.
    let carry_slices: Vec<_> = packed
        .slices
        .iter()
        .filter(|s| s.carry8.as_deref() == Some("carry_cnt"))
        .collect();
    assert_eq!(carry_slices.len(), 1);

    let placed = place_ultra(&packed.slices, &grid, &UltraPlaceOpts::default()).unwrap();
    // The CARRY8 survives placing: exactly one placed slice holds it.
    let carry_placed: Vec<_> = placed
        .slices
        .iter()
        .filter(|p| packed.slices[p.packed_idx].carry8.as_deref() == Some("carry_cnt"))
        .collect();
    assert_eq!(carry_placed.len(), 1);

    let violations = check_legality(&packed.slices, &placed.slices, &grid, &limits);
    assert!(violations.is_empty(), "violations: {violations:?}");

    let netlist = render_placed_netlist(&packed.slices, &placed.slices);
    assert!(netlist.contains("CARRY8 carry_cnt"));
    assert!(netlist.contains("ff_cnt3"));
    println!("{netlist}");
}

#[test]
fn carry_chain_of_four_is_vertically_contiguous_on_device_grid() {
    let grid = xcku3p_grid();
    let limits = ControlSetLimits::default();
    let chains = vec![vec![
        "c0".to_string(),
        "c1".to_string(),
        "c2".to_string(),
        "c3".to_string(),
    ]];
    let packed = pack_ultra(&[], &[], &chains, &limits).unwrap();
    assert_eq!(packed.stats.carry8_count, 4);

    let placed = place_ultra(&packed.slices, &grid, &UltraPlaceOpts::default()).unwrap();
    assert_eq!(placed.slices.len(), 4);
    let mut ordered = placed.slices.clone();
    ordered.sort_by_key(|p| p.y);
    for w in 1..ordered.len() {
        assert_eq!(ordered[w].x, ordered[w - 1].x, "chain X must be constant");
        assert_eq!(
            ordered[w].y,
            ordered[w - 1].y + 1,
            "chain Y must ascend by 1"
        );
    }

    let violations = check_legality(&packed.slices, &placed.slices, &grid, &limits);
    assert!(violations.is_empty(), "violations: {violations:?}");
}

#[test]
fn placement_is_deterministic_for_same_seed_on_device_grid() {
    let grid = xcku3p_grid();
    let limits = ControlSetLimits::default();
    let chains = vec![vec!["c0".to_string(), "c1".to_string(), "c2".to_string()]];
    let luts = vec![
        ("lut0".to_string(), 0xAAAA_AAAA_AAAA_AAAAu64, false),
        ("lut1".to_string(), 0x5555_5555_5555_5555u64, false),
    ];
    let packed = pack_ultra(&luts, &[], &chains, &limits).unwrap();
    let opts = UltraPlaceOpts {
        seed: 42,
        timing_weight: 0.0,
    };
    let first = place_ultra(&packed.slices, &grid, &opts).unwrap();
    let second = place_ultra(&packed.slices, &grid, &opts).unwrap();
    let sites = |p: &helion_ultrascale::place::UltraPlaced| {
        p.slices
            .iter()
            .map(|s| (s.packed_idx, s.site.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(sites(&first), sites(&second));
}
