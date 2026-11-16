//! Composed `@authenticated` and `@requiresScopes` enforcement at every depth.
//!
//! The descriptor contract in the parent module authorizes root fields from
//! protocol metadata. This module evaluates the Federation authorization
//! directives that composition kept in the supergraph, on every selected field
//! of the normalized operation: root fields, nested object fields, fields
//! reached through interfaces and unions, inline fragments, and fragment
//! spreads (Hive's normalization expands spreads into inline fragments before
//! the router sees the operation). Aliases do not change the field that is
//! checked; they only name the response path in the refusal.
//!
//! Semantics, applied before any downstream request opens:
//!
//! - A field `P.f` returning named type `R` requires the rule on `P`, the rule
//!   on `P.f`, and the rule on `R`. All three must hold (AND across levels).
//! - A field selected on an interface `P` also requires, for every object type
//!   `T` that implements `P`, the rules on `T`, on `T.f`, and on `T.f`'s return
//!   type, because the field may be resolved on any of them. This fails closed
//!   whether or not composition copied implementation rules onto the
//!   interface.
//! - An inline fragment on type `T` requires the rule on `T`.
//! - `@requiresScopes(scopes: [[a, b], [c]])` holds when every scope of at
//!   least one inner set is granted (OR of AND sets). It also requires
//!   authentication. Several scope requirements on one selection must all
//!   hold.
//! - `__typename` is answered without a subgraph and is never refused.
//!   `__schema` and `__type` are governed by the router's introspection
//!   policy, before this check.
//!
//! A refused selection refuses the whole operation, like a refused root field:
//! the router never trims a selection and never returns partial data for it.
//! Each refusal names only its response path. The router may deny early but
//! never grants: subgraph guards and policies stay authoritative.

use std::collections::{BTreeSet, HashMap, HashSet};

use hive_router::{
    query_planner::{
        ast::{selection_item::SelectionItem, value::Value},
        state::supergraph_state::{SupergraphDefinition, SupergraphState},
    },
    sonic_rs::Value as JsonValue,
};

use super::{
    AuthenticatedPrincipal, AuthorizationDenial, DirectiveInclusion, ScopeMatcher,
    VariableResolution, directive_inclusion,
};
use crate::{RouterError, RouterErrorKind};

/// Most refusals reported for one operation. The operation is refused as soon
/// as one selection is; the bound keeps the response and the walk small for a
/// large selection that is refused in many places.
pub(super) const MAX_REPORTED_DENIALS: usize = 32;

/// One composed requirement. Every scope entry must hold; one entry holds when
/// all scopes of any one of its sets are granted.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct Requirement {
    authenticated: bool,
    scopes: Vec<Vec<Vec<String>>>,
}

impl Requirement {
    fn is_empty(&self) -> bool {
        !self.authenticated && self.scopes.is_empty()
    }

    fn and(&mut self, other: &Requirement) {
        self.authenticated |= other.authenticated;
        for alternatives in &other.scopes {
            if !self.scopes.contains(alternatives) {
                self.scopes.push(alternatives.clone());
            }
        }
    }

    fn evaluate(
        &self,
        principal: Option<&AuthenticatedPrincipal>,
        matcher: &dyn ScopeMatcher,
    ) -> Option<(&'static str, &'static str)> {
        if self.is_empty() {
            return None;
        }
        let Some(principal) = principal else {
            return Some(("UNAUTHENTICATED", "authentication is required"));
        };
        let granted = |required: &String| {
            principal
                .scopes()
                .iter()
                .any(|granted| matcher.matches(granted, required))
        };
        let satisfied = self.scopes.iter().all(|alternatives| {
            alternatives
                .iter()
                .any(|scope_set| scope_set.iter().all(granted))
        });
        (!satisfied).then_some(("FORBIDDEN", "required scope is missing"))
    }
}

/// Composed directive rules of one active graph, indexed for the walk.
#[derive(Clone, Debug, Default)]
pub(super) struct NestedAuthorization {
    /// Requirement index for each `(parent type, field)` with a rule.
    fields: HashMap<String, HashMap<String, usize>>,
    /// Requirement index for each fragment type condition with a rule.
    fragments: HashMap<String, usize>,
    requirements: Vec<Requirement>,
    /// Named return type of every field of every object and interface.
    return_types: HashMap<String, HashMap<String, String>>,
    /// Composite types whose selection sets can contain a rule, directly or
    /// through any field, fragment, or possible type below them.
    guarded: HashSet<String>,
}

