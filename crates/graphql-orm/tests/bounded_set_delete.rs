//! Bounded set deletes use the same typed transaction and hooks on each backend.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql"))),
    all(feature = "mssql", not(any(feature = "sqlite", feature = "postgres"))),
))]

#[cfg(feature = "mssql")]
#[allow(dead_code)]
#[path = "support/owned_mssql.rs"]
mod owned_mssql;
#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "support/owned_postgres.rs"]
mod owned_postgres;

use graphql_orm::futures::future::BoxFuture;
use graphql_orm::prelude::*;
#[cfg(not(feature = "mssql"))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(feature = "mssql")]
use std::time::Duration;
#[cfg(feature = "postgres")]
use tracing::instrument::WithSubscriber;

#[derive(GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug)]
#[cfg_attr(
    feature = "sqlite",
    graphql_entity(backend = "sqlite", table = "batch_records")
)]
#[cfg_attr(
    feature = "postgres",
    graphql_entity(backend = "postgres", table = "batch_records")
)]
#[cfg_attr(
    feature = "mssql",
    graphql_entity(
        backend = "mssql",
        table = "batch_records",
        schema_policy = "external_writable"
    )
)]
struct BatchRecord {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    #[filterable(type = "string")]
    id: String,
    #[filterable(type = "string")]
    bucket: String,
}

impl SetDeleteEntity for BatchRecord {}

#[cfg(not(feature = "mssql"))]
#[derive(Clone, Default)]
struct DeleteTrace(Arc<AtomicUsize>);

#[cfg(not(feature = "mssql"))]
impl tracing::Subscriber for DeleteTrace {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Statement<'a>(&'a AtomicUsize);
        impl tracing::field::Visit for Statement<'_> {
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "summary" && value.starts_with("DELETE ") {
                    self.0.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        event.record(&mut Statement(&self.0));
    }
}

#[derive(Clone, Default)]
struct Hooks(Arc<Mutex<Vec<(MutationPhase, String)>>>);
impl MutationHook for Hooks {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.action == ChangeAction::Deleted {
                let row = event.before::<BatchRecord>()?.expect("delete preimage");
                assert_eq!(row.id, event.id);
                assert!(event.after::<BatchRecord>()?.is_none());
                self.0
                    .lock()
                    .unwrap()
                    .push((event.phase.clone(), row.id.clone()));
                if event.phase == MutationPhase::After && row.bucket == "veto" {
                    return Err(async_graphql::Error::new("delete veto"));
                }
            }
            Ok(())
        })
    }
}

struct DenyBucket;
impl RowPolicy for DenyBucket {
    fn can_read_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        _: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(row.downcast_ref::<BatchRecord>().unwrap().bucket != "denied") })
    }
}

async fn sql(db: &Database, statement: &str) {
    <DefaultBackend as WriteBackend>::execute_with_binds(db.pool(), statement, &[])
        .await
        .unwrap();
}

async fn ids(db: &Database) -> Vec<String> {
    DefaultBackend::fetch_rows(db.pool(), "SELECT id FROM batch_records ORDER BY id", &[])
        .await
        .unwrap()
        .iter()
        .map(|row| DefaultBackend::try_get_string(row, "id").unwrap())
        .collect()
}

