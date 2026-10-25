//! Link-set writes of many-to-many relations and link cleanup on deletes.
//!
//! Links are rows of a relation's ORM-owned join table. They are never written
//! implicitly: a link mutation and every delete that removes links compute the
//! exact delta under row locks, present it to
//! [`RuntimeWriteAuthority::authorize_links`] before any write, perform it
//! explicitly and list it in [`RuntimeMutationEffect::links`] for hooks.
//!
//! Link mutations only see links to targets the host's
//! [`RuntimeWriteAuthority::link_target_visibility`] admits. Every requested
//! target must be an existing visible row: the ORM refuses any other with a
//! uniform `denied` before the host is asked or anything is written, so a
//! hidden target and a missing one are indistinguishable. The host then receives
//! every requested target, whatever its current link state, so a refusal never
//! depends on whether a hidden link exists.
use super::*;
use std::collections::BTreeSet;

/// Default bound on the target keys of one link mutation, and on the visible
/// links it may read.
pub const RUNTIME_DEFAULT_MAX_LINK_KEYS: usize = 1_000;
/// Default bound on the links one delete may remove per join relation side.
pub const RUNTIME_DEFAULT_MAX_DELETE_LINKS: usize = 10_000;

/// One link: a source row and a target row of a many-to-many relation.
#[derive(Clone)]
pub struct RuntimeLink {
    source: RuntimeKey,
    target: RuntimeKey,
}
impl RuntimeLink {
    /// Key of the relation's source row.
    pub fn source(&self) -> &RuntimeKey {
        &self.source
    }
    /// Key of the relation's target row.
    pub fn target(&self) -> &RuntimeKey {
        &self.target
    }
}
redacted_debug!(RuntimeLink);

/// Requested change to one source row's links.
#[derive(Clone)]
pub enum RuntimeLinkChange {
    /// Link exactly these targets: missing links are added, others removed.
    Replace(Vec<RuntimeKey>),
    /// Remove then add links. Adding an existing link or removing an absent one
    /// changes nothing; a target may not appear twice across both lists.
    Delta {
        add: Vec<RuntimeKey>,
        remove: Vec<RuntimeKey>,
    },
}
redacted_debug!(RuntimeLinkChange);

#[derive(Clone)]
pub(super) struct LinkSpec {
    relation: RuntimeRelationHandle,
    change: RuntimeLinkChange,
    max_link_keys: usize,
}

