//! Cycle-accurate Helion-T fabric model (CLB LUT+FF + IOB + startup SM).

use helion_bits::Bitstream;
use helion_device::{Device, Far};
use std::collections::BTreeMap;

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
    pub const STARTUP_WORD: u32 = (1 << Self::BIT_INIT)
        | (1 << Self::BIT_GWE)
        | (1 << Self::BIT_DONE)
        | (1 << Self::BIT_EOS);

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
        for b in 0..8u32 {
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
        self.frames.get(&(block, major, minor)).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_bits::{assemble, bitgen, bitgen_pblock, FeatureSet};
    use helion_device::{Device, Far};
    use helion_ir::{CellKind, Design, PortDir};
    use helion_pack::pack;
    use helion_place::{place_with, PlaceOpts};
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
        assert_ne!(reset.word(), fab.stat.word(), "startup flips GTS/GSR → DONE");
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
        let pl = place_with(&p, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
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
            let init = if i == 8 { last_init } else { 0x5555_5555_5555_5555 };
            d.add_cell(format!("u_lut{i}"), CellKind::Lut6 { init });
            d.add_cell(format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", format!("u_ff{i}"), "CLK");
            d.connect(format!("d{i}"), format!("u_lut{i}"), "O");
            d.connect(format!("d{i}"), format!("u_ff{i}"), "D");
            d.connect(format!("q{i}"), format!("u_ff{i}"), "Q");
            d.connect(format!("q{i}"), format!("u_lut{i}"), "I0");
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
        let pla = place_with(&pa, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let plb = place_with(&pbk, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
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
        assert_eq!(static_before, static_after, "static frames must be unchanged after RM swap");
        let rm_after = fab.lut_init(rx, ry, 0);
        assert_eq!(rm_after, 0xAAAA_AAAA_AAAA_AAAA, "RM LUT must swap");
        assert_ne!(full_a.frames, full_b.frames);
        assert!(partial.frames.keys().all(|(b, maj, _)| *b != Far::CLB_IO_CLK || *maj == rm_maj));
    }

    #[test]
    fn bram_runtime_write_read_roundtrip() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut fab = Fabric::new(&dev);
        assert_eq!(fab.bram_read_word(0, 3), 0);
        fab.bram_write_word(0, 3, 0xA5A5_A5A5_A5A5_A5A5);
        assert_eq!(fab.bram_read_word(0, 3), 0xA5A5_A5A5_A5A5_A5A5);
        assert_eq!(fab.bram_read_word(0, 4), 0, "unwritten addr stays init/zero");
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
}
