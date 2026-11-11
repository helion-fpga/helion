//! In-process FTDI MPSSE device model for testing the native JTAG engine.
//!
//! [`MockFtdi`] implements [`MpssePort`] the way an FT2232H channel behaves:
//! it must be put in MPSSE mode with `SET_BITMODE` before it accepts bytes,
//! it parses the MPSSE command stream byte by byte (commands may straddle
//! writes), answers unknown opcodes with `0xFA, opcode`, and every data or
//! TMS clock drives TCK/TMS/TDI into a scan chain of simulated TAPs — the
//! Helion [`Tap`] model (with its fabric, USER chains and config flash) and
//! optional [`GenericTap`] neighbours. TDO bits are packed back exactly as
//! the chip does (bit-mode reads left-aligned, see [`crate::mpsse`]).
//!
//! The model refuses what would silently break a real JTAG target: shifting
//! before the pins are outputs, data written on the rising TCK edge or
//! sampled on the falling edge, and adaptive / 3-phase clocking.

use std::collections::VecDeque;

use helion_device::Device;

use crate::Tap;
use crate::jtag_engine::{JtagState, MpssePort};
use crate::mpsse::{flag, op, pin};

/// A plain IEEE 1149.1 device: IDCODE (opcode 1) or BYPASS (all ones), IR
/// capture pattern `…01`. Used as a chain neighbour in tests.
#[derive(Clone, Debug)]
pub struct GenericTap {
    pub idcode: Option<u32>,
    pub ir_len: u8,
    state: JtagState,
    ir: u64,
    ir_shift: u64,
    dr_shift: u64,
}

impl GenericTap {
    pub fn new(idcode: Option<u32>, ir_len: u8) -> Self {
        assert!((2..=32).contains(&ir_len));
        let mut t = Self {
            idcode,
            ir_len,
            state: JtagState::Reset,
            ir: 0,
            ir_shift: 0,
            dr_shift: 0,
        };
        t.ir = t.reset_ir();
        t
    }

    fn ones(&self) -> u64 {
        (1u64 << self.ir_len) - 1
    }

    fn reset_ir(&self) -> u64 {
        if self.idcode.is_some() {
            1
        } else {
            self.ones()
        }
    }

    pub fn tick(&mut self, tms: bool, tdi: bool) -> bool {
        let tdo = match self.state {
            JtagState::ShiftIr => self.ir_shift & 1 == 1,
            JtagState::ShiftDr => self.dr_shift & 1 == 1,
            _ => false,
        };
        let dr_len = if self.ir == 1 && self.idcode.is_some() {
            32
        } else {
            1
        };
        match self.state {
            JtagState::ShiftIr => {
                self.ir_shift = (self.ir_shift >> 1) | (u64::from(tdi) << (self.ir_len - 1));
            }
            JtagState::ShiftDr => {
                self.dr_shift = (self.dr_shift >> 1) | (u64::from(tdi) << (dr_len - 1));
            }
            _ => {}
        }
        let next = self.state.next(tms);
        match next {
            JtagState::Reset => self.ir = self.reset_ir(),
            JtagState::CaptureIr if self.state != next => self.ir_shift = 0b01,
            JtagState::CaptureDr if self.state != next => {
                self.dr_shift = match (self.ir == 1, self.idcode) {
                    (true, Some(id)) => u64::from(id),
                    _ => 0,
                };
            }
            JtagState::UpdateIr if self.state != next => self.ir = self.ir_shift & self.ones(),
            _ => {}
        }
        self.state = next;
        tdo
    }
}

/// One device on the mock board's scan chain.
#[derive(Clone, Debug)]
pub enum MockChainDevice {
    Helion(Box<Tap>),
    Generic(GenericTap),
}

impl MockChainDevice {
    fn tick(&mut self, tms: bool, tdi: bool) -> bool {
        match self {
            MockChainDevice::Helion(t) => t.tick(tms, tdi),
            MockChainDevice::Generic(g) => g.tick(tms, tdi),
        }
    }
}

