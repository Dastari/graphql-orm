use graphql_orm::prelude::*;

mod catalogue {
    use graphql_orm::prelude::*;

    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(backend = "sqlite", table = "collision_items", plural = "Items")]
    pub struct Item {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }
}

mod warehouse {
    use graphql_orm::prelude::*;

    // Another entity with the same plural: both generate the `items` root.
    #[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(backend = "sqlite", table = "stock_items", plural = "Items")]
    pub struct StockItem {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        pub id: String,
    }
}

use catalogue::*;
use warehouse::*;

schema_roots! {
    backend: "sqlite",
    query_custom_ops: [],
    entities: [Item, StockItem],
}

fn main() {}
