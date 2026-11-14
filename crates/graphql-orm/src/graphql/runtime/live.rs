//! Keyed runtime observations and local Node lookups. Sources and policy are host owned.
use super::plan::{Plan, Planner, invalid};
use super::*;
use crate::{
    db::Database,
    graphql::{
        orm::*,
        subscriptions::{SUBSCRIPTION_CONTINUITY_LOST, SUBSCRIPTION_SOURCE_CLOSED},
    },
};
use async_graphql::{
    Name, Value,
    dynamic::*,
    parser::types::{DocumentOperations, ExecutableDocument, Field as ParsedField, Selection},
};
use std::{collections::BTreeSet, sync::Arc};

/// Trusted host signal. A change contains a complete key, never a public snapshot.
#[derive(Clone)]
pub enum RuntimeLiveEvent {
    /// Publish only after confirmed commit, in commit order for this collection.
    Changed {
        /// Complete schema-bound key of the committed record.
        key: RuntimeKey,
        /// Committed mutation kind. Updates always reload current state.
        action: RuntimeMutationAction,
        /// Captured pre-change evidence; absence denies every removal and delete-event delivery.
        preimage: Option<RuntimeRecord>,
    },
    /// End on restart, missed batch, overflow or schema revision change.
    ContinuityLost,
    /// End when a source is intentionally closed.
    SourceClosed,
}
/// Host-owned, non-durable source. Implementations must publish only committed changes,
/// in commit order per collection, without replay. Use `MutationContext::defer` for
/// transaction-local publication. Each subscriber must get an independent receiver.
pub trait RuntimeLiveSource: Send + Sync + 'static {
    /// Register before the initial authorization/reload fence so concurrent changes
    /// cannot fall through a registration gap. No records may be released here.
    ///
    /// # Errors
    /// Return an error when an independent receiver cannot be registered safely.
    fn subscribe(
        &self,
        schema: &SchemaFingerprint,
        collection: &CollectionId,
    ) -> Result<tokio::sync::broadcast::Receiver<RuntimeLiveEvent>, RuntimeGraphqlError>;
}
/// Deleted-image policy profile. Native RLS cannot establish visibility after deletion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeLiveProfile {
    /// Safe default: refuse deleted-image opt-in, while permitting current-row reloads.
    #[default]
    NativeRls,
    /// The host independently authorizes captured images and all policy dependencies.
    ApplicationPolicy {
        /// Explicit canonical storage/collation assertion for captured-image matching.
        match_profile: RuntimeMatchProfile,
    },
}
#[derive(Clone)]
pub(super) struct Source(pub Arc<dyn RuntimeLiveSource>, pub RuntimeLiveProfile);
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuntimeLiveSource([redacted])")
    }
}
impl RuntimeGraphqlOptions {
    /// Supply a trusted committed-event source and its deleted-image authorization profile.
    pub fn with_live_source(
        mut self,
        source: Arc<dyn RuntimeLiveSource>,
        profile: RuntimeLiveProfile,
    ) -> Self {
        self.live_source = Some(Source(source, profile));
        self
    }
}
/// A default-deny application-policy check for a captured deletion or visibility-removal image. Missing
/// dependencies, another key, another schema or native RLS can never establish a grant.
pub struct RuntimeDeletedImageCheck<'a> {
    /// Installed schema against which the key and image have been validated.
    pub schema: &'a ValidatedRuntimeSchema,
    /// Complete key selected by this subscriber.
    pub key: &'a RuntimeKey,
    /// Captured pre-delete evidence; it is never serialized into a payload.
    pub image: &'a RuntimeRecord,
    /// Fields the subscriber would read, including identity/version dependencies.
    pub projection: &'a [RuntimeFieldHandle],
    /// Current row restriction from the delivery grant; the host must establish it
    /// against the image and refuse any dependency it cannot prove.
    pub required_row: Option<&'a RuntimePredicate>,
}
fn wire_error(code: &'static str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    if code == SUBSCRIPTION_CONTINUITY_LOST {
        return crate::graphql::subscriptions::subscription_end(
            code,
            "subscription lost event continuity; subscribe again, then refetch",
        );
    }
    if code == SUBSCRIPTION_SOURCE_CLOSED {
        return crate::graphql::subscriptions::subscription_end(
            code,
            "subscription event source closed; subscribe again, then refetch",
        );
    }

    async_graphql::Error::new(code).extend_with(|_, e| e.set("code", code))
}
fn safe(error: RuntimeGraphqlError) -> async_graphql::Error {
    wire_error(error.code())
}
fn document(ctx: &async_graphql::Context<'_>) -> ExecutableDocument {
    ExecutableDocument {
        operations: DocumentOperations::Single(ctx.query_env.operation.clone()),
        fragments: ctx.query_env.fragments.clone(),
    }
}
fn single_plan(
    module: &RuntimeGraphqlModule,
    c: &RuntimeCollection,
    field: &ParsedField,
    document: &ExecutableDocument,
    variables: &async_graphql::Variables,
    limits: RuntimeGraphqlLimits,
    purpose: RuntimeReadPurpose,
) -> Result<Plan, RuntimeGraphqlError> {
    let parsed =
        async_graphql::parser::parse_query("{ point(first: 1) { edges { node { __typename } } } }")
            .map_err(|_| invalid())?;
    let operation = parsed.operations.iter().next().ok_or_else(invalid)?.1;
    let Selection::Field(root) = &operation.node.selection_set.node.items[0].node else {
        return Err(invalid());
    };
    let mut root = root.node.clone();
    let Selection::Field(edges) = &mut root.selection_set.node.items[0].node else {
        return Err(invalid());
    };
    let Selection::Field(node) = &mut edges.node.selection_set.node.items[0].node else {
        return Err(invalid());
    };
    node.node.selection_set = field.selection_set.clone();
    let mut planner = Planner {
        document,
        variables,
        limits,
        expanded: 0,
        cost: Default::default(),
        relation_filters: 0,
    };
    let mut plan = planner.point_collection(module, c, &root)?;
    plan.purpose = purpose;
    Ok(plan)
}
fn node_key(module: &RuntimeGraphqlModule, c: &RuntimeCollection, id: &str) -> Option<RuntimeKey> {
    let parts = crate::graphql::federation::decode_node_id(
        id,
        &module.object_name(c),
        c.primary_key.len(),
    )?;
    let collection = module.schema.resolve_collection(&c.id).ok()?;
    let fields = c
        .primary_key
        .iter()
        .zip(parts)
        .map(|(id, value)| {
            let f = module.schema.resolve_field(&collection, id).ok()?;
            let v = match (f.value_kind(), value) {
                (RuntimeValueKind::Boolean, serde_json::Value::Bool(v)) => RuntimeValue::Boolean(v),
                (RuntimeValueKind::Integer, serde_json::Value::Number(v)) => {
                    RuntimeValue::Integer(v.as_i64()?)
                }
                (RuntimeValueKind::String, serde_json::Value::String(v)) => RuntimeValue::String(v),
                (RuntimeValueKind::Uuid, serde_json::Value::String(v)) => {
                    RuntimeValue::Uuid(uuid::Uuid::parse_str(&v).ok()?)
                }
                _ => return None,
            };
            Some((f, v))
        })
        .collect::<Option<Vec<_>>>()?;
    let key = module.schema.runtime_key(&collection, &fields).ok()?;
    let parts = key
        .fields()
        .iter()
        .map(|(_, v)| identity_value(v))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (crate::graphql::federation::encode_node_id(&module.object_name(c), parts).0 == id)
        .then_some(key)
}
pub(super) fn node_field() -> Field {
    Field::new("node", TypeRef::named("Node"), |ctx| {
        FieldFuture::new(async move {
            crate::graphql::assurance::enforce_operation_assurance(
                ctx.ctx,
                GraphqlOperationKind::Query,
                "node",
            )?;
            let results = ctx.data::<super::execution::Results>()?;
            let field = ctx.field();
            let key = field.alias().unwrap_or(field.name());
            results.admit(key)?;
            let value = results
                .0
                .get(key)
                .ok_or_else(|| wire_error("authorization_missing"))?;
            if value == &Value::Null {
                return Ok(None);
            }
            let Value::Object(mut values) = value.clone() else {
                return Err(safe(invalid()));
            };
            let Some(Value::String(typename)) = values.swap_remove(":node-type") else {
                return Err(safe(invalid()));
            };
            Ok(Some(
                super::schema::field_value(Value::Object(values)).with_type(typename),
            ))
        })
    })
    .argument(InputValue::new("NodeId", TypeRef::named_nn("ID")))
}
pub(super) fn prepare_node(
    modules: &[RuntimeGraphqlModule],
    field: &ParsedField,
    document: &ExecutableDocument,
    variables: &async_graphql::Variables,
    limits: RuntimeGraphqlLimits,
) -> Result<Option<(Plan, RuntimeKey, String)>, RuntimeGraphqlError> {
    let planner = Planner {
        document,
        variables,
        limits,
        expanded: 0,
        cost: Default::default(),
        relation_filters: 0,
    };
    let arguments = planner.resolve(field)?;
    let Some(Value::String(id)) = arguments.get("NodeId") else {
        return Ok(None);
    };
    if id.len() > limits.max_variable_bytes {
        return Ok(None);
    }
    // Extract the public type once, then use the shared canonical decoder for its key.
    use base64::Engine;
    let Some(typename) = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(id)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.as_array()?.first()?.as_str().map(str::to_owned))
    else {
        return Ok(None);
    };
    for module in modules {
        for c in module
            .schema
            .schema()
            .collections
            .iter()
            .filter(|c| c.node_identity.is_some() && module.object_name(c) == typename)
        {
            if let Some(key) = node_key(module, c, id) {
                let mut plan = single_plan(
                    module,
                    c,
                    field,
                    document,
                    variables,
                    limits,
                    RuntimeReadPurpose::NodeLookup,
                )?;
                plan.response_key = field.response_key().node.to_string();
                return Ok(Some((plan, key, typename)));
            }
        }
    }
    Ok(None)
}

