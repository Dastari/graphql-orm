//! Backend-neutral default-deny GraphQL tool descriptors.

use graphql_orm_operation_catalog::GraphqlResolverOperationDescriptor;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AiError, DataClassification, GraphqlOperationContract, canonical_json::canonical_json_bytes,
};

/// Stable validated tool identifier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AiToolId(String);

impl AiToolId {
    /// Parses a stable lower-case namespaced tool identifier.
    ///
    /// # Errors
    ///
    /// Returns [`AiError::InvalidConfiguration`] for an empty or unsafe ID.
    pub fn parse(value: impl Into<String>) -> Result<Self, AiError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            });
        if !valid {
            return Err(AiError::InvalidConfiguration(
                "tool IDs must be lower-case ASCII names".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// GraphQL operation kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiToolOperationKind {
    /// Query operation.
    Query,
    /// Mutation operation.
    Mutation,
    /// Subscription/watch operation.
    Subscription,
    /// AI-owned internal operation such as proposal emission.
    Internal,
}

/// Ownership domain used to prevent recursive AI control-plane invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiToolOperationDomain {
    /// Host application operation executed through ordinary authorization.
    Application,
    /// AI-owned structured proposal staging operation.
    ProposalStaging,
    /// AI session/configuration/approval/tool-discovery control plane.
    AiControlPlane,
    /// GraphQL schema introspection or discovery operation.
    SchemaIntrospection,
}

/// Host classification for derive-generated GraphQL resolvers.
///
/// Generated resolver metadata is discovery and drift detection, not
/// authorization or proof that an operation belongs to the host application
/// rather than an AI control plane. Implementations must classify only
/// reviewed application resolvers as callable. Ordinary resolver
/// authorization remains authoritative after this static decision.
pub trait AiGeneratedGraphqlOperationPolicy: Send + Sync {
    /// Returns whether the exact generated resolver may enter the application
    /// tool catalog.
    fn is_application_operation(&self, operation: &GraphqlResolverOperationDescriptor) -> bool;
}

/// Fail-closed generated resolver classifier.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllAiGeneratedGraphqlOperationPolicy;

impl AiGeneratedGraphqlOperationPolicy for DenyAllAiGeneratedGraphqlOperationPolicy {
    fn is_application_operation(&self, _operation: &GraphqlResolverOperationDescriptor) -> bool {
        false
    }
}

/// Rollout maturity ceiling for agent capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMaturity {
    /// Read-only application operations.
    ReadOnly,
    /// Writes only AI-owned structured proposals.
    ProposalOnly,
    /// Explicitly registered, supervised application mutation.
    SupervisedWrite,
    /// Future autonomous application writes; disabled by default.
    AutonomousWrite,
}

/// Default risk class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiToolRisk {
    /// Bounded internal read.
    ReadOnly,
    /// AI-owned proposal staging.
    Proposal,
    /// Proven idempotent low-impact write.
    LowRiskWrite,
    /// Non-idempotent application write.
    NonIdempotentWrite,
    /// Publish, delete, permission, external send, or similar impact.
    HighImpact,
    /// Credential or secret operation; not model-callable by default.
    Secret,
}

/// Approval rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiApprovalRule {
    /// No per-call approval after explicit tool enablement.
    None,
    /// Policy decides using context.
    Policy,
    /// Expiring argument-bound one-shot approval.
    OneShot,
    /// Operation is never model-callable.
    Never,
}

/// Explicit descriptor policy for an owner-visible browser result preview.
///
/// Tool results remain non-browser-disclosable unless this policy is present.
/// The browser path is independently reauthorized and may only return a
/// schema-valid subset of the already reviewed model disclosure projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiBrowserResultPreviewPolicy {
    /// Maximum serialized preview bytes.
    pub maximum_bytes: u64,
    /// Maximum aggregate list records in the preview.
    pub maximum_records: u32,
    /// Maximum JSON nesting depth accepted by the browser response.
    pub maximum_depth: u16,
    /// Highest classification that may be rendered in a browser.
    pub maximum_classification: DataClassification,
}

