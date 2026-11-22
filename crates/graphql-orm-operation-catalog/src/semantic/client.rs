//! Client-facing companion metadata for one semantic catalogue.
//!
//! [`GraphqlClientMetadata`] is a separate, versioned and fingerprinted
//! document. It names facts a GraphQL client generator needs and the SDL does
//! not carry: which entity fields a generated `Changed` stream can deliver,
//! which field holds an entity's expiry time and how it is encoded, and how a
//! relationship's source fields map to its target fields. It is bound to one
//! [`GraphqlSemanticCatalog`] by that catalogue's fingerprint and names only
//! public GraphQL identities that the bound catalogue already publishes.
//!
//! The document is descriptive metadata. It grants no resolver, field, row,
//! database or provider authority, and the server never enforces it: an
//! expiry field is not used to hide rows, and a batchable relationship is
//! still read through the target's own list root and its policies. The
//! semantic catalogue, its wire version and its fingerprint are unchanged by
//! this document.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    GraphqlEntitySemanticMetadata, GraphqlSemanticCatalog, GraphqlSemanticError,
    GraphqlSemanticFieldMetadata, GraphqlSemanticFilterOperator, GraphqlSemanticTypeKind,
    GraphqlSemanticTypeRef, canonical_json_bytes, hex_sha256, valid_fingerprint,
    valid_graphql_name,
};
use crate::{GeneratedGraphqlOperationCategory, GraphqlOperationKind};

/// Current client-metadata wire version.
pub const GRAPHQL_CLIENT_METADATA_VERSION: u16 = 1;

/// Stable client-metadata fingerprint algorithm.
pub const GRAPHQL_CLIENT_METADATA_FINGERPRINT_ALGORITHM: &str =
    "graphql-orm-client-metadata-canonical-json-sha256-v1";

const MAXIMUM_FIELD_NAMES: usize = 2_048;
const MAXIMUM_KEY_FIELDS: usize = 16;

/// Client-facing companion metadata for one semantic catalogue.
///
/// Compose it with [`Self::compose`], which checks every name against the
/// bound catalogue. A deserialized document is a trusted host assertion like
/// the catalogue itself; [`Self::validate_against`] checks it against the
/// catalogue it claims to describe.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlClientMetadata {
    /// Wire version.
    pub version: u16,
    /// Fingerprint of the semantic catalogue this document describes.
    pub semantic_catalog_fingerprint: String,
    /// Entities with at least one client fact, ordered by GraphQL name.
    pub entities: Vec<GraphqlClientEntityMetadata>,
    /// Stable fingerprint over the versioned document.
    pub fingerprint: String,
}

/// Client facts for one public entity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlClientEntityMetadata {
    /// Public GraphQL entity name, as in the semantic catalogue.
    pub entity_name: String,
    /// What the entity's generated `Changed` stream can deliver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_stream: Option<GraphqlChangedStreamMetadata>,
    /// The field holding the record's expiry time, when the entity declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<GraphqlEntityLifetimeMetadata>,
    /// Key mappings of the entity's relationship fields, ordered by field name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<GraphqlRelationshipKeyMetadata>,
}

impl GraphqlClientEntityMetadata {
    /// Creates an entity entry with no facts.
    pub fn new(entity_name: impl Into<String>) -> Self {
        Self {
            entity_name: entity_name.into(),
            changed_stream: None,
            lifetime: None,
            relationships: Vec::new(),
        }
    }

    /// Sets the `Changed` stream facts.
    #[must_use]
    pub fn with_changed_stream(mut self, changed_stream: GraphqlChangedStreamMetadata) -> Self {
        self.changed_stream = Some(changed_stream);
        self
    }

    /// Sets the lifetime field.
    #[must_use]
    pub fn with_lifetime(mut self, lifetime: GraphqlEntityLifetimeMetadata) -> Self {
        self.lifetime = Some(lifetime);
        self
    }

    /// Adds one relationship key mapping.
    #[must_use]
    pub fn with_relationship(mut self, relationship: GraphqlRelationshipKeyMetadata) -> Self {
        self.relationships.push(relationship);
        self
    }

