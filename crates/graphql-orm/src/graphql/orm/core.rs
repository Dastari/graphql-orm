use super::dialect::DatabaseBackend;
use super::query::{
    ChangeAction, DatabaseEntity, DatabaseFilter, DatabaseOrderBy, DatabaseSchema, EntityQuery,
    EntityRelations, FromSqlRow,
};
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static QUERY_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    String(String),
    Json(serde_json::Value),
    JsonNull,
    Uuid(uuid::Uuid),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MutationPhase {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MutationFieldValue {
    pub field: String,
    pub value: SqlValue,
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

trait DeferredEventEmitter: Send {
    fn emit(self: Box<Self>, db: &crate::db::Database);
}

trait PostCommitActionRunner: Send {
    fn run(
        self: Box<Self>,
        db: crate::db::Database,
    ) -> futures::future::BoxFuture<'static, Result<(), String>>;
}

struct DeferredEvent<T>
where
    T: Clone + Send + Sync + 'static,
{
    event: T,
}

impl<T> DeferredEventEmitter for DeferredEvent<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn emit(self: Box<Self>, db: &crate::db::Database) {
        db.emit_event(self.event);
    }
}

struct DeferredAction<F> {
    action: Option<F>,
}

impl<F, Fut, E> PostCommitActionRunner for DeferredAction<F>
where
    F: FnOnce(crate::db::Database) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    fn run(
        mut self: Box<Self>,
        db: crate::db::Database,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        let action = self
            .action
            .take()
            .expect("deferred post-commit action was already taken");
        Box::pin(async move { action(db).await.map_err(|error| error.to_string()) })
    }
}

#[cfg(feature = "sqlite")]
pub struct MutationContext<'tx> {
    db: &'tx crate::db::Database,
    tx: sqlx::Transaction<'tx, sqlx::Sqlite>,
    deferred_events: Vec<Box<dyn DeferredEventEmitter>>,
    deferred_actions: Vec<Box<dyn PostCommitActionRunner>>,
}

#[cfg(feature = "postgres")]
pub struct MutationContext<'tx> {
    db: &'tx crate::db::Database,
    tx: sqlx::Transaction<'tx, sqlx::Postgres>,
    deferred_events: Vec<Box<dyn DeferredEventEmitter>>,
    deferred_actions: Vec<Box<dyn PostCommitActionRunner>>,
}

pub struct MutationQuery<'ctx, 'tx, T>
where
    T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
{
    hook_ctx: &'ctx mut MutationContext<'tx>,
    query: EntityQuery<T>,
}

impl<'ctx, 'tx, T> MutationQuery<'ctx, 'tx, T>
where
    T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
{
    fn new(hook_ctx: &'ctx mut MutationContext<'tx>) -> Self {
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

impl<'tx> MutationContext<'tx> {
    #[cfg(feature = "sqlite")]
    pub fn new(db: &'tx crate::db::Database, tx: sqlx::Transaction<'tx, sqlx::Sqlite>) -> Self {
        Self {
            db,
            tx,
            deferred_events: Vec::new(),
            deferred_actions: Vec::new(),
        }
    }

    #[cfg(feature = "postgres")]
    pub fn new(db: &'tx crate::db::Database, tx: sqlx::Transaction<'tx, sqlx::Postgres>) -> Self {
        Self {
            db,
            tx,
            deferred_events: Vec::new(),
            deferred_actions: Vec::new(),
        }
    }

    pub fn database(&self) -> &crate::db::Database {
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

    #[cfg(feature = "sqlite")]
    pub fn executor(&mut self) -> &mut sqlx::SqliteConnection {
        self.tx.as_mut()
    }

    #[cfg(feature = "postgres")]
    pub fn executor(&mut self) -> &mut sqlx::PgConnection {
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
        F: FnOnce(crate::db::Database) -> Fut + Send + 'static,
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
    ) -> async_graphql::Result<()> {
        if let Some(hook) = self.db.mutation_hook() {
            hook.on_mutation(ctx, self, event).await?;
        }

        Ok(())
    }

    pub async fn insert<'a, T>(
        &'a mut self,
        input: <T as MutationContextInsert>::CreateInput,
    ) -> Result<T, sqlx::Error>
    where
        T: MutationContextInsert,
    {
        T::insert_in_mutation_context(self, input).await
    }

    pub async fn update_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextUpdateById>::Id,
        input: <T as MutationContextUpdateById>::UpdateInput,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: MutationContextUpdateById,
    {
        T::update_by_id_in_mutation_context(self, id, input).await
    }

    pub async fn update_where<'a, T>(
        &'a mut self,
        where_input: <T as MutationContextUpdateWhere>::WhereInput,
        input: <T as MutationContextUpdateWhere>::UpdateInput,
    ) -> Result<i64, sqlx::Error>
    where
        T: MutationContextUpdateWhere,
    {
        T::update_where_in_mutation_context(self, where_input, input).await
    }

    pub async fn delete_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextDeleteById>::Id,
    ) -> Result<bool, sqlx::Error>
    where
        T: MutationContextDeleteById,
    {
        T::delete_by_id_in_mutation_context(self, id).await
    }

    pub async fn delete_where<'a, T>(
        &'a mut self,
        where_input: <T as MutationContextDeleteWhere>::WhereInput,
    ) -> Result<i64, sqlx::Error>
    where
        T: MutationContextDeleteWhere,
    {
        T::delete_where_in_mutation_context(self, where_input).await
    }

    pub fn query<'a, T>(&'a mut self) -> MutationQuery<'a, 'tx, T>
    where
        T: DatabaseEntity + FromSqlRow + Clone + Send + Sync,
    {
        MutationQuery::new(self)
    }

    pub async fn find_by_id<'a, T>(
        &'a mut self,
        id: &'a <T as MutationContextFindById>::Id,
    ) -> Result<Option<T>, sqlx::Error>
    where
        T: MutationContextFindById,
    {
        T::find_by_id_in_mutation_context(self, id).await
    }
}

