#![cfg(feature = "sqlite")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, Clock, CurrentPrincipalResolver, FixedClock,
    PrincipalReference, ResolvedPrincipal, SessionContext,
};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::prelude::{Database, Entity, RepositoryEntity, SqliteBackend};
use graphql_orm_ai::*;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "graphql_orm_ai_sessions",
    plural = "PreTitleLifecycleSessions",
    default_sort = "last_activity_at DESC",
    keyset = "last_activity_at desc, id desc"
)]
struct PreTitleLifecycleSession {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: Uuid,
    owner_principal_kind: String,
    owner_subject: String,
    tenant_id: Option<String>,
    scope_kind: String,
    scope_id: String,
    title: String,
    state: String,
    stream_head: i64,
    message_head: i64,
    last_activity_at: i64,
    archived_at: Option<i64>,
    deleted_at: Option<i64>,
    #[graphql_orm(version, default = "0")]
    row_version: i64,
}

#[derive(RepositoryEntity, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[repository_entity(
    backend = "sqlite",
    table = "graphql_orm_ai_sessions",
    plural = "CurrentTitleSessions",
    default_sort = "last_activity_at DESC",
    keyset = "last_activity_at desc, id desc"
)]
struct CurrentTitleLifecycleSession {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    id: Uuid,
    owner_principal_kind: String,
    owner_subject: String,
    tenant_id: Option<String>,
    scope_kind: String,
    scope_id: String,
    title: String,
    #[graphql_orm(default = "0")]
    title_revision: i64,
    #[graphql_orm(default = "'user'")]
    title_source: String,
    state: String,
    stream_head: i64,
    message_head: i64,
    last_activity_at: i64,
    archived_at: Option<i64>,
    deleted_at: Option<i64>,
    #[graphql_orm(version, default = "0")]
    row_version: i64,
}

struct ToggleAccess {
    allowed: Arc<AtomicBool>,
}

