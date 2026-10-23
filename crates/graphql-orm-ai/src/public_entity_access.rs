//! Current-principal policy binding for generated AI metadata reads.

use crate::public_entities::*;
use crate::{AiAccessPolicy, AiScope, AiSessionAction, AiSessionId};
use agql_auth::{AuthPrincipal, CurrentPrincipalResolver};
use async_graphql::Context;
use graphql_orm::graphql::{
    auth::AuthorizationMode,
    errors::OrmPublicError,
    orm::{
        DefaultWriteBackend, EntityAccessKind, EntityAccessSurface, EntityPolicy, FieldPolicy,
        ReadVisibility, RowPolicy,
    },
};
use graphql_orm::{db::Database, futures::future::BoxFuture};
use std::{any::Any, sync::Arc};

/// Preserve current ORM decisions for SimpleObject scalar fields composed
/// with generated ComplexObject relations.
pub(crate) struct PublicMetadataFieldGuard<'a, T> {
    record: &'a T,
}
impl<'a, T> PublicMetadataFieldGuard<'a, T> {
    pub(crate) fn new(record: &'a T) -> Self {
        Self { record }
    }
}
impl<T> async_graphql::Guard for PublicMetadataFieldGuard<'_, T>
where
    T: graphql_orm::graphql::orm::Entity + Send + Sync + 'static,
{
    async fn check(&self, context: &Context<'_>) -> async_graphql::Result<()> {
        let denied = || OrmPublicError::forbidden().into_graphql_error();
        let database = context.data::<Database<DefaultWriteBackend>>()?;
        if database.authorization_mode() != AuthorizationMode::ExplicitPolicyForAllExposedOperations
            || !database
                .entity_policy()
                .is_some_and(|policy| policy.supports_policy_binding("ai.public.metadata"))
            || !database
                .row_policy()
                .is_some_and(|policy| policy.supports_policy_binding("ai.public.metadata"))
            || !database
                .field_policy()
                .is_some_and(|policy| policy.supports_policy_binding("ai.public.metadata"))
        {
            return Err(denied());
        }
        let surface = if context.query_env.operation.node.ty
            == async_graphql::parser::types::OperationType::Subscription
        {
            EntityAccessSurface::GraphqlSubscription
        } else {
            EntityAccessSurface::GraphqlQuery
        };
        database
            .ensure_entity_access(
                Some(context),
                T::entity_name(),
                T::metadata().read_policy,
                EntityAccessKind::Read,
                surface,
            )
            .await?;
        if !database
            .can_read_row(
                Some(context),
                T::entity_name(),
                T::metadata().read_policy,
                surface,
                self.record,
            )
            .await?
        {
            return Err(denied());
        }
        let field = T::repository_field_policies()
            .iter()
            .find(|field| field.api_name == context.item.node.name.node.as_str())
            .ok_or_else(denied)?;
        database
            .ensure_readable_field(
                context,
                T::entity_name(),
                field.api_name,
                field.read_policy,
                Some(self.record),
            )
            .await
    }
}

/// Mandatory authorization binding for generated, read-only AI metadata entities.
///
/// This permits no content opening, writes, schema ownership or provider disclosure.
/// Keep the original database for AI-owned services and schema modules; compose the
/// public entities with [`Self::database`]. Existing host policies are narrowed,
/// never replaced with an unconditional allow. Missing principal or resolver and
/// policy failures deny access. Metadata reads require scope/session Read and List;
/// the scope tool-policy projection requires the same two scope decisions.
#[derive(Clone)]
pub struct AiPublicEntityAccess {
    repository: Database<DefaultWriteBackend>,
    policy: Arc<dyn AiAccessPolicy>,
    principals: Arc<dyn CurrentPrincipalResolver>,
}

impl AiPublicEntityAccess {
    /// Binds the private repository, host AI access policy and current-principal resolver.
    pub fn new(
        repository: Database<DefaultWriteBackend>,
        policy: Arc<dyn AiAccessPolicy>,
        principals: Arc<dyn CurrentPrincipalResolver>,
    ) -> Self {
        Self {
            repository,
            policy,
            principals,
        }
    }

