//! Series-7 configuration packet stream and `.bit` / `.bin` writers.
//!
//! Replicates openXC7/prjxray `xc7frames2bit` exactly:
//!
//! * Packet stream: the UG470 bus-width-detect preamble, then the fixed
//!   configuration sequence (TIMER/WBST/AR reset, COR0/COR1, IDCODE,
//!   SWITCH, MASK/CTL0/CTL1, FAR=0, WCFG, Type-1 + Type-2 FDRI write of the
//!   frame payload, RCRC, GRESTORE, LFRM, START, shutdown FAR, RCRC,
//!   DESYNC, trailing NOPs).
//! * `.bit`: the TLV envelope (magic + `a`/`b`/`c`/`d`/`e` tags).
//! * `.bin`: the big-endian packet stream alone.
//!
//! Two places where the prose spec this crate was given differs from the
//! reference tool, resolved in favor of the tool (the byte-compare target):
//!
//! * The preamble is the 13-word UG470 sequence `8xFFFFFFFF, 000000BB,
//!   11220044, 2xFFFFFFFF, AA995566`, not the 6-word shorthand.
//! * Type-1 headers place the register at bits 26:13 (`reg << 13`), with
//!   bits 12:11 reserved: e.g. the IDCODE write header is `0x30018001`.
//! * COR0 is `0x02003FE5` (pipeline DONE-in, DONE release phase 4,
//!   no DCI/MMCM stall waits, GTS phase 5, GWE phase 6), not zero.
//! * CTL1 lives at register `0x18`.

use std::error::Error;
use std::fmt;

use crate::frames::{WORDS_PER_FRAME, XC7A35T_FRAME_COUNT, ZERO_PAD_FRAMES};

/// IDCODE of xc7a35t (from `part.json`).
pub const IDCODE_XC7A35T: u32 = 0x0362_D093;

/// COR0 as written by `xc7frames2bit` (prjxray `ConfigurationOptions0Value`
/// with pipeline-DONE-in, DONE release at startup phase 4, no stall waits,
/// GTS release phase 5, GWE release phase 6).
pub const COR0_DEFAULT: u32 = 0x0200_3FE5;

/// Expected FDRI payload words for xc7a35t: 5408 frames plus 12 zero pad
/// frames, 101 words each.
pub const XC7A35T_FDRI_WORDS: usize =
    (XC7A35T_FRAME_COUNT + 6 * ZERO_PAD_FRAMES) * WORDS_PER_FRAME;

/// Configuration registers (UG470 Table 6-2).
pub mod reg {
    /// CRC register (also the NOP packet's address).
    pub const CRC: u32 = 0x00;
    /// Frame Address Register.
    pub const FAR: u32 = 0x01;
    /// Frame Data Register, Input.
    pub const FDRI: u32 = 0x02;
    /// Command register.
    pub const CMD: u32 = 0x04;
    /// Control register 0.
    pub const CTL0: u32 = 0x05;
    /// Mask register.
    pub const MASK: u32 = 0x06;
    /// Configuration Options register 0.
    pub const COR0: u32 = 0x09;
    /// IDCODE register.
    pub const IDCODE: u32 = 0x0C;
    /// Configuration Options register 1.
    pub const COR1: u32 = 0x0E;
    /// Warm Boot Start Address.
    pub const WBSTAR: u32 = 0x10;
    /// Timer register.
    pub const TIMER: u32 = 0x11;
    /// Undocumented register written with 0 during init.
    pub const UNKNOWN: u32 = 0x13;
    /// Control register 1.
    pub const CTL1: u32 = 0x18;
}

/// Command register codes.
pub mod cmd {
    /// No-op.
    pub const NOP: u32 = 0x0;
    /// Write Configuration data.
    pub const WCFG: u32 = 0x1;
    /// Last Frame (update CRC).
    pub const LFRM: u32 = 0x3;
    /// Begin startup sequence.
    pub const START: u32 = 0x5;
    /// Reset CRC.
    pub const RCRC: u32 = 0x7;
    /// Switch CCLK frequency.
    pub const SWITCH: u32 = 0x9;
    /// Restore GWE after startup.
    pub const GRESTORE: u32 = 0xA;
    /// De-synchronize (end of bitstream).
    pub const DESYNC: u32 = 0xD;
}

