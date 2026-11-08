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
    /// All feature names for a tile type, sorted, for parse-back
    /// enumeration. Empty for an unknown tile type. The list enumerates
    /// exactly the features `lookup` resolves for the tile type: no
    /// missing, no extras.
    fn features(&self, tile_type: &str) -> &[String];
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
    features: HashMap<String, Vec<String>>,
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
        let mut features: HashMap<String, Vec<String>> = HashMap::new();
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
            features
                .entry(tile_type.to_string())
                .or_default()
                .push(feature.to_string());
            entries.insert(key, bits);
        }
        tile_types.sort();
        for list in features.values_mut() {
            list.sort();
        }
        Ok(Self {
            entries,
            tile_types,
            features,
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

    /// Load and merge several segbits files into one database. Files are
    /// loaded in order; a feature present in two files fails loud with
    /// [`SegbitsLoadError::DuplicateFeature`] (line 0: no single line
    /// owns a cross-file clash). Intra-file duplicates fail with their
    /// own line number from [`FileSegbitsDb::from_file`].
    pub fn from_files(paths: &[&Path]) -> Result<Self, SegbitsLoadError> {
        let mut merged: Option<Self> = None;
        for path in paths {
            let db = Self::from_file(path)?;
            match merged.as_mut() {
                None => merged = Some(db),
                Some(m) => m.merge(db)?,
            }
        }
        Ok(merged.unwrap_or_else(|| Self {
            entries: HashMap::new(),
            tile_types: Vec::new(),
            features: HashMap::new(),
            source: String::new(),
        }))
    }

    /// Merge another database into this one. Duplicate `(tile_type,
    /// feature)` fails loud; the database is unchanged on failure.
    /// Feature lists stay sorted.
    pub fn merge(&mut self, other: Self) -> Result<(), SegbitsLoadError> {
        for (key, _) in other.entries.iter() {
            if self.entries.contains_key(key) {
                return Err(SegbitsLoadError::DuplicateFeature {
                    line_no: 0,
                    tile_type: key.0.clone(),
                    feature: key.1.clone(),
                });
            }
        }
        for (key, bits) in other.entries {
            if !self.tile_types.iter().any(|t| *t == key.0) {
                self.tile_types.push(key.0.clone());
            }
            self.features
                .entry(key.0.clone())
                .or_default()
                .push(key.1.clone());
            self.entries.insert(key, bits);
        }
        self.tile_types.sort();
        for list in self.features.values_mut() {
            list.sort();
        }
        Ok(())
    }

    /// Synthesize single-feature `X6LUT.INIT[63:0]` entries from the
    /// per-bit `XLUT.INIT[i]` lines of each CLB tile type present.
    ///
    /// The prjuray CLB DBs carry LUT INIT as 64 one-bit features
    /// (`ALUT.INIT[0]` .. `ALUT.INIT[63]`, one set-bit token each),
    /// while the emitter and the harness golden FASM use one 64-bit
    /// feature (`A6LUT.INIT[63:0]`, MSB-first expansion per the bitgen
    /// contract). The synthesized list is MSB-first: list index 0 is
    /// INIT bit 63, list index 63 is INIT bit 0, each taken from the
    /// matching per-bit entry. The two sources agree bit for bit (the
    /// harness cross-checks the same per-bit entries against
    /// BitFiltrator LutLoc on every run), so the synthesis adds no new
    /// encoding, only a regrouping.
    ///
    /// A letter is synthesized only when all 64 per-bit entries exist.
    /// A per-bit entry with anything other than exactly one set-bit
    /// token fails loud: a `!` token or a multi-bit list cannot be one
    /// INIT bit. An already-present `X6LUT.INIT[63:0]` is left alone,
    /// so calling this twice is a no-op. Feature lists stay sorted.
    pub fn synthesize_lut_init(&mut self) -> Result<(), SegbitsLoadError> {
        for tile_type in ["CLEL_L", "CLEL_R", "CLEM", "CLEM_R"] {
            if !self.tile_types.iter().any(|t| t == tile_type) {
                continue;
            }
            for letter in b'A'..=b'H' {
                let letter = letter as char;
                let target = format!("{letter}6LUT.INIT[63:0]");
                if self
                    .entries
                    .contains_key(&(tile_type.to_string(), target.clone()))
                {
                    continue;
                }
                let mut refs: Vec<BitRef> = Vec::with_capacity(64);
                let mut complete = true;
                for i in (0..64).rev() {
                    let per_bit = format!("{letter}LUT.INIT[{i}]");
                    match self.entries.get(&(tile_type.to_string(), per_bit.clone())) {
                        None => {
                            complete = false;
                            break;
                        }
                        Some(bits) => {
                            if bits.len() != 1 || bits[0].must_be_zero {
                                return Err(SegbitsLoadError::MalformedLine {
                                    line_no: 0,
                                    line: format!("{tile_type}.{per_bit}"),
                                    reason: format!(
                                        "LUT INIT bit {i} must list exactly one set bit, found {} token(s)",
                                        bits.len()
                                    ),
                                });
                            }
                            refs.push(bits[0]);
                        }
                    }
                }
                if !complete {
                    continue;
                }
                self.features
                    .entry(tile_type.to_string())
                    .or_default()
                    .push(target.clone());
                self.features
                    .get_mut(tile_type)
                    .expect("tile type entry just pushed")
                    .sort();
                self.entries.insert((tile_type.to_string(), target), refs);
            }
        }
        Ok(())
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

    fn features(&self, tile_type: &str) -> &[String] {
        self.features
            .get(tile_type)
            .map(Vec::as_slice)
            .unwrap_or(&[])
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
    fn features_enumerates_exactly_what_lookup_resolves() {
        // Merge requirement (interfaces session 2 item 3): features()
        // enumerates exactly the features lookup() resolves for the tile
        // type, no missing, no extras, sorted; empty for unknown types.
        let db = FileSegbitsDb::from_text(FIXTURE, "fixture").unwrap();
        assert_eq!(
            db.features("INT"),
            &[
                "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1".to_string(),
                "PIP.INODE_E_1_FT1.BYPASS_E4".to_string(),
            ]
        );
        for feature in db.features("INT") {
            assert!(
                db.lookup("INT", feature).is_some(),
                "listed feature {feature} must resolve"
            );
        }
        assert!(db.features("CLB").is_empty());
        // Vendored INT DB: every listed feature resolves, and the list
        // length equals the entry count, so there are no missing and no
        // extras. (A list built from a different source than lookup
        // would show up here as a count mismatch or a failed lookup.)
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let db = FileSegbitsDb::from_file(&data.join("segbits_int.db")).unwrap();
        assert_eq!(db.features("INT").len(), 3778);
        assert_eq!(db.features("INT").len(), db.feature_count());
        for feature in db.features("INT") {
            assert!(db.lookup("INT", feature).is_some());
        }
    }

    #[test]
    fn lookup_preserves_db_file_order() {
        // The loader must not sort or reorder bit tokens: the bitgen
        // contract requires INIT lists ordered MSB-first, and the DB
        // file order is the only place that order is established.
        // Bits here are deliberately out of numeric order.
        let text = "CLEL_L.A6LUT.INIT[2:0] 12_02 12_00 12_01\n";
        let db = FileSegbitsDb::from_text(text, "fixture").unwrap();
        assert_eq!(
            db.lookup("CLEL_L", "A6LUT.INIT[2:0]").unwrap(),
            &[BitRef::set(12, 2), BitRef::set(12, 0), BitRef::set(12, 1)]
        );
        // The BYPASS fixture line, exact file order including `!` tokens.
        let db = FileSegbitsDb::from_text(FIXTURE, "fixture").unwrap();
        assert_eq!(
            db.lookup("INT", "PIP.INODE_E_1_FT1.BYPASS_E4").unwrap(),
            &[
                BitRef::zero(57, 0),
                BitRef::zero(59, 0),
                BitRef::zero(61, 0),
                BitRef::set(65, 1),
                BitRef::zero(67, 1),
                BitRef::zero(69, 1),
            ]
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

    #[test]
    fn merge_combines_disjoint_dbs_and_rejects_duplicates() {
        let a =
            FileSegbitsDb::from_text("INT.PIP.A.B 64_00\nCLEL_R.AFF.INIT.V0 12_02\n", "a").unwrap();
        let b = FileSegbitsDb::from_text("HPIO_RIGHT.IOB.X.I 02_10\n", "b").unwrap();
        let mut m = a;
        m.merge(b).unwrap();
        assert_eq!(m.feature_count(), 3);
        assert_eq!(m.tile_types(), &["CLEL_R", "HPIO_RIGHT", "INT"]);
        assert!(m.lookup("HPIO_RIGHT", "IOB.X.I").is_some());
        let dup = FileSegbitsDb::from_text("INT.PIP.A.B 65_00\n", "dup").unwrap();
        assert!(matches!(
            m.merge(dup),
            Err(SegbitsLoadError::DuplicateFeature { .. })
        ));
        assert_eq!(m.feature_count(), 3, "failed merge leaves the DB unchanged");
    }

    #[test]
    fn synthesize_lut_init_builds_msb_first_list() {
        // 64 per-bit entries for A, frame i bit (2*i) for INIT bit i.
        let mut text = String::new();
        for i in 0..64 {
            text.push_str(&format!("CLEL_R.ALUT.INIT[{i}] {i:02}_{:02}\n", 2 * i));
        }
        let mut db = FileSegbitsDb::from_text(&text, "fixture").unwrap();
        db.synthesize_lut_init().unwrap();
        let list = db.lookup("CLEL_R", "A6LUT.INIT[63:0]").unwrap().to_vec();
        assert_eq!(list.len(), 64);
        // MSB-first: list index 0 is INIT bit 63, index 63 is INIT bit 0.
        assert_eq!(list[0], BitRef::set(63, 126));
        assert_eq!(list[63], BitRef::set(0, 0));
        assert_eq!(list[1], BitRef::set(62, 124));
        // Idempotent: a second call changes nothing.
        let before = db.feature_count();
        db.synthesize_lut_init().unwrap();
        assert_eq!(db.feature_count(), before);
        assert_eq!(
            db.lookup("CLEL_R", "A6LUT.INIT[63:0]").unwrap(),
            list.as_slice()
        );
    }

    #[test]
    fn synthesize_lut_init_skips_incomplete_letters() {
        let mut text = String::new();
        for i in 0..64 {
            text.push_str(&format!("CLEL_R.ALUT.INIT[{i}] {i:02}_00\n"));
        }
        for i in 0..3 {
            text.push_str(&format!("CLEL_R.BLUT.INIT[{i}] {i:02}_01\n"));
        }
        let mut db = FileSegbitsDb::from_text(&text, "fixture").unwrap();
        db.synthesize_lut_init().unwrap();
        assert!(db.lookup("CLEL_R", "A6LUT.INIT[63:0]").is_some());
        assert!(db.lookup("CLEL_R", "B6LUT.INIT[63:0]").is_none());
    }

    #[test]
    fn synthesize_lut_init_rejects_must_be_zero_per_bit() {
        let mut text = String::new();
        for i in 0..64 {
            if i == 7 {
                text.push_str("CLEL_R.ALUT.INIT[7] !07_00\n");
            } else {
                text.push_str(&format!("CLEL_R.ALUT.INIT[{i}] {i:02}_00\n"));
            }
        }
        let mut db = FileSegbitsDb::from_text(&text, "fixture").unwrap();
        assert!(matches!(
            db.synthesize_lut_init(),
            Err(SegbitsLoadError::MalformedLine { .. })
        ));
    }

    #[test]
    fn vendored_clb_dbs_load_and_synthesize() {
        // The four CLB DBs vendored by the emitter worker (PROVENANCE.md).
        // Counts are the upstream line counts from the same table.
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let mut total = 0;
        for (name, count) in [
            ("segbits_clel_l.db", 768),
            ("segbits_clel_r.db", 768),
            ("segbits_clem.db", 829),
            ("segbits_clem_r.db", 829),
        ] {
            let db = FileSegbitsDb::from_file(&data.join(name))
                .unwrap_or_else(|e| panic!("vendored {name} must load: {e}"));
            assert_eq!(db.feature_count(), count, "feature count for {name}");
            total += count;
        }
        assert_eq!(total, 3194, "total vendored CLB features");
        let mut db = FileSegbitsDb::from_files(&[
            &data.join("segbits_clel_l.db"),
            &data.join("segbits_clel_r.db"),
            &data.join("segbits_clem.db"),
            &data.join("segbits_clem_r.db"),
        ])
        .unwrap();
        assert_eq!(db.feature_count(), 3194);
        db.synthesize_lut_init().unwrap();
        // 4 tile types x 8 LUT letters, each a 64-bit MSB-first list.
        for tile_type in ["CLEL_L", "CLEL_R", "CLEM", "CLEM_R"] {
            for letter in ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'] {
                let feat = format!("{letter}6LUT.INIT[63:0]");
                let list = db
                    .lookup(tile_type, &feat)
                    .unwrap_or_else(|| panic!("{tile_type}.{feat} must synthesize"));
                assert_eq!(list.len(), 64, "{tile_type}.{feat} length");
                assert!(
                    list.iter().all(|r| !r.must_be_zero),
                    "{tile_type}.{feat} must list set bits only"
                );
            }
        }
        // Cross-check against the BitFiltrator LutLoc table (bitloc.rs):
        // the synthesized list for CLEL_R A6LUT, read MSB-first, must
        // equal the table's (minor, frame_ofst) pairs in INIT-bit order.
        let locs = crate::bitloc::lut_locations(crate::bitloc::ClbTileType::ClelR, 0, 0).unwrap();
        let list = db.lookup("CLEL_R", "A6LUT.INIT[63:0]").unwrap();
        for i in 0..64 {
            let r = list[63 - i];
            assert_eq!(r.frame, locs[i].minor, "LUT INIT bit {i} minor");
            assert_eq!(r.bit as u32, locs[i].frame_ofst, "LUT INIT bit {i} offset");
        }
    }
}
