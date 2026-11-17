//! Argument value checks shared by generated static schemas and runtime
//! GraphQL schemas.
//!
//! async-graphql 7.2.1 validates arguments with two gaps: it accepts any value
//! for an input object (a scalar, list or enum where an object is expected), and
//! it skips an argument whose value names a variable the request omits, then
//! applies the variable's default at execution without checking it. It also
//! never applies the rule that a variable's declared type must fit the position
//! it is used in, in either validation mode. A value of the wrong shape then fails
//! later, while its field resolves, with an uncoded coercion message, after
//! earlier fields of the same operation may already have run.
//!
//! A shared checker checks every argument of the selected operation with
//! the values execution resolves, after async-graphql's own validation. The
//! runtime guard calls it with its runtime scalars decoding under their input
//! bounds; generated static schemas install [`GeneratedInputValidation`].
use async_graphql::{
    ServerError, Value, Variables,
    extensions::{
        Extension, ExtensionContext, ExtensionFactory, NextParseQuery, NextPrepareRequest,
        NextValidation,
    },
    parser::types::{ExecutableDocument, OperationDefinition, Selection, SelectionSet},
    registry::{MetaType, MetaTypeName, Registry, ScalarValidatorFn},
};
use std::sync::{Arc, Mutex};

/// The stable code of an argument value refused by these checks.
pub(crate) const INVALID_GRAPHQL_INPUT: &str = "invalid_graphql_input";

/// The refusal both schema kinds return: the code is the message and
/// `extensions.code`, with no position, path or caller value.
pub(crate) fn invalid_input_error() -> ServerError {
    let mut error = ServerError::new(INVALID_GRAPHQL_INPUT, None);
    let mut extensions = async_graphql::ErrorExtensionValues::default();
    extensions.set("code", INVALID_GRAPHQL_INPUT);
    error.extensions = Some(extensions);
    error
}

/// Prefix of the value that stands for a variable reference: the variable's
/// name follows it. A GraphQL literal is never binary, so client text cannot
/// produce it.
const VARIABLE_PLACEHOLDER: &[u8] = b"\0graphql-orm variable\0";
fn variable_placeholder(name: &str) -> Value {
    let mut bytes = VARIABLE_PLACEHOLDER.to_vec();
    bytes.extend_from_slice(name.as_bytes());
    Value::Binary(bytes.into())
}

/// A variable of the selected operation, as a reference to it is checked.
struct VariableUse {
    /// The declared type.
    declared: String,
    /// Whether the definition has a default that is not `null`.
    non_null_default: bool,
    /// Whether the value execution resolves (the request's, or the default)
    /// is present and not `null`. That value was checked against the declared
    /// type once.
    non_null: bool,
}

/// What [`value_valid`] needs besides the value.
struct Check<'a> {
    registry: &'a Registry,
    scalar: &'a dyn Fn(&ScalarValidatorFn, &Value) -> bool,
    variables: Option<&'a std::collections::BTreeMap<String, VariableUse>>,
}

/// The operation a request selects: the named one, or the only one.
pub(crate) fn selected_operation<'a>(
    document: &'a ExecutableDocument,
    name: Option<&str>,
) -> Option<&'a OperationDefinition> {
    document
        .operations
        .iter()
        .find(|(operation, _)| match name {
            Some(expected) => operation.is_some_and(|operation| operation == expected),
            None => document.operations.iter().len() == 1,
        })
        .map(|(_, operation)| &operation.node)
}

/// Whether `value` is a valid input value of the GraphQL type `ty`.
///
/// This makes the checks async-graphql's argument validation makes, and also
/// refuses what async-graphql 7.2.1 accepts: a value that is not an object for
/// an input object, and a type the registry does not define. `scalar` applies a
/// scalar's validator to a value; the runtime path wraps it so its scalars
/// decode under their input bounds. Every visited node costs one unit of
/// `budget`; an exhausted budget refuses.
pub(crate) fn input_value_valid(
    registry: &Registry,
    ty: &str,
    value: &Value,
    budget: &mut usize,
    scalar: &dyn Fn(&ScalarValidatorFn, &Value) -> bool,
) -> bool {
    let check = Check {
        registry,
        scalar,
        variables: None,
    };
    value_valid(&check, ty, false, value, budget)
}

