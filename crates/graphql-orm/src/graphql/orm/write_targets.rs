//! Target selection for generated GraphQL writes.
//!
//! A generated GraphQL update or delete locks the rows it selects before row
//! policy classifies them. These helpers narrow that locking read with the
//! host's optional write-target prefilter and bound a bulk write's rows.

use super::{
    DatabaseEntity, Entity, EntityAccessSurface, EntityQuery, FromSqlRow, MutationContext,
    ReadVisibility, WriteBackend,
};
use crate::db::Database;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};

/// Narrow the locking read of a generated GraphQL write to the rows
/// [`RowPolicy::write_target_prefilter`](super::RowPolicy::write_target_prefilter)
/// selects. Repository writes (`ctx` is `None`) are returned unchanged.
#[doc(hidden)]
pub async fn with_write_target_prefilter<T, B>(
    db: &Database<B>,
    ctx: Option<&async_graphql::Context<'_>>,
    query: EntityQuery<T, B>,
) -> async_graphql::Result<EntityQuery<T, B>>
where
    B: WriteBackend,
    T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    if ctx.is_none() {
        return Ok(query);
    }
    match db
        .write_target_prefilter::<T>(ctx, EntityAccessSurface::GraphqlMutation)
        .await?
    {
        // A prefilter only narrows: every remaining row is still classified
        // by the row callbacks.
        Some(predicate) => query.with_read_visibility(&ReadVisibility::Prefilter(predicate)),
        None => Ok(query),
    }
}

/// Select and lock the target rows of a generated bulk write.
///
/// A GraphQL bulk write applies the write-target prefilter and
/// [`PaginationConfig::max_bulk_write_rows`](super::PaginationConfig::max_bulk_write_rows):
/// it takes at most that many rows plus one look-ahead row and is refused
/// with `PAGE_LIMIT_EXCEEDED` when more match. Repository writes keep the
/// unbounded locking read.
#[doc(hidden)]
pub async fn bulk_write_targets<T, B>(
    db: &Database<B>,
    ctx: Option<&async_graphql::Context<'_>>,
    tx: &mut MutationContext<'_, B>,
    query: EntityQuery<T, B>,
) -> crate::Result<Vec<T>>
where
    B: WriteBackend,
    T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    if ctx.is_none() {
        return query.fetch_all_for_write_in_transaction(tx).await;
    }
    let query = with_write_target_prefilter(db, ctx, query)
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    let Some(max_rows) = db.pagination_config().max_bulk_write_rows else {
        return query.fetch_all_for_write_in_transaction(tx).await;
    };
    let (rows, exceeded) = query
        .fetch_for_write_with_look_ahead_in_transaction(tx, max_rows)
        .await?;
    if exceeded {
        // The message names no row count.
        return Err(crate::graphql::errors::sqlx_error_from_public(
            OrmPublicError::with_message(
                OrmErrorCode::PageLimitExceeded,
                "the bulk write selects more rows than the configured bulk write limit",
            ),
        ));
    }
    Ok(rows)
}