#[async_trait]
impl AiAccessPolicy for ToggleAccess {
    async fn can_access_scope(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        if self.allowed.load(Ordering::SeqCst) {
            AiAccessDecision::allow("title-test", "title-test-v1")
        } else {
            AiAccessDecision::deny("permission_removed", "title-test-v1")
        }
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        if self.allowed.load(Ordering::SeqCst) {
            AiAccessDecision::allow("title-test", "title-test-v1")
        } else {
            AiAccessDecision::deny("permission_removed", "title-test-v1")
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
            tenant_id: Some("tenant-title".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

struct Fixture {
    sessions: OrmAiSessionService,
    title_work: OrmAiSessionTitleWorkService,
    inbox: OrmAiInboxService,
    owner: AuthPrincipal,
    clock: Arc<FixedClock>,
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
            "ai-session-title-test-v1",
            "AI session title lifecycle test",
            module.entities(),
        )
        .await
        .expect("AI schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema migration should apply");

    let owner = principal("title-owner");
    let clock = Arc::new(FixedClock::new(
        OffsetDateTime::now_utc() + Duration::seconds(2),
    ));
    let access_allowed = Arc::new(AtomicBool::new(true));
    let principal_active = Arc::new(AtomicBool::new(true));
    let access_policy = Arc::new(ToggleAccess {
        allowed: access_allowed.clone(),
    });
    let protection_policy = Arc::new(ProtectionPolicy);
    let content_protector = Arc::new(DatabaseManagedContentProtector);
    let principal_resolver: Arc<dyn CurrentPrincipalResolver> = Arc::new(TogglePrincipalResolver {
        principal: owner.clone(),
        active: principal_active.clone(),
        clock: clock.clone(),
    });
    let sessions = OrmAiSessionService::new(
        database.clone(),
        access_policy.clone(),
        protection_policy.clone(),
        content_protector.clone(),
    );
    let title_work = OrmAiSessionTitleWorkService::new(
        database.clone(),
        access_policy,
        protection_policy,
        content_protector,
        principal_resolver.clone(),
        clock.clone(),
        AiSessionTitleWorkLimits::default(),
    );
    let inbox = OrmAiInboxService::new(
        database,
        principal_resolver,
        Arc::new(ToggleAccess {
            allowed: access_allowed.clone(),
        }),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    );
    Fixture {
        sessions,
        title_work,
        inbox,
        owner,
        clock,
        access_allowed,
        principal_active,
    }
}

fn scope_input() -> AiScopeInput {
    AiScopeInput {
        kind: "workspace".to_owned(),
        id: "workspace-title".to_owned(),
        tenant_id: Some("tenant-title".to_owned()),
    }
}

async fn default_session_with_message(fixture: &Fixture, text: &str) -> AiSessionView {
    let session = fixture
        .sessions
        .create_session(
            &fixture.owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: None,
            },
        )
        .await
        .expect("default-title session should create");
    fixture
        .sessions
        .send_message(
            &fixture.owner,
            SendAiMessageInput {
                session_id: session.id,
                text: text.to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("first user message should durably enqueue title work");
    session
}

#[tokio::test]
async fn schema_upgrade_conservatively_fences_preexisting_titles_and_converges() {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
    let previous_plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-session-title-previous-v1",
            "previous AI session title fixture",
            &[PreTitleLifecycleSession::metadata()],
        )
        .await
        .expect("previous session schema should plan");
    database
        .schema()
        .apply_migration(&previous_plan, ApplyOptions::default())
        .await
        .expect("previous session schema should apply");
    let session_id = Uuid::new_v4();
    PreTitleLifecycleSession::insert(
        &database,
        CreatePreTitleLifecycleSessionInput {
            id: session_id,
            owner_principal_kind: "user".to_owned(),
            owner_subject: "previous-owner".to_owned(),
            tenant_id: Some("tenant-title".to_owned()),
            scope_kind: "workspace".to_owned(),
            scope_id: "previous-workspace".to_owned(),
            title: "Existing operator title".to_owned(),
            state: "active".to_owned(),
            stream_head: 0,
            message_head: 0,
            last_activity_at: 1_700_000_000,
            archived_at: None,
            deleted_at: None,
        },
    )
    .await
    .expect("previous session should insert");

    let module = AiSchemaModule;
    let upgrade = database
        .schema()
        .plan_migration_to_entities(
            "ai-session-title-upgrade-v2",
            "AI session title schema 0.52.0",
            module.entities(),
        )
        .await
        .expect("current AI schema should plan over the previous row");
    database
        .schema()
        .apply_migration(&upgrade, ApplyOptions::default())
        .await
        .expect("current AI schema should preserve the previous row");
    let upgraded = CurrentTitleLifecycleSession::find_by_id(&database, &session_id)
        .await
        .expect("upgraded session should load")
        .expect("previous session should remain present");
    assert_eq!(upgraded.title, "Existing operator title");
    assert_eq!(upgraded.title_revision, 0);
    assert_eq!(upgraded.title_source, "user");

    let stable = database
        .schema()
        .plan_migration_to_entities(
            "ai-session-title-upgrade-v3",
            "stable AI session title schema",
            module.entities(),
        )
        .await
        .expect("stable current schema should replan");
    assert!(stable.statements.is_empty());
}

#[tokio::test]
async fn first_message_work_opens_under_current_authority_and_completes_once() {
    let fixture = fixture().await;
    let session = default_session_with_message(&fixture, "Count the active customer records").await;
    let claim = fixture
        .title_work
        .claim_next("title-worker-1")
        .await
        .expect("claim should succeed")
        .expect("first message should create one title job");
    assert_eq!(claim.session_id(), AiSessionId(session.id));
    assert_eq!(claim.expected_title_revision(), 0);
    let input = fixture
        .title_work
        .open_first_message(&claim)
        .await
        .expect("current owner authority should open first text");
    assert_eq!(input.text(), "Count the active customer records");
    for invalid_title in ["   ".to_owned(), "bad\ntitle".to_owned(), "x".repeat(257)] {
        assert!(matches!(
            fixture.title_work.complete(&claim, invalid_title).await,
            Err(AiError::InvalidInput(_))
        ));
    }

    let outcome = fixture
        .title_work
        .complete(&claim, "Active customer count".to_owned())
        .await
        .expect("generated title should commit");
    let AiSessionTitleCommitOutcome::Applied(updated) = outcome else {
        panic!("default title should remain eligible");
    };
    assert_eq!(updated.title, "Active customer count");
    assert_eq!(updated.title_revision, 1);
    assert_eq!(updated.stream_head, 2);

    let replay = fixture
        .title_work
        .complete(&claim, "Active customer count".to_owned())
        .await
        .expect("same completion should be idempotent");
    assert!(matches!(replay, AiSessionTitleCommitOutcome::Applied(_)));
    assert!(
        fixture
            .title_work
            .claim_next("title-worker-2")
            .await
            .expect("empty claim should succeed")
            .is_none()
    );

    fixture
        .sessions
        .send_message(
            &fixture.owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Follow-up question".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("later messages should still send");
    assert!(
        fixture
            .title_work
            .claim_next("title-worker-2")
            .await
            .expect("later messages should not create another title job")
            .is_none()
    );

    let events = fixture
        .sessions
        .session_event_page(&fixture.owner, AiSessionId(session.id), 0, 10)
        .await
        .expect("title event should replay");
    assert_eq!(
        events
            .events
            .iter()
            .filter(|event| event.event_type == "session_title_changed")
            .count(),
        1
    );
    let inbox = fixture
        .inbox
        .inbox_event_page(&fixture.owner, 0, 20)
        .await
        .expect("owner inbox should replay");
    let title_events = inbox
        .events
        .iter()
        .filter(|event| event.event_type == "session_title_changed")
        .collect::<Vec<_>>();
    assert_eq!(title_events.len(), 1);
    assert_eq!(title_events[0].payload.0["title"], "Active customer count");
}

#[tokio::test]
async fn manual_rename_wins_over_an_in_flight_generated_title() {
    let fixture = fixture().await;
    let session = default_session_with_message(&fixture, "Explain this incident").await;
    let claim = fixture
        .title_work
        .claim_next("title-worker-race")
        .await
        .expect("claim should succeed")
        .expect("title work should exist");
    let manually_renamed = fixture
        .sessions
        .rename_session(
            &fixture.owner,
            RenameAiSessionInput {
                session_id: session.id,
                title: "Operator incident notes".to_owned(),
                client_mutation_id: Uuid::new_v4(),
                expected_title_revision: Some(0),
            },
        )
        .await
        .expect("manual rename should win the race");
    assert_eq!(manually_renamed.title_revision, 1);

    let outcome = fixture
        .title_work
        .complete(&claim, "Generated incident title".to_owned())
        .await
        .expect("superseded completion should be a safe terminal outcome");
    assert!(matches!(outcome, AiSessionTitleCommitOutcome::Superseded));
    let authoritative = fixture
        .sessions
        .session(&fixture.owner, AiSessionId(session.id))
        .await
        .expect("session lookup should succeed")
        .expect("session should remain visible");
    assert_eq!(authoritative.title, "Operator incident notes");
    assert_eq!(authoritative.title_revision, 1);
}

#[tokio::test]
async fn leases_fence_stale_workers_and_current_permission_is_rechecked() {
    let fixture = fixture().await;
    default_session_with_message(&fixture, "Describe the deployment state").await;
    let stale_claim = fixture
        .title_work
        .claim_next("title-worker-stale")
        .await
        .expect("first claim should succeed")
        .expect("title work should exist");
    fixture.clock.advance_seconds(301);
    let recovered = fixture
        .title_work
        .claim_next("title-worker-recovered")
        .await
        .expect("expired work should be recoverable")
        .expect("expired lease should be reclaimed");
    assert!(recovered.lease_generation() > stale_claim.lease_generation());
    assert!(matches!(
        fixture.title_work.open_first_message(&stale_claim).await,
        Err(AiError::Conflict)
    ));

    fixture.access_allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture.title_work.open_first_message(&recovered).await,
        Err(AiError::Forbidden)
    ));
    fixture.access_allowed.store(true, Ordering::SeqCst);
    fixture.principal_active.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture.title_work.open_first_message(&recovered).await,
        Err(AiError::ReauthorizationFailed)
    ));
}

