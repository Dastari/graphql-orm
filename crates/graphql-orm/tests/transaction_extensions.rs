//! Typed transaction-scoped extensions visible to mutation hooks (#564).

#![cfg(all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))))]

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::graphql::errors::{OrmErrorCode, OrmPublicError};
use graphql_orm::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "fenced_records", plural = "FencedRecords")]
struct FencedRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    #[filterable(type = "string")]
    id: String,
    #[filterable(type = "string")]
    state: String,
}

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[graphql_entity(backend = "sqlite", table = "fenced_audits", plural = "FencedAudits")]
struct FencedAudit {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    id: String,
    note: String,
}

schema_roots! { backend: "sqlite", query_custom_ops: [], entities: [FencedRecord, FencedAudit] }

/// The capability a reviewed workflow step attaches to its own transaction.
/// Dropping it is observable, so a test can tell when it ended.
struct ReviewedStep {
    step: &'static str,
    dropped: Arc<AtomicBool>,
}
impl Drop for ReviewedStep {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

/// A second, unrelated extension type.
struct Correlation(u64);

#[derive(Default)]
struct Seen {
    /// The step each `FencedRecord` hook call observed.
    steps: Mutex<Vec<Option<&'static str>>>,
    /// What a nested write's hook observed.
    nested: Mutex<Vec<Option<&'static str>>>,
    /// Whether a hook could attach an extension itself.
    hook_insert_refused: AtomicUsize,
    /// Make the next fenced write's hook panic.
    panic_next: AtomicBool,
}

/// Refuses every write to a fenced record unless the transaction carries the
/// reviewed step, and audits admitted writes with a nested write.
struct FenceHook(Arc<Seen>);
impl MutationHook for FenceHook {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            let step = hook_ctx.extension::<ReviewedStep>().map(|step| step.step);
            if event.entity_name == "FencedAudit" {
                self.0.nested.lock().unwrap().push(step);
                return Ok(());
            }
            if event.phase != MutationPhase::Before {
                return Ok(());
            }
            self.0.steps.lock().unwrap().push(step);
            if self.0.panic_next.swap(false, Ordering::SeqCst) {
                panic!("hook failure under test");
            }
            // A hook reads extensions; it cannot grant itself one.
            match hook_ctx.insert_extension(Correlation(0)) {
                Err(error) if error.code == OrmErrorCode::Conflict => {
                    self.0.hook_insert_refused.fetch_add(1, Ordering::SeqCst);
                }
                other => panic!("a hook attached an extension: {:?}", other.is_ok()),
            }
            assert!(hook_ctx.remove_extension::<ReviewedStep>().is_err());
            if step.is_none() {
                return Err(OrmPublicError::with_message(
                    OrmErrorCode::Forbidden,
                    "fenced records change only in a reviewed step",
                )
                .into_graphql_error());
            }
            let audit = format!("{}-{}", event.id, self.0.steps.lock().unwrap().len());
            hook_ctx
                .insert::<FencedAudit>(CreateFencedAuditInput {
                    id: audit,
                    note: "admitted".into(),
                })
                .await
                .map_err(graphql_orm::graphql::errors::graphql_error_from_sqlx)?;
            Ok(())
        })
    }
}

