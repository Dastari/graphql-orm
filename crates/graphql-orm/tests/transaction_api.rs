#![cfg(feature = "sqlite")]

use graphql_orm::db::ConnectionOptions;
use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "transaction_items", plural = "TransactionItems")]
struct TransactionItem {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    value: String,
}

fn input(value: &str) -> CreateTransactionItemInput {
    CreateTransactionItemInput {
        value: value.to_string(),
    }
}

async fn database(max_connections: u32) -> graphql_orm::Result<Database<SqliteBackend>> {
    let database = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        ConnectionOptions::default().max_connections(max_connections),
    )
    .await?;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "transaction-api-init",
            "transaction API test",
            &[TransactionItem::metadata()],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn cancellation_rolls_back_and_reuses_the_connection() -> graphql_orm::Result<()> {
    let database = database(1).await?;
    let sender = database.ensure_event_sender::<u32>();
    let mut events = sender.subscribe();
    let actions = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let task = {
        let database = database.clone();
        let entered = entered.clone();
        let release = release.clone();
        let actions = actions.clone();
        tokio::spawn(async move {
            database
                .transaction(TransactionMode::Default, |tx| {
                    Box::pin(async move {
                        tx.insert::<TransactionItem>(input("cancelled")).await?;
                        tx.queue_event(7_u32);
                        tx.defer(move |_| async move {
                            actions.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, std::convert::Infallible>(())
                        });
                        entered.notify_one();
                        release.notified().await;
                        Ok(())
                    })
                })
                .await
        })
    };
    entered.notified().await;
    task.abort();
    assert!(task.await.expect_err("task was cancelled").is_cancelled());
    assert!(events.try_recv().is_err());
    assert_eq!(actions.load(Ordering::SeqCst), 0);

    tokio::time::timeout(Duration::from_secs(2), async {
        assert_eq!(TransactionItem::count_all(&database).await?, 0);
        TransactionItem::insert(&database, input("connection-reused")).await?;
        graphql_orm::Result::Ok(())
    })
    .await
    .expect("cancelled transaction released its pooled connection")?;
    Ok(())
}

#[tokio::test]
async fn panic_rolls_back_through_transaction_drop() -> graphql_orm::Result<()> {
    let database = database(1).await?;
    let sender = database.ensure_event_sender::<u16>();
    let mut events = sender.subscribe();
    let actions = Arc::new(AtomicUsize::new(0));
    let task = {
        let database = database.clone();
        let actions = actions.clone();
        tokio::spawn(async move {
            let _ = database
                .transaction(TransactionMode::Default, |tx| {
                    Box::pin(async move {
                        tx.insert::<TransactionItem>(input("panic")).await?;
                        tx.queue_event(9_u16);
                        tx.defer(move |_| async move {
                            actions.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, std::convert::Infallible>(())
                        });
                        panic!("intentional transaction callback panic");
                        #[allow(unreachable_code)]
                        Ok::<(), OrmPublicError>(())
                    })
                })
                .await;
        })
    };
    assert!(task.await.expect_err("callback panicked").is_panic());
    assert!(events.try_recv().is_err());
    assert_eq!(actions.load(Ordering::SeqCst), 0);
    assert_eq!(TransactionItem::count_all(&database).await?, 0);
    Ok(())
}

