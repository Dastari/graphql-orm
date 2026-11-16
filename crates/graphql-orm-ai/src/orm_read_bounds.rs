//! Row bounds for the runtime's own ORM reads.
//!
//! Every internal read carries its own bound: a constant, a `bound + 1`
//! look-ahead, or a deployment limit that this crate validates. ORM 0.128.0
//! refuses a repository limit above the database's maximum page limit instead
//! of reducing it, so each internal read raises the maximum for itself only.
//! The read stays bounded by its own limit, and the database-wide maximum that
//! GraphQL clients see is unchanged.

use graphql_orm::graphql::errors::OrmPublicError;
use graphql_orm::graphql::orm::{
    DatabaseFilter, DatabaseOrderBy, Entity, FromSqlRow, MutationQuery, OrmBackend, ReadProjection,
    RepositoryQuery, TransactionProjectionQuery, WriteBackend,
};
use graphql_orm::graphql::pagination::{Connection, KeysetConnectionInput, PageInfo};

/// Applies an internal read's own limit as that read's maximum.
pub(crate) trait InternalReadBound: Sized {
    /// Reads at most `limit` rows, exactly, whatever the database's maximum
    /// page limit is. Pass only a constant or a value this crate has
    /// validated, never an unchecked client value.
    fn internal_limit(self, limit: i64) -> Self;
}

impl<T, B> InternalReadBound for MutationQuery<'_, '_, T, B>
where
    B: WriteBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
{
    fn internal_limit(self, limit: i64) -> Self {
        self.with_max_limit(limit).limit(limit)
    }
}

impl<P, B> InternalReadBound for TransactionProjectionQuery<'_, '_, P, B>
where
    B: WriteBackend,
    P: ReadProjection<B>,
{
    fn internal_limit(self, limit: i64) -> Self {
        self.with_max_limit(limit).limit(limit)
    }
}

impl<T, F, O, B> InternalReadBound for RepositoryQuery<'_, T, F, O, B>
where
    B: OrmBackend,
    T: Entity + FromSqlRow<B> + Clone + Send + Sync + 'static,
    F: DatabaseFilter + Clone + Send + Sync + 'static,
    O: DatabaseOrderBy,
{
    fn internal_limit(self, limit: i64) -> Self {
        self.with_max_limit(limit).limit(limit)
    }
}

/// Clamps a client-supplied page size to the database's maximum page limit.
///
/// Client paging keeps GraphQL page semantics: a host that configures a
/// smaller maximum caps every client page, and the page reports the remaining
/// rows through its cursor or `has_more` flag. Read the clamped size exactly
/// with [`InternalReadBound::internal_limit`], adding any look-ahead row after
/// clamping.
pub(crate) fn client_page_limit(
    config: graphql_orm::graphql::orm::PaginationConfig,
    requested: i64,
) -> i64 {
    config
        .clamp_explicit_limit(Some(requested))
        .unwrap_or(requested)
}

