//! Clocking (CLK block) tests: public-source encodings resolve exactly,
//! and every clocking site without a public encoding fails loud.
//!
//! Data comes from the vendored repo files under
//! `crates/helion-ultrascale/data/` via `CARGO_MANIFEST_DIR`, never from
//! `/tmp`. Hand-checked bit values below were read off the DB lines in
//! `data/CLK_AUDIT.md`, not from the code under test.

use helion_ultrascale::bitgen::{BitgenDevice, BitgenError, emit_frames};
use helion_ultrascale::clk::{
    BufceLeafConfig, BufgceHdioConfig, ClkError, bufgce_div_site_config, bufgce_hdio_ce_blocked,
    bufgce_site_config, bufce_row_site_config, bufgctrl_site_config, clk_features_to_fasm,
    cmt_hdistr_used_feature, cmt_hroute_used_feature, mmcm_site_config, pll_site_config,
    require_no_clock_manager, resolve_bufce_leaf, resolve_bufgce_hdio, resolve_clk_feature,
    resolve_clk_features, resolve_cmt_feature,
};
use helion_ultrascale::device_file::DeviceFile;
use helion_ultrascale::{BitRef, FileSegbitsDb, SegbitsDb};
use std::path::PathBuf;

fn data_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name)
}

fn load(name: &str) -> FileSegbitsDb {
    FileSegbitsDb::from_file(&data_file(name)).unwrap()
}

/// All clocking segbits DBs merged: CMT plus the fifteen RCLK files.
/// The merge must succeed with no duplicate (tile type, feature) key.
fn load_clk_dbs() -> FileSegbitsDb {
    let names = [
        "segbits_cmt_right.db",
        "segbits_rclk_ams_cfgio.db",
        "segbits_rclk_bram_intf_l.db",
        "segbits_rclk_bram_intf_td_l.db",
        "segbits_rclk_bram_intf_td_r.db",
        "segbits_rclk_clel_l_l.db",
        "segbits_rclk_clem_l.db",
        "segbits_rclk_clem_r.db",
        "segbits_rclk_dsp_intf_clkbuf_l.db",
        "segbits_rclk_dsp_intf_l.db",
        "segbits_rclk_dsp_intf_r.db",
        "segbits_rclk_hdio.db",
        "segbits_rclk_int_l.db",
        "segbits_rclk_int_r.db",
        "segbits_rclk_intf_left_term_alto.db",
        "segbits_rclk_xiphy_outer_right.db",
    ];
    let paths: Vec<PathBuf> = names.iter().map(|n| data_file(n)).collect();
    let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
    FileSegbitsDb::from_files(&refs).unwrap()
}

#[test]
fn all_rclk_dbs_load_with_provenance_counts() {
    for (name, count) in [
        ("segbits_rclk_ams_cfgio.db", 144),
        ("segbits_rclk_bram_intf_l.db", 160),
        ("segbits_rclk_bram_intf_td_l.db", 160),
        ("segbits_rclk_bram_intf_td_r.db", 160),
        ("segbits_rclk_clel_l_l.db", 40),
        ("segbits_rclk_clem_l.db", 40),
        ("segbits_rclk_clem_r.db", 40),
        ("segbits_rclk_dsp_intf_clkbuf_l.db", 288),
        ("segbits_rclk_dsp_intf_l.db", 80),
        ("segbits_rclk_dsp_intf_r.db", 80),
        ("segbits_rclk_hdio.db", 512),
        ("segbits_rclk_int_l.db", 1576),
        ("segbits_rclk_int_r.db", 1576),
        ("segbits_rclk_intf_left_term_alto.db", 456),
        ("segbits_rclk_xiphy_outer_right.db", 48),
    ] {
        assert_eq!(load(name).feature_count(), count, "count for {name}");
    }
    let merged = load_clk_dbs();
    assert_eq!(merged.feature_count(), 5360 + 104, "merged CLK features");
    assert_eq!(
        merged.tile_types(),
        &[
            "CMT_RIGHT".to_string(),
            "RCLK_AMS_CFGIO".to_string(),
            "RCLK_BRAM_INTF_L".to_string(),
            "RCLK_BRAM_INTF_TD_L".to_string(),
            "RCLK_BRAM_INTF_TD_R".to_string(),
            "RCLK_CLEL_L_L".to_string(),
            "RCLK_CLEM_L".to_string(),
            "RCLK_CLEM_R".to_string(),
            "RCLK_DSP_INTF_CLKBUF_L".to_string(),
            "RCLK_DSP_INTF_L".to_string(),
            "RCLK_DSP_INTF_R".to_string(),
            "RCLK_HDIO".to_string(),
            "RCLK_INTF_LEFT_TERM_ALTO".to_string(),
            "RCLK_INT_L".to_string(),
            "RCLK_INT_R".to_string(),
            "RCLK_XIPHY_OUTER_RIGHT".to_string(),
        ]
    );
}

