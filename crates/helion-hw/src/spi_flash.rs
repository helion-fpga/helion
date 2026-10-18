//! JTAG-to-SPI Indirect Flash Programmer & JEDEC SPI Flash Simulator (`SpiFlashSim`).
//!
//! Emulates and drives standard JEDEC SPI / QSPI configuration flash commands:
//! - `RDID (0x9F)`: Read 3-byte JEDEC Manufacturer / Device ID (`[0x20, 0xBA, 0x18]`)
//! - `WREN (0x06)`: Write Enable (`WEL = 1` in status register)
//! - `WRDI (0x04)`: Write Disable (`WEL = 0` in status register)
//! - `RDSR (0x05)`: Read Status Register (`WIP` bit 0, `WEL` bit 1, `BP` bits)
//! - `SE   (0xD8)`: 64 KiB Sector Erase (`0xFF` fill; requires `WEL = 1`)
//! - `BE   (0xC7)`: Bulk / Chip Erase (`0xFF` fill; requires `WEL = 1`)
//! - `PP   (0x02)`: 256-Byte Page Program (NOR flash bitwise AND `mem[a] &= d`; requires `WEL = 1`)
//! - `READ (0x03)`: Standard SPI Data Read (24-bit address)
//! - `QREAD(0x6B)`: Quad-Output Fast Read (24-bit address + 1 dummy byte)
//!
//! Tunnelled through IEEE 1149.1 `IR_USR1` / `BSCAN` JTAG DR scans on [`Tap`], with full
//! readback CRC-32 and SHA-256 verification.

use crate::bitstream_crypto::sha256_digest;
use crate::cfgmem::crc32_ieee;
use crate::{Bitstream, IR_USR1, Stat, Tap};

pub const SPI_CMD_RDID: u8 = 0x9F;
pub const SPI_CMD_WREN: u8 = 0x06;
pub const SPI_CMD_WRDI: u8 = 0x04;
pub const SPI_CMD_RDSR: u8 = 0x05;
pub const SPI_CMD_SE: u8 = 0xD8;
pub const SPI_CMD_BE: u8 = 0xC7;
pub const SPI_CMD_PP: u8 = 0x02;
pub const SPI_CMD_READ: u8 = 0x03;
pub const SPI_CMD_QREAD: u8 = 0x6B;

pub const SPI_SECTOR_SIZE: usize = 65_536;
pub const SPI_PAGE_SIZE: usize = 256;
pub const SPI_STATUS_WIP: u8 = 0x01;
pub const SPI_STATUS_WEL: u8 = 0x02;
pub const DEFAULT_JEDEC_ID: [u8; 3] = [0x20, 0xBA, 0x18]; // Micron N25Q128A 128Mb QSPI

/// Cycle- and state-accurate JEDEC NOR SPI Flash simulator.
#[derive(Clone, Debug)]
pub struct SpiFlashSim {
    pub jedec_id: [u8; 3],
    pub status_reg: u8,
    pub mem: Vec<u8>,
    pub sectors_erased: usize,
    pub pages_programmed: usize,
    pub spi_transactions: usize,
}

impl SpiFlashSim {
    pub fn new(capacity_bytes: usize) -> Self {
        let cap = capacity_bytes.max(SPI_SECTOR_SIZE);
        Self {
            jedec_id: DEFAULT_JEDEC_ID,
            status_reg: 0x00,
            mem: vec![0xFFu8; cap],
            sectors_erased: 0,
            pages_programmed: 0,
            spi_transactions: 0,
        }
    }

    pub fn write_enabled(&self) -> bool {
        (self.status_reg & SPI_STATUS_WEL) != 0
    }

