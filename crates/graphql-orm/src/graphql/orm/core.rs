use super::dialect::DatabaseBackend;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::dialect::SqlDialect;
use super::query::{
    ChangeAction, DatabaseEntity, DatabaseSchema, DatabaseSearchSchema, EntityRelations,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::query::{DatabaseFilter, DatabaseOrderBy, Entity, EntityQuery, FromSqlRow};
use super::{DefaultBackend, OrmBackend};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::{DefaultWriteBackend, WriteBackend};
use crate::graphql::auth::{AuthExt, AuthSubject};
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static QUERY_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Runtime SQL bind value used by generated queries and mutation helpers.
///
/// The typed `*Null` variants let generated code preserve the intended
/// database type for nullable fields when a backend requires typed null binds.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    /// Text value.
    String(String),
    /// Typed null for text-like values.
    StringNull,
    /// Binary value.
    Bytes(Vec<u8>),
    /// Typed null for binary values.
    BytesNull,
    /// JSON value.
    Json(serde_json::Value),
    /// Typed null for JSON values.
    JsonNull,
    /// UUID value.
    Uuid(uuid::Uuid),
    /// Typed null for UUID values.
    UuidNull,
    /// Integer value represented as `i64`.
    Int(i64),
    /// Typed null for integer values.
    IntNull,
    /// Floating point value represented as `f64`.
    Float(f64),
    /// Typed null for floating point values.
    FloatNull,
    /// Boolean value.
    Bool(bool),
    /// Typed null for boolean values.
    BoolNull,
    /// Untyped null used when a generated path has no more specific type.
    Null,
}

/// Produce a stable internal event/search identifier for a binary entity key.
///
/// SQL bindings always retain the original bytes; this encoding is used only
/// where existing hook/event contracts require a textual identifier.
#[doc(hidden)]
pub fn binary_key_id(value: &[u8]) -> String {
    let mut encoded = String::with_capacity(4 + value.len() * 2);
    encoded.push_str("bin:");
    for byte in value {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// Produce an unambiguous textual event identifier for an ordered composite key.
#[doc(hidden)]
pub fn composite_key_id(values: &[SqlValue]) -> String {
    fn component(value: &SqlValue) -> String {
        match value {
            SqlValue::String(value) => format!("s{}:{value}", value.len()),
            SqlValue::Bytes(value) => binary_key_id(value),
            SqlValue::Uuid(value) => format!("u:{value}"),
            SqlValue::Int(value) => format!("i:{value}"),
            SqlValue::Float(value) => format!("f:{:016x}", value.to_bits()),
            SqlValue::Bool(value) => format!("b:{value}"),
            SqlValue::Json(value) => {
                let encoded = serde_json::to_string(value).unwrap_or_default();
                format!("j{}:{encoded}", encoded.len())
            }
            SqlValue::StringNull
            | SqlValue::BytesNull
            | SqlValue::JsonNull
            | SqlValue::UuidNull
            | SqlValue::IntNull
            | SqlValue::FloatNull
            | SqlValue::BoolNull
            | SqlValue::Null => "n".to_string(),
        }
    }

    let mut id = String::from("key:");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            id.push('|');
        }
        id.push_str(&component(value));
    }
    id
}

/// Request-local database authorization context for PostgreSQL RLS and
/// structural multi-tenant predicates.
///
/// Attach this value to an `async-graphql` request with `request.data(...)`.
/// Generated PostgreSQL resolvers read it from the GraphQL context and apply it
/// as transaction-local `app.*` settings before executing database work.
///
/// Under [`crate::graphql::auth::AuthorizationMode::DeclaredPoliciesRequired`]
/// (and stricter modes), entities that declare tenant/RLS metadata fail closed
/// when this context is missing.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DbAuthContext {
    /// Application user identifier exposed as `app.user_id`.
    pub user_id: Option<String>,
    /// Auth subject exposed as `app.subject`.
    pub subject: Option<String>,
    /// Tenant identifier exposed as `app.tenant_id`.
    pub tenant_id: Option<String>,
    /// Application roles serialized to `app.roles`.
    pub roles: Vec<String>,
    /// Application scopes serialized to `app.scopes`.
    pub scopes: Vec<String>,
    /// Optional JSON claims serialized to `app.claims`.
    ///
    /// Never store raw tokens here. Claim bodies are redacted in [`Debug`] and
    /// contribute only a fingerprint to cache partition keys.
    pub claims_json: Option<serde_json::Value>,
    /// Optional token reference (`jti` / token id), never a raw secret.
    pub token_id: Option<String>,
    /// Optional session reference.
    pub session_id: Option<String>,
    /// Optional actor/on-behalf-of identity.
    pub actor_id: Option<String>,
    /// Optional organization identifier when distinct from tenant.
    pub organization_id: Option<String>,
    /// Optional authorization/audit correlation identifier.
    pub correlation_id: Option<String>,
    /// Optional host-accepted, session-bound authentication assurance.
    pub assurance: Option<crate::graphql::auth::AuthAssurance>,
    /// Optional policy-version stamp for cache invalidation.
    pub policy_version: Option<String>,
}

impl std::fmt::Debug for DbAuthContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbAuthContext")
            .field("user_id", &self.user_id)
            .field("subject", &self.subject)
            .field("tenant_id", &self.tenant_id)
            .field("roles_len", &self.roles.len())
            .field("scopes_len", &self.scopes.len())
            .field(
                "claims_json",
                &self.claims_json.as_ref().map(|_| "[redacted]"),
            )
            .field("token_id", &self.token_id)
            .field("session_id", &self.session_id)
            .field("actor_id", &self.actor_id)
            .field("organization_id", &self.organization_id)
            .field("correlation_id", &self.correlation_id)
            .field("assurance", &self.assurance)
            .field("policy_version", &self.policy_version)
            .finish()
    }
}

impl DbAuthContext {
    /// Build a database auth context from a request auth subject.
    ///
    /// The subject id is exposed as both `app.user_id` and `app.subject` for
    /// PostgreSQL RLS compatibility. Roles, scopes, and tenant id are copied
    /// directly.
    pub fn from_subject(subject: &AuthSubject) -> Self {
        Self {
            user_id: subject.user_id.clone().or_else(|| Some(subject.id.clone())),
            subject: Some(subject.id.clone()),
            tenant_id: subject.tenant_id.clone(),
            roles: subject.roles.clone(),
            scopes: subject.scopes.clone(),
            claims_json: subject.claims.clone(),
            token_id: subject.token_id.clone(),
            session_id: subject.session_id.clone(),
            actor_id: subject.actor_id.clone(),
            organization_id: subject.organization_id.clone(),
            correlation_id: subject.correlation_id.clone(),
            assurance: subject.assurance.clone(),
            policy_version: None,
        }
    }

    /// Build a database auth context from explicit parts.
    pub fn from_parts(
        user_id: impl Into<String>,
        roles: Vec<String>,
        scopes: Vec<String>,
        tenant_id: Option<String>,
    ) -> Self {
        let user_id = user_id.into();
        Self {
            user_id: Some(user_id.clone()),
            subject: Some(user_id),
            tenant_id,
            roles,
            scopes,
            claims_json: None,
            token_id: None,
            session_id: None,
            actor_id: None,
            organization_id: None,
            correlation_id: None,
            assurance: None,
            policy_version: None,
        }
    }

    /// Build a database auth context from explicit parts including a distinct
    /// SQL subject and optional claims JSON.
    pub fn from_context_parts(
        user_id: Option<String>,
        subject: Option<String>,
        tenant_id: Option<String>,
        roles: Vec<String>,
        scopes: Vec<String>,
        claims_json: Option<serde_json::Value>,
    ) -> Self {
        Self {
            user_id,
            subject,
            tenant_id,
            roles,
            scopes,
            claims_json,
            token_id: None,
            session_id: None,
            actor_id: None,
            organization_id: None,
            correlation_id: None,
            assurance: None,
            policy_version: None,
        }
    }

    /// Attach a policy-version stamp used for DataLoader cache partitioning.
    pub fn with_policy_version(mut self, policy_version: impl Into<String>) -> Self {
        self.policy_version = Some(policy_version.into());
        self
    }

    /// Return a stable key for batching and cache partitioning.
    ///
    /// Roles and scopes are sorted so equivalent contexts do not batch
    /// separately due only to caller ordering. Claim JSON is fingerprinted
    /// rather than embedded so cache keys do not retain sensitive claim bodies.
    pub fn canonical_key(&self) -> String {
        let mut roles = self.roles.clone();
        roles.sort();
        let mut scopes = self.scopes.clone();
        scopes.sort();
        let claims_fp = self
            .claims_json
            .as_ref()
            .map(|value| {
                let encoded = value.to_string();
                let mut hash: u64 = 0xcbf29ce484222325;
                for byte in encoded.as_bytes() {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(0x100000001b3);
                }
                format!("{hash:016x}")
            })
            .unwrap_or_default();
        format!(
            "user={}|subject={}|tenant={}|organization={}|roles={}|scopes={}|token={}|session={}|actor={}|correlation={}|assurance={:?}|policy={}|claims_fp={}",
            self.user_id.as_deref().unwrap_or(""),
            self.subject.as_deref().unwrap_or(""),
            self.tenant_id.as_deref().unwrap_or(""),
            self.organization_id.as_deref().unwrap_or(""),
            roles.join(","),
            scopes.join(","),
            self.token_id.as_deref().unwrap_or(""),
            self.session_id.as_deref().unwrap_or(""),
            self.actor_id.as_deref().unwrap_or(""),
            self.correlation_id.as_deref().unwrap_or(""),
            self.assurance,
            self.policy_version.as_deref().unwrap_or(""),
            claims_fp,
        )
    }

    /// Render PostgreSQL setting names and values for transaction-local auth.
    ///
    /// The returned values are intended for `set_config(name, value, true)`.
    pub fn postgres_settings(&self) -> crate::Result<Vec<(&'static str, String)>> {
        let roles = serde_json::to_string(&self.roles)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        let scopes = serde_json::to_string(&self.scopes)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
        let claims = self
            .claims_json
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?
            .unwrap_or_default();
        let assurance = self
            .assurance
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?
            .unwrap_or_default();

        Ok(vec![
            ("app.user_id", self.user_id.clone().unwrap_or_default()),
            ("app.subject", self.subject.clone().unwrap_or_default()),
            ("app.tenant_id", self.tenant_id.clone().unwrap_or_default()),
            ("app.roles", roles),
            ("app.scopes", scopes),
            ("app.claims", claims),
            (
                "app.organization_id",
                self.organization_id.clone().unwrap_or_default(),
            ),
            (
                "app.correlation_id",
                self.correlation_id.clone().unwrap_or_default(),
            ),
            ("app.assurance", assurance),
            (
                "app.policy_version",
                self.policy_version.clone().unwrap_or_default(),
            ),
        ])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MutationPhase {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOrigin {
    GraphqlMutation,
    Repository,
    InternalMutationHook,
}

/// Isolation intent for an ORM-managed transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransactionMode {
    /// Backend default isolation, suitable when decisions do not race writers.
    #[default]
    Default,
    /// Serialize state-machine decisions against concurrent writers.
    ///
    /// This uses serializable isolation on PostgreSQL and eagerly acquires the
    /// SQLite write lock before the callback can perform its first read.
    StateMachine,
}

/// Safe classification of an ORM-managed transaction failure.
#[derive(Clone, Debug)]
pub enum TransactionError {
    /// The callback rejected the operation. The transaction was rolled back.
    Rejected(crate::graphql::errors::OrmPublicError),
    /// The whole callback may be retried from the beginning.
    Retryable(crate::graphql::errors::OrmPublicError),
    /// A non-retryable database or commit failure.
    Failed(crate::graphql::errors::OrmPublicError),
}

/// Result of a single-statement typed conditional update.
#[derive(Clone, Debug, PartialEq)]
pub enum ConditionalUpdateOutcome<T> {
    /// No row with the supplied primary key exists (or is visible).
    NotFound,
    /// The row exists but one or more expected predicates did not match.
    Conflict,
    /// Exactly one row was updated and returned.
    Updated(T),
}

/// Result of an atomic key-plus-typed-predicate update.
#[derive(Clone, Debug, PartialEq)]
pub enum PredicateUpdateOutcome<T> {
    /// No row with the complete key exists or is visible.
    NotFound,
    /// The key exists, but the additional typed predicate did not match.
    PredicateConflict,
    /// Exactly one row was updated and returned.
    Updated(T),
}

/// Result of an insert that uses a declared primary/unique conflict target.
#[derive(Clone, Debug, PartialEq)]
pub enum InsertIfAbsentOutcome<T> {
    /// The row was inserted by this operation.
    Inserted(T),
    /// A row with the declared conflict target was already present.
    AlreadyPresent(T),
}

/// Explicit upper bound for one typed bulk mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutationLimit(u32);

impl MutationLimit {
    /// Construct a non-zero affected-row ceiling.
    pub fn new(maximum: u32) -> Result<Self, crate::graphql::errors::OrmPublicError> {
        if maximum == 0 {
            return Err(crate::graphql::errors::OrmPublicError::new(
                crate::graphql::errors::OrmErrorCode::InvalidInput,
            )
            .with_internal("bulk mutation maximum must be greater than zero"));
        }
        Ok(Self(maximum))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Outcome of a bounded typed bulk mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoundedMutationOutcome {
    /// The mutation completed without crossing its explicit ceiling.
    Applied { affected: u32 },
    /// More rows matched than the caller authorized; no rows were changed.
    LimitExceeded { maximum: u32 },
}

/// Outcome of one bounded append-only retention purge.
///
/// A limit overflow is detected before any row is deleted. The surrounding
/// retention transaction remains usable so the host may record or report the
/// rejected maintenance attempt explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionPurgeOutcome {
    /// Every matching row was deleted and `affected` is exact.
    Purged { affected: u32 },
    /// More rows matched than the caller-authorized ceiling; nothing changed.
    LimitExceeded { maximum: u32 },
}

/// Redacted post-commit notification for one successful bounded purge.
///
/// Row contents and predicate values are intentionally absent. Hosts that
/// require durable audit evidence should append their own managed audit entity
/// inside the same retention transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionPurgeEvent {
    /// Rust entity type name from managed metadata.
    pub entity_name: &'static str,
    /// Managed physical table name from generated metadata.
    pub table_name: &'static str,
    /// Exact number of rows deleted by the committed purge.
    pub affected: u32,
}

impl TransactionError {
    pub fn public_error(&self) -> &crate::graphql::errors::OrmPublicError {
        match self {
            Self::Rejected(error) | Self::Retryable(error) | Self::Failed(error) => error,
        }
    }

    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.public_error().fmt(formatter)
    }
}

impl std::error::Error for TransactionError {}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationFieldValue {
    pub field: String,
    pub value: SqlValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpsertOutcome<T> {
    pub action: ChangeAction,
    pub entity: T,
}

#[derive(Clone)]
pub struct EntityState {
    value: Arc<dyn Any + Send + Sync>,
    json: serde_json::Value,
}

impl EntityState {
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.value.downcast_ref::<T>()
    }

