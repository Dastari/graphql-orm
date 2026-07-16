use crate::graphql::auth::AuthorizationMode;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::orm::{
    DefaultBackend, OrmBackend, PaginationConfig, SchemaManager, SchemaPolicy,
};
#[cfg(any(feature = "sqlite", feature = "postgres"))]
use crate::graphql::orm::{TransactionBackend, WriteBackend};
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

#[cfg(any(feature = "sqlite", feature = "postgres"))]
tokio::task_local! {
    static ORM_TRANSACTION_ACTIVE: bool;
}

/// Backend-neutral pool sizing options for ORM-owned connection helpers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionOptions {
    /// Maximum number of connections in the backend pool.
    ///
    /// When omitted, the backend driver's default is used except for in-memory
    /// SQLite URLs, where graphql-orm uses one connection so schema and data
    /// stay visible for the lifetime of the pool.
    pub max_connections: Option<u32>,
}

impl ConnectionOptions {
    /// Use backend defaults.
    pub const fn new() -> Self {
        Self {
            max_connections: None,
        }
    }

    /// Set a maximum pool size.
    pub const fn max_connections(mut self, max_connections: u32) -> Self {
        self.max_connections = Some(max_connections);
        self
    }
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            max_connections: None,
        }
    }
}

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
    pagination_config: PaginationConfig,
    authorization_mode: AuthorizationMode,
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
            pagination_config: self.pagination_config,
            authorization_mode: self.authorization_mode,
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
        debug.field("pagination_config", &self.pagination_config);
        debug.field("authorization_mode", &self.authorization_mode);
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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
            pagination_config: PaginationConfig::default(),
            authorization_mode: AuthorizationMode::default(),
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

    /// Return pagination defaults and caps used by generated connection resolvers.
    pub fn pagination_config(&self) -> PaginationConfig {
        self.pagination_config
    }

    /// Update pagination defaults and caps in place.
    pub fn set_pagination_config(&mut self, pagination_config: PaginationConfig) {
        self.pagination_config = pagination_config;
    }

    /// Return a copy of this handle with different pagination defaults and caps.
    pub fn with_pagination_config(mut self, pagination_config: PaginationConfig) -> Self {
        self.pagination_config = pagination_config;
        self
    }

    /// Return the authorization policy enforcement mode.
    pub fn authorization_mode(&self) -> AuthorizationMode {
        self.authorization_mode
    }

    /// Update the authorization policy enforcement mode in place.
    pub fn set_authorization_mode(&mut self, authorization_mode: AuthorizationMode) {
        self.authorization_mode = authorization_mode;
    }

    /// Return a copy of this handle with a different authorization mode.
    pub fn with_authorization_mode(mut self, authorization_mode: AuthorizationMode) -> Self {
        self.authorization_mode = authorization_mode;
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
        let mut senders = match self.event_senders.senders.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        senders.insert(TypeId::of::<T>(), Box::new(sender));
    }

    pub fn ensure_event_sender<T>(&self) -> tokio::sync::broadcast::Sender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        if let Some(sender) = self.event_sender::<T>() {
            return sender;
        }

        let mut senders = match self.event_senders.senders.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

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
        let senders = match self.event_senders.senders.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        senders
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
        if surface == crate::graphql::orm::EntityAccessSurface::RetentionMaintenance
            && self.entity_policy.is_none()
        {
            return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                .with_internal(format!(
                    "retention maintenance for entity {entity_name} requires an entity policy provider"
                ))
                .into_graphql_error());
        }

        match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
                if let Some(policy) = &self.entity_policy {
                    policy
                        .can_access_entity(ctx, self, entity_name, policy_key, kind, surface)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.entity_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "entity {entity_name} declares a policy key but no entity policy provider is registered"
                        ))
                        .into_graphql_error());
                }
                if let Some(policy) = &self.entity_policy {
                    policy
                        .can_access_entity(ctx, self, entity_name, policy_key, kind, surface)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
                if surface == crate::graphql::orm::EntityAccessSurface::GraphqlSubscription
                    && self.entity_policy.is_none()
                {
                    return Ok(false);
                }
                let Some(policy) = &self.entity_policy else {
                    return Ok(false);
                };
                policy
                    .can_access_entity(ctx, self, entity_name, policy_key, kind, surface)
                    .await
            }
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
            Err(OrmPublicError::forbidden().into_graphql_error())
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
        match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_read_field(ctx, self, entity_name, field_name, policy_key, record)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.field_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "field {entity_name}.{field_name} declares a policy key but no field policy provider is registered"
                        ))
                        .into_graphql_error());
                }
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_read_field(ctx, self, entity_name, field_name, policy_key, record)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
                if policy_key.is_none() {
                    // Fields without an explicit policy remain readable only when
                    // a field policy provider is present and allows the default
                    // key-less decision. Without a provider, deny sensitive
                    // field exposure under the strictest mode.
                    if self.field_policy.is_none() {
                        return Ok(true);
                    }
                }
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_read_field(ctx, self, entity_name, field_name, policy_key, record)
                        .await
                } else if policy_key.is_some() {
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
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
        match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
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
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.field_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "field {entity_name}.{field_name} declares a write policy key but no field policy provider is registered"
                        ))
                        .into_graphql_error());
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
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
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
                } else if policy_key.is_some() {
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
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
            Err(OrmPublicError::forbidden().into_graphql_error())
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
            Err(OrmPublicError::forbidden().into_graphql_error())
        }
    }

    /// Authorize a field selected through the repository surface.
    ///
    /// # Errors
    ///
    /// Returns a safe forbidden or authorization-misconfiguration error when
    /// the configured repository field policy does not authorize the read.
    pub async fn ensure_repository_readable_field(
        &self,
        access: Option<crate::graphql::auth::AccessContext<'_>>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&(dyn Any + Send + Sync)>,
    ) -> Result<(), OrmPublicError> {
        let allowed = match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_read_repository_field(
                            access,
                            self,
                            entity_name,
                            field_name,
                            policy_key,
                            record,
                        )
                        .await
                        .map_err(|error| OrmPublicError::internal(format!("{error:?}")))?
                } else {
                    true
                }
            }
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.field_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "field {entity_name}.{field_name} declares a repository read policy but no field policy provider is registered"
                        )));
                }
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_read_repository_field(
                            access,
                            self,
                            entity_name,
                            field_name,
                            policy_key,
                            record,
                        )
                        .await
                        .map_err(|error| OrmPublicError::internal(format!("{error:?}")))?
                } else {
                    true
                }
            }
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_read_repository_field(
                            access,
                            self,
                            entity_name,
                            field_name,
                            policy_key,
                            record,
                        )
                        .await
                        .map_err(|error| OrmPublicError::internal(format!("{error:?}")))?
                } else {
                    policy_key.is_none()
                }
            }
        };
        if allowed {
            Ok(())
        } else {
            Err(OrmPublicError::forbidden())
        }
    }

    /// Authorize a field supplied through an ordinary Rust repository input.
    ///
    /// # Errors
    ///
    /// Returns a safe forbidden or authorization-misconfiguration error when
    /// the schema policy or configured repository field policy denies the write.
    pub async fn ensure_repository_writable_field(
        &self,
        access: Option<crate::graphql::auth::AccessContext<'_>>,
        entity_name: &'static str,
        field_name: &'static str,
        policy_key: Option<&'static str>,
        record: Option<&(dyn Any + Send + Sync)>,
        value: Option<&(dyn Any + Send + Sync)>,
    ) -> Result<(), OrmPublicError> {
        if !self.schema_policy.allows_entity_writes() {
            return Err(OrmPublicError::forbidden());
        }
        let allowed = match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_write_repository_field(
                            access,
                            self,
                            entity_name,
                            field_name,
                            policy_key,
                            record,
                            value,
                        )
                        .await
                        .map_err(|error| OrmPublicError::internal(format!("{error:?}")))?
                } else {
                    true
                }
            }
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.field_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "field {entity_name}.{field_name} declares a repository write policy but no field policy provider is registered"
                        )));
                }
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_write_repository_field(
                            access,
                            self,
                            entity_name,
                            field_name,
                            policy_key,
                            record,
                            value,
                        )
                        .await
                        .map_err(|error| OrmPublicError::internal(format!("{error:?}")))?
                } else {
                    true
                }
            }
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
                if let Some(policy) = &self.field_policy {
                    policy
                        .can_write_repository_field(
                            access,
                            self,
                            entity_name,
                            field_name,
                            policy_key,
                            record,
                            value,
                        )
                        .await
                        .map_err(|error| OrmPublicError::internal(format!("{error:?}")))?
                } else {
                    policy_key.is_none()
                }
            }
        };
        if allowed {
            Ok(())
        } else {
            Err(OrmPublicError::forbidden())
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
        match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
                if let Some(policy) = &self.row_policy {
                    policy
                        .can_read_row(ctx, self, entity_name, policy_key, surface, row)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.row_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "entity {entity_name} declares a row policy key but no row policy provider is registered"
                        ))
                        .into_graphql_error());
                }
                if let Some(policy) = &self.row_policy {
                    policy
                        .can_read_row(ctx, self, entity_name, policy_key, surface, row)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
                if let Some(policy) = &self.row_policy {
                    policy
                        .can_read_row(ctx, self, entity_name, policy_key, surface, row)
                        .await
                } else if policy_key.is_some() {
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
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
        match self.authorization_mode {
            AuthorizationMode::LegacyPermissive => {
                if let Some(policy) = &self.row_policy {
                    policy
                        .can_write_row(ctx, self, entity_name, policy_key, surface, row)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::DeclaredPoliciesRequired => {
                if policy_key.is_some() && self.row_policy.is_none() {
                    return Err(OrmPublicError::new(OrmErrorCode::AuthorizationMisconfigured)
                        .with_internal(format!(
                            "entity {entity_name} declares a row write policy key but no row policy provider is registered"
                        ))
                        .into_graphql_error());
                }
                if let Some(policy) = &self.row_policy {
                    policy
                        .can_write_row(ctx, self, entity_name, policy_key, surface, row)
                        .await
                } else {
                    Ok(true)
                }
            }
            AuthorizationMode::ExplicitPolicyForAllExposedOperations => {
                if let Some(policy) = &self.row_policy {
                    policy
                        .can_write_row(ctx, self, entity_name, policy_key, surface, row)
                        .await
                } else if policy_key.is_some() {
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
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
            Err(OrmPublicError::forbidden().into_graphql_error())
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

#[cfg(any(feature = "sqlite", feature = "postgres"))]
impl<B> Database<B>
where
    B: TransactionBackend,
{
    /// Run host-only append-only retention maintenance atomically.
    ///
    /// This runner always uses
    /// [`TransactionMode::StateMachine`](crate::graphql::orm::TransactionMode::StateMachine)
    /// so bounded
    /// selection and deletion share a deterministic write snapshot. Only
    /// entities explicitly declaring a retention policy implement the
    /// generated purge capability.
    ///
    /// # Errors
    ///
    /// Returns a safe [`TransactionError`](crate::graphql::orm::TransactionError)
    /// when the transaction cannot start or commit, when the callback rejects
    /// the work, or when transaction-local retention cleanup fails. Nested ORM
    /// transactions are rejected before a second transaction is opened.
    pub async fn retention_transaction<T, F>(
        &self,
        callback: F,
    ) -> Result<T, crate::graphql::orm::TransactionError>
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
        F: for<'tx> FnOnce(
            &'tx mut crate::graphql::orm::RetentionContext<'_, B>,
        ) -> futures::future::BoxFuture<
            'tx,
            Result<T, crate::graphql::errors::OrmPublicError>,
        >,
    {
        self.retention_transaction_with_auth(None, callback).await
    }

    /// Run host-only append-only retention maintenance with transaction-local
    /// database authorization/RLS context.
    ///
    /// # Errors
    ///
    /// Returns a safe [`TransactionError`](crate::graphql::orm::TransactionError)
    /// when the transaction or auth context cannot be established, the
    /// callback rejects the work, retention cleanup fails, or commit fails.
    /// Nested ORM transactions are rejected before a second transaction is
    /// opened.
    pub async fn retention_transaction_with_auth<T, F>(
        &self,
        auth: Option<&crate::graphql::orm::DbAuthContext>,
        callback: F,
    ) -> Result<T, crate::graphql::orm::TransactionError>
    where
        for<'c> &'c mut <B::Database as sqlx::Database>::Connection:
            sqlx::Executor<'c, Database = B::Database> + Send,
        F: for<'tx> FnOnce(
            &'tx mut crate::graphql::orm::RetentionContext<'_, B>,
        ) -> futures::future::BoxFuture<
            'tx,
            Result<T, crate::graphql::errors::OrmPublicError>,
        >,
    {
        if ORM_TRANSACTION_ACTIVE
            .try_with(|active| *active)
            .unwrap_or(false)
        {
            return Err(crate::graphql::orm::TransactionError::Rejected(
                OrmPublicError::with_message(
                    OrmErrorCode::Conflict,
                    "nested ORM transactions are not supported",
                ),
            ));
        }

        ORM_TRANSACTION_ACTIVE
            .scope(true, async move {
                let mut tx = B::begin_orm_transaction(
                    self.pool(),
                    crate::graphql::orm::TransactionMode::StateMachine,
                )
                .await
                .map_err(classify_transaction_error::<B>)?;
                B::apply_auth_context_to_transaction(&mut tx, auth)
                    .await
                    .map_err(classify_transaction_error::<B>)?;
                B::clear_retention_context(&mut tx)
                    .await
                    .map_err(classify_transaction_error::<B>)?;
                let mutation = crate::graphql::orm::MutationContext::new(self, tx);
                let mut context = crate::graphql::orm::RetentionContext::new(mutation);
                let value = callback(&mut context).await.map_err(|error| {
                    if error.is_retryable() {
                        crate::graphql::orm::TransactionError::Retryable(error)
                    } else {
                        crate::graphql::orm::TransactionError::Rejected(error)
                    }
                })?;
                context
                    .commit_and_emit()
                    .await
                    .map_err(classify_transaction_error::<B>)?;
                Ok(value)
            })
            .await
    }

    /// Run ORM reads and mutations atomically without exposing a driver transaction.
    ///
    /// The callback must be boxed because its future borrows the transaction-bound
    /// [`MutationContext`](crate::graphql::orm::MutationContext). On callback error it is rolled back; on cancellation the
    /// transaction guard rolls back when dropped. Deferred events and actions run
    /// only after a successful commit.
    pub async fn transaction<T, F>(
        &self,
        mode: crate::graphql::orm::TransactionMode,
        callback: F,
    ) -> Result<T, crate::graphql::orm::TransactionError>
    where
        F: for<'tx> FnOnce(
            &'tx mut crate::graphql::orm::MutationContext<'_, B>,
        ) -> futures::future::BoxFuture<
            'tx,
            Result<T, crate::graphql::errors::OrmPublicError>,
        >,
    {
        self.transaction_with_auth(mode, None, callback).await
    }

    /// Run an ORM transaction with backend-neutral transaction-local auth state.
    ///
    /// PostgreSQL installs `auth` with transaction-local settings before the
    /// callback runs, preserving generated RLS behavior without exposing a raw
    /// connection. SQLite accepts the same portable API and applies no driver
    /// settings. Calling either transaction runner from inside another runner
    /// on the same Tokio task is rejected; independent nested transactions are
    /// never opened implicitly.
    pub async fn transaction_with_auth<T, F>(
        &self,
        mode: crate::graphql::orm::TransactionMode,
        auth: Option<&crate::graphql::orm::DbAuthContext>,
        callback: F,
    ) -> Result<T, crate::graphql::orm::TransactionError>
    where
        F: for<'tx> FnOnce(
            &'tx mut crate::graphql::orm::MutationContext<'_, B>,
        ) -> futures::future::BoxFuture<
            'tx,
            Result<T, crate::graphql::errors::OrmPublicError>,
        >,
    {
        if ORM_TRANSACTION_ACTIVE
            .try_with(|active| *active)
            .unwrap_or(false)
        {
            return Err(crate::graphql::orm::TransactionError::Rejected(
                OrmPublicError::with_message(
                    OrmErrorCode::Conflict,
                    "nested ORM transactions are not supported",
                ),
            ));
        }

        ORM_TRANSACTION_ACTIVE
            .scope(true, async move {
                let mut tx = B::begin_orm_transaction(self.pool(), mode)
                    .await
                    .map_err(classify_transaction_error::<B>)?;
                B::apply_auth_context_to_transaction(&mut tx, auth)
                    .await
                    .map_err(classify_transaction_error::<B>)?;
                let mut context = crate::graphql::orm::MutationContext::new(self, tx);
                let value = callback(&mut context).await.map_err(|error| {
                    if error.is_retryable() {
                        crate::graphql::orm::TransactionError::Retryable(error)
                    } else {
                        crate::graphql::orm::TransactionError::Rejected(error)
                    }
                })?;
                context
                    .commit_and_emit()
                    .await
                    .map_err(classify_transaction_error::<B>)?;
                Ok(value)
            })
            .await
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
fn classify_transaction_error<B: TransactionBackend>(
    error: sqlx::Error,
) -> crate::graphql::orm::TransactionError {
    use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
    let internal = error.to_string();
    if B::is_retryable_transaction_error(&error) {
        crate::graphql::orm::TransactionError::Retryable(
            OrmPublicError::new(OrmErrorCode::ServiceUnavailable).with_internal(internal),
        )
    } else {
        crate::graphql::orm::TransactionError::Failed(
            OrmPublicError::new(OrmErrorCode::InternalError).with_internal(internal),
        )
    }
}

#[cfg(feature = "sqlite")]
impl Database<crate::graphql::orm::SqliteBackend> {
    pub fn new(pool: <crate::graphql::orm::SqliteBackend as OrmBackend>::Pool) -> Self {
        Self::base(pool, Self::default_schema_policy())
    }

    /// Open a SQLite database and wrap it in a [`Database`] handle.
    ///
    /// This is the normal app-facing constructor when callers do not need raw
    /// SQLX pool customization. For `sqlite::memory:` and `mode=memory` URLs,
    /// the helper uses one pooled connection so temporary schema/data are not
    /// split across independent in-memory databases.
    pub async fn connect_sqlite(database_url: impl AsRef<str>) -> crate::Result<Self> {
        Self::connect_sqlite_with_options(database_url, ConnectionOptions::default()).await
    }

    /// Open a SQLite database with ORM-owned connection options.
    pub async fn connect_sqlite_with_options(
        database_url: impl AsRef<str>,
        options: ConnectionOptions,
    ) -> crate::Result<Self> {
        let database_url = database_url.as_ref();
        let max_connections = options.max_connections.or_else(|| {
            if database_url == "sqlite::memory:" || database_url.contains("mode=memory") {
                Some(1)
            } else {
                None
            }
        });
        let mut pool_options = sqlx::sqlite::SqlitePoolOptions::new();
        if let Some(max_connections) = max_connections {
            pool_options = pool_options.max_connections(max_connections);
        }
        let pool = pool_options.connect(database_url).await?;
        Ok(Self::new(pool))
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

    /// Open a PostgreSQL database and wrap it in a [`Database`] handle.
    ///
    /// This is the normal app-facing constructor when callers do not need raw
    /// SQLX pool customization.
    pub async fn connect_postgres(database_url: impl AsRef<str>) -> crate::Result<Self> {
        Self::connect_postgres_with_options(database_url, ConnectionOptions::default()).await
    }

    /// Open a PostgreSQL database with ORM-owned connection options.
    pub async fn connect_postgres_with_options(
        database_url: impl AsRef<str>,
        options: ConnectionOptions,
    ) -> crate::Result<Self> {
        let mut pool_options = sqlx::postgres::PgPoolOptions::new();
        if let Some(max_connections) = options.max_connections {
            pool_options = pool_options.max_connections(max_connections);
        }
        let pool = pool_options.connect(database_url.as_ref()).await?;
        Ok(Self::new(pool))
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

    /// Open a read-only SQL Server connection pool and wrap it in a [`Database`] handle.
    pub async fn connect_ado(connection_string: &str) -> crate::Result<Self> {
        let pool = crate::db::mssql::MssqlPool::connect_ado(connection_string).await?;
        Ok(Self::new(pool))
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

    /// Set pagination defaults and caps for generated connection resolvers.
    pub fn pagination_config(mut self, pagination_config: PaginationConfig) -> Self {
        self.database.pagination_config = pagination_config;
        self
    }

    /// Set the default connection limit used when GraphQL `page.limit` is omitted.
    ///
    /// Pass `None` to preserve unbounded omitted-limit behavior.
    pub fn default_page_limit(mut self, limit: Option<i64>) -> Self {
        self.database.pagination_config.default_limit = limit;
        self
    }

    /// Set the maximum accepted explicit or default connection limit.
    ///
    /// Pass `None` to disable limit capping.
    pub fn max_page_limit(mut self, limit: Option<i64>) -> Self {
        self.database.pagination_config.max_limit = limit;
        self
    }

    /// Disable both the default connection limit and max-limit cap.
    pub fn unbounded_pagination(mut self) -> Self {
        self.database.pagination_config = PaginationConfig::unbounded();
        self
    }

    /// Set the authorization policy enforcement mode.
    pub fn authorization_mode(mut self, authorization_mode: AuthorizationMode) -> Self {
        self.database.authorization_mode = authorization_mode;
        self
    }

    /// Build the configured [`Database`] handle.
    pub fn build(self) -> Database<B> {
        self.database
    }
}

#[cfg(feature = "sqlite")]
pub mod sqlite_helpers {
    pub fn json_from_str<T>(value: &str) -> crate::Result<T>
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
    pub fn json_from_value<T>(value: serde_json::Value) -> crate::Result<T>
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
    pub fn json_from_str<T>(value: &str) -> crate::Result<T>
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
