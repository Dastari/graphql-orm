//! Checked dynamic mutation registration over the runtime mutation engine.
use super::*;
use crate::graphql::orm::*;
use async_graphql::dynamic::*;

/// Explicit opt-in; each enabled action uses a separate state-machine transaction.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RuntimeGraphqlMutationOptions {
    pub create: bool,
    pub update: bool,
    pub delete: bool,
    pub limits: RuntimeMutationLimits,
    /// Bounds only post-commit record authorization, not the transaction itself.
    pub output_authorization_timeout: std::time::Duration,
}
impl Default for RuntimeGraphqlMutationOptions {
    fn default() -> Self {
        Self {
            create: true,
            update: true,
            delete: true,
            limits: Default::default(),
            output_authorization_timeout: std::time::Duration::from_secs(5),
        }
    }
}
pub(super) fn actions(module: &RuntimeGraphqlModule) -> Vec<(&'static str, RuntimeMutationAction)> {
    let Some(options) = &module.options.mutations else {
        return vec![];
    };
    [
        (options.create, "create", RuntimeMutationAction::Create),
        (options.update, "update", RuntimeMutationAction::Update),
        (options.delete, "delete", RuntimeMutationAction::Delete),
    ]
    .into_iter()
    .filter_map(|(enabled, prefix, action)| enabled.then_some((prefix, action)))
    .collect()
}
pub(super) fn type_names(module: &RuntimeGraphqlModule, c: &RuntimeCollection) -> Vec<String> {
    let name = module.object_name(c);
    let Some(options) = &module.options.mutations else {
        return vec![];
    };
    let mut names = vec![format!("{name}MutationPayload")];
    if options.create {
        names.push(format!("Create{name}Input"));
    }
    if options.update {
        names.push(format!("Update{name}Input"));
    }
    if options.update || options.delete {
        names.push(format!("{name}KeyInput"));
    }
    names
}
pub(super) fn types(module: &RuntimeGraphqlModule) -> Result<Vec<Type>, RuntimeGraphqlError> {
    let Some(options) = &module.options.mutations else {
        return Ok(vec![]);
    };
    let mut types = vec![];
    for c in &module.schema.schema().collections {
        let name = module.object_name(c);
        types.push(
            Object::new(format!("{name}MutationPayload"))
                .description(format!("Result of one {name} mutation"))
                .field(
                    super::schema::output("record", TypeRef::named(&name))
                        .description("The authorized record, when the mutation returns one"),
                )
                .into(),
        );
        if options.create {
            let mut input = InputObject::new(format!("Create{name}Input"))
                .description(format!("Values of a new {name} record"));
            let mut count = 0;
            for f in c.fields.iter().filter(|f| !f.generated) {
                let required =
                    !f.nullable && (f.default.is_none() || c.primary_key.contains(&f.id));
                input = input.field(
                    InputValue::new(
                        &f.api_name,
                        if required {
                            TypeRef::named_nn(super::scalar::name(f.value_kind))
                        } else {
                            TypeRef::named(super::scalar::name(f.value_kind))
                        },
                    )
                    .description(f.effective_description()),
                );
                count += 1;
            }
            if count == 0 {
                return Err(RuntimeGraphqlError::new("unsupported_capability"));
            }
            types.push(input.into());
        }
        if options.update {
            let mut input = InputObject::new(format!("Update{name}Input")).description(format!(
                "Changed values of one {name} record; omitted fields are unchanged"
            ));
            let mut count = 0;
            for f in c
                .fields
                .iter()
                .filter(|f| !f.generated && !c.primary_key.contains(&f.id))
            {
                input = input.field(
                    InputValue::new(
                        &f.api_name,
                        TypeRef::named(super::scalar::name(f.value_kind)),
                    )
                    .description(f.effective_description()),
                );
                count += 1;
            }
            if count == 0 {
                return Err(RuntimeGraphqlError::new("unsupported_capability"));
            }
            types.push(input.into());
        }
        if options.update || options.delete {
            let mut input = InputObject::new(format!("{name}KeyInput"))
                .description(format!("Primary key of one {name} record"));
            for id in &c.primary_key {
                let f = c
                    .fields
                    .iter()
                    .find(|f| &f.id == id)
                    .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
                input = input.field(
                    InputValue::new(
                        &f.api_name,
                        TypeRef::named_nn(super::scalar::name(f.value_kind)),
                    )
                    .description(f.effective_description()),
                );
            }
            types.push(input.into());
        }
    }
    Ok(types)
}