    pub fn as_json(&self) -> &serde_json::Value {
        &self.json
    }
}

#[derive(Clone)]
pub struct MutationEvent {
    pub phase: MutationPhase,
    pub action: ChangeAction,
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub metadata: &'static EntityMetadata,
    pub id: String,
    pub changes: Vec<MutationFieldValue>,
    pub before_state: Option<EntityState>,
    pub after_state: Option<EntityState>,
}

impl MutationEvent {
    pub fn before<T: 'static>(&self) -> async_graphql::Result<Option<&T>> {
        self.before_state
            .as_ref()
            .map(|state| {
                state.downcast_ref::<T>().ok_or_else(|| {
                    async_graphql::Error::new(format!(
                        "Mutation before_state for {} is not {}",
                        self.entity_name,
                        std::any::type_name::<T>()
                    ))
                })
            })
            .transpose()
    }

    pub fn after<T: 'static>(&self) -> async_graphql::Result<Option<&T>> {
        self.after_state
            .as_ref()
            .map(|state| {
                state.downcast_ref::<T>().ok_or_else(|| {
                    async_graphql::Error::new(format!(
                        "Mutation after_state for {} is not {}",
                        self.entity_name,
                        std::any::type_name::<T>()
                    ))
                })
            })
            .transpose()
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
trait DeferredEventEmitter<B: OrmBackend>: Send {
    fn emit(self: Box<Self>, db: &crate::db::Database<B>);
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
trait PostCommitActionRunner<B: OrmBackend>: Send {
    fn run(
        self: Box<Self>,
        db: crate::db::Database<B>,
    ) -> futures::future::BoxFuture<'static, Result<(), String>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
struct DeferredEvent<T>
where
    T: Clone + Send + Sync + 'static,
{
    event: T,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<T, B> DeferredEventEmitter<B> for DeferredEvent<T>
where
    B: OrmBackend,
    T: Clone + Send + Sync + 'static,
{
    fn emit(self: Box<Self>, db: &crate::db::Database<B>) {
        db.emit_event(self.event);
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
struct DeferredAction<F> {
    action: Option<F>,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<F, Fut, E, B> PostCommitActionRunner<B> for DeferredAction<F>
where
    B: OrmBackend,
    F: FnOnce(crate::db::Database<B>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    fn run(
        mut self: Box<Self>,
        db: crate::db::Database<B>,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        let action = self
            .action
            .take()
            .expect("deferred post-commit action was already taken");
        Box::pin(async move { action(db).await.map_err(|error| error.to_string()) })
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub struct MutationContext<'tx, B: WriteBackend = DefaultWriteBackend> {
    db: &'tx crate::db::Database<B>,
    tx: sqlx::Transaction<'tx, B::Database>,
    deferred_events: Vec<Box<dyn DeferredEventEmitter<B>>>,
    deferred_actions: Vec<Box<dyn PostCommitActionRunner<B>>>,
}

/// Narrow transaction-bound capability for regulated retention maintenance.
///
/// Values of this type can only be created by
/// [`Database::retention_transaction`](crate::db::Database::retention_transaction)
/// or its auth-aware counterpart. It intentionally exposes append/query
/// operations and generated bounded purge, but no raw executor or ordinary
/// update/delete surface.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub struct RetentionContext<'tx, B: WriteBackend = DefaultWriteBackend> {
    mutation: MutationContext<'tx, B>,
    poisoned: bool,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub struct MutationQuery<'ctx, 'tx, T, B: WriteBackend = DefaultWriteBackend>
where
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    hook_ctx: &'ctx mut MutationContext<'tx, B>,
    query: EntityQuery<T, B>,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<'ctx, 'tx, T, B> MutationQuery<'ctx, 'tx, T, B>
where
    B: WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    fn new(hook_ctx: &'ctx mut MutationContext<'tx, B>) -> Self {
        let resolved = hook_ctx
            .database()
            .pagination_config()
            .resolve_page(None, true);
        Self {
            hook_ctx,
            query: EntityQuery::new().paginate(&super::query::PageInput {
                limit: resolved.limit,
                offset: Some(resolved.offset),
            }),
        }
    }

    pub fn filter<F>(mut self, filter: F) -> Self
    where
        F: DatabaseFilter,
    {
        self.query = self.query.filter(&filter);
        self
    }

    pub fn order_by<O>(mut self, order: O) -> Self
    where
        O: DatabaseOrderBy,
    {
        self.query = self.query.order_by(&order);
        self
    }

    pub fn default_order(mut self) -> Self {
        self.query = self.query.default_order();
        self
    }

    pub fn limit(mut self, limit: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.limit = self
            .hook_ctx
            .database()
            .pagination_config()
            .clamp_explicit_limit(Some(limit));
        self.query.page = Some(page);
        self
    }

    pub fn offset(mut self, offset: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.offset = Some(offset.max(0));
        self.query.page = Some(page);
        self
    }

    pub fn paginate(mut self, page: super::query::PageInput) -> Self {
        let resolved = self
            .hook_ctx
            .database()
            .pagination_config()
            .resolve_page(Some(&page), true);
        self.query = self.query.paginate(&super::query::PageInput {
            limit: resolved.limit,
            offset: Some(resolved.offset),
        });
        self
    }

    pub async fn fetch_all(self) -> crate::Result<Vec<T>> {
        let metadata = T::metadata();
        self.hook_ctx
            .database()
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
        let query = self.query;
        let rows = query.fetch_all_on(self.hook_ctx.executor()).await?;
        authorize_repository_rows::<T, B>(self.hook_ctx.database(), rows).await
    }

    pub async fn fetch_one(self) -> crate::Result<Option<T>> {
        let metadata = T::metadata();
        self.hook_ctx
            .database()
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
        let query = self.query;
        let row = query.fetch_one_on(self.hook_ctx.executor()).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if authorize_repository_row::<T, B>(self.hook_ctx.database(), &row).await? {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    pub async fn count(self) -> crate::Result<i64> {
        let metadata = T::metadata();
        self.hook_ctx
            .database()
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
        if self.hook_ctx.database().row_policy().is_some() {
            return Err(sqlx::Error::Protocol(
                "repository count reads require database-visible row-policy predicates".to_string(),
            ));
        }
        let query = self.query;
        query.count_on(self.hook_ctx.executor()).await
    }

    pub async fn exists(self) -> crate::Result<bool> {
        Ok(self.count().await? > 0)
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub struct WriteInputContext<'ctx, 'tx, B: WriteBackend = DefaultWriteBackend> {
    graphql_ctx: Option<&'ctx async_graphql::Context<'ctx>>,
    entity_name: &'static str,
    origin: WriteOrigin,
    database: Option<&'ctx crate::db::Database<B>>,
    mutation_ctx: Option<&'ctx mut MutationContext<'tx, B>>,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub struct WriteQuery<'ctx, 'write, 'tx, T, B: WriteBackend = DefaultWriteBackend>
where
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    write_ctx: &'ctx mut WriteInputContext<'write, 'tx, B>,
    query: EntityQuery<T, B>,
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<'ctx, 'write, B> WriteInputContext<'ctx, 'write, B>
where
    B: WriteBackend,
{
    pub fn graphql(
        db: &'ctx crate::db::Database<B>,
        ctx: &'ctx async_graphql::Context<'ctx>,
        entity_name: &'static str,
    ) -> Self {
        Self {
            graphql_ctx: Some(ctx),
            entity_name,
            origin: WriteOrigin::GraphqlMutation,
            database: Some(db),
            mutation_ctx: None,
        }
    }

    pub fn repository(db: &'ctx crate::db::Database<B>, entity_name: &'static str) -> Self {
        Self {
            graphql_ctx: None,
            entity_name,
            origin: WriteOrigin::Repository,
            database: Some(db),
            mutation_ctx: None,
        }
    }

    pub fn internal(
        entity_name: &'static str,
        hook_ctx: &'ctx mut MutationContext<'write, B>,
    ) -> Self {
        Self {
            graphql_ctx: None,
            entity_name,
            origin: WriteOrigin::InternalMutationHook,
            database: None,
            mutation_ctx: Some(hook_ctx),
        }
    }

    pub fn graphql_ctx(&self) -> Option<&async_graphql::Context<'ctx>> {
        self.graphql_ctx
    }

    pub fn entity_name(&self) -> &'static str {
        self.entity_name
    }

    pub fn origin(&self) -> WriteOrigin {
        self.origin
    }

    pub fn database(&self) -> &crate::db::Database<B> {
        if let Some(db) = self.database {
            db
        } else {
            self.mutation_ctx
                .as_deref()
                .expect("write input context missing database and mutation context")
                .database()
        }
    }

    pub fn actor<T>(&self) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.graphql_ctx
            .and_then(|ctx| ctx.data_opt::<T>())
            .cloned()
    }

    pub fn auth_user(&self) -> async_graphql::Result<String> {
        self.graphql_ctx
            .ok_or_else(|| async_graphql::Error::new("missing GraphQL context for auth user"))?
            .auth_user_id()
    }

    pub fn auth_subject(&self) -> async_graphql::Result<AuthSubject> {
        self.graphql_ctx
            .ok_or_else(|| async_graphql::Error::new("missing GraphQL context for auth subject"))?
            .auth_subject()
    }

    pub fn query<'a, T>(&'a mut self) -> WriteQuery<'a, 'ctx, 'write, T, B>
    where
        T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        WriteQuery {
            write_ctx: self,
            query: EntityQuery::new(),
        }
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<'ctx, 'write, 'tx, T, B> WriteQuery<'ctx, 'write, 'tx, T, B>
where
    B: WriteBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    pub fn filter<F>(mut self, filter: F) -> Self
    where
        F: DatabaseFilter,
    {
        self.query = self.query.filter(&filter);
        self
    }

    pub fn order_by<O>(mut self, order: O) -> Self
    where
        O: DatabaseOrderBy,
    {
        self.query = self.query.order_by(&order);
        self
    }

    pub fn default_order(mut self) -> Self {
        self.query = self.query.default_order();
        self
    }

    pub fn limit(mut self, limit: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.limit = Some(limit);
        self.query.page = Some(page);
        self
    }

    pub fn offset(mut self, offset: i64) -> Self {
        let mut page = self.query.page.unwrap_or_default();
        page.offset = Some(offset);
        self.query.page = Some(page);
        self
    }

    pub fn paginate(mut self, page: super::query::PageInput) -> Self {
        self.query = self.query.paginate(&page);
        self
    }

    pub async fn fetch_all(self) -> crate::Result<Vec<T>> {
        let query = self.query;
        if let Some(hook_ctx) = self.write_ctx.mutation_ctx.as_deref_mut() {
            query.fetch_all_on(hook_ctx.executor()).await
        } else {
            query.fetch_all(self.write_ctx.database()).await
        }
    }

    pub async fn fetch_one(self) -> crate::Result<Option<T>> {
        let query = self.query;
        if let Some(hook_ctx) = self.write_ctx.mutation_ctx.as_deref_mut() {
            query.fetch_one_on(hook_ctx.executor()).await
        } else {
            query.fetch_one(self.write_ctx.database()).await
        }
    }

    pub async fn count(self) -> crate::Result<i64> {
        let query = self.query;
        if let Some(hook_ctx) = self.write_ctx.mutation_ctx.as_deref_mut() {
            query.count_on(hook_ctx.executor()).await
        } else {
            query.count(self.write_ctx.database()).await
        }
    }

    pub async fn exists(self) -> crate::Result<bool> {
        Ok(self.count().await? > 0)
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<'tx, B> MutationContext<'tx, B>
where
    B: WriteBackend,
{
    pub fn new(db: &'tx crate::db::Database<B>, tx: sqlx::Transaction<'tx, B::Database>) -> Self {
        Self {
            db,
            tx,
            deferred_events: Vec::new(),
            deferred_actions: Vec::new(),
        }
    }

    pub fn database(&self) -> &crate::db::Database<B> {
        self.db
    }

    pub fn actor<T>(&self, ctx: Option<&async_graphql::Context<'_>>) -> Option<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        ctx.and_then(|ctx| ctx.data_opt::<T>()).cloned()
    }

    pub fn auth_user(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
    ) -> async_graphql::Result<String> {
        ctx.ok_or_else(|| async_graphql::Error::new("missing GraphQL context for auth user"))?
            .auth_user_id()
    }

    pub fn auth_subject(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
    ) -> async_graphql::Result<AuthSubject> {
        ctx.ok_or_else(|| async_graphql::Error::new("missing GraphQL context for auth subject"))?
            .auth_subject()
    }

    pub fn executor(&mut self) -> &mut <B::Database as sqlx::Database>::Connection
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
    {
        self.tx.as_mut()
    }

    pub fn queue_event<T>(&mut self, event: T)
    where
        T: Clone + Send + Sync + 'static,
    {
        self.deferred_events.push(Box::new(DeferredEvent { event }));
    }

    pub fn defer<F, Fut, E>(&mut self, action: F)
    where
        F: FnOnce(crate::db::Database<B>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), E>> + Send + 'static,
        E: std::fmt::Display + Send + 'static,
    {
        self.deferred_actions.push(Box::new(DeferredAction {
            action: Some(action),
        }));
    }

    pub fn emit_queued_events(&mut self) {
        for event in self.deferred_events.drain(..) {
            event.emit(self.db);
        }
    }

    pub async fn commit_and_emit(self) -> crate::Result<()> {
        let Self {
            db,
            tx,
            deferred_events,
            deferred_actions,
        } = self;
        tx.commit().await?;
        for event in deferred_events {
            event.emit(db);
        }
        for action in deferred_actions {
            if let Err(error) = action.run(db.clone()).await {
                db.report_post_commit_error(error).await;
            }
        }
        Ok(())
    }

    pub async fn run_mutation_hook<'a>(
        &'a mut self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        event: &'a MutationEvent,
    ) -> async_graphql::Result<()>
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
    {
        if let Some(hook) = self.db.mutation_hook() {
            hook.on_mutation(ctx, self, event).await?;
        }
        #[cfg(any(
            all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
            all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
        ))]
        {
            super::backup::record_change_journal_event(self, event)
                .await
                .map_err(|error| async_graphql::Error::new(error.to_string()))?;
        }

        Ok(())
    }

    pub async fn insert<'a, T>(
        &'a mut self,
        input: <T as MutationContextInsert<B>>::CreateInput,
    ) -> crate::Result<T>
    where
        T: MutationContextInsert<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::insert_in_mutation_context(self, input).await
    }

    pub async fn upsert<'a, T>(
        &'a mut self,
        input: <T as MutationContextUpsert<B>>::UpsertInput,
    ) -> crate::Result<UpsertOutcome<T>>
    where
        T: MutationContextUpsert<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::upsert_in_mutation_context(self, input).await
    }

    pub async fn update_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextUpdateById<B>>::Id,
        input: <T as MutationContextUpdateById<B>>::UpdateInput,
    ) -> crate::Result<Option<T>>
    where
        T: MutationContextUpdateById<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::update_by_id_in_mutation_context(self, id, input).await
    }

