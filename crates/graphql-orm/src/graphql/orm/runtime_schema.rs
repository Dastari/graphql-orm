//! Owned, backend-neutral runtime schema IR.
//!
//! This module lets a host load collection definitions from a durable catalog at runtime and
//! converge them with compile-time derived entities on one semantic representation. All strings
//! are owned; schema objects are referenced through distinct stable ID newtypes rather than
//! interchangeable strings.
//!
//! The IR deliberately covers structural schema semantics only in this slice: collections,
//! fields, primary keys, relations with ordered key pairs, secondary/unique indexes, composite
//! unique groups, defaults, and deterministic ordering. Schema-bound query execution and owned migration planning reuse this IR;
//! opt-in dynamic GraphQL registration remains a later slice. Spatial columns,
//! full-text search, partial/GiST indexes, check constraints, backup ordering, policy hook
//! names, and relation change propagation are deliberately not represented yet; conversion
//! reports them as [`RuntimeSchemaDiagnosticCode::UnsupportedCapability`] instead of silently
//! degrading a declaration.
//!
//! Validated schemas can also resolve fingerprint-bound collection, field,
//! relation, and projection handles and decode selected backend rows into
//! owned records. See [`RuntimeProjection`](super::RuntimeProjection) and
//! [`RuntimeRecord`](super::RuntimeRecord). Query rendering and execution
//! remain outside this schema IR slice.
//!
//! # Validation and fingerprints
//!
//! [`RuntimeSchema::validate`] checks referential integrity, naming, and key compatibility and
//! returns all diagnostics rather than failing on the first. A [`ValidatedRuntimeSchema`]
//! provides:
//!
//! - [`ValidatedRuntimeSchema::canonical_bytes`] / [`ValidatedRuntimeSchema::fingerprint`]:
//!   deterministic canonical serialization including stable IDs, independent of declaration and
//!   map/insertion order. Two catalogs with identical shapes but different stable IDs are
//!   different schemas here.
//! - [`ValidatedRuntimeSchema::structural_fingerprint`]: the ID-free structural schema.
//!   Equivalent static-derived and catalog-loaded schemas must agree on this value even though
//!   static conversion synthesizes IDs. It is structural-only by design; see the method docs
//!   for exactly what it excludes.
//!
//! Fingerprints are FNV-1a drift detectors, not cryptographic signatures or authenticity proof.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::runtime_description::{classification_problem, semantic_token};
use super::{
    GraphqlSemanticClassification, GraphqlSemanticExport, RuntimeDefaultError,
    RuntimeDefaultProblem, RuntimeFieldFormat, RuntimeFormatProblem, validate_semantic_description,
};

use super::core::{
    BackupValueKind, ColumnBackupPolicy, DecimalDef, DeletePolicy, EntityMetadata, FieldMetadata,
    IndexMethod, RelationChangePropagation, SchemaPolicy, fnv1a64,
};

/// Maximum length accepted for physical identifiers; the portable bound across supported
/// backends (PostgreSQL truncates at 63 bytes).
pub const MAX_PHYSICAL_IDENTIFIER_LEN: usize = 63;

const MAX_STABLE_ID_LEN: usize = 128;

fn valid_stable_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STABLE_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '|' && c != '"')
}

macro_rules! stable_id {
    ($(#[$doc:meta])* $name:ident, $kind:literal) => {
        $(#[$doc])*
        #[derive(
            Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(try_from = "String")]
        pub struct $name(String);

        // Deserialization goes through `TryFrom`, so serialized catalog data cannot bypass
        // stable-ID validation. `validate` re-checks IDs anyway for defense in depth.
        impl TryFrom<String> for $name {
            type Error = RuntimeSchemaDiagnostic;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl $name {
            /// Wraps an owned stable ID, rejecting empty, oversized, or
            /// non-printable/reserved-character values.
            pub fn new(value: impl Into<String>) -> Result<Self, RuntimeSchemaDiagnostic> {
                let value = value.into();
                if valid_stable_id(&value) {
                    Ok(Self(value))
                } else {
                    Err(RuntimeSchemaDiagnostic::bare(
                        RuntimeSchemaDiagnosticCode::InvalidStableId,
                        format!(
                            concat!("invalid ", $kind, " stable ID `{}`"),
                            value.escape_debug()
                        ),
                    ))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

stable_id!(
    /// Stable logical ID of a runtime collection.
    CollectionId,
    "collection"
);
stable_id!(
    /// Stable logical ID of a runtime field.
    FieldId,
    "field"
);
stable_id!(
    /// Stable logical ID of a runtime relation.
    RelationId,
    "relation"
);
stable_id!(
    /// Stable logical ID of a runtime index.
    IndexId,
    "index"
);

/// Backend-neutral logical value kind of a runtime field.
///
/// Unlike [`BackupValueKind`], date-time is first-class: backends choose the storage
/// representation, and later slices generate date scalars/filters from it.
///
/// Unit kinds serialize as plain strings (`"integer"`); [`Self::Decimal`]
/// serializes as `{"decimal": {"precision": p, "scale": s}}`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeValueKind {
    Boolean,
    Integer,
    Float,
    String,
    Uuid,
    Json,
    Bytes,
    DateTime,
    /// Exact fixed-precision decimal with the portable static definition:
    /// a scaled `i64` on SQLite, `NUMERIC(p,s)` on PostgreSQL. Values are never
    /// rounded; see [`parse_runtime_decimal`](super::parse_runtime_decimal).
    Decimal(DecimalDef),
}

impl RuntimeValueKind {
    /// Canonical identity spelling: [`Self::as_str`], or `decimal(p,s)`.
    pub fn canonical_name(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Decimal(definition) => {
                format!("decimal({},{})", definition.precision(), definition.scale()).into()
            }
            kind => kind.as_str().into(),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::String => "string",
            Self::Uuid => "uuid",
            Self::Json => "json",
            Self::Bytes => "bytes",
            Self::DateTime => "datetime",
            Self::Decimal(_) => "decimal",
        }
    }
}

/// Backend-neutral default value of a runtime field.
///
/// Literal text follows the grammar of the field's value kind; build literals
/// with the typed constructors such as [`RuntimeDefault::json`] and inspect
/// them with [`RuntimeDefault::typed`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDefault {
    /// A literal in the canonical text grammar of the field's value kind.
    Literal(String),
    /// The transaction/statement timestamp at write time, rendered per backend.
    CurrentTimestamp,
}

/// Server-managed timestamp of a `DateTime` runtime field.
///
/// The database clock writes the field (the same expression and encoding as
/// [`RuntimeDefault::CurrentTimestamp`]); no create or update input may supply
/// it. See the runtime schema IR reference for the exact semantics.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAutoTimestamp {
    /// Set by every runtime insert and never changed afterwards.
    OnCreate,
    /// Set by every runtime insert and refreshed by every runtime update that
    /// writes the row.
    OnCreateAndUpdate,
}

impl RuntimeAutoTimestamp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnCreate => "on_create",
            Self::OnCreateAndUpdate => "on_create_and_update",
        }
    }

    /// Whether a runtime update of the row refreshes the field.
    pub const fn refreshes_on_update(self) -> bool {
        matches!(self, Self::OnCreateAndUpdate)
    }
}

/// Host-supplied actor stamp of a nullable `Uuid` or `String` runtime field,
/// the analogue of a "created by" auditing column.
///
/// The host supplies the acting principal through the mutation's write grant
/// ([`RuntimeWriteGrant::with_actor`](super::RuntimeWriteGrant::with_actor));
/// requests may supply the field only when the grant permits actor assignment
/// ([`RuntimeWriteGrant::with_actor_assignment`](super::RuntimeWriteGrant::with_actor_assignment)).
/// See the runtime schema IR reference for the exact semantics.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAutoActor {
    /// Written by every runtime insert from the grant's actor and never changed
    /// afterwards unless an assignment-permitted request supplies it.
    OnCreate,
}

impl RuntimeAutoActor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OnCreate => "on_create",
        }
    }
}

/// Sort direction for deterministic default ordering.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOrderDirection {
    Asc,
    Desc,
}

impl RuntimeOrderDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

/// One term of a collection's deterministic default ordering.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOrderTerm {
    pub field: FieldId,
    pub direction: RuntimeOrderDirection,
}

/// Owned runtime definition of one field.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeField {
    pub id: FieldId,
    /// Public GraphQL name; validated against GraphQL grammar and `__` reservations.
    pub api_name: String,
    /// Physical column identifier; validated as a portable lowercase identifier.
    pub physical_column: String,
    pub value_kind: RuntimeValueKind,
    pub nullable: bool,
    pub unique: bool,
    pub filterable: bool,
    pub sortable: bool,
    /// Server-generated; rejected as writable input by later slices.
    pub generated: bool,
    pub default: Option<RuntimeDefault>,
    /// Optional semantic descriptor. It never changes the physical shape and is
    /// omitted from serialization when absent, keeping earlier documents and
    /// identities byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<RuntimeFieldFormat>,
    /// Optional public description shared by GraphQL and the semantic catalogue;
    /// see [`RuntimeField::effective_description`]. Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional classification; inherits the collection's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<GraphqlSemanticClassification>,
    /// Optional export disposition; `exportable` unless secret when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<GraphqlSemanticExport>,
    /// Optional server-managed timestamp; valid only on non-key `DateTime`
    /// fields that are not generated and have no literal default. Omitted when
    /// absent, keeping earlier documents and identities byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_timestamp: Option<RuntimeAutoTimestamp>,
    /// Optional host-supplied actor stamp; valid only on nullable, non-key
    /// `Uuid` or `String` fields that are not generated, have no default and do
    /// not declare `auto_timestamp`. Omitted when absent, keeping earlier
    /// documents and identities byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_actor: Option<RuntimeAutoActor>,
}

impl RuntimeField {
    /// Whether clients may supply this field's value: neither generated, a
    /// server-managed timestamp nor a host-supplied actor stamp. An actor stamp
    /// is writable only under a grant that permits actor assignment.
    pub fn is_client_writable(&self) -> bool {
        !self.generated && self.auto_timestamp.is_none() && self.auto_actor.is_none()
    }

