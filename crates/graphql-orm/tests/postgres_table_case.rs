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

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(backend = "postgres", table = "shared_notes", plural = "SharedNotes")]
#[allow(dead_code)]
struct SharedNote {
    #[primary_key]
    id: String,
    title: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "SharedMixedNotes",
    plural = "SharedMixedNotes"
)]
#[allow(dead_code)]
struct SharedMixedNote {
    #[primary_key]
    id: String,
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn bare_names_never_adopt_tables_later_in_search_path()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("table-case-search-path", 1).await?;
    sqlx::raw_sql("CREATE SCHEMA tenant; CREATE TABLE public.shared_notes (id TEXT PRIMARY KEY, legacy TEXT); CREATE TABLE public.sharedmixednotes (id TEXT PRIMARY KEY, legacy TEXT); INSERT INTO public.shared_notes VALUES ('public', 'preserved'); SET search_path = tenant, public")
        .execute(&pool).await?;
    let db = Database::<PostgresBackend>::new(pool);
    let plan = db
        .schema()
        .plan_migration_to_entities("path-1", "create in tenant", &[SharedNote::metadata()])
        .await?;
    assert_eq!(plan.steps.len(), 1, "{:?}", plan.steps);
    assert!(
        matches!(plan.steps[0].step, MigrationStep::CreateTable(_)),
        "{:?}",
        plan.steps
    );
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let legacy: String =
        sqlx::query_scalar("SELECT legacy FROM public.shared_notes WHERE id = 'public'")
            .fetch_one(db.pool())
            .await?;
    assert_eq!(legacy, "preserved");
    let title_exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'tenant' AND table_name = 'shared_notes' AND column_name = 'title')").fetch_one(db.pool()).await?;
    assert!(title_exists);
    // This spelling needs a folding probe, which must still stay in tenant.
    let mixed = db
        .schema()
        .plan_migration_to_entities_with_options(
            "path-2",
            "create mixed table in tenant",
            &[SharedMixedNote::metadata()],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert_eq!(mixed.steps.len(), 1, "{:?}", mixed.steps);
    assert!(matches!(mixed.steps[0].step, MigrationStep::CreateTable(_)));
    db.schema()
        .apply_migration(&mixed, ApplyOptions::default())
        .await?;
    let untouched: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'sharedmixednotes' AND column_name = 'legacy')").fetch_one(db.pool()).await?;
    assert!(untouched);
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn quoted_and_folded_tables_are_distinct_during_replan()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("table-case-both", 1).await?;
    sqlx::raw_sql("CREATE TABLE mixednotes (id UUID NOT NULL PRIMARY KEY, title TEXT NOT NULL); CREATE INDEX idx_mixednotes_title ON mixednotes (title); CREATE TABLE \"MixedNotes\" (legacy TEXT)").execute(&pool).await?;
    let db = Database::<PostgresBackend>::new(pool);
    let plan = db
        .schema()
        .plan_migration_to_entities_with_options(
            "both",
            "existing folded table",
            &[MixedNote::metadata()],
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);
    for declarations in [["mixednotes", "MixedNotes"], ["MixedNotes", "mixednotes"]] {
        let model = PostgresBackend::introspect_schema_including(db.pool(), &declarations).await?;
        let folded = model
            .tables
            .iter()
            .find(|table| table.columns.iter().any(|column| column.name == "id"))
            .unwrap();
        assert_eq!(
            folded.table_name, declarations[0],
            "the first reported declaration wins"
        );
        assert!(
            model
                .tables
                .iter()
                .any(|table| table.table_name == "\"MixedNotes\"")
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn two_declarations_of_one_physical_table_are_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("table-case-alias", 1).await?;
    let db = Database::<PostgresBackend>::new(pool);
    for metadata in [MixedNote::metadata(), LegacySearchNote::metadata()] {
        let mut target = SchemaModel::from_entities(&[metadata]);
        let mut alias = target.tables[0].clone();
        alias.table_name = alias.table_name.to_ascii_lowercase();
        alias.entity_name.push_str("Alias");
        target.tables.push(alias);
        let result = db.schema().plan_migration(
            "alias",
            "refuse aliases",
            &SchemaModel {
                extensions: vec![],
                tables: vec![],
            },
            &target,
        );
        let error = result.expect_err("two declarations must be refused before DDL");
        assert!(
            TableIdentifierConflict::from_error(&error).is_some(),
            "{error:?}"
        );
        let error = db
            .apply_schema_stages(&[SchemaStage::new("alias-stage", "refuse aliases", target)])
            .await
            .expect_err("staged alias validation must precede history DDL");
        assert!(
            TableIdentifierConflict::from_error(&error).is_some(),
            "{error:?}"
        );
    }
    let tables: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'").fetch_one(db.pool()).await?;
    assert_eq!(tables, 0);
    Ok(())
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "postgres",
    table = "MixedDmlNotes",
    plural = "MixedDmlNotes",
    upsert = "key"
)]
struct DmlNote {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    key: String,
    #[sortable]
    title: String,
}

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn mixed_case_update_upsert_delete_and_pagination() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = owned_postgres::OwnedPostgres::pool("table-case-dml", 1).await?;
    let db = Database::<PostgresBackend>::new(pool);
    let plan = db
        .schema()
        .plan_migration_to_entities("dml", "create", &[DmlNote::metadata()])
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let id = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let row = tx
                    .insert::<DmlNote>(CreateDmlNoteInput {
                        key: "one".into(),
                        title: "first".into(),
                    })
                    .await?;
                tx.update_by_id::<DmlNote>(
                    &row.id,
                    UpdateDmlNoteInput {
                        title: Some("updated".into()),
                        ..Default::default()
                    },
                )
                .await?;
                tx.upsert::<DmlNote>(CreateDmlNoteInput {
                    key: "one".into(),
                    title: "upserted".into(),
                })
                .await?;
                assert_eq!(
                    tx.find_by_id::<DmlNote>(&row.id).await?.unwrap().title,
                    "upserted"
                );
                Ok(row.id)
            })
        })
        .await?;
    let rows = EntityQuery::<DmlNote>::new()
        .paginate(&PageInput {
            limit: Some(1),
            offset: Some(0),
        })
        .fetch_all(&db)
        .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    db.transaction(TransactionMode::StateMachine, move |tx| {
        Box::pin(async move {
            tx.delete_by_id::<DmlNote>(&id).await?;
            Ok(())
        })
    })
    .await?;
    assert!(
        EntityQuery::<DmlNote>::new()
            .fetch_all(&db)
            .await?
            .is_empty()
    );
    Ok(())
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "postgres",
    table = "MixedDeleteParents",
    plural = "CaseDeleteParents",
    dependent_deletes
)]
struct CaseDeleteParent {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "postgres",
    table = "MixedDeleteChildren",
    plural = "CaseDeleteChildren",
    dependent_deletes
)]
struct CaseDeleteChild {
    #[primary_key]
    #[sortable]
    id: String,
    #[filterable(type = "string")]
    parent_id: String,
    #[relation(
        target = "CaseDeleteParent",
        from = "parent_id",
        to = "id",
        on_delete = "cascade"
    )]
    parent: Option<CaseDeleteParent>,
}

