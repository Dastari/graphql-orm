use crate::{
    AiDataSourceRef, AiError, AiRunLease, AiScope, AiSourceTrust, AiToolExecutionProvenance,
    AiToolImageReference, DataClassification,
};
use agql_auth::PrincipalReference;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Fresh disclosure purpose for an existing completed result. None grants application execution,
/// refreshes action evidence or renews the original retained-content deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiToolResultDisclosurePurpose {
    /// First disclosure after the actual effect result was durably retained.
    ToolResult,
    /// Reopen for a following provider turn or provider-native continuation.
    Continuation,
    /// Reopen protected historical output included in model inference/history.
    History,
}
// Decoding protected data grants no authority; the owning loader must compare its retained
// call/step, registered descriptor, original result checksum and every exact image reference.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedToolResultSource {
    pub(crate) format_version: u16,
    pub(crate) execution: AiToolExecutionProvenance,
    pub(crate) principal: PrincipalReference,
    pub(crate) scope: AiScope,
    pub(crate) result_sha256: String,
    pub(crate) disclosure_fingerprint: String,
    pub(crate) classification: DataClassification,
    pub(crate) trust: AiSourceTrust,
    pub(crate) delegation_reference: Option<String>,
    pub(crate) images: Vec<AiToolImageReference>,
    pub(crate) provider_cursor_fingerprint: Option<String>,
    pub(crate) effect_attempt_id: Uuid,
    pub(crate) effect_lease_generation: i64,
}

/// Parses only a redacted owning call hint from an ordinary legacy or versioned source address.
/// It proves no authority; protected origin/content must be independently reopened and matched.
pub(crate) fn source_call_hint(reference: &str) -> Option<Uuid> {
    if let Some(versioned) = reference.strip_prefix("v2:") {
        let (id, digest) = versioned.split_once(':')?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        Uuid::parse_str(id).ok().filter(|id| !id.is_nil())
    } else {
        Uuid::parse_str(reference).ok().filter(|id| !id.is_nil())
    }
}
/// Crate-derived current context for one retained protected result. It has no public constructor
/// or deserializer. Caller JSON cannot become retained-call source evidence. This proves checked
/// correlation only; current principal/resource/grant, uncancelled run fence, egress decision and
/// atomic image budget remain separately mandatory.
#[derive(Clone, Debug)]
pub struct AiToolResultDisclosureContext {
    pub(crate) source: RetainedToolResultSource,
    current_principal: PrincipalReference,
    current_run_id: crate::AiRunId,
    current_attempt_id: Uuid,
    current_lease_generation: i64,
    purpose: AiToolResultDisclosurePurpose,
}
impl AiToolResultDisclosureContext {
    pub(crate) fn from_retained(
        source: RetainedToolResultSource,
        lease: &AiRunLease,
        purpose: AiToolResultDisclosurePurpose,
    ) -> Result<Self, AiError> {
        if source.format_version != 1
            || source.execution.session_id() != lease.session_id()
            || source.execution.tool_call_id().0.is_nil()
            || source
                .principal
                .tenant_id
                .as_ref()
                .is_some_and(|tenant| source.scope.tenant_id.as_ref() != Some(tenant))
            || source.classification == DataClassification::Secret
            || source.effect_attempt_id.is_nil()
            || source.effect_lease_generation <= 0
            || source.execution.provider_session_binding_id().is_some()
                != source.provider_cursor_fingerprint.is_some()
        {
            return Err(AiError::Conflict);
        }
        Ok(Self {
            source,
            current_principal: lease.principal_reference().clone(),
            current_run_id: lease.run_id(),
            current_attempt_id: lease.attempt_id(),
            current_lease_generation: lease.lease_generation(),
            purpose,
        })
    }
    /// Original typed invocation from the independently matched retained call/step.
    pub fn execution_provenance(&self) -> &AiToolExecutionProvenance {
        &self.source.execution
    }
    /// Original safe actor reference. It is correlation only and contains no bearer token.
    pub fn original_principal_reference(&self) -> &PrincipalReference {
        &self.source.principal
    }
    /// Current disclosure actor reference, freshly resolved before the owning policy executes.
    pub fn current_principal_reference(&self) -> &PrincipalReference {
        &self.current_principal
    }
    /// Original tenant/application scope checked against retained session/call ownership.
    pub fn scope(&self) -> &AiScope {
        &self.source.scope
    }
    /// Canonical retained model-result checksum, independently compared after decryption.
    pub fn result_sha256(&self) -> &str {
        &self.source.result_sha256
    }
    /// Immutable disclosure schema used for the protected original result.
    pub fn disclosure_fingerprint(&self) -> &str {
        &self.source.disclosure_fingerprint
    }
    /// Original confidentiality preserved during later history/model inference.
    pub const fn classification(&self) -> DataClassification {
        self.source.classification
    }
    /// Original source trust; images cannot become user-provided content via an attachment alias.
    pub const fn source_trust(&self) -> AiSourceTrust {
        self.source.trust
    }
    /// Original safe delegation/grant reference, never a credential or renewed grant.
    pub fn delegation_reference(&self) -> Option<&str> {
        self.source.delegation_reference.as_deref()
    }
    /// Exact declared image references from retained output, not current caller arguments.
    pub fn images(&self) -> &[AiToolImageReference] {
        &self.source.images
    }

