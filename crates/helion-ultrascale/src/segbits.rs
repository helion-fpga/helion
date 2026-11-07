//! Segbits database loading and lookup.
//!
//! The segbits DBs are the ground truth for bitgen emission: each line maps
//! a tile-type feature to the exact configuration bits it controls, in the
//! FASM segbits format used by prjuray-db (CC0 1.0):
//!
//! ```text
//! INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00 !72_00 !72_01 !74_00
//! ```
//!
//! The first token is `<tile_type>.<feature>`. Each following token is
//! `<frame>_<bit>`, with an optional `!` prefix meaning the bit must read 0
//! in the base frameset. A token without `!` means the bit must be set.
//!
//! This module implements the `SegbitsDb` trait from the crew INTERFACES.md.
//! The vendored repo data under `crates/helion-ultrascale/data/` is owned by
//! the segbits-vendor worker; this loader reads whatever path it is given, so
//! it works against the vendored copy, a reference clone, or test fixtures.
//! The loader never reads `/tmp` on its own: callers pass the path.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

/// One configuration bit reference: frame number, bit index within the frame,
/// and whether the bit must read 0 (`!` token) instead of being set.
///
/// `bit` is u16, not u8: the vendored HPIO/HDIO DBs prove u8 cannot hold
/// the data (segbits_hdio_top_right.db has bit 1390,
/// segbits_hdio_bot_right.db bit 1310, segbits_hpio_right.db bit 1359;
/// prjuray `parse_tagbit` returns a bit address within the frame, and a
/// 93-word UltraScale+ frame holds 2976 bit positions). The ported
/// loader took u8 because it was verified against segbits_int.db only
/// (observed 0..47). Widened by segbits-vendor in the vendoring commit
/// so the full vendored set loads; see the `bit_width_above_u8` test.
/// INTERFACES.md states `bit: u16` (orchestrator ruling 2026-11-07 on
/// segbits-vendor 98e282a).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct BitRef {
    pub frame: u32,
    pub bit: u16,
    pub must_be_zero: bool,
}

impl BitRef {
    /// A bit that must be set: `<frame>_<bit>`.
    pub fn set(frame: u32, bit: u16) -> Self {
        Self {
            frame,
            bit,
            must_be_zero: false,
        }
    }

    /// A bit that must read 0: `!<frame>_<bit>`.
    pub fn zero(frame: u32, bit: u16) -> Self {
        Self {
            frame,
            bit,
            must_be_zero: true,
        }
    }
}

impl fmt::Display for BitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.must_be_zero {
            write!(f, "!{}_{:02}", self.frame, self.bit)
        } else {
            write!(f, "{}_{:02}", self.frame, self.bit)
        }
    }
}

/// Segbits lookup contract (crew INTERFACES.md). Implemented by
/// [`FileSegbitsDb`]; bitgen-core and int-pip consume it through this trait.
pub trait SegbitsDb {
    /// Bits for `feature` of tile type `tile_type`, or None if unknown.
    /// An unknown feature is not an error here; callers fail loud on it.
    fn lookup(&self, tile_type: &str, feature: &str) -> Option<&[BitRef]>;
    /// Sorted tile types present in the database.
    fn tile_types(&self) -> &[String];
}

/// Failure to load or parse a segbits file. Malformed lines fail loud: a
/// silently skipped line would be a guessed encoding downstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SegbitsLoadError {
    /// The file could not be read.
    Io(String),
    /// A line does not parse. Carries line number, the raw line, and why.
    MalformedLine {
        line_no: usize,
        line: String,
        reason: String,
    },
    /// The same `<tile_type>.<feature>` appears twice; lookup would be
    /// ambiguous, so the load fails instead of picking one.
    DuplicateFeature {
        line_no: usize,
        tile_type: String,
        feature: String,
    },
}

impl fmt::Display for SegbitsLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "cannot read segbits file: {}", msg),
            Self::MalformedLine {
                line_no,
                line,
                reason,
            } => write!(
                f,
                "segbits line {} is malformed ({}): {:?}",
                line_no, reason, line
            ),
            Self::DuplicateFeature {
                line_no,
                tile_type,
                feature,
            } => write!(
                f,
                "segbits line {} duplicates feature {}.{}",
                line_no, tile_type, feature
            ),
        }
    }
}

impl std::error::Error for SegbitsLoadError {}

/// A segbits database loaded from one FASM-format segbits file.
///
/// Keys are `(tile_type, feature)` pairs; values are the bit refs in file
/// order. Parsing is strict: every non-comment line must be well formed.
#[derive(Debug)]
pub struct FileSegbitsDb {
    entries: HashMap<(String, String), Vec<BitRef>>,
    tile_types: Vec<String>,
    source: String,
}

impl FileSegbitsDb {
    /// Load from a file on disk.
    pub fn from_file(path: &Path) -> Result<Self, SegbitsLoadError> {
        let text = fs::read_to_string(path)
            .map_err(|e| SegbitsLoadError::Io(format!("{}: {}", path.display(), e)))?;
        Self::from_text(&text, &path.display().to_string())
    }

