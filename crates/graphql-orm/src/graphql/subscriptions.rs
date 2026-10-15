//! Generated subscription release checks and bounded, server-only provenance.

use crate::db::Database;
use crate::graphql::errors::{OrmPublicError, graphql_error_from_sqlx};
use crate::graphql::orm::{
    DbAuthContext, DefaultBackend, Entity, EntityAccessKind, EntityAccessSurface, EntityQuery,
    FromSqlRow, OrmBackend, ReadVisibility, SqlDialect, SqlValue,
};
use async_graphql::Context;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Maximum number of relationship hops retained and checked for an event.
pub const MAX_SUBSCRIPTION_PROVENANCE_HOPS: usize = 16;
/// Maximum propagated targets examined for one originating change.
pub const MAX_SUBSCRIPTION_PROPAGATION_EVENTS: usize = 256;

type SourceCheck<B> = for<'a, 'ctx> fn(
    &'a Database<B>,
    &'a Context<'ctx>,
    &'a [SqlValue],
    &'a [SqlValue],
    &'static str,
    &'static str,
) -> BoxFuture<'a, async_graphql::Result<()>>;

/// Server-only source identity. It retains typed keys, never source snapshots.
#[doc(hidden)]
#[derive(Clone)]
pub struct SubscriptionEventSource<B: OrmBackend = DefaultBackend> {
    entity: &'static str,
    id: String,
    keys: Vec<SqlValue>,
    relation: &'static str,
    source_field: &'static str,
    check: SourceCheck<B>,
}
impl<B: OrmBackend> std::fmt::Debug for SubscriptionEventSource<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubscriptionEventSource([redacted])")
    }
}
impl<B: OrmBackend> SubscriptionEventSource<B> {
    /// Constructed by generated propagation code from a complete typed key.
    pub fn new<T>(keys: Vec<SqlValue>, relation: &'static str, source_field: &'static str) -> Self
    where
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        let id = crate::graphql::loaders::RelationKey::from_sql_values(&keys)
            .parts()
            .first()
            .cloned()
            .unwrap_or_default();
        Self {
            entity: T::entity_name(),
            id,
            keys,
            relation,
            source_field,
            check: check_source::<T, B>,
        }
    }
}

