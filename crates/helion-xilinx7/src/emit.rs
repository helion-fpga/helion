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
    /// Paired IOI tile for an IOB tile (same grid row, adjacent column),
    /// e.g. `RIOB33_X73Y45` → `RIOI3_X73Y45`. Needed to configure the
    /// OLOGIC that sits between the fabric and the OBUF.
    fn ioi_tile_for(&self, iob_tile: &str) -> Option<String>;
    /// Does `tile_type` (derived from `ioi_tile`) actually carry segbits for
    /// `site.feature`? Upstream X-Ray ships `*_SING`/`*_TBYTESRC`/
    /// `*_TBYTETERM` IOI tiles with ppips only, so a configurable feature
    /// emitted on them is unassemblable. Returning `false` must make the
    /// emitter fail loudly instead of writing a FASM line that no
    /// `segbits_*.db` can resolve.
    fn ologic_feature_available(&self, ioi_tile: &str, site: &str, feature: &str) -> bool;
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
                // Connect the fabric to the OBUF through the paired OLOGIC.
                //
                // X-Ray models the path as:
                //   fabric -> IOI_IMUX34_n -> IOI_OLOGIC0_D1 -> IOI_OLOGIC0_OQ
                //          -> RIOI_O0 -> IOB_O_OUT0 -> IOB_O0 -> pad
                //
                // On a full IOB/IOI pair the D1->OQ hops are `always`
                // pseudo-pips (hard, free connections) and the ONE genuinely
                // configurable item is the OLOGIC output-mux select
                // `OLOGIC_Y0.OMUX.D1`, with `OQUSED` marking OQ driven.
                // Without OMUX.D1 the pad keeps its init value and reads dark
                // even though DRIVE/SLEW configured the buffer.
                //
                // SING / TBYTESRC / TBYTETERM IOI tiles are different: the
                // whole chain is hardwired there (`IOI_IMUX34_0 -> D1`,
                // `D1 -> OQ`, `OQ -> RIOI_O0` are all `always`) and upstream
                // X-Ray ships NO segbits for those types. Nothing needs
                // configuring, so nothing is emitted -- requiring segbits
                // would wrongly reject ZedBoard LD6/LD7, which are ordinary
                // reachable pins on a RIOI3_SING tile.
                if let Some(ioi) = device.ioi_tile_for(&iob.tile) {
                    let ologic = site_rel.replacen("IOB_", "OLOGIC_", 1);
                    for feat in ["OMUX.D1", "OQUSED"] {
                        // Emit only where the DB defines it; elsewhere the
                        // connection is already hardwired in silicon.
                        if device.ologic_feature_available(&ioi, &ologic, feat) {
                            lines.push(format!("{}.{}.{}", ioi, ologic, feat));
                        }
                    }
                } else {
                    return Err(format!(
                        "emit: output IOB {} ({}) has no paired IOI tile; cannot connect the \
                         fabric to the OBUF",
                        iob.name, iob.tile
                    ));
                }
            }
        }
        let _ = p;
    }

    // BUFGCTRLs, wired as a BUFG (I0 -> O).
    //
    // Feature set copied from a Vivado 2023.2 bitstream for this board
    // (blinky, clk on Y9, BUFGCTRL_X0Y0), decoded with the X-Ray zynq7
    // segbits:
    //   BUFGCTRL_X0Y0:       IN_USE, IS_IGNORE1_INVERTED, ZINV_CE0, ZINV_S0
    //   CLK_BUFG_BUFGCTRL0_I0: CLK_BUFG_BOT_R_CK_MUXED0   (input select)
    //   CLK_BUFG_CK_GCLK0:     CLK_BUFG_BUFGCTRL0_O       (output -> GCLK)
    //
    // INIT_OUT, IS_IGNORE0_INVERTED, ZINV_CE1, ZINV_S1, ZPRESELECT_I0 and
    // PRESELECT_I1 are all ABSENT in the reference. Do not "helpfully" add
    // them: the ZINV bits invert the pin, so setting ZINV_S1/ZINV_CE1 is
    // not how a BUFG deselects I1.
    //
    // The control-pin puzzle resolves here: the reference does not route
    // S0/S1/CE0/CE1/IGNORE0/IGNORE1 to constant nets either. Their
    // effective values come from the site defaults plus these inversion
    // bits, so no tie-off routing is needed.
    //
    // Both muxes are mandatory. The site config only marks the BUFGCTRL
    // used: without the input mux no clock reaches I0, and without the
    // output mux the buffer never drives the global clock spine that feeds
    // FF CLK pins.
    for bufg in &placed.bufgs {
        let site_rel = device
            .site_rel(&bufg.tile, &bufg.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", bufg.tile, bufg.site))?;
        let tile_type = device
            .tile_type(&bufg.tile)
            .ok_or_else(|| format!("emit: no tile type for {}", bufg.tile))?;
        let (top, slot) = bufg_slot(&site_rel, &tile_type)?;
        // segbits name both tiles' sites BUFGCTRL_X0Y0..15; the top tile's
        // tilegrid sites are numbered X0Y16..31.
        let seg_site = format!("BUFGCTRL_X0Y{slot}");
        lines.push(format!("{}.BUFGCTRL.{}.IN_USE", bufg.tile, seg_site));
        lines.push(format!(
            "{}.BUFGCTRL.{}.IS_IGNORE1_INVERTED",
            bufg.tile, seg_site
        ));
        lines.push(format!("{}.BUFGCTRL.{}.ZINV_CE0", bufg.tile, seg_site));
        lines.push(format!("{}.BUFGCTRL.{}.ZINV_S0", bufg.tile, seg_site));
        // I0 input mux: BUFGCTRLn's dedicated I0 input is CK_MUXED<2n>.
        let mux_tile = if top { "CLK_BUFG_TOP_R" } else { "CLK_BUFG_BOT_R" };
        lines.push(format!(
            "{}.CLK_BUFG_BUFGCTRL{slot}_I0.{mux_tile}_CK_MUXED{}",
            bufg.tile,
            2 * slot
        ));
        // O output mux onto the global clock spine: bottom tile owns
        // GCLK0..15, the top tile GCLK16..31.
        let gclk = if top { 16 + slot } else { slot };
        lines.push(format!(
            "{}.CLK_BUFG_CK_GCLK{}.CLK_BUFG_BUFGCTRL{}_O",
            bufg.tile, gclk, slot
        ));
    }

    // RAMB18E1s.
    //
    // Only IN_USE can be emitted today: the port/clock/width configuration and
    // the INIT data live in `block_ram.db`, which this backend does not read.
    // A RAMB18 that is merely marked IN_USE holds no data and has no configured
    // ports, so the design would silently read X. Refuse instead of shipping a
    // bitstream that programs (done=1) and misbehaves.
    if !placed.brams.is_empty() {
        return Err(format!(
            "emit: design uses {} RAMB18E1 block(s) ({}), but the 7-series backend cannot \
             configure BRAM ports or INIT data yet (needs block_ram.db; see findings/\
             investigator_report.md C8). Refusing to emit a non-functional bitstream.",
            placed.brams.len(),
            placed
                .brams
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // DSP48E1s.
    //
    // Placed on sites but no configuration is emitted (USE_MULT, OPMODE,
    // ALUMODE, INMODE, pipeline regs all need the detailed attribute bit map).
    // Silently discarding them would yield a bitstream whose arithmetic reads
    // X. Fail loudly instead.
    if !placed.dsps.is_empty() {
        return Err(format!(
            "emit: design uses {} DSP48E1 block(s) ({}), but the 7-series backend cannot \
             configure DSP48E1 yet (see findings/investigator_report.md C4). Refusing to \
             emit a non-functional bitstream.",
            placed.dsps.len(),
            placed
                .dsps
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // CARRY4s.
    //
    // A CARRY4 needs more than the ACY0/BCY0/CCY0/DCY0 propagate muxes: its
    // CYINIT source must be selected by PRECYINIT.{AX,C0,C1,CIN}, and the
    // per-bit DI/S inputs come from the slice. None of that is captured today
    // -- `PackedCarry` has only a name (pack.rs), so there is no CYINIT source
    // and no DI/S data to emit. Emitting the mux bits alone would configure a
    // carry chain with an undefined init: arithmetic would silently produce
    // wrong results. Fail loudly rather than guess.
    if !placed.carries.is_empty() {
        return Err(format!(
            "emit: design uses {} CARRY4 chain element(s) ({}), but the 7-series backend does \
             not capture carry configuration: PRECYINIT (CYINIT source) and the DI/S inputs \
             are not recorded by pack.rs, so only the propagate muxes could be emitted. \
             Refusing to emit arithmetic that would silently miscompute. See findings/\
             investigator_report.md C2.",
            placed.carries.len(),
            placed
                .carries
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
                // The clock distribution network has a mux/route PIP plus
                // separate "active"/"enable" bits. A Vivado bitstream sets
                // both; without the companions the branch is configured but
                // muted (the exact symptom the original blinky showed).
                let tile_type = device.tile_type(&pip.tile).unwrap_or_default();
                for feature in clock_companions(&tile_type, &pip.dst_wire, &pip.src_wire) {
                    lines.push(format!("{}.{}", pip.tile, feature));
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

/// Extra configuration features that enable a clock-network branch.
///
/// X-Ray models the clock tree as a mux/route PIP *plus* separate
/// active/enable bits. For example, HROW input `CLK_HROW_CK_IN_L0` also
/// needs `CLK_HROW_CK_IN_L0_ACTIVE`, and the BUFG rebuffer needs
/// `GCLK<n>_ENABLE_ABOVE`/`_BELOW`. Vivado sets both halves; a mux without
/// its companion leaves the branch configured but muted.
///
/// Returns feature strings relative to `tile_type`'s tile (no tile prefix).
fn clock_companions(tile_type: &str, dst: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    if tile_type == "HCLK_CMT" {
        if src.starts_with("HCLK_CMT_CCIO") {
            out.push(format!("{src}_ACTIVE"));
            out.push(format!("{src}_USED"));
        }
    } else if tile_type == "CLK_BUFG_REBUF" {
        if let Some(g) = gclk_index(src).or_else(|| gclk_index(dst)) {
            out.push(format!("GCLK{g}_ENABLE_ABOVE"));
            out.push(format!("GCLK{g}_ENABLE_BELOW"));
        }
    } else if tile_type.starts_with("CLK_HROW") {
        if src.starts_with("CLK_HROW_CK_IN_") {
            out.push(format!("{src}_ACTIVE"));
        } else if src.starts_with("CLK_HROW_") && src.contains("_CK_GCLK") {
            out.push(format!("{src}_ACTIVE"));
        }
    } else if tile_type == "HCLK_R" {
        if src.starts_with("HCLK_CK_BUFHCLK") {
            out.push(format!("ENABLE_BUFFER.{src}"));
        }
    }
    out
}

/// Numeric index in the first `GCLK<digits>` of `wire`.
fn gclk_index(wire: &str) -> Option<u32> {
    let start = wire.find("GCLK")? + "GCLK".len();
    let digits: String = wire[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Slot index of a `BUFGCTRL` site within its `CLK_BUFG_{BOT,TOP}_R` tile.
///
/// `site` is the tilegrid name: `BUFGCTRL_X0Y0..15` in the bottom tile and
/// `BUFGCTRL_X0Y16..31` in the top tile. X-Ray's segbits name both tiles'
/// sites `BUFGCTRL_X0Y0..15`, and number the top tile's GCLKs 16..31.
/// Returns `(is_top, slot)`.
fn bufg_slot(site: &str, tile_type: &str) -> Result<(bool, u32), String> {
    let bad = || format!("emit: bad BUFGCTRL site `{site}` in {tile_type}");
    let y: u32 = site
        .rsplit_once('Y')
        .and_then(|(_, y)| y.parse().ok())
        .ok_or_else(bad)?;
    match tile_type {
        "CLK_BUFG_BOT_R" => {
            if y < 16 {
                Ok((false, y))
            } else {
                Err(bad())
            }
        }
        "CLK_BUFG_TOP_R" => {
            if (16..32).contains(&y) {
                Ok((true, y - 16))
            } else {
                Err(bad())
            }
        }
        other => Err(format!("emit: {other} is not a BUFG tile")),
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
    // Find the FF's D net driver: if it's a LUT on the same site+letter,
    // use the letter-matched mux input.
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
        fn ioi_tile_for(&self, _iob_tile: &str) -> Option<String> {
            Some("RIOI3_X0Y0".into())
        }
        fn ologic_feature_available(&self, _ioi: &str, _site: &str, _feature: &str) -> bool {
            true
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
    fn bufg_slot_maps_bottom_and_top() {
        assert_eq!(bufg_slot("BUFGCTRL_X0Y0", "CLK_BUFG_BOT_R").unwrap(), (false, 0));
        assert_eq!(bufg_slot("BUFGCTRL_X0Y15", "CLK_BUFG_BOT_R").unwrap(), (false, 15));
        assert_eq!(bufg_slot("BUFGCTRL_X0Y16", "CLK_BUFG_TOP_R").unwrap(), (true, 0));
        assert_eq!(bufg_slot("BUFGCTRL_X0Y31", "CLK_BUFG_TOP_R").unwrap(), (true, 15));
        assert!(bufg_slot("BUFGCTRL_X0Y16", "CLK_BUFG_BOT_R").is_err());
        assert!(bufg_slot("BUFGCTRL_X0Y0", "CLK_BUFG_TOP_R").is_err());
    }

    #[test]
    fn clock_companions_cover_each_stage() {
        assert_eq!(
            clock_companions("HCLK_CMT", "", "HCLK_CMT_CCIO0"),
            vec!["HCLK_CMT_CCIO0_ACTIVE", "HCLK_CMT_CCIO0_USED"]
        );
        assert_eq!(
            clock_companions(
                "CLK_BUFG_REBUF",
                "CLK_BUFG_REBUF_R_CK_GCLK0_BOT",
                "CLK_BUFG_REBUF_R_CK_GCLK0_TOP"
            ),
            vec!["GCLK0_ENABLE_ABOVE", "GCLK0_ENABLE_BELOW"]
        );
        assert_eq!(
            clock_companions(
                "CLK_HROW_BOT_R",
                "CLK_HROW_CK_MUX_OUT_R0",
                "CLK_HROW_CK_IN_L0"
            ),
            vec!["CLK_HROW_CK_IN_L0_ACTIVE"]
        );
        assert_eq!(
            clock_companions(
                "CLK_HROW_BOT_R",
                "CLK_HROW_CK_MUX_OUT_R0",
                "CLK_HROW_R_CK_GCLK0"
            ),
            vec!["CLK_HROW_R_CK_GCLK0_ACTIVE"]
        );
        assert_eq!(
            clock_companions("HCLK_R", "", "HCLK_CK_BUFHCLK0"),
            vec!["ENABLE_BUFFER.HCLK_CK_BUFHCLK0"]
        );
    }

    #[test]
    fn emit_rejects_unconfigurable_bram_loudly() {
        // A RAMB18 that is merely marked IN_USE has no configured ports and no
        // INIT data, so it would silently read X. The emitter must refuse
        // rather than ship a bitstream that programs (done=1) and misbehaves.
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
        let err = emit_fasm(&routed, &FakeEmit).expect_err("BRAM must be refused");
        assert!(err.contains("u_bram"), "error must name the cell: {err}");
        assert!(err.contains("RAMB18E1"), "error must name the primitive: {err}");
    }

    #[test]
    fn emit_rejects_unconfigurable_dsp_loudly() {
        use crate::pack::PackedDsp;
        use crate::place::PlacedDsp;
        let packed = Packed7 {
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![PackedDsp { name: "u_dsp".into() }],
            carries: vec![],
        };
        let placed = Placed7 {
            packed: packed.clone(),
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![PlacedDsp { name: "u_dsp".into(), tile: "DSP_L_X0Y0".into(), site: "DSP48_X0Y0".into() }],
            carries: vec![],
        };
        let routed = Routed7 { placed, nets: vec![] };
        let err = emit_fasm(&routed, &FakeEmit).expect_err("DSP must be refused");
        assert!(err.contains("u_dsp"), "error must name the cell: {err}");
        assert!(err.contains("DSP48E1"), "error must name the primitive: {err}");
    }

    #[test]
    fn emit_rejects_unconfigurable_carry_loudly() {
        // A CARRY4 without PRECYINIT has an undefined init; emitting only the
        // propagate muxes would silently miscompute arithmetic.
        use crate::pack::PackedCarry;
        use crate::place::PlacedCarry;
        let packed = Packed7 {
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![PackedCarry { name: "u_carry".into() }],
        };
        let placed = Placed7 {
            packed: packed.clone(),
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![PlacedCarry { name: "u_carry".into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into() }],
        };
        let routed = Routed7 { placed, nets: vec![] };
        let err = emit_fasm(&routed, &FakeEmit).expect_err("CARRY4 must be refused");
        assert!(err.contains("u_carry"), "error must name the cell: {err}");
        assert!(err.contains("PRECYINIT"), "error must explain why: {err}");
    }
}
