//! Configuration memory devices (Vivado `create_hw_cfgmem` / `write_cfgmem` /
//! `program_hw_cfgmem`).
//!
//! - [`CFGMEM_PARTS`]: the flash parts "Add Configuration Memory Device"
//!   offers, with their JEDEC IDs and densities (datasheet values).
//! - [`write_cfgmem`]: one entry point that turns a bitstream into an `.mcs`
//!   (Intel HEX) or `.bin` flash image for an interface and start address,
//!   checks it fits the part, and self-checks the MCS by parsing it back.
//! - [`CfgMemSession`]: the indirect programming flow over the sim cable —
//!   a Helion TAP tunnelling JEDEC SPI commands through `IR_USR1` into a
//!   [`SpiFlashSim`] — with erase / blank check / program / verify / boot as
//!   separate steps, the way Vivado's Program Configuration Memory dialog
//!   exposes them.
//!
//! BPI parts are listed so `write_cfgmem -interface bpix16` images can be
//! generated, but there is no BPI flash model: [`CfgMemSession::new`] refuses
//! them rather than pretending to program one. 24-bit SPI addressing caps the
//! listed SPI parts at 128 Mbit.

use crate::cfgmem::{
    CfgBusWidth, crc32_ieee, parse_cfgmem_mcs_at, swap_byte_bits, write_cfgmem_mcs,
};
use crate::jtag_cable::{CableKind, JtagCable};
use crate::native_cfgmem::{SpiOverJtag, SpiPort};
use crate::spi_flash::{JtagSpiBridge, SPI_SECTOR_SIZE, SpiFlashSim};
use crate::{Bitstream, Stat, Tap};
use helion_device::Device;

/// Configuration interface (`write_cfgmem -interface`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgMemInterface {
    SpiX1,
    SpiX2,
    SpiX4,
    BpiX16,
}

impl CfgMemInterface {
    pub const ALL: [CfgMemInterface; 4] = [
        CfgMemInterface::SpiX1,
        CfgMemInterface::SpiX2,
        CfgMemInterface::SpiX4,
        CfgMemInterface::BpiX16,
    ];

    /// Vivado spelling (`SPIx4`, `BPIx16`).
    pub fn as_str(self) -> &'static str {
        match self {
            CfgMemInterface::SpiX1 => "SPIx1",
            CfgMemInterface::SpiX2 => "SPIx2",
            CfgMemInterface::SpiX4 => "SPIx4",
            CfgMemInterface::BpiX16 => "BPIx16",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "spix1" | "spi" | "spi1" => Ok(CfgMemInterface::SpiX1),
            "spix2" | "spi2" => Ok(CfgMemInterface::SpiX2),
            "spix4" | "spi4" | "qspi" => Ok(CfgMemInterface::SpiX4),
            "bpix16" | "bpi" | "bpi16" => Ok(CfgMemInterface::BpiX16),
            o => Err(format!(
                "write_cfgmem: unknown -interface {o} (SPIx1|SPIx2|SPIx4|BPIx16)"
            )),
        }
    }

    pub fn is_spi(self) -> bool {
        !matches!(self, CfgMemInterface::BpiX16)
    }

    pub fn bus_width(self) -> CfgBusWidth {
        match self {
            CfgMemInterface::SpiX1 => CfgBusWidth::SpiX1,
            CfgMemInterface::SpiX2 => CfgBusWidth::SpiX2,
            CfgMemInterface::SpiX4 => CfgBusWidth::QspiX4,
            CfgMemInterface::BpiX16 => CfgBusWidth::BpiX16,
        }
    }
}

/// One configuration flash part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CfgMemPart {
    /// Vivado-style part alias (`mt25ql128-spi-x1_x2_x4`).
    pub name: &'static str,
    pub vendor: &'static str,
    /// Widest interface the part supports (SPI parts also accept x1/x2).
    pub interface: CfgMemInterface,
    pub density_mbit: u32,
    /// `RDID (0x9F)` manufacturer / type / capacity bytes (SPI only).
    pub jedec_id: Option<[u8; 3]>,
}

