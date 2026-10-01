#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::GraphQLSchemaEntity;
use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(
        backend = "sqlite",
        table = "owned_notes",
        plural = "Notes",
        default_sort = "id ASC"
    )
)]
#[cfg_attr(
    all(feature = "postgres", not(feature = "sqlite")),
    graphql_entity(
        backend = "postgres",
        table = "owned_notes",
        plural = "Notes",
        default_sort = "id ASC"
    )
)]
struct Note {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "string")]
    label: String,
    #[graphql_orm(default = "7")]
    score: i64,
    optional: Option<String>,
}

fn schema() -> ValidatedRuntimeSchema {
    RuntimeSchema::from_static_entities(&[Note::metadata()])
        .unwrap()
        .validate()
        .unwrap()
}
fn target<B: RuntimeMigrationBackend>() -> OwnedSchemaModel {
    schema()
        .physical_schema::<B>(RuntimeMigrationLimits::default())
        .unwrap()
}
fn ownership() -> ManagedTableSet {
    ManagedTableSet::new(["owned_notes".into()]).unwrap()
}
fn rejected(error: RuntimeMigrationError, code: RuntimeMigrationDiagnosticCode) {
    assert!(
        matches!(error, RuntimeMigrationError::Diagnostics(ref d) if d.0.iter().any(|d| d.code == code)),
        "{error:?}"
    );
}

#[test]
fn canonical_owned_storage_matches_static_target_and_legacy_hash() {
    let static_target = SchemaModel::from_entities(&[Note::metadata()]);
    let owned = OwnedSchemaModel::from(&static_target);
    assert_eq!(owned.stable_hash(), static_target.stable_hash());
    #[cfg(feature = "sqlite")]
    assert_eq!(
        static_target.stable_hash(),
        "e7b75c205412e474",
        "legacy 0.33.3 physical hash format"
    );
    #[cfg(feature = "sqlite")]
    assert_eq!(
        target::<SqliteBackend>().stable_hash(),
        static_target.stable_hash()
    );
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    assert_eq!(
        target::<PostgresBackend>().stable_hash(),
        static_target.stable_hash()
    );
    assert!(OwnedSchemaModel::default().tables().is_empty());
}

