#[cfg(test)]
use graphql_orm::async_graphql::Schema;
use graphql_orm::async_graphql::SimpleObject;
use graphql_orm::prelude::*;
#[cfg(test)]
use graphql_orm_ai_tool_profiles::{
    AiGraphqlQueryCapabilityCatalog, AiGraphqlQueryCapabilityLimits, GraphqlExecutionTargetId,
};

#[cfg(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql")
))]
compile_error!("Enable exactly one relationship-capability backend feature.");

#[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "mssql")))]
compile_error!("Enable one relationship-capability backend feature.");

/// Parent record with generated one- and many-relationship fields.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex, rename_fields = "PascalCase")]
#[graphql_entity(
    table = "CapabilityParents",
    plural = "CapabilityParents",
    schema_policy = "external_read_only",
    default_sort = "Id ASC",
    auth = "none"
)]
pub struct CapabilityParent {
    /// Stable parent identity.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i32,
    /// Optional profile identity.
    #[filterable(type = "number")]
    pub profile_id: Option<i32>,
    /// Bounded child records.
    #[graphql(skip)]
    #[relation(
        target = "CapabilityChild",
        from = "id",
        to = "ParentId",
        multiple,
        emit_fk = false
    )]
    pub children: Vec<CapabilityChild>,
    /// Optional one-to-one profile.
    #[graphql(skip)]
    #[relation(
        target = "CapabilityProfile",
        from = "profile_id",
        to = "Id",
        emit_fk = false
    )]
    pub profile: Option<CapabilityProfile>,
}

/// Child record selected through a generated relationship.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    table = "CapabilityChildren",
    plural = "CapabilityChildren",
    schema_policy = "external_read_only",
    default_sort = "Id ASC",
    auth = "none"
)]
pub struct CapabilityChild {
    /// Stable child identity.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i32,
    /// Owning parent identity.
    #[filterable(type = "number")]
    #[sortable]
    pub parent_id: i32,
    /// Public child label.
    #[filterable(type = "string")]
    pub label: String,
}

/// Optional one-to-one profile record.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    table = "CapabilityProfiles",
    plural = "CapabilityProfiles",
    schema_policy = "external_read_only",
    default_sort = "Id ASC",
    auth = "none"
)]
pub struct CapabilityProfile {
    /// Stable profile identity.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub id: i32,
    /// Public profile label.
    pub label: String,
}

/// Composite-key parent with a generated to-many relationship.
#[derive(
    GraphQLEntity,
    GraphQLRelations,
    GraphQLOperations,
    SimpleObject,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
)]
#[graphql(complex, rename_fields = "PascalCase")]
#[graphql_entity(
    table = "CompositeCapabilityParents",
    plural = "CompositeCapabilityParents",
    schema_policy = "external_read_only",
    default_sort = "TenantId ASC, ParentId ASC",
    auth = "none"
)]
pub struct CompositeCapabilityParent {
    /// Tenant key component.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub tenant_id: i32,
    /// Parent key component.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub parent_id: i32,
    /// Bounded children selected with a composite relation key.
    #[graphql(skip)]
    #[relation(
        target = "CompositeCapabilityChild",
        from = ["tenant_id", "parent_id"],
        to = ["TenantId", "ParentId"],
        multiple,
        emit_fk = false
    )]
    pub children: Vec<CompositeCapabilityChild>,
}

/// Composite-key child record.
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[graphql(rename_fields = "PascalCase")]
#[graphql_entity(
    table = "CompositeCapabilityChildren",
    plural = "CompositeCapabilityChildren",
    schema_policy = "external_read_only",
    default_sort = "TenantId ASC, ParentId ASC, LineId ASC",
    auth = "none"
)]
pub struct CompositeCapabilityChild {
    /// Tenant key component.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub tenant_id: i32,
    /// Parent key component.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub parent_id: i32,
    /// Child line key component.
    #[primary_key]
    #[filterable(type = "number")]
    #[sortable]
    pub line_id: i32,
}

