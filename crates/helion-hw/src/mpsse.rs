//! FTDI MPSSE command-stream encoder for JTAG (FTDI AN_108, "Command
//! Processor for MPSSE and MCU Host Bus Emulation Modes", and AN_135 /
//! AN_129 for the JTAG pin map and init sequence).
//!
//! Everything here is pure byte encoding — no USB. [`MpsseCmd`] records how
//! many bytes the chip will return for the queued commands and where the
//! clocked-in bits sit inside each returned byte, so a scan can be decoded
//! after one bulk-IN.
//!
//! Bit-mode reads (`0x3B`, `0x6B`): the chip shifts each sampled bit in at
//! bit 7 and moves it right, so after `n` clocks the bits occupy the top `n`
//! bits of the returned byte, first bit at bit `8 - n` (OpenOCD `mpsse.c`
//! `buffer_add_read(…, 8 - this_bits)`).
//!
//! JTAG timing: TDI/TMS change on the falling TCK edge and TDO is sampled on
//! the rising edge, so every data opcode here has `WRITE_NEG` set and
//! `READ_NEG` clear, LSB first.

/// Opcode flag bits for the shifting commands (AN_108 §3.2).
pub mod flag {
    /// Write TDI/TMS on the falling (-ve) TCK edge.
    pub const WRITE_NEG: u8 = 0x01;
    /// Length is in bits (else bytes).
    pub const BIT_MODE: u8 = 0x02;
    /// Read TDO on the falling edge (JTAG never sets this).
    pub const READ_NEG: u8 = 0x04;
    /// LSB first.
    pub const LSB_FIRST: u8 = 0x08;
    /// Clock data out on TDI.
    pub const DO_WRITE_TDI: u8 = 0x10;
    /// Clock data in from TDO.
    pub const DO_READ_TDO: u8 = 0x20;
    /// Clock data out on TMS.
    pub const DO_WRITE_TMS: u8 = 0x40;
}

/// Opcodes used by the JTAG engine (values from AN_108).
pub mod op {
    /// Clock data bytes out on -ve edge, LSB first, no read.
    pub const BYTES_OUT: u8 = 0x19;
    /// Clock data bits out on -ve edge, LSB first, no read.
    pub const BITS_OUT: u8 = 0x1B;
    /// Clock data bytes in (+ve) and out (-ve), LSB first.
    pub const BYTES_INOUT: u8 = 0x39;
    /// Clock data bits in (+ve) and out (-ve), LSB first.
    pub const BITS_INOUT: u8 = 0x3B;
    /// Clock data to TMS pin (no read), -ve edge, LSB first.
    pub const TMS_OUT: u8 = 0x4B;
    /// Clock data to TMS pin with read, out -ve / in +ve, LSB first.
    pub const TMS_INOUT: u8 = 0x6B;
    /// Set data bits low byte (ADBUS): value, direction.
    pub const SET_LOW: u8 = 0x80;
    /// Read data bits low byte.
    pub const GET_LOW: u8 = 0x81;
    /// Set data bits high byte (ACBUS): value, direction.
    pub const SET_HIGH: u8 = 0x82;
    /// Read data bits high byte.
    pub const GET_HIGH: u8 = 0x83;
    /// Connect TDI to TDO for loopback.
    pub const LOOPBACK_ON: u8 = 0x84;
    /// Disconnect TDI/TDO loopback.
    pub const LOOPBACK_OFF: u8 = 0x85;
    /// Set TCK divisor: low byte, high byte.
    pub const SET_DIVISOR: u8 = 0x86;
    /// Flush the chip's buffer to the host now.
    pub const SEND_IMMEDIATE: u8 = 0x87;
    /// Disable the 60 MHz master clock divide-by-5 (H-series).
    pub const DIV5_OFF: u8 = 0x8A;
    /// Enable the divide-by-5 (12 MHz master, FT2232D-compatible).
    pub const DIV5_ON: u8 = 0x8B;
    /// Enable 3-phase data clocking (I2C only).
    pub const THREE_PHASE_ON: u8 = 0x8C;
    /// Disable 3-phase data clocking.
    pub const THREE_PHASE_OFF: u8 = 0x8D;
    /// Clock for n bits (1..=8) with no data transfer.
    pub const CLOCK_BITS: u8 = 0x8E;
    /// Clock for n×8 bits with no data transfer.
    pub const CLOCK_BYTES: u8 = 0x8F;
    /// Enable adaptive clocking (RTCK).
    pub const ADAPTIVE_ON: u8 = 0x96;
    /// Disable adaptive clocking.
    pub const ADAPTIVE_OFF: u8 = 0x97;
    /// The chip answers an unknown opcode with this byte then the opcode.
    pub const BAD_COMMAND: u8 = 0xFA;
}

