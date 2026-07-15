//! Owned values, records, validated schema handles, and backend row decoding.
//!
//! This module is the read-side value boundary for the owned runtime schema
//! IR. It deliberately does not render or execute queries. A later executor
//! can resolve trusted handles once, select their validated physical columns,
//! and pass backend rows to [`RuntimeProjection::decode_row`] without copying
//! schema validation or value decoding into a host application.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt;

use chrono::{SecondsFormat, Timelike, Utc};

use super::{
    CollectionId, FieldId, NoDefaultBackend, OrmBackend, RelationCardinality, RelationId,
    RuntimeValueKind, SchemaFingerprint, ValidatedRuntimeSchema,
};

/// Serialized format version for [`RuntimeRecord`].
pub const RUNTIME_RECORD_FORMAT_VERSION: u32 = 1;

/// Stable machine-usable category for runtime handle, value, record, and row
/// decoding failures.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRecordErrorCode {
    /// A collection stable ID is not present in the validated schema.
    UnknownCollection,
    /// A field stable ID is not present in the requested collection or record.
    UnknownField,
    /// A relation stable ID is not present in the requested collection.
    UnknownRelation,
    /// A field handle belongs to a different collection.
    CrossCollectionField,
    /// A projection contains the same field more than once.
    DuplicateProjectionField,
    /// A projection contains no fields.
    EmptyProjection,
    /// A handle or record belongs to a different validated schema fingerprint.
    SchemaMismatch,
    /// A known field was not selected into this record.
    FieldUnloaded,
    /// A selected field contains SQL `NULL` where a value was requested.
    NullValue,
    /// A selected field or typed accessor used the wrong logical value kind.
    WrongValueKind,
    /// A non-nullable runtime field decoded as SQL `NULL`.
    NonNullableNull,
    /// A selected projection column was absent from the backend row.
    MissingColumn,
    /// A backend column could not be decoded as its declared logical kind.
    BackendTypeMismatch,
    /// A decoded scalar was malformed or non-portable.
    InvalidValue,
    /// The selected backend does not implement runtime row decoding.
    UnsupportedBackend,
    /// Serialized runtime-record data uses an unsupported format or violates
    /// record invariants.
    InvalidRecord,
}

impl RuntimeRecordErrorCode {
    /// Stable lowercase representation suitable for logs and host error maps.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownCollection => "unknown_collection",
            Self::UnknownField => "unknown_field",
            Self::UnknownRelation => "unknown_relation",
            Self::CrossCollectionField => "cross_collection_field",
            Self::DuplicateProjectionField => "duplicate_projection_field",
            Self::EmptyProjection => "empty_projection",
            Self::SchemaMismatch => "schema_mismatch",
            Self::FieldUnloaded => "field_unloaded",
            Self::NullValue => "null_value",
            Self::WrongValueKind => "wrong_value_kind",
            Self::NonNullableNull => "non_nullable_null",
            Self::MissingColumn => "missing_column",
            Self::BackendTypeMismatch => "backend_type_mismatch",
            Self::InvalidValue => "invalid_value",
            Self::UnsupportedBackend => "unsupported_backend",
            Self::InvalidRecord => "invalid_record",
        }
    }
}

impl fmt::Display for RuntimeRecordErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Safe runtime-record error with optional stable-ID context and a retained
/// backend source for trusted server-side logging.
///
/// [`Display`](fmt::Display) and [`Debug`](fmt::Debug) never render physical
/// identifiers, SQL, raw values, or the backend source. Hosts may inspect the
/// standard [`Error::source`](StdError::source) chain in trusted logs.
pub struct RuntimeRecordError {
    code: RuntimeRecordErrorCode,
    collection: Option<CollectionId>,
    field: Option<FieldId>,
    relation: Option<RelationId>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RuntimeRecordError {
    /// Construct a safe error with a stable machine-usable category.
    pub fn new(code: RuntimeRecordErrorCode) -> Self {
        Self {
            code,
            collection: None,
            field: None,
            relation: None,
            source: None,
        }
    }

    fn collection(mut self, collection: &CollectionId) -> Self {
        self.collection = Some(collection.clone());
        self
    }

    fn field(mut self, field: &FieldId) -> Self {
        self.field = Some(field.clone());
        self
    }

    fn relation(mut self, relation: &RelationId) -> Self {
        self.relation = Some(relation.clone());
        self
    }

    /// Retain a backend or parsing source without including it in public
    /// [`Display`](fmt::Display) or [`Debug`](fmt::Debug) output.
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Attach the stable collection and field IDs from a trusted field handle.
    pub fn for_field(mut self, handle: &RuntimeFieldHandle) -> Self {
        self.collection = Some(handle.collection.clone());
        self.field = Some(handle.id.clone());
        self
    }

    /// Stable machine-usable category.
    pub const fn code(&self) -> RuntimeRecordErrorCode {
        self.code
    }

    /// Stable collection context, when the failure is collection-scoped.
    pub fn collection_id(&self) -> Option<&CollectionId> {
        self.collection.as_ref()
    }

    /// Stable field context, when the failure is field-scoped.
    pub fn field_id(&self) -> Option<&FieldId> {
        self.field.as_ref()
    }

    /// Stable relation context, when the failure is relation-scoped.
    pub fn relation_id(&self) -> Option<&RelationId> {
        self.relation.as_ref()
    }
}

impl fmt::Debug for RuntimeRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRecordError")
            .field("code", &self.code)
            .field("collection", &self.collection)
            .field("field", &self.field)
            .field("relation", &self.relation)
            .field("source", &self.source.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl fmt::Display for RuntimeRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "runtime record error: {}", self.code)
    }
}

