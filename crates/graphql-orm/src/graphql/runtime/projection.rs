//! Reviewed host projections for formatted runtime fields.
//!
//! A field carrying a [`RuntimeFieldFormat`] is served either as its plain
//! stored scalar, explicitly allowed with
//! [`RuntimeGraphqlOptions::scalar_format`](super::RuntimeGraphqlOptions::scalar_format),
//! or through a [`RuntimeFieldProjector`] bound by stable field ID with
//! [`RuntimeGraphqlOptions::project_field`](super::RuntimeGraphqlOptions::project_field).
//! A projector declares an owned, flat output object. The ORM registers that
//! object type itself, keeps inputs, filters and ordering on the stored scalar,
//! and calls the projector only with stored values from rows and fields the
//! host read or mutation-output authority already released. There is no raw
//! builder hook: the projector returns typed values that the ORM validates,
//! restricts to the client's selection and encodes within the scalar and
//! response budgets.

use super::RuntimeGraphqlError;
use crate::graphql::orm::{
    DbAuthContext, FieldId, RuntimeFieldFormat, RuntimeFieldHandle, RuntimeFormatNamespace,
    RuntimeMutationAction, RuntimeValue, RuntimeValueKind, ValidatedRuntimeSchema,
};
use futures::future::BoxFuture;
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
    sync::Arc,
    time::Duration,
};

/// Maximum fields of one projected output object.
pub const MAX_PROJECTED_OBJECT_FIELDS: usize = 32;

/// One scalar field of a projected output object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProjectedField {
    name: String,
    kind: RuntimeValueKind,
    nullable: bool,
}

impl RuntimeProjectedField {
    /// Declare a lower-camel GraphQL field of a runtime scalar kind.
    pub fn new(name: impl Into<String>, kind: RuntimeValueKind, nullable: bool) -> Self {
        Self {
            name: name.into(),
            kind,
            nullable,
        }
    }

    /// GraphQL field name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runtime scalar kind.
    pub const fn kind(&self) -> RuntimeValueKind {
        self.kind
    }

    /// Whether the projector may omit or null this field.
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}

/// Owned, flat output object registered by the ORM for a projector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProjectedObject {
    name: String,
    fields: Vec<RuntimeProjectedField>,
}

impl RuntimeProjectedObject {
    /// Validate an object type name and its unique scalar fields.
    ///
    /// # Errors
    ///
    /// `invalid_composition` for invalid or duplicate names and
    /// `cost_exceeded` for an empty object or more than
    /// [`MAX_PROJECTED_OBJECT_FIELDS`] fields.
    pub fn new(
        name: impl Into<String>,
        fields: Vec<RuntimeProjectedField>,
    ) -> Result<Self, RuntimeGraphqlError> {
        let name = name.into();
        if fields.is_empty() || fields.len() > MAX_PROJECTED_OBJECT_FIELDS {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        let mut seen = BTreeSet::new();
        if !super::module::valid_name(&name, true)
            || !fields.iter().all(|field| {
                super::module::valid_name(&field.name, false) && seen.insert(field.name.clone())
            })
        {
            return Err(RuntimeGraphqlError::new("invalid_composition"));
        }
        Ok(Self { name, fields })
    }

    /// GraphQL object type name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Declared fields in output order.
    pub fn fields(&self) -> &[RuntimeProjectedField] {
        &self.fields
    }

    pub(super) fn field(&self, name: &str) -> Option<&RuntimeProjectedField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// Declarative contract read once when a module is compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProjectionContract {
    /// Accepted descriptor namespace.
    pub namespace: RuntimeFormatNamespace,
    /// Accepted descriptor versions.
    pub versions: RangeInclusive<u16>,
    /// Accepted stored value kinds.
    pub value_kinds: BTreeSet<RuntimeValueKind>,
    /// Output object type.
    pub object: RuntimeProjectedObject,
    /// Whether a stored value may resolve to no object; the output is then null.
    pub may_be_absent: bool,
}

impl RuntimeProjectionContract {
    pub(super) fn accepts(&self, kind: RuntimeValueKind, format: &RuntimeFieldFormat) -> bool {
        self.namespace == format.namespace
            && self.versions.contains(&format.version.get())
            && self.value_kinds.contains(&kind)
    }
}

/// Which released record a projection call serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeProjectionPurpose {
    /// Authorized query results.
    Read,
    /// A committed mutation payload released by `authorize_mutation_result`.
    MutationResult {
        action: RuntimeMutationAction,
        kind: super::RuntimeMutationRecordKind,
    },
}

