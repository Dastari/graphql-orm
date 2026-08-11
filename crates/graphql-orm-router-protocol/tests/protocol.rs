use std::{fs, path::PathBuf};

use graphql_orm_router_protocol::{
    AdvertisedEndpoint, ArgumentDescriptor, AuthorizationRequirement, CapabilitySet,
    DescriptorExtension, DescriptorFingerprints, Fingerprint, GraphqlEndpoints,
    OperationDescriptor, ProtocolErrorKind, ProtocolVersion, RootOperationType,
    SchemaAdvertisement, ScopeSet, ScopeTemplate, SubgraphDescriptor, SubgraphDescriptorBuilder,
    SubgraphId, SubgraphIdentity, SubgraphName, UnrepresentablePolicy, UnrepresentablePolicyCode,
};

fn endpoint(value: &str) -> AdvertisedEndpoint {
    AdvertisedEndpoint::try_from(value.to_string()).unwrap()
}

fn scope(value: &str) -> ScopeTemplate {
    ScopeTemplate::parse(value).unwrap()
}

fn descriptor() -> SubgraphDescriptor {
    let mut descriptor = SubgraphDescriptor {
        protocol_version: ProtocolVersion { major: 1, minor: 0 },
        subgraph: SubgraphIdentity {
            id: SubgraphId::try_from("inventory-service".to_string()).unwrap(),
            name: SubgraphName::try_from("Inventory".to_string()).unwrap(),
        },
        graphql: GraphqlEndpoints {
            http: endpoint("http://inventory.internal/graphql"),
            websocket: Some(endpoint("ws://inventory.internal/graphql")),
        },
        schema: SchemaAdvertisement {
            url: endpoint("http://inventory.internal/.well-known/sdl"),
        },
        capabilities: CapabilitySet {
            subscriptions: true,
            authorization_metadata: true,
            schema_fingerprints: true,
        },
        required_semantics: vec![
            "scopeTemplates".to_string(),
            "authorizationMetadata".to_string(),
        ],
        operations: vec![
            OperationDescriptor {
                root_type: RootOperationType::Subscription,
                field_name: "stockChanged".to_string(),
                arguments: vec![ArgumentDescriptor {
                    name: "sku".to_string(),
                    graphql_type: "ID!".to_string(),
                    required: true,
                }],
                authorization: AuthorizationRequirement::AnyScopes {
                    alternatives: vec![
                        ScopeSet {
                            scopes: vec![scope("inventory.read"), scope("sku.{sku}.read")],
                        },
                        ScopeSet {
                            scopes: vec![scope("global.admin")],
                        },
                    ],
                },
            },
            OperationDescriptor {
                root_type: RootOperationType::Query,
                field_name: "stock".to_string(),
                arguments: vec![ArgumentDescriptor {
                    name: "sku".to_string(),
                    graphql_type: "ID!".to_string(),
                    required: true,
                }],
                authorization: AuthorizationRequirement::AllScopes {
                    scopes: vec![scope("sku.{sku}.read")],
                },
            },
        ],
        extensions: Vec::new(),
        fingerprints: DescriptorFingerprints {
            schema: Fingerprint::sha256("inventory SDL v1"),
            authorization: Fingerprint::sha256("placeholder"),
            combined: Fingerprint::sha256("placeholder"),
        },
    };
    descriptor.fingerprints.authorization = descriptor.authorization_fingerprint();
    descriptor.fingerprints.combined = descriptor.combined_fingerprint();
    descriptor
}

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    fs::read_to_string(path).unwrap()
}

#[test]
fn generated_style_descriptor_matches_golden_json_and_round_trips() {
    let descriptor = descriptor();
    let json = serde_json::to_string_pretty(&descriptor).unwrap();
    assert_eq!(json, fixture("generated_descriptor.json").trim_end());
    assert_eq!(
        SubgraphDescriptor::from_json_compatible(&json).unwrap(),
        descriptor
    );
}

