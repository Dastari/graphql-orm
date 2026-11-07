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
        mutation_shapes = "create,upsert,update,delete",
        notify_with = "notify",
        upsert = "record_key"
    )
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(
        backend = "postgres",
        table = "write_records",
        plural = "WriteRecords",
        mutation_shapes = "create,upsert,update,delete",
        notify_with = "notify",
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
        mutation_shapes = "create,upsert,update,delete",
        notify_with = "notify",
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
            Database::new(
                graphql_orm::sqlx::sqlite::SqlitePoolOptions::new()
                    .max_connections(2)
                    .connect("sqlite::memory:")
                    .await?,
            ),
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
#[derive(Clone, Copy, Debug)]
enum Mode {
    Driver,
    Plain,
    Coded,
    Converted,
    Sourced,
}
struct HostRefusal;
impl From<HostRefusal> for async_graphql::Error {
    fn from(_: HostRefusal) -> Self {
        Self::new("host refusal").extend_with(|_, ext| ext.set("detail", "preserved"))
    }
}
async fn fail(db: &Database, mode: Mode) -> async_graphql::Result<()> {
    match mode {
        Mode::Driver => {
            <DefaultBackend as WriteBackend>::execute_with_binds(
                db.pool(),
                "SELECT private_column FROM private_missing_table",
                &[],
            )
            .await?;
            Ok(())
        }
        Mode::Plain => Err(async_graphql::Error::new("host refusal")),
        Mode::Coded => Err(
            async_graphql::Error::from(std::io::Error::other("host refusal")).extend_with(
                |_, ext| {
                    ext.set("code", "CUSTOM_REFUSAL");
                    ext.set("detail", "preserved");
                },
            ),
        ),
        Mode::Converted => {
            Err(HostRefusal)?;
            Ok(())
        }
        Mode::Sourced => {
            Err(std::io::Error::other("private source detail"))?;
            Ok(())
        }
    }
}
struct Callback {
    mode: Mode,
    phase: MutationPhase,
}
impl MutationHook for Callback {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.phase == self.phase {
                if matches!(self.mode, Mode::Driver) {
                    ctx.fetch_rows("SELECT private_column FROM private_missing_table", &[])
                        .await?;
                    Ok(())
                } else {
                    fail(ctx.database(), self.mode).await
                }
            } else {
                Ok(())
            }
        })
    }
}
impl RowPolicy for Callback {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            fail(db, self.mode).await?;
            Ok(true)
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            fail(db, self.mode).await?;
            Ok(true)
        })
    }
}
impl WriteInputTransform for Callback {
    fn before_create<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move { fail(db, self.mode).await })
    }
    fn before_update<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move { fail(db, self.mode).await })
    }
    fn before_upsert<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: &'a mut (dyn Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move { fail(db, self.mode).await })
    }
}
struct Fields(Option<Mode>);
impl FieldPolicy for Fields {
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
        db: &'a Database,
        _: &'static str,
        _: &'static str,
        _: Option<&'static str>,
        _: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if let Some(mode) = self.0 {
                fail(db, mode).await?;
            }
            Ok(true)
        })
    }
}
impl EntityPolicy for Callback {
    fn can_access_entity<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        db: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessKind,
        _: EntityAccessSurface,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            fail(db, self.mode).await?;
            Ok(true)
        })
    }
}
// Single-row-only schemas historically accept any error convertible into GraphQL.
struct NotifyFailure(async_graphql::Error);
impl From<NotifyFailure> for async_graphql::Error {
    fn from(error: NotifyFailure) -> Self {
        error.0
    }
}
async fn notify(
    ctx: &async_graphql::Context<'_>,
    _: &<DefaultBackend as OrmBackend>::Pool,
    _: &WriteRecord,
    _: ChangeAction,
) -> std::result::Result<(), NotifyFailure> {
    if let Some(mode) = ctx.data_opt::<Mode>() {
        fail(ctx.data_unchecked::<Database>(), *mode)
            .await
            .map_err(NotifyFailure)?;
    }
    Ok(())
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn single_row_callback_contract() {
    let (base_db, mut owner) = fixture().await.unwrap();
    for source in [
        "before",
        "after",
        "row",
        "transform",
        "notify",
        "field",
        "entity",
    ] {
        for mode in [
            Mode::Driver,
            Mode::Plain,
            Mode::Coded,
            Mode::Converted,
            Mode::Sourced,
        ] {
            for op in [
                "create",
                "update",
                "upsert_create",
                "upsert_update",
                "delete",
            ] {
                // Creates have no existing row policy; deletes have no write input.
                if (source == "row" && matches!(op, "create" | "upsert_create"))
                    || (matches!(source, "transform" | "field") && op == "delete")
                {
                    continue;
                }
                let mut db = base_db.clone();
                db.set_field_policy(Fields(if source == "field" { Some(mode) } else { None }));
                execute(&db, "DROP TABLE IF EXISTS write_records")
                    .await
                    .unwrap();
                #[cfg(feature = "sqlite")]
                let ddl = "CREATE TABLE write_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
                #[cfg(feature = "postgres")]
                let ddl = "CREATE TABLE write_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL)";
                #[cfg(feature = "mssql")]
                let ddl = "CREATE TABLE write_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, required NVARCHAR(100) NOT NULL, optional NVARCHAR(100), other NVARCHAR(100) NOT NULL)";
                execute(&db, ddl).await.unwrap();
                execute(&db, "INSERT INTO write_records VALUES ('00000000-0000-0000-0000-000000000001', 'existing', 'before', NULL, 'before')").await.unwrap();
                let cb = Callback {
                    mode,
                    phase: if source == "after" {
                        MutationPhase::After
                    } else {
                        MutationPhase::Before
                    },
                };
                match source {
                    "before" | "after" => db.set_mutation_hook(cb),
                    "row" => db.set_row_policy(cb),
                    "transform" => db.set_write_input_transform(cb),
                    "entity" => db.set_entity_policy(cb),
                    _ => (),
                }
                let mut builder = schema_builder(db).data(AuthSubject::new("test-principal"));
                if source == "notify" {
                    builder = builder.data(mode);
                }
                let schema = builder.finish();
                let call = match op {
                    "create" => {
                        r#"createWriteRecord(input: {recordKey: "new", required: "value", other: "value"})"#
                    }
                    "upsert_create" => {
                        r#"upsertWriteRecord(input: {recordKey: "new", required: "value", other: "value"})"#
                    }
                    "upsert_update" => {
                        r#"upsertWriteRecord(input: {recordKey: "existing", required: "value", other: "value"})"#
                    }
                    "update" => {
                        r#"updateWriteRecord(id: "00000000-0000-0000-0000-000000000001", input: {required: "after", other: "after"})"#
                    }
                    _ => r#"deleteWriteRecord(id: "00000000-0000-0000-0000-000000000001")"#,
                };
                let response = schema
                    .execute(format!("mutation {{ {call} {{ success }} }}"))
                    .await;
                assert_eq!(
                    response.errors.len(),
                    1,
                    "{source} {mode:?} {op}: {response:?}"
                );
                let error = &response.errors[0];
                let masked = matches!(mode, Mode::Driver | Mode::Sourced);
                assert_eq!(
                    error.message,
                    if masked {
                        "internal error"
                    } else {
                        "host refusal"
                    },
                    "{source} {mode:?} {op}: {response:?}"
                );
                let expected = if masked {
                    async_graphql::Error::new("internal error")
                        .extend_with(|_, ext| ext.set("code", "INTERNAL_ERROR"))
                } else {
                    match mode {
                        Mode::Plain => async_graphql::Error::new("host refusal"),
                        Mode::Coded => {
                            async_graphql::Error::new("host refusal").extend_with(|_, ext| {
                                ext.set("code", "CUSTOM_REFUSAL");
                                ext.set("detail", "preserved");
                            })
                        }
                        _ => HostRefusal.into(),
                    }
                };
                assert_eq!(
                    error.extensions, expected.extensions,
                    "{source} {mode:?} {op}"
                );
                assert!(!format!("{response:?}").contains("private_"));
            }
        }
    }
    owner.cleanup().unwrap();
}
