//! Vivado `write_cfgmem` Configuration Flash Image Generator (UG908 / UG470):
//! - Intel HEX (`.mcs`) with Extended Linear Address (`04`), Data (`00`), and EOF (`01`) records
//!   and validated two's-complement checksums.
//! - Binary (`.bin`) flash image generation and SPIx1 / SPIx2 / QSPIx4 / BPIx16 bit-order swapping.
//! - MultiBoot & Golden Fallback Image composition (`WBSTAR` warm-boot start address, `TIMER`
//!   watchdog, `IPROG` command sequence, and automatic fallback recovery to golden image on
//!   corrupted/timed-out update image).

use crate::{Bitstream, Fabric, Stat};

pub const CFG_SYNC_WORD: u32 = 0xAA99_5566;
pub const CFG_DUMMY_WORD: u32 = 0xFFFF_FFFF;
pub const CFG_BUS_WIDTH_SYNC: u32 = 0x0000_00BB;
pub const CFG_BUS_WIDTH_DETECT: u32 = 0x1122_0044;
pub const CFG_CMD_IPROG: u32 = 0x0000_000F;
pub const MULTIBOOT_HEADER_MAGIC: [u8; 4] = *b"MB7S";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CfgBusWidth {
    SpiX1,
    SpiX2,
    QspiX4,
    BpiX16,
}

impl CfgBusWidth {
    pub fn bit_swap_required(&self) -> bool {
        matches!(self, Self::BpiX16)
    }
}

/// Reverse bit order within a single byte (`b7..b0 -> b0..b7`) for BPI/selectMAP bit-swapped flash formats.
pub fn swap_bits_in_byte(b: u8) -> u8 {
    b.reverse_bits()
}

/// Apply or undo per-byte bit-order swapping across a payload slice.
pub fn swap_byte_bits(data: &[u8]) -> Vec<u8> {
    data.iter().copied().map(swap_bits_in_byte).collect()
}

/// MultiBoot & Golden Fallback header configuration (UG470 chapter 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiBootConfig {
    /// Flash byte address of the Golden Fallback image (typically `0x0000_0000` or right after barrier).
    pub golden_addr: u32,
    /// Warm Boot Start Address (`WBSTAR`) pointing to the Update image in SPI flash.
    pub wbstar_addr: u32,
    /// Hardware configuration watchdog timer value (`TIMER` register; `0` = disabled).
    pub watchdog_timer: u32,
    /// SPI / QSPI configuration bus width.
    pub bus_width: CfgBusWidth,
}

impl Default for MultiBootConfig {
    fn default() -> Self {
        Self {
            golden_addr: 0x0000_0100,
            wbstar_addr: 0x0001_0000,
            watchdog_timer: 0x0000_FFFF,
            bus_width: CfgBusWidth::QspiX4,
        }
    }
}

/// Parsed MultiBoot header extracted from flash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiBootHeader {
    pub sync_word: u32,
    pub wbstar_addr: u32,
    pub golden_addr: u32,
    pub watchdog_timer: u32,
    pub iprog_cmd: u32,
    pub golden_len: u32,
    pub golden_crc32: u32,
    pub update_len: u32,
    pub update_crc32: u32,
}

/// Result of MultiBoot configuration load from flash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiBootLoadReport {
    pub attempted_wbstar_addr: u32,
    pub active_boot_addr: u32,
    pub booted_update_image: bool,
    pub fallback_triggered: bool,
    pub crc_error_detected: bool,
    pub watchdog_timeout_detected: bool,
    pub stat: Stat,
}

