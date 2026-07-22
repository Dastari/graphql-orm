#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use graphql_orm::prelude::*;

#[cfg(feature = "postgres")]
use std::process::Command;

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq,
)]
#[graphql_entity(
    table = "bounded_sentinel_single",
    plural = "BoundedSentinelSingles",
    default_sort = "ordinal ASC"
)]
struct BoundedSentinelSingle {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    cohort: String,
    #[filterable(type = "boolean")]
    updated: bool,
    #[sortable]
    ordinal: i64,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(
    feature = "sqlite",
    repository_entity(
        backend = "sqlite",
        table = "bounded_sentinel_composite",
        plural = "BoundedSentinelComposites",
        repository_mutations = true,
        unique_composite = "cohort,ordinal",
        upsert = "cohort,ordinal",
        write_policy = "bounded-sentinel.write",
        default_sort = "cohort ASC, ordinal ASC"
    )
)]
#[cfg_attr(
    feature = "postgres",
    repository_entity(
        backend = "postgres",
        table = "bounded_sentinel_composite",
        plural = "BoundedSentinelComposites",
        repository_mutations = true,
        unique_composite = "cohort,ordinal",
        upsert = "cohort,ordinal",
        write_policy = "bounded-sentinel.write",
        default_sort = "cohort ASC, ordinal ASC"
    )
)]
struct BoundedSentinelComposite {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    cohort: String,
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    ordinal: String,
    #[filterable(type = "string")]
    value: String,
}

#[derive(
    GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq,
)]
#[graphql_entity(
    table = "bounded_sentinel_retained",
    plural = "BoundedSentinelRetained",
    append_only = true,
    retention_purge = "bounded-sentinel.purge",
    default_sort = "ordinal ASC"
)]
struct BoundedSentinelRetained {
    #[primary_key]
    id: graphql_orm::uuid::Uuid,
    #[filterable(type = "string")]
    cohort: String,
    payload: String,
    #[sortable]
    ordinal: i64,
}

schema_roots! {
    query_custom_ops: [],
    entities: [BoundedSentinelSingle, BoundedSentinelRetained],
}

#[cfg(feature = "sqlite")]
type TestBackend = SqliteBackend;
#[cfg(feature = "postgres")]
type TestBackend = PostgresBackend;

#[derive(Clone, Default)]
struct CountingHook {
    calls: Arc<AtomicUsize>,
}

impl CountingHook {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl MutationHook<TestBackend> for CountingHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _hook_ctx: &'a mut MutationContext<'_, TestBackend>,
        _event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[derive(Clone)]
struct AllowSentinelPolicies;

impl EntityPolicy<TestBackend> for AllowSentinelPolicies {
    fn can_access_entity<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        _db: &'a Database<TestBackend>,
        _entity_name: &'static str,
        _policy_key: Option<&'static str>,
        _kind: EntityAccessKind,
        _surface: EntityAccessSurface,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async { Ok(true) })
    }
}

struct CardinalityChangeHook {
    target: graphql_orm::uuid::Uuid,
    fired: std::sync::atomic::AtomicBool,
}

impl MutationHook<TestBackend> for CardinalityChangeHook {
    fn on_mutation<'a>(
        &'a self,
        _ctx: Option<&'a async_graphql::Context<'_>>,
        hook_ctx: &'a mut MutationContext<'_, TestBackend>,
        event: &'a MutationEvent,
    ) -> graphql_orm::futures::future::BoxFuture<'a, async_graphql::Result<()>> {
        Box::pin(async move {
            if event.entity_name == BoundedSentinelSingle::metadata().entity_name
                && event.action == ChangeAction::Updated
                && event.phase == MutationPhase::Before
                && !self.fired.swap(true, Ordering::SeqCst)
            {
                hook_ctx
                    .delete_by_id::<BoundedSentinelSingle>(&self.target)
                    .await
                    .map_err(|error| async_graphql::Error::new(error.to_string()))?;
            }
            Ok(())
        })
    }
}