async fn setup() -> (Database, Arc<Seen>) {
    let mut db = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .unwrap();
    for ddl in [
        "CREATE TABLE fenced_records (id TEXT PRIMARY KEY, state TEXT NOT NULL)",
        "CREATE TABLE fenced_audits (id TEXT PRIMARY KEY, note TEXT NOT NULL)",
        "INSERT INTO fenced_records VALUES ('r1', 'open'), ('r2', 'open'), ('r3', 'open')",
    ] {
        graphql_orm::sqlx::query(ddl)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let seen = Arc::new(Seen::default());
    db.set_mutation_hook(FenceHook(seen.clone()));
    (db, seen)
}

async fn states(db: &Database) -> Vec<String> {
    let rows: Vec<(String,)> =
        graphql_orm::sqlx::query_as("SELECT state FROM fenced_records ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    rows.into_iter().map(|row| row.0).collect()
}

fn close() -> UpdateFencedRecordInput {
    UpdateFencedRecordInput {
        state: Some("closed".into()),
    }
}

fn open_records() -> FencedRecordWhereInput {
    FencedRecordWhereInput {
        state: Some(graphql_orm::graphql::filters::StringFilter {
            eq: Some("open".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn reviewed(dropped: &Arc<AtomicBool>) -> ReviewedStep {
    ReviewedStep {
        step: "close-out",
        dropped: dropped.clone(),
    }
}

#[tokio::test]
async fn hooks_read_the_extension_for_every_write_of_the_owning_transaction() {
    let (db, seen) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = reviewed(&dropped);
    let observed_by_deferred = Arc::new(Mutex::new(None));
    let deferred = observed_by_deferred.clone();
    let dropped_at_commit = dropped.clone();
    let updated = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                assert!(tx.extension::<ReviewedStep>().is_none());
                assert!(tx.insert_extension(extension)?.is_none());
                assert_eq!(tx.insert_extension(Correlation(7))?.map(|c| c.0), None);
                assert_eq!(tx.insert_extension(Correlation(8))?.map(|c| c.0), Some(7));
                assert_eq!(tx.extension::<Correlation>().map(|c| c.0), Some(8));
                // A single write, then a bulk write: every row's hook sees it.
                tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                    .await
                    .map_err(OrmPublicError::from)?;
                let bulk = tx
                    .update_where::<FencedRecord>(open_records(), close())
                    .await
                    .map_err(OrmPublicError::from)?;
                tx.defer(move |_db| async move {
                    // The extension ended before the commit, so a deferred
                    // action, which runs after it, finds it already dropped.
                    *deferred.lock().unwrap() = Some(dropped_at_commit.load(Ordering::SeqCst));
                    Ok::<(), String>(())
                });
                assert!(
                    !dropped.load(Ordering::SeqCst),
                    "alive inside the transaction"
                );
                Ok(bulk)
            })
        })
        .await
        .unwrap();
    assert_eq!(updated, 2);
    assert_eq!(states(&db).await, ["closed", "closed", "closed"]);
    assert_eq!(
        *seen.steps.lock().unwrap(),
        [Some("close-out"); 3],
        "single and bulk writes"
    );
    // The hook's nested audit writes ran their own hooks in the same transaction.
    let nested = seen.nested.lock().unwrap().clone();
    assert_eq!(nested.len(), 6, "Before and After for three audit rows");
    assert!(nested.iter().all(|step| *step == Some("close-out")));
    assert_eq!(seen.hook_insert_refused.load(Ordering::SeqCst), 3);
    assert_eq!(*observed_by_deferred.lock().unwrap(), Some(true));
}

#[tokio::test]
async fn other_transactions_handles_and_generated_mutations_see_no_extension() {
    let (db, seen) = setup().await;

    // A transaction that attaches nothing, on the same handle and on a clone.
    for handle in [db.clone(), db.clone()] {
        let outcome = handle
            .transaction(TransactionMode::Default, |tx| {
                Box::pin(async move {
                    tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                        .await
                        .map_err(OrmPublicError::from)?;
                    Ok(())
                })
            })
            .await;
        assert!(outcome.is_err(), "no extension, so the fence refuses");
    }
    // A repository helper opens its own transaction.
    assert!(
        FencedRecord::update_by_id(&db, &"r1".to_owned(), close())
            .await
            .is_err()
    );
    // A generated GraphQL mutation can neither set nor observe one.
    let response = schema_builder(db.clone())
        .data(AuthSubject::new("user-1"))
        .finish()
        .execute(
            r#"mutation { updateFencedRecord(id: "r1", input: {state: "closed"}) { success } }"#,
        )
        .await;
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert_eq!(
        response.errors[0].message,
        "fenced records change only in a reviewed step"
    );

    assert_eq!(states(&db).await, ["open", "open", "open"]);
    let steps = seen.steps.lock().unwrap();
    assert_eq!(steps.len(), 4);
    assert!(steps.iter().all(Option::is_none), "{steps:?}");
}

#[tokio::test]
async fn rollback_drops_the_extension_and_the_next_transaction_starts_empty() {
    let (db, seen) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = reviewed(&dropped);
    let outcome = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert_extension(extension)?;
                tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                    .await
                    .map_err(OrmPublicError::from)?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(matches!(outcome, Err(TransactionError::Rejected(_))));
    assert!(dropped.load(Ordering::SeqCst), "rollback ended it");
    assert_eq!(states(&db).await, ["open", "open", "open"]);
    assert_eq!(*seen.steps.lock().unwrap(), [Some("close-out")]);

    // Nothing leaks into the next transaction on the same handle.
    let outcome = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                assert!(tx.extension::<ReviewedStep>().is_none());
                assert!(tx.extension::<Correlation>().is_none());
                tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                    .await
                    .map_err(OrmPublicError::from)?;
                Ok(())
            })
        })
        .await;
    assert!(outcome.is_err());

    // Nested transactions stay rejected, with or without an extension.
    let inner = db.clone();
    let nested = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert_extension(Correlation(1))?;
                let nested = inner
                    .transaction(TransactionMode::Default, |_| Box::pin(async { Ok(()) }))
                    .await;
                Ok(matches!(nested, Err(TransactionError::Rejected(_))))
            })
        })
        .await
        .unwrap();
    assert!(nested);
}

