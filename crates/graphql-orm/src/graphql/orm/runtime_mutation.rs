//! Schema-bound, single-record writes on an ORM-owned pinned transaction.
use super::runtime_query::{placeholder, value_bind};
use super::*;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use futures::future::BoxFuture;
use std::{collections::BTreeSet, fmt, sync::Arc};

/// Safe runtime mutation category. No variant contains caller values or SQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeMutationErrorCode {
    InvalidInput,
    MissingRequired,
    FieldDenied,
    Denied,
    NotFound,
    Conflict,
    ConstraintViolation,
    AppendOnly,
    UnsupportedBackend,
    UnsupportedGeneration,
    UnsupportedCascade,
    SchemaMismatch,
    LimitExceeded,
    RetryableTransaction,
    HookFailed,
    DatabaseFailed,
    CommitUnknown,
}
impl RuntimeMutationErrorCode {
    /// Stable machine code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::MissingRequired => "missing_required",
            Self::FieldDenied => "field_denied",
            Self::Denied => "denied",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::ConstraintViolation => "constraint_violation",
            Self::AppendOnly => "append_only",
            Self::UnsupportedBackend => "unsupported_backend",
            Self::UnsupportedGeneration => "unsupported_generation",
            Self::UnsupportedCascade => "unsupported_cascade",
            Self::SchemaMismatch => "schema_mismatch",
            Self::LimitExceeded => "limit_exceeded",
            Self::RetryableTransaction => "retryable_transaction",
            Self::HookFailed => "hook_failed",
            Self::DatabaseFailed => "database_failed",
            Self::CommitUnknown => "commit_unknown",
        }
    }
}
/// Redacted mutation failure. The source is available only through trusted error inspection.
pub struct RuntimeMutationError {
    code: RuntimeMutationErrorCode,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}
impl RuntimeMutationError {
    /// Construct an explicit fail-closed host decision.
    pub fn new(code: RuntimeMutationErrorCode) -> Self {
        Self { code, source: None }
    }
    /// Safe category.
    pub fn code(&self) -> RuntimeMutationErrorCode {
        self.code
    }
    /// Retain a trusted diagnostic source without exposing it in formatting.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }
}
impl fmt::Debug for RuntimeMutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RuntimeMutationError")
            .field(&self.code)
            .finish()
    }
}
impl fmt::Display for RuntimeMutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.as_str())
    }
}
impl std::error::Error for RuntimeMutationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|e| e as _)
    }
}
impl From<RuntimeRecordError> for RuntimeMutationError {
    fn from(e: RuntimeRecordError) -> Self {
        let code = if e.code() == RuntimeRecordErrorCode::SchemaMismatch {
            RuntimeMutationErrorCode::SchemaMismatch
        } else {
            RuntimeMutationErrorCode::InvalidInput
        };
        Self::new(code).with_source(e)
    }
}
impl From<RuntimeMutationError> for OrmPublicError {
    fn from(e: RuntimeMutationError) -> Self {
        use RuntimeMutationErrorCode as C;
        let code = match e.code {
            C::Denied | C::FieldDenied => OrmErrorCode::Forbidden,
            C::NotFound => OrmErrorCode::NotFound,
            C::Conflict => OrmErrorCode::Conflict,
            C::ConstraintViolation => OrmErrorCode::ConstraintViolation,
            C::RetryableTransaction => OrmErrorCode::ServiceUnavailable,
            C::DatabaseFailed | C::CommitUnknown | C::HookFailed => OrmErrorCode::InternalError,
            _ => OrmErrorCode::InvalidInput,
        };
        let mut public = Self::with_message(code, e.code.as_str());
        public.runtime_mutation_code = Some(e.code.as_str());
        public.retryable = e.code == C::RetryableTransaction;
        public
    }
}
fn err(code: RuntimeMutationErrorCode) -> RuntimeMutationError {
    RuntimeMutationError::new(code)
}
fn database_error<B: WriteBackend>(e: sqlx::Error) -> RuntimeMutationError {
    let code = if B::is_retryable_write_error(&e) {
        RuntimeMutationErrorCode::RetryableTransaction
    } else if OrmPublicError::from_sqlx(&e).code == OrmErrorCode::ConstraintViolation {
        RuntimeMutationErrorCode::ConstraintViolation
    } else {
        RuntimeMutationErrorCode::DatabaseFailed
    };
    err(code).with_source(e)
}

