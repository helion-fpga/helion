//! Data files of `$readmemh` / `$readmemb` (IEEE 1800, 21.4).
//!
//! A file is white-space separated words, hexadecimal or binary by the
//! task, with `//` and `/* */` comments, `_` inside a word ignored, and
//! `@hhhh` (always hexadecimal) moving the load address. `x`, `z` and `?`
//! digits are not hardware values: they load as 0, and the caller warns.

/// One item of a data file.
#[derive(Debug, PartialEq, Eq)]
pub enum Item {
    /// `@address`
    Addr(u64),
    /// A word, LSB first, and whether it had `x` / `z` digits.
    Word(Vec<bool>, bool),
}

/// Parse a data file. `bin` selects `$readmemb`. Errors start with the
/// line number (`3: ...`).
pub fn parse(text: &str, bin: bool) -> Result<Vec<(Item, u32)>, String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            loop {
                if i + 1 >= b.len() {
                    return Err(format!("{line}: comment without its end"));
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
        } else {
            let st = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && !b[i..].starts_with(b"//") && !b[i..].starts_with(b"/*") {
                i += 1;
            }
            let tok = &text[st..i];
            if let Some(a) = tok.strip_prefix('@') {
                let digits: String = a.chars().filter(|c| *c != '_').collect();
                let v = u64::from_str_radix(&digits, 16).map_err(|_| format!("{line}: bad address '{tok}'"))?;
                out.push((Item::Addr(v), line));
                continue;
            }
            let per = if bin { 1 } else { 4 };
            let mut bits = Vec::new();
            let mut xz = false;
            for ch in tok.chars().rev() {
                if ch == '_' {
                    continue;
                }
                let v = match ch {
                    'x' | 'X' | 'z' | 'Z' | '?' => {
                        xz = true;
                        0
                    }
                    _ => match ch.to_digit(if bin { 2 } else { 16 }) {
                        Some(v) => v,
                        None => return Err(format!("{line}: '{tok}' is not a {} number", if bin { "binary" } else { "hexadecimal" })),
                    },
                };
                for k in 0..per {
                    bits.push((v >> k) & 1 == 1);
                }
            }
            if bits.is_empty() {
                return Err(format!("{line}: '{tok}' has no digits"));
            }
            out.push((Item::Word(bits, xz), line));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(bits: &[bool]) -> u64 {
        bits.iter().enumerate().fold(0, |v, (i, b)| v | ((*b as u64) << i))
    }

    #[test]
    fn hex_words_comments_addresses() {
        let f = "// header\n12 a_B /* two\nlines */ @1_0\nfF //x\n 3x\n";
        let items = parse(f, false).unwrap();
        assert_eq!(items.len(), 5);
        let w: Vec<(u64, bool, u32)> = items
            .iter()
            .map(|(it, l)| match it {
                Item::Word(b, x) => (val(b), *x, *l),
                Item::Addr(a) => (1 << 40 | a, false, *l),
            })
            .collect();
        assert_eq!(w, vec![(0x12, false, 2), (0xab, false, 2), (1 << 40 | 0x10, false, 3), (0xff, false, 4), (0x30, true, 5)]);
    }

    #[test]
    fn binary_words() {
        let items = parse("1_01\n0z1 @3 11", true).unwrap();
        assert_eq!(items[0].0, Item::Word(vec![true, false, true], false));
        assert_eq!(items[1].0, Item::Word(vec![true, false, false], true));
        assert_eq!(items[2].0, Item::Addr(3));
        assert!(parse("12", true).unwrap_err().contains("binary"));
        assert!(parse("g1", false).unwrap_err().starts_with("1: "));
        assert!(parse("1 /* open", false).is_err());
    }
}