    pub async fn update_where<'a, T>(
        &'a mut self,
        where_input: <T as MutationContextUpdateWhere<B>>::WhereInput,
        input: <T as MutationContextUpdateWhere<B>>::UpdateInput,
    ) -> crate::Result<i64>
    where
        T: MutationContextUpdateWhere<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::update_where_in_mutation_context(self, where_input, input).await
    }

    /// Atomically update one versioned entity when its version and typed
    /// expected predicates match.
    pub async fn compare_and_swap<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextCompareAndSwap<B>>::Id,
        expected_version: i64,
        expected: <T as MutationContextCompareAndSwap<B>>::Expected,
        input: <T as MutationContextCompareAndSwap<B>>::UpdateInput,
    ) -> crate::Result<ConditionalUpdateOutcome<T>>
    where
        T: MutationContextCompareAndSwap<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::compare_and_swap_in_mutation_context(self, id, expected_version, expected, input).await
    }

    pub async fn delete_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextDeleteById<B>>::Id,
    ) -> crate::Result<bool>
    where
        T: MutationContextDeleteById<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::delete_by_id_in_mutation_context(self, id).await
    }

    pub async fn delete_where<'a, T>(
        &'a mut self,
        where_input: <T as MutationContextDeleteWhere<B>>::WhereInput,
    ) -> crate::Result<i64>
    where
        T: MutationContextDeleteWhere<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::delete_where_in_mutation_context(self, where_input).await
    }

    pub fn query<'a, T>(&'a mut self) -> MutationQuery<'a, 'tx, T, B>
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        MutationQuery::new(self)
    }

    /// Start a transaction-bound read of a macro-generated typed projection.
    /// Only the projection's declared columns are selected and decoded.
    pub fn project<'a, P>(&'a mut self) -> super::query::TransactionProjectionQuery<'a, 'tx, P, B>
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
        P: super::query::ReadProjection<B>,
    {
        super::query::TransactionProjectionQuery::new(self)
    }

    pub async fn find_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextFindById<B>>::Id,
    ) -> crate::Result<Option<T>>
    where
        T: MutationContextFindById<B> + Entity,
    {
        let metadata = T::metadata();
        self.db
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
        let row = T::find_by_id_in_mutation_context(self, id).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if authorize_repository_row::<T, B>(self.db, &row).await? {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    /// Find one entity by its complete generated primary-key value.
    pub async fn find_by_key<'a, T>(
        &'a mut self,
        key: &'a <T as MutationContextFindByKey<B>>::Key,
    ) -> crate::Result<Option<T>>
    where
        T: MutationContextFindByKey<B> + Entity,
    {
        let metadata = T::metadata();
        self.db
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
        let row = T::find_by_key_in_mutation_context(self, key).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if authorize_repository_row::<T, B>(self.db, &row).await? {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    /// Update one entity selected by its complete generated key.
    pub async fn update_by_key<'a, T>(
        &'a mut self,
        key: &'a <T as MutationContextUpdateByKey<B>>::Key,
        input: <T as MutationContextUpdateByKey<B>>::UpdateInput,
    ) -> crate::Result<Option<T>>
    where
        T: MutationContextUpdateByKey<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::update_by_key_in_mutation_context(self, key, input).await
    }

    /// Delete one entity selected by its complete generated key.
    pub async fn delete_by_key<'a, T>(
        &'a mut self,
        key: &'a <T as MutationContextDeleteByKey<B>>::Key,
    ) -> crate::Result<bool>
    where
        T: MutationContextDeleteByKey<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::delete_by_key_in_mutation_context(self, key).await
    }

    /// Insert unless the declared primary/unique target is already present.
    pub async fn insert_if_absent<T>(
        &mut self,
        input: <T as MutationContextInsertIfAbsent<B>>::CreateInput,
    ) -> crate::Result<InsertIfAbsentOutcome<T>>
    where
        T: MutationContextInsertIfAbsent<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::insert_if_absent_in_mutation_context(self, input).await
    }

    /// Update a typed match set only when it fits within an explicit ceiling.
    pub async fn update_where_bounded<T>(
        &mut self,
        where_input: <T as MutationContextBoundedUpdateWhere<B>>::WhereInput,
        input: <T as MutationContextBoundedUpdateWhere<B>>::UpdateInput,
        limit: MutationLimit,
    ) -> crate::Result<BoundedMutationOutcome>
    where
        T: MutationContextBoundedUpdateWhere<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::update_where_bounded_in_mutation_context(self, where_input, input, limit).await
    }

    /// Delete a typed match set only when it fits within an explicit ceiling.
    pub async fn delete_where_bounded<T>(
        &mut self,
        where_input: <T as MutationContextBoundedDeleteWhere<B>>::WhereInput,
        limit: MutationLimit,
    ) -> crate::Result<BoundedMutationOutcome>
    where
        T: MutationContextBoundedDeleteWhere<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::delete_where_bounded_in_mutation_context(self, where_input, limit).await
    }

    /// Atomically update a complete key plus an additional typed predicate.
    pub async fn update_if<T>(
        &mut self,
        key: &<T as MutationContextPredicateUpdate<B>>::Key,
        expected: <T as MutationContextPredicateUpdate<B>>::Expected,
        input: <T as MutationContextPredicateUpdate<B>>::UpdateInput,
    ) -> crate::Result<PredicateUpdateOutcome<T>>
    where
        T: MutationContextPredicateUpdate<B> + Entity,
    {
        ensure_transaction_entity_write_access::<T, B>(self.db).await?;
        T::update_if_in_mutation_context(self, key, expected, input).await
    }

    pub async fn keyset_page<'a, T>(
        &'a mut self,
        filter: <T as MutationContextKeysetPage<B>>::Filter,
        page: crate::graphql::pagination::KeysetPageInput,
    ) -> Result<crate::graphql::pagination::Connection<T>, crate::graphql::errors::OrmPublicError>
    where
        T: MutationContextKeysetPage<B> + Entity,
    {
        let metadata = T::metadata();
        self.db
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| {
                crate::graphql::errors::OrmPublicError::internal(format!("{error:?}"))
            })?;
        if page.include_total_count && self.db.row_policy().is_some() {
            return Err(crate::graphql::errors::OrmPublicError::new(
                crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured,
            )
            .with_internal(
                "repository keyset total counts require a database-visible row policy",
            ));
        }
        let connection = T::keyset_page_in_mutation_context(self, filter, page).await?;
        for edge in &connection.edges {
            if !authorize_repository_row::<T, B>(self.db, &edge.node)
                .await
                .map_err(crate::graphql::errors::OrmPublicError::from)?
            {
                return Err(crate::graphql::errors::OrmPublicError::forbidden());
            }
        }
        Ok(connection)
    }

    /// Reads a bounded forward or backward keyset connection inside the
    /// current transaction.
    pub async fn keyset_connection_page<'a, T>(
        &'a mut self,
        filter: <T as MutationContextKeysetConnectionPage<B>>::Filter,
        page: crate::graphql::pagination::KeysetConnectionInput,
    ) -> Result<crate::graphql::pagination::Connection<T>, crate::graphql::errors::OrmPublicError>
    where
        T: MutationContextKeysetConnectionPage<B> + Entity,
    {
        let metadata = T::metadata();
        self.db
            .ensure_entity_access(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::Repository,
            )
            .await
            .map_err(|error| {
                crate::graphql::errors::OrmPublicError::internal(format!("{error:?}"))
            })?;
        if page.include_total_count && self.db.row_policy().is_some() {
            return Err(crate::graphql::errors::OrmPublicError::new(
                crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured,
            )
            .with_internal(
                "repository keyset total counts require a database-visible row policy",
            ));
        }
        let connection = T::keyset_connection_page_in_mutation_context(self, filter, page).await?;
        for edge in &connection.edges {
            if !authorize_repository_row::<T, B>(self.db, &edge.node)
                .await
                .map_err(crate::graphql::errors::OrmPublicError::from)?
            {
                return Err(crate::graphql::errors::OrmPublicError::forbidden());
            }
        }
        Ok(connection)
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<'tx, B> RetentionContext<'tx, B>
where
    B: super::TransactionBackend,
    for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = B::Database> + Send,
{
    pub(crate) fn new(mutation: MutationContext<'tx, B>) -> Self {
        Self {
            mutation,
            poisoned: false,
        }
    }

    pub(crate) async fn commit_and_emit(mut self) -> crate::Result<()> {
        if self.poisoned {
            return Err(sqlx::Error::Protocol(
                "retention transaction cannot commit after a failed maintenance operation"
                    .to_string(),
            ));
        }
        B::clear_retention_context(&mut self.mutation.tx).await?;
        self.mutation.commit_and_emit().await
    }

    /// Append a row through the entity's normal generated insert path.
    ///
    /// This enables a host schema to record a separate redacted purge fact in
    /// the same atomic transaction without assigning application-specific
    /// audit semantics to the ORM.
    ///
    /// # Errors
    ///
    /// Returns an ORM error when authorization, transforms, hooks, generated
    /// SQL execution, or row decoding fails. The surrounding retention
    /// transaction is then rolled back when that error is returned.
    pub async fn insert<T>(
        &mut self,
        input: <T as MutationContextInsert<B>>::CreateInput,
    ) -> crate::Result<T>
    where
        T: MutationContextInsert<B> + Entity,
    {
        self.mutation.insert::<T>(input).await
    }

    /// Start a transaction-bound typed entity read.
    pub fn query<T>(&mut self) -> MutationQuery<'_, 'tx, T, B>
    where
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        self.mutation.query::<T>()
    }

    /// Start a transaction-bound least-privilege projection read.
    pub fn project<P>(&mut self) -> super::query::TransactionProjectionQuery<'_, 'tx, P, B>
    where
        P: super::query::ReadProjection<B>,
    {
        self.mutation.project::<P>()
    }

    /// Purge a nonempty typed match set from an explicitly retention-enabled
    /// append-only entity, subject to an explicit nonzero maximum.
    ///
    /// # Errors
    ///
    /// Returns an ORM error if the entity or row policy denies access, the
    /// predicate is empty or cannot be rendered entirely by the database,
    /// trigger context or hooks fail, or selected and affected cardinality do
    /// not match exactly. Any error poisons the context so it cannot be caught
    /// and followed by a successful commit.
    pub async fn purge<T>(
        &mut self,
        filter: <T as RetentionPurge<B>>::Filter,
        limit: MutationLimit,
    ) -> crate::Result<RetentionPurgeOutcome>
    where
        T: RetentionPurge<B>,
    {
        let result = self.purge_inner::<T>(filter, limit).await;
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    async fn purge_inner<T>(
        &mut self,
        filter: <T as RetentionPurge<B>>::Filter,
        limit: MutationLimit,
    ) -> crate::Result<RetentionPurgeOutcome>
    where
        T: RetentionPurge<B>,
    {
        let metadata = T::metadata();
        let Some(policy) = metadata.retention_policy else {
            return Err(sqlx::Error::Protocol(
                "entity is not enabled for retention purge".to_string(),
            ));
        };
        self.mutation
            .db
            .ensure_entity_access(
                None,
                metadata.entity_name,
                Some(policy),
                EntityAccessKind::Write,
                EntityAccessSurface::RetentionMaintenance,
            )
            .await
            .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
        if filter.is_empty() {
            return Err(sqlx::Error::Protocol(
                "retention purge requires a non-empty typed filter".to_string(),
            ));
        }
        if filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(sqlx::Error::Protocol(
                "retention purge requires a filter rendered completely by the database".to_string(),
            ));
        }
        let mut query = EntityQuery::<T, B>::new().filter(&filter);
        query.order_clauses.push(
            T::PRIMARY_KEYS
                .iter()
                .map(|column| B::DIALECT.quote_identifier_path(column))
                .collect::<Vec<_>>()
                .join(", "),
        );
        query.page = Some(super::query::PageInput {
            limit: Some(i64::from(limit.get()) + 1),
            offset: None,
        });
        let rows = query.fetch_all_on(self.mutation.executor()).await?;
        if rows.len() > limit.get() as usize {
            return Ok(RetentionPurgeOutcome::LimitExceeded {
                maximum: limit.get(),
            });
        }

        let mut states = Vec::with_capacity(rows.len());
        for entity in &rows {
            self.mutation
                .db
                .ensure_writable_row(
                    None,
                    metadata.entity_name,
                    metadata.write_policy,
                    EntityAccessSurface::RetentionMaintenance,
                    entity as &(dyn Any + Send + Sync),
                )
                .await
                .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
            let state = entity_state(entity)?;
            self.mutation
                .run_mutation_hook(
                    None,
                    &MutationEvent {
                        phase: MutationPhase::Before,
                        action: ChangeAction::Deleted,
                        entity_name: metadata.entity_name,
                        table_name: metadata.table_name,
                        metadata,
                        id: entity.retention_event_id(),
                        changes: Vec::new(),
                        before_state: Some(state.clone()),
                        after_state: None,
                    },
                )
                .await
                .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
            states.push(state);
        }

        B::set_retention_context(&mut self.mutation.tx, T::TABLE_NAME).await?;
        let (sql, values) = EntityQuery::<T, B>::new()
            .filter(&filter)
            .build_delete_sql();
        let execution = B::execute_with_binds_on(self.mutation.executor(), sql, values).await;
        let cleared = B::clear_retention_context(&mut self.mutation.tx).await;
        let result = execution?;
        cleared?;
        let affected = B::rows_affected(&result);
        if affected != rows.len() as u64 {
            return Err(sqlx::Error::Protocol(format!(
                "retention purge cardinality changed: selected {}, deleted {affected}",
                rows.len(),
            )));
        }

        for (entity, state) in rows.iter().zip(states) {
            self.mutation
                .run_mutation_hook(
                    None,
                    &MutationEvent {
                        phase: MutationPhase::After,
                        action: ChangeAction::Deleted,
                        entity_name: metadata.entity_name,
                        table_name: metadata.table_name,
                        metadata,
                        id: entity.retention_event_id(),
                        changes: Vec::new(),
                        before_state: Some(state),
                        after_state: None,
                    },
                )
                .await
                .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))?;
            T::retention_queue_changed_event(&mut self.mutation, entity).await?;
        }

        let affected = u32::try_from(affected).map_err(|_| {
            sqlx::Error::Protocol("retention purge affected-row count overflow".to_string())
        })?;
        self.mutation.queue_event(RetentionPurgeEvent {
            entity_name: metadata.entity_name,
            table_name: metadata.table_name,
            affected,
        });
        Ok(RetentionPurgeOutcome::Purged { affected })
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn authorize_repository_row<T, B>(db: &crate::db::Database<B>, row: &T) -> crate::Result<bool>
where
    T: Entity,
    B: WriteBackend,
{
    let metadata = T::metadata();
    let record = row.repository_policy_record();
    if db.row_policy().is_some() {
        let record = record.ok_or_else(|| {
            crate::graphql::errors::sqlx_error_from_public(
                crate::graphql::errors::OrmPublicError::new(
                    crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured,
                )
                .with_internal(
                    "entity does not expose the generated repository policy-record capability",
                ),
            )
        })?;
        if !db
            .can_read_row(
                None,
                metadata.entity_name,
                metadata.read_policy,
                EntityAccessSurface::Repository,
                record,
            )
            .await
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?
        {
            return Ok(false);
        }
    }
    for field in T::repository_field_policies() {
        db.ensure_repository_readable_field(
            None,
            metadata.entity_name,
            field.api_name,
            field.read_policy,
            record,
        )
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_public)?;
    }
    Ok(true)
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn authorize_repository_rows<T, B>(
    db: &crate::db::Database<B>,
    rows: Vec<T>,
) -> crate::Result<Vec<T>>
where
    T: Entity,
    B: WriteBackend,
{
    let mut visible = Vec::with_capacity(rows.len());
    for row in rows {
        if authorize_repository_row::<T, B>(db, &row).await? {
            visible.push(row);
        }
    }
    Ok(visible)
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn ensure_transaction_entity_write_access<T: Entity, B: WriteBackend>(
    db: &crate::db::Database<B>,
) -> crate::Result<()> {
    let metadata = T::metadata();
    db.ensure_entity_access(
        None,
        metadata.entity_name,
        metadata.write_policy,
        EntityAccessKind::Write,
        EntityAccessSurface::Repository,
    )
    .await
    .map_err(|error| sqlx::Error::Protocol(format!("{error:?}")))
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationHook<B: WriteBackend = DefaultWriteBackend>: Send + Sync {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_, B>,
        event: &'a MutationEvent,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextInsert<B: WriteBackend = DefaultWriteBackend>: Sized {
    type CreateInput;

    fn insert_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        input: Self::CreateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<Self>>;
}

/// Macro-generated capability implemented only for append-only entities that
/// explicitly declare a dedicated retention policy.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait RetentionPurge<B: super::TransactionBackend = DefaultWriteBackend>:
    Entity + FromSqlRow<B> + serde::Serialize + Clone + Send + Sync + 'static
{
    /// Generated typed filter belonging to this exact entity.
    type Filter: DatabaseFilter + Clone + Send + Sync + 'static;

    #[doc(hidden)]
    fn retention_event_id(&self) -> String;

    #[doc(hidden)]
    fn retention_queue_changed_event<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        entity: &'a Self,
    ) -> futures::future::BoxFuture<'a, crate::Result<()>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpsert<B: WriteBackend = DefaultWriteBackend>: Sized {
    type UpsertInput;

    fn upsert_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        input: Self::UpsertInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<UpsertOutcome<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpdateById<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;
    type UpdateInput;

    fn update_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<Option<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpdateWhere<B: WriteBackend = DefaultWriteBackend>: Sized {
    type WhereInput;
    type UpdateInput;

    fn update_where_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        where_input: Self::WhereInput,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<i64>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextCompareAndSwap<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;
    type Expected;
    type UpdateInput;

    fn compare_and_swap_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
        expected_version: i64,
        expected: Self::Expected,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<ConditionalUpdateOutcome<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextDeleteById<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;

    fn delete_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
    ) -> futures::future::BoxFuture<'a, crate::Result<bool>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextDeleteWhere<B: WriteBackend = DefaultWriteBackend>: Sized {
    type WhereInput;

    fn delete_where_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        where_input: Self::WhereInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<i64>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextFindById<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;

    fn find_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
    ) -> futures::future::BoxFuture<'a, crate::Result<Option<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextFindByKey<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Key;

    fn find_by_key_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        key: &'a Self::Key,
    ) -> futures::future::BoxFuture<'a, crate::Result<Option<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpdateByKey<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Key;
    type UpdateInput;

    fn update_by_key_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        key: &'a Self::Key,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<Option<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextDeleteByKey<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Key;

    fn delete_by_key_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        key: &'a Self::Key,
    ) -> futures::future::BoxFuture<'a, crate::Result<bool>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextInsertIfAbsent<B: WriteBackend = DefaultWriteBackend>: Sized {
    type CreateInput;

    fn insert_if_absent_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        input: Self::CreateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<InsertIfAbsentOutcome<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextBoundedUpdateWhere<B: WriteBackend = DefaultWriteBackend>: Sized {
    type WhereInput;
    type UpdateInput;

    fn update_where_bounded_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        where_input: Self::WhereInput,
        input: Self::UpdateInput,
        limit: MutationLimit,
    ) -> futures::future::BoxFuture<'a, crate::Result<BoundedMutationOutcome>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextBoundedDeleteWhere<B: WriteBackend = DefaultWriteBackend>: Sized {
    type WhereInput;

    fn delete_where_bounded_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        where_input: Self::WhereInput,
        limit: MutationLimit,
    ) -> futures::future::BoxFuture<'a, crate::Result<BoundedMutationOutcome>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextPredicateUpdate<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Key;
    type Expected;
    type UpdateInput;

    fn update_if_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        key: &'a Self::Key,
        expected: Self::Expected,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, crate::Result<PredicateUpdateOutcome<Self>>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextKeysetPage<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Filter;

    fn keyset_page_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        filter: Self::Filter,
        page: crate::graphql::pagination::KeysetPageInput,
    ) -> futures::future::BoxFuture<
        'a,
        Result<
            crate::graphql::pagination::Connection<Self>,
            crate::graphql::errors::OrmPublicError,
        >,
    >;
}

