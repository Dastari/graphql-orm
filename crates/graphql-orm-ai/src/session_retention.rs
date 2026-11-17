//! Bounded host-scheduled conversational retention contracts.

use async_trait::async_trait;

use crate::AiError;

/// Bounded result of one session-retention scan page.
///
/// This report proves only the exact ORM rows removed or scrubbed by one
/// completed pass. Attachment counts prove only cleanup coordination or exact
/// metadata deletion after a separate worker confirmed object absence. They
/// do not prove that every provider-persistent file, artifact row, tool
/// dependency, unresolved accepted proposal, or an entire deleting session was
/// purged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiSessionRetentionReport {
    /// Session rows considered in this scan page.
    pub sessions_scanned: u32,
    /// Sessions whose pruning failed in this pass, for example because of a
    /// malformed row or a failed transaction. Each is left as it was and
    /// retried by a later pass; the rest of the page is still pruned.
    pub sessions_failed: u32,
    /// Sessions whose protected content, attachment state, or checkpoint
    /// references changed.
    pub sessions_changed: u32,
    /// Protected provisional legacy live-delta or ordered provider-activity
    /// event rows deleted.
    pub live_delta_events_deleted: u32,
    /// Protected event rows deleted after a session-deletion retention cutoff.
    pub deleting_session_events_deleted: u32,
    /// Protected principal-inbox payloads tombstoned after a session-deletion
    /// retention cutoff and before message content.
    pub deleting_session_inbox_payloads_purged: u32,
    /// Protected context-summary checkpoints deleted before message scrubbing.
    pub deleting_session_context_checkpoints_deleted: u32,
    /// Context-summary checkpoints invalidated by physical deletion before an
    /// ordinarily expired covered message is scrubbed.
    pub context_checkpoints_invalidated: u32,
    /// Terminal proposal payloads scrubbed after their context dependencies.
    pub deleting_session_proposal_payloads_purged: u32,
    /// Terminal tool-call payloads scrubbed after proposal dependencies.
    pub deleting_session_tool_payloads_purged: u32,
    /// Terminal approval payloads scrubbed with their exact tool calls.
    pub deleting_session_approval_payloads_purged: u32,
    /// Age-expired terminal tool-call payloads scrubbed under the current raw
    /// payload-retention policy.
    pub expired_tool_payloads_purged: u32,
    /// Age-expired terminal approval payloads scrubbed with their exact tool
    /// calls.
    pub expired_approval_payloads_purged: u32,
    /// Age-expired orphaned protected coordinator checkpoints physically
    /// deleted after their terminal history and content dependencies were
    /// re-proved.
    pub expired_run_checkpoints_deleted: u32,
    /// Terminal run pointers cleared before append-only checkpoint deletion.
    pub deleting_session_run_checkpoint_references_cleared: u32,
    /// Append-only coordinator checkpoints physically deleted after their
    /// terminal run pointers and ordinary protected sources were exhausted.
    pub deleting_session_run_checkpoints_deleted: u32,
    /// Finalized message previews scrubbed after a retention cutoff.
    pub message_contents_purged: u32,
    /// Protected message-block rows deleted with those previews.
    pub message_blocks_deleted: u32,
    /// Attachment rows moved into externally verified cleanup after a session
    /// deletion cutoff.
    pub deleting_session_attachment_cleanups_requested: u32,
    /// Fully cleaned attachment metadata rows physically deleted before their
    /// linked message content was scrubbed.
    pub deleting_session_attachments_deleted: u32,
    /// Attachment-artifact rows moved into exact-reference cleanup before
    /// their parent attachment can enter cleanup.
    pub deleting_session_attachment_artifact_cleanups_requested: u32,
    /// Fully tombstoned attachment-artifact metadata rows physically deleted
    /// before their parent attachment was cleaned.
    pub deleting_session_attachment_artifacts_deleted: u32,
    /// Durable provider-session bindings moved irreversibly into exact
    /// provider cleanup after their owning session entered deletion.
    pub deleting_session_provider_cleanups_requested: u32,
    /// Deleting sessions still waiting for exact provider-session absence and
    /// binding-row removal.
    pub provider_session_cleanups_blocked: u32,
    /// Session shells atomically finalized as `deleted` after every bounded
    /// protected/external dependency and coordinator checkpoint was proved
    /// exhausted.
    pub deleting_sessions_finalized: u32,
    /// Sessions skipped because their GraphQL-managed retention policy was
    /// absent or invalid.
    pub sessions_not_ready: u32,
    /// Sessions whose bounded transaction lost a CAS race.
    pub sessions_conflicted: u32,
    /// Eligible messages retained because their run was nonterminal or a
    /// linked attachment still owned external/protected content.
    pub messages_blocked: u32,
    /// Deleting sessions still waiting for bounded attachment/artifact proof,
    /// exact external absence, or a configured provider-file cleanup boundary.
    pub attachment_cleanups_blocked: u32,
    /// Deleting sessions whose proposal set was over-bound, nonterminal, or
    /// still accepted without an authoritative applied outcome.
    pub proposal_payload_purges_blocked: u32,
    /// Deleting sessions whose tool/approval set was over-bound, nonterminal,
    /// uncertain, or retained active authority.
    pub tool_payload_purges_blocked: u32,
    /// Sessions whose age-based raw tool/approval proof exceeded a deployment
    /// bound or could not prove an exact terminal payload graph.
    pub raw_payload_purges_blocked: u32,
    /// Sessions whose age-based protected-checkpoint purge exceeded a proof
    /// bound or could not prove exact terminal history and dependencies.
    pub raw_checkpoint_purges_blocked: u32,
    /// Deleting sessions whose run history exceeded the configured proof bound
    /// or still contained a nonterminal run.
    pub run_checkpoint_purges_blocked: u32,
    /// Opaque cursor for the next bounded session scan page. Absence means the
    /// current cycle reached the end; a later cycle starts again from `None`.
    pub next_session_cursor: Option<String>,
}

