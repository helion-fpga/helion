//! Line-oriented parsers for the Project X-Ray text database files.
//!
//! Covers `segbits_<tile>.db`, `mask_<tile>.db`, `ppips_<tile>.db` and
//! `package_pins.csv`. All parsers return `Result<_, String>` and never panic;
//! `Ok(None)` means "blank/comment/ignorable line".

/// One configuration bit from a segbits line.
///
/// `frame_offset` is relative to the tile's configuration bus base address;
/// `bitpos` is the bit index within the frame's word window. `value == false`
/// means the bit is cleared (`!` prefix in the DB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegBit {
    pub frame_offset: u32,
    pub bitpos: u32,
    pub value: bool,
}

/// Parse a single segbits bit token: `[!]frame_offset_bitpos`, e.g. `07_32`
/// or `!22_07`.
pub fn parse_segbit_token(tok: &str) -> Result<SegBit, String> {
    let (tok, value) = match tok.strip_prefix('!') {
        Some(rest) => (rest, false),
        None => (tok, true),
    };
    let (fo, bp) = tok
        .split_once('_')
        .ok_or_else(|| format!("segbits: bad bit token '{tok}'"))?;
    let frame_offset: u32 = fo
        .parse()
        .map_err(|_| format!("segbits: bad frame offset '{fo}' in '{tok}'"))?;
    let bitpos: u32 = bp
        .parse()
        .map_err(|_| format!("segbits: bad bitpos '{bp}' in '{tok}'"))?;
    Ok(SegBit { frame_offset, bitpos, value })
}

/// Parse one `segbits_<tile>.db` line: `<FEATURE> <bit>...`.
///
/// Returns `Ok(None)` for blank lines, `#` comments, and special features
/// (`<const0>`, `<const1>`, `<0 candidates, ...>`) which carry no bits.
pub fn parse_segbits_line(line: &str) -> Result<Option<(String, Vec<SegBit>)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let feature = parts
        .next()
        .ok_or_else(|| "segbits: empty line after trim".to_string())?;
    if feature.starts_with('<') {
        return Ok(None);
    }
    let mut bits = Vec::new();
    for tok in parts {
        if tok.starts_with('<') {
            // A `<...>` trailer (e.g. `<0 candidates, ...>`) ends the bit list;
            // the annotation itself may contain spaces.
            break;
        }
        bits.push(parse_segbit_token(tok)?);
    }
    Ok(Some((feature.to_string(), bits)))
}

/// Parse one `mask_<tile>.db` line: `bit frame_offset_bitpos`.
///
/// Returns `Ok(None)` for blank lines and `#` comments.
pub fn parse_mask_line(line: &str) -> Result<Option<(u32, u32)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let tok = line
        .strip_prefix("bit ")
        .ok_or_else(|| format!("mask: expected 'bit NN_MM', got '{line}'"))?
        .trim();
    let bit = parse_segbit_token(tok)?;
    Ok(Some((bit.frame_offset, bit.bitpos)))
}

/// Pseudo-pip kind from `ppips_<tile>.db`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PpipKind {
    Always,
    Default,
    Hint,
}

/// One pseudo-pip entry: `<tile>.<dst>.<src> <kind>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ppip {
    /// Full dotted name, e.g. `INT_L.BYP_ALT0.VCC_WIRE`.
    pub name: String,
    pub kind: PpipKind,
}

/// Parse one `ppips_<tile>.db` line.
///
/// Returns `Ok(None)` for blank lines and `#` comments.
pub fn parse_ppips_line(line: &str) -> Result<Option<Ppip>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (name, kind) = line
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("ppips: bad line '{line}'"))?;
    let kind = kind.trim();
    let kind = match kind {
        "always" => PpipKind::Always,
        "default" => PpipKind::Default,
        "hint" => PpipKind::Hint,
        _ => return Err(format!("ppips: unknown kind '{kind}' in '{line}'")),
    };
    Ok(Some(Ppip { name: name.to_string(), kind }))
}

/// One row of `package_pins.csv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagePin {
    /// Package pin name, e.g. `E3`.
    pub pin: String,
    pub bank: String,
    /// Site name, e.g. `IOB_X1Y76`.
    pub site: String,
    /// Tile name, e.g. `RIOB33_X43Y75`.
    pub tile: String,
    /// Xilinx pin function, e.g. `IO_L12P_T1_MRCC_35`.
    pub pin_function: String,
}

