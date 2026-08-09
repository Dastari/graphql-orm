#![cfg(any(feature = "sqlite", feature = "postgres"))]

use std::sync::Arc;

use agql_auth::{
    AuthError, CurrentPrincipalResolver, FixedClock, PrincipalReference, ResolvedPrincipal,
};
use async_trait::async_trait;
use graphql_orm_ai::prelude::DeploymentAiCurrentRuleResolver;
use graphql_orm_ai::{
    AiAgentRuleResolver, AiCurrentRuleResolverLimits, AiRuleApprovalRequirement,
    AiRuleBudgetCeilings, AiRuleConstraints, AiRuleDeploymentLimits, ToolMaturity,
};
use time::{Duration, OffsetDateTime};

struct UnavailablePrincipalResolver;

#[async_trait]
impl CurrentPrincipalResolver for UnavailablePrincipalResolver {
    async fn resolve(
        &self,
        _reference: &PrincipalReference,
    ) -> agql_auth::AuthResult<ResolvedPrincipal> {
        Err(AuthError::Forbidden)
    }
}

fn deployment_constraints() -> AiRuleConstraints {
    AiRuleConstraints {
        enabled: true,
        maximum_classification: graphql_orm_ai::DataClassification::Internal,
        maximum_tool_maturity: ToolMaturity::ReadOnly,
        approval_requirement: AiRuleApprovalRequirement::DescriptorPolicy,
        allowed_tool_fingerprints: None,
        allowed_provider_kinds: None,
        allowed_provider_capabilities: None,
        allow_provider_retention: false,
        allow_byok: false,
        budget: AiRuleBudgetCeilings {
            maximum_steps: Some(8),
            maximum_duration_seconds: Some(300),
            maximum_output_tokens: Some(4_096),
            maximum_cost_microunits: Some(1_000_000),
            maximum_provider_calls: Some(4),
            maximum_tool_units: Some(4),
            maximum_image_units: Some(0),
        },
    }
}

#[test]
fn consumer_can_construct_deployment_resolver_through_public_contracts() {
    let now =
        OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed time should validate");
    let resolver: Arc<dyn AiAgentRuleResolver> = Arc::new(DeploymentAiCurrentRuleResolver::new(
        Arc::new(UnavailablePrincipalResolver),
        Arc::new(FixedClock::new(now)),
        AiCurrentRuleResolverLimits::new(Duration::minutes(2))
            .expect("current principal limits should validate"),
        AiRuleDeploymentLimits::new(1, deployment_constraints())
            .expect("deployment limits should validate"),
    ));

    assert_eq!(Arc::strong_count(&resolver), 1);
}

#[test]
fn invalid_deployment_constraints_fail_before_resolver_construction() {
    let mut constraints = deployment_constraints();
    constraints.maximum_tool_maturity = ToolMaturity::AutonomousWrite;

    assert!(AiRuleDeploymentLimits::new(1, constraints).is_err());
}