    /// The default the database applies: the declared default, or
    /// `CurrentTimestamp` for an automatic timestamp.
    pub fn effective_default(&self) -> Option<&RuntimeDefault> {
        match (&self.default, self.auto_timestamp) {
            (None, Some(_)) => Some(&RuntimeDefault::CurrentTimestamp),
            (default, _) => default.as_ref(),
        }
    }
}

/// Cardinality of a runtime relation from the declaring collection's perspective.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RelationCardinality {
    One,
    Many,
}

impl RelationCardinality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::One => "one",
            Self::Many => "many",
        }
    }
}

/// One ordered source/target key pair of a relation. Arity mismatches are unrepresentable:
/// a relation's keys are a list of pairs, never two independent lists.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationKeyPair {
    /// Field in the declaring collection.
    pub source: FieldId,
    /// Field in the target collection.
    pub target: FieldId,
}

/// Owned runtime definition of one relation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRelation {
    pub id: RelationId,
    /// Public GraphQL field name of the relation on the declaring type.
    pub api_name: String,
    pub target: CollectionId,
    /// Ordered key pairs; at least one is required.
    pub key_pairs: Vec<RelationKeyPair>,
    pub cardinality: RelationCardinality,
    /// Whether the declaring side emits a database foreign-key constraint.
    pub enforce_foreign_key: bool,
    /// Delete behavior; only meaningful on the foreign-key-enforcing side.
    pub on_delete: Option<DeletePolicy>,
    /// Optional public description shared by GraphQL and the semantic catalogue;
    /// see [`RuntimeRelation::effective_description`]. Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional classification; inherits the collection's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<GraphqlSemanticClassification>,
    /// Optional export disposition; `exportable` unless secret when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<GraphqlSemanticExport>,
    /// Many-to-many link table owned by this relation. A join relation has
    /// `Many` cardinality, no key pairs, no foreign key or delete behavior of its
    /// own; links are rows of the join table. Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<RuntimeJoinTable>,
}

/// ORM-owned link table of a many-to-many relation.
///
/// The table holds one row per link: the source collection's primary key in
/// `source` columns and the target collection's primary key in `target`
/// columns. Its primary key is the complete pair, so a link exists at most once.
/// Both sides reference their collection with `ON DELETE CASCADE`, so deleting
/// either record removes its links. Two relations may share one join table only
/// as exact inverses (source and target swapped).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeJoinTable {
    /// Physical link table identifier.
    pub physical_table: String,
    /// The source collection's primary-key fields, in key order, with their link columns.
    pub source: Vec<RuntimeJoinColumn>,
    /// The target collection's primary-key fields, in key order, with their link columns.
    pub target: Vec<RuntimeJoinColumn>,
}

/// One key member of a link table.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeJoinColumn {
    /// Primary-key field of the referenced collection.
    pub field: FieldId,
    /// Physical link-table column.
    pub column: String,
}

/// Owned runtime definition of one secondary index.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIndex {
    pub id: IndexId,
    /// Physical index name; validated as a portable identifier.
    pub name: String,
    /// Ordered indexed fields; at least one is required.
    pub fields: Vec<FieldId>,
    pub unique: bool,
}

/// Owned runtime definition of one collection.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCollection {
    pub id: CollectionId,
    /// Public GraphQL object type name.
    pub api_type_name: String,
    /// Public plural name used for list roots and connections.
    pub api_plural_name: String,
    /// Physical table identifier.
    pub physical_table: String,
    /// Ordered primary-key fields; at least one is required and none may be nullable.
    pub primary_key: Vec<FieldId>,
    /// Rows may be inserted but never updated or deleted.
    pub append_only: bool,
    /// Append-only enforcement admits the ORM's bounded retention capability.
    /// Missing serialized values default to `false` for format-v1 compatibility.
    #[serde(default)]
    pub retention_purge: bool,
    pub fields: Vec<RuntimeField>,
    pub relations: Vec<RuntimeRelation>,
    pub indexes: Vec<RuntimeIndex>,
    /// Groups of fields that must be unique together; each group needs two or more fields.
    pub composite_unique: Vec<Vec<FieldId>>,
    /// Deterministic default ordering.
    pub default_order: Vec<RuntimeOrderTerm>,
    /// Optional public description shared by GraphQL and the semantic catalogue;
    /// see [`RuntimeCollection::effective_description`]. Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional default classification of the collection's members; `internal`
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<GraphqlSemanticClassification>,
    /// Ordered fields whose values form one human-readable label per record
    /// (pickers, relation cells, model context). Presentation metadata: part of
    /// the ID-bearing fingerprint when non-empty, never of the physical schema or
    /// the structural fingerprint. At most [`RUNTIME_MAX_LABEL_FIELDS`] distinct
    /// exportable, non-secret scalar fields of this collection. Empty means no
    /// declared label; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_fields: Vec<FieldId>,
}

/// Maximum ordered label fields of one runtime collection.
pub const RUNTIME_MAX_LABEL_FIELDS: usize = 4;

/// The serialized IR format version this crate reads and writes.
pub const RUNTIME_SCHEMA_FORMAT_VERSION: u32 = 1;

fn current_format_version() -> u32 {
    RUNTIME_SCHEMA_FORMAT_VERSION
}

/// Owned runtime definition of a complete schema.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSchema {
    /// Serialized format version; see [`RUNTIME_SCHEMA_FORMAT_VERSION`]. Documents written
    /// before versioning existed deserialize as version 1. Unsupported versions are rejected
    /// by [`RuntimeSchema::validate`].
    #[serde(default = "current_format_version")]
    pub format_version: u32,
    pub collections: Vec<RuntimeCollection>,
}

impl Default for RuntimeSchema {
    fn default() -> Self {
        Self {
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: Vec::new(),
        }
    }
}

/// Machine-usable category of a runtime schema diagnostic.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSchemaDiagnosticCode {
    InvalidStableId,
    DuplicateStableId,
    InvalidApiName,
    ReservedApiName,
    DuplicateApiName,
    InvalidPhysicalName,
    DuplicatePhysicalName,
    EmptyPrimaryKey,
    UnknownFieldReference,
    NullablePrimaryKeyField,
    EmptyRelationKeys,
    UnknownRelationTarget,
    RelationKeyTypeMismatch,
    MissingDeleteBehavior,
    UnexpectedDeleteBehavior,
    SetNullOnNonNullableKey,
    EmptyIndexFields,
    CompositeUniqueGroupTooSmall,
    DuplicateKeyMember,
    RelationTargetKeyNotUnique,
    InvalidDefault,
    UnsupportedCapability,
    UnsupportedDefault,
    InvalidDefaultOrder,
    /// Retention was enabled for a collection that is not append-only.
    RetentionRequiresAppendOnly,
    /// A semantic field format descriptor is invalid.
    InvalidFormat,
    /// A public description is empty, untrimmed, longer than 1024 bytes or
    /// contains control or directional-formatting characters.
    InvalidDescription,
    /// A classification weakens its collection's, or a secret member is exportable.
    InvalidClassification,
    /// A label field is repeated, too numerous, or not an exportable, non-secret
    /// scalar (JSON and bytes are ineligible).
    InvalidLabelField,
    /// A many-to-many join table is malformed, collides with another physical
    /// name, or is shared by relations that are not exact inverses.
    InvalidJoinTable,
    /// An automatic timestamp is declared on a field that is not a `DateTime`,
    /// is generated, has a literal default, or is a primary or relation key.
    InvalidAutoTimestamp,
    /// An actor stamp is declared on a field that is not a `Uuid` or `String`,
    /// is not nullable, is generated, has a default, is a primary-key member, or
    /// also declares an automatic timestamp.
    InvalidAutoActor,
}

impl RuntimeSchemaDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidStableId => "invalid_stable_id",
            Self::DuplicateStableId => "duplicate_stable_id",
            Self::InvalidApiName => "invalid_api_name",
            Self::ReservedApiName => "reserved_api_name",
            Self::DuplicateApiName => "duplicate_api_name",
            Self::InvalidPhysicalName => "invalid_physical_name",
            Self::DuplicatePhysicalName => "duplicate_physical_name",
            Self::EmptyPrimaryKey => "empty_primary_key",
            Self::UnknownFieldReference => "unknown_field_reference",
            Self::NullablePrimaryKeyField => "nullable_primary_key_field",
            Self::EmptyRelationKeys => "empty_relation_keys",
            Self::UnknownRelationTarget => "unknown_relation_target",
            Self::RelationKeyTypeMismatch => "relation_key_type_mismatch",
            Self::MissingDeleteBehavior => "missing_delete_behavior",
            Self::UnexpectedDeleteBehavior => "unexpected_delete_behavior",
            Self::SetNullOnNonNullableKey => "set_null_on_non_nullable_key",
            Self::EmptyIndexFields => "empty_index_fields",
            Self::CompositeUniqueGroupTooSmall => "composite_unique_group_too_small",
            Self::DuplicateKeyMember => "duplicate_key_member",
            Self::RelationTargetKeyNotUnique => "relation_target_key_not_unique",
            Self::InvalidDefault => "invalid_default",
            Self::RetentionRequiresAppendOnly => "retention_requires_append_only",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::UnsupportedDefault => "unsupported_default",
            Self::InvalidDefaultOrder => "invalid_default_order",
            Self::InvalidFormat => "invalid_format",
            Self::InvalidDescription => "invalid_description",
            Self::InvalidClassification => "invalid_classification",
            Self::InvalidLabelField => "invalid_label_field",
            Self::InvalidJoinTable => "invalid_join_table",
            Self::InvalidAutoTimestamp => "invalid_auto_timestamp",
            Self::InvalidAutoActor => "invalid_auto_actor",
        }
    }
}

/// Value-free detail refining a diagnostic code.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeSchemaDiagnosticDetail {
    /// Why a literal default was rejected.
    Default(RuntimeDefaultProblem),
    /// Why a format descriptor was rejected.
    Format(RuntimeFormatProblem),
    /// Why an automatic timestamp was rejected.
    AutoTimestamp(RuntimeAutoTimestampProblem),
    /// Why an actor stamp was rejected.
    AutoActor(RuntimeAutoActorProblem),
}

