use super::*;
use crate::graphql::orm::*;
use async_graphql::{
    Value, Variables,
    parser::types::{ExecutableDocument, Field, Selection, SelectionSet},
};
use std::collections::BTreeMap;

pub(super) fn invalid() -> RuntimeGraphqlError {
    RuntimeGraphqlError::new("invalid_graphql_input")
}
pub(super) fn query_error(e: RuntimeQueryError) -> RuntimeGraphqlError {
    RuntimeGraphqlError::new(e.code().as_str())
}
fn record_error(e: RuntimeRecordError) -> RuntimeGraphqlError {
    RuntimeGraphqlError::new(e.code().as_str())
}

#[derive(Clone)]
pub(super) struct Plan {
    pub module: RuntimeGraphqlModule,
    pub collection: RuntimeCollectionHandle,
    pub response_key: String,
    pub field_name: String,
    pub filter_intent: FilterIntent,
    pub fields: Vec<RuntimeFieldHandle>,
    pub projection: RuntimeProjection,
    pub predicate: Option<RuntimePredicate>,
    pub order: RuntimeOrder,
    pub requested_order: Vec<RuntimeOrderInput>,
    pub order_was_supplied: bool,
    pub page: RuntimePageRequest,
    pub count: bool,
    pub relation: Option<RuntimeRelationHandle>,
    pub children: Vec<Plan>,
    pub projected: Vec<super::projection::ProjectedPlan>,
}
impl Plan {
    pub fn relations(&self) -> Vec<RuntimeRelationHandle> {
        let mut result = Vec::new();
        for child in &self.children {
            if let Some(r) = &child.relation {
                if !result.contains(r) {
                    result.push(r.clone());
                }
            }
        }
        result
    }
}

