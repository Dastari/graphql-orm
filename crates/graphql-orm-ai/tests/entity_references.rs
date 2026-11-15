#![cfg(feature = "sqlite")]

//! Per-message structured entity references: a reference never grants
//! access, never reveals whether an entity exists, and stays bounded.

use std::sync::{Arc, Mutex};

use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, SessionContext};
use async_trait::async_trait;
use graphql_orm::graphql::orm::{ApplyOptions, OrmSchemaModule};
use graphql_orm::graphql::pagination::KeysetConnectionInput;
use graphql_orm::prelude::{Database, PaginationConfig, SqliteBackend};
use graphql_orm_ai::*;
use serde_json::json;
use uuid::Uuid;

struct OwnerOnly;

#[async_trait]
impl AiAccessPolicy for OwnerOnly {
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

/// Host read policy for the fixture: `widget:visible` and `widget:large`
/// exist and are readable, `widget:hidden` exists but is denied,
/// `widget:broken` fails, and nothing else exists.
#[derive(Default)]
struct Widgets {
    calls: Mutex<Vec<(String, String, String)>>,
    reveal_hidden: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AiEntityReferenceResolver for Widgets {
    async fn resolve(
        &self,
        principal: &AuthPrincipal,
        _scope: &AiScope,
        reference: &AiEntityReferenceInput,
    ) -> Result<Option<AiResolvedEntityReference>, AiError> {
        let AuthPrincipal::User(user) = principal else {
            return Ok(None);
        };
        self.calls.lock().unwrap().push((
            user.user_id.clone(),
            reference.kind.clone(),
            reference.id.clone(),
        ));
        match reference.id.as_str() {
            "visible" => Ok(Some(AiResolvedEntityReference::new(
                json!({"name": "Visible widget", "state": "active"}),
            ))),
            "hidden" if self.reveal_hidden.load(std::sync::atomic::Ordering::SeqCst) => Ok(Some(
                AiResolvedEntityReference::new(json!({"name": "Hidden widget"})),
            )),
            "large" => Ok(Some(AiResolvedEntityReference::new(
                json!({"notes": "x".repeat(4096)}),
            ))),
            "broken" => Err(AiError::PersistenceFailed),
            _ => Ok(None),
        }
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

async fn base_service() -> OrmAiSessionService {
    let database = Database::<SqliteBackend>::connect_sqlite("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open")
        .with_pagination_config(PaginationConfig::explicit_only(
            PaginationConfig::SECURE_MAX_LIMIT,
        ));
    let module = AiSchemaModule;
    let plan = database
        .schema()
        .plan_migration_to_entities(
            "ai-reference-test-v1",
            "AI entity reference test",
            module.entities(),
        )
        .await
        .expect("AI schema migration should plan");
    database
        .schema()
        .apply_migration(&plan, ApplyOptions::default())
        .await
        .expect("AI schema migration should apply");
    OrmAiRuntimeRecoveryService::new(database.clone(), Arc::new(agql_auth::SystemClock))
        .readiness_proof()
        .await
        .expect("a test store should record its first live epoch");
    OrmAiSessionService::new(
        database,
        Arc::new(OwnerOnly),
        Arc::new(ProtectionPolicy),
        Arc::new(DatabaseManagedContentProtector),
    )
}

async fn service_with(widgets: Arc<Widgets>) -> OrmAiSessionService {
    let registry = AiEntityReferenceRegistry::new(["widget"], widgets)
        .expect("registry is valid")
        .with_limits(AiEntityReferenceLimits::new(4, 32, 1024, 2048).expect("limits are valid"));
    base_service().await.with_entity_references(registry)
}

async fn session(service: &OrmAiSessionService, owner: &AuthPrincipal) -> Uuid {
    service
        .create_session(
            owner,
            CreateAiSessionInput {
                execution_selection: None,
                scope: AiScopeInput {
                    kind: "collection".to_owned(),
                    id: "54".to_owned(),
                    tenant_id: Some("tenant-1".to_owned()),
                },
                title: None,
            },
        )
        .await
        .expect("owner creates a session")
        .id
}

fn reference(id: &str) -> AiEntityReferenceInput {
    AiEntityReferenceInput {
        kind: "widget".to_owned(),
        id: id.to_owned(),
    }
}

fn message(
    session_id: Uuid,
    references: Vec<AiEntityReferenceInput>,
    client_message_id: Uuid,
) -> SendAiMessageInput {
    SendAiMessageInput {
        session_id,
        text: "What is the state of this widget?".to_owned(),
        attachment_ids: vec![],
        client_message_id,
        references,
    }
}

async fn message_count(service: &OrmAiSessionService, owner: &AuthPrincipal, id: Uuid) -> usize {
    let page = KeysetConnectionInput {
        last: Some(20),
        ..Default::default()
    }
    .validate(20, 100)
    .expect("page is valid");
    service
        .messages(owner, AiSessionId(id), page)
        .await
        .expect("owner lists messages")
        .edges
        .len()
}

#[tokio::test]
async fn references_are_rejected_without_a_registry_and_for_unknown_kinds() {
    let owner = principal("owner");
    let plain = base_service().await;
    let plain_session = session(&plain, &owner).await;
    assert!(matches!(
        plain
            .send_message(
                &owner,
                message(plain_session, vec![reference("visible")], Uuid::new_v4())
            )
            .await,
        Err(AiError::InvalidInput(_))
    ));
    assert_eq!(message_count(&plain, &owner, plain_session).await, 0);

    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets.clone()).await;
    let id = session(&service, &owner).await;
    for references in [
        vec![AiEntityReferenceInput {
            kind: "gadget".to_owned(),
            id: "visible".to_owned(),
        }],
        vec![reference("visible"), reference("visible")],
        vec![reference("")],
        vec![reference(&"i".repeat(33))],
        (0..5).map(|index| reference(&index.to_string())).collect(),
    ] {
        assert!(matches!(
            service
                .send_message(&owner, message(id, references, Uuid::new_v4()))
                .await,
            Err(AiError::InvalidInput(_))
        ));
    }
    assert_eq!(message_count(&service, &owner, id).await, 0);
    assert!(
        widgets.calls.lock().unwrap().is_empty(),
        "a rejected message must not reach the host resolver"
    );
}

#[tokio::test]
async fn denied_missing_and_failed_references_are_indistinguishable_and_carry_no_data() {
    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets.clone()).await;
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let sent = service
        .send_message(
            &owner,
            message(
                id,
                vec![
                    reference("visible"),
                    reference("hidden"),
                    reference("absent"),
                    reference("broken"),
                ],
                Uuid::new_v4(),
            ),
        )
        .await
        .expect("a message with dropped references is still accepted");

    assert_eq!(
        sent.reference_notices
            .iter()
            .map(|notice| (notice.reference_index, notice.id.as_str(), notice.code))
            .collect::<Vec<_>>(),
        vec![
            (1, "hidden", AiEntityReferenceNoticeCode::Unavailable),
            (2, "absent", AiEntityReferenceNoticeCode::Unavailable),
            (3, "broken", AiEntityReferenceNoticeCode::Unavailable),
        ]
    );
    assert_eq!(
        *widgets.calls.lock().unwrap(),
        ["visible", "hidden", "absent", "broken"].map(|id| (
            "owner".to_owned(),
            "widget".to_owned(),
            id.to_owned()
        )),
        "every reference is resolved once, as the sending principal"
    );

    let blocks = service
        .message_blocks(&owner, sent.message_id, None, 20)
        .await
        .expect("owner reads the transcript");
    assert_eq!(blocks.len(), 5);
    assert_eq!(blocks[0].kind, "text");
    assert_eq!(blocks[1].kind, AI_ENTITY_REFERENCE_BLOCK_KIND);
    assert_eq!(
        blocks[1].content.0["provenance"],
        AI_ENTITY_REFERENCE_PROVENANCE
    );
    assert_eq!(blocks[1].content.0["data"]["state"], "active");
    let model_input = entity_reference_model_input(&blocks[1].kind, &blocks[1].content.0)
        .expect("a resolved reference reaches the model as tagged data");
    let ModelInputBlock::Json { value } = model_input else {
        panic!("a reference is a JSON block");
    };
    assert_eq!(value["provenance"], AI_ENTITY_REFERENCE_PROVENANCE);
    assert_eq!(
        value["reference"],
        json!({"kind": "widget", "id": "visible"})
    );

    // Denied, missing and failed are stored and reported identically apart
    // from the identifier the sender supplied.
    for (block, id) in blocks[2..].iter().zip(["hidden", "absent", "broken"]) {
        assert_eq!(block.kind, AI_ENTITY_REFERENCE_UNAVAILABLE_BLOCK_KIND);
        assert_eq!(
            block.content.0,
            json!({"reference": {"kind": "widget", "id": id}, "status": "unavailable"})
        );
        assert_eq!(block.byte_count, 0);
        assert!(entity_reference_model_input(&block.kind, &block.content.0).is_none());
    }
}

#[tokio::test]
async fn oversized_resolved_data_is_dropped_with_a_typed_notice() {
    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets).await;
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let sent = service
        .send_message(
            &owner,
            message(
                id,
                vec![reference("large"), reference("visible")],
                Uuid::new_v4(),
            ),
        )
        .await
        .expect("message is accepted");
    assert_eq!(sent.reference_notices.len(), 1);
    assert_eq!(sent.reference_notices[0].reference_index, 0);
    assert_eq!(
        sent.reference_notices[0].code,
        AiEntityReferenceNoticeCode::TooLarge
    );
    let blocks = service
        .message_blocks(&owner, sent.message_id, None, 20)
        .await
        .expect("owner reads the transcript");
    assert_eq!(blocks[1].kind, AI_ENTITY_REFERENCE_TOO_LARGE_BLOCK_KIND);
    assert!(blocks[1].content.0.get("data").is_none());
    assert_eq!(blocks[2].kind, AI_ENTITY_REFERENCE_BLOCK_KIND);
    assert!(blocks[2].byte_count > 0 && blocks[2].byte_count <= 1024);
}

#[tokio::test]
async fn a_replay_reports_the_stored_outcome_and_a_changed_reference_list_conflicts() {
    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets.clone()).await;
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let client_message_id = Uuid::new_v4();
    let first = service
        .send_message(
            &owner,
            message(
                id,
                vec![reference("visible"), reference("hidden")],
                client_message_id,
            ),
        )
        .await
        .expect("message is accepted");
    assert_eq!(first.reference_notices.len(), 1);