/// Compute standard IEEE 802.3 CRC-32 over `data`.
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Encode a 48-byte UG470 MultiBoot header (`DUMMY`, `BUS_WIDTH`, `SYNC`, `WBSTAR`, `TIMER`, `IPROG`,
/// plus golden/update image descriptors and CRC32).
pub fn encode_multiboot_header(
    cfg: &MultiBootConfig,
    golden_bytes: &[u8],
    update_bytes: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    out.extend_from_slice(&MULTIBOOT_HEADER_MAGIC);
    out.extend_from_slice(&CFG_DUMMY_WORD.to_be_bytes());
    out.extend_from_slice(&CFG_BUS_WIDTH_SYNC.to_be_bytes());
    out.extend_from_slice(&CFG_BUS_WIDTH_DETECT.to_be_bytes());
    out.extend_from_slice(&CFG_SYNC_WORD.to_be_bytes());
    out.extend_from_slice(&cfg.wbstar_addr.to_be_bytes());
    out.extend_from_slice(&cfg.golden_addr.to_be_bytes());
    out.extend_from_slice(&cfg.watchdog_timer.to_be_bytes());
    out.extend_from_slice(&CFG_CMD_IPROG.to_be_bytes());
    out.extend_from_slice(&(golden_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc32_ieee(golden_bytes).to_be_bytes());
    out.extend_from_slice(&(update_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc32_ieee(update_bytes).to_be_bytes());
    out
}

pub fn decode_multiboot_header(buf: &[u8]) -> Result<MultiBootHeader, String> {
    if buf.len() < 52 {
        return Err("multiboot header: buffer shorter than 52 bytes".into());
    }
    if buf[0..4] != MULTIBOOT_HEADER_MAGIC {
        return Err("multiboot header: missing MB7S magic".into());
    }
    let read_u32 = |off: usize| -> u32 {
        u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    };
    let sync_word = read_u32(16);
    if sync_word != CFG_SYNC_WORD {
        return Err(format!(
            "multiboot header: invalid sync word 0x{sync_word:08x}"
        ));
    }
    Ok(MultiBootHeader {
        sync_word,
        wbstar_addr: read_u32(20),
        golden_addr: read_u32(24),
        watchdog_timer: read_u32(28),
        iprog_cmd: read_u32(32),
        golden_len: read_u32(36),
        golden_crc32: read_u32(40),
        update_len: read_u32(44),
        update_crc32: read_u32(48),
    })
}

/// Build a composite MultiBoot flash binary containing:
/// - MultiBoot jump header (`WBSTAR`, `TIMER`, `IPROG`) at `0x0000_0000`
/// - Golden Fallback bitstream at `cfg.golden_addr`
/// - Update bitstream at `cfg.wbstar_addr`
/// Unused flash regions are padded with `0xFF` (erased NOR flash state).
pub fn build_multiboot_flash_image(
    cfg: &MultiBootConfig,
    golden_bits: &Bitstream,
    update_bits: &Bitstream,
) -> Result<Vec<u8>, String> {
    let golden_pkt = &golden_bits.packets;
    let update_pkt = &update_bits.packets;
    let hdr = encode_multiboot_header(cfg, golden_pkt, update_pkt);

    let g_off = cfg.golden_addr as usize;
    let u_off = cfg.wbstar_addr as usize;
    if g_off < hdr.len() {
        return Err("multiboot: golden_addr overlaps 52-byte MultiBoot header".into());
    }
    if u_off < g_off + golden_pkt.len() {
        return Err("multiboot: wbstar_addr overlaps golden fallback image".into());
    }

    let total_len = u_off + update_pkt.len();
    let mut image = vec![0xFFu8; total_len];
    image[..hdr.len()].copy_from_slice(&hdr);
    image[g_off..g_off + golden_pkt.len()].copy_from_slice(&golden_pkt);
    image[u_off..u_off + update_pkt.len()].copy_from_slice(&update_pkt);

    if cfg.bus_width.bit_swap_required() {
        Ok(swap_byte_bits(&image))
    } else {
        Ok(image)
    }
}

/// Simulate FPGA MultiBoot power-on configuration from a flash image:
/// 1. Reads MultiBoot header at `0x0000_0000` (`WBSTAR`, `TIMER`, `IPROG`).
/// 2. Jumps to `WBSTAR` (`update_addr`) and verifies update image CRC32 & bitstream packet framing.
/// 3. If the update image is intact, programs `fabric` with the update image (`booted_update_image = true`).
/// 4. If the update image is corrupted, erased (watchdog timeout), or fails CRC/decode, sets
///    `fallback_triggered = true` and recovers by programming `fabric` from the Golden image at `golden_addr`!
pub fn simulate_multiboot_load(
    fabric: &mut Fabric,
    flash_bytes: &[u8],
    bus_width: CfgBusWidth,
) -> Result<MultiBootLoadReport, String> {
    let raw = if bus_width.bit_swap_required() {
        swap_byte_bits(flash_bytes)
    } else {
        flash_bytes.to_vec()
    };

    let hdr = decode_multiboot_header(&raw)?;
    let u_off = hdr.wbstar_addr as usize;
    let u_end = u_off.saturating_add(hdr.update_len as usize);

    let mut crc_err = false;
    let mut wdt_timeout = false;

    if u_end <= raw.len() && hdr.update_len > 0 {
        let update_slice = &raw[u_off..u_end];
        // Check if erased (`0xFF` everywhere) -> watchdog timer timeout waiting for sync word
        if update_slice.iter().all(|&b| b == 0xFF) {
            wdt_timeout = true;
        } else {
            let actual_crc = crc32_ieee(update_slice);
            if actual_crc != hdr.update_crc32 {
                crc_err = true;
            } else if let Ok(bits) = Bitstream::from_packets(update_slice) {
                if fabric.program(&bits).is_ok() {
                    fabric.finish_startup();
                    return Ok(MultiBootLoadReport {
                        attempted_wbstar_addr: hdr.wbstar_addr,
                        active_boot_addr: hdr.wbstar_addr,
                        booted_update_image: true,
                        fallback_triggered: false,
                        crc_error_detected: false,
                        watchdog_timeout_detected: false,
                        stat: fabric.stat.clone(),
                    });
                }
                crc_err = true;
            } else {
                crc_err = true;
            }
        }
    } else {
        wdt_timeout = true;
    }

    // Fallback to Golden Image at `hdr.golden_addr`
    let g_off = hdr.golden_addr as usize;
    let g_end = g_off.saturating_add(hdr.golden_len as usize);
    if g_end > raw.len() {
        return Err("multiboot fallback failed: golden image truncated in flash".into());
    }
    let golden_slice = &raw[g_off..g_end];
    if crc32_ieee(golden_slice) != hdr.golden_crc32 {
        return Err("multiboot fallback failed: golden image CRC32 mismatch".into());
    }
    let golden_bits = Bitstream::from_packets(golden_slice)
        .map_err(|e| format!("multiboot golden decode: {e}"))?;
    fabric.program(&golden_bits)?;
    fabric.finish_startup();

    Ok(MultiBootLoadReport {
        attempted_wbstar_addr: hdr.wbstar_addr,
        active_boot_addr: hdr.golden_addr,
        booted_update_image: false,
        fallback_triggered: true,
        crc_error_detected: crc_err,
        watchdog_timeout_detected: wdt_timeout,
        stat: fabric.stat.clone(),
    })
}

/// Format a single Intel HEX record `:LLAAAATTDD...DDCC` with two's-complement checksum.
fn format_ihex_record(addr16: u16, rec_type: u8, data: &[u8]) -> String {
    let ll = data.len() as u8;
    let ah = (addr16 >> 8) as u8;
    let al = (addr16 & 0xFF) as u8;
    let mut sum = u32::from(ll) + u32::from(ah) + u32::from(al) + u32::from(rec_type);
    for &b in data {
        sum += u32::from(b);
    }
    let checksum = ((!(sum & 0xFF)).wrapping_add(1)) as u8;

    let mut line = format!(":{ll:02X}{addr16:04X}{rec_type:02X}");
    for &b in data {
        line.push_str(&format!("{b:02X}"));
    }
    line.push_str(&format!("{checksum:02X}\n"));
    line
}

/// Generate a Vivado `write_cfgmem -format mcs` Intel HEX configuration image.
/// Emits Extended Linear Address (`04`) records at 64 KiB boundaries, 16-byte Data (`00`)
/// records, and the terminating End-of-File (`01`) record `:00000001FF`.
pub fn write_cfgmem_mcs(image: &[u8], base_addr: u32, bus_width: CfgBusWidth) -> String {
    let payload = if bus_width.bit_swap_required() {
        swap_byte_bits(image)
    } else {
        image.to_vec()
    };

    let mut out = String::new();
    let mut current_ela: Option<u16> = None;
    let mut offset = 0usize;

    while offset < payload.len() {
        let abs_addr = base_addr.wrapping_add(offset as u32);
        let ela = (abs_addr >> 16) as u16;
        let addr16 = (abs_addr & 0xFFFF) as u16;

        if current_ela != Some(ela) {
            out.push_str(&format_ihex_record(0x0000, 0x04, &ela.to_be_bytes()));
            current_ela = Some(ela);
        }

        // Do not cross a 64 KiB boundary within a single data record
        let rem_in_64k = 0x10000usize - (addr16 as usize);
        let chunk_len = (payload.len() - offset).min(16).min(rem_in_64k);
        out.push_str(&format_ihex_record(
            addr16,
            0x00,
            &payload[offset..offset + chunk_len],
        ));
        offset += chunk_len;
    }

    // End-of-File record (type 01)
    out.push_str(&format_ihex_record(0x0000, 0x01, &[]));
    out
}

/// Parse and validate an Intel HEX (`.mcs`) configuration image, verifying every record's
/// two's-complement checksum and reconstructing the contiguous flash byte image.
pub fn parse_cfgmem_mcs(mcs_text: &str, bus_width: CfgBusWidth) -> Result<Vec<u8>, String> {
    let mut upper_addr: u32 = 0;
    let mut bytes_map: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut min_addr: Option<u32> = None;
    let mut max_addr: u32 = 0;
    let mut saw_eof = false;

    for (line_no, raw_line) in mcs_text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let Some(hex_body) = line.strip_prefix(':') else {
            return Err(format!("mcs line {}: missing ':' prefix", line_no + 1));
        };
        if hex_body.len() < 10 || hex_body.len() % 2 != 0 {
            return Err(format!("mcs line {}: invalid hex length", line_no + 1));
        }
        let mut raw_bytes = Vec::with_capacity(hex_body.len() / 2);
        for i in (0..hex_body.len()).step_by(2) {
            let b = u8::from_str_radix(&hex_body[i..i + 2], 16)
                .map_err(|_| format!("mcs line {}: invalid hex byte", line_no + 1))?;
            raw_bytes.push(b);
        }

        let ll = raw_bytes[0] as usize;
        if raw_bytes.len() != 5 + ll {
            return Err(format!(
                "mcs line {}: byte count {ll} does not match record length {}",
                line_no + 1,
                raw_bytes.len() - 5
            ));
        }

        // Verify two's-complement checksum: sum of all bytes in record (including checksum) mod 256 == 0
        let sum: u8 = raw_bytes.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
        if sum != 0 {
            return Err(format!(
                "mcs line {}: checksum verification failed (sum=0x{sum:02x})",
                line_no + 1
            ));
        }

        let addr16 = u32::from(u16::from_be_bytes([raw_bytes[1], raw_bytes[2]]));
        let rec_type = raw_bytes[3];
        let data = &raw_bytes[4..4 + ll];

        match rec_type {
            0x00 => {
                let full_addr = (upper_addr << 16) | addr16;
                min_addr = Some(min_addr.map_or(full_addr, |m| m.min(full_addr)));
                max_addr = max_addr.max(full_addr + data.len() as u32);
                bytes_map.push((full_addr, data.to_vec()));
            }
            0x01 => {
                saw_eof = true;
                break;
            }
            0x04 => {
                if data.len() != 2 {
                    return Err(format!(
                        "mcs line {}: type 04 record must have 2 data bytes",
                        line_no + 1
                    ));
                }
                upper_addr = u32::from(u16::from_be_bytes([data[0], data[1]]));
            }
            other => {
                return Err(format!(
                    "mcs line {}: unsupported record type 0x{other:02x}",
                    line_no + 1
                ));
            }
        }
    }

    if !saw_eof {
        return Err("mcs: missing End-of-File (:00000001FF) record".into());
    }

    let base = min_addr.unwrap_or(0);
    let len = (max_addr.saturating_sub(base)) as usize;
    let mut out = vec![0xFFu8; len];
    for (addr, chunk) in bytes_map {
        let off = (addr - base) as usize;
        out[off..off + chunk.len()].copy_from_slice(&chunk);
    }

    if bus_width.bit_swap_required() {
        Ok(swap_byte_bits(&out))
    } else {
        Ok(out)
    }
}

/// Generate a raw binary (`.bin`) configuration image for `write_cfgmem -format bin`.
pub fn write_cfgmem_bin(bits: &Bitstream, bus_width: CfgBusWidth) -> Vec<u8> {
    if bus_width.bit_swap_required() {
        swap_byte_bits(&bits.packets)
    } else {
        bits.packets.clone()
    }
}
