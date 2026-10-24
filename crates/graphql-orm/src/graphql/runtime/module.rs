use super::*;
use crate::{db::Database, graphql::orm::*};
use async_graphql::{dynamic::*, extensions::ExtensionFactory};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Supported runtime read backend; unsupported decoder capabilities fail before I/O.
pub trait RuntimeReadBackend: OrmBackend + RuntimeRowDecoder {}
impl<B: OrmBackend + RuntimeRowDecoder> RuntimeReadBackend for B {}

/// Schema naming and cursor privacy options.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct RuntimeGraphqlOptions {
    pub type_prefix: String,
    pub root_names: BTreeMap<CollectionId, String>,
    pub cursor_profile: RuntimeCursorProfile,
    pub(super) mutations: Option<RuntimeGraphqlMutationOptions>,
    pub(super) projections: BTreeMap<FieldId, super::projection::ProjectorRef>,
    pub(super) scalar_formats: Vec<(RuntimeFormatNamespace, std::ops::RangeInclusive<u16>)>,
    pub(super) relation_filters: RelationFilterOptions,
}
/// Default bound on relation predicates in one GraphQL operation.
pub const RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTERS: usize = 16;
/// Default bound on relation predicate nesting (relation hops) in one `where`.
pub const RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTER_DEPTH: usize = 3;
#[derive(Clone, Debug, Default)]
pub(super) struct RelationFilterOptions {
    pub enabled: bool,
    /// `None` admits every relation; otherwise only the listed relations.
    pub relations: Option<BTreeSet<RelationId>>,
    /// `(max per operation, max nesting depth)`; defaults when `None`.
    pub limits: Option<(usize, usize)>,
}
impl RelationFilterOptions {
    pub fn admits(&self, relation: &RelationId) -> bool {
        self.enabled
            && self
                .relations
                .as_ref()
                .is_none_or(|relations| relations.contains(relation))
    }
    pub fn max_filters(&self) -> usize {
        self.limits
            .map_or(RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTERS, |l| l.0)
    }
    pub fn max_depth(&self) -> usize {
        self.limits
            .map_or(RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTER_DEPTH, |l| l.1)
    }
}
impl RuntimeGraphqlOptions {
    /// Accept relation predicates on every relation: `{is, isNot}` on to-one and
    /// `{some, none, every}` on to-many relations, through generated
    /// `…ToOneFilter`/`…ToManyFilter` inputs in each `…WhereInput`. Each predicate
    /// lowers to a correlated `EXISTS`. Its target layer is authorized with
    /// [`RuntimeReadPurpose::RelationFilter`](super::RuntimeReadPurpose) and the
    /// grant is applied inside the subquery. Off by default, which keeps the SDL
    /// unchanged. Mutation `expected` conditions never accept relation predicates.
    /// See [`Self::with_relation_filters_for`] and [`Self::with_relation_filter_limits`].
    pub fn with_relation_filters(mut self) -> Self {
        self.relation_filters.enabled = true;
        self.relation_filters.relations = None;
        self
    }

    /// Accept relation predicates only on the listed relations (stable IDs).
    /// Compilation rejects unknown relations.
    pub fn with_relation_filters_for(
        mut self,
        relations: impl IntoIterator<Item = RelationId>,
    ) -> Self {
        self.relation_filters.enabled = true;
        self.relation_filters.relations = Some(relations.into_iter().collect());
        self
    }

    /// Bound relation predicates per GraphQL operation (default
    /// [`RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTERS`]) and their nesting depth in
    /// relation hops (default [`RUNTIME_GRAPHQL_DEFAULT_MAX_RELATION_FILTER_DEPTH`]).
    /// Each predicate is one correlated subquery and one target-layer authorization.
    /// Exceeding either fails the operation with `cost_exceeded` before any I/O.
    pub fn with_relation_filter_limits(mut self, max_filters: usize, max_depth: usize) -> Self {
        self.relation_filters.limits = Some((max_filters, max_depth));
        self
    }

    /// Serve one formatted field through a reviewed projector, keyed by its stable
    /// field ID. Compilation rejects unknown, unformatted, key or incompatible fields.
    pub fn project_field(
        mut self,
        field: FieldId,
        projector: Arc<dyn super::RuntimeFieldProjector>,
    ) -> Self {
        self.projections
            .insert(field, super::projection::ProjectorRef(projector));
        self
    }

