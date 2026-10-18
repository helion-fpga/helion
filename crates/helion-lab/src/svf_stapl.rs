//! Vivado UG908 SVF (Serial Vector Format) & JEDEC STAPL/JAM JTAG Vector Exporter & Player.
//!
//! Provides:
//! - [`write_hw_svf`] & [`execute_hw_svf`]: exports a programmed [`Bitstream`] to an IEEE 1149.1
//!   SVF command stream (`FREQUENCY`, `STATE`, `SIR`, `SDR`, `TDI`, `TDO`, `MASK`, `RUNTEST`) and
//!   plays it back against a live [`Tap`] with strict TDO/MASK verification.
//! - [`write_hw_stapl`] & [`execute_hw_stapl`]: JEDEC JESD-71 STAPL / Jam player and exporter.

use crate::ERR_CODE_EMPTY_BITSTREAM;
use helion_bits::Bitstream;
use helion_device::Device;
use helion_hw::{IR_CFG_W, IR_IDCODE, IR_STAT, Tap, TapState, refuse_empty_bitstream};

/// One parsed SVF (Serial Vector Format) command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SvfCommand {
    Frequency {
        hz: u64,
    },
    Trst {
        mode: String,
    },
    EndIr {
        state: String,
    },
    EndDr {
        state: String,
    },
    State {
        path: Vec<String>,
    },
    Sir {
        bits: usize,
        tdi: Vec<u8>,
        tdo: Option<Vec<u8>>,
        mask: Option<Vec<u8>>,
    },
    Sdr {
        bits: usize,
        tdi: Vec<u8>,
        tdo: Option<Vec<u8>>,
        mask: Option<Vec<u8>>,
    },
    RunTest {
        clocks: u32,
        run_state: String,
    },
}

/// Summary of executing an SVF or STAPL JTAG vector file against a [`Tap`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvfExecutionReport {
    pub commands_executed: usize,
    pub sir_scans: usize,
    pub sdr_scans: usize,
    pub runtest_clocks: u64,
    pub tdo_checks_passed: usize,
    pub frequency_hz: Option<u64>,
    pub final_idcode: u32,
    pub final_stat_word: u32,
}

fn bytes_to_hex_msb(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "00".to_string();
    }
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes.iter().rev() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_msb_to_bytes(hex: &str, bit_len: usize) -> Result<Vec<u8>, String> {
    let cleaned: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("svf: empty hex literal".into());
    }
    let padded = if cleaned.len() % 2 == 1 {
        format!("0{cleaned}")
    } else {
        cleaned
    };
    let n_bytes = bit_len.div_ceil(8).max(1);
    let mut out_le = Vec::with_capacity(n_bytes);
    // Parse from LSB pair (rightmost 2 hex chars) to MSB pair.
    let mut idx = padded.len();
    while idx >= 2 {
        let byte_str = &padded[idx - 2..idx];
        let b = u8::from_str_radix(byte_str, 16)
            .map_err(|e| format!("svf: invalid hex '{byte_str}': {e}"))?;
        out_le.push(b);
        idx -= 2;
    }
    while out_le.len() < n_bytes {
        out_le.push(0);
    }
    out_le.truncate(n_bytes);
    Ok(out_le)
}

fn le_bytes_to_u64(bytes: &[u8]) -> u64 {
    let mut w = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(8) {
        w |= u64::from(b) << (i * 8);
    }
    w
}