/// One bounded batch of distinct, non-null, already-authorized stored values.
/// Everything is borrowed for the duration of the call only.
pub struct RuntimeProjectionBatch<'a> {
    pub schema: &'a ValidatedRuntimeSchema,
    pub field: &'a RuntimeFieldHandle,
    pub format: &'a RuntimeFieldFormat,
    pub values: &'a [RuntimeValue],
    /// Output object fields the client selected; only these are emitted.
    pub selected: &'a [&'a str],
    pub purpose: RuntimeProjectionPurpose,
    pub auth: Option<&'a DbAuthContext>,
    /// Trusted request data supplied by the host, if any.
    pub context: Option<&'a (dyn Any + Send + Sync)>,
}

/// Projector outcome for one stored value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeProjected {
    /// Output field values; fields not selected are discarded by the ORM.
    Object(BTreeMap<String, RuntimeValue>),
    /// No object exists; valid only when the contract allows absence.
    Absent,
}

/// Outcomes keyed by the batch's stored values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeProjectionResults {
    entries: Vec<(RuntimeValue, RuntimeProjected)>,
}

impl RuntimeProjectionResults {
    /// Empty result set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the outcome for one requested stored value.
    pub fn insert(&mut self, value: RuntimeValue, outcome: RuntimeProjected) {
        self.entries.push((value, outcome));
    }

    pub(super) fn into_entries(self) -> Vec<(RuntimeValue, RuntimeProjected)> {
        self.entries
    }
}

/// Value-free projector failure. Every variant fails the whole operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeProjectionError {
    /// The host refuses to project for this request.
    Denied,
    /// A host dependency is unavailable.
    Unavailable,
    /// The batch is invalid for this projector.
    Invalid,
}

/// Reviewed host projection of one formatted field into an output object.
///
/// Implementations must be read-only and cancel-safe: the ORM drops the future
/// when the request is cancelled or the call exceeds its time budget.
pub trait RuntimeFieldProjector: Send + Sync + 'static {
    /// The owned contract; read once at module compilation.
    fn contract(&self) -> &RuntimeProjectionContract;

    /// Resolve every value in the batch.
    fn resolve<'a>(
        &'a self,
        batch: RuntimeProjectionBatch<'a>,
    ) -> BoxFuture<'a, Result<RuntimeProjectionResults, RuntimeProjectionError>>;
}

/// Per-operation projection budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeProjectionLimits {
    /// Projector calls per operation (one per projected field per layer).
    pub max_calls: usize,
    /// Distinct stored values per call.
    pub max_values_per_call: usize,
    /// Time budget of one call.
    pub call_timeout: Duration,
}

impl Default for RuntimeProjectionLimits {
    fn default() -> Self {
        Self {
            max_calls: 16,
            max_values_per_call: 1000,
            call_timeout: Duration::from_secs(5),
        }
    }
}

impl RuntimeProjectionLimits {
    pub(super) fn restricted_by(self, other: Self) -> Self {
        Self {
            max_calls: self.max_calls.min(other.max_calls),
            max_values_per_call: self.max_values_per_call.min(other.max_values_per_call),
            call_timeout: self.call_timeout.min(other.call_timeout),
        }
    }
}

/// One projected field and its requested object fields, shown to authorities.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeProjectedSelection<'a> {
    pub field: &'a RuntimeFieldHandle,
    pub object: &'a str,
    pub fields: &'a [String],
}

/// Compiled binding; the contract is copied once at compilation.
pub(super) struct ProjectionBinding {
    pub projector: Arc<dyn RuntimeFieldProjector>,
    pub contract: RuntimeProjectionContract,
}

