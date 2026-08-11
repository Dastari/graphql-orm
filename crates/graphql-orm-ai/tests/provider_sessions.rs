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
        .expect("exact provider-absence proof should remove the binding");
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
