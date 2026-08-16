#![cfg(feature = "sqlite")]

use std::sync::Arc;

use agql_auth::{
    AccessTokenMetadata, AuthPrincipal, AuthUser, CurrentPrincipalResolver, PrincipalReference,
    ResolvedPrincipal, SessionContext, SystemClock,
};
use async_trait::async_trait;
use futures::StreamExt;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::graphql::pagination::KeysetConnectionInput;
use graphql_orm::prelude::{Database, PaginationConfig, SqliteBackend};
use graphql_orm_ai::*;
use time::{Duration as TimeDuration, OffsetDateTime};
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

struct LegacyPreviewProtector;

struct StaticPrincipalResolver {
    principal: AuthPrincipal,
}

#[async_trait]
impl CurrentPrincipalResolver for StaticPrincipalResolver {
    async fn resolve(
        &self,
        reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        ResolvedPrincipal::new(
            reference.clone(),
            self.principal.clone(),
            OffsetDateTime::now_utc(),
        )
    }
}

#[async_trait]
impl AiContentProtector for LegacyPreviewProtector {
    async fn protect(
        &self,
        policy: &AiContentProtectionPolicy,
        context: &ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<ProtectedContentEnvelope, ContentProtectionError> {
        let value = if context.entity == "graphql_orm_ai_messages"
            && context.field == "protected_preview"
        {
            let text = value
                .as_str()
                .ok_or(ContentProtectionError::ValidationFailed)?;
            serde_json::json!({"text": text})
        } else {
            value
        };
        DatabaseManagedContentProtector
            .protect(policy, context, value)
            .await
    }

    async fn open(
        &self,
        policy: &AiContentProtectionPolicy,
        context: &ContentProtectionContext,
        envelope: &ProtectedContentEnvelope,
    ) -> Result<serde_json::Value, ContentProtectionError> {
        DatabaseManagedContentProtector
            .open(policy, context, envelope)
            .await
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

async fn service() -> OrmAiSessionService {
    service_with_protector_and_maximum(
        Arc::new(DatabaseManagedContentProtector),
        PaginationConfig::SECURE_MAX_LIMIT,
    )
    .await
}

async fn service_with_protector(
    content_protector: Arc<dyn AiContentProtector>,
) -> OrmAiSessionService {
    service_with_protector_and_maximum(content_protector, PaginationConfig::SECURE_MAX_LIMIT).await
}

async fn service_with_protector_and_maximum(
    content_protector: Arc<dyn AiContentProtector>,
    maximum_page_limit: i64,
) -> OrmAiSessionService {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open")
        .with_pagination_config(PaginationConfig::explicit_only(maximum_page_limit));
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-session-test-v1",
            "AI session service test",
            module.entities(),
        )
        .await
        .expect("AI schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema migration should apply to in-memory SQLite");
    OrmAiSessionService::new(
        database,
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        content_protector,
    )
}

async fn append_title_events(
    service: &OrmAiSessionService,
    owner: &AuthPrincipal,
    session_id: Uuid,
    first_revision: i64,
    count: i64,
) {
    for revision in first_revision..first_revision + count {
        let renamed = service
            .rename_session(
                owner,
                RenameAiSessionInput {
                    session_id,
                    title: format!("Title {}", revision + 1),
                    client_mutation_id: Uuid::new_v4(),
                    expected_title_revision: Some(revision),
                },
            )
            .await
            .expect("title event should commit");
        assert_eq!(renamed.title_revision, revision + 1);
    }
}

async fn collect_session_sequences(
    service: &OrmAiSessionService,
    owner: &AuthPrincipal,
    session_id: Uuid,
    first: i64,
) -> (Vec<i64>, Vec<usize>, i64, String) {
    let mut after = 0;
    let mut sequences = Vec::new();
    let mut page_lengths = Vec::new();
    let mut final_event_type = String::new();
    loop {
        let page = service
            .session_event_page(owner, AiSessionId(session_id), after, first)
            .await
            .expect("session event page should load");
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

fn scope_input() -> AiScopeInput {
    AiScopeInput {
        kind: "collection".to_owned(),
        id: "54".to_owned(),
        tenant_id: Some("tenant-1".to_owned()),
    }
}

fn run_service(sessions: &OrmAiSessionService) -> OrmAiRunService {
    OrmAiRunService::new(
        sessions.database().clone(),
        Arc::new(SystemClock),
        AiRunServiceLimits::new(TimeDuration::minutes(1), TimeDuration::hours(1), 32, 2, 8)
            .expect("run-service limits should validate"),
    )
}

#[tokio::test]
async fn owner_isolation_atomic_send_idempotency_and_windowed_reads() {
    let service = service().await;
    let owner = principal("owner");
    let stranger = principal("stranger");
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Research".to_owned()),
            },
        )
        .await
        .expect("owner creates a session");
    assert_eq!(session.stream_head, 0);
    assert!(
        service
            .session(&stranger, AiSessionId(session.id))
            .await
            .expect("cross-owner lookup is safely handled")
            .is_none()
    );

    let client_message_id = Uuid::new_v4();
    let first = service
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Find records containing the example term".to_owned(),
                attachment_ids: vec![],
                client_message_id,
            },
        )
        .await
        .expect("message and run are committed atomically");
    let replay = service
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Find records containing the example term".to_owned(),
                attachment_ids: vec![],
                client_message_id,
            },
        )
        .await
        .expect("same idempotency input returns the committed result");
    assert_eq!(first.message_id, replay.message_id);
    assert_eq!(first.run_id, replay.run_id);
    assert!(matches!(
        service
            .send_message(
                &owner,
                SendAiMessageInput {
                    session_id: session.id,
                    text: "Different content".to_owned(),
                    attachment_ids: vec![],
                    client_message_id,
                },
            )
            .await,
        Err(AiError::Conflict)
    ));

    let messages = service
        .messages(
            &owner,
            AiSessionId(session.id),
            KeysetConnectionInput {
                last: Some(20),
                ..Default::default()
            }
            .validate(20, 100)
            .expect("valid keyset request"),
        )
        .await
        .expect("bounded message window loads");
    assert_eq!(messages.edges.len(), 1);
    assert_eq!(
        messages.edges[0].node.preview,
        "Find records containing the example term"
    );
    let bootstrap = service
        .conversation_bootstrap(&owner, AiSessionId(session.id), 20, 20, 100)
        .await
        .expect("owner bootstrap should be authoritative");
    assert_eq!(bootstrap.messages.len(), 1);
    assert_eq!(bootstrap.messages[0].id, first.message_id);
    assert_eq!(bootstrap.watermark, bootstrap.session.stream_head);
    assert_eq!(bootstrap.active_runs.len(), 1);
    assert_eq!(bootstrap.active_runs[0].id, first.run_id);
    assert_eq!(bootstrap.active_runs[0].state, "queued");
    assert!(bootstrap.terminal_runs.is_empty());
    assert!(!bootstrap.reset_required);
    let after_bootstrap = service
        .session_event_page(&owner, AiSessionId(session.id), bootstrap.watermark, 100)
        .await
        .expect("replay should begin strictly after the bootstrap watermark");
    assert!(after_bootstrap.events.is_empty());
    assert_eq!(after_bootstrap.watermark, bootstrap.watermark);
    assert!(matches!(
        service
            .conversation_bootstrap(&stranger, AiSessionId(session.id), 20, 20, 100)
            .await,
        Err(AiError::NotFound)
    ));

    let blocks = service
        .message_blocks(&owner, first.message_id, None, 20)
        .await
        .expect("bounded block window loads");
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].content.0["text"],
        "Find records containing the example term"
    );

    let events = service
        .session_event_page(&owner, AiSessionId(session.id), 0, 100)
        .await
        .expect("durable event catch-up loads");
    assert_eq!(events.watermark, 1);
    assert_eq!(events.events.len(), 1);
    assert_eq!(events.events[0].event_type, "message_queued");
    assert_eq!(
        events.events[0].payload.0["runId"],
        first.run_id.to_string()
    );
}

