//! Compares a generated operation catalogue with the SDL its schema exports.
//!
//! The comparison applies the router's authorization-binding rules: every
//! root field except the planner-owned federation fields `_entities` and
//! `_service` needs exactly one exposed catalogue operation,
//! and that operation's argument names and GraphQL types must equal the SDL
//! field's arguments. Argument descriptions are compared too, because they
//! feed the catalogue fingerprints that descriptors publish.
use graphql_orm::async_graphql::parser::{
    parse_schema,
    types::{TypeKind, TypeSystemDefinition},
};
use graphql_orm::graphql::orm::{GraphqlOperationCatalog, GraphqlOperationKind};
use std::collections::BTreeMap;

/// One root field's arguments: name to (GraphQL type, description).
pub type ArgumentContract = BTreeMap<String, (String, String)>;

/// Root coordinate to argument contract.
pub type RootContract = BTreeMap<(&'static str, String), ArgumentContract>;

fn root_label(kind: GraphqlOperationKind) -> &'static str {
    match kind {
        GraphqlOperationKind::Query => "Query",
        GraphqlOperationKind::Mutation => "Mutation",
        GraphqlOperationKind::Subscription => "Subscription",
        other => panic!("unsupported operation kind {other:?}"),
    }
}

/// The only root fields a router binds without a descriptor.
pub fn is_planner_owned_root_field(name: &str) -> bool {
    matches!(name, "_entities" | "_service")
}

/// Reads the root contract the router derives from SDL.
pub fn sdl_root_contract(sdl: &str) -> RootContract {
    let document = parse_schema(sdl).expect("exported SDL must parse");
    let mut roots = BTreeMap::from([
        ("Query".to_owned(), "Query"),
        ("Mutation".to_owned(), "Mutation"),
        ("Subscription".to_owned(), "Subscription"),
    ]);
    for definition in &document.definitions {
        // Federation SDL adds `extend schema @link(...)`, which names no roots.
        if let TypeSystemDefinition::Schema(schema) = definition
            && !schema.node.extend
        {
            roots.clear();
            if let Some(name) = &schema.node.query {
                roots.insert(name.node.to_string(), "Query");
            }
            if let Some(name) = &schema.node.mutation {
                roots.insert(name.node.to_string(), "Mutation");
            }
            if let Some(name) = &schema.node.subscription {
                roots.insert(name.node.to_string(), "Subscription");
            }
        }
    }
    let mut contract = RootContract::new();
    for definition in document.definitions {
        let TypeSystemDefinition::Type(definition) = definition else {
            continue;
        };
        let Some(root) = roots.get(definition.node.name.node.as_str()) else {
            continue;
        };
        let TypeKind::Object(object) = definition.node.kind else {
            continue;
        };
        for field in object.fields {
            let name = field.node.name.node.to_string();
            if is_planner_owned_root_field(&name) {
                continue;
            }
            let arguments = field
                .node
                .arguments
                .into_iter()
                .map(|argument| {
                    (
                        argument.node.name.node.to_string(),
                        (
                            argument.node.ty.node.to_string(),
                            argument
                                .node
                                .description
                                .map(|description| description.node)
                                .unwrap_or_default(),
                        ),
                    )
                })
                .collect();
            assert!(
                contract.insert((root, name.clone()), arguments).is_none(),
                "SDL defines root field {root}.{name} twice"
            );
        }
    }
    contract
}

/// Reads the root contract the generated catalogue declares.
pub fn catalog_root_contract(catalog: &GraphqlOperationCatalog) -> RootContract {
    let mut contract = RootContract::new();
    for operation in catalog.exposed_operations() {
        let key = (
            root_label(operation.kind()),
            operation.field_name().to_owned(),
        );
        let arguments = operation
            .arguments()
            .iter()
            .map(|argument| {
                (
                    argument.graphql_name().to_owned(),
                    (
                        argument.graphql_type().to_owned(),
                        argument.description().to_owned(),
                    ),
                )
            })
            .collect::<ArgumentContract>();
        assert_eq!(
            arguments.len(),
            operation.arguments().len(),
            "catalogue repeats an argument for {}.{}",
            key.0,
            key.1
        );
        assert!(
            contract.insert(key.clone(), arguments).is_none(),
            "catalogue exposes {}.{} twice",
            key.0,
            key.1
        );
    }
    contract
}

/// Fails with a precise drift report when catalogue and SDL disagree.
pub fn assert_catalog_matches_sdl(catalog: &GraphqlOperationCatalog, sdl: &str) {
    let expected = sdl_root_contract(sdl);
    let actual = catalog_root_contract(catalog);
    let mut drift = Vec::new();
    for key in expected.keys() {
        // Older routers drop underscore root fields from the contract they
        // bind, so a described underscore field would be refused there.
        if key.1.starts_with('_') {
            drift.push(format!(
                "{}.{} is an underscore root field, which routers disagree on binding",
                key.0, key.1
            ));
        }
    }
    for (key, sdl_arguments) in &expected {
        match actual.get(key) {
            None => drift.push(format!("{}.{} has no catalogue entry", key.0, key.1)),
            Some(catalog_arguments) if catalog_arguments != sdl_arguments => drift.push(format!(
                "{}.{} arguments differ\n  sdl:       {sdl_arguments:?}\n  catalogue: {catalog_arguments:?}",
                key.0, key.1
            )),
            Some(_) => {}
        }
    }
    for key in actual.keys() {
        if !expected.contains_key(key) {
            drift.push(format!(
                "catalogue exposes {}.{}, which the SDL does not define",
                key.0, key.1
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "operation catalogue drifted from the exported SDL:\n{}",
        drift.join("\n")
    );
}

/// Applies the router's descriptor contract to the protocol export.
#[cfg(feature = "router-protocol")]
pub fn assert_router_protocol_matches_sdl(catalog: &GraphqlOperationCatalog, sdl: &str) {
    use graphql_orm_router_protocol::RootOperationType;

    let expected = sdl_root_contract(sdl)
        .into_iter()
        .map(|(key, arguments)| {
            (
                key,
                arguments
                    .into_iter()
                    .map(|(name, (graphql_type, _))| (name, graphql_type))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeMap::new();
    for operation in catalog
        .router_protocol_operations()
        .expect("generated catalogue exports protocol operations")
    {
        let root = match operation.root_type {
            RootOperationType::Query => "Query",
            RootOperationType::Mutation => "Mutation",
            RootOperationType::Subscription => "Subscription",
        };
        for argument in &operation.arguments {
            assert_eq!(
                argument.required,
                argument.graphql_type.ends_with('!'),
                "{root}.{} argument {} requiredness disagrees with its type",
                operation.field_name,
                argument.name
            );
        }
        let arguments = operation
            .arguments
            .into_iter()
            .map(|argument| (argument.name, argument.graphql_type))
            .collect::<BTreeMap<_, _>>();
        assert!(
            actual
                .insert((root, operation.field_name), arguments)
                .is_none(),
            "protocol export is ambiguous"
        );
    }
    assert_eq!(
        actual, expected,
        "router protocol operations must bind every SDL root field exactly"
    );
}
