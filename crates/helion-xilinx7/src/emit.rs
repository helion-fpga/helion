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
use std::collections::HashMap;

/// What the emitter needs from the device model.
pub trait EmitDevice {
    /// Tile type for a tile name, e.g. `"CLBLL_L_X14Y100"` → `"CLBLL_L"`.
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
    /// Static clock-network features implied by routed pips. Defaults to
    /// none (unit-test fakes).
    fn clock_statics(&self, _pips: &[&crate::route::RoutedPip]) -> Vec<String> {
        Vec::new()
    }
    /// STEPDOWN lines for the banks of these IOB tiles (banks running a
    /// <=1.8V standard). Defaults to none.
    fn bank_stepdown(&self, _iob_tiles: &[String]) -> Vec<String> {
        Vec::new()
    }
    fn ioi_tile(&self, _iob_tile: &str) -> Option<String> {
        None
    }
    /// Static always-on housekeeping features for this part, from the
    /// DB's `required_features.fasm`. Defaults to none; the XDevice
    /// impl reads the part directory at load.
    fn required_features(&self) -> Vec<String> {
        Vec::new()
    }
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

    // Features every bitstream for this part must contain, straight from
    // the DB's `required_features.fasm`. These static always-on bits are
    // referenced by nothing in the design, so they cannot be derived from
    // routing: emit them first and unconditionally.
    lines.extend(device.required_features());

    // LUTs: INIT vector per occupied ALUT..DLUT.
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

