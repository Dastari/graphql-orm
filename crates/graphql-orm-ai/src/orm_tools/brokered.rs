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

/// Format of the protected evidence a broker-routed tool-call row keeps in
/// its protected arguments.
pub(crate) const BROKERED_CALL_EVIDENCE_FORMAT: &str = "graphql-orm-ai/brokered-call/v1";

/// Protected evidence of one broker-routed call, stored as the row's
/// protected arguments.
///
/// It records the broker origin of the row together with the resolved plan,
/// so the fact that a row was broker-routed lives in the content-protected
/// envelope, not only in the clear provider-facing columns: clearing those
/// columns can never make a broker-routed row pass as a direct one. It also
/// retains the exact offered broker definition fingerprint, the hash of the
/// provider's original broker arguments and the fingerprint of the rules
/// that admitted the mutation. The row's own `argument_hash` stays the hash
/// of the resolved plan.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BrokeredCallEvidence {
    pub(crate) format: String,
    pub(crate) provider_tool_id: String,
    pub(crate) provider_tool_fingerprint: String,
    pub(crate) provider_argument_hash: String,
    pub(crate) rule_fingerprint: String,
    pub(crate) plan: serde_json::Value,
}

fn json_hash(value: &serde_json::Value) -> String {
    graphql_orm_ai_tool_profiles::canonical_json_hash(value)
}

/// One provider call resolved to the exact application capability it runs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedApplicationCall {
    pub(crate) provider_call_id: String,
    pub(crate) provider_tool_id: AiToolId,
    pub(crate) provider_tool_fingerprint: String,
    /// Canonical hash of the provider's own arguments.
    pub(crate) provider_argument_hash: String,
    pub(crate) tool_id: AiToolId,
    pub(crate) tool_fingerprint: String,
    pub(crate) arguments: serde_json::Value,
    /// Fingerprint of the rules that admitted a brokered mutation; `None`
    /// for a direct call.
    pub(crate) rule_fingerprint: Option<String>,
}

impl ResolvedApplicationCall {
    /// The identity of a direct call: both identities are the offered
    /// definition.
    pub(crate) fn direct(call: &AiProviderToolCall) -> Self {
        Self {
            provider_call_id: call.call_id().to_owned(),
            provider_tool_id: call.tool_id().clone(),
            provider_tool_fingerprint: call.tool_fingerprint().to_owned(),
            provider_argument_hash: json_hash(call.arguments()),
            tool_id: call.tool_id().clone(),
            tool_fingerprint: call.tool_fingerprint().to_owned(),
            arguments: call.arguments().clone(),
            rule_fingerprint: None,
        }
    }