impl CfgMemPart {
    pub fn capacity_bytes(&self) -> usize {
        self.density_mbit as usize * 1024 * 1024 / 8
    }

    /// Whether `iface` is wired the part supports.
    pub fn supports(&self, iface: CfgMemInterface) -> bool {
        match (self.interface, iface) {
            (CfgMemInterface::BpiX16, i) => i == CfgMemInterface::BpiX16,
            (_, CfgMemInterface::BpiX16) => false,
            (max, i) => (i as u8) <= (max as u8),
        }
    }

    pub fn jedec_hex(&self) -> String {
        match self.jedec_id {
            Some([a, b, c]) => format!("{a:02X} {b:02X} {c:02X}"),
            None => "-".into(),
        }
    }
}

/// Flash parts offered by "Add Configuration Memory Device".
pub const CFGMEM_PARTS: &[CfgMemPart] = &[
    CfgMemPart {
        name: "mt25ql128-spi-x1_x2_x4",
        vendor: "Micron",
        interface: CfgMemInterface::SpiX4,
        density_mbit: 128,
        jedec_id: Some([0x20, 0xBA, 0x18]),
    },
    CfgMemPart {
        name: "s25fl128sxxxxxx0-spi-x1_x2_x4",
        vendor: "Infineon",
        interface: CfgMemInterface::SpiX4,
        density_mbit: 128,
        jedec_id: Some([0x01, 0x20, 0x18]),
    },
    CfgMemPart {
        name: "w25q128jv-spi-x1_x2_x4",
        vendor: "Winbond",
        interface: CfgMemInterface::SpiX4,
        density_mbit: 128,
        jedec_id: Some([0xEF, 0x40, 0x18]),
    },
    CfgMemPart {
        name: "is25lp128f-spi-x1_x2_x4",
        vendor: "ISSI",
        interface: CfgMemInterface::SpiX4,
        density_mbit: 128,
        jedec_id: Some([0x9D, 0x60, 0x18]),
    },
    CfgMemPart {
        name: "mx25l3233f-spi-x1_x2_x4",
        vendor: "Macronix",
        interface: CfgMemInterface::SpiX4,
        density_mbit: 32,
        jedec_id: Some([0xC2, 0x20, 0x16]),
    },
    CfgMemPart {
        name: "w25q32jv-spi-x1_x2_x4",
        vendor: "Winbond",
        interface: CfgMemInterface::SpiX4,
        density_mbit: 32,
        jedec_id: Some([0xEF, 0x40, 0x16]),
    },
    CfgMemPart {
        name: "mt28ew128a-bpi-x16",
        vendor: "Micron",
        interface: CfgMemInterface::BpiX16,
        density_mbit: 128,
        jedec_id: None,
    },
];

/// Look a part up by exact alias (case-insensitive) or a unique prefix.
pub fn find_cfgmem_part(name: &str) -> Option<&'static CfgMemPart> {
    let n = name.trim().to_ascii_lowercase();
    if let Some(p) = CFGMEM_PARTS.iter().find(|p| p.name == n) {
        return Some(p);
    }
    let mut hits = CFGMEM_PARTS.iter().filter(|p| p.name.starts_with(&n));
    match (hits.next(), hits.next()) {
        (Some(p), None) => Some(p),
        _ => None,
    }
}

/// `write_cfgmem -format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgMemFormat {
    Mcs,
    Bin,
}

impl CfgMemFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            CfgMemFormat::Mcs => "mcs",
            CfgMemFormat::Bin => "bin",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mcs" | "hex" => Ok(CfgMemFormat::Mcs),
            "bin" => Ok(CfgMemFormat::Bin),
            o => Err(format!("write_cfgmem: unknown -format {o} (mcs|bin)")),
        }
    }
}

