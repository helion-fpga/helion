//! helion-bitgen: UltraScale+ bitstream emitter executable.
//!
//! The executable the diff harness invokes (harness/emitter.py protocol).
//! Thin CLI only: it loads the vendored segbits DBs and a device, calls
//! the `bitgen` library, and prints results. It invents no encodings.
//!
//! Subcommands:
//! ```text
//! helion-bitgen emit --fasm FILE --part PART --out FILE.bit [--data-dir DIR]
//! helion-bitgen parse --bit FILE.bit --part PART [--data-dir DIR]
//! helion-bitgen bits --bit FILE.bit --part PART [--data-dir DIR]
//! ```
//! - `emit` writes a raw bitstream for the FASM design, exit 0 on success.
//! - `parse` prints the FASM the bitstream encodes, on stdout, exit 0.
//! - `bits` prints the emitted bitset, one line per set bit:
//!   `FAR_HEX OFFSET VALUE BLOCK`, where FAR_HEX is the 32-bit frame
//!   address in hex, OFFSET is the bit offset within the 93-word frame,
//!   VALUE is 0 or 1 (always 1 here: set bits only), and BLOCK is the
//!   block-registry name the bit belongs to. Block labels are what let
//!   the harness compare per block: without them, one block's set bits
//!   would count as diffs in another block. If `bits` is ever wrong,
//!   the harness falls back to parse-mediated derivation; `bits` failing
//!   loud is better than `bits` mislabeling.
//!
//! Data paths: `--data-dir` overrides the vendored data directory; the
//! default is the crate's `data/` at compile time (`CARGO_MANIFEST_DIR`).
//! The harness invokes this binary from its own working directory, so a
//! relative path that only works from the repo root would be a bug.
//!
//! Device seam (`load_device`): the emitter consumes the `BitgenDevice`
//! trait. `xcku3p` loads the real `DeviceFile` from the vendored
//! `data/xcku3p.json` (critic session 13 FLAG F4). One provisional
//! bridge remains: the real impl does not resolve HPIO tiles (no column
//! mapping in the device file), but the v1 goldens carry HPIO lines, so
//! `HPIO_RIGHT_X0Y0` still resolves to the stub's provisional base FAR
//! 0x000 (critic session 13 FLAG F1). The harness blocks IO_CONFIG
//! before comparing, so no greenness depends on that value, and it must
//! never be consumed as ground truth. The bridge dies the day the
//! xcku3p tilegrid lands. Unknown parts fail loud here.
//!
//! FASM vocabulary gap, closed here (not in the library): the golden
//! FASM uses `A6LUT.INIT[63:0]` and valued `AFF.INIT = 0`, while the
//! prjuray DBs carry per-bit `ALUT.INIT[i]` lines and valueless
//! `AFF.INIT.V0` / `AFF.INIT.V1`. Two translations close the gap:
//! - `FileSegbitsDb::synthesize_lut_init` (library) regroups the 64
//!   per-bit lines into one MSB-first `X6LUT.INIT[63:0]` entry per LUT.
//!   The regrouping adds no new encoding: the harness cross-checks the
//!   same per-bit entries against BitFiltrator LutLoc on every run.
//! - `translate_ff_valued_fasm` (below) rewrites valued FF INIT/SRVAL
//!   lines to the canonical `.V0`/`.V1` spelling, using the resolved
//!   polarity (INIT value 0 selects `.V0`, which sets the bit;
//!   bit = NOT(value); researcher session 1, critic-verified). This is
//!   the same mapping the harness applies in `canonical_roundtrip_fasm`
//!   for comparison, applied one step earlier so the emitter can look
//!   the feature up. Bare `AFF.INIT` (no value) is not translated: no
//!   value selects V0 or V1, and the library fails loud on it as an
//!   unknown feature, exactly as the harness rejects it.
//!
//! No em dashes anywhere.

use helion_ultrascale::bitgen::{
    BitgenDevice, BitgenError, emit_bitstream, parse_bitstream, parse_fasm, parse_frames,
};
use helion_ultrascale::device_file::DeviceFile;
use helion_ultrascale::segbits::{FileSegbitsDb, SegbitsDb};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: helion-bitgen <emit|parse|bits> --part PART [--fasm FILE --out FILE | --bit FILE] [--data-dir DIR]";

