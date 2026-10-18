//! Cycle-accurate Helion-T fabric model (CLB LUT+FF + IOB + startup SM).

pub use helion_bits::{
    Bitstream, FarRange, PBITS_FLAG_HAS_PBLOCK, PBITS_FLAG_RESET_AFTER_RECONFIG,
    PBITS_HEADER_BYTES, PartialBitstream, Pblock, bitgen_partial_pblock, decode_pbits,
    diff_partial_bitstream, encode_imux_hop, encode_pbits, patch_imux_sel, patch_lut_init,
    patch_lut_init_cell, readback_imux_sel, reroute_probe_eco,
};
use helion_device::{Device, Far};
use helion_route::Routed;
use std::collections::{BTreeMap, BTreeSet};

/// One bit of the Helion STAT TAP DR (`IR_STAT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatBit {
    pub bit: u8,
    pub name: &'static str,
    pub value: bool,
    pub description: &'static str,
}

/// Fabric status register (startup SM + CRC). Packed as a 32-bit TAP DR.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub const BIT_CRC_ERR: u8 = 0;
    pub const BIT_INIT: u8 = 1;
    pub const BIT_GTS: u8 = 2;
    pub const BIT_GSR: u8 = 3;
    pub const BIT_GWE: u8 = 4;
    pub const BIT_DONE: u8 = 5;
    pub const BIT_EOS: u8 = 6;
    /// Unconfigured: GTS and GSR asserted.
    pub const RESET_WORD: u32 = (1 << Self::BIT_GTS) | (1 << Self::BIT_GSR);
    /// After `finish_startup`: INIT | GWE | DONE | EOS.
    pub const STARTUP_WORD: u32 =
        (1 << Self::BIT_INIT) | (1 << Self::BIT_GWE) | (1 << Self::BIT_DONE) | (1 << Self::BIT_EOS);

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

    /// Helion STAT TAP DR word (bit 0 = CRC_ERR … bit 6 = EOS).
    pub fn word(&self) -> u32 {
        (u32::from(self.crc_err) << Self::BIT_CRC_ERR)
            | (u32::from(self.init) << Self::BIT_INIT)
            | (u32::from(self.gts) << Self::BIT_GTS)
            | (u32::from(self.gsr) << Self::BIT_GSR)
            | (u32::from(self.gwe) << Self::BIT_GWE)
            | (u32::from(self.done) << Self::BIT_DONE)
            | (u32::from(self.eos) << Self::BIT_EOS)
    }

    pub fn bits(&self) -> [StatBit; 7] {
        [
            StatBit {
                bit: Self::BIT_CRC_ERR,
                name: "CRC_ERR",
                value: self.crc_err,
                description: "configuration CRC error",
            },
            StatBit {
                bit: Self::BIT_INIT,
                name: "INIT",
                value: self.init,
                description: "INIT complete",
            },
            StatBit {
                bit: Self::BIT_GTS,
                name: "GTS",
                value: self.gts,
                description: "global tri-state (I/O held)",
            },
            StatBit {
                bit: Self::BIT_GSR,
                name: "GSR",
                value: self.gsr,
                description: "global set/reset (FFs held)",
            },
            StatBit {
                bit: Self::BIT_GWE,
                name: "GWE",
                value: self.gwe,
                description: "global write enable",
            },
            StatBit {
                bit: Self::BIT_DONE,
                name: "DONE",
                value: self.done,
                description: "configuration done",
            },
            StatBit {
                bit: Self::BIT_EOS,
                name: "EOS",
                value: self.eos,
                description: "end of startup",
            },
        ]
    }
}

#[derive(Clone, Debug)]
struct ClbState {
    /// 8 BLE FF Q
    q: [bool; 8],
    lut_o: [bool; 8],
}

/// Match kind programmed into the fabric ILA trigger FSM / match unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IlaMatchKind {
    Immediate = 0,
    Rising = 1,
    Falling = 2,
}

impl IlaMatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Rising => "rising",
            Self::Falling => "falling",
        }
    }
}

/// UG908-class trigger FSM resident in fabric (Idle → Armed → Fired → Done).
///
/// Match decisions for Rising/Falling come from the bitstream match-unit LUT
/// (`refresh_comb` + `ble_out` of `ila_match_hit`), not host compare of probe
/// `ble_out` history. Immediate fires on the first armed sample.
#[derive(Clone, Debug)]
pub struct IlaTriggerFsm {
    pub kind: IlaMatchKind,
    pub armed: bool,
    pub fired: bool,
    pub done: bool,
    pub pre_trigger: usize,
    pub post_left: usize,
    pub window: usize,
    pub first: bool,
}

impl Default for IlaTriggerFsm {
    fn default() -> Self {
        Self {
            kind: IlaMatchKind::Immediate,
            armed: false,
            fired: false,
            done: false,
            pre_trigger: 0,
            post_left: 0,
            window: 0,
            first: true,
        }
    }
}

impl IlaTriggerFsm {
    pub fn arm(kind: IlaMatchKind, window: usize, pre_trigger: usize) -> Self {
        let window = window.max(1);
        let pre = if kind == IlaMatchKind::Immediate {
            0
        } else {
            pre_trigger.min(window.saturating_sub(1))
        };
        let post_after_trig = window - pre; // includes trigger sample
        Self {
            kind,
            armed: true,
            fired: false,
            done: false,
            pre_trigger: pre,
            post_left: post_after_trig.saturating_sub(1),
            window,
            first: true,
        }
    }

