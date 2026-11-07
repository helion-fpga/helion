//! INT PIP emission: routed wires to exact segbits.
//!
//! A routed net crosses INT tiles through PIPs (programmable interconnect
//! points). Each PIP is one mux selection inside one INT tile, named
//! `<tile>.PIP.<src>.<dst>` in FASM, e.g.
//! `INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`. The segbits DB gives the
//! exact configuration bits for that selection, including `!` bits that must
//! read 0 in the base frameset.
//!
//! Rules, from the crew brief and SPEC.md:
//! - Every emitted bit comes from a DB lookup. A guessed PIP encoding is a
//!   FAIL, so a PIP with no DB entry is never skipped silently and never
//!   guessed: it goes on the missing-PIP list and emission fails loud.
//! - Every `!` bit is asserted to read 0 in the base frameset before the
//!   emission is accepted. The base frameset belongs to bitgen-core; this
//!   module takes a probe closure so the check is testable here and usable
//!   there.
//!
//! Router input shape: the day-1 router scaffold emits `UltraRouted`
//! (wire-ID paths), and wire IDs carry no names, so a wire path alone
//! cannot name a PIP. The PIP layer therefore takes named PIPs
//! (`IntPip { tile, src, dst }`); the WireDb naming seam (wire ID to wire
//! name per tile) is an interface item for the integration worker. Tests
//! build named PIPs from real DB edges, which is the same input the
//! production router will produce once the seam exists.

use crate::segbits::{BitRef, SegbitsDb};
use std::fmt;

/// One INT PIP: a mux selection from wire `src` to wire `dst` in INT tile
/// `tile` (Vivado tile name, e.g. `INT_X12Y45`).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct IntPip {
    pub tile: String,
    pub src: String,
    pub dst: String,
}

impl IntPip {
    pub fn new(tile: &str, src: &str, dst: &str) -> Self {
        Self {
            tile: tile.to_string(),
            src: src.to_string(),
            dst: dst.to_string(),
        }
    }

    /// Segbits feature string, exactly as it appears in the DB.
    pub fn feature(&self) -> String {
        format!("PIP.{}.{}", self.src, self.dst)
    }

    /// Tile type for the DB lookup: the part of the tile name before the
    /// first `_` (e.g. `INT` for `INT_X12Y45`).
    pub fn tile_type(&self) -> &str {
        match self.tile.split_once('_') {
            Some((head, _)) => head,
            None => self.tile.as_str(),
        }
    }

    /// FASM line for this PIP, no value: `<tile>.PIP.<src>.<dst>`.
    pub fn fasm_line(&self) -> String {
        format!("{}.{}", self.tile, self.feature())
    }
}

/// One emitted PIP: the FASM line plus the exact bits it controls, split
/// into bits to set and bits that must read 0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedPip {
    pub pip: IntPip,
    pub fasm_line: String,
    pub set_bits: Vec<BitRef>,
    pub zero_bits: Vec<BitRef>,
}

impl EmittedPip {
    /// Total configuration bits this PIP controls (set plus zero).
    pub fn bit_count(&self) -> usize {
        self.set_bits.len() + self.zero_bits.len()
    }
}

/// A PIP the router needed that has no segbits entry. Kept exact: tile,
/// src, dst, and the design that needed it. This list is the input to the
/// INT PIP completeness audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingPip {
    pub tile: String,
    pub src: String,
    pub dst: String,
    pub design: String,
}

impl fmt::Display for MissingPip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.PIP.{}.{} (design {})",
            self.tile, self.src, self.dst, self.design
        )
    }
}

/// Emission failure. Carries the exact missing list, or the exact bit that
/// violated a zero assertion. Never a bare message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntPipError {
    /// One or more PIPs have no segbits entry. Emission stopped; nothing
    /// was guessed.
    MissingPips(Vec<MissingPip>),
    /// A `!` bit reads 1 in the base frameset. The base frameset is wrong
    /// for this design, or the DB line is wrong for this tile.
    ZeroBitSet {
        tile: String,
        feature: String,
        frame: u32,
        bit: u8,
    },
}

