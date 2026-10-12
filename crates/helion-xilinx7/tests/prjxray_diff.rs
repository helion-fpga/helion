//! Differential validation against the prjxray reference tools.
//!
//! These are dev-time tests: they drive prjxray's Python `FasmAssembler`
//! (and, when available, the `xc7frames2bit` binary) via subprocess and
//! require the X-Ray database. Each test **skips gracefully** (passes with
//! a notice) when its inputs are absent, so `cargo test` stays green on a
//! bare checkout.
//!
//! * `frame_diff_vs_fasm2frames`: same FASM through prjxray's assembler and
//!   this crate's [`helion_xilinx7::frames`] pipeline; the `.frm` outputs
//!   must be byte-identical.
//! * `bit_identical_vs_xc7frames2bit`: same frames through `xc7frames2bit`
//!   and this crate's [`helion_xilinx7::bitstream`] writer; the `.bit`
//!   outputs must be byte-identical modulo the TLV date/time tags.
//!
//! Environment overrides:
//! * `HELION_XILINX_DB`: X-Ray DB root (default `~/workspace/xilinx-dbsrc`).
//! * `HELION_PRJXRAY`: prjxray checkout (default: several candidates).
//! * `HELION_PYTHON`: Python with simplejson/textx/pyyaml/intervaltree.
//! * `HELION_XC7FRAMES2BIT`: path to the `xc7frames2bit` binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::frames::{
    BlockType, FrameAddr, FrameStore, SegBitsDb, TileBitLayout,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Harness: locating inputs (all optional; missing -> skip).
// ---------------------------------------------------------------------------

fn db_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HELION_XILINX_DB") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join("workspace/xilinx-dbsrc");
    p.is_dir().then_some(p)
}

/// Pick a part directory containing `tilegrid.json` (xc7a35t preferred).
fn find_part(artix7: &Path) -> Option<(String, PathBuf)> {
    for name in ["xc7a35t", "xc7a50t"] {
        let tg = artix7.join(name).join("tilegrid.json");
        if tg.is_file() {
            return Some((name.to_string(), tg));
        }
    }
    // Fall back to any directory with a tilegrid.json.
    let mut entries: Vec<_> = std::fs::read_dir(artix7).ok()?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let tg = e.path().join("tilegrid.json");
        if tg.is_file() {
            if let Some(name) = e.file_name().to_str() {
                return Some((name.to_string(), tg));
            }
        }
    }
    None
}

fn prjxray_root() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("HELION_PRJXRAY") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("workspace/prjxray-ref"));
        candidates.push(PathBuf::from(&home).join("workspace/xilinx-dbsrc/prjxray"));
        candidates.push(PathBuf::from("/tmp/prjxray-ref/prjxray"));
    }
    candidates.into_iter().find(|p| {
        // Either a full prjxray checkout (python package) ...
        let full = p.join("prjxray/fasm_assembler.py").is_file();
        // ... or the tools dir carrying fasm2frames.py (package found separately).
        let tools = p.join("fasm2frames.py").is_file();
        full || tools
    })
}

/// Locate the prjxray Python *package* (Database, fasm_assembler).
fn prjxray_package(root: &Path) -> Option<PathBuf> {
    if root.join("prjxray/fasm_assembler.py").is_file() {
        return Some(root.to_path_buf());
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(&home).join("workspace/prjxray-ref");
        if p.join("prjxray/fasm_assembler.py").is_file() {
            return Some(p);
        }
    }
    None
}

/// Locate the `fasm2frames.py` script.
fn fasm2frames_py(root: &Path) -> Option<PathBuf> {
    let direct = root.join("fasm2frames.py");
    if direct.is_file() {
        return Some(direct);
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(&home).join("workspace/xilinx-dbsrc/tools/prjxray/fasm2frames.py");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Python interpreter able to import the prjxray stack.
fn python(prjxray: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("HELION_PYTHON") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("workspace/prjxray-ref/venv/bin/python"));
    }
    candidates.push(PathBuf::from("python3"));
    candidates.into_iter().find(|py| {
        let probe = Command::new(py)
            .arg("-c")
            .arg("import simplejson, yaml, intervaltree, fasm, prjxray.db")
            .env("PYTHONPATH", prjxray.display().to_string())
            .output();
        matches!(probe, Ok(o) if o.status.success())
    })
}

