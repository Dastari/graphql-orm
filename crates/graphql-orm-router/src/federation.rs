use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
use arc_swap::ArcSwap;
use cynic_parser::{common::OperationType, type_system::Definition as CynicDefinition};
use graphql_composition::{
    Subgraphs, compose, diagnostics::Severity as ComposerSeverity, render_federated_sdl,
};
use hive_router::{
    graphql_tools::{
        parser::schema::Definition,
        static_graphql::schema::{Directive, TypeDefinition, Value},
    },
    plugins::hooks::on_supergraph_load::Supergraph,
    query_planner::{planner::QueryPlannerOptions, utils::parsing::safe_parse_schema},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSubgraph {
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) sdl: Arc<str>,
    pub(crate) revision: Option<String>,
}

impl CandidateSubgraph {
    pub(crate) fn new(name: &str, endpoint: &str, sdl: &str) -> Self {
        Self {
            name: name.to_owned(),
            endpoint: endpoint.to_owned(),
            sdl: Arc::from(sdl),
            revision: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompositionDiagnostic {
    pub(crate) severity: DiagnosticSeverity,
    pub(crate) code: Option<String>,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompositionDiagnostics(Vec<CompositionDiagnostic>);

impl CompositionDiagnostics {
    fn from_composer(diagnostics: &graphql_composition::Diagnostics) -> Self {
        Self(
            diagnostics
                .iter()
                .map(|diagnostic| CompositionDiagnostic {
                    severity: match diagnostic.severity() {
                        ComposerSeverity::Warning => DiagnosticSeverity::Warning,
                        ComposerSeverity::Error => DiagnosticSeverity::Error,
                    },
                    code: diagnostic
                        .composite_schemas_error_code()
                        .map(|code| format!("{code:?}")),
                    message: diagnostic.message().to_owned(),
                })
                .collect(),
        )
    }

    fn warnings(&self) -> Vec<CompositionDiagnostic> {
        self.0
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
            .cloned()
            .collect()
    }
}

impl fmt::Display for CompositionDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut diagnostics = self.0.iter();
        if let Some(first) = diagnostics.next() {
            formatter.write_str(&first.message)?;
            for diagnostic in diagnostics {
                write!(formatter, "; {}", diagnostic.message)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub(crate) enum FederationError {
    #[error("a candidate graph must contain at least one subgraph")]
    EmptyCandidate,
    #[error("candidate contains duplicate subgraph name `{name}`")]
    DuplicateSubgraph { name: String },
    #[error("subgraph `{subgraph}` has invalid SDL: {details}")]
    SourceSdl { subgraph: String, details: String },
    #[error("composition rejected the candidate: {0}")]
    Composition(CompositionDiagnostics),
    #[error("failed to render the composed supergraph: {details}")]
    Render { details: String },
    #[error("the composed supergraph is invalid: {details}")]
    SupergraphSdl { details: String },
    #[error("the composed supergraph does not identify subgraph `{subgraph}`")]
    MissingJoinGraph { subgraph: String },
    #[error("Hive could not construct an executable supergraph: {details}")]
    Runtime { details: String },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RootParticipation {
    query: bool,
    mutation: bool,
    subscription: bool,
}

impl RootParticipation {
    fn as_array(self) -> [bool; 3] {
        [self.query, self.mutation, self.subscription]
    }
}

struct ComposedCandidate {
    document: hive_router::graphql_tools::static_graphql::schema::Document,
    sdl: Arc<str>,
    warnings: Vec<CompositionDiagnostic>,
}

pub(crate) struct ActiveGraph {
    pub(crate) version: u64,
    pub(crate) fingerprint: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) supergraph_sdl: Arc<str>,
    pub(crate) warnings: Arc<[CompositionDiagnostic]>,
    pub(crate) hive: Arc<Supergraph>,
}

#[cfg(test)]
pub(crate) struct GraphStore {
    active: ArcSwap<ActiveGraph>,
    #[cfg_attr(not(test), allow(dead_code))]
    replacement: Mutex<()>,
}

#[cfg(test)]
impl GraphStore {
    pub(crate) fn new(inputs: &[CandidateSubgraph]) -> Result<Self, FederationError> {
        Ok(Self {
            active: ArcSwap::from(build_active_graph(inputs, 1)?),
            replacement: Mutex::new(()),
        })
    }

    pub(crate) fn load(&self) -> Arc<ActiveGraph> {
        self.active.load_full()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn replace(
        &self,
        inputs: &[CandidateSubgraph],
    ) -> Result<Arc<ActiveGraph>, FederationError> {
        let _replacement = self
            .replacement
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let version = self.active.load().version.saturating_add(1);
        let candidate = build_active_graph(inputs, version)?;
        self.active.store(candidate.clone());
        Ok(candidate)
    }
}

pub(crate) fn build_active_graph(
    inputs: &[CandidateSubgraph],
    version: u64,
) -> Result<Arc<ActiveGraph>, FederationError> {
    let composed = compose_candidate(inputs)?;
    let fingerprint = format!("sha256:{:x}", Sha256::digest(composed.sdl.as_bytes()));
    let hive = Supergraph::from_document(composed.document, QueryPlannerOptions::default())
        .map_err(|error| FederationError::Runtime {
            details: error.to_string(),
        })?;

    Ok(Arc::new(ActiveGraph {
        version,
        fingerprint,
        supergraph_sdl: composed.sdl,
        warnings: composed.warnings.into(),
        hive: Arc::new(hive),
    }))
}

fn compose_candidate(inputs: &[CandidateSubgraph]) -> Result<ComposedCandidate, FederationError> {
    if inputs.is_empty() {
        return Err(FederationError::EmptyCandidate);
    }

    let mut inputs = inputs.iter().collect::<Vec<_>>();
    inputs.sort_by(|left, right| {
        (&left.name, &left.endpoint, &left.revision).cmp(&(
            &right.name,
            &right.endpoint,
            &right.revision,
        ))
    });
    for pair in inputs.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(FederationError::DuplicateSubgraph {
                name: pair[0].name.clone(),
            });
        }
    }

    let mut root_participation = BTreeMap::new();
    let mut subgraphs = Subgraphs::default();
    for input in &inputs {
        root_participation.insert(input.name.as_str(), inspect_roots(input)?);
        subgraphs
            .ingest_str(&input.sdl, &input.name, Some(&input.endpoint))
            .map_err(|error| FederationError::SourceSdl {
                subgraph: input.name.clone(),
                details: format!("{error:#}"),
            })?;
    }

    let result = compose(&mut subgraphs);
    let diagnostics = CompositionDiagnostics::from_composer(result.diagnostics());
    let graph = result
        .into_result()
        .map_err(|_| FederationError::Composition(diagnostics.clone()))?;
    let sdl = render_federated_sdl(&graph).map_err(|error| FederationError::Render {
        details: error.to_string(),
    })?;
    let document = adapt_composed_supergraph(&sdl, &root_participation)?;
    let normalized_sdl: Arc<str> = Arc::from(document.to_string());

    Ok(ComposedCandidate {
        document,
        sdl: normalized_sdl,
        warnings: diagnostics.warnings(),
    })
}

fn inspect_roots(input: &CandidateSubgraph) -> Result<RootParticipation, FederationError> {
    let document = cynic_parser::parse_type_system_document(&input.sdl).map_err(|error| {
        FederationError::SourceSdl {
            subgraph: input.name.clone(),
            details: error.to_string(),
        }
    })?;
    let mut explicit = RootParticipation::default();
    let mut defaults = RootParticipation::default();
    let mut has_explicit_root = false;

    for definition in document.definitions() {
        let schema = match definition {
            CynicDefinition::Schema(schema) | CynicDefinition::SchemaExtension(schema) => {
                Some(schema)
            }
            _ => None,
        };
        if let Some(schema) = schema {
            for root in schema.root_operations() {
                has_explicit_root = true;
                match root.operation_type() {
                    OperationType::Query => explicit.query = true,
                    OperationType::Mutation => explicit.mutation = true,
                    OperationType::Subscription => explicit.subscription = true,
                }
            }
        }

        let r#type = match definition {
            CynicDefinition::Type(r#type) | CynicDefinition::TypeExtension(r#type) => Some(r#type),
            _ => None,
        };
        if let Some(r#type) = r#type.filter(|definition| definition.is_object()) {
            match r#type.name() {
                "Query" => defaults.query = true,
                "Mutation" => defaults.mutation = true,
                "Subscription" => defaults.subscription = true,
                _ => {}
            }
        }
    }

    Ok(if has_explicit_root {
        explicit
    } else {
        defaults
    })
}

fn adapt_composed_supergraph(
    sdl: &str,
    participation: &BTreeMap<&str, RootParticipation>,
) -> Result<hive_router::graphql_tools::static_graphql::schema::Document, FederationError> {
    let mut document = safe_parse_schema(sdl).map_err(|error| FederationError::SupergraphSdl {
        details: error.to_string(),
    })?;
    let graph_ids = join_graph_ids(&document);
    let root_names = [
        document.query_type_name().cloned(),
        document.mutation_type_name().cloned(),
        document.subscription_type_name().cloned(),
    ];

    let expected: [BTreeSet<String>; 3] = std::array::from_fn(|kind| {
        participation
            .iter()
            .filter(|(_, roots)| roots.as_array()[kind])
            .filter_map(|(name, _)| graph_ids.get(*name).cloned())
            .collect()
    });
    for (name, roots) in participation {
        if roots
            .as_array()
            .into_iter()
            .any(|participates| participates)
            && !graph_ids.contains_key(name)
        {
            return Err(FederationError::MissingJoinGraph {
                subgraph: (*name).to_owned(),
            });
        }
    }

    for definition in &mut document.definitions {
        let Definition::TypeDefinition(TypeDefinition::Object(object)) = definition else {
            continue;
        };
        let Some(kind) = root_names
            .iter()
            .position(|name| name.as_ref() == Some(&object.name))
        else {
            continue;
        };
        let existing = object
            .directives
            .iter()
            .filter(|directive| directive.name == "join__type")
            .filter_map(|directive| directive_enum_argument(directive, "graph"))
            .collect::<BTreeSet<_>>();

        for graph in expected[kind].difference(&existing) {
            object.directives.push(Directive {
                position: Default::default(),
                name: "join__type".to_owned(),
                arguments: vec![("graph".to_owned(), Value::Enum(graph.clone()))],
            });
        }
    }

    add_authorization_security_links(&mut document)?;

    Ok(document)
}

fn add_authorization_security_links(
    document: &mut hive_router::graphql_tools::static_graphql::schema::Document,
) -> Result<(), FederationError> {
    let has_authenticated = document_has_directive(document, "authenticated");
    let has_requires_scopes = document_has_directive(document, "requiresScopes");
    if !has_authenticated && !has_requires_scopes {
        return Ok(());
    }

    let Some(schema) = document.definitions.iter_mut().find_map(|definition| {
        if let Definition::SchemaDefinition(schema) = definition {
            Some(schema)
        } else {
            None
        }
    }) else {
        return Err(FederationError::SupergraphSdl {
            details: "composed authorization directives require a schema definition".to_owned(),
        });
    };

    for (present, url) in [
        (
            has_authenticated,
            "https://specs.apollo.dev/authenticated/v0.1",
        ),
        (
            has_requires_scopes,
            "https://specs.apollo.dev/requiresScopes/v0.1",
        ),
    ] {
        if present
            && !schema.directives.iter().any(|directive| {
                directive.name == "link" && directive_string_argument(directive, "url") == Some(url)
            })
        {
            schema.directives.push(Directive {
                position: Default::default(),
                name: "link".to_owned(),
                arguments: vec![
                    ("url".to_owned(), Value::String(url.to_owned())),
                    ("for".to_owned(), Value::Enum("SECURITY".to_owned())),
                ],
            });
        }
    }

    Ok(())
}

fn document_has_directive(
    document: &hive_router::graphql_tools::static_graphql::schema::Document,
    name: &str,
) -> bool {
    document
        .definitions
        .iter()
        .any(|definition| match definition {
            Definition::SchemaDefinition(schema) => directives_contain(&schema.directives, name),
            Definition::TypeDefinition(definition) => {
                type_definition_has_directive(definition, name)
            }
            Definition::TypeExtension(_) => false,
            Definition::DirectiveDefinition(_) => false,
        })
}

fn type_definition_has_directive(definition: &TypeDefinition, name: &str) -> bool {
    match definition {
        TypeDefinition::Scalar(definition) => directives_contain(&definition.directives, name),
        TypeDefinition::Object(definition) => {
            directives_contain(&definition.directives, name)
                || definition
                    .fields
                    .iter()
                    .any(|field| directives_contain(&field.directives, name))
        }
        TypeDefinition::Interface(definition) => {
            directives_contain(&definition.directives, name)
                || definition
                    .fields
                    .iter()
                    .any(|field| directives_contain(&field.directives, name))
        }
        TypeDefinition::Union(definition) => directives_contain(&definition.directives, name),
        TypeDefinition::Enum(definition) => directives_contain(&definition.directives, name),
        TypeDefinition::InputObject(definition) => directives_contain(&definition.directives, name),
    }
}

fn directives_contain(directives: &[Directive], name: &str) -> bool {
    directives.iter().any(|directive| directive.name == name)
}

fn join_graph_ids(
    document: &hive_router::graphql_tools::static_graphql::schema::Document,
) -> BTreeMap<&str, String> {
    let mut graph_ids = BTreeMap::new();
    for definition in &document.definitions {
        let Definition::TypeDefinition(TypeDefinition::Enum(graph_enum)) = definition else {
            continue;
        };
        if graph_enum.name != "join__Graph" {
            continue;
        }
        for value in &graph_enum.values {
            let Some(join_graph) = value
                .directives
                .iter()
                .find(|directive| directive.name == "join__graph")
            else {
                continue;
            };
            if let Some(name) = directive_string_argument(join_graph, "name") {
                graph_ids.insert(name, value.name.clone());
            }
        }
    }
    graph_ids
}

fn directive_string_argument<'a>(directive: &'a Directive, name: &str) -> Option<&'a str> {
    directive
        .arguments
        .iter()
        .find(|(argument, _)| argument == name)
        .and_then(|(_, value)| match value {
            Value::String(value) => Some(value.as_str()),
            _ => None,
        })
}

fn directive_enum_argument(directive: &Directive, name: &str) -> Option<String> {
    directive
        .arguments
        .iter()
        .find(|(argument, _)| argument == name)
        .and_then(|(_, value)| match value {
            Value::Enum(value) => Some(value.clone()),
            _ => None,
        })
}

#[cfg(test)]
mod wire_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use hive_router::query_planner::{
        ast::normalization::normalize_operation,
        graph::PlannerOverrideContext,
        state::supergraph_state::SupergraphDefinition,
        utils::{cancellation::CancellationToken, parsing::safe_parse_operation},
    };
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread,
    };

