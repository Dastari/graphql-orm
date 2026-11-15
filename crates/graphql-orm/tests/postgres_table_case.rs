//! Bare PostgreSQL entity identifiers retain their historical case folding.
#![cfg(all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))))]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "postgres", table = "MixedNotes", plural = "MixedNotes")]
struct MixedNote {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    title: String,
}
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "MixedNotes", plural = "MixedNotesV2")]
#[allow(dead_code)]
struct MixedNoteV2 {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    title: String,
    extra: Option<String>,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "postgres", table = "lower_notes", plural = "LowerNotes")]
struct LowerNote {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    title: String,
}
#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "lower_notes", plural = "LowerNotesV2")]
#[allow(dead_code)]
struct LowerNoteV2 {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    title: String,
    extra: Option<String>,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn table_case_create_write_read_migrate_and_replan() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = owned_postgres::OwnedPostgres::pool("table-case", 5).await?;
    let db = Database::<PostgresBackend>::new(pool);
    let entities = [MixedNote::metadata(), LowerNote::metadata()];
    let plan = db
        .schema()
        .plan_migration_to_entities("case-1", "create", &entities)
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            let mixed = tx
                .insert::<MixedNote>(CreateMixedNoteInput {
                    title: "mixed".into(),
                })
                .await?;
            let lower = tx
                .insert::<LowerNote>(CreateLowerNoteInput {
                    title: "lower".into(),
                })
                .await?;
            assert_eq!(
                tx.find_by_id::<MixedNote>(&mixed.id).await?.unwrap().title,
                "mixed"
            );
            assert_eq!(
                tx.find_by_id::<LowerNote>(&lower.id).await?.unwrap().title,
                "lower"
            );
            Ok(())
        })
    })
    .await?;
    let unchanged = db
        .schema()
        .plan_migration_to_entities("case-2", "unchanged", &entities)
        .await?;
    assert!(unchanged.steps.is_empty(), "{:?}", unchanged.steps);
    let entities = [MixedNoteV2::metadata(), LowerNoteV2::metadata()];
    let plan = db
        .schema()
        .plan_migration_to_entities("case-3", "add column", &entities)
        .await?;
    assert_eq!(plan.steps.len(), 2, "{:?}", plan.steps);
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    for table in ["mixednotes", "lower_notes"] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE extra IS NULL"))
                .fetch_one(db.pool())
                .await?;
        assert_eq!(count, 1, "migration preserved the row in {table}");
    }
    let unchanged = db
        .schema()
        .plan_migration_to_entities("case-4", "unchanged", &entities)
        .await?;
    assert!(unchanged.steps.is_empty(), "{:?}", unchanged.steps);
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "LegacyMixedNotes",
    plural = "LegacyMixedNotes"
)]
#[allow(dead_code)]
struct LegacyMixedNote {
    #[primary_key]
    id: String,
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn historical_bare_mixed_case_install_needs_no_migration()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("table-case-legacy", 5).await?;
    // Exactly the unquoted DDL emitted by releases before this fix.
    sqlx::query(
        "CREATE TABLE LegacyMixedNotes (id TEXT NOT NULL PRIMARY KEY, title TEXT NOT NULL)",
    )
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO LegacyMixedNotes VALUES ('existing', 'preserved')")
        .execute(&pool)
        .await?;
    let db = Database::<PostgresBackend>::new(pool);
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "legacy-case",
            "existing install",
            &[LegacyMixedNote::metadata()],
        )
        .await?;
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    let title: String =
        sqlx::query_scalar("SELECT title FROM legacymixednotes WHERE id = 'existing'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(title, "preserved");
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "LegacyMixedCaseSearchRecordsWithLongIdentifier",
    plural = "LegacySearchNotes"
)]
#[graphql_orm(search(index = true))]
#[allow(dead_code)]
struct LegacySearchNote {
    #[primary_key]
    id: String,
    #[graphql_orm(searchable(weight = "A"))]
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn mixed_case_search_keeps_declared_metadata_and_helper_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let metadata = LegacySearchNote::metadata();
    let declared = "LegacyMixedCaseSearchRecordsWithLongIdentifier";
    assert_eq!(metadata.table_name, declared);
    let helper = search_table_name(declared);
    assert_ne!(
        helper.to_ascii_lowercase(),
        search_table_name(&declared.to_ascii_lowercase())
    );
    let pool = owned_postgres::OwnedPostgres::pool("table-case-search", 5).await?;
    let db = Database::<PostgresBackend>::new(pool);
    let plan = db
        .schema()
        .plan_migration_to_entities("search-case-1", "create", &[metadata])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let exists: bool = sqlx::query_scalar("SELECT pg_catalog.to_regclass($1) IS NOT NULL")
        .bind(&helper)
        .fetch_one(db.pool())
        .await?;
    assert!(exists, "historical helper name resolves");
    let unchanged = db
        .schema()
        .plan_migration_to_entities("search-case-2", "unchanged", &[metadata])
        .await?;
    assert!(unchanged.steps.is_empty(), "{:?}", unchanged.steps);
    Ok(())
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "MixedCaseHistory",
    plural = "MixedCaseHistories",
    append_only = true
)]
#[graphql_rls(select(predicate = "true"), insert(predicate = "true"))]
#[allow(dead_code)]
struct CaseHistory {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn mixed_case_triggers_and_rls_apply_and_replan() -> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("table-case-policy", 5).await?;
    let db = Database::<PostgresBackend>::new(pool);
    let target =
        SchemaTarget::from_entities(&[CaseHistory::metadata()], &[CaseHistory::rls_metadata()]);
    let plan = db
        .schema()
        .plan_schema_target("case-policy-1", "create", &target)
        .await?;
    db.schema()
        .apply_schema_target(&plan, ApplyOptions::default())
        .await?;
    let unchanged = db
        .schema()
        .plan_schema_target("case-policy-2", "unchanged", &target)
        .await?;
    assert!(
        unchanged.migration.steps.is_empty(),
        "{:?}",
        unchanged.migration.steps
    );
    let installed: bool = sqlx::query_scalar("SELECT relrowsecurity FROM pg_catalog.pg_class WHERE oid = pg_catalog.to_regclass('MixedCaseHistory')")
        .fetch_one(db.pool()).await?;
    assert!(installed);
    Ok(())
}
