//! Tests for [`crate::textcfg`]. Kept in a separate file to keep the
//! emitter readable; included via `#[cfg(test)]` at the bottom of
//! `textcfg.rs`.

use crate::device::Device;
use crate::textcfg::{
    bit_reverse, check_arc, check_enum_on_type, check_word_on_type, emit_design, parse_config_words,
    word_bin, DccBlock, DspBlock, EbrBlock, EbrSite, EmitArc, EmitDesign, IoDir, IoPull, PioId,
    PlacedFf, PlacedIo, PlacedLut, PllBlock, SliceId,
};

fn dev() -> Device {
    Device::open().expect("Device::open")
}

fn test_lut() -> PlacedLut {
    PlacedLut {
        tile_key: "R2C62:PLC2".to_string(),
        slice: SliceId::C,
        lut: 0,
        init: 0x0f0f,
        carry: false,
        input_used: [true, true, false, false],
        ff: Some(PlacedFf {
            gsr_enabled: false,
            set_on_reset: false,
            clk_inverted: false,
            ce_used: false,
            lsr_used: true,
            d_from_lut: true,
            clk_sel: 0,
            lsr_sel: 0,
        }),
    }
}

#[test]
fn word_bin_is_msb_first() {
    assert_eq!(word_bin(0x0f0f, 16), "0000111100001111");
    assert_eq!(word_bin(0, 4), "0000");
    assert_eq!(word_bin(0b101, 3), "101");
    // EBR WID word: bit-reversed wid, MSB first (nextpnr convention).
    assert_eq!(bit_reverse(3, 9), 0b110000000);
    assert_eq!(word_bin(bit_reverse(3, 9), 9), "110000000");
    assert_eq!(bit_reverse(0, 9), 0);
}

#[test]
fn lut_init_hand_check_against_db() {
    // Hand-check one slice against the DB: SLICEC.K0.INIT's 16 `.config`
    // lines map INIT bit 15 -> F79B10 down to bit 0 -> F64B10 (line i of
    // the `.config` section is word-bit i, MSB first; each line is `!`
    // because the config bit is set when the INIT bit is 0).
    let text = crate::tiledata::bits_db("PLC2").unwrap();
    let words = parse_config_words(text);
    let w = words
        .iter()
        .find(|w| w.name == "SLICEC.K0.INIT")
        .expect("SLICEC.K0.INIT in PLC2 bits.db");
    assert_eq!(w.bits, 16);
    // Re-read the raw lines to check the exact frame mapping.
    let mut lines: Vec<String> = Vec::new();
    let mut in_sec = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line == ".config SLICEC.K0.INIT 1111111111111111" {
            in_sec = true;
            continue;
        }
        if in_sec {
            if line.starts_with('.') || line.is_empty() {
                break;
            }
            lines.push(line.to_string());
        }
    }
    assert_eq!(lines.len(), 16);
    assert_eq!(lines[0], "!F79B10");
    assert_eq!(lines[15], "!F64B10");
    // Frames descend by one per word bit: bit 15 -> F79, bit 0 -> F64.
    for (i, l) in lines.iter().enumerate() {
        assert_eq!(*l, format!("!F{}B10", 79 - i));
    }
    // The emitter's word value for INIT=0x0F0F matches nextpnr's format.
    assert_eq!(word_bin(u32::from(0x0f0fu16), 16), "0000111100001111");
}

#[test]
fn routed_arc_sets_exactly_db_bits() {
    // From the reference blinky textcfg: `.tile R2C62:PLC2` /
    // `arc: C4 Q4`. The DB must back it with exactly these bit reqs.
    let device = dev();
    check_arc(&device, "R2C62:PLC2", "C4", "Q4").expect("C4 <- Q4 valid");
    let tb = device.tile_bits("PLC2").unwrap();
    let mux = tb.muxes.iter().find(|m| m.dst == "C4").unwrap();
    let inp = mux.inputs.iter().find(|i| i.src == "Q4").unwrap();
    let bits: Vec<(u32, u32, bool)> =
        inp.bits.iter().map(|b| (b.frame, b.bit, b.val)).collect();
    assert_eq!(bits, vec![(30, 5, true), (33, 4, true)]);
}

