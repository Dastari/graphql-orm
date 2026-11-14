//! Target selection for generated writes.
//!
//! A generated update or delete locks the rows it selects and classifies each
//! with row policy. These helpers narrow that locking read with the host's
//! optional write-target prefilter, bound a GraphQL bulk write by the rows the
//! caller may write, and cap the candidates it scans.

use super::{
    DatabaseEntity, Entity, EntityAccessSurface, EntityQuery, FromSqlRow, MutationContext,
    ReadVisibility, WriteBackend,
};
use crate::db::Database;
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};

/// Candidate rows read per page while a bulk write classifies them.
const CANDIDATE_PAGE: i64 = 500;

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

fn bulk_limit_exceeded() -> sqlx::Error {
    // The message names no row count.
    crate::graphql::errors::sqlx_error_from_public(OrmPublicError::with_message(
        OrmErrorCode::PageLimitExceeded,
        "the bulk write selects more rows than the configured bulk write limit",
    ))
}

/// Select, lock and classify the target rows of a generated bulk write, and
/// return the rows the caller may write.
///
/// Every selected row is classified with
/// [`Database::authorize_write_target`]: a writable row is a target, a row a
/// GraphQL caller can neither read nor write is left out, and any other
/// unwritable row refuses the whole write.
///
/// A GraphQL bulk write also applies the write-target prefilter and
/// [`PaginationConfig::max_bulk_write_rows`](super::PaginationConfig::max_bulk_write_rows),
/// which counts targets only, so rows the caller cannot see never decide
/// whether the bound is reached. When no row policy is installed and the
/// filter is rendered completely in SQL, every match is a target and one
/// query with a look-ahead row decides the bound. Otherwise the candidates are
/// read and classified in pages, never more than
/// [`PaginationConfig::max_scanned_rows`](super::PaginationConfig::max_scanned_rows)
/// of them; a scan that would need more is refused as an authorized read is.
/// Repository writes keep the unbounded locking read.
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
    let surface = if ctx.is_some() {
        EntityAccessSurface::GraphqlMutation
    } else {
        EntityAccessSurface::Repository
    };
    let metadata = T::metadata();
    let classify = |row: T| async move {
        db.authorize_write_target(
            ctx,
            metadata.entity_name,
            metadata.read_policy,
            metadata.write_policy,
            surface,
            &row as &(dyn std::any::Any + Send + Sync),
        )
        .await
        .map(|target| target.then_some(row))
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)
    };
    if ctx.is_none() {
        let mut targets = Vec::new();
        for row in query.fetch_all_for_write_in_transaction(tx).await? {
            targets.extend(classify(row).await?);
        }
        return Ok(targets);
    }
    let query = with_write_target_prefilter(db, ctx, query)
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    let config = db.pagination_config();
    let bound = config.max_bulk_write_rows.map(|bound| bound.max(0));
    let over_bound = |targets: usize| {
        bound.is_some_and(|bound| i64::try_from(targets).unwrap_or(i64::MAX) > bound)
    };
    let mut targets = Vec::new();

    // Nothing is decided in Rust: every match is a target, so the matches
    // themselves are what the bound counts.
    if db.row_policy().is_none() && !query.requires_in_memory_filtering() {
        let rows = match bound {
            Some(bound) => query
                .fetch_write_candidates_in_transaction(tx, 0, bound.saturating_add(1))
                .await?
                .into_iter()
                .map(|(row, _)| row)
                .collect(),
            None => query.fetch_all_for_write_in_transaction(tx).await?,
        };
        if over_bound(rows.len()) {
            return Err(bulk_limit_exceeded());
        }
        for row in rows {
            targets.extend(classify(row).await?);
        }
        return Ok(targets);
    }

    // Row policy or a residual filter decides in Rust: classify bounded pages
    // of candidates, counting only targets against the bound.
    let budget = config.max_scanned_rows.map(|budget| budget.max(0));
    let mut scanned = 0_i64;
    loop {
        // One row past the budget tells an exhausted scan from a complete one.
        let page = match budget {
            Some(budget) => CANDIDATE_PAGE.min(budget.saturating_sub(scanned).saturating_add(1)),
            None => CANDIDATE_PAGE,
        };
        let candidates = query
            .fetch_write_candidates_in_transaction(tx, scanned, page)
            .await?;
        let fetched = i64::try_from(candidates.len()).unwrap_or(i64::MAX);
        for (row, matches) in candidates {
            if budget.is_some_and(|budget| scanned >= budget) {
                return Err(super::query::scan_budget_exhausted());
            }
            scanned = scanned.saturating_add(1);
            if !matches {
                continue;
            }
            targets.extend(classify(row).await?);
            if over_bound(targets.len()) {
                return Err(bulk_limit_exceeded());
            }
        }
        if fetched < page {
            return Ok(targets);
        }
    }
}
