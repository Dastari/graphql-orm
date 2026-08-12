#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "decommish_snapshot_records",
    plural = "DecommishSnapshotRecords",
    default_sort = "provider ASC, tenant_key ASC, generation DESC",
    unique_composite = "provider,tenant_key,digest",
    index(
        name = "idx_decommish_snapshot_records_latest",
        columns = ["provider", "tenant_key", "generation"],
        directions = ["asc", "asc", "desc"]
    )
)]
struct DecommishSnapshotRecord {
    #[primary_key]
    provider: String,
    #[primary_key]
    tenant_key: String,
    tenant_id: Option<String>,
    #[primary_key]
    #[graphql_orm(min_exclusive = 0)]
    generation: i64,
    schema_version: i64,
    #[graphql_orm(non_negative)]
    record_count: i64,
    #[graphql_orm(non_negative)]
    serialized_bytes: i64,
    digest: String,
    payload: String,
    #[graphql_orm(default = false)]
    created_at: String,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "decommish_snapshot_outbox",
    plural = "DecommishSnapshotOutboxRows",
    default_sort = "provider ASC, tenant_key ASC, generation ASC"
)]
struct DecommishSnapshotOutboxRow {
    #[primary_key]
    provider: String,
    #[primary_key]
    tenant_key: String,
    #[primary_key]
    #[graphql_orm(min_exclusive = 0)]
    generation: i64,
    digest: String,
    payload: String,
    #[graphql_orm(default = false)]
    created_at: String,
    #[graphql(skip)]
    #[relation(
        target = "DecommishSnapshotRecord",
        from = ["provider", "tenant_key", "generation"],
        to = ["provider", "tenant_key", "generation"],
        on_delete = "cascade"
    )]
    snapshot: Option<String>,
}

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "renamed_fk_parents",
    plural = "RenamedFkParents",
    default_sort = "provider ASC, generation ASC",
    index(
        name = "idx_renamed_fk_parent_latest",
        columns = ["provider_code", "generation"],
        directions = ["asc", "desc"]
    )
)]
struct RenamedFkParent {
    #[primary_key]
    #[graphql_orm(db_column = "provider")]
    provider_code: String,
    #[primary_key]
    generation: i64,
}

#[derive(GraphQLSchemaEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "sqlite",
    table = "renamed_fk_children",
    plural = "RenamedFkChildren",
    default_sort = "provider ASC, generation ASC"
)]
struct RenamedFkChild {
    #[primary_key]
    #[graphql_orm(db_column = "provider")]
    provider_code: String,
    #[primary_key]
    generation: i64,
    #[graphql(skip)]
    #[relation(
        target = "RenamedFkParent",
        from = ["provider_code", "generation"],
        to = ["provider", "generation"],
        on_delete = "cascade"
    )]
    parent: Option<String>,
}

fn snapshot_entities() -> [&'static EntityMetadata; 2] {
    [
        DecommishSnapshotRecord::metadata(),
        DecommishSnapshotOutboxRow::metadata(),
    ]
}

async fn legacy_database() -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for statement in [
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE IF NOT EXISTS decommish_snapshot_records (provider TEXT NOT NULL, tenant_key TEXT NOT NULL, tenant_id TEXT, generation INTEGER NOT NULL CHECK(generation > 0), schema_version INTEGER NOT NULL, record_count INTEGER NOT NULL CHECK(record_count >= 0), serialized_bytes INTEGER NOT NULL CHECK(serialized_bytes >= 0), digest TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(provider, tenant_key, generation), UNIQUE(provider, tenant_key, digest))",
        "CREATE TABLE IF NOT EXISTS decommish_snapshot_outbox (provider TEXT NOT NULL, tenant_key TEXT NOT NULL, generation INTEGER NOT NULL CHECK(generation > 0), digest TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(provider, tenant_key, generation), FOREIGN KEY(provider, tenant_key, generation) REFERENCES decommish_snapshot_records(provider, tenant_key, generation) ON DELETE CASCADE)",
        "CREATE INDEX IF NOT EXISTS idx_decommish_snapshot_records_latest ON decommish_snapshot_records(provider, tenant_key, generation DESC)",
    ] {
        graphql_orm::sqlx::query(statement)
            .execute(database.pool())
            .await?;
    }
    Ok(database)
}

