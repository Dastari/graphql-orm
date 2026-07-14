use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "ordinary_events", plural = "OrdinaryEvents", append_only = true)]
struct OrdinaryEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
}

async fn wrong_entity(database: &Database<SqliteBackend>) {
    let _ = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<OrdinaryEvent>(
                        OrdinaryEventWhereInput::default(),
                        MutationLimit::new(1)?,
                    )
                    .await
                    .map_err(Into::into)
            })
        })
        .await;
}

fn main() {}
