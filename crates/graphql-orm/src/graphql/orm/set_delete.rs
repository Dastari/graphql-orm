//! Bounded, primary-key-fenced deletes for opted-in typed entities.

use super::{
    BoundedMutationOutcome, ChangeAction, DatabaseBackend, DatabaseFilter, DefaultWriteBackend,
    DeleteQuery, Entity, EntityAccessSurface, EntityQuery, FilterExpression, FromSqlRow,
    MutationContext, MutationEvent, MutationLimit, MutationPhase, SqlDialect, SqlValue,
    TransactionMode, WriteBackend, entity_state, render_delete_query,
};
use crate::graphql::errors::{
    OrmErrorCode, OrmPublicError, sqlx_error_from_graphql, sqlx_error_from_public,
};

/// Opt a mutable entity into [MutationContext::delete_keys_set_bounded] and
/// [MutationContext::delete_where_set_bounded] with an empty impl.
/// Entities with ORM-managed dependent deletes lack the generated adapter.
/// The operation refuses append-only and read-only entities with `FORBIDDEN`,
/// entities with an enabled generated search index with `CONFLICT`, and
/// composite keys without an entity policy with `AUTHORIZATION_MISCONFIGURED`.
///
/// The host must confirm that cascades, SET NULL/SET DEFAULT foreign keys
/// and triggers cannot delete, rewrite or redirect additional rows as a
/// side effect. The ceiling and row policies cover the target rows only.
/// The database principal must be able to select every row in the requested
/// scope to enforce an all-or-refuse filter delete. Database RLS still applies.
///
/// Row write policies must support a batch decision on the pre-delete state.
/// Hooks must support all Before callbacks running before the DELETE and
/// all After callbacks running afterwards. Before hooks may write other
/// rows in the transaction but must leave the targets and match set unchanged.
pub trait SetDeleteEntity<B: WriteBackend = DefaultWriteBackend>: SetDeleteModel<B> {}

/// Generated adapter for [SetDeleteEntity].
#[doc(hidden)]
pub trait SetDeleteModel<B: WriteBackend>:
    Entity + FromSqlRow<B> + serde::Serialize + Clone + Send + Sync + 'static
{
    type Key: Sync;
    type WhereInput: DatabaseFilter + Send;

    fn set_delete_validate_filter(filter: &Self::WhereInput) -> crate::Result<()>;
    fn set_delete_key_values(key: &Self::Key) -> Vec<SqlValue>;
    fn set_delete_row_key_values(&self) -> Vec<SqlValue>;
    fn set_delete_event_id(&self) -> String;
    fn set_delete_queue_change<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, B>,
    ) -> futures::future::BoxFuture<'a, crate::Result<()>>;
}

enum Selection<'a, K, F> {
    Keys(&'a [K]),
    Filter(F),
}