    /// Explicitly serve fields of a format namespace and version range as their
    /// plain stored scalar, with SDL identical to an unformatted field.
    pub fn scalar_format(
        mut self,
        namespace: RuntimeFormatNamespace,
        versions: std::ops::RangeInclusive<u16>,
    ) -> Self {
        self.scalar_formats.push((namespace, versions));
        self
    }

    /// Enable typed runtime mutations explicitly; reads remain the default.
    pub fn with_mutations(mut self, options: RuntimeGraphqlMutationOptions) -> Self {
        self.mutations = Some(options);
        self
    }

    /// Select cursor privacy explicitly. The default requires authenticated encryption.
    pub fn with_cursor_profile(mut self, profile: RuntimeCursorProfile) -> Self {
        self.cursor_profile = profile;
        self
    }
}
/// Hard schema and execution bounds. Hosts may lower these limits.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeGraphqlLimits {
    pub max_types: usize,
    pub max_name_bytes: usize,
    pub max_schema_bytes: usize,
    pub max_query_bytes: usize,
    pub max_variable_bytes: usize,
    pub max_variable_nodes: usize,
    pub max_depth: usize,
    pub max_selections: usize,
    pub max_statements: usize,
    pub max_materialized_nodes: usize,
    pub max_response_bytes: usize,
    pub max_crypto_calls: usize,
    pub query: RuntimeQueryLimits,
    pub relation: RuntimeRelationLimits,
    pub scalars: RuntimeScalarLimits,
    pub projection: super::RuntimeProjectionLimits,
}
impl Default for RuntimeGraphqlLimits {
    fn default() -> Self {
        Self {
            max_types: 4096,
            max_name_bytes: 128,
            max_schema_bytes: 4 * 1024 * 1024,
            max_query_bytes: 64 * 1024,
            max_variable_bytes: 256 * 1024,
            max_variable_nodes: 8192,
            max_depth: 16,
            max_selections: 1024,
            max_statements: 64,
            max_materialized_nodes: 10_000,
            max_response_bytes: 8 * 1024 * 1024,
            max_crypto_calls: 20_000,
            query: Default::default(),
            relation: Default::default(),
            scalars: Default::default(),
            projection: Default::default(),
        }
    }
}
impl RuntimeGraphqlLimits {
    pub(super) fn restricted_by(mut self, other: Self) -> Self {
        macro_rules! bound { ($($field:ident),*) => { $(self.$field = self.$field.min(other.$field);)* }; }
        bound!(
            max_types,
            max_name_bytes,
            max_schema_bytes,
            max_query_bytes,
            max_variable_bytes,
            max_variable_nodes,
            max_depth,
            max_selections,
            max_statements,
            max_materialized_nodes,
            max_response_bytes,
            max_crypto_calls
        );
        macro_rules! nested { ($part:ident; $($field:ident),*) => { $(self.$part.$field = self.$part.$field.min(other.$part.$field);)* }; }
        nested!(query; max_predicate_depth, max_predicate_nodes, max_values_per_list, max_bind_parameters, max_order_terms,
            max_projection_fields, default_page_size, max_page_size, max_cursor_bytes, max_cursor_values);
        nested!(relation; max_parents, max_key_arity, max_page_size, max_bind_parameters, max_cursor_bytes, max_compatible_groups);
        nested!(scalars; max_text_bytes, max_decoded_bytes, max_json_depth, max_json_nodes);
        self.projection = self.projection.restricted_by(other.projection);
        self
    }
}
/// Checked upper-bound admission estimate, not a bound on database scan rows.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeGraphqlCost {
    pub statements: usize,
    pub materialized_nodes: usize,
    pub crypto_calls: usize,
    pub relation_groups: usize,
    /// Host projector calls; one per projected field per batched layer.
    pub projection_calls: usize,
}
impl RuntimeGraphqlCost {
    /// Conservative root-page admission estimate, including one possible resume.
    pub fn for_root_page(
        size: usize,
        count: bool,
        protected: bool,
    ) -> Result<Self, RuntimeGraphqlError> {
        let mut cost = Self::for_relation_layer(1, size, count, protected)?;
        cost.relation_groups = 0;
        Ok(cost)
    }
    /// One bounded batched layer; this does not estimate database scanned rows.
    pub fn for_relation_layer(
        parents: usize,
        size: usize,
        count: bool,
        protected: bool,
    ) -> Result<Self, RuntimeGraphqlError> {
        let overflow = || RuntimeGraphqlError::new("cost_exceeded");
        let nodes = parents.checked_mul(size).ok_or_else(overflow)?;
        Ok(Self {
            statements: if parents == 0 {
                0
            } else {
                1 + usize::from(count)
            },
            materialized_nodes: nodes,
            crypto_calls: if protected {
                nodes.checked_add(parents).ok_or_else(overflow)?
            } else {
                0
            },
            relation_groups: usize::from(parents != 0),
            projection_calls: 0,
        })
    }
    /// Add independent roots/layers without wrapping admission counters.
    pub fn checked_add(self, other: Self) -> Result<Self, RuntimeGraphqlError> {
        let overflow = || RuntimeGraphqlError::new("cost_exceeded");
        Ok(Self {
            statements: self
                .statements
                .checked_add(other.statements)
                .ok_or_else(overflow)?,
            materialized_nodes: self
                .materialized_nodes
                .checked_add(other.materialized_nodes)
                .ok_or_else(overflow)?,
            crypto_calls: self
                .crypto_calls
                .checked_add(other.crypto_calls)
                .ok_or_else(overflow)?,
            relation_groups: self
                .relation_groups
                .checked_add(other.relation_groups)
                .ok_or_else(overflow)?,
            projection_calls: self
                .projection_calls
                .checked_add(other.projection_calls)
                .ok_or_else(overflow)?,
        })
    }
}
/// Immutable module metadata; contains no authority or catalog state.
#[derive(Clone, Debug)]
pub struct RuntimeGraphqlDescriptor {
    pub(super) names: BTreeSet<String>,
    cost: RuntimeGraphqlCost,
}
impl RuntimeGraphqlDescriptor {
    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }
    /// One default 50-row root connection including its optional count.
    /// Nested layers/aliases require additional admission estimates.
    pub fn cost_metadata(&self) -> RuntimeGraphqlCost {
        self.cost
    }
}
/// Safe compilation diagnostics. No physical names or caller values are rendered.
#[derive(Clone, Debug)]
pub struct RuntimeGraphqlDiagnostics(pub Vec<RuntimeGraphqlError>);
impl std::fmt::Display for RuntimeGraphqlDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("runtime_graphql_compilation_failed")
    }
}
impl std::error::Error for RuntimeGraphqlDiagnostics {}
impl From<RuntimeGraphqlError> for RuntimeGraphqlDiagnostics {
    fn from(e: RuntimeGraphqlError) -> Self {
        Self(vec![e])
    }
}

