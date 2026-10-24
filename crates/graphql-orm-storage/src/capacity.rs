//! Bounded destination capacity observations.
//!
//! A [`CapacityObservation`] reports what one configured destination told the
//! authenticated caller about its remaining allocation at one instant. It is
//! bound to a non-secret [`CapacityBinding`] of the backend, location, account
//! and capacity-relevant configuration, and it expires. Callers revalidate an
//! observation with [`CapacityObservation::validate_for`] immediately before
//! admission and observe again when it is stale or the binding changed.
//!
//! Observations are provider facts only. Consumers still subtract their own
//! active reservations and service-owned quotas. [`CapacityState::Unknown`] is
//! never unlimited, and [`CapacityState::ProviderManaged`] is not an unlimited
//! reservation grant.

use std::time::Duration;

use time::OffsetDateTime;

use crate::{StorageBackend, sha256_hex};

/// Default maximum age of a capacity observation.
pub const DEFAULT_CAPACITY_MAX_AGE: Duration = Duration::from_secs(30);

/// Longest maximum age a caller may request for a capacity observation.
pub const CAPACITY_MAX_AGE_LIMIT: Duration = Duration::from_secs(10 * 60);

const BINDING_DOMAIN: &str = "graphql-orm-storage/capacity-binding/v1";

/// Parameters for one capacity observation.
///
/// Every observation performs fresh provider I/O; providers never return a
/// cached observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacityRequest {
    /// Maximum age of the observation, measured from when the provider
    /// measurement began rather than when it completed.
    ///
    /// Values above [`CAPACITY_MAX_AGE_LIMIT`] are clamped to that limit.
    pub max_age: Duration,
}

impl Default for CapacityRequest {
    fn default() -> Self {
        Self {
            max_age: DEFAULT_CAPACITY_MAX_AGE,
        }
    }
}

impl CapacityRequest {
    /// Returns the effective, clamped maximum age.
    #[must_use]
    pub fn effective_max_age(&self) -> Duration {
        self.max_age.min(CAPACITY_MAX_AGE_LIMIT)
    }
}

/// Non-secret identity of the destination an observation applies to.
///
/// The digest covers the backend kind and every provider-supplied component:
/// location, authenticated account and capacity-relevant configuration.
/// Secrets such as passwords or secret keys are never part of the binding.
/// Any change to those inputs yields a different binding.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CapacityBinding {
    backend: StorageBackend,
    digest: String,
}

impl CapacityBinding {
    /// Builds a binding from ordered `(name, value)` components.
    ///
    /// Providers must include their location, authenticated account identity
    /// and any configuration that changes what capacity is observed. Never
    /// include secrets.
    #[must_use]
    pub fn new(backend: StorageBackend, components: &[(&str, &str)]) -> Self {
        let mut preimage = Vec::new();
        push_field(&mut preimage, BINDING_DOMAIN);
        push_field(&mut preimage, backend.as_str());
        for (name, value) in components {
            push_field(&mut preimage, name);
            push_field(&mut preimage, value);
        }
        Self {
            backend,
            digest: sha256_hex(&preimage),
        }
    }

    /// Returns the backend kind this binding identifies.
    #[must_use]
    pub const fn backend(&self) -> StorageBackend {
        self.backend
    }

    /// Returns the lowercase hexadecimal SHA-256 digest of the binding.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

fn push_field(preimage: &mut Vec<u8>, value: &str) {
    preimage.extend_from_slice(&(value.len() as u64).to_be_bytes());
    preimage.extend_from_slice(value.as_bytes());
}

/// Redaction-safe identity of the physical placement that was observed.
///
/// Examples are a local filesystem device/identifier pair or an SMB volume
/// serial number. Comparing the placements of two observations of the same
/// binding detects that the destination was moved to a different volume.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CapacityPlacement(pub String);

/// One bounded capacity observation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CapacityObservation {
    /// Destination this observation applies to.
    pub binding: CapacityBinding,
    /// Observed physical placement, when the provider reports one.
    pub placement: Option<CapacityPlacement>,
    /// When the provider measurement began.
    pub observed_at: OffsetDateTime,
    /// First instant at which the observation is stale: `observed_at` plus
    /// the effective maximum age.
    pub valid_until: OffsetDateTime,
    /// Observed capacity state.
    pub state: CapacityState,
}

