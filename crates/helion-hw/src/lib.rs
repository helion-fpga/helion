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

    pub fn program_partial(&mut self, bits: &Bitstream) -> Result<(), String> {
        self.tap.fabric.program_partial(bits)
    }

    pub fn fabric(&self) -> &Fabric {
        &self.tap.fabric
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

    #[test]
    fn dfx_partial_on_sim_cable() {
        use helion_bits::{bitgen, bitgen_pblock};
        use helion_ir::{CellKind, Design, PortDir};
        use helion_pack::pack;
        use helion_place::{place_with, PlaceOpts};
        use helion_route::route;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        fn nine(last: u64) -> Design {
            let mut d = Design::new("dfx");
            d.add_port("clk", PortDir::In);
            d.add_port("led", PortDir::Out);
            for i in 0..9u32 {
                let init = if i == 8 { last } else { 0x5555_5555_5555_5555 };
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
        let pa = pack(&nine(0x5555_5555_5555_5555), &dev).unwrap();
        let pb = pack(&nine(0xAAAA_AAAA_AAAA_AAAA), &dev).unwrap();
        let pla = place_with(&pa, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let plb = place_with(&pb, &dev, PlaceOpts { timing_weight: 0.75 }).unwrap();
        let ra = route(&pla, &dev).unwrap();
        let rb = route(&plb, &dev).unwrap();
        let full_a = bitgen(&dev, &ra).unwrap();
        let (rx, ry) = (pla.lutff_sites[8].0.x, pla.lutff_sites[8].0.y);
        let (sx, sy) = (pla.lutff_sites[0].0.x, pla.lutff_sites[0].0.y);
        let partial = bitgen_pblock(&dev, &rb, &[(rx, ry)]).unwrap();
        let mut cable = SimCable::open(&dev);
        cable.program(&full_a).unwrap();
        let st_maj = dev.clb_major(sx, sy).unwrap();
        let before = cable.fabric().frame_word(helion_device::Far::CLB_IO_CLK, st_maj, 0);
        cable.program_partial(&partial).unwrap();
        let after = cable.fabric().frame_word(helion_device::Far::CLB_IO_CLK, st_maj, 0);
        assert_eq!(before, after, "sim cable partial must not touch static frames");
        assert_eq!(
            cable.fabric().lut_init(rx, ry, 0),
            0xAAAA_AAAA_AAAA_AAAA
        );
        assert!(cable.stat().done);
    }
}
