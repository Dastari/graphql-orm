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
    note: Option<String>,
}
#[cfg(feature = "sqlite")]
schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [WriteRecord] }
#[cfg(feature = "postgres")]
schema_roots! { backend: "postgres", query_custom_ops: [], entities: [WriteRecord] }
#[cfg(feature = "mssql")]
schema_roots! { backend: "mssql", schema_policy: "external_writable", query_custom_ops: [], entities: [WriteRecord] }
struct Deny {
    legacy: bool,
    fail: bool,
}
impl FieldPolicy for Deny {
    fn ignore_denied_field_write(&self, entity: &'static str, field: &'static str) -> bool {
        self.legacy && entity == "WriteRecord" && matches!(field, "optional" | "required")
    }
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
        ctx: &'a async_graphql::Context<'_>,
        _: &'a Database,
        _: &'static str,
        field: &'static str,
        _: Option<&'static str>,
        record: Option<&'a (dyn Any + Send + Sync)>,
        _: Option<&'a (dyn Any + Send + Sync)>,
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if self.fail {
                return Err(async_graphql::Error::new(format!(
                    "policy error for {field}"
                )));
            }
            if record
                .and_then(|record| record.downcast_ref::<WriteRecord>())
                .is_some_and(|record| record.record_key == "allowed")
            {
                return Ok(true);
            }
            Ok(ctx.data::<String>()? != field)
        })
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

async fn run(operation: &str, field: &str, legacy: bool, fail: bool) {
    let (mut db, mut owner) = fixture().await.unwrap();
    #[cfg(feature = "sqlite")]
    let ddl = "CREATE TABLE write_records (id TEXT PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL, note TEXT)";
    #[cfg(feature = "postgres")]
    let ddl = "CREATE TABLE write_records (id UUID PRIMARY KEY, record_key TEXT UNIQUE NOT NULL, required TEXT NOT NULL, optional TEXT, other TEXT NOT NULL, note TEXT)";
    #[cfg(feature = "mssql")]
    let ddl = "CREATE TABLE write_records (id UNIQUEIDENTIFIER PRIMARY KEY, record_key NVARCHAR(100) UNIQUE NOT NULL, required NVARCHAR(100) NOT NULL, optional NVARCHAR(100), other NVARCHAR(100) NOT NULL, note NVARCHAR(100))";
    execute(&db, ddl).await.unwrap();
    let id = graphql_orm::uuid::Uuid::new_v4();
    let seed = DefaultBackend::normalize_sql(
        "INSERT INTO write_records VALUES (?, 'existing', 'before', 'before', 'before', 'before')",
        1,
    );
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), &seed, &[SqlValue::Uuid(id)])
        .await
        .unwrap();
    if operation.starts_with("bulk") {
        let seed = DefaultBackend::normalize_sql(
            "INSERT INTO write_records VALUES (?, 'allowed', 'before', 'before', 'before', 'before')",
            1,
        );
        <DefaultBackend as WriteBackend>::execute_with_binds(
            db.pool(),
            &seed,
            &[SqlValue::Uuid(graphql_orm::uuid::Uuid::new_v4())],
        )
        .await
        .unwrap();
    }
    db.set_field_policy(Deny { legacy, fail });
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("test-principal"))
        .data(field.to_string())
        .finish();
    let input = if operation == "create" || operation.starts_with("upsert") {
        let key = if operation == "upsert_existing" {
            "existing"
        } else {
            "new"
        };
        format!("recordKey: \"{key}\", required: \"after\", optional: \"after\", other: \"after\"")
    } else if field == "other" {
        "other: \"after\"".to_string()
    } else {
        format!("{field}: \"after\", other: \"after\"")
    };
    let input = if operation.ends_with("_null") {
        "optional: null, other: \"after\"".to_string()
    } else {
        input
    };
    let call = match operation {
        "create" => format!("createWriteRecord(input: {{{input}}})"),
        "upsert_new" | "upsert_existing" => format!("upsertWriteRecord(input: {{{input}}})"),
        "update" | "update_null" => format!("updateWriteRecord(id: \"{id}\", input: {{{input}}})"),
        "bulk" | "bulk_null" => {
            format!(
                "updateWriteRecords(where: {{recordKey: {{inList: [\"existing\", \"allowed\"]}}}}, input: {{{input}}})"
            )
        }
        _ => unreachable!(),
    };
    let response = schema
        .execute(format!(
            "mutation {{ {call} {{ success error {} }} }}",
            if operation.starts_with("bulk") {
                "errorCode"
            } else {
                ""
            }
        ))
        .await;
    let data = response.data.into_json().unwrap();
    let notes = <DefaultBackend as OrmBackend>::fetch_rows(
        db.pool(),
        "SELECT note FROM write_records WHERE record_key = 'existing'",
        &[],
    )
    .await
    .unwrap();
    let existing_note = DefaultBackend::try_get_optional_string(&notes[0], "note").unwrap();
    // No field is denied (`none`), or the denied key-less optional field was
    // left out of an upsert that inserts: the insert branch asks only about
    // client-supplied fields.
    if field == "none" || (field == "note" && operation == "upsert_new") {
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert!(
            data.as_object()
                .unwrap()
                .values()
                .all(|v| v["success"] == true),
            "{data}"
        );
        // The conflict update still clears the omitted optional field: the
        // statement is unchanged, only the decision is now asked.
        let expected = (operation != "upsert_existing").then(|| "before".to_string());
        assert_eq!(existing_note, expected);
        owner.cleanup().unwrap();
        return;
    }
    if !ignored_operation(operation, field, legacy, fail) {
        assert_eq!(existing_note.as_deref(), Some("before"));
    }
    let ignored = ignored_operation(operation, field, legacy, fail);
    if ignored {
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert!(
            data.as_object()
                .unwrap()
                .values()
                .all(|v| v["success"] == true),
            "{data}"
        );
    } else {
        assert!(
            !response.errors.is_empty()
                || data
                    .as_object()
                    .is_some_and(|o| o.values().any(|v| v["success"] == false)),
            "{operation}/{field}: {data}"
        );
        let diagnostic = format!("{:?} {data}", response.errors);
        // Bulk policy callback error mapping is a separate contract; ordinary
        // field denials must retain the field name through that path too.
        if !fail {
            assert!(diagnostic.contains(field), "{diagnostic}");
        }
        if !fail {
            if operation.starts_with("bulk") {
                assert!(response.errors.is_empty());
                assert!(
                    data.as_object()
                        .unwrap()
                        .values()
                        .all(|v| v["errorCode"] == "FORBIDDEN")
                );
            } else {
                assert_eq!(
                    response.errors[0].extensions.as_ref().unwrap().get("code"),
                    Some(&async_graphql::Value::from("FORBIDDEN"))
                );
            }
        }
    }
    let rows = <DefaultBackend as OrmBackend>::fetch_rows(
        db.pool(),
        "SELECT required, optional, other FROM write_records",
        &[],
    )
    .await
    .unwrap();
    let rows: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                DefaultBackend::try_get_string(row, "required").unwrap(),
                DefaultBackend::try_get_optional_string(row, "optional").unwrap(),
                DefaultBackend::try_get_string(row, "other").unwrap(),
            )
        })
        .collect();
    if ignored {
        assert!(rows.iter().any(|row| row.2 == "after"), "{rows:?}");
        assert!(
            rows.iter().all(|row| if field == "optional" {
                row.1.as_deref() != Some("after")
            } else {
                row.0 != "after"
            }),
            "{rows:?}"
        );
    } else {
        assert_eq!(
            rows,
            vec![
                ("before".into(), Some("before".into()), "before".into());
                if operation.starts_with("bulk") { 2 } else { 1 }
            ]
        );
    }
    owner.cleanup().unwrap();
}
fn ignored_operation(operation: &str, field: &str, legacy: bool, fail: bool) -> bool {
    legacy && !fail && (field == "optional" || matches!(operation, "update" | "bulk"))
}
macro_rules! case {
    ($name:ident, $op:literal, $field:literal) => {
        #[tokio::test]
        #[cfg_attr(
            any(feature = "postgres", feature = "mssql"),
            ignore = "requires test-owned disposable database"
        )]
        async fn $name() {
            run($op, $field, false, false).await;
        }
    };
}
case!(create_optional, "create", "optional");
case!(create_required, "create", "required");
case!(update_optional, "update", "optional");
case!(update_required, "update", "required");
case!(upsert_new_optional, "upsert_new", "optional");
case!(upsert_new_required, "upsert_new", "required");
case!(upsert_existing_optional, "upsert_existing", "optional");
case!(upsert_existing_required, "upsert_existing", "required");
case!(bulk_optional, "bulk", "optional");
case!(bulk_required, "bulk", "required");

