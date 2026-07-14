use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "retained_records",
    plural = "RetainedRecords",
    append_only = true,
    retention_purge = "retained_record.purge"
)]
struct RetainedRecord {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    #[sortable]
    status: String,
}

async fn purge(database: &Database<SqliteBackend>, status: String) {
    let _ = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedRecord>(
                        RetainedRecordWhereInput {
                            status: Some(StringFilter {
                                eq: Some(status),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        MutationLimit::new(100)?,
                    )
                    .await
                    .map_err(Into::into)
            })
        })
        .await;
}

fn main() {}