impl CapacityObservation {
    /// Builds an observation whose measurement began at `observed_at`.
    ///
    /// Providers must take `observed_at` before issuing the first provider
    /// request, so slow measurements expire earlier rather than later.
    #[must_use]
    pub fn new(
        binding: CapacityBinding,
        placement: Option<CapacityPlacement>,
        state: CapacityState,
        observed_at: OffsetDateTime,
        request: &CapacityRequest,
    ) -> Self {
        let max_age = request.effective_max_age();
        let max_age = time::Duration::try_from(max_age).unwrap_or_default();
        Self {
            binding,
            placement,
            observed_at,
            valid_until: observed_at.saturating_add(max_age),
            state,
        }
    }

    /// Returns the state when this observation is current for `binding`.
    ///
    /// Call this immediately before admission with the destination's current
    /// [`crate::BlobStore::capacity_binding`]. A rejected observation must be
    /// replaced by a fresh one; it must never be treated as available capacity.
    ///
    /// # Errors
    ///
    /// Returns [`CapacityValidationError`] when the binding differs, the
    /// observation expired, or `now` precedes the observation time.
    pub fn validate_for(
        &self,
        binding: &CapacityBinding,
        now: OffsetDateTime,
    ) -> Result<&CapacityState, CapacityValidationError> {
        if &self.binding != binding {
            return Err(CapacityValidationError::BindingMismatch);
        }
        if now < self.observed_at {
            return Err(CapacityValidationError::ObservedInFuture {
                observed_at: self.observed_at,
            });
        }
        if now >= self.valid_until {
            return Err(CapacityValidationError::Expired {
                valid_until: self.valid_until,
            });
        }
        Ok(&self.state)
    }

    /// Returns whether a newer observation of the same binding reports the
    /// same physical placement as this one.
    ///
    /// Returns `false` when either placement is absent or the bindings differ.
    #[must_use]
    pub fn same_placement(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.placement.is_some()
            && self.placement == other.placement
    }
}

/// Reason an observation cannot be used for admission.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CapacityValidationError {
    /// The destination, account or capacity configuration changed.
    #[error("capacity observation belongs to a different destination binding")]
    BindingMismatch,
    /// The observation is stale.
    #[error("capacity observation expired at {valid_until}")]
    Expired {
        /// First instant at which the observation was stale.
        valid_until: OffsetDateTime,
    },
    /// The validation clock is earlier than the observation time.
    #[error("capacity observation time {observed_at} is after the validation time")]
    ObservedInFuture {
        /// Recorded observation time.
        observed_at: OffsetDateTime,
    },
}

/// Capacity state reported by one destination.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum CapacityState {
    /// The destination reported finite byte capacity for the caller.
    Known(KnownCapacity),
    /// The provider performs no physical-byte capacity admission.
    ///
    /// This is not an unlimited grant: consumers must still enforce a finite
    /// service-owned quota.
    ProviderManaged(ProviderManagedCapacity),
    /// Capacity could not be established. Never treat this as unlimited.
    Unknown(UnknownCapacity),
}

impl CapacityState {
    /// Returns the known capacity, if any.
    #[must_use]
    pub const fn known(&self) -> Option<&KnownCapacity> {
        match self {
            Self::Known(known) => Some(known),
            Self::ProviderManaged(_) | Self::Unknown(_) => None,
        }
    }
}

/// Finite capacity reported to the authenticated caller.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KnownCapacity {
    /// Bytes the authenticated caller may still allocate at the destination.
    ///
    /// Already reduced by every limit described in [`Self::quota`] and by any
    /// filesystem reservation the caller cannot use.
    pub caller_available_bytes: u64,
    /// Total size of the destination volume as reported to the caller.
    pub total_bytes: Option<u64>,
    /// Free bytes on the destination volume regardless of the caller's limits.
    ///
    /// Some servers report the caller-limited value here too; it is never
    /// larger than the provider's physical free space.
    pub physical_available_bytes: Option<u64>,
    /// How quotas were accounted for in [`Self::caller_available_bytes`].
    pub quota: CapacityQuota,
}

