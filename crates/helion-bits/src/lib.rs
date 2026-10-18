//! `.hbits` packets and feature assembly → frames.

use helion_device::{BitLoc, Device, Far};
use helion_route::Routed;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// Stable machine-parseable refuse code: empty / header-only bitstream.
/// Prefixed on bitgen empty-design / no-configured-frames Err strings (keep human text).
pub const ERR_CODE_EMPTY_BITSTREAM: &str = "HELION_E_EMPTY_BITSTREAM";

/// Stable machine-parseable refuse code: board program with USB=0 / no probe.
/// Prefixed on OFL / native Io no-device program Err strings (keep human text).
pub const ERR_CODE_USB_0: &str = "HELION_E_USB_0";

#[derive(Clone, Debug, Default)]
pub struct FeatureSet {
    /// full feature name -> 0/1
    pub bits: BTreeMap<String, bool>,
}

impl FeatureSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: impl Into<String>, val: bool) {
        self.bits.insert(name.into(), val);
    }

    pub fn set_init(&mut self, x: u32, y: u32, ble: u32, init: u64) {
        // Only program 1-bits; unset INIT bits stay at reset 0 (assemble skips false).
        for i in 0..64u32 {
            if (init >> i) & 1 == 1 {
                self.set(format!("CLB_X{x}Y{y}.BLE{ble}.LUT.INIT[{i}]"), true);
            }
        }
    }

    pub fn set_ff_used(&mut self, x: u32, y: u32, ble: u32, used: bool) {
        // FF.USED reset is 0; only assert when the BLE packs an FF.
        if used {
            self.set(format!("CLB_X{x}Y{y}.BLE{ble}.FF.USED"), true);
        }
    }

    pub fn set_imux(&mut self, x: u32, y: u32, mux: u32, sel: u8) {
        // 8-bit sel: bits 0..4 gold-stable; bit5 = N-S±2 / E-W±1; bit6 = E-W±2 / diag±1;
        // bit7 = knight; sel 112-127 N-S±3; 192-255 E-W±3 / diag±2 / N-S±4. Legacy abs for 0..7 unchanged.
        for b in 0..8u32 {
            if (sel >> b) & 1 == 1 {
                self.set(format!("CLB_X{x}Y{y}.IMUX[{mux}][{b}]"), true);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bitstream {
    pub idcode: u32,
    /// (block, major, minor) -> 128-bit frame
    pub frames: BTreeMap<(u8, u16, u8), u128>,
    pub packets: Vec<u8>,
}

impl Bitstream {
    pub fn empty(dev: &Device) -> Self {
        // Sparse from the start: reset frames are absent (same as encode skip of 0).
        // Avoids O(n_clb * minors) zero inserts on every bitgen (Ibex-scale).
        let mut bs = Self {
            idcode: dev.idcode,
            frames: BTreeMap::new(),
            packets: Vec::new(),
        };
        bs.packets = encode_packets(dev.idcode, &bs.frames);
        bs
    }

    /// Rebuild a bitstream from a `.hbits` packet stream (CLI flash / program).
    pub fn from_packets(bytes: &[u8]) -> Result<Self, String> {
        let (idcode, frames) = decode_packets(bytes)?;
        Ok(Self {
            idcode,
            frames,
            packets: bytes.to_vec(),
        })
    }
}

/// FeatureMap abs for CLB INIT/FF/IMUX. Packer is identical across Helion parts.
#[derive(Clone, Copy)]
struct ClbFeat {
    frame_bits: u32,
    init0_abs: [u32; 8],
    ff_used_abs: [u32; 8],
    imux0_abs: u32,
    imux5_abs: u32,
    imux6_abs: u32,
    imux7_abs: u32,
}

fn clb_feat(dev: &Device) -> Result<ClbFeat, String> {
    static CACHED: OnceLock<ClbFeat> = OnceLock::new();
    let fb = dev.featuremap().frame_bits;
    if let Some(c) = CACHED.get() {
        if c.frame_bits == fb {
            return Ok(*c);
        }
        return load_clb_feat(dev);
    }
    let built = load_clb_feat(dev)?;
    Ok(*CACHED.get_or_init(|| built))
}

fn load_clb_feat(dev: &Device) -> Result<ClbFeat, String> {
    let fm = dev.featuremap();
    let mut init0_abs = [0u32; 8];
    let mut ff_used_abs = [0u32; 8];
    for ble in 0..8u32 {
        let k = format!("BLE{ble}.LUT.INIT[0]");
        init0_abs[ble as usize] = fm
            .abs_bit(&k)
            .ok_or_else(|| format!("unknown feature {k}"))?;
        let k = format!("BLE{ble}.FF.USED");
        ff_used_abs[ble as usize] = fm
            .abs_bit(&k)
            .ok_or_else(|| format!("unknown feature {k}"))?;
    }
    let need = |k: &str| fm.abs_bit(k).ok_or_else(|| format!("unknown feature {k}"));
    let feat = ClbFeat {
        frame_bits: fm.frame_bits,
        init0_abs,
        ff_used_abs,
        imux0_abs: need("IMUX[0][0]")?,
        imux5_abs: need("IMUX[0][5]")?,
        imux6_abs: need("IMUX[0][6]")?,
        imux7_abs: need("IMUX[0][7]")?,
    };
    debug_assert_eq!(
        fm.abs_bit("BLE0.LUT.INIT[63]"),
        Some(feat.init0_abs[0] + 63)
    );
    debug_assert_eq!(fm.abs_bit("IMUX[1][0]"), Some(feat.imux0_abs + 5));
    debug_assert_eq!(fm.abs_bit("IMUX[2][5]"), Some(feat.imux5_abs + 2));
    debug_assert_eq!(fm.abs_bit("IMUX[2][6]"), Some(feat.imux6_abs + 2));
    debug_assert_eq!(fm.abs_bit("IMUX[2][7]"), Some(feat.imux7_abs + 2));
    Ok(feat)
}

#[inline]
fn or_clb_mask(
    frames: &mut HashMap<(u8, u16, u8), u128>,
    major: u16,
    abs: u32,
    frame_bits: u32,
    mask: u128,
) {
    if mask == 0 {
        return;
    }
    let minor = (abs / frame_bits) as u8;
    let bit = abs % frame_bits;
    *frames.entry((Far::CLB_IO_CLK, major, minor)).or_insert(0) |= mask << bit;
}

#[inline]
fn or_clb_bit(frames: &mut HashMap<(u8, u16, u8), u128>, major: u16, abs: u32, frame_bits: u32) {
    or_clb_mask(frames, major, abs, frame_bits, 1);
}

/// OR LUT INIT / FF.USED / PathFinder IMUX into frame words (no FeatureSet strings).
fn program_clb_frames(
    dev: &Device,
    routed: &Routed,
    frames: &mut HashMap<(u8, u16, u8), u128>,
) -> Result<(), String> {
    let feat = clb_feat(dev)?;
    let fb = feat.frame_bits;
    let packed = &routed.placed.packed;
    for (i, lutff) in packed.lutffs.iter().enumerate() {
        let (site, ble) = routed.placed.lutff_sites[i];
        let major = dev
            .clb_major(site.x, site.y)
            .ok_or_else(|| format!("not a CLB site CLB_X{}Y{}", site.x, site.y))?;
        let bi = ble as usize;
        if bi >= 8 {
            return Err(format!("unknown feature BLE{ble}.LUT.INIT[0]"));
        }
        let abs0 = feat.init0_abs[bi];
        let bit0 = abs0 % fb;
        if bit0 + 64 <= fb {
            or_clb_mask(frames, major, abs0, fb, lutff.init as u128);
        } else {
            for b in 0..64u32 {
                if (lutff.init >> b) & 1 == 1 {
                    or_clb_bit(frames, major, abs0 + b, fb);
                }
            }
        }
        if !lutff.ff_cell.is_empty() {
            or_clb_bit(frames, major, feat.ff_used_abs[bi], fb);
        }
    }
    for m in &routed.imux {
        let major = dev
            .clb_major(m.x, m.y)
            .ok_or_else(|| format!("not a CLB site CLB_X{}Y{}", m.x, m.y))?;
        if m.mux >= 64 {
            return Err(format!("unknown feature IMUX[{}][0]", m.mux));
        }
        let sel = m.sel;
        for b in 0..8u32 {
            if (sel >> b) & 1 == 0 {
                continue;
            }
            let abs = match b {
                0..=4 => feat.imux0_abs + m.mux * 5 + b,
                5 => feat.imux5_abs + m.mux,
                6 => feat.imux6_abs + m.mux,
                7 => feat.imux7_abs + m.mux,
                _ => unreachable!(),
            };
            or_clb_bit(frames, major, abs, fb);
        }
    }
    Ok(())
}

/// Bitgen a routed design: every LUTFF INIT/FF, IMUX from PathFinder, IOB src, DSP/BRAM USED.
///
/// Honesty: refuses an empty/fake success. `Bitstream::empty` remains for explicit
/// sim smoke only — `bitgen` never returns a header-only stream for a real design.
pub fn bitgen(dev: &Device, routed: &Routed) -> Result<Bitstream, String> {
    let packed = &routed.placed.packed;
    if packed.lutffs.is_empty()
        && packed.iobs.is_empty()
        && packed.macs.is_empty()
        && packed.brams.is_empty()
    {
        return Err(format!(
            "{ERR_CODE_EMPTY_BITSTREAM}: bitgen: empty design (no LUTFF/IOB/DSP/BRAM) — refusing empty/fake bitstream"
        ));
    }
    // Frame buffer: accumulate words in a HashMap, encode once (no FeatureSet / empty packets).
    let mut acc: HashMap<(u8, u16, u8), u128> = HashMap::with_capacity(
        packed
            .lutffs
            .len()
            .saturating_add(routed.imux.len())
            .saturating_add(packed.iobs.len())
            .saturating_add(packed.macs.len())
            .saturating_add(packed.brams.len().saturating_mul(2)),
    );
    program_clb_frames(dev, routed, &mut acc)?;
    for r in &routed.iob_src {
        let major = dev
            .iob_major(r.iob.0, r.iob.1)
            .ok_or_else(|| format!("IOB_X{}Y{} has no major", r.iob.0, r.iob.1))?;
        // bit0 USED, [3:1] BLE, [15:4] CLB y; [24:16] DRIVE/SLEW/PULLTYPE/DIFF_TERM/IN_TERM (0 = default)
        let elec = packed
            .iobs
            .iter()
            .find(|i| i.from_net == r.net)
            .map(|i| {
                Device::iob_electrical_bits(
                    i.drive.as_deref(),
                    i.slew.as_deref(),
                    i.pulltype.as_deref(),
                    i.diff_term.as_deref(),
                    i.in_term.as_deref(),
                )
            })
            .unwrap_or(0);
        let word = 1u128 | ((r.ble as u128) << 1) | ((r.clb.1 as u128) << 4) | elec;
        acc.insert((Far::IOB, major, 0), word);
    }
    for (i, _m) in packed.macs.iter().enumerate() {
        let site = routed.placed.mac_sites[i];
        let word = 1u128 | ((site.x as u128) << 8) | ((site.y as u128) << 16);
        acc.insert((Far::DSP, i as u16, 0), word);
    }
    for (i, b) in packed.brams.iter().enumerate() {
        let site = routed.placed.bram_sites[i];
        let word = 1u128 | ((site.x as u128) << 8) | ((site.y as u128) << 16);
        acc.insert((Far::BRAM, i as u16, 0), word);
        for (wi, w) in b.init.iter().enumerate() {
            let minor = 1u8 + (wi as u8);
            acc.insert((Far::BRAM, i as u16, minor), *w as u128);
        }
    }
    acc.retain(|_, w| *w != 0);
    if acc.is_empty() {
        return Err(format!(
            "{ERR_CODE_EMPTY_BITSTREAM}: bitgen: no configured frames — refusing empty/fake bitstream (design set no bits)"
        ));
    }
    let frames: BTreeMap<(u8, u16, u8), u128> = acc.into_iter().collect();
    let packets = encode_packets(dev.idcode, &frames);
    // Header CRC + body hash are always computed in encode_packets. In-stream
    // CRC_CHECK (0x21) remains a 0 stub (device CRC not modeled); size-stable.
    debug_assert_ne!(
        packets,
        Bitstream::empty(dev).packets,
        "bitgen must not equal Bitstream::empty"
    );
    Ok(Bitstream {
        idcode: dev.idcode,
        frames,
        packets,
    })
}

pub fn assemble(dev: &Device, feats: &FeatureSet) -> Result<Bitstream, String> {
    let mut frames = BTreeMap::new();
    for (name, val) in &feats.bits {
        if !*val {
            continue;
        }
        let loc: BitLoc = dev.locate(name)?;
        let key = (loc.far.block_type, loc.far.major, loc.far.minor);
        *frames.entry(key).or_insert(0) |= 1u128 << loc.bit;
    }
    let packets = encode_packets(dev.idcode, &frames);
    Ok(Bitstream {
        idcode: dev.idcode,
        frames,
        packets,
    })
}

/// A WRITE_FDRI length field is 16-bit, so a contiguous run is chunked at
/// 4095 frames (65520 bytes) instead of silently truncating the length.
pub const MAX_RUN_FRAMES: usize = 4095;

/// `.hbits` header: HBIT, version, idcode, flags, body length, body hash, header CRC.
pub const HBITS_HEADER_BYTES: usize = 4 + 2 + 4 + 4 + 8 + 32 + 4;

/// Encode frames as `.hbits` packets. Only frames that carry configuration are
/// written; a frame the stream never addresses keeps its reset value, so a
/// 4-LUT design does not pay for every frame on the die.
pub fn encode_packets(idcode: u32, frames: &BTreeMap<(u8, u16, u8), u128>) -> Vec<u8> {
    let n_frames = frames.values().filter(|p| **p != 0).count();
    // SYNC(7) + worst-case isolated FAR+FDRI (7+3+16) + CRC_CHECK(7) + DESYNC(3).
    let body_cap = 7 + n_frames.saturating_mul(26) + 10;
    // Write buffer: one reserved Vec; header slot patched after the body.
    let mut out = Vec::with_capacity(HBITS_HEADER_BYTES + body_cap);
    out.resize(HBITS_HEADER_BYTES, 0);
    // SYNC
    out.push(0x01);
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(b"HELI");
    let mut run_start: Option<u32> = None;
    let mut last_far: Option<u32> = None;
    let mut run: Vec<u128> = Vec::with_capacity(n_frames.min(MAX_RUN_FRAMES));
    // BTreeMap iterates in (block, major, minor) order already.
    for ((block, major, minor), payload) in frames {
        if *payload == 0 {
            continue;
        }
        let far = Far {
            block_type: *block,
            die: 0,
            major: *major,
            minor: *minor,
        }
        .encode();
        let contiguous = last_far.map(|prev| far == prev + 1).unwrap_or(false);
        if !contiguous || run.len() >= MAX_RUN_FRAMES {
            if let Some(start) = run_start {
                if !run.is_empty() {
                    flush_run(&mut out, start, &run);
                }
            }
            run.clear();
            run_start = Some(far);
        }
        run.push(*payload);
        last_far = Some(far);
    }
    if let Some(start) = run_start {
        if !run.is_empty() {
            flush_run(&mut out, start, &run);
        }
    }
    // CRC_CHECK
    out.push(0x21);
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    // DESYNC
    out.push(0x02);
    out.extend_from_slice(&0u16.to_le_bytes());

    let body_len = (out.len() - HBITS_HEADER_BYTES) as u64;
    out[0..4].copy_from_slice(b"HBIT");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6..10].copy_from_slice(&idcode.to_le_bytes());
    out[10..14].copy_from_slice(&0u32.to_le_bytes()); // flags
    out[14..22].copy_from_slice(&body_len.to_le_bytes());
    let hash = sha256_lite(&out[HBITS_HEADER_BYTES..]);
    out[22..54].copy_from_slice(&hash);
    let hdr_crc = crc32c(&out[..54]);
    out[54..58].copy_from_slice(&hdr_crc.to_le_bytes());
    out
}

/// Parse a `.hbits` stream back into (idcode, frames). Frames the stream does
/// not address are absent, i.e. still at their reset value.
pub fn decode_packets(bytes: &[u8]) -> Result<(u32, BTreeMap<(u8, u16, u8), u128>), String> {
    if bytes.len() < HBITS_HEADER_BYTES || &bytes[0..4] != b"HBIT" {
        return Err("not a .hbits stream".into());
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != 1 {
        return Err(format!("unsupported .hbits version {version}"));
    }
    let idcode = u32::from_le_bytes(bytes[6..10].try_into().unwrap());
    let body_len = u64::from_le_bytes(bytes[14..22].try_into().unwrap()) as usize;
    let stored_hash = &bytes[22..54];
    let hdr_crc = u32::from_le_bytes(bytes[54..58].try_into().unwrap());
    if crc32c(&bytes[..54]) != hdr_crc {
        return Err("header CRC mismatch".into());
    }
    let body = bytes
        .get(HBITS_HEADER_BYTES..HBITS_HEADER_BYTES + body_len)
        .ok_or_else(|| "truncated .hbits body".to_string())?;
    if sha256_lite(body) != stored_hash {
        return Err("body hash mismatch".into());
    }
    let mut frames = BTreeMap::new();
    let mut far = 0u32;
    let mut synced = false;
    let mut i = 0usize;
    while i < body.len() {
        let op = body[i];
        let len = u16::from_le_bytes(
            body.get(i + 1..i + 3)
                .ok_or_else(|| "truncated packet header".to_string())?
                .try_into()
                .unwrap(),
        ) as usize;
        let payload = body
            .get(i + 3..i + 3 + len)
            .ok_or_else(|| format!("truncated payload for packet {op:#04x}"))?;
        match op {
            0x01 => {
                if payload != b"HELI" {
                    return Err("bad SYNC word".into());
                }
                synced = true;
            }
            0x10 => {
                if len != 4 {
                    return Err("WRITE_FAR must be 4 bytes".into());
                }
                far = u32::from_le_bytes(payload.try_into().unwrap());
            }
            0x11 => {
                if !synced {
                    return Err("WRITE_FDRI before SYNC".into());
                }
                if len % 16 != 0 {
                    return Err(format!("WRITE_FDRI length {len} is not a whole frame"));
                }
                for chunk in payload.chunks(16) {
                    let f = Far::decode(far);
                    frames.insert(
                        (f.block_type, f.major, f.minor),
                        u128::from_le_bytes(chunk.try_into().unwrap()),
                    );
                    far += 1;
                }
            }
            0x12 => {
                // MFWR (Multi-Frame Write compressed packet):
                // 16-byte template word + N * (u32 start_far, u16 run_len) entries
                if !synced {
                    return Err("MFWR before SYNC".into());
                }
                if len < 22 || (len - 16) % 6 != 0 {
                    return Err(format!("bad MFWR payload length {len}"));
                }
                let word = u128::from_le_bytes(payload[0..16].try_into().unwrap());
                for entry in payload[16..].chunks_exact(6) {
                    let start_far = u32::from_le_bytes(entry[0..4].try_into().unwrap());
                    let count = u16::from_le_bytes(entry[4..6].try_into().unwrap()) as u32;
                    for off in 0..count {
                        let f = Far::decode(start_far + off);
                        frames.insert((f.block_type, f.major, f.minor), word);
                    }
                }
            }
            0x21 => {}
            0x02 => synced = false,
            other => return Err(format!("unknown packet {other:#04x}")),
        }
        i += 3 + len;
    }
    Ok((idcode, frames))
}

fn flush_run(body: &mut Vec<u8>, far: u32, run: &[u128]) {
    body.push(0x10); // WRITE_FAR
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&far.to_le_bytes());
    let nbytes = run.len() * 16;
    debug_assert!(
        nbytes <= u16::MAX as usize,
        "FDRI run must fit the length field"
    );
    body.push(0x11); // WRITE_FDRI
    body.extend_from_slice(&(nbytes as u16).to_le_bytes());
    let start = body.len();
    body.resize(start + nbytes, 0);
    for (i, w) in run.iter().enumerate() {
        let off = start + i * 16;
        body[off..off + 16].copy_from_slice(&w.to_le_bytes());
    }
}

/// Reflected CRC-32C (Castagnoli).
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Read LUT INIT back from programmed frames via the HAD FeatureMap.
pub fn readback_lut_init(
    dev: &Device,
    bits: &Bitstream,
    x: u32,
    y: u32,
    ble: u32,
) -> Result<u64, String> {
    let mut init = 0u64;
    for i in 0..64u32 {
        let loc = dev.locate(&format!("CLB_X{x}Y{y}.BLE{ble}.LUT.INIT[{i}]"))?;
        let frame = bits
            .frames
            .get(&(loc.far.block_type, loc.far.major, loc.far.minor))
            .copied()
            .unwrap_or(0);
        if (frame >> loc.bit) & 1 == 1 {
            init |= 1u64 << i;
        }
    }
    Ok(init)
}

/// ECO: change one LUT INIT and rebuild the bitstream (other sites unchanged in intent).
pub fn eco_lut(
    dev: &Device,
    routed: &Routed,
    cell: &str,
    new_init: u64,
) -> Result<Bitstream, String> {
    let mut r = routed.clone();
    let i = r
        .placed
        .packed
        .lutffs
        .iter()
        .position(|l| l.lut_cell == cell || l.ff_cell == cell)
        .ok_or_else(|| format!("eco: no LUT/FF cell {cell}"))?;
    r.placed.packed.lutffs[i].init = new_init;
    bitgen(dev, &r)
}

/// Partial bitstream: only CLB majors (and matching IOB columns) in `sites`.
pub fn bitgen_pblock(
    dev: &Device,
    routed: &Routed,
    sites: &[(u32, u32)],
) -> Result<Bitstream, String> {
    let full = bitgen(dev, routed)?;
    let majors: std::collections::HashSet<u16> = sites
        .iter()
        .filter_map(|(x, y)| dev.clb_major(*x, *y))
        .collect();
    let iob_x: std::collections::HashSet<u32> = sites.iter().map(|(x, _)| *x).collect();
    let mut frames = BTreeMap::new();
    for ((b, maj, min), w) in &full.frames {
        let keep = if *b == Far::CLB_IO_CLK {
            majors.contains(maj)
        } else if *b == Far::IOB {
            iob_x.iter().any(|x| dev.iob_major(*x, 0) == Some(*maj))
        } else {
            false
        };
        if keep && *w != 0 {
            frames.insert((*b, *maj, *min), *w);
        }
    }
    if frames.is_empty() {
        return Err(format!(
            "{ERR_CODE_EMPTY_BITSTREAM}: pblock produced no frames"
        ));
    }
    let mut bs = Bitstream {
        idcode: dev.idcode,
        frames,
        packets: Vec::new(),
    };
    bs.packets = encode_packets(dev.idcode, &bs.frames);
    Ok(bs)
}

/// Header flag bit 0 in `.hbits` when `BITSTREAM.GENERAL.COMPRESS TRUE` (`MFWR`) is active.
pub const HBITS_FLAG_COMPRESSED: u32 = 1 << 0;

/// Vivado UG908 Readback Data (`.rbd`) golden frame image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadbackData {
    pub idcode: u32,
    pub frames: BTreeMap<(u8, u16, u8), u128>,
}

impl ReadbackData {
    /// Serialize `.rbd` into deterministic UG908-style ASCII hex lines.
    pub fn to_rbd_text(&self) -> String {
        let mut out = format!("# Helion UG908 Readback Data (.rbd)\nIDCODE {:#010x}\n", self.idcode);
        for (&(b, maj, min), &w) in &self.frames {
            let far = Far {
                block_type: b,
                die: 0,
                major: maj,
                minor: min,
            }
            .encode();
            out.push_str(&format!("FAR {far:#010x} WORD {w:032x}\n"));
        }
        out
    }

    /// Parse `.rbd` ASCII text back into [`ReadbackData`].
    pub fn from_rbd_text(text: &str) -> Result<Self, String> {
        let mut idcode = 0u32;
        let mut saw_idcode = false;
        let mut frames = BTreeMap::new();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut toks = line.split_whitespace();
            match toks.next() {
                Some("IDCODE") => {
                    let s = toks.next().ok_or("rbd: missing IDCODE value")?;
                    idcode = u32::from_str_radix(s.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("rbd IDCODE: {e}"))?;
                    saw_idcode = true;
                }
                Some("FAR") => {
                    let far_s = toks.next().ok_or("rbd: missing FAR")?;
                    let _word_kw = toks.next().ok_or("rbd: missing WORD keyword")?;
                    let word_s = toks.next().ok_or("rbd: missing WORD hex")?;
                    let far_u32 = u32::from_str_radix(far_s.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("rbd FAR: {e}"))?;
                    let w = u128::from_str_radix(word_s.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("rbd WORD: {e}"))?;
                    let f = Far::decode(far_u32);
                    frames.insert((f.block_type, f.major, f.minor), w);
                }
                other => return Err(format!("rbd: unknown directive {other:?}")),
            }
        }
        if !saw_idcode {
            return Err("rbd: missing IDCODE header".into());
        }
        Ok(Self { idcode, frames })
    }
}

