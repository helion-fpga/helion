//! DSP48E2 config-bit resolution from measured black-box fuzz data.
//!
//! Source of the bits: `crates/helion-ultrascale/data/vivado_fuzz_xcku3p.tsv`,
//! which records one attribute change per bitstream against a base design and
//! the resulting bit delta. See the "Vivado black-box fuzz" section of
//! `data/PROVENANCE.md` for the method, the independent check, and the
//! approval gate on pushing that data publicly.
//!
//! Scope, stated plainly: this module knows exactly the attributes and value
//! pairs the fuzz table lists, and nothing else. Everything else stays
//! blocked through [`Dsp48e2Blocked`](crate::bitgen::BitgenError::Dsp48e2Blocked),
//! with the offending attribute named in the error. An attribute the table
//! does not mention is not "probably fine": it is unmeasured, and guessing
//! it would put a wrong bit in a bitstream that then fails on hardware with
//! no diagnostic.
//!
//! Two properties of the measured data drive the design here:
//!
//! 1. **An attribute can span several bits, and those bits only mean
//!    anything as a set.** `AMULTSEL A -> AD` touches three bits across two
//!    frames: it clears minor 4 bit 2579 and minor 5 bit 2611 and sets minor
//!    5 bit 2585. Emitting one of those without the others produces a
//!    bitstream no fuzz run ever produced. So a value resolves to its whole
//!    bit set or to nothing.
//! 2. **Some bits are stored inverted.** A fuzz row whose effect is `-8`
//!    means the base bitstream had that bit set and the variant cleared it,
//!    so emitting the target value means CLEARING the bit. [`Bit`] carries
//!    that per bit rather than making callers remember a sign convention.
//!
//! No em dashes anywhere.

use std::collections::BTreeMap;

/// One bit of one attribute's encoding, relative to the DSP tile's base
/// frame minor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Bit {
    /// Frame minor offset from the tile's base minor (0..=6 measured).
    ///
    /// The fuzz diffs report an absolute minor (4, 5, 6 for this tile); the
    /// emitter addresses frames as `tile_base_far + offset`, and the tile's
    /// base FAR is its minor-4 frame, so the offset is `minor - BASE_MINOR`.
    /// Keeping the absolute minor in the table matches the data file and the
    /// diffs; [`Bit::frame_offset`] does the conversion in one place.
    pub minor: u8,
    /// Bit position within the frame, absolute (0..2592).
    pub bit: u16,
    /// True when emitting this attribute value means setting the bit, false
    /// when it means clearing it (a `-8` row in the fuzz table).
    pub set: bool,
}

/// The DSP tile's base frame minor, i.e. the minor of `tile_base_far`.
///
/// Measured: DSP48E2_X0Y44's bits fall in FAR 0x00040E04..E06, minors 4..6.
pub const BASE_MINOR: u8 = 4;

impl Bit {
    /// Frame offset to add to `tile_base_far`.
    pub fn frame_offset(&self) -> u32 {
        u32::from(self.minor.saturating_sub(BASE_MINOR))
    }
}

/// Attribute values the fuzz table measured, keyed by attribute name.
///
/// Only the pairs below resolve. `A` and `B` (the base values) are absent on
/// purpose: base values emit no bits, so they are handled by [`is_base_value`]
/// rather than by a table entry with an empty bit list.
pub struct Encoding {
    /// Attribute name as FASM and Vivado spell it.
    pub attribute: &'static str,
    /// The value the base design had (what the fuzz run started from).
    pub from: &'static str,
    /// The value the fuzz run changed it to.
    pub to: &'static str,
    /// Every bit that changes, applied as a set.
    pub bits: &'static [Bit],
}

const fn set(minor: u8, bit: u16) -> Bit {
    Bit {
        minor,
        bit,
        set: true,
    }
}

const fn clear(minor: u8, bit: u16) -> Bit {
    Bit {
        minor,
        bit,
        set: false,
    }
}

