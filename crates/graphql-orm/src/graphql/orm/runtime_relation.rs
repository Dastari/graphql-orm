//! Validated, batched runtime relation reads.
//!
//! Relation source keys are selected only into an opaque parent anchor. They
//! are never marked loaded in the caller-visible [`RuntimeRecord`]. Requests
//! are schema-fingerprint-bound, bounded before rendering, and execute one SQL
//! statement per compatible relation page shape rather than one per parent.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;
use std::fmt;

use super::runtime_query::{
    cursor_after_sql, effective_terms, placeholder, render_order_terms, value_bind,
};
use super::{
    CollectionId, DatabaseBackend, DbAuthContext, FieldId, OrmBackend, RelationCardinality,
    RelationId, RuntimeCollectionHandle, RuntimeEdge, RuntimeFieldHandle, RuntimeFieldState,
    RuntimeOrder, RuntimePageInfo, RuntimePageRequest, RuntimePredicate, RuntimeProjection,
    RuntimeQueryError, RuntimeQueryLimits, RuntimeReadRequest, RuntimeRecord, RuntimeRecordError,
    RuntimeRowDecoder, RuntimeValue, RuntimeValueKind, SchemaFingerprint, SqlDialect, SqlValue,
    ValidatedRuntimeSchema,
};

const RELATION_CURSOR_PREFIX: &str = "gormrr1.";
const GROUP_ALIAS: &str = "__graphql_orm_relation_group";
const ROW_ALIAS: &str = "__graphql_orm_relation_row";
const COUNT_ALIAS: &str = "__graphql_orm_relation_count";

/// Stable category for runtime relation validation and execution failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRelationErrorCode {
    InvalidRelation,
    InvalidRequest,
    InvalidParent,
    SchemaMismatch,
    ResourceLimit,
    UnsupportedKey,
    UnsupportedBackend,
    CursorInvalid,
    CursorMismatch,
    CardinalityViolation,
    Decode,
    BackendExecution,
}

impl RuntimeRelationErrorCode {
    /// Stable machine-readable string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRelation => "invalid_relation",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidParent => "invalid_parent",
            Self::SchemaMismatch => "schema_mismatch",
            Self::ResourceLimit => "resource_limit",
            Self::UnsupportedKey => "unsupported_key",
            Self::UnsupportedBackend => "unsupported_backend",
            Self::CursorInvalid => "cursor_invalid",
            Self::CursorMismatch => "cursor_mismatch",
            Self::CardinalityViolation => "cardinality_violation",
            Self::Decode => "decode",
            Self::BackendExecution => "backend_execution",
        }
    }
}

/// Safe runtime relation error. SQL, identifiers, key values, cursors, and rows are redacted.
pub struct RuntimeRelationError {
    code: RuntimeRelationErrorCode,
    relation: Option<RelationId>,
    collection: Option<CollectionId>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RuntimeRelationError {
    fn new(code: RuntimeRelationErrorCode) -> Self {
        Self {
            code,
            relation: None,
            collection: None,
            source: None,
        }
    }

    fn for_relation(mut self, relation: &super::RuntimeRelationHandle) -> Self {
        self.relation = Some(relation.id().clone());
        self.collection = Some(relation.source().id().clone());
        self
    }

    fn source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Stable machine-usable category.
    pub const fn code(&self) -> RuntimeRelationErrorCode {
        self.code
    }

    /// Stable relation ID, when validation reached a relation.
    pub fn relation_id(&self) -> Option<&RelationId> {
        self.relation.as_ref()
    }

    /// Stable source collection ID, when known.
    pub fn collection_id(&self) -> Option<&CollectionId> {
        self.collection.as_ref()
    }
}

impl fmt::Debug for RuntimeRelationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRelationError")
            .field("code", &self.code)
            .field("relation", &self.relation)
            .field("collection", &self.collection)
            .field("source", &self.source.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl fmt::Display for RuntimeRelationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "runtime relation error: {}", self.code.as_str())
    }
}

impl StdError for RuntimeRelationError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

impl From<RuntimeRecordError> for RuntimeRelationError {
    fn from(error: RuntimeRecordError) -> Self {
        Self::new(RuntimeRelationErrorCode::Decode).source(error)
    }
}

impl From<RuntimeQueryError> for RuntimeRelationError {
    fn from(error: RuntimeQueryError) -> Self {
        Self::new(RuntimeRelationErrorCode::InvalidRequest).source(error)
    }
}

/// Independent hard bounds for one runtime relation layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeRelationLimits {
    pub max_parents: usize,
    pub max_key_arity: usize,
    pub max_page_size: u32,
    pub max_bind_parameters: usize,
    pub max_cursor_bytes: usize,
    pub max_compatible_groups: usize,
}

