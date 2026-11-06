#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "write_records",
        plural = "WriteRecords",
        upsert = "record_key",
        notify_with = "notify"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "write_records",
        plural = "WriteRecords",
        upsert = "record_key",
        notify_with = "notify"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "write_records",
        plural = "WriteRecords",
        schema_policy = "external_writable",
        upsert = "record_key",
        notify_with = "notify"
    )
)]
struct WriteRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    required: String,
    optional: Option<String>,
    other: String,
}
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "write_records",
        plural = "QuietRecords",
        upsert = "record_key",
        notify_with = "quiet_notify",
        notify_on_error = "report"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "write_records",
        plural = "QuietRecords",
        upsert = "record_key",
        notify_with = "quiet_notify",
        notify_on_error = "report"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "write_records",
        plural = "QuietRecords",
        schema_policy = "external_writable",
        upsert = "record_key",
        notify_with = "quiet_notify",
        notify_on_error = "report"
    )
)]
struct QuietRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    required: String,
    optional: Option<String>,
    other: String,
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [WriteRecord, QuietRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [WriteRecord, QuietRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [WriteRecord, QuietRecord] }
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
        let owner = FixtureOwner::start("field-write-denial")?;
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

#[derive(Default)]
struct Calls {
    rows: std::sync::Mutex<Vec<(String, ChangeAction)>>,
    errors: std::sync::Mutex<Vec<String>>,
}
struct Reporter(std::sync::Arc<Calls>);
impl PostCommitErrorHandler for Reporter {
    fn on_post_commit_error<'a>(&'a self, _: &'a Database, error: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.0.errors.lock().unwrap().push(error.into());
        })
    }
}
async fn notification(
    ctx: &async_graphql::Context<'_>,
    value: &str,
    action: ChangeAction,
) -> async_graphql::Result<()> {
    ctx.data_unchecked::<std::sync::Arc<Calls>>()
        .rows
        .lock()
        .unwrap()
        .push((value.into(), action));
    if value == "ok" {
        return Ok(());
    }
    if value == "masked" {
        Err(std::io::Error::other("private driver text"))?;
    }
    use async_graphql::ErrorExtensions;
    Err(async_graphql::Error::new("notification failed")
        .extend_with(|_, ext| ext.set("code", "NOTIFY_FAILED")))
}
async fn assert_committed(
    ctx: &async_graphql::Context<'_>,
    id: graphql_orm::uuid::Uuid,
    value: &str,
    action: ChangeAction,
) {
    let db = ctx.data_unchecked::<Database>();
    let rows = DefaultBackend::fetch_rows(
        db.pool(),
        &format!("SELECT other FROM write_records WHERE id = '{id}'"),
        &[],
    )
    .await
    .unwrap();
    if action == ChangeAction::Deleted {
        assert!(
            rows.is_empty(),
            "deleted pre-image must be absent on a separate pool read"
        );
    } else {
        assert_eq!(
            DefaultBackend::try_get_string(&rows[0], "other").unwrap(),
            value,
            "notification must see committed storage"
        );
    }
}
async fn notify(
    ctx: &async_graphql::Context<'_>,
    _: &<DefaultBackend as OrmBackend>::Pool,
    entity: &WriteRecord,
    action: ChangeAction,
) -> async_graphql::Result<()> {
    assert_committed(ctx, entity.id, &entity.other, action).await;
    notification(ctx, &entity.other, action).await
}
async fn quiet_notify(
    ctx: &async_graphql::Context<'_>,
    _: &<DefaultBackend as OrmBackend>::Pool,
    entity: &QuietRecord,
    action: ChangeAction,
) -> async_graphql::Result<()> {
    assert_committed(ctx, entity.id, &entity.other, action).await;
    notification(ctx, &entity.other, action).await
}
struct Refuse;
impl MutationHook for Refuse {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase == MutationPhase::After {
                return Err(async_graphql::Error::new("rollback"));
            }
            Ok(())
        })
    }
}
async fn setup() -> (Database, FixtureOwner, std::sync::Arc<Calls>) {
    let (mut db, owner) = fixture().await.unwrap();
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE write_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE write_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE write_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, required NVARCHAR(100) NOT NULL, optional NVARCHAR(100), other NVARCHAR(100) NOT NULL)";
    execute(&db, ddl).await.unwrap();
    for id in 1..=3 {
        execute(&db, &format!("INSERT INTO write_records VALUES ('00000000-0000-0000-0000-00000000000{id}', 'key{id}', 'before', NULL, 'before')")).await.unwrap();
    }
    let calls = std::sync::Arc::new(Calls::default());
    db.set_post_commit_error_handler(Reporter(calls.clone()));
    (db, owner, calls)
}
async fn values(db: &Database) -> Vec<String> {
    DefaultBackend::fetch_rows(db.pool(), "SELECT other FROM write_records", &[])
        .await
        .unwrap()
        .iter()
        .map(|r| DefaultBackend::try_get_string(r, "other").unwrap())
        .collect()
}
async fn bulk(report: bool, masked: bool) {
    let (db, mut owner, calls) = setup().await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .data(calls.clone())
        .finish();
    let plural = if report {
        "QuietRecords"
    } else {
        "WriteRecords"
    };
    let value = if masked { "masked" } else { "after" };
    for delete in [false, true] {
        let (op, input, count) = if delete {
            ("delete", String::new(), "deletedCount")
        } else {
            (
                "update",
                format!(", input: {{other: \"{value}\"}}"),
                "affectedCount",
            )
        };
        let response = schema.execute(format!("mutation {{ {op}{plural}(where: {{recordKey: {{startsWith: \"key\"}}}}{input}) {{ success {count} notificationErrors {{ message code }} }} }}")).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let json = response.data.into_json().unwrap();
        let result = json.as_object().unwrap().values().next().unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result[count], 3);
        assert_eq!(result["notificationErrors"].as_array().unwrap().len(), 3);
        assert_eq!(
            result["notificationErrors"][0]["message"],
            if masked {
                "internal error"
            } else {
                "notification failed"
            }
        );
        assert_eq!(
            result["notificationErrors"][0]["code"],
            if masked {
                "INTERNAL_ERROR"
            } else {
                "NOTIFY_FAILED"
            }
        );
        let rows = values(&db).await;
        assert_eq!(rows.len(), if delete { 0 } else { 3 });
        assert!(rows.iter().all(|v| v == value));
    }
    assert_eq!(calls.rows.lock().unwrap().len(), 6);
    assert!(
        calls.rows.lock().unwrap()[..3]
            .iter()
            .all(|(v, a)| v == value && *a == ChangeAction::Updated)
    );
    assert!(
        calls.rows.lock().unwrap()[3..]
            .iter()
            .all(|(v, a)| v == value && *a == ChangeAction::Deleted)
    );
    assert_eq!(calls.errors.lock().unwrap().len(), 6);
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn bulk_default() {
    bulk(false, false).await;
}
#[tokio::test]
async fn bulk_report() {
    bulk(true, false).await;
}
#[tokio::test]
async fn bulk_masks_converted_errors() {
    bulk(false, true).await;
}
async fn single(report: bool) {
    let (db, mut owner, calls) = setup().await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .data(calls.clone())
        .finish();
    let entity = if report { "QuietRecord" } else { "WriteRecord" };
    let response = schema.execute(format!("mutation {{ update{entity}(id: \"00000000-0000-0000-0000-000000000001\", input: {{other: \"after\"}}) {{ success }} }}")).await;
    assert_eq!(
        response.errors.len(),
        if report { 0 } else { 1 },
        "{response:?}"
    );
    if report {
        assert_eq!(
            response.data.into_json().unwrap()[format!("update{entity}")]["success"],
            true
        );
    }
    assert!(values(&db).await.contains(&"after".into()));
    assert_eq!(calls.rows.lock().unwrap().len(), 1);
    assert_eq!(calls.errors.lock().unwrap().len(), usize::from(report));
    owner.cleanup().unwrap();
}
#[tokio::test]
async fn single_default() {
    single(false).await;
}
#[tokio::test]
async fn single_report() {
    single(true).await;
}
#[tokio::test]
async fn rollback_and_refusal_do_not_notify() {
    let (mut db, mut owner, calls) = setup().await;
    db.set_mutation_hook(Refuse);
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .data(calls.clone())
        .finish();
    for filter in ["{}", "{recordKey: {startsWith: \"key\"}}"] {
        for delete in [false, true] {
            let op = if delete { "delete" } else { "update" };
            let input = if delete {
                ""
            } else {
                ", input: {other: \"after\"}"
            };
            let response = schema
                .execute(format!(
                    "mutation {{ {op}WriteRecords(where: {filter}{input}) {{ success }} }}"
                ))
                .await;
            assert!(response.errors.is_empty(), "{response:?}");
            assert_eq!(
                response.data.into_json().unwrap()[format!("{op}WriteRecords")]["success"],
                false
            );
            assert_eq!(values(&db).await, vec!["before"; 3]);
        }
    }
    assert!(calls.rows.lock().unwrap().is_empty());
    assert!(calls.errors.lock().unwrap().is_empty());
    owner.cleanup().unwrap();
}