    const PRODUCTS_V1: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
        type Query { product(id: ID!): Product }
        type Mutation { renameProduct(id: ID!, name: String!): Product }
        type Product @key(fields: "id") { id: ID!, name: String! }
    "#;
    const PRODUCTS_V2: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
        type Query { product(id: ID!): Product, version: String! }
        type Mutation { renameProduct(id: ID!, name: String!): Product }
        type Product @key(fields: "id") { id: ID!, name: String! }
    "#;
    const REVIEWS: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key", "@external"])
        type Product @key(fields: "id") { id: ID! @external, reviews: [Review!]! }
        type Review { body: String! }
    "#;
    const CONFLICTING_REVIEWS: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
        type Query { review: String }
        type Product @key(fields: "id") { id: Int! }
    "#;

    fn inputs(products: &str, reviews: &str) -> Vec<CandidateSubgraph> {
        vec![
            CandidateSubgraph::new("products", "http://127.0.0.1:4101/graphql", products),
            CandidateSubgraph::new("reviews", "http://127.0.0.1:4102/graphql", reviews),
        ]
    }

    fn assert_plan_builds(graph: &ActiveGraph, operation: &str) {
        let parsed = safe_parse_operation(operation).expect("operation should parse");
        let normalized = normalize_operation(&graph.hive.planner.supergraph, &parsed, None)
            .expect("operation should normalize");
        graph
            .hive
            .planner
            .plan_from_normalized_operation(
                &normalized.operation,
                PlannerOverrideContext::default(),
                &CancellationToken::new(),
            )
            .expect("composed graph should produce an executable query plan");
    }