/// Generated repository contract for bounded bidirectional keyset reads.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextKeysetConnectionPage<B: WriteBackend = DefaultWriteBackend>:
    Sized
{
    /// Generated typed filter.
    type Filter;

    /// Reads one bounded keyset connection in an existing transaction.
    fn keyset_connection_page_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        filter: Self::Filter,
        page: crate::graphql::pagination::KeysetConnectionInput,
    ) -> futures::future::BoxFuture<
        'a,
        Result<
            crate::graphql::pagination::Connection<Self>,
            crate::graphql::errors::OrmPublicError,
        >,
    >;
}

pub trait PostCommitErrorHandler<B: OrmBackend = DefaultBackend>: Send + Sync {
    fn on_post_commit_error<'a>(
        &'a self,
        db: &'a crate::db::Database<B>,
        error: &'a str,
    ) -> futures::future::BoxFuture<'a, ()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityAccessKind {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityAccessSurface {
    GraphqlQuery,
    GraphqlMutation,
    GraphqlSubscription,
    GraphqlRelation,
    Repository,
    /// Host-only bounded retention maintenance. This surface is never used by
    /// generated GraphQL roots or ordinary repository transactions.
    RetentionMaintenance,
}

/// Entity policy helper that gates generated access by exact scope strings.
///
/// `ScopeEntityPolicy` is intentionally backend- and auth-crate agnostic. It
/// reads [`AuthSubject`](crate::graphql::auth::AuthSubject) from the
/// `async-graphql` context through [`AuthExt`](crate::graphql::auth::AuthExt)
/// and matches scopes exactly; wildcard or hierarchical semantics belong in an
/// optional application/auth integration layer, not in database RLS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeEntityPolicy {
    /// Scopes accepted for read access.
    pub read_scopes: &'static [&'static str],
    /// Scopes accepted for write access.
    pub write_scopes: &'static [&'static str],
    /// Return an unauthenticated error when no subject is present.
    pub require_auth: bool,
}

impl ScopeEntityPolicy {
    /// Create a policy that requires auth and exact scope matches.
    pub const fn new(
        read_scopes: &'static [&'static str],
        write_scopes: &'static [&'static str],
    ) -> Self {
        Self {
            read_scopes,
            write_scopes,
            require_auth: true,
        }
    }

    /// Create a policy that permits any authenticated subject when no scopes
    /// are configured for the requested access kind.
    pub const fn authenticated() -> Self {
        Self {
            read_scopes: &[],
            write_scopes: &[],
            require_auth: true,
        }
    }

    /// Create a policy that lets host policies decide when auth is absent.
    pub const fn optional(
        read_scopes: &'static [&'static str],
        write_scopes: &'static [&'static str],
    ) -> Self {
        Self {
            read_scopes,
            write_scopes,
            require_auth: false,
        }
    }
}

impl Default for ScopeEntityPolicy {
    fn default() -> Self {
        Self::authenticated()
    }
}

impl<B: OrmBackend> EntityPolicy<B> for ScopeEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a crate::db::Database<B>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            let required_scopes = match kind {
                EntityAccessKind::Read => self.read_scopes,
                EntityAccessKind::Write => self.write_scopes,
            };
            let subject = ctx.and_then(AuthExt::auth_subject_opt);

            let Some(subject) = subject else {
                if self.require_auth {
                    return Err(crate::graphql::errors::OrmPublicError::unauthenticated()
                        .into_graphql_error());
                }
                return Ok(required_scopes.is_empty());
            };

            if required_scopes.is_empty() {
                return Ok(true);
            }

            Ok(required_scopes.iter().any(|scope| subject.has_scope(scope)))
        })
    }
}

pub trait EntityPolicy<B: OrmBackend = DefaultBackend>: Send + Sync {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}

pub trait FieldPolicy<B: OrmBackend = DefaultBackend>: Send + Sync {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    /// Authorize one field selected through a repository-only/full-entity read.
    ///
    /// Existing GraphQL-only policy implementations remain source compatible,
    /// but declared repository field policies fail closed until this method is
    /// implemented deliberately.
    fn can_read_repository_field<'a>(
        &'a self,
        _access: Option<crate::graphql::auth::AccessContext<'a>>,
        _db: &'a crate::db::Database<B>,
        _entity_name: &'static str,
        _field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(policy_key.is_none()) })
    }

    /// Authorize one field supplied through an ordinary Rust repository input.
    ///
    /// The default denies fields with declared policy keys, preventing the
    /// absence of a GraphQL request context from becoming implicit authority.
    fn can_write_repository_field<'a>(
        &'a self,
        _access: Option<crate::graphql::auth::AccessContext<'a>>,
        _db: &'a crate::db::Database<B>,
        _entity_name: &'static str,
        _field_name: &'static str,
        policy_key: Option<&'static str>,
        _record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(policy_key.is_none()) })
    }
}

pub trait RowPolicy<B: OrmBackend = DefaultBackend>: Send + Sync {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait WriteInputTransform<B: WriteBackend = DefaultWriteBackend>: Send + Sync {
    fn before_create_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_, B>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        self.before_create(
            write_ctx.graphql_ctx(),
            write_ctx.database(),
            write_ctx.entity_name(),
            input,
        )
    }

    fn before_update_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_, B>,
        existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        self.before_update(
            write_ctx.graphql_ctx(),
            write_ctx.database(),
            write_ctx.entity_name(),
            existing_row,
            input,
        )
    }

    fn before_upsert_with_context<'a>(
        &'a self,
        write_ctx: &'a mut WriteInputContext<'_, '_, B>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        self.before_upsert(
            write_ctx.graphql_ctx(),
            write_ctx.database(),
            write_ctx.entity_name(),
            input,
        )
    }

    fn before_create<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        let _ = (ctx, db, entity_name, input);
        Box::pin(async { Ok(()) })
    }

    fn before_update<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        let _ = (ctx, db, entity_name, existing_row, input);
        Box::pin(async { Ok(()) })
    }

    fn before_upsert<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database<B>,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        self.before_create(ctx, db, entity_name, input)
    }
}

pub fn mutation_changes(fields: &[&str], values: &[SqlValue]) -> Vec<MutationFieldValue> {
    fields
        .iter()
        .zip(values.iter())
        .map(|(field, value)| MutationFieldValue {
            field: (*field).to_string(),
            value: value.clone(),
        })
        .collect()
}

pub trait OrmResultError {
    fn from_sqlx_error(error: sqlx::Error) -> Self;
}

impl OrmResultError for sqlx::Error {
    fn from_sqlx_error(error: sqlx::Error) -> Self {
        error
    }
}

impl OrmResultError for async_graphql::Error {
    fn from_sqlx_error(error: sqlx::Error) -> Self {
        async_graphql::Error::new(error.to_string())
    }
}

pub fn json_sql_value<T, E>(value: &T) -> Result<SqlValue, E>
where
    T: serde::Serialize,
    E: OrmResultError,
{
    let json = serde_json::to_value(value)
        .map_err(|error| E::from_sqlx_error(sqlx::Error::Encode(Box::new(error))))?;
    Ok(SqlValue::Json(json))
}

pub fn entity_state<T>(value: &T) -> crate::Result<EntityState>
where
    T: serde::Serialize + Clone + Send + Sync + 'static,
{
    let json = serde_json::to_value(value).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    Ok(EntityState {
        value: Arc::new(value.clone()),
        json,
    })
}

/// Capture mutation-hook state while replacing selected top-level fields.
///
/// The retained type-erased value is the redacted JSON object rather than the
/// original entity, so a mutation hook cannot recover protected fields by
/// downcasting the state back to the entity type.
#[doc(hidden)]
pub fn entity_state_redacted<T>(value: &T, sensitive_fields: &[&str]) -> crate::Result<EntityState>
where
    T: serde::Serialize,
{
    let mut json =
        serde_json::to_value(value).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    if let serde_json::Value::Object(fields) = &mut json {
        for field in sensitive_fields {
            if fields.contains_key(*field) {
                fields.insert(
                    (*field).to_string(),
                    serde_json::Value::String("[redacted]".to_string()),
                );
            }
        }
    }
    Ok(EntityState {
        value: Arc::new(json.clone()),
        json,
    })
}

pub fn reset_query_count() {
    QUERY_COUNT.store(0, Ordering::SeqCst);
}

pub fn query_count() -> usize {
    QUERY_COUNT.load(Ordering::SeqCst)
}