/// Host-only bounded retention backend for protected session content.
///
/// This is not a user GraphQL operation. Implementations read only the current
/// GraphQL-managed scope policy, never open protected content, never use raw
/// SQL, and must retain metadata needed for audit, usage, fencing, and restore.
#[async_trait]
pub trait AiSessionRetentionService: Send + Sync {
    /// Prunes one bounded keyset page of sessions.
    ///
    /// `after_session_cursor` must be absent at the start of a scan cycle and
    /// then set to the prior report's `next_session_cursor` until it becomes
    /// absent again.
    ///
    /// # Errors
    ///
    /// Returns a safe error for a malformed cursor, corrupt session/message/
    /// event/context state, unsafe dependency binding, arithmetic overflow, or
    /// persistence failure. Missing policies and safe per-message blockers are
    /// counted without deleting affected content.
    async fn prune_session_content(
        &self,
        after_session_cursor: Option<String>,
    ) -> Result<AiSessionRetentionReport, AiError>;
}

/// Exact provider-retained response selected for deletion by the
/// stored-response ledger.
///
/// Only the stored-response cleanup pass constructs this request, after a
/// generation-fenced claim of a ledger row that session retention released
/// once its session was deleted past the purge cutoff. It is not general
/// provider authority. The identifiers are redacted from `Debug` but remain
/// sensitive deployment metadata: never log them, persist them elsewhere, or
/// expose them to a model or client.
#[derive(Clone, PartialEq, Eq)]
pub struct AiStoredProviderResponseDeletionRequest {
    provider_kind: crate::ProviderKind,
    provider_profile_id: String,
    provider_response_id: String,
    delete_previously_requested: bool,
}

impl AiStoredProviderResponseDeletionRequest {
    #[cfg_attr(not(any(feature = "sqlite", feature = "postgres")), allow(dead_code))]
    pub(crate) fn new(
        provider_kind: crate::ProviderKind,
        provider_profile_id: String,
        provider_response_id: String,
        delete_previously_requested: bool,
    ) -> Self {
        Self {
            provider_kind,
            provider_profile_id,
            provider_response_id,
            delete_previously_requested,
        }
    }

    /// Exact provider family that retains the response.
    pub const fn provider_kind(&self) -> &crate::ProviderKind {
        &self.provider_kind
    }

    /// Exact logical provider profile whose credential stored the response.
    pub fn provider_profile_id(&self) -> &str {
        &self.provider_profile_id
    }

    /// Exact provider response identifier.
    pub fn provider_response_id(&self) -> &str {
        &self.provider_response_id
    }

    /// Whether an earlier deletion intent for this response was recorded.
    ///
    /// This is audit history, never proof of absence: the earlier request may
    /// not have reached the provider, the credential may since address another
    /// project, and intent survives backup and restore.
    pub const fn delete_previously_requested(&self) -> bool {
        self.delete_previously_requested
    }
}

impl std::fmt::Debug for AiStoredProviderResponseDeletionRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiStoredProviderResponseDeletionRequest")
            .field("provider_kind", &self.provider_kind)
            .field("provider_profile_id", &"[REDACTED]")
            .field("provider_response_id", &"[REDACTED]")
            .field(
                "delete_previously_requested",
                &self.delete_previously_requested,
            )
            .finish()
    }
}

