// One backend per build: the generated code below names no backend.
#![cfg(all(feature = "postgres", not(feature = "sqlite")))]

use std::collections::BTreeSet;

use graphql_orm::prelude::*;

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "pg_constraint_upgrade_records",
    plural = "PgConstraintUpgradeRecords",
    unique_composite = "tenant,external_key",
    index = "status"
)]
#[graphql_orm(conditional_index(
    name = "uidx_pg_constraint_active_external",
    columns = ["external_key"],
    unique = true,
    predicate_field = "status",
    predicate_values = ["ACTIVE", "PENDING"]
))]
#[allow(dead_code)]
struct PgConstraintUpgradeRecord {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[unique]
    reservation_id: String,
    tenant: String,
    external_key: String,
    status: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "pg_constraint_upgrade_additions",
    plural = "PgConstraintUpgradeAdditions"
)]
#[allow(dead_code)]
struct PgConstraintUpgradeAddition {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    note: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "pg_compound_fk_parents",
    plural = "PgCompoundFkParents",
    default_sort = "tenant_key ASC, generation DESC",
    index(
        name = "idx_pg_compound_fk_parents_latest",
        columns = ["tenant_key", "generation"],
        directions = ["asc", "desc"]
    )
)]
#[allow(dead_code)]
struct PgCompoundFkParent {
    #[primary_key]
    tenant_key: String,
    #[primary_key]
    generation: i64,
    payload: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    backend = "postgres",
    table = "pg_compound_fk_children",
    plural = "PgCompoundFkChildren",
    default_sort = "id ASC"
)]
#[allow(dead_code)]
struct PgCompoundFkChild {
    #[primary_key]
    id: String,
    tenant_key: String,
    generation: i64,
    #[graphql(skip)]
    #[relation(
        target = "PgCompoundFkParent",
        from = ["tenant_key", "generation"],
        to = ["tenant_key", "generation"],
        on_delete = "cascade"
    )]
    parent: Option<String>,
}