/// Host-supplied projector reference with redacted formatting.
#[derive(Clone)]
pub(super) struct ProjectorRef(pub Arc<dyn RuntimeFieldProjector>);

impl std::fmt::Debug for ProjectorRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuntimeFieldProjector")
    }
}

/// Planned projection of one field in one layer.
#[derive(Clone)]
pub(super) struct ProjectedPlan {
    pub field: RuntimeFieldHandle,
    pub api_name: String,
    pub binding: Arc<ProjectionBinding>,
    pub selected: Vec<String>,
}

impl ProjectedPlan {
    pub(super) fn selection(&self) -> RuntimeProjectedSelection<'_> {
        RuntimeProjectedSelection {
            field: &self.field,
            object: self.binding.contract.object.name(),
            fields: &self.selected,
        }
    }

    /// Call the projector once and return one output value per input record value,
    /// plus the encoded bytes to reserve against the response budget.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn resolve(
        &self,
        schema: &ValidatedRuntimeSchema,
        stored: &[RuntimeValue],
        purpose: RuntimeProjectionPurpose,
        auth: Option<&DbAuthContext>,
        context: Option<&(dyn Any + Send + Sync)>,
        limits: super::RuntimeGraphqlLimits,
    ) -> Result<(Vec<async_graphql::Value>, usize), RuntimeGraphqlError> {
        let mut bytes = 0usize;
        let failed = || RuntimeGraphqlError::new("projection_failed");
        let mut distinct: Vec<RuntimeValue> = Vec::new();
        let mut keys = BTreeMap::new();
        for value in stored {
            if matches!(value, RuntimeValue::Null) {
                continue;
            }
            let key = serde_json::to_string(value).map_err(|_| failed())?;
            if !keys.contains_key(&key) {
                keys.insert(key, distinct.len());
                distinct.push(value.clone());
            }
        }
        if distinct.len() > limits.projection.max_values_per_call {
            return Err(RuntimeGraphqlError::new("cost_exceeded"));
        }
        let mut outputs: Vec<Option<async_graphql::Value>> = vec![None; distinct.len()];
        if !distinct.is_empty() {
            let format = self.field.format().ok_or_else(failed)?;
            let selected = self.selected.iter().map(String::as_str).collect::<Vec<_>>();
            let batch = RuntimeProjectionBatch {
                schema,
                field: &self.field,
                format,
                values: &distinct,
                selected: &selected,
                purpose,
                auth,
                context,
            };
            let results = match tokio::time::timeout(
                limits.projection.call_timeout,
                self.binding.projector.resolve(batch),
            )
            .await
            {
                Err(_) => return Err(RuntimeGraphqlError::new("projection_timeout")),
                Ok(Err(RuntimeProjectionError::Denied)) => {
                    return Err(RuntimeGraphqlError::new("projection_denied"));
                }
                Ok(Err(_)) => return Err(failed()),
                Ok(Ok(results)) => results,
            };
            let contract = &self.binding.contract;
            for (value, outcome) in results.into_entries() {
                let key = serde_json::to_string(&value).map_err(|_| failed())?;
                let index = *keys.get(&key).ok_or_else(failed)?;
                if outputs[index].is_some() {
                    return Err(failed());
                }
                outputs[index] = Some(match outcome {
                    RuntimeProjected::Absent if contract.may_be_absent => {
                        async_graphql::Value::Null
                    }
                    RuntimeProjected::Absent => return Err(failed()),
                    RuntimeProjected::Object(fields) => {
                        if fields
                            .keys()
                            .any(|name| contract.object.field(name).is_none())
                        {
                            return Err(failed());
                        }
                        let mut object = async_graphql::indexmap::IndexMap::new();
                        for name in &self.selected {
                            let declared = contract.object.field(name).ok_or_else(failed)?;
                            let encoded = match fields.get(name) {
                                None | Some(RuntimeValue::Null) if declared.nullable => {
                                    async_graphql::Value::Null
                                }
                                Some(value) if value.kind() == Some(declared.kind) => {
                                    super::scalar::encode(value, limits.scalars)
                                        .map_err(|_| failed())?
                                }
                                _ => return Err(failed()),
                            };
                            bytes = bytes
                                .checked_add(match &encoded {
                                    async_graphql::Value::String(text) => text.len(),
                                    _ => 16,
                                })
                                .ok_or_else(failed)?;
                            object.insert(async_graphql::Name::new(name), encoded);
                        }
                        async_graphql::Value::Object(object)
                    }
                });
            }
        }
        let values = stored
            .iter()
            .map(|value| {
                if matches!(value, RuntimeValue::Null) {
                    return Ok(async_graphql::Value::Null);
                }
                let key = serde_json::to_string(value).map_err(|_| failed())?;
                outputs[keys[&key]].clone().ok_or_else(failed)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((values, bytes))
    }
}

/// Validate bindings and scalar allowances against a schema before serving.
pub(super) fn compile(
    schema: &ValidatedRuntimeSchema,
    projections: &BTreeMap<FieldId, ProjectorRef>,
    scalar_formats: &[(RuntimeFormatNamespace, RangeInclusive<u16>)],
) -> Result<BTreeMap<FieldId, Arc<ProjectionBinding>>, RuntimeGraphqlError> {
    let mut compiled = BTreeMap::new();
    let mut objects: BTreeMap<String, RuntimeProjectedObject> = BTreeMap::new();
    for (id, projector) in projections {
        let (collection, field) = schema
            .schema()
            .collections
            .iter()
            .find_map(|c| c.fields.iter().find(|f| &f.id == id).map(|f| (c, f)))
            .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
        let format = field
            .format
            .as_ref()
            .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
        let contract = projector.0.contract().clone();
        if !contract.accepts(field.value_kind, format) {
            return Err(RuntimeGraphqlError::new("incompatible_projection"));
        }
        // Keys stay scalar so cursors, key inputs and relation batching are unchanged.
        let relation_key = schema.schema().collections.iter().any(|c| {
            c.relations.iter().any(|r| {
                r.key_pairs.iter().any(|pair| {
                    (c.id == collection.id && &pair.source == id)
                        || (r.target == collection.id && &pair.target == id)
                })
            })
        });
        if collection.primary_key.contains(id) || relation_key {
            return Err(RuntimeGraphqlError::new("unsupported_capability"));
        }
        let object = contract.object.clone();
        if objects
            .insert(object.name().to_owned(), object.clone())
            .is_some_and(|existing| existing != object)
        {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        compiled.insert(
            id.clone(),
            Arc::new(ProjectionBinding {
                projector: projector.0.clone(),
                contract,
            }),
        );
    }
    for collection in &schema.schema().collections {
        for field in &collection.fields {
            let Some(format) = &field.format else {
                continue;
            };
            let allowed = scalar_formats.iter().any(|(namespace, versions)| {
                namespace == &format.namespace && versions.contains(&format.version.get())
            });
            if !compiled.contains_key(&field.id) && !allowed {
                return Err(RuntimeGraphqlError::new("missing_projection"));
            }
        }
    }
    Ok(compiled)
}

/// Add a projected field to a layer, merging aliases into one selected-field union
/// in the contract's declared order.
pub(super) fn merge_projected(
    projected: &mut Vec<ProjectedPlan>,
    field: RuntimeFieldHandle,
    api_name: &str,
    binding: &Arc<ProjectionBinding>,
    names: Vec<String>,
) {
    let index = match projected.iter().position(|plan| plan.field == field) {
        Some(index) => index,
        None => {
            projected.push(ProjectedPlan {
                field,
                api_name: api_name.to_owned(),
                binding: binding.clone(),
                selected: Vec::new(),
            });
            projected.len() - 1
        }
    };
    let plan = &mut projected[index];
    let declared = binding.contract.object.fields();
    plan.selected = declared
        .iter()
        .map(RuntimeProjectedField::name)
        .filter(|name| {
            plan.selected.iter().any(|selected| selected == name)
                || names.iter().any(|selected| selected == name)
        })
        .map(str::to_owned)
        .collect();
}
