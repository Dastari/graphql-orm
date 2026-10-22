//! Current-principal policy binding for generated AI metadata reads.

use std::{any::Any, sync::Arc};
use agql_auth::{AuthPrincipal, CurrentPrincipalResolver};
use async_graphql::Context;
use graphql_orm::{db::Database, futures::future::BoxFuture};
use graphql_orm::graphql::{auth::AuthorizationMode, errors::OrmPublicError, orm::{
    DefaultWriteBackend, EntityAccessKind, EntityAccessSurface, EntityPolicy, FieldPolicy,
    ReadVisibility, RowPolicy,
}};
use crate::{AiAccessPolicy, AiScope, AiSessionAction, AiSessionId};
use crate::public_entities::*;

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
        Self { repository, policy, principals }
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
        let current = self.principals.resolve(&reference).await
            .map_err(|_| OrmPublicError::forbidden().into_graphql_error())?;
        if !reference.matches(current.principal()) {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        Ok(current.into_principal())
    }

    async fn scope(&self, principal: &AuthPrincipal, scope: &AiScope) -> bool {
        if principal.reference().tenant_id != scope.tenant_id { return false; }
        for action in [AiSessionAction::Read, AiSessionAction::List] {
            if !self.policy.can_access_scope(principal, scope, action).await.is_allowed() { return false; }
        }
        true
    }

    async fn session(&self, principal: &AuthPrincipal, id: graphql_orm::uuid::Uuid) -> async_graphql::Result<bool> {
        let Some(session) = crate::persistence::AiSessionRecord::find_by_id(&self.repository, &id).await
            .map_err(|_| OrmPublicError::forbidden().into_graphql_error())? else { return Ok(false) };
        let (kind, subject) = crate::orm_sessions::principal_identity(principal);
        if session.owner_principal_kind != kind || session.owner_subject != subject || session.deleted_at.is_some() {
            return Ok(false);
        }
        for action in [AiSessionAction::Read, AiSessionAction::List] {
            if !self.policy.can_access_session(principal, AiSessionId(id), action).await.is_allowed() { return Ok(false); }
        }
        Ok(self.scope(principal, &crate::orm_sessions::record_scope(&session)).await)
    }

    async fn row(&self, context: &Context<'_>, entity: &str, row: &(dyn Any + Send + Sync)) -> async_graphql::Result<bool> {
        let principal = self.current(context).await?;
        let session = match entity {
            "AiPublicSession" => row.downcast_ref::<AiPublicSession>().map(|row| row.id),
            "AiPublicMessage" => row.downcast_ref::<AiPublicMessage>().map(|row| row.session_id),
            "AiPublicRun" => row.downcast_ref::<AiPublicRun>().map(|row| row.session_id),
            "AiPublicAttachment" => row.downcast_ref::<AiPublicAttachment>().filter(|row| row.deleted_at.is_none()).map(|row| row.session_id),
            "AiPublicToolPolicy" => {
                let Some(row) = row.downcast_ref::<AiPublicToolPolicy>() else { return Ok(false) };
                return Ok(self.scope(&principal, &AiScope { kind: row.scope_kind.clone(), id: row.scope_id.clone(), tenant_id: row.tenant_id.clone() }).await);
            }
            _ => None,
        };
        if entity == "AiPublicAttachment" {
            let Some(projection) = row.downcast_ref::<AiPublicAttachment>() else { return Ok(false) };
            let Some(stored) = crate::persistence::AiAttachmentRecord::find_by_id(&self.repository, &projection.id).await
                .map_err(|_| OrmPublicError::forbidden().into_graphql_error())? else { return Ok(false) };
            let (kind, subject) = crate::orm_sessions::principal_identity(&principal);
            if stored.session_id != projection.session_id || stored.owner_principal_kind != kind || stored.owner_subject != subject || stored.deleted_at.is_some() { return Ok(false); }
        }
        match session { Some(id) => self.session(&principal, id).await, None => Ok(false) }
    }
}

fn public(entity: &str) -> bool {
    matches!(entity, "AiPublicSession" | "AiPublicMessage" | "AiPublicRun" | "AiPublicAttachment" | "AiPublicToolPolicy")
}

