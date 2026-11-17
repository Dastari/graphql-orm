//! Client-facing companion metadata and the interpretation fingerprint.
//!
//! `schema_roots!(client_metadata: true)` composes
//! [`GraphqlClientMetadata`] from the listed entities with
//! [`compose_client_metadata`]: the fields each generated `Changed` stream can
//! carry (#340), each entity's declared lifetime field (#670) and each public
//! relationship's key mapping (#672). Every `schema_roots!` also emits
//! `graphql_orm_interpretation_fingerprint`, built with
//! [`interpretation_fingerprint`] (#671).
//!
//! Both are descriptive. Neither grants or changes any authority.

use super::{
    ColumnDef, Entity, GraphqlChangedStreamMetadata, GraphqlClientEntityMetadata,
    GraphqlClientMetadata, GraphqlEntityLifetimeMetadata, GraphqlRelationshipKeyMapping,
    GraphqlRelationshipKeyMetadata, GraphqlRelationshipNotBatchableReason, GraphqlSemanticCatalog,
    OrmBackend, RelationMetadata, SqlDialect, relationship_key_field_exposed,
    relationship_target_filterable,
};

/// Version of the cursor and key encodings the interpretation fingerprint
/// covers: offset cursors, keyset cursor plaintext and sealing, composite key
/// inputs and global `NodeId` values.
///
/// It changes whenever a release changes how an existing cursor, key or
/// `NodeId` is read, so every interpretation fingerprint changes with it.
pub const GRAPHQL_ORM_CURSOR_FORMAT_VERSION: u32 = 1;

/// Algorithm label of [`interpretation_fingerprint`].
pub const GRAPHQL_ORM_INTERPRETATION_ALGORITHM: &str = "graphql-orm-interpretation-sha256-v1";

/// Suggested HTTP response header for a served interpretation fingerprint.
pub const GRAPHQL_ORM_INTERPRETATION_HEADER: &str = "graphql-orm-interpretation";

/// Suggested `connection_ack` payload member for a served interpretation
/// fingerprint.
pub const GRAPHQL_ORM_INTERPRETATION_ACK_FIELD: &str = "graphqlOrmInterpretation";

/// One entity's input to [`compose_client_metadata`], captured by
/// `schema_roots!`.
#[doc(hidden)]
pub struct ClientEntitySource {
    entity_name: &'static str,
    semantic_name: Option<String>,
    changed_root: &'static str,
    carried_fields: Vec<String>,
    identity_carried: bool,
    started_signal: bool,
    lifetime: Option<GraphqlEntityLifetimeMetadata>,
    columns: &'static [ColumnDef],
    relations: &'static [RelationMetadata],
    quote: fn(&str) -> String,
}

impl ClientEntitySource {
    /// Captures an entity's generated facts.
    pub fn of<T: Entity, B: OrmBackend>(changed_root: &'static str, started_signal: bool) -> Self {
        let (carried_fields, identity_carried) =
            crate::graphql::subscriptions::changed_stream_carried_fields::<T, B>();
        Self {
            entity_name: T::entity_name(),
            semantic_name: T::graphql_semantic_metadata().map(|entity| entity.entity_name.clone()),
            changed_root,
            carried_fields,
            identity_carried,
            started_signal,
            lifetime: T::graphql_lifetime(),
            columns: T::columns(),
            relations: T::relation_metadata(),
            quote: |identifier| B::DIALECT.quote_identifier_path(identifier),
        }
    }

    fn column_api_name(&self, column: &str) -> Option<&'static str> {
        column_api_name(self.columns, column, self.quote)
    }
}

fn column_api_name(
    columns: &'static [ColumnDef],
    column: &str,
    quote: fn(&str) -> String,
) -> Option<&'static str> {
    columns
        .iter()
        .find(|definition| definition.name == column || quote(definition.name) == column)
        .map(|definition| definition.api_name)
}

