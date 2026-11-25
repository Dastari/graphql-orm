//! Per-scope conversational content-protection contracts.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use agql_auth::AuthPrincipal;

use crate::{AiError, AiScope};

/// Storage protection selected for a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiContentProtectionMode {
    /// The deployment database/storage layer provides encryption at rest.
    DatabaseManaged,
    /// The application protects content before it reaches ORM persistence.
    /// The host must supply the [`AiContentProtector`] that does so; the crate
    /// has no implementation of this mode.
    ApplicationEncrypted,
}

/// Scope policy resolved before content can be persisted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiContentProtectionPolicy {
    /// Scope governed by this policy.
    pub scope: AiScope,
    /// Selected mode.
    pub mode: AiContentProtectionMode,
    /// Non-secret key policy or key-version reference.
    pub key_policy_reference: Option<String>,
    /// CAS/configuration version.
    pub version: u64,
    /// Whether any required migration/re-protection has completed.
    pub ready: bool,
}

/// Resolves the current, authorized protection policy for one scope.
///
/// Implementations normally read the GraphQL-managed configuration store.
/// They must apply scope/tenant isolation and fail closed when a policy is
/// absent, stale, migrating, or otherwise not ready.
#[async_trait]
pub trait AiContentProtectionPolicyResolver: Send + Sync {
    /// Loads the policy effective for this principal and scope.
    async fn resolve(
        &self,
        principal: &AuthPrincipal,
        scope: &AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError>;
}

/// Associated identity bound into application-level protection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentProtectionContext {
    /// Stable entity/table identity.
    pub entity: String,
    /// Stable row identity.
    pub row_id: String,
    /// Stable field identity.
    pub field: String,
    /// Owning scope.
    pub scope: AiScope,
}

/// Serializable content envelope. No public GraphQL output should expose this
/// type directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "protection", rename_all = "snake_case")]
pub enum ProtectedContentEnvelope {
    /// Content relies on deployment-managed database/storage encryption.
    DatabaseManaged {
        /// Canonical JSON value stored by the ORM.
        value: serde_json::Value,
    },
    /// Content was protected before persistence.
    ApplicationEncrypted {
        /// Envelope version.
        version: u16,
        /// Non-secret key identifier.
        key_id: String,
        /// Authenticated-encryption algorithm identifier.
        algorithm: String,
        /// Encoded nonce/initialization material.
        nonce: String,
        /// Encoded authenticated ciphertext.
        ciphertext: String,
    },
}

/// Content-protection failure without key or plaintext details.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentProtectionError {
    /// No ready policy exists for the scope.
    #[error("content protection policy is not ready")]
    PolicyNotReady,
    /// Required key material is unavailable.
    #[error("content protection key is unavailable")]
    KeyUnavailable,
    /// Envelope identity/authentication validation failed.
    #[error("protected content validation failed")]
    ValidationFailed,
    /// Configured protection mode is unsupported by this implementation.
    #[error("content protection mode is unsupported")]
    Unsupported,
}

pub(crate) fn map_orm_protection_error(
    operation: &'static str,
    error: ContentProtectionError,
) -> AiError {
    let code: &'static str = match error {
        ContentProtectionError::PolicyNotReady => return AiError::RuntimeNotReady,
        ContentProtectionError::KeyUnavailable => "key_unavailable",
        ContentProtectionError::ValidationFailed => "validation_failed",
        ContentProtectionError::Unsupported => "unsupported",
    };
    tracing::debug!(
        target: "graphql_orm_ai::content_protection",
        operation,
        code,
        "AI content protection failed"
    );
    AiError::PersistenceFailed
}

/// Application-level content protection seam. Implementations should bind
/// authenticated encryption to every field in [`ContentProtectionContext`].
///
/// This is an extension point, not an encryption feature. The crate ships no
/// cipher and holds no keys: encryption, key management and key rotation are
/// implemented and reviewed by the host. The seam covers conversational JSON
/// values (message blocks, previews, event payloads, checkpoints and the
/// like). It never sees attachment bytes, which are streamed to the host's
/// blob store as uploaded.
#[async_trait]
pub trait AiContentProtector: Send + Sync {
    /// Protects a canonical JSON value for persistence.
    async fn protect(
        &self,
        policy: &AiContentProtectionPolicy,
        context: &ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<ProtectedContentEnvelope, ContentProtectionError>;

    /// Opens a value after verifying its policy and associated identity.
    async fn open(
        &self,
        policy: &AiContentProtectionPolicy,
        context: &ContentProtectionContext,
        envelope: &ProtectedContentEnvelope,
    ) -> Result<serde_json::Value, ContentProtectionError>;
}

/// Explicit database-managed implementation. It refuses application-encrypted
/// envelopes rather than silently treating ciphertext as plaintext.
///
/// It performs no encryption. `protect` stores the JSON value unchanged in a
/// [`ProtectedContentEnvelope::DatabaseManaged`] envelope, so confidentiality
/// is whatever the deployment's database and storage provide at rest. It
/// returns [`ContentProtectionError::Unsupported`] for an
/// [`AiContentProtectionMode::ApplicationEncrypted`] policy, and this is the
/// only protector the crate provides.
#[derive(Clone, Copy, Debug, Default)]
pub struct DatabaseManagedContentProtector;

#[async_trait]
impl AiContentProtector for DatabaseManagedContentProtector {
    async fn protect(
        &self,
        policy: &AiContentProtectionPolicy,
        _context: &ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<ProtectedContentEnvelope, ContentProtectionError> {
        if !policy.ready {
            return Err(ContentProtectionError::PolicyNotReady);
        }
        if policy.mode != AiContentProtectionMode::DatabaseManaged {
            return Err(ContentProtectionError::Unsupported);
        }
        Ok(ProtectedContentEnvelope::DatabaseManaged { value })
    }

    async fn open(
        &self,
        policy: &AiContentProtectionPolicy,
        _context: &ContentProtectionContext,
        envelope: &ProtectedContentEnvelope,
    ) -> Result<serde_json::Value, ContentProtectionError> {
        if !policy.ready {
            return Err(ContentProtectionError::PolicyNotReady);
        }
        match (policy.mode, envelope) {
            (
                AiContentProtectionMode::DatabaseManaged,
                ProtectedContentEnvelope::DatabaseManaged { value },
            ) => Ok(value.clone()),
            _ => Err(ContentProtectionError::ValidationFailed),
        }
    }
}