impl<B: WriteBackend> MutationContext<'_, B> {
    /// Delete exactly the supplied complete keys with one DELETE on the entity
    /// table inside a [TransactionMode::StateMachine] transaction.
    ///
    /// Empty sets are invalid. A missing key or multiple keys matching the
    /// same row causes a conflict. Exceeding the explicit row limit returns
    /// [BoundedMutationOutcome::LimitExceeded] before selection. A set that
    /// cannot fit in one statement is refused. Any error or cancellation
    /// prevents commit of the surrounding transaction.
    pub async fn delete_keys_set_bounded<T: SetDeleteEntity<B>>(
        &mut self,
        keys: &[T::Key],
        limit: MutationLimit,
    ) -> crate::Result<BoundedMutationOutcome> {
        self.delete_set_bounded::<T>(Selection::Keys(keys), limit)
            .await
    }

    /// Delete the rows matching a non-empty, SQL-renderable typed scalar
    /// filter with one DELETE on the entity table in a
    /// [TransactionMode::StateMachine] transaction.
    ///
    /// A one-row look-ahead enforces the limit before deletion. Zero matches
    /// return [BoundedMutationOutcome::Applied] with an affected count of zero
    /// and no DELETE. If the caller's limit exceeds the statement's key
    /// capacity and the look-ahead finds more matches than fit, the
    /// operation returns `INVALID_INPUT`.
    /// Any error or cancellation prevents commit of the surrounding
    /// transaction. See [SetDeleteEntity] for the opt-in contract.
    pub async fn delete_where_set_bounded<T: SetDeleteEntity<B>>(
        &mut self,
        filter: T::WhereInput,
        limit: MutationLimit,
    ) -> crate::Result<BoundedMutationOutcome> {
        self.delete_set_bounded::<T>(Selection::Filter(filter), limit)
            .await
    }

    async fn delete_set_bounded<T: SetDeleteEntity<B>>(
        &mut self,
        selection: Selection<'_, T::Key, T::WhereInput>,
        limit: MutationLimit,
    ) -> crate::Result<BoundedMutationOutcome> {
        let operation = self.runtime_state.enter();
        let result = self.delete_set_inner::<T>(selection, limit).await;
        if result.is_ok() {
            operation.complete();
        }
        result
    }

    async fn delete_set_inner<T: SetDeleteEntity<B>>(
        &mut self,
        selection: Selection<'_, T::Key, T::WhereInput>,
        limit: MutationLimit,
    ) -> crate::Result<BoundedMutationOutcome> {
        self.transaction_mode().require_write()?;
        if self.transaction_mode() != TransactionMode::StateMachine {
            return Err(set_delete_error(
                OrmErrorCode::Conflict,
                "set delete requires TransactionMode::StateMachine",
            ));
        }
        let metadata = T::metadata();
        if metadata.append_only
            || metadata
                .schema_policy
                .is_some_and(|policy| !policy.allows_entity_writes())
            || !self.database()?.schema_policy().allows_entity_writes()
        {
            return Err(sqlx_error_from_public(OrmPublicError::forbidden()));
        }
        if T::search_index().is_some_and(|index| index.enabled) {
            return Err(set_delete_error(
                OrmErrorCode::Conflict,
                "set delete is unavailable for an entity with a generated search index",
            ));
        }
        if T::PRIMARY_KEYS.len() > 1 && self.database()?.entity_policy().is_none() {
            return Err(set_delete_error(
                OrmErrorCode::AuthorizationMisconfigured,
                "composite repository mutations require an explicit entity policy provider",
            ));
        }
        super::core::ensure_transaction_entity_write_access::<T, B>(self.__policy().view()).await?;
        self.database()?
            .ensure_row_write_provider(metadata.entity_name, metadata.write_policy)
            .map_err(sqlx_error_from_graphql)?;

        let maximum = limit.get() as usize;
        let limit_exceeded = BoundedMutationOutcome::LimitExceeded {
            maximum: limit.get(),
        };
        let (query, expected) = match selection {
            Selection::Keys(keys) => {
                if keys.is_empty() {
                    return Err(set_delete_error(
                        OrmErrorCode::InvalidInput,
                        "set delete requires at least one key",
                    ));
                }
                if keys.len() > maximum {
                    return Ok(limit_exceeded);
                }
                let mut query = EntityQuery::<T, B>::new();
                query.attach_policy(key_predicate::<T, B>(
                    keys.iter().map(T::set_delete_key_values),
                )?);
                (query, Some(keys.len()))
            }
            Selection::Filter(filter) => {
                T::set_delete_validate_filter(&filter)?;
                filter.validate()?;
                if filter.is_empty() || filter.requires_in_memory_filtering(B::DIALECT) {
                    return Err(set_delete_error(
                        OrmErrorCode::InvalidInput,
                        "set delete requires a non-empty filter rendered entirely in SQL",
                    ));
                }
                let query = EntityQuery::<T, B>::new().filter(&filter);
                if query.checked_filter()?.is_none() {
                    return Err(set_delete_error(
                        OrmErrorCode::InvalidInput,
                        "set delete filter rendered no SQL predicate",
                    ));
                }
                (query, None)
            }
        };
        // A filter with a higher caller limit still cannot use more keys than
        // one statement can hold. Read just enough to distinguish that case.
        let capacity = key_capacity::<T, B>();
        let sentinel_limit =
            MutationLimit::new(limit.get().min(capacity as u32)).map_err(sqlx_error_from_public)?;
        let mut rows = query
            .fetch_set_delete_sentinel_in_transaction(self, sentinel_limit)
            .await?;
        if expected.is_some_and(|count| rows.len() != count) {
            return Err(cardinality_changed());
        }
        if rows.len() > maximum {
            return Ok(limit_exceeded);
        }
        if rows.is_empty() {
            return Ok(BoundedMutationOutcome::Applied { affected: 0 });
        }
        let selected = u32::try_from(rows.len()).map_err(|_| cardinality_changed())?;

        let keys: Vec<_> = rows.iter().map(T::set_delete_row_key_values).collect();
        let rendered = render_delete_query(
            B::DIALECT,
            &DeleteQuery {
                table: T::TABLE_NAME,
                filter: Some(key_predicate::<T, B>(keys.iter().cloned())?),
            },
        );

        Self::authorize_set_delete_rows(&self.__policy(), &rows).await?;
        let mut events = Vec::with_capacity(rows.len());
        for row in &rows {
            events.push(MutationEvent {
                phase: MutationPhase::Before,
                action: ChangeAction::Deleted,
                entity_name: metadata.entity_name,
                table_name: metadata.table_name,
                metadata,
                id: row.set_delete_event_id(),
                changes: Vec::new(),
                before_state: Some(entity_state(row)?),
                after_state: None,
            });
        }
        for event in &events {
            self.run_mutation_hook(None, event)
                .await
                .map_err(sqlx_error_from_graphql)?;
        }

        if self.database()?.mutation_hook().is_some() {
            // Transaction isolation fences concurrent changes. Hooks share the
            // transaction, so revalidate their own changes separately.
            let current = query
                .fetch_set_delete_sentinel_in_transaction(self, sentinel_limit)
                .await?;
            if current.len() != rows.len() {
                return Err(cardinality_changed());
            }
            for ((row, key), event) in current.iter().zip(&keys).zip(&mut events) {
                let state = entity_state(row)?;
                if row.set_delete_row_key_values() != *key
                    || event
                        .before_state
                        .as_ref()
                        .is_none_or(|previous| previous.as_json() != state.as_json())
                {
                    return Err(cardinality_changed());
                }
                event.before_state = Some(state);
            }
            Self::authorize_set_delete_rows(&self.__policy(), &current).await?;
            rows = current;
        }

        let result = self.execute(&rendered.sql, &rendered.values).await?;
        let affected = B::rows_affected(&result);
        if affected != u64::from(selected) {
            return Err(cardinality_changed());
        }

        for mut event in events {
            event.phase = MutationPhase::After;
            self.run_mutation_hook(None, &event)
                .await
                .map_err(sqlx_error_from_graphql)?;
        }
        for row in &rows {
            row.set_delete_queue_change(self).await?;
        }
        Ok(BoundedMutationOutcome::Applied { affected: selected })
    }

    /// Row decisions run under the transaction's authority, like its entity
    /// decision.
    async fn authorize_set_delete_rows<T: SetDeleteEntity<B>>(
        db: &super::core::TransactionPolicyHandle<'_, B>,
        rows: &[T],
    ) -> crate::Result<()> {
        let metadata = T::metadata();
        for row in rows {
            db.ensure_writable_row(
                None,
                metadata.entity_name,
                metadata.write_policy,
                EntityAccessSurface::Repository,
                row,
            )
            .await
            .map_err(sqlx_error_from_graphql)?;
        }
        Ok(())
    }
}