/// Value-free reason an actor stamp declaration is invalid.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeAutoActorProblem {
    /// The field's value kind is neither `Uuid` nor `String`.
    UnsupportedKind,
    /// The field is not nullable.
    NotNullable,
    /// The field is also `generated`.
    Generated,
    /// The field declares a default.
    HasDefault,
    /// The field is a primary-key member.
    KeyField,
    /// The field also declares an automatic timestamp.
    AutoTimestamp,
}

impl RuntimeAutoActorProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedKind => "unsupported_kind",
            Self::NotNullable => "not_nullable",
            Self::Generated => "generated",
            Self::HasDefault => "has_default",
            Self::KeyField => "key_field",
            Self::AutoTimestamp => "auto_timestamp",
        }
    }
}

/// Value-free reason an automatic timestamp declaration is invalid.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeAutoTimestampProblem {
    /// The field's value kind is not `DateTime`.
    NotDateTime,
    /// The field is also `generated`.
    Generated,
    /// The field declares a literal default.
    LiteralDefault,
    /// The field is a primary-key member or a relation key.
    KeyField,
}

impl RuntimeAutoTimestampProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotDateTime => "not_date_time",
            Self::Generated => "generated",
            Self::LiteralDefault => "literal_default",
            Self::KeyField => "key_field",
        }
    }
}

/// One structured validation or conversion diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeSchemaDiagnostic {
    pub code: RuntimeSchemaDiagnosticCode,
    pub message: String,
    /// Collection context when the diagnostic is scoped to one collection.
    pub collection: Option<CollectionId>,
    /// Field/relation/index/subject context inside the collection, when applicable.
    pub subject: Option<String>,
    /// Value-free refinement of `code`; omitted from serialization when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<RuntimeSchemaDiagnosticDetail>,
}

impl RuntimeSchemaDiagnostic {
    fn bare(code: RuntimeSchemaDiagnosticCode, message: String) -> Self {
        Self {
            code,
            message,
            collection: None,
            subject: None,
            detail: None,
        }
    }

    fn scoped(
        code: RuntimeSchemaDiagnosticCode,
        message: String,
        collection: &CollectionId,
        subject: Option<&str>,
    ) -> Self {
        Self {
            code,
            message,
            collection: Some(collection.clone()),
            subject: subject.map(str::to_string),
            detail: None,
        }
    }

    fn with_detail(mut self, detail: RuntimeSchemaDiagnosticDetail) -> Self {
        self.detail = Some(detail);
        self
    }
}

