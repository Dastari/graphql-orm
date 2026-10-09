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
pub struct RuntimeGraphqlOptions {
    pub type_prefix: String,
    pub root_names: BTreeMap<CollectionId, String>,
    pub cursor_profile: RuntimeCursorProfile,
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
        let mut module = Self {
            schema,
            options: Arc::new(options),
            descriptor: Arc::new(RuntimeGraphqlDescriptor {
                names: BTreeSet::new(),
                cost: RuntimeGraphqlCost {
                    statements: 2,
                    materialized_nodes: 50,
                    crypto_calls: 51,
                    relation_groups: 0,
                },
            }),
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
    name: String,
    field: Field,
    arguments: BTreeSet<String>,
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
            arguments: BTreeSet::new(),
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
        if !valid_name(&name, false) || !self.arguments.insert(name.clone()) {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        let mut input = InputValue::new(name, ty);
        if let Some(default) = default {
            input = input.default_value(default);
        }
        self.field = self.field.argument(input);
        Ok(self)
    }
}
/// Checked dynamic composition. It never accepts an opaque raw builder callback.
pub struct RuntimeGraphqlComposer<B: RuntimeReadBackend> {
    database: Database<B>,
    query: Object,
    query_name: String,
    subscription_name: Option<String>,
    actions: Vec<Box<dyn FnOnce(SchemaBuilder) -> SchemaBuilder + Send>>,
    names: BTreeSet<String>,
    roots: BTreeSet<String>,
    modules: Vec<RuntimeGraphqlModule>,
    limits: RuntimeGraphqlLimits,
    protection: Option<super::execution::Protection>,
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
            query: Object::new(&query_name),
            query_name,
            subscription_name: None,
            actions: Vec::new(),
            names,
            roots: BTreeSet::new(),
            modules: Vec::new(),
            limits,
            protection: None,
        })
    }
    pub fn query_field(mut self, spec: RuntimeHostField) -> Result<Self, RuntimeGraphqlError> {
        if !valid_name(&spec.name, false) || !self.roots.insert(spec.name) {
            return Err(RuntimeGraphqlError::new("name_collision"));
        }
        self.query = self.query.field(spec.field);
        Ok(self)
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
    pub fn limit_depth(mut self, limit: usize) -> Self {
        self.limits.max_depth = self.limits.max_depth.min(limit);
        self
    }
    pub fn limit_complexity(mut self, limit: usize) -> Self {
        self.limits.max_selections = self.limits.max_selections.min(limit);
        self
    }

    pub fn finish(self) -> Result<Schema, RuntimeGraphqlError> {
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
                Schema::build(&self.query_name, None, self.subscription_name.as_deref()),
                |builder, action| action(builder),
            )
            .register(self.query)
            .limit_depth(self.limits.max_depth)
            .limit_complexity(self.limits.max_selections)
            .limit_recursive_depth(self.limits.max_depth)
            .extension(guard)
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