use crate::db::Database;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
pub(super) type FieldFactory = Box<
    dyn Fn(RuntimeGraphqlModule) -> Result<Vec<(String, Field)>, RuntimeGraphqlError> + Send + Sync,
>;

impl<B: RuntimeMutationBackend> RuntimeGraphqlRequest<B> {
    pub fn with_mutation_environment(
        mut self,
        environment: Arc<RuntimeMutationEnvironment>,
    ) -> Self {
        self.mutation_environment = Some(environment);
        self
    }
    pub fn with_write_authority(mut self, authority: Arc<dyn RuntimeWriteAuthority<B>>) -> Self {
        self.write_authority = Some(Arc::new(authority));
        self
    }
    pub fn with_mutation_hook(mut self, hook: Arc<dyn RuntimeMutationHook<B>>) -> Self {
        self.mutation_hook = Some(Arc::new(hook));
        self
    }
}
impl<B: RuntimeMutationBackend> RuntimeGraphqlComposer<B> {
    pub fn mutation_root(mut self, name: impl Into<String>) -> Result<Self, RuntimeGraphqlError> {
        let name = name.into();
        if !B::RUNTIME_MUTATIONS_SUPPORTED {
            return Err(RuntimeGraphqlError::new("unsupported_backend"));
        }
        if self.mutation.is_some()
            || !super::module::valid_name(&name, true)
            || !self.names.insert(name.clone())
        {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.mutation = Some(Object::new(&name));
        self.mutation_name = Some(name);
        let database = self.database.clone();
        self.mutation_fields = Some(Box::new(move |module| fields(database.clone(), module)));
        Ok(self)
    }
    pub fn mutation_field(mut self, field: RuntimeHostField) -> Result<Self, RuntimeGraphqlError> {
        let (name, field, semantic) = field.into_parts(GraphqlOperationKind::Mutation)?;
        if !super::module::valid_name(&name, false) || !self.mutation_roots.insert(name) {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.mutation = Some(
            self.mutation
                .take()
                .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?
                .field(field),
        );
        self.semantic.root(semantic);
        Ok(self)
    }
}
fn fields<B: RuntimeMutationBackend>(
    database: Database<B>,
    module: RuntimeGraphqlModule,
) -> Result<Vec<(String, Field)>, RuntimeGraphqlError> {
    let mut fields = vec![];
    for c in &module.schema.schema().collections {
        let object = module.object_name(c);
        for (prefix, action) in actions(&module) {
            let name = format!("{prefix}{object}");
            if !super::module::valid_name(&name, false) {
                return Err(RuntimeGraphqlError::new("name_collision"));
            }
            let database = database.clone();
            let field = Field::new(
                &name,
                TypeRef::named_nn(format!("{object}MutationPayload")),
                move |ctx| {
                    let database = database.clone();
                    FieldFuture::new(async move { execute(database, ctx).await.map(Some) })
                },
            )
            .description(super::semantic::mutation_description(action, &object));
            let field = super::semantic::mutation_arguments(action, &object)
                .into_iter()
                .fold(field, |field, (name, ty)| {
                    field.argument(
                        InputValue::new(name, super::schema::type_ref(&ty))
                            .description(super::semantic::argument_description(name)),
                    )
                });
            fields.push((name, field));
        }
    }
    Ok(fields)
}

use super::plan::{Planner, invalid};
use async_graphql::{Value, parser::types::SelectionSet};
#[derive(Clone)]
pub(super) struct Prepared {
    module: RuntimeGraphqlModule,
    collection: RuntimeCollectionHandle,
    request: RuntimeMutationRequest,
    returning: Option<RuntimeProjection>,
    visibility: Option<RuntimePredicate>,
    limits: RuntimeGraphqlLimits,
    shape: super::execution::OutputShape,
    projected: Vec<super::projection::ProjectedPlan>,
    response_bytes: Arc<std::sync::atomic::AtomicUsize>,
    // Shared by occurrences of this response key, fresh for every execution.
    outcome: Arc<Mutex<GroupOutcome>>,
}

enum GroupOutcome {
    Ready,
    Running,
    Finished(async_graphql::Result<Value>),
}
enum GroupStart {
    Cached(async_graphql::Result<Value>),
    Execute(GroupExecution),
}
/// Owns the only execution attempt. Dropping an unfinished future is terminal too.
struct GroupExecution {
    outcome: Arc<Mutex<GroupOutcome>>,
    committed: bool,
    finished: bool,
}
impl GroupExecution {
    fn start(outcome: &Arc<Mutex<GroupOutcome>>) -> async_graphql::Result<GroupStart> {
        let mut state = outcome.lock().map_err(|_| uncertain_error())?;
        match &*state {
            GroupOutcome::Finished(result) => Ok(GroupStart::Cached(result.clone())),
            // The locked executor is serial at mutation roots. Fail closed if a host
            // extension reenters an in-progress resolver; never start a second attempt.
            GroupOutcome::Running => Ok(GroupStart::Cached(Err(uncertain_error()))),
            GroupOutcome::Ready => {
                *state = GroupOutcome::Running;
                Ok(GroupStart::Execute(Self {
                    outcome: outcome.clone(),
                    committed: false,
                    finished: false,
                }))
            }
        }
    }
    fn finish(&mut self, result: &async_graphql::Result<Value>) {
        if let Ok(mut state) = self.outcome.lock() {
            *state = GroupOutcome::Finished(result.clone());
        }
        self.finished = true;
    }
}
impl Drop for GroupExecution {
    fn drop(&mut self) {
        if !self.finished {
            let error = if self.committed {
                committed_error("mutation_payload_canceled")
            } else {
                // Cancellation may occur during COMMIT. Do not promise rollback or
                // invite automatic retries when the outcome cannot be confirmed.
                uncertain_error()
            };
            if let Ok(mut state) = self.outcome.lock() {
                *state = GroupOutcome::Finished(Err(error));
            }
        }
    }
}
fn uncertain_error() -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    wire_error("mutation_outcome_unknown").extend_with(|_, extensions| {
        extensions.set("outcome", "unknown");
        extensions.set("retryable", false);
    })
}
fn mutation_error(error: RuntimeMutationError) -> RuntimeGraphqlError {
    RuntimeGraphqlError::new(error.code().as_str())
}
fn record_error(_: RuntimeRecordError) -> RuntimeGraphqlError {
    RuntimeGraphqlError::new("invalid_graphql_input")
}
fn input_fields(
    module: &RuntimeGraphqlModule,
    c: &RuntimeCollection,
    collection: &RuntimeCollectionHandle,
    input: &Value,
    limits: RuntimeGraphqlLimits,
) -> Result<Vec<(RuntimeFieldHandle, RuntimeValue)>, RuntimeGraphqlError> {
    let Value::Object(values) = input else {
        return Err(invalid());
    };
    values
        .iter()
        .map(|(name, value)| {
            let field = c
                .fields
                .iter()
                .find(|f| f.api_name == name.as_str())
                .ok_or_else(invalid)?;
            let handle = module
                .schema
                .resolve_field(collection, &field.id)
                .map_err(record_error)?;
            let value = if value == &Value::Null {
                RuntimeValue::Null
            } else {
                super::scalar::decode(field.value_kind, value, limits.scalars)?
            };
            Ok((handle, value))
        })
        .collect()
}
pub(super) async fn prepare<B: RuntimeReadBackend>(
    modules: &[RuntimeGraphqlModule],
    context: Option<&RuntimeGraphqlRequest<B>>,
    planner: &mut Planner<'_>,
    selections: &SelectionSet,
    root: &str,
    registry: &async_graphql::registry::Registry,
) -> Result<super::execution::Results, RuntimeGraphqlError> {
    let mut prepared = BTreeMap::new();
    let mut projection_calls = 0usize;
    let response_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for field in planner.selections(selections, root, 0)? {
        for module in modules {
            for c in &module.schema.schema().collections {
                for (prefix, action) in actions(module) {
                    let object = module.object_name(c);
                    if field.name.node.as_str() != format!("{prefix}{object}") {
                        continue;
                    }
                    let context =
                        context.ok_or_else(|| RuntimeGraphqlError::new("authorization_missing"))?;
                    if context.write_authority.is_none() || context.mutation_environment.is_none() {
                        return Err(RuntimeGraphqlError::new("authorization_missing"));
                    }
                    let args = planner.resolve(&field)?;
                    if context.fingerprint != module.schema.fingerprint()
                        || args.get("expectedSchema")
                            != Some(&Value::String(module.schema.fingerprint().to_string()))
                    {
                        return Err(RuntimeGraphqlError::new("schema_mismatch"));
                    }
                    let collection = module
                        .schema
                        .resolve_collection(&c.id)
                        .map_err(record_error)?;
                    let mut fields = vec![];
                    let mut projected = vec![];
                    for payload in planner.selections(
                        &field.selection_set.node,
                        &format!("{object}MutationPayload"),
                        1,
                    )? {
                        if payload.name.node == "__typename" {
                            continue;
                        }
                        if payload.name.node != "record" {
                            return Err(invalid());
                        }
                        for selected in
                            planner.selections(&payload.selection_set.node, &object, 2)?
                        {
                            if selected.name.node == "__typename" {
                                continue;
                            }
                            let f = c
                                .fields
                                .iter()
                                .find(|f| f.api_name == selected.name.node.as_str())
                                .ok_or_else(|| {
                                    RuntimeGraphqlError::new("unsupported_capability")
                                })?;
                            let handle = module
                                .schema
                                .resolve_field(&collection, &f.id)
                                .map_err(record_error)?;
                            if let Some(binding) = module.projection(&f.id) {
                                let names = planner.projected_selection(binding, &selected, 3)?;
                                super::projection::merge_projected(
                                    &mut projected,
                                    handle.clone(),
                                    &f.api_name,
                                    binding,
                                    names,
                                );
                            }
                            if !fields.contains(&handle) {
                                fields.push(handle);
                            }
                        }
                    }
                    projection_calls = projection_calls
                        .checked_add(projected.len())
                        .ok_or_else(invalid)?;
                    if projection_calls > planner.limits.projection.max_calls {
                        return Err(RuntimeGraphqlError::new("cost_exceeded"));
                    }
                    let mut intent = super::plan::FilterIntent::default();
                    let expected = args
                        .get("expected")
                        .filter(|v| **v != Value::Null)
                        .map(|value| {
                            super::plan::filter(
                                &module.schema,
                                &collection,
                                c,
                                value,
                                planner.limits,
                                0,
                                &mut intent,
                            )
                        })
                        .transpose()?;
                    let order = module
                        .schema
                        .runtime_order(&collection, None, planner.limits.query)
                        .map_err(super::plan::query_error)?;
                    let grant = if fields.is_empty() && expected.is_none() {
                        RuntimeReadGrant::new(None)
                    } else {
                        context
                            .authority
                            .authorize(RuntimeReadCheck {
                                schema: &module.schema,
                                collection: &collection,
                                projection: &fields,
                                filter: expected.as_ref(),
                                filter_operations: intent
                                    .operations
                                    .iter()
                                    .map(|(f, op)| (f, *op))
                                    .collect(),
                                requested_order: &[],
                                order_was_supplied: false,
                                order: &order,
                                logical_operators: intent.logical,
                                relation: None,
                                include_count: false,
                                projections: projected
                                    .iter()
                                    .map(super::projection::ProjectedPlan::selection)
                                    .collect(),
                            })
                            .await?
                    };
                    let returning = if fields.is_empty() {
                        None
                    } else {
                        Some(
                            module
                                .schema
                                .resolve_projection(&collection, &fields)
                                .map_err(record_error)?,
                        )
                    };
                    let options = module.options.mutations.as_ref().ok_or_else(invalid)?;
                    let mut limits = options.limits;
                    macro_rules! restrict {
                        ($($field:ident),* $(,)?) => {
                            $(limits.query.$field = limits.query.$field.min(planner.limits.query.$field);)*
                        }
                    }
                    restrict!(
                        max_predicate_depth,
                        max_predicate_nodes,
                        max_values_per_list,
                        max_bind_parameters,
                        max_order_terms,
                        max_projection_fields,
                        default_page_size,
                        max_page_size,
                        max_cursor_bytes,
                        max_cursor_values
                    );
                    limits.max_record_bytes = limits
                        .max_record_bytes
                        .min(planner.limits.max_response_bytes);
                    let input = args
                        .get("input")
                        .map(|v| input_fields(module, c, &collection, v, planner.limits))
                        .transpose()?
                        .unwrap_or_default();
                    let request = if action == RuntimeMutationAction::Create {
                        module.schema.runtime_create_request(
                            &collection,
                            &input,
                            returning.clone(),
                            limits,
                        )
                    } else {
                        let keys = input_fields(
                            module,
                            c,
                            &collection,
                            args.get("key").ok_or_else(invalid)?,
                            planner.limits,
                        )?;
                        let key = module
                            .schema
                            .runtime_key(&collection, &keys)
                            .map_err(mutation_error)?;
                        if action == RuntimeMutationAction::Update {
                            module.schema.runtime_update_request(
                                key,
                                &input,
                                expected,
                                returning.clone(),
                                limits,
                            )
                        } else {
                            module.schema.runtime_delete_request(
                                key,
                                expected,
                                returning.clone(),
                                limits,
                            )
                        }
                    }
                    .map_err(mutation_error)?;
                    // Reserve B's preimage, DML and result reads, excluding host hook I/O.
                    if prepared.len().saturating_add(1).saturating_mul(4)
                        > planner.limits.max_statements
                        || prepared.len().saturating_add(1) > planner.limits.max_materialized_nodes
                    {
                        return Err(RuntimeGraphqlError::new("cost_exceeded"));
                    }
                    let shape = super::execution::OutputShape::compile(
                        planner,
                        registry,
                        &field.selection_set.node,
                        &format!("{object}MutationPayload"),
                        0,
                    )?;
                    prepared.insert(
                        field.response_key().node.to_string(),
                        Prepared {
                            module: module.clone(),
                            collection,
                            request,
                            returning,
                            visibility: grant.predicate,
                            limits: planner.limits,
                            shape,
                            projected,
                            response_bytes: response_bytes.clone(),
                            outcome: Arc::new(Mutex::new(GroupOutcome::Ready)),
                        },
                    );
                }
            }
        }
    }
    Ok(super::execution::Results(BTreeMap::new(), prepared))
}

struct AuthorityAdapter<B: RuntimeMutationBackend> {
    host: Arc<dyn RuntimeWriteAuthority<B>>,
    prepared: Prepared,
}
impl<B: RuntimeMutationBackend> RuntimeWriteAuthority<B> for AuthorityAdapter<B> {
    fn authorize_intent<'a>(
        &'a self,
        check: RuntimeWriteIntent<'a>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> futures::future::BoxFuture<'a, Result<RuntimeWriteGrant, RuntimeMutationError>> {
        Box::pin(async move {
            let grant = self.host.authorize_intent(check, tx).await?;
            grant.graphql_restrict(
                &self.prepared.module.schema,
                &self.prepared.collection,
                self.prepared.visibility.clone(),
                self.prepared.limits.query,
            )
        })
    }
    fn authorize_preimage<'a>(
        &'a self,
        check: RuntimePreimageCheck<'a>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> futures::future::BoxFuture<'a, Result<(), RuntimeMutationError>> {
        self.host.authorize_preimage(check, tx)
    }
    fn authorize_result<'a>(
        &'a self,
        check: RuntimeResultCheck<'a>,
        tx: &'a mut MutationContext<'_, B>,
    ) -> futures::future::BoxFuture<'a, Result<RuntimeReturnGrant, RuntimeMutationError>> {
        self.host.authorize_result(check, tx)
    }
}
fn wire_error(code: &'static str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    async_graphql::Error::new(code).extend_with(|_, extensions| extensions.set("code", code))
}
async fn execute<B: RuntimeMutationBackend>(
    database: Database<B>,
    ctx: ResolverContext<'_>,
) -> async_graphql::Result<FieldValue<'static>> {
    let context = ctx.data::<RuntimeGraphqlRequest<B>>()?;
    let selection = ctx.field();
    let key = selection.alias().unwrap_or(selection.name());
    let prepared = ctx
        .data::<super::execution::Results>()?
        .1
        .get(key)
        .ok_or_else(|| wire_error("authorization_missing"))?
        .clone();
    let mut execution = match GroupExecution::start(&prepared.outcome)? {
        GroupStart::Cached(result) => return result.map(super::schema::field_value),
        GroupStart::Execute(execution) => execution,
    };
    let result = execute_once(database, context, &prepared, key, &mut execution).await;
    execution.finish(&result);
    result.map(super::schema::field_value)
}
async fn execute_once<B: RuntimeMutationBackend>(
    database: Database<B>,
    context: &RuntimeGraphqlRequest<B>,
    prepared: &Prepared,
    key: &str,
    execution: &mut GroupExecution,
) -> async_graphql::Result<Value> {
    let environment = context
        .mutation_environment
        .clone()
        .ok_or_else(|| wire_error("authorization_missing"))?;
    let authority = context
        .write_authority
        .as_ref()
        .and_then(|a| a.downcast_ref::<Arc<dyn RuntimeWriteAuthority<B>>>())
        .ok_or_else(|| wire_error("authorization_missing"))?
        .clone();
    let hook = context
        .mutation_hook
        .as_ref()
        .map(|h| {
            h.downcast_ref::<Arc<dyn RuntimeMutationHook<B>>>()
                .cloned()
                .ok_or_else(|| wire_error("authorization_missing"))
        })
        .transpose()?;
    let request = prepared.request.clone();
    let adapter = AuthorityAdapter {
        host: authority,
        prepared: prepared.clone(),
    };
    let effect = database
        .transaction_with_auth(
            TransactionMode::StateMachine,
            context.auth.as_ref(),
            move |tx| {
                Box::pin(async move {
                    let effect = tx
                        .mutate_runtime(&environment, &request, &adapter)
                        .await
                        .map_err(crate::graphql::errors::OrmPublicError::from)?;
                    if let Some(hook) = hook {
                        tx.run_runtime_before_commit(hook.as_ref(), &effect)
                            .await
                            .map_err(crate::graphql::errors::OrmPublicError::from)?;
                    }
                    Ok(effect)
                })
            },
        )
        .await
        .map_err(|error| {
            let code = match error {
                TransactionError::Rejected(error) => {
                    error.runtime_mutation_code().unwrap_or("mutation_rejected")
                }
                TransactionError::Retryable(_) => "transaction_retryable",
                TransactionError::Failed(_) => "transaction_failed",
            };
            wire_error(code)
        })?;
    execution.committed = true;
    let Some(projection) = prepared.returning.as_ref() else {
        let payload = async_graphql::value!({"record":null});
        reserve_payload(prepared, &payload, key)?;
        return Ok(payload);
    };
    let record = effect
        .record()
        .ok_or_else(|| committed_error("mutation_payload_failed"))?;
    let kind = if effect.action() == RuntimeMutationAction::Delete {
        RuntimeMutationRecordKind::DeletedPreimage
    } else {
        RuntimeMutationRecordKind::CommittedResult
    };
    let timeout = prepared
        .module
        .options
        .mutations
        .as_ref()
        .ok_or_else(|| committed_error("mutation_payload_failed"))?
        .output_authorization_timeout;
    let decision = tokio::time::timeout(
        timeout,
        context
            .authority
            .authorize_mutation_result(RuntimeMutationOutputCheck {
                schema: &prepared.module.schema,
                collection: &prepared.collection,
                action: effect.action(),
                kind,
                projection,
                record,
                projections: prepared
                    .projected
                    .iter()
                    .map(super::projection::ProjectedPlan::selection)
                    .collect(),
            }),
    )
    .await;
    match decision {
        Ok(Ok(())) => {}
        Ok(Err(error)) if error == RuntimeGraphqlError::denied() => {
            return Err(committed_error("mutation_payload_denied"));
        }
        Ok(Err(_)) => return Err(committed_error("mutation_payload_failed")),
        Err(_) => return Err(committed_error("mutation_payload_timeout")),
    }
    // Serialize only the authorized selected projection, never internal policy/key fields.
    let mut values = async_graphql::indexmap::IndexMap::new();
    let metadata = prepared
        .module
        .schema
        .schema()
        .collections
        .iter()
        .find(|c| &c.id == prepared.collection.id())
        .ok_or_else(|| committed_error("mutation_payload_failed"))?;
    for field in projection.fields() {
        if prepared
            .projected
            .iter()
            .any(|projected| &projected.field == field)
        {
            continue;
        }
        let value = match record
            .state(field)
            .map_err(|_| committed_error("mutation_payload_failed"))?
        {
            RuntimeFieldState::Value(value) => {
                super::scalar::encode(value, prepared.limits.scalars)
                    .map_err(|_| committed_error("mutation_payload_failed"))?
            }
            RuntimeFieldState::Null => Value::Null,
            RuntimeFieldState::Unloaded => return Err(committed_error("mutation_payload_failed")),
        };
        let name = &metadata
            .fields
            .iter()
            .find(|f| &f.id == field.id())
            .ok_or_else(|| committed_error("mutation_payload_failed"))?
            .api_name;
        values.insert(async_graphql::Name::new(name), value);
    }
    // Host objects resolve only after commit and the independent release check.
    for projected in &prepared.projected {
        let stored = match record
            .state(&projected.field)
            .map_err(|_| committed_error("mutation_payload_failed"))?
        {
            RuntimeFieldState::Value(value) => value.clone(),
            RuntimeFieldState::Null => RuntimeValue::Null,
            RuntimeFieldState::Unloaded => return Err(committed_error("mutation_payload_failed")),
        };
        let (mut value, _) = projected
            .resolve(
                &prepared.module.schema,
                std::slice::from_ref(&stored),
                super::RuntimeProjectionPurpose::MutationResult {
                    action: effect.action(),
                    kind,
                },
                context.auth.as_ref(),
                context.projection_context.as_deref(),
                prepared.limits,
            )
            .await
            .map_err(|_| committed_error("mutation_payload_failed"))?;
        values.insert(
            async_graphql::Name::new(&projected.api_name),
            value
                .pop()
                .ok_or_else(|| committed_error("mutation_payload_failed"))?,
        );
    }
    let payload = Value::Object(
        [(async_graphql::Name::new("record"), Value::Object(values))]
            .into_iter()
            .collect(),
    );
    reserve_payload(prepared, &payload, key)?;
    Ok(payload)
}