fn payload_field(c: &RuntimeCollection) -> String {
    let mut name = c.api_type_name.clone();
    if let Some(first) = name.get_mut(..1) {
        first.make_ascii_lowercase();
    }
    name
}
pub(super) fn changed_name(module: &RuntimeGraphqlModule, c: &RuntimeCollection) -> String {
    let mut name = module.object_name(c);
    if let Some(first) = name.get_mut(..1) {
        first.make_ascii_lowercase();
    }
    format!("{name}Changed")
}
fn key_text(key: &RuntimeKey) -> Result<String, RuntimeGraphqlError> {
    let values = key
        .fields()
        .iter()
        .map(|(_, v)| identity_value(v))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string(&values).map_err(|_| invalid())
}
fn parse_keys(
    module: &RuntimeGraphqlModule,
    c: &RuntimeCollection,
    value: Value,
    limits: RuntimeGraphqlLimits,
) -> Result<Vec<RuntimeKey>, RuntimeGraphqlError> {
    let Value::List(values) = value else {
        return Err(invalid());
    };
    let limit = usize::from(
        c.live_subscriptions
            .as_ref()
            .ok_or_else(invalid)?
            .subscription_key_limit,
    );
    if values.len() > limit {
        return Err(RuntimeGraphqlError::new("subscription_key_limit_exceeded"));
    }
    let collection = module
        .schema
        .resolve_collection(&c.id)
        .map_err(|_| invalid())?;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let mut fields = Vec::new();
        for id in &c.primary_key {
            let field = c.fields.iter().find(|f| &f.id == id).ok_or_else(invalid)?;
            let value = if c.primary_key.len() == 1 {
                &value
            } else {
                let Value::Object(map) = &value else {
                    return Err(invalid());
                };
                map.get(field.api_name.as_str()).ok_or_else(invalid)?
            };
            let value = decode_key_value(field.value_kind, value, limits.scalars)?;
            fields.push((
                module
                    .schema
                    .resolve_field(&collection, id)
                    .map_err(|_| invalid())?,
                value,
            ));
        }
        let key = module
            .schema
            .runtime_key(&collection, &fields)
            .map_err(|_| invalid())?;
        if seen.insert(key_text(&key)?) {
            result.push(key);
        }
    }
    Ok(result)
}
// Re-check queued continuity after every asynchronous reload/policy check. No payload is
// released if lag or a host terminal signal was already observed during preparation.
fn probe(
    receiver: &mut tokio::sync::broadcast::Receiver<RuntimeLiveEvent>,
    pending: &mut std::collections::VecDeque<RuntimeLiveEvent>,
) -> Result<(), &'static str> {
    use tokio::sync::broadcast::error::TryRecvError;
    loop {
        match receiver.try_recv() {
            Ok(RuntimeLiveEvent::ContinuityLost) => return Err(SUBSCRIPTION_CONTINUITY_LOST),
            Ok(RuntimeLiveEvent::SourceClosed) => return Err(SUBSCRIPTION_SOURCE_CLOSED),
            Ok(event) => {
                if pending.len() >= 1024 {
                    return Err(SUBSCRIPTION_CONTINUITY_LOST);
                }
                pending.push_back(event);
            }
            Err(TryRecvError::Lagged(_)) => return Err(SUBSCRIPTION_CONTINUITY_LOST),
            Err(TryRecvError::Closed) => return Err(SUBSCRIPTION_SOURCE_CLOSED),
            Err(TryRecvError::Empty) => return Ok(()),
        }
    }
}