impl fmt::Display for RuntimeSchemaDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.code.as_str())?;
        if let Some(collection) = &self.collection {
            write!(f, " collection `{collection}`")?;
        }
        if let Some(subject) = &self.subject {
            write!(f, " `{subject}`")?;
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for RuntimeSchemaDiagnostic {}

/// All diagnostics from a failed validation or conversion; never empty.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeSchemaDiagnostics(Vec<RuntimeSchemaDiagnostic>);

impl RuntimeSchemaDiagnostics {
    pub fn diagnostics(&self) -> &[RuntimeSchemaDiagnostic] {
        &self.0
    }

    pub fn into_diagnostics(self) -> Vec<RuntimeSchemaDiagnostic> {
        self.0
    }
}

impl fmt::Display for RuntimeSchemaDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} runtime schema diagnostic(s):", self.0.len())?;
        for diagnostic in &self.0 {
            writeln!(f, "  {diagnostic}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RuntimeSchemaDiagnostics {}

impl From<RuntimeSchemaDiagnostic> for RuntimeSchemaDiagnostics {
    fn from(diagnostic: RuntimeSchemaDiagnostic) -> Self {
        Self(vec![diagnostic])
    }
}

/// Stable 64-bit fingerprint rendered as 16 lowercase hex characters.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String")]
pub struct SchemaFingerprint(String);

// Arbitrary strings cannot masquerade as fingerprints: deserialization requires exactly
// 16 lowercase hexadecimal characters.
impl TryFrom<String> for SchemaFingerprint {
    type Error = RuntimeSchemaDiagnostic;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value.len() == 16
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if valid {
            Ok(Self(value))
        } else {
            Err(RuntimeSchemaDiagnostic::bare(
                RuntimeSchemaDiagnosticCode::InvalidStableId,
                format!(
                    "invalid schema fingerprint `{}`: expected 16 lowercase hex characters",
                    value.escape_debug()
                ),
            ))
        }
    }
}

impl SchemaFingerprint {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self(format!("{:016x}", fnv1a64(bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchemaFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_physical_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    name.len() <= MAX_PHYSICAL_IDENTIFIER_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks a field's default against its value kind. The returned problem never carries the
/// literal itself, so diagnostics cannot echo submitted values.
fn invalid_default(field: &RuntimeField) -> Option<RuntimeDefaultProblem> {
    field
        .default
        .as_ref()
        .and_then(|default| default.typed(field.value_kind).err())
        .map(RuntimeDefaultError::problem)
}

/// True when the referenced target fields are provably unique on the target collection.
fn target_key_is_unique(target: &RuntimeCollection, referenced: &BTreeSet<&FieldId>) -> bool {
    let primary: BTreeSet<&FieldId> = target.primary_key.iter().collect();
    if primary == *referenced {
        return true;
    }
    if referenced.len() == 1 {
        let field_id = referenced.iter().next().expect("non-empty set");
        if target
            .fields
            .iter()
            .any(|field| &&field.id == field_id && field.unique)
        {
            return true;
        }
    }
    if target
        .indexes
        .iter()
        .any(|index| index.unique && index.fields.iter().collect::<BTreeSet<_>>() == *referenced)
    {
        return true;
    }
    target
        .composite_unique
        .iter()
        .any(|group| group.iter().collect::<BTreeSet<_>>() == *referenced)
}

fn invalid_description(
    collection: &CollectionId,
    subject: Option<&str>,
) -> RuntimeSchemaDiagnostic {
    RuntimeSchemaDiagnostic::scoped(
        RuntimeSchemaDiagnosticCode::InvalidDescription,
        "description must be trimmed, non-empty, at most 1024 bytes and free of control or \
         directional-formatting characters"
            .to_string(),
        collection,
        subject,
    )
}

/// Value-free description and classification diagnostics of one field or relation.
fn semantic_member_diagnostics(
    collection: &RuntimeCollection,
    subject: Option<&str>,
    description: Option<&str>,
    classification: GraphqlSemanticClassification,
    export: Option<GraphqlSemanticExport>,
) -> Vec<RuntimeSchemaDiagnostic> {
    let mut diagnostics = Vec::new();
    if description.is_some_and(|text| validate_semantic_description(text).is_err()) {
        diagnostics.push(invalid_description(&collection.id, subject));
    }
    if let Some(problem) = classification_problem(collection, classification, export) {
        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
            RuntimeSchemaDiagnosticCode::InvalidClassification,
            problem.to_string(),
            &collection.id,
            subject,
        ));
    }
    diagnostics
}

/// Semantic members join the ID-bearing identity only when present.
fn push_semantic_members(
    out: &mut String,
    description: Option<&str>,
    classification: Option<GraphqlSemanticClassification>,
    export: Option<GraphqlSemanticExport>,
) {
    if let Some(description) = description {
        out.push_str("|description=");
        out.push_str(&serde_json::to_string(description).expect("strings always serialize"));
    }
    if let Some(classification) = classification {
        out.push_str("|classification=");
        out.push_str(&semantic_token(&classification));
    }
    if let Some(export) = export {
        out.push_str("|export=");
        out.push_str(&semantic_token(&export));
    }
}

impl RuntimeSchema {
    /// Validates the schema, returning every diagnostic found rather than the first.
    ///
    /// A valid schema yields a [`ValidatedRuntimeSchema`], the only type that can produce
    /// canonical bytes and fingerprints.
    pub fn validate(self) -> Result<ValidatedRuntimeSchema, RuntimeSchemaDiagnostics> {
        let mut diagnostics = Vec::new();

        if self.format_version != RUNTIME_SCHEMA_FORMAT_VERSION {
            diagnostics.push(RuntimeSchemaDiagnostic::bare(
                RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                format!(
                    "unsupported runtime schema format version {} (this crate supports {})",
                    self.format_version, RUNTIME_SCHEMA_FORMAT_VERSION
                ),
            ));
        }

        // Cross-collection uniqueness and field-type lookup tables. Stable IDs are re-checked
        // here even though constructors validate them, so no in-memory mutation or alternate
        // construction path can smuggle an invalid ID past validation.
        let mut collection_ids = BTreeSet::new();
        let mut api_type_names = BTreeSet::new();
        let mut api_plural_names = BTreeSet::new();
        let mut physical_tables = BTreeSet::new();
        let mut index_names = BTreeSet::new();
        // Field, relation, and index stable IDs are unique across the whole schema, not just
        // within their collection: catalogs address them without collection context.
        let mut global_field_ids = BTreeSet::new();
        let mut global_relation_ids = BTreeSet::new();
        let mut global_index_ids = BTreeSet::new();
        let mut field_kinds: BTreeMap<&CollectionId, BTreeMap<&FieldId, &RuntimeField>> =
            BTreeMap::new();
        let collections_by_id: BTreeMap<&CollectionId, &RuntimeCollection> =
            self.collections.iter().map(|c| (&c.id, c)).collect();

        let check_id = |id: &str,
                        kind: &str,
                        collection: Option<&CollectionId>,
                        diagnostics: &mut Vec<RuntimeSchemaDiagnostic>| {
            if !valid_stable_id(id) {
                diagnostics.push(RuntimeSchemaDiagnostic {
                    code: RuntimeSchemaDiagnosticCode::InvalidStableId,
                    message: format!("invalid {kind} stable ID `{}`", id.escape_debug()),
                    collection: collection.cloned(),
                    subject: Some(id.to_string()),
                    detail: None,
                });
            }
        };

        for collection in &self.collections {
            check_id(collection.id.as_str(), "collection", None, &mut diagnostics);
            if !collection_ids.insert(&collection.id) {
                diagnostics.push(RuntimeSchemaDiagnostic::bare(
                    RuntimeSchemaDiagnosticCode::DuplicateStableId,
                    format!("duplicate collection ID `{}`", collection.id),
                ));
            }
            let mut fields = BTreeMap::new();
            for field in &collection.fields {
                check_id(
                    field.id.as_str(),
                    "field",
                    Some(&collection.id),
                    &mut diagnostics,
                );
                if fields.insert(&field.id, field).is_some() || !global_field_ids.insert(&field.id)
                {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicateStableId,
                        format!("duplicate field ID `{}`", field.id),
                        &collection.id,
                        Some(field.id.as_str()),
                    ));
                }
            }
            for relation in &collection.relations {
                check_id(
                    relation.id.as_str(),
                    "relation",
                    Some(&collection.id),
                    &mut diagnostics,
                );
                if !global_relation_ids.insert(&relation.id) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicateStableId,
                        format!("duplicate relation ID `{}`", relation.id),
                        &collection.id,
                        Some(relation.id.as_str()),
                    ));
                }
            }
            for index in &collection.indexes {
                check_id(
                    index.id.as_str(),
                    "index",
                    Some(&collection.id),
                    &mut diagnostics,
                );
                if !global_index_ids.insert(&index.id) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicateStableId,
                        format!("duplicate index ID `{}`", index.id),
                        &collection.id,
                        Some(index.id.as_str()),
                    ));
                }
            }
            field_kinds.insert(&collection.id, fields);
        }

        for collection in &self.collections {
            let cid = &collection.id;
            let own_fields = &field_kinds[cid];

            for (name, kind_label, reserved) in [
                (&collection.api_type_name, "api_type_name", true),
                (&collection.api_plural_name, "api_plural_name", true),
            ] {
                if !valid_graphql_name(name) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::InvalidApiName,
                        format!(
                            "{kind_label} `{}` is not a valid GraphQL name",
                            name.escape_debug()
                        ),
                        cid,
                        None,
                    ));
                } else if reserved && name.starts_with("__") {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::ReservedApiName,
                        format!("{kind_label} `{name}` uses the reserved `__` prefix"),
                        cid,
                        None,
                    ));
                }
            }
            if !api_type_names.insert(collection.api_type_name.to_ascii_lowercase()) {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::DuplicateApiName,
                    format!(
                        "api_type_name `{}` collides case-insensitively with another collection",
                        collection.api_type_name
                    ),
                    cid,
                    None,
                ));
            }
            if !api_plural_names.insert(collection.api_plural_name.to_ascii_lowercase()) {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::DuplicateApiName,
                    format!(
                        "api_plural_name `{}` collides case-insensitively with another collection",
                        collection.api_plural_name
                    ),
                    cid,
                    None,
                ));
            }
            if collection.retention_purge && !collection.append_only {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::RetentionRequiresAppendOnly,
                    "retention_purge requires append_only collection semantics".to_string(),
                    cid,
                    None,
                ));
            }
            if collection
                .description
                .as_deref()
                .is_some_and(|text| validate_semantic_description(text).is_err())
            {
                diagnostics.push(invalid_description(cid, None));
            }
            if !valid_physical_name(&collection.physical_table) {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::InvalidPhysicalName,
                    format!(
                        "physical_table `{}` is not a portable identifier",
                        collection.physical_table.escape_debug()
                    ),
                    cid,
                    None,
                ));
            } else if !physical_tables.insert(collection.physical_table.clone()) {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::DuplicatePhysicalName,
                    format!(
                        "physical_table `{}` is used by another collection",
                        collection.physical_table
                    ),
                    cid,
                    None,
                ));
            }

            // Fields.
            let mut field_api_names = BTreeSet::new();
            let mut field_physical_names = BTreeSet::new();
            // Primary-key members and relation keys on either side; automatic
            // timestamps may not be any of them.
            let key_fields: BTreeSet<&FieldId> = collection
                .primary_key
                .iter()
                .chain(
                    collection
                        .relations
                        .iter()
                        .flat_map(|r| r.key_pairs.iter().map(|pair| &pair.source)),
                )
                .chain(
                    self.collections
                        .iter()
                        .flat_map(|c| &c.relations)
                        .filter(|r| &r.target == cid)
                        .flat_map(|r| r.key_pairs.iter().map(|pair| &pair.target)),
                )
                .collect();
            for field in &collection.fields {
                let subject = Some(field.id.as_str());
                if !valid_graphql_name(&field.api_name) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::InvalidApiName,
                        format!(
                            "field api_name `{}` is not a valid GraphQL name",
                            field.api_name.escape_debug()
                        ),
                        cid,
                        subject,
                    ));
                } else if field.api_name.starts_with("__") {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::ReservedApiName,
                        format!(
                            "field api_name `{}` uses the reserved `__` prefix",
                            field.api_name
                        ),
                        cid,
                        subject,
                    ));
                }
                if !field_api_names.insert(field.api_name.to_ascii_lowercase()) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicateApiName,
                        format!(
                            "field api_name `{}` collides case-insensitively within the collection",
                            field.api_name
                        ),
                        cid,
                        subject,
                    ));
                }
                if !valid_physical_name(&field.physical_column) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::InvalidPhysicalName,
                        format!(
                            "physical_column `{}` is not a portable identifier",
                            field.physical_column.escape_debug()
                        ),
                        cid,
                        subject,
                    ));
                } else if !field_physical_names.insert(field.physical_column.clone()) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicatePhysicalName,
                        format!(
                            "physical_column `{}` is used by another field in the collection",
                            field.physical_column
                        ),
                        cid,
                        subject,
                    ));
                }
                if let Some(problem) = invalid_default(field) {
                    let message = match &field.default {
                        Some(RuntimeDefault::CurrentTimestamp) => format!(
                            "current_timestamp default is not valid for a {} field",
                            field.value_kind.as_str()
                        ),
                        _ => format!(
                            "literal default is not a valid {} value ({problem})",
                            field.value_kind.as_str()
                        ),
                    };
                    diagnostics.push(
                        RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::InvalidDefault,
                            message,
                            cid,
                            subject,
                        )
                        .with_detail(RuntimeSchemaDiagnosticDetail::Default(problem)),
                    );
                }
                if let Some(Err(error)) = field.format.as_ref().map(RuntimeFieldFormat::validate) {
                    diagnostics.push(
                        RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::InvalidFormat,
                            format!("field format descriptor is invalid ({})", error.problem()),
                            cid,
                            subject,
                        )
                        .with_detail(RuntimeSchemaDiagnosticDetail::Format(error.problem())),
                    );
                }
                diagnostics.extend(semantic_member_diagnostics(
                    collection,
                    subject,
                    field.description.as_deref(),
                    field.effective_classification(collection),
                    field.export,
                ));
                if field.auto_timestamp.is_some() {
                    let problem = if field.value_kind != RuntimeValueKind::DateTime {
                        Some(RuntimeAutoTimestampProblem::NotDateTime)
                    } else if field.generated {
                        Some(RuntimeAutoTimestampProblem::Generated)
                    } else if matches!(field.default, Some(RuntimeDefault::Literal(_))) {
                        Some(RuntimeAutoTimestampProblem::LiteralDefault)
                    } else if key_fields.contains(&field.id) {
                        Some(RuntimeAutoTimestampProblem::KeyField)
                    } else {
                        None
                    };
                    if let Some(problem) = problem {
                        diagnostics.push(
                            RuntimeSchemaDiagnostic::scoped(
                                RuntimeSchemaDiagnosticCode::InvalidAutoTimestamp,
                                format!("automatic timestamp is invalid ({})", problem.as_str()),
                                cid,
                                subject,
                            )
                            .with_detail(RuntimeSchemaDiagnosticDetail::AutoTimestamp(problem)),
                        );
                    }
                }
                if field.auto_actor.is_some() {
                    // One problem per field, the most specific first: primary-key
                    // members are never nullable, so the key check precedes nullability.
                    let problem = if field.auto_timestamp.is_some() {
                        Some(RuntimeAutoActorProblem::AutoTimestamp)
                    } else if !matches!(
                        field.value_kind,
                        RuntimeValueKind::Uuid | RuntimeValueKind::String
                    ) {
                        Some(RuntimeAutoActorProblem::UnsupportedKind)
                    } else if collection.primary_key.contains(&field.id) {
                        Some(RuntimeAutoActorProblem::KeyField)
                    } else if !field.nullable {
                        Some(RuntimeAutoActorProblem::NotNullable)
                    } else if field.generated {
                        Some(RuntimeAutoActorProblem::Generated)
                    } else if field.default.is_some() {
                        Some(RuntimeAutoActorProblem::HasDefault)
                    } else {
                        None
                    };
                    if let Some(problem) = problem {
                        diagnostics.push(
                            RuntimeSchemaDiagnostic::scoped(
                                RuntimeSchemaDiagnosticCode::InvalidAutoActor,
                                format!("actor stamp is invalid ({})", problem.as_str()),
                                cid,
                                subject,
                            )
                            .with_detail(RuntimeSchemaDiagnosticDetail::AutoActor(problem)),
                        );
                    }
                }
                if field.generated && matches!(field.value_kind, RuntimeValueKind::Decimal(_)) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        "decimal fields cannot be generated".to_string(),
                        cid,
                        subject,
                    ));
                }
                // Runtime decimals are portable (precision 1..=18). The SQL
                // Server MONEY domain is static-only: it has no runtime DDL,
                // storage or decoder, and must not reach SQLite or PostgreSQL.
                if let RuntimeValueKind::Decimal(definition) = field.value_kind
                    && definition.is_mssql_money()
                {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        "SQL Server MONEY is not a runtime decimal definition".to_string(),
                        cid,
                        subject,
                    ));
                }
            }

            // Primary key.
            if collection.primary_key.is_empty() {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::EmptyPrimaryKey,
                    "collection has no primary-key fields".to_string(),
                    cid,
                    None,
                ));
            }
            let mut primary_key_members = BTreeSet::new();
            for key_field in &collection.primary_key {
                if !primary_key_members.insert(key_field) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicateKeyMember,
                        format!("primary_key lists field `{key_field}` more than once"),
                        cid,
                        Some(key_field.as_str()),
                    ));
                }
                match own_fields.get(key_field) {
                    None => diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                        format!("primary_key references unknown field `{key_field}`"),
                        cid,
                        Some(key_field.as_str()),
                    )),
                    Some(field) if field.nullable => {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::NullablePrimaryKeyField,
                            format!("primary_key field `{key_field}` must not be nullable"),
                            cid,
                            Some(key_field.as_str()),
                        ));
                    }
                    Some(field) if matches!(field.value_kind, RuntimeValueKind::Decimal(_)) => {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                            format!("primary_key field `{key_field}` cannot be a decimal"),
                            cid,
                            Some(key_field.as_str()),
                        ));
                    }
                    Some(_) => {}
                }
            }

            // Indexes.
            for index in &collection.indexes {
                let subject = Some(index.id.as_str());
                if !valid_physical_name(&index.name) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::InvalidPhysicalName,
                        format!(
                            "index name `{}` is not a portable identifier",
                            index.name.escape_debug()
                        ),
                        cid,
                        subject,
                    ));
                } else if !index_names.insert(index.name.clone()) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicatePhysicalName,
                        format!(
                            "index name `{}` is used elsewhere in the schema",
                            index.name
                        ),
                        cid,
                        subject,
                    ));
                }
                if index.fields.is_empty() {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::EmptyIndexFields,
                        "index has no fields".to_string(),
                        cid,
                        subject,
                    ));
                }
                let mut index_members = BTreeSet::new();
                for field in &index.fields {
                    if !index_members.insert(field) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::DuplicateKeyMember,
                            format!("index lists field `{field}` more than once"),
                            cid,
                            subject,
                        ));
                    }
                    if !own_fields.contains_key(field) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                            format!("index references unknown field `{field}`"),
                            cid,
                            subject,
                        ));
                    }
                }
            }

            // Composite unique groups.
            for group in &collection.composite_unique {
                if group.len() < 2 {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::CompositeUniqueGroupTooSmall,
                        "composite_unique groups need two or more fields; use a unique field instead"
                            .to_string(),
                        cid,
                        None,
                    ));
                }
                let mut group_members = BTreeSet::new();
                for field in group {
                    if !group_members.insert(field) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::DuplicateKeyMember,
                            format!("composite_unique lists field `{field}` more than once"),
                            cid,
                            Some(field.as_str()),
                        ));
                    }
                    if !own_fields.contains_key(field) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                            format!("composite_unique references unknown field `{field}`"),
                            cid,
                            Some(field.as_str()),
                        ));
                    }
                }
            }

            // Relations.
            let mut relation_api_names = BTreeSet::new();
            for relation in &collection.relations {
                let subject = Some(relation.id.as_str());
                diagnostics.extend(semantic_member_diagnostics(
                    collection,
                    subject,
                    relation.description.as_deref(),
                    relation.effective_classification(collection),
                    relation.export,
                ));
                if !valid_graphql_name(&relation.api_name) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::InvalidApiName,
                        format!(
                            "relation api_name `{}` is not a valid GraphQL name",
                            relation.api_name.escape_debug()
                        ),
                        cid,
                        subject,
                    ));
                } else if relation.api_name.starts_with("__") {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::ReservedApiName,
                        format!(
                            "relation api_name `{}` uses the reserved `__` prefix",
                            relation.api_name
                        ),
                        cid,
                        subject,
                    ));
                }
                if !relation_api_names.insert(relation.api_name.to_ascii_lowercase())
                    || field_kinds[cid]
                        .values()
                        .any(|f| f.api_name.eq_ignore_ascii_case(&relation.api_name))
                {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::DuplicateApiName,
                        format!(
                            "relation api_name `{}` collides case-insensitively within the collection",
                            relation.api_name
                        ),
                        cid,
                        subject,
                    ));
                }
                // Join relations are validated schema-wide after every collection.
                if relation.join.is_some() {
                    continue;
                }
                if relation.key_pairs.is_empty() {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::EmptyRelationKeys,
                        "relation has no key pairs".to_string(),
                        cid,
                        subject,
                    ));
                }
                let Some(target_fields) = field_kinds.get(&relation.target) else {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnknownRelationTarget,
                        format!("relation targets unknown collection `{}`", relation.target),
                        cid,
                        subject,
                    ));
                    continue;
                };
                for pair in &relation.key_pairs {
                    let source = own_fields.get(&pair.source);
                    let target = target_fields.get(&pair.target);
                    if source.is_none() {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                            format!(
                                "relation key references unknown source field `{}`",
                                pair.source
                            ),
                            cid,
                            subject,
                        ));
                    }
                    if target.is_none() {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                            format!(
                                "relation key references unknown target field `{}`",
                                pair.target
                            ),
                            cid,
                            subject,
                        ));
                    }
                    if let (Some(source), Some(target)) = (source, target) {
                        if [source, target]
                            .iter()
                            .any(|field| matches!(field.value_kind, RuntimeValueKind::Decimal(_)))
                        {
                            diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                                RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                                format!(
                                    "key pair `{}`/`{}` uses a decimal field; decimals cannot be \
                                     relation keys",
                                    pair.source, pair.target
                                ),
                                cid,
                                subject,
                            ));
                        }
                        if source.value_kind != target.value_kind {
                            diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                                RuntimeSchemaDiagnosticCode::RelationKeyTypeMismatch,
                                format!(
                                    "key pair `{}`/`{}` mixes {} and {}",
                                    pair.source,
                                    pair.target,
                                    source.value_kind.as_str(),
                                    target.value_kind.as_str()
                                ),
                                cid,
                                subject,
                            ));
                        }
                        if relation.enforce_foreign_key
                            && relation.on_delete == Some(DeletePolicy::SetNull)
                            && !source.nullable
                        {
                            diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                                RuntimeSchemaDiagnosticCode::SetNullOnNonNullableKey,
                                format!(
                                    "set-null delete behavior requires nullable source field `{}`",
                                    pair.source
                                ),
                                cid,
                                subject,
                            ));
                        }
                    }
                }
                // A relation may not reuse a source or target field across key pairs.
                let mut source_members = BTreeSet::new();
                let mut target_members = BTreeSet::new();
                for pair in &relation.key_pairs {
                    if !source_members.insert(&pair.source) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::DuplicateKeyMember,
                            format!(
                                "relation key pairs list source field `{}` more than once",
                                pair.source
                            ),
                            cid,
                            subject,
                        ));
                    }
                    if !target_members.insert(&pair.target) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::DuplicateKeyMember,
                            format!(
                                "relation key pairs list target field `{}` more than once",
                                pair.target
                            ),
                            cid,
                            subject,
                        ));
                    }
                }
                match (relation.enforce_foreign_key, relation.on_delete.as_ref()) {
                    (true, None) => diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::MissingDeleteBehavior,
                        "foreign-key-enforcing relation must state delete behavior".to_string(),
                        cid,
                        subject,
                    )),
                    (false, Some(_)) => diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnexpectedDeleteBehavior,
                        "delete behavior belongs on the foreign-key-enforcing side".to_string(),
                        cid,
                        subject,
                    )),
                    _ => {}
                }
                // A database foreign key requires the referenced key to be provably unique on
                // the target: its primary key, a unique field, a unique index, or a
                // composite-unique group (compared as sets; ordering does not affect
                // uniqueness).
                if relation.enforce_foreign_key
                    && !relation.key_pairs.is_empty()
                    && let Some(target_collection) = collections_by_id.get(&relation.target)
                {
                    let referenced: BTreeSet<&FieldId> =
                        relation.key_pairs.iter().map(|pair| &pair.target).collect();
                    if !target_key_is_unique(target_collection, &referenced) {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::RelationTargetKeyNotUnique,
                            format!(
                                "foreign key references target fields that are not the primary \
                                 key, a unique field, a unique index, or a composite-unique \
                                 group of `{}`",
                                relation.target
                            ),
                            cid,
                            subject,
                        ));
                    }
                }
            }

            // Label fields.
            if collection.label_fields.len() > RUNTIME_MAX_LABEL_FIELDS {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::InvalidLabelField,
                    format!("label_fields lists more than {RUNTIME_MAX_LABEL_FIELDS} fields"),
                    cid,
                    None,
                ));
            }
            let mut label_members = BTreeSet::new();
            for label in &collection.label_fields {
                let subject = Some(label.as_str());
                match own_fields.get(label) {
                    None => diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                        format!("label_fields references unknown field `{label}`"),
                        cid,
                        subject,
                    )),
                    Some(field)
                        if matches!(
                            field.value_kind,
                            RuntimeValueKind::Json | RuntimeValueKind::Bytes
                        ) || field.effective_classification(collection)
                            == GraphqlSemanticClassification::Secret
                            || field.effective_export(collection)
                                != GraphqlSemanticExport::Exportable =>
                    {
                        diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                            RuntimeSchemaDiagnosticCode::InvalidLabelField,
                            format!(
                                "label field `{label}` must be an exportable, non-secret scalar"
                            ),
                            cid,
                            subject,
                        ));
                    }
                    Some(_) => {}
                }
                if !label_members.insert(label) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::InvalidLabelField,
                        format!("label_fields lists field `{label}` more than once"),
                        cid,
                        subject,
                    ));
                }
            }

            // Default ordering.
            for term in &collection.default_order {
                if !own_fields.contains_key(&term.field) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnknownFieldReference,
                        format!("default_order references unknown field `{}`", term.field),
                        cid,
                        Some(term.field.as_str()),
                    ));
                }
            }
        }

        validate_join_tables(&self, &physical_tables, &mut diagnostics);

        if diagnostics.is_empty() {
            Ok(ValidatedRuntimeSchema { schema: self })
        } else {
            Err(RuntimeSchemaDiagnostics(diagnostics))
        }
    }
}

