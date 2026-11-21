mod model {
    use graphql_orm::prelude::*;
    include!("../../../graphql-orm-macros/tests/ui/query_hygiene_entity.inc");

    // Exercise the actual generated query surface in a hostile namespace. The
    // model supplies the shared entity and repository machinery.
    #[allow(dead_code, unused_imports)]
    mod shadowed {
        use super::*;

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
        mod serde {}

        include!("../../../graphql-orm-macros/tests/ui/query_hygiene_resolvers.inc");
    }

    pub fn check_shadowed_roots() {
        let _ = (shadowed::RecordQueries, shadowed::RecordMutations, shadowed::RecordSubscriptions);
    }
}

fn main() {
    model::check_shadowed_roots();
    graphql_orm::async_graphql::Schema::build(
        model::RecordQueries,
        model::RecordMutations,
        model::RecordSubscriptions,
    )
    .enable_federation()
    .finish();
}