    /// Returns a database clone that enforces generated AI metadata authorization.
    ///
    /// Existing entity, row and field callbacks remain additional restrictions.
    /// Non-AI operations need their own host callbacks. Public metadata entities
    /// remain read-only even if a host attempts ordinary repository writes.
    pub fn database(&self) -> Database<DefaultWriteBackend> {
        let mut database = self.repository.clone();
        let guard = Guard {
            access: self.clone(),
            entity: database.entity_policy().cloned(),
            row: database.row_policy().cloned(),
            field: database.field_policy().cloned(),
        };
        database.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
        database.set_entity_policy(guard.clone());
        database.set_row_policy(guard.clone());
        database.set_field_policy(guard);
        database
    }

    async fn current(&self, context: &Context<'_>) -> async_graphql::Result<AuthPrincipal> {
        let observed = agql_auth::principal_from_ctx(context)?;
        let reference = observed.reference();
        let current = self
            .principals
            .resolve(&reference)
            .await
            .map_err(|_| OrmPublicError::forbidden().into_graphql_error())?;
        if !reference.matches(current.principal()) {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        Ok(current.into_principal())
    }

    async fn scope(&self, principal: &AuthPrincipal, scope: &AiScope) -> bool {
        if principal.reference().tenant_id != scope.tenant_id {
            return false;
        }
        for action in [AiSessionAction::Read, AiSessionAction::List] {
            if !self
                .policy
                .can_access_scope(principal, scope, action)
                .await
                .is_allowed()
            {
                return false;
            }
        }
        true
    }

    async fn session(
        &self,
        principal: &AuthPrincipal,
        id: graphql_orm::uuid::Uuid,
    ) -> async_graphql::Result<bool> {
        let Some(session) = crate::persistence::AiSessionRecord::find_by_id(&self.repository, &id)
            .await
            .map_err(|_| OrmPublicError::forbidden().into_graphql_error())?
        else {
            return Ok(false);
        };
        let (kind, subject) = crate::orm_sessions::principal_identity(principal);
        if session.owner_principal_kind != kind
            || session.owner_subject != subject
            || session.deleted_at.is_some()
        {
            return Ok(false);
        }
        for action in [AiSessionAction::Read, AiSessionAction::List] {
            if !self
                .policy
                .can_access_session(principal, AiSessionId(id), action)
                .await
                .is_allowed()
            {
                return Ok(false);
            }
        }
        Ok(self
            .scope(principal, &crate::orm_sessions::record_scope(&session))
            .await)
    }

    async fn row(
        &self,
        context: &Context<'_>,
        entity: &str,
        row: &(dyn Any + Send + Sync),
    ) -> async_graphql::Result<bool> {
        let principal = self.current(context).await?;
        let session = match entity {
            "AiPublicSession" => row.downcast_ref::<AiPublicSession>().map(|row| row.id),
            "AiPublicMessage" => row
                .downcast_ref::<AiPublicMessage>()
                .map(|row| row.session_id),
            "AiPublicRun" => row.downcast_ref::<AiPublicRun>().map(|row| row.session_id),
            "AiPublicAttachment" => row
                .downcast_ref::<AiPublicAttachment>()
                .filter(|row| row.deleted_at.is_none())
                .map(|row| row.session_id),
            "AiPublicToolPolicy" => {
                let Some(row) = row.downcast_ref::<AiPublicToolPolicy>() else {
                    return Ok(false);
                };
                return Ok(self
                    .scope(
                        &principal,
                        &AiScope {
                            kind: row.scope_kind.clone(),
                            id: row.scope_id.clone(),
                            tenant_id: row.tenant_id.clone(),
                        },
                    )
                    .await);
            }
            _ => None,
        };
        if entity == "AiPublicAttachment" {
            let Some(projection) = row.downcast_ref::<AiPublicAttachment>() else {
                return Ok(false);
            };
            let Some(stored) = crate::persistence::AiAttachmentRecord::find_by_id(
                &self.repository,
                &projection.id,
            )
            .await
            .map_err(|_| OrmPublicError::forbidden().into_graphql_error())?
            else {
                return Ok(false);
            };
            let (kind, subject) = crate::orm_sessions::principal_identity(&principal);
            if stored.session_id != projection.session_id
                || stored.owner_principal_kind != kind
                || stored.owner_subject != subject
                || stored.deleted_at.is_some()
            {
                return Ok(false);
            }
        }
        match session {
            Some(id) => self.session(&principal, id).await,
            None => Ok(false),
        }
    }

    async fn source_row(
        &self,
        context: &Context<'_>,
        entity: &str,
        row: &(dyn Any + Send + Sync),
    ) -> async_graphql::Result<bool> {
        use crate::persistence::{
            AiAttachmentRecord, AiMessageRecord, AiRunRecord, AiSessionRecord, AiToolPolicyRecord,
        };
        let principal = self.current(context).await?;
        let session = match entity {
            "AiSessionRecord" => row
                .downcast_ref::<AiSessionRecord>()
                .filter(|row| row.deleted_at.is_none())
                .map(|row| row.id),
            "AiMessageRecord" => row
                .downcast_ref::<AiMessageRecord>()
                .map(|row| row.session_id),
            "AiRunRecord" => row.downcast_ref::<AiRunRecord>().map(|row| row.session_id),
            "AiAttachmentRecord" => {
                let Some(row) = row.downcast_ref::<AiAttachmentRecord>() else {
                    return Ok(false);
                };
                let (kind, subject) = crate::orm_sessions::principal_identity(&principal);
                if row.owner_principal_kind != kind
                    || row.owner_subject != subject
                    || row.deleted_at.is_some()
                {
                    return Ok(false);
                }
                Some(row.session_id)
            }
            "AiToolPolicyRecord" => {
                let Some(row) = row.downcast_ref::<AiToolPolicyRecord>() else {
                    return Ok(false);
                };
                return Ok(self
                    .scope(
                        &principal,
                        &AiScope {
                            kind: row.scope_kind.clone(),
                            id: row.scope_id.clone(),
                            tenant_id: row.tenant_id.clone(),
                        },
                    )
                    .await);
            }
            _ => return Ok(false),
        };
        match session {
            Some(id) => self.session(&principal, id).await,
            None => Ok(false),
        }
    }
}

fn public(entity: &str) -> bool {
    matches!(
        entity,
        "AiPublicSession"
            | "AiPublicMessage"
            | "AiPublicRun"
            | "AiPublicAttachment"
            | "AiPublicToolPolicy"
    )
}

// Private persistence remains absent from the public schema. Only these exact
// source types may prove a generated projection hop, and only on subscriptions.
fn projection_source(entity: &str) -> bool {
    matches!(
        entity,
        "AiSessionRecord"
            | "AiMessageRecord"
            | "AiRunRecord"
            | "AiAttachmentRecord"
            | "AiToolPolicyRecord"
    )
}
fn subscription_context(context: &Context<'_>) -> bool {
    context.query_env.operation.node.ty
        == graphql_orm::async_graphql::parser::types::OperationType::Subscription
}

#[derive(Clone)]
struct Guard {
    access: AiPublicEntityAccess,
    entity: Option<Arc<dyn EntityPolicy<DefaultWriteBackend>>>,
    row: Option<Arc<dyn RowPolicy<DefaultWriteBackend>>>,
    field: Option<Arc<dyn FieldPolicy<DefaultWriteBackend>>>,
}

impl EntityPolicy<DefaultWriteBackend> for Guard {
    fn supports_policy_binding(&self, binding: &str) -> bool {
        binding == "ai.public.metadata"
            || self
                .entity
                .as_ref()
                .is_some_and(|policy| policy.supports_policy_binding(binding))
    }
    fn can_access_entity<'a>(
        &'a self,
        context: Option<&'a Context<'_>>,
        database: &'a Database<DefaultWriteBackend>,
        entity: &'static str,
        policy: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                if kind != EntityAccessKind::Read || policy != Some("ai.public.read") {
                    return Ok(false);
                }
                let Some(context) = context else {
                    return Ok(false);
                };
                self.access.current(context).await?;
                if let Some(delegate) = &self.entity {
                    return delegate
                        .can_access_entity(Some(context), database, entity, policy, kind, surface)
                        .await;
                }
                return Ok(true);
            }
            if projection_source(entity)
                && surface == EntityAccessSurface::GraphqlSubscription
                && kind == EntityAccessKind::Read
            {
                let Some(context) = context else {
                    return Ok(false);
                };
                self.access.current(context).await?;
                if let Some(delegate) = &self.entity {
                    return delegate
                        .can_access_entity(Some(context), database, entity, policy, kind, surface)
                        .await;
                }
                return Ok(true);
            }
            match &self.entity {
                Some(delegate) => {
                    delegate
                        .can_access_entity(context, database, entity, policy, kind, surface)
                        .await
                }
                None => Ok(false),
            }
        })
    }
}

