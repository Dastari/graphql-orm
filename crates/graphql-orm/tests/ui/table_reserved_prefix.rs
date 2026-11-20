use graphql_orm::prelude::*;

// `__gom_self`, `__gom_row_<n>`, `__gom_visibility_<n>` and
// `__gom_closure_<n>` (with any suffix) are the names generated read-policy
// SQL renders: a table of such a name could take an alias's place, or be
// shadowed by a closure's expression (#796). Other `__gom_` names are
// ordinary tables.
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
#[graphql_entity(table = "[__gom_closure_2_t]", plural = "ReachedRows")]
struct ReachedRow {
    #[primary_key]
    #[sortable]
    pub id: String,
}

#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "__gom_self", plural = "SelfRows")]
struct SelfRow {
    #[primary_key]
    #[sortable]
    pub id: String,
}

// Not a generated name: accepted.
#[derive(GraphQLEntity, Clone, Debug)]
#[graphql_entity(table = "__gom_orders", plural = "OrderRows")]
struct OrderRow {
    #[primary_key]
    #[sortable]
    pub id: String,
}

fn main() {}
