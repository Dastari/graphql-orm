//! Execution shared by generated transaction keyset reads.

use futures::future::BoxFuture;

use super::{
    AuthorizedKeysetEntity, DatabaseFilter, EntityQuery, FromSqlRow, MutationContext, PageInput,
    PaginationConfig, WriteBackend, bind_keyset_fingerprint, render_keyset_after,
    render_keyset_before,
};
use crate::graphql::errors::{OrmErrorCode, OrmPublicError};
use crate::graphql::pagination::{
    Connection, Edge, KeysetConnectionInput, KeysetPageInput, KeysetWindowDirection, PageInfo,
    decode_keyset_cursor, encode_keyset_cursor,
};

#[doc(hidden)]
pub enum GeneratedTransactionKeyset {
    Forward(KeysetPageInput),
    Connection(KeysetConnectionInput),
}

struct Window {
    limit: i64,
    cursor: Option<String>,
    backward: bool,
    include_total_count: bool,
}

impl GeneratedTransactionKeyset {
    fn resolve(self, config: PaginationConfig) -> Result<Window, OrmPublicError> {
        let (requested, limit, minimum, cursor, backward, include_total_count) = match self {
            Self::Forward(page) => (
                page.limit,
                page.limit,
                0,
                page.after,
                false,
                page.include_total_count,
            ),
            Self::Connection(page) => {
                let validated = page.validate(PaginationConfig::DEFAULT_LIMIT, i64::MAX)?;
                (
                    page.first.or(page.last),
                    Some(validated.limit),
                    1,
                    validated.cursor,
                    validated.direction == KeysetWindowDirection::Backward,
                    validated.include_total_count,
                )
            }
        };
        if let Some(requested) = requested {
            // A limit supplied by trusted transaction code must be exact or refused.
            config.checked_explicit_limit(requested)?;
        }
        let limit = config
            .resolve_page(
                Some(&PageInput {
                    limit,
                    offset: Some(0),
                }),
                true,
            )
            .limit
            .unwrap_or(PaginationConfig::DEFAULT_LIMIT)
            .max(minimum);
        Ok(Window {
            limit,
            cursor,
            backward,
            include_total_count,
        })
    }
}

/// Execute the generated page on the transaction already held by the caller.
/// The `MutationContext` entry points authorize entity access before this call
/// and check the returned rows and fields before exposing the page.
#[doc(hidden)]
pub fn __gom_transaction_keyset<'a, T, B, F>(
    hook_ctx: &'a mut MutationContext<'_, B>,
    filter: F,
    request: GeneratedTransactionKeyset,
) -> BoxFuture<'a, Result<Connection<T>, OrmPublicError>>
where
    T: AuthorizedKeysetEntity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    B: WriteBackend,
    F: DatabaseFilter + Send + 'a,
{
    Box::pin(async move {
        let order = T::KEYSET_ORDER;
        let fingerprint = bind_keyset_fingerprint(
            Some(T::entity_name()),
            T::KEYSET_FINGERPRINT,
            &filter
                .keyset_filter_identity()
                .into_iter()
                .collect::<Vec<_>>(),
        );
        if filter.requires_in_memory_filtering(B::DIALECT) {
            return Err(OrmPublicError::new(OrmErrorCode::InvalidInput));
        }
        let window = request.resolve(hook_ctx.database().pagination_config())?;
        let mut query = EntityQuery::<T, B>::new().filter(&filter);
        // The count includes all matching rows, regardless of the cursor.
        let total_count = if window.include_total_count {
            Some(
                query
                    .clone()
                    .count_in_transaction(hook_ctx)
                    .await
                    .map_err(OrmPublicError::from)?,
            )
        } else {
            None
        };
        if let Some(cursor) = window.cursor.as_deref() {
            let values = decode_keyset_cursor(cursor, &fingerprint, order.len())?;
            let render = if window.backward {
                render_keyset_before
            } else {
                render_keyset_after
            };
            let (predicate, binds) = render(B::DIALECT, order, &values, query.values.len() + 1)?;
            query = query.where_values(&predicate, binds);
        }
        query.order_clauses = if window.backward {
            order
                .iter()
                .map(|column| column.reversed().order_sql())
                .collect()
        } else {
            order.iter().map(|column| column.order_sql()).collect()
        };
        query = query.paginate(&PageInput {
            limit: Some(window.limit.saturating_add(1)),
            offset: Some(0),
        });
        let mut entities = query
            .fetch_all_in_transaction(hook_ctx)
            .await
            .map_err(OrmPublicError::from)?;
        let has_more = entities.len() > window.limit as usize;
        if has_more {
            entities.truncate(window.limit as usize);
        }
        if window.backward {
            entities.reverse();
        }
        let edges = entities
            .into_iter()
            .map(|entity| Edge {
                cursor: encode_keyset_cursor(&fingerprint, entity.keyset_values()),
                node: entity,
            })
            .collect::<Vec<_>>();
        let had_cursor = window.cursor.is_some();
        Ok(Connection {
            page_info: PageInfo {
                has_next_page: if window.backward {
                    had_cursor
                } else {
                    has_more
                },
                has_previous_page: if window.backward {
                    has_more
                } else {
                    had_cursor
                },
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                total_count,
            },
            edges,
        })
    })
}
