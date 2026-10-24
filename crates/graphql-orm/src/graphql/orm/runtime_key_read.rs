//! Checked complete-key reads. Hosts authorize authority, fields and read intent
//! before constructing a request; an owned schema is not an authorization policy.

use super::*;
use std::{error::Error as StdError, fmt};

/// Stable point-read categories, independent of existing exhaustive query enums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeKeyReadErrorCode {
    InvalidHandle,
    InvalidKey,
    InvalidPredicate,
    InvalidProjection,
    LimitExceeded,
    UnsupportedBackend,
    UnexpectedMultiplicity,
    BackendFailure,
}

impl RuntimeKeyReadErrorCode {
    /// Safe machine-readable category.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidHandle => "invalid_handle",
            Self::InvalidKey => "invalid_key",
            Self::InvalidPredicate => "invalid_predicate",
            Self::InvalidProjection => "invalid_projection",
            Self::LimitExceeded => "limit_exceeded",
            Self::UnsupportedBackend => "unsupported_backend",
            Self::UnexpectedMultiplicity => "unexpected_multiplicity",
            Self::BackendFailure => "backend_failure",
        }
    }
}

/// Redacted failure. The source chain is for trusted diagnostics only; formatting
/// never includes SQL, backend details, identifiers, key or predicate values.
pub struct RuntimeKeyReadError {
    code: RuntimeKeyReadErrorCode,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RuntimeKeyReadError {
    fn new(code: RuntimeKeyReadErrorCode) -> Self {
        Self { code, source: None }
    }
    fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }
    /// Safe category. Missing and policy-hidden rows are both successful `None`.
    pub const fn code(&self) -> RuntimeKeyReadErrorCode {
        self.code
    }
}

impl fmt::Debug for RuntimeKeyReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeKeyReadError")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for RuntimeKeyReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime key read error: {}", self.code.as_str())
    }
}
impl StdError for RuntimeKeyReadError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

/// Immutable owned request. Its key retains the validated schema; there is no
/// caller/context borrow, unchecked constructor or deserialization entry.
///
/// Required restriction and optional caller filter obey normal filterability.
/// Identity does not grant public Filter permission. Key columns are returned
/// only if explicitly projected. This low-level API does not invoke static
/// entity/field/row callbacks or establish principal authority.
#[derive(Clone)]
pub struct RuntimeKeyReadRequest {
    key: RuntimeKey,
    projection: RuntimeProjection,
    required_row: RuntimePredicate,
    additional_filter: Option<RuntimePredicate>,
    limits: RuntimeQueryLimits,
}
impl fmt::Debug for RuntimeKeyReadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RuntimeKeyReadRequest([redacted])")
    }
}

fn validate(
    schema: &ValidatedRuntimeSchema,
    key: &RuntimeKey,
    projection: &RuntimeProjection,
    required_row: &RuntimePredicate,
    additional_filter: Option<&RuntimePredicate>,
    limits: RuntimeQueryLimits,
) -> Result<(), RuntimeKeyReadError> {
    use RuntimeKeyReadErrorCode as E;
    if schema.fingerprint() != key.schema().fingerprint() {
        return Err(RuntimeKeyReadError::new(E::InvalidHandle));
    }
    if key.fields().len() > limits.max_bind_parameters {
        return Err(RuntimeKeyReadError::new(E::LimitExceeded));
    }
    key.validate().map_err(|source| {
        let code = match source.code() {
            RuntimeMutationErrorCode::SchemaMismatch => E::InvalidHandle,
            RuntimeMutationErrorCode::LimitExceeded => E::LimitExceeded,
            _ => E::InvalidKey,
        };
        RuntimeKeyReadError::new(code).with_source(source)
    })?;
    if projection.collection() != key.collection()
        || projection.schema_fingerprint() != &schema.fingerprint()
    {
        return Err(RuntimeKeyReadError::new(E::InvalidProjection));
    }
    if projection.fields().is_empty() || projection.fields().len() > limits.max_projection_fields {
        return Err(RuntimeKeyReadError::new(E::LimitExceeded));
    }
    schema
        .resolve_projection(key.collection(), projection.fields())
        .map_err(|source| RuntimeKeyReadError::new(E::InvalidProjection).with_source(source))?;
    // Count the complete conjunction, including internal identity leaves, under
    // this request's limits rather than the limits used to build its handles.
    let mut nodes = key
        .fields()
        .len()
        .checked_add(2)
        .ok_or_else(|| RuntimeKeyReadError::new(E::LimitExceeded))?;
    let mut depth = 3usize; // top conjunction + identity conjunction + equality
    let mut binds = key.fields().len();
    for predicate in std::iter::once(required_row).chain(additional_filter) {
        if !predicate.belongs_to(&schema.fingerprint(), key.collection().id()) {
            return Err(RuntimeKeyReadError::new(E::InvalidPredicate));
        }
        let (predicate_nodes, predicate_depth, predicate_binds) = predicate
            .key_read_budget(limits)
            .ok_or_else(|| RuntimeKeyReadError::new(E::LimitExceeded))?;
        for field in predicate.referenced_fields() {
            let current = schema
                .resolve_field(key.collection(), field.id())
                .map_err(|source| {
                    RuntimeKeyReadError::new(E::InvalidPredicate).with_source(source)
                })?;
            if &current != field || !field.filterable() {
                return Err(RuntimeKeyReadError::new(E::InvalidPredicate));
            }
        }
        nodes = nodes
            .checked_add(predicate_nodes)
            .ok_or_else(|| RuntimeKeyReadError::new(E::LimitExceeded))?;
        binds = binds
            .checked_add(predicate_binds)
            .ok_or_else(|| RuntimeKeyReadError::new(E::LimitExceeded))?;
        depth = depth.max(
            predicate_depth
                .checked_add(1)
                .ok_or_else(|| RuntimeKeyReadError::new(E::LimitExceeded))?,
        );
    }
    if nodes > limits.max_predicate_nodes
        || depth > limits.max_predicate_depth
        || binds > limits.max_bind_parameters
    {
        return Err(RuntimeKeyReadError::new(E::LimitExceeded));
    }
    Ok(())
}