/// A flash image: the bytes the flash will hold plus the file encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CfgMemImage {
    pub format: CfgMemFormat,
    pub interface: CfgMemInterface,
    pub part: &'static str,
    /// First flash byte address.
    pub start_addr: u32,
    /// Bytes as stored in flash from `start_addr` (BPI images bit-swapped).
    pub payload: Vec<u8>,
    /// File contents (`.mcs` text bytes or the raw `.bin`).
    pub file: Vec<u8>,
    pub crc32: u32,
}

impl CfgMemImage {
    pub fn end_addr(&self) -> u32 {
        self.start_addr + self.payload.len() as u32
    }

    /// Payload as the configuration logic reads it (BPI swap undone).
    pub fn bitstream_bytes(&self) -> Vec<u8> {
        if self.interface.bus_width().bit_swap_required() {
            swap_byte_bits(&self.payload)
        } else {
            self.payload.clone()
        }
    }

    pub fn write(&self, path: &std::path::Path) -> Result<(), String> {
        std::fs::write(path, &self.file)
            .map_err(|e| format!("write_cfgmem: write {}: {e}", path.display()))
    }
}

/// `write_cfgmem -format mcs|bin -interface I -size MB -loadbit "up ADDR f"`.
pub fn write_cfgmem(
    bits: &Bitstream,
    part: &'static CfgMemPart,
    format: CfgMemFormat,
    interface: CfgMemInterface,
    start_addr: u32,
) -> Result<CfgMemImage, String> {
    crate::refuse_empty_bitstream(bits)?;
    if !part.supports(interface) {
        return Err(format!(
            "write_cfgmem: {} does not support -interface {}",
            part.name,
            interface.as_str()
        ));
    }
    if interface.is_spi() && start_addr as usize % SPI_SECTOR_SIZE != 0 {
        return Err(format!(
            "write_cfgmem: start {start_addr:#x} is not on a 64 KiB sector boundary"
        ));
    }
    let raw = &bits.packets;
    let end = start_addr as usize + raw.len();
    if end > part.capacity_bytes() {
        return Err(format!(
            "write_cfgmem: image ends at {end:#x}, past the {} Mbit part ({:#x} bytes)",
            part.density_mbit,
            part.capacity_bytes()
        ));
    }
    let bw = interface.bus_width();
    let payload = if bw.bit_swap_required() {
        swap_byte_bits(raw)
    } else {
        raw.clone()
    };
    let file = match format {
        CfgMemFormat::Mcs => {
            let text = write_cfgmem_mcs(raw, start_addr, bw);
            // Self-check: the file must parse back to the same bitstream.
            let (base, back) = parse_cfgmem_mcs_at(&text, bw)?;
            if base != start_addr || back != *raw {
                return Err("write_cfgmem: MCS round-trip mismatch".into());
            }
            text.into_bytes()
        }
        CfgMemFormat::Bin => payload.clone(),
    };
    Ok(CfgMemImage {
        format,
        interface,
        part: part.name,
        start_addr,
        crc32: crc32_ieee(&payload),
        payload,
        file,
    })
}

/// Read a `.mcs` / `.bin` flash image back (`PROGRAM.FILES`). A `.bin` has
/// no address, so it loads at `bin_start`.
pub fn read_cfgmem_file(
    path: &std::path::Path,
    part: &'static CfgMemPart,
    interface: CfgMemInterface,
    bin_start: u32,
) -> Result<CfgMemImage, String> {
    let file = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let is_mcs = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mcs") || e.eq_ignore_ascii_case("hex"));
    let bw = interface.bus_width();
    let (format, start_addr, payload) = if is_mcs {
        let text = String::from_utf8(file.clone()).map_err(|_| "mcs: not ASCII".to_string())?;
        let (base, bytes) = parse_cfgmem_mcs_at(&text, bw)?;
        // parse undoes the BPI swap; store flash order.
        let stored = if bw.bit_swap_required() {
            swap_byte_bits(&bytes)
        } else {
            bytes
        };
        (CfgMemFormat::Mcs, base, stored)
    } else {
        (CfgMemFormat::Bin, bin_start, file.clone())
    };
    if payload.is_empty() {
        return Err(format!("{}: empty flash image", path.display()));
    }
    if start_addr as usize + payload.len() > part.capacity_bytes() {
        return Err(format!(
            "{}: image does not fit the {} Mbit part",
            path.display(),
            part.density_mbit
        ));
    }
    Ok(CfgMemImage {
        format,
        interface,
        part: part.name,
        start_addr,
        crc32: crc32_ieee(&payload),
        payload,
        file,
    })
}

