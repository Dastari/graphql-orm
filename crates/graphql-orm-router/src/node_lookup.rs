//! Router-hosted `Node` subgraph for global `NodeId` lookup.
//!
//! Subgraphs that opt in expose `NodeId: ID!` with a resolvable
//! `@key(fields: "NodeId")` on each entity, and never declare `interface Node`.
//! The router finds those types in the candidate SDL, contributes a generated
//! subgraph that owns the entity interface, and answers that subgraph's fetches
//! in-process:
//!
//! ```graphql
//! type Query { node(NodeId: ID!): Node  nodes(NodeIds: [ID!]!): [Node]! }
//! interface Node @key(fields: "NodeId") { NodeId: ID! }
//! type Zone implements Node @key(fields: "NodeId", resolvable: false) { NodeId: ID! }
//! ```
//!
//! `node` decodes only the typename and returns `{ __typename, NodeId }`; the
//! planner fetches every other field from the owning subgraph through its
//! `NodeId` key, batched into one `_entities` fetch per subgraph. The router
//! authorizes nothing here: the owner's entity resolver decides whether a row is
//! visible, and an unreadable row is null exactly like a missing one.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use base64::Engine as _;
use cynic_parser::type_system::{Definition, TypeDefinition};
use hive_router::graphql_tools::parser::query::{
    Definition as QueryDefinition, OperationDefinition, Selection, SelectionSet, Value, parse_query,
};
use serde_json::{Map, Value as Json, json};

use crate::federation::{CandidateSubgraph, FederationError};

/// Reserved name of the router-hosted subgraph. A configured subgraph may not use it.
pub const NODE_SUBGRAPH_NAME: &str = "graphql-orm-node";

/// Never contacted: the router answers this subgraph's fetches in-process.
pub(crate) const NODE_SUBGRAPH_ENDPOINT: &str = "http://graphql-orm-node.invalid/graphql";

/// Default upper bound on the `NodeIds` list of `Query.nodes`.
pub const DEFAULT_MAX_NODES: usize = 100;

const NODE_ID: &str = "NodeId";

/// Opt-in router-hosted `node(NodeId)` and `nodes(NodeIds)` lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeLookupConfig {
    pub(crate) max_nodes: usize,
}

impl Default for NodeLookupConfig {
    fn default() -> Self {
        Self {
            max_nodes: DEFAULT_MAX_NODES,
        }
    }
}

impl NodeLookupConfig {
    /// Enables the lookup with the default `nodes` bound, [`DEFAULT_MAX_NODES`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of `NodeIds` one `nodes` field accepts.
    pub fn with_max_nodes(mut self, max_nodes: usize) -> Self {
        self.max_nodes = max_nodes;
        self
    }

    /// Returns the maximum number of `NodeIds` one `nodes` field accepts.
    pub fn max_nodes(&self) -> usize {
        self.max_nodes
    }
}

/// Router-owned authorization metadata for the Node subgraph's root fields.
pub(crate) fn router_operations() -> [graphql_orm_router_protocol::OperationDescriptor; 2] {
    use graphql_orm_router_protocol::{
        ArgumentDescriptor, AuthorizationRequirement, OperationDescriptor, RootOperationType,
    };
    let operation = |field_name: &str, argument: &str, graphql_type: &str| OperationDescriptor {
        root_type: RootOperationType::Query,
        field_name: field_name.to_owned(),
        arguments: vec![ArgumentDescriptor {
            name: argument.to_owned(),
            graphql_type: graphql_type.to_owned(),
            required: true,
        }],
        authorization: AuthorizationRequirement::Authenticated,
    };
    [
        operation("node", NODE_ID, "ID!"),
        operation("nodes", "NodeIds", "[ID!]!"),
    ]
}

/// The `Node` implementations of one composed graph.
#[derive(Debug)]
pub(crate) struct NodeLookup {
    pub(crate) types: BTreeSet<String>,
    pub(crate) max_nodes: usize,
}