pub(super) struct Planner<'a> {
    pub document: &'a ExecutableDocument,
    pub variables: &'a Variables,
    pub limits: RuntimeGraphqlLimits,
    pub expanded: usize,
    pub cost: RuntimeGraphqlCost,
}
impl Planner<'_> {
    pub(super) fn resolve(
        &self,
        v: &async_graphql::parser::types::Field,
    ) -> Result<BTreeMap<String, Value>, RuntimeGraphqlError> {
        v.arguments
            .iter()
            .map(|(n, v)| {
                Ok((
                    n.node.to_string(),
                    v.node.clone().into_const_with(|name| {
                        self.variables.get(&name).cloned().ok_or_else(invalid)
                    })?,
                ))
            })
            .collect()
    }
    pub fn selections(
        &mut self,
        set: &SelectionSet,
        ty: &str,
        depth: usize,
    ) -> Result<Vec<Field>, RuntimeGraphqlError> {
        if depth > self.limits.max_depth {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        // A response-key group keeps its first occurrence in document order.
        let mut fields: async_graphql::indexmap::IndexMap<String, Field> =
            async_graphql::indexmap::IndexMap::new();
        for selection in &set.items {
            self.expanded = self.expanded.checked_add(1).ok_or_else(invalid)?;
            if self.expanded > self.limits.max_selections {
                return Err(RuntimeGraphqlError::new("cost_exceeded"));
            }
            let mut skip = false;
            for directive in selection.node.directives() {
                let name = directive.node.name.node.as_str();
                if matches!(name, "skip" | "include") {
                    let arg = directive
                        .node
                        .arguments
                        .iter()
                        .find(|(n, _)| n.node == "if")
                        .ok_or_else(invalid)?;
                    let value =
                        arg.1.node.clone().into_const_with(|n| {
                            self.variables.get(&n).cloned().ok_or_else(invalid)
                        })?;
                    let Value::Boolean(value) = value else {
                        return Err(invalid());
                    };
                    skip |= if name == "skip" { value } else { !value };
                }
            }
            if skip {
                continue;
            }
            let expanded = match &selection.node {
                Selection::Field(f) => vec![f.node.clone()],
                Selection::FragmentSpread(f) => {
                    let fragment = self
                        .document
                        .fragments
                        .get(&f.node.fragment_name.node)
                        .ok_or_else(invalid)?;
                    if fragment.node.type_condition.node.on.node.as_str() != ty {
                        continue;
                    }
                    self.selections(&fragment.node.selection_set.node, ty, depth + 1)?
                }
                Selection::InlineFragment(f) => {
                    if f.node
                        .type_condition
                        .as_ref()
                        .is_some_and(|t| t.node.on.node.as_str() != ty)
                    {
                        continue;
                    }
                    self.selections(&f.node.selection_set.node, ty, depth + 1)?
                }
            };
            for field in expanded {
                let key = field.response_key().node.to_string();
                if let Some(existing) = fields.get_mut(&key) {
                    existing
                        .selection_set
                        .node
                        .items
                        .extend(field.selection_set.node.items);
                } else {
                    fields.insert(key, field);
                }
            }
        }
        Ok(fields.into_values().collect())
    }
    pub fn collection(
        &mut self,
        module: &RuntimeGraphqlModule,
        c: &RuntimeCollection,
        field: &Field,
        relation: Option<RuntimeRelationHandle>,
        parents: usize,
        depth: usize,
    ) -> Result<Plan, RuntimeGraphqlError> {
        if depth > self.limits.max_depth {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        let collection = module
            .schema
            .resolve_collection(&c.id)
            .map_err(record_error)?;
        let args = self.resolve(field)?;
        let mut filter_intent = FilterIntent::default();
        let predicate = match args.get("where") {
            Some(value) if value != &Value::Null => Some(filter(
                &module.schema,
                &collection,
                c,
                value,
                self.limits,
                0,
                &mut filter_intent,
            )?),
            _ => None,
        };
        let mut input = Vec::new();
        if let Some(orders) = args.get("orderBy").filter(|v| *v != &Value::Null) {
            let orders = list_values(orders);
            if orders.len() > self.limits.query.max_order_terms {
                return Err(RuntimeGraphqlError::new("cost_exceeded"));
            }
            for order in orders {
                let Value::Object(values) = order else {
                    return Err(invalid());
                };
                let f = enum_text(values.get("field").ok_or_else(invalid)?)?;
                let field = c
                    .fields
                    .iter()
                    .find(|v| v.api_name == f)
                    .ok_or_else(invalid)?;
                let direction = match values.get("direction") {
                    None | Some(Value::Null) => RuntimeOrderDirection::Asc,
                    Some(v) if enum_text(v)? == "ASC" => RuntimeOrderDirection::Asc,
                    Some(v) if enum_text(v)? == "DESC" => RuntimeOrderDirection::Desc,
                    _ => return Err(invalid()),
                };
                let nulls = match values.get("nulls") {
                    None | Some(Value::Null) => RuntimeNullPlacement::Last,
                    Some(v) if enum_text(v)? == "FIRST" => RuntimeNullPlacement::First,
                    Some(v) if enum_text(v)? == "LAST" => RuntimeNullPlacement::Last,
                    _ => return Err(invalid()),
                };
                input.push(RuntimeOrderInput {
                    field: module
                        .schema
                        .resolve_field(&collection, &field.id)
                        .map_err(record_error)?,
                    direction,
                    nulls,
                });
            }
        }
        let requested_order = input.clone();
        let order_was_supplied = args.get("orderBy").is_some_and(|v| v != &Value::Null);
        let order = module
            .schema
            .runtime_order(
                &collection,
                if args.get("orderBy").is_some_and(|v| v != &Value::Null) {
                    Some(input)
                } else {
                    None
                },
                self.limits.query,
            )
            .map_err(query_error)?;
        let to_one = relation
            .as_ref()
            .is_some_and(|r| r.cardinality() == RelationCardinality::One);
        let number = |name: &str| -> Result<Option<i64>, RuntimeGraphqlError> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(v)) => v.as_i64().map(Some).ok_or_else(invalid),
                _ => Err(invalid()),
            }
        };
        let text = |name: &str| -> Result<Option<String>, RuntimeGraphqlError> {
            match args.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(v)) => Ok(Some(v.clone())),
                _ => Err(invalid()),
            }
        };
        let first = number("first")?;
        let last = number("last")?;
        let after = text("after")?;
        let before = text("before")?;
        if (first.is_some() || after.is_some()) && (last.is_some() || before.is_some()) {
            return Err(invalid());
        }
        let size = if to_one {
            1
        } else {
            first
                .or(last)
                .unwrap_or(i64::from(self.limits.query.default_page_size))
        };
        if size <= 0 || size > i64::from(self.limits.query.max_page_size) {
            return Err(invalid());
        }
        let page = if last.is_some() || before.is_some() {
            RuntimePageRequest::last(size, before)
        } else {
            RuntimePageRequest::first(size, after)
        };
        let object = module.object_name(c);
        let mut count = false;
        let mut node_set = SelectionSet::default();
        if to_one {
            node_set = field.selection_set.node.clone();
        } else {
            for f in self.selections(
                &field.selection_set.node,
                &format!("{object}Connection"),
                depth + 1,
            )? {
                if f.name.node == "totalCount" {
                    count = true;
                }
                if f.name.node == "edges" {
                    for edge in
                        self.selections(&f.selection_set.node, &format!("{object}Edge"), depth + 1)?
                    {
                        if edge.name.node == "node" {
                            node_set.items.extend(edge.selection_set.node.items);
                        }
                    }
                }
            }
        }
        let protected = !to_one
            && module.options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption;
        let size = usize::try_from(size).map_err(|_| invalid())?;
        let cost = if relation.is_some() {
            RuntimeGraphqlCost::for_relation_layer(parents, size, count, protected)?
        } else {
            RuntimeGraphqlCost::for_root_page(size, count, protected)?
        };
        let nodes = cost.materialized_nodes;
        self.cost = self.cost.checked_add(cost)?;
        if self.cost.statements > self.limits.max_statements
            || self.cost.materialized_nodes > self.limits.max_materialized_nodes
            || self.cost.crypto_calls > self.limits.max_crypto_calls
            || self.cost.relation_groups > self.limits.relation.max_compatible_groups
        {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        let mut fields = Vec::new();
        let mut children = Vec::new();
        let mut projected_fields = Vec::new();
        for selected in self.selections(&node_set, &object, depth + 1)? {
            if selected.name.node == "__typename" {
                continue;
            }
            if let Some(f) = c
                .fields
                .iter()
                .find(|f| f.api_name == selected.name.node.as_str())
            {
                let handle = module
                    .schema
                    .resolve_field(&collection, &f.id)
                    .map_err(record_error)?;
                if let Some(binding) = module.projection(&f.id) {
                    let names = self.projected_selection(binding, &selected, depth + 1)?;
                    super::projection::merge_projected(
                        &mut projected_fields,
                        handle.clone(),
                        &f.api_name,
                        binding,
                        names,
                    );
                }
                if !fields.contains(&handle) {
                    fields.push(handle);
                }
            } else if let Some(r) = c
                .relations
                .iter()
                .find(|r| r.api_name == selected.name.node.as_str())
            {
                let handle = module
                    .schema
                    .resolve_relation(&collection, &r.id)
                    .map_err(record_error)?;
                let target = module
                    .schema
                    .schema()
                    .collections
                    .iter()
                    .find(|c| c.id == r.target)
                    .ok_or_else(invalid)?;
                if nodes > self.limits.relation.max_parents {
                    return Err(RuntimeGraphqlError::new("cost_exceeded"));
                }
                children.push(self.collection(
                    module,
                    target,
                    &selected,
                    Some(handle),
                    nodes,
                    depth + 1,
                )?);
            } else {
                return Err(invalid());
            }
        }
        if !projected_fields.is_empty() {
            self.cost.projection_calls = self
                .cost
                .projection_calls
                .checked_add(projected_fields.len())
                .ok_or_else(invalid)?;
            if self.cost.projection_calls > self.limits.projection.max_calls
                || nodes > self.limits.projection.max_values_per_call
            {
                return Err(RuntimeGraphqlError::new("cost_exceeded"));
            }
        }
        // A count-only/relationship-only request still needs an internal row anchor.
        let projected = if fields.is_empty() {
            vec![
                module
                    .schema
                    .resolve_field(&collection, &c.primary_key[0])
                    .map_err(record_error)?,
            ]
        } else {
            fields.clone()
        };
        let projection = module
            .schema
            .resolve_projection(&collection, &projected)
            .map_err(record_error)?;
        Ok(Plan {
            module: module.clone(),
            collection,
            response_key: field.response_key().node.to_string(),
            field_name: field.name.node.to_string(),
            filter_intent,
            fields,
            projection,
            predicate,
            order,
            requested_order,
            order_was_supplied,
            page,
            count,
            relation,
            children,
            projected: projected_fields,
        })
    }
    /// Validate a projected field's object subselection; returns selected field names.
    pub(super) fn projected_selection(
        &mut self,
        binding: &super::projection::ProjectionBinding,
        field: &Field,
        depth: usize,
    ) -> Result<Vec<String>, RuntimeGraphqlError> {
        let object = &binding.contract.object;
        let mut names: Vec<String> = Vec::new();
        for selected in self.selections(&field.selection_set.node, object.name(), depth)? {
            if selected.name.node == "__typename" {
                continue;
            }
            if !selected.arguments.is_empty() || object.field(&selected.name.node).is_none() {
                return Err(invalid());
            }
            if !names.iter().any(|name| name == selected.name.node.as_str()) {
                names.push(selected.name.node.to_string());
            }
        }
        Ok(names)
    }
}

