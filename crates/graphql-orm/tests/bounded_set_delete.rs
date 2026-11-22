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
use std::sync::{Arc, Mutex};
#[cfg(feature = "mssql")]
use std::time::Duration;
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

#[derive(Clone, Default)]
struct DeleteTrace(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for DeleteTrace {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "graphql_orm::set_delete"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Statement<'a>(&'a Mutex<Vec<String>>);
        impl tracing::field::Visit for Statement<'_> {
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "sql" {
                    self.0.lock().unwrap().push(value.to_owned());
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

#[tokio::test]
async fn bounded_set_delete_in_state_machine() {
    #[cfg(feature = "sqlite")]
    let mut db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    #[cfg(feature = "postgres")]
    let mut owner = owned_postgres::OwnedPostgres::start("bounded-set-delete").unwrap();
    #[cfg(feature = "postgres")]
    let mut db = Database::new(
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
    let mut owner = owned_mssql::OwnedSqlServer::start().await.unwrap();
    #[cfg(feature = "mssql")]
    let mut db = Database::builder(
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

    let capture = DeleteTrace::default();
    let affected = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.delete_keys_set_bounded::<BatchRecord>(&keys, limit(100))
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .with_subscriber(capture.clone())
        .await
        .unwrap();
    assert_eq!(affected, BoundedMutationOutcome::Applied { affected: 100 });
    let statements = capture.0.lock().unwrap().clone();
    assert_eq!(statements.len(), 1, "{statements:?}");
    assert!(statements[0].starts_with("DELETE FROM "), "{statements:?}");
    assert!(statements[0].contains(" IN ("), "{statements:?}");
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
    for (keys, expected) in [
        (vec![], OrmErrorCode::InvalidInput),
        (
            vec!["survivor".to_owned(), "absent".to_owned()],
            OrmErrorCode::Conflict,
        ),
        (
            vec!["survivor".to_owned(), "denied".to_owned()],
            OrmErrorCode::Forbidden,
        ),
    ] {
        let capture = DeleteTrace::default();
        let result = db
            .transaction(TransactionMode::StateMachine, |tx| {
                Box::pin(async move {
                    tx.delete_keys_set_bounded::<BatchRecord>(&keys, limit(2))
                        .await
                        .map_err(OrmPublicError::from)
                })
            })
            .with_subscriber(capture.clone())
            .await;
        assert!(
            matches!(result, Err(TransactionError::Rejected(ref error)) if error.code == expected),
            "{result:?}"
        );
        assert!(capture.0.lock().unwrap().is_empty());
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

    let capture = DeleteTrace::default();
    let applied = db
        .transaction(TransactionMode::StateMachine, |tx| {
            Box::pin(async move {
                tx.delete_where_set_bounded::<BatchRecord>(by_bucket("keep"), limit(1))
                    .await
                    .map_err(OrmPublicError::from)
            })
        })
        .with_subscriber(capture.clone())
        .await
        .unwrap();
    assert_eq!(applied, BoundedMutationOutcome::Applied { affected: 1 });
    assert_eq!(capture.0.lock().unwrap().len(), 1);
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

    #[cfg(not(feature = "sqlite"))]
    owner.cleanup().unwrap();
}
