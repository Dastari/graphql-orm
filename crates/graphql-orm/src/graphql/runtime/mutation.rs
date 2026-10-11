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
}
impl Default for RuntimeGraphqlMutationOptions {
    fn default() -> Self {
        Self {
            create: true,
            update: true,
            delete: true,
            limits: Default::default(),
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
                .field(super::schema::output("record", TypeRef::named(&name)))
                .into(),
        );
        if options.create {
            let mut input = InputObject::new(format!("Create{name}Input"));
            let mut count = 0;
            for f in c.fields.iter().filter(|f| !f.generated) {
                let required =
                    !f.nullable && (f.default.is_none() || c.primary_key.contains(&f.id));
                input = input.field(InputValue::new(
                    &f.api_name,
                    if required {
                        TypeRef::named_nn(super::scalar::name(f.value_kind))
                    } else {
                        TypeRef::named(super::scalar::name(f.value_kind))
                    },
                ));
                count += 1;
            }
            if count == 0 {
                return Err(RuntimeGraphqlError::new("unsupported_capability"));
            }
            types.push(input.into());
        }
        if options.update {
            let mut input = InputObject::new(format!("Update{name}Input"));
            let mut count = 0;
            for f in c
                .fields
                .iter()
                .filter(|f| !f.generated && !c.primary_key.contains(&f.id))
            {
                input = input.field(InputValue::new(
                    &f.api_name,
                    TypeRef::named(super::scalar::name(f.value_kind)),
                ));
                count += 1;
            }
            if count == 0 {
                return Err(RuntimeGraphqlError::new("unsupported_capability"));
            }
            types.push(input.into());
        }
        if options.update || options.delete {
            let mut input = InputObject::new(format!("{name}KeyInput"));
            for id in &c.primary_key {
                let f = c
                    .fields
                    .iter()
                    .find(|f| &f.id == id)
                    .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
                input = input.field(InputValue::new(
                    &f.api_name,
                    TypeRef::named_nn(super::scalar::name(f.value_kind)),
                ));
            }
            types.push(input.into());
        }
    }
    Ok(types)
}

use crate::db::Database;
use std::{collections::BTreeMap, sync::Arc};
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
        if !super::module::valid_name(&field.name, false) || !self.mutation_roots.insert(field.name)
        {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.mutation = Some(
            self.mutation
                .take()
                .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?
                .field(field.field),
        );
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
            let mut field = Field::new(
                &name,
                TypeRef::named_nn(format!("{object}MutationPayload")),
                move |ctx| {
                    let database = database.clone();
                    FieldFuture::new(async move { execute(database, ctx).await.map(Some) })
                },
            )
            .argument(InputValue::new(
                "expectedSchema",
                TypeRef::named_nn("String"),
            ));
            if action != RuntimeMutationAction::Create {
                field = field
                    .argument(InputValue::new(
                        "key",
                        TypeRef::named_nn(format!("{object}KeyInput")),
                    ))
                    .argument(InputValue::new(
                        "expected",
                        TypeRef::named(format!("{object}WhereInput")),
                    ));
            }
            if action != RuntimeMutationAction::Delete {
                let kind = if action == RuntimeMutationAction::Create {
                    "Create"
                } else {
                    "Update"
                };
                field = field.argument(InputValue::new(
                    "input",
                    TypeRef::named_nn(format!("{kind}{object}Input")),
                ));
            }
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
) -> Result<super::execution::Results, RuntimeGraphqlError> {
    let mut prepared = BTreeMap::new();
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
                            if !fields.contains(&handle) {
                                fields.push(handle);
                            }
                        }
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
                    prepared.insert(
                        field.response_key().node.to_string(),
                        Prepared {
                            module: module.clone(),
                            collection,
                            request,
                            returning,
                            visibility: grant.predicate,
                            limits: planner.limits,
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
    // Pending the explicit post-commit output-authority contract decision.
    if prepared.returning.is_some() {
        return Err(wire_error("unsupported_capability"));
    }
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
        prepared,
    };
    database
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
    Ok(super::schema::field_value(
        async_graphql::value!({"record":null}),
    ))
}
