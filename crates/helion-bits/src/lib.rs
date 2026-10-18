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

/// `.pbits` header magic (`PBIT`), version, idcode, flags, pblock bounds, range count, body len, hash, CRC.
pub const PBITS_HEADER_BYTES: usize = 4 + 2 + 4 + 4 + 16 + 4 + 8 + 32 + 4;

/// Flag bit 0 in `.pbits` header: `RESET_AFTER_RECONFIG` (UG909).
pub const PBITS_FLAG_RESET_AFTER_RECONFIG: u32 = 1 << 0;
/// Flag bit 1 in `.pbits` header: explicit `Pblock` bounds present.
pub const PBITS_FLAG_HAS_PBLOCK: u32 = 1 << 1;

/// Vivado UG909 rectangular frame-aligned Reconfigurable Partition floorplan (`Pblock`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pblock {
    pub name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// UG909 `RESET_AFTER_RECONFIG`: pulse GSR locally inside this Pblock after partial load.
    pub reset_after_reconfig: bool,
    /// Strict exclusive RM floorplan: static cells may not be placed inside `x0..=x1, y0..=y1`.
    pub strict: bool,
    /// Bitmask of minor frames covered per CLB tile major (default `0xFFFF` = all 16 minors).
    /// A partial mask (`!= (1 << clb_minors) - 1`) violates tile-column frame alignment (`HDPR-2`).
    pub minor_mask: u16,
}

impl Pblock {
    pub fn new(name: impl Into<String>, x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Self {
            name: name.into(),
            x0,
            y0,
            x1,
            y1,
            reset_after_reconfig: true,
            strict: true,
            minor_mask: 0xFFFF,
        }
    }

    pub fn with_reset_after_reconfig(mut self, enabled: bool) -> Self {
        self.reset_after_reconfig = enabled;
        self
    }

    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    pub fn with_minor_mask(mut self, mask: u16) -> Self {
        self.minor_mask = mask;
        self
    }

    #[inline]
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// True when `(x, y)` lies on the 1-tile perimeter ring of the `Pblock` (legal `PARTPIN` interface tile).
    #[inline]
    pub fn is_boundary_tile(&self, x: u32, y: u32) -> bool {
        if self.contains(x, y) {
            return x == self.x0 || x == self.x1 || y == self.y0 || y == self.y1;
        }
        let dx = if x < self.x0 {
            self.x0 - x
        } else if x > self.x1 {
            x - self.x1
        } else {
            0
        };
        let dy = if y < self.y0 {
            self.y0 - y
        } else if y > self.y1 {
            y - self.y1
        } else {
            0
        };
        dx + dy == 1
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.x0 <= other.x1 && self.x1 >= other.x0 && self.y0 <= other.y1 && self.y1 >= other.y0
    }

    pub fn is_within_device(&self, dev: &Device) -> bool {
        if self.x0 > self.x1 || self.y0 > self.y1 {
            return false;
        }
        let max_x = dev.clb_x0 + dev.interior_cols.saturating_sub(1);
        let max_y = dev.clb_y0 + dev.interior_rows.saturating_sub(1);
        self.x0 >= dev.clb_x0 && self.x1 <= max_x && self.y0 >= dev.clb_y0 && self.y1 <= max_y
    }

    /// Check UG909 tile-column frame alignment and `RESET_AFTER_RECONFIG` rules.
    pub fn is_frame_aligned(&self, dev: &Device) -> bool {
        if !self.is_within_device(dev) {
            return false;
        }
        let full_mask = if dev.clb_minors >= 16 {
            u16::MAX
        } else {
            (1u16 << dev.clb_minors) - 1
        };
        (self.minor_mask & full_mask) == full_mask
    }

    pub fn clb_sites(&self) -> Vec<(u32, u32)> {
        if self.x0 > self.x1 || self.y0 > self.y1 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(
            ((self.x1 - self.x0 + 1) as usize).saturating_mul((self.y1 - self.y0 + 1) as usize),
        );
        for y in self.y0..=self.y1 {
            for x in self.x0..=self.x1 {
                out.push((x, y));
            }
        }
        out
    }

    pub fn clb_majors(&self, dev: &Device) -> Vec<u16> {
        let mut majors: Vec<u16> = self
            .clb_sites()
            .into_iter()
            .filter_map(|(x, y)| dev.clb_major(x, y))
            .collect();
        majors.sort_unstable();
        majors.dedup();
        majors
    }

