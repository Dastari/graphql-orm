//! Explicit policy decisions for the disposable examples' declared read keys.
use async_graphql::Context;
use graphql_orm::{db::Database, prelude::*};

/// Grants the two declared read keys only for these synthetic endpoint examples.
pub struct EndpointFixturePolicy;

impl EntityPolicy<SqliteBackend> for EndpointFixturePolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a Context<'_>>,
        _: &'a Database<SqliteBackend>,
        entity: &'static str,
        policy: Option<&'static str>,
        kind: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(policy.is_none()
                || (entity == "Endpoint"
                    && kind == EntityAccessKind::Read
                    && policy == Some("endpoint.read")))
        })
    }
}

impl FieldPolicy<SqliteBackend> for EndpointFixturePolicy {
    fn can_read_field<'a>(
        &'a self,
        ctx: &'a Context<'_>,
        _: &'a Database<SqliteBackend>,
        entity: &'static str,
        field: &'static str,
        policy: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            assert_eq!(ctx.field().name(), field);
            Ok(policy.is_none()
                || (entity == "Endpoint"
                    && field == "name"
                    && policy == Some("endpoint.name.read")))
        })
    }

    fn can_write_field<'a>(
        &'a self,
        _: &'a Context<'_>,
        _: &'a Database<SqliteBackend>,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
        _: Option<&'a (dyn std::any::Any + Send + Sync)>,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(false) })
    }
}