/// Vivado UG908 Mask Data (`.msd`) dynamic-state bitmask per frame.
///
/// A `1` bit in `masks[&(block, major, minor)]` marks a dynamic state bit (FF state,
/// LUTRAM/SRL contents, or runtime BRAM data) that must be ignored during readback verification.
/// A `0` bit marks a static configuration bit (LUT INIT, FF.USED, IMUX PIP, IOB, DSP) that must match `.rbd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskData {
    pub idcode: u32,
    pub masks: BTreeMap<(u8, u16, u8), u128>,
}

impl MaskData {
    /// Serialize `.msd` into deterministic UG908-style ASCII hex lines.
    pub fn to_msd_text(&self) -> String {
        let mut out = format!("# Helion UG908 Mask Data (.msd)\nIDCODE {:#010x}\n", self.idcode);
        for (&(b, maj, min), &m) in &self.masks {
            let far = Far {
                block_type: b,
                die: 0,
                major: maj,
                minor: min,
            }
            .encode();
            out.push_str(&format!("FAR {far:#010x} MASK {m:032x}\n"));
        }
        out
    }

    /// Parse `.msd` ASCII text back into [`MaskData`].
    pub fn from_msd_text(text: &str) -> Result<Self, String> {
        let mut idcode = 0u32;
        let mut saw_idcode = false;
        let mut masks = BTreeMap::new();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut toks = line.split_whitespace();
            match toks.next() {
                Some("IDCODE") => {
                    let s = toks.next().ok_or("msd: missing IDCODE value")?;
                    idcode = u32::from_str_radix(s.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("msd IDCODE: {e}"))?;
                    saw_idcode = true;
                }
                Some("FAR") => {
                    let far_s = toks.next().ok_or("msd: missing FAR")?;
                    let _mask_kw = toks.next().ok_or("msd: missing MASK keyword")?;
                    let mask_s = toks.next().ok_or("msd: missing MASK hex")?;
                    let far_u32 = u32::from_str_radix(far_s.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("msd FAR: {e}"))?;
                    let m = u128::from_str_radix(mask_s.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("msd MASK: {e}"))?;
                    let f = Far::decode(far_u32);
                    masks.insert((f.block_type, f.major, f.minor), m);
                }
                other => return Err(format!("msd: unknown directive {other:?}")),
            }
        }
        if !saw_idcode {
            return Err("msd: missing IDCODE header".into());
        }
        Ok(Self { idcode, masks })
    }
}

