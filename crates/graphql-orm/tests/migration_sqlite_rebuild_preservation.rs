#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]
use graphql_orm::{graphql::orm::*, prelude::*};

async fn setup(sql: &[&str]) -> graphql_orm::Result<Database<SqliteBackend>> {
    let db = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        graphql_orm::db::ConnectionOptions::default().max_connections(1),
    )
    .await?;
    for sql in sql {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    Ok(db)
}
async fn remove(db: &Database<SqliteBackend>, mode: u8) -> graphql_orm::Result<SchemaModel> {
    let current = introspect_sqlite_schema(db.pool()).await?;
    let mut target = current.clone();
    let table = target
        .tables
        .iter_mut()
        .find(|t| t.table_name == "probe")
        .unwrap();
    match mode {
        0 => {
            table
                .columns
                .iter_mut()
                .find(|c| c.name == "a")
                .unwrap()
                .nullable = true
        }
        1 => table.check_constraints.push(CheckConstraintModel {
            name: "host_check".into(),
            expression: "a IS NOT NULL".into(),
        }),
        _ => table.foreign_keys.push(ForeignKeyModel::single(
            "a",
            "owner",
            "id",
            false,
            DeletePolicy::Restrict,
        )),
    }
    let plan = db
        .schema()
        .plan_migration("remove", "composite only", &current, &target)?;
    assert!(!plan.steps.is_empty());
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    clean(db, &target).await?;
    Ok(target)
}
async fn clean(db: &Database<SqliteBackend>, target: &SchemaModel) -> graphql_orm::Result<()> {
    let actual = introspect_sqlite_schema(db.pool()).await?;
    assert!(
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &actual, target)
            .steps
            .is_empty()
    );
    assert!(!db.schema().validate(&actual, target)?.has_errors());
    let plan = db
        .schema()
        .plan_migration("replan", "unchanged", &actual, target)?;
    assert!(plan.steps.is_empty(), "{plan:?}");
    Ok(())
}
#[tokio::test]
async fn populated_column_rebuild_preserves_physical_contract() -> graphql_orm::Result<()> {
    for mode in 0..3 {
        for (ddl, insert) in [
            (
                "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT,g TEXT GENERATED ALWAYS AS (a || b) STORED)",
                "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','name','value')",
            ),
            (
                "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT COLLATE NOCASE UNIQUE)",
                "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','name','value')",
            ),
            (
                "CREATE TABLE probe(id INTEGER PRIMARY KEY AUTOINCREMENT,a TEXT NOT NULL,b TEXT)",
                "INSERT INTO probe VALUES (99,'name','value'),(999,'deleted','value')",
            ),
            (
                "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT REFERENCES parent(id) ON UPDATE CASCADE ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED)",
                "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','name','value')",
            ),
        ] {
            let db = setup(&[
                "CREATE TABLE owner(id TEXT PRIMARY KEY NOT NULL)",
                "INSERT INTO owner VALUES ('name'),('next'),('updated')",
                "CREATE TABLE parent(id TEXT PRIMARY KEY NOT NULL)",
                "INSERT INTO parent VALUES ('value')",
                ddl,
                insert,
            ])
            .await?;
            sqlx::query("DELETE FROM probe WHERE id=999")
                .execute(db.pool())
                .await?;
            let id_type = if ddl.contains("AUTOINCREMENT") {
                "INTEGER"
            } else {
                "TEXT"
            };
            sqlx::query(&format!("CREATE TABLE child(id TEXT PRIMARY KEY NOT NULL,p {id_type} REFERENCES probe(id) ON UPDATE CASCADE ON DELETE CASCADE)")).execute(db.pool()).await?;
            sqlx::query("INSERT INTO child SELECT 'child',id FROM probe")
                .execute(db.pool())
                .await?;
            sqlx::query("CREATE INDEX host_idx ON probe(a)")
                .execute(db.pool())
                .await?;
            sqlx::query("CREATE TABLE audit(v TEXT)")
                .execute(db.pool())
                .await?;
            sqlx::query("CREATE TRIGGER host_trigger AFTER UPDATE ON probe BEGIN INSERT INTO audit VALUES (new.a); END").execute(db.pool()).await?;
            let target = remove(&db, mode).await?;
            let row: (i64, String) = sqlx::query_as("SELECT rowid,a FROM probe")
                .fetch_one(db.pool())
                .await?;
            assert_eq!(row, (99, "name".into()));
            let children: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM child JOIN probe ON child.p=probe.id")
                    .fetch_one(db.pool())
                    .await?;
            assert_eq!(children, 1);
            let live: String =
                sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='probe'")
                    .fetch_one(db.pool())
                    .await?;
            if ddl.contains("GENERATED") {
                assert!(live.contains("GENERATED ALWAYS AS (a || b) STORED"));
                let g: String = sqlx::query_scalar("SELECT g FROM probe")
                    .fetch_one(db.pool())
                    .await?;
                assert_eq!(g, "namevalue");
            }
            if ddl.contains("NOCASE") {
                assert!(
                    sqlx::query("INSERT INTO probe(id,a,b) VALUES ('two','name','VALUE')")
                        .execute(db.pool())
                        .await
                        .is_err()
                );
            }
            if ddl.contains("AUTOINCREMENT") {
                let seq: i64 =
                    sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='probe'")
                        .fetch_one(db.pool())
                        .await?;
                assert_eq!(seq, 999);
                sqlx::query("INSERT INTO probe(a,b) VALUES ('next','value')")
                    .execute(db.pool())
                    .await?;
                let id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM probe")
                    .fetch_one(db.pool())
                    .await?;
                assert_eq!(id, 1000);
            }
            if ddl.contains("CASCADE") {
                assert!(live.contains("ON UPDATE CASCADE"));
                sqlx::query("UPDATE parent SET id='changed'")
                    .execute(db.pool())
                    .await?;
                let b: String = sqlx::query_scalar("SELECT b FROM probe")
                    .fetch_one(db.pool())
                    .await?;
                assert_eq!(b, "changed");
            }
            sqlx::query("UPDATE probe SET a='updated'")
                .execute(db.pool())
                .await?;
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit")
                .fetch_one(db.pool())
                .await?;
            assert!(count > 0);
            clean(&db, &target).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn unsupported_shapes_refuse_without_touching_schema_or_rows() -> graphql_orm::Result<()> {
    for ddl in [
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b)) WITHOUT ROWID",
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,rowid TEXT,_rowid_ TEXT,oid TEXT,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))",
    ] {
        let db = setup(&[ddl]).await?;
        sqlx::query("INSERT INTO probe(id,a,b) VALUES ('one','a','b')")
            .execute(db.pool())
            .await?;
        let current = introspect_sqlite_schema(db.pool()).await?;
        let mut target = current.clone();
        target.tables[0]
            .columns
            .iter_mut()
            .find(|c| c.name == "a")
            .unwrap()
            .nullable = true;
        let plan = db
            .schema()
            .plan_migration("refuse", "unsafe", &current, &target)?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let error = db
            .schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    record_history: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("UnsupportedCapability"),
            "{error}"
        );
        assert!(!error.to_string().contains("probe"));
        // Refusal precedes even migration-history preparation.
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(after, before);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}