    /// Original opaque provider cursor digest from matched owning lifecycle metadata. It never
    /// contains the cursor or grants remote thread reuse, and is unchanged by a later rebind.
    pub fn original_provider_cursor_fingerprint(&self) -> Option<&str> {
        self.source.provider_cursor_fingerprint.as_deref()
    }

    /// Actual original effect execution attempt, which may follow a human approval wait and
    /// differ from the originating provider request. No continuation changes this value.
    pub const fn effect_attempt_id(&self) -> Uuid {
        self.source.effect_attempt_id
    }

    /// Actual original effect execution fence, separately retained from provider admission and
    /// from the current disclosure fence. It grants no current execution authority.
    pub const fn effect_lease_generation(&self) -> i64 {
        self.source.effect_lease_generation
    }
    /// Current disclosure attempt, which may differ from the original completed effect.
    pub const fn current_attempt_id(&self) -> Uuid {
        self.current_attempt_id
    }
    /// Current disclosure run; historical results preserve their distinct original run identity.
    pub const fn current_run_id(&self) -> crate::AiRunId {
        self.current_run_id
    }
    /// Current disclosure fencing generation, checked around every owning I/O.
    pub const fn current_lease_generation(&self) -> i64 {
        self.current_lease_generation
    }
    /// Current read/disclosure purpose. It never permits invoking the original tool.
    pub const fn purpose(&self) -> AiToolResultDisclosurePurpose {
        self.purpose
    }
    /// Stable origin/content/resource checksum across disclosures. Current actor/fence differences
    /// never relabel or refresh the original historical source.
    pub fn source_reference(&self) -> String {
        let value =
            serde_json::to_value(&self.source).expect("retained source has serializable metadata");
        format!(
            "v2:{}:{}",
            self.source.execution.tool_call_id().0,
            crate::tools::canonical_json_digest(&value)
        )
    }
    /// Exact redacted source metadata for current egress; this is not an allow proof.
    pub fn data_source(&self) -> AiDataSourceRef {
        AiDataSourceRef {
            kind: "application_tool_result".into(),
            reference: self.source_reference(),
            classification: self.source.classification,
            trust: self.source.trust,
        }
    }
}

/// Removes only sources independently bound by actual SDK-derived contexts. Unknown and mixed
/// caller sources retain the complete general policy check, aggregate bytes and count ceiling.
pub(crate) fn ordinary_source_component(
    manifest: &crate::AiEgressManifest,
    contexts: &[AiToolResultDisclosureContext],
) -> Result<crate::AiEgressManifest, AiError> {
    let mut ordinary = manifest.clone();
    let mut removed_images = 0_u32;
    ordinary.sources.retain(|source| {
        let matched = contexts.iter().any(|context| {
            context.data_source() == *source
                || (source.classification == context.classification()
                    && source.trust == context.source_trust()
                    && context.images().iter().any(|image| {
                        image.matches_egress_source(&context.source_reference(), source)
                    }))
        });
        if matched && source.kind == "tool_result_image" {
            removed_images = removed_images.saturating_add(1);
        }
        !matched
    });
    ordinary.attachment_count = ordinary
        .attachment_count
        .checked_sub(removed_images)
        .ok_or(AiError::EgressDenied)?;
    Ok(ordinary)
}