impl StdError for RuntimeRecordError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

/// Finite, deterministic runtime floating-point value.
///
/// Construction rejects NaN and infinities and normalizes negative zero to
/// positive zero, making equality and serialization portable.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeFloat(u64);

impl RuntimeFloat {
    /// Construct a finite value.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeRecordErrorCode::InvalidValue`] for NaN or either
    /// infinity.
    pub fn new(value: f64) -> Result<Self, RuntimeRecordError> {
        if !value.is_finite() {
            return Err(RuntimeRecordError::new(
                RuntimeRecordErrorCode::InvalidValue,
            ));
        }
        let value = if value == 0.0 { 0.0 } else { value };
        Ok(Self(value.to_bits()))
    }

    /// Return the finite `f64` value.
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl fmt::Debug for RuntimeFloat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

impl fmt::Display for RuntimeFloat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

impl serde::Serialize for RuntimeFloat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(self.get())
    }
}

impl<'de> serde::Deserialize<'de> for RuntimeFloat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <f64 as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Canonical runtime date-time value.
///
/// Inputs use RFC 3339. Offsets are normalized to UTC, precision is rounded
/// to PostgreSQL-compatible microseconds, and serialization always uses
/// `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeDateTime(String);

impl RuntimeDateTime {
    /// Parse and canonicalize an RFC 3339 date-time.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeRecordErrorCode::InvalidValue`] when `value` is not a
    /// valid RFC 3339 date-time.
    pub fn parse(value: &str) -> Result<Self, RuntimeRecordError> {
        let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|error| {
            RuntimeRecordError::new(RuntimeRecordErrorCode::InvalidValue).with_source(error)
        })?;
        Ok(Self::from_utc(parsed.with_timezone(&Utc)))
    }

    /// Canonicalize a UTC date-time to portable microsecond precision.
    pub(crate) fn from_utc(value: chrono::DateTime<Utc>) -> Self {
        let rounded = value
            .checked_add_signed(chrono::Duration::nanoseconds(500))
            .unwrap_or(value);
        let microsecond_nanos = (rounded.nanosecond() / 1_000) * 1_000;
        let value = rounded
            .with_nanosecond(microsecond_nanos)
            .unwrap_or(rounded);
        Self(value.to_rfc3339_opts(SecondsFormat::Micros, true))
    }

    /// Return the canonical UTC RFC 3339 representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimeDateTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl serde::Serialize for RuntimeDateTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RuntimeDateTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Owned backend-neutral runtime value.
///
/// [`RuntimeValue::Null`] is an explicitly selected SQL `NULL`. An unselected
/// field is represented by absence from [`RuntimeRecord`], not by this enum.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeValue {
    /// Explicitly selected SQL `NULL`.
    Null,
    /// Boolean value.
    Boolean(bool),
    /// Signed 64-bit integer value.
    Integer(i64),
    /// Finite, normalized floating-point value.
    Float(RuntimeFloat),
    /// Unicode string value.
    String(String),
    /// UUID value.
    Uuid(uuid::Uuid),
    /// Structured JSON value.
    Json(serde_json::Value),
    /// Arbitrary bytes.
    Bytes(Vec<u8>),
    /// Canonical UTC RFC 3339 date-time.
    DateTime(RuntimeDateTime),
}