/// ADBUS pin map for JTAG (AN_129): bit0 TCK, bit1 TDI, bit2 TDO, bit3 TMS.
pub mod pin {
    pub const TCK: u8 = 0x01;
    pub const TDI: u8 = 0x02;
    pub const TDO: u8 = 0x04;
    pub const TMS: u8 = 0x08;
    /// Outputs: TCK, TDI, TMS; TDO is an input.
    pub const JTAG_DIR: u8 = TCK | TDI | TMS;
    /// Idle levels: TCK low, TDI low, TMS high.
    pub const JTAG_IDLE: u8 = TMS;
}

/// GPIO levels a cable needs around the four JTAG pins. Digilent cables gate
/// their JTAG level shifters with ADBUS7 and ACBUS5/6: with the generic
/// profile the buffers stay off and TDO floats high (no chain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CablePins {
    pub name: &'static str,
    pub low_val: u8,
    pub low_dir: u8,
    pub high_val: u8,
    pub high_dir: u8,
}

impl CablePins {
    /// Bare AN_129 wiring: TCK/TDI/TMS out, TDO in, nothing else driven.
    pub const GENERIC: Self = Self {
        name: "generic FTDI",
        low_val: pin::JTAG_IDLE,
        low_dir: pin::JTAG_DIR,
        high_val: 0x00,
        high_dir: 0x00,
    };
    /// Digilent HS2/HS3 and on-board Digilent JTAG (ZedBoard, Arty, Basys,
    /// Nexys): ADBUS5-7 out (ADBUS7 high enables the buffer), ACBUS5/6 out low.
    pub const DIGILENT: Self = Self {
        name: "Digilent",
        low_val: 0xE8,
        low_dir: 0xEB,
        high_val: 0x00,
        high_dir: 0x60,
    };

    /// Pick the profile from the USB descriptor strings.
    pub fn for_usb(manufacturer: Option<&str>, product: Option<&str>) -> Self {
        let is = |s: Option<&str>, w: &str| s.is_some_and(|s| s.to_ascii_lowercase().contains(w));
        if is(manufacturer, "digilent") || is(product, "digilent") {
            Self::DIGILENT
        } else {
            Self::GENERIC
        }
    }
}

/// FTDI `SIO_SET_BITMODE` mode for MPSSE.
pub const BITMODE_MPSSE: u8 = 0x02;
/// FTDI `SIO_SET_BITMODE` mode for reset (back to UART/FIFO).
pub const BITMODE_RESET: u8 = 0x00;

/// Above this TCK the engine samples TDO on the falling edge. Measured on a
/// ZedBoard (on-board Digilent FT232H + level shifters): rising-edge reads
/// are exact up to 15 MHz and every bit arrives one clock late at 30 MHz.
pub const READ_FALLING_ABOVE_HZ: u32 = 15_000_000;

/// TCK frequency in Hz for `divisor`: `master / ((1 + divisor) * 2)` where
/// the master clock is 60 MHz with divide-by-5 off (H-series) or 12 MHz with
/// it on (AN_108 §3.8).
pub fn tck_hz(divisor: u16, div5: bool) -> u32 {
    let master: u64 = if div5 { 12_000_000 } else { 60_000_000 };
    (master / ((1 + u64::from(divisor)) * 2)) as u32
}

