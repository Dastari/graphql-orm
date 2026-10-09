use super::plan::{Plan, Planner, invalid, query_error};
use super::*;
use crate::{db::Database, graphql::orm::*};
use async_graphql::{
    Data, Name, Request, Response, Value, Variables, extensions::*,
    parser::types::ExecutableDocument,
};
use futures::future::BoxFuture;
use std::{
    collections::BTreeMap,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

/// All client-requested capabilities for one selected layer, before protected I/O.
pub struct RuntimeReadCheck<'a> {
    pub schema: &'a ValidatedRuntimeSchema,
    pub collection: &'a RuntimeCollectionHandle,
    pub projection: &'a [RuntimeFieldHandle],
    pub filter: Option<&'a RuntimePredicate>,
    /// Stable field handles and operator names from the existing typed predicate.
    /// Names are eq/ne/lt/lte/gt/gte/contains/startsWith/endsWith/in/notIn/isNull/isNotNull.
    pub filter_operations: Vec<(&'a RuntimeFieldHandle, &'static str)>,
    /// Distinguish client ordering from trusted effective hidden-key ordering.
    pub requested_order: &'a [RuntimeOrderInput],
    pub order_was_supplied: bool,
    pub order: &'a RuntimeOrder,
    /// Boolean operators present in the validated predicate: and/or/not.
    pub logical_operators: Vec<&'static str>,
    pub relation: Option<&'a RuntimeRelationHandle>,
    pub include_count: bool,
}
/// Explicit host grant. No provider or error means denial.
pub struct RuntimeReadGrant {
    predicate: Option<RuntimePredicate>,
}
impl RuntimeReadGrant {
    pub fn new(predicate: Option<RuntimePredicate>) -> Self {
        Self { predicate }
    }
}
/// Hosts authorize every field, operator, traversal and count in the check.
pub trait RuntimeReadAuthority: Send + Sync + 'static {
    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>>;
}
/// Trusted request data. Reuse does not reuse execution caches or grants.
pub struct RuntimeGraphqlRequest<B: RuntimeReadBackend> {
    fingerprint: SchemaFingerprint,
    authority: Arc<dyn RuntimeReadAuthority>,
    auth: Option<DbAuthContext>,
    budget: RuntimeGraphqlLimits,
    scope: Option<String>,
    backend: PhantomData<B>,
}
impl<B: RuntimeReadBackend> RuntimeGraphqlRequest<B> {
    pub fn new(
        fingerprint: SchemaFingerprint,
        authority: Arc<dyn RuntimeReadAuthority>,
        auth: Option<DbAuthContext>,
        budget: RuntimeGraphqlLimits,
    ) -> Self {
        Self {
            fingerprint,
            authority,
            auth,
            budget,
            scope: None,
            backend: PhantomData,
        }
    }
    /// Bind cursors to a trusted authorization partition, never client input.
    pub fn with_cursor_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }
}
#[derive(Clone)]
pub(super) struct Protection {
    pub protector: Arc<dyn RuntimeCursorProtector>,
    pub audience: RuntimeCursorAudience,
    pub limits: RuntimeCursorProtectionLimits,
}
pub(super) struct Results(pub BTreeMap<String, Value>);
pub(super) struct GuardFactory<B: RuntimeReadBackend> {
    pub database: Database<B>,
    pub modules: Vec<RuntimeGraphqlModule>,
    pub limits: RuntimeGraphqlLimits,
    pub protection: Option<Protection>,
}
impl<B: RuntimeReadBackend> ExtensionFactory for GuardFactory<B> {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(Guard {
            database: self.database.clone(),
            modules: self.modules.clone(),
            limits: self.limits,
            protection: self.protection.clone(),
            parsed: Mutex::new(None),
        })
    }
}
struct Guard<B: RuntimeReadBackend> {
    database: Database<B>,
    modules: Vec<RuntimeGraphqlModule>,
    limits: RuntimeGraphqlLimits,
    protection: Option<Protection>,
    parsed: Mutex<Option<(ExecutableDocument, Variables)>>,
}
fn server(e: RuntimeGraphqlError) -> async_graphql::ServerError {
    {
        let mut error = async_graphql::ServerError::new(e.code(), None);
        let mut extensions = async_graphql::ErrorExtensionValues::default();
        extensions.set("code", e.code());
        error.extensions = Some(extensions);
        error
    }
}
fn bounded_variables(
    variables: &Variables,
    limits: RuntimeGraphqlLimits,
) -> Result<(), RuntimeGraphqlError> {
    let mut bytes = 0usize;
    let mut nodes = 0usize;
    let mut stack = Vec::new();
    for (name, value) in variables.iter() {
        bytes = bytes.checked_add(name.len()).ok_or_else(invalid)?;
        if stack.len() >= limits.max_variable_nodes {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        stack.push((value, 0usize));
    }
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes.checked_add(1).ok_or_else(invalid)?;
        if nodes > limits.max_variable_nodes || depth > limits.max_depth {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        let size = match value {
            Value::String(s) => s.len(),
            Value::Enum(s) => s.len(),
            Value::Binary(v) => v.len(),
            _ => 16,
        };
        bytes = bytes.checked_add(size).ok_or_else(invalid)?;
        if bytes > limits.max_variable_bytes {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        match value {
            Value::List(values) => {
                for child in values {
                    if stack.len() >= limits.max_variable_nodes.saturating_sub(nodes) {
                        return Err(RuntimeGraphqlError::new("cost_exceeded"));
                    }
                    stack.push((child, depth + 1));
                }
            }
            Value::Object(values) => {
                for (name, child) in values {
                    bytes = bytes.checked_add(name.len()).ok_or_else(invalid)?;
                    if stack.len() >= limits.max_variable_nodes.saturating_sub(nodes) {
                        return Err(RuntimeGraphqlError::new("cost_exceeded"));
                    }
                    stack.push((child, depth + 1));
                }
            }
            _ => {}
        }
    }
    if bytes > limits.max_variable_bytes {
        return Err(RuntimeGraphqlError::new("cost_exceeded"));
    }
    Ok(())
}
// Allocation-free lexical nesting guard before the GraphQL parser recurses.
fn bounded_query_nesting(query: &str, maximum: usize) -> Result<(), RuntimeGraphqlError> {
    let bytes = query.as_bytes();
    let mut index = 0usize;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'#' => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'"' => {
                let block = bytes.get(index..index + 3) == Some(b"\"\"\"");
                index += if block { 3 } else { 1 };
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = index.saturating_add(2);
                        continue;
                    }
                    if block && bytes.get(index..index + 3) == Some(b"\"\"\"") {
                        index += 3;
                        break;
                    }
                    if !block && bytes[index] == b'"' {
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'{' | b'[' | b'(' => {
                depth += 1;
                if depth > maximum {
                    return Err(RuntimeGraphqlError::new("cost_exceeded"));
                }
                index += 1;
            }
            b'}' | b']' | b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ => index += 1,
        }
    }
    Ok(())
}
#[async_trait::async_trait]
impl<B: RuntimeReadBackend> Extension for Guard<B> {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        request: Request,
        next: NextPrepareRequest<'_>,
    ) -> async_graphql::ServerResult<Request> {
        let limits = request
            .data
            .get(&std::any::TypeId::of::<RuntimeGraphqlRequest<B>>())
            .and_then(|data| data.downcast_ref::<RuntimeGraphqlRequest<B>>())
            .map_or(self.limits, |request| {
                self.limits.restricted_by(request.budget)
            });
        if request.query.len() > limits.max_query_bytes {
            return Err(server(RuntimeGraphqlError::new("cost_exceeded")));
        }
        bounded_variables(&request.variables, limits).map_err(server)?;
        next.run(ctx, request).await
    }
    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> async_graphql::ServerResult<ExecutableDocument> {
        if query.len() > self.limits.max_query_bytes {
            return Err(server(RuntimeGraphqlError::new("cost_exceeded")));
        }
        bounded_variables(variables, self.limits).map_err(server)?;
        bounded_query_nesting(query, self.limits.max_depth).map_err(server)?;
        let document = next
            .run(ctx, query, variables)
            .await
            .map_err(|_| server(invalid()))?;
        *self.parsed.lock().map_err(|_| server(invalid()))? =
            Some((document.clone(), variables.clone()));
        Ok(document)
    }
    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<async_graphql::ValidationResult, Vec<async_graphql::ServerError>> {
        next.run(ctx).await.map_err(|_| vec![server(invalid())])
    }
    async fn execute(
        &self,
        ctx: &ExtensionContext<'_>,
        operation_name: Option<&str>,
        next: NextExecute<'_>,
    ) -> Response {
        let result = self.run(ctx, operation_name).await;
        match result {
            Ok(results) => {
                let mut data = Data::default();
                data.insert(results);
                next.run_with_data(ctx, operation_name, data).await
            }
            Err(error) => Response::from_errors(vec![server(error)]),
        }
    }
}
impl<B: RuntimeReadBackend> Guard<B> {
    async fn run(
        &self,
        ctx: &ExtensionContext<'_>,
        operation_name: Option<&str>,
    ) -> Result<Results, RuntimeGraphqlError> {
        let (document, mut variables) = self
            .parsed
            .lock()
            .map_err(|_| invalid())?
            .clone()
            .ok_or_else(invalid)?;
        let operation = document
            .operations
            .iter()
            .find(|(name, _)| match operation_name {
                Some(expected) => name.is_some_and(|n| n == expected),
                None => document.operations.iter().len() == 1,
            })
            .ok_or_else(invalid)?
            .1;
        for definition in &operation.node.variable_definitions {
            if !variables.contains_key(&definition.node.name.node) {
                if let Some(value) = definition.node.default_value() {
                    variables.insert(definition.node.name.node.clone(), value.clone());
                }
            }
        }
        let request = ctx.data_opt::<RuntimeGraphqlRequest<B>>();
        let limits = request.map_or(self.limits, |request| {
            self.limits.restricted_by(request.budget)
        });
        bounded_variables(&variables, limits)?;
        let mut planner = Planner {
            document: &document,
            variables: &variables,
            limits,
            expanded: 0,
            cost: Default::default(),
        };
        let root_type = ctx.schema_env.registry.query_type.as_str();
        let selected = planner.selections(&operation.node.selection_set.node, root_type, 0)?;
        let mut plans = Vec::new();
        let mut output_shapes = BTreeMap::new();
        for field in selected {
            for module in &self.modules {
                if let Some(c) = module
                    .schema
                    .schema()
                    .collections
                    .iter()
                    .find(|c| module.root_name(c) == field.name.node.as_str())
                {
                    plans.push(planner.collection(module, c, &field, None, 1, 0)?);
                    let shape = OutputShape::compile(
                        &mut planner,
                        &ctx.schema_env.registry,
                        &field.selection_set.node,
                        &format!("{}Connection", module.object_name(c)),
                        0,
                    )?;
                    output_shapes.insert(field.response_key().node.to_string(), shape);
                }
            }
        }
        if plans.is_empty() {
            return Ok(Results(BTreeMap::new()));
        }
        let request = request.ok_or_else(|| RuntimeGraphqlError::new("authorization_missing"))?;
        // Complete all layers' grants and cursor framing checks before any ORM I/O.
        for plan in &mut plans {
            authorize(plan, request, self.protection.as_ref(), limits).await?;
        }
        let mut executor = Executor {
            database: &self.database,
            request,
            protection: self.protection.as_ref(),
            limits,
            response_bytes: 0,
            crypto_calls: 0,
        };
        let mut prepared = Vec::new();
        for plan in &plans {
            let page = executor.page(plan, None).await?;
            let read = plan
                .module
                .schema
                .runtime_read_request_with_relation_keys(
                    &plan.collection,
                    &plan.projection,
                    plan.predicate.clone(),
                    plan.order.clone(),
                    page,
                    plan.count,
                    &plan.relations(),
                    limits.query,
                )
                .map_err(relation_error)?;
            read.validate_graphql_backend(B::DIALECT)
                .map_err(query_error)?;
            prepared.push(read);
        }
        let mut values = BTreeMap::new();
        for (plan, read) in plans.into_iter().zip(prepared) {
            let value = executor.root(&plan, read).await?;
            values.insert(plan.response_key, value);
        }
        let mut response_bytes = 0usize;
        for (key, value) in &values {
            output_shapes.get(key).ok_or_else(invalid)?.measure(
                value,
                &mut response_bytes,
                limits.max_response_bytes,
            )?;
        }
        Ok(Results(values))
    }
}
fn authorize<'a, B: RuntimeReadBackend>(
    plan: &'a mut Plan,
    request: &'a RuntimeGraphqlRequest<B>,
    protection: Option<&'a Protection>,
    limits: RuntimeGraphqlLimits,
) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
    Box::pin(async move {
        if plan.module.schema.fingerprint() != request.fingerprint {
            return Err(RuntimeGraphqlError::new("schema_mismatch"));
        }
        if let Some(token) = page_cursor(&plan.page) {
            if plan.module.options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption {
                let protection =
                    protection.ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
                cursor::validate_token(token, protection.limits)?;
            } else if token.len() > limits.query.max_cursor_bytes {
                return Err(RuntimeGraphqlError::new("invalid_cursor"));
            }
        }
        if plan.module.options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption
            && request
                .scope
                .as_ref()
                .is_none_or(|v| v.is_empty() || v.len() > 4096)
        {
            return Err(RuntimeGraphqlError::new("authorization_missing"));
        }
        let grant = request
            .authority
            .authorize(RuntimeReadCheck {
                schema: &plan.module.schema,
                collection: &plan.collection,
                projection: &plan.fields,
                filter: plan.predicate.as_ref(),
                filter_operations: plan
                    .predicate
                    .as_ref()
                    .map_or_else(Vec::new, RuntimePredicate::graphql_operations),
                requested_order: &plan.requested_order,
                order_was_supplied: plan.order_was_supplied,
                order: &plan.order,
                logical_operators: plan
                    .predicate
                    .as_ref()
                    .map_or_else(Vec::new, RuntimePredicate::graphql_logical_operations),
                relation: plan.relation.as_ref(),
                include_count: plan.count,
            })
            .await?;
        if let Some(predicate) = grant.predicate {
            if !predicate.belongs_to(&plan.module.schema.fingerprint(), plan.collection.id()) {
                return Err(RuntimeGraphqlError::new("invalid_filter"));
            }
            plan.predicate = Some(match plan.predicate.take() {
                Some(filter) => plan
                    .module
                    .schema
                    .runtime_and(&plan.collection, vec![predicate, filter], limits.query)
                    .map_err(query_error)?,
                None => predicate,
            });
        }
        // Validate the authorized layer through the shared renderer before any
        // parent read. Parent-bound cursor decoding is deferred until its anchor exists.
        let page = match &plan.page {
            RuntimePageRequest::First { size, .. } => RuntimePageRequest::first(*size, None),
            RuntimePageRequest::Last { size, .. } => RuntimePageRequest::last(*size, None),
        };
        plan.module
            .schema
            .runtime_read_request(
                &plan.collection,
                &plan.projection,
                plan.predicate.clone(),
                plan.order.clone(),
                page,
                plan.count,
                limits.query,
            )
            .map_err(query_error)?
            .validate_graphql_backend(B::DIALECT)
            .map_err(query_error)?;
        for child in &mut plan.children {
            authorize(child, request, protection, limits).await?;
        }
        Ok(())
    })
}
fn page_cursor(page: &RuntimePageRequest) -> Option<&str> {
    match page {
        RuntimePageRequest::First { after, .. } => after.as_deref(),
        RuntimePageRequest::Last { before, .. } => before.as_deref(),
    }
}
struct Executor<'a, B: RuntimeReadBackend> {
    database: &'a Database<B>,
    request: &'a RuntimeGraphqlRequest<B>,
    protection: Option<&'a Protection>,
    limits: RuntimeGraphqlLimits,
    response_bytes: usize,
    crypto_calls: usize,
}
fn relation_error(e: RuntimeRelationError) -> RuntimeGraphqlError {
    RuntimeGraphqlError::new(e.code().as_str())
}
fn object(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(k, v)| (Name::new(k), v))
            .collect(),
    )
}
impl<B: RuntimeReadBackend> Executor<'_, B> {
    fn context(
        &self,
        plan: &Plan,
        parent: Option<&RuntimeParentAnchor>,
    ) -> Result<Option<RuntimeCursorContext>, RuntimeGraphqlError> {
        if plan.module.options.cursor_profile == RuntimeCursorProfile::Unprotected {
            return Ok(None);
        }
        let protection = self
            .protection
            .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
        let order = plan
            .order
            .terms()
            .iter()
            .map(|t| (t.field().id(), t.direction(), t.nulls()))
            .collect::<Vec<_>>();
        let order = serde_json::to_vec(&order).map_err(|_| invalid())?;
        let parent = parent
            .map(RuntimeParentAnchor::graphql_cursor_scope)
            .transpose()
            .map_err(relation_error)?
            .unwrap_or_default();
        let fingerprint = plan.module.schema.fingerprint();
        RuntimeCursorContext::new(
            &protection.audience,
            self.request.scope.as_deref().ok_or_else(invalid)?,
            &[
                fingerprint.as_str().as_bytes(),
                plan.collection.id().as_str().as_bytes(),
                &order,
                &parent,
            ],
            protection.limits,
        )
        .map(Some)
    }
    fn crypto(&mut self) -> Result<(), RuntimeGraphqlError> {
        self.crypto_calls = self.crypto_calls.checked_add(1).ok_or_else(invalid)?;
        if self.crypto_calls > self.limits.max_crypto_calls {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        Ok(())
    }
    async fn page(
        &mut self,
        plan: &Plan,
        parent: Option<&RuntimeParentAnchor>,
    ) -> Result<RuntimePageRequest, RuntimeGraphqlError> {
        let mut page = plan.page.clone();
        let token = match &mut page {
            RuntimePageRequest::First { after, .. } => after,
            RuntimePageRequest::Last { before, .. } => before,
        };
        if let Some(raw) = token {
            if let Some(context) = self.context(plan, parent)? {
                self.crypto()?;
                let protection = self.protection.ok_or_else(invalid)?;
                *raw = cursor::open(
                    protection.protector.as_ref(),
                    &context,
                    raw,
                    protection.limits,
                )
                .await?;
            }
        }
        Ok(page)
    }
    async fn connection(
        &mut self,
        plan: &Plan,
        parent: Option<&RuntimeParentAnchor>,
        connection: RuntimeConnection,
        nodes: Vec<Value>,
    ) -> Result<Value, RuntimeGraphqlError> {
        let context = self.context(plan, parent)?;
        let mut cursors = Vec::new();
        let mut edges = Vec::new();
        for (edge, node) in connection.edges.into_iter().zip(nodes) {
            let token = if let Some(context) = &context {
                self.crypto()?;
                let protection = self.protection.ok_or_else(invalid)?;
                cursor::seal(
                    protection.protector.as_ref(),
                    context,
                    &edge.cursor,
                    protection.limits,
                )
                .await?
            } else {
                edge.cursor
            };
            self.reserve(token.len())?;
            cursors.push(token.clone());
            edges.push(object([("cursor", Value::String(token)), ("node", node)]));
        }
        Ok(object([
            ("edges", Value::List(edges)),
            (
                "pageInfo",
                object([
                    (
                        "hasNextPage",
                        Value::Boolean(connection.page_info.has_next_page),
                    ),
                    (
                        "hasPreviousPage",
                        Value::Boolean(connection.page_info.has_previous_page),
                    ),
                    (
                        "startCursor",
                        cursors
                            .first()
                            .cloned()
                            .map(Value::String)
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "endCursor",
                        cursors
                            .last()
                            .cloned()
                            .map(Value::String)
                            .unwrap_or(Value::Null),
                    ),
                ]),
            ),
            (
                "totalCount",
                connection
                    .total_count
                    .map(|v| Value::String(v.to_string()))
                    .unwrap_or(Value::Null),
            ),
        ]))
    }
    fn reserve(&mut self, bytes: usize) -> Result<(), RuntimeGraphqlError> {
        self.response_bytes = self.response_bytes.checked_add(bytes).ok_or_else(invalid)?;
        if self.response_bytes > self.limits.max_response_bytes {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        Ok(())
    }
    fn records(
        &mut self,
        plan: &Plan,
        records: &[RuntimeRecord],
    ) -> Result<Vec<Value>, RuntimeGraphqlError> {
        let metadata = plan
            .module
            .schema
            .schema()
            .collections
            .iter()
            .find(|c| &c.id == plan.collection.id())
            .ok_or_else(invalid)?;
        let mut nodes = Vec::new();
        for record in records {
            let mut values = async_graphql::indexmap::IndexMap::new();
            for field in &plan.fields {
                let value = match record.state(field).map_err(|_| invalid())? {
                    RuntimeFieldState::Value(v) => scalar::encode(v, self.limits.scalars)?,
                    RuntimeFieldState::Null => Value::Null,
                    RuntimeFieldState::Unloaded => {
                        return Err(RuntimeGraphqlError::new("projection_mismatch"));
                    }
                };
                let size = match &value {
                    Value::String(v) => v.len(),
                    _ => 16,
                };
                self.reserve(size)?;
                let name = &metadata
                    .fields
                    .iter()
                    .find(|f| &f.id == field.id())
                    .ok_or_else(invalid)?
                    .api_name;
                values.insert(Name::new(name), value);
            }
            nodes.push(Value::Object(values));
        }
        Ok(nodes)
    }
    async fn root(
        &mut self,
        plan: &Plan,
        request: RuntimeAnchoredReadRequest,
    ) -> Result<Value, RuntimeGraphqlError> {
        let connection = self
            .database
            .execute_runtime_anchored_read(&request, self.request.auth.as_ref())
            .await
            .map_err(relation_error)?;
        let records = connection
            .edges
            .iter()
            .map(|e| e.node.clone())
            .collect::<Vec<_>>();
        let mut nodes = self.records(plan, &records)?;
        for child in &plan.children {
            let relation = child.relation.as_ref().ok_or_else(invalid)?;
            let parents = connection
                .relation_parents(relation)
                .map_err(relation_error)?;
            let values = self.layer(child, parents).await?;
            attach(&mut nodes, &child.response_key, values)?;
        }
        let connection = RuntimeConnection {
            edges: connection
                .edges
                .into_iter()
                .map(|e| RuntimeEdge {
                    node: e.node,
                    cursor: e.cursor,
                })
                .collect(),
            page_info: connection.page_info,
            total_count: connection.total_count,
        };
        self.connection(plan, None, connection, nodes).await
    }
    fn layer<'a>(
        &'a mut self,
        plan: &'a Plan,
        parents: Vec<RuntimeParentAnchor>,
    ) -> BoxFuture<'a, Result<Vec<Value>, RuntimeGraphqlError>> {
        Box::pin(async move {
            let relation = plan.relation.as_ref().ok_or_else(invalid)?;
            if parents.is_empty() {
                return Ok(Vec::new());
            }
            let selection = if relation.cardinality() == RelationCardinality::One {
                RuntimeRelationSelection::ToOne
            } else {
                let mut pages = Vec::new();
                for parent in &parents {
                    pages.push(self.page(plan, Some(parent)).await?);
                }
                RuntimeRelationSelection::ToMany {
                    pages,
                    include_count: plan.count,
                }
            };
            let request = plan
                .module
                .schema
                .runtime_relation_batch_request_with_relation_keys(
                    relation,
                    parents.clone(),
                    &plan.projection,
                    plan.predicate.clone(),
                    plan.order.clone(),
                    selection,
                    &plan.relations(),
                    self.limits.relation,
                )
                .map_err(relation_error)?;
            let batch = self
                .database
                .execute_runtime_relation_batch(&request, self.request.auth.as_ref())
                .await
                .map_err(relation_error)?;
            let records = batch
                .results
                .iter()
                .flat_map(|r| match &r.value {
                    RuntimeRelationValue::ToOne(v) => v.iter().cloned().collect::<Vec<_>>(),
                    RuntimeRelationValue::ToMany(c) => {
                        c.edges.iter().map(|e| e.node.clone()).collect()
                    }
                })
                .collect::<Vec<_>>();
            let mut nodes = self.records(plan, &records)?;
            for child in &plan.children {
                let parents = batch
                    .relation_parents(child.relation.as_ref().ok_or_else(invalid)?)
                    .map_err(relation_error)?;
                let values = self.layer(child, parents).await?;
                attach(&mut nodes, &child.response_key, values)?;
            }
            let mut nodes = nodes.into_iter();
            let mut output = Vec::new();
            for (result, parent) in batch.results.into_iter().zip(&parents) {
                output.push(match result.value {
                    RuntimeRelationValue::ToOne(v) => {
                        if v.is_some() {
                            nodes.next().ok_or_else(invalid)?
                        } else {
                            Value::Null
                        }
                    }
                    RuntimeRelationValue::ToMany(connection) => {
                        let values = nodes.by_ref().take(connection.edges.len()).collect();
                        self.connection(plan, Some(parent), connection, values)
                            .await?
                    }
                });
            }
            Ok(output)
        })
    }
}
fn attach(nodes: &mut [Value], key: &str, values: Vec<Value>) -> Result<(), RuntimeGraphqlError> {
    if nodes.len() != values.len() {
        return Err(invalid());
    }
    for (node, value) in nodes.iter_mut().zip(values) {
        let Value::Object(map) = node else {
            return Err(invalid());
        };
        map.insert(Name::new(key), value);
    }
    Ok(())
}