#[tokio::test]
async fn the_owner_can_remove_an_extension_before_later_writes() {
    let (db, seen) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = reviewed(&dropped);
    let outcome = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.insert_extension(extension)?;
                tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                    .await
                    .map_err(OrmPublicError::from)?;
                drop(tx.remove_extension::<ReviewedStep>()?);
                // The same transaction no longer admits fenced writes.
                let refused = tx
                    .update_by_id::<FencedRecord>(&"r2".to_owned(), close())
                    .await
                    .is_err();
                Ok(refused)
            })
        })
        .await;
    // The refused second write changed nothing, so the first one commits.
    assert!(outcome.unwrap());
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(states(&db).await, ["closed", "open", "open"]);
    assert_eq!(
        *seen.steps.lock().unwrap(),
        [Some("close-out"), None],
        "the second write saw no extension"
    );
}

#[tokio::test]
async fn a_panicking_hook_drops_the_extension_with_the_transaction() {
    use graphql_orm::futures::FutureExt as _;
    let (db, seen) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = reviewed(&dropped);
    seen.panic_next.store(true, Ordering::SeqCst);
    let transaction = db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.insert_extension(extension)?;
            tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                .await
                .map_err(OrmPublicError::from)?;
            Ok(())
        })
    });
    let outcome = std::panic::AssertUnwindSafe(transaction)
        .catch_unwind()
        .await;
    assert!(outcome.is_err(), "the hook's panic propagates");
    assert!(
        dropped.load(Ordering::SeqCst),
        "unwinding ended the extension"
    );
    assert_eq!(states(&db).await, ["open", "open", "open"]);

    // The handle is usable, and its next transaction carries nothing over.
    let outcome = db
        .transaction(TransactionMode::Default, |tx| {
            Box::pin(async move { Ok(tx.extension::<ReviewedStep>().is_none()) })
        })
        .await;
    assert!(outcome.unwrap());
}

#[tokio::test]
async fn cancelling_the_transaction_future_drops_the_extension() {
    let (db, seen) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = reviewed(&dropped);
    let transaction = db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.insert_extension(extension)?;
            tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                .await
                .map_err(OrmPublicError::from)?;
            // The request is cancelled while the transaction is open.
            graphql_orm::futures::future::pending::<()>().await;
            Ok(())
        })
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), transaction)
            .await
            .is_err(),
        "the transaction future was dropped mid-transaction"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "cancellation ended the extension"
    );
    assert_eq!(*seen.steps.lock().unwrap(), [Some("close-out")]);
    assert_eq!(states(&db).await, ["open", "open", "open"]);
}

/// What the spawned task observed: extension dropped, new transaction
/// empty, fenced write refused.
type SpawnedTask = Arc<Mutex<Option<tokio::task::JoinHandle<(bool, bool, bool)>>>>;

#[tokio::test]
async fn a_task_spawned_from_a_deferred_action_sees_no_extension() {
    let (db, _) = setup().await;
    let dropped = Arc::new(AtomicBool::new(false));
    let extension = reviewed(&dropped);
    let spawned: SpawnedTask = Default::default();
    let handle = spawned.clone();
    let was_dropped = dropped.clone();
    db.transaction(TransactionMode::Default, |tx| {
        Box::pin(async move {
            tx.insert_extension(extension)?;
            tx.update_by_id::<FencedRecord>(&"r1".to_owned(), close())
                .await
                .map_err(OrmPublicError::from)?;
            tx.defer(move |db| async move {
                // The action is handed the database, never the transaction.
                let task = tokio::spawn(async move {
                    let already_dropped = was_dropped.load(Ordering::SeqCst);
                    // A transaction the task opens on that handle is a new one.
                    let empty = db
                        .transaction(TransactionMode::Default, |tx| {
                            Box::pin(async move { Ok(tx.extension::<ReviewedStep>().is_none()) })
                        })
                        .await
                        .unwrap();
                    // And the fence refuses its writes like anyone else's.
                    let refused = FencedRecord::update_by_id(&db, &"r2".to_owned(), close())
                        .await
                        .is_err();
                    (already_dropped, empty, refused)
                });
                *handle.lock().unwrap() = Some(task);
                Ok::<(), String>(())
            });
            Ok(())
        })
    })
    .await
    .unwrap();
    let task = spawned.lock().unwrap().take().expect("the action ran");
    assert_eq!(task.await.unwrap(), (true, true, true));
    assert_eq!(states(&db).await, ["closed", "open", "open"]);
}
