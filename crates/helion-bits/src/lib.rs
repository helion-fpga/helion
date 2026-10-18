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
    let mut iob_elec_by_net: HashMap<&str, u128> = HashMap::with_capacity(packed.iobs.len());
    for iob in &packed.iobs {
        iob_elec_by_net.entry(iob.from_net.as_str()).or_insert_with(|| {
            Device::iob_electrical_bits(
                iob.drive.as_deref(),
                iob.slew.as_deref(),
                iob.pulltype.as_deref(),
                iob.diff_term.as_deref(),
                iob.in_term.as_deref(),
            )
        });
    }
    for r in &routed.iob_src {
        let major = dev
            .iob_major(r.iob.0, r.iob.1)
            .ok_or_else(|| format!("IOB_X{}Y{} has no major", r.iob.0, r.iob.1))?;
        // bit0 USED, [3:1] BLE, [15:4] CLB y; [24:16] DRIVE/SLEW/PULLTYPE/DIFF_TERM/IN_TERM (0 = default)
        let elec = iob_elec_by_net.get(r.net.as_str()).copied().unwrap_or(0);
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

/// One BRAM bit-lane mapping inside a Vivado UG898 `.mmi` (Memory Map Information) address space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MmiBitLane {
    pub cell_name: String,
    pub placed_site: String,
    pub bram_major: u16,
    pub msb: u8,
    pub lsb: u8,
    pub addr_begin: u64,
    pub addr_end: u64,
    pub parity_bits: u8,
}

impl MmiBitLane {
    pub fn bit_width(&self) -> u8 {
        self.msb.saturating_sub(self.lsb) + 1
    }
}

/// Processor-visible BRAM address space in a `.mmi` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MmiAddressSpace {
    pub name: String,
    pub proc_name: String,
    pub addr_begin: u64,
    pub addr_end: u64,
    pub bus_width: u8,
    pub lanes: Vec<MmiBitLane>,
}

/// Vivado UG898 Memory Map Information (`.mmi`) descriptor mapping processor address
/// spaces to physical `Far::BRAM` tiles and bit-lanes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMapInfo {
    pub part: String,
    pub spaces: Vec<MmiAddressSpace>,
}

impl MemoryMapInfo {
    /// Derive a `.mmi` descriptor directly from a placed & routed design's BRAM18 sites.
    ///
    /// Each BRAM18 tile exposes 64 INIT words × 8 bytes = 512 bytes (or `words_per_bram * 8`)
    /// cascaded sequentially starting at `base_addr`.
    pub fn from_routed(
        dev: &Device,
        routed: &Routed,
        proc_name: &str,
        space_name: &str,
        base_addr: u64,
    ) -> Self {
        let bytes_per_bram = 64u64 * 8; // 64 64-bit INIT words per BRAM major = 512 B
        let mut lanes = Vec::new();
        let mut cur_addr = base_addr;
        for (i, bram) in routed.placed.packed.brams.iter().enumerate() {
            let site = routed
                .placed
                .bram_sites
                .get(i)
                .copied()
                .unwrap_or(helion_device::Site {
                    x: dev.bram_x,
                    y: dev.bram_y0 + (i as u32) * 2,
                    kind: helion_device::SiteKind::Bram,
                });
            let end = cur_addr + bytes_per_bram - 1;
            lanes.push(MmiBitLane {
                cell_name: bram.cell.clone(),
                placed_site: site.id(),
                bram_major: i as u16,
                msb: 63,
                lsb: 0,
                addr_begin: cur_addr,
                addr_end: end,
                parity_bits: 0,
            });
            cur_addr = end + 1;
        }
        let addr_end = if cur_addr > base_addr {
            cur_addr - 1
        } else {
            base_addr + bytes_per_bram - 1
        };
        Self {
            part: dev.part.clone(),
            spaces: vec![MmiAddressSpace {
                name: space_name.into(),
                proc_name: proc_name.into(),
                addr_begin: base_addr,
                addr_end,
                bus_width: 64,
                lanes,
            }],
        }
    }