#[test]
fn framework_neutral_builder_constructs_a_valid_host_payload() {
    let built = SubgraphDescriptorBuilder::new(
        "inventory-service",
        "Inventory",
        "http://inventory.internal/graphql",
        "http://inventory.internal/.well-known/sdl",
        Fingerprint::sha256("inventory SDL v1"),
    )
    .unwrap()
    .websocket("ws://inventory.internal/graphql")
    .unwrap()
    .capabilities(CapabilitySet {
        subscriptions: true,
        authorization_metadata: true,
        schema_fingerprints: true,
    })
    .require_semantic("authorizationMetadata")
    .operation(OperationDescriptor {
        root_type: RootOperationType::Query,
        field_name: "stock".to_owned(),
        arguments: Vec::new(),
        authorization: AuthorizationRequirement::Authenticated,
    })
    .build()
    .unwrap();

    assert_eq!(
        built.protocol_version,
        ProtocolVersion { major: 1, minor: 0 }
    );
    assert_eq!(built.subgraph.id.as_str(), "inventory-service");
    assert!(built.validate_compatible().is_ok());
    assert!(
        serde_json::to_string(&built)
            .unwrap()
            .contains("protocolVersion")
    );
}

#[test]
fn handwritten_descriptor_round_trips() {
    let json = fixture("handwritten_descriptor.json");
    let descriptor = SubgraphDescriptor::from_json_compatible(&json).unwrap();
    assert_eq!(descriptor.subgraph.name.as_str(), "Notes");
    assert!(matches!(
        descriptor.operations[1].authorization,
        AuthorizationRequirement::SubgraphOnly {
            policy: UnrepresentablePolicy {
                code: UnrepresentablePolicyCode::Custom,
                ..
            }
        }
    ));
    assert_eq!(
        serde_json::from_str::<SubgraphDescriptor>(&json).unwrap(),
        descriptor
    );
}

#[test]
fn additive_fields_and_later_minors_are_compatible() {
    let mut value = serde_json::to_value(descriptor()).unwrap();
    value["protocolVersion"]["minor"] = serde_json::json!(9);
    value["futureMetadata"] = serde_json::json!({ "anAdditiveField": true });
    value["graphql"]["futureTransport"] = serde_json::json!("quic");
    let decoded = SubgraphDescriptor::from_json_compatible(&value.to_string()).unwrap();
    assert_eq!(decoded.protocol_version.minor, 9);
}

#[test]
fn optional_extensions_are_canonical_fingerprinted_and_fail_closed_on_drift() {
    let extension = DescriptorExtension::new(
        "example.tool-manifest",
        1,
        serde_json::json!({ "z": 2, "a": { "enabled": true } }),
    )
    .unwrap();
    let built = SubgraphDescriptorBuilder::new(
        "extension-service",
        "Extension",
        "http://extension.internal/graphql",
        "http://extension.internal/sdl",
        Fingerprint::sha256("extension SDL"),
    )
    .unwrap()
    .extension(extension.clone())
    .build()
    .unwrap();
    assert_eq!(built.extensions, vec![extension]);
    assert!(built.validate_compatible().is_ok());

    let mut drifted = built;
    drifted.extensions[0].payload["a"]["enabled"] = serde_json::json!(false);
    assert_eq!(
        drifted.validate_compatible().unwrap_err().kind(),
        ProtocolErrorKind::FingerprintMismatch
    );
}

#[test]
fn extension_order_and_json_object_order_do_not_change_combined_fingerprints() {
    let first = DescriptorExtension::new(
        "example.alpha",
        1,
        serde_json::json!({ "second": 2, "first": 1 }),
    )
    .unwrap();
    let second = DescriptorExtension::new(
        "example.beta",
        2,
        serde_json::json!({ "nested": { "z": false, "a": true } }),
    )
    .unwrap();
    let mut original = descriptor();
    original.extensions = vec![first.clone(), second.clone()];
    original.fingerprints.combined = original.combined_fingerprint();
    let mut reversed = descriptor();
    reversed.extensions = vec![second, first];
    reversed.fingerprints.combined = reversed.combined_fingerprint();
    assert_eq!(
        original.combined_fingerprint(),
        reversed.combined_fingerprint()
    );
    assert!(original.validate_compatible().is_ok());
    assert!(reversed.validate_compatible().is_ok());
}

