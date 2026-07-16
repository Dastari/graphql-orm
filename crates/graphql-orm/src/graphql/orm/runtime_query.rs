//! Validated, backend-neutral runtime read execution.
//!
//! Every executable request is derived from fingerprint-bound runtime schema
//! handles. Physical identifiers and SQL fragments never enter the public
//! request model.

use std::collections::{BTreeSet, HashSet};
use std::error::Error as StdError;
use std::fmt;

use super::{
    CollectionId, DatabaseBackend, DbAuthContext, FieldId, OrmBackend, RuntimeCollectionHandle,
    RuntimeFieldHandle, RuntimeOrderDirection, RuntimeProjection, RuntimeRecord,
    RuntimeRecordError, RuntimeRowDecoder, RuntimeValue, RuntimeValueKind, SchemaFingerprint,
    SqlDialect, SqlValue, ValidatedRuntimeSchema,
};

const RUNTIME_CURSOR_PREFIX: &str = "gormrq1.";

/// Stable category for runtime query validation, cursor, decoding, and execution failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeQueryErrorCode {
    InvalidHandle,
    InvalidRequest,
    InvalidFilter,
    UnsupportedOperator,
    UnsupportedOrder,
    UnsupportedBackend,
    ResourceLimit,
    CursorInvalid,
    CursorSchemaMismatch,
    Decode,
    BackendExecution,
}

impl RuntimeQueryErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidHandle => "invalid_handle",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidFilter => "invalid_filter",
            Self::UnsupportedOperator => "unsupported_operator",
            Self::UnsupportedOrder => "unsupported_order",
            Self::UnsupportedBackend => "unsupported_backend",
            Self::ResourceLimit => "resource_limit",
            Self::CursorInvalid => "cursor_invalid",
            Self::CursorSchemaMismatch => "cursor_schema_mismatch",
            Self::Decode => "decode",
            Self::BackendExecution => "backend_execution",
        }
    }
}

/// Safe runtime query error. SQL, physical identifiers, cursor contents, and values are redacted.
pub struct RuntimeQueryError {
    code: RuntimeQueryErrorCode,
    collection: Option<CollectionId>,
    field: Option<FieldId>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RuntimeQueryError {
    fn new(code: RuntimeQueryErrorCode) -> Self {
        Self {
            code,
            collection: None,
            field: None,
            source: None,
        }
    }

    fn collection(mut self, value: &CollectionId) -> Self {
        self.collection = Some(value.clone());
        self
    }

    fn field(mut self, value: &RuntimeFieldHandle) -> Self {
        self.collection = Some(value.collection_id().clone());
        self.field = Some(value.id().clone());
        self
    }

    fn source(mut self, value: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(value));
        self
    }

    pub const fn code(&self) -> RuntimeQueryErrorCode {
        self.code
    }
    pub fn collection_id(&self) -> Option<&CollectionId> {
        self.collection.as_ref()
    }
    pub fn field_id(&self) -> Option<&FieldId> {
        self.field.as_ref()
    }
}

impl fmt::Debug for RuntimeQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeQueryError")
            .field("code", &self.code)
            .field("collection", &self.collection)
            .field("field", &self.field)
            .field("source", &self.source.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl fmt::Display for RuntimeQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime query error: {}", self.code.as_str())
    }
}

impl StdError for RuntimeQueryError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

impl From<RuntimeRecordError> for RuntimeQueryError {
    fn from(value: RuntimeRecordError) -> Self {
        Self::new(RuntimeQueryErrorCode::Decode).source(value)
    }
}

/// Hard validation limits applied before rendering or database I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeQueryLimits {
    pub max_predicate_depth: usize,
    pub max_predicate_nodes: usize,
    pub max_values_per_list: usize,
    pub max_bind_parameters: usize,
    pub max_order_terms: usize,
    pub max_projection_fields: usize,
    pub default_page_size: u32,
    pub max_page_size: u32,
    pub max_cursor_bytes: usize,
    pub max_cursor_values: usize,
}

impl Default for RuntimeQueryLimits {
    fn default() -> Self {
        Self {
            max_predicate_depth: 16,
            max_predicate_nodes: 256,
            max_values_per_list: 100,
            max_bind_parameters: 999,
            max_order_terms: 16,
            max_projection_fields: 128,
            default_page_size: 50,
            max_page_size: 100,
            max_cursor_bytes: 16 * 1024,
            max_cursor_values: 32,
        }
    }
}

/// Frozen scalar operator set for the first runtime-query version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeScalarOperator {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
    StartsWith,
    EndsWith,
}

/// Membership operator for bounded value lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeListOperator {
    In,
    NotIn,
}

#[derive(Clone, Debug)]
enum PredicateExpr {
    Constant(bool),
    IsNull {
        field: RuntimeFieldHandle,
        is_null: bool,
    },
    Compare {
        field: RuntimeFieldHandle,
        op: RuntimeScalarOperator,
        value: RuntimeValue,
    },
    List {
        field: RuntimeFieldHandle,
        op: RuntimeListOperator,
        values: Vec<RuntimeValue>,
    },
    Between {
        field: RuntimeFieldHandle,
        low: RuntimeValue,
        high: RuntimeValue,
    },
    And(Vec<PredicateExpr>),
    Or(Vec<PredicateExpr>),
    Not(Box<PredicateExpr>),
}

/// Owned validated predicate bound to one schema fingerprint and collection.
#[derive(Clone)]
pub struct RuntimePredicate {
    schema: SchemaFingerprint,
    collection: CollectionId,
    expr: PredicateExpr,
    nodes: usize,
    depth: usize,
    binds: usize,
}

impl fmt::Debug for RuntimePredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimePredicate")
            .field("schema", &self.schema)
            .field("collection", &self.collection)
            .field("expression", &"[redacted]")
            .finish()
    }
}