impl fmt::Display for IntPipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPips(missing) => {
                writeln!(
                    f,
                    "{} PIP(s) have no segbits entry; emission refused:",
                    missing.len()
                )?;
                for m in missing {
                    writeln!(f, "  {}", m)?;
                }
                Ok(())
            }
            Self::ZeroBitSet {
                tile,
                feature,
                frame,
                bit,
            } => write!(
                f,
                "zero bit {}_{:02} reads 1 in the base frameset for {}.{}",
                frame, bit, tile, feature
            ),
        }
    }
}

impl std::error::Error for IntPipError {}

/// Resolution of a PIP list against the DB: every PIP either emitted with
/// its exact bits, or recorded missing. Nothing is skipped, nothing guessed.
/// The caller decides whether missing entries are an error (see
/// [`emit_pips`]) or an audit report (see [`missing_pip_report`]).
#[derive(Clone, Debug, Default)]
pub struct PipResolution {
    pub emitted: Vec<EmittedPip>,
    pub missing: Vec<MissingPip>,
}

/// Resolve PIPs against the DB, partitioning into emitted and missing.
pub fn resolve_pips(design: &str, pips: &[IntPip], db: &dyn SegbitsDb) -> PipResolution {
    let mut out = PipResolution::default();
    for pip in pips {
        match db.lookup(pip.tile_type(), &pip.feature()) {
            Some(bits) => {
                let mut set_bits = Vec::new();
                let mut zero_bits = Vec::new();
                for b in bits {
                    if b.must_be_zero {
                        zero_bits.push(*b);
                    } else {
                        set_bits.push(*b);
                    }
                }
                out.emitted.push(EmittedPip {
                    fasm_line: pip.fasm_line(),
                    pip: pip.clone(),
                    set_bits,
                    zero_bits,
                });
            }
            None => out.missing.push(MissingPip {
                tile: pip.tile.clone(),
                src: pip.src.clone(),
                dst: pip.dst.clone(),
                design: design.to_string(),
            }),
        }
    }
    out
}

/// Emit PIPs: like [`resolve_pips`], but any missing PIP fails the whole
/// emission with the exact missing list. Callers that need a partial result
/// use `resolve_pips` directly.
pub fn emit_pips(
    design: &str,
    pips: &[IntPip],
    db: &dyn SegbitsDb,
) -> Result<Vec<EmittedPip>, IntPipError> {
    let res = resolve_pips(design, pips, db);
    if !res.missing.is_empty() {
        return Err(IntPipError::MissingPips(res.missing));
    }
    Ok(res.emitted)
}

/// Assert every `!` bit of every emitted PIP reads 0 in the base frameset.
/// `base_reads_one(frame, bit)` probes the base frameset: true means the
/// bit reads 1 there. Returns the first violation, naming tile, feature,
/// frame, and bit.
pub fn verify_zero_bits(
    emitted: &[EmittedPip],
    base_reads_one: &dyn Fn(u32, u8) -> bool,
) -> Result<(), IntPipError> {
    for e in emitted {
        for b in &e.zero_bits {
            if base_reads_one(b.frame, b.bit) {
                return Err(IntPipError::ZeroBitSet {
                    tile: e.pip.tile.clone(),
                    feature: e.pip.feature(),
                    frame: b.frame,
                    bit: b.bit,
                });
            }
        }
    }
    Ok(())
}

/// FASM text for emitted PIPs: one line per PIP, in emission order.
pub fn emitted_to_fasm(emitted: &[EmittedPip]) -> String {
    let mut out = String::new();
    for e in emitted {
        out.push_str(&e.fasm_line);
        out.push('\n');
    }
    out
}