impl RuntimeValue {
    /// Logical kind, or `None` for explicit SQL `NULL`.
    pub const fn kind(&self) -> Option<RuntimeValueKind> {
        match self {
            Self::Null => None,
            Self::Boolean(_) => Some(RuntimeValueKind::Boolean),
            Self::Integer(_) => Some(RuntimeValueKind::Integer),
            Self::Float(_) => Some(RuntimeValueKind::Float),
            Self::String(_) => Some(RuntimeValueKind::String),
            Self::Uuid(_) => Some(RuntimeValueKind::Uuid),
            Self::Json(_) => Some(RuntimeValueKind::Json),
            Self::Bytes(_) => Some(RuntimeValueKind::Bytes),
            Self::DateTime(_) => Some(RuntimeValueKind::DateTime),
        }
    }
}

/// Trusted owned handle to one collection in a specific validated schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCollectionHandle {
    schema_fingerprint: SchemaFingerprint,
    id: CollectionId,
    physical_table: String,
}

impl RuntimeCollectionHandle {
    /// Validated schema fingerprint that created this handle.
    pub fn schema_fingerprint(&self) -> &SchemaFingerprint {
        &self.schema_fingerprint
    }

    /// Stable collection ID.
    pub fn id(&self) -> &CollectionId {
        &self.id
    }

    /// Already-validated physical table identifier.
    pub fn physical_table(&self) -> &str {
        &self.physical_table
    }
}

/// Trusted owned handle to one field in a specific collection and schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeFieldHandle {
    schema_fingerprint: SchemaFingerprint,
    collection: CollectionId,
    id: FieldId,
    physical_column: String,
    value_kind: RuntimeValueKind,
    nullable: bool,
}

impl RuntimeFieldHandle {
    /// Validated schema fingerprint that created this handle.
    pub fn schema_fingerprint(&self) -> &SchemaFingerprint {
        &self.schema_fingerprint
    }

    /// Stable owning collection ID.
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection
    }

    /// Stable field ID.
    pub fn id(&self) -> &FieldId {
        &self.id
    }

    /// Already-validated physical column identifier.
    pub fn physical_column(&self) -> &str {
        &self.physical_column
    }

    /// Declared logical value kind.
    pub const fn value_kind(&self) -> RuntimeValueKind {
        self.value_kind
    }

    /// Whether SQL `NULL` is valid for this field.
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}

/// Trusted owned handle to one relation and its resolved ordered key pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRelationHandle {
    schema_fingerprint: SchemaFingerprint,
    source: RuntimeCollectionHandle,
    id: RelationId,
    target: RuntimeCollectionHandle,
    cardinality: RelationCardinality,
    key_pairs: Vec<(RuntimeFieldHandle, RuntimeFieldHandle)>,
}

impl RuntimeRelationHandle {
    /// Validated schema fingerprint that created this handle.
    pub fn schema_fingerprint(&self) -> &SchemaFingerprint {
        &self.schema_fingerprint
    }

    /// Stable relation ID.
    pub fn id(&self) -> &RelationId {
        &self.id
    }

    /// Resolved declaring collection.
    pub fn source(&self) -> &RuntimeCollectionHandle {
        &self.source
    }

    /// Resolved target collection.
    pub fn target(&self) -> &RuntimeCollectionHandle {
        &self.target
    }

    /// Declared relation cardinality from the source perspective.
    pub const fn cardinality(&self) -> RelationCardinality {
        self.cardinality
    }

    /// Ordered `(source, target)` field handles.
    pub fn key_pairs(&self) -> &[(RuntimeFieldHandle, RuntimeFieldHandle)] {
        &self.key_pairs
    }
}

/// Trusted resolved subset of one runtime collection.
///
/// Projection construction rejects empty or duplicate members and binds every
/// field to the same validated schema fingerprint and collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProjection {
    schema_fingerprint: SchemaFingerprint,
    collection: RuntimeCollectionHandle,
    fields: Vec<RuntimeFieldHandle>,
    collection_field_kinds: BTreeMap<FieldId, RuntimeValueKind>,
}

impl RuntimeProjection {
    /// Validated schema fingerprint that created this projection.
    pub fn schema_fingerprint(&self) -> &SchemaFingerprint {
        &self.schema_fingerprint
    }

    /// Resolved collection selected by this projection.
    pub fn collection(&self) -> &RuntimeCollectionHandle {
        &self.collection
    }

    /// Selected fields in caller-supplied order.
    pub fn fields(&self) -> &[RuntimeFieldHandle] {
        &self.fields
    }

