use graphql_orm::prelude::*;

async fn escape(database: &Database<SqliteBackend>) {
    let _ = database
        .retention_transaction(|maintenance| Box::pin(async move { Ok(maintenance) }))
        .await;
}

fn main() {}
