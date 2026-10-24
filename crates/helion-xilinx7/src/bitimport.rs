//! Read a 7-series `.bit` back into frame words.
//!
//! This is the reverse of [`crate::bitstream`]: it parses thebit-file envelope,
//! walks the configuration packets, extracts the FDRI frame payload, and maps
//! frame addresses onto payload word offsets (202 zero pad words between FAR
//! groups, 101 words per frame).
//!
//! Reference semantics: `work/x7decode.py` in the zedboard debug workspace,
//! which is the decoder every bring-up experiment was validated against.

use std::collections::BTreeMap;

/// The payload and the FAR -> word-offset map for its 101-word frames.
pub struct ImportedFrames {
    /// Configuration frame payload words, in stream order.
    pub payload: Vec<u32>,
    /// FAR address -> index of the frame's first word inside `payload`.
    pub frame_index: BTreeMap<u32, usize>,
}

/// Parse the `.bit` envelope (bus-width detect, sync, TLV records) and walk
/// the configuration packets to the FDRI payload. Mirrors `x7decode.payload`.
pub fn parse_payload(b: &[u8]) -> Result<Vec<u32>, String> {
    if b.len() < 13 || b[0] != 0x00 || b[1] != 0x09 {
        return Err("bitimport: missing 0x0009 envelope key".into());
    }
    let mut i = 13usize;
    let mut stream: Option<&[u8]> = None;
    while i < b.len() {
        let t = b[i];
        i += 1;
        if t == b'e' {
            if i + 4 > b.len() {
                return Err("bitimport: truncated 'e' record".into());
            }
            let n = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            i += 4;
            if i + n > b.len() {
                return Err("bitimport: truncated config stream".into());
            }
            stream = Some(&b[i..i + n]);
            i += n;
        } else {
            if i + 2 > b.len() {
                return Err("bitimport: truncated TLV record".into());
            }
            let n = u16::from_be_bytes([b[i], b[i + 1]]) as usize;
            i += 2 + n;
        }
    }
    let stream = stream.ok_or("bitimport: no config stream record")?;
    let words: Vec<u32> = stream
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    // Walk the packet stream to the Type-1 FDRI(count=0) + Type-2 header pair.
    // Registers: 2 = FDRI. Zero-count Type-1 FDRI write precedes the Type-2
    // word whose low 27 bits carry the frame count.
    let mut j = 0usize;
    while j < words.len() {
        let w = words[j];
        let typ = w >> 29;
        if typ == 1 {
            let op = (w >> 27) & 3;
            let reg = (w >> 13) & 0x3FFF;
            let cnt = (w & 0x7FF) as usize;
            j += 1;
            if op == 2 && reg == 2 && cnt == 0 {
                if j >= words.len() {
                    return Err("bitimport: FDRI type-2 header missing".into());
                }
                let w2 = words[j];
                j += 1;
                if (w2 >> 29) != 2 {
                    return Err("bitimport: expected type-2 FDRI header".into());
                }
                let n = (w2 & 0x07FF_FFFF) as usize;
                if j + n > words.len() {
                    return Err("bitimport: FDRI payload truncated".into());
                }
                return Ok(words[j..j + n].to_vec());
            }
            if op == 2 {
                // a counted type-1 write: skip its operands
                j += cnt;
            }
        } else if typ == 2 {
            j += 1 + ((w & 0x07FF_FFFF) as usize);
        } else {
            j += 1;
        }
    }
    Err("bitimport: no FDRI payload found".into())
}

/// Group key used to lay out pad frames in the FDRI payload: consecutive
/// addresses in the same group are contiguous; crossing a group inserts
/// 202 zero pad words (two blocks of 101).
///
/// Mirrors `x7decode.far_group`: (block, top, row) fields of the FAR.
pub fn far_group(addr: u32) -> (u32, u32, u32) {
    ((addr >> 23) & 7, (addr >> 22) & 1, (addr >> 17) & 0x1F)
}

/// Walk the payload along the device's frame-address stream, recording where
/// each address' 101 words live. Mirrors `x7decode.frames_by_address`.
pub fn index_frames(payload: &[u32], addrs: &[u32]) -> Result<ImportedFrames, String> {
    let mut frame_index = BTreeMap::new();
    let mut i = 0usize;
    let mut prev: Option<(u32, u32, u32)> = None;
    for &a in addrs {
        let g = far_group(a);
        if let Some(p) = prev {
            if g != p {
                // two pad blocks of 101 words (202) sit between groups
                if i + 202 > payload.len() {
                    return Err("bitimport: payload exhausted at pad".into());
                }
                for &w in &payload[i..i + 202] {
                    if w != 0 {
                        return Err("bitimport: non-zero pad words between groups".into());
                    }
                }
                i += 202;
            }
        }
        if i + 101 > payload.len() {
            return Err("bitimport: payload exhausted at frame".into());
        }
        frame_index.insert(a, i);
        i += 101;
        prev = Some(g);
    }
    Ok(ImportedFrames { payload: payload.to_vec(), frame_index })
}

/// Full import: bytes -> frames.
pub fn import(b: &[u8], addrs: &[u32]) -> Result<ImportedFrames, String> {
    let payload = parse_payload(b)?;
    index_frames(&payload, addrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const GOLDEN_WORD_COUNT: usize = 1_010_808;
    /// CLBLM column word window of the INT tile at row 44 of the proven
    /// counter column, recorded from the reference decoder
    /// (`work/x7decode.py`): frame 0x422380, words 89 and 90.
    const GOLDEN_BASE: u32 = 0x422380;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/BLINK-242.bit")
    }

    fn frame_addresses() -> Vec<u32> {
        // The device's full configuration stream, same order the bitstream
        // writer uses (AGENTS.md: FrameStore must cover every part.json
        // frame, never the tilegrid union).
        let db_root = std::env::var("HELION_XILINX_DB")
            .map(std::path::PathBuf::from)
            .expect("HELION_XILINX_DB must point at a prjxray-db checkout");
        let part_json = db_root.join("zynq7").join("xc7z020clg484-3").join("part.json");
        let part = crate::db::load::load_part(&part_json).expect("load xc7z020 part info");
        part.frame_addresses().expect("frame addresses")
    }

    #[test]
    fn parses_proven_blinky_payload() {
        let b = std::fs::read(fixture()).expect("fixture bitstream");
        let pay = parse_payload(&b).expect("payload");
        assert_eq!(pay.len(), GOLDEN_WORD_COUNT, "FDRI payload word count");
        assert!(pay[..8].iter().all(|&w| w == 0), "leading zero words");
    }

    #[test]
    fn indexes_proven_blinky_frames() {
        let b = std::fs::read(fixture()).expect("fixture bitstream");
        let addrs = frame_addresses();
        assert_eq!(addrs.len(), 9996, "xc7z020 frame count");
        let ifr = import(&b, &addrs).expect("import");
        let start = *ifr.frame_index.get(&GOLDEN_BASE).expect("frame present");
        assert_eq!(ifr.payload[start + 89], 0x0A420800, "frame w[89]");
        assert_eq!(ifr.payload[start + 90], 0x00010000, "frame w[90]");
    }
}