macro_rules! impl_fixture_batch_loader {
    ($entity:ty) => {
        impl graphql_orm::graphql::loaders::BatchLoadEntity for $entity {
            fn batch_column() -> &'static str {
                "Id"
            }

            fn batch_key_from_row(
                _row: &graphql_orm::DbRow,
            ) -> Result<String, graphql_orm::sqlx::Error> {
                Ok("compile-only-fixture".to_owned())
            }
        }
    };
}

impl_fixture_batch_loader!(CapabilityChild);
impl_fixture_batch_loader!(CapabilityProfile);
impl_fixture_batch_loader!(CompositeCapabilityChild);

schema_roots! {
    schema_policy: "external_read_only",
    entities: [
        CapabilityParent,
        CapabilityChild,
        CapabilityProfile,
        CompositeCapabilityParent,
        CompositeCapabilityChild
    ],
}

#[cfg(test)]
fn finished_schema() -> Schema<QueryRoot, MutationRoot, SubscriptionRoot> {
    Schema::build(
        QueryRoot::default(),
        MutationRoot::default(),
        SubscriptionRoot::default(),
    )
    .finish()
}

#[cfg(test)]
fn relationship<'a>(
    catalog: &'a GraphqlSemanticCatalog,
    entity: &str,
    field: &str,
) -> &'a GraphqlSemanticRelationshipDescriptor {
    catalog
        .entities
        .iter()
        .find(|candidate| candidate.entity_name == entity)
        .and_then(|entity| {
            entity
                .fields
                .iter()
                .find(|candidate| candidate.field_name == field)
        })
        .and_then(|field| field.relationship.as_ref())
        .expect("generated relationship semantics")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pascal_relationship_arguments_match_sdl_and_compile_nested_plan() {
        let schema = finished_schema();
        let sdl = schema.sdl();
        let parent_sdl = sdl
            .split("type CapabilityParent {")
            .nth(1)
            .and_then(|value| value.split("\n}").next())
            .expect("parent SDL object");
        assert!(parent_sdl.contains("OrderBy: CapabilityChildOrderByInput"));
        assert!(!parent_sdl.contains("OrderBy: [CapabilityChildOrderByInput"));
        let query_sdl = sdl
            .split("type Query {")
            .nth(1)
            .and_then(|value| value.split("\n}").next())
            .expect("query SDL object");
        assert!(query_sdl.contains("OrderBy: [CapabilityChildOrderByInput!]"));

        let semantics = graphql_orm_semantic_catalog();
        let children = relationship(semantics, "CapabilityParent", "Children");
        assert_eq!(
            children.arguments,
            vec![
                GraphqlSemanticArgumentDescriptor {
                    graphql_name: "Where".to_owned(),
                    description: "Filter related records".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        "CapabilityChildWhereInput",
                        GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                },
                GraphqlSemanticArgumentDescriptor {
                    graphql_name: "OrderBy".to_owned(),
                    description: "Order related records".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        "CapabilityChildOrderByInput",
                        GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                },
                GraphqlSemanticArgumentDescriptor {
                    graphql_name: "Page".to_owned(),
                    description: "Bound the related record page".to_owned(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        "PageInput",
                        GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                },
            ]
        );

        let profile = relationship(semantics, "CapabilityParent", "Profile");
        assert_eq!(
            profile.cardinality,
            GraphqlSemanticRelationshipCardinality::One
        );
        assert!(profile.arguments.is_empty());
        assert_eq!(
            children.collection_bound,
            Some(GraphqlSemanticCollectionBound::pageable("Page"))
        );
        assert!(profile.collection_bound.is_none());

        let composite = relationship(semantics, "CompositeCapabilityParent", "Children");
        assert_eq!(
            composite.arguments[1].type_ref,
            GraphqlSemanticTypeRef::named(
                "CompositeCapabilityChildOrderByInput",
                GraphqlSemanticTypeKind::Object,
                true,
            )
        );

        let root_list = semantics
            .operations
            .iter()
            .find(|operation| {
                operation.field_name == "CapabilityChildren"
                    && operation.generated_category == Some(GeneratedGraphqlOperationCategory::List)
            })
            .expect("generated root list semantics");
        assert!(matches!(
            &root_list.arguments[1].type_ref,
            GraphqlSemanticTypeRef::List {
                nullable: true,
                item,
                ..
            } if matches!(item.as_ref(), GraphqlSemanticTypeRef::Named {
                name,
                nullable: false,
                ..
            } if name == "CapabilityChildOrderByInput")
        ));

        let capabilities = AiGraphqlQueryCapabilityCatalog::compile(
            "relationship-fixture",
            GraphqlExecutionTargetId::parse("relationship.fixture").expect("target"),
            &sdl,
            semantics,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("complete public SDL and semantics compile");
        let parent = capabilities
            .capabilities()
            .find(|capability| capability.field_name() == "CapabilityParent")
            .expect("single-parent capability");
        let compiled = parent
            .compile(json!({
                "arguments": { "Id": 1 },
                "fields": { "Id": true },
                "relationships": {
                    "Children": {
                        "arguments": {},
                        "fields": { "Id": true, "Label": true },
                        "relationships": {},
                        "maximumItems": 5
                    }
                }
            }))
            .expect("bounded nested relationship plan compiles");
        assert!(
            compiled
                .descriptor()
                .document
                .contains("Children(Page: $v1)")
        );
    }

    #[test]
    fn list_vs_object_relationship_tamper_still_fails_closed() {
        let schema = finished_schema();
        let original = graphql_orm_semantic_catalog();
        let mut entities = original.entities.clone();
        let order_by = entities
            .iter_mut()
            .find(|entity| entity.entity_name == "CapabilityParent")
            .and_then(|entity| {
                entity
                    .fields
                    .iter_mut()
                    .find(|field| field.field_name == "Children")
            })
            .and_then(|field| field.relationship.as_mut())
            .and_then(|relationship| {
                relationship
                    .arguments
                    .iter_mut()
                    .find(|argument| argument.graphql_name == "OrderBy")
            })
            .expect("relationship ordering semantics");
        order_by.type_ref = GraphqlSemanticTypeRef::list(
            true,
            Some(32),
            GraphqlSemanticTypeRef::named(
                "CapabilityChildOrderByInput",
                GraphqlSemanticTypeKind::Object,
                true,
            ),
        );
        let tampered = GraphqlSemanticCatalog::compose_with_custom(
            entities,
            &GraphqlOperationCatalog::compose(std::iter::empty()),
            original.operations.clone(),
        )
        .expect("tampered contract remains internally canonical");
        assert_ne!(tampered.fingerprint, original.fingerprint);
        let alternate_sdl = schema.sdl().replace(
            "OrderBy: CapabilityChildOrderByInput",
            "OrderBy: [CapabilityChildOrderByInput]",
        );
        let corrected_capabilities = AiGraphqlQueryCapabilityCatalog::compile(
            "relationship-fixture",
            GraphqlExecutionTargetId::parse("relationship.fixture").expect("target"),
            &schema.sdl(),
            original,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("corrected contract compiles");
        let alternate_capabilities = AiGraphqlQueryCapabilityCatalog::compile(
            "relationship-fixture",
            GraphqlExecutionTargetId::parse("relationship.fixture").expect("target"),
            &alternate_sdl,
            &tampered,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("internally matched alternate contract compiles");
        assert_ne!(
            corrected_capabilities.fingerprint(),
            alternate_capabilities.fingerprint()
        );
        let error = AiGraphqlQueryCapabilityCatalog::compile(
            "relationship-fixture",
            GraphqlExecutionTargetId::parse("relationship.fixture").expect("target"),
            &schema.sdl(),
            &tampered,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect_err("finished SDL remains authoritative");
        assert!(matches!(
            error,
            graphql_orm_ai_tool_profiles::AiError::InvalidConfiguration(message)
                if message == "semantic relationship arguments have drifted from finished SDL"
        ));
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod secret_generated_results {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// Operator-only provider settings retain their generated GraphQL roots.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql(rename_fields = "PascalCase")]
    #[graphql_entity(
        table = "SecretSettings",
        plural = "SecretSettingsRows",
        keyset = "id asc",
        classification = "secret"
    )]
    struct SecretSettings {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        id: String,
        secret_value: String,
    }

    /// Public classification does not override an absolute field export ban.
    #[derive(
        GraphQLEntity, GraphQLOperations, Clone, Debug, serde::Serialize, serde::Deserialize,
    )]
    #[graphql(rename_fields = "PascalCase")]
    #[graphql_entity(
        table = "HiddenSettings",
        plural = "HiddenSettingsRows",
        keyset = "id asc",
        classification = "public"
    )]
    struct HiddenSettings {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        #[graphql_orm(non_exportable)]
        id: String,
        #[graphql_orm(non_exportable)]
        hidden_value: String,
    }

    /// Exportable inventory remains available beside operator-only settings.
    #[derive(
        GraphQLEntity,
        GraphQLRelations,
        GraphQLOperations,
        SimpleObject,
        Clone,
        Debug,
        serde::Serialize,
        serde::Deserialize,
    )]
    #[graphql(complex, rename_fields = "PascalCase")]
    #[graphql_entity(
        table = "PublicInventory",
        plural = "PublicInventoryRows",
        keyset = "id asc",
        classification = "public"
    )]
    struct PublicInventory {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        id: String,
        label: String,
        settings_id: Option<String>,
        hidden_settings_id: Option<String>,
        #[graphql_orm(sensitive)]
        provider_token: String,
        #[graphql(skip)]
        #[relation(
            target = "SecretSettings",
            from = "settings_id",
            to = "Id",
            propagate_change = "none",
            emit_fk = false
        )]
        settings: Option<SecretSettings>,
        #[graphql(skip)]
        #[relation(
            target = "HiddenSettings",
            from = "hidden_settings_id",
            to = "Id",
            propagate_change = "none",
            emit_fk = false
        )]
        hidden_settings: Option<HiddenSettings>,
    }
    impl_fixture_batch_loader!(SecretSettings);
    impl_fixture_batch_loader!(HiddenSettings);
    schema_roots! {
        query_custom_ops: [],
        entities: [PublicInventory, SecretSettings, HiddenSettings],
        generated_mutations: "none",
    }

    #[test]
    fn mixed_generated_roots_preserve_operator_sdl_without_private_provider_capabilities() {
        let schema = Schema::build(
            QueryRoot::default(),
            MutationRoot::default(),
            SubscriptionRoot::default(),
        )
        .finish();
        let sdl = schema.sdl();
        let semantic = graphql_orm_semantic_catalog();
        let composed = GraphqlSemanticCatalog::compose(
            semantic.entities.clone(),
            &graphql_orm_operation_catalog(),
        )
        .expect("ordinary generated composition");
        assert_eq!(
            composed.fingerprint, semantic.fingerprint,
            "compose and compose_with_custom must share disclosure semantics"
        );
        semantic
            .validate()
            .expect("complete generated semantics remain valid");
        for name in ["SecretSettings", "HiddenSettings"] {
            let operations = semantic
                .operations
                .iter()
                .filter(|operation| operation.generated_entity_name.as_deref() == Some(name))
                .collect::<Vec<_>>();
            assert!(
                operations
                    .iter()
                    .any(|operation| operation.generated_category
                        == Some(GeneratedGraphqlOperationCategory::List))
            );
            assert!(
                operations
                    .iter()
                    .any(|operation| operation.generated_category
                        == Some(GeneratedGraphqlOperationCategory::KeysetList))
            );
            assert!(
                operations
                    .iter()
                    .any(|operation| operation.field_name.ends_with("Scan"))
            );
            assert!(
                operations
                    .iter()
                    .any(|operation| operation.kind == GraphqlOperationKind::Subscription)
            );
            for operation in operations {
                assert!(
                    sdl.contains(&operation.field_name),
                    "operator root disappeared: {}",
                    operation.field_name
                );
                let result = operation
                    .result_disclosure
                    .expect("generated result disclosure");
                assert_eq!(result.export, GraphqlSemanticExport::NeverExport);
                if name == "SecretSettings" {
                    assert_eq!(result.classification, GraphqlSemanticClassification::Secret);
                }
            }
        }
        let capabilities = AiGraphqlQueryCapabilityCatalog::compile(
            "mixed-generated",
            GraphqlExecutionTargetId::parse("mixed.generated").unwrap(),
            &sdl,
            semantic,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .expect("secret generated roots must not abort public catalogue compilation");
        let expected = semantic
            .operations
            .iter()
            .filter(|operation| {
                operation.kind == GraphqlOperationKind::Query
                    && operation.generated_entity_name.as_deref() == Some("PublicInventory")
            })
            .map(|operation| operation.field_name.clone())
            .collect::<BTreeSet<_>>();
        let actual = capabilities
            .capabilities()
            .map(|capability| capability.field_name().to_owned())
            .collect::<BTreeSet<_>>();
        assert!(!expected.is_empty());
        assert_eq!(
            actual, expected,
            "all public roots and only public roots must remain"
        );
        for capability in capabilities.capabilities() {
            let arguments = capability.argument_schema();
            assert!(
                arguments
                    .pointer("/properties/relationships/properties/Settings")
                    .is_none()
            );
            assert!(
                arguments
                    .pointer("/properties/relationships/properties/HiddenSettings")
                    .is_none()
            );
            assert!(
                arguments
                    .pointer("/properties/fields/properties/ProviderToken")
                    .is_none()
            );
            let compact = capability.compact_argument_schema().to_string();
            for private in [
                "\"Settings\"",
                "\"HiddenSettings\"",
                "SecretValue",
                "HiddenValue",
                "ProviderToken",
            ] {
                assert!(
                    !compact.contains(private),
                    "private projection entered provider schema"
                );
            }
        }
        let inventory = capabilities
            .capabilities()
            .find(|capability| capability.field_name() == "PublicInventory")
            .unwrap();
        let public = inventory
            .compile(
                json!({"arguments":{"Id":"inventory-one"},"fields":{"Id":true,"Label":true},"relationships":{}}),
            )
            .expect("public inventory plan");
        assert!(!public.descriptor().document.contains("Settings"));
        assert!(inventory.compile(json!({"arguments":{"Id":"inventory-one"},"fields":{"Id":true},"relationships":{"Settings":{"arguments":{},"fields":{"Id":true},"relationships":{}}}})).is_err(), "provider cannot manually request a hidden relation");
        assert!(inventory.compile_compact(json!({"arguments":{"Id":"inventory-one"},"selections":["Settings.Id"],"relationshipArguments":{},"relationshipMaximumItems":{}})).is_err(), "compact plans cannot forge a private relation path");
    }
    #[test]
    fn generated_descriptor_composition_tightens_disclosure_without_repairing_stale_input() {
        let semantic = graphql_orm_semantic_catalog();
        let source = semantic
            .operations
            .iter()
            .find(|operation| {
                operation.kind == GraphqlOperationKind::Query
                    && operation.generated_entity_name.as_deref() == Some("SecretSettings")
            })
            .unwrap();
        let generated = GraphqlSemanticOperationDescriptor::generated(
            source.kind,
            source.generated_category.unwrap(),
            "SecretSettings",
            source.field_name.clone(),
            source.description.clone(),
            source.arguments.clone(),
            source.result_type.clone(),
            source.has_authorization_policy,
        )
        .unwrap();
        assert!(generated.result_disclosure.is_none());
        let composed =
            GraphqlSemanticCatalog::compose_descriptors(semantic.entities.clone(), [generated])
                .unwrap();
        assert_eq!(
            composed.operations[0].result_disclosure.unwrap().export,
            GraphqlSemanticExport::NeverExport
        );
        let public = semantic
            .operations
            .iter()
            .find(|operation| {
                operation.kind == GraphqlOperationKind::Query
                    && operation.generated_entity_name.as_deref() == Some("PublicInventory")
            })
            .unwrap()
            .clone();
        let tightened = public
            .clone()
            .with_result_disclosure(GraphqlSemanticResultDisclosure::fail_safe())
            .unwrap();
        let composed =
            GraphqlSemanticCatalog::compose_descriptors(semantic.entities.clone(), [tightened])
                .unwrap();
        assert_eq!(
            composed.operations[0].result_disclosure.unwrap(),
            GraphqlSemanticResultDisclosure::fail_safe()
        );
        let mut stale = public;
        stale.field_name = "TamperedRoot".to_owned();
        assert!(
            GraphqlSemanticCatalog::compose_descriptors(semantic.entities.clone(), [stale])
                .is_err()
        );
    }
}