/// One frame mismatch reported by [`verify_readback_masked`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadbackMismatch {
    pub block_type: u8,
    pub major: u16,
    pub minor: u8,
    pub expected: u128,
    pub actual: u128,
    pub mask: u128,
    pub failing_bits: u128,
}

/// Result of [`verify_readback_masked`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadbackVerifyReport {
    pub ok: bool,
    pub checked_frames: usize,
    pub masked_dynamic_bits: usize,
    pub mismatches: Vec<ReadbackMismatch>,
}

/// Generate a Vivado UG908 `.rbd` Readback Data image from a programmed [`Bitstream`].
pub fn generate_rbd(bits: &Bitstream) -> ReadbackData {
    ReadbackData {
        idcode: bits.idcode,
        frames: bits.frames.clone(),
    }
}

/// Generate a Vivado UG908 `.msd` Mask Data image from `bits` and optional `lutram_bles` (`(x, y, ble)`).
///
/// Masks out:
/// - `BLE0..7.FF.INIT` and `BLE0..7.FF.SRVAL` dynamic flip-flop capture bits in every configured CLB major.
/// - `BLE{ble}.LUT.INIT[0..63]` for any `(x, y, ble)` in `lutram_bles` (distributed RAM / SRL).
/// - `Far::BRAM` data frames (`minor >= 1`), while leaving `Far::BRAM` minor `0` (`USED` + site coords) unmasked.
pub fn generate_msd(
    dev: &Device,
    bits: &Bitstream,
    lutram_bles: &[(u32, u32, u8)],
) -> Result<MaskData, String> {
    let fm = dev.featuremap();
    let fb = fm.frame_bits;
    let mut clb_ff_mask_by_minor: BTreeMap<u8, u128> = BTreeMap::new();
    for ble in 0..dev.n_ble {
        for suffix in ["FF.INIT", "FF.SRVAL"] {
            let key = format!("BLE{ble}.{suffix}");
            if let Some(abs) = fm.abs_bit(&key) {
                let minor = (abs / fb) as u8;
                let bit = abs % fb;
                *clb_ff_mask_by_minor.entry(minor).or_insert(0) |= 1u128 << bit;
            }
        }
    }

    let mut clb_majors: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
    for &(b, maj, _) in bits.frames.keys() {
        if b == Far::CLB_IO_CLK {
            clb_majors.insert(maj);
        }
    }
    let mut masks: BTreeMap<(u8, u16, u8), u128> = BTreeMap::new();
    for maj in clb_majors {
        for (&minor, &m) in &clb_ff_mask_by_minor {
            if m != 0 {
                *masks.entry((Far::CLB_IO_CLK, maj, minor)).or_insert(0) |= m;
            }
        }
    }
    for &(x, y, ble) in lutram_bles {
        let major = dev
            .clb_major(x, y)
            .ok_or_else(|| format!("generate_msd: CLB_X{x}Y{y} is not a CLB site"))?;
        let abs0 = fm
            .abs_bit(&format!("BLE{ble}.LUT.INIT[0]"))
            .ok_or_else(|| format!("generate_msd: unknown BLE{ble}.LUT.INIT[0]"))?;
        let minor = (abs0 / fb) as u8;
        let bit0 = abs0 % fb;
        *masks.entry((Far::CLB_IO_CLK, major, minor)).or_insert(0) |= (u64::MAX as u128) << bit0;
    }
    for &(b, maj, min) in bits.frames.keys() {
        if b == Far::BRAM && min >= 1 {
            masks.insert((Far::BRAM, maj, min), u128::MAX);
        }
    }
    Ok(MaskData {
        idcode: bits.idcode,
        masks,
    })
}

