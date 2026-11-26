//! Downloading the Project X-Ray database, for the part picker's Download
//! button and the command line. Nothing here runs unless the user asked:
//! the button, `--fetch-xray-db`, or a yes at the terminal prompt.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use helion_update::xray_db::{self, Family, Phase, State};

/// A download in flight: the worker writes, the UI reads every frame.
#[derive(Debug, Default)]
pub struct Job {
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// Past the download, unpacking and verifying.
    pub unpacking: AtomicBool,
    pub cancel: AtomicBool,
}

impl Job {
    /// One status line, e.g. "Downloading 12 of 44 MB".
    pub fn line(&self) -> String {
        let mb = |b: u64| b.div_ceil(1_000_000);
        let (done, total) = (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        if self.unpacking.load(Ordering::Relaxed) {
            format!("Unpacking and verifying: {} of {} MB", mb(done), mb(total))
        } else {
            format!("Downloading {} of {} MB", mb(done), mb(total.max(done)))
        }
    }
}

/// Families with no directory under `root`. A family the user put there
/// themselves (another checkout) counts as present and is never replaced.
pub fn missing(root: &Path) -> Vec<Family> {
    xray_db::FAMILIES.iter().filter(|f| xray_db::state(root, f) == State::Missing).copied().collect()
}

/// What the user agrees to: size, licence, source and destination.
pub fn consent(root: &Path, families: &[Family]) -> String {
    let names: Vec<&str> = families.iter().map(|f| f.name).collect();
    format!(
        "Download the Project X-Ray database ({}): {} MB, {} MB once unpacked, licence {}, from github.com/{} at {}, into {}",
        names.join(", "),
        xray_db::ARCHIVE_BYTES.div_ceil(1_000_000),
        xray_db::unpacked_bytes(families).div_ceil(1_000_000),
        xray_db::LICENSE,
        xray_db::REPO,
        &xray_db::COMMIT[..12],
        root.display()
    )
}

/// Download and install `families` under `root`, reporting into `job`.
/// The Ok line says what was installed.
pub fn fetch(root: &Path, families: &[Family], job: &Job) -> Result<String, String> {
    let report = xray_db::fetch(root, families, false, &mut |p| {
        job.done.store(p.done, Ordering::Relaxed);
        job.total.store(p.total, Ordering::Relaxed);
        job.unpacking.store(p.phase != Phase::Download, Ordering::Relaxed);
        !job.cancel.load(Ordering::Relaxed)
    })
    .map_err(|e| format!("Project X-Ray database: {e}"))?;
    let mut parts = Vec::new();
    if !report.installed.is_empty() {
        parts.push(format!("installed and verified {}", report.installed.join(", ")));
    }
    if !report.kept.is_empty() {
        parts.push(format!("already installed {}", report.kept.join(", ")));
    }
    if !report.left_alone.is_empty() {
        parts.push(format!("left your own {} as it is", report.left_alone.join(", ")));
    }
    Ok(format!("Project X-Ray database in {}: {}", root.display(), parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_consent_line_states_size_licence_source_and_place() {
        let fams = &xray_db::FAMILIES[..1];
        let line = consent(Path::new("/db"), fams);
        for want in ["artix7", "44 MB", "195 MB once unpacked", "CC0-1.0", "github.com/openXC7/prjxray-db", "0c3504f26f5c", "into /db"] {
            assert!(line.contains(want), "{want} missing from {line}");
        }
    }

    #[test]
    fn progress_reads_in_megabytes_per_phase() {
        let j = Job::default();
        j.done.store(12_000_000, Ordering::Relaxed);
        j.total.store(43_720_890, Ordering::Relaxed);
        assert_eq!(j.line(), "Downloading 12 of 44 MB");
        j.unpacking.store(true, Ordering::Relaxed);
        assert!(j.line().starts_with("Unpacking and verifying"));
    }

    #[test]
    fn only_absent_families_are_missing() {
        let d = std::env::temp_dir().join(format!("helion-xray-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(missing(&d).len(), xray_db::FAMILIES.len());
        std::fs::create_dir_all(d.join("zynq7")).unwrap();
        assert!(missing(&d).iter().all(|f| f.name != "zynq7"), "a user's own zynq7 is not missing");
        let _ = std::fs::remove_dir_all(&d);
    }
}