struct OutputShape {
    fields: Vec<(String, String, OutputShape)>,
}
impl OutputShape {
    fn compile(
        planner: &mut Planner<'_>,
        registry: &async_graphql::registry::Registry,
        set: &async_graphql::parser::types::SelectionSet,
        ty: &str,
        depth: usize,
    ) -> Result<Self, RuntimeGraphqlError> {
        let mut fields = Vec::new();
        for field in planner.selections(set, ty, depth)? {
            let child = if field.name.node == "__typename" {
                Self { fields: Vec::new() }
            } else {
                let target = registry
                    .types
                    .get(ty)
                    .and_then(|t| t.field_by_name(field.name.node.as_str()))
                    .ok_or_else(invalid)?;
                let target = target.ty.trim_matches(['[', ']', '!']);
                Self::compile(
                    planner,
                    registry,
                    &field.selection_set.node,
                    target,
                    depth + 1,
                )?
            };
            fields.push((
                field.response_key().node.to_string(),
                field.name.node.to_string(),
                child,
            ));
        }
        Ok(Self { fields })
    }
    fn measure(
        &self,
        value: &Value,
        bytes: &mut usize,
        maximum: usize,
    ) -> Result<(), RuntimeGraphqlError> {
        let add = |bytes: &mut usize, size: usize| -> Result<(), RuntimeGraphqlError> {
            *bytes = bytes.checked_add(size).ok_or_else(invalid)?;
            if *bytes > maximum {
                return Err(RuntimeGraphqlError::new("cost_exceeded"));
            }
            Ok(())
        };
        add(bytes, 2)?;
        match value {
            Value::List(values) => {
                for value in values {
                    self.measure(value, bytes, maximum)?;
                }
            }
            Value::Object(values) => {
                for (key, name, child) in &self.fields {
                    add(
                        bytes,
                        key.len()
                            .checked_mul(6)
                            .and_then(|n| n.checked_add(4))
                            .ok_or_else(invalid)?,
                    )?;
                    if name == "__typename" {
                        add(bytes, 128)?;
                        continue;
                    }
                    let value = values
                        .get(key.as_str())
                        .or_else(|| values.get(name.as_str()))
                        .ok_or_else(invalid)?;
                    child.measure(value, bytes, maximum)?;
                }
            }
            Value::String(value) => add(bytes, value.len().checked_mul(6).ok_or_else(invalid)?)?,
            _ => add(bytes, 32)?,
        }
        Ok(())
    }
}