#[tokio::test]
async fn notification_bound_refuses_before_writing() {
    let (db, mut owner, calls) = setup().await;
    for chunk in (4..=1001).collect::<Vec<_>>().chunks(500) {
        let rows = chunk
            .iter()
            .map(|id| {
                format!("('00000000-0000-0000-0000-{id:012}', 'key{id}', 'before', NULL, 'before')")
            })
            .collect::<Vec<_>>()
            .join(",");
        execute(&db, &format!("INSERT INTO write_records VALUES {rows}"))
            .await
            .unwrap();
    }
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .data(calls.clone())
        .finish();
    for operation in [
        r#"updateWriteRecords(where: {recordKey: {startsWith: "key"}}, input: {other: "after"}) { success affectedCount }"#,
        r#"deleteWriteRecords(where: {recordKey: {startsWith: "key"}}) { success deletedCount }"#,
    ] {
        let response = schema.execute(format!("mutation {{ {operation} }}")).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let data = response.data.into_json().unwrap();
        let payload = data.as_object().unwrap().values().next().unwrap();
        assert_eq!(payload["success"], false);
        assert_eq!(values(&db).await, vec!["before"; 1001]);
    }
    assert!(calls.rows.lock().unwrap().is_empty());
    owner.cleanup().unwrap();
}

