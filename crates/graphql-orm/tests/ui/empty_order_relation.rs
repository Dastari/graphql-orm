use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", keyset = "id asc")]
struct Record {
    #[primary_key]
    id: String,
    name: String,
}


impl BatchLoadEntity for Record {
    fn batch_column() -> &'static str { "id" }
    fn batch_key_from_row(row: &graphql_orm::DbRow) -> Result<String, graphql_orm::sqlx::Error> {
        use graphql_orm::sqlx::Row;
        row.try_get("id")
    }
}


#[derive(GraphQLEntity, GraphQLRelations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "parents", schema_only = true)]
struct Parent {
    #[primary_key]
    id: String,
    #[graphql(skip)]
    #[serde(default)]
    #[relation(target = "Record", from = "id", to = "id", multiple, emit_fk = false)]
    records: Vec<Record>,
}

fn main() {}
