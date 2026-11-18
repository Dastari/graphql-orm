//! #752 on SQL Server: which tables a narrowed bulk write locks by key.
//!
//! With `narrow_bulk_writes_to_read_visibility` on and
//! `READ_COMMITTED_SNAPSHOT` on, a SQL Server bulk write reads its candidates
//! without a write lock and locks them by a forced key seek. It keeps the
//! single `UPDLOCK, HOLDLOCK` read where a key seek would not lock only the
//! candidates: a key type the ORM's parameter cannot seek on, or an index
//! that does not allow row locks.
#![cfg(feature = "mssql")]
#![cfg(not(any(feature = "sqlite", feature = "postgres")))]

#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::sync::{Arc, Mutex};

/// A string-keyed table: `NVARCHAR` or `VARCHAR`, as each test creates it.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "string_keys",
    plural = "StringKeys",
    schema_policy = "external_writable"
)]
struct StringKey {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    category: String,
    owner: String,
    other: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "int_keys",
    plural = "IntKeys",
    schema_policy = "external_writable"
)]
struct IntKey {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: i64,
    #[filterable(type = "string")]
    category: String,
    owner: String,
    other: String,
}

/// A table with a column named like the generated re-check column.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "alias_keys",
    plural = "AliasKeys",
    schema_policy = "external_writable"
)]
struct AliasKey {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    category: String,
    #[graphql_orm(db_column = "__gom_still_matches")]
    owner: String,
    other: String,
}

schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [StringKey, IntKey, AliasKey] }

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

/// The caller sees and writes the rows it owns (`owner = 'mine'`).
struct OwnerRows;
fn owns(row: &(dyn std::any::Any + Send + Sync)) -> bool {
    row.downcast_ref::<StringKey>()
        .map(|row| row.owner == "mine")
        .or_else(|| row.downcast_ref::<IntKey>().map(|row| row.owner == "mine"))
        .or_else(|| {
            row.downcast_ref::<AliasKey>()
                .map(|row| row.owner == "mine")
        })
        .unwrap_or(false)
}
impl RowPolicy for OwnerRows {
    fn read_visibility<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        entity: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<ReadVisibility>> {
        Box::pin(async move {
            let mine = SqlValue::String("mine".into());
            let predicate = match entity {
                "StringKey" => ReadPredicate::column::<DefaultBackend, StringKey>(
                    "owner",
                    ReadPredicateOp::Eq,
                    mine,
                )?,
                "IntKey" => ReadPredicate::column::<DefaultBackend, IntKey>(
                    "owner",
                    ReadPredicateOp::Eq,
                    mine,
                )?,
                // A predicate names the physical column.
                _ => ReadPredicate::column::<DefaultBackend, AliasKey>(
                    "__gom_still_matches",
                    ReadPredicateOp::Eq,
                    mine,
                )?,
            };
            Ok(ReadVisibility::Complete(predicate))
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
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(owns(row)) })
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
        Box::pin(async move { Ok(owns(row)) })
    }
}

#[derive(Clone, Default)]
struct SqlLog(Arc<Mutex<Vec<String>>>);
impl graphql_orm::graphql::orm::ReadQueryObserver for SqlLog {
    fn on_read(&self, sql: &str, _: usize) {
        self.0.lock().unwrap().push(sql.to_owned());
    }
}
impl SqlLog {
    fn took_key_path(&self) -> bool {
        let reads = std::mem::take(&mut *self.0.lock().unwrap());
        let keyed = reads
            .iter()
            .any(|sql| sql.contains("WITH (XLOCK, FORCESEEK)"));
        let single = reads
            .iter()
            .any(|sql| sql.contains("WITH (UPDLOCK, HOLDLOCK)"));
        assert!(keyed != single, "exactly one path: {reads:#?}");
        keyed
    }
}

/// A database with `READ_COMMITTED_SNAPSHOT` on, narrowing on, and its reads
/// logged.
async fn fixture() -> (Database, SqlLog, owned_mssql::OwnedSqlServer) {
    let owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
    owned_mssql::execute_batch(
        &owner.connection_string,
        "ALTER DATABASE CURRENT SET READ_COMMITTED_SNAPSHOT ON WITH ROLLBACK IMMEDIATE",
    )
    .await
    .unwrap();
    let pool = graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable_with(
        &owner.connection_string,
        graphql_orm::db::mssql::MssqlPoolOptions::new()
            .query_timeout(Some(std::time::Duration::from_secs(30))),
    )
    .await
    .unwrap();
    let log = SqlLog::default();
    let mut db = Database::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .narrow_bulk_writes_to_read_visibility(true)
        .build()
        .with_read_query_observer(log.clone());
    db.set_entity_policy(AllowEntityPolicy);
    db.set_row_policy(OwnerRows);
    (db, log, owner)
}

