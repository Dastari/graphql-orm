#![cfg(feature = "sqlite")]

use std::sync::Arc;

use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, SessionContext};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::graphql::pagination::KeysetConnectionInput;
use graphql_orm::prelude::{Database, SqliteBackend};
use graphql_orm_ai::*;
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
    service_with_protector(Arc::new(DatabaseManagedContentProtector)).await
}

async fn service_with_protector(
    content_protector: Arc<dyn AiContentProtector>,
) -> OrmAiSessionService {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open");
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

fn scope_input() -> AiScopeInput {
    AiScopeInput {
        kind: "collection".to_owned(),
        id: "54".to_owned(),
        tenant_id: Some("tenant-1".to_owned()),
    }
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
