// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

//! Rust repository reads whose row checks run in Rust (`RowPolicy::can_read_row`)
//! define `limit` and `offset` over the rows the caller can see. A page is
//! complete, or the read fails with `PAGE_LIMIT_EXCEEDED` when its candidate
//! row budget runs out first. Reads with no residual row check keep one SQL
//! `LIMIT`/`OFFSET` query.

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "visible_page_rows",
        plural = "VisiblePageRows",
        default_sort = "rank ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "visible_page_rows",
        plural = "VisiblePageRows",
        default_sort = "rank ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "visible_page_rows",
        plural = "VisiblePageRows",
        default_sort = "rank ASC"
    )
)]
struct PageRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    rank: i64,
    #[filterable(type = "string")]
    owner: String,
}

/// Native full-text search over `visible_native_rows`.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "visible_native_rows",
        plural = "VisibleNativeRows",
        default_sort = "seq ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "visible_native_rows",
        plural = "VisibleNativeRows",
        default_sort = "seq ASC"
    )
)]
#[graphql_orm(search(index = true))]
struct NativeSearchRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    seq: i64,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(searchable(weight = "A"))]
    body: String,
}

/// Fallback (Rust-scored) search over `visible_fallback_rows`, the only
/// search strategy on SQL Server.
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "visible_fallback_rows",
        plural = "VisibleFallbackRows",
        default_sort = "seq ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "visible_fallback_rows",
        plural = "VisibleFallbackRows",
        default_sort = "seq ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "visible_fallback_rows",
        plural = "VisibleFallbackRows",
        default_sort = "seq ASC"
    )
)]
#[graphql_orm(search(index = false))]
struct FallbackSearchRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    seq: i64,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(searchable(weight = "A"))]
    body: String,
}

/// An entity whose table is never created: any read of it fails.
#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "visible_missing_rows",
        plural = "VisibleMissingRows",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "visible_missing_rows",
        plural = "VisibleMissingRows",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    feature = "mssql",
    repository_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "visible_missing_rows",
        plural = "VisibleMissingRows",
        default_sort = "id ASC"
    )
)]
#[graphql_orm(search(index = false))]
struct MissingRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[graphql_orm(searchable(weight = "A"))]
    body: String,
}

/// Rows `0..ROWS`; every third one (`rank % 3 == 0`) is visible.
const ROWS: i64 = 30;

fn visible_ranks() -> Vec<i64> {
    (0..ROWS).filter(|rank| rank % 3 == 0).collect()
}

fn ranks(rows: &[PageRow]) -> Vec<i64> {
    rows.iter().map(|row| row.rank).collect()
}

/// What the row policy reports for repository reads.
#[derive(Clone, Copy)]
enum Decision {
    /// `CallbackOnly`: `can_read_row` decides every row.
    Residual,
    /// `Unrestricted`, with a callback that allows every row.
    Unrestricted,
    /// `Unrestricted`, with a callback that still refuses rows.
    UnrestrictedRefusing,
}

struct HideRows {
    decision: Decision,
    checks: Arc<AtomicUsize>,
}