    /// Advance on one fabric sample. `match_hit` is the match-unit LUT output
    /// (Rising/Falling) or ignored for Immediate (fires on first armed sample).
    /// Returns true when the capture window is complete.
    pub fn on_sample(&mut self, match_hit: bool) -> bool {
        if self.done || !self.armed {
            return self.done;
        }
        if !self.fired {
            let fire = match self.kind {
                IlaMatchKind::Immediate => self.first,
                IlaMatchKind::Rising | IlaMatchKind::Falling => match_hit,
            };
            if fire {
                self.fired = true;
                if self.post_left == 0 {
                    self.done = true;
                }
            }
        } else if self.post_left == 0 {
            self.done = true;
        } else {
            self.post_left -= 1;
            if self.post_left == 0 {
                self.done = true;
            }
        }
        self.first = false;
        self.done
    }
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
    /// Runtime BRAM data plane (capture RAM / dual-port writes). Seeded empty on program;
    /// falls back to INIT frames via `bram_init_word` when unread.
    bram_data: BTreeMap<u16, Vec<u64>>,
    /// Optional ILA trigger FSM (armed by deep capture path).
    pub ila_fsm: IlaTriggerFsm,
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
            bram_data: BTreeMap::new(),
            ila_fsm: IlaTriggerFsm::default(),
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
        self.bram_data.clear();
        self.ila_fsm = IlaTriggerFsm::default();
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
            // Bits 0..4 keep gold abs (m*5+b). Bit 5/6/7 live in extension
            // banks after 64×5 so legacy frames stay bit-compatible.
            if b < 5 {
                return Some(512 + 40 + 40 + m * 5 + b);
            }
            if b == 5 {
                return Some(512 + 40 + 40 + 64 * 5 + m);
            }
            if b == 6 {
                return Some(512 + 40 + 40 + 64 * 5 + 64 + m);
            }
            if b == 7 {
                return Some(512 + 40 + 40 + 64 * 5 + 64 + 64 + m);
            }
            return None;
        }
        None
    }

    pub fn frame_bit(&self, block: u8, major: u16, minor: u8, bit: u8) -> bool {
        let f = self
            .frames
            .get(&(block, major, minor))
            .copied()
            .unwrap_or(0);
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
        for b in 0..8u32 {
            if self.clb_feature_bit(x, y, &format!("IMUX[{mux}][{b}]")) {
                s |= 1 << b;
            }
        }
        s
    }

    pub fn bind_iob_to_clb_ble(&mut self, iob_x: u32, iob_y: u32, ble: u8) {
        self.iob_src.insert((iob_x, iob_y), (iob_x, iob_y + 1, ble));
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

    /// IMUX sel: 0-7 S±1 Q, 8-15 N±1 Q, 16-23 local Q, 24-31 local LUT O,
    /// 32-39 S±2 Q, 40-47 N±2 Q, 48-55 W±1 Q, 56-63 E±1 Q,
    /// 64-71 W±2 Q, 72-79 E±2 Q,
    /// 80-87 SW±1 Q, 88-95 SE±1 Q, 96-103 NW±1 Q, 104-111 NE±1 Q,
    /// 112-119 S±3 Q, 120-127 N±3 Q,
    /// 128-135 W2S1, 136-143 W2N1, 144-151 E2S1, 152-159 E2N1,
    /// 160-167 W1S2, 168-175 W1N2, 176-183 E1S2, 184-191 E1N2,
    /// 192-199 W±3 Q, 200-207 E±3 Q,
    /// 208-215 SW2, 216-223 SE2, 224-231 NW2, 232-239 NE2,
    /// 240-247 S±4 Q, 248-255 N±4 Q
    /// (8-bit sel; gold uses sel<32).
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
        if sel < 32 {
            return self.lut_o_at(x, y, sel - 24);
        }
        if sel < 40 {
            return self.q_at(x, y.saturating_sub(2), sel - 32);
        }
        if sel < 48 {
            return self.q_at(x, y + 2, sel - 40);
        }
        if sel < 56 {
            // west neighbor Q (driver west of sink)
            return self.q_at(x.saturating_sub(1), y, sel - 48);
        }
        if sel < 64 {
            // east neighbor Q
            return self.q_at(x + 1, y, sel - 56);
        }
        if sel < 72 {
            // west ±2 Q (driver two columns west) — bit6 bank
            return self.q_at(x.saturating_sub(2), y, sel - 64);
        }
        if sel < 80 {
            // east ±2 Q
            return self.q_at(x + 2, y, sel - 72);
        }
        if sel < 88 {
            // SW diagonal (±1,±1): driver west+south of sink
            return self.q_at(x.saturating_sub(1), y.saturating_sub(1), sel - 80);
        }
        if sel < 96 {
            // SE diagonal: driver east+south
            return self.q_at(x + 1, y.saturating_sub(1), sel - 88);
        }
        if sel < 104 {
            // NW diagonal: driver west+north
            return self.q_at(x.saturating_sub(1), y + 1, sel - 96);
        }
        if sel < 112 {
            // NE diagonal: driver east+north
            return self.q_at(x + 1, y + 1, sel - 104);
        }
        // N-S ±3 (was reserved 112-127): fabric samples Q three rows away.
        if sel < 120 {
            return self.q_at(x, y.saturating_sub(3), sel - 112);
        }
        if sel < 128 {
            return self.q_at(x, y + 3, sel - 120);
        }
        // Knight moves (bit7 bank): (±2,±1) and (±1,±2)
        if sel < 136 {
            // W2S1: driver west±2 + south±1
            return self.q_at(x.saturating_sub(2), y.saturating_sub(1), sel - 128);
        }
        if sel < 144 {
            // W2N1
            return self.q_at(x.saturating_sub(2), y + 1, sel - 136);
        }
        if sel < 152 {
            // E2S1
            return self.q_at(x + 2, y.saturating_sub(1), sel - 144);
        }
        if sel < 160 {
            // E2N1
            return self.q_at(x + 2, y + 1, sel - 152);
        }
        if sel < 168 {
            // W1S2
            return self.q_at(x.saturating_sub(1), y.saturating_sub(2), sel - 160);
        }
        if sel < 176 {
            // W1N2
            return self.q_at(x.saturating_sub(1), y + 2, sel - 168);
        }
        if sel < 184 {
            // E1S2
            return self.q_at(x + 1, y.saturating_sub(2), sel - 176);
        }
        if sel < 192 {
            // E1N2
            return self.q_at(x + 1, y + 2, sel - 184);
        }
        // E-W ±3 (192-207)
        if sel < 200 {
            return self.q_at(x.saturating_sub(3), y, sel - 192);
        }
        if sel < 208 {
            return self.q_at(x + 3, y, sel - 200);
        }
        // Diagonal ±2 (208-239)
        if sel < 216 {
            // SW2
            return self.q_at(x.saturating_sub(2), y.saturating_sub(2), sel - 208);
        }
        if sel < 224 {
            // SE2
            return self.q_at(x + 2, y.saturating_sub(2), sel - 216);
        }
        if sel < 232 {
            // NW2
            return self.q_at(x.saturating_sub(2), y + 2, sel - 224);
        }
        if sel < 240 {
            // NE2
            return self.q_at(x + 2, y + 2, sel - 232);
        }
        // N-S ±4 (240-255); remaining u8 values are 248-255.
        if sel < 248 {
            return self.q_at(x, y.saturating_sub(4), sel - 240);
        }
        self.q_at(x, y + 4, sel.wrapping_sub(248))
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

    /// Re-evaluate combo LUTs without ticking FFs — used so the ILA match-unit
    /// LUT sees post-`step_user` Q values (delay FF = prev, probe = cur).
    pub fn refresh_comb(&mut self) {
        self.eval_comb();
    }

    /// Arm the fabric-resident ILA trigger FSM (deep path).
    pub fn ila_arm_fsm(&mut self, kind: IlaMatchKind, window: usize, pre_trigger: usize) {
        self.ila_fsm = IlaTriggerFsm::arm(kind, window, pre_trigger);
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
            // Registered BLE: pad follows FF Q. Comb BLE (FF.USED=0): pad follows LUT O.
            let v = if self.clb_feature_bit(cx, cy, &format!("BLE{ble}.FF.USED")) {
                self.q_at(cx, cy, ble)
            } else {
                self.lut_o_at(cx, cy, ble)
            };
            self.iobs.insert((ix, iy), v);
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

    /// UG900 force/deposit poke on an IOB pad (after `step_user`).
    pub fn set_led_at(&mut self, x: u32, y: u32, v: bool) {
        self.iobs.insert((x, y), v);
    }

    pub fn ble_q(&self, x: u32, y: u32, ble: u32) -> bool {
        self.clbs
            .get(&(x, y))
            .map(|c| c.q[ble as usize])
            .unwrap_or(false)
    }

    /// Registered BLE → FF Q; comb-only BLE (FF.USED=0) → LUT O (same rule as IOB pads).
    pub fn ble_out(&self, x: u32, y: u32, ble: u32) -> bool {
        if self.clb_feature_bit(x, y, &format!("BLE{ble}.FF.USED")) {
            self.ble_q(x, y, ble)
        } else {
            self.lut_o_at(x, y, ble as u8)
        }
    }

    /// UG900 force/deposit poke on a BLE FF Q; IOBs sourced from that BLE follow.
    pub fn set_ble_q(&mut self, x: u32, y: u32, ble: u32, v: bool) {
        if let Some(c) = self.clbs.get_mut(&(x, y)) {
            if let Some(q) = c.q.get_mut(ble as usize) {
                *q = v;
            }
        }
        let srcs: Vec<(u32, u32)> = self
            .iob_src
            .iter()
            .filter_map(|(&(ix, iy), &(cx, cy, b))| {
                if cx == x && cy == y && u32::from(b) == ble {
                    Some((ix, iy))
                } else {
                    None
                }
            })
            .collect();
        for (ix, iy) in srcs {
            self.iobs.insert((ix, iy), v);
        }
    }

    /// Read programmed BRAM INIT word `addr` of BRAM major `idx` (not pack-only).
    pub fn bram_init_word(&self, idx: u16, addr: usize) -> u64 {
        let minor = 1u8 + addr as u8;
        self.frames
            .get(&(Far::BRAM, idx, minor))
            .copied()
            .unwrap_or(0) as u64
    }

    /// Runtime write into BRAM major `idx` (fabric capture RAM / sample buffer).
    pub fn bram_write_word(&mut self, idx: u16, addr: usize, val: u64) {
        let bank = self.bram_data.entry(idx).or_default();
        if bank.len() <= addr {
            bank.resize(addr + 1, 0);
        }
        bank[addr] = val;
    }

    /// Runtime read: prefers `bram_write_word` data, else programmed INIT.
    pub fn bram_read_word(&self, idx: u16, addr: usize) -> u64 {
        if let Some(bank) = self.bram_data.get(&idx) {
            if let Some(&w) = bank.get(addr) {
                return w;
            }
        }
        self.bram_init_word(idx, addr)
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
        Ok(())
    }

    pub fn frame_word(&self, block: u8, major: u16, minor: u8) -> u128 {
        self.frames
            .get(&(block, major, minor))
            .copied()
            .unwrap_or(0)
    }
    /// Hot-load a differential or Pblock-scoped `.pbits` stream into a live `Fabric`.
    ///
    /// - Scrubs all frames covered by `pbits.far_ranges` before applying `pbits.frames`
    ///   (or applies explicit `0` frame words), so stale `RM_A` configuration inside the
    ///   reconfigured `Pblock` is cleanly cleared when swapping to `RM_B`.
    /// - Strictly preserves all static region flip-flop (`q`) states and runtime `bram_data`.
    /// - When `pbits.reset_after_reconfig` is true (Vivado UG909 `RESET_AFTER_RECONFIG`),
    ///   pulses local GSR inside the reconfigured `Pblock` (re-initializing only the RM's
    ///   BLE FFs to their new `BLE{n}.FF.INIT` values while static FFs continue undisturbed).
    pub fn program_pbits(&mut self, pbits: &PartialBitstream) -> Result<(), String> {
        if pbits.idcode != self.idcode {
            return Err(format!(
                "idcode mismatch .pbits {:#010x} fabric {:#010x}",
                pbits.idcode, self.idcode
            ));
        }
        let mut touched_clb_majors: BTreeSet<u16> = BTreeSet::new();
        for r in &pbits.far_ranges {
            for maj in r.major_start..=r.major_end {
                if r.block_type == Far::CLB_IO_CLK {
                    touched_clb_majors.insert(maj);
                }
                for min in r.minor_start..=r.minor_end {
                    self.frames.remove(&(r.block_type, maj, min));
                }
            }
        }
        for (&(block, major, minor), &word) in &pbits.frames {
            if block == Far::CLB_IO_CLK {
                touched_clb_majors.insert(major);
            }
            if word == 0 {
                self.frames.remove(&(block, major, minor));
            } else {
                self.frames.insert((block, major, minor), word);
            }
            if block == Far::IOB && minor == 0 {
                let dx = major as u32;
                let ix = self.clb_x0 + dx;
                let iy = self.clb_y0.saturating_sub(1);
                if (word & 1) == 1 {
                    let ble = ((word >> 1) & 7) as u8;
                    let cy = ((word >> 4) & 0xfff) as u32;
                    let cx = ix;
                    let cy = if cy == 0 { iy + 1 } else { cy };
                    self.iob_src.insert((ix, iy), (cx, cy, ble));
                } else {
                    self.iob_src.remove(&(ix, iy));
                }
            }
        }
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

        if pbits.reset_after_reconfig {
            let clb_keys: Vec<(u32, u32)> = self.clbs.keys().copied().collect();
            for (x, y) in clb_keys {
                let in_target = if let Some((x0, y0, x1, y1)) = pbits.pblock_bounds {
                    x >= x0 && x <= x1 && y >= y0 && y <= y1
                } else if let Some(maj) = self.clb_major(x, y) {
                    touched_clb_majors.contains(&maj)
                } else {
                    false
                };
                if !in_target {
                    continue;
                }
                for ble in 0..self.n_ble {
                    let init = self.clb_feature_bit(x, y, &format!("BLE{ble}.FF.INIT"));
                    if let Some(c) = self.clbs.get_mut(&(x, y)) {
                        c.q[ble as usize] = init;
                        c.lut_o[ble as usize] = false;
                    }
                }
            }
        }
        self.eval_comb();
        self.eval_iob();
        Ok(())
    }

    /// Perform a guarded DFX live partial reconfiguration swap on `partition`.
    ///
    /// Verifies `HDPR-8` (the partition's [`DfxDecoupler`] must be engaged during active
    /// reconfiguration and cover all boundary [`PartPin`]s) before hot-loading `pbits`.
    pub fn program_dfx_swap(
        &mut self,
        partition: &ReconfigPartition,
        pbits: &PartialBitstream,
    ) -> Result<(), String> {
        partition.decoupler.verify_active_isolation(&partition.partpins)?;
        self.program_pbits(pbits)
    }

    /// Step one user clock while applying [`DfxDecoupler`] boundary clamping when engaged.
    ///
    /// If `decoupler.engaged` is true, any RM-to-static or static-to-RM boundary signal
    /// transition that differs from the decoupled isolation level increments
    /// `decoupler.suppressed_glitches` and clamps the boundary node to its isolated value.
    pub fn step_user_with_decoupler(
        &mut self,
        partpins: &[PartPin],
        decoupler: &mut DfxDecoupler,
    ) {
        self.eval_comb();
        if decoupler.engaged {
            for pp in partpins {
                let raw_val = self.ble_out(pp.anchor_x, pp.anchor_y, u32::from(pp.ble));
                let iso_val = decoupler.isolated_value_for(&pp.net, raw_val);
                if raw_val != iso_val {
                    decoupler.suppressed_glitches += 1;
                }
                if let Some(c) = self.clbs.get_mut(&(pp.anchor_x, pp.anchor_y)) {
                    c.lut_o[pp.ble as usize] = iso_val;
                }
            }
        }
        self.tick_ff();
        if decoupler.engaged {
            for pp in partpins {
                let cur_q = self.ble_q(pp.anchor_x, pp.anchor_y, u32::from(pp.ble));
                let iso_val = decoupler.isolated_value_for(&pp.net, cur_q);
                if cur_q != iso_val {
                    decoupler.suppressed_glitches += 1;
                    self.set_ble_q(pp.anchor_x, pp.anchor_y, u32::from(pp.ble), iso_val);
                }
            }
        }
        self.eval_iob();
    }

    /// Sub-millisecond live LUT6 `INIT` patch directly on the running `Fabric` frames.
    pub fn patch_lut_init_live(
        &mut self,
        x: u32,
        y: u32,
        ble: u32,
        new_init: u64,
    ) -> Result<(), String> {
        let major = self
            .clb_major(x, y)
            .ok_or_else(|| format!("not a CLB site CLB_X{x}Y{y}"))?;
        if ble >= self.n_ble || ble >= 8 {
            return Err(format!("BLE {ble} out of range at CLB_X{x}Y{y}"));
        }
        let abs0 = Self::abs_feature(&format!("BLE{ble}.LUT.INIT[0]"))
            .ok_or_else(|| format!("unknown feature BLE{ble}.LUT.INIT[0]"))?;
        let minor = (abs0 / 128) as u8;
        let bit0 = abs0 % 128;
        let key = (Far::CLB_IO_CLK, major, minor);
        let clear_mask: u128 = (u64::MAX as u128) << bit0;
        let set_mask: u128 = (new_init as u128) << bit0;
        let cur = self.frames.get(&key).copied().unwrap_or(0);
        let updated = (cur & !clear_mask) | set_mask;
        if updated == 0 {
            self.frames.remove(&key);
        } else {
            self.frames.insert(key, updated);
            if !self.used.contains(&(x, y)) {
                self.used.push((x, y));
            }
        }
        self.eval_comb();
        self.eval_iob();
        Ok(())
    }

    /// Sub-millisecond live probe ECO reroute directly on the running `Fabric` frames.
    pub fn reroute_probe_eco_live(
        &mut self,
        src_ble: (u32, u32, u8),
        dst_target: (u32, u32, u32),
    ) -> Result<u8, String> {
        let (sx, sy, dble) = src_ble;
        if self.clb_major(sx, sy).is_none() {
            return Err(format!("probe source CLB_X{sx}Y{sy} is not a legal CLB tile"));
        }
        if u32::from(dble) >= self.n_ble {
            return Err(format!("probe source BLE {dble} out of range"));
        }
        let (dx, dy, dst_mux) = dst_target;
        if dy == self.clb_y0.saturating_sub(1) {
            if dx < self.clb_x0 || dx >= self.clb_x0 + self.interior_cols {
                return Err(format!("IOB_X{dx}Y{dy} is not a legal IOB site"));
            }
            let major = (dx - self.clb_x0) as u16;
            let key = (Far::IOB, major, 0);
            let prev_elec = self.frames.get(&key).copied().unwrap_or(0) & !0xFFFFu128;
            let word = 1u128 | ((dble as u128) << 1) | ((sy as u128) << 4) | prev_elec;
            self.frames.insert(key, word);
            self.iob_src.insert((dx, dy), (sx, sy, dble));
            self.eval_iob();
            return Ok(dble);
        }
        let major = self
            .clb_major(dx, dy)
            .ok_or_else(|| format!("not a CLB site CLB_X{dx}Y{dy}"))?;
        if dst_mux >= 64 {
            return Err(format!("IMUX {dst_mux} out of range at CLB_X{dx}Y{dy}"));
        }
        let sel = encode_imux_hop(sx, sy, dble, dx, dy)?;
        for b in 0..8u32 {
            let abs = Self::abs_feature(&format!("IMUX[{dst_mux}][{b}]"))
                .ok_or_else(|| format!("unknown feature IMUX[{dst_mux}][{b}]"))?;
            let minor = (abs / 128) as u8;
            let bit = abs % 128;
            let key = (Far::CLB_IO_CLK, major, minor);
            let cur = self.frames.get(&key).copied().unwrap_or(0);
            let updated = if (sel >> b) & 1 == 1 {
                cur | (1u128 << bit)
            } else {
                cur & !(1u128 << bit)
            };
            if updated == 0 {
                self.frames.remove(&key);
            } else {
                self.frames.insert(key, updated);
                if !self.used.contains(&(dx, dy)) {
                    self.used.push((dx, dy));
                }
            }
        }
        self.eval_comb();
        self.eval_iob();
        Ok(sel)
    }
}


/// Direction of a DFX `PARTPIN` boundary interface pin between the static shell and an RM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartPinDir {
    StaticToRm,
    RmToStatic,
}

