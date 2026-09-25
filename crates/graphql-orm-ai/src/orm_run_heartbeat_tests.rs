#![cfg(feature = "sqlite")]
//! Lease renewal under real temporary SQLite writer contention.

use std::sync::Arc;

use super::*;
use crate::AiSchemaModule;
use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, Clock, FixedClock, SessionContext};
use graphql_orm::db::ConnectionOptions;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule, TransactionMode};
use graphql_orm::prelude::{Database, SqliteBackend};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

struct Fixture {
    _directory: tempfile::TempDir,
    database: Database<SqliteBackend>,
    runs: OrmAiRunService,
    clock: FixedClock,
    lease: AiRunLease,
}

async fn fixture(retries: usize) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::<SqliteBackend>::connect_sqlite_with_options(
        format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("heartbeat.sqlite").display()
        ),
        ConnectionOptions::default().max_connections(8),
    )
    .await
    .unwrap();
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "heartbeat-contention",
            "Heartbeat contention",
            AiSchemaModule.entities(),
        )
        .await
        .unwrap();
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .unwrap();
    let clock = FixedClock::new(OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap());
    let principal = AuthPrincipal::User(AuthUser {
        user_id: "heartbeat-owner".into(),
        session_id: Uuid::new_v4(),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata::default(),
    });
    let session_id = Uuid::new_v4();
    AiSessionRecord::insert(
        &database,
        CreateAiSessionRecordInput {
            execution_selection: None,
            id: session_id,
            owner_principal_kind: "user".into(),
            owner_subject: "heartbeat-owner".into(),
            tenant_id: None,
            scope_kind: "user".into(),
            scope_id: "heartbeat-owner".into(),
            title: "Synthetic heartbeat".into(),
            title_revision: 0,
            title_source: "default".into(),
            state: "active".into(),
            stream_head: 0,
            message_head: 0,
            last_activity_at: clock.now().unix_timestamp(),
            archived_at: None,
            deleted_at: None,
        },
    )
    .await
    .unwrap();
    AiRunRecord::insert(
        &database,
        CreateAiRunRecordInput {
            execution_selection: None,
            id: Uuid::new_v4(),
            session_id,
            input_message_id: Uuid::new_v4(),
            principal_reference: serde_json::to_value(principal.reference()).unwrap(),
            state: "queued".into(),
            attempt_id: None,
            lease_owner: None,
            lease_generation: 0,
            lease_expires_at: None,
            lease_heartbeat_at: None,
            retry_count: 0,
            next_attempt_at: Some(clock.now().unix_timestamp()),
            error_code: None,
            latest_checkpoint_id: None,
            cancellation_request_id: None,
            cancellation_requested_at: None,
        },
    )
    .await
    .unwrap();
    let runs = OrmAiRunService::new(
        database.clone(),
        Arc::new(clock.clone()),
        AiRunServiceLimits::new(Duration::seconds(60), Duration::hours(1), 16, 2, retries).unwrap(),
    );
    let lease = runs.claim_next("heartbeat-worker").await.unwrap().unwrap();
    let lease = runs.start(&lease).await.unwrap();
    Fixture {
        _directory: directory,
        database,
        runs,
        clock,
        lease,
    }
}

async fn hold_writer(fixture: &Fixture, expire: bool) -> tokio::task::JoinHandle<()> {
    let database = fixture.database.clone();
    let clock = fixture.clock.clone();
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        database
            .transaction(TransactionMode::StateMachine, move |_tx| {
                Box::pin(async move {
                    ready.send(()).unwrap();
                    // SQLx's SQLite busy timeout is five seconds. Force the first
                    // BEGIN IMMEDIATE to exhaust it, then release for a bounded retry.
                    tokio::time::sleep(std::time::Duration::from_millis(5_500)).await;
                    if expire {
                        clock.advance_seconds(61);
                    }
                    Ok(())
                })
            })
            .await
            .unwrap();
    });
    waiting.await.unwrap();
    writer
}

#[tokio::test]
async fn heartbeat_recovers_from_retryable_writer_contention() {
    let fixture = fixture(2).await;
    fixture.clock.advance_seconds(1);
    let writer = hold_writer(&fixture, false).await;
    let renewed = fixture.runs.heartbeat(&fixture.lease).await;
    writer.await.unwrap();
    let renewed = renewed.expect("retryable contention must not abandon the active turn");
    assert_eq!(renewed.run_id(), fixture.lease.run_id());
    assert_eq!(renewed.attempt_id(), fixture.lease.attempt_id());
    assert_eq!(renewed.lease_generation(), fixture.lease.lease_generation());
    assert!(renewed.lease_expires_at() > fixture.lease.lease_expires_at());
    assert!(matches!(
        fixture.runs.heartbeat(&fixture.lease).await,
        Err(AiError::Conflict)
    ));
}

#[tokio::test]
async fn heartbeat_retry_does_not_revive_a_lease_that_expires_during_contention() {
    let fixture = fixture(2).await;
    let writer = hold_writer(&fixture, true).await;
    let result = fixture.runs.heartbeat(&fixture.lease).await;
    writer.await.unwrap();
    assert!(matches!(result, Err(AiError::Conflict)));
}

#[tokio::test]
async fn exhausted_heartbeat_retries_leave_the_original_fence_unchanged() {
    let fixture = fixture(0).await;
    let before = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
        .await
        .unwrap()
        .unwrap();
    let writer = hold_writer(&fixture, false).await;
    let result = fixture.runs.heartbeat(&fixture.lease).await;
    writer.await.unwrap();
    assert!(matches!(result, Err(AiError::PersistenceFailed)));
    let after = AiRunRecord::find_by_id(&fixture.database, &fixture.lease.run_id().0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.row_version, after.row_version);
    assert_eq!(before.lease_expires_at, after.lease_expires_at);
    assert_eq!(before.attempt_id, after.attempt_id);
}
