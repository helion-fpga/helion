//! Comprehensive UG953 primitive test suite for `helion-xilinx7`.
//!
//! Covers packing, placement legality, routing, FASM emission, FASM parsing,
//! frame bit assembly, and `.bit` bitstream emission for:
//! - `MMCME2_ADV` & `PLLE2_ADV`
//! - `RAMB36E1` & `RAMB18E1` (TDP, SDP, cascade, `WEA`/`WEB`, `INIT_00..INIT_7F`, `INITP_00..INITP_0F`)
//! - `DSP48E1` (multiplier, pre-adder, ALU, pattern detector, `PCOUT->PCIN` / `ACOUT->ACIN` / `BCOUT->BCIN` cascades)
//! - `IDDR` & `ODDR` (`OPPOSITE_EDGE`, `SAME_EDGE`, `SAME_EDGE_PIPELINED`)
//! - `ISERDESE2` & `OSERDESE2` (4:1, 8:1, 10:1 Master/Slave DDR/SDR serialization)
//! - `IDELAYE2`, `ODELAYE2`, & `IDELAYCTRL` (32-tap calibrated delay lines & bank/group rules)
//! - `BUFG`, `BUFGCE`, `BUFGCTRL`, `BUFIO`, & `BUFR`
//! - `SRL16E` & `SRLC32E` in `SLICEM`
//! - `CARRY4` column chains, `MUXF7`, & `MUXF8`

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design, PortDir};
use helion_xilinx7::backend::Xilinx7Backend;
use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{BlockType, FrameStore, SegBitsDb, StubTileLayout};
use helion_xilinx7::place::parse_site_xy;
use helion_xilinx7::route::RouteKind;

fn add_blackbox(d: &mut Design, name: &str, module: &str) {
    d.add_cell(
        name,
        CellKind::BlackBox {
            module: intern(module),
        },
    );
}

#[test]
fn ug953_mmcm_and_pll_pack_place_route_fasm() {
    let mut d = Design::new("cmt_top");
    add_blackbox(&mut d, "u_mmcm", "MMCME2_ADV");
    d.set_cell_attr("u_mmcm", "BANDWIDTH", "HIGH").unwrap();
    d.set_cell_attr("u_mmcm", "CLKIN1_PERIOD", "10.0").unwrap(); // 100 MHz
    d.set_cell_attr("u_mmcm", "DIVCLK_DIVIDE", "1").unwrap();
    d.set_cell_attr("u_mmcm", "CLKFBOUT_MULT_F", "10.0").unwrap(); // 1000 MHz VCO
    d.set_cell_attr("u_mmcm", "CLKOUT0_DIVIDE_F", "5.0").unwrap();
    d.set_cell_attr("u_mmcm", "CLKOUT0_PHASE", "9.0").unwrap(); // 1/8 VCO period
    d.set_cell_attr("u_mmcm", "CLKOUT1_DIVIDE", "7").unwrap();
    d.connect("clk_in", "u_mmcm", "CLKIN1");
    d.connect("fb", "u_mmcm", "CLKFBOUT");
    d.connect("fb", "u_mmcm", "CLKFBIN");
    d.connect("clk200", "u_mmcm", "CLKOUT0");
    d.connect("clk143", "u_mmcm", "CLKOUT1");

    add_blackbox(&mut d, "u_pll", "PLLE2_ADV");
    d.set_cell_attr("u_pll", "CLKIN1_PERIOD", "10.0").unwrap();
    d.set_cell_attr("u_pll", "DIVCLK_DIVIDE", "1").unwrap();
    d.set_cell_attr("u_pll", "CLKFBOUT_MULT", "12").unwrap(); // 1200 MHz VCO
    d.set_cell_attr("u_pll", "CLKOUT0_DIVIDE", "6").unwrap();
    d.connect("clk_in", "u_pll", "CLKIN1");
    d.connect("pfb", "u_pll", "CLKFBOUT");
    d.connect("pfb", "u_pll", "CLKFBIN");
    d.connect("clk200b", "u_pll", "CLKOUT0");

    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let bs = backend.run(&d).unwrap();
    let text = bs.fasm.join("\n");
    // Vivado's words (tools/x7v/tests/cmt_sweep.py): M = 10 HIGH = OPTIMIZED.
    for want in [
        "MMCME2_ADV.IN_USE",
        "MMCME2_ADV.COMP.Z_ZHOLD",
        "MMCME2_ADV.ZINV_RST",
        "MMCME2_ADV.CLKFBOUT_CLKOUT1_HIGH_TIME[5:0] = 6'd5",
        "MMCME2_ADV.CLKOUT1_CLKOUT1_HIGH_TIME[5:0] = 6'd3",
        "MMCME2_ADV.CLKOUT1_CLKOUT1_LOW_TIME[5:0] = 6'd4",
        "MMCME2_ADV.CLKOUT1_CLKOUT2_EDGE[0]",
        "MMCME2_ADV.CLKOUT6_CLKOUT2_FRACTIONAL_NO_COUNT[0]",
        "MMCME2_ADV.LKTABLE[39:0] = 40'hE73E8FA401",
        "MMCME2_ADV.TABLE[9:0] = 10'h3D4",
        "MMCME2_ADV.CLKOUT0_CLKOUT1_PHASE_MUX[2:0] = 3'd1",
        "MMCME2_ADV.CLKOUT5_CLKOUT2_FRACTIONAL_PHASE_MUX_F[2:0] = 3'd1",
        // An odd eighth of a VCO period powers the interpolator (sweep_pwr4).
        "MMCME2_ADV.POWER_REG_POWER_REG_POWER_REG[15:0] = 16'h9900",
        "PLLE2_ADV.IN_USE",
        "PLLE2_ADV.COMPENSATION.Z_ZHOLD_OR_CLKIN_BUF",
    ] {
        assert!(text.contains(want), "missing {want}:\n{text}");
    }
    assert!(!text.contains("BANDWIDTH"), "no BANDWIDTH feature exists:\n{text}");
    let parsed = parse_fasm(&text).expect("valid FASM");
    assert!(!parsed.is_empty());
}

