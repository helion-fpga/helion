//! Emit stage: [`Routed7`] → FASM text ([`Bitstream7`]).
//!
//! Walks the placed+routed design and emits one FASM line per configured
//! feature, grouped by tile. The FASM text is the human-readable,
//! diffable interchange format; [`crate::frames`] assembles it into
//! configuration frames and [`crate::bitstream`] writes the `.bit` file.

use crate::pack::IobDir;
use crate::place::Placed7;
use crate::route::{RouteKind, Routed7};
use std::collections::HashSet;

/// What the emitter needs from the device model.
pub trait EmitDevice {
    /// Tile type for a tile name, e.g. `"CLBLL_L_X14Y100"` → `"CLBLL_L"`.
    /// (FASM tile prefixes use the type for segbits lookup, but the line
    /// itself uses the full tile name.)
    fn tile_type(&self, tile: &str) -> Option<String>;
    /// Site name relative to its tile for FASM, e.g.
    /// `("RIOB33_X43Y75", "IOB_X1Y76")` → `"IOB_Y0"` for a master site
    /// (`IOB_Y1` for slave),
    /// `("CLBLL_L_X14Y100", "SLICE_X28Y100")` → `"SLICEL_X0"`.
    /// Returns the site-type-qualified relative name.
    fn site_rel(&self, tile: &str, site: &str) -> Option<String>;
    /// Static clock-spine lines for a BUFGCTRL site: `(tile, feature)`
    /// rebuffer enables, emitted as bare `TILE.FEATURE` FASM lines.
    /// Defaults to none; the XDevice impl derives them from the DB.
    fn bufg_spine_static(&self, _tile: &str, _site: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    /// IOI tile paired with an IOB tile (same bank side, same row),
    /// for OLOGIC/ILOGIC features. Defaults to none; the XDevice impl
    /// finds it in the DB.
    fn ioi_tile(&self, _iob_tile: &str) -> Option<String> {
        None
    }
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

/// FASM drive value for an (IOSTANDARD, DRIVE) pair. Usually the drive
/// string itself, but the LVCMOS33 12mA selection lives in the shared
/// LVCMOS33_LVTTL family as I12_I16 (Vivado ground truth, ZB-10).
fn obuf_drive_value(iostandard: &str, drive: &str) -> String {
    let std = iostandard.to_ascii_uppercase();
    let drv = drive.to_ascii_uppercase();
    match (std.as_str(), drv.as_str()) {
        ("LVCMOS33", "I12") => "I12_I16".to_string(),
        _ => drv,
    }
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

    // FFs: ZINI + D-mux select + control signals (CE, SR).
    //
    // Control signal configuration (per X-Ray DB segbits_clbll_l.db):
    // - {bel}.ZRST: SR pin acts as RESET (vs SET). Emitted when the FF has
    //   an SR net. prjxray's Z prefix names the zero-state option (ZRST =
    //   reset-to-zero, like ZINI = init-to-zero); the FASM feature sets the
    //   documented config bit (bit = 1), it does not clear it.
    // - {slice}.CEUSEDMUX: slice-level, enables the CE input mux. Needed
    //   when any FF in the slice has a CE net.
    // - {slice}.SRUSEDMUX: slice-level, enables the SR input mux. Needed
    //   when any FF in the slice has an SR net.
    // Without CEUSEDMUX/SRUSEDMUX, the FF silently ignores CE/SR pins.
    use std::collections::HashSet;
    let mut ce_slices: HashSet<(String, String)> = HashSet::new();
    let mut sr_slices: HashSet<(String, String)> = HashSet::new();
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
        // Control signals: look up the packed FF for CE/SR nets.
        if let Some(p) = packed.ffs.iter().find(|p| p.name == ff.name) {
            if p.ce_net.is_some() {
                ce_slices.insert((ff.tile.clone(), site_rel.clone()));
            }
            if p.sr_net.is_some() {
                sr_slices.insert((ff.tile.clone(), site_rel.clone()));
                // Configure SR as RESET (the common case; Helion IR does not
                // distinguish set vs reset, and reset is the standard use).
                lines.push(format!("{}.{}.{}.ZRST", ff.tile, site_rel, bel));
            }
        }
    }
    // Emit slice-level CE/SR mux enables (deduplicated).
    for (tile, slice) in &ce_slices {
        lines.push(format!("{}.{}.CEUSEDMUX", tile, slice));
    }
    for (tile, slice) in &sr_slices {
        lines.push(format!("{}.{}.SRUSEDMUX", tile, slice));
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
                // Vivado default input slew is FAST (ground truth:
                // LIOB33_X0Y25 IOB_Y0 SLEW.FAST on the GCLK input).
                // Only LVCMOS33 has DB evidence; other standards keep
                // the old behavior (no slew line).
                if p.iostandard.eq_ignore_ascii_case("LVCMOS33") {
                    let slew = p.slew.as_deref().unwrap_or("FAST");
                    lines.push(format!(
                        "{}.{}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVTTL.SLEW.{}",
                        iob.tile,
                        site_rel,
                        slew.to_ascii_uppercase()
                    ));
                }
                // Input path: no inversion on D (Vivado ILOGIC_Y0.ZINV_D).
                if let Some(logic_tile) = device.ioi_tile(&iob.tile) {
                    let logic = site_rel.replace("IOB_", "ILOGIC_");
                    lines.push(format!("{logic_tile}.{logic}.ZINV_D"));
                }
            }
            IobDir::Out => {
                // OBUF: output driver enabled by DRIVE select (no IN_ONLY).
                // The DRIVE feature prefix depends on the (standard, drive)
                // pair (see obuf_drive_prefix); the FASM value additionally
                // depends on the pair (see obuf_drive_value). Vivado
                // default drive for LVCMOS33 is 12mA (ground truth:
                // LVCMOS33_LVTTL.DRIVE.I12_I16 on the LD0 output).
                let drive = p.drive.as_deref().unwrap_or("I12");
                let prefix = obuf_drive_prefix(&p.iostandard, drive)?;
                let value = obuf_drive_value(&p.iostandard, drive);
                lines.push(format!(
                    "{}.{}.{}.DRIVE.{}",
                    iob.tile, site_rel, prefix, value
                ));
                let slew = p.slew.as_deref().unwrap_or("SLOW");
                lines.push(format!(
                    "{}.{}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVTTL_SSTL135_SSTL15.SLEW.{}",
                    iob.tile,
                    site_rel,
                    slew.to_ascii_uppercase()
                ));
                // Output path: OLOGIC used, D1 mux, single-data-rate
                // bypass (Vivado ground truth on the LD0 output).
                if let Some(logic_tile) = device.ioi_tile(&iob.tile) {
                    let logic = site_rel.replace("IOB_", "OLOGIC_");
                    lines.push(format!("{logic_tile}.{logic}.OQUSED"));
                    lines.push(format!("{logic_tile}.{logic}.OMUX.D1"));
                    lines.push(format!(
                        "{logic_tile}.{logic}.OSERDES.DATA_RATE_TQ.BUF"
                    ));
                }
            }
        }
        // Vivado defaults on every IOB: no pull, no input termination.
        lines.push(format!("{}.{}.PULLTYPE.NONE", iob.tile, site_rel));
        lines.push(format!("{}.{}.IN_TERM.NONE", iob.tile, site_rel));
        let _ = p;
    }

    // BUFGCTRLs. Static configuration for an I0 -> O clock buffer (BUFG).
    //
    // Xilinx UG472 Fig. 2-5 "BUFG as BUFGCTRL" builds a BUFG from a
    // BUFGCTRL by DRIVING the control pins: S0=VDD, CE0=VDD, IGNORE0=GND,
    // S1=GND, CE1=GND, IGNORE1=VDD. UG472 Table 2-3: (CE0,S0,CE1,S1) =
    // (1,1,0,X) or (1,1,X,0) selects I0; any other select state holds O at
    // INIT_OUT with no toggle.
    //
    // Bit meanings below are from prjxray fuzzer 042-clk-bufg-config
    // (generate.py): a `Z` prefix means the FASM feature is the INVERSE of
    // the Vivado attribute (feature present <=> attribute is 0/FALSE); a
    // non-Z feature is present <=> attribute is 1/TRUE.
    //   ZINV_S0/ZINV_CE0 present <=> pin NOT inverted.
    //   IS_IGNORE1_INVERTED present <=> IGNORE1 inverted (Vivado sets it).
    //   INIT_OUT, IS_IGNORE0_INVERTED absent <=> defaults (0, not inverted).
    //
    // Feature set is Vivado ground truth from the ZB-10 same-RTL diff
    // (GT-blinky.bit): IN_USE, ZINV_CE0, ZINV_S0, IS_IGNORE1_INVERTED.
    // ZINV_S1/ZINV_CE1 belong to the I1 path and Vivado does not set
    // them here; neither does it set ZPRESELECT_I0. The old advice to
    // keep ZINV_S1/ZINV_CE1 is overruled: pin driving is the router's
    // job (route_bufg_tieoffs drives S0=1, CE0=1, IGNORE0=0, S1=0,
    // CE1=0, IGNORE1=1), polarity bits only select inversion.
    for bufg in &placed.bufgs {
        let site_rel = device
            .site_rel(&bufg.tile, &bufg.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", bufg.tile, bufg.site))?;
        lines.push(format!("{}.BUFGCTRL.{}.IN_USE", bufg.tile, site_rel));
        // No inversion on the driven I0-path pins: values pass through.
        lines.push(format!("{}.BUFGCTRL.{}.ZINV_CE0", bufg.tile, site_rel));
        lines.push(format!("{}.BUFGCTRL.{}.ZINV_S0", bufg.tile, site_rel));
        // IGNORE1 inverted, exactly as Vivado emits it.
        lines.push(format!("{}.BUFGCTRL.{}.IS_IGNORE1_INVERTED", bufg.tile, site_rel));
        // Rebuffer enables for this BUFG's global line (bare features).
        for (tile, feat) in device.bufg_spine_static(&bufg.tile, &bufg.site) {
            lines.push(format!("{tile}.{feat}"));
        }
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

    // DSP48E1s: placed on sites but configuration FASM not yet emitted.
    // Full DSP configuration (USE_MULT, OPMODE, ALUMODE, INMODE, pipeline
    // registers) requires the detailed attribute bit mapping; future work.
    let _ = &placed.dsps;

    // CARRY4s (ZB-04). Feature paths: TILE.SLICEL_<X>.CARRY4.<bit> and
    // TILE.SLICEL_<X>.PRECYINIT.<src>. Derived from the blinky golden
    // chain (docs/zedboard-handoff/findings/blinky_chain_golden.txt):
    // every chained group takes PRECYINIT.CIN (carry in from the slice
    // below through the dedicated cascade); the chain head takes
    // PRECYINIT.AX (CYINIT from the AX pin, fed by the const-1 LUT) or
    // PRECYINIT.C0 (CYINIT tied 0). The *CY0 line appears only on the
    // head bit A: it steers bit A onto the CYINIT mux instead of the CIN
    // cascade; every other bit cascades by default with no FASM line.
    let co_nets: HashSet<&str> = packed
        .carries
        .iter()
        .filter_map(|c| c.co_net.as_deref())
        .collect();
    for carry in &placed.carries {
        let site_rel = device
            .site_rel(&carry.tile, &carry.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", carry.tile, carry.site))?;
        let packed_c = packed
            .carries
            .iter()
            .find(|c| c.name == carry.name)
            .ok_or_else(|| format!("emit: placed CARRY4 {} has no packed cell", carry.name))?;
        if co_nets.contains(packed_c.ci_net.as_str()) {
            lines.push(format!("{}.{}.PRECYINIT.CIN", carry.tile, site_rel));
        } else {
            // Chain head: CYINIT source is the CI net driver.
            let driver_init = packed
                .luts
                .iter()
                .find(|l| l.output_net == packed_c.ci_net)
                .map(|l| l.init);
            match driver_init {
                Some(u64::MAX) => {
                    lines.push(format!("{}.{}.PRECYINIT.AX", carry.tile, site_rel));
                }
                Some(0) => {
                    lines.push(format!("{}.{}.PRECYINIT.C0", carry.tile, site_rel));
                }
                other => {
                    return Err(format!(
                        "emit: CARRY4 {} head CI net `{}` needs a const-1 (AX) or const-0 (C0) LUT driver, got {other:?}",
                        carry.name, packed_c.ci_net
                    ));
                }
            }
            // Head bit A steers onto the CYINIT mux; only when bit 0 is used.
            if packed_c.o_nets[0].is_some() || packed_c.s_nets[0].is_some() {
                lines.push(format!("{}.{}.CARRY4.ACY0", carry.tile, site_rel));
            }
        }
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

/// Which FFMUX input feeds this FF's D. When the FF is paired with a LUT
/// on the same BEL letter (pack's pairing invariant), D comes from the
/// LUT's O via the letter-matched mux input (`AX` for AFF, `BX` for BFF,
/// `CX` for CFF, `DX` for DFF). The X-Ray DB only documents the
/// letter-matched option per BEL; the bare `AX` default is invalid for
/// B/C/D (no `BFFMUX.AX` in segbits).
fn mux_for_ff(
    ff: &crate::place::PlacedFf,
    packed: &crate::pack::Packed7,
    placed: &Placed7,
) -> &'static str {
    let letter = ff.bel.chars().next().unwrap_or('A');
    let mux_in: &'static str = match letter {
        'B' => "BX",
        'C' => "CX",
        'D' => "DX",
        _ => "AX",
    };
    // D from carry O: the FFMUX selects XOR (the carry sum bit), never a
    // letter input. The golden chain shows FFMUX.XOR on every chain FF.
    // Without this the FF would sample AX instead of the sum: wrong
    // silicon with no loud failure, so this check comes first.
    if let Some(pff) = packed.ffs.iter().find(|f| f.name == ff.name) {
        let from_carry = packed.carries.iter().any(|c| {
            c.o_nets
                .iter()
                .any(|o| o.as_deref() == Some(pff.d_net.as_str()))
        });
        if from_carry {
            return "XOR";
        }
        // Otherwise the D net driver, when it is a LUT on the same
        // site+letter, selects the letter-matched mux input.
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
                return mux_in;
            }
        }
    }
    // Default: letter-matched input (most common); the router/emitter can refine.
    mux_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{IobDir, Packed7, PackedCarry, PackedFf, PackedIob, PackedLut};
    use crate::place::{Placed7, PlacedCarry, PlacedFf, PlacedIob, PlacedLut};
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
            carry_anchor: None,
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
        // Vivado default drive for LVCMOS33 outputs is 12mA (ZB-10).
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.LVCMOS33_LVTTL.DRIVE.I12_I16"), "got:\n{text}");
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.PULLTYPE.NONE"), "got:\n{text}");
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.IN_TERM.NONE"), "got:\n{text}");
    }

    #[test]
    fn emit_ff_control_signals() {
        // FF with CE and SR nets must emit CEUSEDMUX, SRUSEDMUX, and ZRST.
        // Without these, the FF silently ignores its CE/SR pins.
        let mut routed = routed_blinky();
        routed.placed.packed.ffs[0].ce_net = Some("ce".into());
        routed.placed.packed.ffs[0].sr_net = Some("rst".into());
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.CEUSEDMUX"),
            "missing CEUSEDMUX, got:\n{text}"
        );
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.SRUSEDMUX"),
            "missing SRUSEDMUX, got:\n{text}"
        );
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZRST"),
            "missing ZRST, got:\n{text}"
        );
    }

    /// Golden shapes: TILE.SLICEL_X*.PRECYINIT.{AX,CIN} plus a lone
    /// CARRY4.ACY0 on the head bit A (blinky_chain_golden.txt: ACY0 once,
    /// no BCY0/CCY0/DCY0 anywhere in the chain).
    fn routed_carry_chain() -> Routed7 {
        let mut r = routed_blinky();
        r.placed.packed.luts.push(PackedLut {
            name: "u_one".into(), init: u64::MAX,
            input_nets: [None, None, None, None, None, None],
            output_net: "one".into(),
        });
        r.placed.packed.carries = vec![
            PackedCarry {
                name: "u_c0".into(), ci_net: "one".into(), co_net: Some("co0".into()),
                s_nets: [Some("s0".into()), None, None, None],
                di_nets: [None, None, None, None],
                o_nets: [Some("o0".into()), None, None, None],
            },
            PackedCarry {
                name: "u_c1".into(), ci_net: "co0".into(), co_net: None,
                s_nets: [Some("s1".into()), None, None, None],
                di_nets: [None, None, None, None],
                o_nets: [Some("o1".into()), None, None, None],
            },
        ];
        r.placed.carries = vec![
            PlacedCarry { name: "u_c0".into(), tile: "CLB_X0Y0".into(), site: "SLICE_X0Y0".into() },
            PlacedCarry { name: "u_c1".into(), tile: "CLB_X0Y1".into(), site: "SLICE_X0Y1".into() },
        ];
        // One chain FF per group to pin the FFMUX.XOR rule.
        r.placed.packed.ffs.push(PackedFf {
            name: "u_ff0".into(), d_net: "o0".into(), q_net: "q0".into(),
            clk_net: "clk_bufg".into(), ce_net: None, sr_net: None,
        });
        r.placed.ffs.push(PlacedFf {
            name: "u_ff0".into(), tile: "CLB_X0Y0".into(), site: "SLICE_X0Y0".into(), bel: "AFF".into(),
        });
        r
    }

    #[test]
    fn emit_carry_chain_precyinit_matches_golden() {
        let bs = emit_fasm(&routed_carry_chain(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLB_X0Y0.SLICEL_X0.PRECYINIT.AX"), "got:\n{text}");
        assert!(text.contains("CLB_X0Y0.SLICEL_X0.CARRY4.ACY0"), "got:\n{text}");
        assert!(text.contains("CLB_X0Y1.SLICEL_X1.PRECYINIT.CIN"), "got:\n{text}");
        assert_eq!(text.matches("CARRY4.ACY0").count(), 1, "got:\n{text}");
        assert!(!text.contains("BCY0") && !text.contains("CCY0") && !text.contains("DCY0"), "got:\n{text}");
        // Chain FF samples the sum through FFMUX.XOR, never a letter input.
        assert!(text.contains("CLB_X0Y0.SLICEL_X0.AFFMUX.XOR"), "got:\n{text}");
    }

    #[test]
    fn emit_carry_head_without_const_driver_fails_loudly() {
        let mut r = routed_carry_chain();
        // Non-const driver on the head CI: no valid CYINIT source.
        r.placed.packed.luts.iter_mut().find(|l| l.name == "u_one").unwrap().init = 1;
        let err = emit_fasm(&r, &FakeEmit).unwrap_err();
        assert!(err.contains("one") && err.contains("const-1"), "got: {err}");
    }

    #[test]
    fn emit_bufg_i0_config_matches_vivado() {
        // Vivado ground truth from the ZB-10 same-RTL diff
        // (GT-blinky.bit): a BUFG on the I0 path emits exactly IN_USE,
        // ZINV_CE0, ZINV_S0, IS_IGNORE1_INVERTED. ZINV_CE1/ZINV_S1
        // select the I1 path and ZPRESELECT_I0 is absent in Vivado.
        use crate::pack::PackedBufg;
        use crate::place::PlacedBufg;
        let mut routed = routed_blinky();
        routed.placed.packed.bufgs.push(PackedBufg {
            name: "bufg_clk".into(),
            in_net: "clk".into(),
            out_net: "clk_bufg".into(),
        });
        routed.placed.bufgs.push(PlacedBufg {
            name: "bufg_clk".into(),
            tile: "CLK_BUFG_BOT_R_X0Y0".into(),
            site: "BUFGCTRL_X0Y0".into(),
        });
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let got: Vec<&str> = bs
            .fasm
            .iter()
            .map(String::as_str)
            .filter(|l| l.contains("BUFGCTRL_X0Y0"))
            .collect();
        // NOTE: emit_fasm sorts lines; IS_IGNORE1_INVERTED < ZINV_*.
        let want = [
            "CLK_BUFG_BOT_R_X0Y0.BUFGCTRL.BUFGCTRL_X0Y0.IN_USE",
            "CLK_BUFG_BOT_R_X0Y0.BUFGCTRL.BUFGCTRL_X0Y0.IS_IGNORE1_INVERTED",
            "CLK_BUFG_BOT_R_X0Y0.BUFGCTRL.BUFGCTRL_X0Y0.ZINV_CE0",
            "CLK_BUFG_BOT_R_X0Y0.BUFGCTRL.BUFGCTRL_X0Y0.ZINV_S0",
        ];
        assert_eq!(got, want, "BUFGCTRL I0 feature set");
    }

    #[test]
    fn emit_iob_vivado_defaults() {
        // Vivado ground truth (ZB-10 same-RTL diff, GT-blinky.bit):
        // LVCMOS33 outputs use DRIVE I12 (I12_I16 feature) with SLOW
        // slew; inputs use FAST slew; both use PULLTYPE/IN_TERM NONE.
        // (FakeEmit has no IOI tile, so no OLOGIC/ILOGIC lines here;
        // the e2e diff verifies those.)
        let mut routed = routed_blinky();
        routed.placed.packed.iobs.push(PackedIob {
            name: "u_ibuf".into(), port: "clk".into(), dir: IobDir::In,
            net: "clk".into(), loc: Some("H6".into()), iostandard: "LVCMOS33".into(),
            drive: None, slew: None,
        });
        routed.placed.iobs.push(PlacedIob {
            name: "u_ibuf".into(), port: "clk".into(), dir: IobDir::In,
            tile: "LIOB33_X0Y0".into(), site: "IOB_X0Y0".into(),
        });
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let iob: Vec<&str> = bs
            .fasm
            .iter()
            .map(String::as_str)
            .filter(|l| l.contains("IOB_Y1.PULLTYPE") || l.contains("IN_TERM") || l.contains("SLEW.FAST") || l.contains("DRIVE.I12"))
            .collect();
        assert!(
            iob.iter().any(|l| l.ends_with("IOB_Y1.LVCMOS33_LVTTL.DRIVE.I12_I16")),
            "missing I12 drive, got:\n{}",
            bs.fasm.join("\n")
        );
        assert!(
            iob.iter().any(|l| l.ends_with("IOB_Y1.PULLTYPE.NONE")),
            "missing PULLTYPE.NONE, got:\n{}",
            bs.fasm.join("\n")
        );
        assert!(
            iob.iter().any(|l| l.ends_with("IOB_Y1.IN_TERM.NONE")),
            "missing IN_TERM.NONE, got:\n{}",
            bs.fasm.join("\n")
        );
        assert!(
            iob.iter().any(|l| l.contains("SLEW.FAST")),
            "missing input FAST slew, got:\n{}",
            bs.fasm.join("\n")
        );
    }

    #[test]
    fn emit_ff_no_control_signals() {
        // FF without CE/SR must NOT emit the mux enables (keeps bitstream minimal).
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(
            !text.contains("CEUSEDMUX"),
            "unexpected CEUSEDMUX, got:\n{text}"
        );
        assert!(
            !text.contains("SRUSEDMUX"),
            "unexpected SRUSEDMUX, got:\n{text}"
        );
        assert!(!text.contains(".ZRST"), "unexpected ZRST, got:\n{text}");
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
            carry_anchor: None,
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
