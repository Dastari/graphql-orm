use super::dialect::DatabaseBackend;
use super::query::{
    ChangeAction, DatabaseEntity, DatabaseSchema, DatabaseSearchSchema, EntityRelations,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::query::{DatabaseFilter, DatabaseOrderBy, EntityQuery, FromSqlRow};
use super::{DefaultBackend, OrmBackend};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use super::{DefaultWriteBackend, WriteBackend};
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

/// Request-local database authorization context for PostgreSQL RLS.
///
/// Attach this value to an `async-graphql` request with `request.data(...)`.
/// Generated PostgreSQL resolvers read it from the GraphQL context and apply it
/// as transaction-local `app.*` settings before executing database work. When it
/// is absent, generated resolvers use the existing non-RLS-aware execution path.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub claims_json: Option<serde_json::Value>,
}

impl DbAuthContext {
    /// Return a stable key for batching and cache partitioning.
    ///
    /// Roles and scopes are sorted in the key so equivalent contexts do not
    /// batch separately due only to caller ordering.
    pub fn canonical_key(&self) -> String {
        let mut roles = self.roles.clone();
        roles.sort();
        let mut scopes = self.scopes.clone();
        scopes.sort();
        serde_json::json!({
            "user_id": self.user_id,
            "subject": self.subject,
            "tenant_id": self.tenant_id,
            "roles": roles,
            "scopes": scopes,
            "claims_json": self.claims_json,
        })
        .to_string()
    }

    /// Render PostgreSQL setting names and values for transaction-local auth.
    ///
    /// The returned values are intended for `set_config(name, value, true)`.
    pub fn postgres_settings(&self) -> Result<Vec<(&'static str, String)>, sqlx::Error> {
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

        Ok(vec![
            ("app.user_id", self.user_id.clone().unwrap_or_default()),
            ("app.subject", self.subject.clone().unwrap_or_default()),
            ("app.tenant_id", self.tenant_id.clone().unwrap_or_default()),
            ("app.roles", roles),
            ("app.scopes", scopes),
            ("app.claims", claims),
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

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub struct MutationQuery<'ctx, 'tx, T, B: WriteBackend = DefaultWriteBackend>
where
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
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
    T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    fn new(hook_ctx: &'ctx mut MutationContext<'tx, B>) -> Self {
        Self {
            hook_ctx,
            query: EntityQuery::new(),
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

    pub async fn fetch_all(self) -> Result<Vec<T>, sqlx::Error> {
        let query = self.query;
        query.fetch_all_on(self.hook_ctx.executor()).await
    }

    pub async fn fetch_one(self) -> Result<Option<T>, sqlx::Error> {
        let query = self.query;
        query.fetch_one_on(self.hook_ctx.executor()).await
    }

    pub async fn count(self) -> Result<i64, sqlx::Error> {
        let query = self.query;
        query.count_on(self.hook_ctx.executor()).await
    }

    pub async fn exists(self) -> Result<bool, sqlx::Error> {
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
        use crate::graphql::auth::AuthExt;

        self.graphql_ctx
            .ok_or_else(|| async_graphql::Error::new("missing GraphQL context for auth user"))?
            .auth_user()
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

    pub async fn fetch_all(self) -> Result<Vec<T>, sqlx::Error> {
        let query = self.query;
        if let Some(hook_ctx) = self.write_ctx.mutation_ctx.as_deref_mut() {
            query.fetch_all_on(hook_ctx.executor()).await
        } else {
            query.fetch_all(self.write_ctx.database()).await
        }
    }

    pub async fn fetch_one(self) -> Result<Option<T>, sqlx::Error> {
        let query = self.query;
        if let Some(hook_ctx) = self.write_ctx.mutation_ctx.as_deref_mut() {
            query.fetch_one_on(hook_ctx.executor()).await
        } else {
            query.fetch_one(self.write_ctx.database()).await
        }
    }

    pub async fn count(self) -> Result<i64, sqlx::Error> {
        let query = self.query;
        if let Some(hook_ctx) = self.write_ctx.mutation_ctx.as_deref_mut() {
            query.count_on(hook_ctx.executor()).await
        } else {
            query.count(self.write_ctx.database()).await
        }
    }

    pub async fn exists(self) -> Result<bool, sqlx::Error> {
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
        use crate::graphql::auth::AuthExt;

        ctx.ok_or_else(|| async_graphql::Error::new("missing GraphQL context for auth user"))?
            .auth_user()
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

    pub async fn commit_and_emit(self) -> Result<(), sqlx::Error> {
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
    ) -> Result<T, sqlx::Error>
    where
        T: MutationContextInsert<B>,
    {
        T::insert_in_mutation_context(self, input).await
    }

    pub async fn upsert<'a, T>(
        &'a mut self,
        input: <T as MutationContextUpsert<B>>::UpsertInput,
    ) -> Result<UpsertOutcome<T>, sqlx::Error>
    where
        T: MutationContextUpsert<B>,
    {
        T::upsert_in_mutation_context(self, input).await
    }

    pub async fn update_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextUpdateById<B>>::Id,
        input: <T as MutationContextUpdateById<B>>::UpdateInput,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: MutationContextUpdateById<B>,
    {
        T::update_by_id_in_mutation_context(self, id, input).await
    }

    pub async fn update_where<'a, T>(
        &'a mut self,
        where_input: <T as MutationContextUpdateWhere<B>>::WhereInput,
        input: <T as MutationContextUpdateWhere<B>>::UpdateInput,
    ) -> Result<i64, sqlx::Error>
    where
        T: MutationContextUpdateWhere<B>,
    {
        T::update_where_in_mutation_context(self, where_input, input).await
    }

    pub async fn delete_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextDeleteById<B>>::Id,
    ) -> Result<bool, sqlx::Error>
    where
        T: MutationContextDeleteById<B>,
    {
        T::delete_by_id_in_mutation_context(self, id).await
    }

    pub async fn delete_where<'a, T>(
        &'a mut self,
        where_input: <T as MutationContextDeleteWhere<B>>::WhereInput,
    ) -> Result<i64, sqlx::Error>
    where
        T: MutationContextDeleteWhere<B>,
    {
        T::delete_where_in_mutation_context(self, where_input).await
    }

    pub fn query<'a, T>(&'a mut self) -> MutationQuery<'a, 'tx, T, B>
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
        T: DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        MutationQuery::new(self)
    }

    pub async fn find_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextFindById<B>>::Id,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: MutationContextFindById<B>,
    {
        T::find_by_id_in_mutation_context(self, id).await
    }
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
    ) -> futures::future::BoxFuture<'a, Result<Self, sqlx::Error>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpsert<B: WriteBackend = DefaultWriteBackend>: Sized {
    type UpsertInput;