async fn insert_snapshot(
    database: &Database<SqliteBackend>,
    provider: &str,
    tenant_key: &str,
    generation: i64,
    digest: &str,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "INSERT INTO decommish_snapshot_records
         (provider, tenant_key, tenant_id, generation, schema_version, record_count,
          serialized_bytes, digest, payload, created_at)
         VALUES (?, ?, NULL, ?, 1, 0, 0, ?, '{}', '2026-08-10T00:00:00Z')",
    )
    .bind(provider)
    .bind(tenant_key)
    .bind(generation)
    .bind(digest)
    .execute(database.pool())
    .await?;
    Ok(())
}

async fn insert_outbox(
    database: &Database<SqliteBackend>,
    provider: &str,
    tenant_key: &str,
    generation: i64,
    digest: &str,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "INSERT INTO decommish_snapshot_outbox
         (provider, tenant_key, generation, digest, payload, created_at)
         VALUES (?, ?, ?, ?, '{}', '2026-08-10T00:00:00Z')",
    )
    .bind(provider)
    .bind(tenant_key)
    .bind(generation)
    .bind(digest)
    .execute(database.pool())
    .await?;
    Ok(())
}

#[test]
fn compound_relation_lowering_retains_order_and_translates_source_db_columns() {
    let schema =
        SchemaModel::from_entities(&[RenamedFkParent::metadata(), RenamedFkChild::metadata()]);
    schema
        .validate_physical_contract()
        .expect("renamed compound relation should validate");
    let child = schema
        .tables
        .iter()
        .find(|table| table.table_name == "renamed_fk_children")
        .expect("child table");
    assert_eq!(child.foreign_keys.len(), 1);
    assert_eq!(
        child.foreign_keys[0].source_columns().collect::<Vec<_>>(),
        ["provider", "generation"]
    );
    assert_eq!(
        child.foreign_keys[0].target_columns().collect::<Vec<_>>(),
        ["provider", "generation"]
    );
    let parent = schema
        .tables
        .iter()
        .find(|table| table.table_name == "renamed_fk_parents")
        .expect("parent table");
    let latest = parent
        .indexes
        .iter()
        .find(|index| index.name == "idx_renamed_fk_parent_latest")
        .expect("renamed directional index");
    assert_eq!(latest.columns, ["provider", "generation"]);
    assert_eq!(latest.direction_at(1), IndexDirection::Desc);
}

