//! Execution shared by generated GraphQL query resolvers.

use async_graphql::{Context, Result};
use futures::future::BoxFuture;

use super::{
    DatabaseFilter, DatabaseOrderBy, DbAuthContext, Entity, EntityAccessSurface, EntityQuery,
    FromSqlRow, GraphqlAggregateInputOperator, GraphqlAggregateResultRow, GroupedAggregateQuery,
    OrmBackend, PageInput, TypedAggregateField,
};
use crate::db::Database;
use crate::graphql::errors::{graphql_error_from_callback, graphql_error_from_sqlx};
use crate::graphql::loaders::scan_authorized_query;
use crate::graphql::pagination::{Connection, ConnectionSelection};

/// Execute an offset list after the resolver has authorized entity access,
/// filter intent and ordering intent. Keep the execution future's type out of
/// each generated resolver's Send checks.
#[doc(hidden)]
pub fn __gom_fetch_list<'a, T, B, F, O>(
    db: &'a Database<B>,
    ctx: &'a Context<'_>,
    auth: Option<&'a DbAuthContext>,
    filter: Option<&'a F>,
    orders: &'a [O],
    page: Option<&'a PageInput>,
) -> BoxFuture<'a, Result<Connection<T>>>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    F: DatabaseFilter + Clone + Send + Sync + 'static,
    O: DatabaseOrderBy + Sync + 'a,
{
    Box::pin(async move {
        let mut query = EntityQuery::<T, B>::new();
        if let Some(filter) = filter {
            query = query.filter_with_entity_matching(filter);
        }
        for order in orders {
            query = query.order_by_with_context(order, ctx)?;
        }
        if query.order_clauses.is_empty() {
            query = query.default_order();
        }

        let surface = EntityAccessSurface::GraphqlQuery;
        let visibility = db
            .read_visibility::<T>(Some(ctx), surface)
            .await
            .map_err(graphql_error_from_callback)?;
        if visibility.requires_residual_checks() {
            let resolved = db
                .client_pagination_config()
                .resolve_client_page(page, true)
                .map_err(|error| error.into_graphql_error())?;
            let authorized =
                scan_authorized_query(db, ctx, query, auth, &visibility, resolved, surface).await?;
            return Ok(authorized.into_offset_connection());
        }

        query = query.with_read_visibility(&visibility)?;
        if page.is_some() {
            // Resolve the client limit before the exact host connection read.
            let resolved = db
                .client_pagination_config()
                .resolve_client_page(page, true)
                .map_err(|error| error.into_graphql_error())?;
            query = query.paginate(&PageInput {
                limit: resolved.limit,
                offset: Some(resolved.offset),
            });
        }

        query
            .fetch_connection_selected_with_auth(db, auth, ConnectionSelection::from_context(ctx))
            .await
            .map_err(graphql_error_from_sqlx)
    })
}

/// Build and execute the typed aggregate after the resolver has authorized
/// filter intent. The aggregate builder still authorizes the requested groups,
/// metrics and row visibility during execution.
#[doc(hidden)]
pub fn __gom_fetch_aggregate<'a, T, B, F, A, R>(
    ctx: &'a Context<'_>,
    mut query: GroupedAggregateQuery<'a, T, F, A, B>,
    filter: Option<F>,
    group_by: Vec<A>,
    metrics: impl Iterator<Item = (GraphqlAggregateInputOperator, Option<A>)>,
    group_limit: i32,
    map_row: fn(GraphqlAggregateResultRow) -> R,
) -> Result<BoxFuture<'a, Result<Vec<R>>>>
where
    B: OrmBackend,
    T: Entity + Send + Sync + 'static,
    F: DatabaseFilter + Send + Sync + 'a,
    A: TypedAggregateField<T>,
    R: 'a,
{
    use GraphqlAggregateInputOperator as Operator;

    // Build the request synchronously so the generated metric input iterator
    // never becomes part of the execution future.
    if let Some(filter) = filter {
        query = query.filter(filter);
    }
    for field in group_by {
        query = query.group_by(field).map_err(graphql_error_from_sqlx)?;
    }
    for metric in metrics {
        query = match metric {
            (Operator::Count, Some(field)) => query.count(field),
            (Operator::Count, None) => query.count_rows(),
            (Operator::Min, Some(field)) => query.min(field),
            (Operator::Max, Some(field)) => query.max(field),
            (Operator::Sum, Some(field)) => query.sum(field),
            (_, None) => Err(sqlx::Error::Protocol(
                "aggregate metric field is required for this operator".to_owned(),
            )),
        }
        .map_err(graphql_error_from_sqlx)?;
    }
    let query = query
        .group_limit(i64::from(group_limit))
        .map_err(graphql_error_from_sqlx)?;
    Ok(Box::pin(async move {
        let rows = query
            .fetch_with_graphql_context(ctx)
            .await
            .map_err(graphql_error_from_sqlx)?;
        Ok(rows.into_iter().map(|row| map_row(row.into())).collect())
    }))
}