/// Trusted host emission envelope used by generated code.
///
/// The public constructor supports external macro expansions; it does not
/// authenticate untrusted events. Host code constructing witnesses is a trusted
/// server boundary. Raw/deserialized public events on the ordinary observer
/// channel cannot enter generated GraphQL delivery.
#[doc(hidden)]
#[derive(Clone)]
pub struct SubscriptionEvent<E: Clone, B: OrmBackend = DefaultBackend> {
    /// Original event, shared without cloning its payload for rejected subscribers.
    pub event: Arc<E>,
    /// Complete target key retained by the generated emitter.
    pub keys: Vec<SqlValue>,
    /// Whether a generated delete supplied its trusted final preimage.
    pub trusted_delete: bool,
    sources: Vec<SubscriptionEventSource<B>>,
}
impl<E: Clone, B: OrmBackend> std::fmt::Debug for SubscriptionEvent<E, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubscriptionEvent([redacted])")
    }
}
impl<E: Clone, B: OrmBackend> SubscriptionEvent<E, B> {
    /// Construct a generated emission with at most sixteen source hops.
    pub fn new(
        event: E,
        keys: Vec<SqlValue>,
        trusted_delete: bool,
        sources: Vec<SubscriptionEventSource<B>>,
    ) -> Option<Self> {
        (sources.len() <= MAX_SUBSCRIPTION_PROVENANCE_HOPS).then(|| Self {
            event: Arc::new(event),
            keys,
            trusted_delete,
            sources,
        })
    }
    /// Check every source and require the public provenance to match its witness.
    pub async fn authorize_provenance(
        &self,
        db: &Database<B>,
        ctx: &Context<'_>,
        source_entity: Option<&str>,
        source_id: Option<&str>,
        path: &[String],
        propagated: bool,
    ) -> async_graphql::Result<()> {
        if propagated == self.sources.is_empty() {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        if self.sources.is_empty() {
            if source_entity.is_some() || source_id.is_some() || !path.is_empty() {
                return Err(OrmPublicError::forbidden().into_graphql_error());
            }
            return Ok(());
        }
        if self.sources.len() != path.len()
            || self.sources.len() > MAX_SUBSCRIPTION_PROVENANCE_HOPS
            || source_entity != Some(self.sources[0].entity)
            || source_id != Some(self.sources[0].id.as_str())
        {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        for (index, (source, relation)) in self.sources.iter().zip(path).enumerate() {
            if source.relation != relation {
                return Err(OrmPublicError::forbidden().into_graphql_error());
            }
            let target_keys = self
                .sources
                .get(index + 1)
                .map_or(self.keys.as_slice(), |next| next.keys.as_slice());
            (source.check)(
                db,
                ctx,
                &source.keys,
                target_keys,
                source.relation,
                source.source_field,
            )
            .await?;
        }
        Ok(())
    }
}

/// Resolve and authorize one current target, allowing only explicitly witnessed
/// deleted preimages under callback/unrestricted visibility. SQL-only decisions
/// cannot be evaluated against a missing row and therefore suppress that event.
#[doc(hidden)]
pub async fn authorize_subscription_row<T, B>(
    db: &Database<B>,
    ctx: &Context<'_>,
    keys: &[SqlValue],
    deleted_preimage: Option<&T>,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    authorize_subscription_row_matching(db, ctx, keys, deleted_preimage, None).await
}
async fn authorize_subscription_row_matching<T, B>(
    db: &Database<B>,
    ctx: &Context<'_>,
    keys: &[SqlValue],
    deleted_preimage: Option<&T>,
    link: Option<(&str, &SqlValue)>,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    db.ensure_entity_access(
        Some(ctx),
        T::entity_name(),
        T::metadata().read_policy,
        EntityAccessKind::Read,
        EntityAccessSurface::GraphqlSubscription,
    )
    .await?;
    if keys.len() != T::PRIMARY_KEYS.len() || keys.is_empty() {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    let visibility = db
        .read_visibility::<T>(Some(ctx), EntityAccessSurface::GraphqlSubscription)
        .await?;
    let mut query = EntityQuery::<T, B>::new();
    for (column, value) in T::PRIMARY_KEYS.iter().zip(keys) {
        // PRIMARY_KEYS are trusted backend-rendered column identities.
        query = query.where_clause(&format!("{column} = ?"), value.clone());
    }
    if let Some((column, value)) = link {
        query = query.where_clause(
            &format!("{} = ?", B::DIALECT.quote_identifier_path(column)),
            value.clone(),
        );
    }
    query = query
        .with_read_visibility(&visibility)?
        .paginate(&crate::graphql::orm::PageInput {
            limit: Some(2),
            offset: Some(0),
        });
    let auth = ctx.data_opt::<DbAuthContext>();
    let mut rows = query
        .fetch_all_with_auth(db, auth)
        .await
        .map_err(graphql_error_from_sqlx)?;
    if rows.len() > 1 {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    let row = if let Some(row) = rows.pop() {
        Some(row)
    } else if matches!(
        visibility,
        ReadVisibility::CallbackOnly | ReadVisibility::Unrestricted
    ) {
        deleted_preimage.cloned()
    } else {
        None
    };
    if let Some(row) = &row
        && visibility.requires_residual_checks()
        && !db
            .can_read_row(
                Some(ctx),
                T::entity_name(),
                T::metadata().read_policy,
                EntityAccessSurface::GraphqlSubscription,
                row,
            )
            .await?
    {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    Ok(row)
}

/// Check key-field authority before an event identifier or source is released.
#[doc(hidden)]
pub async fn authorize_subscription_keys<T, B>(
    db: &Database<B>,
    ctx: &Context<'_>,
    row: &T,
) -> async_graphql::Result<()>
where
    B: OrmBackend,
    T: Entity + Send + Sync + 'static,
{
    for key in T::PRIMARY_KEYS {
        let column = T::columns()
            .iter()
            .find(|column| {
                column.name == *key || B::DIALECT.quote_identifier_path(column.name) == *key
            })
            .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
        check_field::<T, B>(db, ctx, row, column.api_name).await?;
    }
    Ok(())
}
async fn check_field<T, B>(
    db: &Database<B>,
    ctx: &Context<'_>,
    row: &T,
    field: &str,
) -> async_graphql::Result<()>
where
    B: OrmBackend,
    T: Entity + Send + Sync + 'static,
{
    let (api_name, allowed, policy) = T::subscription_field_policy(field)
        .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
    if !allowed {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    db.ensure_readable_field(ctx, T::entity_name(), api_name, policy, Some(row))
        .await
}
fn check_source<'a, 'ctx, T, B>(
    db: &'a Database<B>,
    ctx: &'a Context<'ctx>,
    keys: &'a [SqlValue],
    target_keys: &'a [SqlValue],
    relation: &'static str,
    source_field: &'static str,
) -> BoxFuture<'a, async_graphql::Result<()>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    Box::pin(async move {
        let column = T::columns()
            .iter()
            .find(|c| c.rust_name == source_field)
            .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
        // Generated upward propagation supports a single-column target key.
        if target_keys.len() != 1 {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        let row = authorize_subscription_row_matching::<T, B>(
            db,
            ctx,
            keys,
            None,
            Some((column.name, &target_keys[0])),
        )
        .await?
        .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
        authorize_subscription_keys::<T, B>(db, ctx, &row).await?;
        check_field::<T, B>(db, ctx, &row, column.api_name).await?;
        check_field::<T, B>(db, ctx, &row, relation).await
    })
}

fn unsupported_subscription_selection() -> async_graphql::Error {
    OrmPublicError::with_message(crate::graphql::errors::OrmErrorCode::InvalidInput,
        "Subscription payloads support persisted subscribable scalar fields only; refresh authorized queries for relations or computed fields").into_graphql_error()
}

/// Reject unsupported nested/computed payload shapes before registration.
#[doc(hidden)]
pub fn validate_subscription_selection<T: Entity, B: OrmBackend>(
    ctx: &Context<'_>,
    entity_field: &str,
) -> async_graphql::Result<()> {
    for selected in
        crate::graphql::loaders::relation_selected_fields(ctx, &ctx.item.node.selection_set.node)
    {
        if selected.node.name.node.as_str().eq_ignore_ascii_case("id") {
            for key in T::PRIMARY_KEYS {
                let available = T::columns().iter().find(|column|
                    column.name == *key || B::DIALECT.quote_identifier_path(column.name) == *key
                ).and_then(|column| T::subscription_field_policy(column.api_name))
                    .is_some_and(|(_, available, _)| available);
                if !available { return Err(unsupported_subscription_selection()); }
            }
        }
        if selected.node.name.node.as_str() != entity_field {
            continue;
        }
        for field in crate::graphql::loaders::relation_selected_fields(
            ctx,
            &selected.node.selection_set.node,
        ) {
            let name = field.node.name.node.as_str();
            if name == "__typename" {
                continue;
            }
            if !field.node.selection_set.node.items.is_empty()
                || !T::columns().iter().any(|column| column.api_name == name)
                || !T::subscription_field_policy(name).is_some_and(|(_, available, _)| available)
            {
                return Err(unsupported_subscription_selection());
            }
        }
    }
    Ok(())
}

/// Preflight selected event identifiers and entity scalars before any metadata.
#[doc(hidden)]
pub async fn authorize_subscription_fields<T, B>(
    db: &Database<B>,
    ctx: &Context<'_>,
    row: &T,
    entity_field: &str,
) -> async_graphql::Result<()>
where
    B: OrmBackend,
    T: Entity + Send + Sync + 'static,
{
    for selected in
        crate::graphql::loaders::relation_selected_fields(ctx, &ctx.item.node.selection_set.node)
    {
        let field_ctx = ctx.with_field(selected);
        if selected.node.name.node.as_str().eq_ignore_ascii_case("id") {
            authorize_subscription_keys::<T, B>(db, &field_ctx, row).await?;
        } else if selected.node.name.node.as_str() == entity_field {
            for scalar in crate::graphql::loaders::relation_selected_fields(
                ctx,
                &selected.node.selection_set.node,
            ) {
                if scalar.node.name.node.as_str() == "__typename" {
                    continue;
                }
                // Nested relation/custom resolvers can fail after outer metadata
                // has been written. Only persisted scalar snapshots are proved here.
                if !scalar.node.selection_set.node.items.is_empty()
                    || !T::columns()
                    .iter()
                    .any(|column| column.api_name == scalar.node.name.node.as_str())
                {
                    return Err(OrmPublicError::forbidden().into_graphql_error());
                }
                let scalar_ctx = field_ctx.with_field(scalar);
                check_field::<T, B>(db, &scalar_ctx, row, scalar.node.name.node.as_str()).await?;
            }
        }
    }
    Ok(())
}