fn row_is_visible(row: &(dyn std::any::Any + Send + Sync)) -> bool {
    if let Some(row) = row.downcast_ref::<PageRow>() {
        return row.owner == "shown";
    }
    if let Some(row) = row.downcast_ref::<FallbackSearchRow>() {
        return row.owner == "shown";
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    if let Some(row) = row.downcast_ref::<NativeSearchRow>() {
        return row.owner == "shown";
    }
    false
}

impl RowPolicy for HideRows {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        let decision = self.decision;
        Box::pin(async move {
            Ok(match decision {
                Decision::Residual => ReadVisibility::CallbackOnly,
                Decision::Unrestricted | Decision::UnrestrictedRefusing => {
                    ReadVisibility::Unrestricted
                }
            })
        })
    }

    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let visible = match self.decision {
            Decision::Unrestricted => true,
            Decision::Residual | Decision::UnrestrictedRefusing => row_is_visible(row),
        };
        Box::pin(async move { Ok(visible) })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// Records the SQL text and row count of every observed read.
#[derive(Clone, Default)]
struct CountReads(Arc<std::sync::Mutex<Vec<(String, usize)>>>);

impl ReadQueryObserver for CountReads {
    fn on_read(&self, sql: &str, rows: usize) {
        self.0.lock().unwrap().push((sql.to_owned(), rows));
    }
}

impl CountReads {
    fn reads(&self) -> Vec<(String, usize)> {
        self.0.lock().unwrap().clone()
    }
}

fn with_policy(database: &Database, decision: Decision) -> (Database, Arc<AtomicUsize>) {
    let checks = Arc::new(AtomicUsize::new(0));
    let mut database = database.clone();
    database.set_row_policy(HideRows {
        decision,
        checks: checks.clone(),
    });
    (database, checks)
}

#[track_caller]
fn assert_code<T: std::fmt::Debug>(result: graphql_orm::Result<T>, code: OrmErrorCode) {
    let error = result.expect_err("the read must fail instead of returning a short page");
    assert_eq!(OrmPublicError::from_sqlx(&error).code, code, "{error:?}");
}

#[cfg(feature = "mssql")]
struct AllowEntityPolicy;
#[cfg(feature = "mssql")]
impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

#[cfg(feature = "sqlite")]
struct FixtureOwner;
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

async fn fixture() -> Result<(Database, FixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    let (database, owner) = (
        Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?,
        FixtureOwner,
    );
    #[cfg(feature = "postgres")]
    let (database, owner) = {
        let owner = FixtureOwner::start("visible-row-pages")?;
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await?;
        (Database::new(pool), owner)
    };
    #[cfg(feature = "mssql")]
    let (database, owner) = {
        let owner = FixtureOwner::start().await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let mut database = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        database.set_entity_policy(AllowEntityPolicy);
        (database, owner)
    };
    execute(
        &database,
        "CREATE TABLE visible_page_rows (id VARCHAR(40) PRIMARY KEY, rank BIGINT NOT NULL, owner VARCHAR(40) NOT NULL)",
    )
    .await?;
    for table in ["visible_fallback_rows", "visible_native_rows"] {
        #[cfg(feature = "mssql")]
        if table == "visible_native_rows" {
            continue;
        }
        execute(
            &database,
            &format!("CREATE TABLE {table} (id VARCHAR(40) PRIMARY KEY, seq BIGINT NOT NULL, owner VARCHAR(40) NOT NULL, body VARCHAR(40) NOT NULL)"),
        )
        .await?;
    }
    #[cfg(feature = "sqlite")]
    execute(&database, "CREATE VIRTUAL TABLE __graphql_orm_fts_visible_native_rows USING fts5(entity_pk UNINDEXED, weight_a, weight_b, weight_c, weight_d, document_text)").await?;
    #[cfg(feature = "postgres")]
    execute(&database, "CREATE TABLE __graphql_orm_search_visible_native_rows (entity_pk TEXT PRIMARY KEY, document_vector TSVECTOR NOT NULL)").await?;
    let rows = (0..ROWS)
        .map(|rank| (rank, rank % 3 == 0))
        .collect::<Vec<_>>();
    insert_rows(&database, &rows).await?;
    Ok((database, owner))
}

/// Insert `(rank, visible)` rows into the entity table and both search tables.
async fn insert_rows(database: &Database, rows: &[(i64, bool)]) -> graphql_orm::Result<()> {
    let owner = |visible: bool| if visible { "shown" } else { "hidden" };
    let values = rows
        .iter()
        .map(|(rank, visible)| format!("('r{rank:03}', {rank}, '{}')", owner(*visible)))
        .collect::<Vec<_>>()
        .join(", ");
    execute(
        database,
        &format!("INSERT INTO visible_page_rows (id, rank, owner) VALUES {values}"),
    )
    .await?;
    let values = rows
        .iter()
        .map(|(rank, visible)| format!("('r{rank:03}', {rank}, '{}', 'needle')", owner(*visible)))
        .collect::<Vec<_>>()
        .join(", ");
    execute(
        database,
        &format!("INSERT INTO visible_fallback_rows (id, seq, owner, body) VALUES {values}"),
    )
    .await?;
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        execute(
            database,
            &format!("INSERT INTO visible_native_rows (id, seq, owner, body) VALUES {values}"),
        )
        .await?;
        for (rank, _) in rows {
            #[cfg(feature = "sqlite")]
            execute(database, &format!("INSERT INTO __graphql_orm_fts_visible_native_rows VALUES ('r{rank:03}', 'needle', '', '', '', 'needle')")).await?;
            #[cfg(feature = "postgres")]
            execute(database, &format!("INSERT INTO __graphql_orm_search_visible_native_rows VALUES ('r{rank:03}', to_tsvector('english', 'needle'))")).await?;
        }
    }
    Ok(())
}

async fn execute(database: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(database.pool(), sql, &[]).await?;
    Ok(())
}

/// `limit(n)` counts visible rows, so a `limit(bound + 1)` overflow check
/// sees the extra visible row even when hidden rows are interleaved.
async fn residual_limit_counts_visible_rows(database: &Database) {
    let (database, _) = with_policy(database, Decision::Residual);
    let rows = PageRow::query(&database)
        .default_order()
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![0, 3, 6, 9]);

    // Ten rows are visible. With `bound = 9`, `limit(bound + 1)` must return
    // all ten so the caller detects the overflow.
    let bound = 9;
    let rows = PageRow::query(&database)
        .default_order()
        .limit(bound + 1)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), visible_ranks());
    assert!(rows.len() as i64 > bound);

    // Asking for more than exist returns every visible row, not an error.
    let rows = PageRow::query(&database)
        .default_order()
        .limit(25)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), visible_ranks());
}