    fn is_empty(&self) -> bool {
        self.changed_stream.is_none() && self.lifetime.is_none() && self.relationships.is_empty()
    }
}

/// What one generated `<Entity>Changed` stream can deliver.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlChangedStreamMetadata {
    /// Public subscription root field.
    pub root_field: String,
    /// Entity fields an event's entity payload may select, in the entity's
    /// field order. Selecting any other field refuses the whole stream.
    /// A field with a field policy is listed; an event whose row the
    /// subscriber may not read in that field is not delivered to them.
    pub carried_fields: Vec<String>,
    /// Whether the event's identity fields (`Id`, and `Key` for a composite
    /// key) may be selected.
    pub identity_carried: bool,
    /// Whether the root accepts the opt-in started signal (`Started: true`).
    pub started_signal: bool,
}

impl GraphqlChangedStreamMetadata {
    /// Creates stream facts.
    pub fn new(
        root_field: impl Into<String>,
        carried_fields: Vec<String>,
        identity_carried: bool,
        started_signal: bool,
    ) -> Self {
        Self {
            root_field: root_field.into(),
            carried_fields,
            identity_carried,
            started_signal,
        }
    }
}

/// How an expiry field encodes its time. Every encoding is UTC.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlLifetimeEncoding {
    /// An RFC 3339 timestamp string in UTC, as the GraphQL `DateTime` scalar
    /// serializes `chrono::DateTime<Utc>`.
    Rfc3339Utc,
    /// Whole seconds since the Unix epoch.
    UnixSeconds,
    /// Whole milliseconds since the Unix epoch.
    UnixMilliseconds,
    /// SQLite's UTC text form, `YYYY-MM-DD HH:MM:SS` with optional
    /// fractional seconds and no offset.
    SqliteUtcText,
}

/// The field holding a record's expiry time.
///
/// A null value means the record does not expire. The server does not enforce
/// expiry: an expired row stays readable until it is written or deleted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlEntityLifetimeMetadata {
    /// Public GraphQL field name of the expiry field.
    pub field: String,
    /// The field's time encoding.
    pub encoding: GraphqlLifetimeEncoding,
}

impl GraphqlEntityLifetimeMetadata {
    /// Creates lifetime facts.
    pub fn new(field: impl Into<String>, encoding: GraphqlLifetimeEncoding) -> Self {
        Self {
            field: field.into(),
            encoding,
        }
    }
}

/// Key mapping facts for one relationship field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GraphqlRelationshipKeyMetadata {
    /// Public relationship field name on the owning entity.
    pub field_name: String,
    /// Public target entity name.
    pub target: String,
    /// The mapping.
    pub key_mapping: GraphqlRelationshipKeyMapping,
}

impl GraphqlRelationshipKeyMetadata {
    /// Creates relationship facts.
    pub fn new(
        field_name: impl Into<String>,
        target: impl Into<String>,
        key_mapping: GraphqlRelationshipKeyMapping,
    ) -> Self {
        Self {
            field_name: field_name.into(),
            target: target.into(),
            key_mapping,
        }
    }
}

/// How a relationship's source fields map to its target fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum GraphqlRelationshipKeyMapping {
    /// Equal values of the source fields and the target fields, position by
    /// position, reproduce the relationship.
    Batchable {
        /// Public source field names on the owning entity, in key order.
        source_fields: Vec<String>,
        /// Public target field names, in the same order.
        target_fields: Vec<String>,
        /// Whether every target field accepts both `Equal` and `In` in the
        /// target's generated `WhereInput`.
        target_filterable: bool,
    },
    /// The relationship cannot be reproduced by a key read. Its condition or
    /// path is not published.
    NotBatchable {
        /// Why.
        reason: GraphqlRelationshipNotBatchableReason,
    },
    /// A key field on either side is not public, is classified secret, is
    /// hidden by a field policy, or is unknown to this catalogue. No key name
    /// is published.
    NoExposedKey,
}

