//! FASM (FPGA Assembly) text parser.
//!
//! Parses the line-oriented FASM format used across the open 7-series flow,
//! e.g.
//!
//! ```text
//! # a comment runs to end of line
//! INT_L_X10Y146.IMUX_L1.N1
//! INT_L_X10Y146.IMUX_L1.N1 = 0
//! CLBLL_L_X12Y34.SLICEL_X0.ALUT.INIT[63]
//! CLBLL_L_X12Y34.SLICEL_X0.ALUT.INIT[63:0] = 64'hDEADBEEFCAFEBABE
//! ```
//!
//! Grammar per line: `TILE.FEATURE[address] = value`, where the `= value`
//! part is optional (an absent value means `1`), the address is `[bit]` or
//! `[hi:lo]` with `hi >= lo`, and the value is `0`, `1`, a plain decimal,
//! or a Verilog constant (`'b`, `'h`, `'d`, `'o`, with optional width).
//! A `#` starts a comment through end of line. `{...}` annotations, if
//! present, are skipped.
//!
//! The parsed form mirrors the `fasm` Python library's
//! [`SetFasmFeature`](https://github.com/chipsalliance/fasm) tuple
//! `(feature, start, end, value)` so that [`crate::frames`] can apply the
//! same canonicalization prjxray's `fasm2frames` uses: a missing value
//! means 1, an explicit width may not exceed the addressed width, and the
//! value must fit the addressed width.

use std::error::Error;
use std::fmt;

/// A FASM feature value: the integer assigned to the addressed bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FasmValue {
    /// `TILE.FEATURE`, `TILE.FEATURE = 1`, `TILE.FEATURE = 1'b1`, ...
    One,
    /// `TILE.FEATURE = 0`. Note: prjxray's `fasm2frames` treats a zero
    /// value as a no-op (its `canonical_features` drops it); see
    /// [`crate::frames::FrameStore::apply_fasm`].
    Zero,
    /// Any other constant, e.g. `64'hDEADBEEF`. `width` is the explicit
    /// Verilog width when given, otherwise the addressed width.
    Bits { bits: u64, width: u32 },
}

impl FasmValue {
    /// Integer value of this assignment (0 or 1 for [`FasmValue::One`]/[`FasmValue::Zero`]).
    pub fn as_u64(self) -> u64 {
        match self {
            FasmValue::One => 1,
            FasmValue::Zero => 0,
            FasmValue::Bits { bits, .. } => bits,
        }
    }
}

/// One parsed FASM line: `TILE.FEATURE[hi:lo] = value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FasmLine {
    /// Tile name, e.g. `INT_L_X10Y146` (up to the first `.`).
    pub tile: String,
    /// Feature path without any bit selection, e.g. `IMUX_L1.N1`.
    pub feature: String,
    /// Bit selection: `Some((hi, lo))`; a single `[i]` becomes `Some((i, i))`.
    pub bit_range: Option<(u32, u32)>,
    /// Assigned value (`One` when the `= value` part is absent).
    pub value: FasmValue,
}

/// A FASM parse failure, with the 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number in the parsed text.
    pub line: usize,
    /// Human-readable description of the problem.
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FASM parse error on line {}: {}", self.line, self.message)
    }
}

impl Error for ParseError {}

/// Parse whole FASM text into lines, skipping blanks and comments.
pub fn parse_fasm(text: &str) -> Result<Vec<FasmLine>, ParseError> {
    let mut out = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        if let Some(line) = parse_line(raw, idx + 1)? {
            out.push(line);
        }
    }
    Ok(out)
}