/// Simulated FTDI MPSSE channel wired to a simulated board.
#[derive(Clone, Debug)]
pub struct MockFtdi {
    /// Chain, index 0 nearest TDO.
    pub chain: Vec<MockChainDevice>,
    mpsse: bool,
    pub latency_ms: u8,
    low_val: u8,
    low_dir: u8,
    pub high_val: u8,
    pub high_dir: u8,
    pub divisor: u16,
    /// Power-up default on H-series is divide-by-5 enabled.
    pub div5: bool,
    adaptive: bool,
    three_phase: bool,
    loopback: bool,
    pending: Vec<u8>,
    rx: VecDeque<u8>,
    /// Total TCK pulses driven.
    pub tck_count: u64,
    /// Every opcode the chip executed, in order (tests assert on this).
    pub opcodes: Vec<u8>,
    /// Bytes written / read over "USB".
    pub bytes_written: usize,
    pub bytes_read: usize,
    pub resets: u32,
}

impl MockFtdi {
    pub fn with_chain(chain: Vec<MockChainDevice>) -> Self {
        Self {
            chain,
            mpsse: false,
            latency_ms: 16,
            low_val: 0,
            low_dir: 0,
            high_val: 0,
            high_dir: 0,
            divisor: 0,
            div5: true,
            adaptive: false,
            three_phase: false,
            loopback: false,
            pending: Vec::new(),
            rx: VecDeque::new(),
            tck_count: 0,
            opcodes: Vec::new(),
            bytes_written: 0,
            bytes_read: 0,
            resets: 0,
        }
    }

    /// One Helion device on the chain.
    pub fn helion(dev: &Device) -> Self {
        Self::with_chain(vec![MockChainDevice::Helion(Box::new(Tap::new(dev)))])
    }

    /// The first Helion TAP on the chain (the simulated board).
    pub fn helion_tap(&self) -> Option<&Tap> {
        self.chain.iter().find_map(|d| match d {
            MockChainDevice::Helion(t) => Some(t.as_ref()),
            _ => None,
        })
    }

    pub fn helion_tap_mut(&mut self) -> Option<&mut Tap> {
        self.chain.iter_mut().find_map(|d| match d {
            MockChainDevice::Helion(t) => Some(t.as_mut()),
            _ => None,
        })
    }

    pub fn in_mpsse(&self) -> bool {
        self.mpsse
    }

    /// ADBUS pin levels as `GET_LOW` would read them.
    pub fn low_pins(&self) -> u8 {
        self.low_val
    }

    fn tms_pin(&self) -> bool {
        self.low_val & pin::TMS != 0
    }

    fn tdi_pin(&self) -> bool {
        self.low_val & pin::TDI != 0
    }

    fn set_pin(&mut self, mask: u8, v: bool) {
        if v {
            self.low_val |= mask;
        } else {
            self.low_val &= !mask;
        }
    }

    /// One TCK pulse with the given TMS/TDI: returns TDO sampled on +ve.
    fn clock(&mut self, tms: bool, tdi: bool) -> Result<bool, String> {
        let need = pin::TCK | pin::TDI | pin::TMS;
        if self.low_dir & need != need {
            return Err(format!(
                "mock FTDI: TCK/TDI/TMS are not outputs (ADBUS dir {:#04x}); \
                 send SET_LOW with direction 0x0B before clocking",
                self.low_dir
            ));
        }
        if self.low_dir & pin::TDO != 0 {
            return Err(
                "mock FTDI: TDO (ADBUS2) driven as an output — would fight the target".into(),
            );
        }
        if self.adaptive {
            return Err(
                "mock FTDI: adaptive clocking on but the board has no RTCK — the chip would stall"
                    .into(),
            );
        }
        if self.three_phase {
            return Err("mock FTDI: 3-phase clocking is for I2C; JTAG targets mis-sample".into());
        }
        self.set_pin(pin::TMS, tms);
        self.set_pin(pin::TDI, tdi);
        self.tck_count += 1;
        let mut bit = tdi;
        for d in self.chain.iter_mut().rev() {
            bit = d.tick(tms, bit);
        }
        Ok(if self.loopback { tdi } else { bit })
    }

    fn need(&self, at: usize, n: usize) -> bool {
        self.pending.len() >= at + n
    }

    /// Execute as many complete commands as `pending` holds.
    fn process(&mut self) -> Result<(), String> {
        let mut at = 0usize;
        while at < self.pending.len() {
            let opc = self.pending[at];
            let used = if opc & 0x80 == 0 {
                match self.shift_cmd(at)? {
                    Some(n) => n,
                    None => break,
                }
            } else {
                match self.ctrl_cmd(at)? {
                    Some(n) => n,
                    None => break,
                }
            };
            self.opcodes.push(opc);
            at += used;
        }
        self.pending.drain(..at);
        Ok(())
    }

