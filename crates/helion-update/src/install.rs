//! Install planning and execution.
//!
//! An update is only applied in place when the install location is
//! user-writable. Otherwise the release asset is downloaded and verified, and
//! the user is told where it is so they can install it manually.
//!
//! macOS code-signing note: the shipped `Helion.app` is ad-hoc signed, not
//! Apple notarized (see AGENTS.md). Replacing only the executable inside the
//! bundle would invalidate the bundle signature, so a `.app` install swaps
//! the whole bundle directory for the freshly released one, which carries its
//! own ad-hoc signature from the release build. If the installed bundle
//! instead carries a real Developer ID `Authority=` signature, the in-place
//! swap is refused and the updater falls back to download-only rather than
//! silently breaking the install.

use std::fs::File;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::assets::{select_asset, AssetKind, SelectedAsset};
use crate::check::ReleaseInfo;
use crate::error::UpdateError;

/// How the new release is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Replace the running executable file in place.
    Binary { exe_path: PathBuf },
    /// Replace a whole macOS `.app` bundle in place.
    MacBundle { bundle_path: PathBuf },
}

/// What the updater will do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMode {
    /// Swap the install in place (binary or bundle).
    InPlace(InstallKind),
    /// The install location is not user-writable (or the bundle is
    /// Developer ID-signed): download and verify the asset, then tell the
    /// user where it is.
    DownloadOnly { reason: String },
}

/// The full plan: what to download and how to install it.
#[derive(Clone, Debug)]
pub struct InstallPlan {
    pub mode: InstallMode,
    pub asset: SelectedAsset,
}

/// Path of the currently running executable.
pub fn current_exe_path() -> Result<PathBuf, UpdateError> {
    std::env::current_exe().map_err(|e| UpdateError::Io(format!("current exe: {e}")))
}

/// If `exe` lives inside a macOS app bundle
/// (`.../Foo.app/Contents/MacOS/<exe>`), return the bundle directory.
pub fn find_app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents_dir.parent()?;
    if bundle.extension()?.to_str()? != "app" {
        return None;
    }
    Some(bundle.to_path_buf())
}

/// True when a file can be created (and removed) inside `dir`.
/// This is the writability probe: no permission bits are inspected, the
/// actual operation is attempted.
pub fn dir_writable(dir: &Path) -> bool {
    match tempfile::NamedTempFile::new_in(dir) {
        Ok(f) => {
            let path = f.path().to_path_buf();
            drop(f);
            let _ = std::fs::remove_file(path);
            true
        }
        Err(_) => false,
    }
}

