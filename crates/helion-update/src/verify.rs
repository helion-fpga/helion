//! SHA256SUMS download, parsing, and verification.
//!
//! Releases publish `SHA256SUMS.txt` (see .github/workflows/release.yml)
//! with one `<hex>  <filename>` line per asset. self_update 0.42 has no
//! checksums feature, so verification is done here with `sha2`.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::assets::{sums_asset, SHA256SUMS_NAME};
use crate::check::ReleaseInfo;
use crate::error::UpdateError;

/// Download the release's SHA256SUMS.txt content.
pub fn fetch_sha256sums(release: &ReleaseInfo) -> Result<String, UpdateError> {
    let asset = sums_asset(&release.assets)
        .ok_or_else(|| UpdateError::ChecksumMissing(SHA256SUMS_NAME.to_string()))?;
    download_to_string(&asset.download_url)
}

/// Download a URL to a string (used for small text files like SHA256SUMS.txt).
fn download_to_string(url: &str) -> Result<String, UpdateError> {
    let mut tmp = tempfile::NamedTempFile::new()?;
    self_update::Download::from_url(url)
        .download_to(&mut tmp)
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    std::fs::read_to_string(tmp.path()).map_err(UpdateError::from)
}

/// Parse SHA256SUMS content into filename -> lowercase hex digest.
/// Accepts both `  ` (text mode) and ` *` (binary mode) separators.
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        // Binary-mode sums prefix the name with `*`; text mode uses a space.
        let name = name.trim_start_matches('*');
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            out.insert(name.to_string(), hash.to_ascii_lowercase());
        }
    }
    out
}

/// Look up the expected digest for a file name.
pub fn lookup_checksum<'a>(
    sums: &'a HashMap<String, String>,
    filename: &str,
) -> Result<&'a str, UpdateError> {
    sums.get(filename)
        .map(String::as_str)
        .ok_or_else(|| UpdateError::ChecksumMissing(filename.to_string()))
}

/// Hash `data` with SHA-256 and compare against `expected_hex`.
/// Comparison is case-insensitive; mismatch is an error, not a bool, so a
/// failed verification can never be silently ignored.
pub fn verify_bytes(data: &[u8], expected_hex: &str, filename: &str) -> Result<(), UpdateError> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hex_encode(hasher.finalize());
    if digest.eq_ignore_ascii_case(expected_hex.trim()) {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch(filename.to_string()))
    }
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.as_ref().len() * 2);
    for b in bytes.as_ref() {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &str = "d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666e814bd22e  helion-3.1.0-x86_64-unknown-linux-gnu.tar.gz\nABCDEF1234567890abcdef1234567890ABCDEF1234567890abcdef1234567890 *Helion-3.1.0-macos-arm64.zip\n";

    #[test]
    fn parses_text_and_binary_mode_lines() {
        let m = parse_sha256sums(SUMS);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m["helion-3.1.0-x86_64-unknown-linux-gnu.tar.gz"],
            "d4735e3a265e16eee03f59718b9b5d03019c07d8b6c51f90da3a666e814bd22e"
        );
        // Uppercase hex is normalized to lowercase.
        assert_eq!(
            m["Helion-3.1.0-macos-arm64.zip"],
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
        );
    }

    #[test]
    fn skips_blank_comment_and_malformed_lines() {
        let m = parse_sha256sums("# comment\n\nnot a sums line\nabc  short\n");
        assert!(m.is_empty());
    }

    #[test]
    fn lookup_missing_is_error() {
        let m = parse_sha256sums(SUMS);
        assert!(matches!(
            lookup_checksum(&m, "nope.tar.gz"),
            Err(UpdateError::ChecksumMissing(_))
        ));
    }

    #[test]
    fn verify_bytes_accepts_matching_digest() {
        let data = b"abc";
        // SHA-256 of b"abc".
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_bytes(data, expected, "f").is_ok());
        // Case-insensitive.
        assert!(verify_bytes(data, &expected.to_ascii_uppercase(), "f").is_ok());
    }

    #[test]
    fn verify_bytes_rejects_wrong_digest() {
        let data = b"abc";
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(matches!(
            verify_bytes(data, wrong, "f.tar.gz"),
            Err(UpdateError::ChecksumMismatch(_))
        ));
    }
}