#[test]
fn bufce_leaf_x0y0_hand_checked_bits() {
    let db = load("segbits_rclk_int_l.db");
    // SYNC clears the CE_TYPE bit, CEINV.V0 clears, DELAY_TAP.V2 sets
    // 02_13 only, IINV.V0 clears, IN_USE.V1 sets 00_15. Read off the
    // DB lines in data/CLK_AUDIT.md.
    let config = BufceLeafConfig {
        ce_type_sync: true,
        ce_inverted: false,
        delay_tap: 2,
        input_inverted: false,
        in_use: true,
    };
    let bits = resolve_bufce_leaf(&db, "RCLK_INT_L", "BUFCE_LEAF_X0Y0", &config).unwrap();
    assert_eq!(
        bits,
        vec![
            BitRef::zero(1, 12),
            BitRef::zero(2, 12),
            BitRef::zero(0, 13),
            BitRef::zero(0, 14),
            BitRef::zero(1, 13),
            BitRef::set(2, 13),
            BitRef::zero(3, 13),
            BitRef::zero(0, 12),
            BitRef::set(0, 15),
        ]
    );
    // Opposite config: ASYNC sets 01_12, CEINV.V1 sets 02_12,
    // DELAY_TAP.V8 sets 00_14 only, IINV.V1 sets 00_12, IN_USE.V0 clears.
    let config = BufceLeafConfig {
        ce_type_sync: false,
        ce_inverted: true,
        delay_tap: 8,
        input_inverted: true,
        in_use: false,
    };
    let bits = resolve_bufce_leaf(&db, "RCLK_INT_L", "BUFCE_LEAF_X0Y0", &config).unwrap();
    assert_eq!(
        bits,
        vec![
            BitRef::set(1, 12),
            BitRef::set(2, 12),
            BitRef::zero(0, 13),
            BitRef::set(0, 14),
            BitRef::zero(1, 13),
            BitRef::zero(2, 13),
            BitRef::zero(3, 13),
            BitRef::set(0, 12),
            BitRef::zero(0, 15),
        ]
    );
}

#[test]
fn bufce_leaf_delay_tap_values_are_exactly_0_1_2_4_8() {
    let db = load("segbits_rclk_int_l.db");
    for tap in [0u8, 1, 2, 4, 8] {
        let config = BufceLeafConfig {
            delay_tap: tap,
            ..BufceLeafConfig::default_used()
        };
        assert!(
            resolve_bufce_leaf(&db, "RCLK_INT_L", "BUFCE_LEAF_X0Y0", &config).is_ok(),
            "tap {tap} must resolve"
        );
    }
    for tap in [3u8, 5, 6, 7, 9, 16, 255] {
        let config = BufceLeafConfig {
            delay_tap: tap,
            ..BufceLeafConfig::default_used()
        };
        assert!(
            matches!(
                resolve_bufce_leaf(&db, "RCLK_INT_L", "BUFCE_LEAF_X0Y0", &config),
                Err(ClkError::InvalidValue { .. })
            ),
            "tap {tap} must fail loud, not round"
        );
    }
}

