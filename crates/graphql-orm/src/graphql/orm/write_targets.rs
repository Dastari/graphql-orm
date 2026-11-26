//! Target selection for generated writes.
//!
//! A generated update or delete locks the rows it selects and classifies each
//! with row policy. These helpers narrow that locking read with the host's
//! optional write-target prefilter (or, for a bulk write, the SQL predicate of
//! its read visibility), bound a GraphQL bulk write by the rows the caller may
//! write, and cap the candidates it scans.

use super::{
    DatabaseBackend, DatabaseEntity, Entity, EntityAccessSurface, EntityQuery, FromSqlRow,
    MutationContext, OrmBackend, ReadVisibility, SqlValue, WriteBackend,
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

/// Narrow the locking read of a generated GraphQL bulk write in SQL.
///
/// A declared write-target prefilter decides alone, as for single-row writes.
/// Without one, and only when the host opted in with
/// [`DatabaseBuilder::narrow_bulk_writes_to_read_visibility`](crate::db::DatabaseBuilder::narrow_bulk_writes_to_read_visibility),
/// a SQL predicate from the policy's `GraphqlMutation`-surface
/// [`read_visibility`](super::RowPolicy::read_visibility) (`Prefilter` or
/// `Complete`) narrows the read, so rows outside the caller's read visibility
/// are never passed to a row callback. The predicate only narrows: every
/// selected row is still classified by the row callbacks. `CallbackOnly` and
/// `Unrestricted` leave the read unchanged, and so does a database without a
/// row policy.
///
/// Also returns whether the host opted in and a SQL predicate narrows the
/// read. Only then, and only with
/// [`DatabaseBuilder::mssql_snapshot_bulk_write_targets`](crate::db::DatabaseBuilder::mssql_snapshot_bulk_write_targets)
/// as well, does [`bulk_write_targets`] select its targets from snapshots on
/// SQL Server.
async fn with_bulk_target_visibility<T, B>(
    db: &Database<B>,
    ctx: Option<&async_graphql::Context<'_>>,
    query: EntityQuery<T, B>,
) -> async_graphql::Result<(EntityQuery<T, B>, bool)>
where
    B: WriteBackend,
    T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    if ctx.is_none() || db.row_policy().is_none() {
        return Ok((query, false));
    }
    let opted_in = db.narrow_bulk_writes_to_read_visibility_enabled();
    let surface = EntityAccessSurface::GraphqlMutation;
    if let Some(predicate) = db.write_target_prefilter::<T>(ctx, surface).await? {
        let query = query.with_read_visibility(&ReadVisibility::Prefilter(predicate))?;
        return Ok((query, opted_in));
    }
    if !opted_in {
        return Ok((query, false));
    }
    match db.read_visibility::<T>(ctx, surface).await? {
        ReadVisibility::Prefilter(predicate) | ReadVisibility::Complete(predicate) => Ok((
            query.with_read_visibility(&ReadVisibility::Prefilter(predicate))?,
            true,
        )),
        ReadVisibility::CallbackOnly | ReadVisibility::Unrestricted => Ok((query, false)),
    }
}

