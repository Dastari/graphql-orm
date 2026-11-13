//! Host-keyed manifest authentication.
//!
//! The backup repository is storage, not a trust anchor: anyone who can write
//! it can rewrite payloads and recompute plain SHA-256 digests. Manifests are
//! therefore sealed with HMAC-SHA256 under a key that only the host holds, and
//! every manifest read verifies that seal before its contents are used.

use std::collections::BTreeMap;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::BackupError;

/// Minimum accepted length of host-supplied key material, in bytes.
pub const MIN_MANIFEST_KEY_BYTES: usize = 32;

/// HKDF salt that separates this crate's derived keys from other uses of the
/// same host key material.
const KDF_SALT: &[u8] = b"graphql-orm-backup manifest authentication";
const MAC_KEY_INFO: &[u8] = b"manifest hmac-sha256 key v1";
const KEY_ID_INFO: &[u8] = b"manifest key id v1";
/// Prefix of every MAC input, so a seal cannot be confused with another
/// HMAC computed under the same derived key.
const MAC_DOMAIN: &[u8] = b"graphql-orm-backup/manifest/hmac-sha256/v2\n";

type HmacSha256 = Hmac<Sha256>;

/// Host-held secret used to authenticate snapshot manifests.
///
/// Supply at least [`MIN_MANIFEST_KEY_BYTES`] bytes of uniformly random key
/// material from a secret store. The crate never writes the key to the backup
/// repository. It derives a dedicated MAC key with HKDF-SHA256 and a fixed
/// label, so the same material can safely serve other purposes. Derived bytes
/// are zeroed on drop, and `Debug` shows only the public key id.
#[derive(Clone)]
pub struct ManifestKey {
    mac_key: Zeroizing<[u8; 32]>,
    key_id: String,
}

impl ManifestKey {
    /// Derives a manifest key from host key material.
    ///
    /// # Errors
    ///
    /// Returns [`BackupError::InvalidManifestKey`] when the material is shorter
    /// than [`MIN_MANIFEST_KEY_BYTES`].
    pub fn from_bytes(material: &[u8]) -> Result<Self, BackupError> {
        if material.len() < MIN_MANIFEST_KEY_BYTES {
            return Err(BackupError::InvalidManifestKey {
                reason: format!(
                    "key material must be at least {MIN_MANIFEST_KEY_BYTES} bytes, got {}",
                    material.len()
                ),
            });
        }
        let kdf = Hkdf::<Sha256>::new(Some(KDF_SALT), material);
        let mut mac_key = Zeroizing::new([0_u8; 32]);
        let mut key_id = [0_u8; 8];
        kdf.expand(MAC_KEY_INFO, mac_key.as_mut())
            .and_then(|()| kdf.expand(KEY_ID_INFO, &mut key_id))
            .map_err(|_| BackupError::InvalidManifestKey {
                reason: "key derivation failed".to_string(),
            })?;
        Ok(Self {
            mac_key,
            key_id: hex(&key_id),
        })
    }

    /// Public identifier recorded in sealed manifests.
    ///
    /// It identifies which key sealed a manifest and reveals nothing about the
    /// key itself.
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    fn mac(&self, canonical: &[u8]) -> HmacSha256 {
        // HMAC accepts keys of any length, so construction cannot fail.
        let mut mac = <HmacSha256 as Mac>::new_from_slice(self.mac_key.as_ref())
            .unwrap_or_else(|_| unreachable!("HMAC accepts any key length"));
        mac.update(MAC_DOMAIN);
        mac.update(canonical);
        mac
    }
}

impl std::fmt::Debug for ManifestKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManifestKey")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

/// How manifests written before repository identity binding are treated.
///
/// Format 1 has no seal; accepting it trusts the repository's contents.
/// Format 2 has a host-keyed seal but no repository identity. Repository reads
/// require explicit legacy trust for either format, and still authenticate a
/// format 2 seal. Content-key and restore-target checks always apply.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LegacyManifestPolicy {
    /// Refuse every older unbound manifest. This is the default.
    #[default]
    Refuse,
    /// Accept only format-2 manifests whose original host-keyed seal verifies.
    /// Refuses unsealed format 1 and never adopts a repository identity.
    AcceptSealedLegacy,
    /// Accept only the listed sealed format-2 snapshots, each with the manifest
    /// `checksum` the host recorded while it still trusted the repository.
    /// A manifest whose contents differ from the pinned checksum is refused.
    TrustPinned(BTreeMap<Uuid, String>),
    /// Accept unauthenticated format-1 content as well as sealed format 2.
    ///
    /// # Warning
    /// A repository writer can forge a format-1 manifest and recompute its plain
    /// checksum. This policy accepts that unauthenticated content. Use it only
    /// after an explicit host decision to trust historical unsealed data. For
    /// sealed backups use [`LegacyManifestPolicy::AcceptSealedLegacy`] instead.
    TrustUnauthenticated,
}

/// Host configuration for sealing and verifying manifests.
///
/// Attach it to the repository handle with
/// `with_manifest_authenticator`, or return it from
/// [`crate::BackupRepository::manifest_authenticator`] in a custom
/// repository. Repository handles without one refuse to read or write
/// manifests.
#[derive(Clone, Debug)]
pub struct ManifestAuthenticator {
    signing_key: ManifestKey,
    previous_keys: Vec<ManifestKey>,
    legacy: LegacyManifestPolicy,
    repository_identity: Option<Uuid>,
}