/// Compare hardware readback frames against golden `.rbd` while masking out dynamic `.msd` bits.
pub fn verify_readback_masked(
    rbd: &ReadbackData,
    msd: &MaskData,
    hw_readback_frames: &BTreeMap<(u8, u16, u8), u128>,
) -> ReadbackVerifyReport {
    let mut all_keys: std::collections::BTreeSet<(u8, u16, u8)> = std::collections::BTreeSet::new();
    for &k in rbd.frames.keys().chain(hw_readback_frames.keys()) {
        all_keys.insert(k);
    }
    let mut mismatches = Vec::new();
    let mut masked_dynamic_bits = 0usize;
    for key in &all_keys {
        let exp = rbd.frames.get(key).copied().unwrap_or(0);
        let act = hw_readback_frames.get(key).copied().unwrap_or(0);
        let mask = msd.masks.get(key).copied().unwrap_or(0);
        masked_dynamic_bits += mask.count_ones() as usize;
        let diff = (exp ^ act) & !mask;
        if diff != 0 {
            mismatches.push(ReadbackMismatch {
                block_type: key.0,
                major: key.1,
                minor: key.2,
                expected: exp,
                actual: act,
                mask,
                failing_bits: diff,
            });
        }
    }
    ReadbackVerifyReport {
        ok: mismatches.is_empty(),
        checked_frames: all_keys.len(),
        masked_dynamic_bits,
        mismatches,
    }
}