/// `offset(k)` skips `k` visible rows.
async fn residual_offset_counts_visible_rows(database: &Database) {
    let (database, _) = with_policy(database, Decision::Residual);
    let page = |offset: i64, limit: i64| {
        let database = database.clone();
        async move {
            ranks(
                &PageRow::query(&database)
                    .default_order()
                    .offset(offset)
                    .limit(limit)
                    .fetch_all()
                    .await
                    .unwrap(),
            )
        }
    };
    assert_eq!(page(2, 3).await, vec![6, 9, 12]);
    assert_eq!(page(8, 5).await, vec![24, 27]);
    assert!(page(10, 5).await.is_empty());

    // Consecutive pages cover every visible row exactly once.
    let mut seen = Vec::new();
    for offset in (0..12).step_by(4) {
        seen.extend(page(offset, 4).await);
    }
    assert_eq!(seen, visible_ranks());

    // An unlimited read starts after the skipped visible rows.
    let rows = PageRow::query(&database)
        .default_order()
        .offset(2)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), visible_ranks()[2..].to_vec());

    // The unlimited read's fail-closed check still counts rows before row
    // policy: 30 rows match, ten are visible, the maximum is 20.
    let capped = database
        .clone()
        .with_pagination_config(database.pagination_config().with_max_limit(Some(20)));
    let refused = PageRow::query(&capped).default_order().fetch_all().await;
    let error = refused.expect_err("more rows than the maximum match");
    let public = OrmPublicError::from_sqlx(&error);
    assert_eq!(public.code, OrmErrorCode::PageLimitExceeded);
    assert!(
        !public.message.chars().any(|c| c.is_ascii_digit()),
        "{}",
        public.message
    );
    assert_code(
        PageRow::query(&capped)
            .default_order()
            .offset(2)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
}

/// `fetch_first` and `fetch_optional_one` use the same visible-row window.
async fn residual_single_row_reads_use_visible_rows(database: &Database) {
    let (database, _) = with_policy(database, Decision::Residual);
    let first = PageRow::query(&database)
        .default_order()
        .offset(1)
        .fetch_first()
        .await
        .unwrap();
    assert_eq!(first.map(|row| row.rank), Some(3));

    // One visible row remains after skipping nine.
    let only = PageRow::query(&database)
        .default_order()
        .offset(9)
        .fetch_optional_one()
        .await
        .unwrap();
    assert_eq!(only.map(|row| row.rank), Some(27));
    // Two visible rows remain after skipping eight: not unique.
    assert!(
        PageRow::query(&database)
            .default_order()
            .offset(8)
            .fetch_optional_one()
            .await
            .is_err()
    );
}

/// A read that runs out of scan budget before its page is complete fails
/// instead of returning a short page.
async fn residual_scan_budget_refuses_short_pages(database: &Database) {
    let (database, _) = with_policy(database, Decision::Residual);
    // Four visible rows need ten examined rows (ranks 0..=9).
    assert_code(
        PageRow::query(&database)
            .default_order()
            .with_max_scanned_rows(9)
            .limit(4)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let rows = PageRow::query(&database)
        .default_order()
        .with_max_scanned_rows(10)
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![0, 3, 6, 9]);
    // Skipped visible rows count toward the budget too.
    assert_code(
        PageRow::query(&database)
            .default_order()
            .with_max_scanned_rows(10)
            .offset(2)
            .limit(4)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    // A budget that covers every matching row returns a complete short page.
    let rows = PageRow::query(&database)
        .default_order()
        .with_max_scanned_rows(ROWS)
        .limit(20)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), visible_ranks());
    // Single-row reads and the database-wide budget follow the same rule.
    assert_code(
        PageRow::query(&database)
            .default_order()
            .with_max_scanned_rows(3)
            .offset(1)
            .fetch_first()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let tight = database
        .clone()
        .with_pagination_config(database.pagination_config().with_max_scanned_rows(Some(5)));
    let refused = PageRow::query(&tight)
        .default_order()
        .limit(4)
        .fetch_all()
        .await;
    let error = refused.expect_err("the budget runs out first");
    let public = OrmPublicError::from_sqlx(&error);
    assert_eq!(public.code, OrmErrorCode::PageLimitExceeded);
    assert!(
        !public.message.chars().any(|c| c.is_ascii_digit()),
        "{}",
        public.message
    );
    let refused = tight
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<PageRow>()
                    .default_order()
                    .limit(4)
                    .fetch_all()
                    .await?)
            })
        })
        .await;
    let TransactionError::Rejected(error) = refused.expect_err("the budget runs out first") else {
        panic!("expected a rejected transaction");
    };
    assert_eq!(error.code, OrmErrorCode::PageLimitExceeded);
    let rows = tight
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<PageRow>()
                    .default_order()
                    .with_max_scanned_rows(10)
                    .limit(4)
                    .fetch_all()
                    .await?)
            })
        })
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![0, 3, 6, 9]);
}

/// Transaction reads follow the same visible-row rule.
async fn transaction_query_counts_visible_rows(database: &Database) {
    let (database, _) = with_policy(database, Decision::Residual);
    let (limited, offset, first) = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let limited = tx
                    .query::<PageRow>()
                    .default_order()
                    .limit(4)
                    .fetch_all()
                    .await?;
                let offset = tx
                    .query::<PageRow>()
                    .default_order()
                    .offset(2)
                    .limit(3)
                    .fetch_all()
                    .await?;
                let first = tx
                    .query::<PageRow>()
                    .default_order()
                    .offset(1)
                    .fetch_one()
                    .await?;
                Ok((limited, offset, first))
            })
        })
        .await
        .unwrap();
    assert_eq!(ranks(&limited), vec![0, 3, 6, 9]);
    assert_eq!(ranks(&offset), vec![6, 9, 12]);
    assert_eq!(first.map(|row| row.rank), Some(3));
}

