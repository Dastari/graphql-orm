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

/// A selected Changed payload prepared before the stream releases an item.
///
/// A nullable scalar guard can fail after authorization preflight. Resolve the
/// selected snapshot inside the generated filter, discard every failed payload
/// and its private diagnostics, and yield only a completed value. The GraphQL
/// event identity and internal observer DTO remain unchanged.
#[doc(hidden)]
pub struct ChangedPayload<T> {
    value: async_graphql::Value,
    type_name: std::borrow::Cow<'static, str>,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T: async_graphql::OutputType> ChangedPayload<T> {
    /// Prepare one selected event; failures never become subscription frames.
    pub async fn prepare(event: T, ctx: &Context<'_>) -> Option<Self> {
        let errors_before = ctx.query_env.errors.lock().unwrap().len();
        let type_name = event.introspection_type_name();
        let selection = ctx.with_selection_set(&ctx.item.node.selection_set);
        let resolved = event.resolve(&selection, ctx.item).await;
        let field_failed = {
            let mut errors = ctx.query_env.errors.lock().unwrap();
            let failed = errors.len() > errors_before;
            if failed {
                errors.truncate(errors_before);
            }
            failed
        };
        if field_failed {
            return None;
        }
        resolved.ok().map(|value| Self {
            value,
            type_name,
            marker: std::marker::PhantomData,
        })
    }
}

impl<T: async_graphql::OutputType> async_graphql::OutputType for ChangedPayload<T> {
    fn type_name() -> std::borrow::Cow<'static, str> {
        T::type_name()
    }

    fn qualified_type_name() -> String {
        T::qualified_type_name()
    }

    fn introspection_type_name(&self) -> std::borrow::Cow<'static, str> {
        self.type_name.clone()
    }

    fn create_type_info(registry: &mut async_graphql::registry::Registry) -> String {
        T::create_type_info(registry)
    }

    async fn resolve(
        &self,
        _ctx: &async_graphql::ContextSelectionSet<'_>,
        _field: &async_graphql::Positioned<async_graphql::parser::types::Field>,
    ) -> async_graphql::ServerResult<async_graphql::Value> {
        Ok(self.value.clone())
    }
}

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

type RelationSourceCheck<B> = for<'a, 'ctx> fn(
    &'a Database<B>,
    &'a Context<'ctx>,
    &'a [SqlValue],
    &'a [SqlValue],
    &'static str,
    &'a [(&'static str, &'static str)],
) -> BoxFuture<'a, async_graphql::Result<()>>;

/// Server-only source identity. It retains typed keys, never source snapshots.
#[doc(hidden)]
#[derive(Clone)]
pub struct SubscriptionEventSource<B: OrmBackend = DefaultBackend> {
    // Monomorphized by the source expansion; no client data or root-scope inheritance.
    operation_authority: for<'ctx> fn(&Context<'ctx>) -> async_graphql::Result<()>,
    entity: &'static str,
    id: String,
    keys: Vec<SqlValue>,
    relation: &'static str,
    source_field: &'static str,
    relation_fields: Vec<(&'static str, &'static str)>,
    relation_check: Option<RelationSourceCheck<B>>,
    check: SourceCheck<B>,
}
impl<B: OrmBackend> std::fmt::Debug for SubscriptionEventSource<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubscriptionEventSource([redacted])")
    }
}
impl<B: OrmBackend> SubscriptionEventSource<B> {
    /// Constructed by generated propagation code from a complete typed key.
    pub fn new<T>(keys: Vec<SqlValue>, relation: &'static str, source_field: &'static str,
        operation_authority: for<'ctx> fn(&Context<'ctx>) -> async_graphql::Result<()>) -> Self
    where
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        let id = if keys.len() > 1 {
            crate::graphql::orm::composite_key_id(&keys)
        } else {
            crate::graphql::loaders::RelationKey::from_sql_values(&keys)
                .parts()
                .first()
                .cloned()
                .unwrap_or_default()
        };
        Self {
            operation_authority,
            entity: T::entity_name(),
            id,
            keys,
            relation,
            source_field,
            relation_fields: Vec::new(),
            relation_check: None,
            check: check_source::<T, B>,
        }
    }

    /// Bind an upward relation to its complete source/target column mapping.
    /// Both types and field names come from generated server code, never clients.
    pub fn new_relation<S, T>(
        keys: Vec<SqlValue>,
        relation: &'static str,
        fields: Vec<(&'static str, &'static str)>,
        operation_authority: for<'ctx> fn(&Context<'ctx>) -> async_graphql::Result<()>,
    ) -> Self
    where
        S: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        let mut source = Self::new::<S>(keys, relation, "", operation_authority);
        source.relation_fields = fields;
        source.relation_check = Some(check_relation_source::<S, T, B>);
        source
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
            (source.operation_authority)(ctx)?;
            if let Some(check) = source.relation_check {
                check(
                    db,
                    ctx,
                    &source.keys,
                    target_keys,
                    source.relation,
                    &source.relation_fields,
                )
                .await?;
            } else {
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
        }
        // Every typed row/relation check can await. Recheck all operation
        // requirements after the final await, not just the last source.
        for source in &self.sources { (source.operation_authority)(ctx)?; }
        Ok(())
    }
}

