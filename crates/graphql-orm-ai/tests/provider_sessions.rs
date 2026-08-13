#![cfg(feature = "sqlite")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, Clock, CurrentPrincipalResolver, FixedClock,
    PrincipalReference, ResolvedPrincipal, SessionContext,
};
use async_graphql::Schema;
use async_trait::async_trait;
use graphql_orm::graphql::orm::{
    ApplyOptions, ColumnBackupPolicy, MigrationStep, OrmSchemaModule, SchemaModuleCatalog,
};
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
            AiAccessDecision::allow("provider-session-test", "v1")
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
            AiAccessDecision::allow("provider-session-test", "v1")
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
        Ok(protection_policy(scope.clone()))
    }
}

fn protection_policy(scope: AiScope) -> AiContentProtectionPolicy {
    AiContentProtectionPolicy {
        scope,
        mode: AiContentProtectionMode::DatabaseManaged,
        key_policy_reference: None,
        version: 1,
        ready: true,
    }
}

struct PrincipalResolver {
    principals: BTreeMap<String, AuthPrincipal>,
    active: Arc<AtomicBool>,
    clock: Arc<FixedClock>,
}

#[async_trait]
impl CurrentPrincipalResolver for PrincipalResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::Forbidden);
        }
        let principal = self
            .principals
            .get(&reference.subject)
            .cloned()
            .ok_or(agql_auth::AuthError::Forbidden)?;
        ResolvedPrincipal::new(reference.clone(), principal, self.clock.now())
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
            tenant_id: Some("tenant-provider-session".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

struct ProviderSessionFixture {
    sessions: OrmAiSessionService,
    runs: OrmAiRunService,
    provider_sessions: OrmAiProviderSessionService,
    owner: AuthPrincipal,
    other: AuthPrincipal,
    access_allowed: Arc<AtomicBool>,
    principal_active: Arc<AtomicBool>,
    clock: Arc<FixedClock>,
}

async fn provider_session_fixture() -> ProviderSessionFixture {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "provider-session-lifecycle-v1",
            "provider session lifecycle",
            module.entities(),
        )
        .await
        .expect("AI schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema migration should apply");

    let owner = principal("provider-session-owner");
    let other = principal("provider-session-other");
    let clock = Arc::new(FixedClock::new(
        OffsetDateTime::now_utc() + Duration::seconds(2),
    ));
    let access_allowed = Arc::new(AtomicBool::new(true));
    let principal_active = Arc::new(AtomicBool::new(true));
    let access_policy: Arc<dyn AiAccessPolicy> = Arc::new(ToggleAccess(access_allowed.clone()));
    let protection_resolver: Arc<dyn AiContentProtectionPolicyResolver> =
        Arc::new(ProtectionPolicy);
    let content_protector: Arc<dyn AiContentProtector> = Arc::new(DatabaseManagedContentProtector);
    let principal_resolver: Arc<dyn CurrentPrincipalResolver> = Arc::new(PrincipalResolver {
        principals: BTreeMap::from([
            ("provider-session-owner".to_owned(), owner.clone()),
            ("provider-session-other".to_owned(), other.clone()),
        ]),
        active: principal_active.clone(),
        clock: clock.clone(),
    });
    let sessions = OrmAiSessionService::new(
        database.clone(),
        access_policy.clone(),
        protection_resolver.clone(),
        content_protector.clone(),
    );
    let runs = OrmAiRunService::new(
        database.clone(),
        clock.clone(),
        AiRunServiceLimits::new(Duration::minutes(1), Duration::minutes(1), 16, 3, 3)
            .expect("run limits should validate"),
    );
    let provider_sessions = OrmAiProviderSessionService::new(
        database,
        access_policy,
        protection_resolver,
        content_protector,
        principal_resolver,
        clock.clone(),
        AiProviderSessionLimits::default(),
        Duration::minutes(5),
    )
    .expect("provider-session service should validate");
    ProviderSessionFixture {
        sessions,
        runs,
        provider_sessions,
        owner,
        other,
        access_allowed,
        principal_active,
        clock,
    }
}

