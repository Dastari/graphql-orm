#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "retained_events",
    plural = "RetainedEvents",
    append_only = true,
    retention_purge = "retained_event.purge"
)]
#[graphql_orm(search(index = true, tokenizer = "unicode61"))]
struct RetainedEvent {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    kind: String,
    #[graphql_orm(searchable(weight = "A"))]
    payload: String,
}

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(
    table = "retention_facts",
    plural = "RetentionFacts",
    append_only = true
)]
struct RetentionFact {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    #[sortable]
    policy: String,
    affected: i32,
}

schema_roots! {
    query_custom_ops: [],
    entities: [RetainedEvent],
}

#[derive(Clone)]
struct RetentionPolicy;

#[derive(Clone, Default)]
struct RetentionHook {
    after_delete: Arc<AtomicUsize>,
    deferred: Arc<AtomicUsize>,
}

impl graphql_orm::graphql::orm::MutationHook<SqliteBackend> for RetentionHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        context: &'a mut MutationContext<'_, SqliteBackend>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name == "RetainedEvent"
                && event.action == ChangeAction::Deleted
                && event.phase == MutationPhase::After
            {
                let active_markers: i64 = graphql_orm::sqlx::query_scalar(
                    "SELECT COUNT(*) FROM __graphql_orm_retention_context",
                )
                .fetch_one(context.executor())
                .await?;
                assert_eq!(active_markers, 0, "the bypass must end before after-hooks");
                assert!(event.before::<RetainedEvent>()?.is_some());
                assert!(event.after::<RetainedEvent>()?.is_none());
                self.after_delete.fetch_add(1, Ordering::SeqCst);
                let deferred = self.deferred.clone();
                context.defer(move |_database| async move {
                    deferred.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, &'static str>(())
                });
            }
            Ok(())
        })
    }
}

impl graphql_orm::graphql::orm::EntityPolicy<SqliteBackend> for RetentionPolicy {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        kind: EntityAccessKind,
        surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if surface != EntityAccessSurface::RetentionMaintenance {
                return Ok(policy_key.is_none());
            }
            Ok(entity_name == "RetainedEvent"
                && policy_key == Some("retained_event.purge")
                && kind == EntityAccessKind::Write)
        })
    }
}

#[derive(Clone)]
struct DenyRetentionRows;

impl RowPolicy<SqliteBackend> for DenyRetentionRows {
    fn can_read_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }

    fn can_write_row<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<SqliteBackend>,
        entity_name: &'static str,
        policy_key: Option<&'static str>,
        surface: EntityAccessSurface,
        _row: &'a (dyn std::any::Any + Send + Sync),
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            Ok(!(entity_name == "RetainedEvent"
                && policy_key.is_none()
                && surface == EntityAccessSurface::RetentionMaintenance))
        })
    }
}

fn create(kind: &str) -> CreateRetainedEventInput {
    CreateRetainedEventInput {
        kind: kind.to_string(),
        payload: "protected".to_string(),
    }
}