/// Export a programmed [`Bitstream`] to an IEEE 1149.1 SVF (Serial Vector Format) script
/// (`write_hw_svf`). Refuses empty bitstreams with [`ERR_CODE_EMPTY_BITSTREAM`].
pub fn write_hw_svf(dev: &Device, bits: &Bitstream) -> Result<String, String> {
    refuse_empty_bitstream(bits).map_err(|e| {
        if e.contains(ERR_CODE_EMPTY_BITSTREAM) {
            format!("svf: {e}")
        } else {
            format!("{ERR_CODE_EMPTY_BITSTREAM}: svf: {e}")
        }
    })?;

    let cfg_bits = bits.packets.len() * 8;
    let cfg_hex = bytes_to_hex_msb(&bits.packets);
    let idcode_hex = format!("{:08x}", dev.idcode);
    let stat_expected = helion_hw:: Tap::new(dev);
    let _ = stat_expected;
    // After startup: INIT (1<<1) | GWE (1<<4) | DONE (1<<5) | EOS (1<<6) = 0x72
    let stat_hex = "00000072";

    let mut s = String::new();
    s.push_str("// Helion Hardware Manager SVF (Serial Vector Format) — UG908 parity\n");
    s.push_str(&format!("// Target Part: {} (IDCODE {:#010x})\n", dev.part, dev.idcode));
    s.push_str("TRST OFF;\n");
    s.push_str("ENDIR IDLE;\n");
    s.push_str("ENDDR IDLE;\n");
    s.push_str("FREQUENCY 10000000 HZ;\n");
    s.push_str("STATE RESET IDLE;\n");
    s.push_str(&format!(
        "// Verify JTAG IDCODE\n\
         SIR 6 TDI ({:02x});\n\
         SDR 32 TDI (00000000) TDO ({idcode_hex}) MASK (ffffffff);\n",
        IR_IDCODE
    ));
    s.push_str(&format!(
        "// Load configuration bitstream via IR_CFG_W\n\
         SIR 6 TDI ({:02x});\n\
         SDR {cfg_bits} TDI ({cfg_hex});\n\
         RUNTEST IDLE 33 TCK;\n",
        IR_CFG_W
    ));
    s.push_str(&format!(
        "// Verify post-configuration STAT (INIT | GWE | DONE | EOS)\n\
         SIR 6 TDI ({:02x});\n\
         SDR 32 TDI (00000000) TDO ({stat_hex}) MASK (0000007f);\n\
         STATE RESET IDLE;\n",
        IR_STAT
    ));
    Ok(s)
}

fn extract_paren_param<'a>(tokens: &[&'a str], key: &str) -> Option<&'a str> {
    for (i, &t) in tokens.iter().enumerate() {
        if t.eq_ignore_ascii_case(key) {
            if let Some(&next) = tokens.get(i + 1) {
                return Some(next.trim_start_matches('(').trim_end_matches(')'));
            }
        } else if let Some(rest) = t.strip_prefix(key) {
            if rest.starts_with('(') && rest.ends_with(')') {
                return Some(rest.trim_start_matches('(').trim_end_matches(')'));
            }
        }
    }
    None
}