fn record_query() {
    QUERY_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn record_executed_query() {
    record_query();
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDef {
    pub name: &'static str,
    pub rust_name: &'static str,
    /// Public GraphQL field name after rename attributes and case features are
    /// applied. Defaults to `name` for hand-built definitions.
    pub api_name: &'static str,
    pub sql_type: &'static str,
    pub spatial: Option<SpatialColumnDef>,
    pub search: Option<SearchFieldDef>,
    pub logical_type: BackupValueKind,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_generated: bool,
    pub is_filterable: bool,
    /// The derive generated an order-input term for this column.
    pub is_sortable: bool,
    /// The column stores a date-time value rendered per backend; its
    /// `logical_type` remains [`BackupValueKind::String`] for backup and
    /// schema-hash compatibility.
    pub is_date_time: bool,
    pub backup_policy: ColumnBackupPolicy,
    pub default: Option<&'static str>,
    pub references: Option<&'static str>,
}

impl ColumnDef {
    pub const fn text(name: &'static str) -> Self {
        Self::new(name, "TEXT")
    }

    pub const fn integer(name: &'static str) -> Self {
        Self::new(name, "INTEGER")
    }

    pub const fn blob(name: &'static str) -> Self {
        Self::new(name, "BLOB")
    }

    pub const fn new(name: &'static str, sql_type: &'static str) -> Self {
        Self {
            name,
            rust_name: name,
            api_name: name,
            sql_type,
            spatial: None,
            search: None,
            logical_type: BackupValueKind::String,
            nullable: true,
            is_primary_key: false,
            is_unique: false,
            is_generated: false,
            is_filterable: false,
            is_sortable: false,
            is_date_time: false,
            backup_policy: ColumnBackupPolicy::Include,
            default: None,
            references: None,
        }
    }

    pub const fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    pub const fn primary_key(mut self) -> Self {
        self.is_primary_key = true;
        self.nullable = false;
        self
    }

    pub const fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }

    pub const fn generated(mut self) -> Self {
        self.is_generated = true;
        self
    }

    pub const fn filterable(mut self) -> Self {
        self.is_filterable = true;
        self
    }

    pub const fn sortable(mut self) -> Self {
        self.is_sortable = true;
        self
    }

    pub const fn date_time(mut self) -> Self {
        self.is_date_time = true;
        self
    }

    pub const fn rust_name(mut self, rust_name: &'static str) -> Self {
        self.rust_name = rust_name;
        self
    }

    pub const fn api_name(mut self, api_name: &'static str) -> Self {
        self.api_name = api_name;
        self
    }

    pub const fn logical_type(mut self, logical_type: BackupValueKind) -> Self {
        self.logical_type = logical_type;
        self
    }

    pub const fn backup_policy(mut self, policy: ColumnBackupPolicy) -> Self {
        self.backup_policy = policy;
        self
    }

    pub const fn default(mut self, default: &'static str) -> Self {
        self.default = Some(default);
        self
    }

    pub const fn references(mut self, references: &'static str) -> Self {
        self.references = Some(references);
        self
    }

    pub const fn spatial(mut self, spatial: SpatialColumnDef) -> Self {
        self.spatial = Some(spatial);
        self
    }

    pub const fn search(mut self, search: SearchFieldDef) -> Self {
        self.search = Some(search);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldMetadata {
    pub name: &'static str,
    pub rust_name: &'static str,
    /// Public GraphQL field name after rename attributes and case features are
    /// applied. Defaults to `name` for hand-built definitions.
    pub api_name: &'static str,
    pub sql_type: &'static str,
    pub spatial: Option<SpatialColumnDef>,
    pub search: Option<SearchFieldDef>,
    pub logical_type: BackupValueKind,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_generated: bool,
    pub is_filterable: bool,
    /// The derive generated an order-input term for this column.
    pub is_sortable: bool,
    /// The column stores a date-time value rendered per backend; its
    /// `logical_type` remains [`BackupValueKind::String`] for backup and
    /// schema-hash compatibility.
    pub is_date_time: bool,
    pub backup_policy: ColumnBackupPolicy,
    pub default: Option<&'static str>,
    pub references: Option<&'static str>,
}

impl From<&ColumnDef> for FieldMetadata {
    fn from(value: &ColumnDef) -> Self {
        Self {
            name: value.name,
            rust_name: value.rust_name,
            api_name: value.api_name,
            sql_type: value.sql_type,
            spatial: value.spatial,
            search: value.search,
            logical_type: value.logical_type,
            nullable: value.nullable,
            is_primary_key: value.is_primary_key,
            is_unique: value.is_unique,
            is_generated: value.is_generated,
            is_filterable: value.is_filterable,
            is_sortable: value.is_sortable,
            is_date_time: value.is_date_time,
            backup_policy: value.backup_policy,
            default: value.default,
            references: value.references,
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, async_graphql::Enum,
)]
pub enum ColumnBackupPolicy {
    Include,
    Exclude,
    Redact,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, async_graphql::Enum,
)]
pub enum BackupValueKind {
    Null,
    Bool,
    Integer,
    Float,
    String,
    Uuid,
    Json,
    Bytes,
}

/// Relative importance for a field inside a denormalized search document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchWeight {
    /// Highest importance.
    A,
    /// High importance.
    B,
    /// Medium importance.
    C,
    /// Lowest importance.
    D,
}

impl SearchWeight {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        }
    }

    pub const fn score_multiplier(self) -> f64 {
        match self {
            Self::A => 1.0,
            Self::B => 0.7,
            Self::C => 0.4,
            Self::D => 0.1,
        }
    }

    pub const fn fallback_weight(self) -> i64 {
        match self {
            Self::A => 10,
            Self::B => 7,
            Self::C => 4,
            Self::D => 1,
        }
    }
}

/// Whether a search index should use a backend-native implementation or fallback matching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchBackendStrategy {
    Native,
    Fallback,
}

/// Physical search storage strategy selected for a generated entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchIndexStrategy {
    /// PostgreSQL shadow table with `tsvector` and a GIN index.
    PostgresTsvector,
    /// SQLite FTS5 virtual table.
    SqliteFts5,
    /// Portable token/document table fallback.
    FallbackTable,
    /// Reserved for future MySQL `FULLTEXT` support.
    MysqlFullText,
    /// Reserved for future SQL Server full-text catalog support.
    MssqlFullText,
}

impl SearchIndexStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PostgresTsvector => "postgres_tsvector",
            Self::SqliteFts5 => "sqlite_fts5",
            Self::FallbackTable => "fallback_table",
            Self::MysqlFullText => "mysql_fulltext",
            Self::MssqlFullText => "mssql_fulltext",
        }
    }
}

/// Search metadata for one local persisted text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchFieldDef {
    /// Rust field name on the entity.
    pub field_name: &'static str,
    /// Database column name used for writes and schema metadata.
    pub column_name: &'static str,
    /// Field weight inside the search document.
    pub weight: SearchWeight,
    /// Optional logical label for diagnostics.
    pub alias: Option<&'static str>,
    /// Optional policy key required for read-policy-protected source fields.
    pub policy: Option<&'static str>,
}

/// Search metadata for one JSON path extracted from a local persisted JSON field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchJsonPathDef {
    /// Rust field name on the entity.
    pub field_name: &'static str,
    /// Database column name used for writes and schema metadata.
    pub column_name: &'static str,
    /// Portable JSON path, such as `$.description.summary` or `$.tags[*].label`.
    pub path: &'static str,
    /// Field weight inside the search document.
    pub weight: SearchWeight,
    /// Optional policy key required for read-policy-protected source fields.
    pub policy: Option<&'static str>,
}

/// Search metadata for related fields copied into a parent entity search document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchRelationFieldDef {
    /// Rust relation field on the source entity.
    pub relation_field: &'static str,
    /// Target entity type name.
    pub target_type: &'static str,
    /// Target Rust field names included in the document.
    pub fields: &'static [&'static str],
    /// Weight assigned to all copied target fields.
    pub weight: SearchWeight,
    /// Maximum related rows included for multi-value relations.
    pub max_items: usize,
    /// Optional policy key required for protected related fields.
    pub policy: Option<&'static str>,
}

/// Entity-level full-text search definition emitted by generated code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchIndexDef {
    /// Rust entity type name.
    pub entity_name: &'static str,
    /// Database table name for the searched entity.
    pub table_name: &'static str,
    /// Primary-key column used to join search rows back to entities.
    pub primary_key: &'static str,
    /// Generated physical search index or table name.
    pub index_name: &'static str,
    /// Backend strategy for physical search storage.
    pub strategy: SearchIndexStrategy,
    /// Whether search storage should be created and queried.
    pub enabled: bool,
    /// Backend search language/config, such as PostgreSQL `english`.
    pub language: &'static str,
    /// SQLite FTS tokenizer, such as `unicode61`.
    pub tokenizer: &'static str,
    /// Minimum token length used by fallback tokenization.
    pub min_token_len: usize,
    /// Whether runtime fallback scoring may be used when native search fails.
    pub fallback_enabled: bool,
    /// Local searchable fields.
    pub fields: &'static [SearchFieldDef],
    /// Local persisted JSON paths copied into denormalized documents.
    pub json_paths: &'static [SearchJsonPathDef],
    /// Related fields copied into denormalized documents.
    pub relations: &'static [SearchRelationFieldDef],
}

/// Logical spatial storage kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpatialKind {
    /// OGC/PostGIS geometry semantics.
    Geometry,
}

impl SpatialKind {
    pub const fn as_sql(self) -> &'static str {
        match self {
            Self::Geometry => "geometry",
        }
    }
}

/// Supported GeoJSON/PostGIS geometry type declarations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SpatialGeometryType {
    Geometry,
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
}

impl SpatialGeometryType {
    pub const fn as_sql(self) -> &'static str {
        match self {
            Self::Geometry => "Geometry",
            Self::Point => "Point",
            Self::LineString => "LineString",
            Self::Polygon => "Polygon",
            Self::MultiPoint => "MultiPoint",
            Self::MultiLineString => "MultiLineString",
            Self::MultiPolygon => "MultiPolygon",
            Self::GeometryCollection => "GeometryCollection",
        }
    }

    pub fn from_sql(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "geometry" => Some(Self::Geometry),
            "point" => Some(Self::Point),
            "linestring" => Some(Self::LineString),
            "polygon" => Some(Self::Polygon),
            "multipoint" => Some(Self::MultiPoint),
            "multilinestring" => Some(Self::MultiLineString),
            "multipolygon" => Some(Self::MultiPolygon),
            "geometrycollection" => Some(Self::GeometryCollection),
            _ => None,
        }
    }
}

/// Spatial column metadata used by schema planning and generated SQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpatialColumnDef {
    /// Spatial kind. Only geometry is currently supported.
    pub kind: SpatialKind,
    /// Declared geometry type.
    pub geometry_type: SpatialGeometryType,
    /// Spatial reference identifier. Defaults to 4326 in generated metadata.
    pub srid: i32,
}

impl SpatialColumnDef {
    pub const fn geometry(geometry_type: SpatialGeometryType, srid: i32) -> Self {
        Self {
            kind: SpatialKind::Geometry,
            geometry_type,
            srid,
        }
    }

    pub fn sql_type(self) -> String {
        format!(
            "{}({},{})",
            self.kind.as_sql(),
            self.geometry_type.as_sql(),
            self.srid
        )
    }
}

/// Optional physical index method for generated index metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IndexMethod {
    /// Backend default index method.
    Default,
    /// PostgreSQL GiST index method, used for PostGIS spatial indexes.
    Gist,
}

