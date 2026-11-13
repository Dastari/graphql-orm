// This fixture selects one backend per build; coexistence has dedicated targets.
#![cfg(not(any(
    all(feature = "sqlite", feature = "postgres"),
    all(feature = "sqlite", feature = "mssql"),
    all(feature = "postgres", feature = "mssql"),
)))]
#![cfg(feature = "sqlite")]
//! The generated operation catalogue must describe exactly the root fields and
//! arguments the generated SDL exports, or a router binding the descriptor to
//! the composed graph refuses to start.

#[path = "support/catalog_sdl_conformance.rs"]
mod catalog_sdl_conformance;

use catalog_sdl_conformance::assert_catalog_matches_sdl;
use graphql_orm::async_graphql::SDLExportOptions;
use graphql_orm::graphql::orm::GraphqlOperationKind;
use graphql_orm::prelude::*;

async fn database() -> graphql_orm::db::Database {
    graphql_orm::db::Database::new(
        sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("SQLite test pool"),
    )
}

/// Representative writable shapes sharing one schema root.
mod mixed {
    use graphql_orm::prelude::*;

    /// Managed version with every optional read capability enabled.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_managed_records",
        plural = "ConformanceManagedRecords",
        keyset = "created_at desc, id asc",
        upsert = "slug",
        aggregate = true
    )]
    #[graphql_orm(operation_authorization(
        categories = ["update", "delete"],
        all_scopes = ["records.write"]
    ))]
    pub struct ConformanceManagedRecord {
        #[primary_key]
        #[sortable]
        pub id: String,
        #[unique]
        #[filterable(type = "string")]
        #[graphql_orm(searchable(weight = "A"))]
        pub slug: String,
        #[filterable(type = "string")]
        pub title: String,
        #[filterable(type = "number")]
        #[sortable]
        pub created_at: i64,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
    }

    /// Compare-and-swap version without managed advancement.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "conformance_cas_records", plural = "ConformanceCasRecords")]
    pub struct ConformanceCasRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
        #[graphql_orm(version, default = "0")]
        pub revision: i64,
    }

    /// Composite key whose writes are repository-only.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_composite_records",
        plural = "ConformanceCompositeRecords",
        repository_mutations = true,
        keyset = "tenant_id asc, code asc",
        default_sort = "tenant_id ASC, code ASC"
    )]
    pub struct ConformanceCompositeRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub tenant_id: String,
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub code: String,
        #[filterable(type = "string")]
        pub value: String,
    }

    /// An entity-level read-only entity inside a writable root.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_external_records",
        plural = "ConformanceExternalRecords",
        schema_policy = "external_read_only"
    )]
    pub struct ConformanceExternalRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    /// Append-only rows expose create and subscriptions only.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_append_records",
        plural = "ConformanceAppendRecords",
        append_only = true
    )]
    pub struct ConformanceAppendRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub message: String,
    }

    /// Shape selection removes resolvers and their catalogue entries together.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_shaped_records",
        plural = "ConformanceShapedRecords",
        query_shapes = "single,list",
        mutation_shapes = "create,update,delete_many"
    )]
    pub struct ConformanceShapedRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
    }

    /// Dependent deletes rewrite the update and delete resolvers.
    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "conformance_parents",
        plural = "ConformanceParents",
        default_sort = "id ASC"
    )]
    pub struct ConformanceParent {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub name: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        dependent_deletes,
        table = "conformance_children",
        plural = "ConformanceChildren",
        default_sort = "id ASC"
    )]
    pub struct ConformanceChild {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub parent_id: String,
        #[relation(
            target = "ConformanceParent",
            from = "parent_id",
            to = "id",
            on_delete = "cascade"
        )]
        pub parent: Option<ConformanceParent>,
    }

    schema_roots! {
        query_custom_ops: [],
        entities: [
            ConformanceManagedRecord,
            ConformanceCasRecord,
            ConformanceCompositeRecord,
            ConformanceExternalRecord,
            ConformanceAppendRecord,
            ConformanceShapedRecord,
            ConformanceParent,
            ConformanceChild,
        ],
    }
}

