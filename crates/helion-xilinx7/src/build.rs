//! Turn a routed 7-series design into a `.bit` file, in one call.
//!
//! This is the sequence the ZedBoard integration test performs by hand:
//!
//! 1. emit FASM from the routed design,
//! 2. load the per-tile-type segbits and ppips the FASM actually references,
//! 3. drop FASM lines for tile types upstream prjxray has no segbits for,
//! 4. assemble every frame listed in `part.json` (never the tilegrid union),
//! 5. serialise the FDRI payload with the reference's zero-pad rule.
//!
//! Step 4 is the one that matters most. The FDRI stream carries no per-frame
//! address, so a missing frame shifts every later frame and the bitstream
//! still reports `done=1`; the tilegrid union is 2194 frames short on xc7z020.
//! See `helion/AGENTS.md`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::bitstream::{self, BitstreamOptions};
use crate::device::Device;
use crate::emit::emit_fasm;
use crate::fasm::parse_fasm;
use crate::frames::{BlockType, SegBitsDb, TileBitLayout};
use crate::frames::{apply_clock_word_50, FrameStore};
use crate::tilegrid::TilegridLayout;
use crate::emit::EmitDevice;
use crate::xdevice::{build_node_map, XDevice};

/// A produced bitstream plus what went into it, so callers can report and
/// assert on the result rather than trusting a byte count.
#[derive(Clone, Debug)]
pub struct GeneratedBitstream {
    /// The `.bit` bytes.
    pub bytes: Vec<u8>,
    /// FASM lines emitted before the `*_SING` filter.
    pub fasm_lines: usize,
    /// FASM features actually written into frames.
    pub features_applied: usize,
    /// Frames in the FDRI payload (device frames + zero pads).
    pub frame_count: usize,
    /// Tile types whose FASM lines were dropped, with the reason.
    pub dropped: Vec<DroppedTile>,
    /// The part the bitstream targets.
    pub part: String,
    /// IDCODE written into the header.
    pub idcode: u32,
}

/// FASM lines skipped because the tile type has no usable segbits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedTile {
    pub tile: String,
    pub tile_type: String,
    pub lines: usize,
    pub reason: String,
}

impl GeneratedBitstream {
    /// A one-line human summary of what was dropped, or `None` if nothing was.
    pub fn dropped_summary(&self) -> Option<String> {
        if self.dropped.is_empty() {
            return None;
        }
        let tiles: Vec<&str> = self.dropped.iter().map(|d| d.tile.as_str()).collect();
        let lines: usize = self.dropped.iter().map(|d| d.lines).sum();
        Some(format!(
            "{lines} FASM line(s) on {} tile(s) were dropped. A *_SING tile hosts only \
             one half of its base type's two-site layout; instances whose tilegrid alias \
             lands in the upper half (bits 64..127) cannot be programmed from upstream \
             segbits. Affected tiles: {}",
            tiles.len(),
            tiles.join(", ")
        ))
    }
}

/// Everything that can go wrong producing a bitstream.
#[derive(Debug)]
pub enum BitstreamBuildError {
    /// The X-Ray device database could not be loaded.
    Device(String),
    /// Synthesis, packing, placement or routing failed.
    Flow(String),
    /// FASM could not be emitted or parsed.
    Fasm(String),
    /// Frames could not be assembled.
    Frames(String),
    /// The final `.bit` could not be serialised.
    Write(String),
}

impl std::fmt::Display for BitstreamBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device(m) => write!(f, "device: {m}"),
            Self::Flow(m) => write!(f, "flow: {m}"),
            Self::Fasm(m) => write!(f, "fasm: {m}"),
            Self::Frames(m) => write!(f, "frames: {m}"),
            Self::Write(m) => write!(f, "bitstream: {m}"),
        }
    }
}

impl std::error::Error for BitstreamBuildError {}

/// Pin constraints parsed from an XDC file.
///
/// Only `PACKAGE_PIN` is read: it is what placement needs, and it is the one
/// constraint that cannot be defaulted. Per-bit ports (`led[0]`) stay distinct
/// so a vector output can spread across pins.
#[derive(Clone, Debug, Default)]
pub struct PinConstraints {
    /// Port name (possibly bit-selected, e.g. `led[0]`) -> package pin.
    pub package_pins: std::collections::BTreeMap<String, String>,
    /// Port name -> I/O standard.
    pub io_standards: std::collections::BTreeMap<String, String>,
}

