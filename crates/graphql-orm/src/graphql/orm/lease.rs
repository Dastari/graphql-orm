//! Backend-neutral fenced lease and durable-attempt contracts.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Monotonically increasing fencing token for one durable resource.
///
/// A token is meaningful only when it is checked together with the resource,
/// owner, attempt, unexpired deadline, and CAS row version in one atomic
/// persistence operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FencingToken(u64);

impl FencingToken {
    /// Initial unclaimed generation.
    pub const ZERO: Self = Self(0);

    /// Returns the numeric generation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, LeaseError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(LeaseError::GenerationExhausted)
    }
}

/// Values carried by every write performed by a leased attempt.
///
/// This value is not independently authoritative. Persistence code must match
/// every field plus the current deadline and CAS row version atomically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseProof {
    /// Durable resource identifier.
    pub resource_id: String,
    /// Worker owner identifier.
    pub owner_id: String,
    /// Unique attempt identifier.
    pub attempt_id: Uuid,
    /// Monotonic fencing token.
    pub fencing_token: FencingToken,
}

/// Portable durable lease state embedded in an ORM entity/projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FencedLeaseState {
    /// Durable resource identifier.
    pub resource_id: String,
    /// Current owner.
    pub owner_id: Option<String>,
    /// Current attempt.
    pub attempt_id: Option<Uuid>,
    /// Current fencing generation.
    pub fencing_token: FencingToken,
    /// Lease deadline as unix milliseconds.
    pub expires_at_ms: Option<i64>,
    /// CAS row version.
    pub row_version: i64,
}

impl FencedLeaseState {
    /// Creates unclaimed state for a resource.
    #[must_use]
    pub fn new(resource_id: impl Into<String>, row_version: i64) -> Self {
        Self {
            resource_id: resource_id.into(),
            owner_id: None,
            attempt_id: None,
            fencing_token: FencingToken::ZERO,
            expires_at_ms: None,
            row_version,
        }
    }

    /// Returns whether the current lease is active at `now_ms`.
    #[must_use]
    pub fn is_active(&self, now_ms: i64) -> bool {
        self.owner_id.is_some()
            && self.attempt_id.is_some()
            && self.expires_at_ms.is_some_and(|deadline| deadline > now_ms)
    }

    /// Applies an atomic-claim transition to this in-memory representation.
    ///
    /// Persistence backends must perform the equivalent predicate and update
    /// atomically; reading and later writing these fields is not a valid claim.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError`] when the TTL or expected row version is invalid,
    /// another lease remains active, or the deadline, fencing generation, or
    /// row version cannot advance safely. An error leaves `self` unchanged.
    pub fn claim(
        &mut self,
        owner_id: impl Into<String>,
        attempt_id: Uuid,
        now_ms: i64,
        lease_ttl_ms: i64,
        expected_row_version: i64,
    ) -> Result<LeaseProof, LeaseError> {
        if lease_ttl_ms <= 0 {
            return Err(LeaseError::InvalidTtl);
        }
        self.ensure_version(expected_row_version)?;
        if self.is_active(now_ms) {
            return Err(LeaseError::AlreadyLeased);
        }

        let next_row_version = self
            .row_version
            .checked_add(1)
            .ok_or(LeaseError::VersionExhausted)?;
        let expires_at_ms = now_ms
            .checked_add(lease_ttl_ms)
            .ok_or(LeaseError::TimeOverflow)?;
        let fencing_token = self.fencing_token.next()?;
        let owner_id = owner_id.into();
        self.owner_id = Some(owner_id.clone());
        self.attempt_id = Some(attempt_id);
        self.fencing_token = fencing_token;
        self.expires_at_ms = Some(expires_at_ms);
        self.row_version = next_row_version;

        Ok(LeaseProof {
            resource_id: self.resource_id.clone(),
            owner_id,
            attempt_id,
            fencing_token,
        })
    }

    /// Validates a fenced attempt before a state transition or child append.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError`] when the CAS version differs, any proof binding
    /// is stale, or the lease is no longer active.
    pub fn validate(
        &self,
        proof: &LeaseProof,
        now_ms: i64,
        expected_row_version: i64,
    ) -> Result<(), LeaseError> {
        self.ensure_version(expected_row_version)?;
        if proof.resource_id != self.resource_id
            || self.owner_id.as_deref() != Some(proof.owner_id.as_str())
            || self.attempt_id != Some(proof.attempt_id)
            || self.fencing_token != proof.fencing_token
        {
            return Err(LeaseError::StaleFence);
        }
        if !self.is_active(now_ms) {
            return Err(LeaseError::Expired);
        }
        Ok(())
    }

