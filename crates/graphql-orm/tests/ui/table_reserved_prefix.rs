use graphql_orm::prelude::*;

// Table names starting with `__gom_` are reserved for the row aliases and
// common table expressions that generated read-policy SQL renders: a table
// of that name could take an alias's place, or be shadowed by a closure's
// expression (#796).
#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "__gom_row_1", plural = "AliasRows")]
struct AliasRow {
    #[primary_key]
    #[sortable]
    pub id: String,
}

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "main.\"__GOM_Closure_1\"", plural = "ClosureRows")]
struct ClosureRow {
    #[primary_key]
    #[sortable]
    pub id: String,
}

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "__gom_orders", plural = "OrderRows")]
struct OrderRow {
    #[primary_key]
    #[sortable]
    pub id: String,
}

fn main() {}