impl PinConstraints {
    /// Parse `set_property PACKAGE_PIN <pin> [get_ports {<port>}]` and
    /// `set_property IOSTANDARD <std> [get_ports {<port>}]` lines.
    ///
    /// Unrecognised lines are ignored: an XDC carries timing and placement
    /// directives far beyond pin assignment.
    pub fn parse(xdc: &str) -> Self {
        let mut out = Self::default();
        for line in xdc.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let (kind, rest) = if let Some(r) = line.strip_prefix("set_property PACKAGE_PIN") {
                ("pin", r)
            } else if let Some(r) = line.strip_prefix("set_property IOSTANDARD") {
                ("io", r)
            } else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            let value = match parts.next() {
                Some(v) => v.to_string(),
                None => continue,
            };
            let rest: String = parts.collect::<Vec<_>>().join(" ");
            // The port is the token(s) inside get_ports { ... }.
            let port = match (rest.find('{'), rest.find('}')) {
                (Some(a), Some(b)) if b > a => rest[a + 1..b].trim().to_string(),
                _ => continue,
            };
            if port.is_empty() {
                continue;
            }
            match kind {
                "pin" => {
                    out.package_pins.insert(port, value);
                }
                _ => {
                    out.io_standards.insert(port, value);
                }
            }
        }
        out
    }

    /// Whether any constraint was parsed.
    pub fn is_empty(&self) -> bool {
        self.package_pins.is_empty() && self.io_standards.is_empty()
    }

    /// Apply these constraints to a design's ports.
    ///
    /// A bit-qualified key (`led[0]`) is recorded on the vector port as the
    /// attribute `LOC[0]`, because a port is one entry in the design and
    /// keeping the bit index in the value is what lets packing tell the eight
    /// `led` IOBs apart. A plain key (`clk`) sets `LOC` directly.
    pub fn apply(&self, design: &mut helion_ir::Design) {
        for port in &mut design.ports {
            for (key, pin) in &self.package_pins {
                match bit_of_port(key, &port.name) {
                    Some(Some(bit)) => port.attrs.set(format!("LOC[{bit}]"), pin.clone()),
                    Some(None) => port.attrs.set("LOC", pin.clone()),
                    None => {}
                }
            }
            for (key, io) in &self.io_standards {
                match bit_of_port(key, &port.name) {
                    Some(Some(bit)) => port.attrs.set(format!("IOSTANDARD[{bit}]"), io.clone()),
                    Some(None) => port.attrs.set("IOSTANDARD", io.clone()),
                    None => {}
                }
            }
        }
    }
}

/// Pack/place/route a design for a Xilinx-7 part and report what it used.
///
/// `report_utilization` and `qor` load devices through the HL10T path, which
/// has no `xc7*` entries, so asking for either on a real part failed with
/// "read .../parts/xc7z020.toml: No such file or directory". This runs the same
/// flow as [`bitstream_for_part`] but stops before frames, which is enough for
/// a utilization or timing report and skips the expensive FASM/frame work.
pub struct X7Utilization {
    /// Occupied LUT sites (each is one 6-input LUT; seen as LUT+FF pairs).
    pub lut_sites: usize,
    /// Occupied flip-flops.
    ///
    /// Distinct from `lut_sites`: a slice holds one 6-input LUT *and* one FF
    /// per BEL, and a design may use either without the other. Reporting the
    /// LUT count for both is what made every 7-series utilization report print
    /// an identical `LUT=` and `FF=`.
    pub ffs: usize,
    /// Total flip-flop sites on the device (8 per slice: 4 LUTs + 4 FFs).
    pub ff_capacity: usize,
    /// Occupied IOB sites, split by direction.
    pub iobs_in: usize,
    /// Output IOBs.
    pub iobs_out: usize,
    /// Occupied block RAM sites.
    pub brams: usize,
    /// Occupied DSP sites.
    pub macs: usize,
    /// Total LUT6 sites on the device.
    pub lut_capacity: usize,
    /// Total IOB sites on the device.
    pub iob_capacity: usize,
    /// Total block RAM sites on the device.
    pub bram_capacity: usize,
    /// Total DSP sites on the device.
    pub dsp_capacity: usize,
    /// Resolved concrete package, e.g. `xc7z020clg484-1`.
    pub part: String,
    /// Worst negative slack at `period_ps`, from the 7-series timing model.
    ///
    /// This is a *model estimate*, not a sign-off number: `TimingModel::xilinx7`
    /// is calibrated to DS181 magnitudes but has not been correlated against a
    /// Vivado timing report. Treat it as an order-of-magnitude guide.
    pub wns_ps: i64,
    /// Timing endpoints (flip-flops) the report covered.
    pub endpoints: usize,
    /// The clock period the slack was measured against, in ps.
    pub period_ps: u64,
}

