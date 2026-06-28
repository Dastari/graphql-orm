#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::graphql::orm::WriteBackend;
use crate::graphql::orm::{DefaultBackend, OrmBackend, SchemaManager, SchemaPolicy};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

#[cfg(feature = "mssql")]
pub mod mssql;

#[derive(Default)]
struct EventSenders {
    senders: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 256;

/// Runtime database handle used by generated resolvers and repository helpers.
///
/// `Database` stores the backend pool plus optional runtime policies, hooks,
/// change-journal state, and schema ownership policy. Cloning a `Database`
/// clones the pool handle and shared runtime state; it does not open a new
/// database connection by itself.
pub struct Database<B: OrmBackend = DefaultBackend> {
    pool: B::Pool,
    mutation_hook: Option<Arc<dyn Any + Send + Sync>>,
    entity_policy: Option<Arc<dyn crate::graphql::orm::EntityPolicy<B>>>,
    field_policy: Option<Arc<dyn crate::graphql::orm::FieldPolicy<B>>>,
    row_policy: Option<Arc<dyn crate::graphql::orm::RowPolicy<B>>>,
    write_input_transform: Option<Arc<dyn Any + Send + Sync>>,
    post_commit_error_handler: Option<Arc<dyn Any + Send + Sync>>,
    change_journal_enabled: bool,
    schema_policy: SchemaPolicy,
    event_senders: Arc<EventSenders>,
    _backend: PhantomData<B>,
}

impl<B: OrmBackend> Clone for Database<B> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            mutation_hook: self.mutation_hook.clone(),
            entity_policy: self.entity_policy.clone(),
            field_policy: self.field_policy.clone(),
            row_policy: self.row_policy.clone(),
            write_input_transform: self.write_input_transform.clone(),
            post_commit_error_handler: self.post_commit_error_handler.clone(),
            change_journal_enabled: self.change_journal_enabled,
            schema_policy: self.schema_policy,
            event_senders: self.event_senders.clone(),
            _backend: PhantomData,
        }
    }
}

impl<B: OrmBackend> std::fmt::Debug for Database<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("Database");
        debug.field("pool", &"DbPool");
        debug.field("schema_policy", &self.schema_policy);
        debug.field("has_mutation_hook", &self.mutation_hook.is_some());
        debug
            .field("has_field_policy", &self.field_policy.is_some())
            .finish()
    }
}

impl<B: OrmBackend> Database<B> {
    fn default_schema_policy() -> SchemaPolicy {
        if B::READ_ONLY {
            SchemaPolicy::ExternalReadOnly
        } else {
            SchemaPolicy::Managed
        }
    }

