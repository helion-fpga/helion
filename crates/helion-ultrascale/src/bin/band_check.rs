//! Full-corpus check for the H-1 band code (`band_check`).
//!
//! Reads the decoder-validation corpus frames and the converted LUTRAM
//! masks, runs the Rust MAP-A and H76 implementations frame by frame,
//! and reports exact/mismatch counts against the stored bands.
//!
//! Inputs (all outside this repo; nothing here is modified):
//! - `<validation-dir>/frames_dump/*.bin`: u32 nframes (little endian),
//!   then per frame u32 FAR + 93 u32 words, little endian.
//! - `<masks-dir>/<pair>.masks.bin`, one per pair, produced by
//!   `h1_band_rust_check_convert.py` in the validation directory from
//!   `h1_ecc_lutram_masks.pkl`: u32 nfar, then per FAR u32 far + 93 u32
//!   mask words (bit i of word w = absolute bit w*32+i masked).
//!   The pair name is the `P-xx` stem of the frames_dump file name.
//!
//! Usage:
//!   cargo run -j1 -p helion-ultrascale --bin band_check -- \
//!     --validation-dir ~/workspace/ultra-bitgen-crew/decoder-validation \
//!     --masks-dir /tmp/band_check_masks
//!
//! Expected totals (H1_ECC_TEST.md): MAP-A masked 144,332/144,816,
//! H76 143,269/144,816, MAP-A residuals all exactly w =
//! 0xf0000000000f (484 frames), block-1 band identically 0 (64,512
//! frames). Exit status is 0 when the run completes; the counts in
//! the output are the verdict, compared against those numbers.

use helion_ultrascale::band_code::{
    BAND_MASK, Q_RESIDUAL, BandCheck, check_stored_band, compute_band_h76, frame_band,
};
use helion_ultrascale::bitgen::{FRAME_BITS, FrameWords, far_decode};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn read_u32_le(bytes: &[u8], off: &mut usize) -> Option<u32> {
    let chunk = bytes.get(*off..*off + 4)?;
    *off += 4;
    Some(u32::from_le_bytes(chunk.try_into().ok()?))
}

fn read_frames(path: &Path) -> Result<Vec<(u32, FrameWords)>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut off = 0usize;
    let n = read_u32_le(&bytes, &mut off)
        .ok_or_else(|| format!("{}: missing frame count", path.display()))? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let far = read_u32_le(&bytes, &mut off)
            .ok_or_else(|| format!("{}: truncated FAR", path.display()))?;
        let mut words = [0u32; 93];
        for w in words.iter_mut() {
            *w = read_u32_le(&bytes, &mut off)
                .ok_or_else(|| format!("{}: truncated frame", path.display()))?;
        }
        out.push((far, words));
    }
    Ok(out)
}

fn read_masks(path: &Path) -> Result<HashMap<u32, [bool; FRAME_BITS]>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut off = 0usize;
    let n = read_u32_le(&bytes, &mut off)
        .ok_or_else(|| format!("{}: missing FAR count", path.display()))? as usize;
    let mut out = HashMap::with_capacity(n);
    for _ in 0..n {
        let far = read_u32_le(&bytes, &mut off)
            .ok_or_else(|| format!("{}: truncated mask FAR", path.display()))?;
        let mut mask = [false; FRAME_BITS];
        for wi in 0..93usize {
            let word = read_u32_le(&bytes, &mut off)
                .ok_or_else(|| format!("{}: truncated mask words", path.display()))?;
            for b in 0..32 {
                if (word >> b) & 1 == 1 {
                    mask[wi * 32 + b] = true;
                }
            }
        }
        out.insert(far, mask);
    }
    Ok(out)
}

fn pair_name(file_name: &str) -> Option<String> {
    // e.g. "P-11_resizer_ultra96.bin" -> "P-11".
    let stem = file_name.strip_suffix(".bin")?;
    Some(stem.split('_').next()?.to_string())
}