/// Pack, place, and route `design` for `part` and return resource counts.
pub fn x7_utilization(
    design: &helion_ir::Design,
    part: &str,
    db_root: Option<&Path>,
    period_ps: u64,
) -> Result<X7Utilization, BitstreamBuildError> {
    let root: PathBuf = match db_root {
        Some(p) => p.to_path_buf(),
        None => crate::device::default_db_root(),
    };
    let mut device =
        Device::load_part_from_db(&root, part).map_err(BitstreamBuildError::Device)?;
    let node_maps = build_node_map(&mut device);

    let packed = crate::pack::pack(design).map_err(BitstreamBuildError::Flow)?;
    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = crate::place::place(&packed, &xdev).map_err(BitstreamBuildError::Flow)?;
    let routed = crate::route::route(&placed, &xdev).map_err(BitstreamBuildError::Flow)?;

    // Timing needs the routed netlist, and only means something with a period,
    // so the caller supplies one. A design with no endpoints has no slack to
    // measure; report 0 rather than the period, so an unmeasured design cannot
    // look like a closed one.
    let report = crate::timing::report_timing_with_device(
        &routed,
        &helion_backend::TimingModel::xilinx7(),
        period_ps,
        Some(&xdev),
    )
    .map_err(BitstreamBuildError::Flow)?;

    let iobs_in = placed
        .packed
        .iobs
        .iter()
        .filter(|i| i.dir == crate::pack::IobDir::In)
        .count();
    Ok(X7Utilization {
        lut_sites: placed.packed.luts.len(),
        ffs: placed.packed.ffs.len(),
        ff_capacity: device.slices().len() * 8,
        iobs_in,
        iobs_out: placed.packed.iobs.len() - iobs_in,
        brams: placed.packed.brams.len(),
        macs: placed.packed.dsps.len(),
        lut_capacity: device.slices().len(),
        iob_capacity: device.iob_sites().len(),
        bram_capacity: device.ramb_sites().len(),
        dsp_capacity: device.dsp_sites().len(),
        part: device.part_dir().to_string(),
        wns_ps: report.wns_ps,
        endpoints: report.endpoints,
        period_ps,
    })
}

