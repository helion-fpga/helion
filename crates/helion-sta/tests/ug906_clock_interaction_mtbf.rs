use helion_ir::{CellKind, Design, PortDir};
use helion_sta::{
    compute_cdc_mtbf_years, create_clock, load_xdc, report_cdc_mtbf, report_clock_interaction,
    Clock, ClockInteractionMatrix, TechCorner, Ug906ClockPairClass,
};

#[test]
fn ug906_clock_interaction_matrix_classifies_all_five_pair_categories() {
    let xdc_text = r#"
create_clock -period 10.000 -name clk_sys [get_ports clk_sys]
create_clock -period 8.000 -name clk_eth [get_ports clk_eth]
create_clock -period 12.500 -name clk_usb [get_ports clk_usb]
create_clock -period 15.000 -name clk_pcie [get_ports clk_pcie]
set_clock_groups -asynchronous -group [get_clocks clk_sys] -group [get_clocks clk_usb]
set_max_delay -datapath_only 6.000 -from [get_clocks clk_sys] -to [get_clocks clk_pcie]
"#;
    let xdc = load_xdc(xdc_text).expect("valid XDC");
    let mut clocks = xdc.clocks.clone();

    // Add a harmonic generated clock (`clk_sys_div2`, 20ns, master = `clk_sys` -> `Timed`)
    clocks.push(Clock {
        name: "clk_sys_div2".to_string(),
        period_ps: 20_000,
        source: "clk_sys".to_string(),
        generated: true,
        master: Some("clk_sys".to_string()),
        divide_by: 2,
        multiply_by: 1,
        invert: false,
        edges: Vec::new(),
    });

    // Add a non-harmonic fractional generated clock (`clk_sys_frac`, 7ns, master = `clk_sys` -> `Unsafe No Common Period`)
    clocks.push(Clock {
        name: "clk_sys_frac".to_string(),
        period_ps: 7_000,
        source: "clk_sys".to_string(),
        generated: true,
        master: Some("clk_sys".to_string()),
        divide_by: 7,
        multiply_by: 10,
        invert: false,
        edges: Vec::new(),
    });

    let matrix = ClockInteractionMatrix::build(&clocks, &xdc, None);
    assert_eq!(matrix.dim(), 6);

    // 1. Timed: intra-clock and harmonic generated clock sharing root `clk_sys`
    assert_eq!(
        matrix.class_of("clk_sys", "clk_sys"),
        Some(Ug906ClockPairClass::Timed)
    );
    assert_eq!(
        matrix.class_of("clk_sys", "clk_sys_div2"),
        Some(Ug906ClockPairClass::Timed)
    );
    assert_eq!(Ug906ClockPairClass::Timed.as_str(), "Timed");

    // 2. Unsafe No Common Clock: `clk_sys` -> `clk_eth` (unrelated roots, no exception)
    assert_eq!(
        matrix.class_of("clk_sys", "clk_eth"),
        Some(Ug906ClockPairClass::UnsafeNoCommonClock)
    );
    assert_eq!(
        Ug906ClockPairClass::UnsafeNoCommonClock.as_str(),
        "Unsafe No Common Clock"
    );

    // 3. Unsafe No Common Period: `clk_sys` -> `clk_sys_frac` (shared root, non-harmonic 10ns vs 7ns)
    assert_eq!(
        matrix.class_of("clk_sys", "clk_sys_frac"),
        Some(Ug906ClockPairClass::UnsafeNoCommonPeriod)
    );
    assert_eq!(
        Ug906ClockPairClass::UnsafeNoCommonPeriod.as_str(),
        "Unsafe No Common Period"
    );

    // 4. User Ignored Async Groups: `clk_sys` <-> `clk_usb` (`set_clock_groups -asynchronous`)
    assert_eq!(
        matrix.class_of("clk_sys", "clk_usb"),
        Some(Ug906ClockPairClass::UserIgnoredAsyncGroups)
    );
    assert_eq!(
        Ug906ClockPairClass::UserIgnoredAsyncGroups.as_str(),
        "User Ignored Async Groups"
    );

    // 5. Partial False Path: `clk_sys` -> `clk_pcie` (`set_max_delay -datapath_only`)
    assert_eq!(
        matrix.class_of("clk_sys", "clk_pcie"),
        Some(Ug906ClockPairClass::PartialFalsePath)
    );
    assert_eq!(
        Ug906ClockPairClass::PartialFalsePath.as_str(),
        "Partial False Path"
    );

    // Verify `report_clock_interaction(...).to_ug906_matrix(&xdc)` matches `ClockInteractionMatrix::build`
    let base_report = report_clock_interaction(&clocks, &xdc, None);
    assert_eq!(base_report.to_ug906_matrix(&xdc), matrix);
    let txt = matrix.text();
    assert!(txt.contains("CLASS=\"Unsafe No Common Clock\""));
    assert!(txt.contains("CLASS=\"Unsafe No Common Period\""));
    assert!(txt.contains("CLASS=\"User Ignored Async Groups\""));
    assert!(txt.contains("CLASS=\"Partial False Path\""));
}

