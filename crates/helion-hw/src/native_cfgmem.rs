//! SPI configuration flash over JTAG for any [`JtagCable`].
//!
//! [`SpiPort`] is the JEDEC command layer (RDID / WREN / RDSR / SE / PP /
//! READ) shared by the Simulation cable's USER1 bridge
//! ([`crate::JtagSpiBridge`]) and [`SpiOverJtag`], which reaches the board's
//! configuration flash through the Helion TAP's [`IR_SPI_ACCESS`]
//! instruction: one Shift-DR clock is one SPI clock, CS# spans
//! Capture-DR..Update-DR, every MISO bit is decoded from TDO. On the native
//! cable that means real MPSSE traffic end to end.
//!
//! The native port is a single-bit SPI path, so reads use `READ (0x03)`
//! (never the quad opcode) and every write is followed by a status poll and
//! a `WEL` check read back over TDO — no step is assumed to have worked.

use helion_bits::Bitstream;
use helion_fabric::Stat;

use crate::IR_SPI_ACCESS;
use crate::jtag_cable::JtagCable;
use crate::spi_flash::{
    SPI_CMD_PP, SPI_CMD_RDID, SPI_CMD_RDSR, SPI_CMD_READ, SPI_CMD_SE, SPI_CMD_WREN, SPI_PAGE_SIZE,
    SPI_STATUS_WEL, SPI_STATUS_WIP,
};

/// Bytes per READ transaction (keeps one DR scan bounded).
pub const READ_CHUNK: usize = 16 * 1024;

/// JEDEC SPI NOR command layer over one SPI transaction primitive.
pub trait SpiPort {
    /// One CS#-low..CS#-high transaction; returns MISO bytes.
    fn spi_xfer(&mut self, tx: &[u8]) -> Result<Vec<u8>, String>;
    /// JTAG DR scans spent so far.
    fn jtag_scans(&self) -> usize;
    /// Single-bit port (no quad reads).
    fn single_bit(&self) -> bool {
        true
    }
    /// Configure the device from `len` flash bytes at `addr`.
    fn boot(&mut self, addr: u32, len: usize) -> Result<Stat, String>;

    fn jedec_id(&mut self) -> Result<[u8; 3], String> {
        let rx = self.spi_xfer(&[SPI_CMD_RDID, 0, 0, 0])?;
        let id = [rx[1], rx[2], rx[3]];
        if id == [0, 0, 0] || id == [0xFF, 0xFF, 0xFF] {
            return Err(format!(
                "no SPI flash answered RDID (read {:02X} {:02X} {:02X})",
                id[0], id[1], id[2]
            ));
        }
        Ok(id)
    }

    fn status(&mut self) -> Result<u8, String> {
        Ok(self.spi_xfer(&[SPI_CMD_RDSR, 0])?[1])
    }

    /// WREN, then confirm WEL reads back set.
    fn write_enable(&mut self) -> Result<(), String> {
        self.spi_xfer(&[SPI_CMD_WREN])?;
        let st = self.status()?;
        if st & SPI_STATUS_WEL == 0 {
            return Err(format!(
                "WREN did not set WEL (status {st:#04x}) — flash write-protected or not answering"
            ));
        }
        Ok(())
    }

    /// Poll RDSR until WIP clears.
    fn wait_ready(&mut self) -> Result<(), String> {
        for _ in 0..100_000 {
            if self.status()? & SPI_STATUS_WIP == 0 {
                return Ok(());
            }
        }
        Err("SPI flash stayed busy (WIP) — erase/program did not finish".into())
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), String> {
        self.write_enable()?;
        self.spi_xfer(&[
            SPI_CMD_SE,
            (addr >> 16) as u8,
            (addr >> 8) as u8,
            addr as u8,
        ])?;
        self.wait_ready()
    }

    fn program_page(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        if data.len() > SPI_PAGE_SIZE {
            return Err("program_page: more than one 256-byte page".into());
        }
        self.write_enable()?;
        let mut cmd = vec![
            SPI_CMD_PP,
            (addr >> 16) as u8,
            (addr >> 8) as u8,
            addr as u8,
        ];
        cmd.extend_from_slice(data);
        self.spi_xfer(&cmd)?;
        self.wait_ready()
    }

    fn read(&mut self, addr: u32, len: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(len);
        let mut cur = addr as usize;
        let end = addr as usize + len;
        while cur < end {
            let n = (end - cur).min(READ_CHUNK);
            let mut cmd = vec![0u8; 4 + n];
            cmd[0] = SPI_CMD_READ;
            cmd[1] = (cur >> 16) as u8;
            cmd[2] = (cur >> 8) as u8;
            cmd[3] = cur as u8;
            let rx = self.spi_xfer(&cmd)?;
            out.extend_from_slice(&rx[4..]);
            cur += n;
        }
        Ok(out)
    }
}

