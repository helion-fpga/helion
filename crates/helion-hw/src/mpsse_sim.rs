//! In-process FTDI-class **bitbang** harness for Helion TAP (no USB hardware).
//!
//! Small step beyond [`crate::native_usb`] enumerate-only:
//! - [`FtdiBitbangSim::open`] — sim "open FTDI" (always succeeds; no libusb)
//! - [`FtdiBitbangSim::shift_ir`] / [`FtdiBitbangSim::shift_dr_u32`] — one JTAG
//!   shift via [`crate::Tap::tick`] (TMS/TDI bitbang)
//! - [`FtdiBitbangSim::read_idcode`] — IR_IDCODE + 32-bit DR scan
//!
//! This is **not** native MPSSE over real USB and must never be reported as
//! hardware program DONE. [`crate::NativeFtdiStub`] remains `NotImplemented`
//! for physical CFG_W / STAT over USB (OFL or sim cable still own program).

use helion_device::Device;

use crate::{Tap, TapState, IR_IDCODE, IR_STAT};

/// Error from the sim bitbang harness (never pretends to be USB I/O failure).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MpsseSimError {
    /// [`FtdiBitbangSim::open`] not called yet.
    NotOpen(&'static str),
}

impl std::fmt::Display for MpsseSimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MpsseSimError::NotOpen(msg) => write!(f, "mpsse-sim not open: {msg}"),
        }
    }
}

impl std::error::Error for MpsseSimError {}

/// Simulated FTDI channel that bitbangs Helion TAP over [`Tap::tick`].
///
/// No rusb, no MPSSE opcodes, no hardware — unit-test / bring-up harness only.
#[derive(Clone, Debug)]
pub struct FtdiBitbangSim {
    open: bool,
    tap: Tap,
}

impl FtdiBitbangSim {
    pub fn new(dev: &Device) -> Self {
        Self {
            open: false,
            tap: Tap::new(dev),
        }
    }