/// Smallest divisor (fastest clock) whose TCK does not exceed `hz` with
/// divide-by-5 off. `hz` above 30 MHz clamps to divisor 0 (30 MHz).
pub fn divisor_for_hz(hz: u32) -> u16 {
    if hz == 0 {
        return u16::MAX;
    }
    let half = 30_000_000u64;
    let d = half.div_ceil(u64::from(hz)).saturating_sub(1);
    d.min(u64::from(u16::MAX)) as u16
}

/// Where the bits of one queued read land in the returned bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadSeg {
    /// `n` whole bytes, LSB first.
    Bytes(usize),
    /// `n` (1..=8) bits from one byte, left-aligned: first bit at `8 - n`.
    Bits(u8),
}

impl ReadSeg {
    pub fn byte_len(self) -> usize {
        match self {
            ReadSeg::Bytes(n) => n,
            ReadSeg::Bits(_) => 1,
        }
    }

    pub fn bit_len(self) -> usize {
        match self {
            ReadSeg::Bytes(n) => n * 8,
            ReadSeg::Bits(n) => n as usize,
        }
    }
}

/// Max bytes per data-byte command (16-bit length field holds count - 1).
pub const MAX_BYTES_PER_CMD: usize = 65_536;

/// One MPSSE command stream plus its read layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MpsseCmd {
    buf: Vec<u8>,
    reads: Vec<ReadSeg>,
    read_falling: bool,
}

impl MpsseCmd {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Sample TDO on the falling TCK edge in the read opcodes queued after
    /// this. At high TCK a buffered cable's TDO arrives later than half a
    /// period; the falling edge gives it a full period (see [`READ_FALLING_ABOVE_HZ`]).
    pub fn read_on_falling(&mut self, on: bool) -> &mut Self {
        self.read_falling = on;
        self
    }

    fn read_op(&self, opc: u8) -> u8 {
        if self.read_falling {
            opc | flag::READ_NEG
        } else {
            opc
        }
    }

    pub fn reads(&self) -> &[ReadSeg] {
        &self.reads
    }

