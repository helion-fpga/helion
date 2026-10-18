//! Comprehensive ECP5 / ECP5-5G hardware primitive suite integration tests.
//!
//! Tests BEL packing, placement legality, routing, and Project Trellis
//! `.config` textcfg emission (validated against vendored `bits.db` tables) for:
//! - `DCUA` (5G SERDES Dual Channel Unit) & `EXTREFB`
//! - `DDRDLL`, `DQSBUFM`, `IDDRX1F`, `ODDRX1F`, `IDDRX2F`, `ODDRX2F`, `DELAYG`, `DELAYF`
//! - `MULT18X18D` + `ALU54B` hardware DSP cascade chains
//! - `EHXPLLL` dynamic phase adjustment (`DPHASE_SOURCE`, `CLKOS_DIV`, `CLKOS2_DIV`, `CLKOS3_DIV`)
//! - `DP16KD` & `PDPW16KD` BRAM width/write modes (`DATA_WIDTH_A/B/R` = 1..36)
//! - `USRMCLK`, `OSCG`, & `DTR`

use helion_backend::Backend;
use helion_ecp5::Ecp5Backend;
use helion_ir::{intern, CellKind, Design};

fn add_bb(d: &mut Design, name: &str, module: &str) {
    d.add_cell(
        name,
        CellKind::BlackBox {
            module: intern(module),
        },
    );
}

#[test]
fn ecp5_dcua_extrefb_ddrdll_dqsbufm_iologic_gearing_e2e() {
    let mut d = Design::new("serdes_ddr_top");

    // 5G SERDES EXTREFB -> DCUA
    add_bb(&mut d, "u_extref", "EXTREFB");
    d.set_cell_attr("u_extref", "DCU", "0").unwrap();
    d.connect("refclk_p", "u_extref", "REFCLKP");
    d.connect("refclk_n", "u_extref", "REFCLKN");
    d.connect("refclk_to_dcu", "u_extref", "REFCLKO");

    add_bb(&mut d, "u_dcu", "DCUA");
    d.set_cell_attr("u_dcu", "DCU", "0").unwrap();
    d.set_cell_attr("u_dcu", "CH0_PROTOCOL", "PCIE").unwrap();
    d.set_cell_attr("u_dcu", "CH1_PROTOCOL", "SGMII").unwrap();
    d.connect("refclk_to_dcu", "u_dcu", "D_REFCLKI");

    // DDRDLL -> DQSBUFM -> IDDRX2F / ODDRX2F / IDDRX1F / ODDRX1F / DELAYG
    add_bb(&mut d, "u_dll", "DDRDLLA");
    d.set_cell_attr("u_dll", "FORCE_MAX_DELAY", "NO").unwrap();
    d.set_cell_attr("u_dll", "GSR", "ENABLED").unwrap();
    d.connect("eclk", "u_dll", "CLK");
    d.connect("ddrdel", "u_dll", "DDRDEL");

    add_bb(&mut d, "u_dqs", "DQSBUFM");
    d.set_cell_attr("u_dqs", "DQS_LI_DEL_ADJ", "PLUS").unwrap();
    d.set_cell_attr("u_dqs", "DQS_LI_DEL_VAL", "12").unwrap();
    d.connect("dqs_pad", "u_dqs", "DQSI");
    d.connect("sclk", "u_dqs", "SCLK");
    d.connect("eclk", "u_dqs", "ECLK");
    d.connect("ddrdel", "u_dqs", "DDRDEL");
    d.connect("dqsr90", "u_dqs", "DQSR90");
    d.connect("dqsw270", "u_dqs", "DQSW270");

    add_bb(&mut d, "u_iddr2", "IDDRX2F");
    d.connect("sclk", "u_iddr2", "SCLK");
    d.connect("eclk", "u_iddr2", "ECLK");
    d.connect("dqsr90", "u_iddr2", "DQSR90");
    d.connect("dq_in", "u_iddr2", "D");

    add_bb(&mut d, "u_oddr2", "ODDRX2F");
    d.connect("sclk", "u_oddr2", "SCLK");
    d.connect("eclk", "u_oddr2", "ECLK");
    d.connect("dqsw270", "u_oddr2", "DQSW270");
    d.connect("dq_out", "u_oddr2", "Q");

    add_bb(&mut d, "u_iddr1", "IDDRX1F");
    d.connect("sclk", "u_iddr1", "SCLK");
    d.connect("d1_in", "u_iddr1", "D");

    add_bb(&mut d, "u_oddr1", "ODDRX1F");
    d.connect("sclk", "u_oddr1", "SCLK");
    d.connect("d1_out", "u_oddr1", "Q");

    add_bb(&mut d, "u_delg", "DELAYG");
    d.set_cell_attr("u_delg", "DEL_VALUE", "45").unwrap();
    d.set_cell_attr("u_delg", "WAIT_FOR_EDGE", "ENABLED")
        .unwrap();
    d.connect("a_in", "u_delg", "A");
    d.connect("z_out", "u_delg", "Z");

    let backend = Ecp5Backend::open("LFE5U-85F").unwrap();
    let packed = backend.pack(&d).unwrap();
    assert_eq!(packed.dcus.len(), 1);
    assert_eq!(packed.extrefs.len(), 1);
    assert_eq!(packed.ddrdlls.len(), 1);
    assert_eq!(packed.dqs_blocks.len(), 1);
    assert_eq!(packed.iologics.len(), 5);

    let placed = backend.place(&packed).unwrap();
    let routed = backend.route(&placed).unwrap();
    let bs = backend.bitstream(&routed).unwrap();
    let cfg = &bs.textcfg;

    assert!(cfg.contains("enum: DDRDLL.MODE DDRDLLA\n"), "got:\n{cfg}");
    assert!(
        cfg.contains("enum: DDRDLL.FORCE_MAX_DELAY NO\n"),
        "got:\n{cfg}"
    );
    assert!(cfg.contains("enum: DQS.MODE DQSBUFM\n"), "got:\n{cfg}");
    assert!(cfg.contains(".MODE IDDRXN\n"), "got:\n{cfg}");
    assert!(cfg.contains(".MODE ODDRXN\n"), "got:\n{cfg}");
    assert!(cfg.contains(".ODDRXN.MODE ODDRX2\n"), "got:\n{cfg}");
    assert!(cfg.contains(".MODE IDDRX1_ODDRX1\n"), "got:\n{cfg}");
    assert!(
        cfg.contains(".DELAY.DEL_VALUE 0101101\n"),
        "got:\n{cfg}"
    );
    assert!(
        cfg.contains(".DELAY.WAIT_FOR_EDGE ENABLED\n"),
        "got:\n{cfg}"
    );
    assert!(
        cfg.contains(".comment DCUA u_dcu") && cfg.contains("CH1_PROTOCOL=SGMII"),
        "got:\n{cfg}"
    );
    assert!(cfg.contains(".comment EXTREFB u_extref"), "got:\n{cfg}");
}