impl ManifestAuthenticator {
    /// Configures the signing key and accepts seals made with it.
    /// Format 3 additionally requires [`Self::with_repository_identity`].
    #[must_use]
    pub fn new(key: ManifestKey) -> Self {
        Self {
            signing_key: key,
            previous_keys: Vec::new(),
            legacy: LegacyManifestPolicy::Refuse,
            repository_identity: None,
        }
    }

    /// Pins the expected repository identity in host configuration. Required
    /// for format 3 sealing and verification. Retain this value outside the
    /// repository to detect replacement of its identity record.
    #[must_use]
    pub fn with_repository_identity(mut self, identity: Uuid) -> Self {
        self.repository_identity = Some(identity);
        self
    }

    pub(crate) fn require_repository_identity(&self) -> Result<Uuid, BackupError> {
        self.repository_identity.ok_or_else(|| BackupError::ManifestAuthenticationFailed {
            key: "repository-id".into(),
            reason: "host repository identity pin is required; initialize and retain its UUID outside the repository".into(),
        })
    }

    pub(crate) fn expected_repository_identity(&self) -> Option<Uuid> {
        self.repository_identity
    }

    pub(crate) fn check_repository_identity(&self, identity: Uuid) -> Result<(), BackupError> {
        if self
            .repository_identity
            .is_some_and(|expected| expected != identity)
        {
            return Err(BackupError::ManifestAuthenticationFailed {
                key: "repository-id".into(),
                reason: "repository identity differs from host pin".into(),
            });
        }
        Ok(())
    }

    /// Also accepts manifests sealed with a retired key, for key rotation.
    /// New manifests are always sealed with the current key.
    #[must_use]
    pub fn with_previous_key(mut self, key: ManifestKey) -> Self {
        self.previous_keys.push(key);
        self
    }

    /// Selects how older unbound manifests are treated.
    #[must_use]
    pub fn with_legacy_manifest_policy(mut self, policy: LegacyManifestPolicy) -> Self {
        self.legacy = policy;
        self
    }

    /// Key id of the key that seals new manifests.
    #[must_use]
    pub fn key_id(&self) -> &str {
        self.signing_key.key_id()
    }

    /// Configured legacy-manifest policy.
    #[must_use]
    pub fn legacy_manifest_policy(&self) -> &LegacyManifestPolicy {
        &self.legacy
    }

    pub(crate) fn seal(&self, canonical: &[u8]) -> crate::ManifestMac {
        crate::ManifestMac {
            algorithm: crate::ManifestMacAlgorithm::HmacSha256,
            key_id: self.signing_key.key_id.clone(),
            repository_id: self.repository_identity,
            mac: hex(
                &repository_mac(&self.signing_key, canonical, self.repository_identity)
                    .finalize()
                    .into_bytes(),
            ),
        }
    }

    /// Verifies a seal in constant time with the key named by its key id.
    pub(crate) fn verify(&self, seal: &crate::ManifestMac, canonical: &[u8]) -> Result<(), String> {
        match seal.algorithm {
            crate::ManifestMacAlgorithm::HmacSha256 => {}
        }
        let key = std::iter::once(&self.signing_key)
            .chain(&self.previous_keys)
            .find(|key| key.key_id == seal.key_id)
            .ok_or_else(|| format!("sealed with unknown key id {}", seal.key_id))?;
        let tag = decode_hex(&seal.mac).ok_or_else(|| "malformed MAC".to_string())?;
        if let Some(identity) = seal.repository_id {
            self.check_repository_identity(identity)
                .map_err(|error| error.to_string())?;
        }
        repository_mac(key, canonical, seal.repository_id)
            .verify_slice(&tag)
            .map_err(|_| "MAC does not match manifest contents".to_string())
    }
}

fn repository_mac(key: &ManifestKey, canonical: &[u8], identity: Option<Uuid>) -> HmacSha256 {
    if let Some(identity) = identity {
        let mut mac = key.mac(b"repository-bound/v3\n");
        mac.update(identity.as_bytes());
        mac.update(canonical);
        mac
    } else {
        key.mac(canonical)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 || !crate::manifest::is_lower_hex(value) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_key_material_is_rejected() {
        let error = ManifestKey::from_bytes(&[7; MIN_MANIFEST_KEY_BYTES - 1])
            .expect_err("short key rejected");
        assert!(matches!(error, BackupError::InvalidManifestKey { .. }));
    }

    #[test]
    fn debug_output_omits_key_material() {
        let key = ManifestKey::from_bytes(&[7; 32]).expect("key");
        let debug = format!("{key:?}");
        assert!(debug.contains(key.key_id()));
        assert!(!debug.contains("mac_key"));
    }

    #[test]
    fn seals_verify_only_under_the_sealing_key() {
        let current = ManifestKey::from_bytes(&[1; 32]).expect("key");
        let other = ManifestKey::from_bytes(&[2; 32]).expect("key");
        assert_ne!(current.key_id(), other.key_id());
        let seal = ManifestAuthenticator::new(current.clone()).seal(b"manifest");

        let verifier = ManifestAuthenticator::new(current.clone());
        verifier.verify(&seal, b"manifest").expect("valid seal");
        verifier
            .verify(&seal, b"manifest!")
            .expect_err("changed contents");
        ManifestAuthenticator::new(other.clone())
            .verify(&seal, b"manifest")
            .expect_err("unknown key id");
        ManifestAuthenticator::new(other)
            .with_previous_key(current)
            .verify(&seal, b"manifest")
            .expect("rotated key still verifies");
    }
}