/// Build a `.bit` for `part` from an already-synthesised design.
///
/// `db_root` is the prjxray-db root (the directory holding `artix7/` and
/// `zynq7/`); pass `None` to use `HELION_XILINX_DB` or the built-in default.
///
/// Pin locations must already be on the design's ports (see
/// [`PinConstraints`]); placement refuses an IOB with no `LOC` rather than
/// guessing.
pub fn bitstream_for_part(
    design: &helion_ir::Design,
    part: &str,
    db_root: Option<&Path>,
) -> Result<GeneratedBitstream, BitstreamBuildError> {
    let root: PathBuf = match db_root {
        Some(p) => p.to_path_buf(),
        None => crate::device::default_db_root(),
    };
    let mut device =
        Device::load_part_from_db(&root, part).map_err(BitstreamBuildError::Device)?;
    let node_maps = build_node_map(&mut device);
    let layout = TilegridLayout::load(&tilegrid_path(&root, part))
        .map_err(BitstreamBuildError::Device)?;

    // --- flow: pack -> place -> route -> FASM -------------------------------
    let packed = crate::pack::pack(design).map_err(BitstreamBuildError::Flow)?;
    let mut xdev = XDevice::new(&device, node_maps);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = crate::place::place(&packed, &xdev).map_err(BitstreamBuildError::Flow)?;
    let routed = crate::route::route(&placed, &xdev).map_err(BitstreamBuildError::Flow)?;
    let bits = emit_fasm(&routed, &xdev).map_err(|e| BitstreamBuildError::Fasm(e.to_string()))?;

    // --- FASM -> frames -----------------------------------------------------
    let lines = parse_fasm(&bits.fasm.join("\n"))
        .map_err(|e| BitstreamBuildError::Fasm(format!("{e:?}")))?;
    let fasm_lines = lines.len();

    // Which tile types does this design actually use?
    let mut types: BTreeSet<String> = BTreeSet::new();
    for l in &lines {
        if let Some(t) = layout.tile_type(&l.tile) {
            types.insert(t.to_string());
        }
    }

    // X-Ray stores the per-tile-type DB files at the family level.
    let family = family_dir(&root, part);
    let mut segbits = SegBitsDb::new();
    let mut dropped: Vec<DroppedTile> = Vec::new();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for l in &lines {
        if let Some(t) = layout.tile_type(&l.tile) {
            *counts.entry(t.to_string()).or_default() += 1;
        }
    }
    for t in &types {
        // A `*_SING` tile is physically smaller with a different frame
        // layout, so the base type's bit positions address words that do not
        // exist in it ("word 102 outside 0..101"). X-Ray ships no segbits for
        // those types, and borrowing is invalid, so their lines are dropped
        // loudly rather than applied wrongly or ignored silently.
        let is_sing = t.contains("_SING") || t.contains("_TBYTESRC") || t.contains("_TBYTETERM");
        let lower = t.to_lowercase();
        let seg = family.join(format!("segbits_{lower}.db"));
        if is_sing && !seg.is_file() {
            // Types that ship their own segbits are never borrowed
            // (`_TBYTESRC`/`_TBYTETERM` do), so this is only about `_SING`.
            //
            // The drop is per INSTANCE, not per type. A `_SING` tile hosts a
            // half of its base type's two-site layout, and the tilegrid alias
            // says which half. `RIOB33_SING_X73Y49` maps its site to
            // `IOB33_Y1`, whose bits are the low 0..63 of the block's two
            // words, so they land inside the smaller tile and can be borrowed;
            // `RIOB33_SING_X73Y0` maps to `IOB33_Y0`, whose bits are 64..127
            // and do not fit. Deciding by type dropped both.
            let borrowable: Vec<&str> = lines
                .iter()
                .filter(|l| layout.tile_type(&l.tile) == Some(t.as_str()))
                .map(|l| l.tile.as_str())
                .filter(|tile| layout.tile_alias(tile).is_some())
                .collect();
            if borrowable.is_empty() {
                dropped.push(DroppedTile {
                    tile: String::new(),
                    tile_type: t.clone(),
                    lines: counts.get(t).copied().unwrap_or(0),
                    reason: format!(
                        "no segbits_{lower}.db upstream and no usable tilegrid alias: a \
                         *SING tile hosts only one half of its base type's layout"
                    ),
                });
                continue;
            }
            // Give the borrowable instances their base type's table, which the
            // per-instance site remap in `apply_fasm` will rewrite.
            let base_type: String = layout
                .tile_alias(borrowable[0])
                .map(|a| a.base_type.clone())
                .unwrap_or_default();
            let base_seg = family.join(format!("segbits_{}.db", base_type.to_lowercase()));
            if base_seg.is_file() {
                let text = std::fs::read_to_string(&base_seg)
                    .map_err(|e| BitstreamBuildError::Fasm(format!("{}: {e}", base_seg.display())))?;
                // `add_segbits` is idempotent for a type already loaded from
                // the base tile's own iteration.
                let _ = segbits.add_segbits(&base_type, BlockType::ClbIoClk, &text);
            }
            // Non-borrowable instances of this type still drop.
            let borrowed: BTreeSet<&str> = borrowable.into_iter().collect();
            for l in &lines {
                if layout.tile_type(&l.tile) == Some(t.as_str()) && !borrowed.contains(l.tile.as_str()) {
                    dropped.push(DroppedTile {
                        tile: l.tile.clone(),
                        tile_type: t.clone(),
                        lines: 1,
                        reason: format!("no segbits_{lower}.db upstream and no usable tilegrid alias"),
                    });
                }
            }
            continue;
        }
        if seg.is_file() {
            let text = std::fs::read_to_string(&seg)
                .map_err(|e| BitstreamBuildError::Fasm(format!("{}: {e}", seg.display())))?;
            segbits
                .add_segbits(t, BlockType::ClbIoClk, &text)
                .map_err(|e| BitstreamBuildError::Fasm(format!("segbits {t}: {e:?}")))?;
        }
        let pp = family.join(format!("ppips_{lower}.db"));
        if pp.is_file() {
            let text = std::fs::read_to_string(&pp)
                .map_err(|e| BitstreamBuildError::Fasm(format!("{}: {e}", pp.display())))?;
            segbits
                .add_ppips(t, &text)
                .map_err(|e| BitstreamBuildError::Fasm(format!("ppips {t}: {e:?}")))?;
        }
    }

    // A borrowed instance whose remapped bits still land outside the tile is
    // the case `RIOB33_SING_X73Y0` hits: its alias maps to `IOB33_Y0`, whose
    // bits are 64..127, but the smaller tile only hosts 0..63. Those lines are
    // dropped rather than written into the neighbouring half of the frame --
    // silently programming the wrong pad is worse than losing one.
    // Detect by looking for the type-level failure the segment write reported.
    // Expand the per-type drop record into per-tile, which is what a user
    // needs in order to know which pins are affected.
    let mut dropped_types: BTreeSet<String> = dropped.iter().map(|d| d.tile_type.clone()).collect();
    let mut dropped_tiles: Vec<DroppedTile> = Vec::new();
    if !dropped_types.is_empty() {
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        for l in &lines {
            if let Some(t) = layout.tile_type(&l.tile) {
                let t = t.to_string();
                if dropped_types.contains(&t) && seen.insert((l.tile.clone(), t.clone())) {
                    let reason = dropped
                        .iter()
                        .find(|d| d.tile_type == t)
                        .map(|d| d.reason.clone())
                        .unwrap_or_default();
                    dropped_tiles.push(DroppedTile {
                        tile: l.tile.clone(),
                        tile_type: t,
                        lines: 0,
                        reason,
                    });
                }
            }
        }
        for d in &mut dropped_tiles {
            d.lines = lines
                .iter()
                .filter(|l| l.tile == d.tile)
                .count();
        }
    }

    // Decide up front which instances of a borrowed type can actually host
    // their remapped bits, so the frame write never sees a line it must abort
    // on. A `_SING` tile is two words wide and the base type splits its two
    // sites across bits 0..63 and 64..127; only the low half fits.
    let mut borrow_ok: HashMap<String, bool> = HashMap::new();
    for l in &lines {
        borrow_ok.entry(l.tile.clone()).or_insert_with(|| {
            let Some(t) = layout.tile_type(&l.tile) else {
                return true;
            };
            if !t.contains("_SING") || !device.type_has_no_segbits(t) {
                return true;
            }
            let Some(alias) = layout.tile_alias(&l.tile) else {
                return false;
            };
            // Does every feature this tile emits resolve in the base type
            // once the site name is remapped, with its bits inside the tile?
            lines
                .iter()
                .filter(|o| o.tile == l.tile)
                .all(|o| borrowed_bits_fit(&segbits, alias, &o.feature, o.bit_range.map(|(lo, _)| lo)))
        });
    }
    let mut unfittable: BTreeSet<String> = BTreeSet::new();
    for (tile, ok) in &borrow_ok {
        if !ok {
            unfittable.insert(tile.clone());
        }
    }
    if !unfittable.is_empty() {
        for tile in &unfittable {
            let tile_type = layout.tile_type(tile).unwrap_or("?").to_string();
            let reason = format!(
                "borrowed layout does not fit this instance: its tilegrid alias maps the \
                 site onto the base type's upper word half (bits 64..127) while a *SING \
                 tile hosts only bits 0..63 (tile type {tile_type})"
            );
            dropped_tiles.push(DroppedTile {
                tile: tile.clone(),
                tile_type: tile_type.clone(),
                lines: lines.iter().filter(|l| &l.tile == tile).count(),
                reason,
            });
            dropped_types.insert(tile_type);
        }
    }

    let kept: Vec<_> = lines
        .into_iter()
        .filter(|l| {
            if unfittable.contains(&l.tile) {
                return false;
            }
            layout
                .tile_type(&l.tile)
                .map(|t| !dropped_types.contains(t))
                .unwrap_or(true)
        })
        .collect();

    // Every valid frame from part.json -- never the tilegrid union.
    let mut store = FrameStore::for_part(device.part())
        .map_err(|e| BitstreamBuildError::Frames(format!("{e:?}")))?;
    let features_applied = match store.apply_fasm(&kept, &segbits, &layout) {
        Ok(n) => n,
        Err(e) => {
            // The usual cause is a fabric bigger than the die it is programmed
            // onto, which is a property of the database rather than of this
            // design: `xc7a35t` ships no tilegrid and borrows `xc7a50t`, whose
            // fabric is taller than the 35T `part.json` describes. Say so
            // instead of leaving a bare "frame ... not in store".
            let msg = e.to_string();
            if msg.contains("not in store") {
                return Err(BitstreamBuildError::Frames(format!(
                    "{msg}. \n\
                     The fabric loaded for this part places tiles outside the die's \
                     configuration frames, so those features have no frame to be \
                     written into. This is a database limitation, not a design error: \
                     a part with no tilegrid of its own borrows another die's fabric, and \
                     the borrowed fabric can be larger than this die. Use a part that \
                     ships its own tilegrid, or accept that the affected features \
                     cannot be programmed on this die."
                )));
            }
            return Err(BitstreamBuildError::Frames(msg));
        }
    };
    let frame_count = store.frame_count();

    // Undescribed clock-network configuration, applied from a positional table
    // rather than from segbits -- see `CLOCK_WORD50_POSITIONAL`.
    apply_clock_word_50(&mut store);

    let payload = store.fdri_payload();

    // The header names the concrete part directory (`xc7z020clg484-1`), not the
    // bare die: a die name does not identify a package, and the header is what
    // a programmer shows. `device.part_dir()` is the package that was loaded.
    let header_part = device.part_dir().to_string();
    let opts = BitstreamOptions::xc7frames2bit(
        &header_part,
        &format!("helion/{header_part}.frm"),
        "2026/01/01",
        "00:00:00",
    );
    let idcode = opts.idcode;
    let bytes = bitstream::write_bit(&payload, &opts)
        .map_err(|e| BitstreamBuildError::Write(e.message.clone()))?;

    Ok(GeneratedBitstream {
        bytes,
        fasm_lines,
        features_applied,
        frame_count,
        dropped: dropped_tiles,
        part: header_part,
        idcode,
    })
}