/// Resolve and authorize one current target, allowing only explicitly witnessed
/// deleted preimages only through an explicit default-deny preimage policy under
/// callback/unrestricted visibility. SQL-only decisions cannot be evaluated
/// against a missing row and therefore suppress that event. Unrestricted does
/// not imply RLS authority for a deleted snapshot.
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
    link: Option<(String, Vec<SqlValue>)>,
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
        query = query.where_clause(
            &format!("{} = ?", B::DIALECT.render_identifier_path(column)),
            value.clone(),
        );
    }
    if let Some((clause, values)) = link {
        query = query.where_values(&clause, values);
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
        if let Some(preimage) = deleted_preimage {
            if db
                .can_read_deleted_preimage(
                    Some(ctx),
                    T::entity_name(),
                    T::metadata().read_policy,
                    EntityAccessSurface::GraphqlSubscription,
                    preimage,
                )
                .await?
            {
                Some(preimage.clone())
            } else {
                None
            }
        } else {
            None
        }
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
            Some((
                format!("{} = ?", B::DIALECT.quote_identifier_path(column.name)),
                vec![target_keys[0].clone()],
            )),
        )
        .await?
        .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
        authorize_subscription_keys::<T, B>(db, ctx, &row).await?;
        check_field::<T, B>(db, ctx, &row, column.api_name).await?;
        check_field::<T, B>(db, ctx, &row, relation).await
    })
}