    /// Extends a lease after validating its fence and CAS version.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError`] when validation fails, the TTL is invalid, or
    /// the new deadline or row version would overflow. An error leaves `self`
    /// unchanged.
    pub fn heartbeat(
        &mut self,
        proof: &LeaseProof,
        now_ms: i64,
        lease_ttl_ms: i64,
        expected_row_version: i64,
    ) -> Result<(), LeaseError> {
        if lease_ttl_ms <= 0 {
            return Err(LeaseError::InvalidTtl);
        }
        self.validate(proof, now_ms, expected_row_version)?;
        let expires_at_ms = now_ms
            .checked_add(lease_ttl_ms)
            .ok_or(LeaseError::TimeOverflow)?;
        let next_row_version = self
            .row_version
            .checked_add(1)
            .ok_or(LeaseError::VersionExhausted)?;
        self.expires_at_ms = Some(expires_at_ms);
        self.row_version = next_row_version;
        Ok(())
    }

    /// Commits one fenced state/child write by validating the lease and
    /// advancing the CAS row version without changing the lease deadline.
    ///
    /// A persistence backend must include the same owner/attempt/generation,
    /// unexpired-deadline, and expected-version predicates in the atomic write.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError`] when validation fails or the row version cannot
    /// advance. An error leaves `self` unchanged.
    pub fn commit_fenced_write(
        &mut self,
        proof: &LeaseProof,
        now_ms: i64,
        expected_row_version: i64,
    ) -> Result<i64, LeaseError> {
        self.validate(proof, now_ms, expected_row_version)?;
        self.row_version = self
            .row_version
            .checked_add(1)
            .ok_or(LeaseError::VersionExhausted)?;
        Ok(self.row_version)
    }

    /// Releases a lease after validating its fence and CAS version.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError`] when validation fails or the row version cannot
    /// advance. An error leaves `self` unchanged.
    pub fn release(
        &mut self,
        proof: &LeaseProof,
        now_ms: i64,
        expected_row_version: i64,
    ) -> Result<(), LeaseError> {
        self.validate(proof, now_ms, expected_row_version)?;
        let next_row_version = self
            .row_version
            .checked_add(1)
            .ok_or(LeaseError::VersionExhausted)?;
        self.owner_id = None;
        self.attempt_id = None;
        self.expires_at_ms = None;
        self.row_version = next_row_version;
        Ok(())
    }

    fn ensure_version(&self, expected: i64) -> Result<(), LeaseError> {
        if self.row_version == expected {
            Ok(())
        } else {
            Err(LeaseError::VersionConflict {
                expected,
                actual: self.row_version,
            })
        }
    }
}

/// Stable fenced-lease transition error.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeaseError {
    /// Lease TTL was non-positive.
    InvalidTtl,
    /// Another lease remains active.
    AlreadyLeased,
    /// Lease has expired.
    Expired,
    /// Owner, attempt, resource, or fencing token is stale.
    StaleFence,
    /// CAS row version differs.
    VersionConflict {
        /// Expected version.
        expected: i64,
        /// Current version.
        actual: i64,
    },
    /// Fencing generation cannot advance.
    GenerationExhausted,
    /// CAS version cannot advance.
    VersionExhausted,
    /// Lease deadline overflowed.
    TimeOverflow,
}

impl fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTtl => formatter.write_str("lease ttl must be positive"),
            Self::AlreadyLeased => formatter.write_str("resource is already leased"),
            Self::Expired => formatter.write_str("lease expired"),
            Self::StaleFence => formatter.write_str("lease fence is stale"),
            Self::VersionConflict { expected, actual } => write!(
                formatter,
                "lease version conflict: expected {expected}, actual {actual}"
            ),
            Self::GenerationExhausted => formatter.write_str("lease generation exhausted"),
            Self::VersionExhausted => formatter.write_str("lease version exhausted"),
            Self::TimeOverflow => formatter.write_str("lease deadline overflowed"),
        }
    }
}

impl Error for LeaseError {}
