// A handwritten field may declare `subscribe` only when it returns a leaf: a
// scalar, an enum, or an `Option` or list of those. A change event is released
// once its selected fields are proved, and nothing proves what an object would
// resolve below the field.
use graphql_orm::async_graphql::{Enum, Result, SimpleObject};
use graphql_orm::prelude::*;

#[derive(SimpleObject)]
struct Outcome {
    label: String,
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Open,
    Closed,
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
}

#[graphql_object_fields]
impl Record {
    // Leaves are accepted.
    #[graphql_orm(subscribe)]
    async fn stage(&self) -> Result<Stage> {
        Ok(Stage::Open)
    }

    #[graphql_orm(subscribe)]
    async fn stages(&self) -> Result<Option<Vec<Stage>>> {
        Ok(None)
    }

    #[graphql_orm(subscribe)]
    async fn label(&self) -> Result<&str> {
        Ok(&self.name)
    }

    // Objects are not.
    #[graphql_orm(subscribe)]
    async fn outcome(&self) -> Result<Option<Outcome>> {
        Ok(None)
    }

    #[graphql_orm(subscribe)]
    async fn outcomes(&self) -> Result<Vec<Outcome>> {
        Ok(Vec::new())
    }
}

fn main() {}