#[test]
fn bufce_leaf_all_32_sites_resolve_in_both_rclk_int_files() {
    for (file, tile_type) in [
        ("segbits_rclk_int_l.db", "RCLK_INT_L"),
        ("segbits_rclk_int_r.db", "RCLK_INT_R"),
    ] {
        let db = load(file);
        for x in 0..8 {
            for y in 0..4 {
                let site = format!("BUFCE_LEAF_X{x}Y{y}");
                let bits = resolve_bufce_leaf(
                    &db,
                    tile_type,
                    &site,
                    &BufceLeafConfig::default_used(),
                )
                .unwrap_or_else(|e| panic!("{tile_type} {site} must resolve: {e}"));
                assert_eq!(bits.len(), 9, "{tile_type} {site} bit count");
            }
        }
    }
}

#[test]
fn bufgce_hdio_iinv_resolves_for_all_four_sites() {
    let db = load("segbits_rclk_hdio.db");
    for (site, bit) in [
        ("BUFGCE_HDIO_X0Y0", 0u16),
        ("BUFGCE_HDIO_X0Y1", 0u16),
        ("BUFGCE_HDIO_X1Y0", 1u16),
        ("BUFGCE_HDIO_X1Y1", 1u16),
    ] {
        let frame = if site.ends_with("Y0") { 5 } else { 4 };
        let set = resolve_bufgce_hdio(
            &db,
            site,
            &BufgceHdioConfig {
                input_inverted: true,
            },
        )
        .unwrap();
        assert_eq!(set, vec![BitRef::set(frame, bit)], "{site} IINV.V1");
        let clear = resolve_bufgce_hdio(
            &db,
            site,
            &BufgceHdioConfig {
                input_inverted: false,
            },
        )
        .unwrap();
        assert_eq!(clear, vec![BitRef::zero(frame, bit)], "{site} IINV.V0");
    }
    // CE inversion has no line for this site: fail loud, named.
    assert!(matches!(
        bufgce_hdio_ce_blocked("BUFGCE_HDIO_X0Y0", "BUFGCE_HDIO.CEINV"),
        ClkError::MissingEncoding { .. }
    ));
}

#[test]
fn cmt_hdistr_and_hroute_used_resolve() {
    let db = load("segbits_cmt_right.db");
    assert_eq!(
        resolve_cmt_feature(&db, "WIRE.CLK_HDISTR_0_0.USED.V1").unwrap(),
        vec![BitRef::set(7, 33)]
    );
    assert_eq!(
        resolve_cmt_feature(&db, "WIRE.CLK_HROUTE_1_0.USED.V1").unwrap(),
        vec![BitRef::set(7, 149)]
    );
    assert_eq!(
        resolve_cmt_feature(&db, "PIP.CLK_HROUTE_1_5.VCC_WIRE").unwrap(),
        vec![BitRef::zero(6, 145)]
    );
    assert_eq!(
        cmt_hdistr_used_feature(0).unwrap(),
        "WIRE.CLK_HDISTR_0_0.USED.V1"
    );
    assert_eq!(
        cmt_hroute_used_feature(23).unwrap(),
        "WIRE.CLK_HROUTE_1_23.USED.V1"
    );
    assert!(matches!(
        cmt_hdistr_used_feature(24),
        Err(ClkError::InvalidValue { .. })
    ));
    assert!(matches!(
        cmt_hroute_used_feature(24),
        Err(ClkError::InvalidValue { .. })
    ));
    // Every CMT feature enumerates exactly what lookup resolves.
    let features = db.features("CMT_RIGHT");
    assert_eq!(features.len(), 104);
    for feature in features {
        assert!(db.lookup("CMT_RIGHT", feature).is_some());
    }
}

