// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]

//! Rust repository builders never return fewer rows than an explicit `limit(n)`
//! asks for because of the database page maximum. A limit above the maximum
//! fails with `PAGE_LIMIT_EXCEEDED`, so a `limit(bound + 1)` overflow check
//! either sees the extra row or gets an error.

use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::graphql::orm::{FindQuery, WriteInputContext};
use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[repository_entity(
    backend = "sqlite",
    table = "limit_rows",
    plural = "LimitRows",
    default_sort = "id ASC",
    keyset = "id asc"
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61"))]
#[graphql_orm(projection(name = "LimitRowProjection", fields = [id, bucket]))]
struct LimitRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    bucket: String,
    #[graphql_orm(searchable(weight = "A"))]
    body: String,
}

/// More rows than the default page maximum (`100`).
const MANY: usize = 150;
/// A caller-side bound above the default page maximum, as in the report.
const BOUND: i64 = 128;

async fn database() -> Database<SqliteBackend> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap()
        .with_schema_policy(SchemaPolicy::Managed);
    assert_eq!(
        database.pagination_config().max_limit,
        Some(PaginationConfig::SECURE_MAX_LIMIT)
    );
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "repository-explicit-limits",
            "repository explicit limits",
            &[LimitRow::metadata()],
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    for index in 0..MANY {
        // Sixty rows in bucket "small" (between the old default page of 50 and
        // the maximum of 100); the rest in bucket "large".
        let bucket = if index < 60 { "small" } else { "large" };
        // Generated inserts also maintain the full-text index.
        LimitRow::insert(
            &database,
            CreateLimitRowInput {
                id: format!("row-{index:04}"),
                bucket: bucket.to_string(),
                body: "needle".to_string(),
            },
        )
        .await
        .unwrap();
    }
    database
}

/// A handle whose maximum admits the caller's `bound + 1` look-ahead.
fn raised(database: &Database<SqliteBackend>) -> Database<SqliteBackend> {
    database
        .clone()
        .with_pagination_config(database.pagination_config().with_max_limit(Some(1_000)))
}

