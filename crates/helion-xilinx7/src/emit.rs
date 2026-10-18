//! Emit stage: [`Routed7`] → FASM text ([`Bitstream7`]).
//!
//! Walks the placed+routed design and emits prjxray-compatible FASM lines
//! for every configured UG953 primitive and routing pip, sorted
//! deterministically.

use crate::pack::{
    BramKind, CascadeOrder, ClockBufKind, CmtKind, DdrClkEdge, IobDir, IodelayKind, MuxfKind,
    RamMode, RamWriteMode, SrlKind, SrType,
};
use crate::place::Placed7;
use crate::route::{RouteKind, Routed7};

/// What the emitter needs from the device model.
pub trait EmitDevice {
    /// Tile type for a tile name, e.g. `"CLBLL_L_X14Y100"` → `"CLBLL_L"`.
    fn tile_type(&self, tile: &str) -> Option<String>;
    /// Site name relative to its tile for FASM.
    fn site_rel(&self, tile: &str, site: &str) -> Option<String>;
}

/// IBUF `.IN` feature prefix for an IOSTANDARD, from `segbits_liob33.db`.
fn ibuf_in_prefix(iostandard: &str) -> Result<&'static str, String> {
    match iostandard.to_ascii_uppercase().as_str() {
        "LVCMOS12" | "LVCMOS15" | "LVCMOS18" => Ok("LVCMOS12_LVCMOS15_LVCMOS18"),
        "LVCMOS25" | "LVCMOS33" | "LVTTL" => Ok("LVCMOS25_LVCMOS33_LVTTL"),
        "LVDS_25" | "SSTL135" | "SSTL15" => Ok("LVDS_25_SSTL135_SSTL15"),
        "TMDS_33" => Ok("TMDS_33"),
        other => Err(format!("emit: unsupported input IOSTANDARD {other}")),
    }
}

/// OBUF `.DRIVE` feature prefix for an `(IOSTANDARD, DRIVE)` pair, from `segbits_liob33.db`.
fn obuf_drive_prefix(iostandard: &str, drive: &str) -> Result<&'static str, String> {
    let std = iostandard.to_ascii_uppercase();
    let drv = drive.to_ascii_uppercase();
    let prefix = match (std.as_str(), drv.as_str()) {
        ("LVCMOS12", "I4" | "I12") => "LVCMOS12",
        ("LVCMOS12", "I8") => "LVCMOS12_LVCMOS25",
        ("LVCMOS15", "I8" | "I12") => "LVCMOS15",
        ("LVCMOS15", "I4") => "LVCMOS15_LVCMOS18_LVCMOS25",
        ("LVCMOS15", "I16") => "LVCMOS15_SSTL15",
        ("LVCMOS18", "I12" | "I16" | "I24") => "LVCMOS18",
        ("LVCMOS18", "I4") => "LVCMOS15_LVCMOS18_LVCMOS25",
        ("LVCMOS25", "I12" | "I16") => "LVCMOS25",
        ("LVCMOS25", "I4") => "LVCMOS15_LVCMOS18_LVCMOS25",
        ("LVCMOS25", "I8") => "LVCMOS12_LVCMOS25",
        ("LVCMOS33", "I16") => "LVCMOS33",
        ("LVCMOS33", "I4" | "I12") => "LVCMOS33_LVTTL",
        ("LVTTL", "I24") => "LVTTL",
        ("LVTTL", "I4" | "I12") => "LVCMOS33_LVTTL",
        ("LVDS_25", "I") => "LVDS_25",
        ("SSTL135", "I") => "SSTL135",
        ("TMDS_33", "I") => "TMDS_33",
        _ => {
            return Err(format!(
                "emit: unsupported (IOSTANDARD, DRIVE) = ({iostandard}, {drive})"
            ))
        }
    };
    Ok(prefix)
}

/// Backend-owned bitstream: FASM text plus assembled configuration frames.
#[derive(Clone, Debug, Default)]
pub struct Bitstream7 {
    /// FASM feature lines, one per line.
    pub fasm: Vec<String>,
    /// Assembled configuration frames (101 words each).
    pub frames: Vec<u32>,
}

/// Encode an MMCM/PLL divider `div` and duty cycle `duty` into prjxray
/// `(HIGH_TIME[5:0], LOW_TIME[5:0], NO_COUNT, EDGE)` DRP register fields.
pub fn encode_cmt_divider(div: u32, duty: f64) -> (u8, u8, bool, bool) {
    if div <= 1 {
        return (1, 1, true, false);
    }
    let high = ((div as f64) * duty).round().clamp(1.0, (div - 1) as f64) as u32;
    let low = (div - high).max(1);
    let edge = high > low;
    ((high & 0x3F) as u8, (low & 0x3F) as u8, false, edge)
}

