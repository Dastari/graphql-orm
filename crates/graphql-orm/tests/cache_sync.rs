#![cfg(feature = "sqlite")]

use graphql_orm::cache_sync::*;
use graphql_orm::futures::{
    Stream, StreamExt,
    future::{BoxFuture, poll_fn},
};
use graphql_orm::prelude::*;
use std::{sync::Arc, task::Poll};
use tokio::time::{Duration, timeout};

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "cache_partitions", plural = "CachePartitions")]
struct DatasetCacheStatus {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[filterable(type = "string")]
    #[sortable]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    generation: i64,
    rows: i64,
    #[graphql_orm(private, subscribe = false)]
    state_json: String,
}
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(table = "cache_items", plural = "CacheItems")]
struct CacheItem {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: String,
    #[filterable(type = "number")]
    #[sortable]
    generation: i64,
    value: String,
}
schema_roots! { generated_mutations: "none", query_custom_ops: [], entities: [DatasetCacheStatus, CacheItem], }

struct Store;
fn key_id(key: &CachePartitionKey) -> String {
    serde_json::to_string(&(key.dataset(), key.partition())).unwrap()
}
impl CacheSyncStore<SqliteBackend> for Store {
    fn load<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, SqliteBackend>,
        key: &'a CachePartitionKey,
    ) -> BoxFuture<'a, Result<CacheState, OrmPublicError>> {
        Box::pin(async move {
            let row = tx.find_by_id::<DatasetCacheStatus>(&key_id(key)).await?;
            row.map(|r| {
                serde_json::from_str(&r.state_json)
                    .map_err(|_| OrmPublicError::new(OrmErrorCode::InternalError))
            })
            .transpose()
            .map(Option::unwrap_or_default)
        })
    }
    fn save<'a>(
        &'a self,
        tx: &'a mut MutationContext<'_, SqliteBackend>,
        key: &'a CachePartitionKey,
        state: &'a CacheState,
    ) -> BoxFuture<'a, Result<(), OrmPublicError>> {
        Box::pin(async move {
            let id = key_id(key);
            let state_json = serde_json::to_string(state).unwrap();
            if tx.find_by_id::<DatasetCacheStatus>(&id).await?.is_some() {
                tx.update_by_id::<DatasetCacheStatus>(
                    &id,
                    UpdateDatasetCacheStatusInput {
                        generation: Some(state.generation()),
                        rows: Some(state.rows()),
                        state_json: Some(state_json),
                    },
                )
                .await?;
            } else {
                tx.insert::<DatasetCacheStatus>(CreateDatasetCacheStatusInput {
                    id,
                    generation: state.generation(),
                    rows: state.rows(),
                    state_json,
                })
                .await?;
            }
            Ok(())
        })
    }
}
fn policy() -> RefreshPolicy {
    RefreshPolicy {
        fresh_for: 60,
        cooldown: 5,
        lease_for: 30,
        retry_backoff: 10,
        max_backoff: 80,
    }
}
fn key(partition: &str) -> CachePartitionKey {
    CachePartitionKey::new("inventory", partition).unwrap()
}
async fn fixture() -> Database<SqliteBackend> {
    let db = Database::<SqliteBackend>::connect_sqlite_with_options(
        "sqlite::memory:",
        graphql_orm::db::ConnectionOptions::default().max_connections(1),
    )
    .await
    .unwrap();
    let plan = db
        .schema()
        .plan_migration_to_entities(
            "cache-init",
            "cache fixture",
            &[DatasetCacheStatus::metadata(), CacheItem::metadata()],
        )
        .await
        .unwrap();
    db.schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    db
}
async fn claimed(
    sync: &CacheSync<SqliteBackend, Store>,
    partition: &str,
    now: i64,
) -> RefreshClaim {
    match sync
        .claim(key(partition), policy(), now, false, None)
        .await
        .unwrap()
    {
        RefreshAdmission::Claimed(c) => c,
        x => panic!("expected claim, got {x:?}"),
    }
}
async fn registered<S: Stream + Unpin>(stream: &mut S) {
    poll_fn(|cx| match stream.poll_next_unpin(cx) {
        Poll::Pending => Poll::Ready(()),
        _ => panic!("unexpected initial event"),
    })
    .await;
}
async fn read_state(db: &Database, partition: &str) -> CacheState {
    serde_json::from_str(
        &DatasetCacheStatus::find_by_key(db, &key_id(&key(partition)))
            .await
            .unwrap()
            .unwrap()
            .state_json,
    )
    .unwrap()
}

