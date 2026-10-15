#![cfg(any(feature = "sqlite", feature = "postgres", feature = "mssql"))]
use graphql_orm::async_graphql::Request;
use graphql_orm::prelude::*;
#[cfg(feature = "mssql")]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "bounded_candidate_rows",
        plural = "BoundedRows"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "bounded_candidate_rows",
        plural = "BoundedRows"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        schema_policy = "external_writable",
        table = "bounded_candidate_rows",
        plural = "BoundedRows"
    )
)]
#[graphql_orm(search(index = false))]
struct BoundedRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    owner: String,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}
#[cfg(feature = "sqlite")]
#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[graphql_entity(
    backend = "sqlite",
    table = "native_bounded_rows",
    plural = "NativeBoundedRows"
)]
#[graphql_orm(search(index = true))]
struct NativeBoundedRow {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    owner: String,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [BoundedRow, NativeBoundedRow], }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [BoundedRow], }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [BoundedRow], }
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

async fn fixture() -> Result<(Database, FixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    {
        Ok((
            Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?,
            FixtureOwner,
        ))
    }
    #[cfg(feature = "postgres")]
    {
        let owner = FixtureOwner::start("bounded-protected-scans")?;
        let pool = graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&owner.url)
            .await?;
        Ok((Database::new(pool), owner))
    }
    #[cfg(feature = "mssql")]
    {
        let owner = FixtureOwner::start().await?;
        let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable(
            &owner.connection_string,
        )
        .await?;
        let mut db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        db.set_entity_policy(AllowEntityPolicy);
        Ok((db, owner))
    }
}
async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(String, usize)>>>);
impl ReadQueryObserver for Capture {
    fn on_read(&self, sql: &str, rows: usize) {
        self.0.lock().unwrap().push((sql.to_string(), rows));
    }
}
impl Capture {
    fn take(&self) -> Vec<(String, usize)> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}