/// Packet opcodes (bits 28:27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Op {
    /// No-op opcode.
    Nop = 0,
    /// Read opcode.
    Read = 1,
    /// Write opcode.
    Write = 2,
}

/// Encode a Type-1 packet header: `[001][op:2][reg:14][rsvd:2][count:11]`.
/// The register occupies bits 26:13; bits 12:11 are reserved zero.
pub fn type1_header(op: Op, reg: u32, count: u32) -> Result<u32, BitstreamError> {
    if reg > 0x3FFF {
        return Err(BitstreamError {
            message: format!("register {reg:#x} does not fit 14 bits"),
        });
    }
    if count > 0x7FF {
        return Err(BitstreamError {
            message: format!("type-1 count {count} does not fit 11 bits"),
        });
    }
    Ok(0x2000_0000 | ((op as u32) << 27) | (reg << 13) | count)
}

/// Encode a Type-2 packet header: `[010][op:2][count:27]`.
pub fn type2_header(op: Op, count: u32) -> Result<u32, BitstreamError> {
    if count > 0x07FF_FFFF {
        return Err(BitstreamError {
            message: format!("type-2 count {count} does not fit 27 bits"),
        });
    }
    Ok(0x4000_0000 | ((op as u32) << 27) | count)
}

/// A bitstream-assembly failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitstreamError {
    /// Human-readable description.
    pub message: String,
}

impl fmt::Display for BitstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bitstream error: {}", self.message)
    }
}

impl Error for BitstreamError {}

/// Options controlling the generated bitstream. Defaults match
/// `xc7frames2bit` for `xc7a35tcsg324-1` except the timestamp, which
/// defaults to the current UTC time (override it for reproducible builds).
#[derive(Debug, Clone)]
pub struct BitstreamOptions {
    /// Value written to the IDCODE register.
    pub idcode: u32,
    /// Value written to COR0.
    pub cor0: u32,
    /// Part name for the `.bit` `b` tag.
    pub part_name: String,
    /// Design source for the `.bit` `a` tag (`"<source>;Generator=<tool>"`).
    pub design_source: String,
    /// Build date for the `.bit` `c` tag (`YYYY/MM/DD`, UTC).
    pub date: String,
    /// Build time for the `.bit` `d` tag (`HH:MM:SS`, UTC).
    pub time: String,
}

impl Default for BitstreamOptions {
    fn default() -> Self {
        let (date, time) = utc_now_strings();
        BitstreamOptions {
            idcode: IDCODE_XC7A35T,
            cor0: COR0_DEFAULT,
            part_name: "xc7a35tcsg324-1".to_string(),
            design_source: ";Generator=helion-xilinx7".to_string(),
            date,
            time,
        }
    }
}

impl BitstreamOptions {
    /// Options reproducing `xc7frames2bit --part_name <part> --frm_file <frm>`
    /// for a fixed timestamp (used by the byte-compare test).
    pub fn xc7frames2bit(part_name: &str, frm_file: &str, date: &str, time: &str) -> Self {
        BitstreamOptions {
            idcode: IDCODE_XC7A35T,
            cor0: COR0_DEFAULT,
            part_name: part_name.to_string(),
            design_source: format!("{frm_file};Generator=xc7frames2bit"),
            date: date.to_string(),
            time: time.to_string(),
        }
    }
}

/// Current UTC date/time as (`YYYY/MM/DD`, `HH:MM:SS`), without external crates.
fn utc_now_strings() -> (String, String) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Howard Hinnant's civil-from-days algorithm.
    let days = secs.div_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let sod = secs.rem_euclid(86400);
    let (hh, mm, ss) = (sod / 3600, (sod / 60) % 60, sod % 60);
    (
        format!("{year:04}/{m:02}/{d:02}"),
        format!("{hh:02}:{mm:02}:{ss:02}"),
    )
}

