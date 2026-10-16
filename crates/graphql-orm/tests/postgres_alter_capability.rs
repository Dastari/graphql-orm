#![cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;

fn model() -> SchemaModel {
    SchemaModel {
        extensions: vec![],
        tables: vec![TableModel {
            entity_name: "Capability".into(),
            table_name: "capability_probe".into(),
            primary_key: "id".into(),
            primary_keys: vec!["id".into()],
            default_sort: "id ASC".into(),
            columns: ["id", "value"]
                .into_iter()
                .map(|name| ColumnModel {
                    name: name.into(),
                    sql_type: "TEXT".into(),
                    spatial: None,
                    nullable: false,
                    is_primary_key: name == "id",
                    is_unique: false,
                    default: None,
                })
                .collect(),
            indexes: vec![],
            composite_unique_indexes: vec![],
            foreign_keys: vec![],
            search_indexes: vec![],
            append_only: false,
            retention_purge: false,
            check_constraints: vec![],
        }],
    }
}
fn variants() -> Vec<SchemaModel> {
    let mut unique = model();
    unique.tables[0].columns[1].is_unique = true;
    let mut primary_flag = model();
    primary_flag.tables[0].columns[1].is_primary_key = true;
    let mut spatial = model();
    spatial.tables[0].columns[1].spatial =
        Some(SpatialColumnDef::geometry(SpatialGeometryType::Point, 4326));
    vec![unique, primary_flag, spatial]
}
fn protocol(error: sqlx::Error) {
    assert!(
        matches!(error, sqlx::Error::Protocol(ref message) if message == "unsupported PostgreSQL column metadata alteration"),
        "{error:?}"
    );
}
#[tokio::test]
async fn unsupported_metadata_fails_before_managed_io_even_with_override() -> Result<(), sqlx::Error>
{
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "postgres://unused:unused@{}/unused",
        listener.local_addr().unwrap()
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy(&url)
        .unwrap();
    let db = Database::<PostgresBackend>::new(pool);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for target in variants() {
            let preview = build_migration_plan(DatabaseBackend::Postgres, &model(), &target);
            assert!(matches!(
                preview.steps.as_slice(),
                [MigrationStep::AlterColumn { .. }]
            ));
            protocol(
                db.schema()
                    .plan_migration("unsupported", "reject", &model(), &target)
                    .unwrap_err(),
            );
            // Public low-level preview remains infallible. Even a manually assembled
            // executable plan must be rejected before opening a connection/history.
            let steps = classify_migration_steps(&preview.steps);
            let plan = PlannedMigration {
                version: "unsupported".into(),
                description: "reject".into(),
                backend: "postgres",
                source_schema_hash: None,
                target_schema_hash: target.stable_hash(),
                plan_hash: "not-an-authorization".into(),
                steps,
                statements: preview.statements,
            };
            protocol(
                db.schema()
                    .apply_migration(
                        &plan,
                        ApplyOptions {
                            allow_destructive: true,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap_err(),
            );
            let combined = PlannedSchemaTarget {
                version: plan.version.clone(),
                description: plan.description.clone(),
                backend: "postgres",
                source_schema_hash: None,
                target_schema_hash: plan.target_schema_hash.clone(),
                target_hash: "preview".into(),
                plan_hash: "preview".into(),
                migration: plan.clone(),
                rls: RlsPolicyPlan {
                    backend: "postgres",
                    target_rls_hash: "empty".into(),
                    statements: vec![],
                },
                statements: plan.statements.clone(),
            };
            protocol(
                db.schema()
                    .apply_schema_target(
                        &combined,
                        ApplyOptions {
                            allow_destructive: true,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap_err(),
            );
        }
        // A real geometry SQL type transition is not a metadata-only rejection.
        let mut geometry_before = model();
        geometry_before.tables[0].columns[1].sql_type = "geometry(Point,4326)".into();
        geometry_before.tables[0].columns[1].spatial =
            Some(SpatialColumnDef::geometry(SpatialGeometryType::Point, 4326));
        let mut geometry_after = geometry_before.clone();
        geometry_after.tables[0].columns[1].sql_type = "geometry(Polygon,4326)".into();
        geometry_after.tables[0].columns[1].spatial = Some(SpatialColumnDef::geometry(
            SpatialGeometryType::Polygon,
            4326,
        ));
        let supported = db.schema().plan_migration(
            "geometry",
            "SQL type change",
            &geometry_before,
            &geometry_after,
        )?;
        assert!(
            supported
                .statements
                .iter()
                .any(|s| s.contains("TYPE geometry(Polygon,4326)"))
        );
        Ok::<(), sqlx::Error>(())
    })
    .await
    .expect("capability rejection must complete without database I/O")?;
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    db.pool().close().await;
    Ok(())
}
#[tokio::test]
async fn owned_bare_constraint_flags_deny_but_explicit_unique_index_is_supported()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("alter-capability")?;
    let db = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    sqlx::query("CREATE TABLE capability_probe(id TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO capability_probe VALUES ('one','preserved')")
        .execute(db.pool())
        .await?;
    let managed = ManagedTableSet::new(["capability_probe".into()])?;
    for target in variants() {
        protocol(
            db.schema()
                .plan_schema_target(
                    "static-live",
                    "reject",
                    &SchemaTarget {
                        schema: target.clone(),
                        rls: RlsSchemaModel { entities: vec![] },
                    },
                )
                .await
                .unwrap_err(),
        );
        let target = OwnedSchemaModel::from(&target);
        for error in [
            db.schema()
                .plan_owned_migration(
                    "unsupported",
                    "reject",
                    &target,
                    &managed,
                    PlanOptions::strict(),
                )
                .await
                .unwrap_err(),
            db.schema()
                .validate_owned_schema(&target, &managed)
                .await
                .unwrap_err(),
        ] {
            assert!(
                matches!(error, RuntimeMigrationError::Diagnostics(ref errors) if errors.0.iter().any(|e| e.code == RuntimeMigrationDiagnosticCode::InvalidPhysicalContract)),
                "{error:?}"
            );
        }
    }
    let absent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema() AND table_name='__graphql_orm_migrations'").fetch_one(db.pool()).await?;
    assert_eq!(absent, 0);
    let constraints: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pg_constraint WHERE conrelid='capability_probe'::regclass AND contype='u'").fetch_one(db.pool()).await?;
    assert_eq!(constraints, 0);
    let value: String = sqlx::query_scalar("SELECT value FROM capability_probe WHERE id='one'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(value, "preserved");
    let mut target = model();
    target.tables[0]
        .indexes
        .push(IndexDef::new("capability_unique_value", &["value"]).unique());
    let target = OwnedSchemaModel::from(&target);
    let plan = db
        .schema()
        .plan_owned_migration(
            "explicit-index",
            "allowed",
            &target,
            &managed,
            PlanOptions::strict(),
        )
        .await?;
    assert!(
        plan.statements()
            .iter()
            .any(|s| s.contains("CREATE UNIQUE INDEX"))
    );
    db.schema()
        .apply_owned_migration(&plan, Default::default())
        .await?;
    let unique: bool = sqlx::query_scalar(
        "SELECT indisunique FROM pg_index WHERE indexrelid='capability_unique_value'::regclass",
    )
    .fetch_one(db.pool())
    .await?;
    assert!(unique);
    let target = db
        .schema()
        .plan_owned_migration(
            "verify-index",
            "replan",
            &target,
            &managed,
            PlanOptions::strict(),
        )
        .await?;
    assert!(target.steps().is_empty());
    db.pool().close().await;
    owned.cleanup()?;
    Ok(())
}

#[tokio::test]
async fn all_stages_are_admitted_before_any_target_ddl_or_history_row()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("staged-alter-capability")?;
    let db = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    sqlx::query("CREATE TABLE capability_probe(id TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO capability_probe VALUES ('one','preserved')")
        .execute(db.pool())
        .await?;
    let mut supported = model();
    let mut extra = supported.tables[0].columns[1].clone();
    extra.name = "must_not_exist".into();
    extra.nullable = true;
    supported.tables[0].columns.push(extra);
    let mut unsupported = supported.clone();
    unsupported.tables[0].columns[1].is_unique = true;
    let stages = vec![
        SchemaStage::new("stage-01", "supported first", supported),
        SchemaStage::new("stage-02", "unsupported second", unsupported),
    ];
    protocol(db.plan_schema_stages(&stages).await.unwrap_err());
    protocol(db.apply_schema_stages(&stages).await.unwrap_err());
    let abi = SchemaAbi::new(stages)?;
    protocol(
        db.schema()
            .plan_upgrade(&abi, "stage-02")
            .await
            .unwrap_err(),
    );
    protocol(
        db.schema()
            .apply_upgrade(
                &abi,
                "stage-02",
                ApplyOptions {
                    allow_destructive: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err(),
    );
    // Legacy staged planning may initialize the validated bookkeeping table,
    // but cannot record a rejected stage or execute the earlier supported DDL.
    let history: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM __graphql_orm_migrations")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(history, 0);
    let extra: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='capability_probe' AND column_name='must_not_exist'").fetch_one(db.pool()).await?;
    assert_eq!(extra, 0);
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id,value FROM capability_probe")
        .fetch_all(db.pool())
        .await?;
    assert_eq!(rows, [("one".into(), "preserved".into())]);
    db.pool().close().await;
    owned.cleanup()?;
    Ok(())
}