    /// Decode one backend row using only this projection's selected fields.
    /// Unexpected extra row columns are ignored.
    ///
    /// # Errors
    ///
    /// Returns a structured [`RuntimeRecordError`] for unsupported backends,
    /// missing selected columns, SQL `NULL` in non-nullable fields, backend
    /// type mismatches, or malformed portable values. No partial record is
    /// returned.
    pub fn decode_row<B>(&self, row: &B::Row) -> Result<RuntimeRecord, RuntimeRecordError>
    where
        B: RuntimeRowDecoder,
    {
        let mut values = BTreeMap::new();
        for field in &self.fields {
            let value = B::decode_runtime_value(row, field)?;
            if matches!(value, RuntimeValue::Null) && !field.nullable {
                return Err(
                    RuntimeRecordError::new(RuntimeRecordErrorCode::NonNullableNull)
                        .for_field(field),
                );
            }
            if value.kind().is_some_and(|kind| kind != field.value_kind) {
                return Err(
                    RuntimeRecordError::new(RuntimeRecordErrorCode::WrongValueKind)
                        .for_field(field),
                );
            }
            values.insert(field.id.clone(), value);
        }
        Ok(RuntimeRecord {
            format_version: RUNTIME_RECORD_FORMAT_VERSION,
            schema_fingerprint: self.schema_fingerprint.clone(),
            collection: self.collection.id.clone(),
            field_kinds: self.collection_field_kinds.clone(),
            values,
        })
    }
}

impl ValidatedRuntimeSchema {
    /// Resolve a trusted collection handle from a stable ID.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeRecordErrorCode::UnknownCollection`] when `id` is not
    /// present in this validated schema.
    pub fn resolve_collection(
        &self,
        id: &CollectionId,
    ) -> Result<RuntimeCollectionHandle, RuntimeRecordError> {
        let collection = self
            .schema()
            .collections
            .iter()
            .find(|collection| &collection.id == id)
            .ok_or_else(|| {
                RuntimeRecordError::new(RuntimeRecordErrorCode::UnknownCollection).collection(id)
            })?;
        Ok(RuntimeCollectionHandle {
            schema_fingerprint: self.fingerprint(),
            id: collection.id.clone(),
            physical_table: collection.physical_table.clone(),
        })
    }