/// `location_default` says whether the position (an argument or an input
/// field) declares a default.
fn value_valid(
    check: &Check<'_>,
    ty: &str,
    location_default: bool,
    value: &Value,
    budget: &mut usize,
) -> bool {
    let Some(remaining) = budget.checked_sub(1) else {
        return false;
    };
    *budget = remaining;
    if let Some(name) = placeholder_name(value) {
        return variable_use_valid(check, ty, location_default, name);
    }
    let registry = check.registry;
    match MetaTypeName::create(ty) {
        MetaTypeName::NonNull(inner) => {
            value != &Value::Null && value_valid(check, inner, false, value, budget)
        }
        MetaTypeName::List(inner) => match value {
            Value::Null => true,
            Value::List(items) => items
                .iter()
                .all(|item| value_valid(check, inner, false, item, budget)),
            // GraphQL coerces a single value to a one-element list.
            item => value_valid(check, inner, false, item, budget),
        },
        MetaTypeName::Named(name) => {
            if value == &Value::Null {
                return true;
            }
            match registry.types.get(name) {
                Some(MetaType::Scalar { is_valid, .. }) => is_valid
                    .as_ref()
                    .is_none_or(|valid| (check.scalar)(valid, value)),
                Some(MetaType::Enum { enum_values, .. }) => match value {
                    Value::Enum(item) => enum_values.contains_key(item.as_str()),
                    Value::String(item) => enum_values.contains_key(item.as_str()),
                    _ => false,
                },
                Some(MetaType::InputObject {
                    input_fields,
                    oneof,
                    ..
                }) => {
                    let Value::Object(values) = value else {
                        return false;
                    };
                    if *oneof && !oneof_member_valid(check, values.values()) {
                        return false;
                    }
                    values
                        .keys()
                        .all(|name| input_fields.contains_key(name.as_str()))
                        && input_fields.values().all(|field| {
                            match values.get(field.name.as_str()) {
                                Some(value) => value_valid(
                                    check,
                                    &field.ty,
                                    field.default_value.is_some(),
                                    value,
                                    budget,
                                ),
                                None => {
                                    !MetaTypeName::create(&field.ty).is_non_null()
                                        || field.default_value.is_some()
                                }
                            }
                        })
                }
                _ => false,
            }
        }
    }
}

/// The variable name a placeholder stands for.
fn placeholder_name(value: &Value) -> Option<&[u8]> {
    match value {
        Value::Binary(bytes) => bytes.strip_prefix(VARIABLE_PLACEHOLDER),
        _ => None,
    }
}

/// The selected operation's definition of the variable `name`.
fn variable<'a>(check: &Check<'a>, name: &[u8]) -> Option<&'a VariableUse> {
    std::str::from_utf8(name)
        .ok()
        .and_then(|name| check.variables?.get(name))
}

/// The GraphQL rule for a oneof input object literal: exactly one field, its
/// value not `null`, and a variable standing for it declared non-null and
/// resolving to a value. A nullable variable may otherwise stand at the
/// member's nullable field type and resolve to `null`, or be omitted.
fn oneof_member_valid<'v>(check: &Check<'_>, mut members: impl Iterator<Item = &'v Value>) -> bool {
    let (Some(member), None) = (members.next(), members.next()) else {
        return false;
    };
    if member == &Value::Null {
        return false;
    }
    placeholder_name(member).is_none_or(|name| {
        variable(check, name).is_some_and(|variable| {
            MetaTypeName::create(&variable.declared).is_non_null() && variable.non_null
        })
    })
}

/// Whether a reference to the variable `name` may stand at a position of type
/// `ty`: the GraphQL rule "All Variable Usages Are Allowed", then the resolved
/// value's nullability.
///
/// async-graphql 7.2.1 declares that rule but never applies it: its validator
/// does not pass input values on to the rule, in either validation mode.
fn variable_use_valid(check: &Check<'_>, ty: &str, location_default: bool, name: &[u8]) -> bool {
    let Some(variable) = variable(check, name) else {
        return false;
    };
    let position = MetaTypeName::create(ty);
    usage_allowed(
        MetaTypeName::create(&variable.declared),
        position,
        variable.non_null_default || location_default,
    ) && (variable.non_null || !position.is_non_null())
}

