use super::*;
use crate::{AiDisclosureRule, AiToolDescriptor, AiToolOperationKind};
use std::collections::BTreeMap;

fn fields() -> AiToolImageFieldBindings {
    AiToolImageFieldBindings::new(
        "Reference",
        "Sha256",
        "Mime",
        "Width",
        "Height",
        "CapturedAtMs",
        "RetainedUntilMs",
    )
    .unwrap()
    .with_action_expires_at_ms("ActionExpiresAtMs")
    .unwrap()
}
fn limits() -> AiToolResultMediaLimits {
    AiToolResultMediaLimits::new(1, 4 * 1024 * 1024, 1536, 4_000_000, 300_000, 5_000).unwrap()
}
fn policy() -> AiToolResultMediaPolicy {
    AiToolResultMediaPolicy::new(
        "capture-owner-v1",
        vec![vec!["Observation".into()]],
        fields(),
        limits(),
        DataClassification::Restricted,
    )
    .unwrap()
}
fn shape() -> AiDisclosureShape {
    let rule = AiDisclosureRule::exportable(DataClassification::Restricted);
    AiDisclosureShape::Object {
        rule,
        fields: BTreeMap::from([(
            "Observation".into(),
            AiDisclosureShape::Object {
                rule,
                fields: fields()
                    .names()
                    .into_iter()
                    .map(|name| (name.into(), AiDisclosureShape::Scalar { rule }))
                    .collect(),
            },
        )]),
    }
}
#[test]
fn exact_exportable_metadata_path_is_required_even_after_deserialization() {
    policy().validate_disclosure(&shape()).unwrap();
    for field in fields().names() {
        let mut changed = shape();
        let AiDisclosureShape::Object { fields, .. } = &mut changed else {
            panic!()
        };
        let AiDisclosureShape::Object { fields, .. } = fields.get_mut("Observation").unwrap()
        else {
            panic!()
        };
        fields.remove(field);
        assert!(policy().validate_disclosure(&changed).is_err());
    }
    let mut wire = serde_json::to_value(policy()).unwrap();
    wire["object_paths"] = serde_json::json!([["Other"]]);
    let changed: AiToolResultMediaPolicy = serde_json::from_value(wire).unwrap();
    assert!(changed.validate_disclosure(&shape()).is_err());
}
#[test]
fn never_export_and_secret_ancestors_cannot_supply_media_metadata() {
    for rule in [
        AiDisclosureRule::never_export(DataClassification::Restricted),
        AiDisclosureRule::exportable(DataClassification::Secret),
    ] {
        let mut changed = shape();
        let AiDisclosureShape::Object { rule: root, .. } = &mut changed else {
            panic!()
        };
        *root = rule;
        assert!(policy().validate_disclosure(&changed).is_err());
    }
}
#[test]
fn unsafe_resolvers_duplicate_paths_and_unbounded_limits_are_denied() {
    for resolver in ["https://invalid.test", "/private/image", "../image", ""] {
        assert!(
            AiToolResultMediaPolicy::new(
                resolver,
                vec![vec!["Observation".into()]],
                fields(),
                limits(),
                DataClassification::Restricted
            )
            .is_err()
        );
    }
    assert!(
        AiToolResultMediaPolicy::new(
            "owner",
            vec![vec![], vec![]],
            fields(),
            limits(),
            DataClassification::Restricted
        )
        .is_err()
    );
    assert!(AiToolResultMediaLimits::new(5, 1, 1, 1, 1, 1).is_err());
    assert!(AiToolResultMediaLimits::new(1, 16 * 1024 * 1024 + 1, 1, 1, 1, 1).is_err());
    assert!(fields().with_action_expires_at_ms("Reference").is_err());
}
#[test]
fn static_media_policy_is_bound_to_tool_fingerprint_and_missing_preserves_legacy_bytes() {
    let descriptor = AiToolDescriptor::new(
        "capture.observe",
        "Read one retained observation",
        AiToolOperationKind::Query,
        "query { Observation { Reference } }",
        serde_json::json!({"type":"object"}),
    )
    .unwrap();
    assert!(
        serde_json::to_value(&descriptor)
            .unwrap()
            .get("result_media")
            .is_none()
    );
    let media = descriptor.clone().with_result_media(policy()).unwrap();
    assert_ne!(media.fingerprint, descriptor.fingerprint);
    assert!(media.has_valid_fingerprint());
    let mut forged = serde_json::to_value(&media).unwrap();
    forged["result_media"]["resolver_id"] = serde_json::json!("other-owner");
    let forged: AiToolDescriptor = serde_json::from_value(forged).unwrap();
    assert!(!forged.has_valid_fingerprint());
}