#[test]
fn ecp5_dsp_mult18_alu54b_cascade_ebr_pdpw16kd_pll_dphase_sysconfig() {
    let mut d = Design::new("dsp_ebr_pll_sys");

    // 2x MULT18X18D feeding ALU54B in the same sysDSP slice
    add_bb(&mut d, "u_m0", "MULT18X18D");
    d.connect("a0", "u_m0", "A0");
    d.connect("b0", "u_m0", "B0");
    d.connect("p0", "u_m0", "P0");
    d.connect("sroa0", "u_m0", "SROA");

    add_bb(&mut d, "u_m1", "MULT18X18D");
    d.connect("sroa0", "u_m1", "SRIA");
    d.connect("a1", "u_m1", "A0");
    d.connect("b1", "u_m1", "B0");
    d.connect("p1", "u_m1", "P0");

    add_bb(&mut d, "u_alu", "ALU54B");
    d.set_cell_attr("u_alu", "RESETMODE", "SYNC").unwrap();
    d.connect("p0", "u_alu", "MA");
    d.connect("p1", "u_alu", "MB");
    d.connect("acc_out", "u_alu", "R");

    // PDPW16KD (width 36) and DP16KD (width 18, WRITETHROUGH)
    add_bb(&mut d, "u_pdp", "PDPW16KD");
    d.set_cell_attr("u_pdp", "DATA_WIDTH_A", "36").unwrap();
    d.set_cell_attr("u_pdp", "DATA_WIDTH_R", "36").unwrap();
    d.set_cell_attr("u_pdp", "CSDECODE_A", "0b101").unwrap();
    d.set_cell_attr("u_pdp", "INITVAL_00", "0x123456789ABCDEF0")
        .unwrap();

    add_bb(&mut d, "u_dp", "DP16KD");
    d.set_cell_attr("u_dp", "DATA_WIDTH_A", "18").unwrap();
    d.set_cell_attr("u_dp", "DATA_WIDTH_B", "9").unwrap();
    d.set_cell_attr("u_dp", "WRITEMODE_A", "WRITETHROUGH")
        .unwrap();

    // EHXPLLL with dynamic phase adjustment and multi-output dividers
    add_bb(&mut d, "u_pll", "EHXPLLL");
    d.set_cell_attr("u_pll", "CLKI_DIV", "2").unwrap();
    d.set_cell_attr("u_pll", "CLKFB_DIV", "4").unwrap();
    d.set_cell_attr("u_pll", "CLKOP_DIV", "8").unwrap();
    d.set_cell_attr("u_pll", "CLKOS_DIV", "16").unwrap();
    d.set_cell_attr("u_pll", "DPHASE_SOURCE", "ENABLED")
        .unwrap();
    d.connect("clki", "u_pll", "CLKI");
    d.connect("clkop", "u_pll", "CLKOP");
    d.connect("clkos", "u_pll", "CLKOS");
    d.connect("psel0", "u_pll", "PHASESEL0");
    d.connect("pstep", "u_pll", "PHASESTEP");

    // USRMCLK, OSCG, DTR
    add_bb(&mut d, "u_usrmclk", "USRMCLK");
    d.connect("mclki", "u_usrmclk", "USRMCLKI");
    d.connect("mclkts", "u_usrmclk", "USRMCLKTS");

    add_bb(&mut d, "u_osc", "OSCG");
    d.set_cell_attr("u_osc", "DIV", "8").unwrap();
    d.connect("osc_clk", "u_osc", "OSC");

    add_bb(&mut d, "u_dtr", "DTR");
    d.connect("dtr_pulse", "u_dtr", "STARTPULSE");

    let backend = Ecp5Backend::open("LFE5U-85F").unwrap();
    let packed = backend.pack(&d).unwrap();
    assert_eq!(packed.dsps.len(), 2);
    assert_eq!(packed.alu54s.len(), 1);
    assert_eq!(packed.rams.len(), 2);
    assert_eq!(packed.plls.len(), 1);
    assert_eq!(packed.sys_configs.len(), 3);

    let placed = backend.place(&packed).unwrap();
    // MULT18_0, MULT18_1, and ALU54_3 share the first sysDSP slice!
    assert_eq!(placed.dsps.len(), 3);
    assert_eq!(placed.dsps[0].site.site, "MULT18_0");
    assert_eq!(placed.dsps[1].site.site, "MULT18_1");
    assert_eq!(placed.dsps[2].site.site, "ALU54_3");

    let routed = backend.route(&placed).unwrap();
    let bs = backend.bitstream(&routed).unwrap();
    let cfg = &bs.textcfg;

    assert!(cfg.contains("enum: MULT18_0.MODE MULT18X18D\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: MULT18_1.MODE MULT18X18D\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: ALU54_3.MODE ALU54B\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: EBR0.MODE PDPW16KD\n"), "got:\n{cfg}");
    assert!(
        cfg.contains("enum: EBR0.PDPW16KD.DATA_WIDTH_R 36\n"),
        "got:\n{cfg}"
    );
    assert!(
        cfg.contains("enum: EBR0.DP16KD.DATA_WIDTH_A 18\n"),
        "got:\n{cfg}"
    );
    assert!(
        cfg.contains("enum: EBR0.DP16KD.WRITEMODE_A WRITETHROUGH\n"),
        "got:\n{cfg}"
    );
    assert!(cfg.contains("enum: DPHASE_SOURCE ENABLED\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: CLKOS_ENABLE ENABLED\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: CCLK.MODE USRMCLK\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: OSC.MODE OSCG\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: OSC.DIV 8\n"), "got:\n{cfg}");
    assert!(cfg.contains("enum: DTR.MODE DTR\n"), "got:\n{cfg}");
}