/// Explicit portable null placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeNullPlacement {
    First,
    Last,
}

/// Caller-supplied order term. Validation resolves it into an executable order.
#[derive(Clone, Debug)]
pub struct RuntimeOrderInput {
    pub field: RuntimeFieldHandle,
    pub direction: RuntimeOrderDirection,
    pub nulls: RuntimeNullPlacement,
}

/// One validated effective order term.
#[derive(Clone, Debug)]
pub struct RuntimeEffectiveOrderTerm {
    field: RuntimeFieldHandle,
    direction: RuntimeOrderDirection,
    nulls: RuntimeNullPlacement,
}

impl RuntimeEffectiveOrderTerm {
    pub fn field(&self) -> &RuntimeFieldHandle {
        &self.field
    }
    pub const fn direction(&self) -> RuntimeOrderDirection {
        self.direction
    }
    pub const fn nulls(&self) -> RuntimeNullPlacement {
        self.nulls
    }
}

/// Validated deterministic total order including missing primary-key tie-breakers.
#[derive(Clone, Debug)]
pub struct RuntimeOrder {
    schema: SchemaFingerprint,
    collection: CollectionId,
    terms: Vec<RuntimeEffectiveOrderTerm>,
}

impl RuntimeOrder {
    pub fn terms(&self) -> &[RuntimeEffectiveOrderTerm] {
        &self.terms
    }
}

/// Bounded bidirectional keyset window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimePageRequest {
    First { size: i64, after: Option<String> },
    Last { size: i64, before: Option<String> },
}

impl RuntimePageRequest {
    pub fn first(size: i64, after: Option<String>) -> Self {
        Self::First { size, after }
    }
    pub fn last(size: i64, before: Option<String>) -> Self {
        Self::Last { size, before }
    }
}

/// Fully validated executable runtime read request.
#[derive(Clone)]
pub struct RuntimeReadRequest {
    schema: SchemaFingerprint,
    collection: RuntimeCollectionHandle,
    output_projection: RuntimeProjection,
    decode_projection: RuntimeProjection,
    predicate: Option<RuntimePredicate>,
    order: RuntimeOrder,
    page: RuntimePageRequest,
    include_total_count: bool,
    limits: RuntimeQueryLimits,
}

impl fmt::Debug for RuntimeReadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeReadRequest")
            .field("schema", &self.schema)
            .field("collection", self.collection.id())
            .field("projection_fields", &self.output_projection.fields().len())
            .field("predicate", &self.predicate.as_ref().map(|_| "[redacted]"))
            .field("order_terms", &self.order.terms.len())
            .field("page", &"[redacted]")
            .field("include_total_count", &self.include_total_count)
            .finish()
    }
}

/// One runtime connection edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeEdge {
    pub node: RuntimeRecord,
    pub cursor: String,
}

/// Complete runtime keyset page information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
}

/// Bounded runtime connection. Count is absent unless explicitly requested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConnection {
    pub edges: Vec<RuntimeEdge>,
    pub page_info: RuntimePageInfo,
    pub total_count: Option<i64>,
}

fn limit_error(collection: &CollectionId) -> RuntimeQueryError {
    RuntimeQueryError::new(RuntimeQueryErrorCode::ResourceLimit).collection(collection)
}

fn check_handle(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    field: &RuntimeFieldHandle,
) -> Result<(), RuntimeQueryError> {
    let current = schema
        .resolve_field(collection, field.id())
        .map_err(|error| {
            RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle).source(error)
        })?;
    if &current != field {
        return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle).field(field));
    }
    Ok(())
}

fn check_collection(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
) -> Result<(), RuntimeQueryError> {
    let current = schema
        .resolve_collection(collection.id())
        .map_err(|error| {
            RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle).source(error)
        })?;
    if &current != collection {
        return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
            .collection(collection.id()));
    }
    Ok(())
}

fn validate_literal(
    field: &RuntimeFieldHandle,
    value: &RuntimeValue,
) -> Result<(), RuntimeQueryError> {
    if matches!(value, RuntimeValue::Null) {
        return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).field(field));
    }
    if value.kind() != Some(field.value_kind()) {
        return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).field(field));
    }
    Ok(())
}

fn compare_supported(kind: RuntimeValueKind, op: RuntimeScalarOperator) -> bool {
    use RuntimeScalarOperator as O;
    use RuntimeValueKind as K;
    match op {
        O::Eq | O::Ne => !matches!(kind, K::Json),
        O::Lt | O::Lte | O::Gt | O::Gte => {
            matches!(kind, K::Integer | K::Float | K::DateTime | K::String)
        }
        O::Contains | O::StartsWith | O::EndsWith => matches!(kind, K::String),
    }
}

