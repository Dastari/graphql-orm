//! Target rows of generated writes: the bulk-write row bound, the SQL
//! write-target prefilter, and bare-pool inserts taking the authorized path.

// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "target_records", plural = "TargetRecords")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "target_records",
        plural = "TargetRecords"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "target_records",
        plural = "TargetRecords",
        schema_policy = "external_writable"
    )
)]
struct TargetRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    #[filterable(type = "string")]
    owner: String,
    other: String,
}

/// The same table behind an entity that declares a write policy key.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "target_records",
        plural = "GuardedRecords",
        write_policy = "records.write"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "target_records",
        plural = "GuardedRecords",
        write_policy = "records.write"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "target_records",
        plural = "GuardedRecords",
        schema_policy = "external_writable",
        write_policy = "records.write"
    )
)]
struct GuardedRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    owner: String,
    other: String,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [TargetRecord, GuardedRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [TargetRecord, GuardedRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [TargetRecord, GuardedRecord] }

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

struct AllowEntityPolicy;
impl EntityPolicy for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

/// The caller owns the rows whose `owner` is `mine`, and can neither read nor
/// write any other row. Every row callback records the row it was asked about.
struct OwnerRows {
    prefilter: bool,
    asked: Arc<Mutex<Vec<String>>>,
}
impl OwnerRows {
    fn decide(&self, row: &(dyn std::any::Any + Send + Sync)) -> bool {
        let row = row
            .downcast_ref::<TargetRecord>()
            .expect("only TargetRecord rows are written here");
        self.asked.lock().unwrap().push(row.record_key.clone());
        row.owner == "mine"
    }
}
impl RowPolicy for OwnerRows {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide(row)) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(self.decide(row)) })
    }
    fn write_target_prefilter<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<Option<ReadPredicate>>> {
        Box::pin(async move {
            assert!(ctx.is_some(), "only GraphQL writes ask for a prefilter");
            assert_eq!(surface, EntityAccessSurface::GraphqlMutation);
            if !self.prefilter {
                return Ok(None);
            }
            Ok(Some(ReadPredicate::column::<DefaultBackend, TargetRecord>(
                "owner",
                ReadPredicateOp::Eq,
                SqlValue::String("mine".into()),
            )?))
        })
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
        let owner = FixtureOwner::start("bulk-write-targets")?;
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
        let db = Database::builder(pool)
            .schema_policy(SchemaPolicy::ExternalWritable)
            .build();
        Ok((db, owner))
    }
}

async fn execute(db: &Database, sql: &str) -> graphql_orm::Result<()> {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[]).await?;
    Ok(())
}

/// Four rows: `key1` and `key2` are the caller's, `key3` and `key4` are not.
async fn setup() -> (Database, FixtureOwner) {
    let (mut db, owner) = fixture().await.unwrap();
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE target_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, owner TEXT NOT NULL, other TEXT NOT NULL)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE target_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, owner TEXT NOT NULL, other TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE target_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, owner NVARCHAR(100) NOT NULL, other NVARCHAR(100) NOT NULL)";
    execute(&db, ddl).await.unwrap();
    for (id, owner_name) in [(1, "mine"), (2, "mine"), (3, "theirs"), (4, "theirs")] {
        execute(&db, &format!("INSERT INTO target_records VALUES ('00000000-0000-0000-0000-00000000000{id}', 'key{id}', '{owner_name}', 'before')")).await.unwrap();
    }
    db.set_entity_policy(AllowEntityPolicy);
    (db, owner)
}

async fn stored(db: &Database) -> Vec<(String, String)> {
    DefaultBackend::fetch_rows(
        db.pool(),
        "SELECT record_key, other FROM target_records ORDER BY record_key",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            DefaultBackend::try_get_string(row, "record_key").unwrap(),
            DefaultBackend::try_get_string(row, "other").unwrap(),
        )
    })
    .collect()
}

fn all_before(rows: &[(String, String)]) -> bool {
    rows.iter().all(|(_, other)| other == "before")
}

async fn mutate(db: &Database, mutation: &str) -> serde_json::Value {
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .finish();
    let response = schema.execute(format!("mutation {{ {mutation} }}")).await;
    assert!(response.errors.is_empty(), "{response:?}");
    let json = response.data.into_json().unwrap();
    json.as_object().unwrap().values().next().unwrap().clone()
}

const UPDATE_ALL: &str = r#"updateTargetRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "after"}) { success affectedCount error errorCode }"#;
const DELETE_ALL: &str = r#"deleteTargetRecords(where: {recordKey: {startsWith: "key"}}) { success deletedCount error errorCode }"#;