    /// Control opcodes (bit 7 set). Returns bytes consumed or `None` if incomplete.
    fn ctrl_cmd(&mut self, at: usize) -> Result<Option<usize>, String> {
        let opc = self.pending[at];
        let arg = |s: &Self, i: usize| s.pending[at + i];
        let n = match opc {
            op::SET_LOW | op::SET_HIGH | op::SET_DIVISOR => {
                if !self.need(at, 3) {
                    return Ok(None);
                }
                let (a, b) = (arg(self, 1), arg(self, 2));
                match opc {
                    op::SET_LOW => {
                        // TDO keeps reading the target, not the latch.
                        self.low_val = a;
                        self.low_dir = b;
                    }
                    op::SET_HIGH => {
                        self.high_val = a;
                        self.high_dir = b;
                    }
                    _ => self.divisor = u16::from(a) | (u16::from(b) << 8),
                }
                3
            }
            op::GET_LOW => {
                self.rx.push_back(self.low_val);
                1
            }
            op::GET_HIGH => {
                self.rx.push_back(self.high_val);
                1
            }
            op::LOOPBACK_ON | op::LOOPBACK_OFF => {
                self.loopback = opc == op::LOOPBACK_ON;
                1
            }
            op::SEND_IMMEDIATE => 1,
            op::DIV5_OFF | op::DIV5_ON => {
                self.div5 = opc == op::DIV5_ON;
                1
            }
            op::THREE_PHASE_ON | op::THREE_PHASE_OFF => {
                self.three_phase = opc == op::THREE_PHASE_ON;
                1
            }
            op::ADAPTIVE_ON | op::ADAPTIVE_OFF => {
                self.adaptive = opc == op::ADAPTIVE_ON;
                1
            }
            op::CLOCK_BITS => {
                if !self.need(at, 2) {
                    return Ok(None);
                }
                let n = arg(self, 1) as usize + 1;
                for _ in 0..n {
                    let (tms, tdi) = (self.tms_pin(), self.tdi_pin());
                    self.clock(tms, tdi)?;
                }
                2
            }
            op::CLOCK_BYTES => {
                if !self.need(at, 3) {
                    return Ok(None);
                }
                let n = (usize::from(arg(self, 1)) | (usize::from(arg(self, 2)) << 8)) + 1;
                for _ in 0..n * 8 {
                    let (tms, tdi) = (self.tms_pin(), self.tdi_pin());
                    self.clock(tms, tdi)?;
                }
                3
            }
            _ => {
                // AN_108 §4.2: bad command → 0xFA then the opcode.
                self.rx.push_back(op::BAD_COMMAND);
                self.rx.push_back(opc);
                1
            }
        };
        Ok(Some(n))
    }