/// Why a relationship cannot be batched by key.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GraphqlRelationshipNotBatchableReason {
    /// The relationship has a source or target condition beyond its keys.
    Conditional,
    /// The relationship field has a field policy, which a read of the
    /// target's list root does not apply.
    FieldPolicy,
}

impl GraphqlClientMetadata {
    /// Composes and validates client metadata for one semantic catalogue.
    ///
    /// Entities without any fact are dropped, entities and relationships are
    /// put in canonical order, and the fingerprint is computed.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalogue is invalid, or when a name does not
    /// match what the catalogue publishes.
    pub fn compose(
        catalog: &GraphqlSemanticCatalog,
        entities: impl IntoIterator<Item = GraphqlClientEntityMetadata>,
    ) -> Result<Self, GraphqlSemanticError> {
        catalog.validate()?;
        let mut entities = entities
            .into_iter()
            .filter(|entity| !entity.is_empty())
            .map(|mut entity| {
                entity
                    .relationships
                    .sort_by(|left, right| left.field_name.cmp(&right.field_name));
                entity
            })
            .collect::<Vec<_>>();
        entities.sort_by(|left, right| left.entity_name.cmp(&right.entity_name));
        let mut metadata = Self {
            version: GRAPHQL_CLIENT_METADATA_VERSION,
            semantic_catalog_fingerprint: catalog.fingerprint.clone(),
            entities,
            fingerprint: String::new(),
        };
        metadata.fingerprint = metadata.compute_fingerprint();
        metadata.validate_against(catalog)?;
        Ok(metadata)
    }

    /// Validates the document's shape, ordering and fingerprint.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported version, malformed names,
    /// non-canonical ordering, bounds violations or a stale fingerprint.
    pub fn validate(&self) -> Result<(), GraphqlSemanticError> {
        if self.version != GRAPHQL_CLIENT_METADATA_VERSION {
            return Err(GraphqlSemanticError::new(
                "client metadata version is unsupported",
            ));
        }
        if !valid_fingerprint(&self.semantic_catalog_fingerprint) {
            return Err(GraphqlSemanticError::new(
                "client metadata catalogue binding is invalid",
            ));
        }
        if self
            .entities
            .windows(2)
            .any(|pair| pair[0].entity_name >= pair[1].entity_name)
        {
            return Err(GraphqlSemanticError::new(
                "client metadata entity ordering is not canonical",
            ));
        }
        for entity in &self.entities {
            validate_entity_shape(entity)?;
        }
        if !valid_fingerprint(&self.fingerprint) || self.compute_fingerprint() != self.fingerprint {
            return Err(GraphqlSemanticError::new(
                "client metadata fingerprint is stale",
            ));
        }
        Ok(())
    }

    /// Validates the document and checks every name against the catalogue it
    /// is bound to.
    ///
    /// # Errors
    ///
    /// Returns an error when [`Self::validate`] fails, the catalogue
    /// fingerprint differs, or a fact names an entity, field, root or target
    /// the catalogue does not publish in that role.
    pub fn validate_against(
        &self,
        catalog: &GraphqlSemanticCatalog,
    ) -> Result<(), GraphqlSemanticError> {
        self.validate()?;
        if self.semantic_catalog_fingerprint != catalog.fingerprint {
            return Err(GraphqlSemanticError::new(
                "client metadata describes another semantic catalogue",
            ));
        }
        for entity in &self.entities {
            let semantic = semantic_entity(catalog, &entity.entity_name)?;
            if let Some(stream) = &entity.changed_stream {
                validate_changed_stream(catalog, semantic, stream)?;
            }
            if let Some(lifetime) = &entity.lifetime {
                validate_lifetime(semantic, lifetime)?;
            }
            for relationship in &entity.relationships {
                validate_relationship(catalog, semantic, relationship)?;
            }
        }
        Ok(())
    }

    /// Returns the facts for one entity, when it has any.
    pub fn entity(&self, entity_name: &str) -> Option<&GraphqlClientEntityMetadata> {
        self.entities
            .iter()
            .find(|entity| entity.entity_name == entity_name)
    }