/// Does `key` name `port`? `Some(Some(bit))` for `led[3]` against `led`,
/// `Some(None)` for an exact match, `None` if unrelated.
///
/// Both bit spellings are accepted. `led[3]` is what an XDC normally writes,
/// but `led_3` is what Helion's own `bit_name` emits for a vector bit's net, so
/// a constraint written in the flat form must resolve too. Accepting only the
/// bracket form silently DROPPED every `led_0..led_7` constraint: no `LOC` was
/// recorded, packing then treated the bits as unconstrained and dropped their
/// IOBs, and the build finished with exit 0 and a bitstream containing no
/// output pads at all. A silent wrong answer is worse than the hard error it
/// replaced, so both spellings resolve here.
fn bit_of_port(key: &str, port: &str) -> Option<Option<u32>> {
    if key == port {
        return Some(None);
    }
    if let Some(open) = key.rfind('[') {
        let close = key[open..].find(']')? + open;
        if &key[..open] == port {
            return key[open + 1..close].parse().ok().map(Some);
        }
    }
    // Flat form: `led_3` for port `led`. Requires the separator so a port named
    // `btn` does not swallow an unrelated `btnx`, and a digits-only suffix so
    // `led_foo` is not read as a bit index.
    if let Some(rest) = key.strip_prefix(port) {
        if let Some(bit) = rest.strip_prefix('_') {
            if !bit.is_empty() && bit.bytes().all(|c| c.is_ascii_digit()) {
                return bit.parse().ok().map(Some);
            }
        }
    }
    None
}

