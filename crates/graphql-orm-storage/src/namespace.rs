//! Durable object-namespace identity and provider-derived capacity-domain facts.
//!
//! Two identities are deliberately separate:
//!
//! - A [`NamespaceId`] identifies one **object namespace**: exactly the
//!   configured root/prefix of a store, or an explicitly marked sub-prefix.
//!   It is stored in a reserved marker object and reconciles exact aliases of
//!   that namespace after restarts and credential rotation.
//! - [`DomainFact`]s describe the **physical/capacity domain**: where the
//!   root lives, the physical store holding it, and the quota constraints on
//!   it. They come from the provider, each with an explicit [`FactScope`].
//!   A marker never establishes that two namespaces have distinct capacity.
//!
//! Read-only observation ([`crate::BlobStore::observe_namespace`],
//! [`crate::BlobStore::observe_capacity_domain`]) never writes. Initialisation
//! ([`crate::BlobStore::initialise_namespace`]) is a separate, explicit
//! create-if-absent operation. Every result that cannot be proven is reported
//! as `Unknown`, `Unsupported` or `Conflict`, never as success or absence.

use std::{fmt, time::Duration};

use time::OffsetDateTime;
use uuid::Uuid;

use crate::{StorageBackend, StorageError, validate_blob_key};

/// Reserved basename of the namespace marker object.
pub const NAMESPACE_MARKER: &str = ".graphql-orm-namespace";

/// Reserved basename of an in-progress local marker publication.
pub const NAMESPACE_MARKER_PUBLISHING: &str = ".graphql-orm-namespace.publishing";

/// Marker format identifier.
pub const NAMESPACE_MARKER_FORMAT: &str = "graphql-orm-storage/namespace/v1";

/// Maximum accepted marker body size in bytes.
pub const MAX_NAMESPACE_MARKER_BYTES: usize = 1024;

/// Default deadline for one marker read.
pub const DEFAULT_MARKER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Random identity of one object namespace.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NamespaceId(Uuid);

impl NamespaceId {
    /// Generates a new random identity for a later explicit initialisation.
    #[must_use]
    pub fn new_random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wraps an existing identity, for example a proposal retried after a lost reply.
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Returns the identity as a UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for NamespaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.hyphenated())
    }
}

/// Which namespace of a store an operation addresses.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum NamespaceTarget {
    /// The configured root/prefix of the store, including every key under it.
    Root,
    /// An explicitly marked sub-prefix of the store (a validated blob key).
    SubPrefix(String),
}

impl NamespaceTarget {
    /// Returns the relative prefix, or `None` for the root.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidStorageKey`] for an unsafe or reserved sub-prefix.
    pub fn relative_prefix(
        &self,
        equivalence: KeyEquivalence,
    ) -> Result<Option<&str>, StorageError> {
        match self {
            Self::Root => Ok(None),
            Self::SubPrefix(prefix) => {
                validate_unreserved_key(prefix, equivalence)?;
                Ok(Some(prefix.as_str()))
            }
        }
    }
}

/// Observed state of one namespace marker.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum NamespaceState {
    /// A complete, valid marker was read.
    Initialised {
        /// Marker identity.
        id: NamespaceId,
    },
    /// No marker and no in-progress publication exist at this location.
    Uninitialised,
    /// A publication is visibly in progress or was interrupted (an Unknown state).
    PublicationInProgress,
    /// A marker key exists but is empty, partial, oversized, malformed or of
    /// a foreign format.
    Conflict {
        /// Redaction-safe reason.
        reason: String,
    },
    /// The provider cannot store or read markers.
    Unsupported,
    /// The marker could not be observed (denied, timed out, I/O failure).
    Unknown {
        /// Redaction-safe reason.
        reason: String,
    },
}

/// Bounded topology scan request. Absent scans are reported as `NotRequested`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TopologyScan {
    /// Ancestor markers above the configured root, within the provider's own
    /// authority boundary. Expands read authority, so it is opt-in.
    pub ancestors: Option<AncestorScan>,
    /// Markers below the configured root.
    pub nested: Option<NestedScan>,
}