impl Default for RuntimeRelationLimits {
    fn default() -> Self {
        Self {
            max_parents: 100,
            max_key_arity: 8,
            max_page_size: 100,
            max_bind_parameters: 32_000,
            max_cursor_bytes: 16 * 1024,
            max_compatible_groups: 100,
        }
    }
}

#[derive(Clone)]
struct AnchorSpec {
    relation: super::RuntimeRelationHandle,
    parent_identity_fields: Vec<RuntimeFieldHandle>,
}

/// Validated parent read that captures requested relation source keys privately.
#[derive(Clone)]
pub struct RuntimeAnchoredReadRequest {
    inner: RuntimeReadRequest,
    output_projection: RuntimeProjection,
    anchors: Vec<AnchorSpec>,
}

impl fmt::Debug for RuntimeAnchoredReadRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeAnchoredReadRequest")
            .field("read", &self.inner)
            .field("relation_count", &self.anchors.len())
            .finish()
    }
}

/// Opaque, owned proof of one parent and one relation's typed source key.
///
/// The anchor is deliberately not serializable and its `Debug` output never
/// includes source or parent key values.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeParentAnchor {
    schema: SchemaFingerprint,
    relation: RelationId,
    source: CollectionId,
    target: CollectionId,
    parent_index: usize,
    parent_identity: Vec<RuntimeValue>,
    relation_key: Option<Vec<RuntimeValue>>,
}

impl RuntimeParentAnchor {
    /// Position of the parent in the anchored read result.
    pub const fn parent_index(&self) -> usize {
        self.parent_index
    }

    /// Stable relation ID bound to this anchor.
    pub fn relation_id(&self) -> &RelationId {
        &self.relation
    }

    /// Whether at least one nullable source-key component was SQL `NULL`.
    pub const fn is_null_key(&self) -> bool {
        self.relation_key.is_none()
    }
}

impl fmt::Debug for RuntimeParentAnchor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeParentAnchor")
            .field("schema", &self.schema)
            .field("relation", &self.relation)
            .field("source", &self.source)
            .field("target", &self.target)
            .field("parent_index", &self.parent_index)
            .field("parent_identity", &"[redacted]")
            .field("relation_key", &"[redacted]")
            .finish()
    }
}

/// One caller-visible parent edge plus opaque relation anchors.
#[derive(Clone, Debug)]
pub struct RuntimeAnchoredEdge {
    pub node: RuntimeRecord,
    pub cursor: String,
    anchors: Vec<RuntimeParentAnchor>,
}

impl RuntimeAnchoredEdge {
    /// Return the anchor for a requested relation.
    pub fn relation_anchor(
        &self,
        relation: &super::RuntimeRelationHandle,
    ) -> Option<&RuntimeParentAnchor> {
        self.anchors.iter().find(|anchor| {
            anchor.schema == *relation.schema_fingerprint() && anchor.relation == *relation.id()
        })
    }
}

/// Bounded parent connection whose hidden relation keys remain opaque.
#[derive(Clone, Debug)]
pub struct RuntimeAnchoredConnection {
    pub edges: Vec<RuntimeAnchoredEdge>,
    pub page_info: RuntimePageInfo,
    pub total_count: Option<i64>,
}

impl RuntimeAnchoredConnection {
    /// Clone all anchors for one requested relation in parent result order.
    ///
    /// # Errors
    ///
    /// Returns `invalid_relation` when the relation was not requested for any
    /// nonempty parent connection.
    pub fn relation_parents(
        &self,
        relation: &super::RuntimeRelationHandle,
    ) -> Result<Vec<RuntimeParentAnchor>, RuntimeRelationError> {
        let anchors = self
            .edges
            .iter()
            .filter_map(|edge| edge.relation_anchor(relation).cloned())
            .collect::<Vec<_>>();
        if !self.edges.is_empty() && anchors.len() != self.edges.len() {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidRelation)
                    .for_relation(relation),
            );
        }
        Ok(anchors)
    }
}

/// Cardinality-specific bounded relation selection.
#[derive(Clone, Debug)]
pub enum RuntimeRelationSelection {
    /// Resolve zero or one row for every parent.
    ToOne,
    /// Resolve one independently bounded keyset connection per parent.
    ToMany {
        /// Exactly one page request per parent anchor, in the same order.
        pages: Vec<RuntimePageRequest>,
        /// Execute exact per-parent counts with the same key and predicate.
        include_count: bool,
    },
}

/// Fully validated runtime relation batch request.
#[derive(Clone)]
pub struct RuntimeRelationBatchRequest {
    relation: super::RuntimeRelationHandle,
    anchors: Vec<RuntimeParentAnchor>,
    output_projection: RuntimeProjection,
    decode_projection: RuntimeProjection,
    predicate: Option<RuntimePredicate>,
    order: RuntimeOrder,
    selection: RuntimeRelationSelection,
    limits: RuntimeRelationLimits,
}

