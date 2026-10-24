//! Link-set writes of many-to-many relations and link cleanup on deletes.
//!
//! Links are rows of a relation's ORM-owned join table. They are never written
//! implicitly: a link mutation and every delete that removes links compute the
//! exact delta under row locks, present it to
//! [`RuntimeWriteAuthority::authorize_links`] before any write, perform it
//! explicitly and list it in [`RuntimeMutationEffect::links`] for hooks.
use super::*;
use std::collections::BTreeSet;

/// Default bound on the links one request may read or change for one relation.
pub const RUNTIME_DEFAULT_MAX_LINKS: usize = 1_000;

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
}

/// Exact link delta of one relation, computed under lock before any write.
pub struct RuntimeLinkCheck<'a> {
    /// The link mutation, or the delete removing these links.
    pub intent: RuntimeWriteIntent<'a>,
    /// The join relation. For a delete of its target, the relation is still
    /// described from its declaring (source) side.
    pub relation: &'a RuntimeRelationHandle,
    pub added: &'a [RuntimeLink],
    pub removed: &'a [RuntimeLink],
}
redacted_debug!(RuntimeLinkCheck<'_>);

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
    /// write authority sees its intent and locked preimage as usual, plus
    /// [`RuntimeWriteAuthority::authorize_links`] with the exact delta. The
    /// returned record is the unchanged source row. At most `max_links` target
    /// keys may be supplied and at most `max_links` current links are read.
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
        max_links: usize,
        limits: RuntimeMutationLimits,
    ) -> Result<RuntimeMutationRequest, RuntimeMutationError> {
        use RuntimeMutationErrorCode as E;
        if relation.join().is_none()
            || self
                .resolve_relation(relation.source(), relation.id())
                .map_err(|e| err(E::SchemaMismatch).with_source(e))?
                != *relation
        {
            return Err(err(E::InvalidInput));
        }
        if key.schema.fingerprint() != self.fingerprint() || key.collection != *relation.source() {
            return Err(err(E::SchemaMismatch));
        }
        let targets: Vec<&RuntimeKey> = match &change {
            RuntimeLinkChange::Replace(keys) => keys.iter().collect(),
            RuntimeLinkChange::Delta { add, remove } => add.iter().chain(remove).collect(),
        };
        if max_links == 0 || targets.len() > max_links {
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
            }),
            max_links,
            dependents: None,
            cause: None,
        })
    }
}