/// Ancestor scan limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AncestorScan {
    /// Maximum number of ancestor levels to inspect.
    pub max_depth: usize,
}

/// Nested scan limits. Every limit is enforced; reaching one reports `Truncated`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NestedScan {
    /// Maximum directory or prefix depth below the root.
    pub max_depth: usize,
    /// Maximum provider listing pages or directory reads.
    pub max_pages: usize,
    /// Maximum keys or directory entries examined.
    pub max_keys: usize,
    /// Wall-clock budget for the whole scan.
    pub max_duration: Duration,
}

/// Completeness of a topology scan. Only `Complete` is evidence of absence.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ScanCompleteness {
    /// Every candidate within the requested scope was examined.
    Complete,
    /// A limit stopped the scan.
    Truncated {
        /// The limit that was reached.
        limit: String,
    },
    /// Part of the scope could not be read.
    Denied {
        /// Redaction-safe reason.
        reason: String,
    },
    /// The provider cannot perform this scan.
    Unsupported,
    /// The scan was not requested.
    NotRequested,
}

/// One marker found by a topology scan.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FoundNamespace {
    /// Location relative to the configured root. Ancestors use `../` components.
    pub relative_path: String,
    /// Marker state at that location.
    pub state: NamespaceState,
}

/// Result of one topology scan.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScanResult {
    /// Completeness of the scan.
    pub completeness: ScanCompleteness,
    /// Markers found. An empty list is absence only when `completeness` is `Complete`.
    pub found: Vec<FoundNamespace>,
}

impl ScanResult {
    /// A scan that was not requested.
    #[must_use]
    pub const fn not_requested() -> Self {
        Self {
            completeness: ScanCompleteness::NotRequested,
            found: Vec::new(),
        }
    }

    /// A scan the provider cannot perform.
    #[must_use]
    pub const fn unsupported() -> Self {
        Self {
            completeness: ScanCompleteness::Unsupported,
            found: Vec::new(),
        }
    }
}

/// Identity of the backend connection an observation was made through.
///
/// An observation is a point-in-time description, not a lease. Fenced
/// operations compare this epoch before writing.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObservationEpoch {
    /// Random per constructed backend instance (clones share it).
    pub backend_instance: Uuid,
    /// Provider reconnect counter, where the provider has one (SMB); otherwise 0.
    pub connection_generation: u64,
    /// Redaction-safe witness of the opened root, where available.
    pub root_witness: Option<String>,
}

/// One namespace observation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NamespaceObservation {
    /// Observed namespace.
    pub target: NamespaceTarget,
    /// Marker state.
    pub state: NamespaceState,
    /// Location of the namespace root, when the provider can describe it.
    pub root_location: Option<DomainFact>,
    /// Ancestor scan result.
    pub ancestors: ScanResult,
    /// Nested scan result.
    pub nested: ScanResult,
    /// Connection epoch of this observation.
    pub epoch: ObservationEpoch,
    /// When the observation started.
    pub observed_at: OffsetDateTime,
}

/// Result of an explicit initialisation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum NamespaceInitialisation {
    /// The proposed identity was published.
    Created(NamespaceId),
    /// The same proposed identity is already present, which reconciles a lost reply.
    AlreadyInitialisedSame(NamespaceId),
    /// A different identity is present. It is never overwritten or adopted.
    AlreadyInitialisedOther(NamespaceId),
    /// The marker location is unusable (foreign, partial or competing publication).
    Conflict {
        /// Redaction-safe reason.
        reason: String,
    },
}

/// Expected namespace and epoch for a fenced write.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NamespaceFence {
    /// Target the fence applies to.
    pub target: NamespaceTarget,
    /// Namespace identity the caller observed.
    pub namespace: NamespaceId,
    /// Epoch of that observation.
    pub epoch: ObservationEpoch,
}