schema_roots! { backend: "postgres", query_custom_ops: [], entities: [CaseDeleteParent, CaseDeleteChild] }

#[tokio::test]
#[ignore = "requires test-owned PostgreSQL container"]
async fn mixed_case_dependent_delete_removes_parent_and_child()
-> Result<(), Box<dyn std::error::Error>> {
    let pool = owned_postgres::OwnedPostgres::pool("table-case-dependent", 5).await?;
    let db = Database::<PostgresBackend>::new(pool.clone());
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "dependent",
            "create",
            &[CaseDeleteParent::metadata(), CaseDeleteChild::metadata()],
        )
        .await?;
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    sqlx::raw_sql("INSERT INTO MixedDeleteParents VALUES ('parent', 'parent'); INSERT INTO MixedDeleteChildren VALUES ('child', 'parent')").execute(&pool).await?;
    let registry =
        StaticDependentDeletes::<PostgresBackend>::new(RuntimeDependentLimits::new(10, 4).unwrap())
            .register::<CaseDeleteParent>()
            .unwrap()
            .register::<CaseDeleteChild>()
            .unwrap();
    let schema = schema_builder(db)
        .data(registry)
        .data(AuthSubject::new("test-principal"))
        .data(DbAuthContext {
            user_id: Some("test-principal".into()),
            ..Default::default()
        })
        .finish();
    let response = schema
        .execute("mutation { deleteCaseDeleteParent(id: \"parent\") { success } }")
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    for table in ["mixeddeleteparents", "mixeddeletechildren"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 0);
    }
    Ok(())
}
