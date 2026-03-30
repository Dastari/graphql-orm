use crate::DbPool;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Default)]
struct EventSenders {
    senders: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct Database {
    pool: DbPool,
    mutation_hook: Option<Arc<dyn crate::graphql::orm::MutationHook>>,
    entity_policy: Option<Arc<dyn crate::graphql::orm::EntityPolicy>>,
    field_policy: Option<Arc<dyn crate::graphql::orm::FieldPolicy>>,
    row_policy: Option<Arc<dyn crate::graphql::orm::RowPolicy>>,
    write_input_transform: Option<Arc<dyn crate::graphql::orm::WriteInputTransform>>,
    post_commit_error_handler: Option<Arc<dyn crate::graphql::orm::PostCommitErrorHandler>>,
    event_senders: Arc<EventSenders>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("pool", &"DbPool")
            .field("has_mutation_hook", &self.mutation_hook.is_some())
            .field("has_field_policy", &self.field_policy.is_some())
            .finish()
    }
}

impl Database {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: None,
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_mutation_hook<H>(pool: DbPool, hook: H) -> Self
    where
        H: crate::graphql::orm::MutationHook + 'static,
    {
        Self {
            pool,
            mutation_hook: Some(Arc::new(hook)),
            entity_policy: None,
            field_policy: None,
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_entity_policy<H>(pool: DbPool, policy: H) -> Self
    where
        H: crate::graphql::orm::EntityPolicy + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: Some(Arc::new(policy)),
            field_policy: None,
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_field_policy<H>(pool: DbPool, policy: H) -> Self
    where
        H: crate::graphql::orm::FieldPolicy + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: Some(Arc::new(policy)),
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_row_policy<H>(pool: DbPool, policy: H) -> Self
    where
        H: crate::graphql::orm::RowPolicy + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: None,
            row_policy: Some(Arc::new(policy)),
            write_input_transform: None,
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_write_input_transform<H>(pool: DbPool, transform: H) -> Self
    where
        H: crate::graphql::orm::WriteInputTransform + 'static,
    {
        Self {
            pool,
            mutation_hook: None,
            entity_policy: None,
            field_policy: None,
            row_policy: None,
            write_input_transform: Some(Arc::new(transform)),
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_hooks<M, E, F>(
        pool: DbPool,
        mutation_hook: M,
        entity_policy: E,
        field_policy: F,
    ) -> Self
    where
        M: crate::graphql::orm::MutationHook + 'static,
        E: crate::graphql::orm::EntityPolicy + 'static,
        F: crate::graphql::orm::FieldPolicy + 'static,
    {
        Self {
            pool,
            mutation_hook: Some(Arc::new(mutation_hook)),
            entity_policy: Some(Arc::new(entity_policy)),
            field_policy: Some(Arc::new(field_policy)),
            row_policy: None,
            write_input_transform: None,
            post_commit_error_handler: None,
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    pub fn set_mutation_hook<H>(&mut self, hook: H)
    where
        H: crate::graphql::orm::MutationHook + 'static,
    {
        self.mutation_hook = Some(Arc::new(hook));
    }

    pub fn mutation_hook(&self) -> Option<&Arc<dyn crate::graphql::orm::MutationHook>> {
        self.mutation_hook.as_ref()
    }

    pub fn set_entity_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::EntityPolicy + 'static,
    {
        self.entity_policy = Some(Arc::new(policy));
    }

    pub fn entity_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::EntityPolicy>> {
        self.entity_policy.as_ref()
    }

    pub fn set_field_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::FieldPolicy + 'static,
    {
        self.field_policy = Some(Arc::new(policy));
    }

    pub fn field_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::FieldPolicy>> {
        self.field_policy.as_ref()
    }

    pub fn set_row_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::RowPolicy + 'static,
    {
        self.row_policy = Some(Arc::new(policy));
    }

    pub fn row_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::RowPolicy>> {
        self.row_policy.as_ref()
    }

    pub fn set_write_input_transform<H>(&mut self, transform: H)
    where
        H: crate::graphql::orm::WriteInputTransform + 'static,
    {
        self.write_input_transform = Some(Arc::new(transform));
    }

    pub fn write_input_transform(
        &self,
    ) -> Option<&Arc<dyn crate::graphql::orm::WriteInputTransform>> {
        self.write_input_transform.as_ref()
    }

    pub fn set_post_commit_error_handler<H>(&mut self, handler: H)
    where
        H: crate::graphql::orm::PostCommitErrorHandler + 'static,
    {
        self.post_commit_error_handler = Some(Arc::new(handler));
    }

    pub fn post_commit_error_handler(
        &self,
    ) -> Option<&Arc<dyn crate::graphql::orm::PostCommitErrorHandler>> {
        self.post_commit_error_handler.as_ref()
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

    pub async fn report_post_commit_error(&self, error: String) {
        if let Some(handler) = &self.post_commit_error_handler {
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

    pub async fn run_before_create(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()> {
        if let Some(transform) = &self.write_input_transform {
            transform.before_create(ctx, self, entity_name, input).await
        } else {
            Ok(())
        }
    }

    pub async fn run_before_update(
        &self,
        ctx: Option<&async_graphql::Context<'_>>,
        entity_name: &'static str,
        existing_row: Option<&(dyn Any + Send + Sync)>,
        input: &mut (dyn Any + Send + Sync),
    ) -> async_graphql::Result<()> {
        if let Some(transform) = &self.write_input_transform {
            transform
                .before_update(ctx, self, entity_name, existing_row, input)
                .await
        } else {
            Ok(())
        }
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