    /// Sim open of an FTDI-class bitbang channel. No USB; always Ok.
    pub fn open(&mut self) -> Result<(), MpsseSimError> {
        self.open = true;
        // 5× TMS=1 → Test-Logic-Reset (IEEE 1149.1).
        self.jtag_reset();
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn tap(&self) -> &Tap {
        &self.tap
    }

    fn require_open(&self) -> Result<(), MpsseSimError> {
        if self.open {
            Ok(())
        } else {
            Err(MpsseSimError::NotOpen(
                "call FtdiBitbangSim::open before JTAG shifts (sim FTDI)",
            ))
        }
    }

    /// Force Test-Logic-Reset via TMS bitbang (5 clocks TMS=1).
    pub fn jtag_reset(&mut self) {
        for _ in 0..5 {
            self.tap.tick(true, false);
        }
        debug_assert_eq!(self.tap.state, TapState::TestLogicReset);
    }

    /// Run-Test/Idle from TLR (TMS=0).
    fn to_idle(&mut self) {
        if self.tap.state == TapState::TestLogicReset {
            self.tap.tick(false, false);
        }
        // From Update-* paths we already land in Idle with TMS=0.
        while self.tap.state != TapState::RunTestIdle
            && self.tap.state != TapState::TestLogicReset
        {
            // Best-effort: TMS=0 usually drains toward Idle from Update.
            self.tap.tick(false, false);
            if self.tap.state == TapState::SelectDr {
                // Idle <- Update left us in SelectDr if TMS was 1; go TLR then Idle.
                self.jtag_reset();
                self.tap.tick(false, false);
                break;
            }
        }
        if self.tap.state == TapState::TestLogicReset {
            self.tap.tick(false, false);
        }
    }

    /// Navigate Idle → Shift-IR (TMS: 1,1,0,0 from Idle).
    fn enter_shift_ir(&mut self) {
        self.to_idle();
        self.tap.tick(true, false); // Select-DR
        self.tap.tick(true, false); // Select-IR
        self.tap.tick(false, false); // Capture-IR
        self.tap.tick(false, false); // Shift-IR
        debug_assert_eq!(self.tap.state, TapState::ShiftIr);
    }

    /// Navigate Idle → Shift-DR (TMS: 1,0,0 from Idle).
    fn enter_shift_dr(&mut self) {
        self.to_idle();
        self.tap.tick(true, false); // Select-DR
        self.tap.tick(false, false); // Capture-DR
        self.tap.tick(false, false); // Shift-DR
        debug_assert_eq!(self.tap.state, TapState::ShiftDr);
    }

    /// Exit Shift-* → Update → Idle (last data bit already presented with TMS=1).
    fn exit_update_idle(&mut self) {
        // Currently in Exit1-*; TMS=1 → Update-*; TMS=0 → Idle
        self.tap.tick(true, false); // Update
        self.tap.tick(false, false); // Idle
        debug_assert_eq!(self.tap.state, TapState::RunTestIdle);
    }

    /// Bitbang one IR shift (LSB first), return bits scanned out.
    pub fn shift_ir(&mut self, val: u8) -> Result<u8, MpsseSimError> {
        self.require_open()?;
        self.enter_shift_ir();
        let n = Tap::IR_LEN as usize;
        let mut scanned = 0u8;
        for i in 0..n {
            let tdi = ((val >> i) & 1) != 0;
            let last = i + 1 == n;
            let tdo = self.tap.tick(last, tdi); // last bit: TMS=1 → Exit1-IR
            if tdo {
                scanned |= 1 << i;
            }
        }
        debug_assert_eq!(self.tap.state, TapState::Exit1Ir);
        self.exit_update_idle();
        Ok(scanned)
    }

    /// Bitbang one 32-bit DR shift (LSB first), return scanned-out word.
    pub fn shift_dr_u32(&mut self, val: u32) -> Result<u32, MpsseSimError> {
        self.require_open()?;
        self.enter_shift_dr();
        let mut scanned = 0u32;
        for i in 0..32 {
            let tdi = ((val >> i) & 1) != 0;
            let last = i == 31;
            let tdo = self.tap.tick(last, tdi);
            if tdo {
                scanned |= 1 << i;
            }
        }
        debug_assert_eq!(self.tap.state, TapState::Exit1Dr);
        self.exit_update_idle();
        Ok(scanned)
    }

    /// Open-path helper: shift `IR_IDCODE`, then scan 32-bit DR.
    pub fn read_idcode(&mut self) -> Result<u32, MpsseSimError> {
        let _ = self.shift_ir(IR_IDCODE)?;
        self.shift_dr_u32(0)
    }

    /// Shift `IR_STAT`, then scan 32-bit STAT word (sim fabric).
    pub fn read_stat_word(&mut self) -> Result<u32, MpsseSimError> {
        let _ = self.shift_ir(IR_STAT)?;
        self.shift_dr_u32(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_fabric::Stat;

    #[test]
    fn open_ftdi_bitbang_one_ir_shift_idcode_without_hardware() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut bb = FtdiBitbangSim::new(&dev);
        assert!(!bb.is_open());
        assert!(matches!(
            bb.shift_ir(IR_IDCODE),
            Err(MpsseSimError::NotOpen(_))
        ));

        bb.open().unwrap();
        assert!(bb.is_open());
        assert_eq!(bb.tap().state, TapState::TestLogicReset);

        // One IR shift into IDCODE, then DR scan — bitbang path, no USB.
        let id = bb.read_idcode().unwrap();
        assert_eq!(id, 0x0001_1A1F);
        assert_eq!(id, dev.idcode);
        assert_eq!(bb.tap().ir, IR_IDCODE);
        assert_eq!(bb.tap().state, TapState::RunTestIdle);

        // STAT via bitbang (reset fabric → RESET_WORD).
        let st = bb.read_stat_word().unwrap();
        assert_eq!(st, Stat::RESET_WORD);
        assert_eq!(bb.tap().ir, IR_STAT);
    }

    #[test]
    fn bitbang_ir_roundtrip_scans_previous_opcode() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut bb = FtdiBitbangSim::new(&dev);
        bb.open().unwrap();
        // After open/reset, IR defaults to IDCODE — first shift scans that out.
        let scanned = bb.shift_ir(IR_STAT).unwrap();
        assert_eq!(scanned, IR_IDCODE);
        assert_eq!(bb.tap().ir, IR_STAT);
        let scanned2 = bb.shift_ir(IR_IDCODE).unwrap();
        assert_eq!(scanned2, IR_STAT);
        assert_eq!(bb.tap().ir, IR_IDCODE);
    }

    #[test]
    fn native_stub_still_not_implemented_for_real_usb() {
        // Honesty: sim harness ≠ native USB MPSSE DONE.
        let err = crate::try_native_usb_program(std::path::Path::new("/dev/null"), false)
            .unwrap_err();
        assert!(matches!(err, crate::NativeUsbError::NotImplemented(_)));
    }
}