impl KnownCapacity {
    /// Returns whether every applicable quota was observed or accounted for
    /// by the provider, so that [`Self::caller_available_bytes`] is complete.
    #[must_use]
    pub const fn quota_accounted(&self) -> bool {
        !matches!(self.quota, CapacityQuota::NotObserved { .. })
    }
}

/// Quota accounting for a [`KnownCapacity`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum CapacityQuota {
    /// Explicit finite quotas apply to the caller; the tightest one is
    /// reflected in the caller-available bytes.
    Limited(Vec<QuotaLimit>),
    /// Quota enforcement is not active for the caller at this destination.
    NotEnforced,
    /// The provider's caller-available value already includes any quota it
    /// enforces for the authenticated account, without itemising it.
    ProviderAccounted,
    /// Quotas could not be observed; caller-available bytes may overstate
    /// what the caller can allocate.
    NotObserved {
        /// Redaction-safe reason.
        reason: String,
    },
}

/// Principal class a quota applies to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum QuotaScope {
    /// Per-user quota.
    User,
    /// Per-group quota.
    Group,
    /// Per-project (directory tree) quota.
    Project,
}

/// One observed finite quota.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QuotaLimit {
    /// Principal class.
    pub scope: QuotaScope,
    /// Numeric principal identifier, such as a user, group or project id.
    pub principal: u64,
    /// Bytes already charged to the principal.
    pub used_bytes: u64,
    /// Limit currently enforced for new allocations.
    pub limit_bytes: u64,
    /// Soft limit, when one is configured.
    pub soft_limit_bytes: Option<u64>,
    /// Hard limit, when one is configured.
    pub hard_limit_bytes: Option<u64>,
}

impl QuotaLimit {
    /// Returns the bytes this quota still allows.
    #[must_use]
    pub const fn remaining_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(self.used_bytes)
    }
}

/// Provider-managed capacity details.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderManagedCapacity {
    /// How the provider-managed state was established.
    pub basis: ProviderManagedBasis,
}

/// Basis for a provider-managed capacity state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ProviderManagedBasis {
    /// The operator declared the destination provider-managed in its
    /// configuration, and the authenticated account reached the exact
    /// destination during this observation.
    ///
    /// The provider itself did not attest the absence of a finite quota;
    /// reachability proves access only. Consumers decide whether an operator
    /// declaration is sufficient for admission.
    OperatorDeclared,
    /// The destination is an Amazon S3 general-purpose bucket reached through
    /// the AWS S3 endpoint for its region, owned by the expected account, and
    /// AWS documents no maximum size for general-purpose buckets.
    AwsGeneralPurposeBucket,
}

/// Details for an unknown capacity state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnknownCapacity {
    /// Why capacity is unknown.
    pub reason: CapacityUnknownReason,
}

impl UnknownCapacity {
    /// Builds an unknown state for a provider without capacity observation.
    #[must_use]
    pub const fn unsupported() -> Self {
        Self {
            reason: CapacityUnknownReason::Unsupported,
        }
    }
}