/// Comparison scope of a [`DomainFact`].
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum FactScope {
    /// Comparable only within one host boot (for example a raw device number).
    HostBoot {
        /// Kernel boot identifier.
        boot_id: String,
    },
    /// Comparable while the same filesystem or volume identity is reported.
    StoreIdentity,
    /// Provider-global identity.
    ProviderGlobal,
    /// No comparable scope; never usable for equality.
    Unspecified,
}

/// One complete, provider-scoped fact. Raw single values never stand alone.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DomainFact {
    /// Where the namespace root is. A location, not a physical or quota domain.
    RootLocation {
        /// Provider kind.
        provider: StorageBackend,
        /// Complete ordered tuple.
        tuple: Vec<(String, String)>,
        /// Comparison scope.
        scope: FactScope,
    },
    /// The physical store (filesystem, pool, volume or bucket) holding the root.
    PhysicalStore {
        /// Provider kind.
        provider: StorageBackend,
        /// Complete ordered tuple.
        tuple: Vec<(String, String)>,
        /// Comparison scope.
        scope: FactScope,
    },
    /// A quota constraint, always bound to the physical store it applies to.
    QuotaConstraint {
        /// Physical store the quota applies to.
        store: Box<DomainFact>,
        /// Quota type, for example `user`, `group` or `project`.
        quota_type: String,
        /// Principal identifier.
        principal: String,
        /// Comparison scope.
        scope: FactScope,
    },
}

/// Outcome of comparing two facts of the same category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactComparison {
    /// Same category, provider and complete tuple within a covering scope.
    Same,
    /// Both facts are complete and comparable, and they differ.
    Different,
    /// The facts cannot be compared: a category, provider or scope mismatch,
    /// or an incomplete tuple. Never evidence of distinctness.
    Unknown,
}

fn scopes_cover(left: &FactScope, right: &FactScope) -> bool {
    match (left, right) {
        (FactScope::HostBoot { boot_id: a }, FactScope::HostBoot { boot_id: b }) => {
            !a.is_empty() && a == b
        }
        (FactScope::StoreIdentity, FactScope::StoreIdentity)
        | (FactScope::ProviderGlobal, FactScope::ProviderGlobal) => true,
        _ => false,
    }
}

fn complete(tuple: &[(String, String)]) -> bool {
    !tuple.is_empty()
        && tuple
            .iter()
            .all(|(name, value)| !name.is_empty() && !value.is_empty())
}

fn compare_tuples(
    left: (&StorageBackend, &[(String, String)], &FactScope),
    right: (&StorageBackend, &[(String, String)], &FactScope),
) -> FactComparison {
    if left.0 != right.0
        || !complete(left.1)
        || !complete(right.1)
        || !scopes_cover(left.2, right.2)
    {
        return FactComparison::Unknown;
    }
    let left_names: Vec<&str> = left.1.iter().map(|(name, _)| name.as_str()).collect();
    let right_names: Vec<&str> = right.1.iter().map(|(name, _)| name.as_str()).collect();
    if left_names != right_names {
        return FactComparison::Unknown;
    }
    if left.1 == right.1 {
        FactComparison::Same
    } else {
        FactComparison::Different
    }
}