/// Path to the `tilegrid.json` that describes `part`'s fabric.
fn tilegrid_path(db_root: &Path, part: &str) -> PathBuf {
    let family = family_dir(db_root, part);
    let die = part.split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or(part);
    let mut i = 3;
    let b = die.as_bytes();
    while i < b.len() && !b[i].is_ascii_digit() {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b't' {
        i += 1;
    }
    let direct = family.join(&die[..i]).join("tilegrid.json");
    if direct.is_file() {
        return direct;
    }
    // Dies with no tilegrid of their own borrow a same-size sibling's.
    if let Ok(entries) = std::fs::read_dir(&family) {
        let mut cands: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("tilegrid.json"))
            .filter(|p| p.is_file())
            .collect();
        cands.sort();
        if let Some(p) = cands.into_iter().next() {
            return p;
        }
    }
    direct
}

/// The family directory (holding `segbits_*.db`) for a part name.
fn family_dir(db_root: &Path, part: &str) -> PathBuf {
    let lower = part.to_ascii_lowercase();
    let fam = if lower.starts_with("xc7z") {
        "zynq7"
    } else {
        "artix7"
    };
    let p = db_root.join(fam);
    if p.is_dir() {
        p
    } else {
        db_root.to_path_buf()
    }
}

#[cfg(test)]
mod round39_bit_of_port {
    use super::bit_of_port;
    #[test]
    fn flat_and_bracket_forms_both_resolve() {
        assert_eq!(bit_of_port("led_0", "led"), Some(Some(0)));
        assert_eq!(bit_of_port("led_7", "led"), Some(Some(7)));
        assert_eq!(bit_of_port("led[3]", "led"), Some(Some(3)));
        assert_eq!(bit_of_port("led", "led"), Some(None));
        assert_eq!(bit_of_port("clk", "led"), None);
        assert_eq!(bit_of_port("ledx_0", "led"), None);
        assert_eq!(bit_of_port("led_foo", "led"), None);
    }
}