    /// Execute one complete `CS#` low -> `tx_bytes` -> `CS#` high SPI transaction
    /// and return the full-duplex `rx_bytes` shifted out on MISO / DQ[3:0].
    pub fn transfer(&mut self, tx: &[u8]) -> Result<Vec<u8>, String> {
        if tx.is_empty() {
            return Ok(Vec::new());
        }
        self.spi_transactions = self.spi_transactions.saturating_add(1);
        let opcode = tx[0];
        let mut rx = vec![0x00u8; tx.len()];

        match opcode {
            SPI_CMD_RDID => {
                for (i, &id_b) in self.jedec_id.iter().enumerate() {
                    if i + 1 < rx.len() {
                        rx[i + 1] = id_b;
                    }
                }
                Ok(rx)
            }
            SPI_CMD_WREN => {
                self.status_reg |= SPI_STATUS_WEL;
                Ok(rx)
            }
            SPI_CMD_WRDI => {
                self.status_reg &= !SPI_STATUS_WEL;
                Ok(rx)
            }
            SPI_CMD_RDSR => {
                for b in rx.iter_mut().skip(1) {
                    *b = self.status_reg;
                }
                Ok(rx)
            }
            SPI_CMD_SE => {
                if !self.write_enabled() {
                    return Err("spi_flash SE (0xD8): Write Enable Latch (WEL) not set".into());
                }
                if tx.len() < 4 {
                    return Err("spi_flash SE (0xD8): requires 24-bit sector address".into());
                }
                let addr = ((tx[1] as usize) << 16) | ((tx[2] as usize) << 8) | (tx[3] as usize);
                let sector_base = (addr / SPI_SECTOR_SIZE) * SPI_SECTOR_SIZE;
                if sector_base + SPI_SECTOR_SIZE > self.mem.len() {
                    return Err(format!(
                        "spi_flash SE: sector 0x{sector_base:06x} out of bounds (capacity 0x{:06x})",
                        self.mem.len()
                    ));
                }
                self.mem[sector_base..sector_base + SPI_SECTOR_SIZE].fill(0xFF);
                self.status_reg &= !(SPI_STATUS_WEL | SPI_STATUS_WIP);
                self.sectors_erased = self.sectors_erased.saturating_add(1);
                Ok(rx)
            }
            SPI_CMD_BE => {
                if !self.write_enabled() {
                    return Err("spi_flash BE (0xC7): Write Enable Latch (WEL) not set".into());
                }
                self.mem.fill(0xFF);
                self.status_reg &= !(SPI_STATUS_WEL | SPI_STATUS_WIP);
                self.sectors_erased = self
                    .sectors_erased
                    .saturating_add(self.mem.len() / SPI_SECTOR_SIZE);
                Ok(rx)
            }
            SPI_CMD_PP => {
                if !self.write_enabled() {
                    return Err("spi_flash PP (0x02): Write Enable Latch (WEL) not set".into());
                }
                if tx.len() < 4 {
                    return Err("spi_flash PP (0x02): requires 24-bit address".into());
                }
                let addr = ((tx[1] as usize) << 16) | ((tx[2] as usize) << 8) | (tx[3] as usize);
                let payload = &tx[4..];
                if payload.len() > SPI_PAGE_SIZE {
                    return Err(format!(
                        "spi_flash PP (0x02): payload {} exceeds 256-byte page size",
                        payload.len()
                    ));
                }
                let page_base = (addr / SPI_PAGE_SIZE) * SPI_PAGE_SIZE;
                let page_off = addr % SPI_PAGE_SIZE;
                if page_base + SPI_PAGE_SIZE > self.mem.len() {
                    return Err(format!(
                        "spi_flash PP: address 0x{addr:06x} out of bounds (capacity 0x{:06x})",
                        self.mem.len()
                    ));
                }
                for (i, &b) in payload.iter().enumerate() {
                    // Authentic NOR flash page wrap & bitwise AND (`1 -> 0` only without erase)
                    let dst = page_base + ((page_off + i) % SPI_PAGE_SIZE);
                    self.mem[dst] &= b;
                }
                self.status_reg &= !(SPI_STATUS_WEL | SPI_STATUS_WIP);
                self.pages_programmed = self.pages_programmed.saturating_add(1);
                Ok(rx)
            }
            SPI_CMD_READ => {
                if tx.len() < 4 {
                    return Err("spi_flash READ (0x03): requires 24-bit address".into());
                }
                let addr = ((tx[1] as usize) << 16) | ((tx[2] as usize) << 8) | (tx[3] as usize);
                let read_len = tx.len() - 4;
                if addr + read_len > self.mem.len() {
                    return Err("spi_flash READ (0x03): read range exceeds flash capacity".into());
                }
                rx[4..].copy_from_slice(&self.mem[addr..addr + read_len]);
                Ok(rx)
            }
            SPI_CMD_QREAD => {
                // 0x6B + 24-bit addr + 1 dummy byte (5 header bytes total)
                if tx.len() < 5 {
                    return Err(
                        "spi_flash QREAD (0x6B): requires 24-bit address + 1 dummy byte".into(),
                    );
                }
                let addr = ((tx[1] as usize) << 16) | ((tx[2] as usize) << 8) | (tx[3] as usize);
                let read_len = tx.len() - 5;
                if addr + read_len > self.mem.len() {
                    return Err("spi_flash QREAD (0x6B): read range exceeds flash capacity".into());
                }
                rx[5..].copy_from_slice(&self.mem[addr..addr + read_len]);
                Ok(rx)
            }
            other => Err(format!("spi_flash: unsupported JEDEC opcode 0x{other:02X}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashVerifyReport {
    pub jedec_id: [u8; 3],
    pub start_addr: u32,
    pub bytes_programmed: usize,
    pub sectors_erased: usize,
    pub pages_programmed: usize,
    pub jtag_dr_scans: usize,
    pub expected_crc32: u32,
    pub readback_crc32: u32,
    pub expected_sha256: [u8; 32],
    pub readback_sha256: [u8; 32],
    pub verified: bool,
}

/// JTAG-to-SPI Indirect Flash Programmer Bridge (`USER1` / `BSCAN_SPARTAN7` / `BSCAN_ARTIX7`).
/// Tunnels SPI transactions through IEEE 1149.1 `IR_USR1` 64-bit DR shifts on [`Tap`].
pub struct JtagSpiBridge<'a> {
    pub tap: &'a mut Tap,
    pub flash: SpiFlashSim,
    pub jtag_dr_scans: usize,
}

impl<'a> JtagSpiBridge<'a> {
    pub fn new(tap: &'a mut Tap, flash: SpiFlashSim) -> Self {
        Self {
            tap,
            flash,
            jtag_dr_scans: 0,
        }
    }

    /// Tunnel a raw SPI transaction through JTAG `IR_USR1` DR scans (8 bytes per 64-bit DR scan).
    pub fn spi_transfer(&mut self, tx: &[u8]) -> Result<Vec<u8>, String> {
        if self.tap.ir != IR_USR1 {
            self.tap.shift_ir(IR_USR1);
        }
        for chunk in tx.chunks(8) {
            let mut word_bytes = [0u8; 8];
            word_bytes[..chunk.len()].copy_from_slice(chunk);
            let tdi = u64::from_le_bytes(word_bytes);
            let _ = self.tap.shift_dr_u64(tdi);
            self.jtag_dr_scans = self.jtag_dr_scans.saturating_add(1);
        }
        self.flash.transfer(tx)
    }

    /// Read the 3-byte JEDEC Flash ID (`0x9F`) via JTAG-to-SPI bridge.
    pub fn read_jedec_id(&mut self) -> Result<[u8; 3], String> {
        let rx = self.spi_transfer(&[SPI_CMD_RDID, 0x00, 0x00, 0x00])?;
        Ok([rx[1], rx[2], rx[3]])
    }

    /// Issue `WREN (0x06)` Write Enable via JTAG-to-SPI bridge.
    pub fn write_enable(&mut self) -> Result<(), String> {
        let _ = self.spi_transfer(&[SPI_CMD_WREN])?;
        Ok(())
    }

    /// Read the 8-bit SPI Status Register (`RDSR 0x05`).
    pub fn read_status(&mut self) -> Result<u8, String> {
        let rx = self.spi_transfer(&[SPI_CMD_RDSR, 0x00])?;
        Ok(rx[1])
    }

    /// Erase one 64 KiB sector containing `addr` (`WREN 0x06` + `SE 0xD8`).
    pub fn erase_sector(&mut self, addr: u32) -> Result<(), String> {
        self.write_enable()?;
        let cmd = [
            SPI_CMD_SE,
            ((addr >> 16) & 0xFF) as u8,
            ((addr >> 8) & 0xFF) as u8,
            (addr & 0xFF) as u8,
        ];
        let _ = self.spi_transfer(&cmd)?;
        Ok(())
    }

    /// Program up to 256 bytes into a single flash page (`WREN 0x06` + `PP 0x02`).
    pub fn program_page(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        if data.len() > SPI_PAGE_SIZE {
            return Err("program_page: data exceeds 256-byte page size".into());
        }
        self.write_enable()?;
        let mut cmd = Vec::with_capacity(4 + data.len());
        cmd.push(SPI_CMD_PP);
        cmd.push(((addr >> 16) & 0xFF) as u8);
        cmd.push(((addr >> 8) & 0xFF) as u8);
        cmd.push((addr & 0xFF) as u8);
        cmd.extend_from_slice(data);
        let _ = self.spi_transfer(&cmd)?;
        Ok(())
    }

    /// Read `len` bytes starting at `addr` using either `READ (0x03)` or `QREAD (0x6B)`.
    pub fn read_data(&mut self, addr: u32, len: usize, quad_mode: bool) -> Result<Vec<u8>, String> {
        let hdr_len = if quad_mode { 5 } else { 4 };
        let mut cmd = vec![0u8; hdr_len + len];
        cmd[0] = if quad_mode {
            SPI_CMD_QREAD
        } else {
            SPI_CMD_READ
        };
        cmd[1] = ((addr >> 16) & 0xFF) as u8;
        cmd[2] = ((addr >> 8) & 0xFF) as u8;
        cmd[3] = (addr & 0xFF) as u8;
        let rx = self.spi_transfer(&cmd)?;
        Ok(rx[hdr_len..].to_vec())
    }

    /// Full Vivado Hardware Manager indirect flash flow:
    /// 1. Query and verify JEDEC ID (`RDID 0x9F`).
    /// 2. Erase all 64 KiB sectors spanned by `[start_addr .. start_addr + image.len()]`.
    /// 3. Program all 256-byte pages (`WREN 0x06` + `PP 0x02`).
    /// 4. Read back (`READ 0x03` or `QREAD 0x6B`) and verify both CRC-32 and SHA-256 digests.
    pub fn erase_program_verify(
        &mut self,
        start_addr: u32,
        image: &[u8],
        quad_read: bool,
    ) -> Result<FlashVerifyReport, String> {
        let jedec_id = self.read_jedec_id()?;
        if jedec_id == [0x00, 0x00, 0x00] || jedec_id == [0xFF, 0xFF, 0xFF] {
            return Err(format!("invalid SPI flash JEDEC ID: {jedec_id:02X?}"));
        }

        let start = start_addr as usize;
        let end = start + image.len();
        let first_sector = (start / SPI_SECTOR_SIZE) * SPI_SECTOR_SIZE;
        let last_sector = ((end.saturating_sub(1)) / SPI_SECTOR_SIZE) * SPI_SECTOR_SIZE;

        let sectors_before = self.flash.sectors_erased;
        let pages_before = self.flash.pages_programmed;

        if !image.is_empty() {
            let mut sec = first_sector;
            while sec <= last_sector {
                self.erase_sector(sec as u32)?;
                sec += SPI_SECTOR_SIZE;
            }
        }

        // Program in 256-byte page-aligned chunks
        let mut cursor = start;
        while cursor < end {
            let page_rem = SPI_PAGE_SIZE - (cursor % SPI_PAGE_SIZE);
            let chunk_len = (end - cursor).min(page_rem);
            let img_off = cursor - start;
            self.program_page(cursor as u32, &image[img_off..img_off + chunk_len])?;
            cursor += chunk_len;
        }

        // Read back and verify CRC32 + SHA-256
        let readback = self.read_data(start_addr, image.len(), quad_read)?;
        let expected_crc32 = crc32_ieee(image);
        let readback_crc32 = crc32_ieee(&readback);
        let expected_sha256 = sha256_digest(image);
        let readback_sha256 = sha256_digest(&readback);
        let verified =
            expected_crc32 == readback_crc32 && expected_sha256 == readback_sha256 && readback == image;

        if !verified {
            return Err(format!(
                "SPI flash readback verification failed: expected CRC32=0x{expected_crc32:08X}, got 0x{readback_crc32:08X}"
            ));
        }

        Ok(FlashVerifyReport {
            jedec_id,
            start_addr,
            bytes_programmed: image.len(),
            sectors_erased: self.flash.sectors_erased - sectors_before,
            pages_programmed: self.flash.pages_programmed - pages_before,
            jtag_dr_scans: self.jtag_dr_scans,
            expected_crc32,
            readback_crc32,
            expected_sha256,
            readback_sha256,
            verified,
        })
    }

    /// Boot the underlying [`Tap`] FPGA fabric directly from a bitstream stored in SPI flash at `addr`..`addr + len`.
    pub fn boot_fabric_from_flash(&mut self, addr: u32, len: usize) -> Result<Stat, String> {
        let bytes = self.read_data(addr, len, true)?;
        let bits = Bitstream::from_packets(&bytes)?;
        self.tap.program(&bits)
    }
}