/// Build the full Series-7 packet stream (preamble + configuration packets)
/// as 32-bit words, replicating `xc7frames2bit`.
///
/// `payload_words` is the FDRI frame payload from
/// [`crate::frames::FrameStore::fdri_payload`] (547,420 words for xc7a35t).
pub fn build_packet_stream(
    payload_words: &[u32],
    opts: &BitstreamOptions,
) -> Result<Vec<u32>, BitstreamError> {
    let mut w = Vec::with_capacity(payload_words.len() + 600);

    // Preamble: UG470 bus-width auto-detect + sync word.
    w.extend_from_slice(&[0xFFFF_FFFF; 8]);
    w.extend_from_slice(&[
        0x0000_00BB,
        0x1122_0044,
        0xFFFF_FFFF,
        0xFFFF_FFFF,
        0xAA99_5566,
    ]);

    // Local helpers: push a NOP, or a Type-1 single-word register write.
    macro_rules! nop {
        () => {
            w.push(type1_header(Op::Nop, reg::CRC, 0)?)
        };
    }
    macro_rules! write_reg {
        ($reg:expr, $val:expr) => {
            w.push(type1_header(Op::Write, $reg, 1)?);
            w.push($val);
        };
    }
    macro_rules! write_cmd {
        ($cmd:expr) => {
            write_reg!(reg::CMD, $cmd)
        };
    }
    macro_rules! nops {
        ($n:expr) => {
            for _ in 0..$n {
                nop!();
            }
        };
    }

    // Initialization sequence.
    nop!();
    write_reg!(reg::TIMER, 0x0);
    write_reg!(reg::WBSTAR, 0x0);
    write_cmd!(cmd::NOP);
    nop!();
    write_cmd!(cmd::RCRC);
    nops!(2);
    write_reg!(reg::UNKNOWN, 0x0);
    write_reg!(reg::COR0, opts.cor0);
    write_reg!(reg::COR1, 0x0);
    write_reg!(reg::IDCODE, opts.idcode);
    write_cmd!(cmd::SWITCH);
    nop!();
    write_reg!(reg::MASK, 0x401);
    write_reg!(reg::CTL0, 0x501);
    write_reg!(reg::MASK, 0x0);
    write_reg!(reg::CTL1, 0x0);
    nops!(8);
    write_reg!(reg::FAR, 0x0);
    write_cmd!(cmd::WCFG);
    nop!();

    // Frame data write: Type-1 FDRI header with count 0, then the Type-2
    // packet carrying the whole payload.
    w.push(type1_header(Op::Write, reg::FDRI, 0)?);
    w.push(type2_header(Op::Write, payload_words.len() as u32)?);
    w.extend_from_slice(payload_words);

    // Finalization sequence.
    write_cmd!(cmd::RCRC);
    nops!(2);
    write_cmd!(cmd::GRESTORE);
    nop!();
    write_cmd!(cmd::LFRM);
    nops!(100);
    write_cmd!(cmd::START);
    nop!();
    write_reg!(reg::FAR, 0x03BE_0000);
    write_reg!(reg::MASK, 0x501);
    write_reg!(reg::CTL0, 0x501);
    write_cmd!(cmd::RCRC);
    nops!(2);
    write_cmd!(cmd::DESYNC);
    nops!(400);

    Ok(w)
}

/// Write the raw packet stream (`.bin`): big-endian 32-bit words.
pub fn write_bin(
    payload_words: &[u32],
    opts: &BitstreamOptions,
) -> Result<Vec<u8>, BitstreamError> {
    let words = build_packet_stream(payload_words, opts)?;
    let mut out = Vec::with_capacity(words.len() * 4);
    for word in words {
        out.extend_from_slice(&word.to_be_bytes());
    }
    Ok(out)
}

