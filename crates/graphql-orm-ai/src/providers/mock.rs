//! Deterministic provider used for orchestration and security tests.

#[cfg(test)]
use std::collections::VecDeque;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures::stream;

use crate::{
    AiProvider, ModelRequest, ProviderBackgroundBinding, ProviderBackgroundObservation,
    ProviderBackgroundRetrievalBinding, ProviderBackgroundRetrievalContext,
    ProviderBackgroundSubmission, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderEventStream, ProviderKind, ProviderRequestContext,
};

#[cfg(test)]
type MockEventBatches = Arc<Mutex<VecDeque<Arc<[ProviderEvent]>>>>;

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
pub(crate) enum MockBackgroundRetrievalFailure {
    RateLimited,
    Unavailable,
    CredentialUnavailable,
    Rejected,
}

/// Captured wire content. `Debug` prints a count, never the content.
#[cfg(test)]
struct WireCapture(std::sync::Mutex<Vec<String>>);

#[cfg(test)]
impl std::fmt::Debug for WireCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let entries = self.0.lock().map_or(0, |entries| entries.len());
        write!(formatter, "WireCapture({entries} requests)")
    }
}

/// Deterministic, network-free provider fixture.
#[derive(Clone, Debug)]
pub struct MockProvider {
    kind: ProviderKind,
    capabilities: ProviderCapabilities,
    events: Arc<[ProviderEvent]>,
    #[cfg(test)]
    event_batches: Option<MockEventBatches>,
    #[cfg(test)]
    background_submission: Option<(String, String, i64)>,
    #[cfg(test)]
    background_delay: Option<std::time::Duration>,
    #[cfg(test)]
    background_binding: Option<(String, u64, bool)>,
    #[cfg(test)]
    background_observation: Option<ProviderBackgroundObservation>,
    #[cfg(test)]
    background_retrieval_failure: Option<MockBackgroundRetrievalFailure>,
    #[cfg(test)]
    stream_failure: Option<crate::AiProviderFailureCategory>,
    #[cfg(test)]
    stateless_native_item_rejected: bool,
    #[cfg(test)]
    prepare_failure: Option<crate::AiProviderFailureCategory>,
    #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
    provider_session_cursor: Option<crate::AiProviderSessionCursor>,
    #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
    provider_session_activations: Arc<Mutex<Vec<crate::AiProviderSessionActivation>>>,
    #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
    discarded_provider_sessions: Arc<AtomicU64>,
    request_count: Arc<AtomicU64>,
    #[cfg(test)]
    wire_capture: Option<Arc<WireCapture>>,
}

impl MockProvider {
    /// Records, for every accepted request, the user content exactly as an
    /// adapter would send it: each JSON input value of the replayed history
    /// and the current input after
    /// [`crate::ModelDispatchReferences::wire_json`]. Crate tests only: the
    /// capture holds whatever entity references resolved to.
    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn with_wire_capture(mut self) -> Self {
        self.wire_capture = Some(Arc::new(WireCapture(std::sync::Mutex::new(Vec::new()))));
        self
    }

    /// The captured wire content, one entry per accepted request.
    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn captured_wire(&self) -> Vec<String> {
        self.wire_capture
            .as_ref()
            .and_then(|capture| capture.0.lock().ok().map(|entries| entries.clone()))
            .unwrap_or_default()
    }

    #[cfg(not(test))]
    fn capture_wire(&self, _dispatch: &crate::ModelDispatchRequest) {}