/// Without a residual row check, `limit` and `offset` stay one SQL query.
async fn sql_authoritative_reads_keep_one_query(database: &Database) {
    for decision in [None, Some(Decision::Unrestricted)] {
        let reads = CountReads::default();
        let (database, checks) = match decision {
            Some(decision) => with_policy(database, decision),
            None => (database.clone(), Arc::new(AtomicUsize::new(0))),
        };
        let database = database.with_read_query_observer(reads.clone());
        let rows = PageRow::query(&database)
            .default_order()
            .offset(2)
            .limit(4)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(ranks(&rows), vec![2, 3, 4, 5]);
        assert_eq!(reads.reads().len(), 1);
        if decision.is_some() {
            assert_eq!(checks.load(Ordering::SeqCst), 4);
        }
    }
}

/// A callback that refuses rows under an SQL-authoritative decision still
/// cannot shorten the page.
async fn refusing_callback_still_fills_the_page(database: &Database) {
    let (database, _) = with_policy(database, Decision::UnrestrictedRefusing);
    let rows = PageRow::query(&database)
        .default_order()
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![0, 3, 6, 9]);
}

/// An unlimited read with an offset keeps every visible row after the skipped
/// ones, even when the skip completes inside the last, short batch.
async fn unlimited_offset_keeps_rows_after_the_skip(database: &Database) {
    // hidden, hidden, hidden, visible, visible: batches of 1, 2 and 4.
    insert_rows(
        database,
        &[
            (100, false),
            (101, false),
            (102, false),
            (103, true),
            (104, true),
        ],
    )
    .await
    .unwrap();
    let (database, _) = with_policy(database, Decision::Residual);
    let from = |rank: i32| IntFilter {
        gte: Some(rank),
        ..Default::default()
    };
    let rows = PageRow::query(&database)
        .filter(PageRowWhereInput {
            rank: Some(from(100)),
            ..Default::default()
        })
        .default_order()
        .offset(1)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![104]);

    let rows = database
        .transaction(TransactionMode::StateMachine, move |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<PageRow>()
                    .filter(PageRowWhereInput {
                        rank: Some(from(100)),
                        ..Default::default()
                    })
                    .default_order()
                    .offset(1)
                    .fetch_all()
                    .await?)
            })
        })
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![104]);

    let hits = FallbackSearchRow::search_db(&database, needle())
        .filter(FallbackSearchRowWhereInput {
            seq: Some(from(100)),
            ..Default::default()
        })
        .default_order()
        .offset(1)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(fallback_seqs(&hits), vec![104]);
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let hits = NativeSearchRow::search_db(&database, needle())
            .filter(NativeSearchRowWhereInput {
                seq: Some(from(100)),
                ..Default::default()
            })
            .offset(1)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.entity.seq).collect::<Vec<_>>(),
            vec![104]
        );
    }
}

/// A residual in-memory filter: even ranks only, decided in Rust.
#[derive(Clone)]
struct EvenRanks;