    /// Parse segbits text. `source` names the origin for error messages
    /// (file path, or a test fixture label).
    pub fn from_text(text: &str, source: &str) -> Result<Self, SegbitsLoadError> {
        let mut entries: HashMap<(String, String), Vec<BitRef>> = HashMap::new();
        let mut tile_types: Vec<String> = Vec::new();
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut tokens = line.split_whitespace();
            let head = tokens
                .next()
                .ok_or_else(|| SegbitsLoadError::MalformedLine {
                    line_no,
                    line: raw.to_string(),
                    reason: "empty feature token".to_string(),
                })?;
            let (tile_type, feature) =
                head.split_once('.')
                    .ok_or_else(|| SegbitsLoadError::MalformedLine {
                        line_no,
                        line: raw.to_string(),
                        reason: "feature token has no tile-type prefix".to_string(),
                    })?;
            if tile_type.is_empty() || feature.is_empty() {
                return Err(SegbitsLoadError::MalformedLine {
                    line_no,
                    line: raw.to_string(),
                    reason: "empty tile type or feature".to_string(),
                });
            }
            let mut bits: Vec<BitRef> = Vec::new();
            for tok in tokens {
                bits.push(parse_bit_token(tok).map_err(|reason| {
                    SegbitsLoadError::MalformedLine {
                        line_no,
                        line: raw.to_string(),
                        reason,
                    }
                })?);
            }
            if bits.is_empty() {
                return Err(SegbitsLoadError::MalformedLine {
                    line_no,
                    line: raw.to_string(),
                    reason: "feature has no bit tokens".to_string(),
                });
            }
            let key = (tile_type.to_string(), feature.to_string());
            if entries.contains_key(&key) {
                return Err(SegbitsLoadError::DuplicateFeature {
                    line_no,
                    tile_type: tile_type.to_string(),
                    feature: feature.to_string(),
                });
            }
            if !tile_types.iter().any(|t| t == tile_type) {
                tile_types.push(tile_type.to_string());
            }
            entries.insert(key, bits);
        }
        tile_types.sort();
        Ok(Self {
            entries,
            tile_types,
            source: source.to_string(),
        })
    }

    /// Where this database was loaded from (path or fixture label).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Number of `(tile_type, feature)` entries.
    pub fn feature_count(&self) -> usize {
        self.entries.len()
    }
}

impl SegbitsDb for FileSegbitsDb {
    fn lookup(&self, tile_type: &str, feature: &str) -> Option<&[BitRef]> {
        self.entries
            .get(&(tile_type.to_string(), feature.to_string()))
            .map(Vec::as_slice)
    }

    fn tile_types(&self) -> &[String] {
        &self.tile_types
    }
}

