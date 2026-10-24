//! Generated identifiers longer than PostgreSQL's 63-byte limit are shortened
//! deterministically, so plan -> apply -> re-plan is empty on every backend.
//! Explicitly declared names over the limit are refused when a PostgreSQL plan
//! would create them; SQLite keeps accepting them. PostgreSQL cases own a
//! verified disposable server.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::{graphql::orm::*, prelude::*};

#[cfg(feature = "sqlite")]
type Backend = SqliteBackend;
#[cfg(feature = "postgres")]
type Backend = PostgresBackend;

const GENERATED_TABLE: &str = "identifier_length_provider_webhook_receipts";
#[cfg(feature = "postgres")]
const DECLARED_TABLE: &str = "identifier_length_declared_receipts";
const DECLARED_INDEX: &str =
    "idx_identifier_length_declared_receipts_provider_kind_provider_response_id";

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "identifier_length_provider_webhook_receipts",
    plural = "IdentifierLengthProviderWebhookReceipts",
    index = "provider_kind,provider_profile_id,provider_response_id,state"
)]
#[allow(dead_code)]
struct GeneratedReceipt {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    provider_kind: String,
    provider_profile_id: String,
    provider_response_id: String,
    state: String,
}

#[derive(GraphQLSchemaEntity, Clone, Debug)]
#[graphql_entity(
    table = "identifier_length_declared_receipts",
    plural = "IdentifierLengthDeclaredReceipts",
    index(
        name = "idx_identifier_length_declared_receipts_provider_kind_provider_response_id",
        columns = ["provider_kind", "provider_response_id"]
    )
)]
#[allow(dead_code)]
struct DeclaredReceipt {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    provider_kind: String,
    provider_response_id: String,
}

#[cfg(feature = "sqlite")]
async fn database(_label: &str) -> graphql_orm::Result<Database<Backend>> {
    Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await
}

#[cfg(feature = "postgres")]
async fn database(label: &str) -> graphql_orm::Result<Database<Backend>> {
    let pool = owned_postgres::OwnedPostgres::pool(label, 5)
        .await
        .map_err(|error| graphql_orm::Error::Configuration(error.to_string().into()))?;
    Ok(Database::<PostgresBackend>::new(pool))
}

#[cfg(feature = "sqlite")]
async fn introspect(database: &Database<Backend>) -> graphql_orm::Result<SchemaModel> {
    introspect_sqlite_schema(database).await
}

#[cfg(feature = "postgres")]
async fn introspect(database: &Database<Backend>) -> graphql_orm::Result<SchemaModel> {
    introspect_postgres_schema(database).await
}

fn generated_index_name(schema: &SchemaModel) -> String {
    let table = schema
        .tables
        .iter()
        .find(|table| table.table_name == GENERATED_TABLE)
        .expect("generated receipt table");
    let names = table
        .indexes
        .iter()
        .filter(|index| index.columns.len() == 4)
        .map(|index| index.name.to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 1, "{names:?}");
    names[0].clone()
}

fn index_changes(steps: &[PlannedMigrationStep]) -> Vec<&MigrationStep> {
    steps
        .iter()
        .map(|step| &step.step)
        .filter(|step| {
            matches!(
                step,
                MigrationStep::CreateIndex { .. } | MigrationStep::DropIndex { .. }
            )
        })
        .collect()
}

#[test]
fn generated_names_over_the_limit_are_bounded_and_stable() {
    let target = SchemaModel::from_entities(&[GeneratedReceipt::metadata()]);
    let name = generated_index_name(&target);
    assert_eq!(name.len(), 63);
    assert!(name.starts_with("idx_identifier_length_provider_webhook_rec"));
    assert_eq!(
        generated_index_name(&SchemaModel::from_entities(&[GeneratedReceipt::metadata()])),
        name
    );
    // Declared names are never rewritten.
    let declared = SchemaModel::from_entities(&[DeclaredReceipt::metadata()]);
    assert!(
        declared.tables[0]
            .indexes
            .iter()
            .any(|index| index.name == DECLARED_INDEX)
    );
    assert!(DECLARED_INDEX.len() > 63);
}