fn single_filter(cohort: impl Into<String>) -> BoundedSentinelSingleWhereInput {
    BoundedSentinelSingleWhereInput {
        cohort: Some(StringFilter {
            eq: Some(cohort.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn composite_filter(cohort: impl Into<String>) -> BoundedSentinelCompositeWhereInput {
    BoundedSentinelCompositeWhereInput {
        cohort: Some(StringFilter {
            eq: Some(cohort.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn retention_filter(cohort: impl Into<String>) -> BoundedSentinelRetainedWhereInput {
    BoundedSentinelRetainedWhereInput {
        cohort: Some(StringFilter {
            eq: Some(cohort.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn scenario_counts(ceiling: u32) -> Vec<u32> {
    let mut counts = vec![
        0,
        ceiling.saturating_sub(1),
        ceiling,
        ceiling.checked_add(1).expect("test ceiling + 1"),
        ceiling.checked_add(257).expect("material overflow fixture"),
    ];
    counts.sort_unstable();
    counts.dedup();
    counts
}

fn no_broadcast_event<T: Clone>(receiver: &mut tokio::sync::broadcast::Receiver<T>) -> bool {
    matches!(
        receiver.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    )
}

#[cfg(feature = "sqlite")]
async fn insert_single_rows(
    database: &Database<TestBackend>,
    cohort: &str,
    count: u32,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "WITH RECURSIVE seq(n) AS (
             SELECT 0 WHERE ? > 0
             UNION ALL SELECT n + 1 FROM seq WHERE n + 1 < ?
         )
         INSERT INTO bounded_sentinel_single (id, cohort, updated, ordinal)
         SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' ||
                      hex(randomblob(2)) || '-' || hex(randomblob(2)) || '-' ||
                      hex(randomblob(6))), ?, 0, n FROM seq",
    )
    .bind(i64::from(count))
    .bind(i64::from(count))
    .bind(cohort)
    .execute(database.pool())
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_single_rows(
    database: &Database<TestBackend>,
    cohort: &str,
    count: u32,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "INSERT INTO bounded_sentinel_single (id, cohort, updated, ordinal)
         SELECT gen_random_uuid(), $1, FALSE, n
         FROM generate_series(0, $2::bigint - 1) AS n",
    )
    .bind(cohort)
    .bind(i64::from(count))
    .execute(database.pool())
    .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_composite_rows(
    database: &Database<TestBackend>,
    cohort: &str,
    count: u32,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "WITH RECURSIVE seq(n) AS (
             SELECT 0 WHERE ? > 0
             UNION ALL SELECT n + 1 FROM seq WHERE n + 1 < ?
         )
         INSERT INTO bounded_sentinel_composite (cohort, ordinal, value)
         SELECT ?, printf('%012d', n), 'original' FROM seq",
    )
    .bind(i64::from(count))
    .bind(i64::from(count))
    .bind(cohort)
    .execute(database.pool())
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_composite_rows(
    database: &Database<TestBackend>,
    cohort: &str,
    count: u32,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "INSERT INTO bounded_sentinel_composite (cohort, ordinal, value)
         SELECT $1, lpad(n::text, 12, '0'), 'original'
         FROM generate_series(0, $2::bigint - 1) AS n",
    )
    .bind(cohort)
    .bind(i64::from(count))
    .execute(database.pool())
    .await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_retained_rows(
    database: &Database<TestBackend>,
    cohort: &str,
    count: u32,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "WITH RECURSIVE seq(n) AS (
             SELECT 0 WHERE ? > 0
             UNION ALL SELECT n + 1 FROM seq WHERE n + 1 < ?
         )
         INSERT INTO bounded_sentinel_retained (id, cohort, payload, ordinal)
         SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' ||
                      hex(randomblob(2)) || '-' || hex(randomblob(2)) || '-' ||
                      hex(randomblob(6))), ?, 'protected', n FROM seq",
    )
    .bind(i64::from(count))
    .bind(i64::from(count))
    .bind(cohort)
    .execute(database.pool())
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_retained_rows(
    database: &Database<TestBackend>,
    cohort: &str,
    count: u32,
) -> graphql_orm::Result<()> {
    graphql_orm::sqlx::query(
        "INSERT INTO bounded_sentinel_retained (id, cohort, payload, ordinal)
         SELECT gen_random_uuid(), $1, 'protected', n
         FROM generate_series(0, $2::bigint - 1) AS n",
    )
    .bind(cohort)
    .bind(i64::from(count))
    .execute(database.pool())
    .await?;
    Ok(())
}

async fn single_count(
    database: &Database<TestBackend>,
    cohort: &str,
    updated: Option<bool>,
) -> graphql_orm::Result<i64> {
    let mut filter = single_filter(cohort);
    filter.updated = updated.map(|updated| BoolFilter {
        eq: Some(updated),
        ..Default::default()
    });
    BoundedSentinelSingle::count_query(database.pool())
        .filter(&filter)
        .count()
        .await
}

#[cfg(feature = "sqlite")]
async fn composite_count(
    database: &Database<TestBackend>,
    cohort: &str,
    value: Option<&str>,
) -> graphql_orm::Result<i64> {
    let count = if let Some(value) = value {
        graphql_orm::sqlx::query_scalar(
            "SELECT COUNT(*) FROM bounded_sentinel_composite WHERE cohort = ? AND value = ?",
        )
        .bind(cohort)
        .bind(value)
        .fetch_one(database.pool())
        .await?
    } else {
        graphql_orm::sqlx::query_scalar(
            "SELECT COUNT(*) FROM bounded_sentinel_composite WHERE cohort = ?",
        )
        .bind(cohort)
        .fetch_one(database.pool())
        .await?
    };
    Ok(count)
}

#[cfg(feature = "postgres")]
async fn composite_count(
    database: &Database<TestBackend>,
    cohort: &str,
    value: Option<&str>,
) -> graphql_orm::Result<i64> {
    let count = if let Some(value) = value {
        graphql_orm::sqlx::query_scalar(
            "SELECT COUNT(*) FROM bounded_sentinel_composite WHERE cohort = $1 AND value = $2",
        )
        .bind(cohort)
        .bind(value)
        .fetch_one(database.pool())
        .await?
    } else {
        graphql_orm::sqlx::query_scalar(
            "SELECT COUNT(*) FROM bounded_sentinel_composite WHERE cohort = $1",
        )
        .bind(cohort)
        .fetch_one(database.pool())
        .await?
    };
    Ok(count)
}

async fn retained_count(
    database: &Database<TestBackend>,
    cohort: &str,
) -> graphql_orm::Result<i64> {
    BoundedSentinelRetained::count_query(database.pool())
        .filter(&retention_filter(cohort))
        .count()
        .await
}

async fn run_single_key_matrix(
    database: &Database<TestBackend>,
    hook: &CountingHook,
) -> Result<(), Box<dyn std::error::Error>> {
    for ceiling in [1, 99, 100, 1_024, 10_000] {
        for count in scenario_counts(ceiling) {
            let cohort = format!("single-update-{ceiling}-{count}");
            insert_single_rows(database, &cohort, count).await?;
            let mut events = database
                .ensure_event_sender::<BoundedSentinelSingleChangedEvent>()
                .subscribe();
            let hook_before = hook.calls();
            let outcome = if count > ceiling {
                let filter = single_filter(&cohort);
                let no_match = single_filter(format!("{cohort}-absent"));
                database
                    .transaction(TransactionMode::StateMachine, move |transaction| {
                        Box::pin(async move {
                            let outcome = transaction
                                .update_where_bounded::<BoundedSentinelSingle>(
                                    filter,
                                    UpdateBoundedSentinelSingleInput {
                                        updated: Some(true),
                                        ..Default::default()
                                    },
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            let usable = transaction
                                .update_where_bounded::<BoundedSentinelSingle>(
                                    no_match,
                                    UpdateBoundedSentinelSingleInput {
                                        updated: Some(true),
                                        ..Default::default()
                                    },
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            assert_eq!(usable, BoundedMutationOutcome::Applied { affected: 0 });
                            Ok(outcome)
                        })
                    })
                    .await?
            } else {
                BoundedSentinelSingle::update_where_bounded(
                    database,
                    single_filter(&cohort),
                    UpdateBoundedSentinelSingleInput {
                        updated: Some(true),
                        ..Default::default()
                    },
                    MutationLimit::new(ceiling)?,
                )
                .await?
            };
            if count > ceiling {
                assert_eq!(
                    outcome,
                    BoundedMutationOutcome::LimitExceeded { maximum: ceiling }
                );
                assert_eq!(single_count(database, &cohort, Some(true)).await?, 0);
                assert_eq!(
                    single_count(database, &cohort, None).await?,
                    i64::from(count)
                );
                assert_eq!(hook.calls(), hook_before);
                assert!(no_broadcast_event(&mut events));
                let output = format!("{outcome:?}");
                assert!(!output.contains(&cohort));
            } else {
                assert_eq!(outcome, BoundedMutationOutcome::Applied { affected: count });
                assert_eq!(
                    single_count(database, &cohort, Some(true)).await?,
                    i64::from(count)
                );
            }
        }

        for count in scenario_counts(ceiling) {
            let cohort = format!("single-delete-{ceiling}-{count}");
            insert_single_rows(database, &cohort, count).await?;
            let mut events = database
                .ensure_event_sender::<BoundedSentinelSingleChangedEvent>()
                .subscribe();
            let hook_before = hook.calls();
            let outcome = if count > ceiling {
                let filter = single_filter(&cohort);
                let no_match = single_filter(format!("{cohort}-absent"));
                database
                    .transaction(TransactionMode::StateMachine, move |transaction| {
                        Box::pin(async move {
                            let outcome = transaction
                                .delete_where_bounded::<BoundedSentinelSingle>(
                                    filter,
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            let usable = transaction
                                .delete_where_bounded::<BoundedSentinelSingle>(
                                    no_match,
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            assert_eq!(usable, BoundedMutationOutcome::Applied { affected: 0 });
                            Ok(outcome)
                        })
                    })
                    .await?
            } else {
                BoundedSentinelSingle::delete_where_bounded(
                    database,
                    single_filter(&cohort),
                    MutationLimit::new(ceiling)?,
                )
                .await?
            };
            if count > ceiling {
                assert_eq!(
                    outcome,
                    BoundedMutationOutcome::LimitExceeded { maximum: ceiling }
                );
                assert_eq!(
                    single_count(database, &cohort, None).await?,
                    i64::from(count)
                );
                assert_eq!(hook.calls(), hook_before);
                assert!(no_broadcast_event(&mut events));
            } else {
                assert_eq!(outcome, BoundedMutationOutcome::Applied { affected: count });
                assert_eq!(single_count(database, &cohort, None).await?, 0);
            }
        }
    }
    Ok(())
}

async fn run_composite_matrix(
    database: &Database<TestBackend>,
    hook: &CountingHook,
) -> Result<(), Box<dyn std::error::Error>> {
    for ceiling in [1, 99, 100, 1_024, 10_000] {
        for count in scenario_counts(ceiling) {
            let cohort = format!("composite-update-{ceiling}-{count}");
            insert_composite_rows(database, &cohort, count).await?;
            let mut events = database
                .ensure_event_sender::<BoundedSentinelCompositeChangedEvent>()
                .subscribe();
            let hook_before = hook.calls();
            let outcome = if count > ceiling {
                let filter = composite_filter(&cohort);
                let no_match = composite_filter(format!("{cohort}-absent"));
                database
                    .transaction(TransactionMode::StateMachine, move |transaction| {
                        Box::pin(async move {
                            let outcome = transaction
                                .update_where_bounded::<BoundedSentinelComposite>(
                                    filter,
                                    UpdateBoundedSentinelCompositeInput {
                                        value: Some("updated".to_string()),
                                    },
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            let usable = transaction
                                .update_where_bounded::<BoundedSentinelComposite>(
                                    no_match,
                                    UpdateBoundedSentinelCompositeInput {
                                        value: Some("updated".to_string()),
                                    },
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            assert_eq!(usable, BoundedMutationOutcome::Applied { affected: 0 });
                            Ok(outcome)
                        })
                    })
                    .await?
            } else {
                BoundedSentinelComposite::update_where_bounded(
                    database,
                    composite_filter(&cohort),
                    UpdateBoundedSentinelCompositeInput {
                        value: Some("updated".to_string()),
                    },
                    MutationLimit::new(ceiling)?,
                )
                .await?
            };
            if count > ceiling {
                assert_eq!(
                    outcome,
                    BoundedMutationOutcome::LimitExceeded { maximum: ceiling }
                );
                assert_eq!(
                    composite_count(database, &cohort, Some("updated")).await?,
                    0
                );
                assert_eq!(
                    composite_count(database, &cohort, None).await?,
                    i64::from(count)
                );
                assert_eq!(hook.calls(), hook_before);
                assert!(no_broadcast_event(&mut events));
            } else {
                assert_eq!(outcome, BoundedMutationOutcome::Applied { affected: count });
                assert_eq!(
                    composite_count(database, &cohort, Some("updated")).await?,
                    i64::from(count)
                );
            }
        }

        for count in scenario_counts(ceiling) {
            let cohort = format!("composite-delete-{ceiling}-{count}");
            insert_composite_rows(database, &cohort, count).await?;
            let mut events = database
                .ensure_event_sender::<BoundedSentinelCompositeChangedEvent>()
                .subscribe();
            let hook_before = hook.calls();
            let outcome = if count > ceiling {
                let filter = composite_filter(&cohort);
                let no_match = composite_filter(format!("{cohort}-absent"));
                database
                    .transaction(TransactionMode::StateMachine, move |transaction| {
                        Box::pin(async move {
                            let outcome = transaction
                                .delete_where_bounded::<BoundedSentinelComposite>(
                                    filter,
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            let usable = transaction
                                .delete_where_bounded::<BoundedSentinelComposite>(
                                    no_match,
                                    MutationLimit::new(ceiling)?,
                                )
                                .await
                                .map_err(OrmPublicError::from)?;
                            assert_eq!(usable, BoundedMutationOutcome::Applied { affected: 0 });
                            Ok(outcome)
                        })
                    })
                    .await?
            } else {
                BoundedSentinelComposite::delete_where_bounded(
                    database,
                    composite_filter(&cohort),
                    MutationLimit::new(ceiling)?,
                )
                .await?
            };
            if count > ceiling {
                assert_eq!(
                    outcome,
                    BoundedMutationOutcome::LimitExceeded { maximum: ceiling }
                );
                assert_eq!(
                    composite_count(database, &cohort, None).await?,
                    i64::from(count)
                );
                assert_eq!(hook.calls(), hook_before);
                assert!(no_broadcast_event(&mut events));
            } else {
                assert_eq!(outcome, BoundedMutationOutcome::Applied { affected: count });
                assert_eq!(composite_count(database, &cohort, None).await?, 0);
            }
        }
    }
    Ok(())
}

async fn run_retention_matrix(
    database: &Database<TestBackend>,
    hook: &CountingHook,
) -> Result<(), Box<dyn std::error::Error>> {
    for ceiling in [1, 99, 100, 1_024, 10_000] {
        for count in scenario_counts(ceiling) {
            let cohort = format!("retention-{ceiling}-{count}");
            insert_retained_rows(database, &cohort, count).await?;
            let mut purge_events = database
                .ensure_event_sender::<RetentionPurgeEvent>()
                .subscribe();
            let mut changed_events = database
                .ensure_event_sender::<BoundedSentinelRetainedChangedEvent>()
                .subscribe();
            let hook_before = hook.calls();
            let outcome = database
                .retention_transaction(|maintenance| {
                    let filter = retention_filter(&cohort);
                    let absent = retention_filter(format!("{cohort}-absent"));
                    Box::pin(async move {
                        let outcome = maintenance
                            .purge::<BoundedSentinelRetained>(filter, MutationLimit::new(ceiling)?)
                            .await?;
                        if matches!(outcome, RetentionPurgeOutcome::LimitExceeded { .. }) {
                            assert!(
                                !maintenance
                                    .query::<BoundedSentinelRetained>()
                                    .filter(absent)
                                    .exists()
                                    .await?
                            );
                        }
                        Ok(outcome)
                    })
                })
                .await?;
            if count > ceiling {
                assert_eq!(
                    outcome,
                    RetentionPurgeOutcome::LimitExceeded { maximum: ceiling }
                );
                assert_eq!(retained_count(database, &cohort).await?, i64::from(count));
                assert_eq!(hook.calls(), hook_before);
                assert!(no_broadcast_event(&mut purge_events));
                assert!(no_broadcast_event(&mut changed_events));
            } else {
                assert_eq!(outcome, RetentionPurgeOutcome::Purged { affected: count });
                assert_eq!(retained_count(database, &cohort).await?, 0);
            }
        }
    }
    Ok(())
}

async fn run_cardinality_change_rollback(
    database: &mut Database<TestBackend>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cohort = "cardinality-change-protected-value";
    BoundedSentinelSingle::insert(
        &*database,
        CreateBoundedSentinelSingleInput {
            cohort: cohort.to_string(),
            updated: false,
            ordinal: 1,
        },
    )
    .await?;
    let target = BoundedSentinelSingle::insert(
        &*database,
        CreateBoundedSentinelSingleInput {
            cohort: cohort.to_string(),
            updated: false,
            ordinal: 2,
        },
    )
    .await?;
    let mut events = database
        .ensure_event_sender::<BoundedSentinelSingleChangedEvent>()
        .subscribe();
    database.set_mutation_hook(CardinalityChangeHook {
        target: target.id,
        fired: std::sync::atomic::AtomicBool::new(false),
    });

    let error = BoundedSentinelSingle::update_where_bounded(
        &*database,
        single_filter(cohort),
        UpdateBoundedSentinelSingleInput {
            updated: Some(true),
            ..Default::default()
        },
        MutationLimit::new(2)?,
    )
    .await
    .expect_err("a selected-row cardinality change must fail closed");
    let public = OrmPublicError::from_sqlx(&error);
    assert_eq!(public.code, OrmErrorCode::InternalError);
    assert!(!format!("{error:?}{public:?}").contains(cohort));
    assert_eq!(single_count(database, cohort, None).await?, 2);
    assert_eq!(single_count(database, cohort, Some(true)).await?, 0);
    assert!(no_broadcast_event(&mut events));
    Ok(())
}

async fn run_matrix(mut database: Database<TestBackend>) -> Result<(), Box<dyn std::error::Error>> {
    database.set_entity_policy(AllowSentinelPolicies);
    let hook = CountingHook::default();
    database.set_mutation_hook(hook.clone());
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "bounded-sentinel-v1",
            "uncapped internal bounded mutation sentinels",
            &[
                BoundedSentinelSingle::metadata(),
                BoundedSentinelComposite::metadata(),
                BoundedSentinelRetained::metadata(),
            ],
        )
        .await?;
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await?;

    assert_eq!(PaginationConfig::DEFAULT_MAX_LIMIT, 100);
    insert_single_rows(&database, "public-page-cap", 101).await?;
    let public_rows = BoundedSentinelSingle::query(database.pool())
        .filter(single_filter("public-page-cap"))
        .limit(1_024)
        .fetch_all()
        .await?;
    assert_eq!(public_rows.len(), 100);

    run_single_key_matrix(&database, &hook).await?;
    run_composite_matrix(&database, &hook).await?;
    run_retention_matrix(&database, &hook).await?;
    run_cardinality_change_rollback(&mut database).await?;
    Ok(())
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_uncapped_internal_sentinels_preserve_exact_bounded_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    run_matrix(database).await
}

#[cfg(feature = "postgres")]
struct OwnedPostgres {
    name: String,
    owner_token: String,
    url: String,
}

#[cfg(feature = "postgres")]
impl Drop for OwnedPostgres {
    fn drop(&mut self) {
        let identity = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{ index .Config.Labels \"graphql-orm.test-owner\" }}",
                &self.name,
            ])
            .output();
        if identity
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).trim() == self.owner_token
            })
        {
            let _ = Command::new("docker")
                .args(["rm", "--force", "--volumes", &self.name])
                .output();
        }
    }
}

#[cfg(feature = "postgres")]
impl OwnedPostgres {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let token = graphql_orm::uuid::Uuid::new_v4().simple().to_string();
        let name = format!("graphql-orm-bounded-sentinel-{token}");
        let password = format!("bounded_{token}");
        let database = format!("bounded_{token}");
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--label",
                &format!("graphql-orm.test-owner={token}"),
                "--publish",
                "127.0.0.1::5432",
                "--env",
                "POSTGRES_USER=bounded_owner",
                "--env",
                &format!("POSTGRES_PASSWORD={password}"),
                "--env",
                &format!("POSTGRES_DB={database}"),
                "postgres:17-alpine",
            ])
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to start owned PostgreSQL 17: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let mut owned = Self {
            name,
            owner_token: token,
            url: String::new(),
        };
        for _ in 0..120 {
            let ready = Command::new("docker")
                .args([
                    "exec",
                    &owned.name,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "bounded_owner",
                ])
                .output()?;
            if ready.status.success() {
                let port = Command::new("docker")
                    .args(["port", &owned.name, "5432/tcp"])
                    .output()?;
                let published = String::from_utf8(port.stdout)?;
                let port = published
                    .lines()
                    .find_map(|line| line.strip_prefix("127.0.0.1:"))
                    .ok_or("owned PostgreSQL was not loopback-published")?;
                owned.url =
                    format!("postgres://bounded_owner:{password}@127.0.0.1:{port}/{database}");
                std::thread::sleep(std::time::Duration::from_millis(500));
                return Ok(owned);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        Err("owned PostgreSQL 17 did not become ready".into())
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "creates and owns a disposable loopback-only PostgreSQL 17 container"]
async fn postgres_uncapped_internal_sentinels_preserve_exact_bounded_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    let owned = OwnedPostgres::start()?;
    let database = Database::<PostgresBackend>::connect_postgres(&owned.url)
        .await?
        .with_schema_policy(SchemaPolicy::Managed);
    run_matrix(database).await
}