impl fmt::Debug for RuntimeRelationBatchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRelationBatchRequest")
            .field("relation", self.relation.id())
            .field("parent_count", &self.anchors.len())
            .field("predicate", &self.predicate.as_ref().map(|_| "[redacted]"))
            .field("order_terms", &self.order.terms().len())
            .finish()
    }
}

/// One parent-associated relation value.
#[derive(Clone, Debug)]
pub enum RuntimeRelationValue {
    ToOne(Option<RuntimeRecord>),
    ToMany(super::RuntimeConnection),
}

/// Deterministic result for one input parent.
#[derive(Clone, Debug)]
pub struct RuntimeRelationResult {
    pub parent_index: usize,
    pub value: RuntimeRelationValue,
}

/// Complete relation layer in input-parent order.
#[derive(Clone, Debug)]
pub struct RuntimeRelationBatch {
    pub results: Vec<RuntimeRelationResult>,
}

fn supported_key_kind(kind: RuntimeValueKind) -> bool {
    matches!(
        kind,
        RuntimeValueKind::Boolean
            | RuntimeValueKind::Integer
            | RuntimeValueKind::String
            | RuntimeValueKind::Uuid
            | RuntimeValueKind::Bytes
            | RuntimeValueKind::DateTime
    )
}

fn record_values(
    record: &RuntimeRecord,
    fields: &[RuntimeFieldHandle],
) -> Result<(Vec<RuntimeValue>, bool), RuntimeRelationError> {
    let mut values = Vec::with_capacity(fields.len());
    let mut any_null = false;
    for field in fields {
        match record.state(field)? {
            RuntimeFieldState::Value(value) => values.push(value.clone()),
            RuntimeFieldState::Null => {
                values.push(RuntimeValue::Null);
                any_null = true;
            }
            RuntimeFieldState::Unloaded => {
                return Err(RuntimeRelationError::new(
                    RuntimeRelationErrorCode::InvalidParent,
                ));
            }
        }
    }
    Ok((values, any_null))
}