    fn checked_collection<'a>(
        &'a self,
        handle: &RuntimeCollectionHandle,
    ) -> Result<&'a super::RuntimeCollection, RuntimeRecordError> {
        if handle.schema_fingerprint != self.fingerprint() {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::SchemaMismatch)
                    .collection(&handle.id),
            );
        }
        self.schema()
            .collections
            .iter()
            .find(|collection| collection.id == handle.id)
            .ok_or_else(|| {
                RuntimeRecordError::new(RuntimeRecordErrorCode::UnknownCollection)
                    .collection(&handle.id)
            })
    }

    /// Resolve a trusted field handle owned by `collection`.
    ///
    /// # Errors
    ///
    /// Returns a schema-mismatch error for a stale collection handle or an
    /// unknown-field error when `id` does not belong to the collection.
    pub fn resolve_field(
        &self,
        collection: &RuntimeCollectionHandle,
        id: &FieldId,
    ) -> Result<RuntimeFieldHandle, RuntimeRecordError> {
        let resolved_collection = self.checked_collection(collection)?;
        let field = resolved_collection
            .fields
            .iter()
            .find(|field| &field.id == id)
            .ok_or_else(|| {
                RuntimeRecordError::new(RuntimeRecordErrorCode::UnknownField)
                    .collection(&collection.id)
                    .field(id)
            })?;
        Ok(RuntimeFieldHandle {
            schema_fingerprint: collection.schema_fingerprint.clone(),
            collection: collection.id.clone(),
            id: field.id.clone(),
            physical_column: field.physical_column.clone(),
            value_kind: field.value_kind,
            nullable: field.nullable,
        })
    }

    /// Resolve a relation handle and its ordered source/target key handles.
    ///
    /// # Errors
    ///
    /// Returns a schema-mismatch error for a stale source handle or an
    /// unknown-relation error when `id` is not declared by the source.
    pub fn resolve_relation(
        &self,
        source: &RuntimeCollectionHandle,
        id: &RelationId,
    ) -> Result<RuntimeRelationHandle, RuntimeRecordError> {
        let source_collection = self.checked_collection(source)?;
        let relation = source_collection
            .relations
            .iter()
            .find(|relation| &relation.id == id)
            .ok_or_else(|| {
                RuntimeRecordError::new(RuntimeRecordErrorCode::UnknownRelation)
                    .collection(&source.id)
                    .relation(id)
            })?;
        let target = self.resolve_collection(&relation.target)?;
        let mut key_pairs = Vec::with_capacity(relation.key_pairs.len());
        for pair in &relation.key_pairs {
            key_pairs.push((
                self.resolve_field(source, &pair.source)?,
                self.resolve_field(&target, &pair.target)?,
            ));
        }
        Ok(RuntimeRelationHandle {
            schema_fingerprint: self.fingerprint(),
            source: source.clone(),
            id: relation.id.clone(),
            target,
            cardinality: relation.cardinality,
            key_pairs,
        })
    }

    /// Resolve a projection from already resolved field handles.
    ///
    /// # Errors
    ///
    /// Rejects stale handles, fields from another collection, duplicate
    /// members, unknown fields, and empty projections before any SQL executes.
    pub fn resolve_projection(
        &self,
        collection: &RuntimeCollectionHandle,
        fields: &[RuntimeFieldHandle],
    ) -> Result<RuntimeProjection, RuntimeRecordError> {
        let resolved_collection = self.checked_collection(collection)?;
        if fields.is_empty() {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::EmptyProjection)
                    .collection(&collection.id),
            );
        }
        let mut seen = BTreeSet::new();
        let mut resolved = Vec::with_capacity(fields.len());
        for field in fields {
            if field.schema_fingerprint != collection.schema_fingerprint {
                return Err(
                    RuntimeRecordError::new(RuntimeRecordErrorCode::SchemaMismatch)
                        .collection(&collection.id)
                        .field(&field.id),
                );
            }
            if field.collection != collection.id {
                return Err(
                    RuntimeRecordError::new(RuntimeRecordErrorCode::CrossCollectionField)
                        .collection(&collection.id)
                        .field(&field.id),
                );
            }
            if !seen.insert(field.id.clone()) {
                return Err(RuntimeRecordError::new(
                    RuntimeRecordErrorCode::DuplicateProjectionField,
                )
                .collection(&collection.id)
                .field(&field.id));
            }
            let current = self.resolve_field(collection, &field.id)?;
            if &current != field {
                return Err(
                    RuntimeRecordError::new(RuntimeRecordErrorCode::SchemaMismatch)
                        .collection(&collection.id)
                        .field(&field.id),
                );
            }
            resolved.push(current);
        }
        let collection_field_kinds = resolved_collection
            .fields
            .iter()
            .map(|field| (field.id.clone(), field.value_kind))
            .collect();
        Ok(RuntimeProjection {
            schema_fingerprint: collection.schema_fingerprint.clone(),
            collection: collection.clone(),
            fields: resolved,
            collection_field_kinds,
        })
    }

    /// Resolve a projection directly from stable collection and field IDs.
    ///
    /// # Errors
    ///
    /// Returns the same pre-execution structured errors as
    /// [`Self::resolve_collection`], [`Self::resolve_field`], and
    /// [`Self::resolve_projection`].
    pub fn resolve_projection_ids(
        &self,
        collection: &CollectionId,
        fields: &[FieldId],
    ) -> Result<RuntimeProjection, RuntimeRecordError> {
        let collection = self.resolve_collection(collection)?;
        let fields = fields
            .iter()
            .map(|field| self.resolve_field(&collection, field))
            .collect::<Result<Vec<_>, _>>()?;
        self.resolve_projection(&collection, &fields)
    }
}

/// Borrowed load state of one known runtime-record field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFieldState<'a> {
    /// The field belongs to the collection but was not selected.
    Unloaded,
    /// The field was selected and contained SQL `NULL`.
    Null,
    /// The field was selected and decoded.
    Value(&'a RuntimeValue),
}

/// Owned values for one runtime collection row.
///
/// The record retains all known field IDs and kinds for its collection, so an
/// absent value can be distinguished as unloaded rather than unknown. Values
/// are stored in stable-ID-ordered maps for deterministic equality and Serde.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeRecord {
    format_version: u32,
    schema_fingerprint: SchemaFingerprint,
    collection: CollectionId,
    field_kinds: BTreeMap<FieldId, RuntimeValueKind>,
    values: BTreeMap<FieldId, RuntimeValue>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeRecordWire {
    format_version: u32,
    schema_fingerprint: SchemaFingerprint,
    collection: CollectionId,
    field_kinds: BTreeMap<FieldId, RuntimeValueKind>,
    values: BTreeMap<FieldId, RuntimeValue>,
}

