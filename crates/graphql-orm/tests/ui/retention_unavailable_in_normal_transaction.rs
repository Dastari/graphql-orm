use graphql_orm::prelude::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "retained_events",
    plural = "RetainedEvents",
    append_only = true,
    retention_purge = "events.purge"
)]
struct RetainedEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
}

async fn ordinary_transaction(database: &Database<SqliteBackend>) {
    let _ = database
        .transaction(TransactionMode::StateMachine, |ordinary| {
            Box::pin(async move {
                ordinary
                    .purge::<RetainedEvent>(
                        RetainedEventWhereInput::default(),
                        MutationLimit::new(1)?,
                    )
                    .await
                    .map_err(Into::into)
            })
        })
        .await;
}

fn main() {}