impl ValidatedRuntimeSchema {
    /// Build a normal runtime read plus opaque source-key capture for relations.
    ///
    /// # Errors
    ///
    /// Rejects stale/cross-collection/duplicate relations and all errors of
    /// [`ValidatedRuntimeSchema::runtime_read_request`] before database I/O.
    #[allow(clippy::too_many_arguments)]
    pub fn runtime_read_request_with_relation_keys(
        &self,
        collection: &RuntimeCollectionHandle,
        projection: &RuntimeProjection,
        predicate: Option<RuntimePredicate>,
        order: RuntimeOrder,
        page: RuntimePageRequest,
        include_total_count: bool,
        relations: &[super::RuntimeRelationHandle],
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimeAnchoredReadRequest, RuntimeRelationError> {
        let metadata = self
            .schema()
            .collections
            .iter()
            .find(|item| item.id == *collection.id())
            .ok_or_else(|| RuntimeRelationError::new(RuntimeRelationErrorCode::SchemaMismatch))?;
        let parent_identity_fields = metadata
            .primary_key
            .iter()
            .map(|id| self.resolve_field(collection, id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut union = projection.fields().to_vec();
        let mut seen = union
            .iter()
            .map(|field| field.id().clone())
            .collect::<BTreeSet<_>>();
        let mut relation_ids = BTreeSet::new();
        let mut anchor_specs = Vec::with_capacity(relations.len());
        for relation in relations {
            let current = self.resolve_relation(collection, relation.id())?;
            if current != *relation
                || relation.source().id() != collection.id()
                || !relation_ids.insert(relation.id().clone())
            {
                return Err(
                    RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidRelation)
                        .for_relation(relation),
                );
            }
            for (source, _) in relation.key_pairs() {
                if seen.insert(source.id().clone()) {
                    union.push(source.clone());
                }
            }
            for field in &parent_identity_fields {
                if seen.insert(field.id().clone()) {
                    union.push(field.clone());
                }
            }
            anchor_specs.push(AnchorSpec {
                relation: relation.clone(),
                parent_identity_fields: parent_identity_fields.clone(),
            });
        }
        let decode_projection = self.resolve_projection(collection, &union)?;
        let inner = self.runtime_read_request(
            collection,
            &decode_projection,
            predicate,
            order,
            page,
            include_total_count,
            limits,
        )?;
        Ok(RuntimeAnchoredReadRequest {
            inner,
            output_projection: projection.clone(),
            anchors: anchor_specs,
        })
    }

    /// Validate one batched relation layer.
    ///
    /// # Errors
    ///
    /// Rejects stale handles/anchors, unsupported keys, duplicate parents,
    /// cardinality/page mismatches, and all configured bounds before I/O.
    #[allow(clippy::too_many_arguments)]
    pub fn runtime_relation_batch_request(
        &self,
        relation: &super::RuntimeRelationHandle,
        anchors: Vec<RuntimeParentAnchor>,
        target_projection: &RuntimeProjection,
        target_predicate: Option<RuntimePredicate>,
        target_order: RuntimeOrder,
        selection: RuntimeRelationSelection,
        limits: RuntimeRelationLimits,
    ) -> Result<RuntimeRelationBatchRequest, RuntimeRelationError> {
        let current = self.resolve_relation(relation.source(), relation.id())?;
        if current != *relation {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidRelation)
                    .for_relation(relation),
            );
        }
        if anchors.len() > limits.max_parents
            || relation.key_pairs().is_empty()
            || relation.key_pairs().len() > limits.max_key_arity
        {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::ResourceLimit)
                    .for_relation(relation),
            );
        }
        if target_projection.schema_fingerprint() != &self.fingerprint()
            || target_projection.collection().id() != relation.target().id()
            || !target_order.belongs_to(&self.fingerprint(), relation.target().id())
            || target_predicate.as_ref().is_some_and(|predicate| {
                !predicate.belongs_to(&self.fingerprint(), relation.target().id())
            })
        {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::SchemaMismatch)
                    .for_relation(relation),
            );
        }
        if relation.key_pairs().iter().any(|(source, target)| {
            source.value_kind() != target.value_kind() || !supported_key_kind(source.value_kind())
        }) {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::UnsupportedKey)
                    .for_relation(relation),
            );
        }
        match (&selection, relation.cardinality()) {
            (RuntimeRelationSelection::ToOne, RelationCardinality::One) => {}
            (RuntimeRelationSelection::ToMany { pages, .. }, RelationCardinality::Many)
                if pages.len() == anchors.len() =>
            {
                for page in pages {
                    let (size, cursor) = match page {
                        RuntimePageRequest::First { size, after } => (*size, after),
                        RuntimePageRequest::Last { size, before } => (*size, before),
                    };
                    if size <= 0
                        || u32::try_from(size)
                            .ok()
                            .is_none_or(|size| size > limits.max_page_size)
                        || cursor.as_ref().is_some_and(|cursor| {
                            cursor.len()
                                > limits
                                    .max_cursor_bytes
                                    .saturating_mul(2)
                                    .saturating_add(RELATION_CURSOR_PREFIX.len())
                        })
                    {
                        return Err(RuntimeRelationError::new(
                            RuntimeRelationErrorCode::ResourceLimit,
                        )
                        .for_relation(relation));
                    }
                }
            }
            _ => {
                return Err(
                    RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidRequest)
                        .for_relation(relation),
                );
            }
        }
        let mut parent_ids = BTreeSet::new();
        for anchor in &anchors {
            if anchor.schema != self.fingerprint()
                || anchor.relation != *relation.id()
                || anchor.source != *relation.source().id()
                || anchor.target != *relation.target().id()
            {
                return Err(
                    RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidParent)
                        .for_relation(relation),
                );
            }
            let identity = serde_json::to_vec(&anchor.parent_identity).map_err(|error| {
                RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidParent).source(error)
            })?;
            if !parent_ids.insert(identity) {
                return Err(
                    RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidParent)
                        .for_relation(relation),
                );
            }
        }
        let mut union = target_projection.fields().to_vec();
        let mut seen = union
            .iter()
            .map(|field| field.id().clone())
            .collect::<BTreeSet<FieldId>>();
        for (_, target) in relation.key_pairs() {
            if seen.insert(target.id().clone()) {
                union.push(target.clone());
            }
        }
        for term in target_order.terms() {
            if seen.insert(term.field().id().clone()) {
                union.push(term.field().clone());
            }
        }
        let decode_projection = self.resolve_projection(relation.target(), &union)?;
        Ok(RuntimeRelationBatchRequest {
            relation: relation.clone(),
            anchors,
            output_projection: target_projection.clone(),
            decode_projection,
            predicate: target_predicate,
            order: target_order,
            selection,
            limits,
        })
    }
}

