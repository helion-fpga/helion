//! Cycle-accurate Helion-T fabric model (CLB LUT+FF + IOB + startup SM).

use helion_bits::Bitstream;
use helion_device::{Device, Far};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Stat {
    pub init: bool,
    pub done: bool,
    pub eos: bool,
    pub crc_err: bool,
    pub gwe: bool,
    pub gsr: bool,
    pub gts: bool,
}

impl Stat {
    fn reset() -> Self {
        Self {
            init: false,
            done: false,
            eos: false,
            crc_err: false,
            gwe: false,
            gsr: true,
            gts: true,
        }
    }
}

#[derive(Clone, Debug)]
struct ClbState {
    /// 8 BLE FF Q
    q: [bool; 8],
    lut_o: [bool; 8],
}

#[derive(Clone, Debug)]
pub struct Fabric {
    pub idcode: u32,
    pub clb_x0: u32,
    pub clb_y0: u32,
    pub interior_cols: u32,
    pub interior_rows: u32,
    pub n_ble: u32,
    pub clb_minors: u32,
    frames: BTreeMap<(u8, u16, u8), u128>,
    clbs: BTreeMap<(u32, u32), ClbState>,
    /// CLBs with any programmed frame; eval skips empty tiles.
    used: Vec<(u32, u32)>,
    /// IOB (x,y) -> pad output after GTS
    iobs: BTreeMap<(u32, u32), bool>,
    /// IOB (x,y) → (clb_x, clb_y, ble)
    iob_src: BTreeMap<(u32, u32), (u32, u32, u8)>,
    pub stat: Stat,
    cfg_steps: u32,
}

impl Fabric {
    pub fn new(dev: &Device) -> Self {
        let mut clbs = BTreeMap::new();
        for s in dev.clb_sites() {
            clbs.insert(
                (s.x, s.y),
                ClbState {
                    q: [false; 8],
                    lut_o: [false; 8],
                },
            );
        }
        let mut iobs = BTreeMap::new();
        for s in dev.iob_sites() {
            iobs.insert((s.x, s.y), false);
        }
        Self {
            idcode: dev.idcode,
            clb_x0: dev.clb_x0,
            clb_y0: dev.clb_y0,
            interior_cols: dev.interior_cols,
            interior_rows: dev.interior_rows,
            n_ble: dev.n_ble,
            clb_minors: dev.clb_minors,
            frames: BTreeMap::new(),
            clbs,
            used: Vec::new(),
            iobs,
            iob_src: BTreeMap::new(),
            stat: Stat::reset(),
            cfg_steps: 0,
        }
    }

    pub fn program(&mut self, bits: &Bitstream) -> Result<(), String> {
        if bits.idcode != self.idcode {
            return Err(format!(
                "idcode mismatch bitstream {:#010x} fabric {:#010x}",
                bits.idcode, self.idcode
            ));
        }
        self.frames = bits.frames.clone();
        self.stat = Stat::reset();
        self.cfg_steps = 0;
        self.iob_src.clear();
        self.used = self
            .clbs
            .keys()
            .copied()
            .filter(|&(x, y)| {
                let Some(major) = self.clb_major(x, y) else {
                    return false;
                };
                (0..self.clb_minors).any(|minor| {
                    self.frames
                        .get(&(Far::CLB_IO_CLK, major, minor as u8))
                        .copied()
                        .unwrap_or(0)
                        != 0
                })
            })
            .collect();
        for ((block, major, minor), word) in &self.frames {
            if *block == Far::IOB && *minor == 0 && (word & 1) == 1 {
                let ble = ((*word >> 1) & 7) as u8;
                let cy = ((*word >> 4) & 0xfff) as u32;
                let dx = *major as u32;
                let ix = self.clb_x0 + dx;
                let iy = self.clb_y0.saturating_sub(1);
                let cx = ix;
                let cy = if cy == 0 { iy + 1 } else { cy };
                self.iob_src.insert((ix, iy), (cx, cy, ble));
            }
        }
        // FF INIT from config
        let keys: Vec<(u32, u32)> = self.clbs.keys().copied().collect();
        for (x, y) in keys {
            for ble in 0..self.n_ble {
                let init = self.clb_feature_bit(x, y, &format!("BLE{ble}.FF.INIT"));
                if let Some(c) = self.clbs.get_mut(&(x, y)) {
                    c.q[ble as usize] = init;
                }
            }
        }
        Ok(())
    }

    pub fn finish_startup(&mut self) {
        // INIT_COMPLETE wait 16
        self.cfg_steps += 16;
        self.stat.init = true;
        // GTS_RELEASE wait 8
        self.cfg_steps += 8;
        self.stat.gts = false;
        // GSR_RELEASE wait 8
        self.cfg_steps += 8;
        self.stat.gsr = false;
        // GWE_RELEASE wait 1
        self.cfg_steps += 1;
        self.stat.gwe = true;
        self.stat.done = true;
        self.stat.eos = true;
        self.stat.crc_err = false;
    }

