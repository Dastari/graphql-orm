//! Closed replay-source contracts for bounded durable subscription waits.
//!
//! Semantic capability discovery and plan/document compilation remain owned
//! by `graphql-orm-ai-tool-profiles`. This module binds those compiled values
//! to deployment-registered replay sources; registration is never authority.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use agql_auth::ResolvedPrincipal;
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AiError, AiToolDescriptor, AiToolOperationKind, GraphqlExecutionTargetId,
    GraphqlGeneratedOperationKind, ToolGraphqlRequest, ToolGraphqlResponse,
};

const MAXIMUM_SOURCE_ID_BYTES: usize = 128;
const MAXIMUM_SOURCE_VERSION_BYTES: usize = 128;
const MAXIMUM_EVENT_ID_BYTES: usize = 1_024;
const MAXIMUM_POSITION_BYTES: usize = 256 * 1024;
const ABSOLUTE_MAXIMUM_SOURCE_EVENT_BYTES: usize = 64 * 1024 * 1024;

/// Stable deployment registration for one replayable subscription source.
///
/// The descriptor contains no destination URL or credential. Its fingerprint
/// changes whenever the source implementation/version or exact semantic
/// subscription coordinate changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiReplayableSubscriptionSourceDescriptor {
    source_id: String,
    source_version: String,
    target_id: GraphqlExecutionTargetId,
    semantic_operation_fingerprint: String,
    fingerprint: String,
}

impl AiReplayableSubscriptionSourceDescriptor {
    /// Creates an exact source registration descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe identifiers/versions or a malformed
    /// semantic-operation SHA-256 fingerprint.
    pub fn new(
        source_id: impl Into<String>,
        source_version: impl Into<String>,
        target_id: GraphqlExecutionTargetId,
        semantic_operation_fingerprint: impl Into<String>,
    ) -> Result<Self, AiError> {
        let source_id = source_id.into();
        let source_version = source_version.into();
        let semantic_operation_fingerprint = semantic_operation_fingerprint.into();
        if !valid_source_id(&source_id)
            || !valid_safe_version(&source_version)
            || !valid_sha256(&semantic_operation_fingerprint)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid replayable subscription source descriptor".to_owned(),
            ));
        }
        let fingerprint = source_descriptor_fingerprint(
            &source_id,
            &source_version,
            &target_id,
            &semantic_operation_fingerprint,
        );
        Ok(Self {
            source_id,
            source_version,
            target_id,
            semantic_operation_fingerprint,
            fingerprint,
        })
    }

    /// Deployment-owned stable source ID.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Deployment-owned source protocol/implementation version.
    pub fn source_version(&self) -> &str {
        &self.source_version
    }

    /// Exact logical GraphQL target.
    pub fn target_id(&self) -> &GraphqlExecutionTargetId {
        &self.target_id
    }

    /// Exact canonical semantic-operation fingerprint.
    pub fn semantic_operation_fingerprint(&self) -> &str {
        &self.semantic_operation_fingerprint
    }

    /// Complete registration fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Opaque replay cursor plus captured watermark.
///
/// Values are meaningful only to the registered source. Durable services
/// protect this value before persistence and never expose it through browser
/// or GraphQL output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiSubscriptionReplayPosition {
    cursor: serde_json::Value,
    watermark: serde_json::Value,
    fingerprint: String,
}

impl AiSubscriptionReplayPosition {
    /// Creates a bounded opaque replay position.
    ///
    /// # Errors
    ///
    /// Returns an error when either value is missing or the canonical pair is
    /// larger than the protected persistence ceiling.
    pub fn new(
        cursor: serde_json::Value,
        watermark: serde_json::Value,
    ) -> Result<Self, AiSubscriptionSourceError> {
        if cursor.is_null() || watermark.is_null() {
            return Err(AiSubscriptionSourceError::InvalidPosition);
        }
        let encoded = serde_json::to_vec(&(&cursor, &watermark))
            .map_err(|_| AiSubscriptionSourceError::InvalidPosition)?;
        if encoded.len() > MAXIMUM_POSITION_BYTES {
            return Err(AiSubscriptionSourceError::LimitExceeded);
        }
        let fingerprint = hex::encode(Sha256::digest(encoded));
        Ok(Self {
            cursor,
            watermark,
            fingerprint,
        })
    }

