use helion_ir::{
    CellKind, Design, PortDir, extract_fdre_clock_enables, gate_bram_idle_enables,
    hoist_bufgce_clock_gates, intern, optimize_dynamic_power, resolve,
};

fn mux_init_q_i0_d_i1_en_i2() -> u64 {
    let mut init = 0u64;
    for addr in 0..64u64 {
        let q = addr & 1;
        let d = (addr >> 1) & 1;
        let en = (addr >> 2) & 1;
        let out = if en == 1 { d } else { q };
        init |= out << addr;
    }
    init
}

#[test]
fn combined_ce_extraction_and_dont_touch_and_ramb36_blackbox_gating() {
    let mut d = Design::new("top_power_opt");
    d.add_port("clk", PortDir::In);
    d.add_port("en_outer", PortDir::In);
    d.add_port("en_inner", PortDir::In);
    d.add_port("d_in", PortDir::In);

    // 1. Hff that ALREADY has CE = `en_outer`, driven by feedback mux with `en_inner`
    d.add_cell(
        "u_fb_lut",
        CellKind::Lut6 {
            init: mux_init_q_i0_d_i1_en_i2(),
        },
    );
    d.add_cell("u_ff_nested", CellKind::Hff);
    d.connect("q_nested", "u_fb_lut", "I0");
    d.connect("d_in", "u_fb_lut", "I1");
    d.connect("en_inner", "u_fb_lut", "I2");
    d.connect("d_mux", "u_fb_lut", "O");

    d.connect("clk", "u_ff_nested", "CLK");
    d.connect("en_outer", "u_ff_nested", "CE");
    d.connect("d_mux", "u_ff_nested", "D");
    d.connect("q_nested", "u_ff_nested", "Q");

    // 2. DONT_TOUCH Hff must NOT be modified
    d.add_cell(
        "u_fb_dt",
        CellKind::Lut6 {
            init: mux_init_q_i0_d_i1_en_i2(),
        },
    );
    d.add_cell("u_ff_dt", CellKind::Hff);
    d.set_cell_attr("u_ff_dt", "DONT_TOUCH", "1").unwrap();
    d.connect("q_dt", "u_fb_dt", "I0");
    d.connect("d_in", "u_fb_dt", "I1");
    d.connect("en_inner", "u_fb_dt", "I2");
    d.connect("d_dt", "u_fb_dt", "O");
    d.connect("clk", "u_ff_dt", "CLK");
    d.connect("d_dt", "u_ff_dt", "D");
    d.connect("q_dt", "u_ff_dt", "Q");

    // 3. RAMB36E1 BlackBox instance in NO_CHANGE mode with active WEA
    d.add_cell(
        "u_ramb36",
        CellKind::BlackBox {
            module: intern("RAMB36E1"),
        },
    );
    d.set_cell_attr("u_ramb36", "WRITE_MODE_A", "NO_CHANGE")
        .unwrap();
    d.connect("1'b1", "u_ramb36", "ENARDEN");
    d.connect("we_a_sig", "u_ramb36", "WEA[0]");
    d.connect("rd_out_unmasked", "u_ramb36", "DOADO[0]");
    d.add_cell("u_sink_ff", CellKind::Hff);
    d.connect("clk", "u_sink_ff", "CLK");
    d.connect("rd_out_unmasked", "u_sink_ff", "D");

    let rep = optimize_dynamic_power(&mut d, 16);
    assert_eq!(rep.fdre_ce_extracted, 1);
    assert_eq!(rep.dead_luts_pruned, 1);
    assert_eq!(rep.bram_ports_gated, 1);
    assert!(rep.summary_text().contains("fdre_ce=1"));

    // Verify u_ff_nested now has D = d_in and CE driven by a 2-input AND LUT combining en_outer & en_inner
    assert_eq!(
        resolve(d.net_on(intern("u_ff_nested"), intern("D")).unwrap()),
        "d_in"
    );
    let new_ce = resolve(d.net_on(intern("u_ff_nested"), intern("CE")).unwrap());
    assert!(new_ce.contains("ce_comb"));
    assert!(d.cell("u_fb_lut").is_none());

    // Verify DONT_TOUCH FF was untouched
    assert_eq!(
        resolve(d.net_on(intern("u_ff_dt"), intern("D")).unwrap()),
        "d_dt"
    );
    assert!(d.cell("u_fb_dt").is_some());

    // Verify RAMB36E1 ENARDEN was gated by we_a_sig
    assert_eq!(
        resolve(d.net_on(intern("u_ramb36"), intern("ENARDEN")).unwrap()),
        "we_a_sig"
    );

    // Second pass is idempotent
    assert_eq!(extract_fdre_clock_enables(&mut d), 0);
    assert_eq!(hoist_bufgce_clock_gates(&mut d, 16), 0);
    assert_eq!(gate_bram_idle_enables(&mut d), 0);
}