/// The macros emit relation-count order aliases for readable to-one relations.
mod one_count {
    use graphql_orm::async_graphql::SimpleObject;
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_count_targets",
        plural = "ConformanceCountTargets"
    )]
    pub struct ConformanceCountTarget {
        #[primary_key]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    #[derive(
        GraphQLEntity,
        GraphQLOperations,
        GraphQLRelations,
        SimpleObject,
        serde::Serialize,
        serde::Deserialize,
        Clone,
        Debug,
    )]
    #[graphql(complex, rename_fields = "PascalCase")]
    #[graphql_entity(
        table = "conformance_count_sources",
        plural = "ConformanceCountSources"
    )]
    pub struct ConformanceCountSource {
        #[primary_key]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub target_id: String,
        #[graphql(skip)]
        #[relation(
            target = "ConformanceCountTarget",
            from = "target_id",
            to = "id",
            emit_fk = false,
            order_aggregate(name = "TargetCount", aggregate = "count")
        )]
        pub target: Option<ConformanceCountTarget>,
    }

    impl BatchLoadEntity for ConformanceCountTarget {
        fn batch_column() -> &'static str {
            "id"
        }
        fn batch_key_from_row(
            row: &graphql_orm::DbRow,
        ) -> Result<String, graphql_orm::sqlx::Error> {
            use graphql_orm::sqlx::Row;
            row.try_get("id")
        }
    }

    schema_roots! {
        query_custom_ops: [],
        entities: [ConformanceCountTarget, ConformanceCountSource],
    }
}

/// A writable root whose only entity has repository-only composite writes.
mod composite_only {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_composite_only",
        plural = "ConformanceCompositeOnlyRecords",
        repository_mutations = true,
        default_sort = "tenant_id ASC, code ASC"
    )]
    pub struct ConformanceCompositeOnlyRecord {
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub tenant_id: String,
        #[primary_key]
        #[graphql_orm(auto_generated = false)]
        #[filterable(type = "string")]
        #[sortable]
        pub code: String,
    }

    schema_roots! {
        query_custom_ops: [],
        entities: [ConformanceCompositeOnlyRecord],
    }
}

/// A read-only root with keyset reads.
mod read_only {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(
        table = "conformance_read_only",
        plural = "ConformanceReadOnlyRecords",
        schema_policy = "external_read_only",
        keyset = "label asc, id asc"
    )]
    pub struct ConformanceReadOnlyRecord {
        #[primary_key]
        #[sortable]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        schema_policy: "external_read_only",
        entities: [ConformanceReadOnlyRecord],
    }
}

/// A mutation allowlist hides one entity's generated writes.
mod allowlist {
    use graphql_orm::prelude::*;

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "conformance_listed", plural = "ConformanceListedRecords")]
    pub struct ConformanceListedRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
        #[graphql_orm(version = "managed", default = "0")]
        pub row_version: i64,
    }

    #[derive(
        GraphQLEntity, GraphQLOperations, serde::Serialize, serde::Deserialize, Clone, Debug,
    )]
    #[graphql_entity(table = "conformance_unlisted", plural = "ConformanceUnlistedRecords")]
    pub struct ConformanceUnlistedRecord {
        #[primary_key]
        pub id: String,
        #[filterable(type = "string")]
        #[sortable]
        pub label: String,
    }

    schema_roots! {
        generated_mutations: "allowlist",
        generated_mutation_allowlist: [ConformanceListedRecord],
        entities: [ConformanceListedRecord, ConformanceUnlistedRecord],
    }
}

macro_rules! sdl_of {
    ($module:ident) => {{
        let schema = $module::schema_builder(database().await)
            .enable_subscription_in_federation()
            .finish();
        (
            schema.sdl(),
            schema.sdl_with_options(SDLExportOptions::new().federation()),
        )
    }};
}

macro_rules! conformance_test {
    ($name:ident, $module:ident) => {
        #[tokio::test]
        async fn $name() {
            let (sdl, federation_sdl) = sdl_of!($module);
            let catalog = $module::graphql_orm_operation_catalog();
            assert_catalog_matches_sdl(catalog, &sdl);
            assert_catalog_matches_sdl(catalog, &federation_sdl);
            #[cfg(feature = "router-protocol")]
            catalog_sdl_conformance::assert_router_protocol_matches_sdl(catalog, &federation_sdl);
        }
    };
}