/// Parse one bit token: `!`?`<frame>_<bit>`. Frame is a u32; bit is a u16.
/// Observed range in zynqusp/segbits_int.db is 0..47, but the vendored
/// HPIO/HDIO DBs run to bit 1390, so the parser accepts the full u16
/// range and lets the DB be the authority, rather than hard-coding the
/// observed range.
fn parse_bit_token(tok: &str) -> Result<BitRef, String> {
    let (must_be_zero, body) = match tok.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, tok),
    };
    let (frame_s, bit_s) = body.split_once('_').ok_or_else(|| {
        format!(
            "bit token {:?} has no '_' separator (want <frame>_<bit>)",
            tok
        )
    })?;
    if frame_s.is_empty() || bit_s.is_empty() {
        return Err(format!("bit token {:?} has an empty frame or bit", tok));
    }
    if frame_s.contains('_') || bit_s.contains('_') {
        return Err(format!("bit token {:?} has extra '_' separators", tok));
    }
    let frame: u32 = frame_s
        .parse()
        .map_err(|_| format!("bit token {:?} has a non-numeric frame", tok))?;
    let bit_n: u32 = bit_s
        .parse()
        .map_err(|_| format!("bit token {:?} has a non-numeric bit", tok))?;
    if bit_n > u16::MAX as u32 {
        return Err(format!(
            "bit token {:?} has bit {} outside 0..65536",
            tok, bit_n
        ));
    }
    Ok(BitRef {
        frame,
        bit: bit_n as u16,
        must_be_zero,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two verbatim lines from prjuray-db zynqusp/segbits_int.db
    /// (commit affbc5e, CC0 1.0), used as the unit-test fixture.
    const FIXTURE: &str = "INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00 !72_00 !72_01 !74_00\n\
         INT.PIP.INODE_E_1_FT1.BYPASS_E4 !57_00 !59_00 !61_00 65_01 !67_01 !69_01\n";

    #[test]
    fn bounce_example_parses_to_exact_bits() {
        // Hand-checked against the DB line:
        // INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00 !72_00 !72_01 !74_00
        let db = FileSegbitsDb::from_text(FIXTURE, "fixture").unwrap();
        let bits = db
            .lookup("INT", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
            .expect("BOUNCE PIP must resolve");
        assert_eq!(
            bits,
            &[
                BitRef::set(64, 0),
                BitRef::zero(66, 0),
                BitRef::zero(68, 0),
                BitRef::zero(72, 0),
                BitRef::zero(72, 1),
                BitRef::zero(74, 0),
            ]
        );
        assert_eq!(db.tile_types(), &["INT".to_string()]);
        assert_eq!(db.feature_count(), 2);
    }

    #[test]
    fn bang_tokens_parse_to_must_be_zero() {
        let db = FileSegbitsDb::from_text(FIXTURE, "fixture").unwrap();
        let bits = db.lookup("INT", "PIP.INODE_E_1_FT1.BYPASS_E4").unwrap();
        let set: Vec<_> = bits.iter().filter(|b| !b.must_be_zero).collect();
        let zero: Vec<_> = bits.iter().filter(|b| b.must_be_zero).collect();
        assert_eq!(set, vec![&BitRef::set(65, 1)]);
        assert_eq!(zero.len(), 5);
    }

    #[test]
    fn unknown_feature_returns_none() {
        let db = FileSegbitsDb::from_text(FIXTURE, "fixture").unwrap();
        assert!(db.lookup("INT", "PIP.NOPE.NOPE").is_none());
        assert!(
            db.lookup("CLB", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
                .is_none()
        );
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let text = "# a comment\n\n".to_string() + FIXTURE;
        let db = FileSegbitsDb::from_text(&text, "fixture").unwrap();
        assert_eq!(db.feature_count(), 2);
    }

    #[test]
    fn malformed_lines_fail_loud() {
        for bad in [
            "INT.PIP.A.B 64_XX\n",
            "INT.PIP.A.B 64_65536\n",
            "INT.PIP.A.B 6400\n",
            "INT.PIP.A.B\n",
            "INTPIP 64_00\n",
            "INT.PIP.A.B 64_00 !\n",
        ] {
            let err = FileSegbitsDb::from_text(bad, "bad").unwrap_err();
            assert!(
                matches!(err, SegbitsLoadError::MalformedLine { .. }),
                "line {:?} must fail as malformed, got {:?}",
                bad,
                err
            );
        }
    }

    #[test]
    fn duplicate_feature_fails_loud() {
        let text = format!("{}{}", FIXTURE.lines().next().unwrap(), "\n")
            + FIXTURE.lines().next().unwrap()
            + "\n";
        let err = FileSegbitsDb::from_text(&text, "dup").unwrap_err();
        assert!(
            matches!(err, SegbitsLoadError::DuplicateFeature { .. }),
            "duplicate must fail, got {:?}",
            err
        );
    }

    #[test]
    fn missing_file_fails_loud() {
        let err = FileSegbitsDb::from_file(std::path::Path::new("/nonexistent/segbits_int.db"))
            .unwrap_err();
        assert!(matches!(err, SegbitsLoadError::Io(_)));
    }

    #[test]
    fn bit_width_above_u8_parses_exactly() {
        // Regression test for the u8 to u16 widening (segbits-vendor).
        // Verbatim line from vendored segbits_hdio_bot_right.db: the bit
        // offset 1310 does not fit u8, and the ported loader rejected it.
        let text =
            "HDIO_BOT_RIGHT.PIP.HDIO_LOGICPAIR_50_OPFFS_Q.HDIO_LOGICPAIR_50_OPFFS_D1 00_1310\n";
        let db = FileSegbitsDb::from_text(text, "fixture").unwrap();
        let bits = db
            .lookup(
                "HDIO_BOT_RIGHT",
                "PIP.HDIO_LOGICPAIR_50_OPFFS_Q.HDIO_LOGICPAIR_50_OPFFS_D1",
            )
            .unwrap();
        assert_eq!(bits, &[BitRef::set(0, 1310)]);
    }

    #[test]
    fn all_vendored_dbs_load() {
        // Regression test for the u8 to u16 widening (segbits-vendor).
        // Loads every vendored DB file from repo data and checks the
        // counts from data/PROVENANCE.md. The HPIO and HDIO files fail
        // to load under a u8 bit type (max bit 1390), so this test
        // fails before the widening and passes after it.
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let mut total = 0;
        for (name, count) in [
            ("segbits_int.db", 3778),
            ("segbits_hpio_right.db", 1560),
            ("segbits_hdio_top_right.db", 812),
            ("segbits_hdio_bot_right.db", 828),
            ("segbits_cmt_right.db", 104),
        ] {
            let db = FileSegbitsDb::from_file(&data.join(name))
                .unwrap_or_else(|e| panic!("vendored {name} must load: {e}"));
            assert_eq!(db.feature_count(), count, "feature count for {name}");
            total += count;
        }
        assert_eq!(total, 7082, "total vendored features");
        let db = FileSegbitsDb::from_file(&data.join("segbits_int.db")).unwrap();
        assert_eq!(db.tile_types(), &["INT".to_string()]);
        let db = FileSegbitsDb::from_file(&data.join("segbits_hpio_right.db")).unwrap();
        assert_eq!(db.tile_types(), &["HPIO_RIGHT".to_string()]);
    }
}