#[tokio::test]
async fn session_event_pages_use_the_snapshotted_watermark_at_the_orm_limit() {
    let service = service().await;
    let owner = principal("event-page-owner");
    let stranger = principal("event-page-stranger");
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Initial".to_owned()),
            },
        )
        .await
        .expect("session should create");

    append_title_events(&service, &owner, session.id, 0, 99).await;
    let page_99 = service
        .session_event_page(&owner, AiSessionId(session.id), 0, 100)
        .await
        .expect("99-event page should load");
    assert_eq!(page_99.events.len(), 99);
    assert_eq!(page_99.watermark, 99);
    assert!(!page_99.has_more);

    append_title_events(&service, &owner, session.id, 99, 1).await;
    let page_100 = service
        .session_event_page(&owner, AiSessionId(session.id), 0, 100)
        .await
        .expect("100-event page should load");
    assert_eq!(page_100.events.len(), 100);
    assert_eq!(page_100.watermark, 100);
    assert!(!page_100.has_more);

    append_title_events(&service, &owner, session.id, 100, 1).await;
    let first_101 = service
        .session_event_page(&owner, AiSessionId(session.id), 0, 100)
        .await
        .expect("first 101-event page should load");
    assert_eq!(first_101.events.len(), 100);
    assert_eq!(first_101.watermark, 101);
    assert!(first_101.has_more);
    let last_101 = service
        .session_event_page(&owner, AiSessionId(session.id), 100, 100)
        .await
        .expect("last 101-event page should load");
    assert_eq!(last_101.events.len(), 1);
    assert_eq!(last_101.events[0].sequence, 101);
    assert!(!last_101.has_more);

    append_title_events(&service, &owner, session.id, 101, 244).await;
    let (sequences, page_lengths, watermark, final_event_type) =
        collect_session_sequences(&service, &owner, session.id, 100).await;
    assert_eq!(page_lengths, [100, 100, 100, 45]);
    assert_eq!(sequences, (1..=345).collect::<Vec<_>>());
    assert_eq!(watermark, 345);
    assert_eq!(final_event_type, "session_title_changed");

    append_title_events(&service, &owner, session.id, 345, 1).await;
    let later = service
        .session_event_page(&owner, AiSessionId(session.id), watermark, 100)
        .await
        .expect("the subsequent replay boundary should observe the new event");
    assert_eq!(later.watermark, 346);
    assert_eq!(later.events.len(), 1);
    assert_eq!(later.events[0].sequence, 346);
    assert_eq!(later.events[0].event_type, "session_title_changed");
    assert!(!later.has_more);

    assert!(matches!(
        service
            .session_event_page(&stranger, AiSessionId(session.id), 0, 100)
            .await,
        Err(AiError::NotFound)
    ));
    assert!(matches!(
        service
            .session_event_page(&owner, AiSessionId(Uuid::new_v4()), 0, 100)
            .await,
        Err(AiError::NotFound)
    ));
}