/// Does every bit of `feature`, looked up in the alias's base type after the
/// site name is remapped, land inside a two-word hosting tile?
///
/// Nearly all `RIOB33` bits sit at positions 0..63 (`IOB_Y1`) or 64..127
/// (`IOB_Y0`) within the same two words, so the test is simply which half.
fn borrowed_bits_fit(
    segbits: &SegBitsDb,
    alias: &crate::frames::TileAlias,
    feature: &str,
    address: Option<u32>,
) -> bool {
    let Some((site, rest)) = feature.split_once('.') else {
        return false;
    };
    // Three different spellings of one site meet here, and none of them
    // equals another:
    //
    //   FASM / segbits      IOB_Y0
    //   tilegrid `sites`    IOB_X1Y49
    //   alias remap         IOB33_Y0 -> IOB33_Y1   (die-level names)
    //
    // What the alias VALUE selects is the segbits site letter, so
    // `IOB33_Y1` means the feature must be looked up as `IOB_Y1`. Matching on
    // the literal FASM site name finds nothing, which reads as "does not fit"
    // and drops a pad that would have worked.
    // The FASM site name is the tile's LOCAL name; the alias value is the
    // site the base type knows it as. Rewrite wholesale rather than comparing
    // them -- they are never spelled the same way.
    let local_prefix = site.rsplit_once('_').map(|(p, _)| p).unwrap_or("");
    let target_suffix = alias
        .sites
        .values()
        .next()
        .and_then(|mapped| mapped.rsplit('_').next())
        .unwrap_or("");
    let rewritten = if local_prefix.is_empty() || target_suffix.is_empty() {
        format!("{site}.{rest}")
    } else {
        format!("{local_prefix}_{target_suffix}.{rest}")
    };
    let Some((_, bits)) = segbits
        .feature_bits(&alias.base_type, &rewritten, address)
        .or_else(|| segbits.feature_bits(&alias.base_type, feature, address))
    else {
        return false;
    };
    // Where a feature's bits land, in words, is
    //
    //     block.offset + start_offset + bitpos / 32
    //
    // and a `_SING` variant owns only the two words starting at
    // `start_offset` within its row-pair. So the bit positions that fit are
    // `bitpos / 32 < 2`, measured from the block -- `IOB_Y1` sits at
    // positions 0..63 (words 0..1) and its instance adds no skew, while
    // `IOB_Y0` sits at 64..127 (words 2..3) and lands PAST the two words the
    // variant owns. For `RIOB33_SING_X73Y0` that is words 4..5 against a
    // window of 2..3.
    //
    // A previous version of this test compared bit positions directly
    // (`bitpos < 64`) after having also added `start_offset` to the frame
    // offset, which double-counted the skew. Stated in words from the block,
    // the rule is the simple one below.
    bits.iter().all(|b| b.bitpos / 32 < 2)
}