/// Find every object type with `NodeId: ID!` and a resolvable `@key(fields: "NodeId")`.
///
/// A type that declares the key but lacks a non-null `ID` field named `NodeId`
/// fails the candidate rather than being silently left out of `Node`.
pub(crate) fn node_implementers(
    inputs: &[CandidateSubgraph],
) -> Result<BTreeSet<String>, FederationError> {
    let mut keyed = BTreeMap::<String, &str>::new();
    let mut fields = BTreeMap::<String, Vec<String>>::new();
    for input in inputs {
        let document = cynic_parser::parse_type_system_document(&input.sdl).map_err(|error| {
            FederationError::SourceSdl {
                subgraph: input.name.clone(),
                details: error.to_string(),
            }
        })?;
        for definition in document.definitions() {
            let (Definition::Type(definition) | Definition::TypeExtension(definition)) = definition
            else {
                continue;
            };
            let TypeDefinition::Object(object) = definition else {
                continue;
            };
            let name = object.name().to_owned();
            if let Some(field) = object.fields().find(|field| field.name() == NODE_ID) {
                fields
                    .entry(name.clone())
                    .or_default()
                    .push(field.ty().to_string());
            }
            let resolvable_node_key = object.directives().any(|directive| {
                matches!(directive.name(), "key" | "federation__key")
                    && directive.arguments().any(|argument| {
                        argument.name() == "fields"
                            && argument.value().as_str().map(str::trim) == Some(NODE_ID)
                    })
                    && !directive.arguments().any(|argument| {
                        argument.name() == "resolvable" && argument.value().as_bool() == Some(false)
                    })
            });
            if resolvable_node_key {
                keyed.insert(name, input.name.as_str());
            }
        }
    }
    for (name, subgraph) in &keyed {
        let types = fields.get(name).map(Vec::as_slice).unwrap_or_default();
        if types.is_empty() || types.iter().any(|ty| ty != "ID!") {
            return Err(FederationError::NodeLookup {
                details: format!(
                    "subgraph `{subgraph}` declares @key(fields: \"NodeId\") on `{name}` without a `NodeId: ID!` field"
                ),
            });
        }
    }
    Ok(keyed.into_keys().collect())
}

/// SDL of the router-hosted subgraph for `types`.
pub(crate) fn node_subgraph_sdl(types: &BTreeSet<String>) -> String {
    let mut sdl = String::from(
        "extend schema @link(url: \"https://specs.apollo.dev/federation/v2.7\", import: [\"@key\"])\n\
         type Query {\n  node(NodeId: ID!): Node\n  nodes(NodeIds: [ID!]!): [Node]!\n}\n\
         interface Node @key(fields: \"NodeId\") {\n  NodeId: ID!\n}\n",
    );
    for name in types {
        sdl.push_str(&format!(
            "type {name} implements Node @key(fields: \"NodeId\", resolvable: false) {{\n  NodeId: ID!\n}}\n"
        ));
    }
    sdl
}

pub(crate) fn node_candidate(types: &BTreeSet<String>) -> CandidateSubgraph {
    CandidateSubgraph::new(
        NODE_SUBGRAPH_NAME,
        NODE_SUBGRAPH_ENDPOINT,
        &node_subgraph_sdl(types),
    )
}

/// The typename a `NodeId` encodes, or `None` if it is not an encoding of a
/// JSON array whose first element is a string.
///
/// The router reads only the typename. The owning subgraph decodes and checks
/// the complete identity, so the router never needs to understand key parts.
pub(crate) fn node_id_typename(node_id: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(node_id)
        .ok()?;
    match serde_json::from_slice::<Json>(&bytes).ok()? {
        Json::Array(parts) if parts.len() >= 2 => parts.first()?.as_str().map(str::to_owned),
        _ => None,
    }
}

/// Answer one GraphQL request addressed to the router-hosted subgraph.
pub(crate) fn execute(lookup: Option<&Arc<NodeLookup>>, body: &[u8]) -> Json {
    match execute_request(lookup, body) {
        Ok(data) => json!({ "data": data }),
        Err(error) => json!({
            "data": null,
            "errors": [{ "message": error.message, "extensions": { "code": error.code } }],
        }),
    }
}

struct NodeError {
    message: String,
    code: &'static str,
}

impl NodeError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: "INVALID_INPUT",
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: "GRAPHQL_VALIDATION_FAILED",
        }
    }
}

struct Request<'a> {
    lookup: &'a NodeLookup,
    variables: Map<String, Json>,
    fragments: BTreeMap<String, &'a SelectionSet<'a, String>>,
}