pub trait MutationHook: Send + Sync {
    fn on_mutation<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
}

pub trait MutationContextInsert: Sized {
    type CreateInput;

    fn insert_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_>,
        input: Self::CreateInput,
    ) -> futures::future::BoxFuture<'a, Result<Self, sqlx::Error>>;
}

pub trait MutationContextUpdateById: Sized {
    type Id;
    type UpdateInput;

    fn update_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_>,
        id: &'a Self::Id,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, Result<Option<Self>, sqlx::Error>>;
}

pub trait MutationContextUpdateWhere: Sized {
    type WhereInput;
    type UpdateInput;

    fn update_where_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_>,
        where_input: Self::WhereInput,
        input: Self::UpdateInput,
    ) -> futures::future::BoxFuture<'a, Result<i64, sqlx::Error>>;
}

pub trait MutationContextDeleteById: Sized {
    type Id;

    fn delete_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_>,
        id: &'a Self::Id,
    ) -> futures::future::BoxFuture<'a, Result<bool, sqlx::Error>>;
}

pub trait MutationContextDeleteWhere: Sized {
    type WhereInput;

    fn delete_where_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_>,
        where_input: Self::WhereInput,
    ) -> futures::future::BoxFuture<'a, Result<i64, sqlx::Error>>;
}

pub trait MutationContextFindById: Sized {
    type Id;

    fn find_by_id_in_mutation_context<'a>(
        hook_ctx: &'a mut MutationContext<'_>,
        id: &'a Self::Id,
    ) -> futures::future::BoxFuture<'a, Result<Option<Self>, sqlx::Error>>;
}

pub trait PostCommitErrorHandler: Send + Sync {
    fn on_post_commit_error<'a>(
        &'a self,
        db: &'a crate::db::Database,
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

pub trait EntityPolicy: Send + Sync {
    fn can_access_entity<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}

pub trait FieldPolicy: Send + Sync {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    fn can_write_field<'a>(
        &'a self,
        ctx: &'a async_graphql::Context<'_>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&'a (dyn std::any::Any + Send + Sync)>,
        value: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}

pub trait RowPolicy: Send + Sync {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;

    fn can_write_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<bool>>;
}

pub trait WriteInputTransform: Send + Sync {
    fn before_create<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;

    fn before_update<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        db: &'a crate::db::Database,
        entity_name: &'static str,
        existing_row: Option<&'a (dyn std::any::Any + Send + Sync)>,
        input: &'a mut (dyn std::any::Any + Send + Sync),
    ) -> futures::future::BoxFuture<'a, async_graphql::Result<()>>;
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
    pub sql_type: &'static str,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
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
            sql_type,
            nullable: true,
            is_primary_key: false,
            is_unique: false,
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