    /// Rebuilds the resolved call of one durable tool-call row from its
    /// stored identities and opened protected arguments
    /// ([`open_call_arguments`]).
    ///
    /// A broker-routed row is re-resolved from its stored resolved identity
    /// and protected evidence only; the broker's process-local loaded
    /// binding is never needed again.
    pub(crate) fn from_record(
        call: &AiToolCallRecord,
        opened: serde_json::Value,
    ) -> Result<Self, AiError> {
        let (arguments, evidence) = open_call_arguments(call, opened)?;
        let tool_id = AiToolId::parse(call.tool_id.clone())?;
        Ok(match evidence {
            Some(evidence) => Self {
                provider_call_id: call.provider_call_id.clone(),
                provider_tool_id: AiToolId::parse(evidence.provider_tool_id)?,
                provider_tool_fingerprint: evidence.provider_tool_fingerprint,
                provider_argument_hash: evidence.provider_argument_hash,
                tool_id,
                tool_fingerprint: call.tool_fingerprint.clone(),
                arguments,
                rule_fingerprint: Some(evidence.rule_fingerprint),
            },
            None => Self {
                provider_call_id: call.provider_call_id.clone(),
                provider_tool_id: tool_id.clone(),
                provider_tool_fingerprint: call.tool_fingerprint.clone(),
                provider_argument_hash: call.argument_hash.clone(),
                tool_id,
                tool_fingerprint: call.tool_fingerprint.clone(),
                arguments,
                rule_fingerprint: None,
            },
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

    /// The value the row protects as its arguments: the provider's own for a
    /// direct call, or the [`BrokeredCallEvidence`] envelope around the
    /// resolved plan for a brokered one.
    ///
    /// # Errors
    ///
    /// Fails closed for a brokered call without its admitting rule
    /// fingerprint.
    pub(crate) fn protected_arguments(&self) -> Result<serde_json::Value, AiError> {
        if !self.is_brokered() {
            return Ok(self.arguments.clone());
        }
        let rule_fingerprint = self.rule_fingerprint.clone().ok_or(AiError::Conflict)?;
        serde_json::to_value(BrokeredCallEvidence {
            format: BROKERED_CALL_EVIDENCE_FORMAT.to_owned(),
            provider_tool_id: self.provider_tool_id.as_str().to_owned(),
            provider_tool_fingerprint: self.provider_tool_fingerprint.clone(),
            provider_argument_hash: self.provider_argument_hash.clone(),
            rule_fingerprint,
            plan: self.arguments.clone(),
        })
        .map_err(|_| AiError::PersistenceFailed)
    }

    /// The broker origin bound into the approval of a brokered call; `None`
    /// for a direct call.
    pub(crate) fn approval_broker_origin(&self) -> Option<crate::AiApprovalBrokerOrigin> {
        if !self.is_brokered() {
            return None;
        }
        Some(crate::AiApprovalBrokerOrigin {
            provider_tool_id: self.provider_tool_id.as_str().to_owned(),
            provider_tool_fingerprint: self.provider_tool_fingerprint.clone(),
            provider_argument_hash: self.provider_argument_hash.clone(),
            rule_fingerprint: self.rule_fingerprint.clone().unwrap_or_default(),
        })
    }

    /// Proves this resolution answers exactly `call`: its call ID, offered
    /// definition and fingerprint, and its exact arguments.
    pub(crate) fn bind_provider_call(self, call: &AiProviderToolCall) -> Result<Self, AiError> {
        let direct = !self.is_brokered();
        if self.provider_call_id != call.call_id()
            || &self.provider_tool_id != call.tool_id()
            || self.provider_tool_fingerprint != call.tool_fingerprint()
            || self.provider_argument_hash != json_hash(call.arguments())
            || (direct && &self.arguments != call.arguments())
            || (!direct
                && (crate::AiCapabilityBrokerOperation::from_tool_id(call.tool_id())
                    != Some(crate::AiCapabilityBrokerOperation::Execute)
                    || self.provider_tool_fingerprint
                        != crate::capability_delivery::fixed_broker_execute_fingerprint()
                    || self.rule_fingerprint.is_none()))
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
/// provider-facing identity is the exact fixed-broker execute definition of
/// the current broker version. A direct row stores neither.
pub(crate) fn valid_provider_identity(call: &AiToolCallRecord) -> bool {
    match (&call.provider_tool_id, &call.provider_tool_fingerprint) {
        (None, None) => true,
        (Some(tool_id), Some(fingerprint)) => {
            tool_id == crate::AI_CAPABILITY_EXECUTE_TOOL_ID
                && fingerprint == crate::capability_delivery::fixed_broker_execute_fingerprint()
                && (tool_id != &call.tool_id || fingerprint != &call.tool_fingerprint)
        }
        _ => false,
    }
}

/// Splits one row's opened protected arguments into the arguments it runs
/// with and, for a broker-routed row, its [`BrokeredCallEvidence`].
///
/// Only the row's clear dual-identity columns claim broker origin. A row
/// without them is a direct row, and its protected arguments are taken
/// verbatim, whatever their shape: direct arguments that happen to look like
/// an envelope stay ordinary arguments. A row with them must open to an
/// envelope that agrees with the columns, so columns added to a direct row
/// are refused. Clearing the columns of a broker-routed row leaves an
/// envelope whose hash is not the row's argument hash, and its approval
/// binds the broker origin ([`crate::AiApprovalBinding::broker_origin`]), so
/// such a row is refused rather than run as direct. The arguments' canonical
/// hash must be the row's argument hash in every case.
///
/// # Errors
///
/// Returns [`AiError::Conflict`] for any disagreement.
pub(crate) fn open_call_arguments(
    call: &AiToolCallRecord,
    opened: serde_json::Value,
) -> Result<(serde_json::Value, Option<BrokeredCallEvidence>), AiError> {
    if !valid_provider_identity(call) {
        return Err(AiError::Conflict);
    }
    if call.provider_tool_id.is_none() {
        if json_hash(&opened) != call.argument_hash {
            return Err(AiError::Conflict);
        }
        return Ok((opened, None));
    }
    let evidence: BrokeredCallEvidence =
        serde_json::from_value(opened).map_err(|_| AiError::Conflict)?;
    if evidence.format != BROKERED_CALL_EVIDENCE_FORMAT
        || call.provider_tool_id.as_deref() != Some(evidence.provider_tool_id.as_str())
        || call.provider_tool_fingerprint.as_deref()
            != Some(evidence.provider_tool_fingerprint.as_str())
        || !valid_sha256(&evidence.provider_argument_hash)
        || !valid_sha256(&evidence.rule_fingerprint)
        || json_hash(&evidence.plan) != call.argument_hash
    {
        return Err(AiError::Conflict);
    }
    Ok((evidence.plan.clone(), Some(evidence)))
}

/// Re-proves that current hierarchical rules still admit one brokered
/// mutation: the rules resolved now for this exact lease and scope must be
/// the ones that admitted it, and must admit its own fingerprint at its
/// reviewed classification.
///
/// # Errors
///
/// Returns [`AiError::ReauthorizationFailed`] when the rules changed or no
/// longer admit the mutation, and propagates a resolver failure.
pub(crate) async fn brokered_rules_admit(
    rules: &dyn crate::AiAgentRuleResolver,
    lease: &AiRunLease,
    scope: &AiScope,
    rule_fingerprint: &str,
    tool_fingerprint: &str,
    binding: (ToolMaturity, AiApprovalRule),
) -> Result<(), AiError> {
    let current = rules.resolve_rules(lease, scope).await?;
    if current.rules().target_scope() != scope
        || current.rules().fingerprint() != rule_fingerprint
        || current
            .rules()
            .constrain_tool(tool_fingerprint, binding.0, binding.1)
            != Some(binding.1)
    {
        return Err(AiError::ReauthorizationFailed);
    }
    Ok(())
}

/// The rule fence of one brokered automatic mutation: the resolver and
/// scope its admission is re-proved against at the pre-effect boundary.
#[derive(Clone)]
pub(crate) struct BrokeredRuleFence {
    pub(crate) rules: Arc<dyn crate::AiAgentRuleResolver>,
    pub(crate) scope: AiScope,
}

/// The final admission of one brokered mutation: its rules are re-proved
/// inside the execution bridge, after principal rehydration, host tool
/// policy and request-context construction, as the last step before the
/// executor. The caller's durable lifecycle (the pre-effect checkpoint of
/// an automatic mutation, or the consumed one-shot approval) proves no
/// earlier dispatch, so a refusal is attested as not started.
pub(crate) struct BrokeredRuleAdmission<'a> {
    pub(crate) rules: &'a dyn crate::AiAgentRuleResolver,
    pub(crate) lease: &'a AiRunLease,
    pub(crate) scope: &'a AiScope,
    pub(crate) resolved: &'a ResolvedApplicationCall,
    pub(crate) binding: (ToolMaturity, AiApprovalRule),
}

#[async_trait]
impl crate::execution::AiFinalAdmission for BrokeredRuleAdmission<'_> {
    async fn admits(&self) -> bool {
        let Some(rule_fingerprint) = self.resolved.rule_fingerprint.as_deref() else {
            return false;
        };
        brokered_rules_admit(
            self.rules,
            self.lease,
            self.scope,
            rule_fingerprint,
            &self.resolved.tool_fingerprint,
            self.binding,
        )
        .await
        .is_ok()
    }

    fn attests_no_effect(&self) -> bool {
        true
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
        // The session's rules admitted the mutation; the caller re-proves
        // them against freshly resolved rules.
        let rule_fingerprint = session
            .mutation_rule_fingerprint()
            .ok_or(AiError::Forbidden)?
            .to_owned();
        let tool_id = execution.capability_id().clone();
        let tool_fingerprint = execution.capability_fingerprint().to_owned();
        Ok(Some(ResolvedApplicationCall {
            provider_call_id: call.call_id().to_owned(),
            provider_tool_id: call.tool_id().clone(),
            provider_tool_fingerprint: call.tool_fingerprint().to_owned(),
            provider_argument_hash: json_hash(call.arguments()),
            tool_id,
            tool_fingerprint,
            arguments: execution.into_plan(),
            rule_fingerprint: Some(rule_fingerprint),
        }))
    }
}

/// Whether one durable row records exactly the provider's requested call,
/// by provider call ID and provider-facing identity.
///
/// A direct row's arguments are the provider's own, so their hash must
/// match. A broker-routed row stores the resolved plan and its protected
/// evidence instead; the caller proves the provider's original arguments
/// against that evidence with [`protected_arguments_match`].
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
        && (call.provider_tool_id.is_some() || call.argument_hash == json_hash(arguments)))
}

/// Whether a row's opened protected arguments are exactly the evidence of
/// the provider's requested call: the provider's own arguments for a direct
/// row, or, for a broker-routed row, an envelope that agrees with the row
/// ([`open_call_arguments`]) and retains the requested broker fingerprint
/// and the hash of the provider's original arguments.
pub(crate) fn protected_arguments_match(
    call: &AiToolCallRecord,
    provider_tool_fingerprint: &str,
    provider_arguments: &serde_json::Value,
    retained: &serde_json::Value,
) -> Result<bool, AiError> {
    let Ok((arguments, evidence)) = open_call_arguments(call, retained.clone()) else {
        return Ok(false);
    };
    Ok(match evidence {
        Some(evidence) => {
            evidence.provider_tool_fingerprint == provider_tool_fingerprint
                && evidence.provider_argument_hash == json_hash(provider_arguments)
        }
        None => &arguments == provider_arguments,
    })
}
