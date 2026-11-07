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
use graphql_orm::prelude::*;
use std::any::Any;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "write_records",
        plural = "WriteRecords",
        upsert = "record_key"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "write_records",
        plural = "WriteRecords",
        upsert = "record_key"
    )
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "write_records",
        plural = "WriteRecords",
        schema_policy = "external_writable",
        upsert = "record_key"
    )
)]
struct WriteRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    #[sortable]
    #[filterable(type = "string")]
    record_key: String,
    #[graphql_orm(write_policy = "guard")]
    required: String,
    #[graphql_orm(write_policy = "guard")]
    optional: Option<String>,
    other: String,
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [WriteRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [WriteRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [WriteRecord] }
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

use async_graphql::ErrorExtensions;
fn refusal() -> async_graphql::Error {
    async_graphql::Error::new("application refusal")
        .extend_with(|_, ext| ext.set("code", "CUSTOM_REFUSAL"))
}
struct Refuse;
impl MutationHook for Refuse {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        _: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async { Err(refusal()) })
    }
}
// A hook failing through `?` is not an authored refusal: its text stays private.
struct FailingHook;
impl MutationHook for FailingHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        _: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async {
            Err(graphql_orm::sqlx::Error::Protocol(
                "SELECT secret FROM private_table".into(),
            ))?;
            Ok(())
        })
    }
}
impl RowPolicy for Refuse {
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
        Box::pin(async { Err(refusal()) })
    }
}
impl WriteInputTransform for Refuse {
    fn before_update<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async { Err(refusal()) })
    }
}
async fn run_refusal(source: &str, delete: bool) {
    let (mut db, mut owner) = fixture().await.unwrap();
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE write_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE write_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE write_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, required NVARCHAR(100) NOT NULL, optional NVARCHAR(100), other NVARCHAR(100) NOT NULL)";
    execute(&db, ddl).await.unwrap();
    execute(&db, "INSERT INTO write_records VALUES ('00000000-0000-0000-0000-000000000001', 'existing', 'before', 'before', 'before')").await.unwrap();
    match source {
        "hook" => db.set_mutation_hook(Refuse),
        "failing_hook" => db.set_mutation_hook(FailingHook),
        "row" => db.set_row_policy(Refuse),
        "transform" => db.set_write_input_transform(Refuse),
        "database" => execute(&db, "DROP TABLE write_records").await.unwrap(),
        _ => unreachable!(),
    }
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test-principal"))
        .finish();
    let masked = matches!(source, "database" | "failing_hook");
    if !masked {
        let single = if delete {
            "deleteWriteRecord(id: \"00000000-0000-0000-0000-000000000001\")"
        } else {
            "updateWriteRecord(id: \"00000000-0000-0000-0000-000000000001\", input: {other: \"after\"})"
        };
        let response = schema
            .execute(format!("mutation {{ {single} {{ success error }} }}"))
            .await;
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(response.errors[0].message, "application refusal");
        assert_eq!(
            response.errors[0].extensions.as_ref().unwrap().get("code"),
            Some(&async_graphql::Value::from("CUSTOM_REFUSAL"))
        );
    }
    let call = if delete {
        "deleteWriteRecords(where: {recordKey: {eq: \"existing\"}})"
    } else {
        "updateWriteRecords(where: {recordKey: {eq: \"existing\"}}, input: {other: \"after\"})"
    };
    let response = schema
        .execute(format!(
            "mutation {{ {call} {{ success error errorCode }} }}"
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let payload = data.as_object().unwrap().values().next().unwrap();
    assert_eq!(payload["success"], false);
    assert_eq!(
        payload["error"],
        if masked {
            "internal error"
        } else {
            "application refusal"
        }
    );
    assert_eq!(
        payload["errorCode"],
        if masked {
            "INTERNAL_ERROR"
        } else {
            "CUSTOM_REFUSAL"
        }
    );
    if source != "database" {
        let rows = <DefaultBackend as OrmBackend>::fetch_rows(
            db.pool(),
            "SELECT other FROM write_records",
            &[],
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            DefaultBackend::try_get_string(&rows[0], "other").unwrap(),
            "before"
        );
    }
    owner.cleanup().unwrap();
}
macro_rules! case {
    ($name:ident, $source:literal, $delete:literal) => {
        #[tokio::test]
        #[cfg_attr(
            any(feature = "postgres", feature = "mssql"),
            ignore = "requires test-owned disposable database"
        )]
        async fn $name() {
            run_refusal($source, $delete).await;
        }
    };
}
case!(update_hook, "hook", false);
case!(delete_hook, "hook", true);
case!(update_row, "row", false);
case!(delete_row, "row", true);
case!(update_transform, "transform", false);
case!(update_failing_hook, "failing_hook", false);
case!(delete_failing_hook, "failing_hook", true);
case!(update_database, "database", false);
case!(delete_database, "database", true);