fn bucket(name: &str) -> LimitRowWhereInput {
    LimitRowWhereInput {
        bucket: Some(StringFilter {
            eq: Some(name.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn search() -> SearchInput {
    SearchInput {
        query: "needle".to_string(),
        mode: Some(SearchMode::Plain),
        min_score: None,
    }
}

#[track_caller]
fn assert_code<T: std::fmt::Debug>(result: graphql_orm::Result<T>, code: OrmErrorCode) {
    let error = result.expect_err("the read must fail instead of truncating");
    assert_eq!(OrmPublicError::from_sqlx(&error).code, code, "{error:?}");
}

#[tokio::test]
async fn repository_limit_above_maximum_refuses_bound_plus_one_overflow_check() {
    let database = database().await;

    // The reported pattern: read `bound + 1` rows and refuse if more than
    // `bound` come back. Clamping to 100 hid the overflow entirely.
    let result = LimitRow::query(&database)
        .default_order()
        .limit(BOUND + 1)
        .fetch_all()
        .await;
    assert_code(result, OrmErrorCode::PageLimitExceeded);

    // With a maximum that admits the look-ahead, the extra row arrives.
    let raised = raised(&database);
    let rows = LimitRow::query(&raised)
        .default_order()
        .limit(BOUND + 1)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(rows.len() as i64, BOUND + 1);
}

#[tokio::test]
async fn repository_limit_at_or_below_maximum_is_exact() {
    let database = database().await;
    for limit in [0, 1, 99, 100] {
        let rows = LimitRow::query(&database)
            .limit(limit)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(rows.len() as i64, limit);
    }
    assert_code(
        LimitRow::query(&database).limit(101).fetch_all().await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_code(
        LimitRow::query(&database).limit(-1).fetch_all().await,
        OrmErrorCode::InvalidInput,
    );
    // `fetch_first` honours the same explicit-limit contract.
    assert_code(
        LimitRow::query(&database).limit(101).fetch_first().await,
        OrmErrorCode::PageLimitExceeded,
    );
}

#[tokio::test]
async fn repository_fetch_all_without_limit_is_complete_or_refused() {
    let database = database().await;
    // Sixty matching rows: previously cut to the default page of 50.
    let small = LimitRow::query(&database)
        .filter(bucket("small"))
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(small.len(), 60);
    // More matching rows than the maximum: refused rather than truncated.
    assert_code(
        LimitRow::query(&database).fetch_all().await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_code(
        LimitRow::query(&database).offset(10).fetch_all().await,
        OrmErrorCode::PageLimitExceeded,
    );
    // An unbounded handle reads every row.
    let unbounded = database
        .clone()
        .with_pagination_config(PaginationConfig::unbounded());
    assert_eq!(
        LimitRow::query(&unbounded).fetch_all().await.unwrap().len(),
        MANY
    );
    // `fetch_first` and `fetch_optional_one` are not overflow checks.
    assert!(
        LimitRow::query(&database)
            .default_order()
            .fetch_first()
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        LimitRow::query(&database)
            .fetch_optional_one()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn transaction_query_limit_is_exact_or_refused() {
    let database = database().await;
    let refused = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<LimitRow>()
                    .default_order()
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?)
            })
        })
        .await;
    let TransactionError::Rejected(error) = refused.expect_err("limit above maximum") else {
        panic!("expected a rejected transaction");
    };
    assert_eq!(error.code, OrmErrorCode::PageLimitExceeded);

    // A raised maximum is honoured in the transaction too; previously the
    // in-transaction read re-applied the built-in maximum of 100.
    let raised = raised(&database);
    let (bounded, unlimited) = raised
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let bounded = tx
                    .query::<LimitRow>()
                    .default_order()
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?
                    .len();
                let unlimited = tx.query::<LimitRow>().fetch_all().await?.len();
                Ok((bounded, unlimited))
            })
        })
        .await
        .unwrap();
    assert_eq!(bounded as i64, BOUND + 1);
    assert_eq!(unlimited, MANY);

    let small = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<LimitRow>()
                    .filter(bucket("small"))
                    .fetch_all()
                    .await?
                    .len())
            })
        })
        .await
        .unwrap();
    assert_eq!(small, 60);
    let overflow = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move { Ok(tx.query::<LimitRow>().fetch_all().await?.len()) })
        })
        .await;
    let TransactionError::Rejected(error) = overflow.expect_err("too many rows") else {
        panic!("expected a rejected transaction");
    };
    assert_eq!(error.code, OrmErrorCode::PageLimitExceeded);
}

#[tokio::test]
async fn transaction_keyset_page_at_maximum_reports_next_page() {
    let database = database().await;
    let page = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.keyset_page::<LimitRow>(
                    LimitRowWhereInput::default(),
                    KeysetPageInput {
                        limit: Some(100),
                        ..Default::default()
                    },
                )
                .await
            })
        })
        .await
        .unwrap();
    assert_eq!(page.edges.len(), 100);
    assert!(page.page_info.has_next_page);

    let raised = raised(&database);
    let page = raised
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.keyset_page::<LimitRow>(
                    LimitRowWhereInput::default(),
                    KeysetPageInput {
                        limit: Some(BOUND),
                        ..Default::default()
                    },
                )
                .await
            })
        })
        .await
        .unwrap();
    assert_eq!(page.edges.len() as i64, BOUND);
    assert!(page.page_info.has_next_page);
}