impl ValidatedRuntimeSchema {
    /// Construct a validated scalar predicate.
    pub fn runtime_compare(
        &self,
        collection: &RuntimeCollectionHandle,
        field: &RuntimeFieldHandle,
        op: RuntimeScalarOperator,
        value: RuntimeValue,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        check_handle(self, collection, field)?;
        if !field.filterable() {
            return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).field(field));
        }
        validate_literal(field, &value)?;
        if !compare_supported(field.value_kind(), op) {
            return Err(
                RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedOperator).field(field),
            );
        }
        if limits.max_predicate_nodes < 1
            || limits.max_predicate_depth < 1
            || limits.max_bind_parameters < 1
        {
            return Err(limit_error(collection.id()));
        }
        Ok(RuntimePredicate {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            expr: PredicateExpr::Compare {
                field: field.clone(),
                op,
                value,
            },
            nodes: 1,
            depth: 1,
            binds: 1,
        })
    }

    /// Construct a validated SQL-null predicate for any field kind.
    pub fn runtime_is_null(
        &self,
        collection: &RuntimeCollectionHandle,
        field: &RuntimeFieldHandle,
        is_null: bool,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        check_handle(self, collection, field)?;
        if !field.filterable() {
            return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).field(field));
        }
        if limits.max_predicate_nodes < 1 || limits.max_predicate_depth < 1 {
            return Err(limit_error(collection.id()));
        }
        Ok(RuntimePredicate {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            expr: PredicateExpr::IsNull {
                field: field.clone(),
                is_null,
            },
            nodes: 1,
            depth: 1,
            binds: 0,
        })
    }

    /// Construct a validated bounded membership predicate. Empty `in` is false;
    /// empty `not_in` is true.
    pub fn runtime_list(
        &self,
        collection: &RuntimeCollectionHandle,
        field: &RuntimeFieldHandle,
        op: RuntimeListOperator,
        values: Vec<RuntimeValue>,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        check_handle(self, collection, field)?;
        if !field.filterable() {
            return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).field(field));
        }
        if matches!(field.value_kind(), RuntimeValueKind::Json) {
            return Err(
                RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedOperator).field(field),
            );
        }
        if values.len() > limits.max_values_per_list || values.len() > limits.max_bind_parameters {
            return Err(limit_error(collection.id()));
        }
        for value in &values {
            validate_literal(field, value)?;
        }
        let expr = if values.is_empty() {
            PredicateExpr::Constant(matches!(op, RuntimeListOperator::NotIn))
        } else {
            PredicateExpr::List {
                field: field.clone(),
                op,
                values: values.clone(),
            }
        };
        Ok(RuntimePredicate {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            expr,
            nodes: 1,
            depth: 1,
            binds: values.len(),
        })
    }

    /// Construct a validated inclusive range predicate.
    pub fn runtime_between(
        &self,
        collection: &RuntimeCollectionHandle,
        field: &RuntimeFieldHandle,
        low: RuntimeValue,
        high: RuntimeValue,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        check_handle(self, collection, field)?;
        if !field.filterable() {
            return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidFilter).field(field));
        }
        if !matches!(
            field.value_kind(),
            RuntimeValueKind::Integer
                | RuntimeValueKind::Float
                | RuntimeValueKind::DateTime
                | RuntimeValueKind::String
        ) {
            return Err(
                RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedOperator).field(field),
            );
        }
        validate_literal(field, &low)?;
        validate_literal(field, &high)?;
        if limits.max_bind_parameters < 2 {
            return Err(limit_error(collection.id()));
        }
        Ok(RuntimePredicate {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            expr: PredicateExpr::Between {
                field: field.clone(),
                low,
                high,
            },
            nodes: 1,
            depth: 1,
            binds: 2,
        })
    }

    fn runtime_combine(
        &self,
        collection: &RuntimeCollectionHandle,
        predicates: Vec<RuntimePredicate>,
        limits: RuntimeQueryLimits,
        is_and: bool,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        check_collection(self, collection)?;
        let mut nodes = 1usize;
        let mut depth = 1usize;
        let mut binds = 0usize;
        let mut exprs = Vec::with_capacity(predicates.len());
        for predicate in predicates {
            if predicate.schema != self.fingerprint() || predicate.collection != *collection.id() {
                return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
                    .collection(collection.id()));
            }
            nodes = nodes
                .checked_add(predicate.nodes)
                .ok_or_else(|| limit_error(collection.id()))?;
            binds = binds
                .checked_add(predicate.binds)
                .ok_or_else(|| limit_error(collection.id()))?;
            depth = depth.max(
                predicate
                    .depth
                    .checked_add(1)
                    .ok_or_else(|| limit_error(collection.id()))?,
            );
            exprs.push(predicate.expr);
        }
        if nodes > limits.max_predicate_nodes
            || depth > limits.max_predicate_depth
            || binds > limits.max_bind_parameters
        {
            return Err(limit_error(collection.id()));
        }
        let expr = if exprs.is_empty() {
            PredicateExpr::Constant(is_and)
        } else if is_and {
            PredicateExpr::And(exprs)
        } else {
            PredicateExpr::Or(exprs)
        };
        Ok(RuntimePredicate {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            expr,
            nodes,
            depth,
            binds,
        })
    }

    /// Structurally combine predicates with `AND`. Empty `AND` is true.
    pub fn runtime_and(
        &self,
        collection: &RuntimeCollectionHandle,
        predicates: Vec<RuntimePredicate>,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        self.runtime_combine(collection, predicates, limits, true)
    }

    /// Structurally combine predicates with `OR`. Empty `OR` is false.
    pub fn runtime_or(
        &self,
        collection: &RuntimeCollectionHandle,
        predicates: Vec<RuntimePredicate>,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        self.runtime_combine(collection, predicates, limits, false)
    }

    /// Negate exactly one validated predicate.
    pub fn runtime_not(
        &self,
        collection: &RuntimeCollectionHandle,
        predicate: RuntimePredicate,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimePredicate, RuntimeQueryError> {
        if predicate.schema != self.fingerprint() || predicate.collection != *collection.id() {
            return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
                .collection(collection.id()));
        }
        let nodes = predicate
            .nodes
            .checked_add(1)
            .ok_or_else(|| limit_error(collection.id()))?;
        let depth = predicate
            .depth
            .checked_add(1)
            .ok_or_else(|| limit_error(collection.id()))?;
        if nodes > limits.max_predicate_nodes || depth > limits.max_predicate_depth {
            return Err(limit_error(collection.id()));
        }
        Ok(RuntimePredicate {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            expr: PredicateExpr::Not(Box::new(predicate.expr)),
            nodes,
            depth,
            binds: predicate.binds,
        })
    }

    /// Resolve a deterministic total runtime order. Missing primary-key fields
    /// are appended in declared key order.
    pub fn runtime_order(
        &self,
        collection: &RuntimeCollectionHandle,
        inputs: Option<Vec<RuntimeOrderInput>>,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimeOrder, RuntimeQueryError> {
        check_collection(self, collection)?;
        let metadata = self
            .schema()
            .collections
            .iter()
            .find(|item| item.id == *collection.id())
            .ok_or_else(|| {
                RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
                    .collection(collection.id())
            })?;
        let supplied = match inputs {
            Some(values) => values,
            None => metadata
                .default_order
                .iter()
                .map(|term| {
                    let field = self
                        .resolve_field(collection, &term.field)
                        .map_err(|error| {
                            RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
                                .source(error)
                        })?;
                    Ok(RuntimeOrderInput {
                        field,
                        direction: term.direction,
                        nulls: RuntimeNullPlacement::Last,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeQueryError>>()?,
        };
        let mut seen = HashSet::new();
        let mut terms = Vec::new();
        for input in supplied {
            check_handle(self, collection, &input.field)?;
            if !input.field.sortable() || matches!(input.field.value_kind(), RuntimeValueKind::Json)
            {
                return Err(
                    RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedOrder)
                        .field(&input.field),
                );
            }
            if !seen.insert(input.field.id().clone()) {
                return Err(
                    RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidRequest)
                        .field(&input.field),
                );
            }
            terms.push(RuntimeEffectiveOrderTerm {
                field: input.field,
                direction: input.direction,
                nulls: input.nulls,
            });
        }
        for id in &metadata.primary_key {
            if seen.insert(id.clone()) {
                let field = self.resolve_field(collection, id).map_err(|error| {
                    RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle).source(error)
                })?;
                if !field.sortable() || matches!(field.value_kind(), RuntimeValueKind::Json) {
                    return Err(
                        RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedOrder)
                            .field(&field),
                    );
                }
                terms.push(RuntimeEffectiveOrderTerm {
                    field,
                    direction: RuntimeOrderDirection::Asc,
                    nulls: RuntimeNullPlacement::Last,
                });
            }
        }
        if terms.is_empty()
            || terms.len() > limits.max_order_terms
            || terms.len() > limits.max_cursor_values
        {
            return Err(limit_error(collection.id()));
        }
        Ok(RuntimeOrder {
            schema: self.fingerprint(),
            collection: collection.id().clone(),
            terms,
        })
    }

    /// Create an executable bounded runtime read request.
    pub fn runtime_read_request(
        &self,
        collection: &RuntimeCollectionHandle,
        projection: &RuntimeProjection,
        predicate: Option<RuntimePredicate>,
        order: RuntimeOrder,
        page: RuntimePageRequest,
        include_total_count: bool,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimeReadRequest, RuntimeQueryError> {
        check_collection(self, collection)?;
        if projection.schema_fingerprint() != &self.fingerprint()
            || projection.collection().id() != collection.id()
            || order.schema != self.fingerprint()
            || order.collection != *collection.id()
        {
            return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
                .collection(collection.id()));
        }
        if projection.fields().is_empty()
            || projection.fields().len() > limits.max_projection_fields
        {
            return Err(limit_error(collection.id()));
        }
        if let Some(filter) = &predicate {
            if filter.schema != self.fingerprint()
                || filter.collection != *collection.id()
                || filter.binds > limits.max_bind_parameters
            {
                return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle)
                    .collection(collection.id()));
            }
        }
        let (size, cursor) = match &page {
            RuntimePageRequest::First { size, after } => (*size, after),
            RuntimePageRequest::Last { size, before } => (*size, before),
        };
        if size <= 0
            || u32::try_from(size)
                .ok()
                .is_none_or(|value| value > limits.max_page_size)
        {
            return Err(limit_error(collection.id()));
        }
        let max_encoded_cursor = limits
            .max_cursor_bytes
            .checked_mul(2)
            .and_then(|value| value.checked_add(RUNTIME_CURSOR_PREFIX.len()))
            .ok_or_else(|| limit_error(collection.id()))?;
        if cursor
            .as_ref()
            .is_some_and(|value| value.len() > max_encoded_cursor)
        {
            return Err(limit_error(collection.id()));
        }
        let mut union = projection.fields().to_vec();
        let mut seen: BTreeSet<FieldId> = union.iter().map(|field| field.id().clone()).collect();
        for term in &order.terms {
            if seen.insert(term.field.id().clone()) {
                union.push(term.field.clone());
            }
        }
        let decode_projection = self
            .resolve_projection(collection, &union)
            .map_err(|error| {
                RuntimeQueryError::new(RuntimeQueryErrorCode::InvalidHandle).source(error)
            })?;
        Ok(RuntimeReadRequest {
            schema: self.fingerprint(),
            collection: collection.clone(),
            output_projection: projection.clone(),
            decode_projection,
            predicate,
            order,
            page,
            include_total_count,
            limits,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CursorEnvelope {
    version: u8,
    schema: SchemaFingerprint,
    collection: CollectionId,
    order: Vec<(FieldId, RuntimeOrderDirection, RuntimeNullPlacement)>,
    values: Vec<RuntimeValue>,
    checksum: String,
}

fn fnv(bytes: &[u8]) -> String {
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
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

fn order_signature(
    order: &RuntimeOrder,
) -> Vec<(FieldId, RuntimeOrderDirection, RuntimeNullPlacement)> {
    order
        .terms
        .iter()
        .map(|term| (term.field.id().clone(), term.direction, term.nulls))
        .collect()
}

fn encode_cursor(
    request: &RuntimeReadRequest,
    values: Vec<RuntimeValue>,
) -> Result<String, RuntimeQueryError> {
    let signature = order_signature(&request.order);
    let payload = serde_json::to_vec(&(
        1u8,
        &request.schema,
        request.collection.id(),
        &signature,
        &values,
    ))
    .map_err(|error| RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid).source(error))?;
    let envelope = CursorEnvelope {
        version: 1,
        schema: request.schema.clone(),
        collection: request.collection.id().clone(),
        order: signature,
        values,
        checksum: fnv(&payload),
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|error| {
        RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid).source(error)
    })?;
    if bytes.len() > request.limits.max_cursor_bytes {
        return Err(limit_error(request.collection.id()));
    }
    Ok(format!("{RUNTIME_CURSOR_PREFIX}{}", hex(&bytes)))
}

fn decode_cursor(
    request: &RuntimeReadRequest,
    cursor: &str,
) -> Result<Vec<RuntimeValue>, RuntimeQueryError> {
    if cursor.len()
        > request
            .limits
            .max_cursor_bytes
            .checked_mul(2)
            .and_then(|v| v.checked_add(RUNTIME_CURSOR_PREFIX.len()))
            .unwrap_or(usize::MAX)
    {
        return Err(limit_error(request.collection.id()));
    }
    let bytes = cursor
        .strip_prefix(RUNTIME_CURSOR_PREFIX)
        .and_then(unhex)
        .ok_or_else(|| {
            RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid)
                .collection(request.collection.id())
        })?;
    if bytes.len() > request.limits.max_cursor_bytes {
        return Err(limit_error(request.collection.id()));
    }
    let envelope: CursorEnvelope = serde_json::from_slice(&bytes).map_err(|_| {
        RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid)
            .collection(request.collection.id())
    })?;
    let payload = serde_json::to_vec(&(
        envelope.version,
        &envelope.schema,
        &envelope.collection,
        &envelope.order,
        &envelope.values,
    ))
    .map_err(|_| RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid))?;
    if envelope.version != 1 || envelope.checksum != fnv(&payload) {
        return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid)
            .collection(request.collection.id()));
    }
    if envelope.schema != request.schema || envelope.collection != *request.collection.id() {
        return Err(
            RuntimeQueryError::new(RuntimeQueryErrorCode::CursorSchemaMismatch)
                .collection(request.collection.id()),
        );
    }
    let signature = order_signature(&request.order);
    if envelope.order != signature
        || envelope.values.len() != signature.len()
        || envelope.values.len() > request.limits.max_cursor_values
    {
        return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid)
            .collection(request.collection.id()));
    }
    for (value, term) in envelope.values.iter().zip(&request.order.terms) {
        if !matches!(value, RuntimeValue::Null) && value.kind() != Some(term.field.value_kind()) {
            return Err(
                RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid).field(&term.field)
            );
        }
        if matches!(value, RuntimeValue::Null) && !term.field.nullable() {
            return Err(
                RuntimeQueryError::new(RuntimeQueryErrorCode::CursorInvalid).field(&term.field)
            );
        }
    }
    Ok(envelope.values)
}