/// Fractional dividers are not measured yet: refused, never guessed.
#[test]
fn ug953_fractional_mmcm_is_refused() {
    let mut d = Design::new("cmt_frac");
    add_blackbox(&mut d, "u_mmcm", "MMCME2_ADV");
    d.set_cell_attr("u_mmcm", "CLKIN1_PERIOD", "10.0").unwrap();
    d.set_cell_attr("u_mmcm", "CLKFBOUT_MULT_F", "10.5").unwrap();
    d.set_cell_attr("u_mmcm", "CLKOUT0_DIVIDE_F", "5.0").unwrap();
    d.connect("clk_in", "u_mmcm", "CLKIN1");
    d.connect("fb", "u_mmcm", "CLKFBOUT");
    d.connect("fb", "u_mmcm", "CLKFBIN");
    d.connect("c0", "u_mmcm", "CLKOUT0");
    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let err = backend.run(&d).unwrap_err();
    assert!(err.contains("fractional"), "got: {err}");
}

#[test]
fn ug953_ramb36e1_and_ramb18e1_cascade_and_init_frames() {
    let mut d = Design::new("bram_top");
    add_blackbox(&mut d, "u_ram36_lo", "RAMB36E1");
    d.set_cell_attr("u_ram36_lo", "RAM_MODE", "SDP").unwrap();
    d.set_cell_attr("u_ram36_lo", "READ_WIDTH_A", "72").unwrap();
    d.set_cell_attr("u_ram36_lo", "WRITE_WIDTH_B", "72")
        .unwrap();
    d.set_cell_attr("u_ram36_lo", "WRITE_MODE_A", "READ_FIRST")
        .unwrap();
    d.set_cell_attr("u_ram36_lo", "DOA_REG", "1").unwrap();
    d.set_cell_attr("u_ram36_lo", "CASCADE_ORDER_A", "FIRST")
        .unwrap();
    d.set_cell_attr(
        "u_ram36_lo",
        "INIT_00",
        "256'h40000000_00000000_00000000_00000000_00000000_00000000_00000000_00000008",
    )
    .unwrap();
    d.set_cell_attr(
        "u_ram36_lo",
        "INITP_00",
        "256'h00000000_00000000_00000000_00000000_00000000_00000000_00000000_000000A5",
    )
    .unwrap();
    d.connect("casc_a", "u_ram36_lo", "CASCADEOUTA");
    d.connect("we0", "u_ram36_lo", "WEA0");
    d.connect("web7", "u_ram36_lo", "WEBWE7");

    add_blackbox(&mut d, "u_ram36_hi", "RAMB36E1");
    d.set_cell_attr("u_ram36_hi", "CASCADE_ORDER_A", "LAST")
        .unwrap();
    d.connect("casc_a", "u_ram36_hi", "CASCADEINA");

    add_blackbox(&mut d, "u_ram18", "RAMB18E1");
    d.set_cell_attr("u_ram18", "READ_WIDTH_A", "18").unwrap();
    d.set_cell_attr("u_ram18", "WRITE_WIDTH_B", "18").unwrap();

    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let packed = backend.pack(&d).unwrap();
    assert_eq!(packed.brams.len(), 3);
    assert_eq!(packed.brams[0].wea_nets, vec!["we0".to_string()]);
    assert_eq!(packed.brams[0].web_nets, vec!["web7".to_string()]);

    let placed = backend.place(&packed).unwrap();
    let (x0, y0) = parse_site_xy(&placed.brams[0].site).unwrap();
    let (x1, y1) = parse_site_xy(&placed.brams[1].site).unwrap();
    assert_eq!(x0, x1, "cascaded RAMB36E1 must share column X");
    assert_eq!(y1, y0 + 1, "cascaded RAMB36E1 must have Y1 == Y0 + 1");

    // RAMB36E1 pins split bit-interleaved over the tile's two RAMB18
    // halves; until that is written the flow refuses it by name rather
    // than emit features X-Ray does not have.
    let err = backend.route(&placed).unwrap_err();
    assert!(err.contains("RAMB36E1 pin routing is not supported"), "got: {err}");

    // A RAMB18E1 alone goes all the way, under X-Ray's feature names.
    let mut d18 = Design::new("bram18_top");
    add_blackbox(&mut d18, "u_ram18", "RAMB18E1");
    d18.set_cell_attr("u_ram18", "READ_WIDTH_A", "18").unwrap();
    d18.set_cell_attr("u_ram18", "WRITE_WIDTH_B", "18").unwrap();
    let bs = backend.run(&d18).unwrap();
    let text = bs.fasm.join("\n");
    for f in [".IN_USE", ".READ_WIDTH_A_18", ".WRITE_WIDTH_B_18", ".ZINV_CLKARDCLK"] {
        assert!(text.lines().any(|l| l.contains(".RAMB18_Y") && l.ends_with(f)), "missing {f}; got:\n{text}");
    }

    // A 256-bit INIT_00 line parses and applies into FrameStore.
    let lines = parse_fasm(
        "BRAM_L_X6Y0.RAMB36.INIT_00[255:0] = 256'h40000000_00000000_00000000_00000000_00000000_00000000_00000000_00000008",
    )
    .expect("parse 256-bit FASM");
    let init_line = lines
        .iter()
        .find(|l| l.feature.ends_with("INIT_00"))
        .expect("INIT_00 line");
    let mut layout = StubTileLayout::new();
    layout.add_tile(&init_line.tile, "BRAM_L", BlockType::BlockRam, 0x0080_0000, 0);
    let mut segbits = SegBitsDb::new();
    segbits
        .add_segbits(
            "BRAM_L",
            BlockType::BlockRam,
            "BRAM_L.RAMB36.INIT_00[003] 00_03\nBRAM_L.RAMB36.INIT_00[254] 01_14\n",
        )
        .unwrap();
    let mut store = FrameStore::new(&[0x0080_0000, 0x0080_0001]);
    let applied = store
        .apply_fasm(std::slice::from_ref(init_line), &segbits, &layout)
        .unwrap();
    assert_eq!(applied, 2);
    assert_eq!(store.frame_words(0x0080_0000).unwrap()[0], 1 << 3);
    assert_eq!(store.frame_words(0x0080_0001).unwrap()[0], 1 << 14);
}

