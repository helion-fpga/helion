//! One TAP-level interface for every cable the debug and flash flows use.
//!
//! [`JtagCable`] is implemented by the in-process [`Tap`] (the Simulation
//! cable) and by [`JtagEngine`] (native FTDI MPSSE — the real USB cable or
//! the [`crate::MockFtdi`] board model). Flows written against it run the
//! same IR/DR traffic on either; the engine version moves every bit through
//! the MPSSE byte stream and decodes TDO from the returned bytes.
//!
//! [`JtagCable::board`] is the simulated board behind a cable (the sim TAP
//! itself, or the TAP the mock FTDI clocks). It is how simulation stands in
//! for the board's free-running user clock and on-chip debug logic; a real
//! USB cable has none, and flows that need it say so instead of guessing.

use helion_bits::Bitstream;
use helion_fabric::Stat;

use crate::jtag_engine::{JtagEngine, MpssePort, stat_from_word};
use crate::{IR_USR1, Tap, TapState, Usr1RleUpload};

/// Which kind of cable a flow ran on — shown on every result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CableKind {
    /// In-process Helion TAP (high-level shifts).
    #[default]
    Sim,
    /// Native JTAG engine → MPSSE bytes → in-process mock FTDI → TAP ticks.
    NativeMock,
    /// Native JTAG engine → MPSSE bytes → real FTDI over USB.
    NativeUsb,
}

impl CableKind {
    pub fn label(self) -> &'static str {
        match self {
            CableKind::Sim => "Simulation cable",
            CableKind::NativeMock => "native FTDI · mock device",
            CableKind::NativeUsb => {
                "native FTDI · USB (chain scan verified on hardware; programming unverified)"
            }
        }
    }

    pub fn is_native(self) -> bool {
        !matches!(self, CableKind::Sim)
    }
}

/// TAP-level operations on the target device.
pub trait JtagCable: Send {
    fn kind(&self) -> CableKind;
    fn describe(&self) -> String;
    /// Load an instruction into the target.
    fn shift_ir(&mut self, ir: u8) -> Result<(), String>;
    /// One 64-bit DR scan (USER chains); returns the captured word.
    fn shift_dr_u64(&mut self, tdi: u64) -> Result<u64, String>;
    /// One DR scan of `nbits` (LSB first); returns the captured bits.
    fn shift_dr_bits(&mut self, tdi: &[u8], nbits: usize) -> Result<Vec<u8>, String>;
    fn read_idcode(&mut self) -> Result<u32, String>;
    fn read_stat(&mut self) -> Result<Stat, String>;
    /// CFG_W + STAT; errors unless DONE reads back high.
    fn program(&mut self, bits: &Bitstream) -> Result<Stat, String>;
    /// The simulated board behind this cable (`None` on a real cable).
    fn board(&mut self) -> Option<&mut Tap>;
    /// Cable traffic so far: (IR scans, DR scans).
    fn scans(&self) -> (usize, usize);
}

impl JtagCable for Tap {
    fn kind(&self) -> CableKind {
        CableKind::Sim
    }

    fn describe(&self) -> String {
        "Simulation cable · in-process Helion TAP".into()
    }

    fn shift_ir(&mut self, ir: u8) -> Result<(), String> {
        Tap::shift_ir(self, ir);
        Ok(())
    }

    fn shift_dr_u64(&mut self, tdi: u64) -> Result<u64, String> {
        Ok(Tap::shift_dr_u64(self, tdi))
    }

    fn shift_dr_bits(&mut self, tdi: &[u8], nbits: usize) -> Result<Vec<u8>, String> {
        if nbits == 0 {
            return Err("zero-length DR scan".into());
        }
        // Bit-accurate scan through `tick` from Run-Test/Idle.
        if self.state == TapState::TestLogicReset {
            self.tick(false, false);
        }
        if self.state != TapState::RunTestIdle {
            for _ in 0..5 {
                self.tick(true, false);
            }
            self.tick(false, false);
        }
        self.tick(true, false); // Select-DR
        self.tick(false, false); // Capture-DR
        self.tick(false, false); // Shift-DR
        let mut out = vec![0u8; nbits.div_ceil(8)];
        for i in 0..nbits {
            let b = tdi.get(i / 8).is_some_and(|v| (v >> (i % 8)) & 1 == 1);
            if self.tick(i + 1 == nbits, b) {
                out[i / 8] |= 1 << (i % 8);
            }
        }
        self.tick(true, false); // Update-DR
        self.tick(false, false); // Idle
        Ok(out)
    }

    fn read_idcode(&mut self) -> Result<u32, String> {
        Ok(Tap::read_idcode(self))
    }

    fn read_stat(&mut self) -> Result<Stat, String> {
        Ok(Tap::read_stat(self))
    }

    fn program(&mut self, bits: &Bitstream) -> Result<Stat, String> {
        let st = Tap::program(self, bits)?;
        if !st.done {
            return Err("program: DONE stayed low after CFG_W".into());
        }
        Ok(st)
    }

