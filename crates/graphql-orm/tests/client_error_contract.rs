// Opt-in client error contract for generated mutation roots: `extensions.outcome`
// on a mutation error (#371) and a stable `errorCode` on single-row results (#385).
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
use async_graphql::ErrorExtensions;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
use std::any::Any;

/// No `mutation_error_codes`: the result types keep their existing shape.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "outcome_records",
        plural = "OutcomeRecords",
        notify_with = "notify"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "outcome_records",
        plural = "OutcomeRecords",
        notify_with = "notify"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "outcome_records",
        plural = "OutcomeRecords",
        schema_policy = "external_writable",
        notify_with = "notify"
    )
)]
struct OutcomeRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    label: String,
    parent_id: Option<String>,
}

/// The same shape with the opt-in result code.
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "coded_records",
        plural = "CodedRecords",
        mutation_error_codes
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "coded_records",
        plural = "CodedRecords",
        mutation_error_codes
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "coded_records",
        plural = "CodedRecords",
        schema_policy = "external_writable",
        mutation_error_codes
    )
)]
struct CodedRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    label: String,
    parent_id: Option<String>,
}

#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [OutcomeRecord, CodedRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [OutcomeRecord, CodedRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [OutcomeRecord, CodedRecord] }

#[cfg(feature = "sqlite")]
struct FixtureOwner(std::path::PathBuf);
#[cfg(feature = "sqlite")]
impl FixtureOwner {
    fn cleanup(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = std::fs::remove_file(&self.0);
        Ok(())
    }
}
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

