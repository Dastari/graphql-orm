use graphql_orm::async_graphql::parser::{
    parse_schema,
    types::{TypeKind, TypeSystemDefinition},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct ParsedFederationSchema {
    pub query: String,
    pub mutation: Option<String>,
    pub subscription: Option<String>,
    pub objects: BTreeMap<String, BTreeSet<String>>,
}

impl ParsedFederationSchema {
    pub fn parse(sdl: &str) -> Self {
        let document = parse_schema(sdl).expect("federation SDL must parse as GraphQL SDL");
        let mut objects = BTreeMap::new();
        let mut explicit_roots = None;

        for definition in document.definitions {
            match definition {
                TypeSystemDefinition::Schema(definition) if !definition.node.extend => {
                    assert!(
                        explicit_roots.is_none(),
                        "SDL must not define multiple schemas"
                    );
                    explicit_roots = Some((
                        definition.node.query.map(|name| name.node.to_string()),
                        definition.node.mutation.map(|name| name.node.to_string()),
                        definition
                            .node
                            .subscription
                            .map(|name| name.node.to_string()),
                    ));
                }
                TypeSystemDefinition::Type(definition) => {
                    if let TypeKind::Object(object) = definition.node.kind {
                        objects.insert(
                            definition.node.name.node.to_string(),
                            object
                                .fields
                                .into_iter()
                                .map(|field| field.node.name.node.to_string())
                                .collect(),
                        );
                    }
                }
                _ => {}
            }
        }

        let (query, mutation, subscription) = explicit_roots.unwrap_or_else(|| {
            (
                objects.contains_key("Query").then(|| "Query".to_string()),
                objects
                    .contains_key("Mutation")
                    .then(|| "Mutation".to_string()),
                objects
                    .contains_key("Subscription")
                    .then(|| "Subscription".to_string()),
            )
        });
        let query = query.expect("SDL must declare or conventionally define a query root");

        for root in [Some(&query), mutation.as_ref(), subscription.as_ref()]
            .into_iter()
            .flatten()
        {
            assert!(
                objects.contains_key(root),
                "declared operation root `{root}` must exist as an object"
            );
        }

        Self {
            query,
            mutation,
            subscription,
            objects,
        }
    }

    pub fn query_fields(&self) -> &BTreeSet<String> {
        self.objects
            .get(&self.query)
            .expect("validated query root must exist")
    }
}