#[tokio::test]
async fn queued_side_effects_run_once_only_after_commit() -> graphql_orm::Result<()> {
    let database = database(1).await?;
    let sender = database.ensure_event_sender::<String>();
    let mut events = sender.subscribe();
    let actions = Arc::new(AtomicUsize::new(0));

    database
        .transaction(TransactionMode::Default, |tx| {
            let actions = actions.clone();
            Box::pin(async move {
                tx.insert::<TransactionItem>(input("commit")).await?;
                tx.queue_event("committed".to_string());
                let deferred_actions = actions.clone();
                tx.defer(move |_| async move {
                    deferred_actions.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, std::convert::Infallible>(())
                });
                assert_eq!(actions.load(Ordering::SeqCst), 0);
                Ok(())
            })
        })
        .await
        .expect("commit succeeds");
    assert_eq!(events.recv().await.expect("one event"), "committed");
    assert!(events.try_recv().is_err());
    assert_eq!(actions.load(Ordering::SeqCst), 1);

    let rejected = database
        .transaction(TransactionMode::Default, |tx| {
            let actions = actions.clone();
            Box::pin(async move {
                tx.queue_event("rolled-back".to_string());
                tx.defer(move |_| async move {
                    actions.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, std::convert::Infallible>(())
                });
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(rejected.is_err());
    assert!(events.try_recv().is_err());
    assert_eq!(actions.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn state_machine_takes_write_lock_before_callback_reads() -> graphql_orm::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "graphql-orm-state-machine-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let first = Database::<SqliteBackend>::connect_sqlite_with_options(
        &url,
        ConnectionOptions::default().max_connections(1),
    )
    .await?;
    let plan = first
        .schema()
        .plan_migration_to_entities("lock-init", "lock test", &[TransactionItem::metadata()])
        .await?;
    first
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    let second = Database::<SqliteBackend>::connect_sqlite_with_options(
        &url,
        ConnectionOptions::default().max_connections(1),
    )
    .await?;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let task = {
        let first = first.clone();
        let entered = entered.clone();
        let release = release.clone();
        tokio::spawn(async move {
            first
                .transaction(TransactionMode::StateMachine, |tx| {
                    Box::pin(async move {
                        let _ = tx.query::<TransactionItem>().count().await?;
                        entered.notify_one();
                        release.notified().await;
                        Ok(())
                    })
                })
                .await
        })
    };
    entered.notified().await;
    let competing = tokio::time::timeout(
        Duration::from_millis(100),
        TransactionItem::insert(&second, input("blocked")),
    )
    .await;
    assert!(
        competing.is_err(),
        "BEGIN IMMEDIATE locked before first read"
    );
    release.notify_one();
    task.await
        .expect("state-machine task")
        .expect("state-machine transaction");
    drop(second);
    drop(first);
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[tokio::test]
async fn nested_runner_is_rejected_without_opening_another_transaction() -> graphql_orm::Result<()>
{
    let database = database(2).await?;
    let nested_database = database.clone();
    database
        .transaction(TransactionMode::Default, move |_| {
            Box::pin(async move {
                let nested = nested_database
                    .transaction(TransactionMode::Default, |_| Box::pin(async { Ok(()) }))
                    .await
                    .expect_err("nested runner must fail closed");
                assert_eq!(nested.public_error().code, OrmErrorCode::Conflict);
                assert_eq!(
                    nested.public_error().message,
                    "nested ORM transactions are not supported"
                );
                Ok(())
            })
        })
        .await
        .expect("outer transaction remains usable");
    Ok(())
}

#[tokio::test]
async fn commit_failure_discards_queued_side_effects() -> graphql_orm::Result<()> {
    let database = database(1).await?;
    graphql_orm::sqlx::query("PRAGMA foreign_keys = ON")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query("CREATE TABLE commit_parent (id INTEGER PRIMARY KEY)")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TABLE commit_child (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER NOT NULL,
            FOREIGN KEY(parent_id) REFERENCES commit_parent(id)
                DEFERRABLE INITIALLY DEFERRED
        )",
    )
    .execute(database.pool())
    .await?;

    let sender = database.ensure_event_sender::<u64>();
    let mut events = sender.subscribe();
    let actions = Arc::new(AtomicUsize::new(0));
    let outcome = database
        .transaction(TransactionMode::Default, |tx| {
            let actions = actions.clone();
            Box::pin(async move {
                graphql_orm::sqlx::query(
                    "INSERT INTO commit_child (id, parent_id) VALUES (1, 999)",
                )
                .execute(tx.executor())
                .await?;
                tx.queue_event(1_u64);
                tx.defer(move |_| async move {
                    actions.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, std::convert::Infallible>(())
                });
                Ok(())
            })
        })
        .await;
    assert!(matches!(outcome, Err(TransactionError::Failed(_))));
    assert!(events.try_recv().is_err());
    assert_eq!(actions.load(Ordering::SeqCst), 0);
    assert_eq!(TransactionItem::count_all(&database).await?, 0);
    Ok(())
}