#[test]
fn incompatible_major_and_unknown_required_semantics_fail_with_stable_categories() {
    let mut major = serde_json::to_value(descriptor()).unwrap();
    major["protocolVersion"]["major"] = serde_json::json!(2);
    let error = SubgraphDescriptor::from_json_compatible(&major.to_string()).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::IncompatibleMajorVersion);
    assert_eq!(error.kind().code(), "INCOMPATIBLE_MAJOR_VERSION");

    let mut semantic = serde_json::to_value(descriptor()).unwrap();
    semantic["requiredSemantics"] = serde_json::json!(["futureRequiredAuthorizationModel"]);
    let error = SubgraphDescriptor::from_json_compatible(&semantic.to_string()).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::UnknownRequiredSemantics);
    assert_eq!(error.kind().code(), "UNKNOWN_REQUIRED_SEMANTICS");
}

#[test]
fn malformed_values_and_unknown_template_arguments_fail_with_stable_categories() {
    let error = ScopeTemplate::parse("scope.{not-valid}").unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::InvalidScopeTemplate);

    let mut malformed = serde_json::to_value(descriptor()).unwrap();
    malformed["graphql"]["http"] = serde_json::json!(" ");
    let error = SubgraphDescriptor::from_json_compatible(&malformed.to_string()).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::MalformedPayload);

    let mut unknown_argument = descriptor();
    unknown_argument.operations[0].authorization = AuthorizationRequirement::AllScopes {
        scopes: vec![scope("sku.{unknown}.read")],
    };
    unknown_argument.fingerprints.authorization = unknown_argument.authorization_fingerprint();
    unknown_argument.fingerprints.combined = unknown_argument.combined_fingerprint();
    let error = unknown_argument.validate_compatible().unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::UnknownTemplateArgument);
}

#[test]
fn canonical_fingerprints_are_invariant_under_logically_unordered_inputs() {
    let original = descriptor();
    let mut permuted = original.clone();
    permuted.operations.reverse();
    permuted.required_semantics.reverse();
    if let AuthorizationRequirement::AnyScopes { alternatives } =
        &mut permuted.operations[0].authorization
    {
        alternatives.reverse();
        alternatives[0].scopes.reverse();
    }
    assert_eq!(
        original.authorization_fingerprint(),
        permuted.authorization_fingerprint()
    );
    assert_eq!(
        original.combined_fingerprint(),
        permuted.combined_fingerprint()
    );

    for operation_order in [false, true] {
        for scope_order in [false, true] {
            let mut candidate = original.clone();
            if operation_order {
                candidate.operations.reverse();
            }
            if scope_order {
                let alternatives = candidate
                    .operations
                    .iter_mut()
                    .find_map(|operation| match &mut operation.authorization {
                        AuthorizationRequirement::AnyScopes { alternatives } => Some(alternatives),
                        _ => None,
                    })
                    .expect("fixture has any-scopes authorization");
                alternatives.reverse();
            }
            assert_eq!(
                original.combined_fingerprint(),
                candidate.combined_fingerprint()
            );
        }
    }
}

#[test]
fn authorization_fingerprint_covers_templated_scope_argument_declarations() {
    let original = descriptor();
    let mut type_changed = original.clone();
    type_changed.operations[0].arguments[0].graphql_type = "String!".to_string();
    let mut requirement_changed = original.clone();
    requirement_changed.operations[0].arguments[0].required = false;

    assert_ne!(
        original.authorization_fingerprint(),
        type_changed.authorization_fingerprint(),
        "argument coercion types affect the value interpolated into a scope template"
    );
    assert_ne!(
        original.authorization_fingerprint(),
        requirement_changed.authorization_fingerprint(),
        "required arguments affect whether a templated scope can be evaluated"
    );

    let mut unrelated_argument = original.clone();
    unrelated_argument.operations[0]
        .arguments
        .push(ArgumentDescriptor {
            name: "locale".to_string(),
            graphql_type: "String".to_string(),
            required: false,
        });
    assert_eq!(
        original.authorization_fingerprint(),
        unrelated_argument.authorization_fingerprint(),
        "arguments not referenced by policy are schema metadata"
    );
    assert_ne!(
        original.combined_fingerprint(),
        unrelated_argument.combined_fingerprint(),
        "the combined fingerprint still detects unrelated argument drift"
    );
}

