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
    deleted_origin: bool,
}
impl<B: OrmBackend> std::fmt::Debug for SubscriptionEventSource<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubscriptionEventSource([redacted])")
    }
}
impl<B: OrmBackend> SubscriptionEventSource<B> {
    /// Identity of one trusted provenance hop for path deduplication.
    #[doc(hidden)]
    pub fn propagation_identity(&self) -> (&'static str, String, &'static str) {
        (
            self.entity,
            crate::graphql::orm::composite_key_id(&self.keys),
            self.relation,
        )
    }

    /// Constructed by generated propagation code from a complete typed key.
    pub fn new<T>(
        keys: Vec<SqlValue>,
        relation: &'static str,
        source_field: &'static str,
        operation_authority: for<'ctx> fn(&Context<'ctx>) -> async_graphql::Result<()>,
    ) -> Self
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
            deleted_origin: false,
        }
    }

    /// Capture the first surviving parent of a transactional delete.
    /// No deleted key or snapshot is retained; delivery requires current parent access.
    pub fn new_delete_target<T>(
        keys: Vec<SqlValue>,
        relation: &'static str,
        operation_authority: for<'ctx> fn(&Context<'ctx>) -> async_graphql::Result<()>,
    ) -> Self
    where
        T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    {
        let mut source = Self::new::<T>(keys, relation, "", operation_authority);
        source.deleted_origin = true;
        source.check = check_delete_target::<T, B>;
        source
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
            || if self.sources[0].deleted_origin {
                source_entity.is_some() || source_id.is_some()
            } else {
                source_entity != Some(self.sources[0].entity)
                    || source_id != Some(self.sources[0].id.as_str())
            }
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
        for source in &self.sources {
            (source.operation_authority)(ctx)?;
        }
        Ok(())
    }
}

fn is_misconfigured(error: &async_graphql::Error) -> bool {
    error
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.get("code"))
        .is_some_and(|code| {
            *code
                == async_graphql::Value::from(
                    crate::graphql::errors::OrmErrorCode::AuthorizationMisconfigured.as_str(),
                )
        })
}

/// Generated `Changed` subscriptions drop an event whose authorization fails,
/// so a misconfigured entity would otherwise never deliver one, silently.
/// Report the first such entity per process on standard error.
fn warn_undeliverable_changed_events(entity_name: &'static str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "graphql-orm warning: Changed subscription events for entity {entity_name} are being \
             dropped because its read decision reported an authorization misconfiguration. See \
             the strict-authorization reference (docs/reference/graphql-orm/strict-authorization.md). \
             Reported once per process."
        );
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
    .await
    .map_err(crate::graphql::errors::graphql_error_from_callback)?;
    if keys.len() != T::PRIMARY_KEYS.len() || keys.is_empty() {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    let visibility = db
        .read_visibility::<T>(Some(ctx), EntityAccessSurface::GraphqlSubscription)
        .await
        .map_err(crate::graphql::errors::graphql_error_from_callback)
        .inspect_err(|error| {
            if is_misconfigured(error) {
                warn_undeliverable_changed_events(T::entity_name());
            }
        })?;
    if db.row_policy().is_none()
        && T::metadata().read_policy.is_some()
        && db.authorization_mode()
            == crate::graphql::auth::AuthorizationMode::ExplicitPolicyForAllExposedOperations
    {
        // Every row defers to `can_read_row`, which refuses without a provider.
        warn_undeliverable_changed_events(T::entity_name());
    }
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
            .await
            .map_err(crate::graphql::errors::graphql_error_from_callback)?
    {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    Ok(row)
}

/// Complete keys a generated `Changed` subscription observes through `Keys`.
#[doc(hidden)]
pub type SubscriptionKeySet = Arc<std::collections::HashSet<crate::graphql::loaders::RelationKey>>;

/// Admit the `Key` and `Keys` arguments of a generated `Changed` subscription.
///
/// Supplying both, or more keys than [`Database::subscription_key_limit`], is
/// `INVALID_INPUT`. The bound applies to the list as sent; duplicates are then
/// ignored. An empty list matches nothing. Admission never reads rows, so it
/// cannot reveal whether a key exists or is readable.
#[doc(hidden)]
pub fn admit_subscription_keys<B: OrmBackend>(
    db: &Database<B>,
    has_key: bool,
    keys: Option<Vec<Vec<SqlValue>>>,
) -> async_graphql::Result<Option<SubscriptionKeySet>> {
    let Some(keys) = keys else {
        return Ok(None);
    };
    admit_subscription_key_count(has_key, keys.len(), db.subscription_key_limit())?;
    Ok(Some(Arc::new(
        keys.iter()
            .map(|values| crate::graphql::loaders::RelationKey::from_sql_values(values))
            .collect(),
    )))
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
fn check_delete_target<'a, 'ctx, T, B>(
    db: &'a Database<B>,
    ctx: &'a Context<'ctx>,
    keys: &'a [SqlValue],
    _: &'a [SqlValue],
    _: &'static str,
    _: &'static str,
) -> BoxFuture<'a, async_graphql::Result<()>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    Box::pin(async move {
        let row = authorize_subscription_row::<T, B>(db, ctx, keys, None)
            .await?
            .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
        authorize_subscription_keys::<T, B>(db, ctx, &row).await
    })
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
            let name = persisted_field_name::<T>(field.node.name.node.as_str());
            if name == "__typename" {
                continue;
            }
            if !field.node.selection_set.node.items.is_empty()
                || if is_node_identity::<T>(name) {
                    !subscription_keys_available::<T, B>()
                } else {
                    !T::columns().iter().any(|column| column.api_name == name)
                        || !T::subscription_field_policy(name)
                            .is_some_and(|(_, available, _)| available)
                }
            {
                return Err(unsupported_subscription_selection());
            }
        }
    }
    Ok(())
}

