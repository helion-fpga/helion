use helion_device::power::{
    ActivityDatabase, ActivitySource, AirflowProfile, BramWriteMode, HeatsinkProfile, NetActivity,
    PowerCellKind, PowerConfidence, PowerNetlist, PowerOptKind, ThermalConditions,
    VectorlessConfig, VoltageRail, analyze_power, lut6_boolean_diff_prob, lut6_propagate_activity,
    lut6_static_prob, parse_saif, parse_vcd_activity, power_opt_design,
    propagate_vectorless_activity,
};
use helion_device::Device;

#[test]
fn saif_and_vcd_edge_cases_and_timescales() {
    // SAIF with 100 ps timescale, PORT + NET sections, and TX/TZ intervals
    let saif = r#"
    (SAIFILE
      (SAIFVERSION "2.0")
      (TIMESCALE 100 ps)
      (DURATION 200)
      (INSTANCE top
        (PORT
          (clk_i (T0 100) (T1 100) (TX 0) (TZ 0) (TC 40))
        )
        (NET
          (tri_bus (T0 50) (T1 50) (TX 50) (TZ 50) (TC 10))
          (all_x   (T0 0)  (T1 0)  (TX 200) (TZ 0) (TC 0))
        )
      )
    )
    "#;
    let db = parse_saif(saif).expect("SAIF should parse");
    // 200 ticks * 0.1 ns = 20 ns duration
    assert!((db.duration_ns - 20.0).abs() < 1e-9);

    let clk = db.lookup("clk_i").unwrap();
    // 40 toggles / 20 ns = 2.0 tr/ns (1 GHz clock)
    assert!((clk.toggle_rate_per_ns - 2.0).abs() < 1e-9);
    assert!((clk.static_prob - 0.5).abs() < 1e-9);

    let tri = db.lookup("tri_bus").unwrap();
    // T1=50 / (50+50+50+50)=200 => P1 = 0.25, TC=10 / 20ns = 0.5 tr/ns
    assert!((tri.static_prob - 0.25).abs() < 1e-9);
    assert!((tri.toggle_rate_per_ns - 0.5).abs() < 1e-9);

    let all_x = db.lookup("all_x").unwrap();
    assert!((all_x.static_prob - 0.5).abs() < 1e-9);
    assert!((all_x.toggle_rate_per_ns - 0.0).abs() < 1e-9);

    // VCD with x/z transitions (x->1 and 1->z do not count as clean 0<->1 toggles)
    let vcd = r#"
    $timescale 100ps $end
    $scope module top $end
    $var wire 1 ! sig $end
    $upscope $end
    $enddefinitions $end
    $dumpvars
    x!
    $end
    #10
    0!
    #30
    1!
    #60
    z!
    #80
    0!
    #100
    1!
    "#;
    let vdb = parse_vcd_activity(vcd).expect("VCD should parse");
    assert!((vdb.duration_ns - 10.0).abs() < 1e-9);
    let sig = vdb.lookup("sig").unwrap();
    // Clean toggles: at #30 (0->1) and at #100 (0->1). #60 (1->z) and #80 (z->0) are x/z transitions.
    assert_eq!(sig.toggle_count, 2);
    // T0 = (#10..#30 = 20) + (#80..#100 = 20) = 40; T1 = (#30..#60 = 30) => P1 = 30 / 70
    assert!((sig.static_prob - (30.0 / 70.0)).abs() < 1e-9);
}