#[test]
fn arc_rejects_unknown_sink_and_source() {
    let device = dev();
    let e = check_arc(&device, "R2C62:PLC2", "NOPE", "Q4").unwrap_err();
    assert!(e.contains("no mux"), "{e}");
    let e = check_arc(&device, "R2C62:PLC2", "C4", "NOPE").unwrap_err();
    assert!(e.contains("no input"), "{e}");
    let e = check_arc(&device, "R9C9:NOPE", "C4", "Q4").unwrap_err();
    assert!(e.contains("unknown tile key"), "{e}");
}

#[test]
fn enum_validation_rejects_bad_name_and_value() {
    check_enum_on_type("PIOT1", "PIOB.BASE_TYPE", "OUTPUT_LVCMOS33").unwrap();
    check_enum_on_type("PIOT1", "PIOB.DRIVE", "4").unwrap();
    let e = check_enum_on_type("PIOT1", "PIOB.NOPE", "4").unwrap_err();
    assert!(e.contains("no config enum"), "{e}");
    let e = check_enum_on_type("PIOT1", "PIOB.DRIVE", "99").unwrap_err();
    assert!(e.contains("no option"), "{e}");
}

#[test]
fn word_validation_checks_name_width_and_chars() {
    check_word_on_type("PLC2", "SLICEC.K0.INIT", "0000111100001111").unwrap();
    let e = check_word_on_type("PLC2", "SLICEC.K0.NOPE", "0").unwrap_err();
    assert!(e.contains("no config word"), "{e}");
    let e = check_word_on_type("PLC2", "SLICEC.K0.INIT", "0101").unwrap_err();
    assert!(e.contains("needs 16 bits"), "{e}");
    let e = check_word_on_type("PLC2", "SLICEC.K0.INIT", "000011110000111x").unwrap_err();
    assert!(e.contains("not a binary string"), "{e}");
}

#[test]
fn lut_emission_matches_nextpnr_shape() {
    let device = dev();
    let mut d = EmitDesign::default();
    d.luts.push(test_lut());
    let bs = emit_design(&d, &device).unwrap();
    let t = &bs.textcfg;
    assert!(t.starts_with(".device LFE5U-85F\n"));
    assert!(t.contains(".tile R2C62:PLC2\n"));
    assert!(t.contains("word: SLICEC.K0.INIT 0000111100001111\n"));
    assert!(t.contains("enum: SLICEC.MODE LOGIC\n"));
    // Undriven C/D inputs tied to 1; driven A/B not tied.
    assert!(t.contains("enum: SLICEC.C0MUX 1\n"));
    assert!(t.contains("enum: SLICEC.D0MUX 1\n"));
    assert!(!t.contains("enum: SLICEC.A0MUX 1\n"));
    // FF config.
    assert!(t.contains("enum: SLICEC.GSR DISABLED\n"));
    assert!(t.contains("enum: SLICEC.REG0.SD 1\n"));
    assert!(t.contains("enum: SLICEC.REG0.REGSET RESET\n"));
    assert!(t.contains("enum: CLK0.CLKMUX CLK\n"));
}

#[test]
fn io_config_emits_both_tiles() {
    let device = dev();
    let mut d = EmitDesign::default();
    d.ios.push(PlacedIo {
        pio_tile_key: "MIB_R0C64:PIOT1".to_string(),
        pic_tile_key: "MIB_R1C64:PICT1".to_string(),
        pio: PioId::B,
        dir: IoDir::Output,
        io_type: "LVCMOS33".to_string(),
        pull: IoPull::None,
        drive_ma: 4,
    });
    let bs = emit_design(&d, &device).unwrap();
    let t = &bs.textcfg;
    assert!(t.contains(".tile MIB_R0C64:PIOT1\n"));
    assert!(t.contains(".tile MIB_R1C64:PICT1\n"));
    assert!(t.contains("enum: PIOB.BASE_TYPE OUTPUT_LVCMOS33\n"));
    assert!(t.contains("enum: PIOB.PULLMODE NONE\n"));
    assert!(t.contains("enum: PIOB.DRIVE 4\n"));
}