/// Write a `.bit` file: TLV envelope + big-endian packet stream.
///
/// Layout: magic `00 09 0F F0 0F F0 0F F0 0F F0 00 00 01`, then tag `a`
/// (design source), `b` (part), `c` (date), `d` (time) each as
/// `u16be(len+1) bytes 0x00`, then tag `e` with `u32be(data_len)` and the
/// packet-stream bytes.
pub fn write_bit(
    payload_words: &[u32],
    opts: &BitstreamOptions,
) -> Result<Vec<u8>, BitstreamError> {
    fn push_string(out: &mut Vec<u8>, tag: u8, s: &str) -> Result<(), BitstreamError> {
        let bytes = s.as_bytes();
        // The reference includes the NUL terminator in the length.
        let len = bytes.len().checked_add(1).ok_or_else(|| BitstreamError {
            message: "TLV string too long".to_string(),
        })?;
        if len > 0xFFFF {
            return Err(BitstreamError {
                message: "TLV string too long".to_string(),
            });
        }
        out.push(tag);
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.extend_from_slice(bytes);
        out.push(0x00);
        Ok(())
    }

    let mut out = vec![
        0x00, 0x09, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x00, 0x00, 0x01,
    ];
    push_string(&mut out, b'a', &opts.design_source)?;
    push_string(&mut out, b'b', &opts.part_name)?;
    push_string(&mut out, b'c', &opts.date)?;
    push_string(&mut out, b'd', &opts.time)?;

    let stream = write_bin(payload_words, opts)?;
    out.push(b'e');
    out.extend_from_slice(&(stream.len() as u32).to_be_bytes());
    out.extend_from_slice(&stream);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_header_encoding() {
        // Type-1: [001][op:2][reg:14][rsvd:2][count:11]; reg at bits 26:13.
        assert_eq!(type1_header(Op::Nop, reg::CRC, 0).unwrap(), 0x2000_0000);
        assert_eq!(
            type1_header(Op::Write, reg::IDCODE, 1).unwrap(),
            0x3001_8001
        );
        assert_eq!(
            type1_header(Op::Write, reg::FDRI, 0).unwrap(),
            0x3000_4000
        );
        assert_eq!(
            type1_header(Op::Write, reg::CTL1, 1).unwrap(),
            0x3003_0001
        );
        // Type-2: [010][op:2][count:27].
        assert_eq!(type2_header(Op::Write, 547_420).unwrap(), 0x5008_5A5C);
        // Bounds are enforced, not wrapped.
        assert!(type1_header(Op::Write, reg::IDCODE, 0x800).is_err());
        assert!(type1_header(Op::Write, 0x4000, 1).is_err());
        assert!(type2_header(Op::Write, 0x0800_0000).is_err());
    }

    #[test]
    fn cor0_default_matches_prjxray() {
        // Pipeline DONE-in (bit 25), DONE release phase 4 (bits 14:12),
        // no DCI/MMCM stall waits (bits 11:6 = 0b111111), GTS phase 5
        // (bits 5:3), GWE phase 6 (bits 2:0).
        assert_eq!(COR0_DEFAULT, 0x0200_3FE5);
    }

    #[test]
    fn packet_stream_shape() {
        let opts = BitstreamOptions::default();
        let payload = vec![0xDEAD_BEEFu32; 2 * WORDS_PER_FRAME];
        let words = build_packet_stream(&payload, &opts).unwrap();

        // Preamble: 8xFF, BB, 11220044, 2xFF, sync.
        assert_eq!(&words[0..8], &[0xFFFF_FFFF; 8]);
        assert_eq!(words[8], 0x0000_00BB);
        assert_eq!(words[9], 0x1122_0044);
        assert_eq!(words[10], 0xFFFF_FFFF);
        assert_eq!(words[11], 0xFFFF_FFFF);
        assert_eq!(words[12], 0xAA99_5566);

        // First packets: NOP, TIMER=0, WBSTAR=0, CMD=NOP.
        assert_eq!(words[13], 0x2000_0000);
        assert_eq!(words[14], 0x3002_2001);
        assert_eq!(words[15], 0x0000_0000);
        assert_eq!(words[16], 0x3002_0001);

        // IDCODE write carries the part's IDCODE.
        let idcode_pos = words
            .windows(2)
            .position(|w| w[0] == 0x3001_8001)
            .expect("IDCODE packet");
        assert_eq!(words[idcode_pos + 1], IDCODE_XC7A35T);
        assert_eq!(IDCODE_XC7A35T, 0x0362_D093);

        // COR0 write carries 0x02003FE5.
        let cor0_pos = words
            .windows(2)
            .position(|w| w[0] == 0x3001_2001)
            .expect("COR0 packet");
        assert_eq!(words[cor0_pos + 1], 0x0200_3FE5);

        // FDRI Type-1 (count 0) then Type-2 with the payload length.
        let fdri1 = words.iter().position(|&w| w == 0x3000_4000).unwrap();
        assert_eq!(words[fdri1 + 1], 0x5000_0000 | payload.len() as u32);
        assert_eq!(
            &words[fdri1 + 2..fdri1 + 2 + payload.len()],
            payload.as_slice()
        );

        // Trailer: RCRC, GRESTORE, LFRM, START, FAR=0x03BE0000, RCRC, DESYNC.
        let tail = &words[fdri1 + 2 + payload.len()..];
        let cmds: Vec<u32> = tail
            .windows(2)
            .filter(|w| w[0] == 0x3000_8001)
            .map(|w| w[1])
            .collect();
        assert_eq!(
            cmds,
            vec![
                cmd::RCRC,
                cmd::GRESTORE,
                cmd::LFRM,
                cmd::START,
                cmd::RCRC,
                cmd::DESYNC
            ]
        );
        // FAR=0x03BE0000 shutdown write is present.
        assert!(tail
            .windows(2)
            .any(|w| w[0] == 0x3000_2001 && w[1] == 0x03BE_0000));
        // Ends with 400 NOPs.
        assert_eq!(words.len() - 400, words.iter().rposition(|&w| w != 0x2000_0000).unwrap() + 1);
        assert!(words[words.len() - 400..].iter().all(|&w| w == 0x2000_0000));
    }

    #[test]
    fn bit_envelope_layout() {
        let opts = BitstreamOptions::xc7frames2bit(
            "xc7a35tcsg324-1",
            "test.frm",
            "2026/10/12",
            "04:00:00",
        );
        let payload = vec![0u32; WORDS_PER_FRAME];
        let bit = write_bit(&payload, &opts).unwrap();

        // Magic.
        let magic: &[u8] = &[
            0x00, 0x09, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x0f, 0xf0, 0x00, 0x00, 0x01,
        ];
        assert_eq!(&bit[..13], magic);
        // Tag 'a' with "<frm>;Generator=xc7frames2bit\0".
        let mut pos = 13;
        assert_eq!(bit[pos], b'a');
        let alen = u16::from_be_bytes([bit[pos + 1], bit[pos + 2]]) as usize;
        let a = &bit[pos + 3..pos + 3 + alen];
        assert_eq!(a, b"test.frm;Generator=xc7frames2bit\0");
        pos += 3 + alen;
        // Tag 'b' with the part name.
        assert_eq!(bit[pos], b'b');
        let blen = u16::from_be_bytes([bit[pos + 1], bit[pos + 2]]) as usize;
        assert_eq!(&bit[pos + 3..pos + 3 + blen], b"xc7a35tcsg324-1\0");
        pos += 3 + blen;
        // Tags 'c' and 'd' carry the fixed date/time.
        assert_eq!(bit[pos], b'c');
        let clen = u16::from_be_bytes([bit[pos + 1], bit[pos + 2]]) as usize;
        assert_eq!(&bit[pos + 3..pos + 3 + clen], b"2026/10/12\0");
        pos += 3 + clen;
        assert_eq!(bit[pos], b'd');
        let dlen = u16::from_be_bytes([bit[pos + 1], bit[pos + 2]]) as usize;
        assert_eq!(&bit[pos + 3..pos + 3 + dlen], b"04:00:00\0");
        pos += 3 + dlen;
        // Tag 'e': u32be data length, then the packet stream.
        assert_eq!(bit[pos], b'e');
        let elen = u32::from_be_bytes([bit[pos + 1], bit[pos + 2], bit[pos + 3], bit[pos + 4]])
            as usize;
        let data = &bit[pos + 5..];
        assert_eq!(data.len(), elen);
        assert_eq!(&data[..4], &[0xFF, 0xFF, 0xFF, 0xFF]); // stream preamble
        assert_eq!(data.len() % 4, 0);
    }

    #[test]
    fn bin_is_stream_only() {
        let opts = BitstreamOptions::default();
        let payload = vec![0u32; WORDS_PER_FRAME];
        let bin = write_bin(&payload, &opts).unwrap();
        let words = build_packet_stream(&payload, &opts).unwrap();
        assert_eq!(bin.len(), words.len() * 4);
        assert_eq!(&bin[..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(u32::from_be_bytes(bin[48..52].try_into().unwrap()), 0xAA99_5566);
    }

    #[test]
    fn utc_strings_are_sane() {
        let (d, t) = utc_now_strings();
        assert_eq!(d.len(), 10);
        assert_eq!(t.len(), 8);
        assert!(d.starts_with("2026/"));
    }
}
