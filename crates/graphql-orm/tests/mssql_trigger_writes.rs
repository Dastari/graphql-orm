#![cfg(feature = "mssql")]
//! Generated MSSQL writes against tables that carry enabled `AFTER` triggers.
//!
//! The triggers write audit rows without `SET NOCOUNT ON`, so they emit their
//! own row counts, and they fire even when the triggering statement matches no
//! rows. SQL Server rejects `OUTPUT` without `INTO` on such tables, and a
//! statement's reported row count must exclude the trigger's own statements.

use graphql_orm::graphql::orm::WriteBackend;
use graphql_orm::prelude::*;

#[path = "support/owned_mssql.rs"]
mod owned_mssql;
use owned_mssql::{OwnedSqlServer, execute_batch};

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmTriggeredRecords",
    plural = "TriggeredRecords",
    schema_policy = "external_writable",
    upsert = "external_key"
)]
struct TriggeredRecord {
    #[primary_key]
    id: String,
    #[unique]
    #[filterable(type = "string")]
    external_key: String,
    value: i64,
    #[graphql_orm(mssql_money)]
    native_money: rust_decimal::Decimal,
    #[graphql_orm(version, default = "0")]
    #[filterable(type = "number")]
    version: i64,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmTriggeredComposites",
    plural = "TriggeredComposites",
    schema_policy = "external_writable",
    repository_mutations = true,
    unique_composite = "tenant_id,record_key",
    upsert = "tenant_id,record_key"
)]
struct TriggeredComposite {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    tenant_id: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    record_key: String,
    #[filterable(type = "string")]
    value: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.GraphqlOrmTriggeredNotes",
    plural = "TriggeredNotes",
    schema_policy = "external_writable",
    auth = "none"
)]
struct TriggeredNote {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    generated_mutations: "allowlist",
    generated_mutation_allowlist: [TriggeredNote],
    query_custom_ops: [],
    entities: [TriggeredNote],
}

type TriggeredSchema = async_graphql::Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

#[derive(Clone)]
struct AllowEntityPolicy;

impl EntityPolicy<MssqlBackend> for AllowEntityPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<MssqlBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

fn writable_database(
    connection_string: &str,
) -> Result<Database<MssqlBackend>, Box<dyn std::error::Error>> {
    let config = graphql_orm::tiberius::Config::from_ado_string(connection_string)?;
    let pool = graphql_orm::db::mssql::MssqlPool::with_max_connections_external_writable(config, 4);
    let mut database = Database::<MssqlBackend>::builder(pool)
        .schema_policy(SchemaPolicy::ExternalWritable)
        .build();
    database.set_entity_policy(AllowEntityPolicy);
    Ok(database)
}

fn record_input(external_key: &str, value: i64) -> CreateTriggeredRecordInput {
    CreateTriggeredRecordInput {
        external_key: external_key.to_owned(),
        value,
        native_money: "900719925474.0993".parse().unwrap(),
    }
}

fn composite_input(record_key: &str, value: &str) -> CreateTriggeredCompositeInput {
    CreateTriggeredCompositeInput {
        tenant_id: "tenant-a".to_owned(),
        record_key: record_key.to_owned(),
        value: value.to_owned(),
    }
}

/// One audit trigger per table. Each writes one row per inserted and deleted
/// row plus one statement marker, and leaves `NOCOUNT` off, so every fired
/// trigger reports extra row counts, including on statements that match
/// nothing.
const TRIGGERED_TABLES: &str = r#"
CREATE TABLE dbo.GraphqlOrmTriggerAudit (
    audit_id BIGINT IDENTITY(1,1) NOT NULL PRIMARY KEY,
    table_name NVARCHAR(64) NOT NULL,
    row_key NVARCHAR(160) NULL
);
CREATE TABLE dbo.GraphqlOrmTriggeredRecords (
    id NVARCHAR(64) NOT NULL PRIMARY KEY,
    external_key NVARCHAR(64) NOT NULL UNIQUE,
    value BIGINT NOT NULL,
    native_money MONEY NOT NULL,
    version BIGINT NOT NULL DEFAULT (0)
);
CREATE TABLE dbo.GraphqlOrmTriggeredComposites (
    tenant_id NVARCHAR(64) NOT NULL,
    record_key NVARCHAR(64) NOT NULL,
    value NVARCHAR(128) NOT NULL,
    CONSTRAINT PK_GraphqlOrmTriggeredComposites PRIMARY KEY (tenant_id, record_key)
);
CREATE TABLE dbo.GraphqlOrmTriggeredNotes (
    id NVARCHAR(64) NOT NULL PRIMARY KEY,
    value NVARCHAR(128) NOT NULL
);
"#;