/// True when the bundle carries a real Developer ID signature.
/// Ad-hoc signed bundles (what Helion ships) have no `Authority=` lines.
#[cfg(target_os = "macos")]
fn bundle_has_developer_id_signature(bundle: &Path) -> bool {
    let out = std::process::Command::new("codesign")
        .args(["-dv", &bundle.to_string_lossy()])
        .output();
    match out {
        Ok(o) => {
            // codesign writes its report to stderr.
            let text = String::from_utf8_lossy(&o.stderr);
            text.lines().any(|l| l.starts_with("Authority="))
        }
        // No codesign binary: cannot verify, treat as unsigned and proceed.
        Err(_) => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn bundle_has_developer_id_signature(_bundle: &Path) -> bool {
    false
}

/// Pure install-mode decision, kept separate from filesystem access so it is
/// unit-testable. `writable` is whether the install location's parent
/// directory is user-writable; `signed` is whether a macOS bundle carries a
/// Developer ID signature.
fn decide_mode(kind: &InstallKind, writable: bool, signed: bool) -> InstallMode {
    if !writable {
        return InstallMode::DownloadOnly {
            reason: match kind {
                InstallKind::Binary { .. } => {
                    "the install directory is not writable".to_string()
                }
                InstallKind::MacBundle { .. } => {
                    "the Applications folder is not writable".to_string()
                }
            },
        };
    }
    if signed {
        return InstallMode::DownloadOnly {
            reason: "the installed Helion.app carries a Developer ID signature; \
                     replacing it automatically would break the signature"
                .to_string(),
        };
    }
    InstallMode::InPlace(kind.clone())
}

/// Decide what to download and how to install it. No network, no changes.
pub fn plan_install(release: &ReleaseInfo) -> Result<InstallPlan, UpdateError> {
    let exe = current_exe_path()?;
    let in_bundle = find_app_bundle(&exe);
    let target = self_update::get_target();
    let asset = select_asset(
        &release.assets,
        &release.version,
        &target,
        in_bundle.is_some(),
    )?;

    let (kind, dir_to_probe) = match &in_bundle {
        Some(bundle) => (
            InstallKind::MacBundle {
                bundle_path: bundle.clone(),
            },
            bundle.parent().map(Path::to_path_buf),
        ),
        None => (
            InstallKind::Binary {
                exe_path: exe.clone(),
            },
            exe.parent().map(Path::to_path_buf),
        ),
    };
    let writable = dir_to_probe.as_deref().is_some_and(dir_writable);
    let signed = in_bundle
        .as_deref()
        .is_some_and(bundle_has_developer_id_signature);
    let mode = decide_mode(&kind, writable, signed);
    // Keep the selected asset consistent with the mode: a bundle install
    // needs the app zip, a binary install needs the tarball.
    debug_assert!(matches!(
        (&mode, &asset.kind),
        (InstallMode::InPlace(InstallKind::MacBundle { .. }), AssetKind::MacAppZip)
            | (InstallMode::InPlace(InstallKind::Binary { .. }), AssetKind::Tarball)
            | (InstallMode::DownloadOnly { .. }, _)
    ));
    Ok(InstallPlan { mode, asset })
}

/// Download a URL into `dest` (created/truncated).
pub fn download_to_file(url: &str, dest: &Path) -> Result<(), UpdateError> {
    let file = File::create(dest)?;
    self_update::Download::from_url(url)
        .download_to(&file)
        .map_err(|e| UpdateError::Network(e.to_string()))
}

/// Install the IDE binary from a downloaded release tarball.
/// Extracts `helion-{version}-{target}/helion-ide` and swaps it over the
/// running executable. Returns the (unchanged) executable path.
pub fn install_binary_from_tarball(
    tarball: &Path,
    version: &Version,
    target: &str,
) -> Result<PathBuf, UpdateError> {
    let stage = tempfile::tempdir()?;
    self_update::Extract::from_source(tarball)
        .archive(self_update::ArchiveKind::Tar(Some(
            self_update::Compression::Gz,
        )))
        .extract_into(stage.path())
        .map_err(|e| UpdateError::Install(format!("extract tarball: {e}")))?;
    let inner = stage
        .path()
        .join(format!("helion-{version}-{target}"))
        .join("helion-ide");
    if !inner.is_file() {
        return Err(UpdateError::Install(format!(
            "archive has no {}",
            inner.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&inner)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&inner, perms)?;
    }
    self_update::self_replace::self_replace(&inner)
        .map_err(|e| UpdateError::Install(format!("replace binary: {e}")))?;
    current_exe_path()
}

/// Install a macOS `.app` bundle from a downloaded release zip.
/// The whole bundle directory is swapped transactionally: the current bundle
/// is renamed aside first, so a failed install restores it instead of
/// leaving a half-installed app.
pub fn install_bundle_from_zip(zip: &Path, bundle_path: &Path) -> Result<(), UpdateError> {
    let parent = bundle_path.parent().ok_or_else(|| {
        UpdateError::Install(format!("bundle has no parent: {}", bundle_path.display()))
    })?;
    let stage = tempfile::tempdir_in(parent)?;
    self_update::Extract::from_source(zip)
        .archive(self_update::ArchiveKind::Zip)
        .extract_into(stage.path())
        .map_err(|e| UpdateError::Install(format!("extract app zip: {e}")))?;
    let staged_app = stage.path().join("Helion.app");
    if !staged_app.is_dir() {
        return Err(UpdateError::Install(
            "archive has no Helion.app".to_string(),
        ));
    }
    let backup = parent.join("Helion.app.update-bak");
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    std::fs::rename(bundle_path, &backup)
        .map_err(|e| UpdateError::Install(format!("move old bundle aside: {e}")))?;
    let result = std::fs::rename(&staged_app, bundle_path);
    match result {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
            // tempdir_in cleanup removes the now-empty staging dir.
            Ok(())
        }
        Err(e) => {
            // Restore the old bundle; never leave a half-installed app.
            let _ = std::fs::rename(&backup, bundle_path);
            Err(UpdateError::Install(format!("install new bundle: {e}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_app_bundle_detects_macos_layout() {
        let exe = Path::new("/Applications/Helion.app/Contents/MacOS/helion-ide");
        assert_eq!(
            find_app_bundle(exe).unwrap(),
            PathBuf::from("/Applications/Helion.app")
        );
    }

    #[test]
    fn find_app_bundle_rejects_plain_paths() {
        assert!(find_app_bundle(Path::new("/usr/local/bin/helion-ide")).is_none());
        assert!(find_app_bundle(Path::new("/home/u/.local/bin/helion-ide")).is_none());
        // Wrong inner layout.
        assert!(find_app_bundle(Path::new("/Applications/Helion.app/MacOS/helion-ide")).is_none());
        assert!(find_app_bundle(Path::new("/Applications/Helion.app/Contents/helion-ide")).is_none());
    }

    #[test]
    fn dir_writable_tmp_is_writable() {
        assert!(dir_writable(std::env::temp_dir().as_path()));
    }

    #[test]
    fn dir_writable_missing_dir_is_not() {
        assert!(!dir_writable(Path::new("/definitely/not/a/real/dir-helion-test")));
    }

    #[test]
    fn decide_mode_binary_writable_is_in_place() {
        let kind = InstallKind::Binary {
            exe_path: PathBuf::from("/home/u/.local/bin/helion-ide"),
        };
        assert_eq!(decide_mode(&kind, true, false), InstallMode::InPlace(kind));
    }

    #[test]
    fn decide_mode_unwritable_is_download_only() {
        let kind = InstallKind::Binary {
            exe_path: PathBuf::from("/usr/local/bin/helion-ide"),
        };
        let mode = decide_mode(&kind, false, false);
        assert!(
            matches!(mode, InstallMode::DownloadOnly { .. }),
            "unwritable install dir must not self-update"
        );
    }

    #[test]
    fn decide_mode_signed_bundle_is_download_only() {
        let kind = InstallKind::MacBundle {
            bundle_path: PathBuf::from("/Applications/Helion.app"),
        };
        let mode = decide_mode(&kind, true, true);
        assert!(
            matches!(mode, InstallMode::DownloadOnly { reason } if reason.contains("signature")),
            "Developer ID-signed bundle must not be swapped silently"
        );
    }

    #[test]
    fn decide_mode_adhoc_bundle_writable_is_in_place() {
        let kind = InstallKind::MacBundle {
            bundle_path: PathBuf::from("/Users/u/Applications/Helion.app"),
        };
        assert_eq!(
            decide_mode(&kind, true, false),
            InstallMode::InPlace(kind)
        );
    }
}