#[tokio::test]
async fn virtual_and_shadow_tables_refuse_before_mutation() -> graphql_orm::Result<()> {
    for table in ["probe", "probe_data"] {
        let db = setup(&[
            "CREATE VIRTUAL TABLE probe USING fts5(id,a,b)",
            "INSERT INTO probe VALUES ('one','a','b')",
        ])
        .await?;
        // Model-only planning cannot identify a virtual/shadow source. Apply
        // must inspect the actual table type before executing any rebuild SQL.
        let regular = setup(&[
            "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL)",
        ])
        .await?;
        let mut current = introspect_sqlite_schema(regular.pool()).await?;
        current.tables[0].table_name = table.into();
        current.tables[0].composite_unique_indexes.clear();
        let mut target = current.clone();
        target.tables[0]
            .columns
            .iter_mut()
            .find(|c| c.name == "a")
            .unwrap()
            .nullable = true;
        let plan = db
            .schema()
            .plan_migration("refuse", "virtual", &current, &target)?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let error = db
            .schema()
            .apply_migration(
                &plan,
                ApplyOptions {
                    record_history: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("UnsupportedCapability"));
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(before, after);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}
#[tokio::test]
async fn case_variant_temp_rebuild_names_cannot_shadow_main_data() -> graphql_orm::Result<()> {
    for temporary in ["__GRAPHQL_ORM_PROBE_NEW", "probe", "PrObE"] {
        let ddl = format!(
            "CREATE TEMP TABLE {temporary}(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL)"
        );
        let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))", "INSERT INTO probe(rowid,id,a,b) VALUES (99,'one','a','b')", &ddl]).await?;
        let current = introspect_sqlite_schema(db.pool()).await?;
        assert_eq!(
            current.tables[0].composite_unique_indexes,
            vec![vec!["a", "b"]],
            "introspection must read main, not TEMP"
        );
        let mut target = current.clone();
        target.tables[0]
            .columns
            .iter_mut()
            .find(|c| c.name == "a")
            .unwrap()
            .nullable = true;
        let plan = db
            .schema()
            .plan_migration("refuse", "temp case shadow", &current, &target)?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM main.sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let error = db
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("UnsupportedCapability"));
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM main.sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(before, after);
        let value: (i64, String) = sqlx::query_as("SELECT rowid,id FROM main.probe")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(value, (99, "one".into()));
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM temp.{temporary}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 0);
    }
    Ok(())
}