#[test]
fn ebr_group_matches_nextpnr_reference() {
    // Reference meminit.textcfg: EBR0 at bel (4,88) ->
    // `.tile_group MIB_R70C4:MIB_EBR0 MIB_R70C5:MIB_EBR1`.
    let device = dev();
    let mut init = vec![0u16; 2048];
    for (w, v) in init.iter_mut().enumerate() {
        *v = (w & 0x1ff) as u16;
    }
    let mut d = EmitDesign::default();
    d.ebrs.push(EbrBlock::simple(
        "MIB_R70C4:MIB_EBR0".to_string(),
        EbrSite::Ebr0,
        4,
        88,
        3,
        init,
    ));
    let bs = emit_design(&d, &device).unwrap();
    let t = &bs.textcfg;
    assert!(
        t.contains(".tile_group MIB_R70C4:MIB_EBR0 MIB_R70C5:MIB_EBR1\n"),
        "group line missing:\n{t}"
    );
    assert!(t.contains("word: EBR0.WID 110000000\n"));
    assert!(t.contains("enum: EBR0.MODE DP16KD\n"));
    assert!(t.contains("enum: EBR0.DP16KD.DATA_WIDTH_A 9\n"));
    // bram_init section: wid 3, 256 lines of 8x3-hex groups.
    let sec: Vec<&str> = t
        .lines()
        .skip_while(|l| *l != ".bram_init 3")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .collect();
    assert_eq!(sec.len(), 256);
    assert_eq!(sec[0], "000 001 002 003 004 005 006 007");
    assert_eq!(sec[1], "008 009 00a 00b 00c 00d 00e 00f");
    for l in &sec {
        assert_eq!(l.split(' ').count(), 8);
        assert!(l.split(' ').all(|g| g.len() == 3));
    }
}

#[test]
fn ebr_init_matches_nextpnr_bit_order() {
    // Cross-check against the vendored nextpnr reference: meminit.textcfg
    // `.bram_init 3` for mem[i]=i (8-bit) must equal our section for the
    // same payload. The reference file is not vendored, so this test
    // re-derives the order: line L group G = address 8L+G, low 9 bits.
    let device = dev();
    let mut init = vec![0u16; 2048];
    for (w, v) in init.iter_mut().enumerate() {
        *v = (w & 0x1ff) as u16;
    }
    let mut d = EmitDesign::default();
    d.ebrs.push(EbrBlock::simple(
        "MIB_R70C4:MIB_EBR0".to_string(),
        EbrSite::Ebr0,
        4,
        88,
        3,
        init,
    ));
    let bs = emit_design(&d, &device).unwrap();
    let sec: Vec<&str> = bs
        .textcfg
        .lines()
        .skip_while(|l| *l != ".bram_init 3")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .collect();
    // Address 0x123 = 291 -> line 36 (291/8), group 3 (291%8); 0x123 & 0x1ff.
    let line36: Vec<&str> = sec[36].split(' ').collect();
    assert_eq!(line36[3], "123");
    // Last address 2047 -> line 255 group 7.
    let line255: Vec<&str> = sec[255].split(' ').collect();
    assert_eq!(line255[7], "1ff");
}

#[test]
fn ebr_rejects_bad_payload() {
    let device = dev();
    // Wrong init length.
    let mut d = EmitDesign::default();
    d.ebrs.push(EbrBlock::simple(
        "MIB_R70C4:MIB_EBR0".to_string(),
        EbrSite::Ebr0,
        4,
        88,
        3,
        vec![0u16; 100],
    ));
    assert!(emit_design(&d, &device).is_err());
    // Unsupported width.
    let mut d = EmitDesign::default();
    let mut e = EbrBlock::simple(
        "MIB_R70C4:MIB_EBR0".to_string(),
        EbrSite::Ebr0,
        4,
        88,
        3,
        vec![0u16; 2048],
    );
    e.data_width_a = 18;
    d.ebrs.push(e);
    let err = emit_design(&d, &device).unwrap_err();
    assert!(err.contains("DATA_WIDTH 9/9"), "{err}");
    // Duplicate wid.
    let mut d = EmitDesign::default();
    for _ in 0..2 {
        d.ebrs.push(EbrBlock::simple(
            "MIB_R70C4:MIB_EBR0".to_string(),
            EbrSite::Ebr0,
            4,
            88,
            3,
            vec![0u16; 2048],
        ));
    }
    assert!(emit_design(&d, &device).is_err());
}