impl RowPolicy<DefaultWriteBackend> for Guard {
    fn supports_policy_binding(&self, binding: &str) -> bool {
        binding == "ai.public.metadata"
            || self
                .row
                .as_ref()
                .is_some_and(|policy| policy.supports_policy_binding(binding))
    }
    fn read_visibility<'a>(
        &'a self,
        context: Option<&'a Context<'_>>,
        database: &'a Database<DefaultWriteBackend>,
        entity: &'static str,
        policy: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            // A complete SQL predicate must never skip the AI row authorization callback.
            if public(entity)
                || (projection_source(entity)
                    && surface == EntityAccessSurface::GraphqlSubscription)
            {
                return Ok(ReadVisibility::CallbackOnly);
            }
            match &self.row {
                Some(delegate) => {
                    delegate
                        .read_visibility(context, database, entity, policy, surface)
                        .await
                }
                None => Ok(ReadVisibility::CallbackOnly),
            }
        })
    }
    fn can_read_row<'a>(
        &'a self,
        context: Option<&'a Context<'_>>,
        database: &'a Database<DefaultWriteBackend>,
        entity: &'static str,
        policy: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                let Some(context) = context else {
                    return Ok(false);
                };
                if !self.access.row(context, entity, row).await? {
                    return Ok(false);
                }
                if let Some(delegate) = &self.row {
                    return delegate
                        .can_read_row(Some(context), database, entity, policy, surface, row)
                        .await;
                }
                return Ok(true);
            }
            if projection_source(entity) && surface == EntityAccessSurface::GraphqlSubscription {
                let Some(context) = context else {
                    return Ok(false);
                };
                if !self.access.source_row(context, entity, row).await? {
                    return Ok(false);
                }
                if let Some(delegate) = &self.row {
                    return delegate
                        .can_read_row(Some(context), database, entity, policy, surface, row)
                        .await;
                }
                return Ok(true);
            }
            match &self.row {
                Some(delegate) => {
                    delegate
                        .can_read_row(context, database, entity, policy, surface, row)
                        .await
                }
                None => Ok(false),
            }
        })
    }
    fn can_write_row<'a>(
        &'a self,
        context: Option<&'a Context<'_>>,
        database: &'a Database<DefaultWriteBackend>,
        entity: &'static str,
        policy: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                return Ok(false);
            }
            match &self.row {
                Some(delegate) => {
                    delegate
                        .can_write_row(context, database, entity, policy, surface, row)
                        .await
                }
                None => Ok(false),
            }
        })
    }
}