#[tokio::test]
async fn promotion_is_atomic_and_generated_subscription_is_post_commit() {
    let db = fixture().await;
    let sync = CacheSync::new(db.clone(), Arc::new(Store));
    let claim = claimed(&sync, "tenant-a", 100).await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("reader"))
        .finish();
    assert!(!schema.sdl().contains("createDatasetCacheStatus"));
    assert!(!schema.sdl().contains("stateJson"));
    let mut stream = Box::pin(schema.execute_stream(
        "subscription { datasetCacheStatusChanged { datasetCacheStatus { generation rows } } }",
    ));
    registered(&mut stream).await;
    // Candidate writes are invisible until status and data both commit.
    sync.promote(claim, 101, 1, Some(90), None, |tx, generation| {
        Box::pin(async move {
            tx.insert::<CacheItem>(CreateCacheItemInput {
                id: "one".into(),
                generation,
                value: "complete".into(),
            })
            .await?;
            Ok(())
        })
    })
    .await
    .unwrap();
    let event = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(event.errors.is_empty(), "{:?}", event.errors);
    assert_eq!(
        event.data.into_json().unwrap()["datasetCacheStatusChanged"]["datasetCacheStatus"]["generation"],
        1
    );
    assert_eq!(
        CacheItem::find_by_key(&db, &"one".to_string())
            .await
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    let state = read_state(&db, "tenant-a").await;
    assert_eq!(state.last_success(), Some(101));
    assert_eq!(state.source_observed_at(), Some(90));
    assert!(state.is_fresh(102));
    assert!(!state.refresh_in_progress(102));
    assert!(matches!(
        sync.claim(key("tenant-a"), policy(), 102, true, None)
            .await
            .unwrap(),
        RefreshAdmission::NotDue
    ));
    assert!(matches!(
        sync.claim(key("tenant-a"), policy(), 106, false, None)
            .await
            .unwrap(),
        RefreshAdmission::NotDue
    ));
    // Reconnect intentionally misses the next notification; initial status query
    // still sees the committed generation and lets the host refetch its projection.
    drop(stream);
    let next = match sync
        .claim(key("tenant-a"), policy(), 106, true, None)
        .await
        .unwrap()
    {
        RefreshAdmission::Claimed(c) => c,
        x => panic!("{x:?}"),
    };
    sync.promote(next, 107, 1, None, None, |_, _| Box::pin(async { Ok(()) }))
        .await
        .unwrap();
    assert_eq!(read_state(&db, "tenant-a").await.generation(), 2);
}

#[tokio::test]
async fn rollback_retains_old_projection_and_emits_no_success() {
    let db = fixture().await;
    let sync = CacheSync::new(db.clone(), Arc::new(Store));
    let claim = claimed(&sync, "tenant-a", 100).await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("reader"))
        .finish();
    let mut stream = Box::pin(schema.execute_stream(
        "subscription { datasetCacheStatusChanged { datasetCacheStatus { generation } } }",
    ));
    registered(&mut stream).await;
    let result = sync
        .promote(claim.clone(), 101, 1, None, None, |tx, generation| {
            Box::pin(async move {
                tx.insert::<CacheItem>(CreateCacheItemInput {
                    id: "rejected".into(),
                    generation,
                    value: "partial".into(),
                })
                .await?;
                Err::<(), _>(OrmPublicError::new(OrmErrorCode::Conflict))
            })
        })
        .await;
    assert!(result.is_err());
    assert_eq!(CacheItem::count_all(&db).await.unwrap(), 0);
    assert_eq!(read_state(&db, "tenant-a").await.generation(), 0);
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
    sync.fail(claim, 102, RefreshFailure::InvalidCandidate, None)
        .await
        .unwrap();
    let state = read_state(&db, "tenant-a").await;
    assert_eq!(state.generation(), 0);
    assert_eq!(state.last_success(), None);
    assert_eq!(state.failure(), Some(RefreshFailure::InvalidCandidate));
    assert_eq!(state.next_attempt(), 112);
}

#[tokio::test]
async fn durable_claim_coalesces_and_restart_reclaims_without_old_worker_promotion() {
    let db = fixture().await;
    let sync = CacheSync::new(db.clone(), Arc::new(Store));
    let old = claimed(&sync, "tenant-a", 100).await;
    // A new coordinator represents a restarted/other process, no memory lock.
    let restarted = CacheSync::new(db.clone(), Arc::new(Store));
    assert!(matches!(
        restarted
            .claim(key("tenant-a"), policy(), 101, true, None)
            .await
            .unwrap(),
        RefreshAdmission::InProgress
    ));
    let other = claimed(&restarted, "tenant-b", 101).await;
    restarted
        .promote(other, 102, 0, None, None, |_, _| Box::pin(async { Ok(()) }))
        .await
        .unwrap();
    let new = claimed(&restarted, "tenant-a", 130).await;
    assert!(
        sync.promote(old, 131, 9, None, None, |_, _| Box::pin(async { Ok(()) }))
            .await
            .is_err()
    );
    restarted
        .promote(new, 131, 0, None, None, |_, _| Box::pin(async { Ok(()) }))
        .await
        .unwrap();
    assert_eq!(read_state(&db, "tenant-a").await.generation(), 1);
    assert_eq!(read_state(&db, "tenant-b").await.generation(), 1);
}

