// Handwritten fields on a generated object: the impl may precede the entity,
// name it by path, or sit in a function body beside it.
#![allow(dead_code)]
use graphql_orm::async_graphql::{Context, Result};
use graphql_orm::prelude::*;

#[graphql_object_fields]
impl Before {
    /// Length of the name.
    async fn name_length(&self) -> Result<i32> {
        Ok(self.name.len() as i32)
    }
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "befores", plural = "Befores")]
#[graphql_orm(object_fields)]
struct Before {
    #[primary_key]
    #[sortable]
    id: String,
    name: String,
}

mod records {
    use graphql_orm::prelude::*;

    #[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql(rename_fields = "PascalCase")]
    #[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
    #[graphql_orm(object_fields)]
    pub struct Record {
        #[primary_key]
        #[sortable]
        pub id: String,
        pub name: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub version: i64,
    }
}

#[graphql_object_fields]
#[allow(clippy::unused_async)]
#[cfg(all())]
impl records::Record {
    /// A borrowed value, a context and an argument.
    #[graphql_orm(read_policy = "records.label", subscribe = true)]
    async fn label<'a>(&'a self, ctx: &Context<'_>, suffix: Option<String>) -> Result<&'a str> {
        let _ = (ctx, suffix);
        Ok(&self.name)
    }

    #[graphql(skip)]
    fn helper(&self) -> usize {
        self.name.len()
    }
}

fn main() {
    #[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[graphql_entity(backend = "sqlite", table = "locals", plural = "Locals")]
    #[graphql_orm(object_fields)]
    struct Local {
        #[primary_key]
        #[sortable]
        id: String,
    }

    #[graphql_object_fields]
    impl Local {
        async fn constant(&self) -> Result<bool> {
            Ok(true)
        }
    }

    assert_eq!(records::Record::computed_object_fields()[0].api_name, "Label");
    assert_eq!(Before::computed_object_fields()[0].api_name, "nameLength");
    assert_eq!(Local::computed_object_fields()[0].description, "Constant");
}
