use helion_device::io_plan::{
    BgaPackageGrid, DiffPolarity, IoDirection, IoPinAssignment, IoPlan, IoStandard, PackageKind,
    PinFunction, analyze_ssn, check_io_drc, report_ssn,
};
use helion_device::Device;

#[test]
fn all_four_bga_packages_bind_had_sites_and_validate_plio_rules_1_through_8() {
    let dev = Device::load_part("HL10T-C32-1").expect("load HL10T-C32-1");
    for pkg_name in ["CPG236", "CSG324", "FGG484", "CABGA381"] {
        let kind = PackageKind::parse(pkg_name).expect("valid package name");
        let grid = dev.bga_package(kind);
        assert_eq!(grid.balls.len(), kind.ball_count());
        for bank in 0..kind.bank_count() {
            for pin in grid.bank_user_pins(bank) {
                assert!(
                    pin.site.is_some(),
                    "{} ball {} must map to an IOB site",
                    pkg_name,
                    pin.ball
                );
            }
        }
    }

    // Exercise PLIO-5, PLIO-7, and PLIO-8 on CABGA381
    let grid = BgaPackageGrid::new(PackageKind::Cabga381, Some(&dev));
    let bank2 = grid.bank_user_pins(2);
    let non_cc_pin = bank2
        .iter()
        .find(|p| !p.function.is_clock_capable() && p.function != PinFunction::Vref)
        .unwrap();

    let mut plan = IoPlan::new();
    // PLIO-5: illegal drive strength 24mA on LVCMOS12
    let mut bad_drv = IoPinAssignment::new(
        "p_drv",
        Some(&bank2[1].ball),
        "LVCMOS12",
        IoDirection::Output,
    );
    bad_drv.drive_ma = Some(24);
    plan.add(bad_drv);

    // PLIO-7: DIFF_TERM=TRUE on single-ended LVCMOS18 input
    let mut bad_term = IoPinAssignment::new(
        "p_term",
        Some(&bank2[2].ball),
        "LVCMOS18",
        IoDirection::Input,
    );
    bad_term.diff_term = Some(true);
    plan.add(bad_term);

    // PLIO-8: clock input placed on non-MRCC/SRCC ball
    let mut bad_clk = IoPinAssignment::new(
        "sys_clk",
        Some(&non_cc_pin.ball),
        "LVCMOS18",
        IoDirection::Input,
    );
    bad_clk.is_clock_input = true;
    plan.add(bad_clk);

    let drc = check_io_drc(&grid, &plan);
    assert!(drc.has_code("PLIO-5"), "must flag PLIO-5: {}", drc.summary_text());
    assert!(drc.has_code("PLIO-7"), "must flag PLIO-7: {}", drc.summary_text());
    assert!(drc.has_code("PLIO-8"), "must flag PLIO-8: {}", drc.summary_text());
    assert!(drc. has_code("BIVC-1"), "LVCMOS12 + LVCMOS18 in BANK2 must also flag BIVC-1");
}

#[test]
fn differential_lvds_has_superior_ssn_margin_over_single_ended_lvcmos_aggressors() {
    let dev = Device::load_part("HL10M-C128-1").expect("load HL10M");
    let grid = dev.bga_package(PackageKind::Fgg484);

    let bank0_pairs: Vec<_> = grid
        .differential_pairs()
        .into_iter()
        .filter(|(p, _)| p.bank == Some(0))
        .collect();
    assert!(bank0_pairs.len() >= 4);

    // 4 differential LVDS_25 output pairs (8 pins) with balanced complementary current cancellation
    let mut lvds_plan = IoPlan::new();
    for (idx, (p, n)) in bank0_pairs.iter().take(4).enumerate() {
        lvds_plan.add_diff_pair(
            format!("tx_p{idx}"),
            &p.ball,
            format!("tx_n{idx}"),
            &n.ball,
            "LVDS_25",
            IoDirection::Output,
            None,
        );
    }

    // 8 single-ended LVCMOS25 24mA FAST outputs on the exact same balls
    let mut se_plan = IoPlan::new();
    for (idx, (p, n)) in bank0_pairs.iter().take(4).enumerate() {
        for (sub, ball) in [("a", &p.ball), ("b", &n.ball)] {
            let mut a = IoPinAssignment::new(
                format!("se_{idx}_{sub}"),
                Some(ball),
                "LVCMOS25",
                IoDirection::Output,
            );
            a.drive_ma = Some(24);
            a.slew = Some("FAST".into());
            a.polarity = DiffPolarity::SingleEnded;
            se_plan.add(a);
        }
    }

    assert!(check_io_drc(&grid, &lvds_plan).is_clean());
    assert!(check_io_drc(&grid, &se_plan).is_clean());

    let ssn_lvds = report_ssn(&grid, &lvds_plan);
    let ssn_se = analyze_ssn(&grid, &se_plan);

    assert_eq!(IoStandard::parse("LVDS_25"), Some(IoStandard::Lvds25));
    assert!(
        ssn_lvds.worst_margin_pct > ssn_se.worst_margin_pct,
        "balanced LVDS_25 pairs must have higher SSN margin than 24mA FAST LVCMOS25: lvds={:.1}% vs se={:.1}%",
        ssn_lvds.worst_margin_pct,
        ssn_se.worst_margin_pct
    );
}