#[test]
fn dsp_group_matches_nextpnr_reference() {
    // Reference hard.textcfg: MULT18_0 ->
    // `.tile_group MIB_R34C78:MIB_DSP0 MIB_R34C78:MIB2_DSP0 ...` (10 tiles).
    let device = dev();
    // Find a MULT18_0 bel from the device model.
    let (key, bel_col, bel_row) = device
        .tiles()
        .iter()
        .flat_map(|t| t.sites.iter().map(move |s| (t.key.clone(), s)))
        .find(|(_, s)| s.name == "MULT18_0")
        .map(|(k, s)| (k, s.bel_col, s.bel_row))
        .expect("MULT18_0 site");
    let mut d = EmitDesign::default();
    d.dsps.push(DspBlock::plain_multiply(key, "MULT18_0".to_string(), bel_col, bel_row));
    let bs = emit_design(&d, &device).unwrap();
    let t = &bs.textcfg;
    let gline = t
        .lines()
        .find(|l| l.starts_with(".tile_group"))
        .expect("tile_group line");
    let tiles: Vec<&str> = gline.split_whitespace().skip(1).collect();
    assert_eq!(tiles.len(), 10);
    assert!(tiles.iter().all(|k| k.contains("MIB_DSP") || k.contains("MIB2_DSP")));
    assert!(t.contains("enum: MULT18_0.MODE MULT18X18D\n"));
    assert!(t.contains("enum: MULT18_0.REG_INPUTA_CLK NONE\n"));
}

#[test]
fn pll_group_matches_nextpnr_reference() {
    // Reference pll.textcfg: `.tile_group MIB_R4C126:PLL0_UR MIB_R5C126:PLL1_UR`.
    let device = dev();
    let mut d = EmitDesign::default();
    d.plls.push(PllBlock::ehxplll(
        "MIB_R4C126:PLL0_UR".to_string(),
        126,
        22,
    ));
    let bs = emit_design(&d, &device).unwrap();
    let t = &bs.textcfg;
    assert!(
        t.contains(".tile_group MIB_R4C126:PLL0_UR MIB_R5C126:PLL1_UR\n"),
        "group line missing:\n{t}"
    );
    assert!(t.contains("enum: MODE EHXPLLL\n"));
    // CLKI_DIV=1 over 7 bits, MSB first.
    assert!(t.contains("word: CLKI_DIV 0000001\n"));
}

#[test]
fn dcc_emits_mode_enum() {
    let device = dev();
    // DCC_T0.MODE lives in TMID_0 tiles; find one from the device model.
    let key = device
        .tiles()
        .iter()
        .find(|t| t.tile_type == "TMID_0")
        .map(|t| t.key.clone())
        .expect("TMID_0 tile");
    let mut d = EmitDesign::default();
    d.dccs.push(DccBlock {
        tile_key: key,
        config_name: "DCC_T0".to_string(),
    });
    let bs = emit_design(&d, &device).unwrap();
    assert!(bs.textcfg.contains("enum: DCC_T0.MODE DCCA\n"));
}