    // The host would now resolve the dropped reference. The accepted message
    // is immutable, so the replay still reports what was stored.
    widgets
        .reveal_hidden
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let replay = service
        .send_message(
            &owner,
            message(
                id,
                vec![reference("visible"), reference("hidden")],
                client_message_id,
            ),
        )
        .await
        .expect("an identical retry is idempotent");
    assert_eq!(replay.message_id, first.message_id);
    assert_eq!(replay.run_id, first.run_id);
    assert_eq!(replay.reference_notices, first.reference_notices);
    assert_eq!(message_count(&service, &owner, id).await, 1);
    assert_eq!(
        service
            .message_blocks(&owner, first.message_id, None, 20)
            .await
            .expect("owner reads the transcript")
            .len(),
        3
    );

    for changed in [
        vec![reference("visible")],
        vec![reference("hidden"), reference("visible")],
        vec![],
    ] {
        assert!(matches!(
            service
                .send_message(&owner, message(id, changed, client_message_id))
                .await,
            Err(AiError::Conflict)
        ));
    }
}

#[tokio::test]
async fn a_reference_gives_a_stranger_no_access_and_never_reaches_the_resolver() {
    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets.clone()).await;
    let owner = principal("owner");
    let stranger = principal("stranger");
    let id = session(&service, &owner).await;
    assert!(matches!(
        service
            .send_message(
                &stranger,
                message(id, vec![reference("visible")], Uuid::new_v4())
            )
            .await,
        Err(AiError::NotFound)
    ));
    assert!(widgets.calls.lock().unwrap().is_empty());
    assert_eq!(message_count(&service, &owner, id).await, 0);
}
