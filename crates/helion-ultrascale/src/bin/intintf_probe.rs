//! Probe for the INT_INTF segbits lane: attribute unexplained decode
//! bits to tile windows, and dump tile windows.
//!
//! `intintf_probe --db-root DIR --bit FILE` decodes the bitstream and
//! resolves every unexplained bit to the tile window that owns it
//! (tile name, tile type, relative frame and relative bit), or reports
//! it as a GAP bit in no window with its FAR column. It also lists the
//! decoded FASM lines for INT_INTF and HDIO tiles, so a design's
//! interface usage can be correlated with the unexplained bits.
//!
//! `intintf_probe --db-root DIR --windows PREFIX` dumps every tile
//! window whose tile type starts with PREFIX (name, type, FAR column,
//! frames, bit range, grid position, sites).
//!
//! This is analysis tooling only: it reads the public tilegrid and
//! segbits through the crate's own loaders and invents nothing.

use std::collections::BTreeMap;
use std::process::ExitCode;

use helion_ultrascale::bitstream_decode::{decode_bitstream, tile_windows, TileWindowInfo};

fn far_parts(far: u32) -> (u8, u8, u16, u8) {
    (
        ((far >> 24) & 0x7) as u8,
        ((far >> 18) & 0x3F) as u8,
        ((far >> 8) & 0x3FF) as u16,
        (far & 0xFF) as u8,
    )
}

fn usage() -> ! {
    eprintln!("usage: intintf_probe --db-root DIR (--bit FILE | --windows PREFIX)");
    std::process::exit(2);
}

fn main() -> ExitCode {
    let mut db_root: Option<String> = None;
    let mut bit: Option<String> = None;
    let mut windows_prefix: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db-root" => db_root = args.next(),
            "--bit" => bit = args.next(),
            "--windows" => windows_prefix = args.next(),
            _ => usage(),
        }
    }
    let Some(db_root) = db_root else { usage() };
    let windows = match tile_windows(&db_root) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("intintf_probe: {e}");
            return ExitCode::from(1);
        }
    };

    if let Some(prefix) = windows_prefix {
        let mut rows: Vec<&TileWindowInfo> = windows
            .iter()
            .filter(|w| w.tile_type.starts_with(&prefix))
            .collect();
        rows.sort_by(|a, b| a.tile_name.cmp(&b.tile_name));
        println!(
            "# windows for tile types starting with {prefix:?}: {}",
            rows.len()
        );
        for w in rows {
            println!(
                "{} {} block={} row={} col={} base_minor={} frames={} bits=[{}, {}) grid=({}, {}) sites={}",
                w.tile_name,
                w.tile_type,
                w.block,
                w.row,
                w.col,
                w.base_minor,
                w.frames,
                w.bit_lo,
                w.bit_hi,
                w.grid_x,
                w.grid_y,
                w.sites.join(","),
            );
        }
        return ExitCode::SUCCESS;
    }

    let Some(bit_path) = bit else { usage() };
    let bytes = match std::fs::read(&bit_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("intintf_probe: cannot read {bit_path}: {e}");
            return ExitCode::from(1);
        }
    };
    let out = match decode_bitstream(&bytes, &db_root) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("intintf_probe: decode failed: {e}");
            return ExitCode::from(1);
        }
    };

    // Attribute every unexplained bit to a window, or to a gap column.
    // Key: (tile_type, tile_name, rel_frame, rel_bit) -> count.
    let mut attributed: BTreeMap<(String, String, u32, u32), u32> = BTreeMap::new();
    // Gap key: (block, row, col, minor, abs_bit) -> count.
    let mut gaps: BTreeMap<(u8, u8, u16, u8, u32), u32> = BTreeMap::new();
    for b in &out.unexplained {
        let abs_bit = (b.word as u32) * 32 + (b.bit as u32);
        let (block, row, col, minor) = far_parts(b.far);
        let mut owner: Option<&TileWindowInfo> = None;
        for w in &windows {
            if w.block == block
                && w.row == row
                && w.col == col
                && minor >= w.base_minor
                && (minor as u32) < (w.base_minor as u32) + w.frames
                && abs_bit >= w.bit_lo
                && abs_bit < w.bit_hi
            {
                owner = Some(w);
                break;
            }
        }
        match owner {
            Some(w) => {
                let rel_frame = (minor - w.base_minor) as u32;
                let rel_bit = abs_bit - w.bit_lo;
                *attributed
                    .entry((
                        w.tile_type.clone(),
                        w.tile_name.clone(),
                        rel_frame,
                        rel_bit,
                    ))
                    .or_default() += 1;
            }
            None => {
                *gaps
                    .entry((block, row, col, minor, abs_bit))
                    .or_default() += 1;
            }
        }
    }

    println!("# fasm lines: {}", out.fasm.len());
    println!("# unexplained bits: {}", out.unexplained.len());
    println!("# --- attributed (tile_type tile_name rel_frame rel_bit count):");
    let mut by_type: BTreeMap<String, u32> = BTreeMap::new();
    for ((ty, name, frame, bit), count) in &attributed {
        println!("{ty} {name} frame={frame} bit={bit} count={count}");
        *by_type.entry(ty.clone()).or_default() += count;
    }
    println!("# --- attributed totals by tile type:");
    for (ty, count) in &by_type {
        println!("# {ty}: {count}");
    }
    println!("# --- gap bits (no window owns them): {} distinct positions", gaps.len());
    let mut gap_total = 0u32;
    for ((block, row, col, minor, abs_bit), count) in &gaps {
        println!("# GAP block={block} row={row} col={col} minor={minor} bit={abs_bit} count={count}");
        gap_total += count;
    }
    println!("# gap bits total: {gap_total}");
    println!("# --- decoded FASM lines for INT_INTF / HDIO tiles:");
    let mut intf_lines: Vec<&String> = out
        .fasm
        .iter()
        .filter(|l| l.contains("INT_INTF") || l.contains("HDIO"))
        .collect();
    intf_lines.sort();
    for l in intf_lines {
        println!("{l}");
    }
    ExitCode::SUCCESS
}