fn by_bucket(bucket: &str) -> BatchRecordWhereInput {
    BatchRecordWhereInput {
        bucket: Some(StringFilter {
            eq: Some(bucket.to_owned()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn limit(max: u32) -> MutationLimit {
    MutationLimit::new(max).unwrap()
}

#[cfg(feature = "sqlite")]
type FixtureOwner = tokio::sync::MutexGuard<'static, ()>;
#[cfg(feature = "postgres")]
type FixtureOwner = owned_postgres::OwnedPostgres;
#[cfg(feature = "mssql")]
type FixtureOwner = owned_mssql::OwnedSqlServer;

async fn fixture() -> (Database, FixtureOwner) {
    // SQLite logs completed queries on its worker thread. Serialize this
    // binary's SQLite fixtures so the global subscriber only counts our DELETE.
    #[cfg(feature = "sqlite")]
    let owner = {
        static FIXTURES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        FIXTURES.lock().await
    };
    #[cfg(feature = "sqlite")]
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    #[cfg(feature = "postgres")]
    let owner = owned_postgres::OwnedPostgres::start("bounded-set-delete").unwrap();
    #[cfg(feature = "postgres")]
    let db = Database::new(
        graphql_orm::sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    graphql_orm::sqlx::query("SET statement_timeout = '15s'")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&owner.url)
            .await
            .unwrap(),
    );
    #[cfg(feature = "mssql")]
    let owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
    #[cfg(feature = "mssql")]
    let db = Database::builder(
        graphql_orm::db::mssql::MssqlPool::connect_ado_external_writable_with(
            &owner.connection_string,
            graphql_orm::db::mssql::MssqlPoolOptions::new()
                .max_connections(2)
                .query_timeout(Some(Duration::from_secs(15))),
        )
        .await
        .unwrap(),
    )
    .schema_policy(SchemaPolicy::ExternalWritable)
    .build();

    sql(&db, "CREATE TABLE batch_records (id VARCHAR(80) NOT NULL PRIMARY KEY, bucket VARCHAR(40) NOT NULL)").await;
    (db, owner)
}

#[tokio::test]
async fn bounded_set_delete_in_state_machine() {
    let (mut db, _owner) = fixture().await;
    let keys: Vec<_> = (0..100)
        .map(|number| format!("record-{number:03}"))
        .collect();
    let values = keys
        .iter()
        .map(|id| format!("('{id}', 'batch')"))
        .collect::<Vec<_>>()
        .join(", ");
    sql(
        &db,
        &format!("INSERT INTO batch_records VALUES {values}, ('survivor', 'keep')"),
    )
    .await;
    let hooks = Hooks::default();
    db.set_mutation_hook(hooks.clone());
    db.set_row_policy(DenyBucket);
    let mut notifications = db
        .ensure_event_sender::<BatchRecordChangedEvent>()
        .subscribe();

    let deletion = db.transaction(TransactionMode::StateMachine, |tx| {
        Box::pin(async move {
            tx.delete_keys_set_bounded::<BatchRecord>(&keys, limit(100))
                .await
                .map_err(OrmPublicError::from)
        })
    });
    #[cfg(not(feature = "mssql"))]
    let capture = DeleteTrace::default();
    #[cfg(feature = "sqlite")]
    tracing::subscriber::set_global_default(capture.clone()).unwrap();
    #[cfg(feature = "postgres")]
    let deletion = deletion.with_subscriber(capture.clone());
    let deletion = deletion.await;
    #[cfg(not(feature = "mssql"))]
    assert_eq!(capture.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        deletion.unwrap(),
        BoundedMutationOutcome::Applied { affected: 100 }
    );
    assert_eq!(ids(&db).await, ["survivor"]);
    for number in 0..100 {
        let event = notifications.try_recv().unwrap();
        assert_eq!(event.action, ChangeAction::Deleted);
        assert_eq!(event.id, format!("record-{number:03}"));
    }
    let calls = std::mem::take(&mut *hooks.0.lock().unwrap());
    assert_eq!(calls.len(), 200);
    assert!(
        calls[..100]
            .iter()
            .all(|(phase, _)| *phase == MutationPhase::Before)
    );
    assert!(
        calls[100..]
            .iter()
            .all(|(phase, _)| *phase == MutationPhase::After)
    );

    sql(&db, "INSERT INTO batch_records VALUES ('veto-a', 'veto'), ('veto-b', 'veto'), ('denied', 'denied')").await;
    let original = contents(&db).await;
    for (keys, expected) in [
        (vec![], OrmErrorCode::InvalidInput),
        (
            vec!["survivor".to_owned(), "survivor".to_owned()],
            OrmErrorCode::Conflict,
        ),
        (
            vec!["survivor".to_owned(), "absent".to_owned()],
            OrmErrorCode::Conflict,
        ),
        (
            vec!["survivor".to_owned(), "denied".to_owned()],
            OrmErrorCode::Forbidden,
        ),
    ] {
        let result = db
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    tx.delete_keys_set_bounded::<BatchRecord>(&keys, limit(2))
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await;
        assert!(
            matches!(result, Err(TransactionError::Rejected(ref error)) if error.code == expected),
            "{result:?}"
        );
        assert_eq!(contents(&db).await, original);
    }
    assert!(hooks.0.lock().unwrap().is_empty());

    let outcome = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let over = tx
                    .delete_where_set_bounded::<BatchRecord>(by_bucket("veto"), limit(1))
                    .await
                    .map_err(OrmPublicError::from)?;
                assert_eq!(over, BoundedMutationOutcome::LimitExceeded { maximum: 1 });
                tx.insert::<BatchRecord>(CreateBatchRecordInput {
                    id: "progress".into(),
                    bucket: "keep".into(),
                })
                .await
                .map_err(OrmPublicError::from)?;
                let veto = tx
                    .delete_where_set_bounded::<BatchRecord>(by_bucket("veto"), limit(2))
                    .await;
                assert!(veto.is_err());
                // Deliberately swallow the veto to verify that this context cannot commit.
                Ok(())
            })
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(ids(&db).await, ["denied", "survivor", "veto-a", "veto-b"]);
    assert!(notifications.try_recv().is_err());

    let empty_filter = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.delete_where_set_bounded::<BatchRecord>(
                    BatchRecordWhereInput::default(),
                    limit(2),
                )
                .await
                .map_err(OrmPublicError::from)
            })
        })
        .await;
    assert!(
        matches!(empty_filter, Err(TransactionError::Rejected(ref error)) if error.code == OrmErrorCode::InvalidInput)
    );

    let applied = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.delete_where_set_bounded::<BatchRecord>(by_bucket("keep"), limit(1))
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await
        .unwrap();
    assert_eq!(applied, BoundedMutationOutcome::Applied { affected: 1 });
    assert_eq!(notifications.try_recv().unwrap().id, "survivor");
    assert_eq!(ids(&db).await, ["denied", "veto-a", "veto-b"]);

    #[cfg(feature = "sqlite")]
    {
        sql(
            &db,
            "INSERT INTO batch_records VALUES ('skip', 'count'), ('take', 'count')",
        )
        .await;
        sql(&db, "CREATE TRIGGER skip_batch_record BEFORE DELETE ON batch_records WHEN OLD.id = 'skip' BEGIN SELECT RAISE(IGNORE); END").await;
        let result = db
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    tx.insert::<BatchRecord>(CreateBatchRecordInput {
                        id: "progress".into(),
                        bucket: "keep".into(),
                    })
                    .await
                    .map_err(OrmPublicError::from)?;
                    let mismatch = tx
                        .delete_where_set_bounded::<BatchRecord>(by_bucket("count"), limit(2))
                        .await
                        .unwrap_err();
                    assert_eq!(
                        OrmPublicError::from_sqlx(&mismatch).code,
                        OrmErrorCode::Conflict
                    );
                    Ok(())
                })
            })
            .await;
        assert!(result.is_err());
        assert_eq!(
            ids(&db).await,
            ["denied", "skip", "take", "veto-a", "veto-b"]
        );
        assert!(notifications.try_recv().is_err());
    }
}