/// Parse the whole `package_pins.csv` text (header + rows).
///
/// The header must be `pin,bank,site,tile,pin_function` (extra columns are
/// ignored; missing columns are an error).
pub fn parse_package_pins(text: &str) -> Result<Vec<PackagePin>, String> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| "package_pins.csv: empty file".to_string())?;
    let cols: Vec<&str> = header.split(',').map(str::trim).collect();
    let need = ["pin", "bank", "site", "tile", "pin_function"];
    let mut idx = [0usize; 5];
    for (i, want) in need.iter().enumerate() {
        idx[i] = cols
            .iter()
            .position(|c| c == want)
            .ok_or_else(|| format!("package_pins.csv: missing column '{want}'"))?;
    }
    let mut out = Vec::new();
    for (lineno, line) in lines.enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let get = |i: usize| -> Result<&str, String> {
            f.get(idx[i])
                .copied()
                .ok_or_else(|| format!("package_pins.csv:{}: short row", lineno + 2))
        };
        out.push(PackagePin {
            pin: get(0)?.to_string(),
            bank: get(1)?.to_string(),
            site: get(2)?.to_string(),
            tile: get(3)?.to_string(),
            pin_function: get(4)?.to_string(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segbits_real_lines() {
        let (f, bits) =
            parse_segbits_line("INT_L.BYP_ALT0.BYP_BOUNCE_N3_3 21_07 !22_07 !23_07 24_07 !25_07")
                .unwrap()
                .unwrap();
        assert_eq!(f, "INT_L.BYP_ALT0.BYP_BOUNCE_N3_3");
        assert_eq!(bits.len(), 5);
        assert_eq!(bits[0], SegBit { frame_offset: 21, bitpos: 7, value: true });
        assert_eq!(bits[1], SegBit { frame_offset: 22, bitpos: 7, value: false });
        assert_eq!(bits[4], SegBit { frame_offset: 25, bitpos: 7, value: false });

        let (f, bits) =
            parse_segbits_line("CLBLL_L.SLICEL_X0.AFFMUX.AX !30_00 30_01 !30_02 !30_03")
                .unwrap()
                .unwrap();
        assert_eq!(f, "CLBLL_L.SLICEL_X0.AFFMUX.AX");
        assert_eq!(bits.len(), 4);
        assert!(!bits[0].value && bits[1].value);

        assert!(parse_segbits_line("").unwrap().is_none());
        assert!(parse_segbits_line("   ").unwrap().is_none());
        assert!(parse_segbits_line("# comment").unwrap().is_none());
        assert!(parse_segbits_line("<const0>").unwrap().is_none());
        assert!(parse_segbits_line("FOO <0 candidates, 1>").unwrap().unwrap().1.is_empty());
        assert!(parse_segbits_line("BADTOKEN xx").is_err());
    }

    #[test]
    fn mask_real_lines() {
        assert_eq!(parse_mask_line("bit 00_00").unwrap(), Some((0, 0)));
        assert_eq!(parse_mask_line("bit 07_32").unwrap(), Some((7, 32)));
        assert!(parse_mask_line("").unwrap().is_none());
        assert!(parse_mask_line("junk").is_err());
    }

    #[test]
    fn ppips_real_lines() {
        let p = parse_ppips_line("INT_L.BYP_ALT0.VCC_WIRE default").unwrap().unwrap();
        assert_eq!(p.name, "INT_L.BYP_ALT0.VCC_WIRE");
        assert_eq!(p.kind, PpipKind::Default);
        let p = parse_ppips_line("CLBLL_L.SLICEL_X0.ALUT.INIT[00] always").unwrap().unwrap();
        assert_eq!(p.kind, PpipKind::Always);
        let p = parse_ppips_line("INT_L.FOO.BAR hint").unwrap().unwrap();
        assert_eq!(p.kind, PpipKind::Hint);
        assert!(parse_ppips_line("").unwrap().is_none());
        assert!(parse_ppips_line("INT_L.A.B bogus").is_err());
    }

    #[test]
    fn package_pins_real_rows() {
        let text = "pin,bank,site,tile,pin_function\n\
                    E3,35,IOB_X1Y76,RIOB33_X43Y75,IO_L12P_T1_MRCC_35\n\
                    H5,35,IOB_X1Y51,RIOB33_X43Y51,IO_L24N_T3_35\n";
        let pins = parse_package_pins(text).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].pin, "E3");
        assert_eq!(pins[0].site, "IOB_X1Y76");
        assert_eq!(pins[0].tile, "RIOB33_X43Y75");
        assert_eq!(pins[0].bank, "35");
        assert_eq!(pins[0].pin_function, "IO_L12P_T1_MRCC_35");
        assert_eq!(pins[1].pin, "H5");
        assert!(parse_package_pins("pin,bank,site\nE3,35,X\n").is_err());
    }
}