fn execute_request(lookup: Option<&Arc<NodeLookup>>, body: &[u8]) -> Result<Json, NodeError> {
    let Some(lookup) = lookup else {
        return Err(NodeError::unsupported("node lookup is not enabled"));
    };
    let request: Json = serde_json::from_slice(body)
        .map_err(|_| NodeError::unsupported("malformed GraphQL request"))?;
    let query = request
        .get("query")
        .and_then(Json::as_str)
        .ok_or_else(|| NodeError::unsupported("GraphQL request has no query"))?;
    let variables = match request.get("variables") {
        Some(Json::Object(variables)) => variables.clone(),
        None | Some(Json::Null) => Map::new(),
        Some(_) => {
            return Err(NodeError::unsupported(
                "GraphQL variables must be an object",
            ));
        }
    };
    let operation_name = request.get("operationName").and_then(Json::as_str);
    let document = parse_query::<String>(query)
        .map_err(|_| NodeError::unsupported("malformed GraphQL query"))?;
    let mut fragments = BTreeMap::new();
    let mut operations = Vec::new();
    for definition in &document.definitions {
        match definition {
            QueryDefinition::Fragment(fragment) => {
                fragments.insert(fragment.name.clone(), &fragment.selection_set);
            }
            QueryDefinition::Operation(operation) => operations.push(operation),
        }
    }
    let selection = operations
        .iter()
        .find_map(|operation| match operation {
            OperationDefinition::SelectionSet(selection)
                if operation_name.is_none() && operations.len() == 1 =>
            {
                Some(selection)
            }
            OperationDefinition::Query(query)
                if operation_name.is_none_or(|name| query.name.as_deref() == Some(name))
                    && (operation_name.is_some() || operations.len() == 1) =>
            {
                Some(&query.selection_set)
            }
            _ => None,
        })
        .ok_or_else(|| NodeError::unsupported("the node subgraph answers one query operation"))?;
    let request = Request {
        lookup,
        variables,
        fragments,
    };
    let mut data = Map::new();
    request.root(selection, &mut data)?;
    Ok(Json::Object(data))
}