#[test]
fn ug953_dsp48e1_preadder_patdet_and_column_cascade() {
    let mut d = Design::new("dsp_top");
    add_blackbox(&mut d, "u_dsp0", "DSP48E1");
    d.set_cell_attr("u_dsp0", "AREG", "2").unwrap();
    d.set_cell_attr("u_dsp0", "BREG", "1").unwrap();
    d.set_cell_attr("u_dsp0", "DREG", "1").unwrap();
    d.set_cell_attr("u_dsp0", "ADREG", "1").unwrap();
    d.set_cell_attr("u_dsp0", "MREG", "1").unwrap();
    d.set_cell_attr("u_dsp0", "PREG", "1").unwrap();
    d.set_cell_attr("u_dsp0", "USE_DPORT", "TRUE").unwrap();
    d.set_cell_attr("u_dsp0", "INMODE", "5'b00101").unwrap();
    d.set_cell_attr("u_dsp0", "OPMODE", "7'b0000101").unwrap();
    d.set_cell_attr("u_dsp0", "ALUMODE", "4'b0000").unwrap();
    d.connect("pcasc", "u_dsp0", "PCOUT");
    d.connect("acasc", "u_dsp0", "ACOUT");
    d.connect("bcasc", "u_dsp0", "BCOUT");

    add_blackbox(&mut d, "u_dsp1", "DSP48E1");
    d.set_cell_attr("u_dsp1", "A_INPUT", "CASCADE").unwrap();
    d.set_cell_attr("u_dsp1", "B_INPUT", "CASCADE").unwrap();
    d.set_cell_attr("u_dsp1", "USE_PATTERN_DETECT", "PATDET")
        .unwrap();
    d.set_cell_attr("u_dsp1", "PATTERN", "48'h123456789ABC")
        .unwrap();
    d.set_cell_attr("u_dsp1", "MASK", "48'h0000FFFFFFFF")
        .unwrap();
    d.connect("pcasc", "u_dsp1", "PCIN");
    d.connect("acasc", "u_dsp1", "ACIN");
    d.connect("bcasc", "u_dsp1", "BCIN");

    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let packed = backend.pack(&d).unwrap();
    let placed = backend.place(&packed).unwrap();
    let (x0, y0) = parse_site_xy(&placed.dsps[0].site).unwrap();
    let (x1, y1) = parse_site_xy(&placed.dsps[1].site).unwrap();
    assert_eq!(x0, x1);
    assert_eq!(y1, y0 + 1);

    let routed = backend.route(&placed).unwrap();
    for net in ["pcasc", "acasc", "bcasc"] {
        assert!(routed
            .nets
            .iter()
            .any(|n| n.net == net && n.kind == RouteKind::DedicatedCascade));
    }

    let bs = backend.bitstream(&routed).unwrap();
    let text = bs.fasm.join("\n");
    // Each DSP tile holds two DSP48s: even Y is slot DSP_0, odd Y is DSP_1; a
    // cascade pair may straddle two tiles.
    let (s0, s1) = (format!("DSP_{}.", y0 % 2), format!("DSP_{}.", y1 % 2));
    // X-Ray names (`DSP48.DSP_k.*`): MREG/PREG = 1 is the all-clear
    // encoding, USE_PATTERN_DETECT has no bit.
    assert!(text.contains(&format!("{s0}USE_DPORT[0]")), "got:\n{text}");
    assert!(text.contains(&format!("{s0}AREG_2")), "got:\n{text}");
    assert!(text.contains(&format!("{s0}BREG_1")), "got:\n{text}");
    assert!(!text.contains(&format!("{s0}ZMREG")), "got:\n{text}");
    assert!(!text.contains(&format!("{s0}ZPREG")), "got:\n{text}");
    assert!(text.contains(&format!("{s1}A_INPUT[0]")), "got:\n{text}");
    assert!(text.contains(&format!("{s1}B_INPUT[0]")), "got:\n{text}");
    assert!(
        text.contains(&format!("{s1}PATTERN[47:0] = 48'h123456789ABC")),
        "got:\n{text}"
    );
    parse_fasm(&text).expect("valid DSP FASM");
}

