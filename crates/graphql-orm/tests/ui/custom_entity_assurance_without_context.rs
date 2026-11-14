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

// Public, but its catalogued coordinate can still be classified with an
// assurance requirement, which only its body can enforce.
#[graphql_orm_custom_operations(kind = "query", authorization = false)]
#[graphql_orm::async_graphql::Object]
impl Queries {
    /// Resolves a widget without a context to check the request with.
    #[graphql(entity)]
    async fn find_widget_by_id(&self, id: String) -> Widget {
        Widget { id }
    }
}

fn main() {}