impl DomainFact {
    /// Compares two facts under the scoped equality rule.
    ///
    /// Facts of different categories never compare. A shared quota principal
    /// on different physical stores is `Different` or `Unknown`, never `Same`.
    #[must_use]
    pub fn compare(&self, other: &Self) -> FactComparison {
        match (self, other) {
            (
                Self::RootLocation {
                    provider: a,
                    tuple: ta,
                    scope: sa,
                },
                Self::RootLocation {
                    provider: b,
                    tuple: tb,
                    scope: sb,
                },
            )
            | (
                Self::PhysicalStore {
                    provider: a,
                    tuple: ta,
                    scope: sa,
                },
                Self::PhysicalStore {
                    provider: b,
                    tuple: tb,
                    scope: sb,
                },
            ) => compare_tuples((a, ta.as_slice(), sa), (b, tb.as_slice(), sb)),
            (
                Self::QuotaConstraint {
                    store: store_a,
                    quota_type: type_a,
                    principal: pa,
                    scope: sa,
                },
                Self::QuotaConstraint {
                    store: store_b,
                    quota_type: type_b,
                    principal: pb,
                    scope: sb,
                },
            ) => {
                if type_a.is_empty() || pa.is_empty() || !scopes_cover(sa, sb) {
                    return FactComparison::Unknown;
                }
                match store_a.compare(store_b) {
                    FactComparison::Same if type_a == type_b && pa == pb => FactComparison::Same,
                    FactComparison::Same => FactComparison::Different,
                    FactComparison::Different => FactComparison::Different,
                    FactComparison::Unknown => FactComparison::Unknown,
                }
            }
            _ => FactComparison::Unknown,
        }
    }
}

/// State of a capacity-domain observation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DomainState {
    /// Every applicable fact category was observed.
    Observed,
    /// Some categories were observed; the named ones were not.
    Partial {
        /// Categories not observed.
        missing: Vec<String>,
    },
    /// The provider cannot report domain facts.
    Unsupported,
    /// Domain facts could not be established.
    Unknown {
        /// Redaction-safe reason.
        reason: String,
    },
}

/// One capacity-domain observation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CapacityDomainObservation {
    /// Observed facts.
    pub facts: Vec<DomainFact>,
    /// Observation state.
    pub state: DomainState,
    /// Connection epoch of this observation.
    pub epoch: ObservationEpoch,
    /// When the observation started.
    pub observed_at: OffsetDateTime,
}

/// Relationship between two namespace observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamespaceRelation {
    /// Same identity at the same root location: an alias of one namespace.
    SameNamespace,
    /// Same identity at different root locations: a copied or cloned marker.
    /// Never alias proof.
    Duplicated,
    /// Different identities. This says nothing about capacity.
    DifferentNamespaces,
    /// Not comparable (uninitialised, conflicting or unknown state or location).
    Unknown,
}

/// Compares two observations without inferring capacity distinctness.
#[must_use]
pub fn relate_namespaces(
    left: &NamespaceObservation,
    right: &NamespaceObservation,
) -> NamespaceRelation {
    let (NamespaceState::Initialised { id: a }, NamespaceState::Initialised { id: b }) =
        (&left.state, &right.state)
    else {
        return NamespaceRelation::Unknown;
    };
    if a != b {
        return NamespaceRelation::DifferentNamespaces;
    }
    match (&left.root_location, &right.root_location) {
        (Some(x), Some(y)) => match x.compare(y) {
            FactComparison::Same => NamespaceRelation::SameNamespace,
            FactComparison::Different => NamespaceRelation::Duplicated,
            FactComparison::Unknown => NamespaceRelation::Unknown,
        },
        _ => NamespaceRelation::Unknown,
    }
}

/// Canonical marker body: one JSON object followed by one newline.
#[must_use]
pub fn encode_marker(id: NamespaceId) -> Vec<u8> {
    format!("{{\"format\":\"{NAMESPACE_MARKER_FORMAT}\",\"namespace_id\":\"{id}\"}}\n").into_bytes()
}