#[tokio::test]
async fn repository_context_keeps_single_and_bounded_notification_contract() {
    let (db, mut owner, calls) = setup().await;
    let filter = || WriteRecordWhereInput {
        record_key: Some(graphql_orm::graphql::filters::StringFilter {
            starts_with: Some("key".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let input = || UpdateWriteRecordInput {
        other: Some("after".into()),
        ..Default::default()
    };
    WriteRecord::update_where_bounded(&db, filter(), input(), MutationLimit::new(3).unwrap())
        .await
        .unwrap();
    assert_eq!(values(&db).await, vec!["after"; 3]);
    let result = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.delete_where_bounded::<WriteRecord>(filter(), MutationLimit::new(3)?)
                    .await
                    .map_err(OrmPublicError::from)?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(result.is_err());
    assert_eq!(values(&db).await.len(), 3);
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.delete_where_bounded::<WriteRecord>(filter(), MutationLimit::new(3)?)
                .await
                .map_err(OrmPublicError::from)
        })
    })
    .await
    .unwrap();
    assert!(values(&db).await.is_empty());
    assert!(calls.rows.lock().unwrap().is_empty());
    owner.cleanup().unwrap();
}

#[tokio::test]
async fn successful_and_empty_bulk_have_no_notification_errors() {
    let (db, mut owner, calls) = setup().await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test"))
        .data(calls.clone())
        .finish();
    for (key, count) in [("key", 3), ("absent", 0)] {
        let response = schema.execute(format!("mutation {{ updateWriteRecords(where: {{recordKey: {{startsWith: \"{key}\"}}}}, input: {{other: \"ok\"}}) {{ success affectedCount notificationErrors {{ message }} }} }}")).await;
        assert!(response.errors.is_empty(), "{response:?}");
        let result = response.data.into_json().unwrap();
        assert_eq!(result["updateWriteRecords"]["success"], true);
        assert_eq!(result["updateWriteRecords"]["affectedCount"], count);
        assert!(result["updateWriteRecords"]["notificationErrors"].is_null());
    }
    assert_eq!(calls.rows.lock().unwrap().len(), 3);
    assert!(calls.errors.lock().unwrap().is_empty());
    owner.cleanup().unwrap();
}
