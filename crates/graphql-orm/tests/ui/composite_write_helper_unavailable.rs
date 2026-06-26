use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug)]
#[graphql_entity(
    backend = "sqlite",
    table = "composite_write_records",
    plural = "CompositeWriteRecords",
    default_sort = "tenant_id ASC, local_id ASC"
)]
struct CompositeWriteRecord {
    #[primary_key]
    #[sortable]
    pub tenant_id: i32,

    #[primary_key]
    #[sortable]
    pub local_id: i32,

    pub name: String,
}

fn main() {
    let _ = CompositeWriteRecord::create;
    let _ = CompositeWriteRecord::update_by_id;
}
