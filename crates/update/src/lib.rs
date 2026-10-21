//! Keeps Mini Remote up to date.
//!
//! New versions are published as GitHub releases. Every release file comes
//! with a `.sig` file: an Ed25519 signature made with a private key that only
//! the maintainer has. The matching public key is built into the app, so a
//! download is only installed when it really comes from the maintainer, even
//! if someone else got access to the GitHub account. The signature also covers
//! the version and file name, so an old release can't be passed off as a new one.

mod feed;
mod install;

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use feed::check;
pub use install::{Downloaded, Installed, download, install, schedule_restart};

/// The GitHub repository releases come from.
pub const REPOSITORY: &str = "JohnKesko/mini-remote";

/// Public half of the release signing key (Ed25519), hex encoded.
pub const PUBLIC_KEY_HEX: &str = "3712ac629b32ef9cd738700f25e6c502f610ad83295846af1a72def2e20aff71";

/// Set this to a URL (for example a local test server) to read the release
/// information from there instead of GitHub. Downloads are still verified.
pub const FEED_OVERRIDE_VAR: &str = "MINI_REMOTE_UPDATE_FEED";

/// The version of this build, e.g. "0.1.0".
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("Could not reach the update server: {0}")]
    Network(String),
    #[error("The update information could not be read: {0}")]
    Feed(String),
    #[error("The download could not be verified, so it was not installed. Please try again later.")]
    BadSignature,
    #[error("Updates can't be installed here: {0}")]
    NotInstallable(String),
    #[error("The update could not be installed: {0}")]
    Install(String),
    #[error("The download was cancelled.")]
    Cancelled,
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        UpdateError::Install(e.to_string())
    }
}

/// A newer version that can be installed on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Version without a leading "v", e.g. "0.2.0".
    pub version: String,
    /// What changed, as written on the release page (plain text / Markdown).
    pub notes: String,
    /// Web page of the release.
    pub page: String,
    pub asset: Asset,
    pub signature: Asset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// File name of the release file for this computer, or `None` when no
/// builds are published for it.
pub fn asset_name(version: &str) -> Option<String> {
    asset_name_for(std::env::consts::OS, std::env::consts::ARCH, version)
}

fn asset_name_for(os: &str, arch: &str, version: &str) -> Option<String> {
    match (os, arch) {
        ("macos", "aarch64") => Some(format!("Mini-Remote-{version}-macos-arm64.zip")),
        ("linux", "x86_64") => Some(format!("mini-remote-{version}-linux-x86_64.tar.gz")),
        ("linux", "aarch64") => Some(format!("mini-remote-{version}-linux-arm64.tar.gz")),
        _ => None,
    }
}

/// Whether `candidate` is a newer version than `current` ("1.2.10" > "1.2.9").
/// Anything after a "-" (pre-release tags) is ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let v = v.trim().trim_start_matches('v');
        let v = v.split(['-', '+']).next().unwrap_or("");
        v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(candidate), parts(current));
    let len = a.len().max(b.len());
    let pad = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..len).map(|i| (pad(&a, i), pad(&b, i))).find(|(x, y)| x != y).is_some_and(|(x, y)| x > y)
}

/// The exact text that is signed for a release file: it names the version and
/// file, and includes the SHA-256 of the contents.
pub fn signed_message(version: &str, file_name: &str, contents: &[u8]) -> Vec<u8> {
    let digest = Sha256::digest(contents);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("mini-remote release v1\nversion {version}\nfile {file_name}\nsha256 {hex}\n").into_bytes()
}

/// Check a `.sig` file (hex encoded signature) against the built-in key.
pub fn verify(version: &str, file_name: &str, contents: &[u8], signature_hex: &str) -> bool {
    verify_with(PUBLIC_KEY_HEX, version, file_name, contents, signature_hex)
}

fn verify_with(public_hex: &str, version: &str, file_name: &str, contents: &[u8], signature_hex: &str) -> bool {
    let (Some(public), Some(signature)) = (from_hex::<32>(public_hex), from_hex::<64>(signature_hex.trim())) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&public) else { return false };
    key.verify(&signed_message(version, file_name, contents), &Signature::from_bytes(&signature)).is_ok()
}

pub fn from_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A temporary folder next to `path` (same disk, so files can be moved in place).
fn sibling_temp_dir(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    let dir = parent.join(format!(".mini-remote-update-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn version_comparison() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.2.10", "1.2.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta", "0.2.0"));
    }

    #[test]
    fn asset_names() {
        assert_eq!(asset_name_for("macos", "aarch64", "1.0.0").unwrap(), "Mini-Remote-1.0.0-macos-arm64.zip");
        assert_eq!(asset_name_for("linux", "x86_64", "1.0.0").unwrap(), "mini-remote-1.0.0-linux-x86_64.tar.gz");
        assert_eq!(asset_name_for("linux", "aarch64", "1.0.0").unwrap(), "mini-remote-1.0.0-linux-arm64.tar.gz");
        assert_eq!(asset_name_for("windows", "x86_64", "1.0.0"), None);
    }

    #[test]
    fn signatures_cover_version_name_and_contents() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = to_hex(key.verifying_key().as_bytes());
        let data = b"app bytes";
        let sig = to_hex(&key.sign(&signed_message("1.0.0", "a.zip", data)).to_bytes());
        assert!(verify_with(&public, "1.0.0", "a.zip", data, &sig));
        // Same file claimed to be a newer version: rejected.
        assert!(!verify_with(&public, "1.0.1", "a.zip", data, &sig));
        assert!(!verify_with(&public, "1.0.0", "b.zip", data, &sig));
        assert!(!verify_with(&public, "1.0.0", "a.zip", b"other bytes", &sig));
        assert!(!verify_with(&public, "1.0.0", "a.zip", data, "nonsense"));
    }
}