    /// Encodes the validated document as canonical JSON, for a build-time
    /// export beside the semantic catalogue.
    ///
    /// # Errors
    ///
    /// Returns an error when validation fails.
    pub fn export_json(&self) -> Result<String, GraphqlSemanticError> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|_| GraphqlSemanticError::new("client metadata encoding failed"))?;
        String::from_utf8(canonical_json_bytes(&value))
            .map_err(|_| GraphqlSemanticError::new("client metadata encoding failed"))
    }

    /// Decodes and validates an exported document.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, unsupported or stale payloads.
    pub fn from_export_json(value: &str) -> Result<Self, GraphqlSemanticError> {
        let metadata: Self = serde_json::from_str(value)
            .map_err(|_| GraphqlSemanticError::new("client metadata payload is invalid"))?;
        metadata.validate()?;
        Ok(metadata)
    }

    fn compute_fingerprint(&self) -> String {
        #[derive(Serialize)]
        struct FingerprintInput<'a> {
            algorithm: &'static str,
            version: u16,
            semantic_catalog_fingerprint: &'a str,
            entities: &'a [GraphqlClientEntityMetadata],
        }
        let value = serde_json::to_value(FingerprintInput {
            algorithm: GRAPHQL_CLIENT_METADATA_FINGERPRINT_ALGORITHM,
            version: self.version,
            semantic_catalog_fingerprint: &self.semantic_catalog_fingerprint,
            entities: &self.entities,
        })
        .expect("client metadata fingerprint input serializes");
        hex_sha256(&canonical_json_bytes(&value))
    }
}

fn names_valid(names: &[String]) -> bool {
    names.iter().all(|name| valid_graphql_name(name))
}

fn validate_entity_shape(entity: &GraphqlClientEntityMetadata) -> Result<(), GraphqlSemanticError> {
    if !valid_graphql_name(&entity.entity_name) || entity.is_empty() {
        return Err(GraphqlSemanticError::new(
            "client metadata entity is invalid",
        ));
    }
    if let Some(stream) = &entity.changed_stream {
        let unique = stream.carried_fields.iter().collect::<BTreeSet<_>>();
        if !valid_graphql_name(&stream.root_field)
            || stream.carried_fields.len() > MAXIMUM_FIELD_NAMES
            || !names_valid(&stream.carried_fields)
            || unique.len() != stream.carried_fields.len()
        {
            return Err(GraphqlSemanticError::new(
                "client metadata changed stream is invalid",
            ));
        }
    }
    if let Some(lifetime) = &entity.lifetime
        && !valid_graphql_name(&lifetime.field)
    {
        return Err(GraphqlSemanticError::new(
            "client metadata lifetime is invalid",
        ));
    }
    if entity.relationships.len() > MAXIMUM_FIELD_NAMES
        || entity
            .relationships
            .windows(2)
            .any(|pair| pair[0].field_name >= pair[1].field_name)
    {
        return Err(GraphqlSemanticError::new(
            "client metadata relationship ordering is not canonical",
        ));
    }
    for relationship in &entity.relationships {
        if !valid_graphql_name(&relationship.field_name)
            || !valid_graphql_name(&relationship.target)
        {
            return Err(GraphqlSemanticError::new(
                "client metadata relationship is invalid",
            ));
        }
        if let GraphqlRelationshipKeyMapping::Batchable {
            source_fields,
            target_fields,
            ..
        } = &relationship.key_mapping
            && (source_fields.is_empty()
                || source_fields.len() > MAXIMUM_KEY_FIELDS
                || source_fields.len() != target_fields.len()
                || !names_valid(source_fields)
                || !names_valid(target_fields))
        {
            return Err(GraphqlSemanticError::new(
                "client metadata relationship key mapping is invalid",
            ));
        }
    }
    Ok(())
}