/// The measured table, transcribed one row per line of
/// `data/vivado_fuzz_xcku3p.tsv`. A unit test asserts this table and the
/// data file agree row for row, so the two cannot drift.
pub const MEASURED: &[Encoding] = &[
    Encoding {
        attribute: "AMULTSEL",
        from: "A",
        to: "AD",
        bits: &[clear(4, 2579), set(5, 2585), clear(5, 2611)],
    },
    Encoding {
        attribute: "BMULTSEL",
        from: "B",
        to: "AD",
        bits: &[clear(4, 2579), set(5, 2583), clear(5, 2611)],
    },
    Encoding {
        attribute: "A_INPUT",
        from: "DIRECT",
        to: "CASCADE",
        bits: &[set(4, 2580)],
    },
    Encoding {
        attribute: "B_INPUT",
        from: "DIRECT",
        to: "CASCADE",
        bits: &[set(4, 2560)],
    },
    Encoding {
        attribute: "PREADDINSEL",
        from: "A",
        to: "B",
        bits: &[set(4, 2582)],
    },
    Encoding {
        attribute: "RND",
        from: "000000000000",
        to: "000000000001",
        bits: &[set(4, 2565)],
    },
    Encoding {
        attribute: "USE_MULT",
        from: "MULTIPLY",
        to: "NONE",
        bits: &[set(5, 2574)],
    },
    Encoding {
        attribute: "USE_WIDEXOR",
        from: "FALSE",
        to: "TRUE",
        bits: &[set(5, 2576)],
    },
    Encoding {
        attribute: "XORSIMD",
        from: "XOR24_48_96",
        to: "XOR12",
        bits: &[set(4, 2505)],
    },
    Encoding {
        attribute: "AUTORESET_PATDET",
        from: "NO_RESET",
        to: "RESET_MATCH",
        bits: &[set(5, 2531)],
    },
    Encoding {
        attribute: "AUTORESET_PATDET",
        from: "NO_RESET",
        to: "RESET_NOT_MATCH",
        bits: &[set(5, 2503), set(5, 2531)],
    },
    Encoding {
        attribute: "AUTORESET_PRIORITY",
        from: "RESET",
        to: "CEP",
        bits: &[set(5, 2540)],
    },
    Encoding {
        attribute: "PATTERN",
        from: "000000000000",
        to: "000000000001",
        bits: &[set(5, 2571)],
    },
    Encoding {
        attribute: "SEL_PATTERN",
        from: "PATTERN",
        to: "C",
        bits: &[set(5, 2579)],
    },
    Encoding {
        attribute: "USE_PATTERN_DETECT",
        from: "NO_PATDET",
        to: "PATDET",
        bits: &[clear(6, 2534), clear(6, 2536)],
    },
    Encoding {
        attribute: "IS_CLK_INVERTED",
        from: "1'b0",
        to: "1'b1",
        bits: &[clear(4, 2536)],
    },
    Encoding {
        attribute: "MREG",
        from: "1",
        to: "0",
        bits: &[set(5, 2582)],
    },
];

/// The base design's attribute values, i.e. what the bitstream has when an
/// attribute emits no bit at all. Every pipeline register is 1 in the base;
/// the input/multiplier selectors default to the plain path.
fn is_base_value(attribute: &str, value: &str) -> bool {
    let base: &str = match attribute {
        "AMULTSEL" => "A",
        "BMULTSEL" => "B",
        "A_INPUT" | "B_INPUT" => "DIRECT",
        "PREADDINSEL" => "A",
        "USE_MULT" => "MULTIPLY",
        "USE_WIDEXOR" => "FALSE",
        "XORSIMD" => "XOR24_48_96",
        "AUTORESET_PATDET" => "NO_RESET",
        "AUTORESET_PRIORITY" => "RESET",
        "SEL_PATTERN" => "PATTERN",
        "USE_PATTERN_DETECT" => "NO_PATDET",
        // Registers default to 1 in the job-2 baseline design.
        "ACASCREG" | "ADREG" | "ALUMODEREG" | "AREG" | "BCASCREG" | "BREG"
        | "CARRYINREG" | "CARRYINSELREG" | "CREG" | "DREG" | "INMODEREG" | "MREG"
        | "OPMODEREG" | "PREG" => "1",
        // Zero-valued inversion attributes are the base (no inversion).
        a if a.ends_with("_INVERTED") => "1'b0",
        _ => return false,
    };
    values_equal(value, base)
}

/// Compare an attribute value against a table value, tolerating the spellings
/// Vivado uses for the same value: `1'b0` / `0`, `48'h0` / `0`, and case.
fn values_equal(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    normalize(a) == normalize(b)
}