fn xc7frames2bit_bin() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("HELION_XC7FRAMES2BIT") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("workspace/xilinx-dbsrc/xc7frames2bit"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

macro_rules! skip {
    ($($arg:tt)*) => {{
        eprintln!("SKIP (missing inputs): {}", format!($($arg)*));
        return;
    }};
}

// ---------------------------------------------------------------------------
// Scratch DB root: symlinks into the real DB plus synthetic mapping files.
// ---------------------------------------------------------------------------

/// Build `<tmp>/helion-x7diff-<part>/` with symlinks to the real DB files
/// plus the `mapping/*.yaml` prjxray's `Database` requires.
fn scratch_db_root(artix7: &Path, part: &str) -> Option<PathBuf> {
    let scratch = std::env::temp_dir().join(format!("helion-x7diff-{}", part));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("mapping")).ok()?;
    std::fs::create_dir_all(scratch.join(part)).ok()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = |src: &Path, dst: &Path| {
            let _ = symlink(src, dst);
        };
        // tilegrid for this part's fabric.
        link(
            &artix7.join(part).join("tilegrid.json"),
            &scratch.join(part).join("tilegrid.json"),
        );
        // fasm2frames.py also wants package_pins.csv and part.json next to
        // the tilegrid. Fall back to any part's copies (only pin/part metadata
        // is read; unused by this differential test).
        let part_dir = scratch.join(part);
        let mut names: Vec<_> = std::fs::read_dir(artix7)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .collect();
        names.sort_by_key(|e| e.file_name());
        for meta in ["package_pins.csv", "part.json"] {
            let own = artix7.join(part).join(meta);
            if own.is_file() {
                link(&own, &part_dir.join(meta));
                continue;
            }
            for e in &names {
                let p = e.path().join(meta);
                if p.is_file() {
                    link(&p, &part_dir.join(meta));
                    break;
                }
            }
        }
        // segbits / ppips / mask / tile_type files at the db root.
        if let Ok(entries) = std::fs::read_dir(artix7) {
            for e in entries.filter_map(|e| e.ok()) {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("segbits_")
                    || name.starts_with("ppips_")
                    || name.starts_with("mask_")
                    || (name.starts_with("tile_type_") && name.ends_with(".json"))
                {
                    link(&e.path(), &scratch.join(name.as_ref()));
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Without symlinks this harness cannot run cheaply; skip.
        return None;
    }

    std::fs::write(
        scratch.join("mapping/parts.yaml"),
        format!("{part}:\n  device: {part}\n"),
    )
    .ok()?;
    std::fs::write(
        scratch.join("mapping/devices.yaml"),
        format!("{part}:\n  fabric: {part}\n"),
    )
    .ok()?;
    Some(scratch)
}

// ---------------------------------------------------------------------------
// My side: tilegrid reader + layout.
// ---------------------------------------------------------------------------

struct TileEntry {
    tile_type: String,
    blocks: HashMap<BlockType, (FrameAddr, u32)>, // + frame count
}

struct TilegridLayout {
    tiles: HashMap<String, TileEntry>,
}

impl TilegridLayout {
    fn load(tilegrid: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(tilegrid).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        let obj = json.as_object()?;
        let mut tiles = HashMap::new();
        for (name, info) in obj {
            let tile_type = info.get("type")?.as_str()?.to_string();
            let mut blocks = HashMap::new();
            if let Some(bits) = info.get("bits").and_then(|b| b.as_object()) {
                for (bus, b) in bits {
                    let block = match bus.as_str() {
                        "CLB_IO_CLK" => BlockType::ClbIoClk,
                        "BLOCK_RAM" => BlockType::BlockRam,
                        _ => continue,
                    };
                    let baseaddr = b.get("baseaddr")?.as_str()?;
                    let baseaddr =
                        u32::from_str_radix(baseaddr.strip_prefix("0x").unwrap_or(baseaddr), 16)
                            .ok()?;
                    let offset = b.get("offset")?.as_u64()? as u32;
                    let frames = b.get("frames")?.as_u64()? as u32;
                    blocks.insert(block, (FrameAddr { baseaddr, offset }, frames));
                }
            }
            tiles.insert(name.clone(), TileEntry { tile_type, blocks });
        }
        Some(TilegridLayout { tiles })
    }

    /// Every frame address in the device (mirrors `Grid.iter_all_frames`).
    fn all_addresses(&self) -> Vec<u32> {
        let mut addrs = Vec::new();
        for tile in self.tiles.values() {
            for (geom, frames) in tile.blocks.values() {
                for i in 0..*frames {
                    addrs.push(geom.baseaddr + i);
                }
            }
        }
        addrs
    }
}

impl TileBitLayout for TilegridLayout {
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr> {
        Some(self.tiles.get(tile)?.blocks.get(&block)?.0)
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        Some(self.tiles.get(tile)?.tile_type.as_str())
    }
}

/// Load segbits (+ `.block_ram` variant) and ppips for the tile types in `types`.
fn load_segbits(artix7: &Path, types: &[&str]) -> Option<SegBitsDb> {
    let mut db = SegBitsDb::new();
    for t in types {
        let lower = t.to_lowercase();
        let seg = artix7.join(format!("segbits_{lower}.db"));
        if seg.is_file() {
            db.add_segbits(t, BlockType::ClbIoClk, &std::fs::read_to_string(seg).ok()?)
                .ok()?;
        }
        let bram = artix7.join(format!("segbits_{lower}.block_ram.db"));
        if bram.is_file() {
            db.add_segbits(t, BlockType::BlockRam, &std::fs::read_to_string(bram).ok()?)
                .ok()?;
        }
        let ppips = artix7.join(format!("ppips_{lower}.db"));
        if ppips.is_file() {
            db.add_ppips(t, &std::fs::read_to_string(ppips).ok()?).ok()?;
        }
    }
    Some(db)
}

/// Dump a frame store in prjxray `.frm` format.
fn dump_frm(store: &FrameStore) -> String {
    let mut out = String::new();
    for &addr in store.frame_addresses() {
        let words = store.frame_words(addr).expect("address in store");
        out.push_str(&format!("0x{addr:08X} "));
        for (i, w) in words.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("0x{w:08X}"));
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Build a small synthetic FASM from real tiles/features of the DB.
fn synthetic_fasm(layout: &TilegridLayout, artix7: &Path) -> Option<String> {
    let mut int_tile: Option<&str> = None;
    let mut clb_tile: Option<&str> = None;
    let mut names: Vec<&String> = layout.tiles.keys().collect();
    names.sort();
    for n in names {
        let t = &layout.tiles[n];
        if int_tile.is_none() && t.tile_type == "INT_L" && !t.blocks.is_empty() {
            int_tile = Some(n);
        }
        if clb_tile.is_none() && t.tile_type == "CLBLL_L" && !t.blocks.is_empty() {
            clb_tile = Some(n);
        }
        if int_tile.is_some() && clb_tile.is_some() {
            break;
        }
    }
    let (int_tile, clb_tile) = (int_tile?, clb_tile?);

    // First plain feature of segbits_int_l.db.
    let int_seg = std::fs::read_to_string(artix7.join("segbits_int_l.db")).ok()?;
    let int_feat = int_seg
        .lines()
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .find(|f| f.starts_with("INT_L.") && !f.contains('['))?;
    let int_feat = int_feat.strip_prefix("INT_L.").unwrap_or(int_feat);

    // INIT bit indices present for SLICEL_X0.ALUT.
    let clb_seg = std::fs::read_to_string(artix7.join("segbits_clbll_l.db")).ok()?;
    let mut init_idx: Vec<u32> = clb_seg
        .lines()
        .filter_map(|l| {
            let f = l.split_whitespace().next()?;
            let rest = f.strip_prefix("CLBLL_L.SLICEL_X0.ALUT.INIT[")?;
            rest.strip_suffix(']')?.parse::<u32>().ok()
        })
        .collect();
    init_idx.sort_unstable();
    init_idx.dedup();
    if init_idx.len() < 8 {
        return None;
    }
    let hi = init_idx[init_idx.len() - 1];
    let lo = hi - 3;

    Some(format!(
        "# synthetic differential-test FASM\n\
         {int_tile}.{int_feat}\n\
         {int_tile}.{int_feat} = 0\n\
         {clb_tile}.SLICEL_X0.ALUT.INIT[{hi}:{lo}] = 4'b1010\n\
         {clb_tile}.SLICEL_X0.ALUT.INIT[07:00] = 8'hA5\n"
    ))
}

#[test]
fn frame_diff_vs_fasm2frames() {
    let Some(db) = db_root() else {
        skip!("no Xilinx DB (set HELION_XILINX_DB)");
    };
    let artix7 = db.join("artix7");
    let Some((part, tilegrid_path)) = find_part(&artix7) else {
        skip!("no tilegrid.json in {}/artix7", db.display());
    };
    let Some(prjxray) = prjxray_root() else {
        skip!("no prjxray checkout (set HELION_PRJXRAY)");
    };
    let Some(pkg) = prjxray_package(&prjxray) else {
        skip!("no prjxray Python package under {}", prjxray.display());
    };
    let Some(fasm2frames) = fasm2frames_py(&prjxray) else {
        skip!("no fasm2frames.py found");
    };
    let Some(py) = python(&pkg) else {
        skip!("no Python with the prjxray stack (set HELION_PYTHON)");
    };
    let Some(scratch) = scratch_db_root(&artix7, &part) else {
        skip!("could not build scratch DB root");
    };

    let Some(layout) = TilegridLayout::load(&tilegrid_path) else {
        skip!("could not parse {}", tilegrid_path.display());
    };
    let Some(fasm_text) = synthetic_fasm(&layout, &artix7) else {
        skip!("could not synthesize FASM for {part}");
    };

    // Reference: prjxray fasm2frames.py -> ref.frm.
    let workdir = scratch.join("work");
    std::fs::create_dir_all(&workdir).expect("workdir");
    std::fs::write(workdir.join("synth.fasm"), &fasm_text).expect("fasm");
    let ref_frm = workdir.join("ref.frm");
    let status = Command::new(&py)
        .arg(&fasm2frames)
        .arg("--db-root")
        .arg(&scratch)
        .arg("--part")
        .arg(&part)
        .arg(workdir.join("synth.fasm"))
        .arg(&ref_frm)
        .env("PYTHONPATH", pkg.display().to_string())
        .output()
        .expect("run fasm2frames.py");
    if !status.status.success() {
        eprintln!("reference stderr:\n{}", String::from_utf8_lossy(&status.stderr));
        panic!("reference fasm2frames.py failed");
    }

    // Mine: parse -> frames -> dump.
    let segbits = load_segbits(&artix7, &["INT_L", "CLBLL_L"]).expect("segbits");
    let mut store = FrameStore::new(&layout.all_addresses());
    let lines =
        helion_xilinx7::fasm::parse_fasm(&fasm_text).expect("parse synthetic FASM");
    store
        .apply_fasm(&lines, &segbits, &layout)
        .expect("apply synthetic FASM");
    let mine = dump_frm(&store);
    std::fs::write(workdir.join("mine.frm"), &mine).expect("mine.frm");

    let reference = std::fs::read_to_string(&ref_frm).expect("ref.frm");
    if mine != reference {
        // Report the first differing frame for debugging.
        for (n, (a, b)) in mine.lines().zip(reference.lines()).enumerate() {
            if a != b {
                panic!(
                    "frame-diff mismatch at .frm line {n} (part {part}):\n mine: {}...\n ref:  {}...",
                    &a[..a.len().min(72)],
                    &b[..b.len().min(72)]
                );
            }
        }
        panic!(
            "frame-diff line count differs: mine {} vs ref {}",
            mine.lines().count(),
            reference.lines().count()
        );
    }
    eprintln!(
        "frame-diff OK: {} frames bit-identical vs fasm2frames ({part})",
        mine.lines().count()
    );
}

/// Normalize the TLV date/time tags (`c`/`d`) of a `.bit` for comparison.
fn normalize_bit_datetime(bit: &[u8]) -> Vec<u8> {
    let mut out = bit.to_vec();
    // Walk the TLV: magic(13) then tags; 'e' starts the data.
    let mut pos = 13;
    while pos + 3 <= out.len() {
        let tag = out[pos];
        if tag == b'e' {
            break;
        }
        if !b"abcd".contains(&tag) {
            break;
        }
        let len = u16::from_be_bytes([out[pos + 1], out[pos + 2]]) as usize;
        if tag == b'c' || tag == b'd' {
            for b in &mut out[pos + 3..pos + 3 + len] {
                *b = b'X';
            }
        }
        pos += 3 + len;
    }
    out
}

#[test]
fn bit_identical_vs_xc7frames2bit() {
    let Some(bin) = xc7frames2bit_bin() else {
        skip!("no xc7frames2bit binary (set HELION_XC7FRAMES2BIT)");
    };
    let Some(db) = db_root() else {
        skip!("no Xilinx DB (set HELION_XILINX_DB)");
    };
    let artix7 = db.join("artix7");
    let Some((part, tilegrid_path)) = find_part(&artix7) else {
        skip!("no tilegrid.json in {}/artix7", db.display());
    };
    // part.yaml: the DB carries part.json, which is valid YAML.
    let part_meta = artix7.join(&part).join("part.yaml");
    let part_meta = if part_meta.is_file() {
        part_meta
    } else {
        let json = artix7.join(&part).join("part.json");
        if !json.is_file() {
            skip!("no part metadata for {part}");
        }
        json
    };

    let Some(layout) = TilegridLayout::load(&tilegrid_path) else {
        skip!("could not parse {}", tilegrid_path.display());
    };
    let segbits = load_segbits(&artix7, &["INT_L", "CLBLL_L"]).expect("segbits");
    let Some(fasm_text) = synthetic_fasm(&layout, &artix7) else {
        skip!("could not synthesize FASM for {part}");
    };
    let mut store = FrameStore::new(&layout.all_addresses());
    let lines = helion_xilinx7::fasm::parse_fasm(&fasm_text).expect("parse");
    store.apply_fasm(&lines, &segbits, &layout).expect("apply");
    let payload = store.fdri_payload();
    let frm_path = std::env::temp_dir().join("helion-x7diff-test.frm");
    std::fs::write(&frm_path, dump_frm(&store)).expect("frm");

    // Reference .bit.
    let ref_bit = std::env::temp_dir().join("helion-x7diff-ref.bit");
    let status = Command::new(&bin)
        .arg("--frm_file")
        .arg(&frm_path)
        .arg("--part_file")
        .arg(&part_meta)
        .arg("--part_name")
        .arg(&part)
        .arg("--output_file")
        .arg(&ref_bit)
        .output()
        .expect("run xc7frames2bit");
    if !status.status.success() {
        eprintln!("stderr:\n{}", String::from_utf8_lossy(&status.stderr));
        panic!("xc7frames2bit failed");
    }

    // Mine, with the same 'a' tag the tool writes.
    let opts = BitstreamOptions::xc7frames2bit(
        &part,
        frm_path.to_str().expect("utf8"),
        "2000/01/01",
        "00:00:00",
    );
    let mine = bitstream::write_bit(&payload, &opts).expect("write .bit");
    let reference = std::fs::read(&ref_bit).expect("ref.bit");

    let (a, b) = (normalize_bit_datetime(&mine), normalize_bit_datetime(&reference));
    assert_eq!(
        a.len(),
        b.len(),
        "length differs: mine {} vs ref {}",
        a.len(),
        b.len()
    );
    if a != b {
        let first = a.iter().zip(b.iter()).position(|(x, y)| x != y).unwrap_or(0);
        panic!(
            ".bit differs at byte {first} (part {part}): mine {:02X?}... vs ref {:02X?}...",
            &a[first..(first + 16).min(a.len())],
            &b[first..(first + 16).min(b.len())]
        );
    }
    eprintln!("byte-compare OK: .bit identical vs xc7frames2bit ({part})");
}