#[tokio::test]
async fn diagnostic_preparation_preserves_host_tables_and_live_rebuild_refuses_collision()
-> graphql_orm::Result<()> {
    for reserved in ["__graphql_orm_probe_new", "__GRAPHQL_ORM_PROBE_NEW"] {
        let db = setup(&[
            "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))",
            "INSERT INTO probe VALUES ('one','a','b')",
            "CREATE TABLE abgraphqlXorm_probe_new(host_data TEXT)",
            "INSERT INTO abgraphqlXorm_probe_new VALUES ('lookalike')",
        ]).await?;
        sqlx::query(&format!("CREATE TABLE {reserved}(host_data TEXT)"))
            .execute(db.pool())
            .await?;
        sqlx::query(&format!(
            "INSERT INTO {reserved} VALUES ('unproven host data')"
        ))
        .execute(db.pool())
        .await?;
        // #417 preparation diagnoses literal candidates and never deletes them.
        db.apply_migrations(&[]).await?;
        let error = remove(&db, 0).await.unwrap_err();
        assert!(
            error.to_string().contains("UnsupportedCapability"),
            "{error}"
        );
        let host: String = sqlx::query_scalar(&format!("SELECT host_data FROM {reserved}"))
            .fetch_one(db.pool())
            .await?;
        assert_eq!(host, "unproven host data");
        let lookalike: String = sqlx::query_scalar("SELECT host_data FROM abgraphqlXorm_probe_new")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(lookalike, "lookalike");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM probe WHERE id='one'")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(count, 1);
    }
    Ok(())
}
#[tokio::test]
async fn migration_restores_an_initially_disabled_fk_pragma() -> graphql_orm::Result<()> {
    let db=setup(&["CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL,b TEXT NOT NULL,UNIQUE(a,b))","INSERT INTO probe VALUES ('one','a','b')", "PRAGMA foreign_keys=OFF"]).await?;
    let target = remove(&db, 0).await?;
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(enabled, 0);
    clean(&db, &target).await
}

#[tokio::test]
async fn dropping_a_declared_rowid_alias_preserves_the_physical_rowid() -> graphql_orm::Result<()> {
    let db = setup(&[
        "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,rowid TEXT,a TEXT NOT NULL)",
        "INSERT INTO probe(_rowid_,id,rowid,a) VALUES (99,'one','7','kept')",
    ])
    .await?;
    let current = introspect_sqlite_schema(db.pool()).await?;
    let mut target = current.clone();
    target.tables[0].columns.retain(|c| c.name != "rowid");
    let plan = db
        .schema()
        .plan_migration("drop-alias", "preserve identity", &current, &target)?;
    db.schema()
        .apply_migration(
            &plan,
            ApplyOptions {
                allow_destructive: true,
                ..Default::default()
            },
        )
        .await?;
    let rowid: i64 = sqlx::query_scalar("SELECT rowid FROM probe")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(rowid, 99);
    clean(&db, &target).await
}