fn value_bind(value: &RuntimeValue) -> Option<SqlValue> {
    match value {
        RuntimeValue::Null => None,
        RuntimeValue::Boolean(value) => Some(SqlValue::Bool(*value)),
        RuntimeValue::Integer(value) => Some(SqlValue::Int(*value)),
        RuntimeValue::Float(value) => Some(SqlValue::Float(value.get())),
        RuntimeValue::String(value) => Some(SqlValue::String(value.clone())),
        RuntimeValue::Uuid(value) => Some(SqlValue::Uuid(*value)),
        RuntimeValue::Json(value) => Some(SqlValue::Json(value.clone())),
        RuntimeValue::Bytes(value) => Some(SqlValue::Bytes(value.clone())),
        RuntimeValue::DateTime(value) => Some(SqlValue::String(value.as_str().to_string())),
    }
}

fn placeholder(backend: DatabaseBackend, index: usize, kind: RuntimeValueKind) -> String {
    let value = backend.placeholder(index);
    if backend == DatabaseBackend::Postgres && kind == RuntimeValueKind::DateTime {
        format!("{value}::timestamptz")
    } else {
        value
    }
}

fn ordered_column(backend: DatabaseBackend, field: &RuntimeFieldHandle) -> String {
    let column = backend.quote_identifier(field.physical_column());
    if field.value_kind() == RuntimeValueKind::String {
        match backend {
            DatabaseBackend::Postgres => format!("{column} COLLATE \"C\""),
            DatabaseBackend::Sqlite => format!("{column} COLLATE BINARY"),
            _ => column,
        }
    } else {
        column
    }
}