impl DatabaseFilter for EvenRanks {
    fn to_sql_conditions(&self) -> (Vec<String>, Vec<SqlValue>) {
        (Vec::new(), Vec::new())
    }
    fn is_empty(&self) -> bool {
        false
    }
    fn requires_in_memory_filtering(&self, _: DatabaseBackend) -> bool {
        true
    }
    fn matches_entity(
        &self,
        entity: &(dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::Result<bool> {
        Ok(entity
            .downcast_ref::<PageRow>()
            .is_some_and(|row| row.rank % 2 == 0))
    }
}

/// With a residual in-memory filter, the scan still pages candidate rows in
/// SQL, so the budget bounds the rows read from the database.
async fn in_memory_filters_keep_candidate_reads_bounded(database: &Database) {
    let reads = CountReads::default();
    let (database, _) = with_policy(database, Decision::Residual);
    let database = database.with_read_query_observer(reads.clone());
    let query = || {
        RepositoryQuery::<PageRow, EvenRanks, PageRowOrderByInput, DefaultBackend>::new(&database)
            .filter(EvenRanks)
            .default_order()
    };
    // Visible and even: ranks 0, 6, 12, 18 and 24.
    let rows = query().limit(3).fetch_all().await.unwrap();
    assert_eq!(ranks(&rows), vec![0, 6, 12]);
    let rows = query().offset(1).limit(2).fetch_all().await.unwrap();
    assert_eq!(ranks(&rows), vec![6, 12]);
    // Three rows need thirteen candidates (ranks 0..=12).
    let before = reads.reads().len();
    assert_code(
        query().with_max_scanned_rows(12).limit(3).fetch_all().await,
        OrmErrorCode::PageLimitExceeded,
    );
    let refused = &reads.reads()[before..];
    assert!(
        refused.iter().map(|(_, rows)| rows).sum::<usize>() <= 13,
        "{refused:?}"
    );
    let before = reads.reads().len();
    let rows = query()
        .with_max_scanned_rows(13)
        .limit(3)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![0, 6, 12]);
    let served = &reads.reads()[before..];
    assert!(
        served.iter().map(|(_, rows)| rows).sum::<usize>() <= 14,
        "{served:?}"
    );
    // An unlimited read with an offset applies the filter to the rest.
    let rows = query().offset(2).fetch_all().await.unwrap();
    assert_eq!(ranks(&rows), vec![12, 18, 24]);
}

/// An order whose terms only mention the key still gets a key tiebreak, so
/// scan batches over tied rows never skip or repeat one.
#[derive(Clone, Copy)]
enum TiedOrder {
    /// Every row sorts equal; the expression mentions the key.
    MentionsKey,
    /// Every row sorts as NULL.
    AllNull,
}

impl DatabaseOrderBy for TiedOrder {
    fn to_sql_order(&self) -> Option<String> {
        Some(match self {
            Self::MentionsKey => "CASE WHEN id <> '' THEN 0 ELSE 1 END ASC".to_string(),
            Self::AllNull => "CASE WHEN rank > 1000 THEN rank END ASC".to_string(),
        })
    }
}

async fn tied_orders_get_a_key_tiebreak(database: &Database) {
    for order in [TiedOrder::MentionsKey, TiedOrder::AllNull] {
        let reads = CountReads::default();
        let (database, _) = with_policy(database, Decision::Residual);
        let database = database.with_read_query_observer(reads.clone());
        let mut seen = Vec::new();
        for offset in (0..12).step_by(3) {
            let rows =
                RepositoryQuery::<PageRow, PageRowWhereInput, TiedOrder, DefaultBackend>::new(
                    &database,
                )
                .order_by(order)
                .offset(offset)
                .limit(3)
                .fetch_all()
                .await
                .unwrap();
            seen.extend(ranks(&rows));
        }
        seen.sort_unstable();
        assert_eq!(seen, visible_ranks());
        for (sql, _) in reads.reads() {
            assert!(
                ["id", "\"id\"", "[id]"]
                    .iter()
                    .any(|key| sql.contains(&format!("END ASC, {key} ASC"))),
                "{sql}"
            );
        }
    }
}

/// A key term ordered directly by a qualified, spaced or quoted reference.
#[derive(Clone, Copy)]
enum QualifiedKey {
    /// `visible_page_rows . id DESC`, bare and spaced.
    Spaced,
    /// The dialect's quoted spelling, spaced.
    Quoted,
}

impl DatabaseOrderBy for QualifiedKey {
    fn to_sql_order(&self) -> Option<String> {
        Some(match self {
            Self::Spaced => "visible_page_rows . id DESC".to_string(),
            #[cfg(feature = "mssql")]
            Self::Quoted => "[visible_page_rows] . [id] DESC".to_string(),
            #[cfg(not(feature = "mssql"))]
            Self::Quoted => "\"visible_page_rows\" . \"id\" DESC".to_string(),
        })
    }
}

/// A qualified key term is a direct key term, so no second key term is
/// appended (SQL Server refuses a repeated order column).
async fn qualified_key_orders_get_no_second_key_term(database: &Database) {
    for order in [QualifiedKey::Spaced, QualifiedKey::Quoted] {
        let reads = CountReads::default();
        let (database, _) = with_policy(database, Decision::Residual);
        let database = database.with_read_query_observer(reads.clone());
        let rows =
            RepositoryQuery::<PageRow, PageRowWhereInput, QualifiedKey, DefaultBackend>::new(
                &database,
            )
            .order_by(order)
            .limit(4)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(ranks(&rows), vec![27, 24, 21, 18]);
        for (sql, _) in reads.reads() {
            assert!(!sql.contains("DESC,"), "{sql}");
        }
    }
}

/// Total candidate rows the observed reads took from the database.
fn rows_read(reads: &CountReads, since: usize) -> usize {
    reads.reads()[since..].iter().map(|(_, rows)| rows).sum()
}

/// One candidate budget covers an unlimited read too, offset scan and rest.
async fn unlimited_reads_share_the_candidate_budget(database: &Database) {
    let reads = CountReads::default();
    let (database, _) = with_policy(database, Decision::Residual);
    let database = database.with_read_query_observer(reads.clone());

    // Thirty candidates: a budget of 30 reads them all, 29 refuses.
    let before = reads.reads().len();
    let rows = PageRow::query(&database)
        .default_order()
        .with_max_scanned_rows(30)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), visible_ranks());
    assert!(rows_read(&reads, before) <= 31);
    let before = reads.reads().len();
    assert_code(
        PageRow::query(&database)
            .default_order()
            .with_max_scanned_rows(29)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert!(rows_read(&reads, before) <= 30, "{:?}", reads.reads());
    let before = reads.reads().len();
    assert_code(
        PageRow::query(&database)
            .default_order()
            .offset(2)
            .with_max_scanned_rows(10)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert!(rows_read(&reads, before) <= 11, "{:?}", reads.reads());

    // A residual in-memory predicate does not lift the bound.
    let before = reads.reads().len();
    assert_code(
        RepositoryQuery::<PageRow, EvenRanks, PageRowOrderByInput, DefaultBackend>::new(&database)
            .filter(EvenRanks)
            .default_order()
            .offset(1)
            .with_max_scanned_rows(10)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert!(rows_read(&reads, before) <= 11, "{:?}", reads.reads());

    let refused = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<PageRow>()
                    .default_order()
                    .offset(1)
                    .with_max_scanned_rows(5)
                    .fetch_all()
                    .await?)
            })
        })
        .await;
    let TransactionError::Rejected(error) = refused.expect_err("the budget runs out first") else {
        panic!("expected a rejected transaction");
    };
    assert_eq!(error.code, OrmErrorCode::PageLimitExceeded);

    // A zero budget on an unlimited search: one look-ahead row, then refused.
    let before = reads.reads().len();
    assert_code(
        FallbackSearchRow::search_db(&database, needle())
            .with_max_scanned_rows(0)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert!(rows_read(&reads, before) <= 1, "{:?}", reads.reads());
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let before = reads.reads().len();
        assert_code(
            NativeSearchRow::search_db(&database, needle())
                .with_max_scanned_rows(0)
                .fetch_all()
                .await,
            OrmErrorCode::PageLimitExceeded,
        );
        assert!(rows_read(&reads, before) <= 1, "{:?}", reads.reads());
        let before = reads.reads().len();
        assert_code(
            NativeSearchRow::search_db(&database, needle())
                .offset(2)
                .with_max_scanned_rows(10)
                .fetch_all()
                .await,
            OrmErrorCode::PageLimitExceeded,
        );
        assert!(rows_read(&reads, before) <= 11, "{:?}", reads.reads());
        let hits = NativeSearchRow::search_db(&database, needle())
            .offset(2)
            .with_max_scanned_rows(30)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.entity.seq).collect::<Vec<_>>(),
            visible_ranks()[2..].to_vec()
        );
    }
}