fn kind_filter(kind: &str) -> RetainedEventWhereInput {
    RetainedEventWhereInput {
        kind: Some(StringFilter {
            eq: Some(kind.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn managed_database(url: &str) -> graphql_orm::Result<Database<SqliteBackend>> {
    let mut database = Database::<SqliteBackend>::connect_sqlite(url).await?;
    database.set_entity_policy(RetentionPolicy);
    let entities = [RetainedEvent::metadata(), RetentionFact::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities("retention-v1", "retention test", &entities)
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    Ok(database)
}

#[tokio::test]
async fn bounded_retention_is_opt_in_policy_gated_and_schema_enforced()
-> Result<(), Box<dyn std::error::Error>> {
    let mut database = managed_database("sqlite::memory:").await?;
    let hook = RetentionHook::default();
    database.set_mutation_hook(hook.clone());
    let target =
        SchemaModel::from_entities(&[RetainedEvent::metadata(), RetentionFact::metadata()]);
    assert!(RetainedEvent::metadata().append_only);
    assert_eq!(
        RetainedEvent::metadata().retention_policy,
        Some("retained_event.purge")
    );
    assert!(
        target
            .tables
            .iter()
            .find(|table| table.table_name == "retained_events")
            .expect("retained target table")
            .retention_purge
    );
    let mut strict_append_only = target.clone();
    strict_append_only
        .tables
        .iter_mut()
        .find(|table| table.table_name == "retained_events")
        .expect("retained target table")
        .retention_purge = false;
    assert_ne!(target.stable_hash(), strict_append_only.stable_hash());
    let disable_plan = graphql_orm::graphql::orm::diff_schema_models_for_backend(
        DatabaseBackend::Sqlite,
        &target,
        &strict_append_only,
    );
    assert!(disable_plan.steps.iter().any(|step| matches!(
        step,
        MigrationStep::SetAppendOnly {
            enabled: true,
            retention_purge: false,
            ..
        }
    )));
    let backup = backup_descriptors_from_entities(&[RetainedEvent::metadata()]);
    assert!(backup[0].retention_purge);
    let sdl = schema_builder(database.clone()).finish().sdl();
    assert!(!sdl.to_ascii_lowercase().contains("purge"));

    RetainedEvent::insert(&database, create("expired")).await?;
    RetainedEvent::insert(&database, create("expired")).await?;
    RetainedEvent::insert(&database, create("keep")).await?;
    assert_eq!(
        RetainedEvent::search(
            database.pool(),
            SearchInput {
                query: "protected".to_string(),
                mode: Some(SearchMode::Plain),
                min_score: None,
            },
        )
        .fetch_all()
        .await?
        .len(),
        3
    );

    let ordinary_delete = graphql_orm::sqlx::query("DELETE FROM retained_events")
        .execute(database.pool())
        .await;
    assert!(ordinary_delete.is_err());
    let ordinary_update =
        graphql_orm::sqlx::query("UPDATE retained_events SET payload = 'tampered'")
            .execute(database.pool())
            .await;
    assert!(ordinary_update.is_err());
    assert!(MutationLimit::new(0).is_err());

    let empty_filter = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedEvent>(
                        RetainedEventWhereInput::default(),
                        MutationLimit::new(3)?,
                    )
                    .await
                    .map_err(Into::into)
            })
        })
        .await;
    assert!(empty_filter.is_err());
    let swallowed_failure = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                let _ = maintenance
                    .purge::<RetainedEvent>(
                        RetainedEventWhereInput::default(),
                        MutationLimit::new(3)?,
                    )
                    .await;
                Ok(())
            })
        })
        .await;
    assert!(matches!(
        swallowed_failure,
        Err(TransactionError::Failed(_))
    ));

    let overflow = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedEvent>(kind_filter("expired"), MutationLimit::new(1)?)
                    .await
                    .map_err(Into::into)
            })
        })
        .await?;
    assert_eq!(
        overflow,
        RetentionPurgeOutcome::LimitExceeded { maximum: 1 }
    );

    let mut events = database
        .ensure_event_sender::<RetentionPurgeEvent>()
        .subscribe();
    let mut changed_events = database
        .ensure_event_sender::<RetainedEventChangedEvent>()
        .subscribe();
    let mut fact_events = database
        .ensure_event_sender::<RetentionFactChangedEvent>()
        .subscribe();
    let purged = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                let outcome = maintenance
                    .purge::<RetainedEvent>(kind_filter("expired"), MutationLimit::new(2)?)
                    .await?;
                maintenance
                    .insert::<RetentionFact>(CreateRetentionFactInput {
                        policy: "retained_event.purge".to_string(),
                        affected: 2,
                    })
                    .await?;
                Ok(outcome)
            })
        })
        .await?;
    assert_eq!(purged, RetentionPurgeOutcome::Purged { affected: 2 });
    assert_eq!(hook.after_delete.load(Ordering::SeqCst), 2);
    assert_eq!(hook.deferred.load(Ordering::SeqCst), 2);
    assert_eq!(
        events
            .recv()
            .await
            .expect("post-commit purge event")
            .affected,
        2
    );
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let mut actions = Vec::new();
    for _ in 0..2 {
        actions.push(
            changed_events
                .recv()
                .await
                .expect("post-commit entity change event")
                .action,
        );
    }
    assert_eq!(
        actions
            .iter()
            .filter(|action| **action == ChangeAction::Deleted)
            .count(),
        2
    );
    assert!(
        actions
            .iter()
            .all(|action| *action == ChangeAction::Deleted)
    );
    assert!(matches!(
        changed_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        fact_events
            .recv()
            .await
            .expect("separate audit fact change event")
            .action,
        ChangeAction::Created
    );

    let rows = RetainedEvent::query(database.pool()).fetch_all().await?;
    assert_eq!(rows.len(), 1);
    assert!(rows.iter().any(|row| row.kind == "keep"));
    let facts = RetentionFact::query(database.pool()).fetch_all().await?;
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].policy, "retained_event.purge");
    assert_eq!(facts[0].affected, 2);
    assert_eq!(
        RetainedEvent::search(
            database.pool(),
            SearchInput {
                query: "protected".to_string(),
                mode: Some(SearchMode::Plain),
                min_score: None,
            },
        )
        .fetch_all()
        .await?
        .len(),
        1,
        "purge must remove only the deleted search documents"
    );

    let live = introspect_sqlite_schema(&database).await?;
    let live_table = live
        .tables
        .iter()
        .find(|table| table.table_name == "retained_events")
        .expect("retained table");
    assert!(live_table.append_only);
    assert!(live_table.retention_purge);
    let clean = database
        .schema()
        .plan_migration("retention-v2", "idempotent", &live, &target)?;
    assert!(clean.steps.is_empty());

    graphql_orm::sqlx::query(
        "ALTER TABLE __graphql_orm_retention_context ADD COLUMN weakened TEXT",
    )
    .execute(database.pool())
    .await?;
    let weakened_context = introspect_sqlite_schema(&database).await?;
    assert!(
        !weakened_context
            .tables
            .iter()
            .find(|table| table.table_name == "retained_events")
            .expect("retained table with weakened context")
            .retention_purge
    );
    let context_repair = database.schema().plan_migration(
        "retention-context-repair",
        "repair managed retention context",
        &weakened_context,
        &target,
    )?;
    database
        .schema()
        .apply_migration(&context_repair, ApplyOptions::default())
        .await?;
    let repaired_context = introspect_sqlite_schema(&database).await?;
    assert!(
        repaired_context
            .tables
            .iter()
            .find(|table| table.table_name == "retained_events")
            .expect("retained table after context repair")
            .retention_purge
    );

    graphql_orm::sqlx::query("DROP TRIGGER graphql_orm_append_only_retained_events_delete")
        .execute(database.pool())
        .await?;
    graphql_orm::sqlx::query(
        "CREATE TRIGGER graphql_orm_append_only_retained_events_delete
         BEFORE DELETE ON retained_events WHEN 0
         BEGIN SELECT RAISE(ABORT, 'append-only entity'); END",
    )
    .execute(database.pool())
    .await?;
    let tampered = introspect_sqlite_schema(&database).await?;
    let repair = database.schema().plan_migration(
        "retention-v1",
        "recorded retention drift",
        &tampered,
        &target,
    )?;
    assert!(repair.steps.iter().any(|step| matches!(
        step.step,
        MigrationStep::SetAppendOnly {
            enabled: true,
            retention_purge: true,
            ..
        }
    )));
    database
        .schema()
        .apply_migration(&repair, ApplyOptions::default())
        .await
        .expect_err("a recorded version with weakened retention enforcement must fail closed");
    Ok(())
}