fn fence(
    receiver: &mut tokio::sync::broadcast::Receiver<RuntimeLiveEvent>,
    pending: &mut std::collections::VecDeque<RuntimeLiveEvent>,
    schema: SchemaFingerprint,
) -> Result<(), &'static str> {
    probe(receiver, pending)?;
    if pending.iter().any(|event| matches!(event, RuntimeLiveEvent::Changed { key, .. } if key.schema_fingerprint() != schema)) {
        return Err(SUBSCRIPTION_CONTINUITY_LOST);
    }
    Ok(())
}

// Count root occurrences before async-graphql opens any of its independent streams.
// Do not merge response names or let directives relax this structural bound: the
// dynamic subscription executor can open a receiver for each repeated occurrence.
pub(super) fn admit_operation(
    document: &async_graphql::parser::types::ExecutableDocument,
    operation: &async_graphql::parser::types::OperationDefinition,
    modules: &[RuntimeGraphqlModule],
    limits: RuntimeGraphqlLimits,
) -> Result<(), RuntimeGraphqlError> {
    if !modules.iter().any(|module| {
        module
            .schema
            .schema()
            .collections
            .iter()
            .any(|collection| collection.live_subscriptions.is_some())
    }) {
        return Ok(());
    }
    crate::graphql::subscription_admission::admit(
        document,
        operation,
        |name| {
            modules.iter().any(|module| {
                module.schema.schema().collections.iter().any(|collection| {
                    collection.live_subscriptions.is_some()
                        && name == changed_name(module, collection)
                })
            })
        },
        limits.max_depth,
        limits.max_selections,
    )
    .map_err(RuntimeGraphqlError::new)
}

// Authorize fresh plans for BOTH purposes. Intersect every layer, including
// nested reads and the relation-filter grants already embedded in predicates.
async fn authorize_live<B: RuntimeReadBackend>(
    original: &Plan,
    request: &RuntimeGraphqlRequest<B>,
    protection: Option<&super::execution::Protection>,
    limits: RuntimeGraphqlLimits,
) -> Result<Plan, RuntimeGraphqlError> {
    let mut registration = original.clone();
    registration.purpose = RuntimeReadPurpose::SubscriptionRegistration;
    super::execution::authorize(&mut registration, request, protection, limits).await?;
    let mut delivery = original.clone();
    delivery.purpose = RuntimeReadPurpose::SubscriptionDelivery;
    super::execution::authorize(&mut delivery, request, protection, limits).await?;
    intersect_grants(&mut delivery, &registration, B::DIALECT, limits)?;
    Ok(delivery)
}
fn intersect_grants(
    delivery: &mut Plan,
    registration: &Plan,
    backend: DatabaseBackend,
    limits: RuntimeGraphqlLimits,
) -> Result<(), RuntimeGraphqlError> {
    if !same_row_grants(delivery, registration, backend)
        && let Some(predicate) = registration.predicate.clone()
    {
        delivery.predicate = Some(match delivery.predicate.take() {
            Some(other) => delivery
                .module
                .schema
                .runtime_and(&delivery.collection, vec![predicate, other], limits.query)
                .map_err(super::plan::query_error)?,
            None => predicate,
        });
    }
    for (child, grant) in delivery.children.iter_mut().zip(&registration.children) {
        intersect_grants(child, grant, backend, limits)?;
    }
    Ok(())
}

async fn authorize_image(
    source: &Source,
    module: &RuntimeGraphqlModule,
    collection: &RuntimeCollection,
    key: &RuntimeKey,
    image: Option<&RuntimeRecord>,
    plan: &Plan,
    authority: &dyn RuntimeReadAuthority,
) -> bool {
    if !collection
        .live_subscriptions
        .as_ref()
        .is_some_and(|l| l.deleted_images)
    {
        return false;
    }
    let RuntimeLiveProfile::ApplicationPolicy { match_profile } = source.1 else {
        return false;
    };
    let Some(image) = image else {
        return false;
    };
    if key
        .fields()
        .iter()
        .any(|(f, v)| image.value(f).ok() != Some(v))
        || plan.fields.iter().any(|field| match image.state(field) {
            Ok(RuntimeFieldState::Value(_)) => false,
            Ok(RuntimeFieldState::Null) => !field.nullable(),
            _ => true,
        })
    {
        return false;
    }
    if plan.predicate.as_ref().is_some_and(|p| {
        !p.matches_captured_image(image, match_profile)
            .unwrap_or(false)
    }) {
        return false;
    }
    authority
        .authorize_deleted_image(RuntimeDeletedImageCheck {
            schema: &module.schema,
            key,
            image,
            projection: &plan.fields,
            required_row: plan.predicate.as_ref(),
        })
        .await
        .is_ok()
}

