//! Host-bound principal projection through the public services, on SQLite and
//! on a test-owned disposable PostgreSQL server.
//!
//! One strict resolver accepts only references carrying the host namespace,
//! as a host that verified the bearer audience would. Every admitting service
//! must therefore persist or resolve the projected reference.
#![cfg(any(
    all(feature = "sqlite", not(any(feature = "postgres", feature = "mssql"))),
    all(feature = "postgres", not(any(feature = "sqlite", feature = "mssql")))
))]

#[cfg(feature = "postgres")]
#[allow(dead_code)]
#[path = "../../graphql-orm/tests/support/owned_postgres.rs"]
mod owned_postgres;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agql_auth::{
    AccessTokenGrantKind, AccessTokenMetadata, AssuranceMatchMode, AuthPrincipal, AuthUser, Clock,
    CurrentPrincipalResolver, FixedClock, PrincipalReference, RecentMfaPolicy, ResolvedPrincipal,
    SessionContext,
};
use async_trait::async_trait;
use futures::StreamExt;
use graphql_orm::graphql::orm::{ApplyOptions, DefaultWriteBackend, OrmSchemaModule};
use graphql_orm::graphql::pagination::KeysetConnectionInput;
use graphql_orm::prelude::Database;
use graphql_orm_ai::*;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const NAMESPACE: &str = "host:admin-clients";
const TENANT: &str = "tenant-projection";

struct AllowAll;

#[async_trait]
impl AiAccessPolicy for AllowAll {
    async fn can_access_scope(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("principal-projection-test", "v1")
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("principal-projection-test", "v1")
    }
}

#[async_trait]
impl AiProposalAccessPolicy for AllowAll {
    async fn can_access_proposal(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _session_id: AiSessionId,
        _action: AiProposalAction,
    ) -> bool {
        true
    }
}

#[async_trait]
impl AiApprovalAccessPolicy for AllowAll {
    async fn can_access_approval(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _session_id: AiSessionId,
        _action: AiApprovalAction,
    ) -> bool {
        true
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

/// Accepts only a reference stamped with its own namespace, then pairs it with
/// the authoritative user exactly like a host's audience-checking resolver.
struct StrictResolver {
    namespace: Option<&'static str>,
    authoritative: AuthPrincipal,
    active: Arc<AtomicBool>,
    clock: Arc<FixedClock>,
}

#[async_trait]
impl CurrentPrincipalResolver for StrictResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        if !self.active.load(Ordering::SeqCst) || reference.audience.as_deref() != self.namespace {
            return Err(agql_auth::AuthError::Forbidden);
        }
        ResolvedPrincipal::new(
            reference.clone(),
            self.authoritative.clone(),
            self.clock.now(),
        )
    }
}