#[tokio::test]
async fn write_context_query_limit_is_exact_or_refused() {
    let database = database().await;
    assert_code(
        WriteInputContext::repository(&database, "LimitRow")
            .query::<LimitRow>()
            .limit(BOUND + 1)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let raised = raised(&database);
    let rows = WriteInputContext::repository(&raised, "LimitRow")
        .query::<LimitRow>()
        .limit(BOUND + 1)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(rows.len() as i64, BOUND + 1);

    let (refused, bounded) = raised
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let refused = WriteInputContext::internal("LimitRow", tx)
                    .query::<LimitRow>()
                    .limit(1_001)
                    .fetch_all()
                    .await
                    .map_err(|error| OrmPublicError::from_sqlx(&error).code)
                    .err();
                let bounded = WriteInputContext::internal("LimitRow", tx)
                    .query::<LimitRow>()
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?
                    .len();
                Ok((refused, bounded))
            })
        })
        .await
        .unwrap();
    assert_eq!(refused, Some(OrmErrorCode::PageLimitExceeded));
    assert_eq!(bounded as i64, BOUND + 1);
}

#[tokio::test]
async fn projection_query_limit_is_exact_or_refused() {
    let database = database().await;
    assert_code(
        LimitRowProjection::query(&database)
            .limit(BOUND + 1)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_code(
        LimitRowProjection::query(&database).fetch_all().await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_eq!(
        LimitRowProjection::query(&database)
            .filter(bucket("small"))
            .fetch_all()
            .await
            .unwrap()
            .len(),
        60
    );
    let raised = raised(&database);
    assert_eq!(
        LimitRowProjection::query(&raised)
            .limit(BOUND + 1)
            .fetch_all()
            .await
            .unwrap()
            .len() as i64,
        BOUND + 1
    );
    // The cardinality look-ahead is not clamped by a maximum of one.
    let one = database
        .clone()
        .with_pagination_config(PaginationConfig::explicit_only(1));
    assert!(
        LimitRowProjection::query(&one)
            .fetch_optional_one()
            .await
            .is_err()
    );

    let bounded = raised
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .project::<LimitRowProjection>()
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?
                    .len())
            })
        })
        .await
        .unwrap();
    assert_eq!(bounded as i64, BOUND + 1);
    let refused = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .project::<LimitRowProjection>()
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?
                    .len())
            })
        })
        .await;
    let TransactionError::Rejected(error) = refused.expect_err("limit above maximum") else {
        panic!("expected a rejected transaction");
    };
    assert_eq!(error.code, OrmErrorCode::PageLimitExceeded);
}

#[tokio::test]
async fn repository_search_limit_is_exact_or_refused() {
    let database = database().await;
    assert_code(
        LimitRow::search_db(&database, search())
            .limit(BOUND + 1)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let raised = raised(&database);
    let hits = LimitRow::search_db(&raised, search())
        .limit(BOUND + 1)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(hits.len() as i64, BOUND + 1);
}

#[tokio::test]
async fn pool_bound_find_query_limit_is_exact_or_refused() {
    let database = database().await;
    let query = || {
        FindQuery::<LimitRow, LimitRowWhereInput, LimitRowOrderByInput, SqliteBackend>::new(
            database.pool(),
        )
    };
    assert_code(
        query().limit(BOUND + 1).fetch_all().await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_eq!(query().limit(100).fetch_all().await.unwrap().len(), 100);
    // Without a limit the pool-bound builder stays unbounded, as before.
    assert_eq!(query().fetch_all().await.unwrap().len(), MANY);
}

#[tokio::test]
async fn propagation_target_budget_above_page_maximum_is_enforced() {
    use graphql_orm::graphql::orm::SqlValue;
    use graphql_orm::graphql::subscriptions::propagation_targets;

    let database = database().await;
    // Every row shares `body`, so a budget of 128 must see the 129th target
    // and refuse. The in-transaction read used to stop at 100 rows.
    let (refused, complete) = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let refused = propagation_targets::<LimitRow, SqliteBackend>(
                    tx,
                    &["body"],
                    &[SqlValue::String("needle".to_string())],
                    BOUND as usize,
                )
                .await
                .is_err();
                let complete = propagation_targets::<LimitRow, SqliteBackend>(
                    tx,
                    &["body"],
                    &[SqlValue::String("needle".to_string())],
                    MANY,
                )
                .await?
                .len();
                Ok((refused, complete))
            })
        })
        .await
        .unwrap();
    assert!(refused);
    assert_eq!(complete, MANY);
}