#[tokio::test]
async fn session_event_pages_follow_a_smaller_orm_maximum() {
    let service =
        service_with_protector_and_maximum(Arc::new(DatabaseManagedContentProtector), 7).await;
    let owner = principal("small-event-page-owner");
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Initial".to_owned()),
            },
        )
        .await
        .expect("session should create");
    append_title_events(&service, &owner, session.id, 0, 20).await;

    let (sequences, page_lengths, watermark, _) =
        collect_session_sequences(&service, &owner, session.id, 7).await;
    assert_eq!(page_lengths, [7, 7, 6]);
    assert_eq!(sequences, (1..=20).collect::<Vec<_>>());
    assert_eq!(watermark, 20);
}

#[tokio::test]
async fn terminal_event_at_sequence_101_replays_across_session_and_inbox_pages() {
    let sessions = service().await;
    let owner = principal("terminal-page-owner");
    let session = sessions
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Initial".to_owned()),
            },
        )
        .await
        .expect("session should create");
    let submitted = sessions
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Run a bounded operation".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message should queue a run");
    append_title_events(&sessions, &owner, session.id, 0, 99).await;

    let runs = run_service(&sessions);
    let lease = runs
        .claim_next("terminal-page-worker")
        .await
        .expect("run claim should succeed")
        .expect("queued run should be eligible");
    assert_eq!(lease.run_id().0, submitted.run_id);
    let lease = runs.start(&lease).await.expect("run should start");
    runs.finish(
        &lease,
        AiRunCompletion::new(
            AiRunState::RecoveryRequired,
            "provider_completion_ambiguous",
            Some("provider_completion_ambiguous".to_owned()),
            None,
        )
        .expect("recovery completion should validate"),
    )
    .await
    .expect("recovery completion should commit");
    append_title_events(&sessions, &owner, session.id, 99, 244).await;

    let (sequences, page_lengths, watermark, _) =
        collect_session_sequences(&sessions, &owner, session.id, 100).await;
    assert_eq!(page_lengths, [100, 100, 100, 45]);
    assert_eq!(sequences, (1..=345).collect::<Vec<_>>());
    assert_eq!(watermark, 345);
    let terminal_page = sessions
        .session_event_page(&owner, AiSessionId(session.id), 100, 100)
        .await
        .expect("terminal page should load");
    assert_eq!(terminal_page.events[0].sequence, 101);
    assert_eq!(terminal_page.events[0].event_type, "run_recovery_required");
    assert_eq!(terminal_page.events[0].run_id, Some(submitted.run_id));
    assert_eq!(
        terminal_page.events[0].payload.0["state"],
        "recovery_required"
    );

    let inbox = OrmAiInboxService::new(
        sessions.database().clone(),
        Arc::new(StaticPrincipalResolver {
            principal: owner.clone(),
        }),
        Arc::new(AllowAll),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    );
    let mut after = 0;
    let mut inbox_sequences = Vec::new();
    let mut inbox_page_lengths = Vec::new();
    loop {
        let page = inbox
            .inbox_event_page(&owner, after, 100)
            .await
            .expect("inbox page should load");
        assert!(!page.reset_required);
        inbox_page_lengths.push(page.events.len());
        for event in page.events {
            after = event.sequence;
            inbox_sequences.push(after);
            if event.event_type == "run_recovery_required" {
                assert_eq!(event.session_id, session.id);
                assert_eq!(event.payload.0["state"], "recovery_required");
            }
        }
        if !page.has_more {
            // The inbox also contains the session-created notification, while
            // the per-session stream begins with the first message.
            assert_eq!(page.watermark, 346);
            break;
        }
    }
    assert_eq!(inbox_page_lengths, [100, 100, 100, 46]);
    assert_eq!(inbox_sequences, (1..=346).collect::<Vec<_>>());
}