    #[cfg(test)]
    fn capture_wire(&self, dispatch: &crate::ModelDispatchRequest) {
        let Some(capture) = &self.wire_capture else {
            return;
        };
        let request = dispatch.request();
        let history = match &request.continuation {
            Some(crate::ModelContinuation::StatelessConversation { messages, .. }) => {
                messages.as_slice()
            }
            _ => &[],
        };
        let wire = history
            .iter()
            .filter_map(|message| match message {
                crate::ModelConversationMessage::User { content } => Some(content.as_slice()),
                _ => None,
            })
            .flatten()
            .chain(&request.input)
            .map(|block| match block {
                crate::ModelInputBlock::Json { value } => {
                    dispatch.references().wire_json(value).to_string()
                }
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(mut entries) = capture.0.lock() {
            entries.push(wire);
        }
    }

    /// Creates a local mock provider yielding the supplied events in order.
    pub fn new(events: impl Into<Vec<ProviderEvent>>) -> Self {
        Self {
            kind: ProviderKind::OpenAiCompatible,
            capabilities: ProviderCapabilities {
                streaming: true,
                custom_tools: true,
                structured_output: true,
                provider_retained_continuation: true,
                local: true,
                ..ProviderCapabilities::default()
            },
            events: events.into().into(),
            #[cfg(test)]
            event_batches: None,
            #[cfg(test)]
            background_submission: None,
            #[cfg(test)]
            background_delay: None,
            #[cfg(test)]
            background_binding: None,
            #[cfg(test)]
            background_observation: None,
            #[cfg(test)]
            background_retrieval_failure: None,
            #[cfg(test)]
            stream_failure: None,
            #[cfg(test)]
            stateless_native_item_rejected: false,
            #[cfg(test)]
            prepare_failure: None,
            #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
            provider_session_cursor: None,
            #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
            provider_session_activations: Arc::new(Mutex::new(Vec::new())),
            #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
            discarded_provider_sessions: Arc::new(AtomicU64::new(0)),
            request_count: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            wire_capture: None,
        }
    }

    /// Changes the provider family exposed by the fixture.
    pub fn with_kind(mut self, kind: ProviderKind) -> Self {
        self.kind = kind;
        self
    }

    /// Changes the declared capabilities exposed by the fixture.
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Returns the number of accepted requests without retaining prompt data.
    pub fn request_count(&self) -> u64 {
        self.request_count.load(Ordering::Acquire)
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn with_event_batches(
        mut self,
        batches: impl IntoIterator<Item = Vec<ProviderEvent>>,
    ) -> Self {
        self.event_batches = Some(Arc::new(Mutex::new(
            batches
                .into_iter()
                .map(Arc::<[ProviderEvent]>::from)
                .collect(),
        )));
        self
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn with_stream_failure(
        mut self,
        category: crate::AiProviderFailureCategory,
    ) -> Self {
        self.stream_failure = Some(category);
        self
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn with_stateless_native_item_rejection(mut self) -> Self {
        self.stateless_native_item_rejected = true;
        self
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn with_prepare_failure(
        mut self,
        category: crate::AiProviderFailureCategory,
    ) -> Self {
        self.prepare_failure = Some(category);
        self
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn with_provider_session_cursor(
        mut self,
        cursor: crate::AiProviderSessionCursor,
    ) -> Self {
        self.provider_session_cursor = Some(cursor);
        self
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn provider_session_activations(&self) -> Vec<crate::AiProviderSessionActivation> {
        self.provider_session_activations
            .lock()
            .expect("mock provider-session activation lock should remain available")
            .clone()
    }

    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn discarded_provider_session_count(&self) -> u64 {
        self.discarded_provider_sessions.load(Ordering::Acquire)
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
    pub(crate) fn with_background_submission(
        mut self,
        response_id: impl Into<String>,
        status: impl Into<String>,
        created_at: i64,
    ) -> Self {
        self.background_submission = Some((response_id.into(), status.into(), created_at));
        self
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
    pub(crate) fn with_background_delay(mut self, delay: std::time::Duration) -> Self {
        self.background_delay = Some(delay);
        self
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
    pub(crate) fn with_background_binding(
        mut self,
        provider_model: impl Into<String>,
        maximum_output_tokens: u64,
        provider_store: bool,
    ) -> Self {
        self.background_binding =
            Some((provider_model.into(), maximum_output_tokens, provider_store));
        self
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
    pub(crate) fn with_background_observation(
        mut self,
        observation: ProviderBackgroundObservation,
    ) -> Self {
        self.background_observation = Some(observation);
        self
    }

    #[cfg(test)]
    #[cfg_attr(not(feature = "provider-openai-core"), allow(dead_code))]
    pub(crate) fn with_background_retrieval_failure(
        mut self,
        failure: MockBackgroundRetrievalFailure,
    ) -> Self {
        self.background_retrieval_failure = Some(failure);
        self
    }
}

#[async_trait]
impl AiProvider for MockProvider {
    fn provider_kind(&self) -> ProviderKind {
        self.kind.clone()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn prepare_dispatch(
        &self,
        request: &ModelRequest,
        context: &ProviderRequestContext,
    ) -> Result<(), ProviderError> {
        context.validate_request(&self.kind, request)?;
        #[cfg(test)]
        if let Some(category) = self.prepare_failure {
            return Err(ProviderError::Classified(category));
        }
        Ok(())
    }

    async fn stream(
        &self,
        dispatch: crate::ModelDispatchRequest,
        context: ProviderRequestContext,
    ) -> Result<ProviderEventStream, ProviderError> {
        context.validate_request(&self.kind, dispatch.request())?;
        self.capture_wire(&dispatch);
        #[cfg(test)]
        if let Some(category) = self.stream_failure {
            return Err(ProviderError::Classified(category));
        }
        #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
        if let Some(session) = context.provider_session() {
            self.provider_session_activations
                .lock()
                .map_err(|_| ProviderError::Unavailable)?
                .push(session.activation());
        }
        self.request_count.fetch_add(1, Ordering::AcqRel);
        #[cfg(test)]
        let events = match &self.event_batches {
            Some(batches) => batches
                .lock()
                .map_err(|_| ProviderError::Unavailable)?
                .pop_front()
                .ok_or(ProviderError::Unavailable)?,
            None => self.events.clone(),
        };
        #[cfg(not(test))]
        let events = self.events.clone();
        #[cfg(test)]
        let stream_items = {
            let mut stream_items = events.iter().cloned().map(Ok).collect::<Vec<_>>();
            if self.stateless_native_item_rejected {
                stream_items.push(Err(ProviderError::StatelessNativeItemRejected));
            }
            stream_items
        };
        #[cfg(not(test))]
        let stream_items = events.iter().cloned().map(Ok).collect::<Vec<_>>();
        Ok(Box::pin(stream::iter(stream_items)))
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn create_empty_session(
        &self,
        _binding: &crate::AiProviderRunBinding,
        descriptor: &crate::AiProviderSessionDescriptor,
        request: &ModelRequest,
    ) -> Result<crate::AiProviderSessionCursor, ProviderError> {
        if descriptor.provider_kind() != &self.kind || descriptor.provider_model() != request.model
        {
            return Err(ProviderError::Rejected);
        }
        #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
        return self
            .provider_session_cursor
            .clone()
            .ok_or(ProviderError::Unsupported);
        #[cfg(not(all(test, any(feature = "sqlite", feature = "postgres"))))]
        Err(ProviderError::Unsupported)
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn discard_empty_session(
        &self,
        _binding: &crate::AiProviderRunBinding,
        descriptor: &crate::AiProviderSessionDescriptor,
        cursor: &crate::AiProviderSessionCursor,
    ) -> Result<(), ProviderError> {
        if descriptor.provider_kind() != &self.kind || cursor.kind().is_empty() {
            return Err(ProviderError::Rejected);
        }
        #[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
        {
            if self.provider_session_cursor.as_ref() != Some(cursor) {
                return Err(ProviderError::Rejected);
            }
            self.discarded_provider_sessions
                .fetch_add(1, Ordering::AcqRel);
            return Ok(());
        }
        #[cfg(not(all(test, any(feature = "sqlite", feature = "postgres"))))]
        Err(ProviderError::Unsupported)
    }

    async fn submit_background(
        &self,
        dispatch: crate::ModelDispatchRequest,
        context: ProviderRequestContext,
        binding: ProviderBackgroundBinding,
    ) -> Result<ProviderBackgroundSubmission, ProviderError> {
        context.validate_request(&self.kind, dispatch.request())?;
        self.capture_wire(&dispatch);
        #[cfg_attr(not(test), allow(unused_variables))]
        let (request, _) = dispatch.into_parts();
        if binding.submission_key().is_empty() || binding.provider_profile_id().is_empty() {
            return Err(ProviderError::Rejected);
        }
        #[cfg(test)]
        {
            if let Some(delay) = self.background_delay {
                tokio::time::sleep(delay).await;
            }
            let (response_id, status, created_at) = self
                .background_submission
                .clone()
                .ok_or(ProviderError::Unsupported)?;
            let maximum_output_tokens = request
                .maximum_output_tokens
                .ok_or(ProviderError::InvalidRequest)?;
            let (provider_model, maximum_output_tokens, provider_store) = self
                .background_binding
                .clone()
                .unwrap_or((request.model, maximum_output_tokens, false));
            self.request_count.fetch_add(1, Ordering::AcqRel);
            Ok(ProviderBackgroundSubmission::new(
                response_id,
                status,
                created_at,
                provider_model,
                maximum_output_tokens,
                provider_store,
            ))
        }
        #[cfg(not(test))]
        {
            let _ = binding;
            Err(ProviderError::Unsupported)
        }
    }

    async fn retrieve_background(
        &self,
        binding: ProviderBackgroundRetrievalBinding,
        context: ProviderBackgroundRetrievalContext,
    ) -> Result<ProviderBackgroundObservation, ProviderError> {
        if !context.permits(&self.kind, &binding) {
            return Err(ProviderError::EgressDenied);
        }
        #[cfg(test)]
        {
            self.request_count.fetch_add(1, Ordering::AcqRel);
            if let Some(failure) = self.background_retrieval_failure {
                return Err(match failure {
                    MockBackgroundRetrievalFailure::RateLimited => ProviderError::RateLimited,
                    MockBackgroundRetrievalFailure::Unavailable => ProviderError::Unavailable,
                    MockBackgroundRetrievalFailure::CredentialUnavailable => {
                        ProviderError::CredentialUnavailable
                    }
                    MockBackgroundRetrievalFailure::Rejected => ProviderError::Rejected,
                });
            }
            let observation = self
                .background_observation
                .clone()
                .ok_or(ProviderError::Unsupported)?;
            Ok(observation)
        }
        #[cfg(not(test))]
        {
            let _ = binding;
            Err(ProviderError::Unsupported)
        }
    }
}