impl NestedAuthorization {
    pub(super) fn build(state: &SupergraphState) -> Result<Self, RouterError> {
        let mut type_rules = HashMap::new();
        let mut field_rules = HashMap::<String, HashMap<String, Requirement>>::new();
        let mut return_types = HashMap::<String, HashMap<String, String>>::new();
        for definition in state.definitions.values() {
            let (name, authenticated, requires_scopes) = match definition {
                SupergraphDefinition::Object(object) => {
                    (&object.name, &object.authenticated, &object.requires_scopes)
                }
                SupergraphDefinition::Interface(interface) => (
                    &interface.name,
                    &interface.authenticated,
                    &interface.requires_scopes,
                ),
                SupergraphDefinition::Scalar(scalar) => {
                    (&scalar.name, &scalar.authenticated, &scalar.requires_scopes)
                }
                SupergraphDefinition::Enum(enumeration) => (
                    &enumeration.name,
                    &enumeration.authenticated,
                    &enumeration.requires_scopes,
                ),
                SupergraphDefinition::Union(_) | SupergraphDefinition::InputObject(_) => continue,
            };
            let rule = requirement(
                !authenticated.is_empty(),
                requires_scopes.iter().map(|directive| &directive.scopes),
                name,
            )?;
            if !rule.is_empty() {
                type_rules.insert(name.clone(), rule);
            }
            if !matches!(
                definition,
                SupergraphDefinition::Object(_) | SupergraphDefinition::Interface(_)
            ) {
                continue;
            }
            for (field_name, field) in definition.fields() {
                return_types
                    .entry(name.clone())
                    .or_default()
                    .insert(field_name.clone(), field.field_type.inner_type().to_owned());
                let rule = requirement(
                    !field.authenticated.is_empty(),
                    field
                        .requires_scopes
                        .iter()
                        .map(|directive| &directive.scopes),
                    &format!("{name}.{field_name}"),
                )?;
                if !rule.is_empty() {
                    field_rules
                        .entry(name.clone())
                        .or_default()
                        .insert(field_name.clone(), rule);
                }
            }
        }

        let possible_types = possible_types(state);
        let type_rule = |name: &str| type_rules.get(name).cloned().unwrap_or_default();
        let field_rule = |parent: &str, field: &str| {
            field_rules
                .get(parent)
                .and_then(|fields| fields.get(field))
                .cloned()
                .unwrap_or_default()
        };
        let mut nested = Self {
            return_types,
            ..Self::default()
        };
        let mut interned = HashMap::<Requirement, usize>::new();
        let mut intern = |rule: Requirement, nested: &mut Self| -> usize {
            *interned.entry(rule.clone()).or_insert_with(|| {
                nested.requirements.push(rule);
                nested.requirements.len() - 1
            })
        };

        let parents = nested
            .return_types
            .iter()
            .map(|(parent, fields)| (parent.clone(), fields.clone()))
            .collect::<Vec<_>>();
        for (parent, fields) in parents {
            for (field, returned) in fields {
                let mut rule = type_rule(&parent);
                rule.and(&field_rule(&parent, &field));
                rule.and(&type_rule(&returned));
                if matches!(
                    state.definitions.get(&parent),
                    Some(SupergraphDefinition::Interface(_))
                ) {
                    for implementation in possible_types.get(&parent).into_iter().flatten() {
                        rule.and(&type_rule(implementation));
                        rule.and(&field_rule(implementation, &field));
                        if let Some(returned) = nested
                            .return_types
                            .get(implementation)
                            .and_then(|fields| fields.get(&field))
                        {
                            rule.and(&type_rule(returned));
                        }
                    }
                }
                if !rule.is_empty() {
                    let index = intern(rule, &mut nested);
                    nested
                        .fields
                        .entry(parent.clone())
                        .or_default()
                        .insert(field, index);
                }
            }
        }
        for (name, rule) in &type_rules {
            if matches!(
                state.definitions.get(name),
                Some(
                    SupergraphDefinition::Object(_)
                        | SupergraphDefinition::Interface(_)
                        | SupergraphDefinition::Union(_)
                )
            ) {
                let index = intern(rule.clone(), &mut nested);
                nested.fragments.insert(name.clone(), index);
            }
        }
        nested.guarded = guarded_types(state, &nested, &possible_types);
        Ok(nested)
    }