impl<'de> serde::Deserialize<'de> for RuntimeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = <RuntimeRecordWire as serde::Deserialize>::deserialize(deserializer)?;
        if wire.format_version != RUNTIME_RECORD_FORMAT_VERSION {
            return Err(serde::de::Error::custom(
                "unsupported runtime record format",
            ));
        }
        for (field, value) in &wire.values {
            let Some(expected) = wire.field_kinds.get(field) else {
                return Err(serde::de::Error::custom(
                    "runtime record value references an unknown field",
                ));
            };
            if value.kind().is_some_and(|kind| &kind != expected) {
                return Err(serde::de::Error::custom(
                    "runtime record value kind does not match field metadata",
                ));
            }
        }
        Ok(Self {
            format_version: wire.format_version,
            schema_fingerprint: wire.schema_fingerprint,
            collection: wire.collection,
            field_kinds: wire.field_kinds,
            values: wire.values,
        })
    }
}

impl RuntimeRecord {
    /// Deserialize and validate a deterministic JSON record representation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeRecordErrorCode::InvalidRecord`] for malformed JSON,
    /// unsupported record versions, unknown value fields, or value-kind
    /// inconsistencies. The Serde source is retained for trusted logging.
    pub fn from_json(input: &str) -> Result<Self, RuntimeRecordError> {
        serde_json::from_str(input).map_err(|error| {
            RuntimeRecordError::new(RuntimeRecordErrorCode::InvalidRecord).with_source(error)
        })
    }

    /// Serialize this record's deterministic, versioned JSON representation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeRecordErrorCode::InvalidRecord`] if serialization
    /// fails. Runtime record values are finite and owned, so this indicates an
    /// unexpected serializer failure rather than invalid database input.
    pub fn to_json(&self) -> Result<String, RuntimeRecordError> {
        serde_json::to_string(self).map_err(|error| {
            RuntimeRecordError::new(RuntimeRecordErrorCode::InvalidRecord).with_source(error)
        })
    }

    /// Serialized record format version.
    pub const fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Validated schema fingerprint used to decode this record.
    pub fn schema_fingerprint(&self) -> &SchemaFingerprint {
        &self.schema_fingerprint
    }

    /// Stable collection ID.
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection
    }

    /// Stable IDs of fields selected into this record.
    pub fn loaded_fields(&self) -> impl Iterator<Item = &FieldId> {
        self.values.keys()
    }

    fn checked_field<'a>(
        &'a self,
        field: &RuntimeFieldHandle,
    ) -> Result<Option<&'a RuntimeValue>, RuntimeRecordError> {
        if field.schema_fingerprint != self.schema_fingerprint {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::SchemaMismatch).for_field(field),
            );
        }
        if field.collection != self.collection {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::CrossCollectionField)
                    .for_field(field),
            );
        }
        let Some(kind) = self.field_kinds.get(&field.id) else {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::UnknownField).for_field(field),
            );
        };
        if *kind != field.value_kind {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::SchemaMismatch).for_field(field),
            );
        }
        Ok(self.values.get(&field.id))
    }

    /// Inspect selected-null versus unloaded state for a known field.
    ///
    /// # Errors
    ///
    /// Rejects handles from a different schema or collection and fields not
    /// known by this record's collection metadata.
    pub fn state<'a>(
        &'a self,
        field: &RuntimeFieldHandle,
    ) -> Result<RuntimeFieldState<'a>, RuntimeRecordError> {
        match self.checked_field(field)? {
            None => Ok(RuntimeFieldState::Unloaded),
            Some(RuntimeValue::Null) => Ok(RuntimeFieldState::Null),
            Some(value) => Ok(RuntimeFieldState::Value(value)),
        }
    }

    /// Read a selected, non-null runtime value.
    ///
    /// # Errors
    ///
    /// Returns structured schema/collection/field errors, `field_unloaded`, or
    /// `null_value` without exposing a physical identifier or raw value.
    pub fn value(&self, field: &RuntimeFieldHandle) -> Result<&RuntimeValue, RuntimeRecordError> {
        match self.checked_field(field)? {
            None => {
                Err(RuntimeRecordError::new(RuntimeRecordErrorCode::FieldUnloaded).for_field(field))
            }
            Some(RuntimeValue::Null) => {
                Err(RuntimeRecordError::new(RuntimeRecordErrorCode::NullValue).for_field(field))
            }
            Some(value) => Ok(value),
        }
    }

    fn typed_value(
        &self,
        field: &RuntimeFieldHandle,
        expected: RuntimeValueKind,
    ) -> Result<&RuntimeValue, RuntimeRecordError> {
        let value = self.value(field)?;
        if value.kind() != Some(expected) {
            return Err(
                RuntimeRecordError::new(RuntimeRecordErrorCode::WrongValueKind).for_field(field),
            );
        }
        Ok(value)
    }

    /// Read a selected non-null boolean.
    pub fn boolean(&self, field: &RuntimeFieldHandle) -> Result<bool, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::Boolean)? {
            RuntimeValue::Boolean(value) => Ok(*value),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read a selected non-null signed integer.
    pub fn integer(&self, field: &RuntimeFieldHandle) -> Result<i64, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::Integer)? {
            RuntimeValue::Integer(value) => Ok(*value),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read a selected non-null finite float.
    pub fn float(&self, field: &RuntimeFieldHandle) -> Result<f64, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::Float)? {
            RuntimeValue::Float(value) => Ok(value.get()),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read a selected non-null Unicode string.
    pub fn string(&self, field: &RuntimeFieldHandle) -> Result<&str, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::String)? {
            RuntimeValue::String(value) => Ok(value),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read a selected non-null UUID.
    pub fn uuid(&self, field: &RuntimeFieldHandle) -> Result<uuid::Uuid, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::Uuid)? {
            RuntimeValue::Uuid(value) => Ok(*value),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read a selected non-null JSON value.
    pub fn json(
        &self,
        field: &RuntimeFieldHandle,
    ) -> Result<&serde_json::Value, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::Json)? {
            RuntimeValue::Json(value) => Ok(value),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read selected non-null bytes.
    pub fn bytes(&self, field: &RuntimeFieldHandle) -> Result<&[u8], RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::Bytes)? {
            RuntimeValue::Bytes(value) => Ok(value),
            _ => unreachable!("kind checked above"),
        }
    }

    /// Read a selected non-null canonical date-time.
    pub fn datetime(
        &self,
        field: &RuntimeFieldHandle,
    ) -> Result<&RuntimeDateTime, RuntimeRecordError> {
        match self.typed_value(field, RuntimeValueKind::DateTime)? {
            RuntimeValue::DateTime(value) => Ok(value),
            _ => unreachable!("kind checked above"),
        }
    }
}