/// Vivado UG909 Partition Pin (`HD.PARTPIN_LOCS` / `PARTPIN`) boundary anchor node.
///
/// Locks the interface tile `(anchor_x, anchor_y)`, source `(src_x, src_y, ble)`,
/// and sink IMUX `(mux, sel)` across all swappable Reconfigurable Modules (`RM_A`, `RM_B`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartPin {
    pub net: String,
    pub dir: PartPinDir,
    /// Interface tile coordinate on the `Pblock` boundary ring.
    pub anchor_x: u32,
    pub anchor_y: u32,
    /// Source BLE index (`0..8`) driving the boundary net.
    pub ble: u8,
    /// Sink IMUX index (`0..64`) at the receiving boundary tile.
    pub mux: u32,
    /// Locked 8-bit IMUX selector word.
    pub sel: u8,
    pub locked: bool,
}

impl PartPin {
    pub fn new(
        net: impl Into<String>,
        dir: PartPinDir,
        anchor_x: u32,
        anchor_y: u32,
        ble: u8,
        mux: u32,
        sel: u8,
    ) -> Self {
        Self {
            net: net.into(),
            dir,
            anchor_x,
            anchor_y,
            ble,
            mux,
            sel,
            locked: true,
        }
    }
}

/// Safe value driven by a [`DfxDecoupler`] onto a boundary net while `decouple_enable` is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationValue {
    /// Clamp boundary net to logic `0`.
    Zero,
    /// Clamp boundary net to logic `1`.
    One,
    /// Hold the last value captured when `engage()` was asserted.
    Hold,
}

