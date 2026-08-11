use std::sync::{Arc, Mutex};

use agql_auth::{AccessTokenMetadata, AuthPrincipal, AuthUser, SessionContext};
use async_graphql::{EmptySubscription, Request, Schema};
use async_trait::async_trait;
use graphql_orm::graphql::pagination::{
    KeysetWindowDirection, PageInfo, ValidatedKeysetConnection,
};
use graphql_orm_ai::*;
use serde_json::json;
use uuid::Uuid;

fn principal(subject: &str) -> AuthPrincipal {
    AuthPrincipal::User(AuthUser {
        user_id: subject.to_owned(),
        session_id: Uuid::new_v4(),
        roles: vec![],
        scopes: vec!["ai:chat".to_owned()],
        session: SessionContext::default(),
        token_claims: AccessTokenMetadata::default(),
    })
}

fn page_info() -> PageInfo {
    PageInfo {
        has_next_page: false,
        has_previous_page: false,
        start_cursor: None,
        end_cursor: None,
        total_count: None,
    }
}

#[derive(Default)]
struct RecordingSessionService {
    message_page: Mutex<Option<ValidatedKeysetConnection>>,
    principal_subject: Mutex<Option<String>>,
    rename_input: Mutex<Option<RenameAiSessionInput>>,
}

#[derive(Default)]
struct RecordingInboxService {
    request: Mutex<Option<(String, i64, i64)>>,
}

#[derive(Default)]
struct RecordingCancellationService {
    request: Mutex<Option<(String, CancelAiRunInput)>>,
}

#[derive(Default)]
struct RecordingToolPreviewService {
    request: Mutex<Option<(String, AiToolCallResultPreviewInput)>>,
}

#[async_trait]
impl AiToolCallResultPreviewService for RecordingToolPreviewService {
    async fn result_preview(
        &self,
        principal: &AuthPrincipal,
        input: AiToolCallResultPreviewInput,
    ) -> Result<Option<AiToolCallResultPreviewView>, AiError> {
        *self
            .request
            .lock()
            .expect("test mutex should not be poisoned") =
            Some((principal.subject().to_owned(), input));
        Ok(Some(AiToolCallResultPreviewView {
            session_id: input.session_id,
            run_id: Uuid::from_u128(73),
            tool_call_id: input.tool_call_id,
            tool_id: "records.read".to_owned(),
            classification: "internal".to_owned(),
            preview: async_graphql::Json(json!({"recordId": "54"})),
        }))
    }
}

#[async_trait]
impl AiRunCancellationService for RecordingCancellationService {
    async fn request_cancellation(
        &self,
        principal: &AuthPrincipal,
        input: CancelAiRunInput,
    ) -> Result<AiRunCancellationView, AiError> {
        *self
            .request
            .lock()
            .expect("test mutex should not be poisoned") =
            Some((principal.subject().to_owned(), input.clone()));
        Ok(AiRunCancellationView {
            session_id: input.session_id,
            run_id: input.run_id,
            client_request_id: input.client_request_id,
            state: "cancelled".to_owned(),
            requested_at: 42,
        })
    }
}

#[async_trait]
impl AiInboxService for RecordingInboxService {
    async fn inbox_event_page(
        &self,
        principal: &AuthPrincipal,
        after_sequence: i64,
        first: i64,
    ) -> Result<AiInboxEventPage, AiError> {
        *self
            .request
            .lock()
            .expect("test mutex should not be poisoned") =
            Some((principal.subject().to_owned(), after_sequence, first));
        Ok(AiInboxEventPage {
            events: vec![],
            watermark: 7,
            has_more: false,
            reset_required: false,
        })
    }

    async fn inbox_events(
        &self,
        _principal: AuthPrincipal,
        _after_sequence: i64,
    ) -> Result<AiInboxEventStream, AiError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[async_trait]
impl AiSessionService for RecordingSessionService {
    async fn sessions(
        &self,
        principal: &AuthPrincipal,
        _page: ValidatedKeysetConnection,
    ) -> Result<AiSessionConnection, AiError> {
        *self
            .principal_subject
            .lock()
            .expect("test mutex should not be poisoned") = Some(principal.subject().to_owned());
        Ok(AiSessionConnection {
            edges: vec![],
            page_info: page_info(),
        })
    }

    async fn session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
    ) -> Result<Option<AiSessionView>, AiError> {
        Ok(None)
    }

    async fn messages(
        &self,
        principal: &AuthPrincipal,
        _session_id: AiSessionId,
        page: ValidatedKeysetConnection,
    ) -> Result<AiMessageConnection, AiError> {
        *self
            .message_page
            .lock()
            .expect("test mutex should not be poisoned") = Some(page);
        *self
            .principal_subject
            .lock()
            .expect("test mutex should not be poisoned") = Some(principal.subject().to_owned());
        Ok(AiMessageConnection {
            edges: vec![],
            page_info: page_info(),
        })
    }

    async fn message_blocks(
        &self,
        _principal: &AuthPrincipal,
        _message_id: Uuid,
        _after_block_index: Option<i64>,
        _first: i64,
    ) -> Result<Vec<AiMessageBlockView>, AiError> {
        Ok(vec![])
    }