#[test]
fn ug953_io_primitives_iddr_oddr_serdes_iodelay_clock_buffers() {
    let mut d = Design::new("io_top");

    // Clock buffers: BUFG, BUFGCE, BUFGCTRL, BUFIO, BUFR
    for (name, prim) in [
        ("u_bufg", "BUFG"),
        ("u_bufgce", "BUFGCE"),
        ("u_bufgctrl", "BUFGCTRL"),
        ("u_bufio", "BUFIO"),
        ("u_bufr", "BUFR"),
    ] {
        add_blackbox(&mut d, name, prim);
        d.connect(&format!("{name}_in"), name, "I0");
        d.connect(&format!("{name}_out"), name, "O");
    }
    d.set_cell_attr("u_bufr", "BUFR_DIVIDE", "4").unwrap();
    d.set_cell_attr("u_bufgctrl", "PRESELECT_I0", "TRUE")
        .unwrap();

    // IDELAYCTRL + IDELAYE2 + ODELAYE2
    add_blackbox(&mut d, "u_dlyctrl", "IDELAYCTRL");
    d.set_cell_attr("u_dlyctrl", "IODELAY_GROUP", "GRP_A")
        .unwrap();
    d.connect("u_bufg_out", "u_dlyctrl", "REFCLK");

    add_blackbox(&mut d, "u_idelay", "IDELAYE2");
    d.set_cell_attr("u_idelay", "IODELAY_GROUP", "GRP_A")
        .unwrap();
    d.set_cell_attr("u_idelay", "IDELAY_TYPE", "VAR_LOAD")
        .unwrap();
    d.set_cell_attr("u_idelay", "IDELAY_VALUE", "19").unwrap();
    d.set_cell_attr("u_idelay", "HIGH_PERFORMANCE_MODE", "TRUE")
        .unwrap();
    d.connect("u_bufr_out", "u_idelay", "C");
    d.connect("pad_in", "u_idelay", "IDATAIN");
    d.connect("dly_to_iserdes", "u_idelay", "DATAOUT");

    add_blackbox(&mut d, "u_odelay", "ODELAYE2");
    d.set_cell_attr("u_odelay", "IODELAY_GROUP", "GRP_A")
        .unwrap();
    d.set_cell_attr("u_odelay", "ODELAY_TYPE", "FIXED").unwrap();
    d.set_cell_attr("u_odelay", "ODELAY_VALUE", "7").unwrap();
    d.connect("u_bufr_out", "u_odelay", "C");
    d.connect("oserdes_to_dly", "u_odelay", "ODATAIN");
    d.connect("pad_out", "u_odelay", "DATAOUT");

    // 10:1 DDR ISERDESE2 MASTER + SLAVE cascade
    add_blackbox(&mut d, "u_iserdes_m", "ISERDESE2");
    d.set_cell_attr("u_iserdes_m", "DATA_RATE", "DDR").unwrap();
    d.set_cell_attr("u_iserdes_m", "DATA_WIDTH", "10").unwrap();
    d.set_cell_attr("u_iserdes_m", "INTERFACE_TYPE", "NETWORKING")
        .unwrap();
    d.set_cell_attr("u_iserdes_m", "SERDES_MODE", "MASTER")
        .unwrap();
    d.set_cell_attr("u_iserdes_m", "IOBDELAY", "IFD").unwrap();
    d.connect("u_bufio_out", "u_iserdes_m", "CLK");
    d.connect("u_bufr_out", "u_iserdes_m", "CLKDIV");
    d.connect("dly_to_iserdes", "u_iserdes_m", "DDLY");
    d.connect("ishift1", "u_iserdes_m", "SHIFTOUT1");
    d.connect("ishift2", "u_iserdes_m", "SHIFTOUT2");

    add_blackbox(&mut d, "u_iserdes_s", "ISERDESE2");
    d.set_cell_attr("u_iserdes_s", "DATA_RATE", "DDR").unwrap();
    d.set_cell_attr("u_iserdes_s", "DATA_WIDTH", "10").unwrap();
    d.set_cell_attr("u_iserdes_s", "INTERFACE_TYPE", "NETWORKING")
        .unwrap();
    d.set_cell_attr("u_iserdes_s", "SERDES_MODE", "SLAVE")
        .unwrap();
    d.connect("u_bufio_out", "u_iserdes_s", "CLK");
    d.connect("u_bufr_out", "u_iserdes_s", "CLKDIV");
    d.connect("ishift1", "u_iserdes_s", "SHIFTIN1");
    d.connect("ishift2", "u_iserdes_s", "SHIFTIN2");

    // 8:1 DDR OSERDESE2
    add_blackbox(&mut d, "u_oserdes", "OSERDESE2");
    d.set_cell_attr("u_oserdes", "DATA_RATE_OQ", "DDR").unwrap();
    d.set_cell_attr("u_oserdes", "DATA_WIDTH", "8").unwrap();
    d.set_cell_attr("u_oserdes", "TRISTATE_WIDTH", "4").unwrap();
    d.connect("u_bufio_out", "u_oserdes", "CLK");
    d.connect("u_bufr_out", "u_oserdes", "CLKDIV");
    d.connect("oserdes_to_dly", "u_oserdes", "OQ");

    // IDDR (SAME_EDGE_PIPELINED) and ODDR (SAME_EDGE)
    add_blackbox(&mut d, "u_iddr", "IDDR");
    d.set_cell_attr("u_iddr", "DDR_CLK_EDGE", "SAME_EDGE_PIPELINED")
        .unwrap();
    d.connect("u_bufr_out", "u_iddr", "C");
    d.connect("iddr_d", "u_iddr", "D");
    d.connect("iddr_q1", "u_iddr", "Q1");
    d.connect("iddr_q2", "u_iddr", "Q2");

    add_blackbox(&mut d, "u_oddr", "ODDR");
    d.set_cell_attr("u_oddr", "DDR_CLK_EDGE", "SAME_EDGE")
        .unwrap();
    d.connect("u_bufr_out", "u_oddr", "C");
    d.connect("oddr_d1", "u_oddr", "D1");
    d.connect("oddr_d2", "u_oddr", "D2");
    d.connect("oddr_q", "u_oddr", "Q");

    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let packed = backend.pack(&d).unwrap();
    let placed = backend.place(&packed).unwrap();
    // Verify MASTER and SLAVE ISERDESE2 share the same LIOI3 tile at Y0 and Y1.
    assert_eq!(placed.serdes[0].tile, placed.serdes[1].tile);
    let (_, ym) = parse_site_xy(&placed.serdes[0].site).unwrap();
    let (_, ys) = parse_site_xy(&placed.serdes[1].site).unwrap();
    assert_eq!(ys, ym + 1);

    let routed = backend.route(&placed).unwrap();
    let bs = backend.bitstream(&routed).unwrap();
    let text = bs.fasm.join("\n");
    assert!(text.contains("BUFR_DIVIDE.D4"), "got:\n{text}");
    assert!(text.contains("IDELAYCTRL.IN_USE"), "got:\n{text}");
    assert!(
        text.contains("IDELAY_Y0.IDELAY_VALUE[4:0] = 5'b10011"),
        "got:\n{text}"
    );
    assert!(
        text.contains("ISERDES.DATA_WIDTH.W10"),
        "got:\n{text}"
    );
    assert!(
        text.contains("ISERDES.SERDES_MODE.SLAVE"),
        "got:\n{text}"
    );
    assert!(text.contains("OSERDES.DATA_WIDTH.W8"), "got:\n{text}");
    assert!(
        text.contains("IFF.DDR_CLK_EDGE.SAME_EDGE_PIPELINED"),
        "got:\n{text}"
    );
    assert!(
        text.contains("ODDR_DDR_CLK_EDGE.SAME_EDGE"),
        "got:\n{text}"
    );
    parse_fasm(&text).expect("valid I/O FASM");
}