async fn fixture() -> Result<(Database, FixtureOwner), Box<dyn std::error::Error>> {
    #[cfg(feature = "sqlite")]
    {
        // A file, so every pooled connection sees the same tables.
        let path = std::env::temp_dir().join(format!(
            "graphql-orm-client-error-contract-{}.db",
            graphql_orm::uuid::Uuid::new_v4().simple()
        ));
        let options = graphql_orm::sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?;
        Ok((Database::new(pool), FixtureOwner(path)))
    }
    #[cfg(feature = "postgres")]
    {
        let owner = FixtureOwner::start("client-error-contract")?;
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

async fn execute(db: &Database, sql: &str) {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), sql, &[])
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

const EXISTING: &str = "00000000-0000-0000-0000-000000000001";
const SECOND: &str = "00000000-0000-0000-0000-000000000002";
const MISSING: &str = "00000000-0000-0000-0000-0000000000ff";

/// Fresh tables with one parent and, in each record table, rows `existing` and `second`.
async fn reset(db: &Database) {
    for table in ["outcome_records", "coded_records", "outcome_parents"] {
        execute(db, &format!("DROP TABLE IF EXISTS {table}")).await;
    }
    #[cfg(feature = "sqlite")]
    let (key, text, parent) = (
        "TEXT PRIMARY KEY",
        "TEXT",
        "TEXT REFERENCES outcome_parents(id) DEFERRABLE INITIALLY DEFERRED",
    );
    #[cfg(feature = "postgres")]
    let (key, text, parent) = (
        "UUID PRIMARY KEY",
        "TEXT",
        "TEXT REFERENCES outcome_parents(id) DEFERRABLE INITIALLY DEFERRED",
    );
    // SQL Server has no deferrable constraints; the commit-failure case is skipped.
    #[cfg(feature = "mssql")]
    let (key, text, parent) = (
        "UNIQUEIDENTIFIER PRIMARY KEY",
        "NVARCHAR(100)",
        "NVARCHAR(100)",
    );
    #[cfg(not(feature = "mssql"))]
    execute(db, "CREATE TABLE outcome_parents (id TEXT PRIMARY KEY)").await;
    #[cfg(feature = "mssql")]
    execute(
        db,
        "CREATE TABLE outcome_parents (id NVARCHAR(100) PRIMARY KEY)",
    )
    .await;
    execute(db, "INSERT INTO outcome_parents VALUES ('parent')").await;
    for table in ["outcome_records", "coded_records"] {
        execute(db, &format!("CREATE TABLE {table} (id {key}, record_key {text} UNIQUE NOT NULL, label {text} NOT NULL, parent_id {parent})")).await;
        execute(
            db,
            &format!("INSERT INTO {table} VALUES ('{EXISTING}', 'existing', 'before', NULL)"),
        )
        .await;
        execute(
            db,
            &format!("INSERT INTO {table} VALUES ('{SECOND}', 'second', 'before', NULL)"),
        )
        .await;
    }
}

/// Every `outcome_records` row, to prove what a refused write left.
async fn snapshot(db: &Database) -> Vec<(String, String, Option<String>)> {
    let rows = OutcomeRecord::find_all(db)
        .await
        .expect("read outcome_records");
    let mut rows = rows
        .into_iter()
        .map(|row| (row.record_key, row.label, row.parent_id))
        .collect::<Vec<_>>();
    rows.sort();
    rows
}

/// Which host callback refuses, and how.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    None,
    Entity,
    BeforeHook,
    AfterHook,
    RowWrite,
    Notify,
    /// A notify handler that sends its own `outcome`, which must not win.
    NotifyForged,
}
fn refusal() -> async_graphql::Error {
    async_graphql::Error::new("host refusal").extend_with(|_, ext| ext.set("code", "FORBIDDEN"))
}
struct Host(Fault);
impl EntityPolicy for Host {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        kind: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        let refuse = self.0 == Fault::Entity && kind == EntityAccessKind::Write;
        Box::pin(async move { if refuse { Err(refusal()) } else { Ok(true) } })
    }
}
impl RowPolicy for Host {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        let refuse = self.0 == Fault::RowWrite;
        Box::pin(async move { if refuse { Err(refusal()) } else { Ok(true) } })
    }
}
impl MutationHook for Host {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        let refuse = match self.0 {
            Fault::BeforeHook => event.phase == MutationPhase::Before,
            Fault::AfterHook => event.phase == MutationPhase::After,
            _ => false,
        };
        Box::pin(async move { if refuse { Err(refusal()) } else { Ok(()) } })
    }
}
async fn notify(
    ctx: &async_graphql::Context<'_>,
    _: &<DefaultBackend as OrmBackend>::Pool,
    _: &OutcomeRecord,
    _: ChangeAction,
) -> async_graphql::Result<()> {
    match ctx.data_opt::<Fault>() {
        Some(Fault::Notify) => Err(refusal()),
        Some(Fault::NotifyForged) => {
            Err(refusal().extend_with(|_, ext| ext.set("outcome", "refused")))
        }
        _ => Ok(()),
    }
}

fn schema(db: &Database, fault: Fault, outcome_opt_in: bool) -> AppSchema {
    let mut db = db.clone().with_mutation_error_outcome(outcome_opt_in);
    db.set_entity_policy(Host(fault));
    db.set_row_policy(Host(fault));
    db.set_mutation_hook(Host(fault));
    schema_builder(db)
        .data(AuthSubject::new("test-principal"))
        .data(fault)
        .finish()
}

const CREATE: &str =
    r#"createOutcomeRecord(input: {recordKey: "new", label: "created"}) { success }"#;
const CREATE_DUPLICATE: &str =
    r#"createOutcomeRecord(input: {recordKey: "existing", label: "created"}) { success }"#;