#[test]
fn semantic_comparison_ignores_only_physical_names_and_redundant_check_parentheses() {
    let target = SchemaModel::from_entities(&snapshot_entities());
    let mut equivalent = target.clone();
    let outbox = equivalent
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("outbox table");
    outbox.foreign_keys[0].constraint_name = Some("legacy_snapshot_fk".to_string());
    outbox.check_constraints[0].name = "legacy_positive_generation".to_string();
    outbox.check_constraints[0].expression =
        format!("(( {} ))", outbox.check_constraints[0].expression);
    let equivalent_diff =
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &equivalent, &target);
    assert!(equivalent_diff.steps.is_empty());
    assert_eq!(equivalent.stable_hash(), target.stable_hash());

    let mut weakened = equivalent;
    let outbox = weakened
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("outbox table");
    outbox.check_constraints[0].expression = "generation >= 0".to_string();
    let weakened_diff = diff_schema_models_for_backend(DatabaseBackend::Sqlite, &weakened, &target);
    assert!(
        weakened_diff
            .steps
            .iter()
            .any(|step| matches!(step, MigrationStep::SetCheckConstraints { .. }))
    );

    let mut ambiguous = target.clone();
    let outbox = ambiguous
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("outbox table");
    outbox.foreign_keys[0].constraint_name = Some("snapshot_fk_first".to_string());
    let mut duplicate = outbox.foreign_keys[0].clone();
    duplicate.constraint_name = Some("snapshot_fk_second".to_string());
    outbox.foreign_keys.push(duplicate);
    let ambiguous_error = ambiguous
        .validate_physical_contract()
        .expect_err("target metadata must reject duplicate semantic foreign keys");
    assert!(ambiguous_error.to_string().contains("duplicate semantic"));
    let ambiguous_diff =
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &ambiguous, &target);
    assert_eq!(
        ambiguous_diff
            .steps
            .iter()
            .filter(|step| matches!(step, MigrationStep::DropForeignKey { .. }))
            .count(),
        1,
        "a duplicate live constraint must not be silently adopted"
    );

    let mut reordered = target.clone();
    let outbox = reordered
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("outbox table");
    outbox.foreign_keys[0].column_pairs.swap(1, 2);
    let reordered_diff =
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &reordered, &target);
    assert!(
        reordered_diff
            .steps
            .iter()
            .any(|step| matches!(step, MigrationStep::DropForeignKey { .. }))
    );
    assert!(
        reordered_diff
            .steps
            .iter()
            .any(|step| matches!(step, MigrationStep::AddForeignKey { .. }))
    );

    let mut case_sensitive_identifier = target.clone();
    let outbox = case_sensitive_identifier
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("outbox table");
    outbox.check_constraints[0].expression = "\"Generation\" > 0".to_string();
    let case_sensitive_diff = diff_schema_models_for_backend(
        DatabaseBackend::Postgres,
        &case_sensitive_identifier,
        &target,
    );
    assert!(
        case_sensitive_diff
            .steps
            .iter()
            .any(|step| matches!(step, MigrationStep::SetCheckConstraints { .. })),
        "quoted PostgreSQL identifier case must not be normalized into a false match"
    );
}

#[test]
fn compound_fk_validation_rejects_non_unique_target_and_index_direction_drift() {
    let target = SchemaModel::from_entities(&snapshot_entities());
    let mut invalid = target.clone();
    let records = invalid
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_records")
        .expect("records table");
    records.primary_keys.clear();
    records.primary_key.clear();
    for column in &mut records.columns {
        column.is_primary_key = false;
    }
    let error = invalid
        .validate_physical_contract()
        .expect_err("compound target tuple must remain unique");
    assert!(error.to_string().contains("exact unique key"));
    let abi_error = SchemaAbi::new(vec![SchemaStage::new(
        "invalid-compound-fk",
        "invalid target tuple",
        invalid,
    )])
    .expect_err("schema ABI must reject the same invalid physical contract");
    assert!(abi_error.to_string().contains("exact unique key"));

    let mut wrong_direction = target.clone();
    let records = wrong_direction
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_records")
        .expect("records table");
    let latest = records
        .indexes
        .iter_mut()
        .find(|index| index.name == "idx_decommish_snapshot_records_latest")
        .expect("latest index");
    latest.column_directions = &[
        IndexDirection::Asc,
        IndexDirection::Asc,
        IndexDirection::Asc,
    ];
    let direction_diff =
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &wrong_direction, &target);
    assert!(direction_diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::DropIndex { index_name, .. }
            if index_name == "idx_decommish_snapshot_records_latest"
    )));
    assert!(direction_diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::CreateIndex { index, .. }
            if index.name == "idx_decommish_snapshot_records_latest"
    )));

    let mut malformed_live_direction = target.clone();
    let records = malformed_live_direction
        .tables
        .iter_mut()
        .find(|table| table.table_name == "decommish_snapshot_records")
        .expect("records table");
    let latest = records
        .indexes
        .iter_mut()
        .find(|index| index.name == "idx_decommish_snapshot_records_latest")
        .expect("latest index");
    latest.column_directions = &[
        IndexDirection::Asc,
        IndexDirection::Asc,
        IndexDirection::Desc,
        IndexDirection::Desc,
    ];
    let malformed_direction_diff =
        diff_schema_models_for_backend(DatabaseBackend::Sqlite, &malformed_live_direction, &target);
    assert!(malformed_direction_diff.steps.iter().any(|step| matches!(
        step,
        MigrationStep::DropIndex { index_name, .. }
            if index_name == "idx_decommish_snapshot_records_latest"
    )));
}

