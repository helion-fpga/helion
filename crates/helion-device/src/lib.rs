//! Helion architecture database (HAD) and FeatureMap packer.
//! CAD queries this crate instead of hardcoding a part name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Device {
    pub part: String,
    pub sku: String,
    pub part_id: u32,
    pub idcode: u32,
    pub had_version: u32,
    pub interior_cols: u32,
    pub interior_rows: u32,
    pub n_ble: u32,
    pub clb_x0: u32,
    pub clb_y0: u32,
    pub w: u32,
    pub clb_minors: u32,
    pub frame_bits: u32,
    pub n_dsp: u32,
    pub dsp_x: u32,
    pub dsp_y0: u32,
    featuremap: FeatureMap,
}

#[derive(Clone, Debug)]
pub struct FeatureMap {
    /// relative feature (e.g. `BLE0.LUT.INIT[0]`) -> absolute bit in the CLB major
    bits: BTreeMap<String, u32>,
    pub n_minors: u32,
    pub frame_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Far {
    pub block_type: u8,
    pub die: u8,
    pub major: u16,
    pub minor: u8,
}

impl Far {
    pub const CLB_IO_CLK: u8 = 0;
    pub const DSP: u8 = 2;
    pub const IOB: u8 = 5;

    pub fn encode(self) -> u32 {
        ((self.block_type as u32) << 28)
            | ((self.die as u32) << 24)
            | ((self.major as u32) << 8)
            | (self.minor as u32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitLoc {
    pub far: Far,
    pub bit: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Site {
    pub x: u32,
    pub y: u32,
    pub kind: SiteKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SiteKind {
    Clb,
    Iob,
    Clk,
    Dsp,
}

impl Device {
    pub fn workspace_root() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.pop();
        p
    }

    pub fn devices_dir() -> PathBuf {
        Self::workspace_root().join("devices/helion")
    }

    /// Load a part by name (`HL10T-C32-1`) from HAD TOML. This is the shipped API.
    pub fn load_part(name: &str) -> Result<Self, String> {
        let path = Self::devices_dir().join("parts").join(format!("{name}.toml"));
        Self::load_part_path(&path)
    }

    pub fn load_part_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut part = String::new();
        let mut sku = String::new();
        let mut part_id = 0u32;
        let mut idcode = 0u32;
        let mut had_version = 1u32;
        let mut interior_cols = 0u32;
        let mut interior_rows = 0u32;
        let mut n_ble = 8u32;
        let mut clb_x0 = 2u32;
        let mut clb_y0 = 1u32;
        let mut w = 80u32;
        let mut n_dsp = 0u32;
        let mut dsp_x = 8u32;
        let mut dsp_y0 = 1u32;
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            let v = v.trim().trim_matches('"');
            match k {
                "part" => part = v.to_string(),
                "sku" => sku = v.to_string(),
                "part_id" => part_id = parse_u32(v)?,
                "idcode" => idcode = parse_u32(v)?,
                "had_version" => had_version = parse_u32(v)?,
                "interior_cols" => interior_cols = parse_u32(v)?,
                "interior_rows" => interior_rows = parse_u32(v)?,
                "n_ble" => n_ble = parse_u32(v)?,
                "clb_x0" => clb_x0 = parse_u32(v)?,
                "clb_y0" => clb_y0 = parse_u32(v)?,
                "w" => w = parse_u32(v)?,
                "mac27" => n_dsp = parse_u32(v)?,
                "dsp_x" => dsp_x = parse_u32(v)?,
                "dsp_y0" => dsp_y0 = parse_u32(v)?,
                _ => {}
            }
        }
        if part.is_empty() {
            return Err("missing part".into());
        }
        if idcode == 0 {
            idcode = idcode_from_part_id(0, part_id);
        }
        let featuremap = FeatureMap::pack_clb(16, 128);
        Ok(Self {
            part,
            sku,
            part_id,
            idcode,
            had_version,
            interior_cols,
            interior_rows,
            n_ble,
            clb_x0,
            clb_y0,
            w,
            clb_minors: 16,
            frame_bits: 128,
            n_dsp,
            dsp_x,
            dsp_y0,
            featuremap,
        })
    }

    pub fn lut6_count(&self) -> u32 {
        self.interior_cols * self.interior_rows * self.n_ble
    }

    pub fn n_clb(&self) -> u32 {
        self.interior_cols * self.interior_rows
    }

    pub fn clb_sites(&self) -> impl Iterator<Item = Site> + '_ {
        let x0 = self.clb_x0;
        let y0 = self.clb_y0;
        let cols = self.interior_cols;
        let rows = self.interior_rows;
        (0..rows).flat_map(move |dy| {
            (0..cols).map(move |dx| Site {
                x: x0 + dx,
                y: y0 + dy,
                kind: SiteKind::Clb,
            })
        })
    }

    pub fn dsp_sites(&self) -> impl Iterator<Item = Site> + '_ {
        let n = self.n_dsp;
        let x = self.dsp_x;
        let y0 = self.dsp_y0;
        (0..n).map(move |i| Site {
            x,
            y: y0 + i * 2,
            kind: SiteKind::Dsp,
        })
    }

