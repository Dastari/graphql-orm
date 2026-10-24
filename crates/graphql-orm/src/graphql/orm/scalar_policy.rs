use super::{DefaultBackend, Entity, EntityAccessKind, EntityAccessSurface, OrmBackend};
use crate::db::Database;
use crate::graphql::errors::OrmPublicError;
use std::marker::PhantomData;

/// Current ORM authorization for a scalar getter owned by `SimpleObject`.
///
/// Prefer [`crate::graphql_policy_object`] to install this guard on the complete
/// object while preserving existing guards and generated relations. The guard
/// checks the current entity, row and selected field decisions on every read;
/// it grants no principal authority or write capability.
pub struct EntityFieldPolicyGuard<'a, T, B: OrmBackend = DefaultBackend> {
    record: &'a T,
    backend: PhantomData<fn() -> B>,
}

impl<'a, T, B: OrmBackend> EntityFieldPolicyGuard<'a, T, B> {
    /// Binds a scalar read to its concrete record and database backend.
    pub fn new(record: &'a T) -> Self {
        Self {
            record,
            backend: PhantomData,
        }
    }
}

impl<T, B> crate::async_graphql::Guard for EntityFieldPolicyGuard<'_, T, B>
where
    T: Entity + Send + Sync + 'static,
    B: OrmBackend + 'static,
{
    async fn check(
        &self,
        context: &crate::async_graphql::Context<'_>,
    ) -> crate::async_graphql::Result<()> {
        let denied = || OrmPublicError::forbidden().into_graphql_error();
        let field = T::repository_field_policies()
            .iter()
            .find(|field| field.api_name == context.item.node.name.node.as_str())
            .ok_or_else(denied)?;
        let subscription = context.query_env.operation.node.ty
            == crate::async_graphql::parser::types::OperationType::Subscription;
        if subscription
            && !T::subscription_field_policy(field.api_name)
                .is_some_and(|(_, available, _)| available)
        {
            return Err(denied());
        }
        let surface = if subscription {
            EntityAccessSurface::GraphqlSubscription
        } else {
            EntityAccessSurface::GraphqlQuery
        };
        let database = context.data::<Database<B>>()?;
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