#[test]
fn unsupported_generation_and_conversion_limits_are_structured() {
    let mut definition = schema().schema().clone();
    definition.collections[0].fields[0].generated = true;
    let schema = definition.validate().unwrap();
    #[cfg(feature = "sqlite")]
    type B = SqliteBackend;
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    type B = PostgresBackend;
    let error = schema
        .physical_schema::<B>(RuntimeMigrationLimits::default())
        .unwrap_err();
    assert_eq!(
        error.0[0].code,
        RuntimeMigrationDiagnosticCode::UnsupportedGeneration
    );
    let error = target::<B>()
        .with_static_entities(&[Note::metadata()])
        .unwrap_err();
    assert_eq!(
        error.0[0].code,
        RuntimeMigrationDiagnosticCode::TargetCollision
    );
    let error = self::schema()
        .physical_schema::<B>(RuntimeMigrationLimits {
            max_target_bytes: 1,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(
        error.0[0].code,
        RuntimeMigrationDiagnosticCode::LimitExceeded
    );
    assert!(ManagedTableSet::new(["__graphql_orm_migrations".into()]).is_err());
    assert!(ManagedTableSet::new(["owned_notes".into(), "owned_notes".into()]).is_err());
}

#[cfg(feature = "sqlite")]
async fn sqlite() -> Database<SqliteBackend> {
    Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap()
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_read_only_plan_shared_apply_noop_and_public_rename()
-> Result<(), Box<dyn std::error::Error>> {
    let db = sqlite().await;
    let target = target::<SqliteBackend>();
    let ownership = ownership();
    let manager = db.schema();
    let plan = manager
        .plan_owned_migration(
            "v1",
            "owned notes",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    let static_plan = manager
        .plan_migration_to_entities("static", "same", &[Note::metadata()])
        .await?;
    assert_eq!(plan.statements(), static_plan.statements);
    assert_eq!(
        static_plan.plan_hash, "20e6f11d31144511",
        "legacy 0.33.3 plan hash format"
    );
    assert_eq!(
        plan.steps().iter().map(|s| s.risk()).collect::<Vec<_>>(),
        static_plan.steps.iter().map(|s| s.risk).collect::<Vec<_>>()
    );
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(
        before, 0,
        "planning must not create application or history tables"
    );
    manager
        .apply_owned_migration(
            &plan,
            ApplyOptions {
                additive_only: true,
                ..Default::default()
            },
        )
        .await?;
    sqlx::query("INSERT INTO owned_notes(id,label) VALUES ('a','kept')")
        .execute(db.pool())
        .await?;
    let stored: (i64, Option<String>) =
        sqlx::query_as("SELECT score, optional FROM owned_notes WHERE id = 'a'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(stored, (7, None));
    let noop = manager
        .plan_owned_migration("v1", "replan", &target, &ownership, PlanOptions::strict())
        .await?;
    assert!(noop.steps().is_empty());
    assert!(noop.statements().is_empty());
    assert!(
        manager
            .apply_owned_migration(&noop, Default::default())
            .await?
            .already_applied
    );
    let static_noop = manager
        .plan_migration_to_entities("static", "same", &[Note::metadata()])
        .await?;
    assert!(
        static_noop.steps.is_empty(),
        "owned-to-static replan: {static_noop:?}"
    );
    let mut renamed = schema().schema().clone();
    renamed.collections[0].api_type_name = "RenamedNote".into();
    renamed.collections[0].api_plural_name = "RenamedNotes".into();
    renamed.collections[0].fields[1].api_name = "renamedLabel".into();
    let renamed = renamed
        .validate()?
        .physical_schema::<SqliteBackend>(Default::default())?;
    let renamed_plan = manager
        .plan_owned_migration(
            "rename",
            "public only",
            &renamed,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(renamed_plan.steps().is_empty());
    assert!(renamed_plan.statements().is_empty());
    let value: String = sqlx::query_scalar("SELECT label FROM owned_notes WHERE id='a'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(value, "kept");
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_guards_baseline_removed_ownership_and_rollback()
-> Result<(), Box<dyn std::error::Error>> {
    let db = sqlite().await;
    let target = target::<SqliteBackend>();
    let ownership = ownership();
    let manager = db.schema();
    let initial = manager
        .plan_owned_migration("v1", "create", &target, &ownership, PlanOptions::strict())
        .await?;
    let wrong = ManagedTableSet::new(["different_table".into()])?;
    rejected(
        manager
            .plan_owned_migration("v1", "wrong", &target, &wrong, PlanOptions::strict())
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::OwnershipMismatch,
    );
    manager
        .apply_owned_migration(&initial, Default::default())
        .await?;
    sqlx::query("CREATE TABLE external_data(id TEXT PRIMARY KEY NOT NULL)")
        .execute(db.pool())
        .await?;
    let noop = manager
        .plan_owned_migration("noop", "scope", &target, &ownership, PlanOptions::strict())
        .await?;
    assert!(noop.steps().is_empty());
    sqlx::query("ALTER TABLE external_data ADD COLUMN extra TEXT")
        .execute(db.pool())
        .await?;
    rejected(
        manager
            .apply_owned_migration(&noop, Default::default())
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::BaselineMismatch,
    );
    let removed = manager
        .plan_owned_migration(
            "drop",
            "removed but still owned",
            &OwnedSchemaModel::default(),
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert_eq!(removed.steps().len(), 1);
    assert_eq!(removed.steps()[0].risk(), MigrationRisk::Destructive);
    assert!(
        manager
            .apply_owned_migration(&removed, Default::default())
            .await
            .is_err()
    );
    let ignored = manager
        .plan_owned_migration(
            "ignore",
            "legacy option",
            &OwnedSchemaModel::default(),
            &ownership,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(ignored.steps().is_empty());
    sqlx::query("INSERT INTO owned_notes(id,label) VALUES ('a','kept')")
        .execute(db.pool())
        .await?;
    let mut definition = schema().schema().clone();
    definition.collections[0].fields.push(RuntimeField {
        id: FieldId::new("required")?,
        api_name: "required".into(),
        physical_column: "required".into(),
        value_kind: RuntimeValueKind::String,
        nullable: false,
        unique: false,
        filterable: false,
        sortable: false,
        generated: false,
        default: None,
    });
    let upgrade = definition
        .validate()?
        .physical_schema::<SqliteBackend>(Default::default())?;
    let plan = manager
        .plan_owned_migration(
            "fail",
            "cannot fill required column",
            &upgrade,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(
        manager
            .apply_owned_migration(&plan, Default::default())
            .await
            .is_err()
    );
    let row: String = sqlx::query_scalar("SELECT label FROM owned_notes WHERE id='a'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, "kept");
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='external_data'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(exists, 1);
    let drop = manager
        .plan_owned_migration(
            "drop",
            "review",
            &OwnedSchemaModel::default(),
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(
            &drop,
            ApplyOptions {
                allow_destructive: true,
                ..Default::default()
            },
        )
        .await?;
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='external_data'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(exists, 1);
    Ok(())
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
async fn postgres_owned_static_noop_baseline_and_host_rls_preservation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("runtime-migrations")?;
    let db = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    let target = target::<PostgresBackend>();
    let ownership = ownership();
    let manager = db.schema();
    let initial = manager
        .plan_owned_migration("v1", "create", &target, &ownership, PlanOptions::strict())
        .await?;
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=current_schema()",
    )
    .fetch_one(db.pool())
    .await?;
    assert_eq!(tables, 0);
    let static_plan = manager
        .plan_migration_to_entities("static", "same", &[Note::metadata()])
        .await?;
    assert_eq!(initial.statements(), static_plan.statements);
    manager
        .apply_owned_migration(&initial, Default::default())
        .await?;
    let no_op = manager
        .plan_owned_migration("v1", "replan", &target, &ownership, PlanOptions::strict())
        .await?;
    assert!(no_op.steps().is_empty(), "{no_op:?}");
    assert!(
        manager
            .apply_owned_migration(&no_op, Default::default())
            .await?
            .already_applied
    );
    assert!(
        manager
            .plan_migration_to_entities("static", "same", &[Note::metadata()])
            .await?
            .steps
            .is_empty()
    );
    for sql in [
        "CREATE ROLE owned_reader NOLOGIN",
        "GRANT SELECT ON owned_notes TO owned_reader",
        "ALTER TABLE owned_notes ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE owned_notes FORCE ROW LEVEL SECURITY",
        "CREATE POLICY host_policy ON owned_notes FOR SELECT TO owned_reader USING (id='allowed')",
        "CREATE TABLE system_journal(id TEXT PRIMARY KEY NOT NULL, note_id TEXT REFERENCES owned_notes(id) ON DELETE RESTRICT)",
        "ALTER TABLE system_journal ENABLE ROW LEVEL SECURITY",
        "ALTER TABLE system_journal FORCE ROW LEVEL SECURITY",
        "CREATE POLICY journal_policy ON system_journal USING (false)",
        "CREATE FUNCTION host_helper() RETURNS integer LANGUAGE SQL AS 'SELECT 42'",
        "INSERT INTO owned_notes(id,label) VALUES ('allowed','public'),('denied','secret')",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
    }
    let snapshot = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (String, bool, bool, String, Option<String>)>("SELECT c.relname,c.relrowsecurity,c.relforcerowsecurity,pg_get_userbyid(c.relowner)::text,c.relacl::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relkind='r' AND c.relname IN ('owned_notes','system_journal') ORDER BY c.relname").fetch_all(&pool).await
    };
    let before = snapshot(db.pool().clone()).await?;
    let policies_before: Vec<(String,String,String)> = sqlx::query_as("SELECT tablename,policyname,qual FROM pg_policies WHERE schemaname=current_schema() ORDER BY tablename,policyname").fetch_all(db.pool()).await?;
    let mut definition = schema().schema().clone();
    let score = definition.collections[0].fields[2].id.clone();
    definition.collections[0].indexes.push(RuntimeIndex {
        id: IndexId::new("score_index")?,
        name: "idx_owned_score".into(),
        fields: vec![score],
        unique: false,
    });
    let upgraded_schema = definition.validate()?;
    let upgrade = upgraded_schema.physical_schema::<PostgresBackend>(Default::default())?;
    let plan = manager
        .plan_owned_migration(
            "v2",
            "new index preserves host RLS",
            &upgrade,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(
            &plan,
            ApplyOptions {
                additive_only: true,
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(before, snapshot(db.pool().clone()).await?);
    let policies_after: Vec<(String,String,String)> = sqlx::query_as("SELECT tablename,policyname,qual FROM pg_policies WHERE schemaname=current_schema() ORDER BY tablename,policyname").fetch_all(db.pool()).await?;
    assert_eq!(policies_before, policies_after);
    let helper: i32 = sqlx::query_scalar("SELECT host_helper()")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(helper, 42);
    let mut tx = db.pool().begin().await?;
    sqlx::query("SET LOCAL ROLE owned_reader")
        .execute(&mut *tx)
        .await?;
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM owned_notes ORDER BY id")
        .fetch_all(&mut *tx)
        .await?;
    assert_eq!(ids, ["allowed"]);
    tx.rollback().await?;
    let certificate = manager
        .runtime_mutation_environment(std::sync::Arc::new(upgraded_schema), &upgrade, &ownership)
        .await?;
    assert_eq!(
        certificate.incoming_dependencies("owned_notes")[0].source_table,
        "system_journal"
    );
    let noop = manager
        .plan_owned_migration(
            "noop",
            "baseline",
            &upgrade,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    sqlx::query("ALTER TABLE system_journal ADD COLUMN extra TEXT")
        .execute(db.pool())
        .await?;
    rejected(
        manager
            .apply_owned_migration(&noop, Default::default())
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::BaselineMismatch,
    );
    db.pool().close().await;
    owned.cleanup()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_reverse_origin_plan_limits_and_rebuild_preservation_guards()
-> Result<(), Box<dyn std::error::Error>> {
    let db = sqlite().await;
    let manager = db.schema();
    let static_plan = manager
        .plan_migration_to_entities("static", "static origin", &[Note::metadata()])
        .await?;
    manager
        .apply_migration(&static_plan, Default::default())
        .await?;
    let owned = target::<SqliteBackend>();
    let ownership = ownership();
    let noop = manager
        .plan_owned_migration(
            "owned",
            "owned origin",
            &owned,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(noop.steps().is_empty());
    rejected(
        manager
            .apply_owned_migration(
                &noop,
                ApplyOptions {
                    expected_current_schema_hash: Some("wrong".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::BaselineMismatch,
    );
    let removed = manager
        .plan_owned_migration(
            "drop",
            "guard",
            &OwnedSchemaModel::default(),
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(
        manager
            .apply_owned_migration(
                &removed,
                ApplyOptions {
                    allow_destructive: true,
                    additive_only: true,
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let empty_db = sqlite().await;
    let bounded = schema().physical_schema::<SqliteBackend>(RuntimeMigrationLimits {
        max_plan_steps: 0,
        ..Default::default()
    })?;
    rejected(
        empty_db
            .schema()
            .plan_owned_migration(
                "bounded",
                "limits retained",
                &bounded,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::LimitExceeded,
    );
    let mut definition = schema().schema().clone();
    definition.collections[0].fields[1].nullable = true; // controlled SQLite rebuild
    let nullable = definition
        .validate()?
        .physical_schema::<SqliteBackend>(Default::default())?;
    sqlx::query("CREATE TRIGGER host_trigger AFTER INSERT ON owned_notes BEGIN SELECT 1; END")
        .execute(db.pool())
        .await?;
    rejected(
        manager
            .plan_owned_migration(
                "rebuild",
                "preserve host trigger",
                &nullable,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::InvalidPhysicalContract,
    );
    sqlx::query("DROP TRIGGER host_trigger")
        .execute(db.pool())
        .await?;
    sqlx::query("CREATE TABLE __graphql_orm_owned_notes_new(id TEXT)")
        .execute(db.pool())
        .await?;
    rejected(
        manager
            .plan_owned_migration(
                "rebuild",
                "temporary name collision",
                &nullable,
                &ownership,
                PlanOptions::strict(),
            )
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::OwnershipMismatch,
    );
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name='__graphql_orm_owned_notes_new'",
    )
    .fetch_one(db.pool())
    .await?;
    assert_eq!(exists, 1, "owned planning must not run global cleanup");
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_composite_keys_all_runtime_types_and_timestamp_defaults()
-> Result<(), Box<dyn std::error::Error>> {
    let db = sqlite().await;
    let mut definition = schema().schema().clone();
    let collection = &mut definition.collections[0];
    collection.primary_key = vec![
        collection.fields[1].id.clone(),
        collection.fields[0].id.clone(),
    ];
    collection.indexes.clear();
    collection.fields[1].filterable = false;
    for (id, kind, nullable, generated, default) in [
        (
            "boolean",
            RuntimeValueKind::Boolean,
            false,
            false,
            Some(RuntimeDefault::Literal("true".into())),
        ),
        ("float", RuntimeValueKind::Float, true, false, None),
        ("uuid", RuntimeValueKind::Uuid, false, true, None),
        ("json", RuntimeValueKind::Json, true, false, None),
        ("bytes", RuntimeValueKind::Bytes, true, false, None),
        (
            "datetime",
            RuntimeValueKind::DateTime,
            false,
            true,
            Some(RuntimeDefault::CurrentTimestamp),
        ),
        (
            "epoch",
            RuntimeValueKind::Integer,
            false,
            true,
            Some(RuntimeDefault::CurrentTimestamp),
        ),
    ] {
        collection.fields.push(RuntimeField {
            id: FieldId::new(id)?,
            api_name: id.into(),
            physical_column: id.into(),
            value_kind: kind,
            nullable,
            unique: false,
            filterable: false,
            sortable: false,
            generated,
            default,
        });
    }
    let target = definition
        .validate()?
        .physical_schema::<SqliteBackend>(Default::default())?;
    let ownership = ownership();
    assert_eq!(target.tables()[0].primary_keys(), ["label", "id"]);
    let manager = db.schema();
    let plan = manager
        .plan_owned_migration(
            "types",
            "types and ordered composite key",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(&plan, Default::default())
        .await?;
    sqlx::query("INSERT INTO owned_notes(id,label,uuid) VALUES ('id','label',?)")
        .bind(graphql_orm::uuid::Uuid::new_v4().to_string())
        .execute(db.pool())
        .await?;
    let row: (String, i64, i64) =
        sqlx::query_as("SELECT datetime, epoch, boolean FROM owned_notes")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(row.0.len(), 27);
    assert!(row.0.ends_with('Z'));
    assert_eq!(RuntimeDateTime::parse(&row.0)?.as_str(), row.0);
    assert!(row.1 > 0);
    assert_eq!(row.2, 1);
    let noop = manager
        .plan_owned_migration(
            "noop",
            "reverse key order retained",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(noop.steps().is_empty(), "{noop:?}");
    Ok(())
}

#[cfg(feature = "sqlite")]
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "host_journal",
    plural = "HostJournal",
    default_sort = "id ASC",
    append_only = true,
    read_policy = "journal_private"
)]
struct HostJournal {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    note_id: String,
}
#[cfg(feature = "sqlite")]
fn build_system_metadata() -> EntityMetadata {
    let mut metadata = HostJournal::metadata().clone();
    // Static system metadata can bind directly to stable runtime physical names.
    metadata.relations = vec![RelationMetadata {
        field_name: "note",
        target_type: "owned_notes",
        source_column: "note_id",
        target_column: "id",
        source_columns: &["note_id"],
        target_columns: &["id"],
        is_multiple: false,
        emit_foreign_key: true,
        on_delete: DeletePolicy::Restrict,
        propagate_change: RelationChangePropagation::None,
        search_fields: None,
    }]
    .into_boxed_slice();
    metadata
}
#[cfg(feature = "sqlite")]
fn system_metadata() -> &'static EntityMetadata {
    static METADATA: std::sync::LazyLock<EntityMetadata> =
        std::sync::LazyLock::new(build_system_metadata);
    &METADATA
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_composed_static_system_target_dependencies_and_policy_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let db = sqlite().await;
    let metadata = system_metadata();
    let composed = target::<SqliteBackend>().with_static_entities(&[metadata])?;
    let ownership = ManagedTableSet::new(["owned_notes".into(), "host_journal".into()])?;
    let manager = db.schema();
    let plan = manager
        .plan_owned_migration(
            "composed",
            "runtime plus journal",
            &composed,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    let static_plan = manager
        .plan_migration_to_entities("static", "equivalent", &[Note::metadata(), metadata])
        .await?;
    assert_eq!(plan.statements(), static_plan.statements);
    manager
        .apply_owned_migration(&plan, Default::default())
        .await?;
    let environment = manager
        .runtime_mutation_environment(std::sync::Arc::new(schema()), &composed, &ownership)
        .await?;
    assert_eq!(
        environment.incoming_dependencies("owned_notes")[0].source_table,
        "host_journal"
    );
    assert_eq!(
        environment.incoming_dependencies("owned_notes")[0].on_delete,
        DeletePolicy::Restrict
    );
    assert_eq!(metadata.read_policy, Some("journal_private"));
    assert!(
        manager
            .plan_owned_migration(
                "noop",
                "replan",
                &composed,
                &ownership,
                PlanOptions::strict()
            )
            .await?
            .steps()
            .is_empty()
    );
    assert!(
        manager
            .plan_migration_to_entities("static", "replan", &[Note::metadata(), metadata])
            .await?
            .steps
            .is_empty()
    );
    sqlx::query("INSERT INTO owned_notes(id,label) VALUES ('a','kept')")
        .execute(db.pool())
        .await?;
    sqlx::query("INSERT INTO host_journal(id,note_id) VALUES ('j','a')")
        .execute(db.pool())
        .await?;
    assert!(
        sqlx::query("UPDATE host_journal SET note_id='a'")
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM owned_notes WHERE id='a'")
            .execute(db.pool())
            .await
            .is_err()
    );
    // Reserved host infrastructure dependencies are also included, not filtered away.
    sqlx::query("CREATE TABLE __graphql_orm_custom_link(id TEXT PRIMARY KEY NOT NULL,note_id TEXT REFERENCES owned_notes(id) ON DELETE CASCADE)").execute(db.pool()).await?;
    let environment = manager
        .runtime_mutation_environment(std::sync::Arc::new(schema()), &composed, &ownership)
        .await?;
    assert!(
        environment
            .incoming_dependencies("owned_notes")
            .iter()
            .any(|dep| dep.source_table == "__graphql_orm_custom_link"
                && dep.on_delete == DeletePolicy::Cascade)
    );
    Ok(())
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
async fn postgres_competing_baselines_destructive_guards_and_atomic_failure()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("owned-migration-guards")?;
    let db = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    let manager = db.schema();
    let target = target::<PostgresBackend>();
    let ownership = ownership();
    let first = manager
        .plan_owned_migration(
            "first",
            "same baseline",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    let competing = manager
        .plan_owned_migration(
            "second",
            "same baseline",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(&first, Default::default())
        .await?;
    rejected(
        manager
            .apply_owned_migration(&competing, Default::default())
            .await
            .unwrap_err(),
        RuntimeMigrationDiagnosticCode::BaselineMismatch,
    );
    let drop = manager
        .plan_owned_migration(
            "drop",
            "explicit removal",
            &OwnedSchemaModel::default(),
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(
        manager
            .apply_owned_migration(&drop, Default::default())
            .await
            .is_err()
    );
    assert!(
        manager
            .apply_owned_migration(
                &drop,
                ApplyOptions {
                    allow_destructive: true,
                    additive_only: true,
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO owned_notes(id,label) VALUES ('a','kept')")
        .execute(db.pool())
        .await?;
    let mut definition = schema().schema().clone();
    definition.collections[0].fields.push(RuntimeField {
        id: FieldId::new("required")?,
        api_name: "required".into(),
        physical_column: "required".into(),
        value_kind: RuntimeValueKind::String,
        nullable: false,
        unique: false,
        filterable: false,
        sortable: false,
        generated: false,
        default: None,
    });
    let target = definition
        .validate()?
        .physical_schema::<PostgresBackend>(Default::default())?;
    let fail = manager
        .plan_owned_migration(
            "fail",
            "required on existing row",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    assert!(
        manager
            .apply_owned_migration(&fail, Default::default())
            .await
            .is_err()
    );
    let row: String = sqlx::query_scalar("SELECT label FROM owned_notes WHERE id='a'")
        .fetch_one(db.pool())
        .await?;
    assert_eq!(row, "kept");
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM __graphql_orm_migrations WHERE version='fail'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(history, 0);
    let drop = manager
        .plan_owned_migration(
            "drop",
            "guarded removal",
            &OwnedSchemaModel::default(),
            &ownership,
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(
            &drop,
            ApplyOptions {
                allow_destructive: true,
                ..Default::default()
            },
        )
        .await?;
    db.pool().close().await;
    owned.cleanup()?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "legacy_times", plural = "LegacyTimes")]
struct LegacyTime {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[date_field]
    created_at: String,
}
#[cfg(feature = "sqlite")]
#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "integer_times",
    plural = "IntegerTimes",
    default_sort = "id ASC"
)]
struct IntegerTime {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    created_at: i64,
}
#[cfg(feature = "sqlite")]
#[test]
fn legacy_datetime_default_conversion_fails_closed_without_changing_static_storage() {
    let static_target = SchemaModel::from_entities(&[LegacyTime::metadata()]);
    let legacy = static_target.tables[0]
        .columns
        .iter()
        .find(|c| c.name == "created_at")
        .unwrap();
    assert_eq!(legacy.default.as_deref(), Some("unixepoch()"));
    let error = RuntimeSchema::from_static_entities(&[LegacyTime::metadata()]).unwrap_err();
    let diagnostic = error
        .diagnostics()
        .iter()
        .find(|d| d.code == RuntimeSchemaDiagnosticCode::UnsupportedDefault)
        .unwrap();
    assert_eq!(
        diagnostic.collection.as_ref().map(CollectionId::as_str),
        Some("legacy_times")
    );
    assert_eq!(diagnostic.subject.as_deref(), Some("created_at"));
    assert!(diagnostic.message.contains("epoch-second datetime"));
    assert!(diagnostic.message.contains("RFC3339/native timestamp"));

    let runtime = RuntimeSchema::from_static_entities(&[IntegerTime::metadata()])
        .unwrap()
        .validate()
        .unwrap();
    let target = runtime
        .physical_schema::<SqliteBackend>(Default::default())
        .unwrap();
    let column = target.tables()[0]
        .columns()
        .iter()
        .find(|c| c.name == "created_at")
        .unwrap();
    assert_eq!(column.sql_type, "INTEGER");
    assert_eq!(column.default.as_deref(), Some("unixepoch()"));
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_owned_plans_reject_unrepresentable_indexes()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let db = Database::<SqliteBackend>::new(pool);
    let manager = db.schema();
    let target = target::<SqliteBackend>();
    let initial = manager
        .plan_owned_migration(
            "initial",
            "owned",
            &target,
            &ownership(),
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(&initial, ApplyOptions::default())
        .await?;
    for sql in [
        "CREATE INDEX unsupported ON owned_notes(label) WHERE score > 0",
        "CREATE INDEX unsupported ON owned_notes(label COLLATE NOCASE)",
        "CREATE INDEX unsupported ON owned_notes(lower(label))",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
        rejected(
            manager
                .plan_owned_migration(
                    "unsafe",
                    "cannot preserve",
                    &target,
                    &ownership(),
                    PlanOptions::strict(),
                )
                .await
                .unwrap_err(),
            RuntimeMigrationDiagnosticCode::InvalidPhysicalContract,
        );
        let retained: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name='unsupported'")
                .fetch_one(db.pool())
                .await?;
        assert_eq!(
            retained, 1,
            "planning does not alter unsupported catalog state"
        );
        sqlx::query("DROP INDEX unsupported")
            .execute(db.pool())
            .await?;
    }
    Ok(())
}

#[cfg(all(feature = "postgres", not(feature = "sqlite")))]
#[tokio::test]
async fn postgres_owned_plans_reject_unrepresentable_indexes()
-> Result<(), Box<dyn std::error::Error>> {
    let mut owned = owned_postgres::OwnedPostgres::start("owned-index-guards")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&owned.url)
        .await?;
    let db = Database::<PostgresBackend>::new(pool);
    let manager = db.schema();
    let target = target::<PostgresBackend>();
    let initial = manager
        .plan_owned_migration(
            "initial",
            "owned",
            &target,
            &ownership(),
            PlanOptions::strict(),
        )
        .await?;
    manager
        .apply_owned_migration(&initial, ApplyOptions::default())
        .await?;
    for sql in [
        "CREATE INDEX unsupported ON owned_notes(label) WHERE score > 0",
        "CREATE INDEX unsupported ON owned_notes(lower(label))",
        "CREATE INDEX unsupported ON owned_notes(label COLLATE \"C\")",
        "CREATE INDEX unsupported ON owned_notes(label DESC NULLS LAST)",
        "CREATE INDEX unsupported ON owned_notes(label) INCLUDE (score)",
        "CREATE INDEX unsupported ON owned_notes(label text_pattern_ops)",
    ] {
        sqlx::query(sql).execute(db.pool()).await?;
        rejected(
            manager
                .plan_owned_migration(
                    "unsafe",
                    "cannot preserve",
                    &target,
                    &ownership(),
                    PlanOptions::strict(),
                )
                .await
                .unwrap_err(),
            RuntimeMigrationDiagnosticCode::InvalidPhysicalContract,
        );
        let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_indexes WHERE schemaname=current_schema() AND indexname='unsupported'").fetch_one(db.pool()).await?;
        assert_eq!(retained, 1);
        sqlx::query("DROP INDEX unsupported")
            .execute(db.pool())
            .await?;
    }
    db.pool().close().await;
    owned.cleanup()?;
    Ok(())
}

#[test]
fn static_quoted_literal_defaults_convert_losslessly() {
    let mut metadata = Note::metadata().clone();
    metadata
        .fields
        .iter_mut()
        .find(|f| f.name == "label")
        .unwrap()
        .default = Some("'a''b'");
    let static_target = SchemaModel::from_entities(&[&metadata]);
    let runtime = RuntimeSchema::from_static_entities(&[&metadata])
        .unwrap()
        .validate()
        .unwrap();
    #[cfg(feature = "sqlite")]
    let owned = runtime
        .physical_schema::<SqliteBackend>(Default::default())
        .unwrap();
    #[cfg(all(feature = "postgres", not(feature = "sqlite")))]
    let owned = runtime
        .physical_schema::<PostgresBackend>(Default::default())
        .unwrap();
    assert_eq!(owned.stable_hash(), static_target.stable_hash());
    assert_eq!(
        owned.tables()[0]
            .columns()
            .iter()
            .find(|c| c.name == "label")
            .unwrap()
            .default
            .as_deref(),
        Some("'a''b'")
    );
    metadata
        .fields
        .iter_mut()
        .find(|f| f.name == "label")
        .unwrap()
        .default = Some("'a'b'");
    let diagnostics = RuntimeSchema::from_static_entities(&[&metadata]).unwrap_err();
    assert!(diagnostics.diagnostics().iter().any(|d| d.code
        == RuntimeSchemaDiagnosticCode::UnsupportedDefault
        && d.subject.as_deref() == Some("label")));
}