/// Relations with a join table: shape, key coverage, names and sharing.
fn validate_join_tables(
    schema: &RuntimeSchema,
    collection_tables: &BTreeSet<String>,
    diagnostics: &mut Vec<RuntimeSchemaDiagnostic>,
) {
    let by_id = schema
        .collections
        .iter()
        .map(|c| (&c.id, c))
        .collect::<BTreeMap<_, _>>();
    let index_names = schema
        .collections
        .iter()
        .flat_map(|c| c.indexes.iter().map(|i| i.name.to_ascii_lowercase()))
        .collect::<BTreeSet<_>>();
    // table -> (source collection, relation) declarations sharing it.
    let mut tables = BTreeMap::<String, Vec<(&RuntimeCollection, &RuntimeRelation)>>::new();
    for collection in &schema.collections {
        for relation in &collection.relations {
            let Some(join) = &relation.join else {
                continue;
            };
            let mut problem = |message: &str| {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::InvalidJoinTable,
                    message.to_string(),
                    &collection.id,
                    Some(relation.id.as_str()),
                ));
            };
            if relation.cardinality != RelationCardinality::Many
                || !relation.key_pairs.is_empty()
                || relation.enforce_foreign_key
                || relation.on_delete.is_some()
            {
                problem(
                    "a join relation has many cardinality, no key pairs and no foreign key or delete behavior of its own",
                );
            }
            let Some(target) = by_id.get(&relation.target) else {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::UnknownRelationTarget,
                    format!("relation targets unknown collection `{}`", relation.target),
                    &collection.id,
                    Some(relation.id.as_str()),
                ));
                continue;
            };
            let covers = |side: &[RuntimeJoinColumn], owner: &RuntimeCollection| {
                side.len() == owner.primary_key.len()
                    && side
                        .iter()
                        .zip(&owner.primary_key)
                        .all(|(column, key)| &column.field == key)
            };
            if !covers(&join.source, collection) || !covers(&join.target, target) {
                problem("join columns must list the source and target primary keys in key order");
            }
            let mut columns = BTreeSet::new();
            if !valid_physical_name(&join.physical_table)
                || join.physical_table.starts_with("__graphql_orm_")
                || join.source.iter().chain(&join.target).any(|c| {
                    !valid_physical_name(&c.column)
                        || !columns.insert(c.column.to_ascii_lowercase())
                })
            {
                problem("join table and column names must be distinct portable identifiers");
            }
            // The planner also names the link table's target index and PostgreSQL
            // names its primary key `<table>_pkey`; neither may collide.
            let target_columns = join
                .target
                .iter()
                .map(|c| c.column.as_str())
                .collect::<Vec<_>>();
            let generated = [
                super::core::generated_index_name(&join.physical_table, &target_columns),
                format!("{}_pkey", join.physical_table),
            ];
            if collection_tables
                .iter()
                .any(|table| table.eq_ignore_ascii_case(&join.physical_table))
                || index_names.contains(&join.physical_table.to_ascii_lowercase())
                || generated.iter().any(|name| {
                    index_names.contains(&name.to_ascii_lowercase())
                        || collection_tables
                            .iter()
                            .any(|t| t.eq_ignore_ascii_case(name))
                })
            {
                problem(
                    "join table name or its generated index names collide with a collection table or index",
                );
            }
            tables
                .entry(join.physical_table.to_ascii_lowercase())
                .or_default()
                .push((collection, relation));
        }
    }
    for declarations in tables.values() {
        let mirrored = match declarations.as_slice() {
            [_] => true,
            [(a, left), (b, right)] => {
                let (l, r) = (left.join.as_ref(), right.join.as_ref());
                left.target == b.id
                    && right.target == a.id
                    && l.zip(r).is_some_and(|(l, r)| {
                        l.physical_table == r.physical_table
                            && l.source == r.target
                            && l.target == r.source
                    })
            }
            _ => false,
        };
        if !mirrored {
            for (collection, relation) in declarations {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::InvalidJoinTable,
                    "a join table is shared only by two exactly inverse relations".to_string(),
                    &collection.id,
                    Some(relation.id.as_str()),
                ));
            }
        }
    }
}