fn render_expr(
    backend: DatabaseBackend,
    expr: &PredicateExpr,
    values: &mut Vec<SqlValue>,
) -> String {
    match expr {
        PredicateExpr::Constant(value) => if *value { "1 = 1" } else { "1 = 0" }.to_string(),
        PredicateExpr::IsNull { field, is_null } => format!(
            "{} IS {}NULL",
            backend.quote_identifier(field.physical_column()),
            if *is_null { "" } else { "NOT " }
        ),
        PredicateExpr::Compare { field, op, value } => {
            let column = ordered_column(backend, field);
            let bind = placeholder(backend, values.len() + 1, field.value_kind());
            values.push(value_bind(value).expect("validated non-null literal"));
            use RuntimeScalarOperator as O;
            match op {
                O::Eq => format!("{column} = {bind}"),
                O::Ne => format!("{column} <> {bind}"),
                O::Lt => format!("{column} < {bind}"),
                O::Lte => format!("{column} <= {bind}"),
                O::Gt => format!("{column} > {bind}"),
                O::Gte => format!("{column} >= {bind}"),
                O::Contains => match backend {
                    DatabaseBackend::Postgres => format!("strpos({column}, {bind}) > 0"),
                    _ => format!("instr({column}, {bind}) > 0"),
                },
                O::StartsWith => match backend {
                    DatabaseBackend::Postgres => {
                        format!("left({column}, char_length({bind})) = {bind}")
                    }
                    _ => format!("instr({column}, {bind}) = 1"),
                },
                O::EndsWith => match backend {
                    DatabaseBackend::Postgres => {
                        format!("right({column}, char_length({bind})) = {bind}")
                    }
                    _ => {
                        let second = placeholder(backend, values.len() + 1, field.value_kind());
                        values.push(value_bind(value).expect("validated non-null literal"));
                        format!("substr({column}, -length({bind})) = {second}")
                    }
                },
            }
        }
        PredicateExpr::List {
            field,
            op,
            values: operands,
        } => {
            let column = ordered_column(backend, field);
            let mut binds = Vec::new();
            for value in operands {
                binds.push(placeholder(backend, values.len() + 1, field.value_kind()));
                values.push(value_bind(value).expect("validated non-null literal"));
            }
            format!(
                "{column} {} ({})",
                if matches!(op, RuntimeListOperator::In) {
                    "IN"
                } else {
                    "NOT IN"
                },
                binds.join(", ")
            )
        }
        PredicateExpr::Between { field, low, high } => {
            let column = ordered_column(backend, field);
            let low_bind = placeholder(backend, values.len() + 1, field.value_kind());
            values.push(value_bind(low).expect("validated"));
            let high_bind = placeholder(backend, values.len() + 1, field.value_kind());
            values.push(value_bind(high).expect("validated"));
            format!("{column} BETWEEN {low_bind} AND {high_bind}")
        }
        PredicateExpr::And(children) => format!(
            "({})",
            children
                .iter()
                .map(|child| render_expr(backend, child, values))
                .collect::<Vec<_>>()
                .join(" AND ")
        ),
        PredicateExpr::Or(children) => format!(
            "({})",
            children
                .iter()
                .map(|child| render_expr(backend, child, values))
                .collect::<Vec<_>>()
                .join(" OR ")
        ),
        PredicateExpr::Not(child) => format!("NOT ({})", render_expr(backend, child, values)),
    }
}