#[tokio::test]
#[ignore = "creates and owns a disposable loopback-only PostgreSQL 17 container"]
async fn constraint_indexes_are_structural_and_additive_upgrade_is_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = super::owned_postgres::OwnedPostgres::start_ready(
        "pg-index",
        super::owned_postgres::POSTGRES_IMAGE,
    )
    .await?;
    let database = Database::<PostgresBackend>::connect_postgres(&owned.url).await?;
    let initial_entities = [PgConstraintUpgradeRecord::metadata()];
    let initial = database
        .schema()
        .plan_migration_to_entities_with_options(
            "pg-constraint-index-v1",
            "initial constraint/index target",
            &initial_entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&initial, ApplyOptions::default())
        .await?;

    let live = introspect_postgres_schema(&database).await?;
    let table = live
        .tables
        .iter()
        .find(|table| table.table_name == "pg_constraint_upgrade_records")
        .ok_or("missing introspected table")?;
    assert!(
        table
            .columns
            .iter()
            .find(|column| column.name == "reservation_id")
            .is_some_and(|column| column.is_unique)
    );
    assert!(
        !table
            .columns
            .iter()
            .find(|column| column.name == "tenant")
            .is_some_and(|column| column.is_unique)
    );
    assert!(
        !table
            .columns
            .iter()
            .find(|column| column.name == "external_key")
            .is_some_and(|column| column.is_unique)
    );
    assert_eq!(
        table.composite_unique_indexes,
        vec![vec!["tenant".to_string(), "external_key".to_string()]]
    );
    let target = SchemaModel::from_entities(&initial_entities);
    let expected_indexes = target.tables[0]
        .indexes
        .iter()
        .map(|index| index.name)
        .collect::<BTreeSet<_>>();
    let live_indexes = table
        .indexes
        .iter()
        .map(|index| index.name)
        .collect::<BTreeSet<_>>();
    assert_eq!(live_indexes, expected_indexes);

    let clean = database.schema().plan_migration(
        "pg-constraint-index-clean",
        "unchanged",
        &live,
        &target,
    )?;
    assert!(
        clean.steps.is_empty(),
        "unchanged target must be idempotent"
    );

    let upgraded_entities = [
        PgConstraintUpgradeRecord::metadata(),
        PgConstraintUpgradeAddition::metadata(),
    ];
    let additive = database
        .schema()
        .plan_migration_to_entities_with_options(
            "pg-constraint-index-v2",
            "add one generated table",
            &upgraded_entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(
        additive
            .steps
            .iter()
            .all(|step| !matches!(step.step, MigrationStep::DropIndex { .. }))
    );
    database
        .schema()
        .apply_migration(&additive, ApplyOptions::default())
        .await?;
    let final_live = introspect_postgres_schema(&database).await?;
    let final_table = final_live
        .tables
        .iter()
        .find(|table| table.table_name == "pg_constraint_upgrade_records")
        .ok_or("missing retained table")?;
    assert_eq!(
        final_table.composite_unique_indexes,
        table.composite_unique_indexes
    );
    assert_eq!(
        final_table
            .indexes
            .iter()
            .map(|index| index.name)
            .collect::<BTreeSet<_>>(),
        expected_indexes
    );

    let compound_entities = [
        PgCompoundFkParent::metadata(),
        PgCompoundFkChild::metadata(),
    ];
    let compound_plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "pg-compound-fk-v1",
            "compound foreign-key target",
            &compound_entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(compound_plan.statements.iter().any(|statement| {
        statement.contains(
            "FOREIGN KEY (tenant_key, generation) REFERENCES pg_compound_fk_parents(tenant_key, generation) ON DELETE CASCADE",
        )
    }));
    database
        .schema()
        .apply_migration(&compound_plan, ApplyOptions::default())
        .await?;

    let compound_live = introspect_postgres_schema(&database).await?;
    let child = compound_live
        .tables
        .iter()
        .find(|table| table.table_name == "pg_compound_fk_children")
        .ok_or("missing PostgreSQL compound child")?;
    assert_eq!(child.foreign_keys.len(), 1);
    assert_eq!(child.foreign_keys[0].column_pairs.len(), 2);
    assert!(child.foreign_keys[0].constraint_name.is_some());
    let parent = compound_live
        .tables
        .iter()
        .find(|table| table.table_name == "pg_compound_fk_parents")
        .ok_or("missing PostgreSQL compound parent")?;
    let latest = parent
        .indexes
        .iter()
        .find(|index| index.name == "idx_pg_compound_fk_parents_latest")
        .ok_or("missing PostgreSQL directional index")?;
    assert_eq!(
        (0..latest.columns.len())
            .map(|position| latest.direction_at(position))
            .collect::<Vec<_>>(),
        [IndexDirection::Asc, IndexDirection::Desc]
    );
    let compound_restart = database
        .schema()
        .plan_migration_to_entities_with_options(
            "pg-compound-fk-v2",
            "compound foreign-key restart",
            &compound_entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    assert!(
        compound_restart.steps.is_empty(),
        "PostgreSQL compound schema drifted: {compound_restart:#?}"
    );

    graphql_orm::sqlx::query(
        "INSERT INTO pg_compound_fk_parents (tenant_key, generation, payload) VALUES ($1, $2, $3)",
    )
    .bind("tenant-a")
    .bind(1_i64)
    .bind("payload")
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO pg_compound_fk_children (id, tenant_key, generation) VALUES ($1, $2, $3)",
    )
    .bind("matching")
    .bind("tenant-a")
    .bind(1_i64)
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query(
        "INSERT INTO pg_compound_fk_children (id, tenant_key, generation) VALUES ($1, $2, $3)",
    )
    .bind("mismatch")
    .bind("tenant-a")
    .bind(2_i64)
    .execute(database.pool())
    .await
    .expect_err("PostgreSQL compound foreign key must reject a partial match");
    graphql_orm::sqlx::query(
        "DELETE FROM pg_compound_fk_parents WHERE tenant_key = $1 AND generation = $2",
    )
    .bind("tenant-a")
    .bind(1_i64)
    .execute(database.pool())
    .await?;
    let remaining: i64 = graphql_orm::sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_compound_fk_children WHERE id = 'matching'",
    )
    .fetch_one(database.pool())
    .await?;
    assert_eq!(remaining, 0);
    Ok(())
}