/// Strictly decodes a marker body.
///
/// Only the exact canonical form produced by [`encode_marker`] is accepted:
/// exactly the `format` and `namespace_id` fields in that order, no
/// duplicates, no whitespace, a canonical lower-case hyphenated UUID, and at
/// most one trailing newline. Anything else is a `Conflict`.
#[must_use]
pub fn decode_marker(bytes: &[u8]) -> NamespaceState {
    let conflict = |reason: &str| NamespaceState::Conflict {
        reason: reason.to_string(),
    };
    if bytes.is_empty() {
        return conflict("empty marker");
    }
    if bytes.len() > MAX_NAMESPACE_MARKER_BYTES {
        return conflict("oversized marker");
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return conflict("marker is not UTF-8");
    };
    let text = text.strip_suffix('\n').unwrap_or(text);
    let prefix = format!("{{\"format\":\"{NAMESPACE_MARKER_FORMAT}\",\"namespace_id\":\"");
    let Some(rest) = text.strip_prefix(&prefix) else {
        return conflict("foreign or non-canonical marker format");
    };
    let Some(candidate) = rest.strip_suffix("\"}") else {
        return conflict("non-canonical marker body");
    };
    match Uuid::parse_str(candidate) {
        Ok(id) if id.hyphenated().to_string() == candidate => NamespaceState::Initialised {
            id: NamespaceId(id),
        },
        _ => conflict("invalid namespace identifier"),
    }
}

/// How a provider resolves key spellings to the same stored object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum KeyEquivalence {
    /// Byte-exact keys (S3).
    Exact,
    /// Possibly case-insensitive local filesystems: case folding, plus
    /// trailing dots and spaces trimmed.
    CaseFolding,
    /// SMB/Windows: case folding, trailing dots and spaces trimmed, alternate
    /// data streams (`:`), and unprovable 8.3 short-name forms refused.
    Windows,
}

const RESERVED_NAMES: [&str; 2] = [NAMESPACE_MARKER, NAMESPACE_MARKER_PUBLISHING];

fn short_name_unprovable(component: &str) -> bool {
    // An 8.3 alias looks like `PREFIX~N...`. If the alphanumeric prefix is a
    // prefix of a reserved name's alphanumerics, the server could resolve it
    // to the marker; that cannot be ruled out from the key, so it is refused.
    let Some((prefix, suffix)) = component.split_once('~') else {
        return false;
    };
    if !suffix.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    let prefix: String = prefix
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if prefix.is_empty() || prefix.len() > 6 {
        return false;
    }
    RESERVED_NAMES.iter().any(|name| {
        let reserved: String = name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        reserved.starts_with(&prefix)
    })
}

/// Returns whether a key component addresses a reserved name under `equivalence`.
#[must_use]
pub fn is_reserved_component(component: &str, equivalence: KeyEquivalence) -> bool {
    match equivalence {
        KeyEquivalence::Exact => RESERVED_NAMES.contains(&component),
        KeyEquivalence::CaseFolding | KeyEquivalence::Windows => {
            let base = if equivalence == KeyEquivalence::Windows {
                component.split(':').next().unwrap_or(component)
            } else {
                component
            };
            let trimmed = base.trim_end_matches(['.', ' ']).to_lowercase();
            RESERVED_NAMES.iter().any(|name| trimmed == *name)
                || (equivalence == KeyEquivalence::Windows && short_name_unprovable(base))
        }
    }
}

/// Validates an ordinary blob key and rejects any reserved component.
///
/// Applies to every ordinary read, write, copy and delete path of a provider,
/// so a parent store can never overwrite or delete a nested store's marker.
/// Under [`KeyEquivalence::Windows`], any `:` is also rejected because it can
/// address an alternate data stream.
///
/// # Errors
///
/// Returns [`StorageError::InvalidStorageKey`] for unsafe or reserved keys.
pub fn validate_unreserved_key(key: &str, equivalence: KeyEquivalence) -> Result<(), StorageError> {
    validate_blob_key(key)?;
    let reserved = key.split('/').any(|component| {
        is_reserved_component(component, equivalence)
            || (equivalence == KeyEquivalence::Windows && component.contains(':'))
    });
    if reserved {
        return Err(StorageError::InvalidStorageKey {
            key: key.to_string(),
        });
    }
    Ok(())
}

/// Returns whether an ordinary listing must hide this key.
#[must_use]
pub fn is_reserved_key(key: &str, equivalence: KeyEquivalence) -> bool {
    key.split('/')
        .any(|component| is_reserved_component(component, equivalence))
}