/// 9-bit Extended Hamming SEC-DED (Single Error Correction, Double Error Detection)
/// syndrome over a 128-bit configuration frame (`u128`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameEcc {
    /// 8-bit Hamming parity over 1-based bit positions `1..=128`.
    pub syndrome: u8,
    /// Overall parity across all 128 data bits and 8 syndrome bits.
    pub overall_parity: bool,
}

impl FrameEcc {
    pub fn compute(word: u128) -> Self {
        let mut syndrome = 0u8;
        let mut data_parity = false;
        let mut rem = word;
        while rem != 0 {
            let bit = rem.trailing_zeros() as u8;
            let pos = bit + 1; // 1..=128 (fits in 8 bits: 0x01..=0x80)
            syndrome ^= pos;
            data_parity = !data_parity;
            rem &= rem - 1;
        }
        Self {
            syndrome,
            overall_parity: data_parity,
        }
    }
}

/// Outcome of SEM SEU check & scrub on a single 128-bit configuration frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeuOutcome {
    /// Zero bit flips detected.
    Clean,
    /// Single-bit SEU detected at `bit` (`0..128`) and automatically corrected in-place.
    CorrectedSingleBit { bit: u8 },
    /// Single-bit upset in the ECC parity storage itself (data word intact).
    CorrectedParityBit,
    /// Double-bit (or even multi-bit) uncorrectable SEU detected (`syndrome != 0`, even parity).
    UncorrectableDoubleBit { syndrome: u8 },
}

/// Summary report returned by [`SemScrubber::scrub_bitstream`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrubReport {
    pub frames_scanned: usize,
    pub corrected_single_bits: Vec<((u8, u16, u8), u8)>,
    pub uncorrectable_double_bits: Vec<((u8, u16, u8), u8)>,
}

impl ScrubReport {
    pub fn is_clean_or_corrected(&self) -> bool {
        self.uncorrectable_double_bits.is_empty()
    }
}

/// Soft Error Mitigation (SEM) SEU Frame Scrubber using Extended Hamming SEC-DED ECC.
#[derive(Clone, Debug)]
pub struct SemScrubber {
    pub golden_ecc: BTreeMap<(u8, u16, u8), FrameEcc>,
    pub corrected_single_bit_count: u64,
    pub uncorrectable_double_bit_count: u64,
}

impl SemScrubber {
    /// Build golden SEC-DED frame syndromes for every configured frame in `bits`.
    pub fn from_bitstream(bits: &Bitstream) -> Self {
        let golden_ecc = bits
            .frames
            .iter()
            .map(|(&k, &w)| (k, FrameEcc::compute(w)))
            .collect();
        Self {
            golden_ecc,
            corrected_single_bit_count: 0,
            uncorrectable_double_bit_count: 0,
        }
    }

    /// Check one 128-bit frame against its golden SEC-DED ECC and auto-correct a single-bit flip in-place.
    pub fn check_and_scrub_frame(&mut self, key: (u8, u16, u8), word: &mut u128) -> SeuOutcome {
        let expected = self
            .golden_ecc
            .get(&key)
            .copied()
            .unwrap_or_else(|| FrameEcc::compute(0));
        let actual = FrameEcc::compute(*word);
        let syn = expected.syndrome ^ actual.syndrome;
        let p_err = expected.overall_parity ^ actual.overall_parity;

        if syn == 0 && !p_err {
            return SeuOutcome::Clean;
        }
        if p_err && (1..=128).contains(&syn) {
            let bit = syn - 1;
            *word ^= 1u128 << bit;
            self.corrected_single_bit_count += 1;
            return SeuOutcome::CorrectedSingleBit { bit };
        }
        if p_err && syn == 0 {
            return SeuOutcome::CorrectedParityBit;
        }
        self.uncorrectable_double_bit_count += 1;
        SeuOutcome::UncorrectableDoubleBit { syndrome: syn }
    }

    /// Scan and scrub all frames in `bits`, correcting any single-bit SEUs and reporting double-bit alarms.
    pub fn scrub_bitstream(&mut self, bits: &mut Bitstream) -> ScrubReport {
        let mut all_keys: std::collections::BTreeSet<(u8, u16, u8)> =
            std::collections::BTreeSet::new();
        for &k in self.golden_ecc.keys().chain(bits.frames.keys()) {
            all_keys.insert(k);
        }
        let mut corrected_single_bits = Vec::new();
        let mut uncorrectable_double_bits = Vec::new();
        let mut modified = false;

        for key in &all_keys {
            let mut w = bits.frames.get(key).copied().unwrap_or(0);
            match self.check_and_scrub_frame(*key, &mut w) {
                SeuOutcome::Clean | SeuOutcome::CorrectedParityBit => {}
                SeuOutcome::CorrectedSingleBit { bit } => {
                    if w == 0 {
                        bits.frames.remove(key);
                    } else {
                        bits.frames.insert(*key, w);
                    }
                    corrected_single_bits.push((*key, bit));
                    modified = true;
                }
                SeuOutcome::UncorrectableDoubleBit { syndrome } => {
                    uncorrectable_double_bits.push((*key, syndrome));
                }
            }
        }
        if modified {
            bits.packets = encode_packets(bits.idcode, &bits.frames);
        }
        ScrubReport {
            frames_scanned: all_keys.len(),
            corrected_single_bits,
            uncorrectable_double_bits,
        }
    }
}

/// Compression telemetry for `BITSTREAM.GENERAL.COMPRESS TRUE` (`MFWR` deduplication).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompressionStats {
    pub uncompressed_bytes: usize,
    pub compressed_bytes: usize,
    pub total_frames: usize,
    pub unique_frames: usize,
    pub mfwr_groups: usize,
}

