//! Runtime for derive-generated Changed subscriptions.

use super::{
    ChangedPayload, SubscriptionEvent, SubscriptionKeySet, admit_generated_operation,
    admit_subscription_keys, authority_check_end, authority_recheck_delay,
    authorize_subscription_fields, authorize_subscription_key_fields, authorize_subscription_keys,
    authorize_subscription_row, continuity_events, lag_end, replay, source_closed,
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
use futures::{
    StreamExt,
    future::BoxFuture,
    stream::{self, BoxStream},
};
use std::sync::Arc;

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
    const REPLAY_ROOT: Option<&'static str> = None;

    fn key_values(key: &Self::Key) -> Vec<SqlValue>;
    fn row_key_values(row: &Self::Row) -> Vec<SqlValue>;
    fn event_key(event: &Self::Event) -> &Self::Key;
    fn view(event: &Self::Event) -> ChangedEventView<'_, Self::Row>;
    /// Build the authorized row payload, or a key-only delete when row is absent.
    fn payload(event: &Self::Event, row: Option<Self::Row>) -> Self::Payload;

    fn with_replay_cursor(payload: Self::Payload, _cursor: String) -> Self::Payload {
        payload
    }
    fn started_payload() -> Option<Self::Payload>;
    fn authorize_resolver(ctx: &Context<'_>) -> Result<()>;
    fn authorize_operation(ctx: &Context<'_>) -> Result<()>;
}

type ChangedStream<'a, T> = BoxStream<'a, Result<ChangedPayload<T>>>;

enum Frame<T> {
    Witness(T),
    Started,
}

type Witness<E, B> = SubscriptionEvent<E, B>;
type Frames<T> = BoxStream<'static, Result<Frame<T>>>;

#[derive(Clone)]
struct Cursor<T> {
    ring: Arc<replay::ChangedReplay<T>>,
    binding: [u8; 32],
}

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
    open_generated_changed_after::<B, E>(ctx, filter, key, keys, started, None)
}

/// Open an opted-in Changed stream, optionally resuming from a cursor.
#[doc(hidden)]
pub fn open_generated_changed_after<'a, B, E>(
    ctx: &'a Context<'_>,
    filter: Option<SubscriptionFilterInput>,
    key: Option<E::Key>,
    keys: Option<Vec<E::Key>>,
    started: bool,
    after: Option<String>,
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
        let (frames, cursor): (_, Option<Cursor<_>>) = if let Some(root) = E::REPLAY_ROOT {
            let binding = replay::binding(
                ctx.data_opt::<crate::graphql::auth::AuthSubject>(),
                root,
                key.as_ref()
                    .map(|key| RelationKey::from_sql_values(&E::key_values(key))),
                keys.as_deref(),
                actions.as_deref(),
            );
            let cursor = open_replay::<B, E>(db, ctx, binding, after, started).await?;
            (cursor.0, Some(cursor.1))
        } else {
            let receiver = db.ensure_event_sender::<Witness<E::Event, B>>().subscribe();
            (
                stream::iter(started.then_some(Ok(Frame::Started)))
                    .chain(continuity_events(receiver).map(|item| item.map(Frame::Witness)))
                    .boxed(),
                None,
            )
        };

        let events = frames.filter_map(move |result| {
            let key = key.clone();
            let keys = keys.clone();
            let actions = actions.clone();
            let cursor = cursor.clone();
            async move {
                let witness = match result {
                    Ok(Frame::Witness(witness)) => witness,
                    Ok(Frame::Started) => {
                        return ChangedPayload::prepare(E::started_payload()?, ctx)
                            .await
                            .map(Ok);
                    }
                    Err(error) => return Some(Err(error)),
                };
                filter_event::<B, E>(db, ctx, witness, key, keys, actions, cursor.as_ref()).await
            }
        });
        let events = watch_subscription_authority(
            events,
            db.subscription_authority_recheck_interval(),
            move || check_stream_authority::<B, E>(db, ctx),
        );
        Ok(events.boxed())
    })
}

async fn open_replay<B, E>(
    db: &Database<B>,
    ctx: &Context<'_>,
    binding: [u8; 32],
    after: Option<String>,
    started: bool,
) -> Result<(Frames<Witness<E::Event, B>>, Cursor<Witness<E::Event, B>>)>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
    let (ring, mut sender) = db.changed_replay::<Witness<E::Event, B>>();
    let after = if let Some(token) = after {
        let position = ring
            .open(&token, &binding)
            .filter(|position| !ring.expired(*position))
            .ok_or_else(replay::reset)?;
        drop(sender);

        // Defer both successful and capacity-evicted resumptions. An early
        // successful replay or started frame would also disclose eviction.
        tokio::time::sleep(authority_recheck_delay(
            db.subscription_authority_recheck_interval(),
        ))
        .await;
        check_stream_authority::<B, E>(db, ctx)
            .await
            .map_err(authority_check_end)?;

        let (current, channel) = db.changed_replay();
        if !Arc::ptr_eq(&current, &ring) {
            return Err(replay::reset());
        }
        sender = channel;
        Some(position)
    } else {
        None
    };
    let (receiver, last) = ring.subscribe(&sender, after).ok_or_else(|| {
        if after.is_some() {
            replay::reset()
        } else {
            source_closed()
        }
    })?;
    let replay = stream::unfold(after.and_then(|pos| pos.sequence.checked_add(1)), {
        let ring = ring.clone();
        move |next| {
            let ring = ring.clone();
            async move {
                let next = next.filter(|&next| next <= last)?;
                match ring.get(next) {
                    Some(witness) => Some((Ok(Frame::Witness(witness)), next.checked_add(1))),
                    None => Some((Err(lag_end()), None)),
                }
            }
        }
    });
    let mut expected = last.checked_add(1);
    let live = continuity_events(receiver).map({
        let ring = ring.clone();
        move |item| {
            let witness = item?;
            let position = witness.replay_position.ok_or_else(lag_end)?;
            // Check continuity before filtering, including events this subscriber cannot read.
            if !ring.is_next(position, expected) {
                return Err(lag_end());
            }
            expected = position.sequence.checked_add(1);
            Ok(Frame::Witness(witness))
        }
    });
    let frames = replay
        .chain(stream::iter(
            (started || after.is_some()).then_some(Ok(Frame::Started)),
        ))
        .chain(live)
        .boxed();
    Ok((frames, Cursor { ring, binding }))
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
    witness: SubscriptionEvent<E::Event, B>,
    key: Option<E::Key>,
    keys: Option<SubscriptionKeySet>,
    actions: Option<Vec<ChangeAction>>,
    cursor: Option<&Cursor<Witness<E::Event, B>>>,
) -> Option<Result<ChangedPayload<E::Payload>>>
where
    B: OrmBackend,
    E: ChangedSubscriptionAdapter<B>,
{
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
        cursor,
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
    cursor: Option<&Cursor<Witness<E::Event, B>>>,
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
    let payload = |row| {
        let payload = E::payload(event, row);
        match cursor {
            Some(cursor) => {
                let token = cursor
                    .ring
                    .seal(witness.replay_position?, &cursor.binding)?;
                Some(E::with_replay_cursor(payload, token))
            }
            None => Some(payload),
        }
    };
    match authorize().await {
        Ok(row) => {
            let prepared = ChangedPayload::prepare(payload(Some(row))?, ctx).await?;
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
            let prepared = ChangedPayload::prepare(payload(None)?, ctx).await?;
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