#[tokio::test]
async fn storage_class_changes_abort_without_replacing_the_original() -> graphql_orm::Result<()> {
    for enabled in [0, 1] {
        let db = setup(&[
            "CREATE TABLE probe(id TEXT PRIMARY KEY NOT NULL,a TEXT NOT NULL)",
            "INSERT INTO probe(rowid,id,a) VALUES (99,'one','001')",
        ])
        .await?;
        sqlx::query(&format!("PRAGMA foreign_keys={enabled}"))
            .execute(db.pool())
            .await?;
        let current = introspect_sqlite_schema(db.pool()).await?;
        let mut target = current.clone();
        target.tables[0]
            .columns
            .iter_mut()
            .find(|c| c.name == "a")
            .unwrap()
            .sql_type = "INTEGER".into();
        let plan =
            db.schema()
                .plan_migration("conversion", "refuse changed values", &current, &target)?;
        let error = db
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("UnsupportedCapability"));
        assert!(!error.to_string().contains("001"));
        let row: (i64, String, String) = sqlx::query_as("SELECT rowid,a,typeof(a) FROM probe")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(row, (99, "001".into(), "text".into()));
        let state: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.pool())
            .await?;
        assert_eq!(state, enabled);
        let leftovers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='__graphql_orm_probe_new'",
        )
        .fetch_one(db.pool())
        .await?;
        assert_eq!(leftovers, 0);
        clean(&db, &current).await?;
        remove(&db, 0).await?;
    }
    Ok(())
}

#[derive(
    GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq,
)]
#[graphql_entity(
    table = "graphql_orm_ai_provider_webhook_receipts",
    plural = "LegacyGraphqlOrmAiProviderWebhookReceipts",
    default_sort = "received_at DESC"
)]
struct LegacyWebhookReceiptRecord {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    provider_kind: String,
    #[filterable(type = "string")]
    provider_event_id: String,
    provider_response_id: Option<String>,
    run_id: Option<graphql_orm::uuid::Uuid>,
    attempt_id: Option<graphql_orm::uuid::Uuid>,
    signature_verified: bool,
    state: String,
    safe_error_code: Option<String>,
    #[sortable]
    received_at: i64,
    processed_at: Option<i64>,
    #[graphql_orm(version, default = "0")]
    row_version: i64,
}