#[tokio::test]
async fn failure_preserves_success_and_backoff_survives_another_coordinator() {
    let db = fixture().await;
    let sync = CacheSync::new(db.clone(), Arc::new(Store));
    let claim = claimed(&sync, "tenant-a", 100).await;
    sync.promote(claim, 101, 5, Some(90), None, |_, _| {
        Box::pin(async { Ok(()) })
    })
    .await
    .unwrap();
    let claim = match sync
        .claim(key("tenant-a"), policy(), 106, true, None)
        .await
        .unwrap()
    {
        RefreshAdmission::Claimed(c) => c,
        x => panic!("{x:?}"),
    };
    sync.fail(claim, 107, RefreshFailure::Unavailable, None)
        .await
        .unwrap();
    let state = read_state(&db, "tenant-a").await;
    assert_eq!(state.generation(), 1);
    assert_eq!(state.rows(), 5);
    assert_eq!(state.last_success(), Some(101));
    assert_eq!(state.fresh_until(), Some(161));
    assert_eq!(state.source_observed_at(), Some(90));
    assert_eq!(state.next_attempt(), 117);
    let restarted = CacheSync::new(db.clone(), Arc::new(Store));
    assert!(matches!(
        restarted
            .claim(key("tenant-a"), policy(), 116, true, None)
            .await
            .unwrap(),
        RefreshAdmission::NotDue
    ));
    let claim = match restarted
        .claim(key("tenant-a"), policy(), 117, true, None)
        .await
        .unwrap()
    {
        RefreshAdmission::Claimed(c) => c,
        x => panic!("{x:?}"),
    };
    restarted
        .fail(claim, 118, RefreshFailure::Throttled, None)
        .await
        .unwrap();
    assert_eq!(read_state(&db, "tenant-a").await.next_attempt(), 138);
}

#[test]
fn budgets_and_policy_are_bounded() {
    let mut budget = RefreshBudget::new(2, 3, 10);
    assert!(budget.charge_page(4, 1).is_err());
    budget.charge_page(2, 5).unwrap();
    budget.charge_page(1, 5).unwrap();
    assert!(budget.charge_page(0, 0).is_err());
    assert!(policy().validate(i64::MAX).is_err());
    assert!(policy().validate(-1).is_err());
    assert!(CachePartitionKey::new("", "tenant").is_err());
    assert!(!CacheState::default().is_fresh(0));
}

struct TenantPolicy(Arc<std::sync::atomic::AtomicBool>);
impl RowPolicy for TenantPolicy {
    fn can_read_row<'a>(
        &'a self,
        ctx: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
        row: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move {
            if surface == EntityAccessSurface::Repository {
                return Ok(true);
            }
            let owner = row
                .downcast_ref::<DatasetCacheStatus>()
                .map(|r| r.id.as_str());
            let expected = key_id(&key("tenant-a"));
            Ok(self.0.load(std::sync::atomic::Ordering::SeqCst)
                && ctx
                    .and_then(|c| c.auth_subject_opt())
                    .is_some_and(|s| s.id == "tenant-a")
                && owner == Some(expected.as_str()))
        })
    }
    fn can_write_row<'a>(
        &'a self,
        _: Option<&'a async_graphql::Context<'_>>,
        _: &'a Database,
        _: &'static str,
        _: Option<&'static str>,
        surface: EntityAccessSurface,
        _: &'a (dyn std::any::Any + Send + Sync),
    ) -> BoxFuture<'a, async_graphql::Result<bool>> {
        Box::pin(async move { Ok(surface == EntityAccessSurface::Repository) })
    }
}

#[tokio::test]
async fn generated_cache_notification_rechecks_tenant_and_revoked_row_policy() {
    let mut db = fixture().await;
    let authority = Arc::new(std::sync::atomic::AtomicBool::new(true));
    db.set_row_policy(TenantPolicy(authority.clone()));
    let sync = CacheSync::new(db.clone(), Arc::new(Store));
    let a = claimed(&sync, "tenant-a", 100).await;
    let b = claimed(&sync, "tenant-b", 100).await;
    let schema = schema_builder(db.clone())
        .data(AuthSubject::new("tenant-a"))
        .finish();
    let mut stream = Box::pin(schema.execute_stream(
        "subscription { datasetCacheStatusChanged { id datasetCacheStatus { generation rows } } }",
    ));
    registered(&mut stream).await;
    sync.promote(b, 101, 7, None, None, |_, _| Box::pin(async { Ok(()) }))
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
    sync.promote(a, 101, 1, None, None, |_, _| Box::pin(async { Ok(()) }))
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["datasetCacheStatusChanged"]["datasetCacheStatus"]["generation"],
        1
    );
    authority.store(false, std::sync::atomic::Ordering::SeqCst);
    let a = match sync
        .claim(key("tenant-a"), policy(), 106, true, None)
        .await
        .unwrap()
    {
        RefreshAdmission::Claimed(c) => c,
        x => panic!("{x:?}"),
    };
    sync.promote(a, 107, 1, None, None, |_, _| Box::pin(async { Ok(()) }))
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
}