    fn board(&mut self) -> Option<&mut Tap> {
        Some(self)
    }

    fn scans(&self) -> (usize, usize) {
        (0, 0)
    }
}

impl<P: MpssePort> JtagCable for JtagEngine<P> {
    fn kind(&self) -> CableKind {
        if self.port().is_mock() {
            CableKind::NativeMock
        } else {
            CableKind::NativeUsb
        }
    }

    fn describe(&self) -> String {
        let d = &self.chain()[self.target()];
        format!(
            "{} · {} · TCK {:.2} MHz · device {}/{}",
            self.port().describe(),
            d.name,
            f64::from(self.tck_hz()) / 1e6,
            self.target() + 1,
            self.chain().len()
        )
    }

    fn shift_ir(&mut self, ir: u8) -> Result<(), String> {
        JtagEngine::shift_ir(self, u64::from(ir)).map(|_| ())
    }

    fn shift_dr_u64(&mut self, tdi: u64) -> Result<u64, String> {
        JtagEngine::shift_dr_u64(self, tdi)
    }

    fn shift_dr_bits(&mut self, tdi: &[u8], nbits: usize) -> Result<Vec<u8>, String> {
        JtagEngine::shift_dr(self, tdi, nbits, true)
    }

    fn read_idcode(&mut self) -> Result<u32, String> {
        JtagEngine::read_idcode(self)
    }

    fn read_stat(&mut self) -> Result<Stat, String> {
        Ok(stat_from_word(self.read_stat_word()?))
    }

    fn program(&mut self, bits: &Bitstream) -> Result<Stat, String> {
        JtagEngine::program(self, bits)
    }

    fn board(&mut self) -> Option<&mut Tap> {
        self.port_mut().board()
    }

    fn scans(&self) -> (usize, usize) {
        (self.stats.ir_scans, self.stats.dr_scans)
    }
}

/// `IR_USR1` RLE capture-RAM upload over any cable (decoded from TDO).
pub fn usr1_upload_rle_via(
    cable: &mut dyn JtagCable,
    major: u16,
    start: u32,
    n: usize,
    wrap: u32,
) -> Result<Usr1RleUpload, String> {
    let wrap = if wrap == 0 { n as u32 } else { wrap };
    let start = start % wrap.max(1);
    cable.shift_ir(IR_USR1)?;
    cable.shift_dr_u64(Tap::usr1_rle_cmd(major, start, wrap, n))?;
    let header = cable.shift_dr_u64(0)?;
    let magic = ((header >> 48) & 0xffff) as u16;
    let n_samples = ((header >> 32) & 0xffff) as usize;
    let n_tokens = ((header >> 16) & 0xffff) as usize;
    if magic != crate::USR1_RLE_HEADER_MAGIC {
        return Err(format!(
            "USR1 RLE header magic {magic:#06x}, expected {:#06x} (TDO readback)",
            crate::USR1_RLE_HEADER_MAGIC
        ));
    }
    let mut tokens = Vec::with_capacity(n_tokens);
    for _ in 0..n_tokens {
        tokens.push(cable.shift_dr_u64(0)?);
    }
    Ok(Usr1RleUpload {
        words: crate::rle_expand_tokens(&tokens, n_samples),
        dr_scans: 1 + n_tokens,
        raw_scans: n,
        n_tokens,
        n_samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JtagEngine, MockFtdi};
    use helion_device::Device;

    fn dev() -> Device {
        Device::load_part("HL10T-C32-1").unwrap()
    }

    #[test]
    fn rle_upload_matches_between_sim_and_native() {
        let d = dev();
        let fill = |t: &mut Tap| {
            for i in 0..12usize {
                t.fabric_mut()
                    .bram_write_word(0, i, if i < 8 { 0 } else { 0x1_0000_0000_0001 });
            }
        };
        let mut sim = Tap::new(&d);
        fill(&mut sim);
        let a = usr1_upload_rle_via(&mut sim, 0, 0, 12, 12).unwrap();
        let mut native = JtagEngine::open(MockFtdi::helion(&d), 6_000_000).unwrap();
        fill(JtagCable::board(&mut native).unwrap());
        let b = usr1_upload_rle_via(&mut native, 0, 0, 12, 12).unwrap();
        assert_eq!(a, b);
        assert_eq!(b.words[11], 0x1_0000_0000_0001);
        assert_eq!(native.kind(), CableKind::NativeMock);
        assert!(native.scans().1 >= 1 + b.dr_scans);
    }

    #[test]
    fn sim_bit_scan_agrees_with_high_level_idcode() {
        let d = dev();
        let mut t = Tap::new(&d);
        JtagCable::shift_ir(&mut t, crate::IR_IDCODE).unwrap();
        let r = t.shift_dr_bits(&[0; 4], 32).unwrap();
        assert_eq!(u32::from_le_bytes([r[0], r[1], r[2], r[3]]), d.idcode);
    }
}