/// Vivado DFX Decoupler IP model (PG375 / UG909).
///
/// Isolates boundary `PARTPIN` signals between the static shell and a Reconfigurable
/// Partition while partial reconfiguration is in progress, preventing spurious glitches
/// from corrupting static sequential state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DfxDecoupler {
    pub name: String,
    pub engaged: bool,
    pub policies: BTreeMap<String, IsolationValue>,
    pub held_values: BTreeMap<String, bool>,
    pub suppressed_glitches: u64,
}

impl DfxDecoupler {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            engaged: false,
            policies: BTreeMap::new(),
            held_values: BTreeMap::new(),
            suppressed_glitches: 0,
        }
    }

    pub fn add_net(&mut self, net: impl Into<String>, policy: IsolationValue) {
        self.policies.insert(net.into(), policy);
    }

    /// Engage decoupler isolation before partial reconfiguration begins, capturing Hold values.
    pub fn engage(&mut self, fabric: &Fabric, partpins: &[PartPin]) {
        for pp in partpins {
            let v = fabric.ble_out(pp.anchor_x, pp.anchor_y, u32::from(pp.ble));
            self.held_values.insert(pp.net.clone(), v);
        }
        self.engaged = true;
    }

    /// Release decoupler isolation after partial reconfiguration and `RESET_AFTER_RECONFIG` complete.
    pub fn release(&mut self) {
        self.engaged = false;
    }

    pub fn isolated_value_for(&self, net: &str, live_val: bool) -> bool {
        if !self.engaged {
            return live_val;
        }
        match self.policies.get(net).copied() {
            Some(IsolationValue::Zero) => false,
            Some(IsolationValue::One) => true,
            Some(IsolationValue::Hold) => self.held_values.get(net).copied().unwrap_or(false),
            None => live_val,
        }
    }

    /// Check that every boundary `PartPin` has a configured isolation policy (`HDPR-8`).
    pub fn covers_partpins(&self, partpins: &[PartPin]) -> Result<(), String> {
        for pp in partpins {
            if !self.policies.contains_key(&pp.net) {
                return Err(format!(
                    "HDPR-8: boundary net '{}' on decoupler '{}' has no isolation policy",
                    pp.net, self.name
                ));
            }
        }
        Ok(())
    }

    /// Check both policy coverage and active engagement (`engaged == true`) for live partial swap (`HDPR-8`).
    pub fn verify_active_isolation(&self, partpins: &[PartPin]) -> Result<(), String> {
        self.covers_partpins(partpins)?;
        if !self.engaged && !partpins.is_empty() {
            return Err(format!(
                "HDPR-8: DFX decoupler '{}' is not engaged during active partial reconfiguration",
                self.name
            ));
        }
        Ok(())
    }
}

/// One swappable Reconfigurable Module (`RM_A`, `RM_B`, ...) assigned to a [`ReconfigPartition`].
#[derive(Clone, Debug)]
pub struct ReconfigModule {
    pub name: String,
    /// Logical cell names (`lut_cell` / `ff_cell`) belonging to this RM.
    pub rm_cells: BTreeSet<String>,
    pub routed: Routed,
    /// Boundary `PARTPIN` placements observed or locked for this RM.
    pub partpins: Vec<PartPin>,
}

impl ReconfigModule {
    pub fn new(
        name: impl Into<String>,
        rm_cells: impl IntoIterator<Item = impl Into<String>>,
        routed: Routed,
    ) -> Self {
        Self {
            name: name.into(),
            rm_cells: rm_cells.into_iter().map(Into::into).collect(),
            routed,
            partpins: Vec::new(),
        }
    }

    pub fn with_partpins(mut self, partpins: Vec<PartPin>) -> Self {
        self.partpins = partpins;
        self
    }
}

/// Vivado UG909 Reconfigurable Partition (`Pblock` + locked `PARTPIN`s + `DfxDecoupler` + swappable RMs).
#[derive(Clone, Debug)]
pub struct ReconfigPartition {
    pub name: String,
    pub pblock: Pblock,
    pub partpins: Vec<PartPin>,
    pub decoupler: DfxDecoupler,
    pub modules: BTreeMap<String, ReconfigModule>,
    pub active_module: Option<String>,
}

impl ReconfigPartition {
    pub fn new(name: impl Into<String>, pblock: Pblock) -> Self {
        let name = name.into();
        let decoupler = DfxDecoupler::new(format!("{name}_decoupler"));
        Self {
            name,
            pblock,
            partpins: Vec::new(),
            decoupler,
            modules: BTreeMap::new(),
            active_module: None,
        }
    }

    /// Lock a boundary `PARTPIN` interface node and register a default `IsolationValue::Zero` policy.
    pub fn add_partpin(&mut self, mut pp: PartPin, isolation: IsolationValue) {
        pp.locked = true;
        self.decoupler.add_net(pp.net.clone(), isolation);
        self.partpins.push(pp);
    }

