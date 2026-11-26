//! The Project X-Ray database for Xilinx 7-series parts, downloaded on
//! request.
//!
//! Helion's 7-series flow reads the device data (tile grids, wiring,
//! configuration bits, package pins) from the open Project X-Ray database.
//! It is several hundred megabytes, so it does not ship inside the app: the
//! IDE and the command line offer to download it, and nothing here runs
//! unless the user said yes.
//!
//! What is downloaded is pinned: one commit of [`REPO`] ([`COMMIT`], licence
//! [`LICENSE`]), as GitHub's archive of that commit. Every file is hashed
//! while it is unpacked, and each family's files must add up to the digest
//! recorded in [`FAMILIES`]. A download that is cut short, corrupted or
//! altered is refused.
//!
//! Nothing is put in place before it is verified. The archive is unpacked
//! into a temporary directory inside the database root and each family is
//! moved into place only after its digest matches, so the flow never sees
//! half a database. A family directory that Helion did not install (the
//! user's own checkout) is left alone unless the caller asks to replace it.
//!
//! The unpacker takes only regular files and directories under the wanted
//! family names. An archive entry with an absolute path, a `..`, a link or
//! a device node fails the install.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::UpdateError;

/// GitHub repository the database comes from.
pub const REPO: &str = "openXC7/prjxray-db";
/// The commit Helion's 7-series flow is verified against
/// (`crates/helion-xilinx7/DB.md`).
pub const COMMIT: &str = "0c3504f26f5cf77b6a7605c7bc83ebe1ac67b26f";
/// The database's licence (SPDX).
pub const LICENSE: &str = "CC0-1.0";
/// Size of the archive GitHub serves for [`COMMIT`], for progress display.
/// GitHub does not announce it, and it can change by a few bytes when
/// GitHub recompresses; it is never used to verify anything.
pub const ARCHIVE_BYTES: u64 = 43_720_890;

/// Marker file Helion writes into a family directory it installed.
const MARKER: &str = ".helion-xray-db";
/// Largest single file accepted from the archive (the biggest in the
/// pinned families is under 40 MB).
const MAX_FILE_BYTES: u64 = 256 << 20;
/// Largest total unpacked size accepted.
const MAX_TOTAL_BYTES: u64 = 4 << 30;

/// One device family of the database: a top-level directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Family {
    /// Directory name (`artix7`).
    pub name: &'static str,
    /// Regular files under it at [`COMMIT`].
    pub files: usize,
    /// Their total size in bytes.
    pub bytes: u64,
    /// SHA-256 over the sorted `"<sha256 of file>  <path>\n"` lines of its
    /// files, paths relative to the database root with `/` separators
    /// ([`family_digest`]). Regenerate with
    /// `cargo run -p helion-update --example xray_db_digest -- <db root>`.
    pub digest: &'static str,
}

/// The families Helion's boards use, at [`COMMIT`].
pub const FAMILIES: &[Family] = &[
    Family { name: "artix7", files: 774, bytes: 194304013, digest: "63481ccd46107039b501d8c7509e649221e3580f1d10651d607f8d2a54988bd8" },
    Family { name: "spartan7", files: 605, bytes: 107815505, digest: "673c52e15e8c9eb65beee943c8b32cc8733a26bc5f6459d7de7af255f0257541" },
    Family { name: "zynq7", files: 706, bytes: 272491312, digest: "e813e3eeac0f31da3ff7412b183bba5c1f680f29388e33268ef08ae94590c1fa" },
];

/// Where Helion installs the database: `HELION_XILINX_DB` when set, else
/// `~/.cache/helion/prjxray-db`, the first place the 7-series flow looks
/// (`helion_xilinx7::device::default_db_root`). `None` with no home directory.
pub fn install_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HELION_XILINX_DB") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".cache/helion/prjxray-db"))
}

/// The database family a 7-series part is described in (`xc7z020clg484-1`:
/// zynq7), or `None` for a part outside [`FAMILIES`].
pub fn family_for_part(part: &str) -> Option<&'static Family> {
    let p = part.to_ascii_lowercase();
    let name = [("xc7z", "zynq7"), ("xc7a", "artix7"), ("xc7s", "spartan7")].into_iter().find(|(pre, _)| p.starts_with(pre))?.1;
    FAMILIES.iter().find(|f| f.name == name)
}