/// Compiled runtime module, ready for checked installation.
#[derive(Clone)]
pub struct RuntimeGraphqlModule {
    pub(super) schema: Arc<ValidatedRuntimeSchema>,
    pub(super) options: Arc<RuntimeGraphqlOptions>,
    descriptor: Arc<RuntimeGraphqlDescriptor>,
    pub(super) projections: Arc<BTreeMap<FieldId, Arc<super::projection::ProjectionBinding>>>,
}
pub(super) fn valid_name(name: &str, upper: bool) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with("__")
        && name.bytes().all(|b| b.is_ascii_alphanumeric())
        && name.as_bytes()[0].is_ascii_alphabetic()
        && if upper {
            name.as_bytes()[0].is_ascii_uppercase()
        } else {
            name.as_bytes()[0].is_ascii_lowercase()
        }
}
impl RuntimeGraphqlModule {
    pub fn compile(
        schema: Arc<ValidatedRuntimeSchema>,
        options: RuntimeGraphqlOptions,
    ) -> Result<Self, RuntimeGraphqlDiagnostics> {
        let cost = RuntimeGraphqlCost::for_root_page(
            50,
            true,
            options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption,
        )?;
        let projections =
            super::projection::compile(&schema, &options.projections, &options.scalar_formats)?;
        let mut module = Self {
            schema,
            options: Arc::new(options),
            descriptor: Arc::new(RuntimeGraphqlDescriptor {
                names: BTreeSet::new(),
                cost,
            }),
            projections: Arc::new(projections),
        };
        if module.schema.schema().collections.len() > 512 {
            return Err(RuntimeGraphqlError::new("cost_exceeded").into());
        }
        bounded_schema(
            &module.schema,
            RuntimeGraphqlLimits::default().max_schema_bytes,
        )?;
        let mut roots = BTreeSet::new();
        for c in &module.schema.schema().collections {
            if c.fields.len() > 1024 || c.relations.len() > 128 {
                return Err(RuntimeGraphqlError::new("cost_exceeded").into());
            }
            if c.fields
                .iter()
                .any(|f| f.filterable && matches!(f.api_name.as_str(), "and" | "or" | "not"))
                || c.relations.iter().any(|r| {
                    module.options.relation_filters.admits(&r.id)
                        && matches!(r.api_name.as_str(), "and" | "or" | "not")
                })
            {
                return Err(RuntimeGraphqlError::new("name_collision").into());
            }

            let handle = module.schema.resolve_collection(&c.id).map_err(|_| {
                RuntimeGraphqlDiagnostics::from(RuntimeGraphqlError::new("invalid_composition"))
            })?;
            module
                .schema
                .runtime_order(&handle, None, RuntimeQueryLimits::default())
                .map_err(|e| {
                    RuntimeGraphqlDiagnostics::from(RuntimeGraphqlError::new(e.code().as_str()))
                })?;
            if !valid_name(&module.object_name(c), true)
                || !valid_name(&module.root_name(c), false)
                || !roots.insert(module.root_name(c))
            {
                return Err(RuntimeGraphqlError::new("name_collision").into());
            }
            let mut fields = BTreeSet::new();
            for name in c
                .fields
                .iter()
                .map(|f| &f.api_name)
                .chain(c.relations.iter().map(|r| &r.api_name))
            {
                if !valid_name(name, false) || !fields.insert(name) {
                    return Err(RuntimeGraphqlError::new("invalid_composition").into());
                }
            }
            for suffix in [
                "",
                "WhereInput",
                "OrderInput",
                "OrderField",
                "Connection",
                "Edge",
            ] {
                let name = format!("{}{suffix}", module.object_name(c));
                if !valid_name(&name, true)
                    || !Arc::make_mut(&mut module.descriptor).names.insert(name)
                {
                    return Err(RuntimeGraphqlError::new("name_collision").into());
                }
            }
        }
        if module
            .options
            .relation_filters
            .relations
            .as_ref()
            .is_some_and(|relations| {
                relations.iter().any(|id| {
                    !module
                        .schema
                        .schema()
                        .collections
                        .iter()
                        .any(|c| c.relations.iter().any(|r| &r.id == id))
                })
            })
        {
            return Err(RuntimeGraphqlError::new("invalid_composition").into());
        }
        for name in super::schema::relation_filter_names(&module) {
            if !valid_name(&name, true) || !Arc::make_mut(&mut module.descriptor).names.insert(name)
            {
                return Err(RuntimeGraphqlError::new("name_collision").into());
            }
        }
        for c in &module.schema.schema().collections {
            for name in super::mutation::type_names(&module, c) {
                if !valid_name(&name, true)
                    || !Arc::make_mut(&mut module.descriptor).names.insert(name)
                {
                    return Err(RuntimeGraphqlError::new("name_collision").into());
                }
            }
        }
        // Projected objects are host-named and may be shared across modules, but
        // never shadow a generated type of this module.
        if module.projections.values().any(|binding| {
            module
                .descriptor
                .names
                .contains(binding.contract.object.name())
        }) {
            return Err(RuntimeGraphqlError::new("name_collision").into());
        }
        // Validate representable input objects before publication.
        let _ = super::mutation::types(&module)?;
        if module.options.root_names.keys().any(|id| {
            !module
                .schema
                .schema()
                .collections
                .iter()
                .any(|c| &c.id == id)
        }) {
            return Err(RuntimeGraphqlError::new("invalid_composition").into());
        }
        Ok(module)
    }
    pub fn descriptor(&self) -> &RuntimeGraphqlDescriptor {
        &self.descriptor
    }
    /// Relation predicate quantifiers this module accepts for a relation: none
    /// unless relation filters admit it, else those of its cardinality.
    pub fn relation_filter_quantifiers(
        &self,
        relation: &RelationId,
    ) -> &'static [RuntimeRelationQuantifier] {
        let cardinality = self
            .schema
            .schema()
            .collections
            .iter()
            .flat_map(|c| &c.relations)
            .find(|r| &r.id == relation)
            .map(|r| r.cardinality);
        match cardinality {
            Some(_) if !self.options.relation_filters.admits(relation) => &[],
            Some(RelationCardinality::One) => &[
                RuntimeRelationQuantifier::Is,
                RuntimeRelationQuantifier::IsNot,
            ],
            Some(RelationCardinality::Many) => &[
                RuntimeRelationQuantifier::Some,
                RuntimeRelationQuantifier::None,
                RuntimeRelationQuantifier::Every,
            ],
            None => &[],
        }
    }
    pub(super) fn projection(
        &self,
        field: &FieldId,
    ) -> Option<&Arc<super::projection::ProjectionBinding>> {
        self.projections.get(field)
    }
    pub(super) fn object_name(&self, c: &RuntimeCollection) -> String {
        format!("{}{}", self.options.type_prefix, c.api_type_name)
    }
    pub(super) fn root_name(&self, c: &RuntimeCollection) -> String {
        self.options
            .root_names
            .get(&c.id)
            .cloned()
            .unwrap_or_else(|| {
                let mut name = c.api_plural_name.clone();
                if let Some(first) = name.get_mut(..1) {
                    first.make_ascii_lowercase();
                }
                name
            })
    }
}