conformance_test!(mixed_writable_root_catalogue_matches_its_sdl, mixed);
conformance_test!(
    composite_only_writable_root_catalogue_matches_its_sdl,
    composite_only
);
conformance_test!(read_only_root_catalogue_matches_its_sdl, read_only);
conformance_test!(mutation_allowlist_root_catalogue_matches_its_sdl, allowlist);

fn placeholder_name(entity: &str) -> String {
    format!("gom{entity}WritesUnavailable")
}

/// The placeholder that keeps a write-less entity's mutation object non-empty
/// is described like any root field, can never be authorized, and refuses
/// every request itself.
#[tokio::test]
async fn write_placeholders_are_described_unsatisfiable_and_refused() {
    use graphql_orm::graphql::orm::{AiMutationExecutionPolicy, GraphqlAuthorizationRequirement};

    let (sdl, federation_sdl) = sdl_of!(mixed);
    for entity in ["ConformanceCompositeRecord", "ConformanceExternalRecord"] {
        let name = placeholder_name(entity);
        assert!(sdl.contains(&name), "{name} is in the SDL");
        assert!(
            federation_sdl.contains(&name),
            "{name} is in the federation SDL"
        );
        let operation = mixed::graphql_orm_operation_catalog()
            .resolve(GraphqlOperationKind::Mutation, &name)
            .unwrap_or_else(|| panic!("{name} is described exactly once"));
        assert_eq!(
            operation.category(),
            GeneratedGraphqlOperationCategory::Unavailable
        );
        assert_eq!(
            operation.authorization(),
            &GraphqlAuthorizationRequirement::unsatisfiable()
        );
        assert_eq!(
            operation.ai_mutation_execution(),
            AiMutationExecutionPolicy::Prohibited
        );
        assert!(operation.arguments().is_empty());
    }

    let schema = composite_only::schema_builder(database().await).finish();
    let name = placeholder_name("ConformanceCompositeOnlyRecord");
    let response = schema.execute(format!("mutation {{ {name} }}")).await;
    assert_eq!(response.errors.len(), 1, "the placeholder is refused");
    let error = serde_json::to_value(&response.errors[0]).expect("error serializes");
    assert_eq!(error["extensions"]["code"], "FORBIDDEN", "{error}");
}

/// Hosts building a strict assurance registry from the catalogue need not
/// classify placeholders, and the registry covers every SDL mutation root.
#[tokio::test]
async fn strict_assurance_registries_cover_write_placeholders() {
    use graphql_orm::graphql::assurance::{
        AssuranceSchemaConfig, GENERATED_PLACEHOLDER_EXEMPTION, OperationAssuranceClassification,
        OperationAssuranceRegistry,
    };

    let (sdl, _) = sdl_of!(composite_only);
    let registry = OperationAssuranceRegistry::builder(
        composite_only::graphql_orm_operation_catalog(),
        AssuranceSchemaConfig::legacy().with_strict_mutation_classification(true),
    )
    .build()
    .expect("a placeholder needs no host classification");
    registry
        .ensure_schema_sdl(&sdl)
        .expect("every SDL mutation root is registered");
    assert_eq!(
        registry.classification(
            GraphqlOperationKind::Mutation,
            &placeholder_name("ConformanceCompositeOnlyRecord")
        ),
        Some(&OperationAssuranceClassification::Exempt {
            reason: GENERATED_PLACEHOLDER_EXEMPTION.to_owned()
        })
    );
}