/// Composes client metadata for one generated schema root.
///
/// # Panics
///
/// Panics when the composed metadata does not validate against the catalogue,
/// which is a generator bug.
#[doc(hidden)]
pub fn compose_client_metadata(
    catalog: &GraphqlSemanticCatalog,
    sources: &[ClientEntitySource],
) -> GraphqlClientMetadata {
    let entities = sources
        .iter()
        .filter_map(|source| {
            let entity_name = source.semantic_name.as_deref()?;
            let semantic = catalog
                .entities
                .iter()
                .find(|entity| entity.entity_name == entity_name)?;
            let mut entity = GraphqlClientEntityMetadata::new(entity_name);
            let stream_published = catalog.operations.iter().any(|operation| {
                operation.field_name == source.changed_root
                    && operation.generated_entity_name.as_deref() == Some(entity_name)
                    && operation.kind == super::GraphqlOperationKind::Subscription
            });
            if stream_published {
                entity = entity.with_changed_stream(GraphqlChangedStreamMetadata::new(
                    source.changed_root,
                    source
                        .carried_fields
                        .iter()
                        .filter(|name| {
                            semantic.fields.iter().any(|field| {
                                &field.field_name == *name
                                    && field.selectable
                                    && field.relationship.is_none()
                            })
                        })
                        .cloned()
                        .collect(),
                    source.identity_carried,
                    source.started_signal,
                ));
            }
            if let Some(lifetime) = &source.lifetime {
                entity = entity.with_lifetime(lifetime.clone());
            }
            for relation in source.relations {
                let Some(field) = semantic.fields.iter().find(|field| {
                    field.field_name == relation.field_name && field.relationship.is_some()
                }) else {
                    continue;
                };
                let target = field
                    .relationship
                    .as_ref()
                    .map(|relationship| relationship.target.clone())
                    .unwrap_or_default();
                let key_mapping = relationship_key_mapping(
                    catalog,
                    source,
                    sources,
                    relation,
                    field.has_field_policy,
                    &target,
                );
                entity = entity.with_relationship(GraphqlRelationshipKeyMetadata::new(
                    relation.field_name,
                    target,
                    key_mapping,
                ));
            }
            Some(entity)
        })
        .collect::<Vec<_>>();
    GraphqlClientMetadata::compose(catalog, entities)
        .expect("macro-generated GraphQL client metadata must validate")
}

fn relationship_key_mapping(
    catalog: &GraphqlSemanticCatalog,
    source: &ClientEntitySource,
    sources: &[ClientEntitySource],
    relation: &RelationMetadata,
    has_field_policy: bool,
    target: &str,
) -> GraphqlRelationshipKeyMapping {
    if relation.has_source_condition || relation.has_target_condition {
        return GraphqlRelationshipKeyMapping::NotBatchable {
            reason: GraphqlRelationshipNotBatchableReason::Conditional,
        };
    }
    if has_field_policy {
        return GraphqlRelationshipKeyMapping::NotBatchable {
            reason: GraphqlRelationshipNotBatchableReason::FieldPolicy,
        };
    }
    let Some(target_source) = sources
        .iter()
        .find(|candidate| candidate.semantic_name.as_deref() == Some(target))
    else {
        return GraphqlRelationshipKeyMapping::NoExposedKey;
    };
    let source_name = source
        .semantic_name
        .as_deref()
        .unwrap_or(source.entity_name);
    let resolve = |owner: &ClientEntitySource, owner_name: &str, columns: &[&str]| {
        if columns.is_empty() {
            return None;
        }
        columns
            .iter()
            .map(|column| {
                let name = owner.column_api_name(column)?;
                relationship_key_field_exposed(catalog, owner_name, name).then(|| name.to_owned())
            })
            .collect::<Option<Vec<_>>>()
    };
    let (Some(source_fields), Some(target_fields)) = (
        resolve(source, source_name, relation.source_columns),
        resolve(target_source, target, relation.target_columns),
    ) else {
        return GraphqlRelationshipKeyMapping::NoExposedKey;
    };
    if source_fields.len() != target_fields.len() {
        return GraphqlRelationshipKeyMapping::NoExposedKey;
    }
    let target_filterable = relationship_target_filterable(catalog, target, &target_fields);
    GraphqlRelationshipKeyMapping::Batchable {
        source_fields,
        target_fields,
        target_filterable,
    }
}