/// A schema that passed [`RuntimeSchema::validate`]; the only source of canonical bytes and
/// fingerprints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedRuntimeSchema {
    schema: RuntimeSchema,
}

impl ValidatedRuntimeSchema {
    pub fn schema(&self) -> &RuntimeSchema {
        &self.schema
    }

    pub fn into_schema(self) -> RuntimeSchema {
        self.schema
    }

    /// Deterministic canonical serialization including stable IDs. Collections and their parts
    /// are rendered in stable-ID order, so declaration/insertion order never changes the bytes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.render(true).into_bytes()
    }

    /// Every physical table the runtime schema owns: collection tables and the
    /// distinct many-to-many join tables, in declaration order. Hosts include them
    /// in the `ManagedTableSet` of owned migrations and mutation environments.
    pub fn owned_table_names(&self) -> Vec<String> {
        let mut names = self
            .schema
            .collections
            .iter()
            .map(|c| c.physical_table.clone())
            .collect::<Vec<_>>();
        for relation in self.schema.collections.iter().flat_map(|c| &c.relations) {
            if let Some(join) = &relation.join
                && !names
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&join.physical_table))
            {
                names.push(join.physical_table.clone());
            }
        }
        names
    }

    /// Fingerprint of [`Self::canonical_bytes`]. Detects structural drift including stable-ID
    /// changes; not a cryptographic signature.
    pub fn fingerprint(&self) -> SchemaFingerprint {
        SchemaFingerprint::from_bytes(&self.canonical_bytes())
    }

    /// Fingerprint of the ID-free **structural** schema: collections ordered by API type name,
    /// parts ordered by API/physical name, references rendered as names. Equivalent
    /// static-derived and catalog-loaded schemas agree on this value even though their stable
    /// IDs differ.
    ///
    /// This is deliberately structural-only. It does not cover authorization policy hooks,
    /// backup enablement/ordering/redaction, runtime hooks, or relation change propagation —
    /// none of which the IR represents. [`RuntimeSchema::from_static_entities`] fails closed
    /// with diagnostics when static metadata carries such semantics, so a successful conversion
    /// plus equal structural fingerprints genuinely means the schemas describe the same tables,
    /// fields, keys, relations, indexes, and defaults.
    pub fn structural_fingerprint(&self) -> SchemaFingerprint {
        SchemaFingerprint::from_bytes(self.render(false).as_bytes())
    }

    /// Fingerprint of only the semantic field format descriptors, keyed by stable
    /// collection and field IDs. Hosts use it with [`Self::fingerprint`] and the
    /// physical target hash to classify a descriptor-only revision. A schema
    /// without descriptors has one constant value.
    pub fn format_fingerprint(&self) -> SchemaFingerprint {
        let mut out = String::from("runtime-schema-formats v1\n");
        let mut collections = self.schema.collections.iter().collect::<Vec<_>>();
        collections.sort_by(|a, b| a.id.cmp(&b.id));
        for collection in collections {
            let mut fields = collection.fields.iter().collect::<Vec<_>>();
            fields.sort_by(|a, b| a.id.cmp(&b.id));
            for field in fields {
                if let Some(format) = &field.format {
                    out.push_str(&format!(
                        "{}|{}|{}\n",
                        collection.id,
                        field.id,
                        serde_json::to_string(&format.canonical())
                            .expect("strings always serialize")
                    ));
                }
            }
        }
        SchemaFingerprint::from_bytes(out.as_bytes())
    }

    fn render(&self, with_ids: bool) -> String {
        let schema = &self.schema;
        let mut collections: Vec<&RuntimeCollection> = schema.collections.iter().collect();
        if with_ids {
            collections.sort_by(|a, b| a.id.cmp(&b.id));
        } else {
            collections.sort_by(|a, b| a.api_type_name.cmp(&b.api_type_name));
        }

        // Field IDs render as either the stable ID or the physical column name; both are unique
        // within a validated collection.
        let field_names: BTreeMap<(&CollectionId, &FieldId), &str> = schema
            .collections
            .iter()
            .flat_map(|c| {
                c.fields
                    .iter()
                    .map(move |f| ((&c.id, &f.id), f.physical_column.as_str()))
            })
            .collect();
        let field_ref = |collection: &CollectionId, field: &FieldId| -> String {
            if with_ids {
                field.to_string()
            } else {
                field_names
                    .get(&(collection, field))
                    .map(|name| (*name).to_string())
                    .unwrap_or_default()
            }
        };

        let mut out = String::from("runtime-schema v1\n");
        for collection in collections {
            let cid = &collection.id;
            out.push_str("collection");
            if with_ids {
                out.push_str(&format!("|id={cid}"));
            }
            out.push_str(&format!(
                "|type={}|plural={}|table={}|append_only={}",
                collection.api_type_name,
                collection.api_plural_name,
                collection.physical_table,
                collection.append_only
            ));
            if collection.retention_purge {
                out.push_str("|retention_purge=true");
            }
            if with_ids {
                push_semantic_members(
                    &mut out,
                    collection.description.as_deref(),
                    collection.classification,
                    None,
                );
                // Ordered and present only when declared, so earlier bytes are unchanged.
                // A JSON array frames each ID, which may itself contain commas.
                if !collection.label_fields.is_empty() {
                    out.push_str("|label=");
                    out.push_str(
                        &serde_json::to_string(
                            &collection
                                .label_fields
                                .iter()
                                .map(|f| f.as_str())
                                .collect::<Vec<_>>(),
                        )
                        .expect("strings always serialize"),
                    );
                }
            }
            out.push('\n');

            out.push_str(&format!(
                "  primary_key|{}\n",
                collection
                    .primary_key
                    .iter()
                    .map(|f| field_ref(cid, f))
                    .collect::<Vec<_>>()
                    .join(",")
            ));

            let mut fields: Vec<&RuntimeField> = collection.fields.iter().collect();
            if with_ids {
                fields.sort_by(|a, b| a.id.cmp(&b.id));
            } else {
                fields.sort_by(|a, b| a.api_name.cmp(&b.api_name));
            }
            for field in fields {
                out.push_str("  field");
                if with_ids {
                    out.push_str(&format!("|id={}", field.id));
                }
                let default = match &field.default {
                    None => "none".to_string(),
                    // JSON-string-escaped and quote-framed (RFC 8259, a stable cross-version
                    // encoding) so hostile literals containing delimiters or newlines cannot
                    // make distinct schemas render identical canonical bytes.
                    Some(RuntimeDefault::Literal(value)) => format!(
                        "literal:{}",
                        serde_json::to_string(value).expect("strings always serialize")
                    ),
                    Some(RuntimeDefault::CurrentTimestamp) => "current_timestamp".to_string(),
                };
                out.push_str(&format!(
                    "|api={}|column={}|kind={}|nullable={}|unique={}|filterable={}|sortable={}|generated={}|default={}",
                    field.api_name,
                    field.physical_column,
                    field.value_kind.canonical_name(),
                    field.nullable,
                    field.unique,
                    field.filterable,
                    field.sortable,
                    field.generated,
                    default
                ));
                // Write semantics, so structural as well; present only when declared,
                // so schemas without it keep their exact earlier bytes.
                if let Some(auto) = field.auto_timestamp {
                    out.push_str("|auto_timestamp=");
                    out.push_str(auto.as_str());
                }
                if let Some(actor) = field.auto_actor {
                    out.push_str("|auto_actor=");
                    out.push_str(actor.as_str());
                }
                // Semantic descriptors join the ID-bearing identity only when present, so
                // schemas without them keep their exact earlier bytes. The structural
                // fingerprint stays structural-only.
                if with_ids && let Some(format) = &field.format {
                    out.push_str("|format=");
                    out.push_str(
                        &serde_json::to_string(&format.canonical())
                            .expect("strings always serialize"),
                    );
                }
                if with_ids {
                    push_semantic_members(
                        &mut out,
                        field.description.as_deref(),
                        field.classification,
                        field.export,
                    );
                }
                out.push('\n');
            }

            let mut relations: Vec<&RuntimeRelation> = collection.relations.iter().collect();
            if with_ids {
                relations.sort_by(|a, b| a.id.cmp(&b.id));
            } else {
                relations.sort_by(|a, b| a.api_name.cmp(&b.api_name));
            }
            let target_names: BTreeMap<&CollectionId, &str> = schema
                .collections
                .iter()
                .map(|c| (&c.id, c.api_type_name.as_str()))
                .collect();
            for relation in relations {
                out.push_str("  relation");
                if with_ids {
                    out.push_str(&format!("|id={}", relation.id));
                }
                let target = if with_ids {
                    relation.target.to_string()
                } else {
                    target_names
                        .get(&relation.target)
                        .map(|name| (*name).to_string())
                        .unwrap_or_default()
                };
                let pairs = relation
                    .key_pairs
                    .iter()
                    .map(|pair| {
                        format!(
                            "{}={}",
                            field_ref(cid, &pair.source),
                            field_ref(&relation.target, &pair.target)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let on_delete = relation
                    .on_delete
                    .as_ref()
                    .map(|policy| policy.as_sql().to_ascii_lowercase())
                    .unwrap_or_else(|| "none".to_string());
                out.push_str(&format!(
                    "|api={}|target={}|keys={}|cardinality={}|fk={}|on_delete={}",
                    relation.api_name,
                    target,
                    pairs,
                    relation.cardinality.as_str(),
                    relation.enforce_foreign_key,
                    on_delete
                ));
                // Structural; present only for join relations, so earlier bytes are unchanged.
                if let Some(join) = &relation.join {
                    let side = |columns: &[RuntimeJoinColumn], owner: &CollectionId| {
                        columns
                            .iter()
                            .map(|c| format!("{}={}", field_ref(owner, &c.field), c.column))
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    out.push_str(&format!(
                        "|join={}|join_source={}|join_target={}",
                        join.physical_table,
                        side(&join.source, cid),
                        side(&join.target, &relation.target)
                    ));
                }
                if with_ids {
                    push_semantic_members(
                        &mut out,
                        relation.description.as_deref(),
                        relation.classification,
                        relation.export,
                    );
                }
                out.push('\n');
            }

            let mut indexes: Vec<&RuntimeIndex> = collection.indexes.iter().collect();
            if with_ids {
                indexes.sort_by(|a, b| a.id.cmp(&b.id));
            } else {
                indexes.sort_by(|a, b| a.name.cmp(&b.name));
            }
            for index in indexes {
                out.push_str("  index");
                if with_ids {
                    out.push_str(&format!("|id={}", index.id));
                }
                out.push_str(&format!(
                    "|name={}|fields={}|unique={}\n",
                    index.name,
                    index
                        .fields
                        .iter()
                        .map(|f| field_ref(cid, f))
                        .collect::<Vec<_>>()
                        .join(","),
                    index.unique
                ));
            }

            let mut groups: Vec<String> = collection
                .composite_unique
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|f| field_ref(cid, f))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .collect();
            groups.sort();
            for group in groups {
                out.push_str(&format!("  composite_unique|{group}\n"));
            }

            if !collection.default_order.is_empty() {
                let order = collection
                    .default_order
                    .iter()
                    .map(|term| {
                        format!(
                            "{} {}",
                            field_ref(cid, &term.field),
                            term.direction.as_str()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                out.push_str(&format!("  default_order|{order}\n"));
            }
        }
        out
    }
}

const EPOCH_DEFAULT_EXPRESSIONS: &[&str] = &[
    "unixepoch()",
    "EXTRACT(EPOCH FROM NOW())::bigint",
    "DATEDIFF_BIG(second, '1970-01-01', SYSUTCDATETIME())",
];

fn convert_default(raw: &str, kind: RuntimeValueKind) -> Option<RuntimeDefault> {
    if let RuntimeValueKind::Decimal(definition) = kind {
        return convert_decimal_default(raw, definition);
    }
    if EPOCH_DEFAULT_EXPRESSIONS.contains(&raw) {
        return (kind == RuntimeValueKind::Integer).then_some(RuntimeDefault::CurrentTimestamp);
    }
    if let Some(quoted) = raw
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        let mut value = String::with_capacity(quoted.len());
        let mut characters = quoted.chars();
        while let Some(character) = characters.next() {
            if character == '\'' && characters.next() != Some('\'') {
                return None;
            }
            value.push(character);
        }
        return Some(RuntimeDefault::Literal(value));
    }
    let literal = raw.parse::<i64>().is_ok()
        || raw.parse::<f64>().is_ok()
        || raw.eq_ignore_ascii_case("true")
        || raw.eq_ignore_ascii_case("false");
    literal.then(|| RuntimeDefault::Literal(raw.to_owned()))
}

/// Static decimal defaults are backend SQL: the scaled mantissa on SQLite and
/// the normalized literal (with `scale` fractional digits) elsewhere. With a
/// non-zero scale, only the latter contains a point; with scale 0 they agree.
fn convert_decimal_default(raw: &str, definition: DecimalDef) -> Option<RuntimeDefault> {
    let value = if definition.scale() > 0 && !raw.contains('.') {
        let mantissa = raw.parse::<i64>().ok()?;
        super::DecimalValue::new(definition.from_scaled_i64(mantissa), definition).ok()?
    } else {
        super::parse_runtime_decimal(definition, raw).ok()?
    };
    Some(RuntimeDefault::decimal(value))
}

fn convert_value_kind(field: &FieldMetadata) -> Option<RuntimeValueKind> {
    if field.is_date_time {
        return Some(RuntimeValueKind::DateTime);
    }
    match field.logical_type {
        BackupValueKind::Bool => Some(RuntimeValueKind::Boolean),
        BackupValueKind::Integer => Some(RuntimeValueKind::Integer),
        BackupValueKind::Float => Some(RuntimeValueKind::Float),
        // The exact definition describes the real storage: a scaled integer on
        // SQLite, `NUMERIC(p,s)` on PostgreSQL.
        BackupValueKind::Decimal => field.decimal.map(RuntimeValueKind::Decimal),
        BackupValueKind::String => Some(RuntimeValueKind::String),
        BackupValueKind::Uuid => Some(RuntimeValueKind::Uuid),
        BackupValueKind::Json => Some(RuntimeValueKind::Json),
        BackupValueKind::Bytes => Some(RuntimeValueKind::Bytes),
        BackupValueKind::DateTime => Some(RuntimeValueKind::DateTime),
        BackupValueKind::Null => None,
    }
}

impl RuntimeSchema {
    /// Converts static [`EntityMetadata`] graphs into the owned IR so compile-time and runtime
    /// schemas converge on one semantic representation.
    ///
    /// Stable IDs are synthesized deterministically from physical names (`<table>`,
    /// `<table>.<column>`, `<table>.<relation>`); they are stable for a given static schema but
    /// carry no meaning beyond it, so compare converted schemas with
    /// [`ValidatedRuntimeSchema::structural_fingerprint`].
    ///
    /// Static declarations using capabilities the IR does not represent yet (spatial, search,
    /// partial/GiST indexes, check constraints) are reported as diagnostics rather than dropped.
    /// Legacy epoch-second defaults on DateTime fields are rejected with `UnsupportedDefault`;
    /// integer epoch-second defaults remain supported. Static declarations/storage are unchanged.
    pub fn from_static_entities(
        entities: &[&EntityMetadata],
    ) -> Result<Self, RuntimeSchemaDiagnostics> {
        let mut diagnostics = Vec::new();
        let mut collections = Vec::new();

        let table_by_entity: BTreeMap<&str, &EntityMetadata> = entities
            .iter()
            .map(|entity| (entity.entity_name, *entity))
            .collect();

        for entity in entities {
            let cid = match CollectionId::new(entity.table_name) {
                Ok(cid) => cid,
                Err(diagnostic) => {
                    diagnostics.push(diagnostic);
                    continue;
                }
            };

            if entity.search.is_some() {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                    "full-text search indexes are not represented in the runtime IR yet"
                        .to_string(),
                    &cid,
                    None,
                ));
            }
            // Fail closed on semantics the IR does not represent: authorization policy hooks,
            // backup enablement/ordering, and non-managed schema ownership. Dropping these
            // silently would let a structural fingerprint claim equivalence between schemas
            // with different security or persistence behavior.
            if entity.read_policy.is_some() || entity.write_policy.is_some() {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                    "entity read/write policy hooks are not represented in the runtime IR"
                        .to_string(),
                    &cid,
                    None,
                ));
            }
            if !entity.backup_enabled
                || entity.backup_export_order.is_some()
                || entity.backup_restore_order.is_some()
            {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                    "non-default backup enablement/ordering is not represented in the runtime IR"
                        .to_string(),
                    &cid,
                    None,
                ));
            }
            if !matches!(entity.schema_policy, None | Some(SchemaPolicy::Managed)) {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                    "only managed schema ownership converts to the runtime IR".to_string(),
                    &cid,
                    None,
                ));
            }
            if !entity.check_constraints.is_empty() {
                diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                    "check constraints are not represented in the runtime IR yet".to_string(),
                    &cid,
                    None,
                ));
            }

            let field_id = |column: &str| FieldId::new(format!("{}.{column}", entity.table_name));

            let mut fields = Vec::new();
            for field in entity.fields.iter() {
                let id = match field_id(field.name) {
                    Ok(id) => id,
                    Err(diagnostic) => {
                        diagnostics.push(diagnostic);
                        continue;
                    }
                };
                if field.spatial.is_some() || field.search.is_some() {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "column `{}` uses spatial/search metadata not represented in the runtime IR yet",
                            field.name
                        ),
                        &cid,
                        Some(field.name),
                    ));
                    continue;
                }
                if field.backup_policy != ColumnBackupPolicy::Include {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "column `{}` backup exclusion/redaction is not represented in the runtime IR",
                            field.name
                        ),
                        &cid,
                        Some(field.name),
                    ));
                }
                if field.decimal.is_some_and(DecimalDef::is_mssql_money) {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "column `{}` is SQL Server MONEY, which the runtime IR does not represent; runtime decimals are portable (precision 1..=18)",
                            field.name
                        ),
                        &cid,
                        Some(field.name),
                    ));
                    continue;
                }
                let Some(value_kind) = convert_value_kind(field) else {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "column `{}` has no convertible logical value kind",
                            field.name
                        ),
                        &cid,
                        Some(field.name),
                    ));
                    continue;
                };
                let default = match field.default {
                    None => None,
                    Some(raw) => match convert_default(raw, value_kind) {
                        Some(default) => Some(default),
                        None => {
                            diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                                RuntimeSchemaDiagnosticCode::UnsupportedDefault,
                                if value_kind == RuntimeValueKind::DateTime && EPOCH_DEFAULT_EXPRESSIONS.contains(&raw) {
                                    format!("column `{}` uses legacy epoch-second datetime default `{raw}`; canonical runtime DateTime requires RFC3339/native timestamp storage, not epoch seconds", field.name)
                                } else {
                                    format!("column `{}` default `{raw}` is not a portable literal or supported timestamp expression for its value kind", field.name)
                                },
                                &cid,
                                Some(field.name),
                            ));
                            None
                        }
                    },
                };
                fields.push(RuntimeField {
                    description: None,
                    classification: None,
                    export: None,
                    id,
                    api_name: field.api_name.to_string(),
                    physical_column: field.name.to_string(),
                    value_kind,
                    nullable: field.nullable,
                    unique: field.is_unique,
                    filterable: field.is_filterable,
                    sortable: field.is_sortable,
                    generated: field.is_generated,
                    default,
                    format: None,
                    auto_timestamp: None,
                    auto_actor: None,
                });
            }

            let mut relations = Vec::new();
            for relation in entity.relations.iter() {
                let id =
                    match RelationId::new(format!("{}.{}", entity.table_name, relation.field_name))
                    {
                        Ok(id) => id,
                        Err(diagnostic) => {
                            diagnostics.push(diagnostic);
                            continue;
                        }
                    };
                if relation.search_fields.is_some() {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "relation `{}` search fields are not represented in the runtime IR yet",
                            relation.field_name
                        ),
                        &cid,
                        Some(relation.field_name),
                    ));
                }
                if relation.propagate_change == RelationChangePropagation::Up {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "relation `{}` upward change propagation is not represented in the runtime IR",
                            relation.field_name
                        ),
                        &cid,
                        Some(relation.field_name),
                    ));
                }
                let Some(target_entity) = table_by_entity.get(relation.target_type) else {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnknownRelationTarget,
                        format!(
                            "relation `{}` targets `{}`, which is not among the converted entities",
                            relation.field_name, relation.target_type
                        ),
                        &cid,
                        Some(relation.field_name),
                    ));
                    continue;
                };
                let target = match CollectionId::new(target_entity.table_name) {
                    Ok(target) => target,
                    Err(diagnostic) => {
                        diagnostics.push(diagnostic);
                        continue;
                    }
                };
                let mut key_pairs = Vec::new();
                for (source, target_column) in relation
                    .source_columns
                    .iter()
                    .zip(relation.target_columns.iter())
                {
                    let source = field_id(source);
                    let target_field =
                        FieldId::new(format!("{}.{target_column}", target_entity.table_name));
                    match (source, target_field) {
                        (Ok(source), Ok(target)) => {
                            key_pairs.push(RelationKeyPair { source, target });
                        }
                        (source, target) => {
                            for result in [source, target] {
                                if let Err(diagnostic) = result {
                                    diagnostics.push(diagnostic);
                                }
                            }
                        }
                    }
                }
                if relation.source_columns.len() != relation.target_columns.len() {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::EmptyRelationKeys,
                        format!(
                            "relation `{}` declares {} source and {} target columns",
                            relation.field_name,
                            relation.source_columns.len(),
                            relation.target_columns.len()
                        ),
                        &cid,
                        Some(relation.field_name),
                    ));
                }
                relations.push(RuntimeRelation {
                    description: None,
                    classification: None,
                    export: None,
                    id,
                    api_name: relation.field_name.to_string(),
                    target,
                    key_pairs,
                    cardinality: if relation.is_multiple {
                        RelationCardinality::Many
                    } else {
                        RelationCardinality::One
                    },
                    enforce_foreign_key: relation.emit_foreign_key,
                    on_delete: relation
                        .emit_foreign_key
                        .then(|| relation.on_delete.clone()),
                    join: None,
                });
            }

            let mut indexes = Vec::new();
            for index in entity.indexes.iter() {
                let id = match IndexId::new(index.name) {
                    Ok(id) => id,
                    Err(diagnostic) => {
                        diagnostics.push(diagnostic);
                        continue;
                    }
                };
                if index.is_spatial
                    || index.method != IndexMethod::Default
                    || index.predicate.is_some()
                {
                    diagnostics.push(RuntimeSchemaDiagnostic::scoped(
                        RuntimeSchemaDiagnosticCode::UnsupportedCapability,
                        format!(
                            "index `{}` uses spatial/method/predicate features not represented in the runtime IR yet",
                            index.name
                        ),
                        &cid,
                        Some(index.name),
                    ));
                    continue;
                }
                let mut index_fields = Vec::new();
                for column in index.columns.iter() {
                    match field_id(column) {
                        Ok(field) => index_fields.push(field),
                        Err(diagnostic) => diagnostics.push(diagnostic),
                    }
                }
                indexes.push(RuntimeIndex {
                    id,
                    name: index.name.to_string(),
                    fields: index_fields,
                    unique: index.is_unique,
                });
            }

            let mut primary_key = Vec::new();
            for column in entity.primary_keys.iter() {
                match field_id(column) {
                    Ok(field) => primary_key.push(field),
                    Err(diagnostic) => diagnostics.push(diagnostic),
                }
            }

            let mut composite_unique = Vec::new();
            for group in entity.composite_unique_indexes.iter() {
                let mut fields = Vec::new();
                for column in group.iter() {
                    match field_id(column) {
                        Ok(field) => fields.push(field),
                        Err(diagnostic) => diagnostics.push(diagnostic),
                    }
                }
                composite_unique.push(fields);
            }

            let default_order = match parse_default_sort(entity, &cid) {
                Ok(order) => order,
                Err(diagnostic) => {
                    diagnostics.push(diagnostic);
                    Vec::new()
                }
            };

            collections.push(RuntimeCollection {
                description: None,
                classification: None,
                id: cid,
                api_type_name: entity.entity_name.to_string(),
                api_plural_name: entity.plural_name.to_string(),
                physical_table: entity.table_name.to_string(),
                primary_key,
                append_only: entity.append_only,
                retention_purge: entity.retention_policy.is_some(),
                fields,
                relations,
                indexes,
                composite_unique,
                default_order,
                label_fields: Vec::new(),
            });
        }

        if diagnostics.is_empty() {
            Ok(Self {
                format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
                collections,
            })
        } else {
            Err(RuntimeSchemaDiagnostics(diagnostics))
        }
    }
}