impl<B> crate::db::Database<B>
where
    B: OrmBackend + RuntimeRowDecoder,
{
    /// Execute a parent read while retaining requested relation keys only in opaque anchors.
    ///
    /// # Errors
    ///
    /// Returns safe runtime query/decode errors without exposing hidden key values.
    pub async fn execute_runtime_anchored_read(
        &self,
        request: &RuntimeAnchoredReadRequest,
        auth: Option<&DbAuthContext>,
    ) -> Result<RuntimeAnchoredConnection, RuntimeRelationError> {
        let connection = self.execute_runtime_read(&request.inner, auth).await?;
        let mut edges = Vec::with_capacity(connection.edges.len());
        for (parent_index, edge) in connection.edges.into_iter().enumerate() {
            let mut anchors = Vec::with_capacity(request.anchors.len());
            for spec in &request.anchors {
                let (parent_identity, parent_null) =
                    record_values(&edge.node, &spec.parent_identity_fields)?;
                if parent_null {
                    return Err(
                        RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidParent)
                            .for_relation(&spec.relation),
                    );
                }
                let source_fields = spec
                    .relation
                    .key_pairs()
                    .iter()
                    .map(|(source, _)| source.clone())
                    .collect::<Vec<_>>();
                let (relation_values, any_null) = record_values(&edge.node, &source_fields)?;
                anchors.push(RuntimeParentAnchor {
                    schema: spec.relation.schema_fingerprint().clone(),
                    relation: spec.relation.id().clone(),
                    source: spec.relation.source().id().clone(),
                    target: spec.relation.target().id().clone(),
                    parent_index,
                    parent_identity,
                    relation_key: (!any_null).then_some(relation_values),
                });
            }
            let node = edge.node.project(&request.output_projection)?;
            edges.push(RuntimeAnchoredEdge {
                node,
                cursor: edge.cursor,
                anchors,
            });
        }
        Ok(RuntimeAnchoredConnection {
            edges,
            page_info: connection.page_info,
            total_count: connection.total_count,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RelationCursorEnvelope {
    version: u8,
    schema: SchemaFingerprint,
    relation: RelationId,
    parent: Vec<RuntimeValue>,
    target: CollectionId,
    order: Vec<(
        FieldId,
        super::RuntimeOrderDirection,
        super::RuntimeNullPlacement,
    )>,
    values: Vec<RuntimeValue>,
    checksum: String,
}

fn checksum(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn relation_order_signature(
    request: &RuntimeRelationBatchRequest,
) -> Vec<(
    FieldId,
    super::RuntimeOrderDirection,
    super::RuntimeNullPlacement,
)> {
    request
        .order
        .terms()
        .iter()
        .map(|term| (term.field().id().clone(), term.direction(), term.nulls()))
        .collect()
}

fn encode_relation_cursor(
    request: &RuntimeRelationBatchRequest,
    anchor: &RuntimeParentAnchor,
    values: Vec<RuntimeValue>,
) -> Result<String, RuntimeRelationError> {
    let order = relation_order_signature(request);
    let payload = serde_json::to_vec(&(
        1u8,
        request.relation.schema_fingerprint(),
        request.relation.id(),
        &anchor.parent_identity,
        request.relation.target().id(),
        &order,
        &values,
    ))
    .map_err(|error| {
        RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid).source(error)
    })?;
    let envelope = RelationCursorEnvelope {
        version: 1,
        schema: request.relation.schema_fingerprint().clone(),
        relation: request.relation.id().clone(),
        parent: anchor.parent_identity.clone(),
        target: request.relation.target().id().clone(),
        order,
        values,
        checksum: checksum(&payload),
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|error| {
        RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid).source(error)
    })?;
    if bytes.len() > request.limits.max_cursor_bytes {
        return Err(
            RuntimeRelationError::new(RuntimeRelationErrorCode::ResourceLimit)
                .for_relation(&request.relation),
        );
    }
    Ok(format!("{RELATION_CURSOR_PREFIX}{}", hex(&bytes)))
}

fn decode_relation_cursor(
    request: &RuntimeRelationBatchRequest,
    anchor: &RuntimeParentAnchor,
    cursor: &str,
) -> Result<Vec<RuntimeValue>, RuntimeRelationError> {
    let bytes = cursor
        .strip_prefix(RELATION_CURSOR_PREFIX)
        .and_then(unhex)
        .filter(|bytes| bytes.len() <= request.limits.max_cursor_bytes)
        .ok_or_else(|| {
            RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid)
                .for_relation(&request.relation)
        })?;
    let envelope: RelationCursorEnvelope = serde_json::from_slice(&bytes).map_err(|_| {
        RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid)
            .for_relation(&request.relation)
    })?;
    let payload = serde_json::to_vec(&(
        envelope.version,
        &envelope.schema,
        &envelope.relation,
        &envelope.parent,
        &envelope.target,
        &envelope.order,
        &envelope.values,
    ))
    .map_err(|error| {
        RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid).source(error)
    })?;
    if envelope.version != 1 || envelope.checksum != checksum(&payload) {
        return Err(
            RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid)
                .for_relation(&request.relation),
        );
    }
    if envelope.schema != *request.relation.schema_fingerprint()
        || envelope.relation != *request.relation.id()
        || envelope.parent != anchor.parent_identity
        || envelope.target != *request.relation.target().id()
        || envelope.order != relation_order_signature(request)
    {
        return Err(
            RuntimeRelationError::new(RuntimeRelationErrorCode::CursorMismatch)
                .for_relation(&request.relation),
        );
    }
    if envelope.values.len() != request.order.terms().len()
        || envelope
            .values
            .iter()
            .zip(request.order.terms())
            .any(|(value, term)| {
                (matches!(value, RuntimeValue::Null) && !term.field().nullable())
                    || (!matches!(value, RuntimeValue::Null)
                        && value.kind() != Some(term.field().value_kind()))
            })
    {
        return Err(
            RuntimeRelationError::new(RuntimeRelationErrorCode::CursorInvalid)
                .for_relation(&request.relation),
        );
    }
    Ok(envelope.values)
}