    /// Source-owned opaque cursor.
    pub fn cursor(&self) -> &serde_json::Value {
        &self.cursor
    }

    /// Source-owned captured replay/live watermark.
    pub fn watermark(&self) -> &serde_json::Value {
        &self.watermark
    }

    /// Stable fingerprint of the complete protected position.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn has_valid_fingerprint(&self) -> bool {
        Self::new(self.cursor.clone(), self.watermark.clone())
            .is_ok_and(|candidate| candidate.fingerprint == self.fingerprint)
    }
}

/// One source event with the replay position committed after that event.
#[derive(Clone, Debug, PartialEq)]
pub struct AiReplayableSubscriptionEvent {
    event_id: String,
    position: AiSubscriptionReplayPosition,
    data: serde_json::Value,
}

impl AiReplayableSubscriptionEvent {
    /// Creates a bounded event emitted by a registered source.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe event ID, invalid replay position or a
    /// missing event payload. Static disclosure/result-byte limits are applied
    /// separately after fresh source authorization.
    pub fn new(
        event_id: impl Into<String>,
        position: AiSubscriptionReplayPosition,
        data: serde_json::Value,
    ) -> Result<Self, AiSubscriptionSourceError> {
        let event_id = event_id.into();
        let event_bytes = serde_json::to_vec(&data)
            .map_err(|_| AiSubscriptionSourceError::InvalidEvent)?
            .len();
        if !valid_safe_reference(&event_id, MAXIMUM_EVENT_ID_BYTES)
            || !position.has_valid_fingerprint()
            || data.is_null()
            || event_bytes > ABSOLUTE_MAXIMUM_SOURCE_EVENT_BYTES
        {
            return Err(AiSubscriptionSourceError::InvalidEvent);
        }
        Ok(Self {
            event_id,
            position,
            data,
        })
    }

    /// Stable source-owned event identity.
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    /// Replay position immediately after this event.
    pub fn position(&self) -> &AiSubscriptionReplayPosition {
        &self.position
    }

    /// Source-projected event payload pending fresh authorization/disclosure.
    pub fn data(&self) -> &serde_json::Value {
        &self.data
    }

    pub(crate) fn checkpoint_value(&self) -> serde_json::Value {
        serde_json::json!({
            "eventId": self.event_id,
            "position": self.position,
            "data": self.data,
        })
    }

    pub(crate) fn from_checkpoint_value(
        value: serde_json::Value,
    ) -> Result<Self, AiSubscriptionSourceError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Snapshot {
            event_id: String,
            position: AiSubscriptionReplayPosition,
            data: serde_json::Value,
        }
        let snapshot = serde_json::from_value::<Snapshot>(value)
            .map_err(|_| AiSubscriptionSourceError::InvalidEvent)?;
        Self::new(snapshot.event_id, snapshot.position, snapshot.data)
    }
}

/// One replay-then-live source item.
#[derive(Clone, Debug, PartialEq)]
pub enum AiReplayableSubscriptionSourceItem {
    /// One event and its atomic next replay position.
    Event(AiReplayableSubscriptionEvent),
    /// Retention removed required history. The durable run must enter the
    /// canonical recovery-required state and never skip to live delivery.
    ResetRequired,
}

/// Type-erased bounded replay-then-live source stream.
pub type AiReplayableSubscriptionStream = Pin<
    Box<
        dyn Stream<Item = Result<AiReplayableSubscriptionSourceItem, AiSubscriptionSourceError>>
            + Send,
    >,
>;