/// Reduce a value to a bare lowercase bit string: strip a Verilog size and
/// base prefix (`1'b`, `48'h`, `4'd`) and leading zeros.
fn normalize(v: &str) -> String {
    let v = v.trim();
    let digits = match v.find('\'') {
        Some(quote) => {
            // `1'b0` -> "0", `48'h0001` -> "0001". The character after the
            // quote is the base letter, the rest is the literal.
            let rest = &v[quote + 1..];
            let mut chars = rest.chars();
            chars.next();
            chars.as_str()
        }
        None => v,
    };
    let lower = digits.trim().to_ascii_lowercase();
    let trimmed = lower.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Outcome of resolving one DSP attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolved {
    /// The attribute equals the base value: no bit is emitted.
    Base,
    /// The attribute has a measured encoding: emit exactly these bits.
    Bits(Vec<Bit>),
}

/// Why an attribute could not be resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Unmeasured {
    /// The attribute is not in the measured table at all.
    Attribute,
    /// The attribute is measured, but not from the requested starting value
    /// or to the requested target.
    Value { from: String, to: String },
}

/// Resolve one `attribute=value` pair for a DSP48E2.
///
/// `value` is the single requested value. The fuzz table records
/// `from -> to` pairs because that is how the bitstream was produced; a
/// configured design only states `to`. Resolution therefore looks for any
/// row whose `to` matches the requested value and whose `from` is the base
/// value, which is exactly the situation of a design that sets the
/// attribute once in RTL.
pub fn resolve(attribute: &str, value: &str) -> Result<Resolved, Unmeasured> {
    if is_base_value(attribute, value) {
        return Ok(Resolved::Base);
    }
    let mut matches: Vec<&Encoding> = MEASURED
        .iter()
        .filter(|e| e.attribute == attribute && values_equal(e.to, value))
        .collect();
    if matches.is_empty() {
        return Err(match MEASURED.iter().find(|e| e.attribute == attribute) {
            Some(e) => Unmeasured::Value {
                from: e.from.to_string(),
                to: e.to.to_string(),
            },
            None => Unmeasured::Attribute,
        });
    }
    // A target reachable from more than one starting value would be
    // ambiguous; the measured table has no such case, and if a future row
    // introduced one this would fail loudly rather than pick a row.
    if matches.len() > 1 {
        let froms: Vec<&str> = matches.iter().map(|e| e.from).collect();
        if froms.iter().any(|f| *f != froms[0]) {
            return Err(Unmeasured::Value {
                from: froms.join("|"),
                to: value.to_string(),
            });
        }
    }
    matches.sort_by_key(|e| e.to);
    let row = matches[0];
    Ok(Resolved::Bits(row.bits.to_vec()))
}