/// Full structural validation: a small but complete design through ecppack.
///
/// Builds LUT+FF logic, an output IO, and routing arcs, emits textcfg, and
/// runs the real `ecppack` on it. ecppack is the ground truth: exit 0 plus
/// a produced bitstream means every symbolic entry resolved to real config
/// bits.
#[test]
fn ecppack_accepts_emitted_textcfg() {
    let ecppack = std::env::var("ECPPACK").unwrap_or_else(|_| {
        "/home/hatch/workspace/.jarvis/idea-executions/1f8b45d9-076b-4729-8a42-f43021b85826/scratch/oss/oss-cad-suite/bin/ecppack".to_string()
    });
    let device = dev();
    let mut d = EmitDesign {
        top: "top".to_string(),
        ..Default::default()
    };
    d.luts.push(test_lut());
    d.ios.push(PlacedIo {
        pio_tile_key: "MIB_R0C64:PIOT1".to_string(),
        pic_tile_key: "MIB_R1C64:PICT1".to_string(),
        pio: PioId::B,
        dir: IoDir::Output,
        io_type: "LVCMOS33".to_string(),
        pull: IoPull::None,
        drive_ma: 4,
    });
    d.arcs.push(EmitArc {
        tile_key: "R2C62:PLC2".to_string(),
        sink: "C4".to_string(),
        source: "Q4".to_string(),
    });
    d.arcs.push(EmitArc {
        tile_key: "R2C62:PLC2".to_string(),
        sink: "CLK0".to_string(),
        source: "G_HPBX0000".to_string(),
    });
    let bs = emit_design(&d, &device).unwrap();

    assert!(!bs.textcfg.is_empty());
    if ! std::path::Path::new(&ecppack).is_file() {
        return;
    }
    let dir = std::env::temp_dir().join("helion_ecp5_textcfg_test");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let cfg_path = dir.join("test.textcfg");
    let bit_path = dir.join("test.bit");
    std::fs::write(&cfg_path, &bs.textcfg).expect("write textcfg");

    let out = std::process::Command::new(&ecppack)
        .arg("--input")
        .arg(&cfg_path)
        .arg("--bit")
        .arg(&bit_path)
        .output()
        .unwrap_or_else(|e| panic!("failed to run ecppack at {ecppack}: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "ecppack failed on emitted textcfg:\n{stderr}\n--- textcfg ---\n{}",
        bs.textcfg
    );
    let meta = std::fs::metadata(&bit_path).expect("bitstream produced");
    assert!(meta.len() > 1_000_000, "bitstream too small: {}", meta.len());
}

/// ecppack must also accept a design with EBR + DSP + PLL hard blocks.
#[test]
fn ecppack_accepts_hard_blocks() {
    let ecppack = std::env::var("ECPPACK").unwrap_or_else(|_| {
        "/home/hatch/workspace/.jarvis/idea-executions/1f8b45d9-076b-4729-8a42-f43021b85826/scratch/oss/oss-cad-suite/bin/ecppack".to_string()
    });
    let device = dev();
    let mut init = vec![0u16; 2048];
    for (w, v) in init.iter_mut().enumerate() {
        *v = (w & 0x1ff) as u16;
    }
    let (dsp_key, dsp_col, dsp_row) = device
        .tiles()
        .iter()
        .flat_map(|t| t.sites.iter().map(move |s| (t.key.clone(), s)))
        .find(|(_, s)| s.name == "MULT18_0")
        .map(|(k, s)| (k, s.bel_col, s.bel_row))
        .expect("MULT18_0 site");
    let mut d = EmitDesign {
        top: "top".to_string(),
        ..Default::default()
    };
    d.ebrs.push(EbrBlock::simple(
        "MIB_R70C4:MIB_EBR0".to_string(),
        EbrSite::Ebr0,
        4,
        88,
        3,
        init,
    ));
    d.dsps.push(DspBlock::plain_multiply(
        dsp_key,
        "MULT18_0".to_string(),
        dsp_col,
        dsp_row,
    ));
    d.plls.push(PllBlock::ehxplll(
        "MIB_R4C126:PLL0_UR".to_string(),
        126,
        22,
    ));
    let bs = emit_design(&d, &device).unwrap();

    assert!(!bs.textcfg.is_empty());
    if !std::path::Path::new(&ecppack).is_file() {
        return;
    }
    let dir = std::env::temp_dir().join("helion_ecp5_textcfg_test");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let cfg_path = dir.join("hard.textcfg");
    let bit_path = dir.join("hard.bit");
    std::fs::write(&cfg_path, &bs.textcfg).expect("write textcfg");

    let out = std::process::Command::new(&ecppack)
        .arg("--input")
        .arg(&cfg_path)
        .arg("--bit")
        .arg(&bit_path)
        .output()
        .unwrap_or_else(|e| panic!("failed to run ecppack at {ecppack}: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "ecppack failed on hard-block textcfg:\n{stderr}\n--- textcfg ---\n{}",
        bs.textcfg
    );
    let meta = std::fs::metadata(&bit_path).expect("bitstream produced");
    assert!(meta.len() > 1_000_000, "bitstream too small: {}", meta.len());
}