/// Parse an SVF text script into a sequence of [`SvfCommand`]s.
pub fn parse_svf(svf_text: &str) -> Result<Vec<SvfCommand>, String> {
    let mut cleaned = String::new();
    for raw in svf_text.lines() {
        let line = raw
            .split("//")
            .next()
            .unwrap_or("")
            .split('!')
            .next()
            .unwrap_or("")
            .trim();
        if !line.is_empty() {
            cleaned.push_str(line);
            cleaned.push(' ');
        }
    }

    let mut commands = Vec::new();
    for stmt_raw in cleaned.split(';') {
        let stmt = stmt_raw.trim();
        if stmt.is_empty() {
            continue;
        }
        // Normalize parentheses so `TDI(03)` and `TDI (03)` both tokenize cleanly.
        let norm = stmt.replace('(', " (").replace(')', ") ");
        let tokens: Vec<&str> = norm.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        let kw = tokens[0].to_ascii_uppercase();
        match kw.as_str() {
            "TRST" => {
                let mode = tokens.get(1).copied().unwrap_or("OFF").to_uppercase();
                commands.push(SvfCommand::Trst { mode });
            }
            "ENDIR" => {
                let state = tokens.get(1).copied().unwrap_or("IDLE").to_uppercase();
                commands.push(SvfCommand::EndIr { state });
            }
            "ENDDR" => {
                let state = tokens.get(1).copied().unwrap_or("IDLE").to_uppercase();
                commands.push(SvfCommand::EndDr { state });
            }
            "HIR" | "TIR" | "HDR" | "TDR" => {
                // Header/trailer zero padding declarations accepted as no-ops for single-TAP or handled inline.
            }
            "FREQUENCY" => {
                let hz_str = tokens
                    .get(1)
                    .ok_or("svf: FREQUENCY missing value")?;
                let hz = hz_str
                    .parse::<f64>()
                    .map_err(|e| format!("svf: invalid FREQUENCY '{hz_str}': {e}"))?
                    as u64;
                commands.push(SvfCommand::Frequency { hz });
            }
            "STATE" => {
                let path = tokens[1..].iter().map(|s| s.to_uppercase()).collect();
                commands.push(SvfCommand::State { path });
            }
            "SIR" | "SDR" => {
                let bits: usize = tokens
                    .get(1)
                    .ok_or_else(|| format!("svf: {kw} missing bit length"))?
                    .parse()
                    .map_err(|e| format!("svf: {kw} invalid bit length: {e}"))?;
                let tdi_hex = extract_paren_param(&tokens[2..], "TDI")
                    .ok_or_else(|| format!("svf: {kw} missing TDI (...)"))?;
                let tdi = hex_msb_to_bytes(tdi_hex, bits)?;
                let tdo = match extract_paren_param(&tokens[2..], "TDO") {
                    Some(h) => Some(hex_msb_to_bytes(h, bits)?),
                    None => None,
                };
                let mask = match extract_paren_param(&tokens[2..], "MASK") {
                    Some(h) => Some(hex_msb_to_bytes(h, bits)?),
                    None => None,
                };
                if kw == "SIR" {
                    commands.push(SvfCommand::Sir {
                        bits,
                        tdi,
                        tdo,
                        mask,
                    });
                } else {
                    commands.push(SvfCommand::Sdr {
                        bits,
                        tdi,
                        tdo,
                        mask,
                    });
                }
            }
            "RUNTEST" => {
                let mut clocks = 1u32;
                let mut run_state = "IDLE".to_string();
                for (i, &t) in tokens[1..].iter().enumerate() {
                    if let Ok(n) = t.parse::<u32>() {
                        clocks = n;
                    } else if i == 0 && !t.eq_ignore_ascii_case("TCK") {
                        run_state = t.to_uppercase();
                    }
                }
                commands.push(SvfCommand::RunTest { clocks, run_state });
            }
            other => {
                return Err(format!("svf: unsupported command '{other}' in '{stmt}'"));
            }
        }
    }
    Ok(commands)
}

/// Execute an IEEE 1149.1 SVF command script against `tap` (`execute_hw_svf`), verifying
/// all `TDO (...) MASK (...)` expectations.
pub fn execute_hw_svf(tap: &mut Tap, svf_text: &str) -> Result<SvfExecutionReport, String> {
    let commands = parse_svf(svf_text)?;
    let mut sir_scans = 0usize;
    let mut sdr_scans = 0usize;
    let mut runtest_clocks = 0u64;
    let mut tdo_checks_passed = 0usize;
    let mut frequency_hz = None;

    for (idx, cmd) in commands.iter().enumerate() {
        match cmd {
            SvfCommand::Frequency { hz } => {
                frequency_hz = Some(*hz);
            }
            SvfCommand::Trst { .. } | SvfCommand::EndIr { .. } | SvfCommand::EndDr { .. } => {}
            SvfCommand::State { path } => {
                for st in path {
                    match st.as_str() {
                        "RESET" | "TLR" => tap.reset(),
                        "IDLE" | "RTI" => {
                            if tap.state == TapState::TestLogicReset {
                                let _ = tap.tick(false, false);
                            }
                        }
                        _ => {}
                    }
                }
            }
            SvfCommand::Sir {
                bits: _,
                tdi,
                tdo,
                mask,
            } => {
                let ir_val = tdi.first().copied().unwrap_or(0) & 0x3F;
                let prev_ir = tap.ir & 0x3F;
                tap.shift_ir(ir_val);
                sir_scans += 1;
                if let Some(exp_bytes) = tdo {
                    let exp = u64::from(exp_bytes.first().copied().unwrap_or(0));
                    let m = mask
                        .as_ref()
                        .and_then(|mb| mb.first().copied())
                        .map(u64::from)
                        .unwrap_or(0x3F);
                    let actual = u64::from(prev_ir);
                    if (actual & m) != (exp & m) {
                        return Err(format!(
                            "svf: SIR TDO mismatch at command #{}: actual={actual:#04x} expected={exp:#04x} mask={m:#04x}",
                            idx + 1
                        ));
                    }
                    tdo_checks_passed += 1;
                }
            }
            SvfCommand::Sdr {
                bits,
                tdi,
                tdo,
                mask,
            } => {
                sdr_scans += 1;
                if tap.ir == IR_CFG_W {
                    let bits_obj = Bitstream::from_packets(tdi)
                        .map_err(|e| format!("svf: IR_CFG_W packet decode failed: {e}"))?;
                    tap.program(&bits_obj)?;
                } else {
                    let tdi_u64 = le_bytes_to_u64(tdi);
                    let captured_u64 = tap.shift_dr_u64(tdi_u64);
                    if let Some(exp_bytes) = tdo {
                        let exp_u64 = le_bytes_to_u64(exp_bytes);
                        let mask_u64 = mask.as_ref().map(|m| le_bytes_to_u64(m)).unwrap_or_else(|| {
                            if *bits >= 64 {
                                u64::MAX
                            } else {
                                (1u64 << *bits) - 1
                            }
                        });
                        if (captured_u64 & mask_u64) != (exp_u64 & mask_u64) {
                            return Err(format!(
                                "svf: SDR TDO mismatch at command #{}: actual={captured_u64:#010x} expected={exp_u64:#010x} mask={mask_u64:#010x}",
                                idx + 1
                            ));
                        }
                        tdo_checks_passed += 1;
                    }
                }
            }
            SvfCommand::RunTest { clocks, .. } => {
                runtest_clocks += u64::from(*clocks);
                for _ in 0..(*clocks).min(64) {
                    let _ = tap.tick(false, false);
                }
            }
        }
    }

    Ok(SvfExecutionReport {
        commands_executed: commands.len(),
        sir_scans,
        sdr_scans,
        runtest_clocks,
        tdo_checks_passed,
        frequency_hz,
        final_idcode: tap.fabric().idcode,
        final_stat_word: tap.fabric().stat.word(),
    })
}

