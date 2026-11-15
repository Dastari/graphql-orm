#![cfg(feature = "sqlite")]

//! Per-message structured entity references: a reference never grants
//! access, never reveals whether an entity exists, stays bounded, and is
//! resolved again for the current principal at every use. Only the handle is
//! stored.

use std::sync::atomic::{AtomicBool, Ordering};
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

/// Host read policy for the fixture. `widget:visible` and `widget:large`
/// exist, `widget:hidden` exists but is denied, `widget:broken` fails and
/// nothing else exists. Access is evaluated on every call: `row_access`
/// stands for a row policy and `field_scope` for a field-level scope that
/// gates the `owner_note` field.
struct Widgets {
    calls: Mutex<Vec<(String, String, String)>>,
    row_access: AtomicBool,
    field_scope: AtomicBool,
}

impl Default for Widgets {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            row_access: AtomicBool::new(true),
            field_scope: AtomicBool::new(true),
        }
    }
}

impl Widgets {
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

const PAYLOAD_MARKER: &str = "Visible widget";
const FIELD_MARKER: &str = "field-scoped note";

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
        if !self.row_access.load(Ordering::SeqCst) {
            return Ok(None);
        }
        match reference.id.as_str() {
            "large" => Ok(Some(AiResolvedEntityReference::new(
                json!({"notes": "x".repeat(4096)}),
            ))),
            "broken" => Err(AiError::PersistenceFailed),
            "hidden" | "absent" => Ok(None),
            _ => {
                let mut data = json!({"name": PAYLOAD_MARKER, "state": "active"});
                if self.field_scope.load(Ordering::SeqCst) {
                    data["owner_note"] = json!(FIELD_MARKER);
                }
                Ok(Some(AiResolvedEntityReference::new(data)))
            }
        }
    }
}

/// Database-managed protection that records everything it is asked to store.
#[derive(Default)]
struct RecordingProtector {
    stored: Mutex<Vec<String>>,
}