pub(super) fn subscription_field<B: RuntimeReadBackend>(
    database: Database<B>,
    module: RuntimeGraphqlModule,
    c: RuntimeCollection,
    limits: RuntimeGraphqlLimits,
    protection: Option<super::execution::Protection>,
) -> SubscriptionField {
    let object = module.object_name(&c);
    let event_name = format!("{object}ChangedEvent");
    let keys_description = keys_description(&c);
    let key_type = if c.primary_key.len() == 1 {
        let f = c
            .fields
            .iter()
            .find(|f| f.id == c.primary_key[0])
            .expect("validated key");
        key_scalar(f.value_kind).to_owned()
    } else {
        format!("{object}KeyInput")
    };
    let root = changed_name(&module, &c);
    SubscriptionField::new(
        changed_name(&module, &c),
        TypeRef::named_nn(event_name),
        move |ctx| {
            let database = database.clone();
            let module = module.clone();
            let c = c.clone();
            let protection = protection.clone();
            let root = root.clone();
            SubscriptionFieldFuture::new(async move {
                // Before registration, admission reads or any event delivery.
                crate::graphql::assurance::enforce_operation_assurance(
                    ctx.ctx,
                    GraphqlOperationKind::Subscription,
                    &root,
                )?;
                let request = ctx
                    .data_opt::<RuntimeGraphqlRequest<B>>()
                    .ok_or_else(|| wire_error("authorization_missing"))?;
                let limits = limits.restricted_by(request.budget);
                let mut variables = ctx.ctx.query_env.variables.clone();
                for definition in &ctx.ctx.query_env.operation.node.variable_definitions {
                    if !variables.contains_key(&definition.node.name.node)
                        && let Some(value) = definition.node.default_value()
                    {
                        variables.insert(definition.node.name.node.clone(), value.clone());
                    }
                }
                super::execution::bounded_variables(&variables, limits).map_err(safe)?;
                let source = module
                    .options
                    .live_source
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| wire_error("subscription_source_missing"))?;
                if let Value::List(values) = ctx
                    .args
                    .try_get("keys")
                    .map_err(|_| safe(invalid()))?
                    .as_value()
                {
                    crate::graphql::subscriptions::admit_subscription_key_count(
                        false,
                        values.len(),
                        database.subscription_key_limit(),
                    )
                    .map_err(|_| wire_error("subscription_key_limit_exceeded"))?;
                }
                let keys = parse_keys(
                    &module,
                    &c,
                    ctx.args
                        .try_get("keys")
                        .map_err(|_| safe(invalid()))?
                        .as_value()
                        .clone(),
                    limits,
                )
                .map_err(safe)?;
                let mut receiver = source
                    .0
                    .subscribe(&module.schema.fingerprint(), &c.id)
                    .map_err(safe)?;
                let mut pending = std::collections::VecDeque::new();
                let document = document(ctx.ctx);
                let mut planner = Planner {
                    document: &document,
                    variables: &variables,
                    limits,
                    expanded: 0,
                    cost: Default::default(),
                    relation_filters: 0,
                };
                let entity_field = payload_field(&c);
                let selections = planner
                    .selections(
                        &ctx.ctx.item.node.selection_set.node,
                        &format!("{}ChangedEvent", module.object_name(&c)),
                        0,
                    )
                    .map_err(safe)?;
                let shape = super::execution::OutputShape::compile(
                    &mut planner,
                    &ctx.ctx.schema_env.registry,
                    &ctx.ctx.item.node.selection_set.node,
                    &format!("{}ChangedEvent", module.object_name(&c)),
                    0,
                )
                .map_err(safe)?;
                let mut field = ctx.ctx.item.node.clone();
                field.selection_set.node.items.clear();
                for selected in selections.iter().filter(|f| f.name.node == entity_field) {
                    field
                        .selection_set
                        .node
                        .items
                        .extend(selected.selection_set.node.items.clone());
                }
                let mut plan = single_plan(
                    &module,
                    &c,
                    &field,
                    &document,
                    &variables,
                    limits,
                    RuntimeReadPurpose::SubscriptionRegistration,
                )
                .map_err(safe)?;
                // Event identity itself is disclosure: authorize every complete key member,
                // including payloads that select only `action`.
                for id in &c.primary_key {
                    let handle = module
                        .schema
                        .resolve_field(&plan.collection, id)
                        .map_err(|_| safe(invalid()))?;
                    if !plan.fields.contains(&handle) {
                        plan.fields.push(handle);
                    }
                }
                plan.projection = module
                    .schema
                    .resolve_projection(&plan.collection, &plan.fields)
                    .map_err(|_| safe(invalid()))?;
                let mut registration_cost = RuntimeGraphqlCost::default();
                for _ in &keys {
                    registration_cost = registration_cost.checked_add(plan.cost).map_err(safe)?;
                }
                registration_cost.admit(limits).map_err(safe)?;
                let original = plan.clone();
                let eligibility =
                    authorize_live(&original, request, protection.as_ref(), limits).await;
                fence(&mut receiver, &mut pending, module.schema.fingerprint())
                    .map_err(wire_error)?;
                let mut eligibility = eligibility.map_err(safe)?;
                // Keep bounded admission probes without retaining per-key visibility
                // history. Do not run projectors or fetch related records here.
                eligibility.children.clear();
                eligibility.projected.clear();
                eligibility.node_id = false;
                eligibility.record_version = false;
                eligibility
                    .fields
                    .retain(|field| c.primary_key.contains(field.id()));
                eligibility.projection = module
                    .schema
                    .resolve_projection(&eligibility.collection, &eligibility.fields)
                    .map_err(|_| safe(invalid()))?;
                for key in &keys {
                    let value = super::execution::point(
                        &database,
                        request,
                        &eligibility,
                        key,
                        protection.as_ref(),
                        limits,
                    )
                    .await;
                    fence(&mut receiver, &mut pending, module.schema.fingerprint())
                        .map_err(wire_error)?;
                    value.map_err(safe)?;
                }
                let key_set = keys
                    .iter()
                    .map(key_text)
                    .collect::<Result<BTreeSet<_>, _>>()
                    .map_err(safe)?;
                let c = Arc::new(c);
                let key_set = Arc::new(key_set);
                let original = Arc::new(original);
                let stream =
                    futures::stream::unfold(Some((receiver, pending)), move |subscription_state| {
                        let c = c.clone();
                        let key_set = key_set.clone();
                        let original = original.clone();
                        let module = module.clone();
                        let source = source.clone();
                        let database = database.clone();
                        let entity_field = entity_field.clone();
                        let shape = shape.clone();
                        let protection = protection.clone();
                        async move {
                            let (mut receiver, mut pending) = subscription_state?;
                            loop {
                                let event = match pending.pop_front() {
                                    Some(event) => event,
                                    None => match receiver.recv().await {
                                        Ok(event) => event,
                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(
                                            _,
                                        )) => {
                                            return Some((
                                                Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                                None,
                                            ));
                                        }
                                        Err(_) => {
                                            return Some((
                                                Err(wire_error(SUBSCRIPTION_SOURCE_CLOSED)),
                                                None,
                                            ));
                                        }
                                    },
                                };
                                if let Err(code) =
                                    fence(&mut receiver, &mut pending, module.schema.fingerprint())
                                {
                                    return Some((Err(wire_error(code)), None));
                                }
                                let (key, action, preimage) = match event {
                                    RuntimeLiveEvent::ContinuityLost => {
                                        return Some((
                                            Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                            None,
                                        ));
                                    }
                                    RuntimeLiveEvent::SourceClosed => {
                                        return Some((
                                            Err(wire_error(SUBSCRIPTION_SOURCE_CLOSED)),
                                            None,
                                        ));
                                    }
                                    RuntimeLiveEvent::Changed {
                                        key,
                                        action,
                                        preimage,
                                    } => (key, action, preimage),
                                };
                                if key.schema_fingerprint() != module.schema.fingerprint() {
                                    return Some((
                                        Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                        None,
                                    ));
                                }
                                if key.collection().id() != &c.id {
                                    continue;
                                }
                                // Source handles are not execution authority. Rebind the complete
                                // logical key to this installed schema before constructing any SQL.
                                if key.fields().len() != c.primary_key.len() {
                                    continue;
                                }
                                let fields = c
                                    .primary_key
                                    .iter()
                                    .map(|id| {
                                        let value = key
                                            .fields()
                                            .iter()
                                            .find(|(f, _)| f.id() == id)?
                                            .1
                                            .clone();
                                        Some((
                                            module
                                                .schema
                                                .resolve_field(&original.collection, id)
                                                .ok()?,
                                            value,
                                        ))
                                    })
                                    .collect::<Option<Vec<_>>>();
                                let Some(fields) = fields else {
                                    continue;
                                };
                                let Ok(key) =
                                    module.schema.runtime_key(&original.collection, &fields)
                                else {
                                    continue;
                                };
                                let Ok(identity) = key_text(&key) else {
                                    continue;
                                };
                                if !key_set.contains(&identity) {
                                    continue;
                                }
                                let grant =
                                    authorize_live(&original, request, protection.as_ref(), limits)
                                        .await;
                                if let Err(code) =
                                    fence(&mut receiver, &mut pending, module.schema.fingerprint())
                                {
                                    return Some((Err(wire_error(code)), None));
                                }
                                let delivery = match grant {
                                    Ok(plan) => plan,
                                    // Match derived subscriptions: revocation withholds events
                                    // quietly and does not expose whether this key ever existed.
                                    Err(error) if error == RuntimeGraphqlError::denied() => {
                                        continue;
                                    }
                                    Err(_) => {
                                        return Some((
                                            Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                            None,
                                        ));
                                    }
                                };
                                let mut image_proved = false;
                                if action == RuntimeMutationAction::Delete {
                                    image_proved = authorize_image(
                                        &source,
                                        &module,
                                        &c,
                                        &key,
                                        preimage.as_ref(),
                                        &delivery,
                                        request.authority.as_ref(),
                                    )
                                    .await;
                                    if let Err(code) = fence(
                                        &mut receiver,
                                        &mut pending,
                                        module.schema.fingerprint(),
                                    ) {
                                        return Some((Err(wire_error(code)), None));
                                    }
                                    if !image_proved {
                                        continue;
                                    }
                                }
                                let current = super::execution::point(
                                    &database,
                                    request,
                                    &delivery,
                                    &key,
                                    protection.as_ref(),
                                    limits,
                                )
                                .await;
                                if let Err(code) =
                                    fence(&mut receiver, &mut pending, module.schema.fingerprint())
                                {
                                    return Some((Err(wire_error(code)), None));
                                }
                                let value = match current {
                                    Ok(value) => value,
                                    Err(_) => {
                                        return Some((
                                            Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                            None,
                                        ));
                                    }
                                };
                                // A queued update can reload after a later deletion.
                                // Every tombstone therefore needs the same image proof,
                                // regardless of the event's original mutation kind.
                                if value.is_none() && !image_proved {
                                    image_proved = authorize_image(
                                        &source,
                                        &module,
                                        &c,
                                        &key,
                                        preimage.as_ref(),
                                        &delivery,
                                        request.authority.as_ref(),
                                    )
                                    .await;
                                    if let Err(code) = fence(
                                        &mut receiver,
                                        &mut pending,
                                        module.schema.fingerprint(),
                                    ) {
                                        return Some((Err(wire_error(code)), None));
                                    }
                                    if !image_proved {
                                        continue;
                                    }
                                }
                                let grant =
                                    authorize_live(&original, request, protection.as_ref(), limits)
                                        .await;
                                if let Err(code) =
                                    fence(&mut receiver, &mut pending, module.schema.fingerprint())
                                {
                                    return Some((Err(wire_error(code)), None));
                                }
                                let release = match grant {
                                    Ok(plan) => plan,
                                    Err(error) if error == RuntimeGraphqlError::denied() => {
                                        continue;
                                    }
                                    Err(_) => {
                                        return Some((
                                            Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                            None,
                                        ));
                                    }
                                };
                                if !same_grants(&delivery, &release, B::DIALECT) {
                                    return Some((
                                        Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                        None,
                                    ));
                                }
                                if image_proved {
                                    let approved = authorize_image(
                                        &source,
                                        &module,
                                        &c,
                                        &key,
                                        preimage.as_ref(),
                                        &release,
                                        request.authority.as_ref(),
                                    )
                                    .await;
                                    if let Err(code) = fence(
                                        &mut receiver,
                                        &mut pending,
                                        module.schema.fingerprint(),
                                    ) {
                                        return Some((Err(wire_error(code)), None));
                                    }
                                    if !approved {
                                        continue;
                                    }
                                }
                                if let Err(code) =
                                    fence(&mut receiver, &mut pending, module.schema.fingerprint())
                                {
                                    return Some((Err(wire_error(code)), None));
                                }
                                let mut payload = async_graphql::indexmap::IndexMap::new();
                                payload.insert(
                                    Name::new("action"),
                                    Value::Enum(Name::new(if value.is_none() {
                                        "DELETED"
                                    } else {
                                        match action {
                                            RuntimeMutationAction::Create => "CREATED",
                                            _ => "UPDATED",
                                        }
                                    })),
                                );
                                payload.insert(
                                    Name::new("changeKind"),
                                    Value::Enum(Name::new("DIRECT")),
                                );
                                let id = match event_identity(&key) {
                                    Ok(value) => value,
                                    Err(_) => {
                                        return Some((
                                            Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                            None,
                                        ));
                                    }
                                };
                                payload.insert(Name::new("id"), id);
                                if key.fields().len() > 1 {
                                    let mut output = async_graphql::indexmap::IndexMap::new();
                                    for (field, value) in key.fields() {
                                        let name = c
                                            .fields
                                            .iter()
                                            .find(|f| &f.id == field.id())
                                            .expect("key")
                                            .api_name
                                            .as_str();
                                        let value = match value {
                                            RuntimeValue::Integer(v) => Value::Number((*v).into()),
                                            v => scalar::encode(v),
                                        };
                                        output.insert(Name::new(name), value);
                                    }
                                    payload.insert(Name::new("key"), Value::Object(output));
                                }
                                payload.insert(Name::new("sourceEntity"), Value::Null);
                                payload.insert(Name::new("sourceId"), Value::Null);
                                payload.insert(Name::new("path"), Value::List(Vec::new()));
                                payload
                                    .insert(Name::new(&entity_field), value.unwrap_or(Value::Null));
                                let payload = Value::Object(payload);
                                if shape
                                    .measure(&payload, &mut 0, limits.max_response_bytes)
                                    .is_err()
                                {
                                    return Some((
                                        Err(wire_error(SUBSCRIPTION_CONTINUITY_LOST)),
                                        None,
                                    ));
                                }
                                return Some((
                                    Ok(super::schema::field_value(payload)),
                                    Some((receiver, pending)),
                                ));
                            }
                        }
                    });
                Ok(stream)
            })
        },
    )
    .description("Observe committed changes to selected keys")
    .argument(
        InputValue::new("keys", TypeRef::named_nn_list(key_type)).description(keys_description),
    )
}