/// Vendored segbits DB files, in load order. The CLB DBs carry the LUT
/// and FF encodings; the rest carry INT PIP, IO, clock, and HDIO.
const SEGBITS_FILES: [&str; 9] = [
    "segbits_int.db",
    "segbits_hpio_right.db",
    "segbits_hdio_top_right.db",
    "segbits_hdio_bot_right.db",
    "segbits_cmt_right.db",
    "segbits_clel_l.db",
    "segbits_clel_r.db",
    "segbits_clem.db",
    "segbits_clem_r.db",
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("helion-bitgen: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, opts) = parse_args(args)?;
    let data_dir = opts
        .data_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data"));
    match cmd.as_str() {
        "emit" => {
            let out_path = opts.out.ok_or("--emit needs --out FILE")?;
            cmd_emit(opts.fasm.as_deref(), &opts.part, &out_path, &data_dir)
        }
        "parse" => {
            let bit_path = opts.bit.ok_or("--parse needs --bit FILE")?;
            let text = cmd_parse(&bit_path, &opts.part, &data_dir)?;
            print!("{text}");
            Ok(())
        }
        "bits" => {
            let bit_path = opts.bit.ok_or("--bits needs --bit FILE")?;
            let text = cmd_bits(&bit_path, &opts.part, &data_dir)?;
            print!("{text}");
            Ok(())
        }
        other => Err(format!("unknown subcommand {other:?}\n{USAGE}")),
    }
}

struct Opts {
    fasm: Option<String>,
    bit: Option<String>,
    part: String,
    out: Option<String>,
    data_dir: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<(String, Opts), String> {
    if args.is_empty() {
        return Err(format!("no subcommand given\n{USAGE}"));
    }
    let cmd = args[0].clone();
    if cmd != "emit" && cmd != "parse" && cmd != "bits" {
        return Err(format!("unknown subcommand {cmd:?}\n{USAGE}"));
    }
    let mut opts = Opts {
        fasm: None,
        bit: None,
        part: String::new(),
        out: None,
        data_dir: None,
    };
    let mut i = 1;
    while i < args.len() {
        let (flag, inline_val) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (args[i].clone(), None),
        };
        let take = |i: &mut usize, inline: Option<String>| -> Result<String, String> {
            if let Some(v) = inline {
                return Ok(v);
            }
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| format!("flag {flag} needs a value\n{USAGE}"))
        };
        match flag.as_str() {
            "--fasm" => opts.fasm = Some(take(&mut i, inline_val)?),
            "--bit" => opts.bit = Some(take(&mut i, inline_val)?),
            "--part" => opts.part = take(&mut i, inline_val)?,
            "--out" => opts.out = Some(take(&mut i, inline_val)?),
            "--data-dir" => opts.data_dir = Some(PathBuf::from(take(&mut i, inline_val)?)),
            other => return Err(format!("unknown flag {other:?}\n{USAGE}")),
        }
        i += 1;
    }
    if opts.part.is_empty() {
        return Err(format!("--part PART is required\n{USAGE}"));
    }
    Ok((cmd, opts))
}

/// Load and merge the vendored segbits DBs, then synthesize the
/// single-feature LUT INIT lists. Fails loud on any missing or
/// malformed file: a silently partial DB would emit guessed bits.
fn load_segbits(data_dir: &Path) -> Result<FileSegbitsDb, String> {
    let paths: Vec<PathBuf> = SEGBITS_FILES
        .iter()
        .map(|name| data_dir.join(name))
        .collect();
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut db = FileSegbitsDb::from_files(&refs).map_err(|e| {
        format!(
            "cannot load vendored segbits DBs from {}: {e}",
            data_dir.display()
        )
    })?;
    db.synthesize_lut_init()
        .map_err(|e| format!("cannot synthesize LUT INIT entries: {e}"))?;
    Ok(db)
}

/// Device for a part. The real `DeviceFile` implementation of
/// `BitgenDevice` (interfaces session 2 item 4 FLAG, merged at
/// 1ad06df) is the device: SLICE base FARs from the file's own CLB
/// column majors, INT base FARs by the provisional rule (CLB column
/// major - 1, validated against the file's minors table), RAMB18
/// from the BRAM content columns, and the full 41,460-frame config
/// order, so `emit_bitstream` writes every real frame (untouched
/// frames as zeros) instead of the stub's touched-frames-only stream.
///
/// One provisional bridge stays (critic session 13 FLAG F1): the
/// real impl returns `None` for HPIO tiles, but the v1 goldens carry
/// `HPIO_RIGHT_X0Y0` lines, so that one tile still resolves to the
/// stub's provisional base FAR 0x000 with tile type `HPIO_RIGHT`.
/// The value is admitted invented (no public HPIO column major
/// exists); the harness blocks IO_CONFIG before comparing, so no
/// block's greenness depends on it. The bridge is exactly one tile,
/// not a table: every other tile the file does not resolve still
/// fails loud. It dies when the xcku3p tilegrid lands.
///
/// Only `xcku3p` is wired. `xcvu3p.json` exists on disk but is
/// provisional (computed frame total 73,035 vs UG570 Table 1's
/// 71,820), so it stays unwired until that gap is resolved by a
/// CTO decision; any other part fails loud here.
struct HpioProvisionalFallback {
    inner: DeviceFile,
    /// The real impl's tile list plus the one bridged HPIO tile:
    /// `parse_bitstream` only visits tiles in this list, so omitting
    /// the HPIO tile would leave its emitted bits unexplained and
    /// parse would fail loud on them.
    names: Vec<String>,
}