// Only a generated NodeId opts into this path; arbitrary computed fields remain refused.
fn is_node_identity<T: Entity>(name: &str) -> bool {
    name == "NodeId" && T::subscription_field_policy(name) == Some(("__gom_node_id", true, None))
}

fn subscription_keys_available<T: Entity, B: OrmBackend>() -> bool {
    T::PRIMARY_KEYS.iter().all(|key| {
        T::columns()
            .iter()
            .find(|column| {
                column.name == *key || B::DIALECT.quote_identifier_path(column.name) == *key
            })
            .and_then(|column| T::subscription_field_policy(column.api_name))
            .is_some_and(|(_, available, _)| available)
    })
}

/// The persisted field behind a selected name: the fixed record version name
/// stands for the declared managed version field.
fn persisted_field_name<T: Entity>(name: &str) -> &str {
    match T::managed_version_field() {
        Some(field) if name == crate::graphql::orm::RECORD_VERSION_FIELD => field,
        _ => name,
    }
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
                let name = persisted_field_name::<T>(scalar.node.name.node.as_str());
                if name == "__typename" {
                    continue;
                }
                let scalar_ctx = field_ctx.with_field(scalar);
                if is_node_identity::<T>(name) {
                    if !scalar.node.selection_set.node.items.is_empty() {
                        return Err(OrmPublicError::forbidden().into_graphql_error());
                    }
                    authorize_subscription_keys::<T, B>(db, &scalar_ctx, row).await?;
                    continue;
                }
                // Nested relation/custom resolvers can fail after outer metadata
                // has been written. Only persisted scalar snapshots are proved here.
                if !scalar.node.selection_set.node.items.is_empty()
                    || !T::columns().iter().any(|column| column.api_name == name)
                {
                    return Err(OrmPublicError::forbidden().into_graphql_error());
                }
                check_field::<T, B>(db, &scalar_ctx, row, name).await?;
            }
        }
    }
    Ok(())
}

/// `extensions.code` of the error that ends a generated `Changed` stream whose
/// subscriber fell further behind than the event channel holds. The client
/// must subscribe again, then refetch the state it shows; events were missed.
pub const SUBSCRIPTION_CONTINUITY_LOST: &str = "SUBSCRIPTION_CONTINUITY_LOST";

