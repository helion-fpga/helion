//! `.hbits` packets and feature assembly → frames.

use helion_device::{BitLoc, Device, Far};
use helion_route::Routed;
use std::collections::BTreeMap;

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
        for i in 0..64u32 {
            let b = (init >> i) & 1 == 1;
            self.set(format!("CLB_X{x}Y{y}.BLE{ble}.LUT.INIT[{i}]"), b);
        }
    }

    pub fn set_ff_used(&mut self, x: u32, y: u32, ble: u32, used: bool) {
        self.set(format!("CLB_X{x}Y{y}.BLE{ble}.FF.USED"), used);
    }

    pub fn set_imux(&mut self, x: u32, y: u32, mux: u32, sel: u8) {
        for b in 0..5u32 {
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
        let mut frames = BTreeMap::new();
        for major in 0..dev.n_clb() as u16 {
            for minor in 0..dev.clb_minors as u8 {
                frames.insert((Far::CLB_IO_CLK, major, minor), 0);
            }
        }
        let mut bs = Self {
            idcode: dev.idcode,
            frames,
            packets: Vec::new(),
        };
        bs.packets = encode_packets(dev.idcode, &bs.frames);
        bs
    }
}

/// Bitgen a routed design: every LUTFF INIT/FF, IMUX from PathFinder, IOB src, DSP/BRAM USED.
pub fn bitgen(dev: &Device, routed: &Routed) -> Result<Bitstream, String> {
    let mut feats = FeatureSet::new();
    for (i, lutff) in routed.placed.packed.lutffs.iter().enumerate() {
        let (site, ble) = routed.placed.lutff_sites[i];
        feats.set_init(site.x, site.y, ble as u32, lutff.init);
        feats.set_ff_used(site.x, site.y, ble as u32, true);
    }
    for m in &routed.imux {
        feats.set_imux(m.x, m.y, m.mux, m.sel);
    }
    let mut bs = assemble(dev, &feats)?;
    for r in &routed.iob_src {
        let major = dev
            .iob_major(r.iob.0, r.iob.1)
            .ok_or_else(|| format!("IOB_X{}Y{} has no major", r.iob.0, r.iob.1))?;
        // bit0 USED, [3:1] BLE, [15:4] CLB y
        let word = 1u128 | ((r.ble as u128) << 1) | ((r.clb.1 as u128) << 4);
        bs.frames.insert((Far::IOB, major, 0), word);
    }
    for (i, _m) in routed.placed.packed.macs.iter().enumerate() {
        let site = routed.placed.mac_sites[i];
        let word = 1u128 | ((site.x as u128) << 8) | ((site.y as u128) << 16);
        bs.frames.insert((Far::DSP, i as u16, 0), word);
    }
    for (i, _b) in routed.placed.packed.brams.iter().enumerate() {
        let site = routed.placed.bram_sites[i];
        let word = 1u128 | ((site.x as u128) << 8) | ((site.y as u128) << 16);
        bs.frames.insert((Far::BRAM, i as u16, 0), word);
    }
    bs.packets = encode_packets(dev.idcode, &bs.frames);
    Ok(bs)
}

pub fn assemble(dev: &Device, feats: &FeatureSet) -> Result<Bitstream, String> {
    let mut bs = Bitstream::empty(dev);
    for (name, val) in &feats.bits {
        if !*val {
            continue;
        }
        let loc: BitLoc = dev.locate(name)?;
        let key = (loc.far.block_type, loc.far.major, loc.far.minor);
        let frame = bs.frames.entry(key).or_insert(0);
        *frame |= 1u128 << loc.bit;
    }
    bs.packets = encode_packets(dev.idcode, &bs.frames);
    Ok(bs)
}

pub fn encode_packets(idcode: u32, frames: &BTreeMap<(u8, u16, u8), u128>) -> Vec<u8> {
    let mut body = Vec::new();
    // SYNC
    body.push(0x01);
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(b"HELI");
    let mut last_far: Option<u32> = None;
    let mut run: Vec<u128> = Vec::new();
    let mut keys: Vec<_> = frames.keys().copied().collect();
    keys.sort();
    for (block, major, minor) in keys {
        let far = Far {
            block_type: block,
            die: 0,
            major,
            minor,
        }
        .encode();
        let payload = frames[&(block, major, minor)];
        if let Some(prev) = last_far {
            if far == prev + 1 {
                run.push(payload);
                last_far = Some(far);
                continue;
            }
            flush_run(&mut body, prev + 1 - run.len() as u32, &run);
            run.clear();
        }
        run.push(payload);
        last_far = Some(far);
    }
    if !run.is_empty() {
        let start = last_far.unwrap() + 1 - run.len() as u32;
        flush_run(&mut body, start, &run);
    }
    // CRC_CHECK
    body.push(0x21);
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    // DESYNC
    body.push(0x02);
    body.extend_from_slice(&0u16.to_le_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(b"HBIT");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&idcode.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    let hash = sha256_lite(&body);
    out.extend_from_slice(&hash);
    let hdr_crc = crc32c(&out);
    out.extend_from_slice(&hdr_crc.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn flush_run(body: &mut Vec<u8>, far: u32, run: &[u128]) {
    body.push(0x10); // WRITE_FAR
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&far.to_le_bytes());
    let bytes = run.len() * 16;
    body.push(0x11); // WRITE_FDRI
    body.extend_from_slice(&(bytes as u16).to_le_bytes());
    for w in run {
        body.extend_from_slice(&w.to_le_bytes());
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
pub fn readback_lut_init(dev: &Device, bits: &Bitstream, x: u32, y: u32, ble: u32) -> Result<u64, String> {
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
pub fn eco_lut(dev: &Device, routed: &Routed, cell: &str, new_init: u64) -> Result<Bitstream, String> {
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
        return Err("pblock produced no frames".into());
    }
    let mut bs = Bitstream {
        idcode: dev.idcode,
        frames,
        packets: Vec::new(),
    };
    bs.packets = encode_packets(dev.idcode, &bs.frames);
    Ok(bs)
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
    use helion_ir::{CellKind, Design};
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
        assert!(pb.frames.len() < bs.frames.len(), "partial must be smaller");
        assert!(pb.frames.keys().all(|k| bs.frames.contains_key(k)));
    }
}
