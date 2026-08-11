#![cfg(feature = "sqlite")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, Clock, CurrentPrincipalResolver, FixedClock,
    PrincipalReference, ResolvedPrincipal, SessionContext,
};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::prelude::{Database, SqliteBackend};
use graphql_orm_ai::*;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

struct ToggleAccess(Arc<AtomicBool>);

#[async_trait]
impl AiAccessPolicy for ToggleAccess {
    async fn can_access_scope(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        if self.0.load(Ordering::SeqCst) {
            AiAccessDecision::allow("run-cancellation-test", "v1")
        } else {
            AiAccessDecision::deny("permission_removed", "v1")
        }
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        if self.0.load(Ordering::SeqCst) {
            AiAccessDecision::allow("run-cancellation-test", "v1")
        } else {
            AiAccessDecision::deny("permission_removed", "v1")
        }
    }
}

struct ProtectionPolicy;

#[async_trait]
impl AiContentProtectionPolicyResolver for ProtectionPolicy {
    async fn resolve(
        &self,
        _principal: &AuthPrincipal,
        scope: &AiScope,
    ) -> Result<AiContentProtectionPolicy, AiError> {
        Ok(AiContentProtectionPolicy {
            scope: scope.clone(),
            mode: AiContentProtectionMode::DatabaseManaged,
            key_policy_reference: None,
            version: 1,
            ready: true,
        })
    }
}

struct TogglePrincipalResolver {
    principal: AuthPrincipal,
    active: Arc<AtomicBool>,
    clock: Arc<FixedClock>,
}

#[async_trait]
impl CurrentPrincipalResolver for TogglePrincipalResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::Forbidden);
        }
        ResolvedPrincipal::new(reference.clone(), self.principal.clone(), self.clock.now())
    }
}