fn parse_default_sort(
    entity: &EntityMetadata,
    cid: &CollectionId,
) -> Result<Vec<RuntimeOrderTerm>, RuntimeSchemaDiagnostic> {
    let raw = entity.default_sort.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut terms = Vec::new();
    for part in raw.split(',') {
        let mut tokens = part.split_whitespace();
        let column = tokens.next().unwrap_or_default();
        let direction = match tokens.next() {
            None => RuntimeOrderDirection::Asc,
            Some(token) if token.eq_ignore_ascii_case("asc") => RuntimeOrderDirection::Asc,
            Some(token) if token.eq_ignore_ascii_case("desc") => RuntimeOrderDirection::Desc,
            Some(other) => {
                return Err(RuntimeSchemaDiagnostic::scoped(
                    RuntimeSchemaDiagnosticCode::InvalidDefaultOrder,
                    format!("default_sort direction `{other}` is not ASC or DESC"),
                    cid,
                    Some(entity.default_sort),
                ));
            }
        };
        if column.is_empty() || tokens.next().is_some() {
            return Err(RuntimeSchemaDiagnostic::scoped(
                RuntimeSchemaDiagnosticCode::InvalidDefaultOrder,
                format!(
                    "default_sort `{}` is not `column [ASC|DESC]` terms",
                    entity.default_sort
                ),
                cid,
                Some(entity.default_sort),
            ));
        }
        terms.push(RuntimeOrderTerm {
            field: FieldId::new(format!("{}.{column}", entity.table_name))?,
            direction,
        });
    }
    Ok(terms)
}