/// The immutable transaction-captured image presented after confirmed commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMutationRecordKind {
    CommittedResult,
    /// The deleted row's authorized preimage; never a post-commit database reread.
    DeletedPreimage,
}
/// Independent release check. Unselected/internal fields remain unloaded.
pub struct RuntimeMutationOutputCheck<'a> {
    pub schema: &'a ValidatedRuntimeSchema,
    pub collection: &'a RuntimeCollectionHandle,
    pub action: RuntimeMutationAction,
    pub kind: RuntimeMutationRecordKind,
    pub projection: &'a RuntimeProjection,
    pub record: &'a RuntimeRecord,
    /// Projected fields whose host objects will be resolved after this release check.
    pub projections: Vec<super::RuntimeProjectedSelection<'a>>,
}
fn committed_error(code: &'static str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    wire_error(code).extend_with(|_, extensions| {
        extensions.set("outcome", "committed");
        extensions.set("retryable", false);
    })
}

fn reserve_payload(prepared: &Prepared, payload: &Value, key: &str) -> async_graphql::Result<()> {
    use std::sync::atomic::Ordering;
    let maximum = prepared.limits.max_response_bytes;
    let mut bytes = key.len().saturating_mul(6).saturating_add(8);
    prepared
        .shape
        .measure(payload, &mut bytes, maximum)
        .map_err(|_| committed_error("mutation_payload_failed"))?;
    prepared
        .response_bytes
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
            used.checked_add(bytes).filter(|total| *total <= maximum)
        })
        .map_err(|_| committed_error("mutation_payload_failed"))?;
    Ok(())
}
