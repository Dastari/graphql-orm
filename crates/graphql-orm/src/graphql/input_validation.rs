//! Argument value checks shared by generated static schemas and runtime
//! GraphQL schemas.
//!
//! async-graphql 7.2.1 validates arguments with two gaps: it accepts any value
//! for an input object (a scalar, list or enum where an object is expected), and
//! it skips an argument whose value names a variable the request omits, then
//! applies the variable's default at execution without checking it. A value
//! that is not an object then fails later, while its field resolves, with an
//! uncoded coercion message, after earlier fields of the same operation may
//! already have run.
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

/// The value [`input_value_valid`] accepts at any type: it stands for an
/// argument's variable reference, whose value is checked once against the
/// variable's declared type. A GraphQL literal is never binary, so client text
/// cannot produce it.
const VALIDATED_VARIABLE: &[u8] = b"\0graphql-orm validated variable\0";
fn validated_variable() -> Value {
    Value::Binary(VALIDATED_VARIABLE.to_vec().into())
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
    let Some(remaining) = budget.checked_sub(1) else {
        return false;
    };
    *budget = remaining;
    if matches!(value, Value::Binary(bytes) if bytes.as_ref() == VALIDATED_VARIABLE) {
        return true;
    }
    match MetaTypeName::create(ty) {
        MetaTypeName::NonNull(inner) => {
            value != &Value::Null && input_value_valid(registry, inner, value, budget, scalar)
        }
        MetaTypeName::List(inner) => match value {
            Value::Null => true,
            Value::List(items) => items
                .iter()
                .all(|item| input_value_valid(registry, inner, item, budget, scalar)),
            // GraphQL coerces a single value to a one-element list.
            item => input_value_valid(registry, inner, item, budget, scalar),
        },
        MetaTypeName::Named(name) => {
            if value == &Value::Null {
                return true;
            }
            match registry.types.get(name) {
                Some(MetaType::Scalar { is_valid, .. }) => {
                    is_valid.as_ref().is_none_or(|valid| scalar(valid, value))
                }
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
                    if *oneof && (values.len() != 1 || values.values().any(|v| v == &Value::Null)) {
                        return false;
                    }
                    values
                        .keys()
                        .all(|name| input_fields.contains_key(name.as_str()))
                        && input_fields.values().all(|field| {
                            match values.get(field.name.as_str()) {
                                Some(value) => {
                                    input_value_valid(registry, &field.ty, value, budget, scalar)
                                }
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

/// Check every argument of `operation` against its declared type, as execution
/// resolves it.
///
/// Each variable's value (the request's, or else the operation default) is
/// checked once against the variable's declared type; then each argument is
/// checked with its variable references standing for those checked values, and
/// an omitted variable without a default standing for `null`. Fragments are
/// checked once, under their type condition. Introspection fields are left to
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
    let mut resolved = std::collections::BTreeMap::new();
    for definition in &operation.variable_definitions {
        let name = &definition.node.name.node;
        let value = variables
            .get(name)
            .or_else(|| definition.node.default_value());
        if let Some(value) = value {
            if !input_value_valid(
                registry,
                &definition.node.var_type.node.to_string(),
                value,
                budget,
                scalar,
            ) {
                return false;
            }
            resolved.insert(name.clone(), value != &Value::Null);
        }
    }
    let Some(root) = (match operation.ty {
        OperationType::Query => Some(registry.query_type.as_str()),
        OperationType::Mutation => registry.mutation_type.as_deref(),
        OperationType::Subscription => registry.subscription_type.as_deref(),
    }) else {
        return false;
    };
    let mut walk = ArgumentWalk {
        registry,
        variables: &resolved,
        budget,
        scalar,
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
    registry: &'a Registry,
    /// Whether each variable with a value resolves to a non-null value.
    variables: &'a std::collections::BTreeMap<async_graphql::Name, bool>,
    budget: &'b mut usize,
    scalar: &'a dyn Fn(&ScalarValidatorFn, &Value) -> bool,
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
            let Ok(value) = value.node.clone().into_const_with(|variable| {
                Ok::<_, ()>(match self.variables.get(&variable) {
                    Some(true) => validated_variable(),
                    None | Some(false) => Value::Null,
                })
            }) else {
                return false;
            };
            if !input_value_valid(
                self.registry,
                &declared.ty,
                &value,
                self.budget,
                self.scalar,
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
/// Generated `schema_builder` helpers install this automatically. A schema
/// built by hand from generated roots should install it too.
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
    parsed: Mutex<Option<(ExecutableDocument, Variables, usize)>>,
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
            Some((document.clone(), variables.clone(), query.len()));
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
        let Some((document, variables, query_bytes)) = parsed.as_ref() else {
            return Err(vec![invalid_input_error()]);
        };
        // An unknown or ambiguous operation name is async-graphql's to report.
        let Some(operation) = selected_operation(document, selected.as_deref()) else {
            return Ok(result);
        };
        // The walk visits each literal and variable node once per type
        // wrapper; this bound is far above any request it can accept.
        let mut budget = query_bytes
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