fn cursor_after_sql(
    backend: DatabaseBackend,
    order: &[RuntimeEffectiveOrderTerm],
    cursor: &[RuntimeValue],
    values: &mut Vec<SqlValue>,
) -> String {
    let mut branches = Vec::new();
    for index in 0..order.len() {
        let mut terms = Vec::new();
        for prior in 0..index {
            terms.push(cursor_equal(backend, &order[prior], &cursor[prior], values));
        }
        terms.push(cursor_compare(
            backend,
            &order[index],
            &cursor[index],
            values,
        ));
        branches.push(format!("({})", terms.join(" AND ")));
    }
    format!("({})", branches.join(" OR "))
}

fn cursor_equal(
    backend: DatabaseBackend,
    term: &RuntimeEffectiveOrderTerm,
    value: &RuntimeValue,
    values: &mut Vec<SqlValue>,
) -> String {
    let column = ordered_column(backend, &term.field);
    if matches!(value, RuntimeValue::Null) {
        format!("{column} IS NULL")
    } else {
        let bind = placeholder(backend, values.len() + 1, term.field.value_kind());
        values.push(value_bind(value).expect("validated cursor"));
        format!("{column} = {bind}")
    }
}

fn cursor_compare(
    backend: DatabaseBackend,
    term: &RuntimeEffectiveOrderTerm,
    value: &RuntimeValue,
    values: &mut Vec<SqlValue>,
) -> String {
    let column = ordered_column(backend, &term.field);
    match value {
        RuntimeValue::Null => match term.nulls {
            RuntimeNullPlacement::First => format!("{column} IS NOT NULL"),
            RuntimeNullPlacement::Last => "1 = 0".to_string(),
        },
        value => {
            let bind = placeholder(backend, values.len() + 1, term.field.value_kind());
            values.push(value_bind(value).expect("validated cursor"));
            let operator = if term.direction == RuntimeOrderDirection::Asc {
                ">"
            } else {
                "<"
            };
            let scalar = format!("{column} {operator} {bind}");
            match term.nulls {
                RuntimeNullPlacement::First => format!("({column} IS NOT NULL AND {scalar})"),
                RuntimeNullPlacement::Last => format!("({column} IS NULL OR {scalar})"),
            }
        }
    }
}