impl ValidatedRuntimeSchema {
    /// Construct a checked point read with a mandatory host-required restriction.
    /// Hosts authenticate and authorize the exact fields/intent before this call.
    /// An explicitly constructed true predicate is permitted for trusted hosts;
    /// it does not prove that the host performed authorization.
    ///
    /// Projection/predicate/bind limits include the complete identity conjunction.
    /// Paging/cursor settings are unused: execution always fetches at most two
    /// candidates to detect multiplicity, returning at most one record.
    pub fn runtime_key_read_request(
        &self,
        key: &RuntimeKey,
        projection: &RuntimeProjection,
        required_row: &RuntimePredicate,
        additional_filter: Option<&RuntimePredicate>,
        limits: RuntimeQueryLimits,
    ) -> Result<RuntimeKeyReadRequest, RuntimeKeyReadError> {
        validate(
            self,
            key,
            projection,
            required_row,
            additional_filter,
            limits,
        )?;
        Ok(RuntimeKeyReadRequest {
            key: key.clone(),
            projection: projection.clone(),
            required_row: required_row.clone(),
            additional_filter: additional_filter.cloned(),
            limits,
        })
    }
}

impl<B: OrmBackend + RuntimeRowDecoder> crate::db::Database<B> {
    /// Perform one current SELECT using existing auth-context/native RLS support.
    /// Missing and hidden rows both return `None`; duplicates fail without any
    /// record disclosure. Projection is exact. No retry, write or journal occurs.
    ///
    /// The caller supplies coherent trusted `auth`, authorizes fields/read intent
    /// and constructs the required row restriction. This is not a static policy
    /// wrapper. Cancellation uses the existing backend read/connection cleanup;
    /// no stronger snapshot or cross-call consistency guarantee is introduced.
    pub async fn execute_runtime_key_read(
        &self,
        request: &RuntimeKeyReadRequest,
        auth: Option<&DbAuthContext>,
    ) -> Result<Option<RuntimeRecord>, RuntimeKeyReadError> {
        use RuntimeKeyReadErrorCode as E;
        if !B::RUNTIME_ROW_DECODING_SUPPORTED
            || !matches!(
                B::DIALECT,
                DatabaseBackend::Sqlite | DatabaseBackend::Postgres
            )
        {
            return Err(RuntimeKeyReadError::new(E::UnsupportedBackend));
        }
        validate(
            request.key.schema(),
            &request.key,
            &request.projection,
            &request.required_row,
            request.additional_filter.as_ref(),
            request.limits,
        )?;
        let mut values = Vec::new();
        let mut conditions = vec![super::runtime_mutation::key_sql(
            B::DIALECT,
            request.key.fields(),
            &mut values,
        )];
        conditions.push(request.required_row.render(B::DIALECT, &mut values));
        if let Some(filter) = &request.additional_filter {
            conditions.push(filter.render(B::DIALECT, &mut values));
        }
        if values.len() > request.limits.max_bind_parameters {
            return Err(RuntimeKeyReadError::new(E::LimitExceeded));
        }
        let columns = request
            .projection
            .fields()
            .iter()
            .map(|field| B::DIALECT.quote_identifier(field.physical_column()))
            .collect::<Vec<_>>()
            .join(", ");
        let condition = conditions
            .into_iter()
            .map(|part| format!("({part})"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!(
            "SELECT {columns} FROM {} WHERE {condition} LIMIT 2",
            B::DIALECT.quote_identifier(request.key.collection().physical_table())
        );
        let rows = super::fetch_rows_with_auth::<B>(self.pool(), &sql, &values, auth)
            .await
            .map_err(|source| RuntimeKeyReadError::new(E::BackendFailure).with_source(source))?;
        if rows.len() > 1 {
            return Err(RuntimeKeyReadError::new(E::UnexpectedMultiplicity));
        }
        rows.first()
            .map(|row| {
                request.projection.decode_row::<B>(row).map_err(|source| {
                    RuntimeKeyReadError::new(E::BackendFailure).with_source(source)
                })
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Clone, Copy)]
    struct Probe<const SUPPORTED: bool>;
    #[derive(Default)]
    struct State {
        calls: AtomicUsize,
        dropped: AtomicBool,
        wait: bool,
        fail: bool,
        sql: Mutex<String>,
        rows: Vec<String>,
    }
    impl<const SUPPORTED: bool> OrmBackend for Probe<SUPPORTED> {
        type Pool = Arc<State>;
        type Row = String;
        const DIALECT: DatabaseBackend = if SUPPORTED {
            DatabaseBackend::Sqlite
        } else {
            DatabaseBackend::Mssql
        };
        const READ_ONLY: bool = false;
        fn fetch_rows<'a>(
            pool: &'a Self::Pool,
            sql: &'a str,
            _: &'a [SqlValue],
        ) -> BoxFuture<'a, crate::Result<Vec<Self::Row>>> {
            Box::pin(async move {
                pool.calls.fetch_add(1, Ordering::SeqCst);
                *pool.sql.lock().unwrap() = sql.into();
                struct Guard(Arc<State>);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.0.dropped.store(true, Ordering::SeqCst);
                    }
                }
                let _guard = Guard(pool.clone());
                if pool.wait {
                    std::future::pending::<()>().await;
                }
                if pool.fail {
                    return Err(sqlx::Error::Protocol(
                        "SECRET SQL SELECT FROM hidden_table".into(),
                    ));
                }
                Ok(pool.rows.clone())
            })
        }
        fn try_get_i64(_: &Self::Row, _: &str) -> crate::Result<i64> {
            unreachable!()
        }
        fn try_get_f64(_: &Self::Row, _: &str) -> crate::Result<f64> {
            unreachable!()
        }
        fn try_get_string(row: &Self::Row, _: &str) -> crate::Result<String> {
            Ok(row.clone())
        }
        fn try_get_optional_i64(_: &Self::Row, _: &str) -> crate::Result<Option<i64>> {
            unreachable!()
        }
        fn try_get_optional_f64(_: &Self::Row, _: &str) -> crate::Result<Option<f64>> {
            unreachable!()
        }
        fn try_get_optional_string(row: &Self::Row, _: &str) -> crate::Result<Option<String>> {
            Ok(Some(row.clone()))
        }
    }
    impl<const SUPPORTED: bool> RuntimeRowDecoder for Probe<SUPPORTED> {
        const RUNTIME_ROW_DECODING_SUPPORTED: bool = SUPPORTED;
        fn decode_runtime_value(
            row: &Self::Row,
            _: &RuntimeFieldHandle,
        ) -> Result<RuntimeValue, RuntimeRecordError> {
            Ok(RuntimeValue::String(row.clone()))
        }
    }
    struct Deny;
    impl<const SUPPORTED: bool> EntityPolicy<Probe<SUPPORTED>> for Deny {
        fn can_access_entity<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            _: &'a crate::db::Database<Probe<SUPPORTED>>,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessKind,
            _: EntityAccessSurface,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(false) })
        }
    }
    fn request() -> RuntimeKeyReadRequest {
        let field = |name: &str, kind| RuntimeField {
            description: None,
            classification: None,
            export: None,
            id: FieldId::new(name).unwrap(),
            api_name: name.into(),
            physical_column: name.into(),
            value_kind: kind,
            nullable: false,
            unique: false,
            filterable: false,
            sortable: false,
            generated: false,
            default: None,
            format: None,
        };
        let schema = RuntimeSchema {
            format_version: RUNTIME_SCHEMA_FORMAT_VERSION,
            collections: vec![RuntimeCollection {
                description: None,
                classification: None,
                id: CollectionId::new("rows").unwrap(),
                api_type_name: "Row".into(),
                api_plural_name: "Rows".into(),
                physical_table: "point_rows".into(),
                primary_key: vec![FieldId::new("id").unwrap()],
                append_only: false,
                retention_purge: false,
                fields: vec![
                    field("id", RuntimeValueKind::Integer),
                    field("label", RuntimeValueKind::String),
                ],
                relations: vec![],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![],
                label_fields: Vec::new(),
            }],
        }
        .validate()
        .unwrap();
        let collection = schema
            .resolve_collection(&CollectionId::new("rows").unwrap())
            .unwrap();
        let id = schema
            .resolve_field(&collection, &FieldId::new("id").unwrap())
            .unwrap();
        let label = schema
            .resolve_field(&collection, &FieldId::new("label").unwrap())
            .unwrap();
        let key = schema
            .runtime_key(&collection, &[(id, RuntimeValue::Integer(42))])
            .unwrap();
        let projection = schema.resolve_projection(&collection, &[label]).unwrap();
        let row = schema
            .runtime_and(&collection, vec![], Default::default())
            .unwrap();
        schema
            .runtime_key_read_request(&key, &projection, &row, None, Default::default())
            .unwrap()
    }
    #[tokio::test]
    async fn unsupported_decoder_and_corrupted_request_fail_before_backend_io() {
        let state = Arc::new(State::default());
        let unsupported =
            crate::db::Database::<Probe<false>>::with_entity_policy(state.clone(), Deny);
        let mut request = request();
        assert_eq!(
            unsupported
                .execute_runtime_key_read(&request, None)
                .await
                .unwrap_err()
                .code(),
            RuntimeKeyReadErrorCode::UnsupportedBackend
        );
        let db = crate::db::Database::<Probe<true>>::with_entity_policy(state.clone(), Deny);
        request.limits.max_bind_parameters = 0;
        assert_eq!(
            db.execute_runtime_key_read(&request, None)
                .await
                .unwrap_err()
                .code(),
            RuntimeKeyReadErrorCode::LimitExceeded
        );
        assert_eq!(state.calls.load(Ordering::SeqCst), 0);
        #[cfg(feature = "mssql")]
        assert_eq!(
            MssqlBackend::RUNTIME_ROW_DECODING_SUPPORTED,
            Probe::<false>::RUNTIME_ROW_DECODING_SUPPORTED
        );
    }
    #[cfg(feature = "mssql")]
    #[tokio::test]
    async fn actual_mssql_pool_is_rejected_without_acquisition() {
        // Even a regression must target only this test's owned loopback socket,
        // never a default SQL Server endpoint or an application database.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut config = crate::tiberius::Config::new();
        config.host("127.0.0.1");
        config.port(listener.local_addr().unwrap().port());
        let pool = crate::db::mssql::MssqlPool::new(config);
        let db = crate::db::Database::<MssqlBackend>::new(pool);
        let request = request();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            db.execute_runtime_key_read(&request, None),
        )
        .await
        .expect("unsupported MSSQL must reject before any acquisition wait");
        assert_eq!(
            result.unwrap_err().code(),
            RuntimeKeyReadErrorCode::UnsupportedBackend
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[tokio::test]
    async fn cancellation_drops_the_one_read_without_retry() {
        let state = Arc::new(State {
            wait: true,
            ..Default::default()
        });
        let db = crate::db::Database::<Probe<true>>::with_entity_policy(state.clone(), Deny);
        let request = request();
        let mut future = Box::pin(db.execute_runtime_key_read(&request, None));
        assert!(matches!(
            futures::poll!(&mut future),
            std::task::Poll::Pending
        ));
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        drop(future);
        assert!(state.dropped.load(Ordering::SeqCst));
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn exact_select_and_backend_sources_remain_redacted() {
        let state = Arc::new(State {
            fail: true,
            ..Default::default()
        });
        let db = crate::db::Database::<Probe<true>>::with_entity_policy(state.clone(), Deny);
        let error = db
            .execute_runtime_key_read(&request(), None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), RuntimeKeyReadErrorCode::BackendFailure);
        assert!(StdError::source(&error).is_some());
        assert!(!format!("{error} {error:?}").contains("SECRET"));
        let sql = state.sql.lock().unwrap();
        assert!(sql.starts_with("SELECT \"label\" FROM "));
        assert!(sql.ends_with(" LIMIT 2"));
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}