pub(super) fn helpers() -> Vec<Type> {
    vec![
        Scalar::new("UUID").into(),
        Enum::new("ChangeAction")
            .item(EnumItem::new("CREATED"))
            .item(EnumItem::new("UPDATED"))
            .item(EnumItem::new("DELETED"))
            .into(),
        Enum::new("ChangeKind")
            .item(EnumItem::new("DIRECT"))
            .item(EnumItem::new("PROPAGATED"))
            .into(),
    ]
}
pub(super) fn types(module: &RuntimeGraphqlModule, c: &RuntimeCollection) -> Vec<Type> {
    let object = module.object_name(c);
    let mut result = Vec::new();
    if c.primary_key.len() > 1
        && !module
            .options
            .mutations
            .as_ref()
            .is_some_and(|o| o.update || o.delete)
    {
        let mut key = InputObject::new(format!("{object}KeyInput"));
        for id in &c.primary_key {
            let field = c
                .fields
                .iter()
                .find(|f| &f.id == id)
                .expect("validated key");
            key = key.field(InputValue::new(
                &field.api_name,
                TypeRef::named_nn(key_scalar(field.value_kind)),
            ));
        }
        result.push(key.into());
    }
    let id_type = if c.primary_key.len() == 1 {
        key_scalar(
            c.fields
                .iter()
                .find(|f| f.id == c.primary_key[0])
                .expect("validated key")
                .value_kind,
        )
    } else {
        "String"
    };
    if c.primary_key.len() > 1 {
        let mut key = Object::new(format!("{object}Key"));
        for id in &c.primary_key {
            let field = c.fields.iter().find(|f| &f.id == id).expect("key");
            key = key.field(super::schema::output(
                &field.api_name,
                TypeRef::named_nn(key_scalar(field.value_kind)),
            ));
        }
        result.push(key.into());
    }
    let mut event = Object::new(format!("{object}ChangedEvent"))
        .description(format!("Live changes of {object}"));
    if c.primary_key.len() > 1 {
        event = event.field(super::schema::output(
            "key",
            TypeRef::named_nn(format!("{object}Key")),
        ));
    }
    for (name, ty) in [
        ("action", TypeRef::named_nn("ChangeAction")),
        ("changeKind", TypeRef::named_nn("ChangeKind")),
        ("id", TypeRef::named_nn(id_type)),
        ("sourceEntity", TypeRef::named("String")),
        ("sourceId", TypeRef::named("String")),
        ("path", TypeRef::named_nn_list_nn("String")),
    ] {
        event = event.field(super::schema::output(name, ty).description(field_description(name)));
    }
    event = event.field(super::schema::output(
        &payload_field(c),
        TypeRef::named(object),
    ));
    result.push(event.into());
    result
}
pub(super) fn semantic_field(
    name: &str,
    ty: GraphqlSemanticTypeRef,
    c: &RuntimeCollection,
) -> GraphqlSemanticFieldMetadata {
    GraphqlSemanticFieldMetadata {
        field_name: name.into(),
        description: field_description(name),
        type_ref: ty,
        selectable: true,
        filter_operators: Vec::new(),
        filter_input: None,
        sortable: false,
        groupable: false,
        aggregate_operators: Vec::new(),
        aggregate_value_kind: None,
        relationship: None,
        classification: c.effective_classification(),
        export: GraphqlSemanticExport::Exportable,
        has_field_policy: true,
    }
}
pub(super) fn event_semantics(
    module: &RuntimeGraphqlModule,
    c: &RuntimeCollection,
) -> Result<
    (
        Vec<GraphqlEntitySemanticMetadata>,
        GraphqlSemanticOperationDescriptor,
    ),
    RuntimeGraphqlError,
