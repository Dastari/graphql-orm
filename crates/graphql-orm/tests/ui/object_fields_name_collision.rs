// A handwritten field may not take the name of a generated one, whether the
// name is derived from the method or declared.
use graphql_orm::async_graphql::Result;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    display_name: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[graphql_object_fields]
impl Record {
    async fn display_name(&self) -> Result<String> {
        Ok(self.display_name.to_uppercase())
    }
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "others", plural = "Others")]
#[graphql_orm(object_fields)]
struct Other {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(version = "managed", default = "0")]
    version: i64,
}

#[graphql_object_fields]
impl Other {
    #[graphql(name = "RecordVersion")]
    async fn handwritten_version(&self) -> Result<i64> {
        Ok(self.version)
    }
}

fn main() {}
