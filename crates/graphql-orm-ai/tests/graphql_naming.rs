use async_graphql::{EmptyMutation, EmptySubscription, Schema};
use async_graphql_parser::types::{TypeKind, TypeSystemDefinition};
use graphql_orm::graphql::orm::GraphqlRootFieldNames;
use graphql_orm_ai::{
    AiApprovalMutationRoot, AiApprovalQueryRoot, AiAttachmentMutationRoot, AiAttachmentQueryRoot,
    AiConfigurationMutationRoot, AiConfigurationQueryRoot, AiMutationRoot, AiProposalMutationRoot,
    AiProposalQueryRoot, AiQueryRoot, AiRuleMutationRoot, AiRuleQueryRoot, AiSkillMutationRoot,
    AiSkillQueryRoot, AiSubscriptionRoot, AiUsageQueryRoot,
};

fn assert_sdl_fields<T: GraphqlRootFieldNames>(sdl: &str, type_name: &str) {
    let document = async_graphql_parser::parse_schema(sdl).unwrap();
    let mut actual = document
        .definitions
        .iter()
        .find_map(|definition| {
            let TypeSystemDefinition::Type(definition) = definition else {
                return None;
            };
            let TypeKind::Object(object) = &definition.node.kind else {
                return None;
            };
            (definition.node.name.node.as_str() == type_name).then_some(&object.fields)
        })
        .unwrap_or_else(|| panic!("{type_name} is missing from the SDL"))
        .iter()
        .map(|field| field.node.name.node.as_str())
        .collect::<Vec<_>>();
    actual.sort_unstable();
    let mut declared = T::ROOT_FIELD_NAMES.to_vec();
    declared.sort_unstable();
    assert_eq!(declared, actual, "{type_name}");
}

#[test]
fn declared_root_field_names_equal_sdl() {
    macro_rules! check_pair {
        ($query:ident, $mutation:ident, $subscription:expr) => {{
            let sdl = Schema::build($query, $mutation, $subscription)
                .finish()
                .sdl();
            assert_sdl_fields::<$query>(&sdl, stringify!($query));
            assert_sdl_fields::<$mutation>(&sdl, stringify!($mutation));
            sdl
        }};
    }

    let sdl = check_pair!(AiQueryRoot, AiMutationRoot, AiSubscriptionRoot);
    assert_sdl_fields::<AiSubscriptionRoot>(&sdl, "AiSubscriptionRoot");
    check_pair!(
        AiApprovalQueryRoot,
        AiApprovalMutationRoot,
        EmptySubscription
    );
    check_pair!(
        AiAttachmentQueryRoot,
        AiAttachmentMutationRoot,
        EmptySubscription
    );
    check_pair!(
        AiConfigurationQueryRoot,
        AiConfigurationMutationRoot,
        EmptySubscription
    );
    check_pair!(
        AiProposalQueryRoot,
        AiProposalMutationRoot,
        EmptySubscription
    );
    check_pair!(AiRuleQueryRoot, AiRuleMutationRoot, EmptySubscription);
    check_pair!(AiSkillQueryRoot, AiSkillMutationRoot, EmptySubscription);
    let sdl = Schema::build(AiUsageQueryRoot, EmptyMutation, EmptySubscription)
        .finish()
        .sdl();
    assert_sdl_fields::<AiUsageQueryRoot>(&sdl, "AiUsageQueryRoot");
}

