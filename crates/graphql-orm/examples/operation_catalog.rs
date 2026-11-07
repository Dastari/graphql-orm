//! Inspect and serialize operation metadata emitted by `schema_roots!`.

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "operation_catalog_accounts",
    plural = "OperationCatalogAccounts",
    schema_policy = "managed"
)]
struct OperationCatalogAccount {
    #[primary_key]
    id: String,

    #[filterable(type = "string")]
    #[sortable]
    label: String,
}

schema_roots! {
    backend: "sqlite",
    schema_policy: "managed",
    auth: "none",
    entities: [OperationCatalogAccount],
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let generated =
        <OperationCatalogAccount as GraphqlOperationMetadata>::generated_graphql_operations();
    assert!(!generated.is_empty(), "the derive should emit operations");
    let catalog = graphql_orm_operation_catalog();
    let operations = catalog
        .operations()
        .iter()
        .map(|operation| {
            serde_json::json!({
                "root_type": operation.root_type(),
                "field_name": operation.field_name(),
                "kind": operation.kind().as_str(),
                "category": operation.category().as_str(),
                "exposed": operation.is_exposed(),
                "fingerprint": operation.fingerprint(),
                "authorization_fingerprint": operation.authorization_fingerprint(),
                "router_export_fingerprint": operation.router_export_fingerprint(),
            })
        })
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "operation_fingerprint_algorithm": GRAPHQL_OPERATION_FINGERPRINT_ALGORITHM,
        "authorization_fingerprint_algorithm": GRAPHQL_AUTHORIZATION_FINGERPRINT_ALGORITHM,
        "router_export_fingerprint_algorithm": GRAPHQL_ROUTER_EXPORT_FINGERPRINT_ALGORITHM,
        "catalog_fingerprint": catalog.fingerprint(),
        "operations": operations,
    });

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_catalog_serializes() {
        super::run().expect("generated catalog example should run");
    }
}
