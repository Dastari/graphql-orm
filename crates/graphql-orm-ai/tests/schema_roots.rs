//! Exercises the public AI roots from a host schema.

#![cfg(feature = "sqlite")]

use graphql_orm::prelude::*;
use graphql_orm_ai::*;

#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql_entity(backend = "sqlite", table = "notes", plural = "Notes")]
pub struct Note {
    #[primary_key]
    #[graphql_orm(auto_generated = false)]
    #[sortable]
    pub id: String,
}

mod full {
    use super::*;

    schema_roots! {
        backend: "sqlite",
        schema_policy: "managed",
        entities: [Note],
        extra_query_types: [
            AiQueryRoot,
            AiApprovalQueryRoot,
            AiAttachmentQueryRoot,
            AiConfigurationQueryRoot,
            AiProposalQueryRoot,
            AiRuleQueryRoot,
            AiSkillQueryRoot,
        ],
        extra_mutation_types: [
            AiMutationRoot,
            AiApprovalMutationRoot,
            AiAttachmentMutationRoot,
            AiConfigurationMutationRoot,
            AiProposalMutationRoot,
            AiRuleMutationRoot,
            AiSkillMutationRoot,
        ],
        extra_subscription_types: [AiSubscriptionRoot],
    }
}

mod usage_only {
    use super::*;

    schema_roots! {
        backend: "sqlite",
        schema_policy: "managed",
        entities: [Note],
        extra_query_types: [AiUsageQueryRoot],
    }
}

#[tokio::test]
async fn ai_roots_compose_with_an_ordinary_entity() {
    let db = Database::connect_sqlite("sqlite::memory:").await.unwrap();
    let full = full::schema_builder(db.clone()).finish().sdl();
    let usage_only = usage_only::schema_builder(db).finish().sdl();
    assert!(full.contains("type Note {"));

    #[cfg(not(graphql_case_pascal))]
    let (query, mutation, subscription, usage) = (
        "aiSessions",
        "createAiSession",
        "aiSessionEvents",
        "aiUsage",
    );
    #[cfg(graphql_case_pascal)]
    let (query, mutation, subscription, usage) = (
        "AiSessions",
        "CreateAiSession",
        "AiSessionEvents",
        "AiUsage",
    );

    for field in [query, mutation, subscription, usage] {
        assert!(full.contains(&format!("\t{field}(")), "{field}");
    }
    assert!(usage_only.contains(&format!("\t{usage}(")));
}