/// One link mutation's request and exact effective delta, or a delete's links.
///
/// Authorize the `requested_*` targets to decide uniformly: they list every
/// target the client named, independent of current link state, so the outcome
/// cannot reveal whether a hidden link exists. `added`/`removed` are the
/// effective delta computed under lock over visible links only.
#[non_exhaustive]
pub struct RuntimeLinkCheck<'a> {
    /// The link mutation, or the delete removing these links.
    pub intent: RuntimeWriteIntent<'a>,
    /// The join relation, always described from its declaring (source) side,
    /// including for a delete of a target row.
    pub relation: &'a RuntimeRelationHandle,
    /// Every `set` (Replace) or `add` (Delta) target named by a link mutation.
    /// Empty for a delete.
    pub requested_add: &'a [RuntimeKey],
    /// Every `remove` target named by a link mutation. Empty for Replace and a delete.
    pub requested_remove: &'a [RuntimeKey],
    /// Links that will be inserted.
    pub added: &'a [RuntimeLink],
    /// Links that will be removed. For a delete, every link of the deleted row,
    /// visible or not.
    pub removed: &'a [RuntimeLink],
}
redacted_debug!(RuntimeLinkCheck<'_>);

/// The join relation whose visible targets a link mutation asks for.
#[non_exhaustive]
pub struct RuntimeLinkVisibilityCheck<'a> {
    pub intent: RuntimeWriteIntent<'a>,
    /// Declaring side of the join relation; the predicate is on its target collection.
    pub relation: &'a RuntimeRelationHandle,
}
redacted_debug!(RuntimeLinkVisibilityCheck<'_>);

/// Applied link delta of one relation, pending until commit.
#[derive(Clone)]
pub struct RuntimeLinkEffect {
    relation: RuntimeRelationHandle,
    added: Vec<RuntimeLink>,
    removed: Vec<RuntimeLink>,
}
impl RuntimeLinkEffect {
    pub fn relation(&self) -> &RuntimeRelationHandle {
        &self.relation
    }
    pub fn added(&self) -> &[RuntimeLink] {
        &self.added
    }
    pub fn removed(&self) -> &[RuntimeLink] {
        &self.removed
    }
}
redacted_debug!(RuntimeLinkEffect);

fn key_identity(key: &RuntimeKey) -> Result<String, RuntimeMutationError> {
    serde_json::to_string(&key.fields.iter().map(|(_, v)| v).collect::<Vec<_>>())
        .map_err(|_| err(RuntimeMutationErrorCode::InvalidInput))
}

impl ValidatedRuntimeSchema {
    /// Change the links of `key`'s row through the many-to-many `relation`.
    ///
    /// The request is an `Update` of the source row that writes no fields: the
    /// write authority sees its intent and locked preimage as usual, then
    /// [`RuntimeWriteAuthority::link_target_visibility`] scopes the request to
    /// visible targets and [`RuntimeWriteAuthority::authorize_links`] receives the
    /// requested targets and the exact delta. Every requested target (`set`,
    /// `add` or `remove`) must be an existing row the visibility predicate admits;
    /// otherwise the mutation fails with `denied` before `authorize_links` and
    /// before any write, whether the target is hidden, missing or already linked.
    /// Links to hidden targets are never read, counted, replaced or removed.
    /// The returned record is the unchanged
    /// source row. At most `max_link_keys` target keys may be supplied and at most
    /// `max_link_keys` visible links may exist after the change.
    ///
    /// # Errors
    ///
    /// Rejects non-join or stale relations, keys of other collections,
    /// duplicate targets and bounds before any I/O.
    pub fn runtime_link_request(
        &self,
        relation: &RuntimeRelationHandle,
        key: RuntimeKey,
        change: RuntimeLinkChange,
        returning: Option<RuntimeProjection>,
        max_link_keys: usize,
        limits: RuntimeMutationLimits,
    ) -> Result<RuntimeMutationRequest, RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        self.check_link_change(relation, &change, max_link_keys)?;
        if key.schema.fingerprint() != self.fingerprint() || key.collection != *relation.source() {
            return Err(err(E::SchemaMismatch));
        }
        key.validate()?;
        let c = collection(self, relation.source())?;
        if c.append_only {
            return Err(err(E::AppendOnly));
        }
        if let Some(p) = &returning {
            self.resolve_projection(relation.source(), p.fields())?;
            if p.fields().len() > limits.query.max_projection_fields {
                return Err(err(E::LimitExceeded));
            }
        }
        Ok(RuntimeMutationRequest {
            schema: Arc::new(self.clone()),
            collection: relation.source().clone(),
            action: RuntimeMutationAction::Update,
            fields: Vec::new(),
            key: Some(key),
            expected: None,
            returning,
            limits,
            links: Some(LinkSpec {
                relation: relation.clone(),
                change,
                max_link_keys,
            }),
            attached: Vec::new(),
            max_delete_links: RUNTIME_DEFAULT_MAX_DELETE_LINKS,
            dependents: None,
            cause: None,
        })
    }
}

