//! Device-side ports on the Helion TAP model beyond IDCODE / STAT / CFG_W /
//! USER1: the USER2..USER4 user data registers and the configuration-flash
//! SPI access instruction ([`crate::IR_SPI_ACCESS`]).
//!
//! The SPI port is wired at board level ([`Tap::attach_config_flash`]): the
//! flash model sits on the device's configuration SPI pins. While the SPI
//! access IR is active, every Shift-DR clock is one SPI clock: TDI drives
//! MOSI and TDO returns the bit the flash drives on MISO for that clock. CS#
//! falls at Capture-DR and rises at Update-DR (or Test-Logic-Reset), which is
//! when the flash executes the transaction (program / erase side effects).
//! SPI is MSB-first; JTAG shifts LSB-first, so the host bit-reverses bytes.

use crate::spi_flash::SpiFlashSim;
use crate::{IR_USR2, IR_USR3, IR_USR4, Tap};

/// `true` for the USER2..USER4 instructions (64-bit user data registers).
pub fn is_user_chain(ir: u8) -> bool {
    matches!(ir, IR_USR2 | IR_USR3 | IR_USR4)
}

/// Index into the user register file for USER2..USER4.
pub(crate) fn user_index(ir: u8) -> usize {
    match ir {
        IR_USR2 => 0,
        IR_USR3 => 1,
        _ => 2,
    }
}

/// IR opcode for BSCAN user chain `n` (1..=4).
pub fn user_chain_ir(n: u8) -> Option<u8> {
    match n {
        1 => Some(crate::IR_USR1),
        2 => Some(IR_USR2),
        3 => Some(IR_USR3),
        4 => Some(IR_USR4),
        _ => None,
    }
}

/// Configuration-logic sync: the packet stream starts at the `HBIT` magic.
/// Bits before it (BYPASS registers of chain neighbours, padding) are
/// skipped, the way 7-series config logic waits for its sync word. Searches
/// the first 64 bit offsets; returns the stream unchanged when no magic is
/// found (the packet parser then reports the error).
pub(crate) fn cfg_sync(buf: &[u8], nbits: usize) -> Vec<u8> {
    const MAGIC: &[u8; 4] = b"HBIT";
    if buf.starts_with(MAGIC) {
        return buf.to_vec();
    }
    let bit = |i: usize| (buf[i / 8] >> (i % 8)) & 1;
    for off in 1..=64usize.min(nbits.saturating_sub(32)) {
        let byte_at = |k: usize| -> u8 { (0..8).fold(0u8, |a, j| a | (bit(off + k * 8 + j) << j)) };
        if (0..4).all(|k| byte_at(k) == MAGIC[k]) {
            let n = (nbits - off) / 8;
            return (0..n).map(byte_at).collect();
        }
    }
    buf.to_vec()
}

/// Configuration flash on the board's SPI pins plus the in-flight transaction.
#[derive(Clone, Debug)]
pub(crate) struct ConfigFlashPort {
    pub flash: SpiFlashSim,
    /// CS# low (a transaction is open).
    pub selected: bool,
    /// MOSI bytes clocked so far (MSB-first per byte).
    pub tx: Vec<u8>,
    /// Bits clocked in the current transaction.
    pub bits: usize,
    /// Last flash error on CS# rise (e.g. program without WREN).
    pub last_err: Option<String>,
}

impl Tap {
    /// Wire a configuration flash to the device's SPI pins (board model).
    pub fn attach_config_flash(&mut self, flash: SpiFlashSim) {
        self.spi = Some(ConfigFlashPort {
            flash,
            selected: false,
            tx: Vec::new(),
            bits: 0,
            last_err: None,
        });
    }

    /// The board's configuration flash, if one is wired.
    pub fn config_flash(&self) -> Option<&SpiFlashSim> {
        self.spi.as_ref().map(|p| &p.flash)
    }

    pub fn config_flash_mut(&mut self) -> Option<&mut SpiFlashSim> {
        self.spi.as_mut().map(|p| &mut p.flash)
    }

    /// Error the flash raised on the last CS# rise, if any.
    pub fn config_flash_error(&self) -> Option<&str> {
        self.spi.as_ref().and_then(|p| p.last_err.as_deref())
    }

    /// The 64-bit USER2..USER4 data register (`None` for other IRs).
    pub fn user_register(&self, ir: u8) -> Option<u64> {
        is_user_chain(ir).then(|| self.user_regs[user_index(ir)])
    }

    pub(crate) fn spi_select(&mut self) {
        if let Some(p) = self.spi.as_mut() {
            p.selected = true;
            p.tx.clear();
            p.bits = 0;
        }
    }

    /// MISO for the clock about to happen (no flash wired → line floats low).
    pub(crate) fn spi_miso(&self) -> bool {
        let Some(p) = self.spi.as_ref() else {
            return false;
        };
        if !p.selected {
            return false;
        }
        let byte_i = p.bits / 8;
        let bit_i = p.bits % 8;
        let out = p.flash.drive_byte(&p.tx[..byte_i.min(p.tx.len())]);
        (out >> (7 - bit_i)) & 1 == 1
    }

    pub(crate) fn spi_mosi(&mut self, tdi: bool) {
        let Some(p) = self.spi.as_mut() else {
            return;
        };
        if !p.selected {
            return;
        }
        let byte_i = p.bits / 8;
        let bit_i = p.bits % 8;
        if p.tx.len() <= byte_i {
            p.tx.push(0);
        }
        if tdi {
            p.tx[byte_i] |= 1 << (7 - bit_i);
        }
        p.bits += 1;
    }

    /// CS# rises: the flash executes the complete bytes it received.
    pub(crate) fn spi_release(&mut self) {
        let Some(p) = self.spi.as_mut() else {
            return;
        };
        if !p.selected {
            return;
        }
        p.selected = false;
        let whole = p.bits / 8;
        let tx: Vec<u8> = p.tx[..whole.min(p.tx.len())].to_vec();
        p.tx.clear();
        p.bits = 0;
        if tx.is_empty() {
            return;
        }
        p.last_err = p.flash.transfer(&tx).err();
    }
}