/// One entity's input to [`interpretation_fingerprint`], captured by
/// `schema_roots!`.
#[doc(hidden)]
#[derive(serde::Serialize)]
pub struct InterpretationEntity {
    entity: &'static str,
    /// Compile-time facts: whether the entity serves `NodeId`, and the exact
    /// keyset order fingerprint its keyset cursors are bound to.
    generated: &'static str,
    /// The effective default sort, which orders offset pages.
    default_sort: &'static str,
    /// Primary-key fields in key order: public name, logical value kind and
    /// column type.
    keys: Vec<(&'static str, String, &'static str)>,
}

impl InterpretationEntity {
    /// Captures an entity's interpretation facts.
    pub fn of<T: Entity, B: OrmBackend>(generated: &'static str) -> Self {
        let quote = |identifier: &str| B::DIALECT.quote_identifier_path(identifier);
        let keys = T::PRIMARY_KEYS
            .iter()
            .filter_map(|key| {
                T::columns()
                    .iter()
                    .find(|column| column.name == *key || quote(column.name) == *key)
            })
            .map(|column| {
                (
                    column.api_name,
                    format!("{:?}", column.logical_type),
                    column.sql_type,
                )
            })
            .collect();
        Self {
            entity: T::entity_name(),
            generated,
            default_sort: T::DEFAULT_SORT,
            keys,
        }
    }
}

/// Derives the interpretation fingerprint of one generated schema root.
///
/// The value is lowercase hex SHA-256 over canonical JSON of:
///
/// - [`GRAPHQL_ORM_INTERPRETATION_ALGORITHM`] and
///   [`GRAPHQL_ORM_CURSOR_FORMAT_VERSION`];
/// - the backend dialect name;
/// - for each entity, sorted by entity name so the order of `entities:` does
///   not matter: its name (which `NodeId` values and keyset cursor bindings
///   encode), whether it serves `NodeId`, the keyset order fingerprint its
///   keyset cursors are bound to (every order column with direction and null
///   placement, key suffix included), its effective default sort, and each
///   primary-key field's public name, logical value kind and column type, in
///   key order;
/// - the SHA-256 of the host salt, when one is given.
///
/// It names no secret, host, path or version string. Column names and sort
/// expressions enter only through the hash. The cursor sealing key of
/// [`super::AuthorizedScanConfig`] is not an input: rotating it invalidates
/// sealed scan cursors, so change the salt when you rotate it.
pub fn interpretation_fingerprint(
    dialect: &'static str,
    entities: &[InterpretationEntity],
    salt: Option<&str>,
) -> String {
    use sha2::{Digest, Sha256};
    #[derive(serde::Serialize)]
    struct Input<'a> {
        algorithm: &'static str,
        cursor_format: u32,
        dialect: &'static str,
        entities: Vec<&'a InterpretationEntity>,
        salt: Option<String>,
    }
    let mut entities = entities.iter().collect::<Vec<_>>();
    entities.sort_by_key(|entity| entity.entity);
    let input = Input {
        algorithm: GRAPHQL_ORM_INTERPRETATION_ALGORITHM,
        cursor_format: GRAPHQL_ORM_CURSOR_FORMAT_VERSION,
        dialect,
        entities,
        salt: salt.map(|salt| format!("{:x}", Sha256::digest(salt.as_bytes()))),
    };
    let encoded = serde_json::to_vec(&input).expect("interpretation input serializes");
    format!("{:x}", Sha256::digest(&encoded))
}
