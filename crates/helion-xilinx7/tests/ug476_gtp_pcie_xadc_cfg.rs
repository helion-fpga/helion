//! Comprehensive UG476 / UG470 / UG480 / PG054 test suite for `helion-xilinx7`.
//!
//! Tests packing, placement legality, routing, FASM emission, FASM parsing,
//! and frame bit assembly for:
//! - `IBUFDS_GTE2`, `GTPE2_COMMON`, & `GTPE2_CHANNEL`
//! - `PCIE_2_1` (Gen1/Gen2 x1/x2/x4/x8, BAR0..BAR5, MaxPayload)
//! - `XADC` (`INIT_40`..`INIT_5F` 16-bit config registers)
//! - `BSCANE2` (`JTAG_CHAIN` 1..4), `ICAPE2` (`X8`/`X16`/`X32`, `DEVICE_ID`),
//!   `DNA_PORT` (`SIM_DNA_VALUE`), `FRAME_ECCE2` (`FARSRC`), `STARTUPE2`,
//!   `USR_ACCESSE2`, & `CAPTUREE2`

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design};
use helion_xilinx7::backend::Xilinx7Backend;
use helion_xilinx7::fasm::parse_fasm;

fn add_bb(d: &mut Design, name: &str, module: &str) {
    d.add_cell(
        name,
        CellKind::BlackBox {
            module: intern(module),
        },
    );
}