/// The GraphQL `IsVariableUsageAllowed` and `AreTypesCompatible`: a nullable
/// variable may stand where a value is required only when a non-null default
/// stands behind it (`default`), and list wrapping must match.
///
/// One extension keeps what async-graphql executes: a variable that is not a
/// list may stand where a list is expected when the list's item type admits it,
/// since its value is coerced to a one-element list.
fn usage_allowed(variable: MetaTypeName<'_>, location: MetaTypeName<'_>, default: bool) -> bool {
    use MetaTypeName::{List, Named, NonNull};
    match (variable, location) {
        (NonNull(variable), NonNull(location)) | (List(variable), List(location)) => usage_allowed(
            MetaTypeName::create(variable),
            MetaTypeName::create(location),
            false,
        ),
        (variable, NonNull(location)) => {
            default && usage_allowed(variable, MetaTypeName::create(location), false)
        }
        // A single value coerced to a one-element list, its nullability kept.
        (variable, List(item)) if !variable.is_list() => {
            usage_allowed(variable, MetaTypeName::create(item), default)
        }
        (NonNull(variable), location) => {
            usage_allowed(MetaTypeName::create(variable), location, false)
        }
        (Named(variable), Named(location)) => variable == location,
        (List(_), Named(_)) | (Named(_), List(_)) => false,
    }
}

/// Check every argument of `operation` against its declared type, as execution
/// resolves it.
///
/// Each variable's value (the request's, or else the operation default) is
/// checked once against the variable's declared type; then each argument is
/// checked with each variable reference standing for that checked value. A
/// reference is accepted only where the GraphQL rule "All Variable Usages Are
/// Allowed" admits the variable's declared type (nullability, defaults and
/// list wrapping included), which async-graphql 7.2.1 does not apply, and a
/// variable that resolves to `null` or is omitted without a default only where
/// `null` is allowed. Fragments are checked once, under their type condition. Introspection fields are left to
/// async-graphql. Returns `false` when a value is invalid, a field or argument
/// is not in `registry`, or `budget` runs out.
pub(crate) fn validate_arguments(
    registry: &Registry,
    document: &ExecutableDocument,
    operation: &OperationDefinition,
    variables: &Variables,
    budget: &mut usize,
    scalar: &dyn Fn(&ScalarValidatorFn, &Value) -> bool,
) -> bool {
    use async_graphql::parser::types::OperationType;
    let mut uses = std::collections::BTreeMap::new();
    for definition in &operation.variable_definitions {
        let definition = &definition.node;
        let declared = definition.var_type.node.to_string();
        let value = variables
            .get(&definition.name.node)
            .or_else(|| definition.default_value());
        if let Some(value) = value
            && !input_value_valid(registry, &declared, value, budget, scalar)
        {
            return false;
        }
        uses.insert(
            definition.name.node.to_string(),
            VariableUse {
                declared,
                non_null_default: definition
                    .default_value()
                    .is_some_and(|value| value != &Value::Null),
                non_null: value.is_some_and(|value| value != &Value::Null),
            },
        );
    }
    let Some(root) = (match operation.ty {
        OperationType::Query => Some(registry.query_type.as_str()),
        OperationType::Mutation => registry.mutation_type.as_deref(),
        OperationType::Subscription => registry.subscription_type.as_deref(),
    }) else {
        return false;
    };
    let mut walk = ArgumentWalk {
        check: Check {
            registry,
            scalar,
            variables: Some(&uses),
        },
        budget,
        fragments: Vec::new(),
    };
    if !walk.selection_set(&operation.selection_set.node, root) {
        return false;
    }
    let mut checked = std::collections::BTreeSet::new();
    while let Some(name) = walk.fragments.pop() {
        if !checked.insert(name.clone()) {
            continue;
        }
        let Some(fragment) = document.fragments.get(&name) else {
            return false;
        };
        if !walk.selection_set(
            &fragment.node.selection_set.node,
            fragment.node.type_condition.node.on.node.as_str(),
        ) {
            return false;
        }
    }
    true
}