/// Emit FASM for a routed design.
pub fn emit_fasm(routed: &Routed7, device: &dyn EmitDevice) -> Result<Bitstream7, String> {
    let placed = &routed.placed;
    let packed = &placed.packed;
    let mut lines: Vec<String> = Vec::new();

    // 1. LUTs: INIT vector per occupied ALUT..DLUT.
    for lut in &placed.luts {
        let site_rel = device
            .site_rel(&lut.tile, &lut.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", lut.tile, lut.site))?;
        let bel = lut_bel(&lut.bel)?;
        lines.push(format!(
            "{}.{}.{}.INIT[63:0] = 64'h{:016X}",
            lut.tile, site_rel, bel, lut.init
        ));
    }

    // 2. SRL16E / SRLC32E in SLICEM sites.
    for srl in &placed.srls {
        let site_rel = device
            .site_rel(&srl.tile, &srl.site)
            .unwrap_or_else(|| "SLICEM_X0".to_string());
        let bel = lut_bel(&srl.bel)?;
        // Expand 16-bit SRL16E INIT (duplicated into upper/lower 32 bits for O6) or 32-bit SRLC32E INIT.
        let init64 = match srl.kind {
            SrlKind::Srl16e => {
                let mut v = 0u64;
                for i in 0..16 {
                    if ((srl.init >> i) & 1) != 0 {
                        // A0..A3 map to LUT inputs A2..A5 (with A1=1 for O6).
                        v |= 0b11u64 << (i * 2);
                    }
                }
                v | (v << 32)
            }
            SrlKind::Srlc32e => {
                let mut v = 0u64;
                for i in 0..32 {
                    if ((srl.init >> i) & 1) != 0 {
                        v |= 0b11u64 << (i * 2);
                    }
                }
                v
            }
        };
        lines.push(format!(
            "{}.{}.{}.INIT[63:0] = 64'h{:016X}",
            srl.tile, site_rel, bel, init64
        ));
        lines.push(format!("{}.{}.{}.SRL", srl.tile, site_rel, bel));
        lines.push(format!("{}.{}.WA7USED", srl.tile, site_rel));
        if srl.kind == SrlKind::Srlc32e {
            lines.push(format!("{}.{}.{}.SMALL", srl.tile, site_rel, bel));
        }
        if srl.is_clk_inverted {
            lines.push(format!("{}.{}.CLKINV", srl.tile, site_rel));
        }
        if let Some(psrl) = packed.srls.iter().find(|p| p.name == srl.name) {
            if psrl.ce_net.is_some() {
                lines.push(format!("{}.{}.WEMUX.CE", srl.tile, site_rel));
            }
        }
    }

    // 3. MUXF7 / MUXF8 wide slice multiplexers.
    for muxf in &placed.muxfs {
        let site_rel = device
            .site_rel(&muxf.tile, &muxf.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", muxf.tile, muxf.site))?;
        match muxf.kind {
            MuxfKind::Muxf7 => {
                lines.push(format!("{}.{}.{}.IN_USE", muxf.tile, site_rel, muxf.bel));
                let out_mux = if muxf.bel == "F7BMUX" { "CMUX" } else { "AMUX" };
                lines.push(format!("{}.{}.{}.F7", muxf.tile, site_rel, out_mux));
            }
            MuxfKind::Muxf8 => {
                lines.push(format!("{}.{}.F8MUX.IN_USE", muxf.tile, site_rel));
                lines.push(format!("{}.{}.BMUX.F8", muxf.tile, site_rel));
            }
        }
    }

    // 4. FFs: ZINI + D-mux select.
    for ff in &placed.ffs {
        let site_rel = device
            .site_rel(&ff.tile, &ff.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", ff.tile, ff.site))?;
        let bel = ff_bel(&ff.bel)?;
        let mux_src = mux_for_ff(ff, packed, placed);
        lines.push(format!("{}.{}.{}.ZINI", ff.tile, site_rel, bel));
        lines.push(format!("{}.{}.{}MUX.{}", ff.tile, site_rel, bel, mux_src));
    }

    // 5. IOBs.
    for iob in &placed.iobs {
        let site_rel = device
            .site_rel(&iob.tile, &iob.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", iob.tile, iob.site))?;
        let p = packed
            .iobs
            .iter()
            .find(|p| p.name == iob.name)
            .ok_or_else(|| format!("emit: packed IOB {} not found", iob.name))?;
        match iob.dir {
            IobDir::In => {
                let prefix = ibuf_in_prefix(&p.iostandard)?;
                lines.push(format!("{}.{}.{}.IN", iob.tile, site_rel, prefix));
                lines.push(format!(
                    "{}.{}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVDS_25_LVTTL_SSTL135_SSTL15_TMDS_33.IN_ONLY",
                    iob.tile, site_rel
                ));
            }
            IobDir::Out => {
                let drive = p.drive.as_deref().unwrap_or("I16");
                let prefix = obuf_drive_prefix(&p.iostandard, drive)?;
                lines.push(format!(
                    "{}.{}.{}.DRIVE.{}",
                    iob.tile,
                    site_rel,
                    prefix,
                    drive.to_ascii_uppercase()
                ));
                let slew = p.slew.as_deref().unwrap_or("SLOW");
                lines.push(format!(
                    "{}.{}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVTTL_SSTL135_SSTL15.SLEW.{}",
                    iob.tile,
                    site_rel,
                    slew.to_ascii_uppercase()
                ));
            }
        }
    }

    // 6. Clock buffers (BUFG, BUFGCE, BUFGCTRL, BUFIO, BUFR).
    for bufg in &placed.bufgs {
        let pbufg = packed.bufgs.iter().find(|b| b.name == bufg.name);
        let kind = pbufg.map(|b| b.kind).unwrap_or(ClockBufKind::Bufg);
        match kind {
            ClockBufKind::Bufio => {
                let rel = if bufg.site.ends_with("Y0") || bufg.site.ends_with("Y2") {
                    "BUFIO_X0Y0"
                } else {
                    "BUFIO_X0Y1"
                };
                lines.push(format!("{}.{}.IN_USE", bufg.tile, rel));
            }
            ClockBufKind::Bufr => {
                let rel = if bufg.site.ends_with("Y0") || bufg.site.ends_with("Y2") {
                    "BUFR_Y0"
                } else {
                    "BUFR_Y1"
                };
                lines.push(format!("{}.{}.IN_USE", bufg.tile, rel));
                if let Some(pb) = pbufg {
                    let div_tok = if pb.bufr_divide.starts_with(|c: char| c.is_ascii_digit()) {
                        format!("D{}", pb.bufr_divide)
                    } else {
                        pb.bufr_divide.clone()
                    };
                    lines.push(format!("{}.{}.BUFR_DIVIDE.{}", bufg.tile, rel, div_tok));
                }
            }
            _ => {
                let site_rel = device
                    .site_rel(&bufg.tile, &bufg.site)
                    .ok_or_else(|| format!("emit: no rel site for {} {}", bufg.tile, bufg.site))?;
                lines.push(format!("{}.BUFGCTRL.{}.IN_USE", bufg.tile, site_rel));
                lines.push(format!(
                    "{}.BUFGCTRL.{}.IS_IGNORE0_INVERTED",
                    bufg.tile, site_rel
                ));
                if let Some(pb) = pbufg {
                    if pb.init_out == 1 {
                        lines.push(format!("{}.BUFGCTRL.{}.ZINIT_OUT", bufg.tile, site_rel));
                    }
                    if pb.preselect_i0 {
                        lines.push(format!("{}.BUFGCTRL.{}.PRESELECT_I0", bufg.tile, site_rel));
                    }
                    if pb.preselect_i1 {
                        lines.push(format!("{}.BUFGCTRL.{}.PRESELECT_I1", bufg.tile, site_rel));
                    }
                }
            }
        }
    }

    // 7. CMTs (MMCME2_ADV / PLLE2_ADV).
    for cmt in &placed.cmts {
        let Some(pcmt) = packed.cmts.iter().find(|c| c.name == cmt.name) else {
            continue;
        };
        let bel = match cmt.kind {
            CmtKind::MmcmE2Adv => "MMCME2_ADV",
            CmtKind::PllE2Adv => "PLLE2_ADV",
        };
        lines.push(format!("{}.{}.IN_USE", cmt.tile, bel));
        lines.push(format!(
            "{}.{}.BANDWIDTH.{}",
            cmt.tile, bel, pcmt.bandwidth
        ));
        lines.push(format!(
            "{}.{}.COMPENSATION.{}",
            cmt.tile, bel, pcmt.compensation
        ));

        let (div_hi, div_lo, div_nc, div_edge) = encode_cmt_divider(pcmt.divclk_divide, 0.5);
        lines.push(format!(
            "{}.{}.DIVCLK_DIVCLK_HIGH_TIME[5:0] = 6'h{:02X}",
            cmt.tile, bel, div_hi
        ));
        lines.push(format!(
            "{}.{}.DIVCLK_DIVCLK_LOW_TIME[5:0] = 6'h{:02X}",
            cmt.tile, bel, div_lo
        ));
        if div_nc {
            lines.push(format!("{}.{}.DIVCLK_DIVCLK_NO_COUNT", cmt.tile, bel));
        }
        if div_edge {
            lines.push(format!("{}.{}.DIVCLK_DIVCLK_EDGE", cmt.tile, bel));
        }

        let mult_int = pcmt.clkfbout_mult_f.floor() as u32;
        let (fb_hi, fb_lo, fb_nc, fb_edge) = encode_cmt_divider(mult_int, 0.5);
        lines.push(format!(
            "{}.{}.CLKFBOUT_CLKOUT1_HIGH_TIME[5:0] = 6'h{:02X}",
            cmt.tile, bel, fb_hi
        ));
        lines.push(format!(
            "{}.{}.CLKFBOUT_CLKOUT1_LOW_TIME[5:0] = 6'h{:02X}",
            cmt.tile, bel, fb_lo
        ));
        if fb_nc {
            lines.push(format!("{}.{}.CLKFBOUT_CLKOUT2_NO_COUNT", cmt.tile, bel));
        }
        if fb_edge {
            lines.push(format!("{}.{}.CLKFBOUT_CLKOUT2_EDGE", cmt.tile, bel));
        }
        if cmt.kind == CmtKind::MmcmE2Adv {
            let frac = ((pcmt.clkfbout_mult_f.fract() * 8.0).round() as u8) & 0x7;
            if frac != 0 {
                lines.push(format!("{}.{}.CLKFBOUT_CLKOUT2_FRAC_EN", cmt.tile, bel));
                lines.push(format!(
                    "{}.{}.CLKFBOUT_CLKOUT2_FRAC[2:0] = 3'b{:03b}",
                    cmt.tile, bel, frac
                ));
            }
        }

        for i in 0..7 {
            if cmt.kind == CmtKind::PllE2Adv && i >= 6 {
                break;
            }
            let (hi, lo, nc, edge) =
                encode_cmt_divider(pcmt.clkout_divide[i], pcmt.clkout_duty[i]);
            lines.push(format!(
                "{}.{}.CLKOUT{i}_CLKOUT1_HIGH_TIME[5:0] = 6'h{:02X}",
                cmt.tile, bel, hi
            ));
            lines.push(format!(
                "{}.{}.CLKOUT{i}_CLKOUT1_LOW_TIME[5:0] = 6'h{:02X}",
                cmt.tile, bel, lo
            ));
            if nc {
                lines.push(format!("{}.{}.CLKOUT{i}_CLKOUT2_NO_COUNT", cmt.tile, bel));
            }
            if edge {
                lines.push(format!("{}.{}.CLKOUT{i}_CLKOUT2_EDGE", cmt.tile, bel));
            }
            if pcmt.clkout_nets[i].is_some() {
                lines.push(format!(
                    "{}.{}.CLKOUT{i}_CLKOUT1_OUTPUT_ENABLE",
                    cmt.tile, bel
                ));
            }
        }
    }

    // 8. Block RAMs (RAMB18E1 and RAMB36E1).
    for bram in &placed.brams {
        let pbram = packed.brams.iter().find(|b| b.name == bram.name);
        let kind = pbram.map(|b| b.kind).unwrap_or(BramKind::Ramb18e1);
        match kind {
            BramKind::Ramb18e1 => {
                let site_rel = device
                    .site_rel(&bram.tile, &bram.site)
                    .unwrap_or_else(|| {
                        if bram.site.ends_with("Y1") || bram.site.ends_with("Y3") {
                            "RAMB18_Y1".to_string()
                        } else {
                            "RAMB18_Y0".to_string()
                        }
                    });
                lines.push(format!("{}.RAMB18.{}.IN_USE", bram.tile, site_rel));
                emit_bram_common_features(&mut lines, &bram.tile, &site_rel, bram, pbram);
            }
            BramKind::Ramb36e1 => {
                // In prjxray segbits_bram_l.db, RAMB36E1 activates both RAMB18_Y0 and RAMB18_Y1
                // plus RAMB36 wrapper features.
                lines.push(format!("{}.RAMB18_Y0.IN_USE", bram.tile));
                lines.push(format!("{}.RAMB18_Y1.IN_USE", bram.tile));
                lines.push(format!("{}.RAMB36.IN_USE", bram.tile));
                emit_bram_common_features(&mut lines, &bram.tile, "RAMB36", bram, pbram);
            }
        }
    }

    // 9. DSP48E1s.
    for dsp in &placed.dsps {
        let pdsp = packed.dsps.iter().find(|d| d.name == dsp.name);
        let dsp_site = if let Some(y_pos) = dsp.site.rfind('Y') {
            let y_str = &dsp.site[y_pos + 1..];
            let y: i32 = y_str.parse().unwrap_or(0);
            if y % 2 == 0 {
                "DSP_0"
            } else {
                "DSP_1"
            }
        } else {
            "DSP_0"
        };

        let (adreg, alumodereg, opmodereg, carryinreg, carryinselreg, inmodereg) = match pdsp {
            Some(p) => (
                p.adreg,
                p.alumodereg,
                p.opmodereg,
                p.carryinreg,
                p.carryinselreg,
                p.inmodereg,
            ),
            None => (0, 0, 0, 0, 0, 0),
        };
        if adreg == 0 {
            lines.push(format!("{}.{}.ZADREG", dsp.tile, dsp_site));
        }
        if alumodereg == 0 {
            lines.push(format!("{}.{}.ZALUMODEREG", dsp.tile, dsp_site));
        }
        if opmodereg == 0 {
            lines.push(format!("{}.{}.ZOPMODEREG", dsp.tile, dsp_site));
        }
        if carryinreg == 0 {
            lines.push(format!("{}.{}.ZCARRYINREG", dsp.tile, dsp_site));
        }
        if carryinselreg == 0 {
            lines.push(format!("{}.{}.ZCARRYINSELREG", dsp.tile, dsp_site));
        }
        if inmodereg == 0 {
            lines.push(format!("{}.{}.ZINMODEREG", dsp.tile, dsp_site));
        }

        if let Some(p) = pdsp {
            if p.areg > 0 {
                lines.push(format!("{}.{}.AREG_{}", dsp.tile, dsp_site, p.areg));
            }
            if p.breg > 0 {
                lines.push(format!("{}.{}.BREG_{}", dsp.tile, dsp_site, p.breg));
            }
            if p.creg > 0 {
                lines.push(format!("{}.{}.CREG", dsp.tile, dsp_site));
            }
            if p.dreg > 0 {
                lines.push(format!("{}.{}.DREG", dsp.tile, dsp_site));
            }
            if p.mreg > 0 {
                lines.push(format!("{}.{}.MREG", dsp.tile, dsp_site));
            }
            if p.preg > 0 {
                lines.push(format!("{}.{}.PREG", dsp.tile, dsp_site));
            }
            if p.use_dport {
                lines.push(format!("{}.{}.USE_DPORT", dsp.tile, dsp_site));
            }
            if p.use_mult != "MULTIPLY" {
                lines.push(format!("{}.{}.USE_MULT.{}", dsp.tile, dsp_site, p.use_mult));
            }
            if p.use_simd != "ONE48" {
                lines.push(format!("{}.{}.USE_SIMD.{}", dsp.tile, dsp_site, p.use_simd));
            }
            if p.a_input_cascade || p.acin_net.is_some() {
                lines.push(format!("{}.{}.A_INPUT.CASCADE", dsp.tile, dsp_site));
            }
            if p.b_input_cascade || p.bcin_net.is_some() {
                lines.push(format!("{}.{}.B_INPUT.CASCADE", dsp.tile, dsp_site));
            }
            if p.use_pattern_detect {
                lines.push(format!("{}.{}.USE_PATTERN_DETECT.PATDET", dsp.tile, dsp_site));
                lines.push(format!(
                    "{}.{}.PATTERN[47:0] = 48'h{:012X}",
                    dsp.tile, dsp_site, p.pattern
                ));
                lines.push(format!(
                    "{}.{}.MASK[47:0] = 48'h{:012X}",
                    dsp.tile, dsp_site, p.mask
                ));
                lines.push(format!(
                    "{}.{}.SEL_PATTERN.{}",
                    dsp.tile, dsp_site, p.sel_pattern
                ));
                lines.push(format!("{}.{}.SEL_MASK.{}", dsp.tile, dsp_site, p.sel_mask));
            }
            for i in 0..5 {
                if ((p.inmode >> i) & 1) == 1 {
                    lines.push(format!("{}.{}.INMODE[{}]", dsp.tile, dsp_site, i));
                }
            }
        }

        for i in 0..7 {
            if ((dsp.opmode >> i) & 1) == 0 {
                lines.push(format!(
                    "{}.{}.ZIS_OPMODE_INVERTED[{}]",
                    dsp.tile, dsp_site, i
                ));
            }
        }
        for i in 0..4 {
            if ((dsp.alumode >> i) & 1) == 0 {
                lines.push(format!(
                    "{}.{}.ZIS_ALUMODE_INVERTED[{}]",
                    dsp.tile, dsp_site, i
                ));
            }
        }
    }

    // 10. CARRY4s.
    for carry in &placed.carries {
        let site_rel = device
            .site_rel(&carry.tile, &carry.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", carry.tile, carry.site))?;
        lines.push(format!("{}.{}.CARRY4.IN_USE", carry.tile, site_rel));
        if let Some(pcarry) = packed.carries.iter().find(|c| c.name == carry.name) {
            if pcarry.ci_net.is_some() {
                lines.push(format!("{}.{}.PRECYINIT.CIN", carry.tile, site_rel));
            } else if let Some(cy) = &pcarry.cyinit_net {
                match cy.as_str() {
                    "1" | "vcc" | "VCC" | "$true" => {
                        lines.push(format!("{}.{}.PRECYINIT.C1", carry.tile, site_rel));
                    }
                    "0" | "gnd" | "GND" | "$false" => {
                        lines.push(format!("{}.{}.PRECYINIT.C0", carry.tile, site_rel));
                    }
                    _ => {
                        lines.push(format!("{}.{}.PRECYINIT.AX", carry.tile, site_rel));
                    }
                }
            } else {
                lines.push(format!("{}.{}.PRECYINIT.C0", carry.tile, site_rel));
            }
            for (i, letter) in ['A', 'B', 'C', 'D'].iter().enumerate() {
                if pcarry.di_nets[i].is_some() {
                    lines.push(format!("{}.{}.CARRY4.{}CY0", carry.tile, site_rel, letter));
                }
            }
        }
    }

    // 11. IDDR / ODDR in ILOGICE2 / OLOGICE2.
    for ddr in &placed.ioddrs {
        let Some(pddr) = packed.ioddrs.iter().find(|d| d.name == ddr.name) else {
            continue;
        };
        let y_suffix = if ddr.site.ends_with("Y1") || ddr.site.ends_with("Y3") {
            "Y1"
        } else {
            "Y0"
        };
        if ddr.is_input {
            let bel = format!("ILOGIC_{y_suffix}");
            lines.push(format!("{}.{}.IDDR_OR_ISERDES.IN_USE", ddr.tile, bel));
            lines.push(format!("{}.{}.IFF.ZINV_C", ddr.tile, bel));
            match pddr.ddr_clk_edge {
                DdrClkEdge::OppositeEdge => {
                    lines.push(format!(
                        "{}.{}.IFF.DDR_CLK_EDGE.OPPOSITE_EDGE",
                        ddr.tile, bel
                    ));
                }
                DdrClkEdge::SameEdge => {
                    lines.push(format!("{}.{}.IFF.DDR_CLK_EDGE.SAME_EDGE", ddr.tile, bel));
                }
                DdrClkEdge::SameEdgePipelined => {
                    lines.push(format!(
                        "{}.{}.IFF.DDR_CLK_EDGE.SAME_EDGE_PIPELINED",
                        ddr.tile, bel
                    ));
                }
            }
            if pddr.srtype == SrType::Sync {
                lines.push(format!("{}.{}.IFF.SRTYPE.SYNC", ddr.tile, bel));
            }
            if pddr.init_q1 == 0 {
                lines.push(format!("{}.{}.IFF.ZINIT_Q1", ddr.tile, bel));
            }
            if pddr.init_q2 == 0 {
                lines.push(format!("{}.{}.IFF.ZINIT_Q2", ddr.tile, bel));
            }
        } else {
            let bel = format!("OLOGIC_{y_suffix}");
            lines.push(format!("{}.{}.ODDR.IN_USE", ddr.tile, bel));
            lines.push(format!("{}.{}.OMUX.D1", ddr.tile, bel));
            lines.push(format!("{}.{}.OQUSED", ddr.tile, bel));
            match pddr.ddr_clk_edge {
                DdrClkEdge::OppositeEdge => {
                    lines.push(format!(
                        "{}.{}.ODDR_DDR_CLK_EDGE.OPPOSITE_EDGE",
                        ddr.tile, bel
                    ));
                }
                DdrClkEdge::SameEdge | DdrClkEdge::SameEdgePipelined => {
                    lines.push(format!("{}.{}.ODDR_DDR_CLK_EDGE.SAME_EDGE", ddr.tile, bel));
                }
            }
            if pddr.srtype == SrType::Sync {
                lines.push(format!("{}.{}.OSERDES.SRTYPE.SYNC", ddr.tile, bel));
            }
            if pddr.init_q1 == 0 {
                lines.push(format!("{}.{}.ZINIT_OQ", ddr.tile, bel));
            }
        }
    }

    // 12. ISERDESE2 / OSERDESE2.
    for ser in &placed.serdes {
        let Some(pser) = packed.serdes.iter().find(|s| s.name == ser.name) else {
            continue;
        };
        let y_suffix = if ser.site.ends_with("Y1") || ser.site.ends_with("Y3") {
            "Y1"
        } else {
            "Y0"
        };
        if ser.is_input {
            let bel = format!("ILOGIC_{y_suffix}");
            lines.push(format!("{}.{}.IDDR_OR_ISERDES.IN_USE", ser.tile, bel));
            lines.push(format!("{}.{}.ISERDES.IN_USE", ser.tile, bel));
            lines.push(format!(
                "{}.{}.ISERDES.INTERFACE_TYPE.{}",
                ser.tile, bel, pser.interface_type
            ));
            lines.push(format!(
                "{}.{}.ISERDES.DATA_RATE.{}",
                ser.tile, bel, pser.data_rate
            ));
            lines.push(format!(
                "{}.{}.ISERDES.DATA_WIDTH.W{}",
                ser.tile, bel, pser.data_width
            ));
            lines.push(format!(
                "{}.{}.ISERDES.SERDES_MODE.{}",
                ser.tile, bel, pser.serdes_mode
            ));
            lines.push(format!(
                "{}.{}.IDELAYMUX.{}",
                ser.tile, bel, pser.iobdelay
            ));
        } else {
            let bel = format!("OLOGIC_{y_suffix}");
            lines.push(format!("{}.{}.OSERDES.IN_USE", ser.tile, bel));
            lines.push(format!(
                "{}.{}.OSERDES.DATA_RATE_OQ.{}",
                ser.tile, bel, pser.data_rate
            ));
            lines.push(format!(
                "{}.{}.OSERDES.DATA_WIDTH.W{}",
                ser.tile, bel, pser.data_width
            ));
            lines.push(format!(
                "{}.{}.OSERDES.SERDES_MODE.{}",
                ser.tile, bel, pser.serdes_mode
            ));
            lines.push(format!(
                "{}.{}.OSERDES.TRISTATE_WIDTH.W{}",
                ser.tile, bel, pser.tristate_width
            ));
        }
    }

    // 13. IDELAYE2 / ODELAYE2 / IDELAYCTRL.
    for dly in &placed.iodelays {
        let Some(pdly) = packed.iodelays.iter().find(|d| d.name == dly.name) else {
            continue;
        };
        if dly.kind == IodelayKind::IdelayCtrl {
            lines.push(format!("{}.IDELAYCTRL.IN_USE", dly.tile));
            lines.push(format!("{}.IDELAYCTRL.BIAS_MODE", dly.tile));
            continue;
        }
        let y_suffix = if dly.site.ends_with("Y1") || dly.site.ends_with("Y3") {
            "Y1"
        } else {
            "Y0"
        };
        let bel_prefix = match dly.kind {
            IodelayKind::IdelayE2 => format!("IDELAY_{y_suffix}"),
            IodelayKind::OdelayE2 => format!("ODELAY_{y_suffix}"),
            IodelayKind::IdelayCtrl => unreachable!(),
        };
        lines.push(format!("{}.{}.IN_USE", dly.tile, bel_prefix));
        lines.push(format!(
            "{}.{}.IDELAY_TYPE.{}",
            dly.tile, bel_prefix, pdly.delay_type
        ));
        lines.push(format!(
            "{}.{}.IDELAY_VALUE[4:0] = 5'b{:05b}",
            dly.tile,
            bel_prefix,
            pdly.delay_value & 0x1F
        ));
        lines.push(format!(
            "{}.{}.ZIDELAY_VALUE[4:0] = 5'b{:05b}",
            dly.tile,
            bel_prefix,
            (!pdly.delay_value) & 0x1F
        ));
        lines.push(format!(
            "{}.{}.DELAY_SRC.{}",
            dly.tile, bel_prefix, pdly.delay_src
        ));
        if pdly.high_performance_mode {
            lines.push(format!("{}.{}.HIGH_PERFORMANCE_MODE", dly.tile, bel_prefix));
        }
        if pdly.cinvctrl_sel {
            lines.push(format!("{}.{}.CINVCTRL_SEL", dly.tile, bel_prefix));
        }
        if pdly.pipe_sel {
            lines.push(format!("{}.{}.PIPE_SEL", dly.tile, bel_prefix));
        }
    }

    // 13b. GTPE2_COMMON, GTPE2_CHANNEL, IBUFDS_GTE2, PCIE_2_1, XADC, and CFG_CENTER_MID primitives.
    for ibuf in &placed.ibufds_gte2s {
        if let Some(pib) = packed.ibufds_gte2s.iter().find(|x| x.name == ibuf.name) {
            lines.push(format!("{}.{}.IN_USE", ibuf.tile, ibuf.site));
            if pib.clkcm_cfg {
                lines.push(format!("{}.{}.CLKCM_CFG", ibuf.tile, ibuf.site));
            }
            if pib.clkrcv_trst {
                lines.push(format!("{}.{}.CLKRCV_TRST", ibuf.tile, ibuf.site));
            }
            lines.push(format!(
                "{}.{}.CLKSWING_CFG[1:0] = 2'b{:02b}",
                ibuf.tile, ibuf.site, pib.clkswing_cfg
            ));
        }
    }
    for gtc in &placed.gtp_commons {
        if let Some(pg) = packed.gtp_commons.iter().find(|x| x.name == gtc.name) {
            lines.push(format!("{}.GTPE2_COMMON.IN_USE", gtc.tile));
            lines.push(format!(
                "{}.GTPE2_COMMON.PLL0_FBDIV[5:0] = 6'd{}",
                gtc.tile, pg.pll0_fbdiv
            ));
            lines.push(format!(
                "{}.GTPE2_COMMON.PLL0_FBDIV_45.D{}",
                gtc.tile, pg.pll0_fbdiv_45
            ));
            lines.push(format!(
                "{}.GTPE2_COMMON.PLL0_REFCLK_DIV.D{}",
                gtc.tile, pg.pll0_refclk_div
            ));
            lines.push(format!(
                "{}.GTPE2_COMMON.PLL1_FBDIV[5:0] = 6'd{}",
                gtc.tile, pg.pll1_fbdiv
            ));
            lines.push(format!(
                "{}.GTPE2_COMMON.PLL1_FBDIV_45.D{}",
                gtc.tile, pg.pll1_fbdiv_45
            ));
            lines.push(format!(
                "{}.GTPE2_COMMON.PLL1_REFCLK_DIV.D{}",
                gtc.tile, pg.pll1_refclk_div
            ));
        }
    }
    for gch in &placed.gtp_channels {
        if let Some(pg) = packed.gtp_channels.iter().find(|x| x.name == gch.name) {
            lines.push(format!("{}.GTPE2_CHANNEL.IN_USE", gch.tile));
            lines.push(format!(
                "{}.GTPE2_CHANNEL.RXOUT_DIV.D{}",
                gch.tile, pg.rxout_div
            ));
            lines.push(format!(
                "{}.GTPE2_CHANNEL.TXOUT_DIV.D{}",
                gch.tile, pg.txout_div
            ));
            lines.push(format!(
                "{}.GTPE2_CHANNEL.RX_DATA_WIDTH.W{}",
                gch.tile, pg.rx_data_width
            ));
            lines.push(format!(
                "{}.GTPE2_CHANNEL.TX_DATA_WIDTH.W{}",
                gch.tile, pg.tx_data_width
            ));
            if pg.rx_int_datawidth == 1 {
                lines.push(format!("{}.GTPE2_CHANNEL.RX_INT_DATAWIDTH", gch.tile));
            }
            if pg.tx_int_datawidth == 1 {
                lines.push(format!("{}.GTPE2_CHANNEL.TX_INT_DATAWIDTH", gch.tile));
            }
        }
    }
    for pcie in &placed.pcies {
        if let Some(pp) = packed.pcies.iter().find(|x| x.name == pcie.name) {
            lines.push(format!("{}.PCIE_2_1.IN_USE", pcie.tile));
            lines.push(format!(
                "{}.PCIE_2_1.LINK_CAP_MAX_LINK_SPEED.GEN{}",
                pcie.tile, pp.max_link_speed
            ));
            lines.push(format!(
                "{}.PCIE_2_1.LINK_CAP_MAX_LINK_WIDTH.X{}",
                pcie.tile, pp.max_link_width
            ));
            lines.push(format!(
                "{}.PCIE_2_1.DEV_CAP_MAX_PAYLOAD_SUPPORTED[2:0] = 3'd{}",
                pcie.tile,
                pp.max_payload_supported & 0x7
            ));
            for (i, &bval) in pp.bar.iter().enumerate() {
                if bval != 0 {
                    lines.push(format!(
                        "{}.PCIE_2_1.BAR{i}[31:0] = 32'h{bval:08X}",
                        pcie.tile
                    ));
                }
            }
        }
    }
    for xadc in &placed.xadcs {
        if let Some(px) = packed.xadcs.iter().find(|x| x.name == xadc.name) {
            lines.push(format!("{}.XADC.IN_USE", xadc.tile));
            for (&reg, &val) in &px.init_regs {
                lines.push(format!(
                    "{}.XADC.INIT_{reg:02X}[15:0] = 16'h{val:04X}",
                    xadc.tile
                ));
            }
        }
    }
    for cfg in &placed.cfg_prims {
        if let Some(pc) = packed.cfg_prims.iter().find(|x| x.name == cfg.name) {
            match cfg.kind {
                crate::pack::CfgPrimKind::Bscane2 => {
                    lines.push(format!(
                        "{}.BSCAN.JTAG_CHAIN_{}.IN_USE",
                        cfg.tile, pc.jtag_chain
                    ));
                    if pc.disable_jtag {
                        lines.push(format!("{}.BSCAN.DISABLE_JTAG", cfg.tile));
                    }
                }
                crate::pack::CfgPrimKind::Icape2 => {
                    lines.push(format!("{}.ICAP.IN_USE", cfg.tile));
                    lines.push(format!("{}.ICAP.ICAP_WIDTH.{}", cfg.tile, pc.icap_width));
                    lines.push(format!(
                        "{}.ICAP.DEVICE_ID[31:0] = 32'h{:08X}",
                        cfg.tile, pc.device_id
                    ));
                }
                crate::pack::CfgPrimKind::DnaPort => {
                    lines.push(format!("{}.DNA_PORT.IN_USE", cfg.tile));
                    if pc.sim_dna_value != 0 {
                        lines.push(format!(
                            "{}.DNA_PORT.SIM_DNA_VALUE[56:0] = 57'h{:015X}",
                            cfg.tile, pc.sim_dna_value
                        ));
                    }
                }
                crate::pack::CfgPrimKind::FrameEcce2 => {
                    lines.push(format!("{}.FRAME_ECC.IN_USE", cfg.tile));
                    lines.push(format!("{}.FRAME_ECC.FARSRC.{}", cfg.tile, pc.farsrc));
                }
                crate::pack::CfgPrimKind::Startupe2 => {
                    lines.push(format!("{}.STARTUP.IN_USE", cfg.tile));
                    if pc.prog_usr {
                        lines.push(format!("{}.STARTUP.PROG_USR", cfg.tile));
                    }
                }
                crate::pack::CfgPrimKind::UsrAccesse2 => {
                    lines.push(format!("{}.USR_ACCESS.IN_USE", cfg.tile));
                }
                crate::pack::CfgPrimKind::Capturee2 => {
                    lines.push(format!("{}.CAPTURE.IN_USE", cfg.tile));
                    if pc.oneshot {
                        lines.push(format!("{}.CAPTURE.ONESHOT", cfg.tile));
                    }
                }
            }
        }
    }

    // 14. Routing pips: TILE.DST.SRC.
    for net in &routed.nets {
        if let RouteKind::Pips(pips) = &net.kind {
            for pip in pips {
                if !pip.is_pseudo {
                    lines.push(format!("{}.{}.{}", pip.tile, pip.dst_wire, pip.src_wire));
                }
            }
        }
    }

    lines.sort();
    lines.dedup();
    Ok(Bitstream7 {
        fasm: lines,
        frames: Vec::new(),
    })
}

fn emit_bram_common_features(
    lines: &mut Vec<String>,
    tile: &str,
    prefix: &str,
    bram: &crate::place::PlacedBram,
    pbram: Option<&crate::pack::PackedBram>,
) {
    if bram.read_width_a > 0 {
        lines.push(format!(
            "{}.{}.READ_WIDTH_A_{}",
            tile, prefix, bram.read_width_a
        ));
    }
    if bram.read_width_b > 0 {
        lines.push(format!(
            "{}.{}.READ_WIDTH_B_{}",
            tile, prefix, bram.read_width_b
        ));
    }
    if bram.write_width_a > 0 {
        lines.push(format!(
            "{}.{}.WRITE_WIDTH_A_{}",
            tile, prefix, bram.write_width_a
        ));
    }
    if bram.write_width_b > 0 {
        lines.push(format!(
            "{}.{}.WRITE_WIDTH_B_{}",
            tile, prefix, bram.write_width_b
        ));
    }
    let Some(pb) = pbram else {
        return;
    };
    if pb.ram_mode == RamMode::Sdp {
        lines.push(format!("{}.{}.SDP_READ_WIDTH_36", tile, prefix));
        lines.push(format!("{}.{}.SDP_WRITE_WIDTH_36", tile, prefix));
    }
    let wm_str = |wm: RamWriteMode| match wm {
        RamWriteMode::WriteFirst => "WRITE_FIRST",
        RamWriteMode::ReadFirst => "READ_FIRST",
        RamWriteMode::NoChange => "NO_CHANGE",
    };
    lines.push(format!(
        "{}.{}.WRITE_MODE_A_{}",
        tile,
        prefix,
        wm_str(pb.write_mode_a)
    ));
    lines.push(format!(
        "{}.{}.WRITE_MODE_B_{}",
        tile,
        prefix,
        wm_str(pb.write_mode_b)
    ));
    if pb.doa_reg == 1 {
        lines.push(format!("{}.{}.DOA_REG", tile, prefix));
    }
    if pb.dob_reg == 1 {
        lines.push(format!("{}.{}.DOB_REG", tile, prefix));
    }
    match pb.cascade_order_a {
        CascadeOrder::None => {}
        CascadeOrder::First => lines.push(format!("{}.{}.RAM_EXTENSION_A_LOWER", tile, prefix)),
        CascadeOrder::Middle => lines.push(format!("{}.{}.RAM_EXTENSION_A_BOTH", tile, prefix)),
        CascadeOrder::Last => lines.push(format!("{}.{}.RAM_EXTENSION_A_UPPER", tile, prefix)),
    }
    match pb.cascade_order_b {
        CascadeOrder::None => {}
        CascadeOrder::First => lines.push(format!("{}.{}.RAM_EXTENSION_B_LOWER", tile, prefix)),
        CascadeOrder::Middle => lines.push(format!("{}.{}.RAM_EXTENSION_B_BOTH", tile, prefix)),
        CascadeOrder::Last => lines.push(format!("{}.{}.RAM_EXTENSION_B_UPPER", tile, prefix)),
    }
    for (&row, words) in &pb.init_rows {
        lines.push(format!(
            "{}.{}.INIT_{row:02X}[255:0] = 256'h{:016X}{:016X}{:016X}{:016X}",
            tile, prefix, words[3], words[2], words[1], words[0]
        ));
    }
    for (&prow, words) in &pb.initp_rows {
        lines.push(format!(
            "{}.{}.INITP_{prow:02X}[255:0] = 256'h{:016X}{:016X}{:016X}{:016X}",
            tile, prefix, words[3], words[2], words[1], words[0]
        ));
    }
}

/// `"A6LUT"` → `"ALUT"`.
fn lut_bel(bel: &str) -> Result<String, String> {
    let letter = bel
        .chars()
        .next()
        .ok_or_else(|| format!("emit: bad LUT bel `{bel}`"))?;
    if !('A'..='D').contains(&letter) || !bel.ends_with("6LUT") {
        return Err(format!("emit: bad LUT bel `{bel}`"));
    }
    Ok(format!("{letter}LUT"))
}

/// `"AFF"` → `"AFF"` (validate shape).
fn ff_bel(bel: &str) -> Result<String, String> {
    let letter = bel
        .chars()
        .next()
        .ok_or_else(|| format!("emit: bad FF bel `{bel}`"))?;
    if !('A'..='D').contains(&letter) || !bel.ends_with("FF") {
        return Err(format!("emit: bad FF bel `{bel}`"));
    }
    Ok(format!("{letter}FF"))
}

fn mux_for_ff(
    ff: &crate::place::PlacedFf,
    packed: &crate::pack::Packed7,
    placed: &Placed7,
) -> &'static str {
    let letter = ff.bel.chars().next().unwrap_or('A');
    if let Some(pff) = packed.ffs.iter().find(|f| f.name == ff.name) {
        for lut in &placed.luts {
            if lut.site == ff.site
                && lut.bel.starts_with(letter)
                && packed
                    .luts
                    .iter()
                    .find(|l| l.name == lut.name)
                    .map(|l| l.output_net == pff.d_net)
                    .unwrap_or(false)
            {
                return "AX";
            }
        }
    }
    "AX"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{IobDir, Packed7, PackedFf, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedFf, PlacedIob, PlacedLut};
    use crate::route::{RouteKind, Routed7, RoutedNet7};

    struct FakeEmit;
    impl EmitDevice for FakeEmit {
        fn tile_type(&self, _tile: &str) -> Option<String> {
            Some("CLBLL_L".into())
        }
        fn site_rel(&self, tile: &str, site: &str) -> Option<String> {
            if tile.starts_with("CLB") {
                Some(if site.ends_with("Y0") {
                    "SLICEL_X0".into()
                } else {
                    "SLICEL_X1".into()
                })
            } else if tile.starts_with("RIOB") {
                Some("IOB_Y1".into())
            } else if tile.starts_with("BRAM") {
                Some("RAMB18_X0Y0".into())
            } else {
                Some("BUFGCTRL_X0Y0".into())
            }
        }
    }

    fn routed_blinky() -> Routed7 {
        let packed = Packed7 {
            luts: vec![PackedLut {
                name: "u_inv".into(),
                init: 0x5555_5555_5555_5555,
                input_nets: [Some("q".into()), None, None, None, None, None],
                output_net: "d".into(),
            }],
            ffs: vec![PackedFf {
                name: "u_ff".into(),
                d_net: "d".into(),
                q_net: "q".into(),
                clk_net: "clk_bufg".into(),
                ce_net: None,
                sr_net: None,
            }],
            iobs: vec![PackedIob {
                name: "u_obuf".into(),
                port: "led".into(),
                dir: IobDir::Out,
                net: "q".into(),
                loc: Some("H5".into()),
                iostandard: "LVCMOS33".into(),
                drive: None,
                slew: None,
            }],
            ..Default::default()
        };
        let placed = Placed7 {
            packed,
            luts: vec![PlacedLut {
                name: "u_inv".into(),
                init: 0x5555_5555_5555_5555,
                tile: "CLBLL_L_X0Y0".into(),
                site: "SLICE_X0Y0".into(),
                bel: "A6LUT".into(),
            }],
            ffs: vec![PlacedFf {
                name: "u_ff".into(),
                tile: "CLBLL_L_X0Y0".into(),
                site: "SLICE_X0Y0".into(),
                bel: "AFF".into(),
            }],
            iobs: vec![PlacedIob {
                name: "u_obuf".into(),
                port: "led".into(),
                dir: IobDir::Out,
                tile: "RIOB33_X0Y0".into(),
                site: "IOB_X0Y0".into(),
            }],
            ..Default::default()
        };
        Routed7 {
            placed,
            nets: vec![RoutedNet7 {
                net: "q".into(),
                kind: RouteKind::SiteInternal,
            }],
        }
    }

    #[test]
    fn emit_blinky_fasm() {
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.ALUT.INIT[63:0] = 64'h5555555555555555"),
            "got:\n{text}"
        );
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZINI"),
            "got:\n{text}"
        );
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFFMUX.AX"),
            "got:\n{text}"
        );
        assert!(
            text.contains("RIOB33_X0Y0.IOB_Y1.LVCMOS33.DRIVE.I16"),
            "got:\n{text}"
        );
    }

    #[test]
    fn lut_bel_names() {
        assert_eq!(lut_bel("A6LUT").unwrap(), "ALUT");
        assert_eq!(lut_bel("D6LUT").unwrap(), "DLUT");
        assert!(lut_bel("X6LUT").is_err());
    }

    #[test]
    fn emit_bram_fasm() {
        use crate::pack::PackedBram;
        use crate::place::PlacedBram;
        let packed = Packed7 {
            brams: vec![PackedBram {
                name: "u_bram".into(),
                read_width_a: 18,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
                ..Default::default()
            }],
            ..Default::default()
        };
        let placed = Placed7 {
            packed,
            brams: vec![PlacedBram {
                name: "u_bram".into(),
                tile: "BRAM_L_X0Y0".into(),
                site: "RAMB18_X0Y0".into(),
                read_width_a: 18,
                read_width_b: 18,
                write_width_a: 18,
                write_width_b: 18,
            }],
            ..Default::default()
        };
        let routed = Routed7 {
            placed,
            nets: vec![],
        };
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(
            text.contains("BRAM_L_X0Y0.RAMB18.RAMB18_X0Y0.IN_USE"),
            "got:\n{text}"
        );
    }
}