/// Exact source-open request compiled and owned by the server.
#[derive(Clone, Debug)]
pub struct AiReplayableSubscriptionOpenRequest {
    request: ToolGraphqlRequest,
    position: AiSubscriptionReplayPosition,
}

impl AiReplayableSubscriptionOpenRequest {
    pub(crate) fn new(request: ToolGraphqlRequest, position: AiSubscriptionReplayPosition) -> Self {
        Self { request, position }
    }

    /// Exact server-authored subscription operation and typed variables.
    pub fn request(&self) -> &ToolGraphqlRequest {
        &self.request
    }

    /// Exact protected replay position from which replay must begin.
    pub fn position(&self) -> &AiSubscriptionReplayPosition {
        &self.position
    }
}

/// Authenticated replay-then-live source owned by a deployment integration.
///
/// The worker supplies a freshly resolved principal at capture/open and again
/// for every event. `authorize_event` must reapply the ordinary subscription,
/// tenant, row and field policies and return only the exact compiled
/// projection. A source event is never authority by itself.
#[async_trait]
pub trait AiReplayableSubscriptionSource: Send + Sync {
    /// Captures a cursor/watermark before the run releases its coordinator
    /// lease, closing the replay-to-live gap.
    async fn capture_position(
        &self,
        principal: &ResolvedPrincipal,
        request: &ToolGraphqlRequest,
    ) -> Result<AiSubscriptionReplayPosition, AiSubscriptionSourceError>;

    /// Opens replay from the exact captured/last committed position and then
    /// follows live delivery. The stream must emit `ResetRequired` for a
    /// retention gap rather than silently skipping history.
    async fn open(
        &self,
        principal: &ResolvedPrincipal,
        request: AiReplayableSubscriptionOpenRequest,
    ) -> Result<AiReplayableSubscriptionStream, AiSubscriptionSourceError>;

    /// Rechecks current ordinary subscription/row/field authority for one
    /// event and returns the exact compiled projected response.
    async fn authorize_event(
        &self,
        principal: &ResolvedPrincipal,
        request: &ToolGraphqlRequest,
        event: &AiReplayableSubscriptionEvent,
    ) -> Result<ToolGraphqlResponse, AiSubscriptionSourceError>;
}

/// Safe replay-source failure classification.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum AiSubscriptionSourceError {
    /// Current principal, tenant, session, subscription, row or field access
    /// was denied/revoked.
    #[error("subscription source authorization failed")]
    Authorization,
    /// Required history is no longer retained.
    #[error("subscription source replay reset is required")]
    ResetRequired,
    /// Cursor or watermark shape/fingerprint is invalid.
    #[error("subscription replay position is invalid")]
    InvalidPosition,
    /// Event identity or payload is malformed.
    #[error("subscription source event is invalid")]
    InvalidEvent,
    /// A deployment/event bound was exceeded.
    #[error("subscription source limit was exceeded")]
    LimitExceeded,
    /// Source transport failed without a safe event adoption.
    #[error("subscription source is unavailable")]
    Unavailable,
}

#[derive(Clone)]
struct RegisteredReplaySource {
    descriptor: AiReplayableSubscriptionSourceDescriptor,
    source: Arc<dyn AiReplayableSubscriptionSource>,
}

/// Default-deny registry for authenticated replayable subscription sources.
///
/// A semantic `ReplayThenLive` declaration without an exact source
/// registration remains unavailable. Registration never grants execution or
/// disclosure authority.
#[derive(Clone, Default)]
pub struct AiReplayableSubscriptionSourceRegistry {
    sources: BTreeMap<(GraphqlExecutionTargetId, String), RegisteredReplaySource>,
}