impl ValidatedRuntimeSchema {
    /// A current join relation and a bounded change of distinct targets of its target collection.
    fn check_link_change(
        &self,
        relation: &RuntimeRelationHandle,
        change: &RuntimeLinkChange,
        max_link_keys: usize,
    ) -> Result<(), RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        if relation.join().is_none()
            || self
                .resolve_relation(relation.source(), relation.id())
                .map_err(|e| err(E::SchemaMismatch).with_source(e))?
                != *relation
        {
            return Err(err(E::InvalidInput));
        }
        let targets: Vec<&RuntimeKey> = match change {
            RuntimeLinkChange::Replace(keys) => keys.iter().collect(),
            RuntimeLinkChange::Delta { add, remove } => add.iter().chain(remove).collect(),
        };
        if max_link_keys == 0 || targets.len() > max_link_keys {
            return Err(err(E::LimitExceeded));
        }
        let mut seen = BTreeSet::new();
        for target in targets {
            if target.schema.fingerprint() != self.fingerprint()
                || target.collection != *relation.target()
            {
                return Err(err(E::SchemaMismatch));
            }
            target.validate()?;
            if !seen.insert(key_identity(target)?) {
                return Err(err(E::InvalidInput));
            }
        }
        Ok(())
    }
}

impl RuntimeMutationRequest {
    /// Also change the written row's links through the many-to-many `relation`, in the same
    /// transaction as a create or update.
    ///
    /// After the row write and its result check, the change runs exactly as
    /// [`ValidatedRuntimeSchema::runtime_link_request`] on the written row's key: the host's
    /// [`RuntimeWriteAuthority::link_target_visibility`] scopes it, every requested target
    /// must be existing and visible or the whole write fails with `denied`, and
    /// [`RuntimeWriteAuthority::authorize_links`] sees an `Update` link intent of the row. Any
    /// refusal rolls back the row write too. A create's links bind to its generated key.
    /// [`RuntimeMutationEffect::links`] lists every change. At most one change per relation.
    ///
    /// # Errors
    ///
    /// `invalid_input` for a delete, a link request, a dependent change, a relation of another
    /// collection or a second change of one relation; plus the bounds and target checks of
    /// `runtime_link_request`, before any I/O.
    pub fn with_link_change(
        mut self,
        relation: &RuntimeRelationHandle,
        change: RuntimeLinkChange,
        max_link_keys: usize,
    ) -> Result<Self, RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        if !matches!(
            self.action,
            RuntimeMutationAction::Create | RuntimeMutationAction::Update
        ) || self.links.is_some()
            || self.cause.is_some()
            || relation.source() != &self.collection
            || self
                .attached
                .iter()
                .any(|spec| spec.relation.id() == relation.id())
        {
            return Err(err(E::InvalidInput));
        }
        self.schema
            .check_link_change(relation, &change, max_link_keys)?;
        if collection(&self.schema, &self.collection)?.append_only {
            return Err(err(E::AppendOnly));
        }
        self.attached.push(LinkSpec {
            relation: relation.clone(),
            change,
            max_link_keys,
        });
        Ok(self)
    }
    /// The link changes attached to a create or update, in the order they apply.
    pub fn attached_link_changes(
        &self,
    ) -> impl Iterator<Item = (&RuntimeRelationHandle, &RuntimeLinkChange)> {
        self.attached.iter().map(|l| (&l.relation, &l.change))
    }
    /// The link mutation an attached change runs as, on the written row.
    pub(super) fn attached_link_request(
        &self,
        spec: &LinkSpec,
        key: &[(RuntimeFieldHandle, RuntimeValue)],
    ) -> RuntimeMutationRequest {
        RuntimeMutationRequest {
            schema: self.schema.clone(),
            collection: self.collection.clone(),
            action: RuntimeMutationAction::Update,
            fields: Vec::new(),
            key: Some(RuntimeKey {
                schema: self.schema.clone(),
                collection: self.collection.clone(),
                fields: key.to_vec(),
            }),
            expected: None,
            returning: None,
            limits: self.limits,
            dependents: None,
            cause: None,
            links: Some(spec.clone()),
            attached: Vec::new(),
            max_delete_links: self.max_delete_links,
        }
    }
    /// The join relation and change of a link request.
    pub fn link_change(&self) -> Option<(&RuntimeRelationHandle, &RuntimeLinkChange)> {
        self.links.as_ref().map(|l| (&l.relation, &l.change))
    }
    /// Bound the links a delete may remove per join relation side, visible or
    /// not. Defaults to [`RUNTIME_DEFAULT_MAX_DELETE_LINKS`], which is separate from
    /// a link mutation's key bound so a heavily linked row stays deletable.
    pub fn with_max_delete_links(mut self, max_delete_links: usize) -> Self {
        self.max_delete_links = max_delete_links;
        self
    }
}