    /// Whether the graph carries no composed directive rule at all.
    pub(super) fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }

    /// Walks the included selections and returns the refusals, at most
    /// [`MAX_REPORTED_DENIALS`] of them. A selection set's own refusals come
    /// before those below it, and sibling subtrees follow document order.
    pub(super) fn authorize(
        &self,
        root_type: &str,
        selections: &[SelectionItem],
        variables: &HashMap<String, JsonValue>,
        variable_resolution: VariableResolution,
        principal: Option<&AuthenticatedPrincipal>,
        matcher: &dyn ScopeMatcher,
    ) -> Vec<AuthorizationDenial> {
        let mut denials = Vec::new();
        if self.is_empty() || !self.guarded.contains(root_type) {
            return denials;
        }
        let mut walk = Walk {
            nested: self,
            variables,
            variable_resolution,
            principal,
            matcher,
            decisions: vec![None; self.requirements.len()],
            path_nodes: Vec::new(),
            reported: HashSet::new(),
        };
        // An explicit stack keeps the walk iterative, so an operation's depth
        // never becomes the router's stack depth.
        let mut stack = vec![Frame {
            selections,
            parent: root_type,
            path: None,
            root: true,
        }];
        while let Some(frame) = stack.pop() {
            // Children are pushed in reverse so that sibling subtrees are
            // walked in document order.
            let mut children = Vec::new();
            for selection in frame.selections {
                match selection {
                    SelectionItem::Field(field) => {
                        if walk.excluded(field.skip_if.as_deref(), field.include_if.as_deref()) {
                            continue;
                        }
                        if field.name == "__typename" {
                            continue;
                        }
                        let key = field.alias.as_deref().unwrap_or(&field.name);
                        let path = Some(walk.push_path(frame.path, key));
                        let Some(returned) = self
                            .return_types
                            .get(frame.parent)
                            .and_then(|fields| fields.get(&field.name))
                        else {
                            // Root fields are covered by the descriptor
                            // contract, which refuses every undescribed one.
                            // Below the root, an unknown field fails closed.
                            if !frame.root {
                                walk.deny(
                                    path,
                                    "FORBIDDEN",
                                    "operation is not described by authorization metadata",
                                    &mut denials,
                                );
                            }
                            continue;
                        };
                        if let Some(index) = self
                            .fields
                            .get(frame.parent)
                            .and_then(|fields| fields.get(&field.name))
                        {
                            walk.check(*index, path, &mut denials);
                        }
                        if !field.selections.items.is_empty() && self.guarded.contains(returned) {
                            children.push(Frame {
                                selections: &field.selections.items,
                                parent: returned,
                                path,
                                root: false,
                            });
                        }
                    }
                    SelectionItem::InlineFragment(fragment) => {
                        if walk
                            .excluded(fragment.skip_if.as_deref(), fragment.include_if.as_deref())
                        {
                            continue;
                        }
                        let condition = if fragment.type_condition.is_empty() {
                            frame.parent
                        } else {
                            fragment.type_condition.as_str()
                        };
                        if let Some(index) = self.fragments.get(condition) {
                            walk.check(*index, frame.path, &mut denials);
                        }
                        if self.guarded.contains(condition) {
                            children.push(Frame {
                                selections: &fragment.selections.items,
                                parent: condition,
                                path: frame.path,
                                root: frame.root,
                            });
                        }
                    }
                    // Hive's normalization expands every spread into an
                    // inline fragment before the analysis hook; a spread
                    // that is still present cannot be typed, so it is
                    // refused rather than skipped.
                    SelectionItem::FragmentSpread(_) => {
                        walk.deny(
                            frame.path,
                            "FORBIDDEN",
                            "operation is not described by authorization metadata",
                            &mut denials,
                        );
                    }
                }
                if denials.len() >= MAX_REPORTED_DENIALS {
                    return denials;
                }
            }
            stack.extend(children.into_iter().rev());
        }
        denials
    }
}

struct Frame<'a> {
    selections: &'a [SelectionItem],
    parent: &'a str,
    path: Option<usize>,
    root: bool,
}