fn bind_key(
    backend: DatabaseBackend,
    target_fields: &[RuntimeFieldHandle],
    key: &[RuntimeValue],
    values: &mut Vec<SqlValue>,
) -> Result<String, RuntimeRelationError> {
    let mut clauses = Vec::with_capacity(key.len());
    for (field, value) in target_fields.iter().zip(key) {
        let bind = placeholder(backend, values.len() + 1, field.value_kind());
        let value = value_bind(value)
            .ok_or_else(|| RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidParent))?;
        values.push(value);
        clauses.push(format!(
            "{} = {bind}",
            backend.quote_identifier(field.physical_column())
        ));
    }
    Ok(clauses.join(" AND "))
}

#[derive(Clone)]
struct QueryGroup {
    id: usize,
    anchor_indexes: Vec<usize>,
    key: Vec<RuntimeValue>,
    page: Option<RuntimePageRequest>,
}

fn group_requests(
    request: &RuntimeRelationBatchRequest,
) -> Result<Vec<QueryGroup>, RuntimeRelationError> {
    let pages = match &request.selection {
        RuntimeRelationSelection::ToOne => None,
        RuntimeRelationSelection::ToMany { pages, .. } => Some(pages),
    };
    let mut grouped = BTreeMap::<Vec<u8>, QueryGroup>::new();
    for (index, anchor) in request.anchors.iter().enumerate() {
        let Some(key) = &anchor.relation_key else {
            continue;
        };
        let page = pages.map(|pages| pages[index].clone());
        let page_key = match &page {
            None => (0u8, 0i64, None),
            Some(RuntimePageRequest::First { size, after }) => (1, *size, after.clone()),
            Some(RuntimePageRequest::Last { size, before }) => (2, *size, before.clone()),
        };
        let signature = serde_json::to_vec(&(key, page_key)).map_err(|error| {
            RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidRequest).source(error)
        })?;
        let next_id = grouped.len();
        grouped
            .entry(signature)
            .and_modify(|group| group.anchor_indexes.push(index))
            .or_insert_with(|| QueryGroup {
                id: next_id,
                anchor_indexes: vec![index],
                key: key.clone(),
                page,
            });
    }
    if grouped.len() > request.limits.max_compatible_groups {
        return Err(
            RuntimeRelationError::new(RuntimeRelationErrorCode::ResourceLimit)
                .for_relation(&request.relation),
        );
    }
    Ok(grouped.into_values().collect())
}

fn render_relation_rows(
    request: &RuntimeRelationBatchRequest,
    backend: DatabaseBackend,
    groups: &[QueryGroup],
) -> Result<(String, Vec<SqlValue>), RuntimeRelationError> {
    let target_fields = request
        .relation
        .key_pairs()
        .iter()
        .map(|(_, target)| target.clone())
        .collect::<Vec<_>>();
    let columns = request
        .decode_projection
        .fields()
        .iter()
        .map(|field| backend.quote_identifier(field.physical_column()))
        .collect::<Vec<_>>()
        .join(", ");
    let table = backend.quote_identifier(request.relation.target().physical_table());
    let group_alias = backend.quote_identifier(GROUP_ALIAS);
    let row_alias = backend.quote_identifier(ROW_ALIAS);
    let mut values = Vec::new();
    let mut branches = Vec::new();
    for group in groups {
        let mut predicates = vec![bind_key(backend, &target_fields, &group.key, &mut values)?];
        if let Some(predicate) = &request.predicate {
            predicates.push(predicate.render(backend, &mut values));
        }
        let (limit, backward, cursor) = match &group.page {
            None => (2, false, None),
            Some(RuntimePageRequest::First { size, after }) => (*size + 1, false, after.as_ref()),
            Some(RuntimePageRequest::Last { size, before }) => (*size + 1, true, before.as_ref()),
        };
        let terms = effective_terms(&request.order, backward);
        if let Some(cursor) = cursor {
            let anchor = &request.anchors[group.anchor_indexes[0]];
            let cursor_values = decode_relation_cursor(request, anchor, cursor)?;
            predicates.push(cursor_after_sql(
                backend,
                &terms,
                &cursor_values,
                &mut values,
            ));
        }
        if values.len() > request.limits.max_bind_parameters {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::ResourceLimit)
                    .for_relation(&request.relation),
            );
        }
        let order = render_order_terms(backend, &terms);
        let branch_alias = backend.quote_identifier(&format!("__gorm_relation_{}", group.id));
        branches.push(format!(
            "SELECT * FROM (SELECT {columns}, CAST({} AS BIGINT) AS {group_alias}, \
             ROW_NUMBER() OVER (ORDER BY {order}) AS {row_alias} FROM {table} \
             WHERE {} ORDER BY {order} LIMIT {limit}) AS {branch_alias}",
            group.id,
            predicates.join(" AND ")
        ));
    }
    let sql = format!(
        "{} ORDER BY {group_alias}, {row_alias}",
        branches.join(" UNION ALL ")
    );
    Ok((sql, values))
}