    pub const fn default(mut self, default: &'static str) -> Self {
        self.default = Some(default);
        self
    }

    pub const fn references(mut self, references: &'static str) -> Self {
        self.references = Some(references);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldMetadata {
    pub name: &'static str,
    pub sql_type: &'static str,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub default: Option<&'static str>,
    pub references: Option<&'static str>,
}

impl From<&ColumnDef> for FieldMetadata {
    fn from(value: &ColumnDef) -> Self {
        Self {
            name: value.name,
            sql_type: value.sql_type,
            nullable: value.nullable,
            is_primary_key: value.is_primary_key,
            is_unique: value.is_unique,
            default: value.default,
            references: value.references,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexDef {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub is_unique: bool,
}

impl IndexDef {
    pub const fn new(name: &'static str, columns: &'static [&'static str]) -> Self {
        Self {
            name,
            columns,
            is_unique: false,
        }
    }

    pub const fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }
}

pub type IndexMetadata = IndexDef;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct RelationMetadata {
    pub field_name: &'static str,
    pub target_type: &'static str,
    pub source_column: &'static str,
    pub target_column: &'static str,
    pub is_multiple: bool,
    pub on_delete: DeletePolicy,
}

#[derive(Clone, Debug)]
pub struct EntityMetadata {
    pub entity_name: &'static str,
    pub table_name: &'static str,
    pub plural_name: &'static str,
    pub primary_key: &'static str,
    pub default_sort: &'static str,
    pub read_policy: Option<&'static str>,
    pub write_policy: Option<&'static str>,
    pub fields: Box<[FieldMetadata]>,
    pub indexes: Box<[IndexMetadata]>,
    pub composite_unique_indexes: Box<[Box<[&'static str]>]>,
    pub relations: Box<[RelationMetadata]>,
}

impl EntityMetadata {
    pub fn from_schema<T>(
        entity_name: &'static str,
        read_policy: Option<&'static str>,
        write_policy: Option<&'static str>,
    ) -> Self
    where
        T: DatabaseEntity + DatabaseSchema + EntityRelations,
    {
        Self {
            entity_name,
            table_name: T::TABLE_NAME,
            plural_name: T::PLURAL_NAME,
            primary_key: T::PRIMARY_KEY,
            default_sort: T::DEFAULT_SORT,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnModel {
    pub name: String,
    pub sql_type: String,
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
pub struct TableModel {
    pub entity_name: String,
    pub table_name: String,
    pub primary_key: String,
    pub default_sort: String,
    pub columns: Vec<ColumnModel>,
    pub indexes: Vec<IndexMetadata>,
    pub composite_unique_indexes: Vec<Vec<String>>,
    pub foreign_keys: Vec<ForeignKeyModel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaModel {
    pub tables: Vec<TableModel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaStage {
    pub version: String,
    pub description: String,
    pub target_schema: SchemaModel,
}

impl SchemaStage {
    pub fn new(
        version: impl Into<String>,
        description: impl Into<String>,
        target_schema: SchemaModel,
    ) -> Self {
        Self {
            version: version.into(),
            description: description.into(),
            target_schema,
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

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedSchemaStage {
    pub version: String,
    pub description: String,
    pub plan: MigrationPlan,
}

impl From<&EntityMetadata> for TableModel {
    fn from(value: &EntityMetadata) -> Self {
        Self {
            entity_name: value.entity_name.to_string(),
            table_name: value.table_name.to_string(),
            primary_key: value.primary_key.to_string(),
            default_sort: value.default_sort.to_string(),
            columns: value
                .fields
                .iter()
                .map(|field| ColumnModel {
                    name: field.name.to_string(),
                    sql_type: field.sql_type.to_string(),
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
                .map(|relation| ForeignKeyModel {
                    source_column: relation.source_column.to_string(),
                    target_table: relation.target_type.to_string(),
                    target_column: relation.target_column.to_string(),
                    is_multiple: relation.is_multiple,
                    on_delete: relation.on_delete.clone(),
                })
                .collect(),
        }
    }
}

impl SchemaModel {
    pub fn from_entities(entities: &[&EntityMetadata]) -> Self {
        let entity_table_names = entities
            .iter()
            .map(|entity| (entity.entity_name, entity.table_name))
            .collect::<std::collections::BTreeMap<_, _>>();

        Self {
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

#[derive(Clone, Debug, PartialEq)]
pub enum MigrationStep {
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
