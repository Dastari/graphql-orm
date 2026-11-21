mod model {
    use graphql_orm::prelude::*;
    include!("../../../graphql-orm-macros/tests/ui/query_hygiene_entity.inc");
}

// Exercise the actual generated query surface in a hostile namespace. The
// model supplies the shared entity and repository machinery.
#[allow(dead_code, unused_imports)]
mod shadowed {
    use super::model::*;

    struct Option;
    struct Some;
    struct None;
    struct Ok;
    struct Err;
    struct Vec;
    struct Box;
    struct String;
    struct Result;
    struct Send;
    struct Sync;
    struct Into;
    struct IntoIterator;
    mod std {}

    include!("../../../graphql-orm-macros/tests/ui/query_hygiene_resolvers.inc");
}

fn main() {
    let _ = shadowed::RecordQueries;
    graphql_orm::async_graphql::Schema::build(
        model::RecordQueries,
        graphql_orm::async_graphql::EmptyMutation,
        graphql_orm::async_graphql::EmptySubscription,
    )
    .enable_federation()
    .finish();
}