    fn base(pool: B::Pool, schema_policy: SchemaPolicy) -> Self {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: None,
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy,
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn with_mutation_hook<H>(pool: B::Pool, hook: H) -> Self
    where
        B: WriteBackend,
        H: crate::graphql::orm::MutationHook<B> + 'static,
    {
        let hook: Arc<dyn crate::graphql::orm::MutationHook<B>> = Arc::new(hook);
        Self {
            pool,
            mutation_hook: Some(Arc::new(hook)),
            entity_policy: None,
            field_policy: None,
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy: Self::default_schema_policy(),
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    pub fn with_entity_policy<H>(pool: B::Pool, policy: H) -> Self
    where
        H: crate::graphql::orm::EntityPolicy<B> + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: Some(Arc::new(policy)),
            field_policy: None,
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy: Self::default_schema_policy(),
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    pub fn with_field_policy<H>(pool: B::Pool, policy: H) -> Self
    where
        H: crate::graphql::orm::FieldPolicy<B> + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: Some(Arc::new(policy)),
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy: Self::default_schema_policy(),
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    pub fn with_row_policy<H>(pool: B::Pool, policy: H) -> Self
    where
        H: crate::graphql::orm::RowPolicy<B> + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: None,
            row_policy: Some(Arc::new(policy)),
            write_input_transform: None,
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy: Self::default_schema_policy(),
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn with_write_input_transform<H>(pool: B::Pool, transform: H) -> Self
    where
        B: WriteBackend,
        H: crate::graphql::orm::WriteInputTransform<B> + 'static,
    {
        let transform: Arc<dyn crate::graphql::orm::WriteInputTransform<B>> = Arc::new(transform);
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: None,
            row_policy: None,
            write_input_transform: Some(Arc::new(transform)),
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy: Self::default_schema_policy(),
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn with_hooks<M, E, F>(
        pool: B::Pool,
        mutation_hook: M,
        entity_policy: E,
        field_policy: F,
    ) -> Self
    where
        B: WriteBackend,
        M: crate::graphql::orm::MutationHook<B> + 'static,
        E: crate::graphql::orm::EntityPolicy<B> + 'static,
        F: crate::graphql::orm::FieldPolicy<B> + 'static,
    {
        let mutation_hook: Arc<dyn crate::graphql::orm::MutationHook<B>> = Arc::new(mutation_hook);
        Self {
            pool,
            mutation_hook: Some(Arc::new(mutation_hook)),
            entity_policy: Some(Arc::new(entity_policy)),
            field_policy: Some(Arc::new(field_policy)),
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            change_journal_enabled: false,
            schema_policy: Self::default_schema_policy(),
            event_senders: Arc::new(EventSenders::default()),
            _backend: PhantomData,
        }
    }

    /// Return the backend pool used by generated code.
    pub fn pool(&self) -> &B::Pool {
        &self.pool
    }

    /// Return the schema ownership policy attached to this database handle.
    pub fn schema_policy(&self) -> SchemaPolicy {
        self.schema_policy
    }

    /// Update the schema ownership policy in place.
    pub fn set_schema_policy(&mut self, schema_policy: SchemaPolicy) {
        self.schema_policy = schema_policy;
    }

    /// Return a copy of this handle with a different schema ownership policy.
    pub fn with_schema_policy(mut self, schema_policy: SchemaPolicy) -> Self {
        self.schema_policy = schema_policy;
        self
    }

    /// Create a schema manager for explicit validation, planning, and migration application.
    pub fn schema(&self) -> SchemaManager<'_, B> {
        SchemaManager::new(self)
    }

    pub fn with_change_journal(mut self) -> Self {
        self.change_journal_enabled = true;
        self
    }

    pub fn set_change_journal_enabled(&mut self, enabled: bool) {
        self.change_journal_enabled = enabled;
    }

    pub fn change_journal_enabled(&self) -> bool {
        self.change_journal_enabled
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn set_mutation_hook<H>(&mut self, hook: H)
    where
        B: WriteBackend,
        H: crate::graphql::orm::MutationHook<B> + 'static,
    {
        let hook: Arc<dyn crate::graphql::orm::MutationHook<B>> = Arc::new(hook);
        self.mutation_hook = Some(Arc::new(hook));
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn mutation_hook(&self) -> Option<&Arc<dyn crate::graphql::orm::MutationHook<B>>>
    where
        B: WriteBackend,
    {
        self.mutation_hook
            .as_ref()
            .and_then(|hook| hook.downcast_ref::<Arc<dyn crate::graphql::orm::MutationHook<B>>>())
    }

    pub fn set_entity_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::EntityPolicy<B> + 'static,
    {
        self.entity_policy = Some(Arc::new(policy));
    }

    pub fn entity_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::EntityPolicy<B>>> {
        self.entity_policy.as_ref()
    }

    pub fn set_field_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::FieldPolicy<B> + 'static,
    {
        self.field_policy = Some(Arc::new(policy));
    }

    pub fn field_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::FieldPolicy<B>>> {
        self.field_policy.as_ref()
    }

    pub fn set_row_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::RowPolicy<B> + 'static,
    {
        self.row_policy = Some(Arc::new(policy));
    }

    pub fn row_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::RowPolicy<B>>> {
        self.row_policy.as_ref()
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn set_write_input_transform<H>(&mut self, transform: H)
    where
        B: WriteBackend,
        H: crate::graphql::orm::WriteInputTransform<B> + 'static,
    {
        let transform: Arc<dyn crate::graphql::orm::WriteInputTransform<B>> = Arc::new(transform);
        self.write_input_transform = Some(Arc::new(transform));
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn write_input_transform(
        &self,
    ) -> Option<&Arc<dyn crate::graphql::orm::WriteInputTransform<B>>>
    where
        B: WriteBackend,
    {
        self.write_input_transform.as_ref().and_then(|transform| {
            transform.downcast_ref::<Arc<dyn crate::graphql::orm::WriteInputTransform<B>>>()
        })
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn set_post_commit_error_handler<H>(&mut self, handler: H)
    where
        B: WriteBackend,
        H: crate::graphql::orm::PostCommitErrorHandler<B> + 'static,
    {
        let handler: Arc<dyn crate::graphql::orm::PostCommitErrorHandler<B>> = Arc::new(handler);
        self.post_commit_error_handler = Some(Arc::new(handler));
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub fn post_commit_error_handler(
        &self,
    ) -> Option<&Arc<dyn crate::graphql::orm::PostCommitErrorHandler<B>>>
    where
        B: WriteBackend,
    {
        self.post_commit_error_handler.as_ref().and_then(|handler| {
            handler.downcast_ref::<Arc<dyn crate::graphql::orm::PostCommitErrorHandler<B>>>()
        })
    }

    pub fn register_event_sender<T>(&self, sender: tokio::sync::broadcast::Sender<T>)
    where
        T: Clone + Send + Sync + 'static,
    {
        self.event_senders
            .senders
            .write()
            .expect("event sender lock poisoned")
            .insert(TypeId::of::<T>(), Box::new(sender));
    }

    pub fn ensure_event_sender<T>(&self) -> tokio::sync::broadcast::Sender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        if let Some(sender) = self.event_sender::<T>() {
            return sender;
        }

        let mut senders = self
            .event_senders
            .senders
            .write()
            .expect("event sender lock poisoned");

        if let Some(sender) = senders
            .get(&TypeId::of::<T>())
            .and_then(|sender| sender.downcast_ref::<tokio::sync::broadcast::Sender<T>>())
        {
            return sender.clone();
        }

        let (sender, _) = tokio::sync::broadcast::channel(DEFAULT_EVENT_CHANNEL_CAPACITY);
        senders.insert(TypeId::of::<T>(), Box::new(sender.clone()));
        sender
    }

    pub fn event_sender<T>(&self) -> Option<tokio::sync::broadcast::Sender<T>>
    where
        T: Clone + Send + Sync + 'static,
    {
        self.event_senders
            .senders
            .read()
            .expect("event sender lock poisoned")
            .get(&TypeId::of::<T>())
            .and_then(|sender| sender.downcast_ref::<tokio::sync::broadcast::Sender<T>>())
            .cloned()
    }

    pub fn emit_event<T>(&self, event: T)
    where
        T: Clone + Send + Sync + 'static,
    {
        let sender = self.ensure_event_sender::<T>();
        let _ = sender.send(event);
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn report_post_commit_error(&self, error: String)
    where
        B: WriteBackend,
    {
        if let Some(handler) = self.post_commit_error_handler() {
            handler.on_post_commit_error(self, &error).await;
        } else {
            eprintln!("graphql-orm post-commit action failed: {error}");
        }
    }

    pub async fn can_access_entity(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: crate::graphql::orm::EntityAccessKind,
        surface: crate::graphql::orm::EntityAccessSurface,
    ) -> async_graphql::Result<bool> {
        if kind == crate::graphql::orm::EntityAccessKind::Write
            && !self.schema_policy.allows_entity_writes()
        {
            return Ok(false);
        }
        if let Some(policy) = &self.entity_policy {
            policy
                .can_access_entity(ctx, self, entity_name, policy_key, kind, surface)
                .await
        } else {
            Ok(true)
        }
    }

    pub async fn ensure_entity_access(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: crate::graphql::orm::EntityAccessKind,
        surface: crate::graphql::orm::EntityAccessSurface,
    ) -> async_graphql::Result<()> {
        if self
            .can_access_entity(ctx, entity_name, policy_key, kind, surface)
            .await?
        {
            Ok(())
        } else {
            Err(async_graphql::Error::new(format!(
                "Access denied for {} {:?} on {:?}",
                entity_name, kind, surface
            )))
        }
    }

    pub async fn can_read_field(
        &self,
        ctx: &async_graphql::Context<'_>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&(dyn Any + Send + Sync)>,
    ) -> async_graphql::Result<bool> {
        if let Some(policy) = &self.field_policy {
            policy
                .can_read_field(ctx, self, entity_name, field_name, policy_key, record)
                .await
        } else {
            Ok(true)
        }
    }

    pub async fn can_write_field(
        &self,
        ctx: &async_graphql::Context<'_>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&(dyn Any + Send + Sync)>,
        value: Option<&(dyn Any + Send + Sync)>,
    ) -> async_graphql::Result<bool> {
        if !self.schema_policy.allows_entity_writes() {
            return Ok(false);
        }
        if let Some(policy) = &self.field_policy {
            policy
                .can_write_field(
                    ctx,
                    self,
                    entity_name,
                    field_name,
                    policy_key,
                    record,
                    value,
                )
                .await
        } else {
            Ok(true)
        }
    }

    pub async fn ensure_readable_field(
        &self,
        ctx: &async_graphql::Context<'_>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&(dyn Any + Send + Sync)>,
    ) -> async_graphql::Result<()> {
        if self
            .can_read_field(ctx, entity_name, field_name, policy_key, record)
            .await?
        {
            Ok(())
        } else {
            Err(async_graphql::Error::new(format!(
                "Access denied for field {}.{}",
                entity_name, field_name
            )))
        }
    }

    pub async fn ensure_writable_field(
        &self,
        ctx: &async_graphql::Context<'_>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&(dyn Any + Send + Sync)>,
        value: Option<&(dyn Any + Send + Sync)>,
    ) -> async_graphql::Result<()> {
        if self
            .can_write_field(ctx, entity_name, field_name, policy_key, record, value)
            .await?
        {
            Ok(())
        } else {
            Err(async_graphql::Error::new(format!(
                "Write denied for field {}.{}",
                entity_name, field_name
            )))
        }
    }

    pub async fn can_read_row(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: crate::graphql::orm::EntityAccessSurface,
        row: &(dyn Any + Send + Sync),
    ) -> async_graphql::Result<bool> {
        if let Some(policy) = &self.row_policy {
            policy
                .can_read_row(ctx, self, entity_name, policy_key, surface, row)
                .await
        } else {
            Ok(true)
        }
    }

    pub async fn can_write_row(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: crate::graphql::orm::EntityAccessSurface,
        row: &(dyn Any + Send + Sync),
    ) -> async_graphql::Result<bool> {
        if !self.schema_policy.allows_entity_writes() {
            return Ok(false);
        }
        if let Some(policy) = &self.row_policy {
            policy
                .can_write_row(ctx, self, entity_name, policy_key, surface, row)
                .await
        } else {
            Ok(true)
        }
    }

    pub async fn ensure_writable_row(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: crate::graphql::orm::EntityAccessSurface,
        row: &(dyn Any + Send + Sync),
    ) -> async_graphql::Result<()> {
        if self
            .can_write_row(ctx, entity_name, policy_key, surface, row)
            .await?
        {
            Ok(())
        } else {
            Err(async_graphql::Error::new(format!(
                "Write denied for row {} on {:?}",
                entity_name, surface
            )))
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn run_before_create(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
    {
        let mut write_ctx = if let Some(ctx) = ctx {
            crate::graphql::orm::WriteInputContext::graphql(self, ctx, entity_name)
        } else {
            crate::graphql::orm::WriteInputContext::repository(self, entity_name)
        };
        self.run_before_create_with_context(&mut write_ctx, input)
            .await
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn run_before_create_with_context(
        &self,
        write_ctx: &mut crate::graphql::orm::WriteInputContext<'_, '_, B>,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
    {
        if let Some(transform) = self.write_input_transform() {
            transform.before_create_with_context(write_ctx, input).await
        } else {
            Ok(())
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn run_before_update(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        existing_row: Option<&(dyn Any + Send + Sync)>,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
    {
        let mut write_ctx = if let Some(ctx) = ctx {
            crate::graphql::orm::WriteInputContext::graphql(self, ctx, entity_name)
        } else {
            crate::graphql::orm::WriteInputContext::repository(self, entity_name)
        };
        self.run_before_update_with_context(&mut write_ctx, existing_row, input)
            .await
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn run_before_update_with_context(
        &self,
        write_ctx: &mut crate::graphql::orm::WriteInputContext<'_, '_, B>,
        existing_row: Option<&(dyn Any + Send + Sync)>,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
    {
        if let Some(transform) = self.write_input_transform() {
            transform
                .before_update_with_context(write_ctx, existing_row, input)
                .await
        } else {
            Ok(())
        }
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn run_before_upsert(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
    {
        let mut write_ctx = if let Some(ctx) = ctx {
            crate::graphql::orm::WriteInputContext::graphql(self, ctx, entity_name)
        } else {
            crate::graphql::orm::WriteInputContext::repository(self, entity_name)
        };
        self.run_before_upsert_with_context(&mut write_ctx, input)
            .await
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub async fn run_before_upsert_with_context(
        &self,
        write_ctx: &mut crate::graphql::orm::WriteInputContext<'_, '_, B>,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()>
    where
        B: WriteBackend,
    {
        if let Some(transform) = self.write_input_transform() {
            transform.before_upsert_with_context(write_ctx, input).await
        } else {
            Ok(())
        }
    }
}

#[cfg(feature = "sqlite")]
impl Database<crate::graphql::orm::SqliteBackend> {
    pub fn new(pool: <crate::graphql::orm::SqliteBackend as OrmBackend>::Pool) -> Self {
        Self::base(pool, Self::default_schema_policy())
    }

    pub fn builder(
        pool: <crate::graphql::orm::SqliteBackend as OrmBackend>::Pool,
    ) -> DatabaseBuilder<crate::graphql::orm::SqliteBackend> {
        DatabaseBuilder {
            database: Self::new(pool),
        }
    }
}

#[cfg(feature = "postgres")]
impl Database<crate::graphql::orm::PostgresBackend> {
    pub fn new(pool: <crate::graphql::orm::PostgresBackend as OrmBackend>::Pool) -> Self {
        Self::base(pool, Self::default_schema_policy())
    }

    pub fn builder(
        pool: <crate::graphql::orm::PostgresBackend as OrmBackend>::Pool,
    ) -> DatabaseBuilder<crate::graphql::orm::PostgresBackend> {
        DatabaseBuilder {
            database: Self::new(pool),
        }
    }
}

#[cfg(feature = "mssql")]
impl Database<crate::graphql::orm::MssqlBackend> {
    pub fn new(pool: <crate::graphql::orm::MssqlBackend as OrmBackend>::Pool) -> Self {
        Self::base(pool, Self::default_schema_policy())
    }

    pub fn builder(
        pool: <crate::graphql::orm::MssqlBackend as OrmBackend>::Pool,
    ) -> DatabaseBuilder<crate::graphql::orm::MssqlBackend> {
        DatabaseBuilder {
            database: Self::new(pool),
        }
    }
}

pub struct DatabaseBuilder<B: OrmBackend = DefaultBackend> {
    database: Database<B>,
}

impl<B: OrmBackend> DatabaseBuilder<B> {
    /// Set the schema ownership policy for the resulting [`Database`].
    pub fn schema_policy(mut self, schema_policy: SchemaPolicy) -> Self {
        self.database.schema_policy = schema_policy;
        self
    }

    /// Enable or disable change-journal recording for generated write paths.
    pub fn change_journal_enabled(mut self, enabled: bool) -> Self {
        self.database.change_journal_enabled = enabled;
        self
    }

    /// Build the configured [`Database`] handle.
    pub fn build(self) -> Database<B> {
        self.database
    }
}

#[cfg(feature = "sqlite")]
pub mod sqlite_helpers {
    pub fn json_from_str<T>(value: &str) -> Result<T, sqlx::Error>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_str(value).map_err(|error| sqlx::Error::Decode(Box::new(error)))
    }

    pub fn uuid_to_string(value: &uuid::Uuid) -> String {
        value.to_string()
    }

    pub fn int_to_bool(value: i32) -> bool {
        value != 0
    }

    pub fn str_to_uuid(value: &str) -> Result<uuid::Uuid, uuid::Error> {
        uuid::Uuid::parse_str(value)
    }

    pub fn str_to_datetime(value: &str) -> Result<String, std::convert::Infallible> {
        Ok(value.to_string())
    }

    pub fn json_to_vec<T>(value: &str) -> Vec<T>
    where
        T: serde::de::DeserializeOwned,
    {
        json_from_str(value).unwrap_or_default()
    }
}

#[cfg(feature = "postgres")]
pub mod postgres_helpers {
    pub fn json_from_value<T>(value: serde_json::Value) -> Result<T, sqlx::Error>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_value(value).map_err(|error| sqlx::Error::Decode(Box::new(error)))
    }

    pub fn uuid_to_string(value: &uuid::Uuid) -> String {
        value.to_string()
    }

    pub fn int_to_bool(value: i32) -> bool {
        value != 0
    }

    pub fn str_to_uuid(value: &str) -> Result<uuid::Uuid, uuid::Error> {
        uuid::Uuid::parse_str(value)
    }

    pub fn str_to_datetime(value: &str) -> Result<String, std::convert::Infallible> {
        Ok(value.to_string())
    }

    pub fn json_to_vec<T>(value: &str) -> Vec<T>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_str(value).unwrap_or_default()
    }
}

#[cfg(feature = "mssql")]
pub mod mssql_helpers {
    pub fn json_from_str<T>(value: &str) -> Result<T, sqlx::Error>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_str(value).map_err(|error| sqlx::Error::Decode(Box::new(error)))
    }

    pub fn uuid_to_string(value: &uuid::Uuid) -> String {
        value.to_string()
    }

    pub fn int_to_bool(value: i32) -> bool {
        value != 0
    }

    pub fn str_to_uuid(value: &str) -> Result<uuid::Uuid, uuid::Error> {
        uuid::Uuid::parse_str(value)
    }

    pub fn str_to_datetime(value: &str) -> Result<String, std::convert::Infallible> {
        Ok(value.to_string())
    }

    pub fn json_to_vec<T>(value: &str) -> Vec<T>
    where
        T: serde::de::DeserializeOwned,
    {
        json_from_str(value).unwrap_or_default()
    }
}