impl BitgenDevice for HpioProvisionalFallback {
    fn part(&self) -> &str {
        <DeviceFile as BitgenDevice>::part(&self.inner)
    }

    fn idcode(&self) -> Option<u32> {
        <DeviceFile as BitgenDevice>::idcode(&self.inner)
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        if tile == "HPIO_RIGHT_X0Y0" {
            return Some("HPIO_RIGHT");
        }
        <DeviceFile as BitgenDevice>::tile_type(&self.inner, tile)
    }

    fn tile_base_far(&self, tile: &str) -> Option<u32> {
        if tile == "HPIO_RIGHT_X0Y0" {
            return Some(0x000);
        }
        <DeviceFile as BitgenDevice>::tile_base_far(&self.inner, tile)
    }

    fn tile_names(&self) -> &[String] {
        &self.names
    }

    fn config_order_fars(&self) -> &[u32] {
        <DeviceFile as BitgenDevice>::config_order_fars(&self.inner)
    }
}

fn load_device(part: &str, data_dir: &Path) -> Result<Box<dyn BitgenDevice>, String> {
    match part {
        "xcku3p" => {
            let file = DeviceFile::from_path(&data_dir.join("xcku3p.json")).map_err(|e| {
                format!(
                    "cannot load device file {}: {e}",
                    data_dir.join("xcku3p.json").display()
                )
            })?;
            let mut names: Vec<String> = <DeviceFile as BitgenDevice>::tile_names(&file).to_vec();
            names.push("HPIO_RIGHT_X0Y0".to_string());
            names.sort();
            names.dedup();
            Ok(Box::new(HpioProvisionalFallback { inner: file, names }))
        }
        other => Err(format!(
            "part {other} has no emitter device yet: only xcku3p is wired; \
            xcvu3p.json is provisional until its frame-total gap is resolved"
        )),
    }
}

fn cmd_emit(
    fasm_path: Option<&str>,
    part: &str,
    out_path: &str,
    data_dir: &Path,
) -> Result<(), String> {
    // FASM comes from --fasm FILE; with no --fasm it comes from stdin
    // (the harness always passes --fasm, stdin is a convenience for
    // shell pipelines, and reading it only when no file is given
    // cannot change the harness path).
    let text = match fasm_path {
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("cannot read FASM {path}: {e}"))?
        }
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| format!("cannot read FASM from stdin: {e}"))?;
            buf
        }
    };
    let text = translate_ff_valued_fasm(&text)?;
    let device = load_device(part, data_dir)?;
    let segbits = load_segbits(data_dir)?;
    let bytes = emit_bitstream(&text, device.as_ref(), &segbits)
        .map_err(|e: BitgenError| format!("emit failed: {e}"))?;
    std::fs::write(out_path, bytes)
        .map_err(|e| format!("cannot write bitstream {out_path}: {e}"))?;
    Ok(())
}

fn cmd_parse(bit_path: &str, part: &str, data_dir: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(bit_path).map_err(|e| format!("cannot read bitstream {bit_path}: {e}"))?;
    let device = load_device(part, data_dir)?;
    let segbits = load_segbits(data_dir)?;
    parse_bitstream(&bytes, device.as_ref(), &segbits)
        .map_err(|e: BitgenError| format!("parse failed: {e}"))
}

/// Emit the bitset of a bitstream as `FAR_HEX OFFSET VALUE BLOCK` lines,
/// one per set bit, sorted by (FAR, offset).
///
/// The bitset is derived from the bitstream bytes alone (no FASM input):
/// `parse_bitstream` renders the present features, each feature is
/// re-resolved to its set bits, and each bit is labeled with its block.
/// Every set bit in the frames must appear in the re-resolved set, and
/// every re-resolved set bit must read 1 in the frames; either mismatch
/// fails loud instead of printing a partial bitset. `parse_bitstream`
/// already fails loud on unexplained set bits, so the cross-check here
/// guards the re-resolution itself, not the emitter.
fn cmd_bits(bit_path: &str, part: &str, data_dir: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(bit_path).map_err(|e| format!("cannot read bitstream {bit_path}: {e}"))?;
    let device = load_device(part, data_dir)?;
    let segbits = load_segbits(data_dir)?;
    bits_for_bytes(&bytes, device.as_ref(), &segbits)
}

