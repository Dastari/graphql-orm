use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;

/// A handwritten entity.
#[derive(SimpleObject, GraphQLSemanticObject)]
#[graphql_orm(classification = "internal")]
struct Widget {
    /// Public widget identity.
    id: String,
}

#[derive(Default)]
struct Queries;

#[graphql_orm_custom_operations(kind = "query")]
#[graphql_orm::async_graphql::Object]
impl Queries {
    /// Resolves a widget without a context to check the request with.
    #[graphql(entity)]
    async fn find_widget_by_id(&self, id: String) -> Widget {
        Widget { id }
    }
}

fn main() {}