#[derive(Clone, Default)]
pub(super) struct FilterIntent {
    pub operations: Vec<(RuntimeFieldHandle, &'static str)>,
    pub logical: Vec<&'static str>,
}

/// Lower a host-validated structural Value using the canonical runtime filter path.
///
/// The caller must validate GraphQL coercion and duplicate wire keys BEFORE
/// conversion to Value. Value maps cannot expose erased duplicate occurrences.
/// This function preserves every occurrence PRESENT in the supplied value; it
/// proves neither transport provenance nor principal/policy authority.
/// # Errors
/// Rejects invalid handles/input, unsupported operators and bounded input costs.
pub fn prepare_runtime_filter(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    input: &Value,
    limits: RuntimeGraphqlLimits,
) -> Result<CheckedRuntimeFilter, RuntimeFilterError> {
    prepare_filter_inner(schema, collection, input, limits, false)
}
fn prepare_filter_inner(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    input: &Value,
    limits: RuntimeGraphqlLimits,
    canonical_execution: bool,
) -> Result<CheckedRuntimeFilter, RuntimeFilterError> {
    if canonical_execution && limits.query.max_predicate_depth == 0 {
        return Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::LimitExceeded,
        ));
    }
    let current = schema
        .resolve_collection(collection.id())
        .map_err(|_| RuntimeFilterError::new(RuntimeFilterErrorCode::InvalidHandle))?;
    if &current != collection {
        return Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::SchemaMismatch,
        ));
    }
    let c = schema
        .schema()
        .collections
        .iter()
        .find(|c| &c.id == collection.id())
        .ok_or(RuntimeFilterError::new(
            RuntimeFilterErrorCode::InvalidHandle,
        ))?;
    // Bound the supplied tree before allocating decoded literals or clones.
    if !canonical_execution {
        let mut pending = vec![(input, 0usize)];
        let mut nodes = 0usize;
        let mut bytes = 0usize;
        while let Some((value, depth)) = pending.pop() {
            nodes = nodes.checked_add(1).ok_or(RuntimeFilterError::new(
                RuntimeFilterErrorCode::LimitExceeded,
            ))?;
            if nodes > limits.max_variable_nodes || depth > limits.max_depth {
                return Err(RuntimeFilterError::new(
                    RuntimeFilterErrorCode::LimitExceeded,
                ));
            }
            let size =
                match value {
                    Value::String(s) => s.len(),
                    Value::Binary(s) => s.len(),
                    Value::Enum(s) => s.as_str().len(),
                    Value::Object(entries) => {
                        if entries.len()
                            > limits
                                .max_variable_nodes
                                .saturating_sub(nodes)
                                .saturating_sub(pending.len())
                        {
                            return Err(RuntimeFilterError::new(
                                RuntimeFilterErrorCode::LimitExceeded,
                            ));
                        }
                        for (k, v) in entries {
                            bytes = bytes.checked_add(k.as_str().len()).ok_or(
                                RuntimeFilterError::new(RuntimeFilterErrorCode::LimitExceeded),
                            )?;
                            pending.push((v, depth + 1));
                        }
                        0
                    }
                    Value::List(values) => {
                        if values.len()
                            > limits
                                .max_variable_nodes
                                .saturating_sub(nodes)
                                .saturating_sub(pending.len())
                        {
                            return Err(RuntimeFilterError::new(
                                RuntimeFilterErrorCode::LimitExceeded,
                            ));
                        }
                        for v in values {
                            pending.push((v, depth + 1));
                        }
                        0
                    }
                    _ => 32,
                };
            bytes = bytes.checked_add(size).ok_or(RuntimeFilterError::new(
                RuntimeFilterErrorCode::LimitExceeded,
            ))?;
            if bytes > limits.max_variable_bytes {
                return Err(RuntimeFilterError::new(
                    RuntimeFilterErrorCode::LimitExceeded,
                ));
            }
        }
    }
    let builder = schema
        .runtime_filter_builder(collection, limits.query)?
        .with_graphql_literal_limits(
            limits.scalars.max_text_bytes,
            limits.scalars.max_decoded_bytes,
        );
    let builder = if canonical_execution {
        builder.for_canonical_execution()
    } else {
        builder
    };
    let root = checked_filter(&builder, c, input, limits, 0)?;
    builder.finish(root)
}

