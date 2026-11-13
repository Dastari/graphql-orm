//! Row authority for generated single-row reads.
//!
//! Get-by-key roots, `NodeId` lookups, Federation `_entities` and the release
//! of a generated mutation's committed row resolve the request's
//! [`ReadVisibility`] exactly as generated list reads do: the SQL predicate
//! restricts the row, and [`crate::db::Database::can_read_row`] runs only when
//! that decision requires residual checks. Repository single-row reads apply
//! the `Repository`-surface predicate and keep calling `can_read_row` for the
//! row, as repository list reads do.
//!
//! Every host callback error (`read_visibility`, `can_read_row`) is mapped to
//! a sanitized public error here, so generated call sites cannot release a
//! provider's internal message.
use super::*;
use crate::db::Database;
use crate::graphql::errors::{graphql_error_from_callback, graphql_error_from_sqlx};

/// The request's read visibility for `T` on `surface`, with provider errors
/// sanitized.
async fn visibility_for<T, B>(
    db: &Database<B>,
    ctx: Option<&async_graphql::Context<'_>>,
    surface: EntityAccessSurface,
) -> async_graphql::Result<ReadVisibility>
where
    B: OrmBackend,
    T: Entity + 'static,
{
    db.read_visibility::<T>(ctx, surface)
        .await
        .map_err(graphql_error_from_callback)
}

/// Residual row callback for one single-row candidate, with provider errors
/// sanitized.
async fn authorize_residual<T, B>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    surface: EntityAccessSurface,
    visibility: &ReadVisibility,
    row: Option<T>,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + Send + Sync + 'static,
{
    let Some(row) = row else {
        return Ok(None);
    };
    if visibility.requires_residual_checks()
        && !db
            .can_read_row(
                Some(ctx),
                T::entity_name(),
                T::metadata().read_policy,
                surface,
                &row as &(dyn std::any::Any + Send + Sync),
            )
            .await
            .map_err(graphql_error_from_callback)?
    {
        return Ok(None);
    }
    Ok(Some(row))
}

/// Read the row `key_query` selects under `visibility`, allowing at most one.
async fn read_key_under<T, B>(
    db: &Database<B>,
    auth: Option<&DbAuthContext>,
    key_query: EntityQuery<T, B>,
    visibility: &ReadVisibility,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let mut rows = key_query
        .with_read_visibility(visibility)
        .map_err(graphql_error_from_callback)?
        .paginate(&PageInput {
            limit: Some(2),
            offset: Some(0),
        })
        .fetch_all_with_auth(db, auth)
        .await
        .map_err(graphql_error_from_sqlx)?;
    if rows.len() > 1 {
        return Err(crate::graphql::errors::OrmPublicError::forbidden().into_graphql_error());
    }
    Ok(rows.pop())
}

/// Fetch one generated get-by-key row under the request's read visibility.
///
/// `query` must restrict the complete key. Entity access is checked by the
/// caller first.
#[doc(hidden)]
pub async fn fetch_authorized_point_row<T, B>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    query: EntityQuery<T, B>,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let surface = EntityAccessSurface::GraphqlQuery;
    let visibility = visibility_for::<T, B>(db, Some(ctx), surface).await?;
    let row = read_key_under(db, ctx.data_opt::<DbAuthContext>(), query, &visibility).await?;
    authorize_residual(db, ctx, surface, &visibility, row).await
}

/// Authorize one row a batching Federation loader resolved for `columns`.
///
/// The batched key loader selects every representation of one fetch in a
/// single statement and takes no read predicate, so a decision with a SQL
/// predicate re-reads the key under it; a row outside the predicate becomes
/// `None`. `columns` are the backend-quoted key columns the loader matched,
/// and `values` their bound values.
#[doc(hidden)]
pub async fn authorize_loaded_point_row<T, B>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    columns: &[&str],
    values: Vec<SqlValue>,
    loaded: Option<T>,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let surface = EntityAccessSurface::GraphqlQuery;
    // Resolve the decision before looking at the loaded row, so a refusal is
    // the same for an existing and a missing key.
    let visibility = visibility_for::<T, B>(db, Some(ctx), surface).await?;
    if loaded.is_none() {
        return Ok(None);
    }
    let row = if visibility.predicate_expression().is_some() {
        if columns.is_empty() || columns.len() != values.len() {
            return Err(crate::graphql::errors::OrmPublicError::forbidden().into_graphql_error());
        }
        let clause = columns
            .iter()
            .map(|column| format!("{column} = ?"))
            .collect::<Vec<_>>()
            .join(" AND ");
        read_key_under(
            db,
            ctx.data_opt::<DbAuthContext>(),
            EntityQuery::<T, B>::new().where_values(&clause, values),
            &visibility,
        )
        .await?
    } else {
        loaded
    };
    authorize_residual(db, ctx, surface, &visibility, row).await
}

/// The committed row a generated mutation may release, if any.
///
/// Applies the `GraphqlMutation`-surface read visibility as a list read would:
/// a SQL predicate re-reads `key_query` under it after the commit, and that
/// re-read row is the one checked and released; `can_read_row` runs only when
/// the decision requires residual checks. Errors are sanitized; the caller
/// marks them as committed.
pub(crate) async fn mutation_output_row<T, B>(
    db: &Database<B>,
    ctx: &async_graphql::Context<'_>,
    key_query: EntityQuery<T, B>,
    entity: T,
) -> async_graphql::Result<Option<T>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let surface = EntityAccessSurface::GraphqlMutation;
    let visibility = visibility_for::<T, B>(db, Some(ctx), surface).await?;
    let row = if visibility.predicate_expression().is_some() {
        read_key_under(db, ctx.data_opt::<DbAuthContext>(), key_query, &visibility).await?
    } else {
        Some(entity)
    };
    authorize_residual(db, ctx, surface, &visibility, row).await
}

/// Apply the `Repository`-surface read predicate to a repository read.
///
/// Repository reads keep calling `can_read_row` for every row; the predicate
/// only narrows the SQL, as for repository list reads.
#[doc(hidden)]
pub async fn with_repository_read_visibility<T, B>(
    db: &Database<B>,
    query: EntityQuery<T, B>,
) -> crate::Result<EntityQuery<T, B>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    let visibility = db
        .read_visibility::<T>(None, EntityAccessSurface::Repository)
        .await
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)?;
    query
        .with_read_visibility(&visibility)
        .map_err(crate::graphql::errors::sqlx_error_from_graphql)
}