struct Walk<'a> {
    nested: &'a NestedAuthorization,
    variables: &'a HashMap<String, JsonValue>,
    variable_resolution: VariableResolution,
    principal: Option<&'a AuthenticatedPrincipal>,
    matcher: &'a dyn ScopeMatcher,
    /// One evaluation per distinct requirement per operation.
    decisions: Vec<Option<Option<(&'static str, &'static str)>>>,
    /// Response-path nodes: parent node and response key.
    path_nodes: Vec<(Option<usize>, &'a str)>,
    reported: HashSet<Vec<String>>,
}

impl<'a> Walk<'a> {
    /// Whether a selection is left out of this check. Under preflight a
    /// condition whose variable is not yet known is left to the complete check
    /// that runs before execution, as for root fields.
    fn excluded(&self, skip_if: Option<&str>, include_if: Option<&str>) -> bool {
        directive_inclusion(
            skip_if,
            include_if,
            self.variables,
            self.variable_resolution,
        ) != DirectiveInclusion::Included
    }

    fn push_path(&mut self, parent: Option<usize>, key: &'a str) -> usize {
        self.path_nodes.push((parent, key));
        self.path_nodes.len() - 1
    }

    fn check(&mut self, index: usize, path: Option<usize>, denials: &mut Vec<AuthorizationDenial>) {
        let decision = *self.decisions[index].get_or_insert_with(|| {
            self.nested.requirements[index].evaluate(self.principal, self.matcher)
        });
        if let Some((code, message)) = decision {
            self.deny(path, code, message, denials);
        }
    }

    fn deny(
        &mut self,
        path: Option<usize>,
        code: &'static str,
        message: &'static str,
        denials: &mut Vec<AuthorizationDenial>,
    ) {
        let mut segments = Vec::new();
        let mut node = path;
        while let Some(index) = node {
            let (parent, key) = self.path_nodes[index];
            segments.push(key.to_owned());
            node = parent;
        }
        segments.reverse();
        if self.reported.insert(segments.clone()) {
            denials.push(AuthorizationDenial {
                code,
                message,
                path: Some(segments),
            });
        }
    }
}

/// Builds the composed rule of one directive site, refusing a malformed
/// `scopes` argument so the graph is not published with a rule it cannot
/// evaluate.
fn requirement<'a>(
    authenticated: bool,
    requires_scopes: impl Iterator<Item = &'a Value>,
    site: &str,
) -> Result<Requirement, RouterError> {
    let mut rule = Requirement {
        authenticated,
        scopes: Vec::new(),
    };
    for scopes in requires_scopes {
        let invalid = || {
            RouterError::new(
                RouterErrorKind::AuthorizationMetadata,
                format!(
                    "@requiresScopes on `{site}` must be a non-empty list of scope lists of strings"
                ),
            )
        };
        let Value::List(alternatives) = scopes else {
            return Err(invalid());
        };
        if alternatives.is_empty() {
            return Err(invalid());
        }
        let mut sets = BTreeSet::new();
        for alternative in alternatives {
            let Value::List(scopes) = alternative else {
                return Err(invalid());
            };
            let mut set = BTreeSet::new();
            for scope in scopes {
                let Value::String(scope) = scope else {
                    return Err(invalid());
                };
                set.insert(scope.clone());
            }
            sets.insert(set.into_iter().collect::<Vec<_>>());
        }
        rule.authenticated = true;
        let sets = sets.into_iter().collect::<Vec<_>>();
        if !rule.scopes.contains(&sets) {
            rule.scopes.push(sets);
        }
    }
    Ok(rule)
}

/// Object types each interface or union may resolve to.
fn possible_types(state: &SupergraphState) -> HashMap<String, BTreeSet<String>> {
    let mut possible = state
        .interface_to_object_types
        .iter()
        .map(|(interface, objects)| (interface.clone(), objects.clone()))
        .collect::<HashMap<_, _>>();
    for definition in state.definitions.values() {
        if let SupergraphDefinition::Union(union) = definition {
            possible
                .entry(union.name.clone())
                .or_default()
                .extend(union.types.iter().cloned());
        }
    }
    possible
}