/// Which steps `program_hw_cfgmem` runs (Vivado's PROGRAM.* properties).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CfgMemProgramOpts {
    pub erase: bool,
    pub blank_check: bool,
    pub program: bool,
    pub verify: bool,
}

impl Default for CfgMemProgramOpts {
    fn default() -> Self {
        Self {
            erase: true,
            blank_check: false,
            program: true,
            verify: true,
        }
    }
}

/// Result of one cfgmem operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CfgMemReport {
    /// Steps in order, each `step: detail`.
    pub steps: Vec<String>,
    pub sectors_erased: usize,
    pub pages_programmed: usize,
    pub jtag_dr_scans: usize,
    /// `Some(true)` verified, `Some(false)` mismatched, `None` not verified.
    pub verified: Option<bool>,
    pub expected_crc32: Option<u32>,
    pub readback_crc32: Option<u32>,
    /// Non-0xFF bytes found by a blank check.
    pub nonblank_bytes: Option<usize>,
}

/// Where the session's flash lives.
enum Link {
    /// Simulation cable: TAP + flash model behind the USER1 bridge.
    Sim { tap: Tap, flash: SpiFlashSim },
    /// Any [`JtagCable`] reaching the board's flash through
    /// [`crate::IR_SPI_ACCESS`] (native FTDI: real MPSSE traffic).
    Cable(Box<dyn JtagCable>),
}

/// Indirect flash programming (`create_hw_cfgmem` … `program_hw_cfgmem`).
pub struct CfgMemSession {
    pub part: &'static CfgMemPart,
    link: Link,
}

impl std::fmt::Debug for CfgMemSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CfgMemSession")
            .field("part", &self.part.name)
            .field("cable", &self.cable_kind())
            .finish()
    }
}

fn spi_part_id(part: &CfgMemPart) -> Result<[u8; 3], String> {
    part.jedec_id.ok_or_else(|| {
        format!(
            "create_hw_cfgmem: {} is BPI; helion-hw has no BPI flash model \
             (write_cfgmem -interface BPIx16 images are still generated)",
            part.name
        )
    })
}

impl CfgMemSession {
    /// `create_hw_cfgmem -hw_device D PART` on the sim cable.
    pub fn new(dev: &Device, part: &'static CfgMemPart) -> Result<Self, String> {
        let id = spi_part_id(part)?;
        let mut flash = SpiFlashSim::new(part.capacity_bytes());
        flash.jedec_id = id;
        Ok(Self {
            part,
            link: Link::Sim {
                tap: Tap::new(dev),
                flash,
            },
        })
    }

    /// `create_hw_cfgmem` on a native cable: the flash is whatever the board
    /// wires to the device's configuration SPI pins.
    pub fn over_cable(
        part: &'static CfgMemPart,
        cable: Box<dyn JtagCable>,
    ) -> Result<Self, String> {
        spi_part_id(part)?;
        Ok(Self {
            part,
            link: Link::Cable(cable),
        })
    }

    pub fn cable_kind(&self) -> CableKind {
        match &self.link {
            Link::Sim { .. } => CableKind::Sim,
            Link::Cable(c) => c.kind(),
        }
    }

    pub fn cable_detail(&self) -> String {
        match &self.link {
            Link::Sim { .. } => "Simulation cable · USER1 SPI bridge model".into(),
            Link::Cable(c) => format!("{} · IR SPI_ACCESS", c.describe()),
        }
    }