impl AiBrowserResultPreviewPolicy {
    /// Creates bounded browser-preview policy.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or excessive byte, record, or depth bounds,
    /// or when browser disclosure would permit `Secret` content.
    pub fn new(
        maximum_bytes: u64,
        maximum_records: u32,
        maximum_depth: u16,
        maximum_classification: DataClassification,
    ) -> Result<Self, AiError> {
        if !(1..=1024 * 1024).contains(&maximum_bytes)
            || !(1..=100_000).contains(&maximum_records)
            || !(1..=32).contains(&maximum_depth)
            || maximum_classification == DataClassification::Secret
        {
            return Err(AiError::InvalidConfiguration(
                "invalid browser result preview limits".to_owned(),
            ));
        }
        Ok(Self {
            maximum_bytes,
            maximum_records,
            maximum_depth,
            maximum_classification,
        })
    }
}

/// Server-authored application tool descriptor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiToolDescriptor {
    /// Stable ID.
    pub id: AiToolId,
    /// Human/model-facing description without sensitive schema details.
    pub description: String,
    /// Operation kind.
    pub operation_kind: AiToolOperationKind,
    /// Owning operation domain used for recursion prevention.
    pub operation_domain: AiToolOperationDomain,
    /// Server-authored GraphQL document. Empty only for internal tools.
    pub document: String,
    /// JSON Schema 2020-12 argument schema.
    pub argument_schema: serde_json::Value,
    /// Result projection identifier/expression controlled by the server.
    pub result_projection: String,
    /// Exact local/remote GraphQL contract for non-internal tools.
    pub graphql_contract: Option<GraphqlOperationContract>,
    /// Capability maturity.
    pub maturity: ToolMaturity,
    /// Default risk.
    pub risk: AiToolRisk,
    /// Approval rule.
    pub approval: AiApprovalRule,
    /// Maximum result bytes before artifacting/truncation.
    pub maximum_result_bytes: u64,
    /// Maximum result records.
    pub maximum_result_records: u32,
    /// Maximum model-facing data classification.
    pub maximum_classification: DataClassification,
    /// Whether retries are safe with a stable idempotency key.
    pub idempotent: bool,
    /// Optional reviewed browser-preview policy. Missing means never expose a
    /// stored result to an application frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_result_preview: Option<AiBrowserResultPreviewPolicy>,
    /// Stable fingerprint over the complete contract.
    pub fingerprint: String,
}

