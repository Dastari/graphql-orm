use crate::DbPool;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Default)]
struct EventSenders {
    senders: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

#[derive(Clone)]
pub struct Database {
    pool: DbPool,
    mutation_hook: Option<Arc<dyn crate::graphql::orm::MutationHook>>,
    field_policy: Option<Arc<dyn crate::graphql::orm::FieldPolicy>>,
    event_senders: Arc<EventSenders>,
}

impl Database {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            mutation_hook: None,
            field_policy: None,
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
            field_policy: None,
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
            field_policy: Some(Arc::new(policy)),
            event_senders: Arc::new(EventSenders::default()),
        }
    }

    pub fn with_hooks<M, F>(pool: DbPool, mutation_hook: M, field_policy: F) -> Self
    where
        M: crate::graphql::orm::MutationHook + 'static,
        F: crate::graphql::orm::FieldPolicy + 'static,
    {
        Self {
            pool,
            mutation_hook: Some(Arc::new(mutation_hook)),
            field_policy: Some(Arc::new(field_policy)),
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

    pub fn set_field_policy<H>(&mut self, policy: H)
    where
        H: crate::graphql::orm::FieldPolicy + 'static,
    {
        self.field_policy = Some(Arc::new(policy));
    }

    pub fn field_policy(&self) -> Option<&Arc<dyn crate::graphql::orm::FieldPolicy>> {
        self.field_policy.as_ref()
    }

    pub async fn run_mutation_hook(
        &self,
        ctx: &async_graphql::Context<'_>,
        event: &crate::graphql::orm::MutationEvent,
    ) -> async_graphql::Result<()> {
        if let Some(hook) = &self.mutation_hook {
            hook.on_mutation(Some(ctx), self, event).await?;
        }

        Ok(())
    }

    pub async fn run_mutation_hook_without_context(
        &self,
        event: &crate::graphql::orm::MutationEvent,
    ) -> async_graphql::Result<()> {
        if let Some(hook) = &self.mutation_hook {
            hook.on_mutation(None, self, event).await?;
        }

        Ok(())
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
        if let Some(sender) = self.event_sender::<T>() {
            let _ = sender.send(event);
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
}

#[cfg(feature = "sqlite")]
pub mod sqlite_helpers {
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

#[cfg(feature = "postgres")]
pub mod postgres_helpers {
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
