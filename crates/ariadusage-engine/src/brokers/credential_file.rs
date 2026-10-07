// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthCredentials.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentials.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::call::BrokerCall;

/// Maximum permitted credential file size (1 MiB).
pub const MAX_CREDENTIAL_FILE_SIZE: u64 = 1024 * 1024;

/// A declared path to a credential file.
///
/// Outside crates cannot construct a `CredentialDecl` directly:
/// ```compile_fail
/// use ariadusage_engine::brokers::credential_file::CredentialDecl;
/// let _ = CredentialDecl::new("/path/to/cred");
/// ```
#[derive(Clone)]
pub struct CredentialDecl {
    path: PathBuf,
}

impl CredentialDecl {
    #[allow(dead_code)]
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the filesystem path for this declared credential file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Test hook allowing integration tests to construct a declared path.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn for_test(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl fmt::Debug for CredentialDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialDecl")
            .field("path_len", &self.path.as_os_str().len())
            .finish()
    }
}

/// Stat-based fingerprint of a credential file capturing filesystem device, inode,
/// modification time in nanoseconds, and file size.
///
/// Plaintext path is skipped during serialization to keep persisted broker state
/// free of user path text and secret tokens.
#[derive(Clone, Eq, Serialize, Deserialize)]
pub struct StatFingerprint {
    #[serde(skip)]
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub mtime_ns: i128,
    pub size: u64,
}

impl PartialEq for StatFingerprint {
    fn eq(&self, other: &Self) -> bool {
        self.dev == other.dev
            && self.ino == other.ino
            && self.mtime_ns == other.mtime_ns
            && self.size == other.size
            && (self.path.as_os_str().is_empty()
                || other.path.as_os_str().is_empty()
                || self.path == other.path)
    }
}

impl std::hash::Hash for StatFingerprint {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dev.hash(state);
        self.ino.hash(state);
        self.mtime_ns.hash(state);
        self.size.hash(state);
    }
}

impl fmt::Debug for StatFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatFingerprint")
            .field("path_len", &self.path.as_os_str().len())
            .field("dev", &self.dev)
            .field("ino", &self.ino)
            .field("mtime_ns", &self.mtime_ns)
            .field("size", &self.size)
            .finish()
    }
}

/// Successful read of a declared credential file, returning zeroized bytes and stat fingerprint.
pub struct CredentialRead {
    pub bytes: Zeroizing<Vec<u8>>,
    pub stat: StatFingerprint,
}

impl fmt::Debug for CredentialRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialRead")
            .field("bytes_len", &self.bytes.len())
            .field("stat", &self.stat)
            .finish()
    }
}

/// Errors encountered while reading or validating a declared credential file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialFileError {
    #[error("credential file not found")]
    NotFound,

    #[error("credential file is untrusted")]
    Untrusted,

    #[error("credential file is unreadable")]
    Unreadable,

    #[error("credential file is too large")]
    TooLarge,

    #[error("credential file is malformed")]
    Malformed,

    #[error("operation cancelled")]
    Cancelled,
}

/// Computes the standard SHA-256 hexadecimal content fingerprint of the given byte buffer.
pub fn content_fingerprint(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Reads the declared credential file safely.
///
/// Follows symlinks, opens with `O_RDONLY|O_NONBLOCK|O_NOCTTY|O_CLOEXEC`, checks that the target
/// is a regular file owned by the current EUID, and enforces a size limit of 1 MiB.
pub fn read(
    decl: &CredentialDecl,
    call: &BrokerCall,
) -> Result<CredentialRead, CredentialFileError> {
    read_with_expected_uid(decl, call, None)
}

/// Reads the declared credential file safely with an injected expected UID (for test isolation).
pub fn read_with_expected_uid(
    decl: &CredentialDecl,
    call: &BrokerCall,
    expected_uid: Option<u32>,
) -> Result<CredentialRead, CredentialFileError> {
    if call.cancel.is_cancelled() {
        return Err(CredentialFileError::Cancelled);
    }

    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let fd = match rustix::fs::open(
            decl.path(),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOCTTY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => {
                return Err(CredentialFileError::NotFound);
            }
            Err(_) => return Err(CredentialFileError::Unreadable),
        };

        let stat = match rustix::fs::fstat(&fd) {
            Ok(stat) => stat,
            Err(_) => return Err(CredentialFileError::Unreadable),
        };

        // Must be a regular file (e.g. FIFOs return immediately on nonblocking open and are rejected here).
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
            return Err(CredentialFileError::Unreadable);
        }

        // Owner check: must match current EUID or injected expected_uid.
        let required_uid = expected_uid.unwrap_or_else(|| rustix::process::geteuid().as_raw());
        if stat.st_uid != required_uid {
            return Err(CredentialFileError::Untrusted);
        }

        // Size check: must not exceed 1 MiB.
        if (stat.st_size as u64) > MAX_CREDENTIAL_FILE_SIZE {
            return Err(CredentialFileError::TooLarge);
        }

        let mtime_ns = (stat.st_mtime as i128) * 1_000_000_000 + (stat.st_mtime_nsec as i128);
        let stat_fp = StatFingerprint {
            path: decl.path().to_path_buf(),
            dev: stat.st_dev,
            ino: stat.st_ino,
            mtime_ns,
            size: stat.st_size as u64,
        };

        let mut file = std::fs::File::from(fd);
        let mut buffer = Vec::new();
        let mut handle = (&mut file).take(MAX_CREDENTIAL_FILE_SIZE + 1);
        if handle.read_to_end(&mut buffer).is_err() {
            return Err(CredentialFileError::Unreadable);
        }

        if buffer.len() as u64 > MAX_CREDENTIAL_FILE_SIZE {
            return Err(CredentialFileError::TooLarge);
        }

        Ok(CredentialRead {
            bytes: Zeroizing::new(buffer),
            stat: stat_fp,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (decl, expected_uid);
        Err(CredentialFileError::Unreadable)
    }
}

/// Stores last-seen stat fingerprints per profile digest to detect credential changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSeen {
    entries: BTreeMap<String, StatFingerprint>,
}

