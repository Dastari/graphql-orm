#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "records",
    aggregate = true,
    aggregate_source = true,
    keyset = "id asc",
    federation_key(fields = ["id"]),
    node_id
)]
pub struct Record {
    #[primary_key]
    #[sortable]
    pub id: ::std::string::String,
    #[graphql_orm(searchable(weight = "A"))]
    pub title: ::std::string::String,
}
