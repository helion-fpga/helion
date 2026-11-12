//! On-disk ECO sessions: an [`crate::eco`] session that survives restarts.
//!
//! Layout under `<project>/.helion/eco/` ([`eco_dir`]):
//!
//! - `base.hck2` — the implemented design the ECO session was opened on
//!   (its bitstream is the pre-ECO image every incremental `.pbits` diffs
//!   against);
//! - `current.hck2` — the edited session after the last change (absent
//!   until the first change);
//! - `journal.txt` — part, pending place/route flag, every applied op with
//!   its result, and the last written ECO bitstream ([`EcoJournal`]).
//!
//! Both checkpoints are v2 `.hck2` files (exact netlist + placement +
//! routing + bitstream, no re-implementation on load), so a restored
//! session continues exactly where the previous process stopped,
//! including a netlist edit whose place/route is still pending. The v1
//! `.hckp` format re-implements from the netlist on open and therefore
//! cannot hold an ECO'd placement.

use std::path::{Path, PathBuf};

use crate::eco::EcoOp;
use crate::{CheckpointSummary, Session, load_checkpoint, save_checkpoint_file};
use helion_bits::Bitstream;
use helion_device::Device;
use helion_sta::Constraints;

const BASE: &str = "base.hck2";
const CURRENT: &str = "current.hck2";
const JOURNAL: &str = "journal.txt";
const MAGIC: &str = "helion-eco 1";

/// `<project_dir>/.helion/eco`, the per-project ECO state directory.
pub fn eco_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".helion").join("eco")
}

/// One applied op and its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcoEntry {
    pub op: EcoOp,
    pub result: Result<String, String>,
}

/// The last ECO bitstream written from the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcoOutputRecord {
    pub bit: PathBuf,
    pub pbits: PathBuf,
    pub full_frames: usize,
    pub changed_frames: usize,
}

/// Everything about an ECO session that is not in its checkpoints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EcoJournal {
    pub part: String,
    /// Structural edits not yet placed and routed.
    pub pending: bool,
    pub entries: Vec<EcoEntry>,
    pub output: Option<EcoOutputRecord>,
}

/// A session restored by [`load_eco`].
pub struct StoredEco {
    pub journal: EcoJournal,
    pub session: Session,
    /// Pre-ECO bitstream (from `base.hck2`).
    pub base: Bitstream,
    /// Modification time of the newest state file (seconds since epoch).
    pub saved_unix: u64,
}

fn word(w: &str, what: &str) -> Result<String, String> {
    if w.is_empty() || w.chars().any(char::is_whitespace) {
        Err(format!("eco journal: {what} {w:?} must be one word"))
    } else {
        Ok(w.to_string())
    }
}

/// Compact one-line record of an op (inverse of [`op_from_record`]).
pub fn op_record(op: &EcoOp) -> String {
    match op {
        EcoOp::SetLutInit { cell, init } => format!("lut_init {cell} {init:016X}"),
        EcoOp::ReconnectPin { cell, pin, net } => format!("reconnect {cell} {pin} {net}"),
        EcoOp::InsertBuffer { net, name } => format!("buffer {net} {name}"),
        EcoOp::AddLut { name, init } => format!("add_lut {name} {init:016X}"),
        EcoOp::AddFf { name, clk } => format!("add_ff {name} {clk}"),
        EcoOp::RemoveCell { cell } => format!("remove {cell}"),
    }
}

/// Parse an [`op_record`] line.
pub fn op_from_record(rec: &str) -> Result<EcoOp, String> {
    let w: Vec<&str> = rec.split_whitespace().collect();
    let arg = |i: usize| -> Result<String, String> {
        w.get(i)
            .map(|s| s.to_string())
            .ok_or_else(|| format!("eco journal: truncated op {rec:?}"))
    };
    let hex = |i: usize| -> Result<u64, String> {
        u64::from_str_radix(&arg(i)?, 16).map_err(|_| format!("eco journal: bad INIT in {rec:?}"))
    };
    let op = match w.first().copied() {
        Some("lut_init") => EcoOp::SetLutInit { cell: arg(1)?, init: hex(2)? },
        Some("reconnect") => EcoOp::ReconnectPin { cell: arg(1)?, pin: arg(2)?, net: arg(3)? },
        Some("buffer") => EcoOp::InsertBuffer { net: arg(1)?, name: arg(2)? },
        Some("add_lut") => EcoOp::AddLut { name: arg(1)?, init: hex(2)? },
        Some("add_ff") => EcoOp::AddFf { name: arg(1)?, clk: arg(2)? },
        Some("remove") => EcoOp::RemoveCell { cell: arg(1)? },
        _ => return Err(format!("eco journal: unknown op {rec:?}")),
    };
    Ok(op)
}