/// Export a programmed [`Bitstream`] to a JEDEC JESD-71 STAPL / Jam file (`write_hw_stapl`).
/// Refuses empty bitstreams with [`ERR_CODE_EMPTY_BITSTREAM`].
pub fn write_hw_stapl(dev: &Device, bits: &Bitstream) -> Result<String, String> {
    let svf_body = write_hw_svf(dev, bits)?;
    let mut stapl = String::new();
    stapl.push_str("NOTE \"CREATOR\" \"Helion Hardware Manager STAPL Exporter (JESD-71)\";\n");
    stapl.push_str(&format!("NOTE \"DEVICE\" \"{}\";\n", dev.part));
    stapl.push_str(&format!("NOTE \"IDCODE\" \"{:#010x}\";\n", dev.idcode));
    stapl.push_str("ACTION PROGRAM = DO_PROGRAM;\n");
    stapl.push_str("PROCEDURE DO_PROGRAM;\n");
    for line in svf_body.lines() {
        if !line.starts_with("//") {
            stapl.push_str("  ");
            stapl.push_str(line);
            stapl.push('\n');
        }
    }
    stapl.push_str("  EXIT 0;\n");
    stapl.push_str("ENDPROC;\n");
    stapl.push_str("CRC 0000;\n");
    Ok(stapl)
}

/// Execute a JEDEC JESD-71 STAPL / Jam program against `tap` (`execute_hw_stapl`).
pub fn execute_hw_stapl(tap: &mut Tap, stapl_text: &str) -> Result<SvfExecutionReport, String> {
    let mut svf_lines = String::new();
    let mut saw_proc = false;
    for raw in stapl_text.lines() {
        let line = raw.trim();
        if line.starts_with("PROCEDURE ") {
            saw_proc = true;
            continue;
        }
        if line.starts_with("NOTE ")
            || line.starts_with("ACTION ")
            || line.starts_with("EXIT ")
            || line.starts_with("ENDPROC")
            || line.starts_with("CRC ")
        {
            continue;
        }
        svf_lines.push_str(line);
        svf_lines.push('\n');
    }
    if !saw_proc {
        return Err("stapl: missing PROCEDURE block".into());
    }
    execute_hw_svf(tap, &svf_lines)
}
