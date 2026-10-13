//! In-app update check and installer for Helion.
//!
//! Update checks run only on fresh sessions (app startup); there are no
//! periodic background checks while the app runs. The flow:
//!
//! 1. Query the GitHub Releases API for `helion-fpga/helion`.
//! 2. Compare the latest stable release tag against the compiled-in version.
//! 3. If newer, the IDE offers Update / Later; a manual "Check for updates"
//!    menu item re-runs the same check on demand.
//! 4. On Update: download the platform asset, verify it against the release's
//!    published `SHA256SUMS.txt`, swap the binary, and restart.
//!
//! The `self_update` crate (0.42 line, the last supporting the workspace MSRV
//! of Rust 1.85) provides the GitHub backend, download, archive extraction,
//! and binary replacement primitives. SHA256SUMS verification is done here
//! with `sha2` because 0.42 has no checksums feature.
//!
//! macOS note: the shipped `Helion.app` is ad-hoc signed, not Apple
//! notarized. Replacing only the executable inside the bundle would invalidate
//! the bundle signature, so on macOS an in-bundle install replaces the whole
//! `.app` directory with the freshly released bundle (which carries its own
//! ad-hoc signature). If the installed bundle carries a real Developer ID
//! signature instead, the updater refuses the in-place swap and falls back to
//! download-only rather than silently breaking the install.

mod assets;
mod check;
mod error;
mod install;
mod verify;

pub use assets::{select_asset, AssetKind, SelectedAsset};
pub use check::{check_for_update, current_version, is_newer, latest_stable, parse_tag, AssetInfo, ReleaseInfo, REPO_NAME, REPO_OWNER};
pub use error::UpdateError;
pub use install::{current_exe_path, dir_writable, download_to_file, find_app_bundle, install_binary_from_tarball, install_bundle_from_zip, plan_install, InstallKind, InstallMode, InstallPlan};
pub use verify::{fetch_sha256sums, lookup_checksum, parse_sha256sums, verify_bytes};

/// Rust target triple of this build, e.g. "x86_64-unknown-linux-gnu".
/// Used to pick the matching release asset.
pub fn target_triple() -> String {
    self_update::get_target().to_string()
}