/// The router protocol carries the unsatisfiable requirement verbatim.
#[cfg(feature = "router-protocol")]
#[test]
fn write_placeholders_export_an_unsatisfiable_router_requirement() {
    use graphql_orm::graphql::orm::GRAPHQL_UNSATISFIABLE_SCOPE;
    use graphql_orm_router_protocol::{AuthorizationRequirement, RootOperationType};

    let name = placeholder_name("ConformanceCompositeOnlyRecord");
    let operation = composite_only::graphql_orm_operation_catalog()
        .router_protocol_operations()
        .expect("protocol export")
        .into_iter()
        .find(|operation| {
            operation.root_type == RootOperationType::Mutation && operation.field_name == name
        })
        .expect("the placeholder is exported");
    let AuthorizationRequirement::AllScopes { scopes } = operation.authorization else {
        panic!("the placeholder requires scopes");
    };
    assert_eq!(
        scopes
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>(),
        [GRAPHQL_UNSATISFIABLE_SCOPE]
    );
    assert!(
        GRAPHQL_UNSATISFIABLE_SCOPE.contains(' '),
        "no bearer scope can contain a space"
    );
}

#[tokio::test]
async fn managed_version_writes_publish_the_expected_version_argument() {
    let catalog = mixed::graphql_orm_operation_catalog();
    for category in [
        GeneratedGraphqlOperationCategory::Update,
        GeneratedGraphqlOperationCategory::Delete,
    ] {
        let operation = catalog
            .exposed_operations()
            .find(|operation| {
                operation.category() == category
                    && operation
                        .entity_rust_type()
                        .ends_with("ConformanceManagedRecord")
            })
            .expect("managed record write is exposed");
        let argument = operation
            .arguments()
            .iter()
            // Every argument-case feature spells the same two words.
            .find(|argument| {
                argument
                    .graphql_name()
                    .replace('_', "")
                    .eq_ignore_ascii_case("expectedVersion")
            })
            .expect("managed record write declares the expected version");
        assert_eq!(argument.graphql_type(), "Int");
    }
}

/// The SDL side of the comparison reads exact argument types.
#[test]
fn sdl_contract_reads_every_argument_type() {
    let contract = catalog_sdl_conformance::sdl_root_contract(
        "type Query { a(id: ID!, expected: Int, keys: [String!]): Int _hidden: Int _service: Int _entities: Int }",
    );
    assert_eq!(
        contract
            .keys()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["_hidden", "a"],
        "only the planner-owned federation fields are exempt"
    );
    let arguments = &contract[&("Query", "a".to_owned())];
    assert_eq!(arguments["id"].0, "ID!");
    assert_eq!(arguments["expected"].0, "Int");
    assert_eq!(arguments["keys"].0, "[String!]");
}