/// Total unpacked size of `families`, for the consent prompt.
pub fn unpacked_bytes(families: &[Family]) -> u64 {
    families.iter().map(|f| f.bytes).sum()
}

/// URL of the pinned archive.
pub fn archive_url() -> String {
    format!("https://codeload.github.com/{REPO}/tar.gz/{COMMIT}")
}

/// What is in a family's directory under a database root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No such directory.
    Missing,
    /// Helion installed it from the pinned commit and verified it.
    Installed,
    /// Something else is there: the user's own checkout or another commit.
    /// Helion uses it but does not vouch for it, and never replaces it
    /// unless asked.
    Other,
}

/// The state of `family` under `root`. Cheap: it reads the marker file and
/// does not hash the tree (see [`verify_family`]).
pub fn state(root: &Path, family: &Family) -> State {
    let dir = root.join(family.name);
    if !dir.is_dir() {
        return State::Missing;
    }
    match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(text) if text == marker_text(family) => State::Installed,
        _ => State::Other,
    }
}

fn marker_text(family: &Family) -> String {
    format!("{REPO}@{COMMIT} {}\n", family.digest)
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A family's digest from its files' `(path, sha256 hex)` pairs.
fn digest_of_lines(files: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    for (path, sum) in files {
        h.update(format!("{sum}  {path}\n").as_bytes());
    }
    hex(h.finalize())
}

/// Hash every regular file of `family_name` under `root`: the digest, the
/// file count and the total size. The marker file is not part of it.
pub fn family_digest(root: &Path, family_name: &str) -> Result<(String, usize, u64), UpdateError> {
    let mut files = BTreeMap::new();
    let mut bytes = 0u64;
    let mut stack = vec![root.join(family_name)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                if dir == root.join(family_name) && entry.file_name() == MARKER {
                    continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .map_err(|e| UpdateError::Io(e.to_string()))?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let mut h = Sha256::new();
                bytes += std::io::copy(&mut File::open(&path)?, &mut h)?;
                files.insert(rel, hex(h.finalize()));
            } else {
                return Err(UpdateError::Install(format!("{} is not a regular file", path.display())));
            }
        }
    }
    Ok((digest_of_lines(&files), files.len(), bytes))
}

/// Re-hash an installed family and compare it with the pinned digest.
pub fn verify_family(root: &Path, family: &Family) -> Result<bool, UpdateError> {
    let (digest, files, _) = family_digest(root, family.name)?;
    Ok(digest == family.digest && files == family.files)
}

/// A step of [`fetch`], for progress display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Download,
    Unpack,
    Install,
}

/// Progress of [`fetch`]: `done` of `total` bytes in `phase` (`total` is 0
/// when it is not known).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

/// What [`fetch`] did with each family it was asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Downloaded, verified and put in place.
    pub installed: Vec<&'static str>,
    /// Already installed by Helion from the pinned commit.
    pub kept: Vec<&'static str>,
    /// A directory Helion did not install is there, and `replace` was off.
    pub left_alone: Vec<&'static str>,
}

/// Counts what is written and stops when the caller cancels.
struct Counted<'a, W: Write> {
    inner: W,
    done: u64,
    total: u64,
    phase: Phase,
    on_progress: &'a mut dyn FnMut(Progress) -> bool,
}

impl<W: Write> Write for Counted<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.done += n as u64;
        if !(self.on_progress)(Progress { phase: self.phase, done: self.done, total: self.total }) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, CANCELLED));
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

const CANCELLED: &str = "cancelled";

/// Which of `families` an install would fetch, keep or leave alone.
fn plan(root: &Path, families: &[Family], replace: bool) -> (Vec<Family>, Report) {
    let mut wanted = Vec::new();
    let mut report = Report::default();
    for f in families {
        match state(root, f) {
            State::Installed => report.kept.push(f.name),
            State::Other if !replace => report.left_alone.push(f.name),
            State::Missing | State::Other => wanted.push(*f),
        }
    }
    (wanted, report)
}