    /// Contiguous FAR (Frame Address Register) ranges covering this `Pblock`.
    pub fn far_ranges(&self, dev: &Device) -> Vec<FarRange> {
        let majors = self.clb_majors(dev);
        let minor_end = dev.clb_minors.saturating_sub(1) as u8;
        let mut ranges: Vec<FarRange> = Vec::new();
        for maj in majors {
            if let Some(last) = ranges.last_mut() {
                if last.block_type == Far::CLB_IO_CLK
                    && last.major_end + 1 == maj
                    && last.minor_start == 0
                    && last.minor_end == minor_end
                {
                    last.major_end = maj;
                    continue;
                }
            }
            ranges.push(FarRange {
                block_type: Far::CLB_IO_CLK,
                major_start: maj,
                major_end: maj,
                minor_start: 0,
                minor_end,
            });
        }
        ranges
    }
}

/// Contiguous Frame Address Register (FAR) range addressed by a partial bitstream (`.pbits`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FarRange {
    pub block_type: u8,
    pub major_start: u16,
    pub major_end: u16,
    pub minor_start: u8,
    pub minor_end: u8,
}

impl FarRange {
    pub fn contains(&self, block_type: u8, major: u16, minor: u8) -> bool {
        self.block_type == block_type
            && major >= self.major_start
            && major <= self.major_end
            && minor >= self.minor_start
            && minor <= self.minor_end
    }

    pub fn frame_count(&self) -> usize {
        let majors = (self.major_end as usize).saturating_sub(self.major_start as usize) + 1;
        let minors = (self.minor_end as usize).saturating_sub(self.minor_start as usize) + 1;
        majors.saturating_mul(minors)
    }
}

/// Differential or Pblock-scoped Partial Frame Bitstream (`.pbits`).
///
/// Unlike a full `.hbits` stream (which omits zero frames because full `program`
/// wipes the die), `.pbits` carries explicit `far_ranges` and supports zeroed
/// frames so hot-swapping `RM_A -> RM_B` scrubs stale `RM_A` configuration bits
/// inside the `Pblock` without disturbing static frames or static FF/BRAM state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialBitstream {
    pub idcode: u32,
    pub reset_after_reconfig: bool,
    pub pblock_bounds: Option<(u32, u32, u32, u32)>,
    pub far_ranges: Vec<FarRange>,
    /// Frame map for the partial region (may include explicit `0` words for scrubbed frames).
    pub frames: BTreeMap<(u8, u16, u8), u128>,
    pub packets: Vec<u8>,
}

impl PartialBitstream {
    pub fn from_pbits(bytes: &[u8]) -> Result<Self, String> {
        decode_pbits(bytes)
    }

    /// Convert non-zero partial frames into a legacy `Bitstream` container.
    pub fn to_bitstream(&self) -> Bitstream {
        let frames: BTreeMap<(u8, u16, u8), u128> = self
            .frames
            .iter()
            .filter(|(_, w)| **w != 0)
            .map(|(k, w)| (*k, *w))
            .collect();
        let packets = encode_packets(self.idcode, &frames);
        Bitstream {
            idcode: self.idcode,
            frames,
            packets,
        }
    }
}