    fn upsert_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        input: Self::UpsertInput,
    ) -> futures::future::BoxFuture<'a, Result<UpsertOutcome<Self>, sqlx::Error>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpdateById<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;
    type UpdateInput;

    fn update_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, Result<Option<Self>, sqlx::Error>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextUpdateWhere<B: WriteBackend = DefaultWriteBackend>: Sized {
    type WhereInput;
    type UpdateInput;

    fn update_where_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        where_input: Self::WhereInput,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, Result<i64, sqlx::Error>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextDeleteById<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;

    fn delete_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
    ) -> futures::future::BoxFuture<'a, Result<bool, sqlx::Error>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextDeleteWhere<B: WriteBackend = DefaultWriteBackend>: Sized {
    type WhereInput;

    fn delete_where_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        where_input: Self::WhereInput,
    ) -> futures::future::BoxFuture<'a, Result<i64, sqlx::Error>>;
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub trait MutationContextFindById<B: WriteBackend = DefaultWriteBackend>: Sized {
    type Id;

    fn find_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_, B>,
        id: &'a Self::Id,
    ) -> futures::future::BoxFuture<'a, Result<Option<Self>, sqlx::Error>>;
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

pub fn entity_state<T>(value: &T) -> Result<EntityState, sqlx::Error>
where
    T: serde::Serialize + Clone + Send + Sync + 'static,
{
    let json = serde_json::to_value(value).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    Ok(EntityState {
        value: Arc::new(value.clone()),
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
    pub sql_type: &'static str,
    pub spatial: Option<SpatialColumnDef>,
    pub search: Option<SearchFieldDef>,
    pub logical_type: BackupValueKind,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_generated: bool,
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
            sql_type,
            spatial: None,
            search: None,
            logical_type: BackupValueKind::String,
            nullable: true,
            is_primary_key: false,
            is_unique: false,
            is_generated: false,
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

    pub const fn rust_name(mut self, rust_name: &'static str) -> Self {
        self.rust_name = rust_name;
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
    pub sql_type: &'static str,
    pub spatial: Option<SpatialColumnDef>,
    pub search: Option<SearchFieldDef>,
    pub logical_type: BackupValueKind,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_generated: bool,
    pub backup_policy: ColumnBackupPolicy,
    pub default: Option<&'static str>,
    pub references: Option<&'static str>,
}

impl From<&ColumnDef> for FieldMetadata {
    fn from(value: &ColumnDef) -> Self {
        Self {
            name: value.name,
            rust_name: value.rust_name,
            sql_type: value.sql_type,
            spatial: value.spatial,
            search: value.search,
            logical_type: value.logical_type,
            nullable: value.nullable,
            is_primary_key: value.is_primary_key,
            is_unique: value.is_unique,
            is_generated: value.is_generated,
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
}

impl IndexDef {
    pub const fn new(name: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            name,
            columns,
            is_unique: false,
            method: IndexMethod::Default,
            is_spatial: false,
        }
    }

    pub const fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }

    pub const fn spatial_gist(name: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            name,
            columns,
            is_unique: false,
            method: IndexMethod::Gist,
            is_spatial: true,
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
    pub fields: Box<[FieldMetadata]>,
    pub indexes: Box<[IndexMetadata]>,
    pub composite_unique_indexes: Box<[Box<[&'static str]>]>,
    pub relations: Box<[RelationMetadata]>,
    pub search: Option<&'static SearchIndexDef>,
}

impl EntityMetadata {
    pub fn from_schema<T>(
        entity_name: &'static str,
        backup_enabled: bool,
        backup_export_order: Option<i32>,
        backup_restore_order: Option<i32>,
        read_policy: Option<&'static str>,
        write_policy: Option<&'static str>,
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
    pub fn new(stages: Vec<SchemaStage>) -> Result<Self, sqlx::Error> {
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
    ) -> Result<Vec<&SchemaStage>, sqlx::Error> {
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
        Self {
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
                    default: field.default.map(str::to_string),
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
        }
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

        Self {
            extensions,
            tables: entities
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
                .collect(),
        }
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
        canonical.push('\n');

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
            canonical.push_str(column.default.as_deref().unwrap_or(""));
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

fn fnv1a64(bytes: &[u8]) -> u64 {
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
            require_clean_schema: true,
            dry_run: false,
            expected_current_schema_hash: None,
            record_history: true,
        }
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