/// Download the pinned database and install `families` under `root`.
///
/// `on_progress` is called as bytes arrive and files are unpacked; returning
/// `false` cancels, and then (as after any error) `root` holds exactly what
/// it held before. `replace` also replaces a family directory that Helion
/// did not install.
///
/// The caller must have the user's consent: this downloads several hundred
/// megabytes.
pub fn fetch(root: &Path, families: &[Family], replace: bool, on_progress: &mut dyn FnMut(Progress) -> bool) -> Result<Report, UpdateError> {
    let (wanted, report) = plan(root, families, replace);
    if wanted.is_empty() {
        return Ok(report);
    }
    let made_root = !root.exists();
    std::fs::create_dir_all(root)?;
    let result = (|| {
        // Inside `root`, so the final renames stay on one filesystem.
        let stage = tempfile::Builder::new().prefix(".helion-xray-partial-").tempdir_in(root)?;
        let archive = stage.path().join("db.tar.gz");
        {
            let mut out = Counted { inner: File::create(&archive)?, done: 0, total: ARCHIVE_BYTES, phase: Phase::Download, on_progress: &mut *on_progress };
            self_update::Download::from_url(&archive_url()).download_to(&mut out).map_err(|e| {
                let text = e.to_string();
                if text.contains(CANCELLED) { UpdateError::Cancelled } else { UpdateError::Network(text) }
            })?;
            out.flush()?;
        }
        install_archive(File::open(&archive)?, root, stage.path(), &wanted, on_progress)
    })();
    match result {
        Ok(installed) => Ok(Report { installed, ..report }),
        Err(e) => {
            if made_root {
                // Only succeeds when nothing was installed into it.
                let _ = std::fs::remove_dir(root);
            }
            Err(e)
        }
    }
}

/// The path of an archive entry inside the database, when it belongs to one
/// of `wanted`: the archive's top-level directory is dropped, and every
/// remaining component must be a plain name.
fn entry_target(path: &Path, wanted: &[Family]) -> Result<Option<(Family, Vec<String>)>, UpdateError> {
    let bad = || UpdateError::Install(format!("archive entry {} is not a plain relative path", path.display()));
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(bad)?.to_string()),
            Component::CurDir => {}
            _ => return Err(bad()),
        }
    }
    if parts.len() < 2 {
        return Ok(None);
    }
    parts.remove(0);
    Ok(wanted.iter().find(|f| f.name == parts[0]).map(|f| (*f, parts)))
}