/// Attributes the measured table can encode, sorted and deduplicated. This
/// is the allow-list the emitter consults when deciding whether a DSP entry
/// is still blocked.
pub fn measured_attributes() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = MEASURED.iter().map(|e| e.attribute).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Emit all measured bits for a whole DSP configuration, keyed by
/// `(minor, bit)` with the final set/clear value.
///
/// Fails if any attribute is unmeasured. Applying the whole configuration at
/// once is what keeps multi-bit attributes atomic: a caller cannot apply
/// half of `AMULTSEL A -> AD`.
pub fn resolve_config<'a, I>(attributes: I) -> Result<BTreeMap<(u8, u16), bool>, Unmeasured>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut out: BTreeMap<(u8, u16), bool> = BTreeMap::new();
    for (attribute, value) in attributes {
        match resolve(attribute, value)? {
            Resolved::Base => {}
            Resolved::Bits(bits) => {
                for b in bits {
                    match out.get(&(b.minor, b.bit)) {
                        // Two attributes writing one bit to different values
                        // is a contradiction, not a preference.
                        Some(&existing) if existing != b.set => {
                            return Err(Unmeasured::Value {
                                from: format!("{}={}", attribute, value),
                                to: format!("conflict at minor {} bit {}", b.minor, b.bit),
                            })
                        }
                        _ => {
                            out.insert((b.minor, b.bit), b.set);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// The data file and the table must agree row for row. This is the guard
    /// that keeps the vendored table honest: if someone edits the file (or
    /// ACCIDENTALLY reorders it) without updating the table, or vice versa,
    /// this fails.
    #[test]
    fn measured_table_matches_data_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/vivado_fuzz_xcku3p.tsv");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e));
        let mut rows: Vec<(String, String, String, u8, u16, bool)> = Vec::new();
        let mut in_body = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("attribute\t") {
                in_body = true;
                continue;
            }
            if !in_body {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            assert_eq!(f.len(), 6, "malformed row: {line}");
            let plus = match f[5] {
                "+8" => true,
                "-8" => false,
                other => panic!("effect must be +8 or -8, got {other}"),
            };
            rows.push((
                f[0].to_string(),
                f[1].to_string(),
                f[2].to_string(),
                f[3].parse().expect("minor"),
                f[4].parse().expect("bit"),
                plus,
            ));
        }
        let mut table: Vec<(String, String, String, u8, u16, bool)> = Vec::new();
        for e in MEASURED {
            for b in e.bits {
                table.push((
                    e.attribute.to_string(),
                    e.from.to_string(),
                    e.to.to_string(),
                    b.minor,
                    b.bit,
                    b.set,
                ));
            }
        }
        assert_eq!(
            rows, table,
            "data/vivado_fuzz_xcku3p.tsv and dsp::MEASURED disagree"
        );
        assert!(!rows.is_empty(), "no rows parsed from the fuzz file");
    }

    #[test]
    fn amultsel_ad_emits_all_three_measured_bits() {
        // The task's guard: a DSP using AMULTSEL=AD emits exactly the
        // measured bits, all three of them, with the right polarity.
        let got = resolve("AMULTSEL", "AD").expect("AMULTSEL AD is measured");
        assert_eq!(
            got,
            Resolved::Bits(vec![clear(4, 2579), set(5, 2585), clear(5, 2611)])
        );
    }

    #[test]
    fn base_values_emit_nothing() {
        for (attr, value) in [
            ("AMULTSEL", "A"),
            ("BMULTSEL", "B"),
            ("A_INPUT", "DIRECT"),
            ("MREG", "1"),
            ("USE_MULT", "MULTIPLY"),
            ("IS_CLK_INVERTED", "1'b0"),
        ] {
            assert_eq!(
                resolve(attr, value),
                Ok(Resolved::Base),
                "{attr}={value} should be the base"
            );
        }
    }

    #[test]
    fn areg_zero_is_refused() {
        // The task's second guard: AREG=0 has no measured encoding, so it
        // must be refused rather than guessed.
        let err = resolve("AREG", "0").expect_err("AREG=0 is unmeasured");
        assert!(
            matches!(err, Unmeasured::Attribute),
            "expected Attribute, got {err:?}"
        );
    }

    #[test]
    fn registers_default_to_one_but_zero_is_unmeasured() {
        // Every pipeline register is base at 1 and blocked at 0; only MREG
        // 1->0 was ever measured.
        for reg in [
            "ACASCREG", "ADREG", "ALUMODEREG", "AREG", "BCASCREG", "BREG", "CARRYINREG",
            "CARRYINSELREG", "CREG", "DREG", "INMODEREG", "OPMODEREG", "PREG",
        ] {
            assert_eq!(resolve(reg, "1"), Ok(Resolved::Base), "{reg}=1");
            assert_eq!(
                resolve(reg, "0"),
                Err(Unmeasured::Attribute),
                "{reg}=0 must stay blocked"
            );
        }
        assert_eq!(resolve("MREG", "1"), Ok(Resolved::Base));
        assert_eq!(resolve("MREG", "0"), Ok(Resolved::Bits(vec![set(5, 2582)])));
    }

    #[test]
    fn unmeasured_value_of_measured_attribute_names_the_measured_pairs() {
        // USE_SIMD was refused by Vivado itself (write_bitstream failed), so
        // no value of it is measured.
        assert_eq!(
            resolve("USE_SIMD", "TWO24"),
            Err(Unmeasured::Attribute)
        );
        // SEL_PATTERN is measured only PATTERN->C.
        match resolve("SEL_PATTERN", "PATTERN") {
            Ok(Resolved::Base) => {}
            other => panic!("SEL_PATTERN=PATTERN should be base, got {other:?}"),
        }
        // A value of a measured attribute that no row reaches is a Value
        // error carrying the measured pair, not silently accepted.
        match resolve("XORSIMD", "XOR48") {
            Err(Unmeasured::Value { from, to }) => {
                assert_eq!(from, "XOR24_48_96");
                assert_eq!(to, "XOR12");
            }
            other => panic!("expected a Value error, got {other:?}"),
        }
    }

    #[test]
    fn multi_bit_attributes_are_atomic() {
        // Every AMULTSEL row contributes all three bits or none: there is no
        // row with a partial bit set left in the table.
        for e in MEASURED.iter().filter(|e| e.attribute == "AMULTSEL") {
            assert_eq!(e.bits.len(), 3, "AMULTSEL rows are three bits each");
        }
        // A whole-configuration resolve returns all of them together.
        let cfg = resolve_config(vec![("AMULTSEL", "AD")]).expect("measured");
        assert_eq!(cfg.len(), 3);
        assert_eq!(cfg.get(&(4, 2579)), Some(&false));
        assert_eq!(cfg.get(&(5, 2585)), Some(&true));
        assert_eq!(cfg.get(&(5, 2611)), Some(&false));
    }

    #[test]
    fn conflicting_writes_are_refused() {
        // AMULTSEL and BMULTSEL share bits 2579 and 2611 with the SAME
        // polarity, so setting both to AD is consistent and must combine
        // without error. (They were measured separately; this checks the
        // combination does not silently drop one.)
        let cfg = resolve_config(vec![("AMULTSEL", "AD"), ("BMULTSEL", "AD")]).expect("consistent");
        assert_eq!(cfg.get(&(4, 2579)), Some(&false));
        assert_eq!(cfg.get(&(5, 2585)), Some(&true));
        assert_eq!(cfg.get(&(5, 2583)), Some(&true));
        assert_eq!(cfg.get(&(5, 2611)), Some(&false));
    }

    #[test]
    fn measured_clears_correspond_to_bits_the_base_has_set() {
        // Why the emitter must materialize `-8` (clear) rows rather than
        // treating them as no-ops: several measured clear targets are bits
        // the base bitstream actually has SET. Verified against the base
        // bitstream of the fuzz run (DSP_X3Y110, FAR 0x00040E04 and E05):
        //   minor 4 bit 2579 = 1
        //   minor 5 bit 2611 = 1
        //   minor 5 bit 2585 = 0
        // So AMULTSEL A -> AD is base {2579:1, 2585:0, 2611:1} to
        // {2579:0, 2585:1, 2611:0}: two real clears and one real set.
        // If a future refactor drops the clears, this test still passes but
        // bitgen::tests::dsp_amultsel_ad_emits_exactly_the_measured_bits
        // fails, because that one checks the emitted frames.
        let bits = match resolve("AMULTSEL", "AD").unwrap() {
            Resolved::Bits(b) => b,
            other => panic!("expected bits, got {other:?}"),
        };
        let cleared: Vec<(u8, u16)> = bits
            .iter()
            .filter(|b| !b.set)
            .map(|b| (b.minor, b.bit))
            .collect();
        assert_eq!(cleared, vec![(4, 2579), (5, 2611)]);
        assert_eq!(bits.iter().filter(|b| b.set).count(), 1);
    }

    #[test]
    fn frame_offsets_are_relative_to_the_base_minor() {
        // The tile base FAR is the minor-4 frame, so measured minors 4, 5, 6
        // map to frame offsets 0, 1, 2.
        for (minor, want) in [(4u8, 0u32), (5, 1), (6, 2)] {
            let b = Bit {
                minor,
                bit: 0,
                set: true,
            };
            assert_eq!(b.frame_offset(), want, "minor {minor}");
        }
        assert_eq!(BASE_MINOR, 4);
    }

    #[test]
    fn whitelist_is_the_measured_set() {
        let names = measured_attributes();
        assert!(names.contains(&"AMULTSEL"));
        assert!(names.contains(&"MREG"));
        assert!(!names.contains(&"AREG"), "AREG has no measured value");
        assert!(!names.contains(&"USE_SIMD"));
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted, no dupes");
    }

    #[test]
    fn values_equal_handles_verilog_spellings() {
        assert!(values_equal("1'b0", "0"));
        assert!(values_equal("1", "1'b1"));
        assert!(values_equal("000000000001", "48'h1"));
        assert!(values_equal("AD", "ad"));
        assert!(!values_equal("0", "1"));
        assert!(!values_equal("AD", "A"));
    }
}