impl AiReplayableSubscriptionSourceRegistry {
    /// Creates an empty, deny-all source registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one exact target/semantic-operation source.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed descriptor or duplicate coordinate.
    pub fn register(
        &mut self,
        descriptor: AiReplayableSubscriptionSourceDescriptor,
        source: Arc<dyn AiReplayableSubscriptionSource>,
    ) -> Result<(), AiError> {
        let validated = AiReplayableSubscriptionSourceDescriptor::new(
            descriptor.source_id.clone(),
            descriptor.source_version.clone(),
            descriptor.target_id.clone(),
            descriptor.semantic_operation_fingerprint.clone(),
        )?;
        if validated != descriptor {
            return Err(AiError::InvalidConfiguration(
                "subscription source descriptor fingerprint is stale".to_owned(),
            ));
        }
        let key = (
            descriptor.target_id.clone(),
            descriptor.semantic_operation_fingerprint.clone(),
        );
        if self
            .sources
            .insert(key, RegisteredReplaySource { descriptor, source })
            .is_some()
        {
            return Err(AiError::AlreadyExists(
                "replayable subscription source".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns whether an exact compiled subscription has a registered source.
    /// Discovery only; this does not authorize opening it.
    pub fn contains_compiled(&self, descriptor: &AiToolDescriptor) -> bool {
        self.resolve_compiled(descriptor).is_ok()
    }

    pub(crate) fn resolve_compiled(
        &self,
        descriptor: &AiToolDescriptor,
    ) -> Result<AiResolvedReplaySource, AiError> {
        let contract = descriptor
            .graphql_contract
            .as_ref()
            .ok_or(AiError::Forbidden)?;
        let semantic = contract.semantic_operation().ok_or(AiError::Forbidden)?;
        if descriptor.operation_kind != AiToolOperationKind::Subscription
            || semantic.kind() != GraphqlGeneratedOperationKind::Subscription
        {
            return Err(AiError::Forbidden);
        }
        let registered = self
            .sources
            .get(&(
                contract.target_id.clone(),
                semantic.operation_fingerprint().to_owned(),
            ))
            .ok_or(AiError::Forbidden)?;
        Ok(AiResolvedReplaySource {
            descriptor: registered.descriptor.clone(),
            source: registered.source.clone(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct AiResolvedReplaySource {
    pub descriptor: AiReplayableSubscriptionSourceDescriptor,
    pub source: Arc<dyn AiReplayableSubscriptionSource>,
}

fn source_descriptor_fingerprint(
    source_id: &str,
    source_version: &str,
    target_id: &GraphqlExecutionTargetId,
    operation_fingerprint: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"graphql-orm-ai/replayable-subscription-source/v1\0");
    for value in [
        source_id,
        source_version,
        target_id.as_str(),
        operation_fingerprint,
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn valid_source_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAXIMUM_SOURCE_ID_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_safe_version(value: &str) -> bool {
    valid_safe_reference(value, MAXIMUM_SOURCE_VERSION_BYTES)
}

fn valid_safe_reference(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\\'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_position_fingerprint_detects_tampering() {
        let position = AiSubscriptionReplayPosition::new(
            serde_json::json!({"sequence": 12}),
            serde_json::json!({"head": 20}),
        )
        .expect("valid position");
        assert!(position.has_valid_fingerprint());

        let mut encoded = serde_json::to_value(position).expect("serialize position");
        encoded["cursor"]["sequence"] = serde_json::json!(13);
        let tampered: AiSubscriptionReplayPosition =
            serde_json::from_value(encoded).expect("deserialize position");
        assert!(!tampered.has_valid_fingerprint());
    }

    #[test]
    fn source_descriptor_is_stably_fingerprinted() {
        let target = GraphqlExecutionTargetId::parse("workshop.graphql").expect("target");
        let first = AiReplayableSubscriptionSourceDescriptor::new(
            "workshop.labour",
            "v1",
            target.clone(),
            "a".repeat(64),
        )
        .expect("descriptor");
        let second = AiReplayableSubscriptionSourceDescriptor::new(
            "workshop.labour",
            "v1",
            target,
            "a".repeat(64),
        )
        .expect("descriptor");
        assert_eq!(first, second);
        assert!(valid_sha256(first.fingerprint()));
    }
}
