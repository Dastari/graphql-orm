// A persisted column without a generated getter (`read = false`, `private`)
// still owns its GraphQL name: generated read paths decide that name by the
// column's policy. A handwritten field may not take it, by derived or by
// explicit name.
use graphql_orm::async_graphql::Result;
use graphql_orm::prelude::*;

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "records", plural = "Records")]
#[graphql_orm(object_fields)]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(read = false)]
    status: String,
}

#[graphql_object_fields]
impl Record {
    #[graphql_orm(read_policy = "records.status")]
    async fn status(&self) -> Result<Option<String>> {
        Ok(Some(self.status.to_uppercase()))
    }
}

#[derive(GraphQLEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(backend = "sqlite", table = "others", plural = "Others")]
#[graphql_orm(object_fields)]
struct Other {
    #[primary_key]
    #[sortable]
    id: String,
    #[graphql_orm(private)]
    custody_note: String,
}

#[graphql_object_fields]
impl Other {
    #[graphql(name = "CustodyNote")]
    async fn note(&self) -> Result<Option<String>> {
        Ok(Some(self.custody_note.clone()))
    }
}

fn main() {}