/// Bounds enforced before protected I/O and when decoding authoritative records.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeMutationLimits {
    pub max_input_fields: usize,
    pub max_key_fields: usize,
    pub max_value_bytes: usize,
    pub max_record_bytes: usize,
    pub max_json_depth: usize,
    pub query: RuntimeQueryLimits,
}
impl Default for RuntimeMutationLimits {
    fn default() -> Self {
        Self {
            max_input_fields: 128,
            max_key_fields: 16,
            max_value_bytes: 1024 * 1024,
            max_record_bytes: 4 * 1024 * 1024,
            max_json_depth: 32,
            query: RuntimeQueryLimits::default(),
        }
    }
}
/// Exact operation; no bulk, nested writes or implicit upsert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMutationAction {
    Create,
    Update,
    Delete,
}
/// Complete, non-null primary key bound to a validated schema.
#[derive(Clone)]
pub struct RuntimeKey {
    schema: Arc<ValidatedRuntimeSchema>,
    collection: RuntimeCollectionHandle,
    fields: Vec<(RuntimeFieldHandle, RuntimeValue)>,
}
/// Immutable validated mutation request. Omission is distinct from SQL NULL.
#[derive(Clone)]
pub struct RuntimeMutationRequest {
    schema: Arc<ValidatedRuntimeSchema>,
    collection: RuntimeCollectionHandle,
    action: RuntimeMutationAction,
    fields: Vec<(RuntimeFieldHandle, RuntimeValue)>,
    key: Option<RuntimeKey>,
    expected: Option<RuntimePredicate>,
    returning: Option<RuntimeProjection>,
    limits: RuntimeMutationLimits,
}
macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => {$(impl fmt::Debug for $ty {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($ty), "([redacted])")) }
    })+};
}
redacted_debug!(RuntimeKey, RuntimeMutationRequest);
impl RuntimeKey {
    /// Complete key members in declared primary-key order; trusted host use only.
    pub fn fields(&self) -> &[(RuntimeFieldHandle, RuntimeValue)] {
        &self.fields
    }
    pub fn collection(&self) -> &RuntimeCollectionHandle {
        &self.collection
    }
}
/// Borrowed intent. Hosts must validate their complete policy/public revision here.
#[derive(Clone, Copy)]
pub struct RuntimeWriteIntent<'a> {
    request: &'a RuntimeMutationRequest,
}
impl<'a> RuntimeWriteIntent<'a> {
    pub fn action(self) -> RuntimeMutationAction {
        self.request.action
    }
    pub fn collection(self) -> &'a RuntimeCollectionHandle {
        &self.request.collection
    }
    pub fn fields(self) -> &'a [(RuntimeFieldHandle, RuntimeValue)] {
        &self.request.fields
    }
    pub fn key(self) -> Option<&'a RuntimeKey> {
        self.request.key.as_ref()
    }
    pub fn expected(self) -> Option<&'a RuntimePredicate> {
        self.request.expected.as_ref()
    }
    pub fn returning(self) -> Option<&'a RuntimeProjection> {
        self.request.returning.as_ref()
    }
}
/// Authoritative preimage from the pinned transaction, never a caller preview.
pub struct RuntimePreimageCheck<'a> {
    pub intent: RuntimeWriteIntent<'a>,
    pub record: &'a RuntimeRecord,
}
/// Actual staged result, including defaults; delete uses its authorized preimage.
pub struct RuntimeResultCheck<'a> {
    pub intent: RuntimeWriteIntent<'a>,
    pub preimage: Option<&'a RuntimeRecord>,
    pub record: &'a RuntimeRecord,
}
redacted_debug!(
    RuntimeWriteIntent<'_>,
    RuntimePreimageCheck<'_>,
    RuntimeResultCheck<'_>
);
/// Explicit approval of the entire intent, internal policy projection and row restriction.
pub struct RuntimeWriteGrant {
    projection: RuntimeProjection,
    predicate: Option<RuntimePredicate>,
}
impl RuntimeWriteGrant {
    /// Reject cross-schema/collection predicates; this does not narrow input approval.
    pub fn new(
        projection: RuntimeProjection,
        predicate: Option<RuntimePredicate>,
    ) -> Result<Self, RuntimeMutationError> {
        if predicate.as_ref().is_some_and(|p| {
            !p.belongs_to(
                projection.schema_fingerprint(),
                projection.collection().id(),
            )
        }) {
            return Err(err(RuntimeMutationErrorCode::SchemaMismatch));
        }
        Ok(Self {
            projection,
            predicate,
        })
    }
}
/// Explicit final-row approval and exact authorized return projection.
pub struct RuntimeReturnGrant {
    projection: Option<RuntimeProjection>,
}
impl RuntimeReturnGrant {
    pub fn new(projection: Option<RuntimeProjection>) -> Self {
        Self { projection }
    }
}
redacted_debug!(RuntimeWriteGrant, RuntimeReturnGrant);
/// Required host authority; there is no default allow implementation.
pub trait RuntimeWriteAuthority<B: RuntimeMutationBackend>: Send + Sync {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>>;
    fn authorize_preimage<'a>(
        &'a self,
        check: RuntimePreimageCheck<'a>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>>;
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>>;
}
/// Pending work, not committed success or an event. Only the runner establishes commit.
pub struct RuntimeMutationEffect {
    action: RuntimeMutationAction,
    record: Option<RuntimeRecord>,
}
impl RuntimeMutationEffect {
    pub fn action(&self) -> RuntimeMutationAction {
        self.action
    }
    pub fn record(&self) -> Option<&RuntimeRecord> {
        self.record.as_ref()
    }
}
redacted_debug!(RuntimeMutationEffect);
/// Atomic host journal hook. Invoke only through `run_runtime_before_commit`.
pub trait RuntimeMutationHook<B: RuntimeMutationBackend>: Send + Sync {
    fn before_commit<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, B>,
        pending: &'a RuntimeMutationEffect,
    ) -> BoxFuture<'a, Result<(), RuntimeMutationError>>;
}
/// Additive opt-in capability; existing third-party backends gain no required methods.
pub trait RuntimeMutationBackend: TransactionBackend + RuntimeRowDecoder {
    const RUNTIME_MUTATIONS_SUPPORTED: bool;
}
#[cfg(feature = "sqlite")]
impl RuntimeMutationBackend for SqliteBackend {
    const RUNTIME_MUTATIONS_SUPPORTED: bool = true;
}
#[cfg(feature = "postgres")]
impl RuntimeMutationBackend for PostgresBackend {
    const RUNTIME_MUTATIONS_SUPPORTED: bool = true;
}
#[cfg(feature = "mssql")]
impl RuntimeMutationBackend for MssqlBackend {
    const RUNTIME_MUTATIONS_SUPPORTED: bool = false;
}