> {
    let object = module.object_name(c);
    let event = format!("{object}ChangedEvent");
    let named = |name: &str, kind, nullable| GraphqlSemanticTypeRef::named(name, kind, nullable);
    let mut fields = Vec::new();
    for (name, ty, kind, nullable) in [
        (
            "action",
            "ChangeAction",
            GraphqlSemanticTypeKind::Enum,
            false,
        ),
        (
            "changeKind",
            "ChangeKind",
            GraphqlSemanticTypeKind::Enum,
            false,
        ),
        (
            "sourceEntity",
            "String",
            GraphqlSemanticTypeKind::Scalar,
            true,
        ),
        ("sourceId", "String", GraphqlSemanticTypeKind::Scalar, true),
    ] {
        fields.push(semantic_field(name, named(ty, kind, nullable), c));
    }
    let key_type = if c.primary_key.len() == 1 {
        key_scalar(
            c.fields
                .iter()
                .find(|f| f.id == c.primary_key[0])
                .expect("key")
                .value_kind,
        )
        .to_owned()
    } else {
        format!("{object}KeyInput")
    };
    let id_type = if c.primary_key.len() == 1 {
        key_type.as_str()
    } else {
        "String"
    };
    fields.push(dependent_semantic(
        "id",
        named(id_type, GraphqlSemanticTypeKind::Scalar, false),
        c,
        &c.primary_key,
    ));
    fields.push(semantic_field(
        "path",
        GraphqlSemanticTypeRef::list(
            false,
            None,
            named("String", GraphqlSemanticTypeKind::Scalar, false),
        ),
        c,
    ));
    let mut extra_entities = Vec::new();
    if c.primary_key.len() > 1 {
        let key_name = format!("{object}Key");
        let key_fields = c
            .primary_key
            .iter()
            .map(|id| {
                let field = c.fields.iter().find(|f| &f.id == id).expect("key");
                dependent_semantic(
                    &field.api_name,
                    named(
                        key_scalar(field.value_kind),
                        GraphqlSemanticTypeKind::Scalar,
                        false,
                    ),
                    c,
                    std::slice::from_ref(id),
                )
            })
            .collect::<Vec<_>>();
        extra_entities.push(GraphqlEntitySemanticMetadata {
            generated_input_members: Vec::new(),
            entity_name: key_name.clone(),
            description: "Complete runtime key".into(),
            default_classification: c.effective_classification(),
            fields: key_fields.into_boxed_slice(),
            label_fields: Vec::new(),
            record_version: None,
        });
        let mut key = dependent_semantic(
            "key",
            named(&key_name, GraphqlSemanticTypeKind::Object, false),
            c,
            &c.primary_key,
        );
        key.relationship = Some(GraphqlSemanticRelationshipDescriptor {
            target: key_name,
            cardinality: GraphqlSemanticRelationshipCardinality::One,
            arguments: Vec::new(),
            collection_bound: None,
        });
        fields.push(key);
    }
    let mut entity = semantic_field(
        &payload_field(c),
        named(&object, GraphqlSemanticTypeKind::Object, true),
        c,
    );
    entity.relationship = Some(GraphqlSemanticRelationshipDescriptor {
        target: object.clone(),
        cardinality: GraphqlSemanticRelationshipCardinality::One,
        arguments: Vec::new(),
        collection_bound: None,
    });
    fields.push(entity);
    let entity = GraphqlEntitySemanticMetadata {
        generated_input_members: Vec::new(),
        entity_name: event.clone(),
        description: format!("Live changes of {object}"),
        default_classification: c.effective_classification(),
        fields: fields.into_boxed_slice(),
        label_fields: Vec::new(),
        record_version: None,
    };
    let operation = GraphqlSemanticOperationDescriptor::generated(
        GraphqlOperationKind::Subscription,
        GeneratedGraphqlOperationCategory::Subscription,
        object,
        changed_name(module, c),
        "Observe committed changes to selected keys",
        vec![GraphqlSemanticArgumentDescriptor {
            graphql_name: "keys".into(),
            description: keys_description(c),
            type_ref: GraphqlSemanticTypeRef::list(
                true,
                Some(u32::from(
                    c.live_subscriptions
                        .as_ref()
                        .expect("live declaration")
                        .subscription_key_limit,
                )),
                named(
                    &key_type,
                    if c.primary_key.len() == 1 {
                        GraphqlSemanticTypeKind::Scalar
                    } else {
                        GraphqlSemanticTypeKind::Object
                    },
                    false,
                ),
            ),
        }],
        named(&event, GraphqlSemanticTypeKind::Object, false),
        true,
    )
    .map_err(|_| invalid())?
    .with_subscription_observation(GraphqlSubscriptionObservationDescriptor {
        replay_mode: GraphqlSubscriptionReplayMode::BestEffort,
        maximum_duration_seconds: None,
        maximum_events: None,
        condition_fields: Vec::new(),
    })
    .map_err(|_| invalid())?;
    extra_entities.push(entity);
    Ok((extra_entities, operation))
}
pub(super) fn node_semantic() -> Result<GraphqlSemanticOperationDescriptor, RuntimeGraphqlError> {
    GraphqlSemanticOperationDescriptor::custom(
        GraphqlOperationKind::Query,
        "node",
        "Look up one currently visible record by global identity",
        vec![GraphqlSemanticArgumentDescriptor {
            graphql_name: "NodeId".into(),
            description: "Global record identity".into(),
            type_ref: parse_graphql_type("ID!").map_err(|_| invalid())?,
        }],
        GraphqlSemanticTypeRef::named("Node", GraphqlSemanticTypeKind::Object, true),
        true,
    )
    .and_then(|op| op.with_result_disclosure(GraphqlSemanticResultDisclosure::fail_safe()))
    .map_err(|_| invalid())
}