impl LastSeen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if the file changed since last seen for this profile.
    pub fn has_changed(&self, profile_digest: &str, current: &StatFingerprint) -> bool {
        match self.entries.get(profile_digest) {
            Some(stored) => stored != current,
            None => true,
        }
    }

    /// Records the current fingerprint for this profile.
    pub fn record(&mut self, profile_digest: &str, fingerprint: StatFingerprint) {
        self.entries.insert(profile_digest.to_owned(), fingerprint);
    }

    /// Gets the recorded fingerprint for this profile, if any.
    pub fn get(&self, profile_digest: &str) -> Option<&StatFingerprint> {
        self.entries.get(profile_digest)
    }

    /// Returns a reference to all entries.
    pub fn entries(&self) -> &BTreeMap<String, StatFingerprint> {
        &self.entries
    }
}

/// Manages quarantine for profiles whose credential file was proven invalid.
/// Quarantine holds only while the current fingerprint matches the quarantined one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantine {
    entries: BTreeMap<String, StatFingerprint>,
}

impl Quarantine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if this profile's file is currently quarantined.
    /// If the current fingerprint matches stored -> returns `true`.
    /// If the fingerprint changed -> clears quarantine automatically and returns `false`.
    pub fn is_quarantined(&mut self, profile_digest: &str, current: &StatFingerprint) -> bool {
        if let Some(stored) = self.entries.get(profile_digest) {
            if stored == current {
                return true;
            }
            self.entries.remove(profile_digest);
        }
        false
    }

    /// Places a profile under quarantine with its current fingerprint.
    pub fn quarantine(&mut self, profile_digest: &str, fingerprint: StatFingerprint) {
        self.entries.insert(profile_digest.to_owned(), fingerprint);
    }

    /// Manually clears quarantine for a profile.
    pub fn clear(&mut self, profile_digest: &str) {
        self.entries.remove(profile_digest);
    }

    /// Returns a reference to all quarantined entries.
    pub fn entries(&self) -> &BTreeMap<String, StatFingerprint> {
        &self.entries
    }
}

/// Re-reads a declared credential file up to 3 times, 50 ms apart, while the publication
/// predicate `accept` returns false. Cancellation is checked before each attempt.
pub async fn read_with_publication_retry<Accept>(
    decl: &CredentialDecl,
    call: &BrokerCall,
    accept: Accept,
) -> Result<CredentialRead, CredentialFileError>
where
    Accept: Fn(&CredentialRead) -> bool,
{
    read_with_publication_retry_impl(call, || read(decl, call), accept).await
}

/// Generic bounded publication retry helper.
///
/// At most 3 reads, 50 ms apart. If `call` is cancelled, returns `Cancelled` (with 0 reads
/// if cancelled before the first read). Returns the final error if attempts are exhausted.
pub async fn read_with_publication_retry_impl<T, F, Accept>(
    call: &BrokerCall,
    mut read_fn: F,
    accept: Accept,
) -> Result<T, CredentialFileError>
where
    F: FnMut() -> Result<T, CredentialFileError>,
    Accept: Fn(&T) -> bool,
{
    const MAX_ATTEMPTS: usize = 3;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

    let mut last_err = CredentialFileError::NotFound;

    for attempt in 0..MAX_ATTEMPTS {
        if call.cancel.is_cancelled() {
            return Err(CredentialFileError::Cancelled);
        }

        if attempt > 0 {
            tokio::select! {
                _ = call.cancel.cancelled() => {
                    return Err(CredentialFileError::Cancelled);
                }
                _ = tokio::time::sleep(RETRY_DELAY) => {}
            }
            if call.cancel.is_cancelled() {
                return Err(CredentialFileError::Cancelled);
            }
        }

        match read_fn() {
            Ok(val) => {
                if accept(&val) {
                    return Ok(val);
                } else {
                    last_err = CredentialFileError::Malformed;
                }
            }
            Err(e) => {
                last_err = e;
            }
        }
    }

    Err(last_err)
}