fn one_line(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl EcoJournal {
    pub fn encode(&self) -> Result<String, String> {
        let mut out = format!("{MAGIC}\npart {}\npending {}\n", word(&self.part, "part")?, u8::from(self.pending));
        for e in &self.entries {
            let rec = op_record(&e.op);
            for w in rec.split(' ') {
                word(w, "op field")?;
            }
            let (tag, msg) = match &e.result {
                Ok(m) => ("ok", m),
                Err(m) => ("err", m),
            };
            out.push_str(&format!("op\t{tag}\t{rec}\t{}\n", one_line(msg)));
        }
        if let Some(o) = &self.output {
            out.push_str(&format!(
                "output\t{}\t{}\t{}\t{}\n",
                o.full_frames,
                o.changed_frames,
                one_line(&o.bit.display().to_string()),
                one_line(&o.pbits.display().to_string())
            ));
        }
        Ok(out)
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            return Err("eco journal: not a helion ECO journal".into());
        }
        let mut j = EcoJournal::default();
        for line in lines {
            if let Some(p) = line.strip_prefix("part ") {
                j.part = p.trim().to_string();
            } else if let Some(p) = line.strip_prefix("pending ") {
                j.pending = p.trim() == "1";
            } else if let Some(rest) = line.strip_prefix("op\t") {
                let f: Vec<&str> = rest.splitn(3, '\t').collect();
                let [tag, rec, msg] = f[..] else {
                    return Err(format!("eco journal: bad op line {line:?}"));
                };
                let op = op_from_record(rec)?;
                let result = match tag {
                    "ok" => Ok(msg.to_string()),
                    "err" => Err(msg.to_string()),
                    _ => return Err(format!("eco journal: bad op result {tag:?}")),
                };
                j.entries.push(EcoEntry { op, result });
            } else if let Some(rest) = line.strip_prefix("output\t") {
                let f: Vec<&str> = rest.splitn(4, '\t').collect();
                let [full, changed, bit, pbits] = f[..] else {
                    return Err(format!("eco journal: bad output line {line:?}"));
                };
                j.output = Some(EcoOutputRecord {
                    bit: PathBuf::from(bit),
                    pbits: PathBuf::from(pbits),
                    full_frames: full.parse().map_err(|_| "eco journal: bad frame count")?,
                    changed_frames: changed.parse().map_err(|_| "eco journal: bad frame count")?,
                });
            } else if !line.trim().is_empty() {
                return Err(format!("eco journal: unknown line {line:?}"));
            }
        }
        Ok(j)
    }
}

fn write_checkpoint_atomic(dir: &Path, name: &str, session: &Session) -> Result<(), String> {
    let tmp = dir.join(format!(".{name}.tmp"));
    save_checkpoint_file(session, &Constraints::default(), &tmp, &CheckpointSummary::default())?;
    std::fs::rename(&tmp, dir.join(name)).map_err(|e| format!("eco save {name}: {e}"))
}

fn write_journal(dir: &Path, journal: &EcoJournal) -> Result<(), String> {
    let tmp = dir.join(format!(".{JOURNAL}.tmp"));
    std::fs::write(&tmp, journal.encode()?).map_err(|e| format!("eco save journal: {e}"))?;
    std::fs::rename(&tmp, dir.join(JOURNAL)).map_err(|e| format!("eco save journal: {e}"))
}

