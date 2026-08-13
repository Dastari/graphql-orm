use graphql_orm::prelude::*;

/// Project-neutral externally managed work record used by the coordinated
/// capability fixture.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    backend = "mssql",
    table = "dbo.WorkRecords",
    plural = "WorkRecords",
    schema_policy = "external_writable",
    default_sort = "id ASC",
    aggregate = true,
    auth = "required",
    read_policy = "work_records.read",
    write_policy = "work_records.write"
)]
pub struct WorkRecord {
    /// Stable application-owned record identity.
    #[primary_key]
    #[filterable(type = "string")]
    #[sortable]
    pub id: String,
    /// Optional team grouping key.
    #[filterable(type = "string")]
    #[sortable]
    pub team: Option<String>,
    /// Integral work units.
    #[filterable(type = "number")]
    pub units: i64,
    /// Exact work cost.
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    pub cost: graphql_orm::rust_decimal::Decimal,
}

schema_roots! {
    backend: "mssql",
    schema_policy: "external_writable",
    query_custom_ops: [],
    entities: [WorkRecord],
}

/// Returns the canonical public semantic graph emitted beside the schema.
pub fn semantic_catalog() -> &'static GraphqlSemanticCatalog {
    graphql_orm_semantic_catalog()
}

/// Builds a typed, bounded grouped multi-sum without accepting SQL fragments.
pub async fn grouped_totals(
    database: &Database<MssqlBackend>,
) -> graphql_orm::Result<Vec<AggregateResultRow>> {
    WorkRecord::aggregate(database)
        .group_by(WorkRecordAggregateField::Team)?
        .count_rows()?
        .sum(WorkRecordAggregateField::Units)?
        .sum(WorkRecordAggregateField::Cost)?
        .group_limit(25)?
        .fetch()
        .await
}

/// Opens only the deliberate writable SQL Server access mode.
pub async fn connect_external_writable(
    connection_string: &str,
) -> graphql_orm::Result<Database<MssqlBackend>> {
    Database::<MssqlBackend>::connect_ado_external_writable(connection_string).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_catalog_includes_the_opted_in_aggregate_root() {
        let catalog = semantic_catalog();
        catalog.validate().expect("fixture semantics validate");
        let entity = catalog
            .entities
            .iter()
            .find(|entity| entity.entity_name == "WorkRecord")
            .expect("work record semantic entity");
        assert!(
            entity
                .fields
                .iter()
                .any(|field| field.description == "Optional team grouping key.")
        );
        assert!(catalog.operations.iter().any(|operation| {
            operation.kind == GraphqlOperationKind::Query
                && operation.generated_category
                    == Some(GeneratedGraphqlOperationCategory::Aggregate)
        }));
    }
}