/// Parse one line; returns `Ok(None)` for blank/comment-only lines.
pub fn parse_line(raw: &str, lineno: usize) -> Result<Option<FasmLine>, ParseError> {
    let err = |message: String| ParseError { line: lineno, message };

    // Strip any {...} annotations first (a '#' inside a quoted annotation
    // value is not a comment starter).
    let no_annot = strip_annotations(raw)
        .map_err(|message| ParseError { line: lineno, message })?;
    // '#' starts a comment through end of line.
    let line = match no_annot.find('#') {
        Some(i) => &no_annot[..i],
        None => &no_annot,
    }
    .trim();
    if line.is_empty() {
        return Ok(None);
    }

    // Split off `= value`.
    let (target, value_text) = match line.find('=') {
        Some(i) => (line[..i].trim_end(), Some(line[i + 1..].trim_start())),
        None => (line, None),
    };
    if target.is_empty() {
        return Err(err("missing tile.feature target".to_string()));
    }

    // Tile is up to the first '.'; the rest is the dotted feature path.
    let dot = target
        .find('.')
        .ok_or_else(|| err(format!("missing '.' separating tile and feature in {target:?}")))?;
    let (tile, feature_path) = (&target[..dot], &target[dot + 1..]);
    check_identifier(tile).map_err(|e| err(format!("bad tile name {tile:?}: {e}")))?;
    if feature_path.is_empty() {
        return Err(err(format!("missing feature after tile {tile:?}")));
    }

    // Split off the trailing `[..]` bit selection, if any.
    let (feature, bit_range) = split_bit_range(feature_path)
        .map_err(|e| err(format!("bad feature address in {feature_path:?}: {e}")))?;
    for segment in feature.split('.') {
        check_identifier(segment)
            .map_err(|e| err(format!("bad feature segment {segment:?}: {e}")))?;
    }

    let address_width: u64 = match bit_range {
        None => 1,
        Some((hi, lo)) => u64::from(hi) - u64::from(lo) + 1,
    };
    let value = match value_text {
        None => FasmValue::One,
        Some(text) => parse_value(text, address_width)
            .map_err(|message| ParseError { line: lineno, message })?,
    };

    Ok(Some(FasmLine {
        tile: tile.to_string(),
        feature: feature.to_string(),
        bit_range,
        value,
    }))
}

/// Remove a `{...}` annotation span, honoring `"..."` quotes and `\` escapes.
fn strip_annotations(line: &str) -> Result<String, String> {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c != b'{' {
            out.push(c as char);
            i += 1;
            continue;
        }
        // Skip balanced braces.
        i += 1;
        let mut depth = 1usize;
        let mut in_string = false;
        while i < bytes.len() && depth > 0 {
            let d = bytes[i];
            if in_string {
                if d == b'\\' {
                    i += 1; // skip escaped char
                } else if d == b'"' {
                    in_string = false;
                }
            } else if d == b'"' {
                in_string = true;
            } else if d == b'{' {
                depth += 1;
            } else if d == b'}' {
                depth -= 1;
            }
            i += 1;
        }
        if depth > 0 {
            return Err("unterminated '{...}' annotation".to_string());
        }
    }
    Ok(out)
}

/// Identifier per the FASM grammar: `[a-zA-Z][a-zA-Z0-9_]*`.
fn check_identifier(s: &str) -> Result<(), &'static str> {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return Err("must start with an ASCII letter"),
    }
    if !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("must contain only ASCII letters, digits, '_'");
    }
    Ok(())
}

/// Split `FEATURE[hi:lo]` / `FEATURE[i]` into the bare feature and
/// `Some((hi, lo))`; returns `(feature, None)` when there is no `[..]`.
type SplitBitRange<'a> = (&'a str, Option<(u32, u32)>);

fn split_bit_range(feature_path: &str) -> Result<SplitBitRange<'_>, String> {
    let open = match feature_path.rfind('[') {
        Some(i) => i,
        None => return Ok((feature_path, None)),
    };
    if !feature_path.ends_with(']') {
        return Err("unbalanced '[' without closing ']'".to_string());
    }
    let feature = &feature_path[..open];
    if feature.is_empty() {
        return Err("missing feature name before '['".to_string());
    }
    let inner = &feature_path[open + 1..feature_path.len() - 1];
    let (hi_s, lo_s) = match inner.find(':') {
        Some(i) => (&inner[..i], &inner[i + 1..]),
        None => (inner, inner),
    };
    let hi = parse_decimal(hi_s).ok_or_else(|| format!("bad bit index {hi_s:?}"))?;
    let lo = parse_decimal(lo_s).ok_or_else(|| format!("bad bit index {lo_s:?}"))?;
    if hi < lo {
        return Err(format!("[{hi}:{lo}]: hi must be >= lo"));
    }
    Ok((feature, Some((hi, lo))))
}

/// Parse a decimal with optional `_` separators.
fn parse_decimal(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        return None;
    }
    let digits: String = s.bytes().filter(|&b| b != b'_').map(|b| b as char).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// Parse a FASM value: `0`, `1`, a plain decimal, or a Verilog constant
/// `[width]'[b|h|d|o]digits`. `address_width` is 1 without a bit selection,
/// else `hi - lo + 1`; mirrors the `fasm` library's checks: an explicit
/// width may not exceed the addressed width, and the value must fit the
/// addressed width.
fn parse_value(text: &str, address_width: u64) -> Result<FasmValue, String> {
    if text.is_empty() {
        return Err("missing value after '='".to_string());
    }
    if let Some(quote) = text.find('\'') {
        return parse_verilog_const(text, quote, address_width);
    }
    // Plain decimal.
    if !text.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        return Err(format!("bad value {text:?}"));
    }
    let digits: String = text.bytes().filter(|&b| b != b'_').map(|b| b as char).collect();
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("value {text:?} out of range"))?;
    check_fits(value, None, address_width, text)?;
    Ok(normalize_small(value, address_width))
}