fn bounded(db: &Database, bound: Option<i64>) -> Database {
    db.clone()
        .with_pagination_config(PaginationConfig::default().with_max_bulk_write_rows(bound))
}

/// #446: a GraphQL bulk write over the bound is refused, never truncated.
#[tokio::test]
async fn bulk_writes_over_the_bound_are_refused_with_a_stable_code() {
    let (db, mut owner) = setup().await;
    assert_eq!(
        PaginationConfig::default().max_bulk_write_rows,
        Some(PaginationConfig::DEFAULT_MAX_BULK_WRITE_ROWS)
    );
    assert_eq!(PaginationConfig::unbounded().max_bulk_write_rows, None);

    // Four rows match and the bound is three: nothing is written.
    let tight = bounded(&db, Some(3));
    for mutation in [UPDATE_ALL, DELETE_ALL] {
        let result = mutate(&tight, mutation).await;
        assert_eq!(result["success"], false, "{result}");
        assert_eq!(result["errorCode"], "PAGE_LIMIT_EXCEEDED", "{result}");
        let message = result["error"].as_str().unwrap();
        assert!(
            !message.chars().any(|character| character.is_ascii_digit()),
            "the refusal names no row count: {message}"
        );
        let rows = stored(&db).await;
        assert_eq!(rows.len(), 4, "{mutation}");
        assert!(all_before(&rows), "{mutation}");
    }

    // At the bound the result is the one an unbounded write gives.
    let exact = bounded(&db, Some(4));
    let result = mutate(&exact, UPDATE_ALL).await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["affectedCount"], 4);
    assert!(stored(&db).await.iter().all(|(_, other)| other == "after"));

    // Trusted repository code is not bounded by the GraphQL limit.
    let updated = TargetRecord::update_where(
        &tight,
        TargetRecordWhereInput {
            record_key: Some(graphql_orm::graphql::filters::StringFilter {
                starts_with: Some("key".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateTargetRecordInput {
            other: Some("repository".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated, 4);

    // `None` removes the bound.
    let unbounded = bounded(&db, None);
    let result = mutate(&unbounded, DELETE_ALL).await;
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["deletedCount"], 4);
    assert!(stored(&db).await.is_empty());
    owner.cleanup().unwrap();
}

/// #602: with a write-target prefilter, hidden rows are never selected, so no
/// row callback sees them; results equal the callback-only ones.
#[tokio::test]
async fn write_target_prefilter_keeps_hidden_rows_from_row_callbacks() {
    for prefilter in [false, true] {
        let (mut db, mut owner) = setup().await;
        let asked = Arc::new(Mutex::new(Vec::new()));
        db.set_row_policy(OwnerRows {
            prefilter,
            asked: asked.clone(),
        });
        let hidden_asked = |asked: &Arc<Mutex<Vec<String>>>| {
            let mut asked = asked.lock().unwrap();
            let hidden = asked.iter().any(|key| key == "key3" || key == "key4");
            asked.clear();
            hidden
        };

        let result = mutate(&db, UPDATE_ALL).await;
        assert_eq!(result["success"], true, "{result}");
        assert_eq!(result["affectedCount"], 2, "prefilter={prefilter}");
        assert_eq!(hidden_asked(&asked), !prefilter, "updateMany");
        assert_eq!(
            stored(&db).await,
            [
                ("key1", "after"),
                ("key2", "after"),
                ("key3", "before"),
                ("key4", "before")
            ]
            .map(|(key, other)| (key.to_owned(), other.to_owned()))
        );

        // A hidden single-row target reads as missing either way.
        for mutation in [
            r#"updateTargetRecord(id: "00000000-0000-0000-0000-000000000003", input: {other: "x"}) { success error }"#,
            r#"deleteTargetRecord(id: "00000000-0000-0000-0000-000000000003") { success error }"#,
        ] {
            let result = mutate(&db, mutation).await;
            assert_eq!(result["success"], false, "{result}");
            assert_eq!(result["error"], "Entity not found");
            assert_eq!(hidden_asked(&asked), !prefilter, "{mutation}");
        }
        owner.cleanup().unwrap();
    }
}

/// The bulk bound counts the rows the caller may write. For a fixed set of
/// such rows the payload is the same whether or not other, hidden rows match
/// the filter, with a callback-only row policy and with a SQL prefilter.
#[tokio::test]
async fn hidden_matching_rows_never_change_a_bulk_write_outcome() {
    for prefilter in [false, true] {
        for (bound, mutation) in [
            (1, UPDATE_ALL),
            (2, UPDATE_ALL),
            (1, DELETE_ALL),
            (2, DELETE_ALL),
        ] {
            let mut payloads = Vec::new();
            for hidden_rows_match in [true, false] {
                let (mut db, mut owner) = setup().await;
                if !hidden_rows_match {
                    execute(&db, "DELETE FROM target_records WHERE owner = 'theirs'")
                        .await
                        .unwrap();
                }
                db.set_row_policy(OwnerRows {
                    prefilter,
                    asked: Default::default(),
                });
                let payload = mutate(&bounded(&db, Some(bound)), mutation).await;
                let mine: Vec<_> = stored(&db)
                    .await
                    .into_iter()
                    .filter(|(key, _)| key == "key1" || key == "key2")
                    .collect();
                payloads.push((payload, mine));
                owner.cleanup().unwrap();
            }
            assert_eq!(
                payloads[0], payloads[1],
                "prefilter={prefilter} bound={bound} {mutation}"
            );
            // Two rows are the caller's: a bound of one refuses, two admits.
            let (payload, _) = &payloads[0];
            if bound == 1 {
                assert_eq!(payload["success"], false, "{payload}");
                assert_eq!(payload["errorCode"], "PAGE_LIMIT_EXCEEDED");
            } else {
                assert_eq!(payload["success"], true, "{payload}");
            }
        }
    }
}

/// A row policy decided in Rust classifies candidates in pages under the scan
/// budget authorized reads use. A scan that would need more candidates is the
/// same host-configured refusal a read gets; a SQL prefilter avoids the scan.
#[tokio::test]
async fn a_bulk_write_candidate_scan_is_capped_by_the_read_scan_budget() {
    for prefilter in [false, true] {
        let (mut db, mut owner) = setup().await;
        db.set_row_policy(OwnerRows {
            prefilter,
            asked: Default::default(),
        });
        // Four candidates match; the budget allows three.
        let tight = db
            .clone()
            .with_pagination_config(PaginationConfig::default().with_max_scanned_rows(Some(3)));
        let payload = mutate(&tight, UPDATE_ALL).await;
        if prefilter {
            // SQL removes the hidden rows: two candidates, within the budget.
            assert_eq!(payload["success"], true, "{payload}");
            assert_eq!(payload["affectedCount"], 2);
        } else {
            assert_eq!(payload["success"], false, "{payload}");
            assert_eq!(payload["errorCode"], "PAGE_LIMIT_EXCEEDED");
            assert!(
                payload["error"].as_str().unwrap().contains("scan budget"),
                "{payload}"
            );
            assert!(all_before(&stored(&db).await), "nothing was written");
        }
        // A budget that covers the candidates admits the write either way.
        let roomy = db
            .clone()
            .with_pagination_config(PaginationConfig::default().with_max_scanned_rows(Some(4)));
        let payload = mutate(&roomy, DELETE_ALL).await;
        assert_eq!(payload["success"], true, "{payload}");
        assert_eq!(payload["deletedCount"], 2);
        owner.cleanup().unwrap();
    }
}

fn new_record(key: &str) -> CreateTargetRecordInput {
    CreateTargetRecordInput {
        record_key: key.into(),
        owner: "mine".into(),
        other: "inserted".into(),
    }
}

/// #601: `insert` with a bare pool takes the same authorized path as a
/// `Database` with nothing installed.
#[tokio::test]
async fn bare_pool_insert_takes_the_authorized_path() {
    let (db, mut owner) = setup().await;

    // An entity that declares a write policy key has no provider on a bare
    // pool: the write is refused instead of running unchecked.
    let error = GuardedRecord::insert(
        db.pool(),
        CreateGuardedRecordInput {
            record_key: "guarded".into(),
            owner: "mine".into(),
            other: "inserted".into(),
        },
    )
    .await
    .expect_err("a bare pool carries no policy provider");
    assert_eq!(
        OrmPublicError::from_sqlx(&error).code,
        OrmErrorCode::AuthorizationMisconfigured
    );
    assert_eq!(stored(&db).await.len(), 4);

    // An entity without a policy key keeps working. On SQL Server the pool
    // itself was opened for entity writes, which the default handle honours.
    let inserted = TargetRecord::insert(db.pool(), new_record("bare"))
        .await
        .unwrap();
    assert_eq!(inserted.record_key, "bare");
    assert_eq!(stored(&db).await.len(), 5);

    // The same call on the `Database` is unchanged.
    let inserted = TargetRecord::insert(&db, new_record("handle"))
        .await
        .unwrap();
    assert_eq!(inserted.other, "inserted");
    owner.cleanup().unwrap();
}