#[derive(Clone)]
struct Guard {
    access: AiPublicEntityAccess,
    entity: Option<Arc<dyn EntityPolicy<DefaultWriteBackend>>>,
    row: Option<Arc<dyn RowPolicy<DefaultWriteBackend>>>,
    field: Option<Arc<dyn FieldPolicy<DefaultWriteBackend>>>,
}

impl EntityPolicy<DefaultWriteBackend> for Guard {
    fn can_access_entity<'a>(&'a self, context: Option<&'a Context<'_>>, database: &'a Database<DefaultWriteBackend>, entity: &'static str, policy: Option<&'static str>, kind: EntityAccessKind, surface: EntityAccessSurface) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                if kind != EntityAccessKind::Read || policy != Some("ai.public.read") { return Ok(false); }
                let Some(context) = context else { return Ok(false) };
                self.access.current(context).await?;
                if let Some(delegate) = &self.entity { return delegate.can_access_entity(Some(context), database, entity, policy, kind, surface).await; }
                return Ok(true);
            }
            match &self.entity { Some(delegate) => delegate.can_access_entity(context, database, entity, policy, kind, surface).await, None => Ok(false) }
        })
    }
}

impl RowPolicy<DefaultWriteBackend> for Guard {
    fn read_visibility<'a>(&'a self, context: Option<&'a Context<'_>>, database: &'a Database<DefaultWriteBackend>, entity: &'static str, policy: Option<&'static str>, surface: EntityAccessSurface) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            // A complete SQL predicate must never skip the AI row authorization callback.
            if public(entity) { return Ok(ReadVisibility::CallbackOnly); }
            match &self.row { Some(delegate) => delegate.read_visibility(context, database, entity, policy, surface).await, None => Ok(ReadVisibility::CallbackOnly) }
        })
    }
    fn can_read_row<'a>(&'a self, context: Option<&'a Context<'_>>, database: &'a Database<DefaultWriteBackend>, entity: &'static str, policy: Option<&'static str>, surface: EntityAccessSurface, row: &'a (dyn Any + Send + Sync)) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                let Some(context) = context else { return Ok(false) };
                if !self.access.row(context, entity, row).await? { return Ok(false); }
                if let Some(delegate) = &self.row { return delegate.can_read_row(Some(context), database, entity, policy, surface, row).await; }
                return Ok(true);
            }
            match &self.row { Some(delegate) => delegate.can_read_row(context, database, entity, policy, surface, row).await, None => Ok(false) }
        })
    }
    fn can_write_row<'a>(&'a self, context: Option<&'a Context<'_>>, database: &'a Database<DefaultWriteBackend>, entity: &'static str, policy: Option<&'static str>, surface: EntityAccessSurface, row: &'a (dyn Any + Send + Sync)) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) { return Ok(false); }
            match &self.row { Some(delegate) => delegate.can_write_row(context, database, entity, policy, surface, row).await, None => Ok(false) }
        })
    }
}

impl FieldPolicy<DefaultWriteBackend> for Guard {
    fn can_read_field<'a>(&'a self, context: &'a Context<'_>, database: &'a Database<DefaultWriteBackend>, entity: &'static str, field: &'static str, policy: Option<&'static str>, row: Option<&'a (dyn Any + Send + Sync)>) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) {
                if let Some(row) = row { if !self.access.row(context, entity, row).await? { return Ok(false); } }
                else { self.access.current(context).await?; }
                if let Some(delegate) = &self.field { return delegate.can_read_field(context, database, entity, field, policy, row).await; }
                return Ok(true);
            }
            match &self.field { Some(delegate) => delegate.can_read_field(context, database, entity, field, policy, row).await, None => Ok(false) }
        })
    }
    fn can_write_field<'a>(&'a self, context: &'a Context<'_>, database: &'a Database<DefaultWriteBackend>, entity: &'static str, field: &'static str, policy: Option<&'static str>, row: Option<&'a (dyn Any + Send + Sync)>, value: Option<&'a (dyn Any + Send + Sync)>) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if public(entity) { return Ok(false); }
            match &self.field { Some(delegate) => delegate.can_write_field(context, database, entity, field, policy, row, value).await, None => Ok(false) }
        })
    }
}