/// Encode a [`PartialBitstream`] into compact `.pbits` packets (retaining explicit 0-frames in diff).
pub fn encode_pbits(
    idcode: u32,
    reset_after_reconfig: bool,
    pblock_bounds: Option<(u32, u32, u32, u32)>,
    far_ranges: &[FarRange],
    frames: &BTreeMap<(u8, u16, u8), u128>,
) -> Vec<u8> {
    let range_bytes = far_ranges.len() * 8;
    let body_cap = range_bytes + 7 + frames.len().saturating_mul(26) + 10;
    let mut out = Vec::with_capacity(PBITS_HEADER_BYTES + body_cap);
    out.resize(PBITS_HEADER_BYTES, 0);

    for r in far_ranges {
        out.push(r.block_type);
        out.push(0);
        out.extend_from_slice(&r.major_start.to_le_bytes());
        out.extend_from_slice(&r.major_end.to_le_bytes());
        out.push(r.minor_start);
        out.push(r.minor_end);
    }

    // SYNC
    out.push(0x01);
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(b"HELI");

    let mut run_start: Option<u32> = None;
    let mut last_far: Option<u32> = None;
    let mut run: Vec<u128> = Vec::with_capacity(frames.len().min(MAX_RUN_FRAMES));
    for ((block, major, minor), payload) in frames {
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

    let mut flags = 0u32;
    if reset_after_reconfig {
        flags |= PBITS_FLAG_RESET_AFTER_RECONFIG;
    }
    let (x0, y0, x1, y1) = if let Some(b) = pblock_bounds {
        flags |= PBITS_FLAG_HAS_PBLOCK;
        b
    } else {
        (0, 0, 0, 0)
    };
    let body_len = (out.len() - PBITS_HEADER_BYTES) as u64;
    out[0..4].copy_from_slice(b"PBIT");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6..10].copy_from_slice(&idcode.to_le_bytes());
    out[10..14].copy_from_slice(&flags.to_le_bytes());
    out[14..18].copy_from_slice(&x0.to_le_bytes());
    out[18..22].copy_from_slice(&y0.to_le_bytes());
    out[22..26].copy_from_slice(&x1.to_le_bytes());
    out[26..30].copy_from_slice(&y1.to_le_bytes());
    out[30..34].copy_from_slice(&(far_ranges.len() as u32).to_le_bytes());
    out[34..42].copy_from_slice(&body_len.to_le_bytes());
    let hash = sha256_lite(&out[PBITS_HEADER_BYTES..]);
    out[42..74].copy_from_slice(&hash);
    let hdr_crc = crc32c(&out[..74]);
    out[74..78].copy_from_slice(&hdr_crc.to_le_bytes());
    out
}