fn semantic_entity<'a>(
    catalog: &'a GraphqlSemanticCatalog,
    entity_name: &str,
) -> Result<&'a GraphqlEntitySemanticMetadata, GraphqlSemanticError> {
    catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == entity_name)
        .ok_or_else(|| GraphqlSemanticError::new("client metadata entity is not catalogued"))
}

fn semantic_field<'a>(
    entity: &'a GraphqlEntitySemanticMetadata,
    field_name: &str,
) -> Option<&'a GraphqlSemanticFieldMetadata> {
    entity
        .fields
        .iter()
        .find(|field| field.field_name == field_name)
}

/// A selectable, non-list scalar or enum field with no relationship.
fn is_plain_leaf(field: &GraphqlSemanticFieldMetadata) -> bool {
    field.selectable
        && field.relationship.is_none()
        && matches!(
            field.type_ref,
            GraphqlSemanticTypeRef::Named {
                kind: GraphqlSemanticTypeKind::Scalar | GraphqlSemanticTypeKind::Enum,
                ..
            }
        )
}

fn validate_changed_stream(
    catalog: &GraphqlSemanticCatalog,
    entity: &GraphqlEntitySemanticMetadata,
    stream: &GraphqlChangedStreamMetadata,
) -> Result<(), GraphqlSemanticError> {
    let root = catalog.operations.iter().any(|operation| {
        operation.kind == GraphqlOperationKind::Subscription
            && operation.field_name == stream.root_field
            && operation.generated_category == Some(GeneratedGraphqlOperationCategory::Subscription)
            && operation.generated_entity_name.as_deref() == Some(entity.entity_name.as_str())
    });
    let mut previous = None;
    let ordered = stream.carried_fields.iter().all(|name| {
        let position = entity
            .fields
            .iter()
            .position(|field| &field.field_name == name && field.selectable);
        let ordered = position.is_some() && position > previous;
        previous = position;
        ordered
    });
    if !root || !ordered {
        return Err(GraphqlSemanticError::new(
            "client metadata changed stream does not match the catalogue",
        ));
    }
    Ok(())
}

fn validate_lifetime(
    entity: &GraphqlEntitySemanticMetadata,
    lifetime: &GraphqlEntityLifetimeMetadata,
) -> Result<(), GraphqlSemanticError> {
    let valid = semantic_field(entity, &lifetime.field).is_some_and(is_exposed_leaf);
    if !valid {
        return Err(GraphqlSemanticError::new(
            "client metadata lifetime does not match the catalogue",
        ));
    }
    Ok(())
}

fn validate_relationship(
    catalog: &GraphqlSemanticCatalog,
    entity: &GraphqlEntitySemanticMetadata,
    relationship: &GraphqlRelationshipKeyMetadata,
) -> Result<(), GraphqlSemanticError> {
    let mismatch =
        || GraphqlSemanticError::new("client metadata relationship does not match the catalogue");
    let field = semantic_field(entity, &relationship.field_name).ok_or_else(mismatch)?;
    let descriptor = field.relationship.as_ref().ok_or_else(mismatch)?;
    if descriptor.target != relationship.target {
        return Err(mismatch());
    }
    let GraphqlRelationshipKeyMapping::Batchable {
        source_fields,
        target_fields,
        target_filterable,
    } = &relationship.key_mapping
    else {
        return Ok(());
    };
    let exposed = |owner: &str, name: &String| relationship_key_field_exposed(catalog, owner, name);
    if !source_fields
        .iter()
        .all(|name| exposed(&entity.entity_name, name))
        || !target_fields
            .iter()
            .all(|name| exposed(&relationship.target, name))
    {
        return Err(mismatch());
    }
    let filterable = relationship_target_filterable(catalog, &relationship.target, target_fields);
    if *target_filterable != filterable {
        return Err(mismatch());
    }
    Ok(())
}