/// Whether an incoming foreign key belongs to a join table of this schema.
pub(super) fn is_link_dependency(
    schema: &ValidatedRuntimeSchema,
    dependency: &VerifiedRuntimeDependency,
) -> bool {
    schema.schema().collections.iter().any(|c| {
        c.relations.iter().any(|r| {
            r.join
                .as_ref()
                .is_some_and(|j| j.physical_table == dependency.source_table)
        })
    })
}

/// One side of a join table as seen from a row being written: the columns that
/// hold this row's key and the other side's fields and columns.
struct LinkSide {
    relation: RuntimeRelationHandle,
    own: Vec<(RuntimeFieldHandle, String)>,
    other: Vec<(RuntimeFieldHandle, String)>,
    other_collection: RuntimeCollectionHandle,
    /// Whether this row is the relation's source.
    is_source: bool,
}

/// Every distinct join table referencing `collection`, once per table.
fn link_sides(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
) -> Result<Vec<LinkSide>, RuntimeMutationError> {
    let mut tables = BTreeSet::new();
    let mut sides = Vec::new();
    for c in &schema.schema().collections {
        for r in &c.relations {
            let Some(join) = &r.join else {
                continue;
            };
            let touches_source = &c.id == collection.id();
            let touches_target = &r.target == collection.id();
            if !(touches_source || touches_target)
                || !tables.insert(join.physical_table.to_ascii_lowercase())
            {
                continue;
            }
            let source = schema.resolve_collection(&c.id)?;
            let relation = schema.resolve_relation(&source, &r.id)?;
            let handle = relation
                .join()
                .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
            // A self-relation holds this row on both sides: remove both directions.
            for is_source in [true, false] {
                if (is_source && !touches_source) || (!is_source && !touches_target) {
                    continue;
                }
                let (own, other, other_collection) = if is_source {
                    (handle.source(), handle.target(), relation.target().clone())
                } else {
                    (handle.target(), handle.source(), relation.source().clone())
                };
                sides.push(LinkSide {
                    relation: relation.clone(),
                    own: own.to_vec(),
                    other: other.to_vec(),
                    other_collection,
                    is_source,
                });
            }
        }
    }
    Ok(sides)
}

fn decode_keys<B: RuntimeMutationBackend>(
    schema: &ValidatedRuntimeSchema,
    collection: &RuntimeCollectionHandle,
    fields: &[RuntimeFieldHandle],
    rows: &[B::Row],
) -> Result<Vec<RuntimeKey>, RuntimeMutationError> {
    let projection = schema.resolve_projection(collection, fields)?;
    rows.iter()
        .map(|row| {
            let record = projection.decode_row::<B>(row)?;
            let key = fields
                .iter()
                .map(|f| Ok((f.clone(), record.value(f)?.clone())))
                .collect::<Result<Vec<_>, RuntimeMutationError>>()?;
            schema.runtime_key(collection, &key)
        })
        .collect()
}