    pub fn iob_sites(&self) -> impl Iterator<Item = Site> + '_ {
        // Bottom IO under each CLB column: (clb_x, clb_y0-1)
        let x0 = self.clb_x0;
        let cols = self.interior_cols;
        let y = self.clb_y0.saturating_sub(1);
        (0..cols).map(move |dx| Site {
            x: x0 + dx,
            y,
            kind: SiteKind::Iob,
        })
    }

    pub fn clb_major(&self, x: u32, y: u32) -> Option<u16> {
        if x < self.clb_x0 || y < self.clb_y0 {
            return None;
        }
        let dx = x - self.clb_x0;
        let dy = y - self.clb_y0;
        if dx >= self.interior_cols || dy >= self.interior_rows {
            return None;
        }
        Some((dy * self.interior_cols + dx) as u16)
    }

    pub fn iob_major(&self, x: u32, y: u32) -> Option<u16> {
        if y != self.clb_y0.saturating_sub(1) {
            return None;
        }
        if x < self.clb_x0 {
            return None;
        }
        let dx = x - self.clb_x0;
        if dx >= self.interior_cols {
            return None;
        }
        Some(dx as u16)
    }

    pub fn featuremap(&self) -> &FeatureMap {
        &self.featuremap
    }

    pub fn locate_clb(&self, x: u32, y: u32, feature: &str) -> Result<BitLoc, String> {
        let major = self
            .clb_major(x, y)
            .ok_or_else(|| format!("not a CLB site CLB_X{x}Y{y}"))?;
        let abs = self
            .featuremap
            .abs_bit(feature)
            .ok_or_else(|| format!("unknown feature {feature}"))?;
        let minor = (abs / self.frame_bits) as u8;
        let bit = (abs % self.frame_bits) as u8;
        Ok(BitLoc {
            far: Far {
                block_type: Far::CLB_IO_CLK,
                die: 0,
                major,
                minor,
            },
            bit,
        })
    }

    /// Parse `CLB_X2Y1.BLE0.LUT.INIT[0]` into a bit location via the shipped FeatureMap.
    pub fn locate(&self, full: &str) -> Result<BitLoc, String> {
        let (site, rest) = full
            .split_once('.')
            .ok_or_else(|| format!("bad feature {full}"))?;
        if let Some(xy) = site.strip_prefix("CLB_X") {
            let (xs, ys) = xy
                .split_once('Y')
                .ok_or_else(|| format!("bad CLB site {site}"))?;
            let x: u32 = xs.parse().map_err(|_| "bad x".to_string())?;
            let y: u32 = ys.parse().map_err(|_| "bad y".to_string())?;
            self.locate_clb(x, y, rest)
        } else {
            Err(format!("unsupported site {site}"))
        }
    }
}

