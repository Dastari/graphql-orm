#![cfg(all(feature = "postgres", not(feature = "sqlite")))]

use graphql_orm::prelude::*;
use graphql_orm::sqlx::Row;
use std::sync::Arc;

async fn database() -> Option<Database<PostgresBackend>> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    Database::<PostgresBackend>::connect_postgres(url)
        .await
        .ok()
}

#[tokio::test]
async fn state_machine_is_serializable_auth_local_and_conflicts_are_retryable()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(database) = database().await else {
        eprintln!("skipping live transaction test: TEST_DATABASE_URL is not set");
        return Ok(());
    };
    graphql_orm::sqlx::query("DROP TABLE IF EXISTS graphql_orm_transaction_state")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE graphql_orm_transaction_state (id BIGINT PRIMARY KEY, value BIGINT NOT NULL)",
    )
    .execute(database.pool())
    .await?;
    graphql_orm::sqlx::query("INSERT INTO graphql_orm_transaction_state (id, value) VALUES (1, 0)")
        .execute(database.pool())
        .await?;

    let auth = DbAuthContext::from_parts(
        "transaction-user",
        vec!["writer".to_string()],
        vec!["state.write".to_string()],
        Some("tenant-transaction".to_string()),
    );
    let settings = database
        .transaction_with_auth(TransactionMode::StateMachine, Some(&auth), |tx| {
            Box::pin(async move {
                let row = graphql_orm::sqlx::query(
                    "SELECT current_setting('transaction_isolation') AS isolation,
                            current_setting('app.user_id', true) AS user_id,
                            current_setting('app.tenant_id', true) AS tenant_id",
                )
                .fetch_one(tx.executor())
                .await?;
                Ok::<_, OrmPublicError>((
                    row.try_get::<String, _>("isolation")?,
                    row.try_get::<String, _>("user_id")?,
                    row.try_get::<String, _>("tenant_id")?,
                ))
            })
        })
        .await?;
    assert_eq!(settings.0, "serializable");
    assert_eq!(settings.1, "transaction-user");
    assert_eq!(settings.2, "tenant-transaction");

    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let run = |database: Database<PostgresBackend>, barrier: Arc<tokio::sync::Barrier>| async move {
        database
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    let row = graphql_orm::sqlx::query(
                        "SELECT value FROM graphql_orm_transaction_state WHERE id = 1",
                    )
                    .fetch_one(tx.executor())
                    .await?;
                    let value: i64 = row.try_get("value")?;
                    barrier.wait().await;
                    graphql_orm::sqlx::query(
                        "UPDATE graphql_orm_transaction_state SET value = $1 WHERE id = 1",
                    )
                    .bind(value + 1)
                    .execute(tx.executor())
                    .await?;
                    Ok::<_, OrmPublicError>(())
                })
            })
            .await
    };
    let (left, right) = tokio::join!(
        run(database.clone(), barrier.clone()),
        run(database.clone(), barrier)
    );
    let outcomes = [left, right];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let conflict = outcomes
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one serializable transaction conflicts");
    assert!(conflict.is_retryable());
    assert_eq!(
        conflict.public_error().code,
        OrmErrorCode::ServiceUnavailable
    );
    assert!(conflict.public_error().is_retryable());

    graphql_orm::sqlx::query("DROP TABLE graphql_orm_transaction_state")
        .execute(database.pool())
        .await?;
    Ok(())
}