    /// Derive and lock `PARTPIN` boundary anchors from a reference routed design (`RM_A`).
    ///
    /// Scans all IMUX arcs in `routed` that cross between static cells (outside `self.pblock`)
    /// and `rm_cells` (inside `self.pblock`) and locks their boundary anchor tiles and IMUX selectors.
    pub fn lock_partpins_from_routed(
        &mut self,
        routed: &Routed,
        rm_cells: &BTreeSet<String>,
        isolation: IsolationValue,
    ) -> Result<Vec<PartPin>, String> {
        let placed = &routed.placed;
        let mut ff_info: BTreeMap<&str, (u32, u32, u8, bool)> = BTreeMap::new();
        for (i, lf) in placed.packed.lutffs.iter().enumerate() {
            let (site, ble) = placed.lutff_sites[i];
            let is_rm = rm_cells.contains(&lf.lut_cell) || rm_cells.contains(&lf.ff_cell);
            if !lf.ff_cell.is_empty() {
                ff_info.insert(lf.ff_cell.as_str(), (site.x, site.y, ble, is_rm));
            }
        }
        let mut derived = Vec::new();
        for (i, lf) in placed.packed.lutffs.iter().enumerate() {
            let (sink_site, sink_ble) = placed.lutff_sites[i];
            let sink_is_rm = rm_cells.contains(&lf.lut_cell) || rm_cells.contains(&lf.ff_cell);
            for (pin, drv_ff) in &lf.lut_pins {
                let Some(&(sx, sy, sble, src_is_rm)) = ff_info.get(drv_ff.as_str()) else {
                    continue;
                };
                if src_is_rm == sink_is_rm {
                    continue;
                }
                let mux = u32::from(sink_ble) * 8 + u32::from(*pin);
                let sel = encode_imux_hop(sx, sy, sble, sink_site.x, sink_site.y)?;
                let (dir, ax, ay) = if !src_is_rm && sink_is_rm {
                    (PartPinDir::StaticToRm, sink_site.x, sink_site.y)
                } else {
                    (PartPinDir::RmToStatic, sx, sy)
                };
                let net_name = format!("{drv_ff}_to_{}_p{pin}", lf.lut_cell);
                let pp = PartPin::new(net_name, dir, ax, ay, sble, mux, sel);
                derived.push(pp.clone());
                self.add_partpin(pp, isolation);
            }
        }
        Ok(derived)
    }

    pub fn add_module(&mut self, mut rm: ReconfigModule) {
        if rm.partpins.is_empty() {
            rm.partpins = self.partpins.clone();
        }
        if self.active_module.is_none() {
            self.active_module = Some(rm.name.clone());
        }
        self.modules.insert(rm.name.clone(), rm);
    }

    /// Generate a `.pbits` partial bitstream for a registered Reconfigurable Module (`rm_name`),
    /// enforcing locked `PARTPIN` boundary IMUX selectors in the generated frames.
    pub fn bitgen_rm(&self, dev: &Device, rm_name: &str) -> Result<PartialBitstream, String> {
        let rm = self
            .modules
            .get(rm_name)
            .ok_or_else(|| format!("unknown RM '{rm_name}' in partition '{}'", self.name))?;
        let mut full = helion_bits::bitgen(dev, &rm.routed)?;
        for pp in &self.partpins {
            if self.pblock.contains(pp.anchor_x, pp.anchor_y) {
                patch_imux_sel(dev, &mut full, pp.anchor_x, pp.anchor_y, pp.mux, pp.sel)?;
            }
        }
        let far_ranges = self.pblock.far_ranges(dev);
        let mut frames = BTreeMap::new();
        for ((b, maj, min), w) in &full.frames {
            if far_ranges.iter().any(|r| r.contains(*b, *maj, *min)) && *w != 0 {
                frames.insert((*b, *maj, *min), *w);
            }
        }
        if frames.is_empty() {
            return Err(format!(
                "{}: RM '{}' in pblock '{}' produced no frames",
                helion_bits::ERR_CODE_EMPTY_BITSTREAM,
                rm_name,
                self.pblock.name
            ));
        }
        let bounds = Some((
            self.pblock.x0,
            self.pblock.y0,
            self.pblock.x1,
            self.pblock.y1,
        ));
        let packets = encode_pbits(
            dev.idcode,
            self.pblock.reset_after_reconfig,
            bounds,
            &far_ranges,
            &frames,
        );
        Ok(PartialBitstream {
            idcode: dev.idcode,
            reset_after_reconfig: self.pblock.reset_after_reconfig,
            pblock_bounds: bounds,
            far_ranges,
            frames,
            packets,
        })
    }
}

/// Single Vivado UG909 DFX DRC violation (`HDPR-1` through `HDPR-8`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DfxDrcViolation {
    pub code: &'static str,
    pub partition: String,
    pub objects: String,
    pub message: String,
}

/// Report returned by [`check_dfx_drc`] covering UG909 `HDPR-1` .. `HDPR-8`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DfxDrcReport {
    pub violations: Vec<DfxDrcViolation>,
}

impl DfxDrcReport {
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn error_count(&self) -> usize {
        self.violations.len()
    }

    pub fn has_rule(&self, code: &str) -> bool {
        self.violations.iter().any(|v| v.code == code)
    }

    pub fn add(
        &mut self,
        code: &'static str,
        partition: impl Into<String>,
        objects: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.violations.push(DfxDrcViolation {
            code,
            partition: partition.into(),
            objects: objects.into(),
            message: message.into(),
        });
    }
}

