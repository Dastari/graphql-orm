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
//! - A field `T.f` of a type that implements interface `I` also requires the
//!   rules declared on `I.f` and on `I`. Composition keeps a rule declared on
//!   an interface, or on an `@interfaceObject` type in another subgraph, only
//!   on the interface.
//! - An inline fragment on type `T` requires the rule on `T`.
//! - `@requiresScopes(scopes: [[a, b], [c]])` holds when every scope of at
//!   least one inner set is granted (OR of AND sets). It also requires
//!   authentication. Several scope requirements on one selection must all
//!   hold, and each subgraph's declaration on a shared type or field is its
//!   own requirement: composition merges them into one list of alternatives,
//!   so they are read from each subgraph's SDL (see
//!   `federation::scope_sites`) and checked against the composed directives.
//! - `__typename` is answered without a subgraph and is never refused.
//!   `__schema` and `__type` are governed by the router's introspection
//!   policy, before this check.
//!
//! A refused selection refuses the whole operation, like a refused root field:
//! the router never trims a selection and never returns partial data for it.
//! Each refusal names only its response path. The router may deny early but
//! never grants: subgraph guards and policies stay authoritative.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
use crate::{
    RouterError, RouterErrorKind,
    federation::{ScopeSite, SubgraphScopeRequirements},
};

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
    pub(super) fn build(
        state: &SupergraphState,
        declared: &SubgraphScopeRequirements,
    ) -> Result<Self, RouterError> {
        let mut type_rules = HashMap::new();
        let mut field_rules = HashMap::<String, HashMap<String, Requirement>>::new();
        let mut return_types = HashMap::<String, HashMap<String, String>>::new();
        let mut visited = HashSet::<ScopeSite>::new();
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
            let site = (name.clone(), None);
            let rule = site_requirement(
                !authenticated.is_empty(),
                requires_scopes.iter().map(|directive| &directive.scopes),
                &site,
                declared,
            )?;
            visited.insert(site);
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
                let site = (name.clone(), Some(field_name.clone()));
                let rule = site_requirement(
                    !field.authenticated.is_empty(),
                    field
                        .requires_scopes
                        .iter()
                        .map(|directive| &directive.scopes),
                    &site,
                    declared,
                )?;
                visited.insert(site);
                if !rule.is_empty() {
                    field_rules
                        .entry(name.clone())
                        .or_default()
                        .insert(field_name.clone(), rule);
                }
            }
        }
        // Every subgraph declaration must land on a composed site. One that
        // does not cannot be enforced, so the graph is refused.
        if let Some((type_name, field)) = declared
            .sites
            .keys()
            .chain(&declared.authenticated)
            .find(|site| !visited.contains(*site))
        {
            return Err(unmapped(type_name, field.as_deref()));
        }

        let possible_types = possible_types(state);
        let implemented = implemented_interfaces(state);
        let type_rule = |name: &str| type_rules.get(name).cloned().unwrap_or_default();
        // A field's own rule includes the rules declared on the same field of
        // every interface its type implements, and those interfaces' type
        // rules. Composition keeps a directive declared on an interface, or on
        // an `@interfaceObject` type in another subgraph, only on the
        // interface, so the implementation must inherit it.
        let field_rule = |parent: &str, field: &str| {
            let mut rule = field_rules
                .get(parent)
                .and_then(|fields| fields.get(field))
                .cloned()
                .unwrap_or_default();
            for interface in implemented.get(parent).into_iter().flatten() {
                let declares_field = return_types
                    .get(interface)
                    .is_some_and(|fields| fields.contains_key(field));
                if !declares_field {
                    continue;
                }
                if let Some(inherited) = field_rules
                    .get(interface)
                    .and_then(|fields| fields.get(field))
                {
                    rule.and(inherited);
                }
                rule.and(&type_rule(interface));
            }
            rule
        };
        let mut nested = Self::default();
        let mut interned = HashMap::<Requirement, usize>::new();
        let mut intern = |rule: Requirement, nested: &mut Self| -> usize {
            *interned.entry(rule.clone()).or_insert_with(|| {
                nested.requirements.push(rule);
                nested.requirements.len() - 1
            })
        };

        for (parent, fields) in &return_types {
            for (field, returned) in fields {
                let mut rule = type_rule(parent);
                rule.and(&field_rule(parent, field));
                rule.and(&type_rule(returned));
                if matches!(
                    state.definitions.get(parent),
                    Some(SupergraphDefinition::Interface(_))
                ) {
                    for implementation in possible_types.get(parent).into_iter().flatten() {
                        rule.and(&type_rule(implementation));
                        rule.and(&field_rule(implementation, field));
                        if let Some(returned) = return_types
                            .get(implementation)
                            .and_then(|fields| fields.get(field))
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
                        .insert(field.clone(), index);
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
        nested.return_types = return_types;
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
                    "@requiresScopes on `{site}` must be a non-empty list of non-empty scope lists of strings"
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
                if scope.is_empty() {
                    return Err(invalid());
                }
                set.insert(scope.clone());
            }
            // An empty set would be satisfied by every caller.
            if set.is_empty() {
                return Err(invalid());
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

/// The rule of one composed site, with each subgraph's `@requiresScopes`
/// declaration as its own entry, so that all of them must hold. Composition
/// merged the declarations into one list of alternatives; the declarations
/// must account for exactly that list, or the graph is refused.
fn site_requirement<'a>(
    authenticated: bool,
    composed: impl Iterator<Item = &'a Value>,
    site: &ScopeSite,
    declared: &SubgraphScopeRequirements,
) -> Result<Requirement, RouterError> {
    let label = match site {
        (type_name, Some(field)) => format!("{type_name}.{field}"),
        (type_name, None) => type_name.clone(),
    };
    let composed = requirement(authenticated, composed, &label)?;
    // Composition applies `@authenticated` when any subgraph declares it.
    if authenticated != declared.authenticated.contains(site) {
        return Err(unmapped(&site.0, site.1.as_deref()));
    }
    let declared = declared.sites.get(site).cloned().unwrap_or_default();
    // A secondary defence only. Composition deduplicates alternatives, so a
    // declaration whose scope sets another declaration repeats leaves the
    // composed union unchanged, and this comparison cannot see it dropped.
    // The primary defence is the name-based refusal in
    // `federation::scope_sites`: every directive application composition
    // could read as `@requiresScopes` or `@authenticated` must resolve to a
    // recorded site, or the graph is refused before this point.
    let composed_sets = composed.scopes.iter().flatten().collect::<BTreeSet<_>>();
    let declared_sets = declared.iter().flatten().collect::<BTreeSet<_>>();
    if composed_sets != declared_sets {
        return Err(unmapped(&site.0, site.1.as_deref()));
    }
    Ok(Requirement {
        authenticated: composed.authenticated || !declared.is_empty(),
        scopes: declared,
    })
}

fn unmapped(type_name: &str, field: Option<&str>) -> RouterError {
    let site = match field {
        Some(field) => format!("{type_name}.{field}"),
        None => type_name.to_owned(),
    };
    RouterError::new(
        RouterErrorKind::AuthorizationMetadata,
        format!(
            "authorization directives on `{site}` cannot be matched to the subgraph declarations they were composed from"
        ),
    )
}

/// Interfaces each object or interface implements, transitively.
fn implemented_interfaces(state: &SupergraphState) -> HashMap<String, BTreeSet<String>> {
    let direct = state
        .definitions
        .values()
        .filter_map(|definition| {
            let implements = match definition {
                SupergraphDefinition::Object(object) => &object.join_implements,
                SupergraphDefinition::Interface(interface) => &interface.join_implements,
                _ => return None,
            };
            Some((
                definition.name().to_owned(),
                implements
                    .iter()
                    .map(|implemented| implemented.interface.clone())
                    .collect::<BTreeSet<_>>(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut closed = HashMap::new();
    for name in direct.keys() {
        let mut reached = BTreeSet::new();
        let mut pending = direct[name].iter().cloned().collect::<Vec<_>>();
        while let Some(interface) = pending.pop() {
            if reached.insert(interface.clone()) {
                pending.extend(direct.get(&interface).into_iter().flatten().cloned());
            }
        }
        closed.insert(name.clone(), reached);
    }
    closed
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
        let nested = NestedAuthorization::build(
            &graph.hive.snapshot().planner.supergraph,
            &graph.scope_requirements,
        )
        .unwrap();
        assert!(nested.is_empty());
        assert!(nested.guarded.is_empty());
    }

    fn graph_of(subgraphs: &[(&str, &str)]) -> std::sync::Arc<ActiveGraph> {
        let inputs = subgraphs
            .iter()
            .map(|(name, sdl)| {
                crate::federation::CandidateSubgraph::new(
                    name,
                    &format!("http://{name}.test/graphql"),
                    sdl,
                )
            })
            .collect::<Vec<_>>();
        crate::federation::GraphStore::new(&inputs).unwrap().load()
    }

    /// A catalogue whose root fields are all public, so only composed
    /// directives can refuse.
    fn public_catalog(graph: &ActiveGraph) -> Result<AuthorizationCatalog, RouterError> {
        let operations = super::super::graph_root_contract(graph)?
            .into_iter()
            .map(|((root_type, field_name), arguments)| OperationDescriptor {
                root_type,
                field_name,
                arguments: arguments
                    .into_iter()
                    .map(|(name, graphql_type)| ArgumentDescriptor {
                        name,
                        required: graphql_type.ends_with('!'),
                        graphql_type,
                    })
                    .collect(),
                authorization: AuthorizationRequirement::Public,
            })
            .collect();
        AuthorizationCatalog::build(graph, &[descriptor(operations)])
    }

    fn check_graph(
        graph: &ActiveGraph,
        query: &str,
        scopes: Option<&[&str]>,
    ) -> Vec<AuthorizationDenial> {
        let catalog = public_catalog(graph).unwrap();
        let document = hive_router::query_planner::utils::parsing::parse_operation(query);
        let operation = hive_router::query_planner::ast::normalization::normalize_operation(
            &graph.hive.snapshot().planner.supergraph,
            &document,
            None,
        )
        .unwrap()
        .operation;
        let principal = scopes.map(|scopes| {
            AuthenticatedPrincipal::new("subject", scopes.iter().copied(), None).unwrap()
        });
        let mut results = [VariableResolution::Preflight, VariableResolution::Complete]
            .map(|resolution| {
                catalog.authorize_operation(
                    &operation,
                    &HashMap::new(),
                    principal.as_ref(),
                    &ExactScopeMatcher,
                    resolution,
                )
            })
            .into_iter();
        let preflight = results.next().unwrap();
        let complete = results.next().unwrap();
        assert_eq!(preflight, complete, "{query}");
        complete
    }

    const SHARED_A: &str = r#"
        extend schema @link(
            url: "https://specs.apollo.dev/federation/v2.7"
            import: ["@key", "@shareable", "@requiresScopes"]
        )
        type Query { item: Item, secret: Secret }
        type Item @key(fields: "id") {
            id: ID!
            name: String @shareable @requiresScopes(scopes: [["a"]])
        }
        type Secret @shareable @requiresScopes(scopes: [["a"]]) { id: ID! }
    "#;

    const SHARED_B: &str = r#"
        extend schema @link(
            url: "https://specs.apollo.dev/federation/v2.7"
            import: ["@key", "@shareable", "@requiresScopes"]
        )
        type Item @key(fields: "id") {
            id: ID!
            name: String @shareable @requiresScopes(scopes: [["b"]])
        }
        type Secret @shareable @requiresScopes(scopes: [["b"]]) { id: ID! }
    "#;

    #[test]
    fn per_subgraph_scope_requirements_are_combined_with_and_semantics() {
        let graph = graph_of(&[("alpha", SHARED_A), ("beta", SHARED_B)]);
        for scopes in [&["a"][..], &["b"]] {
            assert_eq!(
                check_graph(&graph, "{ item { id name } }", Some(scopes)),
                vec![refused("FORBIDDEN", &["item", "name"])],
                "{scopes:?}"
            );
            assert_eq!(
                check_graph(&graph, "{ secret { id } }", Some(scopes)),
                vec![
                    refused("FORBIDDEN", &["secret"]),
                    refused("FORBIDDEN", &["secret", "id"]),
                ],
                "{scopes:?}"
            );
        }
        assert_eq!(
            check_graph(&graph, "{ item { name } }", None),
            vec![refused("UNAUTHENTICATED", &["item", "name"])]
        );
        for query in ["{ item { id name } }", "{ secret { id } }"] {
            assert!(
                check_graph(&graph, query, Some(&["a", "b"])).is_empty(),
                "{query}"
            );
        }
    }

    const MEDIA_OWNER: &str = r#"
        extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key"])
        type Query { book: Book, media: Media }
        interface Media @key(fields: "id") { id: ID! }
        type Book implements Media @key(fields: "id") { id: ID!, title: String }
    "#;

    const MEDIA_OBJECT: &str = r#"
        extend schema @link(
            url: "https://specs.apollo.dev/federation/v2.7"
            import: ["@key", "@interfaceObject", "@authenticated", "@requiresScopes"]
        )
        type Media @key(fields: "id") @interfaceObject {
            id: ID!
            reviews: String @authenticated
            audit: String @requiresScopes(scopes: [["audit"]])
        }
    "#;

    #[test]
    fn interface_object_rules_apply_to_every_implementation() {
        let graph = graph_of(&[("owner", MEDIA_OWNER), ("annotations", MEDIA_OBJECT)]);
        assert_eq!(
            check_graph(&graph, "{ book { title reviews } }", None),
            vec![refused("UNAUTHENTICATED", &["book", "reviews"])]
        );
        assert!(check_graph(&graph, "{ book { title reviews } }", Some(&[])).is_empty());
        let spread = "{ book { r: reviews ...F } } fragment F on Book { audit }";
        let narrowed = "{ media { ... on Book { reviews audit } } }";
        for (query, reviews, audit) in [
            (spread, ["book", "r"], ["book", "audit"]),
            (narrowed, ["media", "reviews"], ["media", "audit"]),
        ] {
            assert_eq!(
                check_graph(&graph, query, None),
                vec![
                    refused("UNAUTHENTICATED", &reviews),
                    refused("UNAUTHENTICATED", &audit),
                ],
                "{query}"
            );
            assert_eq!(
                check_graph(&graph, query, Some(&[])),
                vec![refused("FORBIDDEN", &audit)],
                "{query}"
            );
            assert!(
                check_graph(&graph, query, Some(&["audit"])).is_empty(),
                "{query}"
            );
        }
        // The interface field itself keeps its rule.
        assert_eq!(
            check_graph(&graph, "{ media { audit } }", Some(&[])),
            vec![refused("FORBIDDEN", &["media", "audit"])]
        );
    }

    #[test]
    fn empty_or_non_string_scope_sets_fail_graph_construction() {
        for scopes in ["[[]]", "[[\"a\"], []]", "[[1]]", "[]"] {
            let sdl = format!(
                r#"
                extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes"])
                type Query {{ status: String @requiresScopes(scopes: {scopes}) }}
                "#
            );
            let built =
                crate::federation::GraphStore::new(&[crate::federation::CandidateSubgraph::new(
                    "bad",
                    "http://bad.test/graphql",
                    &sdl,
                )]);
            assert!(
                matches!(
                    built,
                    Err(crate::federation::FederationError::SourceSdl { .. })
                ),
                "{scopes}"
            );
        }
    }

    #[test]
    fn declarations_that_do_not_match_the_composed_sets_refuse_the_graph() {
        // Composition reads `@other__requiresScopes` as the imported
        // directive, and so does the router.
        let graph = graph_of(&[(
            "odd",
            r#"
            extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes", "@authenticated"])
            type Query {
                status: String @other__requiresScopes(scopes: [["a"]])
                viewer: String @authenticated
            }
            "#,
        )]);
        assert_eq!(
            check_graph(&graph, "{ status }", Some(&[])),
            vec![refused("FORBIDDEN", &["status"])]
        );
        let state = &graph.hive.snapshot().planner.supergraph;
        let site = |field: &str| ("Query".to_owned(), Some(field.to_owned()));
        let mismatched = [
            // Missing declarations.
            SubgraphScopeRequirements::default(),
            // A different scope set.
            SubgraphScopeRequirements {
                sites: BTreeMap::from([(site("status"), vec![vec![vec!["b".to_owned()]]])]),
                authenticated: BTreeSet::from([site("viewer")]),
            },
            // A declaration with no composed site.
            SubgraphScopeRequirements {
                sites: BTreeMap::from([
                    (site("status"), vec![vec![vec!["a".to_owned()]]]),
                    (site("absent"), vec![vec![vec!["a".to_owned()]]]),
                ]),
                authenticated: BTreeSet::from([site("viewer")]),
            },
            // `@authenticated` the composed graph does not carry.
            SubgraphScopeRequirements {
                sites: BTreeMap::from([(site("status"), vec![vec![vec!["a".to_owned()]]])]),
                authenticated: BTreeSet::from([site("viewer"), site("status")]),
            },
        ];
        for declared in mismatched {
            assert_eq!(
                NestedAuthorization::build(state, &declared)
                    .unwrap_err()
                    .kind(),
                RouterErrorKind::AuthorizationMetadata,
                "{declared:?}"
            );
        }
        assert!(NestedAuthorization::build(state, &graph.scope_requirements).is_ok());
    }

    #[test]
    fn deferred_and_streamed_selections_are_walked() {
        assert_eq!(
            both(
                "{ zone(id: \"z1\") { id ... @defer { owner { id } } } }",
                None
            ),
            vec![refused("UNAUTHENTICATED", &["zone", "owner"])]
        );
        assert_eq!(
            both(
                "{ zone(id: \"z1\") { ... on Zone @defer(label: \"later\") { audit } } }",
                Some(&[])
            ),
            vec![refused("FORBIDDEN", &["zone", "audit"])]
        );
        assert_eq!(
            both(
                "{ search @stream(initialCount: 0) { ... on Zone { owner { id } } } }",
                None
            ),
            vec![refused("UNAUTHENTICATED", &["search", "owner"])]
        );
    }

    const OVERLAP_ALPHA: &str = r#"
        extend schema @link(
            url: "https://specs.apollo.dev/federation/v2.7"
            import: ["@key", "@shareable", "@requiresScopes"]
        )
        type Query { item: Item }
        type Item @key(fields: "id") {
            id: ID!
            name: String @shareable @requiresScopes(scopes: [["a"], ["b"]])
        }
    "#;

    fn overlap_beta(link: &str, directive: &str) -> String {
        format!(
            r#"
            extend schema {link}
            type Item @key(fields: "id") {{
                id: ID!
                name: String @shareable {directive}
            }}
            "#
        )
    }

    const PREFIX_LINK: &str = r#"@link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key", "@shareable", "@requiresScopes", "@authenticated"])"#;
    const RENAMED_LINK: &str = r#"@link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key", "@shareable", { name: "@requiresScopes", as: "@scopes" }, { name: "@authenticated", as: "@signedIn" }])"#;
    const COMPOSITE_LINK: &str = r#"@link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@key", "@shareable"]) @link(url: "https://specs.grafbase.com/composite-schemas/v1")"#;

    #[test]
    fn every_composition_spelling_of_a_shared_requirement_is_enforced() {
        let spellings = [
            (PREFIX_LINK, r#"@zzz__requiresScopes(scopes: [["a"]])"#),
            (RENAMED_LINK, r#"@zzz__scopes(scopes: [["a"]])"#),
            (COMPOSITE_LINK, r#"@requiresScopes(scopes: [["a"]])"#),
        ];
        for (link, directive) in spellings {
            let beta = overlap_beta(link, directive);
            let graph = graph_of(&[("alpha", OVERLAP_ALPHA), ("beta", &beta)]);
            // `b` meets alpha's alternatives but not beta's requirement.
            assert_eq!(
                check_graph(&graph, "{ item { name } }", Some(&["b"])),
                vec![refused("FORBIDDEN", &["item", "name"])],
                "{directive}"
            );
            assert!(
                check_graph(&graph, "{ item { name } }", Some(&["a"])).is_empty(),
                "{directive}"
            );
        }
        // Two spellings on one field of one subgraph.
        let doubled = OVERLAP_ALPHA.replace(
            r#"@requiresScopes(scopes: [["a"], ["b"]])"#,
            r#"@requiresScopes(scopes: [["a"], ["b"]]) @zzz__requiresScopes(scopes: [["a"]])"#,
        );
        let graph = graph_of(&[("alpha", &doubled)]);
        assert_eq!(
            check_graph(&graph, "{ item { name } }", Some(&["b"])),
            vec![refused("FORBIDDEN", &["item", "name"])]
        );
        assert!(check_graph(&graph, "{ item { name } }", Some(&["a"])).is_empty());
    }

    #[test]
    fn a_unique_alternative_from_another_spelling_is_also_required() {
        let beta = overlap_beta(PREFIX_LINK, r#"@zzz__requiresScopes(scopes: [["c"]])"#);
        let graph = graph_of(&[("alpha", OVERLAP_ALPHA), ("beta", &beta)]);
        for scopes in [&["a"][..], &["b"], &["c"]] {
            assert_eq!(
                check_graph(&graph, "{ item { name } }", Some(scopes)),
                vec![refused("FORBIDDEN", &["item", "name"])],
                "{scopes:?}"
            );
        }
        assert!(check_graph(&graph, "{ item { name } }", Some(&["b", "c"])).is_empty());
    }

    #[test]
    fn every_composition_spelling_of_authenticated_is_enforced() {
        let alpha = OVERLAP_ALPHA.replace(r#" @requiresScopes(scopes: [["a"], ["b"]])"#, "");
        for (link, directive) in [
            (PREFIX_LINK, "@zzz__authenticated"),
            (RENAMED_LINK, "@zzz__signedIn"),
            (COMPOSITE_LINK, "@authenticated"),
        ] {
            let beta = overlap_beta(link, directive);
            let graph = graph_of(&[("alpha", &alpha), ("beta", &beta)]);
            assert_eq!(
                check_graph(&graph, "{ item { id name } }", None),
                vec![refused("UNAUTHENTICATED", &["item", "name"])],
                "{directive}"
            );
            assert!(
                check_graph(&graph, "{ item { id name } }", Some(&[])).is_empty(),
                "{directive}"
            );
        }
    }

    #[test]
    fn authorization_directives_on_arguments_and_inputs_fail_graph_construction() {
        for body in [
            r#"type Query { item(x: String @requiresScopes(scopes: [["a"]])): String }"#,
            "type Query { item(x: String @authenticated): String }",
            r#"type Query { item(x: Filter): String } input Filter { name: String @requiresScopes(scopes: [["a"]]) }"#,
        ] {
            let sdl = format!(
                r#"
                extend schema @link(url: "https://specs.apollo.dev/federation/v2.7", import: ["@requiresScopes", "@authenticated"])
                {body}
                "#
            );
            let built =
                crate::federation::GraphStore::new(&[crate::federation::CandidateSubgraph::new(
                    "args",
                    "http://args.test/graphql",
                    &sdl,
                )]);
            assert!(
                matches!(
                    built,
                    Err(crate::federation::FederationError::SourceSdl { .. })
                ),
                "{body}"
            );
        }
    }
}