/// Composite types below which the walk can meet a rule. A type is guarded
/// when one of its fields or its own fragment carries a rule, or when it
/// reaches a guarded type through a field's return type, a possible type, or
/// an interface it implements (a selection on any of them can narrow to the
/// others with an inline fragment).
fn guarded_types(
    state: &SupergraphState,
    nested: &NestedAuthorization,
    possible_types: &HashMap<String, BTreeSet<String>>,
) -> HashSet<String> {
    let mut edges = HashMap::<&str, BTreeSet<&str>>::new();
    for (parent, fields) in &nested.return_types {
        edges
            .entry(parent)
            .or_default()
            .extend(fields.values().map(String::as_str));
    }
    for (abstract_type, members) in possible_types {
        for member in members {
            edges
                .entry(abstract_type)
                .or_default()
                .insert(member.as_str());
            edges
                .entry(member)
                .or_default()
                .insert(abstract_type.as_str());
        }
    }
    for definition in state.definitions.values() {
        let implements = match definition {
            SupergraphDefinition::Object(object) => &object.join_implements,
            SupergraphDefinition::Interface(interface) => &interface.join_implements,
            _ => continue,
        };
        for implemented in implements {
            edges
                .entry(definition.name())
                .or_default()
                .insert(implemented.interface.as_str());
            edges
                .entry(implemented.interface.as_str())
                .or_default()
                .insert(definition.name());
        }
    }
    // Members of a common abstract type can narrow to each other through it,
    // which the edges above already join in both directions.
    let mut reverse = HashMap::<&str, Vec<&str>>::new();
    for (from, targets) in &edges {
        for target in targets {
            reverse.entry(target).or_default().push(from);
        }
    }
    let mut guarded = HashSet::<String>::new();
    let mut pending = nested
        .fields
        .keys()
        .map(String::as_str)
        .chain(nested.fragments.keys().map(String::as_str))
        .collect::<Vec<_>>();
    while let Some(name) = pending.pop() {
        if !guarded.insert(name.to_owned()) {
            continue;
        }
        if let Some(sources) = reverse.get(name) {
            pending.extend(sources.iter().copied());
        }
    }
    guarded
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use graphql_orm_router_protocol::{
        ArgumentDescriptor, AuthorizationRequirement, Fingerprint, OperationDescriptor,
        RootOperationType,
    };

    use super::super::{
        AuthorizationCatalog, ExactScopeMatcher, VariableResolution, tests::descriptor,
    };
    use super::*;
    use crate::federation::ActiveGraph;

    const DIRECTIVE_SDL: &str = r#"
        extend schema @link(
            url: "https://specs.apollo.dev/federation/v2.7"
            import: ["@key", "@authenticated", "@requiresScopes"]
        )
        type Query {
            zone(id: ID!): Zone
            search: [SearchResult!]!
            node(id: ID!): Node
            status: String!
        }
        type Subscription { zoneChanged(id: ID!): Zone }
        interface Node { id: ID! }
        type Zone implements Node @key(fields: "id") {
            id: ID!
            name: String!
            owner: Account @authenticated
            audit: String @requiresScopes(scopes: [["audit.read", "zones.read"], ["zones.admin"]])
            vault: Vault
            parent: Zone
        }
        type Vault @requiresScopes(scopes: [["vault.read"]]) {
            id: ID!
            secret: String @requiresScopes(scopes: [["vault.secret"]])
        }
        type Account { id: ID!, email: String @authenticated }
        type Badge implements Node { id: ID! @authenticated, label: String! }
        union SearchResult = Zone | Vault
    "#;

    fn directive_graph() -> std::sync::Arc<ActiveGraph> {
        crate::federation::GraphStore::new(&[crate::federation::CandidateSubgraph::new(
            "catalog",
            "http://catalog.test/graphql",
            DIRECTIVE_SDL,
        )])
        .unwrap()
        .load()
    }

    fn public(root_type: RootOperationType, field_name: &str, id: bool) -> OperationDescriptor {
        OperationDescriptor {
            root_type,
            field_name: field_name.to_owned(),
            arguments: id
                .then(|| ArgumentDescriptor {
                    name: "id".to_owned(),
                    graphql_type: "ID!".to_owned(),
                    required: true,
                })
                .into_iter()
                .collect(),
            authorization: AuthorizationRequirement::Public,
        }
    }

    fn directive_catalog(graph: &ActiveGraph) -> AuthorizationCatalog {
        let mut descriptor = descriptor(vec![
            public(RootOperationType::Query, "zone", true),
            public(RootOperationType::Query, "search", false),
            public(RootOperationType::Query, "node", true),
            public(RootOperationType::Query, "status", false),
            public(RootOperationType::Subscription, "zoneChanged", true),
        ]);
        descriptor.fingerprints.schema = Fingerprint::sha256(DIRECTIVE_SDL);
        descriptor.fingerprints.combined = descriptor.combined_fingerprint();
        AuthorizationCatalog::build(graph, &[descriptor]).unwrap()
    }

    fn check(
        query: &str,
        variables: &[(&str, bool)],
        scopes: Option<&[&str]>,
        resolution: VariableResolution,
    ) -> Vec<AuthorizationDenial> {
        check_with(query, variables, scopes, resolution, &ExactScopeMatcher)
    }

    fn check_with(
        query: &str,
        variables: &[(&str, bool)],
        scopes: Option<&[&str]>,
        resolution: VariableResolution,
        matcher: &dyn ScopeMatcher,
    ) -> Vec<AuthorizationDenial> {
        let graph = directive_graph();
        let catalog = directive_catalog(&graph);
        let document = hive_router::query_planner::utils::parsing::parse_operation(query);
        let operation = hive_router::query_planner::ast::normalization::normalize_operation(
            &graph.hive.snapshot().planner.supergraph,
            &document,
            None,
        )
        .unwrap()
        .operation;
        let variables = variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), JsonValue::from(*value)))
            .collect();
        let principal = scopes.map(|scopes| {
            AuthenticatedPrincipal::new("subject", scopes.iter().copied(), None).unwrap()
        });
        catalog.authorize_operation(
            &operation,
            &variables,
            principal.as_ref(),
            matcher,
            resolution,
        )
    }

    fn refused(code: &'static str, path: &[&str]) -> AuthorizationDenial {
        AuthorizationDenial {
            code,
            message: if code == "UNAUTHENTICATED" {
                "authentication is required"
            } else {
                "required scope is missing"
            },
            path: Some(path.iter().map(|key| (*key).to_owned()).collect()),
        }
    }

    fn both(query: &str, scopes: Option<&[&str]>) -> Vec<AuthorizationDenial> {
        let preflight = check(query, &[], scopes, VariableResolution::Preflight);
        let complete = check(query, &[], scopes, VariableResolution::Complete);
        assert_eq!(preflight, complete, "{query}");
        complete
    }

    #[test]
    fn nested_authenticated_fields_are_refused_for_anonymous_callers() {
        assert_eq!(
            both("{ zone(id: \"z1\") { id owner { id } } }", None),
            vec![refused("UNAUTHENTICATED", &["zone", "owner"])]
        );
        // Deeper levels, and the field's own directive below a protected one.
        assert_eq!(
            both(
                "{ zone(id: \"z1\") { parent { parent { owner { email } } } } }",
                None
            ),
            vec![
                refused("UNAUTHENTICATED", &["zone", "parent", "parent", "owner"]),
                refused(
                    "UNAUTHENTICATED",
                    &["zone", "parent", "parent", "owner", "email"]
                ),
            ]
        );
        // An authenticated caller without scopes is served.
        assert!(both("{ zone(id: \"z1\") { owner { email } } }", Some(&[])).is_empty());
        // Unprotected selections stay public.
        assert!(
            both(
                "{ status zone(id: \"z1\") { id name parent { name } } }",
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn requires_scopes_is_an_or_of_and_sets_and_implies_authentication() {
        let query = "{ zone(id: \"z1\") { audit } }";
        assert_eq!(
            both(query, None),
            vec![refused("UNAUTHENTICATED", &["zone", "audit"])]
        );
        for denied in [
            &[][..],
            &["audit.read"],
            &["zones.read"],
            &["zones.admin.x"],
        ] {
            assert_eq!(
                both(query, Some(denied)),
                vec![refused("FORBIDDEN", &["zone", "audit"])],
                "{denied:?}"
            );
        }
        for allowed in [
            &["audit.read", "zones.read"][..],
            &["zones.admin"],
            &["zones.read", "zones.admin", "audit.read"],
        ] {
            assert!(both(query, Some(allowed)).is_empty(), "{allowed:?}");
        }
    }

    #[test]
    fn rules_on_the_parent_field_and_return_type_must_all_hold() {
        // `Zone.vault` returns `Vault`, which requires `vault.read`;
        // `Vault.secret` also requires `vault.secret`.
        let query = "{ zone(id: \"z1\") { vault { id secret } } }";
        assert_eq!(
            both(query, Some(&["vault.secret"])),
            vec![
                refused("FORBIDDEN", &["zone", "vault"]),
                refused("FORBIDDEN", &["zone", "vault", "id"]),
                refused("FORBIDDEN", &["zone", "vault", "secret"]),
            ]
        );
        assert_eq!(
            both(query, Some(&["vault.read"])),
            vec![refused("FORBIDDEN", &["zone", "vault", "secret"])]
        );
        assert!(both(query, Some(&["vault.read", "vault.secret"])).is_empty());
    }

    #[test]
    fn aliases_and_fragments_reach_the_same_rule() {
        let expected = |path: &[&str]| vec![refused("UNAUTHENTICATED", path)];
        assert_eq!(
            both("{ zone(id: \"z1\") { holder: owner { id } } }", None),
            expected(&["zone", "holder"])
        );
        assert_eq!(
            both(
                "query { zone(id: \"z1\") { ...Holder } } fragment Holder on Zone { owner { id } }",
                None
            ),
            expected(&["zone", "owner"])
        );
        assert_eq!(
            both(
                "{ zone(id: \"z1\") { ... on Zone { owner { id } } } }",
                None
            ),
            expected(&["zone", "owner"])
        );
        assert_eq!(
            both(
                "{ ... on Query { area: zone(id: \"z1\") { ... { owner { id } } } } }",
                None
            ),
            expected(&["area", "owner"])
        );
        assert!(
            both(
                "query { zone(id: \"z1\") { ...Holder } } fragment Holder on Zone { owner { id } }",
                Some(&[])
            )
            .is_empty()
        );
    }

    #[test]
    fn interface_and_union_members_are_authorized() {
        // A union member's type rule applies to its fragment and its fields.
        assert_eq!(
            both("{ search { ... on Vault { id } } }", Some(&[])),
            vec![
                refused("FORBIDDEN", &["search"]),
                refused("FORBIDDEN", &["search", "id"]),
            ]
        );
        assert_eq!(
            both("{ search { ... on Zone { owner { id } } } }", None),
            vec![refused("UNAUTHENTICATED", &["search", "owner"])]
        );
        assert!(both("{ search { ... on Zone { id name } } }", None).is_empty());
        assert!(both("{ search { ... on Vault { id } } }", Some(&["vault.read"])).is_empty());
        // A field selected through an interface carries the rules of every
        // implementation: `Badge.id` is `@authenticated`.
        assert_eq!(
            both("{ node(id: \"n1\") { id } }", None),
            vec![refused("UNAUTHENTICATED", &["node", "id"])]
        );
        assert!(both("{ node(id: \"n1\") { id } }", Some(&[])).is_empty());
        assert!(both("{ node(id: \"n1\") { ... on Badge { label } } }", None).is_empty());
        assert_eq!(
            both(
                "{ node(id: \"n1\") { ... on Zone { owner { id } } } }",
                None
            ),
            vec![refused("UNAUTHENTICATED", &["node", "owner"])]
        );
    }

    #[test]
    fn typename_stays_available_at_every_depth() {
        for query in [
            "{ __typename }",
            "{ zone(id: \"z1\") { __typename owner { __typename } } }",
            "{ search { __typename ... on Vault { __typename } } }",
            "{ node(id: \"n1\") { __typename } }",
        ] {
            let expected = if query.contains("owner") {
                vec![refused("UNAUTHENTICATED", &["zone", "owner"])]
            } else if query.contains("on Vault") {
                vec![refused("UNAUTHENTICATED", &["search"])]
            } else {
                Vec::new()
            };
            assert_eq!(both(query, None), expected, "{query}");
        }
    }

    #[test]
    fn skip_and_include_follow_the_root_variable_rules() {
        let query = "query Read($show: Boolean!) { zone(id: \"z1\") { id owner @include(if: $show) { id } } }";
        assert!(
            check(
                query,
                &[("show", false)],
                None,
                VariableResolution::Complete
            )
            .is_empty()
        );
        assert_eq!(
            check(query, &[("show", true)], None, VariableResolution::Complete),
            vec![refused("UNAUTHENTICATED", &["zone", "owner"])]
        );
        // Preflight leaves an unknown condition to the complete check, which
        // runs before execution.
        assert!(check(query, &[], None, VariableResolution::Preflight).is_empty());
        let skipped = "{ zone(id: \"z1\") { id owner @skip(if: true) { id } } }";
        assert!(both(skipped, None).is_empty());
    }

    #[test]
    fn subscriptions_are_authorized_below_the_root() {
        assert_eq!(
            both(
                "subscription { zoneChanged(id: \"z1\") { audit } }",
                Some(&[])
            ),
            vec![refused("FORBIDDEN", &["zoneChanged", "audit"])]
        );
        assert!(
            both(
                "subscription { zoneChanged(id: \"z1\") { audit } }",
                Some(&["zones.admin"])
            )
            .is_empty()
        );
    }

    #[derive(Debug, Default)]
    struct CountingMatcher(AtomicUsize);

    impl ScopeMatcher for CountingMatcher {
        fn matches(&self, granted: &str, required: &str) -> bool {
            self.0.fetch_add(1, Ordering::Relaxed);
            granted == required
        }
    }

    #[test]
    fn large_selections_evaluate_each_rule_once_and_stop_after_the_report_bound() {
        // 2,000 aliased selections of the same protected fields, nested ten
        // deep: each distinct rule is evaluated once per operation.
        let mut inner = String::from("id audit vault { secret }");
        for _ in 0..10 {
            inner = format!("id audit parent {{ {inner} }}");
        }
        let selections = (0..2_000)
            .map(|index| format!("z{index}: zone(id: \"z{index}\") {{ {inner} }}"))
            .collect::<Vec<_>>()
            .join(" ");
        let query = format!("{{ {selections} }}");
        let scopes = ["zones.admin", "vault.read", "vault.secret"];

        let matcher = CountingMatcher::default();
        let started = Instant::now();
        let allowed = check_with(
            &query,
            &[],
            Some(&scopes),
            VariableResolution::Complete,
            &matcher,
        );
        assert!(allowed.is_empty());
        // Three distinct requirements, each matched against three grants at
        // most once per required scope.
        assert!(
            matcher.0.load(Ordering::Relaxed) <= 30,
            "{} scope comparisons",
            matcher.0.load(Ordering::Relaxed)
        );

        let refused = check(&query, &[], Some(&[]), VariableResolution::Complete);
        assert_eq!(refused.len(), MAX_REPORTED_DENIALS);
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "parsing, normalization and both walks took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn malformed_scope_arguments_are_refused_when_the_graph_is_built() {
        let string = |value: &str| Value::String(value.to_owned());
        for malformed in [
            Value::Null,
            Value::List(Vec::new()),
            Value::List(vec![string("scope")]),
            Value::List(vec![Value::List(vec![Value::Int(1)])]),
        ] {
            assert_eq!(
                requirement(false, [&malformed].into_iter(), "Type.field")
                    .unwrap_err()
                    .kind(),
                RouterErrorKind::AuthorizationMetadata
            );
        }
        let rule = requirement(
            false,
            [&Value::List(vec![Value::List(vec![
                string("b"),
                string("a"),
            ])])]
            .into_iter(),
            "Type.field",
        )
        .unwrap();
        assert!(rule.authenticated);
        assert_eq!(
            rule.scopes,
            vec![vec![vec!["a".to_owned(), "b".to_owned()]]]
        );
    }

    #[test]
    fn graphs_without_directives_skip_the_walk() {
        let graph =
            crate::federation::GraphStore::new(&[crate::federation::CandidateSubgraph::new(
                "plain",
                "http://plain.test/graphql",
                r#"
                extend schema @link(url: "https://specs.apollo.dev/federation/v2.7")
                type Query { status: String! }
            "#,
            )])
            .unwrap()
            .load();
        let nested = NestedAuthorization::build(&graph.hive.snapshot().planner.supergraph).unwrap();
        assert!(nested.is_empty());
        assert!(nested.guarded.is_empty());
    }
}
