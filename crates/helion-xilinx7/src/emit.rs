//! Emit stage: [`Routed7`] → FASM text ([`Bitstream7`]).
//!
//! Walks the placed+routed design and emits one FASM line per configured
//! feature, grouped by tile. The FASM text is the human-readable,
//! diffable interchange format; [`crate::frames`] assembles it into
//! configuration frames and [`crate::bitstream`] writes the `.bit` file.

use crate::pack::IobDir;
use crate::place::Placed7;
use crate::route::{RouteKind, Routed7};

/// What the emitter needs from the device model.
pub trait EmitDevice {
    /// Tile type for a tile name, e.g. `"CLBLL_L_X14Y100"` → `"CLBLL_L"`.
    /// (FASM tile prefixes use the type for segbits lookup, but the line
    /// itself uses the full tile name.)
    fn tile_type(&self, tile: &str) -> Option<String>;
    /// Site name relative to its tile for FASM, e.g.
    /// `("RIOB33_X43Y75", "IOB_X1Y76")` → `"IOB_Y1"`,
    /// `("CLBLL_L_X14Y100", "SLICE_X28Y100")` → `"SLICEL_X0"`.
    /// Returns the site-type-qualified relative name.
    fn site_rel(&self, tile: &str, site: &str) -> Option<String>;
}

/// IBUF `.IN` feature prefix for an IOSTANDARD, from
/// `segbits_liob33.db`. The prefix encodes which standards share the same
/// configuration bits; an unknown standard is a loud error, not a default.
fn ibuf_in_prefix(iostandard: &str) -> Result<&'static str, String> {
    match iostandard.to_ascii_uppercase().as_str() {
        "LVCMOS12" | "LVCMOS15" | "LVCMOS18" => Ok("LVCMOS12_LVCMOS15_LVCMOS18"),
        "LVCMOS25" | "LVCMOS33" | "LVTTL" => Ok("LVCMOS25_LVCMOS33_LVTTL"),
        "LVDS_25" | "SSTL135" | "SSTL15" => Ok("LVDS_25_SSTL135_SSTL15"),
        "TMDS_33" => Ok("TMDS_33"),
        other => Err(format!("emit: unsupported input IOSTANDARD {other}")),
    }
}

/// OBUF `.DRIVE` feature prefix for an (IOSTANDARD, DRIVE) pair, from
/// `segbits_liob33.db`. Xilinx groups standards that share drive-strength
/// bits; the prefix is NOT always the bare standard name. Unknown
/// combinations are a loud error, not a guessed feature.
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
    /// FASM feature lines, one per line
    /// (e.g. `CLBLL_L_X14Y100.SLICEL_X0.ALUT.INIT[63:0] = 64'h...`).
    pub fasm: Vec<String>,
    /// Assembled configuration frames (101 words each); filled by
    /// [`crate::frames`] from [`Bitstream7::fasm`].
    pub frames: Vec<u32>,
}

