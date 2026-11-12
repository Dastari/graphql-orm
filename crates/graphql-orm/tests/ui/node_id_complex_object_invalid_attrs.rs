// The shadowing guard must read every name, so it refuses a non-literal name
// and a `#[graphql(...)]` attribute it cannot parse instead of skipping them.
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

macro_rules! entity {
    ($name:ident, $table:literal, $plural:literal) => {
        #[derive(GraphQLEntity, GraphQLOperations, SimpleObject, Clone, Debug, serde::Serialize, serde::Deserialize)]
        #[graphql(complex)]
        #[graphql_orm(compose_complex_object)]
        #[graphql_entity(backend = "sqlite", table = $table, plural = $plural, node_id)]
        struct $name {
            #[primary_key]
            #[sortable]
            id: String,
            name: String,
        }
    };
}

entity!(Constant, "constant_records", "ConstantRows");
entity!(Malformed, "malformed_records", "MalformedRows");

const FIELD: &str = "NodeId";

#[graphql_complex_object]
impl Constant {
    #[graphql(name = FIELD)]
    async fn handwritten(&self) -> String {
        self.id.clone()
    }
}

#[graphql_complex_object]
impl Malformed {
    #[graphql(name = "label" "extra")]
    async fn handwritten(&self) -> String {
        self.id.clone()
    }
}

fn main() {}