pub(super) fn filter(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    _c: &RuntimeCollection,
    value: &Value,
    limits: RuntimeGraphqlLimits,
    _depth: usize,
    intent: &mut FilterIntent,
) -> Result<RuntimePredicate, RuntimeGraphqlError> {
    let checked = prepare_filter_inner(schema, collection, value, limits, true)
        .map_err(|e| RuntimeGraphqlError::new(e.graphql_code()))?;
    struct Flat<'a>(&'a mut FilterIntent);
    impl RuntimeFilterIntentVisitor for Flat<'_> {
        fn enter_boolean(&mut self, k: RuntimeFilterBoolean, ignored: bool) {
            if ignored {
                return;
            }
            let name = match k {
                RuntimeFilterBoolean::ImplicitAnd => return,
                RuntimeFilterBoolean::And => "and",
                RuntimeFilterBoolean::Or => "or",
                RuntimeFilterBoolean::Not => "not",
            };
            if !self.0.logical.contains(&name) {
                self.0.logical.push(name);
            }
        }
        fn leave_boolean(&mut self, _: RuntimeFilterBoolean, _: bool) {}
        fn field(&mut self, f: &RuntimeFieldHandle, op: RuntimeFilterOperator, ignored: bool) {
            if ignored {
                return;
            }
            let name = match op {
                RuntimeFilterOperator::Scalar(op) => match op {
                    RuntimeScalarOperator::Eq => "eq",
                    RuntimeScalarOperator::Ne => "ne",
                    RuntimeScalarOperator::Lt => "lt",
                    RuntimeScalarOperator::Lte => "lte",
                    RuntimeScalarOperator::Gt => "gt",
                    RuntimeScalarOperator::Gte => "gte",
                    RuntimeScalarOperator::Contains => "contains",
                    RuntimeScalarOperator::StartsWith => "startsWith",
                    RuntimeScalarOperator::EndsWith => "endsWith",
                },
                RuntimeFilterOperator::List(RuntimeListOperator::In) => "in",
                RuntimeFilterOperator::List(RuntimeListOperator::NotIn) => "notIn",
                RuntimeFilterOperator::IsNull(true) => "isNull",
                RuntimeFilterOperator::IsNull(false) => "isNotNull",
                _ => return,
            };
            self.0.operations.push((f.clone(), name));
        }
    }
    checked.visit_intent(&mut Flat(intent));
    Ok(checked.predicate().clone())
}
fn checked_filter(
    builder: &RuntimeFilterBuilder,
    c: &RuntimeCollection,
    value: &Value,
    limits: RuntimeGraphqlLimits,
    depth: usize,
) -> Result<RuntimeFilterNode, RuntimeFilterError> {
    if depth >= limits.query.max_predicate_depth {
        return Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::LimitExceeded,
        ));
    }
    let Value::Object(entries) = value else {
        return Err(RuntimeFilterError::new(
            RuntimeFilterErrorCode::InvalidInput,
        ));
    };
    let mut nodes = Vec::new();
    for (name, value) in entries {
        let logical = match name.as_str() {
            "and" => Some(RuntimeFilterBoolean::And),
            "or" => Some(RuntimeFilterBoolean::Or),
            "not" => Some(RuntimeFilterBoolean::Not),
            _ => None,
        };
        if let Some(kind) = logical {
            if value == &Value::Null {
                nodes.push(builder.ignored_boolean(kind)?);
                continue;
            }
            if kind == RuntimeFilterBoolean::Not {
                nodes.push(builder.not(checked_filter(builder, c, value, limits, depth + 1)?)?)
            } else {
                let values = list_values(value);
                if values.len() > limits.query.max_predicate_nodes {
                    return Err(RuntimeFilterError::new(
                        RuntimeFilterErrorCode::LimitExceeded,
                    ));
                }
                let children = values
                    .into_iter()
                    .map(|v| checked_filter(builder, c, v, limits, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                nodes.push(if kind == RuntimeFilterBoolean::And {
                    builder.and(children)?
                } else {
                    builder.or(children)?
                });
            }
            continue;
        }
        let f = c
            .fields
            .iter()
            .find(|f| f.api_name == name.as_str())
            .ok_or(RuntimeFilterError::new(
                RuntimeFilterErrorCode::InvalidInput,
            ))?;
        // Resolve through the already checked builder's schema, never physical names.
        let field = builder.resolve_graphql_field(&f.id)?;
        if value == &Value::Null {
            nodes.push(builder.ignored_field(&field, RuntimeFilterOperator::Field)?);
            continue;
        }
        let Value::Object(ops) = value else {
            return Err(RuntimeFilterError::new(
                RuntimeFilterErrorCode::InvalidInput,
            ));
        };
        if ops.is_empty() {
            nodes.push(builder.ignored_field(&field, RuntimeFilterOperator::Field)?);
        }
        for (op, value) in ops {
            let operation = match op.as_str() {
                "isNull" => RuntimeFilterOperator::IsNull(value != &Value::Boolean(false)),
                "in" => RuntimeFilterOperator::List(RuntimeListOperator::In),
                "notIn" => RuntimeFilterOperator::List(RuntimeListOperator::NotIn),
                _ => RuntimeFilterOperator::Scalar(match op.as_str() {
                    "eq" => RuntimeScalarOperator::Eq,
                    "ne" => RuntimeScalarOperator::Ne,
                    "lt" => RuntimeScalarOperator::Lt,
                    "lte" => RuntimeScalarOperator::Lte,
                    "gt" => RuntimeScalarOperator::Gt,
                    "gte" => RuntimeScalarOperator::Gte,
                    "contains" => RuntimeScalarOperator::Contains,
                    "startsWith" => RuntimeScalarOperator::StartsWith,
                    "endsWith" => RuntimeScalarOperator::EndsWith,
                    _ => {
                        return Err(RuntimeFilterError::new(
                            RuntimeFilterErrorCode::InvalidInput,
                        ));
                    }
                }),
            };
            if value == &Value::Null {
                nodes.push(builder.ignored_field(&field, operation)?);
                continue;
            }
            let decode = |v| {
                scalar::decode(f.value_kind, v, limits.scalars)
                    .map_err(|_| RuntimeFilterError::new(RuntimeFilterErrorCode::InvalidInput))
            };
            nodes.push(match operation {
                RuntimeFilterOperator::IsNull(_) => {
                    let Value::Boolean(v) = value else {
                        return Err(RuntimeFilterError::new(
                            RuntimeFilterErrorCode::InvalidInput,
                        ));
                    };
                    builder.is_null(&field, *v)?
                }
                RuntimeFilterOperator::List(op) => {
                    let values = list_values(value);
                    if values.len() > limits.query.max_values_per_list {
                        return Err(RuntimeFilterError::new(
                            RuntimeFilterErrorCode::LimitExceeded,
                        ));
                    }
                    builder.membership(
                        &field,
                        op,
                        values
                            .into_iter()
                            .map(decode)
                            .collect::<Result<Vec<_>, _>>()?,
                    )?
                }
                RuntimeFilterOperator::Scalar(op) => builder.compare(&field, op, decode(value)?)?,
                _ => {
                    return Err(RuntimeFilterError::new(
                        RuntimeFilterErrorCode::InvalidInput,
                    ));
                }
            });
        }
    }
    builder.implicit_and(nodes)
}

fn list_values(value: &Value) -> Vec<&Value> {
    match value {
        Value::List(values) => values.iter().collect(),
        value => vec![value],
    }
}
fn enum_text(value: &Value) -> Result<&str, RuntimeGraphqlError> {
    match value {
        Value::String(v) => Ok(v),
        Value::Enum(v) => Ok(v.as_str()),
        _ => Err(invalid()),
    }
}

#[cfg(test)]
mod checked_compatibility_tests {
    use super::*;
    fn schema() -> ValidatedRuntimeSchema {
        RuntimeSchema {
            format_version: 1,
            collections: vec![RuntimeCollection {
                id: CollectionId::new("items").unwrap(),
                api_type_name: "Item".into(),
                api_plural_name: "Items".into(),
                physical_table: "items".into(),
                primary_key: vec![FieldId::new("id").unwrap()],
                append_only: false,
                retention_purge: false,
                description: None,
                classification: None,
                fields: [
                    ("id", RuntimeValueKind::Integer, false),
                    ("text", RuntimeValueKind::String, true),
                    ("flag", RuntimeValueKind::Boolean, true),
                    ("bytes", RuntimeValueKind::Bytes, true),
                    ("score", RuntimeValueKind::Float, true),
                    ("hidden", RuntimeValueKind::String, false),
                ]
                .into_iter()
                .map(|(n, k, filterable)| RuntimeField {
                    description: None,
                    classification: None,
                    export: None,
                    id: FieldId::new(n).unwrap(),
                    api_name: n.into(),
                    physical_column: n.into(),
                    value_kind: k,
                    nullable: false,
                    unique: n == "id",
                    filterable,
                    sortable: n == "id",
                    generated: false,
                    default: None,
                    format: None,
                })
                .collect(),
                relations: vec![],
                indexes: vec![],
                composite_unique: vec![],
                default_order: vec![],
            }],
        }
        .validate()
        .unwrap()
    }
    fn legacy(
        s: &ValidatedRuntimeSchema,
        input: Value,
        limits: RuntimeGraphqlLimits,
    ) -> Result<RuntimePredicate, RuntimeGraphqlError> {
        let c = &s.schema().collections[0];
        let h = s.resolve_collection(&c.id).unwrap();
        filter(s, &h, c, &input, limits, 0, &mut FilterIntent::default())
    }
    #[test]
    // Internal old-filter compatibility; these are not claims that unknown or
    // unfilterable wire fields pass GraphQL schema validation.
    fn legacy_null_skip_and_empty_operator_costs_remain_unchanged() {
        let s = schema();
        let limits = RuntimeGraphqlLimits {
            query: RuntimeQueryLimits {
                max_predicate_nodes: 1,
                max_predicate_depth: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        for value in [
            async_graphql::value!({"text":{"eq":null}}),
            async_graphql::value!({"and":null}),
            async_graphql::value!({"text":{}}),
            async_graphql::value!({"hidden":{"eq":null}}),
            async_graphql::value!({"flag":{"lt":null}}),
            async_graphql::value!({"bytes":{"contains":null}}),
        ] {
            assert!(legacy(&s, value, limits).is_ok());
        }
    }
    #[test]
    fn legacy_query_errors_configured_literals_and_postgres_suffix_budget_survive() {
        let s = schema();
        let limits = RuntimeGraphqlLimits::default();
        assert_eq!(
            legacy(&s, async_graphql::value!({"hidden":{"eq":"x"}}), limits)
                .unwrap_err()
                .code(),
            "invalid_filter"
        );
        assert_eq!(
            legacy(&s, async_graphql::value!({"flag":{"lt":true}}), limits)
                .unwrap_err()
                .code(),
            "unsupported_operator"
        );
        let mut limits = limits;
        limits.scalars.max_text_bytes = 128 * 1024;
        limits.max_variable_bytes = 256 * 1024;
        assert!(
            legacy(
                &s,
                async_graphql::value!({"text":{"eq":"x".repeat(70*1024)}}),
                limits
            )
            .is_ok()
        );
        limits.query.max_bind_parameters = 1;
        assert!(legacy(&s, async_graphql::value!({"text":{"endsWith":"x"}}), limits).is_ok());
        let small = RuntimeGraphqlLimits {
            max_variable_bytes: 256,
            ..Default::default()
        };
        let input =
            async_graphql::value!({"score":{"in":[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8,0.9,1.1]}});
        let variables = Variables::from_json(
            serde_json::json!({"where":serde_json::to_value(&input).unwrap()}),
        );
        super::super::execution::bounded_variables(&variables, small).unwrap();
        assert!(legacy(&s, input, small).is_ok());
        // This preserves legacy logical admission. PostgreSQL emits one native
        // suffix bind; SQLite emits two. Only the new public checked artifact
        // reserves the worst-case emitted count here.
    }
}
