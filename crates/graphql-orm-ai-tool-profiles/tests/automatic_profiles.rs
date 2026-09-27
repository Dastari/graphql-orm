use graphql_orm_ai_tool_profiles::{
    AiApprovalRule, AiDisclosureRule, AiDisclosureSchema, AiDisclosureShape, AiGraphqlArgumentPlan,
    AiGraphqlArgumentValue, AiGraphqlProfileInput, AiGraphqlSelection, AiGraphqlToolManifest,
    AiGraphqlToolManifestBuilder, AiGraphqlToolProfile, AiToolOperationKind, AiToolRisk,
    DataClassification, GraphqlExecutionTargetId, ToolMaturity,
};
use serde_json::json;

const SDL: &str = r#"
    schema { query: Query, mutation: Mutation }
    type Query { record: Record! }
    type Mutation { changeRecord(value: String!): Record! }
    type Record { id: ID! }
"#;

fn profile(
    risk: AiToolRisk,
) -> Result<AiGraphqlToolProfile, graphql_orm_ai_tool_profiles::AiError> {
    let rule = AiDisclosureRule::exportable(DataClassification::Internal);
    let disclosure = AiDisclosureSchema::new(
        "change-result-v1",
        AiDisclosureShape::object(
            rule,
            [(
                "changeRecord".to_owned(),
                AiDisclosureShape::object(
                    rule,
                    [("id".to_owned(), AiDisclosureShape::scalar(rule))],
                ),
            )],
        ),
    )
    .unwrap();
    Ok(AiGraphqlToolProfile::automatic_mutation(
        "reviewed-change",
        "changeRecord",
        "Perform one reviewed record change",
        vec![AiGraphqlSelection::scalar("id")],
        disclosure,
        4096,
        1,
        risk,
        false,
    )?
    .with_inputs([AiGraphqlProfileInput::string(
        "Value",
        "Reviewed value",
        true,
        1,
        128,
    )])
    .with_arguments([AiGraphqlArgumentPlan::new(
        "value",
        AiGraphqlArgumentValue::input("Value"),
    )]))
}

fn compile(
    profile: AiGraphqlToolProfile,
) -> Result<AiGraphqlToolManifest, graphql_orm_ai_tool_profiles::AiError> {
    let mut builder = AiGraphqlToolManifestBuilder::new(
        "records-service",
        GraphqlExecutionTargetId::parse("application-graph").unwrap(),
        SDL,
    )?;
    builder.add_custom_profile(profile)?;
    builder.build()
}

#[test]
fn automatic_profile_roundtrips_exact_closed_mutation_contract() {
    let manifest = compile(profile(AiToolRisk::NonIdempotentWrite).unwrap()).unwrap();
    let descriptor = &manifest.entries[0].descriptor;
    assert_eq!(descriptor.operation_kind, AiToolOperationKind::Mutation);
    assert_eq!(descriptor.maturity, ToolMaturity::AutonomousWrite);
    assert_eq!(descriptor.approval, AiApprovalRule::None);
    assert!(!descriptor.idempotent);
    let document = &descriptor.document;
    assert!(document.contains("changeRecord(value: $Value)"));
    let decoded =
        AiGraphqlToolManifest::from_extension_payload(manifest.extension_payload().unwrap())
            .unwrap();
    decoded.validate_against_finished_schema(SDL).unwrap();
    assert_eq!(decoded, manifest);
}

#[test]
fn automatic_profiles_reject_high_impact_secret_and_non_write_risks() {
    for risk in [
        AiToolRisk::ReadOnly,
        AiToolRisk::Proposal,
        AiToolRisk::HighImpact,
        AiToolRisk::Secret,
    ] {
        assert!(profile(risk).is_err(), "{risk:?}");
    }
}

#[test]
fn deserialization_cannot_widen_automatic_profile_admission() {
    let original = serde_json::to_value(profile(AiToolRisk::NonIdempotentWrite).unwrap()).unwrap();
    for (field, value) in [
        ("root_type", json!("query")),
        ("risk", json!("high_impact")),
        ("approval", json!("one_shot")),
    ] {
        let mut altered = original.clone();
        altered[field] = value;
        let decoded = serde_json::from_value(altered).unwrap();
        assert!(compile(decoded).is_err(), "tampered {field}");
    }
}