fn check_relation_source<'a, 'ctx, S, T, B>(
    db: &'a Database<B>,
    ctx: &'a Context<'ctx>,
    keys: &'a [SqlValue],
    target_keys: &'a [SqlValue],
    relation: &'static str,
    fields: &'a [(&'static str, &'static str)],
) -> BoxFuture<'a, async_graphql::Result<()>>
where
    B: OrmBackend,
    S: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    Box::pin(async move {
        let denied = || OrmPublicError::forbidden().into_graphql_error();
        if fields.is_empty()
            || fields.len() > S::columns().len()
            || target_keys.len() != T::PRIMARY_KEYS.len()
            || target_keys.is_empty()
        {
            return Err(denied());
        }
        let mut predicates = T::PRIMARY_KEYS
            .iter()
            .map(|column| {
                format!(
                    "__gom_propagation_target.{} = ?",
                    B::DIALECT.render_identifier_path(column)
                )
            })
            .collect::<Vec<_>>();
        let mut source_fields = Vec::new();
        let mut target_fields = Vec::new();
        for (source, target) in fields {
            let source = S::columns()
                .iter()
                .find(|column| column.rust_name == *source)
                .ok_or_else(denied)?;
            let target = T::columns()
                .iter()
                .find(|column| column.rust_name == *target)
                .ok_or_else(denied)?;
            predicates.push(format!(
                "__gom_propagation_target.{} = {}.{}",
                B::DIALECT.quote_identifier_path(target.name),
                B::DIALECT.quote_identifier_path(S::TABLE_NAME),
                B::DIALECT.quote_identifier_path(source.name)
            ));
            source_fields.push(source.api_name);
            target_fields.push(target.api_name);
        }
        let clause = format!(
            "EXISTS (SELECT 1 FROM {} AS __gom_propagation_target WHERE {})",
            B::DIALECT.quote_identifier_path(T::TABLE_NAME),
            predicates.join(" AND ")
        );
        let row = authorize_subscription_row_matching::<S, B>(
            db,
            ctx,
            keys,
            None,
            Some((clause, target_keys.to_vec())),
        )
        .await?
        .ok_or_else(denied)?;
        authorize_subscription_keys::<S, B>(db, ctx, &row).await?;
        for field in source_fields {
            check_field::<S, B>(db, ctx, &row, field).await?;
        }
        check_field::<S, B>(db, ctx, &row, relation).await?;
        let target = authorize_subscription_row::<T, B>(db, ctx, target_keys, None)
            .await?
            .ok_or_else(denied)?;
        authorize_subscription_keys::<T, B>(db, ctx, &target).await?;
        for field in target_fields {
            check_field::<T, B>(db, ctx, &target, field).await?;
        }
        Ok(())
    })
}

/// Read bounded propagation targets on the same pinned mutation transaction.
/// This grants no release authority: every hop is revalidated for each subscriber.
#[cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
#[doc(hidden)]
pub async fn propagation_targets<T, B>(
    context: &mut crate::graphql::orm::MutationContext<'_, B>,
    fields: &[&str],
    values: &[SqlValue],
    remaining: usize,
) -> crate::Result<Vec<T>>
where
    B: crate::graphql::orm::WriteBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    if fields.is_empty()
        || fields.len() != values.len()
        || remaining == 0
        || remaining > MAX_SUBSCRIPTION_PROPAGATION_EVENTS
    {
        return Err(sqlx::Error::Protocol(
            "invalid generated propagation tuple/budget".into(),
        ));
    }
    let mut query = EntityQuery::<T, B>::new();
    for (field, value) in fields.iter().zip(values) {
        let column = T::columns()
            .iter()
            .find(|column| column.rust_name == *field)
            .ok_or_else(|| {
                sqlx::Error::Protocol("unknown generated propagation target column".into())
            })?;
        query = query.where_clause(
            &format!("{} = ?", B::DIALECT.quote_identifier_path(column.name)),
            value.clone(),
        );
    }
    // One lookahead detects a fanout that cannot be represented completely.
    let rows = query
        .paginate(&crate::graphql::orm::PageInput {
            limit: Some(remaining.min(MAX_SUBSCRIPTION_PROPAGATION_EVENTS) as i64 + 1),
            offset: Some(0),
        })
        .fetch_all_in_transaction(context)
        .await?;
    if rows.len() > remaining {
        return Err(sqlx::Error::Protocol(
            "generated propagation target budget exceeded".into(),
        ));
    }
    Ok(rows)
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
        if selected.node.name.node.as_str().eq_ignore_ascii_case("id")
            || selected.node.name.node.as_str().eq_ignore_ascii_case("key")
        {
            for key in T::PRIMARY_KEYS {
                let available = T::columns()
                    .iter()
                    .find(|column| {
                        column.name == *key || B::DIALECT.quote_identifier_path(column.name) == *key
                    })
                    .and_then(|column| T::subscription_field_policy(column.api_name))
                    .is_some_and(|(_, available, _)| available);
                if !available {
                    return Err(unsupported_subscription_selection());
                }
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
        if selected.node.name.node.as_str().eq_ignore_ascii_case("id")
            || selected.node.name.node.as_str().eq_ignore_ascii_case("key")
        {
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