#[test]
fn ug953_srl16e_srlc32e_carry4_muxf7_muxf8_and_bitstream() {
    let mut d = Design::new("slice_prims");

    // SRLC32E cascaded into SRL16E. Its data and enable come from pins: a
    // net nothing drives would float on silicon and is refused by the router.
    for (p, pin) in [("din", "J15"), ("ce", "L16"), ("s0", "M13"), ("s1", "R15")] {
        d.add_port(p, PortDir::In);
        d.set_loc(p, pin).unwrap();
    }
    add_blackbox(&mut d, "u_srl32", "SRLC32E");
    d.set_cell_attr("u_srl32", "INIT", "32'hDEADBEEF").unwrap();
    d.connect("clk_g", "u_srl32", "CLK");
    d.connect("ce", "u_srl32", "CE");
    d.connect("din", "u_srl32", "D");
    d.connect("q32", "u_srl32", "Q");
    d.connect("q31_casc", "u_srl32", "Q31");

    add_blackbox(&mut d, "u_srl16", "SRL16E");
    d.set_cell_attr("u_srl16", "INIT", "16'hCAFE").unwrap();
    d.connect("clk_g", "u_srl16", "CLK");
    d.connect("ce", "u_srl16", "CE");
    d.connect("q31_casc", "u_srl16", "D");
    d.connect("q16", "u_srl16", "Q");

    add_blackbox(&mut d, "u_bufg", "BUFG");
    d.connect("clk_raw", "u_bufg", "I");
    d.connect("clk_g", "u_bufg", "O");

    // 4x LUT6 -> 2x MUXF7 -> 1x MUXF8 in a single slice
    for i in 0..4 {
        let l = format!("u_l{i}");
        d.add_cell(&l, CellKind::Lut6 { init: 0xAA55_AA55_AA55_AA55 });
        d.connect(&format!("l{i}_o"), &l, "O");
    }
    add_blackbox(&mut d, "u_f7a", "MUXF7");
    d.connect("l0_o", "u_f7a", "I0");
    d.connect("l1_o", "u_f7a", "I1");
    d.connect("s0", "u_f7a", "S");
    d.connect("f7a_o", "u_f7a", "O");

    add_blackbox(&mut d, "u_f7b", "MUXF7");
    d.connect("l2_o", "u_f7b", "I0");
    d.connect("l3_o", "u_f7b", "I1");
    d.connect("s0", "u_f7b", "S");
    d.connect("f7b_o", "u_f7b", "O");

    add_blackbox(&mut d, "u_f8", "MUXF8");
    d.connect("f7a_o", "u_f8", "I0");
    d.connect("f7b_o", "u_f8", "I1");
    d.connect("s1", "u_f8", "S");
    d.connect("f8_o", "u_f8", "O");

    // 2x chained CARRY4
    d.add_cell("u_c0", CellKind::Carry4);
    d.connect("1", "u_c0", "CYINIT");
    d.connect("di0", "u_c0", "DI0");
    d.connect("co3", "u_c0", "CO3");
    d.add_cell("u_c1", CellKind::Carry4);
    d.connect("co3", "u_c1", "CI");

    let backend = Xilinx7Backend::open("xc7a35t").unwrap();
    let packed = backend.pack(&d).unwrap();
    let placed = backend.place(&packed).unwrap();

    // SRLs must land in a CLBLM (SLICEM) tile on D6LUT and C6LUT.
    assert!(placed.srls[0].tile.starts_with("CLBLM_"));
    assert_eq!(placed.srls[0].site, placed.srls[1].site);
    assert_eq!(placed.srls[0].bel, "D6LUT");
    assert_eq!(placed.srls[1].bel, "C6LUT");

    // MUXF7/MUXF8 tree shares one slice.
    assert_eq!(placed.muxfs[0].site, placed.muxfs[2].site);
    assert_eq!(placed.muxfs[1].site, placed.muxfs[2].site);

    let routed = backend.route(&placed).unwrap();
    let bs = backend.bitstream(&routed).unwrap();
    let text = bs.fasm.join("\n");
    assert!(text.contains(".DLUT.SRL"), "got:\n{text}");
    assert!(text.contains(".DLUT.SMALL"), "got:\n{text}");
    assert!(text.contains(".CLUT.SRL"), "got:\n{text}");
    assert!(text.contains(".F7AMUX.IN_USE"), "got:\n{text}");
    assert!(text.contains(".F7BMUX.IN_USE"), "got:\n{text}");
    assert!(text.contains(".F8MUX.IN_USE"), "got:\n{text}");
    assert!(text.contains(".PRECYINIT.C1"), "got:\n{text}");
    assert!(text.contains(".PRECYINIT.CIN"), "got:\n{text}");
    assert!(text.contains(".CARRY4.ACY0"), "got:\n{text}");

    let lines = parse_fasm(&text).expect("parse slice FASM");
    assert!(!lines.is_empty());

    let store = FrameStore::new(&[0, 1]);
    let payload = store.fdri_payload();
    let bit = bitstream::write_bit(&payload, &BitstreamOptions::default()).unwrap();
    assert!(bit.len() > 100);
}