pub(super) fn identity_value(
    value: &RuntimeValue,
) -> Result<serde_json::Value, RuntimeGraphqlError> {
    Ok(match value {
        RuntimeValue::Boolean(v) => (*v).into(),
        RuntimeValue::Integer(v) => (*v).into(),
        RuntimeValue::String(v) => v.clone().into(),
        RuntimeValue::Uuid(v) => v.to_string().into(),
        _ => return Err(invalid()),
    })
}

impl std::fmt::Debug for RuntimeLiveEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuntimeLiveEvent([redacted])")
    }
}
pub(super) fn dependent_semantic(
    name: &str,
    ty: GraphqlSemanticTypeRef,
    c: &RuntimeCollection,
    dependencies: &[FieldId],
) -> GraphqlSemanticFieldMetadata {
    let mut result = semantic_field(name, ty, c);
    for id in dependencies {
        if let Some(field) = c.fields.iter().find(|f| &f.id == id) {
            result.classification = result.classification.max(field.effective_classification(c));
            if field.effective_export(c) == GraphqlSemanticExport::NeverExport {
                result.export = GraphqlSemanticExport::NeverExport;
            }
        }
    }
    result
}

pub(super) fn key_scalar(kind: RuntimeValueKind) -> &'static str {
    match kind {
        RuntimeValueKind::Integer => "Int",
        RuntimeValueKind::Uuid => "UUID",
        _ => scalar::name(kind),
    }
}

