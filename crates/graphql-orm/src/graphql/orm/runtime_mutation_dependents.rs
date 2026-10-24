//! Opt-in Cascade/SetNull dependents of one runtime delete, performed by the ORM.
//!
//! The database's own modifying foreign-key actions are never relied on: every row
//! they would change is found and locked first, authorized through the same
//! [`RuntimeWriteAuthority`] path as a direct write, and then changed explicitly by
//! the ORM before the requested row is deleted. By then the database action has no
//! row left to change. A referencing row inserted concurrently is either blocked by
//! the locks (PostgreSQL row locks, SQLite's immediate write transaction) or makes
//! the final delete fail as a retryable serialization failure under the
//! state-machine transaction's isolation; it is never changed silently.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Largest accepted [`RuntimeDependentLimits::max_rows`].
pub const RUNTIME_DEPENDENT_MAX_ROWS: usize = 65_536;
/// Largest accepted [`RuntimeDependentLimits::max_depth`].
pub const RUNTIME_DEPENDENT_MAX_DEPTH: usize = 64;

/// Bounds for the dependent changes of one opted-in delete. Exceeding either
/// refuses with `limit_exceeded` before any host callback for a dependent and
/// before any write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeDependentLimits {
    max_rows: usize,
    max_depth: usize,
}
impl RuntimeDependentLimits {
    /// At most `max_rows` dependent rows deleted or nulled (excluding the requested
    /// row) and `max_depth` relation hops from the requested row.
    ///
    /// # Errors
    ///
    /// `limit_exceeded` above [`RUNTIME_DEPENDENT_MAX_ROWS`] or
    /// [`RUNTIME_DEPENDENT_MAX_DEPTH`].
    pub fn new(max_rows: usize, max_depth: usize) -> Result<Self, RuntimeMutationError> {
        if max_rows > RUNTIME_DEPENDENT_MAX_ROWS || max_depth > RUNTIME_DEPENDENT_MAX_DEPTH {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        Ok(Self {
            max_rows,
            max_depth,
        })
    }
    /// Maximum dependent rows deleted or nulled, excluding the requested row.
    pub const fn max_rows(self) -> usize {
        self.max_rows
    }
    /// Maximum relation hops from the requested row; its direct referencers are depth 1.
    pub const fn max_depth(self) -> usize {
        self.max_depth
    }
}
impl Default for RuntimeDependentLimits {
    /// 256 rows and depth 8.
    fn default() -> Self {
        Self {
            max_rows: 256,
            max_depth: 8,
        }
    }
}

/// Why the ORM writes a dependent row: a relation whose foreign key references a
/// row being deleted, with a `Cascade` (delete) or `SetNull` (update) policy.
#[derive(Clone)]
pub struct RuntimeDependentCause {
    relation: RuntimeRelationHandle,
    policy: DeletePolicy,
    referenced: RuntimeKey,
    depth: usize,
}
impl RuntimeDependentCause {
    /// Relation owned by the dependent row's collection; its target is the deleted row's.
    pub fn relation(&self) -> &RuntimeRelationHandle {
        &self.relation
    }
    /// `Cascade` deletes the dependent row; `SetNull` clears the relation's key fields.
    pub fn policy(&self) -> &DeletePolicy {
        &self.policy
    }
    /// Key of the deleted row this dependent row references.
    pub fn referenced(&self) -> &RuntimeKey {
        &self.referenced
    }
    /// Minimum relation hops from the requested row (1 for its direct referencers).
    /// When a row is reachable on several shortest paths, `relation` and
    /// `referenced` name the first in key order.
    pub fn depth(&self) -> usize {
        self.depth
    }
}
impl fmt::Debug for RuntimeDependentCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeDependentCause")
            .field("relation", self.relation.id())
            .field("policy", &self.policy)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

impl RuntimeMutationRequest {
    /// Opt in to Cascade/SetNull dependents for this delete.
    ///
    /// Without it, a delete whose table has any incoming `Cascade` or `SetNull`
    /// foreign key refuses with `unsupported_cascade`. With it, every dependent row
    /// is locked and passed to the same write authority (intent, preimage and
    /// result checks, with [`RuntimeWriteIntent::cause`] set) and listed in
    /// [`RuntimeMutationEffect::dependents`] for hooks. Any denial, cycle,
    /// `Restrict` reference to a row being deleted, or exceeded limit refuses the
    /// whole delete before any write. Incoming modifying foreign keys from tables
    /// outside the runtime schema still refuse with `unsupported_cascade`.
    pub fn with_dependent_changes(
        mut self,
        limits: RuntimeDependentLimits,
    ) -> Result<Self, RuntimeMutationError> {
        if self.action != RuntimeMutationAction::Delete || self.cause.is_some() {
            return Err(err(RuntimeMutationErrorCode::InvalidInput));
        }
        self.dependents = Some(limits);
        Ok(self)
    }
    /// Dependent-change bounds when the request opted in.
    pub fn dependent_limits(&self) -> Option<RuntimeDependentLimits> {
        self.dependents
    }
}

/// One row planned for deletion: its key and the values its referencers point at.
struct Node {
    collection: RuntimeCollectionHandle,
    key: Vec<(RuntimeFieldHandle, RuntimeValue)>,
    record: RuntimeRecord,
    cause: Option<RuntimeDependentCause>,
    depth: usize,
    identity: String,
}
type Prepared = Vec<(RuntimeMutationRequest, PreparedRuntimeWrite)>;

fn identity(
    collection: &RuntimeCollectionHandle,
    key: &[(RuntimeFieldHandle, RuntimeValue)],
) -> Result<String, RuntimeMutationError> {
    let values = key.iter().map(|(_, value)| value).collect::<Vec<_>>();
    let values = serde_json::to_string(&values)
        .map_err(|_| err(RuntimeMutationErrorCode::DatabaseFailed))?;
    Ok(format!("{}\u{0}{values}", collection.id()))
}

fn key_fields(
    schema: &ValidatedRuntimeSchema,
    handle: &RuntimeCollectionHandle,
) -> Result<Vec<RuntimeFieldHandle>, RuntimeMutationError> {
    collection(schema, handle)?
        .primary_key
        .iter()
        .map(|id| Ok(schema.resolve_field(handle, id)?))
        .collect()
}

/// Fields of `handle` that some incoming foreign key references, plus its key.
fn referenced_projection(
    schema: &ValidatedRuntimeSchema,
    environment: &RuntimeMutationEnvironment,
    handle: &RuntimeCollectionHandle,
) -> Result<RuntimeProjection, RuntimeMutationError> {
    let c = collection(schema, handle)?;
    let mut fields = key_fields(schema, handle)?;
    for dependency in environment.incoming_dependencies(&c.physical_table) {
        for pair in &dependency.column_pairs {
            let field = c
                .fields
                .iter()
                .find(|f| f.physical_column == pair.target_column)
                .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
            let field = schema.resolve_field(handle, &field.id)?;
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    Ok(schema.resolve_projection(handle, &fields)?)
}

/// Incoming dependencies in a backend-independent order.
fn sorted_dependencies<'a>(
    schema: &ValidatedRuntimeSchema,
    environment: &'a RuntimeMutationEnvironment,
    table: &str,
) -> Vec<&'a VerifiedRuntimeDependency> {
    // Links of the schema's own join tables are removed by each row's delete.
    let mut dependencies = environment
        .incoming_dependencies(table)
        .iter()
        .filter(|d| !super::links::is_link_dependency(schema, d))
        .collect::<Vec<_>>();
    dependencies.sort_by_key(|d| {
        (
            d.source_table.clone(),
            d.column_pairs
                .iter()
                .map(|p| (p.source_column.clone(), p.target_column.clone()))
                .collect::<Vec<_>>(),
        )
    });
    dependencies
}

/// The runtime relation that owns an incoming foreign key, or `unsupported_cascade`.
fn dependent_relation(
    schema: &ValidatedRuntimeSchema,
    dependency: &VerifiedRuntimeDependency,
    target: &RuntimeCollectionHandle,
) -> Result<RuntimeRelationHandle, RuntimeMutationError> {
    let unsupported = || err(RuntimeMutationErrorCode::UnsupportedCascade);
    let source = schema
        .schema()
        .collections
        .iter()
        .find(|c| c.physical_table == dependency.source_table)
        .ok_or_else(unsupported)?;
    let source_handle = schema.resolve_collection(&source.id)?;
    for relation in source.relations.iter().filter(|r| {
        r.enforce_foreign_key
            && r.cardinality == RelationCardinality::One
            && &r.target == target.id()
            && r.on_delete.as_ref() == Some(&dependency.on_delete)
    }) {
        let handle = schema.resolve_relation(&source_handle, &relation.id)?;
        if handle.key_pairs().len() == dependency.column_pairs.len()
            && dependency.column_pairs.iter().all(|pair| {
                handle.key_pairs().iter().any(|(s, t)| {
                    s.physical_column() == pair.source_column
                        && t.physical_column() == pair.target_column
                })
            })
        {
            return Ok(handle);
        }
    }
    Err(unsupported())
}

impl<B: RuntimeMutationBackend> MutationContext<'_, B> {
    async fn select_dependent_rows(
        &mut self,
        request: &RuntimeMutationRequest,
        projection: &RuntimeProjection,
        condition: &[(String, &RuntimeFieldHandle, &RuntimeValue)],
        limit: usize,
    ) -> Result<Vec<RuntimeRecord>, RuntimeMutationError> {
        let mut values = Vec::new();
        let condition = condition
            .iter()
            .map(|(column, field, value)| {
                format!(
                    "{} = {}",
                    B::DIALECT.quote_identifier(column),
                    bind_value(B::DIALECT, field, value, &mut values)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        if values.len() > request.limits.query.max_bind_parameters {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        // Key order makes discovery, limits and causes independent of storage order.
        let order = key_fields(&request.schema, projection.collection())?
            .iter()
            .map(|f| format!("{} ASC", B::DIALECT.quote_identifier(f.physical_column())))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {} FROM {} WHERE {} ORDER BY {} LIMIT {}{}",
            columns(B::DIALECT, projection),
            B::DIALECT.quote_identifier(projection.collection().physical_table()),
            condition,
            order,
            limit,
            if B::DIALECT == DatabaseBackend::Postgres {
                " FOR UPDATE"
            } else {
                ""
            }
        );
        let rows = self
            .fetch_rows(&sql, &values)
            .await
            .map_err(database_error::<B>)?;
        rows.iter()
            .map(|row| {
                let record = projection.decode_row::<B>(row)?;
                bounded_record(&record, projection, request.limits)?;
                Ok(record)
            })
            .collect()
    }

    /// Whether any row of `dependency`'s source table references `node`.
    async fn restrict_referenced(
        &mut self,
        request: &RuntimeMutationRequest,
        dependency: &VerifiedRuntimeDependency,
        node: &Node,
    ) -> Result<bool, RuntimeMutationError> {
        let target = collection(&request.schema, &node.collection)?;
        let mut values = Vec::new();
        let mut condition = Vec::new();
        for pair in &dependency.column_pairs {
            let field = target
                .fields
                .iter()
                .find(|f| f.physical_column == pair.target_column)
                .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
            let field = request.schema.resolve_field(&node.collection, &field.id)?;
            let value = node.record.value(&field)?;
            if matches!(value, RuntimeValue::Null) {
                return Ok(false);
            }
            condition.push(format!(
                "{} = {}",
                B::DIALECT.quote_identifier(&pair.source_column),
                bind_value(B::DIALECT, &field, value, &mut values)
            ));
        }
        let sql = format!(
            "SELECT 1 AS present FROM {} WHERE {} LIMIT 1",
            B::DIALECT.quote_identifier(&dependency.source_table),
            condition.join(" AND ")
        );
        let rows = self
            .fetch_rows(&sql, &values)
            .await
            .map_err(database_error::<B>)?;
        Ok(!rows.is_empty())
    }

    /// Lock and return the source rows of `relation` referencing `node`.
    async fn referencers(
        &mut self,
        request: &RuntimeMutationRequest,
        environment: &RuntimeMutationEnvironment,
        relation: &RuntimeRelationHandle,
        node: &Node,
        limit: usize,
    ) -> Result<Vec<(Vec<(RuntimeFieldHandle, RuntimeValue)>, RuntimeRecord)>, RuntimeMutationError>
    {
        let mut condition = Vec::with_capacity(relation.key_pairs().len());
        for (source, target) in relation.key_pairs() {
            let value = node.record.value(target)?;
            if matches!(value, RuntimeValue::Null) {
                return Ok(Vec::new());
            }
            condition.push((source.physical_column().to_owned(), target, value));
        }
        let projection = referenced_projection(&request.schema, environment, relation.source())?;
        let keys = key_fields(&request.schema, relation.source())?;
        let records = self
            .select_dependent_rows(request, &projection, &condition, limit)
            .await?;
        records
            .into_iter()
            .map(|record| {
                let key = keys
                    .iter()
                    .map(|f| Ok((f.clone(), record.value(f)?.clone())))
                    .collect::<Result<Vec<_>, RuntimeMutationError>>()?;
                Ok((key, record))
            })
            .collect()
    }

    async fn prepare_dependent(
        &mut self,
        environment: &RuntimeMutationEnvironment,
        request: &RuntimeMutationRequest,
        authority: &dyn RuntimeWriteAuthority<B>,
    ) -> Result<PreparedRuntimeWrite, RuntimeMutationError> {
        // The row exists and is locked, so a row hidden from the host's grant is a denial.
        self.prepare_runtime_write(environment, request, authority)
            .await
            .map_err(|error| {
                if error.code() == RuntimeMutationErrorCode::NotFound {
                    err(RuntimeMutationErrorCode::Denied).with_source(error)
                } else {
                    error
                }
            })
    }

    /// Plan, lock and authorize every dependent change of an opted-in delete whose
    /// requested row is already locked and authorized. Performs no writes.
    pub(super) async fn prepare_runtime_dependents(
        &mut self,
        environment: &RuntimeMutationEnvironment,
        request: &RuntimeMutationRequest,
        authority: &dyn RuntimeWriteAuthority<B>,
    ) -> Result<Prepared, RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        let limits = request.dependents.ok_or_else(|| err(E::InvalidInput))?;
        let schema = request.schema.clone();
        let key = request.key.as_ref().ok_or_else(|| err(E::InvalidInput))?;
        let projection = referenced_projection(&schema, environment, &request.collection)?;
        let condition = key
            .fields
            .iter()
            .map(|(f, v)| (f.physical_column().to_owned(), f, v))
            .collect::<Vec<_>>();
        let record = self
            .select_dependent_rows(request, &projection, &condition, 2)
            .await?;
        let [record] = <[RuntimeRecord; 1]>::try_from(record).map_err(|_| err(E::NotFound))?;
        let root_identity = identity(&request.collection, &key.fields)?;
        let mut nodes = vec![Node {
            collection: request.collection.clone(),
            key: key.fields.clone(),
            record,
            cause: None,
            depth: 0,
            identity: root_identity.clone(),
        }];
        let mut known = BTreeMap::from([(root_identity, 0usize)]);
        // (referencing row, referenced row); a row is deleted after all its referencers.
        let mut edges = BTreeSet::new();
        // Level order: each row is discovered at its minimum depth, deterministically.
        let mut frontier = vec![0usize];
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for index in frontier {
                let table = nodes[index].collection.physical_table().to_owned();
                for dependency in sorted_dependencies(&schema, environment, &table) {
                    match dependency.on_delete {
                        // Conservative: a Restrict reference to any row being deleted refuses.
                        DeletePolicy::Restrict => {
                            if self
                                .restrict_referenced(request, dependency, &nodes[index])
                                .await?
                            {
                                return Err(err(E::ConstraintViolation));
                            }
                        }
                        DeletePolicy::SetNull => {
                            dependent_relation(&schema, dependency, &nodes[index].collection)?;
                        }
                        DeletePolicy::Cascade => {
                            let relation =
                                dependent_relation(&schema, dependency, &nodes[index].collection)?;
                            let rows = self
                                .referencers(
                                    request,
                                    environment,
                                    &relation,
                                    &nodes[index],
                                    limits.max_rows + 1,
                                )
                                .await?;
                            if rows.len() > limits.max_rows {
                                return Err(err(E::LimitExceeded));
                            }
                            for (row_key, record) in rows {
                                let id = identity(relation.source(), &row_key)?;
                                let child = match known.get(&id) {
                                    Some(child) => *child,
                                    None => {
                                        let depth = nodes[index].depth + 1;
                                        if depth > limits.max_depth || nodes.len() > limits.max_rows
                                        {
                                            return Err(err(E::LimitExceeded));
                                        }
                                        let referenced = schema.runtime_key(
                                            &nodes[index].collection,
                                            &nodes[index].key,
                                        )?;
                                        nodes.push(Node {
                                            collection: relation.source().clone(),
                                            key: row_key,
                                            record,
                                            cause: Some(RuntimeDependentCause {
                                                relation: relation.clone(),
                                                policy: DeletePolicy::Cascade,
                                                referenced,
                                                depth,
                                            }),
                                            depth,
                                            identity: id.clone(),
                                        });
                                        known.insert(id, nodes.len() - 1);
                                        next.push(nodes.len() - 1);
                                        nodes.len() - 1
                                    }
                                };
                                edges.insert((child, index));
                            }
                        }
                    }
                }
            }
            frontier = next;
        }
        // Topological delete order, deepest then key order first. A row that can
        // never become ready lies on a data cycle (including a self-reference).
        let mut referencers = vec![0usize; nodes.len()];
        for (_, referenced) in &edges {
            referencers[*referenced] += 1;
        }
        let rank = |index: usize| {
            (
                std::cmp::Reverse(nodes[index].depth),
                nodes[index].identity.clone(),
                index,
            )
        };
        let mut ready = (0..nodes.len())
            .filter(|index| referencers[*index] == 0)
            .map(rank)
            .collect::<BTreeSet<_>>();
        let mut order = Vec::with_capacity(nodes.len());
        while let Some(entry) = ready.pop_first() {
            let index = entry.2;
            order.push(index);
            for (_, referenced) in edges.range((index, 0)..=(index, usize::MAX)) {
                referencers[*referenced] -= 1;
                if referencers[*referenced] == 0 {
                    ready.insert(rank(*referenced));
                }
            }
        }
        if order.len() != nodes.len() {
            return Err(err(E::UnsupportedCascade));
        }
        let mut count = nodes.len() - 1;
        // Rows referencing a deleted row through SetNull, unless deleted themselves.
        let mut nulls = Vec::new();
        let mut nulled = BTreeSet::new();
        for &index in &order {
            let depth = nodes[index].depth;
            let table = nodes[index].collection.physical_table().to_owned();
            for dependency in sorted_dependencies(&schema, environment, &table)
                .into_iter()
                .filter(|d| d.on_delete == DeletePolicy::SetNull)
            {
                let relation = dependent_relation(&schema, dependency, &nodes[index].collection)?;
                let rows = self
                    .referencers(
                        request,
                        environment,
                        &relation,
                        &nodes[index],
                        limits.max_rows + 1,
                    )
                    .await?;
                if rows.len() > limits.max_rows {
                    return Err(err(E::LimitExceeded));
                }
                for (row_key, _) in rows {
                    let id = identity(relation.source(), &row_key)?;
                    if known.contains_key(&id) || !nulled.insert((id, relation.id().clone())) {
                        continue;
                    }
                    count += 1;
                    if count > limits.max_rows || depth + 1 > limits.max_depth {
                        return Err(err(E::LimitExceeded));
                    }
                    let referenced =
                        schema.runtime_key(&nodes[index].collection, &nodes[index].key)?;
                    nulls.push((
                        row_key,
                        RuntimeDependentCause {
                            relation: relation.clone(),
                            policy: DeletePolicy::SetNull,
                            referenced,
                            depth: depth + 1,
                        },
                    ));
                }
            }
        }
        // Authorize every dependent change in execution order before any write.
        let mut prepared = Vec::with_capacity(nulls.len() + order.len());
        for (row_key, cause) in nulls {
            let fields = cause
                .relation
                .key_pairs()
                .iter()
                .map(|(source, _)| (source.clone(), RuntimeValue::Null))
                .collect::<Vec<_>>();
            let key = schema.runtime_key(cause.relation.source(), &row_key)?;
            let mut dependent =
                schema.runtime_update_request(key, &fields, None, None, request.limits)?;
            dependent.cause = Some(cause);
            let write = self
                .prepare_dependent(environment, &dependent, authority)
                .await?;
            prepared.push((dependent, write));
        }
        for index in order.into_iter().filter(|index| *index != 0) {
            // `order` ends with the requested row (index 0), which runs last.
            let node = &nodes[index];
            let key = schema.runtime_key(&node.collection, &node.key)?;
            let mut dependent = schema.runtime_delete_request(key, None, None, request.limits)?;
            dependent.cause = node.cause.clone();
            let write = self
                .prepare_dependent(environment, &dependent, authority)
                .await?;
            prepared.push((dependent, write));
        }
        Ok(prepared)
    }
}