/// Whether every target field accepts `Equal` and `In` in the target entity's
/// generated filter. Producers use this to fill
/// [`GraphqlRelationshipKeyMapping::Batchable::target_filterable`].
pub fn relationship_target_filterable(
    catalog: &GraphqlSemanticCatalog,
    target: &str,
    target_fields: &[String],
) -> bool {
    let Some(target) = catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == target)
    else {
        return false;
    };
    target_fields.iter().all(|name| {
        semantic_field(target, name).is_some_and(|field| {
            field
                .filter_operators
                .contains(&GraphqlSemanticFilterOperator::Equal)
                && field
                    .filter_operators
                    .contains(&GraphqlSemanticFilterOperator::In)
        })
    })
}

/// Whether `field_name` names a selectable, non-list scalar or enum field of
/// `entity_name` with no field policy and a classification below secret: a
/// key field a client can read and send back. Producers use this to choose
/// [`GraphqlRelationshipKeyMapping::NoExposedKey`].
pub fn relationship_key_field_exposed(
    catalog: &GraphqlSemanticCatalog,
    entity_name: &str,
    field_name: &str,
) -> bool {
    catalog
        .entities
        .iter()
        .find(|entity| entity.entity_name == entity_name)
        .and_then(|entity| semantic_field(entity, field_name))
        .is_some_and(is_exposed_leaf)
}