impl FieldPolicy<DefaultWriteBackend> for Guard {
    fn supports_policy_binding(&self, binding: &str) -> bool {
        binding == "ai.public.metadata"
            || self
                .field
                .as_ref()
                .is_some_and(|policy| policy.supports_policy_binding(binding))
    }
    fn can_read_field<'a>(
        &'a self,
        context: &'a Context<'_>,
        database: &'a Database<DefaultWriteBackend>,
        entity: &'static str,
        field: &'static str,
        policy: Option<&'static str>,
        row: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                if let Some(row) = row {
                    if !self.access.row(context, entity, row).await? {
                        return Ok(false);
                    }
                } else {
                    self.access.current(context).await?;
                }
                if let Some(delegate) = &self.field {
                    return delegate
                        .can_read_field(context, database, entity, field, policy, row)
                        .await;
                }
                return Ok(true);
            }
            if projection_source(entity) && subscription_context(context) {
                // Only the public owner-issued ID and the declared projection
                // link can witness this hop. No protected/content/custody field.
                if !field.eq_ignore_ascii_case("id")
                    && !field
                        .replace('_', "")
                        .eq_ignore_ascii_case("publicMetadata")
                {
                    return Ok(false);
                }
                let Some(row) = row else {
                    return Ok(false);
                };
                if !self.access.source_row(context, entity, row).await? {
                    return Ok(false);
                }
                if let Some(delegate) = &self.field {
                    return delegate
                        .can_read_field(context, database, entity, field, policy, Some(row))
                        .await;
                }
                return Ok(true);
            }
            match &self.field {
                Some(delegate) => {
                    delegate
                        .can_read_field(context, database, entity, field, policy, row)
                        .await
                }
                None => Ok(false),
            }
        })
    }
    fn can_write_field<'a>(
        &'a self,
        context: &'a Context<'_>,
        database: &'a Database<DefaultWriteBackend>,
        entity: &'static str,
        field: &'static str,
        policy: Option<&'static str>,
        row: Option<&'a (dyn Any + Send + Sync)>,
        value: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                return Ok(false);
            }
            match &self.field {
                Some(delegate) => {
                    delegate
                        .can_write_field(context, database, entity, field, policy, row, value)
                        .await
                }
                None => Ok(false),
            }
        })
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod notification_tests {
    use super::*;
    use crate::persistence::*;
    use graphql_orm::async_graphql::Response;
    use graphql_orm::futures::{Stream, StreamExt, future::poll_fn};
    use graphql_orm::prelude::*;
    use std::task::Poll;
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    schema_roots! { query_custom_ops: [], generated_mutations: "none", entities: [AiPublicSession, AiPublicMessage, AiPublicRun, AiPublicAttachment, AiPublicToolPolicy], }

    async fn registered<S: Stream<Item = Response> + Unpin>(stream: &mut S) {
        poll_fn(|cx| match stream.poll_next_unpin(cx) {
            Poll::Pending => Poll::Ready(()),
            other => panic!("unexpected initial event {other:?}"),
        })
        .await;
    }
    #[derive(Clone)]
    struct Allow;
    impl EntityPolicy for Allow {
        fn supports_policy_binding(&self, binding: &str) -> bool {
            binding == "ai.public.metadata"
        }
        fn can_access_entity<'a>(
            &'a self,
            _: Option<&'a Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessKind,
            _: EntityAccessSurface,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    impl RowPolicy for Allow {
        fn supports_policy_binding(&self, binding: &str) -> bool {
            binding == "ai.public.metadata"
        }
        fn can_read_row<'a>(
            &'a self,
            _: Option<&'a Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn Any + Send + Sync),
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
        fn can_write_row<'a>(
            &'a self,
            _: Option<&'a Context<'_>>,
            _: &'a Database,
            _: &'static str,
            _: Option<&'static str>,
            _: EntityAccessSurface,
            _: &'a (dyn Any + Send + Sync),
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }
    impl FieldPolicy for Allow {
        fn supports_policy_binding(&self, binding: &str) -> bool {
            binding == "ai.public.metadata"
        }
        fn can_read_field<'a>(
            &'a self,
            _: &'a Context<'_>,
            _: &'a Database,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn Any + Send + Sync)>,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
        fn can_write_field<'a>(
            &'a self,
            _: &'a Context<'_>,
            _: &'a Database,
            _: &'static str,
            _: &'static str,
            _: Option<&'static str>,
            _: Option<&'a (dyn Any + Send + Sync)>,
            _: Option<&'a (dyn Any + Send + Sync)>,
        ) -> BoxFuture<'a, async_graphql::Result<bool>> {
            Box::pin(async { Ok(true) })
        }
    }

    // This fixture isolates generated graph propagation/rollback. Principal and
    // AI-policy behavior is qualified through the public binding integration tests.
    async fn fixture() -> (Database, Uuid, Uuid, Uuid) {
        let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
        let plan = db
            .schema()
            .plan_migration_to_entities(
                "ai-notification-owned",
                "test",
                crate::AiSchemaModule.entities(),
            )
            .await
            .unwrap();
        db.schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap();
        db.set_authorization_mode(AuthorizationMode::ExplicitPolicyForAllExposedOperations);
        db.set_entity_policy(Allow);
        db.set_row_policy(Allow);
        db.set_field_policy(Allow);
        let session = Uuid::new_v4();
        let message = Uuid::new_v4();
        let run = Uuid::new_v4();
        AiSessionRecord::insert(
            &db,
            CreateAiSessionRecordInput {
                execution_selection: None,
                id: session,
                owner_principal_kind: "user".into(),
                owner_subject: "tester".into(),
                tenant_id: None,
                scope_kind: "owned".into(),
                scope_id: "test".into(),
                title: "before".into(),
                title_revision: 0,
                title_source: "user".into(),
                state: "active".into(),
                stream_head: 0,
                message_head: 0,
                last_activity_at: 0,
                archived_at: None,
                deleted_at: None,
            },
        )
        .await
        .unwrap();
        AiMessageRecord::insert(
            &db,
            CreateAiMessageRecordInput {
                id: message,
                session_id: session,
                sequence: 1,
                message_role: "user".into(),
                author_principal_kind: None,
                author_subject: None,
                client_message_id: None,
                content_hash: None,
                run_id: Some(run),
                provider_kind: None,
                provider_model: None,
                protected_preview: None,
                block_count: 0,
                completion_state: "streaming".into(),
                finalized_at: None,
                content_purged_at: None,
            },
        )
        .await
        .unwrap();
        AiRunRecord::insert(
            &db,
            CreateAiRunRecordInput {
                execution_selection: None,
                id: run,
                session_id: session,
                input_message_id: message,
                principal_reference: serde_json::json!({}),
                state: "queued".into(),
                attempt_id: None,
                lease_owner: None,
                lease_generation: 0,
                lease_expires_at: None,
                lease_heartbeat_at: None,
                retry_count: 0,
                next_attempt_at: None,
                error_code: None,
                latest_checkpoint_id: None,
                cancellation_request_id: None,
                cancellation_requested_at: None,
            },
        )
        .await
        .unwrap();
        (db, session, message, run)
    }
    #[tokio::test]
    async fn message_update_propagates_to_both_existing_public_parents() {
        let (db, session, message, run) = fixture().await;
        let schema = schema_builder(db.clone())
            .data(AuthSubject::new("tester"))
            .finish();
        let mut sessions = Box::pin(schema.execute_stream(format!(
            "subscription {{ aiPublicSessionChanged(key: \"{session}\") {{ sourceId path }} }}"
        )));
        let mut runs = Box::pin(schema.execute_stream(format!(
            "subscription {{ aiPublicRunChanged(key: \"{run}\") {{ sourceId path }} }}"
        )));
        registered(&mut sessions).await;
        registered(&mut runs).await;
        AiMessageRecord::update_by_id(
            &db,
            &message,
            UpdateAiMessageRecordInput {
                completion_state: Some("complete".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let a = timeout(Duration::from_secs(2), sessions.next())
            .await
            .unwrap()
            .unwrap();
        let b = timeout(Duration::from_secs(2), runs.next())
            .await
            .unwrap()
            .unwrap();
        assert!(a.errors.is_empty(), "{:?}", a.errors);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        let a = a.data.into_json().unwrap();
        let b = b.data.into_json().unwrap();
        assert_eq!(a["aiPublicSessionChanged"]["sourceId"], message.to_string());
        assert_eq!(b["aiPublicRunChanged"]["sourceId"], message.to_string());
        assert!(
            a["aiPublicSessionChanged"]["path"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part == "session")
        );
        assert!(
            b["aiPublicRunChanged"]["path"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part == "run")
        );
    }
    #[tokio::test]
    async fn run_update_propagates_to_its_existing_public_session() {
        let (db, session, _, run) = fixture().await;
        let schema = schema_builder(db.clone())
            .data(AuthSubject::new("tester"))
            .finish();
        let mut sessions = Box::pin(schema.execute_stream(format!(
            "subscription {{ aiPublicSessionChanged(key: \"{session}\") {{ sourceId path }} }}"
        )));
        registered(&mut sessions).await;
        AiRunRecord::update_by_id(
            &db,
            &run,
            UpdateAiRunRecordInput {
                state: Some("running".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let response = timeout(Duration::from_secs(2), sessions.next())
            .await
            .unwrap()
            .unwrap();
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let value = response.data.into_json().unwrap();
        assert_eq!(
            value["aiPublicSessionChanged"]["sourceId"],
            run.to_string()
        );
        assert!(
            value["aiPublicSessionChanged"]["path"]
                .as_array()
                .unwrap()
                .iter()
                .any(|part| part == "session")
        );
    }
    #[tokio::test]
    async fn rolled_back_private_source_update_emits_no_public_metadata() {
        let (db, session, message, _) = fixture().await;
        let schema = schema_builder(db.clone())
            .data(AuthSubject::new("tester"))
            .finish();
        let mut stream = Box::pin(schema.execute_stream(format!(
            "subscription {{ aiPublicSessionChanged(key: \"{session}\") {{ id }} }}"
        )));
        registered(&mut stream).await;
        let result: Result<(), TransactionError> = db
            .transaction(TransactionMode::Default, move |tx| {
                Box::pin(async move {
                    tx.update_by_id::<AiMessageRecord>(
                        &message,
                        UpdateAiMessageRecordInput {
                            completion_state: Some("rolled-back".into()),
                            ..Default::default()
                        },
                    )
                    .await?;
                    Err(OrmPublicError::forbidden())
                })
            })
            .await;
        assert!(result.is_err());
        assert!(
            timeout(Duration::from_millis(100), stream.next())
                .await
                .is_err()
        );
    }
}
