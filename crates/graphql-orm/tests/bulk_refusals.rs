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
