use graphql_orm::prelude::*;

#[derive(RepositoryEntity, Clone, serde::Serialize, serde::Deserialize)]
#[repository_entity(backend = "sqlite", table = "private_records", plural = "PrivateRecords")]
#[graphql_orm(projection(
    name = "PrivateRecordProjection",
    fields = [id, value],
    private = true
))]
struct PrivateRecord {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
}

fn assert_output<T: async_graphql::OutputType>() {}
fn assert_input<T: async_graphql::InputType>() {}

fn main() {
    assert_output::<PrivateRecord>();
    assert_input::<PrivateRecordWhereInput>();
    assert_input::<PrivateRecordOrderByInput>();
    assert_input::<CreatePrivateRecordInput>();
    assert_input::<UpdatePrivateRecordInput>();
    assert_output::<PrivateRecordProjection>();
    assert_input::<PrivateRecordProjection>();
}