const TRIGGERS: [&str; 3] = [
    r#"CREATE TRIGGER dbo.GraphqlOrmTriggeredRecordsAudit ON dbo.GraphqlOrmTriggeredRecords
AFTER INSERT, UPDATE, DELETE AS
BEGIN
    INSERT INTO dbo.GraphqlOrmTriggerAudit (table_name, row_key)
    SELECT 'records', id FROM inserted UNION ALL SELECT 'records', id FROM deleted;
    INSERT INTO dbo.GraphqlOrmTriggerAudit (table_name, row_key) VALUES ('records', NULL);
END"#,
    r#"CREATE TRIGGER dbo.GraphqlOrmTriggeredCompositesAudit ON dbo.GraphqlOrmTriggeredComposites
AFTER INSERT, UPDATE, DELETE AS
BEGIN
    INSERT INTO dbo.GraphqlOrmTriggerAudit (table_name, row_key)
    SELECT 'composites', record_key FROM inserted
    UNION ALL SELECT 'composites', record_key FROM deleted;
    INSERT INTO dbo.GraphqlOrmTriggerAudit (table_name, row_key) VALUES ('composites', NULL);
END"#,
    r#"CREATE TRIGGER dbo.GraphqlOrmTriggeredNotesAudit ON dbo.GraphqlOrmTriggeredNotes
AFTER INSERT, UPDATE, DELETE AS
BEGIN
    INSERT INTO dbo.GraphqlOrmTriggerAudit (table_name, row_key)
    SELECT 'notes', id FROM inserted UNION ALL SELECT 'notes', id FROM deleted;
    INSERT INTO dbo.GraphqlOrmTriggerAudit (table_name, row_key) VALUES ('notes', NULL);
END"#,
];

async fn audit_rows(
    database: &Database<MssqlBackend>,
    table_name: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let rows = database
        .pool()
        .fetch_rows(
            "SELECT CAST(COUNT(*) AS BIGINT) FROM dbo.GraphqlOrmTriggerAudit WHERE table_name = @P1",
            &[SqlValue::String(table_name.to_owned())],
        )
        .await?;
    Ok(rows[0].try_get::<i64, _>(0)?)
}

fn assert_mutation_ok(response: &async_graphql::Response, operation: &str) {
    assert!(
        response.errors.is_empty(),
        "{operation} failed: {:?}",
        response.errors
    );
    let data = response
        .data
        .clone()
        .into_json()
        .expect("GraphQL JSON data");
    assert_eq!(
        data[operation]["success"],
        serde_json::Value::Bool(true),
        "{operation} did not succeed: {data}"
    );
}