/// All generated input families are exercised through complete query capabilities
/// over SDL produced by the real macros, including create/update and composite keys.
#[tokio::test]
async fn generated_input_families_compile_complete_capabilities() {
    use graphql_orm::graphql::orm::*;
    use graphql_orm_ai_tool_profiles::*;
    let mut all_covered = Vec::new();
    for (mut sdl, catalog) in [
        (sdl_of!(mixed).0, mixed::graphql_orm_semantic_catalog()),
        (
            sdl_of!(one_count).0,
            one_count::graphql_orm_semantic_catalog(),
        ),
        (
            sdl_of!(composite_only).0,
            composite_only::graphql_orm_semantic_catalog(),
        ),
    ] {
        if catalog
            .entities
            .iter()
            .any(|entity| entity.entity_name == "ConformanceCountSource")
        {
            assert!(sdl.contains("TargetCount: OrderDirection"), "{sdl}");
        }
        let start = sdl.find("type Query {").expect("generated query root");
        let end = start + sdl[start..].find("\n}").unwrap() + 2;
        sdl.replace_range(start..end, "type Query {\n}");
        let entities = catalog
            .entities
            .iter()
            .filter(|entity| {
                matches!(
                    entity.entity_name.as_str(),
                    "ConformanceManagedRecord"
                        | "ConformanceCountSource"
                        | "ConformanceCountTarget"
                        | "ConformanceCompositeRecord"
                        | "ConformanceCompositeOnlyRecord"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut operations = Vec::new();
        let mut covered = Vec::new();
        for entity in &entities {
            for name in [
                format!("{}KeyInput", entity.entity_name),
                format!("Create{}Input", entity.entity_name),
                format!("Update{}Input", entity.entity_name),
                format!("{}WhereInput", entity.entity_name),
                format!("{}OrderByInput", entity.entity_name),
                format!("{}KeysetOrderByInput", entity.entity_name),
                format!("{}AggregateMetricInput", entity.entity_name),
                "SearchInput".to_owned(),
                "SubscriptionFilterInput".to_owned(),
                "PageInput".to_owned(),
            ] {
                if !sdl.contains(&format!("input {name} {{")) {
                    continue;
                }
                let field = format!("probe{}", covered.len());
                let argument_name = "input";
                let page_argument = if name == "PageInput" {
                    ", page: PageInput"
                } else {
                    ""
                };
                let mut arguments = vec![GraphqlSemanticArgumentDescriptor {
                    graphql_name: argument_name.into(),
                    description: "Generated input.".into(),
                    type_ref: GraphqlSemanticTypeRef::named(
                        &name,
                        GraphqlSemanticTypeKind::Object,
                        true,
                    ),
                }];
                if name == "PageInput" {
                    arguments.push(GraphqlSemanticArgumentDescriptor {
                        graphql_name: "page".into(),
                        description: "Server pagination.".into(),
                        type_ref: GraphqlSemanticTypeRef::named(
                            &name,
                            GraphqlSemanticTypeKind::Object,
                            true,
                        ),
                    });
                }
                sdl = sdl.replacen(
                    "type Query {",
                    &format!(
                        "type Query {{\n {field}({argument_name}: {name}{page_argument}): {}",
                        entity.entity_name
                    ),
                    1,
                );
                operations.push(
                    GraphqlSemanticOperationDescriptor::generated(
                        GraphqlOperationKind::Query,
                        GeneratedGraphqlOperationCategory::SingleRead,
                        entity.entity_name.clone(),
                        field,
                        "Inspect a generated input contract.",
                        arguments,
                        GraphqlSemanticTypeRef::named(
                            &entity.entity_name,
                            GraphqlSemanticTypeKind::Object,
                            true,
                        ),
                        true,
                    )
                    .unwrap(),
                );
                covered.push(name);
            }
        }
        assert!(!covered.is_empty());
        let semantic = GraphqlSemanticCatalog::compose_descriptors(entities, operations).unwrap();
        let capabilities = AiGraphqlQueryCapabilityCatalog::compile(
            "generated-inputs",
            GraphqlExecutionTargetId::parse("generated.inputs").unwrap(),
            &sdl,
            &semantic,
            AiGraphqlQueryCapabilityLimits::default(),
        )
        .unwrap();
        assert_eq!(
            capabilities
                .capabilities()
                .filter(|capability| capability.field_name().starts_with("probe"))
                .count(),
            covered.len()
        );
        for (index, name) in covered.iter().enumerate() {
            let capability = capabilities
                .capabilities()
                .find(|capability| capability.field_name() == format!("probe{index}"))
                .unwrap();
            let input = &capability.compact_argument_schema()["properties"]["arguments"]["properties"]
                ["input"];
            let input = input
                .get("anyOf")
                .and_then(|values| values.as_array())
                .map(|values| &values[0])
                .unwrap_or(input);
            assert!(
                !input["properties"]
                    .as_object()
                    .unwrap_or_else(|| panic!("input contract for {name}: {input}"))
                    .is_empty(),
                "generated members are disclosed for {name}"
            );
            if name == "ConformanceCountSourceOrderByInput" {
                assert!(input["properties"].get("TargetCount").is_none());
            }
        }
        all_covered.extend(covered);
    }
    for suffix in [
        "KeyInput",
        "WhereInput",
        "OrderByInput",
        "KeysetOrderByInput",
        "AggregateMetricInput",
        "SearchInput",
        "SubscriptionFilterInput",
        "PageInput",
    ] {
        assert!(
            all_covered.iter().any(|name| name.ends_with(suffix)
                && (suffix != "OrderByInput" || !name.ends_with("KeysetOrderByInput"))),
            "missing {suffix}: {all_covered:?}"
        );
    }
    for prefix in ["Create", "Update"] {
        assert!(
            all_covered.iter().any(|name| name.starts_with(prefix)),
            "missing {prefix}"
        );
    }
}