#[derive(Clone)]
struct OwnedRowPolicy {
    complete: bool,
    calls: Arc<AtomicUsize>,
}
impl RowPolicy for OwnedRowPolicy {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            if self.complete {
                Ok(ReadVisibility::Complete(ReadPredicate::from_filter::<
                    DefaultBackend,
                    _,
                >(
                    &BoundedRowWhereInput {
                        owner: Some(StringFilter {
                            eq: Some("alice".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )?))
            } else {
                Ok(ReadVisibility::CallbackOnly)
            }
        })
    }
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // A complete SQL predicate must bypass this intentionally denying callback.
            Ok(!self.complete
                && ctx.and_then(|c| c.auth_subject_opt()).is_some_and(|s| {
                    let owner = row
                        .downcast_ref::<BoundedRow>()
                        .map(|row| row.owner.as_str());
                    #[cfg(feature = "sqlite")]
                    let owner = owner.or_else(|| {
                        row.downcast_ref::<NativeBoundedRow>()
                            .map(|row| row.owner.as_str())
                    });
                    owner == Some(s.id.as_str())
                }))
        })
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
async fn request(db: &Database, query: &str) -> async_graphql::Response {
    schema_builder(db.clone())
        .finish()
        .execute(Request::new(query).data(AuthSubject::new("alice")))
        .await
}
fn assert_budget_denial(response: async_graphql::Response) {
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(
        response.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&async_graphql::Value::from("AUTHORIZATION_MISCONFIGURED"))
    );
    assert_eq!(response.data, async_graphql::Value::Null);
}
const LIST: &str = "{ boundedRows(page: {limit: 1, offset: 2}) { edges {node {id owner}} pageInfo {totalCount hasNextPage hasPreviousPage} } }";
const SEARCH: &str = "{ boundedRowsSearch(search: {query: \"needle\"}, page: {limit: 1, offset: 2}) { edges {node {id owner} score} pageInfo {totalCount hasNextPage hasPreviousPage} } }";
#[tokio::test]
async fn protected_list_and_fallback_search_enforce_actual_candidate_budgets()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut db, mut owner) = fixture().await?;
    execute(&db, "CREATE TABLE bounded_candidate_rows (id VARCHAR(80) PRIMARY KEY, owner VARCHAR(80) NOT NULL, title VARCHAR(100) NOT NULL)").await?;
    for index in 0..40 {
        execute(
            &db,
            &format!(
                "INSERT INTO bounded_candidate_rows VALUES ('{index:03}', '{}', 'needle')",
                if index < 30 { "bob" } else { "alice" }
            ),
        )
        .await?;
    }
    let capture = Capture::default();
    // Database-side RLS is not discoverable through the RowPolicy registry.
    // Every generated fallback therefore requires the same explicit budget.
    let no_policy = db.clone().with_read_query_observer(capture.clone());
    assert_budget_denial(request(&no_policy, SEARCH).await);
    assert!(capture.take().is_empty());
    let no_policy_limited = no_policy
        .clone()
        .with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 4, 7));
    assert_budget_denial(request(&no_policy_limited, SEARCH).await);
    assert_eq!(
        capture
            .take()
            .iter()
            .map(|(_, count)| *count)
            .sum::<usize>(),
        7
    );
    let no_policy_bounded =
        no_policy.with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64));
    let response = request(&no_policy_bounded, SEARCH).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json()?["boundedRowsSearch"]["pageInfo"]["totalCount"],
        40
    );
    let reads = capture.take();
    assert_eq!(reads.iter().map(|(_, count)| *count).sum::<usize>(), 40);
    assert!(reads.iter().all(|(_, count)| *count <= 7));
    let calls = Arc::new(AtomicUsize::new(0));
    db.set_row_policy(OwnedRowPolicy {
        complete: false,
        calls: calls.clone(),
    });
    let db = db.with_read_query_observer(capture.clone());
    for query in [LIST, SEARCH] {
        assert_budget_denial(request(&db, query).await);
        assert!(
            capture.take().is_empty(),
            "missing config fetched candidates"
        );
        let limited = db
            .clone()
            .with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 4, 7));
        calls.store(0, Ordering::Relaxed);
        assert_budget_denial(request(&limited, query).await);
        let reads = capture.take();
        assert_eq!(reads.iter().map(|(_, count)| *count).sum::<usize>(), 7);
        assert!(reads.iter().all(|(_, count)| *count <= 4));
        assert_eq!(calls.load(Ordering::Relaxed), 7);
        let bounded = db
            .clone()
            .with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64));
        let response = request(&bounded, query).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json()?;
        let field = if query == LIST {
            "boundedRows"
        } else {
            "boundedRowsSearch"
        };
        assert_eq!(data[field]["pageInfo"]["totalCount"], 10);
        assert_eq!(data[field]["edges"][0]["node"]["id"], "032");
        let reads = capture.take();
        assert_eq!(reads.iter().map(|(_, count)| *count).sum::<usize>(), 40);
        assert!(reads.iter().all(|(_, count)| *count <= 7));
    }
    execute(
        &db,
        "UPDATE bounded_candidate_rows SET title = 'needle needle needle needle' WHERE id = '039'",
    )
    .await?;
    let bounded = db
        .clone()
        .with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64));
    let response = request(&bounded, &SEARCH.replace("offset: 2", "offset: 0")).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json()?["boundedRowsSearch"]["edges"][0]["node"]["id"],
        "039"
    );
    capture.take();
    let mut complete = db.clone();
    complete.set_row_policy(OwnedRowPolicy {
        complete: true,
        calls: calls.clone(),
    });
    calls.store(0, Ordering::Relaxed);
    let response = request(&complete, LIST).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json()?["boundedRows"]["pageInfo"]["totalCount"],
        10
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    capture.take();
    let complete = complete.with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 4, 64));
    let response = request(&complete, SEARCH).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json()?["boundedRowsSearch"]["pageInfo"]["totalCount"],
        10
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let reads = capture.take();
    assert_eq!(reads.iter().map(|(_, count)| *count).sum::<usize>(), 10);
    assert!(reads.iter().all(|(_, count)| *count <= 4));
    owner.cleanup()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn native_ranked_search_is_batched_without_changing_strategy_or_tie_order()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut db, mut owner) = fixture().await?;
    execute(&db, "CREATE TABLE native_bounded_rows (id TEXT PRIMARY KEY, owner TEXT NOT NULL, title TEXT NOT NULL)").await?;
    execute(&db, "CREATE VIRTUAL TABLE __graphql_orm_fts_native_bounded_rows USING fts5(entity_pk UNINDEXED, weight_a, weight_b, weight_c, weight_d, document_text)").await?;
    for index in 0..40 {
        execute(
            &db,
            &format!(
                "INSERT INTO native_bounded_rows VALUES ('{index:03}', '{}', 'needle')",
                if index < 30 { "bob" } else { "alice" }
            ),
        )
        .await?;
        execute(&db, &format!("INSERT INTO __graphql_orm_fts_native_bounded_rows VALUES ('{index:03}', 'needle', '', '', '', 'needle')")).await?;
    }
    let native_without_callback = request(&db, "{ nativeBoundedRowsSearch(search: {query: \"needle\"}, page: {limit: 1}) { edges {node {id} score} pageInfo {totalCount} } }").await;
    assert!(
        native_without_callback.errors.is_empty(),
        "{:?}",
        native_without_callback.errors
    );
    assert_eq!(
        native_without_callback.data.into_json()?["nativeBoundedRowsSearch"]["pageInfo"]["totalCount"],
        40
    );
    let calls = Arc::new(AtomicUsize::new(0));
    db.set_row_policy(OwnedRowPolicy {
        complete: false,
        calls: calls.clone(),
    });
    let capture = Capture::default();
    let db = db.with_read_query_observer(capture.clone());
    let query = "{ nativeBoundedRowsSearch(search: {query: \"needle\"}, page: {limit: 1, offset: 2}) { edges {node {id} score} pageInfo {totalCount} } }";
    let limited = db
        .clone()
        .with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 4, 7));
    assert_budget_denial(request(&limited, query).await);
    let reads = capture.take();
    assert_eq!(reads.iter().map(|(_, rows)| *rows).sum::<usize>(), 7);
    assert!(
        reads
            .iter()
            .all(|(sql, rows)| sql.contains(" MATCH ") && *rows <= 4)
    );
    let bounded = db.with_authorized_scan_config(AuthorizedScanConfig::new([11; 32], 7, 64));
    calls.store(0, Ordering::Relaxed);
    let response = request(&bounded, query).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json()?;
    assert_eq!(
        data["nativeBoundedRowsSearch"]["pageInfo"]["totalCount"],
        10
    );
    assert_eq!(
        data["nativeBoundedRowsSearch"]["edges"][0]["node"]["id"],
        "032"
    );
    assert!(
        data["nativeBoundedRowsSearch"]["edges"][0]["score"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let reads = capture.take();
    assert_eq!(reads.iter().map(|(_, rows)| *rows).sum::<usize>(), 40);
    assert!(
        reads
            .iter()
            .all(|(sql, rows)| sql.contains(" MATCH ") && *rows <= 7)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 40);
    owner.cleanup()?;
    Ok(())
}
