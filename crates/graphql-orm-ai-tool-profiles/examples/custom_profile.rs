//! Compile a finished-schema-validated custom GraphQL tool profile.

use graphql_orm_ai_tool_profiles::{
    AiDisclosureRule, AiDisclosureSchema, AiDisclosureShape, AiGraphqlArgumentPlan,
    AiGraphqlArgumentValue, AiGraphqlProfileInput, AiGraphqlSelection,
    AiGraphqlToolManifestBuilder, AiGraphqlToolProfile, DataClassification,
    GraphqlExecutionTargetId,
};

const FINISHED_SDL: &str = r#"
    type Query { account(id: ID!): Account! }
    type Account { id: ID!, label: String! }
"#;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let disclosure = AiDisclosureSchema::new(
        "v1",
        AiDisclosureShape::object(
            AiDisclosureRule::exportable(DataClassification::Internal),
            [(
                "account".to_owned(),
                AiDisclosureShape::object(
                    AiDisclosureRule::exportable(DataClassification::Internal),
                    [
                        (
                            "id".to_owned(),
                            AiDisclosureShape::scalar(AiDisclosureRule::exportable(
                                DataClassification::Internal,
                            )),
                        ),
                        (
                            "label".to_owned(),
                            AiDisclosureShape::scalar(AiDisclosureRule::exportable(
                                DataClassification::Internal,
                            )),
                        ),
                    ],
                ),
            )],
        ),
    )?;
    let profile = AiGraphqlToolProfile::read_only(
        "account-summary",
        "account",
        "Return the approved account identifier and label.",
        vec![
            AiGraphqlSelection::scalar("id"),
            AiGraphqlSelection::scalar("label"),
        ],
        disclosure,
        16 * 1024,
        1,
    )
    .with_inputs([AiGraphqlProfileInput::string(
        "account_id",
        "Approved account identifier.",
        true,
        1,
        128,
    )])
    .with_arguments([AiGraphqlArgumentPlan::new(
        "id",
        AiGraphqlArgumentValue::input("account_id"),
    )]);

    let target = GraphqlExecutionTargetId::parse("accounts-api")?;
    let mut builder = AiGraphqlToolManifestBuilder::new("accounts", target, FINISHED_SDL)?;
    builder.add_custom_profile(profile)?;
    let manifest = builder.build()?;
    let payload = manifest.extension_payload()?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn finished_schema_profile_compiles_and_serializes() {
        super::run().expect("custom profile example should run");
    }
}