    #[test]
    fn composed_graph_builds_cross_subgraph_and_mutation_plans() {
        let graph = GraphStore::new(&inputs(PRODUCTS_V1, REVIEWS))
            .expect("candidate should build")
            .load();

        assert_plan_builds(
            &graph,
            r#"query { product(id: "p1") { id name reviews { body } } }"#,
        );
        assert_plan_builds(
            &graph,
            r#"mutation { renameProduct(id: "p1", name: "desk") { id name } }"#,
        );
        assert!(graph.supergraph_sdl.contains("type Query @join__type"));
        assert!(graph.supergraph_sdl.contains("type Mutation @join__type"));
        assert!(graph.warnings.is_empty());
    }

    #[test]
    fn input_order_does_not_change_the_active_fingerprint() {
        let forward = inputs(PRODUCTS_V1, REVIEWS);
        let mut reverse = forward.clone();
        reverse.reverse();

        let forward = GraphStore::new(&forward).unwrap().load();
        let reverse = GraphStore::new(&reverse).unwrap().load();

        assert_eq!(forward.fingerprint, reverse.fingerprint);
        assert_eq!(forward.supergraph_sdl, reverse.supergraph_sdl);
    }

    #[test]
    fn failed_candidate_preserves_last_known_good_graph() {
        let store = GraphStore::new(&inputs(PRODUCTS_V1, REVIEWS)).unwrap();
        let original = store.load();
        let error = match store.replace(&inputs(PRODUCTS_V1, CONFLICTING_REVIEWS)) {
            Ok(_) => panic!("semantic conflict should fail composition"),
            Err(error) => error,
        };

        assert!(matches!(error, FederationError::Composition(_)));
        assert!(Arc::ptr_eq(&original, &store.load()));
    }

