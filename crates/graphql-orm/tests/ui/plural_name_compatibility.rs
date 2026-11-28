use graphql_orm::prelude::*;

#[derive(Clone, Debug, GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "records", plural = "Ignored-Value", plural = "RecordRows2")]
struct Record {
    #[primary_key]
    #[sortable]
    id: String,
}

// The entity derive can use the plural as metadata without synthesizing
// bulk mutation result identifiers.
#[derive(Clone, Debug, GraphQLEntity)]
#[graphql_entity(table = "entries", plural = "Entries-ByDay")]
struct Entry {
    #[primary_key]
    #[sortable]
    id: String,
}

fn main() {
    let _: Option<UpdateRecordRows2Result> = None;
    let _: Option<DeleteRecordRows2Result> = None;
    assert_eq!(<Entry as DatabaseEntity>::PLURAL_NAME, "Entries-ByDay");
}