/// `extensions.code` of the error that ends a generated `Changed` stream whose
/// event source closed, for example because the channel was replaced.
pub const SUBSCRIPTION_SOURCE_CLOSED: &str = "SUBSCRIPTION_SOURCE_CLOSED";

pub(crate) fn subscription_end(code: &'static str, message: &'static str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    async_graphql::Error::new(message).extend_with(|_, extensions| extensions.set("code", code))
}

/// Events of one generated subscription's channel, checked for continuity
/// before any filter.
///
/// Every event is passed on in order. When the subscriber fell behind and the
/// channel dropped events, or the channel closed, the stream yields exactly one
/// error and ends. Neither error carries a skipped-event count, identifier or
/// payload. A lag error's occurrence and timing still reveal that the shared channel
/// exceeded this receiver's capacity, which hidden writes can cause. Lag is
/// checked before authorization and can end a revoked stream before its next
/// independent authority check.
#[doc(hidden)]
pub fn continuity_events<T>(
    receiver: tokio::sync::broadcast::Receiver<T>,
) -> impl futures::Stream<Item = async_graphql::Result<T>> + Send + 'static
where
    T: Clone + Send + 'static,
{
    use futures::StreamExt;
    use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};
    futures::stream::unfold(Some(BroadcastStream::new(receiver)), |events| async move {
        let mut events = events?;
        match events.next().await {
            Some(Ok(event)) => Some((Ok(event), Some(events))),
            Some(Err(BroadcastStreamRecvError::Lagged(_))) => Some((
                Err(subscription_end(
                    SUBSCRIPTION_CONTINUITY_LOST,
                    "subscription lost event continuity; subscribe again, then refetch",
                )),
                None,
            )),
            None => Some((
                Err(subscription_end(
                    SUBSCRIPTION_SOURCE_CLOSED,
                    "subscription event source closed; subscribe again, then refetch",
                )),
                None,
            )),
        }
    })
}

/// Shared admission before deduplication or row reads.
pub(crate) fn admit_subscription_key_count(
    has_key: bool,
    count: usize,
    limit: usize,
) -> async_graphql::Result<()> {
    if has_key || count > limit {
        return Err(
            OrmPublicError::new(crate::graphql::errors::OrmErrorCode::InvalidInput)
                .into_graphql_error(),
        );
    }
    Ok(())
}

pub use super::subscription_admission::GeneratedSubscriptionAdmission;
#[doc(hidden)]
pub use super::subscription_admission::admit_generated_operation;

fn authority_recheck_delay(interval: std::time::Duration) -> std::time::Duration {
    let interval = interval.clamp(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(300),
    );
    let jitter_ns = interval.as_nanos() as u64 / 10;
    let sample = uuid::Uuid::new_v4().as_u128() as u64;
    interval + std::time::Duration::from_nanos(sample % (jitter_ns + 1))
}

fn authority_check_end(error: async_graphql::Error) -> async_graphql::Error {
    let code = error
        .extensions
        .as_ref()
        .and_then(|ext| ext.get("code"))
        .and_then(|value| {
            if let async_graphql::Value::String(code) = value {
                Some(code.as_str())
            } else {
                None
            }
        });
    // Preserve the callback authoring convention: source-free authored refusals
    // are denials. Typed host failures and infrastructure categories are not.
    // agql-auth deliberately strips sources in ErrorExtensions::extend, so its
    // public service/store and host-configuration codes must be recognized here.
    let denial = error.source.is_none()
        && !matches!(
            code,
            Some(
                "INTERNAL_SERVER_ERROR"
                    | "AUTH_SERVICE_UNAVAILABLE"
                    | "INVALID_CONFIGURATION"
                    | SUBSCRIPTION_CONTINUITY_LOST
                    | SUBSCRIPTION_SOURCE_CLOSED
            )
        )
        && match code.and_then(super::errors::public_code) {
            Some(
                super::errors::OrmErrorCode::Forbidden
                | super::errors::OrmErrorCode::Unauthenticated,
            )
            | None => true,
            Some(_) => false,
        };
    if denial {
        OrmPublicError::forbidden().into_graphql_error()
    } else {
        subscription_end(
            SUBSCRIPTION_CONTINUITY_LOST,
            "subscription authority check failed; subscribe again, then refetch",
        )
    }
}

