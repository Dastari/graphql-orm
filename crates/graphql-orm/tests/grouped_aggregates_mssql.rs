#![cfg(all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))))]

use graphql_orm::prelude::*;
use graphql_orm::rust_decimal::Decimal;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.grouped_aggregate_rows",
    plural = "GroupedAggregateRows",
    schema_policy = "external_read_only",
    aggregate = true,
    auth = "none"
)]
struct GroupedAggregateMssqlRow {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    team: Option<String>,
    units: i64,
    hours: f64,
    #[filterable(type = "decimal")]
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    amount: Decimal,
}

schema_roots! {
    backend: "mssql",
    query_custom_ops: [],
    entities: [GroupedAggregateMssqlRow],
}

#[test]
fn mssql_aggregate_surface_is_capability_driven_and_compiles() {
    let descriptor = GroupedAggregateMssqlRow::generated_graphql_operations()
        .iter()
        .find(|operation| operation.category() == GeneratedGraphqlOperationCategory::Aggregate)
        .expect("opted-in MSSQL aggregate descriptor");
    assert_eq!(descriptor.kind(), GraphqlOperationKind::Query);
    assert_eq!(descriptor.arguments().len(), 4);
    assert!(
        GroupedAggregateMssqlRowAggregateField::Amount
            .aggregate_field()
            .operators()
            .contains(&GraphqlAggregateOperator::Sum)
    );
}