async fn active_run(
    fixture: &ProviderSessionFixture,
    principal: &AuthPrincipal,
    scope_id: &str,
) -> AiRunLease {
    let session = fixture
        .sessions
        .create_session(
            principal,
            CreateAiSessionInput {
                scope: AiScopeInput {
                    kind: "workspace".to_owned(),
                    id: scope_id.to_owned(),
                    tenant_id: Some("tenant-provider-session".to_owned()),
                },
                title: None,
            },
        )
        .await
        .expect("session should create");
    fixture
        .sessions
        .send_message(
            principal,
            SendAiMessageInput {
                session_id: session.id,
                text: "Start a retained provider turn".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message should enqueue a run");
    let claimed = fixture
        .runs
        .claim_next("provider-session-test-worker")
        .await
        .expect("claim should succeed")
        .expect("queued run should exist");
    fixture
        .runs
        .start(&claimed)
        .await
        .expect("run should start")
}

async fn next_active_run(
    fixture: &ProviderSessionFixture,
    principal: &AuthPrincipal,
    session_id: AiSessionId,
) -> AiRunLease {
    fixture
        .sessions
        .send_message(
            principal,
            SendAiMessageInput {
                session_id: session_id.0,
                text: "Continue from authoritative durable history".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("later message should enqueue a run");
    let claimed = fixture
        .runs
        .claim_next("provider-session-test-worker")
        .await
        .expect("later claim should succeed")
        .expect("later queued run should exist");
    fixture
        .runs
        .start(&claimed)
        .await
        .expect("later run should start")
}

#[test]
fn provider_session_cursor_is_private_redacted_schema_state() {
    let module = AiSchemaModule;
    let catalog = SchemaModuleCatalog::compose(&[&module]).expect("AI module should validate");
    let metadata = catalog
        .entities()
        .iter()
        .find(|entity| entity.table_name == "graphql_orm_ai_provider_session_bindings")
        .expect("provider-session binding metadata should exist");
    let table = catalog
        .schema_model()
        .tables
        .into_iter()
        .find(|table| table.table_name == "graphql_orm_ai_provider_session_bindings")
        .expect("provider-session binding table should exist");

    assert!(
        table
            .indexes
            .iter()
            .any(|index| { index.is_unique && index.columns == ["session_id"] })
    );
    assert!(
        table
            .columns
            .iter()
            .any(|column| { column.name == "protected_cursor" && column.nullable })
    );
    assert_eq!(
        metadata
            .fields
            .iter()
            .find(|field| field.name == "protected_cursor")
            .expect("protected cursor metadata should exist")
            .backup_policy,
        ColumnBackupPolicy::Redact
    );

    let sdl = Schema::build(AiQueryRoot, AiMutationRoot, AiSubscriptionRoot)
        .finish()
        .sdl();
    for forbidden in [
        "AiProviderSessionCursor",
        "AiProviderSessionBinding",
        "providerSessionCursor",
        "providerSessionBindings",
        "graphqlOrmAiProviderSessionBindings",
    ] {
        assert!(
            !sdl.contains(forbidden),
            "private provider cursor surface leaked through {forbidden}"
        );
    }
}

#[tokio::test]
async fn provider_session_schema_is_additive_and_idempotent_on_sqlite() {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let module = AiSchemaModule;
    let initial = database
        .schema()
        .plan_migration_to_entities(
            "provider-session-schema-v1",
            "provider session schema",
            module.entities(),
        )
        .await
        .expect("provider-session schema should plan");
    assert!(
        initial.steps.iter().any(|step| {
            matches!(
                &step.step,
                MigrationStep::CreateTable(table)
                    if table.table_name == "graphql_orm_ai_provider_session_bindings"
            )
        }),
        "provider-session table must be part of the additive migration"
    );
    database
        .schema()
        .apply_migration(&initial, ApplyOptions::default())
        .await
        .expect("provider-session schema should apply");

    let repeated = database
        .schema()
        .plan_migration_to_entities(
            "provider-session-schema-v2",
            "provider session schema idempotency",
            module.entities(),
        )
        .await
        .expect("identical schema should replan");
    assert!(repeated.steps.is_empty() && repeated.statements.is_empty());
}

#[tokio::test]
async fn provider_session_cursor_is_owner_run_fenced_revocable_and_exactly_cleaned() {
    let fixture = provider_session_fixture().await;
    let owner = fixture.owner.clone();
    let owner_run = active_run(&fixture, &owner, "owner-workspace").await;
    let descriptor = AiProviderSessionDescriptor::new(
        ProviderKind::LocalHarness,
        "reviewed-local-profile",
        "reviewed-model",
        "a".repeat(64),
        "codex-app-server/v2",
        "b".repeat(64),
    )
    .expect("provider descriptor should validate");
    let claim = fixture
        .provider_sessions
        .bind_for_run(
            &owner_run,
            AiProviderSessionBindRequest::new(
                descriptor,
                AiProviderSessionCursor::new("codex.thread", "thread-owner-1")
                    .expect("cursor should validate"),
                "c".repeat(64),
                Some(fixture.clock.now() + Duration::hours(1)),
            )
            .expect("bind request should validate"),
        )
        .await
        .expect("empty provider session should bind to the exact run");
    assert_eq!(claim.session_id(), owner_run.session_id());
    assert_eq!(claim.run_id(), owner_run.run_id());
    assert_eq!(claim.through_message_sequence(), 0);

    let opened = fixture
        .provider_sessions
        .open_for_run(&owner_run, &claim)
        .await
        .expect("current owner should open the exact protected cursor");
    assert_eq!(opened.cursor().kind(), "codex.thread");
    assert_eq!(
        opened.cursor().expose_to_provider_adapter(),
        "thread-owner-1"
    );

    let other = fixture.other.clone();
    let other_run = active_run(&fixture, &other, "other-workspace").await;
    assert!(matches!(
        fixture
            .provider_sessions
            .open_for_run(&other_run, &claim)
            .await,
        Err(AiError::Conflict | AiError::NotFound | AiError::ReauthorizationFailed)
    ));

    fixture.access_allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .provider_sessions
            .open_for_run(&owner_run, &claim)
            .await,
        Err(AiError::Forbidden)
    ));
    fixture.access_allowed.store(true, Ordering::SeqCst);
    fixture.principal_active.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .provider_sessions
            .open_for_run(&owner_run, &claim)
            .await,
        Err(AiError::ReauthorizationFailed)
    ));
    fixture.principal_active.store(true, Ordering::SeqCst);

    let renewed = fixture
        .provider_sessions
        .heartbeat(&owner_run, &claim)
        .await
        .expect("current exact claim should renew");
    assert!(matches!(
        fixture
            .provider_sessions
            .open_for_run(&owner_run, &claim)
            .await,
        Err(AiError::Conflict)
    ));
    fixture
        .provider_sessions
        .require_cleanup(&renewed, "provider_session_cancelled")
        .await
        .expect("exact claim should become permanently non-resumable");
    let cleanup = fixture
        .provider_sessions
        .claim_cleanup("provider-session-cleanup-worker")
        .await
        .expect("cleanup claim should succeed")
        .expect("invalidated binding should need cleanup");
    let deletion = fixture
        .provider_sessions
        .open_for_cleanup(&cleanup, &protection_policy(cleanup.scope().clone()))
        .await
        .expect("exact cleanup claim should open only its protected cursor");
    assert_eq!(
        deletion.cursor().expose_to_provider_adapter(),
        "thread-owner-1"
    );
    let absence = AiProviderSessionAbsenceProof::for_request(&deletion, fixture.clock.now());
    fixture
        .provider_sessions
        .complete_cleanup(&cleanup, absence)
        .await
        .expect("exact provider-absence proof should tombstone the binding");
    assert!(
        fixture
            .provider_sessions
            .claim_cleanup("provider-session-cleanup-worker")
            .await
            .expect("empty cleanup scan should succeed")
            .is_none()
    );
    assert!(matches!(
        fixture
            .provider_sessions
            .open_for_run(&owner_run, &renewed)
            .await,
        Err(AiError::NotFound | AiError::Conflict)
    ));
}

