//! Runtime for derive-generated Changed subscriptions.

use super::{
    ChangedPayload, SubscriptionEvent, SubscriptionKeySet, admit_generated_operation,
    admit_subscription_keys, authorize_subscription_fields, authorize_subscription_key_fields,
    authorize_subscription_keys, authorize_subscription_row, continuity_events,
    validate_subscription_selection, watch_subscription_authority,
};
use crate::db::{Database, request_database};
use crate::graphql::{
    assurance::enforce_resolver_assurance,
    errors::{OrmErrorCode, OrmPublicError, buffered_errors, graphql_error_from_callback},
    input_validation::require_generated_input_validation,
    loaders::{RelationKey, relation_authorization_error},
    orm::{
        ChangeAction, ChangeKind, Entity, EntityAccessKind, EntityAccessSurface, FromSqlRow,
        GraphqlOperationKind, OrmBackend, SqlValue, SubscriptionFilterInput,
    },
};
use async_graphql::{Context, OutputType, Result};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};

/// Borrowed event metadata used to verify the generated emission witness.
#[doc(hidden)]
pub struct ChangedEventView<'a, R> {
    pub action: ChangeAction,
    pub kind: ChangeKind,
    pub source_entity: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub path: &'a [String],
    pub entity: Option<&'a R>,
}

/// Entity-specific accessors and authority for a generated Changed subscription.
///
/// The derive supplies the resolver and operation authority callbacks. They
/// are trusted host code, as is the construction of emission witnesses.
#[doc(hidden)]
pub trait ChangedSubscriptionAdapter<B: OrmBackend>: 'static {
    type Event: Clone + Send + Sync + 'static;
    type Row: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static;
    type Key: Clone + Send + Sync + 'static;
    type Payload: OutputType + 'static;

    const ENTITY_NAME: &'static str;
    const ENTITY_FIELD: &'static str;
    const KEY_ONLY_DELETES: bool;

    fn key_values(key: &Self::Key) -> Vec<SqlValue>;
    fn row_key_values(row: &Self::Row) -> Vec<SqlValue>;
    fn event_key(event: &Self::Event) -> &Self::Key;
    fn view(event: &Self::Event) -> ChangedEventView<'_, Self::Row>;
    /// Build the authorized row payload, or a key-only delete when row is absent.
    fn payload(event: &Self::Event, row: Option<Self::Row>) -> Self::Payload;
    fn started_payload() -> Option<Self::Payload>;
    fn authorize_resolver(ctx: &Context<'_>) -> Result<()>;
    fn authorize_operation(ctx: &Context<'_>) -> Result<()>;
}

type ChangedStream<'a, T> = BoxStream<'a, Result<ChangedPayload<T>>>;

/// Open a generated Changed stream. The boxed boundary keeps the stream and
/// its authorization futures out of downstream resolver type checking.
#[doc(hidden)]
pub fn open_generated_changed<'a, B, E>(
    ctx: &'a Context<'_>,
    filter: Option<SubscriptionFilterInput>,
    key: Option<E::Key>,
    keys: Option<Vec<E::Key>>,
    started: bool,
) -> BoxFuture<'a, Result<ChangedStream<'a, E::Payload>>>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    Box::pin(async move {
        admit_generated_operation(ctx)?;
        require_generated_input_validation(ctx)?;
        E::authorize_resolver(ctx)?;
        let db = request_database::<B>(ctx)?;
        ensure_entity_access::<B, E>(db, ctx).await?;
        if filter.as_ref().is_some_and(|filter| filter.dummy.is_some()) {
            return Err(OrmPublicError::new(OrmErrorCode::InvalidInput).into_graphql_error());
        }
        validate_subscription_selection::<E::Row, B>(ctx, E::ENTITY_FIELD)?;
        let keys = admit_subscription_keys(
            db,
            key.is_some(),
            keys.map(|keys| keys.iter().map(E::key_values).collect()),
        )?;
        let actions = filter.and_then(|filter| filter.actions);
        let rx = db
            .ensure_event_sender::<SubscriptionEvent<E::Event, B>>()
            .subscribe();

        let events = continuity_events(rx).filter_map(move |result| {
            filter_event::<B, E>(db, ctx, result, key.clone(), keys.clone(), actions.clone())
        });
        let started_frame =
            futures::stream::iter(started.then_some(())).filter_map(move |()| async move {
                ChangedPayload::prepare(E::started_payload()?, ctx)
                    .await
                    .map(Ok)
            });
        let events = watch_subscription_authority(
            started_frame.chain(events),
            db.subscription_authority_recheck_interval(),
            move || check_stream_authority::<B, E>(db, ctx),
        );
        Ok(events.boxed())
    })
}

async fn ensure_entity_access<B: OrmBackend, E: ChangedSubscriptionAdapter<B>>(
    db: &Database<B>,
    ctx: &Context<'_>,
) -> Result<()> {
    db.ensure_entity_access(
        Some(ctx),
        E::ENTITY_NAME,
        E::Row::metadata().read_policy,
        EntityAccessKind::Read,
        EntityAccessSurface::GraphqlSubscription,
    )
    .await
    .map_err(graphql_error_from_callback)
}