/// Unpack the gzip-compressed tar `archive` into `stage`, verify each of
/// `wanted` against its digest, and move the verified families into `root`.
/// Returns the families installed. `stage` must be a directory inside
/// `root`'s filesystem; the caller removes it.
fn install_archive(archive: impl Read, root: &Path, stage: &Path, wanted: &[Family], on_progress: &mut dyn FnMut(Progress) -> bool) -> Result<Vec<&'static str>, UpdateError> {
    let tree = stage.join("tree");
    std::fs::create_dir_all(&tree)?;
    let total: u64 = wanted.iter().map(|f| f.bytes).sum();
    let mut sums: BTreeMap<&'static str, BTreeMap<String, String>> = wanted.iter().map(|f| (f.name, BTreeMap::new())).collect();
    let mut unpacked = 0u64;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| UpdateError::Install(format!("read archive: {e}")))? {
        let mut entry = entry.map_err(|e| UpdateError::Install(format!("read archive: {e}")))?;
        let path = entry.path().map_err(|e| UpdateError::Install(format!("archive entry path: {e}")))?.into_owned();
        let Some((family, parts)) = entry_target(&path, wanted)? else { continue };
        let dest: PathBuf = parts.iter().fold(tree.clone(), |p, c| p.join(c));
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if !kind.is_file() {
            return Err(UpdateError::Install(format!("archive entry {} is a {kind:?}, not a file", path.display())));
        }
        if entry.size() > MAX_FILE_BYTES || unpacked + entry.size() > MAX_TOTAL_BYTES {
            return Err(UpdateError::Install(format!("archive entry {} is larger than the database can be", path.display())));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&dest)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = entry.read(&mut buf).map_err(|e| UpdateError::Install(format!("read archive: {e}")))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            unpacked += n as u64;
            if unpacked > MAX_TOTAL_BYTES {
                return Err(UpdateError::Install("the archive unpacks to more than the database can be".into()));
            }
            if !on_progress(Progress { phase: Phase::Unpack, done: unpacked, total }) {
                return Err(UpdateError::Cancelled);
            }
        }
        if sums.get_mut(family.name).expect("wanted family").insert(parts.join("/"), hex(hasher.finalize())).is_some() {
            return Err(UpdateError::Install(format!("archive has {} twice", path.display())));
        }
    }
    // Verify everything before anything moves.
    for f in wanted {
        let files = &sums[f.name];
        if files.len() != f.files || digest_of_lines(files) != f.digest {
            return Err(UpdateError::ChecksumMismatch(format!("{} ({} of {} files)", f.name, files.len(), f.files)));
        }
    }
    let mut installed = Vec::new();
    for (i, f) in wanted.iter().enumerate() {
        if !on_progress(Progress { phase: Phase::Install, done: i as u64, total: wanted.len() as u64 }) {
            return Err(UpdateError::Cancelled);
        }
        let staged = tree.join(f.name);
        std::fs::write(staged.join(MARKER), marker_text(f))?;
        let target = root.join(f.name);
        if target.exists() {
            // Replacing a directory Helion did not install: it moves into the
            // staging directory (removed with it) only once the new one is
            // ready to take its place, and comes back if that fails.
            let aside = stage.join(format!("replaced-{}", f.name));
            std::fs::rename(&target, &aside)?;
            if let Err(e) = std::fs::rename(&staged, &target) {
                let _ = std::fs::rename(&aside, &target);
                return Err(e.into());
            }
        } else {
            std::fs::rename(&staged, &target)?;
        }
        installed.push(f.name);
    }
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_map_to_their_database_family() {
        assert_eq!(family_for_part("xc7z020clg484-1").map(|f| f.name), Some("zynq7"));
        assert_eq!(family_for_part("XC7A35T").map(|f| f.name), Some("artix7"));
        assert_eq!(family_for_part("xc7s50csga324-1").map(|f| f.name), Some("spartan7"));
        assert!(family_for_part("LFE5U-85F").is_none() && family_for_part("xc7k325t").is_none());
        assert_eq!(unpacked_bytes(FAMILIES), 574_610_830);
    }

    /// A gzip-compressed tar with `files` under a top-level `repo-abc/`.
    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, format!("repo-abc/{path}"), *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    /// The family record a correct install of `files` must match.
    fn family(name: &'static str, files: &[(&str, &[u8])]) -> Family {
        let mine: BTreeMap<String, String> = files
            .iter()
            .filter(|(p, _)| p.starts_with(&format!("{name}/")))
            .map(|(p, d)| (p.to_string(), hex(Sha256::digest(d))))
            .collect();
        let bytes = files.iter().filter(|(p, _)| p.starts_with(&format!("{name}/"))).map(|(_, d)| d.len() as u64).sum();
        Family { name, files: mine.len(), bytes, digest: Box::leak(digest_of_lines(&mine).into_boxed_str()) }
    }

    const FILES: &[(&str, &[u8])] = &[
        ("README.md", b"not part of any family"),
        ("artix7/tilegrid.json", b"{\"grid\": 1}"),
        ("artix7/xc7a35t/part.json", b"{\"idcode\": 1}"),
        ("zynq7/tilegrid.json", b"{\"grid\": 2}"),
        ("kintex7/tilegrid.json", b"a family nobody asked for"),
    ];

    fn install(root: &Path, data: &[u8], wanted: &[Family]) -> Result<Vec<&'static str>, UpdateError> {
        std::fs::create_dir_all(root).unwrap();
        let stage = tempfile::Builder::new().prefix(".helion-xray-partial-").tempdir_in(root).unwrap();
        install_archive(data, root, stage.path(), wanted, &mut |_| true)
    }

    /// Everything under `root`, as sorted relative paths (directories end in `/`).
    fn listing(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                if p.is_dir() {
                    out.push(format!("{rel}/"));
                    walk(&p, root, out);
                } else {
                    out.push(rel);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn a_good_archive_installs_only_the_wanted_families_and_marks_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("db");
        let wanted = [family("artix7", FILES), family("zynq7", FILES)];
        assert_eq!(install(&root, &archive(FILES), &wanted).unwrap(), vec!["artix7", "zynq7"]);
        assert_eq!(
            listing(&root),
            vec!["artix7/", "artix7/.helion-xray-db", "artix7/tilegrid.json", "artix7/xc7a35t/", "artix7/xc7a35t/part.json", "zynq7/", "zynq7/.helion-xray-db", "zynq7/tilegrid.json"]
        );
        assert_eq!(std::fs::read(root.join("artix7/xc7a35t/part.json")).unwrap(), b"{\"idcode\": 1}");
        for f in &wanted {
            assert_eq!(state(&root, f), State::Installed);
            assert!(verify_family(&root, f).unwrap(), "{} re-hashes to its digest", f.name);
        }
        assert_eq!(state(&root, &family("spartan7", FILES)), State::Missing);
    }

    #[test]
    fn a_changed_file_fails_the_install_and_leaves_the_root_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("db");
        let wanted = [family("artix7", FILES), family("zynq7", FILES)];
        let mut tampered = FILES.to_vec();
        tampered[3] = ("zynq7/tilegrid.json", b"{\"grid\": 3}");
        let err = install(&root, &archive(&tampered), &wanted).unwrap_err();
        assert!(matches!(&err, UpdateError::ChecksumMismatch(what) if what.starts_with("zynq7")), "{err}");
        assert!(listing(&root).is_empty(), "nothing installed, not even the family that matched: {:?}", listing(&root));
    }

    #[test]
    fn a_missing_or_extra_file_fails_the_install() {
        let wanted = [family("artix7", FILES)];
        let dir = tempfile::tempdir().unwrap();
        let fewer: Vec<_> = FILES.iter().copied().filter(|(p, _)| *p != "artix7/xc7a35t/part.json").collect();
        assert!(matches!(install(&dir.path().join("a"), &archive(&fewer), &wanted), Err(UpdateError::ChecksumMismatch(_))));
        let mut more = FILES.to_vec();
        more.push(("artix7/extra.db", b"planted"));
        assert!(matches!(install(&dir.path().join("b"), &archive(&more), &wanted), Err(UpdateError::ChecksumMismatch(_))));
    }

    #[test]
    fn a_truncated_archive_fails_the_install() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("db");
        let data = archive(FILES);
        let err = install(&root, &data[..data.len() / 2], &[family("artix7", FILES), family("zynq7", FILES)]).unwrap_err();
        assert!(matches!(err, UpdateError::Install(_) | UpdateError::ChecksumMismatch(_)), "{err}");
        assert!(listing(&root).is_empty());
    }

    /// A tar entry with a raw name, as a hostile archive could carry.
    fn raw_entry(name: &str, kind: tar::EntryType, link: Option<&str>) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut h = tar::Header::new_gnu();
        let bytes = name.as_bytes();
        h.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
        h.set_entry_type(kind);
        h.set_mode(0o644);
        if let Some(l) = link {
            h.set_link_name(l).unwrap();
        }
        let data: &[u8] = if kind.is_file() { b"x" } else { b"" };
        h.set_size(data.len() as u64);
        h.set_cksum();
        tar.append(&h, data).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn an_entry_that_leaves_the_database_or_is_a_link_fails_the_install() {
        let wanted = [family("artix7", FILES)];
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("escaped");
        for (i, data) in [
            raw_entry("repo-abc/artix7/../../../escaped", tar::EntryType::Regular, None),
            raw_entry("repo-abc/artix7/link", tar::EntryType::Symlink, Some("/etc/passwd")),
            raw_entry("repo-abc/artix7/hard", tar::EntryType::Link, Some("repo-abc/artix7/tilegrid.json")),
        ]
        .iter()
        .enumerate()
        {
            let root = dir.path().join(format!("db{i}"));
            let err = install(&root, data, &wanted).unwrap_err();
            assert!(matches!(err, UpdateError::Install(_)), "case {i}: {err}");
            assert!(listing(&root).is_empty(), "case {i} left {:?}", listing(&root));
        }
        assert!(!outside.exists(), "nothing was written outside the database");
        assert!(entry_target(Path::new("/abs/artix7/x"), &wanted).is_err());
        assert!(entry_target(Path::new("repo/artix7/../x"), &wanted).is_err());
        assert_eq!(entry_target(Path::new("repo/kintex7/x"), &wanted).unwrap(), None);
        assert_eq!(entry_target(Path::new("repo/artix7/a/b"), &wanted).unwrap().map(|t| t.1), Some(vec!["artix7".to_string(), "a".into(), "b".into()]));
    }

    #[test]
    fn cancelling_during_unpacking_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("db");
        std::fs::create_dir_all(&root).unwrap();
        let stage = tempfile::Builder::new().prefix(".helion-xray-partial-").tempdir_in(&root).unwrap();
        let err = install_archive(&archive(FILES)[..], &root, stage.path(), &[family("artix7", FILES)], &mut |p| p.phase != Phase::Unpack).unwrap_err();
        assert!(matches!(err, UpdateError::Cancelled), "{err}");
        drop(stage);
        assert!(listing(&root).is_empty());
    }

    #[test]
    fn a_directory_helion_did_not_install_is_left_alone_unless_replacing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("db");
        std::fs::create_dir_all(root.join("artix7")).unwrap();
        std::fs::write(root.join("artix7/mine.txt"), "the user's own checkout").unwrap();
        let artix = family("artix7", FILES);
        assert_eq!(state(&root, &artix), State::Other);
        let (wanted, report) = plan(&root, &[artix, family("zynq7", FILES)], false);
        assert_eq!(wanted.iter().map(|f| f.name).collect::<Vec<_>>(), vec!["zynq7"]);
        assert_eq!(report.left_alone, vec!["artix7"]);
        // Asked to replace: the old directory goes only when the verified one is in place.
        let (wanted, _) = plan(&root, &[artix], true);
        assert_eq!(install(&root, &archive(FILES), &wanted).unwrap(), vec!["artix7"]);
        assert!(!root.join("artix7/mine.txt").exists());
        assert_eq!(state(&root, &artix), State::Installed);
        // And a bad archive does not cost the user their directory.
        let other = dir.path().join("db2");
        std::fs::create_dir_all(other.join("artix7")).unwrap();
        std::fs::write(other.join("artix7/mine.txt"), "keep me").unwrap();
        let mut tampered = FILES.to_vec();
        tampered[1] = ("artix7/tilegrid.json", b"changed");
        assert!(install(&other, &archive(&tampered), &[artix]).is_err());
        assert_eq!(std::fs::read_to_string(other.join("artix7/mine.txt")).unwrap(), "keep me");
    }

    #[test]
    fn an_installed_family_is_kept_and_an_edited_one_no_longer_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("db");
        let artix = family("artix7", FILES);
        install(&root, &archive(FILES), &[artix]).unwrap();
        let (wanted, report) = plan(&root, &[artix], true);
        assert!(wanted.is_empty(), "an installed family is not fetched again");
        assert_eq!(report.kept, vec!["artix7"]);
        std::fs::write(root.join("artix7/tilegrid.json"), "edited").unwrap();
        assert!(!verify_family(&root, &artix).unwrap());
    }

    /// The real thing, over the network (about 100 MB): GitHub's archive of
    /// the pinned commit must unpack to exactly the recorded digests.
    ///
    ///     cargo test -p helion-update -- --ignored the_pinned_database_downloads_and_verifies --nocapture
    #[test]
    #[ignore = "downloads the Project X-Ray database from GitHub"]
    fn the_pinned_database_downloads_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("prjxray-db");
        let mut archive_bytes = 0;
        let report = fetch(&root, FAMILIES, false, &mut |p| {
            if p.phase == Phase::Download {
                archive_bytes = p.done;
            }
            true
        })
        .expect("fetch");
        eprintln!("archive: {archive_bytes} bytes");
        assert_eq!(report.installed, vec!["artix7", "spartan7", "zynq7"]);
        for f in FAMILIES {
            assert_eq!(state(&root, f), State::Installed);
            assert!(verify_family(&root, f).unwrap(), "{} on disk matches its digest", f.name);
        }
        let left: Vec<String> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with('.')).collect();
        assert!(left.is_empty(), "no staging directory left behind: {left:?}");
        // The archive size shown to the user is within 10% of the real one.
        assert!(archive_bytes.abs_diff(ARCHIVE_BYTES) < ARCHIVE_BYTES / 10 + 1, "ARCHIVE_BYTES is {ARCHIVE_BYTES}, the archive is {archive_bytes}");
    }

    #[test]
    fn the_pinned_archive_url_names_the_repository_and_commit() {
        assert_eq!(archive_url(), format!("https://codeload.github.com/{REPO}/tar.gz/{COMMIT}"));
        assert_eq!(COMMIT.len(), 40);
        assert!(COMMIT.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