    /// Emit a Vivado UG898-compatible `.mmi` XML document.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str(&format!(
            "<MemInfo Version=\"1\" Minor=\"0\" Part=\"{}\">\n",
            self.part
        ));
        for sp in &self.spaces {
            out.push_str(&format!(
                "  <Processor Endianness=\"Little\" InstPath=\"{}\">\n",
                sp.proc_name
            ));
            out.push_str(&format!(
                "    <AddressSpace Name=\"{}\" Begin=\"{:#010x}\" End=\"{:#010x}\" BusWidth=\"{}\">\n",
                sp.name, sp.addr_begin, sp.addr_end, sp.bus_width
            ));
            out.push_str("      <BusBlock>\n");
            for lane in &sp.lanes {
                out.push_str(&format!(
                    "        <BitLane MemType=\"RAMB18\" Cell=\"{}\" Placement=\"{}\" BramMajor=\"{}\">\n",
                    lane.cell_name, lane.placed_site, lane.bram_major
                ));
                out.push_str(&format!(
                    "          <DataWidth MSB=\"{}\" LSB=\"{}\"/>\n",
                    lane.msb, lane.lsb
                ));
                out.push_str(&format!(
                    "          <AddressRange Begin=\"{:#010x}\" End=\"{:#010x}\"/>\n",
                    lane.addr_begin, lane.addr_end
                ));
                out.push_str(&format!(
                    "          <Parity ON=\"{}\" NumBits=\"{}\"/>\n",
                    if lane.parity_bits > 0 { "true" } else { "false" },
                    lane.parity_bits
                ));
                out.push_str("        </BitLane>\n");
            }
            out.push_str("      </BusBlock>\n");
            out.push_str("    </AddressSpace>\n");
            out.push_str("  </Processor>\n");
        }
        out.push_str("</MemInfo>\n");
        out
    }

    /// Parse a Vivado UG898 `.mmi` XML document.
    pub fn from_xml(xml: &str) -> Result<Self, String> {
        if !xml.contains("<MemInfo") {
            return Err("mmi: missing <MemInfo root element".into());
        }
        let mut part = String::from("HL10T-C32-1");
        let mut spaces = Vec::new();
        let mut cur_proc = String::new();
        let mut cur_space: Option<MmiAddressSpace> = None;
        let mut cur_lane: Option<MmiBitLane> = None;

        for raw in xml.lines() {
            let line = raw.trim();
            if line.starts_with("<MemInfo") {
                if let Some(p) = xml_attr(line, "Part") {
                    part = p.to_string();
                }
            } else if line.starts_with("<Processor") {
                cur_proc = xml_attr(line, "InstPath").unwrap_or("cpu").to_string();
            } else if line.starts_with("<AddressSpace") {
                let name = xml_attr(line, "Name").unwrap_or("mem").to_string();
                let addr_begin = parse_u64_num(xml_attr(line, "Begin").unwrap_or("0"))?;
                let addr_end = parse_u64_num(xml_attr(line, "End").unwrap_or("0"))?;
                let bus_width = xml_attr(line, "BusWidth")
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(64);
                cur_space = Some(MmiAddressSpace {
                    name,
                    proc_name: cur_proc.clone(),
                    addr_begin,
                    addr_end,
                    bus_width,
                    lanes: Vec::new(),
                });
            } else if line.starts_with("</AddressSpace>") {
                if let Some(sp) = cur_space.take() {
                    spaces.push(sp);
                }
            } else if line.starts_with("<BitLane") {
                let cell_name = xml_attr(line, "Cell").unwrap_or("u_bram").to_string();
                let placed_site = xml_attr(line, "Placement").unwrap_or("BRAM_X10Y1").to_string();
                let bram_major = xml_attr(line, "BramMajor")
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(0);
                cur_lane = Some(MmiBitLane {
                    cell_name,
                    placed_site,
                    bram_major,
                    msb: 63,
                    lsb: 0,
                    addr_begin: 0,
                    addr_end: 0,
                    parity_bits: 0,
                });
            } else if line.starts_with("<DataWidth") {
                if let Some(l) = cur_lane.as_mut() {
                    if let Some(msb) = xml_attr(line, "MSB") {
                        l.msb = msb.parse::<u8>().map_err(|e| format!("mmi MSB: {e}"))?;
                    }
                    if let Some(lsb) = xml_attr(line, "LSB") {
                        l.lsb = lsb.parse::<u8>().map_err(|e| format!("mmi LSB: {e}"))?;
                    }
                }
            } else if line.starts_with("<AddressRange") {
                if let Some(l) = cur_lane.as_mut() {
                    if let Some(b) = xml_attr(line, "Begin") {
                        l.addr_begin = parse_u64_num(b)?;
                    }
                    if let Some(e) = xml_attr(line, "End") {
                        l.addr_end = parse_u64_num(e)?;
                    }
                }
            } else if line.starts_with("<Parity") {
                if let Some(l) = cur_lane.as_mut() {
                    if let Some(nb) = xml_attr(line, "NumBits") {
                        l.parity_bits = nb.parse::<u8>().unwrap_or(0);
                    }
                }
            } else if line.starts_with("</BitLane>") {
                if let (Some(sp), Some(lane)) = (cur_space.as_mut(), cur_lane.take()) {
                    sp.lanes.push(lane);
                }
            }
        }
        if spaces.is_empty() {
            return Err("mmi: no <AddressSpace> found".into());
        }
        Ok(Self { part, spaces })
    }

    /// Emit a compact JSON `.mmi` document.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        out.push_str(&format!("  \"part\": \"{}\",\n", self.part));
        out.push_str("  \"spaces\": [\n");
        for (si, sp) in self.spaces.iter().enumerate() {
            out.push_str("    {\n");
            out.push_str(&format!("      \"name\": \"{}\",\n", sp.name));
            out.push_str(&format!("      \"proc_name\": \"{}\",\n", sp.proc_name));
            out.push_str(&format!("      \"addr_begin\": {},\n", sp.addr_begin));
            out.push_str(&format!("      \"addr_end\": {},\n", sp.addr_end));
            out.push_str(&format!("      \"bus_width\": {},\n", sp.bus_width));
            out.push_str("      \"lanes\": [\n");
            for (li, l) in sp.lanes.iter().enumerate() {
                let comma = if li + 1 == sp.lanes.len() { "" } else { "," };
                out.push_str(&format!(
                    "        {{\"cell_name\":\"{}\",\"placed_site\":\"{}\",\"bram_major\":{},\"msb\":{},\"lsb\":{},\"addr_begin\":{},\"addr_end\":{},\"parity_bits\":{}}}{comma}\n",
                    l.cell_name, l.placed_site, l.bram_major, l.msb, l.lsb, l.addr_begin, l.addr_end, l.parity_bits
                ));
            }
            out.push_str("      ]\n");
            let sp_comma = if si + 1 == self.spaces.len() { "" } else { "," };
            out.push_str(&format!("    }}{sp_comma}\n"));
        }
        out.push_str("  ]\n}\n");
        out
    }

    /// Parse a JSON `.mmi` document emitted by [`Self::to_json`].
    pub fn from_json(json: &str) -> Result<Self, String> {
        let mut part = String::from("HL10T-C32-1");
        let mut spaces = Vec::new();
        let mut cur_name = String::new();
        let mut cur_proc = String::new();
        let mut cur_begin = 0u64;
        let mut cur_end = 0u64;
        let mut cur_width = 64u8;
        let mut cur_lanes: Vec<MmiBitLane> = Vec::new();
        let mut in_space = false;

        for raw in json.lines() {
            let line = raw.trim().trim_end_matches(',');
            if let Some(v) = json_str_field(line, "part") {
                part = v.to_string();
            } else if let Some(v) = json_str_field(line, "name") {
                cur_name = v.to_string();
                in_space = true;
            } else if let Some(v) = json_str_field(line, "proc_name") {
                cur_proc = v.to_string();
            } else if let Some(v) = json_num_field(line, "addr_begin") {
                if !line.contains("\"cell_name\"") {
                    cur_begin = parse_u64_num(v)?;
                }
            } else if let Some(v) = json_num_field(line, "addr_end") {
                if !line.contains("\"cell_name\"") {
                    cur_end = parse_u64_num(v)?;
                }
            } else if let Some(v) = json_num_field(line, "bus_width") {
                cur_width = v.parse::<u8>().unwrap_or(64);
            }
            if line.contains("\"cell_name\"") {
                let cell_name = json_str_field(line, "cell_name")
                    .ok_or("mmi json: missing cell_name")?
                    .to_string();
                let placed_site = json_str_field(line, "placed_site")
                    .ok_or("mmi json: missing placed_site")?
                    .to_string();
                let bram_major = json_num_field(line, "bram_major")
                    .ok_or("mmi json: missing bram_major")?
                    .parse::<u16>()
                    .map_err(|e| format!("mmi json bram_major: {e}"))?;
                let msb = json_num_field(line, "msb")
                    .ok_or("mmi json: missing msb")?
                    .parse::<u8>()
                    .map_err(|e| format!("mmi json msb: {e}"))?;
                let lsb = json_num_field(line, "lsb")
                    .ok_or("mmi json: missing lsb")?
                    .parse::<u8>()
                    .map_err(|e| format!("mmi json lsb: {e}"))?;
                let addr_begin = parse_u64_num(
                    json_num_field(line, "addr_begin").ok_or("mmi json: missing lane addr_begin")?,
                )?;
                let addr_end = parse_u64_num(
                    json_num_field(line, "addr_end").ok_or("mmi json: missing lane addr_end")?,
                )?;
                let parity_bits = json_num_field(line, "parity_bits")
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(0);
                cur_lanes.push(MmiBitLane {
                    cell_name,
                    placed_site,
                    bram_major,
                    msb,
                    lsb,
                    addr_begin,
                    addr_end,
                    parity_bits,
                });
            } else if line == "]" && in_space {
                spaces.push(MmiAddressSpace {
                    name: std::mem::take(&mut cur_name),
                    proc_name: std::mem::take(&mut cur_proc),
                    addr_begin: cur_begin,
                    addr_end: cur_end,
                    bus_width: cur_width,
                    lanes: std::mem::take(&mut cur_lanes),
                });
                in_space = false;
            }
        }
        if spaces.is_empty() {
            return Err("mmi json: no address spaces found".into());
        }
        Ok(Self { part, spaces })
    }

    /// Auto-detect and parse either `.mmi` XML or `.mmi` JSON.
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim_start();
        if t.starts_with('{') {
            Self::from_json(t)
        } else {
            Self::from_xml(t)
        }
    }
}