async fn contents(db: &Database) -> Vec<(String, String)> {
    DefaultBackend::fetch_rows(
        db.pool(),
        "SELECT id, bucket FROM batch_records ORDER BY id",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            DefaultBackend::try_get_string(row, "id").unwrap(),
            DefaultBackend::try_get_string(row, "bucket").unwrap(),
        )
    })
    .collect()
}

fn assert_rejected<T: std::fmt::Debug>(result: Result<T, TransactionError>, code: OrmErrorCode) {
    assert!(
        matches!(result, Err(TransactionError::Rejected(ref error)) if error.code == code),
        "{result:?}"
    );
}

struct BeforeDelete(Option<&'static str>);

impl MutationHook for BeforeDelete {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.action == ChangeAction::Deleted
                && event.phase == MutationPhase::Before
                && event.id == "a"
            {
                match self.0 {
                    Some(statement) => {
                        let result = tx
                            .execute(statement, &[])
                            .await
                            .map_err(|error| async_graphql::Error::new(error.to_string()))?;
                        assert_eq!(<DefaultBackend as WriteBackend>::rows_affected(&result), 1);
                    }
                    None => return Err(OrmPublicError::forbidden().into_graphql_error()),
                }
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn before_hooks_cannot_change_the_match_set_or_commit_a_veto() {
    let (mut db, _owner) = fixture().await;
    sql(
        &db,
        "INSERT INTO batch_records VALUES ('a', 'batch'), ('b', 'batch'), ('c', 'keep')",
    )
    .await;
    let original = contents(&db).await;
    for (statement, code) in [
        (
            Some("UPDATE batch_records SET bucket = 'keep' WHERE id = 'b'"),
            OrmErrorCode::Conflict,
        ),
        (
            Some("DELETE FROM batch_records WHERE id = 'b'"),
            OrmErrorCode::Conflict,
        ),
        (
            Some("INSERT INTO batch_records VALUES ('new', 'batch')"),
            OrmErrorCode::Conflict,
        ),
        (None, OrmErrorCode::Forbidden),
    ] {
        db.set_mutation_hook(BeforeDelete(statement));
        let result = db
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    tx.insert::<BatchRecord>(CreateBatchRecordInput {
                        id: "progress".into(),
                        bucket: "keep".into(),
                    })
                    .await?;
                    let refusal = tx
                        .delete_where_set_bounded::<BatchRecord>(by_bucket("batch"), limit(3))
                        .await
                        .unwrap_err();
                    assert_eq!(OrmPublicError::from_sqlx(&refusal).code, code);
                    Ok(())
                })
            })
            .await;
        assert_rejected(result, OrmErrorCode::Conflict);
        assert_eq!(contents(&db).await, original, "{statement:?}");
    }
}