#[tokio::test]
async fn retry_heartbeat_and_terminal_failure_are_durable_and_fenced() {
    let fixture = fixture().await;
    default_session_with_message(&fixture, "Summarize the queued maintenance").await;
    let claim = fixture
        .title_work
        .claim_next("title-worker-retry")
        .await
        .expect("claim should succeed")
        .expect("title work should exist");
    let renewed = fixture
        .title_work
        .heartbeat(&claim)
        .await
        .expect("heartbeat should rotate the row-version fence");
    assert!(matches!(
        fixture
            .title_work
            .schedule_retry(&claim, Duration::seconds(10), "provider_busy".to_owned())
            .await,
        Err(AiError::Conflict)
    ));
    fixture
        .title_work
        .schedule_retry(&renewed, Duration::seconds(10), "provider_busy".to_owned())
        .await
        .expect("current claim should schedule a durable retry");
    let ready_session =
        default_session_with_message(&fixture, "A newer title job is ready immediately").await;
    let ready_claim = fixture
        .title_work
        .claim_next("title-worker-ready")
        .await
        .expect("ready claim should succeed")
        .expect("future retry must not starve a ready title job");
    assert_eq!(ready_claim.session_id(), AiSessionId(ready_session.id));
    fixture
        .title_work
        .fail(&ready_claim, "test_complete".to_owned())
        .await
        .expect("ready test job should terminate");
    assert!(
        fixture
            .title_work
            .claim_next("title-worker-too-early")
            .await
            .expect("early claim should succeed without work")
            .is_none()
    );
    fixture.clock.advance_seconds(10);
    let retried = fixture
        .title_work
        .claim_next("title-worker-final")
        .await
        .expect("eligible retry should claim")
        .expect("retry should become eligible");
    assert_eq!(retried.retry_count(), 1);
    fixture
        .title_work
        .fail(&retried, "provider_rejected".to_owned())
        .await
        .expect("redacted terminal failure should persist");
    assert!(
        fixture
            .title_work
            .claim_next("title-worker-after-failure")
            .await
            .expect("terminal work should no longer claim")
            .is_none()
    );
}