/// Run Vivado UG909 Dynamic Function eXchange (DFX) DRC checks (`HDPR-1` through `HDPR-8`):
///
/// - `HDPR-1`: Pblock coordinates inverted or outside device CLB interior grid.
/// - `HDPR-2`: Pblock violates tile-column frame alignment (`minor_mask` not full column frames)
///   or `RESET_AFTER_RECONFIG` frame alignment requirements.
/// - `HDPR-3`: Multiple reconfigurable partitions have overlapping `Pblock` tile rectangles.
/// - `HDPR-4`: Static shell logic encroaches on a strict RM `Pblock` (`pblock.strict == true`).
/// - `HDPR-5`: Reconfigurable Module (RM) cell is placed outside its partition `Pblock`.
/// - `HDPR-6`: Boundary net between static logic and RM crosses the partition without a locked
///   `PARTPIN` or crosses through a non-interface tile.
/// - `HDPR-7`: `PARTPIN` anchor lies outside the `Pblock` boundary interface ring, uses a duplicate
///   IMUX resource, or mismatches between swappable RMs (`RM_A`, `RM_B`, ...).
/// - `HDPR-8`: Boundary `PARTPIN` net lacks a configured [`DfxDecoupler`] isolation policy.
pub fn check_dfx_drc(dev: &Device, partitions: &[ReconfigPartition]) -> DfxDrcReport {
    let mut report = DfxDrcReport::default();

    for (idx, part) in partitions.iter().enumerate() {
        let pb = &part.pblock;
        // HDPR-1: Pblock within device CLB array
        if !pb.is_within_device(dev) {
            report.add(
                "HDPR-1",
                &part.name,
                &pb.name,
                format!(
                    "Pblock '{}' (X{}Y{}:X{}Y{}) is outside device CLB bounds (X{}Y{}..=X{}Y{})",
                    pb.name,
                    pb.x0,
                    pb.y0,
                    pb.x1,
                    pb.y1,
                    dev.clb_x0,
                    dev.clb_y0,
                    dev.clb_x0 + dev.interior_cols.saturating_sub(1),
                    dev.clb_y0 + dev.interior_rows.saturating_sub(1),
                ),
            );
        }

        // HDPR-2: Tile-column frame alignment and RESET_AFTER_RECONFIG alignment
        if !pb.is_frame_aligned(dev) {
            report.add(
                "HDPR-2",
                &part.name,
                &pb.name,
                format!(
                    "Pblock '{}' minor_mask {:#06x} is not aligned to full CLB tile frames (RESET_AFTER_RECONFIG={})",
                    pb.name, pb.minor_mask, pb.reset_after_reconfig
                ),
            );
        }

        // HDPR-3: Disjoint Pblocks across partitions
        for other in &partitions[idx + 1..] {
            if pb.overlaps(&other.pblock) {
                report.add(
                    "HDPR-3",
                    &part.name,
                    format!("{},{}", pb.name, other.pblock.name),
                    format!(
                        "Pblock '{}' overlaps with partition '{}' Pblock '{}'",
                        pb.name, other.name, other.pblock.name
                    ),
                );
            }
        }

        // HDPR-7: Validate partition-level PARTPIN anchors (on boundary ring, locked, unique mux)
        let mut seen_mux: BTreeSet<(u32, u32, u32)> = BTreeSet::new();
        for pp in &part.partpins {
            if !pp.locked {
                report.add(
                    "HDPR-7",
                    &part.name,
                    &pp.net,
                    format!("PARTPIN for boundary net '{}' is not locked", pp.net),
                );
            }
            if !pb.is_boundary_tile(pp.anchor_x, pp.anchor_y) {
                report.add(
                    "HDPR-7",
                    &part.name,
                    &pp.net,
                    format!(
                        "PARTPIN '{}' anchor CLB_X{}Y{} is not on Pblock '{}' boundary interface ring",
                        pp.net, pp.anchor_x, pp.anchor_y, pb.name
                    ),
                );
            }
            if !seen_mux.insert((pp.anchor_x, pp.anchor_y, pp.mux)) {
                report.add(
                    "HDPR-7",
                    &part.name,
                    &pp.net,
                    format!(
                        "PARTPIN '{}' conflicts on CLB_X{}Y{} IMUX[{}]",
                        pp.net, pp.anchor_x, pp.anchor_y, pp.mux
                    ),
                );
            }
        }

        // HDPR-8: DFX Decoupler coverage for all boundary PARTPIN nets
        for pp in &part.partpins {
            if !part.decoupler.policies.contains_key(&pp.net) {
                report.add(
                    "HDPR-8",
                    &part.name,
                    &pp.net,
                    format!(
                        "Boundary net '{}' in partition '{}' is not guarded by DFX decoupler '{}'",
                        pp.net, part.name, part.decoupler.name
                    ),
                );
            }
        }

        // Per-RM checks: HDPR-4 (static encroachment), HDPR-5 (RM out of pblock),
        // HDPR-6 (unlocked boundary crossing), HDPR-7 (RM PARTPIN mismatch)
        for rm in part.modules.values() {
            let placed = &rm.routed.placed;
            let mut ff_site: BTreeMap<&str, (u32, u32, u8, bool)> = BTreeMap::new();
            for (i, lf) in placed.packed.lutffs.iter().enumerate() {
                let Some(&(site, ble)) = placed.lutff_sites.get(i) else {
                    continue;
                };
                let is_rm =
                    rm.rm_cells.contains(&lf.lut_cell) || rm.rm_cells.contains(&lf.ff_cell);
                let in_pb = pb.contains(site.x, site.y);
                if is_rm && !in_pb {
                    report.add(
                        "HDPR-5",
                        &part.name,
                        format!("{}:{}", rm.name, lf.lut_cell),
                        format!(
                            "RM '{}' cell '{}' is placed at CLB_X{}Y{} outside Pblock '{}' (X{}Y{}:X{}Y{})",
                            rm.name, lf.lut_cell, site.x, site.y, pb.name, pb.x0, pb.y0, pb.x1, pb.y1
                        ),
                    );
                } else if !is_rm && in_pb && pb.strict {
                    report.add(
                        "HDPR-4",
                        &part.name,
                        format!("{}:{}", rm.name, lf.lut_cell),
                        format!(
                            "Static cell '{}' encroaches on strict Pblock '{}' at CLB_X{}Y{}",
                            lf.lut_cell, pb.name, site.x, site.y
                        ),
                    );
                }
                if !lf.ff_cell.is_empty() {
                    ff_site.insert(lf.ff_cell.as_str(), (site.x, site.y, ble, is_rm));
                }
            }

            // Check boundary crossings (HDPR-6)
            for (i, lf) in placed.packed.lutffs.iter().enumerate() {
                let Some(&(sink_site, sink_ble)) = placed.lutff_sites.get(i) else {
                    continue;
                };
                let sink_is_rm =
                    rm.rm_cells.contains(&lf.lut_cell) || rm.rm_cells.contains(&lf.ff_cell);
                for (pin, drv_ff) in &lf.lut_pins {
                    let Some(&(sx, sy, sble, src_is_rm)) = ff_site.get(drv_ff.as_str()) else {
                        continue;
                    };
                    if src_is_rm == sink_is_rm {
                        continue;
                    }
                    let mux = u32::from(sink_ble) * 8 + u32::from(*pin);
                    let expected_sel = encode_imux_hop(sx, sy, sble, sink_site.x, sink_site.y).ok();
                    let (ax, ay) = if !src_is_rm && sink_is_rm {
                        (sink_site.x, sink_site.y)
                    } else {
                        (sx, sy)
                    };
                    let matched_pp = part.partpins.iter().find(|pp| {
                        pp.anchor_x == ax
                            && pp.anchor_y == ay
                            && pp.ble == sble
                            && pp.mux == mux
                            && Some(pp.sel) == expected_sel
                            && pp.locked
                    });
                    if matched_pp.is_none() || !pb.is_boundary_tile(ax, ay) {
                        report.add(
                            "HDPR-6",
                            &part.name,
                            format!("{}:{}->{}", rm.name, drv_ff, lf.lut_cell),
                            format!(
                                "Boundary net '{} -> {}' in RM '{}' crosses Pblock '{}' without a legal locked PARTPIN at interface tile CLB_X{}Y{}",
                                drv_ff, lf.lut_cell, rm.name, pb.name, ax, ay
                            ),
                        );
                    }
                }
            }

            // Check RM-specific PARTPIN consistency against partition locked PARTPINs (HDPR-7)
            if rm.partpins != part.partpins {
                report.add(
                    "HDPR-7",
                    &part.name,
                    &rm.name,
                    format!(
                        "RM '{}' PARTPIN boundary anchors do not match locked partition '{}' PARTPINs",
                        rm.name, part.name
                    ),
                );
            }
        }
    }

    report
}
#[cfg(test)]
mod tests {
    use super::*;
    use helion_bits::{FeatureSet, assemble, bitgen, bitgen_pblock};
    use helion_device::{Device, Far};
    use helion_ir::{CellKind, Design, PortDir};
    use helion_pack::pack;
    use helion_place::{PlaceOpts, place_with};
    use helion_route::route;

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
        assert_eq!(fab.stat.word(), Stat::STARTUP_WORD);
        let names: Vec<_> = fab.stat.bits().iter().map(|b| b.name).collect();
        assert_eq!(
            names,
            ["CRC_ERR", "INIT", "GTS", "GSR", "GWE", "DONE", "EOS"]
        );
        let reset = Fabric::new(&dev).stat;
        assert_eq!(reset.word(), Stat::RESET_WORD);
        assert_ne!(
            reset.word(),
            fab.stat.word(),
            "startup flips GTS/GSR → DONE"
        );
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

    /// A bitstream that went out through `.hbits` packets and came back must
    /// still program the die to the gold counter waveform.
    #[test]
    fn decoded_hbits_programs_gold_counter() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        let pl = place_with(
            &p,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        let r = route(&pl, &dev).unwrap();
        let bits = bitgen(&dev, &r).unwrap();
        let (idcode, frames) = helion_bits::decode_packets(&bits.packets).unwrap();
        assert_eq!(idcode, dev.idcode);
        assert!(!frames.is_empty(), "decoded stream must carry frames");

        let mut round_tripped = bits.clone();
        round_tripped.frames = frames;
        let iob = r.iob_src[0].iob;
        let mut wave = Vec::new();
        let mut fab = Fabric::new(&dev);
        fab.program(&round_tripped).unwrap();
        fab.finish_startup();
        for _ in 0..16 {
            fab.step_user();
            wave.push(fab.led_at(iob.0, iob.1));
        }
        assert!(wave[0..7].iter().all(|b| !b), "cnt 1..7 LED=0 {wave:?}");
        assert!(wave[7..15].iter().all(|b| *b), "cnt 8..15 LED=1 {wave:?}");
        assert!(!wave[15], "wrap {wave:?}");

        // Same waveform as the un-encoded bitstream: the stream is lossless.
        let mut direct = Fabric::new(&dev);
        direct.program(&bits).unwrap();
        direct.finish_startup();
        let gold: Vec<bool> = (0..16)
            .map(|_| {
                direct.step_user();
                direct.led_at(iob.0, iob.1)
            })
            .collect();
        assert_eq!(wave, gold);
    }