fn main() -> Result<(), String> {
    let mut validation_dir: Option<PathBuf> = None;
    let mut masks_dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--validation-dir" => {
                validation_dir = Some(PathBuf::from(
                    args.next().ok_or("--validation-dir needs a value")?,
                ));
            }
            "--masks-dir" => {
                masks_dir = Some(PathBuf::from(
                    args.next().ok_or("--masks-dir needs a value")?,
                ));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let validation_dir = validation_dir.ok_or("--validation-dir is required")?;
    let masks_dir = masks_dir.ok_or("--masks-dir is required")?;
    let frames_dir = validation_dir.join("frames_dump");
    let mut frame_files: Vec<PathBuf> = std::fs::read_dir(&frames_dir)
        .map_err(|e| format!("cannot list {}: {e}", frames_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bin"))
        .collect();
    frame_files.sort();

    let mut total_b0 = 0usize;
    let mut total_b1 = 0usize;
    let mut map_a_exact = 0usize;
    let mut map_a_w = 0usize;
    let mut map_a_other = 0usize;
    let mut h76_exact = 0usize;
    let mut h76_w = 0usize;
    let mut h76_other = 0usize;
    let mut b1_nonzero = 0usize;
    let mut other_block = 0usize;

    for path in &frame_files {
        let pair = pair_name(
            path.file_name()
                .ok_or("frame file has no name")?
                .to_str()
                .ok_or("frame file name is not UTF-8")?,
        )
        .ok_or_else(|| format!("cannot derive pair name from {}", path.display()))?;
        let masks = read_masks(&masks_dir.join(format!("{pair}.masks.bin")))?;
        let frames = read_frames(path)?;
        let empty_mask = [false; FRAME_BITS];
        let mut pair_b0 = 0usize;
        let mut pair_a = 0usize;
        let mut pair_h = 0usize;
        for (far, words) in &frames {
            let (block, _, _, _) = far_decode(*far);
            match block {
                0 => {
                    total_b0 += 1;
                    pair_b0 += 1;
                    let stored = frame_band(words) & BAND_MASK;
                    let mask = masks.get(far).unwrap_or(&empty_mask);
                    // The same check `parse_bitstream` runs (N7):
                    // Exact counts as MAP-A exact, QResidual as the
                    // w residual, Mismatch as other. H76 below stays
                    // an independent cross-check, not this function.
                    match check_stored_band(*far, words, mask) {
                        BandCheck::Exact => {
                            map_a_exact += 1;
                            pair_a += 1;
                        }
                        BandCheck::QResidual => {
                            map_a_w += 1;
                        }
                        BandCheck::Mismatch { .. } => {
                            map_a_other += 1;
                        }
                    }
                    let pred_h = compute_band_h76(words);
                    if pred_h == stored {
                        h76_exact += 1;
                        pair_h += 1;
                    } else if pred_h ^ stored == Q_RESIDUAL {
                        h76_w += 1;
                    } else {
                        h76_other += 1;
                    }
                }
                1 => {
                    total_b1 += 1;
                    // The same check `parse_bitstream` runs: a
                    // block-1 frame expects a zero band.
                    if check_stored_band(*far, words, &empty_mask) != BandCheck::Exact {
                        b1_nonzero += 1;
                    }
                }
                _ => {
                    other_block += 1;
                }
            }
        }
        println!("{pair}: block0 {pair_b0} MAP-A exact {pair_a} H76 exact {pair_h}");
    }
    println!("block0 frames: {total_b0}");
    println!("MAP-A masked exact: {map_a_exact}");
    println!("MAP-A residual w (0xf0000000000f): {map_a_w}");
    println!("MAP-A residual other: {map_a_other}");
    println!("H76 exact: {h76_exact}");
    println!("H76 residual w: {h76_w}");
    println!("H76 residual other: {h76_other}");
    println!("block1 frames: {total_b1}");
    println!("block1 nonzero band: {b1_nonzero}");
    println!("other block-type frames: {other_block}");
    Ok(())
}
