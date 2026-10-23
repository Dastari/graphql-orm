#![cfg(feature = "mssql")]

use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.ResolverMetadataRecords",
    plural = "ResolverMetadataRecords",
    schema_policy = "external_read_only"
)]
struct ResolverMetadataRecord {
    #[primary_key]
    #[graphql_orm(db_column = "RecordId")]
    id: i32,

    #[graphql_orm(db_column = "DisplayName")]
    #[filterable(type = "string")]
    #[sortable]
    display_name: String,
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_read_only",
    entities: [ResolverMetadataRecord],
}

#[test]
fn mssql_metadata_contains_queries_and_trusted_subscriptions_without_mutations() {
    let generated = ResolverMetadataRecord::generated_graphql_operations();
    assert_eq!(generated.len(), 3);
    assert!(generated.iter().all(|operation| {
        operation.backend() == "mssql" && operation.kind() != GraphqlOperationKind::Mutation
    }));

    let catalog = graphql_orm_operation_catalog();
    assert_eq!(catalog.exposed_operations().count(), 3);
    assert!(
        catalog
            .operations()
            .iter()
            .all(GraphqlResolverOperationDescriptor::is_exposed)
    );
}