    fn clb_major(&self, x: u32, y: u32) -> Option<u16> {
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

    fn abs_feature(feature: &str) -> Option<u32> {
        // Same packer as helion-device FeatureMap::pack_clb
        // INIT n starts at n*64
        if let Some(rest) = feature.strip_prefix("BLE") {
            let (nstr, rest) = rest.split_once('.')?;
            let n: u32 = nstr.parse().ok()?;
            if let Some(idx) = rest.strip_prefix("LUT.INIT[") {
                let idx = idx.strip_suffix(']')?;
                let i: u32 = idx.parse().ok()?;
                return Some(n * 64 + i);
            }
            let mode_base = 512 + n * 5;
            match rest {
                "LUT.FRACTURE" => return Some(mode_base),
                "LUT.CARRY" => return Some(mode_base + 1),
                "DMUX[0]" => return Some(mode_base + 2),
                "DMUX[1]" => return Some(mode_base + 3),
                "OQ_MUX" => return Some(mode_base + 4),
                _ => {}
            }
            let ff_base = 512 + 40 + n * 5;
            match rest {
                "FF.USED" => return Some(ff_base),
                "FF.INIT" => return Some(ff_base + 1),
                "FF.SRVAL" => return Some(ff_base + 2),
                "FF.SYNC" => return Some(ff_base + 3),
                "FF.CLKINV" => return Some(ff_base + 4),
                _ => {}
            }
        }
        if let Some(rest) = feature.strip_prefix("IMUX[") {
            let (mstr, rest) = rest.split_once("][")?;
            let m: u32 = mstr.parse().ok()?;
            let b: u32 = rest.strip_suffix(']')?.parse().ok()?;
            return Some(512 + 40 + 40 + m * 5 + b);
        }
        None
    }

    pub fn frame_bit(&self, block: u8, major: u16, minor: u8, bit: u8) -> bool {
        let f = self.frames.get(&(block, major, minor)).copied().unwrap_or(0);
        (f >> bit) & 1 == 1
    }

    pub fn clb_feature_bit(&self, x: u32, y: u32, feature: &str) -> bool {
        let Some(major) = self.clb_major(x, y) else {
            return false;
        };
        let Some(abs) = Self::abs_feature(feature) else {
            return false;
        };
        let minor = (abs / 128) as u8;
        let bit = (abs % 128) as u8;
        self.frame_bit(Far::CLB_IO_CLK, major, minor, bit)
    }

    pub fn lut_init(&self, x: u32, y: u32, ble: u32) -> u64 {
        let mut init = 0u64;
        for i in 0..64u32 {
            if self.clb_feature_bit(x, y, &format!("BLE{ble}.LUT.INIT[{i}]")) {
                init |= 1u64 << i;
            }
        }
        init
    }

    /// Evaluate LUT6 with given 6-bit address (I0 = LSB).
    pub fn eval_lut(&self, x: u32, y: u32, ble: u32, addr: u8) -> bool {
        let init = self.lut_init(x, y, ble);
        (init >> (addr & 63)) & 1 == 1
    }

    fn imux_sel(&self, x: u32, y: u32, mux: u32) -> u8 {
        let mut s = 0u8;
        for b in 0..5u32 {
            if self.clb_feature_bit(x, y, &format!("IMUX[{mux}][{b}]")) {
                s |= 1 << b;
            }
        }
        s
    }

    pub fn bind_iob_to_clb_ble(&mut self, iob_x: u32, iob_y: u32, ble: u8) {
        self.iob_src
            .insert((iob_x, iob_y), (iob_x, iob_y + 1, ble));
    }

    fn q_at(&self, x: u32, y: u32, ble: u8) -> bool {
        self.clbs
            .get(&(x, y))
            .and_then(|c| c.q.get(ble as usize).copied())
            .unwrap_or(false)
    }

    fn lut_o_at(&self, x: u32, y: u32, ble: u8) -> bool {
        self.clbs
            .get(&(x, y))
            .and_then(|c| c.lut_o.get(ble as usize).copied())
            .unwrap_or(false)
    }

    /// IMUX[4:0]: 0-7 south BLE Q, 8-15 north BLE Q, 16-23 local BLE Q, 24-31 local LUT O.
    fn decode_imux(&self, x: u32, y: u32, sel: u8) -> bool {
        if sel < 8 {
            return self.q_at(x, y.saturating_sub(1), sel);
        }
        if sel < 16 {
            return self.q_at(x, y + 1, sel - 8);
        }
        if sel < 24 {
            return self.q_at(x, y, sel - 16);
        }
        self.lut_o_at(x, y, sel - 24)
    }

    fn eval_comb(&mut self) {
        let coords = self.used.clone();
        // Multi-pass so local LUT-O feedback (sel 24+k) settles.
        for _ in 0..8 {
            for (x, y) in &coords {
                let (x, y) = (*x, *y);
                for ble in 0..self.n_ble {
                    let mut addr = 0u8;
                    for pin in 0..6u32 {
                        let sel = self.imux_sel(x, y, ble * 8 + pin);
                        if self.decode_imux(x, y, sel) {
                            addr |= 1 << pin;
                        }
                    }
                    let o = self.eval_lut(x, y, ble, addr);
                    self.clbs.get_mut(&(x, y)).unwrap().lut_o[ble as usize] = o;
                }
            }
        }
    }

    fn tick_ff(&mut self) {
        if !self.stat.gwe || self.stat.gsr {
            return;
        }
        let coords = self.used.clone();
        for (x, y) in coords {
            for ble in 0..self.n_ble {
                if !self.clb_feature_bit(x, y, &format!("BLE{ble}.FF.USED")) {
                    continue;
                }
                let d = self.clbs[&(x, y)].lut_o[ble as usize];
                self.clbs.get_mut(&(x, y)).unwrap().q[ble as usize] = d;
            }
        }
    }

    fn eval_iob(&mut self) {
        if self.stat.gts {
            for v in self.iobs.values_mut() {
                *v = false;
            }
            return;
        }
        let srcs = self.iob_src.clone();
        for ((ix, iy), (cx, cy, ble)) in srcs {
            let q = self
                .clbs
                .get(&(cx, cy))
                .map(|c| c.q[ble as usize])
                .unwrap_or(false);
            self.iobs.insert((ix, iy), q);
        }
    }

    /// One user clock: combo LUT, then FF, then IOB.
    pub fn step_user(&mut self) {
        self.eval_comb();
        self.tick_ff();
        self.eval_iob();
    }

    pub fn led_at(&self, x: u32, y: u32) -> bool {
        self.iobs.get(&(x, y)).copied().unwrap_or(false)
    }

    pub fn ble_q(&self, x: u32, y: u32, ble: u32) -> bool {
        self.clbs
            .get(&(x, y))
            .map(|c| c.q[ble as usize])
            .unwrap_or(false)
    }

    /// Read programmed BRAM INIT word `addr` of BRAM major `idx` (not pack-only).
    pub fn bram_init_word(&self, idx: u16, addr: usize) -> u64 {
        let minor = 1u8 + addr as u8;
        self.frames
            .get(&(Far::BRAM, idx, minor))
            .copied()
            .unwrap_or(0) as u64
    }

    /// Overlay frames without wiping the rest of the die (DFX partial).
    pub fn program_partial(&mut self, bits: &Bitstream) -> Result<(), String> {
        if bits.idcode != self.idcode {
            return Err(format!(
                "idcode mismatch bitstream {:#010x} fabric {:#010x}",
                bits.idcode, self.idcode
            ));
        }
        for (k, w) in &bits.frames {
            self.frames.insert(*k, *w);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_bits::{assemble, FeatureSet};
    use helion_device::Device;

    #[test]
    fn empty_bitstream_startup_stat() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let bits = Bitstream::empty(&dev);
        let mut fab = Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        assert!(fab.stat.init, "INIT");
        assert!(fab.stat.done, "DONE");
        assert!(fab.stat.eos, "EOS");
        assert!(fab.stat.gwe, "GWE");
        assert!(!fab.stat.gsr, "GSR");
        assert!(!fab.stat.gts, "GTS");
        assert!(!fab.stat.crc_err, "CRC_ERR");
    }

    #[test]
    fn init_poke_changes_lut_and_sram() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut feats = FeatureSet::new();
        feats.set("CLB_X2Y1.BLE0.LUT.INIT[0]", true);
        let bits = assemble(&dev, &feats).unwrap();
        let mut fab = Fabric::new(&dev);
        fab.program(&bits).unwrap();
        let loc = dev.locate("CLB_X2Y1.BLE0.LUT.INIT[0]").unwrap();
        assert_eq!(loc.far.minor, 0);
        assert_eq!(loc.bit, 0);
        assert!(fab.frame_bit(loc.far.block_type, loc.far.major, loc.far.minor, loc.bit));
        assert!(fab.eval_lut(2, 1, 0, 0), "INIT[0]=1 => lut(addr0)=1");
        assert!(!fab.eval_lut(2, 1, 0, 1), "INIT[1]=0 => lut(addr1)=0");

        let mut feats0 = FeatureSet::new();
        feats0.set("CLB_X2Y1.BLE0.LUT.INIT[0]", false);
        let bits0 = assemble(&dev, &feats0).unwrap();
        fab.program(&bits0).unwrap();
        assert!(!fab.eval_lut(2, 1, 0, 0));
        assert!(!fab.frame_bit(loc.far.block_type, loc.far.major, loc.far.minor, loc.bit));
    }
}
