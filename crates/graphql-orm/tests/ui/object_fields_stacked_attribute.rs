// An attribute below `#[graphql_object_fields]` would be applied to the
// generated object after its resolvers were given their field check, where an
// attribute macro could append a resolver that has none. Only lint levels and
// a whole-impl `cfg` are accepted there; `cfg_attr` is refused because what
// it expands to is not known.
use graphql_orm::async_graphql::Result;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

// Stands for any attribute macro that rewrites the impl it is given.
#[graphql_object_fields]
#[graphql_orm::graphql_complex_object]
impl Record {
    async fn one(&self) -> Result<i32> {
        Ok(1)
    }
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "others", plural = "Others")]
#[graphql_orm(object_fields)]
struct Other {
    #[primary_key]
    #[sortable]
    id: String,
}

#[graphql_object_fields]
#[allow(clippy::unused_async)]
#[cfg_attr(all(), graphql_orm::graphql_complex_object)]
impl Other {
    async fn one(&self) -> Result<i32> {
        Ok(1)
    }
}

fn main() {}
