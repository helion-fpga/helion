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
    /// Features the part requires in *every* bitstream, from the X-Ray DB's
    /// `required_features.fasm`.
    ///
    /// These are not optional decoration: the DB ships that file precisely
    /// because a bitstream without them is not a valid configuration for the
    /// part. Every Helion bitstream produced before this was added omitted
    /// them, while the working Vivado reference sets all three.
    fn required_features(&self) -> Vec<String> {
        Vec::new()
    }
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

    /// Every tile of `tile_type` on the part, as `(name, grid_y)` pairs.
    ///
    /// Clock distribution is a *chain*, not a single stage: `CLK_BUFG_REBUF`
    /// repeaters are spaced down the clock column, and the clock only reaches
    /// logic past a repeater once that repeater is enabled. The emitter sees
    /// only the tiles it was routed through, so it cannot discover the rest of
    /// the chain from the netlist alone and needs the device to enumerate them.
    ///
    /// The default returns nothing, which keeps the caller on its previous
    /// single-stage behaviour if a device cannot answer.
    fn tiles_of_type(&self, _tile_type: &str) -> Vec<(String, i32)> {
        Vec::new()
    }
    /// The device row of `tile`, in the same space `tiles_of_type` reports.
    ///
    /// This is the tilegrid's `grid_y`, which is **not** the number in the tile
    /// name: the name counts rows bottom-up and `grid_y` top-down. Anything
    /// that groups tiles by row must use this, or the two spaces silently
    /// never match.
    fn tile_grid_y(&self, _tile: &str) -> Option<i32> {
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

    // Features every bitstream for this part must contain, straight from the
    // DB's `required_features.fasm`. Emit them first and unconditionally.
    //
    // These are the static "always on" housekeeping bits. Nothing in the
    // design references them, so they cannot be derived from routing: if the
    // part directory does not ship the file they are simply absent, and the
    // bitstream is quietly missing mandatory configuration. Some part
    // directories for this die do ship it, so an absent file is a DB gap, not
    // an absence of requirements. Warn rather than omit in silence.
    let required = device.required_features();
    if required.is_empty() {
        eprintln!(
            "emit: warning: no required_features.fasm for this part, so the \
             always-on configuration bits are not being emitted. Other part \
             directories for the same die ship this file."
        );
    }
    lines.extend(required);

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
    // - {bel}.ZRST: SRVAL, the value the register takes on set/reset. This is
    //   NOT "the SR pin acts as reset" and it is NOT conditional on the FF
    //   having an SR net -- it is one half of the INIT/SRVAL pair and is
    //   emitted for every FF, alongside ZINI. See the block below.
    // - {slice}.CEUSEDMUX: slice-level, enables the CE input mux. Needed
    //   when any FF in the slice has a CE net.
    // - {slice}.SRUSEDMUX: slice-level, enables the SR input mux. Needed
    //   when any FF in the slice has an SR net.
    // Without CEUSEDMUX/SRUSEDMUX, the FF silently ignores CE/SR pins.
    use std::collections::HashSet;
    let mut ce_slices: HashSet<(String, String)> = HashSet::new();
    // Slices containing at least one FF: `FFSYNC` is emitted once per slice.
    let mut ff_slices: HashSet<(String, String)> = HashSet::new();
    let mut sr_slices: HashSet<(String, String)> = HashSet::new();
    for ff in &placed.ffs {
        let site_rel = device
            .site_rel(&ff.tile, &ff.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", ff.tile, ff.site))?;
        let bel = ff_bel(&ff.bel)?; // "AFF" -> "AFF"
        // D from the paired LUT's O6 output; `mux_for_ff` picks O6 when the
        // FF's D is driven by the LUT on the same BEL letter, and the raw
        // slice input pin otherwise.
        let mux_src = mux_for_ff(ff, packed, placed);
        lines.push(format!("{}.{}.{}.ZINI", ff.tile, site_rel, bel));
        lines.push(format!("{}.{}.{}MUX.{}", ff.tile, site_rel, bel, mux_src));
        // ZRST is emitted on EVERY flip-flop, not only ones with an SR net.
        //
        // prjxray's FF config model has exactly two bits per FF: `ZINI` (the
        // power-up INIT value) and `ZRST` (SRVAL, the value the register takes
        // on set/reset). They are a pair, and Vivado writes both for every FF
        // it places -- across the reference blinky it sets ZRST on all 26 FFs
        // while SRUSEDMUX is 0 everywhere, i.e. with no SR net at all.
        //
        // Leaving ZRST clear leaves SRVAL at its default, which is not the
        // same as "no effect": the register's stored value is then 1, so a
        // design that inits to 0 powers up as a constant 1. That is exactly
        // the "LED on and frozen" symptom -- it looked like a dead clock, but
        // the clock was fine and the register was simply pinned high.
        //
        // The earlier comment here claimed ZRST meant "SR pin acts as RESET"
        // and only mattered when an SR net existed. That reading is wrong: it
        // is the SRVAL selector, and the reset-vs-set choice is a separate
        // concern.
        lines.push(format!("{}.{}.{}.ZRST", ff.tile, site_rel, bel));
        // Control signals: look up the packed FF for CE/SR nets.
        if let Some(p) = packed.ffs.iter().find(|p| p.name == ff.name) {
            if p.ce_net.is_some() {
                ce_slices.insert((ff.tile.clone(), site_rel.clone()));
            }
            if p.sr_net.is_some() {
                sr_slices.insert((ff.tile.clone(), site_rel.clone()));
            }
        }
        ff_slices.insert((ff.tile.clone(), site_rel.clone()));
    }
    // `FFSYNC` configures the flip-flop's set/reset as synchronous rather than
    // asynchronous. It is a slice-level bit with no `!` polarity
    // (`CLBLM_R.SLICEM_X0.FFSYNC 00_48`), and Vivado sets it on *every* slice
    // it places a flip-flop in: across the reference blinky it is present on
    // 7 of 7 FF tiles. Helion emitted it on none, in any design ever built.
    //
    // A flip-flop left with this unset has its set/reset control in the wrong
    // mode, and the register does not clock.
    for (tile, slice) in &ff_slices {
        lines.push(format!("{}.{}.FFSYNC", tile, slice));
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
    // A CARRY4 is configured by three things, and all three are mandatory:
    //
    //   1. `CARRY4.{A,B,C,D}CY0` -- the per-lane carry mux. In the site model
    //      (`site_type_SLICEL.json`) this is a 2-way mux per lane selecting
    //      between the raw slice pin (`AX`/`BX`/`CX`/`DX`) and `O5`: it is NOT
    //      a "force carry-in to 0" control. Only the lane that actually starts
    //      the chain takes the pin; every other lane ripples from the lane
    //      below. Asserting it on all four lanes of every element -- which this
    //      emitter used to do -- configures lanes the chain never uses and
    //      diverges from Vivado on 3 bits per element.
    //   2. `PRECYINIT.{C0,C1,AX,CIN}` -- the CHAIN's carry-in. This is a 4-way
    //      MUTUALLY EXCLUSIVE mux, not independent flags.
    //        * On the chain ENTRY the carry-in is a constant or the `AX` pin,
    //          so the arm follows the cell's `cyinit`: `0` -> `C0`, `1` -> `C1`.
    //        * On every LATER element the carry-in arrives on the dedicated
    //          column route from the previous element's `COUT`, which is the
    //          `CIN` arm.
    //      Emitting NOTHING for a later element is the bug this code used to
    //      have: the omitted bits read back as the reset default `C0`, which
    //      ties that element's carry-in to constant 0 and severs the column
    //      route. Each CARRY4 then counts from 0 on its own, so a 25-bit
    //      counter silently becomes seven independent 4-bit counters. The
    //      default arm is *not* "leave it alone": C0 is a real, wrong choice.
    //   3. The FF that consumes a sum bit must select the carry path
    //      (`AFFMUX = XOR`), handled in `mux_for_ff`.
    //
    // The lane `S` LUTs are NOT ordinary packed LUTs: they live in the CARRY4's
    // own slice, one per lane, and the packer records their INIT directly on
    // `PackedCarry` (`lane_s_init`) rather than as separate LUT cells. So the
    // generic LUT loop above never sees them and they must be written here.
    //
    // Omitting them leaves each lane's `S` input undriven in the fabric while
    // the netlist still connects it, so the chain computes garbage (or, with
    // the carry-in also wrong, freezes). Vivado's own blinky writes 48 lane LUT
    // functions, all inside its seven chain slices; before this loop Helion
    // wrote none of them.
    //
    // `DI` needs no LUT: it comes straight from the slice input through the
    // `DI1MUX` select, which is already emitted. That matches Vivado, whose
    // chain slices contain no zero-INIT DI LUTs.
    for carry in &placed.carries {
        let Some(pc) = placed.packed.carries.iter().find(|c| c.name == carry.name) else {
            return Err(format!(
                "emit: placed CARRY4 {} has no packed counterpart",
                carry.name
            ));
        };
        let site_rel = device
            .site_rel(&carry.tile, &carry.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", carry.tile, carry.site))?;
        // Lane order A..D maps to ALUT..DLUT, the same letters the lane pins use.
        for (lane, letter) in ["A", "B", "C", "D"].iter().enumerate() {
            // A lane past the end of its chain has no `S` net; `lane_sum_net`
            // is the marker for "this lane is real".
            if pc.lane_sum_net[lane].is_none() {
                continue;
            }
            lines.push(format!(
                "{}.{}.{}LUT.INIT[63:0] = 64'h{:016X}",
                carry.tile, site_rel, letter, pc.lane_s_init[lane]
            ));
        }
        // The carry mux. Only the element that STARTS the chain takes its
        // lane-A carry from the slice pin; every other lane, and every lane of
        // a later element, ripples. `index` is the element's position in its
        // chain, so `index == 0` identifies the entry unambiguously.
        let is_chain_entry = pc.index == 0;
        if is_chain_entry {
            lines.push(format!("{}.{}.CARRY4.ACY0", carry.tile, site_rel));
        }
        // The chain's carry-in. The entry element uses the constant/`AX` arm
        // selected by `cyinit`; every later element must explicitly select the
        // `CIN` arm, because the reset default (`C0`) would tie its carry-in to
        // 0 and break the inter-element column route.
        let arm = match (is_chain_entry, pc.cyinit.as_deref()) {
            (true, Some("0")) => "C0",
            (true, Some("1")) => "C1",
            (true, Some(other)) => {
                return Err(format!(
                    "emit: CARRY4 {} has cyinit `{other}`, expected `0` or `1`",
                    carry.name
                ))
            }
            // An entry with no recorded `cyinit` is a chain of one whose
            // carry-in is irrelevant; `C0` is the safe, Vivado-matching arm.
            (true, None) => "C0",
            (false, _) => "CIN",
        };
        lines.push(format!("{}.{}.PRECYINIT.{arm}", carry.tile, site_rel));
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
                if std::env::var("HELION_DEBUG_CLK").is_ok()
                    && (tile_type.starts_with("HCLK") || tile_type.starts_with("CLK_BUFG"))
                {
                    eprintln!("CLKPIP tile={} type={tile_type} src={} dst={}",
                        pip.tile, pip.src_wire, pip.dst_wire);
                }
                for feature in clock_companions(&tile_type, &pip.dst_wire, &pip.src_wire) {
                    lines.push(format!("{}.{}", pip.tile, feature));
                }
            }
        }
    }

    // The CMT gates each buffered clock spine it drives with
    // `HCLK_CMT_CK_BUFHCLK<n>_USED`. `n` is NOT the CMT mux index: the mux and
    // the buffer are independent, and `HCLK_CMT_MUX_CLK_<m>` can reach any
    // `CK_BUFHCLK<n>` (segbits_hclk_cmt.db lists all twelve). What fixes `n`
    // is the buffer the clock actually leaves on, i.e. the `BUFHCLK<n>` the
    // downstream `HCLK_L`/`HCLK_R` tile enables.
    //
    // The reference blinky drives mux 0 but enables buffer 8, because the left
    // edge numbers its buffer nets from 8 (`HCLK_CK_INOUT_L0` <->
    // `HCLK_CK_BUFHCLK8`). Deriving the index from the mux therefore produces
    // `BUFHCLK0_USED` and leaves the real buffer 8 ungated -- configured, and
    // muted. Read it from the HCLK tiles instead, once they are all known.
    {
        use std::collections::BTreeSet;
        // Each `BUFHCLK<n>` belongs to exactly one band, and a band's buffers
        // are gated by the CMT that drives that band. Pair them by *band*, not
        // by collecting every index and every CMT-prefixed tile.
        //
        // `HCLK_L.ENABLE_BUFFER` only accepts n in 8..=11 and
        // `HCLK_R.ENABLE_BUFFER` only n in 0..=7 (segbits_hclk_l/r.db), so the
        // index alone says which band a buffer belongs to. Gating is done on
        // `HCLK_CMT_L` for the left band and `HCLK_CMT` for the right.
        //
        // Getting this wrong is silent: gating buffer 8 on the right-band CMT
        // configures a buffer on the far side of the die and leaves the real
        // one disabled, so the spine is muted at its source and every flop on
        // it holds its init value. That is a static LED, not a dead design --
        // which is what made it hard to see.
        const LEFT_BAND_FIRST: u32 = 8;
        let mut left_buf: BTreeSet<u32> = BTreeSet::new();
        let mut right_buf: BTreeSet<u32> = BTreeSet::new();
        for line in &lines {
            // `HCLK_L_X..Y...ENABLE_BUFFER.HCLK_CK_BUFHCLK<n>` and the
            // `HCLK_LEAF_CLK_B_*` selection that carries the same index.
            let tile = line.split('.').next().unwrap_or("");
            let band = if tile.starts_with("HCLK_L_") {
                Some(&mut left_buf)
            } else if tile.starts_with("HCLK_R_") {
                Some(&mut right_buf)
            } else {
                None
            };
            let Some(band) = band else { continue };
            for tag in ["ENABLE_BUFFER.HCLK_CK_BUFHCLK", "HCLK_CK_BUFHCLK"] {
                if let Some(rest) = line.split(tag).nth(1) {
                    if let Ok(n) = rest.trim().parse::<u32>() {
                        band.insert(n);
                    }
                }
            }
        }
        // Gate each band on the CMT tile for that band. `HCLK_CMT_L` is the
        // left-edge CMT (three of them exist; the `_L` in the name is the
        // band, not a side suffix), `HCLK_CMT` without `_L` is the right one.
        //
        // These tiles are discovered from the *device*, not from `lines`: the
        // left CMT has no configuration of its own in a design like blinky, so
        // it never appears in `lines` and a prefix scan over them finds
        // nothing. Only the tile sharing the clock column with the HCLK_L/R
        // tile that enabled the buffer may be gated -- gating the other two
        // left CMTs configures buffers on clock columns this design does not
        // use.
        let mut cmt_left: BTreeSet<String> = BTreeSet::new();
        let mut cmt_right: BTreeSet<String> = BTreeSet::new();
        for (name, _y) in device.tiles_of_type("HCLK_CMT_L") {
            cmt_left.insert(name);
        }
        for (name, _y) in device.tiles_of_type("HCLK_CMT") {
            cmt_right.insert(name);
        }
        // Narrow the left CMTs to the one in the same clock column as the
        // HCLK_L tile that enabled the buffer.
        //
        // Compare `grid_y`, not the number in the tile name. The prjxray
        // tilegrid numbers its rows bottom-up in the name but top-down in
        // `grid_y`: `HCLK_L_X174Y26` and `HCLK_CMT_L_X178Y26` both have
        // `grid_y == 130`. Mixing the two spaces never matches, which is
        // exactly how the left CMT stayed ungated.
        let hclk_l_rows: BTreeSet<i32> = lines
            .iter()
            .filter_map(|line| {
                let tile = line.split('.').next().unwrap_or("");
                if !tile.starts_with("HCLK_L_") {
                    return None;
                }
                device.tile_grid_y(tile)
            })
            .collect();
        let mut left_cmt_for_row: Vec<(i32, String)> = Vec::new();
        for (name, gy) in device.tiles_of_type("HCLK_CMT_L") {
            if hclk_l_rows.contains(&gy) {
                left_cmt_for_row.push((gy, name));
            }
        }
        left_cmt_for_row.sort();
        left_cmt_for_row.dedup();
        // A band's buffers are only gated when that band is actually in use;
        // an empty index set would otherwise emit nothing anyway.
        if !left_buf.is_empty() {
            if left_cmt_for_row.is_empty() {
                // The HCLK_L tile that enabled the buffer has no CMT_L in its
                // row, so there is no tile that can drive it. Silently skipping
                // would leave the spine muted at its source -- a design that
                // configures cleanly and never toggles a flop. Fail instead.
                return Err(format!(
                    "emit: HCLK_L enables BUFHCLK{:?} but no HCLK_CMT_L tile shares \
                     its clock row, so nothing can gate that buffer. The clock \
                     would be configured and muted.",
                    left_buf
                ));
            }
            for (_row, tile) in &left_cmt_for_row {
                for n in &left_buf {
                    lines.push(format!("{tile}.HCLK_CMT_CK_BUFHCLK{n}_USED"));
                }
            }
        }
        if !right_buf.is_empty() {
            for tile in &cmt_right {
                for n in &right_buf {
                    if *n >= LEFT_BAND_FIRST {
                        // Defensive: a right-band index >= 8 has no
                        // `HCLK_R` enable to match, so it is a routing bug
                        // upstream. Refuse rather than configure a buffer
                        // that does not exist on that band.
                        return Err(format!(
                            "emit: BUFHCLK{n} was requested on the right band, but \
                             HCLK_R only drives BUFHCLK0..7 (segbits_hclk_r.db). The \
                             left-band buffers are numbered from {LEFT_BAND_FIRST}."
                        ));
                    }
                    lines.push(format!("{tile}.HCLK_CMT_CK_BUFHCLK{n}_USED"));
                }
            }
        }

        // Repeater bracketing for the global-clock column.
        //
        // `CLK_BUFG_REBUF` tiles are bidirectional repeaters spaced ~25 rows
        // apart down the clock column. `GCLK<n>_ENABLE_ABOVE` drives the clock
        // up out of a repeater and `_BELOW` drives it down, so a repeater only
        // matters when the clock has to travel *past* it. The stage that
        // injects the clock is enabled by `clock_companions`; this pass enables
        // every other repeater between that injection point and the far end of
        // the placed logic, which is what Vivado does.
        //
        // Omitting this is invisible for a design whose flops sit right beside
        // the HROW -- the reference blinky's do -- and silently breaks any
        // design placed further down the column, because the clock stops at
        // the last enabled repeater and those rows never see an edge. Deriving
        // the set from the placed rows keeps it right for a design that spans
        // a different part of the column, rather than hardcoding the two
        // repeaters the reference happens to use.
        {
            let mut inject: Option<i32> = None;
            let mut rebufs: Vec<i32> = Vec::new();
            let mut logic: Vec<i32> = Vec::new();
            let mut gclks: BTreeSet<u32> = BTreeSet::new();
            // INT tiles carrying a clock spine tap, from the emitted pips.
            let mut clock_int_tiles: BTreeSet<String> = BTreeSet::new();
            // Spine index (`GCLK_B<n>`) is a DIFFERENT numbering space from the
            // repeater's `GCLK<n>_ENABLE_*`: the spine index selects which of
            // the 32 global clock nets feeds the tile, while the repeater index
            // names the repeater's own column net. Sharing one set mixes the
            // two, so the fanout pass asked for `GFAN0.GCLK_B0` -- a feature
            // that does not exist -- instead of `GCLK_B11`.
            let mut spine_bits: BTreeSet<u32> = BTreeSet::new();
            for line in &lines {
                let tile = line.split('.').next().unwrap_or("");
                // A spine tap reads `INT_R_X..Y...CLK1.GCLK_B11`: the tile is
                // the one the router chose, and the index is the spine bit.
                if tile.starts_with("INT_") && line.contains(".GCLK_B") {
                    let mut parts = line.split('.');
                    let _tile = parts.next();
                    if let Some(bit) = parts.find(|p| p.starts_with("GCLK_B")) {
                        let digits: String = bit
                            .trim_start_matches("GCLK_B")
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect();
                        if let Ok(n) = digits.parse::<u32>() {
                            spine_bits.insert(n);
                        }
                    }
                    clock_int_tiles.insert(tile.to_string());
                }
                if is_clb_tile(tile) {
                    if let Some(r) = tile_row(tile) {
                        logic.push(r);
                    }
                    continue;
                }
                if !tile.starts_with("CLK_BUFG_REBUF") {
                    continue;
                }
                if line.contains("_ENABLE_") {
                    inject = tile_row(tile).or(inject);
                }
                for n in 0..32u32 {
                    if line.contains(&format!("GCLK{n}_")) {
                        gclks.insert(n);
                    }
                }
            }
            // The routed netlist names only the repeater stage it happened to
            // use. The rest of the chain comes from the device, because the
            // clock cannot reach logic past a repeater that is left disabled.
            let mut chain: Vec<(i32, String)> = device
                .tiles_of_type("CLK_BUFG_REBUF")
                .into_iter()
                .filter_map(|(name, _y)| tile_row(&name).map(|r| (r, name)))
                .collect();
            chain.sort();
            chain.dedup_by_key(|(r, _)| *r);
            rebufs.extend(chain.iter().map(|(r, _)| *r));
            // A repeater is an in-line amplifier, not a tap: the clock reaches
            // logic past a repeater only once that repeater is enabled.
            //
            // Enable the neighbour on each side of the injection stage, then
            // keep walking outward while a placed logic row still lies beyond
            // the repeater just enabled. The first step is unconditional
            // because that is what the reference does -- its flops span Y39-Y45
            // beside an injection at Y38, so neither neighbour is on the path,
            // yet Vivado enables both. Matching that keeps the bitstreams
            // close; continuing to walk only when logic demands it keeps the
            // clock correct for a design spread further down the column, where
            // a fixed one-step rule would strand the far flops with no edge.
            if let (Some(iy), Some(&lo), Some(&hi)) =
                (inject, logic.iter().min(), logic.iter().max())
            {
                let (below, above) = rebuf_rows_to_enable(&rebufs, iy, lo, hi);
                let name_of = |row: i32| -> Option<&str> {
                    chain
                        .iter()
                        .find(|(r, _)| *r == row)
                        .map(|(_, n)| n.as_str())
                };
                for (rows, dir) in [(below, "BELOW"), (above, "ABOVE")] {
                    for row in rows {
                        if let Some(t) = name_of(row) {
                            for n in &gclks {
                                lines.push(format!("{t}.GCLK{n}_ENABLE_{dir}"));
                            }
                        }
                    }
                }
            }

            // Global fanout (bypass) network for every row the clock feeds.
            //
            // A `GCLK_B<n>` tap in an INT tile reaches the slice clock pin in
            // that tile, but it does NOT distribute the clock to the rest of
            // the column. Distribution goes through the bypass network:
            //
            //     INT_R.GCLK_B<n> ->> GFAN0, GFAN1
            //     INT_R.GFAN0     ->> BYP_ALT0, BYP_ALT1, BYP_ALT4, BYP_ALT5
            //     INT_R.GFAN1     ->> BYP_ALT2, BYP_ALT3, BYP_ALT6, BYP_ALT7
            //
            // Omitting these leaves the clock present in exactly one INT tile.
            // The bitstream stays valid, every clock tile matches, `clkdiff`
            // reports no difference, and the design does not run: a single
            // flop toggling its own output stays dark, because its clock input
            // mux is never reached. That is the defect this pass fixes.
            //
            // The arms are read from the clock spine index (`n`) rather than
            // hardcoded: `GFAN0` takes the even `BYP_ALT` arms and `GFAN1` the
            // odd ones, and both are driven by the same `GCLK_B<n>`, so every
            // logic row the clock reaches needs the full set of eight.
            //
            // `GND_WIRE` is the return path that makes the bypass network
            // legal, and the `FAN_ALT` arms carry the distributed clock on to
            // the inter-tile spans. All three appear in every configured row of
            // the reference, so they are emitted together with the arms.
            // The INT tiles the clock actually travels through come from the
            // routed pips (`GCLK_B<n> -> CLK1`), not from the tile's row: the
            // spine tap is in the tile the router chose, which is beside the
            // placed logic, and scanning the die for a tile with the same row
            // finds the far edge of the device instead.
            for int_tile in &clock_int_tiles {
                for n in &spine_bits {
                    lines.push(format!("{int_tile}.GFAN0.GCLK_B{n}"));
                    lines.push(format!("{int_tile}.GFAN1.GCLK_B{n}"));
                }
                for alt in [0u32, 1, 4, 5] {
                    lines.push(format!("{int_tile}.BYP_ALT{alt}.GFAN0"));
                }
                for alt in [2u32, 3, 6, 7] {
                    lines.push(format!("{int_tile}.BYP_ALT{alt}.GFAN1"));
                }
                lines.push(format!("{int_tile}.GFAN0.GND_WIRE"));
                lines.push(format!("{int_tile}.GFAN1.GND_WIRE"));
                lines.push(format!("{int_tile}.FAN_ALT1.NL1BEG_N3"));
                // Final hop INTO the slice: `IMUX4` is the input-mux arm that
                // reaches `CLBLM_M_CLK` (the medium slice's clock pin) through
                // the INT/CLB tileconn. `FAN_BOUNCE1` is the bypass arm that
                // drives it; without this the fanout network is energised but
                // nothing taps it, so the slice clock mux is still unselected
                // and the flops hold their init value.
                //
                // `IMUX4` rather than `IMUX5` because the medium slice's clock
                // is `CLBLM_CLK1`, which the clock tileconn binds to the
                // `IMUX4` arm; `IMUX5` serves the large slice.
                lines.push(format!("{int_tile}.IMUX4.FAN_BOUNCE1"));
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
/// Which `CLK_BUFG_REBUF` repeaters to enable for a clock injected at
/// `inject_y` and feeding logic spanning `lo..=hi`.
///
/// Returns `(below, above)`: the repeater rows to enable in each direction.
/// A repeater is an in-line amplifier rather than a tap, so the clock reaches
/// logic past one only once it is enabled.
///
/// The first repeater on each side is always taken, because that is what the
/// reference bitstream does: its flops sit beside the injection stage, so
/// neither neighbour is strictly on the path, yet Vivado programmes both.
/// Beyond that the walk continues only while a logic row still lies past the
/// repeater just enabled, which keeps the clock correct for a design spread
/// down the column instead of stranding its far flops.
///
/// Kept free of I/O so the rule can be tested directly; the caller does the
/// FASM formatting.
fn rebuf_rows_to_enable(
    chain: &[i32],
    inject_y: i32,
    lo: i32,
    hi: i32,
) -> (Vec<i32>, Vec<i32>) {
    let mut ordered: Vec<i32> = chain.to_vec();
    ordered.sort_unstable();
    ordered.dedup();

    let mut below = Vec::new();
    // Walk outward from the injection so the first step is unconditional.
    for row in ordered.iter().copied().filter(|r| *r < inject_y).rev() {
        below.push(row);
        // Stop once this repeater is at or past the lowest logic row: the
        // next one down feeds nothing this design uses.
        if row <= lo {
            break;
        }
    }

    let mut above = Vec::new();
    for row in ordered.iter().copied().filter(|r| *r > inject_y) {
        above.push(row);
        if row >= hi {
            break;
        }
    }

    (below, above)
}

/// The `Y` index in a tile name, e.g. `CLK_BUFG_REBUF_X87Y38` -> `38`./// The `Y` index in a tile name, e.g. `CLK_BUFG_REBUF_X87Y38` -> `38`.
///
/// Every 7-series tile name ends in `X<col>Y<row>`, so the row is available
/// without a device lookup. Returns `None` for a name that does not carry one,
/// which keeps this total rather than panicking on a hand-written FASM line.
fn tile_row(tile: &str) -> Option<i32> {
    let y = tile.rsplit('Y').next()?;
    // `Y38` must be all digits; `rsplit` on a name with no `Y` yields the whole
    // string, which then fails to parse and correctly reports `None`.
    if y.is_empty() || !y.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    y.parse().ok()
}

/// Is `tile` a CLB, i.e. a tile that can carry user logic?
fn is_clb_tile(tile: &str) -> bool {
    let head = tile.split('_').next().unwrap_or("");
    head == "CLBLM" || head == "CLBLL"
}

/// X-Ray models the clock tree as a mux/route PIP *plus* separate
/// active/enable bits. For example, HROW input `CLK_HROW_CK_IN_L0` also
/// needs `CLK_HROW_CK_IN_L0_ACTIVE`, and the BUFG rebuffer needs
/// `GCLK<n>_ENABLE_ABOVE`/`_BELOW`. Vivado sets both halves; a mux without
/// its companion leaves the branch configured but muted.
///
/// Returns feature strings relative to `tile_type`'s tile (no tile prefix).
fn clock_companions(tile_type: &str, dst: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    if tile_type == "HCLK_CMT" || tile_type == "HCLK_CMT_L" {
        // Both CMT variants carry the same companion shape
        // (`segbits_hclk_cmt.db` and `segbits_hclk_cmt_l.db` agree bit for
        // bit), so one arm covers both. The right-hand tile was the only one
        // handled, which meant a clock entering on the LEFT CMT -- the
        // ZedBoard GCLK on Y9 -- got no companions at all.
        if src.starts_with("HCLK_CMT_CCIO") {
            out.push(format!("{src}_ACTIVE"));
            out.push(format!("{src}_USED"));
        }
        // The CMT also gates each buffered clock spine it drives. Without the
        // `_USED` bit the mux selecting the row is configured but the buffer
        // is not enabled, so the spine is muted at its source.
        // `HCLK_CMT_CK_BUFHCLK<n>_USED` (segbits_hclk_cmt.db 27_185/26_185/...).
        // NOTE: `HCLK_CMT_CK_BUFHCLK<n>_USED` is deliberately NOT emitted
        // here. The mux index available at this point is the wrong number:
        // `HCLK_CMT_MUX_CLK_<m>` can reach any `CK_BUFHCLK<n>`, and the index
        // that matters is the one the downstream HCLK tile enables. Deriving
        // it from the mux yields `BUFHCLK0_USED` for a design that needs
        // `BUFHCLK8_USED`. It is emitted in the post-pass at the end of
        // `emit_fasm`, once the HCLK selection is known.
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
        //
        // The two sides number their buffer nets from opposite ends
        // (`ppips_hclk_{l,r}.db`):
        //
        //   HCLK_R: HCLK_CK_INOUT_R0..R13 <-> HCLK_CK_BUFHCLK0..13
        //   HCLK_L: HCLK_CK_INOUT_L0..L3  <-> HCLK_CK_BUFHCLK8..11
        //
        // The HROW takes row `n` from `INOUT_L<n>`/`INOUT_R<n>`, so a left
        // HCLK must enable `BUFHCLK(8+n)`. Enabling the same `n` the right
        // side would use puts the buffer output on an `INOUT_L` that the
        // programmed `CK_IN_L<n>` never reads, and the clock dies there --
        // the LED lights and never blinks. `src` is the wire the router
        // reached this tile on, so translate it rather than trusting it.
        let left = tile_type == "HCLK_L";
        if let Some(n) = src
            .strip_prefix("HCLK_CK_BUFHCLK")
            .and_then(|d| d.parse::<u32>().ok())
        {
            // Recover the row the clock is on, then re-encode it for this side.
            let row = if left {
                // Router already names a left-side net; trust it only if it
                // is in the left band, else fold it back to a row.
                n.checked_sub(8).unwrap_or(n)
            } else {
                n
            };
            let buf = if left { row + 8 } else { row };
            if buf != n {
                out.push(format!("ENABLE_BUFFER.HCLK_CK_BUFHCLK{buf}"));
                // The leaf selection must match the buffer that is enabled.
                out.push(format!(
                    "HCLK_LEAF_CLK_B_TOPL{row}.HCLK_CK_BUFHCLK{buf}"
                ));
            } else {
                out.push(format!("ENABLE_BUFFER.{src}"));
            }
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
    // An FF whose D is driven by its paired LUT must select the LUT OUTPUT,
    // not a slice input pin.
    //
    // `AFFMUX.{A,B,C,D}X` selects the raw slice input pin of that letter --
    // the path used when the FF takes data straight from outside the slice,
    // bypassing the LUT. Nothing drives that pin for a LUT-fed FF, so the
    // FF's D is left dangling and the flop holds its init value forever.
    // That is the whole reason a counter built this way showed a lit but
    // frozen LED: the clock was fine (the spine matches Vivado byte for
    // byte), the pads were fine, and the FF simply never saw new data.
    //
    // `O6` is the LUT6 output mux (`CLBLM_R.SLICEL_X1.AFFMUX.O6 30_04 ...`),
    // which is what Vivado selects for LUT-driven flops. `O5` would tap the
    // 5-input output instead, and is only right for a 5-input function.
    if let Some(pff) = packed.ffs.iter().find(|f| f.name == ff.name) {
        // A sum bit produced by a CARRY4 is NOT on the LUT output: the slice
        // computes `O = S ^ carry_in`, and `AFFMUX.XOR` is the only mux arm
        // that taps it. Selecting `O6` here would give the FF the propagate
        // term without the carry added, i.e. a counter that runs at the wrong
        // value; selecting `AX` would leave D undriven. Check the carry's sum
        // nets before the LUT path.
        // Match on the sum NET, not on a shared site.
        //
        // A chain is a vertical run of adjacent rows in one column, and it is
        // placed before the flops that consume it, so the chain and its
        // consumers routinely land in different columns. Requiring
        // `pc.site == ff.site` therefore never matches for a real chain: every
        // sum bit goes unconsumed, `mux_for_ff` falls through to the bare
        // `AX`/`BX`/`CX`/`DX` default, and the flop is wired to a slice input
        // pin that nothing drives. The flop then holds its init value forever
        // -- a frozen LED with a byte-perfect clock.
        //
        // The sum net is the real predicate: if this carry element publishes
        // the net sitting on the flop's D, the flop must take `XOR`.
        let carry_feeds_ff = placed.packed.carries.iter().any(|c| {
            c.lane_sum_net
                .iter()
                .flatten()
                .any(|n| *n == pff.d_net)
        });
        if carry_feeds_ff {
            return "XOR";
        }
        for lut in &placed.luts {
            if lut.site == ff.site && lut.bel.starts_with(ff.bel.chars().next().unwrap_or('A')) {
                let drives_ff = packed
                    .luts
                    .iter()
                    .find(|l| l.name == lut.name)
                    .map(|l| l.output_net == pff.d_net)
                    .unwrap_or(false);
                if drives_ff {
                    return "O6";
                }
            }
        }
    }
    // Otherwise the data really does come from the slice input pin, so use
    // the letter-matched one. `BFFMUX.AX`/`CFFMUX.AX`/`DFFMUX.AX` do not
    // exist in segbits, so the letter must be honoured.
    match ff.bel.chars().next().unwrap_or('A') {
        'B' => "BX",
        'C' => "CX",
        'D' => "DX",
        _ => "AX",
    }
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
        /// A miniature repeater column shaped like an xc7z020's: five
        /// repeaters with the injection stage in the middle, so the walk rule
        /// can be exercised without a real part file.
        fn tiles_of_type(&self, tile_type: &str) -> Vec<(String, i32)> {
            if tile_type != "CLK_BUFG_REBUF" {
                return Vec::new();
            }
            [13, 38, 65, 90, 117]
                .into_iter()
                .map(|y| (format!("CLK_BUFG_REBUF_X87Y{y}"), y))
                .collect()
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
        // A LUT-fed FF must select the LUT OUTPUT (`O6`), not a slice input
        // pin. `AFFMUX.AX` selects the raw AX pin, which nothing drives for a
        // LUT-driven flop, so the FF's D dangles and it holds its init value
        // forever -- a lit but frozen LED. This assertion previously demanded
        // `AX`, i.e. it locked in the bug.
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFFMUX.O6"), "got:\n{text}");
        assert!(!text.contains("AFFMUX.AX"), "LUT-fed FF must not use the AX bypass:\n{text}");
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
    /// Every flip-flop in the emitted FASM must carry both halves of its
    /// configuration.
    ///
    /// `ZINI` (power-up INIT) and `ZRST` (SRVAL, the set/reset value) are a
    /// pair, and Vivado writes both for every FF it places. Emitting only
    /// `ZINI` is not a no-op: SRVAL keeps its default of 1, so a register that
    /// inits to 0 powers up as a constant 1 and the design looks frozen. That
    /// fault shipped in every bitstream until it was found by diffing against
    /// a working reference, and no test caught it because prjxray silently
    /// drops unmatched FASM lines, so the assemble stayed green.
    ///
    /// This is a structural invariant, so it holds for any design rather than
    /// the one hand-written fixture.
    /// The global-clock column needs its repeaters enabled, not just the one
    /// stage the route happened to name.
    ///
    /// `CLK_BUFG_REBUF` tiles are in-line amplifiers: the clock reaches logic
    /// past a repeater only once that repeater is enabled. Emitting the enable
    /// for the routed stage alone left `CLK_BUFG_REBUF_X87Y13` and
    /// `_X87Y65` unprogrammed while the reference set both. `clkdiff.py` only
    /// found it once it scanned the whole device instead of one hardcoded path.
    #[test]
    fn rebuf_selection_reaches_the_routed_logic() {
        use super::rebuf_rows_to_enable as pick;
        // An xc7z020 column, injection at Y38 -- the reference blinky's shape.
        let chain = [13, 38, 65, 90, 117, 142];
        let (below, above) = pick(&chain, 38, 42, 48);
        assert_eq!(below, vec![13], "the neighbour below the injection");
        assert_eq!(above, vec![65], "the neighbour above the injection");

        // The first step on each side is unconditional, so logic that stops
        // just above the injection still pulls in the neighbour.
        let (below, above) = pick(&chain, 38, 5, 40);
        assert_eq!(below, vec![13], "one repeater covers down to Y5");
        assert_eq!(above, vec![65], "the unconditional first step upward");

        // Logic spread far down the column must pull in every repeater it
        // passes, or the rows past the last enabled one never see an edge.
        let (below, above) = pick(&chain, 38, -20, 40);
        assert_eq!(below, vec![13], "Y13 is the last repeater above Y-20");
        assert_eq!(above, vec![65]);

        // Symmetrically upward: walk until the highest logic row is covered.
        let (below, above) = pick(&chain, 38, 39, 120);
        assert_eq!(below, vec![13], "unconditional first step downward");
        assert_eq!(above, vec![65, 90, 117, 142], "covers up to Y120");
    }

    #[test]
    fn every_ff_gets_both_zini_and_zrst() {
        use std::collections::BTreeSet;
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let ini: BTreeSet<&str> = bs
            .fasm
            .iter()
            .filter_map(|l| l.strip_suffix(".ZINI"))
            .collect();
        let rst: BTreeSet<&str> = bs
            .fasm
            .iter()
            .filter_map(|l| l.strip_suffix(".ZRST"))
            .collect();
        assert!(
            !ini.is_empty(),
            "expected the blinky to place flip-flops; found no .ZINI at all"
        );
        let missing: Vec<&&str> = ini.difference(&rst).collect();
        let extra: Vec<&&str> = rst.difference(&ini).collect();
        assert!(
            missing.is_empty(),
            "these FFs have ZINI but no ZRST, so SRVAL stays at its default 1 \
             and they power up as constants: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "these FFs have ZRST but no ZINI: {extra:?}"
        );
    }

    #[test]
    fn emit_ff_no_control_signals() {
        // An FF with no CE/SR nets must NOT emit the slice mux enables: with
        // no CE or SR net there is nothing for those muxes to select.
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
        // ZRST *is* expected even without an SR net. It is SRVAL, the value
        // the register takes on set/reset, and it must be configured on every
        // FF -- Vivado sets it on all 26 FFs of the reference while SRUSEDMUX
        // is 0 everywhere. Omitting it leaves SRVAL at its default 1, so the
        // register powers up as a constant 1: the LED lights and never
        // blinks. This assertion used to demand its absence, i.e. it locked
        // in the bug.
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZRST"),
            "ZRST must be emitted on every FF, got:\n{text}"
        );
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
        // `HCLK_CMT_L` is a distinct tile type and there are three of them on
        // an xc7z020 against one `HCLK_CMT`. Vivado routes the ZedBoard clock
        // through the L variant, so an untested branch here is the one most
        // likely to be wrong in the field.
        assert_eq!(
            clock_companions("HCLK_CMT_L", "", "HCLK_CMT_CCIO0"),
            vec!["HCLK_CMT_CCIO0_ACTIVE", "HCLK_CMT_CCIO0_USED"],
            "HCLK_CMT_L must emit the same CCIO companions as HCLK_CMT"
        );
        // Both CCIO arms are numbered independently, so check the second one
        // too rather than trusting index 0 to generalise.
        assert_eq!(
            clock_companions("HCLK_CMT_L", "", "HCLK_CMT_CCIO6"),
            vec!["HCLK_CMT_CCIO6_ACTIVE", "HCLK_CMT_CCIO6_USED"]
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

    /// A configured CARRY4 chain entry must emit its lane-A carry mux plus the
    /// PRECYINIT arm its `cyinit` selects. `PRECYINIT` is a 4-way mutually
    /// exclusive mux, so emitting two arms would drive the same bits from two
    /// sources and prjxray rejects the bitstream.
    ///
    /// The entry emits `ACY0` only. The old emitter emitted all four `{A..D}CY0`
    /// and claimed that matched Vivado; decoding the real Vivado blinky shows
    /// the entry sets `ACY0` alone, because the mux selects one lane's carry
    /// source rather than gating every lane.
    #[test]
    fn emit_writes_carry4_muxes_and_one_precyinit_arm() {
        use crate::pack::PackedCarry;
        use crate::place::PlacedCarry;
        let mut pc = PackedCarry::empty("u_carry");
        pc.chain = "c".into();
        pc.index = 0;
        pc.chain_len = 1;
        pc.cyinit = Some("1".into());
        let packed = Packed7 {
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![pc],
        };
        let placed = Placed7 {
            packed: packed.clone(),
            luts: vec![],
            ffs: vec![],
            iobs: vec![],
            bufgs: vec![],
            brams: vec![],
            dsps: vec![],
            carries: vec![PlacedCarry {
                name: "u_carry".into(),
                tile: "CLBLL_L_X0Y0".into(),
                site: "SLICE_X0Y0".into(),
            }],
        };
        let routed = Routed7 { placed, nets: vec![] };
        let bs = emit_fasm(&routed, &FakeEmit).expect("a configured CARRY4 must emit");
        let fasm = bs.fasm.join("\n");
        assert!(
            fasm.contains("CARRY4.ACY0"),
            "the chain entry takes lane A from the slice pin:\n{fasm}"
        );
        // Only the entry lane. A later lane's carry ripples from the lane below,
        // so configuring it as a pin-fed mux is wrong.
        for lane in ["B", "C", "D"] {
            assert!(
                !fasm.contains(&format!("CARRY4.{lane}CY0")),
                "lane {lane} must ripple, not take a pin:\n{fasm}"
            );
        }
        // The `+1` chain entry.
        assert!(
            fasm.contains("PRECYINIT.C1"),
            "a cyinit of `1` must select PRECYINIT.C1:\n{fasm}"
        );
        // Mutually exclusive: no second arm.
        for arm in ["PRECYINIT.C0", "PRECYINIT.CIN", "PRECYINIT.AX"] {
            assert!(!fasm.contains(arm), "PRECYINIT arms are exclusive, saw {arm}");
        }
    }

    /// A chain element past the first is fed by the previous element's `COUT`
    /// over the dedicated column route, so it must select `PRECYINIT.CIN`.
    ///
    /// This is the regression that froze the LED. The old emitter omitted
    /// PRECYINIT entirely for chained elements and the test asserted that
    /// omission. But omitted bits read back as the reset default `C0`, which
    /// ties the element's carry-in to constant 0 and severs the column route:
    /// each CARRY4 then counts from 0 on its own, so a 25-bit counter becomes
    /// seven 4-bit counters. The netlist is still correct, which is why a
    /// netlist-level simulation (work/sim3.py) kept passing while the bitstream
    /// was wrong.
    #[test]
    fn emit_selects_cin_for_chained_elements() {
        use crate::pack::PackedCarry;
        use crate::place::PlacedCarry;
        let mut pc = PackedCarry::empty("u_carry1");
        pc.chain = "c".into();
        pc.index = 1;
        pc.chain_len = 2;
        pc.cyinit = None;
        let packed = Packed7 {
            luts: vec![], ffs: vec![], iobs: vec![], bufgs: vec![],
            brams: vec![], dsps: vec![], carries: vec![pc],
        };
        let placed = Placed7 {
            packed: packed.clone(),
            luts: vec![], ffs: vec![], iobs: vec![], bufgs: vec![],
            brams: vec![], dsps: vec![],
            carries: vec![PlacedCarry {
                name: "u_carry1".into(),
                tile: "CLBLL_L_X0Y1".into(),
                site: "SLICE_X0Y1".into(),
            }],
        };
        let routed = Routed7 { placed, nets: vec![] };
        let bs = emit_fasm(&routed, &FakeEmit).expect("chained element must emit");
        let fasm = bs.fasm.join("\n");
        assert!(
            fasm.contains("PRECYINIT.CIN"),
            "a chained element takes its carry from the column route:\n{fasm}"
        );
        assert!(
            !fasm.contains("CARRY4.ACY0"),
            "a chained element is not the chain entry:\n{fasm}"
        );
        for arm in ["PRECYINIT.C0", "PRECYINIT.C1", "PRECYINIT.AX"] {
            assert!(!fasm.contains(arm), "PRECYINIT arms are exclusive, saw {arm}");
        }
    }
}
