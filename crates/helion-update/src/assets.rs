//! Platform asset selection for a release.
//!
//! Release layout (see RELEASING.md and scripts/pack-unix-release.sh):
//! - `helion-{ver}-x86_64-unknown-linux-gnu.tar.gz`: Linux CLI + IDE tarball.
//!   The IDE binary lives at `{topdir}/helion-ide` inside the archive.
//! - `helion-{ver}-aarch64-apple-darwin.tar.gz`: macOS CLI + IDE tarball,
//!   same layout.
//! - `Helion-{ver}-macos-arm64.zip`: macOS `Helion.app` bundle, used when the
//!   running binary lives inside an installed `.app`.
//! - `SHA256SUMS.txt`: checksums for every file above.

use semver::Version;

use crate::check::AssetInfo;
use crate::error::UpdateError;

pub const SHA256SUMS_NAME: &str = "SHA256SUMS.txt";

/// Which release file to download for this install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// Unix tarball; the IDE binary is extracted from inside it.
    Tarball,
    /// macOS `.app` zip; the whole bundle is replaced.
    MacAppZip,
}

/// An asset chosen for this platform and install shape.
#[derive(Clone, Debug)]
pub struct SelectedAsset {
    pub kind: AssetKind,
    pub name: String,
    pub download_url: String,
}

/// Expected tarball file name for a version and target triple.
pub fn tarball_name(version: &Version, target: &str) -> String {
    format!("helion-{version}-{target}.tar.gz")
}

/// Expected macOS app zip file name for a version.
pub fn mac_app_zip_name(version: &Version) -> String {
    format!("Helion-{version}-macos-arm64.zip")
}

/// Choose the install asset.
///
/// `inside_app_bundle` is true when the running binary lives inside a macOS
/// `.app` bundle; then the bundle zip is selected so the whole signed bundle
/// is replaced instead of a single binary inside it. Otherwise the platform
/// tarball matching `target` (a Rust target triple) is selected.
pub fn select_asset(
    assets: &[AssetInfo],
    version: &Version,
    target: &str,
    inside_app_bundle: bool,
) -> Result<SelectedAsset, UpdateError> {
    let (kind, want) = if inside_app_bundle {
        (AssetKind::MacAppZip, mac_app_zip_name(version))
    } else {
        (AssetKind::Tarball, tarball_name(version, target))
    };
    assets
        .iter()
        .find(|a| a.name == want)
        .map(|a| SelectedAsset {
            kind,
            name: a.name.clone(),
            download_url: a.download_url.clone(),
        })
        .ok_or(UpdateError::NoAsset(want))
}

/// Find the SHA256SUMS.txt asset in a release.
pub fn sums_asset(assets: &[AssetInfo]) -> Option<&AssetInfo> {
    assets.iter().find(|a| a.name == SHA256SUMS_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> AssetInfo {
        AssetInfo {
            name: name.to_string(),
            download_url: format!("https://example.invalid/{name}"),
        }
    }

    fn release_assets() -> Vec<AssetInfo> {
        vec![
            asset("Helion-3.1.0-macos-arm64.zip"),
            asset("helion-3.1.0-aarch64-apple-darwin.tar.gz"),
            asset("helion-3.1.0-x86_64-unknown-linux-gnu.tar.gz"),
            asset("SHA256SUMS.txt"),
        ]
    }

    #[test]
    fn tarball_name_format() {
        assert_eq!(
            tarball_name(&Version::new(3, 1, 0), "x86_64-unknown-linux-gnu"),
            "helion-3.1.0-x86_64-unknown-linux-gnu.tar.gz"
        );
    }

    #[test]
    fn mac_app_zip_name_format() {
        assert_eq!(
            mac_app_zip_name(&Version::new(3, 1, 0)),
            "Helion-3.1.0-macos-arm64.zip"
        );
    }

    #[test]
    fn selects_linux_tarball() {
        let sel = select_asset(
            &release_assets(),
            &Version::new(3, 1, 0),
            "x86_64-unknown-linux-gnu",
            false,
        )
        .unwrap();
        assert_eq!(sel.kind, AssetKind::Tarball);
        assert_eq!(sel.name, "helion-3.1.0-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn selects_macos_tarball_outside_bundle() {
        let sel = select_asset(
            &release_assets(),
            &Version::new(3, 1, 0),
            "aarch64-apple-darwin",
            false,
        )
        .unwrap();
        assert_eq!(sel.kind, AssetKind::Tarball);
        assert_eq!(sel.name, "helion-3.1.0-aarch64-apple-darwin.tar.gz");
    }

    #[test]
    fn selects_app_zip_inside_bundle() {
        let sel = select_asset(
            &release_assets(),
            &Version::new(3, 1, 0),
            "aarch64-apple-darwin",
            true,
        )
        .unwrap();
        assert_eq!(sel.kind, AssetKind::MacAppZip);
        assert_eq!(sel.name, "Helion-3.1.0-macos-arm64.zip");
    }

    #[test]
    fn missing_asset_is_no_asset_error() {
        let err = select_asset(&[], &Version::new(3, 1, 0), "x86_64-unknown-linux-gnu", false)
            .unwrap_err();
        assert!(matches!(err, UpdateError::NoAsset(_)));
    }

    #[test]
    fn sums_asset_found() {
        assert_eq!(sums_asset(&release_assets()).unwrap().name, "SHA256SUMS.txt");
        assert!(sums_asset(&[]).is_none());
    }
}