#[tokio::test]
async fn exact_absence_authorizes_one_fenced_rebind_with_a_fresh_cursor() {
    let fixture = provider_session_fixture().await;
    let owner = fixture.owner.clone();
    let first_run = active_run(&fixture, &owner, "rebind-workspace").await;
    let descriptor = AiProviderSessionDescriptor::new(
        ProviderKind::LocalHarness,
        "reviewed-local-profile",
        "reviewed-model",
        "a".repeat(64),
        "codex-app-server/v2",
        "b".repeat(64),
    )
    .expect("provider descriptor should validate");
    let first_claim = fixture
        .provider_sessions
        .bind_for_run(
            &first_run,
            AiProviderSessionBindRequest::new(
                descriptor.clone(),
                AiProviderSessionCursor::new("codex.thread", "deleted-thread")
                    .expect("old cursor should validate"),
                "c".repeat(64),
                None,
            )
            .expect("old bind request should validate"),
        )
        .await
        .expect("old provider session should bind");
    fixture
        .provider_sessions
        .require_cleanup(&first_claim, "provider_turn_uncertain")
        .await
        .expect("uncertain provider turn should require cleanup");

    let unavailable_plan = AiProviderSessionTurnPlan::new(descriptor.clone(), "d".repeat(64))
        .expect("later plan should validate");
    assert!(matches!(
        fixture
            .provider_sessions
            .disposition_for_run(&first_run, &unavailable_plan)
            .await
            .expect("cleanup disposition should resolve"),
        AiProviderSessionRunDisposition::Unavailable(AiProviderSessionState::CleanupRequired)
    ));

    let cleanup = fixture
        .provider_sessions
        .claim_cleanup("provider-session-cleanup-worker")
        .await
        .expect("cleanup claim should succeed")
        .expect("uncertain binding should need deletion");
    assert!(matches!(
        fixture
            .provider_sessions
            .disposition_for_run(&first_run, &unavailable_plan)
            .await
            .expect("in-progress disposition should resolve"),
        AiProviderSessionRunDisposition::Unavailable(AiProviderSessionState::CleanupInProgress)
    ));
    let deletion = fixture
        .provider_sessions
        .open_for_cleanup(&cleanup, &protection_policy(cleanup.scope().clone()))
        .await
        .expect("cleanup cursor should open");
    assert_eq!(
        deletion.cursor().expose_to_provider_adapter(),
        "deleted-thread",
    );
    fixture
        .provider_sessions
        .schedule_cleanup_retry(
            &cleanup,
            Duration::seconds(1),
            "provider_delete_unavailable",
        )
        .await
        .expect("failed provider deletion should retain the cleanup fence");
    assert!(matches!(
        fixture
            .provider_sessions
            .disposition_for_run(&first_run, &unavailable_plan)
            .await
            .expect("backoff disposition should resolve"),
        AiProviderSessionRunDisposition::Unavailable(AiProviderSessionState::CleanupBackoff)
    ));
    fixture.clock.advance_seconds(2);
    let cleanup = fixture
        .provider_sessions
        .claim_cleanup("provider-session-cleanup-worker")
        .await
        .expect("cleanup retry claim should succeed")
        .expect("backoff expiry should permit cleanup, not rebind");
    let deletion = fixture
        .provider_sessions
        .open_for_cleanup(&cleanup, &protection_policy(cleanup.scope().clone()))
        .await
        .expect("retried cleanup cursor should open");
    assert_eq!(
        deletion.cursor().expose_to_provider_adapter(),
        "deleted-thread",
        "retry must delete the original cursor rather than authorize replacement",
    );
    let absence = AiProviderSessionAbsenceProof::for_request(&deletion, fixture.clock.now());
    fixture
        .provider_sessions
        .complete_cleanup(&cleanup, absence)
        .await
        .expect("exact provider absence should persist");
    assert!(matches!(
        fixture
            .provider_sessions
            .disposition_for_run(&first_run, &unavailable_plan)
            .await
            .expect("same-run deleted disposition should resolve"),
        AiProviderSessionRunDisposition::Unavailable(AiProviderSessionState::Deleted)
    ));

    fixture
        .runs
        .finish(
            &first_run,
            AiRunCompletion::new(
                AiRunState::RecoveryRequired,
                "provider_turn_uncertain",
                None,
                None,
            )
            .expect("recovery completion should validate"),
        )
        .await
        .expect("uncertain run should finish recovery-required");
    let second_run = next_active_run(&fixture, &owner, first_run.session_id()).await;
    let second_plan = AiProviderSessionTurnPlan::new(descriptor.clone(), "d".repeat(64))
        .expect("rebind plan should validate");
    let changed_descriptor = AiProviderSessionDescriptor::new(
        ProviderKind::LocalHarness,
        "reviewed-local-profile",
        "changed-model",
        "a".repeat(64),
        "codex-app-server/v2",
        "b".repeat(64),
    )
    .expect("changed descriptor should validate");
    let changed_plan = AiProviderSessionTurnPlan::new(changed_descriptor, "d".repeat(64))
        .expect("changed plan should validate");
    assert!(matches!(
        fixture
            .provider_sessions
            .disposition_for_run(&second_run, &changed_plan)
            .await
            .expect("changed descriptor disposition should resolve"),
        AiProviderSessionRunDisposition::Unavailable(AiProviderSessionState::Deleted)
    ));
    let authorization = match fixture
        .provider_sessions
        .disposition_for_run(&second_run, &second_plan)
        .await
        .expect("deleted binding should be classified")
    {
        AiProviderSessionRunDisposition::RebindAllowed(authorization) => *authorization,
        other => panic!("expected rebind authority, got {other:?}"),
    };
    let other = fixture.other.clone();
    let other_run = active_run(&fixture, &other, "rebind-other-workspace").await;
    assert!(matches!(
        fixture
            .provider_sessions
            .rebind_for_run(
                &other_run,
                authorization.clone(),
                AiProviderSessionBindRequest::new(
                    descriptor.clone(),
                    AiProviderSessionCursor::new("codex.thread", "swapped-owner-thread")
                        .expect("swapped cursor should validate"),
                    "d".repeat(64),
                    None,
                )
                .expect("swapped request should validate"),
            )
            .await,
        Err(AiError::Conflict | AiError::NotFound | AiError::ReauthorizationFailed)
    ));
    assert!(matches!(
        fixture
            .provider_sessions
            .rebind_for_run(
                &second_run,
                authorization.clone(),
                AiProviderSessionBindRequest::new(
                    descriptor.clone(),
                    AiProviderSessionCursor::new("codex.thread", "swapped-transcript-thread")
                        .expect("swapped cursor should validate"),
                    "e".repeat(64),
                    None,
                )
                .expect("swapped transcript request should validate"),
            )
            .await,
        Err(AiError::Conflict)
    ));
    let stale_authorization = authorization.clone();
    let request = || {
        AiProviderSessionBindRequest::new(
            descriptor.clone(),
            AiProviderSessionCursor::new("codex.thread", "fresh-thread")
                .expect("fresh cursor should validate"),
            "d".repeat(64),
            None,
        )
        .expect("fresh bind request should validate")
    };
    let (first, second) = tokio::join!(
        fixture
            .provider_sessions
            .rebind_for_run(&second_run, authorization, request()),
        fixture
            .provider_sessions
            .rebind_for_run(&second_run, stale_authorization, request())
    );
    let rebound = match (first, second) {
        (Ok(claim), Err(AiError::Conflict)) | (Err(AiError::Conflict), Ok(claim)) => claim,
        outcomes => panic!("exactly one rebind should win: {outcomes:?}"),
    };
    let opened = fixture
        .provider_sessions
        .open_for_run(&second_run, &rebound)
        .await
        .expect("rebound cursor should open under its exact fence");
    assert_eq!(opened.cursor().expose_to_provider_adapter(), "fresh-thread");
    assert_ne!(
        opened.cursor().expose_to_provider_adapter(),
        "deleted-thread"
    );
}