/// Root field whose actual name is checked by the composer.
pub struct RuntimeHostField {
    pub(super) name: String,
    field: Field,
    arguments: Vec<(String, InputValue)>,
    semantic: Option<GraphqlSemanticOperationDescriptor>,
}
impl RuntimeHostField {
    pub fn new<F>(name: impl Into<String>, ty: TypeRef, resolver: F) -> Self
    where
        F: for<'a> Fn(ResolverContext<'a>) -> FieldFuture<'a> + Send + Sync + 'static,
    {
        let name = name.into();
        Self {
            field: Field::new(&name, ty, resolver),
            name,
            arguments: Vec::new(),
            semantic: None,
        }
    }
    /// Add a checked host argument without exposing the root field's name.
    pub fn argument(
        mut self,
        name: impl Into<String>,
        ty: TypeRef,
        default: Option<async_graphql::Value>,
    ) -> Result<Self, RuntimeGraphqlError> {
        let name = name.into();
        if !valid_name(&name, false) || self.arguments.iter().any(|(known, _)| known == &name) {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        let mut input = InputValue::new(&name, ty);
        if let Some(default) = default {
            input = input.default_value(default);
        }
        self.arguments.push((name, input));
        Ok(self)
    }
    /// Describe this root for the semantic catalogue. The descriptor's
    /// description and argument descriptions become the SDL descriptions, so
    /// GraphQL and the catalogue carry one text. Its field name and argument
    /// names must match this field exactly; the operation kind is checked when
    /// the field is added to a root.
    pub fn semantic(
        mut self,
        descriptor: GraphqlSemanticOperationDescriptor,
    ) -> Result<Self, RuntimeGraphqlError> {
        let declared = descriptor
            .arguments
            .iter()
            .map(|argument| argument.graphql_name.as_str())
            .collect::<BTreeSet<_>>();
        let actual = self
            .arguments
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<BTreeSet<_>>();
        if descriptor.field_name != self.name
            || declared != actual
            || declared.len() != descriptor.arguments.len()
        {
            return Err(RuntimeGraphqlError::new("invalid_semantic_catalog"));
        }
        self.semantic = Some(descriptor);
        Ok(self)
    }
    /// The finished field and its descriptor, checked against a root kind.
    pub(super) fn into_parts(
        self,
        kind: GraphqlOperationKind,
    ) -> Result<(String, Field, Option<GraphqlSemanticOperationDescriptor>), RuntimeGraphqlError>
    {
        if self.semantic.as_ref().is_some_and(|d| d.kind != kind) {
            return Err(RuntimeGraphqlError::new("invalid_semantic_catalog"));
        }
        let mut field = self.field;
        if let Some(descriptor) = &self.semantic {
            field = field.description(&descriptor.description);
        }
        for (name, mut input) in self.arguments {
            if let Some(argument) = self.semantic.as_ref().and_then(|d| {
                d.arguments
                    .iter()
                    .find(|argument| argument.graphql_name == name)
            }) {
                input = input.description(&argument.description);
            }
            field = field.argument(input);
        }
        Ok((self.name, field, self.semantic))
    }
}
/// Semantic catalogue state collected during composition.
#[derive(Default)]
pub(super) struct SemanticParts {
    pub(super) entities: Vec<GraphqlEntitySemanticMetadata>,
    pub(super) operations: Vec<GraphqlSemanticOperationDescriptor>,
    pub(super) undescribed: bool,
    pub(super) subscription_described: bool,
}
impl SemanticParts {
    pub(super) fn root(&mut self, descriptor: Option<GraphqlSemanticOperationDescriptor>) {
        match descriptor {
            Some(descriptor) => self.operations.push(descriptor),
            None => self.undescribed = true,
        }
    }
}
/// Checked dynamic composition. It never accepts an opaque raw builder callback.
pub struct RuntimeGraphqlComposer<B: RuntimeReadBackend> {
    pub(super) database: Database<B>,
    pub(super) mutation: Option<Object>,
    pub(super) mutation_name: Option<String>,
    pub(super) mutation_roots: BTreeSet<String>,
    pub(super) mutation_fields: Option<super::mutation::FieldFactory>,
    query: Object,
    query_name: String,
    subscription_name: Option<String>,
    actions: Vec<Box<dyn FnOnce(SchemaBuilder) -> SchemaBuilder + Send>>,
    pub(super) names: BTreeSet<String>,
    roots: BTreeSet<String>,
    modules: Vec<RuntimeGraphqlModule>,
    limits: RuntimeGraphqlLimits,
    protection: Option<super::execution::Protection>,
    projected_types: BTreeMap<String, super::RuntimeProjectedObject>,
    pub(super) semantic: SemanticParts,
}
impl<B: RuntimeReadBackend> RuntimeGraphqlComposer<B> {
    pub fn new(
        database: Database<B>,
        query_name: impl Into<String>,
        limits: RuntimeGraphqlLimits,
    ) -> Result<Self, RuntimeGraphqlError> {
        let query_name = query_name.into();
        if !B::RUNTIME_ROW_DECODING_SUPPORTED {
            return Err(RuntimeGraphqlError::new("unsupported_backend"));
        }
        if !valid_name(&query_name, true) {
            return Err(RuntimeGraphqlError::new("invalid_composition"));
        }
        let mut names = ["String", "Int", "Float", "Boolean", "ID"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        if !names.insert(query_name.clone()) {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        Ok(Self {
            database,
            mutation: None,
            mutation_name: None,
            mutation_roots: BTreeSet::new(),
            mutation_fields: None,
            query: Object::new(&query_name),
            query_name,
            subscription_name: None,
            actions: Vec::new(),
            names,
            roots: BTreeSet::new(),
            modules: Vec::new(),
            limits,
            protection: None,
            projected_types: BTreeMap::new(),
            semantic: SemanticParts::default(),
        })
    }
    pub fn query_field(mut self, spec: RuntimeHostField) -> Result<Self, RuntimeGraphqlError> {
        let (name, field, semantic) = spec.into_parts(GraphqlOperationKind::Query)?;
        if !valid_name(&name, false) || !self.roots.insert(name) {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.query = self.query.field(field);
        self.semantic.root(semantic);
        Ok(self)
    }
    /// Add a host result object's semantic entity to the catalogue built by
    /// [`Self::finish_with_semantic_catalog`]. Register the GraphQL type itself
    /// with [`Self::register`].
    pub fn semantic_entity(mut self, entity: GraphqlEntitySemanticMetadata) -> Self {
        self.semantic.entities.push(entity);
        self
    }
    pub fn register(mut self, ty: Type) -> Result<Self, RuntimeGraphqlError> {
        let name = match &ty {
            Type::Scalar(t) => t.type_name(),
            Type::Object(t) => t.type_name(),
            Type::InputObject(t) => t.type_name(),
            Type::Enum(t) => t.type_name(),
            Type::Interface(t) => t.type_name(),
            Type::Union(t) => t.type_name(),
            Type::Subscription(t) => t.type_name(),
            Type::Upload => "Upload",
        };
        if name.len() > self.limits.max_name_bytes
            || name.starts_with("__")
            || !self.names.insert(name.to_owned())
            || self.names.len() > self.limits.max_types
        {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.actions
            .push(Box::new(move |builder| builder.register(ty)));
        Ok(self)
    }
    pub fn install(
        mut self,
        module: RuntimeGraphqlModule,
    ) -> Result<Self, RuntimeGraphqlDiagnostics> {
        if module.options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption
            && self.protection.is_none()
        {
            return Err(RuntimeGraphqlError::new("invalid_composition").into());
        }
        bounded_schema(&module.schema, self.limits.max_schema_bytes)?;
        if self.modules.is_empty() {
            for ty in super::schema::helpers(self.limits.scalars) {
                self = self.register(ty)?;
            }
        }
        let decimal = module
            .schema
            .schema()
            .collections
            .iter()
            .flat_map(|c| &c.fields)
            .any(|f| matches!(f.value_kind, RuntimeValueKind::Decimal(_)));
        if decimal && !self.names.contains("Decimal") {
            for ty in super::schema::decimal_helpers() {
                self = self.register(ty)?;
            }
        }
        if module.options.mutations.is_some() {
            let factory = self
                .mutation_fields
                .as_ref()
                .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?;
            let fields = factory(module.clone())?;
            for (name, field) in fields {
                if !self.mutation_roots.insert(name) {
                    return Err(RuntimeGraphqlError::new("name_collision").into());
                }
                self.mutation = Some(
                    self.mutation
                        .take()
                        .ok_or_else(|| RuntimeGraphqlError::new("invalid_composition"))?
                        .field(field),
                );
            }
            for ty in super::mutation::types(&module)? {
                self = self.register(ty)?;
            }
        }
        for binding in module.projections.values() {
            let object = &binding.contract.object;
            match self.projected_types.get(object.name()) {
                Some(existing) if existing == object => continue,
                Some(_) => return Err(RuntimeGraphqlError::new("name_collision").into()),
                None => {}
            }
            self = self.register(super::schema::projected_type(object))?;
            self.projected_types
                .insert(object.name().to_owned(), object.clone());
        }
        for ty in super::schema::types(&module) {
            self = self.register(ty)?;
        }
        for c in &module.schema.schema().collections {
            let root = module.root_name(c);
            if !self.roots.insert(root.clone()) {
                return Err(RuntimeGraphqlError::new("name_collision").into());
            }
            self.query = self
                .query
                .field(super::schema::root_field(&root, &module.object_name(c)));
        }
        self.semantic.entities.extend(module.semantic_entities());
        self.semantic
            .operations
            .extend(module.semantic_operations()?);
        self.modules.push(module);
        Ok(self)
    }
    pub fn cursor_protection(
        mut self,
        protector: Arc<dyn RuntimeCursorProtector>,
        audience: RuntimeCursorAudience,
        limits: RuntimeCursorProtectionLimits,
    ) -> Result<Self, RuntimeGraphqlError> {
        if limits.max_plaintext_bytes == 0
            || limits.max_key_id_bytes == 0
            || limits.max_token_bytes == 0
        {
            return Err(RuntimeGraphqlError::new("invalid_composition"));
        }
        self.protection = Some(super::execution::Protection {
            protector,
            audience,
            limits,
        });
        Ok(self)
    }
    pub fn data<D: Send + Sync + 'static>(mut self, data: D) -> Self {
        self.actions
            .push(Box::new(move |builder| builder.data(data)));
        self
    }
    pub fn extension(mut self, extension: impl ExtensionFactory + 'static) -> Self {
        self.actions
            .push(Box::new(move |builder| builder.extension(extension)));
        self
    }
    /// Custom directive factories are unsupported by the locked dynamic framework.
    /// Rejects during composition; the factory is never registered or invoked.
    pub fn directive(
        self,
        _directive: impl async_graphql::CustomDirectiveFactory,
    ) -> Result<Self, RuntimeGraphqlError> {
        Err(RuntimeGraphqlError::new("unsupported_capability"))
    }

    /// Bounds built-in directives per field. This cannot relax the installed budget.
    pub fn limit_directives(mut self, limit: usize) -> Self {
        let limit = limit.min(self.limits.max_selections);
        self.actions
            .push(Box::new(move |builder| builder.limit_directives(limit)));
        self
    }

    pub fn disable_introspection(mut self) -> Self {
        self.actions
            .push(Box::new(SchemaBuilder::disable_introspection));
        self
    }
    /// Host-owned subscriptions only; this adds no ORM event or transport service.
    pub fn subscription_root(mut self, root: Subscription) -> Result<Self, RuntimeGraphqlError> {
        if self.subscription_name.is_some() {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.subscription_name = Some(root.type_name().to_owned());
        self.register(root.into())
    }
    /// Describe one field of the host subscription root for the semantic
    /// catalogue. A composition with a subscription root needs at least one.
    pub fn subscription_semantic(
        mut self,
        descriptor: GraphqlSemanticOperationDescriptor,
    ) -> Result<Self, RuntimeGraphqlError> {
        if descriptor.kind != GraphqlOperationKind::Subscription {
            return Err(RuntimeGraphqlError::new("invalid_semantic_catalog"));
        }
        self.semantic.operations.push(descriptor);
        self.semantic.subscription_described = true;
        Ok(self)
    }
    pub fn limit_depth(mut self, limit: usize) -> Self {
        self.limits.max_depth = self.limits.max_depth.min(limit);
        self
    }
    pub fn limit_complexity(mut self, limit: usize) -> Self {
        self.limits.max_selections = self.limits.max_selections.min(limit);
        self
    }

    /// Finish the schema and its semantic catalogue.
    ///
    /// The catalogue holds every installed module's entities and roots plus
    /// the host's declared roots and result entities, so AI capability indexes
    /// cover runtime collections with no separate input. Fails with
    /// `semantic_descriptor_missing` when a host root lacks
    /// [`RuntimeHostField::semantic`] or a subscription root lacks
    /// [`Self::subscription_semantic`].
    pub fn finish_with_semantic_catalog(
        mut self,
    ) -> Result<(Schema, GraphqlSemanticCatalog), RuntimeGraphqlError> {
        let semantic = std::mem::take(&mut self.semantic);
        if semantic.undescribed
            || (self.subscription_name.is_some() && !semantic.subscription_described)
        {
            return Err(RuntimeGraphqlError::new("semantic_descriptor_missing"));
        }
        let catalog =
            GraphqlSemanticCatalog::compose_descriptors(semantic.entities, semantic.operations)
                .map_err(|_| RuntimeGraphqlError::new("invalid_semantic_catalog"))?;
        Ok((self.finish()?, catalog))
    }

    pub fn finish(mut self) -> Result<Schema, RuntimeGraphqlError> {
        if let Some(root) = self.mutation.take() {
            self.actions
                .push(Box::new(move |builder| builder.register(root)));
        }
        if self.protection.is_none()
            && self
                .modules
                .iter()
                .any(|m| m.options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption)
        {
            return Err(RuntimeGraphqlError::new("invalid_composition"));
        }
        let guard = super::execution::GuardFactory {
            database: self.database,
            modules: self.modules,
            limits: self.limits,
            protection: self.protection,
        };
        self.actions
            .into_iter()
            .fold(
                Schema::build(
                    &self.query_name,
                    self.mutation_name.as_deref(),
                    self.subscription_name.as_deref(),
                )
                .extension(guard)
                .limit_directives(self.limits.max_selections),
                |builder, action| action(builder),
            )
            .register(self.query)
            .limit_depth(self.limits.max_depth)
            .limit_complexity(self.limits.max_selections)
            .limit_recursive_depth(self.limits.max_depth)
            .finish()
            .map_err(|_| RuntimeGraphqlError::new("invalid_composition"))
    }
}

fn bounded_schema(
    schema: &ValidatedRuntimeSchema,
    maximum: usize,
) -> Result<(), RuntimeGraphqlError> {
    struct Counter {
        bytes: usize,
        maximum: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.maximum.saturating_sub(self.bytes) {
                return Err(std::io::Error::other("schema_limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter { bytes: 0, maximum }, schema.schema())
        .map_err(|_| RuntimeGraphqlError::new("cost_exceeded"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptors_are_shareable_and_cost_estimates_do_not_wrap() {
        fn shared<T: Send + Sync>() {}
        shared::<RuntimeGraphqlModule>();
        shared::<RuntimeGraphqlDescriptor>();
        shared::<RuntimeGraphqlOptions>();
        shared::<RuntimeGraphqlLimits>();
        let root = RuntimeGraphqlCost::for_root_page(20, true, true).unwrap();
        let child = RuntimeGraphqlCost::for_relation_layer(20, 5, false, true).unwrap();
        let combined = root.checked_add(child).unwrap();
        assert_eq!(combined.statements, 3);
        assert_eq!(combined.materialized_nodes, 120);
        assert_eq!(combined.crypto_calls, 141);
        assert_eq!(combined.relation_groups, 1);
        assert_eq!(
            RuntimeGraphqlCost::for_root_page(20, false, false)
                .unwrap()
                .crypto_calls,
            0
        );
        assert!(RuntimeGraphqlCost::for_relation_layer(usize::MAX, 2, false, true).is_err());
        assert!(
            root.checked_add(RuntimeGraphqlCost {
                statements: usize::MAX,
                ..Default::default()
            })
            .is_err()
        );
    }
}