/// Provider-specific deletion boundary for responses the provider retains.
///
/// Implementations receive one exact response for one logical profile and
/// must never list, search or select other objects.
/// `OpenAiStoredResponseDeletionService` implements it for OpenAI.
#[async_trait]
pub trait AiStoredProviderResponseDeletionService: Send + Sync {
    /// Deletes the exact response and confirms that it is absent.
    ///
    /// Return `Ok(())` only when the provider acknowledged deleting this exact
    /// response and a following retrieval returned not-found.
    ///
    /// # Errors
    ///
    /// Return [`AiError::InvalidConfiguration`] only when this boundary does
    /// not own the request's provider kind or profile and sent nothing; the
    /// ledger then keeps the row without spending an attempt and counts it in
    /// [`AiStoredResponseCleanupReport::unrouted`]. Return any other error
    /// whenever absence is not confirmed, including a not-found answer to the
    /// delete itself, an acknowledged delete whose follow-up retrieval fails,
    /// and any transport failure. The ledger retries those with backoff and
    /// finally records the response as unconfirmed; it never treats an error
    /// as deletion.
    async fn delete_and_confirm_absent(
        &self,
        request: &AiStoredProviderResponseDeletionRequest,
    ) -> Result<(), AiError>;
}

/// Bounds for one stored-response cleanup pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiStoredResponseCleanupLimits {
    maximum_batch_size: u32,
    cleanup_lease_ttl: time::Duration,
    maximum_delete_attempts: u32,
}

impl AiStoredResponseCleanupLimits {
    /// Creates validated limits with the default bound of 24 deletion
    /// attempts.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless the batch is in
    /// `1..=200` and the claim lifetime is positive and no more than one hour.
    pub fn new(
        maximum_batch_size: u32,
        cleanup_lease_ttl: time::Duration,
    ) -> Result<Self, AiError> {
        if !(1..=200).contains(&maximum_batch_size)
            || !cleanup_lease_ttl.is_positive()
            || cleanup_lease_ttl > time::Duration::hours(1)
        {
            return Err(AiError::InvalidConfiguration(
                "invalid stored-response cleanup limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_batch_size,
            cleanup_lease_ttl,
            maximum_delete_attempts: 24,
        })
    }

    /// Sets the number of deletion intents, interrupted calls included,
    /// before a response is recorded as `cleanup_unconfirmed`.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] unless the bound is in
    /// `1..=1024`.
    pub fn with_maximum_delete_attempts(mut self, maximum: u32) -> Result<Self, AiError> {
        if !(1..=1024).contains(&maximum) {
            return Err(AiError::InvalidConfiguration(
                "invalid stored-response deletion attempt bound".to_owned(),
            ));
        }
        self.maximum_delete_attempts = maximum;
        Ok(self)
    }

    /// Maximum ledger rows claimed in one pass.
    pub const fn maximum_batch_size(self) -> u32 {
        self.maximum_batch_size
    }

    /// Reclaimable cleanup claim lifetime.
    pub const fn cleanup_lease_ttl(self) -> time::Duration {
        self.cleanup_lease_ttl
    }

    /// Deletion intents before a response is recorded as unconfirmed.
    pub const fn maximum_delete_attempts(self) -> u32 {
        self.maximum_delete_attempts
    }
}

impl Default for AiStoredResponseCleanupLimits {
    fn default() -> Self {
        Self {
            maximum_batch_size: 50,
            cleanup_lease_ttl: time::Duration::minutes(5),
            maximum_delete_attempts: 24,
        }
    }
}

/// Result of one stored-response cleanup pass.
///
/// `deleted` counts only responses whose absence the deletion boundary
/// confirmed. Nothing else proves that a response is gone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AiStoredResponseCleanupReport {
    /// Responses confirmed absent; their ledger rows were removed.
    pub deleted: u32,
    /// Responses whose deletion was not confirmed and will be retried.
    pub retried: u32,
    /// Responses that reached the attempt bound and are now terminal
    /// `cleanup_unconfirmed` rows kept for operator follow-up.
    pub unconfirmed: u32,
    /// Candidates another worker claimed or changed first.
    pub deferred: u32,
    /// Responses the installed deletion boundary does not own (another
    /// provider kind, or a profile it is not bound to). Nothing was sent, no
    /// attempt was spent, and the row is offered again an hour later. A
    /// nonzero count is a configuration problem: bind or install a boundary
    /// for that profile.
    pub unrouted: u32,
}