    async fn session_event_page(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
        _after_sequence: i64,
        _first: i64,
    ) -> Result<AiSessionEventPage, AiError> {
        Ok(AiSessionEventPage {
            events: vec![],
            watermark: 0,
            has_more: false,
            reset_required: false,
        })
    }

    async fn create_session(
        &self,
        principal: &AuthPrincipal,
        input: CreateAiSessionInput,
    ) -> Result<AiSessionView, AiError> {
        Ok(AiSessionView {
            id: Uuid::new_v4(),
            scope_kind: input.scope.kind,
            scope_id: input.scope.id,
            title: input.title.unwrap_or_else(|| "New chat".to_owned()),
            title_revision: 0,
            state: "active".to_owned(),
            stream_head: 0,
            last_activity_at: 0,
            archived_at: None,
        })
        .inspect(|_| {
            *self
                .principal_subject
                .lock()
                .expect("test mutex should not be poisoned") = Some(principal.subject().to_owned());
        })
    }

    async fn rename_session(
        &self,
        principal: &AuthPrincipal,
        input: RenameAiSessionInput,
    ) -> Result<AiSessionView, AiError> {
        *self
            .principal_subject
            .lock()
            .expect("test mutex should not be poisoned") = Some(principal.subject().to_owned());
        *self
            .rename_input
            .lock()
            .expect("test mutex should not be poisoned") = Some(input.clone());
        Ok(AiSessionView {
            id: input.session_id,
            scope_kind: "collection".to_owned(),
            scope_id: "54".to_owned(),
            title: input.title,
            title_revision: 2,
            state: "active".to_owned(),
            stream_head: 4,
            last_activity_at: 10,
            archived_at: None,
        })
    }

    async fn archive_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
    ) -> Result<AiSessionView, AiError> {
        Err(AiError::NotFound)
    }

    async fn restore_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
    ) -> Result<AiSessionView, AiError> {
        Err(AiError::NotFound)
    }

    async fn delete_session(
        &self,
        _principal: &AuthPrincipal,
        _session_id: AiSessionId,
    ) -> Result<bool, AiError> {
        Ok(false)
    }

    async fn send_message(
        &self,
        _principal: &AuthPrincipal,
        _input: SendAiMessageInput,
    ) -> Result<SendAiMessagePayload, AiError> {
        Ok(SendAiMessagePayload {
            message_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
        })
    }
}

fn schema(
    service: Arc<RecordingSessionService>,
) -> Schema<AiQueryRoot, AiMutationRoot, EmptySubscription> {
    Schema::build(AiQueryRoot, AiMutationRoot, EmptySubscription)
        .data(service as Arc<dyn AiSessionService>)
        .finish()
}

#[tokio::test]
async fn message_query_defaults_to_bounded_tail_and_passes_current_principal() {
    let service = Arc::new(RecordingSessionService::default());
    let schema = schema(service.clone());
    let session_id = Uuid::new_v4();
    let response = schema
        .execute(
            Request::new(format!(
                "{{ aiMessages(sessionId: \"{session_id}\") {{ edges {{ cursor }} pageInfo {{ hasNextPage }} }} }}"
            ))
            .data(principal("user-a")),
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().expect("response JSON"),
        json!({
            "aiMessages": {
                "edges": [],
                "pageInfo": {"hasNextPage": false}
            }
        })
    );
    let page = service
        .message_page
        .lock()
        .expect("test mutex should not be poisoned")
        .clone()
        .expect("service should receive a page");
    assert_eq!(page.direction, KeysetWindowDirection::Backward);
    assert_eq!(page.limit, 50);
    assert_eq!(
        service
            .principal_subject
            .lock()
            .expect("test mutex should not be poisoned")
            .as_deref(),
        Some("user-a")
    );
}

#[tokio::test]
async fn session_roots_fail_closed_without_authentication() {
    let schema = schema(Arc::new(RecordingSessionService::default()));
    let response = schema.execute("{ aiSessions { edges { cursor } } }").await;

    assert_eq!(response.errors.len(), 1);
}

#[tokio::test]
async fn rename_mutation_passes_closed_input_and_current_principal() {
    let service = Arc::new(RecordingSessionService::default());
    let schema = schema(service.clone());
    let session_id = Uuid::new_v4();
    let mutation_id = Uuid::new_v4();
    let response = schema
        .execute(
            Request::new(format!(
                "mutation {{ renameAiSession(input: {{ sessionId: \"{session_id}\", title: \"Reviewed title\", clientMutationId: \"{mutation_id}\", expectedTitleRevision: 1 }}) {{ id title titleRevision streamHead }} }}"
            ))
            .data(principal("user-a")),
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().expect("response JSON"),
        json!({
            "renameAiSession": {
                "id": session_id,
                "title": "Reviewed title",
                "titleRevision": 2,
                "streamHead": 4,
            }
        })
    );
    let input = service
        .rename_input
        .lock()
        .expect("test mutex should not be poisoned")
        .clone()
        .expect("rename service should receive an input");
    assert_eq!(input.session_id, session_id);
    assert_eq!(input.client_mutation_id, mutation_id);
    assert_eq!(input.expected_title_revision, Some(1));
    assert_eq!(
        service
            .principal_subject
            .lock()
            .expect("test mutex should not be poisoned")
            .as_deref(),
        Some("user-a")
    );
}