impl AiToolDescriptor {
    /// Creates a descriptor with secure defaults.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid ID, missing description, or a
    /// non-internal operation without a server-authored document.
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        operation_kind: AiToolOperationKind,
        document: impl Into<String>,
        argument_schema: serde_json::Value,
    ) -> Result<Self, AiError> {
        let id = AiToolId::parse(id)?;
        let description = description.into();
        let document = document.into();
        if description.trim().is_empty() {
            return Err(AiError::InvalidConfiguration(
                "tool description must not be empty".to_owned(),
            ));
        }
        if operation_kind != AiToolOperationKind::Internal && document.trim().is_empty() {
            return Err(AiError::InvalidConfiguration(
                "GraphQL tools require a server-authored document".to_owned(),
            ));
        }

        let mut descriptor = Self {
            id,
            description,
            operation_kind,
            operation_domain: if operation_kind == AiToolOperationKind::Internal {
                AiToolOperationDomain::ProposalStaging
            } else {
                AiToolOperationDomain::Application
            },
            document,
            argument_schema,
            result_projection: String::new(),
            graphql_contract: None,
            maturity: ToolMaturity::ReadOnly,
            risk: AiToolRisk::ReadOnly,
            approval: AiApprovalRule::None,
            maximum_result_bytes: 64 * 1024,
            maximum_result_records: 100,
            maximum_classification: DataClassification::Internal,
            idempotent: true,
            browser_result_preview: None,
            fingerprint: String::new(),
        };
        descriptor.refresh_fingerprint();
        Ok(descriptor)
    }

    /// Sets maturity and refreshes the fingerprint.
    pub fn with_maturity(mut self, maturity: ToolMaturity) -> Self {
        self.maturity = maturity;
        self.refresh_fingerprint();
        self
    }

    /// Sets risk and approval behavior.
    pub fn with_risk(mut self, risk: AiToolRisk, approval: AiApprovalRule) -> Self {
        self.risk = risk;
        self.approval = approval;
        self.refresh_fingerprint();
        self
    }

    /// Sets a bounded result projection.
    pub fn with_result_projection(mut self, projection: impl Into<String>) -> Self {
        self.result_projection = projection.into();
        self.refresh_fingerprint();
        self
    }

    /// Binds the tool to an exact local/remote target and static operation contract.
    pub fn with_graphql_contract(mut self, contract: GraphqlOperationContract) -> Self {
        self.graphql_contract = Some(contract);
        self.refresh_fingerprint();
        self
    }

    /// Sets the reviewed operation ownership domain.
    pub fn with_operation_domain(mut self, domain: AiToolOperationDomain) -> Self {
        self.operation_domain = domain;
        self.refresh_fingerprint();
        self
    }

    /// Sets output limits.
    pub fn with_output_limits(mut self, bytes: u64, records: u32) -> Self {
        self.maximum_result_bytes = bytes;
        self.maximum_result_records = records;
        self.refresh_fingerprint();
        self
    }

    /// Sets the maximum model-facing classification and refreshes the
    /// immutable descriptor fingerprint.
    pub fn with_maximum_classification(mut self, classification: DataClassification) -> Self {
        self.maximum_classification = classification;
        self.refresh_fingerprint();
        self
    }

    /// Sets whether stable-key retries are safe and refreshes the immutable
    /// descriptor fingerprint.
    pub fn with_idempotent(mut self, idempotent: bool) -> Self {
        self.idempotent = idempotent;
        self.refresh_fingerprint();
        self
    }

    /// Enables a separately bounded owner-visible result preview.
    #[must_use]
    pub fn with_browser_result_preview(mut self, policy: AiBrowserResultPreviewPolicy) -> Self {
        self.browser_result_preview = Some(policy);
        self.refresh_fingerprint();
        self
    }

    fn refresh_fingerprint(&mut self) {
        self.fingerprint.clear();
        let encoded = canonical_json_bytes(self);
        self.fingerprint = hex::encode(Sha256::digest(encoded));
    }

    /// Recomputes and validates the complete descriptor fingerprint.
    #[doc(hidden)]
    pub fn has_valid_fingerprint(&self) -> bool {
        let mut canonical = self.clone();
        canonical.refresh_fingerprint();
        canonical.fingerprint == self.fingerprint
    }
}

/// Detects AI-control-plane or introspection names in a GraphQL document.
#[doc(hidden)]
pub fn contains_forbidden_graphql_name(document: &str) -> bool {
    const FORBIDDEN: &[&str] = &[
        "aisessions",
        "aisession",
        "aimessages",
        "aimessageblocks",
        "aisessioneventpage",
        "aisessionevents",
        "aiproviderprofiles",
        "aicontentprotectionpolicy",
        "createaisession",
        "renameaisession",
        "archiveaisession",
        "restoreaisession",
        "deleteaisession",
        "sendaimessage",
        "upsertaiproviderprofile",
        "setaiprovidercredential",
        "removeaiprovidercredential",
        "setaicontentprotectionpolicy",
        "aitooldiscovery",
        "aitools",
        "aiapprovals",
    ];

    graphql_names(document).any(|name| {
        if name.starts_with("__") && name != "__typename" {
            return true;
        }
        let normalized: String = name
            .bytes()
            .filter(|byte| *byte != b'_')
            .map(|byte| byte.to_ascii_lowercase() as char)
            .collect();
        FORBIDDEN.contains(&normalized.as_str())
    })
}

fn graphql_names(document: &str) -> impl Iterator<Item = &str> {
    let bytes = document.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'#' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'"' => {
                let triple = bytes.get(index..index + 3) == Some(b"\"\"\"");
                index += if triple { 3 } else { 1 };
                while index < bytes.len() {
                    if triple && bytes.get(index..index + 3) == Some(b"\"\"\"") {
                        index += 3;
                        break;
                    }
                    if !triple && bytes[index] == b'"' {
                        index += 1;
                        break;
                    }
                    if bytes[index] == b'\\' && !triple {
                        index = (index + 2).min(bytes.len());
                    } else {
                        index += 1;
                    }
                }
            }
            byte if byte == b'_' || byte.is_ascii_alphabetic() => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index] == b'_'
                        || bytes[index].is_ascii_alphabetic()
                        || bytes[index].is_ascii_digit())
                {
                    index += 1;
                }
                names.push(&document[start..index]);
            }
            _ => index += 1,
        }
    }
    names.into_iter()
}