fn render_request(
    request: &RuntimeReadRequest,
    backend: DatabaseBackend,
) -> Result<(String, Vec<SqlValue>, Option<(String, Vec<SqlValue>)>, bool), RuntimeQueryError> {
    if !matches!(backend, DatabaseBackend::Sqlite | DatabaseBackend::Postgres) {
        return Err(
            RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedBackend)
                .collection(request.collection.id()),
        );
    }
    let mut values = Vec::new();
    let mut predicates = Vec::new();
    if let Some(predicate) = &request.predicate {
        predicates.push(render_expr(backend, &predicate.expr, &mut values));
    }
    let (size, cursor, backward) = match &request.page {
        RuntimePageRequest::First { size, after } => (*size, after, false),
        RuntimePageRequest::Last { size, before } => (*size, before, true),
    };
    if let Some(cursor) = cursor {
        let cursor_values = decode_cursor(request, cursor)?;
        let filter_binds = request
            .predicate
            .as_ref()
            .map_or(0, |predicate| predicate.binds);
        if filter_binds
            .checked_add(cursor_values.len())
            .is_none_or(|total| total > request.limits.max_bind_parameters)
        {
            return Err(limit_error(request.collection.id()));
        }
        let effective = if backward {
            request
                .order
                .terms
                .iter()
                .map(|term| RuntimeEffectiveOrderTerm {
                    field: term.field.clone(),
                    direction: if term.direction == RuntimeOrderDirection::Asc {
                        RuntimeOrderDirection::Desc
                    } else {
                        RuntimeOrderDirection::Asc
                    },
                    nulls: match term.nulls {
                        RuntimeNullPlacement::First => RuntimeNullPlacement::Last,
                        RuntimeNullPlacement::Last => RuntimeNullPlacement::First,
                    },
                })
                .collect::<Vec<_>>()
        } else {
            request.order.terms.clone()
        };
        predicates.push(cursor_after_sql(
            backend,
            &effective,
            &cursor_values,
            &mut values,
        ));
    }
    let columns = request
        .decode_projection
        .fields()
        .iter()
        .map(|field| backend.quote_identifier(field.physical_column()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "SELECT {columns} FROM {}",
        backend.quote_identifier(request.collection.physical_table())
    );
    if !predicates.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&predicates.join(" AND "));
    }
    let effective = if backward {
        request
            .order
            .terms
            .iter()
            .map(|term| RuntimeEffectiveOrderTerm {
                field: term.field.clone(),
                direction: if term.direction == RuntimeOrderDirection::Asc {
                    RuntimeOrderDirection::Desc
                } else {
                    RuntimeOrderDirection::Asc
                },
                nulls: match term.nulls {
                    RuntimeNullPlacement::First => RuntimeNullPlacement::Last,
                    RuntimeNullPlacement::Last => RuntimeNullPlacement::First,
                },
            })
            .collect::<Vec<_>>()
    } else {
        request.order.terms.clone()
    };
    let order_sql = effective
        .iter()
        .flat_map(|term| {
            let column = ordered_column(backend, &term.field);
            let null_rank = if term.nulls == RuntimeNullPlacement::First {
                0
            } else {
                1
            };
            let direction = if term.direction == RuntimeOrderDirection::Asc {
                "ASC"
            } else {
                "DESC"
            };
            vec![
                format!(
                    "CASE WHEN {column} IS NULL THEN {null_rank} ELSE {} END ASC",
                    1 - null_rank
                ),
                format!("{column} {direction}"),
            ]
        })
        .collect::<Vec<_>>()
        .join(", ");
    sql.push_str(" ORDER BY ");
    sql.push_str(&order_sql);
    sql.push_str(&format!(" LIMIT {}", size + 1));
    let count = if request.include_total_count {
        let mut count_values = Vec::new();
        let mut count_sql = format!(
            "SELECT COUNT(*) AS {} FROM {}",
            backend.quote_identifier("__graphql_orm_runtime_count"),
            backend.quote_identifier(request.collection.physical_table())
        );
        if let Some(predicate) = &request.predicate {
            count_sql.push_str(" WHERE ");
            count_sql.push_str(&render_expr(backend, &predicate.expr, &mut count_values));
        }
        Some((count_sql, count_values))
    } else {
        None
    };
    Ok((sql, values, count, backward))
}

