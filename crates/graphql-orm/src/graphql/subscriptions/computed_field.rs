use super::{
    ChangedPayload, admit_generated_operation, authorize_subscription_keys,
    authorize_subscription_row, continuity_events, watch_subscription_authority,
};
use crate::db::{Database, request_database};
use crate::graphql::{
    assurance::enforce_resolver_assurance,
    errors::{OrmPublicError, buffered_errors, graphql_error_from_callback},
    loaders::RelationKey,
    orm::{
        Entity, EntityAccessKind, EntityAccessSurface, FromSqlRow, GraphqlOperationKind,
        MutationContext, SqlValue, WriteBackend, composite_key_id,
    },
};
use async_graphql::{Context, ID, SimpleObject};
use futures::{Stream, StreamExt, future::BoxFuture};
use std::{marker::PhantomData, pin::Pin, sync::Arc};

/// Connects an invalidation topic to the key and operation authority generated
/// by GraphQLOperations.
#[doc(hidden)]
pub trait ComputedFieldInvalidationTarget<B: WriteBackend, K>:
    Entity + FromSqlRow<B> + Clone + Send + Sync + 'static
{
    fn invalidation_key_values(key: &K) -> Vec<SqlValue>;
    fn invalidation_row_key_values(&self) -> Vec<SqlValue>;
    fn authorize_invalidation_target_operation(ctx: &Context<'_>) -> async_graphql::Result<()>;
}

/// A host-declared computed field or topic and its disclosure policy.
///
/// Use a distinct type for each topic and a stable public label. The policy
/// sees the current target row and subscriber context. It must explicitly
/// allow disclosure that the computed value may have changed. Delivery also
/// requires current target operation, row, and key-field authorization.
pub trait ComputedFieldTopic<B: WriteBackend>: Send + Sync + 'static {
    /// The generated entity whose computed field can be read again.
    type Target: ComputedFieldInvalidationTarget<B, Self::Key>;

    /// The generated primary key type of the target entity.
    type Key;

    /// A stable public field or topic name, independent of the source write.
    const TOPIC: &'static str;

    /// Decide whether to disclose this invalidation to the current subscriber.
    /// A refusal or an error silently suppresses delivery.
    fn can_disclose<'a>(
        ctx: &'a Context<'_>,
        db: &'a Database<B>,
        target: &'a Self::Target,
    ) -> BoxFuture<'a, async_graphql::Result<bool>>;
}

/// A signal to refetch a computed field through its normal read operation.
#[derive(SimpleObject)]
pub struct ComputedFieldInvalidation {
    /// Current authorized target ID. Composite keys use the Changed ID encoding.
    id: ID,
    /// Host-declared stable computed field or topic.
    topic: &'static str,
}

/// Stream returned by [`subscribe_computed_field`].
pub type ComputedFieldInvalidationStream<'a> = Pin<
    Box<
        dyn Stream<Item = async_graphql::Result<ChangedPayload<ComputedFieldInvalidation>>>
            + Send
            + 'a,
    >,
>;

// The public payload cannot be used to publish an invalidation through the
// raw observer bus or queue_event.
struct PendingInvalidation<T, B>(Arc<[SqlValue]>, PhantomData<fn() -> (T, B)>);

impl<T, B> Clone for PendingInvalidation<T, B> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<B: WriteBackend> MutationContext<'_, B> {
    /// Enqueue one invalidation of a typed target key from trusted writer code.
    ///
    /// Publishes once per topic type and typed key after successful commit.
    /// A full rollback or failed commit discards it. Raw SQL savepoint rollbacks
    /// leave queued invalidations in the enclosing transaction.
    pub fn invalidate_computed_field<T: ComputedFieldTopic<B>>(&mut self, key: &T::Key) {
        let keys = T::Target::invalidation_key_values(key);
        let identity = RelationKey::from_sql_values(&keys);
        self.queue_commit_only_event(
            PendingInvalidation::<T, B>(keys.into(), PhantomData),
            identity,
        );
    }
}

fn authorize_operation<T: ComputedFieldTopic<B>, B: WriteBackend>(
    ctx: &Context<'_>,
) -> async_graphql::Result<()> {
    T::Target::authorize_invalidation_target_operation(ctx)?;
    enforce_resolver_assurance(ctx, GraphqlOperationKind::Subscription)
}