#[test]
fn cdc_mtbf_years_scales_across_2ff_to_5ff_async_reg_and_tech_corners() {
    // 200 MHz launch (5000 ps), 250 MHz capture (4000 ps)
    let launch_ps = 5_000;
    let capture_ps = 4_000;

    let mtbf_2ff = compute_cdc_mtbf_years(2, launch_ps, capture_ps, true, TechCorner::Typical);
    let mtbf_3ff = compute_cdc_mtbf_years(3, launch_ps, capture_ps, true, TechCorner::Typical);
    let mtbf_4ff = compute_cdc_mtbf_years(4, launch_ps, capture_ps, true, TechCorner::Typical);
    let mtbf_5ff = compute_cdc_mtbf_years(5, launch_ps, capture_ps, true, TechCorner::Typical);

    assert!(mtbf_2ff > 0.0, "2-FF MTBF must be positive");
    assert!(
        mtbf_3ff > mtbf_2ff * 1.0e10,
        "3-FF must improve MTBF by many orders of magnitude over 2-FF: 2FF={mtbf_2ff:e} vs 3FF={mtbf_3ff:e}"
    );
    assert!(
        mtbf_4ff > mtbf_3ff * 1.0e10,
        "4-FF must improve MTBF over 3-FF: 3FF={mtbf_3ff:e} vs 4FF={mtbf_4ff:e}"
    );
    assert!(
        mtbf_5ff > mtbf_4ff * 1.0e10,
        "5-FF must improve MTBF over 4-FF: 4FF={mtbf_4ff:e} vs 5FF={mtbf_5ff:e}"
    );

    // ASYNC_REG = true (intra-SLICE routing) vs ASYNC_REG = false (fabric routing)
    let mtbf_no_async_reg =
        compute_cdc_mtbf_years(2, launch_ps, capture_ps, false, TechCorner::Typical);
    assert!(
        mtbf_2ff > mtbf_no_async_reg * 100.0,
        "ASYNC_REG=TRUE intra-slice placement must yield significantly higher MTBF than unpacked routing"
    );

    // Technology corners: Slow < Typical < Fast, and MultiCornerWorst == Slow
    let slow = compute_cdc_mtbf_years(2, launch_ps, capture_ps, true, TechCorner::Slow);
    let typ = compute_cdc_mtbf_years(2, launch_ps, capture_ps, true, TechCorner::Typical);
    let fast = compute_cdc_mtbf_years(2, launch_ps, capture_ps, true, TechCorner::Fast);
    let worst =
        compute_cdc_mtbf_years(2, launch_ps, capture_ps, true, TechCorner::MultiCornerWorst);

    assert!(slow < typ && typ < fast, "expected Slow ({slow:e}) < Typical ({typ:e}) < Fast ({fast:e})");
    assert_eq!(worst, slow);
}

#[test]
fn report_cdc_mtbf_evaluates_netlist_synchronizer_chains() {
    let mut d = Design::new("cdc_mtbf_netlist");
    d.add_port("clk_src", PortDir::In);
    d.add_port("clk_dst", PortDir::In);

    // Source domain launch FF
    d.add_cell("launch_ff", CellKind::Hff);
    d.connect("clk_src", "launch_ff", "CLK");
    d.connect("din", "launch_ff", "D");
    d.connect("src_q", "launch_ff", "Q");

    // Destination domain 3-FF ASYNC_REG synchronizer chain (connected via passthrough buffer LUTs)
    let mut prev_net = "src_q".to_string();
    for stage in 0..3 {
        let buf = format!("sync_buf{stage}");
        let ff = format!("sync_ff{stage}");
        let d_net = format!("sync_d{stage}");
        let q_net = format!("sync_q{stage}");

        d.add_cell(&buf, CellKind::Lut6 { init: 0xAAAA_AAAA_AAAA_AAAA });
        d.connect(&prev_net, &buf, "I0");
        d.connect(&d_net, &buf, "O");

        d.add_cell(&ff, CellKind::Hff);
        d.set_cell_attr(&ff, "ASYNC_REG", "TRUE").unwrap();
        d.connect("clk_dst", &ff, "CLK");
        d.connect(&d_net, &ff, "D");
        d.connect(&q_net, &ff, "Q");
        prev_net = q_net;
    }

    let mut clocks = Vec::new();
    create_clock(&mut clocks, "clk_src", 8_000, "clk_src");
    create_clock(&mut clocks, "clk_dst", 5_000, "clk_dst");

    let rep = report_cdc_mtbf(&d, &clocks, &Default::default(), TechCorner::Slow);
    assert_eq!(rep.chains.len(), 1);
    assert_eq!(rep.chains[0].stage_count, 3);
    assert!(rep.chains[0].async_reg);
    assert!(rep.chains[0].mtbf_years > 1.0e20);
    assert_eq!(rep.composite_mtbf_years, rep.chains[0].mtbf_years);
}