#[test]
fn rclk_pip_muxes_resolve_with_complement_bits() {
    let db = load_clk_dbs();
    // HDIO BUFGCE input mux: CLK_INT_TOP sets 05_07, the CMT mux
    // alternative clears the same bit.
    assert_eq!(
        resolve_clk_feature(
            &db,
            "RCLK_HDIO",
            "PIP.CLK_BUFGCE_HDIO_0_CLK_IN.CLK_INT_TOP"
        )
        .unwrap(),
        vec![BitRef::set(5, 7)]
    );
    assert_eq!(
        resolve_clk_feature(
            &db,
            "RCLK_HDIO",
            "PIP.CLK_BUFGCE_HDIO_0_CLK_IN.CLK_CMT_MUX_4TO1_0_CLK_OUT"
        )
        .unwrap(),
        vec![BitRef::zero(5, 7)]
    );
    // RCLK_INT leaf clock mux: 3 set bits, 8 must-be-zero bits.
    let bits = resolve_clk_feature(
        &db,
        "RCLK_INT_L",
        "PIP.CLK_LEAF_SITES_0_CLK_IN.CLK_HDISTR_FT0_0",
    )
    .unwrap();
    assert_eq!(bits.len(), 11);
    assert_eq!(bits.iter().filter(|r| !r.must_be_zero).count(), 3);
    assert_eq!(bits.iter().filter(|r| r.must_be_zero).count(), 8);
    // HDISTR FWD/REV pair: complementary single bits.
    assert_eq!(
        resolve_clk_feature(
            &db,
            "RCLK_DSP_INTF_CLKBUF_L",
            "PIP.CLK_HDISTR_R0.CLK_HDISTR_L0.FWD"
        )
        .unwrap(),
        vec![BitRef::zero(4, 47), BitRef::set(6, 47)]
    );
    // PS-only file is vendored but must resolve under its own tile type.
    assert!(
        resolve_clk_feature(
            &db,
            "RCLK_INTF_LEFT_TERM_ALTO",
            "PIP.CLK_BUFG_PS_0_CLK_IN.PS_TO_PL_CLK0"
        )
        .is_ok()
    );
}

#[test]
fn clk_resolution_partitions_resolved_from_missing() {
    let db = load_clk_dbs();
    let requests = [
        (
            "RCLK_INT_L_X0Y0",
            "RCLK_INT_L",
            "BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC",
        ),
        (
            "CMT_RIGHT_X0Y0",
            "CMT_RIGHT",
            "WIRE.CLK_HDISTR_0_0.USED.V1",
        ),
        ("CMT_L_X0Y0", "CMT_L", "BUFGCE_X0Y0.BUFGCE.IINV.V0"),
    ];
    let out = resolve_clk_features(&db, &requests, "clk_blinky");
    assert_eq!(out.resolved.len(), 2);
    assert_eq!(out.missing.len(), 1);
    assert_eq!(out.missing[0].tile_type, "CMT_L");
    assert_eq!(out.missing[0].feature, "BUFGCE_X0Y0.BUFGCE.IINV.V0");
    let fasm = clk_features_to_fasm(&out.resolved);
    assert!(fasm.contains("RCLK_INT_L_X0Y0.BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC"));
    assert!(fasm.contains("CMT_RIGHT_X0Y0.WIRE.CLK_HDISTR_0_0.USED.V1"));
    assert!(!fasm.contains("BUFGCE"));
}

