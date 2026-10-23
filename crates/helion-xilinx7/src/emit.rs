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

    /// Whether `segbits_<type>.db` defines `FEATURE` for `site` on the IOB
    /// tile `iob_tile`. Used for the termination defaults (`PULLTYPE.NONE`,
    /// `IN_TERM.NONE`), which exist on full `RIOB33`/`LIOB33` tiles but not on
    /// the `*_SING` variants. Returning `false` skips the line rather than
    /// emitting FASM that no segbits file can resolve.
    fn iob_feature_available(&self, iob_tile: &str, site: &str, feature: &str) -> bool;
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

/// OBUF `.DRIVE` feature prefix AND suffix for an (IOSTANDARD, DRIVE) pair,
/// from `segbits_liob33.db`. Xilinx groups standards that share drive-strength
/// bits; the prefix is NOT always the bare standard name, and the suffix is
/// the exact set of strengths that row covers (`I12_I16`, not `I16`), because
/// the row name is the only spelling the segbits DB defines. Returns
/// `(prefix, suffix)`; unknown combinations are a loud error, not a guess.
fn obuf_drive_feature(iostandard: &str, drive: &str) -> Result<(&'static str, &'static str), String> {
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
        // `LVCMOS33.DRIVE.I16` MUST NOT be used for an output pad: on
        // `IOB_Y0` it asserts `38_118` / `39_117` instead of clearing
        // `38_118` and asserting `38_126` / `39_125`, and `38_118` is shared
        // with the `IN_ONLY` feature that disables the output driver. The pad
        // then configures, `done` reads 1, and the pin never moves.
        //
        // Bit evidence (segbits_riob33.db, IOB_Y0):
        //   `LVCMOS33.DRIVE.I16` =
        //     38_64 !38_74 !38_76 !38_98 !38_100 !38_102 !38_112 38_118
        //     !38_126 39_65 !39_75 !39_97 !39_101 39_117 !39_119 !39_125 39_127
        //   `LVCMOS33_LVTTL.DRIVE.I12_I16` =
        //     38_64 !38_74 !38_76 !38_98 !38_100 !38_102 !38_112 !38_118
        //     38_126 39_65 !39_75 !39_97 !39_101 !39_117 !39_119 39_125 39_127
        // Ground truth: a real Vivado bitstream for an LVCMOS33 output pad
        // (prjxray-db/artix7/harness/arty-a7/swbut/design.bit, tile
        // RIOB33_X43Y51) emits `LVCMOS33_LVTTL.DRIVE.I12_I16`. Every
        // LVCMOS33 strength X-Ray models (I4/I12/I16) lives on a row of the
        // shared LVCMOS33_LVTTL group.
        ("LVCMOS33", "I4" | "I12" | "I16") => "LVCMOS33_LVTTL",
        ("LVTTL", "I24") => "LVTTL",
        ("LVTTL", "I4" | "I12" | "I16") => "LVCMOS33_LVTTL",
        ("LVDS_25", "I") => "LVDS_25",
        ("SSTL135", "I") => "SSTL135",
        ("TMDS_33", "I") => "TMDS_33",
        _ => {
            return Err(format!(
                "emit: unsupported (IOSTANDARD, DRIVE) = ({iostandard}, {drive})"
            ))
        }
    };
    // The suffix is the row's full strength set. For the shared groups the
    // requested strength is a member of that set, so name the row exactly.
    // Every arm returns a 'static literal so the pair can be returned by value.
    let suffix = match (prefix, drv.as_str()) {
        ("LVCMOS33_LVTTL", "I8") => "I12_I8",
        ("LVCMOS33_LVTTL", _) => "I12_I16",
        ("LVCMOS12", "I4") => "I4",
        ("LVCMOS12", _) => "I12",
        ("LVCMOS12_LVCMOS25", _) => "I8",
        ("LVCMOS15", "I8") => "I8",
        ("LVCMOS15", _) => "I12",
        ("LVCMOS15_LVCMOS18_LVCMOS25", _) => "I4",
        ("LVCMOS15_SSTL15", _) => "I16_I_FIXED",
        ("LVCMOS18", "I12") => "I12_I8",
        ("LVCMOS18", "I24") => "I24",
        ("LVCMOS18", _) => "I16",
        ("LVCMOS25", "I12") => "I12",
        ("LVCMOS25", _) => "I16",
        ("LVTTL", _) => "I24",
        // Fixed-strength / differential standards: the segbits suffix is the
        // bare `I`, e.g. `RIOB33.IOB_Y0.LVDS_25.DRIVE.I` and
        // `RIOB33.IOB_Y0.SSTL135.DRIVE.I_FIXED`. `LVDS_25`/`TMDS_33` only
        // define a DRIVE row on the master site IOB_Y0.
        ("LVDS_25" | "TMDS_33", _) => "I",
        ("SSTL135", _) => "I_FIXED",
        _ => {
            return Err(format!(
                "emit: no DRIVE suffix for (IOSTANDARD, DRIVE) = ({iostandard}, {drive})"
            ))
        }
    };
    Ok((prefix, suffix))
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
                // Terminations, exactly as for an output pad. An input pad
                // needs them just as much: without `PULLTYPE.NONE` the IOB
                // keeps its power-on pull-down, which loads a driven input
                // and visibly degrades it. On the ZedBoard 100 MHz clock pin
                // (Y9 -> LIOB33_X0Y25.IOB_Y0) that was the difference between
                // a working clock and a dead design: a real Vivado bitstream
                // for this board carries `PULLTYPE.NONE` there, and we did
                // not. Ground truth, tile LIOB33_X0Y25 of
                // ~/CDA4253_A1/blinky.bit: IOB_Y0 has
                // `IN_TERM.NONE` + `PULLTYPE.NONE` + `SLEW.FAST` + `IN`.
                //
                // `SLEW` is deliberately NOT emitted here: it selects the
                // output driver's edge rate and an input-only pad has no
                // driver to shape. Vivado does set SLEW.FAST on this pin, but
                // that is its 3.3 V input default rather than something the
                // clock needs, and inventing it would be guessing.
                for feat in ["PULLTYPE.NONE", "IN_TERM.NONE"] {
                    if device.iob_feature_available(&iob.tile, &site_rel, feat) {
                        lines.push(format!("{}.{}.{}", iob.tile, site_rel, feat));
                    }
                }
            }
            IobDir::Out => {
                // OBUF: output driver enabled by DRIVE select (no IN_ONLY).
                // The DRIVE feature prefix depends on the (standard, drive)
                // pair (see obuf_drive_prefix).
                let drive = p.drive.as_deref().unwrap_or("I16");
                let (prefix, suffix) = obuf_drive_feature(&p.iostandard, drive)?;
                lines.push(format!(
                    "{}.{}.{}.DRIVE.{}",
                    iob.tile, site_rel, prefix, suffix
                ));
                let slew = p.slew.as_deref().unwrap_or("SLOW");
                lines.push(format!(
                    "{}.{}.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVTTL_SSTL135_SSTL15.SLEW.{}",
                    iob.tile,
                    site_rel,
                    slew.to_ascii_uppercase()
                ));
                // Terminations. Ground truth: a real Vivado bitstream for a
                // plain LVCMOS33 output pad always carries both of these on
                // every IOB of the tile (see
                // prjxray-db/artix7/harness/arty-a7/swbut/design.bit, tile
                // RIOB33_X43Y51: `PULLTYPE.NONE` and `IN_TERM.NONE` appear for
                // IOB_Y0 and IOB_Y1). They are not decorative: without
                // `PULLTYPE.NONE` the IOB keeps its default pull-up, and
                // without `IN_TERM.NONE` the unterminated-input bits are left
                // at 0, which on IOB_Y1 asserts `IN_TERM.UNTUNED_SPLIT_40`.
                // Emit them only where the DB defines them, so tiles whose
                // segbits lack the row keep working.
                for feat in ["PULLTYPE.NONE", "IN_TERM.NONE"] {
                    if device.iob_feature_available(&iob.tile, &site_rel, feat) {
                        lines.push(format!("{}.{}.{}", iob.tile, site_rel, feat));
                    }
                }
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
                    // `OSERDES.DATA_RATE_TQ.BUF` is the third row a real
                    // Vivado bitstream always carries on a used OLOGIC: it
                    // selects the buffered (non-DDR, non-SDR) tri-state path.
                    // Ground truth, tile RIOI3_X43Y51 of
                    // prjxray-db/artix7/harness/arty-a7/swbut/design.bit:
                    // bits 32_66=1, 32_70=0, 33_69=0 -- exactly BUF.
                    for feat in ["OMUX.D1", "OQUSED", "OSERDES.DATA_RATE_TQ.BUF"] {
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
    // The BUFG input selects this design's buffers need, as `(tile, wire)`.
    // The block below writes them; a routed pip that lands on a *different*
    // buffer's input would drive the same configuration bit from a second
    // source, and prjxray rejects the whole bitstream:
    //
    //   FASM line "...BUFGCTRL0_I0.CLK_BUFG_IMUX28_0" wanted to clear bit
    //   (4329626, 93, 8) but was set by
    //   "...BUFGCTRL0_I0.CLK_BUFG_BOT_R_CK_MUXED0"
    //
    // This turned up only once the router had a working A* heuristic: with real
    // distances, the search started preferring a short hop *through* an unused
    // BUFG slot to reach the clock spine. Denying those wires to the search was
    // tried and is wrong -- `CK_MUXED<n>` is shared by every BUFG in a tile, so
    // blocking the wire also blocks the design's own legal entry. Filtering
    // here, where the set of legitimate selects is known, keeps the search free
    // and the emitted bits unambiguous.
    let mut owned_bufg_inputs: HashSet<(String, String)> = HashSet::new();
    for bufg in &placed.bufgs {
        if let Some((top, slot)) = bufg_slot_of(device, bufg) {
            let mux_tile = if top { "CLK_BUFG_TOP_R" } else { "CLK_BUFG_BOT_R" };
            owned_bufg_inputs.insert((
                bufg.tile.clone(),
                format!("{mux_tile}_CK_MUXED{}", 2 * slot),
            ));
        }
    }

    for net in &routed.nets {
        if let RouteKind::Pips(pips) = &net.kind {
            for pip in pips {
                // A pip whose DESTINATION is a BUFG instance mux belongs to the
                // clock buffer, not to this net.
                if is_bufg_instance_mux(device, &pip.tile, &pip.dst_wire) {
                    continue;
                }
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
    } else if tile_type.starts_with("LIOI3") {
        // Global-clock tap at the input pin. The IBUF output reaches the
        // clock spine through `LIOI_I2GCLK_TOP0.IOI_ILOGIC0_O`, and that is a
        // pseudo-pip the DB marks `always` -- it is the ONLY legal source, so
        // the signal necessarily passes through the ILOGIC. A working Vivado
        // bitstream additionally sets `ILOGIC_Y0.ZINV_D` on that tile, which
        // drives the D input inverted; without it the ILOGIC does not pass
        // the signal through and the clock stops at the pin (the counter
        // then sits frozen at its init value). `ZINV_D` is a real segbit
        // (`LIOI3.ILOGIC_Y0.ZINV_D 29_109`), not a pseudo-pip, so it has to
        // be requested explicitly.
        if dst.contains("I2GCLK") || src.contains("I2GCLK") {
            // Name the ILOGIC that owns the tap: TOP -> Y0, BOT -> Y1.
            let side = if dst.contains("I2GCLK_TOP") || src.contains("I2GCLK_TOP") {
                "Y0"
            } else {
                "Y1"
            };
            out.push(format!("ILOGIC_{side}.ZINV_D"));
        }
    } else if tile_type.starts_with("CLK_HROW") {
        if src.starts_with("CLK_HROW_CK_IN_") {
            out.push(format!("{src}_ACTIVE"));
        } else if src.starts_with("CLK_HROW_") && src.contains("_CK_GCLK") {
            out.push(format!("{src}_ACTIVE"));
        }
        // The HROW stage is `CLK_HROW_CK_MUX_OUT_{L,R}<n>` (the mux) feeding
        // the BUFHCE that actually drives the spine. The mux alone is not
        // enough: the BUFHCE has its own `IN_USE` enable, and without it the
        // branch is configured but muted. Check BOTH wires, because the mux
        // appears as `dst` for the pip that leaves it and as `src` for the
        // pip that follows.
        // Index `n` pairs the two (verified in segbits_clk_hrow_bot_r.db:
        // `MUX_OUT_R8` is word 26_364/27_363 and `BUFHCE_X1Y8` is
        // 27_358/27_359/26_355/27_355), so derive it rather than hardcoding.
        if let Some(n) = hrow_mux_index(dst).or_else(|| hrow_mux_index(src)) {
            out.push(format!("BUFHCE.BUFHCE_X1Y{n}.IN_USE"));
            out.push(format!("BUFHCE.BUFHCE_X1Y{n}.ZINV_CE"));
        }
    } else if tile_type == "HCLK_R" || tile_type == "HCLK_L" {
        // The ENABLE_BUFFER bit is REQUIRED, not decorative: it gates the
        // horizontal clock buffer that feeds the leaf clock spine. Emitting
        // only `HCLK_LEAF_CLK_B_TOPL5.HCLK_CK_BUFHCLK<n>` (the selection)
        // leaves the buffer disabled, so the global clock reaches the HROW
        // mux and then stops -- every FF on that spine keeps its init value.
        //
        // The left arm was missing entirely, and a left-side clock is exactly
        // what a ZedBoard has: the 100 MHz GCLK enters on Y9 ->
        // LIOB33_X0Y25 (left edge) -> HCLK_L. So the one board this backend
        // targets was the one case that never got its buffer enabled.
        // Ground truth, tile HCLK_L_X174Y26 of the working Vivado bitstream
        // ~/CDA4253_A1/blinky.bit: `ENABLE_BUFFER.HCLK_CK_BUFHCLK8` present.
        // The two tile types have identical segbit shapes
        // (`ENABLE_BUFFER.HCLK_CK_BUFHCLK<n>`; see segbits_hclk_{l,r}.db),
        // so one arm covers both.
        if src.starts_with("HCLK_CK_BUFHCLK") {
            out.push(format!("ENABLE_BUFFER.{src}"));
        }
    }
    out
}

/// Index of a `CLK_HROW_CK_MUX_OUT_{L,R}<n>` wire, which selects the matching
/// `BUFHCE` instance in the same HROW tile.
fn hrow_mux_index(wire: &str) -> Option<u32> {
    let rest = wire.strip_prefix("CLK_HROW_CK_MUX_OUT_")?;
    let rest = rest.strip_prefix('L').or_else(|| rest.strip_prefix('R'))?;
    rest.parse().ok()
}

/// Numeric index in the first `GCLK<digits>` of `wire`.
fn gclk_index(wire: &str) -> Option<u32> {    let start = wire.find("GCLK")? + "GCLK".len();
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
/// Is `wire` in `tile` one of a clock buffer's instance muxes?
///
/// These are the `CLK_BUFG_BUFGCTRL<n>_I0/I1` selects that `bufg_slot` and the
/// BUFGCTRL loop below write. They belong to the buffer, never to general
/// routing.
use crate::place::PlacedBufg;

fn is_bufg_instance_mux(device: &dyn EmitDevice, tile: &str, wire: &str) -> bool {
    device
        .tile_type(tile)
        .map(|t| t.contains("CLK_BUFG") && wire.contains("BUFGCTRL"))
        .unwrap_or(false)
}

/// `(is_top, slot)` for a placed buffer, or `None` if it cannot be determined.
fn bufg_slot_of(device: &dyn EmitDevice, bufg: &PlacedBufg) -> Option<(bool, u32)> {
    let site_rel = device.site_rel(&bufg.tile, &bufg.site)?;
    let tile_type = device.tile_type(&bufg.tile)?;
    bufg_slot(&site_rel, &tile_type).ok()
}

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

        fn iob_feature_available(&self, _iob: &str, _site: &str, _feature: &str) -> bool {
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
        // Ground-truth output pad. A real Vivado bitstream for an LVCMOS33
        // output (prjxray-db/artix7/harness/arty-a7/swbut/design.bit, tile
        // RIOB33_X43Y51) carries exactly these on each IOB of the tile. The
        // drive row is the shared LVCMOS33_LVTTL one: `LVCMOS33.DRIVE.I16`
        // also asserts the IN_ONLY bit 39_09 and would hold the driver off.
        assert!(
            text.contains("RIOB33_X0Y0.IOB_Y1.LVCMOS33_LVTTL.DRIVE.I12_I16"),
            "got:\n{text}"
        );
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.PULLTYPE.NONE"), "got:\n{text}");
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.IN_TERM.NONE"), "got:\n{text}");
        assert!(
            text.contains("RIOB33_X0Y0.IOB_Y1.LVCMOS12_LVCMOS15_LVCMOS18_LVCMOS25_LVCMOS33_LVTTL_SSTL135_SSTL15.SLEW.SLOW"),
            "got:\n{text}"
        );
        // And the OLOGIC side, also from the same reference bitstream.
        assert!(text.contains("RIOI3_X0Y0.OLOGIC_Y1.OMUX.D1"), "got:\n{text}");
        assert!(text.contains("RIOI3_X0Y0.OLOGIC_Y1.OQUSED"), "got:\n{text}");
        assert!(
            text.contains("RIOI3_X0Y0.OLOGIC_Y1.OSERDES.DATA_RATE_TQ.BUF"),
            "got:\n{text}"
        );
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
            vec![
                "CLK_HROW_CK_IN_L0_ACTIVE",
                "BUFHCE.BUFHCE_X1Y0.IN_USE",
                "BUFHCE.BUFHCE_X1Y0.ZINV_CE"
            ]
        );
        assert_eq!(
            clock_companions(
                "CLK_HROW_BOT_R",
                "CLK_HROW_CK_MUX_OUT_R0",
                "CLK_HROW_R_CK_GCLK0"
            ),
            vec![
                "CLK_HROW_R_CK_GCLK0_ACTIVE",
                "BUFHCE.BUFHCE_X1Y0.IN_USE",
                "BUFHCE.BUFHCE_X1Y0.ZINV_CE"
            ]
        );
        assert_eq!(
            clock_companions("HCLK_R", "", "HCLK_CK_BUFHCLK0"),
            vec!["ENABLE_BUFFER.HCLK_CK_BUFHCLK0"]
        );
    }

    #[test]
    fn clock_companions_enable_ilogic_clock_tap() {
        assert_eq!(
            clock_companions("LIOI3", "LIOI_I2GCLK_TOP0", "IOI_ILOGIC0_O"),
            vec!["ILOGIC_Y0.ZINV_D"]
        );
    }

    #[test]
    fn clock_companions_enable_bufhce_on_hrow() {
        // The HROW stage is a mux (`MUX_OUT_R<n>`) feeding a BUFHCE that
        // actually drives the spine. Emitting only the mux leaves the buffer
        // disabled: the clock reaches the mux and stops, so every FF keeps its
        // init value. `n` must be derived from the mux, not hardcoded.
        // The mux arrives as `dst` (the pip that leaves it) and as `src`
        // (the pip that follows); both must yield the enable.
        assert_eq!(
            clock_companions("CLK_HROW_BOT_R", "CLK_HROW_CK_MUX_OUT_R8", "X"),
            vec!["BUFHCE.BUFHCE_X1Y8.IN_USE", "BUFHCE.BUFHCE_X1Y8.ZINV_CE"]
        );
        assert_eq!(
            clock_companions("CLK_HROW_BOT_R", "X", "CLK_HROW_CK_MUX_OUT_R9"),
            vec!["BUFHCE.BUFHCE_X1Y9.IN_USE", "BUFHCE.BUFHCE_X1Y9.ZINV_CE"]
        );
        assert_eq!(
            clock_companions("CLK_HROW_TOP_R", "CLK_HROW_CK_MUX_OUT_L3", "X"),
            vec!["BUFHCE.BUFHCE_X1Y3.IN_USE", "BUFHCE.BUFHCE_X1Y3.ZINV_CE"]
        );
    }

    #[test]
    fn hrow_mux_index_parses_both_sides() {
        assert_eq!(hrow_mux_index("CLK_HROW_CK_MUX_OUT_R0"), Some(0));
        assert_eq!(hrow_mux_index("CLK_HROW_CK_MUX_OUT_R11"), Some(11));
        assert_eq!(hrow_mux_index("CLK_HROW_CK_MUX_OUT_L7"), Some(7));
        assert_eq!(hrow_mux_index("CLK_HROW_CK_IN_L7"), None);
        assert_eq!(hrow_mux_index("HCLK_CK_BUFHCLK9"), None);
    }

    #[test]
    fn clock_companions_enable_left_side_hclk() {
        // A left-edge clock (the ZedBoard 100 MHz GCLK on Y9) enters through
        // HCLK_L. Only HCLK_R had an arm, so the horizontal clock buffer was
        // never enabled and the clock died at HROW. Both tile types carry an
        // identical `ENABLE_BUFFER.HCLK_CK_BUFHCLK<n>` segbit shape.
        assert_eq!(
            clock_companions("HCLK_L", "", "HCLK_CK_BUFHCLK8"),
            vec!["ENABLE_BUFFER.HCLK_CK_BUFHCLK8"]
        );
        assert_eq!(
            clock_companions("HCLK_L", "", "HCLK_CK_BUFHCLK9"),
            vec!["ENABLE_BUFFER.HCLK_CK_BUFHCLK9"]
        );
        assert_eq!(
            clock_companions("HCLK_R", "", "HCLK_CK_BUFHCLK8"),
            vec!["ENABLE_BUFFER.HCLK_CK_BUFHCLK8"]
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