    /// Bytes the chip returns for this stream.
    pub fn read_len(&self) -> usize {
        self.reads.iter().map(|r| r.byte_len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.reads.clear();
    }

    pub fn set_low(&mut self, value: u8, dir: u8) -> &mut Self {
        self.buf.extend_from_slice(&[op::SET_LOW, value, dir]);
        self
    }

    pub fn set_high(&mut self, value: u8, dir: u8) -> &mut Self {
        self.buf.extend_from_slice(&[op::SET_HIGH, value, dir]);
        self
    }

    pub fn get_low(&mut self) -> &mut Self {
        self.buf.push(op::GET_LOW);
        self.reads.push(ReadSeg::Bytes(1));
        self
    }

    pub fn divisor(&mut self, div: u16) -> &mut Self {
        self.buf
            .extend_from_slice(&[op::SET_DIVISOR, (div & 0xFF) as u8, (div >> 8) as u8]);
        self
    }

    pub fn div5(&mut self, on: bool) -> &mut Self {
        self.buf.push(if on { op::DIV5_ON } else { op::DIV5_OFF });
        self
    }

    pub fn adaptive(&mut self, on: bool) -> &mut Self {
        self.buf.push(if on {
            op::ADAPTIVE_ON
        } else {
            op::ADAPTIVE_OFF
        });
        self
    }

    pub fn three_phase(&mut self, on: bool) -> &mut Self {
        self.buf.push(if on {
            op::THREE_PHASE_ON
        } else {
            op::THREE_PHASE_OFF
        });
        self
    }

    pub fn loopback(&mut self, on: bool) -> &mut Self {
        self.buf.push(if on {
            op::LOOPBACK_ON
        } else {
            op::LOOPBACK_OFF
        });
        self
    }

    pub fn send_immediate(&mut self) -> &mut Self {
        self.buf.push(op::SEND_IMMEDIATE);
        self
    }

    /// A deliberately invalid opcode: the chip answers `0xFA, opcode`
    /// (AN_135 §5.3 "synchronise the MPSSE").
    pub fn bogus(&mut self, opcode: u8) -> &mut Self {
        self.buf.push(opcode);
        self.reads.push(ReadSeg::Bytes(2));
        self
    }

    /// Clock `n` (1..=7) TMS bits LSB first with TDI held at `tdi`.
    pub fn tms(&mut self, bits: u8, n: u8, tdi: bool, read: bool) -> &mut Self {
        assert!((1..=7).contains(&n), "TMS burst is 1..=7 bits");
        let mut b = bits & ((1u8 << n) - 1);
        if tdi {
            b |= 0x80;
        }
        let opc = if read {
            self.read_op(op::TMS_INOUT)
        } else {
            op::TMS_OUT
        };
        self.buf.extend_from_slice(&[opc, n - 1, b]);
        if read {
            self.reads.push(ReadSeg::Bits(n));
        }
        self
    }

    /// Clock whole bytes on TDI (TMS held), split at [`MAX_BYTES_PER_CMD`].
    pub fn data_bytes(&mut self, data: &[u8], read: bool) -> &mut Self {
        let opc = if read {
            self.read_op(op::BYTES_INOUT)
        } else {
            op::BYTES_OUT
        };
        for chunk in data.chunks(MAX_BYTES_PER_CMD) {
            let n = chunk.len() - 1;
            self.buf
                .extend_from_slice(&[opc, (n & 0xFF) as u8, (n >> 8) as u8]);
            self.buf.extend_from_slice(chunk);
            if read {
                self.reads.push(ReadSeg::Bytes(chunk.len()));
            }
        }
        self
    }

    /// Clock `n` (1..=8) bits on TDI, LSB first.
    pub fn data_bits(&mut self, bits: u8, n: u8, read: bool) -> &mut Self {
        assert!((1..=8).contains(&n), "bit burst is 1..=8 bits");
        let opc = if read {
            self.read_op(op::BITS_INOUT)
        } else {
            op::BITS_OUT
        };
        let mask = if n == 8 { 0xFF } else { (1u8 << n) - 1 };
        self.buf.extend_from_slice(&[opc, n - 1, bits & mask]);
        if read {
            self.reads.push(ReadSeg::Bits(n));
        }
        self
    }

    /// Clock `n` (1..=8) TCK pulses with no data transfer.
    pub fn clock_bits(&mut self, n: u8) -> &mut Self {
        assert!((1..=8).contains(&n));
        self.buf.extend_from_slice(&[op::CLOCK_BITS, n - 1]);
        self
    }

    /// Append another stream (reads keep their order).
    pub fn extend(&mut self, other: &MpsseCmd) -> &mut Self {
        self.buf.extend_from_slice(&other.buf);
        self.reads.extend_from_slice(&other.reads);
        self
    }

    /// Standard JTAG init after the chip is in MPSSE mode (AN_129 §4):
    /// divide-by-5 off, adaptive clocking off, 3-phase off, loopback off,
    /// ADBUS idle levels/directions, ACBUS all inputs, TCK divisor.
    pub fn jtag_init(&mut self, divisor: u16) -> &mut Self {
        self.jtag_init_with(divisor, CablePins::GENERIC)
    }

    /// [`Self::jtag_init`] with a cable's extra GPIO levels.
    pub fn jtag_init_with(&mut self, divisor: u16, pins: CablePins) -> &mut Self {
        self.div5(false)
            .adaptive(false)
            .three_phase(false)
            .loopback(false)
            .set_low(pins.low_val, pins.low_dir)
            .set_high(pins.high_val, pins.high_dir)
            .divisor(divisor)
    }

    /// Unpack the returned bytes into a flat LSB-first bit vector, one entry
    /// per queued read segment (in order).
    pub fn decode(&self, rx: &[u8]) -> Result<Vec<Vec<bool>>, String> {
        if rx.len() < self.read_len() {
            return Err(format!(
                "MPSSE short read: {} of {} byte(s)",
                rx.len(),
                self.read_len()
            ));
        }
        let mut at = 0usize;
        let mut out = Vec::with_capacity(self.reads.len());
        for seg in &self.reads {
            let mut bits = Vec::with_capacity(seg.bit_len());
            match *seg {
                ReadSeg::Bytes(n) => {
                    for b in &rx[at..at + n] {
                        for i in 0..8 {
                            bits.push((b >> i) & 1 == 1);
                        }
                    }
                    at += n;
                }
                ReadSeg::Bits(n) => {
                    let b = rx[at];
                    let lo = 8 - n;
                    for i in 0..n {
                        bits.push((b >> (lo + i)) & 1 == 1);
                    }
                    at += 1;
                }
            }
            out.push(bits);
        }
        Ok(out)
    }
}

/// Pack LSB-first bits into bytes.
pub fn pack_bits(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// Unpack `n` LSB-first bits from bytes.
pub fn unpack_bits(bytes: &[u8], n: usize) -> Vec<bool> {
    (0..n)
        .map(|i| bytes.get(i / 8).is_some_and(|b| (b >> (i % 8)) & 1 == 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_values_match_an108() {
        // Data shifting opcodes are the flag sums AN_108 §3.3/3.4/3.5 lists.
        use flag::*;
        assert_eq!(op::BYTES_OUT, WRITE_NEG | LSB_FIRST | DO_WRITE_TDI);
        assert_eq!(
            op::BITS_OUT,
            WRITE_NEG | BIT_MODE | LSB_FIRST | DO_WRITE_TDI
        );
        assert_eq!(
            op::BYTES_INOUT,
            WRITE_NEG | LSB_FIRST | DO_WRITE_TDI | DO_READ_TDO
        );
        assert_eq!(
            op::BITS_INOUT,
            WRITE_NEG | BIT_MODE | LSB_FIRST | DO_WRITE_TDI | DO_READ_TDO
        );
        assert_eq!(op::TMS_OUT, WRITE_NEG | BIT_MODE | LSB_FIRST | DO_WRITE_TMS);
        assert_eq!(
            op::TMS_INOUT,
            WRITE_NEG | BIT_MODE | LSB_FIRST | DO_WRITE_TMS | DO_READ_TDO
        );
        // Every JTAG opcode writes on -ve and reads on +ve.
        for o in [
            op::BYTES_OUT,
            op::BITS_OUT,
            op::BYTES_INOUT,
            op::BITS_INOUT,
            op::TMS_OUT,
            op::TMS_INOUT,
        ] {
            assert_eq!(o & WRITE_NEG, WRITE_NEG);
            assert_eq!(o & READ_NEG, 0);
        }
    }

    #[test]
    fn encodes_byte_for_byte() {
        let mut c = MpsseCmd::new();
        c.data_bytes(&[0xA5, 0x5A], true);
        assert_eq!(c.bytes(), &[0x39, 0x01, 0x00, 0xA5, 0x5A]);
        let mut c = MpsseCmd::new();
        c.data_bytes(&[0x12], false);
        assert_eq!(c.bytes(), &[0x19, 0x00, 0x00, 0x12]);
        let mut c = MpsseCmd::new();
        c.data_bits(0b101, 3, false);
        assert_eq!(c.bytes(), &[0x1B, 0x02, 0x05]);
        let mut c = MpsseCmd::new();
        c.data_bits(0xFF, 7, true);
        assert_eq!(c.bytes(), &[0x3B, 0x06, 0x7F]);
        // TMS 1,1,0 with TDI=1 held in bit 7.
        let mut c = MpsseCmd::new();
        c.tms(0b011, 3, true, true);
        assert_eq!(c.bytes(), &[0x6B, 0x02, 0x83]);
        // Five TMS ones (Test-Logic-Reset), TDI low.
        let mut c = MpsseCmd::new();
        c.tms(0x1F, 5, false, false);
        assert_eq!(c.bytes(), &[0x4B, 0x04, 0x1F]);
        let mut c = MpsseCmd::new();
        c.set_low(0x08, 0x0B).set_high(0x00, 0x00).divisor(0x05DB);
        assert_eq!(
            c.bytes(),
            &[0x80, 0x08, 0x0B, 0x82, 0x00, 0x00, 0x86, 0xDB, 0x05]
        );
        let mut c = MpsseCmd::new();
        c.clock_bits(8).get_low().send_immediate();
        assert_eq!(c.bytes(), &[0x8E, 0x07, 0x81, 0x87]);
        assert_eq!(c.read_len(), 1);
    }

    #[test]
    fn jtag_init_is_the_an129_sequence() {
        let mut c = MpsseCmd::new();
        c.jtag_init(0x0001);
        assert_eq!(
            c.bytes(),
            &[
                0x8A, 0x97, 0x8D, 0x85, 0x80, 0x08, 0x0B, 0x82, 0x00, 0x00, 0x86, 0x01, 0x00
            ]
        );
        assert_eq!(c.read_len(), 0);
    }

    #[test]
    fn digilent_cables_enable_their_jtag_buffers() {
        assert_eq!(
            CablePins::for_usb(Some("Digilent"), Some("Digilent USB Device")),
            CablePins::DIGILENT
        );
        assert_eq!(
            CablePins::for_usb(Some("FTDI"), Some("Dual RS232-HS")),
            CablePins::GENERIC
        );
        let mut c = MpsseCmd::new();
        c.jtag_init_with(0x0001, CablePins::DIGILENT);
        assert_eq!(&c.bytes()[4..10], &[0x80, 0xE8, 0xEB, 0x82, 0x00, 0x60]);
        // TDO (ADBUS2) must stay an input on every profile.
        assert_eq!(CablePins::DIGILENT.low_dir & pin::TDO, 0);
    }

    #[test]
    fn long_byte_runs_split_at_65536() {
        let data = vec![0x3Cu8; MAX_BYTES_PER_CMD + 1];
        let mut c = MpsseCmd::new();
        c.data_bytes(&data, true);
        let b = c.bytes();
        assert_eq!(&b[..3], &[0x39, 0xFF, 0xFF]);
        let second = 3 + MAX_BYTES_PER_CMD;
        assert_eq!(&b[second..second + 4], &[0x39, 0x00, 0x00, 0x3C]);
        assert_eq!(b.len(), 3 + MAX_BYTES_PER_CMD + 3 + 1);
        assert_eq!(c.read_len(), MAX_BYTES_PER_CMD + 1);
    }

    #[test]
    fn bit_reads_are_left_aligned() {
        let mut c = MpsseCmd::new();
        c.data_bytes(&[0, 0], true)
            .data_bits(0, 3, true)
            .tms(0b011, 3, false, true);
        // bytes LE 0x34 0x12; 3 bits 1,0,1 at bits 5..7 = 0b101_00000; TMS first bit 1 at bit5.
        let rx = [0x34, 0x12, 0b1010_0000, 0b0010_0000];
        let segs = c.decode(&rx).unwrap();
        assert_eq!(pack_bits(&segs[0]), vec![0x34, 0x12]);
        assert_eq!(segs[1], vec![true, false, true]);
        assert_eq!(segs[2], vec![true, false, false]);
        assert!(c.decode(&rx[..3]).is_err(), "short read refused");
    }

    #[test]
    fn tck_math() {
        assert_eq!(tck_hz(0, false), 30_000_000);
        assert_eq!(tck_hz(1, false), 15_000_000);
        assert_eq!(tck_hz(0x05DB, true), 4_000);
        assert_eq!(divisor_for_hz(30_000_000), 0);
        assert_eq!(divisor_for_hz(15_000_000), 1);
        assert_eq!(divisor_for_hz(6_000_000), 4);
        // Never faster than asked.
        assert_eq!(divisor_for_hz(7_000_000), 4);
        assert!(tck_hz(divisor_for_hz(7_000_000), false) <= 7_000_000);
        assert_eq!(divisor_for_hz(1), u16::MAX);
        assert_eq!(divisor_for_hz(100_000_000), 0);
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let bits = unpack_bits(&[0xA5, 0x03], 10);
        assert_eq!(pack_bits(&bits), vec![0xA5, 0x03]);
    }
}