#[tokio::test]
async fn reconnect_replays_terminal_then_switches_live_without_duplication() {
    let sessions = Arc::new(service().await);
    let owner = principal("terminal-reconnect-owner");
    let session = sessions
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Reconnect".to_owned()),
            },
        )
        .await
        .expect("session should create");
    let submitted = sessions
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "Start work before disconnect".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("message should queue");
    let runs = run_service(&sessions);
    let lease = runs
        .claim_next("terminal-reconnect-worker")
        .await
        .expect("run claim should succeed")
        .expect("run should be eligible");
    let lease = runs.start(&lease).await.expect("run should start");
    runs.finish(
        &lease,
        AiRunCompletion::new(
            AiRunState::Failed,
            "provider_failed_closed",
            Some("provider_failed_closed".to_owned()),
            None,
        )
        .expect("failure should validate"),
    )
    .await
    .expect("failure should commit while client is disconnected");

    let subscriptions = OrmAiSubscriptionService::new(
        sessions.clone(),
        Arc::new(StaticPrincipalResolver {
            principal: owner.clone(),
        }),
    )
    .with_reauthorization_interval(std::time::Duration::from_secs(60));
    let mut stream = subscriptions
        .session_events(owner.clone(), AiSessionId(session.id), 1)
        .await
        .expect("subscription should open");
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("terminal replay should not block")
        .expect("stream should yield")
        .expect("terminal replay should succeed");
    let terminal_event = terminal.event.expect("terminal event should replay");
    assert_eq!(terminal_event.sequence, 2);
    assert_eq!(terminal_event.event_type, "run_failed");
    assert_eq!(terminal_event.run_id, Some(submitted.run_id));

    sessions
        .rename_session(
            &owner,
            RenameAiSessionInput {
                session_id: session.id,
                title: "Live after terminal".to_owned(),
                client_mutation_id: Uuid::new_v4(),
                expected_title_revision: Some(0),
            },
        )
        .await
        .expect("live event should commit");
    let live = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("live handoff should not block")
        .expect("stream should yield")
        .expect("live handoff should succeed");
    let live_event = live.event.expect("live event should be present");
    assert_eq!(live_event.sequence, 3);
    assert_eq!(live_event.event_type, "session_title_changed");
}