#[tokio::test]
async fn repository_find_all_and_find_many_are_complete_or_refused() {
    let database = database().await;
    // Previously both returned the first 50 rows of the default page.
    assert_code(
        LimitRow::find_all(&database).await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_eq!(
        LimitRow::find_many(&database, bucket("small"))
            .await
            .unwrap()
            .len(),
        60
    );
    assert_eq!(
        LimitRow::find_all(&raised(&database)).await.unwrap().len(),
        MANY
    );
}

#[tokio::test]
async fn per_read_max_limit_scopes_a_trusted_bound_to_one_read() {
    let database = database().await;
    // The read's own bound admits its look-ahead without touching the
    // database-wide maximum that GraphQL uses.
    assert_eq!(
        LimitRow::query(&database)
            .with_max_limit(BOUND + 1)
            .limit(BOUND + 1)
            .fetch_all()
            .await
            .unwrap()
            .len() as i64,
        BOUND + 1
    );
    assert_eq!(
        database.pagination_config().max_limit,
        Some(PaginationConfig::SECURE_MAX_LIMIT)
    );
    // The override still bounds the read.
    assert_code(
        LimitRow::query(&database)
            .with_max_limit(BOUND)
            .limit(BOUND + 1)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_code(
        LimitRow::query(&database)
            .with_max_limit(BOUND)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert_eq!(
        LimitRow::query(&database)
            .with_max_limit(MANY as i64)
            .fetch_all()
            .await
            .unwrap()
            .len(),
        MANY
    );
    assert_eq!(
        LimitRowProjection::query(&database)
            .with_max_limit(BOUND + 1)
            .limit(BOUND + 1)
            .fetch_all()
            .await
            .unwrap()
            .len() as i64,
        BOUND + 1
    );
    assert_eq!(
        FindQuery::<LimitRow, LimitRowWhereInput, LimitRowOrderByInput, SqliteBackend>::new(
            database.pool()
        )
        .with_max_limit(BOUND + 1)
        .limit(BOUND + 1)
        .fetch_all()
        .await
        .unwrap()
        .len() as i64,
        BOUND + 1
    );

    let (query, projection, refused) = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let query = tx
                    .query::<LimitRow>()
                    .with_max_limit(BOUND + 1)
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?
                    .len();
                let projection = tx
                    .project::<LimitRowProjection>()
                    .with_max_limit(BOUND + 1)
                    .limit(BOUND + 1)
                    .fetch_all()
                    .await?
                    .len();
                let refused = tx
                    .query::<LimitRow>()
                    .with_max_limit(BOUND)
                    .fetch_all()
                    .await
                    .map_err(|error| OrmPublicError::from_sqlx(&error).code)
                    .err();
                Ok((query, projection, refused))
            })
        })
        .await
        .unwrap();
    assert_eq!(query as i64, BOUND + 1);
    assert_eq!(projection as i64, BOUND + 1);
    assert_eq!(refused, Some(OrmErrorCode::PageLimitExceeded));
}

#[tokio::test]
async fn fail_closed_overflow_does_not_disclose_a_row_count() {
    let database = database().await;
    let error = LimitRow::query(&database)
        .fetch_all()
        .await
        .expect_err("more rows than the maximum match");
    let public = OrmPublicError::from_sqlx(&error);
    assert_eq!(public.code, OrmErrorCode::PageLimitExceeded);
    assert!(
        !public.message.chars().any(|c| c.is_ascii_digit()),
        "{}",
        public.message
    );
}