async fn execute(db: &Database, sql: &str) {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
        .await
        .unwrap();
}

async fn mutate(db: &Database, mutation: &str) -> serde_json::Value {
    let response = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .finish()
        .execute(format!("mutation {{ {mutation} }}"))
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    let json = response.data.into_json().unwrap();
    json.as_object().unwrap().values().next().unwrap().clone()
}

/// `other` of every row, by key.
async fn others(db: &Database, table: &str) -> Vec<String> {
    DefaultBackend::fetch_rows(
        db.pool(),
        &format!("SELECT other FROM {table} ORDER BY id"),
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| DefaultBackend::try_get_string(row, "other").unwrap())
    .collect()
}

/// Three rows of the caller's and one hidden row, keyed by `keys`.
fn rows(keys: [&str; 4]) -> String {
    let owners = ["mine", "mine", "theirs", "mine"];
    keys.iter()
        .zip(owners)
        .map(|(key, owner)| format!("({key}, 'key', '{owner}', 'before')"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A Unicode string key and an integer key seek with the ORM's parameters,
/// so the narrowed write locks by key; a `varchar` key under a SQL collation
/// would be converted to compare with the ORM's Unicode parameter, so the
/// write keeps the single locking read instead of failing to plan the seek.
/// Each writes exactly the caller's rows.
#[tokio::test]
async fn narrowed_sql_server_bulk_writes_lock_by_key_only_on_seekable_keys() {
    let (db, log, mut owner) = fixture().await;
    let string_keys = rows(["N'a1 '", "N'A2'", "N'a3'", "N'a4'"]);
    for (column, keyed) in [
        ("NVARCHAR(40) COLLATE SQL_Latin1_General_CP1_CI_AS", true),
        ("NVARCHAR(40) COLLATE Latin1_General_100_CI_AS", true),
        ("VARCHAR(40) COLLATE SQL_Latin1_General_CP1_CI_AS", false),
        ("CHAR(10) COLLATE Latin1_General_100_CI_AS", false),
    ] {
        execute(
            &db,
            &format!(
                "CREATE TABLE string_keys (id {column} PRIMARY KEY, category NVARCHAR(40) NOT NULL, \
                 owner NVARCHAR(40) NOT NULL, other NVARCHAR(40) NOT NULL); \
                 INSERT INTO string_keys VALUES {string_keys}"
            ),
        )
        .await;
        let payload = mutate(
            &db,
            r#"updateStringKeys(where: {category: {eq: "key"}}, input: {other: "after"}) { success affectedCount error errorCode }"#,
        )
        .await;
        assert_eq!(payload["success"], true, "{column}: {payload}");
        assert_eq!(payload["affectedCount"], 3, "{column}");
        assert_eq!(log.took_key_path(), keyed, "{column}");
        assert_eq!(
            others(&db, "string_keys").await,
            ["after", "after", "before", "after"]
        );
        let payload = mutate(
            &db,
            r#"deleteStringKeys(where: {category: {eq: "key"}}) { success deletedCount error errorCode }"#,
        )
        .await;
        assert_eq!(payload["deletedCount"], 3, "{column}: {payload}");
        assert_eq!(log.took_key_path(), keyed, "{column}");
        assert_eq!(others(&db, "string_keys").await, ["before"]);
        execute(&db, "DROP TABLE string_keys").await;
    }

    for column in ["INT", "BIGINT", "SMALLINT"] {
        execute(
            &db,
            &format!(
                "CREATE TABLE int_keys (id {column} PRIMARY KEY, category NVARCHAR(40) NOT NULL, \
                 owner NVARCHAR(40) NOT NULL, other NVARCHAR(40) NOT NULL); \
                 INSERT INTO int_keys VALUES {}",
                rows(["1", "2", "3", "4"])
            ),
        )
        .await;
        let payload = mutate(
            &db,
            r#"updateIntKeys(where: {category: {eq: "key"}}, input: {other: "after"}) { success affectedCount error errorCode }"#,
        )
        .await;
        assert_eq!(payload["affectedCount"], 3, "{column}: {payload}");
        assert!(log.took_key_path(), "{column}");
        assert_eq!(
            others(&db, "int_keys").await,
            ["after", "after", "before", "after"]
        );
        execute(&db, "DROP TABLE int_keys").await;
    }
    drop(db);
    owner.cleanup().unwrap();
}

/// A column named like the generated re-check column does not collide with
/// it: the narrowed write locks by key and writes the caller's rows, as the
/// unnarrowed write does.
#[tokio::test]
async fn a_column_named_like_the_recheck_alias_does_not_collide() {
    let (db, log, mut owner) = fixture().await;
    execute(
        &db,
        &format!(
            "CREATE TABLE alias_keys (id NVARCHAR(40) PRIMARY KEY, category NVARCHAR(40) NOT NULL, \
             __gom_still_matches NVARCHAR(40) NOT NULL, other NVARCHAR(40) NOT NULL); \
             INSERT INTO alias_keys VALUES {}",
            rows(["N'a1'", "N'a2'", "N'a3'", "N'a4'"])
        ),
    )
    .await;
    for (narrow, keyed, other) in [(false, false, "control"), (true, true, "after")] {
        let db = db
            .clone()
            .with_narrow_bulk_writes_to_read_visibility(narrow);
        let payload = mutate(
            &db,
            &format!(
                r#"updateAliasKeys(where: {{category: {{eq: "key"}}}}, input: {{other: "{other}"}}) {{ success affectedCount error errorCode }}"#
            ),
        )
        .await;
        assert_eq!(payload["success"], true, "narrow={narrow}: {payload}");
        assert_eq!(payload["affectedCount"], 3, "narrow={narrow}");
        assert_eq!(log.took_key_path(), keyed, "narrow={narrow}");
        assert_eq!(
            others(&db, "alias_keys").await,
            [other, other, "before", other]
        );
    }
    drop(db);
    owner.cleanup().unwrap();
}

/// An index that does not allow row locks would take page locks, which cover
/// neighbouring hidden rows, so the narrowed write keeps the single locking
/// read there.
#[tokio::test]
async fn an_index_without_row_locks_keeps_the_single_locking_read() {
    let (db, log, mut owner) = fixture().await;
    execute(
        &db,
        &format!(
            "CREATE TABLE string_keys (id NVARCHAR(40) PRIMARY KEY, category NVARCHAR(40) NOT NULL, \
             owner NVARCHAR(40) NOT NULL, other NVARCHAR(40) NOT NULL); \
             CREATE INDEX string_keys_category ON string_keys (category); \
             INSERT INTO string_keys VALUES {}",
            rows(["N'a1'", "N'a2'", "N'a3'", "N'a4'"])
        ),
    )
    .await;
    let update = r#"updateStringKeys(where: {category: {eq: "key"}}, input: {other: "after"}) { success affectedCount error errorCode }"#;
    assert_eq!(mutate(&db, update).await["affectedCount"], 3);
    assert!(log.took_key_path());
    execute(
        &db,
        "ALTER INDEX string_keys_category ON string_keys SET (ALLOW_ROW_LOCKS = OFF)",
    )
    .await;
    assert_eq!(mutate(&db, update).await["affectedCount"], 3);
    assert!(!log.took_key_path());
    drop(db);
    owner.cleanup().unwrap();
}

/// On the key path the scan budget bounds the one candidate read, and a write
/// over it is refused before any candidate is locked; the bulk bound counts
/// the targets.
#[tokio::test]
async fn the_key_path_keeps_the_scan_budget_and_the_bulk_bound() {
    let (db, log, mut owner) = fixture().await;
    execute(
        &db,
        &format!(
            "CREATE TABLE int_keys (id INT PRIMARY KEY, category NVARCHAR(40) NOT NULL, \
             owner NVARCHAR(40) NOT NULL, other NVARCHAR(40) NOT NULL); \
             INSERT INTO int_keys VALUES {}",
            rows(["1", "2", "3", "4"])
        ),
    )
    .await;
    let update = r#"updateIntKeys(where: {category: {eq: "key"}}, input: {other: "after"}) { success affectedCount error errorCode }"#;
    let config = PaginationConfig::default();
    for (config, refused) in [
        (config.with_max_scanned_rows(Some(2)), true),
        (config.with_max_bulk_write_rows(Some(2)), true),
        (config.with_max_scanned_rows(Some(3)), false),
    ] {
        let payload = mutate(&db.clone().with_pagination_config(config), update).await;
        let reads = std::mem::take(&mut *log.0.lock().unwrap());
        assert!(
            reads.iter().any(|sql| sql.contains("WITH (READCOMMITTED)")),
            "{reads:#?}"
        );
        if refused {
            assert_eq!(payload["errorCode"], "PAGE_LIMIT_EXCEEDED", "{payload}");
            assert_eq!(others(&db, "int_keys").await, ["before"; 4]);
        } else {
            assert_eq!(payload["affectedCount"], 3, "{payload}");
        }
    }
    drop(db);
    owner.cleanup().unwrap();
}