/// Additive backend capability for projection-aware runtime row decoding.
///
/// This trait intentionally does not extend [`OrmBackend`], preserving source
/// compatibility for third-party static backends. Its default implementation
/// fails closed; backend authors must decode by [`RuntimeFieldHandle`] kind and
/// return owned values without coercing incompatible database types.
pub trait RuntimeRowDecoder: OrmBackend {
    /// Whether this backend implements the complete runtime-row contract.
    const RUNTIME_ROW_DECODING_SUPPORTED: bool = false;

    /// Decode one selected field from one backend row.
    ///
    /// # Errors
    ///
    /// The default returns [`RuntimeRecordErrorCode::UnsupportedBackend`].
    fn decode_runtime_value(
        _row: &Self::Row,
        field: &RuntimeFieldHandle,
    ) -> Result<RuntimeValue, RuntimeRecordError> {
        Err(RuntimeRecordError::new(RuntimeRecordErrorCode::UnsupportedBackend).for_field(field))
    }
}

impl RuntimeRowDecoder for NoDefaultBackend {}

#[cfg(feature = "mssql")]
impl RuntimeRowDecoder for super::MssqlBackend {}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn map_sqlx_decode_error(field: &RuntimeFieldHandle, error: sqlx::Error) -> RuntimeRecordError {
    let code = if matches!(error, sqlx::Error::ColumnNotFound(_)) {
        RuntimeRecordErrorCode::MissingColumn
    } else {
        RuntimeRecordErrorCode::BackendTypeMismatch
    };
    RuntimeRecordError::new(code)
        .for_field(field)
        .with_source(error)
}

#[cfg(feature = "sqlite")]
fn sqlite_get_optional<T>(
    row: &sqlx::sqlite::SqliteRow,
    field: &RuntimeFieldHandle,
) -> Result<Option<T>, RuntimeRecordError>
where
    for<'row> T: sqlx::Decode<'row, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    use sqlx::Row;
    row.try_get::<Option<T>, _>(field.physical_column.as_str())
        .map_err(|error| map_sqlx_decode_error(field, error))
}

#[cfg(feature = "sqlite")]
impl RuntimeRowDecoder for super::SqliteBackend {
    const RUNTIME_ROW_DECODING_SUPPORTED: bool = true;