impl IndexMethod {
    pub const fn as_sql(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Gist => Some("GIST"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexDef {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub is_unique: bool,
    pub method: IndexMethod,
    pub is_spatial: bool,
    /// Optional portable closed-set predicate for a partial index.
    pub predicate: Option<IndexPredicateDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexPredicateDef {
    pub column: &'static str,
    pub values: &'static [&'static str],
}

impl IndexDef {
    pub const fn new(name: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            name,
            columns,
            is_unique: false,
            method: IndexMethod::Default,
            is_spatial: false,
            predicate: None,
        }
    }

    pub const fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }

    pub const fn where_in(mut self, column: &'static str, values: &'static [&'static str]) -> Self {
        self.predicate = Some(IndexPredicateDef { column, values });
        self
    }

    pub const fn spatial_gist(name: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            name,
            columns,
            is_unique: false,
            method: IndexMethod::Gist,
            is_spatial: true,
            predicate: None,
        }
    }
}

pub type IndexMetadata = IndexDef;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum DeletePolicy {
    Restrict,
    Cascade,
    SetNull,
}

impl DeletePolicy {
    pub fn as_sql(&self) -> &'static str {
        match self {
            DeletePolicy::Restrict => "RESTRICT",
            DeletePolicy::Cascade => "CASCADE",
            DeletePolicy::SetNull => "SET NULL",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationChangePropagation {
    None,
    Up,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelationMetadata {
    pub field_name: &'static str,
    pub target_type: &'static str,
    pub source_column: &'static str,
    pub target_column: &'static str,
    pub source_columns: &'static [&'static str],
    pub target_columns: &'static [&'static str],
    pub is_multiple: bool,
    pub emit_foreign_key: bool,
    pub on_delete: DeletePolicy,
    pub propagate_change: RelationChangePropagation,
    pub search_fields: Option<SearchRelationFieldDef>,
}

#[derive(Clone, Debug)]
pub struct EntityMetadata {
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub plural_name: &'static str,
    /// Compatibility accessor for the first primary-key column.
    pub primary_key: &'static str,
    /// All primary-key columns in declaration order.
    pub primary_keys: Box<[&'static str]>,
    pub schema_policy: Option<SchemaPolicy>,
    pub default_sort: &'static str,
    pub backup_enabled: bool,
    pub backup_export_order: Option<i32>,
    pub backup_restore_order: Option<i32>,
    pub read_policy: Option<&'static str>,
    pub write_policy: Option<&'static str>,
    pub append_only: bool,
    /// Dedicated policy key required by host-only retention maintenance.
    /// `None` means the append-only entity cannot be purged.
    pub retention_policy: Option<&'static str>,
    pub check_constraints: Box<[CheckConstraintDef]>,
    pub fields: Box<[FieldMetadata]>,
    pub indexes: Box<[IndexMetadata]>,
    pub composite_unique_indexes: Box<[Box<[&'static str]>]>,
    pub relations: Box<[RelationMetadata]>,
    pub search: Option<&'static SearchIndexDef>,
}

impl EntityMetadata {
    /// Build metadata for an entity without append-only retention maintenance.
    ///
    /// This compatibility constructor preserves the pre-0.9 signature.
    pub fn from_schema<T>(
        entity_name: &'static str,
        backup_enabled: bool,
        backup_export_order: Option<i32>,
        backup_restore_order: Option<i32>,
        read_policy: Option<&'static str>,
        write_policy: Option<&'static str>,
        append_only: bool,
    ) -> Self
    where
        T: DatabaseEntity + DatabaseSchema + EntityRelations + DatabaseSearchSchema,
    {
        Self::from_schema_with_retention::<T>(
            entity_name,
            backup_enabled,
            backup_export_order,
            backup_restore_order,
            read_policy,
            write_policy,
            append_only,
            None,
        )
    }

    /// Build metadata with an optional dedicated append-only retention policy.
    pub fn from_schema_with_retention<T>(
        entity_name: &'static str,
        backup_enabled: bool,
        backup_export_order: Option<i32>,
        backup_restore_order: Option<i32>,
        read_policy: Option<&'static str>,
        write_policy: Option<&'static str>,
        append_only: bool,
        retention_policy: Option<&'static str>,
    ) -> Self
    where
        T: DatabaseEntity + DatabaseSchema + EntityRelations + DatabaseSearchSchema,
    {
        Self {
            entity_name,
            table_name: T::TABLE_NAME,
            plural_name: T::PLURAL_NAME,
            primary_key: T::PRIMARY_KEY,
            primary_keys: T::PRIMARY_KEYS.to_vec().into_boxed_slice(),
            schema_policy: T::SCHEMA_POLICY,
            default_sort: T::DEFAULT_SORT,
            backup_enabled,
            backup_export_order,
            backup_restore_order,
            read_policy,
            write_policy,
            append_only,
            retention_policy,
            check_constraints: T::check_constraints().to_vec().into_boxed_slice(),
            fields: T::columns()
                .iter()
                .map(FieldMetadata::from)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            indexes: T::indexes().to_vec().into_boxed_slice(),
            composite_unique_indexes: T::composite_unique_indexes()
                .iter()
                .map(|columns| columns.to_vec().into_boxed_slice())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            relations: T::relation_metadata().to_vec().into_boxed_slice(),
            search: T::search_index(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnModel {
    pub name: String,
    pub sql_type: String,
    pub spatial: Option<SpatialColumnDef>,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForeignKeyModel {
    pub source_column: String,
    pub target_table: String,
    pub target_column: String,
    pub is_multiple: bool,
    pub on_delete: DeletePolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchFieldModel {
    pub field_name: String,
    pub column_name: String,
    pub weight: SearchWeight,
    pub alias: Option<String>,
    pub policy: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchJsonPathModel {
    pub field_name: String,
    pub column_name: String,
    pub path: String,
    pub weight: SearchWeight,
    pub policy: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchRelationFieldModel {
    pub relation_field: String,
    pub target_type: String,
    pub fields: Vec<String>,
    pub weight: SearchWeight,
    pub max_items: usize,
    pub policy: Option<String>,
}

/// Search index representation in a structured schema model.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchIndexModel {
    pub name: String,
    pub table_name: String,
    pub entity_name: String,
    pub primary_key: String,
    pub strategy: SearchIndexStrategy,
    pub language: String,
    pub tokenizer: String,
    pub min_token_len: usize,
    pub fallback_enabled: bool,
    pub fields: Vec<SearchFieldModel>,
    pub json_paths: Vec<SearchJsonPathModel>,
    pub relations: Vec<SearchRelationFieldModel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableModel {
    pub entity_name: String,
    pub table_name: String,
    /// Compatibility accessor for the first primary-key column.
    pub primary_key: String,
    /// All primary-key columns in declaration order.
    pub primary_keys: Vec<String>,
    pub default_sort: String,
    pub columns: Vec<ColumnModel>,
    pub indexes: Vec<IndexMetadata>,
    pub composite_unique_indexes: Vec<Vec<String>>,
    pub foreign_keys: Vec<ForeignKeyModel>,
    pub search_indexes: Vec<SearchIndexModel>,
    pub append_only: bool,
    /// Whether generated append-only enforcement admits the exact ORM
    /// transaction-local bounded-retention context.
    pub retention_purge: bool,
    pub check_constraints: Vec<CheckConstraintModel>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckConstraintDef {
    pub name: &'static str,
    pub expression: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckConstraintModel {
    pub name: String,
    pub expression: String,
}

impl TableModel {
    pub fn primary_keys(&self) -> &[String] {
        if self.primary_keys.is_empty() {
            std::slice::from_ref(&self.primary_key)
        } else {
            &self.primary_keys
        }
    }
}

fn sanitize_generated_index_part(value: &str) -> String {
    let mut sanitized = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if ch == '_' {
            sanitized.push('_');
        } else if !sanitized.ends_with('_') {
            sanitized.push('_');
        }
    }
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "column".to_string()
    } else {
        sanitized.to_string()
    }
}

fn generated_index_name(table_name: &str, columns: &[&'static str]) -> String {
    let column_part = columns
        .iter()
        .map(|column| sanitize_generated_index_part(column))
        .collect::<Vec<_>>()
        .join("_");
    let full = format!(
        "idx_{}_{}",
        sanitize_generated_index_part(table_name),
        column_part
    );
    if full.len() <= 63 {
        return full;
    }

    let hash = format!("{:016x}", fnv1a64(full.as_bytes()));
    let prefix_len = 63usize.saturating_sub(hash.len() + 1);
    format!("{}_{}", &full[..prefix_len], hash)
}

fn static_index_name(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn static_index_columns(columns: &[&'static str]) -> &'static [&'static str] {
    Box::leak(columns.to_vec().into_boxed_slice())
}

fn index_columns_match(left: &[&str], right: &[&str]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| left == right)
}

fn table_has_index_for_columns(table: &TableModel, columns: &[&'static str]) -> bool {
    if columns.is_empty() {
        return true;
    }

    if table.primary_keys().len() == columns.len()
        && table
            .primary_keys()
            .iter()
            .zip(columns.iter())
            .all(|(left, right)| left == right)
    {
        return true;
    }

    if columns.len() == 1
        && table
            .columns
            .iter()
            .any(|column| column.name == columns[0] && (column.is_primary_key || column.is_unique))
    {
        return true;
    }

    if table.composite_unique_indexes.iter().any(|index| {
        index_columns_match(
            &index.iter().map(String::as_str).collect::<Vec<_>>(),
            columns,
        )
    }) {
        return true;
    }

    table
        .indexes
        .iter()
        .any(|index| index_columns_match(index.columns, columns))
}

fn add_generated_index(table: &mut TableModel, columns: &[&'static str]) {
    if table_has_index_for_columns(table, columns) {
        return;
    }

    let name = generated_index_name(&table.table_name, columns);
    if table.indexes.iter().any(|index| index.name == name) {
        return;
    }

    table.indexes.push(IndexDef::new(
        static_index_name(name),
        static_index_columns(columns),
    ));
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaModel {
    pub extensions: Vec<String>,
    pub tables: Vec<TableModel>,
}

impl SchemaModel {
    pub fn stable_hash(&self) -> String {
        stable_schema_model_hash(self)
    }
}

/// Database operation covered by a generated PostgreSQL RLS policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RlsOperation {
    /// `FOR SELECT USING (...)`.
    Select,
    /// `FOR INSERT WITH CHECK (...)`.
    Insert,
    /// `FOR UPDATE USING (...) WITH CHECK (...)`.
    Update,
    /// `FOR DELETE USING (...)`.
    Delete,
}

impl RlsOperation {
    /// Lowercase operation name used in deterministic generated policy names.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    /// Uppercase SQL action name reported by PostgreSQL policy introspection.
    pub const fn sql_action(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }
}

/// Row-level security configuration for one operation on one entity.
///
/// Generated metadata uses either a custom predicate or a conservative
/// predicate built from scope, tenant column, and owner column. A policy with no
/// predicate and no generated fields intentionally emits no PostgreSQL policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsOperationPolicy {
    /// Operation this policy applies to.
    pub operation: RlsOperation,
    /// Custom SQL predicate used exactly as provided.
    pub predicate: Option<&'static str>,
    /// Exact scope checked with `graphql_orm.has_scope(...)`.
    pub scope: Option<&'static str>,
    /// Database column compared with `graphql_orm.current_tenant_id()`.
    pub tenant_column: Option<&'static str>,
    /// Database column compared with `graphql_orm.current_user_id()`.
    pub owner_column: Option<&'static str>,
}

impl RlsOperationPolicy {
    /// Create a policy backed by an exact custom SQL predicate.
    pub const fn custom(operation: RlsOperation, predicate: &'static str) -> Self {
        Self {
            operation,
            predicate: Some(predicate),
            scope: None,
            tenant_column: None,
            owner_column: None,
        }
    }

    /// Create a policy whose predicate is generated from configured fields.
    pub const fn generated(
        operation: RlsOperation,
        scope: Option<&'static str>,
        tenant_column: Option<&'static str>,
        owner_column: Option<&'static str>,
    ) -> Self {
        Self {
            operation,
            predicate: None,
            scope,
            tenant_column,
            owner_column,
        }
    }
}

/// Generated RLS metadata for one entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsEntityMetadata {
    /// Rust entity name.
    pub entity_name: &'static str,
    /// Quoted database table path used in generated SQL.
    pub table_name: &'static str,
    /// Whether to emit `FORCE ROW LEVEL SECURITY`.
    pub force: bool,
    /// Operation policies for this table.
    pub policies: &'static [RlsOperationPolicy],
}

/// Collection of all generated RLS metadata in a schema root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsSchemaModel {
    /// Entity-level RLS metadata.
    pub entities: Vec<RlsEntityMetadata>,
}

impl RlsSchemaModel {
    /// Build an RLS model from generated optional entity metadata.
    pub fn from_entities(entities: &[Option<&'static RlsEntityMetadata>]) -> Self {
        Self {
            entities: entities
                .iter()
                .filter_map(|entity| (*entity).cloned())
                .collect(),
        }
    }

    /// Stable hash of the RLS target for planning and history metadata.
    pub fn stable_hash(&self) -> String {
        stable_rls_schema_hash(self)
    }
}

/// Full desired database target for schema management.
///
/// This extends table schema metadata with optional PostgreSQL RLS metadata.
/// `schema_roots!` generates a `graphql_orm_schema_target()` helper returning
/// this type.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaTarget {
    /// Desired tables, columns, indexes, constraints, and extensions.
    pub schema: SchemaModel,
    /// Desired PostgreSQL RLS state.
    pub rls: RlsSchemaModel,
}

impl SchemaTarget {
    /// Build a full schema target from generated entity metadata.
    pub fn from_entities(
        entities: &[&EntityMetadata],
        rls_entities: &[Option<&'static RlsEntityMetadata>],
    ) -> Self {
        Self {
            schema: SchemaModel::from_entities(entities),
            rls: RlsSchemaModel::from_entities(rls_entities),
        }
    }

    /// Stable hash covering both table schema and RLS metadata.
    pub fn stable_hash(&self) -> String {
        let canonical = format!(
            "schema:{}\nrls:{}\n",
            self.schema.stable_hash(),
            self.rls.stable_hash()
        );
        format!("{:016x}", fnv1a64(canonical.as_bytes()))
    }
}

/// Planned RLS SQL statements for a full schema target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlsPolicyPlan {
    /// Backend name used when the plan was produced.
    pub backend: &'static str,
    /// Stable hash of the target RLS model.
    pub target_rls_hash: String,
    /// Deterministic SQL statements for helper functions, RLS flags, and policies.
    pub statements: Vec<String>,
}

/// Planned table migration plus RLS SQL for a [`SchemaTarget`].
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedSchemaTarget {
    /// Migration version identifier.
    pub version: String,
    /// Human-readable migration description.
    pub description: String,
    /// Backend name used when the plan was produced.
    pub backend: &'static str,
    /// Stable hash of the introspected source table schema, when available.
    pub source_schema_hash: Option<String>,
    /// Stable hash of the target table schema.
    pub target_schema_hash: String,
    /// Stable hash of the full target, including RLS.
    pub target_hash: String,
    /// Stable hash of rendered table and RLS statements.
    pub plan_hash: String,
    /// Table migration plan.
    pub migration: PlannedMigration,
    /// RLS policy plan.
    pub rls: RlsPolicyPlan,
    /// Combined table migration and RLS statements in execution order.
    pub statements: Vec<String>,
}

/// Validation report for a full schema target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaTargetValidationReport {
    /// Backend name used for validation.
    pub backend: &'static str,
    /// Runtime schema policy used for validation.
    pub policy: SchemaPolicy,
    /// Table-schema validation report.
    pub schema: SchemaValidationReport,
    /// RLS-specific diagnostics.
    pub rls_diagnostics: Vec<SchemaDiagnostic>,
}

impl SchemaTargetValidationReport {
    /// Return true when table or RLS diagnostics include an error.
    pub fn has_errors(&self) -> bool {
        self.schema.has_errors()
            || self
                .rls_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EntityBackupDescriptor {
    pub entity_name: String,
    pub table_name: String,
    pub primary_key_column: String,
    /// Whether this entity's append-only enforcement supports bounded purge.
    #[serde(default)]
    pub retention_purge: bool,
    pub export_order: i32,
    pub restore_order: i32,
    pub columns: Vec<ColumnBackupDescriptor>,
    pub dependencies: Vec<EntityDependencyDescriptor>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ColumnBackupDescriptor {
    pub column_name: String,
    pub rust_field_name: String,
    pub logical_type: BackupValueKind,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_generated: bool,
    pub backup_policy: ColumnBackupPolicy,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EntityDependencyDescriptor {
    pub entity_name: String,
    pub table_name: String,
    pub source_column: String,
    pub target_column: String,
    pub nullable: bool,
    pub on_delete: DeletePolicy,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphqlOrmSchemaSnapshot {
    pub backend: String,
    pub migration_version: String,
    pub entities: Vec<EntityBackupDescriptor>,
    pub schema_hash: String,
}

/// A named schema ABI stage.
///
/// Each stage points at a complete target [`SchemaModel`]. The schema manager
/// can compute a forward plan from the currently applied stage to a later
/// stage.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaStage {
    pub version: String,
    pub description: String,
    pub target_schema: SchemaModel,
    pub target_schema_hash: String,
}

impl SchemaStage {
    pub fn new(
        version: impl Into<String>,
        description: impl Into<String>,
        target_schema: SchemaModel,
    ) -> Self {
        let target_schema_hash = target_schema.stable_hash();
        Self {
            version: version.into(),
            description: description.into(),
            target_schema,
            target_schema_hash,
        }
    }

    pub fn from_schema_model(
        version: impl Into<String>,
        description: impl Into<String>,
        target_schema: SchemaModel,
    ) -> Self {
        Self::new(version, description, target_schema)
    }

    pub fn from_entities(
        version: impl Into<String>,
        description: impl Into<String>,
        entities: &[&EntityMetadata],
    ) -> Self {
        Self::new(version, description, SchemaModel::from_entities(entities))
    }
}

/// Ordered schema ABI used for automatic forward upgrades.
///
/// Stages are interpreted in vector order. Versions must be unique, and a
/// database can be upgraded only along a known forward path.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaAbi {
    pub stages: Vec<SchemaStage>,
}

impl SchemaAbi {
    pub fn new(stages: Vec<SchemaStage>) -> crate::Result<Self> {
        let mut seen = std::collections::HashSet::new();
        for stage in &stages {
            if stage.version.trim().is_empty() {
                return Err(sqlx::Error::Protocol(
                    "Schema ABI stage version must not be empty".to_string(),
                ));
            }
            if !seen.insert(stage.version.clone()) {
                return Err(sqlx::Error::Protocol(format!(
                    "Duplicate schema ABI stage version: {}",
                    stage.version
                )));
            }
        }
        Ok(Self { stages })
    }

    pub fn latest(&self) -> Option<&SchemaStage> {
        self.stages.last()
    }

    pub fn stage(&self, version: &str) -> Option<&SchemaStage> {
        self.stages.iter().find(|stage| stage.version == version)
    }

    pub fn path(
        &self,
        from_version: Option<&str>,
        to_version: &str,
    ) -> crate::Result<Vec<&SchemaStage>> {
        let to_index = self
            .stages
            .iter()
            .position(|stage| stage.version == to_version)
            .ok_or_else(|| {
                sqlx::Error::Protocol(format!("Unknown target schema ABI version: {to_version}"))
            })?;

        let from_index = match from_version {
            Some(version) => Some(
                self.stages
                    .iter()
                    .position(|stage| stage.version == version)
                    .ok_or_else(|| {
                        sqlx::Error::Protocol(format!(
                            "Current schema ABI version {version} is not present in the ABI"
                        ))
                    })?,
            ),
            None => None,
        };

        if let Some(from_index) = from_index {
            if from_index > to_index {
                return Err(sqlx::Error::Protocol(format!(
                    "Cannot downgrade schema ABI from {} to {}",
                    self.stages[from_index].version, to_version
                )));
            }
        }

        let start = from_index.map(|index| index + 1).unwrap_or(0);
        Ok(self.stages[start..=to_index].iter().collect())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedSchemaStage {
    pub version: String,
    pub description: String,
    pub target_schema_hash: String,
    pub plan: MigrationPlan,
}

impl From<&EntityMetadata> for TableModel {
    fn from(value: &EntityMetadata) -> Self {
        let primary_keys = value
            .primary_keys
            .iter()
            .map(|column| (*column).to_string())
            .collect::<Vec<_>>();
        let mut table = Self {
            entity_name: value.entity_name.to_string(),
            table_name: value.table_name.to_string(),
            primary_key: value.primary_key.to_string(),
            primary_keys,
            default_sort: value.default_sort.to_string(),
            columns: value
                .fields
                .iter()
                .map(|field| ColumnModel {
                    name: field.name.to_string(),
                    sql_type: field.sql_type.to_string(),
                    spatial: field.spatial,
                    nullable: field.nullable,
                    is_primary_key: field.is_primary_key,
                    is_unique: field.is_unique,
                    default: field
                        .default
                        .map(super::dialect::canonicalize_column_default_expression),
                })
                .collect(),
            indexes: value.indexes.iter().cloned().collect(),
            composite_unique_indexes: value
                .composite_unique_indexes
                .iter()
                .map(|columns| columns.iter().map(|column| (*column).to_string()).collect())
                .collect(),
            foreign_keys: value
                .relations
                .iter()
                .filter(|relation| relation.emit_foreign_key)
                .map(|relation| ForeignKeyModel {
                    source_column: relation.source_column.to_string(),
                    target_table: relation.target_type.to_string(),
                    target_column: relation.target_column.to_string(),
                    is_multiple: relation.is_multiple,
                    on_delete: relation.on_delete.clone(),
                })
                .collect(),
            search_indexes: value
                .search
                .filter(|index| index.enabled)
                .map(|index| {
                    vec![SearchIndexModel {
                        name: index.index_name.to_string(),
                        table_name: index.table_name.to_string(),
                        entity_name: index.entity_name.to_string(),
                        primary_key: index.primary_key.to_string(),
                        strategy: index.strategy,
                        language: index.language.to_string(),
                        tokenizer: index.tokenizer.to_string(),
                        min_token_len: index.min_token_len,
                        fallback_enabled: index.fallback_enabled,
                        fields: index
                            .fields
                            .iter()
                            .map(|field| SearchFieldModel {
                                field_name: field.field_name.to_string(),
                                column_name: field.column_name.to_string(),
                                weight: field.weight,
                                alias: field.alias.map(str::to_string),
                                policy: field.policy.map(str::to_string),
                            })
                            .collect(),
                        json_paths: index
                            .json_paths
                            .iter()
                            .map(|json_path| SearchJsonPathModel {
                                field_name: json_path.field_name.to_string(),
                                column_name: json_path.column_name.to_string(),
                                path: json_path.path.to_string(),
                                weight: json_path.weight,
                                policy: json_path.policy.map(str::to_string),
                            })
                            .collect(),
                        relations: index
                            .relations
                            .iter()
                            .map(|relation| SearchRelationFieldModel {
                                relation_field: relation.relation_field.to_string(),
                                target_type: relation.target_type.to_string(),
                                fields: relation
                                    .fields
                                    .iter()
                                    .map(|field| (*field).to_string())
                                    .collect(),
                                weight: relation.weight,
                                max_items: relation.max_items,
                                policy: relation.policy.map(str::to_string),
                            })
                            .collect(),
                    }]
                })
                .unwrap_or_default(),
            append_only: value.append_only,
            retention_purge: value.retention_policy.is_some(),
            check_constraints: value
                .check_constraints
                .iter()
                .map(|constraint| CheckConstraintModel {
                    name: constraint.name.to_string(),
                    expression: constraint.expression.to_string(),
                })
                .collect(),
        };
        table.check_constraints.sort();

        for field in &value.fields {
            if field.is_filterable && field.spatial.is_none() {
                add_generated_index(&mut table, &[field.name]);
            }
        }

        table
    }
}

impl SchemaModel {
    pub fn from_entities(entities: &[&EntityMetadata]) -> Self {
        let entity_table_names = entities
            .iter()
            .map(|entity| (entity.entity_name, entity.table_name))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut extensions = Vec::new();
        if entities
            .iter()
            .flat_map(|entity| entity.fields.iter())
            .any(|field| field.spatial.is_some())
        {
            extensions.push("postgis".to_string());
        }

        let mut tables = entities
            .iter()
            .map(|entity| {
                let mut table = TableModel::from(*entity);
                for foreign_key in &mut table.foreign_keys {
                    if let Some(table_name) =
                        entity_table_names.get(foreign_key.target_table.as_str())
                    {
                        foreign_key.target_table = (*table_name).to_string();
                    }
                }
                table
            })
            .collect::<Vec<_>>();

        let table_positions = tables
            .iter()
            .enumerate()
            .map(|(index, table)| (table.table_name.clone(), index))
            .collect::<std::collections::BTreeMap<_, _>>();

        for entity in entities {
            for relation in &entity.relations {
                let (table_name, columns) = if relation.is_multiple {
                    let Some(target_table) = entity_table_names.get(relation.target_type) else {
                        continue;
                    };
                    (*target_table, relation.target_columns)
                } else {
                    (entity.table_name, relation.source_columns)
                };

                let Some(table_index) = table_positions.get(table_name) else {
                    continue;
                };
                add_generated_index(&mut tables[*table_index], columns);
            }
        }

        Self { extensions, tables }
    }
}

pub fn backup_descriptors_from_entities(
    entities: &[&EntityMetadata],
) -> Vec<EntityBackupDescriptor> {
    let entity_table_names = entities
        .iter()
        .map(|entity| (entity.entity_name, entity.table_name))
        .collect::<std::collections::BTreeMap<_, _>>();
    let entity_metadata_by_table = entities
        .iter()
        .map(|entity| (entity.table_name, *entity))
        .collect::<std::collections::BTreeMap<_, _>>();
    let restore_ranks = restore_ranks_from_entities(entities, &entity_table_names);

    entities
        .iter()
        .filter(|entity| entity.backup_enabled)
        .map(|entity| {
            let restore_order = entity
                .backup_restore_order
                .unwrap_or_else(|| restore_ranks.get(entity.table_name).copied().unwrap_or(0) * 10);
            let export_order = entity.backup_export_order.unwrap_or(restore_order);
            let columns = entity
                .fields
                .iter()
                .map(|field| ColumnBackupDescriptor {
                    column_name: field.name.to_string(),
                    rust_field_name: field.rust_name.to_string(),
                    logical_type: field.logical_type,
                    nullable: field.nullable,
                    is_primary_key: field.is_primary_key,
                    is_generated: field.is_generated,
                    backup_policy: field.backup_policy,
                })
                .collect::<Vec<_>>();
            let dependencies = entity
                .relations
                .iter()
                .filter(|relation| relation.emit_foreign_key)
                .filter_map(|relation| {
                    let target_table = entity_table_names
                        .get(relation.target_type)
                        .copied()
                        .unwrap_or(relation.target_type);
                    let target_entity = entity_metadata_by_table
                        .get(target_table)
                        .map(|metadata| metadata.entity_name)
                        .unwrap_or(relation.target_type);
                    let nullable = entity
                        .fields
                        .iter()
                        .find(|field| field.name == relation.source_column)
                        .map(|field| field.nullable)
                        .unwrap_or(false);
                    Some(EntityDependencyDescriptor {
                        entity_name: target_entity.to_string(),
                        table_name: target_table.to_string(),
                        source_column: relation.source_column.to_string(),
                        target_column: relation.target_column.to_string(),
                        nullable,
                        on_delete: relation.on_delete.clone(),
                    })
                })
                .collect::<Vec<_>>();

            EntityBackupDescriptor {
                entity_name: entity.entity_name.to_string(),
                table_name: entity.table_name.to_string(),
                primary_key_column: entity.primary_key.to_string(),
                retention_purge: entity.retention_policy.is_some(),
                export_order,
                restore_order,
                columns,
                dependencies,
            }
        })
        .collect()
}

pub fn schema_snapshot_from_entities(
    backend: DatabaseBackend,
    migration_version: impl Into<String>,
    entities: &[&EntityMetadata],
) -> GraphqlOrmSchemaSnapshot {
    let entities = backup_descriptors_from_entities(entities);
    let schema_hash = stable_schema_hash(&entities);
    GraphqlOrmSchemaSnapshot {
        backend: format!("{backend:?}"),
        migration_version: migration_version.into(),
        entities,
        schema_hash,
    }
}

pub fn stable_schema_hash(entities: &[EntityBackupDescriptor]) -> String {
    let mut canonical = String::new();
    let mut entities = entities.iter().collect::<Vec<_>>();
    entities.sort_by(|left, right| left.table_name.cmp(&right.table_name));

    for entity in entities {
        canonical.push_str("entity:");
        canonical.push_str(&entity.entity_name);
        canonical.push('|');
        canonical.push_str(&entity.table_name);
        canonical.push('|');
        canonical.push_str(&entity.primary_key_column);
        if entity.retention_purge {
            canonical.push_str("|retention_purge");
        }
        canonical.push('\n');

        let mut columns = entity.columns.iter().collect::<Vec<_>>();
        columns.sort_by(|left, right| left.column_name.cmp(&right.column_name));
        for column in columns {
            canonical.push_str("column:");
            canonical.push_str(&column.column_name);
            canonical.push('|');
            canonical.push_str(&column.rust_field_name);
            canonical.push('|');
            canonical.push_str(column.logical_type.as_schema_str());
            canonical.push('|');
            canonical.push_str(if column.nullable {
                "nullable"
            } else {
                "not_null"
            });
            canonical.push('|');
            canonical.push_str(if column.is_primary_key {
                "pk"
            } else {
                "not_pk"
            });
            canonical.push('|');
            canonical.push_str(if column.is_generated {
                "generated"
            } else {
                "not_generated"
            });
            canonical.push('|');
            canonical.push_str(match column.backup_policy {
                ColumnBackupPolicy::Include => "include",
                ColumnBackupPolicy::Exclude => "exclude",
                ColumnBackupPolicy::Redact => "redact",
            });
            canonical.push('\n');
        }

        let mut dependencies = entity.dependencies.iter().collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            (
                &left.table_name,
                &left.source_column,
                &left.target_column,
                &left.on_delete,
            )
                .cmp(&(
                    &right.table_name,
                    &right.source_column,
                    &right.target_column,
                    &right.on_delete,
                ))
        });
        for dependency in dependencies {
            canonical.push_str("dependency:");
            canonical.push_str(&dependency.table_name);
            canonical.push('|');
            canonical.push_str(&dependency.source_column);
            canonical.push('|');
            canonical.push_str(&dependency.target_column);
            canonical.push('|');
            canonical.push_str(if dependency.nullable {
                "nullable"
            } else {
                "not_null"
            });
            canonical.push('|');
            canonical.push_str(dependency.on_delete.as_sql());
            canonical.push('\n');
        }
    }

    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}

pub fn stable_schema_model_hash(schema: &SchemaModel) -> String {
    let mut canonical = String::new();
    let mut extensions = schema.extensions.iter().collect::<Vec<_>>();
    extensions.sort();
    for extension in extensions {
        canonical.push_str("extension:");
        canonical.push_str(extension);
        canonical.push('\n');
    }

    let mut tables = schema.tables.iter().collect::<Vec<_>>();
    tables.sort_by(|left, right| left.table_name.cmp(&right.table_name));

    for table in tables {
        canonical.push_str("table:");
        canonical.push_str(&table.entity_name);
        canonical.push('|');
        canonical.push_str(&table.table_name);
        canonical.push('|');
        canonical.push_str(&table.primary_key);
        canonical.push('|');
        canonical.push_str(&table.primary_keys().join(","));
        canonical.push('|');
        canonical.push_str(&table.default_sort);
        canonical.push('|');
        canonical.push_str(if table.append_only {
            "append_only"
        } else {
            "mutable"
        });
        if table.retention_purge {
            canonical.push_str("|retention_purge");
        }
        canonical.push('\n');

        let mut constraints = table.check_constraints.iter().collect::<Vec<_>>();
        constraints.sort();
        for constraint in constraints {
            canonical.push_str("check:");
            canonical.push_str(&constraint.name);
            canonical.push('|');
            canonical.push_str(&constraint.expression);
            canonical.push('\n');
        }

        let mut columns = table.columns.iter().collect::<Vec<_>>();
        columns.sort_by(|left, right| left.name.cmp(&right.name));
        for column in columns {
            canonical.push_str("column:");
            canonical.push_str(&column.name);
            canonical.push('|');
            canonical.push_str(&column.sql_type);
            canonical.push('|');
            canonical.push_str(if column.nullable {
                "nullable"
            } else {
                "not_null"
            });
            canonical.push('|');
            canonical.push_str(if column.is_primary_key {
                "pk"
            } else {
                "not_pk"
            });
            canonical.push('|');
            canonical.push_str(if column.is_unique {
                "unique"
            } else {
                "not_unique"
            });
            canonical.push('|');
            canonical.push_str(
                &column
                    .default
                    .as_deref()
                    .map(super::dialect::canonicalize_column_default_expression)
                    .unwrap_or_default(),
            );
            canonical.push('|');
            if let Some(spatial) = column.spatial {
                canonical.push_str(spatial.kind.as_sql());
                canonical.push(':');
                canonical.push_str(spatial.geometry_type.as_sql());
                canonical.push(':');
                canonical.push_str(&spatial.srid.to_string());
            }
            canonical.push('\n');
        }

        let mut indexes = table.indexes.iter().collect::<Vec<_>>();
        indexes.sort_by(|left, right| left.name.cmp(right.name));
        for index in indexes {
            canonical.push_str("index:");
            canonical.push_str(index.name);
            canonical.push('|');
            canonical.push_str(&index.columns.join(","));
            canonical.push('|');
            canonical.push_str(if index.is_unique { "unique" } else { "index" });
            canonical.push('|');
            canonical.push_str(match index.method {
                IndexMethod::Default => "default",
                IndexMethod::Gist => "gist",
            });
            canonical.push('|');
            canonical.push_str(if index.is_spatial {
                "spatial"
            } else {
                "not_spatial"
            });
            canonical.push('|');
            if let Some(predicate) = &index.predicate {
                canonical.push_str("where_in:");
                canonical.push_str(predicate.column);
                canonical.push(':');
                canonical.push_str(&predicate.values.join(","));
            } else {
                canonical.push_str("no_predicate");
            }
            canonical.push('\n');
        }

        let mut search_indexes = table.search_indexes.iter().collect::<Vec<_>>();
        search_indexes.sort_by(|left, right| left.name.cmp(&right.name));
        for index in search_indexes {
            canonical.push_str("search_index:");
            canonical.push_str(&index.name);
            canonical.push('|');
            canonical.push_str(index.strategy.as_str());
            canonical.push('|');
            canonical.push_str(&index.language);
            canonical.push('|');
            canonical.push_str(&index.tokenizer);
            canonical.push('|');
            canonical.push_str(&index.min_token_len.to_string());
            canonical.push('|');
            canonical.push_str(if index.fallback_enabled {
                "fallback"
            } else {
                "native_only"
            });
            canonical.push('\n');

            let mut fields = index.fields.iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.field_name.cmp(&right.field_name));
            for field in fields {
                canonical.push_str("search_field:");
                canonical.push_str(&field.field_name);
                canonical.push('|');
                canonical.push_str(&field.column_name);
                canonical.push('|');
                canonical.push_str(field.weight.as_str());
                canonical.push('|');
                canonical.push_str(field.alias.as_deref().unwrap_or(""));
                canonical.push('|');
                canonical.push_str(field.policy.as_deref().unwrap_or(""));
                canonical.push('\n');
            }

            let mut json_paths = index.json_paths.iter().collect::<Vec<_>>();
            json_paths.sort_by(|left, right| {
                left.field_name
                    .cmp(&right.field_name)
                    .then_with(|| left.path.cmp(&right.path))
            });
            for json_path in json_paths {
                canonical.push_str("search_json_path:");
                canonical.push_str(&json_path.field_name);
                canonical.push('|');
                canonical.push_str(&json_path.column_name);
                canonical.push('|');
                canonical.push_str(&json_path.path);
                canonical.push('|');
                canonical.push_str(json_path.weight.as_str());
                canonical.push('|');
                canonical.push_str(json_path.policy.as_deref().unwrap_or(""));
                canonical.push('\n');
            }

            let mut relations = index.relations.iter().collect::<Vec<_>>();
            relations.sort_by(|left, right| left.relation_field.cmp(&right.relation_field));
            for relation in relations {
                canonical.push_str("search_relation:");
                canonical.push_str(&relation.relation_field);
                canonical.push('|');
                canonical.push_str(&relation.target_type);
                canonical.push('|');
                canonical.push_str(&relation.fields.join(","));
                canonical.push('|');
                canonical.push_str(relation.weight.as_str());
                canonical.push('|');
                canonical.push_str(&relation.max_items.to_string());
                canonical.push('|');
                canonical.push_str(relation.policy.as_deref().unwrap_or(""));
                canonical.push('\n');
            }
        }

        let mut foreign_keys = table.foreign_keys.iter().collect::<Vec<_>>();
        foreign_keys.sort_by(|left, right| {
            (
                &left.source_column,
                &left.target_table,
                &left.target_column,
                &left.on_delete,
            )
                .cmp(&(
                    &right.source_column,
                    &right.target_table,
                    &right.target_column,
                    &right.on_delete,
                ))
        });
        for foreign_key in foreign_keys {
            canonical.push_str("foreign_key:");
            canonical.push_str(&foreign_key.source_column);
            canonical.push('|');
            canonical.push_str(&foreign_key.target_table);
            canonical.push('|');
            canonical.push_str(&foreign_key.target_column);
            canonical.push('|');
            canonical.push_str(foreign_key.on_delete.as_sql());
            canonical.push('\n');
        }
    }

    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}

/// Compute the stable hash used for generated RLS schema models.
///
/// The hash is order-independent for entities and operation policies, making it
/// suitable for plan metadata and drift checks.
pub fn stable_rls_schema_hash(schema: &RlsSchemaModel) -> String {
    let mut canonical = String::new();
    let mut entities = schema.entities.iter().collect::<Vec<_>>();
    entities.sort_by(|left, right| left.table_name.cmp(right.table_name));

    for entity in entities {
        canonical.push_str("rls_table:");
        canonical.push_str(entity.entity_name);
        canonical.push('|');
        canonical.push_str(entity.table_name);
        canonical.push('|');
        canonical.push_str(if entity.force { "force" } else { "enable" });
        canonical.push('\n');

        let mut policies = entity.policies.iter().collect::<Vec<_>>();
        policies.sort_by(|left, right| left.operation.cmp(&right.operation));
        for policy in policies {
            canonical.push_str("policy:");
            canonical.push_str(policy.operation.as_str());
            canonical.push('|');
            canonical.push_str(policy.predicate.unwrap_or(""));
            canonical.push('|');
            canonical.push_str(policy.scope.unwrap_or(""));
            canonical.push('|');
            canonical.push_str(policy.tenant_column.unwrap_or(""));
            canonical.push('|');
            canonical.push_str(policy.owner_column.unwrap_or(""));
            canonical.push('\n');
        }
    }

    format!("{:016x}", fnv1a64(canonical.as_bytes()))
}

impl BackupValueKind {
    pub fn as_schema_str(self) -> &'static str {
        match self {
            BackupValueKind::Null => "null",
            BackupValueKind::Bool => "bool",
            BackupValueKind::Integer => "integer",
            BackupValueKind::Float => "float",
            BackupValueKind::String => "string",
            BackupValueKind::Uuid => "uuid",
            BackupValueKind::Json => "json",
            BackupValueKind::Bytes => "bytes",
        }
    }
}

fn restore_ranks_from_entities(
    entities: &[&EntityMetadata],
    entity_table_names: &std::collections::BTreeMap<&'static str, &'static str>,
) -> std::collections::BTreeMap<&'static str, i32> {
    fn visit(
        table_name: &'static str,
        tables: &std::collections::BTreeMap<&'static str, &EntityMetadata>,
        entity_table_names: &std::collections::BTreeMap<&'static str, &'static str>,
        visiting: &mut std::collections::BTreeSet<&'static str>,
        visited: &mut std::collections::BTreeSet<&'static str>,
        ordered: &mut Vec<&'static str>,
    ) {
        if visited.contains(table_name) || visiting.contains(table_name) {
            return;
        }
        let Some(entity) = tables.get(table_name) else {
            return;
        };

        visiting.insert(table_name);
        for relation in entity
            .relations
            .iter()
            .filter(|relation| relation.emit_foreign_key)
        {
            let target_table = entity_table_names
                .get(relation.target_type)
                .copied()
                .unwrap_or(relation.target_type);
            if tables.contains_key(target_table) {
                visit(
                    target_table,
                    tables,
                    entity_table_names,
                    visiting,
                    visited,
                    ordered,
                );
            }
        }
        visiting.remove(table_name);
        visited.insert(table_name);
        ordered.push(table_name);
    }

    let tables = entities
        .iter()
        .filter(|entity| entity.backup_enabled)
        .map(|entity| (entity.table_name, *entity))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut ordered = Vec::new();
    let mut visiting = std::collections::BTreeSet::new();
    let mut visited = std::collections::BTreeSet::new();
    for table_name in tables.keys() {
        visit(
            table_name,
            &tables,
            entity_table_names,
            &mut visiting,
            &mut visited,
            &mut ordered,
        );
    }

    ordered
        .into_iter()
        .enumerate()
        .map(|(index, table_name)| (table_name, index as i32))
        .collect()
}

pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Clone, Debug, PartialEq)]
pub enum MigrationStep {
    EnableExtension {
        name: String,
    },
    CreateTable(TableModel),
    DropTable {
        table_name: String,
    },
    AddColumn {
        table_name: String,
        column: ColumnModel,
    },
    DropColumn {
        table_name: String,
        column_name: String,
    },
    AlterColumn {
        table_name: String,
        before: ColumnModel,
        after: ColumnModel,
    },
    CreateIndex {
        table_name: String,
        index: IndexMetadata,
    },
    DropIndex {
        table_name: String,
        index_name: String,
    },
    CreateSearchIndex {
        table_name: String,
        index: SearchIndexModel,
    },
    DropSearchIndex {
        table_name: String,
        index_name: String,
    },
    AlterSearchIndex {
        table_name: String,
        before: SearchIndexModel,
        after: SearchIndexModel,
    },
    AddForeignKey {
        table_name: String,
        foreign_key: ForeignKeyModel,
    },
    DropForeignKey {
        table_name: String,
        foreign_key: ForeignKeyModel,
    },
    SetAppendOnly {
        /// Managed physical table whose enforcement contract changes.
        table_name: String,
        /// Whether append-only enforcement is enabled after this step.
        enabled: bool,
        /// Whether enforcement admits exact ORM bounded-retention context.
        retention_purge: bool,
    },
    SetCheckConstraints {
        table_name: String,
        before: Vec<CheckConstraintModel>,
        after: Vec<CheckConstraintModel>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaDiff {
    pub steps: Vec<MigrationStep>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MigrationPlan {
    pub backend: DatabaseBackend,
    pub steps: Vec<MigrationStep>,
    pub statements: Vec<String>,
}

/// Runtime schema ownership and safety policy.
///
/// Backend features compile database support. `SchemaPolicy` decides which
/// schema-management operations and generated write paths are allowed for a
/// concrete [`crate::db::Database`] value.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SchemaPolicy {
    /// Existing database is the source of truth; query and validation only.
    ExternalReadOnly,
    /// Existing database is the source of truth; entity writes may run, schema application may not.
    ExternalWritable,
    /// Compare metadata to the live schema without planning or applying changes.
    ValidateOnly,
    /// Validate and build migration plans without applying them.
    PlanOnly,
    /// Rust entity metadata is the source of truth; explicit application is allowed.
    Managed,
}

impl SchemaPolicy {
    pub const fn allows_entity_writes(self) -> bool {
        !matches!(self, Self::ExternalReadOnly)
    }

    pub const fn allows_validation(self) -> bool {
        true
    }

    pub const fn allows_planning(self) -> bool {
        matches!(
            self,
            Self::ExternalWritable | Self::PlanOnly | Self::Managed
        )
    }

    pub const fn allows_application(self) -> bool {
        matches!(self, Self::Managed)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExternalReadOnly => "external_read_only",
            Self::ExternalWritable => "external_writable",
            Self::ValidateOnly => "validate_only",
            Self::PlanOnly => "plan_only",
            Self::Managed => "managed",
        }
    }
}

impl std::fmt::Display for SchemaPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SchemaPolicy {
    type Err = sqlx::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "external_read_only" => Ok(Self::ExternalReadOnly),
            "external_writable" => Ok(Self::ExternalWritable),
            "validate_only" => Ok(Self::ValidateOnly),
            "plan_only" => Ok(Self::PlanOnly),
            "managed" => Ok(Self::Managed),
            other => Err(sqlx::Error::Protocol(format!(
                "Unknown graphql-orm schema policy: {other}"
            ))),
        }
    }
}

/// Safety switches for explicit migration application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyOptions {
    /// Permit steps classified as [`MigrationRisk::Destructive`].
    pub allow_destructive: bool,
    /// Reject any step that is not classified as [`MigrationRisk::Additive`].
    ///
    /// This is useful for service startup paths that may create missing ORM
    /// tables/indexes but must not alter or rebuild existing structures.
    pub additive_only: bool,
    /// Validate the live schema against the expected ABI baseline before upgrades.
    pub require_clean_schema: bool,
    /// Produce an application report without executing migration SQL.
    pub dry_run: bool,
    /// Optional source schema hash that must match the planned baseline.
    pub expected_current_schema_hash: Option<String>,
    /// Record successful application in `__graphql_orm_migrations`.
    pub record_history: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            allow_destructive: false,
            additive_only: false,
            require_clean_schema: true,
            dry_run: false,
            expected_current_schema_hash: None,
            record_history: true,
        }
    }
}

/// Options that affect how migration plans are built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanOptions {
    /// Ignore live tables that are not present in the target schema.
    ///
    /// Existing behavior treats extra live tables as drift and plans drops for
    /// them. Enable this when graphql-orm owns only a subset of tables in a
    /// shared database.
    pub ignore_unmanaged_tables: bool,
}

impl PlanOptions {
    /// Plan against the full current and target schemas.
    pub const fn strict() -> Self {
        Self {
            ignore_unmanaged_tables: false,
        }
    }

    /// Plan only tables present in the target schema and ignore unrelated live tables.
    pub const fn managed_tables_only() -> Self {
        Self {
            ignore_unmanaged_tables: true,
        }
    }
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self::strict()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaDiagnosticKind {
    MissingTable,
    ExtraTable,
    MissingColumn,
    ExtraColumn,
    NullabilityMismatch,
    TypeMismatch,
    PrimaryKeyMismatch,
    IndexMismatch,
    UniqueConstraintMismatch,
    ForeignKeyMismatch,
    UnsupportedBackendCapability,
    WriteFieldOnReadOnlyBackend,
    ReadOnlyFieldWritable,
    RlsMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaDiagnostic {
    pub severity: SchemaDiagnosticSeverity,
    pub kind: SchemaDiagnosticKind,
    pub table: Option<String>,
    pub column: Option<String>,
    pub message: String,
}

/// Structured result of validating a current schema model against a target model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaValidationReport {
    pub backend: &'static str,
    pub policy: SchemaPolicy,
    pub current_schema_hash: Option<String>,
    pub target_schema_hash: String,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

impl SchemaValidationReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == SchemaDiagnosticSeverity::Error)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MigrationRisk {
    Additive,
    Compatible,
    Risky,
    Destructive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedMigrationStep {
    pub step: MigrationStep,
    pub risk: MigrationRisk,
    pub reason: String,
}

/// A backend-rendered migration plan with stable hashes and risk-classified steps.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedMigration {
    pub version: String,
    pub description: String,
    pub backend: &'static str,
    pub source_schema_hash: Option<String>,
    pub target_schema_hash: String,
    pub plan_hash: String,
    pub steps: Vec<PlannedMigrationStep>,
    pub statements: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedSchemaUpgrade {
    pub stages: Vec<PlannedMigration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedMigrationReport {
    pub version: String,
    pub dry_run: bool,
    pub statements_applied: usize,
    /// True when the version was already present in migration history and no
    /// statements were re-executed. Restart/idempotent apply paths set this.
    pub already_applied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedSchemaUpgrade {
    pub applied: Vec<AppliedMigrationReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedMigrationRecord {
    pub version: String,
    pub description: String,
    pub applied_at: Option<String>,
    pub backend: Option<String>,
    pub graphql_orm_version: Option<String>,
    pub source_schema_hash: Option<String>,
    pub target_schema_hash: Option<String>,
    pub plan_hash: Option<String>,
    pub policy: Option<String>,
}

impl AppliedMigrationRecord {
    pub fn legacy(version: String) -> Self {
        Self {
            version,
            description: String::new(),
            applied_at: None,
            backend: None,
            graphql_orm_version: None,
            source_schema_hash: None,
            target_schema_hash: None,
            plan_hash: None,
            policy: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationApplicationMetadata {
    pub backend: &'static str,
    pub graphql_orm_version: &'static str,
    pub source_schema_hash: Option<String>,
    pub target_schema_hash: String,
    pub plan_hash: String,
    pub policy: SchemaPolicy,
}