    /// Shifting opcodes (bit 7 clear). Returns bytes consumed or `None` if incomplete.
    fn shift_cmd(&mut self, at: usize) -> Result<Option<usize>, String> {
        let opc = self.pending[at];
        let bit_mode = opc & flag::BIT_MODE != 0;
        let lsb = opc & flag::LSB_FIRST != 0;
        let w_tdi = opc & flag::DO_WRITE_TDI != 0;
        let r_tdo = opc & flag::DO_READ_TDO != 0;
        let w_tms = opc & flag::DO_WRITE_TMS != 0;
        let invalid = (w_tms && (w_tdi || !bit_mode)) || !(w_tdi || r_tdo || w_tms);
        if invalid {
            self.rx.push_back(op::BAD_COMMAND);
            self.rx.push_back(opc);
            return Ok(Some(1));
        }
        if (w_tdi || w_tms) && opc & flag::WRITE_NEG == 0 {
            return Err(format!(
                "mock FTDI: opcode {opc:#04x} changes TDI/TMS on the rising TCK edge — \
                 a JTAG target samples there (encoder bug)"
            ));
        }
        if r_tdo && opc & flag::READ_NEG != 0 {
            return Err(format!(
                "mock FTDI: opcode {opc:#04x} samples TDO on the falling edge — \
                 JTAG TDO is valid on the rising edge (encoder bug)"
            ));
        }
        if bit_mode {
            if !self.need(at, 2) {
                return Ok(None);
            }
            let n = self.pending[at + 1] as usize + 1;
            if n > 8 || (w_tms && n > 7) {
                return Err(format!(
                    "mock FTDI: opcode {opc:#04x} length {n} bits out of range"
                ));
            }
            let has_data = w_tdi || w_tms;
            let used = if has_data { 3 } else { 2 };
            if !self.need(at, used) {
                return Ok(None);
            }
            let data = if has_data { self.pending[at + 2] } else { 0 };
            let mut got = 0u8;
            if w_tms {
                // TMS bits LSB first; bit 7 holds TDI for the whole burst.
                let tdi = data & 0x80 != 0;
                for i in 0..n {
                    let tdo = self.clock((data >> i) & 1 == 1, tdi)?;
                    got = (got >> 1) | (u8::from(tdo) << 7);
                }
            } else {
                let tms = self.tms_pin();
                for i in 0..n {
                    let b = if lsb {
                        (data >> i) & 1
                    } else {
                        (data >> (7 - i)) & 1
                    };
                    let tdi = if w_tdi { b == 1 } else { self.tdi_pin() };
                    let tdo = self.clock(tms, tdi)?;
                    got = if lsb {
                        (got >> 1) | (u8::from(tdo) << 7)
                    } else {
                        (got << 1) | u8::from(tdo)
                    };
                }
            }
            if r_tdo {
                self.rx.push_back(got);
            }
            Ok(Some(used))
        } else {
            if !self.need(at, 3) {
                return Ok(None);
            }
            let n =
                (usize::from(self.pending[at + 1]) | (usize::from(self.pending[at + 2]) << 8)) + 1;
            let used = if w_tdi { 3 + n } else { 3 };
            if !self.need(at, used) {
                return Ok(None);
            }
            let tms = self.tms_pin();
            for k in 0..n {
                let data = if w_tdi { self.pending[at + 3 + k] } else { 0 };
                let mut got = 0u8;
                for i in 0..8 {
                    let b = if lsb {
                        (data >> i) & 1
                    } else {
                        (data >> (7 - i)) & 1
                    };
                    let tdi = if w_tdi { b == 1 } else { self.tdi_pin() };
                    let tdo = self.clock(tms, tdi)?;
                    if tdo {
                        got |= if lsb { 1 << i } else { 1 << (7 - i) };
                    }
                }
                if r_tdo {
                    self.rx.push_back(got);
                }
            }
            Ok(Some(used))
        }
    }
}

impl MpssePort for MockFtdi {
    fn describe(&self) -> String {
        let n = self.chain.len();
        format!(
            "mock FTDI MPSSE · {n}-device chain{}",
            if n == 1 { "" } else { " (others in BYPASS)" }
        )
    }

    fn usb_reset(&mut self) -> Result<(), String> {
        self.pending.clear();
        self.rx.clear();
        self.resets += 1;
        Ok(())
    }

    fn set_latency_timer(&mut self, ms: u8) -> Result<(), String> {
        if ms == 0 {
            return Err("mock FTDI: latency timer must be 1..=255 ms".into());
        }
        self.latency_ms = ms;
        Ok(())
    }

    fn set_bitmode(&mut self, _mask: u8, mode: u8) -> Result<(), String> {
        match mode {
            crate::mpsse::BITMODE_RESET => self.mpsse = false,
            crate::mpsse::BITMODE_MPSSE => self.mpsse = true,
            other => return Err(format!("mock FTDI: bitmode {other:#04x} not modelled")),
        }
        Ok(())
    }

    fn purge(&mut self) -> Result<(), String> {
        self.pending.clear();
        self.rx.clear();
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        if !self.mpsse {
            return Err("mock FTDI: channel not in MPSSE mode (SET_BITMODE 0x02 first)".into());
        }
        self.bytes_written += data.len();
        self.pending.extend_from_slice(data);
        self.process()
    }

    fn read(&mut self, n: usize) -> Result<Vec<u8>, String> {
        if self.rx.len() < n {
            return Err(format!(
                "mock FTDI: read timeout — {} of {n} byte(s) available",
                self.rx.len()
            ));
        }
        self.bytes_read += n;
        Ok(self.rx.drain(..n).collect())
    }

    fn board(&mut self) -> Option<&mut Tap> {
        self.helion_tap_mut()
    }