    fn decode_runtime_value(
        row: &Self::Row,
        field: &RuntimeFieldHandle,
    ) -> Result<RuntimeValue, RuntimeRecordError> {
        match field.value_kind {
            RuntimeValueKind::Boolean => Ok(sqlite_get_optional::<bool>(row, field)?
                .map(RuntimeValue::Boolean)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Integer => Ok(sqlite_get_optional::<i64>(row, field)?
                .map(RuntimeValue::Integer)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Float => match sqlite_get_optional::<f64>(row, field)? {
                Some(value) => RuntimeFloat::new(value)
                    .map(RuntimeValue::Float)
                    .map_err(|error| error.for_field(field)),
                None => Ok(RuntimeValue::Null),
            },
            RuntimeValueKind::String => Ok(sqlite_get_optional::<String>(row, field)?
                .map(RuntimeValue::String)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Uuid => match sqlite_get_optional::<String>(row, field)? {
                Some(value) => uuid::Uuid::parse_str(&value)
                    .map(RuntimeValue::Uuid)
                    .map_err(|error| {
                        RuntimeRecordError::new(RuntimeRecordErrorCode::InvalidValue)
                            .for_field(field)
                            .with_source(error)
                    }),
                None => Ok(RuntimeValue::Null),
            },
            RuntimeValueKind::Json => match sqlite_get_optional::<String>(row, field)? {
                Some(value) => serde_json::from_str(&value)
                    .map(RuntimeValue::Json)
                    .map_err(|error| {
                        RuntimeRecordError::new(RuntimeRecordErrorCode::InvalidValue)
                            .for_field(field)
                            .with_source(error)
                    }),
                None => Ok(RuntimeValue::Null),
            },
            RuntimeValueKind::Bytes => Ok(sqlite_get_optional::<Vec<u8>>(row, field)?
                .map(RuntimeValue::Bytes)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::DateTime => match sqlite_get_optional::<String>(row, field)? {
                Some(value) => RuntimeDateTime::parse(&value)
                    .map(RuntimeValue::DateTime)
                    .map_err(|error| error.for_field(field)),
                None => Ok(RuntimeValue::Null),
            },
        }
    }
}

#[cfg(feature = "postgres")]
fn postgres_get_optional<T>(
    row: &sqlx::postgres::PgRow,
    field: &RuntimeFieldHandle,
) -> Result<Option<T>, RuntimeRecordError>
where
    for<'row> T: sqlx::Decode<'row, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    use sqlx::Row;
    row.try_get::<Option<T>, _>(field.physical_column.as_str())
        .map_err(|error| map_sqlx_decode_error(field, error))
}

#[cfg(feature = "postgres")]
impl RuntimeRowDecoder for super::PostgresBackend {
    const RUNTIME_ROW_DECODING_SUPPORTED: bool = true;

    fn decode_runtime_value(
        row: &Self::Row,
        field: &RuntimeFieldHandle,
    ) -> Result<RuntimeValue, RuntimeRecordError> {
        match field.value_kind {
            RuntimeValueKind::Boolean => Ok(postgres_get_optional::<bool>(row, field)?
                .map(RuntimeValue::Boolean)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Integer => Ok(postgres_get_optional::<i64>(row, field)?
                .map(RuntimeValue::Integer)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Float => match postgres_get_optional::<f64>(row, field)? {
                Some(value) => RuntimeFloat::new(value)
                    .map(RuntimeValue::Float)
                    .map_err(|error| error.for_field(field)),
                None => Ok(RuntimeValue::Null),
            },
            RuntimeValueKind::String => Ok(postgres_get_optional::<String>(row, field)?
                .map(RuntimeValue::String)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Uuid => Ok(postgres_get_optional::<uuid::Uuid>(row, field)?
                .map(RuntimeValue::Uuid)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Json => Ok(postgres_get_optional::<
                sqlx::types::Json<serde_json::Value>,
            >(row, field)?
            .map(|value| RuntimeValue::Json(value.0))
            .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::Bytes => Ok(postgres_get_optional::<Vec<u8>>(row, field)?
                .map(RuntimeValue::Bytes)
                .unwrap_or(RuntimeValue::Null)),
            RuntimeValueKind::DateTime => {
                Ok(postgres_get_optional::<chrono::DateTime<Utc>>(row, field)?
                    .map(RuntimeDateTime::from_utc)
                    .map(RuntimeValue::DateTime)
                    .unwrap_or(RuntimeValue::Null))
            }
        }
    }
}