fn render_relation_counts(
    request: &RuntimeRelationBatchRequest,
    backend: DatabaseBackend,
    groups: &[QueryGroup],
) -> Result<(String, Vec<SqlValue>, BTreeMap<usize, usize>), RuntimeRelationError> {
    let target_fields = request
        .relation
        .key_pairs()
        .iter()
        .map(|(_, target)| target.clone())
        .collect::<Vec<_>>();
    let table = backend.quote_identifier(request.relation.target().physical_table());
    let group_alias = backend.quote_identifier(GROUP_ALIAS);
    let count_alias = backend.quote_identifier(COUNT_ALIAS);
    let mut values = Vec::new();
    let mut unique = BTreeMap::<Vec<u8>, (usize, Vec<RuntimeValue>)>::new();
    let mut group_to_count = BTreeMap::new();
    for group in groups {
        let signature = serde_json::to_vec(&group.key).map_err(|error| {
            RuntimeRelationError::new(RuntimeRelationErrorCode::InvalidRequest).source(error)
        })?;
        let next = unique.len();
        let id = unique
            .entry(signature)
            .or_insert_with(|| (next, group.key.clone()))
            .0;
        group_to_count.insert(group.id, id);
    }
    let mut branches = Vec::new();
    for (id, key) in unique.into_values() {
        let mut predicates = vec![bind_key(backend, &target_fields, &key, &mut values)?];
        if let Some(predicate) = &request.predicate {
            predicates.push(predicate.render(backend, &mut values));
        }
        branches.push(format!(
            "SELECT CAST({id} AS BIGINT) AS {group_alias}, COUNT(*) AS {count_alias} FROM {table} WHERE {}",
            predicates.join(" AND ")
        ));
    }
    if values.len() > request.limits.max_bind_parameters {
        return Err(
            RuntimeRelationError::new(RuntimeRelationErrorCode::ResourceLimit)
                .for_relation(&request.relation),
        );
    }
    Ok((branches.join(" UNION ALL "), values, group_to_count))
}

