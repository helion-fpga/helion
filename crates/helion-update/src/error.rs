//! Update error type.

use thiserror::Error;

/// Errors from the update check / download / install flow.
#[derive(Debug, Error)]
pub enum UpdateError {
    /// HTTP or API transport failure.
    #[error("network error: {0}")]
    Network(String),
    /// Release metadata was missing or malformed.
    #[error("release metadata error: {0}")]
    Release(String),
    /// A version string or tag did not parse as semver.
    #[error("version parse error: {0}")]
    Version(String),
    /// The release has no usable asset for this platform.
    #[error("no install asset for this platform in release {0}")]
    NoAsset(String),
    /// Downloaded bytes did not match the published checksum.
    #[error("checksum mismatch for {0}")]
    ChecksumMismatch(String),
    /// SHA256SUMS.txt has no entry for the asset.
    #[error("checksum entry missing for {0}")]
    ChecksumMissing(String),
    /// The install step failed.
    #[error("install error: {0}")]
    Install(String),
    /// Local filesystem failure.
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        UpdateError::Io(e.to_string())
    }
}