    fn is_mock(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jtag_engine::JtagEngine;
    use crate::mpsse::MpsseCmd;
    use crate::{IR_IDCODE, IR_USR2, IR_USR4};
    use helion_bits::Bitstream;
    use helion_fabric::Stat;

    fn dev() -> Device {
        Device::load_part("HL10T-C32-1").unwrap()
    }

    pub(crate) fn counter_bits(d: &Device) -> Bitstream {
        let packed = helion_pack::pack(&helion_ir::Design::structural_counter(), d).unwrap();
        let placed = helion_place::place(&packed, d).unwrap();
        let routed = helion_route::route(&placed, d).unwrap();
        helion_bits::bitgen(d, &routed).unwrap()
    }

    #[test]
    fn refuses_bytes_before_mpsse_and_answers_bad_commands() {
        let mut m = MockFtdi::helion(&dev());
        assert!(m.write(&[0x87]).is_err());
        m.set_bitmode(0x0B, 0x02).unwrap();
        m.write(&[0xAA, 0x87]).unwrap();
        assert_eq!(m.read(2).unwrap(), vec![0xFA, 0xAA]);
        assert!(m.read(1).is_err(), "no invented bytes");
    }

    #[test]
    fn refuses_clocking_with_pins_as_inputs_and_wrong_edges() {
        let mut m = MockFtdi::helion(&dev());
        m.set_bitmode(0x0B, 0x02).unwrap();
        assert!(m.write(&[0x4B, 0x00, 0x01]).is_err(), "pins not outputs");
        let mut m = MockFtdi::helion(&dev());
        m.set_bitmode(0x0B, 0x02).unwrap();
        m.write(&[0x80, 0x08, 0x0B]).unwrap();
        // 0x18 = bytes out on +ve edge: wrong for JTAG.
        let e = m.write(&[0x18, 0x00, 0x00, 0xFF]).unwrap_err();
        assert!(e.contains("rising"), "{e}");
    }

    #[test]
    fn commands_may_straddle_usb_writes() {
        let mut m = MockFtdi::helion(&dev());
        m.set_bitmode(0x0B, 0x02).unwrap();
        m.write(&[0x80, 0x08]).unwrap();
        m.write(&[0x0B, 0x4B]).unwrap();
        m.write(&[0x04, 0x1F]).unwrap();
        assert_eq!(m.tck_count, 5);
        assert_eq!(
            m.helion_tap().unwrap().state,
            crate::TapState::TestLogicReset
        );
    }

    #[test]
    fn hand_encoded_idcode_scan_reads_the_helion_idcode() {
        // Byte-for-byte stream a host would send: reset, Shift-DR, 31 bits
        // in/out, last bit + Exit1/Update/Idle via TMS read.
        let d = dev();
        let mut m = MockFtdi::helion(&d);
        m.set_bitmode(0x0B, 0x02).unwrap();
        let stream = [
            0x80, 0x08, 0x0B, // pins
            0x4B, 0x05, 0x1F, // TMS 1,1,1,1,1,0 → Idle
            0x4B, 0x02, 0x01, // TMS 1,0,0 → Shift-DR
            0x39, 0x02, 0x00, 0x00, 0x00, 0x00, // 24 bits in/out
            0x3B, 0x06, 0x00, // 7 bits in/out
            0x6B, 0x02, 0x03, // last bit, TMS 1,1,0
            0x87,
        ];
        m.write(&stream).unwrap();
        let rx = m.read(5).unwrap();
        let mut id = u32::from(rx[0]) | (u32::from(rx[1]) << 8) | (u32::from(rx[2]) << 16);
        id |= u32::from(rx[3] >> 1) << 24; // 7 bits left-aligned at bit 1
        id |= u32::from(rx[4] >> 5 & 1) << 31; // first of 3 TMS-read bits at bit 5
        assert_eq!(id, d.idcode);
    }

    #[test]
    fn engine_opens_scans_and_reads_idcode_stat() {
        let d = dev();
        let mut e = JtagEngine::open(MockFtdi::helion(&d), 6_000_000).unwrap();
        assert_eq!(e.chain().len(), 1);
        assert_eq!(e.chain()[0].idcode, Some(d.idcode));
        assert_eq!(e.chain()[0].ir_len, 6);
        assert_eq!(e.tck_hz(), 6_000_000);
        assert_eq!(e.port().divisor, 4);
        assert!(!e.port().div5);
        assert_eq!(e.read_idcode().unwrap(), d.idcode);
        assert_eq!(e.read_stat_word().unwrap(), Stat::RESET_WORD);
        // Captured IR equals the previous instruction (Helion capture = IR).
        assert_eq!(
            e.shift_ir(u64::from(IR_IDCODE)).unwrap(),
            u64::from(crate::IR_STAT)
        );
        assert!(e.stats.bytes_in > 0 && e.stats.dr_scans >= 2);
        assert_eq!(e.set_tck_hz(1_000_000).unwrap(), 1_000_000);
        assert_eq!(e.port().divisor, 29);
    }

    #[test]
    fn multi_device_chain_targets_helion_with_neighbours_in_bypass() {
        let d = dev();
        let chain = vec![
            MockChainDevice::Generic(GenericTap::new(Some(0x0362_D093), 6)),
            MockChainDevice::Generic(GenericTap::new(None, 5)),
            MockChainDevice::Helion(Box::new(Tap::new(&d))),
            MockChainDevice::Generic(GenericTap::new(Some(0x0BA0_0477), 4)),
        ];
        let mut e = JtagEngine::open(MockFtdi::with_chain(chain), 15_000_000).unwrap();
        let ids: Vec<_> = e.chain().iter().map(|c| c.idcode).collect();
        assert_eq!(
            ids,
            vec![Some(0x0362_D093), None, Some(d.idcode), Some(0x0BA0_0477)]
        );
        let lens: Vec<_> = e.chain().iter().map(|c| c.ir_len).collect();
        assert_eq!(
            lens,
            vec![6, 5, 6, 4],
            "bypass-only device gets the remainder"
        );
        assert_eq!(e.target(), 2, "Helion picked as target");
        assert_eq!(e.read_idcode().unwrap(), d.idcode);
        // CFG_W through three bypassed neighbours.
        assert!(
            e.program(&Bitstream::empty(&d)).is_err(),
            "empty bitstream refused"
        );
        let bits = counter_bits(&d);
        let st = e.program(&bits).unwrap();
        assert!(st.done);
        assert!(e.port().helion_tap().unwrap().fabric().stat.done);
        // Neighbour access: the Xilinx part's IDCODE via its own IR.
        e.select_target(0).unwrap();
        e.shift_ir(0b000001).unwrap();
        assert_eq!(e.shift_dr_u32(0).unwrap(), 0x0362_D093);
    }

    #[test]
    fn user_chains_two_to_four_round_trip() {
        let d = dev();
        let mut e = JtagEngine::open(MockFtdi::helion(&d), 6_000_000).unwrap();
        for (chain, v) in [
            (2u8, 0x1122_3344_5566_7788u64),
            (3, 0xDEAD_BEEF),
            (4, u64::MAX),
        ] {
            let _ = e.user_dr(chain, &v.to_le_bytes(), 64).unwrap();
            let back = e.user_dr(chain, &0u64.to_le_bytes(), 64).unwrap();
            assert_eq!(
                u64::from_le_bytes(back[..8].try_into().unwrap()),
                v,
                "USER{chain}"
            );
        }
        let tap = e.port().helion_tap().unwrap();
        assert_eq!(tap.user_register(IR_USR2), Some(0));
        assert_eq!(tap.user_register(IR_USR4), Some(0));
        assert!(e.user_dr(5, &[0], 1).is_err());
    }

    #[test]
    fn chain_scan_reports_dead_tdo() {
        // An empty chain: TDO is the TDI flood (all ones) → no device.
        let r = JtagEngine::open(MockFtdi::with_chain(Vec::new()), 1_000_000);
        assert!(r.unwrap_err().contains("no device"));
    }

    #[test]
    fn sync_failure_is_reported() {
        struct Dumb;
        impl MpssePort for Dumb {
            fn describe(&self) -> String {
                "dumb".into()
            }
            fn usb_reset(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn set_latency_timer(&mut self, _: u8) -> Result<(), String> {
                Ok(())
            }
            fn set_bitmode(&mut self, _: u8, _: u8) -> Result<(), String> {
                Ok(())
            }
            fn purge(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn write(&mut self, _: &[u8]) -> Result<(), String> {
                Ok(())
            }
            fn read(&mut self, n: usize) -> Result<Vec<u8>, String> {
                Ok(vec![0; n])
            }
        }
        let e = JtagEngine::open(Dumb, 1_000_000).unwrap_err();
        assert!(e.contains("sync"), "{e}");
    }

    #[test]
    fn get_low_reads_pin_latch() {
        let mut m = MockFtdi::helion(&dev());
        m.set_bitmode(0x0B, 0x02).unwrap();
        let mut c = MpsseCmd::new();
        c.set_low(0x08, 0x0B).get_low();
        m.write(c.bytes()).unwrap();
        assert_eq!(m.read(1).unwrap(), vec![0x08]);
    }
}