/// Compress a [`Bitstream`] using Vivado-class `BITSTREAM.GENERAL.COMPRESS TRUE` (`MFWR` op `0x12`).
///
/// Deduplicates identical 128-bit frame words across the die into compact `MFWR` multi-frame
/// write packets when profitable, and sets `HBITS_FLAG_COMPRESSED` in the `.hbits` header.
pub fn compress_bitstream_with_stats(bits: &Bitstream) -> (Bitstream, CompressionStats) {
    let uncompressed_packets = encode_packets(bits.idcode, &bits.frames);
    let uncompressed_bytes = uncompressed_packets.len();

    // Group non-zero frames by their 128-bit payload value into contiguous (start_far, count) runs.
    let mut by_word: BTreeMap<u128, Vec<(u32, u16)>> = BTreeMap::new();
    let mut total_frames = 0usize;
    for (&(block, major, minor), &payload) in &bits.frames {
        if payload == 0 {
            continue;
        }
        total_frames += 1;
        let far = Far {
            block_type: block,
            die: 0,
            major,
            minor,
        }
        .encode();
        let runs = by_word.entry(payload).or_default();
        if let Some((start, count)) = runs.last_mut() {
            if *start + u32::from(*count) == far && *count < u16::MAX {
                *count += 1;
                continue;
            }
        }
        runs.push((far, 1));
    }

    let unique_frames = by_word.len();
    let mut out = Vec::with_capacity(HBITS_HEADER_BYTES + uncompressed_bytes);
    out.resize(HBITS_HEADER_BYTES, 0);
    // SYNC
    out.push(0x01);
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(b"HELI");

    let mut mfwr_groups = 0usize;
    let mut single_frames: BTreeMap<(u8, u16, u8), u128> = BTreeMap::new();

    for (word, runs) in &by_word {
        let total_for_word: usize = runs.iter().map(|(_, c)| *c as usize).sum();
        if total_for_word >= 2 {
            // Emit MFWR (0x12): 16-byte template word + 6 bytes per (start_far, count) run
            mfwr_groups += 1;
            let payload_len = 16 + runs.len() * 6;
            out.push(0x12);
            out.extend_from_slice(&(payload_len as u16).to_le_bytes());
            out.extend_from_slice(&word.to_le_bytes());
            for &(start_far, count) in runs {
                out.extend_from_slice(&start_far.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
            }
        } else if let Some(&(far, _)) = runs.first() {
            let f = Far::decode(far);
            single_frames.insert((f.block_type, f.major, f.minor), *word);
        }
    }

    // Flush remaining unique singletons as standard contiguous WRITE_FAR + WRITE_FDRI runs
    let mut run_start: Option<u32> = None;
    let mut last_far: Option<u32> = None;
    let mut run: Vec<u128> = Vec::new();
    for ((block, major, minor), payload) in &single_frames {
        let far = Far {
            block_type: *block,
            die: 0,
            major: *major,
            minor: *minor,
        }
        .encode();
        let contiguous = last_far.map(|prev| far == prev + 1).unwrap_or(false);
        if !contiguous || run.len() >= MAX_RUN_FRAMES {
            if let Some(start) = run_start {
                if !run.is_empty() {
                    flush_run(&mut out, start, &run);
                }
            }
            run.clear();
            run_start = Some(far);
        }
        run.push(*payload);
        last_far = Some(far);
    }
    if let Some(start) = run_start {
        if !run.is_empty() {
            flush_run(&mut out, start, &run);
        }
    }

    // CRC_CHECK
    out.push(0x21);
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    // DESYNC
    out.push(0x02);
    out.extend_from_slice(&0u16.to_le_bytes());

    let body_len = (out.len() - HBITS_HEADER_BYTES) as u64;
    out[0..4].copy_from_slice(b"HBIT");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6..10].copy_from_slice(&bits.idcode.to_le_bytes());
    out[10..14].copy_from_slice(&HBITS_FLAG_COMPRESSED.to_le_bytes());
    out[14..22].copy_from_slice(&body_len.to_le_bytes());
    let hash = sha256_lite(&out[HBITS_HEADER_BYTES..]);
    out[22..54].copy_from_slice(&hash);
    let hdr_crc = crc32c(&out[..54]);
    out[54..58].copy_from_slice(&hdr_crc.to_le_bytes());

    let compressed_bytes = out.len();
    let stats = CompressionStats {
        uncompressed_bytes,
        compressed_bytes,
        total_frames,
        unique_frames,
        mfwr_groups,
    };
    (
        Bitstream {
            idcode: bits.idcode,
            frames: bits.frames.clone(),
            packets: out,
        },
        stats,
    )
}

/// Compress a [`Bitstream`] with `BITSTREAM.GENERAL.COMPRESS TRUE` (`MFWR` multi-frame write).
pub fn compress_bitstream(bits: &Bitstream) -> Bitstream {
    compress_bitstream_with_stats(bits).0
}

/// Decompress a compressed (or standard) `.hbits` [`Bitstream`] back into canonical uncompressed `.hbits` packets.
pub fn decompress_bitstream(bits: &Bitstream) -> Result<Bitstream, String> {
    let (idcode, frames) = decode_packets(&bits.packets)?;
    let packets = encode_packets(idcode, &frames);
    Ok(Bitstream {
        idcode,
        frames,
        packets,
    })
}