#[tokio::test]
async fn fresh_sqlite_schema_round_trips_compound_fk_checks_and_descending_index()
-> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query("PRAGMA foreign_keys = ON")
        .execute(database.pool())
        .await?;
    let entities = snapshot_entities();
    let plan = database
        .schema()
        .plan_migration_to_entities("snapshot-fresh-v1", "fresh snapshot schema", &entities)
        .await?;
    assert!(plan.statements.iter().any(|statement| {
        statement.contains(
            "FOREIGN KEY (provider, tenant_key, generation) REFERENCES decommish_snapshot_records(provider, tenant_key, generation) ON DELETE CASCADE",
        )
    }));
    assert!(plan.statements.iter().any(|statement| {
        statement.contains("idx_decommish_snapshot_records_latest")
            && statement.contains("\"provider\" ASC, \"tenant_key\" ASC, \"generation\" DESC")
    }));
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    insert_snapshot(&database, "edge", "tenant-a", 1, "digest-a").await?;
    insert_snapshot(&database, "edge", "tenant-a", 2, "digest-a")
        .await
        .expect_err("digest uniqueness must remain partitioned by provider and tenant");
    insert_snapshot(&database, "edge", "tenant-b", 2, "digest-a").await?;
    insert_snapshot(&database, "edge", "tenant-c", 0, "digest-zero")
        .await
        .expect_err("generation must remain positive");
    graphql_orm::sqlx::query(
        "INSERT INTO decommish_snapshot_records
         (provider, tenant_key, tenant_id, generation, schema_version, record_count,
          serialized_bytes, digest, payload, created_at)
         VALUES ('edge', 'tenant-c', NULL, 1, 1, -1, 0, 'digest-negative', '{}', 'now')",
    )
    .execute(database.pool())
    .await
    .expect_err("record counts must remain non-negative");
    graphql_orm::sqlx::query(
        "INSERT INTO decommish_snapshot_records
         (provider, tenant_key, tenant_id, generation, schema_version, record_count,
          serialized_bytes, digest, payload, created_at)
         VALUES ('edge', 'tenant-d', NULL, 1, 1, 0, -1, 'digest-negative', '{}', 'now')",
    )
    .execute(database.pool())
    .await
    .expect_err("serialized byte sizes must remain non-negative");

    let live = introspect_sqlite_schema(&database).await?;
    let outbox = live
        .tables
        .iter()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("outbox table");
    assert_eq!(outbox.foreign_keys.len(), 1);
    assert_eq!(outbox.foreign_keys[0].column_pairs.len(), 3);
    let restart = database
        .schema()
        .plan_migration_to_entities("snapshot-fresh-v2", "fresh schema restart", &entities)
        .await?;
    assert!(restart.steps.is_empty(), "unexpected drift: {restart:#?}");
    assert!(restart.statements.is_empty());
    let violations = graphql_orm::sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(database.pool())
        .await?;
    assert!(violations.is_empty());

    Ok(())
}