impl<'a> Request<'a> {
    fn root(
        &self,
        selection: &'a SelectionSet<'a, String>,
        data: &mut Map<String, Json>,
    ) -> Result<(), NodeError> {
        for item in &selection.items {
            match item {
                Selection::Field(field) => {
                    if !self.included(&field.directives)? {
                        continue;
                    }
                    let key = field.alias.clone().unwrap_or_else(|| field.name.clone());
                    let value = match field.name.as_str() {
                        "__typename" => Json::String("Query".to_owned()),
                        "node" => {
                            let node_id = self.argument(&field.arguments, NODE_ID)?;
                            let Json::String(node_id) = node_id else {
                                return Err(NodeError::invalid("NodeId must be an ID"));
                            };
                            self.node(&node_id, &field.selection_set)?
                        }
                        "nodes" => {
                            let Json::Array(node_ids) =
                                self.argument(&field.arguments, "NodeIds")?
                            else {
                                return Err(NodeError::invalid("NodeIds must be a list of IDs"));
                            };
                            if node_ids.len() > self.lookup.max_nodes {
                                return Err(NodeError::invalid(format!(
                                    "nodes accepts at most {} NodeIds",
                                    self.lookup.max_nodes
                                )));
                            }
                            let mut nodes = Vec::with_capacity(node_ids.len());
                            for node_id in node_ids {
                                let Json::String(node_id) = node_id else {
                                    return Err(NodeError::invalid(
                                        "NodeIds must be a list of IDs",
                                    ));
                                };
                                nodes.push(self.node(&node_id, &field.selection_set)?);
                            }
                            Json::Array(nodes)
                        }
                        other => {
                            return Err(NodeError::unsupported(format!(
                                "the node subgraph has no root field `{other}`"
                            )));
                        }
                    };
                    data.insert(key, value);
                }
                Selection::InlineFragment(fragment) => {
                    if self.included(&fragment.directives)? {
                        self.root(&fragment.selection_set, data)?;
                    }
                }
                Selection::FragmentSpread(spread) => {
                    if self.included(&spread.directives)? {
                        let fragment = self.fragment(&spread.fragment_name)?;
                        self.root(fragment, data)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `{ __typename, NodeId }` for a composed `Node` type, otherwise null.
    fn node(
        &self,
        node_id: &str,
        selection: &'a SelectionSet<'a, String>,
    ) -> Result<Json, NodeError> {
        let Some(typename) =
            node_id_typename(node_id).filter(|name| self.lookup.types.contains(name))
        else {
            return Ok(Json::Null);
        };
        let mut object = Map::new();
        self.node_fields(&typename, node_id, selection, &mut object)?;
        Ok(Json::Object(object))
    }

    fn node_fields(
        &self,
        typename: &str,
        node_id: &str,
        selection: &'a SelectionSet<'a, String>,
        object: &mut Map<String, Json>,
    ) -> Result<(), NodeError> {
        for item in &selection.items {
            match item {
                Selection::Field(field) => {
                    if !self.included(&field.directives)? {
                        continue;
                    }
                    let key = field.alias.clone().unwrap_or_else(|| field.name.clone());
                    let value = match field.name.as_str() {
                        "__typename" => Json::String(typename.to_owned()),
                        NODE_ID => Json::String(node_id.to_owned()),
                        other => {
                            return Err(NodeError::unsupported(format!(
                                "the node subgraph resolves only __typename and NodeId, not `{other}`"
                            )));
                        }
                    };
                    object.insert(key, value);
                }
                Selection::InlineFragment(fragment) => {
                    let applies = fragment.type_condition.as_ref().is_none_or(|condition| {
                        let condition: &String = condition.as_ref();
                        condition == "Node" || condition == typename
                    });
                    if applies && self.included(&fragment.directives)? {
                        self.node_fields(typename, node_id, &fragment.selection_set, object)?;
                    }
                }
                Selection::FragmentSpread(spread) => {
                    if self.included(&spread.directives)? {
                        let fragment = self.fragment(&spread.fragment_name)?;
                        self.node_fields(typename, node_id, fragment, object)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn fragment(&self, name: &str) -> Result<&'a SelectionSet<'a, String>, NodeError> {
        self.fragments
            .get(name)
            .copied()
            .ok_or_else(|| NodeError::unsupported(format!("unknown fragment `{name}`")))
    }

    fn argument(
        &self,
        arguments: &[(String, Value<'a, String>)],
        name: &str,
    ) -> Result<Json, NodeError> {
        let value = arguments
            .iter()
            .find(|(argument, _)| argument == name)
            .map(|(_, value)| value)
            .ok_or_else(|| NodeError::invalid(format!("argument `{name}` is required")))?;
        self.value(value)
    }

    fn value(&self, value: &Value<'a, String>) -> Result<Json, NodeError> {
        Ok(match value {
            Value::Variable(name) => {
                self.variables.get(name).cloned().ok_or_else(|| {
                    NodeError::invalid(format!("variable `{name}` is not supplied"))
                })?
            }
            Value::String(value) => Json::String(value.clone()),
            Value::Boolean(value) => Json::Bool(*value),
            Value::List(values) => Json::Array(
                values
                    .iter()
                    .map(|value| self.value(value))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Null => Json::Null,
            _ => return Err(NodeError::invalid("unsupported argument value")),
        })
    }

    /// Evaluate `@skip` and `@include`; any other directive is unsupported.
    fn included(
        &self,
        directives: &[hive_router::graphql_tools::parser::query::Directive<'a, String>],
    ) -> Result<bool, NodeError> {
        let mut included = true;
        for directive in directives {
            let condition = match self.argument(&directive.arguments, "if")? {
                Json::Bool(condition) => condition,
                _ => return Err(NodeError::invalid("`if` must be a Boolean")),
            };
            match directive.name.as_str() {
                "skip" => included &= !condition,
                "include" => included &= condition,
                other => {
                    return Err(NodeError::unsupported(format!(
                        "the node subgraph does not support @{other}"
                    )));
                }
            }
        }
        Ok(included)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(types: &[&str], max_nodes: usize) -> Arc<NodeLookup> {
        Arc::new(NodeLookup {
            types: types.iter().map(|name| (*name).to_owned()).collect(),
            max_nodes,
        })
    }

    fn run(lookup: &Arc<NodeLookup>, request: Json) -> Json {
        execute(Some(lookup), request.to_string().as_bytes())
    }

    /// Typenames of the vectors shared with graphql-orm-client and graphql-orm.
    #[test]
    fn shared_vectors_decode_to_their_typename() {
        for (node_id, typename) in [
            (
                "WyJGYW1lRGV2aWNlIiwiOWIyYzBmMmUtMWQxYS00ZjZlLTljMWUtMGExYjJjM2Q0ZTVmIl0",
                "FameDevice",
            ),
            (
                "WyJGYW1lRW5yb2xsbWVudFRlbmFudFJlY29yZCIsImFjbWU6c3ViLnRlYW1fMSJd",
                "FameEnrollmentTenantRecord",
            ),
            ("WyJNYW5hZ2VkU2xvdCIsIm5vcnRoIiwiYSJd", "ManagedSlot"),
            ("WyJUaWNrZXQiLDQyXQ", "Ticket"),
            ("WyJOb3RlIiwiw7xuw69jw7hkw6kg4pyTIl0", "Note"),
            ("WyJRdW90ZSIsInNheSBcImhpXCJcXG4iXQ", "Quote"),
        ] {
            assert_eq!(node_id_typename(node_id).as_deref(), Some(typename));
        }
        for malformed in [
            "",
            "not base64!",
            "WyJUaWNrZXQiLDQyXQ==",
            "eyJhIjoxfQ",
            "WyJUaWNrZXQiXQ",
            "WzQyLDQyXQ",
        ] {
            assert_eq!(node_id_typename(malformed), None, "{malformed}");
        }
    }

    #[test]
    fn node_and_nodes_answer_typename_and_node_id_only() {
        let lookup = lookup(&["Ticket", "Note"], 3);
        let response = run(
            &lookup,
            json!({
                "query": "query($ids:[ID!]!){nodes(NodeIds: $ids){__typename NodeId} one: node(NodeId: \"WyJOb3RlIiwiw7xuw69jw7hkw6kg4pyTIl0\"){... on Node { id: NodeId } __typename}}",
                "variables": {"ids": ["WyJUaWNrZXQiLDQyXQ", "WyJNYW5hZ2VkU2xvdCIsIm5vcnRoIiwiYSJd", "garbage"]},
            }),
        );
        assert_eq!(
            response,
            json!({"data": {
                "nodes": [{"__typename": "Ticket", "NodeId": "WyJUaWNrZXQiLDQyXQ"}, null, null],
                "one": {"id": "WyJOb3RlIiwiw7xuw69jw7hkw6kg4pyTIl0", "__typename": "Note"},
            }})
        );
    }

    #[test]
    fn nodes_over_the_bound_is_refused() {
        let lookup = lookup(&["Ticket"], 2);
        let response = run(
            &lookup,
            json!({"query": "{ nodes(NodeIds: [\"a\", \"b\", \"c\"]) { NodeId } }"}),
        );
        assert_eq!(response["data"], Json::Null);
        assert_eq!(response["errors"][0]["extensions"]["code"], "INVALID_INPUT");
    }

    #[test]
    fn other_fields_and_directives_are_refused() {
        let lookup = lookup(&["Ticket"], 2);
        let other = run(
            &lookup,
            json!({"query": "{ node(NodeId: \"WyJUaWNrZXQiLDQyXQ\") { title } }"}),
        );
        assert_eq!(
            other["errors"][0]["extensions"]["code"],
            "GRAPHQL_VALIDATION_FAILED"
        );
        let skipped = run(
            &lookup,
            json!({"query": "query($s: Boolean!){ node(NodeId: \"WyJUaWNrZXQiLDQyXQ\") @skip(if: $s) { NodeId } }", "variables": {"s": true}}),
        );
        assert_eq!(skipped, json!({"data": {}}));
    }

    #[test]
    fn implementers_need_a_resolvable_node_id_key_and_field() {
        let zones = CandidateSubgraph::new(
            "zones",
            "http://zones.test/graphql",
            r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
            type Query { zones: [Zone!]! }
            type Zone @key(fields: "id") @key(fields: "NodeId") { id: String! NodeId: ID! }
            type Stub @key(fields: "NodeId", resolvable: false) { NodeId: ID! }
            type Plain @key(fields: "id") { id: String! NodeId: ID! }
            "#,
        );
        assert_eq!(
            node_implementers(std::slice::from_ref(&zones)).unwrap(),
            BTreeSet::from(["Zone".to_owned()])
        );
        let broken = CandidateSubgraph::new(
            "broken",
            "http://broken.test/graphql",
            r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
            type Query { broken: Broken }
            type Broken @key(fields: "NodeId") { NodeId: String! }
            "#,
        );
        assert!(matches!(
            node_implementers(&[zones, broken]),
            Err(FederationError::NodeLookup { .. })
        ));
    }
}
