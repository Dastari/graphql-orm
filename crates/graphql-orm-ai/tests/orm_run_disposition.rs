#![cfg(feature = "sqlite")]
//! Owner-authorized retry and acknowledgement of failed runs.

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

struct AllowAll;

#[async_trait]
impl AiAccessPolicy for AllowAll {
    async fn can_access_scope(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("run-disposition-test", "v1")
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("run-disposition-test", "v1")
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

struct StaticResolver {
    principal: AuthPrincipal,
    active: Arc<AtomicBool>,
    clock: Arc<FixedClock>,
}

#[async_trait]
impl CurrentPrincipalResolver for StaticResolver {
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
            tenant_id: Some("tenant-disposition".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

struct Fixture {
    sessions: Arc<OrmAiSessionService>,
    runs: OrmAiRunService,
    dispositions: OrmAiRunDispositionService,
    owner: AuthPrincipal,
    active: Arc<AtomicBool>,
}

async fn fixture_on(database: Database<SqliteBackend>, migrate: bool) -> Fixture {
    if migrate {
        let module = AiSchemaModule;
        let plan = database
            .schema()
            .plan_migration_to_entities(
                "ai-run-disposition-test-v1",
                "AI run disposition test",
                module.entities(),
            )
            .await
            .expect("AI schema migration should plan");
        database
            .schema()
            .apply_migration(&plan, ApplyOptions::default())
            .await
            .expect("AI schema migration should apply");
    }
    let owner = principal("disposition-owner");
    let clock = Arc::new(FixedClock::new(OffsetDateTime::now_utc()));
    let active = Arc::new(AtomicBool::new(true));
    let access_policy: Arc<dyn AiAccessPolicy> = Arc::new(AllowAll);
    let protection_policy: Arc<dyn AiContentProtectionPolicyResolver> = Arc::new(ProtectionPolicy);
    let content_protector: Arc<dyn AiContentProtector> = Arc::new(DatabaseManagedContentProtector);
    let principal_resolver: Arc<dyn CurrentPrincipalResolver> = Arc::new(StaticResolver {
        principal: owner.clone(),
        active: active.clone(),
        clock: clock.clone(),
    });
    let sessions = Arc::new(OrmAiSessionService::new(
        database.clone(),
        access_policy.clone(),
        protection_policy.clone(),
        content_protector.clone(),
    ));
    let runs = OrmAiRunService::new(
        database.clone(),
        clock.clone(),
        AiRunServiceLimits::new(Duration::minutes(1), Duration::minutes(1), 16, 3, 3)
            .expect("run limits should validate"),
    );
    let dispositions = OrmAiRunDispositionService::new(
        database,
        access_policy,
        protection_policy,
        content_protector,
        principal_resolver,
        clock,
        AiRunDispositionLimits::default(),
    );
    Fixture {
        sessions,
        runs,
        dispositions,
        owner,
        active,
    }
}

async fn fixture() -> Fixture {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    fixture_on(database, true).await
}

async fn session(fixture: &Fixture) -> AiSessionView {
    fixture
        .sessions
        .create_session(
            &fixture.owner,
            CreateAiSessionInput {
                scope: AiScopeInput {
                    kind: "workspace".to_owned(),
                    id: "workspace-disposition".to_owned(),
                    tenant_id: Some("tenant-disposition".to_owned()),
                },
                title: None,
            },
        )
        .await
        .expect("session should create")
}

/// Drives one run to a terminal state with the supplied outcome/error code.
async fn failed_run(
    fixture: &Fixture,
    session_id: Uuid,
    final_state: AiRunState,
    outcome_code: &str,
    error_code: Option<&str>,
) -> SendAiMessagePayload {
    let sent = fixture
        .sessions
        .send_message(
            &fixture.owner,
            SendAiMessageInput {
                session_id,
                text: "Count my records".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message should enqueue a run");
    let claimed = fixture
        .runs
        .claim_next("run-disposition-test-worker")
        .await
        .expect("claim should succeed")
        .expect("queued run should exist");
    let running = fixture
        .runs
        .start(&claimed)
        .await
        .expect("run should start");
    fixture
        .runs
        .finish(
            &running,
            AiRunCompletion::new(
                final_state,
                outcome_code,
                error_code.map(str::to_owned),
                None,
            )
            .expect("completion should validate"),
        )
        .await
        .expect("terminal write should commit");
    sent
}

fn failure_record(page: &AiSessionEventPage, event_type: &str) -> serde_json::Value {
    let event = page
        .events
        .iter()
        .find(|event| event.event_type == event_type)
        .unwrap_or_else(|| panic!("{event_type} should be durable"));
    event.payload.0["failure"].clone()
}

#[tokio::test]
async fn retry_authors_a_new_run_over_the_same_message_and_is_idempotent() {
    let fixture = fixture().await;
    let session = session(&fixture).await;
    let sent = failed_run(
        &fixture,
        session.id,
        AiRunState::Failed,
        "agent_rule_budget_exceeded",
        Some("agent_rule_budget_exceeded"),
    )
    .await;

    let client_request_id = Uuid::new_v4();
    let input = RetryAiRunInput {
        session_id: session.id,
        run_id: sent.run_id,
        client_request_id,
    };
    let first = fixture
        .dispositions
        .retry_run(&fixture.owner, input.clone())
        .await
        .expect("a proven-clean failure should admit a retry");
    assert_eq!(first.disposition, AiRunDisposition::Retried);
    assert_eq!(first.input_message_id, sent.message_id);
    let retry_run_id = first.retry_run_id.expect("retry should author a new run");
    assert_ne!(retry_run_id, sent.run_id);

    // Replaying the same key must not author a second run.
    let replay = fixture
        .dispositions
        .retry_run(&fixture.owner, input)
        .await
        .expect("the same idempotency key should replay");
    assert_eq!(replay.retry_run_id, Some(retry_run_id));
    assert_eq!(replay.decided_at, first.decided_at);

    // A different key for the same already-disposed run is refused.
    assert!(matches!(
        fixture
            .dispositions
            .retry_run(
                &fixture.owner,
                RetryAiRunInput {
                    session_id: session.id,
                    run_id: sent.run_id,
                    client_request_id: Uuid::new_v4(),
                },
            )
            .await,
        Err(AiError::Conflict)
    ));

    // The new run is queued over the same durable user message and is claimable.
    let bootstrap = fixture
        .sessions
        .conversation_bootstrap(&fixture.owner, AiSessionId(session.id), 20, 20, 100)
        .await
        .expect("bootstrap should succeed");
    let queued = bootstrap
        .active_runs
        .iter()
        .find(|run| run.id == retry_run_id)
        .expect("the retry run should be active");
    assert_eq!(queued.state, "queued");
    assert_eq!(queued.input_message_id, sent.message_id);
    assert_eq!(
        bootstrap.messages.len(),
        1,
        "retry must not duplicate the prompt"
    );

    // The source run stays terminal: retry never resurrects it.
    let source = bootstrap
        .terminal_runs
        .iter()
        .find(|run| run.id == sent.run_id)
        .expect("the source run should stay terminal");
    assert_eq!(source.state, "failed");
}

#[tokio::test]
async fn recovery_required_refuses_retry_but_still_admits_acknowledgement() {
    let fixture = fixture().await;
    let session = session(&fixture).await;
    let sent = failed_run(
        &fixture,
        session.id,
        AiRunState::RecoveryRequired,
        "provider_turn_uncertain",
        Some("provider_turn_uncertain"),
    )
    .await;

    assert!(
        matches!(
            fixture
                .dispositions
                .retry_run(
                    &fixture.owner,
                    RetryAiRunInput {
                        session_id: session.id,
                        run_id: sent.run_id,
                        client_request_id: Uuid::new_v4(),
                    },
                )
                .await,
            Err(AiError::Conflict)
        ),
        "an unproven external effect must never be re-executed"
    );

    let acknowledged = fixture
        .dispositions
        .acknowledge_run_failure(
            &fixture.owner,
            AcknowledgeAiRunFailureInput {
                session_id: session.id,
                run_id: sent.run_id,
                client_request_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("dismissing a failure asserts nothing about re-execution safety");
    assert_eq!(acknowledged.disposition, AiRunDisposition::Acknowledged);
    assert!(acknowledged.retry_run_id.is_none());

    // Audit history survives the dismissal.
    let page = fixture
        .sessions
        .session_event_page(&fixture.owner, AiSessionId(session.id), 0, 500)
        .await
        .expect("events should replay");
    assert!(
        page.events
            .iter()
            .any(|event| event.event_type == "run_recovery_required")
    );
    assert!(
        page.events
            .iter()
            .any(|event| event.event_type == "run_failure_acknowledged")
    );
}

#[tokio::test]
async fn an_unclassified_failure_is_not_retryable() {
    let fixture = fixture().await;
    let session = session(&fixture).await;
    let sent = failed_run(
        &fixture,
        session.id,
        AiRunState::Failed,
        "worker_stopped",
        None,
    )
    .await;

    let page = fixture
        .sessions
        .session_event_page(&fixture.owner, AiSessionId(session.id), 0, 500)
        .await
        .expect("events should replay");
    let failure = failure_record(&page, "run_failed");
    assert_eq!(failure["retryable"], serde_json::json!(false));
    assert_eq!(failure["admission"], serde_json::json!("refused_uncertain"));
    assert_eq!(failure["code"], serde_json::Value::Null);

    assert!(matches!(
        fixture
            .dispositions
            .retry_run(
                &fixture.owner,
                RetryAiRunInput {
                    session_id: session.id,
                    run_id: sent.run_id,
                    client_request_id: Uuid::new_v4(),
                },
            )
            .await,
        Err(AiError::Conflict)
    ));
}

#[tokio::test]
async fn a_revoked_principal_cannot_dispose_of_a_failure() {
    let fixture = fixture().await;
    let session = session(&fixture).await;
    let sent = failed_run(
        &fixture,
        session.id,
        AiRunState::Failed,
        "agent_rule_budget_exceeded",
        Some("agent_rule_budget_exceeded"),
    )
    .await;
    fixture.active.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .dispositions
            .retry_run(
                &fixture.owner,
                RetryAiRunInput {
                    session_id: session.id,
                    run_id: sent.run_id,
                    client_request_id: Uuid::new_v4(),
                },
            )
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
}

/// Work item 3: a failed run's terminal event and its bounded failure record
/// must survive a host restart and replay to a reconnecting client.
#[tokio::test]
async fn failed_run_events_replay_with_their_failure_record_after_restart() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("run-disposition.sqlite");
    let url = format!("sqlite://{}?mode=rwc", path.display());

    let session_id;
    let run_id;
    {
        let database = Database::<SqliteBackend>::connect_sqlite(&url)
            .await
            .expect("database opens");
        let fixture = fixture_on(database, true).await;
        let view = session(&fixture).await;
        session_id = view.id;
        run_id = failed_run(
            &fixture,
            session_id,
            AiRunState::Failed,
            "agent_rule_budget_exceeded",
            Some("agent_rule_budget_exceeded"),
        )
        .await
        .run_id;
    }

    // Fresh process: new handles, no in-memory state carried over.
    let database = Database::<SqliteBackend>::connect_sqlite(&url)
        .await
        .expect("database reopens");
    let fixture = fixture_on(database, false).await;
    let page = fixture
        .sessions
        .session_event_page(&fixture.owner, AiSessionId(session_id), 0, 500)
        .await
        .expect("durable events should replay after a restart");
    assert!(!page.reset_required);
    let failure = failure_record(&page, "run_failed");
    assert_eq!(failure["version"], serde_json::json!(1));
    assert_eq!(failure["ok"], serde_json::json!(false));
    assert_eq!(failure["retryable"], serde_json::json!(true));
    assert_eq!(failure["admission"], serde_json::json!("allowed"));
    assert_eq!(
        failure["code"],
        serde_json::json!("agent_rule_budget_exceeded")
    );

    // The flag is authoritative: the retry it advertises is actually admitted.
    let disposition = fixture
        .dispositions
        .retry_run(
            &fixture.owner,
            RetryAiRunInput {
                session_id,
                run_id,
                client_request_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("an advertised retryable failure must be retryable");
    assert_eq!(disposition.disposition, AiRunDisposition::Retried);
}
