#[cfg(feature = "sqlite")]
use graphql_orm::prelude::*;

#[cfg(feature = "sqlite")]
#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "assurance_example_records",
    plural = "AssuranceExampleRecords"
)]
struct AssuranceExampleRecord {
    #[primary_key]
    #[filterable(type = "uuid")]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
}

#[cfg(feature = "sqlite")]
schema_roots! {
    backend: "sqlite",
    entities: [AssuranceExampleRecord],
}

#[cfg(feature = "sqlite")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AssuranceSchemaConfig::legacy()
        .with_default_interactive_mutation_policy("interactive.recent-auth")?
        .with_strict_mutation_classification(true);
    let registry =
        OperationAssuranceRegistry::builder(graphql_orm_operation_catalog(), config).build()?;

    registry.audit().assert_complete();
    println!("{}", registry.manifest().to_json_pretty()?);
    for metadata in registry.schema_metadata() {
        if let Some(directive) = metadata.directive {
            println!("{} {directive}", metadata.field_coordinate);
        }
    }

    // The manifest is advisory. A server must also install AssuranceEnforcement
    // so each protected resolver evaluates the current request before work.
    Ok(())
}

#[cfg(not(feature = "sqlite"))]
fn main() {}
