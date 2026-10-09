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
    fn resolve(
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
        let mut fields: BTreeMap<String, Field> = BTreeMap::new();
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
        let predicate = match args.get("where") {
            Some(value) if value != &Value::Null => Some(filter(
                &module.schema,
                &collection,
                c,
                value,
                self.limits,
                0,
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
        })
    }
}

fn filter(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    c: &RuntimeCollection,
    value: &Value,
    limits: RuntimeGraphqlLimits,
    depth: usize,
) -> Result<RuntimePredicate, RuntimeGraphqlError> {
    if depth >= limits.query.max_predicate_depth {
        return Err(RuntimeGraphqlError::new("cost_exceeded"));
    }
    let Value::Object(entries) = value else {
        return Err(invalid());
    };
    let mut predicates = Vec::new();
    for (name, value) in entries {
        if value == &Value::Null {
            continue;
        }
        match name.as_str() {
            "and" | "or" => {
                let values = list_values(value);
                if values.len() > limits.query.max_predicate_nodes {
                    return Err(RuntimeGraphqlError::new("cost_exceeded"));
                }
                let parts = values
                    .into_iter()
                    .map(|v| filter(schema, collection, c, v, limits, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                predicates.push(
                    if name == "and" {
                        schema.runtime_and(collection, parts, limits.query)
                    } else {
                        schema.runtime_or(collection, parts, limits.query)
                    }
                    .map_err(query_error)?,
                );
            }
            "not" => predicates.push(
                schema
                    .runtime_not(
                        collection,
                        filter(schema, collection, c, value, limits, depth + 1)?,
                        limits.query,
                    )
                    .map_err(query_error)?,
            ),
            _ => {
                let f = c
                    .fields
                    .iter()
                    .find(|f| f.api_name == name.as_str())
                    .ok_or_else(invalid)?;
                let handle = schema
                    .resolve_field(collection, &f.id)
                    .map_err(record_error)?;
                let Value::Object(ops) = value else {
                    return Err(invalid());
                };
                for (op, value) in ops {
                    if value == &Value::Null {
                        continue;
                    }
                    let predicate = match op.as_str() {
                        "isNull" => {
                            let Value::Boolean(v) = value else {
                                return Err(invalid());
                            };
                            schema.runtime_is_null(collection, &handle, *v, limits.query)
                        }
                        "in" | "notIn" => {
                            let values = list_values(value);
                            if values.len() > limits.query.max_values_per_list {
                                return Err(RuntimeGraphqlError::new("cost_exceeded"));
                            }
                            let values = values
                                .into_iter()
                                .map(|v| scalar::decode(f.value_kind, v, limits.scalars))
                                .collect::<Result<Vec<_>, _>>()?;
                            schema.runtime_list(
                                collection,
                                &handle,
                                if op == "in" {
                                    RuntimeListOperator::In
                                } else {
                                    RuntimeListOperator::NotIn
                                },
                                values,
                                limits.query,
                            )
                        }
                        _ => {
                            let op = match op.as_str() {
                                "eq" => RuntimeScalarOperator::Eq,
                                "ne" => RuntimeScalarOperator::Ne,
                                "lt" => RuntimeScalarOperator::Lt,
                                "lte" => RuntimeScalarOperator::Lte,
                                "gt" => RuntimeScalarOperator::Gt,
                                "gte" => RuntimeScalarOperator::Gte,
                                "contains" => RuntimeScalarOperator::Contains,
                                "startsWith" => RuntimeScalarOperator::StartsWith,
                                "endsWith" => RuntimeScalarOperator::EndsWith,
                                _ => return Err(invalid()),
                            };
                            schema.runtime_compare(
                                collection,
                                &handle,
                                op,
                                scalar::decode(f.value_kind, value, limits.scalars)?,
                                limits.query,
                            )
                        }
                    }
                    .map_err(query_error)?;
                    predicates.push(predicate);
                }
            }
        }
    }
    schema
        .runtime_and(collection, predicates, limits.query)
        .map_err(query_error)
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