#[test]
fn canonical_fingerprints_ignore_permutations_and_repeated_logical_members() {
    let original = descriptor();
    let expected_authorization = original.authorization_fingerprint();
    let expected_combined = original.combined_fingerprint();

    for reverse_operations in [false, true] {
        for reverse_semantics in [false, true] {
            for reverse_arguments in [false, true] {
                for reverse_scopes in [false, true] {
                    let mut candidate = original.clone();
                    if reverse_operations {
                        candidate.operations.reverse();
                    }
                    if reverse_semantics {
                        candidate.required_semantics.reverse();
                    }
                    if reverse_arguments {
                        for operation in &mut candidate.operations {
                            operation.arguments.reverse();
                        }
                    }
                    if reverse_scopes {
                        let alternatives = candidate
                            .operations
                            .iter_mut()
                            .find_map(|operation| match &mut operation.authorization {
                                AuthorizationRequirement::AnyScopes { alternatives } => {
                                    Some(alternatives)
                                }
                                _ => None,
                            })
                            .expect("fixture has any-scopes authorization");
                        alternatives.reverse();
                        for alternative in alternatives {
                            alternative.scopes.reverse();
                        }
                    }

                    assert_eq!(
                        expected_authorization,
                        candidate.authorization_fingerprint()
                    );
                    assert_eq!(expected_combined, candidate.combined_fingerprint());
                }
            }
        }
    }

    let mut repeated = original.clone();
    repeated
        .required_semantics
        .push("scopeTemplates".to_string());
    let authorization = repeated
        .operations
        .iter_mut()
        .find_map(|operation| match &mut operation.authorization {
            AuthorizationRequirement::AnyScopes { alternatives } => Some(alternatives),
            _ => None,
        })
        .expect("fixture has any-scopes authorization");
    authorization[0].scopes.push(scope("inventory.read"));
    authorization.push(authorization[0].clone());

    assert_eq!(expected_authorization, repeated.authorization_fingerprint());
    assert_eq!(expected_combined, repeated.combined_fingerprint());
}

#[test]
fn duplicate_operations_and_arguments_fail_before_fingerprint_comparison() {
    let mut duplicate_operation = descriptor();
    duplicate_operation
        .operations
        .push(duplicate_operation.operations[0].clone());
    let error = duplicate_operation.validate_compatible().unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::DuplicateOperation);

    let mut duplicate_argument = descriptor();
    let first_argument = duplicate_argument.operations[0].arguments[0].clone();
    duplicate_argument.operations[0]
        .arguments
        .push(first_argument);
    let error = duplicate_argument.validate_compatible().unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::InvalidDescriptor);
}

#[test]
fn malformed_fingerprints_and_derived_fingerprint_mismatches_have_stable_categories() {
    let mut malformed = serde_json::to_value(descriptor()).unwrap();
    malformed["fingerprints"]["schema"] = serde_json::json!("sha256:UPPERCASE");
    let error = SubgraphDescriptor::from_json_compatible(&malformed.to_string()).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::MalformedPayload);

    let mut mismatched = descriptor();
    mismatched.fingerprints.authorization = Fingerprint::sha256("not the descriptor metadata");
    let error = mismatched.validate_compatible().unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::FingerprintMismatch);
}

#[test]
fn malformed_protocol_shapes_have_stable_categories() {
    let missing_required_field = r#"{
        \"protocolVersion\": { \"major\": 1, \"minor\": 0 },
        \"subgraph\": { \"id\": \"inventory-service\", \"name\": \"Inventory\" }
    }"#;
    let error = SubgraphDescriptor::from_json_compatible(missing_required_field).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::MalformedPayload);

    let mut invalid_scope = serde_json::to_value(descriptor()).unwrap();
    invalid_scope["operations"][0]["authorization"] = serde_json::json!({
        "kind": "allScopes",
        "scopes": ["scope.{unterminated"]
    });
    let error = SubgraphDescriptor::from_json_compatible(&invalid_scope.to_string()).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::MalformedPayload);
}

#[test]
fn explicit_subgraph_only_policy_is_valid_without_router_permission() {
    let mut descriptor = descriptor();
    descriptor.operations.push(OperationDescriptor {
        root_type: RootOperationType::Mutation,
        field_name: "reconcile".to_string(),
        arguments: Vec::new(),
        authorization: AuthorizationRequirement::SubgraphOnly {
            policy: UnrepresentablePolicy {
                code: UnrepresentablePolicyCode::Dynamic,
                detail: "depends on current warehouse assignment".to_string(),
            },
        },
    });
    descriptor.fingerprints.authorization = descriptor.authorization_fingerprint();
    descriptor.fingerprints.combined = descriptor.combined_fingerprint();
    descriptor.validate_compatible().unwrap();
}
