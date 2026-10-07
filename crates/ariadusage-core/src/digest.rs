//! Domain-separated SHA-256 digest calculations for profile ids, gate keys, and cache scopes.

use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Computes a domain-separated SHA-256 digest over a namespace and optional parts.
///
/// The namespace and each part in `parts` are joined with a NUL byte (`\0`).
/// Returns a 64-character lowercase hexadecimal string.
pub fn sha256_hex<P: AsRef<[u8]>>(namespace: &str, parts: &[P]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    for part in parts {
        hasher.update(b"\0");
        hasher.update(part.as_ref());
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