#[test]
fn configured_graphql_case_is_coherent_without_aliases() {
    let sdl = Schema::build(AiQueryRoot, AiMutationRoot, AiSubscriptionRoot)
        .finish()
        .sdl();
    for state in ["Working", "Done", "Prompt", "Error"] {
        assert!(sdl.contains(&format!("\t{state}\n")));
    }
    let configuration_sdl = Schema::build(
        AiConfigurationQueryRoot,
        AiConfigurationMutationRoot,
        EmptySubscription,
    )
    .finish()
    .sdl();
    let skill_sdl = Schema::build(AiSkillQueryRoot, AiSkillMutationRoot, EmptySubscription)
        .finish()
        .sdl();
    let rule_sdl = Schema::build(AiRuleQueryRoot, AiRuleMutationRoot, EmptySubscription)
        .finish()
        .sdl();

    #[cfg(not(graphql_case_pascal))]
    {
        assert!(sdl.contains("aiSessions("));
        assert!(sdl.contains("activitySnapshot: AiSessionActivitySnapshot!"));
        assert!(sdl.contains("inboxWatermark: Int!"));
        assert!(sdl.contains("inputMessageSequence: Int"));
        assert!(sdl.contains("failureDisposition: AiRunDisposition\n"));
        assert!(sdl.contains("aiMessages(sessionId:"));
        assert!(sdl.contains("aiToolCallResultPreview(input:"));
        assert!(sdl.contains("contentPurged: Boolean!"));
        assert!(sdl.contains("createAiSession(input:"));
        assert!(sdl.contains("executionSelection: AiSessionExecutionSelection"));
        assert!(sdl.contains("pinAiSessionExecutionSelection(input:"));
        assert!(sdl.contains("reasoningEffort: ModelReasoningEffort!"));
        assert!(sdl.contains("ULTRA"));
        assert!(sdl.contains("cancelAiRun(input:"));
        assert!(sdl.contains("aiSessionEvents(sessionId:"));
        assert!(sdl.contains("aiInboxEventPage("));
        assert!(sdl.contains("aiInboxEvents("));
        assert!(sdl.contains("aiUsage(scope:"));
        assert!(configuration_sdl.contains("LOCAL_HARNESS"));
        assert!(configuration_sdl.contains("openaiCompatible: AiOpenAiCompatibleProfileInput"));
        assert!(configuration_sdl.contains("providerRetainedContinuation: Boolean!"));
        assert!(configuration_sdl.contains("aiBudgetScopeCapacity(scope:"));
        assert!(configuration_sdl.contains("reclaimAiBudgetReservation(input:"));
        assert!(configuration_sdl.contains("uncertainReservationCount: Int!"));
        assert!(configuration_sdl.contains("reclaimable: Boolean!"));
        assert!(skill_sdl.contains("aiSkills(scope:"));
        assert!(skill_sdl.contains("upsertAiSkill(input:"));
        assert!(skill_sdl.contains("publishAiSkillVersion(input:"));
        assert!(skill_sdl.contains("allowedUiIntents: [AiSkillUiIntentBindingInput!]!"));
        assert!(rule_sdl.contains("aiRulePolicy(scope:"));
        assert!(rule_sdl.contains("setAiRulePolicy(input:"));
        assert!(rule_sdl.contains("allowedProviderCapabilities: [AiRuleProviderCapability!]"));
        assert!(!sdl.contains("AiSessions("));
    }

    #[cfg(graphql_case_pascal)]
    {
        assert!(sdl.contains("AiSessions("));
        assert!(sdl.contains("ActivitySnapshot: AiSessionActivitySnapshot!"));
        assert!(sdl.contains("InboxWatermark: Int!"));
        assert!(sdl.contains("InputMessageSequence: Int"));
        assert!(sdl.contains("FailureDisposition: AiRunDisposition\n"));
        assert!(sdl.contains("AiMessages(SessionId:"));
        assert!(sdl.contains("AiToolCallResultPreview(Input:"));
        assert!(sdl.contains("ContentPurged: Boolean!"));
        assert!(sdl.contains("CreateAiSession(Input:"));
        assert!(sdl.contains("ExecutionSelection: AiSessionExecutionSelection"));
        assert!(sdl.contains("PinAiSessionExecutionSelection(Input:"));
        assert!(sdl.contains("ReasoningEffort: ModelReasoningEffort!"));
        assert!(sdl.contains("Ultra"));
        assert!(sdl.contains("RenameAiSession(Input:"));
        assert!(sdl.contains("CancelAiRun(Input:"));
        assert!(sdl.contains("AiSessionEvents(SessionId:"));
        assert!(sdl.contains("AiInboxEventPage("));
        assert!(sdl.contains("AiInboxEvents("));
        assert!(sdl.contains("AiUsage(Scope:"));
        assert!(configuration_sdl.contains("LocalHarness"));
        assert!(configuration_sdl.contains("OpenaiCompatible: AiOpenAiCompatibleProfileInput"));
        assert!(configuration_sdl.contains("ProviderRetainedContinuation: Boolean!"));
        assert!(configuration_sdl.contains("AiBudgetScopeCapacity(Scope:"));
        assert!(configuration_sdl.contains("ReclaimAiBudgetReservation(Input:"));
        assert!(configuration_sdl.contains("UncertainReservationCount: Int!"));
        assert!(configuration_sdl.contains("Reclaimable: Boolean!"));
        assert!(skill_sdl.contains("AiSkills(Scope:"));
        assert!(skill_sdl.contains("UpsertAiSkill(Input:"));
        assert!(skill_sdl.contains("PublishAiSkillVersion(Input:"));
        assert!(skill_sdl.contains("AllowedUiIntents: [AiSkillUiIntentBindingInput!]!"));
        assert!(rule_sdl.contains("AiRulePolicy(Scope:"));
        assert!(rule_sdl.contains("SetAiRulePolicy(Input:"));
        assert!(rule_sdl.contains("AllowedProviderCapabilities: [AiRuleProviderCapability!]"));
        assert!(!rule_sdl.contains("aiRulePolicy("));
        assert!(!configuration_sdl.contains("LOCAL_HARNESS"));
        assert!(!sdl.contains("aiSessions("));
    }
}