/// Start a fresh on-disk session: `session` (implemented, with its
/// bitstream) becomes the base; earlier edits are discarded.
pub fn save_eco_base(dir: &Path, session: &Session) -> Result<(), String> {
    if session.routed.is_none() || session.bitstream.is_none() {
        return Err("eco save: the base session must be routed with a bitstream".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("eco save {}: {e}", dir.display()))?;
    let _ = std::fs::remove_file(dir.join(CURRENT));
    write_checkpoint_atomic(dir, BASE, session)?;
    write_journal(
        dir,
        &EcoJournal {
            part: session.part.clone(),
            ..Default::default()
        },
    )
}

/// Persist the edited `session` and its `journal` (after a change).
pub fn save_eco_state(dir: &Path, session: &Session, journal: &EcoJournal) -> Result<(), String> {
    if !dir.join(BASE).is_file() {
        return Err(format!("eco save: no base checkpoint in {}", dir.display()));
    }
    write_checkpoint_atomic(dir, CURRENT, session)?;
    write_journal(dir, journal)
}

/// Persist only the journal (e.g. a failed op or a written bitstream).
pub fn save_eco_journal(dir: &Path, journal: &EcoJournal) -> Result<(), String> {
    if !dir.join(BASE).is_file() {
        return Err(format!("eco save: no base checkpoint in {}", dir.display()));
    }
    write_journal(dir, journal)
}

/// Whether `dir` holds a saved ECO session.
pub fn has_eco(dir: &Path) -> bool {
    dir.join(BASE).is_file()
}

/// Read the journal alone (cheap; e.g. to resolve the part before
/// [`load_eco`]).
pub fn read_eco_journal(dir: &Path) -> Result<Option<EcoJournal>, String> {
    let path = dir.join(JOURNAL);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    EcoJournal::decode(&text).map(Some)
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Restore the saved session in `dir` (`None` when there is none).
pub fn load_eco(dir: &Path, dev: &Device) -> Result<Option<StoredEco>, String> {
    let base_path = dir.join(BASE);
    if !base_path.is_file() {
        return Ok(None);
    }
    let base_ck = load_checkpoint(&base_path, dev)?;
    let base = base_ck
        .session
        .bitstream
        .clone()
        .ok_or("eco restore: base checkpoint has no bitstream")?;
    let cur_path = dir.join(CURRENT);
    let session = if cur_path.is_file() {
        load_checkpoint(&cur_path, dev)?.session
    } else {
        base_ck.session
    };
    let journal = read_eco_journal(dir)?.unwrap_or_else(|| EcoJournal {
        part: session.part.clone(),
        ..Default::default()
    });
    let saved_unix = [BASE, CURRENT, JOURNAL]
        .iter()
        .map(|f| mtime(&dir.join(f)))
        .max()
        .unwrap_or(0);
    Ok(Some(StoredEco {
        journal,
        session,
        base,
        saved_unix,
    }))
}

/// Forget the saved session (the directory is removed).
pub fn clear_eco(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("eco clear {}: {e}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;
    use crate::eco::EcoImpl;
    use helion_ir::Design;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("helion-eco-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn op_records_round_trip() {
        for op in [
            EcoOp::SetLutInit { cell: "u_lut0".into(), init: 0xDEAD_BEEF },
            EcoOp::ReconnectPin { cell: "c".into(), pin: "I1".into(), net: "n".into() },
            EcoOp::InsertBuffer { net: "q0".into(), name: "b".into() },
            EcoOp::AddLut { name: "l".into(), init: 2 },
            EcoOp::AddFf { name: "f".into(), clk: "clk".into() },
            EcoOp::RemoveCell { cell: "x".into() },
        ] {
            assert_eq!(op_from_record(&op_record(&op)).unwrap(), op);
        }
        assert!(op_from_record("lut_init c zz").is_err());
        assert!(op_from_record("explode").is_err());
    }

    #[test]
    fn session_survives_a_restart_and_keeps_editing() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let dir = eco_dir(&tmp("restart"));
        assert!(load_eco(&dir, &dev).unwrap().is_none());
        let mut s = Session::new(Mode::Project);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        let base = s.bitstream.clone().unwrap();
        save_eco_base(&dir, &s).unwrap();
        let mut journal = EcoJournal { part: s.part.clone(), ..Default::default() };
        for op in [
            EcoOp::SetLutInit { cell: "u_lut0".into(), init: 0x1 },
            EcoOp::RemoveCell { cell: "nope".into() },
            EcoOp::InsertBuffer { net: "q1".into(), name: "eco_buf".into() },
        ] {
            let result = s.apply_eco(&dev, &op);
            journal.pending |= result.is_ok() && op.is_structural();
            journal.entries.push(EcoEntry { op, result });
        }
        save_eco_state(&dir, &s, &journal).unwrap();
        drop(s);

        let st = load_eco(&dir, &dev).unwrap().expect("saved session");
        assert_eq!(st.journal, journal);
        assert!(st.journal.pending);
        assert!(st.journal.entries[1].result.is_err());
        assert_eq!(st.base.frames, base.frames, "the pre-ECO image is kept");
        assert!(st.saved_unix > 0);
        let mut s = st.session;
        let d = s.design.as_ref().unwrap();
        assert!(d.cell("eco_buf").is_some(), "the pending edit survived");
        assert!(matches!(d.cell("u_lut0").unwrap().kind, helion_ir::CellKind::Lut6 { init: 1 }));
        assert!(s.routed.is_none() && s.placed.is_some(), "place/route still pending");
        s.eco_implement(&dev, EcoImpl::PlaceAndRoute).unwrap();
        let out = s.write_eco_bitstream(&dev, &st.base).unwrap();
        assert!(!out.incremental.frames.is_empty());

        journal.pending = false;
        journal.output = Some(EcoOutputRecord {
            bit: "/p/out/top_eco2.bit".into(),
            pbits: "/p/out/top eco2.pbits".into(),
            full_frames: out.full.frames.len(),
            changed_frames: out.incremental.frames.len(),
        });
        save_eco_state(&dir, &s, &journal).unwrap();
        let again = load_eco(&dir, &dev).unwrap().unwrap();
        assert_eq!(again.journal.output, journal.output);
        assert!(again.session.routed.is_some());
        assert_eq!(
            again.session.bitstream.as_ref().unwrap().frames,
            s.bitstream.as_ref().unwrap().frames,
            "the routed ECO image restores bit-exact"
        );

        // Re-opening on a fresh implementation discards the old edits.
        save_eco_base(&dir, &again.session).unwrap();
        let fresh = load_eco(&dir, &dev).unwrap().unwrap();
        assert!(fresh.journal.entries.is_empty());
        clear_eco(&dir).unwrap();
        assert!(!has_eco(&dir));
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn journal_rejects_garbage() {
        assert!(EcoJournal::decode("nope").is_err());
        assert!(EcoJournal::decode(&format!("{MAGIC}\nop\tok\n")).is_err());
        assert!(EcoJournal::decode(&format!("{MAGIC}\nwhat\n")).is_err());
    }
}
