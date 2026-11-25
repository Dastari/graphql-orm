//! Generated mutations resolved through the fixed capability broker.
//!
//! A direct provider call has one identity: the offered definition is the
//! capability that runs. A broker execute call has two. The provider-facing
//! identity is the frozen `graphql.capabilities.execute` definition the
//! provider called; the resolved identity is the exact generated mutation and
//! closed plan the broker resolved it to. Tool results, provider continuation
//! and every provider-facing correlation use the provider-facing identity.
//! Classification, hierarchical rules, approval binding, preview, durable
//! arguments and execution use the resolved one.
use super::*;
use crate::AiProviderToolCall;

/// One provider call resolved to the exact application capability it runs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedApplicationCall {
    pub(crate) provider_call_id: String,
    pub(crate) provider_tool_id: AiToolId,
    pub(crate) provider_tool_fingerprint: String,
    pub(crate) tool_id: AiToolId,
    pub(crate) tool_fingerprint: String,
    pub(crate) arguments: serde_json::Value,
}

impl ResolvedApplicationCall {
    /// The identity of a direct call: both identities are the offered
    /// definition.
    pub(crate) fn direct(call: &AiProviderToolCall) -> Self {
        Self {
            provider_call_id: call.call_id().to_owned(),
            provider_tool_id: call.tool_id().clone(),
            provider_tool_fingerprint: call.tool_fingerprint().to_owned(),
            tool_id: call.tool_id().clone(),
            tool_fingerprint: call.tool_fingerprint().to_owned(),
            arguments: call.arguments().clone(),
        }
    }

    /// Rebuilds the resolved call of one durable tool-call row from its
    /// stored identities and opened protected arguments.
    ///
    /// A broker-routed row is re-resolved from its stored resolved identity
    /// only; the broker's process-local loaded binding is never needed again.
    pub(crate) fn from_record(
        call: &AiToolCallRecord,
        arguments: serde_json::Value,
    ) -> Result<Self, AiError> {
        if !valid_provider_identity(call) {
            return Err(AiError::Conflict);
        }
        Ok(Self {
            provider_call_id: call.provider_call_id.clone(),
            provider_tool_id: AiToolId::parse(call.provider_facing_tool_id().to_owned())?,
            provider_tool_fingerprint: call.provider_facing_tool_fingerprint().to_owned(),
            tool_id: AiToolId::parse(call.tool_id.clone())?,
            tool_fingerprint: call.tool_fingerprint.clone(),
            arguments,
        })
    }

    /// Whether the provider-facing and resolved identities differ.
    pub(crate) fn is_brokered(&self) -> bool {
        self.provider_tool_id != self.tool_id
            || self.provider_tool_fingerprint != self.tool_fingerprint
    }

    /// Durable provider-facing columns: set only for a brokered call, so a
    /// direct row is unchanged.
    pub(crate) fn provider_columns(&self) -> (Option<String>, Option<String>) {
        if self.is_brokered() {
            (
                Some(self.provider_tool_id.as_str().to_owned()),
                Some(self.provider_tool_fingerprint.clone()),
            )
        } else {
            (None, None)
        }
    }

    /// Proves this resolution answers exactly `call`.
    pub(crate) fn bind_provider_call(self, call: &AiProviderToolCall) -> Result<Self, AiError> {
        let direct = !self.is_brokered();
        if self.provider_call_id != call.call_id()
            || &self.provider_tool_id != call.tool_id()
            || self.provider_tool_fingerprint != call.tool_fingerprint()
            || (direct && &self.arguments != call.arguments())
            || (!direct
                && crate::AiCapabilityBrokerOperation::from_tool_id(call.tool_id())
                    != Some(crate::AiCapabilityBrokerOperation::Execute))
        {
            return Err(AiError::Conflict);
        }
        Ok(self)
    }

    /// The reviewed classification of a resolved generated mutation, from
    /// the registered catalogue: an approval-required mutation is a one-shot
    /// supervised write and an automatic one an approval-free autonomous
    /// write. Anything else, or a drifted fingerprint, has none.
    pub(crate) fn mutation_binding(
        &self,
        catalog: &crate::AiToolCatalog,
    ) -> Option<(ToolMaturity, AiApprovalRule)> {
        let capability = catalog.mutation_capability(&self.tool_id)?;
        if capability.fingerprint() != self.tool_fingerprint {
            return None;
        }
        match capability.execution_policy() {
            graphql_orm::graphql::orm::AiMutationExecutionPolicy::Automatic => {
                Some((ToolMaturity::AutonomousWrite, AiApprovalRule::None))
            }
            graphql_orm::graphql::orm::AiMutationExecutionPolicy::ApprovalRequired => {
                Some((ToolMaturity::SupervisedWrite, AiApprovalRule::OneShot))
            }
            _ => None,
        }
    }
}

/// A broker-routed row stores both provider-facing columns, and its
/// provider-facing identity is the broker execute definition. A direct row
/// stores neither.
pub(crate) fn valid_provider_identity(call: &AiToolCallRecord) -> bool {
    match (&call.provider_tool_id, &call.provider_tool_fingerprint) {
        (None, None) => true,
        (Some(tool_id), Some(fingerprint)) => {
            tool_id == crate::AI_CAPABILITY_EXECUTE_TOOL_ID
                && valid_sha256(fingerprint)
                && (tool_id != &call.tool_id || fingerprint != &call.tool_fingerprint)
        }
        _ => false,
    }
}