fn owner() -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: "projection-owner".to_owned(),
        session_id: Uuid::from_u128(0x5e55),
        roles: vec![],
        scopes: vec![],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata {
            jti: Some("access-token-1".to_owned()),
            session_family_id: Some("family-1".to_owned()),
            grant_kind: Some(AccessTokenGrantKind::UserSession),
            session_version: Some("security-v1".to_owned()),
            tenant_id: Some(TENANT.to_owned()),
            expires_at: Some(OffsetDateTime::now_utc() + Duration::hours(1)),
            correlation_id: Some("request-1".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

fn projection(namespace: &str) -> Arc<AiPrincipalProjection> {
    Arc::new(AiPrincipalProjection::fixed(
        AiPrincipalNamespace::new(namespace).expect("namespace should validate"),
    ))
}

#[cfg(feature = "sqlite")]
async fn database(_label: &str) -> Database<DefaultWriteBackend> {
    Database::<graphql_orm::prelude::SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open")
}

#[cfg(feature = "postgres")]
async fn database(label: &str) -> Database<DefaultWriteBackend> {
    let pool = owned_postgres::OwnedPostgres::pool(label, 5)
        .await
        .expect("test-owned PostgreSQL should start");
    Database::<graphql_orm::prelude::PostgresBackend>::new(pool)
}

struct Fixture {
    database: Database<DefaultWriteBackend>,
    owner: AuthPrincipal,
    projection: Arc<AiPrincipalProjection>,
    resolver: Arc<dyn CurrentPrincipalResolver>,
    clock: Arc<FixedClock>,
    sessions: Arc<OrmAiSessionService>,
    runs: OrmAiRunService,
    title_work: OrmAiSessionTitleWorkService,
}

fn access() -> Arc<dyn AiAccessPolicy> {
    Arc::new(AllowAll)
}
fn protection() -> Arc<dyn AiContentProtectionPolicyResolver> {
    Arc::new(ProtectionPolicy)
}
fn protector() -> Arc<dyn AiContentProtector> {
    Arc::new(DatabaseManagedContentProtector)
}

async fn fixture(label: &str) -> Fixture {
    let database = database(label).await;
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-principal-projection-v1",
            "AI principal projection",
            module.entities(),
        )
        .await
        .expect("AI schema should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema should apply");
    // The session service stamps run eligibility from the real clock; lead it
    // so a queued run is claimable regardless of the second boundary.
    // Whole seconds: proposal and approval admission compare against a
    // canonical second.
    let clock = Arc::new(FixedClock::new(
        OffsetDateTime::from_unix_timestamp(
            (OffsetDateTime::now_utc() + Duration::seconds(2)).unix_timestamp(),
        )
        .expect("valid timestamp"),
    ));
    let owner = owner();
    let resolver: Arc<dyn CurrentPrincipalResolver> = Arc::new(StrictResolver {
        namespace: Some(NAMESPACE),
        authoritative: owner.clone(),
        active: Arc::new(AtomicBool::new(true)),
        clock: clock.clone(),
    });
    let projection = projection(NAMESPACE);
    let sessions = Arc::new(
        OrmAiSessionService::new(database.clone(), access(), protection(), protector())
            .with_principal_projection(projection.clone())
            .with_submission_admission(resolver.clone(), clock.clone()),
    );
    let runs = OrmAiRunService::new(
        database.clone(),
        clock.clone(),
        AiRunServiceLimits::new(Duration::minutes(1), Duration::minutes(1), 16, 3, 3)
            .expect("run limits should validate"),
    );
    let title_work = OrmAiSessionTitleWorkService::new(
        database.clone(),
        access(),
        protection(),
        protector(),
        resolver.clone(),
        clock.clone(),
        AiSessionTitleWorkLimits::new(
            Duration::minutes(1),
            Duration::minutes(5),
            Duration::minutes(1),
            16,
            3,
            3,
            256,
            64 * 1024,
        )
        .expect("title limits should validate"),
    );
    Fixture {
        database,
        owner,
        projection,
        resolver,
        clock,
        sessions,
        runs,
        title_work,
    }
}

impl Fixture {
    fn stamped(&self) -> PrincipalReference {
        self.projection
            .project(&self.owner)
            .expect("owner should project")
    }

    fn dispositions(&self, projected: bool) -> OrmAiRunDispositionService {
        let service = OrmAiRunDispositionService::new(
            self.database.clone(),
            access(),
            protection(),
            protector(),
            self.resolver.clone(),
            self.clock.clone(),
            AiRunDispositionLimits::default(),
        );
        if projected {
            service.with_principal_projection(self.projection.clone())
        } else {
            service
        }
    }

    fn cancellations(&self, projected: bool) -> OrmAiRunCancellationService {
        let service = OrmAiRunCancellationService::new(
            self.database.clone(),
            access(),
            protection(),
            protector(),
            self.resolver.clone(),
            self.clock.clone(),
            AiRunCancellationLimits::new(Duration::minutes(1), 4).expect("limits"),
            Arc::new(AiRunCancellationHub::new(16).expect("hub")),
        );
        if projected {
            service.with_principal_projection(self.projection.clone())
        } else {
            service
        }
    }

    async fn session(&self) -> AiSessionView {
        self.sessions
            .create_session(
                &self.owner,
                CreateAiSessionInput {
                    execution_selection: None,
                    scope: AiScopeInput {
                        kind: "workspace".to_owned(),
                        id: "workspace-projection".to_owned(),
                        tenant_id: Some(TENANT.to_owned()),
                    },
                    title: None,
                },
            )
            .await
            .expect("session should create")
    }

    async fn send(
        &self,
        sessions: &OrmAiSessionService,
        session_id: Uuid,
        client_message_id: Uuid,
    ) -> Result<SendAiMessagePayload, AiError> {
        sessions
            .send_message(
                &self.owner,
                SendAiMessageInput {
                    session_id,
                    text: "Count the active customer records".to_owned(),
                    attachment_ids: vec![],
                    client_message_id,
                },
            )
            .await
    }

    async fn stream_head(&self, session_id: Uuid) -> i64 {
        self.sessions
            .session(&self.owner, AiSessionId(session_id))
            .await
            .expect("session should read")
            .expect("session should exist")
            .stream_head
    }

    async fn fail_next_run(&self) -> AiRunLease {
        let claimed = self
            .runs
            .claim_next("projection-worker")
            .await
            .expect("claim should succeed")
            .expect("a queued run should exist");
        let running = self.runs.start(&claimed).await.expect("run should start");
        self.runs
            .finish(
                &running,
                AiRunCompletion::new(
                    AiRunState::Failed,
                    "agent_rule_budget_exceeded",
                    Some("agent_rule_budget_exceeded".to_owned()),
                    None,
                )
                .expect("completion should validate"),
            )
            .await
            .expect("terminal write should commit");
        claimed
    }
}

fn first_page() -> graphql_orm::graphql::pagination::ValidatedKeysetConnection {
    KeysetConnectionInput {
        first: Some(10),
        ..Default::default()
    }
    .validate(10, 50)
    .expect("page should validate")
}

#[tokio::test]
async fn send_title_work_and_duplicate_send_persist_the_stamped_reference() {
    let fixture = fixture("ai-projection-send").await;
    let stamped = fixture.stamped();
    let original = fixture.owner.reference();
    assert_eq!(stamped.audience.as_deref(), Some(NAMESPACE));
    assert_eq!(stamped.expires_at, original.expires_at);

    let session = fixture.session().await;
    let client_message_id = Uuid::new_v4();
    let sent = fixture
        .send(&fixture.sessions, session.id, client_message_id)
        .await
        .expect("projected submission should be admitted");
    let duplicate = fixture
        .send(&fixture.sessions, session.id, client_message_id)
        .await
        .expect("duplicate submission should replay");
    assert_eq!(duplicate.message_id, sent.message_id);
    assert_eq!(duplicate.run_id, sent.run_id);

    let lease = fixture
        .runs
        .claim_next("projection-worker")
        .await
        .expect("claim should succeed")
        .expect("exactly one run should be queued");
    assert_eq!(lease.run_id().0, sent.run_id);
    assert_eq!(lease.principal_reference(), &stamped);
    assert!(
        fixture
            .runs
            .claim_next("projection-worker-2")
            .await
            .expect("claim should succeed")
            .is_none(),
        "a duplicate send must not queue a second run"
    );

    let title = fixture
        .title_work
        .claim_next("title-worker")
        .await
        .expect("title claim should succeed")
        .expect("first message should create title work");
    assert_eq!(title.principal_reference(), &stamped);
    let input = fixture
        .title_work
        .open_first_message(&title)
        .await
        .expect("the strict resolver accepts the stamped title reference");
    assert_eq!(input.text(), "Count the active customer records");

    let serialized = serde_json::to_string(lease.principal_reference()).unwrap();
    for forbidden in ["roles", "scopes", "bearer", "token_hash"] {
        assert!(!serialized.contains(forbidden), "{serialized}");
    }
}

#[tokio::test]
async fn refused_submissions_write_no_message_run_or_title_work() {
    let fixture = fixture("ai-projection-denial").await;
    let session = fixture.session().await;
    let head = fixture.stream_head(session.id).await;

    let unprojected = OrmAiSessionService::new(
        fixture.database.clone(),
        access(),
        protection(),
        protector(),
    )
    .with_submission_admission(fixture.resolver.clone(), fixture.clock.clone());
    let foreign = OrmAiSessionService::new(
        fixture.database.clone(),
        access(),
        protection(),
        protector(),
    )
    .with_principal_projection(projection("host:other-clients"))
    .with_submission_admission(fixture.resolver.clone(), fixture.clock.clone());
    for service in [&unprojected, &foreign] {
        assert!(matches!(
            fixture.send(service, session.id, Uuid::new_v4()).await,
            Err(AiError::ReauthorizationFailed)
        ));
    }

    assert_eq!(fixture.stream_head(session.id).await, head);
    assert!(
        fixture
            .runs
            .claim_next("projection-worker")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .title_work
            .claim_next("title-worker")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn retry_acknowledge_and_cancel_resolve_the_stamped_reference() {
    let fixture = fixture("ai-projection-disposition").await;
    let stamped = fixture.stamped();
    let session = fixture.session().await;
    fixture
        .send(&fixture.sessions, session.id, Uuid::new_v4())
        .await
        .unwrap();
    let failed = fixture.fail_next_run().await;

    let retry = RetryAiRunInput {
        session_id: session.id,
        run_id: failed.run_id().0,
        client_request_id: Uuid::new_v4(),
    };
    assert!(matches!(
        fixture
            .dispositions(false)
            .retry_run(&fixture.owner, retry.clone())
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
    let dispositions = fixture.dispositions(true);
    let retried = dispositions
        .retry_run(&fixture.owner, retry.clone())
        .await
        .expect("projected explicit retry should be admitted");
    let replay = dispositions
        .retry_run(&fixture.owner, retry)
        .await
        .expect("retry replay should be idempotent");
    assert_eq!(replay.retry_run_id, retried.retry_run_id);
    let retried_lease = fixture
        .runs
        .claim_next("projection-worker")
        .await
        .unwrap()
        .expect("retry run should be queued");
    assert_eq!(Some(retried_lease.run_id().0), retried.retry_run_id);
    assert_eq!(retried_lease.principal_reference(), &stamped);

    let running = fixture.runs.start(&retried_lease).await.unwrap();
    fixture
        .runs
        .finish(
            &running,
            AiRunCompletion::new(
                AiRunState::Failed,
                "agent_rule_budget_exceeded",
                Some("agent_rule_budget_exceeded".to_owned()),
                None,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let acknowledged = dispositions
        .acknowledge_run_failure(
            &fixture.owner,
            AcknowledgeAiRunFailureInput {
                session_id: session.id,
                run_id: retried_lease.run_id().0,
                client_request_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("projected acknowledgement should be admitted");
    assert_eq!(acknowledged.disposition, AiRunDisposition::Acknowledged);
    assert_eq!(acknowledged.retry_run_id, None);

    let queued = fixture
        .send(&fixture.sessions, session.id, Uuid::new_v4())
        .await
        .unwrap();
    let cancel = CancelAiRunInput {
        session_id: session.id,
        run_id: queued.run_id,
        client_request_id: Uuid::new_v4(),
    };
    assert!(matches!(
        fixture
            .cancellations(false)
            .request_cancellation(&fixture.owner, cancel.clone())
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
    let cancelled = fixture
        .cancellations(true)
        .request_cancellation(&fixture.owner, cancel)
        .await
        .expect("projected cancellation should be admitted");
    assert_eq!(cancelled.run_id, queued.run_id);
}

#[tokio::test]
async fn streams_admit_the_stamped_reference_before_the_first_page() {
    let fixture = fixture("ai-projection-streams").await;
    let session = fixture.session().await;
    fixture
        .send(&fixture.sessions, session.id, Uuid::new_v4())
        .await
        .unwrap();

    let subscriptions =
        OrmAiSubscriptionService::new(fixture.sessions.clone(), fixture.resolver.clone());
    let mut events = subscriptions
        .session_events(fixture.owner.clone(), AiSessionId(session.id), 0)
        .await
        .expect("projected session stream should open");
    assert!(
        events
            .next()
            .await
            .expect("first item")
            .expect("first page should be delivered")
            .event
            .is_some()
    );

    let foreign_sessions = Arc::new(
        OrmAiSessionService::new(
            fixture.database.clone(),
            access(),
            protection(),
            protector(),
        )
        .with_principal_projection(projection("host:other-clients")),
    );
    let foreign = OrmAiSubscriptionService::new(foreign_sessions, fixture.resolver.clone());
    assert!(matches!(
        foreign
            .session_events(fixture.owner.clone(), AiSessionId(session.id), 0)
            .await,
        Err(AiError::ReauthorizationFailed)
    ));

    let inbox = |namespace: &str| {
        OrmAiInboxService::new(
            fixture.database.clone(),
            fixture.resolver.clone(),
            access(),
            protection(),
            protector(),
        )
        .with_principal_projection(projection(namespace))
    };
    let mut inbox_events = inbox(NAMESPACE)
        .inbox_events(fixture.owner.clone(), 0)
        .await
        .expect("projected inbox stream should open");
    assert!(inbox_events.next().await.expect("first inbox item").is_ok());
    assert!(matches!(
        inbox("host:other-clients")
            .inbox_events(fixture.owner.clone(), 0)
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
}

#[tokio::test]
async fn proposal_and_approval_reads_resolve_the_stamped_reference() {
    let fixture = fixture("ai-projection-review").await;
    let session = fixture.session().await;
    let proposals = |projected: bool| {
        let service = OrmAiProposalService::new(
            fixture.database.clone(),
            fixture.runs.clone(),
            fixture.resolver.clone(),
            Arc::new(AllowAll),
            protection(),
            protector(),
            Arc::new(AiProposalCatalog::new()),
            fixture.clock.clone(),
        );
        if projected {
            service.with_principal_projection(fixture.projection.clone())
        } else {
            service
        }
    };
    let approvals = |projected: bool| {
        let service = OrmAiApprovalService::new(
            fixture.database.clone(),
            fixture.runs.clone(),
            fixture.resolver.clone(),
            Arc::new(AllowAll),
            Arc::new(AiToolCatalog::new()),
            RecentMfaPolicy {
                maximum_age: Duration::minutes(5),
                clock_skew: Duration::seconds(30),
                allowed_amr: vec!["otp".to_owned()],
                allowed_acr: vec![],
                match_mode: AssuranceMatchMode::Any,
            },
            protection(),
            protector(),
            fixture.clock.clone(),
        );
        if projected {
            service.with_principal_projection(fixture.projection.clone())
        } else {
            service
        }
    };
    let id = AiSessionId(session.id);
    assert!(matches!(
        proposals(false)
            .proposals(&fixture.owner, id, first_page())
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
    assert!(
        proposals(true)
            .proposals(&fixture.owner, id, first_page())
            .await
            .expect("projected proposal read should be admitted")
            .edges
            .is_empty()
    );
    assert!(matches!(
        approvals(false)
            .approvals(&fixture.owner, id, first_page())
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
    assert!(
        approvals(true)
            .approvals(&fixture.owner, id, first_page())
            .await
            .expect("projected approval read should be admitted")
            .edges
            .is_empty()
    );
}

#[tokio::test]
async fn namespaces_never_substitute_and_defaults_are_unchanged() {
    let fixture = fixture("ai-projection-defaults").await;
    let owner = fixture.owner.clone();
    let a = projection("host:a").project(&owner).unwrap();
    let b = projection("host:b").project(&owner).unwrap();
    assert_ne!(a, b);
    let (mut a_identity, mut b_identity) = (a.clone(), b.clone());
    a_identity.audience = None;
    b_identity.audience = None;
    assert_eq!(a_identity, b_identity);
    let host_a = StrictResolver {
        namespace: Some("host:a"),
        authoritative: owner.clone(),
        active: Arc::new(AtomicBool::new(true)),
        clock: fixture.clock.clone(),
    };
    assert!(host_a.resolve(&a).await.is_ok());
    assert!(host_a.resolve(&b).await.is_err());
    assert!(host_a.resolve(&owner.reference()).await.is_err());

    // A default consumer, with no projection and no issuer, still persists the
    // unprojected reference.
    let session = fixture.session().await;
    let plain = OrmAiSessionService::new(
        fixture.database.clone(),
        access(),
        protection(),
        protector(),
    );
    fixture
        .send(&plain, session.id, Uuid::new_v4())
        .await
        .expect("default submission is unchanged");
    let lease = fixture
        .runs
        .claim_next("projection-worker")
        .await
        .unwrap()
        .expect("default run should be queued");
    assert_eq!(lease.principal_reference(), &owner.reference());
    assert_eq!(lease.principal_reference().audience, None);
}
