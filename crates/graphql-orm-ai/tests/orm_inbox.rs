#![cfg(feature = "sqlite")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agql_auth::{
    AccessTokenMetadata, AssuranceMatchMode, AuthPrincipal, AuthUser, CurrentPrincipalResolver,
    FixedClock, MfaAcceptance, PrincipalReference, RecentMfaPolicy, ResolvedPrincipal,
    SessionAssurance, SessionContext,
};
use async_trait::async_trait;
use futures::StreamExt;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::prelude::{Database, PaginationConfig, SqliteBackend};
use graphql_orm_ai::*;
use secrecy::SecretString;
use time::Duration as TimeDuration;
use time::OffsetDateTime;
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
        AiAccessDecision::allow("test", "test-v1")
    }

    async fn can_access_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _action: AiSessionAction,
    ) -> AiAccessDecision {
        AiAccessDecision::allow("test", "test-v1")
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

struct AllowConfiguration;

#[async_trait]
impl AiConfigurationAccessPolicy for AllowConfiguration {
    async fn can_configure(
        &self,
        _principal: &AuthPrincipal,
        _scope: &AiScope,
        _action: AiConfigurationAction,
    ) -> bool {
        true
    }
}

struct DenyEndpoints;

impl AiProviderEndpointPolicy for DenyEndpoints {
    fn authorize_endpoint(
        &self,
        _provider_kind: AiProviderKindInput,
        _normalized_url: &str,
    ) -> bool {
        false
    }
}

struct UnusedSecretStore;

#[async_trait]
impl AiSecretStore for UnusedSecretStore {
    async fn resolve(&self, _reference: &SecretRef) -> Result<SecretString, SecretError> {
        Err(SecretError::Unavailable)
    }

    async fn put(
        &self,
        _reference: Option<&SecretRef>,
        _value: SecretString,
    ) -> Result<SecretRef, SecretError> {
        Err(SecretError::ReadOnly)
    }

    async fn delete(&self, _reference: &SecretRef) -> Result<(), SecretError> {
        Err(SecretError::ReadOnly)
    }
}

struct ToggleResolver {
    principal: AuthPrincipal,
    active: Arc<AtomicBool>,
}

#[async_trait]
impl CurrentPrincipalResolver for ToggleResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(agql_auth::AuthError::Forbidden);
        }
        ResolvedPrincipal::new(
            reference.clone(),
            self.principal.clone(),
            OffsetDateTime::now_utc(),
        )
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
            tenant_id: Some("tenant-1".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

fn recent_admin(now: OffsetDateTime) -> AuthPrincipal {
    let assurance = SessionAssurance::new(
        now,
        ["otp", "pwd"],
        Some("urn:test:loa:2".to_owned()),
        Some("test".to_owned()),
        MfaAcceptance::Satisfied,
    )
    .expect("test assurance is valid");
    AuthPrincipal::User(AuthUser {
        user_id: "retention-admin".to_owned(),
        session_id: Uuid::new_v4(),
        roles: vec!["admin".to_owned()],
        scopes: vec![],
        session: SessionContext::default().with_assurance(assurance),
        token_claims: AccessTokenMetadata {
            auth_time: Some(now.unix_timestamp()),
            amr: Some(vec!["otp".to_owned(), "pwd".to_owned()]),
            acr: Some("urn:test:loa:2".to_owned()),
            tenant_id: Some("tenant-1".to_owned()),
            ..AccessTokenMetadata::default()
        },
    })
}

async fn services(
    reauthorization_interval: Duration,
) -> (
    OrmAiSessionService,
    OrmAiInboxService,
    OrmAiInboxPruningService,
    OrmAiSessionRetentionService,
    AuthPrincipal,
    Arc<AtomicBool>,
) {
    services_with_limits(
        reauthorization_interval,
        1,
        PaginationConfig::SECURE_MAX_LIMIT,
    )
    .await
}

async fn services_with_limits(
    reauthorization_interval: Duration,
    replay_page_size: i64,
    maximum_page_limit: i64,
) -> (
    OrmAiSessionService,
    OrmAiInboxService,
    OrmAiInboxPruningService,
    OrmAiSessionRetentionService,
    AuthPrincipal,
    Arc<AtomicBool>,
) {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite opens")
        .with_pagination_config(PaginationConfig::explicit_only(maximum_page_limit));
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-inbox-test-v1",
            "AI principal inbox test",
            module.entities(),
        )
        .await
        .expect("schema plans");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("schema applies");
    let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
        .expect("current second is representable");
    let configuration = OrmAiConfigurationService::new(
        database.clone(),
        Arc::new(AllowConfiguration),
        Arc::new(DenyEndpoints),
        RecentMfaPolicy {
            maximum_age: TimeDuration::minutes(5),
            clock_skew: TimeDuration::seconds(30),
            allowed_amr: vec!["otp".to_owned()],
            allowed_acr: vec!["urn:test:loa:2".to_owned()],
            match_mode: AssuranceMatchMode::All,
        },
        Arc::new(FixedClock::new(now)),
        Arc::new(UnusedSecretStore),
    );
    configuration
        .set_retention_policy(
            &recent_admin(now),
            SetAiRetentionPolicyInput {
                scope: AiScopeInput {
                    kind: "collection".to_owned(),
                    id: "54".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                message_retention_seconds: None,
                delta_retention_seconds: 60,
                raw_payload_retention_seconds: 60,
                audit_retention_seconds: 60,
                deleted_content_purge_seconds: 60,
                provider_file_delete_required: true,
                inbox_event_retention_seconds: 60,
                inbox_minimum_events: 1,
                expected_version: None,
            },
        )
        .await
        .expect("retention policy is GraphQL-service managed");
    let access_policy = Arc::new(AllowAll);
    let protection_policy = Arc::new(ProtectionPolicy);
    let content_protector = Arc::new(DatabaseManagedContentProtector);
    let sessions = OrmAiSessionService::new(
        database.clone(),
        access_policy.clone(),
        protection_policy.clone(),
        content_protector.clone(),
    );
    let owner = principal("owner");
    let active = Arc::new(AtomicBool::new(true));
    let inbox = OrmAiInboxService::new(
        database.clone(),
        Arc::new(ToggleResolver {
            principal: owner.clone(),
            active: active.clone(),
        }),
        access_policy,
        protection_policy,
        content_protector,
    )
    .with_reauthorization_interval(reauthorization_interval)
    .with_replay_page_size(replay_page_size);
    let pruning = OrmAiInboxPruningService::new(
        database.clone(),
        Arc::new(FixedClock::new(now + TimeDuration::seconds(61))),
        AiInboxPruningLimits::new(10, 100).expect("valid pruning limits"),
    );
    let session_retention = OrmAiSessionRetentionService::new(
        database,
        Arc::new(FixedClock::new(now + TimeDuration::seconds(61))),
        AiSessionRetentionLimits::default()
            .with_inbox_event_limit(1)
            .expect("valid session inbox-retention limit"),
    );
    (sessions, inbox, pruning, session_retention, owner, active)
}

async fn append_title_events(
    sessions: &OrmAiSessionService,
    owner: &AuthPrincipal,
    session_id: Uuid,
    first_revision: i64,
    count: i64,
) {
    for revision in first_revision..first_revision + count {
        sessions
            .rename_session(
                owner,
                RenameAiSessionInput {
                    session_id,
                    title: format!("Inbox title {}", revision + 1),
                    client_mutation_id: Uuid::new_v4(),
                    expected_title_revision: Some(revision),
                },
            )
            .await
            .expect("title event should commit");
    }
}

async fn collect_inbox_sequences(
    inbox: &OrmAiInboxService,
    owner: &AuthPrincipal,
    first: i64,
) -> (Vec<i64>, Vec<usize>, i64, String) {
    let mut after = 0;
    let mut sequences = Vec::new();
    let mut page_lengths = Vec::new();
    let mut final_event_type = String::new();
    loop {
        let page = inbox
            .inbox_event_page(owner, after, first)
            .await
            .expect("inbox event page should load");
        assert!(!page.reset_required);
        let watermark = page.watermark;
        page_lengths.push(page.events.len());
        for event in page.events {
            after = event.sequence;
            final_event_type = event.event_type;
            sequences.push(after);
        }
        if !page.has_more {
            return (sequences, page_lengths, watermark, final_event_type);
        }
    }
}

async fn create_session(
    sessions: &OrmAiSessionService,
    principal: &AuthPrincipal,
) -> AiSessionView {
    sessions
        .create_session(
            principal,
            CreateAiSessionInput {
                scope: AiScopeInput {
                    kind: "collection".to_owned(),
                    id: "54".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                title: Some("Inbox".to_owned()),
            },
        )
        .await
        .expect("session is created")
}

#[tokio::test]
async fn owner_pages_atomic_cross_session_events_without_cross_principal_leakage() {
    let (sessions, inbox, _pruning, _retention, owner, _active) =
        services(Duration::from_secs(60)).await;
    let first_session = create_session(&sessions, &owner).await;
    let second_session = create_session(&sessions, &owner).await;
    let sent = sessions
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: first_session.id,
                text: "bounded message".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message commits with its inbox event");

    let first_page = inbox
        .inbox_event_page(&owner, 0, 2)
        .await
        .expect("owner inbox loads");
    assert_eq!(first_page.watermark, 3);
    assert!(first_page.has_more);
    assert_eq!(first_page.events.len(), 2);
    assert_eq!(first_page.events[0].event_type, "session_created");
    assert_eq!(first_page.events[0].session_id, first_session.id);
    assert_eq!(first_page.events[1].session_id, second_session.id);

    let second_page = inbox
        .inbox_event_page(&owner, first_page.events[1].sequence, 2)
        .await
        .expect("owner inbox advances");
    assert_eq!(second_page.events.len(), 1);
    assert_eq!(second_page.events[0].event_type, "message_queued");
    assert_eq!(
        second_page.events[0].payload.0["runId"],
        sent.run_id.to_string()
    );

    let stranger_page = inbox
        .inbox_event_page(&principal("stranger"), 0, 100)
        .await
        .expect("an unrelated empty inbox does not reveal owner activity");
    assert!(stranger_page.events.is_empty());
    assert_eq!(stranger_page.watermark, 0);
}

#[tokio::test]
async fn owner_inbox_pages_use_the_snapshotted_watermark_at_the_orm_limit() {
    let (sessions, inbox, _pruning, _retention, owner, _active) =
        services(Duration::from_secs(60)).await;
    let session = create_session(&sessions, &owner).await;

    append_title_events(&sessions, &owner, session.id, 0, 98).await;
    let page_99 = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("99-event inbox page should load");
    assert_eq!(page_99.events.len(), 99);
    assert_eq!(page_99.watermark, 99);
    assert!(!page_99.has_more);

    append_title_events(&sessions, &owner, session.id, 98, 1).await;
    let page_100 = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("100-event inbox page should load");
    assert_eq!(page_100.events.len(), 100);
    assert_eq!(page_100.watermark, 100);
    assert!(!page_100.has_more);

    append_title_events(&sessions, &owner, session.id, 99, 1).await;
    let first_101 = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("first 101-event inbox page should load");
    assert_eq!(first_101.events.len(), 100);
    assert_eq!(first_101.watermark, 101);
    assert!(first_101.has_more);
    let last_101 = inbox
        .inbox_event_page(&owner, 100, 100)
        .await
        .expect("last 101-event inbox page should load");
    assert_eq!(last_101.events.len(), 1);
    assert_eq!(last_101.events[0].sequence, 101);
    assert!(!last_101.has_more);

    append_title_events(&sessions, &owner, session.id, 100, 243).await;
    sessions
        .archive_session(&owner, AiSessionId(session.id))
        .await
        .expect("terminal archive event should commit");
    let (sequences, page_lengths, watermark, final_event_type) =
        collect_inbox_sequences(&inbox, &owner, 100).await;
    assert_eq!(page_lengths, [100, 100, 100, 45]);
    assert_eq!(sequences, (1..=345).collect::<Vec<_>>());
    assert_eq!(watermark, 345);
    assert_eq!(final_event_type, "session_archived");

    sessions
        .restore_session(&owner, AiSessionId(session.id))
        .await
        .expect("event after the captured watermark should commit");
    let later = inbox
        .inbox_event_page(&owner, watermark, 100)
        .await
        .expect("the subsequent inbox boundary should observe the new event");
    assert_eq!(later.watermark, 346);
    assert_eq!(later.events.len(), 1);
    assert_eq!(later.events[0].sequence, 346);
    assert_eq!(later.events[0].event_type, "session_restored");
    assert!(!later.has_more);

    let stranger = inbox
        .inbox_event_page(&principal("inbox-stranger"), 0, 100)
        .await
        .expect("another principal should see only its own empty stream");
    assert!(stranger.events.is_empty());
    assert_eq!(stranger.watermark, 0);
    assert!(!stranger.has_more);
}

#[tokio::test]
async fn owner_inbox_pages_follow_a_smaller_orm_maximum() {
    let (sessions, inbox, _pruning, _retention, owner, _active) =
        services_with_limits(Duration::from_secs(60), 7, 7).await;
    let session = create_session(&sessions, &owner).await;
    append_title_events(&sessions, &owner, session.id, 0, 19).await;

    let (sequences, page_lengths, watermark, _) = collect_inbox_sequences(&inbox, &owner, 7).await;
    assert_eq!(page_lengths, [7, 7, 6]);
    assert_eq!(sequences, (1..=20).collect::<Vec<_>>());
    assert_eq!(watermark, 20);
}

#[tokio::test]
async fn owner_inbox_subscription_replays_every_maximum_sized_page_before_live_events() {
    let (sessions, inbox, _pruning, _retention, owner, _active) =
        services_with_limits(Duration::from_secs(60), 100, 100).await;
    let session = create_session(&sessions, &owner).await;
    append_title_events(&sessions, &owner, session.id, 0, 100).await;
    let mut stream = inbox
        .inbox_events(owner.clone(), 0)
        .await
        .expect("inbox subscription should open");

    let mut replayed = Vec::new();
    for _ in 0..101 {
        let envelope = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("replay should not stall")
            .expect("replay item should exist")
            .expect("replay item should succeed");
        assert!(!envelope.reset_required);
        replayed.push(envelope.event.expect("durable event").sequence);
    }
    assert_eq!(replayed, (1..=101).collect::<Vec<_>>());

    sessions
        .rename_session(
            &owner,
            RenameAiSessionInput {
                session_id: session.id,
                title: "Live inbox title".to_owned(),
                client_mutation_id: Uuid::new_v4(),
                expected_title_revision: Some(100),
            },
        )
        .await
        .expect("live event should commit");
    let live = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("subscription should wake")
        .expect("live item should exist")
        .expect("live item should succeed");
    let live = live.event.expect("live durable event");
    assert_eq!(live.sequence, 102);
    assert_eq!(live.event_type, "session_title_changed");
}

#[tokio::test]
async fn subscription_replays_to_a_watermark_follows_commits_and_closes_on_revocation() {
    let (sessions, inbox, _pruning, _retention, owner, active) =
        services(Duration::from_millis(20)).await;
    let session = create_session(&sessions, &owner).await;
    let mut stream = inbox
        .inbox_events(owner.clone(), 0)
        .await
        .expect("inbox subscription opens");

    let created = stream
        .next()
        .await
        .expect("created item")
        .expect("created event");
    assert_eq!(
        created.event.expect("durable event").event_type,
        "session_created"
    );

    sessions
        .archive_session(&owner, AiSessionId(session.id))
        .await
        .expect("archive commits with a wakeup");
    let archived = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("subscription wakes")
        .expect("archive item")
        .expect("archive event");
    assert_eq!(
        archived.event.expect("durable event").event_type,
        "session_archived"
    );

    active.store(false, Ordering::SeqCst);
    let revoked = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("periodic reauthorization runs")
        .expect("terminal error item");
    assert!(matches!(revoked, Err(AiError::ReauthorizationFailed)));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn pruning_deletes_only_an_expired_prefix_and_requires_cursor_reset() {
    let (sessions, inbox, pruning, _retention, owner, _active) =
        services(Duration::from_secs(60)).await;
    let session = create_session(&sessions, &owner).await;
    for text in ["first", "second"] {
        sessions
            .send_message(
                &owner,
                SendAiMessageInput {
                    session_id: session.id,
                    text: text.to_owned(),
                    attachment_ids: vec![],
                    client_message_id: Uuid::new_v4(),
                },
            )
            .await
            .expect("message commits");
    }

    let report = pruning
        .prune_inbox_events()
        .await
        .expect("bounded pruning succeeds");
    assert_eq!(report.streams_pruned, 1);
    assert_eq!(report.events_deleted, 2);
    assert_eq!(report.streams_not_ready, 0);

    let stale = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("stale cursor receives an explicit reset");
    assert!(stale.reset_required);
    assert_eq!(stale.watermark, 3);
    assert!(stale.events.is_empty());

    let retained = inbox
        .inbox_event_page(&owner, 2, 100)
        .await
        .expect("retained tail remains readable");
    assert!(!retained.reset_required);
    assert_eq!(retained.events.len(), 1);
    assert_eq!(retained.events[0].sequence, 3);
    assert_eq!(retained.events[0].event_type, "message_queued");

    let repeated = pruning
        .prune_inbox_events()
        .await
        .expect("recent-event floor makes repeated pruning a no-op");
    assert_eq!(repeated.events_deleted, 0);
}

#[tokio::test]
async fn deleting_session_tombstones_inbox_payloads_without_punching_stream_holes() {
    let (sessions, inbox, pruning, retention, owner, _active) =
        services(Duration::from_secs(60)).await;
    let session = create_session(&sessions, &owner).await;
    sessions
        .delete_session(&owner, AiSessionId(session.id))
        .await
        .expect("session deletion should start");

    let payload_pass = retention
        .prune_session_content(None)
        .await
        .expect("session-bound inbox payloads should tombstone");
    assert_eq!(payload_pass.deleting_session_inbox_payloads_purged, 1);
    assert_eq!(payload_pass.deleting_sessions_finalized, 0);
    let reset = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("a tombstone should request explicit reset");
    assert!(reset.reset_required);
    assert!(reset.events.is_empty());
    assert_eq!(reset.watermark, 2);

    let second_payload_pass = retention
        .prune_session_content(None)
        .await
        .expect("the next bounded inbox payload should tombstone");
    assert_eq!(
        second_payload_pass.deleting_session_inbox_payloads_purged,
        1
    );
    assert_eq!(second_payload_pass.deleting_sessions_finalized, 0);

    let final_pass = retention
        .prune_session_content(None)
        .await
        .expect("dependency-free session should finalize");
    assert_eq!(final_pass.deleting_sessions_finalized, 1);
    assert!(
        sessions
            .delete_session(&owner, AiSessionId(session.id))
            .await
            .expect("delete replay after finalization should remain idempotent")
    );

    let prefix = pruning
        .prune_inbox_events()
        .await
        .expect("ordinary pruning should retain prefix semantics");
    assert_eq!(prefix.events_deleted, 1);
    let stale = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("the advanced prefix still requests reset");
    assert!(stale.reset_required);
    assert_eq!(stale.watermark, 2);
}

#[tokio::test]
async fn pruning_fails_closed_for_an_unconfigured_scope() {
    let (sessions, inbox, pruning, _retention, owner, _active) =
        services(Duration::from_secs(60)).await;
    sessions
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: AiScopeInput {
                    kind: "collection".to_owned(),
                    id: "unconfigured".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                title: Some("No retention policy".to_owned()),
            },
        )
        .await
        .expect("session creation is independent from retention configuration");

    let report = pruning
        .prune_inbox_events()
        .await
        .expect("missing policy is a reported fail-closed outcome");
    assert_eq!(report.streams_not_ready, 1);
    assert_eq!(report.events_deleted, 0);
    let page = inbox
        .inbox_event_page(&owner, 0, 100)
        .await
        .expect("unconfigured event remains durable");
    assert_eq!(page.events.len(), 1);
    assert!(!page.reset_required);
}