/// The rescan of an SQL-authoritative page counts the window it already read.
async fn rescans_count_the_first_window(database: &Database) {
    let reads = CountReads::default();
    let (database, _) = with_policy(database, Decision::UnrestrictedRefusing);
    let database = database.with_read_query_observer(reads.clone());
    // The window reads four rows; four visible rows then need ten more.
    let before = reads.reads().len();
    assert_code(
        PageRow::query(&database)
            .default_order()
            .with_max_scanned_rows(12)
            .limit(4)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert!(rows_read(&reads, before) <= 13, "{:?}", reads.reads());
    let before = reads.reads().len();
    let rows = PageRow::query(&database)
        .default_order()
        .with_max_scanned_rows(14)
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(ranks(&rows), vec![0, 3, 6, 9]);
    assert!(rows_read(&reads, before) <= 15, "{:?}", reads.reads());

    let refused = database
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                Ok(tx
                    .query::<PageRow>()
                    .default_order()
                    .with_max_scanned_rows(12)
                    .limit(4)
                    .fetch_all()
                    .await?)
            })
        })
        .await;
    let TransactionError::Rejected(error) = refused.expect_err("the budget runs out first") else {
        panic!("expected a rejected transaction");
    };
    assert_eq!(error.code, OrmErrorCode::PageLimitExceeded);

    // Fallback search: the first read loads every candidate, as without a
    // row check. Its thirty candidates count against the rescan, which then
    // reads at most what is left of the budget plus one.
    let before = reads.reads().len();
    assert_code(
        FallbackSearchRow::search_db(&database, needle())
            .default_order()
            .with_max_scanned_rows(30)
            .limit(4)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    assert!(rows_read(&reads, before) <= 1, "{:?}", reads.reads());
    let before = reads.reads().len();
    let hits = FallbackSearchRow::search_db(&database, needle())
        .default_order()
        .with_max_scanned_rows(60)
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(fallback_seqs(&hits), vec![0, 3, 6, 9]);
    assert!(rows_read(&reads, before) <= 31, "{:?}", reads.reads());
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    {
        let before = reads.reads().len();
        assert_code(
            NativeSearchRow::search_db(&database, needle())
                .with_max_scanned_rows(12)
                .limit(4)
                .fetch_all()
                .await,
            OrmErrorCode::PageLimitExceeded,
        );
        assert!(rows_read(&reads, before) <= 13, "{:?}", reads.reads());
    }
}