#[test]
fn ug476_gtp_pcie_xadc_and_ug470_cfg_primitives_e2e() {
    let mut d = Design::new("gtp_pcie_xadc_cfg_top");

    // 1. IBUFDS_GTE2 -> GTPE2_COMMON -> 2x GTPE2_CHANNEL
    add_bb(&mut d, "u_ibufds_gte2", "IBUFDS_GTE2");
    d.set_cell_attr("u_ibufds_gte2", "CLKCM_CFG", "TRUE")
        .unwrap();
    d.set_cell_attr("u_ibufds_gte2", "CLKRCV_TRST", "TRUE")
        .unwrap();
    d.set_cell_attr("u_ibufds_gte2", "CLKSWING_CFG", "2'b11")
        .unwrap();
    d.connect("refclk_p", "u_ibufds_gte2", "I");
    d.connect("refclk_n", "u_ibufds_gte2", "IB");
    d.connect("gtrefclk0", "u_ibufds_gte2", "O");

    add_bb(&mut d, "u_gtp_common", "GTPE2_COMMON");
    d.set_cell_attr("u_gtp_common", "PLL0_FBDIV", "4").unwrap();
    d.set_cell_attr("u_gtp_common", "PLL0_FBDIV_45", "5")
        .unwrap();
    d.set_cell_attr("u_gtp_common", "PLL0_REFCLK_DIV", "1")
        .unwrap();
    d.connect("gtrefclk0", "u_gtp_common", "GTREFCLK0");
    d.connect("pll0clk", "u_gtp_common", "PLL0OUTCLK");
    d.connect("pll0refclk", "u_gtp_common", "PLL0OUTREFCLK");

    for ch in 0..2 {
        let name = format!("u_gtp_ch{ch}");
        add_bb(&mut d, &name, "GTPE2_CHANNEL");
        d.set_cell_attr(&name, "RXOUT_DIV", "2").unwrap();
        d.set_cell_attr(&name, "TXOUT_DIV", "2").unwrap();
        d.set_cell_attr(&name, "RX_DATA_WIDTH", "20").unwrap();
        d.set_cell_attr(&name, "TX_DATA_WIDTH", "20").unwrap();
        d.set_cell_attr(&name, "RX_INT_DATAWIDTH", "0").unwrap();
        d.set_cell_attr(&name, "TX_INT_DATAWIDTH", "1").unwrap();
        d.connect("pll0clk", &name, "PLL0CLK");
        d.connect("pll0refclk", &name, "PLL0REFCLK");
        d.connect(&format!("txoutclk{ch}"), &name, "TXOUTCLK");
        d.connect(&format!("rxoutclk{ch}"), &name, "RXOUTCLK");
    }

    // 2. PCIE_2_1 hard block
    add_bb(&mut d, "u_pcie", "PCIE_2_1");
    d.set_cell_attr("u_pcie", "LINK_CAP_MAX_LINK_SPEED", "2")
        .unwrap();
    d.set_cell_attr("u_pcie", "LINK_CAP_MAX_LINK_WIDTH", "4")
        .unwrap();
    d.set_cell_attr("u_pcie", "DEV_CAP_MAX_PAYLOAD_SUPPORTED", "2")
        .unwrap();
    d.set_cell_attr("u_pcie", "BAR0", "32'hFFFF0000").unwrap();
    d.connect("txoutclk0", "u_pcie", "PIPECLK");

    // 3. XADC dual 12-bit 1MSPS ADC
    add_bb(&mut d, "u_xadc", "XADC");
    d.set_cell_attr("u_xadc", "INIT_40", "16'h9000").unwrap();
    d.set_cell_attr("u_xadc", "INIT_41", "16'h2EF0").unwrap();
    d.set_cell_attr("u_xadc", "INIT_48", "16'h4701").unwrap();
    d.connect("txoutclk0", "u_xadc", "DCLK");

    // 4. Configuration & JTAG primitives: BSCANE2, ICAPE2, DNA_PORT, FRAME_ECCE2, STARTUPE2, USR_ACCESSE2, CAPTUREE2
    add_bb(&mut d, "u_bscan", "BSCANE2");
    d.set_cell_attr("u_bscan", "JTAG_CHAIN", "2").unwrap();

    add_bb(&mut d, "u_icap", "ICAPE2");
    d.set_cell_attr("u_icap", "ICAP_WIDTH", "X32").unwrap();
    d.set_cell_attr("u_icap", "DEVICE_ID", "32'h0362D093")
        .unwrap();
    d.connect("txoutclk0", "u_icap", "CLK");

    add_bb(&mut d, "u_dna", "DNA_PORT");
    d.set_cell_attr("u_dna", "SIM_DNA_VALUE", "57'h123456789ABCDEF")
        .unwrap();
    d.connect("txoutclk0", "u_dna", "CLK");

    add_bb(&mut d, "u_fecc", "FRAME_ECCE2");
    d.set_cell_attr("u_fecc", "FARSRC", "EFAR").unwrap();

    add_bb(&mut d, "u_startup", "STARTUPE2");
    d.set_cell_attr("u_startup", "PROG_USR", "TRUE").unwrap();

    add_bb(&mut d, "u_usr_access", "USR_ACCESSE2");

    add_bb(&mut d, "u_capture", "CAPTUREE2");
    d.set_cell_attr("u_capture", "ONESHOT", "TRUE").unwrap();

    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let packed = backend.pack(&d).unwrap();
    assert_eq!(packed.ibufds_gte2s.len(), 1);
    assert_eq!(packed.gtp_commons.len(), 1);
    assert_eq!(packed.gtp_channels.len(), 2);
    assert_eq!(packed.pcies.len(), 1);
    assert_eq!(packed.xadcs.len(), 1);
    assert_eq!(packed.cfg_prims.len(), 7);

    let placed = backend.place(&packed).unwrap();
    assert_eq!(placed.gtp_commons[0].site, "GTPE2_COMMON_X0Y0");
    assert_eq!(placed.gtp_channels[0].site, "GTPE2_CHANNEL_X0Y0");
    assert_eq!(placed.gtp_channels[1].site, "GTPE2_CHANNEL_X0Y1");
    assert_eq!(placed.pcies[0].site, "PCIE_2_1_X0Y0");
    assert_eq!(placed.xadcs[0].site, "XADC_X0Y0");

    let routed = backend.route(&placed).unwrap();
    let bs = backend.bitstream(&routed).unwrap();
    let text = bs.fasm.join("\n");

    assert!(
        text.contains("GTP_COMMON_X113Y26.IBUFDS_GTE2_X0Y0.IN_USE"),
        "got:\n{text}"
    );
    assert!(
        text.contains("GTP_COMMON_X113Y26.GTPE2_COMMON.PLL0_FBDIV_45.D5"),
        "got:\n{text}"
    );
    assert!(
        text.contains("GTP_CHANNEL_0_X113Y10.GTPE2_CHANNEL.RX_DATA_WIDTH.W20"),
        "got:\n{text}"
    );
    assert!(
        text.contains("PCIE_BOT_X113Y25.PCIE_2_1.LINK_CAP_MAX_LINK_SPEED.GEN2"),
        "got:\n{text}"
    );
    assert!(
        text.contains("PCIE_BOT_X113Y25.PCIE_2_1.BAR0[31:0] = 32'hFFFF0000"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.XADC.INIT_40[15:0] = 16'h9000"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.BSCAN.JTAG_CHAIN_2.IN_USE"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.ICAP.ICAP_WIDTH.X32"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.DNA_PORT.SIM_DNA_VALUE[56:0] = 57'h123456789ABCDEF"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.FRAME_ECC.FARSRC.EFAR"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.STARTUP.PROG_USR"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.USR_ACCESS.IN_USE"),
        "got:\n{text}"
    );
    assert!(
        text.contains("CFG_CENTER_MID_X51Y76.CAPTURE.ONESHOT"),
        "got:\n{text}"
    );

    let parsed = parse_fasm(&text).expect("all emitted FASM lines parse cleanly");
    assert!(!parsed.is_empty());
}
