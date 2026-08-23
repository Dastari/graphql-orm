//! Owner-authorized, descriptor-bound application-tool result previews.

use agql_auth::{AuthPrincipal, ResolvedPrincipal};
use async_graphql::{InputObject, SimpleObject};
use async_trait::async_trait;
use uuid::Uuid;

use crate::{AiError, AiScope, AiToolDescriptor, ToolGraphqlRequest};

/// Exact owner request for one stored application-tool result.
#[derive(Clone, Copy, Debug, InputObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiToolCallResultPreviewInput {
    /// Exact owning session.
    pub session_id: Uuid,
    /// Exact tool call in that session.
    pub tool_call_id: Uuid,
}

/// Bounded, server-projected result safe for the current browser principal.
#[derive(Clone, Debug, SimpleObject)]
#[cfg_attr(feature = "graphql-case-pascal", graphql(rename_fields = "PascalCase"))]
pub struct AiToolCallResultPreviewView {
    /// Exact owning session.
    pub session_id: Uuid,
    /// Exact run that executed the tool.
    pub run_id: Uuid,
    /// Exact tool call.
    pub tool_call_id: Uuid,
    /// Stable registered tool ID.
    pub tool_id: String,
    /// Persisted result classification.
    pub classification: String,
    /// Host-projected tool arguments safe for the current browser principal.
    ///
    /// `None` means the host withheld the arguments. This field is independently
    /// authorized from the result and never falls back to the protected stored
    /// value.
    pub arguments: Option<async_graphql::Json<serde_json::Value>>,
    /// Host-projected value validated against the descriptor disclosure schema.
    ///
    /// A failed read tool returns only the crate-authored, content-free
    /// [`crate::AiApplicationToolFailureEnvelope`] after its persisted code and
    /// payload agree exactly.
    pub preview: async_graphql::Json<serde_json::Value>,
}

/// Current row/field authorization and projection seam for browser previews.
///
/// Implementations receive the exact registered request and an already
/// protected-and-opened result after current owner, session, scope,
/// descriptor, and tool policy checks. They must apply the application's
/// current row and field policy and return a subset suitable for this
/// principal. The library validates the returned value against the
/// fingerprinted descriptor policy and disclosure schema.
#[async_trait]
pub trait AiToolResultPreviewAuthorizer: Send + Sync {
    /// Returns a currently authorized argument projection, or `None` to
    /// disclose no arguments.
    ///
    /// The service has already rehydrated the current session owner, checked
    /// session/scope read authority, reopened the protected value, and bound
    /// the exact registered descriptor. Implementations must withhold secret
    /// values and keep the projection bounded. The library never substitutes
    /// the stored arguments when this method returns `None`.
    async fn authorize_and_project_arguments(
        &self,
        _principal: &ResolvedPrincipal,
        _scope: &AiScope,
        _descriptor: &AiToolDescriptor,
        _arguments: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AiError> {
        Ok(None)
    }

    /// Returns a currently authorized subset, or `None` to disclose nothing.
    async fn authorize_and_project(
        &self,
        principal: &ResolvedPrincipal,
        scope: &AiScope,
        descriptor: &AiToolDescriptor,
        request: &ToolGraphqlRequest,
        result: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AiError>;
}

/// Owner-authorized application-tool result preview boundary.
#[async_trait]
pub trait AiToolCallResultPreviewService: Send + Sync {
    /// Returns a bounded current-policy preview when the descriptor explicitly
    /// permits browser presentation.
    async fn result_preview(
        &self,
        principal: &AuthPrincipal,
        input: AiToolCallResultPreviewInput,
    ) -> Result<Option<AiToolCallResultPreviewView>, AiError>;
}