#[tokio::test]
async fn exact_legacy_schema_is_recorded_without_ddl_or_row_loss() -> graphql_orm::Result<()> {
    let database = legacy_database().await?;
    insert_snapshot(&database, "edge", "tenant-a", 1, "digest-a").await?;
    insert_outbox(&database, "edge", "tenant-a", 1, "digest-a").await?;
    let entities = snapshot_entities();
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "snapshot-adopt-v1",
            "adopt exact host application snapshot schema",
            &entities,
        )
        .await?;
    assert!(plan.steps.is_empty(), "adoption attempted DDL: {plan:#?}");
    assert!(plan.statements.is_empty());

    let fresh = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    graphql_orm::sqlx::query("PRAGMA foreign_keys = ON")
        .execute(fresh.pool())
        .await?;
    let fresh_plan = fresh
        .schema()
        .plan_migration_to_entities(
            "snapshot-fresh-hash-v1",
            "compare fresh snapshot metadata",
            &entities,
        )
        .await?;
    assert_eq!(plan.target_schema_hash, fresh_plan.target_schema_hash);

    let report = database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    assert_eq!(report.statements_applied, 0);
    assert!(!report.already_applied);
    let violations = graphql_orm::sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(database.pool())
        .await?;
    assert!(violations.is_empty());

    let history_count: i64 = graphql_orm::sqlx::query_scalar(
        "SELECT COUNT(*) FROM __graphql_orm_migrations WHERE version = ?",
    )
    .bind("snapshot-adopt-v1")
    .fetch_one(database.pool())
    .await?;
    assert_eq!(history_count, 1);
    let recorded_target_hash: Option<String> = graphql_orm::sqlx::query_scalar(
        "SELECT target_schema_hash FROM __graphql_orm_migrations WHERE version = ?",
    )
    .bind("snapshot-adopt-v1")
    .fetch_one(database.pool())
    .await?;
    assert_eq!(
        recorded_target_hash.as_deref(),
        Some(plan.target_schema_hash.as_str())
    );
    let row_count: i64 =
        graphql_orm::sqlx::query_scalar("SELECT COUNT(*) FROM decommish_snapshot_records")
            .fetch_one(database.pool())
            .await?;
    assert_eq!(row_count, 1);
    let restart = database
        .schema()
        .plan_migration_to_entities("snapshot-adopt-v2", "adopted restart", &entities)
        .await?;
    assert!(restart.steps.is_empty(), "unexpected drift: {restart:#?}");
    assert!(restart.statements.is_empty());
    Ok(())
}

#[tokio::test]
async fn compound_fk_rejects_partial_matches_and_cascades_only_exact_tuple()
-> graphql_orm::Result<()> {
    let database = legacy_database().await?;
    insert_snapshot(&database, "edge-a", "tenant-a", 1, "digest-a").await?;
    insert_snapshot(&database, "edge-b", "tenant-b", 2, "digest-b").await?;
    insert_outbox(&database, "edge-a", "tenant-a", 1, "digest-a").await?;
    insert_outbox(&database, "edge-b", "tenant-b", 2, "digest-b").await?;

    let mismatch = insert_outbox(&database, "edge-a", "tenant-b", 2, "bad")
        .await
        .expect_err("partial tuple must not satisfy the foreign key");
    assert!(
        mismatch
            .to_string()
            .contains("FOREIGN KEY constraint failed")
    );

    graphql_orm::sqlx::query(
        "DELETE FROM decommish_snapshot_records
         WHERE provider = ? AND tenant_key = ? AND generation = ?",
    )
    .bind("edge-a")
    .bind("tenant-a")
    .bind(1_i64)
    .execute(database.pool())
    .await?;
    let rows = graphql_orm::sqlx::query(
        "SELECT provider, tenant_key, generation FROM decommish_snapshot_outbox",
    )
    .fetch_all(database.pool())
    .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].try_get::<String, _>("provider")?, "edge-b");
    assert_eq!(rows[0].try_get::<String, _>("tenant_key")?, "tenant-b");
    assert_eq!(rows[0].try_get::<i64, _>("generation")?, 2);
    let violations = graphql_orm::sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(database.pool())
        .await?;
    assert!(violations.is_empty());
    Ok(())
}