/// Check subscriber-level authority on an event-independent, jittered schedule.
///
/// Event filtering must never yield a forbidden item. Only this timer converts
/// a definite subscriber denial to FORBIDDEN; host failures lose continuity.
/// The deadline is established before polling the source and is never reset by
/// events. A due timer takes priority over the source, including an in-flight
/// asynchronous event filter. As with every stream, progress requires polling.
#[doc(hidden)]
pub fn watch_subscription_authority<S, T, A, F>(
    stream: S,
    interval: std::time::Duration,
    authorize: A,
) -> impl futures::Stream<Item = async_graphql::Result<T>>
where
    S: futures::Stream<Item = async_graphql::Result<T>>,
    A: FnMut() -> F,
    F: std::future::Future<Output = async_graphql::Result<()>>,
{
    watch_authority_with_delay(stream, authorize, move || authority_recheck_delay(interval))
}

pub(crate) fn watch_authority_with_delay<S, T, A, F, D>(
    stream: S,
    authorize: A,
    mut next_delay: D,
) -> impl futures::Stream<Item = async_graphql::Result<T>>
where
    S: futures::Stream<Item = async_graphql::Result<T>>,
    A: FnMut() -> F,
    F: std::future::Future<Output = async_graphql::Result<()>>,
    D: FnMut() -> std::time::Duration,
{
    use futures::{
        StreamExt,
        future::{Either, select},
    };
    let deadline = tokio::time::Instant::now() + next_delay();
    futures::stream::unfold(
        (
            Box::pin(stream),
            Box::pin(tokio::time::sleep_until(deadline)),
            authorize,
            next_delay,
            false,
        ),
        move |(mut stream, mut timer, mut authorize, mut next_delay, ended)| async move {
            if ended {
                return None;
            }
            loop {
                match select(timer.as_mut(), stream.next()).await {
                    Either::Left(((), _)) => {
                        if let Err(error) = authorize().await {
                            return Some((
                                Err(authority_check_end(error)),
                                (stream, timer, authorize, next_delay, true),
                            ));
                        }
                        timer
                            .as_mut()
                            .reset(tokio::time::Instant::now() + next_delay());
                    }
                    Either::Right((item, _)) => {
                        let item = item?;
                        let ended = item.is_err();
                        return Some((item, (stream, timer, authorize, next_delay, ended)));
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod authority_timer_tests {
    use super::*;
    use futures::{StreamExt, future::poll_fn};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::Poll,
        time::Duration,
    };

    #[tokio::test(start_paused = true)]
    async fn hidden_writes_cannot_change_the_independent_jittered_deadline() {
        let deadline = Duration::from_millis(1037); // 1s plus a controlled 37ms jitter sample.
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
        let hidden = tokio_stream::wrappers::UnboundedReceiverStream::new(receiver)
            .filter_map(|_| futures::future::ready(None::<async_graphql::Result<()>>));
        let idle = futures::stream::pending::<async_graphql::Result<()>>();
        let checks = Arc::new(AtomicUsize::new(0));
        let make_check = || {
            let checks = checks.clone();
            move || {
                checks.fetch_add(1, Ordering::SeqCst);
                futures::future::ready(Err(OrmPublicError::forbidden().into_graphql_error()))
            }
        };
        let mut busy = Box::pin(watch_authority_with_delay(
            hidden,
            make_check(),
            move || deadline,
        ));
        let mut quiet = Box::pin(watch_authority_with_delay(idle, make_check(), move || {
            deadline
        }));
        async fn pending<S: futures::Stream + Unpin>(stream: &mut S) {
            poll_fn(|cx| match stream.poll_next_unpin(cx) {
                Poll::Pending => Poll::Ready(()),
                Poll::Ready(_) => panic!("an event must not trigger authority termination"),
            })
            .await;
        }
        pending(&mut busy).await;
        pending(&mut quiet).await;
        for _ in 0..103 {
            sender.send(()).unwrap();
            pending(&mut busy).await;
            tokio::time::advance(Duration::from_millis(10)).await;
            pending(&mut quiet).await;
        }
        assert_eq!(checks.load(Ordering::SeqCst), 0);
        tokio::time::advance(Duration::from_millis(6)).await;
        pending(&mut busy).await;
        pending(&mut quiet).await;
        assert_eq!(checks.load(Ordering::SeqCst), 0);
        // Queue an event at the deadline too: the timer still wins.
        sender.send(()).unwrap();
        tokio::time::advance(Duration::from_millis(1)).await;
        for stream in [
            &mut busy as &mut (dyn futures::Stream<Item = async_graphql::Result<()>> + Unpin),
            &mut quiet as &mut (dyn futures::Stream<Item = async_graphql::Result<()>> + Unpin),
        ] {
            let error = stream.next().await.unwrap().unwrap_err();
            assert_eq!(error.message, "forbidden");
            assert!(stream.next().await.is_none());
        }
        assert_eq!(checks.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_authority_codes_remain_definite_denials() {
        use async_graphql::ErrorExtensions;
        let mut stream = Box::pin(watch_subscription_authority(
            futures::stream::pending::<async_graphql::Result<()>>(),
            Duration::from_secs(1),
            || async {
                Err(
                    async_graphql::Error::new("private refusal").extend_with(|_, ext| {
                        ext.set("code", "UNKNOWN_HOST_CODE");
                        ext.set("private", "must not escape");
                    }),
                )
            },
        ));
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.message, "forbidden");
        assert!(error.source.is_none());
        assert_eq!(
            error.extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("FORBIDDEN"))
        );
        assert!(error.extensions.unwrap().get("private").is_none());
        assert!(stream.next().await.is_none());
    }

    #[cfg(feature = "auth-agql")]
    #[tokio::test(start_paused = true)]
    async fn agql_auth_infrastructure_errors_end_changed_timer_with_continuity_loss() {
        use agql_auth::AuthError;
        use async_graphql::ErrorExtensions;
        // Audit of AuthError::public_code at locked revision 1f4b4bfb:
        // all infrastructure/store variants use AUTH_SERVICE_UNAVAILABLE;
        // host configuration variants use INVALID_CONFIGURATION.
        for failure in [
            AuthError::AuthServiceUnavailable,
            AuthError::Store("private store diagnostic".into()),
            AuthError::OidcDiscovery("private discovery diagnostic".into()),
            AuthError::OidcTokenExchange("private OIDC diagnostic".into()),
            AuthError::OAuthTokenExchange("private OAuth diagnostic".into()),
            AuthError::OAuthUserInfo("private profile diagnostic".into()),
            AuthError::TokenCreation("private signing diagnostic".into()),
            AuthError::PasswordHashing("private hashing diagnostic".into()),
            AuthError::InvalidConfiguration("private configuration".into()),
            AuthError::Config("private legacy configuration".into()),
            AuthError::InvalidOidcAuthorizationOptions("private options".into()),
            AuthError::JwksUnsupported,
        ] {
            assert!(failure.extend().source.is_none());
            let mut stream = Box::pin(watch_subscription_authority(
                futures::stream::pending::<async_graphql::Result<()>>(),
                Duration::from_secs(1),
                || futures::future::ready(Err(failure.extend())),
            ));
            let started = tokio::time::Instant::now();
            let error = stream.next().await.unwrap().unwrap_err();
            // Allow millisecond rounding by Tokio's timer driver.
            assert!(
                (Duration::from_secs(1)..=Duration::from_millis(1101)).contains(&started.elapsed())
            );
            assert_eq!(
                error.extensions.as_ref().unwrap().get("code"),
                Some(&async_graphql::Value::from(SUBSCRIPTION_CONTINUITY_LOST)),
                "{failure:?}"
            );
            assert!(error.source.is_none());
            assert!(!format!("{error:?}").contains("private"));
            assert!(stream.next().await.is_none());
        }
    }
}