async fn filter_event<B, E>(
    db: &Database<B>,
    ctx: &Context<'_>,
    result: Result<SubscriptionEvent<E::Event, B>>,
    key: Option<E::Key>,
    keys: Option<SubscriptionKeySet>,
    actions: Option<Vec<ChangeAction>>,
) -> Option<Result<ChangedPayload<E::Payload>>>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    // Lost continuity or a closed source yields one error and ends the stream.
    let witness = match result {
        Ok(witness) => witness,
        Err(error) => return Some(Err(error)),
    };
    // Cover all dropped events, including diagnostics from host policy checks.
    let errors_before = match buffered_errors::count(ctx) {
        Ok(count) => count,
        Err(error) => return Some(Err(error)),
    };
    let delivered = prepare_event::<B, E>(
        db,
        ctx,
        &witness,
        key.as_ref(),
        keys.as_deref(),
        actions.as_deref(),
    )
    .await;
    if delivered.is_none()
        && let Err(error) = buffered_errors::truncate(ctx, errors_before)
    {
        return Some(Err(error));
    }
    delivered.map(Ok)
}

async fn prepare_event<B, E>(
    db: &Database<B>,
    ctx: &Context<'_>,
    witness: &SubscriptionEvent<E::Event, B>,
    key: Option<&E::Key>,
    keys: Option<&std::collections::HashSet<RelationKey>>,
    actions: Option<&[ChangeAction]>,
) -> Option<ChangedPayload<E::Payload>>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    let event = witness.event.as_ref();
    let key_matches = RelationKey::from_sql_values(&witness.keys);
    if key.is_some_and(|key| RelationKey::from_sql_values(&E::key_values(key)) != key_matches)
        || keys.is_some_and(|keys| !keys.contains(&key_matches))
        || RelationKey::from_sql_values(&E::key_values(E::event_key(event))) != key_matches
    {
        return None;
    }
    let view = E::view(event);
    if actions.is_some_and(|actions| !actions.contains(&view.action)) {
        return None;
    }
    let exact_key = key.is_some() || keys.is_some();
    let authorize = || authorize_event::<B, E>(db, ctx, witness, &view, &key_matches, exact_key);
    match authorize().await {
        Ok(row) => {
            let prepared = ChangedPayload::prepare(E::payload(event, Some(row)), ctx).await?;
            // Serialization can await host code. Recheck the entire release contract.
            authorize()
                .await
                .map_err(relation_authorization_error)
                .ok()?;
            Some(prepared)
        }
        Err(error) => {
            let _ = relation_authorization_error(error);
            if !E::KEY_ONLY_DELETES
                || !exact_key
                || !witness.trusted_delete
                || view.action != ChangeAction::Deleted
                || view.kind != ChangeKind::Direct
            {
                return None;
            }
            let authorize = || authorize_key_only::<B, E>(db, ctx);
            authorize()
                .await
                .map_err(relation_authorization_error)
                .ok()?;
            let prepared = ChangedPayload::prepare(E::payload(event, None), ctx).await?;
            authorize()
                .await
                .map_err(relation_authorization_error)
                .ok()?;
            Some(prepared)
        }
    }
}

async fn authorize_event<B, E>(
    db: &Database<B>,
    ctx: &Context<'_>,
    witness: &SubscriptionEvent<E::Event, B>,
    view: &ChangedEventView<'_, E::Row>,
    key: &RelationKey,
    exact_key: bool,
) -> Result<E::Row>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    let authorize_provenance = || {
        witness.authorize_provenance(
            db,
            ctx,
            view.source_entity,
            view.source_id,
            view.path,
            view.kind == ChangeKind::Propagated,
        )
    };
    E::authorize_resolver(ctx)?;
    authorize_provenance().await?;
    if witness.trusted_delete
        && view
            .entity
            .is_some_and(|entity| RelationKey::from_sql_values(&E::row_key_values(entity)) != *key)
    {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    let preimage = if witness.trusted_delete && view.action == ChangeAction::Deleted {
        view.entity
    } else {
        None
    };
    let row = authorize_subscription_row::<E::Row, B>(db, ctx, &witness.keys, preimage)
        .await?
        .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
    if exact_key {
        authorize_subscription_keys::<E::Row, B>(db, ctx, &row).await?;
    }
    authorize_subscription_fields(db, ctx, &row, E::ENTITY_FIELD).await?;
    // Query and field-policy awaits must not outlive current authority.
    ensure_entity_access::<B, E>(db, ctx).await?;
    E::authorize_resolver(ctx)?;
    authorize_provenance().await?;
    E::authorize_operation(ctx)?;
    Ok(row)
}

async fn authorize_key_only<B, E>(db: &Database<B>, ctx: &Context<'_>) -> Result<()>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    E::authorize_resolver(ctx)?;
    ensure_entity_access::<B, E>(db, ctx).await?;
    authorize_subscription_key_fields::<E::Row, B>(db, ctx).await?;
    E::authorize_operation(ctx)
}

async fn check_stream_authority<B, E>(db: &Database<B>, ctx: &Context<'_>) -> Result<()>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    let result = async {
        E::authorize_operation(ctx)?;
        enforce_resolver_assurance(ctx, GraphqlOperationKind::Subscription)?;
        if !db
            .can_access_entity(
                Some(ctx),
                E::ENTITY_NAME,
                E::Row::metadata().read_policy,
                EntityAccessKind::Read,
                EntityAccessSurface::GraphqlSubscription,
            )
            .await?
        {
            return Err(OrmPublicError::forbidden().into_graphql_error());
        }
        // Current principal, scopes and assurance must survive the policy await.
        E::authorize_operation(ctx)?;
        enforce_resolver_assurance(ctx, GraphqlOperationKind::Subscription)
    }
    .await;
    if result.is_err() {
        // The timer can preempt event preparation. Discard its private diagnostics.
        buffered_errors::clear(ctx);
    }
    result
}