impl<B> crate::db::Database<B>
where
    B: OrmBackend + RuntimeRowDecoder,
{
    /// Execute a validated bounded runtime keyset read.
    ///
    /// `auth` is applied through the existing backend auth-context path. Hosts
    /// remain responsible for structurally composing required authorization
    /// predicates before constructing the request.
    ///
    /// # Errors
    ///
    /// Returns stable validation/cursor/capability/decode/execution errors. No
    /// safe error display contains SQL, identifiers, cursor values, or row data.
    pub async fn execute_runtime_read(
        &self,
        request: &RuntimeReadRequest,
        auth: Option<&DbAuthContext>,
    ) -> Result<RuntimeConnection, RuntimeQueryError> {
        if !B::RUNTIME_ROW_DECODING_SUPPORTED {
            return Err(
                RuntimeQueryError::new(RuntimeQueryErrorCode::UnsupportedBackend)
                    .collection(request.collection.id()),
            );
        }
        let (sql, values, count, backward) = render_request(request, B::DIALECT)?;
        let (rows, count_rows) = if let Some((count_sql, count_values)) = count {
            let (rows, count_rows) = super::fetch_rows_pair_with_auth::<B>(
                self.pool(),
                &sql,
                &values,
                &count_sql,
                &count_values,
                auth,
            )
            .await
            .map_err(|error| {
                RuntimeQueryError::new(RuntimeQueryErrorCode::BackendExecution)
                    .collection(request.collection.id())
                    .source(error)
            })?;
            (rows, Some(count_rows))
        } else {
            let rows = super::fetch_rows_with_auth::<B>(self.pool(), &sql, &values, auth)
                .await
                .map_err(|error| {
                    RuntimeQueryError::new(RuntimeQueryErrorCode::BackendExecution)
                        .collection(request.collection.id())
                        .source(error)
                })?;
            (rows, None)
        };
        let requested = match request.page {
            RuntimePageRequest::First { size, .. } | RuntimePageRequest::Last { size, .. } => {
                usize::try_from(size).expect("validated positive page size")
            }
        };
        let lookahead = rows.len() > requested;
        let mut decoded = rows
            .into_iter()
            .take(requested)
            .map(|row| {
                request
                    .decode_projection
                    .decode_row::<B>(&row)
                    .map_err(RuntimeQueryError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if backward {
            decoded.reverse();
        }
        let mut edges = Vec::with_capacity(decoded.len());
        for decoded_record in decoded {
            let cursor_values = request
                .order
                .terms
                .iter()
                .map(|term| {
                    match decoded_record
                        .state(&term.field)
                        .map_err(RuntimeQueryError::from)?
                    {
                        super::RuntimeFieldState::Null => Ok(RuntimeValue::Null),
                        super::RuntimeFieldState::Value(value) => Ok(value.clone()),
                        super::RuntimeFieldState::Unloaded => {
                            Err(RuntimeQueryError::new(RuntimeQueryErrorCode::Decode)
                                .field(&term.field))
                        }
                    }
                })
                .collect::<Result<Vec<_>, RuntimeQueryError>>()?;
            let cursor = encode_cursor(request, cursor_values)?;
            let node = decoded_record
                .project(&request.output_projection)
                .map_err(RuntimeQueryError::from)?;
            edges.push(RuntimeEdge { node, cursor });
        }
        let start_cursor = edges.first().map(|edge| edge.cursor.clone());
        let end_cursor = edges.last().map(|edge| edge.cursor.clone());
        let cursor_present = match &request.page {
            RuntimePageRequest::First { after, .. } => after.is_some(),
            RuntimePageRequest::Last { before, .. } => before.is_some(),
        };
        let page_info = if backward {
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
        };
        let total_count = match count_rows {
            None => None,
            Some(rows) if rows.len() == 1 => Some(
                B::try_get_i64(&rows[0], "__graphql_orm_runtime_count").map_err(|error| {
                    RuntimeQueryError::new(RuntimeQueryErrorCode::Decode)
                        .collection(request.collection.id())
                        .source(error)
                })?,
            ),
            Some(_) => {
                return Err(RuntimeQueryError::new(RuntimeQueryErrorCode::Decode)
                    .collection(request.collection.id()));
            }
        };
        Ok(RuntimeConnection {
            edges,
            page_info,
            total_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::orm::{RuntimeCollection, RuntimeField, RuntimeOrderTerm, RuntimeSchema};

    fn collection_id(value: &str) -> CollectionId {
        CollectionId::new(value).unwrap()
    }

    fn field_id(value: &str) -> FieldId {
        FieldId::new(value).unwrap()
    }

    fn field(id: &str, kind: RuntimeValueKind, nullable: bool) -> RuntimeField {
        RuntimeField {
            id: field_id(id),
            api_name: id.to_string(),
            physical_column: id.to_string(),
            value_kind: kind,
            nullable,
            unique: id == "id",
            filterable: true,
            sortable: kind != RuntimeValueKind::Json,
            generated: false,
            default: None,
        }
    }

    fn schema() -> ValidatedRuntimeSchema {
        let id = field("id", RuntimeValueKind::Integer, false);
        RuntimeSchema {
            format_version: 1,
            collections: vec![RuntimeCollection {
                id: collection_id("items"),
                api_type_name: "Item".to_string(),
                api_plural_name: "Items".to_string(),
                physical_table: "runtime_items".to_string(),
                primary_key: vec![id.id.clone()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    id.clone(),
                    field("name", RuntimeValueKind::String, false),
                    field("note", RuntimeValueKind::String, true),
                    field("data", RuntimeValueKind::Json, true),
                ],
                relations: vec![],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![RuntimeOrderTerm {
                    field: id.id,
                    direction: RuntimeOrderDirection::Asc,
                }],
            }],
        }
        .validate()
        .unwrap()
    }

    #[test]
    fn renderer_quotes_handles_and_binds_hostile_values() {
        let schema = schema();
        let collection = schema.resolve_collection(&collection_id("items")).unwrap();
        let name = schema
            .resolve_field(&collection, &field_id("name"))
            .unwrap();
        let projection = schema
            .resolve_projection(&collection, std::slice::from_ref(&name))
            .unwrap();
        let limits = RuntimeQueryLimits::default();
        let hostile = "x' OR 1=1 -- %_ ? $1 \0 雪";
        let filter = schema
            .runtime_compare(
                &collection,
                &name,
                RuntimeScalarOperator::Contains,
                RuntimeValue::String(hostile.to_string()),
                limits,
            )
            .unwrap();
        let order = schema.runtime_order(&collection, None, limits).unwrap();
        let request = schema
            .runtime_read_request(
                &collection,
                &projection,
                Some(filter),
                order,
                RuntimePageRequest::first(5, None),
                false,
                limits,
            )
            .unwrap();
        for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Postgres] {
            let (sql, values, count, _) = render_request(&request, backend).unwrap();
            assert!(sql.contains(&backend.quote_identifier("runtime_items")));
            assert!(sql.contains(&backend.quote_identifier("name")));
            assert!(!sql.contains(hostile));
            assert_eq!(values, vec![SqlValue::String(hostile.to_string())]);
            assert!(count.is_none());
        }
        let debug = format!("{request:?}");
        assert!(!debug.contains(hostile));
        assert!(!debug.contains("runtime_items"));
    }

    #[test]
    fn operator_and_resource_validation_is_fail_closed() {
        let schema = schema();
        let collection = schema.resolve_collection(&collection_id("items")).unwrap();
        let name = schema
            .resolve_field(&collection, &field_id("name"))
            .unwrap();
        let data = schema
            .resolve_field(&collection, &field_id("data"))
            .unwrap();
        let limits = RuntimeQueryLimits::default();
        assert_eq!(
            schema
                .runtime_compare(
                    &collection,
                    &data,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::Json(serde_json::json!({"a": 1})),
                    limits,
                )
                .unwrap_err()
                .code(),
            RuntimeQueryErrorCode::UnsupportedOperator
        );
        assert_eq!(
            schema
                .runtime_compare(
                    &collection,
                    &name,
                    RuntimeScalarOperator::Eq,
                    RuntimeValue::Integer(1),
                    limits,
                )
                .unwrap_err()
                .code(),
            RuntimeQueryErrorCode::InvalidFilter
        );
        let leaf = schema
            .runtime_is_null(&collection, &name, true, limits)
            .unwrap();
        let shallow = schema.runtime_not(&collection, leaf, limits).unwrap();
        let restricted = RuntimeQueryLimits {
            max_predicate_depth: 2,
            ..limits
        };
        assert_eq!(
            schema
                .runtime_not(&collection, shallow, restricted)
                .unwrap_err()
                .code(),
            RuntimeQueryErrorCode::ResourceLimit
        );
        let empty_and = schema.runtime_and(&collection, vec![], limits).unwrap();
        let empty_or = schema.runtime_or(&collection, vec![], limits).unwrap();
        assert!(matches!(empty_and.expr, PredicateExpr::Constant(true)));
        assert!(matches!(empty_or.expr, PredicateExpr::Constant(false)));
    }
}