    #[test]
    fn replacement_retires_only_after_the_last_old_owner_drops() {
        let store = GraphStore::new(&inputs(PRODUCTS_V1, REVIEWS)).unwrap();
        let old = store.load();
        let snapshot = old.hive.snapshot();

        let replacement = store.replace(&inputs(PRODUCTS_V2, REVIEWS)).unwrap();
        assert_ne!(old.fingerprint, replacement.fingerprint);
        assert!(!snapshot.is_retired());

        drop(old);
        assert!(snapshot.is_retired());
    }

    #[ntex::test]
    async fn pinned_subscription_broadcast_reports_bounded_lag() {
        use hive_router::{
            pipeline::active_subscriptions::{ActiveSubscriptions, SubscriptionEvent},
            tokio::sync::broadcast::error::RecvError,
        };

        let active = ActiveSubscriptions::new(2);
        let (producer, mut receiver) = active.register(None);
        for _ in 0..4 {
            assert!(producer.send(SubscriptionEvent::Error(Vec::new())));
        }
        assert!(matches!(receiver.recv().await, Err(RecvError::Lagged(2))));
        assert!(matches!(
            receiver.recv().await,
            Ok(SubscriptionEvent::Error(_))
        ));
    }

    #[test]
    fn concurrent_readers_observe_only_complete_graph_versions() {
        let store = Arc::new(GraphStore::new(&inputs(PRODUCTS_V1, REVIEWS)).unwrap());
        let old = store.load();
        let expected_new = GraphStore::new(&inputs(PRODUCTS_V2, REVIEWS))
            .unwrap()
            .load();
        let running = Arc::new(AtomicBool::new(true));
        let invalid = Arc::new(AtomicBool::new(false));
        let readers = (0..4)
            .map(|_| {
                let store = store.clone();
                let running = running.clone();
                let invalid = invalid.clone();
                let old_fingerprint = old.fingerprint.clone();
                let new_fingerprint = expected_new.fingerprint.clone();
                thread::spawn(move || {
                    while running.load(Ordering::Acquire) {
                        let graph = store.load();
                        let valid_old = graph.version == 1 && graph.fingerprint == old_fingerprint;
                        let valid_new = graph.version == 2 && graph.fingerprint == new_fingerprint;
                        if !valid_old && !valid_new {
                            invalid.store(true, Ordering::Release);
                            break;
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        let replacement = store.replace(&inputs(PRODUCTS_V2, REVIEWS)).unwrap();
        assert_eq!(replacement.version, 2);
        assert_eq!(replacement.fingerprint, expected_new.fingerprint);
        running.store(false, Ordering::Release);
        for reader in readers {
            reader.join().expect("graph reader should not panic");
        }
        assert!(!invalid.load(Ordering::Acquire));
    }

    #[test]
    fn malformed_sdl_and_duplicate_names_have_stable_error_categories() {
        let malformed = [CandidateSubgraph::new(
            "broken",
            "http://broken.test/graphql",
            "not graphql",
        )];
        assert!(matches!(
            GraphStore::new(&malformed),
            Err(FederationError::SourceSdl { .. })
        ));

        let duplicate = [
            CandidateSubgraph::new("same", "http://one.test/graphql", PRODUCTS_V1),
            CandidateSubgraph::new("same", "http://two.test/graphql", PRODUCTS_V1),
        ];
        assert!(matches!(
            GraphStore::new(&duplicate),
            Err(FederationError::DuplicateSubgraph { .. })
        ));
    }

    #[test]
    fn supported_authorization_directives_survive_composition() {
        let protected = r#"
            directive @federation__authenticated on FIELD_DEFINITION
            directive @federation__requiresScopes(scopes: [[String!]!]!) on FIELD_DEFINITION
            extend schema @link(
                url: "https://specs.apollo.dev/federation/v2.7"
                import: ["@requiresScopes"]
            )
            type Query {
                viewer: String @federation__authenticated
                reports: String @requiresScopes(scopes: [["reports:read"]])
            }
            type Subscription {
                reportChanged: String @federation__requiresScopes(scopes: [["reports:read"]])
            }
        "#;
        let graph = GraphStore::new(&[CandidateSubgraph::new(
            "protected",
            "http://protected.test/graphql",
            protected,
        )])
        .unwrap()
        .load();

        assert!(graph.supergraph_sdl.contains("@authenticated"));
        assert!(graph.supergraph_sdl.contains("@requiresScopes"));
        assert!(
            graph
                .supergraph_sdl
                .contains("https://specs.apollo.dev/authenticated/v0.1")
        );
        assert!(
            graph
                .supergraph_sdl
                .contains("https://specs.apollo.dev/requiresScopes/v0.1")
        );

        let snapshot = graph.hive.snapshot();
        let Some(SupergraphDefinition::Object(query)) =
            snapshot.planner.supergraph.definitions.get("Query")
        else {
            panic!("composed graph should contain the Query object");
        };
        assert!(!query.fields["viewer"].authenticated.is_empty());
        assert!(!query.fields["reports"].requires_scopes.is_empty());
        let Some(SupergraphDefinition::Object(subscription)) =
            snapshot.planner.supergraph.definitions.get("Subscription")
        else {
            panic!("composed graph should contain the Subscription object");
        };
        assert!(
            !subscription.fields["reportChanged"]
                .requires_scopes
                .is_empty()
        );
    }

    #[test]
    fn structural_root_adapter_supports_explicit_custom_root_names() {
        let explicit_roots = r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7")
            schema {
                query: RouterQuery
                mutation: RouterMutation
                subscription: RouterSubscription
            }
            type RouterQuery { status: String! }
            type RouterMutation { refresh: String! }
            type RouterSubscription { statusChanged: String! }
        "#;
        let graph = GraphStore::new(&[CandidateSubgraph::new(
            "explicit",
            "http://explicit.test/graphql",
            explicit_roots,
        )])
        .expect("explicit operation roots should compose")
        .load();

        assert_eq!(
            root_join_graphs(&graph, "Query"),
            BTreeSet::from(["EXPLICIT".to_owned()])
        );
        assert_eq!(
            root_join_graphs(&graph, "Mutation"),
            BTreeSet::from(["EXPLICIT".to_owned()])
        );
        assert_eq!(
            root_join_graphs(&graph, "Subscription"),
            BTreeSet::from(["EXPLICIT".to_owned()])
        );
        assert_plan_builds(&graph, "query { status }");
        assert_plan_builds(&graph, "mutation { refresh }");
    }

    #[test]
    fn structural_root_adapter_adds_each_owner_of_a_shared_query_root() {
        let first = r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7")
            type Query { first: String! }
        "#;
        let second = r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7")
            type Query { second: String! }
        "#;
        let graph = GraphStore::new(&[
            CandidateSubgraph::new("first", "http://first.test/graphql", first),
            CandidateSubgraph::new("second", "http://second.test/graphql", second),
        ])
        .expect("distinct query fields should compose")
        .load();

        assert_eq!(
            root_join_graphs(&graph, "Query"),
            BTreeSet::from(["FIRST".to_owned(), "SECOND".to_owned()])
        );
        assert_plan_builds(&graph, "query { first second }");
    }

    fn root_join_graphs(graph: &ActiveGraph, root_name: &str) -> BTreeSet<String> {
        let document = safe_parse_schema(&graph.supergraph_sdl)
            .expect("the constructed supergraph SDL should remain valid");
        document
            .definitions
            .iter()
            .find_map(|definition| {
                let Definition::TypeDefinition(TypeDefinition::Object(object)) = definition else {
                    return None;
                };
                (object.name == root_name).then(|| {
                    object
                        .directives
                        .iter()
                        .filter(|directive| directive.name == "join__type")
                        .filter_map(|directive| directive_enum_argument(directive, "graph"))
                        .collect()
                })
            })
            .expect("composed supergraph should contain its operation root")
    }
}