/// A plain leaf a client may read and send back: no field policy, and not
/// classified secret. Lifetime fields and relationship key fields share it.
fn is_exposed_leaf(field: &GraphqlSemanticFieldMetadata) -> bool {
    is_plain_leaf(field)
        && !field.has_field_policy
        && field.classification < super::GraphqlSemanticClassification::Secret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{
        GraphqlSemanticClassification, GraphqlSemanticExport, GraphqlSemanticOperationDescriptor,
        GraphqlSemanticRelationshipCardinality, GraphqlSemanticRelationshipDescriptor,
        GraphqlSubscriptionObservationDescriptor, GraphqlSubscriptionReplayMode,
    };

    fn field(name: &str, type_ref: GraphqlSemanticTypeRef) -> GraphqlSemanticFieldMetadata {
        GraphqlSemanticFieldMetadata {
            field_name: name.into(),
            description: format!("The {name}."),
            type_ref,
            selectable: true,
            filter_operators: Vec::new(),
            filter_input: None,
            sortable: false,
            groupable: false,
            aggregate_operators: Vec::new(),
            aggregate_value_kind: None,
            relationship: None,
            classification: GraphqlSemanticClassification::Internal,
            export: GraphqlSemanticExport::Exportable,
            has_field_policy: false,
        }
    }

    fn scalar(name: &str, nullable: bool) -> GraphqlSemanticTypeRef {
        GraphqlSemanticTypeRef::named(name, GraphqlSemanticTypeKind::Scalar, nullable)
    }

    fn entity(
        name: &str,
        fields: Vec<GraphqlSemanticFieldMetadata>,
    ) -> GraphqlEntitySemanticMetadata {
        GraphqlEntitySemanticMetadata {
            entity_name: name.into(),
            description: format!("A {name}."),
            default_classification: GraphqlSemanticClassification::Internal,
            fields: fields.into_boxed_slice(),
            generated_input_members: Vec::new(),
            label_fields: Vec::new(),
            record_version: None,
        }
    }

    fn catalog() -> GraphqlSemanticCatalog {
        let mut policy_key = field("SecretParentId", scalar("String", false));
        policy_key.has_field_policy = true;
        let mut secret_key = field("SensitiveParentId", scalar("String", false));
        secret_key.classification = GraphqlSemanticClassification::Secret;
        secret_key.export = GraphqlSemanticExport::NeverExport;
        let mut parent = field(
            "Parent",
            GraphqlSemanticTypeRef::named("Parent", GraphqlSemanticTypeKind::Object, true),
        );
        parent.relationship = Some(GraphqlSemanticRelationshipDescriptor {
            target: "Parent".into(),
            cardinality: GraphqlSemanticRelationshipCardinality::One,
            arguments: Vec::new(),
            collection_bound: None,
        });
        let child = entity(
            "Child",
            vec![
                field("Id", scalar("String", false)),
                field("ParentId", scalar("String", false)),
                policy_key,
                secret_key,
                field(
                    "Tags",
                    GraphqlSemanticTypeRef::list(false, Some(8), scalar("String", false)),
                ),
                field("ExpiresAt", scalar("Int", true)),
                parent,
            ],
        );
        let parent_entity = entity("Parent", vec![field("Id", scalar("String", false))]);
        let changed = GraphqlSemanticOperationDescriptor::generated(
            GraphqlOperationKind::Subscription,
            GeneratedGraphqlOperationCategory::Subscription,
            "Child",
            "ChildChanged",
            "Observe changes to Child records.",
            Vec::new(),
            GraphqlSemanticTypeRef::named(
                "ChildChangedEvent",
                GraphqlSemanticTypeKind::Object,
                false,
            ),
            false,
        )
        .and_then(|descriptor| {
            descriptor.with_subscription_observation(GraphqlSubscriptionObservationDescriptor {
                replay_mode: GraphqlSubscriptionReplayMode::BestEffort,
                maximum_duration_seconds: None,
                maximum_events: None,
                condition_fields: Vec::new(),
            })
        })
        .expect("subscription descriptor");
        GraphqlSemanticCatalog::compose_descriptors([parent_entity, child], [changed])
            .expect("catalogue")
    }

    fn child_facts() -> GraphqlClientEntityMetadata {
        GraphqlClientEntityMetadata::new("Child")
            .with_changed_stream(GraphqlChangedStreamMetadata::new(
                "ChildChanged",
                vec!["Id".into(), "ParentId".into(), "ExpiresAt".into()],
                true,
                false,
            ))
            .with_lifetime(GraphqlEntityLifetimeMetadata::new(
                "ExpiresAt",
                GraphqlLifetimeEncoding::UnixSeconds,
            ))
            .with_relationship(GraphqlRelationshipKeyMetadata::new(
                "Parent",
                "Parent",
                GraphqlRelationshipKeyMapping::Batchable {
                    source_fields: vec!["ParentId".into()],
                    target_fields: vec!["Id".into()],
                    target_filterable: false,
                },
            ))
    }

    /// The semantic catalogue and client metadata hold no fraction, large
    /// integer or non-ASCII key, so their existing canonical bytes are also
    /// their RFC 8785 bytes. Neither encoding nor any fingerprint moved.
    #[test]
    fn existing_canonical_bytes_equal_their_rfc_8785_form() {
        let catalog = catalog();
        let metadata = GraphqlClientMetadata::compose(&catalog, [child_facts()]).unwrap();
        for value in [
            serde_json::to_value(&catalog).unwrap(),
            serde_json::to_value(&metadata).unwrap(),
        ] {
            assert_eq!(
                canonical_json_bytes(&value),
                crate::rfc8785_canonical_json(&value).unwrap()
            );
        }
    }

    #[test]
    fn composes_canonically_and_round_trips() {
        let catalog = catalog();
        let metadata = GraphqlClientMetadata::compose(
            &catalog,
            [child_facts(), GraphqlClientEntityMetadata::new("Parent")],
        )
        .expect("compose");
        // The entity without facts is dropped.
        assert_eq!(metadata.entities.len(), 1);
        assert_eq!(metadata.semantic_catalog_fingerprint, catalog.fingerprint);
        let exported = metadata.export_json().expect("export");
        let decoded = GraphqlClientMetadata::from_export_json(&exported).expect("decode");
        assert_eq!(decoded, metadata);
        decoded.validate_against(&catalog).expect("bound");
        // The export is deterministic.
        assert_eq!(decoded.export_json().expect("export"), exported);
    }

    #[test]
    fn refuses_stale_and_rebound_documents() {
        let catalog = catalog();
        let metadata = GraphqlClientMetadata::compose(&catalog, [child_facts()]).expect("compose");
        let mut stale = metadata.clone();
        stale.entities[0].lifetime = None;
        assert_eq!(
            stale.validate().unwrap_err().message(),
            "client metadata fingerprint is stale"
        );
        let mut rebound = metadata.clone();
        rebound.semantic_catalog_fingerprint = "0".repeat(64);
        rebound.fingerprint = rebound.compute_fingerprint();
        assert_eq!(
            rebound.validate_against(&catalog).unwrap_err().message(),
            "client metadata describes another semantic catalogue"
        );
        assert!(GraphqlClientMetadata::from_export_json("{\"version\":1}").is_err());
    }

    #[test]
    fn refuses_names_the_catalogue_does_not_publish_in_that_role() {
        let catalog = catalog();
        let compose = |facts: GraphqlClientEntityMetadata| {
            GraphqlClientMetadata::compose(&catalog, [facts]).map(|_| ())
        };
        let lifetime = |field: &str| {
            GraphqlClientEntityMetadata::new("Child").with_lifetime(
                GraphqlEntityLifetimeMetadata::new(field, GraphqlLifetimeEncoding::Rfc3339Utc),
            )
        };
        // A list field, a policy field and an unknown field cannot be lifetimes.
        for field in ["Tags", "SecretParentId", "Missing"] {
            assert!(compose(lifetime(field)).is_err(), "{field}");
        }
        // A batchable mapping may not name a policy-guarded key.
        let policy_key = GraphqlClientEntityMetadata::new("Child").with_relationship(
            GraphqlRelationshipKeyMetadata::new(
                "Parent",
                "Parent",
                GraphqlRelationshipKeyMapping::Batchable {
                    source_fields: vec!["SecretParentId".into()],
                    target_fields: vec!["Id".into()],
                    target_filterable: false,
                },
            ),
        );
        assert!(compose(policy_key).is_err());
        // A mapping must name the relationship's real target.
        let wrong_target = GraphqlClientEntityMetadata::new("Child").with_relationship(
            GraphqlRelationshipKeyMetadata::new(
                "Parent",
                "Child",
                GraphqlRelationshipKeyMapping::NoExposedKey,
            ),
        );
        assert!(compose(wrong_target).is_err());
        // A stream must name a generated subscription root of the entity and
        // carry its fields in field order.
        for (root, fields) in [
            ("OtherChanged", vec!["Id".to_owned()]),
            ("ChildChanged", vec!["ParentId".to_owned(), "Id".to_owned()]),
            ("ChildChanged", vec!["Missing".to_owned()]),
        ] {
            let facts = GraphqlClientEntityMetadata::new("Child")
                .with_changed_stream(GraphqlChangedStreamMetadata::new(root, fields, true, false));
            assert!(compose(facts).is_err(), "{root}");
        }
        // A mapping that claims a filterable target must be one.
        let filterable = GraphqlClientEntityMetadata::new("Child").with_relationship(
            GraphqlRelationshipKeyMetadata::new(
                "Parent",
                "Parent",
                GraphqlRelationshipKeyMapping::Batchable {
                    source_fields: vec!["ParentId".into()],
                    target_fields: vec!["Id".into()],
                    target_filterable: true,
                },
            ),
        );
        assert!(compose(filterable).is_err());
        assert!(!relationship_target_filterable(
            &catalog,
            "Parent",
            &["Id".into()]
        ));
        assert!(relationship_key_field_exposed(
            &catalog, "Child", "ParentId"
        ));
        assert!(!relationship_key_field_exposed(
            &catalog,
            "Child",
            "SecretParentId"
        ));
        // A secret-classified key is not exposed, so no mapping may name it,
        // and it cannot be a lifetime field either.
        assert!(!relationship_key_field_exposed(
            &catalog,
            "Child",
            "SensitiveParentId"
        ));
        let secret_key = GraphqlClientEntityMetadata::new("Child").with_relationship(
            GraphqlRelationshipKeyMetadata::new(
                "Parent",
                "Parent",
                GraphqlRelationshipKeyMapping::Batchable {
                    source_fields: vec!["SensitiveParentId".into()],
                    target_fields: vec!["Id".into()],
                    target_filterable: false,
                },
            ),
        );
        assert!(compose(secret_key).is_err());
        assert!(compose(lifetime("SensitiveParentId")).is_err());
    }
}