#[tokio::test]
async fn legacy_webhook_primary_key_upgrade_requires_live_empty_evidence() -> graphql_orm::Result<()>
{
    for populated in [false, true] {
        let db = setup(&[]).await?;
        let legacy = SchemaModel::from_entities(&[LegacyWebhookReceiptRecord::metadata()]);
        let create = build_migration_plan(
            DatabaseBackend::Sqlite,
            &SchemaModel {
                extensions: vec![],
                tables: vec![],
            },
            &legacy,
        );
        for sql in create.statements {
            sqlx::query(&sql).execute(db.pool()).await?;
        }
        let ddl: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE name='graphql_orm_ai_provider_webhook_receipts'",
        )
        .fetch_one(db.pool())
        .await?;
        assert!(ddl.contains("id TEXT NOT NULL PRIMARY KEY"), "{ddl}");
        if populated {
            sqlx::query("INSERT INTO graphql_orm_ai_provider_webhook_receipts(rowid,id,provider_kind,provider_event_id,signature_verified,state,received_at) VALUES (99,'one','openai','event',1,'received',0)").execute(db.pool()).await?;
        }
        let current = introspect_sqlite_schema(db.pool()).await?;
        let mut target = current.clone();
        let table = &mut target.tables[0];
        table.primary_keys = vec!["id".into(), "provider_kind".into()];
        table
            .columns
            .iter_mut()
            .find(|c| c.name == "provider_kind")
            .unwrap()
            .is_primary_key = true;
        for (name, template) in [
            ("receipt_key", "provider_event_id"),
            ("provider_profile_id", "provider_event_id"),
            ("provider_event_kind", "provider_event_id"),
            ("provider_created_at", "received_at"),
        ] {
            let mut column = table
                .columns
                .iter()
                .find(|c| c.name == template)
                .unwrap()
                .clone();
            column.name = name.into();
            table.columns.push(column);
        }
        table.indexes.push(IndexDef {
            name: "receipt_lookup",
            columns: &[
                "provider_kind",
                "provider_profile_id",
                "provider_response_id",
                "state",
            ],
            column_directions: &[],
            is_unique: false,
            method: IndexMethod::Default,
            is_spatial: false,
            predicate: None,
        });
        let plan = db.schema().plan_migration(
            "upgrade-placeholder",
            "composite receipt identity",
            &current,
            &target,
        )?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        let result = db
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await;
        if populated {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("UnsupportedCapability"));
            let after: Vec<(String, String)> = sqlx::query_as(
                "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
            )
            .fetch_all(db.pool())
            .await?;
            assert_eq!(before, after);
            let row: (i64, String) =
                sqlx::query_as("SELECT rowid,id FROM graphql_orm_ai_provider_webhook_receipts")
                    .fetch_one(db.pool())
                    .await?;
            assert_eq!(row, (99, "one".into()));
            clean(&db, &current).await?;
        } else {
            result?;
            clean(&db, &target).await?;
            for (rowid, kind) in [(99, "openai"), (101, "other")] {
                sqlx::query("INSERT INTO graphql_orm_ai_provider_webhook_receipts(rowid,id,provider_kind,provider_event_id,signature_verified,state,received_at,receipt_key,provider_profile_id,provider_event_kind,provider_created_at) VALUES (?,'one',?,'event',1,'received',0,'receipt','profile','completed',0)").bind(rowid).bind(kind).execute(db.pool()).await?;
            }
            assert!(sqlx::query("UPDATE graphql_orm_ai_provider_webhook_receipts SET provider_kind='openai' WHERE rowid=101").execute(db.pool()).await.is_err());
            clean(&db, &target).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn empty_primary_changes_still_refuse_incoming_fks_and_sequence_history()
-> graphql_orm::Result<()> {
    for ddl in [
        "CREATE TABLE probe(id TEXT NOT NULL PRIMARY KEY,a TEXT NOT NULL)",
        "CREATE TABLE probe(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,a TEXT NOT NULL)",
        "CREATE TABLE probe(id TEXT COLLATE NOCASE NOT NULL PRIMARY KEY,a TEXT NOT NULL)",
    ] {
        let db = setup(&[ddl]).await?;
        if ddl.contains("AUTOINCREMENT") {
            sqlx::query("INSERT INTO probe VALUES (999,'kept')")
                .execute(db.pool())
                .await?;
            sqlx::query("DELETE FROM probe").execute(db.pool()).await?;
        } else if !ddl.contains("COLLATE") {
            sqlx::query("CREATE TABLE child(p TEXT REFERENCES probe(id))")
                .execute(db.pool())
                .await?;
        }
        let current = introspect_sqlite_schema(db.pool()).await?;
        let mut target = current.clone();
        let table = target
            .tables
            .iter_mut()
            .find(|t| t.table_name == "probe")
            .unwrap();
        table.primary_keys = vec!["id".into(), "a".into()];
        table
            .columns
            .iter_mut()
            .find(|c| c.name == "a")
            .unwrap()
            .is_primary_key = true;
        let plan = db.schema().plan_migration(
            "refuse-primary",
            "keep physical contract",
            &current,
            &target,
        )?;
        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert!(
            db.schema()
                .apply_migration(&plan, ApplyOptions::default())
                .await
                .unwrap_err()
                .to_string()
                .contains("UnsupportedCapability")
        );
        let after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(db.pool())
        .await?;
        assert_eq!(before, after);
        if ddl.contains("AUTOINCREMENT") {
            let seq: i64 = sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='probe'")
                .fetch_one(db.pool())
                .await?;
            assert_eq!(seq, 999);
        }
        clean(&db, &current).await?;
    }
    Ok(())
}
