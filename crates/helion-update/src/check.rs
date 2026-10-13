//! Release query and version comparison against the GitHub Releases API.
//!
//! Uses `self_update`'s GitHub backend for the API call (handles auth
//! headers and paged responses). Draft releases are not returned for
//! unauthenticated API calls; prereleases are filtered out here by semver so
//! only stable releases are ever offered as updates.

use semver::Version;

use crate::error::UpdateError;

pub const REPO_OWNER: &str = "helion-fpga";
pub const REPO_NAME: &str = "helion";

/// One release asset (file) attached to a GitHub release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetInfo {
    pub name: String,
    pub download_url: String,
}

/// A parsed GitHub release.
#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    /// Raw tag, e.g. "v3.1.0".
    pub tag: String,
    /// Parsed semver of the tag.
    pub version: Version,
    pub assets: Vec<AssetInfo>,
}

/// Version this binary was built as.
pub fn current_version() -> Result<Version, UpdateError> {
    env!("CARGO_PKG_VERSION")
        .parse()
        .map_err(|e| UpdateError::Version(format!("CARGO_PKG_VERSION: {e}")))
}

/// Parse a release tag like "v3.1.0" (leading `v` optional) into semver.
pub fn parse_tag(tag: &str) -> Result<Version, UpdateError> {
    tag.trim_start_matches('v')
        .parse()
        .map_err(|e| UpdateError::Version(format!("tag {tag:?}: {e}")))
}

/// True when `latest` is strictly newer than `current`.
pub fn is_newer(current: &Version, latest: &Version) -> bool {
    latest > current
}

/// Pick the newest stable (non-prerelease) release from a list.
/// Returns None when nothing parses or everything is a prerelease.
pub fn latest_stable(releases: &[ReleaseInfo]) -> Option<ReleaseInfo> {
    releases
        .iter()
        .filter(|r| r.version.pre.is_empty())
        .max_by(|a, b| a.version.cmp(&b.version))
        .cloned()
}

/// Query the GitHub Releases API.
/// Returns Ok(None) when the current build is already at (or past) the
/// latest stable release.
pub fn check_for_update() -> Result<Option<ReleaseInfo>, UpdateError> {
    let current = current_version()?;
    let raw = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .map_err(|e| UpdateError::Release(e.to_string()))?
        .fetch()
        .map_err(|e| UpdateError::Network(e.to_string()))?;

    let mut parsed = Vec::with_capacity(raw.len());
    for r in &raw {
        let version = match parse_tag(&r.version) {
            Ok(v) => v,
            Err(_) => continue,
        };
        parsed.push(ReleaseInfo {
            tag: format!("v{r}", r = r.version),
            version,
            assets: r
                .assets
                .iter()
                .map(|a| AssetInfo {
                    name: a.name.clone(),
                    download_url: a.download_url.clone(),
                })
                .collect(),
        });
    }

    let Some(latest) = latest_stable(&parsed) else {
        return Ok(None);
    };
    if is_newer(&current, &latest.version) {
        Ok(Some(latest))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str) -> ReleaseInfo {
        ReleaseInfo {
            tag: tag.to_string(),
            version: parse_tag(tag).unwrap(),
            assets: vec![],
        }
    }

    #[test]
    fn parse_tag_strips_leading_v() {
        assert_eq!(parse_tag("v3.1.0").unwrap(), Version::new(3, 1, 0));
        assert_eq!(parse_tag("3.1.0").unwrap(), Version::new(3, 1, 0));
    }

    #[test]
    fn parse_tag_rejects_garbage() {
        assert!(parse_tag("not-a-version").is_err());
        assert!(parse_tag("v1.2").is_err());
    }

    #[test]
    fn is_newer_compares_semver() {
        let cur = Version::new(3, 0, 0);
        assert!(is_newer(&cur, &Version::new(3, 0, 1)));
        assert!(is_newer(&cur, &Version::new(3, 1, 0)));
        assert!(is_newer(&cur, &Version::new(4, 0, 0)));
        assert!(!is_newer(&cur, &Version::new(3, 0, 0)));
        assert!(!is_newer(&cur, &Version::new(2, 9, 9)));
    }

    #[test]
    fn latest_stable_skips_prereleases() {
        let releases = vec![rel("v3.0.0"), rel("v3.2.0-rc1"), rel("v3.1.0")];
        let latest = latest_stable(&releases).unwrap();
        assert_eq!(latest.version, Version::new(3, 1, 0));
    }

    #[test]
    fn latest_stable_none_when_all_prerelease() {
        let releases = vec![rel("v3.2.0-rc1"), rel("v3.2.0-beta2")];
        assert!(latest_stable(&releases).is_none());
    }

    #[test]
    fn latest_stable_picks_max_version_not_first() {
        // GitHub returns newest-first, but do not rely on ordering.
        let releases = vec![rel("v3.1.0"), rel("v3.5.0"), rel("v3.2.0")];
        let latest = latest_stable(&releases).unwrap();
        assert_eq!(latest.version, Version::new(3, 5, 0));
    }

    #[test]
    fn latest_stable_empty_is_none() {
        assert!(latest_stable(&[]).is_none());
    }
}