// Live keys follow the derived scalar contract; ordinary RuntimeInt64 values
// remain strings. This decoder is used only for these explicitly typed keys.
pub(super) fn decode_key_value(
    kind: RuntimeValueKind,
    value: &Value,
    limits: RuntimeScalarLimits,
) -> Result<RuntimeValue, RuntimeGraphqlError> {
    if let (RuntimeValueKind::Integer, Value::Number(value)) = (kind, value) {
        return value
            .as_i64()
            .map(RuntimeValue::Integer)
            .ok_or_else(invalid);
    }
    scalar::decode(kind, value, limits)
}

fn event_identity(key: &RuntimeKey) -> Result<Value, RuntimeGraphqlError> {
    if key.fields().len() == 1 {
        return match &key.fields()[0].1 {
            RuntimeValue::Integer(v) => Ok(Value::Number((*v).into())),
            value => Ok(scalar::encode(value)),
        };
    }
    let values = key
        .fields()
        .iter()
        .map(|(_, value)| {
            Ok(match value {
                RuntimeValue::Boolean(v) => SqlValue::Bool(*v),
                RuntimeValue::String(v) => SqlValue::String(v.clone()),
                RuntimeValue::Integer(v) => SqlValue::Int(*v),
                RuntimeValue::Uuid(v) => SqlValue::Uuid(*v),
                _ => return Err(invalid()),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::String(composite_key_id(&values)))
}

impl RuntimeLiveEvent {
    /// Capture a pending runtime effect for a trusted source. This does **not**
    /// establish commit: publish it only from `MutationContext::defer` after commit.
    /// Process `RuntimeMutationEffect::dependents` in their execution order as well.
    ///
    /// # Errors
    /// Rejects another schema/collection or missing/invalid complete-key evidence.
    pub fn from_effect(
        schema: &ValidatedRuntimeSchema,
        effect: &RuntimeMutationEffect,
    ) -> Result<Self, RuntimeMutationError> {
        let collection = schema.resolve_collection(effect.collection().id())?;
        if &collection != effect.collection() {
            return Err(RuntimeMutationError::new(
                RuntimeMutationErrorCode::SchemaMismatch,
            ));
        }
        let definition = schema
            .schema()
            .collections
            .iter()
            .find(|c| &c.id == collection.id())
            .ok_or_else(|| RuntimeMutationError::new(RuntimeMutationErrorCode::SchemaMismatch))?;
        let fields = definition
            .primary_key
            .iter()
            .map(|id| {
                let field = schema.resolve_field(&collection, id)?;
                let value = effect.staged().value(&field)?.clone();
                Ok((field, value))
            })
            .collect::<Result<Vec<_>, RuntimeMutationError>>()?;
        Ok(Self::Changed {
            key: schema.runtime_key(&collection, &fields)?,
            action: effect.action(),
            preimage: effect.preimage().cloned(),
        })
    }
}
pub(super) fn field_description(name: &str) -> String {
    match name {
        "NodeId" => "Global identity of the complete primary key".into(),
        "RecordVersion" => "Managed record version advanced by every runtime write".into(),
        "action" => "Current change or removal".into(),
        "id" | "key" => "Authorized complete record identity".into(),
        "changeKind" => "Direct change provenance".into(),
        "sourceEntity" | "sourceId" => "No cross-collection origin is disclosed".into(),
        "path" => "Empty for direct runtime changes".into(),
        _ => format!("Current authorized {name}"),
    }
}
fn keys_description(c: &RuntimeCollection) -> String {
    format!(
        "Complete keys to observe (IR maximum {})",
        c.live_subscriptions
            .as_ref()
            .expect("live declaration")
            .subscription_key_limit
    )
}

pub(super) fn insert_identity_fields(
    module: &RuntimeGraphqlModule,
    metadata: &RuntimeCollection,
    collection: &RuntimeCollectionHandle,
    record: &RuntimeRecord,
    node_id: bool,
    version: bool,
    values: &mut async_graphql::indexmap::IndexMap<Name, Value>,
) -> Result<(), RuntimeGraphqlError> {
    let Some(identity) = &metadata.node_identity else {
        return Ok(());
    };
    if node_id {
        let parts = metadata
            .primary_key
            .iter()
            .map(|id| {
                let field = module
                    .schema
                    .resolve_field(collection, id)
                    .map_err(|_| invalid())?;
                identity_value(record.value(&field).map_err(|_| invalid())?)
            })
            .collect::<Result<Vec<_>, _>>()?;
        values.insert(
            Name::new("NodeId"),
            Value::String(
                crate::graphql::federation::encode_node_id(&module.object_name(metadata), parts).0,
            ),
        );
    }
    if version {
        let field = module
            .schema
            .resolve_field(collection, &identity.version_field)
            .map_err(|_| invalid())?;
        let RuntimeValue::Integer(version) = record.value(&field).map_err(|_| invalid())? else {
            return Err(invalid());
        };
        values.insert(Name::new("RecordVersion"), Value::Number((*version).into()));
    }
    Ok(())
}

fn same_row_grants(left: &Plan, right: &Plan, backend: DatabaseBackend) -> bool {
    match (&left.predicate, &right.predicate) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let mut av = Vec::new();
            let mut bv = Vec::new();
            a.render(backend, &mut av) == b.render(backend, &mut bv) && av == bv
        }
        _ => false,
    }
}
fn same_grants(left: &Plan, right: &Plan, backend: DatabaseBackend) -> bool {
    same_row_grants(left, right, backend)
        && left.collection == right.collection
        && left.children.len() == right.children.len()
        && left
            .children
            .iter()
            .zip(&right.children)
            .all(|(a, b)| same_grants(a, b, backend))
}