#[derive(Clone, Default)]
struct PerRowTransform {
    calls: std::sync::Arc<std::sync::Mutex<Vec<(String, WriteOrigin)>>>,
    refuse_second: bool,
}
impl WriteInputTransform for PerRowTransform {
    fn before_create_with_context<'a>(
        &'a self,
        ctx: &'a mut WriteInputContext<'_, '_>,
        input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let input = input.downcast_mut::<CreateWriteRecordInput>().unwrap();
            self.calls
                .lock()
                .unwrap()
                .push(("create".into(), ctx.origin()));
            input.other.push('!');
            Ok(())
        })
    }
    fn before_upsert_with_context<'a>(
        &'a self,
        ctx: &'a mut WriteInputContext<'_, '_>,
        input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        self.before_create_with_context(ctx, input)
    }
    fn before_update_with_context<'a>(
        &'a self,
        ctx: &'a mut WriteInputContext<'_, '_>,
        row: Option<&'a (dyn Any + Send + Sync)>,
        input: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let row = row.unwrap().downcast_ref::<WriteRecord>().unwrap();
            let mut calls = self.calls.lock().unwrap();
            calls.push((row.record_key.clone(), ctx.origin()));
            if self.refuse_second && calls.len() == 2 {
                return Err(refusal());
            }
            let input = input.downcast_mut::<UpdateWriteRecordInput>().unwrap();
            input.other = Some(format!(
                "{}:{}!",
                input.other.as_deref().unwrap(),
                row.record_key
            ));
            Ok(())
        })
    }
}
fn create_input(key: &str) -> CreateWriteRecordInput {
    CreateWriteRecordInput {
        record_key: key.into(),
        required: "before".into(),
        optional: None,
        other: "base".into(),
    }
}
fn update_input() -> UpdateWriteRecordInput {
    UpdateWriteRecordInput {
        other: Some("next".into()),
        ..Default::default()
    }
}
fn all_records() -> WriteRecordWhereInput {
    WriteRecordWhereInput {
        record_key: Some(StringFilter {
            contains: Some("key".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
struct AllowTransformFields;
impl FieldPolicy for AllowTransformFields {
    fn can_read_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_field<'a>(
        &'a self,
        _: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn can_write_repository_field<'a>(
        &'a self,
        _: Option<graphql_orm::graphql::auth::AccessContext<'a>>,
        _: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}
async fn transform_fixture() -> (Database, FixtureOwner) {
    let (mut db, owner) = fixture().await.unwrap();
    db.set_field_policy(AllowTransformFields);
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE write_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE write_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE write_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, required NVARCHAR(100) NOT NULL, optional NVARCHAR(100), other NVARCHAR(100) NOT NULL)";
    execute(&db, ddl).await.unwrap();
    (db, owner)
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn transforms_once_and_bulk_inputs_are_independent() {
    let (mut db, mut owner) = transform_fixture().await;
    let transform = PerRowTransform::default();
    let calls = transform.calls.clone();
    db.set_write_input_transform(transform);
    let row = WriteRecord::insert(&db, create_input("key-a"))
        .await
        .unwrap();
    assert_eq!(row.other, "base!");
    assert_eq!(
        std::mem::take(&mut *calls.lock().unwrap()),
        [("create".into(), WriteOrigin::Repository)]
    );
    WriteRecord::insert_many(&db, [create_input("key-b")])
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    calls.lock().unwrap().clear();
    for surface in [
        "repository",
        "graphql",
        "transaction",
        "bounded",
        "transaction_bounded",
    ] {
        match surface {
            "repository" => {
                assert_eq!(
                    WriteRecord::update_where(&db, all_records(), update_input())
                        .await
                        .unwrap(),
                    2
                );
            }
            "graphql" => {
                let response = schema_builder(db.clone()).data(AuthSubject::new("test-principal")).finish()
                    .execute(r#"mutation { updateWriteRecords(where: {recordKey: {contains: "key"}}, input: {other: "next"}) { success affectedCount } }"#).await;
                assert!(response.errors.is_empty(), "{response:?}");
                assert_eq!(
                    response.data.into_json().unwrap()["updateWriteRecords"]["success"],
                    true
                );
            }
            "transaction" => {
                db.transaction(TransactionMode::StateMachine, |tx| {
                    Box::pin(async move {
                        tx.update_where::<WriteRecord>(all_records(), update_input())
                            .await?;
                        Ok(())
                    })
                })
                .await
                .unwrap();
            }
            "transaction_bounded" => {
                db.transaction(TransactionMode::StateMachine, |tx| {
                    Box::pin(async move {
                        tx.update_where_bounded::<WriteRecord>(
                            all_records(),
                            update_input(),
                            MutationLimit::new(2).unwrap(),
                        )
                        .await?;
                        Ok(())
                    })
                })
                .await
                .unwrap();
            }
            "bounded" => {
                WriteRecord::update_where_bounded(
                    &db,
                    all_records(),
                    update_input(),
                    MutationLimit::new(2).unwrap(),
                )
                .await
                .unwrap();
            }
            _ => unreachable!(),
        }
        let mut seen = std::mem::take(&mut *calls.lock().unwrap());
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            seen.iter().map(|v| v.0.as_str()).collect::<Vec<_>>(),
            ["key-a", "key-b"],
            "{surface}"
        );
        let rows = EntityQuery::<WriteRecord>::new()
            .fetch_all(db.pool())
            .await
            .unwrap();
        for row in rows {
            assert_eq!(row.other, format!("next:{}!", row.record_key), "{surface}");
        }
    }
    owner.cleanup().unwrap();
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn second_row_refusal_rolls_back_entire_bulk_write() {
    let (mut db, mut owner) = transform_fixture().await;
    WriteRecord::insert_many(&db, [create_input("key-a"), create_input("key-b")])
        .await
        .unwrap();
    for surface in [
        "graphql",
        "repository",
        "transaction",
        "bounded",
        "transaction_bounded",
    ] {
        let transform = PerRowTransform {
            refuse_second: true,
            ..Default::default()
        };
        let calls = transform.calls.clone();
        db.set_write_input_transform(transform);
        match surface {
            "graphql" => {
                let response = schema_builder(db.clone()).data(AuthSubject::new("test-principal")).finish()
                    .execute(r#"mutation { updateWriteRecords(where: {recordKey: {contains: "key"}}, input: {other: "next"}) { success error errorCode } }"#).await;
                assert!(response.errors.is_empty(), "{response:?}");
                let data = response.data.into_json().unwrap();
                let payload = &data["updateWriteRecords"];
                assert_eq!(payload["success"], false);
                assert_eq!(payload["error"], "application refusal");
                assert_eq!(payload["errorCode"], "CUSTOM_REFUSAL");
            }
            "repository" => {
                assert!(
                    WriteRecord::update_where(&db, all_records(), update_input())
                        .await
                        .is_err()
                );
            }
            "transaction" => {
                assert!(
                    db.transaction(TransactionMode::StateMachine, |tx| Box::pin(async move {
                        tx.update_where::<WriteRecord>(all_records(), update_input())
                            .await?;
                        Ok(())
                    }))
                    .await
                    .is_err()
                );
            }
            "transaction_bounded" => {
                assert!(
                    db.transaction(TransactionMode::StateMachine, |tx| Box::pin(async move {
                        tx.update_where_bounded::<WriteRecord>(
                            all_records(),
                            update_input(),
                            MutationLimit::new(2).unwrap(),
                        )
                        .await?;
                        Ok(())
                    }))
                    .await
                    .is_err()
                );
            }
            "bounded" => {
                assert!(
                    WriteRecord::update_where_bounded(
                        &db,
                        all_records(),
                        update_input(),
                        MutationLimit::new(2).unwrap()
                    )
                    .await
                    .is_err()
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(calls.lock().unwrap().len(), 2, "{surface}");
        for row in EntityQuery::<WriteRecord>::new()
            .fetch_all(db.pool())
            .await
            .unwrap()
        {
            assert_eq!(row.other, "base", "{surface}");
        }
    }
    owner.cleanup().unwrap();
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn single_and_multi_write_paths_transform_once() {
    let (mut db, mut owner) = transform_fixture().await;
    let transform = PerRowTransform::default();
    let calls = transform.calls.clone();
    db.set_write_input_transform(transform);
    let check = |n: usize, origin: WriteOrigin| {
        let seen = std::mem::take(&mut *calls.lock().unwrap());
        assert_eq!(seen.len(), n, "{seen:?}");
        assert!(seen.iter().all(|v| v.1 == origin), "{seen:?}");
    };
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test-principal"))
        .finish();
    for operation in [
        "createWriteRecord",
        "upsertWriteRecord",
        "upsertWriteRecord",
    ] {
        let key = if operation == "createWriteRecord" {
            "key-create"
        } else {
            "key-upsert"
        };
        let response = schema.execute(format!(r#"mutation {{ {operation}(input: {{recordKey: "{key}", required: "before", other: "base"}}) {{ success }} }}"#)).await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            response.data.into_json().unwrap()[operation]["success"],
            true
        );
        check(1, WriteOrigin::GraphqlMutation);
    }
    let row = WriteRecord::insert(&db, create_input("key-repo"))
        .await
        .unwrap();
    check(1, WriteOrigin::Repository);
    WriteRecord::update_by_id(&db, &row.id, update_input())
        .await
        .unwrap();
    check(1, WriteOrigin::Repository);
    let response = schema
        .execute(format!(
            r#"mutation {{ updateWriteRecord(id: "{}", input: {{other: "next"}}) {{ success }} }}"#,
            row.id
        ))
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    check(1, WriteOrigin::GraphqlMutation);
    for _ in 0..2 {
        WriteRecord::upsert(&db, create_input("key-repo-upsert"))
            .await
            .unwrap();
        check(1, WriteOrigin::Repository);
    }
    WriteRecord::upsert_many(
        &db,
        [create_input("key-repo-upsert"), create_input("key-many")],
    )
    .await
    .unwrap();
    check(2, WriteOrigin::Repository);
    WriteRecord::insert_many(
        &db,
        [
            create_input("key-insert-many-a"),
            create_input("key-insert-many-b"),
        ],
    )
    .await
    .unwrap();
    check(2, WriteOrigin::Repository);
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            let row = tx.insert::<WriteRecord>(create_input("key-tx")).await?;
            tx.update_by_id::<WriteRecord>(&row.id, update_input())
                .await?;
            tx.upsert::<WriteRecord>(create_input("key-tx-upsert"))
                .await?;
            tx.upsert::<WriteRecord>(create_input("key-tx-upsert"))
                .await?;
            Ok(())
        })
    })
    .await
    .unwrap();
    check(4, WriteOrigin::Repository);
    let rows = WriteRecord::replace_all(
        &db,
        [create_input("key-replace-a"), create_input("key-replace-b")],
    )
    .await
    .unwrap();
    check(2, WriteOrigin::Repository);
    assert!(rows.iter().all(|row| row.other == "base!"));
    owner.cleanup().unwrap();
}

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn only_hook_writes_have_internal_hook_origin() {
    struct Nested(std::sync::atomic::AtomicBool);
    impl MutationHook for Nested {
        fn on_mutation<'a>(
            &'a self,
            _: Option<&'a async_graphql::Context<'_>>,
            tx: &'a mut MutationContext<'_>,
            _: &'a MutationEvent,
        ) -> BoxFuture<'a, async_graphql::Result<()>> {
            Box::pin(async move {
                if !self.0.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tx.insert::<WriteRecord>(create_input("key-nested"))
                        .await
                        .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
                }
                Ok(())
            })
        }
    }
    let (mut db, mut owner) = transform_fixture().await;
    let transform = PerRowTransform::default();
    let calls = transform.calls.clone();
    db.set_write_input_transform(transform);
    db.set_mutation_hook(Nested(std::sync::atomic::AtomicBool::new(false)));
    db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.insert::<WriteRecord>(create_input("key-outer")).await?;
            tx.insert::<WriteRecord>(create_input("key-after")).await?;
            Ok(())
        })
    })
    .await
    .unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|v| v.1)
            .collect::<Vec<_>>(),
        [
            WriteOrigin::Repository,
            WriteOrigin::InternalMutationHook,
            WriteOrigin::Repository
        ]
    );
    owner.cleanup().unwrap();
}