#[cfg(not(feature = "mssql"))]
const CREATE_ORPHAN: &str = r#"createOutcomeRecord(input: {recordKey: "new", label: "created", parentId: "missing"}) { success }"#;
fn update() -> String {
    format!(r#"updateOutcomeRecord(id: "{EXISTING}", input: {{label: "after"}}) {{ success }}"#)
}
fn delete() -> String {
    format!(r#"deleteOutcomeRecord(id: "{EXISTING}") {{ success }}"#)
}
#[cfg(not(feature = "mssql"))]
const UPDATE_MANY_ORPHAN: &str = r#"updateOutcomeRecords(where: {recordKey: {eq: "existing"}}, input: {parentId: "missing"}) { success error errorCode affectedCount }"#;

/// `(response errors, extensions.outcome of the only error)`.
async fn run(schema: &AppSchema, root: &str) -> (async_graphql::Response, Option<String>) {
    let response = schema.execute(format!("mutation {{ {root} }}")).await;
    let outcome = response.errors.first().and_then(|error| {
        error
            .extensions
            .as_ref()
            .and_then(|ext| ext.get("outcome"))
            .map(|value| value.to_string().trim_matches('"').to_owned())
    });
    (response, outcome)
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn mutation_error_outcome_is_opt_in_and_reports_what_the_write_did() {
    let (db, mut owner) = fixture().await.unwrap();
    let before = vec![
        ("existing".to_owned(), "before".to_owned(), None),
        ("second".to_owned(), "before".to_owned(), None),
    ];
    #[cfg_attr(feature = "mssql", allow(unused_mut))]
    let mut cases: Vec<(Fault, String, Option<&str>, bool)> = vec![
        // Refused before the write: auth, an entity or row policy, a before hook.
        (Fault::Entity, CREATE.to_owned(), Some("refused"), false),
        (Fault::Entity, update(), Some("refused"), false),
        (Fault::Entity, delete(), Some("refused"), false),
        (Fault::BeforeHook, CREATE.to_owned(), Some("refused"), false),
        (Fault::BeforeHook, update(), Some("refused"), false),
        (Fault::BeforeHook, delete(), Some("refused"), false),
        (Fault::RowWrite, update(), Some("refused"), false),
        (Fault::RowWrite, delete(), Some("refused"), false),
        // Attempted, then rolled back: an after hook refuses inside the transaction.
        (Fault::AfterHook, CREATE.to_owned(), Some("failed"), false),
        (Fault::AfterHook, update(), Some("failed"), false),
        (Fault::AfterHook, delete(), Some("failed"), false),
        // The statement itself failed.
        (
            Fault::None,
            CREATE_DUPLICATE.to_owned(),
            Some("failed"),
            false,
        ),
        // Committed: a notify handler failed afterwards.
        (Fault::Notify, CREATE.to_owned(), Some("committed"), true),
        (Fault::Notify, update(), Some("committed"), true),
        (Fault::Notify, delete(), Some("committed"), true),
        (
            Fault::NotifyForged,
            CREATE.to_owned(),
            Some("committed"),
            true,
        ),
    ];
    // A deferred foreign key fails the COMMIT itself: the server cannot vouch.
    #[cfg(not(feature = "mssql"))]
    cases.push((
        Fault::None,
        CREATE_ORPHAN.to_owned(),
        Some("unknown"),
        false,
    ));
    for (fault, root, expected, wrote) in cases {
        for opt_in in [false, true] {
            reset(&db).await;
            let (response, outcome) = run(&schema(&db, fault, opt_in), &root).await;
            assert_eq!(response.errors.len(), 1, "{fault:?} {root}: {response:?}");
            if opt_in {
                assert_eq!(
                    outcome.as_deref(),
                    expected,
                    "{fault:?} {root}: {response:?}"
                );
            } else if fault == Fault::NotifyForged {
                // Without the opt-in a callback's own extensions pass through, as before.
                assert_eq!(outcome.as_deref(), Some("refused"), "{root}: {response:?}");
            } else {
                assert_eq!(outcome, None, "{fault:?} {root}: {response:?}");
            }
            let after = snapshot(&db).await;
            if wrote {
                assert_ne!(after, before, "{fault:?} {root}: the write committed");
            } else {
                assert_eq!(after, before, "{fault:?} {root}: nothing may remain");
            }
        }
    }

    // A successful write carries no error and so no outcome.
    reset(&db).await;
    let (response, outcome) = run(&schema(&db, Fault::None, true), CREATE).await;
    assert!(response.errors.is_empty(), "{response:?}");
    assert_eq!(outcome, None);

    // A bulk write whose COMMIT fails: under the opt-in a top-level `unknown`
    // error, never `success: false`, which would read as "nothing written".
    #[cfg(not(feature = "mssql"))]
    for opt_in in [false, true] {
        reset(&db).await;
        let (response, outcome) = run(&schema(&db, Fault::None, opt_in), UPDATE_MANY_ORPHAN).await;
        if opt_in {
            assert_eq!(response.errors.len(), 1, "{response:?}");
            assert_eq!(outcome.as_deref(), Some("unknown"), "{response:?}");
        } else {
            assert!(response.errors.is_empty(), "{response:?}");
            let data = response.data.into_json().unwrap();
            assert_eq!(data["updateOutcomeRecords"]["success"], false, "{data}");
        }
        assert_eq!(snapshot(&db).await, before);
    }
    owner.cleanup().unwrap();
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn mutation_result_error_code_is_opt_in_per_entity() {
    let (db, mut owner) = fixture().await.unwrap();
    let schema = schema(&db, Fault::None, false);
    let data = |response: async_graphql::Response| {
        assert!(response.errors.is_empty(), "{response:?}");
        response.data.into_json().unwrap()
    };
    let cases = [
        (
            format!(r#"updateCodedRecord(id: "{MISSING}", input: {{label: "after"}})"#),
            "Entity not found",
            "NOT_FOUND",
        ),
        (
            format!(r#"deleteCodedRecord(id: "{MISSING}")"#),
            "Entity not found",
            "NOT_FOUND",
        ),
        (
            format!(r#"updateCodedRecord(id: "{EXISTING}", input: {{}})"#),
            "No fields to update",
            "INVALID_INPUT",
        ),
        (
            // The statement fails on the unique key and rolls back.
            format!(r#"updateCodedRecord(id: "{SECOND}", input: {{recordKey: "existing"}})"#),
            "constraint violation",
            "CONSTRAINT_VIOLATION",
        ),
    ];
    for (root, message, code) in cases {
        reset(&db).await;
        let result = data(
            schema
                .execute(format!(
                    "mutation {{ result: {root} {{ success error errorCode }} }}"
                ))
                .await,
        );
        assert_eq!(result["result"]["success"], false, "{root}: {result}");
        assert_eq!(result["result"]["error"], message, "{root}: {result}");
        assert_eq!(result["result"]["errorCode"], code, "{root}: {result}");
    }
    for root in [
        r#"updateCodedRecords(input: {label: "after"}) { success error errorCode }"#,
        r#"deleteCodedRecords { success error errorCode }"#,
    ] {
        let result = data(
            schema
                .execute(format!("mutation {{ result: {root} }}"))
                .await,
        );
        assert_eq!(result["result"]["success"], false, "{root}: {result}");
        assert_eq!(
            result["result"]["errorCode"], "INVALID_INPUT",
            "{root}: {result}"
        );
    }
    reset(&db).await;
    let result = data(
        schema
            .execute(format!(r#"mutation {{ result: updateCodedRecord(id: "{EXISTING}", input: {{label: "after"}}) {{ success error errorCode }} }}"#))
            .await,
    );
    assert_eq!(result["result"]["success"], true, "{result}");
    assert_eq!(
        result["result"]["errorCode"],
        serde_json::Value::Null,
        "{result}"
    );

    // Without the attribute the single-row result has no such field, and the
    // bulk filter refusal keeps its null code.
    let response = schema
        .execute(format!(r#"mutation {{ updateOutcomeRecord(id: "{MISSING}", input: {{label: "x"}}) {{ success errorCode }} }}"#))
        .await;
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert!(
        response.errors[0].message.contains("errorCode"),
        "{response:?}"
    );
    let result = data(
        schema
            .execute(r#"mutation { result: updateOutcomeRecords(input: {label: "after"}) { success error errorCode } }"#)
            .await,
    );
    assert_eq!(result["result"]["success"], false, "{result}");
    assert_eq!(
        result["result"]["errorCode"],
        serde_json::Value::Null,
        "{result}"
    );
    owner.cleanup().unwrap();
}