    fn with_port<R>(
        &mut self,
        f: impl FnOnce(&mut dyn SpiPort) -> Result<R, String>,
    ) -> Result<(R, usize), String> {
        match &mut self.link {
            Link::Sim { tap, flash } => {
                let fl = std::mem::replace(flash, SpiFlashSim::new(0));
                let mut bridge = JtagSpiBridge::new(tap, fl);
                let r = f(&mut bridge);
                let scans = bridge.jtag_dr_scans;
                *flash = bridge.flash;
                r.map(|v| (v, scans))
            }
            Link::Cable(c) => {
                let mut spi = SpiOverJtag::new(c.as_mut());
                let r = f(&mut spi);
                let scans = spi.scans;
                r.map(|v| (v, scans))
            }
        }
    }

    fn check_range(&self, start: u32, len: usize) -> Result<(), String> {
        if start as usize + len > self.part.capacity_bytes() {
            return Err(format!(
                "cfgmem: range {start:#x}+{len:#x} past the {} Mbit part",
                self.part.density_mbit
            ));
        }
        Ok(())
    }

    /// `RDID` over the cable; errors if it does not match the part.
    pub fn read_id(&mut self) -> Result<[u8; 3], String> {
        let (id, _) = self.with_port(|b| b.jedec_id())?;
        if Some(id) != self.part.jedec_id {
            return Err(format!(
                "cfgmem: JEDEC ID {id:02X?} does not match {} ({})",
                self.part.name,
                self.part.jedec_hex()
            ));
        }
        Ok(id)
    }

    /// Erase the 64 KiB sectors covering `[start, start+len)`.
    pub fn erase(&mut self, start: u32, len: usize) -> Result<CfgMemReport, String> {
        self.check_range(start, len.max(1))?;
        let first = start as usize / SPI_SECTOR_SIZE;
        let last = (start as usize + len.max(1) - 1) / SPI_SECTOR_SIZE;
        let (_, scans) = self.with_port(|b| {
            for sec in first..=last {
                b.erase_sector((sec * SPI_SECTOR_SIZE) as u32)?;
            }
            Ok(())
        })?;
        let n = last + 1 - first;
        Ok(CfgMemReport {
            steps: vec![format!(
                "erase: {n} sector(s) {:#x}..{:#x}",
                first * SPI_SECTOR_SIZE,
                (last + 1) * SPI_SECTOR_SIZE
            )],
            sectors_erased: n,
            jtag_dr_scans: scans,
            ..CfgMemReport::default()
        })
    }

    /// Erase the whole part.
    pub fn erase_all(&mut self) -> Result<CfgMemReport, String> {
        let cap = self.part.capacity_bytes();
        self.erase(0, cap)
    }

    /// Read `len` bytes at `start` back over the cable.
    pub fn readback(&mut self, start: u32, len: usize) -> Result<(CfgMemReport, Vec<u8>), String> {
        self.check_range(start, len)?;
        let (data, scans) = self.with_port(|b| b.read(start, len))?;
        let crc = crc32_ieee(&data);
        Ok((
            CfgMemReport {
                steps: vec![format!("read back: {len} byte(s) at {start:#x} · CRC32 {crc:08X}")],
                jtag_dr_scans: scans,
                readback_crc32: Some(crc),
                ..CfgMemReport::default()
            },
            data,
        ))
    }

    /// Count non-erased bytes in a range (read back over the cable).
    pub fn blank_check(&mut self, start: u32, len: usize) -> Result<CfgMemReport, String> {
        self.check_range(start, len)?;
        let (data, scans) = self.with_port(|b| b.read(start, len))?;
        let nonblank = data.iter().filter(|&&b| b != 0xFF).count();
        Ok(CfgMemReport {
            steps: vec![format!(
                "blank check: {} byte(s) at {start:#x} — {}",
                len,
                if nonblank == 0 {
                    "blank".to_string()
                } else {
                    format!("{nonblank} non-blank")
                }
            )],
            jtag_dr_scans: scans,
            nonblank_bytes: Some(nonblank),
            ..CfgMemReport::default()
        })
    }