async fn authorize_target<T: ComputedFieldTopic<B>, B: WriteBackend>(
    db: &Database<B>,
    ctx: &Context<'_>,
) -> async_graphql::Result<()> {
    authorize_operation::<T, B>(ctx)?;
    db.ensure_entity_access(
        Some(ctx),
        T::Target::entity_name(),
        T::Target::metadata().read_policy,
        EntityAccessKind::Read,
        EntityAccessSurface::GraphqlSubscription,
    )
    .await
    .map_err(graphql_error_from_callback)?;
    authorize_operation::<T, B>(ctx)
}

async fn authorized_id<T: ComputedFieldTopic<B>, B: WriteBackend>(
    db: &Database<B>,
    ctx: &Context<'_>,
    keys: &[SqlValue],
) -> async_graphql::Result<ID> {
    authorize_operation::<T, B>(ctx)?;
    let row = authorize_subscription_row::<T::Target, B>(db, ctx, keys, None)
        .await?
        .ok_or_else(|| OrmPublicError::forbidden().into_graphql_error())?;
    authorize_subscription_keys(db, ctx, &row).await?;
    if !T::can_disclose(ctx, db, &row).await? {
        return Err(OrmPublicError::forbidden().into_graphql_error());
    }
    authorize_target::<T, B>(db, ctx).await?;

    // Use the authorized row's key spelling even if the database's
    // collation matched a differently spelled input key.
    let current_keys = row.invalidation_row_key_values();
    let id = if current_keys.len() == 1 {
        RelationKey::from_sql_values(&current_keys).parts()[0].clone()
    } else {
        composite_key_id(&current_keys)
    };
    Ok(ID(id))
}

/// Open a computed-field invalidation stream in a host subscription resolver.
///
/// Register the resolver's GraphQL field name with
/// [`GeneratedSubscriptionAdmission`](super::GeneratedSubscriptionAdmission).
/// The topic type determines the target and policy. Each event is authorized
/// against the live target before release. Target operation authority is also
/// rechecked while idle. Use the normal query resolver to fetch the new value.
pub async fn subscribe_computed_field<'ctx, T: ComputedFieldTopic<B>, B: WriteBackend>(
    ctx: &Context<'ctx>,
) -> async_graphql::Result<ComputedFieldInvalidationStream<'ctx>> {
    admit_generated_operation(ctx)?;
    let db = request_database::<B>(ctx)?.clone();
    let errors_before = buffered_errors::count(ctx)?;
    let admitted = authorize_target::<T, B>(&db, ctx).await;
    buffered_errors::truncate(ctx, errors_before)?;
    admitted.map_err(|_| OrmPublicError::forbidden().into_graphql_error())?;

    let rx = db
        .ensure_event_sender::<PendingInvalidation<T, B>>()
        .subscribe();
    let interval = db.subscription_authority_recheck_interval();
    let timer_ctx = ctx.clone();
    let timer_db = db.clone();
    let ctx = ctx.clone();
    let events = continuity_events(rx).filter_map(move |result| {
        let ctx = ctx.clone();
        let db = db.clone();
        async move {
            let event = match result {
                Ok(event) => event,
                Err(error) => return Some(Err(error)),
            };
            let result = async {
                let errors_before = buffered_errors::count(&ctx)?;
                let prepared = async {
                    let id = authorized_id::<T, B>(&db, &ctx, &event.0).await.ok()?;
                    let payload = ComputedFieldInvalidation {
                        id: id.clone(),
                        topic: T::TOPIC,
                    };
                    let prepared = ChangedPayload::prepare(payload, &ctx).await?;
                    // Payload resolution can itself await. Verify the release
                    // contract again, including the identity we prepared.
                    let current = authorized_id::<T, B>(&db, &ctx, &event.0).await.ok()?;
                    (current == id).then_some(prepared)
                }
                .await;
                buffered_errors::truncate(&ctx, errors_before)?;
                Ok(prepared)
            }
            .await;
            result.transpose()
        }
    });
    Ok(Box::pin(watch_subscription_authority(
        events,
        interval,
        move || {
            let ctx = timer_ctx.clone();
            let db = timer_db.clone();
            async move {
                let errors_before = buffered_errors::count(&ctx)?;
                let allowed = authorize_target::<T, B>(&db, &ctx).await;
                // A terminating timer can preempt a partially prepared event.
                // Discard its diagnostics as well as the timer's diagnostics.
                buffered_errors::truncate(&ctx, if allowed.is_ok() { errors_before } else { 0 })?;
                allowed
            }
        },
    )))
}