    // FFs: ZINI + D-mux select + control signals (CE, SR).
    //
    // Control signal configuration (per X-Ray DB segbits_clbll_l.db):
    // - {bel}.ZRST: SR pin acts as RESET (vs SET). Always emitted:
    //   with an SR net it configures reset polarity, without one it
    //   records the tied-off reset (the oracle has it on every FF).
    //   prjxray's Z prefix names the zero-state option (ZRST =
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
    // Slices whose SR acts asynchronously (FDCE/FDPE): no FFSYNC.
    let mut async_slices: HashSet<(String, String)> = HashSet::new();
    for ff in &placed.ffs {
        let site_rel = device
            .site_rel(&ff.tile, &ff.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", ff.tile, ff.site))?;
        let bel = ff_bel(&ff.bel)?; // "AFF" -> "AFF"
        // D source (shared rule with the router): same-letter LUT O6 or
        // carry sum inside the slice, else the routed bypass pin.
        let letter = ff.bel.chars().next().unwrap_or('A');
        let mux_src = match crate::place::ff_d_source(ff, placed) {
            crate::place::FfDSource::LutO6 => "O6".to_string(),
            crate::place::FfDSource::CarryXor => "XOR".to_string(),
            crate::place::FfDSource::Bypass => format!("{letter}X"),
        };
        let pff = packed.ffs.iter().find(|p| p.name == ff.name);
        // ZINI: power-on value 0 (absent: INIT=1).
        if !pff.map(|p| p.init).unwrap_or(false) {
            lines.push(format!("{}.{}.{}.ZINI", ff.tile, site_rel, bel));
        }
        lines.push(format!("{}.{}.{}MUX.{}", ff.tile, site_rel, bel, mux_src));
        // Control signals: look up the packed FF for CE/SR nets.
        if let Some(p) = packed.ffs.iter().find(|p| p.name == ff.name) {
            if p.ce_net.is_some() {
                ce_slices.insert((ff.tile.clone(), site_rel.clone()));
            }
            if p.sr_net.is_some() {
                sr_slices.insert((ff.tile.clone(), site_rel.clone()));
            }
            // ZRST: SR loads 0 (reset); absent, SR loads 1 (set). With
            // no SR net Vivado ties the reset pin low and still programs
            // ZRST: the ZB-10 oracle carries ZRST on every chain FF.
            if !p.srval {
                lines.push(format!("{}.{}.{}.ZRST", ff.tile, site_rel, bel));
            }
            if p.sr_async && p.sr_net.is_some() {
                async_slices.insert((ff.tile.clone(), site_rel.clone()));
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

    // FF clock arrival (ZB-10): every placed FF slice gets FFSYNC
    // (without it the FFs cannot clock), and its serving INT tile
    // gets the arrival set. Every used slice also gets NOCLKINV
    // (clock not inverted): the oracle programs it on all chain
    // slices, so the zero-initialized frame does not match Vivado.
    // The serving tile is the same-X INT tile
    // on the slice's side (proven for the X71 column; anything else
    // fails loudly at the existence check). Clock wire by slice
    // letter: SLICE_X0 takes CLK1, SLICE_X1 takes CLK0 (ZB-10 oracle
    // mapping; any other slice name fails loudly). BYP_ALT lines are
    // skipped on tiles where the router already drives BYP_ALT (the
    // head tile's input corridor): emitting the uniform GFAN
    // selection there would double-drive the mux.
    let mut ff_slices: HashSet<(String, String)> = HashSet::new();
    for ff in &placed.ffs {
        let site_rel = device
            .site_rel(&ff.tile, &ff.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", ff.tile, ff.site))?;
        ff_slices.insert((ff.tile.clone(), site_rel));
    }
    // Every slice with placed content (LUT, FF, or carry), in
    // deterministic order. Slices are identified by device rel site.
    let mut used_slices: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    for lut in &placed.luts {
        let site_rel = device
            .site_rel(&lut.tile, &lut.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", lut.tile, lut.site))?;
        used_slices.insert((lut.tile.clone(), site_rel));
    }
    for ff in &placed.ffs {
        let site_rel = device
            .site_rel(&ff.tile, &ff.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", ff.tile, ff.site))?;
        used_slices.insert((ff.tile.clone(), site_rel));
    }
    for c in &placed.carries {
        let site_rel = device
            .site_rel(&c.tile, &c.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", c.tile, c.site))?;
        used_slices.insert((c.tile.clone(), site_rel));
    }
    // Slices whose flops clock on the falling edge take the slice clock
    // inverter (the placer keeps both edges out of one slice).
    let mut negedge_slices: HashSet<(String, String)> = HashSet::new();
    for ff in &placed.ffs {
        if packed.negedge_ffs.contains(&ff.name) {
            if let Some(site_rel) = device.site_rel(&ff.tile, &ff.site) {
                negedge_slices.insert((ff.tile.clone(), site_rel));
            }
        }
    }
    for (tile, slice) in &used_slices {
        if negedge_slices.contains(&(tile.clone(), slice.clone())) {
            lines.push(format!("{tile}.{slice}.CLKINV"));
        } else {
            lines.push(format!("{tile}.{slice}.NOCLKINV"));
        }
    }
    // SLICEM address-input defaults (ZB-10): a used SLICEM slice runs
    // its A/B/C LUTs in logic mode with the DI1 address mux parked
    // on the cascade input, even for unused letters (the oracle shows
    // all three on slices using only ALUT). SLICEL has no DI muxes
    // and the D LUT has no default in the oracle.
    const DI1_DEFAULTS: [(&str, &str); 3] = [
        ("ALUT", "BDI1_BMC31"),
        ("BLUT", "DI_CMC31"),
        ("CLUT", "DI_DMC31"),
    ];
    for (tile, slice) in &used_slices {
        if !slice.starts_with("SLICEM") {
            continue;
        }
        for (bel, src) in DI1_DEFAULTS {
            lines.push(format!("{tile}.{slice}.{bel}.DI1MUX.{src}"));
        }
    }
    // Serving INT tile per FF slice, with the CLK letters it needs.
    let mut int_arrival: std::collections::BTreeMap<String, (bool, bool)> = std::collections::BTreeMap::new();
    for (tile, slice) in &ff_slices {
        if !async_slices.contains(&(tile.clone(), slice.clone())) {
            lines.push(format!("{tile}.{slice}.FFSYNC"));
        }
        let mut parts = tile.split('_');
        let (_family, side, xypart) = match (parts.next(), parts.next(), parts.next()) {
            (Some(fam), Some(side), Some(xy)) if parts.next().is_none() => (fam, side, xy),
            _ => return Err(format!("emit: cannot parse slice tile `{tile}`")),
        };
        let xy = xypart.strip_prefix('X').ok_or_else(|| format!("emit: cannot parse slice tile `{tile}`"))?;
        let (x, y) = xy.split_once('Y').ok_or_else(|| format!("emit: cannot parse slice tile `{tile}`"))?;
        let serving = format!("INT_{side}_X{x}Y{y}");
        device.tile_type(&serving).ok_or_else(|| format!("emit: no serving INT tile `{serving}` for `{tile}`"))?;
        let clk1 = if slice.ends_with("_X0") {
            true
        } else if slice.ends_with("_X1") {
            false
        } else {
            return Err(format!("emit: unknown slice `{slice}` for clock mapping"));
        };
        let entry = int_arrival.entry(serving).or_insert((false, false));
        if clk1 {
            entry.1 = true;
        } else {
            entry.0 = true;
        }
    }
    // Tiles where the router already drives BYP_ALT (input corridor).
    let mut byp_owned: HashSet<String> = HashSet::new();
    for net in &routed.nets {
        if let RouteKind::Pips(pips) = &net.kind {
            for p in pips {
                if p.dst_wire.contains("BYP_ALT") || p.src_wire.contains("BYP_ALT") {
                    byp_owned.insert(p.tile.clone());
                }
            }
        }
    }
    const BYP_FROM_GFAN: [(u8, u8); 8] = [(0, 0), (1, 0), (2, 1), (3, 1), (4, 0), (5, 0), (6, 1), (7, 1)];
    // Bypass inputs that must read 1, never ground. A PRECYINIT.C0 carry
    // head (`cyinit == Some(false)`) computes bit 0 as S0 = !Q0 with its
    // carry generate DI0 taken from the AX bypass pin, which must read 1
    // (an unprogrammed BYP_ALT reads VCC by default, X-Ray ppip
    // `INT_*.BYP_ALT*.VCC_WIRE default`). Grounding it through GFAN froze
    // the counter at bit 0: the extracted netlist toggled only bit 0, and
    // removing exactly this one tie made it match the RTL for 2^25 cycles.
    // AX reaches the INT tile on BYP_ALT1 for slice X0 and BYP_ALT0 for
    // slice X1, identically on CLBLL_L/R and CLBLM_L/R (tileconn + ppips).
    let mut keep_high: HashSet<(String, u8)> = HashSet::new();
    for carry in &placed.carries {
        let Some(pc) = packed.carries.iter().find(|p| p.name == carry.name) else {
            continue;
        };
        if pc.cyinit != Some(false) {
            continue;
        }
        let site_rel = device
            .site_rel(&carry.tile, &carry.site)
            .ok_or_else(|| format!("emit: no rel site for {} {}", carry.tile, carry.site))?;
        let ax_alt = if site_rel.ends_with("_X0") {
            1
        } else if site_rel.ends_with("_X1") {
            0
        } else {
            return Err(format!("emit: unknown slice `{site_rel}` for carry head AX"));
        };
        let mut parts = carry.tile.split('_');
        if let (Some(_), Some(side), Some(xy), None) = (parts.next(), parts.next(), parts.next(), parts.next()) {
            keep_high.insert((format!("INT_{side}_{xy}"), ax_alt));
        }
    }
    for (int, (need_clk0, need_clk1)) in &int_arrival {
        lines.push(format!("{int}.GFAN0.GND_WIRE"));
        lines.push(format!("{int}.GFAN1.GND_WIRE"));
        // Fixed-recipe clock arrival; graph-routed clocks carry their own
        // INT clock-input pips.
        if *need_clk0 && !routed.graph_clocks {
            lines.push(format!("{int}.CLK0.GCLK_B11"));
        }
        if *need_clk1 && !routed.graph_clocks {
            lines.push(format!("{int}.CLK1.GCLK_B11"));
        }
        if !byp_owned.contains(int) {
            for (alt, gfan) in BYP_FROM_GFAN {
                if keep_high.contains(&(int.clone(), alt)) {
                    continue;
                }
                lines.push(format!("{int}.BYP_ALT{alt}.GFAN{gfan}"));
            }
        }
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

    // Banks running a low-voltage standard: STEPDOWN on every IOB of the
    // bank plus the bank's HCLK_IOI3 (Vivado ground truth, fa4b golden).
    let low_v: Vec<String> = placed
        .iobs
        .iter()
        .filter(|i| {
            packed.iobs.iter().find(|p| p.name == i.name).map(|p| {
                let s = p.iostandard.to_ascii_uppercase();
                matches!(s.as_str(), "LVCMOS18" | "LVCMOS15" | "LVCMOS12" | "SSTL135" | "SSTL15" | "HSTL_I" | "HSTL_II" | "HSTL_I_18" | "HSTL_II_18")
            }).unwrap_or(false)
        })
        .map(|i| i.tile.clone())
        .collect();
    if !low_v.is_empty() {
        lines.extend(device.bank_stepdown(&low_v));
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
    // keep ZINV_S1/ZINV_CE1 is overruled, and there is no fabric
    // drive at all: the ZB-10 oracle programs zero tie-offs
    // device-wide (IGNORE1 masks S1/CE1, HIGH pins match IMUX
    // defaults), so polarity bits only select inversion.
    // BUFGCTRL inputs the router drives (tile, index 0-15, I0/I1):
    // parked lines must never double-drive these muxes.
    let mut bufg_used: HashSet<(String, u8, u8)> = HashSet::new();
    for net in &routed.nets {
        if let RouteKind::Pips(pips) = &net.kind {
            for p in pips {
                if let Some(rest) = p.dst_wire.strip_prefix("CLK_BUFG_BUFGCTRL") {
                    let mut parts = rest.split('_');
                    if let (Some(ns), Some(inp), None) = (parts.next(), parts.next(), parts.next()) {
                        let input = match inp {
                            "I0" => 0,
                            "I1" => 1,
                            _ => continue,
                        };
                        if let Ok(n) = ns.parse::<u8>() {
                            if n < 16 {
                                bufg_used.insert((p.tile.clone(), n, input));
                            }
                        }
                    }
                }
            }
        }
    }
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
                // No inversion on the driven I0-path pins: values pass through.
                lines.push(format!("{}.BUFGCTRL.{}.ZINV_CE0", bufg.tile, site_rel));
                lines.push(format!("{}.BUFGCTRL.{}.ZINV_S0", bufg.tile, site_rel));
                // IGNORE1 inverted, exactly as Vivado emits it.
                lines.push(format!("{}.BUFGCTRL.{}.IS_IGNORE1_INVERTED", bufg.tile, site_rel));
                // Rebuffer enables for this BUFG's global line (bare features).
                if !routed.graph_clocks {
                    for (tile, feat) in device.bufg_spine_static(&bufg.tile, &bufg.site) {
                        lines.push(format!("{tile}.{feat}"));
                    }
                }
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
    // Park every unused BUFGCTRL I0/I1 input on its fixed IMUX
    // (ZB-10 oracle: BUFGCTRLn takes IMUX{28+n%4}_{n/4} on both
    // inputs; only the live input keeps the router's source).
    // One tile holds 16 BUFGCTRLs; deduplicate shared tiles.
    let mut bufg_tiles: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for bufg in &placed.bufgs {
        let kind = packed.bufgs.iter().find(|b| b.name == bufg.name).map(|b| b.kind).unwrap_or(ClockBufKind::Bufg);
        if !matches!(kind, ClockBufKind::Bufio | ClockBufKind::Bufr) {
            bufg_tiles.insert(bufg.tile.as_str());
        }
    }
    for tile in bufg_tiles {
        for n in 0..16u8 {
            for input in [0u8, 1u8] {
                if bufg_used.contains(&(tile.to_string(), n, input)) {
                    continue;
                }
                let m = 28 + (n % 4);
                let s = n / 4;
                lines.push(format!(
                    "{tile}.CLK_BUFG_BUFGCTRL{n}_I{input}.CLK_BUFG_IMUX{m}_{s}"
                ));
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

    // CARRY4s (ZB-04). Feature paths: TILE.SLICEL_<X>.CARRY4.<bit> and
    // TILE.SLICEL_<X>.PRECYINIT.<src>. Derived from the blinky golden
    // chain (GT-blinky.bit decodes): a chained group takes PRECYINIT.CIN
    // (carry in from the slice below through the dedicated cascade),
    // except a group chained directly on the head, which takes
    // PRECYINIT.AX (golden Y39X1 transit); a CYINIT-config head takes
    // PRECYINIT.C0 for CYINIT=0 or C1 for CYINIT=1. C0 is const-0
    // (the oracle synth netlist ties head CYINIT to const0); mapping a
    // CYINIT=1 intent to C0 freezes bit 0 on silicon. CARRY4.ACY0
    // accompanies AX exactly (head-AX and transit-AX show it; C0, C1
    // and CIN never do).
    let co_nets: HashSet<&str> = packed
        .carries
        .iter()
        .filter_map(|c| c.co_net.as_deref())
        .collect();
    // Chain heads (CI not chained): a group chained directly on a head
    // selects the head-driven cascade variant.
    let head_names: HashSet<&str> = packed
        .carries
        .iter()
        .filter(|c| !co_nets.contains(c.ci_net.as_str()))
        .map(|c| c.name.as_str())
        .collect();
    let co_driver: HashMap<&str, &str> = packed
        .carries
        .iter()
        .filter_map(|c| c.co_net.as_deref().map(|co| (co, c.name.as_str())))
        .collect();
    // Chains built from directly instantiated CARRY4 primitives: the head
    // wires its CYINIT pin.
    let prim_style: HashSet<&str> = packed
        .carries
        .iter()
        .filter(|c| {
            let mut cur = *c;
            let mut hops = 0;
            while let Some(d) = co_driver.get(cur.ci_net.as_str()).and_then(|n| packed.carries.iter().find(|x| x.name == *n)) {
                cur = d;
                hops += 1;
                if hops > packed.carries.len() {
                    break;
                }
            }
            cur.cyinit_net.is_some()
        })
        .map(|c| c.name.as_str())
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
        if prim_style.contains(carry.name.as_str()) {
            // A directly instantiated CARRY4 chain: carry in from the cascade,
            // or from the constant or fabric net on the head's CYINIT pin;
            // each wired DI selects the bypass generate.
            if co_nets.contains(packed_c.ci_net.as_str()) {
                lines.push(format!("{}.{}.PRECYINIT.CIN", carry.tile, site_rel));
            } else {
                let src = match packed_c.cyinit_net.as_deref() {
                    Some("1" | "vcc" | "VCC" | "$true") => "C1",
                    Some("0" | "gnd" | "GND" | "$false") | None => "C0",
                    Some(_) => "AX",
                };
                lines.push(format!("{}.{}.PRECYINIT.{}", carry.tile, site_rel, src));
            }
            for (i, letter) in ['A', 'B', 'C', 'D'].iter().enumerate() {
                if packed_c.di_nets[i].is_some() {
                    lines.push(format!("{}.{}.CARRY4.{}CY0", carry.tile, site_rel, letter));
                }
            }
            continue;
        }
        if packed.adder_carries.contains(&carry.name) {
            // General adder: carry in from config (head) or the cascade,
            // generate from O5 where the bit has one, sum out on XOR.
            let src = match packed_c.cyinit {
                Some(true) => "C1",
                Some(false) => "C0",
                None => "CIN",
            };
            lines.push(format!("{}.{}.PRECYINIT.{}", carry.tile, site_rel, src));
            for (j, letter) in ['A', 'B', 'C', 'D'].into_iter().enumerate() {
                if packed_c.di_nets[j].is_some() && packed_c.s_nets[j].is_some() {
                    lines.push(format!("{}.{}.CARRY4.{letter}CY0", carry.tile, site_rel));
                }
                if packed_c.o_nets[j].is_some() {
                    lines.push(format!("{}.{}.{letter}OUTMUX.XOR", carry.tile, site_rel));
                }
            }
            continue;
        }
        // ACY0 accompanies AX exactly (golden: head-AX and transit-AX
        // carry it; C0 and CIN never do).
        let mut use_ax = false;
        if let Some(high) = packed_c.cyinit {
            // CYINIT from config: the value selects the source. C0 is
            // const-0, C1 is const-1; crossing them freezes silicon.
            let src = if high { "C1" } else { "C0" };
            lines.push(format!("{}.{}.PRECYINIT.{}", carry.tile, site_rel, src));
        } else if co_nets.contains(packed_c.ci_net.as_str()) {
            let head_driven = co_driver
                .get(packed_c.ci_net.as_str())
                .is_some_and(|d| head_names.contains(*d));
            if head_driven {
                lines.push(format!("{}.{}.PRECYINIT.AX", carry.tile, site_rel));
                use_ax = true;
            } else {
                lines.push(format!("{}.{}.PRECYINIT.CIN", carry.tile, site_rel));
            }
        } else {
            // Legacy const-LUT head: CYINIT source is the CI net driver.
            let driver_init = packed
                .luts
                .iter()
                .find(|l| l.output_net == packed_c.ci_net)
                .map(|l| l.init);
            match driver_init {
                Some(u64::MAX) => {
                    lines.push(format!("{}.{}.PRECYINIT.AX", carry.tile, site_rel));
                    use_ax = true;
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
        }
        if use_ax {
            lines.push(format!("{}.{}.CARRY4.ACY0", carry.tile, site_rel));
        }
    }

    // Clock-network enables implied by the routed pips (rebuffers, HROW
    // taps, BUFHCE, HCLK leaf buffers, CMT markers).
    {
        let all: Vec<&crate::route::RoutedPip> = routed
            .nets
            .iter()
            .filter_map(|n| match &n.kind {
                RouteKind::Pips(p) => Some(p.iter()),
                _ => None,
            })
            .flatten()
            .collect();
        lines.extend(device.clock_statics(&all));
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
            if tile.starts_with("CLBLM") {
                Some("SLICEM_X0".into())
            } else if tile.starts_with("CLB") {
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
                sr_net: None, init: false, srval: false, sr_async: false,
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
            nets: vec![RoutedNet7 { net: "q".into(), kind: RouteKind::SiteInternal }],
            ..Default::default()
        }
    }

    #[test]
    fn emit_blinky_fasm() {
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.ALUT.INIT[63:0] = 64'h5555555555555555"), "got:\n{text}");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZINI"), "got:\n{text}");
        // Paired inverter LUT drives the FF inside the slice (O6), not
        // through the unrouted AX bypass.
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFFMUX.O6"), "got:\n{text}");
        // Vivado default drive for LVCMOS33 outputs is 12mA (ZB-10).
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.LVCMOS33_LVTTL.DRIVE.I12_I16"), "got:\n{text}");
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.PULLTYPE.NONE"), "got:\n{text}");
        assert!(text.contains("RIOB33_X0Y0.IOB_Y1.IN_TERM.NONE"), "got:\n{text}");
    }

    #[test]
    fn emit_ff_clock_arrival() {
        // Every placed FF slice gets FFSYNC (its FFs cannot clock
        // without it), and its serving INT tile gets the clock arrival
        // set: grounded GFANs, GCLK_B11 into the slice's CLK wire
        // (X0 takes CLK1, X1 takes CLK0, the ZB-10 oracle mapping),
        // and the uniform BYP_ALT from GFAN selection.
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.FFSYNC"), "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.GFAN0.GND_WIRE"), "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.GFAN1.GND_WIRE"), "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.CLK1.GCLK_B11"), "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.BYP_ALT0.GFAN0"), "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.BYP_ALT7.GFAN1"), "got:\n{text}");
    }

    #[test]
    fn emit_ff_without_sr_net_gets_zrst() {
        // ZB-10 oracle: every chain FF has ZRST (reset, tied off) even
        // with no SR net. The blinky fixture FF has sr_net None.
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZRST"), "got:\n{text}");
    }

    #[test]
    fn emit_used_slice_gets_noclockinv() {
        // ZB-10 oracle: every used slice programs NOCLKINV (the
        // zero-initialized frame does not match Vivado here).
        let bs = emit_fasm(&routed_blinky(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.NOCLKINV"), "got:\n{text}");
    }

    /// A slice of falling-edge flops takes the clock inverter instead.
    #[test]
    fn emit_falling_edge_slice_gets_clkinv() {
        let mut r = routed_blinky();
        let ff = r.placed.packed.ffs[0].name.clone();
        r.placed.packed.negedge_ffs.insert(ff);
        let bs = emit_fasm(&r, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.CLKINV"), "got:\n{text}");
        assert!(!text.contains("CLBLL_L_X0Y0.SLICEL_X0.NOCLKINV"), "got:\n{text}");
    }

    #[test]
    fn emit_slicem_slice_gets_di1mux_defaults() {
        // ZB-10 oracle: every used SLICEM slice programs the DI1MUX
        // address defaults on A/B/C (even with no B/C LUTs); the D
        // LUT and SLICEL slices never carry them.
        let mut r = routed_blinky();
        r.placed.packed.luts[0].output_net = "dm".into();
        r.placed.luts[0].tile = "CLBLM_R_X0Y0".into();
        r.placed.luts[0].site = "SLICE_X0Y0".into();
        r.placed.packed.ffs[0].d_net = "dm".into();
        r.placed.ffs[0].tile = "CLBLM_R_X0Y0".into();
        r.placed.ffs[0].site = "SLICE_X0Y0".into();
        let bs = emit_fasm(&r, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(text.contains("CLBLM_R_X0Y0.SLICEM_X0.ALUT.DI1MUX.BDI1_BMC31"), "got:\n{text}");
        assert!(text.contains("CLBLM_R_X0Y0.SLICEM_X0.BLUT.DI1MUX.DI_CMC31"), "got:\n{text}");
        assert!(text.contains("CLBLM_R_X0Y0.SLICEM_X0.CLUT.DI1MUX.DI_DMC31"), "got:\n{text}");
        assert!(!text.contains("DLUT.DI1MUX"), "got:\n{text}");
        // The L-type blinky slice must stay clean.
        let base = emit_fasm(&routed_blinky(), &FakeEmit).unwrap().fasm.join("\n");
        assert!(!base.contains("DI1MUX"), "got:\n{base}");
    }

    #[test]
    fn emit_arrival_skips_router_owned_byp() {
        // The head tile's BYP_ALT0 carries the clock input corridor
        // (router-owned): arrival must not double-drive it, but GFAN,
        // CLK, and FFSYNC still go out.
        use crate::route::RoutedPip;
        let mut routed = routed_blinky();
        routed.nets.push(RoutedNet7 {
            net: "clk_in".into(),
            kind: RouteKind::Pips(vec![RoutedPip {
                tile: "INT_L_X0Y0".into(),
                dst_wire: "BYP_ALT0".into(),
                src_wire: "NR1END0".into(),
                is_pseudo: false,
            }]),
        });
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        // The only BYP_ALT line is the router's own pip, never a
        // GFAN selection from arrival.
        assert_eq!(text.matches("BYP_ALT").count(), 1, "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.GFAN0.GND_WIRE"), "got:\n{text}");
        assert!(text.contains("INT_L_X0Y0.CLK1.GCLK_B11"), "got:\n{text}");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.FFSYNC"), "got:\n{text}");
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

    /// Golden shapes: TILE.SLICEL_X*.PRECYINIT.{AX,CIN} with ACY0
    /// accompanying AX exactly (head-driven cascade variant), and no
    /// BCY0/CCY0/DCY0 anywhere in the chain.
    fn routed_carry_chain() -> Routed7 {
        let mut r = routed_blinky();
        r.placed.packed.luts.push(PackedLut {
            name: "u_one".into(), init: u64::MAX,
            input_nets: [None, None, None, None, None, None],
            output_net: "one".into(),
        });
        r.placed.packed.carries = vec![
            PackedCarry {
                name: "u_c0".into(), ci_net: "one".into(), cyinit: None, co_net: Some("co0".into()),
                s_nets: [Some("s0".into()), None, None, None],
                di_nets: [None, None, None, None],
                o_nets: [Some("o0".into()), None, None, None],
                ..Default::default()
            },
            PackedCarry {
                name: "u_c1".into(), ci_net: "co0".into(), cyinit: None, co_net: Some("co1".into()),
                s_nets: [Some("s1".into()), None, None, None],
                di_nets: [None, None, None, None],
                o_nets: [Some("o1".into()), None, None, None],
                ..Default::default()
            },
            PackedCarry {
                name: "u_c2".into(), ci_net: "co1".into(), cyinit: None, co_net: None,
                s_nets: [Some("s2".into()), None, None, None],
                di_nets: [None, None, None, None],
                o_nets: [Some("o2".into()), None, None, None],
                ..Default::default()
            },
        ];
        r.placed.carries = vec![
            PlacedCarry { name: "u_c0".into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into() },
            PlacedCarry { name: "u_c1".into(), tile: "CLBLL_L_X0Y1".into(), site: "SLICE_X0Y1".into() },
            PlacedCarry { name: "u_c2".into(), tile: "CLBLL_L_X0Y2".into(), site: "SLICE_X0Y2".into() },
        ];
        // One chain FF per group to pin the FFMUX.XOR rule.
        r.placed.packed.ffs.push(PackedFf {
            name: "u_ff0".into(), d_net: "o0".into(), q_net: "q0".into(),
            clk_net: "clk_bufg".into(), ce_net: None, sr_net: None, init: false, srval: false, sr_async: false,
        });
        r.placed.ffs.push(PlacedFf {
            name: "u_ff0".into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into(), bel: "AFF".into(),
        });
        r
    }

    #[test]
    fn emit_carry_chain_precyinit_matches_golden() {
        let bs = emit_fasm(&routed_carry_chain(), &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        // Legacy const-1 head: AX plus ACY0.
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.PRECYINIT.AX"), "got:\n{text}");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.CARRY4.ACY0"), "got:\n{text}");
        // Group chained directly on the head (transit variant): AX plus ACY0.
        assert!(text.contains("CLBLL_L_X0Y1.SLICEL_X1.PRECYINIT.AX"), "got:\n{text}");
        assert!(text.contains("CLBLL_L_X0Y1.SLICEL_X1.CARRY4.ACY0"), "got:\n{text}");
        // Group chained on a mid group: CIN with no ACY0.
        assert!(text.contains("CLBLL_L_X0Y2.SLICEL_X1.PRECYINIT.CIN"), "got:\n{text}");
        assert_eq!(text.matches("CARRY4.ACY0").count(), 2, "got:\n{text}");
        assert!(!text.contains("BCY0") && !text.contains("CCY0") && !text.contains("DCY0"), "got:\n{text}");
        // Chain FF samples the sum through FFMUX.XOR, never a letter input.
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFFMUX.XOR"), "got:\n{text}");
    }

    /// Required part features (from `required_features.fasm`) emit first
    /// and unconditionally: nothing in the design references them.
    #[test]
    fn emit_required_features_come_first() {
        struct ReqEmit;
        impl EmitDevice for ReqEmit {
            fn tile_type(&self, _tile: &str) -> Option<String> {
                Some("CLBLL_L".into())
            }
            fn site_rel(&self, _tile: &str, _site: &str) -> Option<String> {
                Some("SLICEL_X0".into())
            }
            fn required_features(&self) -> Vec<String> {
                vec!["CFG_CENTER_MID_X123Y84.ALWAYS_ON_PROP1".into()]
            }
        }
        let r = routed_blinky();
        let text = emit_fasm(&r, &ReqEmit).unwrap().fasm.join("\n");
        let first = text.lines().next().unwrap_or("");
        assert_eq!(first, "CFG_CENTER_MID_X123Y84.ALWAYS_ON_PROP1");
    }

    /// CYINIT value selects the PRECYINIT source: 0 to C0, 1 to C1.
    /// C0 is const-0 (oracle synth netlist ties head CYINIT to const0),
    /// so a CYINIT=1 intent mapped to C0 would freeze bit 0 on silicon.
    #[test]
    fn emit_carry_config_head_selects_c0_or_c1_by_value() {
        use crate::pack::PackedCarry;
        use crate::place::PlacedCarry;
        for (high, want) in [(false, "PRECYINIT.C0"), (true, "PRECYINIT.C1")] {
            let mut r = routed_blinky();
            r.placed.packed.carries = vec![PackedCarry {
                name: "u_h".into(), ci_net: String::new(), cyinit: Some(high),
                co_net: Some("coh".into()),
                s_nets: [Some("s0".into()), None, None, None],
                di_nets: [None, None, None, None],
                o_nets: [Some("o0".into()), None, None, None],
                ..Default::default()
            }];
            r.placed.carries = vec![PlacedCarry {
                name: "u_h".into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into(),
            }];
            let text = emit_fasm(&r, &FakeEmit).unwrap().fasm.join("\n");
            assert!(text.contains(&format!("CLBLL_L_X0Y0.SLICEL_X0.{want}")), "got:\n{text}");
            let other = if high { "PRECYINIT.C0" } else { "PRECYINIT.C1" };
            assert!(!text.contains(other), "crossed CYINIT source, got:\n{text}");
        }
    }

    /// A PRECYINIT.C0 head computes bit 0 from DI0 = AX, which must read 1.
    /// The FF-slice arrival program grounds the serving tile's BYP_ALT
    /// muxes through GFAN; it must leave the head's AX bypass alone
    /// (BYP_ALT1 for slice X0), or bit 0 never carries and the counter
    /// freezes. Every other BYP_ALT stays grounded (carry DI = 0).
    #[test]
    fn emit_c0_carry_head_keeps_ax_bypass_high() {
        use crate::pack::PackedCarry;
        use crate::place::PlacedCarry;
        let mut r = routed_blinky();
        r.placed.packed.carries = vec![PackedCarry {
            name: "u_h".into(), ci_net: String::new(), cyinit: Some(false),
            co_net: Some("coh".into()),
            s_nets: [Some("s0".into()), None, None, None],
            di_nets: [None, None, None, None],
            o_nets: [Some("o0".into()), None, None, None],
            ..Default::default()
        }];
        r.placed.carries = vec![PlacedCarry {
            name: "u_h".into(), tile: "CLBLL_L_X0Y0".into(), site: "SLICE_X0Y0".into(),
        }];
        let text = emit_fasm(&r, &FakeEmit).unwrap().fasm.join("\n");
        assert!(text.contains("CLBLL_L_X0Y0.SLICEL_X0.PRECYINIT.C0"), "got:\n{text}");
        assert!(!text.contains("INT_L_X0Y0.BYP_ALT1."), "head AX bypass grounded, got:\n{text}");
        for alt in [0, 2, 3, 4, 5, 6, 7] {
            assert!(text.contains(&format!("INT_L_X0Y0.BYP_ALT{alt}.GFAN")), "BYP_ALT{alt} lost its tie, got:\n{text}");
        }
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
            ..Default::default()
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
    fn emit_bufg_unused_inputs_parked() {
        // ZB-10 oracle: every unused BUFGCTRL I0/I1 input is parked on
        // a fixed IMUX (BUFGCTRLn -> IMUX{28+n%4}_{n/4}); only the
        // router-driven input keeps its live source.
        use crate::pack::PackedBufg;
        use crate::place::PlacedBufg;
        use crate::route::RoutedPip;
        let mut routed = routed_blinky();
        routed.placed.packed.bufgs.push(PackedBufg {
            name: "bufg_clk".into(),
            in_net: "clk".into(),
            out_net: "clk_bufg".into(),
            ..Default::default()
        });
        routed.placed.bufgs.push(PlacedBufg {
            name: "bufg_clk".into(),
            tile: "CLK_BUFG_BOT_R_X0Y0".into(),
            site: "BUFGCTRL_X0Y0".into(),
        });
        routed.nets.push(RoutedNet7 {
            net: "clk".into(),
            kind: RouteKind::Pips(vec![RoutedPip {
                tile: "CLK_BUFG_BOT_R_X0Y0".into(),
                dst_wire: "CLK_BUFG_BUFGCTRL0_I0".into(),
                src_wire: "CLK_BUFG_BOT_R_CK_MUXED0".into(),
                is_pseudo: false,
            }]),
        });
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(
            text.contains("CLK_BUFG_BOT_R_X0Y0.CLK_BUFG_BUFGCTRL0_I1.CLK_BUFG_IMUX28_0"),
            "missing I1 park, got:\n{text}"
        );
        assert!(
            text.contains("CLK_BUFG_BOT_R_X0Y0.CLK_BUFG_BUFGCTRL15_I1.CLK_BUFG_IMUX31_3"),
            "missing last park, got:\n{text}"
        );
        assert!(
            text.contains("CLK_BUFG_BOT_R_X0Y0.CLK_BUFG_BUFGCTRL0_I0.CLK_BUFG_BOT_R_CK_MUXED0"),
            "used input lost live source, got:\n{text}"
        );
        let parked = text.matches("CLK_BUFG_IMUX2").count()
            + text.matches("CLK_BUFG_IMUX3").count();
        assert_eq!(parked, 31, "want 32 inputs minus 1 used, got:\n{text}");
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
        // FF without CE/SR must NOT emit the mux enables (the oracle
        // has neither), but MUST emit ZRST: the ZB-10 oracle carries
        // ZRST on every chain FF, which have no SR nets (tied-off
        // reset). Minimalism is not the goal; oracle identity is.
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
        assert!(
            text.contains("CLBLL_L_X0Y0.SLICEL_X0.AFF.ZRST"),
            "missing tied-off ZRST, got:\n{text}"
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
            ..Default::default()
        };
        let bs = emit_fasm(&routed, &FakeEmit).unwrap();
        let text = bs.fasm.join("\n");
        assert!(
            text.contains("BRAM_L_X0Y0.RAMB18.RAMB18_X0Y0.IN_USE"),
            "got:\n{text}"
        );
    }
}