    /// Read back and compare against `payload` at `start`.
    pub fn verify(&mut self, start: u32, payload: &[u8]) -> Result<CfgMemReport, String> {
        self.check_range(start, payload.len())?;
        let (data, scans) = self.with_port(|b| b.read(start, payload.len()))?;
        let exp = crc32_ieee(payload);
        let got = crc32_ieee(&data);
        let ok = data == payload;
        let first_bad = data.iter().zip(payload).position(|(a, b)| a != b);
        Ok(CfgMemReport {
            steps: vec![match first_bad {
                None => format!("verify: {} byte(s) match · CRC32 {exp:08X}", payload.len()),
                Some(i) => format!(
                    "verify: MISMATCH at {:#x} · CRC32 expected {exp:08X} got {got:08X}",
                    start as usize + i
                ),
            }],
            jtag_dr_scans: scans,
            verified: Some(ok),
            expected_crc32: Some(exp),
            readback_crc32: Some(got),
            ..CfgMemReport::default()
        })
    }

    /// `program_hw_cfgmem`: the selected steps in Vivado's order.
    pub fn program(
        &mut self,
        image: &CfgMemImage,
        opts: CfgMemProgramOpts,
    ) -> Result<CfgMemReport, String> {
        if !image.interface.is_spi() {
            return Err("program_hw_cfgmem: BPI images cannot be programmed (no BPI model)".into());
        }
        if !self.part.supports(image.interface) {
            return Err(format!(
                "program_hw_cfgmem: image interface {} not supported by {}",
                image.interface.as_str(),
                self.part.name
            ));
        }
        let start = image.start_addr;
        let len = image.payload.len();
        self.check_range(start, len)?;
        let mut rep = CfgMemReport::default();
        let id = self.read_id()?;
        rep.steps
            .push(format!("id: JEDEC {:02X} {:02X} {:02X}", id[0], id[1], id[2]));
        if opts.erase {
            let r = self.erase(start, len)?;
            rep.sectors_erased = r.sectors_erased;
            rep.jtag_dr_scans += r.jtag_dr_scans;
            rep.steps.extend(r.steps);
        }
        if opts.blank_check {
            let r = self.blank_check(start, len)?;
            rep.jtag_dr_scans += r.jtag_dr_scans;
            rep.steps.extend(r.steps);
            if r.nonblank_bytes.unwrap_or(0) > 0 {
                rep.nonblank_bytes = r.nonblank_bytes;
                return Err(format!(
                    "program_hw_cfgmem: blank check failed ({} non-blank bytes); enable erase",
                    r.nonblank_bytes.unwrap_or(0)
                ));
            }
            rep.nonblank_bytes = Some(0);
        }
        if opts.program {
            let payload = image.payload.clone();
            let (pages, scans) = self.with_port(|b| {
                let end = start as usize + payload.len();
                let mut cur = start as usize;
                let mut pages = 0usize;
                while cur < end {
                    let room = crate::SPI_PAGE_SIZE - cur % crate::SPI_PAGE_SIZE;
                    let n = room.min(end - cur);
                    let off = cur - start as usize;
                    b.program_page(cur as u32, &payload[off..off + n])?;
                    pages += 1;
                    cur += n;
                }
                Ok(pages)
            })?;
            rep.pages_programmed = pages;
            rep.jtag_dr_scans += scans;
            rep.steps.push(format!(
                "program: {} byte(s) in {} page(s) at {start:#x}",
                len, rep.pages_programmed
            ));
        }
        if opts.verify {
            let r = self.verify(start, &image.payload)?;
            rep.jtag_dr_scans += r.jtag_dr_scans;
            rep.verified = r.verified;
            rep.expected_crc32 = r.expected_crc32;
            rep.readback_crc32 = r.readback_crc32;
            rep.steps.extend(r.steps);
            if r.verified == Some(false) {
                return Err(format!(
                    "program_hw_cfgmem: verify failed — {}",
                    rep.steps.last().cloned().unwrap_or_default()
                ));
            }
        }
        Ok(rep)
    }