fn xml_attr<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}=\"");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn json_str_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat)? + pat.len();
    let rest = line[start..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn json_num_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let start = line.find(&pat)? + pat.len();
    let rest = line[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_hexdigit() && c != 'x' && c != 'X')
        .unwrap_or(rest.len());
    if end == 0 {
        None
    } else {
        Some(&rest[..end])
    }
}

fn parse_u64_num(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("bad hex {s}: {e}"))
    } else {
        t.parse::<u64>().map_err(|e| format!("bad int {s}: {e}"))
    }
}

/// Contiguous loadable firmware segment extracted from an ELF (`PT_LOAD`) or `.mem` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareSegment {
    pub paddr: u64,
    pub vaddr: u64,
    pub data: Vec<u8>,
}

/// Parsed firmware image ready for `updatemem` BRAM frame splicing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareImage {
    pub entry: u64,
    pub is_64bit: bool,
    pub machine: u16,
    pub segments: Vec<FirmwareSegment>,
}

impl FirmwareImage {
    pub fn total_bytes(&self) -> usize {
        self.segments.iter().map(|s| s.data.len()).sum()
    }
}

/// Parse a little-endian ELF32 or ELF64 binary image and extract all `PT_LOAD` (`p_type == 1`) segments.
pub fn load_elf_firmware(bytes: &[u8]) -> Result<FirmwareImage, String> {
    if bytes.len() < 52 || &bytes[0..4] != b"\x7fELF" {
        return Err("updatemem: not an ELF binary (missing \\x7fELF magic)".into());
    }
    let ei_class = bytes[4];
    let ei_data = bytes[5];
    if ei_data != 1 {
        return Err(format!(
            "updatemem: unsupported ELF endianness {ei_data} (expected little-endian 1)"
        ));
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    match ei_class {
        1 => {
            // ELF32
            let entry = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as u64;
            let phoff = u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize;
            let phentsize = u16::from_le_bytes([bytes[42], bytes[43]]) as usize;
            let phnum = u16::from_le_bytes([bytes[44], bytes[45]]) as usize;
            if phentsize < 32 {
                return Err(format!("updatemem: invalid ELF32 phentsize {phentsize}"));
            }
            let mut segments = Vec::new();
            for i in 0..phnum {
                let off = phoff
                    .checked_add(i.checked_mul(phentsize).ok_or("ELF32 phdr overflow")?)
                    .ok_or("ELF32 phdr offset overflow")?;
                let ph = bytes
                    .get(off..off + 32)
                    .ok_or_else(|| "updatemem: truncated ELF32 program header".to_string())?;
                let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
                if p_type != 1 {
                    continue; // Only PT_LOAD (1)
                }
                let p_offset = u32::from_le_bytes(ph[4..8].try_into().unwrap()) as usize;
                let p_vaddr = u32::from_le_bytes(ph[8..12].try_into().unwrap()) as u64;
                let p_paddr = u32::from_le_bytes(ph[12..16].try_into().unwrap()) as u64;
                let p_filesz = u32::from_le_bytes(ph[16..20].try_into().unwrap()) as usize;
                let p_memsz = u32::from_le_bytes(ph[20..24].try_into().unwrap()) as usize;
                if p_memsz == 0 && p_filesz == 0 {
                    continue;
                }
                let file_slice = bytes
                    .get(p_offset..p_offset.saturating_add(p_filesz))
                    .ok_or_else(|| "updatemem: truncated ELF32 PT_LOAD segment data".to_string())?;
                let mut data = Vec::with_capacity(p_memsz.max(p_filesz));
                data.extend_from_slice(file_slice);
                if p_memsz > data.len() {
                    data.resize(p_memsz, 0);
                }
                segments.push(FirmwareSegment {
                    paddr: p_paddr,
                    vaddr: p_vaddr,
                    data,
                });
            }
            Ok(FirmwareImage {
                entry,
                is_64bit: false,
                machine,
                segments,
            })
        }
        2 => {
            // ELF64
            if bytes.len() < 64 {
                return Err("updatemem: truncated ELF64 header".into());
            }
            let entry = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
            let phoff = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
            let phentsize = u16::from_le_bytes([bytes[54], bytes[55]]) as usize;
            let phnum = u16::from_le_bytes([bytes[56], bytes[57]]) as usize;
            if phentsize < 56 {
                return Err(format!("updatemem: invalid ELF64 phentsize {phentsize}"));
            }
            let mut segments = Vec::new();
            for i in 0..phnum {
                let off = phoff
                    .checked_add(i.checked_mul(phentsize).ok_or("ELF64 phdr overflow")?)
                    .ok_or("ELF64 phdr offset overflow")?;
                let ph = bytes
                    .get(off..off + 56)
                    .ok_or_else(|| "updatemem: truncated ELF64 program header".to_string())?;
                let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
                if p_type != 1 {
                    continue; // Only PT_LOAD (1)
                }
                let p_offset = u64::from_le_bytes(ph[8..16].try_into().unwrap()) as usize;
                let p_vaddr = u64::from_le_bytes(ph[16..24].try_into().unwrap());
                let p_paddr = u64::from_le_bytes(ph[24..32].try_into().unwrap());
                let p_filesz = u64::from_le_bytes(ph[32..40].try_into().unwrap()) as usize;
                let p_memsz = u64::from_le_bytes(ph[40..48].try_into().unwrap()) as usize;
                if p_memsz == 0 && p_filesz == 0 {
                    continue;
                }
                let file_slice = bytes
                    .get(p_offset..p_offset.saturating_add(p_filesz))
                    .ok_or_else(|| "updatemem: truncated ELF64 PT_LOAD segment data".to_string())?;
                let mut data = Vec::with_capacity(p_memsz.max(p_filesz));
                data.extend_from_slice(file_slice);
                if p_memsz > data.len() {
                    data.resize(p_memsz, 0);
                }
                segments.push(FirmwareSegment {
                    paddr: p_paddr,
                    vaddr: p_vaddr,
                    data,
                });
            }
            Ok(FirmwareImage {
                entry,
                is_64bit: true,
                machine,
                segments,
            })
        }
        other => Err(format!("updatemem: unsupported ELF class {other}")),
    }
}

/// Build a valid RISC-V (`EM_RISCV = 243`) little-endian ELF32 executable with one `PT_LOAD` segment.
pub fn build_minimal_elf32(entry: u32, load_addr: u32, payload: &[u8]) -> Vec<u8> {
    let hdr_size = 52u32;
    let phdr_size = 32u32;
    let data_off = hdr_size + phdr_size;
    let filesz = payload.len() as u32;
    let mut out = Vec::with_capacity((data_off + filesz) as usize);
    out.extend_from_slice(b"\x7fELF");
    out.push(1); // EI_CLASS = ELFCLASS32
    out.push(1); // EI_DATA = ELFDATA2LSB
    out.push(1); // EI_VERSION
    out.resize(16, 0);
    out.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    out.extend_from_slice(&243u16.to_le_bytes()); // e_machine = EM_RISCV
    out.extend_from_slice(&1u32.to_le_bytes()); // e_version
    out.extend_from_slice(&entry.to_le_bytes()); // e_entry
    out.extend_from_slice(&hdr_size.to_le_bytes()); // e_phoff = 52
    out.extend_from_slice(&0u32.to_le_bytes()); // e_shoff = 0
    out.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    out.extend_from_slice(&(hdr_size as u16).to_le_bytes()); // e_ehsize = 52
    out.extend_from_slice(&(phdr_size as u16).to_le_bytes()); // e_phentsize = 32
    out.extend_from_slice(&1u16.to_le_bytes()); // e_phnum = 1
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // Program header (PT_LOAD = 1)
    out.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    out.extend_from_slice(&data_off.to_le_bytes()); // p_offset
    out.extend_from_slice(&load_addr.to_le_bytes()); // p_vaddr
    out.extend_from_slice(&load_addr.to_le_bytes()); // p_paddr
    out.extend_from_slice(&filesz.to_le_bytes()); // p_filesz
    out.extend_from_slice(&filesz.to_le_bytes()); // p_memsz
    out.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    out.extend_from_slice(&4u32.to_le_bytes()); // p_align = 4
    out.extend_from_slice(payload);
    out
}

/// Build a valid RISC-V (`EM_RISCV = 243`) little-endian ELF64 executable with one `PT_LOAD` segment.
pub fn build_minimal_elf64(entry: u64, load_addr: u64, payload: &[u8]) -> Vec<u8> {
    let hdr_size = 64u64;
    let phdr_size = 56u64;
    let data_off = hdr_size + phdr_size;
    let filesz = payload.len() as u64;
    let mut out = Vec::with_capacity((data_off + filesz) as usize);
    out.extend_from_slice(b"\x7fELF");
    out.push(2); // EI_CLASS = ELFCLASS64
    out.push(1); // EI_DATA = ELFDATA2LSB
    out.push(1); // EI_VERSION
    out.resize(16, 0);
    out.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    out.extend_from_slice(&243u16.to_le_bytes()); // e_machine = EM_RISCV
    out.extend_from_slice(&1u32.to_le_bytes()); // e_version
    out.extend_from_slice(&entry.to_le_bytes()); // e_entry
    out.extend_from_slice(&hdr_size.to_le_bytes()); // e_phoff = 64
    out.extend_from_slice(&0u64.to_le_bytes()); // e_shoff = 0
    out.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    out.extend_from_slice(&(hdr_size as u16).to_le_bytes()); // e_ehsize = 64
    out.extend_from_slice(&(phdr_size as u16).to_le_bytes()); // e_phentsize = 56
    out.extend_from_slice(&1u16.to_le_bytes()); // e_phnum = 1
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // ELF64 Program Header (PT_LOAD = 1)
    out.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    out.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    out.extend_from_slice(&data_off.to_le_bytes()); // p_offset
    out.extend_from_slice(&load_addr.to_le_bytes()); // p_vaddr
    out.extend_from_slice(&load_addr.to_le_bytes()); // p_paddr
    out.extend_from_slice(&filesz.to_le_bytes()); // p_filesz
    out.extend_from_slice(&filesz.to_le_bytes()); // p_memsz
    out.extend_from_slice(&8u64.to_le_bytes()); // p_align = 8
    out.extend_from_slice(payload);
    out
}

/// Parse a Vivado / Verilog `$readmemh` `.mem` hex firmware file into a [`FirmwareImage`].
///
/// Supports `@HEXADDR` origin directives, `//` line comments, and 8/16/32/64-bit hex tokens.
/// When `bytes_per_token == 0`, token byte width is inferred from token hex length (`<=2`->1B, `<=4`->2B, `<=8`->4B, else 8B).
pub fn load_mem_firmware(
    text: &str,
    default_base_addr: u64,
    bytes_per_token: usize,
) -> Result<FirmwareImage, String> {
    let mut segments: Vec<FirmwareSegment> = Vec::new();
    let mut cur_addr = default_base_addr;
    let mut cur_data: Vec<u8> = Vec::new();
    let mut seg_start = default_base_addr;

    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        for tok in line.split_whitespace() {
            if let Some(addr_hex) = tok.strip_prefix('@') {
                if !cur_data.is_empty() {
                    segments.push(FirmwareSegment {
                        paddr: seg_start,
                        vaddr: seg_start,
                        data: std::mem::take(&mut cur_data),
                    });
                }
                let addr = u64::from_str_radix(addr_hex.trim_start_matches("0x"), 16)
                    .map_err(|e| format!("updatemem .mem bad @address '{tok}': {e}"))?;
                cur_addr = addr;
                seg_start = addr;
                continue;
            }
            let clean = tok
                .strip_prefix("0x")
                .or_else(|| tok.strip_prefix("0X"))
                .unwrap_or(tok)
                .replace('_', "");
            let val = u64::from_str_radix(&clean, 16)
                .map_err(|e| format!("updatemem .mem bad hex token '{tok}': {e}"))?;
            let width = if bytes_per_token > 0 {
                bytes_per_token
            } else if clean.len() <= 2 {
                1
            } else if clean.len() <= 4 {
                2
            } else if clean.len() <= 8 {
                4
            } else {
                8
            };
            let le = val.to_le_bytes();
            cur_data.extend_from_slice(&le[..width.min(8)]);
            cur_addr += width as u64;
        }
    }
    if !cur_data.is_empty() {
        segments.push(FirmwareSegment {
            paddr: seg_start,
            vaddr: seg_start,
            data: cur_data,
        });
    }
    if segments.is_empty() {
        return Err("updatemem: .mem file contained no hex payload".into());
    }
    let entry = segments[0].paddr;
    Ok(FirmwareImage {
        entry,
        is_64bit: false,
        machine: 243,
        segments,
    })
}