#[tokio::test]
async fn archive_preserves_title_commit_and_delete_never_resurrects_a_session() {
    let fixture = fixture().await;
    let archived_session =
        default_session_with_message(&fixture, "Name this archived conversation").await;
    let archived_claim = fixture
        .title_work
        .claim_next("title-worker-archived")
        .await
        .expect("claim should succeed")
        .expect("archived title work should exist");
    fixture
        .sessions
        .archive_session(&fixture.owner, AiSessionId(archived_session.id))
        .await
        .expect("session should archive");
    let outcome = fixture
        .title_work
        .complete(&archived_claim, "Archived conversation".to_owned())
        .await
        .expect("archived visible shell may still receive its title");
    let AiSessionTitleCommitOutcome::Applied(archived) = outcome else {
        panic!("unchanged default title should remain eligible while archived");
    };
    assert_eq!(archived.state, "archived");
    assert_eq!(archived.title, "Archived conversation");

    let deleting_session =
        default_session_with_message(&fixture, "This session will be deleted").await;
    let deleting_claim = fixture
        .title_work
        .claim_next("title-worker-deleting")
        .await
        .expect("claim should succeed")
        .expect("deleting title work should exist");
    fixture
        .sessions
        .delete_session(&fixture.owner, AiSessionId(deleting_session.id))
        .await
        .expect("delete should begin");
    assert!(matches!(
        fixture
            .title_work
            .complete(&deleting_claim, "Must not return".to_owned())
            .await,
        Err(AiError::NotFound)
    ));
    assert!(
        fixture
            .sessions
            .session(&fixture.owner, AiSessionId(deleting_session.id))
            .await
            .expect("hidden lookup should succeed")
            .is_none()
    );
}
