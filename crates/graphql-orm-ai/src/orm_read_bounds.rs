//! Row bounds for the runtime's own ORM reads.
//!
//! Every internal read carries its own bound: a constant, a `bound + 1`
//! look-ahead, or a deployment limit that this crate validates. ORM 0.128.0
//! refuses a repository limit above the database's maximum page limit instead
//! of reducing it, so each internal read raises the maximum for itself only.
//! The read stays bounded by its own limit, and the database-wide maximum that
//! GraphQL clients see is unchanged.

use graphql_orm::graphql::orm::{
    DatabaseFilter, DatabaseOrderBy, Entity, FromSqlRow, MutationQuery, OrmBackend, ReadProjection,
    RepositoryQuery, TransactionProjectionQuery, WriteBackend,
};

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

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::{InternalReadBound, client_page_limit};
    use crate::persistence::{AiInboxStreamRecord, CreateAiInboxStreamRecordInput};
    use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
    use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule, TransactionMode};
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