/// Reads with no residual row check never scan, so the budget neither bounds
/// nor refuses them: with no row policy, and with an `Unrestricted` decision
/// whose callback allows every row.
async fn reads_without_a_residual_check_ignore_the_budget(database: &Database) {
    let unpoliced = database.clone();
    let (unrestricted, _) = with_policy(database, Decision::Unrestricted);
    for database in [unpoliced, unrestricted] {
        let reads = CountReads::default();
        let database = database
            .clone()
            .with_pagination_config(database.pagination_config().with_max_scanned_rows(Some(5)))
            .with_read_query_observer(reads.clone());
        // Thirty fallback candidates, a budget of five: served as on the
        // single-query path.
        let hits = FallbackSearchRow::search_db(&database, needle())
            .default_order()
            .limit(4)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(fallback_seqs(&hits), vec![0, 1, 2, 3]);
        let hits = FallbackSearchRow::search_db(&database, needle())
            .default_order()
            .offset(2)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(hits.len(), ROWS as usize - 2);
        #[cfg(any(feature = "sqlite", feature = "postgres"))]
        {
            let hits = NativeSearchRow::search_db(&database, needle())
                .limit(8)
                .fetch_all()
                .await
                .unwrap();
            assert_eq!(hits.len(), 8);
        }
        // A bounded read with a residual in-memory predicate is still one
        // read of every candidate, not a scan.
        let before = reads.reads().len();
        let rows = RepositoryQuery::<PageRow, EvenRanks, PageRowOrderByInput, DefaultBackend>::new(
            &database,
        )
        .filter(EvenRanks)
        .default_order()
        .offset(1)
        .limit(3)
        .fetch_all()
        .await
        .unwrap();
        assert_eq!(ranks(&rows), vec![2, 4, 6]);
        let window = &reads.reads()[before..];
        assert_eq!(window.len(), 1, "{window:?}");
        assert_eq!(window[0].1, ROWS as usize);
        // Unlimited and plain bounded reads: one query each, no refusal.
        let before = reads.reads().len();
        let rows = PageRow::query(&database)
            .default_order()
            .limit(12)
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(rows.len(), 12);
        let rows = PageRow::query(&database)
            .default_order()
            .fetch_all()
            .await
            .unwrap();
        assert_eq!(rows.len(), ROWS as usize);
        assert_eq!(reads.reads().len() - before, 2);
    }
}

/// `limit(0)` returns an empty page before any read on every builder, with
/// or without a row policy, after authorization and limit validation.
async fn empty_pages_never_reach_the_database(database: &Database) {
    let unpoliced = database.clone();
    let (residual, _) = with_policy(database, Decision::Residual);
    let (unrestricted, _) = with_policy(database, Decision::Unrestricted);
    for database in [unpoliced, residual, unrestricted] {
        let reads = CountReads::default();
        let database = database.clone().with_read_query_observer(reads.clone());
        // The table does not exist, so a read would fail.
        assert!(
            MissingRow::query(&database)
                .limit(1)
                .fetch_all()
                .await
                .is_err()
        );
        let before = reads.reads().len();
        assert!(
            MissingRow::query(&database)
                .offset(3)
                .limit(0)
                .fetch_all()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            MissingRow::query(&database)
                .limit(0)
                .fetch_first()
                .await
                .unwrap()
                .is_none()
        );
        let (all, one) = database
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    let all = tx
                        .query::<MissingRow>()
                        .offset(3)
                        .limit(0)
                        .fetch_all()
                        .await?;
                    let one = tx.query::<MissingRow>().limit(0).fetch_one().await?;
                    Ok((all, one))
                })
            })
            .await
            .unwrap();
        assert!(all.is_empty() && one.is_none());
        assert!(
            MissingRow::search_db(&database, needle())
                .offset(2)
                .limit(0)
                .fetch_all()
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(reads.reads().len(), before, "{:?}", reads.reads());
        // Limit validation still runs first.
        assert_code(
            MissingRow::query(&database).limit(-1).fetch_all().await,
            OrmErrorCode::InvalidInput,
        );
        assert_code(
            MissingRow::search_db(&database, needle())
                .limit(1_000)
                .fetch_all()
                .await,
            OrmErrorCode::PageLimitExceeded,
        );
    }
}

/// An empty page reads nothing; a zero budget refuses only when rows exist.
async fn zero_limits_and_budgets(database: &Database) {
    let reads = CountReads::default();
    let (database, _) = with_policy(database, Decision::Residual);
    let database = database.with_read_query_observer(reads.clone());
    let rows = PageRow::query(&database)
        .default_order()
        .offset(5)
        .limit(0)
        .fetch_all()
        .await
        .unwrap();
    assert!(rows.is_empty());
    assert!(reads.reads().is_empty());
    let none = || PageRowWhereInput {
        rank: Some(IntFilter {
            gte: Some(1_000),
            ..Default::default()
        }),
        ..Default::default()
    };
    let rows = PageRow::query(&database)
        .filter(none())
        .default_order()
        .with_max_scanned_rows(0)
        .limit(3)
        .fetch_all()
        .await
        .unwrap();
    assert!(rows.is_empty());
    assert_code(
        PageRow::query(&database)
            .default_order()
            .with_max_scanned_rows(0)
            .limit(3)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let hits = FallbackSearchRow::search_db(&database, needle())
        .filter(FallbackSearchRowWhereInput {
            seq: Some(IntFilter {
                gte: Some(1_000),
                ..Default::default()
            }),
            ..Default::default()
        })
        .with_max_scanned_rows(0)
        .limit(3)
        .fetch_all()
        .await
        .unwrap();
    assert!(hits.is_empty());
}

fn needle() -> SearchInput {
    SearchInput {
        query: "needle".to_string(),
        mode: Some(SearchMode::Plain),
        min_score: None,
    }
}

fn fallback_seqs(hits: &[SearchHit<FallbackSearchRow>]) -> Vec<i64> {
    hits.iter().map(|hit| hit.entity.seq).collect()
}

/// Fallback search reads one bounded candidate set and pages its visible hits.
async fn fallback_search_counts_visible_hits(database: &Database) {
    let reads = CountReads::default();
    let (database, _) = with_policy(database, Decision::Residual);
    let database = database.with_read_query_observer(reads.clone());
    let search = || FallbackSearchRow::search_db(&database, needle()).default_order();
    let before = reads.reads().len();
    let hits = search().limit(4).fetch_all().await.unwrap();
    assert_eq!(fallback_seqs(&hits), vec![0, 3, 6, 9]);
    // One read of every candidate, at most the budget plus one.
    let candidate_reads = &reads.reads()[before..];
    assert_eq!(candidate_reads.len(), 1, "{candidate_reads:?}");
    assert_eq!(candidate_reads[0].1, ROWS as usize);

    let mut seen = Vec::new();
    for offset in (0..12).step_by(4) {
        seen.extend(fallback_seqs(
            &search().offset(offset).limit(4).fetch_all().await.unwrap(),
        ));
    }
    assert_eq!(seen, visible_ranks());

    // More candidates than the budget: refused before scoring, after reading
    // at most the budget plus one.
    let before = reads.reads().len();
    assert_code(
        search()
            .with_max_scanned_rows(29)
            .limit(1)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let refused = &reads.reads()[before..];
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].1, 30);
    let hits = search()
        .with_max_scanned_rows(30)
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(fallback_seqs(&hits), vec![0, 3, 6, 9]);
}