/// Error returned when a fence no longer matches the destination.
#[must_use]
#[cfg_attr(
    not(any(feature = "local", feature = "s3", feature = "smb")),
    allow(dead_code)
)]
pub(crate) fn fence_invalidated(key: &str, reason: &str) -> StorageError {
    StorageError::PreconditionFailed {
        key: key.to_string(),
        condition: format!("namespace fence invalidated: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(scope: FactScope, value: &str) -> DomainFact {
        DomainFact::RootLocation {
            provider: StorageBackend::Local,
            tuple: vec![
                ("dev".to_string(), "1".to_string()),
                ("ino".to_string(), value.to_string()),
            ],
            scope,
        }
    }

    fn boot(id: &str) -> FactScope {
        FactScope::HostBoot {
            boot_id: id.to_string(),
        }
    }

    #[test]
    fn marker_round_trips_only_in_canonical_form() {
        let id = NamespaceId::new_random();
        assert_eq!(
            decode_marker(&encode_marker(id)),
            NamespaceState::Initialised { id }
        );
        let body = String::from_utf8(encode_marker(id)).expect("utf8");
        let without_newline = body.trim_end().to_string();
        assert_eq!(
            decode_marker(without_newline.as_bytes()),
            NamespaceState::Initialised { id }
        );
        for bad in [
            String::new(),
            format!("{body}\n"),
            body.replace("namespace/v1", "namespace/v2"),
            body.replace(",\"namespace_id\"", ", \"namespace_id\""),
            body.replace('}', ",\"extra\":1}"),
            format!(
                "{{\"format\":\"{NAMESPACE_MARKER_FORMAT}\",\"namespace_id\":\"{id}\",\"namespace_id\":\"{id}\"}}"
            ),
            body.to_uppercase(),
            body.replace(&id.to_string(), &id.as_uuid().simple().to_string()),
            body[..body.len() / 2].to_string(),
            "x".repeat(MAX_NAMESPACE_MARKER_BYTES + 1),
        ] {
            assert!(
                matches!(
                    decode_marker(bad.as_bytes()),
                    NamespaceState::Conflict { .. }
                ),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn reserved_names_follow_provider_equivalence() {
        assert!(is_reserved_component(
            NAMESPACE_MARKER,
            KeyEquivalence::Exact
        ));
        assert!(!is_reserved_component(
            ".GRAPHQL-ORM-NAMESPACE",
            KeyEquivalence::Exact
        ));
        for alias in [
            ".GRAPHQL-ORM-NAMESPACE",
            ".Graphql-Orm-Namespace",
            ".graphql-orm-namespace.",
            ".graphql-orm-namespace ",
        ] {
            assert!(
                is_reserved_component(alias, KeyEquivalence::CaseFolding),
                "{alias}"
            );
            assert!(
                is_reserved_component(alias, KeyEquivalence::Windows),
                "{alias}"
            );
        }
        for alias in [
            ".graphql-orm-namespace:stream",
            ".graphql-orm-namespace::$DATA",
            "GRAPHQ~1",
            "graphq~2.pub",
        ] {
            assert!(
                is_reserved_component(alias, KeyEquivalence::Windows),
                "{alias}"
            );
        }
        assert!(!is_reserved_component("PROGRA~1", KeyEquivalence::Windows));
        assert!(!is_reserved_component("ordinary", KeyEquivalence::Windows));
    }

    #[test]
    fn ordinary_keys_reject_reserved_components_at_any_depth() {
        for key in [
            "a/.graphql-orm-namespace",
            ".graphql-orm-namespace.publishing",
            "a/b/.graphql-orm-namespace/c",
        ] {
            assert!(
                validate_unreserved_key(key, KeyEquivalence::Exact).is_err(),
                "{key}"
            );
        }
        assert!(
            validate_unreserved_key("a/.GRAPHQL-ORM-NAMESPACE", KeyEquivalence::CaseFolding)
                .is_err()
        );
        assert!(validate_unreserved_key("a/file:stream", KeyEquivalence::Windows).is_err());
        assert!(validate_unreserved_key("a/file:stream", KeyEquivalence::Exact).is_ok());
        assert!(validate_unreserved_key("a/b/c.txt", KeyEquivalence::Windows).is_ok());
        assert!(is_reserved_key(
            "x/.graphql-orm-namespace",
            KeyEquivalence::Exact
        ));
    }

    #[test]
    fn scoped_fact_equality_never_infers_distinctness_from_incomplete_scope() {
        assert_eq!(
            location(boot("b1"), "5").compare(&location(boot("b1"), "5")),
            FactComparison::Same
        );
        assert_eq!(
            location(boot("b1"), "5").compare(&location(boot("b1"), "6")),
            FactComparison::Different
        );
        assert_eq!(
            location(boot("b1"), "5").compare(&location(boot("b2"), "5")),
            FactComparison::Unknown
        );
        assert_eq!(
            location(FactScope::Unspecified, "5").compare(&location(FactScope::Unspecified, "5")),
            FactComparison::Unknown
        );
        assert_eq!(
            location(boot("b1"), "").compare(&location(boot("b1"), "")),
            FactComparison::Unknown
        );
        let store = |value: &str| DomainFact::PhysicalStore {
            provider: StorageBackend::Local,
            tuple: vec![("fs".to_string(), value.to_string())],
            scope: FactScope::StoreIdentity,
        };
        assert_eq!(
            location(boot("b1"), "5").compare(&store("5")),
            FactComparison::Unknown
        );
        let quota = |fs: &str, principal: &str| DomainFact::QuotaConstraint {
            store: Box::new(store(fs)),
            quota_type: "user".to_string(),
            principal: principal.to_string(),
            scope: FactScope::StoreIdentity,
        };
        assert_eq!(
            quota("fs1", "1000").compare(&quota("fs1", "1000")),
            FactComparison::Same
        );
        // A shared principal never joins two filesystems.
        assert_eq!(
            quota("fs1", "1000").compare(&quota("fs2", "1000")),
            FactComparison::Different
        );
    }

    #[test]
    fn copied_markers_are_duplicated_not_aliases() {
        let id = NamespaceId::new_random();
        let observation =
            |location: Option<DomainFact>, state: NamespaceState| NamespaceObservation {
                target: NamespaceTarget::Root,
                state,
                root_location: location,
                ancestors: ScanResult::not_requested(),
                nested: ScanResult::not_requested(),
                epoch: ObservationEpoch {
                    backend_instance: Uuid::nil(),
                    connection_generation: 0,
                    root_witness: None,
                },
                observed_at: OffsetDateTime::UNIX_EPOCH,
            };
        let here = observation(
            Some(location(boot("b"), "1")),
            NamespaceState::Initialised { id },
        );
        let alias = observation(
            Some(location(boot("b"), "1")),
            NamespaceState::Initialised { id },
        );
        let copy = observation(
            Some(location(boot("b"), "2")),
            NamespaceState::Initialised { id },
        );
        let unlocated = observation(None, NamespaceState::Initialised { id });
        let other = observation(
            None,
            NamespaceState::Initialised {
                id: NamespaceId::new_random(),
            },
        );
        assert_eq!(
            relate_namespaces(&here, &alias),
            NamespaceRelation::SameNamespace
        );
        assert_eq!(
            relate_namespaces(&here, &copy),
            NamespaceRelation::Duplicated
        );
        assert_eq!(
            relate_namespaces(&here, &unlocated),
            NamespaceRelation::Unknown
        );
        assert_eq!(
            relate_namespaces(&here, &other),
            NamespaceRelation::DifferentNamespaces
        );
        let pending = observation(None, NamespaceState::PublicationInProgress);
        assert_eq!(
            relate_namespaces(&here, &pending),
            NamespaceRelation::Unknown
        );
    }
}