/// Reason capacity is unknown.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum CapacityUnknownReason {
    /// The provider implementation does not observe capacity.
    Unsupported,
    /// The provider is reachable but its API does not report a finite quota
    /// or whether one applies.
    ProviderQuotaNotObservable,
    /// The current platform has no supported capacity interface.
    PlatformUnsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(location: &str) -> CapacityBinding {
        CapacityBinding::new(
            StorageBackend::Local,
            &[("location", location), ("account", "1000")],
        )
    }

    fn known(available: u64) -> CapacityState {
        CapacityState::Known(KnownCapacity {
            caller_available_bytes: available,
            total_bytes: Some(available * 2),
            physical_available_bytes: Some(available),
            quota: CapacityQuota::NotEnforced,
        })
    }

    #[test]
    fn binding_is_stable_and_component_sensitive() {
        assert_eq!(binding("/a"), binding("/a"));
        assert_ne!(binding("/a"), binding("/b"));
        assert_eq!(binding("/a").digest().len(), 64);
        // Length-prefixed fields cannot be shifted between components.
        assert_ne!(
            CapacityBinding::new(StorageBackend::Smb, &[("ab", "c")]),
            CapacityBinding::new(StorageBackend::Smb, &[("a", "bc")]),
        );
        assert_ne!(
            CapacityBinding::new(StorageBackend::Smb, &[("x", "y")]),
            CapacityBinding::new(StorageBackend::S3, &[("x", "y")]),
        );
    }

    #[test]
    fn validation_rejects_changed_binding_stale_and_future_observations() {
        let observed_at = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1);
        let request = CapacityRequest {
            max_age: Duration::from_secs(30),
        };
        let observation =
            CapacityObservation::new(binding("/a"), None, known(5), observed_at, &request);

        let current = observed_at + time::Duration::seconds(29);
        let state = observation.validate_for(&binding("/a"), current);
        assert_eq!(state, Ok(&known(5)));
        assert_eq!(
            observation.validate_for(&binding("/b"), observed_at),
            Err(CapacityValidationError::BindingMismatch)
        );
        let stale = observed_at + time::Duration::seconds(30);
        assert_eq!(
            observation.validate_for(&binding("/a"), stale),
            Err(CapacityValidationError::Expired { valid_until: stale })
        );
        let early = observed_at - time::Duration::seconds(1);
        assert_eq!(
            observation.validate_for(&binding("/a"), early),
            Err(CapacityValidationError::ObservedInFuture { observed_at })
        );
    }

    #[test]
    fn max_age_is_clamped() {
        let request = CapacityRequest {
            max_age: Duration::from_secs(86_400),
        };
        let observation = CapacityObservation::new(
            binding("/a"),
            None,
            known(1),
            OffsetDateTime::UNIX_EPOCH,
            &request,
        );
        assert_eq!(
            observation.valid_until - observation.observed_at,
            time::Duration::try_from(CAPACITY_MAX_AGE_LIMIT).expect("duration")
        );
    }

    #[test]
    fn placement_comparison_requires_same_binding_and_present_placement() {
        let request = CapacityRequest::default();
        let at = OffsetDateTime::UNIX_EPOCH;
        let volume = |name: &str| Some(CapacityPlacement(name.to_string()));
        let first = CapacityObservation::new(binding("/a"), volume("v1"), known(1), at, &request);
        let same = CapacityObservation::new(binding("/a"), volume("v1"), known(2), at, &request);
        let moved = CapacityObservation::new(binding("/a"), volume("v2"), known(2), at, &request);
        let other = CapacityObservation::new(binding("/b"), volume("v1"), known(2), at, &request);
        let absent = CapacityObservation::new(binding("/a"), None, known(2), at, &request);
        assert!(first.same_placement(&same));
        assert!(!first.same_placement(&moved));
        assert!(!first.same_placement(&other));
        assert!(!absent.same_placement(&absent.clone()));
    }

    #[test]
    fn quota_accounting_and_remaining_bytes() {
        let limit = QuotaLimit {
            scope: QuotaScope::User,
            principal: 1000,
            used_bytes: 70,
            limit_bytes: 50,
            soft_limit_bytes: Some(50),
            hard_limit_bytes: Some(100),
        };
        assert_eq!(limit.remaining_bytes(), 0);
        let mut capacity = KnownCapacity {
            caller_available_bytes: 0,
            total_bytes: None,
            physical_available_bytes: None,
            quota: CapacityQuota::Limited(vec![limit]),
        };
        assert!(capacity.quota_accounted());
        capacity.quota = CapacityQuota::NotObserved {
            reason: "unsupported filesystem".to_string(),
        };
        assert!(!capacity.quota_accounted());
    }
}