fn principal(subject: &str) -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: subject.to_owned(),
        session_id: Uuid::new_v4(),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata {
            tenant_id: Some("tenant-cancel".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

struct Fixture {
    sessions: OrmAiSessionService,
    runs: OrmAiRunService,
    cancellation: Arc<OrmAiRunCancellationService>,
    owner: AuthPrincipal,
    access_allowed: Arc<AtomicBool>,
    principal_active: Arc<AtomicBool>,
}

async fn fixture() -> Fixture {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-run-cancellation-test-v1",
            "AI run cancellation test",
            module.entities(),
        )
        .await
        .expect("AI schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema migration should apply");

    let owner = principal("run-owner");
    let clock = Arc::new(FixedClock::new(
        OffsetDateTime::now_utc() + Duration::seconds(2),
    ));
    let access_allowed = Arc::new(AtomicBool::new(true));
    let principal_active = Arc::new(AtomicBool::new(true));
    let access_policy: Arc<dyn AiAccessPolicy> = Arc::new(ToggleAccess(access_allowed.clone()));
    let protection_policy: Arc<dyn AiContentProtectionPolicyResolver> = Arc::new(ProtectionPolicy);
    let content_protector: Arc<dyn AiContentProtector> = Arc::new(DatabaseManagedContentProtector);
    let principal_resolver: Arc<dyn CurrentPrincipalResolver> = Arc::new(TogglePrincipalResolver {
        principal: owner.clone(),
        active: principal_active.clone(),
        clock: clock.clone(),
    });
    let hub = Arc::new(AiRunCancellationHub::new(32).expect("hub limits should validate"));
    let sessions = OrmAiSessionService::new(
        database.clone(),
        access_policy.clone(),
        protection_policy.clone(),
        content_protector.clone(),
    );
    let runs = OrmAiRunService::new(
        database.clone(),
        clock.clone(),
        AiRunServiceLimits::new(Duration::minutes(1), Duration::minutes(1), 16, 3, 3)
            .expect("run limits should validate"),
    )
    .with_cancellation_hub(hub.clone());
    let cancellation = Arc::new(OrmAiRunCancellationService::new(
        database,
        access_policy,
        protection_policy,
        content_protector,
        principal_resolver,
        clock,
        AiRunCancellationLimits::default(),
        hub,
    ));
    Fixture {
        sessions,
        runs,
        cancellation,
        owner,
        access_allowed,
        principal_active,
    }
}

async fn active_run(fixture: &Fixture) -> (AiSessionView, AiRunLease) {
    let session = fixture
        .sessions
        .create_session(
            &fixture.owner,
            CreateAiSessionInput {
                scope: AiScopeInput {
                    kind: "workspace".to_owned(),
                    id: "workspace-cancel".to_owned(),
                    tenant_id: Some("tenant-cancel".to_owned()),
                },
                title: None,
            },
        )
        .await
        .expect("session should create");
    let sent = fixture
        .sessions
        .send_message(
            &fixture.owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Count my records".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message should enqueue a run");
    let claimed = fixture
        .runs
        .claim_next("run-cancellation-test-worker")
        .await
        .expect("claim should succeed")
        .expect("queued run should exist");
    assert_eq!(claimed.run_id(), AiRunId(sent.run_id));
    let running = fixture
        .runs
        .start(&claimed)
        .await
        .expect("run should start");
    (session, running)
}

#[tokio::test]
async fn owner_cancellation_is_idempotent_durable_and_observable() {
    let fixture = fixture().await;
    let (session, running) = active_run(&fixture).await;
    let client_request_id = Uuid::new_v4();
    let input = CancelAiRunInput {
        session_id: session.id,
        run_id: running.run_id().0,
        client_request_id,
    };
    let first = fixture
        .cancellation
        .request_cancellation(&fixture.owner, input.clone())
        .await
        .expect("owner should cancel the active run");
    let replay = fixture
        .cancellation
        .request_cancellation(&fixture.owner, input)
        .await
        .expect("same cancellation request should replay");
    assert_eq!(first.session_id, replay.session_id);
    assert_eq!(first.run_id, replay.run_id);
    assert_eq!(first.client_request_id, replay.client_request_id);
    assert_eq!(first.requested_at, replay.requested_at);
    assert_eq!(first.state, "cancelled");

    let observed = fixture
        .runs
        .cancellation(&running)
        .await
        .expect("cancellation lookup should succeed")
        .expect("durable marker should be visible");
    assert_eq!(observed.client_request_id(), client_request_id);
    let events = fixture
        .sessions
        .session_event_page(&fixture.owner, AiSessionId(session.id), 0, 20)
        .await
        .expect("owner should replay events");
    let event_types = events
        .events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        event_types,
        [
            "message_queued",
            "run_cancellation_requested",
            "run_cancelled"
        ]
    );
}

#[tokio::test]
async fn cancellation_rechecks_current_principal_and_permissions() {
    let fixture = fixture().await;
    let (session, running) = active_run(&fixture).await;
    let input = CancelAiRunInput {
        session_id: session.id,
        run_id: running.run_id().0,
        client_request_id: Uuid::new_v4(),
    };

    fixture.access_allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .cancellation
            .request_cancellation(&fixture.owner, input.clone())
            .await,
        Err(AiError::Forbidden)
    ));
    fixture.access_allowed.store(true, Ordering::SeqCst);
    fixture.principal_active.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .cancellation
            .request_cancellation(&fixture.owner, input)
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
}

#[tokio::test]
async fn cancellation_wakes_the_fenced_worker_and_wrong_pairs_fail_closed() {
    let fixture = fixture().await;
    let (session, running) = active_run(&fixture).await;
    let wrong_pair = CancelAiRunInput {
        session_id: Uuid::new_v4(),
        run_id: running.run_id().0,
        client_request_id: Uuid::new_v4(),
    };
    assert!(matches!(
        fixture
            .cancellation
            .request_cancellation(&fixture.owner, wrong_pair)
            .await,
        Err(AiError::NotFound)
    ));

    let wait = fixture
        .runs
        .wait_for_cancellation(&running, std::time::Duration::from_secs(5));
    let request = fixture.cancellation.request_cancellation(
        &fixture.owner,
        CancelAiRunInput {
            session_id: session.id,
            run_id: running.run_id().0,
            client_request_id: Uuid::new_v4(),
        },
    );
    let (observed, view) = tokio::join!(wait, request);
    assert!(observed.expect("wait should succeed").is_some());
    assert_eq!(view.expect("request should succeed").state, "cancelled");
}