/// Bitset of already-read bitstream bytes. Split out of `cmd_bits` so
/// tests can call it in-process without files.
fn bits_for_bytes(
    bytes: &[u8],
    device: &dyn BitgenDevice,
    segbits: &dyn SegbitsDb,
) -> Result<String, String> {
    let frames =
        parse_frames(bytes).map_err(|e: BitgenError| format!("cannot parse frames: {e}"))?;
    let fasm = parse_bitstream(bytes, device, segbits)
        .map_err(|e: BitgenError| format!("parse failed: {e}"))?;
    let entries =
        parse_fasm(&fasm).map_err(|e: BitgenError| format!("parse-back is not valid FASM: {e}"))?;
    let mut bits: BTreeMap<(u32, u16), &'static str> = BTreeMap::new();
    for entry in &entries {
        let tile_type = device
            .tile_type(&entry.tile)
            .ok_or_else(|| format!("parse-back tile {} is unknown to the device", entry.tile))?;
        let base_far = device
            .tile_base_far(&entry.tile)
            .ok_or_else(|| format!("parse-back tile {} has no base FAR", entry.tile))?;
        let refs = segbits.lookup(tile_type, &entry.feature).ok_or_else(|| {
            format!(
                "parse-back feature {}.{} is not in the segbits DB",
                tile_type, entry.feature
            )
        })?;
        let n_set = refs.iter().filter(|r| !r.must_be_zero).count();
        let mut set_idx = 0usize;
        // Collect the wanted set-bit refs first: a feature whose bits
        // are all clear (a complement line such as AFF.INIT.V1 or
        // CARRY8.DI0.AX, whose refs are all `!`) contributes no set
        // bits and needs no block label. Classification is lazy so an
        // unclassifiable complement line cannot fail `bits` for bits
        // it does not set. A feature that does set bits and matches
        // no block still fails loud below.
        let mut wanted: Vec<&helion_ultrascale::segbits::BitRef> = Vec::new();
        for r in refs.iter() {
            if r.must_be_zero {
                continue;
            }
            // MSB-first expansion over the set bits only, same rule as
            // `emit_frames`: listed set bit j takes value bit (n-1-j).
            let want = match &entry.value {
                Some(v) => v.bit(n_set - 1 - set_idx),
                None => true,
            };
            set_idx += 1;
            if want {
                wanted.push(r);
            }
        }
        if wanted.is_empty() {
            continue;
        }
        let block = classify_feature(&entry.tile, &entry.feature).ok_or_else(|| {
            format!(
                "parse-back feature {}.{} matches no block in the registry",
                entry.tile, entry.feature
            )
        })?;
        for r in wanted {
            let far = base_far.checked_add(r.frame).ok_or_else(|| {
                format!(
                    "tile {} feature {}: base FAR overflow",
                    entry.tile, entry.feature
                )
            })?;
            bits.entry((far, r.bit)).or_insert(block);
        }
    }
    // Cross-check against the frames: no set bit may be unattributed,
    // and no attributed bit may read 0.
    let mut frame_set: HashSet<(u32, u16)> = HashSet::new();
    for (far, words) in &frames {
        for (wi, word) in words.iter().enumerate() {
            let mut w = *word;
            while w != 0 {
                let b = w.trailing_zeros() as usize;
                frame_set.insert((*far, (wi * 32 + b) as u16));
                w &= w - 1;
            }
        }
    }
    for pos in frame_set.iter() {
        if !bits.contains_key(pos) {
            return Err(format!(
                "bit {1} of FAR 0x{0:08x} is set in the bitstream but no parse-back feature explains it",
                pos.0, pos.1
            ));
        }
    }
    for (far, bit) in bits.keys() {
        if !frame_set.contains(&(*far, *bit)) {
            return Err(format!(
                "re-resolved bit {bit} of FAR 0x{far:08x} reads 0 in the bitstream"
            ));
        }
    }
    let mut out = String::new();
    for ((far, bit), block) in &bits {
        out.push_str(&format!("{far:08x} {bit} 1 {block}\n"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// FASM vocabulary translation (valued FF INIT/SRVAL to .V0/.V1)
// ---------------------------------------------------------------------------

/// Rewrite valued FF INIT/SRVAL lines to the canonical DB spelling.
///
/// Input line `SLICE_X0Y0.AFF.INIT = 0` becomes `SLICE_X0Y0.AFF.INIT.V0`;
/// value 1 becomes `.V1`. Polarity, resolved by researcher session 1:
/// the V suffix IS the attribute value (V0 = INIT 0, which sets the bit;
/// V1 = INIT 1, which clears it). SRVAL is the same. Every other line
/// passes through byte-identical, including comments and blank lines.
///
/// A valued `.V0`/`.V1` line (`AFF.INIT.V0 = 0`) fails loud: the V suffix
/// already is the value, so a value on it is contradictory input, and
/// the harness canonicalization rejects it the same way. A bare
/// `AFF.INIT` (no value) is not rewritten and fails later in the
/// library as an unknown feature: no value selects V0 or V1.
fn translate_ff_valued_fasm(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let (code, comment) = match raw_line.find('#') {
            Some(p) => (&raw_line[..p], Some(&raw_line[p..])),
            None => (raw_line, None),
        };
        let trimmed = code.trim_end();
        if trimmed.trim().is_empty() {
            out.push_str(raw_line);
            out.push('\n');
            continue;
        }
        if let Some(rewritten) = rewrite_ff_line(trimmed, line_no)? {
            out.push_str(&rewritten);
            if let Some(c) = comment {
                out.push(' ');
                out.push_str(c);
            }
            out.push('\n');
        } else {
            out.push_str(raw_line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Rewrite one comment-stripped line when it is a valued FF INIT/SRVAL
/// line, or a valued `.V0`/`.V1` line (which fails loud). Returns
/// `Ok(None)` for lines that are neither.
fn rewrite_ff_line(code: &str, line_no: usize) -> Result<Option<String>, String> {
    let (lhs, _) = match code.split_once('=') {
        Some((l, r)) => (l.trim(), r.trim()),
        None => return Ok(None),
    };
    let parts: Vec<&str> = lhs.split('.').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Ok(None);
    }
    let (tile, bel, attr) = (parts[0], parts[1], parts[2]);
    if tile.is_empty() || !is_ff_bel(bel) || (attr != "INIT" && attr != "SRVAL") {
        return Ok(None);
    }
    if parts.len() == 4 {
        // `AFF.INIT.V0 = 0`: the V suffix already is the value.
        if parts[3] == "V0" || parts[3] == "V1" {
            return Err(format!(
                "FASM line {line_no}: {lhs} takes no value (the .V suffix already is the value)"
            ));
        }
        return Ok(None);
    }
    // Valued `AFF.INIT = <value>`: parse the value with the library's
    // own FASM parser, so the accepted spellings match emit exactly.
    let entries = parse_fasm(code).map_err(|e: BitgenError| format!("FASM line {line_no}: {e}"))?;
    let value = entries
        .first()
        .and_then(|e| e.value.as_ref())
        .ok_or_else(|| format!("FASM line {line_no}: FF attribute {lhs} has no value"))?;
    if value.significant_bits() > 1 {
        return Err(format!(
            "FASM line {line_no}: FF attribute {lhs} value is not 0 or 1"
        ));
    }
    let n = u8::from(value.bit(0));
    Ok(Some(format!("{tile}.{bel}.{attr}.V{n}")))
}

/// True for FF BEL names: a letter A-H plus `FF` or `FF2`.
fn is_ff_bel(bel: &str) -> bool {
    let b = bel.as_bytes();
    b.len() >= 3
        && b.len() <= 4
        && (b'A'..=b'H').contains(&b[0])
        && (&bel[1..] == "FF" || &bel[1..] == "FF2")
}

// ---------------------------------------------------------------------------
// Block classification (mirrors harness/fasm.py `classify`)
// ---------------------------------------------------------------------------

/// Map a parse-back feature to its block in the registry, mirroring
/// `harness/fasm.py::classify` (same patterns, same order). Returns None
/// for a feature that matches no known block pattern; the caller fails
/// loud. The `.V0`/`.V1` FF spellings are accepted here (parse-back
/// renders them) even though the harness `classify` regexes match only
/// the bare `AFF.INIT` spelling: with `bits` implemented the harness
/// never classifies parse-back lines itself, and the fallback path
/// would raise on them. That harness-side gap is flagged in COORD.md
/// for the diff-harness worker, not fixed here.
fn classify_feature(tile: &str, feature: &str) -> Option<&'static str> {
    if is_lut_init(feature) || is_lut_init_bit(feature) {
        return Some("LUT_INIT");
    }
    if is_ff_init(feature) {
        return Some("FF_INIT");
    }
    // Any other feature on an FF BEL (ABCDFF/EFGHFF control attributes
    // CEUSED/CLKINV/MODE/SRINV/SRUSED/SYNC, FF2 pairs): the harness
    // registry names `.ABCDFF.`, `.ABCFF.`, and `.CEUSED.V0`/`.V1`
    // explicitly; the BEL rule covers those and the EFGHFF group and
    // the remaining control attributes the registry does not list.
    // A BEL is one or more A-H letters plus FF or FF2.
    let bel = feature.split('.').next().unwrap_or("");
    let bel_body = bel.strip_suffix("FF2").or_else(|| bel.strip_suffix("FF"));
    if let Some(letters) = bel_body {
        if !letters.is_empty() && letters.bytes().all(|b| (b'A'..=b'H').contains(&b)) {
            return Some("FF_INIT");
        }
    }
    if (feature.contains("INITP") || feature.contains("INIT_P"))
        && (feature.contains("BRAM") || feature.contains("RAMB") || tile.contains("RAMB"))
    {
        return Some("BRAM_PARITY");
    }
    if feature.contains("INIT")
        && (feature.contains("BRAM") || feature.contains("RAMB") || tile.contains("RAMB"))
    {
        return Some("BRAM_CONTENT");
    }
    if tile.contains("DSP48E2") || feature.contains("DSP48E2") {
        return Some("DSP48E2");
    }
    if tile.contains("URAM") || feature.contains("URAM") {
        return Some("URAM288");
    }
    if feature.contains("BUFGCE")
        || feature.contains("BUFGCTRL")
        || feature.contains("MMCM")
        || feature.contains("PLL")
    {
        return Some("CLK");
    }
    if tile.starts_with("CMT_") && (feature.starts_with("PIP.") || feature.contains(".PIP.")) {
        return Some("CLK");
    }
    if feature.starts_with("PIP.") || feature.contains(".PIP.") {
        return Some("INT_PIP");
    }
    if feature.contains("IOB") || tile.starts_with("HPIO") || tile.starts_with("HDIO") {
        return Some("IO_CONFIG");
    }
    None
}

/// `^([A-H])6LUT\.INIT\[63:0\]$` (harness `_LUT_INIT_RE`).
fn is_lut_init(feature: &str) -> bool {
    let b = feature.as_bytes();
    feature.len() == 16 && (b'A'..=b'H').contains(&b[0]) && &feature[1..] == "6LUT.INIT[63:0]"
}

/// Per-bit LUT INIT as the parse-back renders it: `^([A-H])LUT\.INIT\[\d+\]$`.
/// The DB carries per-bit `ALUT.INIT[i]` lines; the grouped `[63:0]`
/// form is `is_lut_init` above. Same block, same bits.
fn is_lut_init_bit(feature: &str) -> bool {
    let mut parts = feature.split('.');
    let bel = match parts.next() {
        Some(b) => b,
        None => return false,
    };
    let attr = match parts.next() {
        Some(a) => a,
        None => return false,
    };
    if parts.next().is_some() {
        return false;
    }
    let b = bel.as_bytes();
    if bel.len() != 4 || !(b'A'..=b'H').contains(&b[0]) || &bel[1..] != "LUT" {
        return false;
    }
    let inner = match attr.strip_prefix("INIT[").and_then(|s| s.strip_suffix(']')) {
        Some(i) => i,
        None => return false,
    };
    !inner.is_empty() && inner.bytes().all(|c| c.is_ascii_digit())
}

/// `^([A-H])FF2?\.(INIT|SRVAL)(\.V[01])?$` (harness `_FF_INIT_RE` /
/// `_FF_SRVAL_RE`, plus the `.V0`/`.V1` spelling parse-back renders).
fn is_ff_init(feature: &str) -> bool {
    let base = feature
        .strip_suffix(".V0")
        .or_else(|| feature.strip_suffix(".V1"))
        .unwrap_or(feature);
    let mut parts = base.split('.');
    let bel = match parts.next() {
        Some(b) => b,
        None => return false,
    };
    let attr = match parts.next() {
        Some(a) => a,
        None => return false,
    };
    if parts.next().is_some() {
        return false;
    }
    is_ff_bel(bel) && (attr == "INIT" || attr == "SRVAL")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ultrascale::bitgen::{emit_frames, far_encode};
    use std::collections::BTreeSet;

    fn data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
    }

    #[test]
    fn real_device_fars_match_device_file() {
        // The emitter's device base FARs come from the vendored xcku3p
        // device file itself. Recompute them here from the file and
        // assert the loaded device agrees, so a device-file change
        // fails loudly here instead of silently shifting every
        // emitted bit.
        use helion_ultrascale::device_file::DeviceFile;
        let dev = DeviceFile::from_path(&data_dir().join("xcku3p.json")).unwrap();
        let clb_col = dev.clb_col_major("SLR0", 0, 0).unwrap();
        assert_eq!(clb_col, 5, "CLB column 0 row 0 column major");
        let tile_type = dev.clb_tile_type("SLR0", 0, 0).unwrap();
        assert_eq!(tile_type, "CLEL_R");
        let slice_base = far_encode(0, 0, clb_col as u16, 0).unwrap();
        assert_eq!(slice_base, 0x500);
        // INT column rule (reference.py): INT col = CLB col - 1.
        let int_base = far_encode(0, 0, (clb_col - 1) as u16, 0).unwrap();
        assert_eq!(int_base, 0x400);
        // The loaded device agrees.
        let device = load_device("xcku3p", &data_dir()).unwrap();
        assert_eq!(device.tile_type("SLICE_X0Y0"), Some("CLEL_R"));
        assert_eq!(device.tile_base_far("SLICE_X0Y0"), Some(0x500));
        assert_eq!(device.tile_base_far("INT_X0Y0"), Some(0x400));
        assert_eq!(device.idcode(), Some(0x04A6_3093));
        // The HPIO bridge (critic session 13 FLAG F1): the one
        // provisional value the real impl does not cover. It is
        // invented, pinned here so any change is deliberate, and
        // must never be consumed as ground truth.
        assert_eq!(device.tile_type("HPIO_RIGHT_X0Y0"), Some("HPIO_RIGHT"));
        assert_eq!(device.tile_base_far("HPIO_RIGHT_X0Y0"), Some(0x000));
        // A tile the file does not cover at all still fails loud.
        assert_eq!(device.tile_type("CMT_RIGHT_X0Y0"), None);
        assert_eq!(device.tile_base_far("CMT_RIGHT_X0Y0"), None);
        // The bridged tile is in the tile list, or parse-back would
        // fail on its emitted bits as unexplained.
        assert!(device.tile_names().contains(&"HPIO_RIGHT_X0Y0".to_string()));
    }

    #[test]
    fn unknown_part_fails_loud() {
        assert!(load_device("xcvu3p", &data_dir()).is_err());
        assert!(load_device("xczu3eg", &data_dir()).is_err());
    }

    #[test]
    fn ff_translation_rewrites_valued_lines_only() {
        let input = "# comment\nSLICE_X0Y0.AFF.INIT = 0\nSLICE_X0Y0.BFF.INIT = 1\n\
            SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n\
            SLICE_X0Y0.CFF.SRVAL = 1'b0\nSLICE_X0Y0.DFF.INIT = 0x1\n\
            INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n";
        let out = translate_ff_valued_fasm(input).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "# comment");
        assert_eq!(lines[1], "SLICE_X0Y0.AFF.INIT.V0");
        assert_eq!(lines[2], "SLICE_X0Y0.BFF.INIT.V1");
        assert_eq!(lines[3], "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555");
        assert_eq!(lines[4], "SLICE_X0Y0.CFF.SRVAL.V0");
        assert_eq!(lines[5], "SLICE_X0Y0.DFF.INIT.V1");
        assert_eq!(lines[6], "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1");
    }

    #[test]
    fn ff_translation_rejects_bad_values_and_valued_v() {
        assert!(translate_ff_valued_fasm("SLICE_X0Y0.AFF.INIT = 2\n").is_err());
        assert!(translate_ff_valued_fasm("SLICE_X0Y0.AFF.INIT = 1'h2\n").is_err());
        assert!(translate_ff_valued_fasm("SLICE_X0Y0.AFF.INIT.V0 = 0\n").is_err());
        assert!(translate_ff_valued_fasm("SLICE_X0Y0.AFF.INIT.V1 = 1\n").is_err());
        // Bare AFF.INIT passes through untouched (the library fails loud
        // on it as an unknown feature at emit).
        let out = translate_ff_valued_fasm("SLICE_X0Y0.AFF.INIT\n").unwrap();
        assert_eq!(out, "SLICE_X0Y0.AFF.INIT\n");
    }

    #[test]
    fn classify_matches_harness_registry() {
        assert_eq!(
            classify_feature("SLICE_X0Y0", "A6LUT.INIT[63:0]"),
            Some("LUT_INIT")
        );
        assert_eq!(
            classify_feature("SLICE_X0Y0", "ALUT.INIT[0]"),
            Some("LUT_INIT")
        );
        assert_eq!(
            classify_feature("SLICE_X0Y0", "ALUT.INIT[63]"),
            Some("LUT_INIT")
        );
        assert_eq!(classify_feature("SLICE_X0Y0", "AFF.INIT"), Some("FF_INIT"));
        assert_eq!(
            classify_feature("SLICE_X0Y0", "AFF.INIT.V0"),
            Some("FF_INIT")
        );
        assert_eq!(
            classify_feature("SLICE_X0Y0", "BFF.SRVAL.V1"),
            Some("FF_INIT")
        );
        assert_eq!(
            classify_feature("SLICE_X0Y0", "AFF2.INIT.V0"),
            Some("FF_INIT")
        );
        // FF group control attributes the harness registry does not
        // list by name (EFGHFF, CLKINV, MODE, SYNC): same block.
        assert_eq!(
            classify_feature("SLICE_X0Y0", "ABCDFF.CEUSED.V0"),
            Some("FF_INIT")
        );
        assert_eq!(
            classify_feature("SLICE_X0Y0", "EFGHFF.SYNC.ASYNC"),
            Some("FF_INIT")
        );
        assert_eq!(
            classify_feature("SLICE_X0Y0", "ABCDFF2.SRUSED.V0"),
            Some("FF_INIT")
        );
        // CARRY8 has no block in the harness registry: `bits` must
        // fail loud on a CARRY8 feature that sets bits, never guess.
        assert_eq!(classify_feature("SLICE_X0Y0", "CARRY8.DI0.DI0"), None);
        assert_eq!(
            classify_feature("INT_X0Y0", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"),
            Some("INT_PIP")
        );
        assert_eq!(
            classify_feature("HPIO_RIGHT_X0Y0", "IOB_X0Y0.IOSTANDARD_IN.LVCMOS12"),
            Some("IO_CONFIG")
        );
        assert_eq!(
            classify_feature("CMT_RIGHT_X0Y0", "PIP.CLK_HROUTE.SOME"),
            Some("CLK")
        );
        assert_eq!(classify_feature("SLICE_X0Y0", "NOSUCH.FEATURE"), None);
    }

    #[test]
    fn emit_parse_roundtrip_blinky_golden() {
        // End-to-end on the harness blinky golden, in-process: emit the
        // raw golden FASM (valued FF spelling, LUT [63:0] spelling),
        // parse it back, and compare in the harness canonical form via
        // the library's own round trip plus a spot check that the
        // parse-back renders the canonical .V0 spelling.
        let golden = include_str!("../../tests/fixtures/blinky.xcku3p.fasm");
        let device = load_device("xcku3p", &data_dir()).unwrap();
        let segbits = load_segbits(&data_dir()).unwrap();
        let translated = translate_ff_valued_fasm(golden).unwrap();
        let bytes = emit_bitstream(&translated, device.as_ref(), &segbits).unwrap();
        let back = parse_bitstream(&bytes, device.as_ref(), &segbits).unwrap();
        assert!(
            back.contains("SLICE_X0Y0.AFF.INIT.V0\n"),
            "parse-back renders canonical .V0, got:\n{back}"
        );
        assert!(
            back.contains("SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h5555555555555555\n"),
            "parse-back renders the LUT INIT value, got:\n{back}"
        );
        // Re-emitting the parse-back must set identical bits. The
        // comparison is on set bits, not on the raw frame maps: emit
        // creates an (empty) frame entry for every frame any feature
        // references, including value-0 `.V1` features, so the re-emit
        // legitimately carries empty frames the first emit never
        // touched (the parse-back renders complement `.V1` lines for
        // FF2 BELs the golden never mentions; their INIT bits live in
        // CLEL frames the golden does not reference). Empty frames
        // carry no configuration, so they are not a round-trip
        // difference.
        let frames1 = emit_frames(&translated, device.as_ref(), &segbits).unwrap();
        let frames2 = emit_frames(&back, device.as_ref(), &segbits).unwrap();
        let set_bits = |frames: &BTreeMap<u32, [u32; 93]>| -> BTreeSet<(u32, u16)> {
            let mut out = BTreeSet::new();
            for (far, words) in frames {
                for (wi, word) in words.iter().enumerate() {
                    let mut w = *word;
                    while w != 0 {
                        let b = w.trailing_zeros() as usize;
                        out.insert((*far, (wi * 32 + b) as u16));
                        w &= w - 1;
                    }
                }
            }
            out
        };
        assert_eq!(
            set_bits(&frames1),
            set_bits(&frames2),
            "parse-back re-emits identical set bits"
        );
    }

    #[test]
    fn bits_labels_blinky_set_bits() {
        let golden = include_str!("../../tests/fixtures/blinky.xcku3p.fasm");
        let device = load_device("xcku3p", &data_dir()).unwrap();
        let segbits = load_segbits(&data_dir()).unwrap();
        let translated = translate_ff_valued_fasm(golden).unwrap();
        let bytes = emit_bitstream(&translated, device.as_ref(), &segbits).unwrap();
        let frames = parse_frames(&bytes).unwrap();
        let mut frame_set: HashSet<(u32, u16)> = HashSet::new();
        for (far, words) in &frames {
            for (wi, word) in words.iter().enumerate() {
                let mut w = *word;
                while w != 0 {
                    let b = w.trailing_zeros() as usize;
                    frame_set.insert((*far, (wi * 32 + b) as u16));
                    w &= w - 1;
                }
            }
        }
        // LUT 0x5555... has 32 set bits, AFF INIT.V0 sets 1, the two PIPs
        // set 1 each (BOUNCE 64_00, GCLK 1 bit), HPIO sets at least 1.
        // The exact HPIO count is DB-defined; assert the labeled total
        // equals the frame set-bit total and every block is present.
        let out = bits_for_bytes(&bytes, device.as_ref(), &segbits).unwrap();
        let mut labeled: HashSet<(u32, u16)> = HashSet::new();
        let mut blocks: HashSet<&str> = HashSet::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(f.len(), 4, "bits line shape: {line}");
            labeled.insert((
                u32::from_str_radix(f[0], 16).unwrap(),
                f[1].parse().unwrap(),
            ));
            assert_eq!(f[2], "1");
            blocks.insert(f[3]);
        }
        assert_eq!(labeled, frame_set, "labeled bits equal frame set bits");
        assert!(blocks.contains("LUT_INIT"));
        assert!(blocks.contains("FF_INIT"));
        assert!(blocks.contains("INT_PIP"));
        assert!(blocks.contains("IO_CONFIG"));
    }
}