/// Read back a 64-bit BRAM INIT word (`word_idx` in `0..255`) from `Far::BRAM` major `bram_major`.
pub fn readback_bram_word(bits: &Bitstream, bram_major: u16, word_idx: u8) -> u64 {
    let minor = 1u8.saturating_add(word_idx);
    bits.frames
        .get(&(Far::BRAM, bram_major, minor))
        .copied()
        .unwrap_or(0) as u64
}

/// Vivado UG898 `updatemem`: sub-10ms ELF/MEM-to-BRAM bitstream firmware splicer.
///
/// Maps `image` (`PT_LOAD` ELF segments or `.mem` segments) across the bit-lanes and
/// address ranges of `mmi` (matching `space_name`, `proc_name`, or the default space when
/// `space_name` is empty), patches `Far::BRAM` INIT frames in `bits` in-place, and
/// re-encodes `bits.packets` without re-running synthesis, placement, or routing.
/// Returns the number of modified/populated BRAM INIT frames.
pub fn updatemem(
    dev: &Device,
    bits: &mut Bitstream,
    mmi: &MemoryMapInfo,
    space_name: &str,
    image: &FirmwareImage,
) -> Result<usize, String> {
    if bits.idcode != dev.idcode {
        return Err(format!(
            "updatemem: idcode mismatch bitstream {:#010x} device {:#010x}",
            bits.idcode, dev.idcode
        ));
    }
    let space = if space_name.is_empty() {
        mmi.spaces.first()
    } else {
        mmi.spaces
            .iter()
            .find(|s| s.name == space_name || s.proc_name == space_name)
    }
    .ok_or_else(|| format!("updatemem: address space '{space_name}' not found in .mmi"))?;

    if space.lanes.is_empty() {
        return Err(format!(
            "updatemem: address space '{}' has no BRAM BitLanes",
            space.name
        ));
    }

    let bus_bytes = (space.bus_width.max(8) as u64) / 8;
    let mut touched_frames: std::collections::BTreeSet<(u8, u16, u8)> =
        std::collections::BTreeSet::new();

    for seg in &image.segments {
        if seg.data.is_empty() {
            continue;
        }
        let seg_end = seg
            .paddr
            .checked_add(seg.data.len() as u64 - 1)
            .ok_or("updatemem: segment address overflow")?;
        if seg.paddr < space.addr_begin || seg_end > space.addr_end {
            return Err(format!(
                "updatemem: segment [{:#010x}..={:#010x}] out of bounds for address space '{}' [{:#010x}..={:#010x}]",
                seg.paddr, seg_end, space.name, space.addr_begin, space.addr_end
            ));
        }

        for (byte_idx, &byte_val) in seg.data.iter().enumerate() {
            let byte_addr = seg.paddr + byte_idx as u64;
            let bus_bit_lo = ((byte_addr - space.addr_begin) % bus_bytes) as u8 * 8;
            let bus_bit_hi = bus_bit_lo + 7;

            for lane in &space.lanes {
                if byte_addr < lane.addr_begin || byte_addr > lane.addr_end {
                    continue;
                }
                if lane.msb < bus_bit_lo || lane.lsb > bus_bit_hi {
                    continue;
                }
                // Ensure BRAM USED frame (minor 0) is present for this BRAM major.
                let used_key = (Far::BRAM, lane.bram_major, 0);
                bits.frames.entry(used_key).or_insert_with(|| {
                    let (sx, sy) = helion_device::Site::parse_id(&lane.placed_site)
                        .map(|s| (s.x, s.y))
                        .unwrap_or((dev.bram_x, dev.bram_y0 + u32::from(lane.bram_major) * 2));
                    1u128 | ((sx as u128) << 8) | ((sy as u128) << 16)
                });

                // Compute bit overlap between this byte [bus_bit_lo..=bus_bit_hi] and lane [lane.lsb..=lane.msb].
                let ov_lo = bus_bit_lo.max(lane.lsb);
                let ov_hi = bus_bit_hi.min(lane.msb);
                let n_bits = ov_hi - ov_lo + 1;
                let mask = if n_bits >= 8 {
                    0xFFu64
                } else {
                    (1u64 << n_bits) - 1
                };
                let extracted = ((byte_val as u64) >> (ov_lo - bus_bit_lo)) & mask;

                let lane_width_bits = u64::from(lane.bit_width());
                let bus_word_idx = (byte_addr - lane.addr_begin) / bus_bytes;
                let lane_bit_offset = bus_word_idx * lane_width_bits + u64::from(ov_lo - lane.lsb);
                let bram_word_idx = (lane_bit_offset / 64) as u8;
                let bit_in_word = (lane_bit_offset % 64) as u32;

                let minor = 1u8
                    .checked_add(bram_word_idx)
                    .ok_or("updatemem: BRAM word index overflow (>254)")?;
                let frame_key = (Far::BRAM, lane.bram_major, minor);
                let cur = bits.frames.get(&frame_key).copied().unwrap_or(0) as u64;
                let updated = (cur & !(mask << bit_in_word)) | (extracted << bit_in_word);
                if updated == 0 {
                    bits.frames.remove(&frame_key);
                } else {
                    bits.frames.insert(frame_key, updated as u128);
                }
                touched_frames.insert(frame_key);
            }
        }
    }

    bits.packets = encode_packets(bits.idcode, &bits.frames);
    Ok(touched_frames.len())
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

    #[test]
    fn mmi_xml_and_json_roundtrip_and_from_routed() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.add_cell("u_bram0", CellKind::Bram18);
        d.add_cell("u_bram1", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();

        let mmi = MemoryMapInfo::from_routed(&dev, &r, "u_rv32_hb1", "imem", 0x0000_0000);
        assert_eq!(mmi.spaces.len(), 1);
        assert_eq!(mmi.spaces[0].lanes.len(), 2);
        assert_eq!(mmi.spaces[0].addr_begin, 0);
        assert_eq!(mmi.spaces[0].addr_end, 1023);

        let xml = mmi.to_xml();
        assert!(xml.contains("<MemInfo"));
        assert!(xml.contains("<BitLane"));
        let from_xml = MemoryMapInfo::parse(&xml).unwrap();
        assert_eq!(from_xml, mmi);

        let json = mmi.to_json();
        let from_json = MemoryMapInfo::parse(&json).unwrap();
        assert_eq!(from_json, mmi);
    }

    #[test]
    fn updatemem_splices_elf32_elf64_and_mem_in_sub_10ms() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.add_cell("u_bram0", CellKind::Bram18);
        d.add_cell("u_bram1", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let mut bs = bitgen(&dev, &r).unwrap();
        let clb_frames_before: BTreeMap<_, _> = bs
            .frames
            .iter()
            .filter(|((b, _, _), _)| *b != Far::BRAM)
            .map(|(k, v)| (*k, *v))
            .collect();

        let mmi = MemoryMapInfo::from_routed(&dev, &r, "u_rv32_hb1", "imem", 0x0000_0000);

        // 1. ELF32 PT_LOAD firmware splice (< 10 ms)
        let mut code = Vec::new();
        code.extend_from_slice(&0x0000_0513u32.to_le_bytes()); // li a0, 0
        code.extend_from_slice(&0x0015_0513u32.to_le_bytes()); // addi a0, a0, 1
        code.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        code.extend_from_slice(&0xCAFE_BABEu32.to_le_bytes());
        let elf32_bytes = build_minimal_elf32(0x0000_0000, 0x0000_0000, &code);
        let fw32 = load_elf_firmware(&elf32_bytes).unwrap();
        assert!(!fw32.is_64bit);
        assert_eq!(fw32.total_bytes(), 16);

        let t0 = std::time::Instant::now();
        let patched_frames = updatemem(&dev, &mut bs, &mmi, "imem", &fw32).unwrap();
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_millis() < 10,
            "updatemem must complete in < 10 ms, took {elapsed:?}"
        );
        assert_eq!(patched_frames, 2);
        assert_eq!(
            readback_bram_word(&bs, 0, 0),
            0x0015_0513_0000_0513u64,
            "word 0 must carry first two RV32 instructions"
        );
        assert_eq!(
            readback_bram_word(&bs, 0, 1),
            0xCAFE_BABE_DEAD_BEEFu64,
            "word 1 must carry next 64 bits"
        );

        // Non-BRAM CLB/IOB frames must be 100% untouched by updatemem!
        let clb_frames_after: BTreeMap<_, _> = bs
            .frames
            .iter()
            .filter(|((b, _, _), _)| *b != Far::BRAM)
            .map(|(k, v)| (*k, *v))
            .collect();
        assert_eq!(clb_frames_before, clb_frames_after);

        // 2. ELF64 PT_LOAD firmware splice into second BRAM (addr 0x200 = 512)
        let elf64_bytes =
            build_minimal_elf64(0x0000_0200, 0x0000_0200, &0x1122_3344_5566_7788u64.to_le_bytes());
        let fw64 = load_elf_firmware(&elf64_bytes).unwrap();
        assert!(fw64.is_64bit);
        updatemem(&dev, &mut bs, &mmi, "imem", &fw64).unwrap();
        assert_eq!(readback_bram_word(&bs, 1, 0), 0x1122_3344_5566_7788u64);

        // 3. Verilog .mem hex firmware loader + multi-lane bit slicing (32-bit low/high split lanes)
        let mem_text = "// Firmware image\n@0000\nA5A55A5A 12345678\n@0008\nF00DCAFE 89ABCDEF\n";
        let fw_mem = load_mem_firmware(mem_text, 0, 4).unwrap();
        let split_mmi = MemoryMapInfo {
            part: dev.part.clone(),
            spaces: vec![MmiAddressSpace {
                name: "split_bus".into(),
                proc_name: "u_rv32_hb1".into(),
                addr_begin: 0,
                addr_end: 511,
                bus_width: 64,
                lanes: vec![
                    MmiBitLane {
                        cell_name: "u_bram_lo".into(),
                        placed_site: "BRAM_X10Y1".into(),
                        bram_major: 0,
                        msb: 31,
                        lsb: 0,
                        addr_begin: 0,
                        addr_end: 511,
                        parity_bits: 0,
                    },
                    MmiBitLane {
                        cell_name: "u_bram_hi".into(),
                        placed_site: "BRAM_X10Y3".into(),
                        bram_major: 1,
                        msb: 63,
                        lsb: 32,
                        addr_begin: 0,
                        addr_end: 511,
                        parity_bits: 0,
                    },
                ],
            }],
        };
        updatemem(&dev, &mut bs, &split_mmi, "split_bus", &fw_mem).unwrap();
        // Low 32-bit lane (major 0) packs the two low 32-bit words (0xA5A55A5A and 0xF00DCAFE) into word 0!
        assert_eq!(readback_bram_word(&bs, 0, 0), 0xF00D_CAFE_A5A5_5A5Au64);
        // High 32-bit lane (major 1) packs the two high 32-bit words (0x12345678 and 0x89ABCDEF) into word 0!
        assert_eq!(readback_bram_word(&bs, 1, 0), 0x89AB_CDEF_1234_5678u64);

        // Decoded .hbits packet stream must preserve the spliced BRAM frames.
        let (_, decoded) = decode_packets(&bs.packets).unwrap();
        assert_eq!(decoded, bs.frames);
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