/// Owns a loopback-only disposable SQL Server container and removes it after
/// testing. This never accepts an ambient application database URL.
#[tokio::test]
#[ignore = "starts a test-owned SQL Server 2022 container"]
async fn generated_writes_succeed_on_tables_with_counting_after_triggers()
-> Result<(), Box<dyn std::error::Error>> {
    let mut server = OwnedSqlServer::start().await?;
    execute_batch(&server.connection_string, TRIGGERED_TABLES).await?;
    for trigger in TRIGGERS {
        execute_batch(&server.connection_string, trigger).await?;
    }
    let database = writable_database(&server.connection_string)?;

    // Raw execution reports the statement's own count, not the trigger's.
    let raw = <MssqlBackend as WriteBackend>::execute_with_binds(
        database.pool(),
        "INSERT INTO dbo.GraphqlOrmTriggeredNotes (id, value) VALUES (@P1, @P2)",
        &[
            SqlValue::String("raw".to_owned()),
            SqlValue::String("raw".to_owned()),
        ],
    )
    .await?;
    assert_eq!(<MssqlBackend as WriteBackend>::rows_affected(&raw), 1);
    let raw = <MssqlBackend as WriteBackend>::execute_with_binds(
        database.pool(),
        "UPDATE dbo.GraphqlOrmTriggeredNotes SET value = @P1 WHERE id = @P2",
        &[
            SqlValue::String("none".to_owned()),
            SqlValue::String("missing".to_owned()),
        ],
    )
    .await?;
    assert_eq!(<MssqlBackend as WriteBackend>::rows_affected(&raw), 0);
    assert_eq!(audit_rows(&database, "notes").await?, 3);

    // Single-row repository writes.
    let first = TriggeredRecord::insert(&database, record_input("first", 1)).await?;
    assert_eq!(first.version, 0);
    let updated = TriggeredRecord::update_by_id(
        &database,
        &first.id,
        UpdateTriggeredRecordInput {
            value: Some(2),
            ..Default::default()
        },
    )
    .await?
    .expect("inserted row should update");
    assert_eq!(updated.value, 2);

    let upserted = TriggeredRecord::upsert(&database, record_input("first", 3)).await?;
    assert_eq!(upserted.action, ChangeAction::Updated);
    assert_eq!(upserted.entity.value, 3);
    let created = TriggeredRecord::upsert(&database, record_input("second", 1)).await?;
    assert_eq!(created.action, ChangeAction::Created);

    // Compare-and-swap: OUTPUT without INTO is rejected on triggered tables.
    let version = upserted.entity.version;
    let swapped = TriggeredRecord::compare_and_swap(
        &database,
        &first.id,
        version,
        TriggeredRecordWhereInput {
            external_key: Some(StringFilter {
                eq: Some("first".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        },
        UpdateTriggeredRecordInput {
            value: Some(4),
            ..Default::default()
        },
    )
    .await?;
    let ConditionalUpdateOutcome::Updated(swapped) = swapped else {
        panic!("matching compare-and-swap should update a triggered table");
    };
    assert_eq!(swapped.value, 4);
    assert_eq!(swapped.version, version + 1);
    assert_eq!(swapped.native_money.to_string(), "900719925474.0993");
    assert!(matches!(
        TriggeredRecord::compare_and_swap(
            &database,
            &first.id,
            version,
            TriggeredRecordWhereInput::default(),
            UpdateTriggeredRecordInput {
                value: Some(5),
                ..Default::default()
            },
        )
        .await?,
        ConditionalUpdateOutcome::Conflict
    ));
    assert!(matches!(
        TriggeredRecord::compare_and_swap(
            &database,
            &"missing".to_owned(),
            0,
            TriggeredRecordWhereInput::default(),
            UpdateTriggeredRecordInput {
                value: Some(5),
                ..Default::default()
            },
        )
        .await?,
        ConditionalUpdateOutcome::NotFound
    ));
    assert_eq!(
        TriggeredRecord::find_by_id(&database, &first.id)
            .await?
            .expect("swapped row should remain")
            .value,
        4
    );

    // Bulk repository writes.
    let bulk = TriggeredRecord::insert_many(
        &database,
        [record_input("bounded-a", 10), record_input("bounded-b", 20)],
    )
    .await?;
    assert_eq!(bulk.len(), 2);
    let bounded_filter = || TriggeredRecordWhereInput {
        external_key: Some(StringFilter {
            contains: Some("bounded-".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        TriggeredRecord::update_where_bounded(
            &database,
            bounded_filter(),
            UpdateTriggeredRecordInput {
                value: Some(30),
                ..Default::default()
            },
            MutationLimit::new(2)?,
        )
        .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    assert_eq!(
        TriggeredRecord::delete_where_bounded(&database, bounded_filter(), MutationLimit::new(2)?)
            .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    let bulk_upsert = TriggeredRecord::upsert_many(
        &database,
        [record_input("bulk-a", 1), record_input("bulk-b", 2)],
    )
    .await?;
    assert!(
        bulk_upsert
            .iter()
            .all(|outcome| outcome.action == ChangeAction::Created)
    );
    let bulk_upsert = TriggeredRecord::upsert_many(
        &database,
        [record_input("bulk-a", 11), record_input("bulk-b", 12)],
    )
    .await?;
    assert!(
        bulk_upsert
            .iter()
            .all(|outcome| outcome.action == ChangeAction::Updated)
    );

    // Composite-key writes, including a conditional insert that matches nothing.
    assert!(matches!(
        TriggeredComposite::insert_if_absent(&database, composite_input("one", "first")).await?,
        InsertIfAbsentOutcome::Inserted(_)
    ));
    assert!(matches!(
        TriggeredComposite::insert_if_absent(&database, composite_input("one", "second")).await?,
        InsertIfAbsentOutcome::AlreadyPresent(_)
    ));
    let composite_key = TriggeredCompositeKey {
        tenant_id: "tenant-a".to_owned(),
        record_key: "one".to_owned(),
    };
    let composite = TriggeredComposite::update_by_key(
        &database,
        &composite_key,
        UpdateTriggeredCompositeInput {
            value: Some("updated".to_owned()),
        },
    )
    .await?
    .expect("composite key update should find the row");
    assert_eq!(composite.value, "updated");
    TriggeredComposite::insert(&database, composite_input("two", "second")).await?;
    let tenant_filter = || TriggeredCompositeWhereInput {
        tenant_id: Some(StringFilter {
            eq: Some("tenant-a".to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        TriggeredComposite::update_where_bounded(
            &database,
            tenant_filter(),
            UpdateTriggeredCompositeInput {
                value: Some("bounded".to_owned()),
            },
            MutationLimit::new(2)?,
        )
        .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );
    assert_eq!(
        TriggeredComposite::delete_where_bounded(
            &database,
            tenant_filter(),
            MutationLimit::new(2)?
        )
        .await?,
        BoundedMutationOutcome::Applied { affected: 2 }
    );

    // Writes inside an explicit transaction.
    database
        .transaction(TransactionMode::Default, |context| {
            Box::pin(async move {
                context
                    .insert::<TriggeredRecord>(record_input("transactional", 7))
                    .await?;
                Ok::<(), graphql_orm::graphql::errors::OrmPublicError>(())
            })
        })
        .await?;

    // Generated GraphQL mutations.
    let schema: TriggeredSchema = schema_builder(database.clone()).finish();
    let response = schema
        .execute(
            r#"mutation {
                createTriggeredNote(input: { id: "note-a", value: "a" }) { success }
            }"#,
        )
        .await;
    assert_mutation_ok(&response, "createTriggeredNote");
    let response = schema
        .execute(
            r#"mutation {
                updateTriggeredNote(id: "note-a", input: { value: "b" }) {
                    success
                    triggeredNote { value }
                }
            }"#,
        )
        .await;
    assert_mutation_ok(&response, "updateTriggeredNote");
    let response = schema
        .execute(
            r#"mutation {
                deleteTriggeredNote(id: "note-a") { success }
            }"#,
        )
        .await;
    assert_mutation_ok(&response, "deleteTriggeredNote");

    assert!(TriggeredRecord::delete_by_id(&database, &first.id).await?);
    assert!(
        TriggeredRecord::find_by_id(&database, &first.id)
            .await?
            .is_none()
    );
    assert!(audit_rows(&database, "records").await? > 0);
    assert!(audit_rows(&database, "composites").await? > 0);

    server.cleanup()?;
    Ok(())
}