#[tokio::test]
async fn partial_legacy_fk_is_not_adopted_as_the_compound_contract() -> graphql_orm::Result<()> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for statement in [
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE decommish_snapshot_records (provider TEXT NOT NULL, tenant_key TEXT NOT NULL, tenant_id TEXT, generation INTEGER NOT NULL CHECK(generation > 0), schema_version INTEGER NOT NULL, record_count INTEGER NOT NULL CHECK(record_count >= 0), serialized_bytes INTEGER NOT NULL CHECK(serialized_bytes >= 0), digest TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(provider, tenant_key, generation), UNIQUE(provider, tenant_key, digest))",
        "CREATE TABLE decommish_snapshot_outbox (provider TEXT NOT NULL, tenant_key TEXT NOT NULL, generation INTEGER NOT NULL CHECK(generation > 0), digest TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(provider, tenant_key, generation), FOREIGN KEY(provider) REFERENCES decommish_snapshot_records(provider) ON DELETE CASCADE)",
        "CREATE INDEX idx_decommish_snapshot_records_latest ON decommish_snapshot_records(provider, tenant_key, generation DESC)",
    ] {
        graphql_orm::sqlx::query(statement)
            .execute(database.pool())
            .await?;
    }
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "snapshot-reject-partial",
            "reject partial legacy relation",
            &snapshot_entities(),
        )
        .await?;
    assert!(
        plan.steps
            .iter()
            .any(|step| matches!(step.step, MigrationStep::DropForeignKey { .. }))
    );
    assert!(
        plan.steps
            .iter()
            .any(|step| matches!(step.step, MigrationStep::AddForeignKey { .. }))
    );
    assert!(!plan.statements.is_empty());
    Ok(())
}

#[tokio::test]
async fn compound_fk_rebuild_failure_rolls_back_schema_data_and_history() -> graphql_orm::Result<()>
{
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    for statement in [
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE decommish_snapshot_records (provider TEXT NOT NULL, tenant_key TEXT NOT NULL, tenant_id TEXT, generation INTEGER NOT NULL CHECK(generation > 0), schema_version INTEGER NOT NULL, record_count INTEGER NOT NULL CHECK(record_count >= 0), serialized_bytes INTEGER NOT NULL CHECK(serialized_bytes >= 0), digest TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(provider, tenant_key, generation), UNIQUE(provider, tenant_key, digest))",
        "CREATE TABLE decommish_snapshot_outbox (provider TEXT NOT NULL, tenant_key TEXT NOT NULL, generation INTEGER NOT NULL CHECK(generation > 0), digest TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(provider, tenant_key, generation))",
        "CREATE INDEX idx_decommish_snapshot_records_latest ON decommish_snapshot_records(provider, tenant_key, generation DESC)",
        "INSERT INTO decommish_snapshot_outbox (provider, tenant_key, generation, digest, payload, created_at) VALUES ('orphan', 'tenant', 1, 'digest', '{}', '2026-08-10T00:00:00Z')",
    ] {
        graphql_orm::sqlx::query(statement)
            .execute(database.pool())
            .await?;
    }
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "snapshot-failing-rebuild",
            "add compound integrity to invalid legacy rows",
            &snapshot_entities(),
        )
        .await?;
    let error = database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect_err("foreign_key_check must reject the orphaned rebuild");
    assert!(error.to_string().contains("foreign_key_check failed"));

    let orphan_count: i64 = graphql_orm::sqlx::query_scalar(
        "SELECT COUNT(*) FROM decommish_snapshot_outbox WHERE provider = 'orphan'",
    )
    .fetch_one(database.pool())
    .await?;
    assert_eq!(orphan_count, 1);
    let live = introspect_sqlite_schema(&database).await?;
    let outbox = live
        .tables
        .iter()
        .find(|table| table.table_name == "decommish_snapshot_outbox")
        .expect("original outbox table remains");
    assert!(outbox.foreign_keys.is_empty());
    let history_count: i64 = graphql_orm::sqlx::query_scalar(
        "SELECT COUNT(*) FROM __graphql_orm_migrations WHERE version = ?",
    )
    .bind("snapshot-failing-rebuild")
    .fetch_one(database.pool())
    .await?;
    assert_eq!(history_count, 0);
    Ok(())
}