/// Validates that one broker call names the exact frozen definition this
/// turn offered, and returns its operation.
pub(crate) fn offered_broker_operation(
    provider_result: &AiProviderCallResult,
    provider_call: &AiProviderToolCall,
    delivery: &crate::AiCapabilityDeliveryTurn,
) -> Result<crate::AiCapabilityBrokerOperation, AiError> {
    let operation = crate::AiCapabilityBrokerOperation::from_tool_id(provider_call.tool_id())
        .ok_or(AiError::Forbidden)?;
    let expected = delivery
        .broker_definition(operation)
        .ok_or(AiError::Forbidden)?;
    let offered = provider_result
        .request_snapshot()
        .tools
        .iter()
        .filter(|definition| definition.tool_id == operation.tool_id())
        .collect::<Vec<_>>();
    if offered.len() != 1
        || *offered[0] != expected
        || provider_call.tool_fingerprint() != expected.fingerprint
    {
        return Err(AiError::Forbidden);
    }
    Ok(operation)
}

impl OrmAiApplicationToolCallService {
    /// Resolves one fixed-broker execute call that addresses a loaded
    /// generated mutation, without executing anything.
    ///
    /// Returns `None` when the call is not a broker execute, the session does
    /// not broker mutations, or the loaded reference is not a mutation: the
    /// ordinary broker path then handles the call. A resolution proves the
    /// exact offered broker definition, the loaded binding, the current
    /// index, host capability policy and the session's bound rules, and that
    /// the registered mutation still carries the loaded fingerprint. It is
    /// not authorization: the caller classifies and rule-checks the resolved
    /// mutation, and ordinary approval and resolver authorization still run.
    ///
    /// # Errors
    ///
    /// Fails closed for a substituted broker definition, a stale or foreign
    /// loaded reference, drift, revocation, or a drifted registration.
    pub(crate) async fn resolve_brokered_mutation(
        &self,
        lease: &AiRunLease,
        provider_result: &AiProviderCallResult,
        index: usize,
        delivery: &crate::AiCapabilityDeliveryTurn,
    ) -> Result<Option<ResolvedApplicationCall>, AiError> {
        let call = provider_result
            .tool_calls()
            .get(index)
            .ok_or(AiError::Conflict)?;
        let session = delivery.session();
        if crate::AiCapabilityBrokerOperation::from_tool_id(call.tool_id())
            != Some(crate::AiCapabilityBrokerOperation::Execute)
            || !session.brokers_mutations()
            || !session.holds_loaded_mutation(call.arguments())
        {
            return Ok(None);
        }
        offered_broker_operation(provider_result, call, delivery)?;
        let run = crate::AiCapabilityRunBinding::from_lease(
            lease,
            provider_result.provider_kind().clone(),
            delivery.session_binding(),
        );
        let execution = Box::pin(delivery.broker().authorize_broker_execution(
            lease.principal_reference(),
            &run,
            session,
            call.arguments(),
        ))
        .await?;
        if !execution.is_consequential() {
            return Err(AiError::Conflict);
        }
        let capability = self
            .runtime
            .tool_catalog()
            .mutation_capability(execution.capability_id())
            .ok_or(AiError::Forbidden)?;
        if capability.fingerprint() != execution.capability_fingerprint() {
            return Err(AiError::Forbidden);
        }
        let tool_id = execution.capability_id().clone();
        let tool_fingerprint = execution.capability_fingerprint().to_owned();
        Ok(Some(ResolvedApplicationCall {
            provider_call_id: call.call_id().to_owned(),
            provider_tool_id: call.tool_id().clone(),
            provider_tool_fingerprint: call.tool_fingerprint().to_owned(),
            tool_id,
            tool_fingerprint,
            arguments: execution.into_plan(),
        }))
    }
}

/// Whether one durable row records exactly the provider's requested call,
/// by provider call ID and provider-facing identity.
///
/// A direct row's arguments are the provider's own, so their hash must
/// match. A broker-routed row stores the resolved closed plan instead; the
/// caller proves its protected arguments against the row's own hash with
/// [`resolved_arguments_match`].
pub(crate) fn row_records_call(
    call: &AiToolCallRecord,
    requested: &AiProviderToolCall,
) -> Result<bool, AiError> {
    row_records_parts(
        call,
        requested.call_id(),
        requested.tool_id().as_str(),
        requested.tool_fingerprint(),
        requested.arguments(),
    )
}

/// [`row_records_call`] for a call retained as its parts.
pub(crate) fn row_records_parts(
    call: &AiToolCallRecord,
    call_id: &str,
    tool_id: &str,
    tool_fingerprint: &str,
    arguments: &serde_json::Value,
) -> Result<bool, AiError> {
    Ok(valid_provider_identity(call)
        && call.provider_call_id == call_id
        && call.provider_facing_tool_id() == tool_id
        && call.provider_facing_tool_fingerprint() == tool_fingerprint
        && (call.provider_tool_id.is_some()
            || call.argument_hash == canonical_json_hash(arguments)?))
}

/// Whether a row's opened protected arguments are the ones it must hold:
/// the provider's own for a direct row, or, for a broker-routed row, the
/// stored resolved plan whose canonical hash is the row's argument hash.
pub(crate) fn resolved_arguments_match(
    call: &AiToolCallRecord,
    provider_arguments: &serde_json::Value,
    retained: &serde_json::Value,
) -> Result<bool, AiError> {
    if call.provider_tool_id.is_some() {
        Ok(canonical_json_hash(retained)? == call.argument_hash)
    } else {
        Ok(retained == provider_arguments)
    }
}
