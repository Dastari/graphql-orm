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
#[non_exhaustive]
pub struct RuntimeReadCheck<'a> {
    pub schema: &'a ValidatedRuntimeSchema,
    pub collection: &'a RuntimeCollectionHandle,
    pub projection: &'a [RuntimeFieldHandle],
    pub filter: Option<&'a RuntimePredicate>,
    /// Requested field handles and operators retained before predicate constant lowering.
    /// Names are eq/ne/lt/lte/gt/gte/contains/startsWith/endsWith/in/notIn/isNull/isNotNull,
    /// and search for each searchable field of a request with a search argument.
    pub filter_operations: Vec<(&'a RuntimeFieldHandle, &'static str)>,
    /// Distinguish client ordering from trusted effective hidden-key ordering.
    pub requested_order: &'a [RuntimeOrderInput],
    pub order_was_supplied: bool,
    pub order: &'a RuntimeOrder,
    /// Explicit Boolean operators in the validated client input: and/or/not.
    pub logical_operators: Vec<&'static str>,
    pub relation: Option<&'a RuntimeRelationHandle>,
    pub include_count: bool,
    /// Formatted fields served through host projectors and their selected
    /// object fields. Each stored field also appears in `projection`.
    pub projections: Vec<super::RuntimeProjectedSelection<'a>>,
    /// Relation predicates used directly by this layer's filter, in input order.
    /// Each target is authorized as its own [`RuntimeReadPurpose::RelationFilter`]
    /// layer whose grant is applied inside the correlated subquery.
    pub relation_filters: Vec<(&'a RuntimeRelationHandle, RuntimeRelationQuantifier)>,
    /// Why this layer is read.
    pub purpose: RuntimeReadPurpose,
    /// Originating read purpose, retained on relation-filter checks. This is `Select`,
    /// `NodeLookup`, `SubscriptionRegistration` or `SubscriptionDelivery`, never `RelationFilter`.
    pub operation_purpose: RuntimeReadPurpose,
    /// The full-text search this layer carries, if any. Only root lists of
    /// searchable collections accept `search`; every other layer reports none.
    /// Each searchable field also appears in `filter_operations` as `"search"`.
    pub search: Option<RuntimeReadSearchCheck>,
}
/// The full-text search of a read layer, as presented to
/// [`RuntimeReadAuthority::authorize`].
///
/// A host can refuse search, or any mode it does not offer (for example `Web`,
/// which SQLite evaluates like `Plain`), by returning a denial; nothing is read.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RuntimeReadSearchCheck {
    /// The requested mode; [`SearchMode::Plain`](crate::graphql::filters::SearchMode::Plain)
    /// when the request names none.
    pub mode: crate::graphql::filters::SearchMode,
    /// Whether the request supplies `minScore`.
    pub min_score: bool,
    /// Whether results are ordered by relevance, because no `orderBy` was given.
    pub ranked: bool,
}
impl RuntimeReadSearchCheck {
    pub(super) fn of(search: &crate::graphql::orm::RuntimeSearchRequest) -> Self {
        Self {
            mode: search.mode(),
            min_score: search.min_score().is_some(),
            ranked: search.ranks(),
        }
    }
}
/// Why a layer is presented to [`RuntimeReadAuthority::authorize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeReadPurpose {
    /// Records of this layer are selected and returned.
    Select,
    /// A local `node(NodeId:)` point read.
    NodeLookup,
    /// Admission and initial visibility fence for a keyed subscription.
    SubscriptionRegistration,
    /// Current disclosure eligibility, including initial visibility and committed-event reloads.
    SubscriptionDelivery,
    /// The layer is the target of a relation predicate on `relation`. Nothing is
    /// returned; the grant's row predicate restricts which related rows can match,
    /// and an `authorization_denied` error makes every related row non-matching.
    /// `projection` is empty and `filter` is the client's target filter.
    RelationFilter(RuntimeRelationQuantifier),
}
/// Explicit host grant. No provider or error means denial.
pub struct RuntimeReadGrant {
    pub(super) predicate: Option<RuntimePredicate>,
}
impl RuntimeReadGrant {
    pub fn new(predicate: Option<RuntimePredicate>) -> Self {
        Self { predicate }
    }
}
/// Hosts authorize every field, operator, traversal and count in the check.
pub trait RuntimeReadAuthority: Send + Sync + 'static {
    /// Independently authorize a captured image for any removal, including an
    /// update that reloads as absent, and every dependency of
    /// `required_row`. Missing evidence must deny. Native RLS cannot grant here.
    fn authorize_deleted_image<'a>(
        &'a self,
        _check: super::RuntimeDeletedImageCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async { Err(RuntimeGraphqlError::denied()) })
    }

    /// Independently release a transaction-captured record after confirmed commit.
    /// Missing policy-required values must deny; this callback cannot roll back work.
    fn authorize_mutation_result<'a>(
        &'a self,
        _check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<(), RuntimeGraphqlError>> {
        Box::pin(async { Err(RuntimeGraphqlError::denied()) })
    }

    /// Release or withhold a committed mutation's captured record. The ORM calls this
    /// method, not [`Self::authorize_mutation_result`], after confirmed commit.
    ///
    /// The default releases exactly when `authorize_mutation_result` succeeds and returns
    /// its error otherwise, so existing hosts keep their behaviour. A host whose callers
    /// may write rows they may not read returns [`RuntimeMutationRelease::Withhold`]: the
    /// payload's `record` is `null`, without an error, and no record value, projected host
    /// object or relation is read or serialized. Errors keep their committed
    /// classification (`mutation_payload_denied`, `mutation_payload_failed`).
    fn release_mutation_result<'a>(
        &'a self,
        check: RuntimeMutationOutputCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeMutationRelease, RuntimeGraphqlError>> {
        let decision = self.authorize_mutation_result(check);
        Box::pin(async move { decision.await.map(|()| RuntimeMutationRelease::Release) })
    }

    fn authorize<'a>(
        &'a self,
        check: RuntimeReadCheck<'a>,
    ) -> BoxFuture<'a, Result<RuntimeReadGrant, RuntimeGraphqlError>>;
}
/// The host's decision for a committed mutation's record
/// ([`RuntimeReadAuthority::release_mutation_result`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeMutationRelease {
    /// Serialize the authorized selected projection.
    Release,
    /// The caller may not see this record: the payload's `record` is `null`. The mutation
    /// remains committed; this is not an error.
    Withhold,
}
/// Trusted request data. Reuse does not reuse execution caches or grants.
pub struct RuntimeGraphqlRequest<B: RuntimeReadBackend> {
    pub(super) fingerprint: SchemaFingerprint,
    pub(super) authority: Arc<dyn RuntimeReadAuthority>,
    pub(super) auth: Option<DbAuthContext>,
    pub(super) budget: RuntimeGraphqlLimits,
    scope: Option<String>,
    pub(super) mutation_environment: Option<Arc<RuntimeMutationEnvironment>>,
    pub(super) write_authority: Option<Arc<dyn std::any::Any + Send + Sync>>,
    pub(super) mutation_hook: Option<Arc<dyn std::any::Any + Send + Sync>>,
    pub(super) projection_context: Option<Arc<dyn std::any::Any + Send + Sync>>,
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
            mutation_environment: None,
            write_authority: None,
            mutation_hook: None,
            projection_context: None,
            backend: PhantomData,
        }
    }
    /// Trusted host data passed to field projectors for this request only.
    pub fn with_projection_context(
        mut self,
        context: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        self.projection_context = Some(context);
        self
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
/// Assurance refusals of the request pre-pass, by response key.
pub(super) type Denials = BTreeMap<String, async_graphql::Error>;
pub(super) struct Results(
    pub BTreeMap<String, Value>,
    pub BTreeMap<String, super::mutation::Prepared>,
    pub Denials,
);
impl Results {
    /// A root's resolver returns the pre-pass refusal for its response key, so
    /// the pre-pass decision binds even if a later evaluation would admit it.
    pub(super) fn admit(&self, key: &str) -> async_graphql::Result<()> {
        match self.2.get(key) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
    /// The request guard's prepared results for a query or mutation root.
    ///
    /// The guard prepares an operation in the `execute` extension hook, which
    /// async-graphql runs for [`Schema::execute`](async_graphql::dynamic::Schema::execute)
    /// only. A query or mutation sent through `execute_stream` reaches its
    /// resolvers unprepared, without the assurance pre-pass or any host read
    /// authorization; every runtime root then refuses with the stable
    /// `request_not_prepared` code and does no work.
    pub(super) fn prepared<'a>(
        ctx: &'a async_graphql::dynamic::ResolverContext<'_>,
    ) -> async_graphql::Result<&'a Self> {
        ctx.data_opt::<Self>()
            .ok_or_else(|| coded_error(REQUEST_NOT_PREPARED))
    }
}
/// Stable code of a runtime query or mutation root that the request guard did
/// not prepare, for example one sent through `Schema::execute_stream`.
pub(super) const REQUEST_NOT_PREPARED: &str = "request_not_prepared";
/// A client-visible error carrying `code` as its message and `code` extension.
pub(super) fn coded_error(code: &'static str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    async_graphql::Error::new(code).extend_with(|_, extensions| extensions.set("code", code))
}
/// Evaluate every executed root's assurance before request preparation reads
/// the database or calls any host authority. A refused root is not prepared.
fn assurance_denials(
    ctx: &ExtensionContext<'_>,
    planner: &Planner<'_>,
    selections: &async_graphql::parser::types::SelectionSet,
    kind: GraphqlOperationKind,
) -> Result<Denials, RuntimeGraphqlError> {
    use crate::graphql::assurance::{AssuranceCheck, AssuranceEnforcement, enforce_check};
    let mut denied = Denials::new();
    if ctx.data_opt::<AssuranceEnforcement>().is_none() {
        return Ok(denied);
    }
    for root in planner.root_occurrences(selections)? {
        if denied.contains_key(&root.key) {
            continue;
        }
        if let Err(error) = enforce_check(&AssuranceCheck::from_extension(ctx, kind, &root.name)) {
            denied.insert(root.key, error);
        }
    }
    Ok(denied)
}
/// Fail closed for PostgreSQL row-level security without request context.
///
/// Runtime reads, writes and live registrations apply the request's
/// [`DbAuthContext`] as the `app.*` settings RLS policies read. Without one, a
/// role subject to RLS sees no rows (silently filtered) and a role that bypasses
/// it, such as the table owner, sees every row. A runtime operation whose module
/// includes a table with row-level security enabled is therefore refused with
/// `authorization_misconfigured` before any host authority call or database
/// work, unless the request carries a `DbAuthContext` or the named trusted
/// [`SystemAccess`](crate::graphql::auth::SystemAccess), or the database runs
/// [`AuthorizationMode::LegacyPermissive`](crate::graphql::auth::AuthorizationMode::LegacyPermissive).
///
/// The check covers every table a module's operations can read or write: each
/// collection's table, its link tables and search documents, and, for writes,
/// the tables of verified incoming dependencies. It reads `pg_class` live, so
/// RLS enabled outside the ORM counts too. Other backends are unaffected.
pub(super) struct RowSecurity<'a, B: RuntimeReadBackend> {
    database: &'a Database<B>,
    exempt: bool,
    admitted: std::collections::BTreeSet<String>,
}
impl<'a, B: RuntimeReadBackend> RowSecurity<'a, B> {
    pub(super) fn new(
        database: &'a Database<B>,
        auth: Option<&DbAuthContext>,
        system_access: bool,
    ) -> Self {
        Self {
            database,
            exempt: B::DIALECT != DatabaseBackend::Postgres
                || auth.is_some()
                || system_access
                || database.authorization_mode()
                    == crate::graphql::auth::AuthorizationMode::LegacyPermissive,
            admitted: Default::default(),
        }
    }
    /// Admit one module's operation, or refuse it before any work.
    pub(super) async fn admit(
        &mut self,
        module: &RuntimeGraphqlModule,
        environment: Option<&RuntimeMutationEnvironment>,
    ) -> Result<(), RuntimeGraphqlError> {
        if self.exempt {
            return Ok(());
        }
        let mut tables = std::collections::BTreeSet::new();
        for c in &module.schema.schema().collections {
            tables.insert(c.physical_table.clone());
            if c.search.is_some() {
                tables.insert(search_table_name(&c.physical_table));
            }
            for relation in &c.relations {
                if let Some(join) = &relation.join {
                    tables.insert(join.physical_table.clone());
                }
            }
            if let Some(environment) = environment {
                for dependency in environment.incoming_dependencies(&c.physical_table) {
                    tables.insert(dependency.source_table.clone());
                }
            }
        }
        tables.retain(|table| !self.admitted.contains(table));
        let tables = tables.into_iter().collect::<Vec<_>>();
        for chunk in tables.chunks(256) {
            let candidates = (1..=chunk.len())
                .map(|index| {
                    format!(
                        "to_regclass(quote_ident({}))",
                        B::DIALECT.placeholder(index)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT 1 AS row_security FROM pg_catalog.pg_class WHERE relrowsecurity AND oid IN ({candidates}) LIMIT 1"
            );
            let binds = chunk
                .iter()
                .map(|table| SqlValue::String(table.clone()))
                .collect::<Vec<_>>();
            let rows = B::fetch_rows(self.database.pool(), &sql, &binds)
                .await
                .map_err(|_| RuntimeGraphqlError::new("backend_execution"))?;
            if !rows.is_empty() {
                return Err(RuntimeGraphqlError::new("authorization_misconfigured"));
            }
        }
        self.admitted.extend(tables);
        Ok(())
    }
}
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
            operation_name: Mutex::new(None),
        })
    }
}
struct Guard<B: RuntimeReadBackend> {
    database: Database<B>,
    modules: Vec<RuntimeGraphqlModule>,
    limits: RuntimeGraphqlLimits,
    protection: Option<Protection>,
    parsed: Mutex<Option<(ExecutableDocument, Variables)>>,
    operation_name: Mutex<Option<String>>,
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
pub(super) fn bounded_variables(
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
/// Coerce the selected operation's variables before async-graphql sees them:
/// every variable the request omits takes its operation default, and a nullable
/// one without a default takes `null`, as GraphQL variable coercion requires.
///
/// async-graphql 7.2.1 evaluates `@skip`/`@include` and validates arguments
/// against the raw request variables, but resolves arguments with the defaults
/// applied. With the defaults written into the request, directive evaluation,
/// validation, this guard's planning and execution all read one set of values.
fn apply_variable_defaults(
    request: &mut Request,
    max_nesting: usize,
    limits: RuntimeGraphqlLimits,
) -> Result<(), RuntimeGraphqlError> {
    // The parser recurses; bound it exactly as `parse_query` does. A query
    // that does not parse is refused there, unchanged.
    bounded_query_nesting(&request.query, max_nesting)?;
    let operation_name = request.operation_name.clone();
    let Ok(document) = request.parsed_query() else {
        return Ok(());
    };
    let Some((_, operation)) =
        document
            .operations
            .iter()
            .find(|(name, _)| match operation_name.as_deref() {
                Some(expected) => name.is_some_and(|name| name == expected),
                None => document.operations.iter().len() == 1,
            })
    else {
        return Ok(());
    };
    let definitions = operation
        .node
        .variable_definitions
        .iter()
        .filter_map(|definition| {
            definition
                .node
                .default_value()
                .map(|value| (definition.node.name.node.clone(), value.clone()))
        })
        .collect::<Vec<_>>();
    let mut changed = false;
    for (name, value) in definitions {
        if let std::collections::btree_map::Entry::Vacant(entry) = request.variables.entry(name) {
            entry.insert(value);
            changed = true;
        }
    }
    if changed {
        bounded_variables(&request.variables, limits)?;
    }
    Ok(())
}
/// Check every argument of the selected operation against its declared type,
/// as execution resolves it: each variable value once against the variable's
/// declared type, then each argument with its variable references standing for
/// those checked values. Fragments are checked once, under their type
/// condition. Introspection fields are left to async-graphql.
fn validate_arguments(
    registry: &async_graphql::registry::Registry,
    document: &ExecutableDocument,
    operation: &async_graphql::parser::types::OperationDefinition,
    variables: &Variables,
    limits: RuntimeGraphqlLimits,
) -> Result<(), RuntimeGraphqlError> {
    use async_graphql::parser::types::OperationType;
    let mut budget = limits
        .max_query_bytes
        .saturating_add(limits.max_variable_nodes)
        .saturating_mul(2);
    for definition in &operation.variable_definitions {
        let Some(value) = variables.get(&definition.node.name.node) else {
            continue;
        };
        if !super::scalar::input_value_valid(
            registry,
            &definition.node.var_type.node.to_string(),
            value,
            &mut budget,
        ) {
            return Err(invalid());
        }
    }
    let root = match operation.ty {
        OperationType::Query => Some(registry.query_type.as_str()),
        OperationType::Mutation => registry.mutation_type.as_deref(),
        OperationType::Subscription => registry.subscription_type.as_deref(),
    }
    .ok_or_else(invalid)?;
    let mut pending = Vec::new();
    validate_selection_arguments(
        registry,
        &operation.selection_set.node,
        root,
        variables,
        &mut budget,
        &mut pending,
    )?;
    let mut checked = std::collections::BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !checked.insert(name.clone()) {
            continue;
        }
        let fragment = document.fragments.get(&name).ok_or_else(invalid)?;
        validate_selection_arguments(
            registry,
            &fragment.node.selection_set.node,
            fragment.node.type_condition.node.on.node.as_str(),
            variables,
            &mut budget,
            &mut pending,
        )?;
    }
    Ok(())
}
fn validate_selection_arguments(
    registry: &async_graphql::registry::Registry,
    set: &async_graphql::parser::types::SelectionSet,
    ty: &str,
    variables: &Variables,
    budget: &mut usize,
    fragments: &mut Vec<Name>,
) -> Result<(), RuntimeGraphqlError> {
    use async_graphql::parser::types::Selection;
    for selection in &set.items {
        match &selection.node {
            Selection::Field(field) => {
                let name = field.node.name.node.as_str();
                if name.starts_with("__") {
                    continue;
                }
                let definition = registry
                    .types
                    .get(ty)
                    .and_then(|ty| ty.field_by_name(name))
                    .ok_or_else(invalid)?;
                for (argument, value) in &field.node.arguments {
                    let declared = definition
                        .args
                        .get(argument.node.as_str())
                        .ok_or_else(invalid)?;
                    let value = value.node.clone().into_const_with(|variable| {
                        Ok::<_, RuntimeGraphqlError>(match variables.get(&variable) {
                            None | Some(Value::Null) => Value::Null,
                            Some(_) => super::scalar::validated_variable(),
                        })
                    })?;
                    if !super::scalar::input_value_valid(registry, &declared.ty, &value, budget) {
                        return Err(invalid());
                    }
                }
                validate_selection_arguments(
                    registry,
                    &field.node.selection_set.node,
                    async_graphql::registry::MetaTypeName::concrete_typename(&definition.ty),
                    variables,
                    budget,
                    fragments,
                )?;
            }
            Selection::FragmentSpread(spread) => {
                fragments.push(spread.node.fragment_name.node.clone());
            }
            Selection::InlineFragment(fragment) => validate_selection_arguments(
                registry,
                &fragment.node.selection_set.node,
                fragment
                    .node
                    .type_condition
                    .as_ref()
                    .map_or(ty, |condition| condition.node.on.node.as_str()),
                variables,
                budget,
                fragments,
            )?,
        }
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
        let mut request = next.run(ctx, request).await?;
        apply_variable_defaults(&mut request, self.limits.max_depth, limits).map_err(server)?;
        *self.operation_name.lock().map_err(|_| server(invalid()))? =
            request.operation_name.clone();
        Ok(request)
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
        {
            let selected = self
                .operation_name
                .lock()
                .map_err(|_| vec![server(invalid())])?;
            let parsed = self.parsed.lock().map_err(|_| vec![server(invalid())])?;
            let (document, _) = parsed.as_ref().ok_or_else(|| vec![server(invalid())])?;
            if let Some((_, operation)) =
                document
                    .operations
                    .iter()
                    .find(|(name, _)| match selected.as_deref() {
                        Some(expected) => name.is_some_and(|name| name == expected),
                        None => document.operations.iter().len() == 1,
                    })
            {
                super::live::admit_operation(document, &operation.node, &self.modules, self.limits)
                    .map_err(|error| vec![server(error)])?;
            }
        }
        // Argument, variable and default values are validated in this step:
        // the runtime scalars apply their input bounds only while it runs.
        let result = super::scalar::validating_input(next.run(ctx))
            .await
            .map_err(|_| vec![server(invalid())])?;
        // async-graphql skips an argument whose value names a missing variable
        // and accepts any value for an input object; check every argument again
        // with the values execution resolves.
        let selected = self
            .operation_name
            .lock()
            .map_err(|_| vec![server(invalid())])?
            .clone();
        let parsed = self.parsed.lock().map_err(|_| vec![server(invalid())])?;
        let (document, variables) = parsed.as_ref().ok_or_else(|| vec![server(invalid())])?;
        let operation = document
            .operations
            .iter()
            .find(|(name, _)| match selected.as_deref() {
                Some(expected) => name.is_some_and(|name| name == expected),
                None => document.operations.iter().len() == 1,
            })
            .ok_or_else(|| vec![server(invalid())])?
            .1;
        validate_arguments(
            &ctx.schema_env.registry,
            document,
            &operation.node,
            variables,
            self.limits,
        )
        .map_err(|error| vec![server(error)])?;
        Ok(result)
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
        let (document, variables) = self
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
        // `prepare_request` wrote every operation default into the request, so
        // async-graphql evaluates `@skip`/`@include` and arguments with exactly
        // these variables. A defaulted variable still missing here means the
        // executed document is not the one prepared (another extension replaced
        // it): directive evaluation could then differ, so refuse.
        if operation
            .node
            .variable_definitions
            .iter()
            .any(|definition| {
                !variables.contains_key(&definition.node.name.node)
                    && definition.node.default_value().is_some()
            })
        {
            return Err(invalid());
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
            relation_filters: 0,
        };
        let kind = match operation.node.ty {
            async_graphql::parser::types::OperationType::Query => GraphqlOperationKind::Query,
            async_graphql::parser::types::OperationType::Mutation => GraphqlOperationKind::Mutation,
            async_graphql::parser::types::OperationType::Subscription => {
                GraphqlOperationKind::Subscription
            }
        };
        // Before any root is planned, read or authorized by the host.
        let denied = assurance_denials(ctx, &planner, &operation.node.selection_set.node, kind)?;
        if operation.node.ty == async_graphql::parser::types::OperationType::Mutation {
            let root = ctx
                .schema_env
                .registry
                .mutation_type
                .as_deref()
                .ok_or_else(invalid)?;
            return super::mutation::prepare(
                &self.modules,
                request,
                RowSecurity::new(
                    &self.database,
                    request.and_then(|request| request.auth.as_ref()),
                    ctx.data_opt::<crate::graphql::auth::SystemAccess>()
                        .is_some(),
                ),
                self.protection.as_ref(),
                &mut planner,
                &operation.node.selection_set.node,
                root,
                &ctx.schema_env.registry,
                denied,
            )
            .await;
        }
        let root_type = ctx.schema_env.registry.query_type.as_str();
        let selected = planner.selections(&operation.node.selection_set.node, root_type, 0)?;
        let mut plans = Vec::new();
        let mut output_shapes = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut values = BTreeMap::new();
        for field in selected {
            if denied.contains_key(field.response_key().node.as_str()) {
                continue;
            }
            if field.name.node == "node"
                && self.modules.iter().any(|m| {
                    m.schema
                        .schema()
                        .collections
                        .iter()
                        .any(|c| c.node_identity.is_some())
                })
            {
                let key = field.response_key().node.to_string();
                match super::live::prepare_node(
                    &self.modules,
                    &field,
                    &document,
                    &variables,
                    limits,
                )? {
                    Some((plan, identity, typename)) => {
                        planner.cost = planner.cost.checked_add(plan.cost)?;
                        let shape = OutputShape::compile(
                            &mut planner,
                            &ctx.schema_env.registry,
                            &field.selection_set.node,
                            &typename,
                            0,
                        )?;
                        output_shapes.insert(key, shape);
                        nodes.push((plan, identity, typename));
                    }
                    None => {
                        output_shapes.insert(key.clone(), OutputShape { fields: Vec::new() });
                        values.insert(key, Value::Null);
                    }
                }
                continue;
            }

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
        planner.cost.admit(limits)?;
        if plans.is_empty() && nodes.is_empty() {
            let mut bytes = 0;
            for (key, value) in &values {
                output_shapes[key].measure(value, &mut bytes, limits.max_response_bytes)?;
            }
            return Ok(Results(values, BTreeMap::new(), denied));
        }
        let request = request.ok_or_else(|| RuntimeGraphqlError::new("authorization_missing"))?;
        let mut row_security = RowSecurity::new(
            &self.database,
            request.auth.as_ref(),
            ctx.data_opt::<crate::graphql::auth::SystemAccess>()
                .is_some(),
        );
        for plan in plans.iter().chain(nodes.iter().map(|(plan, _, _)| plan)) {
            row_security.admit(&plan.module, None).await?;
        }
        // Complete all layers' grants and cursor framing checks before any ORM I/O.
        for plan in &mut plans {
            authorize(plan, request, self.protection.as_ref(), limits).await?;
        }
        for (plan, _, _) in &mut nodes {
            authorize(plan, request, self.protection.as_ref(), limits).await?;
        }
        let mut executor = Executor {
            database: &self.database,
            request,
            protection: self.protection.as_ref(),
            limits,
            response_bytes: 0,
            crypto_calls: 0,
            projection_calls: 0,
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
            let read = match &plan.search {
                None => read,
                Some(search) => {
                    // An index that is still being built would give silently
                    // incomplete results.
                    if !crate::graphql::orm::runtime_search_storage_ready::<B>(
                        self.database.pool(),
                        plan.collection.physical_table(),
                    )
                    .await
                    {
                        return Err(RuntimeGraphqlError::new("search_index_not_ready"));
                    }
                    read.with_search(search.clone()).map_err(query_error)?
                }
            };
            read.validate_graphql_backend(B::DIALECT)
                .map_err(query_error)?;
            prepared.push(read);
        }
        for (plan, key, typename) in nodes {
            let mut value = point(
                &self.database,
                request,
                &plan,
                &key,
                self.protection.as_ref(),
                limits,
            )
            .await?
            .unwrap_or(Value::Null);
            if let Value::Object(values) = &mut value {
                values.insert(Name::new(":node-type"), Value::String(typename));
            }
            values.insert(plan.response_key, value);
        }
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
        Ok(Results(values, BTreeMap::new(), denied))
    }
}
pub(super) fn authorize<'a, B: RuntimeReadBackend>(
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
        if !plan.point_read
            && plan.module.options.cursor_profile == RuntimeCursorProfile::AuthenticatedEncryption
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
                    .filter_intent
                    .operations
                    .iter()
                    .map(|(field, op)| (field, *op))
                    .collect(),
                requested_order: &plan.requested_order,
                order_was_supplied: plan.order_was_supplied,
                order: &plan.order,
                logical_operators: plan.filter_intent.logical.clone(),
                relation: plan.relation.as_ref(),
                include_count: plan.count,
                projections: plan
                    .projected
                    .iter()
                    .map(super::projection::ProjectedPlan::selection)
                    .collect(),
                relation_filters: plan
                    .filter_intent
                    .relations
                    .iter()
                    .map(|(relation, quantifier)| (relation, *quantifier))
                    .collect(),
                purpose: plan.purpose,
                operation_purpose: plan.purpose,
                search: plan.search.as_ref().map(RuntimeReadSearchCheck::of),
            })
            .await?;
        if !plan.relation_sites.is_empty() {
            relation_grants(plan, request, limits).await?;
        }
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
            child.purpose = plan.purpose;
            authorize(child, request, protection, limits).await?;
        }
        Ok(())
    })
}
/// Authorize every relation filter target layer, then rebuild the layer's filter
/// with their grants applied inside each subquery. A denied target matches nothing.
async fn relation_grants<B: RuntimeReadBackend>(
    plan: &mut Plan,
    request: &RuntimeGraphqlRequest<B>,
    limits: RuntimeGraphqlLimits,
) -> Result<(), RuntimeGraphqlError> {
    let mut grants = Vec::with_capacity(plan.relation_sites.len());
    for site in &plan.relation_sites {
        let decision = request
            .authority
            .authorize(RuntimeReadCheck {
                schema: &plan.module.schema,
                collection: &site.collection,
                projection: &[],
                filter: site.filter.as_ref(),
                filter_operations: site
                    .intent
                    .operations
                    .iter()
                    .map(|(field, op)| (field, *op))
                    .collect(),
                requested_order: &[],
                order_was_supplied: false,
                order: &site.order,
                logical_operators: site.intent.logical.clone(),
                relation: Some(&site.relation),
                include_count: false,
                projections: Vec::new(),
                relation_filters: site
                    .intent
                    .relations
                    .iter()
                    .map(|(relation, quantifier)| (relation, *quantifier))
                    .collect(),
                purpose: RuntimeReadPurpose::RelationFilter(site.quantifier),
                operation_purpose: plan.purpose,
                search: None,
            })
            .await;
        grants.push(match decision {
            Ok(grant) => match grant.predicate {
                Some(predicate)
                    if !predicate
                        .belongs_to(&plan.module.schema.fingerprint(), site.collection.id()) =>
                {
                    return Err(RuntimeGraphqlError::new("invalid_filter"));
                }
                predicate => predicate,
            },
            Err(error) if error == RuntimeGraphqlError::denied() => Some(
                plan.module
                    .schema
                    .runtime_or(&site.collection, Vec::new(), limits.query)
                    .map_err(query_error)?,
            ),
            Err(error) => return Err(error),
        });
    }
    let value = plan.where_value.as_ref().ok_or_else(invalid)?;
    let mut intent = super::plan::FilterIntent::default();
    let mut bound = super::plan::RelationGrants::Bound {
        grants: &grants,
        next: 0,
    };
    let predicate = super::plan::filter(
        &plan.module.schema,
        &plan.collection,
        value,
        limits,
        &mut intent,
        &mut bound,
    )?;
    if !matches!(bound, super::plan::RelationGrants::Bound { next, .. } if next == grants.len()) {
        return Err(invalid());
    }
    plan.predicate = Some(predicate);
    Ok(())
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
    projection_calls: usize,
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
        if plan.point_read
            || plan.module.options.cursor_profile == RuntimeCursorProfile::Unprotected
        {
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
        let mut parts = vec![
            fingerprint.as_str().as_bytes(),
            plan.collection.id().as_str().as_bytes(),
            &order,
            &parent,
        ];
        // A cursor taken under one caller filter must not resume under another.
        // Only the digest is authenticated; unfiltered requests keep the
        // associated data they always had.
        if let Some(filter) = &plan.filter_binding {
            parts.push(b"filter-v1");
            parts.push(filter);
        }
        RuntimeCursorContext::new(
            &protection.audience,
            self.request.scope.as_deref().ok_or_else(invalid)?,
            &parts,
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
            let mut edge_value = object([("cursor", Value::String(token)), ("node", node)]);
            // A searchable collection's edges carry their score: null without
            // a search argument.
            if plan
                .module
                .schema
                .schema()
                .collections
                .iter()
                .any(|c| c.id == *plan.collection.id() && c.search.is_some())
                && let Value::Object(map) = &mut edge_value
            {
                map.insert(
                    async_graphql::Name::new(super::schema::search_member("score")),
                    match edge.score {
                        Some(score) => Value::from(score.0),
                        None => Value::Null,
                    },
                );
            }
            edges.push(edge_value);
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
                // Projected stored scalars are never emitted; their objects are attached later.
                if plan
                    .projected
                    .iter()
                    .any(|projected| &projected.field == field)
                {
                    continue;
                }
                let value = match record.state(field).map_err(|_| invalid())? {
                    RuntimeFieldState::Value(v) => scalar::encode(v),
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
            super::live::insert_identity_fields(
                &plan.module,
                metadata,
                &plan.collection,
                record,
                plan.node_id,
                plan.record_version,
                &mut values,
            )?;
            nodes.push(Value::Object(values));
        }
        Ok(nodes)
    }
    /// Resolve projected objects for authorized records after their layer is read.
    async fn project(
        &mut self,
        plan: &Plan,
        records: &[RuntimeRecord],
        nodes: &mut [Value],
    ) -> Result<(), RuntimeGraphqlError> {
        for projected in &plan.projected {
            let stored = records
                .iter()
                .map(|record| match record.state(&projected.field) {
                    Ok(RuntimeFieldState::Value(value)) => Ok(value.clone()),
                    Ok(RuntimeFieldState::Null) => Ok(RuntimeValue::Null),
                    _ => Err(RuntimeGraphqlError::new("projection_mismatch")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if stored
                .iter()
                .all(|value| matches!(value, RuntimeValue::Null))
            {
                attach(nodes, &projected.api_name, vec![Value::Null; stored.len()])?;
                continue;
            }
            self.projection_calls = self.projection_calls.checked_add(1).ok_or_else(invalid)?;
            if self.projection_calls > self.limits.projection.max_calls {
                return Err(RuntimeGraphqlError::new("cost_exceeded"));
            }
            let (values, bytes) = projected
                .resolve(
                    &plan.module.schema,
                    &stored,
                    RuntimeProjectionPurpose::Read,
                    self.request.auth.as_ref(),
                    self.request.projection_context.as_deref(),
                    self.limits,
                )
                .await?;
            self.reserve(bytes)?;
            attach(nodes, &projected.api_name, values)?;
        }
        Ok(())
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
        self.project(plan, &records, &mut nodes).await?;
        for child in &plan.children {
            let relation = child.relation.as_ref().ok_or_else(invalid)?;
            let parents = connection
                .relation_parents(relation)
                .map_err(relation_error)?;
            let values = self.layer(child, parents).await?;
            attach(
                &mut nodes,
                &super::schema::relation_output_key(&child.field_name, &child.response_key),
                values,
            )?;
        }
        if plan.point_read {
            return Ok(object([(
                "edges",
                Value::List(
                    nodes
                        .into_iter()
                        .map(|node| object([("node", node)]))
                        .collect(),
                ),
            )]));
        }
        let connection = RuntimeConnection {
            edges: connection
                .edges
                .into_iter()
                .map(|e| RuntimeEdge {
                    node: e.node,
                    cursor: e.cursor,
                    score: e.score,
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
            self.project(plan, &records, &mut nodes).await?;
            for child in &plan.children {
                let parents = batch
                    .relation_parents(child.relation.as_ref().ok_or_else(invalid)?)
                    .map_err(relation_error)?;
                let values = self.layer(child, parents).await?;
                attach(
                    &mut nodes,
                    &super::schema::relation_output_key(&child.field_name, &child.response_key),
                    values,
                )?;
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
/// Per-request crypto and projection call counts of mutation payload relation reads.
#[derive(Default)]
pub(super) struct PayloadBudget {
    crypto_calls: std::sync::atomic::AtomicUsize,
    projection_calls: std::sync::atomic::AtomicUsize,
}

/// Read authorized to-one relation layers of one committed mutation record.
#[allow(clippy::too_many_arguments)]
pub(super) async fn payload_relations<B: RuntimeReadBackend>(
    database: &Database<B>,
    request: &RuntimeGraphqlRequest<B>,
    protection: Option<&Protection>,
    limits: RuntimeGraphqlLimits,
    budget: &PayloadBudget,
    schema: &ValidatedRuntimeSchema,
    record: &RuntimeRecord,
    children: &[Plan],
) -> Result<Vec<(String, Value)>, RuntimeGraphqlError> {
    use std::sync::atomic::Ordering;
    // Mutation fields of one request run serially; carry their counts forward.
    let mut executor = Executor {
        database,
        request,
        protection,
        limits,
        response_bytes: 0,
        crypto_calls: budget.crypto_calls.load(Ordering::SeqCst),
        projection_calls: budget.projection_calls.load(Ordering::SeqCst),
    };
    let result = payload_layers(&mut executor, schema, record, children).await;
    budget
        .crypto_calls
        .store(executor.crypto_calls, Ordering::SeqCst);
    budget
        .projection_calls
        .store(executor.projection_calls, Ordering::SeqCst);
    result
}
async fn payload_layers<B: RuntimeReadBackend>(
    executor: &mut Executor<'_, B>,
    schema: &ValidatedRuntimeSchema,
    record: &RuntimeRecord,
    children: &[Plan],
) -> Result<Vec<(String, Value)>, RuntimeGraphqlError> {
    let mut output = Vec::with_capacity(children.len());
    for child in children {
        let relation = child.relation.as_ref().ok_or_else(invalid)?;
        let parents = schema
            .runtime_relation_parents(relation, std::slice::from_ref(record))
            .map_err(relation_error)?;
        let mut values = executor.layer(child, parents).await?;
        let value = values.pop().ok_or_else(invalid)?;
        output.push((
            super::schema::relation_output_key(&child.field_name, &child.response_key),
            value,
        ));
    }
    Ok(output)
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

#[derive(Clone)]
pub(super) struct OutputShape {
    fields: Vec<(String, String, OutputShape)>,
}
impl OutputShape {
    pub(super) fn compile(
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
    pub(super) fn measure(
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
                        .get(super::schema::relation_output_key(name, key).as_str())
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

#[cfg(test)]
mod alias_tests {
    use super::*;
    #[test]
    fn response_budget_uses_schema_identity_and_separate_relation_slots() {
        let scalar = || OutputShape { fields: vec![] };
        let shape = OutputShape {
            fields: vec![
                ("caption".into(), "label".into(), scalar()),
                ("label".into(), "caption".into(), scalar()),
                ("link".into(), "children".into(), scalar()),
            ],
        };
        let value = object([
            ("label", Value::String("long scalar".repeat(10))),
            ("caption", Value::String("c".into())),
            ("children:link", Value::String("relation".into())),
        ]);
        let mut bytes = 0;
        shape.measure(&value, &mut bytes, usize::MAX).unwrap();
        let mut exact = 0;
        shape.measure(&value, &mut exact, bytes).unwrap();
        assert!(shape.measure(&value, &mut 0, bytes - 1).is_err());
        // Repeating a long scalar under another schema field's alias still charges
        // for the long scalar, never the shorter value stored under that name.
        let repeated = OutputShape {
            fields: vec![
                ("caption".into(), "label".into(), scalar()),
                ("other".into(), "label".into(), scalar()),
            ],
        };
        assert!(repeated.measure(&value, &mut 0, bytes).is_err());
    }
}

/// One authorized complete-key read through the existing anchored executor.
pub(super) async fn point<B: RuntimeReadBackend>(
    database: &Database<B>,
    request: &RuntimeGraphqlRequest<B>,
    plan: &Plan,
    key: &RuntimeKey,
    protection: Option<&Protection>,
    limits: RuntimeGraphqlLimits,
) -> Result<Option<Value>, RuntimeGraphqlError> {
    let mut plan = plan.clone();
    let identity = plan
        .module
        .schema
        .runtime_identity_predicate(key)
        .map_err(query_error)?;
    plan.predicate = Some(match plan.predicate.take() {
        Some(predicate) => plan
            .module
            .schema
            .runtime_and(&plan.collection, vec![predicate, identity], limits.query)
            .map_err(query_error)?,
        None => identity,
    });
    let read = plan
        .module
        .schema
        .runtime_read_request_with_relation_keys(
            &plan.collection,
            &plan.projection,
            plan.predicate.clone(),
            plan.order.clone(),
            RuntimePageRequest::first(1, None),
            false,
            &plan.relations(),
            limits.query,
        )
        .map_err(relation_error)?;
    let mut executor = Executor {
        database,
        request,
        protection,
        limits,
        response_bytes: 0,
        crypto_calls: 0,
        projection_calls: 0,
    };
    let Value::Object(connection) = executor.root(&plan, read).await? else {
        return Err(invalid());
    };
    let Some(Value::List(edges)) = connection.get("edges") else {
        return Err(invalid());
    };
    Ok(edges.first().and_then(|edge| {
        if let Value::Object(edge) = edge {
            edge.get("node").cloned()
        } else {
            None
        }
    }))
}