/// Emit FASM for a routed design.
///
/// Every line is `TILE.FEATURE` or `TILE.FEATURE = value`. Tile order is
/// sorted for deterministic output.
pub fn emit_fasm(routed: &Routed7, device: &dyn EmitDevice) -> Result<Bitstream7, String> {
    let placed = &routed.placed;
    let packed = &placed.packed;
    let mut lines: Vec<String> = Vec::new();

    // LUTs: INIT vector per occupied ALUT..DLUT.
    for lut in &placed.luts {
        let site_rel = device
            .site_rel(&lut.tile, &lut.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", lut.tile, lut.site))?;
        let bel = lut_bel(&lut.bel)?; // "A6LUT" -> "ALUT"
        lines.push(format!(
            "{}.{}.{}.INIT[63:0] = 64'h{:016X}",
            lut.tile, site_rel, bel, lut.init
        ));
    }

    // FFs: ZINI + D-mux select.
    for ff in &placed.ffs {
        let site_rel = device
            .site_rel(&ff.tile, &ff.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", ff.tile, ff.site))?;
        let bel = ff_bel(&ff.bel)?; // "AFF" -> "AFF"
        // D from LUT O: the AFFMUX selects AX when the paired LUT drives D.
        // (Pack pairs FF.D == LUT.O on the same BEL letter.)
        let mux_src = mux_for_ff(ff, packed, placed);
        lines.push(format!("{}.{}.{}.ZINI", ff.tile, site_rel, bel));
        lines.push(format!("{}.{}.{}MUX.{}", ff.tile, site_rel, bel, mux_src));
    }

    // IOBs.
    for iob in &placed.iobs {
        let site_rel = device
            .site_rel(&iob.tile, &iob.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", iob.tile, iob.site))?;
        let p = packed.iobs.iter().find(|p| p.name == iob.name).ok_or_else(|| {
            format!("emit: packed IOB {} not found", iob.name)
        })?;
        match iob.dir {
            IobDir::In => {
                // IBUF: input buffer. The IN feature lives under an
                // iostandard-specific prefix (see ibuf_in_prefix), e.g.
                // "RIOB33_X0Y0.IOB_Y1.LVCMOS25_LVCMOS33_LVTTL.IN".
                // IN_ONLY disables the output driver.
                let prefix = ibuf_in_prefix(&p.iostandard)?;
                lines.push(format!("{}.{}.{}.IN", iob.tile, site_rel, prefix));
                lines.push(format!(
                    "{}.{}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVDS_25_LVTTL_SSTL135_SSTL15_TMDS_33.IN_ONLY",
                    iob.tile, site_rel
                ));
            }
            IobDir::Out => {
                // OBUF: output driver enabled by DRIVE select (no IN_ONLY).
                // The DRIVE feature prefix depends on the (standard, drive)
                // pair (see obuf_drive_prefix).
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
        let _ = p;
    }

    // BUFGCTRLs. Feature path: TILE.BUFGCTRL.<site>.IN_USE.
    for bufg in &placed.bufgs {
        let site_rel = device
            .site_rel(&bufg.tile, &bufg.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", bufg.tile, bufg.site))?;
        lines.push(format!("{}.BUFGCTRL.{}.IN_USE", bufg.tile, site_rel));
        lines.push(format!("{}.BUFGCTRL.{}.IS_IGNORE0_INVERTED", bufg.tile, site_rel));
    }

    // RAMB18E1s. Feature path: TILE.RAMB18.<site>.IN_USE. INIT data
    // programming from the IR's Bram18 words is not yet emitted; the
    // block_ram.db bit mapping is future work.
    for bram in &placed.brams {
        let site_rel = device
            .site_rel(&bram.tile, &bram.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", bram.tile, bram.site))?;
        lines.push(format!("{}.RAMB18.{}.IN_USE", bram.tile, site_rel));
    }

    // DSP48E1s: emit FASM configuration based on Project X-Ray findings.
    // Site naming: DSP_0 (even Y) or DSP_1 (odd Y).
    // OPMODE/ALUMODE via IS_*_INVERTED parameters; pipeline regs via *REG tags.
    for dsp in &placed.dsps {
        // Determine DSP_0 vs DSP_1 from site Y coordinate.
        // Site format: DSP48E1_X<num>Y<num>
        let dsp_site = if let Some(y_pos) = dsp.site.rfind('Y') {
            let y_str = &dsp.site[y_pos + 1..];
            let y: i32 = y_str.parse().unwrap_or(0);
            if y % 2 == 0 { "DSP_0" } else { "DSP_1" }
        } else {
            "DSP_0"
        };

        // Pipeline registers: disable (use direct inputs, not registered)
        // From X-Ray fuzzer: ADREG, ALUMODEREG, etc. with invert=1 means
        // the Z-prefixed tag sets the bit to 0 (disabled).
        lines.push(format!("{}.{}.ZADREG", dsp.tile, dsp_site));
        lines.push(format!("{}.{}.ZALUMODEREG", dsp.tile, dsp_site));
        lines.push(format!("{}.{}.ZOPMODEREG", dsp.tile, dsp_site));
        lines.push(format!("{}.{}.ZCARRYINREG", dsp.tile, dsp_site));
        lines.push(format!("{}.{}.ZCARRYINSELREG", dsp.tile, dsp_site));
        lines.push(format!("{}.{}.ZINMODEREG", dsp.tile, dsp_site));

        // OPMODE[6:0]: emit IS_OPMODE_INVERTED bits for 0-valued opmode bits.
        // In X-Ray, the Z prefix with value 1 means the inversion bit is set,
        // which corresponds to opmode bit = 0 (inverted from default 1).
        // For simplicity, we emit the inversion bits directly:
        // - opmode bit = 1 → IS_OPMODE_INVERTED = 0 → omit (default)
        // - opmode bit = 0 → IS_OPMODE_INVERTED = 1 → emit ZIS_OPMODE_INVERTED
        for i in 0..7 {
            if ((dsp.opmode >> i) & 1) == 0 {
                lines.push(format!(
                    "{}.{}.ZIS_OPMODE_INVERTED[{}]",
                    dsp.tile, dsp_site, i
                ));
            }
        }
        // ALUMODE[3:0]: same logic
        for i in 0..4 {
            if ((dsp.alumode >> i) & 1) == 0 {
                lines.push(format!(
                    "{}.{}.ZIS_ALUMODE_INVERTED[{}]",
                    dsp.tile, dsp_site, i
                ));
            }
        }

        // Note: Full OPMODE/ALUMODE value tying to VCC/GND requires
        // routing configuration, which is future work. The above sets
        // the inversion bits; the actual OPMODE ports must be tied
        // via the routing fabric (not yet implemented).
    }

    // CARRY4s. Feature path: TILE.SLICEL_<X>.CARRY4.IN_USE. The carry chain
    // muxing (CYINIT, CI) is not yet emitted; minimal placement only.
    for carry in &placed.carries {
        let site_rel = device
            .site_rel(&carry.tile, &carry.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", carry.tile, carry.site))?;
        lines.push(format!("{}.{}.CARRY4.IN_USE", carry.tile, site_rel));
    }

    // Routing pips: TILE.DST.SRC. Pseudo-pips are always-on hard
    // connections (e.g. OLOGIC bypass): traversed for connectivity but
    // never emitted (no configuration bits).
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

/// Which AFFMUX input feeds this FF's D. When the FF is paired with a LUT
/// on the same BEL letter (pack's pairing invariant), D comes from the
/// LUT's O via the `AX` mux input.
fn mux_for_ff(
    ff: &crate::place::PlacedFf,
    packed: &crate::pack::Packed7,
    placed: &Placed7,
) -> &'static str {
    let letter = ff.bel.chars().next().unwrap_or('A');
    // Find the FF's D net driver: if it's a LUT on the same site+letter, use AX.
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
    // Default: AX (most common); the router/emitter can refine.
    "AX"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{IobDir, Packed7, PackedFf, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedFf, PlacedIob, PlacedLut};
    use crate::route::{Routed7, RoutedNet7, RouteKind};

    struct FakeEmit;
    impl EmitDevice for FakeEmit {
        fn tile_type(&self, _tile: &str) -> Option<String> {
            Some("CLBLL_L".into())
        }
        fn site_rel(&self, tile: &str, site: &str) -> Option<String> {
            if tile.starts_with("CLB") {
                Some(if site.ends_with("Y0") { "SLICEL_X0".into() } else { "SLICEL_X1".into() })
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
                name: "u_obuf".into(), port: "led".into(), dir: IobDir::Out,
                net: "q".into(), loc: Some("H5".into()), iostandard: "LVCMOS33".into(),
                drive: None, slew: None,
            }],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
        };
        let placed = Placed7 {
            packed: packed.clone(),
            luts: vec![PlacedLut { name: "u_inv".into(), init: 0x5555_5555_5555_5555, tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into(), bel: "A6LUT".into() }],
            ffs: vec![PlacedFf { name: "u_ff".into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into(), bel: "AFF".into() }],
            iobs: vec![PlacedIob { name: "u_obuf".into(), port: "led".into(), dir: IobDir::Out, tile: "RIOB33_X0Y0".into(), site: "IOB_X0Y0".into() }],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
        };
        Routed7 {
            placed,
            nets: vec![RoutedNet7 { net: "q".into(), kind: RouteKind::SiteInternal }],
        }
    }

    #[test]
    fn emit_blinky_fasm() {
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.ALUT.INIT[63:0] = 64'h5555555555555555"), "got:\n{text}");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZINI"), "got:\n{text}");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFFMUX.AX"), "got:\n{text}");
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.LVCMOS33.DRIVE.I16"), "got:\n{text}");
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
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![PackedBram { name: "u_bram".into(), init: vec![] }],
            dsps: vec![],
            carries: vec![],
        };
        let placed = Placed7 {
            packed: packed.clone(),
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![PlacedBram { name: "u_bram".into(), tile: "BRAM_L_X0Y0".into(), site: "RAMB18_X0Y0".into() }],
            dsps: vec![],
            carries: vec![],
        };
        let routed = Routed7 { placed, nets: vec![] };
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("BRAM_L_X0Y0.RAMB18.RAMB18_X0Y0.IN_USE"), "got:\n{text}");
    }
}