/// Parse `[width]'radix digits`.
fn parse_verilog_const(text: &str, quote: usize, address_width: u64) -> Result<FasmValue, String> {
    let (width_text, rest) = (&text[..quote], &text[quote + 1..]);
    let width: Option<u32> = if width_text.trim().is_empty() {
        None
    } else {
        // The grammar allows whitespace between width and '\''.
        let w = width_text.trim();
        Some(
            parse_decimal(w)
                .ok_or_else(|| format!("bad Verilog width in {text:?}"))?,
        )
    };
    let mut chars = rest.chars();
    let radix = chars.next().ok_or_else(|| format!("missing radix in {text:?}"))?;
    let (base, valid): (u32, fn(u8) -> bool) = match radix {
        'b' | 'B' => (2, |b: u8| b == b'0' || b == b'1' || b == b'_'),
        'h' | 'H' => (16, |b: u8| b.is_ascii_hexdigit() || b == b'_'),
        'd' | 'D' => (10, |b: u8| b.is_ascii_digit() || b == b'_'),
        'o' | 'O' => (8, |b: u8| (b'0'..=b'7').contains(&b) || b == b'_'),
        _ => return Err(format!("bad Verilog radix {radix:?} in {text:?}")),
    };
    let digits: String = chars.collect();
    // Allow whitespace between radix and digits, like the grammar's S*.
    let digits = digits.trim_start();
    if digits.is_empty() || !digits.bytes().all(valid) {
        return Err(format!("bad Verilog digits in {text:?}"));
    }
    let clean: String = digits.bytes().filter(|&b| b != b'_').map(|b| b as char).collect();
    let value = u64::from_str_radix(&clean, base)
        .map_err(|_| format!("value {text:?} out of range"))?;
    check_fits(value, width, address_width, text)?;
    let implicit_width = address_width.min(64);
    Ok(normalize_small(value, u64::from(width.unwrap_or(implicit_width as u32))))
}

/// Enforce: explicit width <= addressed width, value fits both.
fn check_fits(
    value: u64,
    width: Option<u32>,
    address_width: u64,
    text: &str,
) -> Result<(), String> {
    if let Some(w) = width {
        if w == 0 || w > 64 {
            return Err(format!("Verilog width out of range in {text:?}"));
        }
        if u64::from(w) > address_width {
            return Err(format!(
                "width {w} exceeds addressed width {address_width} in {text:?}"
            ));
        }
        // A 64-bit width holds any u64; shifting by 64 would overflow.
        if w < 64 && value >> w != 0 {
            return Err(format!("value does not fit in {w} bits in {text:?}"));
        }
    }
    if address_width < 64 && value >> address_width != 0 {
        return Err(format!(
            "value does not fit in addressed width {address_width} in {text:?}"
        ));
    }
    Ok(())
}