#[tokio::test]
async fn messages_read_exact_legacy_object_previews() {
    let service = service_with_protector(Arc::new(LegacyPreviewProtector)).await;
    let owner = principal("legacy-owner");
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Legacy preview".to_owned()),
            },
        )
        .await
        .expect("legacy preview session should be created");
    service
        .send_message(
            &owner,
            SendAiMessageInput {
                session_id: session.id,
                text: "legacy protected preview".to_owned(),
                attachment_ids: vec![],
                client_message_id: Uuid::new_v4(),
            },
        )
        .await
        .expect("legacy preview fixture should persist");

    let messages = service
        .messages(
            &owner,
            AiSessionId(session.id),
            KeysetConnectionInput {
                last: Some(20),
                ..Default::default()
            }
            .validate(20, 100)
            .expect("legacy preview page should validate"),
        )
        .await
        .expect("0.62.0 object preview should remain readable");
    assert_eq!(messages.edges.len(), 1);
    assert_eq!(messages.edges[0].node.preview, "legacy protected preview");
}

#[tokio::test]
async fn owner_rename_is_idempotent_revision_fenced_and_durably_replayed() {
    let service = service().await;
    let owner = principal("rename-owner");
    let stranger = principal("rename-stranger");
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: None,
            },
        )
        .await
        .expect("default-title session should be created");
    assert_eq!(session.title, "New chat");
    assert_eq!(session.title_revision, 0);

    let client_mutation_id = Uuid::new_v4();
    let renamed = service
        .rename_session(
            &owner,
            RenameAiSessionInput {
                session_id: session.id,
                title: "  Durable research  ".to_owned(),
                client_mutation_id,
                expected_title_revision: Some(0),
            },
        )
        .await
        .expect("owner rename should commit");
    assert_eq!(renamed.title, "Durable research");
    assert_eq!(renamed.title_revision, 1);
    assert_eq!(renamed.stream_head, 1);

    let replay = service
        .rename_session(
            &owner,
            RenameAiSessionInput {
                session_id: session.id,
                title: "Durable research".to_owned(),
                client_mutation_id,
                expected_title_revision: Some(0),
            },
        )
        .await
        .expect("identical mutation replay should be stable");
    assert_eq!(replay.title_revision, 1);
    assert_eq!(replay.stream_head, 1);

    assert!(matches!(
        service
            .rename_session(
                &owner,
                RenameAiSessionInput {
                    session_id: session.id,
                    title: "Different title".to_owned(),
                    client_mutation_id,
                    expected_title_revision: None,
                },
            )
            .await,
        Err(AiError::Conflict)
    ));
    assert!(matches!(
        service
            .rename_session(
                &owner,
                RenameAiSessionInput {
                    session_id: session.id,
                    title: "Stale title".to_owned(),
                    client_mutation_id: Uuid::new_v4(),
                    expected_title_revision: Some(0),
                },
            )
            .await,
        Err(AiError::Conflict)
    ));
    assert!(matches!(
        service
            .rename_session(
                &stranger,
                RenameAiSessionInput {
                    session_id: session.id,
                    title: "Cross-owner title".to_owned(),
                    client_mutation_id: Uuid::new_v4(),
                    expected_title_revision: None,
                },
            )
            .await,
        Err(AiError::NotFound)
    ));

    let events = service
        .session_event_page(&owner, AiSessionId(session.id), 0, 10)
        .await
        .expect("rename event should replay");
    assert_eq!(events.events.len(), 1);
    assert_eq!(events.events[0].event_type, "session_title_changed");
    assert_eq!(events.events[0].payload.0["title"], "Durable research");
    assert_eq!(events.events[0].payload.0["titleRevision"], 1);
    assert_eq!(events.events[0].payload.0["actor"], "user");
}