impl<B> crate::db::Database<B>
where
    B: OrmBackend + RuntimeRowDecoder,
{
    /// Execute one validated, bounded runtime relation layer.
    ///
    /// All compatible parents are represented by branches of one bounded SQL
    /// statement. Optional counts use one additional statement under the same
    /// PostgreSQL transaction-local auth context.
    ///
    /// # Errors
    ///
    /// Returns stable capability, cursor, cardinality, decode, and backend
    /// categories. Safe diagnostics contain no SQL, physical identifiers,
    /// parent keys, predicates, cursors, or row values.
    pub async fn execute_runtime_relation_batch(
        &self,
        request: &RuntimeRelationBatchRequest,
        auth: Option<&DbAuthContext>,
    ) -> Result<RuntimeRelationBatch, RuntimeRelationError> {
        if !B::RUNTIME_ROW_DECODING_SUPPORTED
            || !matches!(
                B::DIALECT,
                DatabaseBackend::Sqlite | DatabaseBackend::Postgres
            )
        {
            return Err(
                RuntimeRelationError::new(RuntimeRelationErrorCode::UnsupportedBackend)
                    .for_relation(&request.relation),
            );
        }
        let groups = group_requests(request)?;
        let mut results = request
            .anchors
            .iter()
            .map(|anchor| RuntimeRelationResult {
                parent_index: anchor.parent_index,
                value: match request.relation.cardinality() {
                    RelationCardinality::One => RuntimeRelationValue::ToOne(None),
                    RelationCardinality::Many => {
                        RuntimeRelationValue::ToMany(super::RuntimeConnection {
                            edges: Vec::new(),
                            page_info: RuntimePageInfo {
                                has_next_page: false,
                                has_previous_page: false,
                                start_cursor: None,
                                end_cursor: None,
                            },
                            total_count: match request.selection {
                                RuntimeRelationSelection::ToMany {
                                    include_count: true,
                                    ..
                                } => Some(0),
                                _ => None,
                            },
                        })
                    }
                },
            })
            .collect::<Vec<_>>();
        if groups.is_empty() {
            return Ok(RuntimeRelationBatch { results });
        }
        let (sql, values) = render_relation_rows(request, B::DIALECT, &groups)?;
        let counts_requested = matches!(
            request.selection,
            RuntimeRelationSelection::ToMany {
                include_count: true,
                ..
            }
        );
        let (rows, count_rows, count_groups) = if counts_requested {
            let (count_sql, count_values, count_groups) =
                render_relation_counts(request, B::DIALECT, &groups)?;
            let (rows, counts) = super::fetch_rows_pair_with_auth::<B>(
                self.pool(),
                &sql,
                &values,
                &count_sql,
                &count_values,
                auth,
            )
            .await
            .map_err(|error| {
                RuntimeRelationError::new(RuntimeRelationErrorCode::BackendExecution)
                    .for_relation(&request.relation)
                    .source(error)
            })?;
            (rows, Some(counts), count_groups)
        } else {
            let rows = super::fetch_rows_with_auth::<B>(self.pool(), &sql, &values, auth)
                .await
                .map_err(|error| {
                    RuntimeRelationError::new(RuntimeRelationErrorCode::BackendExecution)
                        .for_relation(&request.relation)
                        .source(error)
                })?;
            (rows, None, BTreeMap::new())
        };
        let mut decoded = BTreeMap::<usize, Vec<RuntimeRecord>>::new();
        for row in rows {
            let group = usize::try_from(B::try_get_i64(&row, GROUP_ALIAS).map_err(|error| {
                RuntimeRelationError::new(RuntimeRelationErrorCode::Decode).source(error)
            })?)
            .map_err(|error| {
                RuntimeRelationError::new(RuntimeRelationErrorCode::Decode).source(error)
            })?;
            decoded
                .entry(group)
                .or_default()
                .push(request.decode_projection.decode_row::<B>(&row)?);
        }
        let mut counts = BTreeMap::new();
        if let Some(count_rows) = count_rows {
            for row in count_rows {
                let group =
                    usize::try_from(B::try_get_i64(&row, GROUP_ALIAS).map_err(|error| {
                        RuntimeRelationError::new(RuntimeRelationErrorCode::Decode).source(error)
                    })?)
                    .map_err(|error| {
                        RuntimeRelationError::new(RuntimeRelationErrorCode::Decode).source(error)
                    })?;
                let count = B::try_get_i64(&row, COUNT_ALIAS).map_err(|error| {
                    RuntimeRelationError::new(RuntimeRelationErrorCode::Decode).source(error)
                })?;
                counts.insert(group, count);
            }
        }
        for group in groups {
            let mut records = decoded.remove(&group.id).unwrap_or_default();
            match &group.page {
                None => {
                    if records.len() > 1 {
                        return Err(RuntimeRelationError::new(
                            RuntimeRelationErrorCode::CardinalityViolation,
                        )
                        .for_relation(&request.relation));
                    }
                    let value = records
                        .pop()
                        .map(|record| record.project(&request.output_projection))
                        .transpose()?;
                    for index in group.anchor_indexes {
                        results[index].value = RuntimeRelationValue::ToOne(value.clone());
                    }
                }
                Some(page) => {
                    let (size, backward, cursor_present) = match page {
                        RuntimePageRequest::First { size, after } => {
                            (*size as usize, false, after.is_some())
                        }
                        RuntimePageRequest::Last { size, before } => {
                            (*size as usize, true, before.is_some())
                        }
                    };
                    let lookahead = records.len() > size;
                    records.truncate(size);
                    if backward {
                        records.reverse();
                    }
                    for index in group.anchor_indexes {
                        let anchor = &request.anchors[index];
                        let mut edges = Vec::with_capacity(records.len());
                        for record in &records {
                            let cursor_values = request
                                .order
                                .terms()
                                .iter()
                                .map(|term| match record.state(term.field())? {
                                    RuntimeFieldState::Value(value) => Ok(value.clone()),
                                    RuntimeFieldState::Null => Ok(RuntimeValue::Null),
                                    RuntimeFieldState::Unloaded => Err(RuntimeRelationError::new(
                                        RuntimeRelationErrorCode::Decode,
                                    )),
                                })
                                .collect::<Result<Vec<_>, RuntimeRelationError>>()?;
                            edges.push(RuntimeEdge {
                                node: record.project(&request.output_projection)?,
                                cursor: encode_relation_cursor(request, anchor, cursor_values)?,
                            });
                        }
                        let start_cursor = edges.first().map(|edge| edge.cursor.clone());
                        let end_cursor = edges.last().map(|edge| edge.cursor.clone());
                        let count = count_groups
                            .get(&group.id)
                            .and_then(|count_group| counts.get(count_group))
                            .copied();
                        results[index].value =
                            RuntimeRelationValue::ToMany(super::RuntimeConnection {
                                edges,
                                page_info: if backward {
                                    RuntimePageInfo {
                                        has_next_page: cursor_present,
                                        has_previous_page: lookahead,
                                        start_cursor,
                                        end_cursor,
                                    }
                                } else {
                                    RuntimePageInfo {
                                        has_next_page: lookahead,
                                        has_previous_page: cursor_present,
                                        start_cursor,
                                        end_cursor,
                                    }
                                },
                                total_count: count,
                            });
                    }
                }
            }
        }
        results.sort_by_key(|result| result.parent_index);
        Ok(RuntimeRelationBatch { results })
    }
}