/// Reads one host keyset window completely, in pages no larger than the
/// database's maximum page limit.
///
/// A host keyset read (`Record::keyset_connection_page` with no GraphQL
/// context) refuses a `first` or `last` above the maximum with
/// `PAGE_LIMIT_EXCEEDED`; it never returns fewer rows than asked for. A window
/// this crate has already validated may be larger than a host's maximum, so
/// it is read here as consecutive cursor pages of at most that maximum until
/// the window is full or the set ends. The result is the window one page of
/// that size would return: the same edges in canonical order, the outer
/// cursors, and both has-page flags, so callers continue from its cursors as
/// before. A window within the maximum, or an ambiguous one (both `first` and
/// `last`), is passed through unchanged for the ORM to read or refuse.
pub(crate) async fn keyset_window<T, F, Fut>(
    maximum: Option<i64>,
    page: KeysetConnectionInput,
    mut fetch: F,
) -> Result<Connection<T>, OrmPublicError>
where
    F: FnMut(KeysetConnectionInput) -> Fut,
    Fut: std::future::Future<Output = Result<Connection<T>, OrmPublicError>>,
{
    let backward = match (page.first, page.last) {
        (Some(_), None) => false,
        (None, Some(_)) => true,
        _ => return fetch(page).await,
    };
    let requested = page.first.or(page.last).unwrap_or_default();
    let Some(maximum) = maximum.map(|maximum| maximum.max(1)) else {
        return fetch(page).await;
    };
    if requested <= maximum {
        return fetch(page).await;
    }
    let mut cursor = if backward {
        page.before.clone()
    } else {
        page.after.clone()
    };
    let mut remaining = requested;
    let mut chunks: Vec<Connection<T>> = Vec::new();
    loop {
        let size = remaining.min(maximum);
        let chunk = fetch(KeysetConnectionInput {
            after: if backward { None } else { cursor.clone() },
            before: if backward { cursor.clone() } else { None },
            first: (!backward).then_some(size),
            last: backward.then_some(size),
            // A requested count is read once, with the first page.
            include_total_count: page.include_total_count && chunks.is_empty(),
        })
        .await?;
        let read = i64::try_from(chunk.edges.len()).unwrap_or(i64::MAX);
        remaining = remaining.saturating_sub(read);
        let more = if backward {
            chunk.page_info.has_previous_page
        } else {
            chunk.page_info.has_next_page
        };
        cursor = if backward {
            chunk.page_info.start_cursor.clone()
        } else {
            chunk.page_info.end_cursor.clone()
        };
        chunks.push(chunk);
        if remaining <= 0 || !more || read == 0 || cursor.is_none() {
            break;
        }
    }
    // `chunks[0]` is nearest the starting cursor, the last one farthest.
    let first_read = chunks.first().map(|chunk| chunk.page_info.clone());
    let last_read = chunks.last().map(|chunk| chunk.page_info.clone());
    let (Some(first_read), Some(last_read)) = (first_read, last_read) else {
        return Err(OrmPublicError::new(
            graphql_orm::graphql::errors::OrmErrorCode::InternalError,
        ));
    };
    if backward {
        chunks.reverse();
    }
    let edges = chunks
        .into_iter()
        .flat_map(|chunk| chunk.edges)
        .collect::<Vec<_>>();
    let page_info = if backward {
        PageInfo {
            has_next_page: first_read.has_next_page,
            has_previous_page: last_read.has_previous_page,
            start_cursor: last_read.start_cursor,
            end_cursor: first_read.end_cursor,
            total_count: first_read.total_count,
        }
    } else {
        PageInfo {
            has_next_page: last_read.has_next_page,
            has_previous_page: first_read.has_previous_page,
            start_cursor: first_read.start_cursor,
            end_cursor: last_read.end_cursor,
            total_count: first_read.total_count,
        }
    };
    Ok(Connection { edges, page_info })
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::{InternalReadBound, client_page_limit};
    use crate::persistence::{AiInboxStreamRecord, CreateAiInboxStreamRecordInput};
    use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
    use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule, TransactionMode};
    use graphql_orm::graphql::pagination::{Connection, KeysetConnectionInput, PageInfo};
    use graphql_orm::prelude::{Database, PaginationConfig, SqliteBackend};

    const ROWS: i64 = 150;
    const BOUND: i64 = 128;

    async fn database() -> Database<SqliteBackend> {
        let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        assert_eq!(
            database.pagination_config(),
            PaginationConfig::default(),
            "these reads must work on a default database"
        );
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-read-bounds-test-v1",
                "AI internal read bounds",
                crate::AiSchemaModule.entities(),
            )
            .await
            .expect("schema should plan");
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .expect("schema should apply");
        database
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    for index in 0..ROWS {
                        tx.insert::<AiInboxStreamRecord>(CreateAiInboxStreamRecordInput {
                            id: uuid::Uuid::new_v4(),
                            principal_kind: "user".to_owned(),
                            principal_subject: format!("subject-{index}"),
                            stream_head: 0,
                            minimum_retained_sequence: 1,
                            last_event_at: index,
                        })
                        .await
                        .map_err(OrmPublicError::from)?;
                    }
                    Ok(())
                })
            })
            .await
            .expect("stream rows should insert");
        database
    }

    #[tokio::test]
    async fn internal_bound_plus_one_read_is_exact_on_a_default_database() {
        let database = database().await;
        let (exact, refused) = database
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    let exact = tx
                        .query::<AiInboxStreamRecord>()
                        .default_order()
                        .internal_limit(BOUND + 1)
                        .fetch_all()
                        .await
                        .map_err(OrmPublicError::from)?
                        .len();
                    // The plain ORM limit is refused above the page maximum,
                    // which is why internal reads carry their own bound.
                    let refused = tx
                        .query::<AiInboxStreamRecord>()
                        .limit(BOUND + 1)
                        .fetch_all()
                        .await
                        .map_err(|error| OrmPublicError::from_sqlx(&error).code)
                        .err();
                    Ok((exact, refused))
                })
            })
            .await
            .expect("transaction should commit");
        assert_eq!(exact as i64, BOUND + 1);
        assert_eq!(refused, Some(OrmErrorCode::PageLimitExceeded));
    }

    /// A keyset source over `0..rows` that refuses any page above `maximum`,
    /// as a host keyset read does, and records every page size it served.
    fn fake_keyset(
        rows: i64,
        maximum: i64,
        sizes: &std::sync::Mutex<Vec<i64>>,
        page: KeysetConnectionInput,
    ) -> Result<Connection<i64>, OrmPublicError> {
        use graphql_orm::graphql::pagination::Edge;
        let size = page.first.or(page.last).expect("one direction");
        if size > maximum {
            return Err(OrmPublicError::new(OrmErrorCode::PageLimitExceeded));
        }
        sizes.lock().unwrap().push(size);
        let parse = |cursor: &Option<String>| cursor.as_ref().map(|c| c.parse::<i64>().unwrap());
        let (start, end) = if page.last.is_some() {
            let end = parse(&page.before).unwrap_or(rows);
            ((end - size).max(0), end)
        } else {
            let start = parse(&page.after).map_or(0, |after| after + 1);
            (start, (start + size).min(rows))
        };
        let edges = (start..end)
            .map(|node| Edge {
                node,
                cursor: node.to_string(),
            })
            .collect::<Vec<_>>();
        Ok(Connection {
            page_info: PageInfo {
                has_next_page: end < rows,
                has_previous_page: start > 0,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
                total_count: page.include_total_count.then_some(rows),
            },
            edges,
        })
    }

    #[tokio::test]
    async fn keyset_windows_above_the_maximum_are_read_completely() {
        let nodes = |connection: &Connection<i64>| {
            connection
                .edges
                .iter()
                .map(|edge| edge.node)
                .collect::<Vec<_>>()
        };
        // Forward: 25 rows of 30, in pages of at most 10.
        let sizes = std::sync::Mutex::new(Vec::new());
        let forward = super::keyset_window(
            Some(10),
            KeysetConnectionInput {
                first: Some(25),
                include_total_count: true,
                ..Default::default()
            },
            |page| std::future::ready(fake_keyset(30, 10, &sizes, page)),
        )
        .await
        .expect("window is read in pages");
        assert_eq!(nodes(&forward), (0..25).collect::<Vec<_>>());
        assert_eq!(*sizes.lock().unwrap(), [10, 10, 5]);
        assert!(forward.page_info.has_next_page && !forward.page_info.has_previous_page);
        assert_eq!(forward.page_info.start_cursor.as_deref(), Some("0"));
        assert_eq!(forward.page_info.end_cursor.as_deref(), Some("24"));
        assert_eq!(forward.page_info.total_count, Some(30));

        // Backward from a cursor: the 25 rows before 28, in canonical order.
        let sizes = std::sync::Mutex::new(Vec::new());
        let backward = super::keyset_window(
            Some(10),
            KeysetConnectionInput {
                last: Some(25),
                before: Some("28".to_owned()),
                ..Default::default()
            },
            |page| std::future::ready(fake_keyset(30, 10, &sizes, page)),
        )
        .await
        .expect("window is read in pages");
        assert_eq!(nodes(&backward), (3..28).collect::<Vec<_>>());
        assert_eq!(*sizes.lock().unwrap(), [10, 10, 5]);
        assert!(backward.page_info.has_next_page && backward.page_info.has_previous_page);
        assert_eq!(backward.page_info.start_cursor.as_deref(), Some("3"));
        assert_eq!(backward.page_info.end_cursor.as_deref(), Some("27"));

        // A window larger than the set ends with the set.
        let sizes = std::sync::Mutex::new(Vec::new());
        let all = super::keyset_window(
            Some(10),
            KeysetConnectionInput {
                first: Some(200),
                ..Default::default()
            },
            |page| std::future::ready(fake_keyset(30, 10, &sizes, page)),
        )
        .await
        .expect("window is read in pages");
        assert_eq!(nodes(&all), (0..30).collect::<Vec<_>>());
        assert!(!all.page_info.has_next_page);

        // Within the maximum, or without one, the page is passed through.
        let sizes = std::sync::Mutex::new(Vec::new());
        super::keyset_window(
            None,
            KeysetConnectionInput {
                first: Some(7),
                ..Default::default()
            },
            |page| std::future::ready(fake_keyset(30, 10, &sizes, page)),
        )
        .await
        .expect("one page");
        assert_eq!(*sizes.lock().unwrap(), [7]);
    }

    #[test]
    fn client_pages_follow_the_host_maximum() {
        assert_eq!(client_page_limit(PaginationConfig::default(), 500), 100);
        assert_eq!(client_page_limit(PaginationConfig::default(), 7), 7);
        assert_eq!(
            client_page_limit(PaginationConfig::explicit_only(7), 100),
            7
        );
        assert_eq!(client_page_limit(PaginationConfig::unbounded(), 500), 500);
    }
}