impl<B: RuntimeMutationBackend> MutationContext<'_, B> {
    /// Require every requested target to be an existing row of the relation's
    /// target collection that satisfies `visible`, in bind-bounded batches.
    /// PostgreSQL locks the rows `FOR KEY SHARE` so they cannot be deleted before
    /// commit; SQLite's immediate write transaction already excludes writers.
    /// Any failure is a uniform `Denied`, so a hidden target and a missing one
    /// are indistinguishable.
    async fn admit_link_targets(
        &mut self,
        relation: &RuntimeRelationHandle,
        targets: &[&RuntimeKey],
        visible: Option<&RuntimePredicate>,
        limits: RuntimeMutationLimits,
    ) -> Result<(), RuntimeMutationError> {
        let d = B::DIALECT;
        let join = relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let fields = join.target();
        let tuple = fields
            .iter()
            .map(|(field, _)| d.quote_identifier(field.physical_column()))
            .collect::<Vec<_>>()
            .join(", ");
        let reserved = visible.map_or(0, |p| {
            let mut probe = Vec::new();
            p.render(d, &mut probe);
            probe.len()
        });
        let chunk =
            (limits.query.max_bind_parameters.saturating_sub(reserved) / fields.len()).max(1);
        for batch in targets.chunks(chunk) {
            let mut values = Vec::new();
            let list = batch
                .iter()
                .map(|target| {
                    let binds = fields
                        .iter()
                        .zip(&target.fields)
                        .map(|((field, _), (_, value))| bind_value(d, field, value, &mut values))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({binds})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let mut condition = format!("({tuple}) IN ({list})");
            if let Some(visible) = visible {
                condition = format!("{condition} AND ({})", visible.render(d, &mut values));
            }
            if values.len() > limits.query.max_bind_parameters {
                return Err(err(RuntimeMutationErrorCode::LimitExceeded));
            }
            let sql = format!(
                "SELECT 1 AS {} FROM {} WHERE {condition}{}",
                d.quote_identifier("graphql_orm_admitted"),
                d.quote_identifier(relation.target().physical_table()),
                if d == DatabaseBackend::Postgres {
                    " FOR KEY SHARE"
                } else {
                    ""
                }
            );
            let rows = self
                .fetch_rows(&sql, &values)
                .await
                .map_err(database_error::<B>)?;
            // Requested targets are distinct, so each admitted key matches one row.
            if rows.len() != batch.len() {
                return Err(err(RuntimeMutationErrorCode::Denied));
            }
        }
        Ok(())
    }

    /// Lock and read the other-side keys linked to `key` through `side`, only
    /// those whose target row satisfies `visible` when given.
    async fn linked_keys(
        &mut self,
        request: &RuntimeMutationRequest,
        side: &LinkSide,
        key: &RuntimeKey,
        visible: Option<&RuntimePredicate>,
        limit: usize,
    ) -> Result<Vec<RuntimeKey>, RuntimeMutationError> {
        let d = B::DIALECT;
        let join = side
            .relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let alias = d.quote_identifier("graphql_orm_link");
        let mut values = Vec::new();
        let mut condition = side
            .own
            .iter()
            .zip(&key.fields)
            .map(|((field, column), (_, value))| {
                format!(
                    "{alias}.{} = {}",
                    d.quote_identifier(column),
                    bind_value(d, field, value, &mut values)
                )
            })
            .collect::<Vec<_>>();
        if let Some(visible) = visible {
            // Unqualified predicate columns resolve to the target row of the subquery.
            let correlation = side
                .other
                .iter()
                .map(|(field, column)| {
                    format!(
                        "{}.{} = {alias}.{}",
                        d.quote_identifier("graphql_orm_target"),
                        d.quote_identifier(field.physical_column()),
                        d.quote_identifier(column)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            condition.push(format!(
                "EXISTS (SELECT 1 FROM {} AS {} WHERE {correlation} AND ({}))",
                d.quote_identifier(side.other_collection.physical_table()),
                d.quote_identifier("graphql_orm_target"),
                visible.render(d, &mut values)
            ));
        }
        if values.len() > request.limits.query.max_bind_parameters {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        let selected = side
            .other
            .iter()
            .map(|(field, column)| {
                format!(
                    "{alias}.{} AS {}",
                    d.quote_identifier(column),
                    d.quote_identifier(field.physical_column())
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let order = side
            .other
            .iter()
            .map(|(_, column)| format!("{alias}.{}", d.quote_identifier(column)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {selected} FROM {} AS {alias} WHERE {} ORDER BY {order} LIMIT {}{}",
            d.quote_identifier(join.physical_table()),
            condition.join(" AND "),
            limit.saturating_add(1),
            if d == DatabaseBackend::Postgres {
                " FOR UPDATE"
            } else {
                ""
            }
        );
        let rows = self
            .fetch_rows(&sql, &values)
            .await
            .map_err(database_error::<B>)?;
        if rows.len() > limit {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        let fields = side
            .other
            .iter()
            .map(|(f, _)| f.clone())
            .collect::<Vec<_>>();
        decode_keys::<B>(&request.schema, &side.other_collection, &fields, &rows)
    }

    /// Insert links in bind-bounded batches. An existing pair (for example one a
    /// concurrent writer added from the inverse side) is left unchanged; returns
    /// the links actually inserted.
    async fn insert_links(
        &mut self,
        schema: &ValidatedRuntimeSchema,
        relation: &RuntimeRelationHandle,
        links: &[RuntimeLink],
        limits: RuntimeMutationLimits,
    ) -> Result<Vec<RuntimeLink>, RuntimeMutationError> {
        let d = B::DIALECT;
        let join = relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let per_row = join.source().len() + join.target().len();
        let chunk = (limits.query.max_bind_parameters / per_row).max(1);
        let columns = join
            .source()
            .iter()
            .chain(join.target())
            .map(|(_, c)| d.quote_identifier(c))
            .collect::<Vec<_>>()
            .join(", ");
        let returning = join
            .target()
            .iter()
            .map(|(field, column)| {
                format!(
                    "{} AS {}",
                    d.quote_identifier(column),
                    d.quote_identifier(field.physical_column())
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let target_fields = join
            .target()
            .iter()
            .map(|(f, _)| f.clone())
            .collect::<Vec<_>>();
        let mut inserted = Vec::new();
        for batch in links.chunks(chunk) {
            let mut values = Vec::new();
            let rows = batch
                .iter()
                .map(|link| {
                    let binds = join
                        .source()
                        .iter()
                        .zip(&link.source.fields)
                        .chain(join.target().iter().zip(&link.target.fields))
                        .map(|((field, _), (_, value))| bind_value(d, field, value, &mut values))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({binds})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "INSERT INTO {} ({columns}) VALUES {rows} ON CONFLICT DO NOTHING RETURNING {returning}",
                d.quote_identifier(join.physical_table())
            );
            let returned = self
                .fetch_rows(&sql, &values)
                .await
                .map_err(database_error::<B>)?;
            let keys = decode_keys::<B>(schema, relation.target(), &target_fields, &returned)?;
            let mut done = BTreeSet::new();
            for key in &keys {
                done.insert(key_identity(key)?);
            }
            for link in batch {
                if done.contains(&key_identity(&link.target)?) {
                    inserted.push(link.clone());
                }
            }
        }
        Ok(inserted)
    }

    /// Remove `targets` linked to `source` in bind-bounded batches; every pair
    /// must exist (all are locked).
    async fn delete_links(
        &mut self,
        relation: &RuntimeRelationHandle,
        source: &RuntimeKey,
        targets: &[RuntimeKey],
        limits: RuntimeMutationLimits,
    ) -> Result<(), RuntimeMutationError> {
        let d = B::DIALECT;
        let join = relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let per_row = join.target().len();
        let chunk = (limits
            .query
            .max_bind_parameters
            .saturating_sub(join.source().len())
            / per_row)
            .max(1);
        let tuple = join
            .target()
            .iter()
            .map(|(_, c)| d.quote_identifier(c))
            .collect::<Vec<_>>()
            .join(", ");
        for batch in targets.chunks(chunk) {
            let mut values = Vec::new();
            let own = join
                .source()
                .iter()
                .zip(&source.fields)
                .map(|((field, column), (_, value))| {
                    format!(
                        "{} = {}",
                        d.quote_identifier(column),
                        bind_value(d, field, value, &mut values)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            let list = batch
                .iter()
                .map(|target| {
                    let binds = join
                        .target()
                        .iter()
                        .zip(&target.fields)
                        .map(|((field, _), (_, value))| bind_value(d, field, value, &mut values))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({binds})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "DELETE FROM {} WHERE {own} AND ({tuple}) IN ({list})",
                d.quote_identifier(join.physical_table())
            );
            let result = self
                .execute(&sql, &values)
                .await
                .map_err(database_error::<B>)?;
            if Self::rows_affected(&result) != batch.len() as u64 {
                return Err(err(RuntimeMutationErrorCode::Conflict));
            }
        }
        Ok(())
    }

    /// Delete every link holding `key` on `side` in one statement.
    async fn delete_side(
        &mut self,
        side: &LinkSide,
        key: &RuntimeKey,
    ) -> Result<u64, RuntimeMutationError> {
        let d = B::DIALECT;
        let join = side
            .relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let mut values = Vec::new();
        let own = side
            .own
            .iter()
            .zip(&key.fields)
            .map(|((field, column), (_, value))| {
                format!(
                    "{} = {}",
                    d.quote_identifier(column),
                    bind_value(d, field, value, &mut values)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let result = self
            .execute(
                &format!(
                    "DELETE FROM {} WHERE {own}",
                    d.quote_identifier(join.physical_table())
                ),
                &values,
            )
            .await
            .map_err(database_error::<B>)?;
        Ok(Self::rows_affected(&result))
    }

    /// Lock, authorize and remove every link of a row about to be deleted.
    pub(super) async fn remove_runtime_links(
        &mut self,
        request: &RuntimeMutationRequest,
        authority: &dyn RuntimeWriteAuthority<B>,
    ) -> Result<Vec<RuntimeLinkEffect>, RuntimeMutationError> {
        let key = request
            .key
            .as_ref()
            .ok_or_else(|| err(RuntimeMutationErrorCode::InvalidInput))?;
        let sides = link_sides(&request.schema, &request.collection)?;
        let mut planned = Vec::new();
        for side in &sides {
            let others = self
                .linked_keys(request, side, key, None, request.max_delete_links)
                .await?;
            let removed = others
                .into_iter()
                .map(|other| {
                    if side.is_source {
                        RuntimeLink {
                            source: key.clone(),
                            target: other,
                        }
                    } else {
                        RuntimeLink {
                            source: other,
                            target: key.clone(),
                        }
                    }
                })
                .collect::<Vec<_>>();
            if !removed.is_empty() {
                planned.push((side.relation.clone(), removed));
            }
        }
        // A self-relation may list the same link from both sides once.
        let mut effects: Vec<RuntimeLinkEffect> = Vec::new();
        for (relation, removed) in planned {
            match effects.iter_mut().find(|e| e.relation == relation) {
                Some(effect) => {
                    for link in removed {
                        let identity = (key_identity(&link.source)?, key_identity(&link.target)?);
                        let mut duplicate = false;
                        for existing in &effect.removed {
                            duplicate |= (
                                key_identity(&existing.source)?,
                                key_identity(&existing.target)?,
                            ) == identity;
                        }
                        if !duplicate {
                            effect.removed.push(link);
                        }
                    }
                }
                None => effects.push(RuntimeLinkEffect {
                    relation,
                    added: Vec::new(),
                    removed,
                }),
            }
        }
        let intent = RuntimeWriteIntent { request };
        for effect in &effects {
            authority
                .authorize_links(
                    RuntimeLinkCheck {
                        intent,
                        relation: &effect.relation,
                        requested_add: &[],
                        requested_remove: &[],
                        added: &[],
                        removed: &effect.removed,
                    },
                    self,
                )
                .await?;
        }
        // One statement per side; together they remove exactly the planned links.
        let mut deleted = 0u64;
        for side in &sides {
            deleted += self.delete_side(side, key).await?;
        }
        let planned = effects.iter().map(|e| e.removed.len() as u64).sum::<u64>();
        if deleted != planned {
            return Err(err(RuntimeMutationErrorCode::Conflict));
        }
        Ok(effects)
    }

    /// Execute a link request after the source row's intent and preimage checks.
    pub(super) async fn apply_runtime_links(
        &mut self,
        request: &RuntimeMutationRequest,
        authority: &dyn RuntimeWriteAuthority<B>,
    ) -> Result<RuntimeLinkEffect, RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        let spec = request.links.as_ref().ok_or_else(|| err(E::InvalidInput))?;
        let key = request.key.as_ref().ok_or_else(|| err(E::InvalidInput))?;
        let join = spec.relation.join().ok_or_else(|| err(E::SchemaMismatch))?;
        let intent = RuntimeWriteIntent { request };
        let visible = authority
            .link_target_visibility(
                RuntimeLinkVisibilityCheck {
                    intent,
                    relation: &spec.relation,
                },
                self,
            )
            .await?;
        if visible.as_ref().is_some_and(|p| {
            !p.belongs_to(&request.schema.fingerprint(), spec.relation.target().id())
        }) {
            return Err(err(E::SchemaMismatch));
        }
        if let Some(p) = &visible {
            p.check_mutation_limits(request.limits)?;
        }
        let (requested_add, requested_remove): (&[RuntimeKey], &[RuntimeKey]) = match &spec.change {
            RuntimeLinkChange::Replace(keys) => (keys, &[]),
            RuntimeLinkChange::Delta { add, remove } => (add, remove),
        };
        let requested = requested_add
            .iter()
            .chain(requested_remove)
            .collect::<Vec<_>>();
        if !requested.is_empty() {
            self.admit_link_targets(&spec.relation, &requested, visible.as_ref(), request.limits)
                .await?;
        }
        let side = LinkSide {
            relation: spec.relation.clone(),
            own: join.source().to_vec(),
            other: join.target().to_vec(),
            other_collection: spec.relation.target().clone(),
            is_source: true,
        };
        let current = self
            .linked_keys(request, &side, key, visible.as_ref(), spec.max_link_keys)
            .await?;
        let mut linked = BTreeSet::new();
        for target in &current {
            linked.insert(key_identity(target)?);
        }
        let mut add = Vec::new();
        for k in requested_add {
            if !linked.contains(&key_identity(k)?) {
                add.push(k);
            }
        }
        let mut remove = Vec::new();
        match &spec.change {
            // Replace only touches visible links.
            RuntimeLinkChange::Replace(keys) => {
                let mut wanted = BTreeSet::new();
                for k in keys {
                    wanted.insert(key_identity(k)?);
                }
                for k in &current {
                    if !wanted.contains(&key_identity(k)?) {
                        remove.push(k);
                    }
                }
            }
            RuntimeLinkChange::Delta {
                remove: requested, ..
            } => {
                for k in requested {
                    if linked.contains(&key_identity(k)?) {
                        remove.push(k);
                    }
                }
            }
        }
        let link = |target: &RuntimeKey| RuntimeLink {
            source: key.clone(),
            target: target.clone(),
        };
        let mut effect = RuntimeLinkEffect {
            relation: spec.relation.clone(),
            added: add.into_iter().map(link).collect(),
            removed: remove.into_iter().map(link).collect(),
        };
        if current.len() - effect.removed.len() + effect.added.len() > spec.max_link_keys {
            return Err(err(E::LimitExceeded));
        }
        authority
            .authorize_links(
                RuntimeLinkCheck {
                    intent,
                    relation: &effect.relation,
                    requested_add,
                    requested_remove,
                    added: &effect.added,
                    removed: &effect.removed,
                },
                self,
            )
            .await?;
        let removed_targets = effect
            .removed
            .iter()
            .map(|l| l.target.clone())
            .collect::<Vec<_>>();
        self.delete_links(&effect.relation, key, &removed_targets, request.limits)
            .await?;
        effect.added = self
            .insert_links(
                &request.schema,
                &effect.relation,
                &effect.added,
                request.limits,
            )
            .await?;
        Ok(effect)
    }
}