#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn explicit_opt_out_preserves_legacy_omission() {
    for operation in ["create", "update", "upsert_new", "upsert_existing", "bulk"] {
        for field in ["optional", "required"] {
            run(operation, field, true, false).await;
        }
    }
}
#[tokio::test]
#[cfg_attr(
    any(feature = "postgres", feature = "mssql"),
    ignore = "requires test-owned disposable database"
)]
async fn explicit_opt_out_never_swallows_policy_errors() {
    for operation in ["create", "update", "upsert_new", "upsert_existing", "bulk"] {
        run(operation, "optional", true, true).await;
    }
}

// `other` declares no `write_policy`: the write decision is asked for it with
// `None`, and a policy that denies it by name is obeyed on every write path.
case!(create_keyless, "create", "other");
case!(update_keyless, "update", "other");
case!(upsert_new_keyless, "upsert_new", "other");
case!(upsert_existing_keyless, "upsert_existing", "other");
case!(bulk_keyless, "bulk", "other");

case!(update_optional_null, "update_null", "optional");
case!(bulk_optional_null, "bulk_null", "optional");

// `note` is key-less, optional and left out of every upsert input here. The
// conflict update writes it as NULL on an existing row, so the decision is
// asked with the current row and `None`; an upsert that inserts does not ask
// about a field the client did not supply.
case!(
    upsert_existing_omitted_keyless_optional,
    "upsert_existing",
    "note"
);
case!(upsert_new_omitted_keyless_optional, "upsert_new", "note");
case!(upsert_existing_allowed, "upsert_existing", "none");