/// Normalize the trivial constants 0/1 to [`FasmValue::Zero`]/[`FasmValue::One`].
/// (Safe: [`crate::frames`] expands `One` as the integer 1, exactly like the
/// `fasm` library's `canonical_features`.)
fn normalize_small(value: u64, width: u64) -> FasmValue {
    match value {
        0 => FasmValue::Zero,
        1 => FasmValue::One,
        _ => FasmValue::Bits {
            bits: value,
            width: width.min(64) as u32,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> FasmLine {
        parse_line(s, 1).expect("parse failed").expect("empty line")
    }

    #[test]
    fn plain_feature_implies_one() {
        let l = line("INT_L_X10Y146.IMUX_L1.N1");
        assert_eq!(l.tile, "INT_L_X10Y146");
        assert_eq!(l.feature, "IMUX_L1.N1");
        assert_eq!(l.bit_range, None);
        assert_eq!(l.value, FasmValue::One);
    }

    #[test]
    fn explicit_zero_and_one() {
        assert_eq!(line("T_X0Y0.F = 0").value, FasmValue::Zero);
        assert_eq!(line("T_X0Y0.F = 1").value, FasmValue::One);
        assert_eq!(line("T_X0Y0.F=1").value, FasmValue::One);
    }

    #[test]
    fn comments_and_blank_lines() {
        assert_eq!(parse_line("# hello", 3).unwrap(), None);
        assert_eq!(parse_line("   ", 3).unwrap(), None);
        assert_eq!(parse_line("", 3).unwrap(), None);
        let l = line("T_X0Y0.F = 1 # trailing comment");
        assert_eq!(l.value, FasmValue::One);
        // '#' inside a quoted annotation is not a comment.
        let l = line("T_X0Y0.F {note=\"a#b\"} # real comment");
        assert_eq!(l.tile, "T_X0Y0");
    }

    #[test]
    fn single_bit_selection() {
        let l = line("CLBLL_L_X12Y34.SLICEL_X0.ALUT.INIT[63]");
        assert_eq!(l.bit_range, Some((63, 63)));
        assert_eq!(l.value, FasmValue::One);
        let l = line("T_X0Y0.F[7] = 0");
        assert_eq!((l.bit_range, l.value), (Some((7, 7)), FasmValue::Zero));
    }

    #[test]
    fn bit_range_and_verilog_const() {
        let l = line("CLBLL_L_X12Y34.SLICEL_X0.ALUT.INIT[63:0] = 64'hDEADBEEFCAFEBABE");
        assert_eq!(l.bit_range, Some((63, 0)));
        assert_eq!(
            l.value,
            FasmValue::Bits {
                bits: 0xDEADBEEFCAFEBABE,
                width: 64
            }
        );
    }

    #[test]
    fn verilog_radices_and_underscores() {
        assert_eq!(
            line("T_X0Y0.F[3:0] = 4'b1010").value,
            FasmValue::Bits { bits: 0b1010, width: 4 }
        );
        assert_eq!(
            line("T_X0Y0.F[7:0] = 8'hff").value,
            FasmValue::Bits { bits: 0xff, width: 8 }
        );
        assert_eq!(
            line("T_X0Y0.F[7:0] = 'd255").value,
            FasmValue::Bits { bits: 255, width: 8 }
        );
        assert_eq!(
            line("T_X0Y0.F[5:0] = 6'o17").value,
            FasmValue::Bits { bits: 0o17, width: 6 }
        );
        // Underscore separators are ignored.
        assert_eq!(
            line("T_X0Y0.F[15:0] = 16'b1010_0101_0000_1111").value,
            FasmValue::Bits {
                bits: 0b1010_0101_0000_1111,
                width: 16
            }
        );
        // Plain decimal.
        assert_eq!(
            line("T_X0Y0.F[3:0] = 5").value,
            FasmValue::Bits { bits: 5, width: 4 }
        );
        // Trivial constants normalize.
        assert_eq!(line("T_X0Y0.F[3:0] = 4'b0001").value, FasmValue::One);
        assert_eq!(line("T_X0Y0.F[3:0] = 4'b0000").value, FasmValue::Zero);
    }

    #[test]
    fn width_checks_mirror_fasm_library() {
        // Explicit width may not exceed the addressed width ...
        assert!(parse_line("T_X0Y0.F[3:0] = 8'hff", 1).is_err());
        // ... but may be smaller.
        assert!(parse_line("T_X0Y0.F[3:0] = 2'b11", 1).is_ok());
        // Value must fit the addressed width.
        assert!(parse_line("T_X0Y0.F[3:0] = 16", 1).is_err());
        assert!(parse_line("T_X0Y0.F = 5", 1).is_err());
        // Value must fit the explicit width.
        assert!(parse_line("T_X0Y0.F[3:0] = 2'b111", 1).is_err());
        // hi < lo is rejected.
        assert!(parse_line("T_X0Y0.F[0:3] = 1", 1).is_err());
        // Width > 64 is rejected.
        assert!(parse_line("T_X0Y0.F[63:0] = 65'h1", 1).is_err());
    }

    #[test]
    fn malformed_lines_error() {
        for bad in [
            "NOTILE",
            ".F = 1",
            "T_X0Y0. = 1",
            "T_X0Y0.F[3 = 1",
            "T_X0Y0.F[3:0 = 1",
            "T_X0Y0.F = ",
            "T_X0Y0.F = 2",
            "T_X0Y0.F = 1'b2",
            "T_X0Y0.F = 'x10",
            "9TILE.F = 1",
            "T_X0Y0.F {unterminated",
        ] {
            assert!(parse_line(bad, 1).is_err(), "should fail: {bad:?}");
        }
    }

    #[test]
    fn parse_fasm_collects_lines() {
        let text = "# comment\n\nT_X0Y0.A\nT_X1Y1.B[0] = 0\n";
        let lines = parse_fasm(text).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].tile, "T_X0Y0");
        assert_eq!(lines[1].value, FasmValue::Zero);
    }

    #[test]
    fn error_carries_line_number() {
        let err = parse_fasm("T_X0Y0.A\nbogus line\n").unwrap_err();
        assert_eq!(err.line, 2);
    }
}
