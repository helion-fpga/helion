//! IEEE 1149.1 TAP + helion-prog + hw_server sim cable (no board).

use helion_bits::Bitstream;
use helion_device::Device;
use helion_fabric::{Fabric, Stat};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapState {
    TestLogicReset,
    RunTestIdle,
    SelectDr,
    CaptureDr,
    ShiftDr,
    Exit1Dr,
    UpdateDr,
    SelectIr,
    CaptureIr,
    ShiftIr,
    Exit1Ir,
    UpdateIr,
}

pub const IR_IDCODE: u8 = 0b000011;
pub const IR_STAT: u8 = 0b010010;
pub const IR_CFG_W: u8 = 0b010000;

#[derive(Clone, Debug)]
pub struct Tap {
    pub state: TapState,
    pub ir: u8,
    fabric: Fabric,
    ir_shift: u8,
    shlen: u8,
}

impl Tap {
    pub fn new(dev: &Device) -> Self {
        Self {
            state: TapState::TestLogicReset,
            ir: IR_IDCODE,
            fabric: Fabric::new(dev),
            ir_shift: 0,
            shlen: 0,
        }
    }

    /// 5× TMS=1 → Test-Logic-Reset.
    pub fn reset(&mut self) {
        self.state = TapState::TestLogicReset;
        self.ir = IR_IDCODE;
    }

    fn tms(&mut self, tms: bool) {
        use TapState::*;
        self.state = match (self.state, tms) {
            (TestLogicReset, false) => RunTestIdle,
            (TestLogicReset, true) => TestLogicReset,
            (RunTestIdle, true) => SelectDr,
            (RunTestIdle, false) => RunTestIdle,
            (SelectDr, true) => SelectIr,
            (SelectDr, false) => CaptureDr,
            (CaptureDr, false) => ShiftDr,
            (CaptureDr, true) => Exit1Dr,
            (ShiftDr, false) => ShiftDr,
            (ShiftDr, true) => Exit1Dr,
            (Exit1Dr, true) => UpdateDr,
            (Exit1Dr, false) => ShiftDr,
            (UpdateDr, false) => RunTestIdle,
            (UpdateDr, true) => SelectDr,
            (SelectIr, true) => TestLogicReset,
            (SelectIr, false) => CaptureIr,
            (CaptureIr, false) => ShiftIr,
            (CaptureIr, true) => Exit1Ir,
            (ShiftIr, false) => ShiftIr,
            (ShiftIr, true) => Exit1Ir,
            (Exit1Ir, true) => UpdateIr,
            (Exit1Ir, false) => ShiftIr,
            (UpdateIr, false) => RunTestIdle,
            (UpdateIr, true) => SelectDr,
        };
    }

    pub fn shift_ir(&mut self, val: u8) {
        self.reset();
        self.tms(false); // idle
        self.tms(true); // select-dr
        self.tms(true); // select-ir
        self.tms(false); // capture-ir
        self.tms(false); // shift-ir
        self.ir_shift = val;
        self.ir = val;
        self.tms(true); // exit1-ir
        self.tms(true); // update-ir
        self.tms(false); // idle
        let _ = self.shlen;
    }

    pub fn read_idcode(&mut self) -> u32 {
        self.shift_ir(IR_IDCODE);
        self.fabric.idcode
    }

    pub fn read_stat(&mut self) -> Stat {
        self.shift_ir(IR_STAT);
        self.fabric.stat.clone()
    }

    /// CFG_W: TAP IR then load frames into the fabric (sim cable).
    pub fn program(&mut self, bits: &Bitstream) -> Result<Stat, String> {
        self.shift_ir(IR_CFG_W);
        self.fabric.program(bits)?;
        self.fabric.finish_startup();
        Ok(self.read_stat())
    }
}

#[derive(Clone, Debug)]
pub struct SimCable {
    tap: Tap,
}

impl SimCable {
    pub fn open(dev: &Device) -> Self {
        Self { tap: Tap::new(dev) }
    }

    pub fn program(&mut self, bits: &Bitstream) -> Result<(), String> {
        self.tap.program(bits)?;
        Ok(())
    }

    pub fn stat(&self) -> Stat {
        self.tap.fabric.stat.clone()
    }
}

pub fn hw_server_program(dev: &Device, bits: &Bitstream) -> Result<Stat, String> {
    let mut c = SimCable::open(dev);
    c.program(bits)?;
    Ok(c.stat())
}

/// helion-prog API (sim cable).
pub fn prog_sim(dev: &Device, bits: &Bitstream) -> Result<Stat, String> {
    hw_server_program(dev, bits)
}

pub fn prog_empty(dev: &Device) -> Result<Stat, String> {
    prog_sim(dev, &Bitstream::empty(dev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;

    #[test]
    fn tap_idcode_and_cfg_w_stat() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut tap = Tap::new(&dev);
        assert_eq!(tap.read_idcode(), 0x0001_1A1F);
        let st = tap.program(&Bitstream::empty(&dev)).unwrap();
        assert!(st.init && st.done && st.eos && st.gwe);
        assert!(!st.gsr && !st.gts && !st.crc_err);
        assert_eq!(tap.ir, IR_STAT);
    }

    #[test]
    fn helion_prog_sim_empty() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let st = prog_empty(&dev).unwrap();
        assert!(st.done && st.gwe && !st.crc_err);
    }
}