impl RuntimeMutationRequest {
    /// The join relation and change of a link request.
    pub fn link_change(&self) -> Option<(&RuntimeRelationHandle, &RuntimeLinkChange)> {
        self.links.as_ref().map(|l| (&l.relation, &l.change))
    }
    /// Bound the links a delete may remove per join relation, and that a link
    /// request may read. Defaults to [`RUNTIME_DEFAULT_MAX_LINKS`].
    pub fn with_max_links(mut self, max_links: usize) -> Self {
        self.max_links = max_links;
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

impl<B: RuntimeMutationBackend> MutationContext<'_, B> {
    /// Lock and read the other-side keys linked to `key` through `side`.
    async fn linked_keys(
        &mut self,
        request: &RuntimeMutationRequest,
        side: &LinkSide,
        key: &RuntimeKey,
        limit: usize,
    ) -> Result<Vec<RuntimeKey>, RuntimeMutationError> {
        let join = side
            .relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let mut values = Vec::new();
        let condition = side
            .own
            .iter()
            .zip(&key.fields)
            .map(|((field, column), (_, value))| {
                format!(
                    "{} = {}",
                    B::DIALECT.quote_identifier(column),
                    bind_value(B::DIALECT, field, value, &mut values)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let selected = side
            .other
            .iter()
            .map(|(field, column)| {
                format!(
                    "{} AS {}",
                    B::DIALECT.quote_identifier(column),
                    B::DIALECT.quote_identifier(field.physical_column())
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let order = side
            .other
            .iter()
            .map(|(_, column)| B::DIALECT.quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {selected} FROM {} WHERE {condition} ORDER BY {order} LIMIT {}{}",
            B::DIALECT.quote_identifier(join.physical_table()),
            limit.saturating_add(1),
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
        if rows.len() > limit {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        let fields = side
            .other
            .iter()
            .map(|(f, _)| f.clone())
            .collect::<Vec<_>>();
        let projection = request
            .schema
            .resolve_projection(&side.other_collection, &fields)?;
        rows.iter()
            .map(|row| {
                let record = projection.decode_row::<B>(row)?;
                let key = fields
                    .iter()
                    .map(|f| Ok((f.clone(), record.value(f)?.clone())))
                    .collect::<Result<Vec<_>, RuntimeMutationError>>()?;
                request.schema.runtime_key(&side.other_collection, &key)
            })
            .collect()
    }

    async fn write_link(
        &mut self,
        relation: &RuntimeRelationHandle,
        link: &RuntimeLink,
        insert: bool,
        limits: RuntimeMutationLimits,
    ) -> Result<(), RuntimeMutationError> {
        let join = relation
            .join()
            .ok_or_else(|| err(RuntimeMutationErrorCode::SchemaMismatch))?;
        let mut values = Vec::new();
        let members = join
            .source()
            .iter()
            .zip(&link.source.fields)
            .chain(join.target().iter().zip(&link.target.fields))
            .map(|((field, column), (_, value))| {
                (
                    B::DIALECT.quote_identifier(column),
                    bind_value(B::DIALECT, field, value, &mut values),
                )
            })
            .collect::<Vec<_>>();
        if values.len() > limits.query.max_bind_parameters {
            return Err(err(RuntimeMutationErrorCode::LimitExceeded));
        }
        let table = B::DIALECT.quote_identifier(join.physical_table());
        let sql = if insert {
            format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                members
                    .iter()
                    .map(|(c, _)| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                members
                    .iter()
                    .map(|(_, v)| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            format!(
                "DELETE FROM {table} WHERE {}",
                members
                    .iter()
                    .map(|(c, v)| format!("{c} = {v}"))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            )
        };
        let result = self
            .execute(&sql, &values)
            .await
            .map_err(database_error::<B>)?;
        if Self::rows_affected(&result) != 1 {
            return Err(err(RuntimeMutationErrorCode::Conflict));
        }
        Ok(())
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
        let mut planned = Vec::new();
        for side in link_sides(&request.schema, &request.collection)? {
            let others = self
                .linked_keys(request, &side, key, request.max_links)
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
                planned.push((side.relation, removed));
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
                        added: &[],
                        removed: &effect.removed,
                    },
                    self,
                )
                .await?;
        }
        for effect in &effects {
            for link in &effect.removed {
                self.write_link(&effect.relation, link, false, request.limits)
                    .await?;
            }
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
        let side = LinkSide {
            relation: spec.relation.clone(),
            own: join.source().to_vec(),
            other: join.target().to_vec(),
            other_collection: spec.relation.target().clone(),
            is_source: true,
        };
        let current = self
            .linked_keys(request, &side, key, request.max_links)
            .await?;
        let mut linked = BTreeSet::new();
        for target in &current {
            linked.insert(key_identity(target)?);
        }
        let (add, remove): (Vec<&RuntimeKey>, Vec<&RuntimeKey>) = match &spec.change {
            RuntimeLinkChange::Replace(keys) => {
                let mut wanted = BTreeSet::new();
                for k in keys {
                    wanted.insert(key_identity(k)?);
                }
                let mut remove = Vec::new();
                for k in &current {
                    if !wanted.contains(&key_identity(k)?) {
                        remove.push(k);
                    }
                }
                let mut add = Vec::new();
                for k in keys {
                    if !linked.contains(&key_identity(k)?) {
                        add.push(k);
                    }
                }
                (add, remove)
            }
            RuntimeLinkChange::Delta { add, remove } => {
                let mut added = Vec::new();
                for k in add {
                    if !linked.contains(&key_identity(k)?) {
                        added.push(k);
                    }
                }
                let mut removed = Vec::new();
                for k in remove {
                    if linked.contains(&key_identity(k)?) {
                        removed.push(k);
                    }
                }
                (added, removed)
            }
        };
        let link = |target: &RuntimeKey| RuntimeLink {
            source: key.clone(),
            target: target.clone(),
        };
        let effect = RuntimeLinkEffect {
            relation: spec.relation.clone(),
            added: add.into_iter().map(link).collect(),
            removed: remove.into_iter().map(link).collect(),
        };
        if current.len() - effect.removed.len() + effect.added.len() > request.max_links {
            return Err(err(E::LimitExceeded));
        }
        authority
            .authorize_links(
                RuntimeLinkCheck {
                    intent: RuntimeWriteIntent { request },
                    relation: &effect.relation,
                    added: &effect.added,
                    removed: &effect.removed,
                },
                self,
            )
            .await?;
        for link in &effect.removed {
            self.write_link(&effect.relation, link, false, request.limits)
                .await?;
        }
        for link in &effect.added {
            self.write_link(&effect.relation, link, true, request.limits)
                .await?;
        }
        Ok(effect)
    }
}