#[tokio::test]
async fn default_transaction_refuses_set_delete() {
    let (db, _owner) = fixture().await;
    sql(
        &db,
        "INSERT INTO batch_records VALUES ('a', 'batch'), ('b', 'batch')",
    )
    .await;
    let original = contents(&db).await;
    assert_rejected(
        db.transaction(TransactionMode::Default, |tx| {
            Box::pin(async move {
                tx.delete_where_set_bounded::<BatchRecord>(by_bucket("batch"), limit(2))
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .await,
        OrmErrorCode::Conflict,
    );
    assert_eq!(contents(&db).await, original);
}

#[cfg(feature = "postgres")]
struct UpdateAfterSelection;

#[cfg(feature = "postgres")]
impl MutationHook for UpdateAfterSelection {
    fn on_mutation<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        tx: &'a mut MutationContext<'_>,
        event: &'a MutationEvent,
    ) -> BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.action == ChangeAction::Deleted
                && event.phase == MutationPhase::Before
                && event.id == "a"
            {
                // Execute through the pool on a second connection. The hook runs
                // after the set-delete transaction has established its snapshot.
                sql(
                    tx.database(),
                    "UPDATE batch_records SET bucket = 'external' WHERE id = 'b'",
                )
                .await;
            }
            Ok(())
        })
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn concurrent_update_after_selection_causes_serialization_failure() {
    let (mut db, _owner) = fixture().await;
    sql(
        &db,
        "INSERT INTO batch_records VALUES ('a', 'batch'), ('b', 'batch'), ('c', 'keep')",
    )
    .await;
    let mut expected = contents(&db).await;
    expected[1].1 = "external".to_owned();
    db.set_mutation_hook(UpdateAfterSelection);
    let result = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                let error = tx
                    .delete_where_set_bounded::<BatchRecord>(by_bucket("batch"), limit(2))
                    .await
                    .unwrap_err();
                assert_eq!(
                    error
                        .as_database_error()
                        .and_then(|error| error.code())
                        .as_deref(),
                    Some("40001")
                );
                Err::<(), _>(OrmPublicError::from(error))
            })
        })
        .await;
    assert!(
        matches!(result, Err(TransactionError::Retryable(error)) if error.code == OrmErrorCode::ServiceUnavailable)
    );
    assert_eq!(contents(&db).await, expected);
}

#[cfg(feature = "sqlite")]
mod model_refusals {
    use super::*;