#[test]
fn carry4_and_muxf8_propagation_and_sku_thermal_scaling() {
    let dev_t = Device::load_part("HL10T-C32-1").unwrap();
    let dev_s = Device::load_part("HL10S-C64-1").unwrap();
    let dev_m = Device::load_part("HL10M-C128-1").unwrap();

    let mut nl = PowerNetlist::new();
    nl.primary_inputs = vec![
        "di0".into(),
        "di1".into(),
        "di2".into(),
        "di3".into(),
        "s0".into(),
        "s1".into(),
        "s2".into(),
        "s3".into(),
        "sel7".into(),
        "sel8".into(),
    ];
    nl.primary_outputs = vec!["out8".into()];
    nl.add_clock("clk", "clk", 100.0);

    nl.add_cell(
        "u_carry4",
        PowerCellKind::Carry4 {
            ci: None,
            cyinit: Some("di0".into()),
            di: [
                Some("di0".into()),
                Some("di1".into()),
                Some("di2".into()),
                Some("di3".into()),
            ],
            s: [
                Some("s0".into()),
                Some("s1".into()),
                Some("s2".into()),
                Some("s3".into()),
            ],
            co: [
                Some("co0".into()),
                Some("co1".into()),
                Some("co2".into()),
                Some("co3".into()),
            ],
            o: [
                Some("o0".into()),
                Some("o1".into()),
                Some("o2".into()),
                Some("o3".into()),
            ],
        },
    );
    nl.add_cell(
        "u_muxf7_a",
        PowerCellKind::Muxf7 {
            i0: "o0".into(),
            i1: "o1".into(),
            s: "sel7".into(),
            o: "m7a".into(),
        },
    );
    nl.add_cell(
        "u_muxf7_b",
        PowerCellKind::Muxf7 {
            i0: "o2".into(),
            i1: "o3".into(),
            s: "sel7".into(),
            o: "m7b".into(),
        },
    );
    nl.add_cell(
        "u_muxf8",
        PowerCellKind::Muxf8 {
            i0: "m7a".into(),
            i1: "m7b".into(),
            s: "sel8".into(),
            o: "out8".into(),
        },
    );

    let cfg = VectorlessConfig::default();
    let act = propagate_vectorless_activity(&nl, None, &cfg);
    assert!(act.lookup("co3").unwrap().toggle_rate_per_ns > 0.0);
    assert!(act.lookup("out8").unwrap().toggle_rate_per_ns > 0.0);

    // Larger SKU dies have higher static leakage (more LUTs, BRAMs, DSPs)
    let tc = ThermalConditions::default();
    let rep_t = dev_t.analyze_power(&nl, None, &tc, &cfg);
    let rep_s = dev_s.analyze_power(&nl, None, &tc, &cfg);
    let rep_m = dev_m.analyze_power(&nl, None, &tc, &cfg);

    assert!(rep_t.static_power_mw < rep_s.static_power_mw);
    assert!(rep_s.static_power_mw < rep_m.static_power_mw);
    assert!(rep_m.rail(VoltageRail::Vccint).unwrap().static_mw > rep_t.rail(VoltageRail::Vccint).unwrap().static_mw);
    assert!(rep_m.rail(VoltageRail::Vccbram).unwrap().static_mw > rep_t.rail(VoltageRail::Vccbram).unwrap().static_mw);
}

#[test]
fn power_opt_design_is_idempotent_after_first_pass() {
    let dev = Device::load_part("HL10T-C32-1").unwrap();
    let mut nl = PowerNetlist::new();
    nl.primary_inputs = vec!["d_in".into(), "en".into()];
    nl.primary_outputs = vec!["q_out".into()];
    nl.add_clock("clk", "clk", 250.0);

    // Muxf7 feedback mux: i0 = q_out (when en=0), i1 = d_in (when en=1)
    nl.add_cell(
        "u_mux_fb",
        PowerCellKind::Muxf7 {
            i0: "q_out".into(),
            i1: "d_in".into(),
            s: "en".into(),
            o: "d_fb".into(),
        },
    );
    nl.add_cell(
        "u_ff",
        PowerCellKind::Ff {
            clk: "clk".into(),
            d: "d_fb".into(),
            ce: None,
            sr: None,
            q: "q_out".into(),
            sr_sync: true,
            sr_val: false,
        },
    );

    let mut seed = ActivityDatabase::new();
    seed.insert(NetActivity::new("en", 0.01, 0.10, ActivitySource::Saif));

    let tc = ThermalConditions::default();
    let vc = VectorlessConfig::default();

    let pass1 = dev.power_opt_design(&mut nl, Some(&seed), &tc, &vc);
    assert_eq!(pass1.ffs_clock_gated, 1);
    assert!(matches!(
        &pass1.actions[0].kind,
        PowerOptKind::FfFeedbackClockEnable { ff_cell, ce_net, .. }
        if ff_cell == "u_ff" && ce_net == "en"
    ));
    assert!(pass1.estimated_saved_mw > 0.0);
    assert!(pass1.summary_text().contains("ff_ce_gated=1"));

    // Running power_opt_design a second time on the already-optimized netlist finds 0 new actions
    let pass2 = dev.power_opt_design(&mut nl, Some(&seed), &tc, &vc);
    assert_eq!(pass2.actions.len(), 0);
    assert_eq!(pass2.ffs_clock_gated, 0);
    assert!((pass2.estimated_saved_mw - 0.0).abs() < 1e-9);

    // Verify Shannon cofactor identity: f = (!S & I0) | (S & I1)
    let p = [0.4, 0.7, 0.25];
    let p_mux = lut6_static_prob(0xCA, &p);
    assert!((p_mux - (0.75 * 0.4 + 0.25 * 0.7)).abs() < 1e-12);
    assert!((lut6_boolean_diff_prob(0xCA, 0, &p) - 0.75).abs() < 1e-12);
    assert!((lut6_boolean_diff_prob(0xCA, 1, &p) - 0.25).abs() < 1e-12);
    let (_, d_mux) = lut6_propagate_activity(0xCA, &[(0.4, 0.1), (0.7, 0.2), (0.25, 0.05)]);
    assert!(d_mux > 0.0);
}