impl SpiPort for crate::JtagSpiBridge<'_> {
    fn spi_xfer(&mut self, tx: &[u8]) -> Result<Vec<u8>, String> {
        self.spi_transfer(tx)
    }

    fn jtag_scans(&self) -> usize {
        self.jtag_dr_scans
    }

    fn single_bit(&self) -> bool {
        false
    }

    fn boot(&mut self, addr: u32, len: usize) -> Result<Stat, String> {
        self.boot_fabric_from_flash(addr, len)
    }
}

/// Flash access through [`IR_SPI_ACCESS`] on any cable.
pub struct SpiOverJtag<'a> {
    pub cable: &'a mut dyn JtagCable,
    ir_loaded: bool,
    pub scans: usize,
}

impl<'a> SpiOverJtag<'a> {
    pub fn new(cable: &'a mut dyn JtagCable) -> Self {
        Self {
            cable,
            ir_loaded: false,
            scans: 0,
        }
    }
}

impl SpiPort for SpiOverJtag<'_> {
    fn spi_xfer(&mut self, tx: &[u8]) -> Result<Vec<u8>, String> {
        if tx.is_empty() {
            return Ok(Vec::new());
        }
        if !self.ir_loaded {
            self.cable.shift_ir(IR_SPI_ACCESS)?;
            self.ir_loaded = true;
        }
        // SPI is MSB first, JTAG shifts LSB first.
        let rev: Vec<u8> = tx.iter().map(|b| b.reverse_bits()).collect();
        let rx = self.cable.shift_dr_bits(&rev, tx.len() * 8)?;
        self.scans += 1;
        Ok(rx.iter().map(|b| b.reverse_bits()).collect())
    }

    fn jtag_scans(&self) -> usize {
        self.scans
    }

    /// Read the image back over TDO and configure the device with CFG_W.
    fn boot(&mut self, addr: u32, len: usize) -> Result<Stat, String> {
        let bytes = self.read(addr, len)?;
        let bits = Bitstream::from_packets(&bytes)
            .map_err(|e| format!("boot: flash image at {addr:#x} is not a bitstream: {e}"))?;
        self.ir_loaded = false;
        self.cable.program(&bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spi_flash::SpiFlashSim;
    use crate::{JtagEngine, MockFtdi, Tap};
    use helion_device::Device;

    fn native() -> JtagEngine<MockFtdi> {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let mut m = MockFtdi::helion(&d);
        m.helion_tap_mut()
            .unwrap()
            .attach_config_flash(SpiFlashSim::new(1 << 20));
        JtagEngine::open(m, 15_000_000).unwrap()
    }

    #[test]
    fn rdid_wren_erase_program_read_over_mpsse() {
        let mut e = native();
        let mut spi = SpiOverJtag::new(&mut e);
        assert_eq!(spi.jedec_id().unwrap(), crate::DEFAULT_JEDEC_ID);
        spi.erase_sector(0x10000).unwrap();
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        spi.program_page(0x10000, &data[..256]).unwrap();
        spi.program_page(0x10100, &data[256..]).unwrap();
        assert_eq!(spi.read(0x10000, 300).unwrap(), data);
        assert!(spi.scans > 6);
        // The bytes really landed in the board's flash model.
        let f = e.port().helion_tap().unwrap().config_flash().unwrap();
        assert_eq!(&f.mem[0x10000..0x10000 + 300], &data[..]);
        assert_eq!(f.sectors_erased, 1);
        assert_eq!(f.pages_programmed, 2);
    }

    #[test]
    fn no_flash_wired_is_reported_not_invented() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let mut e = JtagEngine::open(MockFtdi::helion(&d), 1_000_000).unwrap();
        let mut spi = SpiOverJtag::new(&mut e);
        let err = spi.jedec_id().unwrap_err();
        assert!(err.contains("no SPI flash"), "{err}");
        assert!(spi.write_enable().is_err());
    }

    #[test]
    fn sim_tap_bit_path_agrees() {
        let d = Device::load_part("HL10T-C32-1").unwrap();
        let mut t = Tap::new(&d);
        t.attach_config_flash(SpiFlashSim::new(1 << 20));
        let mut spi = SpiOverJtag::new(&mut t);
        assert_eq!(spi.jedec_id().unwrap(), crate::DEFAULT_JEDEC_ID);
        assert_eq!(spi.status().unwrap(), 0);
        spi.write_enable().unwrap();
        assert_eq!(spi.status().unwrap() & SPI_STATUS_WEL, SPI_STATUS_WEL);
    }
}