/// The rows a SQL Server bulk write's filter selects changed between its
/// candidate read and its validation. Retrying the whole write is safe.
fn targets_changed() -> sqlx::Error {
    let mut error = OrmPublicError::with_message(
        OrmErrorCode::ServiceUnavailable,
        "the bulk write's target rows changed concurrently; retry the write",
    );
    error.retryable = true;
    crate::graphql::errors::sqlx_error_from_public(error)
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
/// A GraphQL bulk write narrows its locking read in SQL with the write-target
/// prefilter when the row policy declares one. Without one, a host that opted
/// in with
/// [`DatabaseBuilder::narrow_bulk_writes_to_read_visibility`](crate::db::DatabaseBuilder::narrow_bulk_writes_to_read_visibility)
/// narrows it with the SQL predicate of the policy's `GraphqlMutation`-surface
/// read visibility, so rows outside it are not classified; a row the caller
/// may write but not read is then a target only when a write-target prefilter
/// selects it.
///
/// With the opt-in and a SQL predicate, rows outside the predicate are not
/// locked either on PostgreSQL, whose `FOR UPDATE` locks only the rows the
/// predicate returns. SQL Server's single `UPDLOCK, HOLDLOCK` read locks every
/// row a scan reads. With the separate opt-in
/// [`DatabaseBuilder::mssql_snapshot_bulk_write_targets`](crate::db::DatabaseBuilder::mssql_snapshot_bulk_write_targets),
/// where [`EntityQuery::snapshot_write_candidates_key_index`] finds a unique
/// key index, SQL Server instead reads the candidates in one snapshot without
/// a lock, locks them exclusively by a forced seek on that index, checks them
/// again, and reads them again in a new snapshot: a candidate set that changed
/// meanwhile refuses the write as a retryable conflict. The target set then
/// equals the rows that match at that validation snapshot, but no range is
/// locked, so the transaction is no longer serializable. `key_value` gives a
/// row's key.
///
/// It also applies
/// [`PaginationConfig::max_bulk_write_rows`](super::PaginationConfig::max_bulk_write_rows),
/// which counts targets only, so rows the caller cannot see never decide
/// whether the bound is reached. When no row policy is installed and the
/// filter is rendered completely in SQL, every match is a target and one
/// query with a look-ahead row decides the bound. Otherwise the candidates are
/// read and classified in pages, never more than
/// [`PaginationConfig::max_scanned_rows`](super::PaginationConfig::max_scanned_rows)
/// of them; a scan that would need more is refused as an authorized read is.
/// Repository writes keep the unbounded locking read.
///
/// This is the target admission of a generated bulk write (`write_pipeline`).
/// A GraphQL write presents its request with the authority of its bulk
/// admission: one that was not earned for `T` under that request on `db`, the
/// transaction's `Database`, is refused, and the authority is issued once the
/// targets are admitted.
#[doc(hidden)]
pub async fn bulk_write_targets<T, B>(
    db: &Database<B>,
    request: Option<super::__GeneratedWriteRequestAuthority<'_, '_>>,
    tx: &mut MutationContext<'_, B>,
    query: EntityQuery<T, B>,
    key_value: fn(&T) -> SqlValue,
) -> crate::Result<Vec<T>>
where
    B: WriteBackend,
    T: Entity + DatabaseEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    if let Some((ctx, authority)) = request {
        authority
            .__require_bulk_targets::<T, B>(ctx, db, tx.__base_database())
            .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    }
    let targets =
        admit_bulk_write_targets(db, request.map(|(ctx, _)| ctx), tx, query, key_value).await?;
    if let Some((_, authority)) = request {
        authority.__issue_bulk_targets();
    }
    Ok(targets)
}

async fn admit_bulk_write_targets<T, B>(
    db: &Database<B>,
    ctx: Option<&async_graphql::Context<'_>>,
    tx: &mut MutationContext<'_, B>,
    query: EntityQuery<T, B>,
    key_value: fn(&T) -> SqlValue,
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
    let (query, narrowed) = with_bulk_target_visibility(db, ctx, query)
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    // Generated bulk writes exist only for entities with a single-column key.
    // The snapshot path is a separate opt-in: it does not keep the bulk
    // write's transaction serializable.
    let key_index = if narrowed
        && db.mssql_snapshot_bulk_write_targets_enabled()
        && <B as OrmBackend>::DIALECT == DatabaseBackend::Mssql
        && T::PRIMARY_KEYS.len() == 1
    {
        EntityQuery::<T, B>::snapshot_write_candidates_key_index(tx).await?
    } else {
        None
    };
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
    if let Some(key_index) = key_index {
        // One consistent read of every candidate, bounded by the budget.
        let limit = budget.map(|budget| budget.saturating_add(1));
        let candidates = query
            .fetch_write_candidate_keys_in_transaction(tx, limit, key_value)
            .await?;
        // Refused before any candidate is locked.
        if budget.is_some_and(|budget| candidates.read > budget) {
            return Err(super::query::scan_budget_exhausted());
        }
        let mut locked = std::collections::BTreeSet::new();
        for (row, matches) in query
            .lock_write_candidates_by_key_in_transaction(
                tx,
                &candidates.keys,
                &key_index,
                key_value,
            )
            .await?
        {
            // A row that left the filter or the predicate before it was
            // locked is not a candidate.
            if !matches {
                continue;
            }
            locked.insert(super::composite_key_id(&[key_value(&row)]));
            targets.extend(classify(row).await?);
            if over_bound(targets.len()) {
                return Err(bulk_limit_exceeded());
            }
        }
        // Every candidate is now locked and checked. Read the candidates
        // again, in one snapshot: if they are exactly the locked rows, the
        // targets are the rows the filter selects at that instant, and none
        // of them can change before the write commits. Otherwise another
        // transaction changed which rows match since the first read.
        let current = query
            .fetch_write_candidate_keys_in_transaction(tx, limit, key_value)
            .await?;
        // A read cut off at the budget may have missed a candidate.
        if budget.is_some_and(|budget| current.read > budget) {
            return Err(super::query::scan_budget_exhausted());
        }
        let current = current
            .keys
            .iter()
            .map(|key| super::composite_key_id(std::slice::from_ref(key)))
            .collect::<std::collections::BTreeSet<_>>();
        if current != locked {
            return Err(targets_changed());
        }
        return Ok(targets);
    }
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