fn collection<'a>(
    schema: &'a ValidatedRuntimeSchema,
    handle: &RuntimeCollectionHandle,
) -> Result<&'a RuntimeCollection, RuntimeMutationError> {
    if &schema.fingerprint() != handle.schema_fingerprint() {
        return Err(err(RuntimeMutationErrorCode::SchemaMismatch));
    }
    schema
        .schema()
        .collections
        .iter()
        .find(|c| &c.id == handle.id())
        .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))
}
pub(super) fn check_value(
    field: &RuntimeFieldHandle,
    value: &RuntimeValue,
    limits: RuntimeMutationLimits,
) -> Result<(), RuntimeMutationError> {
    if matches!(value, RuntimeValue::Null) {
        if !field.nullable() {
            return Err(err(RuntimeMutationErrorCode::InvalidInput));
        }
    } else if value.kind() != Some(field.value_kind()) {
        return Err(err(RuntimeMutationErrorCode::InvalidInput));
    }
    if let RuntimeValue::Json(value) = value {
        let mut stack = vec![(value, 1usize)];
        while let Some((node, depth)) = stack.pop() {
            if depth > limits.max_json_depth {
                return Err(err(RuntimeMutationErrorCode::LimitExceeded));
            }
            match node {
                serde_json::Value::Array(a) => stack.extend(a.iter().map(|v| (v, depth + 1))),
                serde_json::Value::Object(o) => stack.extend(o.values().map(|v| (v, depth + 1))),
                _ => {}
            }
        }
    }
    let bytes =
        serde_json::to_vec(value).map_err(|_| err(RuntimeMutationErrorCode::InvalidInput))?;
    if bytes.len() > limits.max_value_bytes {
        return Err(err(RuntimeMutationErrorCode::LimitExceeded));
    }
    Ok(())
}
impl ValidatedRuntimeSchema {
    /// Validate all primary-key members, preserving declared order.
    pub fn runtime_key(
        &self,
        handle: &RuntimeCollectionHandle,
        fields: &[(RuntimeFieldHandle, RuntimeValue)],
    ) -> Result<RuntimeKey, RuntimeMutationError> {
        let c = collection(self, handle)?;
        if fields.len() != c.primary_key.len() {
            return Err(err(RuntimeMutationErrorCode::InvalidInput));
        }
        let mut ordered = Vec::with_capacity(fields.len());
        for id in &c.primary_key {
            let expected = self.resolve_field(handle, id)?;
            let matches = fields
                .iter()
                .filter(|(f, _)| f == &expected)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(err(RuntimeMutationErrorCode::InvalidInput));
            }
            check_value(&expected, &matches[0].1, RuntimeMutationLimits::default())?;
            ordered.push(matches[0].clone());
        }
        Ok(RuntimeKey {
            schema: Arc::new(self.clone()),
            collection: handle.clone(),
            fields: ordered,
        })
    }
    /// Create input: omitted defaults/generators are evaluated at write time.
    pub fn runtime_create_request(
        &self,
        collection: &RuntimeCollectionHandle,
        fields: &[(RuntimeFieldHandle, RuntimeValue)],
        returning: Option<RuntimeProjection>,
        limits: RuntimeMutationLimits,
    ) -> Result<RuntimeMutationRequest, RuntimeMutationError> {
        self.mutation_request(
            collection,
            RuntimeMutationAction::Create,
            fields,
            None,
            None,
            returning,
            limits,
        )
    }
    /// Patch input: omitted fields remain unchanged; keys are immutable.
    pub fn runtime_update_request(
        &self,
        key: RuntimeKey,
        fields: &[(RuntimeFieldHandle, RuntimeValue)],
        expected: Option<RuntimePredicate>,
        returning: Option<RuntimeProjection>,
        limits: RuntimeMutationLimits,
    ) -> Result<RuntimeMutationRequest, RuntimeMutationError> {
        let collection = key.collection.clone();
        self.mutation_request(
            &collection,
            RuntimeMutationAction::Update,
            fields,
            Some(key),
            expected,
            returning,
            limits,
        )
    }
    /// Delete exactly one complete key; incoming modifying actions are rejected at execution.
    pub fn runtime_delete_request(
        &self,
        key: RuntimeKey,
        expected: Option<RuntimePredicate>,
        returning: Option<RuntimeProjection>,
        limits: RuntimeMutationLimits,
    ) -> Result<RuntimeMutationRequest, RuntimeMutationError> {
        let collection = key.collection.clone();
        self.mutation_request(
            &collection,
            RuntimeMutationAction::Delete,
            &[],
            Some(key),
            expected,
            returning,
            limits,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn mutation_request(
        &self,
        handle: &RuntimeCollectionHandle,
        action: RuntimeMutationAction,
        fields: &[(RuntimeFieldHandle, RuntimeValue)],
        key: Option<RuntimeKey>,
        expected: Option<RuntimePredicate>,
        returning: Option<RuntimeProjection>,
        limits: RuntimeMutationLimits,
    ) -> Result<RuntimeMutationRequest, RuntimeMutationError> {
        let c = collection(self, handle)?;
        if c.append_only && action != RuntimeMutationAction::Create {
            return Err(err(RuntimeMutationErrorCode::AppendOnly));
        }
        if fields.len() > limits.max_input_fields || c.primary_key.len() > limits.max_key_fields {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        if action == RuntimeMutationAction::Update && fields.is_empty() {
            return Err(err(RuntimeMutationErrorCode::InvalidInput));
        }
        if key
            .as_ref()
            .is_some_and(|k| k.schema.fingerprint() != self.fingerprint())
            || expected
                .as_ref()
                .is_some_and(|p| !p.belongs_to(&self.fingerprint(), &c.id))
        {
            return Err(err(RuntimeMutationErrorCode::SchemaMismatch));
        }
        if let Some(p) = &returning {
            self.resolve_projection(handle, p.fields())?;
        }
        if returning
            .as_ref()
            .is_some_and(|p| p.fields().len() > limits.query.max_projection_fields)
        {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        if let Some(p) = &expected {
            p.check_mutation_limits(limits)?;
        }
        let mut seen = BTreeSet::new();
        for (field, value) in fields {
            if self.resolve_field(handle, field.id())? != *field {
                return Err(err(RuntimeMutationErrorCode::SchemaMismatch));
            }
            if !seen.insert(field.id()) {
                return Err(err(RuntimeMutationErrorCode::InvalidInput));
            }
            let definition = c
                .fields
                .iter()
                .find(|f| &f.id == field.id())
                .ok_or_else(|| err(RuntimeMutationErrorCode::InvalidInput))?;
            if definition.generated
                || (action != RuntimeMutationAction::Create && c.primary_key.contains(field.id()))
            {
                return Err(err(RuntimeMutationErrorCode::FieldDenied));
            }
            check_value(field, value, limits)?;
        }
        let input_size = fields.iter().try_fold(0usize, |size, (_, value)| {
            let bytes = serde_json::to_vec(value)
                .map_err(|_| err(RuntimeMutationErrorCode::InvalidInput))?;
            size.checked_add(bytes.len())
                .filter(|size| *size <= limits.max_record_bytes)
                .ok_or_else(|| err(RuntimeMutationErrorCode::LimitExceeded))
        })?;
        let _ = input_size;
        if let Some(key) = &key {
            for (f, v) in &key.fields {
                check_value(f, v, limits)?;
            }
        }
        for field in &c.fields {
            if field.generated && field.value_kind != RuntimeValueKind::Uuid {
                return Err(err(RuntimeMutationErrorCode::UnsupportedGeneration));
            }
            if action == RuntimeMutationAction::Create
                && !seen.contains(&field.id)
                && !field.generated
                && field.default.is_none()
                && !field.nullable
            {
                return Err(err(RuntimeMutationErrorCode::MissingRequired));
            }
        }
        Ok(RuntimeMutationRequest {
            schema: Arc::new(self.clone()),
            collection: handle.clone(),
            action,
            fields: fields.to_vec(),
            key,
            expected,
            returning,
            limits,
        })
    }
}

fn bind_value(
    backend: DatabaseBackend,
    field: &RuntimeFieldHandle,
    value: &RuntimeValue,
    values: &mut Vec<SqlValue>,
) -> String {
    if let Some(value) = value_bind(value) {
        values.push(value);
        placeholder(backend, values.len(), field.value_kind())
    } else {
        "NULL".into()
    }
}
fn key_sql(
    backend: DatabaseBackend,
    fields: &[(RuntimeFieldHandle, RuntimeValue)],
    values: &mut Vec<SqlValue>,
) -> String {
    fields
        .iter()
        .map(|(f, v)| {
            format!(
                "{} = {}",
                backend.quote_identifier(f.physical_column()),
                bind_value(backend, f, v, values)
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}
fn columns(backend: DatabaseBackend, projection: &RuntimeProjection) -> String {
    projection
        .fields()
        .iter()
        .map(|f| backend.quote_identifier(f.physical_column()))
        .collect::<Vec<_>>()
        .join(", ")
}
fn bounded_record(
    record: &RuntimeRecord,
    projection: &RuntimeProjection,
    limits: RuntimeMutationLimits,
) -> Result<(), RuntimeMutationError> {
    for field in projection.fields() {
        match record.state(field)? {
            RuntimeFieldState::Null => check_value(field, &RuntimeValue::Null, limits)?,
            RuntimeFieldState::Value(value) => check_value(field, value, limits)?,
            RuntimeFieldState::Unloaded => {
                return Err(err(RuntimeMutationErrorCode::DatabaseFailed));
            }
        }
    }
    if record.to_json()?.len() > limits.max_record_bytes {
        return Err(err(RuntimeMutationErrorCode::LimitExceeded));
    }
    Ok(())
}
impl<B: RuntimeMutationBackend> MutationContext<'_, B> {
    /// Execute pending schema-bound work. Any error or cancellation poisons this transaction.
    ///
    /// The host must pin the physical environment and complete policy revision under its
    /// schema/DDL fence. A successful effect remains uncommitted until the outer runner returns.
    pub async fn mutate_runtime(
        &mut self,
        environment: &RuntimeMutationEnvironment,
        request: &RuntimeMutationRequest,
        authority: &dyn RuntimeWriteAuthority<B>,
    ) -> Result<RuntimeMutationEffect, RuntimeMutationError> {
        let guard = self.runtime_state.enter(); // first poll, before all awaits and validation
        let result = self
            .mutate_runtime_inner(environment, request, authority)
            .await?;
        guard.complete();
        Ok(result)
    }
    /// Execute an atomic journal hook with cancellation poisoning, even after successful DML.
    pub async fn run_runtime_before_commit(
        &mut self,
        hook: &dyn RuntimeMutationHook<B>,
        pending: &RuntimeMutationEffect,
    ) -> Result<(), RuntimeMutationError> {
        let guard = self.runtime_state.enter();
        if !B::RUNTIME_MUTATIONS_SUPPORTED {
            return Err(err(RuntimeMutationErrorCode::UnsupportedBackend));
        }
        if self.transaction_mode() != TransactionMode::StateMachine {
            return Err(err(RuntimeMutationErrorCode::InvalidInput));
        }
        hook.before_commit(self, pending).await?;
        guard.complete();
        Ok(())
    }
    async fn runtime_select(
        &mut self,
        request: &RuntimeMutationRequest,
        projection: &RuntimeProjection,
        key: &[(RuntimeFieldHandle, RuntimeValue)],
        predicate: Option<&RuntimePredicate>,
        lock: bool,
    ) -> Result<Option<RuntimeRecord>, RuntimeMutationError> {
        let mut values = Vec::new();
        let mut condition = key_sql(B::DIALECT, key, &mut values);
        if let Some(predicate) = predicate {
            condition = format!(
                "({condition}) AND ({})",
                predicate.render(B::DIALECT, &mut values)
            );
        }
        if values.len() > request.limits.query.max_bind_parameters {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        let sql = format!(
            "SELECT {} FROM {} WHERE {} LIMIT 2{}",
            columns(B::DIALECT, projection),
            B::DIALECT.quote_identifier(request.collection.physical_table()),
            condition,
            if lock && B::DIALECT == DatabaseBackend::Postgres {
                " FOR UPDATE"
            } else {
                ""
            }
        );
        let rows = self
            .fetch_rows(&sql, &values)
            .await
            .map_err(database_error::<B>)?;
        if rows.len() > 1 {
            return Err(err(RuntimeMutationErrorCode::DatabaseFailed));
        }
        rows.first()
            .map(|row| {
                let record = projection.decode_row::<B>(row)?;
                bounded_record(&record, projection, request.limits)?;
                Ok(record)
            })
            .transpose()
    }
    async fn mutate_runtime_inner(
        &mut self,
        environment: &RuntimeMutationEnvironment,
        request: &RuntimeMutationRequest,
        authority: &dyn RuntimeWriteAuthority<B>,
    ) -> Result<RuntimeMutationEffect, RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        if !B::RUNTIME_MUTATIONS_SUPPORTED
            || B::READ_ONLY
            || !B::RUNTIME_ROW_DECODING_SUPPORTED
            || !matches!(
                B::DIALECT,
                DatabaseBackend::Sqlite | DatabaseBackend::Postgres
            )
        {
            return Err(err(E::UnsupportedBackend));
        }
        if self.transaction_mode() != TransactionMode::StateMachine {
            return Err(err(E::InvalidInput));
        }
        if environment.schema().fingerprint() != request.schema.fingerprint()
            || environment.backend() != B::DIALECT
        {
            return Err(err(E::SchemaMismatch));
        }
        let c = collection(&request.schema, &request.collection)?;
        if request.action == RuntimeMutationAction::Delete
            && environment
                .incoming_dependencies(&c.physical_table)
                .iter()
                .any(|d| !matches!(d.on_delete, DeletePolicy::Restrict))
        {
            return Err(err(E::UnsupportedCascade));
        }
        let intent = RuntimeWriteIntent { request };
        let grant = authority.authorize_intent(intent, self).await?;
        // Validate every host-supplied handle before selecting any protected values.
        request
            .schema
            .resolve_projection(&request.collection, grant.projection.fields())?;
        if let Some(p) = &grant.predicate {
            p.check_mutation_limits(request.limits)?;
        }
        let mut binds = request
            .fields
            .iter()
            .filter(|(_, value)| !matches!(value, RuntimeValue::Null))
            .count();
        if request.action == RuntimeMutationAction::Create {
            binds = binds.saturating_add(c.fields.iter().filter(|field| field.generated).count());
        } else {
            binds = binds.saturating_add(c.primary_key.len());
            for predicate in [grant.predicate.as_ref(), request.expected.as_ref()]
                .into_iter()
                .flatten()
            {
                binds = binds.saturating_add(predicate.mutation_bind_count());
            }
        }
        binds = binds.max(
            c.primary_key.len().saturating_add(
                grant
                    .predicate
                    .as_ref()
                    .map_or(0, RuntimePredicate::mutation_bind_count),
            ),
        );
        if binds > request.limits.query.max_bind_parameters {
            return Err(err(E::LimitExceeded));
        }
        let mut fields = grant.projection.fields().to_vec();
        let key_fields = c
            .primary_key
            .iter()
            .map(|id| request.schema.resolve_field(&request.collection, id))
            .collect::<Result<Vec<_>, _>>()?;
        fields.extend(key_fields.clone());
        fields.extend(request.fields.iter().map(|(f, _)| f.clone()));
        if let Some(expected) = &request.expected {
            fields.extend(expected.referenced_fields().into_iter().cloned());
        }
        if let Some(returning) = &request.returning {
            fields.extend(returning.fields().iter().cloned());
        }
        let mut seen = BTreeSet::new();
        fields.retain(|f| seen.insert(f.id().clone()));
        if fields.len() > request.limits.query.max_projection_fields {
            return Err(err(E::LimitExceeded));
        }
        let projection = request
            .schema
            .resolve_projection(&request.collection, &fields)?;
        let keys_projection = request
            .schema
            .resolve_projection(&request.collection, &key_fields)?;
        let preimage = if let Some(key) = &request.key {
            let record = self
                .runtime_select(
                    request,
                    &projection,
                    &key.fields,
                    grant.predicate.as_ref(),
                    true,
                )
                .await?
                .ok_or_else(|| err(E::NotFound))?;
            authority
                .authorize_preimage(
                    RuntimePreimageCheck {
                        intent,
                        record: &record,
                    },
                    self,
                )
                .await
                .map_err(|error| {
                    if matches!(error.code(), E::Denied | E::FieldDenied) {
                        err(E::NotFound).with_source(error)
                    } else {
                        error
                    }
                })?;
            if let Some(expected) = &request.expected {
                if self
                    .runtime_select(
                        request,
                        &keys_projection,
                        &key.fields,
                        Some(expected),
                        false,
                    )
                    .await?
                    .is_none()
                {
                    return Err(err(E::Conflict));
                }
            }
            Some(record)
        } else {
            None
        };
        let mut values = Vec::new();
        let table = B::DIALECT.quote_identifier(&c.physical_table);
        let sql = match request.action {
            RuntimeMutationAction::Create => {
                let mut entries = request.fields.clone();
                for field in c.fields.iter().filter(|f| f.generated) {
                    entries.push((
                        request
                            .schema
                            .resolve_field(&request.collection, &field.id)?,
                        RuntimeValue::Uuid(uuid::Uuid::new_v4()),
                    ));
                }
                if entries.is_empty() {
                    format!("INSERT INTO {table} DEFAULT VALUES")
                } else {
                    let names = entries
                        .iter()
                        .map(|(f, _)| B::DIALECT.quote_identifier(f.physical_column()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let binds = entries
                        .iter()
                        .map(|(f, v)| bind_value(B::DIALECT, f, v, &mut values))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("INSERT INTO {table} ({names}) VALUES ({binds})")
                }
            }
            RuntimeMutationAction::Update | RuntimeMutationAction::Delete => {
                let assignments = request
                    .fields
                    .iter()
                    .map(|(f, v)| {
                        format!(
                            "{} = {}",
                            B::DIALECT.quote_identifier(f.physical_column()),
                            bind_value(B::DIALECT, f, v, &mut values)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let key = request.key.as_ref().ok_or_else(|| err(E::InvalidInput))?;
                let mut condition = key_sql(B::DIALECT, &key.fields, &mut values);
                for p in [grant.predicate.as_ref(), request.expected.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    condition =
                        format!("({condition}) AND ({})", p.render(B::DIALECT, &mut values));
                }
                if request.action == RuntimeMutationAction::Update {
                    format!("UPDATE {table} SET {assignments} WHERE {condition}")
                } else {
                    format!("DELETE FROM {table} WHERE {condition}")
                }
            }
        };
        if values.len() > request.limits.query.max_bind_parameters {
            return Err(err(E::LimitExceeded));
        }
        let sql = format!("{sql} RETURNING {}", columns(B::DIALECT, &keys_projection));
        let rows = self
            .fetch_rows(&sql, &values)
            .await
            .map_err(database_error::<B>)?;
        if rows.is_empty() {
            return Err(err(E::Conflict));
        }
        if rows.len() != 1 {
            return Err(err(E::DatabaseFailed));
        }
        let actual_keys = keys_projection.decode_row::<B>(&rows[0])?;
        bounded_record(&actual_keys, &keys_projection, request.limits)?;
        let key = key_fields
            .iter()
            .map(|f| actual_keys.value(f).map(|v| (f.clone(), v.clone())))
            .collect::<Result<Vec<_>, _>>()?;
        let result = if request.action == RuntimeMutationAction::Delete {
            preimage.as_ref().ok_or_else(|| err(E::NotFound))?.clone()
        } else {
            self.runtime_select(request, &projection, &key, grant.predicate.as_ref(), false)
                .await?
                .ok_or_else(|| err(E::Denied))?
        };
        let return_grant = authority
            .authorize_result(
                RuntimeResultCheck {
                    intent,
                    preimage: preimage.as_ref(),
                    record: &result,
                },
                self,
            )
            .await?;
        if return_grant.projection != request.returning {
            return Err(err(E::FieldDenied));
        }
        let record = request
            .returning
            .as_ref()
            .map(|p| result.project(p))
            .transpose()?;
        Ok(RuntimeMutationEffect {
            action: request.action,
            record,
        })
    }
}