#[tokio::test]
async fn rename_rejects_blank_control_and_oversized_titles() {
    let service = service().await;
    let owner = principal("title-validation-owner");
    let session = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: None,
            },
        )
        .await
        .expect("session should be created");
    for invalid_title in ["   ".to_owned(), "bad\ntitle".to_owned(), "x".repeat(257)] {
        assert!(matches!(
            service
                .rename_session(
                    &owner,
                    RenameAiSessionInput {
                        session_id: session.id,
                        title: invalid_title,
                        client_mutation_id: Uuid::new_v4(),
                        expected_title_revision: None,
                    },
                )
                .await,
            Err(AiError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        service
            .rename_session(
                &owner,
                RenameAiSessionInput {
                    session_id: session.id,
                    title: "Valid title".to_owned(),
                    client_mutation_id: Uuid::nil(),
                    expected_title_revision: None,
                },
            )
            .await,
        Err(AiError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn archive_restore_and_session_keyset_are_bounded() {
    let service = service().await;
    let owner = principal("owner");
    let first = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("First".to_owned()),
            },
        )
        .await
        .expect("first session");
    let second = service
        .create_session(
            &owner,
            CreateAiSessionInput {
                scope: scope_input(),
                title: Some("Second".to_owned()),
            },
        )
        .await
        .expect("second session");

    let archived = service
        .archive_session(&owner, AiSessionId(first.id))
        .await
        .expect("archive uses CAS");
    assert_eq!(archived.state, "archived");
    assert!(matches!(
        service
            .send_message(
                &owner,
                SendAiMessageInput {
                    session_id: first.id,
                    text: "cannot send while archived".to_owned(),
                    attachment_ids: vec![],
                    client_message_id: Uuid::new_v4(),
                },
            )
            .await,
        Err(AiError::Conflict)
    ));
    let restored = service
        .restore_session(&owner, AiSessionId(first.id))
        .await
        .expect("restore uses CAS");
    assert_eq!(restored.state, "active");

    let page = service
        .sessions(
            &owner,
            KeysetConnectionInput {
                first: Some(1),
                include_total_count: true,
                ..Default::default()
            }
            .validate(10, 50)
            .expect("valid page"),
        )
        .await
        .expect("session page loads");
    assert_eq!(page.edges.len(), 1);
    assert!(page.page_info.has_next_page);
    assert!(page.page_info.total_count.is_none());

    let full_page = service
        .sessions(
            &owner,
            KeysetConnectionInput {
                first: Some(2),
                ..Default::default()
            }
            .validate(10, 50)
            .expect("valid full page"),
        )
        .await
        .expect("full session page loads");
    let hidden_id = full_page.edges[0].node.id;
    assert!(hidden_id == first.id || hidden_id == second.id);
    assert!(
        service
            .delete_session(&owner, AiSessionId(hidden_id))
            .await
            .expect("delete begins")
    );
    assert!(
        service
            .delete_session(&owner, AiSessionId(hidden_id))
            .await
            .expect("delete replay is idempotent")
    );
    assert!(
        service
            .session(&owner, AiSessionId(hidden_id))
            .await
            .expect("hidden lookup succeeds")
            .is_none()
    );
    let visible_page = service
        .sessions(
            &owner,
            KeysetConnectionInput {
                first: Some(1),
                ..Default::default()
            }
            .validate(10, 50)
            .expect("valid visible page"),
        )
        .await
        .expect("visible session page loads");
    assert_eq!(visible_page.edges.len(), 1);
    assert_ne!(visible_page.edges[0].node.id, hidden_id);
    assert!(!visible_page.page_info.has_next_page);
}