#[tokio::test]
async fn retention_requires_a_dedicated_policy_provider_and_rejects_nesting()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:").await?;
    let entities = [RetainedEvent::metadata()];
    let plan = database
        .schema()
        .plan_migration_to_entities("retention-policy-v1", "policy test", &entities)
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;
    RetainedEvent::insert(&database, create("expired")).await?;
    let denied = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedEvent>(kind_filter("expired"), MutationLimit::new(1)?)
                    .await
                    .map_err(Into::into)
            })
        })
        .await;
    assert!(denied.is_err());

    let mut database = database;
    database.set_entity_policy(RetentionPolicy);
    let nested_database = database.clone();
    database
        .retention_transaction(move |_maintenance| {
            Box::pin(async move {
                let nested = nested_database
                    .transaction(TransactionMode::Default, |_ordinary| {
                        Box::pin(async move { Ok::<_, OrmPublicError>(()) })
                    })
                    .await;
                assert!(matches!(nested, Err(TransactionError::Rejected(_))));
                Ok(())
            })
        })
        .await?;

    database.set_row_policy(DenyRetentionRows);
    let row_denied = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedEvent>(kind_filter("expired"), MutationLimit::new(1)?)
                    .await
                    .map_err(Into::into)
            })
        })
        .await;
    assert!(matches!(row_denied, Err(TransactionError::Rejected(_))));
    assert_eq!(
        RetainedEvent::query(database.pool())
            .fetch_all()
            .await?
            .len(),
        1
    );

    Ok(())
}