fn key_predicate<T, B>(
    keys: impl ExactSizeIterator<Item = Vec<SqlValue>>,
) -> crate::Result<FilterExpression>
where
    B: WriteBackend,
    T: SetDeleteModel<B>,
{
    let arity = T::PRIMARY_KEYS.len();
    if keys.len() > key_capacity::<T, B>() {
        return Err(set_delete_error(
            OrmErrorCode::InvalidInput,
            "set delete key set exceeds one statement's capacity",
        ));
    }
    let keys: Vec<_> = keys.collect();
    if keys.is_empty() || keys.iter().any(|key| key.len() != arity) {
        return Err(set_delete_error(
            OrmErrorCode::InvalidInput,
            "set delete requires complete keys",
        ));
    }
    let columns: Vec<_> = T::PRIMARY_KEYS
        .iter()
        .map(|column| B::DIALECT.quote_identifier_path(column))
        .collect();
    let columns: Vec<_> = columns.iter().map(String::as_str).collect();
    Ok(crate::graphql::loaders::relation_key_filter(
        &columns, &keys,
    ))
}

fn key_capacity<T: Entity, B: WriteBackend>() -> usize {
    let arity = T::PRIMARY_KEYS.len();
    let capacity = B::DIALECT
        .max_statement_binds()
        .checked_div(arity)
        .unwrap_or(0);
    // Composite predicates nest OR branches. Stay below SQLite's default
    // expression-depth cap as well as the backend's bind ceiling.
    if arity > 1 && B::DIALECT == DatabaseBackend::Sqlite {
        capacity.min(500)
    } else {
        capacity
    }
}

fn set_delete_error(code: OrmErrorCode, message: &'static str) -> sqlx::Error {
    sqlx_error_from_public(OrmPublicError::with_message(code, message))
}

fn cardinality_changed() -> sqlx::Error {
    set_delete_error(
        OrmErrorCode::Conflict,
        "set delete could not verify the exact target rows",
    )
}
