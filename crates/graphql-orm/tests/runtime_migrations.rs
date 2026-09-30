#![cfg(any(feature = "sqlite", feature = "postgres"))]

use graphql_orm::GraphQLSchemaEntity;
use graphql_orm::db::Database;
use graphql_orm::graphql::orm::*;

#[cfg(feature = "postgres")]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "owned_notes", plural = "Notes", default_sort = "id ASC")]
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
        target::<SqliteBackend>().stable_hash(),
        static_target.stable_hash()
    );
    #[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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