#[tokio::test]
async fn rollback_cancellation_and_pool_reuse_clear_retention_context() -> graphql_orm::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "graphql-orm-retention-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let mut database = managed_database(&url).await?;
    let hook = RetentionHook::default();
    database.set_mutation_hook(hook.clone());
    let mut events = database
        .ensure_event_sender::<RetentionPurgeEvent>()
        .subscribe();
    RetainedEvent::insert(&database, create("rollback")).await?;
    let mut changed_events = database
        .ensure_event_sender::<RetainedEventChangedEvent>()
        .subscribe();

    let rejected = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                let _ = maintenance
                    .purge::<RetainedEvent>(kind_filter("rollback"), MutationLimit::new(1)?)
                    .await?;
                Err::<(), _>(graphql_orm::graphql::errors::OrmPublicError::new(
                    graphql_orm::graphql::errors::OrmErrorCode::Conflict,
                ))
            })
        })
        .await;
    assert!(rejected.is_err());
    assert_eq!(hook.after_delete.load(Ordering::SeqCst), 1);
    assert_eq!(hook.deferred.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        changed_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        RetainedEvent::query(database.pool())
            .fetch_all()
            .await?
            .len(),
        1
    );

    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let task_database = database.clone();
    let task_started = started.clone();
    let task_release = release.clone();
    let task = tokio::spawn(async move {
        task_database
            .retention_transaction(|maintenance| {
                Box::pin(async move {
                    let _ = maintenance
                        .purge::<RetainedEvent>(kind_filter("rollback"), MutationLimit::new(1)?)
                        .await?;
                    task_started.notify_one();
                    task_release.notified().await;
                    Ok(())
                })
            })
            .await
    });
    started.notified().await;
    task.abort();
    let _ = task.await;
    assert_eq!(hook.after_delete.load(Ordering::SeqCst), 2);
    assert_eq!(hook.deferred.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        changed_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    let panic_database = database.clone();
    let panic_task = tokio::spawn(async move {
        panic_database
            .retention_transaction::<(), _>(|maintenance| {
                Box::pin(async move {
                    let _ = maintenance
                        .purge::<RetainedEvent>(kind_filter("rollback"), MutationLimit::new(1)?)
                        .await?;
                    panic!("intentional retention rollback test")
                })
            })
            .await
    });
    assert!(
        panic_task
            .await
            .expect_err("retention callback must panic in its task")
            .is_panic()
    );
    assert_eq!(hook.after_delete.load(Ordering::SeqCst), 3);
    assert_eq!(hook.deferred.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        changed_events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    assert_eq!(
        RetainedEvent::query(database.pool())
            .fetch_all()
            .await?
            .len(),
        1
    );
    let stale_marker_count: i64 =
        graphql_orm::sqlx::query_scalar("SELECT COUNT(*) FROM __graphql_orm_retention_context")
            .fetch_one(database.pool())
            .await?;
    assert_eq!(stale_marker_count, 0);
    assert!(
        graphql_orm::sqlx::query("DELETE FROM retained_events")
            .execute(database.pool())
            .await
            .is_err()
    );
    drop(database);
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[tokio::test]
async fn concurrent_retention_workers_never_over_delete_and_busy_is_retryable()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "graphql-orm-retention-concurrency-{}.sqlite",
        graphql_orm::uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let database = managed_database(&url).await?;
    RetainedEvent::insert(&database, create("concurrent")).await?;

    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let first_database = database.clone();
    let first_started = started.clone();
    let first_release = release.clone();
    let first = tokio::spawn(async move {
        first_database
            .retention_transaction(|maintenance| {
                Box::pin(async move {
                    first_started.notify_one();
                    first_release.notified().await;
                    maintenance
                        .purge::<RetainedEvent>(kind_filter("concurrent"), MutationLimit::new(1)?)
                        .await
                        .map_err(Into::into)
                })
            })
            .await
    });
    started.notified().await;

    let second_database = database.clone();
    let second = tokio::spawn(async move {
        second_database
            .retention_transaction(|maintenance| {
                Box::pin(async move {
                    maintenance
                        .purge::<RetainedEvent>(kind_filter("concurrent"), MutationLimit::new(1)?)
                        .await
                        .map_err(Into::into)
                })
            })
            .await
    });
    tokio::task::yield_now().await;
    release.notify_one();

    assert_eq!(
        first.await.expect("first worker task")?,
        RetentionPurgeOutcome::Purged { affected: 1 }
    );
    match second.await.expect("second worker task") {
        Ok(RetentionPurgeOutcome::Purged { affected: 0 }) => {}
        Err(error) if error.is_retryable() => {}
        other => panic!("unexpected second worker outcome: {other:?}"),
    }
    assert!(
        RetainedEvent::query(database.pool())
            .fetch_all()
            .await?
            .is_empty()
    );
    drop(database);
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[tokio::test]
async fn retention_predicate_values_are_bound_not_interpolated()
-> Result<(), Box<dyn std::error::Error>> {
    let database = managed_database("sqlite::memory:").await?;
    let hostile = "expired' OR 1 = 1 --";
    RetainedEvent::insert(&database, create(hostile)).await?;
    RetainedEvent::insert(&database, create("safe")).await?;

    let outcome = database
        .retention_transaction(|maintenance| {
            Box::pin(async move {
                maintenance
                    .purge::<RetainedEvent>(kind_filter(hostile), MutationLimit::new(1)?)
                    .await
                    .map_err(Into::into)
            })
        })
        .await?;
    assert_eq!(outcome, RetentionPurgeOutcome::Purged { affected: 1 });
    let remaining = RetainedEvent::query(database.pool()).fetch_all().await?;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].kind, "safe");
    Ok(())
}