struct ArgumentWalk<'a, 'b> {
    check: Check<'a>,
    budget: &'b mut usize,
    fragments: Vec<async_graphql::Name>,
}
impl ArgumentWalk<'_, '_> {
    fn selection_set(&mut self, set: &SelectionSet, ty: &str) -> bool {
        for selection in &set.items {
            let valid = match &selection.node {
                Selection::Field(field) => self.field(&field.node, ty),
                Selection::FragmentSpread(spread) => {
                    self.fragments.push(spread.node.fragment_name.node.clone());
                    true
                }
                Selection::InlineFragment(fragment) => self.selection_set(
                    &fragment.node.selection_set.node,
                    fragment
                        .node
                        .type_condition
                        .as_ref()
                        .map_or(ty, |condition| condition.node.on.node.as_str()),
                ),
            };
            if !valid {
                return false;
            }
        }
        true
    }
    fn field(&mut self, field: &async_graphql::parser::types::Field, ty: &str) -> bool {
        let name = field.name.node.as_str();
        if name.starts_with("__") {
            return true;
        }
        let Some(definition) = self
            .check
            .registry
            .types
            .get(ty)
            .and_then(|ty| ty.field_by_name(name))
        else {
            return false;
        };
        for (argument, value) in &field.arguments {
            let Some(declared) = definition.args.get(argument.node.as_str()) else {
                return false;
            };
            // A variable reference stands as a placeholder naming it, checked
            // against the position it is used in; its value was checked once
            // against its declared type.
            let Ok(value) = value
                .node
                .clone()
                .into_const_with(|variable| Ok::<_, ()>(variable_placeholder(variable.as_str())))
            else {
                return false;
            };
            if !value_valid(
                &self.check,
                &declared.ty,
                declared.default_value.is_some(),
                &value,
                self.budget,
            ) {
                return false;
            }
        }
        self.selection_set(
            &field.selection_set.node,
            MetaTypeName::concrete_typename(&definition.ty),
        )
    }
}

/// Number of value nodes in `variables`, counted without recursion.
fn variable_nodes(variables: &Variables) -> usize {
    let mut nodes = 0usize;
    let mut stack = variables.values().collect::<Vec<_>>();
    while let Some(value) = stack.pop() {
        nodes = nodes.saturating_add(1);
        match value {
            Value::List(items) => stack.extend(items),
            Value::Object(fields) => stack.extend(fields.values()),
            _ => {}
        }
    }
    nodes
}

/// Number of selections, argument value nodes and variable-default nodes in
/// `document`, counted without recursion. A request handed over already parsed
/// has no query text to measure, so the check's work bound is sized from this.
fn document_nodes(document: &ExecutableDocument) -> usize {
    let mut nodes = 0usize;
    let mut sets = Vec::new();
    let mut values = Vec::<Value>::new();
    for (_, operation) in document.operations.iter() {
        sets.push(&operation.node.selection_set.node);
        for definition in &operation.node.variable_definitions {
            values.extend(
                definition
                    .node
                    .default_value
                    .as_ref()
                    .map(|v| v.node.clone()),
            );
        }
    }
    sets.extend(
        document
            .fragments
            .values()
            .map(|fragment| &fragment.node.selection_set.node),
    );
    while let Some(set) = sets.pop() {
        for selection in &set.items {
            nodes = nodes.saturating_add(1);
            match &selection.node {
                Selection::Field(field) => {
                    // Variable references count as one node each.
                    values.extend(field.node.arguments.iter().filter_map(|(_, value)| {
                        value
                            .node
                            .clone()
                            .into_const_with(|_| Ok::<_, ()>(Value::Null))
                            .ok()
                    }));
                    sets.push(&field.node.selection_set.node);
                }
                Selection::InlineFragment(fragment) => {
                    sets.push(&fragment.node.selection_set.node);
                }
                Selection::FragmentSpread(_) => {}
            }
        }
    }
    while let Some(value) = values.pop() {
        nodes = nodes.saturating_add(1);
        match value {
            Value::List(items) => values.extend(items),
            Value::Object(fields) => values.extend(fields.into_values()),
            _ => {}
        }
    }
    nodes
}