    fn nine_inv(last_init: u64) -> Design {
        let mut d = Design::new("dfx");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        for i in 0..9u32 {
            let init = if i == 8 {
                last_init
            } else {
                0x5555_5555_5555_5555
            };
            let lut_name = format!("u_lut{i}");
            let ff_name = format!("u_ff{i}");
            let d_net = format!("d{i}");
            let q_net = format!("q{i}");
            d.add_cell(&lut_name, CellKind::Lut6 { init });
            d.add_cell(&ff_name, CellKind::Hff);
            d.connect("clk", &ff_name, "CLK");
            d.connect(&d_net, &lut_name, "O");
            d.connect(&d_net, &ff_name, "D");
            d.connect(&q_net, &ff_name, "Q");
            d.connect(&q_net, &lut_name, "I0");
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q0", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    #[test]
    fn dfx_rm_swap_leaves_static_frames() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let da = nine_inv(0x5555_5555_5555_5555);
        let db = nine_inv(0xAAAA_AAAA_AAAA_AAAA);
        let pa = pack(&da, &dev).unwrap();
        let pbk = pack(&db, &dev).unwrap();
        let pla = place_with(
            &pa,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        let plb = place_with(
            &pbk,
            &dev,
            PlaceOpts {
                timing_weight: 0.75, seed: 0 },
        )
        .unwrap();
        assert_ne!(
            (pla.lutff_sites[0].0.x, pla.lutff_sites[0].0.y),
            (pla.lutff_sites[8].0.x, pla.lutff_sites[8].0.y),
            "RM must occupy a different CLB"
        );
        let ra = route(&pla, &dev).unwrap();
        let rb = route(&plb, &dev).unwrap();
        let full_a = bitgen(&dev, &ra).unwrap();
        let full_b = bitgen(&dev, &rb).unwrap();
        let (sx, sy) = (pla.lutff_sites[0].0.x, pla.lutff_sites[0].0.y);
        let (rx, ry) = (pla.lutff_sites[8].0.x, pla.lutff_sites[8].0.y);
        let partial = bitgen_pblock(&dev, &rb, &[(rx, ry)]).unwrap();
        let static_maj = dev.clb_major(sx, sy).unwrap();
        let rm_maj = dev.clb_major(rx, ry).unwrap();
        assert_ne!(static_maj, rm_maj);

        let mut fab = Fabric::new(&dev);
        fab.program(&full_a).unwrap();
        fab.finish_startup();
        let static_before: Vec<_> = (0..dev.clb_minors as u8)
            .map(|m| fab.frame_word(Far::CLB_IO_CLK, static_maj, m))
            .collect();
        let rm_before = fab.lut_init(rx, ry, 0);
        assert_eq!(rm_before, 0x5555_5555_5555_5555);

        fab.program_partial(&partial).unwrap();
        let static_after: Vec<_> = (0..dev.clb_minors as u8)
            .map(|m| fab.frame_word(Far::CLB_IO_CLK, static_maj, m))
            .collect();
        assert_eq!(
            static_before, static_after,
            "static frames must be unchanged after RM swap"
        );
        let rm_after = fab.lut_init(rx, ry, 0);
        assert_eq!(rm_after, 0xAAAA_AAAA_AAAA_AAAA, "RM LUT must swap");
        assert_ne!(full_a.frames, full_b.frames);
        assert!(
            partial
                .frames
                .keys()
                .all(|(b, maj, _)| *b != Far::CLB_IO_CLK || *maj == rm_maj)
        );
    }

    #[test]
    fn bram_runtime_write_read_roundtrip() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut fab = Fabric::new(&dev);
        assert_eq!(fab.bram_read_word(0, 3), 0);
        fab.bram_write_word(0, 3, 0xA5A5_A5A5_A5A5_A5A5);
        assert_eq!(fab.bram_read_word(0, 3), 0xA5A5_A5A5_A5A5_A5A5);
        assert_eq!(
            fab.bram_read_word(0, 4),
            0,
            "unwritten addr stays init/zero"
        );
    }

    #[test]
    fn ila_trigger_fsm_rising_completes_window() {
        let mut fsm = IlaTriggerFsm::arm(IlaMatchKind::Rising, 8, 2);
        assert!(fsm.armed && !fsm.fired);
        // miss, miss, hit → then 5 more post samples (post_left starts at 5)
        assert!(!fsm.on_sample(false));
        assert!(!fsm.on_sample(false));
        assert!(!fsm.on_sample(true)); // fire; post_left=5
        for _ in 0..4 {
            assert!(!fsm.on_sample(false));
        }
        assert!(fsm.on_sample(false));
        assert!(fsm.done && fsm.fired);
        assert_eq!(fsm.pre_trigger, 2);
    }

    #[test]
    fn ila_trigger_fsm_immediate_fires_first() {
        let mut fsm = IlaTriggerFsm::arm(IlaMatchKind::Immediate, 4, 99);
        assert_eq!(fsm.pre_trigger, 0);
        assert!(!fsm.on_sample(false)); // fire on first; post_left=3
        assert!(!fsm.on_sample(false));
        assert!(!fsm.on_sample(false));
        assert!(fsm.on_sample(false));
    }

    /// Build a two-CLB design where static cells (0..8) live in CLB 0 and
    /// RM cell 8 lives in CLB 1 and connects across the static<->RM boundary.
    fn build_static_and_rm_design(rm_init: u64) -> Design {
        let mut d = Design::new("dfx_coupled");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        for i in 0..9u32 {
            let init = if i == 8 {
                rm_init
            } else {
                0x5555_5555_5555_5555
            };
            let lut_name = format!("u_lut{i}");
            let ff_name = format!("u_ff{i}");
            let d_net = format!("d{i}");
            let q_net = format!("q{i}");
            d.add_cell(&lut_name, CellKind::Lut6 { init });
            d.add_cell(&ff_name, CellKind::Hff);
            d.connect("clk", &ff_name, "CLK");
            d.connect(&d_net, &lut_name, "O");
            d.connect(&d_net, &ff_name, "D");
            d.connect(&q_net, &ff_name, "Q");
            if i == 8 {
                // RM LUT8 samples static FF0 Q (q0) on I0!
                d.connect("q0", &lut_name, "I0");
            } else {
                d.connect(&q_net, &lut_name, "I0");
            }
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q0", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    #[test]
    fn dfx_drc_hdpr1_through_hdpr8_checks() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let da = build_static_and_rm_design(0xAAAA_AAAA_AAAA_AAAA);
        let db = build_static_and_rm_design(0x5555_5555_5555_5555);
        let pa = pack(&da, &dev).unwrap();
        let pb_pack = pack(&db, &dev).unwrap();
        let pla = place_with(
            &pa,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let plb = place_with(
            &pb_pack,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let ra = route(&pla, &dev).unwrap();
        let rb = route(&plb, &dev).unwrap();

        let (rx, ry) = (pla.lutff_sites[8].0.x, pla.lutff_sites[8].0.y);
        let rm_cells: BTreeSet<String> = ["u_lut8".to_string(), "u_ff8".to_string()]
            .into_iter()
            .collect();

        // 1. Clean legal partition with RM_A and RM_B must pass all HDPR-1..HDPR-8 checks.
        let pblock = Pblock::new("pblock_rm", rx, ry, rx, ry);
        let mut part = ReconfigPartition::new("rp0", pblock.clone());
        let locked_pps = part
            .lock_partpins_from_routed(&ra, &rm_cells, IsolationValue::Zero)
            .unwrap();
        assert_eq!(locked_pps.len(), 1, "must derive 1 static->RM PARTPIN");
        part.add_module(ReconfigModule::new("RM_A", rm_cells.clone(), ra.clone()));
        part.add_module(ReconfigModule::new("RM_B", rm_cells.clone(), rb.clone()));

        let clean_drc = check_dfx_drc(&dev, std::slice::from_ref(&part));
        assert!(
            clean_drc.ok(),
            "clean partition must have 0 DFX DRC violations: {:?}",
            clean_drc.violations
        );

        // 2. HDPR-1 (out-of-bounds Pblock) and HDPR-2 (unaligned minor_mask).
        let bad_pb = Pblock::new("pb_oob", 0, 0, 999, 999).with_minor_mask(0x00FF);
        let bad_part12 = ReconfigPartition::new("rp_bad12", bad_pb);
        let rep12 = check_dfx_drc(&dev, &[bad_part12]);
        assert!(rep12.has_rule("HDPR-1"), "expected HDPR-1: {rep12:?}");
        let unaligned_pb = Pblock::new("pb_unaligned", rx, ry, rx, ry).with_minor_mask(0x00FF);
        let rep2 = check_dfx_drc(&dev, &[ReconfigPartition::new("rp_bad2", unaligned_pb)]);
        assert!(rep2.has_rule("HDPR-2"), "expected HDPR-2: {rep2:?}");

        // 3. HDPR-3 (overlapping Pblocks).
        let part_overlap = ReconfigPartition::new("rp_overlap", pblock.clone());
        let rep3 = check_dfx_drc(&dev, &[part.clone(), part_overlap]);
        assert!(rep3.has_rule("HDPR-3"), "expected HDPR-3: {rep3:?}");

        // 4. HDPR-4 (static logic encroaching on strict Pblock) & HDPR-5 (RM cell outside Pblock).
        let (sx, sy) = (pla.lutff_sites[0].0.x, pla.lutff_sites[0].0.y);
        let encroached_pb = Pblock::new(
            "pb_encroach",
            sx.min(rx),
            sy.min(ry),
            sx.max(rx),
            sy.max(ry),
        );
        let mut part4 = ReconfigPartition::new("rp4", encroached_pb);
        part4.add_module(ReconfigModule::new("RM_A", rm_cells.clone(), ra.clone()));
        let rep4 = check_dfx_drc(&dev, &[part4]);
        assert!(rep4.has_rule("HDPR-4"), "expected HDPR-4: {rep4:?}");

        let wrong_pb = Pblock::new("pb_wrong", sx, sy, sx, sy);
        let mut part5 = ReconfigPartition::new("rp5", wrong_pb);
        part5.add_module(ReconfigModule::new("RM_A", rm_cells.clone(), ra.clone()));
        let rep5 = check_dfx_drc(&dev, &[part5]);
        assert!(rep5.has_rule("HDPR-5"), "expected HDPR-5: {rep5:?}");

        // 5. HDPR-6 (boundary net crossing without locked PARTPIN).
        let mut part6 = ReconfigPartition::new("rp6", pblock.clone());
        part6.add_module(ReconfigModule::new("RM_A", rm_cells.clone(), ra.clone()));
        let rep6 = check_dfx_drc(&dev, &[part6]);
        assert!(rep6.has_rule("HDPR-6"), "expected HDPR-6: {rep6:?}");

        // 6. HDPR-7 (PARTPIN mismatch / unlocked) and HDPR-8 (missing decoupler policy).
        let mut part78 = part.clone();
        part78.decoupler.policies.clear();
        if let Some(rm_b) = part78.modules.get_mut("RM_B") {
            rm_b.partpins.clear();
        }
        let rep78 = check_dfx_drc(&dev, &[part78]);
        assert!(rep78.has_rule("HDPR-7"), "expected HDPR-7: {rep78:?}");
        assert!(rep78.has_rule("HDPR-8"), "expected HDPR-8: {rep78:?}");
    }

    #[test]
    fn multi_rm_live_swap_preserves_static_state_and_enforces_decoupler() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        // RM_A inverts static q0 (~q0 = 0x5555...), RM_B buffers static q0 (q0 = 0xAAAA...).
        let da = build_static_and_rm_design(0x5555_5555_5555_5555);
        let db = build_static_and_rm_design(0xAAAA_AAAA_AAAA_AAAA);
        let pa = pack(&da, &dev).unwrap();
        let pb_pack = pack(&db, &dev).unwrap();
        let pla = place_with(
            &pa,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let plb = place_with(
            &pb_pack,
            &dev,
            PlaceOpts {
                timing_weight: 0.75,
                seed: 0,
            },
        )
        .unwrap();
        let ra = route(&pla, &dev).unwrap();
        let rb = route(&plb, &dev).unwrap();

        let (sx, sy, sble) = (
            pla.lutff_sites[0].0.x,
            pla.lutff_sites[0].0.y,
            pla.lutff_sites[0].1,
        );
        let (rx, ry, rble) = (
            pla.lutff_sites[8].0.x,
            pla.lutff_sites[8].0.y,
            pla.lutff_sites[8].1,
        );
        let rm_cells: BTreeSet<String> = ["u_lut8".to_string(), "u_ff8".to_string()]
            .into_iter()
            .collect();

        let pblock = Pblock::new("pblock_rm", rx, ry, rx, ry).with_reset_after_reconfig(true);
        let mut partition = ReconfigPartition::new("rp0", pblock);
        partition
            .lock_partpins_from_routed(&ra, &rm_cells, IsolationValue::Hold)
            .unwrap();
        partition.add_module(ReconfigModule::new("RM_A", rm_cells.clone(), ra.clone()));
        partition.add_module(ReconfigModule::new("RM_B", rm_cells.clone(), rb.clone()));

        let full_a = bitgen(&dev, &ra).unwrap();
        let pbits_a = partition.bitgen_rm(&dev, "RM_A").unwrap();
        let pbits_b = partition.bitgen_rm(&dev, "RM_B").unwrap();

        let mut fab = Fabric::new(&dev);
        fab.program(&full_a).unwrap();
        fab.finish_startup();

        // Seed static runtime BRAM and step the fabric so static FF has active state.
        fab.bram_write_word(0, 7, 0xCAFE_F00D_1234_5678);
        fab.step_user(); // q0 toggles to true; RM_A (inv) lut_o = !q0
        let static_q_before = fab.ble_q(sx, sy, u32::from(sble));
        assert!(static_q_before, "static FF0 Q must be 1 after first step");

        // Set RM FF Q to true before swap so we can verify RESET_AFTER_RECONFIG resets RM FF to 0.
        fab.set_ble_q(rx, ry, u32::from(rble), true);
        assert!(fab.ble_q(rx, ry, u32::from(rble)));

        // Attempting program_dfx_swap while decoupler is NOT engaged must fail with HDPR-8.
        let unengaged_err = fab.program_dfx_swap(&partition, &pbits_b).unwrap_err();
        assert!(
            unengaged_err.contains("HDPR-8"),
            "must reject unengaged decoupler: {unengaged_err}"
        );

        // Engage decoupler, swap RM_A -> RM_B, verify static FF + BRAM untouched and RM FF reset.
        let partpins = partition.partpins.clone();
        partition.decoupler.engage(&fab, &partpins);
        fab.program_dfx_swap(&partition, &pbits_b).unwrap();
        assert_eq!(
            fab.ble_q(sx, sy, u32::from(sble)),
            static_q_before,
            "static FF Q must be preserved across live partial reconfiguration"
        );
        assert_eq!(
            fab.bram_read_word(0, 7),
            0xCAFE_F00D_1234_5678,
            "static runtime BRAM data must be preserved across live partial reconfiguration"
        );
        assert!(
            !fab.ble_q(rx, ry, u32::from(rble)),
            "RESET_AFTER_RECONFIG must reset RM FF Q inside the Pblock to INIT (0)"
        );
        assert_eq!(fab.lut_init(rx, ry, u32::from(rble)), 0xAAAA_AAAA_AAAA_AAAA);

        // Release decoupler and clock once: static q0=1 -> RM_B (buffer) latches 1!
        partition.decoupler.release();
        fab.step_user();
        assert!(
            fab.ble_q(rx, ry, u32::from(rble)),
            "RM_B buffer must latch 1 when static q0 was 1"
        );

        // Swap back RM_B -> RM_A via decoded .pbits packet stream.
        let decoded_pbits_a = decode_pbits(&pbits_a.packets).unwrap();
        partition.decoupler.engage(&fab, &partpins);
        fab.program_dfx_swap(&partition, &decoded_pbits_a).unwrap();
        partition.decoupler.release();
        assert_eq!(fab.lut_init(rx, ry, u32::from(rble)), 0x5555_5555_5555_5555);

        // Live sub-millisecond LUT INIT and probe ECO on running Fabric:
        fab.patch_lut_init_live(rx, ry, u32::from(rble), 0xFFFF_FFFF_FFFF_FFFF)
            .unwrap();
        assert_eq!(fab.lut_init(rx, ry, u32::from(rble)), 0xFFFF_FFFF_FFFF_FFFF);
        let sel = fab
            .reroute_probe_eco_live((sx, sy, sble), (rx, ry, u32::from(rble) * 8))
            .unwrap();
        assert_eq!(sel, encode_imux_hop(sx, sy, sble, rx, ry).unwrap());
    }
}