/// Decode a `.pbits` stream back into a [`PartialBitstream`].
pub fn decode_pbits(bytes: &[u8]) -> Result<PartialBitstream, String> {
    if bytes.len() < PBITS_HEADER_BYTES || &bytes[0..4] != b"PBIT" {
        return Err("not a .pbits stream".into());
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != 1 {
        return Err(format!("unsupported .pbits version {version}"));
    }
    let idcode = u32::from_le_bytes(bytes[6..10].try_into().unwrap());
    let flags = u32::from_le_bytes(bytes[10..14].try_into().unwrap());
    let x0 = u32::from_le_bytes(bytes[14..18].try_into().unwrap());
    let y0 = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
    let x1 = u32::from_le_bytes(bytes[22..26].try_into().unwrap());
    let y1 = u32::from_le_bytes(bytes[26..30].try_into().unwrap());
    let n_ranges = u32::from_le_bytes(bytes[30..34].try_into().unwrap()) as usize;
    let body_len = u64::from_le_bytes(bytes[34..42].try_into().unwrap()) as usize;
    let stored_hash = &bytes[42..74];
    let hdr_crc = u32::from_le_bytes(bytes[74..78].try_into().unwrap());
    if crc32c(&bytes[..74]) != hdr_crc {
        return Err(".pbits header CRC mismatch".into());
    }
    let body = bytes
        .get(PBITS_HEADER_BYTES..PBITS_HEADER_BYTES + body_len)
        .ok_or_else(|| "truncated .pbits body".to_string())?;
    if sha256_lite(body) != stored_hash {
        return Err(".pbits body hash mismatch".into());
    }
    let range_bytes = n_ranges
        .checked_mul(8)
        .ok_or_else(|| "overflow in .pbits FAR range table".to_string())?;
    if body.len() < range_bytes {
        return Err("truncated .pbits FAR range table".into());
    }
    let mut far_ranges = Vec::with_capacity(n_ranges);
    for c in body[..range_bytes].chunks_exact(8) {
        far_ranges.push(FarRange {
            block_type: c[0],
            major_start: u16::from_le_bytes([c[2], c[3]]),
            major_end: u16::from_le_bytes([c[4], c[5]]),
            minor_start: c[6],
            minor_end: c[7],
        });
    }
    let mut frames = BTreeMap::new();
    let mut far = 0u32;
    let mut synced = false;
    let mut i = range_bytes;
    while i < body.len() {
        let op = body[i];
        let len = u16::from_le_bytes(
            body.get(i + 1..i + 3)
                .ok_or_else(|| "truncated .pbits packet header".to_string())?
                .try_into()
                .unwrap(),
        ) as usize;
        let payload = body
            .get(i + 3..i + 3 + len)
            .ok_or_else(|| format!("truncated payload for .pbits packet {op:#04x}"))?;
        match op {
            0x01 => {
                if payload != b"HELI" {
                    return Err("bad SYNC word in .pbits".into());
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
                for chunk in payload.chunks_exact(16) {
                    let f = Far::decode(far);
                    frames.insert(
                        (f.block_type, f.major, f.minor),
                        u128::from_le_bytes(chunk.try_into().unwrap()),
                    );
                    far += 1;
                }
            }
            0x21 => {}
            0x02 => synced = false,
            other => return Err(format!("unknown .pbits packet {other:#04x}")),
        }
        i += 3 + len;
    }
    let reset_after_reconfig = (flags & PBITS_FLAG_RESET_AFTER_RECONFIG) != 0;
    let pblock_bounds = if (flags & PBITS_FLAG_HAS_PBLOCK) != 0 {
        Some((x0, y0, x1, y1))
    } else {
        None
    };
    Ok(PartialBitstream {
        idcode,
        reset_after_reconfig,
        pblock_bounds,
        far_ranges,
        frames,
        packets: bytes.to_vec(),
    })
}

/// Generate a `.pbits` partial bitstream for a rectangular [`Pblock`].
///
/// Includes the FAR ranges covering `pblock` and all non-zero frames inside `pblock`.
pub fn bitgen_partial_pblock(
    dev: &Device,
    routed: &Routed,
    pblock: &Pblock,
) -> Result<PartialBitstream, String> {
    if !pblock.is_within_device(dev) {
        return Err(format!(
            "HDPR-1: pblock {} (X{}Y{}:X{}Y{}) out of device CLB bounds",
            pblock.name, pblock.x0, pblock.y0, pblock.x1, pblock.y1
        ));
    }
    if !pblock.is_frame_aligned(dev) {
        return Err(format!(
            "HDPR-2: pblock {} minor_mask {:#06x} is not aligned to full CLB column frames",
            pblock.name, pblock.minor_mask
        ));
    }
    let full = bitgen(dev, routed)?;
    let far_ranges = pblock.far_ranges(dev);
    let mut frames = BTreeMap::new();
    for ((b, maj, min), w) in &full.frames {
        if far_ranges.iter().any(|r| r.contains(*b, *maj, *min)) && *w != 0 {
            frames.insert((*b, *maj, *min), *w);
        }
    }
    if frames.is_empty() {
        return Err(format!(
            "{ERR_CODE_EMPTY_BITSTREAM}: pblock {} produced no frames",
            pblock.name
        ));
    }
    let bounds = Some((pblock.x0, pblock.y0, pblock.x1, pblock.y1));
    let packets = encode_pbits(
        dev.idcode,
        pblock.reset_after_reconfig,
        bounds,
        &far_ranges,
        &frames,
    );
    Ok(PartialBitstream {
        idcode: dev.idcode,
        reset_after_reconfig: pblock.reset_after_reconfig,
        pblock_bounds: bounds,
        far_ranges,
        frames,
        packets,
    })
}

/// Generate a compact differential `.pbits` stream containing only modified frames
/// between `base` and `target` (optionally scoped to `pblock`).
///
/// Frames present in `base` that become `0` in `target` are recorded explicitly
/// with payload `0` so live fabric swap scrubs the old RM configuration.
pub fn diff_partial_bitstream(
    dev: &Device,
    base: &Bitstream,
    target: &Bitstream,
    pblock: Option<&Pblock>,
) -> Result<PartialBitstream, String> {
    if base.idcode != dev.idcode || target.idcode != dev.idcode {
        return Err(format!(
            "idcode mismatch in diff_partial_bitstream: dev={:#010x} base={:#010x} target={:#010x}",
            dev.idcode, base.idcode, target.idcode
        ));
    }
    let (far_ranges, reset_after_reconfig, bounds) = if let Some(pb) = pblock {
        if !pb.is_within_device(dev) {
            return Err(format!(
                "HDPR-1: pblock {} (X{}Y{}:X{}Y{}) out of device CLB bounds",
                pb.name, pb.x0, pb.y0, pb.x1, pb.y1
            ));
        }
        if !pb.is_frame_aligned(dev) {
            return Err(format!(
                "HDPR-2: pblock {} minor_mask {:#06x} is not frame-aligned",
                pb.name, pb.minor_mask
            ));
        }
        (
            pb.far_ranges(dev),
            pb.reset_after_reconfig,
            Some((pb.x0, pb.y0, pb.x1, pb.y1)),
        )
    } else {
        (Vec::new(), false, None)
    };

    let in_scope = |b: u8, maj: u16, min: u8| -> bool {
        if far_ranges.is_empty() {
            true
        } else {
            far_ranges.iter().any(|r| r.contains(b, maj, min))
        }
    };

    let mut all_keys: std::collections::BTreeSet<(u8, u16, u8)> = std::collections::BTreeSet::new();
    for &k in base.frames.keys().chain(target.frames.keys()) {
        if in_scope(k.0, k.1, k.2) {
            all_keys.insert(k);
        }
    }
    let mut frames = BTreeMap::new();
    let mut diff_ranges: Vec<FarRange> = far_ranges;
    for k in all_keys {
        let w0 = base.frames.get(&k).copied().unwrap_or(0);
        let w1 = target.frames.get(&k).copied().unwrap_or(0);
        if w0 != w1 {
            frames.insert(k, w1);
            if pblock.is_none() {
                diff_ranges.push(FarRange {
                    block_type: k.0,
                    major_start: k.1,
                    major_end: k.1,
                    minor_start: k.2,
                    minor_end: k.2,
                });
            }
        }
    }
    let packets = encode_pbits(
        dev.idcode,
        reset_after_reconfig,
        bounds,
        &diff_ranges,
        &frames,
    );
    Ok(PartialBitstream {
        idcode: dev.idcode,
        reset_after_reconfig,
        pblock_bounds: bounds,
        far_ranges: diff_ranges,
        frames,
        packets,
    })
}

/// Sub-millisecond in-place LUT6 `INIT` ECO patcher (`< 1 ms`, O(1) frame lookup).
///
/// Directly updates the 64-bit truth table for `CLB_X{x}Y{y}.BLE{ble}` in `bits.frames`
/// and re-encodes the sparse packet buffer without running placement or routing.
pub fn patch_lut_init(
    dev: &Device,
    bits: &mut Bitstream,
    x: u32,
    y: u32,
    ble: u32,
    new_init: u64,
) -> Result<(), String> {
    if bits.idcode != dev.idcode {
        return Err(format!(
            "idcode mismatch bitstream {:#010x} device {:#010x}",
            bits.idcode, dev.idcode
        ));
    }
    let major = dev
        .clb_major(x, y)
        .ok_or_else(|| format!("not a CLB site CLB_X{x}Y{y}"))?;
    let bi = ble as usize;
    if bi >= dev.n_ble as usize || bi >= 8 {
        return Err(format!("BLE {ble} out of range at CLB_X{x}Y{y}"));
    }
    let feat = clb_feat(dev)?;
    let fb = feat.frame_bits;
    let abs0 = feat.init0_abs[bi];
    let minor = (abs0 / fb) as u8;
    let bit0 = abs0 % fb;
    let key = (Far::CLB_IO_CLK, major, minor);
    let clear_mask: u128 = (u64::MAX as u128) << bit0;
    let set_mask: u128 = (new_init as u128) << bit0;
    let cur = bits.frames.get(&key).copied().unwrap_or(0);
    let updated = (cur & !clear_mask) | set_mask;
    if updated == 0 {
        bits.frames.remove(&key);
    } else {
        bits.frames.insert(key, updated);
    }
    bits.packets = encode_packets(bits.idcode, &bits.frames);
    Ok(())
}

/// Sub-millisecond LUT `INIT` ECO by logical cell name using existing placement coordinates.
pub fn patch_lut_init_cell(
    dev: &Device,
    routed: &Routed,
    bits: &mut Bitstream,
    cell: &str,
    new_init: u64,
) -> Result<(u32, u32, u8), String> {
    let idx = routed
        .placed
        .packed
        .lutffs
        .iter()
        .position(|l| l.lut_cell == cell || l.ff_cell == cell)
        .ok_or_else(|| format!("eco: no LUT/FF cell {cell}"))?;
    let (site, ble) = routed.placed.lutff_sites[idx];
    patch_lut_init(dev, bits, site.x, site.y, ble as u32, new_init)?;
    Ok((site.x, site.y, ble))
}

/// Read back an 8-bit IMUX selector `CLB_X{x}Y{y}.IMUX[{mux}][0..7]` directly from `bits`.
pub fn readback_imux_sel(
    dev: &Device,
    bits: &Bitstream,
    x: u32,
    y: u32,
    mux: u32,
) -> Result<u8, String> {
    let major = dev
        .clb_major(x, y)
        .ok_or_else(|| format!("not a CLB site CLB_X{x}Y{y}"))?;
    if mux >= 64 {
        return Err(format!("IMUX {mux} out of range at CLB_X{x}Y{y}"));
    }
    let feat = clb_feat(dev)?;
    let fb = feat.frame_bits;
    let mut sel = 0u8;
    for b in 0..8u32 {
        let abs = match b {
            0..=4 => feat.imux0_abs + mux * 5 + b,
            5 => feat.imux5_abs + mux,
            6 => feat.imux6_abs + mux,
            7 => feat.imux7_abs + mux,
            _ => unreachable!(),
        };
        let minor = (abs / fb) as u8;
        let bit = abs % fb;
        let word = bits
            .frames
            .get(&(Far::CLB_IO_CLK, major, minor))
            .copied()
            .unwrap_or(0);
        if (word >> bit) & 1 == 1 {
            sel |= 1u8 << b;
        }
    }
    Ok(sel)
}

/// Directly patch `CLB_X{x}Y{y}.IMUX[{mux}][0..7]` in `bits` in O(1) time.
pub fn patch_imux_sel(
    dev: &Device,
    bits: &mut Bitstream,
    x: u32,
    y: u32,
    mux: u32,
    sel: u8,
) -> Result<(), String> {
    if bits.idcode != dev.idcode {
        return Err(format!(
            "idcode mismatch bitstream {:#010x} device {:#010x}",
            bits.idcode, dev.idcode
        ));
    }
    let major = dev
        .clb_major(x, y)
        .ok_or_else(|| format!("not a CLB site CLB_X{x}Y{y}"))?;
    if mux >= 64 {
        return Err(format!("IMUX {mux} out of range at CLB_X{x}Y{y}"));
    }
    let feat = clb_feat(dev)?;
    let fb = feat.frame_bits;
    for b in 0..8u32 {
        let abs = match b {
            0..=4 => feat.imux0_abs + mux * 5 + b,
            5 => feat.imux5_abs + mux,
            6 => feat.imux6_abs + mux,
            7 => feat.imux7_abs + mux,
            _ => unreachable!(),
        };
        let minor = (abs / fb) as u8;
        let bit = abs % fb;
        let key = (Far::CLB_IO_CLK, major, minor);
        let cur = bits.frames.get(&key).copied().unwrap_or(0);
        let updated = if (sel >> b) & 1 == 1 {
            cur | (1u128 << bit)
        } else {
            cur & !(1u128 << bit)
        };
        if updated == 0 {
            bits.frames.remove(&key);
        } else {
            bits.frames.insert(key, updated);
        }
    }
    bits.packets = encode_packets(bits.idcode, &bits.frames);
    Ok(())
}

/// Compute the HAD IMUX select byte connecting driver `(from_x, from_y, dble)` to sink `(to_x, to_y)`.
pub fn encode_imux_hop(
    from_x: u32,
    from_y: u32,
    dble: u8,
    to_x: u32,
    to_y: u32,
) -> Result<u8, String> {
    if dble >= 8 {
        return Err(format!("driver BLE {dble} out of range (0..8)"));
    }
    if from_x == to_x && from_y == to_y {
        return Ok(16 + dble);
    }
    if from_x == to_x && from_y + 1 == to_y {
        return Ok(dble);
    }
    if from_x == to_x && to_y + 1 == from_y {
        return Ok(8 + dble);
    }
    if from_x == to_x && from_y + 2 == to_y {
        return Ok(32 + dble);
    }
    if from_x == to_x && to_y + 2 == from_y {
        return Ok(40 + dble);
    }
    if from_y == to_y && from_x + 1 == to_x {
        return Ok(48 + dble);
    }
    if from_y == to_y && to_x + 1 == from_x {
        return Ok(56 + dble);
    }
    if from_y == to_y && from_x + 2 == to_x {
        return Ok(64 + dble);
    }
    if from_y == to_y && to_x + 2 == from_x {
        return Ok(72 + dble);
    }
    if from_x + 1 == to_x && from_y + 1 == to_y {
        return Ok(80 + dble);
    }
    if to_x + 1 == from_x && from_y + 1 == to_y {
        return Ok(88 + dble);
    }
    if from_x + 1 == to_x && to_y + 1 == from_y {
        return Ok(96 + dble);
    }
    if to_x + 1 == from_x && to_y + 1 == from_y {
        return Ok(104 + dble);
    }
    if from_x == to_x && from_y + 3 == to_y {
        return Ok(112 + dble);
    }
    if from_x == to_x && to_y + 3 == from_y {
        return Ok(120 + dble);
    }
    if from_x + 2 == to_x && from_y + 1 == to_y {
        return Ok(128 + dble);
    }
    if from_x + 2 == to_x && to_y + 1 == from_y {
        return Ok(136 + dble);
    }
    if to_x + 2 == from_x && from_y + 1 == to_y {
        return Ok(144 + dble);
    }
    if to_x + 2 == from_x && to_y + 1 == from_y {
        return Ok(152 + dble);
    }
    if from_x + 1 == to_x && from_y + 2 == to_y {
        return Ok(160 + dble);
    }
    if from_x + 1 == to_x && to_y + 2 == from_y {
        return Ok(168 + dble);
    }
    if to_x + 1 == from_x && from_y + 2 == to_y {
        return Ok(176 + dble);
    }
    if to_x + 1 == from_x && to_y + 2 == from_y {
        return Ok(184 + dble);
    }
    if from_y == to_y && from_x + 3 == to_x {
        return Ok(192 + dble);
    }
    if from_y == to_y && to_x + 3 == from_x {
        return Ok(200 + dble);
    }
    if from_x + 2 == to_x && from_y + 2 == to_y {
        return Ok(208 + dble);
    }
    if to_x + 2 == from_x && from_y + 2 == to_y {
        return Ok(216 + dble);
    }
    if from_x + 2 == to_x && to_y + 2 == from_y {
        return Ok(224 + dble);
    }
    if to_x + 2 == from_x && to_y + 2 == from_y {
        return Ok(232 + dble);
    }
    if from_x == to_x && from_y + 4 == to_y {
        return Ok(240 + dble);
    }
    if from_x == to_x && to_y + 4 == from_y {
        return Ok(248 + dble);
    }
    Err(format!(
        "ECO probe: no single-hop IMUX route from CLB_X{from_x}Y{from_y}/BLE{dble} to CLB_X{to_x}Y{to_y}"
    ))
}

/// Sub-millisecond incremental probe ECO reroute (`< 1 ms`, no placement or PathFinder).
///
/// Re-routes either:
/// - an IOB debug/probe output pad when `dst_target.1 == dev.clb_y0 - 1` (`(iob_x, iob_y, _)`), or
/// - a CLB IMUX input `(dst_x, dst_y, mux)` (e.g. an ILA capture/trigger probe LUT pin)
/// to sample `src_ble = (src_x, src_y, dble)` directly in `bits`.
/// Returns the programmed selector byte (for CLB IMUX) or `dble` (for IOB pad bind).
pub fn reroute_probe_eco(
    dev: &Device,
    bits: &mut Bitstream,
    src_ble: (u32, u32, u8),
    dst_target: (u32, u32, u32),
) -> Result<u8, String> {
    if bits.idcode != dev.idcode {
        return Err(format!(
            "idcode mismatch bitstream {:#010x} device {:#010x}",
            bits.idcode, dev.idcode
        ));
    }
    let (sx, sy, dble) = src_ble;
    if dev.clb_major(sx, sy).is_none() {
        return Err(format!("probe source CLB_X{sx}Y{sy} is not a legal CLB tile"));
    }
    if u32::from(dble) >= dev.n_ble {
        return Err(format!("probe source BLE {dble} out of range"));
    }
    let (dx, dy, dst_mux) = dst_target;
    if dy == dev.clb_y0.saturating_sub(1) {
        let major = dev
            .iob_major(dx, dy)
            .ok_or_else(|| format!("IOB_X{dx}Y{dy} is not a legal IOB site"))?;
        let key = (Far::IOB, major, 0);
        let prev_elec = bits.frames.get(&key).copied().unwrap_or(0) & !0xFFFFu128;
        let word = 1u128 | ((dble as u128) << 1) | ((sy as u128) << 4) | prev_elec;
        bits.frames.insert(key, word);
        bits.packets = encode_packets(bits.idcode, &bits.frames);
        return Ok(dble);
    }
    let sel = encode_imux_hop(sx, sy, dble, dx, dy)?;
    patch_imux_sel(dev, bits, dx, dy, dst_mux, sel)?;
    Ok(sel)
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
    fn pbits_encode_decode_and_diff_scrubs_zeroed_frames() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let base = bitgen(&dev, &r).unwrap();
        let (site, ble) = r.placed.lutff_sites[0];

        let pblock = Pblock::new("pblock_rm", site.x, site.y, site.x, site.y)
            .with_reset_after_reconfig(true);
        assert!(pblock.is_within_device(&dev));
        assert!(pblock.is_frame_aligned(&dev));

        let pbits = bitgen_partial_pblock(&dev, &r, &pblock).unwrap();
        assert!(pbits.reset_after_reconfig);
        assert_eq!(pbits.pblock_bounds, Some((site.x, site.y, site.x, site.y)));
        assert_eq!(pbits.far_ranges.len(), 1);
        let round = decode_pbits(&pbits.packets).unwrap();
        assert_eq!(round, pbits);

        // Corrupt magic and CRC must fail decode_pbits.
        let mut bad_crc = pbits.packets.clone();
        bad_crc[6] ^= 0xFF;
        assert!(decode_pbits(&bad_crc).is_err());

        // Diff against a target where the CLB frame is cleared to 0:
        // diff_partial_bitstream must record an explicit 0 word so the fabric scrubs it.
        let mut target = base.clone();
        let maj = dev.clb_major(site.x, site.y).unwrap();
        target.frames.remove(&(Far::CLB_IO_CLK, maj, 0));
        let diff = diff_partial_bitstream(&dev, &base, &target, Some(&pblock)).unwrap();
        assert_eq!(diff.frames.get(&(Far::CLB_IO_CLK, maj, 0)), Some(&0u128));
        let diff_rt = decode_pbits(&diff.packets).unwrap();
        assert_eq!(diff_rt.frames.get(&(Far::CLB_IO_CLK, maj, 0)), Some(&0u128));

        // Check ble LUT INIT in pblock stream
        let _ = ble;
    }

    #[test]
    fn sub_millisecond_patch_lut_init_and_reroute_probe_eco() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let mut bs = bitgen(&dev, &r).unwrap();
        let (site, ble) = r.placed.lutff_sites[0];

        let t0 = std::time::Instant::now();
        patch_lut_init(&dev, &mut bs, site.x, site.y, ble as u32, 0xDEAD_BEEF_CAFE_BABEu64)
            .unwrap();
        let elapsed_lut = t0.elapsed();
        assert!(
            elapsed_lut.as_micros() < 1_000,
            "patch_lut_init must complete in < 1 ms, took {elapsed_lut:?}"
        );
        assert_eq!(
            readback_lut_init(&dev, &bs, site.x, site.y, ble as u32).unwrap(),
            0xDEAD_BEEF_CAFE_BABEu64
        );

        // Cell-based LUT ECO patch must match eco_lut bit-for-bit.
        let cell_name = r.placed.packed.lutffs[0].lut_cell.clone();
        let mut bs_cell = bitgen(&dev, &r).unwrap();
        patch_lut_init_cell(&dev, &r, &mut bs_cell, &cell_name, 0x1234_5678_9ABC_DEF0).unwrap();
        let full_eco = eco_lut(&dev, &r, &cell_name, 0x1234_5678_9ABC_DEF0).unwrap();
        assert_eq!(bs_cell.frames, full_eco.frames);
        assert_eq!(bs_cell.packets, full_eco.packets);

        // Sub-millisecond probe ECO reroute to a neighboring CLB IMUX and to an IOB pad.
        let t1 = std::time::Instant::now();
        let sel = reroute_probe_eco(
            &dev,
            &mut bs,
            (site.x, site.y, ble),
            (site.x + 1, site.y, 0),
        )
        .unwrap();
        let elapsed_probe = t1.elapsed();
        assert!(
            elapsed_probe.as_micros() < 1_000,
            "reroute_probe_eco must complete in < 1 ms, took {elapsed_probe:?}"
        );
        assert_eq!(sel, 48 + ble);
        assert_eq!(
            readback_imux_sel(&dev, &bs, site.x + 1, site.y, 0).unwrap(),
            48 + ble
        );

        let iob = r.iob_src[0].iob;
        let iob_ble = reroute_probe_eco(&dev, &mut bs, (site.x, site.y, 3), (iob.0, iob.1, 0)).unwrap();
        assert_eq!(iob_ble, 3);
    }
}