/// Native search pages ranked hits in SQL batches.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn native_search_counts_visible_hits(database: &Database) {
    let (database, _) = with_policy(database, Decision::Residual);
    let seqs = |hits: Vec<SearchHit<NativeSearchRow>>| {
        hits.iter().map(|hit| hit.entity.seq).collect::<Vec<_>>()
    };
    let hits = NativeSearchRow::search_db(&database, needle())
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(seqs(hits), vec![0, 3, 6, 9]);
    let mut seen = Vec::new();
    for offset in (0..12).step_by(4) {
        seen.extend(seqs(
            NativeSearchRow::search_db(&database, needle())
                .offset(offset)
                .limit(4)
                .fetch_all()
                .await
                .unwrap(),
        ));
    }
    assert_eq!(seen, visible_ranks());
    assert_code(
        NativeSearchRow::search_db(&database, needle())
            .with_max_scanned_rows(9)
            .limit(4)
            .fetch_all()
            .await,
        OrmErrorCode::PageLimitExceeded,
    );
    let hits = NativeSearchRow::search_db(&database, needle())
        .with_max_scanned_rows(10)
        .limit(4)
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(seqs(hits), vec![0, 3, 6, 9]);
}

#[cfg(any(feature = "postgres", feature = "mssql"))]
async fn run_all(database: &Database) {
    residual_limit_counts_visible_rows(database).await;
    residual_offset_counts_visible_rows(database).await;
    residual_single_row_reads_use_visible_rows(database).await;
    residual_scan_budget_refuses_short_pages(database).await;
    transaction_query_counts_visible_rows(database).await;
    sql_authoritative_reads_keep_one_query(database).await;
    refusing_callback_still_fills_the_page(database).await;
    in_memory_filters_keep_candidate_reads_bounded(database).await;
    tied_orders_get_a_key_tiebreak(database).await;
    zero_limits_and_budgets(database).await;
    qualified_key_orders_get_no_second_key_term(database).await;
    unlimited_reads_share_the_candidate_budget(database).await;
    rescans_count_the_first_window(database).await;
    reads_without_a_residual_check_ignore_the_budget(database).await;
    empty_pages_never_reach_the_database(database).await;
    fallback_search_counts_visible_hits(database).await;
    #[cfg(feature = "postgres")]
    native_search_counts_visible_hits(database).await;
    // Adds rows, so it runs last.
    unlimited_offset_keeps_rows_after_the_skip(database).await;
}

#[cfg(feature = "sqlite")]
mod sqlite_cases {
    use super::*;

    macro_rules! case {
        ($name:ident) => {
            #[tokio::test]
            async fn $name() {
                let (database, mut owner) = fixture().await.unwrap();
                super::$name(&database).await;
                owner.cleanup().unwrap();
            }
        };
    }

    case!(residual_limit_counts_visible_rows);
    case!(residual_offset_counts_visible_rows);
    case!(residual_single_row_reads_use_visible_rows);
    case!(residual_scan_budget_refuses_short_pages);
    case!(transaction_query_counts_visible_rows);
    case!(sql_authoritative_reads_keep_one_query);
    case!(refusing_callback_still_fills_the_page);
    case!(in_memory_filters_keep_candidate_reads_bounded);
    case!(tied_orders_get_a_key_tiebreak);
    case!(zero_limits_and_budgets);
    case!(qualified_key_orders_get_no_second_key_term);
    case!(unlimited_reads_share_the_candidate_budget);
    case!(rescans_count_the_first_window);
    case!(reads_without_a_residual_check_ignore_the_budget);
    case!(empty_pages_never_reach_the_database);
    case!(fallback_search_counts_visible_hits);
    case!(native_search_counts_visible_hits);
    case!(unlimited_offset_keeps_rows_after_the_skip);
}

#[cfg(any(feature = "postgres", feature = "mssql"))]
#[tokio::test]
#[ignore = "owned database lane"]
async fn visible_row_pages_on_owned_database() {
    let (database, mut owner) = fixture().await.unwrap();
    run_all(&database).await;
    owner.cleanup().unwrap();
}