/// Refuse, before any resolver runs, an argument value that is not a valid
/// value of its declared type, with the `invalid_graphql_input` code runtime
/// GraphQL schemas use.
///
/// async-graphql 7.2.1 accepts any value where an input object is expected, and
/// leaves an omitted variable's default unchecked until execution. This
/// extension checks every argument of the selected operation after
/// async-graphql's own validation, with the values execution resolves, so a
/// scalar, list or enum sent for an input object (as a literal, a variable or a
/// variable default, at any depth) refuses the whole request before any field
/// resolves.
///
/// The check also refuses a variable whose declared type does not fit the
/// position it is used in (the GraphQL rule "All Variable Usages Are
/// Allowed"), which async-graphql 7.2.1 does not apply in either validation
/// mode, and a nullable, `null` or omitted variable standing for the member of
/// a oneof input object. Field arguments are checked; directive arguments are
/// left to async-graphql.
///
/// Under `ValidationMode::Strict`, async-graphql's default, async-graphql's own
/// argument validation substitutes the supplied variable values first, so it
/// refuses a oneof member variable whose supplied value is `null` before this
/// check runs, with its own message (`Invalid value for argument ...`) and no
/// `invalid_graphql_input` code. This check refuses that request under
/// `ValidationMode::Fast`, and refuses a nullable or omitted member variable in
/// either mode.
///
/// Generated `schema_builder` helpers install this automatically. A schema
/// built by hand from generated roots does not get the check unless it
/// installs this extension.
///
/// ```
/// use graphql_orm::graphql::input_validation::GeneratedInputValidation;
/// // Pass it to async_graphql::Schema::build(...).extension(GeneratedInputValidation).
/// let _ = GeneratedInputValidation;
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct GeneratedInputValidation;

impl ExtensionFactory for GeneratedInputValidation {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(InputValidation {
            selected: Mutex::new(None),
            parsed: Mutex::new(None),
        })
    }
}

struct InputValidation {
    selected: Mutex<Option<String>>,
    parsed: Mutex<Option<(ExecutableDocument, Variables)>>,
}

#[async_graphql::async_trait::async_trait]
impl Extension for InputValidation {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        request: async_graphql::Request,
        next: NextPrepareRequest<'_>,
    ) -> async_graphql::ServerResult<async_graphql::Request> {
        let request = next.run(ctx, request).await?;
        *self.selected.lock().map_err(|_| invalid_input_error())? = request.operation_name.clone();
        Ok(request)
    }

    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> async_graphql::ServerResult<ExecutableDocument> {
        let document = next.run(ctx, query, variables).await?;
        *self.parsed.lock().map_err(|_| invalid_input_error())? =
            Some((document.clone(), variables.clone()));
        Ok(document)
    }

    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<async_graphql::ValidationResult, Vec<ServerError>> {
        let result = next.run(ctx).await?;
        let selected = self
            .selected
            .lock()
            .map_err(|_| vec![invalid_input_error()])?
            .clone();
        let parsed = self
            .parsed
            .lock()
            .map_err(|_| vec![invalid_input_error()])?;
        let Some((document, variables)) = parsed.as_ref() else {
            return Err(vec![invalid_input_error()]);
        };
        // An unknown or ambiguous operation name is async-graphql's to report.
        let Some(operation) = selected_operation(document, selected.as_deref()) else {
            return Ok(result);
        };
        // The walk visits each document and variable node once per type
        // wrapper; this bound is far above any request it can accept. It is
        // sized from the parsed document, which a request may supply without
        // query text.
        let mut budget = document_nodes(document)
            .saturating_add(variable_nodes(variables))
            .saturating_mul(16)
            .saturating_add(1024);
        if validate_arguments(
            &ctx.schema_env.registry,
            document,
            operation,
            variables,
            &mut budget,
            &|valid, value| valid(value),
        ) {
            Ok(result)
        } else {
            Err(vec![invalid_input_error()])
        }
    }
}