#[tokio::test]
async fn generated_long_index_replans_empty_through_ordinary_plans() -> graphql_orm::Result<()> {
    let database = database("identifier-length-ordinary").await?;
    let entities = [GeneratedReceipt::metadata()];
    let target = SchemaModel::from_entities(&entities);
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "identifier-length-v1",
            "generated index",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let live = introspect(&database).await?;
    assert_eq!(generated_index_name(&live), generated_index_name(&target));
    let replan =
        database
            .schema()
            .plan_migration("identifier-length-v2", "replan", &live, &target)?;
    assert!(
        index_changes(&replan.steps).is_empty(),
        "{:?}",
        replan.statements
    );
    Ok(())
}

#[tokio::test]
async fn generated_long_index_replans_empty_through_owned_plans() {
    let database = database("identifier-length-owned").await.unwrap();
    let target =
        OwnedSchemaModel::from(&SchemaModel::from_entities(&[GeneratedReceipt::metadata()]));
    let ownership = ManagedTableSet::new([GENERATED_TABLE.into()]).unwrap();
    let plan = database
        .schema()
        .plan_owned_migration(
            "v1",
            "generated",
            &target,
            &ownership,
            PlanOptions::strict(),
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_owned_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    let replan = database
        .schema()
        .plan_owned_migration("v2", "replan", &target, &ownership, PlanOptions::strict())
        .await
        .unwrap();
    assert!(replan.steps().is_empty(), "{:?}", replan.statements());
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn postgres_refuses_declared_names_it_would_truncate() {
    let database = database("identifier-length-declared").await.unwrap();
    let entities = [DeclaredReceipt::metadata()];
    let error = database
        .schema()
        .plan_migration_to_entities_with_options(
            "identifier-length-declared",
            "declared index",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await
        .expect_err("PostgreSQL would truncate the declared index name");
    let message = error.to_string();
    assert!(message.contains(DECLARED_INDEX), "{message}");
    assert!(message.contains("truncates identifiers"), "{message}");

    let target = OwnedSchemaModel::from(&SchemaModel::from_entities(&entities));
    let ownership = ManagedTableSet::new([DECLARED_TABLE.into()]).unwrap();
    let error = database
        .schema()
        .plan_owned_migration("v1", "declared", &target, &ownership, PlanOptions::strict())
        .await
        .expect_err("owned PostgreSQL plans refuse the declared name too");
    let diagnostics = match error {
        RuntimeMigrationError::Diagnostics(diagnostics) => diagnostics,
        other => panic!("unexpected owned planning error: {other:?}"),
    };
    assert_eq!(
        diagnostics.0[0].code,
        RuntimeMigrationDiagnosticCode::InvalidPhysicalContract
    );
    assert_eq!(diagnostics.0[0].subject.as_deref(), Some(DECLARED_INDEX));
    assert!(
        introspect(&database)
            .await
            .unwrap()
            .tables
            .iter()
            .all(|table| table.table_name != DECLARED_TABLE)
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_keeps_accepting_long_declared_names() -> graphql_orm::Result<()> {
    let database = database("identifier-length-declared").await?;
    let entities = [DeclaredReceipt::metadata()];
    let target = SchemaModel::from_entities(&entities);
    let plan = database
        .schema()
        .plan_migration_to_entities_with_options(
            "identifier-length-declared",
            "declared index",
            &entities,
            PlanOptions::managed_tables_only(),
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let live = introspect(&database).await?;
    assert!(
        live.tables
            .iter()
            .flat_map(|table| table.indexes.iter())
            .any(|index| index.name == DECLARED_INDEX)
    );
    let replan = database.schema().plan_migration(
        "identifier-length-declared-2",
        "replan",
        &live,
        &target,
    )?;
    assert!(
        index_changes(&replan.steps).is_empty(),
        "{:?}",
        replan.statements
    );
    Ok(())
}