#[test]
fn bufgce_bufgctrl_div_row_mmcm_pll_all_fail_loud() {
    // Known BUFGCE pip, no encoding.
    assert!(matches!(
        bufgce_site_config("BUFGCE_X0Y0", "CEINV"),
        Err(ClkError::MissingEncoding { .. })
    ));
    assert!(matches!(
        bufgce_site_config("BUFGCE_X0Y0", "IINV"),
        Err(ClkError::MissingEncoding { .. })
    ));
    // Unknown pip name is a different defect from a known pip with no bits.
    assert!(matches!(
        bufgce_site_config("BUFGCE_X0Y0", "FOO"),
        Err(ClkError::UnknownPip { .. })
    ));
    for err in [
        bufgctrl_site_config("BUFGCTRL_X0Y0", "BUFGCTRL.I0INV"),
        bufgce_div_site_config("BUFGCE_DIV_X0Y0", "BUFGCE_DIV.CEINV"),
        bufce_row_site_config("BUFCE_ROW_X0Y0", "BUFCE_ROW.CEINV"),
        mmcm_site_config("MMCM_X0Y0", "MMCME4_ADV.CLKOUT0_DIVIDE"),
        pll_site_config("PLL_X0Y0", "PLLE4_ADV.CLKOUT0_DIVIDE"),
    ] {
        assert!(matches!(err, ClkError::MissingEncoding { .. }));
    }
    assert!(require_no_clock_manager(&[], &[]).is_ok());
    assert!(matches!(
        require_no_clock_manager(&["MMCM_X0Y0"], &[]),
        Err(ClkError::MissingEncoding { .. })
    ));
    assert!(matches!(
        require_no_clock_manager(&[], &["PLL_X0Y0"]),
        Err(ClkError::MissingEncoding { .. })
    ));
}

#[test]
fn clk_blinky_golden_splits_resolved_from_blocked() {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("clk_blinky.fasm"),
    )
    .unwrap();
    let db = load_clk_dbs();
    let mut requests: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !(line.starts_with("CMT_") || line.starts_with("RCLK_")) {
            continue;
        }
        let (tile, feature) = line.split_once('.').unwrap();
        // Tile type is the tile name up to _X<num>: CMT_RIGHT_X0Y0
        // gives CMT_RIGHT, RCLK_INT_L_X0Y0 gives RCLK_INT_L,
        // CMT_L_X0Y0 gives CMT_L.
        let xpos = tile.rfind("_X").unwrap();
        let tile_type = &tile[..xpos];
        requests.push((
            tile.to_string(),
            tile_type.to_string(),
            feature.to_string(),
        ));
    }
    assert_eq!(requests.len(), 9, "CLK lines in the golden");
    let refs: Vec<(&str, &str, &str)> = requests
        .iter()
        .map(|(t, ty, f)| (t.as_str(), ty.as_str(), f.as_str()))
        .collect();
    let out = resolve_clk_features(&db, &refs, "clk_blinky");
    assert_eq!(out.resolved.len(), 8, "encodable CLK lines resolve");
    assert_eq!(out.missing.len(), 1, "exactly one blocked CLK line");
    assert_eq!(out.missing[0].tile_type, "CMT_L");
    assert!(out.missing[0].feature.contains("BUFGCE"));
}

#[test]
fn cmt_and_rclk_tiles_fail_loud_in_emit_frames() {
    // Absolute emission needs a base FAR per CMT/RCLK tile; no public
    // tilegrid gives one, so the device returns None and emit fails
    // loud with UnknownTile instead of inventing a FAR.
    let device = DeviceFile::from_path(&data_file("xcku3p.json")).unwrap();
    let db = load_clk_dbs();
    for fasm in [
        "CMT_RIGHT_X0Y0.WIRE.CLK_HDISTR_0_0.USED.V1\n",
        "RCLK_INT_L_X0Y0.BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC\n",
    ] {
        assert!(matches!(
            emit_frames(fasm, &device, &db),
            Err(BitgenError::UnknownTile { .. })
        ));
    }
    // The device itself confirms it: no tile type, no base FAR.
    assert!(device.tile_type("CMT_RIGHT_X0Y0").is_none());
    assert!(device.tile_base_far("CMT_RIGHT_X0Y0").is_none());
    assert!(device.tile_type("RCLK_INT_L_X0Y0").is_none());
    assert!(device.tile_base_far("RCLK_INT_L_X0Y0").is_none());
}