    /// Configure the fabric from flash (`boot_hw_device` after programming):
    /// reads `len` bytes at `start` back over the cable and runs CFG_W.
    pub fn boot(&mut self, start: u32, len: usize) -> Result<Stat, String> {
        self.check_range(start, len)?;
        let (st, _) = self.with_port(|b| b.boot(start, len))?;
        Ok(st)
    }

    /// Flash-model bytes (tests): the sim flash, or the mock board's flash.
    /// `None` on a real cable — read it back with [`Self::readback`].
    pub fn model_bytes(&mut self, start: u32, len: usize) -> Option<Vec<u8>> {
        let mem = match &mut self.link {
            Link::Sim { flash, .. } => &flash.mem,
            Link::Cable(c) => &c.board()?.config_flash()?.mem,
        };
        let s = (start as usize).min(mem.len());
        let e = (s + len).min(mem.len());
        Some(mem[s..e].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits() -> (Device, Bitstream) {
        use helion_ir::Design;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let d = Design::structural_counter();
        let packed = helion_pack::pack(&d, &dev).unwrap();
        let placed = helion_place::place(&packed, &dev).unwrap();
        let routed = helion_route::route(&placed, &dev).unwrap();
        let b = helion_bits::bitgen(&dev, &routed).unwrap();
        (dev, b)
    }

    fn part(n: &str) -> &'static CfgMemPart {
        find_cfgmem_part(n).unwrap()
    }

    #[test]
    fn catalog_lookup_and_interfaces() {
        let p = part("mt25ql128");
        assert_eq!(p.capacity_bytes(), 16 * 1024 * 1024);
        assert!(p.supports(CfgMemInterface::SpiX1));
        assert!(p.supports(CfgMemInterface::SpiX4));
        assert!(!p.supports(CfgMemInterface::BpiX16));
        assert!(part("mt28ew128a").supports(CfgMemInterface::BpiX16));
        assert!(find_cfgmem_part("w25q").is_none(), "ambiguous prefix");
        assert_eq!(CfgMemInterface::parse("spix4").unwrap(), CfgMemInterface::SpiX4);
    }

    #[test]
    fn write_cfgmem_mcs_and_bin_roundtrip() {
        let (_, b) = bits();
        let p = part("mt25ql128");
        let mcs = write_cfgmem(&b, p, CfgMemFormat::Mcs, CfgMemInterface::SpiX4, 0x1_0000).unwrap();
        assert_eq!(mcs.payload, b.packets);
        let text = String::from_utf8(mcs.file.clone()).unwrap();
        assert!(text.ends_with(":00000001FF\n"));
        let bin = write_cfgmem(&b, p, CfgMemFormat::Bin, CfgMemInterface::SpiX1, 0).unwrap();
        assert_eq!(bin.file, b.packets);
        // BPI images are bit-swapped and refuse SPI parts.
        assert!(write_cfgmem(&b, p, CfgMemFormat::Bin, CfgMemInterface::BpiX16, 0).is_err());
        let bpi = write_cfgmem(
            &b,
            part("mt28ew128a"),
            CfgMemFormat::Mcs,
            CfgMemInterface::BpiX16,
            0,
        )
        .unwrap();
        assert_eq!(bpi.bitstream_bytes(), b.packets);
        assert_ne!(bpi.payload, b.packets);
        // Misaligned and oversize starts are refused.
        assert!(write_cfgmem(&b, p, CfgMemFormat::Mcs, CfgMemInterface::SpiX4, 0x100).is_err());
        assert!(
            write_cfgmem(&b, p, CfgMemFormat::Mcs, CfgMemInterface::SpiX4, 0x00FF_0000 + 0x10000)
                .is_err()
        );
    }

    #[test]
    fn read_cfgmem_file_restores_address_and_bytes() {
        let (_, b) = bits();
        let p = part("mt25ql128");
        let img = write_cfgmem(&b, p, CfgMemFormat::Mcs, CfgMemInterface::SpiX4, 0x2_0000).unwrap();
        let dir = std::env::temp_dir().join(format!("helion-cfgmem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("img.mcs");
        img.write(&path).unwrap();
        let back = read_cfgmem_file(&path, p, CfgMemInterface::SpiX4, 0).unwrap();
        assert_eq!(back.start_addr, 0x2_0000);
        assert_eq!(back.payload, img.payload);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_erase_program_verify_boot() {
        let (dev, b) = bits();
        let p = part("w25q32jv");
        let img = write_cfgmem(&b, p, CfgMemFormat::Mcs, CfgMemInterface::SpiX4, 0).unwrap();
        let mut s = CfgMemSession::new(&dev, p).unwrap();
        assert_eq!(s.read_id().unwrap(), [0xEF, 0x40, 0x16]);
        // Fresh flash is blank.
        let blank = s.blank_check(0, img.payload.len()).unwrap();
        assert_eq!(blank.nonblank_bytes, Some(0));
        let rep = s.program(&img, CfgMemProgramOpts::default()).unwrap();
        assert_eq!(rep.verified, Some(true));
        assert!(rep.sectors_erased >= 1 && rep.pages_programmed >= 1);
        assert!(rep.jtag_dr_scans > 0);
        assert_eq!(s.model_bytes(0, img.payload.len()).unwrap(), img.payload);
        // Blank check without erase now fails honestly.
        let opts = CfgMemProgramOpts {
            erase: false,
            blank_check: true,
            program: true,
            verify: true,
        };
        assert!(s.program(&img, opts).is_err());
        // Boot from flash configures the fabric.
        let st = s.boot(0, img.payload.len()).unwrap();
        assert!(st.done);
        // Erase-all then verify reports a mismatch.
        s.erase_all().unwrap();
        let v = s.verify(0, &img.payload).unwrap();
        assert_eq!(v.verified, Some(false));
        assert!(s.boot(0, img.payload.len()).is_err());
    }

    #[test]
    fn native_session_erase_program_verify_readback_boot_over_mpsse() {
        let (dev, b) = bits();
        let p = part("w25q32jv");
        let img = write_cfgmem(&b, p, CfgMemFormat::Bin, CfgMemInterface::SpiX1, 0).unwrap();
        let mut board = crate::MockFtdi::helion(&dev);
        let mut flash = SpiFlashSim::new(p.capacity_bytes());
        flash.jedec_id = p.jedec_id.unwrap();
        board.helion_tap_mut().unwrap().attach_config_flash(flash);
        let eng = crate::JtagEngine::open(board, 15_000_000).unwrap();
        let mut s = CfgMemSession::over_cable(p, Box::new(eng)).unwrap();
        assert_eq!(s.cable_kind(), CableKind::NativeMock);
        assert_eq!(s.read_id().unwrap(), [0xEF, 0x40, 0x16]);
        let rep = s.program(&img, CfgMemProgramOpts::default()).unwrap();
        assert_eq!(rep.verified, Some(true));
        assert!(rep.sectors_erased >= 1 && rep.pages_programmed >= 1);
        assert_eq!(s.model_bytes(0, img.payload.len()).unwrap(), img.payload);
        let (rb, data) = s.readback(0, img.payload.len()).unwrap();
        assert_eq!(data, img.payload);
        assert_eq!(rb.readback_crc32, Some(crc32_ieee(&img.payload)));
        let st = s.boot(0, img.payload.len()).unwrap();
        assert!(st.done, "booted from the flash image over the native engine");
        s.erase(0, img.payload.len()).unwrap();
        assert_eq!(s.verify(0, &img.payload).unwrap().verified, Some(false));
        // Wrong part on the board: JEDEC mismatch is reported, not assumed.
        let other = part("mt25ql128");
        let board2 = crate::MockFtdi::helion(&dev);
        let eng2 = crate::JtagEngine::open(board2, 1_000_000).unwrap();
        let mut s2 = CfgMemSession::over_cable(other, Box::new(eng2)).unwrap();
        assert!(s2.read_id().unwrap_err().contains("no SPI flash"));
    }

    #[test]
    fn bpi_sessions_are_refused() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        assert!(CfgMemSession::new(&dev, part("mt28ew128a")).is_err());
    }
}