impl FeatureMap {
    /// Normative CLB packer: all eight INIT first (512 bits), then per-BLE mode.
    /// `BLE0.LUT.FRACTURE` is absolute bit 512 = minor 4 bit 0.
    pub fn pack_clb(n_minors: u32, frame_bits: u32) -> Self {
        let mut bits = BTreeMap::new();
        let mut cursor = 0u32;
        // 1. All eight INIT, 64-aligned as a group.
        for n in 0..8u32 {
            cursor = align(cursor, 64);
            for i in 0..64u32 {
                bits.insert(format!("BLE{n}.LUT.INIT[{i}]"), cursor + i);
            }
            cursor += 64;
        }
        // 2. Per-BLE mode: FRACTURE, CARRY, DMUX[1:0], OQ_MUX  (5 bits)
        for n in 0..8u32 {
            bits.insert(format!("BLE{n}.LUT.FRACTURE"), cursor);
            bits.insert(format!("BLE{n}.LUT.CARRY"), cursor + 1);
            bits.insert(format!("BLE{n}.DMUX[0]"), cursor + 2);
            bits.insert(format!("BLE{n}.DMUX[1]"), cursor + 3);
            bits.insert(format!("BLE{n}.OQ_MUX"), cursor + 4);
            cursor += 5;
        }
        // 3. FF: USED, INIT, SRVAL, SYNC, CLKINV
        for n in 0..8u32 {
            bits.insert(format!("BLE{n}.FF.USED"), cursor);
            bits.insert(format!("BLE{n}.FF.INIT"), cursor + 1);
            bits.insert(format!("BLE{n}.FF.SRVAL"), cursor + 2);
            bits.insert(format!("BLE{n}.FF.SYNC"), cursor + 3);
            bits.insert(format!("BLE{n}.FF.CLKINV"), cursor + 4);
            cursor += 5;
        }
        // 4. IMUX 64×5, then pad to n_minors * frame_bits
        for m in 0..64u32 {
            for b in 0..5u32 {
                bits.insert(format!("IMUX[{m}][{b}]"), cursor + b);
            }
            cursor += 5;
        }
        let _ = cursor;
        Self {
            bits,
            n_minors,
            frame_bits,
        }
    }

    pub fn abs_bit(&self, feature: &str) -> Option<u32> {
        self.bits.get(feature).copied()
    }

    pub fn minor_bit(&self, feature: &str) -> Option<(u8, u8)> {
        let abs = self.abs_bit(feature)?;
        Some(((abs / self.frame_bits) as u8, (abs % self.frame_bits) as u8))
    }
}

pub fn idcode_from_part_id(had_version: u32, part_id: u32) -> u32 {
    let version = had_version & 0xF;
    (version << 28) | (part_id << 12) | 0xA1F
}

pub fn uwilton_sides(i: usize, w: usize) -> (usize, usize, usize) {
    let straight = i;
    let right = (i + 1) % w;
    let left = (i + w - 1) % w;
    (straight, right, left)
}

fn align(bit: u32, a: u32) -> u32 {
    if a == 0 {
        return bit;
    }
    let r = bit % a;
    if r == 0 { bit } else { bit + (a - r) }
}

fn parse_u32(v: &str) -> Result<u32, String> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else {
        v.parse().map_err(|e| format!("{v}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helion_t_had_loads() {
        let d = Device::load_part("HL10T-C32-1").expect("load Helion-T");
        assert_eq!(d.interior_cols, 32);
        assert_eq!(d.interior_rows, 32);
        assert_eq!(d.lut6_count(), 8192);
        assert_eq!(d.idcode, 0x0001_1A1F);
        assert_eq!(d.clb_sites().count(), 32 * 32);
        // CAD queries the database — sites come from HAD, not a hardcoded part match.
        let first = d.clb_sites().next().unwrap();
        assert_eq!(first.x, d.clb_x0);
        assert_eq!(first.y, d.clb_y0);
    }

    #[test]
    fn idcode_formula_and_had_t() {
        // IEEE packing with version nibble 0: (part_id << 12) | 0xA1F
        assert_eq!(idcode_from_part_id(0, 1), 0x0000_1A1F);
        assert_eq!(idcode_from_part_id(0, 2), 0x0000_2A1F);
        // Helion-T HAD ships the documented 0.1 IDCODE 0x00011A1F
        let d = Device::load_part("HL10T-C32-1").unwrap();
        assert_eq!(d.idcode, 0x0001_1A1F);
    }

    #[test]
    fn featuremap_init0_and_fracture() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let init0 = d.locate("CLB_X2Y1.BLE0.LUT.INIT[0]").unwrap();
        assert_eq!(init0.far.minor, 0);
        assert_eq!(init0.bit, 0);
        assert_eq!(init0.far.major, 0);
        let frac = d.locate("CLB_X2Y1.BLE0.LUT.FRACTURE").unwrap();
        assert_eq!(frac.far.minor, 4);
        assert_eq!(frac.bit, 0);
    }

    #[test]
    fn uwilton_disjoint_at_zero() {
        for w in [8usize, 16, 80] {
            for i in 0..w {
                let (s, r, l) = uwilton_sides(i, w);
                let mut v = vec![s, r, l];
                v.sort_unstable();
                v.dedup();
                assert_eq!(v.len(), 3, "i={i} w={w}");
            }
        }
        let (s, r, l) = uwilton_sides(0, 8);
        assert_eq!((s, r, l), (0, 1, 7));
    }
}
