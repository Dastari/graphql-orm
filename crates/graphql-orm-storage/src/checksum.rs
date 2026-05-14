use sha2::{Digest, Sha256};

/// Computes a lowercase hexadecimal SHA-256 checksum for object bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