/// Human-readable missing-PIP report: one line per missing PIP with tile,
/// src, dst, and design. Empty list reports zero missing, never silence.
pub fn missing_pip_report(missing: &[MissingPip]) -> String {
    if missing.is_empty() {
        return "Missing PIPs: 0\n".to_string();
    }
    let mut out = format!("Missing PIPs: {}\n", missing.len());
    for m in missing {
        out.push_str(&format!("  {}\n", m));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segbits::FileSegbitsDb;

    /// Two verbatim lines from prjuray-db zynqusp/segbits_int.db
    /// (commit affbc5e, CC0 1.0).
    const FIXTURE: &str = "INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00 !72_00 !72_01 !74_00\n\
         INT.PIP.INODE_E_1_FT1.BYPASS_E4 !57_00 !59_00 !61_00 65_01 !67_01 !69_01\n";

    fn fixture_db() -> FileSegbitsDb {
        FileSegbitsDb::from_text(FIXTURE, "fixture").unwrap()
    }

    #[test]
    fn pip_names_match_db_vocabulary() {
        let pip = IntPip::new("INT_X12Y45", "BOUNCE_E_0_FT1", "INODE_E_1_FT1");
        assert_eq!(pip.feature(), "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1");
        assert_eq!(pip.tile_type(), "INT");
        assert_eq!(
            pip.fasm_line(),
            "INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
        );
    }

    #[test]
    fn bounce_pip_emits_exact_bits() {
        let db = fixture_db();
        let pip = IntPip::new("INT_X12Y45", "BOUNCE_E_0_FT1", "INODE_E_1_FT1");
        let emitted = emit_pips("blinky", &[pip], &db).unwrap();
        assert_eq!(emitted.len(), 1);
        let e = &emitted[0];
        assert_eq!(e.set_bits, vec![BitRef::set(64, 0)]);
        assert_eq!(
            e.zero_bits,
            vec![
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ]
        );
        assert_eq!(
            emitted_to_fasm(&emitted),
            "INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1\n"
        );
    }

    #[test]
    fn zero_bit_assertion_passes_on_zero_base_and_fires_on_one() {
        let db = fixture_db();
        let pip = IntPip::new("INT_X12Y45", "BOUNCE_E_0_FT1", "INODE_E_1_FT1");
        let emitted = emit_pips("blinky", &[pip], &db).unwrap();
        assert!(verify_zero_bits(&emitted, &|_, _| false).is_ok());
        // Base reads 1 at 66_00, one of the BOUNCE zero bits.
        let err = verify_zero_bits(&emitted, &|f, b| f == 66 && b == 0).unwrap_err();
        assert_eq!(
            err,
            IntPipError::ZeroBitSet {
                tile: "INT_X12Y45".to_string(),
                feature: "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1".to_string(),
                frame: 66,
                bit: 0,
            }
        );
        // A set bit reading 1 in the base is not a violation.
        assert!(verify_zero_bits(&emitted, &|f, b| f == 64 && b == 0).is_ok());
    }

    #[test]
    fn missing_pip_fails_loud_with_exact_list() {
        let db = fixture_db();
        let good = IntPip::new("INT_X12Y45", "BOUNCE_E_0_FT1", "INODE_E_1_FT1");
        let bad = IntPip::new("INT_X12Y45", "NOPE_SRC", "NOPE_DST");
        let res = resolve_pips("blinky", &[good.clone(), bad.clone()], &db);
        assert_eq!(res.emitted.len(), 1);
        assert_eq!(
            res.missing,
            vec![MissingPip {
                tile: "INT_X12Y45".to_string(),
                src: "NOPE_SRC".to_string(),
                dst: "NOPE_DST".to_string(),
                design: "blinky".to_string(),
            }]
        );
        let err = emit_pips("blinky", &[good, bad], &db).unwrap_err();
        let text = format!("{}", err);
        assert!(text.contains("INT_X12Y45.PIP.NOPE_SRC.NOPE_DST"));
        assert!(text.contains("blinky"));
        assert_eq!(
            missing_pip_report(&res.missing),
            "Missing PIPs: 1\n  INT_X12Y45.PIP.NOPE_SRC.NOPE_DST (design blinky)\n"
        );
    }

    #[test]
    fn empty_pip_list_emits_nothing_and_reports_zero_missing() {
        let db = fixture_db();
        let res = resolve_pips("blinky", &[], &db);
        assert!(res.emitted.is_empty() && res.missing.is_empty());
        assert_eq!(missing_pip_report(&res.missing), "Missing PIPs: 0\n");
        assert!(emit_pips("blinky", &[], &db).unwrap().is_empty());
    }
}