#[async_trait]
impl AiContentProtector for RecordingProtector {
    async fn protect(
        &self,
        policy: &AiContentProtectionPolicy,
        context: &ContentProtectionContext,
        value: serde_json::Value,
    ) -> Result<ProtectedContentEnvelope, ContentProtectionError> {
        self.stored.lock().unwrap().push(value.to_string());
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

async fn database() -> Database<SqliteBackend> {
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
    database
}

fn service_on(
    database: Database<SqliteBackend>,
    protector: Arc<dyn AiContentProtector>,
) -> OrmAiSessionService {
    OrmAiSessionService::new(
        database,
        Arc::new(OwnerOnly),
        Arc::new(ProtectionPolicy),
        protector,
    )
}

fn registry(widgets: Arc<Widgets>) -> AiEntityReferenceRegistry {
    AiEntityReferenceRegistry::new(["widget"], widgets)
        .expect("registry is valid")
        .with_limits(AiEntityReferenceLimits::new(4, 32, 1024, 2048).expect("limits are valid"))
}

async fn base_service() -> OrmAiSessionService {
    service_on(database().await, Arc::new(DatabaseManagedContentProtector))
}

async fn service_with(widgets: Arc<Widgets>) -> OrmAiSessionService {
    base_service()
        .await
        .with_entity_references(registry(widgets))
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

fn unavailable(id: &str) -> serde_json::Value {
    json!({"reference": {"kind": "widget", "id": id}, "status": "unavailable"})
}

/// The model blocks of one message for one turn, as JSON values.
async fn turn_input(
    service: &OrmAiSessionService,
    principal: &AuthPrincipal,
    session_id: Uuid,
    message_ids: &[Uuid],
) -> Vec<Vec<serde_json::Value>> {
    service
        .entity_reference_model_input(principal, AiSessionId(session_id), message_ids)
        .await
        .expect("turn input builds")
        .into_iter()
        .map(|message| {
            message
                .blocks
                .into_iter()
                .map(|block| match block {
                    ModelInputBlock::Json { value } => value,
                    other => panic!("a reference is a JSON block, got {other:?}"),
                })
                .collect()
        })
        .collect()
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
    assert_eq!(
        widgets.call_count(),
        0,
        "a rejected message must not reach the host resolver"
    );
}

#[tokio::test]
async fn only_handles_are_stored_and_unavailable_references_are_indistinguishable() {
    let widgets = Arc::new(Widgets::default());
    let protector = Arc::new(RecordingProtector::default());
    let service = service_on(database().await, protector.clone())
        .with_entity_references(registry(widgets.clone()));
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let sent = service
        .send_message(
            &owner,
            message(
                id,
                ["visible", "hidden", "absent", "broken"]
                    .map(reference)
                    .to_vec(),
                Uuid::new_v4(),
            ),
        )
        .await
        .expect("a message with unavailable references is still accepted");

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

    // Nothing the hook returned was handed to persistence.
    {
        let stored = protector.stored.lock().unwrap();
        assert!(stored.iter().any(|value| value.contains("\"visible\"")));
        for value in stored.iter() {
            assert!(
                !value.contains(PAYLOAD_MARKER),
                "resolved data was stored: {value}"
            );
            assert!(
                !value.contains(FIELD_MARKER),
                "resolved data was stored: {value}"
            );
        }
    }

    let blocks = service
        .message_blocks(&owner, sent.message_id, None, 20)
        .await
        .expect("owner reads the transcript");
    assert_eq!(blocks.len(), 5);
    assert_eq!(blocks[0].kind, "text");
    assert!(
        blocks[1..]
            .iter()
            .all(|block| block.kind == AI_ENTITY_REFERENCE_BLOCK_KIND)
    );
    assert_eq!(
        blocks[1].content.0["provenance"],
        AI_ENTITY_REFERENCE_PROVENANCE
    );
    assert_eq!(blocks[1].content.0["data"]["state"], "active");
    // Denied, missing and failed read identically apart from the identifier
    // the sender supplied.
    for (block, id) in blocks[2..].iter().zip(["hidden", "absent", "broken"]) {
        assert_eq!(block.content.0, unavailable(id));
    }

    let turn = turn_input(&service, &owner, id, &[sent.message_id]).await;
    assert_eq!(turn.len(), 1);
    assert_eq!(turn[0][0]["status"], "resolved");
    assert_eq!(turn[0][0]["provenance"], AI_ENTITY_REFERENCE_PROVENANCE);
    assert_eq!(
        turn[0][0]["reference"],
        json!({"kind": "widget", "id": "visible"})
    );
    for block in &turn[0][1..] {
        assert_eq!(block["status"], "unavailable");
        assert!(block.get("data").is_none());
    }
}

#[tokio::test]
async fn oversized_resolved_data_is_never_delivered() {
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
    assert_eq!(blocks[1].content.0, unavailable("large"));
    assert_eq!(blocks[2].content.0["status"], "resolved");
}

/// The reviewed defect: data resolved when a message was sent kept being
/// disclosed after the sender lost access. A field scope is revoked first,
/// then the row; the next turn and the transcript follow current authority
/// each time, while the user keeps access to the session itself.
#[tokio::test]
async fn revocation_between_turns_reaches_the_next_turn_and_the_transcript() {
    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets.clone()).await;
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let sent = service
        .send_message(
            &owner,
            message(id, vec![reference("visible")], Uuid::new_v4()),
        )
        .await
        .expect("message is accepted");
    assert!(sent.reference_notices.is_empty());

    let first_turn = turn_input(&service, &owner, id, &[sent.message_id]).await;
    assert_eq!(first_turn[0][0]["data"]["owner_note"], FIELD_MARKER);
    assert_eq!(first_turn[0][0]["data"]["name"], PAYLOAD_MARKER);

    // The field scope is revoked. The row is still readable.
    widgets.field_scope.store(false, Ordering::SeqCst);
    let second_turn = turn_input(&service, &owner, id, &[sent.message_id]).await;
    assert_eq!(second_turn[0][0]["data"]["name"], PAYLOAD_MARKER);
    assert!(!second_turn[0][0].to_string().contains(FIELD_MARKER));
    let transcript = service
        .message_blocks(&owner, sent.message_id, None, 20)
        .await
        .expect("owner still reads the session");
    assert_eq!(transcript[1].content.0["data"]["name"], PAYLOAD_MARKER);
    assert!(!transcript[1].content.0.to_string().contains(FIELD_MARKER));

    // Row access is revoked. Session access is unchanged.
    widgets.row_access.store(false, Ordering::SeqCst);
    let third_turn = turn_input(&service, &owner, id, &[sent.message_id]).await;
    assert_eq!(third_turn[0].len(), 1);
    assert_eq!(third_turn[0][0]["status"], "unavailable");
    assert!(!third_turn[0][0].to_string().contains(PAYLOAD_MARKER));
    let transcript = service
        .message_blocks(&owner, sent.message_id, None, 20)
        .await
        .expect("owner still reads the session");
    assert_eq!(transcript.len(), 2);
    assert_eq!(
        transcript[0].content.0["text"],
        "What is the state of this widget?"
    );
    assert_eq!(transcript[1].content.0, unavailable("visible"));

    // Access granted again is honoured again: nothing was cached either way.
    widgets.row_access.store(true, Ordering::SeqCst);
    let fourth_turn = turn_input(&service, &owner, id, &[sent.message_id]).await;
    assert_eq!(fourth_turn[0][0]["status"], "resolved");
}

/// Exports every AI-owned table through the portable ORM backup runtime.
async fn export_snapshot(
    database: &Database<SqliteBackend>,
) -> (
    graphql_orm::graphql::orm::GraphqlOrmSchemaSnapshot,
    std::collections::BTreeMap<String, Vec<graphql_orm::graphql::orm::BackupRow>>,
) {
    use graphql_orm::graphql::orm::GraphqlOrmBackupRuntime;
    let entities = AiSchemaModule.entities().to_vec();
    let schema = database.schema_snapshot("ai-reference-test-v1", &entities);
    let mut descriptors = database.list_backup_entities(&entities);
    descriptors.sort_by_key(|descriptor| (descriptor.export_order, descriptor.table_name.clone()));
    let mut snapshot = database
        .begin_consistent_snapshot()
        .await
        .expect("backup snapshot should begin");
    let mut rows = std::collections::BTreeMap::new();
    for descriptor in &descriptors {
        let exported = database
            .export_table_rows(&mut snapshot, descriptor)
            .await
            .expect("backup rows should export");
        rows.insert(descriptor.table_name.clone(), exported);
    }
    drop(snapshot);
    (schema, rows)
}

/// A fresh, migrated, empty store with no live epoch of its own.
async fn empty_target() -> Database<SqliteBackend> {
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
    database
}

/// A real backup export and restore into a fresh store. The backup holds the
/// handle and nothing the reference resolved to, and the restored session,
/// opened where the principal has no access to the entity, shows the marker.
#[tokio::test]
async fn a_backup_holds_handles_only_and_a_restored_session_reauthorizes_on_use() {
    use graphql_orm::graphql::orm::{GraphqlOrmBackupRuntime, RestoreContext};

    let widgets = Arc::new(Widgets::default());
    let source = database().await;
    let protector = Arc::new(RecordingProtector::default());
    let original = service_on(source.clone(), protector.clone())
        .with_entity_references(registry(widgets.clone()));
    let owner = principal("owner");
    let id = session(&original, &owner).await;
    let sent = original
        .send_message(
            &owner,
            message(id, vec![reference("visible")], Uuid::new_v4()),
        )
        .await
        .expect("message is accepted");
    // The reference resolved, and was used, on the source deployment.
    let used = turn_input(&original, &owner, id, &[sent.message_id]).await;
    assert_eq!(used[0][0]["data"]["name"], PAYLOAD_MARKER);
    assert!(
        protector
            .stored
            .lock()
            .unwrap()
            .iter()
            .all(|value| !value.contains(PAYLOAD_MARKER) && !value.contains(FIELD_MARKER))
    );

    // The exported backup carries the handle and no resolved value.
    let (schema, rows) = export_snapshot(&source).await;
    let exported = serde_json::to_string(&rows).expect("backup rows serialize");
    assert!(exported.contains("visible"), "the handle is in the backup");
    assert!(!exported.contains(PAYLOAD_MARKER));
    assert!(!exported.contains(FIELD_MARKER));

    // Restore into a fresh store.
    let target = empty_target().await;
    let entities = AiSchemaModule.entities().to_vec();
    let current = target.schema_snapshot("ai-reference-test-v1", &entities);
    target
        .restore_backup_rows(&schema, &current, &rows, &RestoreContext::empty_database())
        .await
        .expect("snapshot should restore into an empty target");

    // The restored deployment's host gives the principal no access.
    let denied = Arc::new(Widgets::default());
    denied.row_access.store(false, Ordering::SeqCst);
    let restored = service_on(target.clone(), Arc::new(DatabaseManagedContentProtector))
        .with_entity_references(registry(denied.clone()));
    let transcript = restored
        .message_blocks(&owner, sent.message_id, None, 20)
        .await
        .expect("owner reads the restored transcript");
    assert_eq!(transcript.len(), 2);
    assert_eq!(
        transcript[0].content.0["text"],
        "What is the state of this widget?"
    );
    assert_eq!(transcript[1].content.0, unavailable("visible"));
    let turn = turn_input(&restored, &owner, id, &[sent.message_id]).await;
    assert_eq!(turn[0][0]["status"], "unavailable");
    assert!(!turn[0][0].to_string().contains(PAYLOAD_MARKER));
    assert_eq!(
        denied.call_count(),
        2,
        "each use asked the restored deployment's host"
    );

    // A restored deployment with no registry resolves nothing.
    let bare = service_on(target, Arc::new(DatabaseManagedContentProtector));
    assert_eq!(
        turn_input(&bare, &owner, id, &[sent.message_id]).await[0][0]["status"],
        "unavailable"
    );
    assert_eq!(
        bare.message_blocks(&owner, sent.message_id, None, 20)
            .await
            .expect("owner reads the restored transcript")[1]
            .content
            .0,
        unavailable("visible")
    );
}

/// A provider request that is replayed or rebuilt carries handles, and is
/// resolved for the current principal each time it is about to be sent.
#[tokio::test]
async fn a_request_is_resolved_again_before_every_provider_call() {
    let widgets = Arc::new(Widgets::default());
    let registry = registry(widgets.clone());
    let owner = principal("owner");
    let scope = AiScope::new("collection", "54").with_tenant_id("tenant-1");
    let resolved = registry
        .resolve_for_use(&owner, &scope, &[reference("visible")])
        .await
        .remove(0)
        .model_input_block();
    let mut request = ModelRequest {
        model: "test-model".to_owned(),
        instructions: Vec::new(),
        input: vec![
            ModelInputBlock::Text {
                text: "What is the state of this widget?".to_owned(),
            },
            resolved,
        ],
        continuation: None,
        continuation_mode: ModelContinuationMode::StatelessReplay,
        tools: Vec::new(),
        builtin_tools: Vec::new(),
        maximum_builtin_tool_calls: None,
        reasoning_summary: ModelReasoningSummaryRequest::Disabled,
        reasoning_effort: ModelReasoningEffort::Unspecified,
        output_schema: None,
        maximum_output_tokens: Some(64),
    };
    let wire = |request: &ModelRequest| serde_json::to_string(request).expect("serializes");
    assert!(wire(&request).contains(FIELD_MARKER));

    // A retry after the field scope is revoked drops the field.
    widgets.field_scope.store(false, Ordering::SeqCst);
    registry
        .resolve_request_for_use(&owner, &scope, &mut request)
        .await;
    assert!(wire(&request).contains(PAYLOAD_MARKER));
    assert!(!wire(&request).contains(FIELD_MARKER));

    // A retry after the row is revoked carries the marker.
    widgets.row_access.store(false, Ordering::SeqCst);
    registry
        .resolve_request_for_use(&owner, &scope, &mut request)
        .await;
    assert!(!wire(&request).contains(PAYLOAD_MARKER));
    assert!(wire(&request).contains(r#""status":"unavailable""#));
    assert!(wire(&request).contains("What is the state of this widget?"));

    // Withholding reduces every reference to its handle, with no data.
    widgets.row_access.store(true, Ordering::SeqCst);
    registry
        .resolve_request_for_use(&owner, &scope, &mut request)
        .await;
    assert!(wire(&request).contains(PAYLOAD_MARKER));
    withhold_request_entity_references(&mut request);
    assert!(!wire(&request).contains(PAYLOAD_MARKER));
    assert!(wire(&request).contains(r#""status":"handle""#));
}

#[tokio::test]
async fn one_turn_resolves_at_most_the_configured_number_of_references() {
    let widgets = Arc::new(Widgets::default());
    let service = base_service().await.with_entity_references(
        registry(widgets.clone()).with_limits(
            AiEntityReferenceLimits::new(4, 32, 1024, 4096)
                .expect("limits are valid")
                .with_maximum_resolutions_per_use(3)
                .expect("cap is valid"),
        ),
    );
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let mut message_ids = Vec::new();
    for group in [["a", "b"], ["c", "d"]] {
        message_ids.push(
            service
                .send_message(
                    &owner,
                    message(id, group.map(reference).to_vec(), Uuid::new_v4()),
                )
                .await
                .expect("message is accepted")
                .message_id,
        );
    }
    let before = widgets.call_count();
    let turn = turn_input(&service, &owner, id, &message_ids).await;
    assert_eq!(
        turn.iter()
            .flatten()
            .map(|block| block["status"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["resolved", "resolved", "resolved", "unavailable"]
    );
    assert_eq!(
        turn[1][1]["reference"],
        json!({"kind": "widget", "id": "d"})
    );
    assert_eq!(
        widgets.call_count() - before,
        3,
        "the host hook is called at most the cap per turn"
    );

    assert!(matches!(
        service
            .entity_reference_model_input(
                &owner,
                AiSessionId(id),
                &[message_ids[0], message_ids[0]]
            )
            .await,
        Err(AiError::InvalidInput(_))
    ));
    assert!(matches!(
        service
            .entity_reference_model_input(&owner, AiSessionId(id), &[Uuid::new_v4()])
            .await,
        Err(AiError::NotFound)
    ));
}

#[tokio::test]
async fn a_replay_stores_nothing_new_and_a_changed_reference_list_conflicts() {
    let widgets = Arc::new(Widgets::default());
    let service = service_with(widgets.clone()).await;
    let owner = principal("owner");
    let id = session(&service, &owner).await;
    let client_message_id = Uuid::new_v4();
    let references = vec![reference("visible"), reference("hidden")];
    let first = service
        .send_message(&owner, message(id, references.clone(), client_message_id))
        .await
        .expect("message is accepted");
    assert_eq!(first.reference_notices.len(), 1);

    let replay = service
        .send_message(&owner, message(id, references, client_message_id))
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
    assert_eq!(widgets.call_count(), 0);
    assert_eq!(message_count(&service, &owner, id).await, 0);

    let sent = service
        .send_message(
            &owner,
            message(id, vec![reference("visible")], Uuid::new_v4()),
        )
        .await
        .expect("owner sends");
    let after_send = widgets.call_count();
    assert!(matches!(
        service
            .entity_reference_model_input(&stranger, AiSessionId(id), &[sent.message_id])
            .await,
        Err(AiError::NotFound)
    ));
    assert!(matches!(
        service
            .message_blocks(&stranger, sent.message_id, None, 20)
            .await,
        Err(AiError::NotFound)
    ));
    assert_eq!(widgets.call_count(), after_send);
}