fn sha256_lite(data: &[u8]) -> [u8; 32] {
    // 0.1 header hash: CRC32C repeated; not cryptographic. Field is 32 bytes.
    let c = crc32c(data);
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design, PortDir};
    use helion_pack::pack;
    use helion_place::place;
    use helion_route::route;

    #[test]
    fn assemble_sets_init0() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut f = FeatureSet::new();
        f.set("CLB_X2Y1.BLE0.LUT.INIT[0]", true);
        let bs = assemble(&dev, &f).unwrap();
        let loc = dev.locate("CLB_X2Y1.BLE0.LUT.INIT[0]").unwrap();
        let frame = bs.frames[&(loc.far.block_type, loc.far.major, loc.far.minor)];
        assert_eq!((frame >> loc.bit) & 1, 1);
        assert_eq!(loc.far.minor, 0);
        assert_eq!(loc.bit, 0);
    }

    #[test]
    fn bram_bitgen_is_not_a_noop() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let bs = bitgen(&dev, &r).unwrap();
        let empty = Bitstream::empty(&dev);
        assert_ne!(bs.frames, empty.frames);
        assert!(
            bs.frames.keys().any(|(b, _, _)| *b == Far::BRAM),
            "BRAM frame missing"
        );
    }

    /// Dense encoding (every frame on the die, zeros included) as shipped through
    /// commit 9b864af: 16384 CLB frames * 16 B. The sparse encoder must beat it.
    const DENSE_BYTES: usize = 272_485;

    #[test]
    fn packet_stream_is_sparse_and_round_trips() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let bs = bitgen(&dev, &r).unwrap();

        let nonzero: BTreeMap<(u8, u16, u8), u128> = bs
            .frames
            .iter()
            .filter(|(_, w)| **w != 0)
            .map(|(k, w)| (*k, *w))
            .collect();
        assert!(!nonzero.is_empty(), "counter must configure some frames");

        // Sparse: only configured frames are written.
        let (idcode, decoded) = decode_packets(&bs.packets).unwrap();
        assert_eq!(idcode, dev.idcode);
        assert_eq!(
            decoded, nonzero,
            "every configured frame must survive encode/decode and nothing else may be written"
        );
        assert!(
            bs.packets.len() < DENSE_BYTES / 100,
            "sparse .hbits must beat the dense {DENSE_BYTES} B stream by >100x, got {}",
            bs.packets.len()
        );
        // Not a no-op: the stream still carries the LUT INIT payloads.
        let (site, ble) = r.placed.lutff_sites[0];
        let major = dev.clb_major(site.x, site.y).unwrap();
        assert!(
            decoded
                .keys()
                .any(|(b, maj, _)| *b == Far::CLB_IO_CLK && *maj == major),
            "the placed CLB major must be in the stream"
        );
        assert_eq!(
            readback_lut_init(&dev, &bs, site.x, site.y, ble as u32).unwrap(),
            helion_ir::INC4_INIT[0]
        );
    }

    #[test]
    fn iob_drive_slew_pulltype_change_iob_frame() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let plain = Design::structural_counter();
        let p = pack(&plain, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let a = bitgen(&dev, &r).unwrap();
        let iob_a: Vec<_> = a
            .frames
            .iter()
            .filter(|((b, _, _), _)| *b == Far::IOB)
            .map(|(k, w)| (*k, *w))
            .collect();
        assert!(!iob_a.is_empty(), "counter must program an IOB frame");
        assert!(
            iob_a.iter().all(|(_, w)| (*w >> 16) == 0),
            "default electrical bits must be 0: {iob_a:?}"
        );

        let mut d = Design::structural_counter();
        d.set_drive("led", "4").unwrap();
        d.set_slew("led", "FAST").unwrap();
        d.set_pulltype("led", "PULLUP").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let b = bitgen(&dev, &r).unwrap();
        let iob_b: Vec<_> = b
            .frames
            .iter()
            .filter(|((blk, _, _), _)| *blk == Far::IOB)
            .map(|(k, w)| (*k, *w))
            .collect();
        assert_ne!(
            iob_a, iob_b,
            "DRIVE/SLEW/PULLTYPE must change the IOB frame"
        );
        let elec = Device::iob_electrical_bits(Some("4"), Some("FAST"), Some("PULLUP"), None, None);
        assert!(
            iob_b.iter().any(|(_, w)| (*w & elec) == elec),
            "IOB word must carry electrical bits {elec:#x}: {iob_b:?}"
        );

        let mut d = Design::structural_counter();
        d.set_diff_term("led", "TRUE").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let c = bitgen(&dev, &r).unwrap();
        let iob_c: Vec<_> = c
            .frames
            .iter()
            .filter(|((blk, _, _), _)| *blk == Far::IOB)
            .map(|(k, w)| (*k, *w))
            .collect();
        assert_ne!(iob_a, iob_c, "DIFF_TERM/IN_TERM must change the IOB frame");
        let term =
            Device::iob_electrical_bits(None, None, None, Some("TRUE"), Some("UNTUNED_SPLIT_50"));
        assert!(
            iob_c.iter().any(|(_, w)| (*w & term) == term),
            "IOB word must carry DIFF_TERM/IN_TERM bits {term:#x}: {iob_c:?}"
        );
    }

    #[test]
    fn fdri_runs_fit_the_length_field() {
        // A dense die-wide write used to overflow the 16-bit FDRI length and
        // silently truncate; runs are now chunked, so it decodes exactly.
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut frames = BTreeMap::new();
        for major in 0..dev.n_clb() as u16 {
            for minor in 0..dev.clb_minors as u8 {
                frames.insert(
                    (Far::CLB_IO_CLK, major, minor),
                    (major as u128) << 8 | minor as u128 | 1,
                );
            }
        }
        let packets = encode_packets(dev.idcode, &frames);
        let (_, back) = decode_packets(&packets).unwrap();
        assert_eq!(back.len(), frames.len(), "every frame must decode");
        assert_eq!(back, frames);
        assert!(frames.len() > MAX_RUN_FRAMES, "must exercise run chunking");
    }

    #[test]
    fn decode_rejects_corrupt_stream() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bs = Bitstream::empty(&dev);
        assert!(decode_packets(b"nope").is_err());
        let mut bad = bs.packets.clone();
        bad[0] = b'X';
        assert!(decode_packets(&bad).is_err(), "magic must be checked");
        let mut crc = bs.packets.clone();
        crc[6] ^= 0xFF;
        assert!(decode_packets(&crc).is_err(), "header CRC must be checked");
    }

    #[test]
    fn readback_and_eco_change_init() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let bs = bitgen(&dev, &r).unwrap();
        let (site, ble) = r.placed.lutff_sites[0];
        let init = readback_lut_init(&dev, &bs, site.x, site.y, ble as u32).unwrap();
        assert_eq!(init, 0x5555_5555_5555_5555);
        let eco = eco_lut(&dev, &r, "u_lut", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        let init2 = readback_lut_init(&dev, &eco, site.x, site.y, ble as u32).unwrap();
        assert_eq!(init2, 0xAAAA_AAAA_AAAA_AAAA);
        assert_ne!(bs.frames, eco.frames);
        let pb = bitgen_pblock(&dev, &r, &[(site.x, site.y)]).unwrap();
        // Sparse bitgen: blinky already fits one major, so pblock may equal full.
        // Honesty bar is subset + packets never larger than the parent stream.
        assert!(pb.frames.keys().all(|k| bs.frames.contains_key(k)));
        assert!(
            pb.packets.len() <= bs.packets.len(),
            "partial packets must not exceed full"
        );
        assert!(!pb.frames.is_empty(), "pblock must carry the placed major");
    }

    #[test]
    fn bitgen_pblock_no_sites_refuses_with_empty_code() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let err = bitgen_pblock(&dev, &r, &[]).unwrap_err();
        assert!(
            err.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable code missing: {err}"
        );
        assert!(err.contains("pblock produced no frames"), "{err}");
        assert!(!err.contains("DONE=1"), "must not invent DONE: {err}");
    }

    /// Gold counter sparse `.hbits` size (README / QoR table). Must stay stable.
    const COUNTER_HBITS_BYTES: usize = 185;

    #[test]
    fn counter_bitgen_golden_size_and_deterministic() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let a = bitgen(&dev, &r).unwrap();
        let b = bitgen(&dev, &r).unwrap();
        assert_eq!(
            a.packets.len(),
            COUNTER_HBITS_BYTES,
            "counter .hbits must stay {COUNTER_HBITS_BYTES} B (got {})",
            a.packets.len()
        );
        assert_eq!(a.frames.len(), 5, "counter configures 5 sparse frames");
        assert_eq!(a.packets, b.packets, "counter bitgen must be deterministic");
        assert_eq!(a.frames, b.frames);
        assert_ne!(
            a.packets,
            Bitstream::empty(&dev).packets,
            "counter must not be an empty/fake stream"
        );
        let (idcode, decoded) = decode_packets(&a.packets).unwrap();
        assert_eq!(idcode, dev.idcode);
        assert_eq!(decoded.len(), 5);
        // Header integrity: CRC + body hash already enforced by decode_packets.
        assert!(a.packets.len() > HBITS_HEADER_BYTES);
    }

    #[test]
    fn bitgen_clb_frames_match_featureset_assemble() {
        // Frame buffering must be encode-equivalent to the FeatureSet locate path.
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let bs = bitgen(&dev, &r).unwrap();
        let mut feats = FeatureSet::new();
        for (i, lutff) in r.placed.packed.lutffs.iter().enumerate() {
            let (site, ble) = r.placed.lutff_sites[i];
            feats.set_init(site.x, site.y, ble as u32, lutff.init);
            feats.set_ff_used(site.x, site.y, ble as u32, !lutff.ff_cell.is_empty());
        }
        for m in &r.imux {
            feats.set_imux(m.x, m.y, m.mux, m.sel);
        }
        let assembled = assemble(&dev, &feats).unwrap();
        for (k, w) in &assembled.frames {
            assert_eq!(
                bs.frames.get(k),
                Some(w),
                "CLB frame {k:?} must match FeatureSet assemble"
            );
        }
        let (idcode, decoded) = decode_packets(&bs.packets).unwrap();
        assert_eq!(idcode, dev.idcode);
        assert_eq!(decoded, bs.frames);
    }

    #[test]
    fn bitgen_refuses_empty_design() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let empty = Routed {
            placed: helion_place::Placed {
                packed: helion_pack::Packed {
                    lutffs: vec![],
                    iobs: vec![],
                    macs: vec![],
                    brams: vec![],
                },
                lutff_sites: vec![],
                iob_sites: vec![],
                mac_sites: vec![],
                bram_sites: vec![],
                timing_weight: 0.0,
                cost: 0.0,
            },
            iob_src: vec![],
            imux: vec![],
            pathfinder_iters: 0,
            overused: 0,
            imux_skip: 0,
            congestion: vec![],
            congestion_x0: 0,
            congestion_y0: 0,
            congestion_cols: 0,
            congestion_rows: 0,
        };
        let err = bitgen(&dev, &empty).unwrap_err();
        assert!(
            err.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable code missing: {err}"
        );
        assert!(
            err.contains("empty design") || err.contains("refusing"),
            "{err}"
        );
        assert!(!err.contains("DONE=1"), "must not invent DONE: {err}");
        assert!(!err.contains("DONE=1 / TAP STAT"), "{err}");
    }

    #[test]
    fn bitgen_refuses_all_zero_init_with_no_iob() {
        // Soft path: LUT INIT=0 + no FF.USED + no IOB → assemble writes nothing.
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("zero_only");
        d.add_port("clk", PortDir::In);
        d.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        d.add_cell("u_ff", CellKind::Hff);
        // Leave ff unpackable (no D/Q nets) so pack may still produce a LUTFF
        // with empty ff_cell — or pack a lone LUT. Either way INIT=0 sets no bits.
        let p = pack(&d, &dev).unwrap();
        if p.lutffs.is_empty() && p.iobs.is_empty() {
            // Pack dropped everything — bitgen empty-design path.
            let pl = place(&p, &dev).unwrap();
            let r = route(&pl, &dev).unwrap();
            let err = bitgen(&dev, &r).unwrap_err();
            assert!(
                err.contains(ERR_CODE_EMPTY_BITSTREAM),
                "stable code missing: {err}"
            );
            assert!(err.contains("refusing") || err.contains("empty"), "{err}");
            assert!(!err.contains("DONE=1"), "must not invent DONE: {err}");
            return;
        }
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        // Force INIT=0 and strip IOB routes so frames stay empty if soft.
        let mut r = r;
        for l in &mut r.placed.packed.lutffs {
            l.init = 0;
            l.ff_cell.clear();
        }
        r.iob_src.clear();
        r.imux.clear();
        r.placed.packed.iobs.clear();
        let err = bitgen(&dev, &r).unwrap_err();
        assert!(
            err.contains(ERR_CODE_EMPTY_BITSTREAM),
            "stable code missing: {err}"
        );
        assert!(
            err.contains("no configured frames")
                || err.contains("refusing")
                || err.contains("empty"),
            "{err}"
        );
        assert!(!err.contains("DONE=1"), "must not invent DONE: {err}");
    }

    #[test]
    fn ug908_rbd_msd_generation_and_masked_readback_verification() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let mut bs = bitgen(&dev, &r).unwrap();
        bs.frames.insert((Far::BRAM, 0, 1), 0x1111_2222_3333_4444u128);

        let (site0, _) = r.placed.lutff_sites[0];
        // Treat BLE7 at site0 as LUTRAM so its 64-bit LUT INIT is also masked in .msd.
        let rbd = generate_rbd(&bs);
        let msd = generate_msd(&dev, &bs, &[(site0.x, site0.y, 7)]).unwrap();

        // Round-trip .rbd and .msd text representations
        let rbd_rt = ReadbackData::from_rbd_text(&rbd.to_rbd_text()).unwrap();
        let msd_rt = MaskData::from_msd_text(&msd.to_msd_text()).unwrap();
        assert_eq!(rbd_rt, rbd);
        assert_eq!(msd_rt, msd);

        // Flipping dynamic FF.INIT, LUTRAM BLE7.LUT.INIT, and BRAM runtime word (minor 1)
        // must be masked by .msd and pass verify_readback_masked!
        let mut hw_frames = bs.frames.clone();
        let ff_loc = dev
            .locate(&format!("CLB_X{}Y{}.BLE0.FF.INIT", site0.x, site0.y))
            .unwrap();
        *hw_frames
            .entry((ff_loc.far.block_type, ff_loc.far.major, ff_loc.far.minor))
            .or_insert(0) ^= 1u128 << ff_loc.bit;
        let lutram_loc = dev
            .locate(&format!("CLB_X{}Y{}.BLE7.LUT.INIT[13]", site0.x, site0.y))
            .unwrap();
        *hw_frames
            .entry((
                lutram_loc.far.block_type,
                lutram_loc.far.major,
                lutram_loc.far.minor,
            ))
            .or_insert(0) ^= 1u128 << lutram_loc.bit;
        hw_frames.insert((Far::BRAM, 0, 1), 0xDEAD_BEEF_CAFE_F00Du128);

        let rep_ok = verify_readback_masked(&rbd_rt, &msd_rt, &hw_frames);
        assert!(
            rep_ok.ok,
            "dynamic FF/LUTRAM/BRAM state changes must be masked out: {:?}",
            rep_ok.mismatches
        );
        assert!(rep_ok.masked_dynamic_bits > 0);

        // Flipping a static LUT0 INIT bit or IMUX routing PIP bit must fail readback verify!
        let lut0_loc = dev
            .locate(&format!("CLB_X{}Y{}.BLE0.LUT.INIT[0]", site0.x, site0.y))
            .unwrap();
        *hw_frames
            .entry((lut0_loc.far.block_type, lut0_loc.far.major, lut0_loc.far.minor))
            .or_insert(0) ^= 1u128 << lut0_loc.bit;
        let rep_fail = verify_readback_masked(&rbd_rt, &msd_rt, &hw_frames);
        assert!(!rep_fail.ok, "corrupted static LUT bit must fail verification");
        assert_eq!(rep_fail.mismatches.len(), 1);
        assert_eq!(rep_fail.mismatches[0].failing_bits, 1u128 << lut0_loc.bit);
    }

    #[test]
    fn sem_scrubber_sec_ded_corrects_all_128_single_bit_seus_and_flags_double_bit() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let gold = bitgen(&dev, &r).unwrap();
        let mut scrubber = SemScrubber::from_bitstream(&gold);

        let (&sample_key, &sample_word) = gold.frames.iter().next().unwrap();
        // Exhaustively verify single-bit SEU correction across all 128 bit positions (0..128)
        for bit in 0..128u8 {
            let mut corrupted = sample_word ^ (1u128 << bit);
            let outcome = scrubber.check_and_scrub_frame(sample_key, &mut corrupted);
            assert_eq!(outcome, SeuOutcome::CorrectedSingleBit { bit });
            assert_eq!(
                corrupted, sample_word,
                "bit {bit} must be auto-corrected to exact golden frame word"
            );
        }
        assert_eq!(scrubber.corrected_single_bit_count, 128);

        // Verify double-bit upsets across distinct bit pairs are detected as UncorrectableDoubleBit
        for (b0, b1) in [(0u8, 1u8), (3, 67), (15, 127), (63, 64)] {
            let mut corrupted2 = sample_word ^ (1u128 << b0) ^ (1u128 << b1);
            let outcome = scrubber.check_and_scrub_frame(sample_key, &mut corrupted2);
            assert!(
                matches!(outcome, SeuOutcome::UncorrectableDoubleBit { .. }),
                "double-bit flip ({b0}, {b1}) must trigger UncorrectableDoubleBit, got {outcome:?}"
            );
        }

        // Full-bitstream scrub test: inject 1-bit SEU in one frame, verify scrub_bitstream restores it
        let mut live_bs = gold.clone();
        *live_bs.frames.get_mut(&sample_key).unwrap() ^= 1u128 << 42;
        let report = scrubber.scrub_bitstream(&mut live_bs);
        assert!(report.is_clean_or_corrected());
        assert_eq!(report.corrected_single_bits, vec![(sample_key, 42)]);
        assert_eq!(live_bs.frames, gold.frames);
        assert_eq!(live_bs.packets, gold.packets);
    }

    #[test]
    fn mfwr_bitstream_compression_reduces_size_and_roundtrips_losslessly() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut frames = BTreeMap::new();
        // Populate 64 CLB majors with repeated template words (typical replicated logic / LUTs)
        for major in 0..64u16 {
            frames.insert((Far::CLB_IO_CLK, major, 0), 0x5555_5555_5555_5555_AAAA_AAAA_AAAA_AAAAu128);
            frames.insert((Far::CLB_IO_CLK, major, 4), 0x0000_0000_0000_0001_0000_0000_0000_0101u128);
        }
        let uncomp_packets = encode_packets(dev.idcode, &frames);
        let uncomp = Bitstream {
            idcode: dev.idcode,
            frames: frames.clone(),
            packets: uncomp_packets,
        };

        let (comp, stats) = compress_bitstream_with_stats(&uncomp);
        assert_eq!(stats.total_frames, 128);
        assert_eq!(stats.unique_frames, 2);
        assert_eq!(stats.mfwr_groups, 2);
        assert!(
            comp.packets.len() < uncomp.packets.len() / 2,
            "MFWR compressed stream ({} B) must be >2x smaller than uncompressed ({} B)",
            comp.packets.len(),
            uncomp.packets.len()
        );
        let flags = u32::from_le_bytes(comp.packets[10..14].try_into().unwrap());
        assert_eq!(flags & HBITS_FLAG_COMPRESSED, HBITS_FLAG_COMPRESSED);

        // decode_packets and Bitstream::from_packets must transparently decode MFWR packets!
        let from_comp = Bitstream::from_packets(&comp.packets).unwrap();
        assert_eq!(from_comp.idcode, dev.idcode);
        assert_eq!(from_comp.frames, uncomp.frames);

        let decomp = decompress_bitstream(&comp).unwrap();
        assert_eq!(decomp.frames, uncomp.frames);
        assert_eq!(decomp.packets, uncomp.packets);
    }
}