#[tokio::test]
async fn cancel_run_mutation_passes_exact_pair_and_current_principal() {
    let session_service = Arc::new(RecordingSessionService::default());
    let cancellation = Arc::new(RecordingCancellationService::default());
    let schema = Schema::build(AiQueryRoot, AiMutationRoot, EmptySubscription)
        .data(session_service as Arc<dyn AiSessionService>)
        .data(cancellation.clone() as Arc<dyn AiRunCancellationService>)
        .finish();
    let session_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let client_request_id = Uuid::new_v4();
    let response = schema
        .execute(
            Request::new(format!(
                "mutation {{ cancelAiRun(input: {{ sessionId: \"{session_id}\", runId: \"{run_id}\", clientRequestId: \"{client_request_id}\" }}) {{ sessionId runId clientRequestId state requestedAt }} }}"
            ))
            .data(principal("run-owner")),
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().expect("response JSON"),
        json!({
            "cancelAiRun": {
                "sessionId": session_id,
                "runId": run_id,
                "clientRequestId": client_request_id,
                "state": "cancelled",
                "requestedAt": 42,
            }
        })
    );
    let recorded = cancellation
        .request
        .lock()
        .expect("test mutex should not be poisoned")
        .clone()
        .expect("cancellation service should receive a request");
    assert_eq!(recorded.0, "run-owner");
    assert_eq!(recorded.1.session_id, session_id);
    assert_eq!(recorded.1.run_id, run_id);
    assert_eq!(recorded.1.client_request_id, client_request_id);
}

#[tokio::test]
async fn tool_result_preview_query_passes_exact_pair_and_current_principal() {
    let session_service = Arc::new(RecordingSessionService::default());
    let preview = Arc::new(RecordingToolPreviewService::default());
    let schema = Schema::build(AiQueryRoot, AiMutationRoot, EmptySubscription)
        .data(session_service as Arc<dyn AiSessionService>)
        .data(preview.clone() as Arc<dyn AiToolCallResultPreviewService>)
        .finish();
    let session_id = Uuid::new_v4();
    let tool_call_id = Uuid::new_v4();
    let response = schema
        .execute(
            Request::new(format!(
                "{{ aiToolCallResultPreview(input: {{ sessionId: \"{session_id}\", toolCallId: \"{tool_call_id}\" }}) {{ sessionId runId toolCallId toolId classification preview }} }}"
            ))
            .data(principal("preview-owner")),
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().expect("response JSON"),
        json!({
            "aiToolCallResultPreview": {
                "sessionId": session_id,
                "runId": Uuid::from_u128(73),
                "toolCallId": tool_call_id,
                "toolId": "records.read",
                "classification": "internal",
                "preview": {"recordId": "54"},
            }
        })
    );
    let recorded = preview
        .request
        .lock()
        .expect("test mutex should not be poisoned")
        .clone()
        .expect("preview service should receive a request");
    assert_eq!(recorded.0, "preview-owner");
    assert_eq!(recorded.1.session_id, session_id);
    assert_eq!(recorded.1.tool_call_id, tool_call_id);
}

#[tokio::test]
async fn event_page_rejects_unbounded_requests_before_service_execution() {
    let service = Arc::new(RecordingSessionService::default());
    let schema = schema(service.clone());
    let response = schema
        .execute(
            Request::new(format!(
                "{{ aiSessionEventPage(sessionId: \"{}\", first: 501) {{ watermark }} }}",
                Uuid::new_v4()
            ))
            .data(principal("user-a")),
        )
        .await;

    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0]
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.get("code")),
        Some(&async_graphql::Value::from("AI_INVALID_INPUT"))
    );
}

#[tokio::test]
async fn inbox_query_uses_bounded_defaults_and_the_current_principal() {
    let session_service = Arc::new(RecordingSessionService::default());
    let inbox = Arc::new(RecordingInboxService::default());
    let schema = Schema::build(AiQueryRoot, AiMutationRoot, EmptySubscription)
        .data(session_service as Arc<dyn AiSessionService>)
        .data(inbox.clone() as Arc<dyn AiInboxService>)
        .finish();
    let response = schema
        .execute(
            Request::new(
                "{ aiInboxEventPage { watermark hasMore resetRequired events { sequence } } }",
            )
            .data(principal("user-a")),
        )
        .await;

    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().expect("response JSON"),
        json!({
            "aiInboxEventPage": {
                "watermark": 7,
                "hasMore": false,
                "resetRequired": false,
                "events": [],
            }
        })
    );
    assert_eq!(
        inbox
            .request
            .lock()
            .expect("test mutex should not be poisoned")
            .as_ref(),
        Some(&("user-a".to_owned(), 0, 100))
    );
}