    // The derive omits set-delete adapters for append-only and read-only
    // models. Supply test adapters to exercise the separate runtime guard.
    macro_rules! unsupported_model {
        ($model:ident, $($options:tt)*) => {
            #[derive(GraphQLEntity, serde::Serialize, serde::Deserialize, Clone, Debug)]
            #[graphql_entity(backend = "sqlite", table = "batch_records", $($options)*)]
            struct $model {
                #[primary_key]
                #[sortable]
                id: String,
                bucket: String,
            }

            impl SetDeleteModel<SqliteBackend> for $model {
                type Key = String;
                type WhereInput = BatchRecordWhereInput;

                fn set_delete_validate_filter(_: &Self::WhereInput) -> graphql_orm::Result<()> {
                    unreachable!("ineligible model reached filter validation")
                }

                fn set_delete_key_values(_: &Self::Key) -> Vec<SqlValue> {
                    unreachable!("ineligible model reached key binding")
                }

                fn set_delete_row_key_values(&self) -> Vec<SqlValue> {
                    unreachable!("ineligible model reached row selection")
                }

                fn set_delete_event_id(&self) -> String {
                    unreachable!("ineligible model reached event preparation")
                }

                fn set_delete_queue_change<'a>(
                    &'a self,
                    _: &'a mut MutationContext<'_, SqliteBackend>,
                ) -> BoxFuture<'a, graphql_orm::Result<()>> {
                    unreachable!("ineligible model reached change notification")
                }
            }

            impl SetDeleteEntity for $model {}
        };
    }

    unsupported_model!(AppendOnlyRecord, append_only = true);
    unsupported_model!(ReadOnlyRecord, schema_policy = "external_read_only");

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(backend = "sqlite", table = "batch_records")]
    #[graphql_orm(search(index = true))]
    struct IndexedRecord {
        #[primary_key]
        #[sortable]
        id: String,
        #[graphql_orm(searchable(weight = "A"))]
        bucket: String,
    }
    impl SetDeleteEntity for IndexedRecord {}

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        backend = "sqlite",
        table = "composite_records",
        repository_mutations = true
    )]
    struct CompositeRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[sortable]
        id: String,
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        bucket: String,
    }
    impl SetDeleteEntity for CompositeRecord {}

    async fn assert_key_refused<T: SetDeleteEntity<Key = String>>(
        db: &Database,
        code: OrmErrorCode,
    ) {
        let original = contents(db).await;
        let result = db
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    tx.delete_keys_set_bounded::<T>(&["a".to_owned()], limit(2))
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await;
        assert_rejected(result, code);
        assert_eq!(contents(db).await, original);
    }

    #[tokio::test]
    async fn set_delete_refuses_unsupported_models() {
        let (db, _owner) = fixture().await;
        sql(
            &db,
            "INSERT INTO batch_records VALUES ('a', 'batch'), ('b', 'batch')",
        )
        .await;
        assert_key_refused::<AppendOnlyRecord>(&db, OrmErrorCode::Forbidden).await;
        assert_key_refused::<ReadOnlyRecord>(&db, OrmErrorCode::Forbidden).await;
        assert_key_refused::<IndexedRecord>(&db, OrmErrorCode::Conflict).await;

        sql(&db, "CREATE TABLE composite_records (id TEXT NOT NULL, bucket TEXT NOT NULL, PRIMARY KEY (id, bucket))").await;
        sql(
            &db,
            "INSERT INTO composite_records VALUES ('a', 'batch'), ('a', 'keep')",
        )
        .await;
        let stored = || async {
            DefaultBackend::fetch_rows(
                db.pool(),
                "SELECT id, bucket FROM composite_records ORDER BY id, bucket",
                &[],
            )
            .await
            .unwrap()
            .iter()
            .map(|row| {
                (
                    DefaultBackend::try_get_string(row, "id").unwrap(),
                    DefaultBackend::try_get_string(row, "bucket").unwrap(),
                )
            })
            .collect::<Vec<_>>()
        };
        let original = stored().await;
        let result = db
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    let key = CompositeRecordKey {
                        id: "a".into(),
                        bucket: "batch".into(),
                    };
                    tx.delete_keys_set_bounded::<CompositeRecord>(&[key], limit(2))
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .await;
        assert_rejected(result, OrmErrorCode::AuthorizationMisconfigured);
        assert_eq!(stored().await, original);
    }
}
